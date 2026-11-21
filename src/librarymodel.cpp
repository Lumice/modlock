#include "apppaths.h"
#include "performancecontroller.h"
#include "librarymodel.h"
#include "applywarningpolicy.h"
#include "backendclient.h"
#include "libraryrows.h"
#include "librarymediacontroller.h"
#include "localartworkprovider.h"
#include "fontdata.h"
#include "launchdata.h"
#include "remotedata.h"
#include <algorithm>
#include <limits>
#include <QProcess>
#include <QNetworkAccessManager>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QCryptographicHash>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonArray>
#include <QRunnable>
#include <QSaveFile>
#include <QStandardPaths>
#include <QThreadPool>
#include <QTimer>
#include <QDateTime>
#include <QUuid>
#include <QUrl>
#include <fcntl.h>
#include <unistd.h>

namespace {
bool remoteRequestMatches(const QJsonObject &request,const QVariantMap &marker,const QString &root) {
    if(request["dataDir"]!=QJsonValue(root) || marker.value("root").toString()!=root)return false;
    for(const auto *key:{"jobId","profileId","expectedRevision","operationId","consentFingerprint","cohortFingerprint","selectionDigest"})
        if(request[key]!=QJsonValue(marker.value(QLatin1String(key)).toString()))return false;
    if(marker.value("method").toInt()==int(BackendClient::Call::RemoteReplaceCommit))
        for(const auto *key:{"modId","purpose","initialRevision","targetDigest","afterRecordDigest"})
            if(request[key]!=QJsonValue(marker.value(QLatin1String(key)).toString()))return false;
    return true;
}
bool bootstrapMarker(const QJsonObject &marker,const QString &root) {
    const auto params=marker["params"].toObject();
    return marker.size()==6 && marker["bootstrapVersion"]==QJsonValue(1) && marker["root"]==QJsonValue(root)
        && LaunchData::root(marker["root"]) && LaunchData::hex(marker["operationId"],32)
        && marker["method"]==QJsonValue(int(BackendClient::Call::LibraryInitialize)) && marker["expectedRevision"]==QJsonValue("0")
        && marker["params"].isObject() && params.size()==3 && params["version"]==QJsonValue(1)
        && params["operationId"]==marker["operationId"] && params["expectedRevision"]==marker["expectedRevision"];
}
}

class ContextTask final : public QObject, public QRunnable {
    Q_OBJECT
public:
    using Result = std::pair<bool, QVariantMap>;
    explicit ContextTask(std::function<Result()> action) : m_action(std::move(action)) {}
    void run() override {
        const auto [ok, marker] = m_action(); emit finished(ok, marker);
    }
signals:
    void finished(bool ok, const QVariantMap &marker);
private:
    std::function<Result()> m_action;
};

// Calls whose whole write set is one SQLite commit in the helper
// (backend/src/native.rs). They carry no app marker.
static bool singleCommitEdit(BackendClient::Call call) {
    return call==BackendClient::Call::SetEnabled || call==BackendClient::Call::SetFixEnabled || call==BackendClient::Call::SetFixAutomatically
        || call==BackendClient::Call::Profile || call==BackendClient::Call::SetModsEnabled || call==BackendClient::Call::RemoveModsFromProfile;
}

