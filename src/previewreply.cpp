#include "previewreply.h"
#include "previewnetworkfactory.h"
#include <QQuickTextureFactory>
#include <cstring>

struct PreviewEndpoint { QMutex mutex; PreviewReply *reply = nullptr; };

PreviewStatus::PreviewStatus(QObject *parent) : QObject(parent), m_factory(PreviewNetworkFactory::instance()) {
    if (!m_factory) return;
    connect(m_factory, &PreviewNetworkFactory::status, this,
        [this](const QString &token, const QUrl &url, State state, const QString &detail) {
            if (token != m_token || url != m_source || !m_active) return;
            m_state = state; m_detail = detail; emit changed();
        });
    connect(m_factory, &PreviewNetworkFactory::capacityAvailable, this, [this] {
        if (m_active && m_state == Busy) retry();
    });
}
PreviewStatus::~PreviewStatus() { if (m_factory) m_factory->release(m_token); }
void PreviewStatus::setSource(const QUrl &v) { if (m_source != v) { m_source = v; reset(); } }
void PreviewStatus::setContextKey(const QString &v) { if (m_context != v) { m_context = v; reset(); } }
void PreviewStatus::setActive(bool v) { if (m_active != v) { m_active = v; reset(); } }
void PreviewStatus::setGallery(bool v) { if (m_gallery != v) { m_gallery = v; reset(); } }
void PreviewStatus::reset() {
    if (m_factory) m_factory->release(m_token);
    m_token.clear(); m_imageSource.clear(); m_detail.clear();
    m_state = m_source.isEmpty() ? Missing : !m_active ? Hidden : Loading;
    if (m_active && !m_source.isEmpty() && m_factory
        && (m_source.toEncoded().size() > 7000 || m_context.toUtf8().size() > 1024)) {
        m_state = Unavailable; m_detail = QStringLiteral("Preview request exceeds the control limit");
    } else if (m_active && !m_source.isEmpty() && m_factory) {
        m_token = m_factory->acquire(m_source, m_gallery);
        if (m_token.isEmpty()) m_state = Busy;
        else m_imageSource = QUrl(QStringLiteral("image://remoteartwork/") + m_token);
    }
    emit changed();
}
void PreviewStatus::retry() { reset(); }

