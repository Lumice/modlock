#include "librarymodel.h"
#include "appimagepalette.h"
#include "apppaths.h"
#include "installlink.h"
#include "instancebroker.h"
#include "remotecontroller.h"
#include "catalogdata.h"
#include "localartworkprovider.h"
#include "previewnetworkfactory.h"
#include "audiopreview.h"
#ifdef MODLOCK_UI_TEST_DRIVERS
#include "../tests/animationcheck.h"
#include "../tests/gallerycapture.h"
#include "../tests/uicheck.h"
#include "../tests/libraryflickercheck.h"
#endif

#include <QCommandLineParser>
#include <cstdio>
#include <QDebug>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
#include <QStyleHints>
#include <QPalette>
#include <QCryptographicHash>
#include <QLocalServer>
#include <QLocalSocket>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QQmlIncubationController>
#include <QtQml/qqmlextensionplugin.h>
#include <QQuickWindow>
#include <QStandardPaths>
#include <QTimer>
#include <QUrl>

Q_IMPORT_QML_PLUGIN(DeadlockStylePlugin)

class SourceLoader final : public QObject {
    Q_OBJECT
public:
    SourceLoader(LibraryModel *model, QString helper, QString dataDir) : m_model(model), m_helper(std::move(helper)), m_dataDir(std::move(dataDir)) {}
public slots:
    void openSource(const QUrl &source) {
        if (source.isLocalFile()) m_model->loadSource(m_helper, source.toLocalFile(), m_dataDir);
    }
private:
    LibraryModel *m_model;
    QString m_helper, m_dataDir;
};

#ifdef MODLOCK_UI_TEST_DRIVERS
static bool isTestPath(const QString &root, const QString &path)
{
    if (!QDir::isAbsolutePath(path) || path.split('/').contains("..")) return false;
    // Some startup checks intentionally pass missing files and directories.
    // Verify the nearest existing ancestor, rejecting dangling symlinks too.
    QFileInfo info(QDir::cleanPath(path));
    while (!info.exists() && !info.isSymLink())
        info.setFile(info.absolutePath());
    const QString resolved = info.canonicalFilePath();
    return resolved == root || resolved.startsWith(root + '/');
}