LibraryModel::LibraryModel(QObject *parent)
    : QSortFilterProxyModel(parent), m_rows(new LibraryRows(this)), m_client(new BackendClient(this)), m_catalog(new CatalogController(m_client,this)), m_media(new LibraryMediaController(m_catalog,this)), m_remote(new RemoteController(this,m_client)), m_localImport(new LocalImportController(this)), m_launch(new LaunchController(this,m_client)), m_performance(new PerformanceController(this)), m_autoexec(new GameConfigController(this,"autoexec")), m_crosshair(new GameConfigController(this,"crosshair")), m_fonts(new FontController(this)), m_options(new OptionController(this)) {
    m_autoApplyTimer.setSingleShot(true);
    m_autoApplyTimer.setInterval(0);
    connect(&m_autoApplyTimer, &QTimer::timeout, this, [this] { m_autoDue = true; advanceAutoApply(); });
    m_installedMetadataTimer.setInterval(500);
    connect(&m_installedMetadataTimer,&QTimer::timeout,this,&LibraryModel::enrichInstalledMods);
    connect(m_catalog,&CatalogController::libraryMediaReady,this,[this](const QString &key,const QString &root,const CatalogDecodedResult &result){
        if(key!=m_installedMetadataKey || key.isEmpty())return;
        const auto id=m_installedMetadataId;const auto epoch=m_installedMetadataEpoch;
        m_installedMetadataKey.clear();m_installedMetadataId.clear();
        if(epoch!=m_loadEpoch || root!=m_dataDir)return;
        const auto summary=result.detail.value("summary").toMap();
        if(result.phase=="succeeded" && !summary.isEmpty()) {
            if(m_installedMetadataCache.size()>=128)m_installedMetadataCache.clear();
            m_installedMetadataCache.insert(id,summary);applyInstalledMetadata(id,summary);
        }
    });
    m_rows->setMediaSource(m_media);
    m_updatesTimer.setInterval(1000);
    connect(&m_updatesTimer,&QTimer::timeout,this,&LibraryModel::advanceUpdateCheck);
    connect(&m_updatesTimer,&QTimer::timeout,this,&LibraryModel::advanceUpdateReviews);
    m_updatesTimer.start();
    connect(m_client,&BackendClient::modUpdatesReady,this,[this](const QVariantMap &v){
        m_updatesReadPending=false;
        if(v.value("profileId").toString()!=m_viewedProfileId || v.value("revision").toString()!=m_revision)return;
        m_updatesError.clear();m_modUpdates=v.value("mods").toList();m_lastUpdateCheck=v.value("lastUpdateCheck");
        const bool membershipChanged=m_rows->setUpdates(m_modUpdates);
        if(m_updatesOnly && membershipChanged)invalidateFilter();
        emit updatesChanged();emit selectionChanged();
        if(m_updatesFinish){m_updatesFinish=false;m_updatesChecking=false;emit updatesChanged();}
        else if(!m_updatesChecking && (!m_lastUpdateCheck.isValid() || QDateTime::currentSecsSinceEpoch()-m_lastUpdateCheck.toLongLong()>=21600))checkForUpdates();
    });
    connect(m_client,&BackendClient::rejected,this,[this](BackendClient::Call c,const QString &,const QString &){if(c==BackendClient::Call::ModUpdatesGet){m_updatesReadPending=false;m_updatesChecking=false;m_updatesFinish=false;m_updatesError=(m_client->launchOwned() || m_launch->writeGate()) ? tr("Checking for updates will work after you close Deadlock.") : tr("Couldn't check mod updates. Try again.");emit updatesChanged();}});
    connect(m_catalog,&CatalogController::libraryMediaReady,this,[this](const QString &key,const QString &root,const CatalogDecodedResult &result){
        if(key==m_updateResourceKey && root==m_dataDir){m_updateResourceKey.clear();
            const auto refreshError=result.provenance.value("refreshError").toMap().value("kind").toString();
            if(QStringList{"catalogTransport","catalogUnavailable","catalogRateLimited"}.contains(result.errorKind)
                    || QStringList{"catalogTransport","catalogUnavailable","catalogRateLimited"}.contains(refreshError)){
                m_updatesOffline=true;m_updateResources.clear();
            }
            m_updateWait.restart();QTimer::singleShot(0,this,&LibraryModel::advanceUpdateCheck);}
    });
    connect(m_catalog,&CatalogController::libraryMediaInvalidated,this,[this](const QString &root,const QString &,const QString &){if(root==m_dataDir && !m_updatesChecking)checkForUpdates();});
    connect(m_remote,&RemoteController::changed,this,[this]{QTimer::singleShot(0,this,&LibraryModel::advanceUpdateReviews);});

    connect(this,&LibraryModel::nativeChanged,this,[this]{if(canScanInstalledMods() && m_detectedGamePath.isEmpty() && !m_installedModsBusy)detectGame();});
    connect(this, &LibraryModel::totalCountChanged, this, &LibraryModel::refreshHomeRecent);
    connect(this, &LibraryModel::totalCountChanged, this, &LibraryModel::refreshHomeEnabled);
    connect(this, &LibraryModel::enabledCountChanged, this, &LibraryModel::refreshHomeEnabled);
    connect(this, &LibraryModel::sourceChanged, this, &LibraryModel::refreshHomeEnabled);
    connect(m_media,&LibraryMediaController::mediaChanged,this,[this](const QString &row){
        if(m_rows->notifyMediaChanged(row)){
            const auto contains = [&row](const QVariantList &list) { return std::any_of(list.cbegin(), list.cend(), [&row](const QVariant &item) { return item.toMap().value("id").toString() == row; }); };
            if(contains(m_homeRecent))refreshHomeRecent();
            if(contains(m_homeEnabled))refreshHomeEnabled();
            if(row==m_selectedId)emit selectionChanged();
        }
    });
    connect(m_media,&LibraryMediaController::capacityAvailable,this,[this]{updateSelectedMediaDemand();emit libraryMediaCapacityChanged();});
    connect(this,&LibraryModel::selectionChanged,this,&LibraryModel::updateSelectedMediaDemand);
    m_remoteRetry.setSingleShot(true);connect(&m_remoteRetry,&QTimer::timeout,this,&LibraryModel::reconcileRemote);
    connect(m_remote,&RemoteController::changed,this,[this]{emit shutdownChanged();if(m_shutdownRequested)QTimer::singleShot(0,this,&LibraryModel::advanceShutdown);else if(m_switching)QTimer::singleShot(0,this,&LibraryModel::advanceLoad);});
    connect(m_localImport,&LocalImportController::changed,this,[this]{if(m_shutdownRequested)QTimer::singleShot(0,this,&LibraryModel::advanceShutdown);else if(m_switching)QTimer::singleShot(0,this,&LibraryModel::advanceLoad);else QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);});
    connect(m_options,&OptionController::changed,this,[this]{QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);});
    connect(m_fonts,&FontController::changed,this,[this]{QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);});
    connect(m_remote,&RemoteController::changed,this,[this]{QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);});
    connect(m_launch,&LaunchController::changed,this,[this]{
        if(m_launch->gameStatus().value("state")=="running") {
            if(!m_autoToggles.isEmpty()) {
                for(const auto &entry:m_fullProfileOrder) {
                    const auto row=entry.toMap();
                    if(m_autoToggles.contains(row.value("id").toString()))
                        m_rows->setEnabled(row.value("id").toString(),row.value("enabled").toBool());
                }
                m_autoToggles.clear();m_autoBulkIds.clear();
                reconcileSelection();emit enabledCountChanged();emit selectionChanged();emit nativeChanged();emit shutdownChanged();
            }
            if(m_closeAfterToggles){m_closeAfterToggles=false;requestShutdown();return;}
        }
        QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);
    });
    connect(m_client,&BackendClient::launchRetired,this,[this]{m_restartAfterDrain=false;if(m_shutdownRequested)advanceShutdown();else if(m_switching)advanceLoad();else QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);});
    connect(this,&LibraryModel::loadingChanged,this,[this]{if(!m_loading && !m_deferredLoads.isEmpty() && !m_shutdownRequested)QTimer::singleShot(0,this,&LibraryModel::advanceLoad);});
    connect(m_remote,&RemoteController::statusEvidence,this,&LibraryModel::remoteStatus);
    connect(m_remote,&RemoteController::replacementTargetEvidence,this,&LibraryModel::remoteTarget);
    connect(m_remote,&RemoteController::replacementTargetRejected,this,&LibraryModel::remoteTargetRejected);
    connect(m_client,&BackendClient::remoteSnapshot,this,&LibraryModel::remoteReplay);
    connect(m_client,&BackendClient::remoteRejected,this,&LibraryModel::remoteReplayRejected);
    connect(m_client,&BackendClient::remoteReady,this,[this](BackendClient::Call call,const QJsonObject &request,const QVariantMap &){
        if(int(call)==m_marker.value("method").toInt() && remoteMarkerPending() && m_remoteStage==RemoteStage::Replay && remoteRequestMatches(request,m_marker,m_dataDir)) {
            m_remoteToken.clear();m_remoteStage=RemoteStage::Idle;setBusy(false);scheduleRemoteReconcile(1000);
        }
    });
    connect(m_catalog,&CatalogController::changed,this,[this]{emit shutdownChanged();if(m_shutdownRequested)QTimer::singleShot(0,this,&LibraryModel::advanceShutdown);else if(m_switching)QTimer::singleShot(0,this,&LibraryModel::advanceLoad);});
    connect(m_client,&BackendClient::lifecycleChanged,this,[this]{ emit nativeChanged(); if(canScanInstalledMods() && m_detectedGamePath.isEmpty() && !m_installedModsBusy) detectGame(); });
    connect(this,&LibraryModel::nativeChanged,this,[this]{emit shutdownChanged();if(m_shutdownRequested)QTimer::singleShot(0,this,&LibraryModel::advanceShutdown);else if(m_switching)QTimer::singleShot(0,this,&LibraryModel::advanceLoad);});
    connect(m_client,&BackendClient::retired,this,[this]{
        if(remoteMarkerPending() || recoveryWrapperPending()) {m_remoteToken.clear();m_remoteStage=RemoteStage::Idle;m_reconciling=false;scheduleRemoteReconcile(1000);}
        emit shutdownChanged();if(m_shutdownRequested){advanceShutdown();return;}if(m_switching){advanceLoad();return;}
        if(m_restartAfterDrain){
            m_restartAfterDrain=false;
            refresh();
        }
    });
    setSourceModel(m_rows);
    setDynamicSortFilter(true);
    connect(m_rows, &QAbstractItemModel::dataChanged, this,
            [this](const QModelIndex &, const QModelIndex &, const QList<int> &roles) {
        const auto changed=[&](int role){return roles.isEmpty() || roles.contains(role);};
        if ((m_enabledOnly && changed(LibraryRows::ModEnabled))
            || (m_updatesOnly && changed(LibraryRows::ModUpdateStatus))
            || (m_needsAttentionOnly && changed(LibraryRows::ModHealthStatus))
            || ((!m_category.isEmpty() || !m_heroFilter.isEmpty() || !m_typeFilter.isEmpty() || !m_query.trimmed().isEmpty()) && (changed(LibraryRows::Kind) || changed(LibraryRows::Hero)))
            || (!m_query.trimmed().isEmpty() && (changed(LibraryRows::Title) || changed(LibraryRows::Author)))) {
            beginFilterChange(); endFilterChange(Direction::Rows);
        }
    });
    const auto changed = [this] { reconcileSelection(); emit countChanged(); };
    connect(this, &QAbstractItemModel::rowsInserted, this, changed);
    connect(this, &QAbstractItemModel::rowsRemoved, this, changed);
    connect(this, &QAbstractItemModel::modelReset, this, changed);
    connect(this, &QAbstractItemModel::layoutChanged, this, changed);
    connect(m_client, &BackendClient::bulkResultReady, this, [this](const QVariantMap &summary) {
        m_lastBulkResult=summary; emit lastBulkResultChanged();
    });
    connect(m_client,&BackendClient::heroReady,this,[this](const QVariantMap &result){
        if(m_nativeMode && m_heroRevision==m_revision && result.value("revision").toString()==m_revision) {
            if(m_rows->setDetectedHero(result.value("modId").toString(),result.value("heroDisplay").toString())) {
                invalidateFilter();emit categoriesChanged();emit selectionChanged();
            }
        }
    });
    connect(m_client,&BackendClient::lifecycleChanged,this,[this]{if(m_client->retiring() || !m_client->isRunning()){m_heroPending=false;m_heroQueue.clear();}});
    connect(m_client,&BackendClient::heroSettled,this,[this]{m_heroPending=false;QTimer::singleShot(0,this,&LibraryModel::detectNextHero);});
    connect(m_client,&BackendClient::rejected,this,[this](BackendClient::Call call,const QString &,const QString &){if(call==BackendClient::Call::HeroDetect){m_heroPending=false;QTimer::singleShot(0,this,&LibraryModel::detectNextHero);}});
    connect(m_client, &BackendClient::snapshotReady, this, &LibraryModel::applySnapshot);
    connect(m_client, &BackendClient::recoverySnapshotReady, this, [this](const LibrarySnapshot &snapshot) {
        if(m_remoteStage==RemoteStage::RecoverySnapshot || (recoveryWrapperPending() && m_remoteStage==RemoteStage::RecoveryView)){remoteRecoverySnapshot(snapshot);return;}
        m_revision = snapshot.revision; m_recoveryRequired = snapshot.recoveryRequired; m_haveRecoveryContext = true;
        // Recovery mutations emit completed next; context reads continue lookup.
        if (!m_writeBusy) { setBusy(false); reconcileOperation(); }
        emit nativeChanged();
    });
    connect(m_client, &BackendClient::recoverCommitted, this, [this](const LibrarySnapshot &snapshot,const QString &digest) {
        if(recoveryWrapperPending()){recoveryWrapperResult(snapshot,digest);return;}
        if(remoteMarkerPending()){remoteRecoveryResult(snapshot,digest);return;}
        if(m_marker.value("method").toInt()==int(BackendClient::Call::Apply)) {
            // Recover returns its default profile, not the user's requested view.
            m_revision=snapshot.revision;m_recoveryRequired=snapshot.recoveryRequired;m_haveRecoveryContext=true;
            emit nativeChanged();
        } else applySnapshot(snapshot);
        // completed(Recover) next checks the original operation before a fresh view.
    });
    connect(m_client, &BackendClient::capabilitiesReady, this, [this] { emit nativeChanged(); inspectInitialization();if(remoteMarkerPending() || recoveryWrapperPending())scheduleRemoteReconcile(); });
    connect(m_client,&BackendClient::recoveryInspectionReady,this,&LibraryModel::recoveryInspection);
    connect(m_client, &BackendClient::upstreamMissing, this, [this] {
        m_reconciling = false; setBusy(false); setLoading(false);
        if (!m_marker.isEmpty() && m_client->isRunning())
            QTimer::singleShot(0, this, [this] { reconcileOperation(); });
    });
    connect(m_client, &BackendClient::nativeMissing, this, [this] {
        m_nativeMode = false; m_revision = "0"; emit modeChanged(); emit nativeChanged();
        if(m_inspectState!="checking" && m_inspectRoot!=m_dataDir)inspectInitialization();
    });
    connect(m_client,&BackendClient::bootstrapInspectionReady,this,[this](const LibraryInitializationInspection &inspection){
        if(inspection.root!=m_dataDir || m_client->nativeRoot()!=m_dataDir || m_demoMode)return;
        m_inspectRoot=inspection.root;m_inspectSession=m_client->sessionGeneration();m_inspectState=inspection.state;m_inspectEligible=inspection.canCreate;
        if(inspection.state=="absent")m_inspectReason=tr("Creates an empty native library. Existing upstream data stays unchanged.");
        else if(inspection.state=="initialized")m_inspectReason=tr("A native library already exists in this data root.");
        else {
            if(inspection.reason=="orphanLibrarySidecar")m_inspectReason=tr("Native database sidecars exist without a library. Preserve them for recovery.");
            else if(inspection.reason=="unknownRootEntry")m_inspectReason=tr("Unrecognized files prevent creation. Preserve and review the listed paths or choose a different empty native data root.");
            else if(inspection.reason=="unsafeRoot")m_inspectReason=tr("The native data root must be an owned directory with safe, non-symlink ancestors.");
            else if(inspection.reason=="bootstrapLimit")m_inspectReason=tr("Bootstrap artifact limits reached. Review or recover the listed artifacts, or choose a different empty native data root.");
            else if(inspection.reason=="bootstrapStorage")m_inspectReason=tr("Bootstrap artifacts cannot be safely inspected. Preserve them for recovery or choose a different empty native data root.");
            else if(inspection.reason=="unsupportedLibrary")m_inspectReason=tr("The existing native library uses an unsupported version. Preserve it for recovery.");
            else m_inspectReason=tr("The existing native library cannot be read safely. Preserve it and its sidecars for recovery.");
            if(!inspection.blockedPaths.isEmpty()) {
                QString displayRoot;
                for(const auto c:inspection.root) {
                    if(c.category()==QChar::Other_Control || c.category()==QChar::Other_Format)
                        displayRoot+=QStringLiteral("\\u%1").arg(uint(c.unicode()),4,16,QLatin1Char('0'));
                    else displayRoot+=c;
                }
                m_inspectReason+=QStringLiteral("\n")+tr("Inspected data root: %1").arg(displayRoot)
                    +QStringLiteral("\n")+tr("Paths relative to this root:")
                    +QStringLiteral("\n")+inspection.blockedPaths.join(QStringLiteral("\n"));
            }
            if(inspection.pathsTruncated)m_inspectReason+=QStringLiteral("\n")+tr("Additional paths are not shown.");
        }
        emit nativeChanged();
    });
    connect(m_client,&BackendClient::bootstrapInitialized,this,[this](const QString &root,const QString &id){
        if(!bootstrapPending() || root!=m_dataDir || m_client->nativeRoot()!=root || id!=m_marker.value("operationId").toString()) {
            m_outcomeUnknown=true;m_reconciling=false;setBusy(false);setError(tr("The creation receipt does not match the saved operation context."));return;
        }
        confirmBootstrap(true);
    });
    connect(m_client,&BackendClient::bootstrapSnapshotReady,this,[this](const LibrarySnapshot &snapshot){
        if(!m_bootstrapConfirming || !bootstrapPending() || m_client->nativeRoot()!=m_dataDir) {
            m_bootstrapConfirming=false;m_outcomeUnknown=true;m_reconciling=false;setLoading(false);setBusy(false);
            setError(tr("The current snapshot does not match the saved creation context."));return;
        }
        m_requestedProfileId.clear();
        applySnapshot(snapshot);
    });
    connect(m_client, &BackendClient::reviewReady, this, [this](BackendClient::Call call, const QVariantMap &review) {
        if (call == BackendClient::Call::FontsPreview) { setBusy(false); m_fonts->acceptReview(review); return; }
        if (call == BackendClient::Call::ProfileEditPreview) {setBusy(false); m_options->acceptProfileReview(review); return;}
        if (call == BackendClient::Call::ApplyPreview && m_autoPreview) {
            m_autoPreview = false;
            setBusy(false);
            if (!m_autoToggles.isEmpty()) { QTimer::singleShot(0, this, &LibraryModel::advanceAutoApply); return; }
            if (autoApplyWorkflowBlocked()) { m_autoDue = true; return; }
            if (m_autoReviewRevision != m_revision) { m_autoReviewRevision.clear(); m_autoDue = true; QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply); return; }
            m_autoReview = review;
            QTimer::singleShot(0, this, &LibraryModel::advanceAutoApply);
            return;
        }
        if (call == BackendClient::Call::ImportPreview) m_importReview = review;
        else m_applyReview = review;
        setBusy(false); emit reviewChanged();
    });
    m_healthRefreshTimer.setInterval(30000);
    connect(&m_healthRefreshTimer, &QTimer::timeout, this, &LibraryModel::refreshModHealth);
    connect(this, &LibraryModel::nativeChanged, this, [this] {
        if (nativeMode() && !busy() && !loading() && !m_healthRefreshTimer.isActive()) {
            m_healthRefreshTimer.start();
            QTimer::singleShot(0, this, &LibraryModel::refreshModHealth);
        }
    });
    m_healthTimer.setInterval(500);
    connect(&m_healthTimer,&QTimer::timeout,this,&LibraryModel::refreshModHealth);
    connect(m_client,&BackendClient::healthReady,this,[this](const QVariantMap &v){
        if (!nativeMode() || m_viewedProfileId != m_activeProfileId) return;
        m_health=v; m_rows->setHealth(v.value("report").toMap());
        if(v.value("running").toBool())m_healthTimer.start();else m_healthTimer.stop();
        reconcileSelection();
        emit selectionChanged();
        emit healthChanged();
        if (!v.value("running").toBool() && v.value("report").toMap().isEmpty() && !v.contains("error"))
            QTimer::singleShot(0, this, [this] { checkModHealth(); });
    });
    connect(m_client,&BackendClient::rejected,this,[this](BackendClient::Call c,const QString &kind,const QString &message){if(c==BackendClient::Call::HealthCheck||c==BackendClient::Call::HealthGet){m_healthTimer.stop();m_health={{"running",false},{"error",QVariantMap{{"kind",kind},{"message",message}}}};emit healthChanged();}});
    connect(m_client, &BackendClient::completed, this, [this](BackendClient::Call call) {
        if(remoteMarkerPending() || recoveryWrapperPending())return;
        if (m_autoCycle && (call == BackendClient::Call::SetEnabled || call == BackendClient::Call::SetModsEnabled)) {
            m_autoInFlight.clear(); m_autoInFlightBulk.clear();
            if (m_autoDue && m_automaticApply) { m_autoDue = false; m_autoApplyTimer.start(); }
        }
        if (call == BackendClient::Call::FontsCommit) return; // Fresh viewed-profile confirmation owns marker completion.
        if (call == BackendClient::Call::Recover) {
            m_recoveryRequired = false; checkOriginalAfterRecovery(); return;
        }
        if(LaunchData::call(call)) return;
        const auto done=[this,call] { m_outcomeUnknown = false; setBusy(false, false, tr("Operation completed and verified."));
            if(call==BackendClient::Call::PerformanceSelect && m_viewedProfileId==m_activeProfileId) {
                scheduleAutomaticApply(m_performanceApplyNeedsReview);
                m_performanceApplyNeedsReview = true;
            }
            if((call==BackendClient::Call::SetFixEnabled || call==BackendClient::Call::RemoveModsFromProfile || call==BackendClient::Call::DeleteMods)
                && m_viewedProfileId==m_activeProfileId) scheduleAutomaticApply(false);
        };
        if(singleCommitEdit(call)) { m_editInFlight=false; done(); } else clearMarker(done);
    });
    connect(m_client, &BackendClient::unknownOutcome, this, [this](const QString &message) {
        // No marker and no unknown outcome for an edit: the helper either
        // committed it or did not. Editing waits for the next snapshot.
        const bool lostEdit=std::exchange(m_editInFlight,false) && !m_shutdownRequested;
        if(lostEdit)m_editLost=true;
        if (m_autoCycle) { m_autoBlocked = true; m_autoApplyTimer.stop(); m_closeAfterToggles = false; emit shutdownChanged(); }
        if(remoteMarkerPending() || recoveryWrapperPending()) {m_remoteToken.clear();m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false,false,tr("Recovery remains unresolved. Its exact identity is retained."));setError(message);scheduleRemoteReconcile(1000);return;}
        if(m_remoteStage==RemoteStage::Inspect || m_remoteStage==RemoteStage::RecoverySnapshot) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(message);return;
        }
        if(bootstrapPending() || m_bootstrapMarkerFileBusy) {
            m_bootstrapConfirming=false;m_outcomeUnknown=true;m_reconciling=false;setLoading(false);
            setBusy(false,false,tr("Library creation may have completed. Reconnect, then check creation with the same operation."));setError(message);return;
        }
        if(m_client->launchOwned()||!m_launch->m_binding.isEmpty()||m_launch->settingsSavePending()||m_launch->m_intent!=LaunchController::Intent::None) {
            m_launch->transportLost(message);m_reconciling=false;
            setBusy(false);setLoading(false);setError(message);return;
        }
        if(lostEdit) {
            // Reconnect and show what the helper saved.
            m_options->rejected(tr("The connection was lost. Review setup changes again."));
            m_fonts->rejected(tr("The connection was lost. Review fonts again."));
            m_reconciling=false;setLoading(false);invalidateReviews();
            if(m_switching) {
                // Leaving this source cancels its unsaved automatic work.
                m_autoApplyTimer.stop(); m_autoCycle=false; m_autoBlocked=false;
                m_autoDue=false; m_autoPreview=false; m_autoForceReview=false;
                m_autoToggles.clear(); m_autoBulkIds.clear(); m_autoInFlight.clear(); m_autoInFlightBulk.clear();
                m_autoRoot.clear(); m_autoProfile.clear();
                setBusy(false); advanceLoad(); return;
            }
            setBusy(true,false,tr("The connection was lost while saving. Loading the current state…"));
            refresh();return;
        }
        if(m_marker.value("method").toInt()==int(BackendClient::Call::ProfileEditCommit))m_options->finished(false,tr("The operation may have completed. Check status or recover before editing."));
        else m_options->rejected(tr("Check status or recover before reviewing setup changes again."));
        m_fonts->rejected(tr("The font operation may have completed. Check status or recover before reviewing again."));
        m_outcomeUnknown = true; m_reconciling = false; setLoading(false); invalidateReviews();
        setBusy(false, false, tr("Operation outcome is unknown. Refresh to reconnect, then check status or recover before editing.")); setError(message);
    });
    connect(m_client, &BackendClient::rejected, this, [this](BackendClient::Call call, const QString &kind, const QString &message) {
        const auto errorGeneration=m_errorGeneration;
        const bool edit=singleCommitEdit(call);
        if(edit)m_editInFlight=false;
        if(call==BackendClient::Call::HeroDetect || call==BackendClient::Call::PerformanceGet || LaunchData::call(call) || call == BackendClient::Call::ModUpdatesGet || call == BackendClient::Call::HealthCheck || call == BackendClient::Call::HealthGet || (int(call)>=181 && int(call)<=184)) return;
        if (m_autoCycle && (call == BackendClient::Call::SetEnabled || call == BackendClient::Call::SetModsEnabled
                            || call == BackendClient::Call::ApplyPreview || call == BackendClient::Call::Apply)) {
            for (auto it = m_autoInFlight.cbegin(); it != m_autoInFlight.cend(); ++it)
                if (!m_autoToggles.contains(it.key())) { m_autoToggles.insert(it.key(), it.value()); if (m_autoInFlightBulk.contains(it.key())) m_autoBulkIds.insert(it.key()); }
            m_autoInFlight.clear(); m_autoInFlightBulk.clear(); m_autoBlocked = true; m_autoPreview = false; m_autoApplyTimer.stop();
            m_closeAfterToggles = false; emit shutdownChanged();
            if (m_autoToggles.isEmpty()) m_autoCycle = false;
            if (call == BackendClient::Call::ApplyPreview)
                QTimer::singleShot(0, this, [this] { emit automaticReviewRequired(); });
        }
        if((recoveryWrapperPending() || remoteMarkerPending()) && m_remoteStage==RemoteStage::RecoveryView
            && call==BackendClient::Call::Snapshot && kind=="unknownProfile" && !m_recoveryViewProfile.isEmpty()) {
            m_recoveryViewProfile.clear();m_client->requestSnapshot(m_statePath,{},true);return;
        }
        if((recoveryWrapperPending() || remoteMarkerPending()) && call==BackendClient::Call::Recover) {
            setError(kind+": "+message);
            if(m_remoteStage==RemoteStage::RecoveryReplay && m_remoteOutcome.value("status")=="failed") {
                const auto failure=m_remoteOutcome.value("error").toMap();
                if(failure.value("kind").toString()==kind && failure.value("message").toString()==message) {recoveryWrapperTerminal();return;}
            } else if(m_remoteStage==RemoteStage::Recovery) {
                m_remoteStage=RemoteStage::RecoveryOperation;
                const auto id=remoteMarkerPending()?m_marker.value("recoveryId").toString():m_marker.value("operationId").toString();
                m_client->nativeRequest(BackendClient::Call::Operation,{{"operationId",id}},{{"recoveryOperation",true}});
                return;
            }
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);return;
        }
        if((recoveryWrapperPending() || m_remoteStage==RemoteStage::Inspect || m_remoteStage==RemoteStage::RecoverySnapshot)
            && (call==BackendClient::Call::RecoveryInspect || call==BackendClient::Call::RecoverySnapshot || call==BackendClient::Call::Operation || call==BackendClient::Call::Snapshot)) {
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setLoading(false);setBusy(false);
            setError(replacementRecoveryContext()?tr("The saved download could not be checked. Try again."):kind+": "+message);if(recoveryWrapperPending() || remoteMarkerPending())scheduleRemoteReconcile(1500);return;
        }
        if(remoteMarkerPending() && (call==BackendClient::Call::Recover || call==BackendClient::Call::Operation || call==BackendClient::Call::Snapshot || call==BackendClient::Call::RecoverySnapshot)) {
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;if(kind=="recoveryRequired")m_recoveryRequired=true;
            setLoading(false);setBusy(false,false,tr("Download installation is unresolved; the saved identity is retained."));
            if(kind!="outcomeUnknown" && kind!="remoteBusy" && kind!="launchBusy")setError(replacementRecoveryContext()?tr("The saved download could not be checked. Try again."):kind+": "+message);
            scheduleRemoteReconcile(1500);return;
        }
        if(call==BackendClient::Call::LibraryInspect) {
            m_inspectState="unavailable";m_inspectRoot.clear();m_inspectEligible=false;
            m_inspectReason=tr("Cannot inspect the native data root. Check the helper and try again.");
            emit nativeChanged();return;
        }
        if(call==BackendClient::Call::LibraryInitialize && bootstrapPending()) {
            if(kind=="nativeAlreadyInitialized" && m_bootstrapRetry){confirmBootstrap(false);return;}
            m_bootstrapConfirming=false;m_reconciling=false;m_outcomeUnknown=true;
            setBusy(false,false,kind=="nativeAlreadyInitialized" ? tr("Another library already exists. Check creation to reconcile this operation.") : tr("Creation is unresolved. Check creation to retry the same operation."));
            setError(kind+": "+message);return;
        }
        if(call==BackendClient::Call::Snapshot && m_bootstrapConfirming) {
            m_bootstrapConfirming=false;m_reconciling=false;m_outcomeUnknown=true;setLoading(false);
            setBusy(false,false,tr("The current native library could not be confirmed. Check creation again."));setError(kind+": "+message);return;
        }
        if((m_client->launchOwned()||m_launch->writeGate())&&(kind=="launchBusy"||kind=="nativeBusy")&&(call==BackendClient::Call::Snapshot||call==BackendClient::Call::RecoverySnapshot)) {setLoading(false);return;}
        const auto error = kind + ": " + message + (m_autoBlocked && !m_autoToggles.isEmpty()
            ? tr(" Some queued inclusion choices were not saved; change a choice again to retry.") : QString());
        if(call==BackendClient::Call::ProfileEditPreview) m_options->rejected(error);
        if(call==BackendClient::Call::ProfileEditCommit) m_options->finished(false,error);
        if (call == BackendClient::Call::FontsPreview || call == BackendClient::Call::FontsCommit) m_fonts->rejected(error);
        m_reconciling = false; setLoading(false); invalidateReviews();
        if (call == BackendClient::Call::Snapshot && kind == "unknownProfile" && m_refreshFallback) {
            m_refreshFallback = false; m_profileFallbackWarning = true; m_viewedProfileId.clear();
            m_client->requestSnapshot(m_statePath, {}); setLoading(true); return;
        }
        if (call == BackendClient::Call::RecoverySnapshot && kind == "nativeNotImported") {
            m_bisect.clear();m_launchFingerprint.clear();
    m_healthTimer.stop();m_health.clear();m_rows->setHealth({});emit healthChanged();
            m_revision = "0"; m_haveRecoveryContext = true; setBusy(false); reconcileOperation();
        } else if (call == BackendClient::Call::Recover && kind == "nativeNotImported") {
            // Recover acquired the root lock and reconciled import artifacts.
            // Look up the original identity before discarding frontend context.
            m_recoveryRequired = false; checkOriginalAfterRecovery();
        } else if (kind == "recoveryRequired" || call == BackendClient::Call::Recover) {
            m_outcomeUnknown = true;
            if (kind == "recoveryRequired") m_recoveryRequired = true;
            setBusy(false, false, tr("Recovery has not completed. Resolve the error and try Recover again.")); setError(error);
        } else if (edit || (!m_marker.isEmpty() && call != BackendClient::Call::Operation && call != BackendClient::Call::Snapshot && call != BackendClient::Call::RecoverySnapshot)) {
            const auto done=[this, error, kind, errorGeneration] {
                m_outcomeUnknown = false; setBusy(false); setErrorIfCurrent(errorGeneration,error); emit selectionChanged();
                if (kind == "staleRevision") refreshKeepingError(true);
            };
            // A refused edit left no marker. A leftover marker is never cleared by it.
            if(edit) { if(m_marker.isEmpty()) done(); else { setBusy(false); setError(error); emit selectionChanged(); } }
            else clearMarker(done);
        } else { setBusy(false); setError(error); emit selectionChanged(); }
    });
    connect(m_client, &BackendClient::operationReady, this, [this](const QVariantMap &outcome) {
        if(recoveryWrapperPending() || (remoteMarkerPending() && (m_remoteStage==RemoteStage::RecoveryOperation || m_remoteStage==RemoteStage::OriginalOperation))){recoveryWrapperOperation(outcome);return;}
        if(remoteMarkerPending()){remoteOperation(outcome);return;}
        if(bootstrapPending() || m_bootstrapMarkerFileBusy)return;
        m_reconciling = false;
        const auto id = outcome.value("operationId").toString();
        if (id != m_marker.value("operationId").toString()) {
            setBusy(false); setError(tr("Operation lookup returned a different operation ID.")); return;
        }
        const auto status = outcome.value("status").toString();
        const auto failure = outcome.value("error").toMap();
        const auto method=static_cast<BackendClient::Call>(m_marker.value("method",-1).toInt());
        const bool recoveredUnknown=status=="unknown"&&m_recoveryBarrier&&(method==BackendClient::Call::OptionsAttach||method==BackendClient::Call::ProfileEditCommit||method==BackendClient::Call::Apply);
        if((status=="succeeded"||status=="failed"||recoveredUnknown)&&!m_recoveryRequired) {
            switch(method) {
            case BackendClient::Call::LocalCommit: m_confirmationTarget=ConfirmationTarget::Import;break;
            case BackendClient::Call::FontsCommit: m_confirmationTarget=ConfirmationTarget::Fonts;break;
            case BackendClient::Call::OptionsAttach: m_confirmationTarget=ConfirmationTarget::OptionsAttach;break;
            case BackendClient::Call::ProfileEditCommit: m_confirmationTarget=ConfirmationTarget::ProfileEdit;break;
            case BackendClient::Call::Apply: m_confirmationTarget=ConfirmationTarget::Apply;break;
            default: m_confirmationTarget=ConfirmationTarget::None;break;
            }
            if(m_confirmationTarget!=ConfirmationTarget::None) {
                const auto confirmed=outcome.value("confirmedRevision").toString();
                if(status=="succeeded"&&confirmed.toULongLong()>m_revision.toULongLong())m_revision=confirmed;
                confirmLocalView(status=="succeeded",status=="failed" ? failure.value("kind").toString()+": "+failure.value("message").toString() : recoveredUnknown ? tr("No pending operation remains. Review the desired setup again if needed.") : QString());return;
            }
        }
        if(method==BackendClient::Call::Apply) {
            m_outcomeUnknown=true;if(status=="recoveryRequired")m_recoveryRequired=true;
            setBusy(false,false,tr("Apply is unresolved. Check status or recover before editing."));return;
        }
        // Only the original operation's terminal result can settle generic saved context.
        if ((status == "succeeded" || status == "failed") && !m_recoveryRequired) {
            const auto detail = status == "failed" ? failure.value("kind").toString() + ": " + failure.value("message").toString() : QString();
            const auto errorGeneration=m_errorGeneration;
            clearMarker([this, detail, errorGeneration] {
                m_recoveryBarrier = false; m_outcomeUnknown = false; setBusy(false, false, tr("Saved operation outcome reconciled."));
                if(!detail.isEmpty())setErrorIfCurrent(errorGeneration,detail);
                refreshKeepingError(!detail.isEmpty());
            });
        } else {
            m_outcomeUnknown = true; if (status == "recoveryRequired") m_recoveryRequired = true;
            setBusy(false, false, tr("The result is unresolved. Recover checks for unfinished work before you continue editing."));
        }
    });
    connect(m_client, &BackendClient::failed, this, [this](const QString &message) {
        const auto failedErrorGeneration=m_errorGeneration;
        if(remoteMarkerPending() || recoveryWrapperPending()) {
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);setLoading(false);setError(message);scheduleRemoteReconcile(1500);return;
        }
        if(m_remoteStage==RemoteStage::Inspect || m_remoteStage==RemoteStage::RecoverySnapshot) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setLoading(false);setError(message);return;
        }
        if(!m_client->isRunning() || m_client->retiring()) {
            m_inspectState="unavailable";m_inspectEligible=false;m_inspectRoot.clear();
            m_inspectReason=tr("The helper is unavailable. Refresh to reconnect and inspect the native data root.");
            emit nativeChanged();
        }
        if(bootstrapPending() || m_bootstrapMarkerFileBusy) {
            m_bootstrapConfirming=false;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);setLoading(false);
            setError(message);emit nativeChanged();return;
        }
        m_options->rejected(message);
        m_fonts->rejected(message);
        if (m_refreshFallback && message.startsWith(QStringLiteral("unknownProfile:"))) {
            // The viewed profile was removed externally. Rediscover the default view once.
            m_refreshFallback = false; m_profileFallbackWarning = true;
            m_client->start(m_helperPath, m_statePath, m_dataDir, m_defaultMode);
            return;
        }
        m_refreshFallback = false; m_profileFallbackWarning = false;
        const auto preserved=std::exchange(m_preservedError,{});
        const auto errorGeneration=m_preservedErrorGeneration;
        m_reconciling = false; setBusy(false);
        if(preserved.isEmpty() || !setErrorIfCurrent(errorGeneration,preserved))setErrorIfCurrent(failedErrorGeneration,message);
        setLoading(false);
        if (!m_marker.isEmpty() && m_client->isRunning()) QTimer::singleShot(0, this, [this] { reconcileOperation(); });
    });
    connect(m_client,&BackendClient::fontsCommitted,this,[this](const LibrarySnapshot &snapshot) {
        m_revision=snapshot.revision; m_confirmationTarget=ConfirmationTarget::Fonts; confirmLocalView(true);
    });
    connect(m_client,&BackendClient::profileEditCommitted,this,[this](const LibrarySnapshot &snapshot) {
        m_revision=snapshot.revision;m_confirmationTarget=ConfirmationTarget::ProfileEdit;confirmLocalView(true);
    });
    connect(m_client,&BackendClient::applyCommitted,this,[this](const LibrarySnapshot &snapshot) {
        m_revision=snapshot.revision;m_confirmationTarget=ConfirmationTarget::Apply;confirmLocalView(true);
    });
    connect(m_client,&BackendClient::operationFontOffers,this,[this](const QString &id,const QString &game,const QVariantMap &mods) {
        if(m_marker.value("method").toInt()==int(BackendClient::Call::LocalCommit) && id==m_marker.value("operationId").toString()) {
            const auto mod=m_marker.value("modId").toString();
            m_fonts->seedOfferData(game,mod,mods.value(mod).toMap());
        }
    });
}
QVariantList LibraryModel::recentMods() const {
    return m_homeRecent;
}
QVariantList LibraryModel::enabledMods() const {
    return m_homeEnabled;
}
void LibraryModel::refreshHomeRecent() {
    const auto next = m_rows->homeRecent();
    if (next != m_homeRecent) { m_homeRecent = next; emit recentModsChanged(); }
}
void LibraryModel::refreshHomeEnabled() {
    const auto next = m_activeProfileId == m_viewedProfileId ? m_rows->homeEnabled() : QVariantList{};
    if (next != m_homeEnabled) { m_homeEnabled = next; emit enabledModsChanged(); }
}
int LibraryModel::totalCount() const { return m_rows->rowCount(); }
int LibraryModel::enabledCount() const { return m_rows->enabledCount(); }
QStringList LibraryModel::categories() const { return m_rows->categories(); }
QStringList LibraryModel::heroes() const { return m_rows->heroes(); }
QStringList LibraryModel::types() const { return m_rows->types(); }

