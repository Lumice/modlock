#include "framemetrics.h"

#include <QScreen>
#include <algorithm>
#include <cmath>

FrameMetrics::FrameMetrics(QObject *parent) : QObject(parent) {
    m_timer.setInterval(1000);
    connect(&m_timer, &QTimer::timeout, this, &FrameMetrics::summarize);
}

FrameMetrics::~FrameMetrics() {
    disconnect(m_swapConnection);
}

void FrameMetrics::setWindow(QQuickWindow *window) {
    if (m_window == window) return;
    disconnect(m_swapConnection);
    disconnect(m_screenConnection);
    disconnect(m_refreshConnection);
    m_window = window;
    if (window) {
        m_swapConnection = connect(window, &QQuickWindow::frameSwapped, this, [buffer = m_buffer] {
            std::lock_guard lock(buffer->mutex);
            if (buffer->active && buffer->timestamps.size() < 10000)
                buffer->timestamps.push_back(std::chrono::steady_clock::now());
        }, Qt::DirectConnection);
        auto screenChanged = [this](QScreen *screen) {
            disconnect(m_refreshConnection);
            if (screen) m_refreshConnection = connect(screen, &QScreen::refreshRateChanged,
                this, &FrameMetrics::metricsChanged);
            emit metricsChanged();
        };
        m_screenConnection = connect(window, &QWindow::screenChanged, this, screenChanged);
        screenChanged(window->screen());
    }
    emit windowChanged();
    emit metricsChanged();
}

qreal FrameMetrics::refreshRate() const {
    return m_window && m_window->screen() ? m_window->screen()->refreshRate() : 0;
}

void FrameMetrics::setCollecting(bool value) {
    if (m_collecting == value) return;
    m_collecting = value;
    {
        std::lock_guard lock(m_buffer->mutex);
        m_buffer->active = value;
        m_buffer->timestamps.clear();
    }
    m_fps = 0;
    m_p95 = 0;
    m_samples = 0;
    if (value) m_timer.start(); else m_timer.stop();
    emit collectingChanged();
    emit metricsChanged();
}

void FrameMetrics::summarize() {
    std::vector<std::chrono::steady_clock::time_point> times;
    {
        std::lock_guard lock(m_buffer->mutex);
        times.swap(m_buffer->timestamps);
    }
    m_samples = static_cast<int>(times.size());
    m_fps = 0;
    m_p95 = 0;
    if (times.size() > 1) {
        const double seconds = std::chrono::duration<double>(times.back() - times.front()).count();
        if (seconds > 0) m_fps = (times.size() - 1) / seconds;
        std::vector<double> intervals;
        intervals.reserve(times.size() - 1);
        for (size_t i = 1; i < times.size(); ++i)
            intervals.push_back(std::chrono::duration<double, std::milli>(times[i] - times[i - 1]).count());
        std::sort(intervals.begin(), intervals.end());
        const auto index = static_cast<size_t>(std::ceil(intervals.size() * .95)) - 1;
        m_p95 = intervals[index];
    }
    emit metricsChanged();
}

