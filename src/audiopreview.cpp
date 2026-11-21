#include "audiopreview.h"
#include <QAudioSink>
#include <QAuthenticator>
#include <QMediaDevices>
#include <QNetworkAccessManager>
#include <QNetworkProxy>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <algorithm>

namespace AudioPreview {
namespace {

bool allowedMime(const QByteArray &type) {
    return type == "audio/mpeg" || type == "audio/mp3" || type == "audio/ogg" || type == "application/octet-stream";
}

enum class Headers { Pending, Admitted, Handled };

class SystemOutput final : public Output {
public:
    ~SystemOutput() override { if (m_sink) m_sink->stop(); }
    QString start(const QAudioFormat &format) override {
        const auto device = QMediaDevices::defaultAudioOutput();
        if (device.isNull()) return QStringLiteral("output");
        if (!device.isFormatSupported(format)) return QStringLiteral("outputFormat");
        m_sink = std::make_unique<QAudioSink>(device, format);
        m_sink->setVolume(SessionVolume);
        m_io = m_sink->start();
        if (!m_io || failed()) return QStringLiteral("output");
        return {};
    }
    qsizetype bytesFree() const override { return m_sink ? m_sink->bytesFree() : 0; }
    qsizetype write(const char *data, qsizetype size) override { return m_io ? m_io->write(data, size) : -1; }
    void setPaused(bool paused) override {
        if (!m_sink) return;
        if (paused) m_sink->suspend();
        else m_sink->resume();
    }
    bool drained() const override {
        return !m_sink || m_sink->state() == QtAudio::IdleState || m_sink->state() == QtAudio::StoppedState;
    }
    bool failed() const override {
        const auto error = m_sink ? m_sink->error() : QtAudio::OpenError;
        return error == QtAudio::OpenError || error == QtAudio::IOError || error == QtAudio::FatalError;
    }
private:
    std::unique_ptr<QAudioSink> m_sink;
    QIODevice *m_io = nullptr;
};

}

std::unique_ptr<Output> createSystemOutput() { return std::make_unique<SystemOutput>(); }

Fetch::Fetch(QNetworkAccessManager *network, QObject *parent) : QObject(parent), m_network(network) {
    m_deadline.setSingleShot(true);
    connect(&m_deadline, &QTimer::timeout, this, [this] { fail(QStringLiteral("network")); });
    // No credentials are ever supplied; any challenge ends the fetch.
    connect(m_network, &QNetworkAccessManager::authenticationRequired, this, [this](QNetworkReply *reply, QAuthenticator *) {
        if (reply == m_reply) fail(QStringLiteral("network"));
    });
    connect(m_network, &QNetworkAccessManager::proxyAuthenticationRequired, this, [this](const QNetworkProxy &, QAuthenticator *) {
        if (m_reply) fail(QStringLiteral("network"));
    });
}

Fetch::~Fetch() { drop(); }

QString Fetch::start(const QString &url) {
    RemoteWireUri uri;
    const auto bytes = url.toUtf8();
    if (QString::fromUtf8(bytes) != url || !remoteWireUri(bytes, uri)) {
        m_closed = true;
        return QStringLiteral("policy");
    }
    m_deadline.start(FetchDeadlineMs);
    request(uri);
    return {};
}

void Fetch::cancel() {
    m_closed = true;
    m_deadline.stop();
    drop();
}

void Fetch::request(const RemoteWireUri &uri) {
    m_current = uri;
    m_body.clear();
    m_mime.clear();
    m_announced = -1;
    m_admitted = false;
    const auto url = QUrl::fromEncoded(uri.effective, QUrl::StrictMode);
    // remoteWireUri already enforced the policy; QUrl must agree on the origin.
    if (!url.isValid() || url.scheme() != QLatin1String("https") || url.host() != QString::fromLatin1(uri.host)
        || !url.userInfo().isEmpty() || url.port(443) != 443) {
        fail(QStringLiteral("policy"));
        return;
    }
    QNetworkRequest request(url);
    request.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::ManualRedirectPolicy);
    request.setAttribute(QNetworkRequest::CacheLoadControlAttribute, QNetworkRequest::AlwaysNetwork);
    request.setAttribute(QNetworkRequest::CacheSaveControlAttribute, false);
    request.setAttribute(QNetworkRequest::CookieLoadControlAttribute, QNetworkRequest::Manual);
    request.setAttribute(QNetworkRequest::CookieSaveControlAttribute, QNetworkRequest::Manual);
    request.setAttribute(QNetworkRequest::AuthenticationReuseAttribute, QNetworkRequest::Manual);
    request.setMaximumRedirectsAllowed(0);
    request.setTransferTimeout(TransferTimeoutMs);
    request.setRawHeader("Accept", "audio/mpeg, audio/ogg");
    request.setRawHeader("Accept-Encoding", "identity");
    request.setRawHeader("User-Agent", "Modlock/1 preview");
    m_reply = m_network->get(request);
    m_reply->setReadBufferSize(64 * 1024);
    connect(m_reply, &QNetworkReply::metaDataChanged, this, [this] { admitHeaders(); });
    connect(m_reply, &QNetworkReply::readyRead, this, &Fetch::readBody);
    connect(m_reply, &QNetworkReply::finished, this, &Fetch::done);
}

