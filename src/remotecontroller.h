#pragma once

#include "backendclient.h"
#include <QJsonObject>
#include <QElapsedTimer>
#include <QSet>
#include <QTimer>
#include <QtQml/qqmlregistration.h>

class LibraryModel;

class RemoteController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(QString linkedFileId READ linkedFileId NOTIFY changed)
    Q_PROPERTY(bool available READ available NOTIFY changed)
    Q_PROPERTY(bool visible READ visible NOTIFY changed)
    Q_PROPERTY(bool downloadsRequested READ downloadsRequested NOTIFY changed)
    Q_PROPERTY(QString continuingDownload READ continuingDownload NOTIFY changed)
    Q_PROPERTY(bool retiring READ retiring NOTIFY changed)
    Q_PROPERTY(QString phase READ phase NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QString noticeError READ noticeError NOTIFY changed)
    Q_PROPERTY(QVariantList jobs READ jobs NOTIFY changed)
    Q_PROPERTY(QVariantList files READ files NOTIFY changed)
    Q_PROPERTY(QVariantList candidates READ candidates NOTIFY changed)
    Q_PROPERTY(QVariantList members READ members NOTIFY changed)
    Q_PROPERTY(QVariantList diagnostics READ diagnostics NOTIFY changed)
    Q_PROPERTY(QStringList selectedIds READ selectedIds WRITE setSelectedIds NOTIFY changed)
    Q_PROPERTY(QStringList selectedChoiceNames READ selectedChoiceNames NOTIFY changed)
    Q_PROPERTY(QVariantMap review READ review NOTIFY changed)
    Q_PROPERTY(QVariantMap progress READ progress NOTIFY changed)
    Q_PROPERTY(QVariantMap job READ job NOTIFY changed)
    Q_PROPERTY(QString currentJobId READ currentJobId NOTIFY changed)
    Q_PROPERTY(QString resolutionIdentity READ resolutionIdentity NOTIFY changed)
    Q_PROPERTY(QString prepareKey READ prepareKey NOTIFY changed)
    Q_PROPERTY(QString nextControlSeq READ nextControlSeq NOTIFY changed)
    Q_PROPERTY(int filePage READ filePage NOTIFY changed)
    Q_PROPERTY(bool fileHasMore READ fileHasMore NOTIFY changed)
    Q_PROPERTY(int candidatePage READ candidatePage NOTIFY changed)
    Q_PROPERTY(bool candidateHasMore READ candidateHasMore NOTIFY changed)
    Q_PROPERTY(int memberPage READ memberPage NOTIFY changed)
    Q_PROPERTY(bool memberHasMore READ memberHasMore NOTIFY changed)
    Q_PROPERTY(QString memberCandidateId READ memberCandidateId NOTIFY changed)
    Q_PROPERTY(int diagnosticPage READ diagnosticPage NOTIFY changed)
    Q_PROPERTY(bool diagnosticHasMore READ diagnosticHasMore NOTIFY changed)
    Q_PROPERTY(bool canCommit READ canCommit NOTIFY changed)
    Q_PROPERTY(bool canResolve READ canResolve NOTIFY changed)
    Q_PROPERTY(bool hasConsent READ hasConsent NOTIFY changed)
    Q_PROPERTY(bool canPrepare READ canPrepare NOTIFY changed)
    Q_PROPERTY(bool canReleasePreparation READ canReleasePreparation NOTIFY changed)
    Q_PROPERTY(bool preparationReleasePending READ preparationReleasePending NOTIFY changed)
    Q_PROPERTY(bool hasResolution READ hasResolution NOTIFY changed)
    Q_PROPERTY(bool replacementAvailable READ replacementAvailable NOTIFY changed)
    Q_PROPERTY(bool replacementActive READ replacementActive NOTIFY changed)
    Q_PROPERTY(bool replacementFileSelection READ replacementFileSelection NOTIFY changed)
    Q_PROPERTY(bool replacementJob READ replacementJob NOTIFY changed)
    Q_PROPERTY(bool automaticReplacementJob READ automaticReplacementJob NOTIFY changed)
    Q_PROPERTY(QString replacementAction READ replacementAction NOTIFY changed)
    Q_PROPERTY(QString replacementModId READ replacementModId NOTIFY changed)
    Q_PROPERTY(QString replacementProfileId READ replacementProfileId NOTIFY changed)
    Q_PROPERTY(QVariantMap replacementTarget READ replacementTarget NOTIFY changed)
    Q_PROPERTY(QVariantMap replacementSkip READ replacementSkip NOTIFY changed)
    Q_PROPERTY(bool replacementObservationPending READ replacementObservationPending NOTIFY changed)
    Q_PROPERTY(bool replacementTargetCurrent READ replacementTargetCurrent NOTIFY changed)
    Q_PROPERTY(bool skipPending READ skipPending NOTIFY changed)
    Q_PROPERTY(QString savedFileObservation READ savedFileObservation NOTIFY changed)
    Q_PROPERTY(QStringList selectedFileIds READ selectedFileIds NOTIFY changed)
    Q_PROPERTY(quint64 selectedFileBytes READ selectedFileBytes NOTIFY changed)