namespace {
class PreviewImageResponse;
struct PreviewImageEndpoint { QMutex mutex; PreviewImageResponse *response = nullptr; };
class PreviewImageResponse final : public QQuickImageResponse {
public:
    PreviewImageResponse(std::shared_ptr<PreviewBridge> bridge, std::shared_ptr<PreviewControl> control,
                         std::shared_ptr<PreviewImageEndpoint> endpoint)
        : m_bridge(std::move(bridge)), m_control(std::move(control)), m_endpoint(std::move(endpoint)) { m_endpoint->response = this; }
    ~PreviewImageResponse() override {
        { QMutexLocker lock(&m_endpoint->mutex); m_endpoint->response = nullptr; }
        previewCancel(m_bridge, m_control);
    }
    QQuickTextureFactory *textureFactory() const override {
        return m_image.isNull() ? nullptr : QQuickTextureFactory::textureFactoryForImage(m_image);
    }
    QString errorString() const override { return m_error; }
    void cancel() override { previewCancel(m_bridge, m_control); }
    void complete(const QImage &image, const QString &error) {
        m_image = image; m_error = error;
        emit finished(); // No response work or access after this signal.
    }
private:
    std::shared_ptr<PreviewBridge> m_bridge;
    std::shared_ptr<PreviewControl> m_control;
    std::shared_ptr<PreviewImageEndpoint> m_endpoint;
    QImage m_image;
    QString m_error;
};
}
PreviewImageProvider::PreviewImageProvider(std::shared_ptr<PreviewBridge> bridge) : m_bridge(std::move(bridge)) {}
QQuickImageResponse *PreviewImageProvider::requestImageResponse(const QString &id, const QSize &) {
    std::shared_ptr<PreviewControl> control;
    { QMutexLocker lock(&m_bridge->mutex); control = m_bridge->controls.value(id);
      if (control && control->claimed) control.reset();
      if (control) control->claimed = true; }
    if (!control) { control = std::make_shared<PreviewControl>(); control->cancelled = true; }
    auto endpoint = std::make_shared<PreviewImageEndpoint>();
    auto *response = new PreviewImageResponse(m_bridge, control, endpoint);
    control->complete = [endpoint](const QImage &image, std::shared_ptr<PreviewBody>, QByteArray, bool, bool, QString error) {
        QMutexLocker lock(&endpoint->mutex);
        if (auto *response = endpoint->response)
            QMetaObject::invokeMethod(response, [response, image, error] {
                response->complete(image, error);
            }, Qt::QueuedConnection);
    };
    previewAttach(m_bridge, control);
    return response;
}
PreviewReply::PreviewReply(const QNetworkRequest &request, std::shared_ptr<PreviewBridge> bridge, QObject *parent)
    : QNetworkReply(parent), m_bridge(std::move(bridge)) {
    setRequest(request); setUrl(request.url()); setOperation(QNetworkAccessManager::GetOperation);
    open(QIODevice::ReadOnly | QIODevice::Unbuffered);
    m_control = std::make_shared<PreviewControl>();
    m_control->url = request.url(); m_control->proxy = true;
    m_endpoint = std::make_shared<PreviewEndpoint>(); m_endpoint->reply = this;
    m_control->complete = [endpoint = m_endpoint](const QImage &, std::shared_ptr<PreviewBody> body, QByteArray mime, bool cached, bool stale, QString error) {
        QMutexLocker lock(&endpoint->mutex);
        if (auto *reply = endpoint->reply)
            QMetaObject::invokeMethod(reply, [reply, body = std::move(body), mime, cached, stale, error] {
                reply->deliver(body, mime, cached, stale, error);
            }, Qt::QueuedConnection);
    };
    previewAttach(m_bridge, m_control);
}
PreviewReply::~PreviewReply() {
    { QMutexLocker lock(&m_endpoint->mutex); m_endpoint->reply = nullptr; }
    previewCancel(m_bridge, m_control);
}
void PreviewReply::abort() {
    if (m_done) { previewCancel(m_bridge, m_control); m_body.reset(); return; }
    previewCancel(m_bridge, m_control);
}
qint64 PreviewReply::bytesAvailable() const {
    return (m_body ? m_body->bytes.size() - m_offset : 0) + QNetworkReply::bytesAvailable();
}
void PreviewReply::deliver(std::shared_ptr<PreviewBody> body, const QByteArray &mime, bool cached, bool stale, const QString &error) {
    if (m_done) return;
    m_done = true; m_body = std::move(body);
    setAttribute(QNetworkRequest::SourceIsFromCacheAttribute, cached);
    setAttribute(static_cast<QNetworkRequest::Attribute>(FreshnessAttribute), cached ? (stale ? "stale" : "fresh") : "network");
    setAttribute(static_cast<QNetworkRequest::Attribute>(ProvenanceAttribute), cached ? "disk-cache" : "network");
    if (!m_body || m_control->cancelled) {
        m_body.reset(); setError(m_control->cancelled ? OperationCanceledError : ContentNotFoundError, error);
        emit errorOccurred(this->error());
    } else {
        setHeader(QNetworkRequest::ContentTypeHeader, QString::fromLatin1(mime));
        setHeader(QNetworkRequest::ContentLengthHeader, m_body->bytes.size());
        setAttribute(QNetworkRequest::HttpStatusCodeAttribute, 200);
        emit metaDataChanged(); emit readyRead();
    }
    setFinished(true); emit finished();
}
qint64 PreviewReply::readData(char *data, qint64 maximum) {
    if (!m_body) return m_done ? -1 : 0;
    const auto count = qMin(maximum, m_body->bytes.size() - m_offset);
    if (count <= 0) return -1;
    std::memcpy(data, m_body->bytes.constData() + m_offset, size_t(count)); m_offset += count;
    if (m_offset == m_body->bytes.size()) m_body.reset();
    return count;
}
