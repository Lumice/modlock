#include <cmath>
#include <QDesktopServices>
#include <QDir>
#include <QDirIterator>
#include "apppaths.h"
#include "appimagepalette.h"
#include <QPalette>
#include <QRegularExpression>
#include <QSettings>
#include <QStandardPaths>
#include "catalogdata.h"
#include "catalogcontroller.h"
#include <QQmlComponent>
#include <QQmlEngine>
#include <QQmlContext>
#include <QQmlProperty>
#include <QJsonDocument>
#include <QJsonArray>
#include <QTextDocument>
#include "uicheck.h"
#include "crosshaircapture.h"
#include "downloadcapture.h"
#include "progressaccessibility.h"
#include "librarymodel.h"
#include "fontcontroller.h"
#include "optioncontroller.h"
#include "gameconfigcontroller.h"
#include "launchhealth.h"
#include "remotecontroller.h"
#include "storagecontroller.h"
#include <QCoreApplication>
#include <QCryptographicHash>
#include <QAccessible>
#include <QGuiApplication>
#include <QDateTime>
#include <QElapsedTimer>
#include <QEventLoop>
#include <QFileInfo>
#include <QFile>
#include <QImage>
#include <QMouseEvent>
#include <QKeyEvent>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QMimeData>
#include <QLocale>
#include <QPointer>
#include <QQuickItem>
#include <QQuickWindow>
#include <QSet>
#include <QTimer>
#include <QUrl>
#include <cstdio>
#include <memory>
#include <stdexcept>