public:
    explicit RemoteController(LibraryModel *model, BackendClient *client);
    QString linkedFileId() const { return hasResolution() ? m_linkedFileId : QString(); }
    bool resolveInstallLink(const QVariantMap &submission, const QString &fileId);
    bool available() const;
    bool visible() const { return m_visible; }
    bool downloadsRequested() const { return m_downloadsRequested; }
    QString continuingDownload() const { return m_continuingJob; }
    bool retiring() const { return m_retiring || (m_shutdown && m_client->remoteOwned()); }
    QString phase() const { return m_phase; }
    QString error() const { return m_error; }
    QString noticeError() const { return m_noticeError==m_error ? m_noticeError : QString(); }
    QVariantList jobs() const;
    QVariantList files() const { return m_files; }
    QVariantList candidates() const { return m_candidates; }
    QVariantList members() const { return m_members; }
    QVariantList diagnostics() const { return m_diagnostics; }
    QStringList selectedIds() const { return m_selectedIds; }
    QStringList selectedChoiceNames() const;
    void setSelectedIds(QStringList ids);
    QVariantMap review() const { return m_review; }
    QVariantMap progress() const { return m_job.value("progress").toMap(); }
    QVariantMap job() const { return m_job; }
    QString currentJobId() const { return m_jobId; }
    QString resolutionIdentity() const { return m_resolveKey; }
    QString prepareKey() const { return m_prepareKey; }
    QString nextControlSeq() const;
    int filePage() const { return m_filePage; }
    bool fileHasMore() const { return m_fileHasMore; }
    int candidatePage() const { return m_candidatePage; }
    bool candidateHasMore() const { return m_candidateHasMore; }
    int memberPage() const { return m_memberPage; }
    bool memberHasMore() const { return m_memberHasMore; }
    QString memberCandidateId() const { return m_memberCandidate; }
    int diagnosticPage() const { return m_diagnosticPage; }
    bool diagnosticHasMore() const { return m_diagnosticHasMore; }
    bool canCommit() const;
    bool canResolve() const;
    bool hasConsent() const { return !m_consents.value(m_jobId).isEmpty(); }
    bool canPrepare() const;
    bool canReleasePreparation() const;
    bool preparationReleasePending() const { return !m_releaseKey.isEmpty(); }
    bool hasResolution() const { return !m_resolveKey.isEmpty(); }
    bool replacementAvailable() const;
    bool replacementActive() const { return !m_replaceAction.isEmpty(); }
    bool replacementFileSelection() const;
    bool replacementJob() const { return !m_job.value("purpose").toString().isEmpty(); }
    QString replacementAction() const { return m_replaceAction; }
    QString replacementModId() const { return m_replaceMod; }
    QString replacementProfileId() const { return m_replaceProfile; }
    QVariantMap replacementTarget() const { return m_replaceTarget; }
    QVariantMap replacementSkip() const { return m_replaceSkip; }
    bool replacementObservationPending() const { return !m_replaceParams.isEmpty(); }
    bool replacementTargetCurrent() const { return replacementContextCurrent(); }
    bool skipPending() const { return !m_skipParams.isEmpty(); }
    QString savedFileObservation() const { return m_savedFileObservation; }
    bool shutdownReady() const { return m_shutdown && !m_client->remoteOwned() && !m_client->retiring(); }

    Q_INVOKABLE QVariantMap installMetadata(const QString &jobId) const { return m_installMetadata.value(jobId); }
    Q_INVOKABLE QString installName(const QString &jobId) const;
    Q_INVOKABLE bool addSelection(const QVariantMap &metadata);
    QStringList selectedFileIds() const;
    quint64 selectedFileBytes() const;
    Q_INVOKABLE void pickFile(const QString &fileId, bool selected, bool acceptArchived = false);
    Q_INVOKABLE void pickAllFiles(bool selected);
    Q_INVOKABLE bool startSelectedFiles();
    Q_INVOKABLE bool retryInstall(const QString &jobId);
    Q_INVOKABLE void showDownloads();
    Q_INVOKABLE void refreshDownloads();
    Q_INVOKABLE bool resolve(const QVariantMap &submission);
    Q_INVOKABLE bool beginReplacement(const QString &modId,const QString &action);
    Q_INVOKABLE bool beginCatalogUpdate(const QString &modId) { return beginBatchReplacement(modId); }
    Q_INVOKABLE bool refreshReplacementJob();
    Q_INVOKABLE bool refreshReplacementObservation();
    Q_INVOKABLE bool startReplacement(const QString &fileId,const QString &candidateFingerprint,const QString &purpose,bool acceptArchived);
    Q_INVOKABLE bool changeSkip(const QString &fileId,const QString &candidateFingerprint,bool clear);
    Q_INVOKABLE bool loadFiles(int page);
    Q_INVOKABLE bool start(const QString &fileId, const QString &candidateFingerprint, bool acceptArchived);
    Q_INVOKABLE bool bindConsent(const QString &jobId, const QString &fileId, const QString &candidateFingerprint, bool acknowledge);
    Q_INVOKABLE void releaseResolution();
    Q_INVOKABLE bool selectJob(const QString &jobId);
    Q_INVOKABLE bool control(const QString &jobId, const QString &controlSeq, const QString &action);
    Q_INVOKABLE bool prepare(const QString &jobId, bool acknowledgeRebase);
    Q_INVOKABLE bool releasePreparationForRetry();
    Q_INVOKABLE bool loadCandidates(const QString &jobId, const QString &prepareKey, int page);
    Q_INVOKABLE bool loadMembers(const QString &jobId, const QString &prepareKey, const QString &candidateId, int page);
    Q_INVOKABLE bool loadDiagnostics(const QString &jobId, const QString &prepareKey, int page);
    Q_INVOKABLE bool reviewSelection(const QString &jobId, const QStringList &candidateIds, const QVariantMap &metadata);
    Q_INVOKABLE void invalidateReview();
    Q_INVOKABLE bool commit(const QString &jobId);
    Q_INVOKABLE void close();
    void loadRoot(const QString &root);
    void shutdown();
    void commitSettled(bool success, const QString &message);
    void reconcileStatus(const QString &jobId);
    void replayCommit(const QVariantMap &marker, const QString &reviewToken = {});
    void confirmReplacementTarget(const QVariantMap &marker,const QString &revision);
