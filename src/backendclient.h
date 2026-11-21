#pragma once

#include "libraryrows.h"
#include "catalogdata.h"
#include <QObject>
#include <QTimer>
#include <QSet>
#include <QHash>
#include <QPointer>
#include <optional>

class QProcess;

struct LibraryInitializationInspection {
    QString root, state, revision, reason;
    QStringList blockedPaths;
    int candidates = 0;
    bool canCreate = false, pathsTruncated = false;
};
Q_DECLARE_METATYPE(LibraryInitializationInspection)

// One asynchronous JSON-RPC session. A dispatched mutation has no kill deadline.
class BackendClient final : public QObject {
    Q_OBJECT
public:
    enum class Call { Snapshot, ImportPreview, ImportCommit, SetEnabled, Profile, ApplyPreview, Apply, Restore, Operation, Recover, RecoverySnapshot, LocalPrepare, LocalStatus, LocalReview, LocalCancel, LocalCommit, LocalDismiss, FontsPreview, FontsCommit, OptionsGet = 19, OptionsPrepare = 20, OptionsStatus = 21, OptionsCancel = 22, OptionsDismiss = 23, OptionsReview = 24, OptionsAttach = 25, ProfileEditPreview = 26, ProfileEditCommit = 27, LaunchOpen = 28, LaunchSession = 29, LaunchSettingsGet = 30, LaunchSettingsSet = 31, LaunchBegin = 33, LaunchCommit = 34, LaunchQuery = 35, LaunchBarriers = 36, LaunchResolve = 37, LaunchControl = 38, LaunchStatus = 39, LaunchCancel = 40, LaunchOwner = 41, LaunchClose = 42, GameStatus = 43, GameStop = 44, CatalogOpen = 45, CatalogBegin = 46, CatalogStatus = 47, CatalogCancel = 48, CatalogRelease = 49, CatalogClose = 50, LibraryInspect = 51, LibraryInitialize = 52,
        RemoteOpen = 153, RemoteResolve = 154, RemoteResolveStatus = 155, RemoteResolveRelease = 156,
        RemoteStart = 157, RemoteList = 158, RemoteStatus = 159, RemoteControl = 160,
        RemotePrepare = 161, RemoteCandidates = 162, RemoteCandidateMembers = 163, RemoteDiagnostics = 164,
        RemoteReview = 165, RemoteCommit = 166, RemoteClose = 167, RemotePrepareRelease = 168, RecoveryInspect = 169,
        RemoteReplaceTarget = 170, RemoteReplaceSkip = 171, RemoteReplaceStart = 172, RemoteReplaceList = 173,
        RemoteReplaceStatus = 174, RemoteReplaceReview = 175, RemoteReplaceCommit = 176, SetModsEnabled = 177, RemoveModsFromProfile = 178, PerformanceGet = 179, PerformanceSelect = 180, ConfigGet = 181, ConfigPreview = 182, ConfigCommit = 183, ConfigDecode = 184, BisectStart = 185, BisectAnswer = 186, BisectFinish = 187, HealthCheck = 188, HealthGet = 189, SetFixEnabled = 190, SetFixAutomatically = 191, ModUpdatesGet = 192, HeroDetect = 193, DeleteMods = 194 };
    explicit BackendClient(QObject *parent = nullptr);
    ~BackendClient() override;
    void start(const QString &helperPath, const QString &statePath, const QString &dataDir = {}, bool nativeFirst = false, bool localSession = false);
    void requestSnapshot(const QString &statePath, const QString &profileId, bool rediscoverNative = false);
    void requestBootstrapSnapshot();
    void nativeRequest(Call call, QJsonObject params = {}, const QJsonObject &launchBinding = {}, const CatalogRequestIdentity &catalogIdentity = {});
    bool catalogPaused() const;
    bool catalogOwned() const { return m_catalogOwned; }
    bool catalogControlPending(Call, const QString &key = {}) const;
    bool launchOwned() const { return m_launchOwned; }
    bool launchRetiring() const { return m_launchRetiring; }
    bool remoteOwned() const { return m_remoteOwned || m_remoteRecoveryOwned; }
    bool remoteOwnerOwned() const { return m_remoteOwned; }
    void setRemoteRecoveryOwned(bool value);
    bool launchWriting() const { return m_launchWriting; }
    void setLaunchWriting(bool value);
    void retireLaunch();
    void stop();
    bool isRunning() const { return m_process != nullptr; }
    bool readyForRequests() const { return m_ready; }
    bool helperStopped() const { return !m_ready && !m_stopReason.isEmpty(); }
    QString stopReason() const { return helperStopped() ? m_stopReason : QString(); }
    QString nativeRoot() const { return m_dataDir; }
    quint64 sessionGeneration() const { return m_session; }
    bool localSession() const { return m_localSession; }
    bool retiring() const { return !m_retiring.isNull(); }
    bool writing() const { return m_writing; }
    bool snapshotPending() const;
signals:
    void capabilitiesReady();
    void upstreamMissing();
    void configRejected(const QString &kind, const QString &error);
    void heroSettled();
    void heroReady(const QVariantMap &result);
    void modUpdatesReady(const QVariantMap &result);
    void healthReady(const QVariantMap &result);
    void configReady(BackendClient::Call call, const QVariantMap &result);
    void performanceReady(const QVariantMap &result);
    void bulkResultReady(const QVariantMap &summary);
    void lifecycleChanged();
    void launchReady(BackendClient::Call call, const QVariantMap &result);
    void catalogReady(BackendClient::Call call, const CatalogDecodedResult &result);
    void catalogRejected(BackendClient::Call call, const QString &requestKey, const QString &kind, const QString &message);
    void catalogDelayed(BackendClient::Call call, const QString &requestKey);
    void remoteReady(BackendClient::Call call, const QJsonObject &request, const QVariantMap &result);
    void remoteSnapshot(BackendClient::Call call, const QJsonObject &request, const LibrarySnapshot &snapshot);
    void remoteRejected(BackendClient::Call call, const QJsonObject &request, const QString &kind, const QString &message);
    void recoveryInspectionReady(const QJsonObject &request, const QVariantMap &result);
    void retired();
    void launchRetired();
    void localReady(BackendClient::Call call, const QVariantMap &result);
    void localCommitted(const LibrarySnapshot &snapshot);
    void fontsCommitted(const LibrarySnapshot &snapshot);
    void optionsCommitted(const LibrarySnapshot &snapshot);
    void profileEditCommitted(const LibrarySnapshot &snapshot);
    void applyCommitted(const LibrarySnapshot &snapshot);
    void recoverCommitted(const LibrarySnapshot &snapshot, const QString &snapshotDigest);
    void snapshotReady(const LibrarySnapshot &snapshot);
    void bootstrapInspectionReady(const LibraryInitializationInspection &inspection);
    void bootstrapInitialized(const QString &root, const QString &operationId);
    void bootstrapSnapshotReady(const LibrarySnapshot &snapshot);
    void recoverySnapshotReady(const LibrarySnapshot &snapshot);
    void reviewReady(BackendClient::Call call, const QVariantMap &review);
    void operationReady(const QVariantMap &outcome);
    void operationFontOffers(const QString &operationId, const QString &gamePath, const QVariantMap &mods);
    void nativeMissing();
    void completed(BackendClient::Call call);
    void failed(const QString &message);
    void rejected(BackendClient::Call call, const QString &kind, const QString &message);
    void unknownOutcome(const QString &message);
private:
    friend class ResponseDecoder;
    struct Request { Call call = Call::Snapshot; quint64 generation = 0; bool native = false, mutation = false; QString profile; QString job; bool bulk = false; QString mod, revision, deploymentLayout, capacityLayout, game, operationId; QJsonObject params, binding, receiptParams; Call receiptCall = Call::Snapshot; int catalogTicks = 0; bool catalogNotified = false, catalogDelayed = false; CatalogRequestIdentity catalogIdentity; bool bootstrapSnapshot = false; };
    bool acquireBulk();
    void releaseBulk();
    void flushDeferred();
    void retireProcess();
    void sendHello();
    void sendSnapshot();
    void receive();
    void drainStderr();
    void decodeNext();
    void handleResponse(const QJsonObject &response, const LibrarySnapshot &snapshot, bool validSnapshot, bool validOperationSnapshot, const QVariantMap &localResult, bool validLocal, qsizetype frameBytes);
    void fail(const QString &message);
    void send(qint64 id, const QString &method, const QJsonObject &params = {});
    QProcess *m_process = nullptr;
    QPointer<QProcess> m_retiring;
    bool m_localSession = false, m_deferredSnapshot = false;
    bool m_launchOwned = false, m_launchWriting = false, m_launchRetiring = false, m_launchCloseSent = false;
    bool m_remoteOwned = false, m_remoteRecoveryOwned = false;
    QString m_remoteRoot, m_remoteSession;
    QHash<QString, std::pair<Call, QJsonObject>> m_launchControls;
    std::optional<std::pair<Call, QJsonObject>> m_deferredBulk;
    static QPointer<BackendClient> s_bulkOwner;
    static QList<QPointer<BackendClient>> s_waiters;
    QTimer m_timeout, m_catalogTimer, m_launchRetirementTimer;
    bool m_catalogOwned = false;
    QString m_catalogRoot, m_catalogSession;
    QByteArray m_stdout, m_stderr;
    qsizetype m_stdoutScanned = 0;
    QString m_statePath, m_profileId, m_dataDir;
    QHash<qint64, Request> m_requests;
    qint64 m_nextId = 0, m_helloId = 0;
    quint64 m_session = 0, m_generation = 0, m_inspectGeneration = 0;
    QString m_stopReason; // why the helper was disconnected, shown on later refusals
    bool m_ready = false, m_parsing = false, m_nativeView = false, m_probeNative = false, m_writing = false, m_bootstrapSnapshotRequested = false;
};