bool Fetch::admitHeaders() {
    if (m_admitted) return true;
    if (!m_reply || m_closed) return false;
    const auto status = m_reply->attribute(QNetworkRequest::HttpStatusCodeAttribute);
    if (!status.isValid()) return false;
    const int code = status.toInt();
    if (code == 301 || code == 302 || code == 303 || code == 307 || code == 308) {
        // Every raw Location goes through the same GameBanana policy as the first URL.
        const auto location = m_reply->rawHeader("Location");
        RemoteWireUri next;
        if (++m_redirects > RedirectLimit) { fail(QStringLiteral("redirects")); return false; }
        if (location.isEmpty() || !remoteResolveLocation(m_current, location, next)) { fail(QStringLiteral("policy")); return false; }
        drop();
        request(next);
        return false;
    }
    if (code != 200) { fail(QStringLiteral("network")); return false; }
    const auto encoding = m_reply->rawHeader("Content-Encoding").trimmed().toLower();
    if (!encoding.isEmpty() && encoding != "identity") { fail(QStringLiteral("unsupported")); return false; }
    const auto type = m_reply->rawHeader("Content-Type").split(';').first().trimmed().toLower();
    if (!allowedMime(type)) { fail(QStringLiteral("unsupported")); return false; }
    if (m_reply->hasRawHeader("Content-Length")) {
        const auto raw = m_reply->rawHeader("Content-Length").trimmed();
        bool ok = !raw.isEmpty() && raw.size() <= 18 && std::all_of(raw.cbegin(), raw.cend(), [](char c) { return c >= '0' && c <= '9'; });
        const auto length = ok ? raw.toLongLong(&ok) : -1;
        if (!ok || length <= 0) { fail(QStringLiteral("damaged")); return false; }
        if (length > BodyLimit) { fail(QStringLiteral("tooLarge")); return false; }
        m_announced = length;
    }
    m_mime = type;
    m_admitted = true;
    return true;
}

void Fetch::readBody() {
    if (!m_reply || m_closed) return;
    if (!m_admitted) {
        auto *reply = m_reply.data();
        if (!admitHeaders()) {
            // Body bytes before a final status are never accepted.
            if (!m_closed && m_reply == reply) fail(QStringLiteral("network"));
            return;
        }
    }
    while (m_reply && m_reply->bytesAvailable() > 0) {
        const auto chunk = m_reply->read(qMin<qint64>(64 * 1024, BodyLimit + 1 - m_body.size()));
        if (chunk.isEmpty()) break;
        m_body += chunk;
        if (m_body.size() > BodyLimit) { fail(QStringLiteral("tooLarge")); return; }
        if (m_announced >= 0 && m_body.size() > m_announced) { fail(QStringLiteral("damaged")); return; }
    }
}

void Fetch::done() {
    if (!m_reply || m_closed) return;
    if (m_reply->error() != QNetworkReply::NoError) { fail(QStringLiteral("network")); return; }
    auto *reply = m_reply.data();
    if (!admitHeaders()) {
        if (!m_closed && m_reply == reply) fail(QStringLiteral("network"));
        return;
    }
    readBody();
    if (m_closed || !m_reply) return;
    if (m_body.isEmpty() || (m_announced >= 0 && m_body.size() != m_announced)) { fail(QStringLiteral("damaged")); return; }
    const auto body = m_body;
    const auto mime = m_mime;
    m_closed = true;
    m_deadline.stop();
    drop();
    m_body.clear();
    emit finished(body, mime);
}