void LibraryModel::setLoading(bool loading) {
    if (m_loading == loading) return;
    m_loading = loading; emit loadingChanged(); emit nativeChanged();
    // A reconnect after a lost edit ended without a snapshot: stop waiting so
    // Reload is available. Editing stays closed until a snapshot arrives.
    if (!loading && m_editLost && m_busy && !m_writeBusy) setBusy(false);
    if (!loading) QTimer::singleShot(0, this, &LibraryModel::advanceAutoApply);
}
quint64 LibraryModel::setError(const QString &error) {
    const auto generation=++m_errorGeneration;
    if (m_error == error) return generation;
    m_error = error; emit errorChanged();
    return generation;
}
bool LibraryModel::setErrorIfCurrent(quint64 generation,const QString &error) {
    if(m_errorGeneration!=generation)return false;
    setError(error);
    return true;
}
void LibraryModel::clearErrorIfCurrent(quint64 generation) {
    setErrorIfCurrent(generation,{});
}
void LibraryModel::clearSource() {
    if(!m_updateResourceKey.isEmpty())m_catalog->cancelLibraryMedia(m_updateResourceKey);
    m_updateResourceKey.clear();m_updateBinding.clear();m_updateResources.clear();m_updateReviews.clear();m_updateBatchTargets.clear();m_updateFiles.clear();m_updateDigests.clear();m_updateActiveId.clear();m_updateActiveJob.clear();m_updateBatchConfirmed=false;m_updateNeedsAttention=false;m_updatesError.clear();
    m_updateReviewActive=false;m_updatesChecking=false;m_updatesReadPending=false;m_updatesFinish=false;
    m_modUpdates.clear();m_lastUpdateCheck={};m_rows->setUpdates({});emit updatesChanged();
    m_bisect.clear();m_launchFingerprint.clear();
    m_healthTimer.stop();m_health.clear();m_rows->setHealth({});emit healthChanged();
    m_fontOwnerIds.clear(); m_fontOwners.clear(); m_fontOwnerRows.clear(); m_canUndoFonts=false;
    m_sourcePath.clear(); m_activeProfileId.clear(); m_viewedProfileId.clear();
    m_warnings.clear(); m_warningCodes.clear(); m_profiles.clear(); emit sourceChanged();
}
void LibraryModel::notifyRowsChanged() {
    reconcileSelection();
    emit totalCountChanged(); emit enabledCountChanged(); emit categoriesChanged();
    // Details can change on refresh even when the stable selected ID is unchanged.
    emit selectionChanged();
}
void LibraryModel::resetLibraryMediaContext(){
    m_selectedMediaRow.clear();m_selectedMediaSubmission={};
    m_media->clear();
}
void LibraryModel::updateLibraryMediaContext(){
    if(m_shutdownRequested||m_switching||!m_nativeMode||m_demoMode||m_dataDir.isEmpty()){
        resetLibraryMediaContext();
        return;
    }
    m_media->setContext(m_dataDir,m_loadEpoch,m_libraryMediaActive);
    updateSelectedMediaDemand();
}
void LibraryModel::setLibraryMediaActive(bool active){
    if(m_libraryMediaActive==active)return;m_libraryMediaActive=active;
    updateLibraryMediaContext();
}
void LibraryModel::demandModMedia(const QString &consumer,const QString &modId,bool wanted){
    if(!wanted){m_media->demand(consumer,modId,{},false);return;}
    if(!m_libraryMediaActive||m_switching||m_shutdownRequested||!m_nativeMode||m_demoMode)return;
    const auto key=m_rows->mediaSubmission(modId);if(key)m_media->demand(consumer,modId,*key,true);
}
void LibraryModel::updateSelectedMediaDemand(){
    const auto key=(m_libraryMediaActive&&!m_dataDir.isEmpty()&&!m_switching&&!m_shutdownRequested&&m_nativeMode&&!m_demoMode)
        ?m_rows->mediaSubmission(m_selectedId):std::optional<SubmissionKey>{};
    if(!m_selectedMediaRow.isEmpty()&&(!key||m_selectedMediaRow!=m_selectedId||m_selectedMediaSubmission!=*key)){
        m_media->demand(QStringLiteral("selected"),m_selectedMediaRow,m_selectedMediaSubmission,false);m_selectedMediaRow.clear();m_selectedMediaSubmission={};
    }
    if(key){m_selectedMediaRow=m_selectedId;m_selectedMediaSubmission=*key;m_media->demand(QStringLiteral("selected"),m_selectedMediaRow,*key,true);}
}
void LibraryModel::loadSource(const QString &helperPath, const QString &statePath, const QString &dataDir) {
    if(m_shutdownRequested)return;m_deferredLoads.enqueue({helperPath,statePath,dataDir,false,false,0});advanceLoad();
}
void LibraryModel::loadDemoData(int rows) {
    if(m_shutdownRequested)return;m_deferredLoads.enqueue({{}, {}, {}, false,true,rows});advanceLoad();
}
void LibraryModel::performDemoData(int rows) {
    ++m_loadEpoch; m_recoveryBarrier = false; m_outcomeUnknown = false; m_marker.clear(); m_recoveryRequired = false; m_editInFlight = false;
    if(std::exchange(m_editLost,false)){m_busy=false;m_status.clear();}
    m_nativeMode = false; m_inspectState="unavailable";m_inspectEligible=false;m_inspectRoot.clear();invalidateReviews();
    m_client->stop();m_remote->loadRoot({});
    m_refreshFallback = false; m_profileFallbackWarning = false; m_demoMode = true; m_launch->detachRoot(); emit modeChanged(); emit nativeChanged(); setLoading(false); setError({}); clearSource();
    m_activeProfileId = m_viewedProfileId = QStringLiteral("default");
    m_profiles = {QVariantMap{{"id", "default"}, {"name", "Default"},
                              {"modCount", std::clamp(rows, 0, 100000)}}};
    emit sourceChanged();
    const QStringList heroes{"Abrams", "Bebop", "Dynamo", "Haze", "Ivy", "Lash", "McGinnis", "Seven", "Vindicta", "Wraith"};
    const QStringList titles{"Classic finish", "Quiet footsteps", "Minimal HUD", "Field equipment", "Alternate voice", "Clear ability icons"};
    const QStringList kinds{"Skins", "Sounds", "Interface"};
    QList<LibraryMod> mods;
    rows = std::clamp(rows, 0, 100000); mods.reserve(rows);
    for (int i = 0; i < rows; ++i) {
        LibraryMod mod;
        mod.id = QStringLiteral("demo_%1").arg(i + 1);
        mod.title = QStringLiteral("%1 %2").arg(titles[i % titles.size()]).arg(i / titles.size() + 1);
        mod.author = QStringLiteral("Demo Author"); mod.hero = heroes[i % heroes.size()]; mod.kind = kinds[i % kinds.size()];
        mod.status = QStringLiteral("installed"); mod.enabled = i % 5 != 0; mod.installOrder = i;
        // UI checks supply fixture images; alternate their order per row.
        if (const auto first = qEnvironmentVariable("UI_PREVIEW_FIRST"), second = qEnvironmentVariable("UI_PREVIEW_SECOND");
            !first.isEmpty() && !second.isEmpty())
            mod.images = i % 2 ? QStringList{second, first} : QStringList{first, second};
        if (qEnvironmentVariableIsSet("UI_PREVIEW_ALTERNATE_HIDDEN")) mod.previewHidden = i % 2;
        mods.append(std::move(mod));
    }
    m_heroQueue.clear();m_heroRevision=m_revision;
    if(m_nativeMode)for(const auto &mod:mods)if(mod.heroDetectionEligible)m_heroQueue.append(mod.id);
    m_rows->replace(std::move(mods));
    QTimer::singleShot(0,this,&LibraryModel::detectNextHero); notifyRowsChanged();
    if(!m_deferredLoads.isEmpty())QTimer::singleShot(0,this,&LibraryModel::advanceLoad);
}
bool LibraryModel::canRefresh() const {
    return !m_shutdownRequested && !m_switching && !m_demoMode && !m_helperPath.isEmpty() && (!m_busy || m_editLost);
}
void LibraryModel::refresh() {
    if (!canRefresh()) return;
    if(m_client->retiring()){m_restartAfterDrain=true;return;}
    setLoading(true);
    m_refreshFallback = !m_viewedProfileId.isEmpty(); m_profileFallbackWarning = false;
    invalidateReviews();
    if (!m_client->isRunning()) m_client->start(m_helperPath, m_statePath, m_dataDir, m_nativeMode || (m_defaultMode && !bootstrapPending()));
    m_client->requestSnapshot(m_statePath, m_viewedProfileId, m_defaultMode && !m_nativeMode && !bootstrapPending());
}
bool LibraryModel::profileBrowseAllowed() const {
    return !m_autoCycle&&!m_shutdownRequested&&!m_switching&&!m_client->retiring()&&!m_client->writing()&&!m_client->launchWriting()
        &&!m_demoMode&&!m_helperPath.isEmpty()&&!busy()&&!writeBusy()
        &&(!(m_outcomeUnknown||m_recoveryRequired)||m_launchOnlyRecovery||(bootstrapPending()&&!m_nativeMode));
}
bool LibraryModel::canBrowseProfiles() const { return !m_loading && (m_demoMode || profileBrowseAllowed()); }
void LibraryModel::viewProfile(const QString &id) {
    if (m_demoMode) {
        if (!canBrowseProfiles()) return;
        for (const auto &profile : m_profiles) {
            if (profile.toMap().value("id").toString() != id) continue;
            m_viewedProfileId = id;
            refreshHomeEnabled();
            emit sourceChanged();
            return;
        }
        return;
    }
    if (!profileBrowseAllowed() || id.isEmpty()) return;
    if (m_loading && !m_client->isRunning()) { m_initialProfile = id; return; }
    invalidateReviews();
    m_refreshFallback = false; m_profileFallbackWarning = false;
    setError({}); setLoading(true);
    if (!m_client->isRunning()) m_client->start(m_helperPath, m_statePath, m_dataDir, m_nativeMode || (m_defaultMode && !bootstrapPending()));
    m_requestedProfileId = id;
    m_client->requestSnapshot(m_statePath, id);
}
void LibraryModel::applySnapshot(const LibrarySnapshot &snapshot) {
    const auto snapshotErrorGeneration=m_errorGeneration;
    // A startup/native rediscovery is not evidence for the saved creation ID.
    // Keep any upstream view until same-ID reconciliation requests its own read.
    if(bootstrapPending() && snapshot.sourceKind=="native" && !m_bootstrapConfirming) {
        setLoading(false);m_outcomeUnknown=true;emit nativeChanged();return;
    }
    if((recoveryWrapperPending() || remoteMarkerPending()) && m_remoteStage==RemoteStage::RecoveryView) {
        const auto minimum=(recoveryWrapperPending() && m_remoteOutcome.value("status")=="succeeded"
            ? m_remoteOutcome.value("confirmedRevision") : m_marker.value(remoteMarkerPending()?"recoveryExpectedRevision":"expectedRevision")).toString().toULongLong();
        if(m_client->nativeRoot()!=m_dataDir || snapshot.sourceKind!="native"
            || (!m_recoveryViewProfile.isEmpty() && snapshot.selectedProfileId!=m_recoveryViewProfile)
            || (m_remoteOutcome.value("status")=="succeeded" && snapshot.revision.toULongLong()<minimum)) {
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setLoading(false);setBusy(false);
            setError(tr("The current view does not confirm the saved recovery."));scheduleRemoteReconcile(1500);return;
        }
    }
    // A recovery confirmation reads native state as evidence, but an explicit
    // upstream source remains the displayed read-only library.
    if(recoveryWrapperPending() && m_remoteStage==RemoteStage::RecoveryView && !m_defaultMode && !m_nativeMode) {
        m_revision=snapshot.revision;m_haveRecoveryContext=true;
        setLoading(false);
        finishRecoveryWrapper(snapshot.recoveryRequired || m_remoteOutcome.value("status")!="succeeded",false,snapshotErrorGeneration);
        return;
    }
    if(remoteMarkerPending() && m_remoteStage==RemoteStage::Snapshot) {
        const auto expected=m_marker.value("expectedRevision").toString().toULongLong();
        const bool success=m_remoteOutcome.value("status")=="succeeded";
        const auto floor=expected+quint64(success);
        if(m_client->nativeRoot()!=m_dataDir || snapshot.sourceKind!="native" || snapshot.selectedProfileId!=m_marker.value("profileId").toString()
            || snapshot.revision.toULongLong()<floor || snapshot.recoveryRequired) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setLoading(false);setBusy(false);
            setError(tr("The current profile view does not yet confirm the saved download outcome."));scheduleRemoteReconcile(1500);return;
        }
    }
    m_refreshFallback = false;
    if (m_localConfirmation && snapshot.revision.toULongLong() < m_revision.toULongLong()) {
        m_outcomeUnknown = true; setLoading(false); setBusy(false); setError(tr("The fresh profile snapshot predates the confirmed operation. Refresh to verify again.")); return;
    }
    if(m_viewedProfileId!=snapshot.selectedProfileId||m_sourcePath!=snapshot.sourcePath||m_nativeMode!=(snapshot.sourceKind=="native"))resetLibraryMediaContext();
    if (snapshot.selectedProfileId == m_requestedProfileId) m_requestedProfileId.clear();
    if (m_revision != snapshot.revision || m_viewedProfileId != snapshot.selectedProfileId) invalidateReviews();
    m_fixAutomatically=snapshot.fixAutomatically;
    m_healthTimer.stop();m_health.clear();m_rows->setHealth({});emit healthChanged();
    m_nativeMode = snapshot.sourceKind == "native"; m_haveRecoveryContext = m_nativeMode; m_revision = m_nativeMode ? snapshot.revision : "0";
    m_bisect = snapshot.bisect; m_launchFingerprint = snapshot.launchFingerprint;
    m_gamePath = snapshot.gamePath; m_appliedProfileId = snapshot.appliedProfileId;
    m_pendingChanges = snapshot.pendingChanges; m_pendingStatus=snapshot.pendingStatus;m_pendingReason=snapshot.pendingReason; m_canRestore = snapshot.canRestore;
    m_recoveryRequired = snapshot.recoveryRequired;
    // The helper reports a launch barrier only when the Steam handoff is the sole unfinished work.
    m_launchOnlyRecovery=m_marker.isEmpty()&&snapshot.recoveryRequired&&LaunchData::barrier(snapshot.launchBarrier,m_dataDir);
    m_canUndoFonts = snapshot.canUndoFonts;
    m_fontOwnerIds = snapshot.fontOwnerIds;
    m_fontOwners = snapshot.fontOwners;
    m_fontOwnerRows = snapshot.fontOwnerRows;
    m_sourcePath = snapshot.sourcePath; m_activeProfileId = snapshot.activeProfileId;
    m_viewedProfileId = snapshot.selectedProfileId; m_profiles = snapshot.profiles; m_warnings = snapshot.warnings; m_warningCodes = snapshot.warningCodes;
    if (m_profileFallbackWarning) {
        m_warnings.append(tr("The previously viewed profile was removed. Showing an available profile without activating it."));
        m_profileFallbackWarning = false;
    }
    // The paired helper emits every profile row in ordered_mod_ids order.
    // Preserve that canonical sequence before installing the filtered proxy.
    m_fullProfileOrder.clear();
    for(const auto &mod:snapshot.mods)m_fullProfileOrder.append(QVariantMap{{"id",mod.id},{"title",mod.title},{"enabled",mod.enabled},{"available",mod.available}});
    auto mods = snapshot.mods;
    for (auto &mod : mods) if (!mod.localArtwork.isEmpty()) mod.localPreview = LocalArtworkProvider::admitOwned(snapshot.storageRoot.isEmpty()?m_dataDir:snapshot.storageRoot,QJsonObject::fromVariantMap(mod.localArtwork));
    m_heroQueue.clear();m_heroRevision=m_revision;
    if(m_nativeMode)for(const auto &mod:mods)if(mod.heroDetectionEligible)m_heroQueue.append(mod.id);
    m_rows->replace(std::move(mods));
    const bool lostEdit=std::exchange(m_editLost,false);
    if(lostEdit) {
        // This snapshot is what the helper saved. Choices it does not hold were
        // not saved; drop them so every row shows the current state.
        bool unsaved=!m_autoToggles.isEmpty();
        for (auto it = m_autoInFlight.cbegin(); it != m_autoInFlight.cend(); ++it) {
            const auto mod = modDetails(it.key());
            if (!mod.isEmpty() && mod.value("enabled").toBool() != it.value()) unsaved = true;
        }
        m_autoToggles.clear(); m_autoBulkIds.clear(); m_autoInFlight.clear(); m_autoInFlightBulk.clear();
        // Automatic Apply continues from the saved state, with a review.
        if (m_autoCycle) { m_autoBlocked = false; m_autoForceReview = true; m_autoDue = true; }
        m_warnings.prepend(unsaved ? tr("The connection was lost while saving. This is the current state. Some choices were not saved; change them again.")
                                  : tr("The connection was lost while saving. This is the current state."));
    }
    if (m_autoCycle && m_autoProfile == snapshot.selectedProfileId && m_autoRoot == m_dataDir) {
        const auto show = [this](const QString &id, bool enabled) {
            const auto mod = modDetails(id);
            if (!mod.isEmpty() && (!enabled || mod.value("selectionReady", mod.value("available")).toBool())) m_rows->setEnabled(id, enabled);
        };
        for (auto it = m_autoInFlight.cbegin(); it != m_autoInFlight.cend(); ++it) show(it.key(), it.value());
        for (auto it = m_autoToggles.cbegin(); it != m_autoToggles.cend(); ++it) show(it.key(), it.value());
    }
    QTimer::singleShot(0,this,&LibraryModel::detectNextHero);
    m_media->retainRows([this](const QString &id){return m_nativeMode?m_rows->mediaSubmission(id):std::optional<SubmissionKey>{};});
    updateLibraryMediaContext();
    notifyRowsChanged(); emit sourceChanged(); emit modeChanged();
    QTimer::singleShot(0, this, &LibraryModel::refreshModHealth);
    const bool wasLoading = m_loading;
    setLoading(false);
    if (!wasLoading) emit nativeChanged();
    m_preservedError.clear();
    if(lostEdit) setBusy(false);
    if(m_bootstrapConfirming) {
        m_bootstrapConfirming=false;
        const bool created=m_bootstrapCreated;
        // Do not start a new clear once closing has begun. The durable identity
        // remains available for same-ID reconciliation on the next start.
        if(!reserveBootstrapMarkerWork()) {
            m_outcomeUnknown=true;m_reconciling=false;setBusy(false);return;
        }
        const auto errorGeneration=snapshotErrorGeneration;
        persistMarker({},[this,created,errorGeneration]{
            m_marker.clear();m_bootstrapMarkerDurable=false;m_outcomeUnknown=false;m_reconciling=false;
            m_inspectState="initialized";m_inspectEligible=false;m_inspectRoot=m_dataDir;
            m_inspectReason=tr("A native library already exists in this data root.");
            setBusy(false,false,created ? tr("Native library created.") : tr("Existing native library loaded."));
            clearErrorIfCurrent(errorGeneration);
        },[this]{
            // The empty file may have been committed before directory fsync
            // failed, so a retry must durably save the identity again.
            m_bootstrapMarkerDurable=false;m_outcomeUnknown=true;m_reconciling=false;
            setBusy(false,false,tr("The library was read, but its saved creation context could not be cleared. Check creation again."));
        },true);
        return;
    }
    if(recoveryWrapperPending() && m_remoteStage==RemoteStage::RecoveryView) {
        const bool success=m_remoteOutcome.value("status")=="succeeded";
        finishRecoveryWrapper(snapshot.recoveryRequired || !success,false,snapshotErrorGeneration);
        return;
    }
    if(remoteMarkerPending() && m_remoteStage==RemoteStage::RecoveryView) {
        auto original=m_marker;original.remove("recoveryId");original.remove("recoveryExpectedRevision");
        const auto failure=m_remoteOutcome.value("error").toMap();
        const auto detail=replacementMarkerPending()?tr("Recovery could not be completed. The original download is still being checked."):failure.value("kind").toString()+": "+failure.value("message").toString();
        // A newer helper failure during persistence must survive; only capture
        // the saved recovery detail when no later error replaced it.
        const auto errorGeneration=snapshotErrorGeneration;
        persistMarker(original,[this,detail,errorGeneration]{
            m_remoteMarkerDurable=true;m_remoteRecoveryTerminal=false;m_remoteStage=RemoteStage::Idle;m_remoteOutcome.clear();
            m_reconciling=false;m_outcomeUnknown=true;m_recoveryRequired=true;
            setBusy(false,false,tr("Recovery was rejected; the original download context is retained."));
            if(!detail.isEmpty())setErrorIfCurrent(errorGeneration,detail);
            scheduleRemoteReconcile();
        },[this]{
            m_remoteMarkerDurable=false;m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);
        });
        return;
    }
    m_launch->viewRead(snapshot.revision);
    if(remoteMarkerPending() && m_remoteStage==RemoteStage::Snapshot) {
        const bool success=m_remoteOutcome.value("status")=="succeeded";
        if(success && replacementMarkerPending()) {
            m_remoteConfirmationRevision=snapshot.revision;m_remoteStage=RemoteStage::Target;
            setBusy(true,false,tr("Checking the current file choices…"));m_remote->confirmReplacementTarget(m_marker,snapshot.revision);return;
        }
        finishRemoteOutcome(success);
        return;
    }
    if (m_localConfirmation) {
        m_localConfirmation = false;
        const auto success = m_localConfirmationSuccess; const auto error = m_localConfirmationError;
        const auto target=m_confirmationTarget;
        const auto attachedJob=target==ConfirmationTarget::OptionsAttach ? m_marker.value("jobId").toString() : QString();
        if(snapshot.recoveryRequired){m_localConfirmation=true;m_outcomeUnknown=true;setBusy(false);setError(tr("Recovery is required before the operation can be confirmed."));return;}
        const auto errorGeneration=m_localConfirmationErrorGeneration;
        clearMarker([this,success,error,target,attachedJob,errorGeneration] {
            m_outcomeUnknown=false;m_recoveryBarrier=false;m_confirmationTarget=ConfirmationTarget::None;
            setBusy(false,false,target==ConfirmationTarget::Apply ? success ? tr("Apply outcome verified.") : tr("Apply outcome checked. Review the current desired setup.") : tr("Saved operation outcome verified."));
            switch(target) {
            case ConfirmationTarget::Fonts:m_fonts->finished(success,error);break;
            case ConfirmationTarget::Import:m_localImport->commitConfirmed(success,error);if(success)m_fonts->publishOffer();break;
            case ConfirmationTarget::OptionsAttach:m_options->finished(success,error,attachedJob);break;
            case ConfirmationTarget::ProfileEdit:
                m_options->finished(success,error);
                if(success)scheduleAutomaticApply(m_editApplyNeedsReview);
                m_editApplyNeedsReview=true;break;
            case ConfirmationTarget::Apply:
                if (m_autoCycle && success && m_autoBlocked) { m_autoBlocked = false; m_autoForceReview = true; m_autoDue = true; }
                if (m_autoCycle && !success) m_autoBlocked = true;
                if (m_autoCycle && !success && m_autoToggles.isEmpty()) { m_autoCycle = false; emit nativeChanged(); }
                if (m_autoCycle && success && m_autoToggles.isEmpty() && !m_autoApplyTimer.isActive()) { m_autoCycle = false; emit nativeChanged(); }
                if(error.isEmpty())clearErrorIfCurrent(errorGeneration);else setErrorIfCurrent(errorGeneration,error);break;
            case ConfirmationTarget::None:break;
            }
        });
        return;
    }
    if (!bootstrapPending() && !m_bootstrapMarkerFileBusy && (!m_marker.isEmpty() || m_outcomeUnknown) && !m_writeBusy && !m_reconciling) QTimer::singleShot(0, this, [this] { reconcileOperation(); });
    if(!m_deferredLoads.isEmpty())QTimer::singleShot(0,this,&LibraryModel::advanceLoad);
}

