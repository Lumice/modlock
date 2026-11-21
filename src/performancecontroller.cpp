#include "performancecontroller.h"
#include "librarymodel.h"
#include "backendclient.h"
PerformanceController::PerformanceController(LibraryModel *library):QObject(library),m_library(library){
 m_retryTimer.setSingleShot(true);
 connect(&m_retryTimer,&QTimer::timeout,this,[this]{
  if(!m_retryLaunchBusy)return;
  if(m_root!=m_library->dataDir()||!available()){m_retryLaunchBusy=false;emit changed();return;}
  if(!m_library->m_client->isRunning()||m_library->m_client->retiring()||m_library->m_client->launchOwned()||m_library->busy()||m_library->loading()){m_retryTimer.start(500);return;}
  m_retryLaunchBusy=false;refresh();
 });
 connect(library,&LibraryModel::nativeChanged,this,[this]{
  const auto context=m_library->dataDir()+"|"+m_library->revision()+"|"+m_library->gamePath()+"|"+QString::number(m_library->m_client->sessionGeneration());
  if(available()&&!m_library->busy()&&!m_wait&&context!=m_context)refresh();else emit changed();
 });
 connect(library,&LibraryModel::loadingChanged,this,&PerformanceController::changed);
 connect(library->m_client,&BackendClient::capabilitiesReady,this,[this]{m_wait=false;m_data.clear();refresh();});
 connect(library->m_client,&BackendClient::performanceReady,this,[this](const QVariantMap &data){m_wait=false;m_retryLaunchBusy=false;m_retryTimer.stop();if(m_root==m_library->dataDir()){m_data=data;m_error.clear();}emit changed();});
 connect(library->m_client,&BackendClient::rejected,this,[this](BackendClient::Call call,const QString &kind,const QString &error){
  if(call!=BackendClient::Call::PerformanceGet&&call!=BackendClient::Call::PerformanceSelect)return;
  m_wait=false;
  if(call==BackendClient::Call::PerformanceGet&&(kind=="launchBusy"||(kind=="unsupportedAction"&&m_library->m_client->launchOwned()))){m_retryLaunchBusy=true;m_error.clear();m_retryTimer.start(500);}
  else {m_retryLaunchBusy=false;m_retryTimer.stop();m_error=error;}
  emit changed();
 });
 connect(library->m_client,&BackendClient::failed,this,[this](const QString &e){m_wait=false;m_retryLaunchBusy=false;m_retryTimer.stop();m_error=e;emit changed();});
}
bool PerformanceController::available()const{return m_library->nativeMode()&&m_library->m_client->readyForRequests();}
bool PerformanceController::busy()const{return m_wait||m_retryLaunchBusy||m_library->busy()||m_library->loading();}
void PerformanceController::refresh(){
 if(m_root!=m_library->dataDir()){m_data.clear();m_wait=false;}
 if(!available()||m_wait||m_library->busy()) {emit changed();return;}
 m_root=m_library->dataDir();m_context=m_root+"|"+m_library->revision()+"|"+m_library->gamePath()+"|"+QString::number(m_library->m_client->sessionGeneration());m_wait=true;m_library->m_client->nativeRequest(BackendClient::Call::PerformanceGet);emit changed();
}
QVariantList PerformanceController::preview(const QString &id)const{for(const auto &v:presets()){const auto p=v.toMap();if(p.value("id").toString()==id)return p.value("entries").toList();}return {};}
bool PerformanceController::select(const QString &id){
 if(!available()||busy()||!m_library->profileEditsAvailable())return false;
 if(!id.isEmpty()&&preview(id).isEmpty())return false;
 m_library->m_performanceApplyNeedsReview = m_library->m_pendingStatus != "current" || m_library->m_pendingChanges || m_library->externalGameinfoChange();
 return m_library->mutate(int(BackendClient::Call::PerformanceSelect),{{"presetId",id},{"profileId",m_library->viewedProfileId()}},tr("Saving performance config selection…"));
}
