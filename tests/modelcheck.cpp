#include "apppaths.h"
#include "storagecontroller.h"
#include "maintenancecontroller.h"
#include "librarymodel.h"
#include "launchhealth.h"
#include "profilesharecontroller.h"
#include "catalogcontroller.h"
#include "librarymediacontroller.h"
#include "performancecontroller.h"
#include "optiondata.h"
#include "launchdata.h"
#include "libraryrows.h"
#include "remotedata.h"
#include "remotecontroller.h"
#include "backendclient.h"
#include "localimportcontroller.h"
#include "localartworkprovider.h"
#include <QImage>
#include <QQmlEngine>
#include <QQmlComponent>
#include <QQmlContext>
#include <sys/stat.h>
#include <fcntl.h>
#include <QFile>
#include <QFileInfo>
#include <QTemporaryDir>
#include <QCoreApplication>
#include <QElapsedTimer>
#include <QThread>
#include <QTimer>
#include <QDateTime>
#include <QProcess>
#include <QProcessEnvironment>
#include <QJsonArray>
#include <QJsonDocument>
#include <QCryptographicHash>
#include <QStandardPaths>
#include <QDir>
#include <QDirIterator>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <memory>
#include <stdexcept>
#define QVERIFY(value) do { if (!(value)) throw std::runtime_error(#value); } while (false)
#define QVERIFY2(value, message) do { if (!(value)) throw std::runtime_error(message); } while (false)
#define QCOMPARE(actual, expected) QVERIFY((actual) == (expected))
#define QTRY_VERIFY_WITH_TIMEOUT(value, timeout) do { QElapsedTimer timer; timer.start(); while (!(value) && timer.elapsed() < timeout) { QCoreApplication::processEvents(); QThread::msleep(1); } QVERIFY(value); } while (false)
static void waitEvents(int duration) { QElapsedTimer timer; timer.start(); while (timer.elapsed() < duration) { QCoreApplication::processEvents(); QThread::msleep(1); } }

#include "catalog-scheduling-check.h"
#include "settings-catalog-check.h"
#include "launch-settings-check.h"

static QJsonObject previewSnapshot(const QJsonValue &images = QJsonValue(QJsonValue::Undefined),
                                   const QJsonValue &hidden = QJsonValue(QJsonValue::Undefined)) {
    auto snapshot = QJsonDocument::fromJson(R"({"protocolVersion":1,"storeVersion":33,"sourcePath":"/synthetic/state.json",
        "activeProfileId":"default","selectedProfileId":"default","warnings":[],
        "profiles":[{"id":"default","name":"Default","isActive":true,"modCount":1}],
        "mods":[{"id":"image-mod","remoteId":"123","title":"Artwork","author":"Synthetic Author","hero":"Ivy",
            "category":"Skins","status":"installed","enabled":true,"installOrder":null,"installedVpks":[],"missingVpks":[]}]})").object();
    auto mods = snapshot.value("mods").toArray();
    auto mod = mods.first().toObject();
    mod.insert("images", images); mod.insert("previewHidden", hidden);
    mods[0] = mod; snapshot.insert("mods", mods);
    return snapshot;
}