signals:
    void changed();
    void retryStarted(const QString &jobId);
    void statusEvidence(const QVariantMap &status);
    void replacementTargetEvidence(const QJsonObject &request,const QVariantMap &result);
    void replacementTargetRejected(const QJsonObject &request,const QString &kind,const QString &message);
private:
    friend class ModelCheck;
    friend class LibraryModel;
    bool beginBatchReplacement(const QString &modId);
    bool beginReplacementFor(const QString &modId,const QString &action,bool batch);
    bool m_batchReplacement=false;
    bool automaticReplacementJob() const { return m_installTargets.contains(m_jobId); }
    QVariantMap replacementSubmission() const;
    bool installTargetCurrent() const;
    QVariantMap existingSubmissionMod() const;
    void startNextFile();
    void clearInstallQueue(const QString &jobId);
    QVariantList m_pickedFiles, m_downloadQueue;
    QJsonObject m_appendParams;
    QVariantMap m_appendTarget;
    QHash<QString,QVariantMap> m_installTargets;
    QString m_queueProfile, m_queueMod, m_nextReplacementPurpose, m_queueJob;
    QString m_resolutionRevision;
    bool m_collectFiles=false, m_choosingFiles=false;
    void advanceInstall();
    void stopInstall(const QString &message);
    QHash<QString, QString> m_installStages;
    QHash<QString, QString> m_installResultMods;
    QHash<QString, qint64> m_installCompleted;
    QHash<QString, QString> m_installErrors;
    QHash<QString, quint64> m_failureSequences;
    QHash<QString, QVariantMap> m_installMetadata;
    QVariantList m_installCandidates;
    QString m_linkedFileId;
    void pulse();
    void send(BackendClient::Call call, QJsonObject params = {});
    void received(BackendClient::Call call, const QJsonObject &request, const QVariantMap &result);
    void rejected(BackendClient::Call call, const QJsonObject &request, const QString &kind, const QString &message);
    bool current(const QJsonObject &request) const;
    bool sameJob(const QJsonObject &request) const;
    bool currentPreparation(const QString &jobId, const QString &key) const;
    void resetReview();
    void acceptStatus(const QVariantMap &status);
    void requestReplacementObservation();
    void renewResolution();
    void releaseSkipResolution();
    void invalidateReplacementResolution();
    bool replacementContextCurrent() const;
    bool pendingStartListed(const QString &jobId) const;
    void requestJobStatus(const QString &jobId);
    void continueDownloaded();
    BackendClient::Call statusCall(const QString &jobId) const;
    QString requestIdentity(BackendClient::Call call, QJsonObject params) const;
    LibraryModel *m_model;
    BackendClient *m_client;
    QTimer m_poll;
    QHash<QString,QString> m_deferredActions;
    QElapsedTimer m_replaceObservationAge;
    QSet<QString> m_inFlight;
    QSet<QString> m_selectable;
    QSet<QString> m_seenCandidates;
    QSet<QString> m_resolveReleases;
    QSet<int> m_seenFilePages;
    QHash<int,QByteArray> m_filePageEvidence;
    QHash<QString,QPair<int,QByteArray>> m_fileEvidence, m_candidateEvidence;
    QHash<QString,QJsonObject> m_pendingStarts, m_pendingControls;
    QHash<QString,QJsonObject> m_startPins;
    QHash<QString,QString> m_consents;
    QHash<QString,QString> m_candidateNames;
    QHash<QString,QVariantMap> m_statuses;
    QJsonObject m_submission, m_resolveParams, m_prepareParams, m_reviewParams, m_replayParams;
    QJsonObject m_replaceParams,m_skipParams,m_confirmationParams;
    BackendClient::Call m_replayCall=BackendClient::Call::RemoteCommit;
    QVariantList m_jobs, m_files, m_candidates, m_members, m_diagnostics;
    QVariantList m_addJobs,m_replaceJobs;
    QVariantMap m_job, m_review;
    QVariantMap m_replaceTarget,m_replaceSkip;
    QStringList m_selectedIds;
    QString m_root, m_session, m_phase = QStringLiteral("idle"), m_error, m_noticeError, m_resolveKey, m_selectionToken;
    QString m_jobId, m_commitJobId, m_prepareKey, m_reviewKey, m_memberCandidate, m_releaseJob, m_releaseKey, m_reconcileJob;
    QString m_replaceAction,m_replaceProfile,m_replaceMod,m_replaceRevision;
    QString m_skipHeldResolve, m_continuingJob;
    bool m_downloadsRequested=false;
    QString m_savedFileObservation;
    int m_lastFilePage=-1;
    int m_filePage=0, m_candidatePage=0, m_memberPage=0, m_diagnosticPage=0;
    bool m_fileHasMore=false, m_candidateHasMore=false, m_memberHasMore=false, m_diagnosticHasMore=false;
    bool m_visible=false, m_shutdown=false, m_retiring=false, m_openReady=false, m_resolveAdmitted=false, m_prepareAdmitted=false, m_reviewAdmitted=false, m_committing=false, m_candidatesLoaded=false;
    bool m_replaceExpiryNotified=false;
    bool m_addListCurrent=false, m_replaceListCurrent=false;
};
