#include "apppaths.h"
#include "launchcontroller.h"
#include "launchdata.h"
#include "librarymodel.h"
#include <QCryptographicHash>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QRunnable>
#include <QThreadPool>
#include <QUuid>

namespace {
using C=BackendClient::Call;
QString freshId(){return QUuid::createUuid().toString(QUuid::Id128);}
class LaunchFileTask final : public QObject,public QRunnable {
    Q_OBJECT
public:
    explicit LaunchFileTask(std::function<std::pair<bool,QJsonObject>()> action):m_action(std::move(action)){}
    void run() override {const auto r=m_action();emit finished(r.first,r.second);}
signals:
    void finished(bool ok,const QJsonObject &value);
private:
    std::function<std::pair<bool,QJsonObject>()> m_action;
};

}

LaunchController::LaunchController(LibraryModel *model,BackendClient *client):QObject(model),m_model(model),m_client(client){
    m_health=new LaunchHealth(model);
    m_poll.setInterval(2000);connect(&m_poll,&QTimer::timeout,this,[this]{
        if(!m_liveAttempt&&!m_barriers.isEmpty()&&m_game.value("state").toString()!="running"
            &&(!m_barrierRetryAge.isValid()||m_barrierRetryAge.elapsed()>=10000)){m_barriersReadNeeded=true;m_barrierRetryAge.restart();}
        poll();
    });
    m_followUp.setSingleShot(true);connect(&m_followUp,&QTimer::timeout,this,[this]{
        if(m_pending)send(m_pending->retryAdmission?m_pending->call:C::LaunchControl,m_pending->retryAdmission?m_pending->params:QJsonObject{{"controlId",m_pending->params["controlId"]}});
        if(m_liveAttempt&&m_beginAdmitted&&!m_statusUnavailable
            &&QStringList{"reviewing","preparing","verifyingPrepared","openingSteam"}.contains(m_phase))
            send(C::LaunchStatus,{{"epoch",m_binding.value("epoch")},{"attemptId",m_binding.value("attemptId")}});
    });
    connect(client,&BackendClient::launchReady,this,&LaunchController::reply);
    connect(client,&BackendClient::failed,this,[this](const QString &message){
        if(settingsSavePending()||(m_pending&&m_pending->call==C::LaunchSettingsGet))transportLost(message);
    });
    connect(client,&BackendClient::capabilitiesReady,this,[this]{m_settingsReadNeeded=true;m_barriersReadNeeded=true;m_attemptReadNeeded=m_binding.contains("epoch");m_poll.start();poll();update();});
    connect(client,&BackendClient::lifecycleChanged,this,&LaunchController::changed);
    const auto ownerRetired=[this]{
        // An owner-open draft has never reached persistence or LaunchBegin. Only
        // release its UI gate once the possibly opened owner's lifetime ends.
        if(unsentOpenDraft()){m_binding={};m_openDraft={};m_intent=Intent::None;m_attemptReadNeeded=false;m_phase="failed";}
        if(m_pending&&m_pending->call==C::LaunchSettingsGet)settingsReadResult("unconfirmed",tr("Reconnect to read the saved arguments."));
        if(m_intent==Intent::SaveSettings)abandonSettingsOpen(tr("The settings save was not sent. Reconnect before trying again."));
        else if(settingsSavePending()) {m_settingsReadNeeded=true;if(m_settingsSaveState.value("status")!="refused")settingsSaveResult("unconfirmed",tr("Reconnect to check the save."));}
        m_attemptReadNeeded=m_binding.contains("epoch");m_barriersReadNeeded=true;
        m_inFlight.clear();m_pending.reset();m_epoch.clear();m_liveGesture=false;m_liveAttempt=false;m_beginAdmitted=false;m_statusUnavailable=true;m_ready={};m_client->setLaunchWriting(false);update();
    };
    connect(client,&BackendClient::retired,this,ownerRetired);
    connect(client,&BackendClient::launchRetired,this,ownerRetired);
    connect(client,&BackendClient::rejected,this,[this](C c,const QString &kind,const QString &message){
        if(!LaunchData::call(c))return;m_inFlight.remove(int(c));
        if(c==C::LaunchStatus&&kind=="unknownLiveLaunch")return;
        if(c==C::LaunchControl&&m_pending&&m_pending->call==C::LaunchSettingsGet){
            const auto detail=kind+": "+message;
            m_pending.reset();m_settingsError=detail;
            settingsReadResult("unconfirmed",detail);emit settingsChanged();update();return;
        }
        if(kind=="launchBusy" && !(m_pending && m_pending->call==C::LaunchSettingsSet && (c==C::LaunchSettingsSet||c==C::LaunchControl))) {
            if(m_pending && m_pending->call==c)m_pending->retryAdmission=true;
            scheduleFollowUp(200);update();return;
        }
        const auto detail=kind+": "+message;
        if(c==C::LaunchSettingsGet&&m_pending&&m_pending->call==c)settingsReadResult("failed",detail);
        if(c==C::LaunchOpen&&m_pending&&m_pending->call==c&&m_intent==Intent::SaveSettings)abandonSettingsOpen(detail);
        const bool settingsSet=m_pending&&m_pending->call==C::LaunchSettingsSet
            &&(c==C::LaunchSettingsSet||c==C::LaunchControl)&&settingsPendingCurrent(*m_pending);
        if(settingsSet){m_pending.reset();checkSettingsSave(detail,c==C::LaunchSettingsSet);}
        else if(c==C::LaunchControl&&m_pending&&m_pending->call==C::LaunchOpen&&m_intent==Intent::SaveSettings)abandonSettingsOpen(detail);
        if(c==C::LaunchSettingsGet){m_settingsError=detail;emit settingsChanged();}
        else if(!settingsSet)m_error=detail;
        // Rejection belongs to this command. It never rewrites settled attempt facts.
        if(m_pending&&m_pending->call==c)m_pending.reset();
        if(c==C::LaunchCommit||c==C::LaunchBegin){m_liveGesture=false;m_attemptReadNeeded=true;}
        // A refused reconciliation read (for example launchBusy) is retried by the next poll.
        if(c==C::LaunchQuery)m_attemptReadNeeded=true;
        if(c==C::LaunchBarriers)m_barriersReadNeeded=true;
        if(c==C::LaunchOpen)retireUnsentOpen();
        update();
    });
    connect(model,&LibraryModel::sourceChanged,this,&LaunchController::contextChanged);
}
QString LaunchController::activeProfileName()const{for(const auto &v:m_model->profiles()){const auto p=v.toMap();if(p["id"].toString()==m_model->activeProfileId())return p["name"].toString();}return m_model->activeProfileId();}
QString LaunchController::targetProfileName()const{if(!m_profileName.isEmpty())return m_profileName;for(const auto &v:m_model->profiles()){const auto p=v.toMap();if(p["id"].toString()==targetProfileId())return p["name"].toString();}return targetProfileId();}
bool LaunchController::available()const{return m_model->nativeMode()&&m_client->readyForRequests()&&!m_shuttingDown&&!m_client->retiring();}
bool LaunchController::writeGate()const{return m_gameRunningLock||!AppPaths::stateWriteError().isEmpty()||settingsSavePending()||!m_binding.isEmpty()||!m_barriers.isEmpty()||(m_terminalRead&&m_barriersReadNeeded)||m_client->launchOwned()||m_fileBusy||m_pending.has_value();}
bool LaunchController::canLaunch()const{const auto b=m_model->bisect();const bool bisectReady=!b.value("active").toBool() || (!b.value("profileChanged").toBool() && b.value("profileId").toString()==m_model->activeProfileId());return bisectReady&&available()&&!writeGate()&&!m_model->busy()&&!m_model->loading()&&!m_model->outcomeUnknown()&&!m_model->awaitingEditSnapshot()&&!m_model->recoveryRequired()&&m_model->m_marker.isEmpty()&&!m_model->m_localImport->busy()&&m_model->m_localImport->phase()!="ready"&&!m_model->m_options->busy()&&!m_model->m_options->hasPreparation()&&!m_settingsRevision.isEmpty()&&!settingsSavePending();}
bool LaunchController::contextCurrent()const{return m_binding.value("root")==QJsonValue(m_model->dataDir())&&m_binding.value("profileId")==QJsonValue(m_model->activeProfileId())&&m_binding.value("expectedRevision")==QJsonValue(m_model->revision())&&m_binding.value("settingsRevision")==QJsonValue(m_settingsRevision);}
bool LaunchController::canCommit()const{return available()&&m_beginAdmitted&&m_liveAttempt&&m_liveGesture&&!m_committed&&!m_ready.isEmpty()&&!busy()&&contextCurrent()&&m_phase=="ready";}
bool LaunchController::canCancel()const{return available()&&(m_liveAttempt||(m_liveGesture&&!m_committed))&&!m_binding.isEmpty()&&m_phase!="cancelled"&&m_phase!="failed"&&m_phase!="completed"&&m_phase!="resolved"&&m_phase!="timedOut"&&handoffState()!="dispatched"&&handoffState()!="uncertain";}
bool LaunchController::canStopWaiting()const{return available()&&m_liveAttempt&&!m_statusUnavailable&&!m_binding.isEmpty()&&(observationState()=="waiting"||observationState()=="running"||handoffState()=="dispatched"||handoffState()=="uncertain")&&m_phase!="timedOut";}
bool LaunchController::canResolve()const{return available()&&!busy()&&!m_model->writeBusy()&&!m_client->writing()&&!m_client->launchWriting()&&!m_barriers.isEmpty()&&(!m_liveAttempt||QStringList{"failed","cancelled","timedOut","completed","resolved"}.contains(m_phase));}
bool LaunchController::canStop()const{if(!available()||!m_gameAge.isValid()||m_gameAge.elapsed()>=2000)return false;for(const auto &v:m_game.value("processes").toList())if(v.toMap().value("canStop").toBool())return true;return false;}
QVariantMap LaunchController::gameStatus()const{auto out=m_game;if(!m_gameAge.isValid()||m_gameAge.elapsed()>=2000){auto rows=out.value("processes").toList();for(auto &v:rows){auto row=v.toMap();row["canStop"]=false;v=row;}out["processes"]=rows;}return out;}
void LaunchController::update(){emit changed();emit m_model->nativeChanged();maybeAutoCommit();}
void LaunchController::settingsReadResult(const QString &status,const QString &error){
    if(m_settingsReadState.value("contextId").toString()!=settingsContextId()||m_settingsReadState.isEmpty())return;
    m_settingsReadState["status"]=status;m_settingsReadState["error"]=error;emit settingsReadStateChanged();
}
void LaunchController::settingsSaveResult(const QString &status,const QString &error){
    if(m_settingsSaveState.value("contextId").toString()!=settingsContextId()||m_settingsSaveState.isEmpty())return;
    if(m_settingsSaveState.value("status").toString()=="confirmed"&&status!="confirmed")return;
    m_settingsSaveState["status"]=status;m_settingsSaveState["error"]=error;emit settingsSaveStateChanged();
}
void LaunchController::maybeAutoCommit(){
    // Ready and the control receipt are independent prerequisites. Reconsider the
    // original gesture when either changes, not only on a newer status sequence.
    if(m_beginAdmitted&&m_liveAttempt&&m_ready.value("changed")==QJsonValue(false)&&canCommit())commit();
}
QString LaunchController::path(const QString &suffix)const{const auto key=QCryptographicHash::hash(m_root.toUtf8(),QCryptographicHash::Sha256).toHex();return QDir(AppPaths::stateRoot()).filePath("pending/"+key+suffix+".json");}
void LaunchController::loadRoot(const QString &root){
    m_barrierRetryAge.invalidate();
    m_health->load(root);
    ++m_rootGeneration;m_root=root;m_binding={};m_openDraft={};m_ready={};m_attempt={};m_facts={};m_game={};m_barriers={};m_epoch.clear();m_settingsRevision.clear();m_arguments.clear();m_saveArguments.clear();m_intent=Intent::None;m_committed=false;m_liveGesture=false;m_liveAttempt=false;m_beginSent=false;m_beginAdmitted=false;m_haveSequence=false;m_pending.reset();m_inFlight.clear();m_phase="idle";m_error.clear();m_settingsError.clear();m_profileName.clear();m_preparationOutcome="unknown";m_preparationError.clear();m_preparationViewConfirmed=false;m_viewReadSent=false;m_attemptReadNeeded=false;m_shuttingDown=false;m_statusUnavailable=true;m_terminalRead=false;m_gameAge.invalidate();m_poll.stop();m_followUp.stop();m_statusDelay=50;m_fileBusy=true;
    m_settingsSnapshot.clear();m_settingsReadState.clear();m_settingsSaveState.clear();m_settingsReadControlId.clear();
    emit settingsContextChanged();emit settingsSnapshotChanged();emit settingsReadStateChanged();emit settingsSaveStateChanged();
    const auto stateError=AppPaths::stateWriteError();
    if(!stateError.isEmpty()){m_fileBusy=false;m_error=stateError;m_settingsError=stateError;update();return;}
    const auto generation=m_rootGeneration;const auto refPath=path(".launch");
    auto *task=new LaunchFileTask([refPath]{
        // The helper handoff record and snapshot replaced this old Play reference.
        QFile::remove(refPath);return std::make_pair(true,QJsonObject{});
    });
    connect(task,&LaunchFileTask::finished,this,[this,generation,root](bool,const QJsonObject &){
        if(generation!=m_rootGeneration||root!=m_root)return;
        m_fileBusy=false;emit settingsChanged();update();if(available())poll();
    },Qt::QueuedConnection);QThreadPool::globalInstance()->start(task);update();
}
void LaunchController::detachRoot(){
    m_health->load(QString());
    ++m_rootGeneration;m_intent=Intent::None;m_saveArguments.clear();
    m_root.clear();m_settingsRevision.clear();m_arguments.clear();m_settingsError.clear();
    m_settingsSnapshot.clear();m_settingsReadState.clear();m_settingsSaveState.clear();m_settingsReadControlId.clear();
    emit settingsContextChanged();emit settingsSnapshotChanged();emit settingsReadStateChanged();emit settingsSaveStateChanged();emit settingsChanged();update();
}
void LaunchController::abandonSettingsOpen(const QString &detail){
    if(m_intent!=Intent::SaveSettings)return;
    m_intent=Intent::None;m_saveArguments.clear();settingsSaveResult("failed",detail);emit settingsChanged();
    if(m_client->launchOwned()&&!m_client->retiring()){if(!m_shuttingDown)m_model->m_restartAfterDrain=true;m_client->retireLaunch();}
}
void LaunchController::checkSettingsSave(const QString &detail,bool refused){
    if(!settingsSavePending())return;
    m_settingsReadNeeded=true;m_settingsError=detail;
    settingsSaveResult(refused?"refused":"unconfirmed",detail);emit settingsChanged();
    // Even a refused command may leave an owner alive. Drain it before the read;
    // a lost reply can still have a writer finishing its single SQLite commit.
    if(m_client->launchOwned()&&!m_client->retiring()){
        if(!m_shuttingDown)m_model->m_restartAfterDrain=true;
        m_client->retireLaunch();
    }
    update();
}
bool LaunchController::settingsPendingCurrent(const Pending &pending)const{
    return pending.call==C::LaunchSettingsSet&&pending.generation==m_rootGeneration&&pending.root==m_root
        &&pending.params["dataDir"]==QJsonValue(m_root)&&m_settingsSaveState.value("contextId").toString()==settingsContextId()
        &&pending.params["operationId"].toString()==m_settingsSaveState.value("operationId").toString()
        &&pending.params["expectedSettingsRevision"].toString()==m_settingsSaveState.value("expectedRevision").toString()
        &&pending.params["additionalArguments"]==QJsonValue(m_saveArguments);
}