class ModelCheck : public QObject {
    const QString root = QStringLiteral(TEST_SOURCE_DIR);
    QString helper() const { return QStringLiteral(TEST_HELPER_PATH); }
    QString fixture() const { return root + "/tests/fixtures/upstream-state-v33.json"; }
    QString peer(QTemporaryDir &dir, const QString &mode) {
        const auto path = dir.filePath(mode);
        if (!QFile::link((mode.startsWith("recovery-") ? root + "/tests/recovery-helper.py" : (mode.startsWith("native-local") ? root + "/tests/model-helper.py" : root + "/tests/controlled-helper.py")), path)) return {};
        return path;
    }
public:
    static bool pairedStartupSettled(const LibraryModel &model) {
        // Complete pairing enables launch reads that reduced-feature doubles
        // used to omit. Observe their completion before arranging an edit race.
        const auto *launch=model.m_launch;
        return model.m_client->readyForRequests() && !model.loading()
            && !launch->m_fileBusy && !launch->m_pending
            && !launch->m_settingsReadNeeded && !launch->m_barriersReadNeeded;
    }
    void paired_hello(const QJsonObject &hello, bool accepted, bool real = false, bool errorReply = false) {
        QTemporaryDir dir;
        QFile config(dir.filePath("hello.json")); QVERIFY(config.open(QIODevice::WriteOnly));
        config.write(QJsonDocument(hello).toJson()); config.close();
        if(errorReply){QFile error(dir.filePath("hello-error"));QVERIFY(error.open(QIODevice::WriteOnly));error.close();}
        const auto path=dir.filePath("peer");
        QVERIFY(QFile::link(root+"/tests/hello-peer.py",path));
        BackendClient client; QString failure; int ready=0, rejected=0;
        connect(&client,&BackendClient::failed,&client,[&](const QString &text){failure=text;});
        connect(&client,&BackendClient::capabilitiesReady,&client,[&]{++ready;});
        connect(&client,&BackendClient::rejected,&client,[&]{++rejected;});
        client.start(real ? helper() : path,{},dir.filePath("native"),true,true);
        client.nativeRequest(BackendClient::Call::SetEnabled,{{"modId","guard"},{"enabled",true}});
        QTRY_VERIFY_WITH_TIMEOUT(ready || !failure.isEmpty(),5000);
        if(accepted) {
            QVERIFY2(failure.isEmpty(),qPrintable(failure)); QCOMPARE(ready,1);
            QVERIFY(client.readyForRequests()); QVERIFY(!client.writing());
        } else {
            QVERIFY2(failure=="The app and helper do not match. Reinstall Modlock so both come from the same build.",qPrintable(failure));
            QCOMPARE(ready,0); QVERIFY(!client.readyForRequests()); QVERIFY(!client.writing());
            client.nativeRequest(BackendClient::Call::SetEnabled,{{"modId","guard"},{"enabled",true}});
        }
        QVERIFY(rejected>=1);
        if(!real) {
            QFile trace(dir.filePath("wire")); QVERIFY(trace.open(QIODevice::ReadOnly));
            QCOMPARE(trace.readAll(),QByteArray("hello\n"));
        }
        client.stop();
    }
    void paired_oneoff_admission(const QString &action) {
        QTemporaryDir dir;LibraryModel model;
        const auto peerPath=dir.filePath("peer");
        QVERIFY(QFile::link(root+"/tests/admission-controller-peer.py",peerPath));
        model.loadNative(peerPath,dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.nativeMode() && !model.busy(),7000);
        StorageController storage;MaintenanceController maintenance;ProfileShareController share;
        storage.setLibrary(&model);maintenance.setLibrary(&model);share.setLibrary(&model);
        storage.review(dir.filePath("destination"));QTRY_VERIFY_WITH_TIMEOUT(!storage.busy() && storage.result().contains("token"),5000);
        maintenance.scan();QTRY_VERIFY_WITH_TIMEOUT(!maintenance.busy() && maintenance.result().value("canClean").toBool(),5000);
        if(action=="share-privacy") {
            share.exportProfile("default",true);
            QTRY_VERIFY_WITH_TIMEOUT(!share.busy() && share.result().value("code").toString()=="fixture-with-local-name",5000);
        } else {
            share.review("fixture-code");QTRY_VERIFY_WITH_TIMEOUT(!share.busy() && share.result().value("canCreate").toBool(),5000);
        }
        QFile contract(root+"/tests/paired-helper-contract.json");QVERIFY(contract.open(QIODevice::ReadOnly));
        auto features=QJsonDocument::fromJson(contract.readAll()).array();
        for(qsizetype i=0;i<features.size();++i)if(features[i]==QJsonValue("native.crosshair.v1")){features.removeAt(i);break;}
        QFile hello(dir.filePath("hello.json"));QVERIFY(hello.open(QIODevice::WriteOnly));
        hello.write(QJsonDocument(QJsonObject{{"protocolVersion",1},{"readOnly",false},{"capabilities",features}}).toJson());hello.close();
        QFile wire(dir.filePath("wire"));QVERIFY(wire.open(QIODevice::WriteOnly|QIODevice::Truncate));wire.close();
        model.m_client->stop();model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.busy() && model.error().contains("Reinstall Modlock"),7000);
        QVERIFY(model.nativeMode());QVERIFY(!model.m_client->readyForRequests());
        if(action=="scan" || action=="storage-status-recovery")storage.scan();else if(action=="review")storage.review(dir.filePath("destination"));
        else if(action=="move")storage.move();else if(action=="resume")storage.resume();else if(action=="cancel")storage.cancel();
        else if(action=="maintenance-scan")maintenance.scan();else if(action=="clean")maintenance.clean();
        else if(action=="share-export" || action=="share-privacy")share.exportProfile("default",false);else share.review("fixture-code");
        waitEvents(250);
        QVERIFY(wire.open(QIODevice::ReadOnly));const auto traffic=wire.readAll();wire.close();
        QVERIFY2(!traffic.contains("native.storage.") && !traffic.contains("native.maintenance.") && !traffic.contains("native.profileShare."),traffic.constData());
        QVERIFY(!storage.busy());QVERIFY(!maintenance.busy());QVERIFY(!share.busy());
        if(action=="share-privacy")QVERIFY(share.result().value("code").toString().isEmpty());
        if(action=="clean" || action=="storage-status-recovery") {
            QVERIFY(contract.seek(0));const auto fullFeatures=QJsonDocument::fromJson(contract.readAll()).array();
            QVERIFY(hello.open(QIODevice::WriteOnly|QIODevice::Truncate));
            hello.write(QJsonDocument(QJsonObject{{"protocolVersion",1},{"readOnly",false},{"capabilities",fullFeatures}}).toJson());hello.close();
            model.m_client->stop();model.refresh();
            QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.nativeMode() && !model.busy(),7000);
            if(action=="storage-status-recovery")storage.scan();else storage.review(dir.filePath("destination"));
            QTRY_VERIFY_WITH_TIMEOUT(!storage.busy() && storage.result().contains("token") && storage.error().isEmpty(),5000);
            maintenance.scan();QTRY_VERIFY_WITH_TIMEOUT(!maintenance.busy() && maintenance.result().value("canClean").toBool() && maintenance.error().isEmpty(),5000);
            share.review("fixture-code");QTRY_VERIFY_WITH_TIMEOUT(!share.busy() && share.result().value("canCreate").toBool() && share.error().isEmpty(),5000);
        }
    }
    void paired_model_startup(bool source, bool probe = false) {
        QTemporaryDir dir; LibraryModel model;
        if(source && !probe) model.loadSource(peer(dir,"hello"),"/synthetic/source",dir.filePath("native"));
        else model.loadNative(peer(dir,source?"hello":"native-truth-table"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.m_client->readyForRequests(),5000);
        if(source) {
            QVERIFY(model.readOnly()); QVERIFY(!model.nativeMode()); QVERIFY(model.canImport());
            QVERIFY(!model.canSetEnabled()); QVERIFY(!model.canPreviewApply()); QVERIFY(!model.canEditProfiles());
            QVERIFY(!model.setModEnabled("image-mod",false)); QVERIFY(!model.previewApply("/synthetic/game"));
        } else {
            QTRY_VERIFY_WITH_TIMEOUT(model.launch()->canLaunch() && model.localImport()->canPrepare(),5000);
            QVERIFY(model.nativeMode()); QVERIFY(!model.readOnly());
            QVERIFY(model.canSetEnabled()); QVERIFY(model.canPreviewApply()); QVERIFY(model.canEditProfiles());
            QVERIFY(model.catalog()->available()); QVERIFY(model.remote()->available()); QVERIFY(model.remote()->replacementAvailable());
        }
        model.catalog()->browse();
        QTRY_VERIFY_WITH_TIMEOUT(model.catalog()->error()=="Synthetic catalog is offline.",5000);
        QVERIFY(model.m_client->readyForRequests());
        QVERIFY2(model.error().isEmpty(),qPrintable(model.error()));
    }
    void paired_features_do_not_upgrade_review_format() {
        QTemporaryDir dir; const auto data=dir.filePath("native"); QVERIFY(QDir().mkpath(data));
        QFile review(QDir(data).filePath("review.json")); QVERIFY(review.open(QIODevice::WriteOnly));
        review.write(R"({"token":"legacy","gamePath":"/synthetic/game","profileId":"default","fileCount":7,"replacedMounts":[],"totalBytes":42,"warnings":[]})");review.close();
        LibraryModel model;model.loadNative(peer(dir,"native-family-legacy-apply"),data);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.canPreviewApply(),5000);
        QVERIFY(model.m_client->readyForRequests());
        QVERIFY(model.previewApply("/synthetic/game"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && !model.error().isEmpty(),5000);
        QCOMPARE(model.error(),QString("Helper returned a folder review that does not match the requested layout. Review again."));
        QVERIFY(model.applyReview().isEmpty()); QVERIFY(!model.commitApply()); QVERIFY(!model.m_client->writing());
    }
    void step6_settings_ordinary(const QString &arguments) {
        LaunchSettingsPeer peer; LibraryModel model; auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch() && model.canPreviewApply(),5000);
        peer.put("hold-set");
        bool overlapAccepted=false;
        connect(launch,&LaunchController::settingsSaveStateChanged,&model,[&]{
            if(launch->settingsSaveState().value("status")=="opening")overlapAccepted=launch->saveSettings("overlap");
        });
        QVERIFY(launch->saveSettings(arguments));
        QVERIFY(!overlapAccepted); QVERIFY(!launch->saveSettings("overlap"));
        QVERIFY(!launch->canLaunch()); QVERIFY(!launch->launchActive()); QVERIFY(!model.canPreviewApply());
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("set-started"),5000);
        QVERIFY(launch->settingsSavePending()); QVERIFY(!launch->canLaunch()); QVERIFY(!model.canPreviewApply());
        peer.put("release-writer");
        QTRY_VERIFY_WITH_TIMEOUT(launch->settingsSaveState().value("status")=="confirmed" && launch->canLaunch() && model.canPreviewApply(),7000);
        QCOMPARE(launch->additionalArguments(),arguments); QCOMPARE(launch->settingsRevision(),QString("1"));
        QCOMPARE(launch->settingsSnapshot().value("additionalArguments").toString(),arguments);
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
        QCOMPARE(peer.calls("native.launch.settings.set").first()["expected"].toString(),QString("0"));
        QVERIFY(peer.calls("native.launch.settings.operation").isEmpty());
        QVERIFY(launch->launchActive());
        QTRY_VERIFY_WITH_TIMEOUT(peer.calls("native.launch.begin").size()==1,5000);
        QCOMPARE(peer.calls("native.launch.begin").first()["settingsRevision"].toString(),launch->settingsRevision());
    }
    void step6_settings_lost(const QString &mode) {
        LaunchSettingsPeer peer(mode=="characterize-stored"?QString("lost-stored"):mode); LibraryModel model; auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch() && model.canPreviewApply(),5000);
        peer.put("hold-read"); if(mode.endsWith("retry"))peer.put("fail-read");
        if(mode.endsWith("dead-read"))peer.put("dead-read");
        const auto wanted=mode.contains("same-ui")?QString("old"):QString("  -novid\t\"synthetic\"  ");
        QVERIFY(launch->saveSettings(wanted));
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("lost") && model.m_client->retiring(),5000);
        QVERIFY(!launch->canLaunch()); QVERIFY(!launch->launchActive()); QVERIFY(!model.canPreviewApply());
        QVERIFY(!launch->saveSettings("overlap"));
        const auto snapshots=peer.calls("native.snapshot").size();
        if(!mode.endsWith("automatic"))model.refresh(); waitEvents(100);
        QCOMPARE(peer.calls("native.snapshot").size(),snapshots); // Old writer still owns its lifetime.
        QVERIFY(!peer.has("writer-finished"));
        peer.put("release-writer");
        if(!mode.endsWith("automatic"))model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("read-started"),7000);
        QVERIFY(peer.has("writer-finished"));
        QVERIFY(launch->settingsSavePending()); QVERIFY(!launch->canLaunch()); QVERIFY(!model.canPreviewApply());
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
        peer.put("release-read");
        if(mode=="characterize-stored") {
            QTRY_VERIFY_WITH_TIMEOUT(launch->settingsReadState().value("status")=="succeeded",5000);
            if(launch->settingsSavePending()) {
                QTRY_VERIFY_WITH_TIMEOUT(!launch->busy(),5000);
                QVERIFY(launch->reconcileSettings());
            }
        }
        if(mode.endsWith("retry")) {
            QTRY_VERIFY_WITH_TIMEOUT(launch->settingsReadState().value("status")=="failed",5000);
            QVERIFY(launch->settingsSavePending()); QVERIFY(!launch->canLaunch()); QVERIFY(!model.canPreviewApply());
            QVERIFY(QFile::remove(QDir(peer.root).filePath("fail-read")));
            QVERIFY(launch->reconcileSettings());
        }
        if(mode.endsWith("dead-read")) {
            QTRY_VERIFY_WITH_TIMEOUT(!model.m_client->isRunning(),5000);
            QVERIFY(launch->settingsSavePending()); QVERIFY(!launch->canLaunch()); QVERIFY(!model.canPreviewApply());
            QVERIFY(QFile::remove(QDir(peer.root).filePath("dead-read")));
            QVERIFY(launch->reconcileSettings());
        }
        QTRY_VERIFY_WITH_TIMEOUT(!launch->settingsSavePending() && launch->canLaunch() && model.canPreviewApply(),7000);
        const bool stored=!mode.contains("unstored");
        const auto actual=stored?wanted:mode.contains("same-ui")?QString("external"):QString("old");
        QCOMPARE(launch->additionalArguments(),actual);
        QCOMPARE(launch->settingsRevision(),stored?QString("1"):mode.contains("same-ui")?QString("4"):QString("0"));
        QCOMPARE(launch->settingsSnapshot().value("additionalArguments").toString(),actual);
        QCOMPARE(launch->settingsSnapshot().value("revision").toString(),launch->settingsRevision());
        QCOMPARE(launch->settingsSaveState().value("status").toString(),stored?QString("confirmed"):QString("failed"));
        if(!stored)QVERIFY(launch->settingsSaveState().value("error").toString().contains("not saved",Qt::CaseInsensitive));
        waitEvents(150); QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
        if(mode!="characterize-stored")QVERIFY(peer.calls("native.launch.settings.operation").isEmpty());
        if(!stored) {
            QVERIFY(launch->saveSettings("retry"));
            QTRY_VERIFY_WITH_TIMEOUT(launch->settingsSaveState().value("status")=="confirmed" && launch->canLaunch(),7000);
            QCOMPARE(launch->additionalArguments(),QString("retry"));
            QCOMPARE(peer.calls("native.launch.settings.set").size(),2);
        }
    }
    void step6_settings_refusal(const QString &mode) {
        LaunchSettingsPeer peer(mode); LibraryModel model; auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        if(mode.endsWith("dead-read"))peer.put("dead-read");
        QVERIFY(launch->saveSettings(mode.startsWith("refused-same")?"old":"wanted"));
        if(mode.endsWith("dead-read")) {
            QTRY_VERIFY_WITH_TIMEOUT(peer.has("read-started") && !model.m_client->isRunning(),5000);
            QVERIFY(launch->settingsSavePending());QVERIFY(!model.canPreviewApply());
            QVERIFY(QFile::remove(QDir(peer.root).filePath("dead-read")));
            QVERIFY(launch->reconcileSettings());
        }
        QTRY_VERIFY_WITH_TIMEOUT(launch->settingsSaveState().value("status")=="failed" && launch->canLaunch() && model.canPreviewApply(),7000);
        QCOMPARE(launch->additionalArguments(),mode=="stale"?QString("external"):QString("old"));
        QCOMPARE(launch->settingsRevision(),mode=="stale"?QString("4"):QString("0"));
        const auto reason=mode=="stale"?QString("staleLaunchSettings: Launch settings changed; reload before saving.")
            :mode=="refused-busy"?QString("launchBusy: Close Deadlock before saving."):QString("gameRunning: Close Deadlock before saving.");
        QCOMPARE(launch->settingsSaveState().value("error").toString(),reason);
        QCOMPARE(launch->settingsError(),reason);
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
        QVERIFY(launch->saveSettings("retry"));
        QTRY_VERIFY_WITH_TIMEOUT(launch->settingsSaveState().value("status")=="confirmed" && launch->canLaunch(),7000);
        QCOMPARE(launch->additionalArguments(),QString("retry"));
        QCOMPARE(peer.calls("native.launch.settings.set").size(),2);
    }
    void step6_confirmation_control_retry() {
        LaunchSettingsPeer peer("lost-stored-get-control-refusal"); LibraryModel model;auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        QVERIFY(launch->saveSettings("wanted"));
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("lost") && model.m_client->retiring(),5000);
        peer.put("release-writer");
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("blocked-read-control") && launch->settingsReadState().value("status")=="unconfirmed",7000);
        QVERIFY(!launch->busy()); QVERIFY(launch->settingsSavePending());
        QVERIFY(!launch->canLaunch()); QVERIFY(!model.canPreviewApply());
        QVERIFY(launch->reconcileSettings());
        QTRY_VERIFY_WITH_TIMEOUT(!launch->settingsSavePending() && launch->canLaunch() && model.canPreviewApply(),7000);
        QCOMPARE(launch->additionalArguments(),QString("wanted"));
        QCOMPARE(launch->settingsSaveState().value("status").toString(),QString("confirmed"));
        QVERIFY(launch->settingsError().isEmpty());
        QVERIFY(peer.calls("native.launch.settings.get").size()>=3);
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
    }
    void step6_settings_control_read_failure(const QString &mode) {
        LaunchSettingsPeer peer(mode);LibraryModel model;auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        QVERIFY(launch->saveSettings("wanted"));
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("control-refused") && launch->settingsSaveState().value("status")=="confirmed" && launch->canLaunch(),7000);
        QCOMPARE(launch->additionalArguments(),QString("wanted"));QCOMPARE(launch->settingsRevision(),QString("1"));
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
    }
    void step6_settings_lifecycle(const QString &change) {
        LaunchSettingsPeer peer("lost-stored"); LaunchSettingsPeer next;
        LibraryModel model; auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        QVERIFY(launch->saveSettings("wanted"));
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("lost") && model.m_client->retiring(),5000);
        if(change=="close")model.requestShutdown();
        else if(change=="source")model.loadDemoData(3);
        else if(change=="root")model.loadNative(next.helper,next.root);
        else { model.m_activeProfileId="other";emit model.sourceChanged();model.refresh(); }
        waitEvents(100);
        QVERIFY(!launch->canLaunch());
        if(change=="close")QVERIFY(!model.shutdownReady());
        if(change=="source"||change=="root")QVERIFY(model.m_switching);
        peer.put("release-writer");
        if(change=="close")QTRY_VERIFY_WITH_TIMEOUT(model.shutdownReady(),7000);
        else if(change=="source")QTRY_VERIFY_WITH_TIMEOUT(model.demoMode() && !model.m_switching,7000);
        else if(change=="root") {
            QTRY_VERIFY_WITH_TIMEOUT(model.dataDir()==next.root && !model.loading() && launch->canLaunch(),7000);
            QCOMPARE(launch->additionalArguments(),QString("old")); QCOMPARE(launch->settingsRevision(),QString("0"));
            const auto before=launch->settingsSnapshot();
            model.m_client->launchReady(BackendClient::Call::LaunchControl,{{"controlId","old-control"},{"state","succeeded"},{"value",QVariantMap{{"version",1},{"revision","88"},{"additionalArguments","late"}}}});
            waitEvents(100); QCOMPARE(launch->settingsSnapshot(),before);
        } else {
            QTRY_VERIFY_WITH_TIMEOUT(launch->canLaunch(),7000);
            QCOMPARE(launch->additionalArguments(),QString("wanted"));
        }
        QCOMPARE(peer.calls("native.launch.settings.set").size(),1);
        QVERIFY(next.calls("native.launch.settings.set").isEmpty());
    }
    void step6_settings_open_reentry(const QString &change) {
        LaunchSettingsPeer peer;LibraryModel model;auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        connect(launch,&LaunchController::settingsSaveStateChanged,&model,[&]{
            if(launch->settingsSaveState().value("status")!="opening")return;
            if(change=="close")model.requestShutdown();else model.loadDemoData(3);
        });
        QVERIFY(launch->saveSettings("wanted"));
        if(change=="close")QTRY_VERIFY_WITH_TIMEOUT(model.shutdownReady(),7000);
        else QTRY_VERIFY_WITH_TIMEOUT(model.demoMode() && !model.m_switching,7000);
        QVERIFY(peer.calls("native.launch.settings.set").isEmpty());
    }
    void step6_settings_read_reentry() {
        LaunchSettingsPeer peer("lost-stored");LibraryModel model;auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        const auto context=launch->settingsContextId();
        connect(launch,&LaunchController::settingsSnapshotChanged,&model,[&]{
            if(launch->settingsContextId()==context&&launch->settingsSnapshot().value("revision")=="1")model.loadDemoData(3);
        });
        QVERIFY(launch->saveSettings("wanted"));
        QTRY_VERIFY_WITH_TIMEOUT(peer.has("lost") && model.m_client->retiring(),5000);
        peer.put("release-writer");model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(model.demoMode() && !model.m_switching,7000);
        QVERIFY(launch->settingsSaveState().isEmpty());QVERIFY(launch->settingsSnapshot().isEmpty());
        QVERIFY(launch->settingsRevision().isEmpty());QVERIFY(launch->additionalArguments().isEmpty());
        QVERIFY(launch->settingsError().isEmpty());
    }
    void step6_settings_stale_read() {
        LaunchSettingsPeer peer;LaunchSettingsPeer next;LibraryModel model;auto *launch=model.launch();
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && launch->canLaunch(),5000);
        QVERIFY(launch->readSettings());QVERIFY(launch->m_pending.has_value());
        const auto old=*launch->m_pending;
        model.loadNative(next.helper,next.root);
        QTRY_VERIFY_WITH_TIMEOUT(model.dataDir()==next.root && !model.loading() && launch->canLaunch(),7000);
        const auto before=launch->settingsSnapshot();
        // Replay the old read continuation itself, beyond transport's session filter.
        launch->complete(BackendClient::Call::LaunchSettingsGet,{{"version",1},{"revision","88"},{"additionalArguments","late"}},&old);
        QCOMPARE(launch->settingsSnapshot(),before);QCOMPARE(launch->settingsRevision(),QString("0"));
        QCOMPARE(launch->additionalArguments(),QString("old"));
    }
    void step6_settings_old_files(bool sentinel) {
        LaunchSettingsPeer peer; LibraryModel model;
        const auto key=QCryptographicHash::hash(peer.root.toUtf8(),QCryptographicHash::Sha256).toHex();
        const auto base=QDir(AppPaths::stateRoot()).filePath("pending/"+key+".launch-settings.json");
        QVERIFY(QDir().mkpath(QFileInfo(base).absolutePath()));
        struct stat before[2]{};
        const QStringList paths{base,base+".lock"};
        if(sentinel) for(int i=0;i<2;++i) {
            QFile file(paths[i]);QVERIFY(file.open(QIODevice::WriteOnly));file.write("untouched legacy sentinel\n");file.close();
            const timespec times[2]{{1,0},{2,0}};QVERIFY(::utimensat(AT_FDCWD,QFile::encodeName(paths[i]).constData(),times,0)==0);
            QVERIFY(::lstat(QFile::encodeName(paths[i]).constData(),&before[i])==0);
        }
        model.loadNative(peer.helper,peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.launch()->canLaunch(),5000);
        QVERIFY(model.launch()->saveSettings("wanted"));
        QTRY_VERIFY_WITH_TIMEOUT(model.launch()->settingsSaveState().value("status")=="confirmed" && model.launch()->canLaunch(),7000);
        for(int i=0;i<2;++i) {
            if(!sentinel){QVERIFY(!QFileInfo::exists(paths[i]));continue;}
            struct stat after{};QVERIFY(::lstat(QFile::encodeName(paths[i]).constData(),&after)==0);
            QCOMPARE(after.st_dev,before[i].st_dev);QCOMPARE(after.st_ino,before[i].st_ino);
            QCOMPARE(after.st_size,before[i].st_size);QCOMPARE(after.st_mode,before[i].st_mode);
            QCOMPARE(after.st_mtim.tv_sec,before[i].st_mtim.tv_sec);QCOMPARE(after.st_mtim.tv_nsec,before[i].st_mtim.tv_nsec);
            QCOMPARE(after.st_ctim.tv_sec,before[i].st_ctim.tv_sec);QCOMPARE(after.st_ctim.tv_nsec,before[i].st_ctim.tv_nsec);
            QCOMPARE(after.st_atim.tv_sec,before[i].st_atim.tv_sec);QCOMPARE(after.st_atim.tv_nsec,before[i].st_atim.tv_nsec);
            QFile file(paths[i]);QVERIFY(file.open(QIODevice::ReadOnly));QCOMPARE(file.readAll(),QByteArray("untouched legacy sentinel\n"));
        }
    }
    void step5_media_demand(const QString &mode) {
        LibraryMediaPeer peer;
        LibraryModel model;
        const QString id = mode == "long identity" ? QString(513, QChar(0x754c)) + "ab" : "row";
        QCOMPARE(mode == "long identity" ? id.toUtf8().size() : 0, mode == "long identity" ? 1541 : 0);
        peer.snapshot(id);
        model.loadNative(root + "/tests/library-media-peer.py", peer.root);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.nativeMode(), 5000);
        model.setSelectedId("anchor");
        model.setLibraryMediaActive(true);
        QTRY_VERIFY_WITH_TIMEOUT(model.catalog()->available(), 5000);
        const auto demand = [&](const QString &consumer, const QString &identity, bool wanted) {
            model.demandModMedia(consumer, identity, wanted);
        };
        if (mode == "unknown row") {
            demand("test", "absent", true); waitEvents(350);
            QVERIFY(peer.calls("native.catalog.begin").isEmpty());
            QVERIFY(peer.calls("native.catalog.open").isEmpty());
            return;
        }
        demand("test", id, true); demand("test", id, true);
        QTRY_VERIFY_WITH_TIMEOUT(peer.calls("native.catalog.begin").size() == 1, 5000);
        const auto begin=peer.calls("native.catalog.begin").first()["params"].toObject();
        QCOMPARE(begin["operation"].toObject()["args"].toObject()["submission"].toObject()["submissionId"].toString(), QString("1000"));
        if (mode == "withdrawal") {
            demand("test", id, false);
            QTRY_VERIFY_WITH_TIMEOUT(peer.calls("native.catalog.cancel").size() == 1, 5000);
            peer.complete("1000", "https://images.gamebanana.com/img/ss/mods/withdrawn.jpg");
            waitEvents(350);
            QVERIFY(model.modDetails(id)["images"].toStringList().isEmpty());
            QCOMPARE(peer.calls("native.catalog.begin").size(), 1);
            return;
        }
        if (mode == "epoch" || mode == "root") {
            QString oldKey, oldRoot; CatalogDecodedResult oldResult;
            bool changed=false;
            const auto retirement=QObject::connect(model.catalog(), &CatalogController::libraryMediaReady, &model,
                [&](const QString &key, const QString &at, const CatalogDecodedResult &result) {
                    if (changed || result.phase != "succeeded") return;
                    oldKey=key; oldRoot=at; oldResult=result; changed=true;
                    // Delivery to LibraryMediaController is queued: retire its
                    // owner after validation but before that result is consumed.
                    ++model.m_loadEpoch;
                    if (mode == "root") {
                        model.m_dataDir=peer.fixture.filePath("new-library");
                        QDir().mkpath(model.m_dataDir); peer.snapshot(id,model.m_dataDir);
                    }
                    model.updateLibraryMediaContext();
                });
            const auto disconnectRetirement=qScopeGuard([&] { QObject::disconnect(retirement); });
            peer.complete("1000", "https://images.gamebanana.com/img/ss/mods/old.jpg");
            QTRY_VERIFY_WITH_TIMEOUT(changed, 5000); waitEvents(100);
            QVERIFY(model.modDetails(id)["images"].toStringList().isEmpty());
            QCOMPARE(peer.calls("native.catalog.begin").size(), 1);
            if (mode == "root") {
                const auto next=model.m_dataDir;
                model.loadNative(root + "/tests/library-media-peer.py", next);
                QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.m_switching && model.sourcePath()==next+"/library.json", 5000);
                model.setSelectedId("anchor");
                QTRY_VERIFY_WITH_TIMEOUT(model.catalog()->available(), 5000);
            }
            QVERIFY(QFile::remove(peer.root + "/complete-1000"));
            demand("new-card", id, true);
            QTRY_VERIFY_WITH_TIMEOUT(peer.calls("native.catalog.begin", model.m_dataDir).size() == (mode == "root" ? 1 : 2), 5000);
            emit model.catalog()->libraryMediaReady(oldKey, oldRoot, oldResult);
            waitEvents(100);
            QVERIFY(model.modDetails(id)["images"].toStringList().isEmpty());
        }
        const QString image="https://images.gamebanana.com/img/ss/mods/current.jpg";
        peer.complete("1000", image, model.m_dataDir);
        QTRY_VERIFY_WITH_TIMEOUT(model.modDetails(id)["images"].toStringList() == QStringList{image}, 5000);
        QVERIFY(model.modDetails(id)["previewHidden"].toBool());
        QCOMPARE(peer.calls("native.catalog.begin", model.m_dataDir).size(), mode == "epoch" ? 2 : 1);
    }
    void edge_bootstrap_root_keeps_pending_creation() {
        const auto legacy=QDir(QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation)).filePath("deadlock-native");
        const auto modern=QDir(QStandardPaths::writableLocation(QStandardPaths::GenericDataLocation)).filePath("modlock");
        const auto id=QString(32,'a');const QJsonObject params{{"version",1},{"operationId",id},{"expectedRevision","0"}};
        const QJsonObject marker{{"bootstrapVersion",1},{"root",legacy},{"operationId",id},{"method",int(BackendClient::Call::LibraryInitialize)},{"expectedRevision","0"},{"params",params}};
        QVERIFY(AppPaths::prepareState());
        const auto name=QDir(AppPaths::stateRoot()).filePath("pending/"+QCryptographicHash::hash(legacy.toUtf8(),QCryptographicHash::Sha256).toHex()+".json");
        const auto bytes=QJsonDocument(marker).toJson(QJsonDocument::Compact);QFile file(name);QVERIFY(file.open(QIODevice::WriteOnly));QVERIFY(file.setPermissions(QFile::ReadOwner|QFile::WriteOwner));QCOMPARE(file.write(bytes),bytes.size());file.close();
        LibraryModel model;model.loadNative(helper(),{});QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),7000);
        QCOMPARE(model.dataDir(),legacy);QCOMPARE(model.bootstrapState(),QString("pending"));QVERIFY(model.outcomeUnknown());
        QVERIFY(!model.canCreateLibrary());QVERIFY(!model.createLibrary());QVERIFY(!QFileInfo::exists(QDir(legacy).filePath("library.sqlite3")));QVERIFY(!QFileInfo::exists(QDir(modern).filePath("library.sqlite3")));
        QVERIFY(file.open(QIODevice::ReadOnly));QCOMPARE(file.readAll(),bytes);file.close();QVERIFY(file.remove());
    }
    void model_selected_helper_discovers_root() {
        QTemporaryDir dir;const auto selected=dir.filePath("selected-root"),log=dir.filePath("discovered");
        const auto wrapper=dir.filePath("helper");QFile f(wrapper);QVERIFY(f.open(QIODevice::WriteOnly));
        const auto encoded=[](const QString &v){return QJsonDocument(QJsonArray{v}).toJson(QJsonDocument::Compact).mid(1).chopped(1);};
        f.write("#!/usr/bin/python3\nimport sys,os,json\nfrom pathlib import Path\nif sys.argv[1:]==['--data-root']:\n Path("+encoded(log)+").write_text('queried')\n print(json.dumps({'root':"+encoded(selected)+"}))\nelse:os.execv("+encoded(helper())+",["+encoded(helper())+"]+sys.argv[1:])\n");f.close();QVERIFY(QFile::setPermissions(wrapper,QFile::ReadOwner|QFile::WriteOwner|QFile::ExeOwner));
        LibraryModel model;model.loadNative(wrapper,{});QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),7000);
        QCOMPARE(model.dataDir(),selected);QVERIFY(QFileInfo::exists(log));
    }
    void model_unsupported_discovery_requires_explicit_root() {
        QTemporaryDir dir;const auto wrapper=dir.filePath("old-helper"),log=dir.filePath("stdin-closed");QFile f(wrapper);QVERIFY(f.open(QIODevice::WriteOnly));
        const auto encoded=QJsonDocument(QJsonArray{log}).toJson(QJsonDocument::Compact).mid(1).chopped(1);
        f.write("#!/usr/bin/python3\nimport sys\nfrom pathlib import Path\nsys.stdin.read()\nPath("+encoded+").write_text('closed')\n");f.close();QVERIFY(QFile::setPermissions(wrapper,QFile::ReadOwner|QFile::WriteOwner|QFile::ExeOwner));
        LibraryModel model;model.loadNative(wrapper,{});QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),7000);
        QVERIFY(model.error().contains("--data-dir"));QVERIFY(QFileInfo::exists(log));
    }
    void running_discards_queued_toggles() {
        LibraryModel model;
        LibraryMod first; first.id="first"; first.enabled=false;
        LibraryMod queued; queued.id="queued"; queued.enabled=false;
        model.m_rows->replace({first,queued});
        model.m_fullProfileOrder={QVariantMap{{"id","first"},{"enabled",false}},QVariantMap{{"id","queued"},{"enabled",false}}};
        model.m_nativeMode=true; model.m_automaticApply=true; model.m_autoCycle=true;
        model.m_autoRoot=model.m_dataDir; model.m_autoProfile=model.m_viewedProfileId;
        model.m_autoInFlight.insert("first",true);
        model.setBusy(true,true);
        QVERIFY(model.setModEnabled("queued",true));
        QVERIFY(model.modDetails("queued").value("enabled").toBool());
        QVERIFY(model.m_autoToggles.contains("queued"));
        model.m_autoBulkIds.insert("queued");
        model.m_client->launchReady(BackendClient::Call::GameStatus,{{"state","running"},{"scanComplete",true}});
        QVERIFY(model.m_autoToggles.isEmpty());
        QVERIFY(model.m_autoBulkIds.isEmpty());
        QVERIFY(!model.modDetails("queued").value("enabled").toBool());
        QCOMPARE(model.actionUnavailableReason(),QString("Close Deadlock to change your mods."));
        model.requestShutdown();
        QVERIFY(model.m_shutdownRequested);
        QVERIFY(!model.shutdownReady());
        model.m_autoInFlight.clear();
        model.setBusy(false);
        model.advanceShutdown();
        QTRY_VERIFY_WITH_TIMEOUT(model.shutdownReady(),1000);
    }
    void running_lock_survives_incomplete_scan() {
        LibraryModel model;
        auto observe=[&](const char *state, bool complete) {
            model.m_client->launchReady(BackendClient::Call::GameStatus,{{"state",state},{"scanComplete",complete}});
        };
        QVERIFY(!model.launch()->writeGate());
        observe("running",true);
        QVERIFY(model.launch()->writeGate());
        observe("unavailable",false);
        QVERIFY(model.launch()->writeGate());
        observe("idle",false);
        QVERIFY(model.launch()->writeGate());
        observe("idle",true);
        QVERIFY(!model.launch()->writeGate());
    }
    void profile_edit_review_decodes_without_mutating_shape() {
        const auto review=QJsonDocument::fromJson(R"({"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","profileId":"default","expectedRevision":"1","summary":{"version":1,"kind":"order","hasChanges":true,"canCommit":true,"before":{"modIds":["a","b"],"selection":null},"after":{"modIds":["b","a"],"selection":null},"affectedEnabledModIds":["a","b"],"precedence":{"status":"requiresApplyReview","message":"Review conflicts."},"payloadValidation":"requiresApplyValidation","message":"Saved changes require Apply","packageConstraint":{"limit":99,"selectedBefore":2,"selectedAfter":2,"beforeIssue":null,"afterIssue":null}}})").object();
        QVariantMap decoded;
        QVERIFY(OptionData::decodeReply(review,decoded));
        QCOMPARE(decoded.value("comparisonRows").toList().size(),2);
        auto extra=review;extra.insert("unexpected",true);
        QVERIFY(!OptionData::decodeReply(extra,decoded));
        auto sharded=review;auto summary=sharded.value("summary").toObject();summary.remove("packageConstraint");summary.insert("version",2);
        const auto capacity=QJsonDocument::fromJson(R"({"deploymentLayout":"shards-v1","perModLimit":99,"perShardLimit":99,"maxShards":10,"before":{"status":"Fits","counts":{"selectedReferences":2,"uniqueFamilies":2,"companionFiles":0,"physicalFiles":2,"totalBytes":20},"shards":[{"index":1,"directoryCount":2}],"partial":false,"issue":null},"after":{"status":"Fits","counts":{"selectedReferences":2,"uniqueFamilies":2,"companionFiles":0,"physicalFiles":2,"totalBytes":20},"shards":[{"index":1,"directoryCount":2}],"partial":false,"issue":null}})").object();
        summary.insert("capacity",capacity);sharded.insert("summary",summary);
        QVERIFY(OptionData::decodeReply(sharded,decoded));
    }
    void home_summary_is_bounded_and_quiet() {
        LibraryModel model;
        model.loadDemoData(0);
        QList<LibraryMod> mods;
        for (int i = 0; i < 8; ++i) {
            LibraryMod mod;
            mod.id = QString::number(i); mod.title = QString("Mod %1").arg(i); mod.enabled = true;
            mod.addedAt = QString("2026-11-%1T00:00:00Z").arg(i + 1, 2, 10, QChar('0'));
            mod.images = {QString("https://example.org/%1.png").arg(i)};
            mods.append(mod);
        }
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QQmlEngine engine;
        engine.rootContext()->setContextProperty("library", &model);
        QQmlComponent component(&engine);
        component.setData(R"(import QtQml
            QtObject {
                property var enabledSummary: library.enabledMods
                property var recentSummary: library.recentMods
                property int enabledChanges: 0
                property int recentChanges: 0
                onEnabledSummaryChanged: enabledChanges++
                onRecentSummaryChanged: recentChanges++
            })", QUrl());
        std::unique_ptr<QObject> observer(component.create());
        QVERIFY2(observer != nullptr, qPrintable(component.errorString()));
        const auto ids = [](const QVariantList &rows) { QStringList result; for (const auto &row : rows) result.append(row.toMap().value("id").toString()); return result; };
        QCOMPARE(ids(model.enabledMods()), (QStringList{"0", "1", "2", "3"}));
        QCOMPARE(ids(model.recentMods()), (QStringList{"7", "6", "5", "4", "3"}));
        const auto first = model.recentMods().first().toMap();
        QCOMPARE(first.value("title").toString(), QString("Mod 7"));
        QCOMPARE(first.value("images").toStringList(), (QStringList{"https://example.org/7.png"}));
        const int enabledChanges = observer->property("enabledChanges").toInt();
        const int recentChanges = observer->property("recentChanges").toInt();
        QVERIFY(model.setModEnabled("7", false));
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges);
        QVERIFY(model.setModEnabled("1", false));
        QCOMPARE(ids(model.enabledMods()), (QStringList{"0", "2", "3", "4"}));
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 1);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges);
        mods[1].enabled = false; mods[7].enabled = false;
        mods[1].title = "Outside both lists";
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 1);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges);
        mods[0].title = "Visible enabled title";
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 2);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges);
        mods[6].title = "Visible recent title";
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 2);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges + 1);
        mods[6].images = {"https://example.org/changed.png"};
        mods[6].previewHidden = true;
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 2);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges + 2);
        const auto changed = model.recentMods().at(1).toMap();
        QCOMPARE(changed.value("images").toStringList(), (QStringList{"https://example.org/changed.png"}));
        QVERIFY(changed.value("previewHidden").toBool());
        model.m_viewedProfileId = "another"; emit model.sourceChanged();
        QVERIFY(model.enabledMods().isEmpty());
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 3);
        QCOMPARE(observer->property("recentChanges").toInt(), recentChanges + 2);
        model.m_viewedProfileId = model.activeProfileId(); emit model.sourceChanged();
        QCOMPARE(ids(model.enabledMods()), (QStringList{"0", "2", "3", "4"}));
        QCOMPARE(observer->property("enabledChanges").toInt(), enabledChanges + 4);
        for (auto &mod : mods) mod.addedAt = "2026-11-01T00:00:00Z";
        mods[0].addedAt.clear();
        model.m_rows->replace(mods); model.notifyRowsChanged();
        QCOMPARE(ids(model.recentMods()), (QStringList{"1", "2", "3", "4", "5"}));
    }
    void review_cancel_stops_batch() {
        for (const bool batch : {false,true}) {
            LibraryModel model;auto *remote=model.remote();const QString id(32,'a');
            remote->m_jobId=id;remote->m_installStages[id]="download";
            remote->m_queueProfile="default";remote->m_queueMod="mod";remote->m_queueJob=id;
            remote->m_resolveKey=QString(32,'c');
            if(batch)remote->m_downloadQueue={QVariantMap{{"fileKey",QVariantMap{{"fileId","13"}}}}};
            QVERIFY(remote->control(id,"1","cancel"));
            QVERIFY(remote->m_downloadQueue.isEmpty());
            QVERIFY(remote->m_resolveKey.isEmpty());
            QVERIFY(!remote->m_installStages.contains(id));
            QVERIFY(remote->m_queueProfile.isEmpty());
            // A terminal status cannot put the cancelled job back into automation.
            remote->acceptStatus({{"jobId",id},{"state","cancelled"}});
            QVERIFY(!remote->m_installStages.contains(id));
        }
    }
    void review_stopped_job_does_not_block_same_submission() {
        LibraryModel model;auto *remote=model.remote();const QString id(32,'a');
        remote->m_submission={{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        remote->m_installStages[id]="stopped";remote->m_startPins[id]={{"submission",remote->m_submission}};
        remote->m_selectionToken=QString(32,'c');
        remote->m_pickedFiles={QVariantMap{{"fileKey",QVariantMap{{"fileId","13"}}}}};
        QVERIFY(remote->startSelectedFiles());
        QVERIFY(!remote->error().contains("already downloading"));
    }
    void review_cancel_allows_another_resolve() {
        QTemporaryDir dir;LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),3000);
        const QString id(32,'a');remote->m_jobId=id;remote->m_installStages[id]="download";
        remote->m_queueJob=id;remote->m_resolveKey=QString(32,'b');remote->m_downloadQueue={QVariantMap{{"fileId","13"}}};
        QVERIFY(!remote->canResolve());QVERIFY(remote->control(id,"1","cancel"));
        QVERIFY(remote->canResolve());
        QVERIFY(remote->resolve({{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}}));
    }
    void review_browse_page_link_uses_append() {
        QTemporaryDir dir;LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),3000);
        remote->m_submission={{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        LibraryMod mod;mod.id="page-link";mod.remoteSubmission=remote->m_submission.toVariantMap();
        static_cast<LibraryRows *>(model.sourceModel())->replace({mod});
        const QVariantMap row{{"fileKey",QVariantMap{{"fileId","12"}}},{"list","current"},{"archiveFlag",false},{"candidateFingerprint",QString(64,'d')}};
        remote->m_files={row};remote->m_downloadQueue={row};remote->m_selectionToken=QString(32,'a');
        remote->m_queueProfile=model.viewedProfileId();remote->m_resolutionRevision=model.revision();
        remote->m_appendTarget={{"modId",mod.id},{"recordDigest",QString(64,'e')},{"receipt",QVariant()}};
        remote->startNextFile();QCOMPARE(remote->m_pendingStarts.size(),1);
        QCOMPARE(remote->m_pendingStarts.cbegin().value()["purpose"],QJsonValue("append"));
        QCOMPARE(remote->m_pendingStarts.cbegin().value()["modId"],QJsonValue(mod.id));
    }
    void review_commit_completion_keeps_job_identity() {
        for(const bool replacement:{false,true})for(const bool success:{false,true}) {
            LibraryModel model;auto *remote=model.remote();const QString a(32,'a'),b(32,'b');
            remote->m_jobId=a;remote->m_installStages[a]="commit";
            if(replacement)remote->m_installTargets[a]={{"modId","mod"}};
            remote->m_committing=true;
            remote->replayCommit({{"jobId",a},{"method",int(replacement?BackendClient::Call::RemoteReplaceCommit:BackendClient::Call::RemoteCommit)}});
            remote->m_jobs={QVariantMap{{"jobId",b}}};
            QVERIFY(remote->selectJob(b));
            remote->m_prepareKey="b-preparation";remote->m_review={{"phase","b-review"}};
            remote->commitSettled(success,success?QString():QString("Save failed"));
            QVERIFY(!remote->m_installStages.contains(b));
            QCOMPARE(remote->m_installStages.value(a),success?QString("done"):QString("stopped"));
            QCOMPARE(remote->m_prepareKey,QString("b-preparation"));
            QCOMPARE(remote->m_review.value("phase").toString(),QString("b-review"));
            QCOMPARE(remote->m_installCompleted.contains(a),success);
        }
    }
    void review_select_all_keeps_archived_consent() {
        LibraryModel model;auto *remote=model.remote();
        const QVariantMap old{{"fileKey",QVariantMap{{"fileId","12"}}},{"list","archived"},{"archiveFlag",true}};
        const QVariantMap current{{"fileKey",QVariantMap{{"fileId","13"}}},{"list","current"},{"archiveFlag",false}};
        const QVariantMap unconfirmed{{"fileKey",QVariantMap{{"fileId","14"}}},{"list","current"},{"archiveFlag",QVariant()}};
        remote->m_files={old,current,unconfirmed};
        remote->pickAllFiles(true);QCOMPARE(remote->selectedFileIds(),QStringList{"13"});
        remote->pickFile("12",true,true);remote->pickAllFiles(true);
        QVERIFY(remote->selectedFileIds().contains("12"));QVERIFY(remote->selectedFileIds().contains("13"));
        QVERIFY(!remote->selectedFileIds().contains("14"));
        for(const auto &v:remote->m_pickedFiles)if(v.toMap().value("fileKey").toMap().value("fileId")=="12")QVERIFY(v.toMap().value("acceptArchived").toBool());
        remote->pickAllFiles(false);QVERIFY(remote->selectedFileIds().isEmpty());
    }
    void separate_hero_and_type_filters() {
        LibraryModel model; auto *rows=static_cast<LibraryRows *>(model.sourceModel());
        LibraryMod a; a.id="a"; a.title="Needle"; a.hero="Ivy"; a.kind="Skins"; a.enabled=true;
        LibraryMod b=a; b.id="b"; b.hero="Seven";
        LibraryMod c=a; c.id="c"; c.kind="Sounds"; c.enabled=false;
        LibraryMod d=a; d.id="d"; d.kind="General"; d.hero="";
        rows->replace({a,b,c,d});
        QCOMPARE(model.property("heroes").toStringList(), (QStringList{"Ivy", "Seven"}));
        QCOMPARE(model.property("types").toStringList(), (QStringList{"Skins", "Sounds"}));
        QVERIFY(model.setProperty("heroFilter", "Ivy"));
        QVERIFY(model.setProperty("typeFilter", "Skins"));
        model.setEnabledOnly(true); model.setQuery("Needle");
        QCOMPARE(model.count(), 1); QCOMPARE(model.idAt(0), QString("a"));
        model.setNeedsAttentionOnly(true); QCOMPARE(model.count(), 0);
        rows->setHealth({{"mods",QVariantList{QVariantMap{{"modId","a"},{"status","needsUpdate"}}}}});
        QCOMPARE(model.count(), 1);
        a.hero="Seven"; rows->replace({a,b,c,d}); QCOMPARE(model.count(), 0);
    }
    void review_snapshot_filter_membership() {
        LibraryModel model;auto *rows=static_cast<LibraryRows *>(model.sourceModel());
        // Exercise our explicit invalidation independently of Qt's automatic
        // role handling, which differs between Qt versions.
        model.setDynamicSortFilter(false);
        LibraryMod mod;mod.id="filtered";mod.title="Needle";mod.author="Author";mod.kind="Skins";mod.hero="Ivy";mod.enabled=true;
        rows->replace({mod});model.setEnabledOnly(true);QCOMPARE(model.count(),1);
        const QPersistentModelIndex visible(model.index(0,0));QCOMPARE(model.idAt(0),mod.id);
        int removed=0;connect(&model,&QAbstractItemModel::rowsRemoved,this,[&]{++removed;});
        mod.enabled=false;rows->replace({mod});QCOMPARE(removed,1);QCOMPARE(model.count(),0);
        mod.enabled=true;rows->replace({mod});QCOMPARE(model.count(),1);model.setEnabledOnly(false);
        for(const QString field:{"title","author","kind","hero"}) {
            const auto query=field=="title"?mod.title:field=="author"?mod.author:field=="kind"?mod.kind:mod.hero;
            model.setQuery(query);QCOMPARE(model.count(),1);
            if(field=="title")mod.title="New title";else if(field=="author")mod.author="New creator";else if(field=="kind")mod.kind="Sound";else mod.hero="Abrams";
            rows->replace({mod});QCOMPARE(model.count(),0);model.setQuery("");
        }
        model.setCategory("Abrams");QCOMPARE(model.count(),1);mod.hero="Seven";rows->replace({mod});QCOMPARE(model.count(),0);
        model.setCategory("Sound");QCOMPARE(model.count(),1);mod.kind="HUD";rows->replace({mod});QCOMPARE(model.count(),0);model.setCategory("");
        model.setUpdatesOnly(true);QCOMPARE(model.count(),0);
        rows->setUpdates({QVariantMap{{"modId",mod.id},{"status","available"}}});QCOMPARE(model.count(),1);
        rows->setUpdates({QVariantMap{{"modId",mod.id},{"status","current"}}});QCOMPARE(model.count(),0);model.setUpdatesOnly(false);
        model.setNeedsAttentionOnly(true);QCOMPARE(model.count(),0);
        rows->setHealth({{"mods",QVariantList{QVariantMap{{"modId",mod.id},{"status","needsUpdate"}}}}});QCOMPARE(model.count(),1);
        rows->setHealth({});QCOMPARE(model.count(),0);
    }
    void library_snapshot_rows_stay_in_place() {
        LibraryRows rows;
        QList<LibraryMod> mods;
        for(int i=0;i<7;++i) { LibraryMod mod;mod.id=QString("mod-%1").arg(i);mod.title="Fixture";mod.kind="Skins";mod.enabled=true;mods.append(mod); }
        mods[0].mediaRevision=17;
        rows.replace(mods);
        mods[0].mediaRevision=0; // Decoded snapshots carry no controller revision.
        const QPersistentModelIndex first(rows.index(0));
        int resets=0,changes=0;QList<int> changedRoles;
        connect(&rows,&QAbstractItemModel::modelReset,this,[&]{++resets;});
        connect(&rows,&QAbstractItemModel::dataChanged,this,[&](const QModelIndex &a,const QModelIndex &b,const QList<int> &roles){++changes;changedRoles=roles;QCOMPARE(a.row(),0);QCOMPARE(b.row(),0);});
        rows.replace(mods);
        QCOMPARE(resets,0);QCOMPARE(changes,0);QVERIFY(first.isValid());
        QCOMPARE(rows.data(first,LibraryRows::MediaRevision).toULongLong(),quint64(17));
        mods[0].title="Renamed";rows.replace(mods);
        QCOMPARE(resets,0);QCOMPARE(changes,1);QVERIFY(first.isValid());
        QCOMPARE(rows.data(first,LibraryRows::Title).toString(),QString("Renamed"));
        QCOMPARE(changedRoles,QList<int>{LibraryRows::Title});
        QCOMPARE(rows.enabledCount(),7);
        rows.replace(mods);QCOMPARE(changes,1);
        mods[0].enabled=false;rows.replace(mods);QCOMPARE(rows.enabledCount(),6);
        mods[0].hero="Ivy";rows.replace(mods);QVERIFY(rows.categories().contains("Ivy"));
        mods.removeLast();rows.replace(mods);QCOMPARE(resets,1);QCOMPARE(rows.rowCount(),6);
    }
    void library_unchanged_health_is_quiet() {
        LibraryRows rows;LibraryMod mod;mod.id="mod";rows.replace({mod});
        int changes=0;
        connect(&rows,&QAbstractItemModel::dataChanged,this,[&]{++changes;});
        rows.setHealth({});QCOMPARE(changes,0);
        const QVariantMap report{{"mods",QVariantList{QVariantMap{{"modId","mod"},{"status","healthy"},{"symptoms",QStringList{}}}}}};
        rows.setHealth(report);QCOMPARE(changes,1);
        rows.setHealth(report);QCOMPARE(changes,1);
        rows.setHealth({});QCOMPARE(changes,2);
    }
    void library_update_replies_are_quiet_when_unchanged() {
        LibraryModel model;model.loadDemoData(7);
        auto *rows=static_cast<LibraryRows *>(model.sourceModel());
        QVariantList updates;
        for(int i=0;i<7;++i)updates.append(QVariantMap{{"modId",model.idAt(i)},{"status","current"}});
        const auto reply=[&]{model.m_client->modUpdatesReady({{"profileId",model.viewedProfileId()},{"revision",model.revision()},
            {"mods",updates},{"lastUpdateCheck",QDateTime::currentSecsSinceEpoch()}});};
        reply();
        int changed=0,reset=0,inserted=0,removed=0,layout=0;
        connect(rows,&QAbstractItemModel::dataChanged,this,[&](const QModelIndex &a,const QModelIndex &b,const QList<int> &roles){++changed;QCOMPARE(a.row(),0);QCOMPARE(b.row(),0);QCOMPARE(roles,QList<int>{LibraryRows::ModUpdateStatus});});
        connect(&model,&QAbstractItemModel::modelReset,this,[&]{++reset;});
        connect(&model,&QAbstractItemModel::rowsInserted,this,[&]{++inserted;});
        connect(&model,&QAbstractItemModel::rowsRemoved,this,[&]{++removed;});
        connect(&model,&QAbstractItemModel::layoutChanged,this,[&]{++layout;});
        reply();QCOMPARE(changed,0);
        auto row=updates[0].toMap();row["status"]="available";updates[0]=row;reply();
        QCOMPARE(changed,1);QCOMPARE(reset+inserted+removed+layout,0);
        model.setUpdatesOnly(true);QCOMPARE(model.count(),1);removed=0;
        reply();QCOMPARE(changed,1);QCOMPARE(reset+inserted+removed+layout,0);
        row["fileId"]="999";updates[0]=row;reply();
        QCOMPARE(changed,1);QCOMPARE(model.modDetails(model.idAt(0)).value("update").toMap().value("fileId").toString(),QString("999"));
        row["status"]="current";updates[0]=row;reply();
        QCOMPARE(changed,2);QCOMPARE(model.count(),0);QCOMPARE(removed,1);
        QCOMPARE(reset+inserted+layout,0);
    }
    void remote_description() {
        const QJsonObject submission{{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        const QJsonObject consent{{"fileId","12"},{"originalName","variant.zip"},{"declaredSize",QJsonValue::Null},
            {"createdAt",QJsonValue::Null},{"md5",QJsonValue::Null},{"list","current"},{"archiveFlag",false}};
        const auto hash=QString::fromLatin1(QCryptographicHash::hash(LaunchData::canonical(QJsonArray{"remoteConsent/v1",submission,"Variants",consent}),QCryptographicHash::Sha256).toHex());
        auto row=consent;row.remove("fileId");
        row["fileKey"]=QJsonObject{{"submission",submission},{"fileId","12"}};
        row["submissionName"]="Variants";row["candidateFingerprint"]=hash;
        const QJsonObject request{{"version",1},{"dataDir","/synthetic"},{"sessionId",QString(32,'a')},{"requestKey",QString(32,'b')},{"page",0}};
        QJsonObject response{{"version",1},{"root","/synthetic"},{"sessionId",QString(32,'a')},{"requestKey",QString(32,'b')},
            {"phase","ready"},{"page",0},{"hasMore",false},{"selectionToken",QString(32,'c')},{"error",QJsonValue::Null}};
        const auto decode=[&] { response["candidates"]=QJsonArray{row};RemoteReply reply;
            return RemoteData::decode(BackendClient::Call::RemoteResolveStatus,response,request,reply); };
        QVERIFY2(decode(),"missing description rejected"); // Older helpers may omit the display text.
        row["description"]="reze_human";QVERIFY2(decode(),"description rejected");
        row["description"]="reze_bomb";QVERIFY(decode()); // Same fingerprint.
        row["description"]=QString(1024,'x');QVERIFY(decode());row["description"]=QString(1025,'x');QVERIFY(!decode());
        row["description"]="bad\ncontrol";QVERIFY(!decode());
    }

    void storage_status_stall(bool destroy) {
        QTemporaryDir dir; LibraryModel model;
        QVERIFY(QDir().mkdir(dir.filePath("native")));
        QFile hold(dir.filePath("native/hold-status"));
        QVERIFY(hold.open(QIODevice::WriteOnly)); hold.close();
        model.loadNative(peer(dir,"native-storage-stall"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.nativeMode() && !model.busy(),3000);

        auto storage=std::make_unique<StorageController>();
        storage->setLibrary(&model);
        storage->scan();
        const auto requests=[&]{
            QFile trace(dir.filePath("native/status-pids"));
            return trace.open(QIODevice::ReadOnly) ? trace.readAll().trimmed() : QByteArray();
        };
        QTRY_VERIFY_WITH_TIMEOUT(!requests().isEmpty(),3000);
        bool validPid=false;
        const auto pid=requests().toInt(&validPid);
        QVERIFY(validPid && pid>0);
        QVERIFY(storage->busy());
        if(destroy) {
            storage.reset();
            QTRY_VERIFY_WITH_TIMEOUT(::kill(pid,0)==-1 && errno==ESRCH,2000);
            QVERIFY(!model.busy());
            return;
        }

        QVERIFY(!model.busy());
        QVERIFY(model.canRefresh());
        model.refresh();
        QVERIFY(model.loading());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),3000);
        QVERIFY(storage->busy());

        // Simulate another owner of the library write gate when status times out.
        model.setBusy(true,true);
        QTRY_VERIFY_WITH_TIMEOUT(!storage->busy(),12000);
        QVERIFY(storage->result().isEmpty());
        QVERIFY(storage->error().contains("timed out"));
        QVERIFY(model.busy() && model.writeBusy());
        QTRY_VERIFY_WITH_TIMEOUT(::kill(pid,0)==-1 && errno==ESRCH,1000);
        model.setBusy(false);

        model.refresh();
        QVERIFY(model.loading());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),3000);
        waitEvents(250);
        QCOMPARE(requests(),QByteArray::number(pid));

        QVERIFY(hold.remove());
        storage->scan();
        QTRY_VERIFY_WITH_TIMEOUT(!storage->busy() && storage->result().contains("current"),3000);
        QVERIFY(storage->error().isEmpty());
        QCOMPARE(requests().split('\n').size(),2);

        QVERIFY(hold.open(QIODevice::WriteOnly)); hold.close();
        storage->scan();
        QTRY_VERIFY_WITH_TIMEOUT(requests().split('\n').size()==3,3000);
        QFile advanced(dir.filePath("native/revision-advanced"));
        QVERIFY(advanced.open(QIODevice::WriteOnly)); advanced.close();
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.revision()=="2",3000);
        QVERIFY(storage->busy());
        QVERIFY(storage->result().isEmpty());

        bool publishedStale=false;
        connect(storage.get(),&StorageController::changed,&model,[&]{
            if(storage->result().value("revision").toString()=="1")publishedStale=true;
        });
        QVERIFY(hold.remove());
        QTRY_VERIFY_WITH_TIMEOUT(!storage->busy() && storage->result().value("revision").toString()=="2",3000);
        QVERIFY(!publishedStale);
        QCOMPARE(requests().split('\n').size(),4);
    }
    void storage_status_ahead_of_model() {
        QTemporaryDir dir;
        LibraryModel model;
        QVERIFY(QDir().mkdir(dir.filePath("native")));
        model.loadNative(peer(dir, "native-storage-stall"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.revision() == "1", 3000);
        QFile advanced(dir.filePath("native/revision-advanced"));
        QVERIFY(advanced.open(QIODevice::WriteOnly));
        advanced.close();

        StorageController storage;
        storage.setLibrary(&model);
        storage.scan();
        QTRY_VERIFY_WITH_TIMEOUT(!storage.busy() && !storage.error().isEmpty(), 3000);
        QVERIFY(storage.result().isEmpty());
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(model.revision() == "2", 3000);
        QTRY_VERIFY_WITH_TIMEOUT(storage.result().value("revision").toString() == "2", 3000);
        QVERIFY(storage.error().isEmpty());
    }

    void storage_cancel_backend_authority() {
        for (const auto *mode : {"native-storage-pre","native-storage-post"}) {
            QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,mode),dir.filePath("native"));
            QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading(),3000);
            StorageController storage; storage.setLibrary(&model); storage.scan();
            QTRY_VERIFY_WITH_TIMEOUT(!storage.busy() && storage.result().contains("revision"),3000);
            const bool pre=QString(mode).endsWith("pre");
            QCOMPARE(storage.result().value("canCancel").toBool(),pre);
            bool offeredAfterSwitch=false;
            connect(&storage,&StorageController::changed,this,[&]{if(!pre && !storage.busy() && storage.result().value("canCancel").toBool())offeredAfterSwitch=true;});
            storage.resume();
            QTRY_VERIFY_WITH_TIMEOUT(!storage.busy() && storage.result().contains("revision"),3000);
            QVERIFY(!offeredAfterSwitch);QVERIFY(storage.result().value("resume").toBool());
            QCOMPARE(storage.result().value("canCancel").toBool(),pre);
            QCOMPARE(storage.error(),QString("Synthetic interrupted move"));
        }
    }
    void launch_unchanged_status_is_quiet() {
        LibraryModel model;
        int notifications=0;
        connect(&model,&LibraryModel::nativeChanged,this,[&]{++notifications;});
        const QVariantMap status{{"phase","reviewing"},{"sequence","1"},{"attempt",QVariantMap{{"state","pending"}}}};
        model.m_client->launchReady(BackendClient::Call::LaunchStatus,status);
        notifications=0;
        model.m_client->launchReady(BackendClient::Call::LaunchStatus,status);
        QCOMPARE(notifications,0);
    }
    void launch_health() {
        QCOMPARE(LaunchHealth::classify(1,false),"ignored");
        QCOMPARE(LaunchHealth::classify(1,true,false,true),"ignored");
        QCOMPARE(LaunchHealth::classify(299,true),"ambiguous");
        QCOMPARE(LaunchHealth::classify(300,true),"normal");
        QCOMPARE(LaunchHealth::classify(900,true,true),"crash");
        const QVariantMap a{{"id","mod"},{"title","Example"},{"enabled",true},{"variants",QStringList{"a"}},{"order",1},{"repairs",true}};
        auto b=a;b["enabled"]=false;b["variants"]=QStringList{"b"};b["order"]=2;b["repairs"]=false;
        const QVariantMap before{{"mods",QVariantList{a}},{"repairs",true},{"performance","low"},{"autoexec","hash-a"}};
        const QVariantMap after{{"mods",QVariantList{b}},{"repairs",false},{"performance","high"},{"autoexec","hash-b"}};
        QCOMPARE(LaunchHealth::differences(before,after).size(),7);
        QVERIFY(LaunchHealth::differences(before,before).isEmpty());
        const auto restored=QJsonDocument::fromJson(QJsonDocument(QJsonObject::fromVariantMap(before)).toJson()).object().toVariantMap();
        QVERIFY(LaunchHealth::differences(restored,before).isEmpty());
        QCOMPARE(LaunchHealth::differences({{"performance",QVariant()}},{{"performance","low"}}).size(),0);
        const auto unknown=QJsonDocument::fromJson("{\"performance\":null}").object().toVariantMap();
        QCOMPARE(LaunchHealth::differences(unknown,{{"performance","low"}}).size(),0);
        QCOMPARE(LaunchHealth::differences({{"performance",""}},{{"performance","low"}}).size(),1);
        QVERIFY(LaunchHealth::differences({},after).isEmpty());
        LibraryModel model;QTemporaryDir dir;
        auto health=model.launch()->health();health->load(dir.path());
        QVERIFY(health->enabled());health->capture("baseline","modded");
        const auto key=QCryptographicHash::hash(dir.path().toUtf8(),QCryptographicHash::Sha256).toHex();
        const auto healthPath=QDir(AppPaths::stateRoot()).filePath("launch-health/"+key+".json");
        QFile saved(healthPath);QVERIFY(saved.open(QIODevice::ReadOnly));
        auto state=QJsonDocument::fromJson(saved.readAll()).object();saved.close();
        state["good"]=state["launch"];
        QVERIFY(saved.open(QIODevice::WriteOnly|QIODevice::Truncate));saved.write(QJsonDocument(state).toJson());saved.close();
        health->load(dir.path());
        health->capture("unobserved","vanilla");
        QVERIFY(saved.open(QIODevice::ReadOnly));const auto beforeExit=saved.readAll();saved.close();
        int notices=0;connect(health,&LaunchHealth::changed,this,[&]{++notices;});
        health->observe("exited");
        QCOMPARE(notices,0);QVERIFY(health->report().isEmpty());
        QVERIFY(saved.open(QIODevice::ReadOnly));QCOMPARE(saved.readAll(),beforeExit);saved.close();
        health->capture("changed","vanilla");health->observe("running");health->observe("exited");
        QCOMPARE(health->report().value("classification").toString(),QString("ambiguous"));
        QCOMPARE(health->report().value("presentation").toString(),QString("notice"));
        QVERIFY(health->report().value("hasBaseline").toBool());
        health->load(dir.path());QVERIFY(health->report().isEmpty()); // Do not reopen a weak report.
        health->setEnabled(false);health->load(dir.path());QVERIFY(!health->enabled());
    }
    void external_gameinfo_state() {
        QTemporaryDir dir; QVERIFY(dir.isValid());
        LibraryModel model;
        model.loadNative(peer(dir,"native-local-external-gameinfo"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading(),7000);
        QVERIFY(model.externalGameinfoChange());
        QVERIFY(!model.externalGameinfoMessage().isEmpty());
        QVERIFY(!model.recoveryRequired());
        QVERIFY(!model.canRecover());
        QVERIFY(!model.canRestore());
        QVERIFY(model.canPreviewApply());
    }
    void catalog_automatic_refresh_on_open_and_reopen() {
        QTemporaryDir dir; QVERIFY(dir.isValid());
        BackendClient client;
        CatalogController catalog(&client);
        int syncBegins=0, syncTerminals=0;
        QObject::connect(&client,&BackendClient::catalogReady,&catalog,[&](BackendClient::Call call,const CatalogDecodedResult &r){
            if(r.kind!="sync")return;
            if(call==BackendClient::Call::CatalogBegin)++syncBegins;
            if(call==BackendClient::Call::CatalogStatus&&(r.phase=="cancelled"||r.phase=="failed"||r.phase=="succeeded"))++syncTerminals;
        });
        client.start(helper(),fixture(),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(client.readyForRequests(),7000);
        catalog.browse();
        QTRY_VERIFY_WITH_TIMEOUT(syncBegins==1,10000);
        QVERIFY(catalog.contentReady());
        catalog.cancel();
        QTRY_VERIFY_WITH_TIMEOUT(syncTerminals>=1,10000);
        catalog.setActive(false);
        catalog.browse();
        QTRY_VERIFY_WITH_TIMEOUT(syncBegins==2,10000);
        catalog.cancel();
        catalog.shutdown();
    }
    void health_real_api() {
        QTemporaryDir dir(QDir::homePath()+"/.cache/sol-health-model-XXXXXX");QVERIFY(dir.isValid());
        const auto data=dir.filePath("native");QVERIFY(QDir().mkpath(dir.filePath("game/citadel/addons")));
        QProcess importer;importer.start(helper());QVERIFY(importer.waitForStarted());qint64 id=0;
        const auto rpc=[&](const QString &method,QJsonObject params){params.insert("dataDir",data);importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');QVERIFY(importer.waitForBytesWritten());while(!importer.canReadLine())QVERIFY(importer.waitForReadyRead(7000));auto r=QJsonDocument::fromJson(importer.readLine()).object();QVERIFY2(!r.contains("error"),QJsonDocument(r).toJson().constData());return r.value("result").toObject();};
        QFile input(fixture());QVERIFY(input.open(QIODevice::ReadOnly));auto source=QJsonDocument::fromJson(input.readAll()).object();auto config=source["local-config"].toObject();auto state=config["state"].toObject();state["localMods"]=QJsonArray{};auto profiles=state["profiles"].toObject();for(const auto &id:profiles.keys()){auto profile=profiles[id].toObject();profile["mods"]=QJsonArray{};profile["enabledMods"]=QJsonObject{};profiles[id]=profile;}state["profiles"]=profiles;config["state"]=state;source["local-config"]=config;
        const auto sourcePath=dir.filePath("health-state.json");QFile output(sourcePath);QVERIFY(output.open(QIODevice::WriteOnly));output.write(QJsonDocument(source).toJson());output.close();
        auto r=rpc("native.importPreview",{{"statePath",sourcePath},{"gamePath",dir.path()}});
        rpc("native.importCommit",{{"token",r.value("token")},{"operationId",QString(32,'c')},{"expectedRevision","0"}});importer.closeWriteChannel();QVERIFY(importer.waitForFinished());
        LibraryModel model;model.loadNative(helper(),data);QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode()&&!model.loading(),15000);
        const auto revision=model.revision();
        QTRY_VERIFY_WITH_TIMEOUT(model.health().value("report").toMap().contains("mods"),15000);
        QVERIFY2(model.health().value("error").isNull(),QJsonDocument::fromVariant(model.health()).toJson().constData());
        QVERIFY(model.health().value("report").toMap().contains("mods"));QCOMPARE(model.revision(),revision);
        QVERIFY(model.fixAutomatically());QVERIFY(model.setFixAutomatically(false));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy(),10000);QVERIFY(!model.fixAutomatically());
        QVERIFY(model.setFixAutomatically(true));QTRY_VERIFY_WITH_TIMEOUT(!model.busy(),10000);QVERIFY(model.fixAutomatically());
    }
    void health_unavailable_api() {
        QTemporaryDir dir(QDir::homePath()+"/.cache/sol-health-model-XXXXXX");QVERIFY(dir.isValid());
        const auto data=dir.filePath("native");QVERIFY(QDir().mkpath(dir.filePath("game/citadel/addons")));
        QProcess importer;importer.start(helper());QVERIFY(importer.waitForStarted());qint64 id=0;
        const auto rpc=[&](const QString &method,QJsonObject params){params.insert("dataDir",data);importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');QVERIFY(importer.waitForBytesWritten());while(!importer.canReadLine())QVERIFY(importer.waitForReadyRead(7000));auto r=QJsonDocument::fromJson(importer.readLine()).object();QVERIFY2(!r.contains("error"),QJsonDocument(r).toJson().constData());return r.value("result").toObject();};
        auto r=rpc("native.importPreview",{{"statePath",fixture()},{"gamePath",dir.path()}});
        rpc("native.importCommit",{{"token",r.value("token")},{"operationId",QString(32,'c')},{"expectedRevision","0"}});importer.closeWriteChannel();QVERIFY(importer.waitForFinished());
        LibraryModel model;model.loadNative(helper(),data);QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode()&&!model.loading(),15000);
        const auto revision=model.revision();QVERIFY(model.healthChecking() || model.checkModHealth());
        QTRY_VERIFY_WITH_TIMEOUT(!model.healthChecking(),15000);
        QCOMPARE(model.health().value("error").toMap().value("kind").toString(),QString("invalidLocalSelection"));QCOMPARE(model.revision(),revision);
        QVERIFY(model.error().isEmpty());
    }
    void mod_updates_api() {
        LibraryModel model;
        for(const auto *name:{"updateCount","lastUpdateCheck","updatingCount","updatesChecking","updatesOnly","updatesOffline"}) QVERIFY(model.metaObject()->indexOfProperty(name)>=0);
        QVERIFY(!model.checkForUpdates());QVERIFY(!model.updateAll());
        model.loadDemoData(3);
        auto *rows=static_cast<LibraryRows *>(model.sourceModel());
        const auto id=rows->data(rows->index(0),LibraryRows::ModId).toString();
        const QVariantList updates{QVariantMap{{"modId",id},{"status","available"},{"createdAt",123}}};
        model.m_client->modUpdatesReady({{"profileId",model.viewedProfileId()},{"revision",model.revision()},{"mods",updates},{"lastUpdateCheck",QDateTime::currentSecsSinceEpoch()}});
        QCOMPARE(model.updateCount(),1);QCOMPARE(model.updatingCount(),0);QVERIFY(!model.updateAll());
        model.setUpdatesOnly(true);QCOMPARE(model.count(),1);
        QCOMPARE(model.modDetails(id).value("update").toMap().value("createdAt").toInt(),123);
        rows->setUpdates({});QCOMPARE(model.count(),0);
        model.setUpdatesOnly(false);QCOMPARE(model.count(),3);
        // An offline refresh ends the remaining network batch, retaining cached results.
        model.m_updateResourceKey="fixture-offline";
        model.m_updateResources.enqueue({SubmissionKey{"mod","123"},"files"});
        CatalogDecodedResult failure;failure.errorKind="catalogTransport";failure.phase="failed";
        model.catalog()->libraryMediaReady("fixture-offline",model.dataDir(),failure);
        QVERIFY(model.updatesOffline());QVERIFY(model.m_updateResources.isEmpty());
        QVERIFY(model.m_updateResourceKey.isEmpty());
        // A late receipt must not finish or overwrite the next check.
        model.m_updateResourceKey="next-check";model.m_updatesOffline=false;
        model.catalog()->libraryMediaReady("fixture-offline",model.dataDir(),failure);
        QVERIFY(!model.updatesOffline());QCOMPARE(model.m_updateResourceKey,QString("next-check"));

    }
    void update_status_error_is_local() {
        LibraryModel model;model.loadDemoData(3);
        QVERIFY(model.error().isEmpty());
        model.m_client->rejected(BackendClient::Call::ModUpdatesGet,"nativeIo","Synthetic update read failure");
        QVERIFY(model.error().isEmpty());
        QVERIFY(model.metaObject()->indexOfProperty("updatesError")>=0);
        QVERIFY(!model.property("updatesError").toString().isEmpty());
        model.m_client->modUpdatesReady({{"profileId",model.viewedProfileId()},{"revision",model.revision()},
            {"mods",QVariantList{}},{"lastUpdateCheck",QDateTime::currentSecsSinceEpoch()}});
        QVERIFY(model.property("updatesError").toString().isEmpty());
    }
    void update_all_unselected_target() {
        struct Environment { QByteArray old=qgetenv("INSTALL_LINK_CASE"); ~Environment(){ if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old); } } environment;
        qputenv("INSTALL_LINK_CASE","page-linked-bulk");
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && model.updateCount()==1 && model.remote()->replacementAvailable(),10000);
        model.setSelectedId("unrelated");QCOMPARE(model.selectedId(),QString("unrelated"));
        QVERIFY(model.updateAll());
        QTRY_VERIFY_WITH_TIMEOUT(model.remote()->replacementActive(),3000);
        QTRY_VERIFY_WITH_TIMEOUT(!model.remote()->m_replaceTarget.isEmpty(),10000);
        QCOMPARE(model.selectedId(),QString("unrelated"));
        QCOMPARE(model.remote()->m_replaceTarget.value("modId").toString(),QString("page-linked"));
        QVERIFY(model.remote()->replacementContextCurrent());
        model.setSelectedId("page-linked");model.setSelectedId("unrelated");
        QVERIFY(model.remote()->replacementContextCurrent());
        const auto revision=model.remote()->m_replaceRevision;model.remote()->m_replaceRevision="999";
        QVERIFY(!model.remote()->replacementContextCurrent());model.remote()->m_replaceRevision=revision;
    }
    void closing_batch_review_cancels_remaining() {
        LibraryModel model;
        model.m_updateReviews.enqueue("next");
        model.m_updateReviewActive=true;
        model.remote()->m_batchReplacement=true;
        model.remote()->m_replaceAction="update";
        model.remote()->close();
        QVERIFY(model.m_updateReviews.isEmpty());
        QVERIFY(!model.m_updateReviewActive);
        const QString job(32,'a');model.m_updateReviewActive=true;model.m_updateReviews.enqueue("next");
        model.m_updateBatchConfirmed=true;
        model.remote()->m_installStages[job]="download";
        model.remote()->m_replaceAction.clear();
        QVERIFY(!model.remote()->replacementActive());
        model.remote()->close();
        QCOMPARE(model.m_updateReviews.size(),1);
        QVERIFY(model.m_updateReviewActive);
        QCOMPARE(model.remote()->m_installStages.value(job),QString("download"));
        model.cancelRemainingUpdates();
        QVERIFY(model.m_updateReviews.isEmpty());
        QVERIFY(!model.m_updateReviewActive);
    }
    void installed_history_does_not_hide_browse_download() {
        LibraryModel model;model.loadDemoData(0);
        const QVariantMap submission{{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        QVariantMap job{{"jobId",QString(32,'a')},{"profileId",model.viewedProfileId()},{"submission",submission},{"state","installed"},{"sequence","1"}};
        model.remote()->m_jobs={job};
        QCOMPARE(model.catalog()->libraryAction(submission).value("action").toString(),QString("download"));
        job["state"]="transferring";model.remote()->m_jobs={job};
        QCOMPARE(model.catalog()->libraryAction(submission).value("action").toString(),QString("progress"));
    }
    void item106_review_f6_queue_owner() {
        struct Env { QByteArray old=qgetenv("INSTALL_LINK_CASE"),autoOld=qgetenv("INSTALL_LINK_AUTO"); ~Env(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);if(autoOld.isNull())qunsetenv("INSTALL_LINK_AUTO");else qputenv("INSTALL_LINK_AUTO",autoOld);} } env;
        qputenv("INSTALL_LINK_CASE","multi");qputenv("INSTALL_LINK_AUTO","1");
        QTemporaryDir dir;LibraryModel model;model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),10000);
        QVERIFY(remote->resolve({{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}}));
        QTRY_VERIFY_WITH_TIMEOUT(remote->files().size()==2,10000);
        remote->pickAllFiles(true);QVERIFY(remote->startSelectedFiles());
        const auto owner=remote->currentJobId();const auto remaining=remote->m_downloadQueue;QCOMPARE(remaining.size(),1);
        const auto resolution=remote->m_resolveKey;const auto profile=remote->m_queueProfile;
        remote->m_session.clear();remote->m_pendingStarts.clear(); // The first job is admitted; no helper controls are needed here.
        remote->m_statuses[owner]={{"jobId",owner},{"state","transferring"}};
        const QString other(32,'e');remote->m_statuses[other]={{"jobId",other},{"state","failed"},{"sequence","1"}};
        for(const auto &action:QStringList{"cancel","dismiss","abandon"}) {
            remote->m_installStages[other]="stopped";
            QVERIFY(remote->control(other,"1",action));
            QCOMPARE(remote->m_downloadQueue,remaining);QCOMPARE(remote->m_resolveKey,resolution);QCOMPARE(remote->m_queueProfile,profile);
            remote->m_pendingControls.remove(other);
        }
        remote->m_installStages[other]="download";
        remote->acceptStatus({{"jobId",other},{"state","cancelled"},{"sequence","2"}});
        QCOMPARE(remote->m_downloadQueue,remaining);
        remote->m_jobId=other;remote->m_job=remote->m_statuses[other];
        remote->advanceInstall();QCOMPARE(remote->m_downloadQueue,remaining);
        QVERIFY(remote->control(owner,"1","cancel"));
        QVERIFY(remote->m_downloadQueue.isEmpty());QVERIFY(remote->m_resolveKey.isEmpty());QVERIFY(remote->m_queueProfile.isEmpty());
    }
    void item106_review_f7_continuation_errors() {
        QTemporaryDir dir;LibraryModel model;model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),10000);
        const QString id(32,'a');remote->m_jobId=id;remote->m_job={{"jobId",id},{"state","downloaded"},{"profileId","another-profile"}};
        remote->continueDownloaded();QVERIFY(remote->error().contains("Switch to this download's profile"));
        remote->m_session=QString(32,'b');remote->m_root=model.m_client->nativeRoot();
        remote->m_replaceParams={{"requestKey",QString(32,'c')}};
        remote->received(BackendClient::Call::RemoteReplaceTarget,{{"dataDir",remote->m_root},{"sessionId",remote->m_session},{"requestKey",QString(32,'c')}},{{"phase","failed"},{"error",QVariantMap{{"kind","providerUnavailable"},{"message","Network unavailable"}}}});
        QCOMPARE(remote->error(),QString("The provider check did not complete. Try again later."));
    }
    void item106_review_f1_replacement_paths() {
        struct Env { QByteArray old=qgetenv("INSTALL_LINK_CASE"),autoOld=qgetenv("INSTALL_LINK_AUTO"),purposeOld=qgetenv("INSTALL_LINK_PURPOSE"); ~Env(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);if(autoOld.isNull())qunsetenv("INSTALL_LINK_AUTO");else qputenv("INSTALL_LINK_AUTO",autoOld);if(purposeOld.isNull())qunsetenv("INSTALL_LINK_PURPOSE");else qputenv("INSTALL_LINK_PURPOSE",purposeOld);} } env;
        qputenv("INSTALL_LINK_AUTO","1");
        qputenv("INSTALL_LINK_CASE","known-receipt");
        for(const auto &purpose:QStringList{"force","reinstall","link"}) {
            qputenv("INSTALL_LINK_PURPOSE",purpose.toUtf8());
            QTemporaryDir dir;LibraryModel model;model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
            QTRY_VERIFY_WITH_TIMEOUT(remote->replacementAvailable() && !model.loading(),10000);
            model.setSelectedId("page-linked");QVERIFY(remote->beginReplacement("page-linked","update"));
            QTRY_VERIFY_WITH_TIMEOUT(!remote->files().isEmpty() && !remote->m_selectionToken.isEmpty(),10000);
            const auto file=remote->files().first().toMap();const auto fileId=file.value("fileKey").toMap().value("fileId").toString();
            remote->m_replaceAction=purpose=="force"?"update":purpose;
            if(purpose=="force")remote->m_replaceSkip={{"state","present"},{"entry",QVariantMap{{"fileId",fileId},{"candidateFingerprint",file.value("candidateFingerprint")}}}};
            QVERIFY(remote->startReplacement(fileId,file.value("candidateFingerprint").toString(),purpose,false));
            const auto job=remote->currentJobId();QVERIFY(remote->m_installStages.value(job)=="download");
            try {QTRY_VERIFY_WITH_TIMEOUT(remote->m_installStages.value(job)=="done",15000);}
            catch(const std::exception &) {throw std::runtime_error((purpose+": stage="+remote->m_installStages.value(job)+" state="+remote->job().value("state").toString()+" error="+remote->error()+" library="+model.error()+" review="+remote->review().value("phase").toString()).toStdString());}
        }
    }
    void item106_review_f2_retained_download() {
        struct Env { QByteArray old=qgetenv("INSTALL_LINK_CASE"),autoOld=qgetenv("INSTALL_LINK_AUTO"); ~Env(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);if(autoOld.isNull())qunsetenv("INSTALL_LINK_AUTO");else qputenv("INSTALL_LINK_AUTO",autoOld);} } env;
        qputenv("INSTALL_LINK_AUTO","1");
        qputenv("INSTALL_LINK_CASE","one");QTemporaryDir dir;LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),10000);
        QString retained;
        QObject::connect(remote,&RemoteController::statusEvidence,&model,[&](const QVariantMap &status){
            if(retained.isEmpty() && status.value("state")=="downloaded") {
                retained=status.value("jobId").toString();remote->m_installStages.clear();remote->m_consents.clear();remote->m_installMetadata.clear();remote->m_startPins.clear();remote->m_installTargets.clear();remote->m_jobId.clear();remote->m_job.clear();
            }
        });
        QVERIFY(remote->resolveInstallLink({{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}},"12"));
        QTRY_VERIFY_WITH_TIMEOUT(remote->visible() && !remote->files().isEmpty() && !remote->m_selectionToken.isEmpty(),10000);
        QCOMPARE(remote->selectedFileIds(),QStringList{"12"});
        QVERIFY(remote->startSelectedFiles());
        QTRY_VERIFY_WITH_TIMEOUT(!retained.isEmpty(),10000);
        // A resumed transfer may already have stopped on missing consent.
        remote->m_installStages[retained]="stopped";remote->m_installErrors[retained]="Select this download's profile and continue manually.";
        QVERIFY(remote->control(retained,{},"install"));
        QTRY_VERIFY_WITH_TIMEOUT(remote->visible() && !remote->files().isEmpty(),10000);
        QCOMPARE(remote->currentJobId(),retained);QVERIFY(remote->m_pendingStarts.isEmpty());
        const auto file=remote->files().first().toMap();
        QVERIFY(remote->bindConsent(retained,"12",file.value("candidateFingerprint").toString(),false));
        QVERIFY(remote->control(retained,{},"install"));
        QTRY_VERIFY_WITH_TIMEOUT(remote->m_installStages.value(retained)=="done",15000);
        QVERIFY(!model.modDetails("installed-variant").isEmpty());
    }
    void item106_review_f3_download_destination() {
        QTemporaryDir dir;LibraryModel model;model.loadNative(peer(dir,"native-links"),dir.filePath("native"));auto *remote=model.remote();
        QTRY_VERIFY_WITH_TIMEOUT(remote->available() && !model.loading(),10000);
        const QVariantMap submission{{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        const QVariantMap fileKey{{"submission",submission},{"fileId","13"}};
        remote->m_resolveKey=QString(32,'b');
        remote->m_downloadQueue={QVariantMap{{"fileKey",fileKey},{"candidateFingerprint",QString(64,'c')}}};
        QVERIFY(remote->hasResolution());
        QVERIFY(!remote->downloadsRequested());
        remote->showDownloads();
        QVERIFY(remote->visible());
        QVERIFY(remote->downloadsRequested());
    }
    void item106_review_f4_two_row_controls() {
        LibraryModel model;auto *remote=model.remote();const QString a(32,'a'),b(32,'b');remote->m_jobId=a;
        remote->m_jobs={QVariantMap{{"jobId",a},{"state","transferring"}},QVariantMap{{"jobId",b},{"state","transferring"}}};
        QVERIFY(remote->control(a,{},"pause"));QVERIFY(remote->control(b,{},"cancel"));
        remote->acceptStatus({{"jobId",a},{"state","transferring"},{"sequence","9"},{"lastControl",QVariantMap{{"controlSeq","7"}}}});
        remote->acceptStatus({{"jobId",b},{"state","transferring"},{"sequence","5"},{"lastControl",QVariantMap{{"controlSeq","3"}}}});
        QCOMPARE(remote->m_pendingControls[a]["action"].toString(),QString("pause"));QCOMPARE(remote->m_pendingControls[a]["controlSeq"].toString(),QString("8"));
        QCOMPARE(remote->m_pendingControls[b]["action"].toString(),QString("cancel"));QCOMPARE(remote->m_pendingControls[b]["controlSeq"].toString(),QString("4"));
    }
    void item106_review_f5_link_wins() {
        LibraryModel model;auto *remote=model.remote();remote->m_root=model.m_client->nativeRoot();remote->m_session=QString(32,'a');remote->m_resolveKey=QString(32,'b');remote->m_collectFiles=true;remote->m_linkedFileId="12";
        remote->m_submission={{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        QVariantList files;
        for(const auto &id:QStringList{"12","13"})files.append(QVariantMap{{"fileKey",QVariantMap{{"submission",remote->m_submission.toVariantMap()},{"fileId",id}}},{"candidateFingerprint",QString(64,'c')},{"createdAt",id=="12"?100:200},{"list","current"},{"archiveFlag",false}});
        remote->received(BackendClient::Call::RemoteResolveStatus,{{"dataDir",remote->m_root},{"sessionId",remote->m_session},{"requestKey",remote->m_resolveKey},{"page",0}},{{"phase","ready"},{"candidates",files},{"selectionToken",QString(32,'d')},{"hasMore",false}});
        QCOMPARE(remote->selectedFileIds(),QStringList{"12"});
    }
    void item106_newest_file_preselected() {
        LibraryModel model;auto *remote=model.remote();
        remote->m_root=model.m_client->nativeRoot();remote->m_session=QString(32,'a');remote->m_resolveKey=QString(32,'b');
        remote->m_submission={{"provider","gamebanana"},{"submissionType","mod"},{"submissionId","42"}};
        remote->m_collectFiles=true;
        QVariantList files;
        for(const auto &entry:QList<QPair<QString,int>>{{"12",100},{"13",300},{"14",200},{"15",500}}) {
            files.append(QVariantMap{{"fileKey",QVariantMap{{"submission",remote->m_submission.toVariantMap()},{"fileId",entry.first}}},
                {"candidateFingerprint",QString(64,'c')},{"createdAt",entry.second},{"list",entry.first=="15"?"archived":"current"},{"archiveFlag",entry.first=="15"}});
        }
        const QJsonObject request{{"dataDir",remote->m_root},{"sessionId",remote->m_session},{"requestKey",remote->m_resolveKey},{"page",0}};
        remote->received(BackendClient::Call::RemoteResolveStatus,request,{{"phase","ready"},{"candidates",files},{"selectionToken",QString(32,'d')},{"hasMore",false}});
        QCOMPARE(remote->selectedFileIds(),QStringList{"13"});
        QVERIFY(remote->visible());QVERIFY(remote->m_downloadQueue.isEmpty());
    }
    void item106_finished_downloads_disappear() {
        LibraryModel model;auto *remote=model.remote();const QString id(32,'a');
        remote->m_jobs={QVariantMap{{"jobId",id},{"state","installed"},{"sequence","2"}}};
        LibraryMod mod;mod.id="removed-mod";mod.title="Downloaded mod";
        model.m_rows->replace({mod});
        remote->m_installStages[id]="done";remote->m_installResultMods[id]=mod.id;
        QVERIFY(remote->jobs().isEmpty());
        model.m_rows->replace({}); // current profile removal or permanent deletion
        QVERIFY(model.modDetails(mod.id).isEmpty());
        QVERIFY(remote->jobs().isEmpty());
        remote->m_jobs={QVariantMap{{"jobId",id},{"state","transferring"},{"sequence","1"}}};
        remote->m_statuses[id]={{"jobId",id},{"state","installed"},{"sequence","2"}};
        QVERIFY(remote->jobs().isEmpty());
    }
    void item106_control_waits_for_job_status() {
        LibraryModel model;auto *remote=model.remote();const QString id(32,'b');
        remote->m_jobs={QVariantMap{{"jobId",id},{"state","transferring"}}};
        QVERIFY(remote->control(id,{},"pause"));
        QVERIFY(remote->m_pendingControls.isEmpty());
        remote->acceptStatus({{"jobId",id},{"state","transferring"},{"sequence","9"},{"lastControl",QVariantMap{{"controlSeq","7"}}}});
        QCOMPARE(remote->m_pendingControls[id]["action"].toString(),QString("pause"));
        QCOMPARE(remote->m_pendingControls[id]["controlSeq"].toString(),QString("8"));
    }
    void item106_player_controls() {
        LibraryModel model;auto *remote=model.remote();const QString id(32,'a');
        remote->m_jobId=id;remote->m_job={{"jobId",id},{"state","transferring"}};
        QVERIFY(remote->control(id,remote->nextControlSeq(),"pause"));
        QCOMPARE(remote->m_pendingControls[id]["action"].toString(),QString("pause"));
        remote->m_pendingControls.clear();
        QVERIFY(remote->control(id,remote->nextControlSeq(),"cancel"));
        QCOMPARE(remote->m_pendingControls[id]["action"].toString(),QString("cancel"));
    }
    void repeated_download_failures_have_new_identity() {
        LibraryModel model; auto *remote=model.remote();
        const QString id(32,'a');
        remote->m_jobId=id;remote->m_jobs={QVariantMap{{"jobId",id},{"state","ready"}}};
        remote->stopInstall("Could not review this download.");
        const auto first=remote->jobs().first().toMap().value("failureSequence").toULongLong();
        QVERIFY(first>0);
        QVERIFY(!remote->addSelection({}));
        const auto second=remote->jobs().first().toMap().value("failureSequence").toULongLong();
        QVERIFY(second>first);
    }
    void item103_busy_preparation_keeps_progress() {
        LibraryModel model; auto *remote=model.remote();
        remote->m_root=model.m_client->nativeRoot();remote->m_session=QString(32,'a');
        remote->m_jobId=QString(32,'b');remote->m_prepareKey=QString(32,'c');
        remote->m_installStages[remote->m_jobId]="candidates";
        QJsonObject request{{"dataDir",remote->m_root},{"sessionId",remote->m_session},{"jobId",remote->m_jobId},{"prepareKey",remote->m_prepareKey}};
        remote->m_prepareParams=request;
        remote->rejected(BackendClient::Call::RemotePrepare,request,"nativeBusy","Library is busy.");
        QVERIFY(remote->error().isEmpty());
        QVERIFY(remote->m_releaseKey.isEmpty());
        QVERIFY(!remote->m_prepareParams.isEmpty());
        remote->advanceInstall();
        QCOMPARE(remote->m_installStages.value(remote->m_jobId),QString("candidates"));
    }
    void background_download_errors_are_not_notices() {
        LibraryModel model;auto *remote=model.remote();
        remote->m_error="Background download status could not be checked.";
        QVERIFY(remote->metaObject()->indexOfProperty("noticeError")>=0);
        QVERIFY(remote->property("noticeError").toString().isEmpty());
    }
    void update_all_shows_one_batch_review() {
        struct Environment { QByteArray old=qgetenv("INSTALL_LINK_CASE"); ~Environment(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);} } environment;
        qputenv("INSTALL_LINK_CASE","page-linked-bulk");
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && model.updateCount()==1 && model.remote()->replacementAvailable(),10000);
        model.m_modUpdates={QVariantMap{{"modId","page-linked"},{"status","available"},{"fileId","12"}},QVariantMap{{"modId","unrelated"},{"status","available"},{"fileId","12"}}};
        QVERIFY(model.updateAll());
        QVERIFY(model.metaObject()->indexOfProperty("batchUpdateReview")>=0);
        QVERIFY(model.property("batchUpdateReview").toBool());
        QCOMPARE(model.property("batchUpdateTargets").toList().size(),2);
        QVERIFY(model.remote()->m_pendingStarts.isEmpty());
        model.remote()->close();
        QCOMPARE(model.updatingCount(),0);
    }
    void update_all_rechecks_reviewed_record_identity() {
        struct Environment { QByteArray old=qgetenv("INSTALL_LINK_CASE"); ~Environment(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);} } environment;
        qputenv("INSTALL_LINK_CASE","page-linked-bulk");
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && model.updateCount()==1 && model.remote()->replacementAvailable(),10000);
        auto row=model.m_modUpdates.first().toMap();row["recordDigest"]=QString(64,'0');model.m_modUpdates={row};
        QVERIFY(model.updateAll());
        QTRY_VERIFY_WITH_TIMEOUT(model.remote()->replacementTargetCurrent(),10000);
        model.confirmBatchUpdates();
        QTRY_VERIFY_WITH_TIMEOUT(model.remote()->visible() && model.remote()->error().contains("changed",Qt::CaseInsensitive),5000);
        QVERIFY(model.remote()->m_pendingStarts.isEmpty());
        QVERIFY(model.m_updateNeedsAttention);
    }
    void update_all_waits_for_existing_apply_review() {
        struct Environment { QByteArray old=qgetenv("INSTALL_LINK_CASE"); ~Environment(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);} } environment;
        qputenv("INSTALL_LINK_CASE","page-linked-bulk");
        QTemporaryDir dir;LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && model.updateCount()==1 && model.remote()->replacementAvailable(),10000);
        model.m_applyReview={{"token","existing-review"}};
        QVERIFY(model.updateAll());
        QVERIFY(!model.remote()->replacementActive());
        model.m_applyReview.clear();model.advanceUpdateReviews();
        QVERIFY(model.remote()->replacementActive());
    }
    void known_receipt_update_uses_automatic_install() {
        struct Environment { QByteArray old=qgetenv("INSTALL_LINK_CASE"); ~Environment(){if(old.isNull())qunsetenv("INSTALL_LINK_CASE");else qputenv("INSTALL_LINK_CASE",old);} } environment;
        qputenv("INSTALL_LINK_CASE","known-receipt");
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-links"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && model.remote()->replacementAvailable(),10000);
        model.setSelectedId("page-linked");
        auto *remote=model.remote();
        QVERIFY(remote->beginReplacement("page-linked","update"));
        QTRY_VERIFY_WITH_TIMEOUT(!remote->files().isEmpty() && !remote->m_selectionToken.isEmpty(),10000);
        const auto file=remote->files().first().toMap();
        QVERIFY(remote->startReplacement(file.value("fileKey").toMap().value("fileId").toString(),file.value("candidateFingerprint").toString(),"update",false));
        QVERIFY(remote->m_installStages.value(remote->m_jobId)=="download");
        QVERIFY(remote->m_installTargets.contains(remote->m_jobId));
    }
    void confirmed_enabled_replacement_schedules_apply() {
        for(const auto &test : {qMakePair(true,QString("demo_2")),qMakePair(true,QString("demo_1")),qMakePair(false,QString("demo_2"))}) {
            QTemporaryDir dir;LibraryModel model;model.loadDemoData(2);
            model.m_dataDir=dir.filePath("native");model.m_pendingChanges=true;model.m_automaticApply=test.first;
            model.m_replacementApplyNeedsReview=false;
            model.m_marker={{"method",int(BackendClient::Call::RemoteReplaceCommit)},{"modId",test.second}};
            model.m_remoteOutcome={{"status","succeeded"}};
            model.finishRemoteOutcome(true);
            QTRY_VERIFY_WITH_TIMEOUT(model.m_marker.isEmpty(),5000);
            QCOMPARE(model.m_autoApplyTimer.isActive(),test.first && test.second=="demo_2");
        }
    }
    void health_api() {
        LibraryModel model;
        QVERIFY(model.metaObject()->indexOfProperty("health")>=0);
        QVERIFY(model.metaObject()->indexOfProperty("healthChecking")>=0);
        QVERIFY(model.metaObject()->indexOfProperty("fixAutomatically")>=0);
        QVERIFY(model.metaObject()->indexOfMethod("setFixEnabled(QString,bool)")>=0);
        QVERIFY(model.metaObject()->indexOfMethod("setFixAutomatically(bool)")>=0);
        QVERIFY(!model.checkModHealth());
        LibraryRows rows;
        LibraryMod mod;mod.id="m";rows.replace({mod});
        QVERIFY(rows.roleNames().values().contains("modHealthStatus"));
        rows.setHealth({{"mods",QVariantList{QVariantMap{{"modId","m"},{"status","fixable"},{"symptoms",QStringList{"cameraAim"}}}}}});
        QCOMPARE(rows.data(rows.index(0),LibraryRows::ModHealthStatus).toString(),QString("fixable"));
        QCOMPARE(rows.data(rows.index(0),LibraryRows::ModHealthSymptoms).toStringList(),QStringList{"cameraAim"});
        QCOMPARE(rows.details("m").value("health").toMap().value("status").toString(), QString("fixable"));
        QVERIFY(model.metaObject()->indexOfProperty("needsAttentionOnly")>=0);
        model.setNeedsAttentionOnly(true); QVERIFY(model.needsAttentionOnly());
        rows.setHealth({});QVERIFY(!rows.data(rows.index(0),LibraryRows::ModHealthStatus).isValid());
        model.loadDemoData(3);
        auto *source = static_cast<LibraryRows *>(model.sourceModel()); QVERIFY(source);
        const auto modId = source->data(source->index(0), LibraryRows::ModId).toString();
        source->setHealth({{"mods", QVariantList{QVariantMap{{"modId",modId},{"status","partlyFixable"}}}}});
        QCOMPARE(model.count(),1);
        source->setHealth({{"mods", QVariantList{QVariantMap{{"modId",modId},{"status","fixed"}}}}});
        QCOMPARE(model.count(),0);
        model.setNeedsAttentionOnly(false); QCOMPARE(model.count(),3);
    }
    void bisect_real_api() {
        LibraryModel empty;
        QVERIFY(empty.metaObject()->indexOfProperty("bisect")>=0);
        QVERIFY(empty.metaObject()->indexOfProperty("canStartBisect")>=0);
        QVERIFY(!empty.startBisect());QVERIFY(!empty.answerBisect("broken"));QVERIFY(!empty.finishBisect());
        QTemporaryDir dir(QDir::homePath()+"/.cache/sol-bisect-model-XXXXXX");QVERIFY(dir.isValid());
        const auto data=dir.filePath("native");
        QVERIFY(QDir().mkpath(dir.filePath("game/citadel/addons")));
        QProcess importer;importer.start(helper());QVERIFY(importer.waitForStarted());qint64 id=0;
        const auto rpc=[&](const QString &method,QJsonObject params){params.insert("dataDir",data);importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');QVERIFY(importer.waitForBytesWritten());while(!importer.canReadLine())QVERIFY(importer.waitForReadyRead(7000));auto r=QJsonDocument::fromJson(importer.readLine()).object();QVERIFY2(!r.contains("error"),QJsonDocument(r).toJson().constData());return r.value("result").toObject();};
        auto r=rpc("native.importPreview",{{"statePath",fixture()},{"gamePath",dir.path()}});
        rpc("native.importCommit",{{"token",r.value("token")},{"operationId",QString(32,'b')},{"expectedRevision","0"}});importer.closeWriteChannel();QVERIFY(importer.waitForFinished());
        const auto admitted = [&](auto action) { bool accepted=false; QTRY_VERIFY_WITH_TIMEOUT(accepted || (accepted=action()),15000); };
        LibraryModel model;model.loadNative(helper(),data);QTRY_VERIFY_WITH_TIMEOUT(model.canStartBisect(),15000);
        QVERIFY(!model.startBisect({"a","a"}));QVERIFY(model.startBisect({"mod_active"}));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&model.bisect().value("active").toBool(),15000);
        QCOMPARE(model.bisect().value("round").toInt(),1);QVERIFY(!model.canStartBisect());
        auto health=model.launch()->health();health->capture("bisect-baseline","vanilla");
        {   // An unobserved exit no longer sets the crash baseline; seed it from the saved launch.
            QFile saved(QDir(AppPaths::stateRoot()).filePath("launch-health/"+QCryptographicHash::hash(data.toUtf8(),QCryptographicHash::Sha256).toHex()+".json"));
            QVERIFY(saved.open(QIODevice::ReadOnly));auto state=QJsonDocument::fromJson(saved.readAll()).object();saved.close();
            state["good"]=state["launch"];QVERIFY(saved.open(QIODevice::WriteOnly|QIODevice::Truncate));saved.write(QJsonDocument(state).toJson());saved.close();
            health->load(data);
        }
        health->capture("bisect-fixture","modded");health->observe("running");health->observe("exited");health->answer(true);
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&(model.bisect().value("done").toBool()||model.bisect().value("round").toInt()==2),15000);
        QVERIFY(health->report().value("bisectAnswered").toBool());admitted([&]{return health->changeBisectAnswer();});
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&model.bisect().value("round").toInt()==1,15000);
        QVERIFY(!model.answerBisect("invalid"));admitted([&]{return model.answerBisect("unsure");});
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&model.bisect().value("round").toInt()==2,15000);
        admitted([&]{return model.answerBisect("broken");});QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&model.bisect().value("done").toBool(),15000);
        QCOMPARE(model.bisect().value("result").toStringList(),QStringList{"mod_active"});
        admitted([&]{return model.answerBisect("undo");});QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&!model.bisect().value("done").toBool(),15000);
        QCOMPARE(model.bisect().value("round").toInt(),2);
        admitted([&]{return model.answerBisect("broken");});QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&model.bisect().value("done").toBool(),15000);
        admitted([&]{return model.finishBisect(true);});QTRY_VERIFY_WITH_TIMEOUT(!model.busy()&&!model.bisect().value("active").toBool(),15000);
        QVERIFY(!model.modDetails("mod_active").value("enabled").toBool());
    }
    void configs_real_api() {
        QTemporaryDir dir(QDir::homePath()+"/.cache/sol-config-model-XXXXXX");QVERIFY(dir.isValid());
        const auto data=dir.filePath("native"),source=dir.filePath("state.json");
        QVERIFY(QDir().mkpath(dir.filePath("game/citadel/cfg")));
        QFile f(source);QVERIFY(f.open(QIODevice::WriteOnly));f.write(R"({"local-config":{"version":33,"state":{"activeProfileId":"default","profiles":{"default":{"name":"Default","mods":[]}}}}})");f.close();
        QProcess importer;importer.start(helper());QVERIFY(importer.waitForStarted());qint64 id=0;
        const auto rpc=[&](const QString &method,QJsonObject params){params.insert("dataDir",data);importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');QVERIFY(importer.waitForBytesWritten());while(!importer.canReadLine())QVERIFY(importer.waitForReadyRead(7000));auto r=QJsonDocument::fromJson(importer.readLine()).object();QVERIFY2(!r.contains("error"),QJsonDocument(r).toJson().constData());return r.value("result").toObject();};
        auto r=rpc("native.importPreview",{{"statePath",source},{"gamePath",dir.path()}});
        rpc("native.importCommit",{{"token",r.value("token")},{"operationId",QString(32,'c')},{"expectedRevision","0"}});importer.closeWriteChannel();QVERIFY(importer.waitForFinished());
        LibraryModel model;model.loadNative(helper(),data);auto *a=model.autoexec();auto *c=model.crosshair();
        QTRY_VERIFY_WITH_TIMEOUT(a->available()&&!a->busy()&&!c->values().isEmpty()&&!c->busy(),15000);
        QVERIFY2(a->error().isEmpty(),qPrintable(a->error()));QCOMPARE(c->exportCode(),QString("0;4;16;1;false;false;1;1;255;255;255;Default"));
        QVERIFY(a->preview("echo model"));QTRY_VERIFY_WITH_TIMEOUT(!a->review().isEmpty()&&!a->busy(),7000);QCOMPARE(a->review().value("after").toString(),QString("echo model"));QVERIFY(a->commit(a->review().value("token").toString()));QTRY_VERIFY_WITH_TIMEOUT(a->canUndo()&&!a->busy(),7000);
        QVERIFY(a->undo());QTRY_VERIFY_WITH_TIMEOUT(!a->review().isEmpty()&&!a->busy(),7000);QVERIFY(a->commit(a->review().value("token").toString()));QTRY_VERIFY_WITH_TIMEOUT(!a->canUndo()&&!a->busy(),7000);QVERIFY(!QFile::exists(dir.filePath("game/citadel/cfg/autoexec.cfg")));
        QVERIFY(!c->setValue("gap",51));QVERIFY(c->setValue("color.r",22));QVERIFY(c->preview());QTRY_VERIFY_WITH_TIMEOUT(!c->review().isEmpty()&&!c->busy(),7000);QVERIFY(c->review().value("additionalDiff").toMap().contains("path"));
        QVERIFY(QDir().rmdir(dir.filePath("game/citadel/cfg")));a->refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!a->busy()&&!a->error().isEmpty(),7000);QVERIFY(model.error().isEmpty());
    }
    void configs_api() {
        LibraryModel model;
        for(auto *c:{model.autoexec(),model.crosshair()}){
            QVERIFY(c);for(const auto *key:{"text","path","managedSections","values","defaults","ranges","review","available","busy","error","canUndo"})QVERIFY(c->metaObject()->indexOfProperty(key)>=0);
            QVERIFY(!c->available());QVERIFY(!c->commit("fake"));QVERIFY(!c->undo());QVERIFY(!c->preview());QVERIFY(!c->importCode("invalid"));
        }
    }
    void performance_api() {
        auto review=QJsonDocument::fromJson(R"({"summaryVersion":2,"deploymentLayout":"shards-v1","token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","profileId":"default","gamePath":"/synthetic/game","packageCount":0,"directoryCount":0,"physicalFileCount":0,"companionFileCount":0,"fileCount":0,"totalBytes":0,"shardCount":1,"shards":[{"index":1,"directoryCount":0}],"replacedMounts":[],"warnings":[]})").object();
        QVariantMap decoded;QVERIFY(OptionData::decodeApplyReview(review,decoded));
        const QJsonObject perf{{"presetId","dmm-clean"},{"presetName","DMM Clean"},{"entryCount",96},{"omittedCount",0},{"detected",QJsonObject{{"state","stock"}}},{"overlayAction","replace"},{"foreignPolicy","preserve-and-refuse"},{"restorePolicy","exact-prior-bytes"}};
        review.insert("performance",perf);QVERIFY(OptionData::decodeApplyReview(review,decoded));QCOMPARE(decoded.value("performance").toMap().value("presetId").toString(),QString("dmm-clean"));
        auto preserved=perf;preserved.insert("overlayAction","preserve");preserved.insert("changes",QJsonArray{});review.insert("performance",preserved);QVERIFY(OptionData::decodeApplyReview(review,decoded));
        auto changed=perf;changed.insert("changes",QJsonArray{QJsonObject{{"name","IndexBufferPoolSizeMB"},{"from","32"},{"to","16"}}});review.insert("performance",changed);QVERIFY(OptionData::decodeApplyReview(review,decoded));
        changed.insert("changes",QJsonArray{QJsonObject{{"name","IndexBufferPoolSizeMB"},{"from",32},{"to","16"}}});review.insert("performance",changed);QVERIFY(!OptionData::decodeApplyReview(review,decoded));
        auto bad=perf;bad.insert("entryCount","bad");review.insert("performance",bad);QVERIFY(!OptionData::decodeApplyReview(review,decoded));
        // A built repair package is one extra physical file beyond the mod folders.
        auto repaired=QJsonDocument::fromJson(R"({"summaryVersion":2,"deploymentLayout":"shards-v1","token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","profileId":"default","gamePath":"/synthetic/game","packageCount":1,"directoryCount":1,"physicalFileCount":2,"companionFileCount":0,"fileCount":1,"totalBytes":30,"shardCount":1,"shards":[{"index":1,"directoryCount":1}],"replacedMounts":[],"warnings":[],"repairs":{"file":{"id":"x","size":10}},"repairModCount":1,"repairSymptoms":["animations"],"upstreamRepairMigration":null})").object();
        QVERIFY(OptionData::decodeApplyReview(repaired,decoded));QCOMPARE(decoded.value("repairModCount").toInt(),1);
        auto noFile=repaired;noFile.insert("repairs",QJsonObject{{"file",QJsonValue::Null}});QVERIFY(!OptionData::decodeApplyReview(noFile,decoded));
        auto extra=repaired;extra.insert("physicalFileCount",3);QVERIFY(!OptionData::decodeApplyReview(extra,decoded));
        auto launch=repaired;for(const auto *k:{"token"})launch.remove(k);
        QVERIFY(LaunchData::reviewSummary(launch,true,{}));
        auto launchNoFile=launch;launchNoFile.insert("repairs",QJsonValue::Null);QVERIFY(!LaunchData::reviewSummary(launchNoFile,true,{}));
        auto launchPlain=launchNoFile;launchPlain.insert("physicalFileCount",1);QVERIFY(LaunchData::reviewSummary(launchPlain,true,{}));
        LibraryModel model;auto *p=model.performance();QVERIFY(p);
        for(const auto *name:{"presets","selection","detected","available","busy","error"})QVERIFY(p->metaObject()->indexOfProperty(name)>=0);
        QVERIFY(!p->available());QVERIFY(!p->select("dmm-clean"));QVERIFY(!p->select(""));QVERIFY(p->preview("unknown").isEmpty());
    }
    void performance_real_api() {
        QTemporaryDir dir(QDir::homePath()+"/.cache/dlmm-modelcheck-XXXXXX");QVERIFY(dir.isValid());const auto data=dir.filePath("native");
        QProcess initializer;initializer.start(helper());QVERIFY(initializer.waitForStarted());
        initializer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},{"params",QJsonObject{{"version",1},{"dataDir",data},{"operationId",QString(32,'a')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
        QVERIFY(initializer.waitForBytesWritten());while(!initializer.canReadLine())QVERIFY(initializer.waitForReadyRead(5000));
        const auto response=QJsonDocument::fromJson(initializer.readLine()).object();QVERIFY2(!response.contains("error"),QJsonDocument(response).toJson().constData());initializer.closeWriteChannel();QVERIFY(initializer.waitForFinished());
        LibraryModel model;model.loadNative(helper(),data);auto *p=model.performance();
        QTRY_VERIFY_WITH_TIMEOUT(p->presets().size()==8,15000);QTRY_VERIFY_WITH_TIMEOUT(!p->busy(),10000);
        QCOMPARE(p->preview("dmm-clean").size(),96);QVERIFY(p->select("dmm-clean"));
        QTRY_VERIFY_WITH_TIMEOUT(p->selection()=="dmm-clean"&&!p->busy(),15000);QVERIFY(model.pendingChanges());
        QVERIFY(p->select(""));QTRY_VERIFY_WITH_TIMEOUT(p->selection().isEmpty()&&!p->busy(),15000);
    }
    void installed_api() {
        LibraryModel model;
        QVERIFY(model.detectedGamePath().isEmpty());QVERIFY(model.detectedGamePaths().isEmpty());
        QVERIFY(model.installedMods().isEmpty());QVERIFY(!model.installedModsBusy());
        QVERIFY(!model.canScanInstalledMods());QVERIFY(!model.scanInstalledMods());
        QVERIFY(!model.importInstalledMods({},{}));
        QVERIFY(!model.importInstalledMods({"a","b"},{"a"}));
        auto *group=model.localImport();
        for(const auto *name:{"groupRows","groupResult","progress"})QVERIFY(group->metaObject()->indexOfProperty(name)>=0);
        QVERIFY(group->metaObject()->indexOfMethod("setGroupLink(QString,QString)")>=0);
        QVERIFY(group->metaObject()->indexOfMethod("dropGroupRow(QString)")>=0);
        QCOMPARE(group->progress().value("filesDone").toInt(),0);
        QVERIFY(!model.importInstalledMods({"missing"},{"different"}));
        QVERIFY(!model.importInstalledMods({"missing"},{}));
        QVERIFY(model.metaObject()->indexOfMethod("importInstalledMods(QStringList,QStringList)")>=0);
        QVERIFY(!model.importInstalledMod("missing"));QVERIFY(!model.identifyInstalledMod("missing"));
        for(const auto *name:{"detectedGamePath","detectedGamePaths","installedMods","installedModsBusy","installedModsError","canScanInstalledMods"})QVERIFY(model.metaObject()->indexOfProperty(name)>=0);
    }
    void installed_group_real_atomic_api() {
        QTemporaryDir dir(QDir::homePath()+"/.cache/sol-group-model-XXXXXX");QVERIFY(dir.isValid());
        const auto data=dir.filePath("native");
        QProcess initializer;initializer.start(helper());QVERIFY(initializer.waitForStarted());
        initializer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},{"params",QJsonObject{{"version",1},{"dataDir",data},{"operationId",QString(32, 'a')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
        QVERIFY(initializer.waitForBytesWritten());while(!initializer.canReadLine())QVERIFY(initializer.waitForReadyRead(5000));
        const auto response=QJsonDocument::fromJson(initializer.readLine()).object();QVERIFY2(!response.contains("error"),QJsonDocument(response).toJson().constData());initializer.closeWriteChannel();QVERIFY(initializer.waitForFinished());
        QVariantList rows;
        for(int i=0;i<2;++i){
            QByteArray tree("vmdl_c\0models\0",14);tree.append(i?'b':'a');tree.append('\0');
            tree.append(QByteArray::fromHex("010000000000ff7f0000000000000000ffff000000"));
            QByteArray bytes=QByteArray::fromHex("3412aa5501000000");quint32 n=tree.size();for(int j=0;j<4;++j)bytes.append(char((n>>(8*j))&255));bytes.append(tree);
            const auto path=dir.filePath(QString("%1.vpk").arg(i));QFile file(path);QVERIFY(file.open(QIODevice::WriteOnly));file.write(bytes);file.close();
            rows.append(QVariantMap{{"id",QString::number(i)},{"path",path},{"packages",QStringList{path}},{"error",QVariant()},{"suggestedMatch",QVariantMap{{"name",QString("Group %1").arg(i)},{"author","Creator"},{"category","Skins"},{"description","Each row"},{"link","https://gamebanana.com/mods/123"}}}});
        }
        LibraryModel model;model.loadNative(helper(),data);QTRY_VERIFY_WITH_TIMEOUT(model.localImport()->canPrepare(),10000);
        model.m_installedScanGame=model.m_gamePath;model.m_installedMods=rows;
        QVERIFY(model.importInstalledMods({"0","1"},{"1"}));auto *local=model.localImport();
        QTRY_VERIFY_WITH_TIMEOUT(local->canCommit() || !local->error().isEmpty(),10000);QVERIFY2(local->canCommit(),qPrintable(local->error()));
        QCOMPARE(local->review().value("groupRows").toList().size(),2);
        QVERIFY(local->setGroupLink("1",""));QVERIFY(!local->canCommit());QVERIFY(local->reviewCurrentDraft());QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),5000);
        QVERIFY(local->dropGroupRow("0"));QVERIFY(local->reviewCurrentDraft());QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),5000);
        QVERIFY(local->commit());QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="succeeded" || !local->error().isEmpty(),10000);QVERIFY2(local->phase()=="succeeded",qPrintable(local->error()));
        QCOMPARE(local->groupResult().value("added").toList().size(),1);QCOMPARE(local->groupResult().value("skipped").toList().size(),1);
        QCOMPARE(model.totalCount(),1);QVERIFY(model.m_rows->enabledCount()==0);
    }
    void installed_grouped_preparation_and_metadata() {
        QTemporaryDir dir;LibraryModel model;
        model.loadNative(peer(dir,"native-local"),dir.filePath("native"));
        auto *local=model.localImport();QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        model.m_installedScanGame=model.m_gamePath;
        const QVariantMap match{{"modId","123"},{"name","Upstream title"},{"author","Creator"},{"link","https://gamebanana.com/mods/123"},{"requiresConfirmation",true}};
        model.m_installedMods={QVariantMap{{"id","group"},{"path","/synthetic/a.vpk"},{"packages",QStringList{"/synthetic/a.vpk","/synthetic/b.vpk"}},{"error",QVariant()},{"suggestedMatch",match}}};
        QVERIFY(model.importInstalledMods({"group"},{}));
        QTRY_VERIFY_WITH_TIMEOUT(local->presentationReady(),3000);
        QCOMPARE(local->metadataDraft().value("name").toString(),"Upstream title");
        QCOMPARE(local->metadataDraft().value("author").toString(),"Creator");
        QVERIFY(local->metadataDraft().value("link").toString().isEmpty());
        QFile trace(dir.filePath("native/trace.jsonl"));QVERIFY(trace.open(QIODevice::ReadOnly));
        bool found=false;
        for(const auto &line:trace.readAll().split('\n')) {
            const auto request=QJsonDocument::fromJson(line).object();
            if(request["method"]==QJsonValue("native.local.prepare")) {
                QCOMPARE(request["params"].toObject()["roots"].toArray(),(QJsonArray{"/synthetic/a.vpk","/synthetic/b.vpk"}));found=true;
            }
        }
        QVERIFY(found);
        CatalogDecodedResult result;result.phase="succeeded";
        result.detail={{"summary",QVariantMap{{"title","Catalog title"},{"author","Remote author"},{"thumbnail","https://images.gamebanana.com/test.jpg"},{"masked",false}}}};
        auto deliver=[&](quint64 epoch) {
            model.m_installedMetadataKey="installed-test";model.m_installedMetadataId="123";model.m_installedMetadataEpoch=epoch;
            emit model.catalog()->libraryMediaReady("installed-test",model.dataDir(),result);
        };
        deliver(model.m_loadEpoch-1);
        QCOMPARE(model.installedMods().first().toMap().value("suggestedMatch").toMap().value("name").toString(),"Upstream title");
        deliver(model.m_loadEpoch);
        const auto updated=model.installedMods().first().toMap().value("suggestedMatch").toMap();
        QCOMPARE(updated.value("name").toString(),"Catalog title");QVERIFY(updated.value("requiresConfirmation").toBool());
        QCOMPARE(updated.value("link"),match.value("link"));
        QVERIFY(model.m_installedMetadataCache.contains("123"));
    }
    void delete_mods_invokable_and_exact_context() {
        QTemporaryDir dir; LibraryModel model;
        model.setAutomaticApply(false);
        model.loadNative(peer(dir,"native-local-delete"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.canBulkOperate(),3000);
        bool accepted=false;
        QVERIFY(QMetaObject::invokeMethod(&model,"deleteMods",Q_RETURN_ARG(bool,accepted),Q_ARG(QStringList,QStringList({"image-mod"}))));
        QVERIFY(accepted);
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && model.rowCount()==0,5000);
        QVERIFY(!model.outcomeUnknown());
        QCOMPARE(model.revision(),QString("2"));
        QCOMPARE(model.lastBulkResult().value("changed").toInt(),1);
        QFile trace(dir.filePath("native/trace.jsonl"));QVERIFY(trace.open(QIODevice::ReadOnly));
        bool found=false;
        for(const auto &line:trace.readAll().split('\n')) {
            const auto request=QJsonDocument::fromJson(line).object();
            if(request["method"]==QJsonValue("native.deleteMods")) {
                QCOMPARE(request["params"].toObject()["modIds"].toArray(),QJsonArray{"image-mod"});
                QCOMPARE(request["params"].toObject()["profileId"].toString(),QString("default"));found=true;
            }
        }
        QVERIFY(found);
    }
    void bulk_invokables_and_summary(bool unknown=false) {
        for(bool remove : {false,true}) {
            QTemporaryDir dir; LibraryModel model;
            QVERIFY(!model.canBulkOperate());QVERIFY(!model.setModsEnabled({"image-mod"},false));
            model.loadNative(peer(dir,unknown ? "native-local-batch-unknown" : "native-local-batch"),dir.filePath("native"));
            QTRY_VERIFY_WITH_TIMEOUT(model.canBulkOperate(),3000);
            QVERIFY(model.lastBulkResult().isEmpty());
            QVERIFY(!model.setModsEnabled({},false));QVERIFY(!model.setModsEnabled({"../bad"},false));
            QVERIFY(!model.removeModsFromProfile({"image-mod","image-mod"}));
            QVERIFY(!model.setModsEnabled(QStringList(257,"image-mod"),false));
            bool accepted=false;
            if(remove) QVERIFY(QMetaObject::invokeMethod(&model,"removeModsFromProfile",Q_RETURN_ARG(bool,accepted),Q_ARG(QStringList,QStringList({"image-mod","missing"}))));
            else QVERIFY(QMetaObject::invokeMethod(&model,"setModsEnabled",Q_RETURN_ARG(bool,accepted),Q_ARG(QStringList,QStringList({"image-mod","missing"})),Q_ARG(bool,false)));
            QVERIFY(accepted);QVERIFY(model.busy());QVERIFY(!model.canBulkOperate());
            QVERIFY(!model.removeModsFromProfile({"image-mod"}));
            QTRY_VERIFY_WITH_TIMEOUT(!model.busy(),4000);
            if(unknown) {
                // The summary travelled in the lost reply. The rows are the helper's state.
                QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.busy() && model.revision()=="2",5000);
                QVERIFY(!model.outcomeUnknown());QVERIFY(model.lastBulkResult().isEmpty());
                QCOMPARE(model.warnings().filter("connection was lost").size(),1);
            } else {
            QCOMPARE(model.lastBulkResult().value("changed").toInt(),1);
            QCOMPARE(model.lastBulkResult().value("skipped").toList().first().toMap().value("reason").toString(),"unknownMod");
            }
            QCOMPARE(model.rowCount(),remove ? 0 : 1);
        }
    }
    void legacy_link_keeps_explicit_target_and_marker() {
        const QString token(32, 'a'), hash(64, 'b');
        QJsonObject request{{"version",1},{"dataDir","/tmp/link-model-check"},{"sessionId",token},{"jobId",token},
            {"selectionToken",token},{"candidateFingerprint",hash},{"fileId","123"},{"profileId","default"},
            {"modId","imported-row"},{"expectedRevision","1"},{"targetDigest",hash},{"purpose","link"},{"acceptArchived",false}};
        QVERIFY(RemoteData::request(BackendClient::Call::RemoteReplaceStart,request));
        request["purpose"]="append";QVERIFY(RemoteData::request(BackendClient::Call::RemoteReplaceStart,request));
        request["purpose"]="link";
        request["modId"]="../unsafe";QVERIFY(!RemoteData::request(BackendClient::Call::RemoteReplaceStart,request));
        request["modId"]="imported-row";request["purpose"]="guess";QVERIFY(!RemoteData::request(BackendClient::Call::RemoteReplaceStart,request));
        QJsonObject marker{{"remoteReplacementMarkerVersion",1},{"method",int(BackendClient::Call::RemoteReplaceCommit)},
            {"root","/tmp/link-model-check"},{"jobId",token},{"purpose","link"},{"profileId","default"},{"modId","imported-row"},
            {"initialRevision","1"},{"expectedRevision","1"},{"targetDigest",hash},{"afterRecordDigest",hash},
            {"operationId","link-operation"},{"consentFingerprint",hash},{"cohortFingerprint",hash},{"selectionDigest",hash}};
        QVERIFY(RemoteData::replacementMarker(marker,"/tmp/link-model-check"));
        auto appendedMarker=marker;appendedMarker["purpose"]="append";
        QVERIFY(RemoteData::replacementMarker(appendedMarker,"/tmp/link-model-check"));
        QCOMPARE(RemoteData::replacementInput(appendedMarker)["params"].toObject()["purpose"],QJsonValue("append"));
        const auto input=RemoteData::replacementInput(marker)["params"].toObject();
        QCOMPARE(input["purpose"],QJsonValue("link"));QCOMPARE(input["modId"],QJsonValue("imported-row"));
        QJsonObject observation{{"version",1},{"dataDir","/tmp/link-model-check"},{"sessionId",token},{"requestKey",token},
            {"profileId","default"},{"modId","imported-row"},{"expectedRevision","1"}};
        QJsonObject target{{"profileId","default"},{"modId","imported-row"},{"revision","1"},{"recordDigest",hash},
            {"receipt",QJsonValue::Null},{"enabled",true},{"previewHidden",true},{"available",true},{"installOrder",7},
            {"metadata",QJsonObject{{"name","Kept"},{"author",""},{"category",""},{"description",""}}},
            {"optionCount","1"},{"selectedOptionIds",QJsonArray{token}}};
        QJsonObject response{{"version",1},{"root","/tmp/link-model-check"},{"sessionId",token},{"requestKey",token},
            {"phase","ready"},{"target",target},{"skip",QJsonObject{{"state","none"},{"entry",QJsonValue::Null}}},{"error",QJsonValue::Null}};
        RemoteReply reply;QVERIFY(RemoteData::decode(BackendClient::Call::RemoteReplaceTarget,response,observation,reply));
        target["selectedOptionIds"]=QJsonArray{};response["target"]=target;
        QVERIFY(RemoteData::decode(BackendClient::Call::RemoteReplaceTarget,response,observation,reply));
        target["receipt"]=QJsonObject{{"unknown",true}};response["target"]=target;
        QVERIFY(!RemoteData::decode(BackendClient::Call::RemoteReplaceTarget,response,observation,reply));
        LibraryMod row;row.linkEligible=true;row.linkReason="";QVERIFY(row.details()["linkEligible"].toBool());
        LibraryModel model;QVERIFY(!model.remote()->beginReplacement("imported-row","link"));
    }
    void local_capability_arrival_enables_real_control() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir, "native-local"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 3000);
        QVERIFY(model.localImport()); model.localImport()->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(model.localImport()->canPrepare(),3000);
        QVERIFY(model.localImport()->prepare({QUrl("file:///synthetic/a%20space.vpk")}));
        QTRY_VERIFY_WITH_TIMEOUT(!model.localImport()->review().isEmpty(),3000);
        QCOMPARE(model.localImport()->metadataDraft().value("name").toString(),"Local family");
        QCOMPARE(model.localImport()->selectionDraft(),QStringList{QString(64,'a')});
    }
    void local_prepare_keeps_navigation_and_destination() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-local-crossing"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(!local->prepare({QUrl("https://example.invalid/mod.vpk")}));
        QVERIFY(!local->prepare({QUrl("/synthetic/no-scheme.vpk")}));
        QVERIFY(local->prepare({QUrl("file:///synthetic/a%20space.vpk")}));
        model.viewProfile("fast"); model.setQuery("still responsive");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !local->review().isEmpty(),4000);
        QCOMPARE(model.viewedProfileId(),"fast"); QCOMPARE(local->destinationName(),"default");
        QCOMPARE(local->review().value("exclusions").toList().size(),2);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft()));
        auto dirty=local->metadataDraft(); dirty.insert("name","A newer typed name"); local->setMetadataDraft(dirty);
        local->setVisible(false); local->setVisible(true);
        waitEvents(600); QVERIFY(!local->canCommit()); QCOMPARE(local->metadataDraft(),dirty);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft()));
        QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),3000);
        auto selection=local->selectionDraft(); local->setSelectionDraft({}); QVERIFY(!local->canCommit()); local->setSelectionDraft(selection);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft()));
        QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),3000);
        QVERIFY(local->commit()); QVERIFY(!local->cancel()); QVERIFY(!local->commit());
        QVERIFY(!model.setModEnabled("image-mod",false));
        QTRY_VERIFY_WITH_TIMEOUT(!model.writeBusy() && local->phase()=="succeeded",4000);
        QCOMPARE(local->phase(),"succeeded"); QCOMPARE(model.viewedProfileId(),"fast"); QCOMPARE(model.revision(),"2");
        QFile trace(dir.filePath("native/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly)); const auto lines=trace.readAll();
        QVERIFY(lines.contains("/synthetic/a space.vpk")); QVERIFY(!lines.contains("a%20space.vpk"));
        int commits=0; for(const auto &line:lines.split('\n')) { auto r=QJsonDocument::fromJson(line).object(); if(r["method"]==QJsonValue("native.local.commit")) { ++commits; QCOMPARE(r["params"].toObject()["profileId"].toString(),"default"); } }
        QCOMPARE(commits,1);
    }
    void local_cancel_uses_control_rpc_without_helper_kill() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local-held"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")}));
        model.viewProfile("slow"); waitEvents(50); QVERIFY(model.loading());
        QVERIFY(local->cancel()); QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled",500);
        QVERIFY(model.loading()); QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),2000);
        QCOMPARE(model.viewedProfileId(),"slow");
        QVERIFY(local->dismiss(local->jobId())); QTRY_VERIFY_WITH_TIMEOUT(local->jobId().isEmpty(),1000);
    }
    void state_safety() {
        QTemporaryDir fixture; LibraryModel model;
        const auto startupError=AppPaths::stateWriteError();
        if(!qEnvironmentVariableIsEmpty("SAFETY_FIX_LINK")) {
            const auto folder=QDir(AppPaths::configHome()).filePath("dl-mod-manager");
            QVERIFY(QFile::remove(folder));QVERIFY(QDir().mkdir(folder));
        }
        QCOMPARE(AppPaths::stateWriteError(),startupError);
        model.loadNative(peer(fixture,"native-local-held"),qEnvironmentVariable("SAFETY_LIBRARY"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading(),7000);
        QVERIFY(model.rowCount()>0);
        QVERIFY(model.launch()->health()->enabled()); // legacy launch history is never loaded
        const auto expected=qEnvironmentVariable("SAFETY_ERROR");
        if(!expected.isEmpty()) {
            QCOMPARE(model.actionUnavailableReason(),expected);
            QVERIFY(!model.canSetEnabled());QVERIFY(!model.setModEnabled("image-mod",false));
            QVERIFY(!model.launch()->saveSettings("-novid"));
            model.launch()->health()->setEnabled(false);
            QCOMPARE(model.launch()->health()->error(),expected);
            QVERIFY(!model.canRecover());QVERIFY(!model.recover());
            QCOMPARE(model.error(),expected);
            QVERIFY(!model.createLibrary());
            QCOMPARE(model.actionUnavailableReason(),expected);
            QCOMPARE(model.launch()->settingsError(),expected);
            StorageController storage;storage.setLibrary(&model);
            QStringList refusals;
            for(const auto method : {"move","resume","cancel"}) {
                QVERIFY(QMetaObject::invokeMethod(&storage,method));
                if(storage.error()!=expected || storage.busy())refusals.append(QString::fromUtf8(method));
                QTRY_VERIFY_WITH_TIMEOUT(!storage.busy(),3000);
            }
            QVERIFY2(refusals.isEmpty(),qPrintable("Missing storage refusals: "+refusals.join(", ")));
            QCOMPARE(model.bootstrapReason(),expected);
        } else {
            // Edits have no marker; a library delete is a transaction that writes one.
            QTRY_VERIFY_WITH_TIMEOUT(model.canDeleteMods(),4000);
            QVERIFY(model.deleteMods({"image-mod"}));
            const auto hash=QCryptographicHash::hash(model.dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
            const auto file=QDir(qEnvironmentVariable("XDG_CONFIG_HOME")).filePath("dl-mod-manager/state/pending/"+hash+".json");
            QTRY_VERIFY_WITH_TIMEOUT(QFileInfo::exists(file),3000);
            QTRY_VERIFY_WITH_TIMEOUT(!model.writeBusy(),7000);
        }
    }
    void local_main_writer_allows_control_without_marker() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local-held"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); waitEvents(100);
        QVERIFY(model.setModEnabled("image-mod",false)); QTRY_VERIFY_WITH_TIMEOUT(model.writeBusy(),1000);
        QVERIFY(!local->canPrepare()); QVERIFY(!local->reviewSelection({},{})); QVERIFY(!local->commit());
        QVERIFY(local->cancel()); QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled",1000);
        QVERIFY(model.writeBusy()); QVERIFY(!local->dismiss(local->jobId()));
        const auto hash=QCryptographicHash::hash(model.dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
        QFile marker(QDir(AppPaths::stateRoot()).filePath("pending/"+hash+".json"));
        // The main writer is an edit: it has no marker for local control to disturb.
        waitEvents(200); QVERIFY(model.writeBusy()); QVERIFY(!marker.exists());
        // Pending-operation state on a fresh install uses the neutral root.
        QCOMPARE(AppPaths::stateRoot(), QDir(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation)).filePath("dl-mod-manager/state"));
        QDirIterator paths(QString::fromUtf8(qgetenv("HOME")), QDir::AllEntries | QDir::NoDotAndDotDot | QDir::Hidden, QDirIterator::Subdirectories);
        while (paths.hasNext()) QVERIFY(!QDir(QString::fromUtf8(qgetenv("HOME"))).relativeFilePath(paths.next()).contains("lumi"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.writeBusy(),7000);
    }
    void local_unknown_disconnect_reconciles_same_operation() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local-unknown-fresh"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); QTRY_VERIFY_WITH_TIMEOUT(!local->review().isEmpty(),3000);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft())); QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),3000);
        model.viewProfile("fast"); QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),3000);
        QVERIFY(local->commit()); QTRY_VERIFY_WITH_TIMEOUT(model.outcomeUnknown(),3000); QVERIFY(!local->cancel()); QVERIFY(!local->canDismiss()); QVERIFY(!local->dismiss(local->jobId()));
        QVERIFY(model.reconcileOperation()); QTRY_VERIFY_WITH_TIMEOUT(model.loading(),2000);
        QVERIFY(model.outcomeUnknown() && model.busy());
        const auto hash=QCryptographicHash::hash(model.dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
        QFile marker(QDir(AppPaths::stateRoot()).filePath("pending/"+hash+".json")); QVERIFY(marker.open(QIODevice::ReadOnly)); QVERIFY(!QJsonDocument::fromJson(marker.readAll()).object().isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(!model.outcomeUnknown() && !model.writeBusy() && local->phase()=="succeeded",3000);
        QCOMPARE(model.viewedProfileId(),"fast"); QCOMPARE(model.revision(),"2"); QCOMPARE(local->phase(),"succeeded");
        QFile trace(dir.filePath("native/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly)); const auto lines=trace.readAll();
        int commits=0; QString operation;
        for(const auto &line:lines.split('\n')) { auto r=QJsonDocument::fromJson(line).object(); if(r["method"]==QJsonValue("native.local.commit")) { ++commits; operation=r["params"].toObject()["operationId"].toString(); } if(r["method"]==QJsonValue("native.operation")) QCOMPARE(r["params"].toObject()["operationId"].toString(),operation); }
        QCOMPARE(commits,1);
    }
    void local_malformed_oversize_dto_is_rejected() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); QTRY_VERIFY_WITH_TIMEOUT(!local->review().isEmpty(),3000);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft())); QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),3000);
        auto good=QJsonObject::fromVariantMap(local->review()); QVariantMap output;
        QVERIFY(LocalImportController::decodeReply(good,output));
        QVERIFY(LocalImportController::decodeReply(good,output,true));
        for(const auto *key:{"inactiveOptionCount","retainedPackageCount","retainedLogicalPackageBytes","selectedLogicalPackageBytes","uniquePayloadBytes","retainedArchiveBytes","archives"}) {
            auto bad=good; bad.remove(key); QVERIFY2(!LocalImportController::decodeReply(bad,output,true),key);
        }

        for(const auto *key:{"jobId","token","modId","profileId","metadata","sources","candidates","selectedCandidateIds","selectedPackageCount","storedPackageCount","exclusions","uniqueCopyBytes","logicalPackageBytes","warnings","sourceArtwork"}) {
            auto bad=good; bad.remove(key); QVERIFY2(!LocalImportController::decodeReply(bad,output),key);
        }
        auto bad=good; bad.insert("storedPackageCount",2); QVERIFY(!LocalImportController::decodeReply(bad,output));
        bad=good; auto metadata=bad["metadata"].toObject(); metadata.insert("name",QString(257,'x')); bad.insert("metadata",metadata); QVERIFY(!LocalImportController::decodeReply(bad,output));
        bad=good; bad.insert("expectedRevision","01"); QVERIFY(!LocalImportController::decodeReply(bad,output));
        bad=good; auto candidates=good["candidates"].toArray(); auto row=candidates[0].toObject(); row.insert("available","yes"); candidates[0]=row; bad.insert("candidates",candidates); QVERIFY(!LocalImportController::decodeReply(bad,output));
        bad=good; QJsonArray tooMany; for(int i=0;i<1025;++i) tooMany.append(good["candidates"].toArray()[0]); bad.insert("candidates",tooMany); QVERIFY(!LocalImportController::decodeReply(bad,output));
    }
    void local_artwork_bounds_fingerprint_and_queue() {
        QTemporaryDir dir; LocalArtworkProvider provider;
        auto descriptor=[&](const QString &name) { QFile file(name); QVERIFY(file.open(QIODevice::ReadOnly)); auto bytes=file.readAll(); struct stat st{}; QVERIFY(::stat(QFile::encodeName(name).constData(),&st)==0); return QJsonObject{{"path",name},{"format",name.endsWith("jpg")?"jpeg":"png"},{"sha256",QString::fromLatin1(QCryptographicHash::hash(bytes,QCryptographicHash::Sha256).toHex())},{"size",qint64(bytes.size())},{"device",qint64(st.st_dev)},{"inode",qint64(st.st_ino)},{"modifiedSeconds",qint64(st.st_mtim.tv_sec)},{"modifiedNanos",qint64(st.st_mtim.tv_nsec)}}; };
        auto check=[&](const QString &url,bool success) { auto *response=provider.requestImageResponse(url.sliced(QString("image://localartwork/").size()),QSize(80,80)); bool done=false; connect(response,&QQuickImageResponse::finished,this,[&] {done=true;}); QTRY_VERIFY_WITH_TIMEOUT(done,3000); QCOMPARE(response->errorString().isEmpty(),success); delete response; };
        for(const auto *extension:{"png","jpg"}) { QImage image(32,24,QImage::Format_RGB32); image.fill(Qt::red); auto path=dir.filePath(QString("image.")+extension); QVERIFY(image.save(path)); auto url=LocalArtworkProvider::admitSource(descriptor(path)); QVERIFY(!url.isEmpty()); check(url,true); QFile file(path); QVERIFY(file.open(QIODevice::WriteOnly)); file.write("corrupt"); file.close(); check(url,false); }
        QImage huge(8193,1,QImage::Format_RGB32); huge.fill(Qt::blue); auto hugePath=dir.filePath("huge.png"); QVERIFY(huge.save(hugePath)); check(LocalArtworkProvider::admitSource(descriptor(hugePath)),false);
        auto path=dir.filePath("link.png"); QImage image(16,16,QImage::Format_RGB32); image.fill(Qt::green); QVERIFY(image.save(path)); auto admitted=LocalArtworkProvider::admitSource(descriptor(path)); QVERIFY(QFile::rename(path,dir.filePath("target.png"))); QVERIFY(QFile::link(dir.filePath("target.png"),path)); check(admitted,false);
        check("image://localartwork/unknown",false);
        QList<QQuickImageResponse*> responses; int finished=0;
        for(int i=0;i<70;++i) { auto *r=provider.requestImageResponse("unknown",QSize(80,80)); connect(r,&QQuickImageResponse::finished,this,[&] {++finished;}); responses.append(r); }
        QTRY_VERIFY_WITH_TIMEOUT(finished==70,3000); for(auto *r:responses) { QVERIFY(!r->errorString().isEmpty()); delete r; }
    }
    void local_marker_persistence_failure_dispatches_no_commit() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); QTRY_VERIFY_WITH_TIMEOUT(!local->review().isEmpty(),3000);
        QVERIFY(local->reviewSelection(local->selectionDraft(),local->metadataDraft())); QTRY_VERIFY_WITH_TIMEOUT(local->canCommit(),3000);
        const auto hash=QCryptographicHash::hash(model.dataDir().toUtf8(),QCryptographicHash::Sha256).toHex();
        const auto marker=QDir(AppPaths::stateRoot()).filePath("pending/"+hash+".json");
        QVERIFY(QDir().mkpath(marker)); QVERIFY(local->commit());
        QTRY_VERIFY_WITH_TIMEOUT(!model.writeBusy() && local->error().contains("persist"),2000);
        QFile trace(dir.filePath("native/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly)); QVERIFY(!trace.readAll().contains("native.local.commit"));
        QVERIFY(QDir().rmdir(marker));
    }
    void local_default_root_and_graceful_restart() {
        QTemporaryDir dir; const auto old=qgetenv("XDG_DATA_HOME"); qputenv("XDG_DATA_HOME",dir.path().toUtf8());
        struct Restore { QByteArray value; ~Restore() { if(value.isNull()) qunsetenv("XDG_DATA_HOME"); else qputenv("XDG_DATA_HOME",value); } } restore{old};
        LibraryModel model; model.loadNative(peer(dir,"native-local-slow"),{}); auto *local=model.localImport(); local->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000); QCOMPARE(model.dataDir(),dir.filePath("modlock"));
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); waitEvents(100);
        auto *client=local->findChild<BackendClient*>(); QVERIFY(client);
        auto *process=client->findChild<QProcess*>(); QVERIFY(process); auto pid=process->processId(); auto id=local->jobId();
        client->stop(); QVERIFY(client->retiring());
        QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="interrupted",3000); QCOMPARE(local->jobId(),id);
        QTRY_VERIFY_WITH_TIMEOUT(::kill(pid,0)==-1 && errno==ESRCH,1000);
        QFile trace(dir.filePath("modlock/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly)); auto lines=trace.readAll(); QVERIFY(lines.contains("EOF"));
        QSet<qint64> snapshotProcesses,localProcesses;
        for(const auto &line:lines.split('\n')) {auto row=QJsonDocument::fromJson(line).object(); if(row["method"]==QJsonValue("native.snapshot")) snapshotProcesses.insert(row["pid"].toInteger()); if(row["method"].toString().startsWith("native.local.")) localProcesses.insert(row["pid"].toInteger());}
        for(auto processId:localProcesses) QVERIFY(!snapshotProcesses.contains(processId));
    }
    void local_main_preview_restart_preserves_job_owner() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local-slow"),dir.filePath("native")); auto *local=model.localImport(); local->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000); QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); waitEvents(80);
        auto *client=local->findChild<BackendClient*>(); auto pid=client->findChild<QProcess*>()->processId(); auto id=local->jobId();
        QVERIFY(model.previewApply("/synthetic/game")); waitEvents(80); QVERIFY(model.cancelPreview());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),2000); QCOMPARE(local->jobId(),id); QCOMPARE(client->findChild<QProcess*>()->processId(),pid);
        QTRY_VERIFY_WITH_TIMEOUT(local->canCancel(),3000);
        QVERIFY(local->cancel()); QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled",1000);
    }
    void local_poll_error_cannot_clear_main_writer_marker() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,"native-local-held-status-error"),dir.filePath("native")); auto *local=model.localImport(); local->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000); QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); waitEvents(80);
        QVERIFY(model.setModEnabled("image-mod",false));
        QTRY_VERIFY_WITH_TIMEOUT(local->error().contains("localPollFailure"),1500); QVERIFY(model.writeBusy()); QVERIFY(model.error().isEmpty());
        QVERIFY(local->cancel()); QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled",1000); QVERIFY(model.writeBusy());
        QTRY_VERIFY_WITH_TIMEOUT(!model.writeBusy(),7000);
    }
    void local_one_bulk_permit_and_latest_view(const QString &mode="native-local-crossing") {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,mode),dir.filePath("native")); auto *local=model.localImport(); local->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000); QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")}));
        QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="ready",2000);
        model.viewProfile("slow"); model.viewProfile("fast"); QVERIFY(local->cancel());
        // The maximum frame crosses thousands of pipe reads in this 1 ms polling harness.
        const int deliveryTimeout=mode.contains("boundary") ? 8000 : 3000;
        QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled" && !model.loading(),deliveryTimeout); QCOMPARE(model.viewedProfileId(),"fast"); QVERIFY(!local->canCommit());
        QFile trace(dir.filePath("native/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly)); QSet<QString> outstanding;
        int maximum=0;
        for(const auto &line:trace.readAll().split('\n')) {
            auto row=QJsonDocument::fromJson(line).object(); auto key=QString::number(row["pid"].toInteger())+":"+QString::number(row["id"].toInteger());
            if(row["event"]==QJsonValue("response")) {outstanding.remove(key); continue;}
            const auto method=row["method"].toString();
            const bool bulk=method=="native.snapshot" || method=="native.local.review" || (method=="native.local.status" && row["params"].toObject()["includeCandidates"].toBool());
            if(bulk) {outstanding.insert(key); maximum=qMax(maximum,int(outstanding.size()));}
        }
        QCOMPARE(maximum,1); QVERIFY(outstanding.isEmpty());
    }
    void local_old_job_reply_cannot_replace_current_job(const QString &mode="native-local-old-job") {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir,mode),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true); QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")})); const auto current=local->jobId();
        QTRY_VERIFY_WITH_TIMEOUT(local->error().contains("invalid local import response"),2000);
        QCOMPARE(local->jobId(),current); QVERIFY(!local->canCommit()); QVERIFY(local->review().isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="interrupted",2000); QVERIFY(!local->canCancel());
    }
    void native_snapshot_accepts_versions_one_and_two_strictly() {
        auto native = previewSnapshot();
        native.insert("sourceKind", "native"); native.insert("revision", "1");
        native.insert("pendingChanges", true); native.insert("gamePath", "/synthetic/game");
        native.insert("appliedProfileId", QJsonValue::Null); native.insert("canRestore", false);
        native.insert("recoveryRequired", false);
        auto mods = native.value("mods").toArray(); auto mod = mods[0].toObject();
        mod.insert("appliedEnabled", false); mod.insert("available", true);
        mod.insert("unavailableReason", QJsonValue::Null); mods[0] = mod; native.insert("mods", mods);
        LibrarySnapshot decoded;
        for (int version : {1, 2}) {
            native.insert("storeVersion", version);
            QVERIFY(LibrarySnapshot::decode(native, decoded));
            QCOMPARE(decoded.selectedProfileId, "default"); QCOMPARE(decoded.mods[0].id, "image-mod");
            for (const auto *field : {"revision", "pendingChanges", "gamePath", "appliedProfileId", "canRestore", "recoveryRequired"}) {
                auto bad = native; bad.remove(field); QVERIFY(!LibrarySnapshot::decode(bad, decoded));
                QCOMPARE(decoded.selectedProfileId, "default");
            }
            for (const auto *field : {"appliedEnabled", "available", "unavailableReason"}) {
                auto bad = native; auto rows = mods; auto row = mod; row.remove(field); rows[0] = row;
                bad.insert("mods", rows); QVERIFY(!LibrarySnapshot::decode(bad, decoded));
            }
        }
        for (const auto &version : {QJsonValue(0), QJsonValue(3), QJsonValue(33), QJsonValue(1.5), QJsonValue(-1), QJsonValue("2"), QJsonValue()}) {
            native.insert("storeVersion", version); QVERIFY(!LibrarySnapshot::decode(native, decoded));
        }
        QVERIFY(LibrarySnapshot::decode(previewSnapshot(), decoded));
        auto warningSnapshot=previewSnapshot();
        warningSnapshot.insert("warnings",QJsonArray{QJsonObject{{"code","unverifiedRestore"},{"message","Check your restore point."}},"old string",42,QJsonObject{{"future",true}}});
        QVERIFY(LibrarySnapshot::decode(warningSnapshot,decoded));
        QCOMPARE(decoded.warnings,QStringList{"Check your restore point."});
        QCOMPARE(decoded.warningCodes,QStringList{"unverifiedRestore"});


        LibraryModel demo; demo.loadDemoData(3);
        QVERIFY(!demo.canImport() && !demo.canEditProfiles() && !demo.canPreviewApply() && !demo.canRestore() && !demo.canRecover());
        QVERIFY(!demo.previewImport("/source", "/game") && !demo.previewApply("/game") && !demo.commitApply() && !demo.commitImport());
    }
    void family_review_counts_are_strict_and_legacy_compatible() {
        const QStringList countKeys{"packageCount", "directoryCount", "physicalFileCount", "companionFileCount"};
        const auto review = [&](bool importing, const QJsonObject &counts, bool accepted) {
            QTemporaryDir dir; const auto data = dir.filePath("native"); QVERIFY(QDir().mkpath(data));
            QJsonObject body{{"token", "review"}, {"gamePath", "/game"}, {"totalBytes", 114}, {"warnings", QJsonArray{}}};
            if (importing) {
                body.insert("profileCount", 1); body.insert("modCount", 1); body.insert("availableModCount", 1); body.insert("unavailableModCount", 0);
                body.insert("profiles", QJsonArray{}); body.insert("unavailableMods", QJsonArray{}); body.insert("summariesTruncated", false);
            } else { body.insert("profileId", "default"); body.insert("fileCount", 1); body.insert("replacedMounts", QJsonArray{}); }
            for (auto i = counts.begin(); i != counts.end(); ++i) body.insert(i.key(), i.value());
            QFile fixture(dir.filePath("native/review.json")); QVERIFY(fixture.open(QIODevice::WriteOnly)); fixture.write(QJsonDocument(body).toJson()); fixture.close();
            BackendClient client; bool ready = false, done = false, valid = false; QVariantMap received;
            connect(&client, &BackendClient::snapshotReady, this, [&](const LibrarySnapshot &) { ready = true; });
            connect(&client, &BackendClient::reviewReady, this, [&](BackendClient::Call, const QVariantMap &v) { received = v; valid = true; done = true; });
            connect(&client, &BackendClient::failed, this, [&](const QString &) { done = true; });
            client.start(peer(dir, "native-family-review"), "/source", data, true);
            QTRY_VERIFY_WITH_TIMEOUT(ready, 3000);
            client.nativeRequest(importing ? BackendClient::Call::ImportPreview : BackendClient::Call::ApplyPreview, {{"profileId", "default"}});
            QTRY_VERIFY_WITH_TIMEOUT(done, 3000);
            QVERIFY2(valid == accepted, qPrintable(QString("%1 review accepted=%2: %3").arg(importing ? "Import" : "Apply").arg(valid).arg(QString::fromUtf8(QJsonDocument(counts).toJson(QJsonDocument::Compact)))));
            if (accepted) for (const auto &key : countKeys) QCOMPARE(received.contains(key), counts.contains(key));
        };
        const QJsonObject good{{"packageCount", 2}, {"directoryCount", 1}, {"physicalFileCount", 3}, {"companionFileCount", 2}};
        for (bool importing : {false, true}) {
            review(importing, {}, true); review(importing, good, true);
            for (const auto &key : countKeys) {
                auto partial = good; partial.remove(key); review(importing, partial, false);
                review(importing, {{key, 0}}, false);
                for (const auto &bad : {QJsonValue(-1), QJsonValue(1.5), QJsonValue("1"), QJsonValue(), QJsonValue(true)}) {
                    auto counts = good; counts.insert(key, bad); review(importing, counts, false);
                }
            }
            auto counts = good; counts.insert("directoryCount", 3); review(importing, counts, false);
            counts = good; counts.insert("companionFileCount", 4); review(importing, counts, false);
        }
        auto shared = good; shared.insert("directoryCount", 2); shared.insert("physicalFileCount", 2);
        review(true, shared, true); review(false, shared, false);
        auto mismatch = good; mismatch.insert("physicalFileCount", 4); review(false, mismatch, false);
        mismatch = good; mismatch.insert("fileCount", 2); review(false, mismatch, false);
        QJsonObject zeros; for (const auto &key : countKeys) zeros.insert(key, 0);
        zeros.insert("fileCount", 0); review(false, zeros, true); review(true, zeros, true);
    }
    void family_operation_reconciliation_keeps_unknown_identity() {
        QTemporaryDir dir; const auto data = dir.filePath("native"); const auto executable = peer(dir, "native-family-unknown");
        const auto marker = QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(data.toUtf8(), QCryptographicHash::Sha256).toHex() + ".json");
        const auto identity = [&] { QFile f(marker); QVERIFY(f.open(QIODevice::ReadOnly)); return QJsonDocument::fromJson(f.readAll()).object().value("operationId").toString(); };
        QString original;
        {
            // The edit's invalid reply is settled by a reload, with no marker.
            LibraryModel model; model.loadNative(executable, data);
            QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model), 3000); QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
            model.setSelectedId("image-mod"); QVERIFY(model.setModEnabled("image-mod", false));
            QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/finished.json")) && !model.loading() && !model.busy(), 5000);
            QVERIFY(!model.outcomeUnknown()); QVERIFY(!QFile::exists(marker)); QCOMPARE(model.revision(), "2");
        }
        {
            // An edit marker left by an older build is still read and reconciled.
            QFile finished(dir.filePath("native/finished.json")); QVERIFY(finished.open(QIODevice::ReadOnly));
            original = QJsonDocument::fromJson(finished.readAll()).object().value("operationId").toString(); QVERIFY(!original.isEmpty());
            QVERIFY(AppPaths::prepareState()); QVERIFY(QDir().mkpath(QFileInfo(marker).absolutePath()));
            QFile f(marker); QVERIFY(f.open(QIODevice::WriteOnly));
            f.write(QJsonDocument(QJsonObject{{"operationId", original}, {"method", int(BackendClient::Call::SetEnabled)}}).toJson(QJsonDocument::Compact));
        }
        LibraryModel reopened; reopened.loadNative(executable, data);
        // Observe the intended invalid old outcome before asking for a new
        // reply, rather than refreshing in the middle of startup reconciliation.
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/outcome-read")) && !reopened.loading() && !reopened.busy(), 3000);
        QCOMPARE(reopened.error(),QString("Helper returned an invalid operation outcome."));
        QVERIFY(reopened.outcomeUnknown()); QCOMPARE(identity(), original);
        QFile allowed(dir.filePath("native/allow-outcome")); QVERIFY(allowed.open(QIODevice::WriteOnly)); allowed.close();
        reopened.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!reopened.loading() && !reopened.busy() && !reopened.outcomeUnknown(), 3000);
        QCOMPARE(reopened.revision(), "2"); QCOMPARE(reopened.viewedProfileId(), "default");
        QCOMPARE(reopened.selectedId(), "image-mod"); QVERIFY(!reopened.selectedMod().value("enabled").toBool());
        QVERIFY(identity().isEmpty());
        QFile trace(dir.filePath("native/dispatches")); QVERIFY(trace.open(QIODevice::ReadOnly)); QCOMPARE(trace.readAll(), QByteArray("write\n"));
    }
    void nativeSnapshotDecode() {
        auto object = previewSnapshot();
        object.insert("sourceKind", "native"); object.insert("storeVersion", 1);
        object.insert("revision", "9007199254740993");
        object.insert("pendingChanges", true); object.insert("gamePath", "/synthetic/game");
        object.insert("appliedProfileId", QJsonValue::Null);
        object.insert("canRestore", false); object.insert("recoveryRequired", false);
        auto mods = object.value("mods").toArray(); auto mod = mods[0].toObject();
        mod.insert("appliedEnabled", false); mod.insert("available", true);
        mod.insert("unavailableReason", QJsonValue::Null); mods[0] = mod; object.insert("mods", mods);
        LibrarySnapshot snapshot;
        QVERIFY(LibrarySnapshot::decode(object, snapshot));
        object.insert("storageRoot", "/synthetic/other drive");
        QVERIFY(LibrarySnapshot::decode(object, snapshot));
        QCOMPARE(snapshot.storageRoot, QString("/synthetic/other drive"));
        object.insert("storageRoot", 123); QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        object.insert("storageRoot", "relative"); QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        object.insert("storageRoot", QJsonValue::Null); QVERIFY(LibrarySnapshot::decode(object, snapshot));
        QCOMPARE(snapshot.mods[0].details().value("appliedEnabled").toBool(), false);
        QVERIFY(snapshot.mods[0].details().value("available").toBool());
        object.insert("revision", 3); QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        object.insert("revision", "01"); QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        object.insert("revision", "1"); object.insert("pendingChanges", "yes");
        QVERIFY(!LibrarySnapshot::decode(object, snapshot));
    }
    void nativeAsyncAuthorityAndRefresh() {
        QTemporaryDir dir; LibraryModel model;
        QVERIFY(QMetaObject::invokeMethod(&model, "loadNative", Q_ARG(QString, peer(dir, "native-delay")), Q_ARG(QString, dir.filePath("native"))));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(model.property("nativeMode").toBool());
        QCOMPARE(model.property("revision").toString(), "1");
        QVERIFY(model.modDetails("image-mod").value("enabled").toBool());
        QVERIFY(model.setModEnabled("image-mod", false));
        QVERIFY(model.modDetails("image-mod").value("enabled").toBool());
        QVERIFY(model.property("writeBusy").toBool());
        model.refresh(); model.viewProfile("fast");
        model.setQuery("Artwork"); QCOMPARE(model.count(), 1);
        QTimer heartbeat; heartbeat.setInterval(10); int beats = 0;
        connect(&heartbeat, &QTimer::timeout, this, [&] { ++beats; }); heartbeat.start();
        QTRY_VERIFY_WITH_TIMEOUT(!model.property("busy").toBool(), 9000);
        QVERIFY(beats > 300);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(!model.modDetails("image-mod").value("enabled").toBool());
        QCOMPARE(model.property("revision").toString(), "2");
        QVERIFY(!model.modDetails("image-mod").value("appliedEnabled").toBool());
    }
    void nativeRejectedMutation() {
        QTemporaryDir dir; LibraryModel model;
        QVERIFY(QMetaObject::invokeMethod(&model, "loadNative", Q_ARG(QString, peer(dir, "native-reject")), Q_ARG(QString, dir.filePath("native"))));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(!model.property("busy").toBool(), 7000);
        QVERIFY(model.error().contains("staleRevision"));
        QVERIFY(model.modDetails("image-mod").value("enabled").toBool());
        QCOMPARE(model.property("revision").toString(), "1");
    }
    void item115SmallLaunchCauseReplacesEmptyOrStaleError(){
        for(const auto &previous:QStringList{QString(),QString("Unrelated old error")}){
            LibraryModel model;model.setError(previous);const QString current="recoveryRequired: Current Play preparation failed: storageFull";
            // Play's preparation failed and the snapshot read after it reports unfinished files.
            QTemporaryDir dir;model.m_dataDir=dir.filePath("native");
            auto *launch=model.launch();launch->m_error=current;launch->m_committed=true;launch->m_attempt={{"state","terminal"}};launch->m_binding={{"expectedRevision","1"}};
            LibrarySnapshot unfinished;unfinished.sourceKind="native";unfinished.revision="1";unfinished.selectedProfileId="default";unfinished.recoveryRequired=true;
            launch->m_viewReadSent=true;model.applySnapshot(unfinished);
            QVERIFY(launch->preparationViewConfirmed());QCOMPARE(launch->error(),current);QCOMPARE(model.error(),previous);
            // Play owns its cause. Retiring or refreshing must not copy it into the library.
            model.m_helperPath=helper();model.m_restartAfterDrain=true;emit model.m_client->retired();
            QVERIFY(model.loading());QCOMPARE(launch->error(),current);QCOMPARE(model.error(),previous);
            model.applySnapshot(unfinished);
            QVERIFY(!model.loading());QVERIFY(model.recoveryRequired());QCOMPARE(model.error(),previous);
            model.m_client->rejected(BackendClient::Call::Recover,"nativeIo","Current Recover failed");
            const QString recovery="nativeIo: Current Recover failed";
            QCOMPARE(model.error(),recovery);QCOMPARE(launch->error(),current);
            model.refresh();QCOMPARE(model.error(),recovery);
            model.applySnapshot(unfinished);QCOMPARE(model.error(),recovery);QCOMPARE(launch->error(),current);
            unfinished.recoveryRequired=false;model.applySnapshot(unfinished);
            QCOMPARE(model.error(),recovery);QCOMPARE(launch->error(),current);
        }
        // A successful generic Recover with no original marker cannot leave a copied Play error.
        QTemporaryDir dir;LibraryModel model;model.m_dataDir=dir.filePath("native");
        const QString cause="recoveryRequired: Play preparation failed";
        model.launch()->m_error=cause;model.m_outcomeUnknown=true;
        model.m_remoteOutcome={{"status","succeeded"}};
        model.finishRecoveryWrapper(false,false,model.m_errorGeneration);
        QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
        QVERIFY(!model.outcomeUnknown());QVERIFY(model.error().isEmpty());QCOMPARE(model.launch()->error(),cause);
    }
    void item124HistoricalRecoveryDoesNotProveOriginal(){
        // A saved successful Recover ID is not a result for the original operation.
        for(const auto method:{BackendClient::Call::ImportCommit,BackendClient::Call::LaunchCommit}){
            QTemporaryDir dir;LibraryModel model;model.m_dataDir=dir.filePath("native");
            const QVariantMap marker{{"method",int(method)},{"operationId","original"},{"recoveryId","older-recovery"}};
            model.m_marker=marker;model.m_outcomeUnknown=true;model.m_recoveryRequired=true;
            model.m_client->operationReady({{"operationId","older-recovery"},{"status","succeeded"},{"confirmedRevision","4"}});
            QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
            model.m_client->operationReady({{"operationId","original"},{"status","unknown"}});
            QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
            QVERIFY(model.outcomeUnknown());QVERIFY(model.recoveryRequired());QCOMPARE(model.m_marker,marker);
            QVERIFY(!model.canSetEnabled());QVERIFY(!model.canEditProfiles());
        }
    }
    void item124RestartViewWaitsForTheAttempt(){
        // After an app restart the helper's handoff names the launch. A snapshot that
        // arrives before that attempt is read says nothing about the attempt's result.
        using C=BackendClient::Call;
        LibrarySnapshot snapshot;snapshot.sourceKind="native";snapshot.revision="5";snapshot.selectedProfileId="default";
        const QJsonObject barriers{{"barriers",QJsonArray{QJsonObject{{"epoch","7"},{"attemptId","attempt-a"}}}}};
        {
            LibraryModel model;auto *launch=model.launch();
            launch->complete(C::LaunchBarriers,barriers);
            QVERIFY(launch->writeGate());QCOMPARE(launch->m_binding.value("attemptId").toString(),QString("attempt-a"));QVERIFY(launch->m_attempt.isEmpty());
            // The snapshot comes first.
            launch->m_viewReadSent=true;model.applySnapshot(snapshot);
            QVERIFY(!launch->preparationViewConfirmed());QVERIFY(!launch->viewReadDue());
            // Then the attempt: its preparation left revision 7.
            launch->acceptAttempt({{"epoch","7"},{"attemptId","attempt-a"},{"recordRevision","3"},{"state","handoff"},{"preparation",QJsonObject{{"resultingRevision","7"}}}},false);
            QVERIFY(launch->viewReadDue());
            launch->m_viewReadSent=true;model.applySnapshot(snapshot);
            QVERIFY(!launch->preparationViewConfirmed());
            snapshot.revision="7";launch->m_viewReadSent=true;model.applySnapshot(snapshot);
            QVERIFY(launch->preparationViewConfirmed());
        }
        {
            // An attempt that names no revision gives no floor: no snapshot can confirm it.
            LibraryModel model;auto *launch=model.launch();
            launch->complete(C::LaunchBarriers,barriers);
            launch->acceptAttempt({{"epoch","7"},{"attemptId","attempt-a"},{"recordRevision","3"},{"state","terminal"}},false);
            QVERIFY(launch->viewReadDue());
            launch->m_viewReadSent=true;model.applySnapshot(snapshot);
            QVERIFY(!launch->preparationViewConfirmed());QVERIFY(launch->writeGate());
        }
    }
    void item124EditingWaitsForTheViewAfterPlay(){
        // The binding of a finished Play attempt closes editing until a snapshot shows
        // the attempt's result. Game-config saves have no other app-side gate.
        using C=BackendClient::Call;
        QTemporaryDir dir;const auto data=dir.filePath("native"),source=dir.filePath("state.json");
        QVERIFY(QDir().mkpath(dir.filePath("game/citadel/cfg")));
        QFile f(source);QVERIFY(f.open(QIODevice::WriteOnly));f.write(R"({"local-config":{"version":33,"state":{"activeProfileId":"default","profiles":{"default":{"name":"Default","mods":[]}}}}})");f.close();
        QProcess importer;importer.start(helper());QVERIFY(importer.waitForStarted());qint64 id=0;
        const auto rpc=[&](const QString &method,QJsonObject params){params.insert("dataDir",data);importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');QVERIFY(importer.waitForBytesWritten());while(!importer.canReadLine())QVERIFY(importer.waitForReadyRead(7000));auto r=QJsonDocument::fromJson(importer.readLine()).object();QVERIFY2(!r.contains("error"),QJsonDocument(r).toJson().constData());return r.value("result").toObject();};
        auto r=rpc("native.importPreview",{{"statePath",source},{"gamePath",dir.path()}});
        rpc("native.importCommit",{{"token",r.value("token")},{"operationId",QString(32,'c')},{"expectedRevision","0"}});importer.closeWriteChannel();QVERIFY(importer.waitForFinished());
        LibraryModel model;model.loadNative(helper(),data);auto *a=model.autoexec();auto *c=model.crosshair();auto *launch=model.launch();
        QTRY_VERIFY_WITH_TIMEOUT(a->available()&&!a->busy()&&!c->values().isEmpty()&&!c->busy()&&model.canSetEnabled()&&model.canEditProfiles()&&model.profileEditsAvailable(),15000);
        QVERIFY(a->preview("echo play"));QTRY_VERIFY_WITH_TIMEOUT(!a->review().isEmpty()&&!a->busy(),7000);
        const auto token=a->review().value("token").toString();
        const auto closed=[&]{return !model.canSetEnabled()&&!model.canEditProfiles()&&!model.profileEditsAvailable()&&!a->commit(token)&&!c->preview();};
        const auto barriersRead=[&]{launch->m_barriersReadNeeded=true;launch->poll();QTRY_VERIFY_WITH_TIMEOUT(!launch->m_barriersReadNeeded&&!launch->busy(),7000);};
        // A Play attempt ended. Its record names a revision that the library does not show yet.
        const auto shown=model.revision();
        const auto attempt=[&](const QString &revision,const QString &record){return QJsonObject{{"epoch","1"},{"attemptId","attempt-a"},{"recordRevision",record},{"state","terminal"},
            {"preparation",QJsonObject{{"resultingRevision",revision}}},{"outcome",QJsonObject{{"status","observed"},{"reason","gameCycleCompleted"}}}};};
        launch->m_binding={{"root",data},{"epoch","1"},{"attemptId","attempt-a"},{"expectedRevision",shown}};launch->m_committed=true;launch->m_preparationViewConfirmed=false;
        int snapshots=0;connect(model.m_client,&BackendClient::snapshotReady,&model,[&snapshots]{++snapshots;});
        launch->acceptAttempt(attempt(QString::number(shown.toULongLong()+1),"1"),false);
        // 1. The snapshot is pending.
        QVERIFY(launch->m_viewReadSent);QVERIFY(model.loading());QVERIFY(closed());
        // 2. The helper refuses it.
        emit model.m_client->rejected(C::Snapshot,"launchBusy","The launch owner is busy.");
        QVERIFY(!model.loading());QVERIFY(!a->busy());QVERIFY(closed());
        // 3. The helper reports no handoff, and a snapshot arrives that predates the attempt's result.
        barriersRead();QVERIFY(launch->barriers().isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(snapshots>=1&&!model.loading()&&!a->busy(),7000);
        QVERIFY(!launch->preparationViewConfirmed());QVERIFY(closed());
        barriersRead();QTRY_VERIFY_WITH_TIMEOUT(!model.loading()&&!a->busy(),7000);
        QVERIFY(closed());QVERIFY(!launch->m_binding.isEmpty());
        // 4. The record now names the revision the library shows: the next snapshot qualifies.
        launch->acceptAttempt(attempt(shown,"2"),false);
        QTRY_VERIFY_WITH_TIMEOUT(launch->preparationViewConfirmed(),7000);
        QTRY_VERIFY_WITH_TIMEOUT(launch->m_binding.isEmpty()&&model.canSetEnabled()&&model.canEditProfiles()&&model.profileEditsAvailable()&&!a->busy()&&!c->busy(),15000);
        QVERIFY(c->preview());QTRY_VERIFY_WITH_TIMEOUT(!c->review().isEmpty()&&!c->busy(),7000);
        QVERIFY(a->preview("echo play"));QTRY_VERIFY_WITH_TIMEOUT(!a->review().isEmpty()&&!a->busy(),7000);
        QVERIFY(a->commit(a->review().value("token").toString()));QTRY_VERIFY_WITH_TIMEOUT(a->canUndo()&&!a->busy(),7000);
    }
    void item115SmallGesturesClearOldDisplayError(){
        for(const auto &gesture:QStringList{"play","apply","restore"}){
            QTemporaryDir dir;LibraryModel model;model.loadNative(helper(),dir.filePath("native"));QTRY_VERIFY_WITH_TIMEOUT(model.m_client->readyForRequests()&&!model.loading(),7000);
            model.m_nativeMode=true;model.m_activeProfileId="default";model.m_viewedProfileId="default";model.m_revision="1";model.m_gamePath="/synthetic/game";model.m_canRestore=true;
            model.m_marker.clear();model.m_outcomeUnknown=false;model.m_recoveryRequired=false;model.setBusy(false);model.setLoading(false);
            auto *launch=model.launch();launch->m_fileBusy=false;launch->m_settingsRevision="0";launch->m_binding={};launch->m_pending.reset();launch->m_barriers.clear();launch->m_terminalRead=false;
            model.setError("Unrelated earlier error");
            const bool started=gesture=="play"?launch->launchActive():gesture=="apply"?model.previewApply("/synthetic/game"):model.restorePrevious();
            // Play keeps a still-current library warning (e.g. unsaved queued choices).
            QVERIFY(started);if(gesture=="play")QCOMPARE(model.error(),QString("Unrelated earlier error"));else QVERIFY(model.error().isEmpty());
        }
    }
    void item115RecoveryCauseKeepsMarker() {
        QTemporaryDir dir;const auto data=dir.filePath("native");
        const auto markerPath=QDir(AppPaths::stateRoot()).filePath("pending/"+QCryptographicHash::hash(data.toUtf8(),QCryptographicHash::Sha256).toHex()+".json");
        QVERIFY(QDir().mkpath(QFileInfo(markerPath).absolutePath()));QFile saved(markerPath);QVERIFY(saved.open(QIODevice::WriteOnly));
        saved.write(R"({"operationId":"original","method":2})");saved.close();
        LibraryModel model;model.loadNative(peer(dir,"recovery-unknown"),data);QTRY_VERIFY_WITH_TIMEOUT(model.canRecover()&&!model.busy(),7000);
        const auto marker=model.m_marker;
        model.m_client->rejected(BackendClient::Call::Restore,"recoveryRequired","An unfinished game operation needs explicit recovery. Cause during restore-publication: gameRunning: injected guard; refusal check failed: nativeIo: Input/output error");
        waitEvents(100);
        QVERIFY(model.recoveryRequired());QVERIFY(model.outcomeUnknown());QVERIFY(model.canRecover());QCOMPARE(model.m_marker,marker);QVERIFY(QFile::exists(markerPath));
        QVERIFY(model.error().contains("gameRunning"));QVERIFY(model.error().contains("nativeIo"));
    }
    void item115FinalRecoverShowsLatestBlocker() {
        QTemporaryDir dir;const auto data=dir.filePath("native");
        const auto markerPath=QDir(AppPaths::stateRoot()).filePath("pending/"+QCryptographicHash::hash(data.toUtf8(),QCryptographicHash::Sha256).toHex()+".json");
        QVERIFY(QDir().mkpath(QFileInfo(markerPath).absolutePath()));QFile saved(markerPath);QVERIFY(saved.open(QIODevice::WriteOnly));saved.write(R"({"operationId":"original","method":2})");saved.close();
        LibraryModel model;model.loadNative(peer(dir,"recovery-unknown"),data);QTRY_VERIFY_WITH_TIMEOUT(model.canRecover()&&!model.busy(),7000);
        model.m_client->rejected(BackendClient::Call::Restore,"recoveryRequired","Recovery needed. Cause during publication: storageFull: No space left");
        const auto original=model.m_marker;model.m_marker={{"nativeRecoveryMarkerVersion",1},{"method",int(BackendClient::Call::Recover)},{"operationId","recover-now"},{"originalMarker",original}};
        const auto wrapper=model.m_marker;
        for (const auto &row:QList<QPair<QString,QString>>{{"gameRunning","Close Deadlock before recovery."},{"nativeIo","Cannot read recovery files."}}) {
            model.m_remoteStage=LibraryModel::RemoteStage::RecoveryReplay;model.m_remoteOutcome.clear();
            model.m_client->rejected(BackendClient::Call::Recover,row.first,row.second);waitEvents(100);
            QVERIFY(model.recoveryRequired());QVERIFY(model.recoveryNeedsAttention());QVERIFY(model.outcomeUnknown());QVERIFY(model.canRecover());QCOMPARE(model.m_marker,wrapper);QVERIFY(QFile::exists(markerPath));
            QCOMPARE(model.error(),row.first+": "+row.second);QVERIFY(!model.error().contains("storageFull"));
        }
    }
    // Exercise real worker completions; helper signals are delivered before Qt
    // can deliver the queued result. "repeat" is a new event with the old text.
    void step8LateError(const QString &site,const QString &race) {
        QTemporaryDir dir; LibraryModel model; QVERIFY(dir.isValid());
        model.m_dataDir=dir.filePath("native");
        const QString newer="nativeIo: Newer helper rejection";
        const QString captured="nativeBusy: Earlier operation failure";
        const QVariantMap marker{{"operationId","original"},{"method",int(BackendClient::Call::Restore)}};
        const auto checkSaved=[&](const QVariantMap &expected){
            QFile file(model.markerPath());QVERIFY(file.open(QIODevice::ReadOnly));
            QCOMPARE(QJsonDocument::fromJson(file.readAll()).object().toVariantMap(),expected);
        };
        bool injected=false;
        const auto reject=[&]{injected=true;model.m_client->rejected(BackendClient::Call::Snapshot,"nativeIo","Newer helper rejection");};
        bool continuation=false,failed=false;
        QObject callbacks; // disconnect before captured locals die, including on assertion failure
        auto reentry=QMetaObject::Connection{};
        if(race=="reentry" || race=="queued")reentry=connect(&model,&LibraryModel::nativeChanged,&callbacks,[&]{
            const bool owns=model.m_otherMarkerFileTasks || model.m_bootstrapMarkerFileBusy;
            if(!injected && owns && (race=="queued" || !model.m_busy))reject();
        });
        if(site.startsWith("bootstrap-dispatch")) {
            model.loadNative(helper(),model.m_dataDir);
            QTRY_VERIFY_WITH_TIMEOUT(model.canCreateLibrary(),7000);
            if(race=="clear-entry" || race=="busy-entry") {
                model.setError("nativeIo: Previous display failure");
                if(race=="clear-entry")reentry=connect(&model,&LibraryModel::errorChanged,&callbacks,[&]{if(!injected && model.error().isEmpty())reject();});
                else reentry=connect(&model,&LibraryModel::nativeChanged,&callbacks,[&]{if(!injected && model.m_bootstrapMarkerFileBusy && model.m_writeBusy)reject();});
            }
            if(site.endsWith("retry")) {
                const QString id(32,'a');
                model.m_marker={{"bootstrapVersion",1},{"root",model.m_dataDir},{"operationId",id},
                    {"method",int(BackendClient::Call::LibraryInitialize)},{"expectedRevision","0"},
                    {"params",QVariantMap{{"version",1},{"operationId",id},{"expectedRevision","0"}}}};
                model.m_outcomeUnknown=true;
                QVERIFY(model.reconcileOperation());
            } else QVERIFY(model.createLibrary());
            const auto expected=model.m_marker;
            // The marker was reserved, but the dispatch precondition has gone
            // away before its worker completion. No helper write may be sent.
            model.m_writeBusy=false;
            if(race=="newer")reject();
            QTRY_VERIFY_WITH_TIMEOUT(!model.m_bootstrapMarkerFileBusy,7000);
            if(reentry)disconnect(reentry);
            QCOMPARE(model.error(),race=="none"?QString("Creation could not be sent. Reconnect and check the saved creation context."):newer);
            QVERIFY(model.m_bootstrapMarkerDurable);QCOMPARE(model.m_marker,expected);checkSaved(expected);
            QVERIFY(model.outcomeUnknown());QVERIFY(!model.busy());QVERIFY(!model.m_reconciling);
            QVERIFY(!QFileInfo::exists(QDir(model.m_dataDir).filePath("library.sqlite3")));
            if(race!="none")QVERIFY(injected);
            return;
        }
        if(site.startsWith("apply-") || site=="outcome-refresh") {
            model.loadNative(peer(dir,"native-local-held"),model.m_dataDir);
            QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading(),7000);
        }
        if(race=="refresh")reentry=connect(&model,&LibraryModel::loadingChanged,&callbacks,[&]{if(!injected && model.loading())reject();});
        model.setError(race=="repeat"?newer:QString("nativeIo: Previous display failure"));
        model.setBusy(true);
        if(race=="early-review")reentry=connect(&model,&LibraryModel::reviewChanged,&callbacks,[&]{if(!injected)reject();});
        QString intended;
        if(site=="persist-failure") {
            model.m_marker=marker;
            QVERIFY(QDir().mkpath(model.markerPath())); // a real QSaveFile refusal
            model.persistMarker({{"operationId","never-saved"}},[&]{continuation=true;},[&]{failed=true;});
            intended="Cannot persist operation recovery context. Check config storage and permissions.";
        } else if(site=="marker-read-failure") {
            QVERIFY(QDir().mkpath(QFileInfo(model.markerPath()).absolutePath()));
            QFile file(model.markerPath());QVERIFY(file.open(QIODevice::WriteOnly));file.write("{");file.close();
            model.setBusy(false);
            model.beginLoad(peer(dir,"hello"),{},model.m_dataDir,false);
            intended="Cannot read saved operation context. Preserve and restore the config file before retrying.";
        } else if(site=="refusal" || site=="refusal-refresh") {
            model.m_marker=marker;model.m_outcomeUnknown=true;
            const QString kind=site=="refusal-refresh"?"staleRevision":"nativeBusy";
            intended=kind+": Earlier operation failure";
            model.m_client->rejected(BackendClient::Call::Restore,kind,"Earlier operation failure");
        } else if(site=="outcome" || site=="outcome-refresh" || site.startsWith("replay-")) {
            model.m_marker=marker;model.m_outcomeUnknown=true;
            model.m_client->operationReady({{"operationId","original"},{"status","failed"},
                {"error",QVariantMap{{"kind","nativeBusy"},{"message","Earlier operation failure"}}}});
            intended=captured;
        } else if(site.startsWith("apply-")) {
            const bool success=site=="apply-success";
            model.m_marker={{"operationId","original"},{"method",int(BackendClient::Call::Apply)}};
            model.m_confirmationTarget=LibraryModel::ConfirmationTarget::Apply;
            model.m_autoCycle=true;model.m_autoBlocked=false;
            model.confirmLocalView(success,success?QString():captured);
            intended=success?QString():captured;
        } else QVERIFY2(false,"unknown STEP8 site");
        if(race=="newer" || race=="repeat")reject();
        if(site=="marker-read-failure") {
            QTRY_VERIFY_WITH_TIMEOUT(model.m_invalidMarker && model.m_client->isRunning(),7000);
            QVERIFY(model.outcomeUnknown());QVERIFY(model.m_marker.isEmpty());
        } else {
            QTRY_VERIFY_WITH_TIMEOUT(!model.m_localConfirmation && model.m_otherMarkerFileTasks==0,7000);
            QVERIFY(!model.busy());QVERIFY(!model.writeBusy());
            if(site=="persist-failure") {
                QVERIFY(failed);QVERIFY(!continuation);QCOMPARE(model.m_marker,marker);
                QVERIFY(QFileInfo(model.markerPath()).isDir());
            } else {
                QVERIFY(model.m_marker.isEmpty());QVERIFY(!model.outcomeUnknown());checkSaved({});
                if(site.startsWith("apply-")) {
                    QCOMPARE(model.m_confirmationTarget,LibraryModel::ConfirmationTarget::None);
                    QVERIFY(!model.m_autoCycle);QVERIFY(!model.m_recoveryBarrier);
                }
            }
        }
        if(site.startsWith("replay-")) {
            // A read after the queued clear must not replay the old captured
            // failure over a newer helper response. refresh() preserves reads.
            QCOMPARE(model.error(),captured);
            const bool duringFailure=site.startsWith("replay-failure-reentry");
            if(duringFailure) {
                model.setBusy(true);
                reentry=connect(&model,&LibraryModel::nativeChanged,&callbacks,[&]{
                    if(injected || model.busy())return;
                    injected=true;
                    if(site.endsWith("repeat"))model.m_client->rejected(BackendClient::Call::Snapshot,"nativeBusy","Earlier operation failure");
                    else model.m_client->rejected(BackendClient::Call::Snapshot,"nativeIo","Newer helper rejection");
                });
            } else if(site=="replay-failure-repeat")model.m_client->rejected(BackendClient::Call::Snapshot,"nativeBusy","Earlier operation failure");
            else if(site!="replay-failure-current")reject();
            if(site=="replay-snapshot") {
                LibrarySnapshot snapshot;snapshot.sourceKind="native";snapshot.revision="1";snapshot.selectedProfileId="default";
                model.m_client->snapshotReady(snapshot);
            } else model.m_client->failed("nativeIo: Current read failed");
            QCOMPARE(model.error(),duringFailure?(site.endsWith("repeat")?captured:newer):site=="replay-failure-current"?captured:site=="replay-snapshot"?newer:QString("nativeIo: Current read failed"));
            if(duringFailure)QVERIFY(injected);
        } else {const auto want=race=="none"?intended:newer;QVERIFY2(model.error()==want,qPrintable("error ["+model.error()+"] expected ["+want+"]"));}
        if(reentry)disconnect(reentry);
        if(race!="none")QVERIFY(injected);
    }
    void step8RecoveryError(const QString &site,const QString &race) {
        QTemporaryDir dir;LibraryModel model;model.m_dataDir=dir.filePath("native");
        const QString newer="nativeIo: Newer helper rejection";
        const QString captured="nativeBusy: Earlier recovery failure";
        bool injected=false,retryScheduled=false;
        const auto reject=[&]{injected=true;model.m_client->rejected(BackendClient::Call::Snapshot,"nativeIo","Newer helper rejection");};
        if(site=="remote" || site=="wrapper-snapshot") {
            bool initialView=false;QObject initialCallbacks;
            const auto initial=connect(model.m_client,&BackendClient::snapshotReady,&initialCallbacks,[&]{initialView=true;});
            model.m_client->start(peer(dir,"hello"),{},model.m_dataDir,false);
            QTRY_VERIFY_WITH_TIMEOUT(initialView && model.m_client->readyForRequests(),7000);
            disconnect(initial);
            model.m_marker={{"remoteMarkerVersion",2},{"method",int(BackendClient::Call::RemoteCommit)},{"root",model.m_dataDir},
                {"profileId","default"},{"jobId",QString(32,'a')},{"expectedRevision","1"},{"operationId","remote-op"},
                {"modId","image-mod"},{"recoveryId","recovery"},{"recoveryExpectedRevision","1"}};
            if(site=="wrapper-snapshot")model.m_marker={{"nativeRecoveryMarkerVersion",1},{"method",int(BackendClient::Call::Recover)},
                {"root",model.m_dataDir},{"expectedRevision","1"},{"originalMarker",QVariantMap{}}};
        }
        auto expected=model.m_marker;expected.remove("recoveryId");expected.remove("recoveryExpectedRevision");
        if(site=="wrapper-snapshot")expected.clear();
        model.setError(race=="repeat"?newer:QString("nativeIo: Previous display failure"));
        model.m_remoteStage=LibraryModel::RemoteStage::RecoveryView;
        if(site!="bootstrap-clear")model.m_remoteOutcome={{"status","failed"},{"error",QVariantMap{{"kind","nativeBusy"},{"message","Earlier recovery failure"}}}};
        model.setBusy(true);
        QObject callbacks;
        if(race=="snapshot-entry") {
            model.m_loading=true;
            connect(&model,&LibraryModel::loadingChanged,&callbacks,[&]{
                if(injected || model.loading())return;
                injected=true;model.m_client->rejected(BackendClient::Call::Profile,"nativeIo","Newer helper rejection");
            });
        }
        const auto connection=connect(&model,&LibraryModel::nativeChanged,&callbacks,[&]{
            if(race=="reentry" && !injected && (model.m_otherMarkerFileTasks || model.m_bootstrapMarkerFileBusy) && !model.m_busy)reject();
            if(!model.m_otherMarkerFileTasks)retryScheduled=model.m_remoteRetry.isActive();
        });
        if(site=="remote" || site=="wrapper-snapshot") {
            LibrarySnapshot snapshot;snapshot.sourceKind="native";snapshot.revision="1";snapshot.selectedProfileId="default";
            model.applySnapshot(snapshot);
        } else if(site=="bootstrap-clear") {
            model.m_marker={{"method",int(BackendClient::Call::LibraryInitialize)},{"operationId","creation"}};
            model.m_bootstrapConfirming=true;model.m_bootstrapCreated=true;
            LibrarySnapshot snapshot;snapshot.sourceKind="native";snapshot.revision="1";snapshot.selectedProfileId="default";
            model.applySnapshot(snapshot);
        } else model.finishRecoveryWrapper(true,false,model.m_errorGeneration);
        if(race=="newer" || race=="repeat")reject();
        QTRY_VERIFY_WITH_TIMEOUT(!model.m_otherMarkerFileTasks && !model.m_bootstrapMarkerFileBusy,7000);
        disconnect(connection);
        QCOMPARE(model.m_marker,expected);
        QFile saved(model.markerPath());QVERIFY(saved.open(QIODevice::ReadOnly));
        QCOMPARE(QJsonDocument::fromJson(saved.readAll()).object().toVariantMap(),expected);
        QVERIFY(!model.busy());QVERIFY(!model.m_reconciling);QVERIFY(model.m_remoteOutcome.isEmpty());
        if(site=="bootstrap-clear") {
            QVERIFY(!model.outcomeUnknown());QVERIFY(!model.m_bootstrapMarkerDurable);
            QCOMPARE(model.bootstrapState(),QString("initialized"));
        } else {
            QVERIFY(model.recoveryRequired());QCOMPARE(model.outcomeUnknown(),site=="remote");
            QCOMPARE(model.m_remoteMarkerDurable,site=="remote");
            QCOMPARE(model.m_remoteStage,LibraryModel::RemoteStage::Idle);
            QVERIFY(!model.m_remoteRecoveryTerminal);
            if(site=="remote")QVERIFY(retryScheduled);
        }
        const auto want=race=="none"?(site=="bootstrap-clear"?QString():captured):newer;
        QVERIFY2(model.error()==want,qPrintable("error ["+model.error()+"] expected ["+want+"]"));
        if(race!="none")QVERIFY(injected);
    }
    void item129FailedRecoveryPreservesNewerError() {
        QTemporaryDir dir; LibraryModel model;
        model.m_dataDir=dir.filePath("native");
        model.m_remoteOutcome={{"status","failed"},{"error",QVariantMap{{"kind","nativeIo"},{"message","Earlier recovery failure"}}}};
        model.setBusy(true);
        model.finishRecoveryWrapper(true,false,model.m_errorGeneration);
        model.m_client->rejected(BackendClient::Call::Snapshot,"nativeIo","Newer read failure");
        QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
        QCOMPARE(model.error(),QString("nativeIo: Newer read failure"));
    }
    void item133RemoteRecoveryPreservesNewerError(bool repeatedIdentical = false) {
        QTemporaryDir dir; LibraryModel model;
        const auto data=dir.filePath("native");
        model.m_dataDir=data;
        const auto peerPath=peer(dir,"hello");
        QVERIFY(!peerPath.isEmpty());
        bool initialView=false;
        connect(model.m_client,&BackendClient::snapshotReady,&model,[&]{initialView=true;});
        model.m_client->start(peerPath,QString(),data,false);
        QTRY_VERIFY_WITH_TIMEOUT(initialView && model.m_client->readyForRequests() && !model.m_client->snapshotPending(),7000);
        model.m_marker={{"remoteMarkerVersion",2},{"method",int(BackendClient::Call::RemoteCommit)},{"root",data},
            {"profileId","default"},{"jobId",QString(32,'a')},{"expectedRevision","1"},{"operationId","remote-op"},{"modId","image-mod"}};
        auto expected=model.m_marker;expected.remove("recoveryId");expected.remove("recoveryExpectedRevision");
        const QString kind=repeatedIdentical?"nativeIo":"nativeBusy";
        const QString message=repeatedIdentical?"Newer read failure":"Recovery is still busy";
        model.m_remoteStage=LibraryModel::RemoteStage::RecoveryView;
        model.m_remoteOutcome={{"status","failed"},{"error",QVariantMap{{"kind",kind},{"message",message}}}};
        model.setBusy(true);
        LibrarySnapshot snapshot;
        snapshot.sourceKind="native";snapshot.revision="1";snapshot.selectedProfileId="default";
        model.applySnapshot(snapshot);
        // Persistence runs on a worker; its completion cannot be delivered
        // until this thread processes events. Inject a real helper rejection.
        const QString newerKind="nativeIo";
        const QString newerMessage="Newer read failure";
        model.m_client->rejected(BackendClient::Call::Snapshot,newerKind,newerMessage);
        QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
        QCOMPARE(model.error(),newerKind+": "+newerMessage);
        // The durable marker, recovery state and retry survive; only the error
        // text is preserved from the newer rejection.
        QCOMPARE(model.m_marker,expected);
        QVERIFY(model.m_remoteMarkerDurable);
        QVERIFY(model.m_recoveryRequired);
        QVERIFY(model.outcomeUnknown());
        QCOMPARE(model.m_remoteStage,LibraryModel::RemoteStage::Idle);
        QVERIFY(!model.busy());
        QVERIFY(model.m_remoteOutcome.isEmpty());
        QVERIFY(!model.m_remoteRecoveryTerminal);
        QFile saved(model.markerPath());QVERIFY(saved.open(QIODevice::ReadOnly));
        QCOMPARE(QJsonDocument::fromJson(saved.readAll()).object().toVariantMap(),expected);
    }
    void item126RecoveryError(bool newerFailure = false, bool repeatedFailure = false) {
        QTemporaryDir dir; LibraryModel model;
        model.m_dataDir=dir.filePath("native");
        const QString stale="nativeNotImported: No native library has been imported; review an upstream import first.";
        model.setError(stale);
        model.m_remoteStage=LibraryModel::RemoteStage::RecoveryView;
        model.setBusy(true);
        model.finishRecoveryWrapper(false,true,model.m_errorGeneration);
        // Persistence runs on a worker; its completion cannot be delivered
        // until this thread processes events. Inject a real helper rejection.
        const QString kind=repeatedFailure?"nativeNotImported":"nativeIo";
        const QString message=repeatedFailure?stale.mid(stale.indexOf(": ")+2):"A newer helper read failed.";
        if(newerFailure) model.m_client->rejected(BackendClient::Call::Snapshot,kind,message);
        QTRY_VERIFY_WITH_TIMEOUT(model.m_otherMarkerFileTasks==0,7000);
        QCOMPARE(model.error(),newerFailure?kind+": "+message:QString());
    }
    void recoveryBarrier(const QString &mode, bool explicitSource = false) {
        QTemporaryDir dir; const auto data = dir.filePath("native");
        const auto marker = QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(data.toUtf8(), QCryptographicHash::Sha256).toHex() + ".json");
        QVERIFY(QDir().mkpath(QFileInfo(marker).absolutePath()));
        QFile saved(marker); QVERIFY(saved.open(QIODevice::WriteOnly));
        saved.write(R"({"operationId":"original","method":2})"); saved.close();
        LibraryModel model;
        const auto executable = peer(dir, mode);
        if (explicitSource) model.loadSource(executable, "/missing/source", data);
        else model.loadNative(executable, data);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.busy(), 7000);
        if (mode.contains("terminal")) {
            QTRY_VERIFY_WITH_TIMEOUT(!model.outcomeUnknown(), 7000);
            QCOMPARE(model.revision(), "4"); return;
        }
        QTRY_VERIFY_WITH_TIMEOUT(model.canRecover() && !model.busy(), 7000);
        QVERIFY(model.outcomeUnknown()); QVERIFY(model.canRecover());
        QVERIFY(!model.canEditProfiles());
        QFile lock(dir.filePath("native/busy")); QVERIFY(lock.open(QIODevice::WriteOnly)); lock.close();
        QVERIFY(model.recover());
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && model.error().contains("nativeBusy"), 7000);
        QVERIFY(model.outcomeUnknown()); QVERIFY(model.error().contains("nativeBusy"));
        QVERIFY(QFile::remove(lock.fileName()));
        QVERIFY(model.recover());
        if (!mode.contains("no-doc")) {
            // Recovery confirms a clean root, but does not invent an outcome for
            // an original operation that the helper still reports as unknown.
            QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/recovered")) && !model.busy() && !model.loading(),7000);
            QVERIFY(model.outcomeUnknown()); QVERIFY(!model.canEditProfiles());
            QVERIFY(saved.open(QIODevice::ReadOnly));
            QCOMPARE(QJsonDocument::fromJson(saved.readAll()).object().value("operationId").toString(),"original");
            QCOMPARE(model.revision(),"4"); return;
        }
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && !model.loading() && !model.outcomeUnknown(), 7000);
        if (mode.contains("no-doc")) {
            QVERIFY(!model.nativeMode());
            QFile trace(dir.filePath("native/trace.jsonl")); QVERIFY(trace.open(QIODevice::ReadOnly));
            const auto lines = trace.readAll().trimmed().split('\n');
            bool originalQueriedAfterRecover = false, sawRecover = false;
            for (const auto &line : lines) {
                const auto r = QJsonDocument::fromJson(line).object();
                if (r.value("method") == "native.recover") sawRecover = true;
                if (sawRecover && r.value("method") == "native.operation" && r.value("params").toObject().value("operationId") == "original") originalQueriedAfterRecover = true;
            }
            QVERIFY(originalQueriedAfterRecover);
            QVERIFY2(model.error().contains("operationInterrupted"),qPrintable(model.error()));
        } else QCOMPARE(model.revision(), "5");
    }
    // Step 7 guards. An edit is one database commit in the helper, so after a
    // lost reply the helper's saved state is the whole truth.
    static QByteArray lostTrace(const QTemporaryDir &dir) {
        QFile trace(dir.filePath("native/trace")); return trace.open(QIODevice::ReadOnly) ? trace.readAll() : QByteArray();
    }
    void lostEditReplyShowsHelperState(bool stored) {
        QTemporaryDir dir;
        LibraryModel model; model.loadNative(peer(dir, stored ? "native-edit-lost-stored" : "native-edit-lost-unstored"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.canSetEnabled(), 7000);
        QVERIFY(model.modDetails("image-mod").value("enabled").toBool());
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/lost")) && !model.writeBusy(), 7000);
        // The model reconnects by itself; there is no outcome to check and no marker.
        QVERIFY(!model.outcomeUnknown()); QVERIFY(!model.canSetEnabled());
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && !model.busy(), 7000);
        QVERIFY(!QFile::exists(QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(model.dataDir().toUtf8(), QCryptographicHash::Sha256).toHex() + ".json")));
        QCOMPARE(model.warnings().filter("connection was lost").size(), 1);
        QVERIFY(model.warnings().first().contains("connection was lost"));
        QVERIFY(model.warnings().contains("An existing library warning."));
        // The row and revision are the helper's, whether or not it stored the change.
        QCOMPARE(model.modDetails("image-mod").value("enabled").toBool(), !stored);
        QCOMPARE(model.revision(), QString(stored ? "2" : "1"));
        QCOMPARE(lostTrace(dir).count("native.setEnabled\n"), 1);
        QVERIFY(!lostTrace(dir).contains("native.recover"));
        // Editing works again without Recover, at the helper's revision.
        QVERIFY(!model.outcomeUnknown()); QVERIFY(!model.recoveryRequired()); QVERIFY(model.canSetEnabled());
        QVERIFY(model.setModEnabled("image-mod", stored));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && model.modDetails("image-mod").value("enabled").toBool() == stored, 7000);
        QCOMPARE(model.revision(), QString(stored ? "3" : "2"));
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(!lostTrace(dir).contains("native.recover"));
    }
    void lostToggleReplyStillReachesAutomaticApply(bool stored) {
        QTemporaryDir dir;
        LibraryModel model; model.setAutomaticApply(true); model.loadNative(peer(dir, stored ? "native-edit-lost-stored" : "native-edit-lost-unstored"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.canSetEnabled(), 7000);
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/lost")) && !model.writeBusy(), 7000);
        // The helper's state is reviewed for Apply at its revision. A toggle it
        // never stored is dropped, not sent again.
        QTRY_VERIFY_WITH_TIMEOUT(!model.applyReview().isEmpty(), 9000);
        QCOMPARE(model.revision(), QString(stored ? "2" : "1"));
        QCOMPARE(model.modDetails("image-mod").value("enabled").toBool(), !stored);
        QCOMPARE(model.warnings().filter("not saved").size(), stored ? 0 : 1);
        QVERIFY(model.m_autoToggles.isEmpty()); QVERIFY(model.m_autoInFlight.isEmpty());
        const auto trace = lostTrace(dir);
        QCOMPARE(trace.count("native.setEnabled\n"), 1);
        QVERIFY(trace.lastIndexOf("native.applyPreview\n") > trace.indexOf("native.setEnabled\n"));
        QVERIFY(!trace.contains("native.apply\n")); QVERIFY(!trace.contains("native.recover"));
        QVERIFY(!model.outcomeUnknown()); QVERIFY(!model.recoveryRequired());
    }
    // Edits half of the marker-persistence split; transactions stay in
    // local_marker_persistence_failure_dispatches_no_commit. An edit has no marker.
    void lostEditSnapshotFailureKeepsWritesClosed() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir, "native-edit-lost-stored-snapshot-error"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.canSetEnabled(), 7000);
        QVERIFY(model.canPreviewApply());
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(model.error().contains("Snapshot temporarily unavailable") && !model.loading() && !model.busy(), 7000);
        QVERIFY(model.m_editLost);
        QVERIFY(!model.outcomeUnknown());
        QVERIFY(!model.canPreviewApply());
        QVERIFY(!model.canSetEnabled());
        QVERIFY(!model.canRestore());
        QVERIFY(!model.remoteAdmissionAvailable());
        QVERIFY(!model.fontsAvailable());
        QVERIFY(!model.profileEditsAvailable());
        QVERIFY(!model.launch()->canLaunch());
        QFile allow(dir.filePath("native/allow-snapshot")); QVERIFY(allow.open(QIODevice::WriteOnly)); allow.close();
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.busy() && model.canSetEnabled(), 7000);
        QVERIFY(!model.m_editLost);
        QCOMPARE(model.revision(), QString("2"));
        QVERIFY(!model.modDetails("image-mod").value("enabled").toBool());
        QVERIFY(model.canPreviewApply());
        QCOMPARE(lostTrace(dir).count("native.setEnabled\n"), 1);
        QVERIFY(!lostTrace(dir).contains("native.recover"));
    }
    void lostEditRemainingWriteGates() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(helper(), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.m_client->readyForRequests(), 7000);
        model.m_nativeMode=true; model.m_demoMode=false; model.m_viewedProfileId="default";
        model.m_activeProfileId="default"; model.m_busy=false; model.m_loading=false;
        model.m_outcomeUnknown=false; model.m_recoveryRequired=false; model.m_marker.clear();
        auto *launch=model.m_launch;
        launch->m_binding={}; launch->m_barriers={};
        launch->m_terminalRead=false; launch->m_barriersReadNeeded=false;
        launch->m_fileBusy=false; launch->m_pending.reset(); launch->m_gameRunningLock=false;
        launch->m_shuttingDown=false; launch->m_settingsRevision="1";
        QVERIFY(model.profileEditsAvailable()); QVERIFY(launch->canLaunch());
        BackendClient prepared;
        prepared.start(peer(dir,"native-local"),{},model.dataDir(),true,true);
        QTRY_VERIFY_WITH_TIMEOUT(prepared.readyForRequests(),7000);
        model.m_editLost=true;
        QVERIFY(!model.profileEditsAvailable()); QVERIFY(!launch->canLaunch());
        QVERIFY(!model.dispatchPreparationCommit(&prepared,"default",model.revision(),{},"",BackendClient::Call::LocalCommit));
        QVERIFY(model.m_marker.isEmpty());
        model.m_editLost=false;
        QVERIFY(model.profileEditsAvailable()); QVERIFY(launch->canLaunch());
    }
    void lostEditSourceSwitchCompletes(bool automatic = false) {
        QTemporaryDir dir; LibraryModel model;
        model.setAutomaticApply(automatic);
        model.loadNative(peer(dir, "native-edit-lost-stored-held"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.canSetEnabled(), 7000);
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/held")), 7000);
        if(automatic) QVERIFY(model.setModEnabled("image-mod", true));
        model.loadDemoData(3);
        QVERIFY(model.m_switching);
        QFile release(dir.filePath("native/release")); QVERIFY(release.open(QIODevice::WriteOnly)); release.close();
        QTRY_VERIFY_WITH_TIMEOUT(model.demoMode() && !model.m_switching, 7000);
        QCOMPARE(model.totalCount(), 3);
        QVERIFY(!model.m_autoCycle); QVERIFY(!model.m_autoBlocked);
        QVERIFY(model.m_autoToggles.isEmpty()); QVERIFY(model.m_autoInFlight.isEmpty());
        QVERIFY(model.m_marker.isEmpty()); QVERIFY(!model.outcomeUnknown());
        QCOMPARE(lostTrace(dir).count("native.setEnabled\n"), 1);
        QVERIFY(!lostTrace(dir).contains("native.recover"));
        QTemporaryDir next;
        model.loadNative(peer(next,"native-edit-ok"),next.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.canSetEnabled(),7000);
        model.setAutomaticApply(false);
        QVERIFY(model.setModEnabled("image-mod",false));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && model.revision()=="2",7000);
        QCOMPARE(lostTrace(dir).count("native.setEnabled\n"),1);
    }
    void lostEditKeepsLocalStatusAndCancel() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir,"native-local-slow"),dir.filePath("native"));
        auto *local=model.localImport(); local->setVisible(true);
        QTRY_VERIFY_WITH_TIMEOUT(local->canPrepare(),3000);
        QVERIFY(local->prepare({QUrl::fromLocalFile("/synthetic/a.vpk")}));
        QTRY_VERIFY_WITH_TIMEOUT(!local->jobId().isEmpty() && local->phase()=="hashing",3000);
        model.m_editLost=true; emit model.nativeChanged();
        const auto polls=[&]{QFile trace(dir.filePath("native/trace.jsonl"));return trace.open(QIODevice::ReadOnly) ? trace.readAll().count("native.local.status") : 0;};
        const auto before=polls();
        QTRY_VERIFY_WITH_TIMEOUT(polls()>before,2000);
        QVERIFY(!local->canPrepare()); QVERIFY(!local->canCommit());
        QVERIFY(local->canCancelJob(local->jobId()));
        QVERIFY(local->cancelJob(local->jobId()));
        QTRY_VERIFY_WITH_TIMEOUT(local->phase()=="cancelled",3000);
        QVERIFY(model.m_editLost);
    }
    void edit_saves_without_marker_storage() {
        QTemporaryDir dir; LibraryModel model; model.loadNative(peer(dir, "native-edit-ok"), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.canSetEnabled(), 7000);
        const auto marker = QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(model.dataDir().toUtf8(), QCryptographicHash::Sha256).toHex() + ".json");
        QVERIFY(QDir().mkpath(marker)); QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy(), 3000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QCOMPARE(model.revision(), QString("2")); QVERIFY(!model.modDetails("image-mod").value("enabled").toBool());
        QCOMPARE(lostTrace(dir).count("native.setEnabled\n"), 1);
        QVERIFY(QDir().rmdir(marker));
    }
    // Transactions half of the disconnected-writer split: Restore keeps its marker.
    void disconnectedTransactionKeepsMarkerAndReconciles(const QString &mode = "native-restore-lost") {
        QTemporaryDir dir; const auto executable = peer(dir, mode);
        const auto data = dir.filePath("native");
        {
            LibraryModel model; model.loadNative(executable, data);
            QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && model.canRestore(), 7000);
            QVERIFY(model.restorePrevious());
            QTRY_VERIFY_WITH_TIMEOUT(model.outcomeUnknown(), 7000);
            QVERIFY(!model.canSetEnabled());
            const auto marker = QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(data.toUtf8(), QCryptographicHash::Sha256).toHex() + ".json");
            QFile file(marker); QVERIFY(file.open(QIODevice::ReadOnly));
            QVERIFY(!QJsonDocument::fromJson(file.readAll()).object().value("operationId").toString().isEmpty());
        }
        LibraryModel reopened; reopened.loadNative(executable, data);
        QTRY_VERIFY_WITH_TIMEOUT(!reopened.loading() && !reopened.busy() && !reopened.outcomeUnknown(), 7000);
        QCOMPARE(reopened.revision(), "2");
        QCOMPARE(lostTrace(dir).count("native.restore\n"), 1);
        QVERIFY(lostTrace(dir).contains("native.operation\n"));
    }
    // Edits half of the same split: no marker and no outcome to check. The
    // model waits for the old writer, reconnects and shows what the helper saved.
    void disconnectedEditReloadsHelperState(const QString &mode = "native-disconnect") {
        QTemporaryDir dir; const auto data = dir.filePath("native");
        LibraryModel model; model.loadNative(peer(dir, mode), data);
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model), 7000);
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(dir.filePath("native/dispatches")) && !model.writeBusy(), 7000);
        QVERIFY(!model.outcomeUnknown()); QVERIFY(!model.canSetEnabled());
        QVERIFY(!QFile::exists(QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(data.toUtf8(), QCryptographicHash::Sha256).toHex() + ".json")));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && !model.busy(), 7000);
        QVERIFY(QFile::exists(dir.filePath("native/finished.json")));
        QCOMPARE(model.revision(), "2");
        QVERIFY(!model.modDetails("image-mod").value("enabled").toBool());
        QVERIFY(!model.outcomeUnknown()); QVERIFY(model.canSetEnabled());
        QCOMPARE(model.warnings().filter("connection was lost").size(), 1);
        QFile trace(dir.filePath("native/dispatches")); QVERIFY(trace.open(QIODevice::ReadOnly));
        QCOMPARE(trace.readAll(), QByteArray("write\n"));
    }
    void refreshDiscoversExternalImport(bool explicitSource) {
        QTemporaryDir dir;
        const auto data = dir.filePath("native");
        // Isolate default discovery for both helpers and never read the real source.
        const auto oldXdg = qgetenv("XDG_DATA_HOME");
        struct RestoreEnv { QByteArray value; ~RestoreEnv() { if (value.isNull()) qunsetenv("XDG_DATA_HOME"); else qputenv("XDG_DATA_HOME", value); } } restore{oldXdg};
        qputenv("XDG_DATA_HOME", dir.path().toUtf8());
        const auto source = dir.filePath("dev.stormix.deadlock-mod-manager/state.json");
        QVERIFY(QDir().mkpath(QFileInfo(source).absolutePath()));
        QFile file(source); QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(R"({"local-config":{"version":33,"state":{"activeProfileId":"upstream-only","profiles":{"upstream-only":{"name":"Upstream","mods":[]}}}}})"); file.close();
        const auto importedSource = dir.filePath("external-import.json");
        QFile imported(importedSource); QVERIFY(imported.open(QIODevice::WriteOnly));
        imported.write(R"({"local-config":{"version":33,"state":{"activeProfileId":"default","profiles":{"default":{"name":"Default","mods":[]}}}}})"); imported.close();
        // HOME controls the higher-priority Flatpak path as well.
        const auto oldHome = qgetenv("HOME");
        struct RestoreHome { QByteArray value; ~RestoreHome() { qputenv("HOME", value); } } restoreHome{oldHome};
        qputenv("HOME", dir.path().toUtf8());
        LibraryModel model;
        if (explicitSource) model.loadSource(helper(), source, data); else model.loadNative(helper(), data);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error())); QVERIFY(!model.nativeMode());
        // Import through a separate helper, then refresh the GUI's source view.
        QProcess importer; importer.start(helper()); QVERIFY(importer.waitForStarted());
        qint64 requestId=0;
        const auto rpc=[&](const QString &method,QJsonObject params) {
            params.insert("dataDir",data);
            importer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",++requestId},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n');
            QVERIFY(importer.waitForBytesWritten());
            while(!importer.canReadLine()) QVERIFY(importer.waitForReadyRead(7000));
            const auto response=QJsonDocument::fromJson(importer.readLine()).object();
            QVERIFY2(!response.contains("error"),QJsonDocument(response).toJson().constData());
            return response.value("result").toObject();
        };
        const auto review=rpc("native.importPreview",{{"statePath",importedSource},{"gamePath",dir.path()}});
        const auto importedSnapshot=rpc("native.importCommit",{{"token",review.value("token")},{"operationId",QString(32,'a')},{"expectedRevision","0"}});
        QCOMPARE(importedSnapshot.value("sourceKind").toString(),"native");
        importer.closeWriteChannel(); QVERIFY(importer.waitForFinished());
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(model.warnings().isEmpty());
        QCOMPARE(model.viewedProfileId(), explicitSource ? QString("upstream-only") : QString("default"));
        QCOMPARE(model.nativeMode(), !explicitSource);
        QCOMPARE(model.revision(), explicitSource ? QString("0") : QString("1"));
    }
    void pendingStartupCannotReplaceDemo() {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir, "native-delay"), dir.filePath("native"));
        model.loadDemoData(3); waitEvents(300);
        QVERIFY(model.demoMode()); QCOMPARE(model.totalCount(), 3); QVERIFY(!model.nativeMode());
    }
    void nativeCapabilityAndMalformedContracts(const QString &mode) {
        QTemporaryDir dir; LibraryModel model;
        model.loadNative(peer(dir, mode), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        if (mode == "native-read-only") {
            QCOMPARE(model.error(),QString("The app and helper do not match. Reinstall Modlock so both come from the same build."));
            QVERIFY(!model.m_client->readyForRequests());
            QVERIFY(!model.canSetEnabled()); QVERIFY(!model.setModEnabled("image-mod", false));
        } else if (mode == "native-delay") {
            QTRY_VERIFY_WITH_TIMEOUT(model.canPreviewApply() && model.canEditProfiles(),3000);
        } else QVERIFY(!model.error().isEmpty());
    }
    void slowReviewCanBeCancelledWithoutAWrite() {
        QTemporaryDir dir; LibraryModel model;
        model.loadSource(peer(dir, "native-slow-preview"), "/synthetic/state", dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(model.previewImport("/synthetic/state", "/synthetic/game"));
        waitEvents(300); QVERIFY(model.busy()); QVERIFY(!model.writeBusy());
        bool cancelled = false;
        QVERIFY(QMetaObject::invokeMethod(&model, "cancelPreview", Q_RETURN_ARG(bool, cancelled)));
        QVERIFY(cancelled);
        QTRY_VERIFY_WITH_TIMEOUT(!model.busy() && !model.loading(), 7000);
        QVERIFY(model.error().isEmpty()); QVERIFY(model.importReview().isEmpty());
        QVERIFY(!QFile::exists(dir.filePath("native/library.sqlite3")));
    }
    void failedRecoveryContextReadKeepsPendingIdentity() {
        QTemporaryDir dir; const auto data = dir.filePath("native");
        const auto marker = QDir(AppPaths::stateRoot()).filePath("pending/" + QCryptographicHash::hash(data.toUtf8(), QCryptographicHash::Sha256).toHex() + ".json");
        QVERIFY(QDir().mkpath(QFileInfo(marker).absolutePath()));
        QFile file(marker); QVERIFY(file.open(QIODevice::WriteOnly)); file.write(R"({"operationId":"original","method":2})"); file.close();
        LibraryModel model; model.loadSource(peer(dir, "recovery-store-error"), "/missing/source", data);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading() && !model.busy(), 7000);
        QTRY_VERIFY_WITH_TIMEOUT(model.error().contains("nativeDatabase"), 7000);
        QVERIFY(model.outcomeUnknown());
        QVERIFY(file.open(QIODevice::ReadOnly));
        QCOMPARE(QJsonDocument::fromJson(file.readAll()).object().value("operationId").toString(), "original");
    }
    void notificationsFollowAuthoritativeState() {
        QTemporaryDir dir; LibraryModel model; QVariantMap notified; int notifications = 0;
        connect(&model, &LibraryModel::nativeChanged, this, [&] {
            ++notifications;
            notified = {{"loading", model.loading()}, {"canImport", model.canImport()}, {"profile", model.viewedProfileId()}};
        });
        model.loadSource(helper(), fixture(), dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QVERIFY(!notified.value("loading").toBool()); QVERIFY(notified.value("canImport").toBool());
        QCOMPARE(notified.value("profile").toString(), "default");
        const auto beforeRefresh = notifications; model.refresh();
        QVERIFY(notifications > beforeRefresh); QVERIFY(notified.value("loading").toBool());
        QVERIFY(!notified.value("canImport").toBool());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(notified.value("canImport").toBool());
        model.viewProfile("other");
        QVERIFY(notified.value("loading").toBool()); QVERIFY(!notified.value("canImport").toBool());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(!notified.value("loading").toBool()); QVERIFY(notified.value("canImport").toBool());
        QCOMPARE(notified.value("profile").toString(), "other");
    }
    void defaults() {
        LibraryModel model;
        QCOMPARE(model.totalCount(), 0);
        QVERIFY(model.readOnly());
        QVERIFY(!model.demoMode());
        QVERIFY(model.selectedId().isEmpty());
        QVERIFY(model.selectedMod().isEmpty());
    }
    void demoSelectionFilteringAndCounts() {
        LibraryModel model;
        int total = 0, categories = 0;
        connect(&model, &LibraryModel::totalCountChanged, this, [&] { ++total; });
        connect(&model, &LibraryModel::categoriesChanged, this, [&] { ++categories; });
        model.loadDemoData(12);
        QVERIFY(!model.readOnly());
        QVERIFY(model.demoMode());
        QCOMPARE(model.totalCount(), 12);
        QCOMPARE(model.enabledCount(), 9);
        QCOMPARE(model.categories().size(), 13);
        QVERIFY(model.categories().contains("Vindicta"));
        QVERIFY(total > 0 && categories > 0);
        model.setSelectedId("demo_2");
        model.setQuery("does not exist");
        QCOMPARE(model.count(), 0);
        QVERIFY(model.selectedId().isEmpty());
        QVERIFY(model.selectedMod().isEmpty());
        model.setQuery("");
        model.setSelectedId("demo_2");
        model.setCategory("Sounds");
        QCOMPARE(model.selectedId(), "demo_2");
        QCOMPARE(model.count(), 4);
        model.loadDemoData(12);
        QCOMPARE(model.selectedId(), "demo_2");
        QCOMPARE(model.selectedMod().value("id").toString(), "demo_2");
        QVERIFY(model.selectedMod().contains("images"));
        QVERIFY(model.selectedMod().value("images").toStringList().isEmpty());
        QVERIFY(!model.selectedMod().value("previewHidden").toBool());
        QVERIFY(model.setModEnabled("demo_2", false));
        QCOMPARE(model.enabledCount(), 8);
        QVERIFY(!model.selectedMod().value("enabled").toBool());
        model.setEnabledOnly(true);
        QVERIFY(model.selectedId() != "demo_2");
        QVERIFY(!model.setModEnabled("missing", true));
        QVERIFY(model.idAt(-1).isEmpty());
        model.loadDemoData(0);
        QCOMPARE(model.totalCount(), 0);
        QVERIFY(model.categories().isEmpty());
        QVERIFY(model.selectedMod().isEmpty());
    }
    void share_export_survives_background_notifications() {
        QTemporaryDir dir; QVERIFY(dir.isValid()); const auto data=dir.filePath("native");
        QProcess initializer; initializer.start(helper()); QVERIFY(initializer.waitForStarted());
        initializer.write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.library.initialize"},{"params",QJsonObject{{"version",1},{"dataDir",data},{"operationId",QString(32,'a')},{"expectedRevision","0"}}}}).toJson(QJsonDocument::Compact)+'\n');
        QVERIFY(initializer.waitForBytesWritten()); while(!initializer.canReadLine()) QVERIFY(initializer.waitForReadyRead(5000));
        const auto response=QJsonDocument::fromJson(initializer.readLine()).object(); QVERIFY(!response.contains("error")); initializer.closeWriteChannel(); QVERIFY(initializer.waitForFinished());
        LibraryModel model; model.loadNative(helper(),data);
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && !model.loading() && !model.busy(),10000);
        ProfileShareController controller; controller.setLibrary(&model); controller.exportProfile("default",false);
        QTRY_VERIFY_WITH_TIMEOUT(!controller.result().value("code").toString().isEmpty(),10000);
        const auto code=controller.result().value("code");
        QMetaObject::invokeMethod(&model,"nativeChanged");
        QCOMPARE(controller.result().value("code"),code);
        model.refresh(); QTRY_VERIFY_WITH_TIMEOUT(!model.loading(),10000);
        QCOMPARE(controller.result().value("code"),code);
    }
    void detected_hero_transport() {
        QTemporaryDir dir;LibraryModel model;int mutations=0;
        connect(model.m_client,&BackendClient::completed,this,[&](BackendClient::Call){++mutations;});
        model.loadNative(peer(dir,"native-local-hero"),dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(model.nativeMode() && model.modDetails("image-mod").value("hero").toString()=="Grey Talon",7000);
        model.setCategory("Grey Talon");QCOMPARE(model.count(),1);
        QCOMPARE(mutations,0);QVERIFY(model.error().isEmpty());
    }
    void detected_library_heroes() {
        LibraryModel model;model.loadDemoData(3);
        auto *rows=static_cast<LibraryRows *>(model.sourceModel());
        LibraryMod local;local.id="local";local.title="Imported";local.kind="Other";local.heroDetectionEligible=true;
        LibraryMod remote;remote.id="remote";remote.hero="Ivy";remote.kind="Skins";
        rows->replace({local,remote});model.m_nativeMode=true;model.m_revision="7";model.m_heroRevision="7";
        model.m_client->heroReady({{"modId","local"},{"revision","6"},{"heroDisplay","Abrams"}});
        QCOMPARE(rows->details("local").value("hero").toString(),QString());
        model.m_client->heroReady({{"modId","local"},{"revision","7"},{"heroDisplay","Grey Talon"}});
        QCOMPARE(rows->details("local").value("hero").toString(),QString("Grey Talon"));
        QVERIFY(rows->categories().contains("Grey Talon"));
        model.setCategory("Grey Talon");QCOMPARE(model.count(),1);
        model.setCategory("");model.setQuery("Grey Talon");QCOMPARE(model.count(),1);
        model.m_client->heroReady({{"modId","remote"},{"revision","7"},{"heroDisplay","Abrams"}});
        QCOMPARE(rows->details("remote").value("hero").toString(),QString("Ivy"));
    }
    void library_metadata_is_strict_and_available_in_roles() {
        auto object = previewSnapshot();
        auto mods = object["mods"].toArray(); auto mod = mods[0].toObject();
        mod["addedAt"] = "2026-11-01T22:00:02Z"; mod["updatedAt"] = QJsonValue::Null;
        mod["sizeBytes"] = 123456; mod["variantName"] = "Blue"; mod["fileName"] = "blue_dir.vpk"; mod["authorId"] = "123";
        mods[0] = mod; object["mods"] = mods;
        LibrarySnapshot snapshot; QVERIFY(LibrarySnapshot::decode(object, snapshot));
        LibraryRows rows; rows.replace(snapshot.mods);
        QCOMPARE(rows.details("image-mod").value("sizeBytes").toInt(), 123456);
        QCOMPARE(rows.data(rows.index(0), LibraryRows::AuthorId).toString(), QString("123"));
        QCOMPARE(rows.data(rows.index(0), LibraryRows::AddedAt).toString(), QString("2026-11-01T22:00:02Z"));
        for (const auto &bad : {QJsonValue(-1), QJsonValue(1.5), QJsonValue("12"), QJsonValue(9007199254740992.0)}) {
            auto invalid = mod; invalid["sizeBytes"] = bad; mods[0] = invalid; object["mods"] = mods;
            QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        }
        for (const auto *field : {"addedAt", "updatedAt", "authorId", "variantName", "fileName"}) {
            auto invalid = mod; invalid[field] = "bad\nmetadata"; mods[0] = invalid; object["mods"] = mods;
            QVERIFY(!LibrarySnapshot::decode(object, snapshot));
        }
    }
    void previewDecodeAndRoles() {
        LibrarySnapshot snapshot;
        const QStringList expected{"https://example.invalid/first.png", "https://example.invalid/second.jpg?size=640"};
        QVERIFY(LibrarySnapshot::decode(previewSnapshot(QJsonArray::fromStringList(expected), true), snapshot));
        QCOMPARE(snapshot.mods.size(), 1);
        const auto details = snapshot.mods.first().details();
        QCOMPARE(details.value("images").toStringList(), expected);
        QVERIFY(details.value("previewHidden").toBool());
        LibraryRows rows; rows.replace(snapshot.mods);
        const int imageRole = rows.roleNames().key("modPreview", -1);
        const int hiddenRole = rows.roleNames().key("modPreviewHidden", -1);
        QVERIFY(imageRole >= Qt::UserRole && hiddenRole >= Qt::UserRole);
        QCOMPARE(rows.data(rows.index(0), imageRole).toString(), "https://example.invalid/first.png");
        QVERIFY(rows.data(rows.index(0), hiddenRole).toBool());
        QVERIFY(LibrarySnapshot::decode(previewSnapshot(), snapshot));
        const auto legacy = snapshot.mods.first().details();
        QVERIFY(legacy.contains("images"));
        QVERIFY(legacy.value("images").toStringList().isEmpty());
        QVERIFY(!legacy.value("previewHidden").toBool());
        rows.replace(snapshot.mods);
        QVERIFY(rows.data(rows.index(0), imageRole).toString().isEmpty());
        QVERIFY(!rows.data(rows.index(0), hiddenRole).toBool());
    }
    void previewInvalidMetadataDoesNotBreakMods() {
        const QString longest = QStringLiteral("https://example.invalid/") + QString(4072, 'a');
        QCOMPARE(longest.size(), 4096);
        QJsonArray images{"http://example.invalid/no.png", "file:///tmp/no.png", "data:image/png;base64,bad", "javascript:bad()",
            "https://", "https:///no-host.png", "https://user:pass@example.invalid/no.png", "https://@example.invalid/no.png",
            "https://example.invalid:bad/no.png", "https://example.invalid/%zz.png", "https://example.invalid/control\n.png",
            12, QJsonValue(), longest + 'a', "https://example.invalid/first.png", "https://example.invalid/first.png", longest};
        LibrarySnapshot snapshot;
        QVERIFY(LibrarySnapshot::decode(previewSnapshot(images, "invalid"), snapshot));
        QCOMPARE(snapshot.mods.size(), 1);
        const auto details = snapshot.mods.first().details();
        QCOMPARE(details.value("images").toStringList(), (QStringList{"https://example.invalid/first.png", longest}));
        QCOMPARE(details.value("title").toString(), "Artwork");
        QVERIFY(!details.value("previewHidden").toBool());
        for (const auto &invalid : {QJsonValue("https://example.invalid/not-an-array"), QJsonValue(42), QJsonValue()}) {
            QVERIFY(LibrarySnapshot::decode(previewSnapshot(invalid), snapshot));
            QVERIFY(snapshot.mods.first().details().value("images").toStringList().isEmpty());
        }
        images = QJsonArray{QJsonValue(), "file:///tmp/no.png"};
        for (int i = 0; i < 40; ++i) {
            const auto url = QStringLiteral("https://example.invalid/%1.png").arg(i);
            images.append(url); images.append(url);
        }
        QVERIFY(LibrarySnapshot::decode(previewSnapshot(images), snapshot));
        const auto bounded = snapshot.mods.first().details().value("images").toStringList();
        QCOMPARE(bounded.size(), 32);
        QCOMPARE(bounded.first(), "https://example.invalid/0.png");
        QCOMPARE(bounded.last(), "https://example.invalid/31.png");
    }
    void helperPreviewSelectionAndRefresh() {
        QTemporaryDir dir;
        const auto path = dir.filePath("state.json");
        QFile fixtureFile(fixture()); QVERIFY(fixtureFile.open(QIODevice::ReadOnly));
        auto document = QJsonDocument::fromJson(fixtureFile.readAll());
        auto rootObject = document.object();
        auto config = rootObject.value("local-config").toObject();
        auto state = config.value("state").toObject();
        auto mods = state.value("localMods").toArray();
        auto mod = mods.first().toObject();
        mod.insert("images", QJsonArray{"https://example.invalid/actual.png", "https://example.invalid/gallery.png"});
        mod.insert("isNSFW", true); mods[0] = mod;
        state.insert("localMods", mods); config.insert("state", state); rootObject.insert("local-config", config);
        const auto original = QJsonDocument(rootObject).toJson();
        const auto write = [&](const QByteArray &bytes) {
            QFile file(path); QVERIFY(file.open(QIODevice::WriteOnly | QIODevice::Truncate));
            QCOMPARE(file.write(bytes), bytes.size());
        };
        write(original);
        const auto modified = QFileInfo(path).lastModified();
        LibraryModel model; model.loadSource(helper(), path);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        model.setSelectedId("mod_active");
        QCOMPARE(model.selectedMod().value("images").toStringList(), (QStringList{"https://example.invalid/actual.png", "https://example.invalid/gallery.png"}));
        QVERIFY(model.selectedMod().value("previewHidden").toBool());
        const int imageRole = model.roleNames().key("modPreview", -1);
        QCOMPARE(model.data(model.index(0, 0), imageRole).toString(), "https://example.invalid/actual.png");
        QFile source(path); QVERIFY(source.open(QIODevice::ReadOnly));
        QCOMPARE(source.readAll(), original); source.close();
        QCOMPARE(QFileInfo(path).lastModified(), modified);
        model.setSelectedId("mod_disabled");
        QVERIFY(model.selectedMod().value("images").toStringList().isEmpty());
        model.setSelectedId("mod_active");
        int selectionUpdates = 0;
        connect(&model, &LibraryModel::selectionChanged, this, [&] { ++selectionUpdates; });
        mod.insert("images", QJsonArray{"https://example.invalid/refreshed.png"});
        mod.insert("isNSFW", false); mods[0] = mod;
        state.insert("localMods", mods); config.insert("state", state); rootObject.insert("local-config", config);
        write(QJsonDocument(rootObject).toJson());
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.selectedId(), "mod_active");
        QCOMPARE(model.selectedMod().value("images").toStringList(), (QStringList{"https://example.invalid/refreshed.png"}));
        QVERIFY(!model.selectedMod().value("previewHidden").toBool());
        QVERIFY(selectionUpdates > 0);
    }
    void actualHelperAndRefresh() {
        QTemporaryDir dir;
        const auto path = dir.filePath("state.json");
        QVERIFY(QFile::copy(fixture(), path));
        QFile source(path); QVERIFY(source.open(QIODevice::ReadOnly));
        const auto original = source.readAll(); source.close();
        const auto modified = QFileInfo(path).lastModified();
        LibraryModel model;
        model.loadSource(helper(), path);
        QVERIFY(model.loading());
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QCOMPARE(model.totalCount(), 4);
        QCOMPARE(model.enabledCount(), 1);
        QCOMPARE(model.activeProfileId(), "default");
        QCOMPARE(model.viewedProfileId(), "default");
        QCOMPARE(model.sourcePath(), path);
        QCOMPARE(model.profiles().size(), 2);
        QVERIFY(!model.warnings().isEmpty());
        QCOMPARE(model.modDetails("mod_active").value("hero").toString(), "Ivy");
        QVERIFY(model.modDetails("stale").isEmpty());
        QCOMPARE(model.modDetails("mod_active").value("installedVpks").toStringList(), QStringList{"pak01_dir.vpk"});
        QVERIFY(!model.setModEnabled("mod_active", false));
        QCOMPARE(model.enabledCount(), 1);
        QCOMPARE(model.modDetails("mod_future").value("status").toString(), "future-status");
        QVERIFY(!model.modDetails("mod_future").value("enabled").toBool());
        QVERIFY(!model.modDetails("mod_transient").value("enabled").toBool());
        model.setSelectedId("mod_active");
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.selectedId(), "mod_active");
        model.viewProfile("other");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.viewedProfileId(), "other");
        QCOMPARE(model.activeProfileId(), "default");
        QCOMPARE(model.totalCount(), 1);
        QCOMPARE(model.idAt(0), "inactive");
        model.viewProfile("missing");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(model.error().contains("unknownProfile"));
        QCOMPARE(model.viewedProfileId(), "other");
        QCOMPARE(model.idAt(0), "inactive");
        model.viewProfile("default");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(model.error().isEmpty());
        QVERIFY(source.open(QIODevice::ReadOnly));
        QCOMPARE(source.readAll(), original);
        QCOMPARE(QFileInfo(path).lastModified(), modified);
    }
    void sourceErrors(const QByteArray &bytes, const QString &kind) {
        QTemporaryDir dir; const auto path = dir.filePath("state.json");
        QFile file(path); QVERIFY(file.open(QIODevice::WriteOnly)); file.write(bytes); file.close();
        LibraryModel model; model.loadSource(helper(), path);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().contains(kind), qPrintable(model.error()));
        QCOMPARE(model.totalCount(), 0);
    }
    void transport(const QString &mode, bool success) {
        QTemporaryDir dir; LibraryModel model;
        QTimer heartbeat; heartbeat.setInterval(10); int beats = 0;
        connect(&heartbeat, &QTimer::timeout, this, [&] { ++beats; }); heartbeat.start();
        model.loadSource(peer(dir, mode), "/never/read");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.error().isEmpty(), success);
        if (mode == "timeout") QVERIFY(beats > 100);
        QVERIFY(!model.error().contains("SYNTHETIC_STDERR_SENTINEL"));
        QCOMPARE(model.totalCount(), 0);
        if (success) QCOMPARE(model.sourcePath(), "/synthetic/state.json");
    }
    void missingHelperAndSource() {
        LibraryModel model; model.loadSource("/missing/helper", "/missing/state");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(!model.error().isEmpty());
        model.loadSource(helper(), "/missing/state");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY(model.error().contains("sourceNotFound"));
    }
    void emptyStoreAndFreshRefresh() {
        QTemporaryDir dir; const auto path = dir.filePath("state.json");
        const auto write = [&](const QByteArray &bytes) {
            QFile file(path); QVERIFY(file.open(QIODevice::WriteOnly | QIODevice::Truncate));
            QCOMPARE(file.write(bytes), bytes.size());
        };
        QVERIFY(QFile::copy(fixture(), path));
        LibraryModel model; model.loadSource(helper(), path);
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.totalCount(), 4);
        write(R"({"local-config":{"version":33,"state":{"profiles":{}}}})");
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QCOMPARE(model.totalCount(), 0);
        QVERIFY(model.profiles().isEmpty());
        QVERIFY(model.activeProfileId().isEmpty());
        QVERIFY(model.viewedProfileId().isEmpty());
        QVERIFY(model.selectedMod().isEmpty());
        QVERIFY(model.categories().isEmpty());
        write(R"({"local-config":{"version":33,"state":{"activeProfileId":"default","profiles":{"default":{"mods":[{}, {"id":"valid","remoteId":"1","status":"installed"}]}}}}})");
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QVERIFY2(model.error().isEmpty(), qPrintable(model.error()));
        QCOMPARE(model.totalCount(), 1);
        QCOMPARE(model.idAt(0), "valid");
        QVERIFY(!model.warnings().isEmpty());
    }
    void childStopsOnDestruction() {
        QTemporaryDir dir; qint64 pid = 0;
        {
            LibraryModel model;
            model.loadSource(peer(dir, "partial"), "/never/read");
            QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
            const auto children = model.findChildren<QProcess *>();
            QVERIFY(!children.isEmpty());
            pid = children.first()->processId(); QVERIFY(pid > 0);
        }
        QTRY_VERIFY_WITH_TIMEOUT(::kill(pid, 0) == -1 && errno == ESRCH, 1000);
    }
    void supersededProfileReplies() {
        QTemporaryDir dir; LibraryModel model;
        model.loadSource(peer(dir, "race"), "/never/read");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        model.viewProfile("slow");
        model.viewProfile("fast");
        QTRY_VERIFY_WITH_TIMEOUT(!model.loading(), 7000);
        QCOMPARE(model.viewedProfileId(), "fast");
        waitEvents(400);
        QCOMPARE(model.viewedProfileId(), "fast");
        QVERIFY(model.error().isEmpty());
        model.viewProfile("slow");
        model.loadDemoData(3);
        waitEvents(400);
        QVERIFY(model.demoMode());
        QCOMPARE(model.totalCount(), 3);
    }
    void item53_shared_local_helper_stop() {
        QTemporaryDir dir; QVERIFY(dir.isValid());
        const auto peerPath = dir.filePath("peer");
        QVERIFY(QFile::link(root + "/tests/helper-stop-peer.py", peerPath));
        LibraryModel model;
        model.loadNative(peerPath, dir.filePath("native"));
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.nativeMode(), 7000);
        QVERIFY(!model.helperStopped());
        QVERIFY(model.stopReason().isEmpty());

        QFile spoof(dir.filePath("spoof-wire-error"));
        QVERIFY(spoof.open(QIODevice::WriteOnly)); spoof.close();
        QVERIFY(model.setModEnabled("image-mod", false));
        QTRY_VERIFY_WITH_TIMEOUT(model.error().contains("helperStopped"), 5000);
        QVERIFY(!model.helperStopped());
        QVERIFY(model.stopReason().isEmpty());
        QVERIFY(model.m_client->isRunning());
        QVERIFY(model.m_client->readyForRequests());
        QVERIFY(QFile::remove(dir.filePath("spoof-wire-error")));

        QFile failMethod(dir.filePath("fail-on-method"));
        QVERIFY(failMethod.open(QIODevice::WriteOnly | QIODevice::Truncate));
        failMethod.write("native.restore"); failMethod.close();
        QVERIFY(model.restorePrevious());
        QTRY_VERIFY_WITH_TIMEOUT(model.helperStopped() && model.outcomeUnknown(), 7000);
        QVERIFY(!model.stopReason().isEmpty());
        QVERIFY(!model.m_client->isRunning());
        QVERIFY(QFile::remove(dir.filePath("fail-on-method")));

        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && !model.helperStopped() && !model.outcomeUnknown(), 7000);
        QVERIFY(model.stopReason().isEmpty());

        QFile recoveryFlag(dir.filePath("recovery-required"));
        QVERIFY(recoveryFlag.open(QIODevice::WriteOnly)); recoveryFlag.close();
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(model.recoveryNeedsAttention(), 7000);
        QVERIFY(failMethod.open(QIODevice::WriteOnly | QIODevice::Truncate));
        failMethod.write("native.snapshot"); failMethod.close();
        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(model.helperStopped(), 7000);
        QVERIFY(model.recoveryNeedsAttention());
        QVERIFY(!model.stopReason().isEmpty());
        QVERIFY(QFile::remove(dir.filePath("fail-on-method")));
        QVERIFY(QFile::remove(dir.filePath("recovery-required")));

        model.refresh();
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && !model.helperStopped() && !model.recoveryNeedsAttention(), 7000);
        QVERIFY(model.stopReason().isEmpty());
        QVERIFY(model.canRefresh());
        model.loadNative(peerPath, dir.filePath("next-native"));
        QVERIFY(model.m_switching);
        QVERIFY(!model.canRefresh());
        QVERIFY(!model.property("canRefresh").toBool());
        QTRY_VERIFY_WITH_TIMEOUT(pairedStartupSettled(model) && model.canRefresh(), 7000);
    }
};
int main(int argc, char **argv) {
    QTemporaryDir home;
    const bool safety = argc > 1 && QString::fromUtf8(argv[1]) == "state-safety";
    if (!safety) {
        qputenv("HOME",home.path().toUtf8());
        for (const auto &key : QProcessEnvironment::systemEnvironment().keys())
            if (key.startsWith("XDG_")) qputenv(key.toUtf8(),home.filePath(key).toUtf8());
        for (const auto *key : {"XDG_CONFIG_HOME","XDG_DATA_HOME","XDG_CACHE_HOME","XDG_STATE_HOME","XDG_RUNTIME_DIR","XDG_CONFIG_DIRS","XDG_DATA_DIRS"}) {
            const auto path=home.filePath(key);QDir().mkpath(path);qputenv(key,path.toUtf8());
        }
        QDir().mkpath(home.filePath(".cache"));
    }
    QCoreApplication app(argc, argv);
    ModelCheck check;
    int failed = 0, passed = 0;
    const auto run = [&](const char *name, auto body) { if (argc > 1 && !QString::fromUtf8(name).contains(QString::fromUtf8(argv[1]))) return; try { body(); ++passed; std::printf("PASS %s\n", name); } catch (const std::exception &e) { ++failed; std::fprintf(stderr, "FAIL %s: %s\n", name, e.what()); } };
    if(safety) { try { check.state_safety(); return 0; } catch(const std::exception &e) { std::fprintf(stderr,"FAIL state-safety: %s\n",e.what()); return 1; } }
    QFile contract(QStringLiteral(TEST_SOURCE_DIR)+"/tests/paired-helper-contract.json");
    if(!contract.open(QIODevice::ReadOnly))return 2;
    const auto required=QJsonDocument::fromJson(contract.readAll()).array();
    const QJsonObject full{{"protocolVersion",1},{"readOnly",false},{"capabilities",required}};
    for(qsizetype index=0;index<required.size();++index) {
        const auto entry=required[index];
        auto hello=full; auto features=required; features.removeAt(index); hello["capabilities"]=features;
        run(qPrintable("step9 hello missing "+entry.toString()),[&]{check.paired_hello(hello,false);});
    }
    for(const auto &field:QStringList{"protocolVersion","readOnly","capabilities"}) {
        auto hello=full;hello.remove(field);
        run(qPrintable("step9 hello absent "+field),[&]{check.paired_hello(hello,false);});
        for(const auto &bad:QJsonArray{QJsonValue::Null,true,"1",QJsonObject{}}) {
            if(field=="readOnly" && bad==QJsonValue(true))continue;
            hello=full;hello[field]=bad;
            run(qPrintable("step9 hello illtyped "+field+" "+QString::number(bad.type())),[&]{check.paired_hello(hello,false);});
        }
    }
    for(const auto &value:QJsonArray{2,1.5}) {
        auto hello=full;hello["protocolVersion"]=value;
        run(qPrintable("step9 hello wrong protocol "+QString::number(value.toDouble())),[&]{check.paired_hello(hello,false);});
    }
    auto readonly=full;readonly["readOnly"]=true;
    run("step9 hello readOnly",[&]{check.paired_hello(readonly,false);});
    run("step9fix hello RPC error",[&]{check.paired_hello(QJsonObject{{"code",-32601},{"message","Method not found"}},false,false,true);});
    for(const auto &action:QStringList{"scan","review","move","resume","cancel","maintenance-scan","clean","share-export","share-review"})
        run(qPrintable("step9fix oneoff "+action),[&]{check.paired_oneoff_admission(action);});
    for(const auto &action:QStringList{"share-privacy","storage-status-recovery"})
        run(qPrintable("step9simplify "+action),[&]{check.paired_oneoff_admission(action);});
    auto malformed=full;auto features=required;features.append(17);malformed["capabilities"]=features;
    run("step9 hello malformed entry",[&]{check.paired_hello(malformed,false);});
    run("step9 hello paired fixture",[&]{check.paired_hello(full,true);});
    auto extra=full;features=required;features.append("future.harmless");extra["capabilities"]=features;
    run("step9 hello unknown feature",[&]{check.paired_hello(extra,true);});
    run("step9 hello real paired helper",[&]{check.paired_hello(full,true,true);});
    run("step9 paired features do not upgrade review format",[&]{check.paired_features_do_not_upgrade_review_format();});
    run("step9 full fixture native startup",[&]{check.paired_model_startup(false);});
    run("step9 full fixture source stays read-only",[&]{check.paired_model_startup(true);});
    run("step9 full fixture native discovery keeps source read-only",[&]{check.paired_model_startup(true,true);});
    for(const auto &arguments:QStringList{"", "  -novid\t\"synthetic\"  ",QString::fromUtf8("-name 界")})
        run(qPrintable("step6 ordinary "+QString::number(arguments.toUtf8().size())),[&]{check.step6_settings_ordinary(arguments);});
    for(const auto &mode:QStringList{"characterize-stored","lost-stored","lost-stored-automatic","lost-unstored","lost-stored-retry","lost-stored-dead-read","lost-unstored-same-ui"})
        run(qPrintable("step6 "+mode),[&]{check.step6_settings_lost(mode);});
    for(const auto &mode:QStringList{"stale","refused","refused-busy","refused-same","refused-same-dead-read"})
        run(qPrintable("step6 "+mode),[&]{check.step6_settings_refusal(mode);});
    for(const auto &mode:QStringList{"control-refusal","unknown-control"})
        run(qPrintable("step6 "+mode),[&]{check.step6_settings_control_read_failure(mode);});
    run("step6 confirmation control fresh retry",[&]{check.step6_confirmation_control_retry();});
    for(const auto &mode:QStringList{"root","profile","source","close"})
        run(qPrintable("step6 lifecycle "+mode),[&]{check.step6_settings_lifecycle(mode);});
    for(const auto &mode:QStringList{"source","close"})
        run(qPrintable("step6 opening reentry "+mode),[&]{check.step6_settings_open_reentry(mode);});
    run("step6 read signal source reentry",[&]{check.step6_settings_read_reentry();});
    run("step6 stale old read continuation",[&]{check.step6_settings_stale_read();});
    run("step6 old files untouched",[&]{check.step6_settings_old_files(true);});
    run("step6 no reference files created",[&]{check.step6_settings_old_files(false);});
    for (const auto &mode : QStringList{"known row", "withdrawal", "epoch", "root", "unknown row", "long identity"})
        run(qPrintable("step5 media " + mode), [&] { check.step5_media_demand(mode); });
    run("edge bootstrap root", [&] { check.edge_bootstrap_root_keeps_pending_creation(); });
    run("scoped model selected helper", [&] { check.model_selected_helper_discovers_root(); });
    run("scoped model unsupported discovery", [&] { check.model_unsupported_discovery_requires_explicit_root(); });
    run("settings catalog saved policy", [&] { checkSettingsCatalogPolicy(); });
    run("settings catalog reset gate", [&] { checkSettingsCatalogResetGate(QStringLiteral(TEST_HELPER_PATH)); });
    run("running discards queued toggles", [&] { check.running_discards_queued_toggles(); });
    run("running lock survives incomplete scan", [&] { check.running_lock_survives_incomplete_scan(); });
    run("profile edit review shape", [&] { check.profile_edit_review_decodes_without_mutating_shape(); });
    run("home summary bounded and quiet", [&] { check.home_summary_is_bounded_and_quiet(); });
    run("item106 review F6", [&] { check.item106_review_f6_queue_owner(); });
    run("item106 review F7", [&] { check.item106_review_f7_continuation_errors(); });
    run("item106 review F1", [&] { check.item106_review_f1_replacement_paths(); });
    run("item106 review F2", [&] { check.item106_review_f2_retained_download(); });
    run("item106 review F3", [&] { check.item106_review_f3_download_destination(); });
    run("item106 review F4", [&] { check.item106_review_f4_two_row_controls(); });
    run("item106 review F5", [&] { check.item106_review_f5_link_wins(); });
    run("item106 newest", [&] { check.item106_newest_file_preselected(); });
    run("item106 finished", [&] { check.item106_finished_downloads_disappear(); });
    run("item106 deferred controls", [&] { check.item106_control_waits_for_job_status(); });
    run("item106 controls", [&] { check.item106_player_controls(); });
    run("remote description", [&] { check.remote_description(); });
    run("review regression cancel batch", [&] { check.review_cancel_stops_batch(); });
    run("review regression cancel then resolve", [&] { check.review_cancel_allows_another_resolve(); });
    run("review regression Browse page link", [&] { check.review_browse_page_link_uses_append(); });
    run("review regression stopped submission", [&] { check.review_stopped_job_does_not_block_same_submission(); });
    run("review regression commit job identity", [&] { check.review_commit_completion_keeps_job_identity(); });
    run("review regression archived select all", [&] { check.review_select_all_keeps_archived_consent(); });
    run("review regression snapshot filters", [&] { check.review_snapshot_filter_membership(); });
    run("library snapshot rows stay in place", [&] { check.library_snapshot_rows_stay_in_place(); });
    run("library unchanged health is quiet", [&] { check.library_unchanged_health_is_quiet(); });
    run("library update replies quiet when unchanged", [&] { check.library_update_replies_are_quiet_when_unchanged(); });
    run("launch unchanged status is quiet", [&] { check.launch_unchanged_status_is_quiet(); });
    run("launch health classification diff persistence", [&] { check.launch_health(); });
    run("detected hero transport", [&] { check.detected_hero_transport(); });
    run("detected library heroes", [&] { check.detected_library_heroes(); });
    run("mod updates API and filter", [&] { check.mod_updates_api(); });
    run("update status error is local", [&] { check.update_status_error_is_local(); });
    run("update all unselected target", [&] { check.update_all_unselected_target(); });
    run("closing batch review cancels remaining", [&] { check.closing_batch_review_cancels_remaining(); });
    run("installed history does not hide Browse download", [&] { check.installed_history_does_not_hide_browse_download(); });
    run("repeated download failures have new identity", [&] { check.repeated_download_failures_have_new_identity(); });
    run("item103 busy preparation keeps progress", [&] { check.item103_busy_preparation_keeps_progress(); });
    run("background download errors are not notices", [&] { check.background_download_errors_are_not_notices(); });
    run("update all one review", [&] { check.update_all_shows_one_batch_review(); });
    run("update all rechecks reviewed record identity", [&] { check.update_all_rechecks_reviewed_record_identity(); });
    run("update all waits for existing apply review", [&] { check.update_all_waits_for_existing_apply_review(); });
    run("known receipt update uses automatic install", [&] { check.known_receipt_update_uses_automatic_install(); });
    run("confirmed replacement schedules apply", [&] { check.confirmed_enabled_replacement_schedules_apply(); });
    run("health unavailable API", [&] { check.health_unavailable_api(); });
    run("health real API", [&] { check.health_real_api(); });
    run("health API", [&] { check.health_api(); });
    run("bisect real API", [&] { check.bisect_real_api(); });
    run("external gameinfo state", [&] { check.external_gameinfo_state(); });
    run("catalog automatic refresh on open and reopen", [&] { check.catalog_automatic_refresh_on_open_and_reopen(); });
    run("installed group real atomic API", [&] { check.installed_group_real_atomic_api(); });
    run("installed API", [&] { check.installed_api(); });
    run("installed grouped preparation and metadata", [&] { check.installed_grouped_preparation_and_metadata(); });
    run("performance real API", [&] { check.performance_real_api(); });
    run("config real API", [&] { check.configs_real_api(); });
    run("config API", [&] { check.configs_api(); });
    run("performance API", [&] { check.performance_api(); });
    run("delete mods exact context", [&] { check.delete_mods_invokable_and_exact_context(); });
    run("bulk invokables and summary", [&] { check.bulk_invokables_and_summary(); });
    run("bulk lost reply shows helper state", [&] { check.bulk_invokables_and_summary(true); });
    run("local_capability_arrival_enables_real_control", [&] { check.local_capability_arrival_enables_real_control(); });
    run("local_prepare_keeps_navigation_and_destination", [&] { check.local_prepare_keeps_navigation_and_destination(); });
    run("local_cancel_uses_control_rpc_without_helper_kill", [&] { check.local_cancel_uses_control_rpc_without_helper_kill(); });
    run("local_main_writer_allows_control_without_marker", [&] { check.local_main_writer_allows_control_without_marker(); });
    run("local_unknown_disconnect_reconciles_same_operation", [&] { check.local_unknown_disconnect_reconciles_same_operation(); });
    run("local_malformed_oversize_dto_is_rejected", [&] { check.local_malformed_oversize_dto_is_rejected(); });
    run("local_artwork_bounds_fingerprint_and_queue", [&] { check.local_artwork_bounds_fingerprint_and_queue(); });
    run("local_marker_persistence_failure_dispatches_no_commit", [&] { check.local_marker_persistence_failure_dispatches_no_commit(); });
    run("edit_saves_without_marker_storage", [&] { check.edit_saves_without_marker_storage(); });
    run("local_default_root_and_graceful_restart", [&] { check.local_default_root_and_graceful_restart(); });
    run("local_main_preview_restart_preserves_job_owner", [&] { check.local_main_preview_restart_preserves_job_owner(); });
    run("local_poll_error_cannot_clear_main_writer_marker", [&] { check.local_poll_error_cannot_clear_main_writer_marker(); });
    run("local_one_bulk_permit_and_latest_view", [&] { check.local_one_bulk_permit_and_latest_view(); });
    run("legacy link explicit target and marker", [&] { check.legacy_link_keeps_explicit_target_and_marker(); });
    run("local_old_job_reply_cannot_replace_current_job", [&] { check.local_old_job_reply_cannot_replace_current_job(); });
    run("local_oversized_small_wire_response_is_rejected", [&] { check.local_old_job_reply_cannot_replace_current_job("native-local-oversized"); });
    run("local_maximum_bulk_frame_with_small_cancel", [&] { check.local_one_bulk_permit_and_latest_view("native-local-crossing-boundary"); });
    run("native_snapshot_accepts_versions_one_and_two_strictly", [&] { check.native_snapshot_accepts_versions_one_and_two_strictly(); });
    run("family_review_counts_are_strict_and_legacy_compatible", [&] { check.family_review_counts_are_strict_and_legacy_compatible(); });
    run("family_operation_reconciliation_keeps_unknown_identity", [&] { check.family_operation_reconciliation_keeps_unknown_identity(); });
    run("protocol response limit reloads edit state", [&] { check.disconnectedEditReloadsHelperState("native-response-limit"); });
    run("protocol response limit preserves transaction marker", [&] { check.disconnectedTransactionKeepsMarkerAndReconciles("native-restore-lost-limit"); });
    run("protocol internal error reloads edit state", [&] { check.disconnectedEditReloadsHelperState("native-internal-error"); });
    run("protocol internal error preserves transaction marker", [&] { check.disconnectedTransactionKeepsMarkerAndReconciles("native-restore-lost-internal"); });
    run("default refresh discovers external import", [&] { check.refreshDiscoversExternalImport(false); });
    run("explicit source refresh stays upstream", [&] { check.refreshDiscoversExternalImport(true); });
    run("notifications follow authoritative state", [&] { check.notificationsFollowAuthoritativeState(); });
    run("item115 small launch cause", [&] { check.item115SmallLaunchCauseReplacesEmptyOrStaleError(); });
    run("item124 historical recovery does not prove original", [&] { check.item124HistoricalRecoveryDoesNotProveOriginal(); });
    run("item115 small gesture errors", [&] { check.item115SmallGesturesClearOldDisplayError(); });
    run("item124 restart view waits for the attempt", [&] { check.item124RestartViewWaitsForTheAttempt(); });
    run("item124 editing waits for the view after Play", [&] { check.item124EditingWaitsForTheViewAfterPlay(); });
    for(const auto &site:QStringList{"persist-failure","refusal","refusal-refresh","outcome","apply-success","apply-failure"})
        for(const auto &race:QStringList{"none","newer","repeat","reentry"})
            run(qPrintable("step8 "+site+" "+race),[&]{check.step8LateError(site,race);});
    for(const auto &site:QStringList{"apply-success","apply-failure"})
        run(qPrintable("step8 "+site+" queued"),[&]{check.step8LateError(site,"queued");});
    for(const auto &race:QStringList{"none","newer"})
        run(qPrintable("step8 marker-read-failure "+race),[&]{check.step8LateError("marker-read-failure",race);});
    for(const auto &site:QStringList{"replay-snapshot","replay-failure","replay-failure-repeat","replay-failure-current"})
        run(qPrintable("step8 "+site),[&]{check.step8LateError(site,"none");});
    for(const auto &site:QStringList{"bootstrap-dispatch-create","bootstrap-dispatch-retry"})
        for(const auto &race:QStringList{"none","newer"})
            run(qPrintable("step8 "+site+" "+race),[&]{check.step8LateError(site,race);});
    for(const auto &site:QStringList{"wrapper","remote","bootstrap-clear"})
        for(const auto &race:QStringList{"none","newer","repeat"})
            run(qPrintable("step8 "+site+" "+race),[&]{check.step8RecoveryError(site,race);});
    for(const auto &site:QStringList{"wrapper","remote","bootstrap-clear"})
        run(qPrintable("step8 "+site+" reentry"),[&]{check.step8RecoveryError(site,"reentry");});
    run("step8 outcome-refresh none",[&]{check.step8LateError("outcome-refresh","none");});
    run("step8 outcome-refresh reentry",[&]{check.step8LateError("outcome-refresh","refresh");});
    run("step8fix refusal before marker ownership",[&]{check.step8LateError("refusal","early-review");});
    for(const auto &site:QStringList{"bootstrap-dispatch-create","bootstrap-dispatch-retry"})
        run(qPrintable("step8fix "+site+" initial clear"),[&]{check.step8LateError(site,"clear-entry");});
    for(const auto &site:QStringList{"bootstrap-dispatch-create","bootstrap-dispatch-retry"})
        run(qPrintable("step8fix "+site+" initial busy"),[&]{check.step8LateError(site,"busy-entry");});
    for(const auto &site:QStringList{"bootstrap-clear","remote","wrapper-snapshot"})
        run(qPrintable("step8fix "+site+" snapshot entry"),[&]{check.step8RecoveryError(site,"snapshot-entry");});
    for(const auto &site:QStringList{"replay-failure-reentry","replay-failure-reentry-repeat"})
        run(qPrintable("step8fix "+site),[&]{check.step8LateError(site,"none");});
    run("item129 failed recovery preserves newer error", [&] { check.item129FailedRecoveryPreservesNewerError(); });
    run("item133 remote recovery preserves newer error", [&] { check.item133RemoteRecoveryPreservesNewerError(); });
    run("item133 remote recovery repeated identical error", [&] { check.item133RemoteRecoveryPreservesNewerError(true); });
    run("item126 stale recovery error", [&] { check.item126RecoveryError(); });
    run("item126 newer helper failure", [&] { check.item126RecoveryError(true); });
    run("item126 repeated helper failure", [&] { check.item126RecoveryError(true,true); });
    run("item115 latest recovery blocker", [&] { check.item115FinalRecoverShowsLatestBlocker(); });
    run("item115 recovery cause keeps marker", [&] { check.item115RecoveryCauseKeepsMarker(); });
    run("failed recovery context preserves original identity", [&] { check.failedRecoveryContextReadKeepsPendingIdentity(); });
    run("slow review cancellation", [&] { check.slowReviewCanBeCancelledWithoutAWrite(); });
    for (const auto *mode : {"native-read-only", "native-delay", "native-invalid-bool", "native-invalid-revision"})
        run(mode, [&] { check.nativeCapabilityAndMalformedContracts(mode); });
    run("disconnected edit completes without kill or replay", [&] { check.disconnectedEditReloadsHelperState(); });
    run("disconnected transaction keeps marker and reconciles", [&] { check.disconnectedTransactionKeepsMarkerAndReconciles(); });
    run("lost edit reply stored shows helper state", [&] { check.lostEditReplyShowsHelperState(true); });
    run("item136 rejected reconnect snapshot closes writes", [&] { check.lostEditSnapshotFailureKeepsWritesClosed(); });
    run("item136 source switch after lost edit", [&] { check.lostEditSourceSwitchCompletes(); });
    run("item136 automatic source switch after lost edit", [&] { check.lostEditSourceSwitchCompletes(true); });
    run("item136 local status and cancellation survive snapshot failure", [&] { check.lostEditKeepsLocalStatusAndCancel(); });
    run("item136 remaining write gates", [&] { check.lostEditRemainingWriteGates(); });
    run("lost edit reply unstored shows helper state", [&] { check.lostEditReplyShowsHelperState(false); });
    run("lost toggle reply stored still reaches automatic Apply", [&] { check.lostToggleReplyStillReachesAutomaticApply(true); });
    run("lost toggle reply unstored is dropped before automatic Apply", [&] { check.lostToggleReplyStillReachesAutomaticApply(false); });
    run("pending startup cannot replace demo", [&] { check.pendingStartupCannotReplaceDemo(); });
    run("unknown no-journal lock barrier", [&] { check.recoveryBarrier("recovery-unknown"); });
    run("no-document recovery with missing source", [&] { check.recoveryBarrier("recovery-no-doc"); });
    run("explicit source cannot bypass recovery", [&] { check.recoveryBarrier("recovery-no-doc-source", true); });
    run("historical outcome cannot regress snapshot", [&] { check.recoveryBarrier("recovery-terminal"); });
    run("native async authority/refresh safety", [&] { check.nativeAsyncAuthorityAndRefresh(); });
    run("native rejection", [&] { check.nativeRejectedMutation(); });
    run("native snapshot decode", [&] { check.nativeSnapshotDecode(); });
    run("storage Cancel backend authority", [&] { check.storage_cancel_backend_authority(); });
    run("storage status newer than the current model", [&] { check.storage_status_ahead_of_model(); });
    run("storage status stalled helper times out and can retry", [&] { check.storage_status_stall(false); });
    run("storage status stalled helper exits with controller", [&] { check.storage_status_stall(true); });
    run("defaults", [&] { check.defaults(); });
    run("separate hero and type filters", [&] { check.separate_hero_and_type_filters(); });
    run("demo selection/filter/counts", [&] { check.demoSelectionFilteringAndCounts(); });
    run("share export survives background notifications", [&] { check.share_export_survives_background_notifications(); });
    run("library metadata strict fields and roles", [&] { check.library_metadata_is_strict_and_available_in_roles(); });
    for(const auto &mode:QStringList{"shared","cap","refusal","deferred"})
        run(QStringLiteral("library media open retry %1").arg(mode).toUtf8().constData(), [&] { checkLibraryMediaOpenRetry(QStringLiteral(TEST_HELPER_PATH),mode); });
    run("library media busy open recovery queued", [&] { checkLibraryMediaBusyOpenRecovery(QStringLiteral(TEST_HELPER_PATH),false); });
    run("library media busy open recovery browse", [&] { checkLibraryMediaBusyOpenRecovery(QStringLiteral(TEST_HELPER_PATH),true); });
    run("catalog scheduling deferred media", [&] { checkCatalogDeferredMedia(QStringLiteral(TEST_HELPER_PATH)); });
    run("cached catalog scheduling", [&] { checkCatalogCacheScheduling(QStringLiteral(TEST_HELPER_PATH)); });
    for(const auto *mode : {"overlap", "stall", "catalogBusy", "catalogRequestPending", "shutdown"})
        run(QStringLiteral("catalog scheduling %1").arg(mode).toUtf8().constData(), [&] { checkCatalogPollCadence(QStringLiteral(TEST_HELPER_PATH), mode); });
    run("home feed preserves browse state", [&] { checkHomeFeedPreservesBrowseState(QStringLiteral(TEST_HELPER_PATH)); });
    run("home feed yields to active browse", [&] { checkHomeFeedYieldsToActiveBrowse(QStringLiteral(TEST_HELPER_PATH)); });
    run("home feed rejects stale root nsfw inactive responses", [&] { checkHomeFeedRejectsStaleRootNsfwInactiveResponses(QStringLiteral(TEST_HELPER_PATH)); });
    run("home feed releases cancelled receipts and never auto syncs", [&] { checkHomeFeedReleasesCancelledReceiptsAndNeverAutoSyncs(QStringLiteral(TEST_HELPER_PATH)); });
    run("preview decode/roles/backward compatibility", [&] { check.previewDecodeAndRoles(); });
    run("preview URL validation/bounds", [&] { check.previewInvalidMetadataDoesNotBreakMods(); });
    run("helper preview selection/refresh/read-only", [&] { check.helperPreviewSelectionAndRefresh(); });
    run("actual helper/refresh/read-only", [&] { check.actualHelperAndRefresh(); });
    run("empty source", [&] { check.sourceErrors({}, "invalidState"); });
    run("truncated source", [&] { check.sourceErrors("{", "invalidState"); });
    run("unsupported source", [&] { check.sourceErrors(R"({"local-config":{"version":34,"state":{}}})", "unsupportedVersion"); });
    for (const auto *mode : {"hello-version", "hello-write", "hello-capabilities", "invalid", "oversized", "truncated", "timeout", "exit", "bad-snapshot", "wrong-profile", "partial", "stderr"})
        run(mode, [&] { check.transport(mode, QString(mode) == "partial" || QString(mode) == "stderr"); });
    run("missing helper/source", [&] { check.missingHelperAndSource(); });
    run("empty store/fresh refresh/partial state", [&] { check.emptyStoreAndFreshRefresh(); });
    run("child stops on destruction", [&] { check.childStopsOnDestruction(); });
    run("superseded profile replies", [&] { check.supersededProfileReplies(); });
    run("item53 shared local helper stop", [&] { check.item53_shared_local_helper_stop(); });
    if (argc > 1 && passed == 0 && failed == 0) {
        std::fprintf(stderr, "No model checks matched filter '%s'.\n", argv[1]);
        return 2;
    }
    std::printf("%d passed, %d failed\n", passed, failed);
    return failed ? 1 : 0;
}
