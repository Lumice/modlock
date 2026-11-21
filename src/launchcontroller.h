#pragma once
#include "backendclient.h"
#include "launchhealth.h"
#include <QElapsedTimer>
#include <QtQml/qqmlregistration.h>
#include <memory>

class LibraryModel;

class LaunchController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(LaunchHealth* health READ health CONSTANT)
    Q_PROPERTY(bool available READ available NOTIFY changed)
    Q_PROPERTY(bool canLaunch READ canLaunch NOTIFY changed)
    Q_PROPERTY(bool canCommit READ canCommit NOTIFY changed)
    Q_PROPERTY(bool canCancel READ canCancel NOTIFY changed)
    Q_PROPERTY(bool canStopWaiting READ canStopWaiting NOTIFY changed)
    Q_PROPERTY(bool canResolve READ canResolve NOTIFY changed)
    Q_PROPERTY(bool canStop READ canStop NOTIFY changed)
    Q_PROPERTY(bool writeGate READ writeGate NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(QString phase READ phase NOTIFY changed)
    Q_PROPERTY(QString activeProfileName READ activeProfileName NOTIFY changed)
    Q_PROPERTY(QVariantMap attempt READ attempt NOTIFY changed)
    Q_PROPERTY(QString preparationOutcome READ preparationOutcome NOTIFY changed)
    Q_PROPERTY(QString preparationError READ preparationError NOTIFY changed)
    Q_PROPERTY(bool preparationViewConfirmed READ preparationViewConfirmed NOTIFY changed)
    Q_PROPERTY(QString targetProfileId READ targetProfileId NOTIFY changed)
    Q_PROPERTY(QString targetProfileName READ targetProfileName NOTIFY changed)
    Q_PROPERTY(QString mode READ mode NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QString preparationState READ preparationState NOTIFY changed)
    Q_PROPERTY(QString handoffState READ handoffState NOTIFY changed)
    Q_PROPERTY(QString observationState READ observationState NOTIFY changed)
    Q_PROPERTY(QVariantMap review READ review NOTIFY changed)
    Q_PROPERTY(QVariantMap progress READ progress NOTIFY changed)
    Q_PROPERTY(QVariantMap facts READ facts NOTIFY changed)
    Q_PROPERTY(QVariantMap gameStatus READ gameStatus NOTIFY changed)
    Q_PROPERTY(QVariantList barriers READ barriers NOTIFY changed)
    Q_PROPERTY(QString additionalArguments READ additionalArguments NOTIFY settingsChanged)
    Q_PROPERTY(QString settingsRevision READ settingsRevision NOTIFY settingsChanged)
    Q_PROPERTY(QString settingsError READ settingsError NOTIFY settingsChanged)
    Q_PROPERTY(bool settingsSavePending READ settingsSavePending NOTIFY settingsChanged)
    Q_PROPERTY(QString settingsContextId READ settingsContextId NOTIFY settingsContextChanged)
    Q_PROPERTY(QVariantMap settingsSnapshot READ settingsSnapshot NOTIFY settingsSnapshotChanged)
    Q_PROPERTY(QVariantMap settingsReadState READ settingsReadState NOTIFY settingsReadStateChanged)
    Q_PROPERTY(QVariantMap settingsSaveState READ settingsSaveState NOTIFY settingsSaveStateChanged)
public:
    explicit LaunchController(LibraryModel *model, BackendClient *client);
    LaunchHealth *health() const { return m_health; }
    bool available() const;
    bool canLaunch() const;
    bool canCommit() const;
    bool canCancel() const;
    bool canStopWaiting() const;
    bool canResolve() const;
    bool canStop() const;
    bool writeGate() const;
    bool busy() const { return m_pending.has_value() || m_fileBusy; }
    QString phase() const { return m_phase; }
    QString activeProfileName() const;
    QVariantMap attempt() const { return m_attempt.toVariantMap(); }
    QString preparationOutcome() const { return m_preparationOutcome; }
    QString preparationError() const { return m_preparationError; }
    bool preparationViewConfirmed() const { return m_preparationViewConfirmed; }
    QString targetProfileId() const { return m_binding["profileId"].toString(); }
    QString targetProfileName() const;
    QString mode() const { return m_binding["mode"].toString(); }
    QString error() const { return m_error; }
    QString preparationState() const { return m_facts.value("preparationState").toString(); }
    QString handoffState() const { return m_facts.value("handoffState").toString(); }
    QString observationState() const { return m_facts.value("observationState").toString(); }
    QVariantMap review() const { return m_ready.toVariantMap(); }
    QVariantMap progress() const { return m_facts.value("progress").toMap(); }
    QVariantMap facts() const { return m_facts; }
    QVariantMap gameStatus() const;
    QVariantList barriers() const { return m_barriers; }
    QString additionalArguments() const { return m_arguments; }
    QString settingsRevision() const { return m_settingsRevision; }
    QString settingsError() const { return m_settingsError; }
    bool settingsSavePending() const { const auto status=m_settingsSaveState.value("status").toString(); return status=="opening"||status=="saving"||status=="unconfirmed"||status=="refused"; }
    QString settingsContextId() const { return QString::number(m_rootGeneration); }
    QVariantMap settingsSnapshot() const { return m_settingsSnapshot; }
    QVariantMap settingsReadState() const { return m_settingsReadState; }
    QVariantMap settingsSaveState() const { return m_settingsSaveState; }
    Q_INVOKABLE bool launchActive(const QString &mode = QStringLiteral("modded"));
    Q_INVOKABLE bool commit();
    Q_INVOKABLE bool cancel();
    Q_INVOKABLE bool checkAgain();
    Q_INVOKABLE bool stopGame(const QString &sessionToken);
    Q_INVOKABLE bool readSettings();
    Q_INVOKABLE bool saveSettings(const QString &additionalArguments);
    Q_INVOKABLE bool reconcileSettings();
    Q_INVOKABLE bool queryBarriers();
    Q_INVOKABLE bool resolveBarrier(bool steamIntentCancelled);
    Q_INVOKABLE bool retire();
    void loadRoot(const QString &root);
    void detachRoot();
    void contextChanged();
    void shutdown();
    void transportLost(const QString &message);
    bool viewRead(const QString &revision);
    bool holdsSetup() const { return !m_binding.isEmpty(); }
    bool knownBarrier() const { return !m_barriers.isEmpty(); }
signals:
    void changed();
    void settingsChanged();
    void settingsContextChanged();
    void settingsSnapshotChanged();
    void settingsReadStateChanged();
    void settingsSaveStateChanged();
private:
    QElapsedTimer m_barrierRetryAge;
    friend class LibraryModel;
    friend class ModelCheck;
    struct Pending {
        BackendClient::Call call;
        QJsonObject params;
        QString root;
        quint64 generation = 0;
        bool retryAdmission = false;
    };
    enum class Intent { None, Launch, SaveSettings, Resolve };
    void send(BackendClient::Call call, QJsonObject params = {});
    void reply(BackendClient::Call call, const QVariantMap &result);
    void complete(BackendClient::Call call, const QJsonObject &result, const Pending *pending = nullptr);
    void settingsReadResult(const QString &status, const QString &error = {});
    void settingsSaveResult(const QString &status, const QString &error = {});
    void poll();
    void scheduleFollowUp(int delay);
    void open(Intent intent);
    void continueIntent();
    void acceptAttempt(const QJsonObject &attempt, bool live);
    void abandonSettingsOpen(const QString &detail);
    void checkSettingsSave(const QString &detail, bool refused = false);
    bool settingsPendingCurrent(const Pending &pending) const;
    QString path(const QString &suffix) const;
    void update();
    void maybeAutoCommit();
    void beginAttempt();
    bool unsentOpenDraft() const;
    void retireUnsentOpen();
    bool contextCurrent() const;
    bool viewReadDue() const;
    void readView();
    LaunchHealth *m_health;
    LibraryModel *m_model;
    BackendClient *m_client;
    QTimer m_poll, m_followUp;
    int m_statusDelay = 50;
    QElapsedTimer m_gameAge;
    QSet<int> m_inFlight;
    std::optional<Pending> m_pending;
    Intent m_intent = Intent::None;
    QJsonObject m_binding, m_ready, m_attempt, m_resolving, m_openDraft;
    QVariantMap m_facts, m_game;
    bool m_gameRunningLock=false;
    QVariantMap m_settingsSnapshot, m_settingsReadState, m_settingsSaveState;
    QVariantList m_barriers;
    QString m_root, m_epoch, m_phase = QStringLiteral("idle"), m_error, m_profileName;
    QString m_preparationOutcome=QStringLiteral("unknown"),m_preparationError;
    bool m_preparationViewConfirmed=false;
    QString m_arguments, m_settingsRevision, m_settingsError, m_saveArguments;
    QString m_settingsReadControlId;
    quint64 m_rootGeneration=0, m_sequence=0, m_openDraftGeneration=0, m_settingsSnapshotSerial=0;
    bool m_haveSequence=false, m_liveGesture=false, m_liveAttempt=false, m_beginSent=false, m_beginAdmitted=false, m_committed=false, m_fileBusy=false;
    bool m_viewReadSent=false;
    bool m_statusUnavailable=true, m_terminalRead=false;
    bool m_shuttingDown=false, m_settingsReadNeeded=false, m_barriersReadNeeded=false, m_attemptReadNeeded=false;
};
