#pragma once

#include <QSortFilterProxyModel>
#include <QJsonObject>
#include <QQueue>
#include <QElapsedTimer>
#include <functional>
#include "localimportcontroller.h"
#include "fontcontroller.h"
#include "performancecontroller.h"
#include "gameconfigcontroller.h"
#include "optioncontroller.h"
#include "launchcontroller.h"
#include "catalogcontroller.h"
#include "remotecontroller.h"
#include <QtQml/qqmlregistration.h>

class BackendClient;
class LibraryRows;
class LibraryMediaController;
struct LibrarySnapshot;

class LibraryModel : public QSortFilterProxyModel {
    Q_OBJECT
    QML_ELEMENT
    Q_PROPERTY(QString detectedGamePath READ detectedGamePath NOTIFY installedModsChanged)
    Q_PROPERTY(QStringList detectedGamePaths READ detectedGamePaths NOTIFY installedModsChanged)
    Q_PROPERTY(QVariantList installedMods READ installedMods NOTIFY installedModsChanged)
    Q_PROPERTY(bool installedModsBusy READ installedModsBusy NOTIFY installedModsChanged)
    Q_PROPERTY(QString installedModsError READ installedModsError NOTIFY installedModsChanged)
    Q_PROPERTY(bool canScanInstalledMods READ canScanInstalledMods NOTIFY nativeChanged)
    Q_PROPERTY(CatalogController* catalog READ catalog CONSTANT)
    Q_PROPERTY(RemoteController* remote READ remote CONSTANT)
    Q_PROPERTY(qreal viewScrollPosition READ viewScrollPosition WRITE setViewScrollPosition NOTIFY viewContextChanged)
    Q_PROPERTY(QString viewFocus READ viewFocus WRITE setViewFocus NOTIFY viewContextChanged)
    Q_PROPERTY(LaunchController* launch READ launch CONSTANT)
    Q_PROPERTY(bool closeBlocked READ closeBlocked NOTIFY shutdownChanged)
    Q_PROPERTY(bool shutdownReady READ shutdownReady NOTIFY shutdownChanged)
    Q_PROPERTY(LocalImportController* localImport READ localImport CONSTANT)
    Q_PROPERTY(GameConfigController* autoexec READ autoexec CONSTANT)
    Q_PROPERTY(GameConfigController* crosshair READ crosshair CONSTANT)
    Q_PROPERTY(PerformanceController* performance READ performance CONSTANT)
    Q_PROPERTY(FontController* fonts READ fonts CONSTANT)
    Q_PROPERTY(OptionController* options READ options CONSTANT)
    Q_PROPERTY(QString query READ query WRITE setQuery NOTIFY queryChanged)
    Q_PROPERTY(QString category READ category WRITE setCategory NOTIFY categoryChanged)
    Q_PROPERTY(QString heroFilter READ heroFilter WRITE setHeroFilter NOTIFY heroFilterChanged)
    Q_PROPERTY(QString typeFilter READ typeFilter WRITE setTypeFilter NOTIFY typeFilterChanged)
    Q_PROPERTY(QStringList heroes READ heroes NOTIFY categoriesChanged)
    Q_PROPERTY(QStringList types READ types NOTIFY categoriesChanged)
    Q_PROPERTY(bool needsAttentionOnly READ needsAttentionOnly WRITE setNeedsAttentionOnly NOTIFY healthChanged)
    Q_PROPERTY(bool enabledOnly READ enabledOnly WRITE setEnabledOnly NOTIFY enabledOnlyChanged)
    Q_PROPERTY(int count READ count NOTIFY countChanged)
    Q_PROPERTY(QVariantList recentMods READ recentMods NOTIFY recentModsChanged)
    Q_PROPERTY(QVariantList enabledMods READ enabledMods NOTIFY enabledModsChanged)
    Q_PROPERTY(int totalCount READ totalCount NOTIFY totalCountChanged)
    Q_PROPERTY(int enabledCount READ enabledCount NOTIFY enabledCountChanged)
    Q_PROPERTY(QString selectedId READ selectedId WRITE setSelectedId NOTIFY selectionChanged)
    Q_PROPERTY(QVariantMap selectedMod READ selectedMod NOTIFY selectionChanged)
    Q_PROPERTY(bool readOnly READ readOnly NOTIFY modeChanged)
    Q_PROPERTY(bool demoMode READ demoMode NOTIFY modeChanged)
    Q_PROPERTY(bool loading READ loading NOTIFY loadingChanged)
    Q_PROPERTY(QString error READ error NOTIFY errorChanged)
    Q_PROPERTY(QString sourcePath READ sourcePath NOTIFY sourceChanged)
    Q_PROPERTY(QStringList warnings READ warnings NOTIFY sourceChanged)
    Q_PROPERTY(QStringList warningCodes READ warningCodes NOTIFY sourceChanged)
    Q_PROPERTY(QVariantList profiles READ profiles NOTIFY sourceChanged)
    Q_PROPERTY(QString activeProfileId READ activeProfileId NOTIFY sourceChanged)
    Q_PROPERTY(QString viewedProfileId READ viewedProfileId NOTIFY sourceChanged)
    Q_PROPERTY(QStringList categories READ categories NOTIFY categoriesChanged)
    Q_PROPERTY(bool nativeMode READ nativeMode NOTIFY nativeChanged)
    Q_PROPERTY(QString revision READ revision NOTIFY nativeChanged)
    Q_PROPERTY(QString gamePath READ gamePath NOTIFY nativeChanged)
    Q_PROPERTY(QString dataDir READ dataDir NOTIFY nativeChanged)
    Q_PROPERTY(QString appliedProfileId READ appliedProfileId NOTIFY nativeChanged)
    Q_PROPERTY(bool pendingChanges READ pendingChanges NOTIFY nativeChanged)
    Q_PROPERTY(QString pendingStatus READ pendingStatus NOTIFY nativeChanged)
    Q_PROPERTY(QString pendingReason READ pendingReason NOTIFY nativeChanged)
    Q_PROPERTY(bool canRestore READ canRestore NOTIFY nativeChanged)
    Q_PROPERTY(bool externalGameinfoChange READ externalGameinfoChange NOTIFY nativeChanged)
    Q_PROPERTY(QString externalGameinfoMessage READ externalGameinfoMessage NOTIFY nativeChanged)
    Q_PROPERTY(bool recoveryRequired READ recoveryRequired NOTIFY nativeChanged)
    Q_PROPERTY(bool recoveryNeedsAttention READ recoveryNeedsAttention NOTIFY nativeChanged)
    Q_PROPERTY(bool outcomeUnknown READ outcomeUnknown NOTIFY nativeChanged)
    Q_PROPERTY(bool previewBusy READ previewBusy NOTIFY nativeChanged)
    Q_PROPERTY(bool busy READ busy NOTIFY nativeChanged)
    Q_PROPERTY(bool writeBusy READ writeBusy NOTIFY nativeChanged)
    Q_PROPERTY(QString status READ status NOTIFY nativeChanged)
    Q_PROPERTY(QVariantMap importReview READ importReview NOTIFY reviewChanged)
    Q_PROPERTY(QVariantMap applyReview READ applyReview NOTIFY reviewChanged)
    Q_PROPERTY(bool autoApplyWorkflowOpen READ autoApplyWorkflowOpen WRITE setAutoApplyWorkflowOpen NOTIFY autoApplyWorkflowOpenChanged)
    Q_PROPERTY(bool canImport READ canImport NOTIFY nativeChanged)
    Q_PROPERTY(QString bootstrapState READ bootstrapState NOTIFY nativeChanged)
    Q_PROPERTY(bool canCreateLibrary READ canCreateLibrary NOTIFY nativeChanged)
    Q_PROPERTY(QString bootstrapReason READ bootstrapReason NOTIFY nativeChanged)
    Q_PROPERTY(bool canEditProfiles READ canEditProfiles NOTIFY nativeChanged)
    Q_PROPERTY(bool canBrowseProfiles READ canBrowseProfiles NOTIFY nativeChanged)
    Q_PROPERTY(bool canDeleteMods READ canDeleteMods NOTIFY nativeChanged)
    Q_PROPERTY(bool canBulkOperate READ canBulkOperate NOTIFY nativeChanged)
    Q_PROPERTY(QVariantMap lastBulkResult READ lastBulkResult NOTIFY lastBulkResultChanged)
    Q_PROPERTY(bool canSetEnabled READ canSetEnabled NOTIFY nativeChanged)
    Q_PROPERTY(bool canToggleEnabled READ canToggleEnabled NOTIFY nativeChanged)
    Q_PROPERTY(bool canBulkToggle READ canBulkToggle NOTIFY nativeChanged)
    Q_PROPERTY(bool automaticApply READ automaticApply WRITE setAutomaticApply NOTIFY nativeChanged)
    Q_PROPERTY(bool canPreviewApply READ canPreviewApply NOTIFY nativeChanged)
    Q_PROPERTY(bool canRecover READ canRecover NOTIFY nativeChanged)
    Q_PROPERTY(QString actionUnavailableReason READ actionUnavailableReason NOTIFY nativeChanged)
    Q_PROPERTY(bool helperStopped READ helperStopped NOTIFY nativeChanged)
    Q_PROPERTY(QString stopReason READ stopReason NOTIFY nativeChanged)
    Q_PROPERTY(bool canRefresh READ canRefresh NOTIFY nativeChanged)
public:
    explicit LibraryModel(QObject *parent = nullptr);
    QString detectedGamePath() const { return m_detectedGamePath; }
    QStringList detectedGamePaths() const { return m_detectedGamePaths; }
    QVariantList installedMods() const { return m_installedMods; }
    bool installedModsBusy() const { return m_installedModsBusy; }
    QString installedModsError() const { return m_installedModsError; }
    bool canScanInstalledMods() const;
    Q_INVOKABLE bool detectGame();
    Q_INVOKABLE bool scanInstalledMods(const QString &gamePath = {});
    Q_INVOKABLE bool identifyInstalledMod(const QString &id);
    Q_INVOKABLE bool importInstalledMods(const QStringList &ids, const QStringList &linkIds = {});
    Q_INVOKABLE bool importInstalledMod(const QString &id, bool confirmSuggestedLink = false);
    CatalogController *catalog() const { return m_catalog; }
    RemoteController *remote() const { return m_remote; }
    qreal viewScrollPosition() const { return m_viewScrollPosition; }
    void setViewScrollPosition(qreal value) { m_viewScrollPosition=value; emit viewContextChanged(); }
    QString viewFocus() const { return m_viewFocus; }
    void setViewFocus(const QString &value) { m_viewFocus=value; emit viewContextChanged(); }
    LaunchController *launch() const { return m_launch; }
    bool closeBlocked() const;
    bool shuttingDown() const { return m_shutdownRequested; }
    bool shutdownReady() const { return m_shutdownReady; }
    Q_INVOKABLE void requestShutdown();
    LocalImportController *localImport() const { return m_localImport; }
    GameConfigController *autoexec() const {return m_autoexec;}
    GameConfigController *crosshair() const {return m_crosshair;}
    PerformanceController *performance() const { return m_performance; }
    FontController *fonts() const { return m_fonts; }
    OptionController *options() const { return m_options; }
    QVariantList fullProfileOrder() const { return m_fullProfileOrder; }
    bool profileEditsAvailable() const;
    bool dispatchProfileEditPreview(const QString &root,const QString &profile,const QString &revision,const QJsonObject &change);
    bool dispatchProfileEditCommit(const QString &root,const QString &profile,const QString &revision,const QString &token);
    bool dispatchOptionsAttach(BackendClient *client,const QString &profile,const QString &revision,QJsonObject params,const QString &modId);
    QString helperPath() const { return m_helperPath; }
    bool helperReady() const { return !m_dataDir.isEmpty() && m_client->readyForRequests() && !m_client->retiring() && m_client->nativeRoot()==m_dataDir; }
    quint64 loadEpoch() const { return m_loadEpoch; }
    bool dispatchLocalCommit(BackendClient *client, const QString &profile, const QString &revision, QJsonObject params, const QString &modId = {});
    bool remoteAdmissionAvailable() const;
    bool remoteMarkerPending() const;
    bool replacementMarkerPending() const;
    bool dispatchRemoteCommit(const QString &job,const QString &profile,const QString &revision,const QString &token,const QString &mod,
                              const QString &consent,const QString &cohort,const QString &selection);
    bool dispatchReplacementCommit(const QVariantMap &job,const QVariantMap &review,const QString &consent,const QString &revision);
    bool fontsAvailable() const;
    bool dispatchFontsPreview(const QString &root, const QString &game, const QString &revision, const QJsonObject &change);
    bool dispatchFontsCommit(const QString &root, const QString &game, const QString &revision, const QString &modId, const QString &token);
    bool nativeMode() const { return m_nativeMode; }
    QString revision() const { return m_revision; }
    QString gamePath() const { return m_gamePath; }
    QString dataDir() const { return m_dataDir; }
    QString appliedProfileId() const { return m_appliedProfileId; }
    bool pendingChanges() const { return m_pendingChanges; }
    QString pendingStatus() const {return m_pendingStatus;}
    QString pendingReason() const {return m_pendingReason;}
    bool canRestore() const;
    bool externalGameinfoChange() const { return !m_recoveryRequired && m_pendingReason == QStringLiteral("externalGameinfoChange"); }
    QString externalGameinfoMessage() const;
    bool recoveryRequired() const { return m_recoveryRequired; }
    bool recoveryNeedsAttention() const {
        return m_recoveryRequired && !(m_launchOnlyRecovery
            && (m_launch->phase() == "running" || m_launch->phase() == "waitingForGame"));
    }
    bool outcomeUnknown() const { return m_outcomeUnknown; }
    bool awaitingEditSnapshot() const { return m_editLost; }
    bool previewBusy() const { return m_busy && m_previewBusy; }
    bool busy() const { return m_busy || m_bootstrapMarkerFileBusy; }
    bool writeBusy() const { return m_writeBusy || m_bootstrapMarkerFileBusy; }
    QString status() const { return m_status; }
    QVariantMap importReview() const { return m_importReview; }
    QVariantMap applyReview() const { return m_applyReview; }
    bool autoApplyWorkflowOpen() const { return m_autoApplyWorkflowOpen; }
    void setAutoApplyWorkflowOpen(bool open);
    bool canImport() const;
    QString bootstrapState() const;
    bool canCreateLibrary() const;
    QString bootstrapReason() const;
    bool canEditProfiles() const;
    bool canBrowseProfiles() const;
    bool canSetEnabled() const;
    bool canToggleEnabled() const;
    bool canBulkToggle() const;
    bool automaticApply() const { return m_automaticApply; }
    void setAutomaticApply(bool value);
    bool canBulkOperate() const;
    bool canDeleteMods() const;
    QVariantMap lastBulkResult() const { return m_lastBulkResult; }
    Q_PROPERTY(QVariantMap bisect READ bisect NOTIFY nativeChanged)
    Q_PROPERTY(bool canStartBisect READ canStartBisect NOTIFY nativeChanged)
    Q_PROPERTY(bool fixAutomatically READ fixAutomatically NOTIFY healthChanged)
    bool fixAutomatically() const { return m_fixAutomatically; }
    Q_INVOKABLE bool setFixEnabled(const QString &id, bool enabled);
    Q_INVOKABLE bool setFixAutomatically(bool enabled);
    Q_PROPERTY(int updateCount READ updateCount NOTIFY updatesChanged)
    Q_PROPERTY(QVariant lastUpdateCheck READ lastUpdateCheck NOTIFY updatesChanged)
    Q_PROPERTY(int updatingCount READ updatingCount NOTIFY updatesChanged)
    Q_PROPERTY(bool batchUpdateReview READ batchUpdateReview NOTIFY updatesChanged)
    Q_PROPERTY(QVariantList batchUpdateTargets READ batchUpdateTargets NOTIFY updatesChanged)
    Q_PROPERTY(bool updatesChecking READ updatesChecking NOTIFY updatesChanged)
    Q_PROPERTY(bool updatesOffline READ updatesOffline NOTIFY updatesChanged)
    Q_PROPERTY(QString updatesError READ updatesError NOTIFY updatesChanged)
    Q_PROPERTY(bool updatesOnly READ updatesOnly WRITE setUpdatesOnly NOTIFY updatesChanged)
    int updateCount() const;
    QVariant lastUpdateCheck() const { return m_lastUpdateCheck; }
    int updatingCount() const { return m_updateReviews.size() + (m_updateReviewActive ? 1 : 0); }
    bool batchUpdateReview() const { return updatingCount()>0 && !m_updateBatchConfirmed; }
    QVariantList batchUpdateTargets() const { return m_updateBatchTargets; }
    bool updatesChecking() const { return m_updatesChecking; }
    bool updatesOffline() const { return m_updatesOffline; }
    QString updatesError() const { return m_updatesError; }
    bool updatesOnly() const { return m_updatesOnly; }
    void setUpdatesOnly(bool value);
    Q_INVOKABLE bool checkForUpdates();
    Q_INVOKABLE bool updateAll();
    Q_INVOKABLE void confirmBatchUpdates();
    Q_INVOKABLE void cancelRemainingUpdates();
    Q_SIGNAL void updatesChanged();
    Q_PROPERTY(QVariantMap health READ health NOTIFY healthChanged)
    Q_PROPERTY(bool healthChecking READ healthChecking NOTIFY healthChanged)
    QVariantMap health() const { return m_health; }
    bool healthChecking() const { return m_health.value("running").toBool(); }
    Q_INVOKABLE bool checkModHealth();
    Q_INVOKABLE void refreshModHealth();
    Q_SIGNAL void healthChanged();
    QVariantMap launchFingerprint() const { return m_launchFingerprint; }
    QVariantMap bisect() const { return m_bisect; }
    bool canStartBisect() const;
    Q_INVOKABLE bool startBisect(const QStringList &modIds = {});
    Q_INVOKABLE bool answerBisect(const QString &answer);
    Q_INVOKABLE bool finishBisect(bool keepDisabled = false);
    Q_INVOKABLE bool setModsEnabled(const QStringList &ids, bool enabled);
    Q_INVOKABLE bool deleteMods(const QStringList &ids);
    Q_INVOKABLE bool removeModsFromProfile(const QStringList &ids);
    bool canPreviewApply() const;
    bool canRecover() const;
    QString actionUnavailableReason() const;
    bool helperStopped() const;
    QString stopReason() const;
    bool canRefresh() const;
    QString query() const { return m_query; }
    QString category() const { return m_category; }
    QString heroFilter() const { return m_heroFilter; }
    QString typeFilter() const { return m_typeFilter; }
    QStringList heroes() const;
    QStringList types() const;
    void setHeroFilter(const QString &value);
    void setTypeFilter(const QString &value);
    bool needsAttentionOnly() const { return m_needsAttentionOnly; }
    void setNeedsAttentionOnly(bool value);
    bool enabledOnly() const { return m_enabledOnly; }
    int count() const { return rowCount(); }
    QVariantList recentMods() const;
    QVariantList enabledMods() const;
    int totalCount() const;
    int enabledCount() const;
    QString selectedId() const { return m_selectedId; }
    void setSelectedId(const QString &id);
    QVariantMap selectedMod() const { return modDetails(m_selectedId); }
    Q_INVOKABLE void setLibraryMediaActive(bool active);
    Q_INVOKABLE void demandModMedia(const QString &consumer,const QString &modId,bool wanted);
    bool readOnly() const;
    bool demoMode() const { return m_demoMode; }
    bool loading() const { return m_loading; }
    QString error() const { return m_error; }
    QString sourcePath() const { return m_sourcePath; }
    QStringList warnings() const { return m_warnings; }
    QStringList warningCodes() const { return m_warningCodes; }
    QVariantList profiles() const { return m_profiles; }
    QString activeProfileId() const { return m_activeProfileId; }
    QString viewedProfileId() const { return m_viewedProfileId; }
    QStringList categories() const;
    void setQuery(const QString &value);
    void setCategory(const QString &value);
    void setEnabledOnly(bool value);
    void loadSource(const QString &helperPath, const QString &statePath = {}, const QString &dataDir = {});
    Q_INVOKABLE void loadNative(const QString &helperPath, const QString &dataDir);
    Q_INVOKABLE int rowForId(const QString &id) const;
    Q_INVOKABLE QString localPath(const QUrl &url) const;
    Q_INVOKABLE bool previewImport(const QString &statePath, const QString &gamePath);
    Q_INVOKABLE bool commitImport();
    Q_INVOKABLE bool inspectInitialization();
    Q_INVOKABLE bool createLibrary();
    Q_INVOKABLE bool cancelPreview();
    Q_INVOKABLE void discardImportReview();
    Q_INVOKABLE bool createProfile(const QString &name);
    Q_INVOKABLE bool copyProfile(const QString &id, const QString &name);
    Q_INVOKABLE bool renameProfile(const QString &id, const QString &name);
    Q_INVOKABLE bool removeProfile(const QString &id);
    Q_INVOKABLE bool previewRestore();
    Q_INVOKABLE bool useGameFolder(const QString &path);
    Q_INVOKABLE bool previewApply(const QString &gamePath);
    Q_INVOKABLE bool commitApply();
    Q_INVOKABLE void discardApplyReview();
    Q_INVOKABLE bool restorePrevious();
    Q_INVOKABLE bool reconcileOperation();
    Q_INVOKABLE bool recover();
    void loadDemoData(int rows);
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void viewProfile(const QString &id);
    Q_INVOKABLE QVariantMap modDetails(const QString &id) const;
    Q_INVOKABLE bool setModEnabled(const QString &id, bool enabled);
    Q_INVOKABLE QString idAt(int row) const;
signals:
    void installedModsChanged();
    void lastBulkResultChanged();
    void viewContextChanged();
    void nativeChanged();
    void shutdownChanged();
    void reviewChanged();
    void autoApplyWorkflowOpenChanged();
    void automaticReviewRequired();
    void queryChanged();
    void categoryChanged();
    void heroFilterChanged();
    void typeFilterChanged();
    void enabledOnlyChanged();
    void countChanged();
    void totalCountChanged();
    void enabledCountChanged();
    void recentModsChanged();
    void enabledModsChanged();
    void categoriesChanged();
    void selectionChanged();
    void libraryMediaCapacityChanged();
    void modeChanged();
    void loadingChanged();
    void errorChanged();
    void sourceChanged();
protected:
    bool filterAcceptsRow(int sourceRow, const QModelIndex &parent) const override;
private:
    QStringList m_heroQueue;
    bool m_heroPending = false;
    QString m_heroRevision;
    void detectNextHero();
    friend class ModelCheck;
    void enrichInstalledMods();
    void applyInstalledMetadata(const QString &, const QVariantMap &);
    QTimer m_installedMetadataTimer;
    QHash<QString,QVariantMap> m_installedMetadataCache;
    QSet<QString> m_installedMetadataAttempted;
    QString m_installedMetadataKey, m_installedMetadataId;
    quint64 m_installedMetadataEpoch=0;
    bool installedRead(const QString &method, const QJsonObject &params);
    QString m_detectedGamePath, m_installedModsError, m_installedScanGame;
    QStringList m_detectedGamePaths;
    QVariantList m_installedMods;
    bool m_installedModsBusy = false;

