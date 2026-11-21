#include "librarymediacontroller.h"
#include "catalogcontroller.h"
#include <QMetaObject>
#include <QTimer>
#include <QUuid>
#include <algorithm>
#include <utility>

namespace {
constexpr qsizetype ConsumerLimit=128, QueueLimit=64, EntryLimit=128, FactLimit=512;
constexpr qsizetype EntryByteLimit=8*1024*1024;
}

LibraryMediaController::LibraryMediaController(CatalogController *catalog,QObject *parent)
    : QObject(parent),m_catalog(catalog) {
    connect(catalog,&CatalogController::changed,this,&LibraryMediaController::catalogChanged);
    connect(catalog,&CatalogController::libraryMediaReady,this,[this](const QString &key,const QString &root,const CatalogDecodedResult &result){
        QMetaObject::invokeMethod(this,[this,key,root,result]{receive(key,root,result);},Qt::QueuedConnection);
    });
    connect(catalog,&CatalogController::libraryMediaEvidence,this,[this](const QString &root,const CatalogDecodedResult &result){
        QMetaObject::invokeMethod(this,[this,root,result]{observe(root,result);},Qt::QueuedConnection);
    });
    connect(catalog,&CatalogController::libraryMediaInvalidated,this,&LibraryMediaController::invalidate);
}
QString LibraryMediaController::identity(const QString &row,const SubmissionKey &submission){
    return QString::number(row.size())+QLatin1Char(':')+row+QLatin1Char(':')+submission.identity();
}
void LibraryMediaController::setContext(const QString &root,quint64 epoch,bool active){
    if(root!=m_root||epoch!=m_epoch){clear();m_root=root;m_epoch=epoch;}
    if(m_active==active)return;
    m_active=active;
    if(!active){cancelWork();m_consumers.clear();m_demands.clear();m_order.clear();m_satisfied.clear();}
    else catalogChanged();
}
void LibraryMediaController::clear(){
    m_active=false;cancelWork();m_consumers.clear();m_demands.clear();m_order.clear();m_satisfied.clear();
    m_facts.clear();m_session.clear();clearEntries();m_root.clear();m_epoch=0;
}
void LibraryMediaController::retainRows(const std::function<std::optional<SubmissionKey>(const QString &)> &current){
    const auto consumers=m_consumers.keys();
    for(const auto &consumer:consumers){
        const auto d=m_consumers.value(consumer);const auto key=current(d.row);
        if(!key||*key!=d.submission)withdraw(consumer);
    }
    const auto entries=m_entries.keys();
    for(const auto &id:entries){
        if(!m_entries.contains(id))continue;
        const auto d=m_entries.value(id).demand;const auto key=current(d.row);
        if(!key||*key!=d.submission)removeEntry(id);
    }
}
void LibraryMediaController::demand(const QString &consumer,const QString &row,const SubmissionKey &submission,bool wanted){
    if(consumer.isEmpty()||consumer.toUtf8().size()>128)return;
    if(!wanted){
        const auto existing=m_consumers.constFind(consumer);
        if(existing!=m_consumers.cend()&&existing->row==row)withdraw(consumer);
        return;
    }
    if(!m_active||row.isEmpty()||row.toUtf8().size()>4096||!submission.valid())return;
    const auto id=identity(row,submission);
    auto existing=m_consumers.constFind(consumer);
    if(existing!=m_consumers.cend()&&existing->identity==id)return;
    if(existing!=m_consumers.cend())withdraw(consumer);
    if(m_consumers.size()>=ConsumerLimit||(consumer!=QStringLiteral("selected")&&!m_consumers.contains(QStringLiteral("selected"))&&m_consumers.size()>=ConsumerLimit-1))return;
    const Demand d{row,id,submission};m_consumers.insert(consumer,d);
    if(!m_demands.contains(id))m_demands.insert(id,d);
    if(m_entries.contains(id)){m_entries[id].used=++m_clock;m_satisfied.insert(id);}
    schedule();
}
void LibraryMediaController::withdraw(const QString &consumer){
    const auto found=m_consumers.constFind(consumer);if(found==m_consumers.cend())return;
    const bool capacityWasTight=m_consumers.size()>=ConsumerLimit-1;
    const auto id=found->identity;m_consumers.remove(consumer);
    const bool stillWanted=std::any_of(m_consumers.cbegin(),m_consumers.cend(),[&](const Demand &d){return d.identity==id;});
    if(!stillWanted){m_demands.remove(id);m_order.removeAll(id);m_satisfied.remove(id);if(m_work&&m_work->demand.identity==id)cancelWork();}
    if(capacityWasTight)notifyCapacity();
    schedule();
}
void LibraryMediaController::cancelWork(){
    if(!m_work)return;
    const auto work=*m_work;m_work.reset();
    if(work.admitted&&m_facts.contains(work.demand.submission.identity())){
        auto &facts=m_facts[work.demand.submission.identity()];
        if(work.refresh)facts.refreshAttempted=false;else facts.readAttempted=false;
    }
    m_catalog->cancelLibraryMedia(work.key);
}
void LibraryMediaController::clearEntries(){
    QSet<QString> rows;for(const auto &entry:std::as_const(m_entries))rows.insert(entry.demand.row);
    m_entries.clear();m_entryBytes=0;
    for(const auto &row:std::as_const(rows))emit mediaChanged(row);
}
void LibraryMediaController::removeEntry(const QString &id){
    const auto it=m_entries.find(id);if(it==m_entries.end())return;
    const auto row=it->demand.row;m_entryBytes-=it->bytes;m_entries.erase(it);emit mediaChanged(row);
}
void LibraryMediaController::removeSubmission(const SubmissionKey &submission){
    const auto keys=m_entries.keys();
    for(const auto &id:keys)if(m_entries.contains(id)&&m_entries.value(id).demand.submission==submission)removeEntry(id);
}
void LibraryMediaController::updateSubmission(const Demand &d,const LibraryMediaProjection &media){
    QList<Demand> affected;
    for(const auto &entry:std::as_const(m_entries))if(entry.demand.submission==d.submission)affected.append(entry.demand);
    if(std::none_of(affected.cbegin(),affected.cend(),[&](const Demand &old){return old.identity==d.identity;}))affected.append(d);
    for(const auto &row:std::as_const(affected))putEntry(row,media);
}
void LibraryMediaController::putEntry(const Demand &d,const LibraryMediaProjection &media){
    if(media.images.isEmpty() && media.authorId.isEmpty()){removeEntry(d.identity);return;}
    qsizetype bytes=media.authorId.toUtf8().size()+d.identity.toUtf8().size()+d.row.toUtf8().size()+d.submission.identity().toUtf8().size();
    for(const auto &url:media.images)bytes+=url.toUtf8().size();
    if(bytes>EntryByteLimit)return;
    auto existing=m_entries.find(d.identity);
    if(existing!=m_entries.end()&&existing->media==media){existing->used=++m_clock;return;}
    const bool replacing=existing!=m_entries.end();
    qsizetype remainingBytes=m_entryBytes-(replacing?existing->bytes:0);
    qsizetype remainingCount=m_entries.size()-(replacing?1:0);
    QList<QString> candidates;
    for(auto it=m_entries.cbegin();it!=m_entries.cend();++it)
        if(it.key()!=d.identity&&!m_demands.contains(it.key()))candidates.append(it.key());
    std::sort(candidates.begin(),candidates.end(),[this](const QString &a,const QString &b){return m_entries.value(a).used<m_entries.value(b).used;});
    QList<QString> evict;
    for(const auto &candidate:std::as_const(candidates)){
        if(remainingCount<EntryLimit&&remainingBytes+bytes<=EntryByteLimit)break;
        remainingBytes-=m_entries.value(candidate).bytes;--remainingCount;evict.append(candidate);
    }
    // A demanded projection may own an open viewer. Keep it stable and leave
    // this newcomer on its fallback when inactive entries cannot make room.
    if(remainingCount>=EntryLimit||remainingBytes+bytes>EntryByteLimit){
        // Changed identity or masking evidence invalidates this row's older
        // projection even when the replacement cannot be retained.
        if(replacing)removeEntry(d.identity);
        return;
    }
    if(replacing){m_entryBytes-=existing->bytes;m_entries.erase(existing);}
    QSet<QString> changed;
    for(const auto &candidate:std::as_const(evict)){
        auto victim=m_entries.find(candidate);
        changed.insert(victim->demand.row);m_entryBytes-=victim->bytes;m_entries.erase(victim);
    }
    m_entries.insert(d.identity,Entry{d,media,bytes,++m_clock});m_entryBytes+=bytes;changed.insert(d.row);
    for(const auto &row:std::as_const(changed))emit mediaChanged(row);
}
std::optional<LibraryMediaProjection> LibraryMediaController::projection(const QString &row,const SubmissionKey &submission) const {
    const auto it=m_entries.constFind(identity(row,submission));
    if(it==m_entries.cend())return {};return it->media;
}
void LibraryMediaController::schedule(){
    if(m_scheduled)return;m_scheduled=true;
    QTimer::singleShot(0,this,[this]{m_scheduled=false;pump();});
}
void LibraryMediaController::notifyCapacity(){
    if(m_capacityScheduled)return;m_capacityScheduled=true;
    QTimer::singleShot(0,this,[this]{m_capacityScheduled=false;if(m_active)emit capacityAvailable();});
}
bool LibraryMediaController::needsRead(const Demand &d) const {
    const auto facts=m_facts.value(d.submission.identity());
    return !facts.readAttempted||(facts.cacheHasMedia&&!m_entries.contains(d.identity)&&!m_satisfied.contains(d.identity));
}
bool LibraryMediaController::needsRefresh(const Demand &d) const {
    const auto facts=m_facts.value(d.submission.identity());
    return facts.readAttempted&&facts.refreshWanted&&!facts.refreshAttempted;
}
void LibraryMediaController::catalogChanged(){
    const auto session=m_catalog->libraryMediaSession(),root=m_catalog->libraryMediaRoot();
    if(!session.isEmpty()&&!root.isEmpty()&&root==m_root&&session!=m_session){
        if(m_work&&m_work->admitted&&m_work->session!=session)cancelWork();
        m_session=session;m_facts.clear();m_satisfied.clear();clearEntries();
    }else if((session.isEmpty()||root!=m_root)&&!m_session.isEmpty()){
        if(m_work&&m_work->admitted)cancelWork();
        m_satisfied.clear();clearEntries();
    }
    schedule();
}
void LibraryMediaController::pump(){
    if(!m_active||m_root.isEmpty()||m_catalog->libraryMediaClientRoot()!=m_root)return;
    if(m_work&&!m_demands.contains(m_work->demand.identity))cancelWork();
    // New delegates can arrive after a stale entry started its online refresh.
    // Their cache reads take priority even when that refresh was already admitted.
    if(m_work && m_work->refresh && std::any_of(m_demands.cbegin(),m_demands.cend(),
            [this](const Demand &d){return needsRead(d);}))cancelWork();
    if(!m_work){
        const auto canTrack=[this](const Demand &d){return m_facts.contains(d.submission.identity())||m_facts.size()<FactLimit;};
        const auto queued=m_order;
        for(const auto &id:queued)if(!m_demands.contains(id)||!canTrack(m_demands.value(id))||(!needsRead(m_demands.value(id))&&!needsRefresh(m_demands.value(id))))m_order.removeAll(id);
        const auto selected=m_consumers.constFind(QStringLiteral("selected"));
        if(selected!=m_consumers.cend()&&canTrack(*selected)&&!m_order.contains(selected->identity)&&(needsRead(*selected)||needsRefresh(*selected))&&m_order.size()<QueueLimit)m_order.append(selected->identity);
        for(auto it=m_demands.cbegin();it!=m_demands.cend()&&m_order.size()<QueueLimit;++it)
            if(canTrack(*it)&&!m_order.contains(it.key())&&(needsRead(*it)||needsRefresh(*it)))m_order.append(it.key());
        for(int pass=0;pass<2&&!m_work;++pass)for(const auto &id:std::as_const(m_order)){
            if(!m_demands.contains(id))continue;
            const auto d=m_demands.value(id);const auto factKey=d.submission.identity();
            if(!m_facts.contains(factKey)&&m_facts.size()>=FactLimit)continue;
            const bool read=needsRead(d),refresh=needsRefresh(d);
            if((pass==0&&read)||(pass==1&&refresh)){
                m_work=Work{d,QUuid::createUuid().toString(QUuid::Id128),{},pass==1,false};break;
            }
        }
    }
    if(!m_work||m_work->admitted)return;
    const auto key=m_work->key;
    if(m_catalog->requestLibraryMedia(key,m_work->demand.submission,m_work->refresh)&&m_work&&m_work->key==key){
        m_work->admitted=true;m_work->session=m_catalog->libraryMediaSession();
        auto &facts=m_facts[m_work->demand.submission.identity()];
        if(m_work->refresh)facts.refreshAttempted=true;else facts.readAttempted=true;
        m_satisfied.insert(m_work->demand.identity);
    }
}
void LibraryMediaController::receive(const QString &key,const QString &root,const CatalogDecodedResult &result){
    if(!m_work||m_work->key!=key)return;
    const auto work=*m_work;m_work.reset();
    if(!m_active||root!=m_root||!m_demands.contains(work.demand.identity))return;
    if(work.admitted&&(work.session!=m_session||result.session!=work.session||result.root!=root))return;
    auto facts=m_facts.value(work.demand.submission.identity());
    if(result.errorKind=="libraryMediaPreempted"){
        if(!work.refresh)facts.readAttempted=false;
        m_facts.insert(work.demand.submission.identity(),facts);schedule();return;
    }
    if(!work.admitted)facts.readAttempted=false;
    if(result.valid&&(result.errorKind=="catalogPrivate"||result.errorKind=="catalogNotFound")&&work.admitted){
        facts.cacheHasMedia=false;facts.refreshWanted=false;m_facts.insert(work.demand.submission.identity(),facts);removeSubmission(work.demand.submission);
    }else if(result.valid&&result.phase=="succeeded"&&result.submission==work.demand.submission){
        if(!work.refresh)facts.refreshWanted=result.refreshRecommended;
        if(result.state=="unavailable"){
            facts.cacheHasMedia=false;m_facts.insert(work.demand.submission.identity(),facts);removeSubmission(work.demand.submission);
        }else if(result.state=="ready"||result.state=="partial"){
            const auto summary=result.detail.value("summary").toMap();
            auto images=summary.value("images").toStringList();
            const auto thumbnail=summary.value("thumbnail").toString();if(images.isEmpty()&&!thumbnail.isEmpty())images.append(thumbnail);
            const LibraryMediaProjection media{images,summary.contains("masked")?summary.value("masked").toBool():true,summary.value("authorId").toString()};
            facts.cacheHasMedia=!images.isEmpty();m_facts.insert(work.demand.submission.identity(),facts);updateSubmission(work.demand,media);
        }else m_facts.insert(work.demand.submission.identity(),facts);
    }else m_facts.insert(work.demand.submission.identity(),facts);
    schedule();
}
void LibraryMediaController::observe(const QString &root,const CatalogDecodedResult &result){
    if(!result.valid||root!=m_root||result.root!=root||m_session.isEmpty()||result.session!=m_session||!result.submission.valid())return;
    if(result.state=="unavailable"||result.errorKind=="catalogPrivate"||result.errorKind=="catalogNotFound"){
        const auto key=result.submission.identity();
        if(m_facts.contains(key)){auto &facts=m_facts[key];facts.cacheHasMedia=false;facts.refreshWanted=false;facts.readAttempted=true;}
        removeSubmission(result.submission);return;
    }
    if(result.phase!="succeeded"||result.kind!="detail"||(result.state!="ready"&&result.state!="partial"))return;
    const auto summary=result.detail.value("summary").toMap();auto images=summary.value("images").toStringList();
    const auto thumbnail=summary.value("thumbnail").toString();if(images.isEmpty()&&!thumbnail.isEmpty())images.append(thumbnail);
    const LibraryMediaProjection media{images,summary.contains("masked")?summary.value("masked").toBool():true,summary.value("authorId").toString()};
    QList<Demand> affected;for(const auto &entry:std::as_const(m_entries))if(entry.demand.submission==result.submission)affected.append(entry.demand);
    const auto factKey=result.submission.identity();if(m_facts.contains(factKey)){m_facts[factKey].cacheHasMedia=!images.isEmpty();m_facts[factKey].refreshWanted=result.refreshRecommended;}
    for(const auto &d:std::as_const(affected))putEntry(d,media);
}
void LibraryMediaController::invalidate(const QString &root,const QString &session,const QString &submission){
    if(root!=m_root||session.isEmpty()||session!=m_session)return;
    if(m_work&&(submission.isEmpty()||m_work->demand.submission.identity()==submission))cancelWork();
    for(auto it=m_facts.begin();it!=m_facts.end();++it)if(submission.isEmpty()||it.key()==submission){
        it->readAttempted=false;it->cacheHasMedia=false;it->refreshWanted=false;
    }
    const auto entries=m_entries.keys();
    for(const auto &id:entries)if(m_entries.contains(id)&&(submission.isEmpty()||m_entries.value(id).demand.submission.identity()==submission))removeEntry(id);
    for(auto it=m_satisfied.begin();it!=m_satisfied.end();){
        const auto demand=m_demands.constFind(*it);
        if(submission.isEmpty()||(demand!=m_demands.cend()&&demand->submission.identity()==submission))it=m_satisfied.erase(it);else ++it;
    }
    schedule();
}
