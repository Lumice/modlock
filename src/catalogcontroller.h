#pragma once
#include "backendclient.h"
#include "catalogmodel.h"
#include <QElapsedTimer>
#include <QtQml/qqmlregistration.h>

class CatalogController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(CatalogModel* model READ model CONSTANT)
    Q_PROPERTY(bool available READ available NOTIFY changed)
    Q_PROPERTY(bool active READ active WRITE setActive NOTIFY changed)
    Q_PROPERTY(bool canClearCache READ canClearCache NOTIFY changed)
    Q_PROPERTY(QString clearCacheReason READ clearCacheReason NOTIFY changed)
    Q_PROPERTY(bool hideNsfw READ hideNsfw WRITE setHideNsfw NOTIFY changed)
    Q_PROPERTY(bool contentReady READ contentReady NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(bool favoriteBusy READ favoriteBusy NOTIFY changed)
    Q_PROPERTY(bool advanced READ advanced WRITE setAdvanced NOTIFY changed)
    Q_PROPERTY(QString route READ route NOTIFY changed)
    Q_PROPERTY(QString status READ status NOTIFY changed)
    Q_PROPERTY(QString workNotice READ workNotice NOTIFY workNoticeChanged)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(QString errorSummary READ errorSummary NOTIFY changed)
    Q_PROPERTY(QString prerequisite READ prerequisite NOTIFY changed)
    Q_PROPERTY(QString contextNotice READ contextNotice NOTIFY changed)
    Q_PROPERTY(QString searchPrerequisite READ searchPrerequisite NOTIFY changed)
    Q_PROPERTY(QVariantMap queryState READ queryState NOTIFY changed)
    Q_PROPERTY(QStringList categories READ categories NOTIFY changed)
    Q_PROPERTY(QVariantMap heroArtwork READ heroArtwork NOTIFY changed)
    Q_PROPERTY(QStringList heroes READ heroes NOTIFY changed)
    Q_PROPERTY(QVariantList authors READ authors NOTIFY changed)
    Q_PROPERTY(QVariantMap detail READ detail NOTIFY changed)
    Q_PROPERTY(QVariantMap resource READ resource NOTIFY changed)
    Q_PROPERTY(QString resourceKind READ resourceKind NOTIFY changed)
    Q_PROPERTY(int resourcePage READ resourcePage NOTIFY changed)
    Q_PROPERTY(QVariantMap syncProgress READ syncProgress NOTIFY changed)
    Q_PROPERTY(QVariant lastCompletedAt READ lastCompletedAt NOTIFY changed)
    Q_PROPERTY(QString refreshMessage READ refreshMessage NOTIFY changed)
    Q_PROPERTY(QVariantMap provenance READ provenance NOTIFY changed)
    Q_PROPERTY(QString provenanceText READ provenanceText NOTIFY changed)
    Q_PROPERTY(QString total READ total NOTIFY changed)
    Q_PROPERTY(int page READ page NOTIFY changed)
    Q_PROPERTY(bool hasNext READ hasNext NOTIFY changed)
    Q_PROPERTY(bool canBack READ canBack NOTIFY changed)
    Q_PROPERTY(QString selectedIdentity READ selectedIdentity NOTIFY changed)
    Q_PROPERTY(QString focusKey READ focusKey NOTIFY changed)
    Q_PROPERTY(qreal scrollPosition READ scrollPosition NOTIFY changed)
    Q_PROPERTY(qulonglong viewGeneration READ viewGeneration NOTIFY changed)
    Q_PROPERTY(bool shutdownReady READ shutdownReady NOTIFY changed)
    Q_PROPERTY(int libraryActionRevision READ libraryActionRevision NOTIFY libraryActionChanged)
    Q_PROPERTY(bool homeActive READ homeActive WRITE setHomeActive NOTIFY homeChanged)
    Q_PROPERTY(bool homeLoading READ homeLoading NOTIFY homeChanged)
    Q_PROPERTY(bool homeUnavailable READ homeUnavailable NOTIFY homeChanged)
    Q_PROPERTY(qulonglong homeEpoch READ homeEpoch NOTIFY homeChanged)
    Q_PROPERTY(QVariantList homeLatest READ homeLatest NOTIFY homeChanged)
    Q_PROPERTY(QVariantList homePopular READ homePopular NOTIFY homeChanged)
    Q_PROPERTY(QVariantMap homeProvenance READ homeProvenance NOTIFY homeChanged)
public:
    explicit CatalogController(BackendClient *,QObject *parent=nullptr);
    CatalogModel *model() const {return m_model;}
    bool available() const;
    bool active() const {return m_active;}
    bool homeActive() const { return m_homeActive; }
    void setHomeActive(bool value);
    bool homeLoading() const;
    bool homeUnavailable() const { return m_homeUnavailable; }
    quint64 homeEpoch() const { return m_homeEpoch; }
    QVariantList homeLatest() const { return m_homeLatest; }
    QVariantList homePopular() const { return m_homePopular; }
    QVariantMap homeProvenance() const { return m_homeProvenance; }
    bool canClearCache() const { return clearCacheReason().isEmpty(); }
    QString clearCacheReason() const;
    bool hideNsfw() const { return m_hideNsfw; }
    void setHideNsfw(bool value);
    bool contentReady() const { return m_contentReady; }
    bool busy() const;
    bool favoriteBusy() const;
    bool advanced() const { return m_context.advanced; }
    void setAdvanced(bool value) { m_context.advanced=value; emit changed(); }
    QString route() const {return m_context.route;}
    QString status() const {return m_status;}
    QString workNotice() const {return m_workNotice;}
    QString error() const {return m_error;}
    QString errorSummary() const {return m_errorSummary.isEmpty()?m_error:m_errorSummary;}
    QString prerequisite() const;
    QString contextNotice() const {return m_contextNotice;}
    QString searchPrerequisite() const;
    QVariantMap queryState() const;
    QStringList categories() const {return m_categories;}
    QVariantMap heroArtwork() const {return m_heroArtwork;}
    QStringList heroes() const {return m_heroes;}
    QVariantList authors() const {return m_authors;}
    QVariantMap detail() const {return m_detail;}
    QVariantMap resource() const {return m_resource;}
    QString resourceKind() const {return m_resourceKind;}
    int resourcePage() const {return m_resourcePage;}
    QVariantMap provenance() const {return m_provenance;}
    QVariantMap syncProgress() const {return m_syncProgress.isEmpty()?m_provenance.value("syncProgress").toMap():m_syncProgress;}
    QVariant lastCompletedAt() const {return m_provenance.value("lastCompletedAt");}
    QString refreshMessage() const;
    QString provenanceText() const;
    QString total() const {return m_total;}
    int page() const {return m_context.page;}
    bool hasNext() const;
    bool canBack() const {return !m_history.isEmpty()||m_returnLibrary;}
    QString selectedIdentity() const {return m_context.selected;}
    QString focusKey() const {return m_context.focus;}
    qreal scrollPosition() const {return m_context.scroll;}
    quint64 viewGeneration() const {return m_generation;}
    bool shutdownReady() const {return m_shutdown&&!m_client->catalogOwned()&&!m_client->retiring();}
    QString libraryMediaSession() const { return m_session; }
    QString libraryMediaRoot() const { return m_root; }
    QString libraryMediaClientRoot() const { return m_client->nativeRoot(); }
    bool requestLibraryMedia(const QString &key, const SubmissionKey &submission, bool refresh, const QString &kind = QStringLiteral("detail"));
    void cancelLibraryMedia(const QString &key);
    void query(CatalogQuery);
    void openSubmission(SubmissionKey);
    void cancelView(quint64);
    void setFavorite(SubmissionKey,bool);
    void shutdown();
    void resumeAfterRetirement();
    void setActive(bool);
    Q_INVOKABLE void browse();
    int libraryActionRevision() const { return m_libraryActionRevision; }
    Q_INVOKABLE QVariantMap libraryAction(const QVariantMap &submission) const;
    Q_INVOKABLE bool runLibraryAction(const QVariantMap &submission);
    Q_INVOKABLE void updateQuery(const QVariantMap &);
    Q_INVOKABLE void openSubmission(const QVariantMap &,bool fromLibrary=false);
    Q_INVOKABLE void openCollection(const QString &);
    Q_INVOKABLE void openItem(const QString &);
    Q_INVOKABLE void author(const QString &);
    Q_INVOKABLE void findAuthors(const QString &);
    Q_INVOKABLE void back();
    Q_INVOKABLE void refresh();
    Q_INVOKABLE void clearCacheAndResync();
    Q_INVOKABLE void retry();
    Q_INVOKABLE void cancel();
    Q_INVOKABLE void nextPage(int delta);
    Q_INVOKABLE void loadResource(const QString &,int page=1,bool refresh=false);
    Q_INVOKABLE void closeResource();
    Q_INVOKABLE void setFavorite(const QVariantMap &,bool);
    Q_INVOKABLE void remember(const QString &,qreal,const QString &,qulonglong generation);
    Q_INVOKABLE QString displayCount(const QString &value) const;
    Q_INVOKABLE bool openLink(const QUrl &);
signals:
    void libraryActionChanged();
    void changed();
    void homeChanged();
    void workNoticeChanged();
    void returnToLibrary();
    void remoteRequested();
    void libraryMediaReady(const QString &key, const QString &root, const CatalogDecodedResult &result);
    void libraryMediaEvidence(const QString &root, const CatalogDecodedResult &result);
    void libraryMediaInvalidated(const QString &root, const QString &session, const QString &submission);
private:
    int m_libraryActionRevision = 0;
    struct Context {QString route="browse",collection,selected,focus="search";SubmissionKey submission;CatalogQuery query;int page=0; qreal scroll=0; bool advanced=false;};
    struct Receipt {qint64 nextPoll=0,fastUntil=0;CatalogRequestIdentity identity;QString digest,preferenceFloor,mediaKey,homeSlot;quint64 homeEpoch=0;bool homeNsfw=true;bool admitted=false,sent=false,obsolete=false,cancelled=false,terminal=false,released=false,reconcile=false,userCancelled=false,mediaReported=false,mediaInvalidated=false;bool isHome() const { return !homeSlot.isEmpty(); }};
    struct DelayedControl {BackendClient::Call call;QString key;};
    void pulse();
    void schedulePoll();
    int pollCadence(const Receipt &) const;
    void session();
    void deferPassiveOpen();
    bool passiveOpenDeferred();
    void readCurrent();
    void advanceHomeFeed();
    void invalidateHome(bool clearResults,bool resetUnavailable=true);
    void cancelHomeReceipts();
    void begin(const QString &,const QJsonObject &,bool rootWide=false);
    void control(BackendClient::Call,const QString &key={});
    void received(BackendClient::Call,const CatalogDecodedResult &);
    void rejected(BackendClient::Call,const QString &,const QString &,const QString &);
    void publish(const CatalogDecodedResult &);
    void transition(Context,bool push);
    bool reconcileFavorite(const SubmissionKey &);
    void reconcileSelectedFavorite();
    void invalidateSelectedFavorite();
    bool clearDelayedControl(BackendClient::Call,const QString &);
    QString calculateWorkNotice() const;
    void updateWorkNotice();
    void setError(const QString &error,const QString &summary={}) {m_error=error;m_errorSummary=summary;}
    bool acceptPreferencesRevision(const QString &,bool pageProjection=false);
    bool current(const Receipt &) const;
    void loseSession();
    void preemptLibraryMedia();
    void failLibraryOpen(const QString &kind, const QString &message);
    bool libraryControlPending(const QString &key) const;
    void detachMediaControls(const QString &key);
    void pruneDetachedLibraryControls();
    void pruneDetachedMediaInvalidations();
    BackendClient *m_client;
    CatalogModel *m_model;
    QTimer m_poll;
    QTimer m_passiveOpenRetry;
    qint64 m_passiveOpenDeadline=0;
    int m_passiveOpenDelay=2000;
    QElapsedTimer m_pollClock;
    QHash<QString,Receipt> m_receipts;
    QHash<QString,CatalogRequestIdentity> m_detachedLibraryControls;
    QHash<QString,CatalogRequestIdentity> m_detachedMediaInvalidations;
    QList<DelayedControl> m_delayedControls;
    QList<Context> m_history;
    Context m_context;
    quint64 m_generation=1;
    QString m_root,m_session,m_phase,m_status,m_workNotice,m_error,m_errorSummary,m_preferencesRevision,m_total="0",m_contextNotice,m_resourceKind;
    QVariantMap m_heroArtwork;
    QStringList m_categories,m_heroes;
    QVariantList m_authors;
    QVariantMap m_detail,m_resource,m_provenance,m_syncProgress;
    std::optional<CatalogDecodedResult> m_currentPage, m_adjacent;
    std::optional<SubmissionKey> m_deferredFavorite;
    struct SelectedFavorite {SubmissionKey key;bool value=false;QString revision;};
    std::optional<SelectedFavorite> m_selectedFavorite;
    QString m_selectedReconcileRevision;
    QString m_startupRoot;
    QString m_libraryOpenKey, m_libraryOpenRoot;
    bool m_passiveOpening=false,m_mediaDrainWakeScheduled=false;
    int m_resourcePage=1;
    bool m_resourceAutoAttempted=false;
    bool m_contentReady=false;
    bool m_pausedNotice=false;
    bool m_mediaDeferred=false; // a Library media read was refused during a pause; signal when it ends
    bool m_hideNsfw=true;
    bool m_active=false,m_shutdown=false,m_readNeeded=true,m_autoAttempted=false,m_returnLibrary=false,m_reopen=false,m_closeSent=false;
    bool m_homeActive=false,m_homeDeferred=false,m_homeLatestDone=false,m_homePopularDone=false,m_homeUnavailable=false;
    quint64 m_homeEpoch=1;
    QVariantList m_homeLatest,m_homePopular;
    QVariantMap m_homeProvenance;
    int m_keys=0,m_libraryReturnDepth=0;
};
