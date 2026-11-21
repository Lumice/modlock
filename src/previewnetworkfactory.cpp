#include "previewnetworkfactory.h"
#include <QBuffer>
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSet>
#include <QCryptographicHash>
#include <QRunnable>
#include <utility>
#include <chrono>
#include <limits>
#include <algorithm>
#include <QImageReader>
#include <QLockFile>
#include <QNetworkAccessManager>
#include <QNetworkCacheMetaData>
#include <QNetworkDiskCache>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QThreadPool>
#include <QTimer>
#include <QUuid>
#include <deque>

namespace {
constexpr qint64 BodyLimit = 8LL * 1024 * 1024;
constexpr int Chunk = 65536, ActiveLimit = 4, WaitingLimit = 64, ControlLimit = 200;
constexpr int ControlBytes = 8000, WaitingBytes = 512000;
PreviewNetworkFactory *currentFactory = nullptr;
bool safeUrl(const QUrl &url) {
    return url.isValid() && url.scheme() == "https" && !url.host().isEmpty() && url.userInfo().isEmpty()
        && !url.hasFragment() && (url.port(-1) == -1 || url.port() == 443)
        && url.toEncoded().size() <= 7000 && url.host() != QStringLiteral("native-preview.invalid");
}
QByteArray imageMime(QByteArray value) {
    value = value.split(';').first().trimmed().toLower();
    return value == "image/png" || value == "image/jpeg" || value == "image/webp" || value == "image/gif" ? value : QByteArray();
}
bool mimeMatches(const QByteArray &mime, const QByteArray &format) {
    return (mime == "image/png" && format == "png") || (mime == "image/jpeg" && (format == "jpeg" || format == "jpg"))
        || (mime == "image/webp" && format == "webp") || (mime == "image/gif" && format == "gif");
}
struct DecodeResult { QImage thumb, gallery; QString error; };
DecodeResult decode(const QByteArray &bytes, const QByteArray &mime) {
    QBuffer buffer; buffer.setData(bytes); buffer.open(QIODevice::ReadOnly);
    QImageReader reader(&buffer);
    reader.setAutoTransform(true);
    const auto size = reader.size();
    const qint64 pixels = qint64(size.width()) * size.height();
    if (!mimeMatches(mime, reader.format()) || size.width() < 1 || size.height() < 1 || pixels > 32000000
        || pixels > (64LL * 1024 * 1024) / 8)
        return {{}, {}, QStringLiteral("Unsupported image or source allocation limit exceeded")};
    // Eight bytes/pixel conservatively covers Qt's 64-bit formats even if a plugin ignores scaled decode.
    reader.setScaledSize(size.scaled(QSize(1920,1080), Qt::KeepAspectRatio));
    auto image = reader.read(); // Static first frame only; never iterate an animation.
    if (image.isNull() || image.sizeInBytes() > 64LL * 1024 * 1024)
        return {{}, {}, QStringLiteral("Corrupt image or decode allocation limit exceeded")};
    if (image.width() > 1920 || image.height() > 1080)
        image = image.scaled(QSize(1920,1080), Qt::KeepAspectRatio, Qt::SmoothTransformation);
    auto thumb = image.scaled(QSize(530,350), Qt::KeepAspectRatio, Qt::SmoothTransformation);
    return {std::move(thumb), std::move(image), {}};
}
constexpr auto OriginalUrl = static_cast<QNetworkRequest::Attribute>(QNetworkRequest::User + 71);
constexpr auto CacheSequence = static_cast<QNetworkRequest::Attribute>(QNetworkRequest::User + 72);
QUrl slotUrl(const QUrl &url, int slot) {
    const auto key = QCryptographicHash::hash(url.toEncoded(), QCryptographicHash::Sha256).toHex();
    return QUrl(QStringLiteral("https://native-preview.invalid/v1/") + QString::fromLatin1(key) + '/' + QString::number(slot));
}
struct Candidate { QNetworkCacheMetaData metadata; quint64 sequence = 0; int slot = -1; };
int controlSize(const std::shared_ptr<PreviewControl> &control) {
    return QJsonDocument(QJsonObject{{"url", control->url.toString(QUrl::FullyEncoded)},
        {"request", control->token}, {"gallery", control->gallery}, {"proxy", control->proxy}}).toJson(QJsonDocument::Compact).size() + 128;
}
struct Job {
    QString key;
    QUrl url, effectiveUrl;
    QList<std::shared_ptr<PreviewControl>> subscribers;
    QByteArray bytes, mime;
    QNetworkCacheMetaData metadata;
    QList<Candidate> candidates;
    quint64 sequence = 0;
    int loadedSlot = -1;
    quint64 epoch = 0;
    QNetworkReply *reply = nullptr;
    QTimer *total = nullptr;
    int redirects = 0, controlBytes = 0;
    bool cached = false, stale = false, refreshing = false, refreshAttempted = false, preemptCache = false, decoding = false, held = false, aborted = false;
    QString cacheProblem;
};
}

