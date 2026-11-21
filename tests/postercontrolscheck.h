#pragma once
#include <QTemporaryDir>
#include <QThread>

static void startPosterControlsCheck(QQuickWindow *window) {
    QTimer::singleShot(0, window, [window] {
        try {
            const bool dark = qEnvironmentVariable("POSTER_THEME") != "light";
            auto palette = QGuiApplication::palette();
            palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
            const QColor text(dark ? "#eee8dc" : "#1b1626");
            for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) palette.setColor(role, text);
            QGuiApplication::setPalette(palette);
            window->resize(qEnvironmentVariableIntValue("POSTER_WIDTH"), 640);
            QTemporaryDir files;
            require(files.isValid(), "Poster artwork fixture directory failed");
            QImage image(530, 350, QImage::Format_RGB32);
            image.fill(QColor("#347987"));
            require(image.save(files.filePath("art.png")), "Poster artwork fixture failed");
            QQmlComponent component(qmlEngine(window));
            component.setData(R"QML(import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle
Rectangle {
    id: fixture
    required property url artSource
    anchors.fill: parent
    z: 100000
    color: Theme.window
    readonly property bool darkPalette: Tokens.dark
    property int details: 0
    property int downloads: 0
    property int favorites: 0
    property bool cardMarked: false
    function reduced(value) { Preferences.reduceMotion = value }
    Component.onCompleted: { Preferences.showMaturePreviews = false; Preferences.reduceMotion = false }
    component Host: QtObject {
        property string scope: "library"
        property string presentationMode: "grid"
        property real width: 230
        property real cellWidth: 230
        property real gridHeight: 230
        property real listHeight: 80
        property real posterFooterHeight: 52
        property bool ready: true
        property bool modelCoherent: true
        property bool viewerOpen: false
        function inRange(item) { return true }
        function issueToken() { return "poster-fixture" }
        function selectIndex(index, identity) {}
        function userInteracted() {}
        function openViewer(owner, button) {}
    }
    Host { id: libraryHost }
    Host { id: browseHost; scope: "browse"; gridHeight: 282 }
    MediaCard {
        objectName: "hiddenPoster"; x: 20; y: 20
        index: 0; anchorIdentity: "hidden"; resultHost: libraryHost
        ownerSnapshot: ({identity: "hidden", source: fixture.artSource.toString(), hidden: true})
        heading: "A deliberately long hidden mod title with a status badge"
        subtitle: "Author · Character · Skins"; healthLabel: "Needs attention"
        actionKind: "switch"; actionChecked: true; actionEnabled: true; actionName: "Enable hidden mod"
        onDetailsRequested: fixture.details++
    }
    MediaCard {
        objectName: "browsePoster"; x: 260; y: 20
        index: 1; anchorIdentity: "browse"; resultHost: browseHost
        ownerSnapshot: ({identity: "browse", source: fixture.artSource.toString(), hidden: false})
        heading: "Browse poster with real decoded artwork"; subtitle: "Author · Skins"
        catalogStats: [{icon: "download", text: "659.1k"}, {icon: "heart", text: "129"}]
        statsText: "659,123 downloads, 129 likes"
        actionKind: "favorite"; actionVisible: true; actionEnabled: true; actionName: "Add to favorites"; actionText: "Add to favorites"
        installVisible: true; installEnabled: true
        onInstallRequested: fixture.downloads++
        onActionRequested: fixture.favorites++
    }
    MediaCard {
        objectName: "selectPoster"; x: 20; y: 270
        index: 2; anchorIdentity: "select"; resultHost: libraryHost
        ownerSnapshot: ({identity: "select", source: fixture.artSource.toString(), hidden: false})
        heading: "Selectable library poster"; subtitle: "Author · Skins"
        selectable: true; marked: fixture.cardMarked
        onMarkToggled: next => fixture.cardMarked = next
        actionKind: "switch"; actionChecked: true; actionEnabled: true; actionName: "Enable selectable mod"
    }
    HomeModTile {
        objectName: "artHomePoster"; x: 500; y: 20; width: 222; height: 222
        poster: true; title: "Recent poster"; imageSource: fixture.artSource; contextKey: "home-art"
    }
    HomeModTile {
        objectName: "emptyHomePoster"; x: 500; y: 330; width: 222; height: 222
        poster: true; title: "No artwork"; contextKey: "home-empty"
    }
})QML", QUrl());
            std::unique_ptr<QObject> object(component.createWithInitialProperties({{"artSource", QUrl::fromLocalFile(files.filePath("art.png"))}}));
            if (!object) qWarning() << component.errors();
            require(bool(object), "Poster controls component did not load");
            auto *item = qobject_cast<QQuickItem *>(object.get());
            item->setParentItem(window->contentItem());
            const auto wait = [&](auto condition) {
                QElapsedTimer elapsed; elapsed.start();
                while (!condition() && elapsed.elapsed() < 2000) { QCoreApplication::processEvents(); QThread::msleep(5); }
                require(condition(), "Poster controls did not settle");
            };
            const auto settle = [] { QEventLoop loop; QTimer::singleShot(260, &loop, &QEventLoop::quit); loop.exec(); };
            const auto save = [&](const QString &name) {
                const auto directory = qEnvironmentVariable("POSTER_CAPTURE_DIR");
                if (directory.isEmpty()) return;
                require(QDir().mkpath(directory), "Cannot create poster capture directory");
                require(window->grabWindow().save(directory + "/" + name + ".png"), "Poster capture failed");
            };
            const auto move = [&](QQuickItem *target) {
                const auto point = target->mapToScene(QPointF(target->width()/2, target->height()/2));
                QMouseEvent event(QEvent::MouseMove, point, point, window->mapToGlobal(point.toPoint()), Qt::NoButton, Qt::NoButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &event);
                settle();
            };
            auto *hidden = qobject_cast<QQuickItem *>(named(object.get(), "hiddenPoster"));
            auto *preview = hidden->property("preview").value<QQuickItem *>();
            auto *reveal = qobject_cast<QQuickItem *>(named(hidden, "revealPreview"));
            auto *title = qobject_cast<QQuickItem *>(named(hidden, "rowTitle"));
            settle();
            require(object->property("darkPalette").toBool()==dark, "Poster fixture theme does not match capture label");
            require(reveal->isVisible(), "Hidden poster reveal control missing");
            const QRectF revealBounds(reveal->mapToItem(hidden, QPointF()), reveal->size());
            const QRectF titleBounds(title->mapToItem(hidden, QPointF()), title->size());
            require(!revealBounds.intersects(titleBounds), "Hidden long-title poster covers Show preview");
            save("hidden-and-browse-art");
            move(reveal);
            require(named(hidden,"cardHoverHandler")->property("hovered").toBool(), "Offscreen pointer did not hover poster");
            require(!hidden->property("lifted").toBool(), "Hidden reveal target moves on hover");
            require(qAbs(preview->scale()-1)<.001, "Hidden poster zoom moves reveal controls");
            save("hover-hidden");
            realClickItem(window, reveal);
            wait([&] { return preview->property("ready").toBool(); });
            require(preview->property("revealed").toBool() && object->property("details").toInt()==0, "Reveal click was intercepted by details");
            settle();
            require(qAbs(named(preview,"previewImage")->property("scale").toDouble()-1.05)<.001, "Ready poster artwork does not zoom");
            save("hover-ready");
            auto *browse = qobject_cast<QQuickItem *>(named(object.get(), "browsePoster"));
            auto *browsePreview = browse->property("preview").value<QQuickItem *>();
            wait([&] { return browsePreview->property("ready").toBool(); });
            auto *stats = qobject_cast<QQuickItem *>(named(browse, "catalogStatsRow"));
            auto *accessible = QAccessible::queryAccessibleInterface(stats);
            require(accessible && accessible->text(QAccessible::Name).contains("129 likes"), "Poster likes accessibility missing");
            for (auto *child : objects(stats))
                if (child->property("truncated").isValid()) require(!child->property("truncated").toBool(), "Poster stats truncated");
            auto *install = qobject_cast<QQuickItem *>(named(browse, "installButton"));
            auto *favorite = qobject_cast<QQuickItem *>(named(browse, "favoriteButton"));
            auto *favBg = favorite->property("background").value<QQuickItem *>();
            require(favBg, "Browse favorite background missing");
            const QRectF installRect(install->mapToItem(browse, QPointF()), install->size());
            const QRectF favRect(favorite->mapToItem(browse, QPointF()), favorite->size());
            const QRectF favBgRect(favBg->mapToItem(favorite, QPointF()), favBg->size());
            const QRectF statsRect(stats->mapToItem(browse, QPointF()), stats->size());
            require(browse->width() == 222, "Browse poster test width changed from 222 px");
            require(qAbs(favorite->height() - install->height()) < 0.5 && favorite->height() >= 32, "Favorite control height does not match Download control height");
            require(favBgRect.x() >= -0.01 && favBgRect.y() >= -0.01 && favBgRect.right() <= favorite->width() + 0.01 && favBgRect.bottom() <= favorite->height() + 0.01, "Favorite background paints outside control bounds");
            require(qAbs(favRect.top() - installRect.top()) < 0.5 && qAbs(favRect.bottom() - installRect.bottom()) < 0.5, "Favorite and Download controls are not aligned");
            require(!favRect.intersects(installRect) && !favRect.intersects(statsRect) && !installRect.intersects(statsRect), "Browse footer controls overlap");
            favorite->setProperty("hoverEnabled", true);
            install->setProperty("hoverEnabled", true);
            const auto restFavColor = favBg->property("color").value<QColor>();
            move(browsePreview);
            require(named(browse, "cardHoverHandler")->property("hovered").toBool(), "Offscreen pointer did not hover Browse poster");
            save("browse-card-hover");
            move(favorite);
            require(favorite->property("hovered").toBool(), "Offscreen pointer did not hover favorite button");
            require(favBg->property("color").value<QColor>() != restFavColor, "Favorite button background ignores hover state");
            save("browse-favorite-hover");
            move(install);
            require(install->property("hovered").toBool(), "Offscreen pointer did not hover Download button");
            save("browse-download-hover");
            realClickItem(window, install);
            realClickItem(window, favorite);
            require(object->property("downloads").toInt()==1 && object->property("favorites").toInt()==1, "Poster footer pointer actions failed");
            auto *selectCard = qobject_cast<QQuickItem *>(named(object.get(), "selectPoster"));
            auto *selectPreview = selectCard->property("preview").value<QQuickItem *>();
            wait([&] { return selectPreview->property("ready").toBool(); });
            auto *markBox = qobject_cast<QQuickItem *>(named(selectCard, "markBox"));
            require(markBox && !markBox->isVisible(), "Unmarked library selector must stay hidden before hover or focus");
            markBox->setProperty("hoverEnabled", true);
            move(selectPreview);
            require(markBox->isVisible(), "Library selector did not appear on poster hover");
            auto *markBg = markBox->property("background").value<QQuickItem *>();
            auto *markInd = markBox->property("indicator").value<QQuickItem *>();
            auto *selectSwitch = qobject_cast<QQuickItem *>(named(selectCard, "modSwitch"));
            require(markBg && markInd && selectSwitch, "Library selector internals missing");
            const QRectF markRect(markBox->mapToItem(selectCard, QPointF()), markBox->size());
            const QPointF markInset = markBox->mapToItem(markBox->parentItem(), QPointF());
            const QRectF switchRect(selectSwitch->mapToItem(selectCard, QPointF()), selectSwitch->size());
            const QPointF indCenter = markInd->mapToItem(markBox, QPointF(markInd->width() / 2.0, markInd->height() / 2.0));
            const QColor markBgColor = markBg->property("color").value<QColor>();
            require(markBox->width() >= 30 && markBox->width() <= 32.5 && markBox->height() >= 30 && markBox->height() <= 32.5, "Library grid selector is not compact (~32 px)");
            require(qAbs(markInset.x() - 8) < 0.5 && qAbs(markInset.y() - 8) < 0.5, "Library grid selector does not have ~8 px inset within the lifted artwork");
            require(qAbs(markInd->width() - 18) < 0.5 && qAbs(markInd->height() - 18) < 0.5, "Library selector indicator size changed");
            require(qAbs(indCenter.x() - markBox->width() / 2.0) <= 0.6 && qAbs(indCenter.y() - markBox->height() / 2.0) <= 0.6, "Library selector indicator is not centered");
            require(markBgColor.alphaF() < 0.99, "Library grid selector background must be quiet/translucent rather than solid opaque");
            require(!markRect.intersects(switchRect), "Library selector overlaps enable switch");
            auto *markAcc = QAccessible::queryAccessibleInterface(markBox);
            require(markAcc && markAcc->role() == QAccessible::CheckBox && markAcc->text(QAccessible::Name).contains("Select Selectable library poster"), "Library selector accessibility role/name missing");
            move(markBox);
            require(markBox->property("hovered").toBool(), "Offscreen pointer did not hover library selector");
            save("library-selector-hover");
            realClickItem(window, markBox);
            require(object->property("cardMarked").toBool() && markBox->property("checked").toBool(), "Library selector pointer click failed");
            save("library-selector-checked");
            item->forceActiveFocus(Qt::OtherFocusReason);
            auto *empty = qobject_cast<QQuickItem *>(named(object.get(), "emptyHomePoster"));
            move(empty);
            require(markBox->isVisible(), "Checked library selector disappeared when unhovered");
            markBox->forceActiveFocus(Qt::TabFocusReason);
            settle();
            realKey(window, Qt::Key_Space);
            settle();
            require(!object->property("cardMarked").toBool() && !markBox->property("checked").toBool(), "Library selector Space toggle failed");
            item->forceActiveFocus(Qt::OtherFocusReason);
            move(empty);
            auto *emptyPreview = named(empty, "previewImage")->parent()->parent();
            require(empty->property("hovered").toBool() && qAbs(emptyPreview->property("scale").toDouble()-1)<.001, "No-art Home placeholder zooms");
            auto *home = qobject_cast<QQuickItem *>(named(object.get(), "artHomePoster"));
            move(home);
            auto *homePreview = named(home, "previewImage")->parent()->parent();
            wait([&] { return homePreview->property("ready").toBool(); });
            settle();
            require(qAbs(named(homePreview,"previewImage")->property("scale").toDouble()-1.05)<.001 && home->scale()==1, "Home artwork zoom or stable button geometry wrong");
            save("home-hover");
            QMetaObject::invokeMethod(object.get(), "reduced", Q_ARG(QVariant, true));
            settle();
            require(qAbs(named(homePreview,"previewImage")->property("scale").toDouble()-1)<.001, "Home reduced motion did not stop zoom");
            QMetaObject::invokeMethod(object.get(), "reduced", Q_ARG(QVariant, false));
            move(hidden);
            auto *artwork = qobject_cast<QQuickItem *>(named(preview, "previewImage"));
            require(hidden->property("lifted").toBool() && qAbs(artwork->scale()-1.05)<.001, "Ready MediaCard must zoom and lift before the live toggle");
            const auto liftedPosition = preview->mapToItem(hidden, QPointF());
            require(qAbs(liftedPosition.y()+3)<.001, "MediaCard did not physically lift");
            QMetaObject::invokeMethod(object.get(), "reduced", Q_ARG(QVariant, true));
            settle();
            const auto restingPosition = preview->mapToItem(hidden, QPointF());
            require(!hidden->property("lifted").toBool() && qAbs(artwork->scale()-1)<.001, "Poster reduced motion did not stop artwork zoom/lift");
            require(qAbs(restingPosition.x()-liftedPosition.x())<.001 && qAbs(restingPosition.y()-liftedPosition.y()-3)<.001, "Reduced motion did not restore mapped MediaCard position");
            save("reduced-motion");
            std::puts("PASS UI poster controls: hidden geometry/reveal pointer, real artwork, footer actions, hover and reduced motion");
            QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL UI poster controls: %s\n", error.what()); QCoreApplication::exit(1);
        }
    });
}
