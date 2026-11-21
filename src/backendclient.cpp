#include "backendclient.h"
#include "localimportcontroller.h"
#include "fontdata.h"
#include "optiondata.h"
#include "launchdata.h"
#include "remotedata.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QCryptographicHash>
#include <QDebug>
#include <QDir>
#include <QRunnable>
#include <QProcess>
#include <QThreadPool>
#include <algorithm>
#include <limits>

namespace {
constexpr qsizetype responseLimit = 32 * 1024 * 1024;
// Paired app/helper wire contract. Data document versions are validated separately.
constexpr const char *requiredFeatures[] = {
    "library.snapshot",
    "native.snapshot",
    "native.importPreview",
    "native.importCommit",
    "native.setEnabled",
    "native.setFixEnabled",
    "native.setFixAutomatically",
    "native.bulkLibrary.v1",
    "native.bisect.v1",
    "native.health.v1",
    "native.maintenance.v1",
    "native.storage.v1",
    "native.hero.detect",
    "native.modUpdates.v1",
    "native.installedScan.v1",
    "native.installedGroupImport.v1",
    "native.setModsEnabled",
    "native.removeModsFromProfile",
    "native.deleteMods",
    "native.profile",
    "native.operation",
    "native.applyPreview",
    "native.apply",
    "native.restore",
    "native.recover",
    "native.local.prepare",
    "native.local.status",
    "native.local.review",
    "native.local.cancel",
    "native.local.dismiss",
    "native.local.commit",
    "native.local.archives.v1",
    "native.fonts.v1",
    "native.perfConfig.v1",
    "native.autoexec.v1",
    "native.crosshair.v1",
    "native.options.v1",
    "native.profileEdit.v1",
    "native.profileShare.v1",
    "native.deploymentShards.v1",
    "native.launch.v1",
    "native.catalog.v1",
    "native.libraryBootstrap.v1",
    "native.remotePayload.v1",
    "native.remoteReplace.v1",
};

bool catalogCall(BackendClient::Call call) { return call >= BackendClient::Call::CatalogOpen && call <= BackendClient::Call::CatalogClose; }
bool catalogKeys(const QJsonObject &o, std::initializer_list<const char *> keys) {
    return o.size() == qsizetype(keys.size()) && std::all_of(keys.begin(), keys.end(), [&o](auto k){ return o.contains(QLatin1String(k)); });
}
bool catalogOne(const QJsonValue &v, const char *values) { return v.isString() && QString::fromLatin1(values).split('|').contains(v.toString()); }
bool catalogError(const QJsonValue &v) {
    const auto o=v.toObject();
    return v.isObject() && catalogKeys(o,{"kind","message","retryAfterSeconds"})
        && catalogOne(o["kind"],"catalogCancelled|catalogUnavailable|catalogStorage|catalogFull|catalogIncomplete|catalogSearchUnavailable|catalogPrivate|catalogNotFound|catalogInvalidProviderResponse|catalogResponseTooLarge|catalogRateLimited|catalogRefused|catalogTransport|catalogStalePreferences")
        && o["message"].isString() && o["message"].toString().toUtf8().size()<=256
        && (o["retryAfterSeconds"].isNull() || (o["retryAfterSeconds"].isDouble() && o["retryAfterSeconds"].toInteger(-1)>=0 && o["retryAfterSeconds"].toInteger(-1)<=86400));
}
// Independent framing pass: iterative JSON grammar, not a payload DOM. Its
// one-byte container stack is bounded by the already bounded shared frame.
// Nested duplicate keys/depth are payload policy; only outer keys attribute ID.
class CatalogEnvelope final {
public:
    explicit CatalogEnvelope(const QByteArray &bytes): bytes(bytes) {}
    qint64 id = 0;
    bool tooDeep = false;
    bool resultHasKey(const QString &wanted) {
        if(resultStart<0)return false;
        const auto saved=at;at=resultStart;space();
        if(!take('{')){at=saved;return false;}
        space();bool found=false;
        if(!take('}'))for(;;) {
            space();const auto start=at;
            if(!string() || at-start>256){at=saved;return false;}
            const auto decoded=QJsonDocument::fromJson("["+bytes.mid(start,at-start)+"]").array();
            if(decoded.size()!=1 || !decoded[0].isString()){at=saved;return false;}
            if(decoded[0].toString()==wanted)found=true;
            space();if(!take(':') || !value()){at=saved;return false;}
            space();if(take('}'))break;if(!take(',')){at=saved;return false;}
        }
        const bool complete=at==resultEnd;at=saved;return complete && found;
    }
    bool read() {
        if(bytes.size()>responseLimit)return false;
        space();if(!take('{'))return false;
        QSet<QString> keys;bool rpc=false,integer=false;
        for(;;){
            space();const auto keyStart=at;if(!string()||at-keyStart>64)return false;
            const auto keyArray=QJsonDocument::fromJson("["+bytes.mid(keyStart,at-keyStart)+"]").array();
            if(keyArray.size()!=1||!keyArray[0].isString())return false;
            const auto key=keyArray[0].toString();
            if(!QStringList{"jsonrpc","id","result","error"}.contains(key)||keys.contains(key))return false;
            keys.insert(key);space();if(!take(':'))return false;
            space();const auto start=at;if(!value())return false;
            if(key=="result"){resultStart=start;resultEnd=at;}
            if(key=="id"){
                if(at-start>20)return false;
                const auto raw=bytes.mid(start,at-start);bool ok=false;
                id=raw.toLongLong(&ok);integer=ok&&raw.size()<=20&&raw==QByteArray::number(id);
            }else if(key=="jsonrpc"){
                if(at-start>64)return false;
                const auto scalar=QJsonDocument::fromJson("["+bytes.mid(start,at-start)+"]").array();
                rpc=scalar.size()==1&&scalar[0]==QJsonValue("2.0");
            }
            space();if(take('}'))break;if(!take(','))return false;
        }
        space();return at==bytes.size()&&keys.size()==3&&keys.contains("id")&&rpc&&integer
            &&(keys.contains("result")!=keys.contains("error"));
    }
private:
    const QByteArray &bytes;
    qsizetype at=0;
    qsizetype resultStart=-1,resultEnd=-1;
    void space(){while(at<bytes.size()&&(bytes[at]==' '||bytes[at]=='\t'||bytes[at]=='\r'||bytes[at]=='\n'))++at;}
    bool take(char c){if(at<bytes.size()&&bytes[at]==c){++at;return true;}return false;}
    bool digit()const{return at<bytes.size()&&bytes[at]>='0'&&bytes[at]<='9';}
    bool string(){
        if(!take('"'))return false;
        while(at<bytes.size()){
            const auto c=static_cast<unsigned char>(bytes[at++]);
            if(c=='"')return true;
            if(c<32)return false;
            if(c=='\\'){
                if(at>=bytes.size())return false;const char escaped=bytes[at++];
                if(escaped=='u'){
                    for(int n=0;n<4;++n){if(at>=bytes.size())return false;const char hex=bytes[at++];if(!((hex>='0'&&hex<='9')||(hex>='a'&&hex<='f')||(hex>='A'&&hex<='F')))return false;}
                }else if(!QByteArray("\"\\/bfnrt").contains(escaped))return false;
            }else if(c>=128){
                int continuation=0;unsigned char low=0x80,high=0xbf;
                if(c>=0xc2&&c<=0xdf)continuation=1;
                else if(c>=0xe0&&c<=0xef){continuation=2;if(c==0xe0)low=0xa0;if(c==0xed)high=0x9f;}
                else if(c>=0xf0&&c<=0xf4){continuation=3;if(c==0xf0)low=0x90;if(c==0xf4)high=0x8f;}
                else return false;
                for(int n=0;n<continuation;++n){if(at>=bytes.size())return false;const auto next=static_cast<unsigned char>(bytes[at++]);if(next<(n==0?low:0x80)||next>(n==0?high:0xbf))return false;}
            }
        }
        return false;
    }
    bool number(){
        take('-');if(take('0')){}else{if(!digit()||bytes[at]=='0')return false;while(digit())++at;}
        if(take('.')){if(!digit())return false;while(digit())++at;}
        if(take('e')||take('E')){if(!take('+'))take('-');if(!digit())return false;while(digit())++at;}
        return true;
    }
    bool value(){
        enum State:char {ObjectFirst,ObjectKey,ObjectValue,ObjectComma,ArrayFirst,ArrayValue,ArrayComma};
        QByteArray stack;
        auto atom=[&](){
            space();if(at>=bytes.size())return false;
            if(stack.size()+1>16)tooDeep=true;
            const char c=bytes[at];
            if(c=='{'){++at;stack.append(ObjectFirst);return true;}
            if(c=='['){++at;stack.append(ArrayFirst);return true;}
            if(c=='"')return string();
            for(const auto *literal:{"true","false","null"}){
                const QByteArray text(literal);if(bytes.mid(at,text.size())==text){at+=text.size();return true;}
            }
            return number();
        };
        if(!atom())return false;
        while(!stack.isEmpty()){
            space();const auto state=State(stack.back());
            if(state==ObjectFirst||state==ObjectKey){
                if(state==ObjectFirst&&take('}')){stack.chop(1);continue;}
                if(!string())return false;space();if(!take(':'))return false;stack.back()=ObjectValue;
            }else if(state==ObjectValue){stack.back()=ObjectComma;if(!atom())return false;}
            else if(state==ObjectComma){if(take('}'))stack.chop(1);else if(take(','))stack.back()=ObjectKey;else return false;}
            else if(state==ArrayFirst||state==ArrayValue){
                if(state==ArrayFirst&&take(']')){stack.chop(1);continue;}
                stack.back()=ArrayComma;if(!atom())return false;
            }else{if(take(']'))stack.chop(1);else if(take(','))stack.back()=ArrayValue;else return false;}
        }
        return true;
    }
};
bool catalogIntegers(const QByteArray &bytes) {
    for(qsizetype i=0;i<bytes.size();){
        if(bytes[i]=='"'){++i;while(i<bytes.size()){const char c=bytes[i++];if(c=='\\')++i;else if(c=='"')break;}}
        else if(bytes[i]=='-'||(bytes[i]>='0'&&bytes[i]<='9')){
            const auto start=i++;while(i<bytes.size()&&!QByteArray(",]} \t\r\n").contains(bytes[i]))++i;
            const auto value=bytes.mid(start,i-start);bool ok=false;const auto integer=value.toLongLong(&ok);
            if(!ok||value!=QByteArray::number(integer))return false;
        }else ++i;
    }
    return true;
}
bool catalogOuterError(const QJsonValue &value) {
    const auto e=value.toObject(),data=e["data"].toObject();
    if(!value.isObject()||!catalogKeys(e,{"code","message","data"})||!catalogKeys(data,{"kind"})
        ||!e["code"].isDouble()||!e["message"].isString()||e["message"].toString().toUtf8().size()>256)return false;
    const auto code=e["code"].toInteger();const auto kind=data["kind"].toString();
    if(code==-32700)return kind=="parseError";
    if(code==-32600)return kind=="invalidRequest";
    if(code==-32601)return kind=="methodNotFound";
    if(code==-32602)return kind=="invalidParams";
    return code==-32020&&catalogOne(data["kind"],"catalogBusy|catalogRootBusy|catalogSessionMismatch|catalogClosed|catalogKeyConflict|catalogUnknownRequest|catalogRequestPending|catalogSessionExhausted|catalogUnavailable|responseTooLarge|launchRootBusy");
}
bool bootstrapError(const QJsonValue &value) {
    const auto e=value.toObject(), data=e["data"].toObject();
    if (!value.isObject() || !catalogKeys(e,{"code","message","data"}) || !catalogKeys(data,{"kind"})
        || !e["code"].isDouble() || !e["message"].isString() || e["message"].toString().toUtf8().size()>4096
        || !LaunchData::id(data["kind"])) return false;
    const auto code=e["code"].toInteger(); const auto kind=data["kind"].toString();
    if(code==-32700)return kind=="parseError";
    if(code==-32600)return kind=="invalidRequest";
    if(code==-32601)return kind=="methodNotFound";
    if(code==-32602)return kind=="invalidParams";
    if(code==-32603)return kind=="bootstrapOutcomeUnknown" || kind=="responseTooLarge";
    return code==-32020;
}
bool bootstrapInspect(const QJsonValue &value, const QJsonObject &params, LibraryInitializationInspection &out) {
    const auto o=value.toObject();
    if(!value.isObject() || !catalogKeys(params,{"version","dataDir"}) || params["version"]!=QJsonValue(1)
        || !LaunchData::root(params["dataDir"]) || !catalogKeys(o,{"version","root","state","canCreate","revision","reason","candidates","blockedPaths","pathsTruncated"})
        || o["version"]!=QJsonValue(1) || o["root"]!=params["dataDir"] || !LaunchData::root(o["root"])
        || o["root"].toString().toUtf8().size()>4096 || !o["canCreate"].isBool() || !o["pathsTruncated"].isBool()
        || !o["candidates"].isDouble() || o["candidates"].toInteger(-1)<0 || o["candidates"].toInteger(-1)>256
        || o["candidates"].toDouble()!=double(o["candidates"].toInteger(-1)) || !o["blockedPaths"].isArray()
        || o["blockedPaths"].toArray().size()>8) return false;
    const auto state=o["state"].toString();
    if(state=="absent") {
        if(o["canCreate"]!=QJsonValue(true) || !o["revision"].isNull() || !o["reason"].isNull() || o["candidates"].toInteger()>=8)return false;
    } else if(state=="initialized") {
        if(o["canCreate"]!=QJsonValue(false) || o["reason"]!=QJsonValue("existingLibrary") || !LaunchData::decimal(o["revision"]))return false;
    } else if(state=="blocked") {
        if(o["canCreate"]!=QJsonValue(false) || !o["revision"].isNull()
            || !catalogOne(o["reason"],"unreadableLibrary|unsupportedLibrary|orphanLibrarySidecar|unknownRootEntry|unsafeRoot|bootstrapLimit|bootstrapStorage"))return false;
    } else return false;
    QStringList paths;
    for(const auto &value:o["blockedPaths"].toArray()) {
        if(!value.isString())return false;
        const auto path=value.toString(); const auto bytes=path.toUtf8();
        if(path.isEmpty() || bytes.size()>512 || QString::fromUtf8(bytes)!=path || path.contains(QChar(0))
            || QDir::isAbsolutePath(path) || path.split('/').contains("..") || path.split('/').contains("."))return false;
        // Filesystem names can contain control or direction characters. Display
        // them as literal escapes, never as layout controls or rich text.
        QString display;
        for(const auto c:path) {
            if(c.category()==QChar::Other_Control || c.category()==QChar::Other_Format)
                display+=QStringLiteral("\\u%1").arg(uint(c.unicode()),4,16,QLatin1Char('0'));
            else display+=c;
        }
        paths.append(display);
    }
    out={o["root"].toString(),state,o["revision"].toString(),o["reason"].toString(),paths,int(o["candidates"].toInteger()),o["canCreate"].toBool(),o["pathsTruncated"].toBool()};
    return true;
}
bool bootstrapReceipt(const LibrarySnapshot &snapshot,const QJsonObject &object) {
    const auto profiles=object["profiles"].toArray();
    if(snapshot.sourceKind!="native" || snapshot.revision!="1" || !snapshot.sourcePath.isEmpty() || !snapshot.gamePath.isEmpty()
        || snapshot.activeProfileId!="default" || snapshot.selectedProfileId!="default" || snapshot.canRestore || snapshot.recoveryRequired
        || !snapshot.mods.isEmpty() || profiles.size()!=1 || !profiles[0].isObject())return false;
    const auto profile=profiles[0].toObject();
    return profile["id"]==QJsonValue("default") && profile["name"]==QJsonValue("Default")
        && profile["isActive"]==QJsonValue(true) && profile["protected"]==QJsonValue(true) && profile["modCount"]==QJsonValue(0);
}
bool catalogDecode(BackendClient::Call call,const QJsonValue &value,const QJsonObject &params,const QJsonObject &binding,qsizetype bytes) {
    using C=BackendClient::Call;
    const auto o=value.toObject();
    if(!value.isObject()||o["version"]!=QJsonValue(1)||!LaunchData::root(o["root"])||o["root"].toString().toUtf8().size()>4096||!LaunchData::hex(o["catalogSession"],32))return false;
    if(call!=C::CatalogOpen && o["catalogSession"]!=params["catalogSession"])return false;
    if(binding.contains("root")&&binding["catalogSession"]==o["catalogSession"]&&o["root"]!=binding["root"])return false;
    if(call==C::CatalogOpen||(call==C::CatalogStatus&&params["requestKey"].isNull())) {
        return bytes<=16384&&catalogKeys(o,{"version","root","catalogSession","phase","error"})
            &&catalogOne(o["phase"],"opening|ready|failed|closing|closed")&&(o["phase"]==QJsonValue("failed")?catalogError(o["error"]):o["error"].isNull());
    }
    if(call==C::CatalogClose)return bytes<=16384&&catalogKeys(o,{"version","root","catalogSession","phase"})&&catalogOne(o["phase"],"closing|closed");
    if(o["requestKey"]!=params["requestKey"]||!LaunchData::hex(o["requestKey"],32))return false;
    if(call==C::CatalogCancel||call==C::CatalogRelease)return bytes<=16384&&catalogKeys(o,{"version","root","catalogSession","requestKey","state"})
        &&catalogOne(o["state"],call==C::CatalogRelease?"released":"requested|alreadyTerminal|released");
    if(!LaunchData::decimal(o["viewGeneration"])||!LaunchData::hex(o["inputDigest"],64))return false;
    if(call==C::CatalogBegin)return bytes<=16384&&catalogKeys(o,{"version","root","catalogSession","requestKey","viewGeneration","inputDigest","phase"})
        &&o["viewGeneration"]==params["viewGeneration"]&&catalogOne(o["phase"],"queued|running|succeeded|failed|cancelled");
    if(!catalogKeys(o,{"version","root","catalogSession","requestKey","viewGeneration","inputDigest","operation","phase","progress","result","error"})||!catalogOne(o["phase"],"queued|running|succeeded|failed|cancelled|released"))return false;
    const auto operation=o["operation"].toObject();
    if(!catalogKeys(operation,{"kind","args"})||!catalogOne(operation["kind"],"query|facets|authors|detail|files|comments|changelog|collections|collection|sync|favoritesGet|favoriteSet"))return false;
    if(o["phase"]==QJsonValue("released"))return bytes<=16384&&operation["args"].isNull()&&o["progress"].isNull()&&o["result"].isNull()&&o["error"].isNull();
    if(!operation["args"].isObject())return false;
    if(!o["progress"].isNull()){
        const auto progress=o["progress"].toObject();
        if(!catalogKeys(progress,{"stage","completed","total"})||!LaunchData::id(progress["stage"])||progress["stage"].toString().size()>32||!LaunchData::decimal(progress["completed"])||(!progress["total"].isNull()&&!LaunchData::decimal(progress["total"])))return false;
    }
    if(catalogOne(o["phase"],"queued|running"))return bytes<=16384&&o["result"].isNull()&&o["error"].isNull();
    if(bytes>1048576)return false;
    if(o["phase"]==QJsonValue("succeeded"))return o["result"].isObject()&&catalogKeys(o["result"].toObject(),{"identity","catalogRevision","provenance","data"})&&o["error"].isNull();
    return o["result"].isNull()&&catalogError(o["error"])&&(o["phase"]!=QJsonValue("cancelled")||o["error"].toObject()["kind"]==QJsonValue("catalogCancelled"));
}

}