void LaunchController::send(C c,QJsonObject p){
    if(m_shuttingDown||!m_client->readyForRequests()||m_client->retiring()||m_inFlight.contains(int(c)))return;
    p["dataDir"]=m_root;
    if(LaunchData::asynchronous(c)){
        if(m_pending){if(!m_pending->retryAdmission||m_pending->call!=c)return;p=m_pending->params;m_pending->retryAdmission=false;}
        else {p["controlId"]=freshId();m_pending=Pending{c,p,m_root,m_rootGeneration};}
    }
    auto binding=m_binding;
    if(!m_ready.isEmpty())binding["originalReady"]=m_ready;
    if((c==C::LaunchResolve||(c==C::LaunchControl&&m_pending&&m_pending->call==C::LaunchResolve))&&!m_resolving.isEmpty())binding={{"resolvedAttempt",QJsonObject{{"epoch",m_resolving.value("epoch")},{"attemptId",m_resolving.value("attemptId")}}}};
    m_inFlight.insert(int(c));
    if(c==C::LaunchSettingsGet){m_settingsReadControlId=p["controlId"].toString();m_settingsReadState={{"contextId",settingsContextId()},{"requestId",freshId()},{"status",QStringLiteral("reading")},{"snapshotSerial",QString()},{"error",QString()}};}
    m_client->nativeRequest(c,p,binding);
    if(c==C::LaunchSettingsGet)emit settingsReadStateChanged();
    if(c!=C::LaunchStatus)update();
}
void LaunchController::reply(C c,const QVariantMap &result){
    m_inFlight.remove(int(c));const auto o=QJsonObject::fromVariantMap(result);
    if(LaunchData::asynchronous(c)){if(!m_pending||m_pending->call!=c||o["controlId"]!=m_pending->params["controlId"])return;send(C::LaunchControl,{{"controlId",o["controlId"]}});return;}
    if(c==C::LaunchControl){
        if(!m_pending||o["controlId"]!=m_pending->params["controlId"])return;
        if(o["state"]==QJsonValue("pending")){
            scheduleFollowUp(50);
            return;
        }
        if(o["state"]==QJsonValue("failed") && o["error"].toObject()["kind"]==QJsonValue("launchBusy") && m_pending->call!=C::LaunchSettingsSet) {
            m_pending->params["controlId"]=freshId();m_pending->retryAdmission=true;scheduleFollowUp(200);return;
        }
        const auto pending=*m_pending;const auto original=pending.call;m_pending.reset();
        if(o["state"]==QJsonValue("succeeded")){
            complete(original,o["value"].toObject(),&pending);
            QTimer::singleShot(0,this,&LaunchController::poll);
        }
        else {
            m_liveGesture=false;const bool failed=o["state"]==QJsonValue("failed");const auto e=o["error"].toObject();
            const auto detail=failed?e["kind"].toString()+": "+e["message"].toString():tr("This connection cannot confirm the request. Reconnect to check the current state.");
            if(original==C::LaunchSettingsSet&&settingsPendingCurrent(pending))checkSettingsSave(detail,failed);
            else if(original==C::LaunchSettingsGet){m_settingsError=detail;settingsReadResult(failed?"failed":"unconfirmed",detail);emit settingsChanged();}
            else {m_error=detail;if(original==C::LaunchOpen&&m_intent==Intent::SaveSettings)abandonSettingsOpen(detail);}
            m_attemptReadNeeded=m_binding.contains("epoch");if(original==C::LaunchOpen)retireUnsentOpen();
        }
    }else {
        const bool unchanged=c==C::LaunchStatus && o["phase"]!=QJsonValue("unknown") && m_haveSequence
            && o["sequence"].toString().toULongLong()<=m_sequence;
        complete(c,o);
        if(unchanged)return;
    }
    update();
}
void LaunchController::complete(C c,const QJsonObject &o,const Pending *pending){
    if(pending&&(pending->generation!=m_rootGeneration||pending->root!=m_root||pending->params["dataDir"]!=QJsonValue(m_root)))return;
    const auto params=pending?pending->params:QJsonObject{};
    switch(c){
    case C::LaunchOpen:m_epoch=o["epoch"].toString();continueIntent();break;
    case C::LaunchSettingsGet:{
        const auto saveStatus=m_settingsSaveState.value("status").toString();
        const bool checking=settingsSavePending()&&(saveStatus=="unconfirmed"||saveStatus=="refused");
        m_arguments=o["additionalArguments"].toString();m_settingsRevision=o["revision"].toString();m_settingsError.clear();
        if(params["controlId"].toString()==m_settingsReadControlId&&m_settingsReadState.value("contextId").toString()==settingsContextId()){
            const auto serial=QString::number(++m_settingsSnapshotSerial);m_settingsSnapshot={{"contextId",settingsContextId()},{"serial",serial},{"revision",m_settingsRevision},{"additionalArguments",m_arguments}};m_settingsReadState["status"]="succeeded";m_settingsReadState["snapshotSerial"]=serial;m_settingsReadState["error"]=QString();emit settingsSnapshotChanged();emit settingsReadStateChanged();
        }
        if(checking){
            const bool saved=saveStatus=="unconfirmed"&&m_arguments==m_saveArguments;
            if(saved)m_settingsSaveState["savedRevision"]=m_settingsRevision;
            else m_settingsError=saveStatus=="refused"&&!m_settingsSaveState.value("error").toString().isEmpty()
                ?m_settingsSaveState.value("error").toString():tr("Launch arguments were not saved. Try again.");
            m_saveArguments.clear();settingsSaveResult(saved?"confirmed":"failed",m_settingsError);
        }
        emit settingsChanged();contextChanged();break;}
    case C::LaunchSettingsSet:{
        if(!pending||!settingsPendingCurrent(*pending))break;
        m_arguments=params["additionalArguments"].toString();m_settingsRevision=o["revision"].toString();m_settingsError.clear();
        const auto serial=QString::number(++m_settingsSnapshotSerial);
        m_settingsSnapshot={{"contextId",settingsContextId()},{"serial",serial},{"revision",m_settingsRevision},{"additionalArguments",m_arguments}};
        emit settingsSnapshotChanged();
        m_settingsSaveState["savedRevision"]=m_settingsRevision;m_saveArguments.clear();settingsSaveResult("confirmed");emit settingsChanged();
        if(m_client->launchOwned()&&!m_client->retiring()){if(!m_shuttingDown)m_model->m_restartAfterDrain=true;m_client->retireLaunch();}
        break;}
    case C::LaunchBegin:
        if(m_beginSent&&m_liveAttempt&&o["state"]!=QJsonValue("unknown"))m_beginAdmitted=true;
        acceptAttempt(o,false);break;
    case C::LaunchCommit:case C::LaunchQuery:acceptAttempt(o,false);break;
    case C::LaunchStatus:{
        const bool newer=!m_haveSequence || o["sequence"].toString().toULongLong()>m_sequence;
        if(newer)m_statusDelay=50;
        if(m_liveAttempt&&QStringList{"reviewing","preparing","verifyingPrepared","openingSteam"}.contains(o["phase"].toString()) && m_statusDelay){
            scheduleFollowUp(m_statusDelay);
            m_statusDelay=m_statusDelay<400?m_statusDelay*2:0;
        }
        if(o["phase"]==QJsonValue("unknown")){
            // Initial publication may lag the exact admitted begin receipt. This
            // does not revoke that original gesture or permit a historical one.
            if(m_beginAdmitted&&m_liveAttempt&&!m_haveSequence)break;
            m_liveGesture=false;m_liveAttempt=false;
            if(!m_statusUnavailable)m_attemptReadNeeded=true;
            m_statusUnavailable=true;break;
        }
        const auto sequence=o["sequence"].toString().toULongLong();if(m_haveSequence&&sequence<=m_sequence)break;m_haveSequence=true;m_sequence=sequence;
        m_health->observe(o["observationState"].toString());
        m_facts=o.toVariantMap();m_phase=o["phase"].toString();if(!o["error"].isNull()){const auto e=o["error"].toObject();if(e["kind"]!=QJsonValue("launchBusy"))m_error=e["kind"].toString()+": "+e["message"].toString();}
        const auto preparation=o["preparationState"].toString();if(m_committed&&preparation!="executing"&&m_phase!="preparing"&&m_phase!="verifyingPrepared"&&m_phase!="reviewing")m_client->setLaunchWriting(false);
        acceptAttempt(o["attempt"].toObject(),true);
        if(QStringList{"failed","cancelled","timedOut","completed","resolved"}.contains(m_phase)) {
            if(!m_terminalRead){m_terminalRead=true;m_barriersReadNeeded=true;}
            readView();
        }
        break;}
    case C::LaunchBarriers:{
        m_barriers=o["barriers"].toArray().toVariantList();
        const auto reason=m_attempt.value("outcome").toObject().value("reason").toString();
        const auto cancelledError=[](const QString &error){return error.isEmpty() || error.startsWith("localPreparationCancelled:");};
        const bool quietCancel=m_barriers.isEmpty() && m_attempt.value("state")==QJsonValue("terminal")
            && reason=="beforeDispatch" && (m_phase=="cancelled" || m_phase=="idle") && cancelledError(m_error) && cancelledError(m_preparationError);
        if(quietCancel){m_phase="idle";m_error.clear();m_preparationError.clear();}
        const auto state=m_attempt.value("state").toString();
        if(!m_barriers.isEmpty()&&m_binding.isEmpty()){
            // After an app restart the helper's handoff record names the launch to show.
            const auto barrier=m_barriers.first().toMap();
            m_binding={{"root",m_root},{"epoch",barrier.value("epoch").toString()},{"attemptId",barrier.value("attemptId").toString()}};
            m_committed=true;m_attemptReadNeeded=true;m_phase="reconciling";
        }
        if(m_barriers.isEmpty()&&!m_binding.isEmpty()&&state=="handoff")m_attemptReadNeeded=true;
        // Without a handoff, an attempt whose owner is gone can never reach Steam.
        const bool abandoned=state!="handoff"&&state!="terminal"&&!m_liveAttempt&&!m_liveGesture&&!m_attemptReadNeeded&&!m_client->launchOwned();
        if(m_barriers.isEmpty()&&!m_binding.isEmpty()&&!viewReadDue()&&(state=="terminal"||abandoned)){
            const bool quiet=quietCancel||reason=="startTimedOut"||reason=="gameCycleCompleted";
            m_binding={};
            if(quiet){m_phase="idle";m_attempt={};m_facts={};m_model->refresh();}
            else if(abandoned)m_phase="failed";
            if(m_client->launchOwned()){m_model->m_restartAfterDrain=true;m_client->retireLaunch();}
        }
        break;}
    case C::LaunchResolve:m_resolving={};if(o["epoch"]==m_binding.value("epoch")&&o["attemptId"]==m_binding.value("attemptId"))acceptAttempt(o,false);m_barriersReadNeeded=true;m_model->refresh();break;
    case C::LaunchOwner:
        if(o["busy"]==QJsonValue(false)){
            readView();
            if(o["active"]==QJsonValue(false)&&m_attempt.value("state")==QJsonValue("terminal"))m_client->setLaunchWriting(false);
        }
        break;
    case C::GameStatus:{const auto old=m_game.value("state").toString();m_game=o.toVariantMap();if(o["state"]==QJsonValue("running"))m_gameRunningLock=true;else if(o["state"]==QJsonValue("idle") && o["scanComplete"]==QJsonValue(true))m_gameRunningLock=false;m_gameAge.restart();if(!m_liveAttempt&&!m_barriers.isEmpty()&&old!=o["state"].toString()){m_barriersReadNeeded=true;m_barrierRetryAge.restart();QTimer::singleShot(0,this,&LaunchController::poll);}if(!m_liveAttempt&&!m_barriers.isEmpty()&&o["state"]==QJsonValue("running"))m_phase="running";QTimer::singleShot(2000,this,&LaunchController::changed);break;}
    case C::GameStop:send(C::GameStatus);break;
    case C::LaunchCancel:m_liveGesture=false;m_liveAttempt=false;m_model->m_restartAfterDrain=true;m_client->retireLaunch();break;
    default:break;
    }
}
void LaunchController::acceptAttempt(const QJsonObject &o,bool live){
    if(o["state"]==QJsonValue("unknown")){m_liveGesture=false;m_error=tr("The saved launch outcome is unknown. Its original identity has been retained.");return;}
    if(!m_attempt.isEmpty()&&o["epoch"]==m_attempt.value("epoch")&&o["attemptId"]==m_attempt.value("attemptId")&&o["recordRevision"].toString().toULongLong()<m_attempt.value("recordRevision").toString().toULongLong())return;
    if(!live&&m_statusUnavailable) {
        const auto outcome=o["outcome"].toObject()["status"].toString();
        m_phase=outcome=="observed"?"completed":outcome=="resolved"?"resolved":outcome=="cancelled"?"cancelled":outcome=="failed"?"failed":"interrupted";
    }
    m_attempt=o;
    if(m_committed&&!o["preparation"].isNull())m_preparationOutcome="succeeded";
    else if(m_committed&&o["outcome"].toObject()["reason"]==QJsonValue("preparationFailed")){m_preparationOutcome="failed";if(live)m_preparationError=m_error;}
    if(!m_committed&&m_liveGesture&&live&&o["state"]==QJsonValue("ready"))m_ready=o["ready"].toObject();
    if(o["state"]==QJsonValue("terminal")){m_liveGesture=false;m_liveAttempt=false;m_ready={};m_client->setLaunchWriting(false);if(!m_terminalRead){m_terminalRead=true;m_barriersReadNeeded=true;}}
    readView();
}
void LaunchController::scheduleFollowUp(int delay){
    if(m_followUp.isActive())delay=qMin(delay,m_followUp.remainingTime());
    m_followUp.start(qMax(0,delay));
}
void LaunchController::poll(){
    if(!available()||m_fileBusy)return;
    readView();
    // The helper settled an old handoff after the snapshot was taken; show the current state.
    if(m_binding.isEmpty()&&m_barriers.isEmpty()&&!m_barriersReadNeeded&&!m_pending&&m_model->m_launchOnlyRecovery&&!m_model->loading())m_model->refresh();
    if(m_pending){send(m_pending->retryAdmission?m_pending->call:C::LaunchControl,m_pending->retryAdmission?m_pending->params:QJsonObject{{"controlId",m_pending->params["controlId"]}});}
    else if(m_settingsReadNeeded){m_settingsReadNeeded=false;send(C::LaunchSettingsGet);}
    else if(m_attemptReadNeeded&&!m_binding.isEmpty()){m_attemptReadNeeded=false;send(C::LaunchQuery,{{"epoch",m_binding.value("epoch")},{"attemptId",m_binding.value("attemptId")}});}
    else if(m_barriersReadNeeded){m_barriersReadNeeded=false;send(C::LaunchBarriers);}
    if(m_beginAdmitted&&m_binding.contains("epoch")&&!m_statusUnavailable)send(C::LaunchStatus,{{"epoch",m_binding.value("epoch")},{"attemptId",m_binding.value("attemptId")}});
    if(m_client->launchOwned())send(C::LaunchOwner);
    send(C::GameStatus);
}
void LaunchController::open(Intent intent){m_intent=intent;if(m_epoch.isEmpty())send(C::LaunchOpen,{{"openId",freshId()}});else continueIntent();}
bool LaunchController::unsentOpenDraft()const{
    return !m_openDraft.isEmpty()&&m_openDraftGeneration==m_rootGeneration&&m_binding==m_openDraft
        &&!m_binding.contains("epoch")&&!m_beginSent&&!m_committed&&!m_fileBusy
        &&m_attempt.isEmpty()&&m_ready.isEmpty()&&m_model->m_marker.isEmpty();
}
void LaunchController::retireUnsentOpen(){
    if(!unsentOpenDraft())return;
    m_liveGesture=false;m_intent=Intent::None;m_phase="failed";
    if(!m_client->isRunning()&&!m_client->launchOwned()&&!m_client->retiring()){
        // The owner lifetime is already over; no retirement signal can follow.
        // The predicate above proves this exact local draft never reached Begin.
        m_binding={};m_openDraft={};m_attemptReadNeeded=false;
        m_statusUnavailable=true;m_poll.stop();
        m_error+=QStringLiteral("\n")+tr("Refresh to reconnect, then start a new launch.");
        return;
    }
    m_model->m_restartAfterDrain=true;m_client->retireLaunch();
}
void LaunchController::continueIntent(){const auto intent=std::exchange(m_intent,Intent::None);if(intent==Intent::Launch)beginAttempt();else if(intent==Intent::SaveSettings){
    if(m_shuttingDown||m_settingsSaveState.value("contextId").toString()!=settingsContextId()
        ||m_settingsSaveState.value("status")!="opening"||m_settingsRevision!=m_settingsSaveState.value("expectedRevision").toString()
        ||m_epoch.isEmpty()||!m_client->launchOwned()||!m_client->isRunning()||m_client->retiring()||m_client->nativeRoot()!=m_root){
        m_saveArguments.clear();settingsSaveResult("failed",tr("The settings save was not sent. Reconnect and try again."));emit settingsChanged();
        if(m_client->launchOwned()&&!m_client->retiring()){if(!m_shuttingDown)m_model->m_restartAfterDrain=true;m_client->retireLaunch();}return;
    }
    m_settingsSaveState["operationId"]=freshId();m_settingsSaveState["status"]="saving";
    send(C::LaunchSettingsSet,{{"epoch",m_epoch},{"operationId",m_settingsSaveState.value("operationId").toString()},
        {"expectedSettingsRevision",m_settingsSaveState.value("expectedRevision").toString()},{"additionalArguments",m_saveArguments}});
    emit settingsSaveStateChanged();emit settingsChanged();
    }else if(intent==Intent::Resolve){send(C::LaunchResolve,{{"epoch",m_epoch},{"journalId",m_resolving.value("journalId")},{"expectedRecordRevision",m_resolving.value("recordRevision")},{"pairSha256",m_resolving["pairSha256"]},{"steamIntentCancelled",m_resolving.value("ack")}});}}
