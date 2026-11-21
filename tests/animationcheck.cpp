#include "animationcheck.h"

#include <QColor>
#include <QCoreApplication>
#include <QDebug>
#include <QImage>
#include <QQmlContext>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTimer>
#include <memory>

namespace {
int markerPosition(QQuickWindow *window, QQuickItem *track, const QColor &color) {
    const QImage image = window->grabWindow();
    if (image.isNull() || track->width() < 80 || track->height() < 1) return -1;
    const QRectF rect = track->mapRectToScene(QRectF(0, 0, track->width(), track->height()));
    const qreal scale = qreal(image.width()) / window->width();
    const int y = qRound(rect.center().y() * scale);
    for (int x = qRound(rect.left() * scale); x < qRound(rect.right() * scale); ++x) {
        if (x < 0 || x >= image.width() || y < 0 || y >= image.height()) continue;
        const QColor pixel = image.pixelColor(x, y);
        if (pixel.red() == color.red() && pixel.green() == color.green() && pixel.blue() == color.blue())
            return x;
    }
    return -1;
}
}

void startAnimationCheck(QQuickWindow *window) {
    QTimer::singleShot(200, window, [window] {
        auto *checkbox = window->findChild<QObject *>(QStringLiteral("animationTest"));
        auto *context = checkbox ? qmlContext(checkbox) : nullptr;
        auto *track = context ? qobject_cast<QQuickItem *>(context->objectForName(QStringLiteral("track"))) : nullptr;
        auto *marker = context ? qobject_cast<QQuickItem *>(context->objectForName(QStringLiteral("marker"))) : nullptr;
        if (!checkbox || !track || !marker) {
            qCritical() << "Animation check could not find the test strip";
            QCoreApplication::exit(20);
            return;
        }
        const QColor color = marker->property("color").value<QColor>();
        auto first = std::make_shared<int>(-1);
        checkbox->setProperty("checked", true);
        QTimer::singleShot(250, window, [window, track, color, first] {
            *first = markerPosition(window, track, color);
        });
        QTimer::singleShot(850, window, [window, track, color, first] {
            const int second = markerPosition(window, track, color);
            if (*first < 0 || second < 0 || qAbs(second - *first) < 10) {
                qCritical() << "FAIL: the enabled test strip did not visibly move. Pixel positions:" << *first << second;
                QCoreApplication::exit(21);
                return;
            }
            qInfo() << "PASS: test strip visibly moved. Pixel positions:" << *first << second;
            QCoreApplication::exit(0);
        });
    });
}