void LibraryModel::setQuery(const QString &value) {
    if (m_query == value) return;
    beginFilterChange(); m_query = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit queryChanged();
}
void LibraryModel::setCategory(const QString &value) {
    if (m_category == value) return;
    beginFilterChange(); m_category = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit categoryChanged();
}
void LibraryModel::setHeroFilter(const QString &value) {
    if (m_heroFilter == value) return;
    beginFilterChange(); m_heroFilter = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit heroFilterChanged();
}
void LibraryModel::setTypeFilter(const QString &value) {
    if (m_typeFilter == value) return;
    beginFilterChange(); m_typeFilter = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit typeFilterChanged();
}
void LibraryModel::setEnabledOnly(bool value) {
    if (m_enabledOnly == value) return;
    beginFilterChange(); m_enabledOnly = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit enabledOnlyChanged();
}
void LibraryModel::setNeedsAttentionOnly(bool value) {
    if (m_needsAttentionOnly == value) return;
    beginFilterChange(); m_needsAttentionOnly = value; endFilterChange(Direction::Rows);
    reconcileSelection(); emit healthChanged();
}
bool LibraryModel::filterAcceptsRow(int row, const QModelIndex &parent) const {
    const auto index = m_rows->index(row, 0, parent);
    if (m_updatesOnly && m_rows->data(index, LibraryRows::ModUpdateStatus).toString() != "available") return false;
    if (m_needsAttentionOnly) {
        const auto status = m_rows->data(index, LibraryRows::ModHealthStatus).toString();
        if (status != "fixable" && status != "partlyFixable" && status != "partlyFixed" && status != "needsUpdate") return false;
    }
    if (m_enabledOnly && !m_rows->data(index, LibraryRows::ModEnabled).toBool()) return false;
    if (!m_category.isEmpty() && m_rows->data(index, LibraryRows::Kind).toString() != m_category && m_rows->data(index, LibraryRows::Hero).toString() != m_category) return false;
    if (!m_heroFilter.isEmpty() && m_rows->data(index, LibraryRows::Hero).toString() != m_heroFilter) return false;
    if (!m_typeFilter.isEmpty() && m_rows->data(index, LibraryRows::Kind).toString() != m_typeFilter) return false;
    const QString query = m_query.trimmed();
    if (query.isEmpty()) return true;
    for (const auto role : {LibraryRows::Title, LibraryRows::Hero, LibraryRows::Kind, LibraryRows::Author})
        if (m_rows->data(index, role).toString().contains(query, Qt::CaseInsensitive)) return true;
    return false;
}
QVariantMap LibraryModel::modDetails(const QString &id) const { return m_rows->details(id); }
bool LibraryModel::canStartBisect() const {
    return canSetEnabled() && !m_bisect.value("active").toBool() && m_viewedProfileId==m_activeProfileId;
}
bool LibraryModel::startBisect(const QStringList &modIds) {
    if(!canStartBisect())return false;
    QStringList ids=modIds;
    if(ids.isEmpty())for(const auto &v:m_fullProfileOrder){const auto m=v.toMap();if(m.value("enabled").toBool())ids.append(m.value("id").toString());}
    if(ids.isEmpty() || ids.size()>256)return false;
    QJsonArray array;QSet<QString> seen;
    for(const auto &id:ids){if(seen.contains(id) || !LaunchData::id(QJsonValue(id)))return false;seen.insert(id);array.append(id);}
    return mutate(int(BackendClient::Call::BisectStart),{{"profileId",m_viewedProfileId},{"modIds",array}},tr("Starting broken mod search…"));
}
bool LibraryModel::answerBisect(const QString &answer) {
    if(!m_bisect.value("active").toBool() || (m_bisect.value("done").toBool() && answer!="undo") || m_bisect.value("profileChanged").toBool() || !QStringList{"broken","fine","unsure","undo"}.contains(answer))return false;
    return mutate(int(BackendClient::Call::BisectAnswer),{{"answer",answer}},tr("Preparing the next test…"));
}
bool LibraryModel::finishBisect(bool keepDisabled) {
    if(!m_bisect.value("active").toBool() || (keepDisabled && !m_bisect.value("done").toBool()))return false;
    return mutate(int(BackendClient::Call::BisectFinish),{{"keepDisabled",keepDisabled}},tr("Restoring the original setup…"));
}
bool LibraryModel::canDeleteMods() const { return canSetEnabled() && !m_viewedProfileId.isEmpty(); }
bool LibraryModel::canBulkOperate() const {
    return canSetEnabled() && !m_viewedProfileId.isEmpty();
}
bool LibraryModel::queueAutoToggles(const QStringList &ids, bool enabled, bool bulk) {
    if (!(bulk ? canBulkToggle() : canToggleEnabled()) || ids.isEmpty() || ids.size() > 256) return false;
    QSet<QString> unique;
    for (const auto &id : ids) {
        const auto mod = modDetails(id);
        if (!LaunchData::id(QJsonValue(id)) || unique.contains(id) || (!bulk && (mod.isEmpty()
            || (enabled && !mod.value("selectionReady", mod.value("available")).toBool())))) {
            setError(tr("Choose a nonempty set of ready files in Files/options before including this mod.")); return false;
        }
        unique.insert(id);
    }
    const bool starting = !m_autoCycle;
    if (starting) {
        const bool applyingReviewed = m_marker.value("method").toInt() == int(BackendClient::Call::Apply);
        m_autoCycle = true; m_autoBlocked = false;
        m_autoRoot = m_dataDir; m_autoProfile = m_viewedProfileId;
        m_autoForceReview = (!applyingReviewed && m_pendingChanges) || m_pendingStatus == "unknown" || externalGameinfoChange();
    }
    if (m_autoBlocked) { m_autoBlocked = false; m_autoForceReview = true; }
    bool changed = false;
    for (const auto &id : ids) {
        const auto mod = modDetails(id);
        const bool eligible = !mod.isEmpty() && (!enabled || mod.value("selectionReady", mod.value("available")).toBool());
        const bool modified = eligible && m_rows->setEnabled(id, enabled);
        if (bulk || modified) { m_autoToggles.insert(id, enabled); changed = true; }
        if (bulk) m_autoBulkIds.insert(id); else if (modified) m_autoBulkIds.remove(id);
    }
    if (bulk) { m_lastBulkResult.clear(); emit lastBulkResultChanged(); }
    if (!changed) { if (starting) { m_autoCycle = false; emit nativeChanged(); } return false; }
    m_autoReview.clear(); m_autoReviewRevision.clear();
    m_autoDue = false; m_autoApplyTimer.start();
    reconcileSelection(); emit enabledCountChanged(); emit selectionChanged(); emit nativeChanged();
    advanceAutoApply();
    return true;
}
void LibraryModel::scheduleAutomaticApply(bool forceReview) {
    if(!m_automaticApply || !m_pendingChanges)return;
    if(!m_autoCycle) {
        m_autoCycle=true;m_autoBlocked=false;m_autoRoot=m_dataDir;m_autoProfile=m_viewedProfileId;
        m_autoForceReview=forceReview || m_pendingStatus=="unknown" || externalGameinfoChange();
    } else if(forceReview) m_autoForceReview=true;
    m_autoReview.clear();m_autoReviewRevision.clear();m_autoDue=false;m_autoApplyTimer.start();emit nativeChanged();
}
void LibraryModel::advanceAutoApply() {
    if (!m_autoCycle || m_autoBlocked || m_shutdownRequested || m_switching || busy() || m_loading
        || m_outcomeUnknown || m_editLost || m_recoveryRequired || !m_marker.isEmpty()) return;
    if (m_autoRoot != m_dataDir || m_autoProfile != m_viewedProfileId) {
        m_autoBlocked = true; setError(tr("The profile changed while inclusion choices were waiting to be saved.")); return;
    }
    if (autoApplyWorkflowBlocked()) return;
    if (m_launch->writeGate() || m_client->retiring()) return;
    // Require a complete, recent idle observation before automatic work.
    if (!m_launch->m_gameAge.isValid() || m_launch->m_gameAge.elapsed() >= 2000
        || m_launch->gameStatus().value("state").toString() != "idle" || !m_launch->gameStatus().value("scanComplete").toBool()) return;
    if (!m_autoToggles.isEmpty()) {
        m_autoDispatch = true;
        const bool available = canSetEnabled();
        m_autoDispatch = false;
        if (!available) return;
        const bool enabled = m_autoToggles.cbegin().value();
        QStringList ids;
        for (auto it = m_autoToggles.cbegin(); it != m_autoToggles.cend(); ++it)
            if (it.value() == enabled && ids.size() < 256) ids.append(it.key());
        bool bulk = ids.size() > 1;
        for (const auto &id : ids) if (m_autoBulkIds.contains(id)) bulk = true;
        for (const auto &id : ids) { m_autoToggles.remove(id); m_autoInFlight.insert(id, enabled); if (m_autoBulkIds.remove(id)) m_autoInFlightBulk.insert(id); }
        QJsonObject params{{"profileId", m_autoProfile}, {"enabled", enabled}};
        if (bulk) params.insert("modIds", QJsonArray::fromStringList(ids)); else params.insert("modId", ids.first());
        m_autoDispatch = true;
        const bool started = mutate(int(bulk ? BackendClient::Call::SetModsEnabled : BackendClient::Call::SetEnabled), params, tr("Saving inclusion choices…"));
        m_autoDispatch = false;
        if (!started) {
            for (const auto &id : ids) { m_autoToggles.insert(id, enabled); if (m_autoInFlightBulk.contains(id)) m_autoBulkIds.insert(id); }
            m_autoInFlight.clear(); m_autoInFlightBulk.clear();
            m_autoBlocked = true; m_closeAfterToggles = false; emit shutdownChanged(); setError(tr("Inclusion choices could not be saved. Review the current library state."));
        }
        return;
    }
    if (m_closeAfterToggles) { m_closeAfterToggles = false; m_autoCycle = false; requestShutdown(); return; }
    if (!m_autoReview.isEmpty()) {
        if (m_autoReviewRevision != m_revision) {
            m_autoReview.clear(); m_autoReviewRevision.clear(); m_autoDue = true;
        } else {
            const auto review = m_autoReview;
            bool takeover = !review.value("upstreamRepairMigration").toString().isEmpty();
            for (const auto &mount : review.value("replacedMounts").toList())
                if (!mount.toString().startsWith("citadel/deadlock_native/", Qt::CaseInsensitive)) takeover = true;
            const bool safe = m_automaticApply && !m_autoForceReview && !takeover && ApplyWarningPolicy::onlyOverlapNotes(review.value("warnings").toList())
                && (m_pendingReason.isEmpty() || (m_pendingStatus == "pending" && m_pendingReason == "Performance config selection changed."))
                && m_pendingStatus != "unknown" && !externalGameinfoChange()
                && m_launch->gameStatus().value("processes").toList().isEmpty() && !m_recoveryRequired && !m_outcomeUnknown;
            m_autoReview.clear(); m_autoReviewRevision.clear();
            m_applyReview = review;
            m_autoDispatch = true;
            const bool committed = safe && commitApply();
            m_autoDispatch = false;
            if (committed) return;
            if (safe && (m_launch->writeGate() || busy() || m_loading || m_client->retiring())) {
                m_autoReview = review; m_autoReviewRevision = m_revision; m_applyReview.clear(); return;
            }
            m_autoCycle = false;
            emit nativeChanged(); emit reviewChanged();
            return;
        }
    }
    if (!m_automaticApply) { m_autoCycle = false; emit nativeChanged(); return; }
    if (!m_autoDue || m_autoApplyTimer.isActive()) return;
    m_autoDue = false;
    if (!m_pendingChanges) { m_autoCycle = false; emit nativeChanged(); return; }
    m_autoDispatch = true;
    const bool available = canPreviewApply();
    m_autoDispatch = false;
    if (!available || m_gamePath.isEmpty()) { m_autoCycle = false; emit nativeChanged(); emit automaticReviewRequired(); return; }
    m_autoPreview = true;
    m_autoDispatch = true;
    const bool started = previewApply(m_gamePath);
    m_autoDispatch = false;
    if (started) m_autoReviewRevision = m_revision;
    else { m_autoPreview = false; m_autoCycle = false; emit nativeChanged(); }
}
bool LibraryModel::setModsEnabled(const QStringList &ids, bool enabled) {
    if (m_automaticApply && canBulkToggle()) return queueAutoToggles(ids, enabled, true);
    return bulkMutation(ids,false,enabled);
}
bool LibraryModel::deleteMods(const QStringList &ids) {
    if(!canDeleteMods() || ids.isEmpty() || ids.size()>256)return false;
    QSet<QString> unique;
    for(const auto &id:ids) {
        if(!LaunchData::id(QJsonValue(id)) || unique.contains(id) || modDetails(id).isEmpty())return false;
        unique.insert(id);
    }
    m_lastBulkResult.clear();emit lastBulkResultChanged();
    return mutate(int(BackendClient::Call::DeleteMods),{{"profileId",m_viewedProfileId},{"modIds",QJsonArray::fromStringList(ids)}},tr("Deleting mods from the library…"));
}
bool LibraryModel::removeModsFromProfile(const QStringList &ids) { return bulkMutation(ids,true,false); }
bool LibraryModel::bulkMutation(const QStringList &ids, bool remove, bool enabled) {
    if(!canBulkOperate() || ids.isEmpty() || ids.size()>256)return false;
    QSet<QString> unique;
    for(const auto &id:ids) {
        if(!LaunchData::id(QJsonValue(id)) || unique.contains(id))return false;
        unique.insert(id);
    }
    QJsonObject params{{"profileId",m_viewedProfileId},{"modIds",QJsonArray::fromStringList(ids)}};
    if(!remove)params.insert("enabled",enabled);
    m_lastBulkResult.clear();emit lastBulkResultChanged();
    return mutate(int(remove ? BackendClient::Call::RemoveModsFromProfile : BackendClient::Call::SetModsEnabled),params,tr("Saving batch library changes…"));
}
bool LibraryModel::setModEnabled(const QString &id, bool enabled) {
    if (m_nativeMode) {
        if (m_automaticApply && canToggleEnabled()) return queueAutoToggles({id}, enabled);
        if (!canSetEnabled()) return false;
        const auto mod = modDetails(id);
        if (mod.isEmpty() || (enabled && !mod.value("selectionReady",mod.value("available")).toBool())) { setError(tr("Choose a nonempty set of ready files in Files/options before including this mod.")); return false; }
        return mutate(int(BackendClient::Call::SetEnabled), {{"profileId", m_viewedProfileId}, {"modId", id}, {"enabled", enabled}}, tr("Saving inclusion choices…"));
    }
    if (readOnly() || !m_rows->setEnabled(id, enabled)) return false;
    reconcileSelection(); emit enabledCountChanged(); emit selectionChanged();
    return true;
}
QString LibraryModel::idAt(int row) const {
    return row >= 0 && row < rowCount() ? data(index(row, 0), LibraryRows::ModId).toString() : QString{};
}
void LibraryModel::setSelectedId(const QString &id) {
    if (m_selectedId == id) return;
    if (!id.isEmpty() && match(index(0, 0), LibraryRows::ModId, id, 1, Qt::MatchExactly).isEmpty()) return;
    m_selectedId = id; emit selectionChanged();
}
void LibraryModel::reconcileSelection() {
    if (!m_selectedId.isEmpty() && !match(index(0, 0), LibraryRows::ModId, m_selectedId, 1, Qt::MatchExactly).isEmpty()) return;
    const auto next = idAt(0);
    if (m_selectedId != next) { m_selectedId = next; emit selectionChanged(); }
}


bool LibraryModel::readOnly() const { return !m_demoMode && !m_nativeMode; }
bool LibraryModel::bootstrapPending() const { return m_marker.value("method",-1).toInt()==int(BackendClient::Call::LibraryInitialize); }
QString LibraryModel::bootstrapState() const {
    if(m_invalidMarker)return QStringLiteral("invalid");
    if(bootstrapPending() || m_bootstrapMarkerFileBusy)return m_bootstrapConfirming ? QStringLiteral("confirming") : busy() ? QStringLiteral("creating") : QStringLiteral("pending");
    return m_inspectState;
}
QString LibraryModel::bootstrapReason() const {
    const auto stateError=AppPaths::stateWriteError();if(!stateError.isEmpty())return stateError;
    if(m_invalidMarker)return tr("The saved operation context cannot be read. Preserve and restore it before editing.");
    if(bootstrapPending() || m_bootstrapMarkerFileBusy)return tr("Creation is unresolved. Check creation to use the saved operation again.");
    return m_inspectReason;
}
bool LibraryModel::canCreateLibrary() const {
    return !m_demoMode && !m_nativeMode && !m_shutdownRequested && !m_switching && !m_launch->writeGate()
        && !m_client->retiring() && !m_client->writing() && m_client->isRunning() && m_client->nativeRoot()==m_dataDir
        && m_client->readyForRequests() && m_inspectState=="absent" && m_inspectEligible && m_inspectRoot==m_dataDir && m_inspectSession==m_client->sessionGeneration()
        && !busy() && !m_otherMarkerFileTasks && !m_loading && !m_outcomeUnknown && !m_recoveryRequired && m_marker.isEmpty()
        && !m_localImport->busy() && m_localImport->phase()!="ready" && !m_options->busy() && !m_options->hasPreparation();
}
bool LibraryModel::inspectInitialization() {
    if(m_demoMode || m_shutdownRequested || !m_client->isRunning() || !m_client->readyForRequests() || m_client->retiring() || m_client->nativeRoot()!=m_dataDir)return false;
    m_inspectState="checking";m_inspectEligible=false;m_inspectRoot.clear();
    m_inspectReason=tr("Checking the native data root…");emit nativeChanged();
    m_client->nativeRequest(BackendClient::Call::LibraryInspect);return true;
}
bool LibraryModel::createLibrary() {
    if(!canCreateLibrary())return false;
    const auto id=QUuid::createUuid().toString(QUuid::Id128);
    const QJsonObject params{{"version",1},{"operationId",id},{"expectedRevision","0"}};
    const QJsonObject marker{{"bootstrapVersion",1},{"root",m_dataDir},{"operationId",id},
        {"method",int(BackendClient::Call::LibraryInitialize)},{"expectedRevision","0"},{"params",params}};
    if(!bootstrapMarker(marker,m_dataDir))return false;
    if(!reserveBootstrapMarkerWork())return false;
    const auto session=m_client->sessionGeneration();
    const auto saved=marker.toVariantMap();m_marker=saved;m_bootstrapRetry=false;m_bootstrapMarkerDurable=false;m_outcomeUnknown=true;
    auto errorGeneration=m_errorGeneration;
    setBusy(true,true,tr("Creating an empty native library… Keep the app open."));
    if(m_errorGeneration==errorGeneration)errorGeneration=setError({});
    invalidateReviews();
    persistMarker(saved,[this,session,errorGeneration]{m_bootstrapMarkerDurable=true;m_outcomeUnknown=true;dispatchBootstrap(session,errorGeneration);},[this]{
        // QSaveFile may have committed before a later directory fsync failed.
        // Keep this identity in memory and require persistence again before send.
        m_bootstrapMarkerDurable=false;m_outcomeUnknown=true;
        setBusy(false,false,tr("Creation was not sent because its recovery context could not be saved. Check creation to retry saving it."));
    },true);
    return true;
}
void LibraryModel::dispatchBootstrap(quint64 session,quint64 errorGeneration) {
    if(m_shutdownRequested || m_shutdownReady) {
        m_outcomeUnknown=true;m_reconciling=false;setBusy(false);return;
    }
    if(!bootstrapMarker(QJsonObject::fromVariantMap(m_marker),m_dataDir) || !m_bootstrapMarkerDurable
        || !m_writeBusy || m_client->retiring() || m_client->writing() || !m_client->isRunning()
        || m_client->sessionGeneration()!=session || m_client->nativeRoot()!=m_dataDir
        || !m_client->readyForRequests()) {
        m_outcomeUnknown=true;m_reconciling=false;setBusy(false);setErrorIfCurrent(errorGeneration,tr("Creation could not be sent. Reconnect and check the saved creation context."));return;
    }
    m_client->nativeRequest(BackendClient::Call::LibraryInitialize,QJsonObject::fromVariantMap(m_marker.value("params").toMap()));
}
void LibraryModel::confirmBootstrap(bool created) {
    if(!bootstrapPending() || !m_bootstrapMarkerDurable || m_client->nativeRoot()!=m_dataDir)return;
    m_bootstrapCreated=created;m_bootstrapConfirming=true;m_outcomeUnknown=true;
    setBusy(true,true,tr("Confirming the current native library…"));setLoading(true);
    m_client->requestBootstrapSnapshot();
}
bool LibraryModel::writesIdle() const {
    return m_client->readyForRequests() && !m_launch->writeGate() && !m_shutdownRequested && !m_switching && !busy() && !m_loading && !m_outcomeUnknown && !m_editLost && !m_recoveryRequired;
}
bool LibraryModel::canImport() const { return !m_demoMode && !m_nativeMode && writesIdle(); }
bool LibraryModel::canEditProfiles() const { return m_nativeMode && !m_autoCycle && writesIdle(); }
bool LibraryModel::canSetEnabled() const { return m_nativeMode && (!m_autoCycle || m_autoDispatch) && writesIdle(); }
bool LibraryModel::canToggleEnabled() const {
    if (canSetEnabled()) return true;
    const bool applyingReviewed = m_marker.value("method").toInt() == int(BackendClient::Call::Apply) && (busy() || m_localConfirmation);
    return m_automaticApply && !m_editLost && (m_autoCycle || applyingReviewed) && (!m_autoBlocked || (!busy() && !m_outcomeUnknown)) && !m_closeAfterToggles && !m_shutdownRequested && !m_switching
        && !m_launch->writeGate() && !m_recoveryRequired && (!m_outcomeUnknown || m_localConfirmation)
        && (!m_autoCycle || (m_autoRoot == m_dataDir && m_autoProfile == m_viewedProfileId));
}
bool LibraryModel::canBulkToggle() const {
    return m_client->readyForRequests() && canToggleEnabled();
}
void LibraryModel::setAutomaticApply(bool value) {
    if (m_automaticApply == value) return;
    m_automaticApply = value;
    if (!value) { m_autoApplyTimer.stop(); m_autoDue = false; }
    emit nativeChanged();
    QTimer::singleShot(0, this, &LibraryModel::advanceAutoApply);
}
void LibraryModel::setAutoApplyWorkflowOpen(bool open) {
    if (m_autoApplyWorkflowOpen == open) return;
    m_autoApplyWorkflowOpen = open;
    emit autoApplyWorkflowOpenChanged();
    if (!open) QTimer::singleShot(0, this, &LibraryModel::advanceAutoApply);
}
bool LibraryModel::autoApplyWorkflowBlocked() const {
    return m_autoApplyWorkflowOpen || !m_applyReview.isEmpty() || !m_importReview.isEmpty()
        || m_localImport->busy() || m_options->busy() || m_options->visible()
        || m_options->orderVisible() || m_options->historyVisible() || m_fonts->busy()
        || m_fonts->visible() || m_fonts->managerVisible() || m_remote->visible();
}
bool LibraryModel::canPreviewApply() const { return m_nativeMode && (!m_autoCycle || m_autoDispatch) && writesIdle() && !m_viewedProfileId.isEmpty(); }
QString LibraryModel::externalGameinfoMessage() const
{
    return externalGameinfoChange() ? tr("Deadlock's game files were changed by another mod manager or a game update. Apply again to put your mods back using the current game files.") : QString();
}
bool LibraryModel::canRestore() const { return m_nativeMode && m_canRestore && m_autoToggles.isEmpty() && m_autoInFlight.isEmpty() && writesIdle(); }
bool LibraryModel::canRecover() const { return AppPaths::stateWriteError().isEmpty() && !m_invalidMarker && !bootstrapPending() && !m_shutdownRequested && !m_client->launchOwned() && !m_client->retiring() && !m_launch->knownBarrier() && !m_launchOnlyRecovery && !m_demoMode && !busy() && !m_loading && m_client->isRunning() && (m_nativeMode || m_haveRecoveryContext) && (m_recoveryRequired || m_outcomeUnknown) && (!remoteMarkerPending() || m_recoveryRequired || m_marker.contains("recoveryId")) && m_client->readyForRequests(); }
bool LibraryModel::replacementMarkerPending() const {return m_marker.value("method",-1).toInt()==int(BackendClient::Call::RemoteReplaceCommit);}
bool LibraryModel::remoteMarkerPending() const {return m_marker.value("method",-1).toInt()==int(BackendClient::Call::RemoteCommit) || replacementMarkerPending();}
bool LibraryModel::recoveryWrapperPending() const {return m_marker.value("nativeRecoveryMarkerVersion").toInt()==1 && m_marker.value("method",-1).toInt()==int(BackendClient::Call::Recover);}
bool LibraryModel::replacementRecoveryContext() const {return replacementMarkerPending() || (recoveryWrapperPending()
    && m_marker.value("originalMarker").toMap().value("method",-1).toInt()==int(BackendClient::Call::RemoteReplaceCommit));}