static QString testRoot()
{
    const QFileInfo directory(qEnvironmentVariable("MODLOCK_UI_TEST_ROOT"));
    const QString root = directory.canonicalFilePath();
    const auto shared = QFileDevice::ReadGroup | QFileDevice::WriteGroup | QFileDevice::ExeGroup
        | QFileDevice::ReadOther | QFileDevice::WriteOther | QFileDevice::ExeOther;
    if (!directory.isDir() || (directory.permissions() & shared)
        || !isTestPath(root, qEnvironmentVariable("HOME"))) return {};

    for (const char *key : {"XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME",
                           "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "QML_DISK_CACHE_PATH"}) {
        const QString path = qEnvironmentVariable(key);
        if (!path.isEmpty() && !isTestPath(root, path)) return {};
    }
    return root;
}
#endif

int main(int argc, char *argv[]) {
    if (qEnvironmentVariableIsEmpty("QT_QUICK_CONTROLS_STYLE"))
        qputenv("QT_QUICK_CONTROLS_STYLE", "DeadlockStyle");
    // Qt's default wheel deceleration cancels most of a quick wheel spin
    // (five notches moved ~200 px). 12000 roughly doubles to triples the
    // distance while one notch still moves three lines.
    if (qEnvironmentVariableIsEmpty("QT_QUICK_FLICKABLE_WHEEL_DECELERATION"))
        qputenv("QT_QUICK_FLICKABLE_WHEEL_DECELERATION", "12000");
    // Defence in depth only: sound previews never use Qt's FFmpeg media path,
    // but if a host plugin loads, it must not open nested network or file URLs.
    qputenv("QT_FFMPEG_PROTOCOL_WHITELIST", "none");
    qRegisterMetaType<CatalogDecodedResult>();
    QGuiApplication app(argc, argv);
    app.setApplicationName(QStringLiteral("Deadlock Native"));
    app.setApplicationDisplayName(QStringLiteral("Modlock"));
    app.setOrganizationName(QStringLiteral("dl-mod-manager"));
    app.setOrganizationDomain(QString());
    app.setDesktopFileName(QStringLiteral("modlock"));
    QCommandLineParser parser;
    parser.setApplicationDescription(QStringLiteral("Native mod profiles with reviewed import, Apply and Restore."));
    parser.addHelpOption();
    parser.addOption({QStringLiteral("source"), QStringLiteral("Read this upstream state.json instead of discovering it."), QStringLiteral("path")});
    parser.addOption({QStringLiteral("helper"), QStringLiteral("Use this helper executable (default: beside this app)."), QStringLiteral("path")});
    parser.addOption({QStringLiteral("data-dir"), QStringLiteral("Use this isolated native library root for every native request."), QStringLiteral("path")});
    parser.addOption({QStringLiteral("profile"), QStringLiteral("Browse this profile ID without activating it."), QStringLiteral("id")});
    parser.addOption({QStringLiteral("demo"), QStringLiteral("Use an in-memory sample library. Changes are discarded on exit.")});
    parser.addOption({QStringLiteral("rows"), QStringLiteral("Number of demo mods (1–100000; requires --demo)."), QStringLiteral("count"), QStringLiteral("240")});
    parser.addOption({QStringLiteral("performance"), QStringLiteral("Open the rendering diagnostics panel.")});
    parser.addOption({QStringLiteral("screenshot"), QStringLiteral("Save a screenshot after loading settles, then exit."), QStringLiteral("path")});
#ifdef MODLOCK_UI_TEST_DRIVERS
    parser.addOption({QStringLiteral("smoke-test"), QStringLiteral("Check the model and UI, then exit.")});
    parser.addOption({QStringLiteral("check-animation"), QStringLiteral("Verify that the test marker visibly moves, then exit.")});
    QCommandLineOption screenshotPage(QStringLiteral("screenshot-page"), QStringLiteral("Before --screenshot, show this page or dialog (home, library, grid, browse, profiles, apply, launch-settings, select, narrow, gallery)."), QStringLiteral("page"));
    screenshotPage.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(screenshotPage);
    QCommandLineOption uiCheck(QStringLiteral("check-ui"), QStringLiteral("Run synthetic UI integration assertions."), QStringLiteral("scenario"));
    uiCheck.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(uiCheck);
#endif
    parser.addPositionalArgument("url", "GameBanana one-click install link.", "[url...]");
    parser.process(app);

    const bool screenshot = parser.isSet("screenshot");
    bool automated = screenshot;
    QString uiScenario;
#ifdef MODLOCK_UI_TEST_DRIVERS
    const bool smoke = parser.isSet("smoke-test");
    const bool animationCheck = parser.isSet("check-animation");
    const bool uiCheckRun = parser.isSet(uiCheck);
    const bool fixtureRun = smoke || animationCheck || uiCheckRun || parser.isSet(screenshotPage);
    automated = automated || smoke || animationCheck;
    uiScenario = parser.value(uiCheck);
    const auto sandbox = fixtureRun ? testRoot() : QString();
    if (fixtureRun && (sandbox.isEmpty() || (parser.isSet("source") && !isTestPath(sandbox, parser.value("source")))
                      || (parser.isSet("data-dir") && !isTestPath(sandbox, parser.value("data-dir"))))) {
        qCritical("UI tests require disposable paths. Run the command through tests/isolated-ui-check.py.");
        return 1;
    }
#else
    constexpr bool uiCheckRun = false;
#endif
    applyAppImagePalette(QGuiApplication::styleHints()->colorScheme());
#ifdef MODLOCK_UI_TEST_DRIVERS
    if (screenshot && parser.value(screenshotPage) == "gallery") {
        const auto theme = qEnvironmentVariable("MODLOCK_GALLERY_THEME");
        if (theme == "dark" || theme == "light") {
            auto palette = app.palette();
            palette.setColor(QPalette::Window, QColor(theme == "dark" ? "#0e0c15" : "#f1eff5"));
            const QColor text(theme == "dark" ? "#eee8dc" : "#1b1626");
            const QColor disabled(theme == "dark" ? "#716a7b" : "#9b94a6");
            for (const auto role : {QPalette::WindowText, QPalette::Text, QPalette::ButtonText}) {
                palette.setColor(role, text);
                palette.setColor(QPalette::Disabled, role, disabled);
            }
            app.setPalette(palette);
        }
    }
#endif
    const auto platform = QGuiApplication::platformName();
    const bool startupDialog = !uiCheckRun && !automated && platform != "offscreen" && platform != "minimal";
    // The same plain startup dialog handles discovery and instance failures.
    // Automated launches must never wait for acknowledgement.
    const auto startupFailure = [&](const QString &message) {
        std::fprintf(stderr, "%s\n", message.toUtf8().constData());
        if (!startupDialog) return 1;
        QQmlApplicationEngine errorWindow;
        errorWindow.rootContext()->setContextProperty("startupError", message);
        errorWindow.loadData(R"(import QtQuick
import QtQuick.Controls
ApplicationWindow {
    visible: true
    width: 480
    height: messageDialog.implicitHeight + 32
    title: qsTr("Modlock")
    flags: Qt.Dialog | Qt.WindowTitleHint | Qt.WindowCloseButtonHint
    Dialog {
        id: messageDialog
        anchors.centerIn: parent
        width: parent.width - 32
        visible: true
        modal: true
        closePolicy: Popup.NoAutoClose
        standardButtons: Dialog.Ok
        contentItem: Label {
            text: startupError
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        onAccepted: Qt.quit()
    }
})");
        if (!errorWindow.rootObjects().isEmpty()) app.exec();
        return 1;
    };
    const bool demo = parser.isSet(QStringLiteral("demo"));
    bool validRows = false;
    const int rows = parser.value(QStringLiteral("rows")).toInt(&validRows);
    if (!validRows || rows < 1 || rows > 100000 || (!demo && parser.isSet(QStringLiteral("rows")))
        || (demo && (parser.isSet(QStringLiteral("source")) || parser.isSet(QStringLiteral("helper")) || parser.isSet(QStringLiteral("profile")) || parser.isSet(QStringLiteral("data-dir"))))) {
        qCritical("Use --demo with --rows 1–100000; source, helper and profile options apply to real mode.");
        return 1;
    }
    const QString helper = parser.isSet(QStringLiteral("helper"))
        ? QFileInfo(parser.value(QStringLiteral("helper"))).absoluteFilePath()
        : QDir(QCoreApplication::applicationDirPath()).filePath(QStringLiteral("modlock-helper"));
    const auto chosenRoot=parser.isSet("data-dir") ? parser.value("data-dir") : AppPaths::dataRoot(helper);
    if(chosenRoot.isEmpty())
        return startupFailure(AppPaths::rootSelectionError);
#ifdef MODLOCK_UI_TEST_DRIVERS
    if (fixtureRun && !isTestPath(sandbox, chosenRoot)) {
        qCritical("The selected library is outside the disposable UI test directory.");
        return 1;
    }
#endif
    const QString dataDir=QDir(chosenRoot).absolutePath();
    // One interactive window per library: a second launch raises the first.
    const bool interactive = !demo && !automated && (!uiCheckRun || uiScenario == "native-install-link-forwarded");
    QStringList pendingLinks=parser.positionalArguments();
    InstanceBroker instanceBroker;
    if (interactive) {
        const auto result=instanceBroker.start(dataDir,pendingLinks);
        // "ok" acknowledges delivery, not successful window activation. The
        // historical peer cannot report whether Wayland granted focus/raising.
        if(result==InstanceBroker::Forwarded)return 0;
        if(result==InstanceBroker::Failed) {
            qCritical("Could not contact or start the app's private instance socket.");
            const auto message = QCoreApplication::translate("main",
                "Another copy of Modlock, or the older DL Mod Manager, is already running but isn't responding. Close it, then open Modlock again.");
            return startupFailure(message);
        }
    }
    // Pin persistent state before any model or worker can read/write records.
    AppPaths::stateWriteError();
    const auto cacheRoot = parser.isSet("data-dir") ? QDir(dataDir).filePath("cache") : AppPaths::previewCacheRoot();
    if (qEnvironmentVariableIsEmpty("QML_DISK_CACHE_PATH"))
        qputenv("QML_DISK_CACHE_PATH", QDir(AppPaths::qtCacheRoot()).filePath("qmlcache").toUtf8());
    const auto previewDirectory = QDir(cacheRoot).filePath(QStringLiteral("previews"));
    PreviewNetworkFactory previewNetwork(previewDirectory, dataDir);
    // Outlives the engine; creates its audio output only on a user's Play.
    AudioPreviewController audioPreview;
    QQmlApplicationEngine engine;
    engine.rootContext()->setContextProperty(QStringLiteral("audioPreview"), &audioPreview);
    engine.setOfflineStoragePath(AppPaths::offlineStorageRoot());
    engine.rootContext()->setContextProperty(QStringLiteral("preferencesLocation"), AppPaths::preferencesLocation());
    engine.rootContext()->setContextProperty("preferencesDefaults",AppPaths::preferencesDefaults);
    engine.rootContext()->setContextProperty("preferencesSaveNotice",AppPaths::preferencesSaveNotice);
    engine.rootContext()->setContextProperty(QStringLiteral("libraryDataRoot"), dataDir);
    QFile executable(QCoreApplication::applicationFilePath());
    QCryptographicHash buildHash(QCryptographicHash::Sha256);
    const QString buildIdentifier = executable.open(QIODevice::ReadOnly) && buildHash.addData(&executable)
        ? QString::fromLatin1(buildHash.result().toHex().left(12)) : QString();
    engine.rootContext()->setContextProperty(QStringLiteral("buildIdentifier"), buildIdentifier);
    engine.addImageProvider("localartwork",new LocalArtworkProvider);
    engine.addImageProvider("remoteartwork", previewNetwork.imageProvider());
    engine.setNetworkAccessManagerFactory(&previewNetwork);
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed, &app, [] {
        QCoreApplication::exit(10);
    }, Qt::QueuedConnection);
    // UI checks serve fixture images from file: URLs; the product only loads
    // provider (https) and owned local artwork.
    engine.rootContext()->setContextProperty(QStringLiteral("uiCheckFilePreviews"), uiCheckRun);
    engine.loadFromModule(QStringLiteral("Deadlock.Native"), QStringLiteral("Main"));
    if (engine.rootObjects().isEmpty()) return 10;
    auto *root = engine.rootObjects().first();
    auto *window = qobject_cast<QQuickWindow *>(root);
    auto *model = root->findChild<LibraryModel *>(QStringLiteral("libraryModel"));
    if (!window || !model) return 10;
    SourceLoader sourceLoader(model, helper, dataDir);
    QObject::connect(&instanceBroker,&InstanceBroker::received,window,[&](const QStringList &links){
        if(pendingLinks.size()+links.size()<=10)pendingLinks.append(links);
        else QMetaObject::invokeMethod(root,"installLinkError",Q_ARG(QVariant,QStringLiteral("Too many install links. Try again after closing the current review.")));
        window->showNormal();window->raise();window->requestActivate();
    });
    QTimer installLinks;
    installLinks.setInterval(100);
    QObject::connect(&installLinks,&QTimer::timeout,window,[&]{
        if(pendingLinks.isEmpty())return;
        const auto stateError=AppPaths::stateWriteError();
        if(!stateError.isEmpty()) {
            pendingLinks.removeFirst();
            QMetaObject::invokeMethod(root,"installLinkError",Q_ARG(QVariant,stateError));
            return;
        }
        if(model->loading())return;
        QVariantMap submission;QString file;
        if(!parseInstallLink(pendingLinks.first(),submission,file)) {
            pendingLinks.removeFirst();
            QMetaObject::invokeMethod(root,"installLinkError",Q_ARG(QVariant,QStringLiteral("This install link is not supported. Use a GameBanana Mod or Sound one-click link with an HTTPS GameBanana file URL.")));
            return;
        }
        if(!model->error().isEmpty() || model->readOnly()) {
            pendingLinks.removeFirst();
            QMetaObject::invokeMethod(root,"installLinkError",Q_ARG(QVariant,QStringLiteral("Open a writable native library before using this install link.")));
            return;
        }
        QVariant ready;
        QMetaObject::invokeMethod(root,"canOpenInstallLink",Q_RETURN_ARG(QVariant, ready));
        if(!ready.toBool() || !model->remote()->canResolve())return;
        if(model->remote()->resolveInstallLink(submission,file))pendingLinks.removeFirst();
    });
    installLinks.start();
    QObject::connect(root, SIGNAL(sourceRequested(QUrl)), &sourceLoader, SLOT(openSource(QUrl)));
    if (demo) model->loadDemoData(rows);
    else {
        if (parser.isSet("source")) model->loadSource(helper, parser.value("source"), dataDir);
        else model->loadNative(helper, dataDir);
        if (parser.isSet(QStringLiteral("profile"))) model->viewProfile(parser.value(QStringLiteral("profile")));
    }
    bool performanceVisible = parser.isSet("performance");
#ifdef MODLOCK_UI_TEST_DRIVERS
    performanceVisible = performanceVisible || smoke || animationCheck;
#endif
    if (performanceVisible)
        root->setProperty("performanceVisible", true);
#ifdef MODLOCK_UI_TEST_DRIVERS
    const bool gridCheck = uiScenario == "grid-previews";
    bool gridUpdatesObserved = false;
    QObject::connect(model, &LibraryModel::updatesChanged, &app, [&]{ if (gridCheck && model->updatesChecking()) gridUpdatesObserved = true; });
    if (uiScenario == "library-flicker")
        startLibraryFlickerCheck(window, model);
    else if (uiCheckRun && !gridCheck)
        startUiCheck(window, model, uiScenario);
    else if (animationCheck)
        startAnimationCheck(window);
    else if (smoke || screenshot || gridCheck) {
#else
    if (screenshot) {
#endif
        auto *ready = new QTimer(&app);
        ready->setInterval(40);
        QObject::connect(ready, &QTimer::timeout, &app, [&, ready, attempts = 0]() mutable {
            if (++attempts > 400) { qCritical("Timed out waiting for library loading."); app.exit(16); return; }
            if (model->loading()) return;
#ifdef MODLOCK_UI_TEST_DRIVERS
            if (gridCheck && !model->updatesChecking() && !model->checkForUpdates()) return;
#endif
            ready->stop();
            bool capture = screenshot;
            int captureDelay = 700;
#ifdef MODLOCK_UI_TEST_DRIVERS
            if (smoke) {
                QMetaObject::invokeMethod(root, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("library")));
                auto *search = root->findChild<QObject *>(QStringLiteral("modSearch"));
                if (!search || !model->error().isEmpty() || (!demo && parser.isSet("source") && !model->readOnly())) { app.exit(13); return; }
                if (demo) {
                    if (model->totalCount() != rows || !model->idAt(-1).isEmpty() || !model->idAt(rows).isEmpty()) { app.exit(2); return; }
                    const int enabled = model->enabledCount();
                    model->setEnabledOnly(true);
                    if (model->count() != enabled) { app.exit(3); return; }
                    model->setEnabledOnly(false);
                    search->setProperty("text", QStringLiteral("Vindicta"));
                    if (!QMetaObject::invokeMethod(search, "textEdited") || model->query() != QStringLiteral("Vindicta")) { app.exit(14); return; }
                    search->setProperty("text", QString()); QMetaObject::invokeMethod(search, "textEdited");
                }
                auto *animation = root->findChild<QObject *>(QStringLiteral("animationTest"));
                if (!animation) { app.exit(13); return; }
                animation->setProperty("checked", true);
                window->resize(800, 600);
                QTimer::singleShot(250, window, [window] { window->resize(1200, 780); });
            }
            if (gridCheck) QMetaObject::invokeMethod(root, "screenshotSetup", Q_ARG(QVariant, QStringLiteral("grid")));
            if (parser.isSet(QStringLiteral("screenshot-page"))) {
                for (const auto &page : parser.value(screenshotPage).split(QLatin1Char(',')))
                    QMetaObject::invokeMethod(root, "screenshotSetup", Q_ARG(QVariant, page));
                captureDelay = qEnvironmentVariableIntValue("SCREENSHOT_DELAY_MS") + 4500;
            }
            if (gridCheck) captureDelay = 7000;
            capture = capture || gridCheck;
#endif
            if (capture) {
                // Occluded Wayland windows can stop receiving frames. Keep layout,
                // delegate incubation and image presentation moving while capture settles.
                auto *captureFrames = new QTimer(&app);
                captureFrames->setInterval(100);
                QObject::connect(captureFrames, &QTimer::timeout, &app, [&] {
                    if (auto *controller = engine.incubationController()) controller->incubateFor(5);
                    window->grabWindow();
                });
                captureFrames->start();
            }
            QTimer::singleShot(captureDelay, &app, [&] {
                const auto image = window->grabWindow();
#ifdef MODLOCK_UI_TEST_DRIVERS
                if (gridCheck && !checkGridCapture(window, model, gridUpdatesObserved)) { app.exit(1); return; }
                if (screenshot && parser.value(screenshotPage) == "gallery") {
                    if (!captureGallery(window, parser.value("screenshot"))) { app.exit(12); return; }
                } else
#endif
                if (screenshot && (image.isNull() || !image.save(parser.value("screenshot")))) { app.exit(12); return; }
                qInfo() << "UI check finished; mods" << model->totalCount() << "enabled" << model->enabledCount()
                        << "warnings" << model->warnings().size() << "readOnly" << model->readOnly() << "sourceError" << !model->error().isEmpty();
                app.exit(0);
            });
        });
        ready->start();
    }
    const int result = app.exec();
    audioPreview.stop();
    previewNetwork.shutdown();
    return result;
}
#include "main.moc"