    QVariantMap m_lastBulkResult;
    QTimer m_autoApplyTimer;
    QMap<QString, bool> m_autoToggles;
    QMap<QString, bool> m_autoInFlight;
    QSet<QString> m_autoBulkIds, m_autoInFlightBulk;
    QString m_autoProfile, m_autoRoot;
    QVariantMap m_autoReview;
    QString m_autoReviewRevision;
    bool m_performanceApplyNeedsReview = true;
    bool m_automaticApply = false, m_autoCycle = false, m_autoPreview = false;
    bool m_autoDue = false, m_autoForceReview = false, m_autoBlocked = false;
    bool m_autoDispatch = false;
    bool m_autoApplyWorkflowOpen = false;
    bool m_closeAfterToggles = false;
    bool m_editApplyNeedsReview = true, m_replacementApplyNeedsReview = true;
    void scheduleAutomaticApply(bool forceReview);
    bool queueAutoToggles(const QStringList &ids, bool enabled, bool bulk = false);
    void advanceAutoApply();
    bool autoApplyWorkflowBlocked() const;
    bool bulkMutation(const QStringList &ids, bool remove, bool enabled);
    bool profileBrowseAllowed() const;
    friend class PerformanceController;
    friend class GameConfigController;
    friend class FontController;
    friend class LaunchController;
    friend class RemoteController;
    friend class StorageController;
    void reconcileRemote();
    void remoteOperation(const QVariantMap &outcome);
    void remoteReplay(BackendClient::Call call,const QJsonObject &request,const LibrarySnapshot &snapshot);
    void remoteReplayRejected(BackendClient::Call call,const QJsonObject &request,const QString &kind,const QString &message);
    void remoteStatus(const QVariantMap &status);
    void remoteTarget(const QJsonObject &request,const QVariantMap &result);
    void remoteTargetRejected(const QJsonObject &request,const QString &kind,const QString &message);
    void finishRemoteOutcome(bool success);
    void remoteRecoveryResult(const LibrarySnapshot &snapshot,const QString &snapshotDigest);
    void remoteRecoverySnapshot(const LibrarySnapshot &snapshot);
    void recoveryInspection(const QJsonObject &request,const QVariantMap &result);
    void saveRecoveryChoice(const QString &operationId,const QString &revision);
    bool recoveryWrapperPending() const;
    bool replacementRecoveryContext() const;
    void recoveryWrapperResult(const LibrarySnapshot &snapshot,const QString &snapshotDigest);
    void recoveryWrapperOperation(const QVariantMap &outcome);
    void recoveryWrapperTerminal();
    void recoveryWrapperView();
    void finishRecoveryWrapper(bool recoveryStillRequired,bool cleanUnimported,quint64 errorGeneration);
    bool remoteRecover();
    void scheduleRemoteReconcile(int delay=0);
    void advanceShutdown();
    void advanceLoad();
    void performDemoData(int rows);
    void applySnapshot(const LibrarySnapshot &snapshot);
    void beginLoad(const QString &helperPath, const QString &statePath, const QString &dataDir, bool nativeFirst);
    void refreshKeepingError(bool preserve);
    void checkOriginalAfterRecovery();
    void setBusy(bool value, bool write = false, const QString &status = {});
    bool reserveBootstrapMarkerWork();
    void persistMarker(const QVariantMap &marker, const std::function<void()> &then, const std::function<void()> &failure = {}, bool bootstrapFileWork = false);
    void clearMarker(const std::function<void()> &then = {});
    bool bootstrapPending() const;
    void dispatchBootstrap(quint64 session,quint64 errorGeneration);
    void confirmBootstrap(bool created);
    bool mutate(int call, QJsonObject params, const QString &status);
    bool profileAction(const QString &action, const QString &id, const QString &name);
    QString markerPath() const;
    void invalidateReviews();
    void reconcileSelection();
    void setLoading(bool loading);
    // Shared "no write is in flight and none is blocked" core of the action
    // predicates. Each predicate adds its own terms on top of this.
    bool writesIdle() const;
    quint64 setError(const QString &error);
    bool setErrorIfCurrent(quint64 generation,const QString &error);
    void clearErrorIfCurrent(quint64 generation);
    void notifyRowsChanged();
    void refreshHomeRecent();
    void refreshHomeEnabled();
    void clearSource();
    void resetLibraryMediaContext();
    void updateLibraryMediaContext();
    void updateSelectedMediaDemand();
    void readModUpdates(bool finish = false);
    void advanceUpdateCheck();
    void advanceUpdateReviews();
    QVariantList m_modUpdates;
    QVariant m_lastUpdateCheck;
    QTimer m_updatesTimer;
    QElapsedTimer m_updateWait;
    bool m_updatesOffline = false;
    QString m_updatesError;
    QQueue<QPair<SubmissionKey,QString>> m_updateResources;
    QString m_updateResourceKey, m_updateBinding;
    QQueue<QString> m_updateReviews;
    QVariantList m_updateBatchTargets;
    QHash<QString,QString> m_updateFiles, m_updateDigests;
    QString m_updateActiveId, m_updateActiveJob;
    bool m_updateBatchConfirmed=false, m_updateNeedsAttention=false;
    bool m_updateReviewActive=false, m_updatesChecking=false, m_updatesOnly=false, m_updatesReadPending=false, m_updatesFinish=false;
    QVariantMap m_bisect, m_launchFingerprint;
    bool m_fixAutomatically = true;
    bool m_needsAttentionOnly = false;
    QTimer m_healthRefreshTimer;
    QVariantMap m_health;
    QTimer m_healthTimer;
    LibraryRows *m_rows;
    QVariantList m_homeRecent, m_homeEnabled;
    BackendClient *m_client;
    CatalogController *m_catalog;
    LibraryMediaController *m_media;
    RemoteController *m_remote;
    qreal m_viewScrollPosition=0;
    QString m_viewFocus="search";
    LocalImportController *m_localImport;
    LaunchController *m_launch;
    PerformanceController *m_performance;
    GameConfigController *m_autoexec,*m_crosshair;
    FontController *m_fonts;
    OptionController *m_options;
    QVariantList m_fullProfileOrder;
    QHash<QString,QSet<QString>> m_fontOwnerIds;
    QHash<QString,QVariantList> m_fontOwners;
    QVariantList m_fontOwnerRows;
    bool m_canUndoFonts = false;
    enum class ConfirmationTarget { None, Import, Fonts, OptionsAttach, ProfileEdit, Apply };
    ConfirmationTarget m_confirmationTarget = ConfirmationTarget::None;
    bool dispatchPreparationCommit(BackendClient *client,const QString &profile,const QString &revision,QJsonObject params,const QString &modId,BackendClient::Call call);
    bool m_localConfirmation = false;
    QString m_localConfirmationError, m_requestedProfileId;
    quint64 m_localConfirmationErrorGeneration = 0;
    bool m_localConfirmationSuccess = false;
    void confirmLocalView(bool success, const QString &error = {});
    QString m_query, m_category, m_heroFilter, m_typeFilter, m_selectedId;
    QString m_selectedMediaRow;
    SubmissionKey m_selectedMediaSubmission;
    bool m_libraryMediaActive=false;
    quint64 m_errorGeneration = 0, m_preservedErrorGeneration = 0;
    QString m_helperPath, m_statePath, m_sourcePath, m_activeProfileId, m_viewedProfileId, m_error;
    QStringList m_warnings, m_warningCodes;
    QVariantList m_profiles;
    QString m_pendingStatus,m_pendingReason;
    QString m_dataDir, m_revision = "0", m_gamePath, m_appliedProfileId, m_status, m_preservedError, m_initialProfile;
    QVariantMap m_importReview, m_applyReview, m_marker;
    QString m_inspectState = "unavailable", m_inspectReason, m_inspectRoot;
    bool m_inspectEligible = false, m_bootstrapMarkerDurable = false, m_bootstrapMarkerFileBusy = false;
    bool m_bootstrapConfirming = false, m_bootstrapCreated = false, m_bootstrapRetry = false;
    int m_otherMarkerFileTasks = 0;
    quint64 m_loadEpoch = 0, m_inspectSession = 0;
    bool m_invalidMarker = false;
    bool m_launchOnlyRecovery=false;
    bool m_shutdownRequested=false, m_shutdownReady=false, m_restartAfterDrain=false;
    struct DeferredLoad {QString helper,state,root;bool nativeFirst=false,demo=false;int rows=0;};
    QQueue<DeferredLoad> m_deferredLoads;
    bool m_switching=false;
    enum class RemoteStage { Idle, Operation, Replay, Status, Snapshot, Target, Inspect, RecoverySnapshot, Recovery, RecoveryOperation, RecoveryReplay, OriginalOperation, RecoveryView, LegacyView };
    RemoteStage m_remoteStage=RemoteStage::Idle;
    QTimer m_remoteRetry;
    QVariantMap m_remoteOutcome;
    QString m_remoteToken;
    QString m_remoteConfirmationRevision;
    QString m_recoveryViewProfile;
    bool m_remoteMarkerDurable=false, m_remoteRecoveryTerminal=false;
    bool m_haveRecoveryContext = false, m_recoveryBarrier = false, m_previewBusy = false;
    bool m_nativeMode = false, m_pendingChanges = false, m_canRestore = false, m_recoveryRequired = false;
    bool m_busy = false, m_writeBusy = false, m_outcomeUnknown = false, m_defaultMode = false, m_reconciling = false;
    // An edit is one database commit in the helper and has no marker. A lost
    // reply is settled by the next snapshot, which is the helper's saved state.
    bool m_editInFlight = false, m_editLost = false;
    bool m_enabledOnly = false, m_demoMode = false, m_loading = false, m_refreshFallback = false, m_profileFallbackWarning = false;
};
