#pragma once
#include <QPainter>
#include <QTemporaryDir>
#include <QThread>

// ModDetails spotlight: privacy transitions, instant clear, fade-in and live reduced motion on
// the production component. SPOTLIGHT_EXPECT=effect requires actual effect pixels (an RHI
// renderer); otherwise the ordinary Software adaptation must keep the plain surface.
static void startSpotlightCheck(QQuickWindow *window) {
    QTimer::singleShot(0, window, [window] {
        try {
            const bool dark = qEnvironmentVariable("SPOTLIGHT_THEME") != "light";
            const bool expectEffect = qEnvironmentVariable("SPOTLIGHT_EXPECT") == "effect";
            const bool software = window->rendererInterface()->graphicsApi() == QSGRendererInterface::Software;
            require(expectEffect != software, expectEffect ? "Effect run is not using an RHI renderer" : "Plain run is not using the Software adaptation");
            auto palette = QGuiApplication::palette();
            palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
            const QColor text(dark ? "#eee8dc" : "#1b1626");
            for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) palette.setColor(role, text);
            QGuiApplication::setPalette(palette);
            window->resize(940, 720);
            QTemporaryDir files;
            require(files.isValid(), "Spotlight artwork fixture directory failed");
            // Textured synthetic art keeps a solid centre for the sharp-preview pixel checks.
            const auto paint = [&](const QString &name, const QColor &base, const QColor &stripe) {
                QImage image(320, 200, QImage::Format_RGB32);
                image.fill(base);
                QPainter painter(&image);
                for (int x = 0; x < 320; x += 40) {
                    if (x >= 120 && x < 200) continue;
                    painter.fillRect(x, 0, 16, 200, stripe);
                }
                painter.end();
                require(image.save(files.filePath(name)), "Spotlight artwork fixture failed");
                return QUrl::fromLocalFile(files.filePath(name)).toString();
            };
            const bool approved = !qEnvironmentVariableIsEmpty("SPOTLIGHT_ART_FIRST");
            const auto first = approved ? QUrl::fromLocalFile(qEnvironmentVariable("SPOTLIGHT_ART_FIRST")).toString()
                                        : paint("first.png", QColor(210, 45, 65), QColor(250, 200, 70));
            const auto second = approved ? QUrl::fromLocalFile(qEnvironmentVariable("SPOTLIGHT_ART_SECOND")).toString()
                                         : paint("second.png", QColor(30, 115, 220), QColor(80, 220, 160));
            QQmlComponent component(qmlEngine(window));
            component.setData(R"QML(import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle
Rectangle {
    id: fixture
    required property string first
    required property string second
    anchors.fill: parent
    z: 100000
    color: Theme.window
    readonly property bool darkPalette: Tokens.dark
    function reduced(value) { Preferences.reduceMotion = value }
    function art(hidden) {
        return {id: "spotlight", title: "Spotlight synthetic artwork", enabled: true, images: [first, second],
                previewHidden: hidden, installedVpks: [], missingVpks: []}
    }
    Component.onCompleted: { Preferences.showMaturePreviews = false; Preferences.reduceMotion = false }
    // Side inspector width and the actual styled dialog content/overlay host.
    ModDetails { objectName: "inspectorHost"; x: 0; y: 0; width: 330; height: 720; readOnly: true
                 mod: fixture.art(false); mediaContextKey: "spotlight-inspector" }
    Dialog {
        parent: Overlay.overlay
        popupType: Popup.Item
        x: 360; y: 20; width: 540; height: 680
        modal: false; title: "Mod details"; standardButtons: Dialog.Close
        Component.onCompleted: open()
        ModDetails { objectName: "dialogHost"; anchors.fill: parent; readOnly: true
                     mod: fixture.art(false); mediaContextKey: "spotlight-dialog" }
    }
})QML", QUrl());
            std::unique_ptr<QObject> object(component.createWithInitialProperties({{"first", first}, {"second", second}}));
            if (!object) qWarning() << component.errors();
            require(bool(object), "Spotlight component did not load");
            auto *item = qobject_cast<QQuickItem *>(object.get());
            item->setParentItem(window->contentItem());
            const auto wait = [&](auto condition, const char *what) {
                QElapsedTimer elapsed; elapsed.start();
                while (!condition() && elapsed.elapsed() < 4000) { QCoreApplication::processEvents(); QThread::msleep(5); }
                require(condition(), what);
            };
            const auto settle = [] { QEventLoop loop; QTimer::singleShot(320, &loop, &QEventLoop::quit); loop.exec(); };
            const auto save = [&](const QString &name) {
                const auto directory = qEnvironmentVariable("SPOTLIGHT_CAPTURE_DIR");
                if (directory.isEmpty()) return;
                require(QDir().mkpath(directory), "Cannot create spotlight capture directory");
                require(window->grabWindow().save(directory + "/" + name + ".png"), "Spotlight capture failed");
            };
            auto *inspector = qobject_cast<QQuickItem *>(named(object.get(), "inspectorHost"));
            auto *dialog = qobject_cast<QQuickItem *>(named(object.get(), "dialogHost"));
            const auto spotlightOf = [](QObject *host) { return qobject_cast<QQuickItem *>(named(host, "detailSpotlight")); };
            const auto previewOf = [](QObject *host) { return named(host, "detailPreview"); };
            const auto effectOf = [](QObject *host) { return named(host, "detailSpotlightEffect"); };
            // Sample the decorative gap above the sharp preview, never its pixels.
            const auto backdropPixel = [&](QObject *host) {
                auto *preview = qobject_cast<QQuickItem *>(previewOf(host));
                const auto point = preview->mapToScene(QPointF(preview->width() / 2, -8));
                const QRectF sharp(preview->mapToScene(QPointF()), preview->size());
                require(!sharp.contains(point), "Backdrop witness overlaps the sharp preview");
                const auto pixels = window->grabWindow();
                return pixels.pixelColor(qRound(point.x() * pixels.width() / window->width()), qRound(point.y() * pixels.height() / window->height()));
            };
            const auto distance = [](const QColor &a, const QColor &b) {
                return qAbs(a.red() - b.red()) + qAbs(a.green() - b.green()) + qAbs(a.blue() - b.blue());
            };
            const auto artMap = [&](bool hidden) {
                return QVariantMap{{"id", "spotlight"}, {"title", "Spotlight synthetic artwork"}, {"enabled", true},
                                   {"images", QStringList{first, second}}, {"previewHidden", hidden},
                                   {"installedVpks", QStringList{}}, {"missingVpks", QStringList{}}};
            };
            const auto panelFor = [](QObject *host) { return host->property("background").value<QObject *>()->property("color").value<QColor>(); };
            // Immediately after an unsafe change, before any event processing.
            const auto cleared = [&](QObject *host, const char *what) {
                auto *spotlight = spotlightOf(host);
                require(!host->property("spotlightEligible").toBool(), what);
                require(!spotlight->isVisible() && spotlight->opacity() == 0 && !spotlight->property("fading").toBool(), what);
                require(!effectOf(host)->property("active").toBool() && !effectOf(host)->property("item").value<QObject *>(), what);
            };
            const auto plainPixels = [&](QObject *host, const char *what) {
                if (expectEffect) settle();
                require(distance(backdropPixel(host), panelFor(host)) < 12, what);
            };
            const auto shown = [&](QObject *host, const char *what) {
                auto *spotlight = spotlightOf(host);
                wait([&] { return host->property("spotlightEligible").toBool() && !spotlight->property("fading").toBool() && spotlight->opacity() == 1; }, what);
                require(spotlight->isVisible(), what);
                std::fprintf(stderr,"SPOTLIGHT_GEOMETRY %.2f %.2f\n",spotlight->width(),spotlight->height());
                require(spotlight->width()>0 && spotlight->height()>0, "Spotlight has no drawable geometry");
                require(effectOf(host)->property("active").toBool() == expectEffect, "Spotlight effect gate does not match the renderer");
                if (expectEffect) {
                    settle();
                    const auto panel = panelFor(host);
                    const auto pixel = backdropPixel(host);
                    std::fprintf(stderr,"SPOTLIGHT_PIXEL %s panel=%s pixel=%s delta=%d\n",qPrintable(host->objectName()),qPrintable(panel.name()),qPrintable(pixel.name()),distance(pixel,panel));
                    save("witness-" + host->objectName());
                    require(distance(backdropPixel(host), panel) > 30, "Effect renderer did not draw the spotlight backdrop");
                    const auto tail = spotlight->mapToScene(QPointF(spotlight->width()/2, spotlight->height()-2));
                    const auto frame = window->grabWindow();
                    const auto tailPixel = frame.pixelColor(qRound(tail.x()*frame.width()/window->width()),qRound(tail.y()*frame.height()/window->height()));
                    require(distance(tailPixel,panel)<12, "Spotlight does not fade back to the host panel below the title");
                } else {
                    require(distance(backdropPixel(host), panelFor(host)) < 12, "Software adaptation drew something behind the preview");
                }
            };
            wait([&] { return object->property("darkPalette").toBool() == dark; }, "Spotlight fixture theme does not match capture label");
            wait([&] { return imageReady(previewOf(inspector)) && imageReady(previewOf(dialog)); }, "Spotlight previews did not load");
            shown(inspector, "Inspector spotlight did not appear for ready artwork");
            shown(dialog, "Dialog spotlight did not appear for ready artwork");
            if (!approved) {
                renderedColor(window, previewOf(inspector), QColor(210, 45, 65));
                renderedColor(window, previewOf(dialog), QColor(210, 45, 65));
            }
            save("ready");

            // Gallery change: instant clear, then a fade that live reduced motion stops at once.
            click(inspector, "nextPreview");
            cleared(inspector, "Gallery change kept the previous backdrop");
            wait([&] { return inspector->property("spotlightEligible").toBool(); }, "Next gallery image did not become eligible");
            auto *spotlight = spotlightOf(inspector);
            require(spotlight->property("fading").toBool() && spotlight->opacity() < 1, "Spotlight did not fade in");
            QMetaObject::invokeMethod(object.get(), "reduced", Q_ARG(QVariant, true));
            require(!spotlight->property("fading").toBool() && spotlight->opacity() == 1, "Live reduced motion did not stop the fade");
            if (!approved) renderedColor(window, previewOf(inspector), QColor(30, 115, 220));
            save("reduced-motion");
            click(inspector, "previousPreview");
            cleared(inspector, "Gallery change under reduced motion kept the previous backdrop");
            wait([&] { return inspector->property("spotlightEligible").toBool(); }, "Previous gallery image did not become eligible");
            require(!spotlight->property("fading").toBool() && spotlight->opacity() == 1, "Reduced motion still faded");
            QMetaObject::invokeMethod(object.get(), "reduced", Q_ARG(QVariant, false));
            shown(inspector, "Spotlight did not return after the gallery round trip");

            // Mature/hidden selection, reveal, owner/context change, no art, back to ready.
            inspector->setProperty("mod", artMap(true));
            cleared(inspector, "Hidden selection kept the backdrop");
            require(previewOf(inspector)->property("masked").toBool(), "Hidden fixture is not masked");
            plainPixels(inspector, "Masked selection drew backdrop pixels");
            save("masked");
            click(previewOf(inspector), "revealPreview");
            shown(inspector, "Revealed artwork did not show the spotlight");
            // Controlled context injection (a library root, profile or selection switch changes this key).
            inspector->setProperty("mediaContextKey", "spotlight-inspector-new-owner");
            cleared(inspector, "Owner/context change kept the revealed backdrop");
            require(previewOf(inspector)->property("masked").toBool(), "Reveal carried to a new owner");
            plainPixels(inspector, "New owner drew the revealed backdrop");
            inspector->setProperty("mod", QVariantMap{{"id", "spotlight"}, {"title", "No artwork"}, {"images", QStringList{}},
                                                      {"installedVpks", QStringList{}}, {"missingVpks", QStringList{}}});
            cleared(inspector, "No-art selection kept a backdrop");
            plainPixels(inspector, "No-art selection drew backdrop pixels");
            save("no-art");
            inspector->setProperty("mod", artMap(false));
            shown(inspector, "Ready artwork did not return the spotlight");

            // Inactive/hidden panel.
            inspector->setVisible(false);
            cleared(inspector, "Hidden panel kept its backdrop");
            inspector->setVisible(true);
            shown(inspector, "Spotlight did not return when the panel reappeared");
            // Imported names may wrap far beyond the viewport; decoration must stay bounded.
            auto longTitle = artMap(false);
            longTitle.insert("title", QStringLiteral("Long imported mod title ").repeated(256));
            inspector->setProperty("mod", longTitle);
            wait([&] { return named(inspector,"detailTitle")->property("height").toDouble()>384; }, "Long-title bound fixture did not wrap");
            require(spotlightOf(inspector)->height()>0 && spotlightOf(inspector)->height()<=384, "Long title creates an oversized spotlight target");
            inspector->setProperty("mod", artMap(false));
            wait([&] { return named(inspector,"detailTitle")->property("height").toDouble()<384; }, "Short title layout did not return");
            shown(inspector,"Spotlight did not recover after long-title layout");
            save("final");
            std::puts(expectEffect ? "PASS UI spotlight: effect renderer, privacy transitions, fade and live reduced motion"
                                   : "PASS UI spotlight: software plain surface, privacy transitions, fade and live reduced motion");
            QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL UI spotlight: %s\n", error.what()); QCoreApplication::exit(1);
        }
    });
}
