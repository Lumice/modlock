#pragma once
#include "previewreply.h"
#include <QQmlNetworkAccessManagerFactory>
#include <QThread>
#include <functional>
#include <atomic>
#include <QMutex>
#include <QHash>
#include <QImage>

class PreviewService;
struct PreviewBody {
    QByteArray bytes;
    std::function<void()> released;
    ~PreviewBody() { if (released) released(); }
};
struct PreviewControl {
    QString token;
    QUrl url;
    bool gallery = false, proxy = false;
    std::atomic_bool cancelled{false}, busy{false};
    bool claimed = false, delivered = false;
    // Consumed by the service after attach; delivery uses a guarded response endpoint.
    std::function<void(const QImage &, std::shared_ptr<PreviewBody>, QByteArray, bool, bool, QString)> complete;
};
struct PreviewBridge {
    QMutex mutex;
    QHash<QString, std::shared_ptr<PreviewControl>> controls;
    PreviewService *service = nullptr;
    bool stopped = false;
};
void previewAttach(const std::shared_ptr<PreviewBridge> &, const std::shared_ptr<PreviewControl> &);
void previewCancel(const std::shared_ptr<PreviewBridge> &, const std::shared_ptr<PreviewControl> &);

class PreviewNetworkFactory final : public QObject, public QQmlNetworkAccessManagerFactory {
    Q_OBJECT
public:
    explicit PreviewNetworkFactory(QString cacheDirectory, QString root);
    ~PreviewNetworkFactory() override;
    void shutdown();
    QNetworkAccessManager *create(QObject *parent) override;
    PreviewImageProvider *imageProvider();
    QString acquire(const QUrl &, bool);
    void release(const QString &);
    static PreviewNetworkFactory *instance();
signals:
    void status(const QString &token, const QUrl &url, PreviewStatus::State state, const QString &detail);
    void capacityAvailable();
private:
    QThread m_thread;
    std::shared_ptr<PreviewBridge> m_bridge;
};
