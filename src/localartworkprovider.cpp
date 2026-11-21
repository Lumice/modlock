#include "localartworkprovider.h"
#include <QBuffer>
#include <QCryptographicHash>
#include <QFile>
#include <QImageReader>
#include <QJsonDocument>
#include <QMutex>
#include <QMutexLocker>
#include <QQuickTextureFactory>
#include <QRunnable>
#include <QThreadPool>
#include <QTimer>
#include <atomic>
#include <cmath>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
namespace {
QMutex registryMutex;
QHash<QString, QJsonObject> registry;
std::atomic<int> outstanding{0};
QThreadPool &workers() {
    static QThreadPool pool;
    static const bool configured = [] {
        pool.setMaxThreadCount(2);
        return true;
    }();
    Q_UNUSED(configured);
    return pool;
}
bool hex(const QJsonValue &v, int n) {
    auto s = v.toString();
    if (!v.isString() || s.size() != n)
        return false;
    for (auto c : s)
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')))
            return false;
    return true;
}
bool integer(const QJsonValue &v, bool positive = true) {
    return v.isDouble() && std::isfinite(v.toDouble()) && (!positive || v.toDouble() >= 0) &&
           std::floor(v.toDouble()) == v.toDouble();
}
QString admit(QJsonObject o) {
    auto key = QString::fromLatin1(
        QCryptographicHash::hash(QJsonDocument(o).toJson(QJsonDocument::Compact), QCryptographicHash::Sha256).toHex());
    QMutexLocker lock(&registryMutex);
    if (registry.size() >= 100000 && !registry.contains(key))
        return {};
    registry.insert(key, o);
    return "image://localartwork/" + key;
}
int openRegular(const QString &path) {
    if (!path.startsWith('/') || path.contains(QChar::Null))
        return -1;
    auto parts = path.split('/', Qt::SkipEmptyParts);
    if (parts.isEmpty())
        return -1;
    int fd = ::open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    for (qsizetype i = 0; i < parts.size() && fd >= 0; ++i) {
        if (parts[i] == ".." || parts[i] == ".") {
            ::close(fd);
            return -1;
        }
        const auto name = QFile::encodeName(parts[i]);
        int next = ::openat(fd, name.constData(),
                            O_RDONLY | O_NOFOLLOW | O_CLOEXEC | (i + 1 < parts.size() ? O_DIRECTORY : 0) | O_NONBLOCK);
        ::close(fd);
        fd = next;
    }
    return fd;
}
class Response final : public QQuickImageResponse, public QRunnable {
  public:
    Response(QJsonObject source, QSize size, bool accepted)
        : m_source(std::move(source)), m_size(size), m_accepted(accepted) {
        setAutoDelete(false);
    }
    QQuickTextureFactory *textureFactory() const override {
        return m_image.isNull() ? nullptr : QQuickTextureFactory::textureFactoryForImage(m_image);
    }
    QString errorString() const override {
        return m_image.isNull()
                   ? QStringLiteral("Local preview unavailable: changed, invalid, cancelled or decode limit exceeded.")
                   : QString();
    }
    void cancel() override { m_cancelled = true; }
    void run() override {
        if (m_accepted) {
            decode();
            --outstanding;
        }
        QMetaObject::invokeMethod(this, [this] { emit finished(); }, Qt::QueuedConnection);
    }

