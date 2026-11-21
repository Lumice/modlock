#pragma once
#include <QElapsedTimer>
#include <QEventLoop>
#include <QImage>
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQmlProperty>
#include <QQuickItem>
#include <QQuickWindow>
#include <QTemporaryDir>
#include <QThread>
#include <QTimer>
#include <cmath>
#include <memory>

namespace {

inline void motionApplyPalette(bool dark) {
    auto palette = QGuiApplication::palette();
    palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
    palette.setColor(QPalette::Base, QColor(dark ? "#120f1b" : "#ffffff"));
    palette.setColor(QPalette::AlternateBase, QColor(dark ? "#14111d" : "#e9e6ef"));
    palette.setColor(QPalette::Button, QColor(dark ? "#1a1626" : "#fbfafd"));
    const QColor text(dark ? "#eee8dc" : "#1b1626");
    const QColor disabled(dark ? "#716a7b" : "#9b94a6");
    for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) {
        palette.setColor(role, text);
        palette.setColor(QPalette::Disabled, role, disabled);
    }
    QGuiApplication::setPalette(palette);
}

inline void motionSaveCapture(QQuickWindow *window, const QString &name) {
    const auto dir = qEnvironmentVariable("MOTION_CAPTURE_DIR");
    if (dir.isEmpty()) return;
    require(QDir().mkpath(dir), "Cannot create motion capture directory");
    require(window->grabWindow().save(dir + "/" + name + ".png"),
            qPrintable("Motion capture failed: " + name));
}

inline void motionPump(int ms) {
    QEventLoop loop;
    QTimer::singleShot(ms, &loop, &QEventLoop::quit);
    loop.exec();
}

inline bool motionWaitFor(const std::function<bool()> &pred, int timeoutMs = 2500) {
    QElapsedTimer t;
    t.start();
    while (t.elapsed() < timeoutMs) {
        if (pred()) return true;
        QCoreApplication::processEvents(QEventLoop::AllEvents, 10);
        QThread::msleep(5);
    }
    return pred();
}

