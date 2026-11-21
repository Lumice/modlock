#include "optioncontroller.h"
#include "librarymodel.h"
#include "localimportcontroller.h"
#include "fontdata.h"
#include <QJsonArray>
#include <QDir>
#include <QUuid>
#include <algorithm>
namespace {
bool terminal(const QString &s){return QStringList{"cancelled","interrupted","failed","succeeded","recoveryRequired"}.contains(s);}
bool pinned(const QVariantMap &a,const QVariantMap &b){for(const auto *k:{"profileId","modId","expectedRevision"})if(a.value(k)!=b.value(k))return false;return !a.contains("beforeRecordDigest")||a.value("beforeRecordDigest")==b.value("beforeRecordDigest");}
}
OptionController::OptionController(LibraryModel *library):QObject(library),m_library(library){
    auto *gateway=library->localImport();
    m_poll.setInterval(300);m_poll.setSingleShot(true);connect(&m_poll,&QTimer::timeout,this,&OptionController::poll);m_poll.start();
    connect(gateway,&LocalImportController::optionsReady,this,&OptionController::accept);
    connect(gateway,&LocalImportController::optionsRejected,this,[this](BackendClient::Call call,const QString &message){
        m_wait=false;m_error=message;
        switch(call){case BackendClient::Call::OptionsPrepare:case BackendClient::Call::OptionsCancel:case BackendClient::Call::OptionsDismiss:m_loaded=false;break;default:break;}
        updateHistory();emit changed();
    });
    connect(gateway,&LocalImportController::optionsSessionReset,this,[this]{m_loaded=false;m_wait=false;m_jobs.clear();m_job.clear();m_refreshJob.clear();m_detailPending=false;invalidate(tr("The preparation helper disconnected. Reopen Files/options and prepare again."));updateHistory();});
    connect(gateway,&LocalImportController::changed,this,&OptionController::sync);
    connect(library,&LibraryModel::nativeChanged,this,&OptionController::sync);
    connect(library,&LibraryModel::sourceChanged,this,&OptionController::sync);
}
bool OptionController::hasPreparation()const{for(const auto &j:m_jobs)if(!terminal(j.status.value("phase").toString()))return true;return false;}
bool OptionController::available()const{return m_library->profileEditsAvailable()&&m_library->localImport()->optionsAvailable();}
bool OptionController::context()const{return m_library->nativeMode()&&m_root==m_library->dataDir()&&m_helper==m_library->helperPath()&&m_profile==m_library->viewedProfileId()&&m_revision==m_library->revision()&&(m_mod.isEmpty()||!m_library->modDetails(m_mod).isEmpty());}
bool OptionController::editable()const{return available()&&context()&&!m_wait&&!m_library->localImport()->optionsBusy()&&!m_committing&&!m_library->busy();}
bool OptionController::busy()const{return m_wait||m_library->localImport()->optionsBusy()||m_committing||m_library->busy();}
bool OptionController::canPrepare()const{return editable()&&!m_mod.isEmpty()&&!m_rows.isEmpty()&&m_library->localImport()->canPrepare();}
bool OptionController::canBind()const{return editable()&&!m_job.isEmpty()&&job().value("phase")=="ready"&&job().value("profileId").toString()==m_profile&&job().value("modId").toString()==m_mod&&job().value("expectedRevision").toString()==m_revision;}
bool OptionController::canAttach()const{return canBind()&&!attachmentReview().value("token").toString().isEmpty();}
bool OptionController::canSave()const{return editable()&&!m_selectionReview.value("token").toString().isEmpty()&&m_selectionReview.value("summary").toMap().value("canCommit").toBool();}
QVariantMap OptionController::job()const{return m_jobs.value(m_job).status;}
QVariantList OptionController::candidates()const{return m_jobs.value(m_job).candidates;}
QVariantList OptionController::bindings()const{return m_jobs.value(m_job).bindings;}
QVariantMap OptionController::attachmentReview()const{return m_jobs.value(m_job).review;}
QVariantList OptionController::jobs()const{QVariantList out;for(const auto &j:m_jobs)out.append(j.status);return out;}
void OptionController::pin(){m_root=m_library->dataDir();m_helper=m_library->helperPath();m_profile=m_library->viewedProfileId();m_revision=m_library->revision();m_profileName=m_profile;for(const auto &v:m_library->profiles()){const auto p=v.toMap();if(p.value("id").toString()==m_profile){m_profileName=p.value("name").toString();break;}}}
void OptionController::edits(){++m_edits;m_selectionReview.clear();m_previewChange={};emit changed();}
void OptionController::invalidate(const QString &reason){m_detailPending=false;m_selectionReview.clear();m_previewChange={};m_rows.clear();m_order.clear();m_selected.clear();m_totals.clear();for(auto &j:m_jobs)j.review.clear();if(m_visible||m_orderVisible)m_error=reason;emit changed();}
void OptionController::sync(){
    if(m_library->localImport()->retirementInProgress()){emit changed();return;}
    if(!context()&&!m_committing&&(!m_rows.isEmpty()||!m_order.isEmpty()||!m_selectionReview.isEmpty()||m_detailPending))invalidate(tr("The profile, revision or mod changed. Reopen this editor to review the current setup."));
    emit changed();
}
bool OptionController::request(BackendClient::Call call,QJsonObject params){
    if(m_wait)return false;m_wait=true;
    if(!m_library->localImport()->optionsRequest(call,params)){m_wait=false;return false;}emit changed();return true;
}
bool OptionController::open(const QString &mod){
    if(!available()||m_committing||m_library->modDetails(mod).isEmpty())return false;
    if(context()&&m_mod==mod&&(!m_rows.isEmpty()||m_detailPending)){m_visible=true;m_orderVisible=false;emit changed();return true;}
    pin();m_order.clear();m_mod=mod;m_title=m_library->modDetails(mod).value("title").toString();m_visible=true;m_orderVisible=false;m_rows.clear();m_selected.clear();m_totals.clear();m_selectionReview.clear();m_error.clear();m_job.clear();m_detailPending=true;
    emit changed();poll();return true;
}
bool OptionController::openOrder(){
    if(!m_library->profileEditsAvailable()||m_committing)return false;
    if(context()&&m_mod.isEmpty()&&!m_order.isEmpty()){m_orderVisible=true;m_visible=false;emit changed();return true;}
    pin();m_rows.clear();m_selected.clear();m_totals.clear();m_job.clear();m_detailPending=false;m_mod.clear();m_title.clear();m_order=m_library->fullProfileOrder();m_orderVisible=true;m_visible=false;m_selectionReview.clear();m_previewChange={};m_error.clear();emit changed();return true;
}
void OptionController::showHistory(){m_historyVisible=true;m_loaded=false;m_library->localImport()->refreshHistory();poll();emit changed();}
void OptionController::hideHistory(){m_historyVisible=false;emit changed();}
void OptionController::hide(bool order){if(order)m_orderVisible=false;else m_visible=false;emit changed();}
bool OptionController::showJob(const QString &id){
    if(!m_jobs.contains(id)||m_committing)return false;auto s=m_jobs[id].status;
    if(s.value("profileId").toString()!=m_library->viewedProfileId()||s.value("expectedRevision").toString()!=m_library->revision()||m_library->modDetails(s.value("modId").toString()).isEmpty()){
        m_error=tr("This job's target or revision is no longer current. Cancel or dismiss it in preparation history.");emit changed();return false;
    }
    if(!open(s.value("modId").toString()))return false;m_job=id;m_historyVisible=false;emit changed();return true;
}
bool OptionController::setSelected(const QString &id,bool selected){
    if(!editable())return false;
    auto it=std::find_if(m_rows.cbegin(),m_rows.cend(),[&](const QVariant &v){auto r=v.toMap();return r.value("id").toString()==id&&r.value("state")=="ready";});
    if(it==m_rows.cend()||(selected&&!m_selected.contains(id)&&m_selected.size()>=99))return false;
    if(selected){if(!m_selected.contains(id))m_selected.append(id);}else m_selected.removeAll(id);m_selected.sort();edits();return true;
}
bool OptionController::prepareLocal(const QList<QUrl> &roots){
    if(roots.isEmpty()||roots.size()>64)return false;QJsonArray paths;
    for(const auto &url:roots){const auto p=url.toLocalFile();if(!url.isValid()||!url.isLocalFile()||!url.host().isEmpty()||!QDir::isAbsolutePath(p)||p.toUtf8().size()>4096||p.contains(QChar::Null))return false;paths.append(p);}
    return prepare({{"kind","local"},{"roots",paths}});
}
bool OptionController::prepareArchive(const QString &id){
    if(!FontData::hex(id,32))return false;
    const bool owned=std::any_of(m_rows.cbegin(),m_rows.cend(),[&](const QVariant &v){auto r=v.toMap();return r.value("state")=="needsPreparation"&&r.value("archiveId").toString()==id;});
    return owned&&prepare({{"kind","retainedArchive"},{"archiveId",id}});
}
bool OptionController::prepare(QJsonObject source){
    if(!canPrepare())return false;const auto id=QUuid::createUuid().toString(QUuid::Id128);
    if(!request(BackendClient::Call::OptionsPrepare,{{"profileId",m_profile},{"modId",m_mod},{"expectedRevision",m_revision},{"jobId",id},{"source",source}}))return false;
    m_job=id;Job j;j.status={{"jobId",id},{"kind","localOptions"},{"profileId",m_profile},{"modId",m_mod},{"expectedRevision",m_revision},{"phase","enumerating"},{"canCancel",true}};m_jobs.insert(id,j);m_error.clear();updateHistory();emit changed();return true;
}
bool OptionController::setBinding(const QString &option,const QString &candidate){
    if(!canBind())return false;
    auto &j=m_jobs[m_job];
    if(!std::any_of(m_rows.cbegin(),m_rows.cend(),[&](const QVariant &v){auto r=v.toMap();return r.value("id").toString()==option&&r.value("state")!="ready";}))return false;
    if(!candidate.isEmpty()&&!std::any_of(j.candidates.cbegin(),j.candidates.cend(),[&](const QVariant &v){auto c=v.toMap();return c.value("id").toString()==candidate&&c.value("available").toBool();}))return false;
    for(qsizetype i=j.bindings.size();i-->0;){auto b=j.bindings[i].toMap();if(b.value("optionId")==option||b.value("candidateId")==candidate)j.bindings.removeAt(i);}
    if(!candidate.isEmpty())j.bindings.append(QVariantMap{{"optionId",option},{"candidateId",candidate}});j.review.clear();emit changed();return true;
}
bool OptionController::reviewBindings(const QVariantList &bindings){
    if(!canBind()||bindings.isEmpty()||bindings.size()>1024)return false;
    QSet<QString> options,candidates;QJsonArray rows;
    for(const auto &v:bindings){auto b=v.toMap();auto o=b.value("optionId").toString(),c=b.value("candidateId").toString();if(b.size()!=2||!FontData::hex(o,32)||!FontData::hex(c,64)||options.contains(o)||candidates.contains(c))return false;options.insert(o);candidates.insert(c);rows.append(QJsonObject{{"optionId",o},{"candidateId",c}});}
    m_jobs[m_job].review.clear();m_error.clear();return request(BackendClient::Call::OptionsReview,{{"jobId",m_job},{"bindings",rows}});
}
bool OptionController::attach(){
    if(!canAttach())return false;auto token=attachmentReview().value("token").toString();m_committing=true;
    if(!m_library->localImport()->attachOptions(m_profile,m_revision,{{"jobId",m_job},{"token",token}},m_mod)){m_committing=false;emit changed();return false;}
    m_jobs[m_job].review.clear();emit changed();return true;
}
bool OptionController::previewSelection(){
    if(!editable()||m_mod.isEmpty()||m_selected.isEmpty()||m_selected.size()>99)return false;
    m_previewChange={{"kind","selection"},{"modId",m_mod},{"optionIds",QJsonArray::fromStringList(m_selected)}};m_previewEdits=m_edits;m_selectionReview.clear();
    return m_library->dispatchProfileEditPreview(m_root,m_profile,m_revision,m_previewChange);
}
bool OptionController::move(const QString &id,int offset){
    if(!editable()||!m_orderVisible||(offset!=-1&&offset!=1))return false;
    for(qsizetype i=0;i<m_order.size();++i)if(m_order[i].toMap().value("id").toString()==id){auto dest=i+offset;if(dest<0||dest>=m_order.size())return false;m_order.move(i,dest);edits();return true;}return false;
}
bool OptionController::previewOrder(){
    if(!editable()||!m_orderVisible)return false;QJsonArray ids;for(const auto &v:m_order)ids.append(v.toMap().value("id").toString());
    m_previewChange={{"kind","order"},{"modIds",ids}};m_previewEdits=m_edits;m_selectionReview.clear();return m_library->dispatchProfileEditPreview(m_root,m_profile,m_revision,m_previewChange);
}
void OptionController::acceptProfileReview(const QVariantMap &review){
    if(!context()||m_previewEdits!=m_edits||review.value("profileId").toString()!=m_profile||review.value("expectedRevision").toString()!=m_revision)return;
    const auto s=review.value("summary").toMap(),after=s.value("after").toMap();bool valid=s.value("kind").toString()==m_previewChange.value("kind").toString();
    QStringList expected;for(const auto &v:m_library->fullProfileOrder())expected.append(v.toMap().value("id").toString());
    auto beforeIds=review.value("summary").toMap().value("before").toMap().value("modIds").toStringList();
    valid=valid&&beforeIds==expected;
    if(s.value("kind")=="selection")valid=valid&&after.value("modIds").toStringList()==expected;
    if(s.value("kind")=="order")valid=valid&&m_mod.isEmpty()&&QJsonArray::fromVariantList(after.value("modIds").toList())==m_previewChange.value("modIds").toArray();
    else{auto selection=after.value("selection").toMap();QStringList ids;for(const auto &v:selection.value("choices").toList())ids.append(v.toMap().value("optionId").toString());ids.sort();valid=valid&&selection.value("modId").toString()==m_mod&&ids==m_selected;}
    if(valid){
        m_selectionReview=review;
        if(s.value("kind")=="order"){
            // Titles are presentation from the pinned, complete editor snapshot.
            // The worker's before/after IDs and the checked permutation stay authoritative.
            QMap<QString,QString> titles;
            for(const auto &v:m_order){const auto row=v.toMap();titles.insert(row.value("id").toString(),row.value("title").toString());}
            auto comparison=review.value("comparisonRows").toList();
            for(auto &v:comparison){auto row=v.toMap();row.insert("beforeTitle",titles.value(row.value("before").toString()));row.insert("afterTitle",titles.value(row.value("after").toString()));v=row;}
            m_selectionReview.insert("comparisonRows",comparison);
        }
    }else m_error=tr("The review differs from the requested setup. Review again.");emit changed();
}
bool OptionController::commitSelection(){if(!canSave())return false;m_committing=true;const auto token=m_selectionReview.value("token").toString();if(!m_library->dispatchProfileEditCommit(m_root,m_profile,m_revision,token)){m_committing=false;emit changed();return false;}m_selectionReview.clear();emit changed();return true;}
bool OptionController::cancel(){if(m_committing)return false;if(!m_job.isEmpty()&&job().value("canCancel").toBool()&&!cancelJob(m_job))return false;m_visible=m_orderVisible=false;m_rows.clear();m_order.clear();m_selected.clear();m_selectionReview.clear();m_previewChange={};m_detailPending=false;emit changed();return true;}
bool OptionController::canCancelJob(const QString &id)const{return !m_committing && m_jobs.contains(id) && m_jobs[id].status.value("canCancel").toBool() && m_library->localImport()->canCancelOptionsJob(id);}
bool OptionController::cancelJob(const QString &id){if(!canCancelJob(id))return false;m_jobs[id].review.clear();return m_library->localImport()->cancelOptionsJob(id);}
bool OptionController::dismiss(const QString &id){if(!m_jobs.contains(id)||!terminal(m_jobs[id].status.value("phase").toString())||m_committing)return false;return request(BackendClient::Call::OptionsDismiss,{{"jobId",id}});}
void OptionController::finished(bool success,const QString &message,const QString &attachedJob){
    const bool savedSelection=m_committing && attachedJob.isEmpty();
    m_committing=false;m_library->localImport()->optionsFinished();m_selectionReview.clear();m_error=message;
    if(success && savedSelection){m_visible=false;m_orderVisible=false;}
    if(!attachedJob.isEmpty()&&m_jobs.contains(attachedJob)){auto &j=m_jobs[attachedJob];j.review.clear();if(success){m_refreshJob=attachedJob;j.status.insert("phase","succeeded");j.status.insert("canCancel",false);}}
    else if(success&&!m_job.isEmpty()&&m_jobs.contains(m_job))m_refreshJob=m_job;
    if(success&&m_visible&&m_library->viewedProfileId()==m_profile&&!m_library->modDetails(m_mod).isEmpty()){pin();m_detailPending=true;}
    if(success&&m_orderVisible&&m_library->viewedProfileId()==m_profile){pin();m_order=m_library->fullProfileOrder();}
    updateHistory();emit changed();
}
void OptionController::rejected(const QString &message){m_selectionReview.clear();m_previewChange={};for(auto &j:m_jobs)j.review.clear();m_error=message;emit changed();}
void OptionController::updateHistory(){bool blocks=false;for(const auto &j:m_jobs)if(!terminal(j.status.value("phase").toString()))blocks=true;m_library->localImport()->setOptionsHistory(m_jobs.size(),blocks,m_loaded);}
void OptionController::poll(){
    m_poll.start();
    if(m_wait||m_committing||!m_library->localImport()->optionsAvailable())return;
    if(!m_loaded){request(BackendClient::Call::OptionsStatus);return;}
    if(!m_refreshJob.isEmpty()){const auto id=m_refreshJob;if(request(BackendClient::Call::OptionsStatus,{{"jobId",id}}))m_refreshJob.clear();return;}
    if(m_detailPending&&context()){if(request(BackendClient::Call::OptionsGet,{{"profileId",m_profile},{"modId",m_mod},{"expectedRevision",m_revision}}))m_detailPending=false;return;}
    // Observe the bounded purpose population together: a cancelled noncurrent
    // row must not wait behind another active row's sequence of status reads.
    for(const auto &j:m_jobs){const auto phase=j.status.value("phase").toString();if(!terminal(phase)&&phase!="ready"){request(BackendClient::Call::OptionsStatus);return;}}
    if(m_visible&&context()&&job().value("phase")=="ready"&&!m_jobs[m_job].candidatesLoaded)request(BackendClient::Call::OptionsStatus,{{"jobId",m_job},{"includeCandidates",true}});
}
void OptionController::accept(BackendClient::Call call,const QVariantMap &r){
    m_poll.start();m_wait=false;
    if(call==BackendClient::Call::OptionsGet){if(context()&&r.value("profileId").toString()==m_profile&&r.value("modId").toString()==m_mod&&r.value("expectedRevision").toString()==m_revision){auto d=r.value("detail").toMap();m_rows=d.value("rows").toList();m_selected=d.value("selectedIds").toStringList();m_totals=d.value("totals").toMap();m_error.clear();}}
    else if(r.contains("jobs")){m_loaded=true;QMap<QString,Job> next;for(const auto &v:r.value("jobs").toList()){auto s=v.toMap();auto id=s.value("jobId").toString();auto old=m_jobs.value(id);if(!old.status.isEmpty()&&(!pinned(old.status,s)||s.value("sequence").toString().toULongLong()<old.status.value("sequence").toString().toULongLong())){m_error=tr("Preparation history changed its pinned identity or sequence.");m_loaded=false;updateHistory();emit changed();return;}old.status=s;if(terminal(s.value("phase").toString()))old.review.clear();next.insert(id,old);}m_jobs=next;}
    else if(call==BackendClient::Call::OptionsDismiss){auto id=r.value("jobId").toString();m_jobs.remove(id);if(m_job==id)m_job.clear();m_loaded=false;m_library->localImport()->refreshHistory();}
    else if(call==BackendClient::Call::OptionsReview){if(canCancelJob(m_job)&&context()&&r.value("profileId").toString()==m_profile&&r.value("modId").toString()==m_mod&&r.value("expectedRevision").toString()==m_revision&&r.value("summary").toMap().value("attachedOptionCount").toInt()==m_jobs.value(m_job).bindings.size())m_jobs[m_job].review=r;}
    else{
        const auto id=r.value("jobId").toString();if(!m_jobs.contains(id)){m_error=tr("A preparation reply has no matching job.");emit changed();return;}
        auto &j=m_jobs[id];auto prior=j.status;
        if(prior.value("profileId")!=r.value("profileId")||prior.value("modId")!=r.value("modId")||prior.value("expectedRevision")!=r.value("expectedRevision")||(prior.contains("beforeRecordDigest")&&prior.value("beforeRecordDigest")!=r.value("beforeRecordDigest"))){m_error=tr("A preparation reply changed its pinned target.");j.review.clear();emit changed();return;}
        if(r.value("sequence").toString().toULongLong()>=prior.value("sequence").toString().toULongLong()){j.status=r;j.status.remove("candidateSummary");if(r.contains("candidateSummary")){j.candidates=r.value("candidateSummary").toMap().value("candidates").toList();j.candidatesLoaded=true;}if(terminal(r.value("phase").toString()))j.review.clear();auto error=r.value("error").toMap();if(!error.isEmpty())m_error=error.value("kind").toString()+": "+error.value("message").toString();}
    }
    updateHistory();emit changed();
}

void OptionController::discardSelectionReview(){if(m_committing)return;m_selectionReview.clear();m_previewChange={};emit changed();}
void OptionController::discardAttachmentReview(){if(m_committing)return;if(m_jobs.contains(m_job))m_jobs[m_job].review.clear();emit changed();}