class PreviewService final : public QObject {
    Q_OBJECT
public:
    PreviewService(QString directory, QString root, std::shared_ptr<PreviewBridge> bridge)
        : m_directory(std::move(directory)), m_root(std::move(root)), m_bridge(std::move(bridge)) {
        m_decoders.setParent(this);
        m_decoders.setMaxThreadCount(2);
        m_decoders.setExpiryTimeout(-1);
    }
    void initialize() {
        m_network = new QNetworkAccessManager(this);
        connect(m_network, &QNetworkAccessManager::authenticationRequired, this, [](QNetworkReply *reply, QAuthenticator *) { reply->abort(); });
        // Lock lives outside the disk-cache directory; no cache API/files are touched when another owner holds it.
        if (!m_directory.isEmpty()) {
            const QFileInfo directory(m_directory);
            if (QDir().mkpath(directory.absolutePath())) {
                // Existing directory aliases must name the same lock and cache owner.
                const auto existing = directory.canonicalFilePath();
                m_directory = existing.isEmpty()
                    ? QDir(QDir(directory.absolutePath()).canonicalPath()).filePath(directory.fileName()) : existing;
                m_lock = std::make_unique<QLockFile>(m_directory + QStringLiteral(".lock"));
                m_lock->setStaleLockTime(0);
                if (m_lock->tryLock(0)) {
                    m_cache = new QNetworkDiskCache(this);
                    m_cache->setCacheDirectory(m_directory);
                    m_cache->setMaximumCacheSize(128LL * 1024 * 1024);
                }
            }
        }
        m_cacheNotice = m_cache ? QString() : QStringLiteral("Disk preview cache unavailable; network-only artwork");
    }
    void submit(const std::shared_ptr<PreviewControl> &control) {
        if (m_closing || control->cancelled || !safeUrl(control->url)) {
            publish(control, PreviewStatus::Unavailable, QStringLiteral("Preview unavailable or cancelled"));
            complete(control, {}, {}, {}, false, false, QStringLiteral("Preview unavailable or cancelled")); return;
        }
        const int cost = controlSize(control);
        if (cost > ControlBytes) {
            publish(control, PreviewStatus::Unavailable, QStringLiteral("Preview control exceeds the request limit"));
            complete(control, {}, {}, {}, false, false, QStringLiteral("Preview request too large")); return;
        }
        const auto key = QString::fromLatin1(QCryptographicHash::hash(m_root.toUtf8() + '\n' + control->url.toEncoded() + "\nvisible", QCryptographicHash::Sha256).toHex());
        bool blockedByLease = false;
        for (const auto &job : m_active) {
            if (job->key == key && job->held) blockedByLease = true;
            if (job->key == key && !job->aborted && !job->held) {
                job->subscribers.append(control);
                if (job->refreshing) {
                    // Visible demand supersedes this disposable refresh. Retain the charged
                    // job until its reply/decoder finishes, then use its validated cache.
                    job->preemptCache = true;
                    if (job->reply) job->reply->abort();
                }
                return;
            }
        }
        for (const auto &job : m_waiting) {
            if (job->key == key) {
                if (job->controlBytes + cost > ControlBytes || m_waitingBytes + cost > WaitingBytes) { busy(control); return; }
                job->controlBytes += cost; m_waitingBytes += cost; job->subscribers.append(control); return;
            }
        }
        if ((m_active.size() >= ActiveLimit || blockedByLease) && (m_waiting.size() >= WaitingLimit || m_waitingBytes + cost > WaitingBytes)) {
            busy(control); return;
        }
        auto job = std::make_shared<Job>(); job->key = key; job->url = control->url;
        job->controlBytes = cost; job->subscribers.append(control);
        if (m_active.size() < ActiveLimit && !blockedByLease) start(job);
        else {
            m_waiting.push_back(job); m_waitingBytes += cost;
            // Refreshes may have started before the grid's later delegates arrived.
            // Drain them now so visible cache reads do not wait on offline HTTP.
            const auto active = m_active;
            for (const auto &refresh : active) {
                if (refresh->refreshing && refresh->reply && !refresh->held && !refresh->decoding) {
                    refresh->aborted = true;
                    refresh->reply->abort();
                }
            }
        }
    }
    void cancel(const std::shared_ptr<PreviewControl> &control) {
        bool fullControlCapacityFreed = false;
        { QMutexLocker lock(&m_bridge->mutex);
          fullControlCapacityFreed = m_bridge->controls.size() >= ControlLimit;
          m_bridge->controls.remove(control->token); }
        // Before provider attach there is no response to finish. attach observes the atomic cancellation.
        for (auto it = m_waiting.begin(); it != m_waiting.end(); ++it) {
            auto job = *it;
            if (job->subscribers.removeOne(control)) {
                const int cost = controlSize(control);
                job->controlBytes -= cost; m_waitingBytes -= cost;
                complete(control, {}, {}, {}, false, false, QStringLiteral("Cancelled"));
                if (job->subscribers.isEmpty()) m_waiting.erase(it);
                emit capacityAvailable(); return;
            }
        }
        for (const auto &job : m_active) {
            if (job->subscribers.removeOne(control)) {
                complete(control, {}, {}, {}, false, false, QStringLiteral("Cancelled"));
                if (job->subscribers.isEmpty()) {
                    job->aborted = true;
                    if (job->reply) job->reply->abort();
                    else if (!job->decoding && !job->held) retire(job);
                }
                emit capacityAvailable(); return;
            }
        }
        if (!control->busy || fullControlCapacityFreed) emit capacityAvailable();
    }
    void shutdown() {
        m_closing = true;
        for (auto &job : m_waiting) for (auto &control : job->subscribers)
            complete(control, {}, {}, {}, false, false, QStringLiteral("Preview service closing"));
        m_waiting.clear(); m_waitingBytes = 0;
        for (auto &job : m_active) {
            job->aborted = true;
            if (job->reply) { job->reply->disconnect(this); job->reply->abort(); job->reply->deleteLater(); job->reply = nullptr; }
            if (job->total) job->total->stop();
        }
        // Workers never touch responses. Retain jobs/bytes until all actual workers have returned.
        m_decoders.waitForDone();
        for (auto &job : m_active) for (auto &control : job->subscribers)
            complete(control, {}, {}, {}, false, false, QStringLiteral("Preview service closing"));
        m_active.clear();
        delete m_cache; m_cache = nullptr;
        delete m_network; m_network = nullptr;
        m_lock.reset();
    }
signals:
    void status(QString token, QUrl url, PreviewStatus::State state, QString detail);
    void capacityAvailable();
private:
    void complete(const std::shared_ptr<PreviewControl> &control, const QImage &image,
                  std::shared_ptr<PreviewBody> body, QByteArray mime, bool cached, bool stale, QString error) {
        auto callback = std::move(control->complete);
        if (callback) callback(image, std::move(body), std::move(mime), cached, stale, std::move(error));

    }
    void publish(const std::shared_ptr<PreviewControl> &control, PreviewStatus::State state, QString detail) {
        if (!control->cancelled) emit status(control->token, control->url, state, detail);
    }
    void busy(const std::shared_ptr<PreviewControl> &control) {
        control->busy = true;
        publish(control, PreviewStatus::Busy, QStringLiteral("Artwork busy; retry when capacity is available"));
        complete(control, {}, {}, {}, false, false, QStringLiteral("Preview service busy (retriable)"));
    }
    void start(const std::shared_ptr<Job> &job) {
        m_active.append(job); lookupCache(job);
    }
    void lookupCache(const std::shared_ptr<Job> &job) {
        job->candidates.clear(); job->sequence = 0; job->loadedSlot = -1;
        if (m_cache && !m_cacheReadDisabled && !m_noStore.contains(job->url)) {
            for (int slot = 0; slot < 2; ++slot) {
                const auto key = slotUrl(job->url, slot);
                auto metadata = m_cache->metaData(key);
                const auto attributes = metadata.attributes();
                bool ok = false;
                const auto sequence = attributes.value(CacheSequence).toULongLong(&ok);
                if (metadata.isValid() && metadata.url() == key && attributes.value(OriginalUrl).toUrl() == job->url
                    && ok && sequence > 0 && sequence < std::numeric_limits<quint64>::max()) {
                    job->sequence = qMax(job->sequence, sequence);
                    job->candidates.append({metadata, sequence, slot});
                }
            }
            std::sort(job->candidates.begin(), job->candidates.end(), [](const Candidate &a, const Candidate &b) { return a.sequence > b.sequence; });
            auto legacy = m_cache->metaData(job->url);
            if (legacy.isValid() && legacy.url() == job->url) job->candidates.append({legacy, 0, -1});
        }
        bool disallowed = false;
        for (const auto &candidate : job->candidates) for (const auto &header : candidate.metadata.rawHeaders())
            if ((header.first.compare("cache-control", Qt::CaseInsensitive) == 0 && header.second.toLower().contains("no-store"))
                || header.first.compare("set-cookie", Qt::CaseInsensitive) == 0) disallowed = true;
        if (disallowed) {
            job->candidates.clear(); job->cacheProblem = QStringLiteral("Retained artwork disallowed by storage policy");
        }
        cachedCandidate(job);
    }
    void cachedCandidate(const std::shared_ptr<Job> &job) {
        while (!job->candidates.isEmpty()) {
            const auto candidate = job->candidates.takeFirst(); job->metadata = candidate.metadata;
            qint64 declared = -1; int headerBytes = 0; bool noStore = false;
            job->mime.clear(); job->bytes.clear(); job->bytes.reserve(BodyLimit);
            for (const auto &header : job->metadata.rawHeaders()) {
                headerBytes += header.first.size() + header.second.size();
                if (header.first.compare("content-type", Qt::CaseInsensitive) == 0) job->mime = imageMime(header.second);
                if (header.first.compare("content-length", Qt::CaseInsensitive) == 0) {
                    bool ok = false; declared = header.second.toLongLong(&ok); if (!ok || declared < 0) declared = BodyLimit + 1;
                }
                if (header.first.compare("cache-control", Qt::CaseInsensitive) == 0 && header.second.toLower().contains("no-store")) noStore = true;
            }
            if (!job->mime.isEmpty() && declared <= BodyLimit && headerBytes <= 65536 && !noStore) {
                // Use only the public cache API, never an inferred backing-file path.
                std::unique_ptr<QIODevice> device(m_cache->data(job->metadata.url()));
                if (device && (device->isSequential() || device->size() <= BodyLimit)) {
                    while (job->bytes.size() < BodyLimit) {
                        auto chunk = device->read(qMin<qint64>(Chunk, BodyLimit - job->bytes.size()));
                        if (chunk.isEmpty()) break;
                        job->bytes.append(chunk);
                    }
                    if (!job->bytes.isEmpty() && device->atEnd() && (declared < 0 || declared == job->bytes.size())) {
                        job->cached = true; job->loadedSlot = candidate.slot;
                        job->stale = !job->metadata.expirationDate().isValid() || job->metadata.expirationDate() <= QDateTime::currentDateTimeUtc();
                        decodeJob(job); return;
                    }
                }
            }
            job->cacheProblem = QStringLiteral("Corrupt, disallowed, unsupported or oversized cached image; trying retained fallback");
        }
        job->cached = false; job->bytes.clear(); job->mime.clear(); network(job, job->url);
    }
    void network(const std::shared_ptr<Job> &job, const QUrl &url) {
        if (job->aborted || !safeUrl(url)) { fail(job, QStringLiteral("Unsafe preview redirect")); return; }
        if (!job->total) {
            job->total = new QTimer(this); job->total->setSingleShot(true); job->total->setInterval(30000);
            auto *timer = job->total;
            connect(timer, &QTimer::timeout, this, [this, job, timer] {
                if (job->total != timer) return;
                if (job->reply) job->reply->abort(); else fail(job, QStringLiteral("Preview total timeout"));
            });
            job->total->start();
        }
        if (job->bytes.isEmpty()) job->bytes.reserve(BodyLimit);
        QNetworkRequest request(url);
        request.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::ManualRedirectPolicy);
        request.setAttribute(QNetworkRequest::CacheLoadControlAttribute, QNetworkRequest::AlwaysNetwork);
        request.setAttribute(QNetworkRequest::CacheSaveControlAttribute, false);
        request.setAttribute(QNetworkRequest::CookieLoadControlAttribute, QNetworkRequest::Manual);
        request.setAttribute(QNetworkRequest::CookieSaveControlAttribute, QNetworkRequest::Manual);
        request.setAttribute(QNetworkRequest::AuthenticationReuseAttribute, QNetworkRequest::Manual);
        request.setTransferTimeout(std::chrono::seconds(20));
        request.setRawHeader("Accept", "image/png,image/jpeg,image/webp,image/gif");
        request.setRawHeader("Accept-Encoding", "identity");
        job->effectiveUrl = url;
        const auto epoch = ++job->epoch;
        auto *reply = m_network->get(request); job->reply = reply; reply->setReadBufferSize(Chunk);
        connect(reply, &QNetworkReply::readyRead, this, [this, job, reply, epoch] {
            if (job->reply == reply && job->epoch == epoch) read(job);
        });
        connect(reply, &QNetworkReply::finished, this, [this, job, reply, epoch] {
            if (job->reply != reply || job->epoch != epoch) return;
            read(job);
            if (job->reply != reply) return;
            const auto statusCode = reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();
            const auto redirect = reply->attribute(QNetworkRequest::RedirectionTargetAttribute).toUrl();
            job->reply = nullptr;
            if (job->preemptCache && !job->aborted) {
                reply->deleteLater(); fallbackCache(job); return;
            }
            if (!job->aborted && reply->error() == QNetworkReply::NoError && statusCode >= 300 && statusCode < 400 && !redirect.isEmpty()) {
                const auto next = reply->url().resolved(redirect); reply->deleteLater(); job->bytes.clear();
                if (++job->redirects > 3 || !safeUrl(next)) { fail(job, QStringLiteral("Preview redirect limit or HTTPS policy")); return; }
                network(job, next); return;
            }
            if (job->aborted || reply->error() != QNetworkReply::NoError || statusCode != 200 || job->bytes.isEmpty()) {
                const auto error = job->aborted ? QStringLiteral("Cancelled") : QStringLiteral("Preview unavailable: incomplete or failed network response");
                reply->deleteLater(); fail(job, error); return;
            }
            job->mime = imageMime(reply->rawHeader("Content-Type"));
            if (job->mime.isEmpty()) { reply->deleteLater(); fail(job, QStringLiteral("Unsupported preview MIME")); return; }
            job->metadata = QNetworkCacheMetaData(); job->metadata.setUrl(job->url);
            QNetworkCacheMetaData::RawHeaderList headers;
            int headerBytes = 0;
            for (const auto &header : reply->rawHeaderPairs()) {
                headerBytes += header.first.size() + header.second.size(); headers.append(header);
            }
            if (headerBytes > 65536 || (!reply->rawHeader("Content-Encoding").isEmpty() && reply->rawHeader("Content-Encoding").toLower() != "identity")) {
                reply->deleteLater(); fail(job, QStringLiteral("Unsupported preview response headers or encoding")); return;
            }
            job->metadata.setRawHeaders(headers);
            // Preserve provider expiry/last-modified; absence stays invalid/expired, never manufactured fresh.
            job->metadata.setExpirationDate(QDateTime::fromString(QString::fromLatin1(reply->rawHeader("Expires")), Qt::RFC2822Date));
            job->metadata.setLastModified(QDateTime::fromString(QString::fromLatin1(reply->rawHeader("Last-Modified")), Qt::RFC2822Date));
            const auto cacheControl = reply->rawHeader("Cache-Control").toLower();
            const bool noStore = cacheControl.contains("no-store") || reply->hasRawHeader("Set-Cookie");
            job->metadata.setSaveToDisk(!noStore);
            if (noStore) {
                // Bounded conservative policy memory applies to every slot and legacy lookup.
                if (m_noStore.size() < ControlLimit) m_noStore.insert(job->url);
                else m_cacheReadDisabled = true;
            }
            reply->deleteLater();
            if (job->total) job->total->stop();
            decodeJob(job);
        });
    }
    void read(const std::shared_ptr<Job> &job) {
        if (!job->reply || job->aborted) return;
        const auto declared = job->reply->header(QNetworkRequest::ContentLengthHeader);
        if (declared.isValid() && (declared.toLongLong() < 0 || declared.toLongLong() > BodyLimit)) {
            job->aborted = true; job->reply->abort(); return;
        }
        while (job->reply->bytesAvailable() > 0) {
            if (job->bytes.size() == BodyLimit) { job->aborted = true; job->reply->abort(); return; }
            auto chunk = job->reply->read(qMin<qint64>(Chunk, BodyLimit - job->bytes.size()));
            if (chunk.isEmpty()) break;
            job->bytes.append(chunk);
            if (job->bytes.size() > BodyLimit) { job->bytes.clear(); job->aborted = true; job->reply->abort(); return; }
        }
    }
    void decodeJob(const std::shared_ptr<Job> &job) {
        job->decoding = true;
        const auto epoch = ++job->epoch;
        // At most four active bodies (32 MiB total), including the two executing/two waiting decode inputs.
        m_decoders.start(QRunnable::create([this, job, epoch, bytes = job->bytes, mime = job->mime]() mutable {
            auto result = decode(bytes, mime);
            bytes.clear(); // No worker byte holder survives result publication.
            QMetaObject::invokeMethod(this, [this, job, epoch, result = std::move(result)] { decoded(job, epoch, result); }, Qt::QueuedConnection);
        }));
    }
    void fallbackCache(const std::shared_ptr<Job> &job) {
        job->preemptCache = false; job->refreshing = false; job->cached = false;
        job->bytes.clear();
        if (job->total) { job->total->stop(); job->total->deleteLater(); job->total = nullptr; }
        lookupCache(job);
    }
    void decoded(const std::shared_ptr<Job> &job, quint64 epoch, const DecodeResult &result) {
        if (job->epoch != epoch) return;
        job->decoding = false;
        if (m_closing || !m_active.contains(job)) return;
        if (job->aborted) { retire(job); return; }
        if (!result.error.isEmpty()) {
            if (job->preemptCache) { fallbackCache(job); return; }
            if (job->cached) {
                job->cacheProblem = QStringLiteral("Corrupt or unsupported cached image: ") + result.error;
                job->cached = false; job->bytes.clear(); cachedCandidate(job); return;
            }
            fail(job, result.error); return;
        }
        job->preemptCache = false;
        if (!job->cached && m_cache && job->metadata.saveToDisk()) {
            // Qt 6.10 insert removes the previous same-key entry before commit. Alternate
            // opaque slots preserve the last usable entry and the original legacy URL.
            auto replacement = job->metadata;
            replacement.setUrl(slotUrl(job->url, job->loadedSlot == 0 ? 1 : 0));
            auto attributes = replacement.attributes();
            attributes.insert(OriginalUrl, job->url); attributes.insert(CacheSequence, QVariant::fromValue(job->sequence + 1));
            replacement.setAttributes(attributes);
            if (auto *device = m_cache->prepare(replacement)) {
                bool completeWrite = true;
                for (qint64 offset = 0; offset < job->bytes.size();) {
                    const auto length = qMin<qint64>(Chunk, job->bytes.size() - offset);
                    if (device->write(job->bytes.constData() + offset, length) != length) { completeWrite = false; break; }
                    offset += length;
                }
                if (completeWrite) m_cache->insert(device);
                else {
                    // Cancel this known prepared insertion only. Cache owns/deletes its device.
                    // remove() finds the outstanding insertion before any on-disk URL entry.
                    m_cache->remove(replacement.url());
                }
            }
        }
        const auto subscribers = job->subscribers;
        std::shared_ptr<PreviewBody> body;
        for (const auto &control : subscribers) if (!control->delivered && control->proxy && !control->cancelled) {
            if (!body) {
                body = std::make_shared<PreviewBody>(); body->bytes = std::move(job->bytes); job->held = true;
                const auto bridge = m_bridge;
                body->released = [bridge, job] {
                    QMutexLocker lock(&bridge->mutex);
                    if (!bridge->stopped && bridge->service)
                        QMetaObject::invokeMethod(bridge->service, [service = bridge->service, job] { service->bodyReleased(job); }, Qt::QueuedConnection);
                };
            }
        }
        for (const auto &control : subscribers) {
            if (control->delivered) continue;
            control->delivered = true;
            if (control->cancelled) { complete(control, {}, {}, {}, false, false, QStringLiteral("Cancelled")); continue; }
            QString detail = job->cached ? (job->stale ? QStringLiteral("Cached image; expired artwork retained offline") : QStringLiteral("Cached image")) : QStringLiteral("Refreshed image");
            if (!m_cacheNotice.isEmpty()) detail += QStringLiteral(". ") + m_cacheNotice;
            if (!job->cacheProblem.isEmpty()) detail += QStringLiteral(". ") + job->cacheProblem;
            publish(control, job->cached ? PreviewStatus::Cached : PreviewStatus::Refreshed, detail);
            complete(control, control->gallery ? result.gallery : result.thumb, control->proxy ? body : nullptr,
                     job->mime, job->cached, job->stale, {});
        }
        // Cache-hit image remains stable; background refresh only replaces validated disk bytes.
        job->bytes.clear();
        if (!job->held) afterDelivery(job);
    }
    void bodyReleased(const std::shared_ptr<Job> &job) {
        job->held = false;
        if (m_active.contains(job)) afterDelivery(job);
    }
    void afterDelivery(const std::shared_ptr<Job> &job) {
        if (job->cached && job->stale && !job->refreshAttempted && !job->aborted && !m_closing) {
            job->refreshAttempted = true; job->refreshing = true; job->cached = false; job->bytes.clear();
            // User-visible queued work precedes disposable background refresh.
            if (!m_waiting.empty()) { retire(job); return; }
            network(job, job->url);
        } else retire(job);
    }
    void fail(const std::shared_ptr<Job> &job, const QString &reason) {
        for (const auto &control : job->subscribers) {
            publish(control, control->delivered ? PreviewStatus::Cached : PreviewStatus::Unavailable, control->delivered ? QStringLiteral("Cached image retained; refresh unavailable") : job->cacheProblem.isEmpty() ? reason : job->cacheProblem + QStringLiteral(". ") + reason);
            complete(control, {}, {}, {}, false, false, reason);
        }
        job->subscribers.clear(); retire(job);
    }
    void retire(const std::shared_ptr<Job> &job) {
        if (job->decoding || job->held) return;
        if (job->total) { job->total->stop(); job->total->deleteLater(); job->total = nullptr; }
        job->bytes.clear(); m_active.removeOne(job);
        if (!m_closing) {
            while (m_active.size() < ActiveLimit && !m_waiting.empty()) {
                const auto it = std::find_if(m_waiting.begin(), m_waiting.end(), [this](const auto &waiting) {
                    return std::none_of(m_active.cbegin(), m_active.cend(), [&waiting](const auto &active) { return active->key == waiting->key; });
                });
                if (it == m_waiting.end()) break;
                auto next = *it; m_waiting.erase(it); m_waitingBytes -= next->controlBytes;
                start(next);
            }
            emit capacityAvailable();
        }
    }
    QString m_directory, m_root, m_cacheNotice;
    std::shared_ptr<PreviewBridge> m_bridge;
    std::unique_ptr<QLockFile> m_lock;
    QNetworkAccessManager *m_network = nullptr;
    QNetworkDiskCache *m_cache = nullptr;
    QThreadPool m_decoders;
    QList<std::shared_ptr<Job>> m_active;
    std::deque<std::shared_ptr<Job>> m_waiting;
    int m_waitingBytes = 0;
    QSet<QUrl> m_noStore;
    bool m_closing = false, m_cacheReadDisabled = false;
};

