#pragma once

#include <QPointer>
#include <QQuickWindow>
#include <QTimer>
#include <QtQml/qqmlregistration.h>
#include <chrono>
#include <mutex>
#include <memory>
#include <vector>

class FrameMetrics : public QObject {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QQuickWindow *window READ window WRITE setWindow NOTIFY windowChanged)
    Q_PROPERTY(bool collecting READ collecting WRITE setCollecting NOTIFY collectingChanged)
    Q_PROPERTY(qreal refreshRate READ refreshRate NOTIFY metricsChanged)
    Q_PROPERTY(qreal fps READ fps NOTIFY metricsChanged)
    Q_PROPERTY(qreal p95 READ p95 NOTIFY metricsChanged)
    Q_PROPERTY(int samples READ samples NOTIFY metricsChanged)
public:
    explicit FrameMetrics(QObject *parent = nullptr);
    ~FrameMetrics() override;
    QQuickWindow *window() const { return m_window; }
    void setWindow(QQuickWindow *window);
    bool collecting() const { return m_collecting; }
    void setCollecting(bool value);
    qreal refreshRate() const;
    qreal fps() const { return m_fps; }
    qreal p95() const { return m_p95; }
    int samples() const { return m_samples; }
signals:
    void windowChanged();
    void collectingChanged();
    void metricsChanged();
private:
    void summarize();
    QPointer<QQuickWindow> m_window;
    QMetaObject::Connection m_swapConnection;
    QMetaObject::Connection m_screenConnection;
    QMetaObject::Connection m_refreshConnection;
    QTimer m_timer;
    struct Samples {
        std::mutex mutex;
        bool active = false;
        std::vector<std::chrono::steady_clock::time_point> timestamps;
    };
    std::shared_ptr<Samples> m_buffer = std::make_shared<Samples>();
    bool m_collecting = false;
    qreal m_fps = 0;
    qreal m_p95 = 0;
    int m_samples = 0;
};