// Independent of the client lifetime: EOF lets the helper join its writer and
// reparent a persistent launcher. Drain completion, then normal process exit,
// is the only replacement boundary; there is no kill timer.
class RetirementDrain final : public QObject {
    Q_OBJECT
public:
    explicit RetirementDrain(QProcess *process):m_process(process) {
        process->setParent(this);
        connect(process,&QProcess::readyReadStandardOutput,this,&RetirementDrain::drain);
        connect(process,&QProcess::readyReadStandardError,this,&RetirementDrain::drain);
        connect(process,&QProcess::finished,this,&RetirementDrain::drain);
        process->closeWriteChannel();
        QTimer::singleShot(0,this,&RetirementDrain::drain);
    }
signals:
    void drained();
private:
    void drain() {
        bool more=false;
        for(const auto channel:{QProcess::StandardOutput,QProcess::StandardError}) {
            m_process->setReadChannel(channel);m_process->read(64*1024);more=more||m_process->bytesAvailable()>0;
        }
        m_process->setReadChannel(QProcess::StandardOutput);
        if(more){if(!m_queued){m_queued=true;QTimer::singleShot(0,this,[this]{m_queued=false;drain();});}return;}
        if(m_process->state()==QProcess::NotRunning&&!m_done){m_done=true;emit drained();deleteLater();}
    }
    QProcess *m_process;
    bool m_queued=false,m_done=false;
};

