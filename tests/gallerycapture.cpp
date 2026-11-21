#include "gallerycapture.h"
#include <QQuickWindow>
#include <QQuickItem>
#include <QCoreApplication>
#include <QHoverEvent>
#include <QPainter>
#include <QDebug>

static QList<QQuickItem *> itemsNamed(QQuickItem *root, const QString &name)
{
    QList<QQuickItem *> result;
    if (root->objectName() == name) result.append(root);
    for (auto *child : root->childItems()) result.append(itemsNamed(child, name));
    return result;
}

static QQuickItem *itemNamed(QQuickItem *root, const QString &name)
{
    return itemsNamed(root, name).value(0);
}

// Capture actual focus states sequentially: Qt permits one active focus item.
bool captureGallery(QQuickWindow *window, const QString &path)
{
    const auto cells = itemsNamed(window->contentItem(), "galleryCell");
    if (cells.isEmpty()) return false;
    QList<QQuickItem *> focuses;
    for (auto *cell : cells) {
        qInfo().noquote() << "GALLERY_STATE" << cell->property("caption").toString();
        auto *control = qvariant_cast<QQuickItem *>(cell->property("control"));
        if (!control) return false;
        auto state = cell->property("captureState").toString();
        if (auto *header = itemNamed(control, "galleryHeader")) {
            auto *play = itemNamed(header, "launchActiveButton");
            auto *options = itemNamed(header, "launchMenuButton");
            if (!play || !options) return false;
            // Demo mode hides native launch actions. Show only these inert gallery
            // instances, without changing the library or invoking launch actions.
            play->parentItem()->setVisible(true);
            play->setEnabled(true);
            options->setEnabled(true);
            control = state.startsWith("options ") ? options : play;
            state.remove("options ");
        }
        if (state == "selected") control->setProperty("currentValue", "grid");
        if (auto *segment = itemNamed(control, "gallerySegment")) control = segment;
        if (state.contains("disabled")) control->parentItem()->setEnabled(false);
        if (state.startsWith("checked") && !control->setProperty("checked", true)) return false;
        if (state == "indeterminate") {
            if (control->property("checkState").isValid()) control->setProperty("checkState", 1);
            else control->setProperty("indeterminate", true);
        }
        if (state == "stopped") control->setProperty("running", false);
        if (state.contains("highlighted"))
            if (!control->setProperty("highlighted", true)) return false;
        if (state == "inactive") control->setProperty("active", false);
        if (state == "header") control->setProperty("position", 0);
        if (state.contains("pressed")) {
            const char *property = control->property("down").isValid() ? "down" : "pressed";
            if (!control->setProperty(property, true)) return false;
        }
        if (state.contains("hovered")) {
            control->setProperty("hoverEnabled", true);
            QHoverEvent event(QEvent::HoverEnter, QPointF(5, 5), QPointF(5, 5), QPointF(-1, -1));
            QCoreApplication::sendEvent(control, &event);
            if (!control->property("hovered").toBool()) return false;
        }
        if (state.endsWith("focus") || state == "selected text") focuses.append(cell);
        if (state == "selected text") QMetaObject::invokeMethod(control, "selectAll");
    }
    QCoreApplication::processEvents();
    // Layouts polish at render time. Align only after the formerly hidden
    // production Play rows have their final geometry.
    window->grabWindow();
    for (auto *cell : cells) {
        auto *sample = qvariant_cast<QQuickItem *>(cell->property("control"));
        if (auto *header = itemNamed(sample, "galleryHeader")) {
            auto *play = itemNamed(header, "launchActiveButton");
            auto *options = itemNamed(header, "launchMenuButton");
            header->setX(6 - play->mapToItem(header, QPointF()).x());
            sample->setWidth(play->width() + options->width() + 20);
            if (!play->isVisible() || play->mapToItem(sample, QPointF()).x() != 6) return false;
        }
        const auto rect = cell->mapRectToScene(cell->boundingRect());
        if (!cell->isVisible() || rect.left() < 0 || rect.top() < 0
            || rect.right() > window->width() || rect.bottom() > window->height()) {
            qWarning() << "Gallery cell outside capture" << cell->property("caption") << rect << window->size();
            return false;
        }
    }
    QImage image = window->grabWindow();
    if (image.isNull()) return false;
    for (auto *cell : focuses) {
        qInfo().noquote() << "GALLERY_STATE" << cell->property("caption").toString();
        auto *control = qvariant_cast<QQuickItem *>(cell->property("control"));
        if (auto *header = itemNamed(control, "galleryHeader"))
            control = itemNamed(header, cell->property("captureState").toString().startsWith("options ")
                ? "launchMenuButton" : "launchActiveButton");
        if (auto *segment = itemNamed(control, "gallerySegment")) control = segment;
        control->forceActiveFocus(Qt::TabFocusReason);
        QCoreApplication::processEvents();
        // Text inputs use activeFocus; other controls expose visualFocus.
        if (!control->hasActiveFocus() || (control->property("visualFocus").isValid()
            && !control->property("visualFocus").toBool())) return false;
        const QImage focused = window->grabWindow();
        const auto origin = cell->mapToScene(QPointF()) * image.devicePixelRatio();
        const QRect rect(origin.toPoint(), QSize(qRound(cell->width() * image.devicePixelRatio()),
                                                qRound(cell->height() * image.devicePixelRatio())));
        QPainter painter(&image);
        painter.drawImage(rect, focused, rect);
    }
    // Log labels only after all state assertions and the image save succeed.
    if (!image.save(path)) return false;
    for (auto *cell : cells) {
        qInfo().noquote() << "GALLERY_CELL" << cell->property("caption").toString()
                          + "\t" + cell->property("captureState").toString();
    }
    qInfo() << "GALLERY_SIZE" << cells.size();
    qInfo().noquote() << "GALLERY_THEME" << (itemNamed(window->contentItem(), "controlGallery")->property("dark").toBool() ? "dark" : "light");
    return true;
}
