#pragma once
#include <QNetworkReply>
#include <QQuickAsyncImageProvider>
#include <QtQml/qqmlregistration.h>
#include <memory>

class PreviewNetworkFactory;
struct PreviewControl;
struct PreviewBridge;
struct PreviewBody;
struct PreviewEndpoint;

// Presentation only: metadata age/authority remains with the catalog model.
class PreviewStatus : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QUrl source READ source WRITE setSource NOTIFY changed)
    Q_PROPERTY(QString contextKey READ contextKey WRITE setContextKey NOTIFY changed)
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY changed)
    Q_PROPERTY(bool gallery READ gallery WRITE setGallery NOTIFY changed)
    Q_PROPERTY(QUrl imageSource READ imageSource NOTIFY changed)
    Q_PROPERTY(State state READ state NOTIFY changed)
    Q_PROPERTY(QString detail READ detail NOTIFY changed)
public:
    enum State { Missing, Hidden, Loading, Cached, Refreshed, Unavailable, Busy };
    Q_ENUM(State)
    explicit PreviewStatus(QObject *parent = nullptr);
    ~PreviewStatus() override;
    QUrl source() const { return m_source; }
    QString contextKey() const { return m_context; }
    bool active() const { return m_active; }
    bool gallery() const { return m_gallery; }
    QUrl imageSource() const { return m_imageSource; }
    State state() const { return m_state; }
    QString detail() const { return m_detail; }
    void setSource(const QUrl &);
    void setContextKey(const QString &);
    void setActive(bool);
    void setGallery(bool);
    Q_INVOKABLE void retry();
signals:
    void changed();
private:
    void reset();
    QUrl m_source, m_imageSource;
    QString m_context, m_token, m_detail;
    bool m_active = false, m_gallery = false;
    State m_state = Missing;
    PreviewNetworkFactory *m_factory = nullptr;
};

class PreviewImageProvider final : public QQuickAsyncImageProvider {
public:
    explicit PreviewImageProvider(std::shared_ptr<PreviewBridge> bridge);
    QQuickImageResponse *requestImageResponse(const QString &, const QSize &) override;
private:
    std::shared_ptr<PreviewBridge> m_bridge;
};

// Compatibility reply; its lease keeps the service body/slot charged until read or destroyed.
class PreviewReply final : public QNetworkReply {
    Q_OBJECT
public:
    enum Attribute { FreshnessAttribute = QNetworkRequest::User + 51,
                     ProvenanceAttribute = QNetworkRequest::User + 52 };
    PreviewReply(const QNetworkRequest &, std::shared_ptr<PreviewBridge>, QObject *parent);
    ~PreviewReply() override;
    void abort() override;
    qint64 bytesAvailable() const override;
    void deliver(std::shared_ptr<PreviewBody>, const QByteArray &, bool, bool, const QString &);
protected:
    qint64 readData(char *, qint64) override;
private:
    std::shared_ptr<PreviewBridge> m_bridge;
    std::shared_ptr<PreviewControl> m_control;
    std::shared_ptr<PreviewBody> m_body;
    std::shared_ptr<PreviewEndpoint> m_endpoint;
    qint64 m_offset = 0;
    bool m_done = false;
};
