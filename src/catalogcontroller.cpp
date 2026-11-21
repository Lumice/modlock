#include "catalogcontroller.h"
#include "librarymodel.h"
#include "libraryrows.h"
#include <QDateTime>
#include <QScopeGuard>
#include <QTimeZone>
#include <QDesktopServices>
#include <QJsonDocument>
#include <QLocale>
#include <QUuid>
#include <algorithm>
#include <functional>
#include <utility>
#include <cmath>

namespace {
QString token(){return QUuid::createUuid().toString(QUuid::Id128);}
QJsonObject collectionArgs(const QString &id,int page,bool refresh){return {{"collectionId",id},{"page",page},{"pageSize",50},{"refresh",refresh}};}
}
CatalogController::CatalogController(BackendClient *client,QObject *parent):QObject(parent),m_client(client),m_model(new CatalogModel(this)) {
    QTimer::singleShot(0,this,[this]{
        auto *library=qobject_cast<LibraryModel *>(this->parent());
        if(!library)return;
        auto refresh=[this]{++m_libraryActionRevision;emit libraryActionChanged();};
        connect(library->sourceModel(),&QAbstractItemModel::modelReset,this,refresh);
        connect(library->sourceModel(),&QAbstractItemModel::dataChanged,this,refresh);
        connect(library,&LibraryModel::nativeChanged,this,refresh);
        connect(library,&LibraryModel::sourceChanged,this,refresh);
        connect(library,&LibraryModel::updatesChanged,this,refresh);
        connect(library->remote(),&RemoteController::changed,this,refresh);
        refresh();
    });
    m_pollClock.start();
    m_passiveOpenRetry.setParent(this);
    m_passiveOpenRetry.setSingleShot(true);
    m_passiveOpenRetry.setTimerType(Qt::PreciseTimer);
    connect(&m_passiveOpenRetry,&QTimer::timeout,this,[this]{
        if(!m_shutdown&&available()){emit changed();if(m_homeActive&&!m_active&&!m_homeUnavailable)advanceHomeFeed();}
    });
    m_poll.setInterval(250);
    m_poll.setTimerType(Qt::PreciseTimer);
    connect(this,&CatalogController::changed,this,&CatalogController::updateWorkNotice);
    connect(&m_poll,&QTimer::timeout,this,&CatalogController::pulse);
    connect(client,&BackendClient::catalogReady,this,&CatalogController::received);
    connect(client,&BackendClient::catalogRejected,this,&CatalogController::rejected);
    connect(client,&BackendClient::catalogDelayed,this,[this](auto,const QString &key){
        if(m_detachedLibraryControls.contains(key)||(!key.isEmpty()&&m_receipts.contains(key)&&(!m_receipts.value(key).mediaKey.isEmpty()||m_receipts.value(key).isHome())) || (key.isEmpty()&&m_passiveOpening&&!m_active))return;
        m_status=tr("Catalog is waiting for shared Library work.");emit changed();});
    connect(client,&BackendClient::capabilitiesReady,this,[this]{
        const bool rootChanged=!m_startupRoot.isEmpty()&&m_startupRoot!=m_client->nativeRoot();
        if(rootChanged){
            m_contentReady=false;m_context=Context{};m_history.clear();m_detail.clear();m_resource.clear();m_resourceKind.clear();m_provenance.clear();m_model->replace({},{});m_categories.clear();m_heroes.clear();m_heroArtwork.clear();m_authors.clear();m_returnLibrary=false;m_libraryReturnDepth=0;++m_generation;
        }
        invalidateHome(true,true);
        loseSession();if(m_active&&!m_shutdown)session();else if(m_homeActive&&!m_shutdown)advanceHomeFeed();emit changed();});
    connect(client,&BackendClient::lifecycleChanged,this,[this]{if(m_client->retiring()||!m_client->isRunning())loseSession();emit changed();});
    connect(client,&BackendClient::retired,this,[this]{loseSession();emit changed();});
}
QVariantMap CatalogController::libraryAction(const QVariantMap &submission) const {
    const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(submission));
    const auto *library=qobject_cast<const LibraryModel *>(parent());
    if(!key || !library)return {{"action","unavailable"}};
    const auto *rows=static_cast<const LibraryRows *>(library->sourceModel());
    QVariantMap match;
    for(int i=0;i<rows->rowCount();++i) {
        const auto index=rows->index(i);
        const auto id=rows->data(index,LibraryRows::ModId).toString();
        const auto candidate=rows->mediaSubmission(id);
        if(!candidate || *candidate!=*key)continue;
        if(!match.isEmpty())return {{"action","ambiguous"}};
        match=rows->details(id);
    }
    for(const auto &value:library->remote()->jobs()) {
        const auto job=value.toMap();
        const auto jobKey=SubmissionKey::decode(QJsonObject::fromVariantMap(job.value("submission").toMap()));
        if(!jobKey || *jobKey!=*key || job.value("profileId").toString()!=library->viewedProfileId())continue;
        const auto state=job.value("state").toString();
        if(QStringList{"failed","cancelled","abandoned","paused","installed"}.contains(state) || job.value("added").toBool())continue;
        const auto p=job.value("progress").toMap();
        const auto total=p.value("total").toDouble();
        return {{"action","progress"},{"percent",total>0?int(100*p.value("bytesWritten").toDouble()/total):-1}};
    }
    if(match.isEmpty())return {{"action","download"},{"enabled",library->remote()->canResolve()}};
    const auto id=match.value("id").toString();
    if(match.value("update").toMap().value("status")=="available")
        return {{"action","update"},{"id",id},{"enabled",library->remote()->replacementAvailable()
                && !library->remote()->replacementActive() && library->remoteAdmissionAvailable()}};
    QString profileName;
    for (const auto &value : library->profiles()) {
        const auto profile = value.toMap();
        if (profile.value("id").toString() == library->viewedProfileId()) { profileName = profile.value("name").toString(); break; }
    }
    const bool enabled=match.value("enabled").toBool();
    return {{"action",enabled?"disable":"enable"},{"id",id},{"profileName",profileName},
            {"enabled",library->canToggleEnabled() && (enabled || match.value("selectionReady").toBool())}};
}
bool CatalogController::runLibraryAction(const QVariantMap &submission) {
    auto *library=qobject_cast<LibraryModel *>(parent());
    const auto action=libraryAction(submission);
    if(!library || !action.value("enabled").toBool())return false;
    const auto kind=action.value("action").toString(),id=action.value("id").toString();
    if(kind=="update")return library->remote()->beginCatalogUpdate(id);
    if(kind=="enable" || kind=="disable")return library->setModEnabled(id,kind=="enable");
    return false;
}
bool CatalogController::available() const {return m_client->isRunning()&&!m_client->retiring()&&!m_client->localSession()&&m_client->readyForRequests();}
bool CatalogController::busy() const {return (m_phase=="opening"||m_phase=="closing")||std::any_of(m_receipts.cbegin(),m_receipts.cend(),[this](const Receipt &r){return !r.terminal&&!r.obsolete&&current(r);});}
QString CatalogController::clearCacheReason() const {
    if(m_shutdown || !available())return tr("Connect the catalog from Browse before clearing its cache.");
    if(m_phase!="ready" || m_session.isEmpty() || m_root!=m_client->nativeRoot() || !m_client->catalogOwned())return tr("Wait for the catalog to connect.");
    if(m_client->catalogPaused())return tr("Wait for the current library change to finish.");
    if(busy() || m_receipts.size()>=8 || std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return !r.terminal;}))return tr("Wait for the current catalog requests to finish.");
    return {};
}
void CatalogController::setHideNsfw(bool value){
    if(m_hideNsfw==value)return;
    m_hideNsfw=value;
    invalidateHome(true,true);
    if(m_homeActive&&!m_active&&!m_shutdown&&available())QMetaObject::invokeMethod(this,&CatalogController::advanceHomeFeed,Qt::QueuedConnection);
    for(auto &context:m_history){context.query.hideNsfw=value;if(value)context.query.nsfwOnly=false;}
    auto next=m_context;next.query.hideNsfw=value;if(value)next.query.nsfwOnly=false;
    if(next.route=="browse")transition(next,false);
    else {m_context=std::move(next);emit changed();}
}
bool CatalogController::favoriteBusy() const {return std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return r.identity.kind=="favoriteSet"||r.identity.kind=="favoritesGet";});}
QString CatalogController::calculateWorkNotice() const {
    QStringList notices;
    if(std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return r.userCancelled;}))
        notices.append(tr("Finishing cancellation…"));
    // Only publish a pause while the poll can observe its end.
    if(available()&&m_poll.isActive()&&m_client->catalogPaused()&&(busy()||!m_receipts.isEmpty()||(m_active&&m_readNeeded)))
        notices.append(tr("Catalog is waiting for other work to finish."));
    if(std::any_of(m_delayedControls.cbegin(),m_delayedControls.cend(),[this](const DelayedControl &d){return m_client->catalogControlPending(d.call,d.key);}))
        notices.append(tr("A catalog reply is taking longer than expected; the request is still pending."));
    return notices.join("\n");
}
void CatalogController::updateWorkNotice(){
    const auto notice=calculateWorkNotice();
    if(m_workNotice==notice)return;
    m_workNotice=notice;emit workNoticeChanged();
}
bool CatalogController::clearDelayedControl(BackendClient::Call call,const QString &key){
    const auto count=m_delayedControls.size();
    m_delayedControls.erase(std::remove_if(m_delayedControls.begin(),m_delayedControls.end(),[&](const DelayedControl &d){return d.call==call&&d.key==key;}),m_delayedControls.end());
    return m_delayedControls.size()!=count;
}
QString CatalogController::prerequisite() const {
    if(m_shutdown)return tr("Catalog is closing.");
    if(!m_client->isRunning()||!m_client->readyForRequests()||m_client->retiring())return tr("Browse is disconnected. Reconnect from Library.");
    return {};
}
QString CatalogController::searchPrerequisite() const {
    const auto state=m_provenance.value("searchState").toString();
    if(state=="unavailable")return tr("Submission and author search require SQLite FTS5 in the catalog helper. Browsing and collection name/author search remain available.");
    if(state=="rebuilding")return tr("Submission and author search are waiting for the catalog’s local FTS index to finish rebuilding.");
    return {};
}
QVariantMap CatalogController::queryState() const {auto q=m_context.query.json().toVariantMap();q.insert("kind",m_context.query.type=="collections"?"collections":m_context.query.map.value_or(false)?"maps":m_context.query.type);return q;}
QString CatalogController::provenanceText() const {
    if(m_provenance.isEmpty())return {};
    const auto date=[&](const QVariant &v){return QLocale().toString(QDateTime::fromSecsSinceEpoch(v.toLongLong()).toLocalTime(),QLocale::ShortFormat);};
    QStringList lines;
    const auto completed=lastCompletedAt();
    const auto p=syncProgress();
    if(completed.isValid()&&!completed.isNull())lines.append(tr("Full mod list last loaded: %1").arg(date(completed)));
    else if(!p.isEmpty()&&!p.value("total").isNull())lines.append(tr("Full mod list still loading: %1 of %2 so far. Newest mods are loaded first.").arg(p.value("completed").toString(),p.value("total").toString()));
    else lines.append(tr("Full mod list still loading. Newest mods are loaded first."));
    const auto checked=m_provenance.value("lastSuccessfulSyncAt");
    if(checked.isValid()&&!checked.isNull())lines.append(tr("Last checked for updates: %1").arg(date(checked)));
    if(m_provenance.value("stale").toBool())lines.append(tr("Saved results may be out of date. Refresh to check again."));
    const auto failure=m_provenance.value("refreshError").toMap();
    if(!failure.isEmpty())lines.append(tr("Last refresh didn't finish: %1").arg(failure.value("message").toString()));
    return lines.join("\n");
}
bool CatalogController::hasNext() const {bool ok=false;const auto count=m_total.toULongLong(&ok);return ok&&quint64(m_context.page+1)*quint64(m_context.query.type=="collections"||m_context.route=="collection"?50:m_context.query.pageSize)<count;}
bool CatalogController::current(const Receipt &r) const {return !r.obsolete&&!r.isHome()&&(r.identity.generation==QString::number(m_generation)||(r.identity.generation=="0"&&r.identity.kind=="sync"&&r.identity.args.value("clearCache")==QJsonValue(true)))&&r.identity.root==m_root&&r.identity.session==m_session;}
void CatalogController::deferPassiveOpen(){
    m_passiveOpenDeadline=m_pollClock.elapsed()+m_passiveOpenDelay;
    m_passiveOpenDelay=m_passiveOpenDelay==2000?5000:15000;
    if(!m_shutdown&&available())m_passiveOpenRetry.start(int(m_passiveOpenDeadline-m_pollClock.elapsed()));
}
bool CatalogController::passiveOpenDeferred(){
    const auto remaining=m_passiveOpenDeadline-m_pollClock.elapsed();
    if(remaining<=0)return false;
    if(!m_passiveOpenRetry.isActive())m_passiveOpenRetry.start(int(remaining));
    return true;
}
void CatalogController::loseSession(){
    if(m_shutdown||m_client->retiring()||!m_client->isRunning())m_passiveOpenRetry.stop();
    const bool uncertain=std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return r.identity.kind=="favoriteSet";});
    if((m_active||uncertain)&&(!m_session.isEmpty()||!m_receipts.isEmpty())){
        setError(uncertain?tr("The favorite outcome needs an authoritative read after reconnecting. No change will be replayed."):tr("Catalog connection ended. Reconnect to refresh."),
                 uncertain?tr("The favorite change needs checking. Reconnect before trying again."):tr("Browse is disconnected. Reconnect from Library."));
    }
    const auto keys=m_receipts.keys();for(const auto &key:keys)detachMediaControls(key);
    m_receipts.clear();pruneDetachedLibraryControls();pruneDetachedMediaInvalidations();m_delayedControls.clear();m_pausedNotice=false;m_mediaDeferred=false;m_homeDeferred=false;m_session.clear();m_root.clear();m_phase.clear();m_preferencesRevision.clear();m_keys=0;m_closeSent=false;m_reopen=false;m_readNeeded=true;m_currentPage.reset();m_adjacent.reset();m_deferredFavorite.reset();m_selectedReconcileRevision.clear();m_libraryOpenKey.clear();m_libraryOpenRoot.clear();m_passiveOpening=false;invalidateSelectedFavorite();m_model->clearFavorites();
    invalidateHome(true,false);
    m_poll.stop();
}
void CatalogController::setActive(bool value){
    if(m_active==value)return;
    m_active=value;
    if(value){
        m_autoAttempted=false;
        cancelHomeReceipts();
    }else{
        cancelView(m_generation);m_resource.clear();m_resourceKind.clear();m_readNeeded=true;
        if(m_homeActive&&m_homeUnavailable&&m_phase=="ready"){m_homeUnavailable=false;m_homeLatestDone=!m_homeLatest.isEmpty();m_homePopularDone=!m_homePopular.isEmpty();}
    }
    if(value&&!m_shutdown){m_poll.start();session();}
    else if(!value&&m_homeActive&&!m_shutdown&&available())QMetaObject::invokeMethod(this,&CatalogController::advanceHomeFeed,Qt::QueuedConnection);
    emit changed();
    emit homeChanged();
}
void CatalogController::setHomeActive(bool value){
    if(m_homeActive==value)return;
    m_homeActive=value;
    if(!value){
        cancelHomeReceipts();
        m_homeDeferred=false;
        emit homeChanged();
        return;
    }
    cancelHomeReceipts();
    invalidateHome(true,true);
    if(!m_active&&!m_shutdown)QMetaObject::invokeMethod(this,&CatalogController::advanceHomeFeed,Qt::QueuedConnection);
}
bool CatalogController::homeLoading() const {
    if(!m_homeActive||m_active||m_shutdown||m_homeUnavailable||!available()||m_phase=="failed")return false;
    if(m_phase=="opening"&&m_passiveOpening)return true;
    if(std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return r.isHome()&&!r.terminal&&!r.obsolete;}))return true;
    return !m_homeLatestDone||!m_homePopularDone;
}
void CatalogController::cancelHomeReceipts(){
    bool any=false;
    for(auto &r:m_receipts)if(r.isHome()&&!r.terminal&&!r.obsolete){r.obsolete=true;any=true;}
    if(any){++m_homeEpoch;m_poll.start();}
}
void CatalogController::invalidateHome(bool clearResults,bool resetUnavailable){
    ++m_homeEpoch;
    for(auto &r:m_receipts)if(r.isHome()&&!r.terminal)r.obsolete=true;
    m_homeLatestDone=false;m_homePopularDone=false;m_homeDeferred=false;
    if(resetUnavailable)m_homeUnavailable=false;
    if(clearResults){m_homeLatest.clear();m_homePopular.clear();m_homeProvenance.clear();}
    emit homeChanged();
}
void CatalogController::advanceHomeFeed(){
    if(!m_homeActive||m_active||m_shutdown||m_reopen||m_homeUnavailable||!available())return;
    if(m_homeLatestDone&&m_homePopularDone)return;
    if(m_client->catalogPaused()){m_homeDeferred=true;schedulePoll();return;}
    if(m_session.isEmpty()||m_phase=="opening"||m_phase=="closing"){
        if(!m_active&&m_phase=="failed"&&m_session.isEmpty()&&!m_client->catalogOwned())m_phase.clear();
        if(!m_active&&m_phase=="failed"){m_reopen=true;m_poll.start();return;}
        if(passiveOpenDeferred())return;
        if(m_session.isEmpty()&&!m_client->catalogControlPending(BackendClient::Call::CatalogOpen)){
            m_passiveOpening=true;
            session();
        }
        return;
    }
    if(m_phase!="ready"||m_root!=m_client->nativeRoot()||m_receipts.size()>=8)return;
    if(std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return (r.isHome()||r.mediaKey.isEmpty())&&!r.terminal;}))return;
    const QString slot=!m_homeLatestDone?QStringLiteral("latest"):QStringLiteral("popular");
    CatalogQuery q;
    q.type=QStringLiteral("mod");
    q.sort=slot==QLatin1String("latest")?QStringLiteral("releaseDate"):QStringLiteral("downloadCount");
    q.hideNsfw=m_hideNsfw;
    q.hideObsolete=false;
    q.page=0;
    q.pageSize=5;
    const auto args=q.json();
    if(!CatalogData::operation(QStringLiteral("query"),args))return;
    pruneDetachedLibraryControls();pruneDetachedMediaInvalidations();
    QString wireKey;
    do{wireKey=token();}while(m_receipts.contains(wireKey)||m_detachedLibraryControls.contains(wireKey)||m_detachedMediaInvalidations.contains(wireKey));
    Receipt r;
    r.identity={46,m_root,m_session,wireKey,QStringLiteral("0"),{},QStringLiteral("query"),args,true};
    r.homeSlot=slot;
    r.homeEpoch=m_homeEpoch;
    r.homeNsfw=m_hideNsfw;
    r.preferenceFloor=m_preferencesRevision;
    r.nextPoll=m_pollClock.elapsed();
    r.fastUntil=r.nextPoll+1000;
    m_receipts.insert(r.identity.key,r);
    pulse();
}
void CatalogController::session(){if(m_shutdown||!available()||(!m_active&&passiveOpenDeferred()))return;m_poll.start();if(m_session.isEmpty()&&!m_client->catalogControlPending(BackendClient::Call::CatalogOpen)){
    m_startupRoot=m_client->nativeRoot();m_phase="opening";m_status=tr("Opening independent catalog cache…");control(BackendClient::Call::CatalogOpen);
}}
bool CatalogController::requestLibraryMedia(const QString &key,const SubmissionKey &submission,bool refresh,const QString &kind){
    pruneDetachedLibraryControls();pruneDetachedMediaInvalidations();
    if(key.isEmpty()||key.toUtf8().size()>128||!submission.valid()||m_shutdown||m_reopen||!available())return false;
    if(m_client->catalogPaused()){m_mediaDeferred=true;schedulePoll();return false;}
    if(!m_active&&m_phase=="failed"&&m_session.isEmpty()&&!m_client->catalogOwned())m_phase.clear();
    if(!m_active&&m_phase=="failed"){
        // Preserve Browse's error until it leaves the view. Recycle through the
        // owned-session close path, without notifying consumers synchronously.
        m_reopen=true;m_poll.start();return false;
    }
    if(m_session.isEmpty()&&!m_active&&passiveOpenDeferred())return false;
    if(m_session.isEmpty()||m_phase=="opening"){
        m_libraryOpenKey=key;m_libraryOpenRoot=m_client->nativeRoot();
        if(m_session.isEmpty()&&!m_client->catalogControlPending(BackendClient::Call::CatalogOpen)){
            m_passiveOpening=!m_active;
            // Backend admission can reject synchronously. Deliver that outcome
            // after the caller has returned with its read still unadmitted.
            QMetaObject::invokeMethod(this,&CatalogController::session,Qt::QueuedConnection);
        }
        return false;
    }
    if(m_phase!="ready"||m_root!=m_client->nativeRoot()||m_client->catalogPaused()||m_receipts.size()>=8)return false;
    if(std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return !r.mediaKey.isEmpty();}))return false;
    if((m_active&&m_readNeeded)||std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return r.mediaKey.isEmpty()&&!r.isHome()&&!r.terminal;}))return false;
    // Terminal operation evidence is not receipt release or physical callback drain.
    if(std::any_of(m_receipts.cbegin(),m_receipts.cend(),[this](const Receipt &r){return r.mediaInvalidated&&r.sent&&r.identity.root==m_root&&r.identity.session==m_session;})
        ||std::any_of(m_detachedMediaInvalidations.cbegin(),m_detachedMediaInvalidations.cend(),[this](const CatalogRequestIdentity &i){return i.root==m_root&&i.session==m_session;}))return false;
    QJsonObject args{{"submission",submission.json()},{"refresh",refresh}};
    if(kind!="detail")args.insert("page",1);
    if(!QStringList{"detail","files","changelog"}.contains(kind)||!CatalogData::operation(kind,args))return false;
    QString wireKey;do{wireKey=token();}while(m_receipts.contains(wireKey)||m_detachedLibraryControls.contains(wireKey)||m_detachedMediaInvalidations.contains(wireKey));
    Receipt r;r.identity={46,m_root,m_session,wireKey,QStringLiteral("0"),{},kind,args,true};r.mediaKey=key;
    // Cache-only reads are short worker operations. Poll them promptly;
    // online refreshes retain the normal cadence. Admission itself needs no tick.
    r.nextPoll=m_pollClock.elapsed();
    if(!refresh)r.fastUntil=r.nextPoll+1000;
    m_receipts.insert(r.identity.key,r);pulse();return true;
}
void CatalogController::cancelLibraryMedia(const QString &key){
    if(m_libraryOpenKey==key){m_libraryOpenKey.clear();m_libraryOpenRoot.clear();}
    for(auto &r:m_receipts)if(r.mediaKey==key&&!r.terminal)r.obsolete=true;
    m_poll.start();
}
void CatalogController::preemptLibraryMedia(){
    for(auto &r:m_receipts)if(!r.mediaKey.isEmpty()&&!r.terminal&&!r.obsolete){
        r.obsolete=true;r.mediaReported=true;
        CatalogDecodedResult result;result.phase="cancelled";result.errorKind="libraryMediaPreempted";result.root=r.identity.root;result.session=r.identity.session;
        emit libraryMediaReady(r.mediaKey,r.identity.root,result);
    }
}
void CatalogController::failLibraryOpen(const QString &kind,const QString &message){
    const auto key=std::exchange(m_libraryOpenKey,{}),root=std::exchange(m_libraryOpenRoot,{});
    if(!key.isEmpty()){CatalogDecodedResult result;result.phase="failed";result.errorKind=kind;result.error=message;result.root=root;result.session=m_session;emit libraryMediaReady(key,root,result);}
}
bool CatalogController::libraryControlPending(const QString &key) const {
    using C=BackendClient::Call;
    return m_client->catalogControlPending(C::CatalogBegin,key)||m_client->catalogControlPending(C::CatalogStatus,key)
        ||m_client->catalogControlPending(C::CatalogCancel,key)||m_client->catalogControlPending(C::CatalogRelease,key);
}
void CatalogController::detachMediaControls(const QString &key){
    pruneDetachedLibraryControls();pruneDetachedMediaInvalidations();
    const auto it=m_receipts.constFind(key);
    if(it!=m_receipts.cend()&&!it->mediaKey.isEmpty()&&libraryControlPending(key))m_detachedLibraryControls.insert(key,it->identity);
    // Once the receipt is gone, retain only identities with an actual control still pending.
    if(it!=m_receipts.cend()&&it->mediaInvalidated&&it->sent&&libraryControlPending(key))m_detachedMediaInvalidations.insert(key,it->identity);
}
void CatalogController::pruneDetachedLibraryControls(){
    for(auto it=m_detachedLibraryControls.begin();it!=m_detachedLibraryControls.end();)
        if(!libraryControlPending(it.key()))it=m_detachedLibraryControls.erase(it);else ++it;
}
void CatalogController::pruneDetachedMediaInvalidations(){
    bool releasedCurrent=false;
    for(auto it=m_detachedMediaInvalidations.begin();it!=m_detachedMediaInvalidations.end();){
        if(libraryControlPending(it.key())){++it;continue;}
        releasedCurrent|=it->root==m_root&&it->session==m_session;
        it=m_detachedMediaInvalidations.erase(it);
    }
    if(releasedCurrent&&!m_mediaDrainWakeScheduled){
        m_mediaDrainWakeScheduled=true;
        QTimer::singleShot(0,this,[this]{m_mediaDrainWakeScheduled=false;emit changed();});
    }
}
void CatalogController::control(BackendClient::Call call,const QString &key){
    if(m_client->catalogControlPending(call,key))return;
    CatalogRequestIdentity i; i.call=int(call);i.root=m_root;i.rootBound=!m_root.isEmpty();i.session=m_session;i.key=key;
    QJsonObject p;
    if(call!=BackendClient::Call::CatalogOpen)p.insert("catalogSession",m_session);
    if(call==BackendClient::Call::CatalogStatus||call==BackendClient::Call::CatalogCancel||call==BackendClient::Call::CatalogRelease)p.insert("requestKey",key.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(key));
    if(!key.isEmpty()&&m_receipts.contains(key)){i=m_receipts.value(key).identity;i.call=int(call);i.digest=m_receipts.value(key).digest;}
    m_client->nativeRequest(call,p,{},i);
}
void CatalogController::begin(const QString &kind,const QJsonObject &args,bool rootWide){
    if(m_shutdown||m_phase!="ready"||!CatalogData::operation(kind,args))return;
    preemptLibraryMedia();
    for(const auto &r:std::as_const(m_receipts))if(!r.terminal&&!r.obsolete&&r.identity.kind==kind&&r.identity.args==args&&(rootWide||current(r))&&(kind!="favoritesGet"||r.preferenceFloor==m_preferencesRevision))return;
    if(m_receipts.size()>=8){m_status=tr("Waiting for retained catalog receipts to be released.");m_readNeeded=true;emit changed();return;}
    pruneDetachedLibraryControls();pruneDetachedMediaInvalidations();QString wireKey;do{wireKey=token();}while(m_receipts.contains(wireKey)||m_detachedLibraryControls.contains(wireKey)||m_detachedMediaInvalidations.contains(wireKey));
    Receipt r;r.identity={46,m_root,m_session,wireKey,rootWide?QStringLiteral("0"):QString::number(m_generation),{},kind,args,true};
    r.preferenceFloor=m_preferencesRevision;
    m_receipts.insert(r.identity.key,r);m_poll.start();
}
int CatalogController::pollCadence(const Receipt &r) const {
    return r.fastUntil>m_pollClock.elapsed()?20:250;
}
void CatalogController::schedulePoll(){
    if((m_shutdown&&!m_client->catalogOwned())||m_client->retiring()||!m_client->isRunning()){
        m_poll.stop();return;
    }
    const bool sessionWork=m_shutdown||m_reopen||m_phase=="opening"||m_phase=="closing";
    if(m_receipts.isEmpty()&&!sessionWork&&!(m_active&&m_readNeeded)&&!m_client->catalogPaused()&&!m_mediaDeferred&&!m_homeDeferred){
        m_poll.stop();return;
    }
    int delay=250;
    if(!sessionWork&&!m_client->catalogPaused()){
        const auto now=m_pollClock.elapsed();
        for(const auto &r:std::as_const(m_receipts))delay=std::min(delay,int(std::clamp<qint64>(r.nextPoll-now,1,250)));
    }
    m_poll.start(delay);
}
void CatalogController::pulse(){
    const auto reschedule=qScopeGuard([this]{schedulePoll();});
    using C=BackendClient::Call;
    updateWorkNotice();
    if(m_client->retiring()||!m_client->isRunning())return;
    if(!available())return;
    const bool paused=m_client->catalogPaused();
    if(paused){
        if(!m_active&&m_homeActive)m_homeDeferred=true;
        const auto status=tr("Catalog is waiting for shared Library work; receipts are retained.");
        const bool notify=!m_pausedNotice||m_status!=status;
        m_pausedNotice=true;m_status=status;if(notify)emit changed();return;
    }
    if(m_pausedNotice||m_mediaDeferred||m_homeDeferred){m_pausedNotice=false;m_mediaDeferred=false;m_homeDeferred=false;emit changed();emit homeChanged();}
    if(m_shutdown||m_reopen){
        if(m_session.isEmpty()){
            if(!m_client->catalogOwned()){if(m_reopen&&!m_shutdown){m_reopen=false;if(m_phase=="failed")m_phase.clear();if(m_active)session();else if(m_homeActive&&!m_homeUnavailable)advanceHomeFeed();else emit changed();}return;}
            // An open with a lost acknowledgment is still owned. Idempotent open
            // recovers that same session solely so close can drain it.
            if(m_active||m_shutdown||!passiveOpenDeferred())control(C::CatalogOpen);return;
        }
        if(!m_closeSent&&!m_client->catalogControlPending(C::CatalogClose)){m_closeSent=true;control(C::CatalogClose);}
        else if(m_phase=="closing")control(C::CatalogStatus);
        return;
    }
    if(m_session.isEmpty()){if(m_active)session();else if(m_homeActive&&!m_homeUnavailable)advanceHomeFeed();return;}
    if(m_phase=="opening"||m_phase=="closing"){control(C::CatalogStatus);return;}
    if(m_phase!="ready")return;
    const auto receiptKeys=m_receipts.keys();
    bool cancelledReceiptRemoved=false;
    for(const auto &key:receiptKeys){
        if(!m_receipts.contains(key))continue;
        const auto r=m_receipts.value(key);
        if(m_pollClock.elapsed()<r.nextPoll)continue;
        m_receipts[key].nextPoll=m_pollClock.elapsed()+pollCadence(r);
        if(r.terminal){control(C::CatalogRelease,key);continue;}
        if(r.obsolete&&r.admitted&&!r.cancelled){m_receipts[key].cancelled=true;control(C::CatalogCancel,key);continue;}
        if(!r.admitted){
            if(r.obsolete&&!r.sent){m_receipts.remove(key);cancelledReceiptRemoved=true;continue;}
            if(m_client->catalogControlPending(C::CatalogBegin,key))continue;
            QJsonObject p{{"catalogSession",m_session},{"requestKey",key},{"viewGeneration",r.identity.generation},{"operation",QJsonObject{{"kind",r.identity.kind},{"args",r.identity.args}}}};
            if(r.mediaKey.isEmpty()&&!r.isHome()&&!r.mediaInvalidated){
                if(r.identity.kind=="sync"){
                    m_receipts[key].mediaInvalidated=true;emit libraryMediaInvalidated(r.identity.root,r.identity.session,{});
                }else if(QStringList{"detail","files","comments","changelog"}.contains(r.identity.kind)&&r.identity.args["refresh"]==QJsonValue(true)){
                    const auto submission=SubmissionKey::decode(r.identity.args["submission"]);
                    if(submission){m_receipts[key].mediaInvalidated=true;emit libraryMediaInvalidated(r.identity.root,r.identity.session,submission->identity());}
                }
            }
            if(!m_receipts.contains(key)||m_receipts.value(key).obsolete||m_shutdown||m_reopen)continue;
            m_receipts[key].sent=true;
            m_client->nativeRequest(C::CatalogBegin,p,{},r.identity);
        }else control(C::CatalogStatus,key);
    }
    if(m_deferredFavorite&&m_receipts.size()<8){const auto key=*m_deferredFavorite;m_deferredFavorite.reset();reconcileFavorite(key);}
    if(m_active)reconcileSelectedFavorite();
    if(m_readNeeded&&m_active){m_readNeeded=false;readCurrent();}
    if(!m_active&&m_homeActive&&!m_homeUnavailable&&(!m_homeLatestDone||!m_homePopularDone)
        &&!std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &r){return (r.isHome()||r.mediaKey.isEmpty())&&!r.terminal;}))
        advanceHomeFeed();
    if(cancelledReceiptRemoved){emit changed();emit homeChanged();}
}
void CatalogController::received(BackendClient::Call call,const CatalogDecodedResult &r){
    const auto reschedule=qScopeGuard([this]{schedulePoll();});
    using C=BackendClient::Call;
    pruneDetachedMediaInvalidations();
    const bool delayCleared=clearDelayedControl(call,r.key);
    if(call==C::CatalogOpen||(call==C::CatalogStatus&&r.key.isEmpty())){
        if(!m_session.isEmpty()&&m_session!=r.session){if(delayCleared)emit changed();return;}
        m_session=r.session;m_phase=r.phase;
        if(r.phase=="ready"){
            m_passiveOpenRetry.stop();m_passiveOpenDeadline=0;m_passiveOpenDelay=2000;
            if(!m_root.isEmpty()&&m_root!=r.root){invalidateHome(true,true);setError(tr("Catalog root changed; closing its session before further work."));m_reopen=true;pulse();if(delayCleared)emit changed();return;}
            m_root=r.root;m_status=tr("Catalog cache ready.");m_libraryOpenKey.clear();m_libraryOpenRoot.clear();m_passiveOpening=false;
            if(!m_shutdown&&!m_reopen&&m_active&&m_readNeeded){m_readNeeded=false;readCurrent();}
            else if(!m_shutdown&&!m_reopen&&!m_active&&m_homeActive){m_homeUnavailable=false;advanceHomeFeed();}
            emit homeChanged();
        }else if(r.phase=="failed"){
            const bool wasPassive=m_passiveOpening&&!m_active;
            deferPassiveOpen();failLibraryOpen(r.errorKind,r.error);m_passiveOpening=false;
            if(!wasPassive){setError(r.error);m_status=tr("Catalog unavailable (%1). Library remains usable.").arg(r.errorKind);}
            if(!m_active){
                m_reopen=true;m_poll.start();
                if(m_homeActive){m_homeUnavailable=true;emit homeChanged();}
            }
        }
        if(r.phase=="closed"){
            const bool reopen=m_reopen;loseSession();m_phase="closed";
            if(reopen&&!m_shutdown){m_phase.clear();if(m_active)session();else if(m_homeActive&&!m_homeUnavailable)advanceHomeFeed();}
        }
        emit changed();return;
    }
    if(call==C::CatalogClose){if(r.session!=m_session){if(delayCleared)emit changed();return;}m_phase=r.phase;if(r.phase=="closed"){const bool reopen=m_reopen;loseSession();m_phase="closed";if(reopen&&!m_shutdown){m_phase.clear();if(m_active)session();else if(m_homeActive&&!m_homeUnavailable)advanceHomeFeed();}}emit changed();return;}
    if(!m_receipts.contains(r.key)||r.session!=m_session){if(m_detachedLibraryControls.contains(r.key))pruneDetachedLibraryControls();if(delayCleared)emit changed();return;}
    if(call==C::CatalogRelease){
        detachMediaControls(r.key);m_receipts.remove(r.key);
        if(m_keys>=500&&m_receipts.isEmpty()){m_reopen=true;m_readNeeded=true;}
        else if(!m_shutdown&&!m_reopen&&!m_active&&m_homeActive&&!m_homeUnavailable)advanceHomeFeed();
        emit changed();emit homeChanged();return;
    }
    if(call==C::CatalogCancel){
        if(r.state=="released"){
            detachMediaControls(r.key);m_receipts.remove(r.key);
            if(!m_shutdown&&!m_reopen&&!m_active&&m_homeActive&&!m_homeUnavailable)advanceHomeFeed();
        }
        emit changed();emit homeChanged();return;
    }
    if(call==C::CatalogBegin){auto &receipt=m_receipts[r.key];receipt.admitted=true;receipt.digest=r.digest;++m_keys;if(receipt.obsolete){if(!receipt.cancelled){receipt.cancelled=true;control(C::CatalogCancel,r.key);}}else if(receipt.fastUntil>m_pollClock.elapsed()){receipt.nextPoll=m_pollClock.elapsed()+pollCadence(receipt);control(C::CatalogStatus,r.key);}if(receipt.isHome())emit homeChanged();else emit changed();return;}
    auto receipt=m_receipts.value(r.key);
    if(r.phase=="released"){
        detachMediaControls(r.key);m_receipts.remove(r.key);
        if(!m_shutdown&&!m_reopen&&!m_active&&m_homeActive&&!m_homeUnavailable)advanceHomeFeed();
        emit changed();emit homeChanged();return;
    }
    m_receipts[r.key].admitted=true;
    if(!receipt.mediaKey.isEmpty()){
        if(r.phase=="succeeded"||r.phase=="failed"||r.phase=="cancelled"){
            m_receipts[r.key].terminal=true;
            if(!receipt.obsolete&&!receipt.mediaReported){m_receipts[r.key].mediaReported=true;emit libraryMediaReady(receipt.mediaKey,receipt.identity.root,r);}
            control(C::CatalogRelease,r.key);
        }
        emit changed();return;
    }
    if(receipt.isHome()){
        if(r.phase=="succeeded"||r.phase=="failed"||r.phase=="cancelled"){
            m_receipts[r.key].terminal=true;
            const bool currentHome=!receipt.obsolete&&m_homeActive&&!m_active&&!m_shutdown&&!m_reopen
                &&receipt.identity.root==m_root&&receipt.identity.session==m_session
                &&receipt.homeEpoch==m_homeEpoch&&receipt.homeNsfw==m_hideNsfw;
            if(currentHome){
                if(r.phase=="succeeded"){
                    QVariantList rows;
                    const int count=std::min<int>(5,r.rows.size());
                    rows.reserve(count);
                    for(int i=0;i<count;++i)rows.append(r.rows[i].metadata);
                    if(receipt.homeSlot==QLatin1String("latest")){
                        m_homeLatest=rows;
                        m_homeLatestDone=true;

                    }else if(receipt.homeSlot==QLatin1String("popular")){
                        m_homePopular=rows;
                        m_homePopularDone=true;

                    }
                    m_homeUnavailable=false;
                    m_homeProvenance=r.provenance;
                }else{
                    // One facade owns both shelves and their provenance. Fail
                    // the whole read instead of displaying mixed snapshots.
                    invalidateHome(true,false);
                    m_homeLatestDone=true;m_homePopularDone=true;m_homeUnavailable=true;
                }
            }
            control(C::CatalogRelease,r.key);
            if(currentHome){
                emit homeChanged();
                advanceHomeFeed();
            }
        }
        return;
    }
    if(!r.progress.isEmpty()&&current(receipt)){
        if(r.kind=="sync")m_syncProgress=r.progress;
        m_status=r.progress.value("total").isNull()?tr("%1: %2 completed").arg(r.progress.value("stage").toString(),r.progress.value("completed").toString()):tr("%1: %2 of %3").arg(r.progress.value("stage").toString(),r.progress.value("completed").toString(),r.progress.value("total").toString());
    }else if(current(receipt))m_status=r.phase=="queued"?tr("Catalog request queued."):r.phase=="running"?tr("Reading catalog metadata…"):m_status;
    if(r.phase=="succeeded"||r.phase=="failed"||r.phase=="cancelled"){
        m_receipts[r.key].terminal=true;
        if(current(receipt)&&QStringList{"detail","files","comments","changelog"}.contains(r.kind)){
            if(r.phase=="succeeded"&&(r.kind=="detail"||r.state=="unavailable"))emit libraryMediaEvidence(receipt.identity.root,r);
            else if(r.kind=="detail"&&r.phase=="failed"&&(r.errorKind=="catalogPrivate"||r.errorKind=="catalogNotFound")){
                auto evidence=r;const auto submission=SubmissionKey::decode(receipt.identity.args["submission"]);
                if(submission){evidence.submission=*submission;emit libraryMediaEvidence(receipt.identity.root,evidence);}
            }
        }
        if(r.phase=="succeeded"){
            if(r.kind=="favoriteSet"||r.kind=="favoritesGet"){
                if(!acceptPreferencesRevision(r.preferencesRevision)){
                    // Older cross-key evidence cannot repair a newer projection.
                    // Reconcile only the visible selection, once per observed floor.
                    reconcileSelectedFavorite();
                    control(C::CatalogRelease,r.key);emit changed();return;
                }
                if(r.favoriteKnown){
                    if(m_context.route=="submission"&&m_context.submission==r.submission)
                        m_selectedFavorite=SelectedFavorite{r.submission,r.favorite,r.preferencesRevision};
                    m_model->favorite(r.submission,r.favorite,r.preferencesRevision);
                    auto summary=m_detail.value("summary").toMap();
                    if(summary.value("identity").toString()==r.submission.identity()){
                        summary.insert("favoriteKnown",true);summary.insert("favorite",r.favorite);
                        summary.insert("favoriteRevision",r.preferencesRevision);m_detail.insert("summary",summary);
                    }
                }
                reconcileSelectedFavorite();
                if(r.kind=="favoriteSet"){m_status=tr("Favorite saved with authoritative revision %1.").arg(r.preferencesRevision);if(m_context.route=="browse"&&m_context.query.favoritesOnly){m_readNeeded=true;}}
            }else if(current(receipt))publish(r);
        }else if(current(receipt)||r.kind=="favoriteSet"||r.kind=="favoritesGet"){
            if(r.kind=="sync")m_syncProgress.clear();
            if(r.errorKind!="catalogCancelled")setError(tr("%1 (%2)").arg(r.error,r.errorKind),r.kind=="sync"?tr("Couldn't refresh the catalog. Showing saved results."):(r.error.isEmpty()?tr("Couldn't complete the catalog request."):r.error));
            m_status=r.phase=="cancelled"?tr("Catalog request cancelled."):tr("Catalog request failed.");
            if(r.kind=="favoriteSet"){const auto k=SubmissionKey::decode(receipt.identity.args["submission"]);m_preferencesRevision.clear();m_selectedReconcileRevision.clear();invalidateSelectedFavorite();m_model->clearFavorites();if(k)reconcileFavorite(*k);}
        }
        control(C::CatalogRelease,r.key);
    }
    emit changed();
}
void CatalogController::rejected(BackendClient::Call call,const QString &key,const QString &kind,const QString &message){
    const auto reschedule=qScopeGuard([this]{schedulePoll();});
    if((kind=="catalogBusy"||kind=="catalogRequestPending")&&m_receipts.contains(key)){
        auto &receipt=m_receipts[key];
        receipt.fastUntil=0;
        receipt.nextPoll=m_pollClock.elapsed()+250;
    }

    using C=BackendClient::Call;
    const auto detached=m_detachedMediaInvalidations.constFind(key);
    const bool oldInvalidation=detached!=m_detachedMediaInvalidations.cend()&&(detached->root!=m_root||detached->session!=m_session);
    pruneDetachedMediaInvalidations();
    if(oldInvalidation&&!m_receipts.contains(key))return;
    if(m_detachedLibraryControls.contains(key)&&!m_receipts.contains(key)){pruneDetachedLibraryControls();emit changed();return;}
    const bool passive=m_receipts.contains(key)&&!m_receipts.value(key).mediaKey.isEmpty();
    if(passive){
        if(m_client->catalogControlPending(call,key)){emit changed();return;}
        auto receipt=m_receipts.value(key);
        if(kind=="catalogBusy"||kind=="catalogRequestPending"){
            if(call==C::CatalogBegin)m_receipts[key].sent=false;
            if(call==C::CatalogCancel)m_receipts[key].cancelled=false;
        }else{
            if(kind=="catalogUnknownRequest"&&call==C::CatalogCancel&&m_client->catalogControlPending(C::CatalogBegin,key))return;
            if(!receipt.obsolete&&!receipt.mediaReported){CatalogDecodedResult result;result.phase="failed";result.errorKind=kind;result.error=message;result.root=receipt.identity.root;result.session=receipt.identity.session;emit libraryMediaReady(receipt.mediaKey,receipt.identity.root,result);m_receipts[key].mediaReported=true;}
            if(kind=="catalogUnknownRequest"||call==C::CatalogBegin){detachMediaControls(key);m_receipts.remove(key);}
            else {m_receipts[key].obsolete=true;m_reopen=true;}
            if(kind=="catalogSessionExhausted")m_reopen=true;
        }
        emit changed();return;
    }
    const bool passiveHome=m_receipts.contains(key)&&m_receipts.value(key).isHome();
    if(passiveHome){
        if(m_client->catalogControlPending(call,key))return;
        auto receipt=m_receipts.value(key);
        if(kind=="catalogBusy"||kind=="catalogRequestPending"){
            if(call==C::CatalogBegin)m_receipts[key].sent=false;
            if(call==C::CatalogCancel)m_receipts[key].cancelled=false;
            return;
        }
        if(kind=="catalogUnknownRequest"&&call==C::CatalogCancel&&m_client->catalogControlPending(C::CatalogBegin,key))return;
        const bool currentHome=!receipt.obsolete&&m_homeActive&&!m_active&&!m_shutdown&&!m_reopen
            &&receipt.identity.root==m_root&&receipt.identity.session==m_session
            &&receipt.homeEpoch==m_homeEpoch&&receipt.homeNsfw==m_hideNsfw;
        if(currentHome){
            invalidateHome(true,false);
            // Exhaustion is a session recycle, not a permanent feed failure.
            m_homeUnavailable=kind!=QLatin1String("catalogSessionExhausted");
            m_homeLatestDone=m_homeUnavailable;m_homePopularDone=m_homeUnavailable;
        }
        if(kind=="catalogUnknownRequest"||call==C::CatalogBegin)m_receipts.remove(key);
        else {m_receipts[key].obsolete=true;m_reopen=true;}
        if(kind=="catalogSessionExhausted")m_reopen=true;
        if(currentHome){
            emit homeChanged();
            if(!m_reopen&&!m_homeUnavailable)advanceHomeFeed();
        }
        return;
    }
    const bool openingControl=call==C::CatalogOpen||(call==C::CatalogStatus&&key.isEmpty()&&m_phase=="opening");
    const bool passiveOpen=openingControl&&m_passiveOpening&&!m_active;
    if(openingControl&&!m_client->catalogControlPending(call,key)){
        deferPassiveOpen();failLibraryOpen(kind,message);m_passiveOpening=false;m_phase="failed";
        if(!m_active){
            m_reopen=true;m_poll.start();
            if(m_homeActive){m_homeUnavailable=true;emit homeChanged();}
        }
    }
    if(m_client->catalogControlPending(call,key)){
        if(kind=="catalogUnavailable"&&std::none_of(m_delayedControls.cbegin(),m_delayedControls.cend(),[&](const DelayedControl &d){return d.call==call&&d.key==key;}))m_delayedControls.append({call,key});
        if(!(call==C::CatalogOpen&&m_passiveOpening&&!m_active))m_status=message;
        emit changed();return;
    }
    const bool delayCleared=clearDelayedControl(call,key);
    if(kind=="catalogBusy"||kind=="catalogRequestPending"){
        if(call==C::CatalogBegin&&m_receipts.contains(key))m_receipts[key].sent=false;
        if(call==C::CatalogCancel&&m_receipts.contains(key))m_receipts[key].cancelled=false;
        if(call==C::CatalogClose)m_closeSent=false;
        m_status=message;emit changed();return;
    }
    if(m_receipts.contains(key)){
        const auto r=m_receipts.value(key);
        if(kind=="catalogUnknownRequest"){
            if(call==C::CatalogCancel&&m_client->catalogControlPending(C::CatalogBegin,key)){if(delayCleared)emit changed();return;}
            detachMediaControls(key);m_receipts.remove(key);
            if(r.identity.kind=="favoriteSet"){const auto k=SubmissionKey::decode(r.identity.args["submission"]);if(k)reconcileFavorite(*k);}
        }else if(kind=="catalogInvalidEvidence"){
            // No domain value is published. Retain the receipt and cancel it;
            // if its terminal cannot be validated, close only the catalog.
            m_receipts[key].obsolete=true;m_reopen=true;m_readNeeded=false;
        }else if(call==C::CatalogBegin){
            detachMediaControls(key);m_receipts.remove(key);
            if(kind=="catalogSessionExhausted"){m_reopen=true;m_readNeeded=true;}
        }
        if(current(r)||r.identity.kind=="favoriteSet")setError(message);
    }else if(!passiveOpen)setError(message);
    if(call==C::CatalogClose)m_closeSent=false;
    if(call==C::CatalogOpen&&!m_client->catalogOwned())m_phase="failed";
    if(!passiveOpen)m_status=tr("Catalog: %1").arg(kind);
    emit changed();
}
void CatalogController::publish(const CatalogDecodedResult &r){
    if(r.kind=="facets"){m_categories=r.categories;m_heroes=r.heroes;m_heroArtwork=r.heroArtwork;return;}
    if(r.kind=="authors"){m_authors=r.authors;return;}
    if(r.kind=="sync"){m_provenance=r.provenance;m_syncProgress.clear();m_status=refreshMessage();m_readNeeded=true;invalidateHome(true,true);return;}
    if(r.kind=="files"||r.kind=="comments"||r.kind=="changelog"){
        if(r.kind!=m_resourceKind||r.page!=m_resourcePage)return;
        if(r.state=="unavailable"){
            cancelView(m_generation);m_readNeeded=false;m_detail=r.detail;m_resource.clear();m_resourceKind.clear();m_provenance=r.provenance;m_contentReady=true;
            m_status=tr("Provider visibility evidence hides all cached submission resources.");return;
        }
        m_resource=r.detail;m_resource.insert("state",r.state=="ready"?r.detail.value("state",QStringLiteral("ready")):r.state);m_resource.insert("provenance",r.provenance);m_resourceKind=r.kind;
        if(r.refreshRecommended&&!m_resourceAutoAttempted){m_resourceAutoAttempted=true;loadResource(r.kind,r.page>0?r.page:1,true);}return;
    }
    if(r.kind=="query"||r.kind=="collections"||r.kind=="collection"){
        if(!r.preferencesRevision.isEmpty()&&!acceptPreferencesRevision(r.preferencesRevision,true)){m_readNeeded=true;return;}
        if(m_adjacent&&std::abs(m_adjacent->page-r.page)!=1)m_adjacent.reset();
        m_contentReady=true;m_currentPage=r;
        m_model->replace(r.rows,r.provenance,r.preferencesRevision);m_context.page=r.page;
    }
    m_contentReady=true;m_provenance=r.provenance;m_total=r.total;
    if(r.kind=="detail"){
        m_detail=r.detail;
        if(r.state=="unavailable"){m_resource.clear();m_resourceKind.clear();for(auto &pending:m_receipts)if(current(pending)&&QStringList{"files","comments","changelog"}.contains(pending.identity.kind))pending.obsolete=true;}
        auto s=m_detail.value("summary").toMap();
        if(m_selectedFavorite&&m_selectedFavorite->key==r.submission
            &&m_selectedFavorite->revision==m_preferencesRevision&&!s.isEmpty()){
            s.insert("favoriteKnown",true);s.insert("favorite",m_selectedFavorite->value);
            s.insert("favoriteRevision",m_selectedFavorite->revision);m_detail.insert("summary",s);
        }
    }else if(r.kind=="collection")m_detail=r.detail;
    m_status=m_model->count()==0&&(r.kind=="query"||r.kind=="collections")?tr("No matching cached entries. Refresh can populate the catalog."):tr("Metadata ready.");
    const bool recommended=r.refreshRecommended||(r.kind=="query"&&(r.provenance.value("stale").toBool()));
    const bool syncRunning=std::any_of(m_receipts.cbegin(),m_receipts.cend(),[](const Receipt &pending){return !pending.terminal && pending.identity.kind=="sync";});
    if(recommended&&!m_autoAttempted&&!syncRunning){m_autoAttempted=true;
        if(r.kind=="detail")begin("detail",{{"submission",m_context.submission.json()},{"refresh",true}});
        else if(r.kind=="collection")begin("collection",collectionArgs(m_context.collection,m_context.page,true));
        else begin("sync",{{"forceRefresh",false},{"forceReconcile",false}});
    }
}
void CatalogController::readCurrent(){
    if(!m_active||m_shutdown||m_phase!="ready")return;
    m_context.query.hideNsfw=m_hideNsfw;
    if(m_context.route=="submission"){
        begin("detail",{{"submission",m_context.submission.json()},{"refresh",false}});reconcileSelectedFavorite();
    }else if(m_context.route=="collection")begin("collection",collectionArgs(m_context.collection,m_context.page,false));
    else if(m_context.query.type=="collections")begin("collections",{{"search",m_context.query.search},{"hideNsfw",m_context.query.hideNsfw},{"page",m_context.page},{"pageSize",50}});
    else {auto q=m_context.query;q.page=m_context.page;begin("query",q.json());begin("facets",q.scope());}
}
void CatalogController::cancelView(quint64 generation){for(auto it=m_receipts.begin();it!=m_receipts.end();++it)if(it->identity.generation==QString::number(generation)&&it->identity.kind!="favoriteSet"&&it->identity.kind!="favoritesGet")it->obsolete=true;m_poll.start();}
void CatalogController::transition(Context next,bool push){
    next.query.hideNsfw=m_hideNsfw;
    if(m_hideNsfw)next.query.nsfwOnly=false;
    cancelView(m_generation);
    if(push){if(m_history.size()>=7){m_history.removeFirst();if(m_libraryReturnDepth>0)--m_libraryReturnDepth;m_contextNotice=tr("Some older browsing history is unavailable.");}m_history.append(m_context);}
    m_contentReady=false;m_context=std::move(next);++m_generation;m_selectedFavorite.reset();m_selectedReconcileRevision.clear();m_detail.clear();m_resource.clear();m_resourceKind.clear();m_model->replace({},{});m_provenance.clear();m_currentPage.reset();m_adjacent.reset();m_total="0";setError({});m_autoAttempted=false;m_readNeeded=true;
    if(m_active&&m_phase=="ready"){m_readNeeded=false;readCurrent();}else session();emit changed();
}
void CatalogController::query(CatalogQuery q){Context c=m_context;c.route="browse";c.query=std::move(q);c.page=c.query.page;c.scroll=0;c.selected.clear();c.focus="search";transition(c,false);}
void CatalogController::browse(){
    if(m_context.route!="browse"){
        Context listing;
        for(qsizetype n=m_history.size();n>0;--n)if(m_history[n-1].route=="browse"){
            listing=m_history[n-1];
            // Explicit navigation restores this listing, without making the
            // unrelated current detail its Back destination.
            m_history.erase(m_history.begin()+n-1,m_history.end());break;
        }
        m_libraryReturnDepth=std::min(m_libraryReturnDepth,int(m_history.size()));
        transition(listing,false);
    }
    setActive(true);
}
void CatalogController::updateQuery(const QVariantMap &v){
    auto q=m_context.query;
    if(v.contains("kind")){const auto kind=v.value("kind").toString();if(!QStringList{"mod","sound","wip","maps","collections"}.contains(kind))return;q.type=kind=="maps"?"mod":kind;q.map=kind=="maps"?std::optional<bool>(true):std::nullopt;q.audio=kind=="sound"?std::optional<bool>(true):std::nullopt;q.categories.clear();q.heroes.clear();}
    if(v.contains("search"))q.search=v.value("search").toString();if(v.contains("authorRemoteId"))q.author=v.value("authorRemoteId").toString();if(v.contains("categories"))q.categories=v.value("categories").toStringList();if(v.contains("heroes"))q.heroes=v.value("heroes").toStringList();if(v.contains("sort"))q.sort=v.value("sort").toString();
    if(v.contains("excludeFilters"))q.exclude=v.value("excludeFilters").toBool();q.hideNsfw=m_hideNsfw;if(v.contains("nsfwOnly"))q.nsfwOnly=v.value("nsfwOnly").toBool();if(q.hideNsfw)q.nsfwOnly=false;
    if(v.contains("hideObsolete"))q.hideObsolete=v.value("hideObsolete").toBool();if(v.contains("favoritesOnly"))q.favoritesOnly=v.value("favoritesOnly").toBool();if(v.contains("includeWips"))q.includeWips=v.value("includeWips").toBool();
    for(auto name:{"updatedAfter","addedAfter","addedBefore"})if(v.contains(name)){const auto val=v.value(name);std::optional<qint64> t;if(!val.isNull()&&val.isValid()&&!val.toString().isEmpty()){auto date=QDateTime::fromString(val.toString(),Qt::ISODate);if(val.toString().size()==10){const auto day=QDate::fromString(val.toString(),Qt::ISODate);date=QDateTime(day,QTime(0,0),QTimeZone::UTC);}if(!date.isValid()){setError(tr("Enter an ISO date such as 2026-10-28."));emit changed();return;}t=date.toSecsSinceEpoch();}if(QString(name)=="updatedAfter")q.updatedAfter=t;else if(QString(name)=="addedAfter")q.addedAfter=t;else q.addedBefore=t;}
    if(q.json()==m_context.query.json())return;
    q.page=0;
    if(q.type!="collections"&&!CatalogData::operation("query",q.json())){setError(tr("The filters exceed the catalog’s supported bounds or date range."));emit changed();return;}
    if(q.type=="collections"&&!CatalogData::operation("collections",{{"search",q.search},{"hideNsfw",q.hideNsfw},{"page",0},{"pageSize",50}})){setError(tr("Collection search is limited to 512 UTF-8 bytes."));emit changed();return;}
    query(q);
}
void CatalogController::openSubmission(SubmissionKey key){if(!key.valid())return;Context c=m_context;c.route="submission";c.submission=key;c.selected=key.identity();c.focus="back";c.scroll=0;c.page=0;transition(c,true);emit remoteRequested();}
void CatalogController::openSubmission(const QVariantMap &value,bool fromLibrary){const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(value));if(!key)return;if(fromLibrary){
        std::optional<Context> listing;
        if(m_context.route=="browse")listing=m_context;
        else for(qsizetype n=m_history.size();n>0;--n)if(m_history[n-1].route=="browse"){listing=m_history[n-1];break;}
        m_returnLibrary=true;m_history.clear();if(listing)m_history.append(*listing);
        m_libraryReturnDepth=m_history.size();
        Context c=m_context;c.route="submission";c.submission=*key;c.selected=key->identity();c.focus="back";c.scroll=0;c.page=0;transition(c,false);setActive(true);emit remoteRequested();}else {setActive(true);openSubmission(*key);}}