void Fetch::fail(const QString &kind) {
    if (m_closed) return;
    m_closed = true;
    m_deadline.stop();
    drop();
    m_body.clear();
    emit failed(kind);
}

void Fetch::drop() {
    if (!m_reply) return;
    auto *reply = m_reply.data();
    m_reply = nullptr;
    disconnect(reply, nullptr, this, nullptr);
    reply->abort();
    reply->deleteLater();
}

}

using namespace AudioPreview;

AudioPreviewController::AudioPreviewController(QObject *parent) : QObject(parent), m_factory(createSystemOutput) {
    m_pump.setInterval(PumpIntervalMs);
    connect(&m_pump, &QTimer::timeout, this, &AudioPreviewController::pump);
}

AudioPreviewController::~AudioPreviewController() {
    ++m_generation;
    if (m_fetch) {
        m_fetch->disconnect(this);
        delete m_fetch;
        m_fetch = nullptr;
    }
    release();
}

void AudioPreviewController::setNetworkAccessManager(QNetworkAccessManager *network) { m_network = network; }
void AudioPreviewController::setOutputFactory(OutputFactory factory) { m_factory = std::move(factory); }

QNetworkAccessManager *AudioPreviewController::network() {
    if (m_network) return m_network;
    if (!m_ownNetwork) {
        m_ownNetwork = std::make_unique<QNetworkAccessManager>();
        m_ownNetwork->setRedirectPolicy(QNetworkRequest::ManualRedirectPolicy);
    }
    return m_ownNetwork.get();
}

QString AudioPreviewController::describe(const QString &kind) {
    if (kind == "policy") return tr("This sound preview isn't hosted on GameBanana, so Modlock won't play it.");
    if (kind == "redirects") return tr("This sound preview kept redirecting, so Modlock stopped loading it.");
    if (kind == "network") return tr("Couldn't load this sound preview. Check your connection and try again.");
    if (kind == "tooLarge") return tr("This sound preview is too large to play here.");
    if (kind == "unsupported") return tr("This sound preview uses a format Modlock can't play.");
    if (kind == "damaged") return tr("This sound preview is damaged and can't be played.");
    if (kind == "output") return tr("No sound output is available.");
    if (kind == "outputFormat") return tr("Your sound output can't play this preview.");
    return {};
}

void AudioPreviewController::toggle(const QString &identity, const QString &url, bool concealed) {
    if (identity.isEmpty() || url.isEmpty()) return;
    if (concealed && !m_revealed.contains(identity)) return;
    if (identity == m_identity && url == m_url) {
        if (m_state == "playing" && m_output) {
            m_pump.stop();
            m_output->setPaused(true);
            setPhase(QStringLiteral("paused"));
            return;
        }
        if (m_state == "paused" && m_output) {
            m_output->setPaused(false);
            setPhase(QStringLiteral("playing"));
            m_pump.start();
            pump();
            return;
        }
        if (m_state == "loading") { stop(); return; }
    }
    begin(identity, url);
}

void AudioPreviewController::reveal(const QString &identity) {
    if (identity.isEmpty() || m_revealed.contains(identity)) return;
    m_revealed.insert(identity);
    emit changed();
}

void AudioPreviewController::stop() {
    ++m_generation;
    release();
    m_identity.clear();
    m_url.clear();
    setPhase(QStringLiteral("idle"));
}

void AudioPreviewController::invalidate() {
    m_revealed.clear();
    stop();
}

void AudioPreviewController::begin(const QString &identity, const QString &url) {
    release();
    const auto generation = ++m_generation;
    m_identity = identity;
    m_url = url;
    m_frames = 0;
    m_largestChunk = m_largestPump = 0;
    auto *fetch = new Fetch(network());
    m_fetch = fetch;
    connect(fetch, &Fetch::finished, this, [this, generation](const QByteArray &body, const QByteArray &mime) { fetched(generation, body, mime); });
    connect(fetch, &Fetch::failed, this, [this, generation](const QString &kind) { if (generation == m_generation) fail(kind); });
    setPhase(QStringLiteral("loading"));
    const auto refused = fetch->start(url);
    if (!refused.isEmpty() && generation == m_generation) fail(refused);
}

