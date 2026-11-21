#include "apppaths.h"
#include "launchhealth.h"
#include "librarymodel.h"
#include <QCryptographicHash>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSaveFile>
#include <QTimer>
#include <QStandardPaths>
LaunchHealth::LaunchHealth(LibraryModel *model):QObject(model),m_model(model){
 connect(model,&LibraryModel::nativeChanged,this,[this]{
 if(!m_report.value("confirmed").toBool()||m_report.value("bisectAnswered").toBool()||m_model->busy())return;
 QTimer::singleShot(0,this,[this]{if(m_report.value("confirmed").toBool()&&!m_report.value("bisectAnswered").toBool()&&!m_model->busy())answer(true);});
 });
}
QString LaunchHealth::classify(qint64 seconds,bool observed,bool abnormal,bool stopped){
 if(!observed||stopped)return "ignored";
 if(abnormal)return "crash";
 // Long sessions are a useful baseline, without claiming how the game exited.
 return seconds>=300?"normal":seconds>=0?"ambiguous":"unknown";
}
void LaunchHealth::load(const QString &root){
 m_root=root;m_attempt.clear();m_seen=false;m_finished=false;m_uptime.invalidate();m_launch.clear();m_good.clear();m_report.clear();m_error.clear();m_enabled=true;
 const auto key=QCryptographicHash::hash(root.toUtf8(),QCryptographicHash::Sha256).toHex();
 m_file=QDir(AppPaths::stateRoot()).filePath("launch-health/"+key+".json");
 if(!AppPaths::stateWriteError().isEmpty()){emit changed();return;}
 QFile f(m_file);if(f.open(QIODevice::ReadOnly)&&f.size()<=4*1024*1024){const auto d=QJsonDocument::fromJson(f.readAll()).object().toVariantMap();if(d.value("root").toString()==root){m_enabled=d.value("enabled",true).toBool();m_good=d.value("good").toMap();m_report=d.value("report").toMap();m_launch=d.value("launch").toMap();if(!m_report.value("confirmed").toBool()&&m_report.value("classification").toString()!="crash")m_report.clear();}}
 emit changed();
}
void LaunchHealth::save(){
 if(m_file.isEmpty())return;
 m_error=AppPaths::stateWriteError();if(!m_error.isEmpty())return;
 if(!AppPaths::prepareState()){m_error=AppPaths::settingsFolderError();return;}
 QDir().mkpath(QFileInfo(m_file).absolutePath());QSaveFile f(m_file);
 const auto bytes=QJsonDocument(QJsonObject::fromVariantMap({{"root",m_root},{"enabled",m_enabled},{"good",m_good},{"launch",m_launch},{"report",m_report}})).toJson(QJsonDocument::Compact);
 if(!f.open(QIODevice::WriteOnly)||f.write(bytes)!=bytes.size()||!f.commit())m_error=tr("Couldn't save session history. Check free disk space.");
}
void LaunchHealth::setEnabled(bool enabled){m_enabled=enabled;if(!enabled)m_report.clear();save();emit changed();}
QVariantMap LaunchHealth::snapshot()const{
 QVariantList mods;for(const auto &entry:m_model->fullProfileOrder()){
 const auto row=m_model->modDetails(entry.toMap().value("id").toString());
 mods.append(QVariantMap{{"position",mods.size()},{"id",row.value("id")},{"title",row.value("title")},{"enabled",row.value("enabled")},{"order",row.value("installOrder")},{"variants",row.value("selectedOptionIds")},{"repairs",row.value("fixEnabled")}});}
 auto digest=[](const QString &s){return QString::fromLatin1(QCryptographicHash::hash(s.toUtf8(),QCryptographicHash::Sha256).toHex());};
 return {{"fingerprint",m_model->launchFingerprint()},{"profile",m_model->activeProfileId()},{"mods",mods},{"modsKnown",m_model->activeProfileId()==m_model->viewedProfileId()},{"repairs",m_model->fixAutomatically()},{"performance",m_model->launchFingerprint().contains("performance")?m_model->launchFingerprint().value("performance"):(!m_model->performance()->busy()&&!m_model->performance()->presets().isEmpty()?QVariant(m_model->performance()->selection()):QVariant())},
 {"autoexec",!m_model->autoexec()->path().isEmpty()&&!m_model->autoexec()->busy()?digest(m_model->autoexec()->text()):QString()},
 {"crosshair",!m_model->crosshair()->path().isEmpty()&&!m_model->crosshair()->busy()?digest(m_model->crosshair()->text()):QString()},
 {"autoexecText",m_model->autoexec()->text()},{"crosshairValues",m_model->crosshair()->savedValues()},{"revision",m_model->revision()},{"bisect",m_model->bisect()}};
}
void LaunchHealth::capture(const QString &attempt,const QString &mode){
 m_attempt=attempt;m_launch=snapshot();m_launch["mode"]=mode;m_seen=false;m_finished=false;m_uptime.invalidate();m_report.clear();save();emit changed();
}
QVariantList LaunchHealth::differences(const QVariantMap &before,const QVariantMap &after){
 QVariantList result;if(before.isEmpty())return result;
 auto add=[&](const QString &kind,const QString &label,const QVariant &old,const QVariant &now,const QString &id=QString()){
 if(old!=now)result.append(QVariantMap{{"kind",kind},{"text",label},{"before",old},{"after",now},{"id",id}});};
 QMap<QString,QVariantMap> previous,current;
 for(const auto &v:before.value("mods").toList())previous[v.toMap().value("id").toString()]=v.toMap();
 for(const auto &v:after.value("mods").toList())current[v.toMap().value("id").toString()]=v.toMap();
 const auto oldFingerprint=before.value("fingerprint").toMap(),newFingerprint=after.value("fingerprint").toMap();
 for(const auto &key:{"generation","gameBuild"})if(oldFingerprint.value(key).isValid()&&newFingerprint.value(key).isValid()&&!oldFingerprint.value(key).isNull()&&!newFingerprint.value(key).isNull())add(key,QString::fromLatin1(key)=="gameBuild"?tr("Deadlock was updated"):tr("Changed applied mod generation"),oldFingerprint.value(key),newFingerprint.value(key));
 auto keys=previous.keys();for(const auto &id:current.keys())if(!keys.contains(id))keys.append(id);
 if(!before.value("modsKnown",true).toBool()||!after.value("modsKnown",true).toBool())keys.clear();
 for(const auto &id:keys){const auto a=previous.value(id),b=current.value(id);const auto title=b.value("title",a.value("title",id)).toString();
 add("enabled",b.value("enabled").toBool()?tr("Enabled %1").arg(title):tr("Disabled or removed %1").arg(title),a.value("enabled",false),b.value("enabled",false),id);
 if(!a.isEmpty()&&!b.isEmpty()){
 add("variants",tr("Changed options for %1").arg(title),a.value("variants").toStringList(),b.value("variants").toStringList(),id);
 add("order",tr("Changed load order for %1").arg(title),a.value("position",a.value("order")),b.value("position",b.value("order")),id);
 add("fix",tr("Changed repairs for %1").arg(title),a.value("repairs"),b.value("repairs"),id);}}
 for(const auto &key:{"repairs","performance","autoexec","crosshair","mode"}){
 const auto k=QString::fromLatin1(key);if(!before.value(k).isValid()||!after.value(k).isValid()||before.value(k).isNull()||after.value(k).isNull())continue;
 if((k=="autoexec"||k=="crosshair")&&(before.value(k).toString().isEmpty()||after.value(k).toString().isEmpty()))continue;
 const QString label=k=="repairs"?tr("Changed automatic repairs"):k=="performance"?tr("Changed performance preset"):k=="autoexec"?tr("Changed autoexec.cfg"):k=="crosshair"?tr("Changed crosshair settings"):tr("Changed launch mode");
 add(k,label,before.value(k),after.value(k));}
 return result;
}
void LaunchHealth::observe(const QString &state){
 if(m_attempt.isEmpty()||m_finished)return;
 if(state=="running")m_launch["fingerprint"]=m_model->launchFingerprint();
 if(state=="running"&&!m_seen){m_seen=true;m_uptime.start();}
 if(state!="exited")return;
 m_finished=true;if(!m_enabled||!m_seen)return;
 const auto classification=classify(m_uptime.elapsed()/1000,true);
 if(classification=="normal"){m_good=m_launch;m_report.clear();save();emit changed();return;}
 const auto changes=differences(m_good,m_launch);
 if(m_good.isEmpty()||changes.isEmpty()){m_report.clear();save();emit changed();return;}
 m_report={{"classification",classification},{"presentation","notice"},{"confirmed",false},{"changes",changes},{"hasBaseline",true},
 {"early",true},{"profile",m_launch.value("profile")}};
 save();emit changed();
}
void LaunchHealth::answer(bool crashed){
 if(m_report.isEmpty())return;
 if(!crashed){m_good=m_launch;m_report.clear();}
 else {m_report["confirmed"]=true;
 const auto round=m_launch.value("bisect").toMap(),live=m_model->bisect();
 if(round.value("active").toBool()&&round==live&&!m_report.value("bisectAnswered").toBool()){
 if(m_model->answerBisect("broken")){m_report["bisectAnswered"]=true;m_report["answerRevision"]=QString::number(m_model->revision().toULongLong()+1);}
 }}
 save();emit changed();
}
bool LaunchHealth::changeBisectAnswer(){
 const auto round=m_launch.value("bisect").toMap(),live=m_model->bisect();
 if(m_model->revision()!=m_report.value("answerRevision").toString()||!m_report.value("bisectAnswered").toBool()||!live.value("active").toBool()||live.value("profileId")!=round.value("profileId")||live.value("round").toInt()>round.value("round").toInt()+1)return false;
 if(!m_model->answerBisect("undo"))return false;
 m_report["bisectAnswered"]=false;m_report["confirmed"]=false;save();emit changed();return true;
}
void LaunchHealth::dismiss(){m_report.clear();save();emit changed();}
bool LaunchHealth::undo(int index){
 const auto changes=m_report.value("changes").toList();if(index<0||index>=changes.size()||m_model->viewedProfileId()!=m_model->activeProfileId()||m_model->activeProfileId()!=m_launch.value("profile").toString()||m_root!=m_model->dataDir()||m_model->busy())return false;
 const auto c=changes[index].toMap();const auto k=c.value("kind").toString(),id=c.value("id").toString();
 const auto row=m_model->modDetails(id);
 if(k=="enabled"&&row.value("enabled")==c.value("after"))return m_model->setModEnabled(id,c.value("before").toBool());
 if(k=="fix"&&row.value("fixEnabled")==c.value("after"))return m_model->setFixEnabled(id,c.value("before").toBool());
 if(k=="repairs"&&m_model->fixAutomatically()==c.value("after").toBool())return m_model->setFixAutomatically(c.value("before").toBool());
 if(k=="performance"&&m_model->performance()->selection()==c.value("after").toString())return m_model->performance()->select(c.value("before").toString());
 if(k=="autoexec"&&m_launch.value("autoexecText")==m_model->autoexec()->text()){
 const bool ok=m_model->autoexec()->preview(m_good.value("autoexecText").toString());if(ok)emit configReviewRequested(k);return ok;}
 if(k=="crosshair"&&m_launch.value("crosshairValues")==m_model->crosshair()->savedValues()){
 const auto values=m_good.value("crosshairValues").toMap();for(auto it=values.begin();it!=values.end();++it)if(!m_model->crosshair()->setValue(it.key(),it.value()))return false;
 const bool ok=m_model->crosshair()->preview();if(ok)emit configReviewRequested(k);return ok;}
 if(k=="variants"&&row.value("selectedOptionIds").toStringList()==c.value("after").toStringList()){
 auto options=m_model->options();if(!options->open(id))return false;
 auto timer=new QTimer(this);timer->setInterval(50);const auto root=m_root,attempt=m_attempt;const auto wanted=c.value("before").toStringList();
 connect(timer,&QTimer::timeout,this,[this,timer,options,root,attempt,id,wanted,ticks=0]()mutable{
 if(root!=m_root||attempt!=m_attempt||!options->visible()||options->modId()!=id||++ticks>100){timer->deleteLater();return;}
 if(options->busy()||options->rows().isEmpty())return;
 timer->stop();timer->deleteLater();
 QSet<QString> ready;for(const auto &v:options->rows()){const auto r=v.toMap();if(r.value("state")=="ready")ready.insert(r.value("id").toString());}
 for(const auto &option:wanted)if(!ready.contains(option)){m_error=tr("Previous options are unavailable. Review the available choices.");emit changed();return;}
 const auto selected=options->selectedIds();for(const auto &option:selected)if(!wanted.contains(option)&&!options->setSelected(option,false))return;
 for(const auto &option:wanted)if(!options->setSelected(option,true))return;
 options->previewSelection();
 });timer->start();return true;
 }
 if(k=="order"){
 auto options=m_model->options();if(!options->openOrder())return false;
 const auto order=options->order();int at=-1;for(int i=0;i<order.size();++i)if(order[i].toMap().value("id").toString()==id)at=i;
 const int target=c.value("before").toInt();if(at<0||target<0||target>=order.size())return false;
 while(at!=target){const int step=target>at?1:-1;if(!options->move(id,step))return false;at+=step;}
 return options->previewOrder();
 }
 m_error=tr("This change needs review in its settings page. No changes were made.");emit changed();return false;
}