void CatalogController::openCollection(const QString &id){bool ok=false;const auto n=id.toULongLong(&ok);if(!ok||!n||QString::number(n)!=id)return;Context c=m_context;c.route="collection";c.collection=id;c.page=0;c.selected="collection:"+id;c.focus="back";c.scroll=0;transition(c,true);emit remoteRequested();}
void CatalogController::openItem(const QString &identity){const auto item=m_model->item(identity);if(item.isEmpty())return;m_context.selected=identity;m_context.focus="results";if(item.value("collection").toBool())openCollection(item.value("metadata").toMap().value("header").toMap().value("collectionId").toString());else if(item.value("state")=="ready"||item.value("state")=="missing")openSubmission(item.value("submission").toMap());}
void CatalogController::author(const QString &id){bool ok=false;const auto n=id.toULongLong(&ok);if(!ok||!n||QString::number(n)!=id)return;Context c;c.query=m_context.query;c.query.type="";c.query.includeWips=true;c.query.map.reset();c.query.audio.reset();c.query.author=id;c.query.search.clear();c.query.categories.clear();c.query.heroes.clear();c.query.page=0;c.focus="results";transition(c,true);emit remoteRequested();}
void CatalogController::findAuthors(const QString &search){if(m_context.query.type=="collections")return;for(auto &r:m_receipts)if(current(r)&&r.identity.kind=="authors")r.obsolete=true;m_authors.clear();auto args=m_context.query.scope();args.insert("search",search);args.insert("hideNsfw",m_context.query.hideNsfw);if(!CatalogData::operation("authors",args))setError(tr("Author search is limited to 512 UTF-8 bytes."));else begin("authors",args);emit changed();}
void CatalogController::back(){
    if(m_returnLibrary&&m_history.size()<=m_libraryReturnDepth){
        m_returnLibrary=false;m_libraryReturnDepth=0;setActive(false);emit returnToLibrary();
    }else if(!m_history.isEmpty()){auto c=m_history.takeLast();transition(c,false);}
}
void CatalogController::clearCacheAndResync(){
    const auto reason=clearCacheReason();
    if(!reason.isEmpty()){setError(reason);emit changed();return;}
    setError({});m_autoAttempted=true;
    begin("sync",{{"forceRefresh",true},{"forceReconcile",true},{"clearCache",true}},true);
    emit changed();
}
void CatalogController::refresh(){if(m_shutdown)return;setError({});m_autoAttempted=true;if(m_phase!="ready"){retry();return;}if(m_context.route=="submission")begin("detail",{{"submission",m_context.submission.json()},{"refresh",true}});else if(m_context.route=="collection")begin("collection",collectionArgs(m_context.collection,m_context.page,true));else begin("sync",{{"forceRefresh",true},{"forceReconcile",false}});emit changed();}
void CatalogController::retry(){setError({});m_selectedReconcileRevision.clear();if(!available()){emit changed();return;}if(m_phase=="failed"){m_reopen=true;m_readNeeded=true;m_poll.start();pulse();}else if(m_phase=="ready"){m_readNeeded=true;m_autoAttempted=false;pulse();}else session();}
void CatalogController::cancel(){
    m_readNeeded=false;
    for(auto &r:m_receipts)if(!r.terminal&&!r.obsolete&&r.identity.generation==QString::number(m_generation)&&r.identity.kind!="favoriteSet"&&r.identity.kind!="favoritesGet")r.userCancelled=true;
    cancelView(m_generation);m_status=tr("Cancelling this view’s catalog requests…");emit changed();pulse();
}
void CatalogController::nextPage(int delta){
    if(delta!=1&&delta!=-1)return;const int next=m_context.page+delta;if(next<0||(delta==1&&!hasNext()))return;
    auto previous=m_currentPage;auto cached=m_adjacent;
    Context c=m_context;c.page=next;c.query.page=next;c.scroll=0;c.focus="results";c.selected.clear();transition(c,false);
    m_adjacent=std::move(previous);
    if(cached&&cached->page==next)publish(*cached);
}
void CatalogController::loadResource(const QString &kind,int page,bool refresh){
    if(m_context.route!="submission"||!QStringList{"files","comments","changelog"}.contains(kind)||page<1||page>(kind=="files"?7:500))return;
    const bool different=m_resourceKind!=kind||m_resourcePage!=page;
    if(different){
        for(auto it=m_receipts.begin();it!=m_receipts.end();++it)if(current(*it)&&QStringList{"files","comments","changelog"}.contains(it->identity.kind))it->obsolete=true;
        m_resource.clear();m_resourceAutoAttempted=false;
    }
    m_resourceKind=kind;m_resourcePage=page;
    if(refresh)m_resourceAutoAttempted=true;
    begin(kind,{{"submission",m_context.submission.json()},{"page",page},{"refresh",refresh}});emit changed();
}
void CatalogController::closeResource(){for(auto it=m_receipts.begin();it!=m_receipts.end();++it)if(current(*it)&&QStringList{"files","comments","changelog"}.contains(it->identity.kind))it->obsolete=true;m_resource.clear();m_resourceKind.clear();emit changed();}
void CatalogController::invalidateSelectedFavorite(){
    m_selectedFavorite.reset();
    auto summary=m_detail.value("summary").toMap();
    if(!summary.isEmpty()){
        summary.insert("favoriteKnown",false);summary.remove("favoriteRevision");m_detail.insert("summary",summary);
    }
}
bool CatalogController::acceptPreferencesRevision(const QString &revision,bool pageProjection){
    if(!m_preferencesRevision.isEmpty()&&revision.toULongLong()<m_preferencesRevision.toULongLong())return false;
    if(m_preferencesRevision!=revision){
        m_model->clearFavorites();m_adjacent.reset();m_currentPage.reset();invalidateSelectedFavorite();
        if(!pageProjection&&(m_context.route=="browse"||m_context.route=="collection"))m_readNeeded=true;
    }
    m_preferencesRevision=revision;return true;
}
bool CatalogController::reconcileFavorite(const SubmissionKey &key){
    if(!key.valid()||m_shutdown||m_phase!="ready")return false;
    const auto args=QJsonObject{{"submission",key.json()}};
    for(const auto &r:std::as_const(m_receipts))if(!r.terminal&&!r.obsolete&&r.identity.kind=="favoritesGet"
        &&r.identity.args==args&&r.preferenceFloor==m_preferencesRevision)return true;
    if(m_receipts.size()>=8){m_deferredFavorite=key;return false;}
    begin("favoritesGet",args,true);return true;
}
void CatalogController::reconcileSelectedFavorite(){
    if(!m_active||m_context.route!="submission"||!m_context.submission.valid()||m_shutdown)return;
    if(m_selectedFavorite&&m_selectedFavorite->key==m_context.submission&&m_selectedFavorite->revision==m_preferencesRevision)return;
    const auto floor=m_preferencesRevision.isEmpty()?QStringLiteral("initial"):m_preferencesRevision;
    // One automatic authoritative read per selected context and observed root
    // revision. A failed/older read needs newer evidence or explicit Retry.
    if(m_selectedReconcileRevision==floor)return;
    if(reconcileFavorite(m_context.submission))m_selectedReconcileRevision=floor;
}
void CatalogController::setFavorite(SubmissionKey key,bool desired){
    if(!key.valid()||favoriteBusy()||m_shutdown)return;
    QString evidenceRevision;
    if(m_context.route=="submission"&&m_context.submission==key){
        const auto summary=m_detail.value("summary").toMap();
        if(m_selectedFavorite&&m_selectedFavorite->key==key&&summary.value("favoriteKnown").toBool()
            &&summary.value("favoriteRevision").toString()==m_selectedFavorite->revision)
            evidenceRevision=m_selectedFavorite->revision;
    }else if(m_context.route=="browse"||m_context.route=="collection")evidenceRevision=m_model->favoriteRevision(key);
    if(evidenceRevision.isEmpty()||evidenceRevision!=m_preferencesRevision)return;
    begin("favoriteSet",{{"submission",key.json()},{"desired",desired},{"expectedRevision",evidenceRevision}},true);emit changed();
}
void CatalogController::setFavorite(const QVariantMap &v,bool desired){const auto k=SubmissionKey::decode(QJsonObject::fromVariantMap(v));if(k)setFavorite(*k,desired);}
void CatalogController::remember(const QString &selected,qreal scroll,const QString &focus,qulonglong generation){if(generation!=m_generation)return;m_context.selected=selected;m_context.scroll=std::max(qreal(0),scroll);m_context.focus=focus;}
bool CatalogController::openLink(const QUrl &url){
    // Explicit destinations must already be among the validated current DTO's
    // URLs. QML cannot turn arbitrary text into a link or invoke local schemes.
    if((url.scheme()!="https"&&url.scheme()!="http")||!url.isValid()||!url.userInfo().isEmpty())return false;
    const auto wanted=url.toString(QUrl::FullyEncoded);
    std::function<bool(const QVariant &)> contains=[&](const QVariant &v){
        if(v.metaType().id()==QMetaType::QVariantMap){
            const auto m=v.toMap();
            for(auto it=m.cbegin();it!=m.cend();++it){
                if(QStringList{"url","profileUrl","link"}.contains(it.key())&&it.value().metaType().id()==QMetaType::QString){const auto s=it.value().toString();if((s.startsWith("https://")||s.startsWith("http://"))&&QUrl(s).toString(QUrl::FullyEncoded)==wanted)return true;}
                if((it.value().metaType().id()==QMetaType::QVariantMap||it.value().metaType().id()==QMetaType::QVariantList)&&contains(it.value()))return true;
            }
        }else if(v.metaType().id()==QMetaType::QVariantList){for(const auto &x:v.toList())if(contains(x))return true;}
        return false;
    };
    return (contains(m_detail)||contains(m_resource))&&QDesktopServices::openUrl(url);
}
void CatalogController::shutdown(){if(m_shutdown)return;m_shutdown=true;m_passiveOpenRetry.stop();m_active=false;m_readNeeded=false;invalidateHome(true,true);for(auto &r:m_receipts)r.obsolete=true;m_poll.start();pulse();emit changed();}
void CatalogController::resumeAfterRetirement(){m_shutdown=false;m_closeSent=false;m_poll.stop();invalidateHome(true,true);loseSession();emit changed();}

QString CatalogController::displayCount(const QString &value) const { bool ok=false;const auto n=value.toULongLong(&ok);return ok?QLocale().toString(n):value; }

QString CatalogController::refreshMessage() const {
    const auto failure=m_provenance.value("refreshError").toMap();
    const auto kind=failure.value("kind").toString();
    if(!kind.isEmpty()){
        if(kind=="catalogRateLimited")return tr("GameBanana is busy. Try again later.");
        if(kind=="catalogTransport")return tr("Couldn't reach GameBanana.");
        if(kind=="catalogRefused")return tr("GameBanana refused the request. Try again later.");
        return tr("Couldn't refresh the catalog.");
    }
    const auto p=syncProgress();
    if(!p.isEmpty())return p.value("total").isNull()?tr("Loading catalog: %1 loaded").arg(p.value("completed").toString()):tr("Loading catalog: %1 of %2 loaded").arg(p.value("completed").toString(),p.value("total").toString());
    return tr("Catalog is up to date.");
}
