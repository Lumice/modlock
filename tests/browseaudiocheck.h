#pragma once
// Browse sound previews through the normal QML controls. Part 1 instantiates
// production CatalogRow cards (grid and list, minimum widths) in a fixture
// result host so the controls are interactive; the real AudioPreviewController
// from the app's context runs with a FakeNetwork transport and FakeOutput.
// Part 2 drives the real BrowsePage: the Sounds segment, a production
// delegate, and every invalidation path that the demo catalog can reach.
#include "audiofakes.h"
#include <QQmlExpression>
#include <functional>
#include <QThread>

static void startBrowseAudioCheck(QQuickWindow *window, LibraryModel *model) {
    const bool dark = qEnvironmentVariable("AUDIO_THEME") != "light";
    auto palette = QGuiApplication::palette();
    palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
    const QColor text(dark ? "#eee8dc" : "#1b1626");
    for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) palette.setColor(role, text);
    QGuiApplication::setPalette(palette);
    window->resize(1200, 780);
    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("browse")));
    QTimer::singleShot(1800, window, [window, model, dark] {
        try {
            const QString theme = dark ? QStringLiteral("dark") : QStringLiteral("light");
            auto *audio = qobject_cast<AudioPreviewController *>(qvariant_cast<QObject *>(qmlEngine(window)->rootContext()->contextProperty("audioPreview")));
            require(audio, "audioPreview context controller missing");
            require(!audio->outputCreated(), "audio output created at startup");
            auto *network = new FakeNetwork(window);
            auto output = std::make_shared<FakeOutputState>();
            audio->setNetworkAccessManager(network);
            audio->setOutputFactory(fakeOutputFactory(output));
            const auto settle = [] { QEventLoop loop; QTimer::singleShot(160, &loop, &QEventLoop::quit); loop.exec(); QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete); };
            const auto wait = [](const std::function<bool()> &condition, const char *message) {
                QElapsedTimer elapsed; elapsed.start();
                while (!condition() && elapsed.elapsed() < 3000) { QCoreApplication::processEvents(); QCoreApplication::sendPostedEvents(nullptr, QEvent::DeferredDelete); QThread::msleep(5); }
                require(condition(), message);
            };
            const auto save = [&](const QString &name) {
                const auto directory = qEnvironmentVariable("AUDIO_CAPTURE_DIR");
                if (directory.isEmpty()) return;
                require(QDir().mkpath(directory), "Cannot create audio capture directory");
                require(window->grabWindow().save(directory + "/" + theme + "-" + name + ".png"), "Audio capture failed");
            };
            const auto accessibleName = [](QObject *item) {
                auto *accessible = QAccessible::queryAccessibleInterface(item);
                return accessible ? accessible->text(QAccessible::Name) : QString();
            };
            const auto rectIn = [](QQuickItem *item, QQuickItem *card) { return QRectF(item->mapToItem(card, QPointF()), item->size()); };
            QFile mp3File(qEnvironmentVariable("AUDIO_FIXTURES") + "/silence-250ms.mp3"), oggFile(qEnvironmentVariable("AUDIO_FIXTURES") + "/silence-250ms.ogg");
            require(mp3File.open(QIODevice::ReadOnly) && oggFile.open(QIODevice::ReadOnly), "AUDIO_FIXTURES not readable");
            const auto mp3Bytes = mp3File.readAll(), oggBytes = oggFile.readAll();

            // ---- Part 1: production CatalogRow cards in a fixture host.
            QQmlComponent component(qmlEngine(window));
            component.setData(R"QML(import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle
Rectangle {
    id: fixture
    anchors.fill: parent
    z: 100000
    color: Theme.window
    readonly property bool darkPalette: Tokens.dark
    property int details: 0
    property int downloads: 0
    property int favorites: 0
    function setReduced(value) { Preferences.reduceMotion = value }
    Component.onCompleted: { Preferences.showMaturePreviews = false; Preferences.reduceMotion = false }
    FontMetrics { id: body; font.pointSize: Theme.body }
    component Host: QtObject {
        property string scope: "browse"
        property string presentationMode: "grid"
        property real width: 230
        property real cellWidth: width
        property real gridHeight: 282
        property real listHeight: 64
        property real posterFooterHeight: 52
        property bool ready: true
        property bool modelCoherent: true
        property bool viewerOpen: false
        function inRange(item) { return true }
        function issueToken() { return "audio-fixture" }
        function selectIndex(index, identity) {}
        function userInteracted() {}
        function openViewer(owner, button) {}
    }
    Host { id: gridHost; objectName: "gridHost" }
    Host { id: listHost; objectName: "listHost"; presentationMode: "list"; width: 672; listHeight: Math.max(64, Math.ceil(body.height * 3 + 16)) }
    Repeater {
        model: [
            {name: "gridA", x: 16, y: 16, grid: true, id: "gamebanana:sound:101", kind: "sound", url: "https://files.gamebanana.com/preview/sounds/a.mp3", mature: false, title: "Peacemaker opening announcer with a long two line title"},
            {name: "gridB", x: 248, y: 16, grid: true, id: "gamebanana:sound:102", kind: "sound", url: "https://files.gamebanana.com/preview/sounds/b.ogg", mature: false, title: "Pinball sinners"},
            {name: "gridMature", x: 480, y: 16, grid: true, id: "gamebanana:sound:103", kind: "sound", url: "https://files.gamebanana.com/preview/sounds/c.mp3", mature: true, title: "Mature voice pack"},
            {name: "gridNone", x: 712, y: 16, grid: true, id: "gamebanana:sound:104", kind: "sound", url: "", mature: false, title: "Sound without a preview"},
            {name: "gridMod", x: 944, y: 16, grid: true, id: "gamebanana:mod:105", kind: "mod", url: "", mature: false, title: "Ordinary image mod"},
            {name: "listA", x: 16, y: 320, grid: false, id: "gamebanana:sound:111", kind: "sound", url: "https://files.gamebanana.com/preview/sounds/a.mp3", mature: false, title: "List sound with preview"},
            {name: "listMature", x: 16, y: 400, grid: false, id: "gamebanana:sound:113", kind: "sound", url: "https://files.gamebanana.com/preview/sounds/c.mp3", mature: true, title: "List mature sound"},
            {name: "listNone", x: 16, y: 480, grid: false, id: "gamebanana:sound:114", kind: "sound", url: "", mature: false, title: "List sound without a preview"},
            {name: "listMod", x: 16, y: 560, grid: false, id: "gamebanana:mod:115", kind: "mod", url: "", mature: false, title: "List image mod"}
        ]
        delegate: CatalogRow {
            required property var modelData
            objectName: modelData.name
            x: modelData.x; y: modelData.y
            resultHost: modelData.grid ? gridHost : listHost
            identity: modelData.id
            title: modelData.title
            author: "Fixture author"; category: "Voices"; hero: ""
            favoriteKnown: true; favorite: false; collection: false
            submission: ({provider: "gamebanana", submissionType: modelData.kind, submissionId: modelData.id.replace(/\D/g, "")})
            metadata: ({downloads: "1234", likes: "56", audioUrl: modelData.url, masked: modelData.mature})
            ownerSnapshot: ({identity: modelData.id, source: "", sources: [], hidden: modelData.mature, title: modelData.title})
            installVisible: true; installEnabled: true; installText: "Download"
            catalogStats: [{icon: "download", text: "1.2k"}]
            statsText: "1,234 downloads"
            onDetailsRequested: fixture.details++
            onInstallRequested: fixture.downloads++
            onActionRequested: fixture.favorites++
        }
    }
}
)QML", QUrl());
            std::unique_ptr<QObject> fixture(component.create());
            if (!fixture) qWarning() << component.errors();
            require(bool(fixture), "Browse audio fixture did not load");
            qobject_cast<QQuickItem *>(fixture.get())->setParentItem(window->contentItem());
            settle();
            require(fixture->property("darkPalette").toBool() == dark, "Audio fixture theme does not match capture label");
            const auto card = [&](const char *name) { return qobject_cast<QQuickItem *>(named(fixture.get(), name)); };
            const auto part = [&](QQuickItem *owner, const char *name) { return qobject_cast<QQuickItem *>(findNamed(owner, name)); };
            const auto counters = [&] { return fixture->property("details").toInt() + fixture->property("downloads").toInt() + fixture->property("favorites").toInt(); };

            for (const char *name : {"gridA", "listA"}) {
                auto *row = card(name);
                require(row->width() == (QByteArray(name).startsWith("grid") ? 222 : 664), "fixture card not at the minimum/list width");
                auto *play = part(row, "soundPlayButton");
                require(play && play->isVisible() && play->isEnabled(), "sound card play button missing");
                require(!part(row, "rowPreview")->isVisible(), "sound card still shows image artwork");
                require(accessibleName(play) == "Play sound preview of " + row->property("heading").toString(), "play accessible name wrong");
                const auto button = rectIn(play, row);
                require(QRectF(0, 0, row->width(), row->height()).contains(button), "play button outside its card");
                for (const char *other : {"rowTitle", "favoriteButton", "installButton", "catalogStatsRow"}) {
                    auto *item = part(row, other);
                    require(item && item->isVisible(), "card control missing");
                    require(!button.intersects(rectIn(item, row)), "play button overlaps title, stats, favorite or Download");
                }
            }
            for (const char *name : {"gridNone", "listNone"}) {
                auto *row = card(name);
                auto *note = part(row, "soundNote");
                require(note && note->isVisible() && note->property("text").toString() == "No sound preview", "missing URL lacks No sound preview");
                require(!part(row, "soundPlayButton")->isVisible() && !part(row, "soundRevealButton")->isVisible(), "missing URL offers a control");
            }
            for (const char *name : {"gridMature", "listMature"}) {
                auto *row = card(name);
                require(part(row, "soundRevealButton")->isVisible() && !part(row, "soundPlayButton")->isVisible(), "mature sound playable without reveal");
                require(!rectIn(part(row, "soundRevealButton"), row).intersects(rectIn(part(row, "rowTitle"), row)), "reveal overlaps the title");
            }
            for (const char *name : {"gridMod", "listMod"}) {
                auto *row = card(name);
                require(!findNamed(row, "soundPreview") && part(row, "rowPreview")->isVisible(), "image mod card changed");
            }
            save("idle");

            // Click isolation and loading.
            auto *gridA = card("gridA");
            auto *playA = part(gridA, "soundPlayButton");
            realClickItem(window, playA);
            require(network->requests.size() == 1 && network->requests.last().url() == QUrl("https://files.gamebanana.com/preview/sounds/a.mp3"), "Play did not issue the checked fetch");
            require(audio->state() == "loading" && audio->currentIdentity() == "gamebanana:sound:101" && !audio->outputCreated(), "loading state wrong");
            QObject *busy = nullptr;
            for (auto *object : objects(playA)) if (object->inherits("QQuickBusyIndicator")) busy = object;
            require(busy && busy->property("running").toBool(), "loading indicator missing");
            fixture->metaObject()->invokeMethod(fixture.get(), "setReduced", Q_ARG(QVariant, true));
            require(!busy->property("running").toBool(), "loading indicator animates under reduced motion");
            fixture->metaObject()->invokeMethod(fixture.get(), "setReduced", Q_ARG(QVariant, false));
            require(accessibleName(playA).startsWith("Stop loading"), "loading button name wrong");
            realClickItem(window, playA);
            require(audio->state() == "idle" && network->replies.first()->aborted, "second click did not cancel loading");
            { // Double click on the control never opens details.
                const auto p = playA->mapToScene(QPointF(playA->width() / 2, playA->height() / 2));
                const auto g = window->mapToGlobal(p.toPoint());
                QMouseEvent press(QEvent::MouseButtonPress, p, p, g, Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
                QMouseEvent release(QEvent::MouseButtonRelease, p, p, g, Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
                QMouseEvent doubled(QEvent::MouseButtonDblClick, p, p, g, Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
                QMouseEvent press2(QEvent::MouseButtonPress, p, p, g, Qt::LeftButton, Qt::LeftButton, Qt::NoModifier);
                QMouseEvent release2(QEvent::MouseButtonRelease, p, p, g, Qt::LeftButton, Qt::NoButton, Qt::NoModifier);
                QCoreApplication::sendEvent(window, &press); QCoreApplication::sendEvent(window, &release);
                QCoreApplication::sendEvent(window, &press2); QCoreApplication::sendEvent(window, &doubled); QCoreApplication::sendEvent(window, &release2);
                settle();
            }
            require(counters() == 0, "sound control click reached details, favorite or Download");
            audio->stop();

            // Play, pause, resume.
            const auto requests = network->requests.size();
            realClickItem(window, playA);
            require(network->requests.size() == requests + 1, "Play did not fetch again");
            network->last()->serve(200, "audio/mpeg", mp3Bytes);
            require(audio->state() == "playing" && output->created == 1 && output->format.sampleRate() == 44100, "MP3 did not start with fake output");
            require(accessibleName(playA) == "Pause sound preview of " + gridA->property("heading").toString(), "playing button name wrong");
            require(accessibleName(part(card("listA"), "soundPlayButton")).startsWith("Play "), "another owner shows playing");
            save("playing");
            realClickItem(window, playA);
            require(audio->state() == "paused" && output->paused, "Pause click failed");
            realClickItem(window, playA);
            require(audio->state() == "playing" && !output->paused, "Resume click failed");

            // Switching to B replaces A; B ends back at Play.
            auto *playB = part(card("gridB"), "soundPlayButton");
            realClickItem(window, playB);
            require(audio->currentIdentity() == "gamebanana:sound:102" && audio->state() == "loading" && output->destroyed == 1, "switching did not stop A");
            require(accessibleName(playA).startsWith("Play "), "A still shows Pause after switching");
            network->last()->serve(200, "application/octet-stream", oggBytes);
            require(audio->state() == "playing" && output->created == 2, "Vorbis did not start");
            for (int guard = 0; audio->state() == "playing" && guard < 100000; ++guard) { output->drain(); audio->pumpNow(); }
            require(audio->state() == "idle" && output->destroyed == 2 && accessibleName(playB).startsWith("Play "), "end did not return B to Play");

            // Error: plain message on the card.
            realClickItem(window, playA);
            network->last()->serve(200, "text/html", "<!DOCTYPE html><html></html>");
            require(audio->state() == "error" && audio->errorKind() == "unsupported", "HTML admitted as audio");
            auto *note = part(gridA, "soundNote");
            require(note->isVisible() && note->property("text").toString() == AudioPreviewController::describe("unsupported"), "error message not shown on the card");
            require(accessibleName(playA).startsWith("Try sound preview of"), "error retry name wrong");
            save("error");
            audio->stop();

            // The compact row must retain a visible failure after hover/focus
            // leave, at the application's actual minimum window width.
            auto *listError = card("listA");
            auto *listPlay = part(listError, "soundPlayButton");
            listPlay->setProperty("hoverEnabled", true);
            realClickItem(window, listPlay);
            network->last()->serve(200, "text/html", "<!DOCTYPE html><html></html>");
            window->resize(window->minimumWidth(), 780);
            qobject_cast<QQuickItem *>(fixture.get())->forceActiveFocus(Qt::OtherFocusReason);
            const QPointF outside(window->width() - 8, window->height() - 8);
            QMouseEvent leave(QEvent::MouseMove, outside, outside, window->mapToGlobal(outside.toPoint()), Qt::NoButton, Qt::NoButton, Qt::NoModifier);
            QCoreApplication::sendEvent(window, &leave);
            settle();
            auto *listNote = part(listError, "soundNote");
            require(!listPlay->hasActiveFocus() && !listPlay->property("hovered").toBool(), "list error check still depends on focus or hover");
            require(listNote->isVisible() && listNote->property("text").toString() == "Can't play", "compact sound failure has no persistent indication");
            const auto errorBounds = rectIn(listNote, listError);
            require(QRectF(0, 0, listError->width(), listError->height()).contains(errorBounds), "list error indicator leaves its row");
            for (const char *name : {"rowTitle", "favoriteButton", "installButton"})
                require(!errorBounds.intersects(rectIn(part(listError, name), listError)), "list error indicator overlaps a title or action");
            save("list-error-minimum");
            window->resize(1200, 780);
            settle();
            audio->stop();

            // Owner-specific mature reveal, never an implicit play.
            auto *mature = card("gridMature");
            const auto beforeReveal = network->requests.size();
            realClickItem(window, part(mature, "soundRevealButton"));
            require(audio->revealedIdentities().contains("gamebanana:sound:103") && network->requests.size() == beforeReveal && audio->state() == "idle", "reveal started playback or did not record");
            require(part(mature, "soundPlayButton")->isVisible() && !part(mature, "soundRevealButton")->isVisible(), "revealed card lacks Play");
            require(part(card("listMature"), "soundRevealButton")->isVisible(), "reveal leaked to another owner");
            save("revealed");
            realClickItem(window, part(mature, "soundPlayButton"));
            require(network->requests.size() == beforeReveal + 1 && audio->state() == "loading", "revealed Play did not fetch");
            audio->invalidate();
            require(part(mature, "soundRevealButton")->isVisible(), "invalidation kept the reveal");
            fixture->setProperty("visible", false);
            require(counters() == 0, "sound controls reached card actions");
            fixture.reset();
            settle();

            // ---- Part 2: the real BrowsePage.
            realClick(window, "browseSoundsButton");
            wait([&] { return model->catalog()->queryState().value("kind") == "sound"; }, "Sounds segment did not select sounds");
            require(named(window, "browseSoundsButton")->property("checked").toBool(), "Sounds segment not checked");
            for (auto *object : objects(window)) {
                if (!object->inherits("QQuickComboBox")) continue;
                for (const auto &entry : object->property("model").toList())
                    require(entry.toMap().value("value") != "sound", "Sounds still listed under More");
            }
            QObject *page = nullptr;
            for (auto *object : objects(window)) if (object->property("audioContext").isValid()) page = object;
            require(page, "BrowsePage audio context missing");
            const auto rows = [&] {
                CatalogRow sound;
                sound.key = {"sound", "201"}; sound.identity = sound.key.identity(); sound.title = "Real delegate sound";
                sound.author = "Fixture"; sound.state = "ready"; sound.masked = false; sound.favoriteKnown = true;
                sound.metadata = {{"downloads", 10}, {"likes", 2}, {"audioUrl", "https://files.gamebanana.com/preview/sounds/r.mp3"}, {"masked", false}};
                CatalogRow mod = sound;
                mod.key = {"mod", "202"}; mod.identity = mod.key.identity(); mod.title = "Real delegate mod";
                mod.metadata = {{"downloads", 10}, {"likes", 2}, {"thumbnail", "file:///missing-fixture.png"}, {"masked", false}};
                CatalogRow unknown = sound;
                unknown.key = {"sound", "203"}; unknown.identity = unknown.key.identity(); unknown.title = "Real unknown-rated sound";
                unknown.metadata = {{"downloads", 10}, {"likes", 2}, {"audioUrl", "https://files.gamebanana.com/preview/sounds/u.mp3"}};
                model->catalog()->model()->replace({sound, mod, unknown}, {}, {});
                return sound.identity;
            };
            const auto identity = rows();
            QQuickItem *real = nullptr, *realUnknown = nullptr;
            wait([&] {
                real = nullptr; realUnknown = nullptr;
                for (auto *object : objects(window)) {
                    if (object->objectName() != "catalogRow") continue;
                    const auto heading = object->property("heading").toString();
                    if (heading == "Real delegate sound") real = qobject_cast<QQuickItem *>(object);
                    if (heading == "Real unknown-rated sound") realUnknown = qobject_cast<QQuickItem *>(object);
                }
                return real && findNamed(real, "soundPlayButton") && realUnknown && findNamed(realUnknown, "soundRevealButton");
            }, "real Browse delegate has no sound preview");
            require(part(real, "soundPlayButton")->isVisible() && !part(real, "rowPreview")->isVisible(), "real delegate did not switch to the sound panel");
            require(part(realUnknown, "soundRevealButton")->isVisible() && !part(realUnknown, "soundPlayButton")->isVisible(), "unknown-rated sound not concealed on real BrowsePage");
            const auto url = QStringLiteral("https://files.gamebanana.com/preview/sounds/r.mp3");
            const auto startPlaying = [&] {
                audio->reveal(identity);
                audio->toggle(identity, url, false);
                network->last()->serve(200, "audio/mpeg", mp3Bytes);
                require(audio->state() == "playing", "programmatic play for invalidation failed");
            };
            const auto expectInvalidated = [&](const char *message) {
                settle();
                require(audio->state() == "idle" && audio->revealedIdentities().isEmpty(), message);
            };
            startPlaying();
            rows();
            expectInvalidated("results reset did not stop playback");
            startPlaying();
            QQmlExpression(qmlContext(page), page, "Preferences.showMaturePreviews = true").evaluate();
            expectInvalidated("maturity preference change did not stop playback");
            QQmlExpression(qmlContext(page), page, "Preferences.showMaturePreviews = false").evaluate();
            startPlaying();
            page->setProperty("sourceDomain", page->property("sourceDomain").toString() + "-other-root");
            expectInvalidated("library/root context change did not stop playback");
            rows();
            startPlaying();
            model->catalog()->updateQuery({{"search", "announcer"}});
            expectInvalidated("query change did not stop playback");
            rows();
            startPlaying();
            QPointer<QObject> browse(page);
            model->catalog()->openItem(identity);
            wait([&] { return model->catalog()->route() != "browse" && !browse; }, "opening details did not leave Browse");
            expectInvalidated("route change did not stop playback");
            require(output->created == output->destroyed, "an output outlived its owner");
            std::printf("PASS UI Browse audio %s: grid/list controls, isolation, play/pause/switch/end/error, reveal, real Sounds segment and invalidation\n", qPrintable(theme));
            QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL Browse audio: %s\n", error.what());
            QCoreApplication::exit(1);
        }
    });
}