static void startMotionEnterStaggerCheck(QQuickWindow *window, LibraryModel *model) {
    QTimer::singleShot(0, window, [window, model] {
        try {
            const bool dark = qEnvironmentVariable("MOTION_THEME") != "light";
            const QString suffix = dark ? "dark" : "light";
            motionApplyPalette(dark);
            window->resize(1040, 920);
            motionPump(30);
            QTemporaryDir files;
            require(files.isValid(), "Motion fixture temp dir failed");
            QImage image(320, 180, QImage::Format_RGB32);
            image.fill(QColor("#347987"));
            require(image.save(files.filePath("art.png")), "Motion artwork fixture failed");
            const QString artUrl = QUrl::fromLocalFile(files.filePath("art.png")).toString();

            // 1) Production app navigation flow: Home -> Library (1st visit) -> Home -> Library (revisit) -> Grid mode.
            require(motionWaitFor([&] { return !model->loading() && model->count() >= 15; }),
                    "Demo library did not finish loading");
            auto *prefs = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
            auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
            require(prefs && tokens, "Singletons missing");
            prefs->setProperty("reduceMotion", false);
            motionPump(20);
            require(tokens->property("dark").toBool() == dark, "Palette theme mismatch");
            QCoreApplication::processEvents();

            realClick(window, "libraryTab");
            QList<QQuickItem *> liveRows;
            require(motionWaitFor([&] {
                liveRows.clear();
                for (auto *obj : objects(window)) {
                    if (obj->objectName() == "modRow") {
                        if (auto *item = qobject_cast<QQuickItem *>(obj); item && item->isVisible())
                            liveRows.append(item);
                    }
                }
                return liveRows.size() >= 6;
            }), "Library cards did not materialize on first visit");

            QQuickItem *row0 = nullptr;
            QQuickItem *row5 = nullptr;
            for (auto *r : liveRows) {
                const int idx = r->property("index").toInt();
                if (idx == 0) row0 = r;
                if (idx == 5) row5 = r;
            }
            require(row0 && row5, "Expected library rows 0 and 5 missing");
            // Wait long enough for row0 (0ms delay) to advance well ahead of row5 (125ms delay)
            motionPump(65);
            require(row0->property("enterOpacity").toDouble() > row5->property("enterOpacity").toDouble() + 0.05,
                    "Production Library first visit did not stagger row0 ahead of row5");
            require(row0->property("enterOffsetY").toDouble() < row5->property("enterOffsetY").toDouble(),
                    "Production Library first visit row0 y-shift did not lead row5");
            motionSaveCapture(window, "stagger-live-library-mid-" + suffix);

            require(motionWaitFor([&] {
                return qAbs(row0->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(row5->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(row5->property("enterOffsetY").toDouble()) < 0.01;
            }), "Production Library cards did not settle");

            // Navigate away to Home and back to Library -> revisit must NOT replay stagger.
            realClick(window, "homeTab");
            require(motionWaitFor([&] { return findNamed(window, "homePage") != nullptr; }),
                    "Could not return to HomePage");
            motionPump(60);
            realClick(window, "libraryTab");
            QList<QQuickItem *> revisitRows;
            require(motionWaitFor([&] {
                revisitRows.clear();
                for (auto *obj : objects(window)) {
                    if (obj->objectName() == "modRow") {
                        if (auto *item = qobject_cast<QQuickItem *>(obj); item && item->isVisible())
                            revisitRows.append(item);
                    }
                }
                return revisitRows.size() >= 6;
            }), "Library cards missing on second visit");
            for (auto *r : revisitRows) {
                require(qAbs(r->property("enterOpacity").toDouble() - 1.0) < 0.001
                        && qAbs(r->property("enterOffsetY").toDouble()) < 0.01,
                        "Re-visiting Library unexpectedly replayed enter stagger");
            }
            // Switching presentation mode on an already-visited page must NOT replay stagger.
            realClick(window, "gridViewButton");
            auto *modList = named(window, "modList");
            QList<QQuickItem *> gridModeRows;
            require(motionWaitFor([&] {
                if (modList->property("presentationMode").toString() != "grid") return false;
                gridModeRows.clear();
                for (auto *obj : objects(window)) {
                    if (obj->objectName() == "modRow") {
                        if (auto *item = qobject_cast<QQuickItem *>(obj); item && item->isVisible())
                            gridModeRows.append(item);
                    }
                }
                return gridModeRows.size() >= 4;
            }), "Grid view cards missing");
            for (auto *r : gridModeRows) {
                require(qAbs(r->property("enterOpacity").toDouble() - 1.0) < 0.001
                        && qAbs(r->property("enterOffsetY").toDouble()) < 0.01,
                        "Mode switch replayed enter stagger on visited page");
            }

            // 2) Controlled 16-card Grid MediaResultView + HomeModTile check:
            // verifies ~12 card cap (0..11 stagger, >=12 immediate), HomeModTile untransformed root,
            // and Tokens.reduceMotion disable + mid-flight snap.
            window->resize(1040, 1020);
            QQmlComponent harnessComp(qmlEngine(window));
            harnessComp.setData(R"QML(import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle

Rectangle {
    id: harness
    required property string artSource
    anchors.fill: parent
    z: 200000
    color: Theme.window
    property bool secondVisit: false
    property bool reduce: false
    onReduceChanged: Preferences.reduceMotion = reduce
    Component.onCompleted: Preferences.reduceMotion = false

    MediaViewState { id: visitBank }
    ListModel {
        id: cardsModel
        Component.onCompleted: {
            for (let i = 0; i < 16; ++i)
                append({ modId: "c-" + i, modTitle: "Card " + i })
        }
    }
    Loader {
        id: gridLoader
        x: 16; y: 16; width: 1008; height: 930
        active: true
        sourceComponent: MediaResultView {
            id: harnessGrid
            stateBank: visitBank
            model: cardsModel
            scope: "harnessGrid"
            domain: "demo"
            contextKey: "ctx"
            selectedIdentity: ""
            focusKey: ""
            presentationMode: "grid"
            ready: true
            indexForIdentity: id => {
                for (let i = 0; i < cardsModel.count; ++i)
                    if (cardsModel.get(i).modId === id) return i
                return -1
            }
            identityAt: idx => idx >= 0 && idx < cardsModel.count ? cardsModel.get(idx).modId : ""
            snapshotFor: id => ({ identity: id, source: harness.artSource, hidden: false })
            delegateComponent: Component {
                MediaCard {
                    required property string modId
                    required property string modTitle
                    resultHost: harnessGrid
                    ownerSnapshot: ({ identity: modId, source: harness.artSource, hidden: false })
                    anchorIdentity: modId
                    heading: modTitle
                    subtitle: "Skins"
                    actionKind: "none"
                }
            }
        }
    }
    function reloadGrid() {
        gridLoader.active = false
        gridLoader.active = true
    }

    // Controlled HomeModTile enter test
    Item {
        id: homeRow
        x: 16; y: 946; width: 900; height: 70
        property bool armed: false
        property int gen: 0
        HomeModTile {
            objectName: "tile0"; x: 0; y: 0; width: 200; height: 80
            title: "Tile 0"; imageSource: harness.artSource; contextKey: "t0"
            enterIndex: 0; enterArmed: homeRow.armed; enterGeneration: homeRow.gen
        }
        HomeModTile {
            objectName: "tile4"; x: 220; y: 0; width: 200; height: 80
            title: "Tile 4"; imageSource: harness.artSource; contextKey: "t4"
            enterIndex: 4; enterArmed: homeRow.armed; enterGeneration: homeRow.gen
        }
        HomeModTile {
            objectName: "tile12"; x: 440; y: 0; width: 200; height: 80
            title: "Tile 12"; imageSource: harness.artSource; contextKey: "t12"
            enterIndex: 12; enterArmed: homeRow.armed; enterGeneration: homeRow.gen
        }
        function trigger() {
            ++gen
            armed = true
        }
    }
    function triggerTiles() { homeRow.trigger() }
})QML", QUrl());
            std::unique_ptr<QObject> harness(harnessComp.createWithInitialProperties({{"artSource", artUrl}}));
            if (!harness) qWarning() << harnessComp.errors();
            require(bool(harness), "Motion stagger harness failed to load");
            auto *harnessItem = qobject_cast<QQuickItem *>(harness.get());
            harnessItem->setParentItem(window->contentItem());

            const auto collectCards = [&] {
                QMap<int, QQuickItem *> byIdx;
                for (auto *obj : objects(harness.get())) {
                    if (obj->inherits("MediaCard") || obj->property("anchorIdentity").isValid()) {
                        if (obj->property("enterOpacity").isValid() && obj->property("enterOffsetY").isValid()
                                && obj->property("index").isValid()) {
                            byIdx.insert(obj->property("index").toInt(), qobject_cast<QQuickItem *>(obj));
                        }
                    }
                }
                return byIdx;
            };
            QMap<int, QQuickItem *> gridCards;
            require(motionWaitFor([&] {
                gridCards = collectCards();
                return gridCards.contains(0) && gridCards.contains(8) && gridCards.contains(11) && gridCards.contains(12);
            }), "16-card grid harness did not instantiate cards 0, 8, 11, 12");

            // At t ~ 45ms: card 0 (0ms delay) has started rising/fading,
            // card 8 (200ms delay) is still paused at 0 opacity / 6px offset,
            // and card 12 (>= 12 cap) is already at full opacity / 0px offset!
            motionPump(45);
            const double c0Op = gridCards[0]->property("enterOpacity").toDouble();
            const double c0Y = gridCards[0]->property("enterOffsetY").toDouble();
            const double c8Op = gridCards[8]->property("enterOpacity").toDouble();
            const double c8Y = gridCards[8]->property("enterOffsetY").toDouble();
            const double c12Op = gridCards[12]->property("enterOpacity").toDouble();
            const double c12Y = gridCards[12]->property("enterOffsetY").toDouble();
            require(c0Op > 0.05 && c0Y < 5.9, "Card 0 did not begin stagger animation");
            require(c8Op < 0.01 && qAbs(c8Y - 6.0) < 0.05, "Card 8 (200ms pause) did not stay offset waiting its turn");
            require(c0Op > c8Op + 0.05, "Stagger ordering broken between card 0 and card 8");
            require(qAbs(c12Op - 1.0) < 0.001 && qAbs(c12Y) < 0.01, "Card 12 (>= 12 cap) unexpectedly staggered");
            motionSaveCapture(window, "stagger-grid-mid-" + suffix);

            require(motionWaitFor([&] {
                return qAbs(gridCards[0]->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(gridCards[11]->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(gridCards[11]->property("enterOffsetY").toDouble()) < 0.01;
            }), "Capped grid cards did not settle");
            motionSaveCapture(window, "stagger-grid-settled-" + suffix);

            // Second visit with same stateBank must NOT replay
            QMetaObject::invokeMethod(harness.get(), "reloadGrid");
            QMap<int, QQuickItem *> revisitCards;
            require(motionWaitFor([&] {
                revisitCards = collectCards();
                return revisitCards.contains(0) && revisitCards.contains(8);
            }), "Reloaded grid cards missing");
            require(qAbs(revisitCards[0]->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(revisitCards[8]->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(revisitCards[8]->property("enterOffsetY").toDouble()) < 0.01,
                    "Second visit to same MediaResultView replayed stagger");

            // HomeModTile stagger + untransformed root check + reduceMotion snap
            auto *tile0 = qobject_cast<QQuickItem *>(named(harness.get(), "tile0"));
            auto *tile4 = qobject_cast<QQuickItem *>(named(harness.get(), "tile4"));
            auto *tile12 = qobject_cast<QQuickItem *>(named(harness.get(), "tile12"));
            QMetaObject::invokeMethod(harness.get(), "triggerTiles");
            motionPump(40);
            require(tile0->y() == 0.0 && tile4->y() == 0.0, "HomeModTile root y was moved during enter stagger");
            require(tile0->property("enterOpacity").toDouble() > tile4->property("enterOpacity").toDouble() + 0.05,
                    "HomeModTile stagger did not lead tile0 ahead of tile4");
            require(qAbs(tile4->property("enterOffsetY").toDouble() - 6.0) < 0.05,
                    "HomeModTile tile4 (100ms delay) did not wait paused at 6px");
            require(qAbs(tile12->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(tile12->property("enterOffsetY").toDouble()) < 0.01,
                    "HomeModTile tile12 (>= 12 cap) staggered instead of appearing immediately");

            // Mid-stagger reduceMotion = true must snap immediately
            harness->setProperty("reduce", true);
            QCoreApplication::processEvents();
            require(qAbs(tile4->property("enterOpacity").toDouble() - 1.0) < 0.001
                    && qAbs(tile4->property("enterOffsetY").toDouble()) < 0.01,
                    "Setting reduceMotion true did not snap HomeModTile stagger immediately");

            // Fresh grid visit with reduceMotion = true must start at full opacity / 0px offset
            QQmlComponent rmComp(qmlEngine(window));
            rmComp.setData(R"QML(import QtQuick
import Deadlock.Native
Item {
    id: rmRoot
    required property string artSource
    width: 1000; height: 600
    MediaViewState { id: rmBank }
    ListModel {
        id: rmModel
        Component.onCompleted: {
            for (let i = 0; i < 6; ++i)
                append({ modId: "r-" + i, modTitle: "RM " + i })
        }
    }
    MediaResultView {
        id: rmResults
        anchors.fill: parent
        stateBank: rmBank; model: rmModel; scope: "rmGrid"; domain: "demo"; contextKey: "rm"
        selectedIdentity: ""; focusKey: ""
        presentationMode: "grid"; ready: true
        indexForIdentity: id => { for (let i = 0; i < rmModel.count; ++i) if (rmModel.get(i).modId === id) return i; return -1 }
        identityAt: idx => idx >= 0 && idx < rmModel.count ? rmModel.get(idx).modId : ""
        snapshotFor: id => ({ identity: id, source: rmRoot.artSource, hidden: false })
        delegateComponent: Component {
            MediaCard {
                required property string modId
                required property string modTitle
                resultHost: rmResults
                ownerSnapshot: ({ identity: modId, source: rmRoot.artSource, hidden: false })
                anchorIdentity: modId; heading: modTitle; actionKind: "none"
            }
        }
    }
})QML", QUrl());
            std::unique_ptr<QObject> rmObj(rmComp.createWithInitialProperties({{"artSource", artUrl}}));
            require(bool(rmObj), "Reduce-motion grid fixture failed");
            qobject_cast<QQuickItem *>(rmObj.get())->setParentItem(window->contentItem());
            QList<QQuickItem *> rmCards;
            require(motionWaitFor([&] {
                rmCards.clear();
                for (auto *obj : objects(rmObj.get()))
                    if (obj->property("enterOpacity").isValid() && obj->property("enterOffsetY").isValid())
                        rmCards.append(qobject_cast<QQuickItem *>(obj));
                return rmCards.size() >= 6;
            }), "Reduce-motion cards did not appear");
            for (auto *c : rmCards) {
                require(qAbs(c->property("enterOpacity").toDouble() - 1.0) < 0.001
                        && qAbs(c->property("enterOffsetY").toDouble()) < 0.01,
                        "reduceMotion=true did not disable enter stagger");
            }
            prefs->setProperty("reduceMotion", false);

            std::puts("PASS UI motion enter stagger");
            QCoreApplication::exit(0);
        } catch (const std::exception &e) {
            std::fprintf(stderr, "FAIL UI motion enter stagger: %s\n", e.what());
            QCoreApplication::exit(1);
        }
    });
}

static void startMotionToggleFlashCheck(QQuickWindow *window, LibraryModel *model) {
    QTimer::singleShot(0, window, [window, model] {
        try {
            const bool dark = qEnvironmentVariable("MOTION_THEME") != "light";
            const QString suffix = dark ? "dark" : "light";
            motionApplyPalette(dark);
            window->resize(1040, 820);
            motionPump(30);

            QTemporaryDir files;
            require(files.isValid(), "Toggle flash temp dir failed");
            QImage vivid(320, 240, QImage::Format_RGB32);
            for (int y = 0; y < vivid.height(); ++y) {
                for (int x = 0; x < vivid.width(); ++x) {
                    const QColor band = (x < 106) ? QColor("#e04472")
                                      : (x < 212) ? QColor("#2fb8c9")
                                                  : QColor("#e5b53d");
                    vivid.setPixelColor(x, y, band);
                }
            }
            require(vivid.save(files.filePath("vivid.png")), "Vivid artwork fixture failed");
            const QString artUrl = QUrl::fromLocalFile(files.filePath("vivid.png")).toString();

            require(motionWaitFor([&] { return !model->loading() && model->count() >= 6; }),
                    "Demo library did not finish loading");
            auto *prefs = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
            auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
            auto *themeObj = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Theme");
            require(prefs && tokens && themeObj, "Singletons missing");
            prefs->setProperty("reduceMotion", false);
            motionPump(20);
            require(tokens->property("dark").toBool() == dark, "Palette theme mismatch");

            // 1) Production Library card: click real switch OFF then ON, verifying brass flash ring & settle.
            realClick(window, "libraryTab");
            QQuickItem *targetRow = nullptr;
            QQuickItem *targetSwitch = nullptr;
            require(motionWaitFor([&] {
                for (auto *obj : objects(window)) {
                    if (obj->objectName() == "modRow") {
                        auto *item = qobject_cast<QQuickItem *>(obj);
                        if (!item || !item->isVisible() || !item->property("interactable").toBool()
                                || item->property("switchedOff").toBool()
                                || item->property("enterOpacity").toDouble() < 0.99)
                            continue;
                        if (auto *sw = qobject_cast<QQuickItem *>(findNamed(item, "modSwitch"));
                                sw && sw->isVisible() && sw->isEnabled() && sw->width() >= 20 && sw->height() >= 20) {
                            targetRow = item;
                            targetSwitch = sw;
                            return true;
                        }
                    }
                }
                return false;
            }), "No settled enabled Library card with modSwitch found");
            motionPump(20);

            auto *liveRing = qobject_cast<QQuickItem *>(named(targetRow, "toggleFlashRing"));
            auto *liveAnim = named(targetRow, "toggleFlashAnimation");
            require(qAbs(liveRing->opacity()) < 0.001, "Initial toggleFlashRing was not hidden");

            realClickItem(window, targetSwitch);
            require(motionWaitFor([&] { return targetRow->property("switchedOff").toBool(); }),
                    "Library mod did not turn off via switch click");
            motionPump(70);
            require(!liveAnim->property("running").toBool() && liveRing->opacity() < 0.001,
                    "Brass flash ring wrongly fired when turning mod OFF");

            realClickItem(window, targetSwitch);
            require(motionWaitFor([&] { return !targetRow->property("switchedOff").toBool(); }),
                    "Library mod did not turn back on via switch click");
            bool sawLiveFlash = false;
            require(motionWaitFor([&] {
                if (liveRing->opacity() > 0.35 && liveRing->isVisible()) {
                    sawLiveFlash = true;
                    return true;
                }
                return false;
            }, 600), "Brass flash ring did not flash when turning Library mod ON");
            require(sawLiveFlash, "Live toggle flash ring never peaked");
            motionSaveCapture(window, "toggle-flash-list-mid-" + suffix);

            require(motionWaitFor([&] {
                return !liveAnim->property("running").toBool() && liveRing->opacity() < 0.001 && !liveRing->isVisible();
            }, 800), "Brass flash ring did not settle back to invisible");

            // 2) Controlled vivid poster cards + delegate recycling + reduceMotion checks.
            QQmlComponent harnessComp(qmlEngine(window));
            harnessComp.setData(R"QML(import QtQuick
import QtQuick.Controls
import Deadlock.Native
import DeadlockStyle

Rectangle {
    id: harness
    required property string artSource
    anchors.fill: parent
    z: 200000
    color: Theme.window
    property bool cardAOn: true
    property bool cardBOn: false
    property bool reduce: false
    onReduceChanged: Preferences.reduceMotion = reduce
    Component.onCompleted: Preferences.reduceMotion = false

    MediaViewState { id: bank }
    ListModel {
        id: singleModel
        ListElement { modId: "slot-off"; modTitle: "Slot Off"; modOn: false }
    }
    MediaResultView {
        id: hostA
        x: 32; y: 32; width: 260; height: 340
        stateBank: bank; model: singleModel; scope: "toggleA"; domain: "demo"; contextKey: "ta"
        selectedIdentity: ""; focusKey: ""; presentationMode: "grid"; ready: true
        indexForIdentity: id => 0
        identityAt: idx => "a"
        snapshotFor: id => ({ identity: id, source: harness.artSource, hidden: false })
        delegateComponent: Component {
            MediaCard {
                objectName: "cardA"
                resultHost: hostA
                ownerSnapshot: ({ identity: "a", source: harness.artSource, hidden: false })
                anchorIdentity: "a"
                heading: "Turned Off Mod"
                subtitle: "Artwork fades to grey"
                actionKind: "switch"
                actionChecked: harness.cardAOn
                actionEnabled: true
            }
        }
    }
    MediaResultView {
        id: hostB
        x: 324; y: 32; width: 260; height: 340
        stateBank: bank; model: singleModel; scope: "toggleB"; domain: "demo"; contextKey: "tb"
        selectedIdentity: ""; focusKey: ""; presentationMode: "grid"; ready: true
        indexForIdentity: id => 0
        identityAt: idx => "b"
        snapshotFor: id => ({ identity: id, source: harness.artSource, hidden: false })
        delegateComponent: Component {
            MediaCard {
                objectName: "cardB"
                resultHost: hostB
                ownerSnapshot: ({ identity: "b", source: harness.artSource, hidden: false })
                anchorIdentity: "b"
                heading: "Turned On Mod"
                subtitle: "Brass ring flash"
                actionKind: "switch"
                actionChecked: harness.cardBOn
                actionEnabled: true
            }
        }
    }
    // Delegate identity change simulation: switches from slot-off (OFF) to slot-on (ON)
    MediaResultView {
        id: hostRecycle
        x: 616; y: 32; width: 260; height: 340
        stateBank: bank; model: singleModel; scope: "toggleRecycle"; domain: "demo"; contextKey: "tr"
        selectedIdentity: ""; focusKey: ""; presentationMode: "grid"; ready: true
        indexForIdentity: id => 0
        identityAt: idx => singleModel.count > 0 ? singleModel.get(0).modId : ""
        snapshotFor: id => ({ identity: id, source: harness.artSource, hidden: false })
        delegateComponent: Component {
            MediaCard {
                required property string modId
                required property string modTitle
                required property bool modOn
                objectName: "recycleCard"
                resultHost: hostRecycle
                ownerSnapshot: ({ identity: modId, source: harness.artSource, hidden: false })
                anchorIdentity: modId
                heading: modTitle
                actionKind: "switch"
                actionChecked: modOn
                actionEnabled: true
            }
        }
    }
    function recycleOffToOn() {
        singleModel.set(0, { modId: "slot-on", modTitle: "Different Mod", modOn: true })
    }
})QML", QUrl());
            std::unique_ptr<QObject> harness(harnessComp.createWithInitialProperties({{"artSource", artUrl}}));
            if (!harness) qWarning() << harnessComp.errors();
            require(bool(harness), "Toggle flash harness failed to load");
            qobject_cast<QQuickItem *>(harness.get())->setParentItem(window->contentItem());

            QQuickItem *cardA = nullptr;
            QQuickItem *cardB = nullptr;
            QQuickItem *recycleCard = nullptr;
            require(motionWaitFor([&] {
                cardA = qobject_cast<QQuickItem *>(findNamed(harness.get(), "cardA"));
                cardB = qobject_cast<QQuickItem *>(findNamed(harness.get(), "cardB"));
                recycleCard = qobject_cast<QQuickItem *>(findNamed(harness.get(), "recycleCard"));
                if (!cardA || !cardB || !recycleCard) return false;
                auto *pA = findNamed(cardA, "rowPreview");
                auto *pB = findNamed(cardB, "rowPreview");
                return pA && pB && pA->property("ready").toBool() && pB->property("ready").toBool();
            }), "Controlled toggle cards or artwork never became ready");

            auto *prevA = named(cardA, "rowPreview");
            auto *imgA = qobject_cast<QQuickItem *>(named(cardA, "previewImage"));
            auto *ringA = qobject_cast<QQuickItem *>(named(cardA, "toggleFlashRing"));
            auto *ringB = qobject_cast<QQuickItem *>(named(cardB, "toggleFlashRing"));
            auto *animB = named(cardB, "toggleFlashAnimation");
            auto *recycleRing = qobject_cast<QQuickItem *>(named(recycleCard, "toggleFlashRing"));
            auto *recycleAnim = named(recycleCard, "toggleFlashAnimation");

            const QColor brass = themeObj->property("brass").value<QColor>();
            const auto ringBorder = QQmlProperty(ringB, "border.color").read().value<QColor>();
            require(ringBorder == brass, "toggleFlashRing color is not Theme.brass");

            require(qAbs(prevA->property("desaturateProgress").toDouble()) < 0.001, "Card A started desaturated");
            require(qAbs(imgA->property("_displayOpacity").toDouble() - 1.0) < 0.001, "Card A started dimmed");

            // Turn Card A OFF (fade to grey over Tokens.normal) and Card B ON (brass flash ring)
            harness->setProperty("cardAOn", false);
            harness->setProperty("cardBOn", true);
            motionPump(85);

            const double midDesat = prevA->property("desaturateProgress").toDouble();
            const double midOp = imgA->property("_displayOpacity").toDouble();
            const double ringBOp = ringB->opacity();
            require(midDesat > 0.05 && midDesat < 0.95, "Card A desaturateProgress did not animate smoothly over Tokens.normal");
            require(midOp > 0.58 && midOp < 0.96, "Card A previewImage opacity did not fade smoothly over Tokens.normal");
            require(ringA->opacity() < 0.001, "Card A flashed brass ring when turning OFF");
            require(ringBOp > 0.35 && ringB->isVisible(), "Card B brass flash ring not visible mid-flash");
            motionSaveCapture(window, "toggle-flash-poster-mid-" + suffix);

            require(motionWaitFor([&] {
                return qAbs(prevA->property("desaturateProgress").toDouble() - 1.0) < 0.001
                    && qAbs(imgA->property("_displayOpacity").toDouble() - 0.55) < 0.01
                    && !animB->property("running").toBool()
                    && ringB->opacity() < 0.001;
            }, 800), "Toggle transitions did not settle to final state");
            motionSaveCapture(window, "toggle-flash-settled-" + suffix);

            // Delegate identity change (different modId off -> on) must NOT fire brass flash ring
            QMetaObject::invokeMethod(harness.get(), "recycleOffToOn");
            motionPump(100);
            require(!recycleAnim->property("running").toBool() && recycleRing->opacity() < 0.001,
                    "Delegate reuse across different mod identities falsely triggered brass flash ring");

            // reduceMotion = true disables toggle flash and fade animation completely
            harness->setProperty("reduce", true);
            motionPump(20);
            harness->setProperty("cardAOn", true);
            motionPump(20);
            require(ringA->opacity() < 0.001 && !named(cardA, "toggleFlashAnimation")->property("running").toBool(),
                    "reduceMotion=true still animated brass toggle flash ring");
            require(qAbs(prevA->property("desaturateProgress").toDouble()) < 0.001
                    && qAbs(imgA->property("_displayOpacity").toDouble() - 1.0) < 0.001,
                    "reduceMotion=true did not immediately restore desaturated artwork");

            // Mid-flight reduceMotion = true must interrupt brass ring AND snap off-fade immediately
            harness->setProperty("reduce", false);
            motionPump(20);
            harness->setProperty("cardBOn", false);
            require(motionWaitFor([&] {
                return qAbs(named(cardB, "rowPreview")->property("desaturateProgress").toDouble() - 1.0) < 0.001;
            }, 600), "Card B did not finish turning off for mid-flight check");

            harness->setProperty("cardAOn", false);
            harness->setProperty("cardBOn", true);
            motionPump(75);
            require(prevA->property("desaturateProgress").toDouble() < 0.95, "Card A already finished before mid-flight snap");
            require(ringB->opacity() > 0.25, "Card B ring not active before mid-flight snap");

            harness->setProperty("reduce", true);
            QCoreApplication::processEvents();
            require(qAbs(prevA->property("desaturateProgress").toDouble() - 1.0) < 0.001
                    && qAbs(imgA->property("_displayOpacity").toDouble() - 0.55) < 0.01,
                    "Setting reduceMotion=true did not snap fade-to-grey mid-flight");
            require(!animB->property("running").toBool() && ringB->opacity() < 0.001,
                    "Setting reduceMotion=true did not stop brass flash ring mid-flight");

            prefs->setProperty("reduceMotion", false);
            std::puts("PASS UI motion toggle flash");
            QCoreApplication::exit(0);
        } catch (const std::exception &e) {
            std::fprintf(stderr, "FAIL UI motion toggle flash: %s\n", e.what());
            QCoreApplication::exit(1);
        }
    });
}

static void startMotionPlayIgnitionCheck(QQuickWindow *window, LibraryModel *model) {
    QTimer::singleShot(0, window, [window, model] {
        try {
            const bool dark = qEnvironmentVariable("MOTION_THEME") != "light";
            const QString suffix = dark ? "dark" : "light";
            motionApplyPalette(dark);
            window->resize(1040, 760);
            motionPump(30);

            require(motionWaitFor([&] {
                return model->nativeMode() && !model->loading() && model->launch()->canLaunch();
            }, 4000), "Native launch readiness did not settle");

            auto *prefs = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
            auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
            require(prefs && tokens, "Singletons missing");
            prefs->setProperty("reduceMotion", false);
            motionPump(20);
            require(tokens->property("dark").toBool() == dark, "Palette theme mismatch");

            auto *playBtn = qobject_cast<QQuickItem *>(named(window, "launchActiveButton"));
            auto *playBg = qvariant_cast<QQuickItem *>(playBtn->property("background"));
            auto *ignitionBar = qobject_cast<QQuickItem *>(named(playBg, "playIgnitionBar"));
            auto *ignitionAnim = named(playBg, "playIgnitionAnimation");
            auto *playGlyph = qobject_cast<QQuickItem *>(named(playBtn, "playGlyphIcon"));
            auto *sunburstGlyph = qobject_cast<QQuickItem *>(named(playBtn, "playSunburstIcon"));
            auto *glyphAnim = named(playBtn, "playIgnitionGlyphAnimation");
            auto *sheenAnim = named(playBg, "playSheenAnimation");

            require(!sheenAnim->property("running").toBool(), "Pending sheen should be off when pendingChanges=false");
            require(!ignitionAnim->property("running").toBool() && !glyphAnim->property("running").toBool(),
                    "Ignition animations running before Play clicked");
            require(sunburstGlyph->opacity() < 0.001 && qAbs(playGlyph->opacity() - 1.0) < 0.001,
                    "Play glyph not at rest before launch");

            // 1) Click Play to start launch -> verify one-shot plaque shimmer + sunburst spin
            realClick(window, "launchActiveButton");
            require(model->launch()->phase() == "reviewing", "Clicking Play did not start launch review");

            bool sawSunburst = false;
            bool sawShimmer = false;
            double maxSunOp = 0.0;
            double maxBarOp = 0.0;
            double maxBarX = -1000.0;
            QElapsedTimer t;
            t.start();
            while (t.elapsed() < 350) {
                motionPump(20);
                const double sunOp = sunburstGlyph->opacity();
                const double barOp = ignitionBar->opacity();
                const double barX = ignitionBar->x();
                maxSunOp = std::max(maxSunOp, sunOp);
                maxBarOp = std::max(maxBarOp, barOp);
                maxBarX = std::max(maxBarX, barX);
                if (sunOp > 0.7 && qAbs(sunburstGlyph->rotation()) < 90.0) sawSunburst = true;
                if (barOp > 0.5 && barX > -ignitionBar->width() + 5.0) sawShimmer = true;
                if (sawSunburst && sawShimmer) {
                    motionSaveCapture(window, "play-ignition-mid-" + suffix);
                    break;
                }
            }
            require(sawSunburst, "Play glyph did not spin into visible sunburst on launch start");
            require(sawShimmer, "Play plaque shimmer did not sweep across plaque on launch start");

            // 2) Wait for one-shot ignition to finish -> zero lasting indicator
            require(motionWaitFor([&] {
                return !ignitionAnim->property("running").toBool()
                    && !glyphAnim->property("running").toBool();
            }, 1500), "Play ignition remained running (must be one-shot with no lasting indicator)");
            require(sunburstGlyph->opacity() < 0.001, "Sunburst remained visible after ignition finished");
            require(qAbs(playGlyph->opacity() - 1.0) < 0.001 && qAbs(playGlyph->rotation()) < 0.01,
                    "Play glyph did not return cleanly to resting icon");
            require(ignitionBar->opacity() < 0.001 && ignitionBar->x() <= -ignitionBar->width() + 0.5,
                    "Play ignition shimmer bar did not reset cleanly off left edge");
            motionSaveCapture(window, "play-ignition-settled-" + suffix);

            // 3) Cancel attempt back to idle and verify reduceMotion=true disables ignition completely
            require(model->launch()->cancel(), "Could not cancel first launch attempt");
            require(motionWaitFor([&] { return model->launch()->canLaunch(); }, 4000),
                    "Launch did not return to idle/canLaunch after cancel");

            prefs->setProperty("reduceMotion", true);
            motionPump(20);
            realClick(window, "launchActiveButton");
            require(model->launch()->phase() == "reviewing", "Second Play click did not start launch review");
            motionPump(60);
            require(!ignitionAnim->property("running").toBool() && !glyphAnim->property("running").toBool(),
                    "Ignition animated even with reduceMotion=true");
            require(sunburstGlyph->opacity() < 0.001 && ignitionBar->opacity() < 0.001
                    && qAbs(playGlyph->opacity() - 1.0) < 0.001,
                    "Ignition visual state changed under reduceMotion=true");

            // 4) Mid-flight reduceMotion=true immediately stops and resets both ignition animations
            require(model->launch()->cancel(), "Could not cancel second launch attempt");
            require(motionWaitFor([&] { return model->launch()->canLaunch(); }, 4000),
                    "Launch did not return to idle/canLaunch for mid-flight check");

            prefs->setProperty("reduceMotion", false);
            motionPump(20);
            realClick(window, "launchActiveButton");
            require(motionWaitFor([&] {
                return sunburstGlyph->opacity() > 0.5 && ignitionBar->opacity() > 0.4;
            }, 400), "Third launch did not enter active ignition before mid-flight stop");

            prefs->setProperty("reduceMotion", true);
            QCoreApplication::processEvents();
            require(!ignitionAnim->property("running").toBool() && !glyphAnim->property("running").toBool(),
                    "Setting reduceMotion=true did not stop Play ignition animations mid-flight");
            require(sunburstGlyph->opacity() < 0.001 && ignitionBar->opacity() < 0.001
                    && qAbs(playGlyph->opacity() - 1.0) < 0.001 && qAbs(playGlyph->rotation()) < 0.01,
                    "Setting reduceMotion=true did not reset Play ignition glyphs/bar mid-flight");

            prefs->setProperty("reduceMotion", false);
            std::puts("PASS UI motion play ignition");
            QCoreApplication::exit(0);
        } catch (const std::exception &e) {
            std::fprintf(stderr, "FAIL UI motion play ignition: %s\n", e.what());
            QCoreApplication::exit(1);
        }
    });
}

} // namespace