  private:
    void decode() {
        if (m_cancelled || m_source.isEmpty())
            return;
        int fd = openRegular(m_source["path"].toString());
        if (fd < 0)
            return;
        struct stat before{};
        if (::fstat(fd, &before) != 0 || !S_ISREG(before.st_mode) || before.st_size < 1 ||
            before.st_size > 16 * 1024 * 1024 || before.st_size != m_source["size"].toInteger()) {
            ::close(fd);
            return;
        }
        if (m_source.contains("device") && (quint64(before.st_dev) != m_source["device"].toVariant().toULongLong() ||
                                            quint64(before.st_ino) != m_source["inode"].toVariant().toULongLong() ||
                                            before.st_mtim.tv_sec != m_source["modifiedSeconds"].toInteger() ||
                                            before.st_mtim.tv_nsec != m_source["modifiedNanos"].toInteger())) {
            ::close(fd);
            return;
        }
        QFile file;
        if (!file.open(fd, QIODevice::ReadOnly | QIODevice::Unbuffered, QFileDevice::AutoCloseHandle)) {
            ::close(fd);
            return;
        }
        QCryptographicHash hash(QCryptographicHash::Sha256);
        QByteArray admitted;
        admitted.reserve(before.st_size);
        qint64 remaining = before.st_size;
        while (remaining > 0) {
            if (m_cancelled)
                return;
            const auto requested = qMin<qint64>(65536, remaining);
            const auto bytes = file.read(requested);
            if (bytes.size() != requested || file.error() != QFileDevice::NoError)
                return;
            hash.addData(bytes);
            admitted.append(bytes);
            remaining -= bytes.size();
        }
        // Probe only one extra byte; concurrent growth cannot extend the read budget.
        if (!file.read(1).isEmpty() || file.error() != QFileDevice::NoError || m_cancelled)
            return;
        struct stat beforeDecode{};
        if (::fstat(fd, &beforeDecode) != 0 || before.st_dev != beforeDecode.st_dev ||
            before.st_ino != beforeDecode.st_ino || before.st_size != beforeDecode.st_size ||
            before.st_mtim.tv_sec != beforeDecode.st_mtim.tv_sec ||
            before.st_mtim.tv_nsec != beforeDecode.st_mtim.tv_nsec ||
            before.st_ctim.tv_sec != beforeDecode.st_ctim.tv_sec ||
            before.st_ctim.tv_nsec != beforeDecode.st_ctim.tv_nsec ||
            QString::fromLatin1(hash.result().toHex()) != m_source["sha256"].toString())
            return;
        // Decode the verified bounded bytes, never a file that can grow during decoding.
        QBuffer buffer(&admitted);
        if (!buffer.open(QIODevice::ReadOnly))
            return;
        QImageReader reader(&buffer, m_source["format"].toString().toLatin1());
        reader.setDecideFormatFromContent(false);
        reader.setAutoTransform(true);
        const auto dimensions = reader.size();
        if (dimensions.width() < 1 || dimensions.height() < 1 || dimensions.width() > 8192 ||
            dimensions.height() > 8192 || qint64(dimensions.width()) * dimensions.height() > 16777216 || m_cancelled)
            return;
        QSize target = m_size.isValid() ? m_size : QSize(960, 640);
        target.setWidth(qBound(1, target.width(), 1600));
        target.setHeight(qBound(1, target.height(), 1000));
        reader.setScaledSize(dimensions.scaled(target, Qt::KeepAspectRatio));
        auto decoded = reader.read();
        struct stat after{};
        if (::fstat(fd, &after) != 0 || before.st_dev != after.st_dev || before.st_ino != after.st_ino ||
            before.st_size != after.st_size || before.st_mtim.tv_sec != after.st_mtim.tv_sec ||
            before.st_mtim.tv_nsec != after.st_mtim.tv_nsec || before.st_ctim.tv_sec != after.st_ctim.tv_sec ||
            before.st_ctim.tv_nsec != after.st_ctim.tv_nsec || m_cancelled)
            return;
        m_image = std::move(decoded);
    }
    QJsonObject m_source;
    QSize m_size;
    bool m_accepted;
    std::atomic_bool m_cancelled{false};
    QImage m_image;
};
} // namespace
LocalArtworkProvider::LocalArtworkProvider() {
    QImageReader::setAllocationLimit(64);
}
bool LocalArtworkProvider::validOwned(const QJsonObject &o) {
    return hex(o["id"], 32) && hex(o["sha256"], 64) && integer(o["size"]) && o["size"].toDouble() > 0 &&
           o["size"].toDouble() <= 16 * 1024 * 1024 &&
           (o["format"] == QJsonValue("png") || o["format"] == QJsonValue("jpeg"));
}
bool LocalArtworkProvider::validSource(const QJsonObject &o) {
    auto owned = o;
    owned.insert("id", QString(32, '0'));
    return validOwned(owned) && o["path"].isString() && o["path"].toString().startsWith('/') &&
           o["path"].toString().toUtf8().size() <= 4096 && integer(o["device"]) && integer(o["inode"]) &&
           integer(o["modifiedSeconds"], false) && integer(o["modifiedNanos"]) &&
           o["modifiedNanos"].toDouble() < 1000000000;
}
QString LocalArtworkProvider::admitSource(const QJsonObject &o) {
    return validSource(o) ? admit(o) : QString();
}
QString LocalArtworkProvider::admitOwned(const QString &root, const QJsonObject &o) {
    if (!validOwned(o) || !root.startsWith('/'))
        return {};
    auto source = o;
    source.insert("path", root + "/artwork/" + o["id"].toString() + ".image");
    return admit(source);
}
QQuickImageResponse *LocalArtworkProvider::requestImageResponse(const QString &id, const QSize &size) {
    QJsonObject source;
    {
        QMutexLocker lock(&registryMutex);
        source = registry.value(id);
    }
    const bool accepted = outstanding.fetch_add(1) < 34;
    if (!accepted)
        --outstanding;
    auto *response = new Response(source, size, accepted);
    if (accepted)
        workers().start(response);
    else
        QTimer::singleShot(0, response, [response] { response->run(); });
    return response;
}