void previewAttach(const std::shared_ptr<PreviewBridge> &bridge, const std::shared_ptr<PreviewControl> &control) {
    if (!control) return;
    QMutexLocker lock(&bridge->mutex);
    if (control->proxy) {
        control->token = QUuid::createUuid().toString(QUuid::Id128);
        if (bridge->controls.size() >= ControlLimit) {
            auto callback = std::move(control->complete);
            lock.unlock();
            if (callback) callback({}, {}, {}, false, false, QStringLiteral("Preview service busy (retriable)"));
            return;
        }
        bridge->controls.insert(control->token, control);
    }
    if (bridge->stopped || !bridge->service) {
        auto callback = std::move(control->complete);
        if (callback) callback({}, {}, {}, false, false, QStringLiteral("Preview service closed"));
        return;
    }
    QMetaObject::invokeMethod(bridge->service, [service = bridge->service, control] { service->submit(control); }, Qt::QueuedConnection);
}
void previewCancel(const std::shared_ptr<PreviewBridge> &bridge, const std::shared_ptr<PreviewControl> &control) {
    if (!control) return;
    if (control->cancelled.exchange(true)) return;
    QMutexLocker lock(&bridge->mutex);
    if (!bridge->stopped && bridge->service)
        QMetaObject::invokeMethod(bridge->service, [service = bridge->service, control] { service->cancel(control); }, Qt::QueuedConnection);
}
namespace {
class PreviewNetworkAccessManager final : public QNetworkAccessManager {
public:
    PreviewNetworkAccessManager(std::shared_ptr<PreviewBridge> bridge, QObject *parent)
        : QNetworkAccessManager(parent), m_bridge(std::move(bridge)) {}
protected:
    QNetworkReply *createRequest(Operation operation, const QNetworkRequest &request, QIODevice * = nullptr) override {
        // The compatibility surface admits only artwork GETs; no parallel remote request bypass.
        auto actual = request;
        if (operation != GetOperation || !safeUrl(request.url())) actual.setUrl(QUrl());
        return new PreviewReply(actual, m_bridge, this);
    }
private:
    std::shared_ptr<PreviewBridge> m_bridge;
};
}
PreviewNetworkFactory::PreviewNetworkFactory(QString directory, QString root) : m_bridge(std::make_shared<PreviewBridge>()) {
    currentFactory = this;
    QImageReader::setAllocationLimit(64);
    auto *service = new PreviewService(std::move(directory), std::move(root), m_bridge);
    m_bridge->service = service;
    service->moveToThread(&m_thread);
    connect(&m_thread, &QThread::started, service, &PreviewService::initialize);
    connect(service, &PreviewService::status, this, &PreviewNetworkFactory::status, Qt::QueuedConnection);
    connect(service, &PreviewService::capacityAvailable, this, &PreviewNetworkFactory::capacityAvailable, Qt::QueuedConnection);
    connect(&m_thread, &QThread::finished, service, &QObject::deleteLater);
    m_thread.setObjectName(QStringLiteral("PreviewService")); m_thread.start();
}
PreviewNetworkFactory::~PreviewNetworkFactory() { shutdown(); currentFactory = nullptr; }
void PreviewNetworkFactory::shutdown() {
    PreviewService *service;
    {
        QMutexLocker lock(&m_bridge->mutex); if (m_bridge->stopped) return;
        m_bridge->stopped = true; service = m_bridge->service;
        for (const auto &control : m_bridge->controls) control->cancelled = true;
        m_bridge->controls.clear();
    }
    disconnect(service, nullptr, this, nullptr);
    QMetaObject::invokeMethod(service, [service] { service->shutdown(); }, Qt::BlockingQueuedConnection);
    m_thread.quit(); m_thread.wait();
    QMutexLocker lock(&m_bridge->mutex); m_bridge->service = nullptr;
}
PreviewNetworkFactory *PreviewNetworkFactory::instance() { return currentFactory; }
QNetworkAccessManager *PreviewNetworkFactory::create(QObject *parent) { return new PreviewNetworkAccessManager(m_bridge, parent); }
PreviewImageProvider *PreviewNetworkFactory::imageProvider() { return new PreviewImageProvider(m_bridge); }
QString PreviewNetworkFactory::acquire(const QUrl &url, bool gallery) {
    if (url.toEncoded().size() > 7000) return {};
    auto control = std::make_shared<PreviewControl>();
    control->token = QUuid::createUuid().toString(QUuid::Id128); control->url = url;
    control->gallery = gallery;
    QMutexLocker lock(&m_bridge->mutex);
    if (m_bridge->stopped || m_bridge->controls.size() >= ControlLimit) return {};
    m_bridge->controls.insert(control->token, control); return control->token;
}
void PreviewNetworkFactory::release(const QString &token) {
    std::shared_ptr<PreviewControl> control;
    { QMutexLocker lock(&m_bridge->mutex); control = m_bridge->controls.value(token); }
    previewCancel(m_bridge, control);

}
#include "previewnetworkfactory.moc"