namespace {
void require(bool result, const char *message) {
    if (!result) throw std::runtime_error(message);
}
QMap<QString, QByteArray> profileBoundarySnapshot() {
    QMap<QString, QByteArray> tree;
    for (const auto &key : {"XDG_DATA_HOME", "XDG_CONFIG_HOME"}) {
        const auto root = qEnvironmentVariable(key);
        require(!root.isEmpty(), "Boundary check needs private XDG roots");
        QDirIterator it(root, QDir::AllEntries | QDir::Hidden | QDir::NoDotAndDotDot, QDirIterator::Subdirectories);
        while (it.hasNext()) {
            const auto path = it.next();
            QFile file(path);
            if (it.fileInfo().isDir()) tree.insert(path + "/", {});
            else { require(file.open(QIODevice::ReadOnly), "Cannot snapshot private XDG file"); tree.insert(path, file.readAll()); }
        }
    }
    return tree;
}
QList<QObject *> objects(QObject *root) {
    QList<QObject *> found, pending{root};
    QSet<QObject *> seen;
    while (!pending.isEmpty()) {
        auto *object = pending.takeLast();
        if (seen.contains(object)) continue;
        seen.insert(object); found.append(object); pending.append(object->children());
        if (auto *item = qobject_cast<QQuickItem *>(object))
            for (auto *child : item->childItems()) pending.append(child);
    }
    return found;
}
QObject *findNamed(QObject *root, const char *name) {
    for (auto *object : objects(root)) if (object->objectName() == QLatin1String(name)) return object;
    return nullptr;
}
QObject *named(QObject *root, const char *name) {
    if (auto *object = findNamed(root, name)) return object;
    throw std::runtime_error(std::string("UI object missing: ") + name);
}
QObject *localReviewPopup(QObject *root) {
    for(auto *object : objects(root))
        if(object->inherits("QQuickPopup") && object->property("title").toString()=="Add to library") return object;
    return nullptr;
}
QObject *profilePopup(QObject *root) {
    for(auto *object : objects(root))
        if(object->inherits("QQuickPopup") && object->property("title").toString()=="Profiles") return object;
    return nullptr;
}
void seedGoodLaunch(LibraryModel *model, const QString &mode) {
    auto *health=model->launch()->health();
    health->capture("fixture-baseline",mode);
    const auto key=QCryptographicHash::hash(model->dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
    QFile file(QDir(AppPaths::stateRoot()).filePath("launch-health/"+key+".json"));
    require(file.open(QIODevice::ReadOnly),"cannot read crash baseline fixture");
    auto state=QJsonDocument::fromJson(file.readAll()).object();file.close();
    state["good"]=state["launch"];
    require(file.open(QIODevice::WriteOnly|QIODevice::Truncate),"cannot write crash baseline fixture");
    require(file.write(QJsonDocument(state).toJson())>0,"cannot save crash baseline fixture");file.close();
    health->load(model->dataDir());
}
void click(QObject *root, const char *name);
void submitProfile(QObject *root,const QString &mode,const QString &name) {
    auto *popup=profilePopup(root);
    require(popup,"profile editor missing");
    require(QMetaObject::invokeMethod(popup,"selectMode",Q_ARG(QVariant,QVariant(mode))),"profile mode selection failed");
    named(root,"profileNameField")->setProperty("text",name);
    click(root,mode=="duplicate" ? "copyProfileButton" : mode=="rename" ? "renameProfileButton" : "createProfileButton");
    require(QMetaObject::invokeMethod(popup,"close"),"profile editor did not close");
}
void plain(QObject *root, const char *name, const QString &text = {}) {
    auto *object = named(root, name);
    const auto format = object->property("textFormat");
    if (!format.isValid() || format.toInt() != Qt::PlainText)
        throw std::runtime_error(std::string("metadata is not PlainText: ") + name);
    if (!text.isEmpty()) require(object->property("text").toString() == text, "metadata was interpreted or changed");
}
void click(QObject *root, const char *name) {
    auto *control = named(root, name);
    if (!control->property("enabled").toBool()) throw std::runtime_error(std::string("Control is disabled: ") + name);
    // Menu items live in closed menus; the menu opens on demand, so only
    // require visibility for ordinary controls.
    if (!control->inherits("QQuickMenuItem") && !control->property("visible").toBool())
        throw std::runtime_error(std::string("Control is invisible: ") + name);
    require(QMetaObject::invokeMethod(control, "clicked"), "control could not be activated");
}
void realClick(QQuickWindow *window,const char *name) {
    auto *item=qobject_cast<QQuickItem*>(named(window,name));
    require(item && item->isEnabled() && item->isVisible(),"real click requires an enabled visible Item");
    const auto p=item->mapToScene(QPointF(item->width()/2,item->height()/2));
    QMouseEvent press(QEvent::MouseButtonPress,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::LeftButton,Qt::NoModifier);
    QMouseEvent release(QEvent::MouseButtonRelease,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::NoButton,Qt::NoModifier);
    QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
}
void realClickItem(QQuickWindow *window,QQuickItem *item,QPointF local={}) {
    require(item && item->isEnabled() && item->isVisible(),"real click requires an enabled visible Item");
    const auto p=item->mapToScene(local.isNull() ? QPointF(item->width()/2,item->height()/2) : local);
    QMouseEvent press(QEvent::MouseButtonPress,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::LeftButton,Qt::NoModifier);
    QMouseEvent release(QEvent::MouseButtonRelease,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::NoButton,Qt::NoModifier);
    QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
}
void openProfilesViaChooser(QQuickWindow *window) {
    realClick(window, "profileChooser");
    auto *popup = named(window, "profileChooser")->property("popup").value<QObject *>();
    QElapsedTimer wait; wait.start();
    while (wait.elapsed() < 1500) {
        auto *manage = qobject_cast<QQuickItem *>(findNamed(window, "manageProfilesMenuItem"));
        if (popup && popup->property("visible").toBool() && manage && manage->isVisible() && manage->width() > 0 && manage->height() > 0)
            break;
        QCoreApplication::processEvents(QEventLoop::AllEvents, 15);
    }
    auto *manage = qobject_cast<QQuickItem *>(named(window, "manageProfilesMenuItem"));
    require(popup && popup->property("visible").toBool(), "profileChooser popup did not open");
    require(manage && manage->isVisible() && manage->isEnabled(), "manageProfilesMenuItem unavailable");
    realClickItem(window, manage);
}
void realKey(QQuickWindow *window,int key,Qt::KeyboardModifiers modifiers=Qt::NoModifier,const QString &text={}) {
    QKeyEvent press(QEvent::KeyPress,key,modifiers,text),release(QEvent::KeyRelease,key,modifiers,text);
    QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
}
void chooseCategory(QQuickWindow *window, int index) {
    realClick(window, "categoryChooser");
    realKey(window, Qt::Key_Home);
    for (int i = 0; i < index; ++i) realKey(window, Qt::Key_Down);
    realKey(window, Qt::Key_Return);
}
void realText(QQuickWindow *window,const char *name,const QString &text) {
    realClick(window,name); realKey(window,Qt::Key_A,Qt::ControlModifier); realKey(window,Qt::Key_Backspace); realKey(window,0,Qt::NoModifier,text);
}
void realDrop(QQuickWindow *window,const QUrl &url) {
    QMimeData mime; mime.setUrls({url});
    QDragEnterEvent enter(QPointF(450,300),Qt::CopyAction,&mime,Qt::LeftButton,Qt::NoModifier);
    QCoreApplication::sendEvent(window,&enter); require(enter.isAccepted(),"local DropArea rejected real drag entry");
    QDropEvent drop(QPointF(450,300),Qt::CopyAction,&mime,Qt::LeftButton,Qt::NoModifier);
    QCoreApplication::sendEvent(window,&drop); require(drop.isAccepted(),"local DropArea rejected real local URL drop");
}
void workflowControlsDisabled(QObject *root) {
    for (const auto *name : {"importButton", "manageProfilesMenuItem", "applyButton", "restoreButton"})
        require(!named(root, name)->property("enabled").toBool(), "workflow control enabled during loading/busy");
}
void workflowControlsReady(QObject *root, bool native, bool restore) {
    require(named(root, "importButton")->property("enabled").toBool() == !native, "live Import enabled state is stale");
    require(named(root, "manageProfilesMenuItem")->property("enabled").toBool() == native, "live Profiles enabled state is stale");
    require(named(root, "applyButton")->property("enabled").toBool() == native, "live Apply enabled state is stale");
    require(named(root, "restoreButton")->property("enabled").toBool() == restore, "live Restore enabled state is stale");
}
bool imageReady(QObject *preview) {
    auto *image = named(preview, "previewImage");
    require(image->property("status").toInt() != 3, "preview image failed to load");
    return image->property("status").toInt() == 1;
}
bool colorMatches(QQuickWindow *window, QObject *preview, const QColor &expected) {
    auto *item = qobject_cast<QQuickItem *>(named(preview, "previewImage"));
    if (!item || !item->isVisible()) return false;
    const auto pixels = window->grabWindow();
    const auto center = item->mapToScene(QPointF(item->width() / 2, item->height() / 2));
    const auto actual = pixels.pixelColor(qRound(center.x() * pixels.width() / window->width()),
                                         qRound(center.y() * pixels.height() / window->height()));
    return qAbs(actual.red() - expected.red()) < 8 && qAbs(actual.green() - expected.green()) < 8 && qAbs(actual.blue() - expected.blue()) < 8;
}
void renderedColor(QQuickWindow *window, QObject *preview, const QColor &expected) {
    auto *item = qobject_cast<QQuickItem *>(named(preview, "previewImage"));
    require(item && item->isVisible(), "loaded preview is not visible");
    const auto pixels = window->grabWindow();
    require(!pixels.isNull(), "preview frame could not be captured");
    const auto center = item->mapToScene(QPointF(item->width() / 2, item->height() / 2));
    const auto actual = pixels.pixelColor(qRound(center.x() * pixels.width() / window->width()),
                                         qRound(center.y() * pixels.height() / window->height()));
    require(qAbs(actual.red() - expected.red()) < 8 && qAbs(actual.green() - expected.green()) < 8
            && qAbs(actual.blue() - expected.blue()) < 8, "ready preview did not render its image pixels");
}
QVariantMap previewMod(const QString &id, bool hidden) {
    return {{"id", id}, {"title", "Synthetic artwork"}, {"enabled", false},
            {"images", QStringList{qEnvironmentVariable("UI_PREVIEW_FIRST"), qEnvironmentVariable("UI_PREVIEW_SECOND")}},
            {"previewHidden", hidden}, {"installedVpks", QStringList{}}, {"missingVpks", QStringList{}}};
}
void capture(QQuickWindow *window, const QString &name) {
    const auto directory = qEnvironmentVariable("UI_NATIVE_CAPTURES");
    if (!directory.isEmpty()) require(window->grabWindow().save(directory + "/" + name + ".png"), "capture failed");
}
void toggleIncluded(QQuickWindow *window, bool included) {
    auto *details = named(window, "modDetails"); auto *control = named(details, "modSwitch");
    require(control->property("enabled").toBool(), "native inclusion switch disabled");
    require(control->property("visible").toBool(), "native inclusion switch invisible");
    control->setProperty("checked", included);
    require(QMetaObject::invokeMethod(control, "toggled"), "native switch could not toggle");
}
void reviewCounts(QQuickWindow *window, const QVariantMap &review, bool importing) {
    const auto expected = qEnvironmentVariable(importing ? "UI_NATIVE_IMPORT_COUNTS" : "UI_NATIVE_APPLY_COUNTS").split(',');
    if (expected.size() != 5) return;
    const QStringList keys{"packageCount", "directoryCount", "physicalFileCount", "companionFileCount", "totalBytes"};
    for (int i = 0; i < keys.size(); ++i)
        require(review.contains(keys[i]) && review.value(keys[i]).toLongLong() == expected[i].toLongLong(), qPrintable("Wrong review count: " + keys[i]));
    const auto summary = named(window, "reviewSummary")->property("text").toString();
    const auto expectedSummary = qEnvironmentVariable(importing ? "UI_NATIVE_IMPORT_SUMMARY" : "UI_NATIVE_APPLY_SUMMARY");
    if (importing && (expectedSummary.isEmpty() || !summary.contains(expectedSummary)))
        throw std::runtime_error(("explicit family counts not displayed with correct plurals: " + summary).toStdString());
    if (importing) require(summary.contains(QLocale().toString(expected[4].toLongLong())), "actual copy bytes not displayed");
    else require(summary.contains(" mod") && summary.contains("to copy") && !summary.contains("VPK"), "player Apply summary missing");
    plain(window, "reviewSummary"); plain(window, "reviewReasons"); plain(window, "reviewPaths");
}
void common(QQuickWindow *window, LibraryModel *model) {
    require(model->readOnly() && !model->demoMode(), "real mode was replaced with demo");
    require(!named(window, "sourcePathLabel")->property("text").toString().isEmpty(), "source state not visible");
    int switches = 0;
    for (auto *object : objects(window)) {
        if (object->objectName() != "modSwitch") continue;
        ++switches;
        require(!object->property("enabled").toBool(), "real enabled switch is interactive");
    }
    require(switches > 0, "read-only switch not found");
    const auto id = model->selectedId();
    const auto previous = model->selectedMod().value("enabled").toBool();
    require(!model->setModEnabled(id, !previous), "real-mode mutation was accepted");
    require(model->selectedMod().value("enabled").toBool() == previous, "real-mode mutation changed state");
}
}

#include "mediademandcheck.h"
#include "viewersessioncheck.h"
#include "postercontrolscheck.h"
#include "spotlightcheck.h"
#include "browseaudiocheck.h"
#include "decochromecheck.h"
#include "profilemenucheck.h"
#include "motion08check.h"

void startUiCheck(QQuickWindow *window, LibraryModel *model, const QString &scenario) {
    if (scenario == "motion-enter-stagger") { startMotionEnterStaggerCheck(window, model); return; }
    if (scenario == "motion-toggle-flash") { startMotionToggleFlashCheck(window, model); return; }
    if (scenario == "motion-play-ignition") { startMotionPlayIgnitionCheck(window, model); return; }
    if (scenario.startsWith("deco-chrome-")) { startDecoChromeCheck(window, model, scenario); return; }
    if (scenario == "profile-menu") { startProfileMenuCheck(window, model); return; }
    if (scenario == "poster-controls") { startPosterControlsCheck(window); return; }
    if (scenario == "spotlight") { startSpotlightCheck(window); return; }
    if (scenario == "browse-audio") { startBrowseAudioCheck(window, model); return; }
    if (qEnvironmentVariableIsSet("UI_DETAILS_CAPTURE")) model->setAutomaticApply(false);
    if (scenario == "viewer-session" || scenario == "viewer-data-change") { startViewerSessionCheck(window, scenario == "viewer-data-change"); return; }
    if (scenario == "media-demand") { startMediaDemandCheck(window); return; }
    if (scenario == "media-hidden-reuse" || scenario == "media-reveal-reuse") {
        struct GuardState {
            int ticks = 0, stage = 0, mode = 0, trip = 0, reuses = 0, samples = 0, hiddenToVisible = 0;
            QString revealedId;
            bool revealDropped = false, gridSized = false;
            QElapsedTimer step;
            QList<QPair<QPointer<QObject>, QString>> owners;
        };
        const auto state = std::make_shared<GuardState>();
        auto *timer = new QTimer(window); timer->setInterval(25);
        window->resize(1200, 780);
        QObject::connect(timer, &QTimer::timeout, window, [=] {
            try {
                require(++state->ticks < 800, "media reuse guard timed out");
                if (model->loading()) return;
                if (state->stage == 0) {
                    require(model->demoMode() && model->count() == 240, "media reuse fixture missing");
                    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("library")));
                    state->stage = 1; state->step.start(); return;
                }
                int ready = 0, masked = 0;
                QObject *hiddenRow = nullptr, *revealedRow = nullptr;
                for (auto *row : objects(window)) {
                    if (row->objectName() != "modRow") continue;
                    const auto id = row->property("modId").toString();
                    const auto mod = model->modDetails(id);
                    require(mod.value("id").toString() == id && !id.isEmpty(), "card has no current model owner");
                    const auto images = mod.value("images").toStringList();
                    require(!images.isEmpty(), "model owner has no fixture artwork");
                    auto *preview = named(row, "rowPreview");
                    auto *image = named(preview, "previewImage");
                    const bool active = row->property("mediaActive").toBool();
                    const bool isMasked = preview->property("masked").toBool();
                    const auto source = image->property("source").toUrl();
                    const int status = image->property("status").toInt();
                    // Check every live card on every 25 ms tick, including pooled cards.
                    // Expectations come from the model, never the card's ownerSnapshot.
                    if (status == 1) {
                        require(preview->property("source").toUrl() == QUrl(images.first())
                                && source == QUrl(images.first()), "M1: Ready image belongs to another row");
                        require(!isMasked, "M2: masked preview has Ready pixels");
                        if (active) ++ready;
                    }
                    if (mod.value("previewHidden").toBool() && id != state->revealedId)
                        require(isMasked, "M2: hidden row lost its mask without a reveal");
                    if (isMasked || !active) {
                        require(source.isEmpty() && status == 0, "M2/M5: masked or inactive row retained requested/decoded pixels");
                        if (active && isMasked) { ++masked; hiddenRow = row; }
                    }
                    if (id == state->revealedId) {
                        revealedRow = row;
                        if (!active) state->revealDropped = true;
                    }
                    bool known = false;
                    for (auto &owner : state->owners) {
                        if (owner.first != row) continue;
                        if (owner.second != id) {
                            if (model->modDetails(owner.second).value("previewHidden").toBool() && !mod.value("previewHidden").toBool()) ++state->hiddenToVisible;
                            ++state->reuses; owner.second = id;
                        }
                        known = true; break;
                    }
                    if (!known) state->owners.append({row, id});
                }
                ++state->samples;
                auto *results = named(window, "modList");
                auto *view = results->property("view").value<QObject *>();
                require(view, "media reuse view missing");
                if (results->property("restoring").toBool()) return;
                if (state->stage == 1) {
                    if (state->mode && !state->gridSized) {
                        // Size the newly loaded grid, not the retired list host.
                        // Odd columns let recycling cross the fixture's parity.
                        require(results->setProperty("minimumCellWidth", results->property("width").toReal()/3.1), "cannot size reuse grid");
                        state->gridSized=true; state->step.restart(); return;
                    }
                    if (state->step.elapsed() < 300 || !ready || !masked) return;
                    if (state->mode) require(std::lround(view->property("width").toReal()/view->property("cellWidth").toReal())==3,"reuse grid does not have three columns");
                    if (scenario == "media-reveal-reuse") {
                        require(hiddenRow, "no hidden row to reveal");
                        state->revealedId = hiddenRow->property("modId").toString();
                        click(named(hiddenRow, "rowPreview"), "revealPreview");
                    }
                    state->stage = 2; return;
                }
                if (state->stage == 2) {
                    if (!state->revealedId.isEmpty()) {
                        require(revealedRow, "revealed row disappeared before scrolling");
                        auto *preview = named(revealedRow, "rowPreview");
                        if (!imageReady(preview)) return;
                        require(!preview->property("masked").toBool(), "explicit reveal did not show artwork");
                    }
                    view->setProperty("contentY", view->property("contentHeight").toReal() * .45);
                    state->stage = 3; state->trip = 0; state->step.restart(); return;
                }
                if (state->step.elapsed() < 400 || !ready || !masked) return;
                if (state->trip < 3) {
                    if (!state->revealedId.isEmpty()) {
                        require(!revealedRow || !revealedRow->property("mediaActive").toBool(), "revealed owner did not leave the viewport");
                        state->revealDropped = true;
                    }
                    view->setProperty("contentY", state->trip == 0 ? view->property("contentHeight").toReal() * .8
                        : state->trip == 1 ? view->property("contentHeight").toReal() * .62 : 0.0);
                    ++state->trip; state->step.restart(); return;
                }
                require(state->reuses > 0, "scroll did not actually reuse any card");
                require(state->hiddenToVisible > 0, "reuse did not include a hidden-to-visible owner change");
                if (!state->revealedId.isEmpty()) {
                    require(state->revealDropped && revealedRow, "revealed owner did not leave and return");
                    auto *preview = named(revealedRow, "rowPreview");
                    auto *image = named(preview, "previewImage");
                    require(preview->property("masked").toBool() && !preview->property("revealed").toBool()
                            && image->property("source").toUrl().isEmpty() && image->property("status").toInt() == 0,
                            "M2: reveal survived scrolling away and reuse");
                }
                std::printf("PASS %s %s: %d tick samples, %d owner reuses\n", qPrintable(scenario),
                            state->mode ? "grid" : "list", state->samples, state->reuses);
                if (!state->mode) {
                    state->mode = 1; state->stage = 1; state->reuses = 0; state->samples = 0; state->hiddenToVisible = 0;
                    state->revealedId.clear(); state->revealDropped = false; state->owners.clear();
                    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("grid")));
                    state->step.restart(); return;
                }
                timer->stop(); QCoreApplication::exit(0);
            } catch (const std::exception &e) {
                timer->stop(); std::fprintf(stderr, "FAIL %s: %s\n", qPrintable(scenario), e.what());
                QCoreApplication::exit(1);
            }
        });
        timer->start(); return;
    }
    if (scenario == "first-run-welcome" || scenario == "first-run" || scenario == "first-run-pick" || scenario == "first-run-explicit") {
        auto *timer = new QTimer(window); timer->setInterval(50);
        QObject::connect(timer, &QTimer::timeout, window, [window,model,timer,scenario,stage=0,ticks=0]() mutable {
            try {
                require(++ticks < 600, qPrintable("first run timed out: " + model->error()));
                if (model->loading() || model->busy()) return;
                if (scenario == "first-run-explicit") {
                    require(!model->error().isEmpty(), "explicit unreadable source lost its error");
                    timer->stop(); std::puts("PASS UI explicit source error"); QCoreApplication::exit(0); return;
                }
                if (stage == 0) {
                    if (!model->canCreateLibrary()) return;
                    require(model->error().isEmpty(), "missing old manager is reported as an error");
                    require(!named(window,"statusNotice")->property("visible").toBool(), "normal first run shows a status banner");
                    require(model->performance()->error().isEmpty(), "missing old manager leaked into Tweaks");
                    require(model->autoexec()->error().isEmpty() && model->crosshair()->error().isEmpty(), "missing old manager leaked into config tabs");
                    require(named(window,"homeWelcome")->property("visible").toBool(), "Home welcome missing");
                    if (scenario == "first-run-welcome") {timer->stop();std::puts("PASS UI first-run welcome");QCoreApplication::exit(0);return;}
                    for (const auto &page : {"library", "browse", "tweaks", "downloads"}) {
                        QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString(page)));
                        QCoreApplication::processEvents();
                        require(named(window,"firstRunGameSettings")->property("visible").toBool(), "no-game page has no folder path");
                        if (QString(page)=="downloads") require(named(window,"downloadsBrowseButton")->property("visible").toBool(), "Downloads has no Browse action");
                    }
                    QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("settings")));
                    stage=1; return;
                }
                if (stage == 1) {
                    require(named(window,"chooseGameFolderButton")->property("enabled").toBool(), "first-run folder picker disabled");
                    auto *folderRow = named(window, "libraryFolderRow");
                    QObject *openButton = nullptr;
                    for (auto *item : objects(folderRow))
                        if (item->metaObject()->indexOfMethod("click()") >= 0 && item->property("text").toString() == "Open" && item->property("enabled").toBool())
                            openButton = item;
                    require(openButton, "Settings folder has no Open action");
                    QQmlComponent receiver(qmlEngine(window));
                    receiver.setData(R"(import QtQml
                        QtObject { property url opened; function record(location: url) { opened = location } }
                    )", QUrl());
                    std::unique_ptr<QObject> capture(receiver.create());
                    require(capture != nullptr, "Cannot record the Settings folder action");
                    QDesktopServices::setUrlHandler("file", capture.get(), "record");
                    const bool activated = QMetaObject::invokeMethod(openButton, "click");
                    QDesktopServices::unsetUrlHandler("file");
                    const auto url = capture->property("opened").toUrl();
                    require(activated && url.isLocalFile() && url.toLocalFile() == model->dataDir()
                            && !url.hasQuery() && !url.hasFragment(),
                            qPrintable(QString("Open folder handed off %1 (expected %2), activated=%3").arg(url.toString(QUrl::FullyEncoded), model->dataDir()).arg(activated)));
                    if (scenario == "first-run-pick") {
                        QVariant accepted;
                        require(QMetaObject::invokeMethod(named(window,"settingsPage"),"chooseGameFolder",Q_RETURN_ARG(QVariant,accepted),Q_ARG(QVariant,qEnvironmentVariable("ITEM111_GAME"))), "cannot accept chosen folder");
                        require(accepted.toBool(), "folder choice before library creation rejected"); stage=3; return;
                    }
                    require(model->createLibrary(), "cannot create first library"); stage=2; return;
                }
                if (stage == 2) {
                    if (!model->nativeMode() || !model->canEditProfiles()) return;
                    require(model->gamePath().isEmpty(), "fresh library unexpectedly has a game");
                    auto *local=model->localImport();
                    if (!local->canPrepare()) return;
                    require(local->prepare({QUrl::fromLocalFile(qEnvironmentVariable("ITEM111_MOD"))}), "cannot import without game"); stage=20; return;
                }
                if (stage == 20) {
                    auto *local=model->localImport();
                    require(local->error().isEmpty(), qPrintable(local->error()));
                    if (!local->presentationReady()) return;
                    require(local->reviewCurrentDraft(), "cannot review without game"); stage=21; return;
                }
                if (stage == 21) {
                    auto *local=model->localImport();
                    require(local->error().isEmpty(), qPrintable(local->error()));
                    if (!local->canCommit()) return;
                    require(local->commit(), "cannot add mod without game"); stage=22; return;
                }
                if (stage == 22) {
                    require(model->error().isEmpty(), qPrintable(model->error()));
                    if (model->count()!=1) return;
                    require(model->gamePath().isEmpty(), "import required a game folder");
                    for (const auto &page : {"home", "library", "browse", "tweaks", "downloads", "settings"}) {
                        QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString(page)));
                        QCoreApplication::processEvents();
                        capture(window, "no-game-" + QString(page));
                    }
                    require(model->performance()->error().isEmpty(), "no-game Tweaks reports an error");
                    require(model->autoexec()->error().isEmpty() && model->crosshair()->error().isEmpty(), "no-game config tabs report an error");
                    stage=23; return;
                }
                if (stage == 23) {
                    if (!model->canEditProfiles()) return;
                    require(model->useGameFolder(qEnvironmentVariable("ITEM111_GAME")), "cannot choose game folder"); stage=3; return;
                }
                if (stage == 3) {
                    require(model->error().isEmpty(), qPrintable(model->error()));
                    require(model->gamePath()==qEnvironmentVariable("ITEM111_GAME"), "chosen folder not saved");
                    // Choosing the folder can start automatic Apply before its RPC finishes.
                    if (!model->canPreviewApply()) return;
                    capture(window, "fake-game-settings");
                    timer->stop(); std::puts("PASS UI first run: welcome, quiet startup, create library, choose game"); QCoreApplication::exit(0);
                }
            } catch (const std::exception &e) { std::fprintf(stderr,"FAIL first run: %s\n",e.what()); QCoreApplication::exit(1); timer->stop(); }
        }); timer->start(); return;
    }
    if (scenario == "native-perf-apply") {
        auto *timer=new QTimer(window);timer->setInterval(5);
        auto initialRevision=std::make_shared<QString>();
        auto stage=std::make_shared<int>(0);auto start=std::make_shared<qint64>(QDateTime::currentMSecsSinceEpoch());
        QObject::connect(timer,&QTimer::timeout,window,[=]{
            if(QDateTime::currentMSecsSinceEpoch()-*start>120000){std::fprintf(stderr,"PERF_TIMEOUT %s\n",qPrintable(model->error()));QCoreApplication::exit(1);return;}
            if(*stage==0){
                if(model->loading() || !model->canPreviewApply() || !model->canSetEnabled())return;
                *initialRevision=model->revision();*start=QDateTime::currentMSecsSinceEpoch();*stage=1;
                if(qEnvironmentVariable("ITEM104_MODE")=="toggle"){
                    model->setAutomaticApply(true);
                    for(const auto &v:model->fullProfileOrder()){const auto row=v.toMap();if(row.value("enabled").toBool()){model->setModEnabled(row.value("id").toString(),false);return;}}
                }else model->previewApply(model->gamePath());
            }else if(*stage==1 && !model->applyReview().isEmpty()){
                std::printf("PERF_REVIEW_MS %lld\n",QDateTime::currentMSecsSinceEpoch()-*start);std::fflush(stdout);
                if(qEnvironmentVariable("ITEM104_MODE")=="toggle"){std::fprintf(stderr,"PERF_AUTO_REVIEW_REQUIRED %s\n",qPrintable(model->error()));QCoreApplication::exit(1);return;}
                if(model->commitApply())*stage=2;
            }else if(model->revision()!=*initialRevision && !model->busy() && !model->loading() && !model->pendingChanges() && model->applyReview().isEmpty()){
                std::printf("PERF_FINISHED_MS %lld\n",QDateTime::currentMSecsSinceEpoch()-*start);std::fflush(stdout);timer->stop();QCoreApplication::exit(0);
            }
        });timer->start();return;
    }

    if (scenario == "downloads-active") { startDownloadCapture(window, model); return; }
    if (scenario.startsWith("crosshair-review-")) { startCrosshairRegression(window, scenario.mid(17)); return; }
    if (scenario == "crosshair-values") { startCrosshairCapture(window); return; }
    if (scenario == "shared-controls") {
        QTimer::singleShot(0, window, [window, model] {
            try {
                QQmlComponent probe(qmlEngine(window));
                probe.setData(R"(import QtQuick
import QtQuick.Controls.Basic
import Deadlock.Native
Column {
    id: probeRoot
    property var appModel: null
    width: 600; spacing: 8
    property string selection: "one"
    property string requestedSelection: ""
    property bool acceptSelection: true
    FilterChip { objectName: "testChip"; text: "On only"; count: 3 }
    Disclosure { objectName: "testDisclosure"; width: 500; label: "Files"; summary: "pak01.vpk" }
    SegmentedControl {
        objectName: "testSegments"
        model: [{text: "One", value: "one", objectName: "testSegmentOne"}, {text: "Two", value: "two", objectName: "testSegmentTwo"}]
        currentValue: probeRoot.selection
        onActivated: value => {
            probeRoot.requestedSelection = value
            if (probeRoot.acceptSelection) probeRoot.selection = value
        }
    }
    SettingRow { width: 500; title: "Motion"; description: "Animation setting"; Switch { objectName: "testSetting" } }
    Callout { width: 500; title: "Warning"; text: "Review these files"; tone: "warning"; Button { objectName: "testAction"; text: "Review" } }
    StatusPill { text: "In library"; tone: "brass"; iconName: "check" }
    ModRow {
        objectName: "quietRow"; nativeMode: true
        index: 0; ownerSnapshot: null
        resultHost: ({scope: "library", presentationMode: "grid", width: 500, cellWidth: 500, gridHeight: 200, listHeight: 90,
                      modelCoherent: false, ready: false, inRange: () => false})
        modId: "fixture"; modTitle: "Fixture"; heroName: ""; modKind: ""; modAuthor: ""
        modEnabled: true; readOnly: false; modStatus: ""; modPreview: ""; modPreviewHidden: false
        modAppliedEnabled: false; modSelectionReady: true; modPendingStatus: "pending"; modPendingApply: true
        modSelectionSaved: true; modAvailable: true; modUnavailableReason: ""; modUpdateStatus: ""; modHealthStatus: ""
    }
    ModDetails {
        objectName: "updateDetails"; width: 330; height: 850; readOnly: false
        technicalExpanded: true
        mod: ({id: "fixture", title: "Fixture", enabled: true, remoteSubmission: {submissionId: "1", submissionType: "Mod"}, fileName: "very_long_original_download_name_that_differs_from_installed_package_names_and_must_remain_readable.vpk", installedVpks: ["pak01.vpk"], missingVpks: [],
            update: {versionUnknown: true, changelog: {items: [{text: "Saving is back and automatic"}]}}})
    }
    ModDetails {
        objectName: "nativeDetails"; width: 330; height: 850; nativeMode: true; readOnly: false
        mod: ({id: "m1", title: "Linked Mod", enabled: true, remoteSubmission: {provider: "gamebanana", submissionType: "mod", submissionId: "42"}, linkEligible: false, linkReason: "", installedVpks: [], missingVpks: [],
            update: {status: "unsure", reason: "Version family could not be confirmed."}})
    }
    SettingsPage {
        objectName: "probeSettings"; width: 600; height: 400
        library: probeRoot.appModel
    }

})", QUrl());
                std::unique_ptr<QObject> object(probe.createWithInitialProperties({{QStringLiteral("appModel"), QVariant::fromValue(model)}}));
                if (!object) qWarning() << probe.errors();
                require(bool(object), "shared controls probe failed to load");
                auto *item = qobject_cast<QQuickItem *>(object.get());
                item->setParentItem(window->contentItem());
                auto *chip = object->findChild<QQuickItem *>("testChip");
                auto *accessibleChip = QAccessible::queryAccessibleInterface(chip);
                require(accessibleChip && accessibleChip->text(QAccessible::Name) == "On only, 3", "chip accessible count missing");
                chip->setProperty("count", -1);
                require(accessibleChip->text(QAccessible::Name) == "On only", "unknown chip count announced");
                chip->forceActiveFocus(Qt::TabFocusReason);
                require(chip->property("visualFocus").toBool(), "chip focus ring missing");
                realKey(window, Qt::Key_Space);
                require(chip->property("checked").toBool(), "chip Space toggle failed");
                realKey(window, Qt::Key_Tab);
                auto *disclosure = object->findChild<QQuickItem *>("testDisclosure");
                require(disclosure->hasActiveFocus() && disclosure->property("visualFocus").toBool(), "disclosure Tab focus failed");
                realKey(window, Qt::Key_Space);
                require(disclosure->property("checked").toBool(), "disclosure Space toggle failed");
                realKey(window, Qt::Key_Tab);
                realKey(window, Qt::Key_Right);
                auto *segments = object->findChild<QObject *>("testSegments");
                require(segments->property("currentValue").toString() == "two", "segment Right failed");
                realKey(window, Qt::Key_Right);
                require(segments->property("currentValue").toString() == "one", "segment wrap failed");
                realKey(window, Qt::Key_Left);
                require(segments->property("currentValue").toString() == "two", "segment Left failed");
                object->setProperty("selection", "one");
                require(segments->property("currentValue").toString() == "one", "segment activation broke consumer binding");
                auto *one = qobject_cast<QQuickItem *>(named(segments, "testSegmentOne"));
                auto *two = qobject_cast<QQuickItem *>(named(segments, "testSegmentTwo"));
                auto *accessibleTwo = QAccessible::queryAccessibleInterface(two);
                require(accessibleTwo && accessibleTwo->role() == QAccessible::RadioButton
                    && accessibleTwo->state().checkable && !accessibleTwo->state().checked,
                    "unselected segment lacks accessible radio state");
                // A request may leave the model unchanged, as in Bisect with every mod enabled.
                object->setProperty("acceptSelection", false);
                object->setProperty("requestedSelection", "");
                realClickItem(window, two);
                require(object->property("requestedSelection").toString() == "two", "segment click lost its request");
                require(one->property("checked").toBool() && !two->property("checked").toBool() && !accessibleTwo->state().checked,
                    "segment click displayed a selection the model did not accept");
                object->setProperty("acceptSelection", true);
                two->forceActiveFocus(Qt::TabFocusReason);
                realKey(window, Qt::Key_Space);
                require(segments->property("currentValue").toString() == "two"
                    && !one->property("checked").toBool() && two->property("checked").toBool() && accessibleTwo->state().checked,
                    "segment Space did not update the model and selected state");
                auto *actions = accessibleTwo->actionInterface();
                require(actions, "segment exposes no accessible actions");
                const auto selectAction = actions->actionNames().contains(QAccessibleActionInterface::pressAction())
                    ? QAccessibleActionInterface::pressAction() : QAccessibleActionInterface::toggleAction();
                require(actions->actionNames().contains(selectAction), "segment exposes no selection action");
                object->setProperty("selection", "one");
                for (bool accept : {false, true}) {
                    object->setProperty("acceptSelection", accept);
                    object->setProperty("requestedSelection", "");
                    actions->doAction(selectAction);
                    require(object->property("requestedSelection").toString() == "two", "accessible action lost its request");
                    require(accessibleTwo->state().checked == accept && one->property("checked").toBool() == !accept,
                        "accessible action bypassed the model selection");
                }
                auto *setting = object->findChild<QQuickItem *>("testSetting");
                setting->forceActiveFocus(Qt::TabFocusReason);
                realKey(window, Qt::Key_Space);
                require(setting->property("checked").toBool(), "setting trailing control failed");
                auto *action = object->findChild<QQuickItem *>("testAction");
                action->forceActiveFocus(Qt::TabFocusReason);
                require(action->property("visualFocus").toBool(), "callout action focus ring missing");
                auto *row = object->findChild<QObject *>("quietRow");
                require(row && row->property("status").toString().isEmpty(), "normal pending mod has a status pill");
                row->setProperty("modEnabled", false);
                row->setProperty("profileName", "Test profile");
                const auto *rowAccessible = QAccessible::queryAccessibleInterface(named(row, "modSwitch"));
                require(rowAccessible && rowAccessible->text(QAccessible::Name) == "Fixture, off in Test profile", "Library switch lost off state/profile");
                row->setProperty("modPendingApply", false);
                row->setProperty("modPendingStatus", "current");
                require(row->property("status").toString().isEmpty(), "saved mod has a misleading pending pill");
                row->setProperty("modPendingStatus", "selectionRequired");
                require(row->property("status").toString() == "Choose files", "actionable mod status hidden");
                auto *callout = object->findChild<QObject *>("modUpdateCard");
                require(callout && callout->property("text").toString() == "The installed version could not be identified. Update to the latest file.", "version warning shows changelog instead of state explanation");
                auto *detail = object->findChild<QQuickItem *>("updateDetails");
                auto *toggle = detail->findChild<QQuickItem *>("modSwitch");
                require(toggle && toggle->isEnabled(), "details switch missing");
                toggle->forceActiveFocus(Qt::TabFocusReason);
                realKey(window, Qt::Key_Tab);
                auto *next = window->activeFocusItem();
                require(next && qobject_cast<QQuickItem *>(callout)->isAncestorOf(next), "details Tab must reach update action after switch");
                auto *fileName = detail->findChild<QQuickItem *>("detailFileName");
                require(fileName && fileName->property("text").toString().startsWith("very_long_original"), "expanded filename differs from summary");
                auto *updateDetailFiles = detail->findChild<QQuickItem *>("detailFiles");
                require(updateDetailFiles && updateDetailFiles->isVisible() && updateDetailFiles->property("text").toString() == "Installed packages:\npak01.vpk", "empty missingVpks row was not suppressed");

                // Native ModDetails behavior: closed menus on load/selection changes, technical details, and update status/gates.
                auto *nativeDetail = object->findChild<QQuickItem *>("nativeDetails");
                require(nativeDetail, "nativeDetails probe missing");
                auto countVisiblePopups = [&]() {
                    int n = 0;
                    for (auto *o : objects(window))
                        if (o->inherits("QQuickPopup") && o->property("visible").toBool()) ++n;
                    return n;
                };
                auto *actionsMenu = nativeDetail->findChild<QObject *>("modActionsMenu");
                require(actionsMenu && !actionsMenu->property("opened").toBool() && !actionsMenu->property("visible").toBool(), "modActionsMenu opened automatically");
                require(countVisiblePopups() == 0, "unexpected popup opened automatically on load");
                auto *techPanel = nativeDetail->findChild<QQuickItem *>("technicalDetailsPanel");
                require(techPanel && !techPanel->isVisible(), "technical details must be hidden by default");
                auto *statusPill = nativeDetail->findChild<QQuickItem *>("modUpdateStatusPill");
                auto *statusNote = nativeDetail->findChild<QQuickItem *>("modUpdateStatusNote");
                require(statusPill && statusPill->isVisible() && statusPill->property("text").toString() == "Version couldn't be confirmed", "unsure status not visible in default panel");
                require(statusNote && statusNote->isVisible() && statusNote->property("text").toString().contains("Version family"), "unsure reason missing in default panel");
                auto *checkBtn = nativeDetail->findChild<QQuickItem *>("modCheckUpdatesButton");
                auto *optionsBtn = nativeDetail->findChild<QQuickItem *>("filesOptionsButton");
                require(checkBtn && checkBtn->isVisible(), "Check for updates button missing in main panel");
                require(optionsBtn && optionsBtn->isVisible(), "Files and options button missing in main panel");
                require(!checkBtn->isEnabled() && !optionsBtn->isEnabled(), "actions stayed enabled without remote/options controllers");

                // Toggling nativeMode or changing mod selection never opens any popup.
                nativeDetail->setProperty("nativeMode", false);
                nativeDetail->setProperty("nativeMode", true);
                require(countVisiblePopups() == 0, "toggling nativeMode opened a popup");

                auto waitUntil = [&](auto &&predicate) {
                    QElapsedTimer elapsed; elapsed.start();
                    while (!predicate() && elapsed.elapsed() < 1000)
                        QCoreApplication::processEvents(QEventLoop::AllEvents, 20);
                    return predicate();
                };
                auto *actionsBtn = nativeDetail->findChild<QQuickItem *>("modActionsButton");
                require(actionsBtn && actionsBtn->isVisible(), "modActionsButton missing");
                QMetaObject::invokeMethod(actionsBtn, "clicked");
                require(waitUntil([&] { return actionsMenu->property("opened").toBool(); }) && countVisiblePopups() == 1, "clicking More did not open exactly one menu");
                realKey(window, Qt::Key_Escape);
                require(waitUntil([&] { return !actionsMenu->property("visible").toBool(); }) && countVisiblePopups() == 0, "Escape did not close modActionsMenu");

                // Explicit Technical details toggle works and suppresses empty None recorded rows; selection invalidation closes menu.
                QMetaObject::invokeMethod(actionsBtn, "clicked");
                require(waitUntil([&] { return actionsMenu->property("opened").toBool(); }), "reopening modActionsMenu failed");
                auto *techBtn = actionsMenu->findChild<QQuickItem *>("technicalDetailsButton");
                require(techBtn && techBtn->isVisible(), "technicalDetailsButton missing in More menu");
                realClickItem(window, techBtn);
                require(waitUntil([&] { return !actionsMenu->property("visible").toBool(); }), "activating Technical details did not close menu");
                require(nativeDetail->property("technicalExpanded").toBool() && techPanel->isVisible(), "Technical details action did not expand panel");
                auto *nativeFiles = nativeDetail->findChild<QQuickItem *>("detailFiles");
                require(nativeFiles && !nativeFiles->isVisible() && !nativeFiles->property("text").toString().contains("None recorded"), "empty package lists showed None recorded");
                auto *hideTechBtn = nativeDetail->findChild<QQuickItem *>("hideTechnicalDetailsButton");
                require(hideTechBtn && hideTechBtn->isVisible(), "hideTechnicalDetailsButton missing");
                QMetaObject::invokeMethod(hideTechBtn, "clicked");
                require(!nativeDetail->property("technicalExpanded").toBool() && !techPanel->isVisible(), "Hide button did not collapse technical details");

                QMetaObject::invokeMethod(actionsBtn, "clicked");
                require(waitUntil([&] { return actionsMenu->property("opened").toBool(); }), "menu failed to reopen before selection change");
                QVariantMap unlinkedMod;
                unlinkedMod.insert("id", "m2");
                unlinkedMod.insert("title", "Unlinked Local");
                unlinkedMod.insert("enabled", true);
                unlinkedMod.insert("linkEligible", true);
                unlinkedMod.insert("installedVpks", QStringList{});
                unlinkedMod.insert("missingVpks", QStringList{});
                nativeDetail->setProperty("mod", unlinkedMod);
                require(waitUntil([&] { return !actionsMenu->property("visible").toBool(); }) && countVisiblePopups() == 0, "selection change left modActionsMenu open");
                require(statusPill->isVisible() && statusPill->property("text").toString() == "Not linked", "unlinked eligible mod status wrong");
                require(!checkBtn->isVisible(), "Check for updates shown for unlinked mod");

                // Unlinked uncheckable mod with linkReason.
                unlinkedMod.insert("linkEligible", false);
                unlinkedMod.insert("linkReason", "Local archive without provenance.");
                nativeDetail->setProperty("mod", unlinkedMod);
                require(statusPill->isVisible() && statusPill->property("text").toString() == "Unlinked", "unlinked uncheckable status wrong");
                require(statusNote->isVisible() && statusNote->property("text").toString().contains("Local archive without provenance."), "linkReason hidden for unlinked mod");

                // SettingsPage must not expose developer overlay controls.
                auto *settingsObj = object->findChild<QObject *>("probeSettings");
                require(settingsObj, "probeSettings missing");
                for (auto *o : objects(settingsObj)) {
                    const auto txt = o->property("text").toString();
                    require(txt != "For developers" && txt != "Show performance overlay" && txt != "Hide performance overlay", "SettingsPage still exposes developer performance overlay");
                }
                std::puts("PASS UI shared controls: keyboard, focus, counts, icons, ModDetails menus/updates and Settings");
                QCoreApplication::exit(0);
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL shared controls: %s\n", error.what());
                QCoreApplication::exit(30);
            }
        });
        return;
    }
    if (scenario == "palette") {
        if (qEnvironmentVariableIsSet("CHECK_PORTAL_DARK")) {
            applyAppImagePalette(Qt::ColorScheme::Dark);
        }
        QTimer::singleShot(0, window, [window] {
            try {
                QQmlComponent probe(qmlEngine(window));
                probe.setData(R"(import QtQuick
import QtQuick.Controls
import DeadlockStyle
import Deadlock.Native
Item {
    property var cardCheck
    MediaCard {
        index: 0; anchorIdentity: "palette"; heading: "Palette"
        ownerSnapshot: ({identity: "palette"})
        resultHost: ({scope: "library", presentationMode: "list", width: 500, cellWidth: 500, listHeight: 100,
            ready: false, modelCoherent: false, inRange: () => false, issueToken: () => "palette"})
        selectable: true; markMode: true
    }
    property SystemPalette colors: SystemPalette { colorGroup: SystemPalette.Active }
    property color windowColor: colors.window
    property color textColor: colors.windowText
    property color highlightColor: colors.highlight
    width: 200; height: 80; z: 100000
    Component.onCompleted: Tokens.colourMode = "desktop"
    property real faintContrast: Tokens.contrast(Tokens.textFaint, Tokens.surface2)
    CheckBox { id: focusCheck; checked: true; focus: true }
    function checkBrand(): bool {
        Tokens.colourMode = "modlock"
        const backgrounds = [Tokens.window, Tokens.surface1, Tokens.surface2, Tokens.surface3, Tokens.mix(Tokens.window, Tokens.accent, .28)]
        const inks = [Tokens.text, Tokens.textMuted, Tokens.textFaint, Tokens.accentInk, Tokens.brass, Tokens.live, Tokens.warning, Tokens.danger, Tokens.success]
        let worst = 21
        for (const ink of inks) {
            const ratio = Tokens.worstContrast(ink, backgrounds)
            console.info("BRAND CONTRAST", Tokens.dark ? "Noir" : "Ivory", ink, ratio)
            worst = Math.min(worst, ratio)
        }
        for (const fill of [Tokens.accent, Tokens.accentHover, Tokens.accentPressed]) worst = Math.min(worst, Tokens.contrast(Tokens.accentText, fill))
        const specs = Tokens.dark ? ["#8f879f", "#d2ae68", "#45d3de", "#f1a04e", "#f2707a", "#5fcf98"]
                                  : ["#655e76", "#735619", "#0d6f78", "#8a4c00", "#b02a33", "#186a3c"]
        const semantic = [Tokens.textFaint, Tokens.brass, Tokens.live, Tokens.warning, Tokens.danger, Tokens.success]
        for (let i = 0; i < semantic.length; ++i) {
            const spec = Qt.color(specs[i]), actual = semantic[i]
            if (Math.max(Math.abs(spec.r-actual.r), Math.abs(spec.g-actual.g), Math.abs(spec.b-actual.b)) > .15) return false
        }
        // Offset rings border the surrounding surface on both edges, with a 2px gap to the fill.
        if (Tokens.worstContrast(Tokens.focus, [Tokens.window, Tokens.surface1, Tokens.surface2, Tokens.surface3]) < 3) return false
        const ring = focusCheck.indicator.children.find(child => child.objectName === "checkFocusRing")
        const fillContrast = Tokens.contrast(Tokens.focus, focusCheck.indicator.color)
        console.info("CHECKBOX FOCUS", Tokens.dark ? "Noir" : "Ivory", "against checked fill", fillContrast)
        // Low contrast against the fill is safe only with an outer ring and a
        // transparent gap: both actual adjacent colours are the surrounding surface.
        if (!ring || ring.border.color !== Tokens.focus || ring.border.width < 2) return false
        if (fillContrast < 3 && (ring.x + ring.border.width > -2 || ring.y + ring.border.width > -2
            || ring.width < focusCheck.indicator.width + 8 || ring.color.a !== 0)) return false
        const cardRing = cardCheck.indicator.children.find(child => child.objectName === "checkFocusRing")
        const surface = cardCheck.background.color
        const cardContrast = Tokens.contrast(Tokens.focus, surface)
        for (const artwork of [Qt.color("white"), Qt.color("black")]) {
            const adjacent = Tokens.mix(artwork, surface, surface.a)
            const ratio = Tokens.contrast(Tokens.focus, adjacent)
            console.info("CARD CHECKBOX FOCUS", Tokens.dark ? "Noir" : "Ivory", "artwork", artwork, "surface", adjacent, ratio)
            if (ratio < 3) return false
        }
        // Opaque backing makes the adjacent colour identical over white and black artwork.
        if (surface.a !== 1 || surface !== Tokens.surface2 || cardContrast < 3) return false
        const origin = cardCheck.indicator.mapToItem(cardCheck.background, cardRing.x, cardRing.y)
        if (origin.x < 4 || origin.y < 4
            || origin.x + cardRing.width > cardCheck.background.width - 4
            || origin.y + cardRing.height > cardCheck.background.height - 4) return false
        if (Tokens.contrast(Tokens.text, Tokens.surface4) < 3) return false
        const boundary = Math.min(Tokens.contrast(Tokens.fieldBorder, Tokens.window), Tokens.contrast(Tokens.accent, Tokens.window))
        Tokens.colourMode = "desktop"
        return worst >= 4.5 && boundary >= 3
    }
    property color normalColor: Tokens.surface2
    property color hoverColor: Tokens.surface3
    property color pressedColor: Tokens.surface4
    property color inkColor: Tokens.accentInk
    property real inkWorst: Tokens.worstContrast(Tokens.accentInk, [Tokens.window, Tokens.surface1, Tokens.surface2,
        Tokens.surface3, Tokens.surface4, Tokens.mix(Tokens.window, Tokens.accent, 0.28)])
    // Review cases: mid-grey windows with white, black and grey accents.
    // Margin above the best contrast any colour could reach (capped at 4.5); never negative.
    function greyCase(w, a) {
        const bg = [w, Tokens.mix(w, a, 0.28)]
        const best = Math.max(Tokens.worstContrast(a, bg), Tokens.worstContrast(grey(255), bg), Tokens.worstContrast(grey(0), bg))
        return Tokens.worstContrast(Tokens.readableOn(a, bg), bg) - Math.min(4.5, best)
    }
    function grey(v) { return Qt.rgba(v / 255, v / 255, v / 255, 1) }
    property real greyWorst: Math.min(greyCase(grey(118), grey(255)), greyCase(grey(118), grey(0)),
                                      greyCase(grey(118), grey(110)), greyCase(grey(128), grey(0)))
    property color checkedHoverBackground: Tokens.mix(Tokens.window, Tokens.accent, 0.28)
    Button { objectName: "paletteButton"; text: "Hover"; hoverEnabled: true; width: 150; height: 50 }

})", QUrl());
                std::unique_ptr<QObject> object(probe.create());
                require(bool(object), "palette probe failed to load");
                object->setProperty("cardCheck", QVariant::fromValue(object->findChild<QObject *>("markBox")));
                const QString actual = object->property("windowColor").value<QColor>().name()
                    + "," + object->property("textColor").value<QColor>().name()
                    + "," + object->property("highlightColor").value<QColor>().name();
                std::fprintf(stderr, "PALETTE %s\n", qPrintable(actual));
                const auto expected = qEnvironmentVariable("EXPECT_PALETTE");
                require(expected.isEmpty() || actual == expected, "SystemPalette does not match fixture");
                const auto currentPalette = QGuiApplication::palette();
                QStringList roleColors;
                for (auto state : {QPalette::Active,QPalette::Inactive,QPalette::Disabled})
                    for (int role=0; role<QPalette::NColorRoles; ++role)
                        roleColors << currentPalette.color(state, QPalette::ColorRole(role)).name(QColor::HexArgb);
                const auto allColors = roleColors.join(',');
                std::fprintf(stderr,"PALETTE_ROLES %s\n",qPrintable(allColors));
                const auto expectedRoles=qEnvironmentVariable("EXPECT_ROLE_COLORS");
                require(expectedRoles.isEmpty() || expectedRoles==allColors,"theme palette roles changed");
                if (qEnvironmentVariableIsSet("CHECK_KDE_SHADE"))
                    require(currentPalette.color(QPalette::Disabled, QPalette::Window).name() == "#1f2124", "KDE Breeze Dark disabled Window rounding differs");
                if (qEnvironmentVariableIsSet("CHECK_BUILTIN_CONTRAST")) {
                    auto luminance = [](QColor c) {
                        auto channel = [](double x) { return x <= .04045 ? x/12.92 : std::pow((x+.055)/1.055, 2.4); };
                        return .2126*channel(c.redF()) + .7152*channel(c.greenF()) + .0722*channel(c.blueF());
                    };
                    const QPalette::ColorRole pairs[][2] = {
                        {QPalette::WindowText,QPalette::Window}, {QPalette::Text,QPalette::Base},
                        {QPalette::ButtonText,QPalette::Button}, {QPalette::HighlightedText,QPalette::Highlight},
                        {QPalette::ToolTipText,QPalette::ToolTipBase}, {QPalette::PlaceholderText,QPalette::Base}};
                    for (auto state : {QPalette::Active,QPalette::Inactive,QPalette::Disabled}) {
                        for (const auto &pair : pairs) {
                            double a=luminance(currentPalette.color(state,pair[0])), b=luminance(currentPalette.color(state,pair[1]));
                            double ratio=(qMax(a,b)+.05)/(qMin(a,b)+.05);
                            std::fprintf(stderr,"CONTRAST state=%d role=%d %.3f\n",int(state),int(pair[0]),ratio);
                            require(ratio >= (state == QPalette::Disabled ? 3.0 : 4.5), "built-in text contrast too low");
                        }
                    }
                    // Accent used as text (checked filters, focus rings) on a checked, hovered button.
                    double ink=luminance(object->property("inkColor").value<QColor>()), back=luminance(object->property("checkedHoverBackground").value<QColor>());
                    double inkRatio=(qMax(ink,back)+.05)/(qMin(ink,back)+.05);
                    std::fprintf(stderr,"CONTRAST accentInk %.3f worst %.3f grey %.3f\n",inkRatio,object->property("inkWorst").toDouble(),object->property("greyWorst").toDouble());
                    require(inkRatio >= 4.5, "accent text contrast too low");
                    require(object->property("inkWorst").toDouble() >= 4.5, "accent text contrast too low on a surface");
                    require(object->property("greyWorst").toDouble() >= -0.001, "accent text below the best reachable contrast on mid-grey windows");
                }
                if (qEnvironmentVariableIsSet("CHECK_PALETTE_HOVER")) {
                    const auto palette = QGuiApplication::palette();
                    require(palette.color(QPalette::Active, QPalette::Base) == QColor(17,24,35), "View background mapping wrong");
                    require(palette.color(QPalette::Active, QPalette::Text) == QColor(211,220,231), "View text mapping wrong");
                    require(palette.color(QPalette::Active, QPalette::Button) == QColor(34,43,56), "Button mapping wrong");
                    require(palette.color(QPalette::Active, QPalette::PlaceholderText) == QColor(101,111,121), "inactive foreground mapping wrong");
                    require(palette.color(QPalette::Active, QPalette::Link) == QColor(82,161,231), "Link mapping wrong");
                    require(palette.color(QPalette::Active, QPalette::AlternateBase) == QColor(25,33,45), "alternate View mapping wrong");
                    require(palette.color(QPalette::Inactive, QPalette::Window) == QColor(24,32,44), "Inactive subgroup mapping wrong");
                    require(palette.color(QPalette::Disabled, QPalette::ToolTipBase) == QColor(12,19,29), "Tooltip must use active colours in all states");
                    require(palette.color(QPalette::Disabled, QPalette::WindowText) != palette.color(QPalette::Active, QPalette::WindowText), "disabled state effect missing");
                    auto *button = object->findChild<QObject *>("paletteButton");
                    auto *background = button->property("background").value<QObject *>();
                    require(background, "button background missing");
                    auto settle = [](int duration) {
                        QEventLoop loop;
                        QTimer::singleShot(duration, &loop, &QEventLoop::quit);
                        loop.exec();
                    };
                    // Deliver the same hover events as Qt Quick's delivery agent.
                    // Keep the probe detached: the offscreen platform's stationary
                    // global cursor otherwise re-enters it on every rendered frame.
                    auto move = [button](QPointF pos) {
                        QHoverEvent event(pos.x() < 150 ? QEvent::HoverEnter : QEvent::HoverLeave,
                            pos, pos, QPointF(190, 70));
                        QCoreApplication::sendEvent(button, &event);
                    };
                    move(QPointF(190, 70)); settle(160);
                    const auto normal = object->property("normalColor").value<QColor>();
                    const auto hover = object->property("hoverColor").value<QColor>();
                    auto between = [](QColor c, QColor a, QColor b) {
                        for (int channel = 0; channel < 3; ++channel) {
                            const int v[] = {c.red(), c.green(), c.blue()};
                            const int lo[] = {a.red(), a.green(), a.blue()};
                            const int hi[] = {b.red(), b.green(), b.blue()};
                            if (v[channel] < qMin(lo[channel], hi[channel]) - 2 || v[channel] > qMax(lo[channel], hi[channel]) + 2) return false;
                        }
                        return true;
                    };
                    std::fprintf(stderr, "BUTTON normal actual=%s expected=%s hovered=%d\n", qPrintable(background->property("color").value<QColor>().name()), qPrintable(normal.name()), button->property("hovered").toBool());
                    require(between(background->property("color").value<QColor>(), normal, normal), "normal button differs from Tokens");
                    // Brief entry, reversal mid-animation, then sustained entry.
                    for (int duration : {24, 160, 160, 160}) {
                        const bool entering = duration == 24 || !button->property("hovered").toBool();
                        move(entering ? QPointF(20, 20) : QPointF(190, 70));
                        require(button->property("hovered").toBool() == entering, "mouse did not change hover state");
                        for (int elapsed = 0; elapsed < duration; elapsed += 8) {
                            settle(8);
                            require(between(background->property("color").value<QColor>(), normal, hover), "hover animation flashed outside Token endpoints");
                        }
                        if (duration > 110)
                            require(between(background->property("color").value<QColor>(), entering ? hover : normal, entering ? hover : normal), "hover animation did not settle");
                    }
                    button->setProperty("down", true);
                    settle(160);
                    const auto pressed = object->property("pressedColor").value<QColor>();
                    require(between(background->property("color").value<QColor>(), pressed, pressed), "pressed button differs from Tokens");
                    std::fprintf(stderr, "PASS brief/reversed/sustained hover stays between Token endpoints; pressed settles\n");
                }
                require(object->property("faintContrast").toDouble() >= 4.5, "desktop faint text contrast too low");
                bool brandPass = false;
                require(QMetaObject::invokeMethod(object.get(), "checkBrand", Q_RETURN_ARG(bool, brandPass)) && brandPass,
                        "brand palette contrast too low");
                QCoreApplication::exit(0);
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL palette: %s\n", error.what());
                QCoreApplication::exit(30);
            }
        });
        return;
    }
    if (scenario == "legacy-state-reconcile") {
        auto *timer=new QTimer(window);timer->setInterval(50);
        QObject::connect(timer,&QTimer::timeout,window,[window,model,timer,ticks=0]() mutable {
            try {
                require(++ticks<400,"legacy pending reference was not reconciled");
                if(model->loading() || model->busy())return;
                QFile file(qEnvironmentVariable("EXPECT_PENDING_PATH"));
                if(!file.open(QIODevice::ReadOnly) || QJsonDocument::fromJson(file.readAll()).object().size()!=0)return;
                require(AppPaths::stateRoot()==qEnvironmentVariable("EXPECT_STATE_ROOT"),"wrong state generation");
                require(!model->outcomeUnknown() && !model->recoveryRequired(),"legacy outcome remains blocked");
                timer->stop();std::puts("PASS legacy pending reference reconciled");QCoreApplication::exit(0);
            } catch(const std::exception &e){timer->stop();std::fprintf(stderr,"FAIL legacy state: %s\n",e.what());QCoreApplication::exit(1);}
        });timer->start();return;
    }
    if (scenario == "runtime-paths") {
        QTimer::singleShot(400, window, [window] {
            try {
                const auto selected = AppPaths::stateRoot();
                require(selected == QString::fromUtf8(qgetenv("EXPECT_STATE_ROOT")), "wrong persistent state root");
                if(qEnvironmentVariableIsSet("EXPECT_LIBRARY_ROOT")) require(qmlEngine(window)->rootContext()->contextProperty("libraryDataRoot").toString()==qEnvironmentVariable("EXPECT_LIBRARY_ROOT"),"app and helper chose different libraries");
                require(qmlEngine(window)->offlineStoragePath() == AppPaths::offlineStorageRoot(), "offline storage mismatch");
                QCoreApplication::exit(0);
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL runtime-paths: %s\n", error.what());
                QCoreApplication::exit(30);
            }
        });
        return;
    }
    if (scenario == "appearance-preferences") {
        QTimer::singleShot(0, window, [window] {
            try {
                auto *engine = qmlEngine(window);
                auto *prefs = engine->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
                const bool restart = qEnvironmentVariableIsSet("APPEARANCE_RESTART");
                require(prefs->property("colourMode").toString() == (restart ? "desktop" : "modlock"), "appearance mode default/restart failed");
                require(prefs->property("reduceMotion").toBool() == restart, "motion default/restart failed");
                require(prefs->property("hideNsfwMods").toBool() == !restart, "mature mod filter default/restart failed");
                if (!restart) {
                    prefs->setProperty("colourMode", "desktop");
                    prefs->setProperty("reduceMotion", true);
                    require(prefs->setProperty("hideNsfwMods", false), "mature mod filter could not be changed");
                    require(QMetaObject::invokeMethod(prefs, "sync"), "appearance sync failed");
                }
                std::puts("PASS appearance defaults/restart"); QCoreApplication::exit(0);
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL appearance: %s\n", error.what()); QCoreApplication::exit(30);
            }
        });
        return;
    }
    if (scenario == "preferences") {
        QTimer::singleShot(0, window, [window, model] {
        try {
            auto *engine = qmlEngine(window);
            auto *preferences = engine->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
            require(preferences, "preferences singleton missing");
            require(QCoreApplication::organizationName() == "dl-mod-manager", "organisation is not neutral");
            require(QCoreApplication::applicationName() == "Deadlock Native", "application identity changed");
            const auto rawConfig = QDir::cleanPath(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation));
            const auto config = QFileInfo(rawConfig).canonicalFilePath().isEmpty() ? rawConfig : QFileInfo(rawConfig).canonicalFilePath();
            // PREFERENCES_EXPECT_LEGACY=1: migration was blocked, so the existing file stays in use.
            // PREFERENCES_EXPECT_LEGACY=2: neither location is safe, so a session-only file outside the config home is used.
            const auto mode = qEnvironmentVariableIntValue("PREFERENCES_EXPECT_LEGACY");
            const auto location = preferences->property("location").toUrl().toLocalFile();
            if (mode == 2) {require(location=="/dev/null/settings.conf", "session settings must remain in memory");
                require(!AppPaths::preferencesSaveNotice.isEmpty(),"missing settings notice");
                require(named(window,"preferencesSaveNoticeDialog")->property("visible").toBool(),"settings notice hidden");}
            else require(location == QDir(config).filePath(mode ? QStringLiteral("lumi/Deadlock Native.conf") : QStringLiteral("modlock/settings.conf")), "preferences location is incorrect");
            require(preferences->property("showMaturePreviews").toBool() == (qgetenv("PREFERENCES_EXPECT_MATURE") == "1"), "mature-preview preference was lost");
            require(engine->rootContext()->contextProperty("libraryDataRoot").toString() == QDir(QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation)).filePath(qEnvironmentVariable("PREFERENCES_EXPECT_DATA_ROOT", "modlock")), "default library root moved");
            require(QCoreApplication::organizationDomain().isEmpty(), "organisation domain is not neutral");
            require(AppPaths::previewCacheRoot() == QDir(QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation)).filePath("dl-mod-manager"), "preview cache root incorrect");
            require(QString::fromUtf8(qgetenv("QML_DISK_CACHE_PATH")) == QDir(AppPaths::qtCacheRoot()).filePath("qmlcache"), "QML disk cache incorrect");
            require(QStandardPaths::writableLocation(QStandardPaths::CacheLocation) == AppPaths::qtCacheRoot(), "private Qt cache incorrect");
            require(!QFileInfo(AppPaths::qtCacheRoot()).isSymLink(), "Qt cache must not alias old cache");
            require(engine->offlineStoragePath() == QDir(QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation)).filePath("dl-mod-manager/QML/OfflineStorage"), "offline storage incorrect");
            require(preferences->property("colourMode").isValid() && preferences->property("reduceMotion").isValid(), "appearance preferences missing");
            preferences->setProperty("colourMode", "desktop");
            preferences->setProperty("reduceMotion", true);
            auto *tokens = engine->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
            require(tokens && tokens->property("colourMode").toString() == "desktop" && tokens->property("fast").toInt() == 0
                    && tokens->property("normal").toInt() == 0 && tokens->property("panel").toInt() == 0, "appearance bindings failed");
            preferences->setProperty("showMaturePreviews", qgetenv("PREFERENCES_WRITE_MATURE") == "1");
            QEventLoop flush;
            QTimer::singleShot(700, &flush, &QEventLoop::quit);
            flush.exec();
            require(QMetaObject::invokeMethod(preferences, "sync"), "preferences sync failed");
            QSettings persisted(location, QSettings::IniFormat);
            if(mode!=2) require(persisted.value("display/showMaturePreviews").toBool() == preferences->property("showMaturePreviews").toBool(), "preference write used wrong path");
            std::puts("PASS preferences");
            QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            std::fprintf(stderr, "FAIL preferences: %s\n", error.what());
            QCoreApplication::exit(30);
        }
        });
        return;
    }
    if (scenario == "review56-installed" || scenario == "review56-header") {
        QTimer::singleShot(2000, window, [window, model, scenario] {
            QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, scenario == "review56-installed" ? QStringLiteral("modpage") : QStringLiteral("library")));
            QTimer::singleShot(3000, window, [window, model, scenario] {
                try {
                    if (scenario == "review56-installed") {
                        auto *action = named(window, "catalogDetailAction");
                        require(action->property("visible").toBool() && action->property("checked").isValid(), "installed action consumer did not find operable switch");
                        require(action->property("enabled").toBool(), "installed action is disabled");
                        const bool before = action->property("checked").toBool();
                        realClick(window, "catalogDetailAction");
                        QTimer::singleShot(1200, window, [window, before] {
                            try {
                                require(named(window, "catalogDetailAction")->property("checked").toBool() != before, "installed details handler failed to change profile state");
                                const auto path = qEnvironmentVariable("REVIEW56_SCREENSHOT");
                                if (!path.isEmpty()) require(window->grabWindow().save(path), "installed screenshot failed");
                                std::puts("PASS UI installed action identity and handler"); QCoreApplication::exit(0);
                            } catch (const std::exception &error) { std::fprintf(stderr, "FAIL installed: %s\n", error.what()); QCoreApplication::exit(1); }
                        });
                        return;
                    }
                    auto *tokens = qmlEngine(window)->singletonInstance<QObject *>("DeadlockStyle", "Tokens");
                    auto luminance = [](QColor color) {
                        auto linear = [](double value) { return value <= .04045 ? value / 12.92 : std::pow((value + .055) / 1.055, 2.4); };
                        return .2126 * linear(color.redF()) + .7152 * linear(color.greenF()) + .0722 * linear(color.blueF());
                    };
                    auto contrast = [luminance](QColor a, QColor b) { const auto x = luminance(a), y = luminance(b); return (std::max(x,y)+.05)/(std::min(x,y)+.05); };
                    for (const auto *name : {"launchActiveButton", "launchMenuButton"}) {
                        auto *button = named(window, name);
                        qobject_cast<QQuickItem *>(button)->forceActiveFocus(Qt::TabFocusReason);
                        auto *background = qvariant_cast<QQuickItem *>(button->property("background"));
                        auto *outer = qobject_cast<QQuickItem *>(named(background, "playOuterFocusRing"));
                        auto *inner = named(background, "playFillFocusRing");
                        const auto fillRatio = contrast(QQmlProperty(inner, "border.color").read().value<QColor>(), tokens->property("accent").value<QColor>());
                        const auto outerRatio = contrast(tokens->property("focus").value<QColor>(), tokens->property("window").value<QColor>());
                        require(outer->isVisible(), "Play keyboard focus ring hidden");
                        require(outer->x() == -4 && outer->width() == background->width() + 8, "Play ring lost offset gap");
                        require(fillRatio >= 3 && outerRatio >= 3, "Play focus border contrast below 3:1");
                        std::fprintf(stderr, "PLAY FOCUS %s fill %.2f backdrop %.2f\n", name, fillRatio, outerRatio);
                    }
                    auto *chooser = qobject_cast<QQuickItem *>(named(window, "profileChooser"));
                    auto *manage = qobject_cast<QQuickItem *>(named(window, "manageProfilesMenuItem"));
                    require(manage && manage->isEnabled(), "header management menu item unavailable");
                    auto *popup = chooser->property("popup").value<QObject *>();
                    realClick(window, "profileChooser");
                    require(popup && popup->property("visible").toBool(), "profile chooser popup did not open");
                    chooser->forceActiveFocus(Qt::TabFocusReason);
                    realKey(window, Qt::Key_Tab);
                    require(manage->hasActiveFocus(), "Tab from picker did not reach header management");
                    realKey(window, Qt::Key_Tab);
                    require(!popup->property("visible").toBool() && !manage->hasActiveFocus() && !chooser->hasActiveFocus(), "Tab cannot leave profile controls");
                    realClick(window, "profileChooser");
                    require(popup->property("visible").toBool(), "profile chooser popup did not reopen");
                    manage->setEnabled(false);
                    chooser->forceActiveFocus(Qt::TabFocusReason);
                    realKey(window, Qt::Key_Tab);
                    require(!popup->property("visible").toBool() && !manage->hasActiveFocus() && !chooser->hasActiveFocus(), "profile popup trapped Tab on disabled management");
                    manage->setEnabled(true);
                    realClick(window, "profileChooser");
                    QTimer::singleShot(300, window, [window] {
                        auto *chooser = named(window, "profileChooser");
                        auto *list = qobject_cast<QQuickItem *>(findNamed(window, "profilePopupList"));
                        if (!list || list->height() <= 0 || list->property("count").toInt() != chooser->property("count").toInt()) {
                            std::fputs("FAIL profile popup list geometry\n", stderr); QCoreApplication::exit(1); return;
                        }
                        const auto path = qEnvironmentVariable("REVIEW56_SCREENSHOT");
                        if (!path.isEmpty()) window->grabWindow().save(path);
                        std::puts("PASS UI header management Tab and picker exit"); QCoreApplication::exit(0);
                    });
                } catch (const std::exception &error) { std::fprintf(stderr, "FAIL review56: %s\n", error.what()); QCoreApplication::exit(1); }
            });
        });
        return;
    }
    if (scenario == "browse-cards") {
        const int width = qEnvironmentVariableIntValue("UI_REVIEW_WIDTH");
        const int height = qEnvironmentVariableIntValue("UI_REVIEW_HEIGHT");
        if (width && height) window->resize(width, height);
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("browse")));
        QTimer::singleShot(1800, window, [window, model] {
            for (auto *object : objects(window))
                if (object->property("browsePresentationMode").isValid())
                    object->setProperty("browsePresentationMode", qEnvironmentVariable("BROWSE_CARD_MODE", "list"));
            CatalogRow row;
            row.key = {"mod", "42"};
            if (model->nativeMode()) {
                const auto submission = model->modDetails(model->idAt(0)).value("remoteSubmission").toMap();
                const auto key = SubmissionKey::decode(QJsonObject::fromVariantMap(submission));
                if (key) row.key = *key;
            }
            row.identity = row.key.identity();
            row.title = "A very long mod title with a distinctive second line";
            row.author = "Artwork author"; row.state = "ready"; row.masked = true; row.favoriteKnown = true;
            row.metadata = {{"downloads", 1234}, {"likes", 567}, {"thumbnail", "file:///missing-fixture.png"}};
            CatalogRow uninstalled = row;
            uninstalled.key = {"mod", "9999999"}; uninstalled.identity = uninstalled.key.identity();
            uninstalled.title = "Masked preview fixture";
            model->catalog()->model()->replace({row, uninstalled}, {}, {});
            QTimer::singleShot(400, window, [window, model] {
                try {
                    QQuickItem *card = nullptr;
                    for (auto *object : objects(window))
                        if (object->objectName() == "catalogRow" && object->property("heading").toString().startsWith("A very long"))
                            card = qobject_cast<QQuickItem *>(object);
                    require(card, "production CatalogRow missing");
                    auto *host = qvariant_cast<QQuickItem *>(card->property("resultHost"));
                    require(host, "production Browse result host missing");
                    const auto originalWidth = host->width();
                    if (card->property("stacked").toBool()) {
                        host->setWidth(230); // A real minimum cell leaves a 222 px card.
                        QEventLoop settle;
                        QTimer::singleShot(60, &settle, &QEventLoop::quit);
                        settle.exec();
                        require(host->property("cellWidth").toReal() == 230 && card->width() == 222, "minimum Browse cell/card geometry wrong");
                    }
                    auto *favorite = qobject_cast<QQuickItem *>(named(card, "favoriteButton"));
                    auto *preview = qvariant_cast<QQuickItem *>(card->property("preview"));
                    require(card->property("heading").toString().startsWith("A very long"), "CatalogRow model title binding failed");
                    require(preview && preview->property("hidden").toBool(), "production masked preview missing");
                    require(favorite && favorite->isVisible(), "production favorite missing");
                    const QRectF art(preview->mapToItem(card, QPointF()), preview->size());
                    const QRectF heart(favorite->mapToItem(card, QPointF()), favorite->size());
                    require(!art.intersects(heart), "favorite overlaps masked preview");
                    require(heart.left() >= 0 && heart.right() <= card->width(), "favorite outside card");
                    auto *stats = qobject_cast<QQuickItem *>(named(card, "catalogStatsRow"));
                    const QRectF statsRect(stats->mapToItem(card, QPointF()), stats->size());
                    require(!statsRect.intersects(heart), "stats overlap favorite action");
                    require(statsRect.left() >= 0 && statsRect.right() <= card->width(), "stats outside card");
                    int statsLabels = 0;
                    for (auto *object : objects(stats)) {
                        if (!object->property("text").isValid() || !object->property("truncated").isValid()) continue;
                        ++statsLabels;
                        require(!object->property("truncated").toBool(), "production stats label truncated");
                    }
                    require(statsLabels > 0, "production stats labels missing");
                    if (card->property("poster").toBool()) {
                        require(statsLabels==2, "Grid Browse footer omits existing like count");
                        auto *accessible=QAccessible::queryAccessibleInterface(stats);
                        require(accessible && accessible->text(QAccessible::Name).contains("567 likes"), "Grid like count accessible name missing");
                    }
                    auto *download = qobject_cast<QQuickItem *>(named(card, "installButton"));
                    if (download->isVisible()) {
                        const QRectF buttonRect(download->mapToItem(card, QPointF()), download->size());
                        require(buttonRect.left() >= 0 && buttonRect.right() <= card->width(), "Download outside card");
                        require(!buttonRect.intersects(heart) && !buttonRect.intersects(statsRect), "Download overlaps footer");
                        auto *label = qvariant_cast<QQuickItem *>(download->property("contentItem"));
                        require(label, "Download content missing");
                        QObject *text = nullptr;
                        for (auto *object : objects(label))
                            if (object->property("text").isValid() && object->property("truncated").isValid()) text = object;
                        require(text && text->property("truncated").isValid(), "Download text truncation property missing");
                        require(!text->property("truncated").toBool(), "Download label truncated");
                        require(download->width() >= download->implicitWidth(), "Download below implicit width");
                    }
                    require(!named(card, "installButton")->property("highlighted").toBool(), "Download must remain secondary");
                    const auto submission = card->property("submission").toMap();
                    const auto key = *SubmissionKey::decode(QJsonObject::fromVariantMap(submission));
                    if (model->nativeMode()) {
                        const auto action = model->catalog()->libraryAction(submission);
                        auto *installed = named(card, "catalogInstalledSwitch");
                        require(action.value("action") == "enable" || action.value("action") == "disable", "fixture did not bind installed CatalogRow");
                        require(installed->property("visible").toBool() && installed->property("checked").toBool() == (action.value("action") == "disable"), "installed CatalogRow action binding failed");
                        require(!named(card, "installButton")->property("visible").toBool(), "installed row retained Download");
                    }
                    favorite->setProperty("checked", true);
                    QMetaObject::invokeMethod(favorite, "clicked");
                    require(!favorite->property("checked").toBool(), "favorite click handler lost model binding");
                    model->catalog()->model()->favorite(key, true, "fixture");
                    require(favorite->property("checked").toBool(), "favorite model binding stale");
                    model->catalog()->model()->favorite(key, false, "fixture2");
                    require(!favorite->property("checked").toBool(), "favorite binding lost after update");
                    const auto minimumCapture = qEnvironmentVariable("BROWSE_CARD_MIN_SCREENSHOT");
                    if (card->property("stacked").toBool() && !minimumCapture.isEmpty()) {
                        const auto image = window->grabWindow();
                        const auto origin = card->mapToScene(QPointF());
                        const auto scale = image.devicePixelRatio();
                        require(image.copy(QRectF(origin * scale, card->size() * scale).toAlignedRect()).save(minimumCapture), "minimum Browse capture failed");
                    }
                    host->setWidth(originalWidth);
                    QEventLoop settle;
                    QTimer::singleShot(60, &settle, &QEventLoop::quit);
                    settle.exec();
                    const auto capture = qEnvironmentVariable("BROWSE_CARD_SCREENSHOT");
                    if (!capture.isEmpty()) require(window->grabWindow().save(capture), "Browse capture failed");
                    std::puts("PASS UI Browse production CatalogRow bindings and masked actions"); QCoreApplication::exit(0);
                } catch (const std::exception &error) {
                    std::fprintf(stderr, "FAIL Browse cards: %s\n", error.what()); QCoreApplication::exit(1);
                }
            });
        });
        return;
    }
    if (scenario == "browse-filters") {
        const int width = qEnvironmentVariableIntValue("UI_REVIEW_WIDTH");
        const int height = qEnvironmentVariableIntValue("UI_REVIEW_HEIGHT");
        if (width && height) window->resize(width, height);
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("browse")));
        QTimer::singleShot(1800, window, [window, model] {
            try {
                model->catalog()->updateQuery({{"search", "fixture"}, {"categories", QStringList{"Test"}}, {"heroes", QStringList{"Abrams"}}, {"excludeFilters", true}});
                const auto before = model->catalog()->queryState();
                realClick(window, "browseModsButton");
                require(named(window, "browseModsButton")->property("checked").toBool(), "selected Mods segment became unchecked");
                require(model->catalog()->queryState() == before, "selected Mods segment cleared filters");
                realClick(window, "browseSkinsButton");
                require(model->catalog()->queryState().value("heroes").toStringList() == QStringList{"Abrams"}, "Mods to skins lost compatible hero");
                realClick(window, "browseSkinsButton");
                require(named(window, "browseSkinsButton")->property("checked").toBool(), "selected skins segment became unchecked");
                realClick(window, "browseModsButton");
                require(model->catalog()->queryState().value("heroes").toStringList() == QStringList{"Abrams"}, "skins to Mods lost Abrams");
                realClick(window, "browseCollectionsButton");
                const auto after = model->catalog()->queryState();
                require(after.value("search") == "fixture" && after.value("excludeFilters").toBool(), "kind switch cleared search/exclusion");
                require(after.value("categories").toStringList().isEmpty() && after.value("heroes").toStringList().isEmpty(), "kind switch kept incompatible filters");
                model->catalog()->updateQuery({{"kind", "mod"}, {"search", ""}, {"excludeFilters", false}, {"favoritesOnly", true}, {"hideNsfw", true}});
                named(window, "browseFilterFlow")->setProperty("enabled", true);
                auto *more = named(window, "browseMoreFilters");
                require(more->property("count").toInt() == 2, "active secondary filters are not counted");
                const auto *filtersAccessible = QAccessible::queryAccessibleInterface(more);
                require(filtersAccessible && filtersAccessible->text(QAccessible::Name).contains("2"), "filter count is missing from the accessible name");
                realClick(window, "browseMoreFilters");
                QTimer::singleShot(300, window, [window, model] {
                    try {
                        require(named(window, "browseMatureResults")->property("checked").toBool(), "mature result filter is missing");
                        require(named(window, "browseFavoritesFilter")->property("checked").toBool(), "favorites filter is missing");
                        named(window, "browseFilterFlow")->setProperty("enabled", true);
                        click(window, "browseResetFilters");
                        const auto query = model->catalog()->queryState();
                        require(!query.value("favoritesOnly").toBool() && query.value("hideNsfw").toBool(), "reset did not restore secondary defaults");
                        std::puts("PASS UI Browse secondary filters"); QCoreApplication::exit(0);
                    } catch (const std::exception &error) {
                        std::fprintf(stderr, "FAIL Browse filters: %s\n", error.what()); QCoreApplication::exit(1);
                    }
                });
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL Browse filters: %s\n", error.what()); QCoreApplication::exit(1);
            }
        });
        return;
    }
    if (scenario == "browse-hero-picker") {
        const int width = qEnvironmentVariableIntValue("UI_REVIEW_WIDTH");
        const int height = qEnvironmentVariableIntValue("UI_REVIEW_HEIGHT");
        window->resize(width ? width : 1200, height ? height : 780);
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("browse")));
        QTimer::singleShot(2000, window, [window, model] {
            try {
                named(window, "browseFilterFlow")->setProperty("enabled", true);
                auto *hero = qobject_cast<QQuickItem *>(named(window, "browseHeroFilter"));
                require(hero, "hero picker missing");
                hero->setVisible(true);
                auto *popup = hero->property("popup").value<QObject *>();
                require(popup && QMetaObject::invokeMethod(popup, "open"), "hero popup could not open");
                QTimer::singleShot(300, window, [window, model, hero, popup] {
                    try {
                        capture(window, "hero-popup");
                        if (qEnvironmentVariableIsSet("UI_HERO_CAPTURE_ONLY")) { QCoreApplication::exit(0); return; }
                        require(popup->property("height").toDouble() <= 340, "hero popup shows more than about ten rows");
                        require(hero->property("editable").toBool(), "hero picker does not accept typing");
                        QMetaObject::invokeMethod(popup, "close");
                        realClickItem(window, hero, QPointF(15, hero->height() / 2));
                        realKey(window, Qt::Key_A, Qt::ControlModifier);
                        realKey(window, 0, Qt::NoModifier, QStringLiteral("Vin"));
                        realKey(window, Qt::Key_Return);
                        require(model->catalog()->queryState().value("heroes").toStringList() == QStringList{QStringLiteral("Vindicta")}, "typing a hero and pressing Enter did not select it");
                        require(QMetaObject::invokeMethod(popup, "open"), "hero popup could not reopen");
                        require(hero->property("currentText").toString() == "Vindicta", "reopening lost the selected hero");
                        std::puts("PASS UI searchable hero picker"); QCoreApplication::exit(0);
                    } catch (const std::exception &error) {
                        std::fprintf(stderr, "FAIL hero picker: %s\n", error.what()); QCoreApplication::exit(1);
                    }
                });
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL hero picker: %s\n", error.what()); QCoreApplication::exit(1);
            }
        });
        return;
    }
    if (scenario == "crosshair-narrow") {
        window->resize(720, 520);
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("crosshair")));
        QTimer::singleShot(4000, window, [window] {
            try {
                auto *panel = window->findChild<QQuickItem *>("crosshairPanel");
                require(panel && panel->property("zoom").toInt() == 1, "crosshair preview must start at 1x");
                require(window->findChild<QObject *>("crosshairHero"), "crosshair hero selector missing");
                require(window->findChild<QObject *>("crosshairZoom"), "crosshair zoom selector missing");
                auto *config = qvariant_cast<GameConfigController *>(panel->property("controller"));
                require(config, "crosshair controller missing");
                if (config->available()) {
                    panel->setProperty("hero", "hero_inferno");
                    const QVariant effective = panel->property("v");
                    const auto values = effective.metaType().id() == qMetaTypeId<QJSValue>()
                        ? effective.value<QJSValue>().toVariant().toMap() : effective.toMap();
                    require(values.value("dotSize").toInt() == 5, "preview did not resolve hero dot size");
                    require(values.value("dotOutlineWidth").toInt() == 2, "preview did not resolve hero outline");
                    panel->setProperty("hero", "");
                }
                require(config->setValue("overrides.hero_inferno.themed", true), "per-hero setting rejected");
                require(config->values().value("overrides").toMap().value("hero_inferno").toMap().value("themed").toBool(), "per-hero setting did not persist in model");
                require(!config->values().value("themed").toBool(), "per-hero edit changed base");
                config->clearHeroOverride("hero_inferno");
                require(!config->values().value("overrides").toMap().contains("hero_inferno"), "per-hero reset did not inherit base");
                for (auto *object : objects(window)) {
                    if (!object->property("text").toString().startsWith("Changes the crosshair for all heroes")) continue;
                    auto *label = qobject_cast<QQuickItem *>(object);
                    require(label && label->mapToScene(QPointF()).x() + label->width() <= window->width() + 1, "crosshair controls overflow the window");
                    const QString capture = qEnvironmentVariable("CROSSHAIR_CAPTURE_PATH");
                    if (!capture.isEmpty()) require(window->grabWindow().save(capture), "crosshair screenshot failed");
                    std::puts("PASS UI narrow crosshair"); QCoreApplication::exit(0); return;
                }
                throw std::runtime_error("crosshair introduction missing");
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL narrow crosshair: %s\n", error.what()); QCoreApplication::exit(1);
            }
        });
        return;
    }
    if (scenario == "item121-loading") {
        auto *timer = new QTimer(window);
        timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer] {
            if (model->loading()) return;
            timer->stop(); model->refresh();
            QTimer::singleShot(100, window, [window, model] {
                try {
                    require(model->loading(), "loading fixture did not hold refresh");
                    require(named(window, "homePage")->property("visible").toBool(), "Home page hidden while loading");
                    require(window->grabWindow().save(qEnvironmentVariable("UI_LOADING_CAPTURE")), "loading capture failed");
                    std::puts("PASS UI Home during refresh"); QCoreApplication::exit(0);
                } catch (const std::exception &e) { std::fprintf(stderr, "FAIL loading: %s\n", e.what()); QCoreApplication::exit(1); }
            });
        }); timer->start(); return;
    }
    if (scenario == "item121-tiles") {
        QQmlComponent component(qmlEngine(window));
        component.setData(R"(import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import Deadlock.Native
Control {
    width: 1200; height: 500
    property int requestedSize: 32
    font.pointSize: requestedSize
    background: Rectangle { color: Theme.window }
    GridLayout {
        anchors.fill: parent; anchors.margins: 24; columns: 4; columnSpacing: 12
        Repeater {
            model: ["Profile", "Performance", "Crosshair", "Autoexec"]
            SetupTile { required property string modelData; objectName: modelData; title: modelData; value: "Saved"; iconName: "edit" }
        }
    }
})", QUrl("qrc:/qt/qml/Deadlock/Native/qml/TileProbe.qml"));
        auto *tiles = qobject_cast<QQuickItem *>(component.create());
        require(tiles, qPrintable(component.errorString()));
        tiles->setParentItem(window->contentItem()); tiles->setZ(10000);
        window->resize(1200, 500);
        if (qEnvironmentVariableIntValue("UI_TILE_FONT") > 0) tiles->setProperty("requestedSize", qEnvironmentVariableIntValue("UI_TILE_FONT"));
        QTimer::singleShot(500, window, [window, tiles] {
            try {
                for (const auto &name : {"Profile", "Performance", "Crosshair", "Autoexec"}) {
                    auto *tile = named(tiles, name);
                    require(tile->property("implicitHeight").toReal() >= tile->property("implicitContentHeight").toReal() + tile->property("topPadding").toReal() + tile->property("bottomPadding").toReal(), "large font overflows setup tile");
                }
                const auto path = qEnvironmentVariable("UI_TILE_CAPTURE");
                if (!path.isEmpty()) require(window->grabWindow().save(path), "tile capture failed");
                std::puts("PASS UI production SetupTiles at requested large font"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { std::fprintf(stderr, "FAIL tiles: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        return;
    }
    if (scenario == "fix789-installed" || scenario == "fix789-profiles" || scenario == "fix789-inherited" || scenario == "fix789-explicit-none" || scenario == "fix789-conflicts") {
        auto *timer = new QTimer(window); timer->setInterval(150);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, scenario, stage=0, ticks=0]() mutable {
            try {
                require(++ticks < 80, "fix789 probe timed out");
                if (model->loading() || !model->nativeMode() || model->busy()) return;
                if (stage == 0) {
                    if (scenario == "fix789-inherited" || scenario == "fix789-explicit-none") {
                        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QString("tweaks")));
                    } else if (scenario == "fix789-installed") {
                        if (model->installedModsBusy()) return;
                        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QString("installed")));
                    } else if (scenario == "fix789-profiles") {
                        openProfilesViaChooser(window);
                    } else {
                        if (!model->canPreviewApply()) return;
                        require(model->previewApply(model->gamePath()), "conflict preview rejected");
                    }
                    ++stage; return;
                }
                if (scenario == "fix789-inherited" || scenario == "fix789-explicit-none") {
                    if (!model->performance()->available()) return;
                    realClick(window, "performancePreset_");
                    auto *choose = named(window, "choosePerformancePreset_");
                    require(choose->property("visible").toBool() && choose->property("enabled").toBool(), "baseline per-card None action missing");
                    require(!model->performance()->select("none"), "invalid literal none admitted");
                    click(window, "choosePerformancePreset_");
                } else if (scenario == "fix789-installed") {
                    if (model->installedModsBusy()) return;
                    require(model->installedMods().size() == 1, "unidentified fixture row missing");
                    int notices=0;
                    for (auto *o : objects(window)) {
                        const auto text=o->property("text").toString();
                        if (o->property("visible").toBool() && text.startsWith("Not identified yet")) ++notices;
                    }
                    require(notices == 1, "unidentified notice missing or duplicated");
                } else if (scenario == "fix789-profiles") {
                    auto *popup = profilePopup(window);
                    require(popup->property("targetId") == model->viewedProfileId(), "dialog does not target viewed profile");
                    require(popup->property("contextCurrent").toBool(), "viewed profile revision rejected");
                    require(named(window, "useProfileButton")->property("visible").toBool(), "dialog-wide Use action hidden");
                    popup->setProperty("targetRevision", "stale");
                    require(!popup->property("contextCurrent").toBool() && !named(window, "shareProfileButton")->property("enabled").toBool(), "stale revision allows sharing");
                    popup->setProperty("targetRevision", model->revision());
                } else {
                    if (model->applyReview().isEmpty()) return;
                    for (auto *o : objects(window)) {
                        if (o->property("label").toString().startsWith("Conflicting files")) o->setProperty("checked", true);
                    }
                    bool full=false, partial=false;
                    for (auto *o : objects(window)) {
                        full |= o->property("text").toString() == "Mod A wins over Mod B for materials/test.vtex_c (lower package order).";
                        partial |= o->property("text").toString().contains("additional conflicts are not shown");
                    }
                    require(full && partial, "conflict warning lost a mod/path or partial-list notice");
                }
                timer->stop(); QTimer::singleShot(350, window, [window] { capture(window, "fix789"); std::puts("PASS UI fix789 regression"); QCoreApplication::exit(0); });
            } catch (const std::exception &e) { timer->stop(); std::fprintf(stderr, "FAIL fix789: %s\n", e.what()); QCoreApplication::exit(1); }
        }); timer->start(); return;
    }
    if (scenario == "fix789-options") {
        window->resize(720, 640);
        QQmlComponent component(qmlEngine(window), QUrl("qrc:/qt/qml/Deadlock/Native/qml/OptionsDialog.qml"));
        auto *dialog = component.createWithInitialProperties({{"parent", QVariant::fromValue(window->contentItem())}, {"controller", QVariant::fromValue(model->options())}});
        require(dialog, qPrintable(component.errorString()));
        QMetaObject::invokeMethod(dialog, "open");
        QTimer::singleShot(700, window, [window, dialog] {
            try {
                auto *save = qobject_cast<QQuickItem *>(named(dialog, "saveSelectionButton"));
                require(window->width() == 720 && save && save->isVisible(), "minimum-width Save missing");
                auto *footer = qobject_cast<QQuickItem *>(dialog->property("footer").value<QObject *>());
                require(footer, "Options footer missing");
                for (auto *object : objects(footer)) {
                    auto *item = qobject_cast<QQuickItem *>(object);
                    if (!item || !item->isVisible() || !item->inherits("QQuickAbstractButton")) continue;
                    const auto rect = item->mapRectToItem(footer, QRectF(0, 0, item->width(), item->height()));
                    require(rect.left() >= 15 && rect.right() <= footer->width() - 15, "Options action outside padded footer at 720px");
                }
                capture(window, "options-720");
                std::puts("PASS UI production Options footer at 720px"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { std::fprintf(stderr, "FAIL Options: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        return;
    }
    if (scenario == "item121-dialogs") {
        QQmlComponent component(qmlEngine(window), QUrl("qrc:/qt/qml/Deadlock/Native/qml/NativeReviewDialog.qml"));
        auto *dialog = component.createWithInitialProperties({{"parent", QVariant::fromValue(window->contentItem())}, {"library", QVariant::fromValue(model)}, {"workflow", "apply-review"}, {"sourcePath", ""}, {"gamePath", "/fixture"}});
        require(dialog, qPrintable(component.errorString()));
        QTimer::singleShot(500, window, [window, dialog] {
            try {
                auto *button = qobject_cast<QQuickItem *>(named(dialog, "applyCommitButton"));
                require(button, "Apply button missing");
                const auto right = button->mapToScene(QPointF(button->width(), 0)).x();
                std::fprintf(stderr, "Apply geometry: right=%g x=%g width=%g\n", right, dialog->property("x").toReal(), dialog->property("width").toReal());
                require(right <= dialog->property("x").toReal() + dialog->property("width").toReal() - 15, "Apply button touches right edge");
                QMetaObject::invokeMethod(dialog, "close");
                dialog->deleteLater();
                std::puts("PASS UI production Apply footer padding"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { std::fprintf(stderr, "FAIL dialog: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        return;
    }
    if (scenario == "performance-details") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("tweaks")));
        QTimer::singleShot(4000, window, [window] {
            try {
                realClick(window, "performancePreset_balanced");
                require(!named(window, "choosePerformancePreset_balanced")->property("visible").toBool(), "active preset offers a mutation");
                require(named(window, "choosePerformancePreset_")->property("visible").toBool(), "baseline None action missing for selected preset");
                auto *settings = named(window, "performanceSettingsList");
                auto *skipped = named(window, "performanceSkippedList");
                require(settings->property("count").toInt() == 0 && skipped->property("count").toInt() == 0, "details should start collapsed");
                realClick(window, "performanceSettingsToggle");
                realClick(window, "performanceSkippedToggle");
                require(settings->property("count").toInt() == 1 && skipped->property("count").toInt() == 1, "details did not expand");
                window->resize(720, 520);
                QTimer::singleShot(300, window, [window] {
                    try {
                        auto *summary = qobject_cast<QQuickItem *>(named(window, "performanceSummary"));
                        require(summary && summary->mapToScene(QPointF()).x() + summary->width() <= window->width() + 1, "performance summary overflows the window");
                        std::puts("PASS UI performance details and narrow layout");
                        QCoreApplication::exit(0);
                    } catch (const std::exception &error) {
                        std::fprintf(stderr, "FAIL performance details: %s\n", error.what()); QCoreApplication::exit(1);
                    }
                });
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL performance details: %s\n", error.what()); QCoreApplication::exit(1);
            }
        });
        return;
    }
    if (scenario == "autoexec-actions") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("autoexec")));
        QTimer::singleShot(4000, window, [window] {
            try {
                auto *editor = qobject_cast<QQuickItem *>(named(window, "autoexecEditor"));
                auto *save = qobject_cast<QQuickItem *>(named(window, "autoexecSave"));
                require(editor && save, "autoexec editor or save button is not an item");
                const auto editorBottom = editor->mapToScene(QPointF(0, editor->height())).y();
                const auto saveTop = save->mapToScene(QPointF()).y();
                require(saveTop >= editorBottom && saveTop - editorBottom < 100, "save action is not close to the editor");
                std::puts("PASS autoexec actions");
                QCoreApplication::exit(0);
            } catch (const std::exception &error) {
                std::fprintf(stderr, "FAIL autoexec actions: %s\n", error.what());
                QCoreApplication::exit(1);
            }
        });
        return;
    }
    if (scenario == "review-capture") {
        const auto page = qEnvironmentVariable("UI_REVIEW_PAGE", "select");
        for (const auto &part : page.split(','))
            QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, part));
        window->resize(qEnvironmentVariableIntValue("UI_REVIEW_WIDTH"), qEnvironmentVariableIntValue("UI_REVIEW_HEIGHT"));
        if (qEnvironmentVariableIsSet("UI_REVIEW_OPEN_DETAILS"))
            QTimer::singleShot(2000, window, [window] { click(window, "rowTitle"); });
        if (qEnvironmentVariableIsSet("UI_REVIEW_BROWSE_LIST")) {
            QTimer::singleShot(2000, window, [window] {
                for (auto *object : objects(window)) {
                    auto *item = qobject_cast<QQuickItem *>(object);
                    if (item && item->isVisible() && object->property("text").toString() == "List") {
                        realClickItem(window, item); return;
                    }
                }
            });
        }
        if (qEnvironmentVariableIsSet("UI_REVIEW_MORE_FILTERS"))
            QTimer::singleShot(2000, window, [window] { realClick(window, "browseMoreFilters"); });
        if (qEnvironmentVariableIsSet("UI_REVIEW_SCROLL_TO")) {
            QTimer::singleShot(2000, window, [window] {
                auto *target = qobject_cast<QQuickItem *>(named(window, qgetenv("UI_REVIEW_SCROLL_TO").constData()));
                for (auto *item = target; item; item = item->parentItem()) {
                    if (item->metaObject()->indexOfProperty("contentY") < 0) continue;
                    item->setProperty("contentY", item->property("contentY").toReal() + target->mapToItem(item, QPointF()).y() - 100);
                    break;
                }
            });
        }
        if (qEnvironmentVariableIsSet("UI_REVIEW_PARTIAL_REPAIR")) {
            QTimer::singleShot(2000, window, [window] {
                for (auto *object : objects(window)) {
                    if (object->objectName() == "modRow" && object->property("visible").toBool())
                        object->setProperty("modHealthStatus", "partlyFixed");
                    if (object->objectName() == "modDetails") {
                        auto mod = object->property("mod").toMap();
                        mod.insert("health", QVariantMap{{"status", "partlyFixed"}, {"fixSymptoms", QStringList{"cameraAim"}}, {"remainingSymptoms", QStringList{"animations"}}});
                        object->setProperty("mod", mod);
                    }
                }
            });
        }
        QTimer::singleShot(5000, window, [window, model] {
            const auto expected = qEnvironmentVariable("UI_REVIEW_EXPECT_HOME");
            if (!expected.isEmpty()) {
                auto *callout = named(window, "homeStatusCallout");
                const bool expectNotice = expected == "Apply the selected profile" || expected == "Choose files for the selected profile"
                    || expected == "Choose files for some mods before playing" || (expected == "Your game setup needs checking" && model->error().isEmpty())
                    || (expected == "Another mod manager or a game update changed the game files" && !named(window,"statusNotice")->property("externalChangeShown").toBool());
                require(callout->property("visible").toBool() == expectNotice, "Home notice visibility does not match fixture facts");
                if (expectNotice)
                    require(callout->property("title").toString() == expected, "Home notice wording does not match fixture facts");
            }
            capture(window, "review");
            std::puts("PASS UI review capture");
            QCoreApplication::exit(0);
        });
        return;
    }
    if (scenario != "home" && scenario != "home-settings-summary" && scenario != "home-catalog")
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("library")));
    if (scenario == "filter-overflow") {
        auto *timer = new QTimer(window); timer->setInterval(250);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, stage=0, ticks=0]() mutable {
            try {
                require(++ticks < 150, "filter overflow check timed out");
                if (model->loading()) return;
                if (stage == 0) {
                    window->resize(720, 520);
                    named(window,"updateAllButton")->setProperty("visible", true);
                    ++stage; return;
                }
                auto checkFlow = [window](const char *name, int minimum) {
                    auto *flow=qobject_cast<QQuickItem *>(named(window,name));
                    require(flow && flow->isVisible() && flow->width()>0, "filter flow is unavailable");
                    int count=0;
                    for (auto *item : flow->childItems()) {
                        if (!item->isVisible() || item->width()<=0 || item->height()<=0) continue;
                        ++count;
                        const auto inFlow=item->mapRectToItem(flow,QRectF(0,0,item->width(),item->height()));
                        const auto inWindow=item->mapRectToScene(QRectF(0,0,item->width(),item->height()));
                        require(inFlow.left()>=-1 && inFlow.right()<=flow->width()+1, "filter extends beyond its wrapping row");
                        require(inWindow.left()>=-1 && inWindow.right()<=window->width()+1 && inWindow.top()>=-1 && inWindow.bottom()<=window->height()+1, "filter is clipped at 720x520");
                    }
                    require(count>=minimum, "not all filter controls were checked");
                };
                if (stage == 1) {
                    require(window->width()==720 && window->height()==520, "filter check did not use 720x520");
                    for (const auto *name : {"categoryChooser","heroChooser","enabledFilter","needsAttentionFilter","updatesFilter","updateAllButton"})
                        require(named(window,name)->property("visible").toBool(), "Library filter is hidden");
                    for (const auto *name : {"updatesFilter", "needsAttentionFilter"}) {
                        auto *chip = named(window, name);
                        auto *accessible = QAccessible::queryAccessibleInterface(chip);
                        require(chip->property("count").toInt() == -1 && accessible
                            && accessible->text(QAccessible::Name) == chip->property("text").toString(),
                            "plain filter chip exposes a count");
                    }
                    checkFlow("libraryFilterFlow",6);
                    QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QStringLiteral("browse")));
                    ++stage; return;
                }
                if (stage == 2) {
                    auto *flow=findNamed(window,"browseFilterFlow");
                    if (!flow || !flow->property("visible").toBool()) return;
                    flow->setProperty("enabled",true);
                    named(window,"browseResetFilters")->setProperty("visible",true);
                    ++stage; return;
                }
                if (stage == 3) {
                    checkFlow("browseFilterFlow",5);
                    realClick(window,"browseMoreFilters"); ++stage; return;
                }
                auto *favorite=qobject_cast<QQuickItem *>(findNamed(window,"browseFavoritesFilter"));
                if (!favorite || !favorite->isVisible()) return;
                auto *grid=favorite->parentItem();
                require(grid && grid->width()>0, "Browse filter popup is unavailable");
                int count=0;
                for (auto *item : grid->childItems()) {
                    if (!item->isVisible() || item->width()<=0 || item->height()<=0) continue;
                    const auto rect=item->mapRectToItem(grid,QRectF(0,0,item->width(),item->height()));
                    require(rect.left()>=-1 && rect.right()<=grid->width()+1, "popup filter needs horizontal scrolling");
                    ++count;
                }
                require(count>=8, "Browse popup filters were not checked");
                timer->stop(); std::puts("PASS UI wrapping filters and popup at 720x520"); QCoreApplication::exit(0); return;
            } catch (const std::exception &e) { timer->stop(); std::fprintf(stderr,"FAIL filter overflow: %s\n",e.what()); QCoreApplication::exit(1); }
        }); timer->start(); return;
    }
    if (scenario == "storage-refusal") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("library")));
        auto *timer = new QTimer(window); timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, timer, ticks=0]() mutable {
            try {
                require(++ticks < 150, "newer-library refusal timed out");
                auto *label = named(window, "errorLabel");
                if (!label || label->property("text").toString().isEmpty()) return;
                require(label->property("text").toString() == "This library was changed by a newer Modlock. Update the app to open it.", ("newer-library message is not clear: " + label->property("text").toString()).toStdString().c_str());
                require(label->property("visible").toBool(), "newer-library banner is hidden");
                timer->stop(); std::puts("PASS UI older-helper refusal banner"); QCoreApplication::exit(0);
            } catch (const std::exception &e) {
                timer->stop(); std::fprintf(stderr, "FAIL storage-refusal: %s\n", e.what()); QCoreApplication::exit(1);
            }
        }); timer->start(); return;
    }
    if (scenario == "storage-banner") {
        QQmlComponent component(qmlEngine(window),QUrl("qrc:/qt/qml/Deadlock/Native/qml/StorageDialog.qml"));
        StorageController controller;
        std::unique_ptr<QObject> dialog(component.createWithInitialProperties({{"library",QVariant::fromValue(model)},{"controller",QVariant::fromValue(&controller)}}));
        require(dialog!=nullptr,qPrintable(component.errorString()));
        auto *banner = named(dialog.get(), "storageInterruptedBanner");
        require(banner, "interrupted storage banner missing");
        QVariant beforeSwitch;
        require(QMetaObject::invokeMethod(dialog.get(), "interruptionText", Q_RETURN_ARG(QVariant, beforeSwitch), Q_ARG(QVariant, true)), "interruption text unavailable");
        require(beforeSwitch.toString().contains("may contain files you changed after the move stopped — check it before deleting."), "interrupted storage banner lacks caution");
        const auto afterSwitch=banner->property("text").toString();
        require(afterSwitch.contains("Resume finishes removing verified old copies.") && afterSwitch.contains("Cancel is no longer available.") && !afterSwitch.contains("fresh copy"), "post-switch banner shows pre-switch advice");
        std::puts("PASS UI interrupted storage caution");
        QTimer::singleShot(0, [] { QCoreApplication::exit(0); }); return;
    }
    if (scenario == "settings-storage-available" || scenario == "settings-storage-offline") {
        auto *timer = new QTimer(window);
        timer->setInterval(50);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, scenario, opened=false, ticks=0]() mutable {
            try {
                require(++ticks < 160, "Settings did not display the expected storage status");
                if (!model->nativeMode() || !model->helperReady() || model->busy() || model->loading()) return;
                if (!opened) {
                    click(window, "settingsTab");
                    opened = true;
                    bool checking = false;
                    for (auto *item : objects(named(window, "settingsPage")))
                        checking |= item->property("visible").toBool() && item->property("text").toString() == "Checking storage…";
                    require(checking, "Opening storage Settings did not show the in-progress check");
                }
                for (auto *item : objects(named(window, "settingsPage"))) {
                    if (!item->property("visible").toBool()) continue;
                    const auto text = item->property("text").toString();
                    if (scenario == "settings-storage-offline" ? text.contains("Reconnect") && text.contains("drive")
                            : text.contains("Folder size: 0.00 GiB") && text.contains("Free on drive:")) {
                        timer->stop(); std::puts("PASS UI storage status"); QCoreApplication::exit(0); return;
                    }
                }
            } catch (const std::exception &e) {
                timer->stop(); std::fprintf(stderr, "FAIL UI storage status: %s\n", e.what()); QCoreApplication::exit(1);
            }
        });
        timer->start(); return;
    }
    if (scenario == "storage") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("settings")));
        auto *timer = new QTimer(window); timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, timer, ticks=0, stage=0]() mutable {
            try {
                require(++ticks < 400, "storage review timed out");
                auto *controller=qobject_cast<StorageController *>(named(window,"settingsStorageController"));
                require(controller,"storage controller missing");
                if(controller->busy())return;
                require(controller->error().isEmpty(),qPrintable(controller->error()));
                if(stage==0){
                    if(!controller->result().contains("current"))return;
                    require(named(window,"storageLocationRow"),"storage row missing");
                    capture(window,"storage-settings");
                    QMetaObject::invokeMethod(named(window,"changeStorageLocation"),"clicked");
                    stage=1;return;
                }
                if(stage==1){
                    const auto folder=qEnvironmentVariable("STORAGE_REVIEW_DESTINATION");
                    require(!folder.isEmpty(),"synthetic destination is required");
                    controller->review(folder);stage=2;return;
                }
                require(controller->result().contains("token"),"storage review token missing");
                require(named(window,"storageCancel"),"storage cancel action missing");
                require(named(window,"storageCancel")->property("text").toString()=="Cancel move","storage cancel wording changed");
                require(named(window,"storageMoveConfirm")->property("enabled").toBool(),"review cannot be confirmed");
                require(named(window,"storageMoveConfirm")->property("highlighted").toBool(),"move action is not primary");
                capture(window,"storage-review");
                timer->stop();std::puts("PASS UI storage row and reviewed move dialog");QCoreApplication::exit(0);
            }catch(const std::exception &e){timer->stop();std::fprintf(stderr,"FAIL storage: %s\n",e.what());QCoreApplication::exit(1);}
        });timer->start();return;
    }
    if (scenario == "maintenance" || scenario == "maintenance-details") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("maintenance")));
        auto *timer = new QTimer(window); timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, scenario, ticks=0, cleaned=false]() mutable {
            try {
                require(++ticks < 200, "maintenance dialog timed out");
                if (model->loading()) return;
                QObject *controller=nullptr;
                for(auto *o:objects(window))if(o->objectName()=="maintenanceController")controller=o;
                if(!controller || controller->property("busy").toBool())return;
                require(controller->property("error").toString().isEmpty(), "maintenance scan failed");
                const auto result=controller->property("result").toMap();
                if(result.value("token").toString().isEmpty())return;
                const QString caution=QStringLiteral("Unfinished copy left in /fixture/copy (1 MB). It may contain files you changed after the move stopped — check it before deleting.");
                QVariant label;
                require(QMetaObject::invokeMethod(named(window,"maintenanceDialog"), "itemName", Q_RETURN_ARG(QVariant,label), Q_ARG(QVariant,QVariant(QVariantMap{{"kind","storageCopy"},{"path","/fixture/copy"},{"bytes",123},{"reason",QStringLiteral("Kept. ")+caution}}))), "maintenance formatter unavailable");
                require(label.toString().contains(caution), "Maintenance hides the storage-copy caution");
                require(!named(window,"maintenanceSummary")->property("text").toString().isEmpty(), "maintenance summary missing");
                require(named(window,"maintenanceClean")->property("enabled").toBool()==(result.value("canClean").toBool()||result.value("resume").toBool()), "cleanup review guard mismatch");
                if (scenario == "maintenance-details") {
                    realClick(window, "maintenanceKept"); realClick(window, "maintenanceDetails");
                    require(named(window,"maintenanceKept")->property("checked").toBool(), "kept folders did not expand");
                    require(named(window,"maintenanceDetails")->property("checked").toBool(), "paths did not expand");
                    auto *scroll = named(window,"maintenanceScroll");
                    require(scroll->property("contentWidth").toDouble() <= scroll->property("availableWidth").toDouble(), "maintenance paths overflow horizontally");
                    timer->stop(); QTimer::singleShot(300, window, [window] {capture(window,"maintenance-details"); std::puts("PASS UI maintenance details"); QCoreApplication::exit(0);}); return;
                }
                if(!cleaned && result.value("canClean").toBool()) {cleaned=true;click(window,"maintenanceClean");return;}
                if(cleaned) {
                    require(result.value("count").toInt()==0,"cleanup did not refresh the space review");
                    require(controller->property("freedBytes").isValid(), "cleanup lost freed-space result");
                    require(named(window,"maintenanceFreed")->property("visible").toBool(), "freed space is hidden");
                }
                require(!named(window,"maintenanceDetails")->property("checked").toBool(), "paths must start collapsed");
                require(!named(window,"maintenanceKept")->property("checked").toBool(), "kept folders must start collapsed");
                capture(window, cleaned ? "cleanup-complete" : "cleanup-empty");
                timer->stop(); std::puts("PASS UI maintenance review"); std::fflush(stdout); QCoreApplication::exit(0);
            } catch(const std::exception &e) {timer->stop();std::fprintf(stderr,"FAIL maintenance: %s\n",e.what());QCoreApplication::exit(1);}
        }); timer->start(); return;
    }
    if (scenario == "hero-skins") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("skins")));
        QTimer::singleShot(600, window, [window, model] {
            try {
                auto *grid = named(window, "heroGrid");
                require(grid->property("visible").toBool(), "hero grid hidden");
                grid->setProperty("artwork", QVariantMap{{"Abrams", QVariantMap{{"thumbnail","https://example.invalid/masked.png"},{"masked",true}}}});
                auto *heroPreview=named(window,"heroSkin_Abrams");
                require(heroPreview->property("source").toUrl().toString()=="https://example.invalid/masked.png", "hero thumbnail not connected");
                require(heroPreview->property("hidden").toBool(), "hero thumbnail lost mature-content mask");
                grid->setProperty("search", "Grey");
                require(grid->property("count").toInt() == 1, "hero search did not filter locally");
                grid->setProperty("search", "");
                click(window, "heroTile_Abrams");
                const auto query = model->catalog()->queryState();
                require(query.value("heroes").toStringList() == QStringList{"Abrams"}, "hero click did not filter");
                require(query.value("categories").toStringList() == QStringList{"Skins"}, "skins category missing");
                require(!grid->property("visible").toBool(), "hero grid did not close");
                require(named(window, "catalogSkinResults")->property("visible").toBool(), "filtered list hidden");
                click(window, "allSkinHeroesButton");
                require(grid->property("visible").toBool(), "return to heroes failed");
                std::fprintf(stdout, "PASS hero grid, 45 heroes, selection filter and return\n");
                QCoreApplication::exit(0);
            } catch (const std::exception &e) { std::fprintf(stderr, "FAIL hero-skins: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        return;
    }
    if (scenario == "downloads-page" || scenario == "updates-offline" || scenario == "autoexec-managed") {
        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, scenario == "downloads-page" ? QStringLiteral("downloads") : scenario == "autoexec-managed" ? QStringLiteral("autoexec") : QStringLiteral("grid")));
        auto *timer = new QTimer(window); timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window,model,timer,scenario,ticks=0,started=false]() mutable {
            try {
                ++ticks;
                if(model->loading()) { require(ticks<70,"Fixture load timed out"); return; }
                if(scenario=="autoexec-managed") {
                    if(ticks<30)return;
                    QObject *toggle=nullptr;
                    for(auto *o:objects(window))if(o->objectName()=="managedSectionToggle" && o->property("visible").toBool()) {toggle=o;break;}
                    require(toggle,"Managed section disclosure is hidden");
                    auto *text=named(toggle->parent(),"managedSectionText");
                    if(!started) {
                        require(!text->property("text").toString().isEmpty(),"Collapsed managed section is blank");
                        require(!text->property("text").toString().contains('\n'),"Managed section not collapsed");
                        require(toggle->property("text").toString().startsWith("Show "),"Managed line count missing");
                        realClick(window,"managedSectionToggle");started=true;return;
                    }
                    require(text->property("text").toString().contains('\n'),"Managed section did not expand");
                    // Expanded text can move the button below the viewport.
                    // Activate the same disclosure directly for the collapse assertion.
                    click(toggle->parent(),"managedSectionToggle");
                    require(!text->property("text").toString().contains('\n'),"Managed section did not collapse");
                } else if(scenario=="downloads-page") {
                    require(named(window,"downloadsTab")->property("checked").toBool(),"Downloads nav is not highlighted");
                    require(!model->remote()->visible(),"Downloads opened a dialog");
                    require(named(window,"downloadsBrowseButton")->property("visible").toBool(),"Downloads empty action hidden");
                    realClick(window,"downloadsBrowseButton");
                    require(named(window,"browseTab")->property("checked").toBool(),"Browse action did not navigate");
                } else if(scenario=="updates-offline") {
                    if(!started) { require(ticks<150,"Initial update check did not finish"); if(ticks<15 || model->updatesChecking())return; if(!model->checkForUpdates())return;model->checkModHealth();started=true;return; }
                    if(model->updatesChecking() || model->healthChecking()) { require(ticks<200,"Offline update check remained busy"); return; }
                    require(model->updatesOffline(),"Offline result missing");
                    require(named(window,"checkUpdatesButton")->property("enabled").toBool(),"Check button still disabled");
                    capture(window,"updates-offline");
                }
                timer->stop();std::fprintf(stdout,"PASS UI %s\n",qPrintable(scenario));QCoreApplication::exit(0);
            }catch(const std::exception &e){timer->stop();std::fprintf(stderr,"FAIL UI: %s\n",e.what());QCoreApplication::exit(1);}
        });timer->start();return;
    }
    if (scenario == "mod-updates") {
        auto *timer = new QTimer(window);timer->setInterval(100);
        QObject::connect(timer,&QTimer::timeout,window,[window,model,timer,stage=0,ticks=0]() mutable {
            try {
                require(++ticks<150,"Mod updates UI timed out");
                if(model->loading() || model->updateCount()==0)return;
                if(stage==0){click(window,"libraryTab");++stage;return;}
                if(stage==1){
                    require(named(window,"updateAllButton")->property("visible").toBool(),"Update all hidden");
                    require(named(window,"updateAllButton")->property("text").toString().contains(QString::number(model->updateCount())),"Update count absent");
                    realClick(window,"updatesFilter");++stage;return;
                }
                require(model->updatesOnly() && model->count()==model->updateCount(),"Updates filter differs from count");
                bool pill=false;for(auto *o:objects(window))if(o->objectName()=="modUpdatePill" && o->property("visible").toBool())pill=true;
                require(pill,"Update available pill hidden");
                timer->stop();std::puts("PASS UI mod updates: pill, filter and Update all count");QCoreApplication::exit(0);
            }catch(const std::exception &e){timer->stop();std::fprintf(stderr,"FAIL UI mod updates: %s\n",e.what());QCoreApplication::exit(1);}
        });timer->start();return;
    }
    if (scenario == "home-settings-summary") {
        QQmlComponent states(qmlEngine(window));
        states.setData(R"(import QtQuick
import "qrc:/qt/qml/Deadlock/Native/qml/HomeState.js" as HomeState
QtObject {
    property bool correct: {
        const base = {gamePath: "/fixture", pendingStatus: "current", appliedProfileId: "p", activeProfileId: "p", viewedProfileId: "p", enabledCount: 0}
        return HomeState.state(base) === "current"
            && HomeState.state(Object.assign({}, base, {pendingChanges: true})) === "pending"
            && HomeState.state(Object.assign({}, base, {appliedProfileId: ""})) === "pending"
            && HomeState.state(Object.assign({}, base, {appliedProfileId: "", enabledCount: 2, pendingStatus: "current"})) === "pending"
            && HomeState.action("pending") === "Play"
    }
})", QUrl());
        std::unique_ptr<QObject> stateProbe(states.create());
        require(stateProbe && stateProbe->property("correct").toBool(), "Home state/action differs from baseline");
        auto *timer = new QTimer(window);
        timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, stage = 0, ticks = 0]() mutable {
            try {
                require(++ticks < 100, "Home and Settings check timed out");
                if (model->loading()) return;
                if (stage == 0) {
                    QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("settings"))); ++stage; return;
                }
                auto *settings = named(window, "settingsPage");
                const QString setupPrompt = QStringLiteral("Choose your Deadlock folder to check the game setup.");
                auto *gameStatus = named(settings, "gameSetupStatus");
                require(gameStatus->property("visible").toBool() && gameStatus->property("text").toString() == setupPrompt,
                        "Settings did not show the game folder prompt");
                for (auto *item : objects(settings)) {
                    if (!item->property("visible").toBool()) continue;
                    const auto text = item->property("text").toString();
                    require(!text.startsWith("Checking storage") && !text.startsWith("Checking… used"),
                            "Demo Settings reports a storage scan while idle");
                }

                auto *dataRow = named(window, "libraryFolderRow");
                require(dataRow->property("visible").toBool(), "app data folder hidden");
                require(dataRow->property("label").toString() == "App data folder", "app data folder label missing");
                require(dataRow->property("path").toString() == model->dataDir(), "app data folder path differs");
                require(named(window, "automaticApplySwitch"), "automatic apply setting missing");
                auto *colours = named(window, "settingsColours");
                const QVariant originalColourMode = colours->property("currentValue");
                require(QMetaObject::invokeMethod(colours, "activated", Q_ARG(QVariant, QStringLiteral("desktop"))), "colour control does not activate");
                require(colours->property("currentValue").toString() == "desktop", "desktop colours do not update live");
                require(QMetaObject::invokeMethod(colours, "activated", Q_ARG(QVariant, QStringLiteral("modlock"))), "brand colour control does not activate");
                require(colours->property("currentValue").toString() == "modlock", "brand colours do not update live");
                QMetaObject::invokeMethod(colours, "activated", Q_ARG(QVariant, originalColourMode));
                require(named(window, "reduceMotionSwitch"), "reduce motion setting missing");
                QFile executable(QCoreApplication::applicationFilePath()); QCryptographicHash hash(QCryptographicHash::Sha256);
                require(executable.open(QIODevice::ReadOnly) && hash.addData(&executable), "cannot read executable for build check");
                require(named(window, "settingsBuildId")->property("text").toString().contains(QString::fromLatin1(hash.result().toHex().left(12))), "build identifier does not match executable");
                timer->stop(); std::puts("PASS UI Home states and Settings status and metadata"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { timer->stop(); std::fprintf(stderr, "FAIL UI Home/Settings: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        timer->start(); return;
    }
    if (scenario == "review-status") {
        QTimer::singleShot(100, window, [window] {
            try {
                QQmlComponent launchWarnings(qmlEngine(window));
                launchWarnings.setData(R"(import QtQml
import "qrc:/qt/qml/Deadlock/Native/qml/LaunchPresentation.js" as LaunchPresentation
QtObject {
    readonly property string technicalWarning: LaunchPresentation.summaryRows({
        warnings: [{code: "unverifiedRestore", message: "Check your restore point."}]
    }).join(" ")
})", QUrl());
                std::unique_ptr<QObject> launchWarningProbe(launchWarnings.create());
                require(launchWarningProbe && launchWarningProbe->property("technicalWarning").toString() == "Check your restore point.",
                        "Launch review details did not show the warning message without its internal code");
                QQmlComponent component(qmlEngine(window), QUrl("qrc:/qt/qml/Deadlock/Native/qml/StatusNotice.qml"));
                QVariantMap data{{"lastBulkResult",QVariantMap{{"changed",1}}},{"health",QVariantMap{}},{"launch",QVariantMap{{"writeGate",false}}},
                    {"busy",false},{"loading",false},{"error",""},{"warnings",QStringList{}},{"demoMode",false},{"nativeMode",true},
                    {"bootstrapReason",""},{"bootstrapState","ready"},{"outcomeUnknown",false},{"recoveryRequired",false},{"recoveryNeedsAttention",false},
                    {"externalGameinfoChange",false},{"canRecover",true},{"updatesOffline",false}};
                std::unique_ptr<QObject> notice(component.createWithInitialProperties({{"library",data}}));
                require(bool(notice), qPrintable(component.errorString()));
                auto *item = qobject_cast<QQuickItem *>(notice.get());
                item->setParentItem(window->contentItem()); item->setWidth(700); item->setHeight(200);
                require(named(item,"bulkResultLabel")->property("visible").toBool(), "bulk result missing");
                click(item,"dismissBulkResult");
                require(!item->property("visible").toBool(), "dismissed bulk result occupies status spot");
                data["lastBulkResult"]=QVariantMap{}; item->setProperty("library",data);
                require(item->property("dismissedBulkKey").toString().isEmpty(), "model clear retained bulk dismissal key");
                data["lastBulkResult"]=QVariantMap{{"changed",1}}; item->setProperty("library",data);
                require(named(item,"bulkResultLabel")->property("visible").toBool(), "identical later result stayed dismissed");
                click(item,"dismissBulkResult");
                data["lastBulkResult"] = QVariantMap{{"changed",2}}; item->setProperty("library",data);
                item->setProperty("importActive",true);
                require(!item->property("visible").toBool(), "bulk result hides import controls");
                item->setProperty("importActive",false);
                data["updatesOffline"] = true; item->setProperty("library",data);
                require(named(item,"modHealthNotice")->property("visible").toBool(), "offline check was hidden by bulk result");
                require(named(item,"modHealthNotice")->property("text").toString().contains("GameBanana"), "offline result missing");
                data["lastBulkResult"] = QVariantMap{{"changed",0},{"skipped",QVariantList{QVariantMap{{"reason","selectionRequired"}}}}}; item->setProperty("library",data);
                data["busy"]=true; item->setProperty("library",data);
                data["busy"]=false; item->setProperty("library",data);
                item->setProperty("showChecks",false);
                require(named(item,"bulkResultLabel")->property("visible").toBool(), "unseen skipped result lost behind offline notice");
                item->setProperty("showChecks",true);
                require(named(item,"modHealthNotice")->property("visible").toBool(), "offline notice missing on return");
                item->setProperty("showChecks",false);
                require(named(item,"bulkResultLabel")->property("visible").toBool(), "navigation dismissed skipped result");
                item->setProperty("importActive",true);
                data["busy"]=true; item->setProperty("library",data);
                data["busy"]=false; item->setProperty("library",data);
                item->setProperty("importActive",false);
                require(named(item,"bulkResultLabel")->property("visible").toBool(), "busy operation discarded feedback hidden by import");
                click(item,"dismissBulkResult");
                require(!item->property("visible").toBool(), "explicit bulk dismissal failed");
                data["outcomeUnknown"] = true; item->setProperty("library",data);
                require(named(item,"checkOperationButton")->property("visible").toBool(), "Check status hidden");
                data["recoveryNeedsAttention"] = true; item->setProperty("library",data);
                require(named(item,"recoverButton")->property("visible").toBool(), "Recover hidden");
                const QString generic="An earlier change didn't finish. Recover before making more changes.";
                // Only a cause the helper attached to this recovery is shown, without
                // internal kind names; any other library error keeps the generic text.
                for(const auto &row:QList<QPair<QString,QString>>{
                    {"recoveryRequired: Unfinished. Cause during Apply: nativeIo: Not enough disk space (ENOSPC 28).",generic+"\nNot enough disk space (ENOSPC 28)."},
                    {"recoveryRequired: Unfinished. Cause during Apply: nativeDatabase: Not enough disk space to save the library (SQLITE_FULL 13).",generic+"\nNot enough disk space to save the library (SQLITE_FULL 13)."},
                    {"recoveryRequired: Unfinished. Cause during Restore: gameRunning: Close Deadlock first.; refusal check failed: nativeIo: Cannot access native or source files.",generic+"\nCannot access native or source files."},
                    {"recoveryRequired: Unfinished. Cause during Apply: gameRunning: Close Deadlock first.",generic+"\nClose Deadlock first."},
                    {"recoveryRequired: Unfinished. Cause during Apply: selectionInvalid: redTeam: Shared Pack (mod123) is missing.",generic+"\nredTeam: Shared Pack (mod123) is missing."},
                    {"nativeTransport: The helper connection was lost.",generic},
                    {"disk: No space left",generic},
                    {"",generic}}){
                    data["error"]=row.first;data["recoveryRequired"]=true;item->setProperty("library",data);
                    auto *status=named(item,"operationStatus");require(status->property("visible").toBool(),"Recovery text hidden");
                    const auto text=status->property("text").toString();
                    if(text!=row.second)throw std::runtime_error(("Recovery text mismatch: "+text).toStdString());
                    require(status->property("textFormat").toInt()==int(Qt::PlainText),"Recovery text is not plain text");
                    require(named(item,"recoverButton")->property("visible").toBool(),"Recover hidden beside recovery text");
                }
                data["outcomeUnknown"]=false; data["recoveryRequired"]=false; data["recoveryNeedsAttention"]=false;
                const QString performanceGuidance = QStringLiteral("Your game has a different version of these performance settings. To remove them, close Deadlock, choose this preset again and apply it, then choose None and apply again.");
                for (const auto &error : {QStringLiteral("performanceOutdated"), QStringLiteral("performanceOutdated: Previous preset version detected.")}) {
                    data["error"] = error;
                    item->setProperty("library", data);
                    auto *label = named(item, "errorLabel");
                    require(label->property("visible").toBool() && label->property("text").toString() == performanceGuidance,
                            "Outdated performance settings did not show the removal steps");
                }
                data["error"]=""; data["lastBulkResult"]=QVariantMap{};
                const QString existingWarning="An existing library warning.";
                const QString lostWarning="The connection was lost while saving. This is the current state. Some choices were not saved; change them again.";
                data["warnings"]=QStringList{existingWarning,lostWarning}; item->setProperty("library",data);
                auto *warning=named(item,"warningLabel");
                for (int state=0; state<3; ++state) {
                    data["busy"]=state==1; data["externalGameinfoChange"]=state==2;
                    data["externalGameinfoMessage"]="Game files changed."; data["status"]="Reviewing saved choices…";
                    item->setProperty("library",data);
                    require(warning->property("visible").toBool(), "Lost-edit warning hidden by work or game-file status");
                    const auto text=warning->property("text").toString();
                    require(text.contains(existingWarning) && text.contains(lostWarning), "Existing warning hid the lost-edit notice");
                    require(warning->property("textFormat").toInt()==int(Qt::PlainText), "Lost-edit warning is not plain text");
                }
                // Exercise Main's actual occupied binding, not just an isolated notice.
                auto *mainNotice=named(window,"statusNotice");
                auto *launchNotice=named(window,"launchNotice");
                mainNotice->setProperty("library",data);
                launchNotice->setProperty("visible",true);
                require(mainNotice->property("occupied").toBool(), "Main launch priority fixture not active");
                require(mainNotice->property("visible").toBool(), "Main launch notice hid the lost-edit warning");
                require(named(mainNotice,"warningLabel")->property("visible").toBool(), "Main lost-edit warning is hidden");
                require(!named(mainNotice,"operationStatus")->property("visible").toBool(), "Launch notice also showed the library operation bar");
                auto *helperStop = findNamed(window, "helperStopNotice");
                require(helperStop && !helperStop->property("visible").toBool(), "Demo mode unexpectedly showed helperStopNotice");
                std::puts("PASS UI review status: dismiss, import, offline checks, recovery and lost-edit warnings"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { std::fprintf(stderr,"FAIL review status: %s\n",e.what()); QCoreApplication::exit(1); }
        }); return;
    }
    if (scenario == "helper-stop-notice") {
        const auto peerDir = qEnvironmentVariable("ITEM53_PEER_DIR");
        auto *timer = new QTimer(window); timer->setInterval(50);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, peerDir, stage = 0, ticks = 0]() mutable {
            try {
                require(!peerDir.isEmpty(), "ITEM53_PEER_DIR is missing");
                require(++ticks < 600, qPrintable(QString("helper-stop-notice timed out at stage %1: loading=%2 busy=%3 stopped=%4 reason=%5 error=%6")
                    .arg(stage).arg(model->loading()).arg(model->busy()).arg(model->helperStopped()).arg(model->stopReason(), model->error())));
                const auto writeFlag = [&](const QString &name, const QByteArray &body = {}) {
                    QFile f(peerDir + "/" + name);
                    require(f.open(QIODevice::WriteOnly | QIODevice::Truncate), qPrintable("Cannot write " + name));
                    if (!body.isEmpty()) require(f.write(body) == body.size(), qPrintable("Cannot write body of " + name));
                };
                auto *notice = named(window, "helperStopNotice");
                auto *reloadButton = named(window, "helperStopReloadButton");
                auto *launchNotice = named(window, "launchNotice");
                auto *statusNotice = named(window, "statusNotice");
                if (stage == 0) {
                    if (!model->nativeMode() || model->loading() || !model->canSetEnabled() || !model->canRestore()
                        || model->launch()->busy() || model->launch()->writeGate()) return;
                    require(!notice->property("visible").toBool(), "helperStopNotice visible on healthy startup");
                    writeFlag("fail-on-method", "native.restore");
                    require(model->restorePrevious(), "cannot start restore");
                    stage = 1; return;
                }
                if (stage == 1) {
                    if (!notice->property("visible").toBool() || !model->outcomeUnknown()) return;
                    require(statusNotice->property("visible").toBool(), "StatusNotice uncertainty hidden behind helperStopNotice");
                    require(named(window, "operationStatus")->property("visible").toBool(), "operationStatus hidden behind helperStopNotice");
                    require(named(window, "checkOperationButton")->property("visible").toBool(), "Check status hidden behind helperStopNotice");
                    const auto body = notice->property("text").toString();
                    require(body.contains("Helper exited unexpectedly") && body.contains("Reload the library or restart Modlock to reconnect."),
                            qPrintable("Missing cause or restart guidance beside uncertainty: " + body));
                    require(reloadButton->property("visible").toBool() && reloadButton->property("enabled").toBool(), "helperStopReloadButton unusable beside uncertainty");
                    QFile::remove(peerDir + "/fail-on-method");
                    writeFlag("legacy-launch-barrier");
                    realClick(window, "helperStopReloadButton");
                    stage = 2; return;
                }
                if (stage == 2) {
                    auto *clearBtn = findNamed(window, "clearLegacyLaunchButton");
                    if (model->loading() || model->busy() || model->outcomeUnknown() || notice->property("visible").toBool()
                        || !launchNotice->property("visible").toBool() || !clearBtn || !clearBtn->property("visible").toBool()
                        || !clearBtn->property("enabled").toBool() || !model->launch()->error().contains("saved launch outcome is unknown")) return;
                    require(!model->helperStopped() && model->stopReason().isEmpty(), "helperStopped survived uncertainty reconnect");
                    writeFlag("fail-on-method", "native.launch.resolve");
                    realClick(window, "clearLegacyLaunchButton");
                    stage = 3; return;
                }
                if (stage == 3) {
                    if (!notice->property("visible").toBool()) return;
                    require(launchNotice->property("visible").toBool(), "LaunchNotice disappeared when helper stopped");
                    require(!model->outcomeUnknown(), "launch clear stop unexpectedly set model outcomeUnknown");
                    const auto body = notice->property("text").toString();
                    require(body.contains("Helper exited unexpectedly") && body.contains("Reload the library or restart Modlock to reconnect."),
                            qPrintable("Missing cause or restart guidance: " + body));
                    require(reloadButton->property("visible").toBool() && reloadButton->property("enabled").toBool(), "helperStopReloadButton unusable");
                    QFile::remove(peerDir + "/fail-on-method");
                    realClick(window, "helperStopReloadButton");
                    stage = 4; return;
                }
                auto *clearBtn = findNamed(window, "clearLegacyLaunchButton");
                if (model->loading() || model->busy() || model->outcomeUnknown() || notice->property("visible").toBool()
                    || !launchNotice->property("visible").toBool() || !clearBtn || !clearBtn->property("visible").toBool()
                    || !clearBtn->property("enabled").toBool()
                    || !model->launch()->error().contains("saved launch outcome is unknown")) return;
                require(!model->helperStopped() && model->stopReason().isEmpty(), "helperStopped survived live reconnect");
                QFile::remove(peerDir + "/legacy-launch-barrier");
                timer->stop();
                std::puts("PASS UI helper-stop-notice: cause, guidance, simultaneous LaunchNotice/uncertainty, reconnect");
                std::fflush(stdout); QCoreApplication::exit(0);
            } catch (const std::exception &e) {
                timer->stop(); std::fprintf(stderr, "FAIL helper-stop-notice (stage %d): %s\n", stage, e.what()); QCoreApplication::exit(1);
            }
        });
        timer->start(); return;
    }
    if (scenario == "native-first-play") {
        QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("home")));
        auto *timer = new QTimer(window); timer->setInterval(50);
        QObject::connect(timer,&QTimer::timeout,window,[window,model,timer,started=false,ticks=0]() mutable {
            try {
                require(++ticks < 240, qPrintable("first Play timed out: " + model->launch()->error()));
                if (!started) {
                    if (model->loading() || model->detectedGamePath().isEmpty() || !model->launch()->canLaunch()) return;
                    require(model->gamePath().isEmpty(), "first-play fixture has a saved game folder");
                    click(window,"launchActiveButton"); started=true; return;
                }
                require(model->launch()->error().isEmpty(),qPrintable(model->launch()->error()));
                if (model->launch()->phase() != "ready") return;
                require(model->launch()->review().value("summary").toMap().value("gamePath").toString() == model->detectedGamePath(), "launch did not review detected folder");
                model->launch()->cancel(); timer->stop(); std::puts("PASS UI first Play reviews detected folder"); QCoreApplication::exit(0);
            } catch(const std::exception &e) { timer->stop(); std::fprintf(stderr,"FAIL first Play: %s\n",e.what()); QCoreApplication::exit(1); }
        }); timer->start(); return;
    }
    if (scenario == "home-profile") {
        QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("home")));
        auto palette=QGuiApplication::palette();
        const bool dark=qEnvironmentVariable("HOME_PROFILE_THEME")=="dark";
        palette.setColor(QPalette::Window,QColor(dark?"#0e0c15":"#f1eff5"));
        palette.setColor(QPalette::WindowText,QColor(dark?"#eee8dc":"#1b1626"));
        QGuiApplication::setPalette(palette);
        auto *timer=new QTimer(window); timer->setInterval(100);
        QObject::connect(timer,&QTimer::timeout,window,[window,model,timer,ticks=0,clicked=false]() mutable {
            try {
                require(++ticks<100,"Home profile fixture timed out");
                if(model->loading() || ticks<8)return;
                if (clicked) {
                    const auto profileCase = qEnvironmentVariable("HOME_PROFILE_CASE");
                    const bool available = profileCase != "viewed-no-apply" && profileCase != "viewed-no-preview";
                    if (available) {
                        require(model->error().isEmpty(),qPrintable(model->error()));
                        if (model->previewBusy()) return;
                        require(model->applyReview().value("profileId").toString()=="fast", "Home Apply preview was not acknowledged for the selected profile");
                    }
                    timer->stop(); std::puts("PASS UI Home profile button"); QCoreApplication::exit(0); return;
                }
                require(model->error().isEmpty(),qPrintable(model->error()));
                if(qEnvironmentVariable("HOME_PROFILE_CASE").startsWith("viewed-") && model->viewedProfileId()!="fast") {
                    model->viewProfile("fast");return;
                }
                require(model->error().isEmpty(),qPrintable(model->error()));
                const auto profileCase = qEnvironmentVariable("HOME_PROFILE_CASE");
                const bool actionableNotice = profileCase != "active-identical" && profileCase != "active-unapplied";
                auto *callout = named(window, "homeStatusCallout");
                require(callout->property("visible").toBool() == actionableNotice, "Home status notice visibility does not match profile state");
                if (actionableNotice) {
                    require(callout->property("title").toString() == qEnvironmentVariable("HOME_PROFILE_SENTENCE"), "Home headline refers to wrong profile");
                    require(named(window, "homeStatusAction")->property("text").toString() == qEnvironmentVariable("HOME_PROFILE_ACTION"), "Home action refers to wrong profile");
                } else {
                    require(named(window, "launchActiveButton")->property("visible").toBool(), "Header Play button hidden");
                    require(named(window, "profileChooser")->property("visible").toBool(), "Header profile chooser hidden");
                }
                capture(window,"home");
                if (profileCase.startsWith("viewed-") && !profileCase.endsWith("choices")) {
                    const bool available = profileCase != "viewed-no-apply" && profileCase != "viewed-no-preview";
                    auto *button = qobject_cast<QQuickItem *>(named(window,"homeStatusAction"));
                    require(button && button->isVisible(), "Home Apply button is hidden");
                    require(button->isEnabled() == available, "Home Apply capability gate is wrong");
                    // Send pointer events even when disabled to verify refusal through the real control.
                    const auto p = button->mapToScene(QPointF(button->width()/2,button->height()/2));
                    QMouseEvent press(QEvent::MouseButtonPress,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::LeftButton,Qt::NoModifier);
                    QMouseEvent release(QEvent::MouseButtonRelease,p,p,window->mapToGlobal(p.toPoint()),Qt::LeftButton,Qt::NoButton,Qt::NoModifier);
                    QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
                    require(model->busy() == available, "Home Apply click did not respect capability");
                    clicked = true; return;
                }
                timer->stop();
                std::puts("PASS UI Home profile"); QCoreApplication::exit(0);
            } catch(const std::exception &e) { std::fprintf(stderr,"FAIL UI Home profile: %s\n",e.what()); QCoreApplication::exit(1); }
        });timer->start();return;
    }
    if (scenario == "home") {
        auto *timer = new QTimer(window);
        timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, stage = -1, ticks = 0, before = QMap<QString, QByteArray>{}]() mutable {
            try {
                require(++ticks < 100, "Home check timed out");
                if (model->loading()) return;
                if (stage == -1) {
                    auto *downloads = named(window, "downloadsTab");
                    downloads->setProperty("visible", true);
                    downloads->setProperty("activeDownloads", 1234);
                    ++stage; return;
                }
                if (stage == 0) {
                    auto *downloads = qobject_cast<QQuickItem *>(named(window, "downloadsTab"));
                    auto *badge = qobject_cast<QQuickItem *>(named(downloads, "downloadCountBadge"));
                    auto *icon = qobject_cast<QQuickItem *>(named(downloads, "navigationIcon"));
                    require(badge && icon, "Downloads badge or icon missing");
                    const QRectF badgeRect(badge->mapToItem(downloads, QPointF()), badge->size());
                    const QRectF iconRect(icon->mapToItem(downloads, QPointF()), icon->size());
                    require(!badgeRect.intersects(iconRect), "Downloads badge overlaps the navigation icon");
                    const auto *accessible = QAccessible::queryAccessibleInterface(downloads);
                    require(accessible && accessible->text(QAccessible::Name).contains("1234"), "Downloads accessible count missing");
                    downloads->setProperty("activeDownloads", 0);
                    downloads->setProperty("visible", false);
                    require(named(window, "homePage")->property("visible").toBool(), "Home did not render as start page");
                    require(!named(window, "homeStatusCallout")->property("visible").toBool(), "Demo setup shows unrequested status callout");
                    QQmlComponent emptyHome(qmlEngine(window), QUrl("qrc:/qt/qml/Deadlock/Native/qml/HomePage.qml"));
                    LibraryModel emptyLibrary;
                    std::unique_ptr<QObject> emptyPage(emptyHome.createWithInitialProperties({{"library", QVariant::fromValue(&emptyLibrary)}}));
                    require(emptyPage && named(emptyPage.get(), "homeWelcome")->property("visible").toBool(), "Uninitialized Home welcome hidden");
                    require(named(window, "profileChooser")->property("visible").toBool(), "Demo profile picker hidden");
                    require(named(window, "profileChooser")->property("enabled").toBool(), "Demo profile picker disabled");
                    require(!model->canEditProfiles() && !named(window, "manageProfilesMenuItem")->property("enabled").toBool(), "Demo profile editing enabled");
                    const auto profiles = model->profiles();
                    require(profiles.size() == 1 && profiles.first().toMap().value("id") == "default"
                            && profiles.first().toMap().value("name") == "Default"
                            && profiles.first().toMap().value("modCount").toInt() == model->totalCount(), "Demo Default metadata differs");
                    stage = 1; return;
                }
                if (stage == 1) {
                    // Baseline after demo loading with canonical settings pre-seeded; include directories and exact file bytes.
                    before = profileBoundarySnapshot();
                    realClick(window, "profileChooser");
                    ++stage; return;
                }
                if (stage == 2) {
                    auto *chooser = named(window, "profileChooser");
                    auto *popup = chooser->property("popup").value<QObject *>();
                    require(popup && popup->property("visible").toBool(), "Actual profile picker did not open");
                    require(chooser->property("count").toInt() == 1, "Demo offers another profile");
                    realClick(window, "profileOption");
                    ++stage; return;
                }
                if (stage == 3) {
                    auto *popup = named(window, "profileChooser")->property("popup").value<QObject *>();
                    require(popup && !popup->property("visible").toBool(), "Selecting Default did not close the actual picker");
                    require(model->viewedProfileId() == "default" && model->activeProfileId() == "default", "Demo picker changed profile identity");
                    realClick(window, "profileChooser");
                    stage = 30; return;
                }
                if (stage == 30) {
                    auto *popup = named(window, "profileChooser")->property("popup").value<QObject *>();
                    require(popup && popup->property("visible").toBool(), "Actual profile picker did not reopen for Manage profiles check");
                    auto *manage = qobject_cast<QQuickItem *>(named(window, "manageProfilesMenuItem"));
                    require(manage && manage->isVisible() && !manage->isEnabled(), "Demo Manage profiles enabled or hidden");
                    // Disabled controls still receive pointer events, as a user would send.
                    const auto pos = manage->mapToScene(QPointF(manage->width()/2, manage->height()/2));
                    QMouseEvent press(QEvent::MouseButtonPress,pos,pos,window->mapToGlobal(pos.toPoint()),Qt::LeftButton,Qt::LeftButton,Qt::NoModifier);
                    QMouseEvent release(QEvent::MouseButtonRelease,pos,pos,window->mapToGlobal(pos.toPoint()),Qt::LeftButton,Qt::NoButton,Qt::NoModifier);
                    QCoreApplication::sendEvent(window,&press); QCoreApplication::sendEvent(window,&release);
                    stage = 4; return;
                }
                if (stage == 4) {
                    require(!model->canEditProfiles() && !model->canCreateLibrary() && !model->canPreviewApply() && !model->canRestore(), "Demo library/game write gate opened");
                    require(!model->createProfile("Demo attempt"), "Demo profile creation was accepted");
                    require(!profilePopup(window) || !profilePopup(window)->property("visible").toBool(), "Disabled Manage profiles opened dialog");
                    for (auto *object : objects(window))
                        require(!object->property("visible").toBool() || !object->property("text").toString().startsWith("Play uses "), "Demo shows Play recap");
                    require(before == profileBoundarySnapshot(), "Demo profile actions wrote private XDG data/config");
                    if (auto *popup = named(window, "profileChooser")->property("popup").value<QObject *>())
                        if (popup->property("visible").toBool()) realKey(window, Qt::Key_Escape);
                    std::puts("PASS demo profile boundary: real picker, disabled edits, byte-identical XDG data/config, no Play recap");
                    realClick(window, "homeOpenBrowseButton"); ++stage; return;
                }
                if (stage == 5) {
                    require(named(window, "modSearch")->property("visible").toBool(), "Demo Home action did not open the sample library");
                    const auto firstId = model->idAt(0);
                    require(!firstId.isEmpty(), "Demo sample mod missing");
                    const bool wasEnabled = model->modDetails(firstId).value("enabled").toBool();
                    model->setSelectedId(firstId);
                    toggleIncluded(window, !wasEnabled);
                    require(model->modDetails(firstId).value("enabled").toBool() != wasEnabled, "Demo in-memory inclusion toggle failed");
                    click(window, "homeTab"); ++stage; return;
                }
                require(named(window, "homePage")->property("visible").toBool(), "Home return navigation failed");
                require(before == profileBoundarySnapshot(), "Demo navigation or in-memory toggle wrote private XDG data/config");
                timer->stop(); std::puts("PASS UI home: start screen, status action, Library and return navigation"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { timer->stop(); std::fprintf(stderr, "FAIL UI home: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        timer->start(); return;
    }
    if (scenario == "home-catalog") {
        const auto theme = qEnvironmentVariable("HOME_CATALOG_THEME");
        if (theme == "dark" || theme == "light") {
            auto palette = QGuiApplication::palette();
            const bool dark = theme == "dark";
            palette.setColor(QPalette::Window, QColor(dark ? "#0e0c15" : "#f1eff5"));
            const QColor text(dark ? "#eee8dc" : "#1b1626");
            const QColor disabled(dark ? "#716a7b" : "#9b94a6");
            for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) {
                palette.setColor(role, text);
                palette.setColor(QPalette::Disabled, role, disabled);
            }
            QGuiApplication::setPalette(palette);
        }
        auto *timer = new QTimer(window);
        timer->setInterval(100);
        QObject::connect(timer, &QTimer::timeout, window, [window, model, timer, stage = 0, ticks = 0, switchedProfile = false, previous = QHash<QQuickItem *, bool>{}, startY = 0., extent = 0.]() mutable {
            try {
                require(++ticks < 160, "Home catalog check timed out");
                auto *catalog = model->catalog();
                if (model->loading() || catalog->homeLoading()) return;
                if (!switchedProfile) {
                    const auto viewProfile = qEnvironmentVariable("HOME_CATALOG_VIEW_PROFILE");
                    if (!viewProfile.isEmpty() && model->viewedProfileId() != viewProfile) {
                        switchedProfile = true;
                        model->viewProfile(viewProfile);
                        return;
                    }
                    switchedProfile = true;
                }
                const auto mode = qEnvironmentVariable("HOME_CATALOG_MODE", "populated");
                auto saveCaptures = [window](const QString &cap720, const QString &cap1200) {
                    auto settleAndSave = [window](int w, int h, const QString &path, const char *err) {
                        if (path.isEmpty()) return;
                        window->resize(w, h);
                        for (int step = 0; step < 6; ++step) {
                            QEventLoop loop;
                            QTimer::singleShot(30, &loop, &QEventLoop::quit);
                            loop.exec();
                        }
                        require(window->grabWindow().save(path), err);
                    };
                    settleAndSave(720, 780, cap720, "720x780 Home capture failed");
                    settleAndSave(1200, 780, cap1200, "1200x780 Home capture failed");
                };
                if (mode == "coexist") {
                    require(model->externalGameinfoChange(), "External coexistence fixture not admitted");
                    auto *global = named(window, "statusNotice");
                    auto *home = named(window, "homeStatusCallout");
                    auto *launch = named(window, "launchNotice");
                    if (stage == 0) {
                        require(global->property("externalChangeShown").toBool() && !home->property("visible").toBool(), "External warning duplicated without launch occupancy");
                        // Controlled presentation injection, not a real launch failure.
                        launch->setProperty("visible", true); stage=1; return;
                    }
                    if (stage == 1) {
                        require(global->property("occupied").toBool() && !global->property("externalChangeShown").toBool(), "Launch did not occupy global notice");
                        require(home->property("visible").toBool() && named(window,"homeStatusAction")->property("text").toString()=="Apply again…", "External warning/action lost behind launch notice");
                        launch->setProperty("visible", false); stage=2; return;
                    }
                    require(global->property("externalChangeShown").toBool() && !home->property("visible").toBool(), "External duplicate suppression did not restore");
                    timer->stop(); std::puts("PASS UI home external notice coexistence"); QCoreApplication::exit(0); return;
                }
                if (mode == "unknown") {
                    require(model->pendingStatus() == "unknown" && model->error().isEmpty(), "Independent unknown fixture is not admitted");
                    require(named(window, "homeStatusCallout")->property("visible").toBool(), "Independent unknown notice hidden");
                    require(!named(window, "statusNotice")->property("visible").toBool(), "Independent unknown fixture has a global notice");
                    timer->stop(); std::puts("PASS UI home independent unknown"); QCoreApplication::exit(0); return;
                }
                if (mode == "viewport") {
                    auto *scroll = qobject_cast<QQuickItem *>(named(window, "homePage"));
                    auto *flick = scroll->property("contentItem").value<QQuickItem *>();
                    auto *content = named(window, "homeShelfContent");
                    require(flick && content, "Viewport fixture missing geometry");
                    const auto admissions = [&] {
                        QHash<QQuickItem *, bool> result;
                        for (auto *object : objects(window)) {
                            if (object->objectName() != "homeRecentTile" && object->objectName() != "homeLatestTile" && object->objectName() != "homePopularTile") continue;
                            auto *tile = qobject_cast<QQuickItem *>(object);
                            const auto point = tile->mapToItem(scroll, QPointF(0, 0));
                            const bool expected = point.y()+tile->height()>-80 && point.y()<scroll->property("availableHeight").toDouble()+80;
                            require(tile->property("previewActive").toBool()==expected, "Viewport position change did not update preview admission");
                            result.insert(tile, expected);
                        }
                        require(!result.isEmpty(), "Viewport fixture checked no tiles");
                        return result;
                    };
                    if (stage == 0) { window->resize(720,780); stage=1; return; }
                    const auto current = admissions();
                    if (stage == 1) {
                        require(current.values().contains(true) && current.values().contains(false), "Controlled initial viewport lacks mixed admission");
                        previous=current; startY=flick->property("contentY").toDouble();
                        const auto bottom=flick->property("contentHeight").toDouble()-flick->height();
                        require(bottom>startY+80, "Viewport fixture cannot scroll across a boundary");
                        flick->setProperty("contentY", bottom); stage=2; return;
                    }
                    if (stage == 2) {
                        require(flick->property("contentY").toDouble()!=startY && current!=previous, "Scroll did not cross any tile boundary");
                        previous=current; startY=flick->property("contentY").toDouble(); extent=flick->property("contentHeight").toDouble();
                        content->setProperty("y", 2000); stage=3; return;
                    }
                    require(flick->property("contentY").toDouble()==startY && flick->property("contentHeight").toDouble()==extent, "Position-only fixture changed scroll or extent");
                    if (stage == 3) {
                        require(current!=previous && !current.values().contains(true), "Position-only change crossed no tile boundary");
                        content->setProperty("y", 0); stage=4; return;
                    }
                    require(current==previous, "Restoring content position did not restore preview admission");
                    timer->stop(); std::puts("PASS UI home viewport scroll and position-only admission"); QCoreApplication::exit(0); return;
                }
                if (mode == "capture") {
                    if (stage == 0) { ++stage; return; }
                    saveCaptures(qEnvironmentVariable("HOME_CATALOG_CAPTURE_720"), qEnvironmentVariable("HOME_CATALOG_CAPTURE_1200"));
                    timer->stop(); std::puts("PASS UI home catalog capture"); QCoreApplication::exit(0); return;
                }
                if (mode == "empty") {
                    require(catalog->homeLatest().isEmpty() && catalog->homePopular().isEmpty(), "Empty catalog unexpectedly populated shelves");
                    require(named(window, "homeCatalogFallback")->property("visible").toBool(), "Empty cache fallback hidden");
                    require(named(window, "homeOpenBrowseButton")->property("visible").toBool(), "Open Browse button hidden");
                    require(!catalog->active() && catalog->route() == "browse" && catalog->model()->count() == 0, "Empty Home polluted Browse state");
                    saveCaptures(qEnvironmentVariable("HOME_CATALOG_CAPTURE_720"), qEnvironmentVariable("HOME_CATALOG_CAPTURE_1200"));
                    if (qEnvironmentVariable("HOME_CATALOG_CLICK_BROWSE") == "1") {
                        click(window, "homeOpenBrowseButton");
                        QCoreApplication::processEvents();
                        require(catalog->active() && catalog->route() == "browse", "Open Browse did not activate Browse");
                    }
                    timer->stop(); std::puts("PASS UI home catalog empty"); QCoreApplication::exit(0); return;
                }
                if (stage == 0) {
                    require(!catalog->active() && catalog->route() == "browse", "Home loading activated Browse");
                    require(catalog->model()->count() == 0 && catalog->total() == "0" && !catalog->contentReady() && catalog->provenance().isEmpty(), "Home loading mutated Browse model");
                    require(catalog->queryState().value("sort").toString() == "default" && catalog->error().isEmpty(), "Home loading mutated Browse query or error");
                    require(catalog->homeLatest().size() > 0 && catalog->homeLatest().size() <= 5,
                            QString("Latest shelf count invalid (latest=%1 popular=%2 homeActive=%3 active=%4 avail=%5 unavail=%6 loading=%7 native=%8 session=%9 error=%10)")
                                .arg(catalog->homeLatest().size()).arg(catalog->homePopular().size())
                                .arg(catalog->homeActive()).arg(catalog->active()).arg(catalog->available())
                                .arg(catalog->homeUnavailable()).arg(catalog->homeLoading()).arg(model->nativeMode())
                                .arg(catalog->libraryMediaSession(), catalog->error()).toStdString().c_str());
                    require(catalog->homePopular().size() > 0 && catalog->homePopular().size() <= 5, "Popular shelf count invalid");
                    require(named(window, "homeCatalogQualifier")->property("visible").toBool(), "Saved catalog qualifier hidden");
                    require(named(window, "homeLatestSection")->property("visible").toBool(), "Latest section hidden");
                    require(named(window, "homePopularSection")->property("visible").toBool(), "Popular section hidden");
                    if (!model->recentMods().isEmpty()) {
                        auto *recentGrid = qobject_cast<QQuickItem *>(named(window, "homeRecentGrid"));
                        require(recentGrid && recentGrid->isVisible(), "Recently added grid hidden when recentMods is non-empty");
                        window->resize(720, 780);
                        stage = 10; return;
                    }
                    stage = 12;
                }
                if (stage == 10) {
                    auto *page = named(window, "homePage");
                    if (window->width() != 720 || page->property("shelfColumns").toInt() != 3) return;
                    window->resize(1200, 780);
                    stage = 11; return;
                }
                if (stage == 11) {
                    auto *page = named(window, "homePage");
                    if (window->width() != 1200 || page->property("shelfColumns").toInt() != 5) return;
                    stage = 12;
                }
                if (stage == 12) {
                    const bool incomplete = catalog->homeProvenance().value("incomplete").toBool();
                    require(named(window, "homePopularQualifier")->property("visible").toBool() == incomplete, "Popular partial qualifier mismatch");
                    if (catalog->homeProvenance().value("stale").toBool())
                        require(named(window, "homeCatalogQualifier")->property("text").toString().contains("Saved results may be out of date."), "Stale qualifier text missing");
                    QList<QObject *> maskedTiles;
                    for (auto *obj : objects(window))
                        if (obj->objectName() == "homeLatestTile" || obj->objectName() == "homePopularTile")
                            if (obj->property("masked").toBool()) maskedTiles.append(obj);
                    if (qEnvironmentVariable("HOME_CATALOG_EXPECT_MASKED") == "1") {
                        require(!maskedTiles.isEmpty(), "Expected masked Home catalog tile missing");
                        for (auto *tile : maskedTiles) {
                            auto *fallback = named(tile, "previewFallback");
                            require(fallback && fallback->property("visible").toBool() && fallback->property("text").toString() == "Preview hidden", "Masked Home tile did not conceal preview");
                        }
                    }
                    saveCaptures(qEnvironmentVariable("HOME_CATALOG_CAPTURE_720"), qEnvironmentVariable("HOME_CATALOG_CAPTURE_1200"));
                    if (qEnvironmentVariable("HOME_CATALOG_CAPTURE_ONLY") == "1") {
                        timer->stop(); std::puts("PASS UI home catalog capture-only"); QCoreApplication::exit(0); return;
                    }
                    click(window, "homePopularSeeAll");
                    stage = 1; return;
                }
                if (stage == 1) {
                    require(catalog->active() && catalog->route() == "browse", "See all did not open Browse");
                    require(catalog->queryState().value("sort").toString() == "downloadCount", "See all did not preserve Most downloaded sort");
                    if (catalog->busy()) return;
                    require(catalog->model()->count() > 0, "Browse did not load after See all");
                    click(window, "homeTab");
                    ++stage; return;
                }
                if (stage == 2) {
                    if (catalog->homeLoading()) return;
                    require(!catalog->active() && catalog->homeActive() && !catalog->homeLatest().isEmpty(), "Returning to Home did not restore shelves");
                    click(window, "homeLatestTile");
                    ++stage; return;
                }
                if (stage == 3) {
                    if (catalog->route() != "submission" || catalog->busy()) return;
                    require(catalog->active() && catalog->detail().value("state").toString() == "ready", "Clicking Home card did not open CatalogDetails");
                    catalog->back();
                    ++stage; return;
                }
                require(catalog->active() && catalog->route() == "browse", "Back from CatalogDetails did not return to Browse listing");
                timer->stop(); std::puts("PASS UI home catalog populated, see all, and detail back"); QCoreApplication::exit(0);
            } catch (const std::exception &e) { timer->stop(); std::fprintf(stderr, "FAIL UI home catalog: %s\n", e.what()); QCoreApplication::exit(1); }
        });
        timer->start(); return;
    }
    if (scenario == "rich-description") {
        try {
            auto blocks=QJsonDocument::fromJson(R"([{"type":"heading","level":2,"runs":[{"text":"Title","bold":true}]},{"type":"paragraph","runs":[{"text":"<img src=x onerror=bad> & \"quoted\"","color":"#ff0000","link":"https://gamebanana.com/mods/711907"}]},{"type":"list","ordered":true,"items":[[{"text":"One"}],[{"text":"Two"}]]},{"type":"rule"},{"type":"quote","runs":[{"text":"Quote"}]},{"type":"image","url":"https://images.gamebanana.com/banner.png","alt":"Banner","width":860,"height":120}])").array();
            require(CatalogData::richDescriptionValid(blocks),"rich block model rejected");
            auto bad=blocks; auto block=bad[1].toObject(); auto runs=block["runs"].toArray();auto run=runs[0].toObject();run["link"]="javascript:bad";runs[0]=run;block["runs"]=runs;bad[1]=block;
            require(!CatalogData::richDescriptionValid(bad),"unsafe rich link accepted");
            bad=blocks;block=bad[5].toObject();block["url"]="https://evil.test/banner.png";bad[5]=block;
            require(!CatalogData::richDescriptionValid(bad),"foreign image accepted");
            bad=blocks;block=bad[5].toObject();block["url"]="https://puu.sh/KUiaZ/3ae805ca34.png";bad[5]=block;
            require(CatalogData::richDescriptionValid(bad),"public Reze image host rejected");
            bad=blocks;block=bad[1].toObject();block["html"]="<script>bad</script>";bad[1]=block;
            require(!CatalogData::richDescriptionValid(bad),"unknown markup field accepted");
            QJsonArray excessiveRuns;for(int n=0;n<2049;++n)excessiveRuns.append(QJsonObject{{"text","x"}});
            require(!CatalogData::richDescriptionValid(QJsonArray{QJsonObject{{"type","paragraph"},{"runs",excessiveRuns}}}),"run limit not enforced");
            QQmlComponent component(qmlEngine(window));
            component.setData("import QtQuick\nimport Deadlock.Native\nRichDescription { width: 640; controller: null; hidden: true }",QUrl());
            std::unique_ptr<QObject> rich(component.createWithInitialProperties({{"blocks",blocks.toVariantList()}}));
            if (!rich) throw std::runtime_error(component.errorString().toStdString());
            auto *item=qobject_cast<QQuickItem *>(rich.get());require(item,"rich renderer did not create item");item->setParentItem(window->contentItem());
            QCoreApplication::processEvents();
            bool found=false;
            for(auto *object:objects(rich.get())) {
                if(object->objectName()!="richDescriptionText")continue;
                const auto text=object->property("text").toString();
                if(!text.contains("onerror"))continue;
                require(text.contains("&lt;img")&&text.contains("&quot;quoted&quot;"),"provider markup not escaped");
                require(object->property("textFormat").toInt()==4,"renderer is not StyledText");
                QTextDocument document;document.setHtml(text);
                require(document.toPlainText()=="<img src=x onerror=bad> & \"quoted\"","injected markup was interpreted");
                require(object->property("implicitHeight").toReal()>0,"rich text did not lay out");found=true;
            }
            require(found,"rich paragraph was not instantiated");
            std::fprintf(stdout,"PASS rich-description model validation and literal-markup rendering\n");QTimer::singleShot(0, [] { QCoreApplication::exit(0); });
        } catch(const std::exception &e) {std::fprintf(stderr,"FAIL rich-description: %s\n",e.what());QTimer::singleShot(0, [] { QCoreApplication::exit(1); });}
        return;
    }
    if (scenario == "native-close") QGuiApplication::setQuitOnLastWindowClosed(false);
    struct State { QString shareCode; int stage = 0; int ticks = 0; qint64 modified = 0; bool sawLoading = false; bool linkReviewed = false; qint64 linkReviewAt = 0; bool busyMissing = false; bool busyChecked = false; QSet<QString> launchCaptures; };
    const auto state = std::make_shared<State>();
    auto *timer = new QTimer(window);
    timer->setInterval(40);
    QObject::connect(timer, &QTimer::timeout, window, [=] {
        try {
            require(++state->ticks < (qEnvironmentVariableIsSet("UI_NATIVE_LONG_FIXTURE") ? 7500 : (scenario == "native-play" || scenario == "native-play-loss" || scenario == "native-launch-clean" || scenario == "native-launch-held" || scenario == "native-launch-unproven" || scenario == "native-launch-recovered" || scenario == "native-launch-live-cycle" || scenario == "native-launch-game-cycle") ? 650 : 350), "UI check timed out");
            if(scenario=="step6-launch-settings") {
                auto *launch=model->launch(); const QDir root(model->dataDir());
                const auto touch=[&](const QString &name){QFile file(root.filePath(name));require(file.open(QIODevice::WriteOnly),"cannot release private fixture");};
                const bool stored=qEnvironmentVariable("STEP6_MODE")=="lost-stored";
                if(state->stage==0) {
                    if(model->loading()||!launch->canLaunch())return;
                    QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("launch-settings")));
                    ++state->stage;return;
                }
                auto *editor=findNamed(window,"launchArgumentsEditor");if(!editor)return;
                QObject *dialog=editor;
                while(dialog&&dialog->metaObject()->indexOfProperty("draftStamp")<0)dialog=dialog->parent();
                require(dialog,"missing launch settings dialog");
                if(state->stage==1) {
                    if(!editor->property("enabled").toBool()||launch->busy())return;
                    editor->setProperty("text","wanted");
                    if(!dialog->property("canSave").toBool())return;
                    require(QMetaObject::invokeMethod(dialog,"save"),"cannot invoke settings save");
                    ++state->stage;return;
                }
                if(state->stage==2) {
                    if(qEnvironmentVariable("STEP6_MODE")=="open-refusal") {
                        if(launch->settingsSaveState().value("status")!="failed"||!launch->canLaunch()||!model->canPreviewApply())return;
                        require(editor->property("text").toString()=="wanted" && dialog->property("dirty").toBool(),"unsent save discarded the draft");
                        require(dialog->property("canSave").toBool(),"unsent draft cannot be retried");
                        const auto error=named(dialog,"launchSettingsError")->property("text").toString();
                        require(error.contains("Owner admission refused")&&!error.contains("nativeIo:"),"unsent error lost its explanation or exposed an internal code");
                        QFile calls(root.filePath("calls.jsonl"));require(calls.open(QIODevice::ReadOnly),"missing private trace");
                        require(!calls.readAll().contains("native.launch.settings.set"),"unsent save dispatched a write");
                        QFile mode(root.filePath("mode"));require(mode.open(QIODevice::WriteOnly),"cannot release open refusal");mode.write("ordinary");mode.close();
                        require(QMetaObject::invokeMethod(dialog,"save"),"cannot retry retained draft");state->stage=5;return;
                    }
                    if(!QFile::exists(root.filePath("lost")))return;
                    require(!launch->canLaunch()&&!model->canPreviewApply(),"pending save opened Play/Apply");
                    touch("release-writer");model->refresh();++state->stage;return;
                }
                if(state->stage==3) {
                    if(!QFile::exists(root.filePath("read-started"))) {model->refresh();return;}
                    require(!launch->canLaunch()&&!model->canPreviewApply(),"confirmation read opened Play/Apply");
                    touch("release-read");++state->stage;return;
                }
                if(launch->settingsSavePending()||!launch->canLaunch()||!model->canPreviewApply())return;
                const auto mode=qEnvironmentVariable("STEP6_MODE");
                const bool saved=stored||mode=="open-refusal";
                require(editor->property("text").toString()==(saved?"wanted":mode=="stale"?"external":"old"),"editor does not show actual stored arguments");
                require(dialog->property("baselineRevision").toString()==(saved?"1":mode=="stale"?"4":"0"),"editor revision is not authoritative");
                require(!dialog->property("dirty").toBool(),"failed save kept unsaved draft as current arguments");
                require(!dialog->property("saveUnknown").toBool(),"save is still unknown after authoritative read");
                const auto message=dialog->property("message").toString();
                require(saved?message=="Saved.":mode=="stale"?message.contains("reload before saving"):mode.startsWith("refused")?message.contains("Close Deadlock"):message.contains("not saved",Qt::CaseInsensitive),"missing save result or refusal reason");
                const auto displayed=named(dialog,"launchSettingsError")->property("text").toString();
                require(saved?displayed=="Saved.":mode=="stale"?displayed=="Launch settings changed; reload before saving.":mode.startsWith("refused")?displayed=="Close Deadlock before saving.":displayed.contains("not saved",Qt::CaseInsensitive),"displayed refusal explanation differs from the result");
                require(editor->property("enabled").toBool(),"editor did not reopen after read");
                timer->stop();std::puts("PASS STEP6 launch settings authoritative editor");QCoreApplication::exit(0);return;
            }
            if (scenario == "crash-health" || scenario == "crash-bisect-health") {
                if(model->loading() || model->busy())return;
                auto health=model->launch()->health();
                if(state->stage==0 && scenario=="crash-health") {
                    if(!model->canSetEnabled())return;
                    seedGoodLaunch(model,"modded");
                    const auto mods=model->fullProfileOrder();require(!mods.isEmpty(),"crash fixture has no mods");const auto mod=mods.first().toMap();
                    require(model->setModEnabled(mod.value("id").toString(),!mod.value("enabled").toBool()),"fixture toggle was refused");state->stage=10;return;
                }
                if(state->stage==10)state->stage=0;
                else if(state->stage==0 && scenario=="crash-bisect-health" && !model->bisect().value("active").toBool()) { seedGoodLaunch(model,"vanilla");require(model->startBisect(),"fixture bisect did not start");return; }
                if(state->stage==0){health->capture("fixture-exit","modded");health->observe("running");health->observe("exited");state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=1;return;}
                if(QDateTime::currentMSecsSinceEpoch()-state->modified<350)return;
                if(state->stage==1){auto *notice=named(window,"crashNotice");require(notice->property("visible").toBool()&&!notice->property("modal").toBool(),"weak crash evidence did not show a nonmodal notice");click(window,"reviewCrashNoticeButton");capture(window,"crash-question");click(window,"confirmCrashButton");state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=2;return;}
                require(health->report().value("confirmed").toBool(),"crash answer was lost");
                if(scenario=="crash-bisect-health") {
                    require(health->report().value("bisectAnswered").toBool(),"confirmed crash did not answer bisect");
                    capture(window,"crash-bisect-auto-answer");require(health->changeBisectAnswer(),"could not change automatic answer");
                }
                capture(window,"crash-changes");health->dismiss();timer->stop();std::puts("PASS UI crash-health question and confirmed changes");QCoreApplication::exit(0);return;
            }
            if (scenario == "native-install-link" || scenario == "native-install-link-forwarded" || scenario == "native-install-link-invalid") {
                if(model->loading())return;
                if(scenario.endsWith("invalid") || !qEnvironmentVariableIsEmpty("INSTALL_LINK_EXPECT_BLOCKED")) {
                    if(!named(window,"installLinkMessage")->property("visible").toBool())return;
                    require(!model->remote()->visible(),"invalid link opened download review");
                    if(!qEnvironmentVariableIsEmpty("INSTALL_LINK_EXPECT_BLOCKED")) {
                        bool found=false;
                        for(auto *object:objects(named(window,"installLinkMessage")))
                            if(object->property("text").toString()==AppPaths::settingsFolderError())found=true;
                        require(found,"blocked link message missing");
                    }
                } else {
                    if(!qEnvironmentVariableIsEmpty("INSTALL_LINK_AUTO")) {
                        auto *remote=model->remote();
                        const auto seamlessCase=qEnvironmentVariable("INSTALL_LINK_CASE");
                        if(state->stage==0 && seamlessCase=="metadata") {
                            CatalogSummary summary;summary.key={"mod","42"};summary.title="Browse mod";summary.author="Browse author";summary.category="Skins";summary.description="A plain submission description.";
                            CatalogRow row;row.identity=summary.key.identity();row.key=summary.key;row.title=summary.title;row.metadata=summary.presentation();
                            model->catalog()->model()->replace({row},{},{});
                        }

                        if (!state->linkReviewed && !remote->linkedFileId().isEmpty() && !seamlessCase.startsWith("multi")) {
                            if (remote->files().isEmpty()) return;
                            require(remote->visible(), "1-Click link skipped file review");
                            require(remote->jobs().isEmpty(), "1-Click downloaded before confirmation");
                            require(remote->selectedFileIds() == QStringList{remote->linkedFileId()}, "1-Click did not preselect linked file");
                            if (!state->linkReviewAt) state->linkReviewAt=QDateTime::currentMSecsSinceEpoch();
                            if (QDateTime::currentMSecsSinceEpoch()-state->linkReviewAt < 400) return;
                            state->linkReviewed=true;
                            click(window,"downloadSelectedFiles");
                            return;
                        }
                        if(seamlessCase=="batch-auto") {
                            if(state->stage==0) {
                                if(!remote->replacementAvailable() || model->updateCount()!=2)return;
                                model->setAutomaticApply(false);model->setSelectedId("unrelated");
                                require(model->updateAll(),"batch update did not start");state->stage=1;return;
                            }
                            if(state->stage==1) {
                                auto *confirm=findNamed(window,"confirmBatchUpdates");
                                if(!confirm || !confirm->property("visible").toBool())return;
                                require(model->batchUpdateTargets().size()==2,"batch review omitted a target");
                                QMetaObject::invokeMethod(confirm,"clicked");state->stage=2;return;
                            }
                            if(!remote->visible())state->sawLoading=true;
                            else if(state->sawLoading)require(false,"batch reopened an update dialog");
                            if(model->updatingCount()!=0)return;
                            require(!remote->visible(),"batch reopened an update dialog");
                            require(model->selectedId()=="unrelated","batch changed the selected mod");
                            timer->stop();std::puts("PASS UI batch updates after one review");QCoreApplication::exit(0);return;
                        }
                        if(seamlessCase=="known-receipt") {
                            if(state->stage==0) {
                                if(!remote->replacementAvailable())return;
                                model->setSelectedId("page-linked");
                                require(remote->beginReplacement("page-linked","update"),"update did not start");state->stage=1;return;
                            }
                            require(remote->error().isEmpty(),qPrintable(remote->error()));
                            if(state->stage==1) {
                                if(remote->files().isEmpty() || !remote->replacementTargetCurrent())return;
                                const auto file=remote->files().first().toMap();
                                require(remote->startReplacement(file.value("fileKey").toMap().value("fileId").toString(),file.value("candidateFingerprint").toString(),"update",false),"known-receipt download was refused");
                                require(!remote->visible(),"update left its selection dialog open");state->stage=2;return;
                            }
                            if(remote->job().value("state")!="installed" || model->outcomeUnknown())return;
                            require(model->modDetails("page-linked").value("selectedOptionIds").toStringList()==QStringList{QString(32,'d')},"update did not save the replacement");
                            require(model->modDetails("page-linked").value("enabled").toBool(),"update disabled the mod");
                            timer->stop();std::puts("PASS UI known-receipt update finishes automatically");QCoreApplication::exit(0);return;
                        }
                        if(seamlessCase.startsWith("browse-linked")) {
                            if(state->stage==0) {
                                if(!remote->canResolve())return;
                                require(remote->resolve({{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}}),"Browse resolve failed");
                                state->stage=1;return;
                            }
                            if(seamlessCase=="browse-linked-refused") {
                                if(state->stage==2) {
                                    if(!remote->canResolve())return;
                                    require(remote->resolve({{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}}),"second Browse resolve failed");
                                    state->stage=3;return;
                                }
                                if(remote->error().isEmpty())return;
                                if(qEnvironmentVariableIsSet("UI_BROWSE_REPEAT") && state->stage==1) {
                                    state->shareCode=remote->error();
                                    auto *dismiss=named(window,"dismissDownloadBubble");QMetaObject::invokeMethod(dismiss,"clicked");
                                    require(!named(window,"downloadBubbles")->property("visible").toBool(),"refusal was not dismissed");
                                    remote->showDownloads();remote->close();state->stage=2;return;
                                }
                                if(qEnvironmentVariableIsSet("UI_BROWSE_REPEAT") && remote->error()!=state->shareCode)return;
                                auto *message=named(window,"downloadBubbleMessage");
                                require(message && message->property("visible").toBool(),"Rejected auto start has no error bubble");
                                require(!message->property("text").toString().isEmpty(),"Rejected auto start has no explanation");
                                std::puts("PASS UI Browse refusal remains visible in download bubble");QCoreApplication::exit(0);timer->stop();return;
                            }
                            require(remote->error().isEmpty(),qPrintable(remote->error()));
                            if(remote->job().value("state")!="installed" || model->outcomeUnknown())return;
                            const auto saved=model->modDetails("page-linked");
                            require(saved.value("enabled").toBool(),"Browse download disabled local mod");
                            require(saved.value("optionCount").toInt()==3,"Browse download replaced existing choices");
                            require(saved.value("selectedOptionIds").toStringList()==QStringList{QString(32,'c')},"Browse download changed the saved choice");
                            std::puts("PASS UI Browse adds a choice to a page-linked mod");QCoreApplication::exit(0);timer->stop();return;
                        }
                        if(seamlessCase=="legacy-link") {
                            require(remote->error().isEmpty(),qPrintable(remote->error()));
                            if(state->stage==0) {
                                if(!remote->replacementAvailable())return;
                                model->setSelectedId("page-linked");
                                require(remote->beginReplacement("page-linked","link"),"Legacy Link did not start");
                                state->stage=1;return;
                            }
                            if(!remote->replacementTargetCurrent())return;
                            require(!remote->hasResolution(),"Compatibility page identity bypassed explicit Link selection");
                            std::puts("PASS UI legacy Link keeps explicit submission selection");QCoreApplication::exit(0);timer->stop();return;
                        }
                        if(seamlessCase.startsWith("page-linked-bulk")) {
                            if(state->stage==0){
                                if(!remote->replacementAvailable() || model->updateCount()!=1)return;
                                model->setSelectedId("unrelated");
                                require(model->updateAll(),"Update all did not start");state->stage=1;return;
                            }
                            if(qEnvironmentVariableIntValue("UI_BATCH_ATTENTION") && state->stage==2) {
                                auto *detail=findNamed(window,"replacementVersionState");
                                if(!detail || !detail->property("visible").toBool())return;
                                auto *close=named(window,"closeDownloadReview");
                                require(close->property("text")==QStringLiteral("Cancel remaining"),"batch cancellation is not explicit");
                                QMetaObject::invokeMethod(close,"clicked");state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=3;return;
                            }
                            if(qEnvironmentVariableIntValue("UI_BATCH_ATTENTION") && state->stage==3) {
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<1300)return;
                                require(!remote->visible() && model->updatingCount()==0,"cancelled batch reopened");
                                timer->stop();std::puts("PASS UI batch attention and cancellation");QCoreApplication::exit(0);return;
                            }
                            if(!remote->replacementTargetCurrent())return;
                            QObject *dialog=nullptr;
                            for(auto *object:objects(window))if(object->inherits("QQuickPopup") && object->property("visible").toBool()
                                && (object->property("title")==QStringLiteral("Downloads") || object->property("title").toString().startsWith("Update ")))dialog=object;
                            if(!dialog)return;
                            const auto expected=seamlessCase.endsWith("inactive")?QStringLiteral("Update Linked local mod · default"):QStringLiteral("Update Linked local mod");
                            require(model->selectedId()=="unrelated","batch changed the selected mod");
                            require(dialog->property("title")==expected,"Update dialog did not identify its target and profile");
                            bool plainTitle=false;for(auto *object:objects(dialog))if(object->property("text")==expected && object->property("textFormat").toInt()==int(Qt::PlainText))plainTitle=true;
                            require(plainTitle,"Update title is not plain text");
                            if(qEnvironmentVariableIntValue("UI_BATCH_ATTENTION")) {
                                auto *confirm=findNamed(window,"confirmBatchUpdates");if(!confirm)return;
                                QMetaObject::invokeMethod(confirm,"clicked");state->stage=2;return;
                            }
                            timer->stop();std::puts("PASS UI Update all target title");QCoreApplication::exit(0);return;
                        }
                        if(seamlessCase.startsWith("page-linked")) {
                            if(state->stage==0) {
                                if(!remote->replacementAvailable())return;
                                model->setSelectedId("page-linked");
                                require(QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("library"))),"cannot show library");
                                if(!model->selectedMod().value("update").toMap().value("versionUnknown").toBool())return;
                                bool unknown=false;
                                for(auto *o:objects(window))if(o->property("visible").toBool() && o->property("text")==QStringLiteral("The installed version could not be identified. Update to the latest file."))unknown=true;
                                require(unknown,"Library update check hid the version-unknown state");
                                require(remote->beginReplacement("page-linked",seamlessCase.endsWith("multi")?"check":"update"),"page-linked Update did not start");
                                state->stage=1;return;
                            }
                            require(remote->error().isEmpty(),qPrintable(remote->error()));
                            if(state->stage==1) {
                                if(remote->files().size()!=(seamlessCase.endsWith("multi")?2:1))return;
                                bool unknown=false;
                                for(auto *o:objects(window))if(o->property("visible").toBool() && o->property("text")==QStringLiteral("Version unknown, update to the latest file"))unknown=true;
                                require(unknown,"page-linked update did not show version unknown");
                                auto *all=named(window,"selectAllDownloads");
                                require(all->property("visible").toBool(),"Update has no file chooser");
                                if (!all->property("checked").toBool()) {
                                    QMetaObject::invokeMethod(all,"toggle");QMetaObject::invokeMethod(all,"toggled");
                                }
                                state->stage=2;state->modified=QDateTime::currentMSecsSinceEpoch();return;
                            }
                            if(state->stage==2) {
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<350)return;
                                capture(window,"page-linked-update-files");
                                auto *download=named(window,"downloadSelectedFiles");
                                require(download->property("enabled").toBool(),"Update selection cannot download");
                                QMetaObject::invokeMethod(download,"clicked");state->stage=3;state->modified=0;return;
                            }
                            if(state->stage==3 && remote->job().value("state")=="transferring") {
                                require(!remote->visible(),"Update left a modal open while downloading");
                                auto *bubble=qobject_cast<QQuickItem *>(named(window,"downloadBubbles"));
                                require(bubble && bubble->width()<=320,"download bubble is too wide");
                                QString progressError;
                                require(progressAccessibilityMatches(named(bubble,"downloadProgress"),progressError),qPrintable(progressError));
                                if(!window->property("bubbleProgressChecked").toBool()) {
                                    window->setProperty("bubbleProgressChecked",true);
                                    std::puts("PASS UI bubble progress accessibility role and value");
                                }
                                const auto bubbleRect=bubble->mapRectToScene(QRectF(0,0,bubble->width(),bubble->height()));
                                for(const auto *name:{"modPageButton","modActionsButton","modUpdateButton"}) {
                                    auto *button=qobject_cast<QQuickItem *>(named(window,name));
                                    if(button && button->isVisible())require(!bubbleRect.intersects(button->mapRectToScene(QRectF(0,0,button->width(),button->height()))),"download bubble covers inspector actions");
                                }
                                if(!state->modified)state->modified=QDateTime::currentMSecsSinceEpoch();
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<200)return;
                                capture(window,"page-linked-update-bubble");state->stage=4;return;
                            }
                            if(seamlessCase=="page-linked-variants" && state->stage==4) {
                                if(remote->candidates().size()!=2)return;
                                require(remote->visible(),"Update archive variants did not open a chooser");
                                require(remote->selectedIds().isEmpty(),"Update archive variants were guessed");
                                capture(window,"page-linked-update-variants");
                                remote->setSelectedIds({remote->candidates().first().toMap().value("candidateId").toString()});
                                auto *choose=named(window,"addDownloadSelection");
                                require(choose->property("visible").toBool() && choose->property("enabled").toBool(),"Update archive choice button unavailable");
                                require(QMetaObject::invokeMethod(choose,"clicked"),"Update archive choice rejected");
                                state->stage=5;return;
                            }
                            if(state->stage==5 && remote->review().value("phase")=="ready")
                                for(auto *o:objects(window))require(!o->property("visible").toBool() || o->property("text")!=QStringLiteral("Save file choices"),"Update archive choice still requires a manual save");
                            if(remote->job().value("state")!="installed" || model->outcomeUnknown())return;
                            const auto saved=model->modDetails("page-linked");
                            if(saved.value("optionCount").toInt()!=((seamlessCase.endsWith("multi") || seamlessCase=="page-linked-variants")?2:1))return;
                            require(saved.value("enabled").toBool(),"Update disabled the existing mod");
                            require(saved.value("title")=="Linked local mod","Update lost local metadata");
                            require(saved.value("selectedOptionIds").toStringList()==QStringList{QString(32,'d')},"Update retained the old file choice");
                            require(!remote->visible(),"Update left a modal open");
                            require(window->property("bubbleProgressChecked").toBool(),"Update completed without exercising accessible transferring progress");
                            if(qEnvironmentVariableIsSet("UI_DETAILS_CAPTURE") && seamlessCase=="page-linked") {
                                if(state->stage<6){state->stage=6;state->modified=QDateTime::currentMSecsSinceEpoch();return;}
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<500)return;
                                if(state->stage==6) {
                                    for(auto *o:objects(window))if(o->inherits("QQuickPopup") && o->property("visible").toBool()){
                                        realKey(window,Qt::Key_Escape);state->modified=QDateTime::currentMSecsSinceEpoch();return;
                                    }
                                    auto *check=qobject_cast<QQuickItem *>(named(window,"modCheckUpdatesButton"));
                                    require(check->isVisible() && check->isEnabled(),"visible update check is unavailable in the native inspector");
                                    realClickItem(window,check);state->stage=9;state->modified=QDateTime::currentMSecsSinceEpoch();return;
                                }
                                if(state->stage==9) {
                                    if(model->updatesChecking() || model->busy() || model->loading())return;
                                    require(!remote->visible(),"Check for updates opened a replacement chooser");
                                    capture(window,"details-native-wide");window->resize(720,780);
                                    state->stage=7;state->modified=QDateTime::currentMSecsSinceEpoch();return;
                                }
                                if(state->stage==7) {
                                    auto *title=qobject_cast<QQuickItem *>(findNamed(window,"rowTitle"));
                                    if(!title || !title->isVisible())return;
                                    realClickItem(window,title);state->stage=8;state->modified=QDateTime::currentMSecsSinceEpoch();return;
                                }
                                bool opened=false;
                                for(auto *o:objects(window))if(o->property("title")==QStringLiteral("Mod details") && o->property("opened").toBool())opened=true;
                                if(!opened)return;
                                capture(window,"details-native-narrow");
                            }
                            capture(window,"page-linked-updated");
                            std::puts("PASS UI page-linked Update preserves enabled identity and replaces choices");QCoreApplication::exit(0);timer->stop();return;
                        }
                        if(QStringList{"bubble","multi","choice-once","bubble-error","multi-changed"}.contains(seamlessCase)) {
                            if(state->stage==0) {
                                if(seamlessCase.startsWith("multi")) {
                                    if(remote->files().size()!=2)return;
                                    require(remote->visible(),"multi-file picker missing");
                                    require(remote->selectedFileIds()==QStringList{remote->linkedFileId()},"multi-file link did not preselect linked file");
                                    if(!state->modified)state->modified=QDateTime::currentMSecsSinceEpoch();
                                    if(QDateTime::currentMSecsSinceEpoch()-state->modified<400)return;
                                    auto *all=named(window,"selectAllDownloads");
                                    require(all->property("visible").toBool(),"multi-select control missing");
                                    QMetaObject::invokeMethod(all,"toggle");
                                    QMetaObject::invokeMethod(all,"toggled");
                                    state->stage=10;state->modified=QDateTime::currentMSecsSinceEpoch();return;
                                } else {
                                    if(remote->files().isEmpty() && remote->jobs().isEmpty())return;
                                    require(!remote->visible(),"single file opened a modal instead of downloading");
                                }
                                state->stage=1;return;
                            }
                            if(seamlessCase.startsWith("multi") && state->stage==10) {
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<250)return;
                                capture(window,"multi-file-picker");
                                auto *download=named(window,"downloadSelectedFiles");
                                require(download->property("enabled").toBool(),"selected files cannot download");
                                QMetaObject::invokeMethod(download,"clicked");state->stage=1;state->modified=0;return;
                            }
                            if(seamlessCase=="multi-changed" && state->stage==1) {
                                if(remote->error().isEmpty())return;
                                require(remote->visible(),"changed file did not return to selection");
                                require(model->modDetails("installed-variant").value("optionCount").toInt()==1,"changed file was installed");
                                capture(window,"files-changed");std::puts("PASS UI changed queued file requires selection");QCoreApplication::exit(0);timer->stop();return;
                            }
                            if(seamlessCase=="bubble-error" && state->stage==1) {
                                if(remote->error().isEmpty())return;
                                if(!qEnvironmentVariableIntValue("UI_BUBBLE_DISMISS")) require(named(window,"downloadBubbleMessage")->property("text")==QStringLiteral("The selected files are no longer available. Choose files again."),"invalid-selection error was not explained");
                                if(qEnvironmentVariableIntValue("UI_BUBBLE_DISMISS")) {
                                    auto *dismiss=findNamed(window,"dismissDownloadBubble");
                                    require(dismiss && dismiss->property("visible").toBool(),"error bubble dismiss missing");
                                    const auto jobs=remote->jobs().size();
                                    QMetaObject::invokeMethod(dismiss,"clicked");
                                    require(!named(window,"downloadBubbles")->property("visible").toBool(),"dismissed error still covers the app");
                                    require(remote->jobs().size()==jobs && jobs>0,"dismissing notice removed download history");
                                    if(qEnvironmentVariableIsSet("UI_BUBBLE_REPEAT")) {
                                        state->shareCode=remote->error();
                                        require(remote->retryInstall(remote->currentJobId()),"retry was rejected");
                                        state->stage=20;return;
                                    }
                                    std::puts("PASS UI dismiss download error keeps history");QCoreApplication::exit(0);timer->stop();return;
                                }
                                auto *retry=named(window,"retryDownloadBubble");
                                require(retry->property("visible").toBool(),"error bubble retry missing");
                                QMetaObject::invokeMethod(retry,"clicked");state->stage=2;return;
                            }
                            if(seamlessCase=="bubble-error" && state->stage==20) {
                                if(remote->error()!=state->shareCode)return;
                                require(named(window,"downloadBubbles")->property("visible").toBool(),"identical retry failure was hidden");
                                std::puts("PASS UI identical retry error is visible");QCoreApplication::exit(0);timer->stop();return;
                            }
                            require(remote->error().isEmpty(),qPrintable(remote->error()));
                            if(seamlessCase=="bubble" && state->stage==1) {
                                if(remote->jobs().isEmpty() || remote->jobs().first().toMap().value("state")!="transferring")return;
                                if(remote->progress().value("total").toString().isEmpty())return;
                                require(named(window,"downloadBubbles")->property("visible").toBool(),"download bubble missing");
                                if(!state->modified)state->modified=QDateTime::currentMSecsSinceEpoch();
                                if(QDateTime::currentMSecsSinceEpoch()-state->modified<350)return;
                                capture(window,"bubble-downloading");
                                realClick(window,"homeTab");
                                require(named(window,"downloadBubbles")->property("visible").toBool(),"bubble lost on navigation");
                                state->stage=2;state->modified=0;return;
                            }
                            if(seamlessCase=="choice-once" && state->stage==1) {
                                if(remote->candidates().size()!=2)return;
                                require(remote->visible(),"variant chooser missing");
                                require(remote->selectedIds().isEmpty(),"archive variants selected automatically");
                                remote->setSelectedIds({remote->candidates().first().toMap().value("candidateId").toString()});
                                require(remote->addSelection(remote->installMetadata(remote->currentJobId())),"variant choice rejected");
                                state->stage=2;return;
                            }
                            const auto saved=model->modDetails("installed-variant");
                            if(saved.isEmpty() || model->outcomeUnknown())return;
                            if(seamlessCase=="multi" && saved.value("optionCount").toInt()!=2) {model->setQuery("hidden-by-search");return;}
                            if(remote->job().value("state")!="installed")return;
                            require(!remote->visible(),"finished install left a modal open");
                            if(!state->modified)state->modified=QDateTime::currentMSecsSinceEpoch();
                            if(QDateTime::currentMSecsSinceEpoch()-state->modified<(seamlessCase=="choice-once"?2000:350))return;
                            capture(window,"bubble-added");
                            if(seamlessCase=="bubble") {
                                require(!named(window,"downloadBubbles")->property("visible").toBool(),"completed download bubble retained");
                                require(remote->jobs().isEmpty(),"completed download remains in active work");
                                std::puts("PASS UI download bubble clears after library addition");QCoreApplication::exit(0);timer->stop();return;
                            }
                            std::puts("PASS UI seamless download");QCoreApplication::exit(0);timer->stop();return;
                        }
                        if(state->stage==0) {
                            if(remote->jobs().isEmpty())return;
                            if(qEnvironmentVariable("INSTALL_LINK_CASE")=="rebase")model->refresh();
                            state->stage=1;return;
                        }
                        const auto installCase=qEnvironmentVariable("INSTALL_LINK_CASE");
                        if(installCase=="closed" && state->stage==1) {
                            if(remote->prepareKey().isEmpty())return;
                            remote->close();
                            state->stage=2;return;
                        }
                        if(installCase=="review-failure") {
                            if(remote->error().isEmpty())return;
                            require(model->modDetails("installed-variant").isEmpty(),"failed review installed a mod");
                            require(!remote->canCommit(),"failed review can commit");
                            require(named(window,"retryDownloadBubble")->property("visible").toBool(),"error bubble retry is missing");
                            if(state->stage++<10)return;
                            capture(window,"downloads-failed");
                            std::puts("PASS UI failed review stops automatic install with manual continuation");QCoreApplication::exit(0);timer->stop();return;
                        }
                        require(remote->error().isEmpty(),qPrintable(remote->error()));
                        if(qEnvironmentVariableIntValue("INSTALL_LINK_CANDIDATES")==2) {
                            if(remote->candidates().size()!=2)return;
                            require(remote->selectedIds().isEmpty(),"multiple variants selected automatically");
                            require(remote->review().isEmpty(),"multiple variants reviewed automatically");
                            require(named(window,"addDownloadSelection")->property("visible").toBool(),"variant chooser missing");
                            require(!named(window,"addDownloadSelection")->property("enabled").toBool(),"empty choice can install");
                            require(remote->installMetadata(remote->currentJobId()).value("name")=="Variants","chooser name was lost");
                            if(state->stage++<5)return;
                            capture(window,"downloads-variants");
                        } else {
                            if(model->outcomeUnknown())return;
                            const auto item=model->modDetails("installed-variant");
                            if(item.isEmpty())return;
                            require(!item.value("enabled").toBool(),"install enabled mod");
                            if(installCase=="rebase")require(model->revision()=="3","install did not rebase against the changed library");
                            require(item.value("title").toString()==(installCase=="metadata" ? "Browse mod" : "Variants"),"install name was lost");
                            if(installCase=="metadata") {
                                require(item.value("author")=="Browse author","submission author was lost");
                                require(item.value("kind")=="Skins","submission category was lost");
                            }
                            capture(window,"downloads-added");
                        }
                        std::puts("PASS UI install automatic continuation");QCoreApplication::exit(0);timer->stop();return;
                    }
                    if(!model->remote()->hasResolution() || !model->remote()->visible() || model->remote()->phase()!=QStringLiteral("Choose a file to install."))return;
                    bool found=false;
                    for(auto *object:objects(window))if(object->inherits("QQuickPopup") && object->property("title")==QStringLiteral("Select files to download") && object->property("visible").toBool())found=true;
                    require(found,"download review popup missing");
                    bool descriptionFound=false;
                    for(auto *object:objects(window)) if(object->objectName()==QStringLiteral("remoteFileDescription") && object->property("visible").toBool()) {
                        require(object->property("text")==QStringLiteral("reze_human"),"description text lost");
                        require(object->property("maximumLineCount").toInt()==3,"description is not bounded to three lines");
                        descriptionFound=true;
                    }
                    require(descriptionFound,"file description missing");

                    require(model->remote()->linkedFileId()==QStringLiteral("12"),"linked file hint lost");
                    require(model->remote()->jobs().isEmpty(),"link started a download without review");
                }
                std::puts("PASS UI install link review");QCoreApplication::exit(0);timer->stop();return;
            }
            if (model->loading()) {
                if (scenario.startsWith("native-")) workflowControlsDisabled(window);
                if (findNamed(window, "libraryView")) {
                    auto *indicator = findNamed(window, "loadingIndicator");
                    if (!indicator) return;
                    require(indicator->property("running").toBool(), "loading state is hidden");
                }
                state->sawLoading = true;
                return;
            }
            if (scenario == "profile-share") {
                if (model->busy()) return;
                if (state->stage == 0) {
                    require(model->nativeMode(), "share UI requires an isolated native fixture");
                    openProfilesViaChooser(window); state->stage = 1; return;
                }
                if (state->stage == 1) { click(window, "shareProfileButton"); state->stage = 2; return; }
                if (state->stage == 3) {
                    for (auto *object : objects(window)) if (object->objectName() == "profileShareController") return;
                    click(window, "importSharedProfileButton"); state->stage = 4; return;
                }
                if (!named(window, "profileShareDialog")->property("opened").toBool()) return;
                auto *controller = named(window, "profileShareController");
                if (controller->property("busy").toBool()) return;
                if (state->stage == 2) {
                    require(controller->property("error").toString().isEmpty(), "profile export failed");
                    const auto code = controller->property("result").toMap().value("code").toString();
                    require(code.startsWith("DLMM1-"), "export format missing");
                    if (!named(window, "sharedProfileThumbnail")->property("ready").toBool() && state->ticks < 120) return;
                    state->shareCode = code;
                    capture(window, "profile-share-export");
                    QMetaObject::invokeMethod(named(window, "profileShareDialog"), "close");
                    state->stage = 3; return;
                }
                if (state->stage == 4) {
                    named(window, "profileShareCode")->setProperty("text", state->shareCode);
                    click(window, "reviewSharedProfileButton"); state->stage = 5; return;
                }
                if (state->stage == 5) {
                    require(controller->property("error").toString().isEmpty(), "share import review failed");
                    const auto review = controller->property("result").toMap();
                    require(!review.value("rows").toList().isEmpty(), "share review row missing");
                    require(!review.value("rows").toList().first().toMap().value("title").toString().isEmpty(), "share review title missing");
                    require(!review.value("canCreate").toBool(), "missing file allowed profile creation");
                    require(!named(window, "createSharedProfileButton")->property("enabled").toBool(), "create action ignores missing file");
                    require(model->remote()->jobs().isEmpty(), "share review started an unreviewed download");
                    if (!named(window, "sharedProfileThumbnail")->property("ready").toBool() && state->ticks < 240) return;
                    capture(window, "profile-share-import-review");
                    named(window, "profileShareCode")->setProperty("text", "invalid");
                    require(controller->property("result").toMap().isEmpty(), "edited code retained review");
                    click(window, "reviewSharedProfileButton"); state->stage = 6; return;
                }
                require(!controller->property("error").toString().isEmpty(), "malformed profile accepted");
                timer->stop(); std::puts("PASS UI profile share export, missing-file review, edit invalidation and malformed input"); QCoreApplication::exit(0); return;
            }
            if (scenario == "library-filters") {
                auto *category = named(window, "categoryChooser");
                if (state->stage == 0) {
                    require(model->demoMode() && model->count() == 240 && model->typeFilter().isEmpty(), "wrong filter fixture");
                    capture(window, "filters-all-1200x780");
                    realClick(window, "modSearch"); realKey(window, Qt::Key_Tab);
                    require(qobject_cast<QQuickItem *>(category)->hasActiveFocus(), "category is not next to search in keyboard order");
                    for (const auto *next : {"heroChooser", "enabledFilter", "needsAttentionFilter", "updatesFilter", "listViewButton", "gridViewButton"}) {
                        realKey(window, Qt::Key_Tab);
                        require(qobject_cast<QQuickItem *>(named(window,next))->hasActiveFocus(), qPrintable(QString("keyboard order skipped %1").arg(next)));
                    }
                    realClick(window, "modSearch"); realKey(window, Qt::Key_Tab);
                    chooseCategory(window, model->types().indexOf("Skins") + 1);
                    state->stage = 1; return;
                }
                if (state->stage == 1) {
                    require(model->typeFilter() == "Skins" && model->count() == 80, "keyboard category selection did not filter rows");
                    capture(window, "filters-skins-1200x780");
                    model->loadDemoData(240);
                    require(model->typeFilter() == "Skins" && model->count() == 80, "refresh discarded a valid category");
                    realClick(window, "heroChooser"); realKey(window, Qt::Key_Home);
                    for (int i = 0; i <= model->heroes().indexOf("Abrams"); ++i) realKey(window, Qt::Key_Down);
                    realKey(window, Qt::Key_Return);
                    require(model->heroFilter() == "Abrams" && model->count() > 0 && model->count() < 80, "hero and type filters did not combine");
                    realClick(window, "heroChooser"); realKey(window, Qt::Key_Home); realKey(window, Qt::Key_Return);
                    require(model->heroFilter().isEmpty() && model->count() == 80, "resetting hero changed type filter");
                    realText(window, "modSearch", "Classic");
                    require(model->count() == 40, "search did not combine with category");
                    realClick(window, "enabledFilter");
                    require(model->count() == 32 && model->enabledOnly(), "included-only did not combine with search and category");
                    realClick(window, "modSearch"); realKey(window, Qt::Key_Escape);
                    require(model->query().isEmpty() && model->count() == 64, "Escape search reset discarded another filter");
                    realClick(window, "enabledFilter");
                    window->resize(800, 600); state->stage = 2; return;
                }
                if (state->stage == 2) {
                    require(window->width() == 800 && window->height() == 600 && model->count() == 80, "narrow category state differs");
                    capture(window, "filters-skins-800x600");
                    realClick(window, "categoryChooser"); state->stage = 3; return;
                }
                if (state->stage == 3) {
                    realKey(window, Qt::Key_Home); realKey(window, Qt::Key_Return);
                    require(model->typeFilter().isEmpty() && model->count() == 240, "All mods did not reset category");
                    state->stage = 4; return;
                }
                if (state->stage == 4) {
                    capture(window, "filters-all-800x600");
                    chooseCategory(window, model->types().indexOf("Skins") + 1);
                    model->loadDemoData(0); state->stage = 5; return;
                }
                require(model->typeFilter().isEmpty() && category->property("currentIndex").toInt() == 0,
                        "data change left a hidden category filter");
                model->loadDemoData(240);
                require(model->count() == 240, "restored data retained a hidden category filter");
                timer->stop(); std::puts("PASS UI real category click/key filters and reset"); QCoreApplication::exit(0); return;
            }
            if (scenario.startsWith("local-")) {
                auto *local=model->localImport();
                if(state->stage==0) {
                    if(!local->canPrepare()) return;
                    require(named(window,"localFilesButton")->property("enabled").toBool(),"capability arrival left local files disabled");
                    capture(window,"local-before");
                    realDrop(window,qEnvironmentVariableIsEmpty("UI_LOCAL_SOURCE") ? QUrl("file:///synthetic/a%20space.vpk") : QUrl::fromLocalFile(qEnvironmentVariable("UI_LOCAL_SOURCE")));
                    require(local->busy(),"drop did not prepare local content"); state->stage=1; return;
                }
                if(state->stage==1) {
                    realText(window,"modSearch","typed while preparing"); require(model->query()=="typed while preparing","preparation blocked real text entry"); realText(window,"modSearch","");
                    if(scenario=="local-cancel") { capture(window,"local-progress"); realClick(window,"localCancelButton"); state->stage=8; return; }
                    if(local->phase()!="ready") return;
                    realClick(window,"localReviewButton"); state->stage=2; return;
                }
                if(state->stage==2) {
                    if(!local->presentationReady() || !localReviewPopup(window) || !localReviewPopup(window)->property("opened").toBool()) return;
                    if (scenario.startsWith("local-artwork")) {
                        auto *preview=named(window,"localArtworkPreview"); auto *image=named(preview,"previewImage");
                        const auto expected=scenario.endsWith("broken") ? 3 : 1;
                        if(image->property("status").toInt()!=expected) return;
                        if(expected==3) require(named(preview,"previewFallback")->property("text").toString()=="Preview unavailable","local artwork decode error is not visible");
                        else renderedColor(window,preview,QColor(210,45,65));
                        capture(window,"local-artwork");
                    }
                    require(!named(window,"localCommitButton")->property("enabled").toBool(),"unconfirmed selection was importable");
                    realText(window,"localField_name","Keyboard family"); require(local->metadataDraft().value("name")=="Keyboard family","real typing did not update draft");
                    realClick(window,"localHideReview"); state->stage=3; return;
                }
                if(state->stage==3) { if(localReviewPopup(window)) return; realClick(window,"localReviewButton"); state->stage=4; return; }
                if(state->stage==4) {
                    if(!localReviewPopup(window) || !localReviewPopup(window)->property("opened").toBool()) return;
                    require(named(window,"localField_name")->property("text")=="Keyboard family","hidden review lost dirty form");
                    state->stage=5; return;
                }
                if(state->stage==5) {
                    if(!local->canCommit()) return;
                    require(named(window,"localCommitButton")->property("enabled").toBool(),"review result left import control disabled");
                    if (!window->property("localNarrowCaptured").toBool()) {
                        require(window->width()==1200 && window->height()==780,"normal capture geometry differs");
                        capture(window,"local-review"); window->setProperty("localNarrowCaptured",true); window->resize(800,600); return;
                    }
                    require(window->width()==800 && window->height()==600,"narrow capture geometry differs");
                    capture(window,"local-review-narrow"); realClick(window,"localCommitButton");
                    require(model->writeBusy() && !local->canCancel(),"commit did not reserve central write gate"); state->stage=6; return;
                }
                if(state->stage==6) {
                    if(model->writeBusy()) { capture(window,"local-finishing"); return; }
                    if(model->outcomeUnknown()) { require(!named(window,"localDismissButton")->property("enabled").toBool(),"unresolved job Dismiss remained enabled"); capture(window,"local-error"); if(!named(window,"checkOperationButton")->property("enabled").toBool() || !named(window,"checkOperationButton")->property("visible").toBool())return; realClick(window,"checkOperationButton"); require(model->busy(),"Check status did not start reconciliation"); state->stage=7; return; }
                    state->stage=7;
                }
                if(state->stage==7) {
                    if(model->writeBusy() || model->outcomeUnknown()) return;
                    require(local->phase()=="succeeded","local import did not complete");
                    if (!qEnvironmentVariableIsEmpty("UI_LOCAL_SOURCE")) require(model->selectedId().startsWith("local-") && !model->selectedMod().value("enabled").toBool(),"imported local mod was not shown disabled");
                    capture(window,"local-success");
                }
                if(state->stage==8) {
                    if(local->phase()!="cancelled") { capture(window,"local-cancel-requested"); return; }
                    capture(window,"local-cancelled");
                    realClick(window,"localDismissButton"); state->stage=9; return;
                }
                if(state->stage==9 && !local->jobId().isEmpty()) return;
                timer->stop(); std::puts("PASS UI real local click/key/drop"); std::fflush(stdout); QCoreApplication::exit(0); return;
            }
            if (scenario.startsWith("native-")) {
                if(auto *popup=profilePopup(window); popup && !popup->property("opened").toBool()) return;
                if (scenario == "native-auto-game-toggle" || scenario == "native-auto-game-off" || scenario == "native-auto-game-quit") {
                    auto *launch = model->launch();
                    if (state->stage == 0) {
                        if (launch->gameStatus().value("state") != "running" || model->loading() || model->busy() || launch->busy()) return;
                        if (!state->modified) { state->modified = QDateTime::currentMSecsSinceEpoch(); return; } if (QDateTime::currentMSecsSinceEpoch() - state->modified < 1000) return;
                        if (scenario == "native-auto-game-quit") { model->setModEnabled("local-a", true); std::puts("PASS UI quit while game runs"); std::fflush(stdout); timer->stop(); window->close(); return; }
                        if (scenario == "native-auto-game-off") model->setAutomaticApply(false); model->setSelectedId("local-a");
                        require(!named(named(window, "modDetails"), "modSwitch")->property("enabled").toBool(), "inclusion switch enabled while Deadlock runs");
                        require(!model->canToggleEnabled() && !model->canSetEnabled() && !model->canPreviewApply(), "mod editing is available while Deadlock runs");
                        require(!model->profileEditsAvailable() && !model->remoteAdmissionAvailable(), "profile edits or replacements are available while Deadlock runs");
                        require(!model->setModEnabled("local-a", true), "toggle accepted while Deadlock runs");
                        auto *running = named(window, "launchActiveButton");
                        require(running->property("visible").toBool() && running->property("text").toString() == "Running", "running game header status missing");
                        require(model->actionUnavailableReason() == "Close Deadlock to change your mods.", "running game edit refusal missing");
                        QFile ready(qEnvironmentVariable("UI_RUNNING_EDIT_READY")); require(ready.open(QIODevice::WriteOnly), "cannot signal game observation"); ready.close(); state->stage = 1; return;
                    }
                    if (launch->gameStatus().value("state") != "idle" || !model->canToggleEnabled()) return;
                    if (state->stage == 1) { require(named(window, "launchActiveButton")->property("text").toString() != "Running" && model->actionUnavailableReason() != "Close Deadlock to change your mods.", "running game status remains after exit"); toggleIncluded(window, true); state->stage = 2; return; }
                    if (!model->canSetEnabled() || model->busy() || model->loading() || !model->modDetails("local-a").value("enabled").toBool() || model->pendingChanges() != (scenario == "native-auto-game-off")) return;
                    require(model->applyReview().isEmpty(), "automatic Apply requested an unexpected review");
                    timer->stop(); std::puts("PASS UI game exit allows immediate editing"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-variant") {
                    auto *options=model->options();
                    if(state->stage==0) {
                        if(!options->available())return;
                        if(qEnvironmentVariable("UI_VARIANT_CASE")=="variant-off")model->setAutomaticApply(false);
                        if(qEnvironmentVariable("UI_VARIANT_CASE")=="variant") {
                            click(window,"libraryTab");
                            const auto id=qEnvironmentVariable("UI_VARIANT_MOD");
                            if(model->selectedId()!=id){model->setSelectedId(id);return;}
                            auto *button=qobject_cast<QQuickItem *>(findNamed(window,"filesOptionsButton"));
                            if(!button || !button->isVisible() || !button->isEnabled())return;
                            realClickItem(window,button);
                            require(options->visible(),"Files and options did not open the editor");
                            state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=10;return;
                        }
                        require(options->open(qEnvironmentVariable("UI_VARIANT_MOD")),"variant editor did not open");state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=1;return;
                    }
                    if(state->stage==10) {
                        require(QDateTime::currentMSecsSinceEpoch()-state->modified<8000,"options editor did not become ready for focus check");
                        if(!options->editable() || options->selectedIds().size()!=2)return;
                        realKey(window,Qt::Key_Escape);state->stage=11;return;
                    }
                    if(state->stage==11) {
                        require(QDateTime::currentMSecsSinceEpoch()-state->modified<8000,"options close did not restore the invoking button's focus");
                        if(options->visible())return;
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"filesOptionsButton"));
                        if(!button || !button->hasActiveFocus())return;
                        require(!named(window,"modActionsButton")->property("activeFocus").toBool(),"options close focused More instead of its invoker");
                        realClickItem(window,button);state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=1;return;
                    }
                    if(state->stage==1) {
                        require(QDateTime::currentMSecsSinceEpoch()-state->modified<8000,qPrintable(QString("variant editor stalled: editable=%1 selected=%2 rows=%3 error=%4").arg(options->editable()).arg(options->selectedIds().size()).arg(options->rows().size()).arg(options->error())));
                        if(!options->editable() || options->selectedIds().size()!=2)return;
                        auto *save=findNamed(window,"saveSelectionButton");if(!save || !save->property("visible").toBool())return;
                        require(options->setSelected(options->selectedIds().first(),false),"variant choice failed");
                        require(save->property("enabled").toBool(),"variant Save is disabled");
                        require(QMetaObject::invokeMethod(save,"clicked"),"variant Save did not start");state->stage=2;return;
                    }
                    if(state->stage==2) {
                        require(options->error().isEmpty(),qPrintable(options->error()));
                        if(options->visible() || options->busy())return;
                        state->stage=4;return;
                    }
                    if(qEnvironmentVariable("UI_VARIANT_CASE")=="variant-warning") {
                        if(model->applyReview().isEmpty())return;
                        require(!model->applyReview().value("warnings").toList().isEmpty(),"variant warning skipped review");
                    } else if(qEnvironmentVariable("UI_VARIANT_CASE")=="variant-off") {
                        if(model->busy())return;
                        require(model->pendingChanges(),"variant did not remain pending with automatic Apply off");
                    } else if(model->pendingChanges() || model->busy())return;
                    timer->stop();std::puts("PASS UI saved variant applies automatically");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-auto-order") {
                    auto *options=model->options();
                    if(state->stage==0) {
                        if(!model->profileEditsAvailable()) return;
                        require(options->openOrder(),"load order editor did not open");
                        state->stage=10;return;
                    }
                    if(state->stage==10) {
                        if(!options->editable())return;
                        require(options->order().size()>1 && options->move(options->order().first().toMap().value("id").toString(),1),"load order did not change");
                        require(options->previewOrder(),"load order review did not start");state->stage=1;return;
                    }
                    if(state->stage==1) {
                        require(options->error().isEmpty(),qPrintable(options->error()));
                        if(!options->canSave()) return;
                        require(options->commitSelection(),"load order save did not start");state->stage=2;return;
                    }
                    if(state->stage==2) {
                        if(options->busy())return;
                        require(!options->orderVisible(),"saved load order editor stayed open");
                        state->stage=3;return;
                    }
                    if(model->pendingChanges() || model->busy())return;
                    timer->stop();std::puts("PASS UI saved load order applies automatically");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-health-unavailable") {
                    if (model->loading() || model->healthChecking()) return;
                    const auto health=model->health();
                    require(health.value("error").isNull(),"unavailable mod stopped the health check");
                    const auto report=health.value("report").toMap();
                    if (report.isEmpty()) return;
                    require(report.value("mods").toList().size()==1,"available mod was not checked");
                    require(report.value("unavailableMods").toList().size()==1,"unavailable mod was not reported");
                    if (state->stage==0) { QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("library")));state->stage=1;return; }
                    auto *notice=findNamed(window,"modHealthNotice");
                    if (!notice || !notice->property("visible").toBool()) return;
                    require(notice->property("text").toString().contains("couldn't be checked"),"unavailable mod notice missing");
                    timer->stop();std::puts("PASS UI unavailable mod health notice");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-auto-performance") {
                    auto *perf = model->performance();
                    if (state->stage == 0) {
                        if (!perf->available() || perf->busy() || perf->presets().isEmpty()) return;
                        state->shareCode = perf->presets().first().toMap().value("id").toString();
                        require(perf->select(state->shareCode), "preset choice refused");
                        state->stage = 1; return;
                    }
                    require(state->ticks < 220, qPrintable(QString("preset pending=%1 error=%2 review=%3 detected=%4 selection=%5 status=%6").arg(model->pendingChanges()).arg(model->error()).arg(QString::fromUtf8(QJsonDocument::fromVariant(model->applyReview()).toJson())).arg(perf->detected().value("state").toString()).arg(perf->selection()).arg(model->status())));
                    if (model->busy() || model->loading() || perf->busy() || model->pendingChanges()) return;
                    if (state->stage == 1) {
                        require(perf->selection() == state->shareCode, "preset choice was not saved");
                        if (perf->detected().value("presetId").toString() != state->shareCode) return;
                        require(perf->select(""), "could not choose Valve settings");
                        state->stage = 2; return;
                    }
                    require(perf->selection().isEmpty(), "Valve settings choice was not saved");
                    if (!perf->detected().value("presetId").toString().isEmpty()) return;
                    require(model->error().isEmpty(), "automatic preset write failed");
                    timer->stop(); std::puts("PASS UI preset choice automatically uses journaled Apply"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-error") {
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        model->setSelectedId("local-a"); toggleIncluded(window, true); state->stage = 1; return;
                    }
                    if (model->busy() || model->error().isEmpty()) return;
                    require(!model->closeBlocked(), "failed queued choice leaves window permanently blocked");
                    timer->stop(); std::puts("PASS UI failed automatic toggle permits closing"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-bulk" || scenario == "native-auto-bulk-mixed") {
                    const bool mixed = scenario == "native-auto-bulk-mixed";
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canBulkToggle()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        auto *view = named(window, "libraryView");
                        require(QMetaObject::invokeMethod(view, "setMarked", Q_ARG(QVariant, QVariant("local-a")), Q_ARG(QVariant, QVariant(true))), "cannot mark first mod");
                        require(QMetaObject::invokeMethod(view, "setMarked", Q_ARG(QVariant, QVariant(mixed ? "absent" : "local-b")), Q_ARG(QVariant, QVariant(true))), "cannot mark second mod");
                        click(window, "bulkEnableButton"); state->stage = 1; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                    require(state->ticks < 220, qPrintable(QStringLiteral("bulk stuck: error=%1 status=%2 pending=%3 busy=%4 review=%5")
                        .arg(model->error(), model->status()).arg(model->pendingChanges()).arg(model->busy()).arg(!model->applyReview().isEmpty())));
                    if (model->busy() || model->loading() || model->pendingChanges()
                        || QString::fromUtf8(file.readAll()) == state->shareCode) return;
                    require(model->modDetails("local-a").value("enabled").toBool(), "bulk choice was not applied");
                    if (mixed) require(model->modDetails("absent").isEmpty()
                                       && !model->lastBulkResult().value("skipped").toList().isEmpty(), "unknown mod was not reported as skipped");
                    else require(model->modDetails("local-b").value("enabled").toBool(), "second bulk choice was not applied");
                    timer->stop(); std::puts("PASS UI automatic bulk apply"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-external") {
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        model->setSelectedId("local-a"); toggleIncluded(window, true);
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (model->busy() || model->loading()) return;
                        require(QDateTime::currentMSecsSinceEpoch() - state->modified < 600, "fixture missed the debounce window");
                        QFile file(gameinfo); require(file.open(QIODevice::Append), "cannot modify synthetic gameinfo");
                        file.write("\n// external change during debounce\n"); file.close();
                        require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll()); state->stage = 2; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                    require(QString::fromUtf8(file.readAll()) == state->shareCode, "external change was applied without review");
                    if (model->applyReview().isEmpty()) return;
                    require(!model->applyReview().value("warnings").toList().isEmpty(), "external change did not warn in review");
                    timer->stop(); std::puts("PASS UI external change review"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-off") {
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        auto *preferences = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
                        require(preferences && preferences->setProperty("automaticApply", false), "cannot disable automatic Apply");
                        require(!model->automaticApply(), "automatic Apply setting did not reach the model");
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("library")));
                        if (qEnvironmentVariableIsSet("UI_REVIEW_WIDTH"))
                            window->resize(qEnvironmentVariableIntValue("UI_REVIEW_WIDTH"), qEnvironmentVariableIntValue("UI_REVIEW_HEIGHT"));
                        model->setSelectedId("local-a"); toggleIncluded(window, true);
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (model->busy() || QDateTime::currentMSecsSinceEpoch() - state->modified < 1500) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        require(QString::fromUtf8(file.readAll()) == state->shareCode && model->pendingChanges(), "disabled automatic Apply changed the game");
                        capture(window, "pending-review");
                        click(window, "reviewPendingChangesButton");
                        state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        if (model->applyReview().isEmpty()) return;
                        auto *results = named(window, "modList");
                        results->setProperty("restoring", true);
                        require(QMetaObject::invokeMethod(results, "invalidateData"), "cannot queue result restore");
                        require(QMetaObject::invokeMethod(window, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("home"))), "cannot replace library view");
                        state->stage = 3; return;
                    }
                    timer->stop(); std::puts("PASS UI manual apply setting"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-during-apply") {
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        model->setSelectedId("local-a"); toggleIncluded(window, true); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (!model->writeBusy() || !model->status().startsWith("Applying changes")) return;
                        toggleIncluded(window, false); state->stage = 2; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                    if (model->busy() || model->loading() || model->pendingChanges()
                        || model->revision().toULongLong() < 7 || QString::fromUtf8(file.readAll()) == state->shareCode) return;
                    require(!model->modDetails("local-a").value("enabled").toBool(), "toggle during Apply was lost");
                    timer->stop(); std::puts("PASS UI queued toggle during apply"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-gate") {
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    auto *launch = model->launch();
                    if (state->stage == 0) {
                        if (!model->canSetEnabled() || !launch->available() || launch->writeGate() || launch->busy()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        model->setSelectedId("local-a"); toggleIncluded(window, true); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (!model->previewBusy() || !model->status().startsWith("Checking mods")) return;
                        QFile marker(qEnvironmentVariable("UI_NATIVE_GATE_MARKER")); require(marker.open(QIODevice::WriteOnly), "cannot arm launch gate"); marker.close();
                        // readSettings() can return true when send() declines
                        // admission. Wait for the observable gate before testing
                        // the overlap with preview completion.
                        if(!launch->readSettings() || !launch->writeGate()) return;
                        QObject::connect(launch, &LaunchController::changed, timer, [launch,state] {
                            if (state->stage == 2 && !launch->writeGate()) state->stage = 3;
                        });
                        state->stage = 2; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                    const bool changed = QString::fromUtf8(file.readAll()) != state->shareCode;
                    if (state->stage == 2) {
                        require(!changed, "automatic Apply ran while launch gate was held");
                        require(model->applyReview().isEmpty(), "transient launch gate opened a manual review");
                        if (!model->busy() && !state->sawLoading) {
                            state->sawLoading = true;
                            // Release the helper only after the preview reply
                            // has reached the UI while the launch gate is held.
                            require(QFile::remove(qEnvironmentVariable("UI_NATIVE_GATE_MARKER")), "cannot release launch gate");
                            if (qEnvironmentVariableIsSet("UI_NATIVE_GATE_REFRESH")) model->refresh();
                        }
                        return;
                    }
                    require(state->sawLoading, "preview did not finish while the launch gate was held");
                    if (!changed || model->busy() || model->loading() || model->pendingChanges()) return;
                    require(model->applyReview().isEmpty(), "safe automatic Apply opened a manual review");
                    timer->stop(); std::puts("PASS UI transient launch gate retried automatic Apply"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-fonts") {
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        model->setSelectedId("local-a"); toggleIncluded(window, true); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (model->busy() || model->loading() || !model->fonts()->available()) return;
                        model->fonts()->showInstalled();
                        require(model->fonts()->managerVisible(), "font manager did not open");
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 2; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                    const bool changed = QString::fromUtf8(file.readAll()) != state->shareCode;
                    if (state->stage == 2) {
                        require(!changed, "automatic Apply ran while fonts were open");
                        if (QDateTime::currentMSecsSinceEpoch() - state->modified < 1400) return;
                        model->fonts()->closeInstalled(); state->stage = 3; return;
                    }
                    require(model->applyReview().isEmpty(), "closing fonts opened a manual Apply review");
                    if (!changed || model->busy() || model->loading() || model->pendingChanges()) return;
                    timer->stop(); std::puts("PASS UI automatic Apply waits for fonts"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-restore-review" || scenario == "native-auto-restore-run") {
                    const bool restore = scenario == "native-auto-restore-run";
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled() || !model->canRestore()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        model->setSelectedId("local-a"); toggleIncluded(window, true); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        // Automatic Apply has no debounce window. Review Restore after
                        // the inclusion change has finished its journaled transaction.
                        if (model->busy() || model->loading() || !model->canRestore() || model->pendingChanges()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read applied gameinfo");
                        const auto applied = QString::fromUtf8(file.readAll());
                        require(applied != state->shareCode, "inclusion change did not automatically Apply");
                        state->shareCode = applied;
                        require(model->previewRestore(), "Restore review did not start"); state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        if (!model->applyReview().value("restoreReview").toBool()) return;
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 3; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read gameinfo");
                    const bool changed = QString::fromUtf8(file.readAll()) != state->shareCode;
                    if (state->stage == 3) {
                        require(!changed, "automatic Apply changed gameinfo while Restore review was open");
                        require(model->applyReview().value("restoreReview").toBool(), "automatic Apply replaced the Restore review");
                        if (QDateTime::currentMSecsSinceEpoch() - state->modified < 1400) return;
                        click(window, restore ? "restoreCommitButton" : "cancelReviewButton");
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 4; return;
                    }
                    require(QDateTime::currentMSecsSinceEpoch() - state->modified < 7000,
                            qPrintable(QString("Restore review close stalled: workflow=%1 review=%2 busy=%3 pending=%4 status=%5 error=%6")
                                .arg(model->autoApplyWorkflowOpen()).arg(!model->applyReview().isEmpty()).arg(model->busy()).arg(model->pendingChanges()).arg(model->status(),model->error())));
                    if (model->busy() || model->loading()) return;
                    if (!restore) require(!changed, "cancelling Restore changed the applied setup");
                    if (QDateTime::currentMSecsSinceEpoch() - state->modified < 1800) return;
                    if (restore && !changed) return;
                    if (!restore && model->pendingChanges()) return;
                    if (!restore) require(model->applyReview().isEmpty(), "closing Restore opened a manual Apply review");
                    if (restore) require(!model->canRestore(), "Restore did not complete");
                    timer->stop(); std::puts("PASS UI automatic Apply respects Restore review"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-install-enable") {
                    // User sequence: install a mod through Add mods, then switch it on with
                    // automatic Apply enabled. In the regression its files overlap a deployed mod.
                    auto *local = model->localImport();
                    static qint64 started = QDateTime::currentMSecsSinceEpoch();
                    static QString newId;
                    static QStringList before;
                    static bool sawPending = false;
                    require(QDateTime::currentMSecsSinceEpoch() - started < qEnvironmentVariableIntValue("UI_INSTALL_TIMEOUT_MS") + 25000, qPrintable(QString("install-enable stalled: stage=%1 busy=%2 pending=%3 reason=%4 error=%5 importError=%6")
                        .arg(state->stage).arg(model->busy()).arg(model->pendingChanges()).arg(model->pendingReason(), model->error(), local->error())));
                    if (state->stage == 0) {
                        if (!model->canSetEnabled() || !local->canPrepare() || model->loading()) return;
                        require(model->automaticApply(), "fixture does not use automatic Apply");
                        require(!model->pendingChanges(), "deployed fixture already has pending changes");
                        for (int row = 0; row < model->count(); ++row) before << model->idAt(row);
                        require(local->prepare({QUrl::fromLocalFile(qEnvironmentVariable("UI_INSTALL_FOLDER"))}), "Add mods did not start");
                        state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        require(local->error().isEmpty(), qPrintable(local->error()));
                        if (!local->presentationReady()) return;
                        require(local->reviewCurrentDraft(), "Add mods review did not start"); state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        require(local->error().isEmpty(), qPrintable(local->error()));
                        if (!local->canCommit()) return;
                        require(local->commit(), "Add mods commit did not start"); state->stage = 3; return;
                    }
                    if (state->stage == 3) {
                        require(model->error().isEmpty(), qPrintable(model->error()));
                        if (model->busy() || model->loading() || local->busy() || !model->canToggleEnabled()) return;
                        for (int row = 0; row < model->count(); ++row) {
                            const auto id = model->idAt(row);
                            if (!before.contains(id)) newId = id;
                        }
                        if (newId.isEmpty()) return;
                        require(!model->modDetails(newId).value("enabled").toBool(), "installed mod started enabled");
                        std::printf("TRACE_AFTER_INSTALL pending=%d status=%s reason=%s review=%d\n", model->pendingChanges(),
                                    qPrintable(model->pendingStatus()), qPrintable(model->pendingReason()), model->applyReview().isEmpty() ? 0 : 1);
                        require(!model->pendingChanges(), "installing a disabled mod made the deployed set pending");
                        model->setSelectedId(newId); state->stage = 4; return;
                    }
                    if (state->stage == 4) {
                        auto *details = findNamed(window, "modDetails"); auto *control = details ? findNamed(details, "modSwitch") : nullptr;
                        if (model->selectedId() != newId || !control || !control->property("enabled").toBool()) return;
                        toggleIncluded(window, true);
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 5; return;
                    }
                    const auto review = model->applyReview();
                    if (!review.isEmpty()) {
                        QStringList codes;
                        for (const auto &w : review.value("warnings").toList()) codes << w.toMap().value("code").toString();
                        std::printf("TRACE_MANUAL_REVIEW ms=%lld warnings=%s replacedMounts=%s migration=%s pending=%d status=%s reason=%s processes=%lld recovery=%d outcomeUnknown=%d external=%d\n",
                                    static_cast<long long>(QDateTime::currentMSecsSinceEpoch() - state->modified),
                                    qPrintable(codes.join(',')), qPrintable(review.value("replacedMounts").toStringList().join(',')),
                                    qPrintable(review.value("upstreamRepairMigration").toString()), model->pendingChanges(), qPrintable(model->pendingStatus()),
                                    qPrintable(model->pendingReason()), static_cast<long long>(model->launch()->gameStatus().value("processes").toList().size()),
                                    model->recoveryRequired(), model->outcomeUnknown(), model->externalGameinfoChange());
                        std::fflush(stdout);
                        if (qEnvironmentVariableIsSet("UI_INSTALL_WARNING")) {
                            // A real warning next to the overlap notes still needs review.
                            require(codes.contains("blacklisted") && codes.contains("fileConflict"), "mixed warning review lacks its warnings");
                            require(model->pendingChanges(), "warned install was applied without review");
                            timer->stop(); std::puts("PASS UI warned install waits for review"); QCoreApplication::exit(0); return;
                        }
                        require(false, "automatic Apply opened a manual review for a routine install and enable");
                    }
                    if (model->pendingChanges()) { sawPending = true; return; }
                    require(!qEnvironmentVariableIsSet("UI_INSTALL_WARNING") || !sawPending, "warned install was applied without review");
                    if (!sawPending || model->busy() || model->loading()) return;
                    require(model->error().isEmpty(), qPrintable(model->error()));
                    require(model->modDetails(newId).value("enabled").toBool(), "installed mod is not enabled");
                    std::printf("UI_TIMING_ENABLE_TO_APPLIED_MS %lld\n", static_cast<long long>(QDateTime::currentMSecsSinceEpoch() - state->modified));
                    timer->stop(); std::puts("PASS UI installed mod applies automatically"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-auto-apply" || scenario == "native-auto-warning") {
                    const bool warning = scenario == "native-auto-warning";
                    const auto gameinfo = qEnvironmentVariable("UI_NATIVE_GAME") + "/game/citadel/gameinfo.gi";
                    if (state->stage == 0) {
                        if (!model->canSetEnabled()) return;
                        QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                        state->shareCode = QString::fromUtf8(file.readAll());
                        model->setSelectedId("local-a");
                        toggleIncluded(window, true); toggleIncluded(window, false); toggleIncluded(window, true);
                        require(!named(window, "reviewPendingChangesButton")->property("visible").toBool(), "manual review competes with automatic Apply");
                        state->modified = QDateTime::currentMSecsSinceEpoch(); state->stage = 1; return;
                    }
                    QFile file(gameinfo); require(file.open(QIODevice::ReadOnly), "cannot read synthetic gameinfo");
                    const bool changed = QString::fromUtf8(file.readAll()) != state->shareCode;
                    if (warning) {
                        if (model->applyReview().isEmpty()) return;
                        require(!changed, "warning was applied without review");
                        require(!model->applyReview().value("warnings").toList().isEmpty(), "warning review did not open");
                    } else {
                        if (!changed || model->busy() || model->loading() || model->pendingChanges()) return;
                        require(model->applyReview().isEmpty(), "unwarned toggle opened the review");
                        require(model->modDetails("local-a").value("enabled").toBool(), "last queued toggle was lost");
                        std::printf("UI_TIMING_TOGGLE_TO_GAME_MS %lld\n", static_cast<long long>(QDateTime::currentMSecsSinceEpoch() - state->modified));
                    }
                    timer->stop(); std::puts("PASS UI automatic apply"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-launch-clear-legacy" || scenario == "native-launch-clear-legacy-fails" || scenario == "native-launch-clear-copy") {
                    auto *launch = model->launch();
                    if (state->stage == 0) {
                        if (scenario == "native-launch-clear-legacy-fails") {
                            if (launch->gameStatus().value("state").toString() != "running") return;
                            if (!state->modified) { state->modified = QDateTime::currentMSecsSinceEpoch(); return; }
                            if (QDateTime::currentMSecsSinceEpoch() - state->modified < 500) return;
                        }
                        auto *button = qobject_cast<QQuickItem *>(findNamed(window, "clearLegacyLaunchButton"));
                        if (!button || !button->isVisible() || !button->isEnabled()) return;
                        require(named(window,"launchNoticeText")->property("text").toString().contains("was never confirmed. If Deadlock isn't starting, clear it."), "legacy message missing");
                        if (scenario == "native-launch-clear-copy") {
                            QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("home")));
                            QCoreApplication::processEvents(); capture(window, "before-home");
                        }
                        realClick(window, "clearLegacyLaunchButton"); state->stage = 1; return;
                    }
                    if (scenario == "native-launch-clear-legacy-fails") {
                        if (launch->error().isEmpty()) {
                            require(state->ticks < 200, qPrintable(QString("clear failure missing: phase=%1 busy=%2 game=%3 barrier=%4").arg(launch->phase()).arg(launch->busy()).arg(launch->gameStatus().value("state").toString()).arg(launch->barriers().size())));
                            return;
                        }
                        require(launch->error().contains("gameRunning"), qPrintable(launch->error()));
                        auto *error = findNamed(window, "launchNoticeText");
                        require(error && error->property("visible").toBool(), "legacy clear failure is hidden");
                        require(error->property("text").toString().contains("Close Deadlock"), "legacy clear failure is unclear");
                        require(named(window,"clearLegacyLaunchButton")->property("visible").toBool(), "legacy clear button disappeared");
                        require(!launch->barriers().isEmpty(), "failed clear released the barrier");
                        timer->stop(); std::puts("PASS UI legacy launch clear failure"); QCoreApplication::exit(0); return;
                    }
                    if (!launch->barriers().isEmpty() || launch->writeGate() || model->busy()) return;
                    if (scenario == "native-launch-clear-copy") {
                        if (model->loading() || model->recoveryRequired() || !model->canSetEnabled() || !launch->canLaunch()) return;
                        if (state->stage == 1) {
                            capture(window, "after-home");
                            QMetaObject::invokeMethod(window,"screenshotSetup",Q_ARG(QVariant,QString("library")));
                            state->stage = 2; return;
                        }
                        auto *details = findNamed(window,"modDetails");
                        if (!details || !findNamed(details,"modSwitch") || !named(details,"modSwitch")->property("enabled").toBool()) return;
                        capture(window, "after-library");
                    }
                    timer->stop(); std::puts("PASS UI legacy launch cleared"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-launch-expired") {
                    if (!model->canSetEnabled() || model->launch()->writeGate() || model->loading()) return;
                    require(model->launch()->phase() == "idle", "expired handoff retained launch state");
                    require(!named(window,"launchNotice")->property("visible").toBool(), "expired handoff retained banner");
                    require(!model->recoveryRequired(), "expired handoff requires Recover");
                    timer->stop();std::puts("PASS UI expired current-boot handoff clears on restart without Recover");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-launch-recent" || scenario == "native-launch-recent-clear") {
                    auto *launch=model->launch();
                    if(state->stage==1) {
                        if(launch->writeGate() || !model->canSetEnabled() || model->loading())return;
                        require(!named(window,"launchNotice")->property("visible").toBool(), "cleared handoff left a notice");
                        timer->stop();std::puts("PASS UI current-format handoff cleared with one Clear it");QCoreApplication::exit(0);return;
                    }
                    if(launch->barriers().isEmpty() || launch->busy() || !launch->canResolve())return;
                    require(!launch->canLaunch(), "recent handoff lost its hold");
                    require(named(window,"launchNotice")->property("visible").toBool(), "recent crashed handoff lacks a notice");
                    require(named(window,"clearLegacyLaunchButton")->property("visible").toBool(), "recent crashed handoff lacks Clear it");
                    capture(window,"launch-clear-notice");
                    if(scenario=="native-launch-recent-clear"){realClick(window,"clearLegacyLaunchButton");state->stage=1;return;}
                    timer->stop();std::puts("PASS UI recent handoff offers single Clear it");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-launch-game-cycle" || scenario == "native-launch-live-cycle") {
                    auto *launch = model->launch();
                    const auto detail = QStringLiteral("Game cycle stage=%1 phase=%2 game=%3 gate=%4 recovery=%5 toggle=%6 error=%7 launchError=%8 expected=%9 actual=%10 canSet=%11")
                        .arg(state->stage).arg(launch->phase(), launch->gameStatus().value("state").toString()).arg(launch->writeGate()).arg(model->recoveryRequired()).arg(model->canToggleEnabled()).arg(model->error(),launch->error()).arg(state->shareCode).arg(model->modDetails("local-a").value("enabled").toBool()).arg(model->canSetEnabled());
                    require(state->ticks < 340, qPrintable(detail));
                    if (scenario == "native-launch-live-cycle" && state->stage == 0) {
                        if (!launch->canLaunch()) return;
                        require(launch->phase()=="idle", "ready Play is not idle");
                        model->findChild<BackendClient *>()->rejected(BackendClient::Call::LaunchOwner,"launchBusy","The launch owner is busy; poll its existing control or attempt.");
                        require(!named(window,"launchNotice")->property("visible").toBool(), "idle Play shows a launch notice");
                        require(named(window,"launchActiveButton")->property("text").toString().startsWith("Play"), "idle Play shows stale progress");
                        capture(window,"launch-idle");
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"launchActiveButton"));
                        if (!button || !button->isVisible() || !button->isEnabled()) return;
                        realClick(window, "launchActiveButton"); state->stage = 1; return;
                    }
                    const int observing = scenario == "native-launch-live-cycle" ? 1 : 0;
                    if (state->stage == observing) {
                        if(launch->gameStatus().value("state")!="running") {
                            const auto text=named(window,"launchActiveButton")->property("text").toString();
                            if(QStringList{"reviewing","ready","preparing","verifyingPrepared"}.contains(launch->phase())){
                                require(text=="Preparing…", "Play lacks preparation progress");
                                if(!state->launchCaptures.contains("preparing")){capture(window,"launch-preparing");state->launchCaptures.insert("preparing");}
                            }
                            if(QStringList{"openingSteam","waitingForGame"}.contains(launch->phase())){
                                require(text=="Starting…", "Play lacks startup progress");
                                if(!state->launchCaptures.contains("starting")){capture(window,"launch-starting");state->launchCaptures.insert("starting");}
                            }
                        }
                        if (scenario == "native-launch-live-cycle" && launch->canCommit()) { require(launch->commit(), "could not confirm live launch review"); return; }
                        if (launch->gameStatus().value("state").toString() != "running" || launch->phase() != "running") return;
                        if (qEnvironmentVariableIsSet("UI_LAUNCH_RETIRE") && state->shareCode != "retired") {
                            state->shareCode = "retired";require(launch->retire(), "could not retire running launch owner");state->modified=0;return;
                        }
                        if (!state->modified) { state->modified = QDateTime::currentMSecsSinceEpoch(); return; } if (QDateTime::currentMSecsSinceEpoch() - state->modified < (qEnvironmentVariableIsSet("UI_LAUNCH_RETIRE") ? 5000 : 1500)) return;
                        if(model->outcomeUnknown() || model->busy() || model->loading())return;
                        require(!model->canToggleEnabled(), "toggle available while game runs");
                        require(named(window,"launchActiveButton")->property("text").toString() == "Running", "Play does not show running progress");
                        capture(window,"launch-running");
                        if(scenario=="native-launch-live-cycle" && !qEnvironmentVariable("UI_NATIVE_CAPTURES").isEmpty() && !qEnvironmentVariableIsSet("UI_LAUNCH_RETIRE"))require(state->launchCaptures.contains("preparing") && state->launchCaptures.contains("starting"), "launch capture missed a progress state");
                        require(!named(window,"launchNotice")->property("visible").toBool(), "healthy launch shows a warning");
                        require(findNamed(window,"launchDetailsMenuItem"), "Play menu lacks launch details");
                        if (scenario == "native-launch-live-cycle") {
                            auto *client = model->findChild<BackendClient *>();
                            require(client, "missing backend client");
                            client->rejected(BackendClient::Call::LaunchOwner, "launchBusy", "The launch owner is busy; poll its existing control or attempt.");
                            client->rejected(BackendClient::Call::ModUpdatesGet, "unsupportedAction", "An operation is still running.");
                            client->rejected(BackendClient::Call::Snapshot, "nativeBusy", "Native database is busy; retry when the other operation finishes.");
                            const bool quiet = launch->error().isEmpty() && model->error().isEmpty();
                            const bool recoveryQuiet = named(window,"pendingLabel")->property("text").toString() != "Recovery needed" && !named(window,"operationStatus")->property("text").toString().contains("An earlier change didn't finish");
                            const bool updatesQuiet = model->updatesError().isEmpty() || model->updatesError() == "Checking for updates will work after you close Deadlock.";
                            std::printf("ITEM105 busyQuiet=%d recoveryQuiet=%d updatesQuiet=%d updates=%s\n", quiet, recoveryQuiet, updatesQuiet, qPrintable(model->updatesError()));
                            std::fflush(stdout);
                            require(quiet && recoveryQuiet && updatesQuiet, qPrintable("item105 launch false alarms: " + model->error() + "; " + launch->error()));
                            require(named(window,"operationStatus")->property("text").toString() == "Close Deadlock to change your mods.", "healthy handoff lacks close-game guidance");
                        }
                        QFile signal(qEnvironmentVariable("UI_LAUNCH_GAME_SEEN"));
                        require(signal.open(QIODevice::WriteOnly), "could not signal game observation");
                        signal.write("ready"); signal.close(); state->ticks = 0; state->modified = 0; state->stage = observing + 1; return;
                    }
                    if (state->stage == observing + 1) {
                        if (!model->canSetEnabled() || launch->writeGate() || model->busy() || model->loading()) return;
                        require(launch->phase() == "idle", "completed game left a launch banner");
                        require(named(window,"launchActiveButton")->property("text").toString().startsWith("Play"), "Play retained running progress after exit");
                        require(launch->error().isEmpty() && !model->recoveryRequired() && !named(window,"launchNotice")->property("visible").toBool(), "completed game retained a warning");
                        capture(window,"launch-back-to-play");
                        if (!state->modified) { state->modified = QDateTime::currentMSecsSinceEpoch(); return; }
                        if (QDateTime::currentMSecsSinceEpoch() - state->modified < 2000 || model->performance()->busy()) return;
                        require(model->performance()->error().isEmpty(), qPrintable("Tweaks retained a launch-owner error after game exit: " + model->performance()->error()));
                        model->setAutomaticApply(false); state->shareCode = !model->modDetails("local-a").value("enabled").toBool() ? "enabled" : "disabled";
                        require(model->setModEnabled("local-a", state->shareCode == "enabled"), "toggle failed after game exit"); state->stage++; return;
                    }
                    if (!model->canSetEnabled() || model->busy() || model->loading() || model->modDetails("local-a").value("enabled").toBool() != (state->shareCode == "enabled")) return;
                    if(scenario == "native-launch-live-cycle") {
                        model->findChild<BackendClient *>()->rejected(BackendClient::Call::LaunchOwner,"unexpectedCode","Please try again.");
                        require(named(window,"launchNoticeText")->property("text").toString() == "Please try again.", "raw error code reached launch bar");
                        require(named(window,"launchNotice")->property("visible").toBool(), "failure lacks an action notice");
                        capture(window,"launch-failure-notice");
                    }
                    timer->stop(); std::puts("PASS UI game exit allows editing"); QCoreApplication::exit(0); return;
                }
                if (scenario.startsWith("native-mod-delete") || scenario.startsWith("native-mod-remove")) {
                    if(scenario=="native-mod-delete-running") {
                        if(model->launch()->gameStatus().value("state").toString()!="running")return;
                        require(!model->canBulkOperate() && !model->property("canDeleteMods").toBool(),"running game allowed mod removal");
                        require(!named(window,"deleteModButton")->property("enabled").toBool(),"Delete stayed enabled while game runs");
                        timer->stop();std::puts("PASS UI running game blocks deletion");QCoreApplication::exit(0);return;
                    }
                    if(state->stage==6) {
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"bulkDeleteButton"));
                        if(!button || !button->isVisible() || !button->isEnabled())return;
                        const auto bounds=button->mapRectToScene(QRectF(0,0,button->width(),button->height()));
                        require(bounds.left()>=0 && bounds.right()<=window->width() && bounds.bottom()<=window->height(),"bulk Delete is clipped at 720x520");
                        capture(window,"bulk-removal-actions");realClick(window,"bulkDeleteButton");state->stage=1;return;
                    }
                    if(state->stage==0 || state->stage==5) {
                        model->setAutomaticApply(scenario=="native-mod-delete-auto" || scenario.endsWith("-inactive"));
                        if(scenario.endsWith("-inactive") && model->viewedProfileId()!="other") {model->viewProfile("other");return;}
                        if(!model->canBulkOperate())return;
                        model->setSelectedId("local-a");
                        if(scenario=="native-mod-delete-bulk") {
                            window->resize(720,520);
                            auto *view=named(window,"libraryView");
                            require(QMetaObject::invokeMethod(view,"setMarked",Q_ARG(QVariant,QString("local-a")),Q_ARG(QVariant,true)),"could not select first mod");
                            require(QMetaObject::invokeMethod(view,"setMarked",Q_ARG(QVariant,QString("local-b")),Q_ARG(QVariant,true)),"could not select second mod");
                            state->stage=6;return;
                        }
                        if (!named(window,"removeModButton")->property("visible").toBool()) { click(window,"modActionsButton"); return; }
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,scenario.startsWith("native-mod-remove")?"removeModButton":"deleteModButton"));
                        require(button && button->isVisible() && button->isEnabled(),"mod removal action is missing");
                        for(auto *item=button->parentItem();item;item=item->parentItem()) {
                            if(item->metaObject()->indexOfProperty("contentY")<0)continue;
                            const auto y=button->mapToItem(item,QPointF()).y();
                            if(y<0 || y+button->height()>item->height()) {
                                item->setProperty("contentY",std::clamp(item->property("contentY").toReal()+y-60,0.0,std::max(0.0,item->property("contentHeight").toReal()-item->height())));
                                state->stage=5;return;
                            }
                        }
                        capture(window,"mod-removal-actions");
                        realClick(window,scenario.startsWith("native-mod-remove")?"removeModButton":"deleteModButton");state->stage=1;return;
                    }
                    if(state->stage==1) {
                        auto *dialog=findNamed(window,"modRemovalDialog");
                        if(!dialog || !dialog->property("opened").toBool())return;
                        capture(window,"mod-removal-confirmation");
                        if(scenario=="native-mod-delete-cancel") {click(window,"cancelModRemoval");state->stage=3;return;}
                        if(scenario=="native-mod-delete-stale") {model->viewProfile("other");state->stage=4;return;}
                        click(window,"confirmModRemoval");state->stage=2;return;
                    }
                    if(state->stage==4) {
                        if(model->loading())return;
                        require(!named(window,"confirmModRemoval")->property("enabled").toBool(),"stale deletion confirmation stayed enabled");
                        click(window,"cancelModRemoval");state->stage=3;return;
                    }
                    if(model->busy() || model->loading())return;
                    require(model->error().isEmpty(),qPrintable(model->error()));
                    if(state->stage==2) {
                        if(model->rowCount()!=0)return;
                        if(scenario.endsWith("-inactive")) {
                            require(model->activeProfileId()=="default","removal activated the viewed inactive profile");
                            if(!state->modified) {state->modified=QDateTime::currentMSecsSinceEpoch();return;}
                            if(model->autoApplyWorkflowOpen() || QDateTime::currentMSecsSinceEpoch()-state->modified < 2500)return;
                            require(model->activeProfileId()=="default" && model->revision()=="3","inactive removal changed active setup");
                        } else if(scenario=="native-mod-delete-auto" && (model->pendingChanges() || !model->canSetEnabled()))return;
                        if(!scenario.endsWith("-inactive"))require(model->revision()==(scenario=="native-mod-delete-auto"?"4":"3"),"removal/automatic Apply did not commit exactly once");
                    } else {require(model->rowCount()==1 && model->revision()=="2","cancelled/stale confirmation changed the library");}
                    timer->stop();std::puts("PASS UI mod removal");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-launch-clean") {
                    // A start after an attempt that left no Steam handoff: nothing to recover.
                    auto *launch=model->launch();
                    if (!launch->canLaunch() || !model->canSetEnabled() || model->loading()) return;
                    require(launch->phase() == "idle" && launch->barriers().isEmpty(), "start kept launch state without a handoff");
                    require(!named(window,"launchNotice")->property("visible").toBool(), "start showed a launch notice without a handoff");
                    require(!model->recoveryRequired() && !model->outcomeUnknown(), "start asked for recovery without a handoff");
                    timer->stop();std::puts("PASS UI start without a handoff is ready to play");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-launch-held") {
                    // A start while a Steam handoff is unsettled: closed until the player clears it.
                    auto *launch=model->launch();
                    if (state->stage==0) {
                        require(!launch->canLaunch() && !model->canSetEnabled() && !model->canEditProfiles(), "Play or editing opened while a Steam handoff was unsettled");
                        const bool shown=named(window,"launchNotice")->property("visible").toBool() && named(window,"clearLegacyLaunchButton")->property("visible").toBool();
                        if (launch->barriers().isEmpty() || launch->busy() || !launch->canResolve() || model->loading() || !shown) {
                            require(state->ticks < 600, qPrintable(QStringLiteral("the handoff hold was not offered for clearing: barriers=%1 phase=%2 busy=%3 canResolve=%4 modelBusy=%5 unknown=%6 shown=%7 error=%8")
                                .arg(launch->barriers().size()).arg(launch->phase()).arg(launch->busy()).arg(launch->canResolve()).arg(model->busy()).arg(model->outcomeUnknown()).arg(shown).arg(model->error()+" / "+launch->error())));
                            return;
                        }
                        require(launch->phase()=="interrupted" && launch->attempt().value("state").toString()=="handoff", qPrintable("start does not show the launch that Steam was asked to start: "+launch->phase()));
                        if (!qEnvironmentVariableIsSet("UI_HELD_CLEAR")) { timer->stop();std::puts("PASS UI start keeps the Steam handoff hold");QCoreApplication::exit(0);return; }
                        realClick(window,"clearLegacyLaunchButton");state->stage=1;return;
                    }
                    if (!launch->canLaunch() || !model->canSetEnabled() || model->loading()) return;
                    require(launch->barriers().isEmpty() && !model->recoveryRequired() && !model->outcomeUnknown(), "launch state remained after the handoff was cleared");
                    timer->stop();std::puts("PASS UI cleared handoff reopens Play and editing");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-launch-unproven") {
                    // An older build's Play marker goes through ordinary reconciliation:
                    // the marker stays, and Play and editing stay closed with it.
                    auto *launch=model->launch();
                    require(!launch->canLaunch() && !model->canSetEnabled() && !model->canEditProfiles(), "Play or editing opened beside an unproven Play marker");
                    const bool noHandoff=qEnvironmentVariableIsSet("UI_UNPROVEN_NO_HANDOFF");
                    const bool settled=(noHandoff?launch->barriers().isEmpty():!launch->barriers().isEmpty()) && !launch->busy() && !model->busy() && !model->loading() && (noHandoff||model->recoveryRequired());
                    // Stay for two seconds after the checks have run: nothing may settle the marker.
                    if (!settled || ++state->modified < 50) {
                        require(state->ticks < 600, qPrintable(QStringLiteral("the unproven marker did not come to rest: barriers=%1 busy=%2 modelBusy=%3 loading=%4 recovery=%5 unknown=%6")
                            .arg(launch->barriers().size()).arg(launch->busy()).arg(model->busy()).arg(model->loading()).arg(model->recoveryRequired()).arg(model->outcomeUnknown())));
                        return;
                    }
                    require(model->outcomeUnknown(), "an unproven Play marker was treated as settled");
                    if(qEnvironmentVariableIsSet("UI_UNPROVEN_RECOVER") && state->stage==0){
                        require(model->canRecover() && model->recover(), "ordinary Recover was not offered for the old marker");
                        state->stage=1;state->modified=0;return;
                    }
                    if(state->stage==1)require(!model->error().isEmpty(), "unproven historical recovery was not explained");
                    timer->stop();std::puts("PASS UI unproven Play marker keeps Play and editing closed");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-play-loss") {
                    // The helper dies while a launch owns the connection. UI_LOSS_EXPECT says
                    // whether Steam had already been asked to start the game.
                    auto *launch=model->launch();
                    const bool handoff=qEnvironmentVariable("UI_LOSS_EXPECT")=="handoff";
                    if (state->stage==0) {
                        if (!launch->canLaunch()) return;
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"launchActiveButton"));
                        if (!button || !button->isVisible() || !button->isEnabled()) return;
                        realClick(window,"launchActiveButton");state->stage=1;return;
                    }
                    if (state->stage==1) {
                        if (qEnvironmentVariableIsSet("UI_NATIVE_CONFIRM_REVIEW") && launch->canCommit()) { require(launch->commit(), "could not confirm launch preparation");return; }
                        if (model->error().isEmpty() || launch->phase()!="reconciling") return;
                        require(!launch->canLaunch() && !model->canSetEnabled() && !model->canEditProfiles(), "a lost launch connection reopened Play or editing");
                        state->stage=2;return;
                    }
                    if (state->stage==2) { model->refresh();state->stage=3;return; }
                    if (state->stage==3 && handoff) {
                        require(!launch->canLaunch() && !model->canSetEnabled() && !model->canEditProfiles(), "Play or editing reopened before the Steam handoff was settled");
                        if (launch->barriers().isEmpty() || launch->busy() || !launch->canResolve() || model->loading()) return;
                        require(launch->preparationOutcome()=="succeeded", qPrintable("reconnect did not report the finished preparation: "+launch->preparationOutcome()));
                        if (!named(window,"launchNotice")->property("visible").toBool()) return;
                        require(launch->resolveBarrier(true), "could not clear the handoff after reconnect");
                        state->stage=4;return;
                    }
                    if (!launch->canLaunch() || !model->canSetEnabled() || model->loading()) {
                        require(state->ticks < 600, qPrintable(QStringLiteral("the loss was not settled: phase=%1 gate=%2 attempt=%3 unknown=%4 recovery=%5 error=%6")
                            .arg(launch->phase()).arg(launch->writeGate()).arg(launch->attempt().value("state").toString()).arg(model->outcomeUnknown()).arg(model->recoveryRequired()).arg(model->error()+" / "+launch->error())));
                        return;
                    }
                    require(launch->barriers().isEmpty() && !model->recoveryRequired() && !model->outcomeUnknown(), "launch state remained after the loss was settled");
                    std::printf("revision after loss: %s\n",qPrintable(model->revision()));
                    timer->stop();std::puts("PASS UI launch connection loss settled from the helper's state");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-play-cancel" || scenario == "native-play-cancel-unsent") {
                    auto *launch=model->launch();
                    if (state->stage==0) {
                        if (!launch->canLaunch()) return;
                        if (scenario == "native-play-cancel-unsent") {
                            require(launch->launchActive("modded"), "could not start launch");
                            require(launch->cancel(), "could not cancel unsent launch");state->stage=2;return;
                        }
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"launchActiveButton"));
                        if (!button || !button->isVisible() || !button->isEnabled()) return;
                        realClick(window,"launchActiveButton");state->stage=1;return;
                    }
                    if (state->stage==1) {
                        if (launch->attempt().value("state").toString()!="admitted" || !launch->canCancel()) return;
                        require(launch->cancel(),"could not cancel before Steam");state->stage=2;return;
                    }
                    if (launch->writeGate() || launch->busy() || model->busy() || model->loading()) return;
                    if (state->stage==2) {
                        require(launch->phase()=="idle", "cancelled launch did not return to idle");
                        state->modified=QDateTime::currentMSecsSinceEpoch();state->stage=3;return;
                    }
                    auto *banner=findNamed(window,"launchNotice");
                    require(!banner || !banner->property("visible").toBool(), "cancelled launch left its status bar visible");
                    auto *button=qobject_cast<QQuickItem *>(findNamed(window,"launchActiveButton"));
                    // Helper retirement can refresh options after the launch gate clears.
                    if (!launch->canLaunch() || !button || !button->isVisible() || !button->isEnabled()) {
                        require(QDateTime::currentMSecsSinceEpoch()-state->modified < 3000,
                            qPrintable(QString("Play was not re-enabled after cancellation: available=%1 optionsBusy=%2 optionsPrepared=%3 error=%4")
                                .arg(launch->available()).arg(model->options()->busy()).arg(model->options()->hasPreparation()).arg(launch->error())));return;
                    }
                    capture(window,"play-cancelled-idle");
                    timer->stop();std::puts("PASS UI cancelled launch returns to idle");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-play" || scenario == "native-launch-seed" || scenario == "native-launch-recovered") {
                    auto *launch = model->launch();
                    const auto detail = QStringLiteral("Play stage=%1 phase=%2 preparation=%3 handoff=%4 launchError=%5 preparationError=%6 modelError=%7 gate=%8 canResolve=%9 barriers=%10")
                        .arg(state->stage).arg(launch->phase(), launch->preparationState(), launch->handoffState(), launch->error(), launch->preparationError(), model->error())
                        .arg(launch->writeGate()).arg(launch->canResolve()).arg(launch->barriers().size());
                    if (state->stage < 2)
                        require(launch->error().isEmpty() && launch->preparationError().isEmpty() && model->error().isEmpty(), qPrintable(detail));
                    require(state->ticks < 630, qPrintable(detail));
                    if (state->stage == 0) {
                        if (scenario == "native-launch-recovered") {
                            if (auto *banner = findNamed(window, "launchNotice"))
                                require(!banner->property("visible").toBool(), "stale launch displayed a banner during startup");
                        }
                        if (!launch->canLaunch()) return;
                        auto *button=qobject_cast<QQuickItem *>(findNamed(window,"launchActiveButton"));
                        if (!button || !button->isVisible() || !button->isEnabled()) return;
                        if (scenario == "native-launch-recovered") {
                            require(launch->phase() == "idle", "resolved launch still displays a banner");
                            require(model->canSetEnabled(), "resolved launch still blocks editing mods");
                        }
                        state->modified = QDateTime::currentMSecsSinceEpoch();
                        realClick(window, "launchActiveButton"); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (qEnvironmentVariableIsSet("UI_NATIVE_CONFIRM_REVIEW") && launch->canCommit()) {
                            require(launch->commit(), "could not confirm launch preparation");return;
                        }
                        if (launch->phase() != "waitingForGame" || launch->preparationState() != "prepared" || launch->handoffState() != "dispatched") return;
                        if (!state->busyChecked && qEnvironmentVariableIsSet("UI_NATIVE_CAPTURES")) {
                            capture(window,"play-steam-requested");
                            std::printf("Play to fake Steam: %lld ms\n",static_cast<long long>(QDateTime::currentMSecsSinceEpoch()-state->modified));
                        }
                        if (!state->busyChecked && qEnvironmentVariableIsSet("UI_NATIVE_MAX_LAUNCH_MS")) {
                            require(QDateTime::currentMSecsSinceEpoch() - state->modified < qEnvironmentVariableIntValue("UI_NATIVE_MAX_LAUNCH_MS"), "Play took too long to reach Steam handoff");
                            state->busyChecked = true;
                        }
                        if (!launch->preparationViewConfirmed()) return;
                        if (scenario == "native-launch-seed") {
                            timer->stop(); std::puts("PASS UI handoff seeded"); QCoreApplication::exit(0); return;
                        }
                        require(launch->canStopWaiting() && launch->cancel(), "could not stop waiting after Play");
                        state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        if (launch->canResolve()) {
                            require(launch->resolveBarrier(true), "could not resolve stopped fake Steam handoff");
                            state->stage = 3; return;
                        }
                        if (launch->writeGate() || model->busy() || model->loading()) return;
                        model->requestShutdown(); state->stage = 4; return;
                    }
                    if (state->stage == 3) {
                        if (launch->writeGate() || model->busy() || model->loading()) return;
                        model->requestShutdown(); state->stage = 4; return;
                    }
                    if (!model->shutdownReady()) return;
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-close") {
                    if (state->stage == 0) {
                        require(model->nativeMode(), "close test did not load native mode");
                        toggleIncluded(window, false);
                        state->stage=10; return;
                    }
                    if(state->stage==10) {
                        if(!QFile::exists(model->dataDir()+"/dispatches")) return;
                        require(model->writeBusy(),"close fixture did not dispatch a writer");
                        window->close();
                        require(window->isVisible() && window->property("closeWhenReady").toBool(), "window closed during dispatched write");
                        state->stage = 1; return;
                    }
                    if (model->busy() || window->isVisible()) return;
                    require(!model->outcomeUnknown() && model->revision() == "2", "close did not wait for authoritative completion");
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (model->busy()) {
                    workflowControlsDisabled(window);
                    require(!named(window, "refreshButton")->property("enabled").toBool(), "refresh enabled during native operation");
                    if (findNamed(window, "libraryView")) {
                        auto *indicator = findNamed(window, "operationIndicator");
                        auto *search = findNamed(window, "modSearch");
                        // Controls can appear a tick after the page; wait, but fail below
                        // if the operation ends before they were ever checked.
                        if (!indicator || !search) { state->busyMissing = true; return; }
                        state->busyChecked = true;
                        require(indicator->property("running").toBool(), "operation busy indicator hidden");
                        search->setProperty("text", "Tiny"); QMetaObject::invokeMethod(search, "textEdited");
                        require(model->query() == "Tiny", "search blocked by operation");
                        search->setProperty("text", ""); QMetaObject::invokeMethod(search, "textEdited");
                        capture(window, "busy");
                    }
                    return;
                }
                require(!state->busyMissing || state->busyChecked, "busy Library controls never appeared during the operation");
                if ((scenario == "native-error" && state->stage >= 2) || (scenario == "native-apply-changed" && state->stage >= 8)) {
                    require(model->error().contains(scenario == "native-error" ? "invalidGamePath" : "stalePreview"), "expected Apply failure did not surface");
                    if (scenario == "native-error") require(model->gamePath() == qEnvironmentVariable("UI_NATIVE_GAME"), "invalid folder was persisted");
                    require(named(window, "errorLabel")->property("visible").toBool(), "native error hidden");
                    if (state->stage == 2 || state->stage == 8) {
                        capture(window, "error"); window->resize(800, 600); state->stage++; return;
                    }
                    capture(window, "error-narrow"); timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-settings-busy") {
                    if (state->stage == 0) {
                        if (!model->canPreviewApply()) return;
                        require(model->previewApply(model->gamePath()), "cannot start delayed preview");
                        require(model->busy(), "preview did not enter busy state");
                        click(window, "settingsTab");
                        state->stage = 1; return;
                    }
                    auto *settings = findNamed(window, "automaticApplySwitch");
                    if (!settings || !settings->property("visible").toBool()) return;
                    timer->stop(); std::puts("PASS UI settings during preview"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-item47-restore-reason") {
                    if (state->stage==0) {click(window,"settingsTab");state->stage=1;return;}
                    if (state->stage==1) {
                        for (auto *object : objects(window)) if (object->property("advancedToolsOpen").isValid()) {
                            object->setProperty("advancedToolsOpen",true); state->stage=2; break;
                        }
                        return;
                    }
                    auto *reason=findNamed(window,"previousFolderRestoreReason");
                    if (!reason || !reason->property("visible").toBool()) return;
                    require(reason->property("text").toString().contains("This restore point belongs to the previous game folder ("+qEnvironmentVariable("UI_NATIVE_GAME")+")"),"Restore explanation lacks old path");
                    plain(window,"previousFolderRestoreReason");
                    timer->stop(); std::puts("PASS UI previous-folder Restore reason");QCoreApplication::exit(0);return;
                }
                if (scenario == "native-item47-play") {
                    if (state->stage == 0) {
                        if (!model->launch()->canLaunch()) return;
                        click(window,"launchActiveButton"); state->stage=1; return;
                    }
                    if (state->stage == 1) {
                        auto *button=named(window,"launchReviewChangesButton");
                        if (!button->property("visible").toBool()) return;
                        require((model->launch()->preparationError()+model->launch()->error()).contains("Review the changes to continue."),"Play did not explain drift");
                        click(window,"launchReviewChangesButton"); state->stage=2; return;
                    }
                    if (model->applyReview().isEmpty()) {
                        require(state->ticks<200,qPrintable("Play review stuck: phase="+model->launch()->phase()+" gate="+QString::number(model->launch()->writeGate())+" busy="+QString::number(model->launch()->busy())+" error="+model->launch()->error()+" model="+model->error()+" preview="+QString::number(model->canPreviewApply())));return;
                    }
                    require(!model->launch()->writeGate(),"launch gate retained during Apply review");
                    require(named(window,"reviewWarnings")->property("text").toString().contains("Turn off the original"),"Play review omitted takeover");
                    click(window,"cancelReviewButton");
                    timer->stop(); std::puts("PASS UI Play to Apply review"); QCoreApplication::exit(0); return;
                }
                if (scenario.startsWith("native-item45")) {
                    if (state->stage == 0) {
                        require(model->externalGameinfoChange(), "external banner missing");
                        if (!scenario.endsWith("-error")) require(named(window,"importThoseModsButton")->property("visible").toBool(), "direct import missing");
                        if (scenario.endsWith("-import")) { click(window,"importThoseModsButton"); state->stage=1; return; }
                        click(window,"applyAgainButton"); state->stage=1; return;
                    }
                    if (scenario.endsWith("-import")) {
                        require(named(window,"installedModsList")->property("visible").toBool(), "banner did not open game-folder import");
                    } else if (scenario.endsWith("-error")) {
                        require(model->error().contains("unknownMount"), "unknown mount did not refuse");
                        bool chooser=false;
                        try { named(window,"reviewApplyButton"); chooser=true; } catch(const std::exception &) {}
                        require(!chooser, "review error reopened folder chooser for known game");
                    } else {
                        if (state->stage == 1) {
                            // Startup launch/settings reads also hold the write gate.
                            if (!model->canPreviewApply()) return;
                            require(!model->applyReview().isEmpty(), "Apply again did not open review");
                            const auto warning=named(window,"reviewWarnings")->property("text").toString();
                            require(warning.contains("Turn off the original Deadlock Mod Manager's mods (citadel/addons, ") && warning.contains("The files stay where they are; Restore puts this back."), "takeover explanation hidden");
                            plain(window,"reviewWarnings");
                            require(named(window,"reviewWarnings")->property("visible").toBool(), "takeover explanation not visible");
                            click(window,"applyCommitButton"); state->stage=2; return;
                        }
                        if (model->error().isEmpty() && (!model->canRestore() || model->pendingChanges())) return;
                        require(!model->pendingChanges() && model->canRestore(), qPrintable("takeover did not apply: " + model->error() + " revision=" + model->revision() + " pending=" + QString::number(model->pendingChanges()) + " restore=" + QString::number(model->canRestore())));
                    }
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-family-recover") {
                    if (state->stage < 2) {
                        require(model->recoveryRequired() && model->outcomeUnknown() && model->canRecover(), "recognized v2 interruption not recoverable");
                        require(!model->canSetEnabled() && !model->canPreviewApply() && !model->canEditProfiles(), "interrupted family permitted editing");
                        if (state->stage == 0) { capture(window, "recovery"); window->resize(800, 600); state->stage = 1; return; }
                        capture(window, "recovery-narrow"); click(window, "recoverButton"); state->stage = 2; return;
                    }
                    if(model->outcomeUnknown()) return; // Includes queued wrapper dispatch and original-outcome reconciliation.
                    require(!model->recoveryRequired() && !model->appliedProfileId().isEmpty(), "family recovery did not reconcile original Apply");
                    require(model->revision() == qEnvironmentVariable("UI_NATIVE_RECOVERED_REVISION"), "family recovery revision wrong");
                    require(model->canPreviewApply() && model->canRestore() && !model->pendingChanges(), "family recovery capabilities stale");
                    capture(window, "recovered"); timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario.startsWith("native-legacy-review-")) {
                    const bool importing = scenario.endsWith("import");
                    if (!importing && state->stage > 0 && !model->error().isEmpty()) {
                        require(model->error() == "Helper returned a folder review that does not match the requested layout. Review again.", "paired helper failed for a different reason than the legacy review format");
                        require(model->applyReview().isEmpty() && !model->commitApply(), "legacy review acquired commit authority from hello features");
                        timer->stop(); std::puts("PASS UI paired helper rejects legacy review for sharded request"); QCoreApplication::exit(0); return;
                    }
                    if (state->stage == 0) { click(window, importing ? "importButton" : "applyButton"); state->stage = 1; return; }
                    if (state->stage == 1) {
                        if (!importing) {
                            // A known folder skips the chooser and requests the review directly.
                            bool chooserOpen = true;
                            try { named(window, "reviewApplyButton"); } catch (const std::exception &) { chooserOpen = false; }
                            if (!chooserOpen) { if (model->applyReview().isEmpty()) return; state->stage = 2; return; }
                        }
                        named(window, "gameFolderPath")->setProperty("text", "/synthetic/game");
                        if (importing) named(window, "importSourcePath")->setProperty("text", "/synthetic/state.json");
                        click(window, importing ? "reviewImportButton" : "reviewApplyButton"); state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        const auto summary = named(window, "reviewSummary")->property("text").toString();
                        plain(window, "reviewSummary");
                        require(!summary.contains("packages") && !summary.contains("companion"), "legacy review fabricated additive counts");
                        if (!importing) require(summary.contains(QString::number(model->enabledCount()) + (model->enabledCount() == 1 ? " mod" : " mods")) && summary.contains("to copy"), "legacy Apply lost player summary");
                        capture(window, "legacy-review"); click(window, "cancelReviewButton"); state->stage = 3; return;
                    }
                    require(model->importReview().isEmpty() && model->applyReview().isEmpty(), "legacy cancel retained token");
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-legacy-view" || scenario == "native-legacy-edit") {
                    require(model->nativeMode() && model->modDetails("old").value("unavailableReason").toString() == "External chunks unsupported", "literal Store1 entry lost");
                    if (scenario == "native-legacy-edit") {
                        if (state->stage == 0) {
                            auto *preferences = qmlEngine(window)->singletonInstance<QObject *>("Deadlock.Native", "Preferences");
                            require(preferences && preferences->setProperty("automaticApply", false), "cannot disable auto Apply for library-only fixture");
                            model->setSelectedId("old"); toggleIncluded(window, false); state->stage = 1; return;
                        }
                        if (state->stage == 1) { require(!model->modDetails("old").value("enabled").toBool(), "legacy exclusion lost"); openProfilesViaChooser(window); state->stage = 2; return; }
                        if (state->stage == 2) { submitProfile(window,"duplicate","Copied legacy"); state->stage = 3; return; }
                        if (state->stage == 3) { require(model->viewedProfileId() != "default", "legacy copy identity wrong"); openProfilesViaChooser(window); state->stage = 4; return; }
                        if (state->stage == 4) { submitProfile(window,"rename","Renamed legacy"); state->stage = 5; return; }
                        require(model->profiles().size() == 2, "legacy profiles lost");
                    }
                    capture(window, "legacy"); timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario == "native-recovery-no-doc" || scenario == "native-recovery-source") {
                    if (state->stage == 0) {
                        require(model->outcomeUnknown() && model->canRecover() && !model->canImport(), "unknown import was not gated");
                        capture(window, "recovery"); click(window, "recoverButton"); state->stage = 1; return;
                    }
                    if(model->busy()) return;
                    // Known stale pre-import notice after recovery (853a188 reverted).
                    // Keep checking recovery evidence and write gates below.
                    require(model->error().isEmpty() || model->error() == "nativeNotImported: No native library has been imported; review an upstream import first.", qPrintable(model->error()));
                    if (!model->error().isEmpty()) std::fprintf(stderr, "KNOWN stale recovery notice: %s\n", qPrintable(model->error()));
                    // Recovery briefly releases busy after saving its wrapper,
                    // before the actual recovery and original-marker restore.
                    const auto key=QCryptographicHash::hash(model->dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
                    QFile saved(QDir(AppPaths::stateRoot()).filePath("pending/"+key+".json"));
                    if (!saved.open(QIODevice::ReadOnly)) return;
                    const auto marker=QJsonDocument::fromJson(saved.readAll()).object();
                    const auto original=scenario=="native-recovery-no-doc" ? QStringLiteral("frontend-before-dispatch") : QStringLiteral("source-before-dispatch");
                    if (marker.value("operationId").toString()!=original) return;
                    // Recovery resolves filesystem evidence, not an unknown
                    // original identity that was never dispatched to the helper.
                    require(model->outcomeUnknown() && !model->recoveryRequired() && model->readOnly() && !model->nativeMode(), "recovery did not preserve explicit read-only source and unknown original");
                    require(!model->canImport() && !model->canEditProfiles(), "unknown original did not retain the write gate");
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                require(model->error().isEmpty(), qPrintable(model->error()));
                if (scenario.startsWith("native-controls-")) {
                    const bool native = scenario != "native-controls-source";
                    const bool restore = scenario == "native-controls-applied";
                    workflowControlsReady(window, native, restore);
                    if (state->stage == 0) {
                        click(window, "refreshButton"); require(model->loading(), "refresh did not enter loading");
                        workflowControlsDisabled(window); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        auto *chooser = named(window, "profileChooser");
                        require(chooser->property("enabled").toBool() && chooser->property("visible").toBool(), "profile chooser cannot be used");
                        const int index = native && model->profiles().size() > 1 ? 1 - chooser->property("currentIndex").toInt() : 0;
                        chooser->setProperty("currentIndex", index);
                        require(QMetaObject::invokeMethod(chooser, "activated", Q_ARG(int, index)), "profile browse failed");
                        require(model->loading(), "profile browsing did not enter loading"); workflowControlsDisabled(window);
                        state->stage = 2; return;
                    }
                    timer->stop(); std::puts("PASS UI"); QCoreApplication::exit(0); return;
                }
                if (scenario.startsWith("native-import")) {
                    if (state->stage == 0) { require(model->canImport(), "import unavailable"); click(window, "importButton"); state->stage = 1; return; }
                    if (state->stage == 1) {
                        named(window, "gameFolderPath")->setProperty("text", qEnvironmentVariable("UI_NATIVE_GAME"));
                        named(window, "importSourcePath")->setProperty("text", qEnvironmentVariable("UI_NATIVE_SOURCE"));
                        click(window, "reviewImportButton"); state->stage = 2; return;
                    }
                    if (state->stage == 2) {
                        const int available = qEnvironmentVariableIntValue("UI_NATIVE_AVAILABLE");
                        const int unavailable = qEnvironmentVariableIntValue("UI_NATIVE_UNAVAILABLE");
                        require(model->importReview().value("availableModCount").toInt() == (available ? available : 1), "wrong available import count");
                        require(model->importReview().value("unavailableModCount").toInt() == (unavailable ? unavailable : 1), "unsupported import entry omitted");
                        require(named(window, "reviewReasons")->property("text").toString().contains("Missing fixture"), "unsupported reason not visible before import");
                        plain(window, "reviewSummary"); plain(window, "reviewReasons"); plain(window, "reviewPaths");
                        reviewCounts(window, model->importReview(), true);
                        if (qEnvironmentVariableIsSet("UI_NATIVE_MISSING")) require(named(window, "reviewReasons")->property("text").toString().contains("<b>family</b>"), "missing family reason omitted");
                        state->stage = 3; return;
                    }
                    if (state->stage == 3) {
                        capture(window, "import-review");
                        window->resize(800, 600); state->stage = 4; return;
                    }
                    if (state->stage == 4) { capture(window, "import-review-narrow"); click(window, scenario.endsWith("cancel") ? "cancelReviewButton" : "importCommitButton"); state->stage = 5; return; }
                    if (scenario.endsWith("cancel")) require(!model->nativeMode() && model->importReview().isEmpty(), "cancel retained import token");
                    else require(model->nativeMode() && model->revision() == "1" && model->appliedProfileId().isEmpty(), "import did not return authoritative snapshot");
                } else if (scenario == "native-edit") {
                    require(model->nativeMode(), "native mode missing after restart");
                    if (state->stage == 0) { openProfilesViaChooser(window); state->stage = 1; return; }
                    if (state->stage == 1) { require(!named(window, "removeProfileButton")->property("enabled").toBool(), "default removal enabled"); submitProfile(window,"duplicate","Copied fixture"); state->stage = 2; return; }
                    if (state->stage == 2) { require(model->profiles().size() == 2 && model->viewedProfileId() != "default", "copy did not select independent profile"); openProfilesViaChooser(window); state->stage = 3; return; }
                    if (state->stage == 3) { submitProfile(window,"rename","Renamed fixture"); state->stage = 4; return; }
                    if (state->stage == 4) { model->setSelectedId("local-a"); toggleIncluded(window, false); state->stage = 5; return; }
                    if (state->stage == 5) { require(!model->modDetails("local-a").value("enabled").toBool(), "exclude not persisted"); toggleIncluded(window, true); state->stage = 6; return; }
                    require(model->modDetails("local-a").value("enabled").toBool(), "include not persisted");
                    require(model->pendingChanges() && model->appliedProfileId().isEmpty(), "desired incorrectly claimed applied");
                    if (state->stage == 6) {
                        if (qEnvironmentVariableIsSet("UI_NATIVE_FAMILY")) plain(named(window, "modDetails"), "detailTitle", "<b>family</b>");
                        capture(window, "native-normal"); window->resize(800, 600); state->stage = 7; return;
                    }
                    capture(window, "native-normal-narrow");
                } else if (scenario == "native-error") {
                    if (state->stage == 0) { click(window, "applyButton"); state->stage = 1; return; }
                    // A known folder opens the review directly; reach the chooser through "Change folder…".
                    bool chooserOpen = true;
                    try { named(window, "reviewApplyButton"); } catch (const std::exception &) { chooserOpen = false; }
                    if (!chooserOpen) { click(window, "changeApplyFolder"); return; }
                    named(window, "gameFolderPath")->setProperty("text", qEnvironmentVariable("UI_NATIVE_GAME") + "-wrong");
                    click(window, "reviewApplyButton"); state->stage = 2; return;
                } else if (scenario == "native-maintenance") {
                    if (state->stage == 0) { openProfilesViaChooser(window); state->stage = 1; return; }
                    if (state->stage == 1) { submitProfile(window,"create","Temporary fixture"); state->stage = 2; return; }
                    if (state->stage == 2) { require(model->profiles().size() == 3 && model->totalCount() == 0, "new empty profile wrong"); require(model->canPreviewApply(), "never-applied empty profile cannot be applied"); openProfilesViaChooser(window); state->stage = 3; return; }
                    if (state->stage == 3) { click(window, "removeProfileButton"); state->stage = 4; return; }
                    if (state->stage == 4) { require(QMetaObject::invokeMethod(named(window, "removeProfileConfirmation"), "accept"), "remove confirmation failed"); require(QMetaObject::invokeMethod(profilePopup(window),"close"),"profile editor did not close"); state->stage = 5; return; }
                    require(model->profiles().size() == 2 && model->viewedProfileId() == "default", "profile removal failed");
                } else if (scenario.startsWith("native-apply")) {
                    if (state->stage == 0) {
                        require(model->nativeMode() && model->appliedProfileId().isEmpty(), "wrong apply startup state");
                        int copiedIndex = -1;
                        for (int i = 0; i < model->profiles().size(); ++i) if (model->profiles()[i].toMap().value("name") == "Renamed fixture") copiedIndex = i;
                        require(copiedIndex >= 0, "copied profile missing");
                        auto *chooser = named(window, "profileChooser"); chooser->setProperty("currentIndex", copiedIndex);
                        require(QMetaObject::invokeMethod(chooser, "activated", Q_ARG(int, copiedIndex)), "native profile browse failed");
                        state->stage = 1; return;
                    }
                    if (state->stage == 1) { require(model->viewedProfileId() != "default" && model->appliedProfileId().isEmpty(), "browse applied a profile"); click(window, "applyButton"); state->stage = 2; return; }
                    if (state->stage == 2) {
                        // A known folder skips the chooser and requests the review directly.
                        bool chooserOpen = true;
                        try { named(window, "reviewApplyButton"); } catch (const std::exception &) { chooserOpen = false; }
                        if (chooserOpen) { named(window, "gameFolderPath")->setProperty("text", qEnvironmentVariable("UI_NATIVE_GAME")); click(window, "reviewApplyButton"); }
                        else if (model->applyReview().isEmpty()) return;
                        state->stage = 3; return;
                    }
                    if (state->stage == 3) { require(model->applyReview().value("fileCount").toInt() == 1, "wrong reviewed file count"); require(named(window, "reviewReasons")->property("text").toString().contains("citadel/addons"), "changed mounts hidden"); reviewCounts(window, model->applyReview(), false); state->stage = 4; return; }
                    if (state->stage == 4) { capture(window, "apply-review"); window->resize(800, 600); state->stage = 5; return; }
                    if (state->stage == 5) {
                        capture(window, "apply-review-narrow");
                        if (scenario == "native-apply-changed") { std::puts("READY_FOR_FAMILY_APPLY_MUTATION"); std::fflush(stdout); state->stage = 6; return; }
                        click(window, scenario.endsWith("cancel") ? "cancelReviewButton" : "applyCommitButton"); state->stage = 7; return;
                    }
                    if (state->stage == 6) { if (!QFileInfo::exists(qEnvironmentVariable("UI_NATIVE_MUTATION_ACK"))) return; window->resize(1200, 780); click(window, "applyCommitButton"); state->stage = 8; return; }
                    if (scenario.endsWith("cancel")) require(model->appliedProfileId().isEmpty() && model->applyReview().isEmpty() && model->pendingChanges(), "cancelled Apply changed state or retained token");
                    else {
                        const auto detail=QString("apply snapshot not verified: applied=%1 pending=%2 restore=%3 busy=%4 loading=%5 recovery=%6 unknown=%7 warnings=%8 error=%9")
                            .arg(model->appliedProfileId()).arg(model->pendingChanges()).arg(model->canRestore()).arg(model->busy()).arg(model->loading()).arg(model->recoveryRequired()).arg(model->outcomeUnknown()).arg(model->warnings().join("; ")).arg(model->error());
                        if (!model->canRestore() && state->ticks < 325) return;
                        require(!model->appliedProfileId().isEmpty() && !model->pendingChanges() && model->canRestore(), qPrintable(detail)); capture(window, "applied");
                    }
                } else if (scenario == "native-restore") {
                    if (state->stage == 0) { click(window, "restoreButton"); state->stage = 1; return; }
                    if (state->stage == 1) {
                        click(window, "restoreCommitButton"); state->stage = 2; return;
                    }
                    require(model->appliedProfileId().isEmpty() && !model->canRestore() && model->pendingChanges(), "restore lost desired choices or applied claim");
                } else if (scenario == "native-refused-restore") {
                    require(model->nativeMode() && !model->canRestore(), "old checkpoint must load with Restore refused");
                    require(!model->warnings().isEmpty(), "Restore warning missing");
                    require(model->warningCodes().contains("unverifiedRestore"), "Restore warning code missing from model");
                    require(!model->warnings().join(" ").contains("unverifiedRestore"), "warning exposes internal code");
                } else if (scenario == "native-source") {
                    common(window, model); require(model->totalCount() == (qEnvironmentVariableIsSet("UI_NATIVE_FAMILY") ? 3 : 2), "explicit source lost source rows");
                } else {
                    require(model->nativeMode(), "native restart ignored store");
                    if (scenario == "native-snapshot-restored") require(model->appliedProfileId().isEmpty(), "restored setup claimed native profile");
                    else require(!model->appliedProfileId().isEmpty(), "restart lost verified Apply");
                    if (state->stage == 0) { window->resize(800, 600); state->stage = 1; return; }
                    capture(window, "narrow");
                }
                timer->stop(); std::puts("PASS UI"); std::fflush(stdout); QCoreApplication::exit(0); return;
            }
            if (scenario == "source-choice" && state->stage == 0) {
                require(!model->error().isEmpty(), "initial missing source should fail");
                require(QMetaObject::invokeMethod(window, "sourceRequested", Q_ARG(QUrl, QUrl::fromLocalFile(qEnvironmentVariable("UI_CHOSEN_SOURCE")))), "source selection signal is not connected");
                state->stage = 1; return;
            }
            if (scenario == "previews") {
                auto *details = named(window, "modDetails");
                auto *preview = named(details, "detailPreview");
                if (state->stage == 0) {
                    require(!qEnvironmentVariableIsEmpty("UI_PREVIEW_FIRST"), "preview fixture is missing");
                    // Initial restore may wait for the responsive header to settle.
                    // It must resume when the loaded view acquires usable geometry.
                    if (named(window, "modList")->property("restoring").toBool()) return;
                    // Sample unmodified artwork from an enabled row; off cards intentionally dim it.
                    require(model->setModEnabled(model->idAt(0), true), "preview fixture could not enable its first mod");
                    // The panel shows the selected row; fixture mods reuse that row's ID.
                    model->setSelectedId(model->idAt(0));
                    details->setProperty("mod", previewMod(model->idAt(0), false));
                    QObject *row = nullptr;
                    for (auto *object : objects(window))
                        if (object->objectName() == "modRow" && object->property("modId").toString() == model->idAt(0)) { row = object; break; }
                    require(row, "first visible row is missing");
                    row->setObjectName("fixtureRow");
                    state->stage = 1; return;
                }
                if (state->stage == 1) {
                    auto *thumbnail = named(named(window, "fixtureRow"), "rowPreview");
                    if (!imageReady(preview) || !imageReady(thumbnail)) return;
                    // Enabling the ready thumbnail animates its opacity. Wait for
                    // the full artwork color before checking the captured pixels.
                    if (!colorMatches(window, thumbnail, QColor(210, 45, 65)) && ++state->modified < 25) return;
                    state->modified = 0;
                    renderedColor(window, preview, QColor(210, 45, 65));
                    renderedColor(window, thumbnail, QColor(210, 45, 65));
                    if (!qEnvironmentVariableIsEmpty("UI_PREVIEW_CAPTURE_DIR"))
                        window->grabWindow().save(qEnvironmentVariable("UI_PREVIEW_CAPTURE_DIR") + "/gallery.png");
                    require(named(preview, "previewImage")->property("sourceSize").toSize() == QSize(960, 640), "inspector decode is unbounded");
                    require(named(thumbnail, "previewImage")->property("sourceSize").toSize() == QSize(530, 350), "thumbnail decode is unbounded");
                    click(details, "nextPreview"); state->stage = 2; return;
                }
                if (state->stage == 2) {
                    if (!imageReady(preview)) return;
                    renderedColor(window, preview, QColor(30, 115, 220));
                    require(details->property("galleryIndex").toInt() == 1, "gallery did not advance");
                    require(!named(details, "nextPreview")->property("enabled").toBool(), "gallery advanced past its end");
                    click(details, "previousPreview"); state->stage = 3; return;
                }
                if (state->stage == 3) {
                    if (!imageReady(preview)) return;
                    renderedColor(window, preview, QColor(210, 45, 65));
                    click(details, "enlargePreview"); state->stage = 4; return;
                }
                if (state->stage == 4) {
                    auto *enlarged = named(window, "mediaViewerPreview");
                    if (!imageReady(enlarged)) return;
                    // The viewer fades in; sample once the transition has finished.
                    if (!colorMatches(window, enlarged, QColor(210, 45, 65)) && ++state->modified < 25) return;
                    renderedColor(window, enlarged, QColor(210, 45, 65));
                    if (!qEnvironmentVariableIsEmpty("UI_PREVIEW_CAPTURE_DIR"))
                        window->grabWindow().save(qEnvironmentVariable("UI_PREVIEW_CAPTURE_DIR") + "/enlarged.png");
                    require(named(enlarged, "previewImage")->property("sourceSize").toSize() == QSize(1920, 1080), "enlarged decode is unbounded");
                    click(window, "closeEnlargedPreview");
                    details->setProperty("mod", previewMod(model->idAt(0), true));
                    state->stage = 5; return;
                }
                if (state->stage == 5 || state->stage == 7) {
                    auto *image = named(preview, "previewImage");
                    require(image->property("source").toUrl().isEmpty(), "hidden image was loaded");
                    require(image->property("status").toInt() == 0, "hidden image retained decoded pixels");
                    require(named(preview, "previewFallback")->property("text").toString() == "Preview hidden", "hidden fallback is unclear");
                    require(details->property("galleryIndex").toInt() == 0, "selection retained gallery position");
                    if (state->stage == 7) {
                        details->setProperty("mod", QVariantMap{{"id", model->idAt(0)}, {"images", QStringList{}},
                            {"installedVpks", QStringList{}}, {"missingVpks", QStringList{}}});
                        state->stage = 8; return;
                    }
                    click(preview, "revealPreview"); state->stage = 6; return;
                }
                if (state->stage == 6) {
                    if (!imageReady(preview)) return;
                    renderedColor(window, preview, QColor(210, 45, 65));
                    const auto source = preview->property("source").toUrl();
                    preview->setProperty("contextKey", preview->property("contextKey").toString() + "-new-owner");
                    require(preview->property("source").toUrl() == source, "owner change unexpectedly changed the image URL");
                    require(preview->property("masked").toBool(), "reveal carried to a new owner with the same URL");
                    require(named(preview,"previewImage")->property("source").toUrl().isEmpty(), "new owner retained revealed pixels");
                    click(preview,"revealPreview"); state->stage = 12; return;
                }
                if (state->stage == 12) {
                    if (!imageReady(preview)) return;
                    renderedColor(window, preview, QColor(210, 45, 65));
                    click(details, "nextPreview");
                    auto another = previewMod(model->idAt(0), true); another["title"] = "Another synthetic artwork";
                    details->setProperty("mod", another);
                    state->stage = 7; return;
                }
                if (state->stage == 8) {
                    require(named(preview, "previewFallback")->property("text").toString() == "No preview available", "missing-image fallback is unclear");
                    require(named(preview, "previewImage")->property("source").toUrl().isEmpty(), "empty selection retained image");
                    auto broken = previewMod(model->idAt(0), false);
                    broken["images"] = QStringList{qEnvironmentVariable("UI_PREVIEW_FIRST") + ".missing"};
                    details->setProperty("mod", broken);
                    state->stage = 9; return;
                }
                if (state->stage == 9) {
                    if (named(preview, "previewImage")->property("status").toInt() != 3) return;
                    require(named(preview, "previewFallback")->property("text").toString() == "Preview unavailable", "failed-image fallback is unclear");
                    named(window, "modList")->property("view").value<QObject *>()->setProperty("contentY", 94 * 120);
                    state->stage = 10; return;
                }
                if (state->stage == 10) {
                    // Scrolled far away: rows that left the view must drop their pixels.
                    int pooled = 0;
                    for (auto *row : objects(window)) {
                        if (row->objectName() != "modRow" && row->objectName() != "fixtureRow") continue;
                        if (row->property("mediaActive").toBool()) continue;
                        ++pooled;
                        require(named(named(row, "rowPreview"), "previewImage")->property("source").toUrl().isEmpty(), "inactive row retained its image");
                    }
                    if (pooled == 0) return;
                    named(window, "modList")->property("view").value<QObject *>()->setProperty("contentY", 0);
                    state->stage = 11; return;
                }
                // Back at the top: reused rows show their own mod's artwork, not a stale one.
                QQuickItem *even = nullptr, *odd = nullptr;
                for (auto *object : objects(window)) {
                    if (object->objectName() != "modRow" && object->objectName() != "fixtureRow") continue;
                    if (object->property("modId").toString() == model->idAt(0)) even = qobject_cast<QQuickItem *>(object);
                    if (object->property("modId").toString() == model->idAt(1)) odd = qobject_cast<QQuickItem *>(object);
                }
                if (!even || !odd) return;
                auto *evenPreview = named(even, "rowPreview"); auto *oddPreview = named(odd, "rowPreview");
                if (!imageReady(evenPreview) || !imageReady(oddPreview)) return;
                renderedColor(window, evenPreview, QColor(210, 45, 65));
                renderedColor(window, oddPreview, QColor(30, 115, 220));
            } else if (scenario.startsWith("error:")) {
                common(window, model);
                require(model->totalCount() == 0, "source error invented sample rows");
                require(!model->error().isEmpty(), "source error not surfaced");
                const auto kind = scenario.sliced(6);
                if (kind != "helper") require(model->error().contains(kind), "wrong source error kind");
                if (kind == "sourceNotFound" && model->sourcePath().isEmpty()) {
                    // Nothing to read yet: first run shows the welcome instead of an error banner.
                    require(named(window, "welcome")->property("visible").toBool(), "first-run welcome is hidden");
                } else {
                    require(named(window, "errorLabel")->property("visible").toBool(), "error is hidden");
                }
                {
                    // The banner drops the helper's leading "codeName: " prefix only.
                    auto shown = model->error();
                    static const QRegularExpression code(QStringLiteral("^[a-z][A-Za-z]+: "));
                    shown.remove(code);
                    plain(window, "errorLabel", shown);
                }
            } else if (scenario.startsWith("demo")) {
                require(model->demoMode() && !model->readOnly(), "explicit demo did not load");
                require(model->totalCount() == (scenario == "demo" ? 240 : 10000), "demo row option ignored");
                auto *toggle = named(window, "modSwitch");
                require(toggle->property("enabled").toBool(), "demo switch is disabled");
                toggle->setProperty("checked", true);
                require(QMetaObject::invokeMethod(toggle, "toggled"), "demo switch could not be toggled");
                require(model->selectedMod().value("enabled").toBool(), "demo change not visible");
            } else {
                common(window, model);
                require(model->error().isEmpty(), "snapshot failed");
                if (scenario == "empty") {
                    require(model->totalCount() == 0 && model->selectedMod().isEmpty(), "empty library retained stale rows");
                    require(named(window, "emptyLabel")->property("visible").toBool(), "empty state is hidden");
                } else if (scenario == "refresh") {
                    if (state->stage == 0) {
                        require(model->totalCount() == 4, "refresh initial snapshot wrong");
                        state->modified = QFileInfo(model->sourcePath()).lastModified().toMSecsSinceEpoch();
                        std::puts("READY_FOR_REFRESH"); std::fflush(stdout); state->stage = 1; return;
                    }
                    if (state->stage == 1) {
                        if (QFileInfo(model->sourcePath()).lastModified().toMSecsSinceEpoch() == state->modified) return;
                        click(window, "refreshButton"); state->stage = 2; return;
                    }
                    require(model->totalCount() == 0 && model->selectedMod().isEmpty(), "refresh retained stale data");
                } else if (scenario == "profiles" || scenario == "profile-option") {
                    if (state->stage == 0 && scenario == "profiles") {
                        require(model->totalCount() == 4 && model->viewedProfileId() == "default", "initial profile wrong");
                        chooseCategory(window, model->types().indexOf("<b>Category</b>") + 1);
                        require(model->count() == 1, "profile browse filter fixture failed");
                        realClick(window, "profileChooser");
                        realKey(window, Qt::Key_End); realKey(window, Qt::Key_Return);
                        state->stage = 1; return;
                    }
                    require(model->viewedProfileId() == "other" && model->activeProfileId() == "default", "browsing changed active profile");
                    require(model->totalCount() == 1 && model->idAt(0) == "inactive", "inactive profile data wrong");
                    require(model->typeFilter().isEmpty() && named(window, "categoryChooser")->property("currentIndex").toInt() == 0,
                            "profile change left a hidden category filter");
                    require(named(window, "activeProfileLabel")->property("text").toString().contains("<b>Default</b>"), "active and viewed profiles not distinguished");
                } else {
                    if (state->stage == 0) {
                    if (scenario == "snapshot") require(state->sawLoading, "loading behavior was not observed");
                        require(model->totalCount() == 4 && model->enabledCount() == 1, "active working counts wrong");
                        require(model->selectedId() == "mod_active", "string selection is broken");
                        plain(window, "detailTitle", "<b>Literal title</b><img src=\"http://127.0.0.1:9/metadata\">");
                        plain(window, "detailAuthor", "by <b>Literal author</b>");
                        plain(window, "detailHero", "<b>Ivy</b>");
                        plain(window, "detailKind", "<b>Category</b>");
                        plain(window, "detailStatus", "installed");
                        plain(window, "rowTitleText"); plain(window, "rowAuthorStatus");
                        plain(window, "rowHeroCategory"); plain(window, "profileText");
                        plain(window, "activeProfileLabel"); plain(window, "warningLabel");
                        require(named(window, "warningLabel")->property("visible").toBool(), "warnings are hidden");
                        require(named(window, "detailFiles")->property("text").toString().contains("lost.vpk"), "file metadata missing");
                        require(model->types().contains("<b>Category</b>"), "real categories missing");
                        chooseCategory(window, model->types().indexOf("<b>Category</b>") + 1);
                        require(model->typeFilter() == "<b>Category</b>" && model->count() == 1, "category filtering is not connected");
                        plain(window, "categoryText", "<b>Category</b>");
                        chooseCategory(window, 0);
                        require(model->count() == 4, "all mods filter did not reset category");
                        state->stage = 1; return;
                    }
                    bool selected = false;
                    for (auto *object : objects(window)) {
                        if (object->objectName() == "modRow" && object->property("modId").toString() == "mod_transient") {
                            // Click the card's empty area between text and switch.
                            auto *row = qobject_cast<QQuickItem *>(object);
                            // The card settles a tick after the filter change; click once it is ready.
                            if (!row->property("interactable").toBool()) return;
                            realClickItem(window, row, QPointF(row->width() * 0.7, row->height() / 2));
                            selected = true; break;
                        }
                    }
                    require(selected && model->selectedId() == "mod_transient", "row selection lost string ID");
                    plain(window, "detailStatus", "<b>raw-status</b>");
                    auto *enabled = named(window, "enabledFilter");
                    enabled->setProperty("checked", true); QMetaObject::invokeMethod(enabled, "toggled");
                    require(model->count() == 1 && model->selectedId() == "mod_active", "enabled filter is not connected");
                    enabled->setProperty("checked", false); QMetaObject::invokeMethod(enabled, "toggled");
                    auto *search = named(window, "modSearch");
                    search->setProperty("text", "Literal author");
                    QMetaObject::invokeMethod(search, "textEdited");
                    require(model->count() == 1, "author search is not connected");
                    search->setProperty("text", "no matches"); QMetaObject::invokeMethod(search, "textEdited");
                    require(model->count() == 0 && model->selectedMod().isEmpty(), "search retained stale details");
                }
            }
            timer->stop(); std::puts("PASS UI"); std::fflush(stdout); QCoreApplication::exit(0);
        } catch (const std::exception &error) {
            if (scenario.startsWith("native-launch-clear-legacy")) {
                auto *launch = model->launch();
                std::fprintf(stderr, "CLEAR_DIAGNOSTIC barriers=%d writeGate=%d busy=%d phase=%s error=%s\n",
                    int(launch->barriers().size()), int(launch->writeGate()), int(model->busy()),
                    qPrintable(launch->phase()), qPrintable(launch->error()));
            }
            timer->stop(); std::fprintf(stderr, "FAIL UI: %s (scenario %s, stage %d)\n", error.what(), qPrintable(scenario), state->stage); QCoreApplication::exit(30);
        }
    });
    timer->start();
}

bool checkGridCapture(QQuickWindow *window, LibraryModel *model, bool updatesObserved) {
    int wanted=0,loaded=0;
    for (auto *o:objects(window)) if(o->objectName()=="modRow" && o->property("visible").toBool()) {
        ++wanted;
        auto *preview=named(o,"rowPreview");
        if(preview->property("ready").toBool()) ++loaded;
    }
    const bool ok=model->totalCount()==7 && wanted==7 && loaded==7 && updatesObserved;
    std::fprintf(ok ? stdout : stderr,"%s grid capture: %d/%d ready, fixture rows %d, update check observed %d\n",ok ? "PASS" : "FAIL",loaded,wanted,model->totalCount(),updatesObserved);
    return ok;
}