bool LaunchController::launchActive(const QString &mode){if(m_model->bisect().value("active").toBool() && mode!="modded")return false;if(!canLaunch()||(mode!="modded"&&mode!="vanilla"))return false;m_error.clear();
    if(m_model->gamePath().isEmpty()) {
        const auto game=m_model->detectedGamePath(),root=m_root,profile=m_model->activeProfileId();
        if(game.isEmpty()){m_error=tr("Choose Deadlock's folder before playing.");update();return false;}
        auto completion=std::make_shared<QMetaObject::Connection>();
        *completion=connect(m_model,&LibraryModel::nativeChanged,this,[this,completion,game,root,profile,mode]{
            if(m_model->busy()||m_model->loading())return;
            disconnect(*completion);
            if(!m_shuttingDown && m_root==root && m_model->dataDir()==root && m_model->activeProfileId()==profile
                && m_model->gamePath()==game && m_model->error().isEmpty() && !m_model->outcomeUnknown() && !m_model->recoveryRequired())launchActive(mode);
        });
        if(!m_model->useGameFolder(game)){disconnect(*completion);return false;}
        return true;
    }
    m_profileName.clear();for(const auto &v:m_model->profiles()){const auto p=v.toMap();if(p["id"].toString()==m_model->activeProfileId())m_profileName=p["name"].toString();}
    m_binding={{"version",1},{"root",m_root},{"attemptId",freshId()},{"profileId",m_model->activeProfileId()},{"mode",mode},{"expectedRevision",m_model->revision()},{"settingsRevision",m_settingsRevision}};
    m_openDraft=m_binding;m_openDraftGeneration=m_rootGeneration;
    m_preparationOutcome="unknown";m_preparationError.clear();m_preparationViewConfirmed=false;m_liveGesture=true;m_beginSent=false;m_beginAdmitted=false;m_committed=false;m_haveSequence=false;m_statusUnavailable=false;m_terminalRead=false;m_phase="reviewing";m_ready={};m_attempt={};m_facts={};open(Intent::Launch);update();return true;}