bool LibraryModel::remoteAdmissionAvailable() const {
    return m_nativeMode && !m_demoMode && writesIdle() && !m_client->retiring() && !m_client->writing()
        && m_marker.isEmpty() && !m_otherMarkerFileTasks
        && !m_localImport->busy() && m_localImport->phase()!="ready" && !m_options->busy() && !m_options->hasPreparation()
        && m_client->isRunning() && m_client->readyForRequests();
}
bool LibraryModel::fontsAvailable() const { return m_nativeMode && !m_demoMode && writesIdle() && m_marker.isEmpty() && m_client->isRunning(); }
bool LibraryModel::dispatchFontsPreview(const QString &root,const QString &game,const QString &revision,const QJsonObject &change) {
    if(!fontsAvailable() || root!=m_dataDir || game!=m_gamePath || revision!=m_revision) return false;
    invalidateReviews(); setError({}); setBusy(true,false,tr("Reviewing shared game fonts…"));
    m_client->nativeRequest(BackendClient::Call::FontsPreview,{{"dataDir",root},{"gamePath",game},{"expectedRevision",revision},{"change",change}}); return true;
}
bool LibraryModel::dispatchFontsCommit(const QString &root,const QString &game,const QString &revision,const QString &modId,const QString &token) {
    if(!fontsAvailable() || root!=m_dataDir || game!=m_gamePath || revision!=m_revision || !FontData::hex(token,32)) return false;
    // mutate persists these pinned descriptions in the same central marker before dispatch.
    return mutate(int(BackendClient::Call::FontsCommit),{{"token",token},{"_fontGame",game},{"_fontMod",modId}},tr("Changing reviewed shared fonts… Keep the app open."));
}
QString LibraryModel::actionUnavailableReason() const {
    const auto stateError=AppPaths::stateWriteError();if(!stateError.isEmpty())return stateError;
    if (m_invalidMarker) return tr("The saved operation context cannot be read. Preserve and restore it before editing.");
    if (m_launch->gameStatus().value("state") == "running") return tr("Close Deadlock to change your mods.");
    if (m_launch->writeGate()) return tr("A launch owns the game setup. Check launch status before editing.");
    if (busy()) return tr("An operation is running. Search and previews remain available.");
    if (bootstrapPending()) return tr("Check creation before editing the native library.");
    if (m_editLost) return tr("The connection was lost while saving. Reload to see the current state.");
    if (m_outcomeUnknown || m_recoveryRequired) return tr("Check operation status or recover before editing.");
    if (readOnly()) return m_inspectState=="absent" && m_inspectEligible ? tr("Upstream is read-only. Create an empty native library or import a reviewed copy to edit.") : tr("Upstream is read-only. Import a reviewed copy to edit.");
    return {};
}
bool LibraryModel::helperStopped() const {
    return !m_shutdownRequested && !m_demoMode && m_client->helperStopped() && m_client->nativeRoot() == m_dataDir;
}
QString LibraryModel::stopReason() const {
    return helperStopped() ? m_client->stopReason() : QString();
}
void LibraryModel::setBusy(bool value, bool write, const QString &status) {
    if (!value) m_previewBusy = false;
    m_busy = value; m_writeBusy = value && write; m_status = status; emit nativeChanged();
    if(!value){QTimer::singleShot(0,this,&LibraryModel::detectNextHero);QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply);}
}
void LibraryModel::invalidateReviews() {
    if (!m_autoReview.isEmpty() && m_autoCycle) m_autoDue = true;
    m_autoReview.clear(); m_autoReviewRevision.clear(); m_importReview.clear(); m_applyReview.clear(); emit reviewChanged();
}
QString LibraryModel::markerPath() const {
    const auto key = QCryptographicHash::hash(m_dataDir.toUtf8(), QCryptographicHash::Sha256).toHex();
    return QDir(AppPaths::stateRoot()).filePath("pending/" + key + ".json");
}
bool LibraryModel::reserveBootstrapMarkerWork() {
    if(m_bootstrapMarkerFileBusy || m_otherMarkerFileTasks || m_shutdownRequested || m_shutdownReady)return false;
    // Reserve before any signal can reenter shutdown or another marker action.
    m_bootstrapMarkerFileBusy=true;
    return true;
}
void LibraryModel::persistMarker(const QVariantMap &marker, const std::function<void()> &then, const std::function<void()> &failure, bool bootstrapFileWork) {
    const auto stateError=AppPaths::stateWriteError();
    if(!stateError.isEmpty()){setBusy(false);setError(stateError);if(failure)failure();return;}
    // Ordinary marker actions cannot overwrite a pending bootstrap identity,
    // including while its successful clear callback is still being delivered.
    if(!bootstrapFileWork && (m_bootstrapMarkerFileBusy || bootstrapPending())) {
        if(failure)failure();
        return;
    }
    Q_ASSERT(!bootstrapFileWork || m_bootstrapMarkerFileBusy);
    // An older ordinary marker task must finish its callback before bootstrap
    // can reserve the same file, even if transport failure cleared m_busy.
    if(!bootstrapFileWork)++m_otherMarkerFileTasks;
    const auto path = markerPath(); const auto epoch = m_loadEpoch; const auto errorGeneration=m_errorGeneration;
    auto *task = new ContextTask([marker, path] {
        const auto bytes=QJsonDocument::fromVariant(marker).toJson(QJsonDocument::Compact);
        bool ok = bytes.size()<=65536 && AppPaths::prepareState() && QDir().mkpath(QFileInfo(path).absolutePath());
        QSaveFile file(path); if (ok) ok = file.open(QIODevice::WriteOnly);
        if (ok) {
            ok = file.setPermissions(QFile::ReadOwner | QFile::WriteOwner)
                && file.write(bytes) == bytes.size() && file.flush() && file.handle() >= 0
                && ::fsync(file.handle()) == 0 && file.commit();
        }
        // Persist the marker's directory entry, including newly created config
        // ancestors, before allowing a request to leave this process.
        auto directory = QFileInfo(path).absolutePath();
        while (ok) {
            const auto bytes = QFile::encodeName(directory);
            const int fd = ::open(bytes.constData(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
            if (fd < 0) { ok = false; break; }
            ok = ::fsync(fd) == 0; ::close(fd);
            const auto parent = QFileInfo(directory).absolutePath();
            if (parent == directory) break;
            directory = parent;
        }
        return ContextTask::Result{ok, marker};
    });
    connect(task, &ContextTask::finished, this, [this, then, failure, epoch, bootstrapFileWork, errorGeneration](bool ok, const QVariantMap &saved) {
        if (m_loadEpoch == epoch) {
            if (!ok) { setBusy(false); setErrorIfCurrent(errorGeneration,tr("Cannot persist operation recovery context. Check config storage and permissions.")); if (failure) failure(); }
            else { m_marker = saved; then(); }
        }
        // Hold ownership through the continuation: it may dispatch a writer,
        // fail synchronously, or emit signals that reenter shutdown. A stale
        // epoch suppresses state updates, but must still release this task.
        if(bootstrapFileWork)m_bootstrapMarkerFileBusy=false;
        else --m_otherMarkerFileTasks;
        if(bootstrapFileWork || !m_otherMarkerFileTasks)emit nativeChanged();
    }, Qt::QueuedConnection);
    QThreadPool::globalInstance()->start(task);
    emit nativeChanged();
}
void LibraryModel::clearMarker(const std::function<void()> &then) {
    persistMarker({}, [this, then] { m_marker.clear(); if (then) then(); });
}
void LibraryModel::loadNative(const QString &helperPath, const QString &dataDir) {
    if(m_shutdownRequested)return;m_deferredLoads.enqueue({helperPath,{},dataDir,true,false,0});advanceLoad();
}
void LibraryModel::advanceLoad() {
    if(m_shutdownRequested || m_deferredLoads.isEmpty())return;
    if(!m_switching) {
        if(m_loadEpoch==0 && !m_client->isRunning() && !m_client->retiring()) {
            const auto next=m_deferredLoads.dequeue();
            if(next.demo)performDemoData(next.rows);else beginLoad(next.helper,next.state,next.root,next.nativeFirst);
            return;
        }
        m_switching=true;
        resetLibraryMediaContext();
        m_status=tr("Waiting for the current library's work to stop safely…");
        emit nativeChanged();
        // Start every owner before waiting on any one of them: local/options
        // and remote preparation can contend for the same slot.
        m_catalog->shutdown();
        m_remote->shutdown();
        m_localImport->beginRetirement(m_dataDir,m_loadEpoch);
        m_launch->shutdown();
    }
    if(remoteMarkerPending() || recoveryWrapperPending())scheduleRemoteReconcile();
    else if((!m_marker.isEmpty() || m_outcomeUnknown) && !m_reconciling && !busy()) {
        if(!m_client->isRunning() && !m_client->retiring() && !m_helperPath.isEmpty())m_client->start(m_helperPath,m_statePath,m_dataDir,m_nativeMode);
        else if(m_client->isRunning())reconcileOperation();
    }
    if(m_client->launchOwned() && m_client->remoteOwned() && !m_client->launchRetiring())m_client->retireLaunch();
    if(!m_marker.isEmpty() || m_outcomeUnknown || m_recoveryRequired || m_otherMarkerFileTasks || m_bootstrapMarkerFileBusy
        || !m_catalog->shutdownReady() || !m_remote->shutdownReady() || !m_localImport->retirementReady(m_dataDir,m_loadEpoch)
        || m_client->launchOwned() || m_client->retiring() || m_client->writing() || m_launch->m_fileBusy)return;
    if(m_client->isRunning()) {
        m_client->stop();
        if(m_client->retiring())return;
    }
    const auto next=m_deferredLoads.dequeue();
    m_switching=false;
    m_catalog->resumeAfterRetirement();
    m_localImport->resumeAfterRetirement();
    if(next.demo)performDemoData(next.rows);else beginLoad(next.helper,next.state,next.root,next.nativeFirst);
}
void LibraryModel::beginLoad(const QString &helperPath, const QString &statePath, const QString &dataDir, bool nativeFirst) {
    const auto epoch = ++m_loadEpoch;
    m_client->stop(); m_helperPath = helperPath;
    const auto chosen=dataDir.isEmpty() ? AppPaths::dataRoot(helperPath) : dataDir;
    if(chosen.isEmpty()) {setLoading(false);setError(AppPaths::rootSelectionError);return;}
    m_dataDir=QDir::cleanPath(QDir(chosen).absolutePath());
    AppPaths::stateWriteError();
    m_statePath = statePath; m_defaultMode = nativeFirst; m_recoveryBarrier = false; m_initialProfile.clear(); m_requestedProfileId.clear(); m_preservedError.clear();
    m_launch->loadRoot(m_dataDir);m_remote->loadRoot(m_dataDir);m_remoteStage=RemoteStage::Idle;m_remoteOutcome.clear();m_remoteToken.clear();m_remoteMarkerDurable=false;m_remoteRecoveryTerminal=false;m_remoteRetry.stop();m_launchOnlyRecovery=false;
    m_fullProfileOrder.clear();m_confirmationTarget=ConfirmationTarget::None;m_localConfirmation=false;
    m_demoMode = false; m_nativeMode = false; m_outcomeUnknown = false; m_recoveryRequired = false; m_editInFlight = false;
    if(std::exchange(m_editLost,false)){m_busy=false;m_status.clear();}
    m_inspectState="unavailable";m_inspectReason.clear();m_inspectRoot.clear();m_inspectEligible=false;
    m_bootstrapMarkerDurable=false;m_bootstrapConfirming=false;m_bootstrapRetry=false;m_invalidMarker=false;
    m_revision = "0"; m_haveRecoveryContext = false; m_marker.clear(); invalidateReviews(); clearSource(); m_rows->replace({}); notifyRowsChanged();
    setError({}); setLoading(true); emit modeChanged(); emit nativeChanged();
    if (m_dataDir.isEmpty()) { m_client->start(helperPath, statePath, {}, nativeFirst); return; }
    const auto stateError=AppPaths::stateWriteError();
    if(!stateError.isEmpty()) {
        setError(stateError);
        m_client->start(helperPath,statePath,m_dataDir,nativeFirst);
        return;
    }
    const auto path = markerPath(); const auto errorGeneration=m_errorGeneration;
    auto *task = new ContextTask([path,root=m_dataDir] {
        QFile file(path); QVariantMap marker; bool ok = true;
        if (file.exists()) {
            ok = file.open(QIODevice::ReadOnly);
            if (ok) {
                QJsonParseError error; auto bytes = file.read(65537); auto doc = QJsonDocument::fromJson(bytes, &error);
                ok = bytes.size() <= 65536 && error.error == QJsonParseError::NoError && doc.isObject(); marker = doc.object().toVariantMap();
                QVariantMap wrapper;
                if(marker.contains("nativeRecoveryMarkerVersion")) {
                    const auto outer=doc.object();const auto original=outer.value("originalMarker");
                    ok=ok && LaunchData::strictJson(bytes) && bytes==QJsonDocument(outer).toJson(QJsonDocument::Compact)
                        && outer.size()==6 && outer["nativeRecoveryMarkerVersion"]==QJsonValue(1)
                        && outer["method"]==QJsonValue(int(BackendClient::Call::Recover)) && outer["root"]==QJsonValue(root)
                        && LaunchData::root(outer["root"]) && LaunchData::id(outer["operationId"]) && LaunchData::decimal(outer["expectedRevision"])
                        && (original.isNull() || (original.isObject() && !original.toObject().isEmpty()
                            && !original.toObject().contains("nativeRecoveryMarkerVersion")
                            && (!original.toObject().contains("root") || original.toObject()["root"]==QJsonValue(root))));
                    wrapper=marker;
                    doc=QJsonDocument(original.isObject()?original.toObject():QJsonObject{});
                    bytes=doc.toJson(QJsonDocument::Compact);marker=doc.object().toVariantMap();
                }
                const auto safeId = [](const QVariant &v) {
                    const auto id = v.toString();
                    return v.metaType().id() == QMetaType::QString && !id.isEmpty() && id.size() <= 64
                        && std::all_of(id.cbegin(), id.cend(), [](QChar c) { return (c.isLetterOrNumber() && c.unicode() < 128) || c == '_' || c == '-'; });
                };
                if (!marker.isEmpty()) ok = ok && safeId(marker.value("operationId")) && (!marker.contains("recoveryId") || safeId(marker.value("recoveryId")));
                if(!marker.isEmpty()) {
                    const auto method=static_cast<BackendClient::Call>(marker.value("method",-1).toInt());
                    ok=ok&&doc.object().value("method")==QJsonValue(int(method));
                    const bool bootstrapContext=marker.contains("bootstrapVersion")||method==BackendClient::Call::LibraryInitialize
                        || (marker.contains("params")&&method!=BackendClient::Call::Apply);
                    if(bootstrapContext) {
                        ok=ok&&LaunchData::strictJson(bytes)&&bytes==QJsonDocument(doc.object()).toJson(QJsonDocument::Compact)
                            &&bootstrapMarker(doc.object(),root);
                    } else switch(method) {
                    case BackendClient::Call::LaunchCommit: {
                        auto object=doc.object();object.remove("recoveryId");
                        ok=ok&&LaunchData::strictJson(bytes)&&LaunchData::marker(object,root);break;
                    }
                    case BackendClient::Call::RemoteCommit:
                        ok=ok&&LaunchData::strictJson(bytes)&&bytes==QJsonDocument(doc.object()).toJson(QJsonDocument::Compact)&&RemoteData::marker(doc.object(),root);break;
                    case BackendClient::Call::RemoteReplaceCommit:
                        ok=ok&&LaunchData::strictJson(bytes)&&bytes==QJsonDocument(doc.object()).toJson(QJsonDocument::Compact)&&RemoteData::replacementMarker(doc.object(),root);break;
                    case BackendClient::Call::Apply: {
                        // Historical Apply markers contain only their operation identity.
                        // A new context is exact; newer/partial layout evidence stays a barrier.
                        const bool context=marker.contains("applyContextVersion")||marker.contains("params")||marker.contains("deploymentLayout");
                        if(context) {
                            bool revisionOk=false;const auto revision=marker.value("expectedRevision").toString();const auto n=revision.toULongLong(&revisionOk);
                            const auto params=doc.object().value("params").toObject();
                            const bool acknowledged=marker.contains("deploymentLayout");
                            ok=ok&&doc.object().value("applyContextVersion")==QJsonValue(1)&&marker.size()==8+int(acknowledged)+int(marker.contains("recoveryId"))
                                &&marker.value("root").toString()==root&&safeId(marker.value("profileId"))&&doc.object().value("expectedRevision").isString()&&revisionOk&&QString::number(n)==revision&&FontData::hex(marker.value("token").toString(),32)
                                &&doc.object().value("params").isObject()&&params.size()==3+int(acknowledged)&&params.value("operationId")==doc.object().value("operationId")&&params.value("expectedRevision")==doc.object().value("expectedRevision")&&params.value("token")==doc.object().value("token")
                                &&(!acknowledged||(doc.object().value("deploymentLayout")==QJsonValue("shards-v1")&&params.value("deploymentLayout")==doc.object().value("deploymentLayout")));
                        }else ok=ok&&marker.size()==2+int(marker.contains("recoveryId"));
                        break;
                    }
                    case BackendClient::Call::OptionsAttach:case BackendClient::Call::ProfileEditCommit: {
                        bool revisionOk=false;const auto revision=marker.value("expectedRevision").toString();const auto n=revision.toULongLong(&revisionOk);
                        ok=ok&&marker.value("root").toString()==root&&safeId(marker.value("profileId"))&&revisionOk&&QString::number(n)==revision;
                        if(method==BackendClient::Call::OptionsAttach)ok=ok&&safeId(marker.value("modId"))&&FontData::hex(marker.value("jobId").toString(),32);
                        else ok=ok&&FontData::hex(marker.value("token").toString(),32);
                        break;
                    }
                    case BackendClient::Call::BisectStart:case BackendClient::Call::BisectAnswer:case BackendClient::Call::BisectFinish:
                    case BackendClient::Call::SetModsEnabled:case BackendClient::Call::RemoveModsFromProfile:case BackendClient::Call::DeleteMods:
                    case BackendClient::Call::SetFixEnabled:case BackendClient::Call::SetFixAutomatically:
                    case BackendClient::Call::PerformanceSelect:case BackendClient::Call::ImportCommit:case BackendClient::Call::SetEnabled:case BackendClient::Call::Profile:
                    case BackendClient::Call::Restore:case BackendClient::Call::Recover:case BackendClient::Call::LocalCommit:case BackendClient::Call::FontsCommit:break;
                    default:ok=false;break;
                    }
                }
                if(!wrapper.isEmpty())marker=wrapper;
            }
        }
        return ContextTask::Result{ok, marker};
    });
    connect(task, &ContextTask::finished, this, [this, helperPath, statePath, nativeFirst, epoch, errorGeneration](bool ok, const QVariantMap &marker) {
        if (m_loadEpoch != epoch) return;
        m_marker = ok ? marker : QVariantMap{}; m_invalidMarker=!ok; m_outcomeUnknown = !ok || !marker.isEmpty();
        m_remoteMarkerDurable=ok&&(remoteMarkerPending()||recoveryWrapperPending());
        m_bootstrapMarkerDurable=ok&&bootstrapPending();
        if (!ok) setErrorIfCurrent(errorGeneration,tr("Cannot read saved operation context. Preserve and restore the config file before retrying."));
        m_client->start(helperPath, statePath, m_dataDir, nativeFirst && !bootstrapPending());
        if (!m_initialProfile.isEmpty()) { m_client->requestSnapshot(statePath, m_initialProfile); m_initialProfile.clear(); }
        emit nativeChanged();
    }, Qt::QueuedConnection);
    QThreadPool::globalInstance()->start(task);
}
void LibraryModel::refreshKeepingError(bool preserve) {
    // Reads keep the current display error. A failed read may keep that same
    // reason only while no newer error event has taken ownership.
    m_preservedError=preserve?m_error:QString();
    m_preservedErrorGeneration=m_errorGeneration;
    refresh();
}
void LibraryModel::checkOriginalAfterRecovery() {
    if(bootstrapPending() || m_bootstrapMarkerFileBusy)return;
    m_recoveryBarrier = true; m_outcomeUnknown = true; m_marker.remove("recoveryId");
    setBusy(true,false,tr("Recovery completed. Checking the original operation outcome…"));
    persistMarker(m_marker, [this] {
        m_reconciling = true;
        m_client->nativeRequest(BackendClient::Call::Operation, {{"operationId", m_marker.value("operationId").toString()}});
    });
}
bool LibraryModel::previewImport(const QString &statePath, const QString &gamePath) {
    if (!canImport() || statePath.isEmpty() || gamePath.isEmpty()) return false;
    invalidateReviews(); m_previewBusy = true; setError({}); setBusy(true, false, tr("Checking files…"));
    m_client->nativeRequest(BackendClient::Call::ImportPreview, {{"statePath", statePath}, {"gamePath", gamePath}}); return true;
}
bool LibraryModel::cancelPreview() {
    if (m_switching || m_launch->writeGate() || m_client->retiring() || !previewBusy() || m_writeBusy || m_client->writing()) return false;
    m_client->stop(); invalidateReviews(); setBusy(false, false, tr("Review cancelled. No changes were committed."));
    setLoading(true);
    m_client->start(m_helperPath, m_statePath, m_dataDir, m_defaultMode || m_nativeMode);
    m_client->requestSnapshot(m_statePath, m_viewedProfileId);
    return true;
}
void LibraryModel::discardImportReview() { m_importReview.clear(); emit reviewChanged(); QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply); }
void LibraryModel::discardApplyReview() { m_applyReview.clear(); emit reviewChanged(); QTimer::singleShot(0,this,&LibraryModel::advanceAutoApply); }
bool LibraryModel::mutate(int callValue, QJsonObject params, const QString &status) {
    const auto call = static_cast<BackendClient::Call>(callValue);
    if (m_autoCycle && !m_autoDispatch) return false;
    if(call==BackendClient::Call::Recover)return false; // Recovery always uses its durable exact context.
    if (m_invalidMarker || m_shutdownRequested || m_switching || m_client->retiring() || m_launch->writeGate() || busy() || bootstrapPending() || m_outcomeUnknown || m_editLost || m_recoveryRequired || !m_marker.isEmpty()) return false;
    const auto id = QUuid::createUuid().toString(QUuid::Id128);
    params.insert("expectedRevision", m_revision); params.insert("operationId", id);
    QVariantMap marker{{"operationId",id},{"method",callValue}};
    if(call==BackendClient::Call::FontsCommit) {
        marker.insert("root",m_dataDir); marker.insert("gamePath",params.take("_fontGame").toString()); marker.insert("modId",params.take("_fontMod").toString()); marker.insert("expectedRevision",m_revision); marker.insert("token",params.value("token").toString());
    }
    if(call==BackendClient::Call::ProfileEditCommit){marker.insert("root",m_dataDir);marker.insert("profileId",params.value("profileId").toString());marker.insert("expectedRevision",m_revision);marker.insert("token",params.value("token").toString());}
    if(call==BackendClient::Call::Apply) {
        marker.insert("applyContextVersion",1);marker.insert("root",m_dataDir);marker.insert("profileId",m_viewedProfileId);marker.insert("expectedRevision",m_revision);marker.insert("token",params.value("token").toString());
        marker.insert("params",params.toVariantMap());
        if(params.contains("deploymentLayout"))marker.insert("deploymentLayout",params.value("deploymentLayout").toString());
    }
    setError({}); setBusy(true, true, status); invalidateReviews();
    // The helper refuses a stale revision and replays by operation ID, so a
    // single-commit edit needs no marker: its outcome is the next snapshot.
    if(singleCommitEdit(call)) { m_editInFlight=true; m_client->nativeRequest(call,params); return true; }
    persistMarker(marker, [this, call, params] {
        // The dispatched acknowledgement is the durable value, never current capability inference.
        m_client->nativeRequest(call,call==BackendClient::Call::Apply?QJsonObject::fromVariantMap(m_marker.value("params").toMap()):params);
    },[this,call] {
        if(call==BackendClient::Call::FontsCommit) m_fonts->finished(false,m_error);
        if(call==BackendClient::Call::ProfileEditCommit) m_options->finished(false,m_error);
    }); return true;
}
bool LibraryModel::commitImport() {
    if (!canImport() || m_importReview.isEmpty()) return false;
    return mutate(int(BackendClient::Call::ImportCommit), {{"token", m_importReview.value("token").toString()}}, tr("Copying reviewed payloads into the native library… Keep the app open."));
}
bool LibraryModel::profileAction(const QString &action, const QString &id, const QString &name) {
    if (!canEditProfiles()) return false;
    QJsonObject params{{"action", action}}; if (!id.isEmpty()) params.insert("profileId", id); if (!name.isEmpty()) params.insert("name", name);
    return mutate(int(BackendClient::Call::Profile), params, tr("Saving profile…"));
}
bool LibraryModel::createProfile(const QString &name) { return profileAction("create", {}, name); }
bool LibraryModel::copyProfile(const QString &id, const QString &name) { return profileAction("copy", id, name); }
bool LibraryModel::renameProfile(const QString &id, const QString &name) { return profileAction("rename", id, name); }
bool LibraryModel::removeProfile(const QString &id) { return profileAction("remove", id, {}); }
bool LibraryModel::previewRestore() {
    if (!canRestore()) return false;
    invalidateReviews(); m_previewBusy=true;setError({});setBusy(true,false,tr("Reviewing the original setup…"));
    m_client->nativeRequest(BackendClient::Call::ApplyPreview,QJsonObject{{"restoreReview",true}});return true;
}
bool LibraryModel::useGameFolder(const QString &path) {
    if (!canEditProfiles() || path.isEmpty()) return false;
    return mutate(int(BackendClient::Call::Profile), {{"action","useGameFolder"},{"gamePath",path}}, tr("Checking game folder…"));
}
bool LibraryModel::previewApply(const QString &gamePath) {
    if (!canPreviewApply() || gamePath.isEmpty()) return false;
    invalidateReviews(); m_previewBusy = true; setError({}); setBusy(true, false, tr("Checking mods…"));
    QJsonObject params{{"profileId",m_viewedProfileId},{"gamePath",gamePath}};
    params.insert("deploymentLayout","shards-v1");
    m_client->nativeRequest(BackendClient::Call::ApplyPreview,params);return true;
}
bool LibraryModel::commitApply() {
    if (!canPreviewApply() || m_applyReview.isEmpty()) return false;
    QJsonObject params{{"token",m_applyReview.value("token").toString()}};
    if(m_applyReview.contains("deploymentLayout")) {
        if(m_applyReview.value("summaryVersion").toInt()!=2||m_applyReview.value("deploymentLayout").toString()!="shards-v1")return false;
        params.insert("deploymentLayout",m_applyReview.value("deploymentLayout").toString());
    }
    return mutate(int(BackendClient::Call::Apply),params,tr("Applying changes…"));
}
bool LibraryModel::restorePrevious() {
    if (!canRestore()) return false;
    m_autoDispatch = true;
    const bool started = mutate(int(BackendClient::Call::Restore), QJsonObject{}, tr("Restoring the previous verified setup… Keep the app open."));
    m_autoDispatch = false;
    if (started) { m_autoApplyTimer.stop(); m_autoDue = false; m_autoCycle = false; emit nativeChanged(); }
    return started;
}
bool LibraryModel::recover() {
    const auto stateError=AppPaths::stateWriteError();if(!stateError.isEmpty()){setError(stateError);return false;}
    return canRecover() && remoteRecover();
}
bool LibraryModel::reconcileOperation() {
    if(remoteMarkerPending() || recoveryWrapperPending()){scheduleRemoteReconcile();return true;}
    if (m_invalidMarker || m_shutdownRequested || m_client->retiring() || m_reconciling || busy() || (m_marker.isEmpty() && !m_outcomeUnknown)) return false;
    if(bootstrapPending()) {
        if(m_otherMarkerFileTasks || m_loading || m_launch->writeGate() || m_client->writing() || !m_client->isRunning() || m_client->nativeRoot()!=m_dataDir || !m_client->readyForRequests())return false;
        const bool mustSave=!m_bootstrapMarkerDurable;
        if(mustSave && !reserveBootstrapMarkerWork())return false;
        const auto session=m_client->sessionGeneration();
        const auto saved=m_marker;
        m_reconciling=true;m_bootstrapRetry=true;m_outcomeUnknown=true;
        auto errorGeneration=m_errorGeneration;
        setBusy(true,true,tr("Checking library creation with its saved operation… Keep the app open."));
        if(m_errorGeneration==errorGeneration)errorGeneration=setError({});
        if(mustSave) {
            persistMarker(saved,[this,session,errorGeneration]{m_bootstrapMarkerDurable=true;dispatchBootstrap(session,errorGeneration);},[this]{
                m_reconciling=false;m_outcomeUnknown=true;setBusy(false,false,tr("The creation context could not be saved. Check config storage before retrying."));
            },true);
        } else dispatchBootstrap(session,errorGeneration);
        return true;
    }
    if(!m_client->readyForRequests())return false;
    if (!m_nativeMode && !m_haveRecoveryContext) {
        setBusy(true, false, tr("Checking library recovery status…"));
        m_client->nativeRequest(BackendClient::Call::RecoverySnapshot); return true;
    }
    if (m_marker.isEmpty()) { setBusy(false); return false; }
    m_reconciling = true; setBusy(true, false, tr("Checking the saved operation outcome…"));
    m_client->nativeRequest(BackendClient::Call::Operation, {{"operationId", m_marker.value("operationId").toString()}}); return true;
}

void LibraryModel::confirmLocalView(bool success,const QString &error) {
    m_localConfirmation = true; m_localConfirmationSuccess = success; m_localConfirmationError = error;
    // Carry display ownership through the confirmation read and marker clear.
    m_localConfirmationErrorGeneration=m_errorGeneration;
    m_outcomeUnknown = true; m_reconciling = false;
    setBusy(true,false,tr("Confirming the currently viewed profile after the saved operation…")); setLoading(true);
    m_client->requestSnapshot(m_statePath,m_requestedProfileId.isEmpty() ? m_viewedProfileId : m_requestedProfileId);
}
bool LibraryModel::dispatchLocalCommit(BackendClient *client,const QString &profile,const QString &revision,QJsonObject params,const QString &modId) {
    return dispatchPreparationCommit(client,profile,revision,params,modId,BackendClient::Call::LocalCommit);
}
bool LibraryModel::dispatchRemoteCommit(const QString &job,const QString &profile,const QString &revision,const QString &token,const QString &mod,
                                        const QString &consent,const QString &cohort,const QString &selection) {
    if(!remoteAdmissionAvailable() || profile!=m_viewedProfileId || revision!=m_revision || !LaunchData::hex(job,32)
        || !LaunchData::id(profile) || !LaunchData::decimal(revision) || !LaunchData::hex(token,32) || !LaunchData::id(mod)
        || !LaunchData::hex(consent,64) || !LaunchData::hex(cohort,64) || !LaunchData::hex(selection,64)) {
        return false;
    }
    const auto id=QUuid::createUuid().toString(QUuid::Id128);
    const QJsonObject context{{"remoteMarkerVersion",2},{"method",int(BackendClient::Call::RemoteCommit)},{"root",m_dataDir},
        {"profileId",profile},{"jobId",job},{"expectedRevision",revision},{"operationId",id},{"modId",mod},
        {"consentFingerprint",consent},{"cohortFingerprint",cohort},{"selectionDigest",selection}};
    if(!RemoteData::marker(context,m_dataDir))return false;

    // Reserve the identity before asynchronous persistence can reenter a load,
    // shutdown or another writer. A failed sync may still have renamed a file.
    m_marker=context.toVariantMap();
    m_remoteToken=token;
    m_remoteMarkerDurable=false;
    m_remoteRecoveryTerminal=false;
    m_remoteOutcome.clear();
    m_outcomeUnknown=true;
    setError({});
    setBusy(true,true,tr("Saving download recovery context…"));
    invalidateReviews();
    persistMarker(m_marker,[this,id]{
        if(!remoteMarkerPending() || m_marker.value("operationId").toString()!=id)return;
        m_remoteMarkerDurable=true;
        m_remoteStage=RemoteStage::Replay;
        setBusy(false,false,tr("Waiting for the exact download operation…"));
        m_remote->replayCommit(m_marker,m_remoteToken);
    },[this]{
        m_remoteMarkerDurable=false;
        m_outcomeUnknown=true;
        setBusy(false,false,tr("The download was not sent because its recovery context could not be synchronized."));
        scheduleRemoteReconcile(1500);
    });
    return true;
}
bool LibraryModel::dispatchReplacementCommit(const QVariantMap &job,const QVariantMap &review,const QString &consent,const QString &revision) {
    if(!remoteAdmissionAvailable() || job.value("profileId").toString()!=m_viewedProfileId
        || revision!=m_revision || review.value("phase")!="ready" || !LaunchData::hex(review.value("reviewToken").toString(),32))return false;
    m_replacementApplyNeedsReview=m_pendingChanges;
    const auto id=QUuid::createUuid().toString(QUuid::Id128);
    const QJsonObject context{{"remoteReplacementMarkerVersion",1},{"method",int(BackendClient::Call::RemoteReplaceCommit)},{"root",m_dataDir},
        {"jobId",job.value("jobId").toString()},{"purpose",job.value("purpose").toString()},{"profileId",job.value("profileId").toString()},
        {"modId",job.value("modId").toString()},{"initialRevision",job.value("initialRevision").toString()},{"expectedRevision",revision},
        {"targetDigest",job.value("targetDigest").toString()},{"afterRecordDigest",review.value("afterRecordDigest").toString()},
        {"operationId",id},{"consentFingerprint",consent},{"cohortFingerprint",job.value("cohort").toMap().value("fingerprint").toString()},
        {"selectionDigest",review.value("selectionDigest").toString()}};
    if(!RemoteData::replacementMarker(context,m_dataDir))return false;
    m_marker=context.toVariantMap();m_remoteToken=review.value("reviewToken").toString();m_remoteMarkerDurable=false;
    m_remoteRecoveryTerminal=false;m_remoteOutcome.clear();m_outcomeUnknown=true;setError({});
    setBusy(true,true,tr("Saving the download context…"));invalidateReviews();
    persistMarker(m_marker,[this,id]{
        if(!replacementMarkerPending() || m_marker.value("operationId").toString()!=id)return;
        m_remoteMarkerDurable=true;m_remoteStage=RemoteStage::Replay;setBusy(false,false,tr("Waiting for the saved operation…"));
        m_remote->replayCommit(m_marker,m_remoteToken);
    },[this]{m_remoteMarkerDurable=false;m_outcomeUnknown=true;
        setBusy(false,false,tr("The operation was not sent because its context could not be saved."));scheduleRemoteReconcile(1500);});
    return true;
}
void LibraryModel::finishRemoteOutcome(bool success) {
    const bool replacement=replacementMarkerPending();const auto failure=m_remoteOutcome.value("error").toMap();
    const bool applyReplacement=replacement && modDetails(m_marker.value("modId").toString()).value("enabled").toBool();
    const auto detail=success?QString():replacement?tr("File choices were not saved. Check the mod and review again."):failure.value("kind").toString()+": "+failure.value("message").toString();
    m_remoteStage=RemoteStage::Idle;
    persistMarker({},[this,success,detail,replacement,applyReplacement]{m_marker.clear();m_remoteMarkerDurable=false;m_remoteRecoveryTerminal=false;m_remoteOutcome.clear();m_remoteToken.clear();m_remoteConfirmationRevision.clear();m_outcomeUnknown=false;m_recoveryRequired=false;m_reconciling=false;
        m_client->setRemoteRecoveryOwned(false);setBusy(false,false,success?(replacement?tr("File choices saved."):tr("Added to the library, disabled.")):tr("The download operation was checked."));m_remote->commitSettled(success,detail);
        if(success && applyReplacement)scheduleAutomaticApply(m_replacementApplyNeedsReview);
        m_replacementApplyNeedsReview=true;
        if(m_shutdownRequested)advanceShutdown();
    },[this]{m_remoteMarkerDurable=false;m_outcomeUnknown=true;setBusy(false,false,tr("The saved outcome was read, but its recovery context could not be cleared."));scheduleRemoteReconcile(1500);});
}
void LibraryModel::remoteTarget(const QJsonObject &request,const QVariantMap &result) {
    if(!replacementMarkerPending() || m_remoteStage!=RemoteStage::Target || m_remoteOutcome.value("status")!="succeeded"
        || request["dataDir"]!=QJsonValue(m_dataDir) || m_client->nativeRoot()!=m_dataDir
        || request["profileId"]!=QJsonValue(m_marker.value("profileId").toString()) || request["modId"]!=QJsonValue(m_marker.value("modId").toString())
        || request["expectedRevision"]!=QJsonValue(m_remoteConfirmationRevision))return;
    if(m_revision!=m_remoteConfirmationRevision || m_viewedProfileId!=m_marker.value("profileId").toString() || m_recoveryRequired) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);return;
    }
    if(result.value("phase")=="ready" && result.value("target").toMap().value("recordDigest")==m_marker.value("afterRecordDigest")) {
        finishRemoteOutcome(true);return;
    }
    m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false,false,tr("The saved operation completed, but the current file choices could not be confirmed."));
    setError(result.value("phase")=="ready"?tr("This mod changed after the saved operation. Check its current choices.")
        :tr("The current mod could not be checked. Its saved operation remains available to review."));
}
void LibraryModel::remoteTargetRejected(const QJsonObject &request,const QString &,const QString &) {
    if(!replacementMarkerPending() || m_remoteStage!=RemoteStage::Target || request["dataDir"]!=QJsonValue(m_dataDir)
        || request["profileId"]!=QJsonValue(m_marker.value("profileId").toString()) || request["modId"]!=QJsonValue(m_marker.value("modId").toString())
        || request["expectedRevision"]!=QJsonValue(m_remoteConfirmationRevision))return;
    m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false,false,tr("The current file choices could not yet be confirmed."));scheduleRemoteReconcile(1500);
}
void LibraryModel::scheduleRemoteReconcile(int delay) {
    if((!remoteMarkerPending() && !recoveryWrapperPending()) || m_remoteRetry.isActive())return;
    m_remoteRetry.start(delay);
}
void LibraryModel::reconcileRemote() {
    if((!remoteMarkerPending() && !recoveryWrapperPending()) || m_invalidMarker)return;
    if(m_client->retiring() || m_otherMarkerFileTasks) {scheduleRemoteReconcile(1500);return;}
    if(m_remoteStage!=RemoteStage::Idle)return;
    if(!m_remoteMarkerDurable) {
        const auto saved=m_marker;
        persistMarker(saved,[this]{
            m_remoteMarkerDurable=true;
            scheduleRemoteReconcile();
        },[this]{
            m_remoteMarkerDurable=false;
            m_outcomeUnknown=true;
            scheduleRemoteReconcile(1500);
        });
        return;
    }
    if(!m_client->isRunning()) {
        if(!m_helperPath.isEmpty())m_client->start(m_helperPath,m_statePath,m_dataDir,true);
        scheduleRemoteReconcile(1500);return;
    }
    if(m_client->nativeRoot()!=m_dataDir || !m_client->readyForRequests() || m_client->writing()) {scheduleRemoteReconcile(1500);return;}
    if(recoveryWrapperPending()) {
        m_client->setRemoteRecoveryOwned(true);
        m_remoteStage=RemoteStage::Recovery;
        setBusy(true,true,tr("Checking the exact saved recovery…"));
        m_client->nativeRequest(BackendClient::Call::Recover,{{"operationId",m_marker.value("operationId").toString()},{"expectedRevision",m_marker.value("expectedRevision").toString()}});
        return;
    }
    const bool legacy=m_marker.value("remoteMarkerVersion").toInt()==1;
    if(m_marker.contains("recoveryId") && !m_remoteRecoveryTerminal && !legacy) {
        if(!m_marker.contains("recoveryExpectedRevision")) {
            m_outcomeUnknown=true;setBusy(false,false,tr("The recovery context has no saved revision. Preserve it for manual recovery."));return;
        }
        // Latch before nativeRequest: a lost acknowledgement may conceal an
        // asynchronous owner even if the write call fails immediately.
        m_client->setRemoteRecoveryOwned(true);
        m_remoteStage=RemoteStage::Recovery;
        setBusy(true,true,tr("Checking the exact saved recovery…"));
        m_client->nativeRequest(BackendClient::Call::Recover,{{"operationId",m_marker.value("recoveryId").toString()},{"expectedRevision",m_marker.value("recoveryExpectedRevision").toString()}});
        return;
    }
    m_remoteStage=RemoteStage::Operation;m_reconciling=true;
    m_client->nativeRequest(BackendClient::Call::Operation,{{"operationId",m_marker.value("operationId").toString()}},{{"remoteOperation",true}});
}
bool LibraryModel::remoteRecover() {
    if(m_remoteStage!=RemoteStage::Idle || m_otherMarkerFileTasks)return false;
    if(recoveryWrapperPending() || (remoteMarkerPending() && m_marker.contains("recoveryId"))){scheduleRemoteReconcile();return true;}
    if(remoteMarkerPending() && !replacementMarkerPending() && m_marker.value("remoteMarkerVersion").toInt()!=2)return false;
    m_remoteStage=RemoteStage::Inspect;
    setBusy(true,false,tr("Inspecting the saved recovery context…"));
    m_client->nativeRequest(BackendClient::Call::RecoveryInspect);
    return true;
}
void LibraryModel::remoteRecoverySnapshot(const LibrarySnapshot &snapshot) {
    if(recoveryWrapperPending() && m_remoteStage==RemoteStage::RecoveryView) {applySnapshot(snapshot);return;}
    if(m_remoteStage!=RemoteStage::RecoverySnapshot || m_client->nativeRoot()!=m_dataDir
        || snapshot.sourceKind!="native" || !LaunchData::decimal(snapshot.revision)) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(tr("Recovery could not pin a current native revision."));return;
    }
    m_revision=snapshot.revision;m_recoveryRequired=snapshot.recoveryRequired;m_haveRecoveryContext=true;
    if(remoteMarkerPending() && !snapshot.recoveryRequired) {
        m_remoteStage=RemoteStage::Idle;setBusy(false);scheduleRemoteReconcile();return;
    }
    auto id=QUuid::createUuid().toString(QUuid::Id128);
    if(id==m_marker.value("operationId").toString())id=QUuid::createUuid().toString(QUuid::Id128);
    saveRecoveryChoice(id,snapshot.revision);
}
void LibraryModel::recoveryInspection(const QJsonObject &request,const QVariantMap &result) {
    const auto errorGeneration=m_errorGeneration;
    if(request["dataDir"]!=QJsonValue(m_dataDir) || m_client->nativeRoot()!=m_dataDir)return;
    const auto state=result.value("state").toString();
    if(m_remoteStage==RemoteStage::LegacyView && recoveryWrapperPending()) {
        if(state=="legacyUnimported" && result.value("expectedRevision").toString()=="0") {
            m_nativeMode=false;m_haveRecoveryContext=true;m_revision="0";
            emit modeChanged();emit nativeChanged();finishRecoveryWrapper(false,m_remoteOutcome.value("error").toMap().value("kind")=="nativeNotImported",errorGeneration);return;
        }
        m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);
        setError(tr("The clean unimported library state could not be confirmed."));scheduleRemoteReconcile(1500);return;
    }
    if(m_remoteStage!=RemoteStage::Inspect)return;
    if(state=="waiting") {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);
        setError(tr("Recovery evidence is incomplete. Preserve it and try again when it can be inspected."));return;
    }
    QString legacyId;
    if(!remoteMarkerPending()) {
        legacyId=m_marker.value("recoveryId").toString();
        if(legacyId.isEmpty() && m_marker.value("method",-1).toInt()==int(BackendClient::Call::Recover))legacyId=m_marker.value("operationId").toString();
    }
    const auto existing=result.value("existingRecovery").toMap().value("params").toMap();
    if(!legacyId.isEmpty() && (state!="ready" || existing.value("operationId").toString()!=legacyId)) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);
        setError(tr("The older recovery context has no matching recorded revision. Preserve its evidence."));return;
    }
    if(state=="ready") {
        auto id=existing.isEmpty()?QUuid::createUuid().toString(QUuid::Id128):existing.value("operationId").toString();
        const auto target=result.value("targetOperationId").toString();
        const auto original=m_marker.value("operationId").toString();
        if(replacementMarkerPending() && target==original) {
            const auto input=RemoteData::replacementInput(QJsonObject::fromVariantMap(m_marker));
            const auto hash=QString::fromLatin1(QCryptographicHash::hash(LaunchData::canonical(input),QCryptographicHash::Sha256).toHex());
            if(result.value("targetInputSha256").toString()!=hash) {
                m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(tr("The recovery evidence does not match the saved operation."));return;
            }
        }
        if(existing.isEmpty() && (id==target || (!original.isEmpty() && id==original)))id=QUuid::createUuid().toString(QUuid::Id128);
        if(id==target || (!original.isEmpty() && id==original
            && (existing.isEmpty() || m_marker.value("method",-1).toInt()!=int(BackendClient::Call::Recover)))) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(tr("Recovery identity conflicts with saved operation evidence."));return;
        }
        saveRecoveryChoice(id,result.value("expectedRevision").toString());return;
    }
    if(state=="notRemote") {
        m_remoteStage=RemoteStage::RecoverySnapshot;
        setBusy(true,false,tr("Reading the current revision for recovery…"));
        m_client->nativeRequest(BackendClient::Call::RecoverySnapshot);return;
    }
    if(state=="legacyImportRecovery" || state=="legacyUnimported") {
        if(remoteMarkerPending()) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);
            setError(tr("The saved download needs a native library view before recovery."));return;
        }
        auto id=QUuid::createUuid().toString(QUuid::Id128);
        if(id==m_marker.value("operationId").toString())id=QUuid::createUuid().toString(QUuid::Id128);
        saveRecoveryChoice(id,QStringLiteral("0"));return;
    }
    m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(tr("Recovery inspection returned an unsupported state."));
}
void LibraryModel::saveRecoveryChoice(const QString &id,const QString &revision) {
    if(!LaunchData::id(QJsonValue(id)) || !LaunchData::decimal(QJsonValue(revision))) {
        m_remoteStage=RemoteStage::Idle;setBusy(false);setError(tr("Recovery context could not be represented safely."));return;
    }
    QVariantMap next;
    if(remoteMarkerPending()) {
        next=m_marker;next["recoveryId"]=id;next["recoveryExpectedRevision"]=revision;
        if(!(replacementMarkerPending()?RemoteData::replacementMarker(QJsonObject::fromVariantMap(next),m_dataDir):RemoteData::marker(QJsonObject::fromVariantMap(next),m_dataDir))) {
            m_remoteStage=RemoteStage::Idle;setBusy(false);setError(tr("Recovery context could not be represented safely."));return;
        }
    } else {
        if(id==m_marker.value("operationId").toString() && m_marker.value("method",-1).toInt()!=int(BackendClient::Call::Recover)) {
            m_remoteStage=RemoteStage::Idle;setBusy(false);setError(tr("Recovery identity conflicts with the saved operation."));return;
        }
        const QJsonValue original=m_marker.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(QJsonObject::fromVariantMap(m_marker));
        next=QJsonObject{{"nativeRecoveryMarkerVersion",1},{"method",int(BackendClient::Call::Recover)},
            {"root",m_dataDir},{"operationId",id},{"expectedRevision",revision},{"originalMarker",original}}.toVariantMap();
        const auto bytes=QJsonDocument::fromVariant(next).toJson(QJsonDocument::Compact);
        if(bytes.size()>65536 || !LaunchData::strictJson(bytes)) {
            m_remoteStage=RemoteStage::Idle;setBusy(false);setError(tr("The saved context is too large or deeply nested to wrap safely."));return;
        }
    }
    // Reserve the complete identity before asynchronous persistence can
    // reenter shutdown, a root switch, or another writer.
    m_marker=next;m_remoteMarkerDurable=false;m_remoteRecoveryTerminal=false;m_outcomeUnknown=true;
    setBusy(true,true,tr("Saving the exact recovery context…"));
    persistMarker(next,[this]{
        m_remoteMarkerDurable=true;m_remoteStage=RemoteStage::Idle;setBusy(false);scheduleRemoteReconcile();
    },[this]{
        m_remoteMarkerDurable=false;m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);
    });
}
void LibraryModel::recoveryWrapperResult(const LibrarySnapshot &snapshot,const QString &digest) {
    if(!recoveryWrapperPending() || m_client->nativeRoot()!=m_dataDir || snapshot.sourceKind!="native"
        || snapshot.revision.toULongLong()<m_marker.value("expectedRevision").toString().toULongLong()) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);return;
    }
    // Historical recovery may commit the interrupted action and then its own
    // recovery revision. Exact replay still binds the recorded revision/digest.
    if(m_remoteStage==RemoteStage::RecoveryReplay && m_remoteOutcome.value("status")=="succeeded"
        && snapshot.revision==m_remoteOutcome.value("confirmedRevision").toString()
        && !digest.isEmpty() && digest==m_remoteOutcome.value("confirmedSnapshotDigest").toString()) {
        recoveryWrapperTerminal();return;
    }
    if(m_remoteStage!=RemoteStage::Recovery) {m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);return;}
    m_remoteStage=RemoteStage::RecoveryOperation;
    setBusy(true,false,tr("Checking the stored recovery outcome…"));
    m_client->nativeRequest(BackendClient::Call::Operation,{{"operationId",m_marker.value("operationId").toString()}},{{"recoveryOperation",true}});
}
void LibraryModel::recoveryWrapperOperation(const QVariantMap &outcome) {
    if((!recoveryWrapperPending() && !remoteMarkerPending()) || m_client->nativeRoot()!=m_dataDir)return;
    const auto id=m_marker.value(remoteMarkerPending()?"recoveryId":"operationId").toString();
    const auto revision=m_marker.value(remoteMarkerPending()?"recoveryExpectedRevision":"expectedRevision").toString();
    if(m_remoteStage==RemoteStage::RecoveryOperation && outcome.value("operationId").toString()==id) {
        const auto status=outcome.value("status").toString();
        const auto confirmed=outcome.value("confirmedRevision").toString();
        const bool revisionMatches=recoveryWrapperPending() ? LaunchData::decimal(QJsonValue(confirmed)) && confirmed.toULongLong()>=revision.toULongLong() : confirmed==revision;
        if((status!="succeeded" && status!="failed") || (status=="succeeded" && !revisionMatches)) {
            m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1500);return;
        }
        m_remoteOutcome=outcome;m_remoteStage=RemoteStage::RecoveryReplay;
        setBusy(true,true,tr("Replaying the exact recovery input…"));
        m_client->nativeRequest(BackendClient::Call::Recover,{{"operationId",id},{"expectedRevision",revision}});
        return;
    }
    const auto original=remoteMarkerPending()?m_marker:m_marker.value("originalMarker").toMap();
    if(m_remoteStage==RemoteStage::OriginalOperation && outcome.value("operationId").toString()==original.value("operationId").toString())recoveryWrapperView();
}
void LibraryModel::recoveryWrapperTerminal() {
    // Exact replay has returned after the actual worker joined, including for a
    // stored pre-dispatch rejection. Keep the wrapper until the current view.
    m_client->setRemoteRecoveryOwned(false);m_reconciling=true;
    const auto original=remoteMarkerPending()?m_marker:m_marker.value("originalMarker").toMap();
    if(original.value("method",-1).toInt()==int(BackendClient::Call::Apply) && m_confirmationTarget==ConfirmationTarget::Apply) {
        // Recovery is not the pending Apply confirmation. Re-read its original
        // operation after terminal recovery rather than clearing that intent.
        m_localConfirmation=false;m_confirmationTarget=ConfirmationTarget::None;
    }
    if(original.isEmpty()){recoveryWrapperView();return;}
    m_remoteStage=RemoteStage::OriginalOperation;
    setBusy(true,false,tr("Checking the original saved operation…"));
    m_client->nativeRequest(BackendClient::Call::Operation,{{"operationId",original.value("operationId").toString()}},{{"recoveryOperation",true}});
}
void LibraryModel::recoveryWrapperView() {
    if(recoveryWrapperPending() && m_remoteOutcome.value("status")=="failed"
        && m_marker.value("expectedRevision")=="0") {
        m_remoteStage=RemoteStage::LegacyView;
        setBusy(true,false,tr("Confirming the clean unimported library state…"));
        m_client->nativeRequest(BackendClient::Call::RecoveryInspect);return;
    }
    m_remoteStage=RemoteStage::RecoveryView;
    m_recoveryViewProfile=(!m_defaultMode && !m_nativeMode)?QString():m_viewedProfileId;
    setBusy(true,false,tr("Confirming the current library view…"));setLoading(true);
    if(!m_defaultMode && !m_nativeMode) m_client->nativeRequest(BackendClient::Call::RecoverySnapshot);
    else m_client->requestSnapshot(m_statePath,m_recoveryViewProfile,true);
}
void LibraryModel::finishRecoveryWrapper(bool recoveryStillRequired,bool cleanUnimported,quint64 errorGeneration) {
    const auto original=m_marker.value("originalMarker").toMap();
    const bool success=cleanUnimported || m_remoteOutcome.value("status")=="succeeded";
    const auto failure=m_remoteOutcome.value("error").toMap();
    const auto detail=success?QString():replacementRecoveryContext()?tr("Recovery could not be completed. The original download is still being checked."):failure.value("kind").toString()+": "+failure.value("message").toString();
    // Only this obsolete snapshot error is explained by a confirmed clean,
    // unimported recovery. Preserve every later error, even identical text.
    const bool staleNotImported=cleanUnimported && m_error.startsWith("nativeNotImported: ");
    persistMarker(original,[this,original,success,recoveryStillRequired,detail,staleNotImported,errorGeneration]{
        m_remoteMarkerDurable=false;m_remoteStage=RemoteStage::Idle;m_remoteOutcome.clear();m_reconciling=false;
        // Recovery does not establish the original operation's outcome.
        m_outcomeUnknown=!original.isEmpty();m_recoveryBarrier=false;m_recoveryRequired=recoveryStillRequired;
        setBusy(false,false,success?tr("Recovery completed; the original operation remains available for reconciliation."):tr("Recovery was rejected; the library still requires recovery."));
        if(staleNotImported)clearErrorIfCurrent(errorGeneration);
        if(!detail.isEmpty())setErrorIfCurrent(errorGeneration,detail);
        if(!original.isEmpty() && !m_shutdownRequested)QTimer::singleShot(0,this,[this]{reconcileOperation();});
        if(m_shutdownRequested)advanceShutdown();else if(m_switching)advanceLoad();
    },[this]{
        m_remoteMarkerDurable=false;m_remoteStage=RemoteStage::Idle;m_reconciling=false;m_outcomeUnknown=true;
        setBusy(false);scheduleRemoteReconcile(1500);
    });
}
void LibraryModel::remoteRecoveryResult(const LibrarySnapshot &snapshot,const QString &digest) {
    if(!remoteMarkerPending() || (m_remoteStage!=RemoteStage::Recovery && (m_remoteStage!=RemoteStage::RecoveryReplay || m_remoteOutcome.value("status")!="succeeded")) || (!replacementMarkerPending() && m_marker.value("remoteMarkerVersion").toInt()!=2)
        || m_client->nativeRoot()!=m_dataDir || snapshot.sourceKind!="native" || snapshot.revision!=m_marker.value("recoveryExpectedRevision").toString()
        || (m_remoteStage==RemoteStage::RecoveryReplay && (digest.isEmpty() || digest!=m_remoteOutcome.value("confirmedSnapshotDigest").toString()))) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);setError(replacementMarkerPending()?tr("Recovery could not be confirmed yet."):tr("Recovery returned mismatched evidence."));scheduleRemoteReconcile(1500);return;
    }
    m_client->setRemoteRecoveryOwned(false);m_remoteRecoveryTerminal=true;m_recoveryRequired=false;m_remoteStage=RemoteStage::Idle;
    m_outcomeUnknown=true;setBusy(false,false,tr("Recovery completed; checking the original download operation…"));scheduleRemoteReconcile();
}
void LibraryModel::remoteOperation(const QVariantMap &outcome) {
    if(m_remoteStage!=RemoteStage::Operation || outcome.value("operationId").toString()!=m_marker.value("operationId").toString())return;
    m_reconciling=false;const auto status=outcome.value("status").toString();
    if(m_marker.value("remoteMarkerVersion").toInt()==1) {
        m_remoteOutcome=outcome;m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;
        setBusy(false,false,m_marker.contains("recoveryId") ? tr("The older recovery identity has no saved revision; preserve its context.")
            : tr("The older download context is retained for read-only reconciliation."));return;
    }
    if(status=="recoveryRequired") {
        m_remoteStage=RemoteStage::Idle;m_recoveryRequired=true;m_outcomeUnknown=true;setBusy(false,false,tr("Recover the interrupted download before continuing."));scheduleRemoteReconcile(5000);return;
    }
    if(status=="succeeded") {
        const auto expected=m_marker.value("expectedRevision").toString().toULongLong();
        if(expected==std::numeric_limits<quint64>::max() || outcome.value("confirmedRevision").toString()!=QString::number(expected+1)
            || outcome.value("confirmedProfile").toString()!=m_marker.value("profileId").toString()) {
            m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setError(tr("The operation snapshot does not match its pinned profile and revision."));scheduleRemoteReconcile(1500);return;
        }
    }
    if(status!="succeeded" && status!="failed" && status!="unknown") {m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;scheduleRemoteReconcile(1500);return;}
    m_remoteOutcome=outcome;
    m_remoteStage=RemoteStage::Replay;m_remote->replayCommit(m_marker,status=="unknown"?m_remoteToken:QString());
}
void LibraryModel::remoteReplay(BackendClient::Call call,const QJsonObject &request,const LibrarySnapshot &snapshot) {
    if(int(call)!=m_marker.value("method").toInt() || !remoteMarkerPending() || m_remoteStage!=RemoteStage::Replay || !remoteRequestMatches(request,m_marker,m_dataDir))return;
    const auto expected=m_marker.value("expectedRevision").toString().toULongLong();
    const auto mod=m_marker.value("modId").toString();
    const auto it=std::find_if(snapshot.mods.cbegin(),snapshot.mods.cend(),[&](const LibraryMod &row){return row.id==mod;});
    if(m_remoteOutcome.value("status")!="succeeded" || expected==std::numeric_limits<quint64>::max()
        || snapshot.sourceKind!="native" || snapshot.selectedProfileId!=m_marker.value("profileId").toString() || snapshot.revision!=QString::number(expected+1)
        || it==snapshot.mods.cend() || (!replacementMarkerPending() && it->enabled)) {
        m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false);scheduleRemoteReconcile(1000);return;
    }
    m_remoteToken.clear();m_remoteStage=RemoteStage::Status;m_remote->reconcileStatus(m_marker.value("jobId").toString());
}
void LibraryModel::remoteReplayRejected(BackendClient::Call call,const QJsonObject &request,const QString &kind,const QString &message) {
    if(int(call)!=m_marker.value("method").toInt() || !remoteMarkerPending() || m_remoteStage!=RemoteStage::Replay || !remoteRequestMatches(request,m_marker,m_dataDir))return;
    const auto failure=m_remoteOutcome.value("error").toMap();
    if(m_remoteOutcome.value("status")=="failed" && kind==failure.value("kind").toString() && message==failure.value("message").toString()) {
        m_remoteToken.clear();m_remoteStage=RemoteStage::Status;m_remote->reconcileStatus(m_marker.value("jobId").toString());return;
    }
    if(kind!="remoteBusy" && kind!="outcomeUnknown" && kind!="launchBusy")m_remoteToken.clear();
    m_remoteStage=RemoteStage::Idle;m_outcomeUnknown=true;setBusy(false,false,tr("The download outcome remains unconfirmed."));scheduleRemoteReconcile(1500);
}
void LibraryModel::remoteStatus(const QVariantMap &status) {
    if(!remoteMarkerPending() || m_remoteStage!=RemoteStage::Status || status.value("jobId").toString()!=m_marker.value("jobId").toString()
        || status.value("root").toString()!=m_dataDir || status.value("profileId").toString()!=m_marker.value("profileId").toString()
        || status.value("cohort").toMap().value("fingerprint").toString()!=m_marker.value("cohortFingerprint").toString())return;
    if(replacementMarkerPending() && (status.value("purpose")!=m_marker.value("purpose") || status.value("modId")!=m_marker.value("modId")
        || status.value("targetDigest")!=m_marker.value("targetDigest") || status.value("initialRevision")!=m_marker.value("initialRevision")))return;
    const auto reviewRevision=status.value("reviewRevision");
    if(!reviewRevision.isNull() && reviewRevision.isValid() && reviewRevision.toString()!=m_marker.value("expectedRevision").toString())return;
    const bool success=m_remoteOutcome.value("status")=="succeeded";const auto operation=status.value("operationId").toString();const auto state=status.value("state").toString();
    const auto failure=m_remoteOutcome.value("error").toMap();
    const bool terminal=success
        ? operation==m_marker.value("operationId").toString() && state=="installed"
        : ((operation==m_marker.value("operationId").toString() && state=="failed")
            || (operation.isEmpty() && failure.value("kind")=="staleReview" && state!="committing"));
    if(!terminal)return;
    m_remoteStage=RemoteStage::Snapshot;m_outcomeUnknown=true;setBusy(true,false,tr("Confirming the pinned profile view…"));setLoading(true);
    m_client->requestSnapshot(m_statePath,m_marker.value("profileId").toString());
}
bool LibraryModel::dispatchOptionsAttach(BackendClient *client,const QString &profile,const QString &revision,QJsonObject params,const QString &modId) {
    return dispatchPreparationCommit(client,profile,revision,params,modId,BackendClient::Call::OptionsAttach);
}
bool LibraryModel::dispatchPreparationCommit(BackendClient *client,const QString &profile,const QString &revision,QJsonObject params,const QString &modId,BackendClient::Call call) {
    const bool attachment=call==BackendClient::Call::OptionsAttach;
    if(call!=BackendClient::Call::LocalCommit&&!attachment)return false;
    if(m_launch->writeGate()||m_shutdownRequested||m_switching||!client||!client->localSession()||client->nativeRoot()!=m_dataDir||client->writing()||!client->readyForRequests()||busy()||writeBusy()||m_outcomeUnknown||m_editLost||m_recoveryRequired||!m_marker.isEmpty()||!m_nativeMode||revision!=m_revision)return false;
    const auto id=QUuid::createUuid().toString(QUuid::Id128);const auto epoch=m_loadEpoch;
    params.insert("profileId",profile);params.insert("expectedRevision",revision);params.insert("operationId",id);if(attachment)params.insert("modId",modId);
    auto connections=std::make_shared<QList<QMetaObject::Connection>>();
    auto clean=[connections]{for(const auto &c:*connections)QObject::disconnect(c);connections->clear();};
    auto matches=[this,id,epoch]{return epoch==m_loadEpoch&&m_marker.value("operationId").toString()==id;};
    auto finish=[this,attachment,jobId=params.value("jobId").toString()](bool success,const QString &message){if(attachment)m_options->finished(success,message,jobId);else m_localImport->commitConfirmed(success,message);};
    auto success=[this,matches,clean,attachment,modId](const LibrarySnapshot &snapshot){
        if(!matches())return;clean();if(!attachment)m_fonts->seedOffer(snapshot,modId);m_revision=snapshot.revision;m_confirmationTarget=attachment ? ConfirmationTarget::OptionsAttach : ConfirmationTarget::Import;confirmLocalView(true);
    };
    if(attachment)connections->append(connect(client,&BackendClient::optionsCommitted,this,success));
    else connections->append(connect(client,&BackendClient::localCommitted,this,success));
    connections->append(connect(client,&BackendClient::unknownOutcome,this,[this,matches,clean,finish](const QString &message){
        if(!matches())return;clean();m_outcomeUnknown=true;setBusy(false,false,tr("The operation may have completed. Check status before continuing."));setError({});finish(false,message);
    }));
    connections->append(connect(client,&BackendClient::rejected,this,[this,matches,clean,finish,call](BackendClient::Call actual,const QString &kind,const QString &message){
        if(actual!=call||!matches())return;clean();
        if(kind=="recoveryRequired"){m_outcomeUnknown=true;m_recoveryRequired=true;setBusy(false);finish(false,kind+": "+message);return;}
        clearMarker([this,kind,message,finish]{setBusy(false);finish(false,kind+": "+message);if(kind=="staleRevision")refresh();});
    }));
    setError({});setBusy(true,true,tr("Saving reviewed retained content… Keep the app open."));invalidateReviews();
    persistMarker({{"root",m_dataDir},{"operationId",id},{"method",int(call)},{"profileId",profile},{"modId",modId},{"jobId",params.value("jobId").toString()},{"expectedRevision",revision}},[this,client,params,epoch,call]{if(m_loadEpoch==epoch)client->nativeRequest(call,params);},[this,clean,finish]{clean();finish(false,m_error);});return true;
}
bool LibraryModel::profileEditsAvailable()const{return !m_launch->writeGate()&&!m_shutdownRequested&&!m_switching&&m_nativeMode&&!m_viewedProfileId.isEmpty()&&!m_demoMode&&!busy()&&!m_loading&&!m_outcomeUnknown&&!m_editLost&&!m_recoveryRequired&&m_marker.isEmpty()&&m_client->isRunning()&&m_client->readyForRequests();}
bool LibraryModel::dispatchProfileEditPreview(const QString &root,const QString &profile,const QString &revision,const QJsonObject &change){
    if(!profileEditsAvailable()||root!=m_dataDir||profile!=m_viewedProfileId||revision!=m_revision)return false;
    QJsonObject params{{"profileId",profile},{"expectedRevision",revision},{"change",change}};
    params.insert("capacityLayout","shards-v1");
    setError({});setBusy(true,false,tr("Reviewing saved setup changes…"));invalidateReviews();m_client->nativeRequest(BackendClient::Call::ProfileEditPreview,params);return true;
}
bool LibraryModel::dispatchProfileEditCommit(const QString &root,const QString &profile,const QString &revision,const QString &token){
    if(!profileEditsAvailable()||root!=m_dataDir||profile!=m_viewedProfileId||revision!=m_revision||!FontData::hex(token,32))return false;
    m_editApplyNeedsReview=m_pendingChanges;
    return mutate(int(BackendClient::Call::ProfileEditCommit),{{"profileId",profile},{"token",token}},tr("Saving desired setup…"));
}