void AudioPreviewController::fetched(quint64 generation, const QByteArray &body, const QByteArray &mime) {
    if (generation != m_generation || m_state != "loading") return;
    if (m_fetch) {
        m_fetch->disconnect(this);
        m_fetch->deleteLater();
        m_fetch = nullptr;
    }
    // MIME is only a hint: bytes must be MP3 or Ogg Vorbis and must not contradict it.
    const auto container = sniff(body);
    if (container != Container::Mp3 && container != Container::Vorbis) { fail(QStringLiteral("unsupported")); return; }
    if (((mime == "audio/mpeg" || mime == "audio/mp3") && container != Container::Mp3)
        || (mime == "audio/ogg" && container != Container::Vorbis)) { fail(QStringLiteral("unsupported")); return; }
    auto opened = PcmDecoder::open(body, m_limits);
    if (!opened.decoder) { fail(opened.errorKind); return; }
    m_decoder = std::move(opened.decoder);
    const int channels = m_decoder->channels();
    const int rate = m_decoder->sampleRate();
    m_frameBytes = channels * int(sizeof(qint16));
    m_chunk.assign(size_t(ChunkFrames * channels), 0);
    m_frameLimit = qint64(rate) * MaximumSeconds;
    m_decoderEnded = false;
    // Prove the stream decodes before any output exists.
    const auto first = m_decoder->read(m_chunk.data(), ChunkFrames);
    if (first <= 0) { fail(QStringLiteral("damaged")); return; }
    m_frames = first;
    m_pending = QByteArray(reinterpret_cast<const char *>(m_chunk.data()), first * m_frameBytes);
    m_largestChunk = m_pending.size();
    m_largestPump = first;
    QAudioFormat format;
    format.setSampleRate(rate);
    format.setChannelCount(channels);
    format.setSampleFormat(QAudioFormat::Int16);
    format.setChannelConfig(QAudioFormat::defaultChannelConfigForChannelCount(channels));
    m_output = m_factory ? m_factory() : nullptr;
    if (!m_output) { fail(QStringLiteral("output")); return; }
    const auto error = m_output->start(format);
    if (!error.isEmpty()) { fail(error); return; }
    setPhase(QStringLiteral("playing"));
    m_pump.start();
    pump();
}

void AudioPreviewController::pump() {
    if (!m_output || m_state != "playing") return;
    if (m_output->failed()) { fail(QStringLiteral("output")); return; }
    qsizetype decoded = 0;
    for (;;) {
        if (m_pending.isEmpty()) {
            if (m_decoderEnded || !m_decoder) break;
            if (m_frames >= m_frameLimit) { m_decoderEnded = true; break; }
            const qsizetype room = m_output->bytesFree() / m_frameBytes;
            const qsizetype want = std::min({room, ChunkFrames, FramesPerPump - decoded, qsizetype(m_frameLimit - m_frames)});
            if (want <= 0) break;
            const auto got = m_decoder->read(m_chunk.data(), want);
            if (got < 0) { fail(QStringLiteral("damaged")); return; }
            if (got == 0) { m_decoderEnded = true; break; }
            decoded += got;
            m_frames += got;
            m_pending = QByteArray(reinterpret_cast<const char *>(m_chunk.data()), got * m_frameBytes);
            m_largestChunk = std::max(m_largestChunk, m_pending.size());
        }
        const qsizetype room = (m_output->bytesFree() / m_frameBytes) * m_frameBytes;
        if (room <= 0) break;
        const auto written = m_output->write(m_pending.constData(), std::min(room, m_pending.size()));
        if (written < 0) { fail(QStringLiteral("output")); return; }
        if (written == 0) break;
        m_pending.remove(0, written);
    }
    m_largestPump = std::max(m_largestPump, decoded);
    if (m_decoderEnded && m_pending.isEmpty()) {
        m_decoder.reset(); // releases decoder memory and the compressed bytes
        if (m_output->drained()) stop();
    }
}

void AudioPreviewController::fail(const QString &kind) {
    ++m_generation;
    release();
    setPhase(QStringLiteral("error"), kind);
}

void AudioPreviewController::release() {
    m_pump.stop();
    if (m_fetch) {
        m_fetch->disconnect(this);
        m_fetch->cancel();
        m_fetch->deleteLater();
        m_fetch = nullptr;
    }
    m_output.reset();
    m_decoder.reset();
    m_pending.clear();
    m_chunk.clear();
    m_chunk.shrink_to_fit();
    m_decoderEnded = false;
}

void AudioPreviewController::setPhase(const QString &state, const QString &errorKind) {
    m_state = state;
    m_errorKind = errorKind;
    emit changed();
}