class ResponseDecoder final : public QObject, public QRunnable {
    Q_OBJECT
public:
    explicit ResponseDecoder(QByteArray line, QHash<qint64,BackendClient::Request> requests)
        : m_line(std::move(line)), m_requests(std::move(requests)) {}
    void run() override {
        const bool catalogPending=std::any_of(m_requests.cbegin(),m_requests.cend(),[](const auto &r){return catalogCall(r.call);});
        const bool remotePending=std::any_of(m_requests.cbegin(),m_requests.cend(),[](const auto &r){return RemoteData::call(r.call);});
        const bool recoveryInspectionPending=std::any_of(m_requests.cbegin(),m_requests.cend(),[](const auto &r){return r.call==BackendClient::Call::RecoveryInspect;});
        CatalogEnvelope envelope(m_line);
        bool attributableCatalog=false;
        if(catalogPending || remotePending || recoveryInspectionPending){
            if(!envelope.read()){
                const auto parsed=QJsonDocument::fromJson(m_line).object();
                const auto candidate=parsed["id"].toInteger(-1);
                if(m_requests.contains(candidate)&&m_requests.value(candidate).call==BackendClient::Call::LibraryInspect){
                    emit inspectionDecoded(candidate,{},"invalidEvidence",tr("Library inspection returned invalid evidence."));return;
                }
                emit decoded(false,{},LibrarySnapshot{},false,false,{},false);return;
            }
            attributableCatalog=m_requests.contains(envelope.id)&&catalogCall(m_requests.value(envelope.id).call);
            if(m_requests.contains(envelope.id) && m_requests.value(envelope.id).call==BackendClient::Call::RecoveryInspect
                && (envelope.tooDeep || m_line.size()+1>65536)) {
                emit recoveryDecoded(envelope.id,{},"invalidEvidence",tr("Recovery inspection exceeds its decoding bounds."));return;
            }
            if(attributableCatalog&&(envelope.tooDeep||m_line.size()+1>1048576)){emit catalogPayloadRejected(envelope.id);return;}
            if(m_requests.contains(envelope.id) && RemoteData::call(m_requests.value(envelope.id).call)) {
                const auto call=m_requests.value(envelope.id).call;
                const bool replay=(call==BackendClient::Call::RemoteCommit || call==BackendClient::Call::RemoteReplaceCommit) && envelope.resultHasKey(QStringLiteral("protocolVersion"));
                const qsizetype limit=replay ? responseLimit : (call==BackendClient::Call::RemoteStatus || call==BackendClient::Call::RemoteReplaceStatus) ? 128*1024 : 64*1024;
                if(envelope.tooDeep || m_line.size()+1>limit) {
                    emit remoteDecoded(envelope.id,{}, {}, false,"invalidEvidence",tr("Download reply exceeds its decoding bounds."));return;
                }
            }
        }
        QJsonParseError error;
        const auto document = QJsonDocument::fromJson(m_line, &error);
        if(attributableCatalog&&error.error!=QJsonParseError::NoError){emit catalogPayloadRejected(envelope.id);return;}
        bool valid = error.error == QJsonParseError::NoError && document.isObject();
        const auto object = document.object();
        const auto request=m_requests.value(object["id"].toInteger(-1));
        if(request.call==BackendClient::Call::RecoveryInspect) {
            const auto id=object["id"].toInteger(-1);
            CatalogEnvelope strict(m_line);QVariantMap result;
            const bool shape=valid && m_line.size()+1<=65536 && strict.read() && !strict.tooDeep && LaunchData::strictJson(m_line)
                && catalogIntegers(m_line) && object.size()==3 && object["jsonrpc"]==QJsonValue("2.0") && id>0
                && request.binding["root"]==request.params["dataDir"] && (object.contains("result")!=object.contains("error"));
            if(shape && object.contains("result") && RemoteData::recoveryInspection(object["result"],request.params,result))
                emit recoveryDecoded(id,result,{},{});
            else if(shape && object.contains("error") && bootstrapError(object["error"])) {
                const auto e=object["error"].toObject();
                emit recoveryDecoded(id,{},e["data"].toObject()["kind"].toString(),e["message"].toString());
            } else emit recoveryDecoded(id,{},"invalidEvidence",tr("Recovery inspection returned invalid or mismatched evidence."));
            return;
        }
        if (RemoteData::call(request.call)) {
            const auto id=object["id"].toInteger(-1);
            CatalogEnvelope strict(m_line);
            const bool snapshot=(request.call==BackendClient::Call::RemoteCommit || request.call==BackendClient::Call::RemoteReplaceCommit) && object["result"].toObject().contains("protocolVersion");
            const qsizetype limit=snapshot ? responseLimit : (request.call==BackendClient::Call::RemoteStatus || request.call==BackendClient::Call::RemoteReplaceStatus) ? 128*1024 : 64*1024;
            const bool shape=valid && m_line.size()+1<=limit && strict.read() && !strict.tooDeep && LaunchData::strictJson(m_line)
                && catalogIntegers(m_line) && object["jsonrpc"]==QJsonValue("2.0") && id>0
                && (object.contains("result")!=object.contains("error"));
            if(!shape){emit remoteDecoded(id,{}, {}, false,"invalidEvidence",tr("Download reply is invalid or mismatched; its outcome remains unconfirmed."));return;}
            if(object.contains("error")) {
                const auto e=object["error"].toObject(), data=e["data"].toObject();
                if(m_line.size()+1>64*1024 || !catalogKeys(e,{"code","message","data"}) || !catalogKeys(data,{"kind"})
                    || !e["code"].isDouble() || !e["message"].isString() || e["message"].toString().toUtf8().size()>512 || !LaunchData::id(data["kind"])) {
                    emit remoteDecoded(id,{}, {}, false,"invalidEvidence",tr("Download returned unrecognized error evidence."));return;
                }
                const auto code=e["code"].toInteger();const auto errorKind=data["kind"].toString();
                const bool codeMatches=(code==-32700 && errorKind=="parseError") || (code==-32600 && errorKind=="invalidRequest")
                    || (code==-32601 && errorKind=="methodNotFound") || (code==-32602 && errorKind=="invalidParams")
                    || (code==-32603 && errorKind=="responseTooLarge") || code==-32020;
                if(!codeMatches) {emit remoteDecoded(id,{}, {}, false,"invalidEvidence",tr("Download returned unrecognized error evidence."));return;}
                const auto message=e["message"].toString();
                const bool safe=!message.contains('/') && !message.contains('\\') && std::none_of(message.cbegin(),message.cend(),[](QChar c){return c.category()==QChar::Other_Control;});
                emit remoteDecoded(id,{}, {}, false,errorKind,safe ? message : tr("The helper could not confirm this download action."));return;
            }
            if(snapshot) {
                LibrarySnapshot decoded;
                const auto expected=request.params["expectedRevision"].toString().toULongLong();
                const bool match=LibrarySnapshot::decode(object["result"].toObject(),decoded) && decoded.sourceKind=="native"
                    && object["result"].toObject()["storeVersion"]==QJsonValue(4)
                    && decoded.selectedProfileId==request.params["profileId"].toString() && LaunchData::decimal(request.params["expectedRevision"])
                    && expected<std::numeric_limits<quint64>::max() && decoded.revision==QString::number(expected+1);
                emit remoteDecoded(id,{},decoded,match,match ? QString() : QStringLiteral("invalidEvidence"),match ? QString() : tr("Download replay returned a mismatched library snapshot."));return;
            }
            RemoteReply reply;
            if(!RemoteData::decode(request.call,object["result"],request.params,reply)) {
                emit remoteDecoded(id,{}, {}, false,"invalidEvidence",tr("Download returned invalid or mismatched evidence."));return;
            }
            emit remoteDecoded(id,reply.value,{},false,{},{});return;
        }
        if(request.call==BackendClient::Call::LibraryInspect) {
            LibraryInitializationInspection inspection;
            CatalogEnvelope strict(m_line);
            const bool shape=valid && m_line.size()+1<=65536 && strict.read() && !strict.tooDeep
                && LaunchData::strictJson(m_line) && catalogIntegers(m_line) && object["jsonrpc"]==QJsonValue("2.0")
                && object["id"].toInteger(-1)>0 && request.binding["root"]==request.params["dataDir"]
                && (object.contains("result")!=object.contains("error"));
            if(shape && object.contains("result") && bootstrapInspect(object["result"],request.params,inspection))
                emit inspectionDecoded(object["id"].toInteger(),inspection,{},{});
            else if(shape && object.contains("error") && bootstrapError(object["error"])) {
                const auto e=object["error"].toObject();
                emit inspectionDecoded(object["id"].toInteger(),{},e["data"].toObject()["kind"].toString(),e["message"].toString());
            } else emit inspectionDecoded(object["id"].toInteger(-1),{},"invalidEvidence",tr("Library inspection returned invalid or mismatched evidence."));
            return;
        }
        if(request.call==BackendClient::Call::LibraryInitialize) {
            CatalogEnvelope strict(m_line);
            valid=valid && m_line.size()+1<=65536 && strict.read() && !strict.tooDeep && LaunchData::strictJson(m_line)
                && catalogIntegers(m_line) && object["jsonrpc"]==QJsonValue("2.0")
                && catalogKeys(request.params,{"version","dataDir","operationId","expectedRevision"})
                && request.params["version"]==QJsonValue(1) && LaunchData::root(request.params["dataDir"])
                && request.binding["root"]==request.params["dataDir"]
                && LaunchData::hex(request.params["operationId"],32) && request.params["expectedRevision"]==QJsonValue("0")
                && (!object.contains("error") || bootstrapError(object["error"]));
        }
        if(request.call==BackendClient::Call::Recover) {
            CatalogEnvelope strict(m_line);
            valid=valid && strict.read() && object["jsonrpc"]==QJsonValue("2.0")
                && (object.contains("result")!=object.contains("error"))
                && (!object.contains("error") || (m_line.size()+1<=65536 && !strict.tooDeep && LaunchData::strictJson(m_line) && catalogIntegers(m_line)));
        }
        if(request.call==BackendClient::Call::Operation && (request.binding["remoteOperation"]==QJsonValue(true) || request.binding["recoveryOperation"]==QJsonValue(true))) {
            CatalogEnvelope strict(m_line);const auto result=object["result"].toObject();const auto status=result["status"].toString();
            const bool recoveryRead=request.binding["recoveryOperation"]==QJsonValue(true);
            const bool compactEvidence=object.contains("error") || status!="succeeded";
            bool exact=valid && strict.read()
                && ((!recoveryRead || compactEvidence) ? !strict.tooDeep && LaunchData::strictJson(m_line) && catalogIntegers(m_line) : true)
                && (!compactEvidence || m_line.size()+1<=65536)
                && object["jsonrpc"]==QJsonValue("2.0") && (object.contains("result")!=object.contains("error"));
            if(object.contains("result")) {
                exact=exact && result["operationId"]==QJsonValue(request.operationId)
                    && (status=="unknown" || status=="recoveryRequired" ? catalogKeys(result,{"operationId","status"})
                        : status=="succeeded" ? catalogKeys(result,{"operationId","status","result"}) && result["result"].isObject()
                            && (request.binding["recoveryOperation"]==QJsonValue(true) || result["result"].toObject()["storeVersion"]==QJsonValue(4))
                        : status=="failed" && catalogKeys(result,{"operationId","status","error"}) && result["error"].isObject()
                            && catalogKeys(result["error"].toObject(),{"kind","message"}) && LaunchData::id(result["error"].toObject()["kind"])
                            && result["error"].toObject()["message"].isString() && result["error"].toObject()["message"].toString().toUtf8().size()<=512);
            } else exact=exact && m_line.size()+1<=65536 && bootstrapError(object["error"]);
            valid=exact;
        }
        if(LaunchData::call(request.call)) valid=valid&&m_line.size()+1<=65536&&LaunchData::strictJson(m_line)&&object.size()==3&&object.contains("jsonrpc")&&object.contains("id")&&(object.contains("result")!=object.contains("error"));
        if (catalogCall(request.call)) {
            const auto id = object["id"].toInteger(-1);
            if (!valid || object["jsonrpc"] != QJsonValue("2.0") || !object["id"].isDouble() || id <= 0
                || !LaunchData::strictJson(m_line) || !catalogIntegers(m_line)
                || (object.contains("result") == object.contains("error"))) { emit catalogPayloadRejected(id); return; }
            CatalogDecodedResult decoded;
            if (object.contains("error")) {
                if (m_line.size()+1 <= 16384 && catalogOuterError(object["error"])) {
                    const auto e=object["error"].toObject();
                    decoded.valid=true; decoded.phase="rejected";
                    decoded.errorKind=e["data"].toObject()["kind"].toString(); decoded.error=e["message"].toString();
                }
            } else if (catalogDecode(request.call,object["result"],request.params,request.binding,m_line.size()+1)) {
                decoded=CatalogData::decode(object["result"].toObject(),request.catalogIdentity);
            }
            emit catalogDecoded(id,decoded); return;
        }
        LibrarySnapshot snapshot;
        const auto bulkSummary = [](const QJsonObject &snapshot) {
            const auto value=snapshot.value("bulkResult");
            if (!value.isObject()) return false;
            const auto summary=value.toObject(); const auto changed=summary.value("changed");
            if(summary.size()!=2 || !changed.isDouble() || changed.toDouble()!=changed.toInt(-1) || changed.toInt(-1)<0 || changed.toInt()>256 || !summary.value("skipped").isArray())return false;
            const auto skipped=summary.value("skipped").toArray();
            if(skipped.size()+changed.toInt()>256)return false;
            QSet<QString> ids;
            for(const auto &entry:skipped) {
                const auto item=entry.toObject();const auto id=item.value("id").toString();
                if(item.size()!=2 || !LaunchData::id(QJsonValue(id)) || ids.contains(id)
                    || !QStringList{"unknownMod","invalidLocalSelection","protectedProfile"}.contains(item.value("reason").toString()))return false;
                ids.insert(id);
            }
            return true;
        };
        const bool bulkCall=request.call==BackendClient::Call::SetModsEnabled || request.call==BackendClient::Call::RemoveModsFromProfile || request.call==BackendClient::Call::DeleteMods;
        const bool decodedSnapshot = valid && (!bulkCall || bulkSummary(object.value("result").toObject())) && LibrarySnapshot::decode(object.value("result").toObject(), snapshot);
        const bool validSnapshot = decodedSnapshot && (request.call!=BackendClient::Call::LibraryInitialize || bootstrapReceipt(snapshot,object.value("result").toObject()))
            && (!request.bootstrapSnapshot || (catalogKeys(request.params,{"dataDir"}) && request.params["dataDir"].isString() && LaunchData::root(request.params["dataDir"])
                && request.binding["root"]==request.params["dataDir"] && snapshot.sourceKind=="native"
                && (snapshot.activeProfileId.isEmpty() || snapshot.selectedProfileId==snapshot.activeProfileId)
                && LaunchData::decimal(QJsonValue(snapshot.revision),true)));
        LibrarySnapshot operationSnapshot;
        const auto result = object.value("result").toObject();
        const bool validOperationSnapshot = valid && result.value("status") == QJsonValue("succeeded")
            && LibrarySnapshot::decode(result.value("result").toObject(), operationSnapshot)
            && operationSnapshot.sourceKind == "native"
            && (!result.value("result").toObject().contains("bulkResult") || bulkSummary(result.value("result").toObject()));
        QVariantMap localResult;
        const bool validLocal = valid && (LaunchData::call(request.call) ? m_line.size()+1<=65536 && LaunchData::decode(request.call, object.value("result"),request.params,request.binding,request.receiptCall,request.receiptParams,localResult) : (OptionData::decodeApplyReview(object.value("result"),localResult) || OptionData::decodeReply(object.value("result"),localResult) || (result.contains("summary") ? FontData::review(object.value("result"),localResult) : LocalImportController::decodeReply(object.value("result"), localResult, true))));
        if (validOperationSnapshot) {
            localResult={{"confirmedFontMods",operationSnapshot.fontOfferMods},{"confirmedFontGame",operationSnapshot.gamePath},{"confirmedRevision",operationSnapshot.revision},{"confirmedProfile",operationSnapshot.selectedProfileId}};
            if(request.binding["recoveryOperation"]==QJsonValue(true))
                localResult.insert("confirmedSnapshotDigest",QString::fromLatin1(QCryptographicHash::hash(QJsonDocument(result["result"].toObject()).toJson(QJsonDocument::Compact),QCryptographicHash::Sha256).toHex()));
        }
        if(request.call==BackendClient::Call::Recover && decodedSnapshot)
            localResult.insert("recoverySnapshotDigest",QString::fromLatin1(QCryptographicHash::hash(QJsonDocument(object["result"].toObject()).toJson(QJsonDocument::Compact),QCryptographicHash::Sha256).toHex()));
        if(bulkCall && validSnapshot) localResult.insert("bulkResult",object.value("result").toObject().value("bulkResult").toVariant());
        if(validOperationSnapshot && result.value("result").toObject().contains("bulkResult")) localResult.insert("bulkResult",result.value("result").toObject().value("bulkResult").toVariant());
        emit decoded(valid, object, snapshot, validSnapshot, validOperationSnapshot, localResult, validLocal);
    }
signals:
    void recoveryDecoded(qint64 id, const QVariantMap &value, const QString &kind, const QString &message);
    void remoteDecoded(qint64 id, const QVariantMap &value, const LibrarySnapshot &snapshot, bool snapshotValid, const QString &kind, const QString &message);
    void inspectionDecoded(qint64 id, const LibraryInitializationInspection &inspection, const QString &kind, const QString &message);
    void catalogPayloadRejected(qint64 id);
    void catalogDecoded(qint64 id, const CatalogDecodedResult &result);
    void decoded(bool valid, const QJsonObject &object, const LibrarySnapshot &snapshot, bool validSnapshot, bool validOperationSnapshot, const QVariantMap &localResult, bool validLocal);
private:
    QByteArray m_line;
    QHash<qint64,BackendClient::Request> m_requests;
};

