#include "launchdata.h"
#include "optiondata.h"
#include <QCryptographicHash>
#include <QDir>
#include <QJsonArray>
#include <QJsonDocument>
#include <algorithm>
#include <cmath>
#include <limits>
#include <functional>

namespace LaunchData {
namespace {
using C = BackendClient::Call;
bool keys(const QJsonObject &o, std::initializer_list<const char *> names) {
    if (o.size() != qsizetype(names.size())) return false;
    return std::all_of(names.begin(), names.end(), [&o](const char *n) { return o.contains(QLatin1String(n)); });
}
bool one(const QJsonValue &v, const char *names) { return v.isString() && QString::fromLatin1(names).split('|').contains(v.toString()); }
bool version(const QJsonObject &o) { return o["version"] == QJsonValue(1); }
bool text(const QJsonValue &v, int max = 4096, bool controls = true) {
    if (!v.isString() || v.toString().toUtf8().size() > max) return false;
    const auto s=v.toString();
    for (qsizetype i=0;i<s.size();++i) {
        const auto c=s[i];
        if (c.isHighSurrogate()) { if (++i>=s.size() || !s[i].isLowSurrogate()) return false; }
        else if (c.isLowSurrogate() || (!controls && c.category()==QChar::Other_Control)) return false;
    }
    return true;
}
bool count(const QJsonValue &v) { return v.isDouble() && std::isfinite(v.toDouble()) && v.toDouble()>=0 && std::floor(v.toDouble())==v.toDouble() && v.toDouble()<18446744073709551616.0; }
bool strings(const QJsonValue &v) { if(!v.isArray()) return false; for(const auto &s:v.toArray())if(!text(s))return false; return true; }
bool error(const QJsonValue &v) { const auto o=v.toObject();return v.isObject()&&keys(o,{"kind","message"})&&id(o["kind"])&&text(o["message"],256,false); }
bool warnings(const QJsonValue &v) {
    if(!v.isArray())return false;
    for(const auto &x:v.toArray()){const auto o=x.toObject();if(!x.isObject()||!keys(o,{"code","message"})||!text(o["code"])||!text(o["message"],65536))return false;}
    return true;
}
bool input(const QJsonValue &v, const QString &operation={}, const QString &revision={}) {
    if(!v.isObject())return false;const auto o=v.toObject(),p=o["params"].toObject();
    const bool verify=o["method"]==QJsonValue("native.prepare.verify");
    return keys(o,{"method","params"})&&one(o["method"],"native.apply|native.vanilla|native.prepare.verify")&&o["params"].isObject()
        &&(verify?keys(p,{"operationId","expectedRevision","token"}):keys(p,{"operationId","expectedRevision","token","deploymentLayout"})&&p["deploymentLayout"]==QJsonValue("shards-v1"))
        &&id(p["operationId"])&&decimal(p["expectedRevision"])&&hex(p["token"],32)
        &&(operation.isEmpty()||p["operationId"]==QJsonValue(operation))&&(revision.isEmpty()||p["expectedRevision"]==QJsonValue(revision));
}
bool summary(const QJsonValue &v, bool changed, const QJsonObject &b) {
    if(!v.isObject())return false;auto o=v.toObject();
    qint64 repairFiles=0; // a built repair package is one extra physical file
    if(o.contains("repairs")) {const auto r=o.take("repairs");repairFiles=r.toObject().value("file").isObject()?1:0;const auto n=o.take("repairModCount");const auto symptoms=o.take("repairSymptoms");const auto migration=o.take("upstreamRepairMigration");if(!changed||!(r.isNull()||r.isObject())||!count(n)||!strings(symptoms)||!(migration.isNull()||migration==QJsonValue("replaceMountPreserveFolder")))return false;}
    if(o.contains("performance")) {const auto value=o.take("performance");auto p=value.toObject();
        if(!OptionData::validPerformanceChanges(p.take("changes")))return false;
        if(!changed||!value.isObject()||!keys(p,{"presetId","presetName","entryCount","omittedCount","detected","overlayAction","foreignPolicy","restorePolicy"})||!text(p["presetId"],256)||!(p["presetName"].isNull()||text(p["presetName"],512))||!count(p["entryCount"])||!count(p["omittedCount"])||!p["detected"].isObject()||!one(p["overlayAction"],"replace|remove|preserve")||p["foreignPolicy"]!=QJsonValue("preserve-and-refuse")||p["restorePolicy"]!=QJsonValue("exact-prior-bytes"))return false;
    }
    if(!id(o["profileId"])||!root(o["gamePath"])||!warnings(o["warnings"])||!strings(o["replacedMounts"])||!count(o["physicalFileCount"])||!count(o["totalBytes"]))return false;
    if(b.contains("profileId")&&o["profileId"]!=b["profileId"])return false;
    if(!changed) return keys(o,{"profileId","gamePath","mode","physicalFileCount","totalBytes","replacedMounts","installedMounts","retainedMounts","warnings","fontsPreserved"})
        &&one(o["mode"],"modded|vanilla")&&(!b.contains("mode")||o["mode"]==b["mode"])&&o["physicalFileCount"]==QJsonValue(0)&&o["totalBytes"]==QJsonValue(0)
        &&o["replacedMounts"].toArray().isEmpty()&&o["installedMounts"].isArray()&&o["installedMounts"].toArray().isEmpty()&&strings(o["retainedMounts"])&&o["fontsPreserved"]==QJsonValue(true);
    const bool vanilla=o.contains("mode");
    if(vanilla ? !keys(o,{"packageCount","directoryCount","physicalFileCount","companionFileCount","profileId","fileCount","totalBytes","gamePath","replacedMounts","warnings","summaryVersion","deploymentLayout","shardCount","shards","mode","profileName","installedMounts","fontsPreserved","retainedMounts"})
               : !keys(o,{"packageCount","directoryCount","physicalFileCount","companionFileCount","profileId","fileCount","totalBytes","gamePath","replacedMounts","warnings","summaryVersion","deploymentLayout","shardCount","shards"}))return false;
    if(b.contains("mode")&&(b["mode"]==QJsonValue("vanilla"))!=vanilla)return false;
    for(const auto *k:{"packageCount","directoryCount","companionFileCount","fileCount","shardCount"})if(!count(o[k]))return false;
    if(o["summaryVersion"]!=QJsonValue(2)||o["deploymentLayout"]!=QJsonValue("shards-v1")||!o["shards"].isArray()||o["shards"].toArray().size()!=o["shardCount"].toInteger(-1))return false;
    qint64 index=0,total=0;
    for(const auto &v:o["shards"].toArray()){const auto s=v.toObject();if(!keys(s,{"index","directoryCount"})||s["index"].toInteger(-1)!=++index||!count(s["directoryCount"])||s["directoryCount"].toDouble()>99)return false;total+=s["directoryCount"].toInteger();}
    if(index<1||index>99||total!=o["directoryCount"].toInteger(-1)||o["fileCount"]!=o["directoryCount"]||o["physicalFileCount"].toInteger(-1)-repairFiles<0||o["companionFileCount"].toInteger(-1)!=o["physicalFileCount"].toInteger(-1)-repairFiles-o["directoryCount"].toInteger(-1))return false;
    return !vanilla||(o["mode"]==QJsonValue("vanilla")&&text(o["profileName"],65536)&&strings(o["installedMounts"])&&strings(o["retainedMounts"])&&o["fontsPreserved"]==QJsonValue(true));
}
bool ready(const QJsonValue &v,const QJsonObject &b) {
    const auto o=v.toObject();if(!v.isObject()||!keys(o,{"reviewToken","preparationOperationId","preparationInput","changed","summary"})||!hex(o["reviewToken"],32)||!id(o["preparationOperationId"])||!o["changed"].isBool()
        ||!input(o["preparationInput"],o["preparationOperationId"].toString(),b["expectedRevision"].toString())||!summary(o["summary"],o["changed"].toBool(),b))return false;
    const auto i=o["preparationInput"].toObject();
    return o["reviewToken"]==i["params"].toObject()["token"]
        &&(i["method"]!=QJsonValue("native.prepare.verify"))==o["changed"].toBool()
        &&(!o["changed"].toBool()||!b.contains("mode")||i["method"]==QJsonValue(b["mode"]==QJsonValue("vanilla")?"native.vanilla":"native.apply"));
}
bool preparation(const QJsonValue &v,const QJsonObject &r) {
    const auto o=v.toObject(),i=o["input"].toObject();
    if(!v.isObject()||!keys(o,{"operationId","input","resultingRevision","changed","proofSha256","gameinfoSha256"})||!id(o["operationId"])||!input(o["input"],o["operationId"].toString())||!decimal(o["resultingRevision"])||!o["changed"].isBool()||!hex(o["proofSha256"],64)||!hex(o["gameinfoSha256"],64)||r.isEmpty()||o["input"]!=r["preparationInput"]||o["changed"]!=r["changed"])return false;
    const auto e=i["params"].toObject()["expectedRevision"].toString().toULongLong();const bool changed=o["changed"].toBool();
    return (!changed||e<std::numeric_limits<quint64>::max())&&o["resultingRevision"].toString()==QString::number(e+quint64(changed));
}
bool query(const QJsonValue &v,const QJsonObject &p,const QJsonObject &b) {
    if(!v.isObject())return false;const auto o=v.toObject();
    if(!version(o)||!decimal(o["epoch"],true)||!id(o["attemptId"])|| (p.contains("epoch")&&o["epoch"]!=p["epoch"]) || (p.contains("attemptId")&&o["attemptId"]!=p["attemptId"]))return false;
    if(o["state"]==QJsonValue("unknown"))return keys(o,{"version","epoch","attemptId","state"});
    if(!keys(o,{"version","epoch","attemptId","recordRevision","state","ready","preparation","handoff","outcome"})||!decimal(o["recordRevision"])||!one(o["state"],"admitted|ready|preparing|prepared|handoff|terminal"))return false;
    if(!o["ready"].isNull()&&(!ready(o["ready"],b)||(b.contains("originalReady")&&o["ready"]!=b["originalReady"])))return false;
    if(!o["preparation"].isNull()&&!preparation(o["preparation"],o["ready"].toObject()))return false;
    if(!o["handoff"].isNull()){const auto h=o["handoff"].toObject();if(!keys(h,{"journalId","stage"})||!id(h["journalId"])||!one(h["stage"],"reserved|armed|dispatchIntent|settling|settled")||o["preparation"].isNull())return false;}
    const auto state=o["state"].toString();
    const bool hasReady=!o["ready"].isNull(),hasPreparation=!o["preparation"].isNull(),hasHandoff=!o["handoff"].isNull(),hasOutcome=!o["outcome"].isNull();
    const bool settled=o["handoff"].toObject()["stage"]==QJsonValue("settled");
    // Mirror launch_record::attempt for the fields projected by query. The
    // private commitInput is omitted; preparing still requires its Ready.
    const bool validState=state=="admitted" ? !hasReady&&!hasPreparation&&!hasHandoff&&!hasOutcome
        : (state=="ready"||state=="preparing") ? hasReady&&!hasPreparation&&!hasHandoff&&!hasOutcome
        : state=="prepared" ? hasPreparation&&!hasHandoff&&!hasOutcome
        : state=="handoff" ? hasHandoff&&!settled&&!hasOutcome
        : state=="terminal"&&hasOutcome&&(!hasHandoff||settled);
    if(!validState)return false;
    if(!o["outcome"].isNull()){
        const auto x=o["outcome"].toObject();const auto pair=x["status"].toString()+"/"+x["reason"].toString();
        if(!keys(x,{"status","reason"})||!QStringList{"cancelled/beforeDispatch","failed/preparationFailed","failed/preDispatchFailed","failed/spawnFailed","observed/gameCycleCompleted","resolved/steamIntentCancelled","resolved/startTimedOut"}.contains(pair)||state!="terminal")return false;
        if(pair=="failed/preparationFailed"&&o["handoff"].toObject()["stage"]==QJsonValue("settled"))return false;
        if((pair=="failed/preDispatchFailed"||pair=="failed/spawnFailed"||pair=="observed/gameCycleCompleted"||pair=="resolved/steamIntentCancelled"||pair=="resolved/startTimedOut")&&o["handoff"].isNull())return false;
    }else if(state=="terminal")return false;
    return true;
}
bool stop(const QJsonValue &v,const QJsonObject &p,bool status) {
    const auto o=v.toObject();return v.isObject()&&keys(o,{"version","sessionToken","status"})&&version(o)&&hex(o["sessionToken"],32)&&(!p.contains("sessionToken")||o["sessionToken"]==p["sessionToken"])
        &&one(o["status"],status?"stopping|alreadyExited|unavailable|exited|stillRunning":"stopping|alreadyExited|unavailable");
}
bool completed(C c,const QJsonValue &v,const QJsonObject &p,const QJsonObject &b) {
    if(!v.isObject())return false;const auto o=v.toObject();
    switch(c) {
    case C::LaunchOpen:case C::LaunchSession:return keys(o,{"version","epoch","admitted","settingsWrites"})&&version(o)&&(o["epoch"].isNull()||decimal(o["epoch"],true))&&decimal(o["admitted"])&&decimal(o["settingsWrites"])&&o["admitted"].toString().toULongLong()<=64&&o["settingsWrites"].toString().toULongLong()<=64&&(c!=C::LaunchOpen||!o["epoch"].isNull())&&(!o["epoch"].isNull()||(o["admitted"]==QJsonValue("0")&&o["settingsWrites"]==QJsonValue("0")));
    case C::LaunchSettingsGet:return keys(o,{"version","revision","additionalArguments"})&&version(o)&&decimal(o["revision"])&&text(o["additionalArguments"],16384)&&arguments(o["additionalArguments"].toString());
    case C::LaunchSettingsSet:return !settingsReceipt(p["additionalArguments"].toString(),p["expectedSettingsRevision"].toString()).isEmpty()&&o==settingsReceipt(p["additionalArguments"].toString(),p["expectedSettingsRevision"].toString());
    case C::LaunchBegin:case C::LaunchCommit:case C::LaunchQuery:return query(v,p,b);
    case C::LaunchResolve:return query(v,b.contains("resolvedAttempt")?b["resolvedAttempt"].toObject():QJsonObject{},b)&&o["state"]==QJsonValue("terminal");
    case C::LaunchBarriers:{
        if(!keys(o,{"version","barriers"})||!version(o)||!o["barriers"].isArray()||o["barriers"].toArray().size()>1)return false;
        for(const auto &v:o["barriers"].toArray()){auto x=v.toObject();if(x.contains("legacy") || x.contains("markerTime")){const auto legacy=x.take("legacy"), marker=x.take("markerTime");if(!legacy.isBool() || !(marker.isNull() || count(marker)))return false;}if(!keys(x,{"version","rootPath","journalId","epoch","attemptId","recordRevision","stage","pairSha256"})||!version(x)||x["rootPath"]!=p["dataDir"]||!id(x["journalId"])||!decimal(x["epoch"],true)||!id(x["attemptId"])||!decimal(x["recordRevision"])||!one(x["stage"],"reserved|armed|dispatchIntent|settling")||!hex(x["pairSha256"],64))return false;}return true;}
    case C::LaunchStatus:{
        if(!version(o)||o["epoch"]!=p["epoch"]||o["attemptId"]!=p["attemptId"])return false;
        if(o["phase"]==QJsonValue("unknown"))return keys(o,{"version","epoch","attemptId","phase"});
        if(!keys(o,{"version","epoch","attemptId","sequence","phase","attempt","preparationState","handoffState","observationState","progress","route","launcher","error","cancelRequested"})||!decimal(o["sequence"])||!one(o["phase"],"reviewing|ready|preparing|verifyingPrepared|openingSteam|waitingForGame|running|cancelled|failed|timedOut|completed|resolved")||!query(o["attempt"],p,b)||!one(o["preparationState"],"reviewing|reviewed|executing|prepared|notConfirmed")||!one(o["handoffState"],"notDispatched|armed|dispatched|spawnFailed|uncertain|settled")||!one(o["observationState"],"notStarted|waiting|running|unavailable|retired|exited")||!o["cancelRequested"].isBool()||(!o["error"].isNull()&&!error(o["error"])))return false;
        if(!o["progress"].isNull()){const auto x=o["progress"].toObject();if(x["kind"]==QJsonValue("repairing")){if(!keys(x,{"kind","checked","total"})||!x["checked"].isDouble()||!x["total"].isDouble()||x["checked"].toInt()<0||x["checked"].toInt()>x["total"].toInt())return false;}else if(x["kind"]==QJsonValue("copying")){if(!keys(x,{"kind","filesDone","filesTotal","bytesDone","bytesTotal"}))return false;for(const auto *k:{"filesDone","filesTotal","bytesDone","bytesTotal"})if(!decimal(x[k]))return false;if(x["filesDone"].toString().toULongLong()>x["filesTotal"].toString().toULongLong()||x["bytesDone"].toString().toULongLong()>x["bytesTotal"].toString().toULongLong())return false;}else if(!keys(x,{"kind"})||!one(x["kind"],"verifying|committing"))return false;}
        if(!o["route"].isNull()){const auto x=o["route"].toObject();if(!keys(x,{"kind","eligibility"})||!QStringList{"direct/executable","flatpakUser/installedRef","flatpakSystem/installedRef","snap/installedPackage","generic/handlerRequest"}.contains(x["kind"].toString()+"/"+x["eligibility"].toString()))return false;}
        if(!o["launcher"].isNull()){const auto x=o["launcher"].toObject();if(!keys(x,{"state","exitCode"})||!one(x["state"],"running|exited|unavailable|queryRetained")||(!x["exitCode"].isNull()&&(!x["exitCode"].isDouble()||std::floor(x["exitCode"].toDouble())!=x["exitCode"].toDouble()||x["exitCode"].toDouble()<-2147483648.0||x["exitCode"].toDouble()>2147483647.0)))return false;}
        return true;}
    case C::LaunchCancel:return keys(o,{"version","epoch","attemptId","status"})&&version(o)&&o["epoch"]==p["epoch"]&&o["attemptId"]==p["attemptId"]&&one(o["status"],"requested|dispatchInProgress");
    case C::LaunchOwner:return keys(o,{"version","state","busy","active","childRetained"})&&version(o)&&one(o["state"],"open|closing|closed")&&o["busy"].isBool()&&o["active"].isBool()&&o["childRetained"].isBool()
        &&(o["state"]!=QJsonValue("closed")||(!o["busy"].toBool()&&!o["active"].toBool()));
    case C::LaunchClose:return keys(o,{"version","status"})&&version(o)&&o["status"]==QJsonValue("closing");
    case C::GameStop:return stop(v,p,false);
    case C::GameStatus:{
        if(!keys(o,{"version","state","scanComplete","processes","stop"})||!version(o)||!one(o["state"],"idle|running|unavailable")||!o["scanComplete"].isBool()||!o["processes"].isArray()||o["processes"].toArray().size()>16||(!o["stop"].isNull()&&!stop(o["stop"],{},true)))return false;
        QSet<QString> tokens;for(const auto &v:o["processes"].toArray()){const auto x=v.toObject();if(!keys(x,{"sessionToken","canStop","stopAvailability"})||!hex(x["sessionToken"],32)||tokens.contains(x["sessionToken"].toString())||!x["canStop"].isBool()||!one(x["stopAvailability"],"available|observationUnavailable|differentUser|pidfdUnsupported|pidfdUnavailable")||x["canStop"].toBool()!=(x["stopAvailability"]==QJsonValue("available")))return false;tokens.insert(x["sessionToken"].toString());}
        return (o["state"]!=QJsonValue("idle")||(o["scanComplete"]==QJsonValue(true)&&tokens.isEmpty()))&&(o["state"]!=QJsonValue("running")||!tokens.isEmpty());}
    default:return false;
    }
}
}

bool reviewSummary(const QJsonValue &v,bool changed,const QJsonObject &b){return summary(v,changed,b);}
bool call(C c){return int(c)>=int(C::LaunchOpen)&&int(c)<=int(C::GameStop)&&int(c)!=32; /* retired settings-operation slot */}
bool asynchronous(C c){return call(c)&&int(c)<=int(C::LaunchResolve);}
bool admitsOwner(C c){return c==C::LaunchOpen||c==C::LaunchBegin||c==C::LaunchCommit||c==C::LaunchSettingsSet||c==C::LaunchResolve;}
QString method(C c){static const QStringList n{"native.launch.open","native.launch.session","native.launch.settings.get","native.launch.settings.set",QString(),"native.launch.begin","native.launch.commit","native.launch.query","native.launch.barriers","native.launch.resolve","native.launch.control","native.launch.status","native.launch.cancel","native.launch.owner","native.launch.close","native.game.status","native.game.stop"};return call(c)?n[int(c)-int(C::LaunchOpen)]:QString();}
bool decimal(const QJsonValue &v,bool positive){if(!v.isString())return false;bool ok=false;const auto s=v.toString();const auto n=s.toULongLong(&ok);return ok&&QString::number(n)==s&&(!positive||n>0);}
bool id(const QJsonValue &v){if(!v.isString()||v.toString().isEmpty()||v.toString().size()>64)return false;const auto s=v.toString();return std::all_of(s.begin(),s.end(),[](QChar c){return (c.unicode()<128&&c.isLetterOrNumber())||c=='_'||c=='-';});}
bool hex(const QJsonValue &v,int length){if(!v.isString()||v.toString().size()!=length)return false;const auto s=v.toString();return std::all_of(s.begin(),s.end(),[](QChar c){return (c>='0'&&c<='9')||(c>='a'&&c<='f');});}
bool root(const QJsonValue &v){if(!text(v)||!QDir::isAbsolutePath(v.toString()))return false;const auto s=v.toString();return !s.contains(QChar(0))&&!s.split('/').contains("..")&&!s.split('/').contains(".");}
bool arguments(const QString &s){if(!text(s,16384))return false;for(const auto c:s)if(c.category()==QChar::Other_Control&&c!='\t')return false;return true;}
bool strictJson(const QByteArray &bytes) {
    // Qt's DOM folds duplicate members. Detect them before admitting new wire or
    // durable contexts, including differently escaped spellings of the same key.
    qsizetype at=0;
    auto space=[&]{while(at<bytes.size()&&(bytes[at]==' '||bytes[at]=='\t'||bytes[at]=='\r'||bytes[at]=='\n'))++at;};
    auto string=[&](QString *key){
        if(at>=bytes.size()||bytes[at]!='"')return false;const auto start=at++;
        bool ended=false;
        while(at<bytes.size()){const auto ch=bytes[at++];if(ch=='"'){ended=true;break;}if(ch=='\\'){if(at>=bytes.size())return false;++at;}else if(uchar(ch)<32)return false;}
        if(!ended)return false;
        if(key){const auto token=bytes.mid(start,at-start);QJsonParseError e;const auto d=QJsonDocument::fromJson("["+token+"]",&e);if(e.error!=QJsonParseError::NoError||!d.isArray()||!d.array()[0].isString())return false;*key=d.array()[0].toString();}
        return true;
    };
    std::function<bool(int)> value=[&](int depth){
        if(depth>16)return false;space();if(at>=bytes.size())return false;const auto ch=bytes[at];
        if(ch=='"')return string(nullptr);
        if(ch=='{'||ch=='['){++at;space();const char close=ch=='{'?'}':']';if(at<bytes.size()&&bytes[at]==close){++at;return true;}QSet<QString> names;
            for(;;){space();if(ch=='{'){QString key;if(!string(&key)||names.contains(key))return false;names.insert(key);space();if(at>=bytes.size()||bytes[at++]!=':')return false;}if(!value(depth+1))return false;space();if(at>=bytes.size())return false;const auto delimiter=bytes[at++];if(delimiter==close)return true;if(delimiter!=',')return false;}
        }
        const auto start=at;while(at<bytes.size()&&bytes[at]!=','&&bytes[at]!=']'&&bytes[at]!='}'&&bytes[at]!=' '&&bytes[at]!='\n'&&bytes[at]!='\r'&&bytes[at]!='\t')++at;return at>start;
    };
    if(!value(0))return false;space();return at==bytes.size();
}
QByteArray canonical(const QJsonValue &v){
    if(v.isObject()){QByteArray out="{";bool first=true;const auto o=v.toObject();auto names=o.keys();std::sort(names.begin(),names.end(),[](const QString &a,const QString &b){return a.toUtf8()<b.toUtf8();});for(const auto &k:names){if(!first)out+=',';first=false;out+=canonical(k)+':'+canonical(o[k]);}return out+'}';}
    if(v.isArray()){QByteArray out="[";bool first=true;for(const auto &x:v.toArray()){if(!first)out+=',';first=false;out+=canonical(x);}return out+']';}
    if(v.isString()){QByteArray out="\"";for(const unsigned char c:v.toString().toUtf8()){switch(c){case '"':out+="\\\"";break;case '\\':out+="\\\\";break;case '\b':out+="\\b";break;case '\t':out+="\\t";break;case '\n':out+="\\n";break;case '\f':out+="\\f";break;case '\r':out+="\\r";break;default:if(c<32){out+="\\u00";out+=QByteArray::number(c,16).rightJustified(2,'0');}else out+=char(c);}}return out+'"';}
    if(v.isBool())return v.toBool()?"true":"false";
    if(v.isNull())return "null";
    // Request identities use strings, booleans and objects only. Version1 markers
    // additionally use small exact integers; never canonicalize floating counters.
    return QByteArray::number(v.toInteger());
}
QJsonObject settingsReceipt(const QString &raw,const QString &expected) {
    if(!arguments(raw)||!decimal(expected)||expected.toULongLong()==std::numeric_limits<quint64>::max())return {};
    const auto revision=QString::number(expected.toULongLong()+1);
    const QJsonObject settings{{"version",1},{"revision",revision},{"additionalArguments",raw}};
    return {{"version",1},{"revision",revision},{"settingsSha256",QString::fromLatin1(QCryptographicHash::hash(canonical(settings),QCryptographicHash::Sha256).toHex())}};
}
QString inputHash(C c,QJsonObject p){const auto r=p.take("dataDir");p.remove("controlId");return QCryptographicHash::hash(canonical(QJsonObject{{"rootPath",r},{"input",QJsonObject{{"method",method(c)},{"params",p}}}}),QCryptographicHash::Sha256).toHex();}
bool decode(C c,const QJsonValue &v,const QJsonObject &p,const QJsonObject &b,C receiptCall,const QJsonObject &receiptParams,QVariantMap &out){
    if(!v.isObject()||!root(p["dataDir"])||(b.contains("root")&&b["root"]!=p["dataDir"]))return false;const auto o=v.toObject();bool ok=false;
    if(asynchronous(c))ok=keys(o,{"version","accepted","controlId","inputSha256"})&&version(o)&&o["accepted"]==QJsonValue(true)&&o["controlId"]==p["controlId"]&&id(o["controlId"])&&o["inputSha256"]==QJsonValue(inputHash(c,p));
    else if(c==C::LaunchControl){
        ok=keys(o,{"version","controlId","state","inputSha256","value","error"})&&version(o)&&o["controlId"]==p["controlId"]&&id(o["controlId"])&&one(o["state"],"pending|succeeded|failed|unknown");
        if(ok&&o["state"]==QJsonValue("unknown"))ok=o["inputSha256"].isNull()&&o["value"].isNull()&&o["error"].isNull();
        else if(ok){ok=asynchronous(receiptCall)&&o["inputSha256"]==QJsonValue(inputHash(receiptCall,receiptParams));if(o["state"]==QJsonValue("pending"))ok=ok&&o["value"].isNull()&&o["error"].isNull();else if(o["state"]==QJsonValue("failed"))ok=ok&&o["value"].isNull()&&error(o["error"]);else ok=ok&&o["error"].isNull()&&completed(receiptCall,o["value"],receiptParams,b);}
    }else ok=completed(c,v,p,b);
    if(ok)out=o.toVariantMap();return ok;
}
bool barrier(const QJsonValue &v,const QString &r){const auto o=v.toObject();return v.isObject()&&keys(o,{"version","rootPath","journalId","epoch","attemptId","recordRevision","stage","preparedOperationId","preparedRevision","proofSha256","fileRecoveryRequired"})&&version(o)&&o["rootPath"]==QJsonValue(r)&&root(o["rootPath"])&&id(o["journalId"])&&decimal(o["epoch"],true)&&id(o["attemptId"])&&decimal(o["recordRevision"])&&one(o["stage"],"armed|dispatchIntent")&&id(o["preparedOperationId"])&&decimal(o["preparedRevision"])&&hex(o["proofSha256"],64)&&o["fileRecoveryRequired"]==QJsonValue(false);}
bool marker(const QJsonObject &o,const QString &r){const auto i=o["preparationInput"].toObject(),p=i["params"].toObject();return keys(o,{"launchContextVersion","method","root","epoch","attemptId","profileId","mode","expectedRevision","settingsRevision","operationId","reviewToken","preparationInput"})&&o["launchContextVersion"]==QJsonValue(1)&&o["method"]==QJsonValue(int(C::LaunchCommit))&&o["root"]==QJsonValue(r)&&root(o["root"])&&decimal(o["epoch"],true)&&id(o["attemptId"])&&id(o["profileId"])&&one(o["mode"],"modded|vanilla")&&decimal(o["expectedRevision"])&&decimal(o["settingsRevision"])&&id(o["operationId"])&&hex(o["reviewToken"],32)&&input(i,o["operationId"].toString(),o["expectedRevision"].toString())&&(i["method"]==QJsonValue("native.prepare.verify")||i["method"]==QJsonValue(o["mode"]==QJsonValue("vanilla")?"native.vanilla":"native.apply"))&&hex(p["token"],32)&&o["reviewToken"]==p["token"];}
}