QString LibraryModel::localPath(const QUrl &url) const { return url.isLocalFile() ? url.toLocalFile() : QString{}; }

#include "librarymodel.moc"

bool LibraryModel::closeBlocked() const { return (!m_autoBlocked&&!m_autoToggles.isEmpty())||m_closeAfterToggles||writeBusy()||m_client->writing()||m_client->launchWriting()||(m_shutdownRequested&&(m_client->retiring()||m_launch->m_fileBusy||!m_catalog->shutdownReady()||!m_remote->shutdownReady()||!m_localImport->retirementReady(m_dataDir,m_loadEpoch)||remoteMarkerPending()||recoveryWrapperPending())); }
void LibraryModel::requestShutdown(){
    if(m_shutdownRequested)return;
    if (!m_autoToggles.isEmpty() && !m_autoBlocked && m_launch->gameStatus().value("state") != "running") {
        m_closeAfterToggles = true; m_autoApplyTimer.stop(); m_autoDue = false;
        emit shutdownChanged(); advanceAutoApply(); return;
    }
    m_shutdownRequested=true;
    resetLibraryMediaContext();
    m_shutdownReady=false;
    m_catalog->shutdown();
    m_remote->shutdown();
    m_localImport->beginRetirement(m_dataDir,m_loadEpoch);
    m_launch->shutdown();
    emit nativeChanged();
    advanceShutdown();
}
void LibraryModel::advanceShutdown(){
    if(!m_shutdownRequested||m_shutdownReady)return;
    if(remoteMarkerPending() || recoveryWrapperPending())scheduleRemoteReconcile();
    if(m_client->launchOwned() && m_client->remoteOwned() && !m_client->launchRetiring())m_client->retireLaunch();
    if(writeBusy()||m_client->writing()||m_client->retiring()||m_launch->m_fileBusy||m_client->launchOwned()
        || !m_catalog->shutdownReady() || !m_remote->shutdownReady() || !m_localImport->retirementReady(m_dataDir,m_loadEpoch)
        || remoteMarkerPending() || recoveryWrapperPending() || m_otherMarkerFileTasks || m_bootstrapMarkerFileBusy) {emit shutdownChanged();return;}
    if(m_client->isRunning()) {
        m_client->stop();
        if(m_client->retiring()){emit shutdownChanged();return;}
    }
    m_shutdownReady=true;
    emit shutdownChanged();
}