BackendClient::BackendClient(QObject *parent) : QObject(parent) {
    m_timeout.setSingleShot(true);
    m_timeout.setInterval(5000);
    connect(&m_timeout, &QTimer::timeout, this, [this] { fail(tr("Helper timed out after 5 seconds. Refresh to retry.")); });
    m_catalogTimer.setInterval(1000);
    connect(&m_catalogTimer,&QTimer::timeout,this,[this]{
        const bool delayed=m_writing||std::any_of(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return r.bulk;});
        QList<std::pair<Call,QString>> delayedSignals, timeoutSignals;
        for(auto it=m_requests.begin();it!=m_requests.end();++it)if(catalogCall(it->call)){
            if(delayed){if(!it->catalogDelayed){it->catalogDelayed=true;delayedSignals.append({it->call,it->params["requestKey"].toString()});}continue;}
            it->catalogDelayed=false;
            if(!it->catalogNotified&&++it->catalogTicks>=15){it->catalogNotified=true;timeoutSignals.append({it->call,it->params["requestKey"].toString()});}
        }
        for(const auto &s:delayedSignals)emit catalogDelayed(s.first,s.second);
        for(const auto &s:timeoutSignals)emit catalogRejected(s.first,s.second,"catalogUnavailable",tr("Catalog reply is delayed; its request remains retained. Library and launch work continue."));
    });
    m_catalogTimer.start();
    m_launchRetirementTimer.setInterval(1000);
    connect(&m_launchRetirementTimer,&QTimer::timeout,this,[this]{
        if(!m_launchRetiring || !m_process || !m_ready)return;
        const auto pending=[this](Call call){return std::any_of(m_requests.cbegin(),m_requests.cend(),[call](const Request &r){return r.call==call;});};
        if(!m_launchCloseSent) {if(!pending(Call::LaunchClose))nativeRequest(Call::LaunchClose);}
        else if(!pending(Call::LaunchOwner))nativeRequest(Call::LaunchOwner);
    });
}
QPointer<BackendClient> BackendClient::s_bulkOwner;
QList<QPointer<BackendClient>> BackendClient::s_waiters;

