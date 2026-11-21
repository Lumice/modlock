#include "storagecontroller.h"
#include "apppaths.h"
#include "librarymodel.h"
#include <QCoreApplication>
#include <QProcess>
#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>
#include <QUuid>
#include <memory>
LibraryModel *StorageController::library() const {return m_library;}
QString StorageController::error() const {
    if(m_error.isEmpty())return m_statusError;
    return m_statusError.isEmpty() ? m_error : m_error+'\n'+m_statusError;
}
void StorageController::setLibrary(LibraryModel *l) {
    if(m_library==l)return;
    if(m_library)disconnect(m_library,nullptr,this,nullptr);
    m_library=l;m_epoch=l ? l->loadEpoch() : 0;
    m_result.clear();m_statusError.clear();m_rescan=true;
    if(l)connect(l,&LibraryModel::nativeChanged,this,[this, observedRevision=l->revision()]() mutable {
        if(!m_library)return;
        const auto revision=m_library->revision();
        // A rejected status reply leaves no result to invalidate. Retry once
        // the model moves on, even if the helper observed the change first.
        const bool stale=m_epoch!=m_library->loadEpoch() || observedRevision!=revision ||
            (m_result.contains("revision") && m_result.value("revision").toString()!=revision);
        observedRevision=revision;
        if(stale){
            m_epoch=m_library->loadEpoch();
            m_result.clear();m_statusError.clear();m_rescan=true;
        }
        queueScan();
        if(stale)emit changed();
    });
    emit changed();
}
void StorageController::queueScan(){
    if(m_rescan && !m_busy && !m_statusProcess && m_library && m_library->nativeMode() &&
       !m_library->loading() && !m_library->busy() && m_library->helperReady())
        QMetaObject::invokeMethod(this,[this]{if(m_rescan)scan();},Qt::QueuedConnection);
}
void StorageController::scan(){request("status");}
void StorageController::review(const QString &folder){request("review", folder);}
void StorageController::move(){request("move");}
void StorageController::resume(){request("resume");}
void StorageController::cancel(){request("cancel");}
void StorageController::request(const QString &method, const QString &folder) {
    const bool status=method=="status";
    const bool clean=method=="move" || method=="resume" || method=="cancel";
    if(clean) {
        const auto stateError=AppPaths::stateWriteError();
        if(!stateError.isEmpty()){m_statusError.clear();m_error=stateError;emit changed();return;}
        if(method=="move" && !m_result.contains("token"))return;
    }
    if(!m_library||m_busy||(status && m_statusProcess)||!m_library->nativeMode()||m_library->busy()||m_library->loading()||!m_library->helperReady())return;
    const auto root=m_library->dataDir();const auto epoch=m_library->loadEpoch();const auto revision=m_library->revision();
    const auto helper=m_library->helperPath();
    QJsonObject params{{"dataDir",root}};
    if(clean){params.insert("operationId",QUuid::createUuid().toString(QUuid::Id128));params.insert("expectedRevision",m_result.value("revision").toString());params.insert("token",m_result.value("token").toString());}
    if(method=="review")params.insert("destination", folder);
    if(method=="move")params.insert("destination",m_result.value("destination").toMap().value("path").toString());
    const auto wire=QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method","native.storage."+method},{"params",params}}).toJson(QJsonDocument::Compact)+'\n';
    // Writes retain application ownership. A status read can still be stuck
    // in kernel I/O after kill(), so reap it asynchronously without blocking
    // destruction of the Settings page or the application.
    auto *process=new QProcess(status ? nullptr : QCoreApplication::instance());
    auto *deadline=status ? new QTimer(process) : nullptr;
    auto bytes=std::make_shared<QByteArray>();
    QPointer<StorageController> self(this);QPointer<LibraryModel> library(m_library);
    m_busy=true;m_rescan=false;m_statusError.clear();m_progress=-1;m_phase=tr("Checking files…");
    if(status){
        m_statusProcess=process;
        connect(this,&QObject::destroyed,process,&QProcess::kill);
        connect(QCoreApplication::instance(),&QCoreApplication::aboutToQuit,process,&QProcess::kill);
    }else{
        m_error.clear();
        library->setBusy(true,clean,tr("Checking library storage…"));
    }
    connect(process,&QProcess::readyReadStandardOutput,process,[process,bytes]{const auto chunk=process->readAllStandardOutput();if(bytes->size()<=32*1024*1024)bytes->append(chunk);});
    auto progressBytes=std::make_shared<QByteArray>();
    connect(process,&QProcess::readyReadStandardError,process,[self,process,progressBytes,status]{
        const auto chunk=process->readAllStandardError();
        if(status)return;
        progressBytes->append(chunk);
        while(progressBytes->contains('\n')) {
            auto line=progressBytes->first(progressBytes->indexOf('\n'));progressBytes->remove(0,line.size()+1);
            const auto update=QJsonDocument::fromJson(line).object();if(!self || !update.contains("storageProgress"))continue;
            const auto phase=update["storageProgress"].toString();
            self->m_phase=phase=="copy"?tr("Copying and checking files…"):phase=="switch"?tr("Switching storage location…"):tr("Removing verified old copies…");
            const auto total=update["total"].toDouble();self->m_progress=total>0?qBound(0.,update["done"].toDouble()/total,1.):-1;emit self->changed();
        }
        if(progressBytes->size()>4096)progressBytes->clear();
    });
    auto done=[self,library,process,deadline,bytes,root,epoch,revision,status,clean,method](bool timedOut){
        if(!timedOut){
            process->deleteLater();
            if(status){
                if(self)self->m_statusProcess.clear();
                // The timer already published the result if it fired first.
                if(!deadline->isActive()){if(self)self->queueScan();return;}
                deadline->stop();
            }
        }
        const bool modelCurrent=library&&library->dataDir()==root&&library->loadEpoch()==epoch;
        const auto response=timedOut ? QJsonObject{} : QJsonDocument::fromJson(*bytes).object();
        const auto result=response.value("result").toObject();
        const bool ok=!timedOut&&process->exitStatus()==QProcess::NormalExit&&process->exitCode()==0&&response.value("id").toInt()==1&&response.contains("result")&&bytes->size()<=32*1024*1024;
        if(!status&&modelCurrent)library->setBusy(false,false,{});
        if(clean&&modelCurrent){if(self)self->m_rescan=true;library->refresh();}
        if(!self)return;self->m_busy=false;self->m_result.clear();
        if(clean && !ok){self->m_result.insert("resume",true);self->m_result.insert("canCancel",false);}

        const bool current=modelCurrent && self->m_library==library;
        auto &error=status ? self->m_statusError : self->m_error;
        if(!current || (status && library->revision()!=revision)){
            error=tr("The library changed. Check again.");
            if(status)self->m_rescan=true;
        }
        else if(timedOut)error=tr("The storage check timed out. Check the drive, then reopen Settings to try again.");
        else if(!ok)error=response.value("error").toObject().value("message").toString(tr("The helper did not return a result. Check again before retrying."));
        else if(status && result.value("revision").toString()!=revision)error=tr("The library changed. Refresh it before checking storage again.");
        else if(!clean&&library->revision()!=revision)error=tr("The library changed. Check again.");
        else if(!clean){
            self->m_result=result.toVariantMap();
            if(self->m_result.contains("source"))self->m_result.insert("current",self->m_result.value("source"));
        }
        if(ok && method=="cancel") {
            self->m_notice=result.value("message").toString();
        }
        self->queueScan();
        emit self->changed();
    };
    connect(process,qOverload<int,QProcess::ExitStatus>(&QProcess::finished),process,[done](int,QProcess::ExitStatus){done(false);});
    connect(process,&QProcess::errorOccurred,process,[done](QProcess::ProcessError e){if(e==QProcess::FailedToStart)done(false);});
    connect(process,&QProcess::started,process,[process,wire]{process->write(wire);process->closeWriteChannel();});
    if(deadline){
        deadline->setSingleShot(true);
        connect(deadline,&QTimer::timeout,process,[done,process]{process->kill();done(true);});
        deadline->start(10000);
    }
    process->start(helper,{});
    if(self)emit self->changed();
}