void LaunchController::beginAttempt(){
    if(!m_liveGesture||m_shuttingDown)return;
    m_openDraft={};
    m_binding["epoch"]=m_epoch;
    if(!contextCurrent()){m_liveGesture=false;m_binding={};m_error=tr("The active setup changed. Start a fresh launch review.");return;}
    m_liveAttempt=true;m_beginSent=true;
    send(C::LaunchBegin,{{"epoch",m_binding.value("epoch")},{"attemptId",m_binding.value("attemptId")},{"profileId",m_binding.value("profileId")},{"mode",m_binding.value("mode")},{"expectedRevision",m_binding.value("expectedRevision")},{"settingsRevision",m_binding.value("settingsRevision")}});
}
bool LaunchController::commit(){
    if(!canCommit()||m_model->busy()||!m_model->m_marker.isEmpty()||m_model->shuttingDown())return false;
    m_health->capture(m_binding["attemptId"].toString(),m_binding["mode"].toString());m_committed=true;m_phase="preparing";m_client->setLaunchWriting(true);
    send(C::LaunchCommit,{{"epoch",m_binding["epoch"]},{"attemptId",m_binding["attemptId"]},{"reviewToken",m_ready["reviewToken"]},{"preparationOperationId",m_ready["preparationOperationId"]},{"preparationInput",m_ready["preparationInput"]},{"settingsRevision",m_binding["settingsRevision"]}});
    update();return true;
}
bool LaunchController::cancel(){
    if(!canCancel()&&!canStopWaiting())return false;
    m_liveGesture=false;
    if(!m_beginSent&&!m_committed){
        m_phase="idle";m_intent=Intent::None;m_openDraft={};
        if(!m_fileBusy)m_binding={};
        m_model->m_restartAfterDrain=true;m_client->retireLaunch();
    }else send(C::LaunchCancel,{{"epoch",m_binding.value("epoch")},{"attemptId",m_binding.value("attemptId")}});
    update();return true;
}
bool LaunchController::checkAgain(){if(!available()||busy())return false;m_liveGesture=false;m_attemptReadNeeded=m_binding.contains("epoch");m_barriersReadNeeded=true;poll();if(!m_model->m_marker.isEmpty())m_model->reconcileOperation();return true;}
bool LaunchController::stopGame(const QString &token){if(!canStop()||!LaunchData::hex(token,32))return false;bool eligible=false;for(const auto &v:m_game.value("processes").toList()){const auto p=v.toMap();if(p.value("sessionToken").toString()==token&&p.value("canStop").toBool())eligible=true;}if(!eligible)return false;m_health->stopped();send(C::GameStop,{{"sessionToken",token}});return true;}
bool LaunchController::readSettings(){if(!available()||busy()||(settingsSavePending()&&m_settingsSaveState.value("status")!="unconfirmed"&&m_settingsSaveState.value("status")!="refused"))return false;send(C::LaunchSettingsGet);return true;}
bool LaunchController::saveSettings(const QString &args){
    if(!available()||busy()||writeGate()||m_model->busy()||m_model->outcomeUnknown()||m_model->recoveryRequired()||m_settingsRevision.isEmpty()||settingsSavePending()||LaunchData::settingsReceipt(args,m_settingsRevision).isEmpty())return false;
    const auto generation=m_rootGeneration;const auto attempt=freshId();
    m_saveArguments=args;m_intent=Intent::SaveSettings;
    m_settingsSaveState={{"contextId",settingsContextId()},{"attemptId",attempt},{"operationId",QString()},{"expectedRevision",m_settingsRevision},{"savedRevision",QString()},{"status",QStringLiteral("opening")},{"error",QString()}};
    emit settingsSaveStateChanged();emit settingsChanged();update();
    if(generation!=m_rootGeneration||m_shuttingDown||m_intent!=Intent::SaveSettings||m_settingsSaveState.value("attemptId")!=attempt)return true;
    m_liveGesture=false;open(Intent::SaveSettings);
    if(m_intent==Intent::SaveSettings&&(!m_pending||m_pending->call!=C::LaunchOpen))abandonSettingsOpen(tr("The settings save was not sent. Reconnect before trying again."));return true;
}
bool LaunchController::reconcileSettings(){
    if(m_shuttingDown||busy()||m_root.isEmpty()||!m_model->nativeMode()||m_model->dataDir()!=m_root||!AppPaths::stateWriteError().isEmpty())return false;
    if(!available()){m_model->refresh();return true;}
    return readSettings();
}
bool LaunchController::queryBarriers(){if(!available()||busy())return false;send(C::LaunchBarriers);return true;}
bool LaunchController::resolveBarrier(bool ack){if(!canResolve())return false;m_liveGesture=false;m_resolving=QJsonObject::fromVariantMap(m_barriers.first().toMap());m_resolving["ack"]=ack;open(Intent::Resolve);return true;}
bool LaunchController::retire(){if(!available()||m_model->writeBusy()||m_client->writing()||m_client->launchWriting()||m_fileBusy)return false;
    // Keep the owner alive until terminal reconciliation has read the attempt and barriers.
    // The regular poll sends those reads; polling here would loop through changed().
    if(m_terminalRead&&(m_attemptReadNeeded||m_barriersReadNeeded||m_pending))return false;
    m_liveGesture=false;m_model->m_restartAfterDrain=true;send(C::LaunchClose);return true;}