bool BackendClient::acquireBulk() {
    if (!s_bulkOwner) { s_bulkOwner = this; return true; }
    if (!s_waiters.contains(this)) s_waiters.append(this);
    return false;
}
void BackendClient::releaseBulk() {
    if (s_bulkOwner != this) return;
    s_bulkOwner.clear();
    const auto waiters = std::exchange(s_waiters, {});
    for (const auto &client : waiters) if (client) QTimer::singleShot(0, client, &BackendClient::flushDeferred);
}
void BackendClient::flushDeferred() {
    if (!m_ready || m_writing || m_retiring) return;
    if (m_deferredSnapshot) { m_deferredSnapshot = false; sendSnapshot(); }
    else if (m_deferredBulk) {
        auto request = std::exchange(m_deferredBulk, {});
        nativeRequest(request->first, request->second);
    }
}
void BackendClient::retireProcess() {
    if (!m_process) return;
    auto *process = std::exchange(m_process, nullptr);
    m_retiring = process; emit lifecycleChanged();
    process->disconnect(this);
    auto *drain=new RetirementDrain(process);
    connect(drain,&RetirementDrain::drained,this,[this] {
        m_retiring.clear();m_launchOwned=false;m_launchWriting=false;m_launchRetiring=false;m_launchCloseSent=false;m_launchRetirementTimer.stop();m_catalogOwned=false;m_catalogRoot.clear();m_catalogSession.clear();m_remoteOwned=false;m_remoteRecoveryOwned=false;m_remoteRoot.clear();m_remoteSession.clear();
        emit lifecycleChanged();emit retired();
    });
}
BackendClient::~BackendClient() {
    if (m_localSession || m_writing || m_launchOwned || m_catalogOwned || remoteOwned()) retireProcess();
    m_writing = false; stop(); emit lifecycleChanged();
}
void BackendClient::stop() {
    if (m_writing && !m_launchOwned) return;
    if (m_launchOwned || m_catalogOwned || remoteOwned()) retireProcess();
    ++m_session; ++m_generation; ++m_inspectGeneration;
    m_timeout.stop(); m_ready = false; m_parsing = false;
    m_stdout.clear(); m_stdoutScanned = 0; m_stderr.clear(); m_helloId = 0; m_requests.clear();
    m_deferredBulk.reset(); m_deferredSnapshot = false; m_bootstrapSnapshotRequested=false; m_launchControls.clear(); releaseBulk();
    if (m_localSession) { retireProcess(); return; }
    if (m_process) {
        auto *process = std::exchange(m_process, nullptr);
        process->disconnect(this); process->setParent(nullptr);
        connect(process, &QProcess::finished, process, &QObject::deleteLater);
        if (process->state() == QProcess::NotRunning) process->deleteLater();
        else process->kill();
    }
}
void BackendClient::start(const QString &helperPath, const QString &statePath, const QString &dataDir, bool nativeFirst, bool localSession) {
    if (m_writing || m_retiring) return;
    if (m_launchOwned || m_catalogOwned || remoteOwned()) { stop(); return; }
    stop(); if (m_retiring) return; m_stopReason.clear(); m_localSession = localSession; m_statePath = statePath; m_profileId.clear(); m_dataDir = dataDir;
    m_nativeView = nativeFirst; m_probeNative = false;
    auto *process = new QProcess(this); m_process = process;
    process->setProcessChannelMode(QProcess::SeparateChannels);
    connect(process, &QProcess::started, this, &BackendClient::sendHello);
    connect(process, &QProcess::readyReadStandardOutput, this, &BackendClient::receive);
    connect(process, &QProcess::readyReadStandardError, this, &BackendClient::drainStderr);
    connect(process, &QProcess::errorOccurred, this, [this](QProcess::ProcessError error) {
        if (error == QProcess::FailedToStart) fail(tr("Cannot start modlock-helper. Check the helper path and executable permissions."));
        else if (error != QProcess::Crashed) fail(tr("Helper process communication failed. Refresh to retry."));
    });
    connect(process, &QProcess::finished, this, [this] {
        fail(tr("Helper exited unexpectedly. Refresh to restart it."));
    });
    m_timeout.start();
    process->start(helperPath, {});
}
void BackendClient::requestSnapshot(const QString &statePath, const QString &profileId, bool rediscoverNative) {
    if (m_writing) return;
    m_bootstrapSnapshotRequested=false;
    ++m_generation; m_statePath = statePath; m_profileId = profileId;
    m_probeNative = rediscoverNative;
    if (m_probeNative) m_nativeView = true;
    m_timeout.start();
    if (m_ready) sendSnapshot();
}
void BackendClient::requestBootstrapSnapshot() {
    if(m_writing)return;
    ++m_generation; m_profileId.clear(); m_probeNative=false; m_bootstrapSnapshotRequested=true;
    if(m_ready)sendSnapshot();
}
bool BackendClient::snapshotPending() const {
    if(m_process&&!m_ready)return true;
    if(m_deferredSnapshot)return true;
    return std::any_of(m_requests.cbegin(),m_requests.cend(),[this](const Request &request){return request.call==Call::Snapshot&&request.generation==m_generation;});
}
void BackendClient::send(qint64 id, const QString &method, const QJsonObject &params) {
    const QJsonObject request{{"jsonrpc", "2.0"}, {"id", id}, {"method", method}, {"params", params}};
    if (!m_process || m_process->write(QJsonDocument(request).toJson(QJsonDocument::Compact) + '\n') < 0)
        fail(tr("Cannot send a request to the helper."));
}
void BackendClient::sendHello() { m_helloId = ++m_nextId; send(m_helloId, "hello"); }
void BackendClient::sendSnapshot() {
    if (m_bootstrapSnapshotRequested) { nativeRequest(Call::Snapshot, {}); return; }
    if (m_nativeView) {
        QJsonObject params;
        // A fallback source can have a different profile namespace from the
        // library another instance imported. Keep its ID only for source fallback.
        if (!m_probeNative && !m_profileId.isEmpty()) params.insert("profileId", m_profileId);
        nativeRequest(Call::Snapshot, params); return;
    }
    if (!acquireBulk()) { m_deferredSnapshot = true; m_timeout.stop(); return; }
    QJsonObject params;
    if (!m_statePath.isEmpty()) params.insert("statePath", m_statePath);
    if (!m_profileId.isEmpty()) params.insert("profileId", m_profileId);
    const auto id = ++m_nextId;
    m_requests.insert(id, {Call::Snapshot, m_generation, false, false, m_profileId, {}, true});
    m_timeout.start(); send(id, "library.snapshot", params);
}
void BackendClient::nativeRequest(Call call, QJsonObject params, const QJsonObject &launchBinding, const CatalogRequestIdentity &catalogIdentity) {
    static const QStringList names{"native.snapshot", "native.importPreview", "native.importCommit", "native.setEnabled", "native.profile", "native.applyPreview", "native.apply", "native.restore", "native.operation", "native.recover", "native.snapshot", "native.local.prepare", "native.local.status", "native.local.review", "native.local.cancel", "native.local.commit", "native.local.dismiss", "native.fonts.preview", "native.fonts.commit", "native.options.get", "native.options.prepare", "native.options.status", "native.options.cancel", "native.options.dismiss", "native.options.review", "native.options.attach", "native.profileEdit.preview", "native.profileEdit.commit"};
    const bool launch = LaunchData::call(call);
    const bool catalog = catalogCall(call);
    const bool remote = RemoteData::call(call);
    const bool recoveryInspect = call==Call::RecoveryInspect;
    const bool repairPreference = call==Call::SetFixEnabled || call==Call::SetFixAutomatically;
    const bool hero = call==Call::HeroDetect;
    const bool updates = call==Call::ModUpdatesGet;
    const bool health = call==Call::HealthCheck || call==Call::HealthGet;
    const bool bisect = int(call)>=185 && int(call)<=187;
    const bool config = int(call)>=181 && int(call)<=184;
    const bool bulkLibrary = call==Call::SetModsEnabled || call==Call::RemoveModsFromProfile || call==Call::DeleteMods;
    const bool bootstrap = call==Call::LibraryInspect || call==Call::LibraryInitialize;
    // Wait for the closed-owner receipt before a history read can create
    // its replacement; otherwise the retirement poll observes the new owner.
    if(launch && m_launchRetiring && LaunchData::asynchronous(call)) {emit rejected(call,"launchBusy",tr("The previous launch owner is still retiring."));return;}
    if (int(call)<0 || (!launch && !catalog && !remote && !bootstrap && !recoveryInspect && !bulkLibrary && !repairPreference && !hero && !updates && !health && !bisect && !config && call!=Call::PerformanceGet && call!=Call::PerformanceSelect && int(call)>=names.size())) { emit rejected(call,"unsupportedAction",tr("Unknown helper action.")); return; }
    static const QStringList catalogNames{"native.catalog.open","native.catalog.begin","native.catalog.status","native.catalog.cancel","native.catalog.release","native.catalog.close"};
    const auto method = hero ? QStringLiteral("native.hero.detect") : updates ? QStringLiteral("native.modUpdates.get") : repairPreference ? (call==Call::SetFixEnabled ? QStringLiteral("native.setFixEnabled") : QStringLiteral("native.setFixAutomatically")) : health ? (call==Call::HealthCheck ? QStringLiteral("native.health.check") : QStringLiteral("native.health.get")) : bisect ? QStringLiteral("native.bisect.")+QStringList{"start","answer","finish"}.at(int(call)-185) : config ? QStringLiteral("native.config.")+QStringList{"get","preview","commit","decode"}.at(int(call)-181) : call==Call::PerformanceGet ? QStringLiteral("native.performance.get") : call==Call::PerformanceSelect ? QStringLiteral("native.performance.select") : bulkLibrary ? (call==Call::SetModsEnabled ? QStringLiteral("native.setModsEnabled") : (call==Call::DeleteMods ? QStringLiteral("native.deleteMods") : QStringLiteral("native.removeModsFromProfile"))) : recoveryInspect ? QStringLiteral("native.recovery.inspect") : bootstrap ? (call==Call::LibraryInspect ? QStringLiteral("native.library.inspect") : QStringLiteral("native.library.initialize")) : catalog ? catalogNames.at(int(call)-45) : remote ? RemoteData::method(call) : launch ? LaunchData::method(call) : names.at(static_cast<int>(call));
    if(recoveryInspect) {
        if(!params.isEmpty() || !LaunchData::root(QJsonValue(m_dataDir)) || m_dataDir.toUtf8().size()>4096) {emit rejected(call,"invalidParams",tr("Invalid recovery inspection context."));return;}
        params.insert("version",1);
    }
    if(bootstrap) {
        const bool input=call==Call::LibraryInspect ? params.isEmpty()
            : catalogKeys(params,{"version","operationId","expectedRevision"}) && params["version"]==QJsonValue(1)
                && LaunchData::hex(params["operationId"],32) && params["expectedRevision"]==QJsonValue("0");
        if(!input || !LaunchData::root(QJsonValue(m_dataDir)) || m_dataDir.toUtf8().size()>4096){emit rejected(call,"invalidParams",tr("Invalid library creation context."));return;}
        if(call==Call::LibraryInspect)params.insert("version",1);
    }
    if(catalog){
        const bool delayed=m_writing||std::any_of(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return r.bulk;});
        if(m_localSession||!m_ready||delayed){
            if(delayed)emit catalogDelayed(call,params["requestKey"].toString());
            emit catalogRejected(call,params["requestKey"].toString(),delayed?"catalogBusy":"catalogUnavailable",tr("Catalog admission is unavailable or delayed by shared helper work."));return;
        }
        if(std::count_if(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return catalogCall(r.call);})>=8 || (m_process&&m_process->bytesToWrite()>256*1024-65536)){
            emit catalogRejected(call,params["requestKey"].toString(),"catalogBusy",tr("Catalog control capacity is retained; wait for its replies."));return;
        }
        for(const auto &r:std::as_const(m_requests))if(r.call==call&&r.params["catalogSession"]==params["catalogSession"]&&r.params["requestKey"]==params["requestKey"]){
            emit catalogRejected(call,params["requestKey"].toString(),"catalogBusy",tr("This catalog control already has a reply pending."));return;
        }
        params.insert("version",1);
        const auto root=m_catalogRoot.isEmpty()?m_dataDir:m_catalogRoot;
        if(root.isEmpty()){emit catalogRejected(call,params["requestKey"].toString(),"invalidParams",tr("Catalog requires an explicit data root."));return;}
        params.insert("dataDir",QDir::cleanPath(root));
    }
    if(remote) {
        params.insert("version",1);
        params.insert("dataDir",m_dataDir);
        if(!RemoteData::request(call,params)) {
            emit remoteRejected(call,params,"invalidParams",tr("The download request does not match its captured context."));return;
        }
        const bool continuation=call==Call::RemoteOpen || call==Call::RemoteList || call==Call::RemoteStatus || call==Call::RemoteReplaceList || call==Call::RemoteReplaceStatus
            || call==Call::RemoteResolveStatus || call==Call::RemoteResolveRelease || call==Call::RemoteControl
            || call==Call::RemoteCandidates || call==Call::RemoteCandidateMembers || call==Call::RemoteDiagnostics
            || call==Call::RemotePrepareRelease || call==Call::RemoteClose
            || ((call==Call::RemoteCommit || call==Call::RemoteReplaceCommit) && params["reviewToken"].isNull());
        const bool wrongOwner=m_remoteOwned && (params["dataDir"]!=QJsonValue(m_remoteRoot) || params["sessionId"]!=QJsonValue(m_remoteSession));
        if(m_localSession || !m_ready
            || !LaunchData::root(params["dataDir"])
            || !LaunchData::hex(params["sessionId"],32) || wrongOwner || (!continuation && (m_launchOwned || m_writing || m_remoteRecoveryOwned))) {
            emit remoteRejected(call,params,"remoteBusy",tr("Download admission is waiting for the current library work."));return;
        }
        if(std::count_if(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return RemoteData::call(r.call);})>=32
            || (m_process && m_process->bytesToWrite()>256*1024-65536)) {
            emit remoteRejected(call,params,"remoteBusy",tr("Download request capacity is retained. Try again."));return;
        }
    }
    if ((params.contains("deploymentLayout") && ((call!=Call::ApplyPreview&&call!=Call::Apply)||params.value("deploymentLayout")!=QJsonValue("shards-v1")))
        || (params.contains("capacityLayout") && (call!=Call::ProfileEditPreview||params.value("capacityLayout")!=QJsonValue("shards-v1")))) {
        emit rejected(call,"unsupportedLayout",tr("The request does not match the reviewed folder layout. Reconnect and review again."));return;
    }
    const bool localAllowed = OptionData::localCall(call);
    const bool ownerRead = call==Call::Snapshot || call==Call::RecoverySnapshot || call==Call::Operation || recoveryInspect;
    if (!remote && ((m_localSession && !localAllowed) || !m_ready || (m_writing && (!launch || LaunchData::admitsOwner(call))) || (m_launchOwned && !launch && !catalog && !ownerRead))) {
        if(call==Call::Snapshot)m_bootstrapSnapshotRequested=false;
        if(!m_ready && !m_stopReason.isEmpty()) {
            emit rejected(call, "helperStopped", tr("The helper stopped after an error: %1 Restart Modlock to reconnect.").arg(m_stopReason)); return;
        }
        emit rejected(call, "unsupportedAction", tr("Wait for the current operation to finish, or reconnect to the helper.")); return;
    }
    const bool mutation = repairPreference || bisect || call==Call::PerformanceSelect || bulkLibrary || call == Call::LibraryInitialize || call == Call::ImportCommit || call == Call::SetEnabled || call == Call::Profile || call == Call::Apply || call == Call::Restore || call == Call::Recover || call == Call::LocalCommit || call == Call::FontsCommit || call == Call::OptionsAttach || call == Call::ProfileEditCommit;
    const bool bulk = call==Call::PerformanceGet || mutation || call == Call::Snapshot || call == Call::RecoverySnapshot || call == Call::Operation
        || call == Call::ImportPreview || call == Call::ApplyPreview || call == Call::LocalReview || call == Call::FontsPreview
        || call == Call::OptionsGet || call == Call::OptionsReview || call == Call::ProfileEditPreview
        || ((call == Call::LocalStatus || call == Call::OptionsStatus) && params.value("includeCandidates").toBool());
    auto wireParams = params;
    if (!catalog && !m_dataDir.isEmpty()) wireParams.insert("dataDir",m_dataDir);
    const QJsonObject envelope{{"jsonrpc","2.0"},{"id",m_nextId+1},{"method",method},{"params",wireParams}};
    if (QJsonDocument(envelope).toJson(QJsonDocument::Compact).size()+1 > 64*1024) {
        if(call==Call::Snapshot)m_bootstrapSnapshotRequested=false;
        if(remote)emit remoteRejected(call,params,"requestTooLarge",tr("The complete download request exceeds 64 KiB."));
        else if(catalog)emit catalogRejected(call,params["requestKey"].toString(),"invalidParams",tr("The complete catalog request exceeds 64 KiB."));
        else emit rejected(call,"requestTooLarge",tr("The complete request exceeds 64 KiB. Reduce the reviewed attachment batch; full install order cannot be truncated.")); return;
    }
    if (bulk && !acquireBulk()) {
        if (call == Call::Snapshot) m_deferredSnapshot = true;
        else m_deferredBulk = std::make_pair(call, params);
        m_timeout.stop(); return;
    }
    if (!catalog && !m_dataDir.isEmpty()) params.insert("dataDir", m_dataDir);
    const auto id = ++m_nextId;
    const auto generation=call==Call::LibraryInspect ? ++m_inspectGeneration : m_generation;
    m_requests.insert(id, {call, generation, true, mutation, call == Call::Profile ? QString{} : params.value("profileId").toString(), params.value("jobId").toString(), bulk});
    if(config || call==Call::ApplyPreview)m_requests[id].params=params;
    if(bootstrap || recoveryInspect || (call==Call::Snapshot && m_bootstrapSnapshotRequested)) {
        m_requests[id].params=params;m_requests[id].binding={{"root",m_dataDir}};
    }
    if(call==Call::Operation && (launchBinding["remoteOperation"]==QJsonValue(true) || launchBinding["recoveryOperation"]==QJsonValue(true)))m_requests[id].binding=launchBinding;
    if(call==Call::Snapshot && m_bootstrapSnapshotRequested){m_requests[id].bootstrapSnapshot=true;m_bootstrapSnapshotRequested=false;}
    m_requests[id].mod = params.value("modId").toString();
    m_requests[id].revision = params.value("expectedRevision").toString();
    m_requests[id].deploymentLayout=params.value("deploymentLayout").toString();
    m_requests[id].capacityLayout=params.value("capacityLayout").toString();
    m_requests[id].game=params.value("gamePath").toString();
    m_requests[id].operationId=params.value("operationId").toString();
    if(catalog){
        m_requests[id].params=params;
        m_requests[id].catalogIdentity=catalogIdentity;
        m_requests[id].catalogIdentity.call=int(call);
        m_requests[id].catalogIdentity.root=m_catalogRoot.isEmpty()?m_dataDir:m_catalogRoot;
        m_requests[id].catalogIdentity.rootBound=!m_catalogRoot.isEmpty();
        if(!m_catalogRoot.isEmpty())m_requests[id].binding={{"root",m_catalogRoot},{"catalogSession",m_catalogSession}};
        // An unacknowledged open may already own a worker/durable cache step.
        if(call==Call::CatalogOpen){m_catalogOwned=true;emit lifecycleChanged();}
    }
    if(remote) {
        m_requests[id].params=params;
        // A lost open/work acknowledgement can conceal a live owner.
        if(!m_remoteOwned) {m_remoteOwned=true;m_remoteRoot=m_dataDir;m_remoteSession=params["sessionId"].toString();emit lifecycleChanged();}
    }
    if (launch) {
        m_requests[id].params=params; m_requests[id].binding=launchBinding;
        if (LaunchData::asynchronous(call)) m_launchControls.insert(params["controlId"].toString(),{call,params});
        if (call==Call::LaunchControl) { const auto original=m_launchControls.value(params["controlId"].toString(),{Call::Snapshot,{}});m_requests[id].receiptCall=original.first;m_requests[id].receiptParams=original.second; }
        // A lost write/acceptance can conceal durable owner admission. Latch before send.
        if (LaunchData::admitsOwner(call)) { m_launchOwned=true; emit lifecycleChanged(); }
    }
    if (mutation) { m_writing=true; emit lifecycleChanged(); }
    // Native snapshots can rehash changed payloads too. Native operations never
    // share the five-second legacy read timeout.
    // Inspection can run alongside the legacy fallback; it must not cancel
    // that read's timeout while the helper processes both requests.
    if(call!=Call::LibraryInspect)m_timeout.stop();
    send(id, method, params);
}
void BackendClient::drainStderr() {
    if (!m_process) return;
    // Limit each drain so a continuously noisy peer cannot starve GUI events.
    m_process->setReadChannel(QProcess::StandardError);
    const auto bytes = m_process->read(64 * 1024);
    const bool more = m_process->bytesAvailable() > 0;
    m_process->setReadChannel(QProcess::StandardOutput);
    m_stderr.append(bytes.right(8192));
    if (m_stderr.size() > 8192) m_stderr.remove(0, m_stderr.size() - 8192);
    if (more) {
        const auto session = m_session;
        QTimer::singleShot(0, this, [this, session] { if (m_session == session) drainStderr(); });
    }
}
void BackendClient::receive() {
    while (m_process && m_process->bytesAvailable() > 0) {
        const auto capacity = responseLimit - m_stdout.size();
        if (capacity <= 0) {
            fail(tr("Helper response exceeds the 32 MiB limit.")); return;
        }
        // Do not consume bytes from the next small frame into a full bulk frame.
        const auto chunk = m_process->read(std::min<qsizetype>(64 * 1024, capacity));
        m_stdout.append(chunk);
        decodeNext();
    }
}
void BackendClient::decodeNext() {
    if (m_parsing || !m_process) return;
    // A large frame arrives in many pipe reads. Scan each byte only once.
    const auto newline = m_stdout.indexOf('\n', m_stdoutScanned);
    if (newline < 0) { m_stdoutScanned = m_stdout.size(); return; }
    m_stdoutScanned = 0;
    auto line = m_stdout.left(newline); m_stdout.remove(0, newline + 1);
    m_parsing = true;
    const auto session = m_session;
    auto *decoder = new ResponseDecoder(std::move(line),m_requests);
    connect(decoder,&ResponseDecoder::recoveryDecoded,this,[this,session](qint64 id,const QVariantMap &value,const QString &kind,const QString &message){
        if(m_session!=session)return;
        m_parsing=false;
        if(m_requests.contains(id) && m_requests.value(id).call==Call::RecoveryInspect) {
            const auto request=m_requests.take(id);
            if(request.params["dataDir"]==QJsonValue(m_dataDir)) {
                if(kind.isEmpty())emit recoveryInspectionReady(request.params,value);
                else emit rejected(Call::RecoveryInspect,kind,message.left(512));
            }
        }
        decodeNext();
    },Qt::QueuedConnection);
    connect(decoder,&ResponseDecoder::remoteDecoded,this,[this,session](qint64 id,const QVariantMap &value,const LibrarySnapshot &snapshot,bool snapshotValid,const QString &kind,const QString &message){
        if(m_session!=session)return;
        m_parsing=false;
        if(m_requests.contains(id) && RemoteData::call(m_requests.value(id).call)) {
            const auto request=m_requests.take(id);
            if(!kind.isEmpty())emit remoteRejected(request.call,request.params,kind,message);
            else if(snapshotValid)emit remoteSnapshot(request.call,request.params,snapshot);
            else {
                if(request.call==Call::RemoteClose && value.value("phase")=="closed" && request.params["dataDir"]==QJsonValue(m_remoteRoot)
                    && request.params["sessionId"]==QJsonValue(m_remoteSession)) {
                    // A closed remote owner and the separately held recovery
                    // custody retire independently. The latter still prevents
                    // the idle-kill path until its exact terminal proof.
                    m_remoteOwned=false;m_remoteRoot.clear();m_remoteSession.clear();emit lifecycleChanged();
                }
                emit remoteReady(request.call,request.params,value);
            }
        }
        decodeNext();
    },Qt::QueuedConnection);
    connect(decoder,&ResponseDecoder::inspectionDecoded,this,[this,session](qint64 id,const LibraryInitializationInspection &inspection,const QString &kind,const QString &message){
        if(m_session!=session)return;
        m_parsing=false;
        if(m_requests.contains(id) && m_requests.value(id).call==Call::LibraryInspect){
            const auto request=m_requests.take(id);
            if(request.generation==m_inspectGeneration && request.params["dataDir"]==QJsonValue(m_dataDir)) {
                if(kind.isEmpty())emit bootstrapInspectionReady(inspection);
                else emit rejected(Call::LibraryInspect,kind,message.left(4096));
            }
        }
        decodeNext();
    },Qt::QueuedConnection);
    connect(decoder,&ResponseDecoder::catalogDecoded,this,[this,session](qint64 id,const CatalogDecodedResult &result){
        if(m_session!=session)return;
        m_parsing=false;
        if(m_requests.contains(id)&&catalogCall(m_requests.value(id).call)){
            const auto request=m_requests.take(id);
            if(!result.valid)emit catalogRejected(request.call,request.params["requestKey"].toString(),"catalogInvalidEvidence",tr("Catalog returned invalid or mismatched evidence; its receipt remains retained."));
            else if(result.phase=="rejected")emit catalogRejected(request.call,request.params["requestKey"].toString(),result.errorKind,result.error);
            else {
                if(request.call==Call::CatalogOpen)m_catalogSession=result.session;
                if((request.call==Call::CatalogOpen||(request.call==Call::CatalogStatus&&request.params["requestKey"].isNull()))&&result.phase=="ready"){
                    m_catalogRoot=result.root;m_catalogSession=result.session;
                }
                if(result.phase=="closed"&&result.key.isEmpty()&&result.session==m_catalogSession){m_catalogOwned=false;m_catalogRoot.clear();m_catalogSession.clear();emit lifecycleChanged();}
                emit catalogReady(request.call,result);
            }
        }
        decodeNext();
    },Qt::QueuedConnection);
    connect(decoder,&ResponseDecoder::catalogPayloadRejected,this,[this,session](qint64 id){
        if(m_session!=session)return;
        m_parsing=false;
        if(m_requests.contains(id)&&catalogCall(m_requests.value(id).call)){
            const auto request=m_requests.take(id);
            emit catalogRejected(request.call,request.params["requestKey"].toString(),"catalogInvalidEvidence",tr("Catalog payload exceeds supported depth or decoding bounds."));
        }
        decodeNext();
    },Qt::QueuedConnection);
    connect(decoder, &ResponseDecoder::decoded, this,
        [this, session, frameBytes = newline + 1](bool valid, const QJsonObject &object, const LibrarySnapshot &snapshot, bool validSnapshot, bool validOperationSnapshot, const QVariantMap &localResult, bool validLocal) {
            if (m_session != session) return;
            m_parsing = false;
            if (!valid) fail(tr("Helper returned malformed JSON."));
            else handleResponse(object, snapshot, validSnapshot, validOperationSnapshot, localResult, validLocal, frameBytes);
            decodeNext();
        }, Qt::QueuedConnection);
    QThreadPool::globalInstance()->start(decoder);
}
void BackendClient::handleResponse(const QJsonObject &response, const LibrarySnapshot &snapshot, bool validSnapshot, bool validOperationSnapshot, const QVariantMap &localResult, bool validLocal, qsizetype frameBytes) {
    const auto token = response.value("id");
    const auto id = token.toInteger(-1);
    if (response.value("jsonrpc").toString() != "2.0" || !token.isDouble() || id <= 0
        || response.contains("result") == response.contains("error")) {
        fail(tr("Helper returned an invalid JSON-RPC response.")); return;
    }
    if (id != m_helloId && !m_requests.contains(id)) {
        if (id <= m_nextId) return;
        fail(tr("Helper returned an unexpected request ID.")); return;
    }
    if (id == m_helloId) {
        const auto mismatch=tr("The app and helper do not match. Reinstall Modlock so both come from the same build.");
        if (response.contains("error")) { fail(mismatch); return; }
        const auto result = response.value("result").toObject();
        const auto capabilities = result.value("capabilities");
        QSet<QString> features;
        bool valid = capabilities.isArray();
        for (const auto &entry : capabilities.toArray()) {
            if (!entry.isString()) valid = false;
            else features.insert(entry.toString());
        }
        const bool complete = std::all_of(std::begin(requiredFeatures),std::end(requiredFeatures),
            [&features](const char *feature){return features.contains(QLatin1String(feature));});
        if (!result.value("protocolVersion").isDouble() || result.value("protocolVersion").toInteger(-1) != 1
            || !result.value("readOnly").isBool() || result.value("readOnly").toBool() || !valid || !complete) {
            fail(mismatch); return;
        }
        m_helloId = 0; m_ready = true; m_timeout.stop(); emit capabilitiesReady();
        if (!m_localSession) sendSnapshot(); return;
    }
    const auto request = m_requests.take(id);
    if (request.bulk) releaseBulk();
    if (!request.mutation && (request.call == Call::Snapshot || request.call == Call::HeroDetect) && request.generation != m_generation) {if(request.call==Call::HeroDetect)emit heroSettled();return;}
    m_timeout.stop();
    if (OptionData::localCall(request.call) && !request.bulk && frameBytes > 48 * 1024) {
        fail(tr("Helper returned an invalid local import response.")); return;
    }
    // Keep mutation context until its complete result has passed validation.
    if (response.contains("error")) {
        const auto error = response.value("error").toObject();
        const auto kind = error.value("data").toObject().value("kind");
        if (!error.value("code").isDouble() || !error.value("message").isString() || !kind.isString()) {
            fail(tr("Helper returned an invalid error response.")); return;
        }
        if (request.mutation && (error.value("code").toInteger() == -32603 || kind.toString() == "responseTooLarge")) {
            if(request.call==Call::LibraryInitialize) {
                // This terminal response leaves publication uncertain, but the
                // helper's dispatch has returned. Preserve its catalog session.
                m_writing=false;emit rejected(request.call,kind.toString(),error.value("message").toString().left(4096));return;
            }
            // Serialization occurs after dispatch: this is not evidence of a
            // rejected mutation. Keep its marker and detach/drain the writer.
            fail(kind.toString() + ": " + error.value("message").toString().left(4096)); return;
        }
        if(request.mutation)m_writing = false;
        if (request.native && request.call == Call::Snapshot && !request.bootstrapSnapshot && kind.toString() == "nativeNotImported") {
            emit nativeMissing(); m_nativeView = false; sendSnapshot(); return;
        }
        if(request.call==Call::HealthCheck || request.call==Call::HealthGet) {
        if(response.contains("error")){const auto e=response.value("error").toObject();emit rejected(request.call,e.value("data").toObject().value("kind").toString(),e.value("message").toString());return;}
        const auto v=response.value("result").toObject();
        if(!v["running"].isBool() || !v["checked"].isDouble() || !v["total"].isDouble() || !(v["report"].isObject()||v["report"].isNull())) {emit rejected(request.call,"invalidEvidence",tr("Invalid health response."));return;}
        emit healthReady(v.toVariantMap());return;
    }
    if (int(request.call)>=181 && int(request.call)<=184) {emit configRejected(request.params.value("kind").toString(),error.value("message").toString().left(4096));return;}
        // No old manager is a normal discovery result, not a failed request.
        if (!request.native && request.call == Call::Snapshot && m_statePath.isEmpty() && kind.toString() == "sourceNotFound") {
            emit upstreamMissing(); return;
        }
        if (request.native) emit rejected(request.call, kind.toString(), error.value("message").toString().left(4096));
        else emit failed(kind.toString() + ": " + error.value("message").toString().left(4096));
        return;
    }
    if (LaunchData::call(request.call)) {
        if (!validLocal || frameBytes>65536) {
            qWarning().noquote() << "Invalid launch reply:" << LaunchData::method(request.call) << "bytes:" << frameBytes;
            fail(tr("Helper returned invalid launch evidence. Reconnect to check its saved outcome.")); return;
        }
        emit launchReady(request.call,localResult);
        if(request.call==Call::LaunchClose) {
            if(remoteOwned()) {m_launchRetiring=true;m_launchCloseSent=true;m_launchRetirementTimer.start();emit lifecycleChanged();}
            else retireLaunch();
        } else if(request.call==Call::LaunchOwner && m_launchRetiring && request.params["dataDir"]==QJsonValue(m_dataDir)
                  && localResult.value("state")=="closed") {
            m_launchRetiring=false;m_launchCloseSent=false;m_launchRetirementTimer.stop();m_launchOwned=false;m_launchWriting=false;
            emit lifecycleChanged();emit launchRetired();
        }
        return;
    }
    if(request.call==Call::HeroDetect) {
        const auto v=response.value("result").toObject();
        if(v.size()!=4 || v["modId"]!=QJsonValue(request.mod) || v["revision"]!=QJsonValue(request.revision)
           || !v["fingerprint"].isString() || !(v["heroDisplay"].isNull() || (v["heroDisplay"].isString() && v["heroDisplay"].toString().size()<=128))) {
            emit rejected(request.call,"invalidEvidence",tr("Invalid hero detection response."));return;
        }
        emit heroReady(v.toVariantMap());emit heroSettled();return;
    }
    if(request.call==Call::ModUpdatesGet) {
        const auto v=response.value("result").toObject();
        if(!v["mods"].isArray() || !v["profileId"].isString() || !v["revision"].isString() || !(v["lastUpdateCheck"].isNull() || v["lastUpdateCheck"].isDouble())) {emit rejected(request.call,"invalidEvidence",tr("Invalid mod update response."));return;}
        QSet<QString> seen;
        for(const auto &entry:v["mods"].toArray()) {const auto row=entry.toObject();bool numeric=false;const auto file=row["fileId"].toString();const auto number=file.toULongLong(&numeric);const bool available=row["status"]==QJsonValue("available");if((row.contains("recordDigest") && !LaunchData::hex(row["recordDigest"],64)) || !row["modId"].isString() || row["modId"].toString().isEmpty() || seen.contains(row["modId"].toString()) || !QStringList{"available","current","unsure"}.contains(row["status"].toString()) || !SubmissionKey::decode(row["submission"]) || (available && (!numeric || !number || QString::number(number)!=file || !row["createdAt"].isDouble() || row["createdAt"].toDouble()<0))) {emit rejected(request.call,"invalidEvidence",tr("Invalid mod update row."));return;}seen.insert(row["modId"].toString());}
        emit modUpdatesReady(v.toVariantMap());return;
    }
    if(request.call==Call::HealthCheck || request.call==Call::HealthGet) {
        if(response.contains("error")){const auto e=response.value("error").toObject();emit rejected(request.call,e.value("data").toObject().value("kind").toString(),e.value("message").toString());return;}
        const auto v=response.value("result").toObject();
        if(!v["running"].isBool() || !v["checked"].isDouble() || !v["total"].isDouble() || !(v["report"].isObject()||v["report"].isNull())) {emit rejected(request.call,"invalidEvidence",tr("Invalid health response."));return;}
        emit healthReady(v.toVariantMap());return;
    }
    if (int(request.call)>=181 && int(request.call)<=184) {
        const auto value=response.value("result");
        if(!value.isObject()){emit configRejected(request.params.value("kind").toString(),tr("Invalid config response."));return;}
        auto data=value.toObject().toVariantMap();data.insert("kind",request.params.value("kind").toString());
        emit configReady(request.call,data);return;
    }
    if (request.call == Call::PerformanceGet) {
        const auto value=response.value("result").toObject();
        if(!value.value("presets").isArray() || !value.value("selection").isString() || !value.value("detected").isObject()) {emit rejected(request.call,"invalidEvidence",tr("Invalid performance catalog response."));return;}
        emit performanceReady(value.toVariantMap());return;
    }
    if (request.call == Call::Snapshot || request.call == Call::RecoverySnapshot || request.mutation) {
        if (!validSnapshot || (request.native && snapshot.sourceKind != "native")
            || (request.mutation && !request.revision.isEmpty() && snapshot.revision.toULongLong()<request.revision.toULongLong())
            || (!request.profile.isEmpty() && snapshot.selectedProfileId != request.profile)) {
            if(request.bootstrapSnapshot){emit rejected(Call::Snapshot,"invalidEvidence",tr("The current native snapshot is invalid or does not match the requested root."));return;}
            if(request.call==Call::LibraryInitialize){m_writing=false;emit rejected(request.call,"invalidEvidence",tr("The creation receipt is invalid or mismatched. Check creation again."));return;}
            fail(tr("Helper returned an invalid library snapshot or mismatched profile.")); return;
        }
        if (!request.native && request.profile.isEmpty()) {
            const bool exists = std::any_of(snapshot.profiles.cbegin(), snapshot.profiles.cend(), [&snapshot](const QVariant &p) { return p.toMap().value("id").toString() == snapshot.activeProfileId; });
            if (exists && snapshot.selectedProfileId != snapshot.activeProfileId) { fail(tr("Helper returned a mismatched profile.")); return; }
        }
        if(request.mutation)m_writing = false;
        if(request.call==Call::LibraryInitialize) {
            if(request.params["dataDir"]!=QJsonValue(m_dataDir) || request.params["operationId"]!=QJsonValue(request.operationId)){
                emit rejected(request.call,"invalidEvidence",tr("Library creation returned mismatched operation context."));return;
            }
            emit bootstrapInitialized(m_dataDir,request.operationId);return;
        }
        if(request.bootstrapSnapshot) {
            if(request.params["dataDir"]!=QJsonValue(m_dataDir)){emit rejected(Call::Snapshot,"invalidEvidence",tr("The current library snapshot belongs to a different root."));return;}
            m_nativeView=true;emit bootstrapSnapshotReady(snapshot);return;
        }
        if (request.call==Call::SetModsEnabled || request.call==Call::RemoveModsFromProfile || request.call==Call::DeleteMods) emit bulkResultReady(localResult.value("bulkResult").toMap());
        if (request.call == Call::LocalCommit) { emit localCommitted(snapshot); emit completed(request.call); return; }
        if (request.call == Call::FontsCommit) { emit fontsCommitted(snapshot); emit completed(request.call); return; }
        if (request.call == Call::OptionsAttach) { emit optionsCommitted(snapshot); return; }
        if (request.call == Call::ProfileEditCommit) { emit profileEditCommitted(snapshot); return; }
        if (request.call == Call::Apply) { emit applyCommitted(snapshot); return; }
        if (request.call == Call::Recover) { emit recoverCommitted(snapshot,localResult.value("recoverySnapshotDigest").toString()); emit completed(request.call); return; }
        if (request.call == Call::RecoverySnapshot) {
            emit recoverySnapshotReady(snapshot);
        } else {
            if (request.native) m_nativeView = true;
            emit snapshotReady(snapshot);
        }
        if (request.mutation) emit completed(request.call); return;
    }
    if (request.call == Call::FontsPreview) {
        if (!validLocal || !localResult.contains("summary")) { fail(tr("Helper returned an invalid font review.")); return; }
        emit reviewReady(request.call,localResult); return;
    }
    if (OptionData::optionsCall(request.call) || request.call == Call::ProfileEditPreview) {
        bool shape = false;
        switch(request.call) {
        case Call::OptionsGet: shape=localResult.contains("detail") && localResult.value("modId").toString()==request.mod; break;
        case Call::OptionsPrepare: case Call::OptionsCancel: shape=localResult.value("kind")=="localOptions" && !localResult.contains("candidateSummary"); break;
        case Call::OptionsStatus: shape=request.job.isEmpty() ? localResult.contains("jobs") : localResult.value("kind")=="localOptions" && localResult.contains("candidateSummary")==request.bulk; break;
        case Call::OptionsDismiss: shape=localResult.value("dismissed").toBool(); break;
        case Call::OptionsReview: shape=localResult.value("summary").toMap().contains("attachedOptionCount"); break;
        case Call::ProfileEditPreview: shape=localResult.value("summary").toMap().contains("canCommit")
            && localResult.value("summary").toMap().value("version").toInt()==(request.capacityLayout=="shards-v1"?2:1); break;
        default: break;
        }
        if(!shape || !validLocal || (!request.job.isEmpty() && localResult.value("jobId").toString()!=request.job && request.call!=Call::OptionsReview)
            || (!request.profile.isEmpty() && localResult.value("profileId").toString()!=request.profile)
            || (!request.mod.isEmpty() && localResult.value("modId").toString()!=request.mod)
            || (!request.revision.isEmpty() && localResult.value("expectedRevision").toString()!=request.revision)) {
            fail(tr("Helper returned an invalid options or profile-edit response.")); return;
        }
        if(request.call==Call::ProfileEditPreview) emit reviewReady(request.call,localResult);
        else emit localReady(request.call,localResult);
        return;
    }
    if (OptionData::importCall(request.call)) {
        bool shape = false;
        switch (request.call) {
        case Call::LocalPrepare: case Call::LocalCancel:
            shape = localResult.contains("phase") && !localResult.contains("candidateSummary"); break;
        case Call::LocalStatus:
            shape = request.job.isEmpty() ? localResult.contains("jobs")
                : localResult.contains("phase") && localResult.contains("candidateSummary") == request.bulk; break;
        case Call::LocalReview: shape = localResult.contains("token") || localResult.contains("groupRows"); break;
        case Call::LocalDismiss: shape = localResult.value("dismissed").toBool(); break;
        default: break;
        }
        if (!shape || !validLocal || (!request.job.isEmpty() && localResult.value("jobId").toString() != request.job)) {
            fail(tr("Helper returned an invalid local import response.")); return;
        }
        emit localReady(request.call, localResult); return;
    }
    if(request.call==Call::ApplyPreview && request.params["restoreReview"]==QJsonValue(true)) {
        const auto review=response.value("result").toObject();
        if(review.size()!=1 || review.value("restoreReview")!=QJsonValue(true)) {
            fail(tr("Helper returned an invalid Restore review."));return;
        }
        emit reviewReady(request.call,review.toVariantMap());return;
    }
    if(request.call==Call::ApplyPreview && !request.deploymentLayout.isEmpty()) {
        if(!validLocal||localResult.value("summaryVersion").toInt()!=2||localResult.value("deploymentLayout").toString()!=request.deploymentLayout||localResult.value("profileId").toString()!=request.profile||QDir::cleanPath(localResult.value("gamePath").toString())!=QDir::cleanPath(request.game)) {
            fail(tr("Helper returned a folder review that does not match the requested layout. Review again."));return;
        }
        emit reviewReady(request.call,localResult);return;
    }
    const auto result = response.value("result").toObject();
    if(request.call==Call::ApplyPreview && (result.contains("summaryVersion")||result.contains("deploymentLayout")||result.contains("shardCount")||result.contains("shards"))) {
        fail(tr("Helper returned a new folder layout without the requested acknowledgement."));return;
    }
    auto integer = [](const QJsonValue &v) { return v.isDouble() && v.toDouble() >= 0 && v.toDouble() == double(v.toInteger(-1)); };
    auto warnings = [](const QJsonValue &v) {
        if (!v.isArray()) return false;
        for (const auto &w : v.toArray()) if (!w.isObject() || !w.toObject().value("code").isString() || !w.toObject().value("message").isString()) return false;
        return true;
    };
    if (request.call == Call::Operation) {
        const auto status = result.value("status").toString();
        bool valid = !request.operationId.isEmpty() && result.value("operationId")==QJsonValue(request.operationId)
            && (status == "unknown" || status == "recoveryRequired" || status == "failed" || status == "succeeded");
        if (status == "succeeded") valid = valid && validOperationSnapshot;
        if (status == "failed") valid = valid && result.value("error").toObject().value("kind").isString() && result.value("error").toObject().value("message").isString();
        if (!valid) { fail(tr("Helper returned an invalid operation outcome.")); return; }
        if(status=="succeeded" && localResult.contains("bulkResult")) emit bulkResultReady(localResult.value("bulkResult").toMap());
        if(status=="succeeded") emit operationFontOffers(result.value("operationId").toString(),localResult.value("confirmedFontGame").toString(),localResult.value("confirmedFontMods").toMap());
        // The model reconciles a fresh snapshot; never materialize a possibly
        // huge historical result again on the GUI thread.
        auto outcome = result; outcome.remove("result");
        if(status=="succeeded"){outcome.insert("confirmedRevision",localResult.value("confirmedRevision").toString());outcome.insert("confirmedProfile",localResult.value("confirmedProfile").toString());if(request.binding["recoveryOperation"]==QJsonValue(true))outcome.insert("confirmedSnapshotDigest",localResult.value("confirmedSnapshotDigest").toString());}
        emit operationReady(outcome.toVariantMap()); return;
    }
    bool valid = result.value("token").isString() && !result.value("token").toString().isEmpty()
        && result.value("gamePath").isString() && integer(result.value("totalBytes")) && warnings(result.value("warnings"));
    const QStringList counts{"packageCount", "directoryCount", "physicalFileCount", "companionFileCount"};
    const bool familyCounts = std::any_of(counts.cbegin(), counts.cend(), [&result](const QString &key) { return result.contains(key); });
    if (familyCounts) {
        for (const auto &key : counts) valid = valid && integer(result.value(key));
        const auto packages = result.value("packageCount").toInteger(-1);
        const auto directories = result.value("directoryCount").toInteger(-1);
        const auto physical = result.value("physicalFileCount").toInteger(-1);
        const auto companions = result.value("companionFileCount").toInteger(-1);
        valid = valid && directories <= packages && companions <= physical;
        if (request.call == Call::ApplyPreview)
            valid = valid && packages<=99 && directories <= physical && companions == physical - directories
                && result.value("fileCount").toInteger(-1) == directories;
    }
    if (request.call == Call::ImportPreview) {
        for (const auto *field : {"profileCount", "modCount", "availableModCount", "unavailableModCount"}) valid = valid && integer(result.value(field));
        valid = valid && result.value("profiles").isArray() && result.value("unavailableMods").isArray() && result.value("summariesTruncated").isBool();
        for (const auto &p : result.value("profiles").toArray()) valid = valid && p.isObject() && p.toObject().value("id").isString() && p.toObject().value("name").isString() && integer(p.toObject().value("modCount"));
        for (const auto &m : result.value("unavailableMods").toArray()) for (const auto *key : {"profileId", "modId", "title", "reason"}) valid = valid && m.toObject().value(key).isString();
    } else {
        valid = valid && result.value("profileId").toString() == request.profile && integer(result.value("fileCount")) && result.value("replacedMounts").isArray();
        if(request.call==Call::ApplyPreview)valid=valid&&result.value("fileCount").toInteger(-1)<=99;
        for (const auto &mount : result.value("replacedMounts").toArray()) valid = valid && mount.isString();
    }
    if (!valid) { fail(tr("Helper returned an invalid review.")); return; }
    emit reviewReady(request.call, result.toVariantMap());
}
void BackendClient::fail(const QString &message) {
    m_stopReason = message.left(4096);
    const bool unknown = m_writing || m_launchOwned || remoteOwned();
    if ((unknown || m_catalogOwned) && m_process) retireProcess();
    m_writing = false; stop(); emit lifecycleChanged();
    if (unknown) emit unknownOutcome(message);
    else emit failed(message);
}

