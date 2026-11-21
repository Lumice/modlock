#include "maintenancecontroller.h"
#include "librarymodel.h"
#include <QCoreApplication>
#include <QProcess>
#include <QJsonDocument>
#include <QJsonObject>
#include <QUuid>
#include <memory>
LibraryModel *MaintenanceController::library() const {return m_library;}
void MaintenanceController::setLibrary(LibraryModel *l) {
    if(m_library==l)return;
    if(m_library)disconnect(m_library,nullptr,this,nullptr);
    m_library=l;m_result.clear();m_freedBytes.clear();
    if(l)connect(l,&LibraryModel::nativeChanged,this,[this]{
        if(m_library && m_result.value("revision").toString()!=m_library->revision()){m_result.clear();emit changed();}
    });
    if(l)connect(l,&LibraryModel::loadingChanged,this,[this]{
        if(m_rescan && m_library && !m_library->loading() && !m_busy){m_rescan=false;scan();}
    });
    emit changed();
}
void MaintenanceController::scan(){request(false);}
void MaintenanceController::clean(){if(m_result.value("canClean").toBool()||m_result.value("resume").toBool())request(true);}
void MaintenanceController::request(bool clean) {
    if(!m_library||m_busy||!m_library->nativeMode()||m_library->busy()||m_library->loading()||!m_library->helperReady())return;
    const auto root=m_library->dataDir();const auto epoch=m_library->loadEpoch();const auto revision=m_library->revision();
    const auto helper=m_library->helperPath();
    QJsonObject params{{"dataDir",root}};
    if(clean){params.insert("operationId",QUuid::createUuid().toString(QUuid::Id128));params.insert("expectedRevision",m_result.value("revision").toString());params.insert("token",m_result.value("token").toString());}
    const auto cleanupBytes=m_result.value("bytes");
    const auto wire=QJsonDocument(QJsonObject{{"jsonrpc","2.0"},{"id",1},{"method",clean?"native.maintenance.clean":"native.maintenance.scan"},{"params",params}}).toJson(QJsonDocument::Compact)+'\n';
    m_busy=true;m_error.clear();if(clean)m_freedBytes.clear();emit changed();
    auto *process=new QProcess(QCoreApplication::instance());auto bytes=std::make_shared<QByteArray>();
    QPointer<MaintenanceController> self(this);QPointer<LibraryModel> library(m_library);
    connect(process,&QProcess::readyReadStandardOutput,process,[process,bytes]{const auto chunk=process->readAllStandardOutput();if(bytes->size()<=32*1024*1024)bytes->append(chunk);});
    connect(process,&QProcess::readyReadStandardError,process,[process]{process->readAllStandardError();});
    auto done=[self,library,process,bytes,root,epoch,revision,clean,cleanupBytes]{
        process->deleteLater();const bool current=library&&library->dataDir()==root&&library->loadEpoch()==epoch;
        const auto response=QJsonDocument::fromJson(*bytes).object();
        const bool ok=process->exitStatus()==QProcess::NormalExit&&process->exitCode()==0&&response.value("id").toInt()==1&&response.contains("result")&&bytes->size()<=32*1024*1024;
        if(clean&&current){if(self)self->m_rescan=ok;library->refresh();}
        if(!self)return;self->m_busy=false;self->m_result.clear();
        if(clean&&ok&&current)self->m_freedBytes=cleanupBytes;
        if(!current)self->m_error=tr("The library changed. Check again.");
        else if(!ok)self->m_error=response.value("error").toObject().value("message").toString(tr("The helper did not return a result. Check again before retrying."));
        else if(!clean&&library->revision()!=revision)self->m_error=tr("The library changed. Check again.");
        else if(!clean)self->m_result=response.value("result").toObject().toVariantMap();
        emit self->changed();
        if(clean&&ok&&current)QMetaObject::invokeMethod(self,[self]{if(self && self->m_library && !self->m_library->loading()){self->m_rescan=false;self->scan();}},Qt::QueuedConnection);
    };
    connect(process,qOverload<int,QProcess::ExitStatus>(&QProcess::finished),process,[done](int,QProcess::ExitStatus){done();});
    connect(process,&QProcess::errorOccurred,process,[done](QProcess::ProcessError e){if(e==QProcess::FailedToStart)done();});
    connect(process,&QProcess::started,process,[process,wire]{process->write(wire);process->closeWriteChannel();});
    process->start(helper,{});
}