int LibraryModel::rowForId(const QString &id) const {
    const int row=m_rows->rowForId(id);
    return row < 0 ? -1 : mapFromSource(m_rows->index(row,0)).row();
}


bool LibraryModel::canScanInstalledMods() const {
    return !m_demoMode && !m_helperPath.isEmpty() && m_client->readyForRequests();
}
bool LibraryModel::installedRead(const QString &method,const QJsonObject &params) {
    if(m_installedModsBusy || m_helperPath.isEmpty() || m_shutdownRequested)return false;
    m_installedModsBusy=true;m_installedModsError.clear();emit installedModsChanged();
    auto *process=new QProcess(this);auto *deadline=new QTimer(process);deadline->setSingleShot(true);
    const auto epoch=m_loadEpoch; const auto scanGame=params.value("gamePath").toString();
    connect(deadline,&QTimer::timeout,process,[process]{process->kill();});deadline->start(120000);
    connect(process,&QProcess::readyReadStandardError,process,[process]{process->readAllStandardError();});
    auto bytes=std::make_shared<QByteArray>();
    connect(process,&QProcess::readyReadStandardOutput,process,[process,bytes]{*bytes+=process->readAllStandardOutput();if(bytes->size()>32*1024*1024)process->kill();});
    connect(process,qOverload<int,QProcess::ExitStatus>(&QProcess::finished),this,[this,process,bytes,method,epoch,scanGame](int code,QProcess::ExitStatus status){
        *bytes+=process->readAllStandardOutput();
        if(epoch==m_loadEpoch) {
            const auto reply=QJsonDocument::fromJson(bytes->trimmed()).object();const auto result=reply.value("result").toObject();
            if(code || status!=QProcess::NormalExit || result.isEmpty())m_installedModsError=reply.value("error").toObject().value("message").toString("Installed package discovery failed.");
            else if(method=="native.game.detect") {m_detectedGamePath=result.value("detectedGamePath").toString();m_detectedGamePaths.clear();for(const auto &p:result.value("candidates").toArray())m_detectedGamePaths.append(p.toString());}
            else {if(!m_installedMetadataKey.isEmpty())m_catalog->cancelLibraryMedia(m_installedMetadataKey);
                m_installedMetadataKey.clear();m_installedMetadataId.clear();
                m_installedMods=result.value("items").toArray().toVariantList();m_installedScanGame=scanGame;m_installedMetadataAttempted.clear();m_installedMetadataTimer.start();}
        }
        m_installedModsBusy=false;emit installedModsChanged();process->deleteLater();
    });
    connect(process,&QProcess::errorOccurred,this,[this,process](QProcess::ProcessError error){if(error==QProcess::FailedToStart){m_installedModsBusy=false;m_installedModsError="Cannot start the helper.";emit installedModsChanged();process->deleteLater();}});
    process->start(m_helperPath,QStringList{});
    const int id=method=="native.game.detect"?179:180;
    process->write(QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",id},{"method",method},{"params",params}}).toJson(QJsonDocument::Compact)+"\n");process->closeWriteChannel();return true;
}
bool LibraryModel::detectGame(){return installedRead("native.game.detect",{});}
bool LibraryModel::scanInstalledMods(const QString &path){
    if(!canScanInstalledMods())return false;
    const auto game=path.isEmpty()?(m_gamePath.isEmpty()?m_detectedGamePath:m_gamePath):path;
    if(game.isEmpty())return false;QJsonObject params{{"gamePath",game}};if(!m_sourcePath.isEmpty())params.insert("sourcePath",m_sourcePath);if(m_nativeMode)params.insert("dataDir",m_dataDir);
    return installedRead("native.installed.scan",params);
}
bool LibraryModel::importInstalledMod(const QString &id,bool confirmSuggestedLink){
    if(m_installedModsBusy || (!m_gamePath.isEmpty() && m_installedScanGame!=m_gamePath) || !m_localImport->canPrepare())return false;
    QVariantMap row;for(const auto &item:m_installedMods){const auto candidate=item.toMap();if(candidate.value("id").toString()==id){if(!row.isEmpty())return false;row=candidate;}}
    if(row.isEmpty() || !row.value("error").isNull() || row.value("alreadyInLibrary").toBool())return false;
    // Metadata is a user-confirmed link only; never fabricate a remote download receipt.
    const auto match=row.value("suggestedMatch").toMap();
    QList<QUrl> roots;
    for(const auto &p:row.value("packages").toStringList())roots.append(QUrl::fromLocalFile(p));
    if(roots.isEmpty())roots.append(QUrl::fromLocalFile(row.value("path").toString()));
    if(!m_localImport->prepare(roots))return false;
    if(!match.isEmpty()) {
        const auto job=m_localImport->jobId();
        auto connection=std::make_shared<QMetaObject::Connection>();
        *connection=connect(m_localImport,&LocalImportController::changed,this,[this,job,match,connection,confirmSuggestedLink]{
            if(m_localImport->jobId()!=job){disconnect(*connection);return;}
            if(!m_localImport->presentationReady())return;
            disconnect(*connection);auto metadata=m_localImport->metadataDraft();metadata.insert("name",match.value("name"));if(confirmSuggestedLink)metadata.insert("link",match.value("link"));metadata.insert("author",match.value("author").toString());
                // Same limits the backend enforces on local metadata (UTF-8 bytes, no control characters).
                const auto clean=[](QString text,int maxBytes,bool multiline){
                    text.removeIf([multiline](QChar c){return c.category()==QChar::Other_Control&&!(multiline&&(c==u'\n'||c==u'\t'));});
                    while(text.toUtf8().size()>maxBytes)text.chop(1);
                    return text.trimmed();};
                if(const auto category=clean(match.value("category").toString(),128,false);!category.isEmpty())metadata.insert("category",category);
                if(const auto description=clean(match.value("description").toString(),16384,true);!description.isEmpty())metadata.insert("description",description);
                m_localImport->setMetadataDraft(metadata);
        });
    }
    m_localImport->setVisible(true);return true;
}
bool LibraryModel::identifyInstalledMod(const QString &id){
    if(m_installedModsBusy || m_shutdownRequested)return false;
    int index=-1;for(int i=0;i<m_installedMods.size();++i)if(m_installedMods[i].toMap().value("id").toString()==id){if(index>=0)return false;index=i;}
    if(index<0 || id.size()!=64)return false;
    auto *manager=new QNetworkAccessManager(this);
    // Explicit allowlist: this operation has one endpoint and follows no redirects.
    const QUrl endpoint("https://api.deadlockmods.app/api/v2/vpk-analyse-hashes");
    if(endpoint.host()!=QStringLiteral("api.deadlockmods.app")){manager->deleteLater();return false;}
    QNetworkRequest request(endpoint);request.setHeader(QNetworkRequest::ContentTypeHeader,"application/json");
    request.setAttribute(QNetworkRequest::RedirectPolicyAttribute,QNetworkRequest::ManualRedirectPolicy);request.setTransferTimeout(30000);
    const auto sha256=m_installedMods[index].toMap().value("sha256").toString();
    auto *reply=manager->post(request,QJsonDocument(QJsonObject{{"sha256",sha256},{"contentSignature",""}}).toJson(QJsonDocument::Compact));
    m_installedModsBusy=true;m_installedModsError.clear();emit installedModsChanged();const auto epoch=m_loadEpoch;
    connect(reply,&QNetworkReply::readyRead,reply,[reply]{if(reply->bytesAvailable()>1024*1024)reply->abort();});
    connect(reply,&QNetworkReply::finished,this,[this,reply,manager,id,index,epoch]{
        if(epoch==m_loadEpoch && index<m_installedMods.size() && m_installedMods[index].toMap().value("id").toString()==id){
            if(reply->error()!=QNetworkReply::NoError)m_installedModsError="Hash identification is unavailable.";
            else {const auto responses=QJsonDocument::fromJson(reply->readAll()).array();QVariantMap match;
                for(const auto &response:responses){const auto o=response.toObject(),basis=o.value("match").toObject(),v=o.value("matchedVpk").toObject();
                    if(basis.value("matchType")!="sha256" || basis.value("certainty").toInt()!=100)continue;
                    const auto type=v.value("submissionType").toString(),num=v.value("submissionId").toString();
                    bool valid=!num.isEmpty()&&num.size()<20;for(const auto c:num)valid=valid&&c>='0'&&c<='9';
                    if(!valid || (type!="mod"&&type!="sound"))continue;
                    const auto modId=(type=="sound"?"snd-":"")+num;
                    if(!match.isEmpty() && match.value("modId").toString()!=modId){match.clear();m_installedModsError="Ambiguous hash matches; choose the GameBanana page manually.";break;}
                    match={{"modId",modId},{"name",v.value("mod").toObject().value("name").toString(modId)},{"basis","exact file hash"},{"link",QString("https://gamebanana.com/%1/%2").arg(type=="sound"?"sounds":"mods",num)},{"requiresConfirmation",true}};
                }
                if(!match.isEmpty()){auto row=m_installedMods[index].toMap();row.insert("suggestedMatch",match);m_installedMods[index]=row;}
            }
        }
        m_installedModsBusy=false;emit installedModsChanged();manager->deleteLater();
    });return true;
}

bool LibraryModel::importInstalledMods(const QStringList &ids,const QStringList &linkIds) {
    if(ids.isEmpty() || ids.size()>64 || QSet<QString>(ids.begin(),ids.end()).size()!=ids.size())return false;
    for(const auto &id:linkIds)if(!ids.contains(id))return false;
    if(ids.size()==1)return importInstalledMod(ids.first(),linkIds.contains(ids.first()));
    if(m_installedModsBusy || (!m_gamePath.isEmpty() && m_installedScanGame!=m_gamePath) || !m_localImport->canPrepare())return false;
    QList<QUrl> roots;QVariantList rows;
    const auto clean=[](QString text,int maxBytes,bool multiline){text.removeIf([multiline](QChar c){return c.category()==QChar::Other_Control&&!(multiline&&(c==u'\n'||c==u'\t'));});while(text.toUtf8().size()>maxBytes)text.chop(1);return text.trimmed();};
    for(const auto &id:ids){
        QVariantMap row;for(const auto &v:m_installedMods)if(v.toMap().value("id").toString()==id){if(!row.isEmpty())return false;row=v.toMap();}
        if(row.isEmpty() || row.value("alreadyInLibrary").toBool())return false;
        auto paths=row.value("packages").toStringList();if(paths.isEmpty())paths.append(row.value("path").toString());QVariantList ordinals;
        for(const auto &path:paths){ordinals.append(roots.size());roots.append(QUrl::fromLocalFile(path));}
        const auto match=row.value("suggestedMatch").toMap();
        auto name=clean(match.value("name").toString(),256,false);if(name.isEmpty())name=QFileInfo(paths.first()).completeBaseName();
        QVariantMap metadata{{"name",name},{"author",clean(match.value("author").toString(),256,false)},{"category",clean(match.value("category").toString(),128,false)},{"description",clean(match.value("description").toString(),16384,true)},{"link",linkIds.contains(id)?match.value("link").toString():QString()}};
        rows.append(QVariantMap{{"id",id},{"rootOrdinals",ordinals},{"metadata",metadata},{"files",row.value("files")},{"bytes",row.value("sizeBytes")},{"problem",QVariant()}});
    }
    if(!m_localImport->prepareGroup(roots,rows))return false;
    m_localImport->setVisible(true);return true;
}
void LibraryModel::applyInstalledMetadata(const QString &id,const QVariantMap &summary) {
    bool changed=false;
    for(auto &item:m_installedMods) {
        auto row=item.toMap();auto match=row.value("suggestedMatch").toMap();
        if(match.value("modId").toString()!=id)continue;
        match.insert("name",summary.value("title"));match.insert("author",summary.value("author"));
        match.insert("category",summary.value("category"));match.insert("description",summary.value("description"));
        match.insert("thumbnail",summary.value("masked").toBool()?QVariant():summary.value("thumbnail"));
        row.insert("suggestedMatch",match);item=row;changed=true;
    }
    if(changed)emit installedModsChanged();
}
void LibraryModel::enrichInstalledMods() {
    if(m_shutdownRequested || m_switching || !m_nativeMode){m_installedMetadataTimer.stop();return;}
    if(m_installedModsBusy || !m_installedMetadataKey.isEmpty())return;
    for(const auto &item:std::as_const(m_installedMods)) {
        const auto match=item.toMap().value("suggestedMatch").toMap();const auto id=match.value("modId").toString();
        if(id.isEmpty() || match.value("link").toString().isEmpty() || m_installedMetadataAttempted.contains(id))continue;
        if(m_installedMetadataCache.contains(id)) {m_installedMetadataAttempted.insert(id);applyInstalledMetadata(id,m_installedMetadataCache.value(id));return;}
        const SubmissionKey submission{id.startsWith("snd-")?QStringLiteral("sound"):QStringLiteral("mod"),id.startsWith("snd-")?id.mid(4):id};
        const auto key=QStringLiteral("installed-")+QUuid::createUuid().toString(QUuid::Id128);
        if(!m_catalog->requestLibraryMedia(key,submission,true))return;
        m_installedMetadataKey=key;m_installedMetadataId=id;m_installedMetadataEpoch=m_loadEpoch;m_installedMetadataAttempted.insert(id);return;
    }
    m_installedMetadataTimer.stop();
}

bool LibraryModel::checkModHealth(){if(!nativeMode() || busy() || loading() || healthChecking() || m_viewedProfileId!=m_activeProfileId || !m_client->readyForRequests())return false; m_health={{"running",true},{"checked",0},{"total",0}};emit healthChanged();m_rows->setHealth({});m_client->nativeRequest(BackendClient::Call::HealthCheck,{{"profileId",m_activeProfileId}});return true;}
void LibraryModel::refreshModHealth(){if(!m_shutdownRequested && nativeMode()&&!busy()&&!loading()&&m_viewedProfileId==m_activeProfileId && m_client->readyForRequests())m_client->nativeRequest(BackendClient::Call::HealthGet,{{"profileId",m_activeProfileId}});}

bool LibraryModel::setFixEnabled(const QString &id,bool enabled){
 if(!canSetEnabled() || modDetails(id).isEmpty())return false;
 return mutate(int(BackendClient::Call::SetFixEnabled),{{"profileId",m_viewedProfileId},{"modId",id},{"enabled",enabled}},tr("Saving repair choice…"));
}
bool LibraryModel::setFixAutomatically(bool enabled){
 if(!canSetEnabled())return false;
 return mutate(int(BackendClient::Call::SetFixAutomatically),{{"enabled",enabled}},tr("Saving automatic fixes…"));
}

int LibraryModel::updateCount() const {int n=0;for(const auto &v:m_modUpdates)if(v.toMap().value("status")=="available")++n;return n;}
void LibraryModel::setUpdatesOnly(bool value){if(value==m_updatesOnly)return;m_updatesOnly=value;invalidateFilter();reconcileSelection();emit updatesChanged();}
void LibraryModel::readModUpdates(bool finish){
    if(m_updatesReadPending)return;
    m_updatesReadPending=true;m_updatesFinish=finish;
    m_client->nativeRequest(BackendClient::Call::ModUpdatesGet,{{"profileId",m_viewedProfileId},{"finish",finish}});
}
bool LibraryModel::checkForUpdates(){
    if(m_client->launchOwned() || m_launch->writeGate()) {
        m_updatesError=tr("Checking for updates will work after you close Deadlock.");emit updatesChanged();return false;
    }
    if(m_shutdownRequested || !nativeMode() || busy() || loading() || m_updatesChecking || m_updatesReadPending || m_updateBinding!=m_dataDir+":"+m_viewedProfileId+":"+m_revision || !m_client->readyForRequests())return false;
    QSet<QString> seen;
    for(const auto &v:m_modUpdates){const auto sub=SubmissionKey::decode(QJsonObject::fromVariantMap(v.toMap().value("submission").toMap()));if(sub && !seen.contains(sub->identity())){seen.insert(sub->identity());m_updateResources.enqueue({*sub,"files"});m_updateResources.enqueue({*sub,"changelog"});}}
    m_updatesOffline=false;m_updateWait.start();m_updatesChecking=true;emit updatesChanged();advanceUpdateCheck();return true;
}
void LibraryModel::advanceUpdateCheck(){
    // The update endpoint writes cache state, so defer it while launch owns
    // the library. Resume the retained check once the launch gate is released.
    if(m_client->launchOwned() || m_launch->writeGate()) {
        const auto message=tr("Checking for updates will work after you close Deadlock.");
        if(m_updatesError!=message){m_updatesError=message;emit updatesChanged();}
        return;
    }
    if(m_updatesError==tr("Checking for updates will work after you close Deadlock.")){m_updatesError.clear();emit updatesChanged();}
    if(m_shutdownRequested || !nativeMode() || busy() || loading() || !m_client->readyForRequests())return;
    const auto binding=m_dataDir+":"+m_viewedProfileId+":"+m_revision;
    if(binding!=m_updateBinding){
        if(!m_updateResourceKey.isEmpty())m_catalog->cancelLibraryMedia(m_updateResourceKey);
        m_updateResourceKey.clear();m_updateResources.clear();
        if(!m_updateBinding.startsWith(m_dataDir+":"+m_viewedProfileId+":"))cancelRemainingUpdates();
        m_updatesChecking=false;m_updatesOffline=false;m_updatesError.clear();m_modUpdates.clear();m_lastUpdateCheck={};m_updateBinding=binding;
        const bool membershipChanged=m_rows->setUpdates({});
        if(m_updatesOnly && membershipChanged)invalidateFilter();
        emit updatesChanged();readModUpdates();return;
    }
    // Bound each network resource (including time waiting for catalog admission).
    // Cancel only this check's receipt; late results cannot advance another check.
    if(m_updatesChecking && !m_updatesFinish && m_updateWait.isValid() && m_updateWait.hasExpired(12000)){
        if(!m_updateResourceKey.isEmpty())m_catalog->cancelLibraryMedia(m_updateResourceKey);
        m_updateResourceKey.clear();m_updateResources.clear();m_updatesOffline=true;
        emit updatesChanged();
    }
    if(m_updatesReadPending || !m_updateResourceKey.isEmpty())return;
    if(!m_updatesChecking){if(!m_lastUpdateCheck.isValid() || QDateTime::currentSecsSinceEpoch()-m_lastUpdateCheck.toLongLong()>=21600)readModUpdates();return;}
    if(m_updateResources.isEmpty()){readModUpdates(true);return;}
    const auto next=m_updateResources.head();const auto key="updates-"+QUuid::createUuid().toString(QUuid::Id128);
    if(m_catalog->requestLibraryMedia(key,next.first,true,next.second)){m_updateResources.dequeue();m_updateResourceKey=key;}
}
bool LibraryModel::updateAll(){
    if(!nativeMode() || !m_remote->replacementAvailable() || busy() || loading() || m_remote->replacementActive() || updatingCount()
        || m_updateBinding!=m_dataDir+":"+m_viewedProfileId+":"+m_revision)return false;
    m_updateBatchTargets.clear();m_updateFiles.clear();m_updateDigests.clear();m_updateBatchConfirmed=false;m_updateNeedsAttention=false;
    for(const auto &v:m_modUpdates){const auto r=v.toMap();if(r.value("status")=="available"){
        const auto id=r.value("modId").toString();m_updateReviews.enqueue(id);m_updateFiles.insert(id,r.value("fileId").toString());m_updateDigests.insert(id,r.value("recordDigest").toString());
        m_updateBatchTargets.append(QVariantMap{{"id",id},{"name",modDetails(id).value("title").toString()},{"fileId",r.value("fileId").toString()}});
    }}
    emit updatesChanged();advanceUpdateReviews();return updatingCount()>0;
}
void LibraryModel::confirmBatchUpdates(){
    if(!batchUpdateReview())return;m_updateBatchConfirmed=true;emit updatesChanged();advanceUpdateReviews();
}
void LibraryModel::cancelRemainingUpdates(){
    const bool closeReview=m_updateReviewActive && m_remote->replacementActive();
    m_updateReviews.clear();m_updateReviewActive=false;m_updateBatchTargets.clear();m_updateFiles.clear();m_updateDigests.clear();m_updateActiveId.clear();m_updateActiveJob.clear();m_updateBatchConfirmed=false;m_updateNeedsAttention=false;emit updatesChanged();
    if(closeReview)m_remote->close();
}
void LibraryModel::advanceUpdateReviews(){
    if(m_shutdownRequested || !m_updateBinding.startsWith(m_dataDir+":"+m_viewedProfileId+":"))return;
    if(m_updateReviewActive){
        if(!m_updateBatchConfirmed)return;
        if(m_remote->replacementActive()) {
            if(m_updateNeedsAttention)return;
            if(!m_remote->error().isEmpty()){m_updateNeedsAttention=true;m_remote->m_visible=true;emit m_remote->changed();return;}
            if(!m_remote->replacementTargetCurrent())return;
            const auto reviewed=m_updateDigests.value(m_updateActiveId);
            if(reviewed.isEmpty() || reviewed!=m_remote->m_replaceTarget.value("recordDigest").toString()) {
                m_updateNeedsAttention=true;m_remote->m_visible=true;
                m_remote->m_error=reviewed.isEmpty()?tr("The reviewed mod could not be confirmed. Review its files again."):tr("This mod changed since the batch review. Review its files again.");
                emit m_remote->changed();return;
            }
            if(m_remote->m_collectFiles || m_remote->m_selectionToken.isEmpty() || !m_remote->m_filePageEvidence.contains(m_remote->m_filePage) || m_remote->skipPending())return;
            const auto id=m_updateFiles.value(m_updateActiveId);QVariantMap file;
            for(const auto &v:m_remote->m_files){const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==id){file=row;break;}}
            if(file.isEmpty() && m_remote->m_fileHasMore){
                if(!m_remote->loadFiles(m_remote->m_filePage+1)){m_updateNeedsAttention=true;m_remote->m_visible=true;emit m_remote->changed();}
                return;
            }
            const auto skip=m_remote->m_replaceSkip;const auto entry=skip.value("entry").toMap();
            const bool skipped=skip.value("state")=="present" && entry.value("fileId").toString()==id && entry.value("candidateFingerprint")==file.value("candidateFingerprint");
            if(file.isEmpty() || file.value("list")=="archived" || file.value("archiveFlag")!=false || skip.value("state")=="unavailable" || skipped){m_updateNeedsAttention=true;m_remote->m_visible=true;emit m_remote->changed();return;}
            bool started=false;
            if(m_remote->replacementFileSelection()) {m_remote->pickFile(id,true,false);started=m_remote->startSelectedFiles();}
            else started=m_remote->startReplacement(id,file.value("candidateFingerprint").toString(),"update",false);
            if(started)m_updateActiveJob=m_remote->m_jobId;
            else {m_updateNeedsAttention=true;m_remote->m_visible=true;emit m_remote->changed();}
            return;
        }
        if(m_updateActiveJob.isEmpty())m_updateActiveJob=m_remote->m_jobId;
        if(m_updateActiveJob.isEmpty())return;
        if(!m_remote->m_installStages.contains(m_updateActiveJob)){
            if(!m_remote->m_pendingStarts.contains(m_updateActiveJob))cancelRemainingUpdates();
            return;
        }
        const auto stage=m_remote->m_installStages.value(m_updateActiveJob);
        if(stage=="stopped"){cancelRemainingUpdates();return;}
        if(stage!="done")return;
        m_updateReviewActive=false;m_updateActiveId.clear();m_updateActiveJob.clear();m_updateNeedsAttention=false;emit updatesChanged();
    }
    if(m_updateReviews.isEmpty()){if(m_updateBatchConfirmed)cancelRemainingUpdates();return;}
    if(busy() || loading() || m_autoCycle || autoApplyWorkflowBlocked() || m_remote->replacementActive())return;
    if(m_remote->beginBatchReplacement(m_updateReviews.head())){m_updateActiveId=m_updateReviews.dequeue();m_updateReviewActive=true;emit updatesChanged();}
}

void LibraryModel::detectNextHero() {
    if(busy() || loading() || m_heroPending || m_heroQueue.isEmpty() || !m_nativeMode || m_heroRevision!=m_revision || !m_client->readyForRequests())return;
    m_heroPending=true;
    m_client->nativeRequest(BackendClient::Call::HeroDetect,{{"version",1},{"profileId",m_viewedProfileId},{"modId",m_heroQueue.takeFirst()},{"expectedRevision",m_revision}});
}