#include "backendclient.moc"

void BackendClient::setLaunchWriting(bool value) { if(m_launchWriting==value)return; m_launchWriting=value; emit lifecycleChanged(); }
void BackendClient::setRemoteRecoveryOwned(bool value) { if(m_remoteRecoveryOwned==value)return; m_remoteRecoveryOwned=value;emit lifecycleChanged(); }
void BackendClient::retireLaunch() {
    if(!m_launchOwned&&!m_process)return;
    if(remoteOwned() && m_process) {
        if(!m_launchOwned) {
            // No launch admission was sent on this session. Preserve the shared
            // helper while letting the launch caller retire its local draft.
            emit launchRetired();
            return;
        }
        if(!m_launchRetiring){m_launchRetiring=true;m_launchCloseSent=false;m_launchRetirementTimer.start();emit lifecycleChanged();}
        QTimer::singleShot(0,this,[this]{
            const bool pending=std::any_of(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return r.call==Call::LaunchClose;});
            if(m_launchRetiring && !m_launchCloseSent && !pending)nativeRequest(Call::LaunchClose);
        });
        return;
    }
    retireProcess(); stop();
}

bool BackendClient::catalogControlPending(Call call,const QString &key) const {
    return std::any_of(m_requests.cbegin(),m_requests.cend(),[&](const Request &r){return r.call==call&&r.params["requestKey"].toString()==key;});
}

bool BackendClient::catalogPaused() const { return m_writing || std::any_of(m_requests.cbegin(),m_requests.cend(),[](const Request &r){return r.bulk;}); }
