#pragma once
#include "backendclient.h"
#include <QUrl>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
class LocalImportController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(bool canPrepare READ canPrepare NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(bool canCancel READ canCancel NOTIFY changed)
    Q_PROPERTY(bool canCommit READ canCommit NOTIFY changed)
    Q_PROPERTY(bool canReview READ canReview NOTIFY changed)
    Q_PROPERTY(bool canDismiss READ canDismiss NOTIFY changed)
    Q_PROPERTY(bool visible READ visible WRITE setVisible NOTIFY changed)
    Q_PROPERTY(QString phase READ phase NOTIFY changed)
    Q_PROPERTY(QString destinationName READ destinationName NOTIFY changed)
    Q_PROPERTY(QString jobId READ jobId NOTIFY changed)
    Q_PROPERTY(QString contextId READ contextId NOTIFY contextChanged)
    Q_PROPERTY(bool presentationReady READ presentationReady NOTIFY changed)
    Q_PROPERTY(bool draftCurrent READ draftCurrent NOTIFY changed)
    Q_PROPERTY(bool cancelPending READ cancelPending NOTIFY changed)
    Q_PROPERTY(QString draftGeneration READ draftGeneration NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QVariantMap review READ review NOTIFY changed)
    Q_PROPERTY(QVariantList groupRows READ groupRows NOTIFY changed)
    Q_PROPERTY(QVariantMap groupResult READ groupResult NOTIFY changed)
    Q_PROPERTY(QVariantMap progress READ progress NOTIFY changed)
    Q_PROPERTY(QVariantList jobs READ jobs NOTIFY changed)
    Q_PROPERTY(QVariantMap metadataDraft READ metadataDraft WRITE setMetadataDraft NOTIFY changed)
    Q_PROPERTY(QStringList selectionDraft READ selectionDraft WRITE setSelectionDraft NOTIFY changed)
  public:
    explicit LocalImportController(LibraryModel *library);
    void beginRetirement(const QString &root, quint64 loadEpoch);
    bool retirementReady(const QString &root, quint64 loadEpoch) const;
    bool retirementInProgress() const { return m_retirementActive; }
    void resumeAfterRetirement();
    static bool decodeReply(const QJsonValue &value, QVariantMap &out, bool allRetained = false);
    bool optionsAvailable() const;
    bool optionsBusy() const;
    bool canCancelOptionsJob(const QString &jobId) const;
    bool cancelOptionsJob(const QString &jobId);
    bool optionsRequest(BackendClient::Call call, QJsonObject params = {});
    bool attachOptions(const QString &profile,const QString &revision,QJsonObject params,const QString &modId);
    void optionsFinished();
    void setOptionsHistory(int count,bool blocksPreparation,bool known);
    void refreshHistory();
    void syncContext();
    bool canPrepare() const;
    bool busy() const;
    bool canCancel() const;
    bool canCommit() const;
    bool canReview() const;
    bool canDismiss() const;
    bool visible() const { return m_visible; }
    void setVisible(bool value);
    QString phase() const;
    QString destinationName() const;
    QString jobId() const { return m_job; }
    QString contextId() const { return m_contextId; }
    bool presentationReady() const;
    bool draftCurrent() const;
    bool cancelPending() const;
    QString draftGeneration() const;
    QString error() const { return m_error; }
    QVariantMap review() const;
    QVariantList groupRows() const { return m_jobs.value(m_job).groupRows; }
    QVariantMap groupResult() const { return m_jobs.value(m_job).result; }
    QVariantMap progress() const;
    bool prepareGroup(const QList<QUrl> &roots, const QVariantList &rows);
    Q_INVOKABLE bool setGroupLink(const QString &id, const QString &link);
    Q_INVOKABLE bool dropGroupRow(const QString &id);
    QVariantList jobs() const;
    QVariantMap metadataDraft() const;
    QStringList selectionDraft() const;
    void setMetadataDraft(const QVariantMap &value);
    void setSelectionDraft(const QStringList &value);
    Q_INVOKABLE bool prepare(const QList<QUrl> &roots, const QUrl &artwork = {});
    Q_INVOKABLE QString startSourcePicker();
    Q_INVOKABLE void cancelSourcePicker(const QString &ticket);
    Q_INVOKABLE bool preparePicked(const QString &ticket, const QList<QUrl> &roots, const QUrl &artwork = {});
    Q_INVOKABLE bool prepareDrop(const QList<QUrl> &roots);
    Q_INVOKABLE bool reviewSelection(const QStringList &candidateIds, const QVariantMap &metadata);
    Q_INVOKABLE bool reviewCurrentDraft();
    Q_INVOKABLE bool cancel();
    Q_INVOKABLE bool cancelJob(const QString &jobId);
    Q_INVOKABLE bool canCancelJob(const QString &jobId) const;
    Q_INVOKABLE bool commit();
    Q_INVOKABLE bool dismiss(const QString &jobId);
    Q_INVOKABLE void showJob(const QString &jobId);
    void commitConfirmed(bool success, const QString &message = {});
  signals:
    void changed();
    void contextChanged();
    void preparationStarted(const QString &jobId, const QString &contextId);
    void cancellationRequested(const QString &jobId);
    void optionsReady(BackendClient::Call call,const QVariantMap &result);
    void optionsRejected(BackendClient::Call call,const QString &message);
    void optionsSessionReset();

  private:
    void driveRetirement();
    struct Job {
        QVariantMap status, review, metadata, result;
        QVariantList groupRows, originalGroupRows;
        QString diagnostic;
        QStringList selection;
        quint64 edits = 0;
        bool initialized = false, durableSuccess = false, groupReviewed = false;
    };
    struct Cancellation { BackendClient::Call call; QString jobId; };
    QString m_retirementRoot;
    quint64 m_retirementEpoch=0, m_retirementSession=0;
    bool m_retirementActive=false, m_retirementStopped=false;
    QTimer m_retirementPoll;
    bool canQueueCancellation(BackendClient::Call call,const QString &jobId) const;
    bool queueCancellation(BackendClient::Call call,const QString &jobId);
    void drainCancellations();
    void finishCancellation(BackendClient::Call call,const QString &jobId = {});
    void resetCancellations();
    bool cancelling() const;
    bool writable() const;
    bool controlAvailable() const;
    void updateContext();
    void poll();
    void accept(BackendClient::Call call, const QVariantMap &result);
    void fail(BackendClient::Call call, const QString &message);
    void requestCandidates();
    LibraryModel *m_library;
    BackendClient *m_client;
    QTimer m_poll;
    QMap<QString, Job> m_jobs;
    QString m_root, m_helper, m_job, m_error;
    QVariantList m_contextIdentity;
    QString m_contextId, m_pickerTicket, m_pickerContext;
    bool m_small = false, m_bulk = false, m_visible = true, m_commit = false, m_commitSession = false;
    bool m_historyRefresh = true;
    quint64 m_reviewEdits = 0;
    int m_optionsCount = 0;
    bool m_optionsPending = false, m_optionsWriting = false, m_optionsBlock = false, m_optionsHistoryKnown = false;
    QList<Cancellation> m_cancellations;
    QString m_cancelInFlight;
    BackendClient::Call m_cancelCall = BackendClient::Call::LocalCancel;
    BackendClient::Call m_optionsCall = BackendClient::Call::OptionsStatus;
};