void LaunchController::shutdown(){
    if(m_shuttingDown)return;m_shuttingDown=true;
    if(m_intent==Intent::SaveSettings)abandonSettingsOpen(tr("The settings save was not sent before closing."));
    if(!m_beginSent&&!m_committed&&!m_fileBusy)m_binding={};m_liveGesture=false;m_poll.stop();m_intent=Intent::None;
    if(m_client->remoteOwned() && m_client->launchOwned())m_client->retireLaunch();
    else if(!m_client->remoteOwned() && !m_model->remoteMarkerPending())m_client->stop();update();
}
void LaunchController::transportLost(const QString &message){
    if(m_pending&&m_pending->call==C::LaunchSettingsGet)settingsReadResult("unconfirmed",message);
    if(m_intent==Intent::SaveSettings)abandonSettingsOpen(tr("The settings save was not sent. Reconnect before trying again."));
    else if(settingsSavePending()){
        m_settingsReadNeeded=true;if(!m_shuttingDown&&m_client->retiring())m_model->m_restartAfterDrain=true;
        if(m_settingsSaveState.value("status")!="refused")settingsSaveResult("unconfirmed",tr("Reconnect to check the saved arguments."));emit settingsChanged();
    }
    m_liveGesture=false;m_liveAttempt=false;m_beginAdmitted=false;m_ready={};m_pending.reset();m_inFlight.clear();m_epoch.clear();m_error=message;m_phase="reconciling";m_statusUnavailable=true;m_poll.stop();update();
}
void LaunchController::contextChanged(){if(m_liveGesture&&!m_committed&&!contextCurrent()){m_error=tr("The active setup changed. Start a fresh launch review.");cancel();}update();}
bool LaunchController::viewReadDue()const{return m_committed&&!m_preparationViewConfirmed&&(m_attempt["preparation"].isObject()||m_attempt["state"]==QJsonValue("terminal"));}
void LaunchController::readView(){
    // One snapshot shows what the attempt left. A refused, lost or stale one is asked again.
    if(!viewReadDue()||(m_viewReadSent&&m_client->snapshotPending())||m_shuttingDown||!m_client->readyForRequests()||m_client->retiring()||m_client->writing())return;
    m_viewReadSent=true;m_model->setLoading(true);
    m_client->requestSnapshot(m_model->m_statePath,m_model->m_requestedProfileId.isEmpty()?m_model->viewedProfileId():m_model->m_requestedProfileId);
}
bool LaunchController::viewRead(const QString &revision){
    if(!std::exchange(m_viewReadSent,false)||!viewReadDue())return false;
    // The floor is the attempt's own result, or the revision it began from. Without either, nothing confirms.
    const auto preparation=m_attempt.value("preparation");
    const auto floor=preparation.isObject()?preparation["resultingRevision"]:m_binding.value("expectedRevision");
    if(!LaunchData::decimal(floor)||revision.toULongLong()<floor.toString().toULongLong())return false;
    m_preparationViewConfirmed=true;if(m_preparationOutcome=="failed")m_liveGesture=false;
    m_client->setLaunchWriting(false);m_barriersReadNeeded=true;update();return true;
}
#include "launchcontroller.moc"
