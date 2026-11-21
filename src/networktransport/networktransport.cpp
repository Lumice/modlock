#include "networktransport.h"
#include <QCoreApplication>
#include <QCryptographicHash>
#include <QDateTime>
#include <QDir>
#include <QDirIterator>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QRandomGenerator>
#include <QSet>
#include <QSocketNotifier>
#include <QUrl>
#include <algorithm>
#include <cerrno>
#include <cstring>
#include <utility>
#include <limits>
#include <QtEndian>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/file.h>
#include <unistd.h>

namespace {
constexpr qsizetype frameLimit = 65536, replyLimit = 16384, pipeLimit = 256 * 1024;
constexpr qint64 bodyLimit = 5 * 1024 * 1024;
bool exact(const QJsonObject &o, std::initializer_list<const char *> keys) {
    return o.size() == qsizetype(keys.size()) && std::all_of(keys.begin(), keys.end(), [&o](auto k){ return o.contains(QLatin1String(k)); });
}
bool token(const QJsonValue &v) {
    if (!v.isString() || v.toString().size() != 32) return false;
    for (auto c : v.toString()) if (!((c >= u'0' && c <= u'9') || (c >= u'a' && c <= u'f'))) return false;
    return true;
}
QString newToken() {
    QByteArray bytes(16, Qt::Uninitialized);
    for (int i = 0; i < 4; ++i) { const auto n = QRandomGenerator::system()->generate(); memcpy(bytes.data()+i*4, &n, 4); }
    return QString::fromLatin1(bytes.toHex());
}
bool allowed(const QUrl &url) {
    const auto bytes = url.toEncoded(QUrl::FullyEncoded);
    return url.isValid() && url.scheme() == QStringLiteral("https") && url.userInfo().isEmpty()
        && !url.hasFragment() && url.port(-1) == -1 && bytes.size() <= 7000
        && (url.host() == QStringLiteral("gamebanana.com") || url.host() == QStringLiteral("api.gamebanana.com"));
}
bool regularOwned(const struct stat &st) { return S_ISREG(st.st_mode) && st.st_uid == geteuid() && st.st_nlink == 1 && (st.st_mode & 077) == 0; }
// JSON already parsed by Qt. This second bounded scanner rejects duplicate
// decoded keys, >16 nesting and numeric spellings that Qt would round/coerce.
class StrictJson {
public:
    explicit StrictJson(const QByteArray &b): b(b) {}
    bool valid() { return value(0) && (space(), i == b.size()); }
private:
    const QByteArray &b; qsizetype i = 0;
    void space() { while (i < b.size() && (b[i]==' '||b[i]=='\r'||b[i]=='\n'||b[i]=='\t')) ++i; }
    bool string(QString *key = nullptr) {
        if (i >= b.size() || b[i++] != '"') return false; const auto start = i-1;
        while (i < b.size()) { const auto c=b[i++]; if(c=='\\') { if(i>=b.size())return false; ++i; }
            else if(c=='"') { if(key) { const auto a=QJsonDocument::fromJson('['+b.mid(start,i-start)+']').array(); if(a.size()!=1||!a[0].isString())return false; *key=a[0].toString(); } return true; } }
        return false;
    }
    bool value(int depth) {
        space(); if (depth > 16 || i >= b.size()) return false;
        const auto c=b[i]; if(c=='"')return string();
        if(c=='{' || c=='[') {
            ++i; space(); const char end=c=='{'?'}':']'; QSet<QString> keys;
            if(i<b.size()&&b[i]==end){++i;return true;}
            for(;;){if(c=='{'){QString key;space();if(!string(&key)||keys.contains(key))return false;keys.insert(key);space();if(i>=b.size()||b[i++]!=':')return false;}
                if(!value(depth+1))return false;space();if(i>=b.size())return false;if(b[i]==end){++i;return true;}if(b[i++]!=',')return false;}
        }
        const auto start=i;while(i<b.size()&&b[i]!=','&&b[i]!=']'&&b[i]!='}'&&b[i]!=' '&&b[i]!='\r'&&b[i]!='\n'&&b[i]!='\t')++i;
        const auto text=b.mid(start,i-start);if(text=="true"||text=="false"||text=="null")return true;
        bool ok=false;const auto n=text.toLongLong(&ok);return ok&&text==QByteArray::number(n)&&text.size()<=20;
    }
};
constexpr qsizetype inventorySlotSize=4096, inventoryHeaderSize=52, inventorySize=8192;
const QByteArray inventoryMagic("DLCINV02",8);
QByteArray inventorySlot(const QByteArray &payload,quint64 sequence){
    if(sequence==0||payload.isEmpty()||payload.size()>inventorySlotSize-inventoryHeaderSize)return {};
    QByteArray slot(inventorySlotSize,'\0');memcpy(slot.data(),inventoryMagic.constData(),8);
    qToBigEndian<quint64>(sequence,slot.data()+8);qToBigEndian<quint32>(quint32(payload.size()),slot.data()+16);
    memcpy(slot.data()+inventoryHeaderSize,payload.constData(),size_t(payload.size()));
    const auto digest=QCryptographicHash::hash(slot.first(20)+slot.sliced(inventoryHeaderSize),QCryptographicHash::Sha256);
    memcpy(slot.data()+20,digest.constData(),32);return slot;
}
bool inventoryRecord(const QByteArray &bytes,QJsonObject &inventory,quint64 &latest){
    if(bytes.size()!=inventorySize)return false;
    latest=0;
    for(int index=0;index<2;++index){
        const auto slot=bytes.mid(index*inventorySlotSize,inventorySlotSize);
        if(slot.first(8)!=inventoryMagic)continue;
        const auto sequence=qFromBigEndian<quint64>(slot.constData()+8);
        const auto length=qFromBigEndian<quint32>(slot.constData()+16);
        if(sequence==0||(sequence-1)%2!=quint64(index)||length==0||length>inventorySlotSize-inventoryHeaderSize)continue;
        if(QCryptographicHash::hash(slot.first(20)+slot.sliced(inventoryHeaderSize),QCryptographicHash::Sha256)!=slot.mid(20,32))continue;
        const auto padding=slot.sliced(inventoryHeaderSize+length);
        if(std::any_of(padding.cbegin(),padding.cend(),[](char c){return c!=0;}))continue;
        const auto payload=slot.mid(inventoryHeaderSize,length);QJsonParseError parse;
        const auto document=QJsonDocument::fromJson(payload,&parse);
        if(parse.error!=QJsonParseError::NoError||!document.isObject()||!StrictJson(payload).valid())continue;
        if(sequence>latest){latest=sequence;inventory=document.object();}
    }
    return latest!=0;
}
QJsonObject errorValue(const char *kind, const char *message) { return {{"kind",QLatin1String(kind)},{"message",QLatin1String(message)},{"retryAfterSeconds",QJsonValue()}}; }
}

NetworkTransport::NetworkTransport(const QString &directory, QObject *parent, int ownerFd): QObject(parent),m_directory(directory),m_session(newToken()) {
    // Every ancestor must be a real directory; the final private namespace must
    // be owned, mode 0700 and contain only the parent inventory/lease.
    if (!QDir::isAbsolutePath(directory) || QDir::cleanPath(directory) != directory) return;
    QString current;
    for (const auto &part : directory.split('/',Qt::SkipEmptyParts)) {
        current += '/' + part; struct stat st{};
        if (::lstat(QFile::encodeName(current).constData(),&st)!=0 || !S_ISDIR(st.st_mode)) return;
    }
    if(ownerFd!=-1){
        if(ownerFd!=3)return;
        // The inherited descriptor is the same locked open file description as
        // the parent's root-scoped owner lease. It intentionally remains open
        // until process exit, including while Qt destructors drain the spool.
        struct stat heldOwner{},namedOwner{};
        QDir ownerDir(QFileInfo(directory).absolutePath());
        if(!ownerDir.cdUp())return;
        const auto ownerPath=QFile::encodeName(ownerDir.filePath(QStringLiteral(".owner.lock")));
        if(::fstat(ownerFd,&heldOwner)!=0||!regularOwned(heldOwner)
            ||::lstat(ownerPath.constData(),&namedOwner)!=0||!regularOwned(namedOwner)
            ||heldOwner.st_dev!=namedOwner.st_dev||heldOwner.st_ino!=namedOwner.st_ino
            ||::flock(ownerFd,LOCK_EX|LOCK_NB)!=0)return;
    }
    const int fd=::open(QFile::encodeName(directory).constData(),O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC);
    if(fd<0)return;
    struct stat st{}; if(::fstat(fd,&st)!=0||st.st_uid!=geteuid()||(st.st_mode&077)!=0){::close(fd);return;}
    if(!m_directoryHandle.open(fd,QIODevice::ReadOnly,QFileDevice::AutoCloseHandle)){::close(fd);return;}
    QDirIterator entries(directory,QDir::AllEntries|QDir::Hidden|QDir::System|QDir::NoDotAndDotDot);
    QSet<QString> names;
    while(entries.hasNext()){entries.next();if(names.size()>=2)return;names.insert(entries.fileName());}
    if(names!=QSet<QString>{".inventory",".lease"})return;
    const int leaseFd=::openat(fd,".lease",O_RDONLY|O_NOFOLLOW|O_CLOEXEC);
    if(leaseFd<0)return;struct stat leaseStat{};
    if(::fstat(leaseFd,&leaseStat)!=0||!regularOwned(leaseStat)||::flock(leaseFd,LOCK_SH|LOCK_NB)!=0){::close(leaseFd);return;}
    if(!m_namespaceLease.open(leaseFd,QIODevice::ReadOnly,QFileDevice::AutoCloseHandle)){::close(leaseFd);return;}
    const int inventoryFd=::openat(fd,".inventory",O_RDWR|O_NOFOLLOW|O_CLOEXEC);
    if(inventoryFd<0)return;struct stat inventoryStat{};m_inventoryHandle=QSharedPointer<QFile>::create();
    if(::fstat(inventoryFd,&inventoryStat)!=0||!regularOwned(inventoryStat)||inventoryStat.st_size!=inventorySize){::close(inventoryFd);return;}
    if(!m_inventoryHandle->open(inventoryFd,QIODevice::ReadWrite,QFileDevice::AutoCloseHandle)){::close(inventoryFd);return;}
    const auto bytes=m_inventoryHandle->read(inventorySize+1);
    if(!inventoryRecord(bytes,m_inventory,m_inventorySequence))return;
    auto decimal=[](const QJsonValue &value,quint64 expected){bool ok=false;const auto text=value.toString();const auto n=text.toULongLong(&ok);return ok&&text==QString::number(n)&&n==expected;};
    const auto fsid=m_inventory.value("fsid").toString();
    const bool inventoryShape=m_inventory.contains("fsid")
        ? exact(m_inventory,{"version","session","device","inode","leaseDevice","leaseInode","slots","bodies","fsid"})
            &&fsid.size()==16&&std::all_of(fsid.cbegin(),fsid.cend(),[](QChar c){return (c>=u'0'&&c<=u'9')||(c>=u'a'&&c<=u'f');})
        : exact(m_inventory,{"version","session","device","inode","leaseDevice","leaseInode","slots","bodies"});
    if(!inventoryShape
        ||m_inventory["version"]!=QJsonValue(1)||!token(m_inventory["session"])||m_inventory["session"].toString()!=QFileInfo(directory).fileName()
        ||!decimal(m_inventory["device"],quint64(st.st_dev))||!decimal(m_inventory["inode"],quint64(st.st_ino))
        ||!decimal(m_inventory["leaseDevice"],quint64(leaseStat.st_dev))||!decimal(m_inventory["leaseInode"],quint64(leaseStat.st_ino))
        ||!m_inventory["bodies"].isArray()||m_inventory["bodies"].toArray().size()>4)return;
    m_slotLimit=int(m_inventory["slots"].toInteger(-1));if(m_slotLimit<1||m_slotLimit>4)return;
    m_clock.start();m_startTimer.setSingleShot(true);
    connect(&m_startTimer,&QTimer::timeout,this,&NetworkTransport::startReady);
    for(int pipe : {STDIN_FILENO,STDOUT_FILENO}) {const int flags=::fcntl(pipe,F_GETFL);if(flags<0||::fcntl(pipe,F_SETFL,flags|O_NONBLOCK)<0)return;}
    m_reader=new QSocketNotifier(STDIN_FILENO,QSocketNotifier::Read,this);
    m_writer=new QSocketNotifier(STDOUT_FILENO,QSocketNotifier::Write,this);m_writer->setEnabled(false);
    connect(m_reader,&QSocketNotifier::activated,this,[this]{readInput();});
    connect(m_writer,&QSocketNotifier::activated,this,[this]{writeOutput();});
    m_valid=true;
}
NetworkTransport::~NetworkTransport() { for(const auto &r:std::as_const(m_requests)){if(r->reply){r->reply->disconnect(this);r->reply->abort();}removeBody(r);} }
QJsonObject NetworkTransport::base(const QString &key) const {
    QJsonObject o{{"version",1},{"transportSession",m_session}};if(!key.isEmpty())o.insert("requestKey",key);return o;
}
void NetworkTransport::enqueue(const QJsonObject &frame) {
    auto bytes=QJsonDocument(frame).toJson(QJsonDocument::Compact)+'\n';
    if(bytes.size()>replyLimit || m_output.size()+bytes.size()>pipeLimit){shutdown();return;}
    m_output+=bytes;m_writer->setEnabled(true);
    // Stop reading before an admitted control could lack response capacity.
    if(m_output.size()>pipeLimit-replyLimit)m_reader->setEnabled(false);
}
void NetworkTransport::respond(const QJsonValue &id,const QJsonObject &result){enqueue({{"jsonrpc","2.0"},{"id",id},{"result",result}});}
void NetworkTransport::reject(const QJsonValue &id,int code,const char *kind,const char *message){enqueue({{"jsonrpc","2.0"},{"id",id},{"error",QJsonObject{{"code",code},{"message",QLatin1String(message)},{"data",QJsonObject{{"kind",QLatin1String(kind)}}}}}});}
void NetworkTransport::readInput() {
    // Byte-wise framing avoids consuming unbounded caller commands ahead of
    // output backpressure. 8 KiB per event keeps network callbacks schedulable.
    for(int n=0;n<8192&&!m_closing&&m_output.size()<=pipeLimit-replyLimit;++n){
        char byte;const auto count=::read(STDIN_FILENO,&byte,1);
        if(count==0){shutdown();return;}
        if(count<0){if(errno==EINTR)continue;if(errno==EAGAIN||errno==EWOULDBLOCK)return;shutdown();return;}
        if(byte=='\n'){
            if(m_oversize)reject({},-32600,"invalidRequest","Request frame exceeds 64 KiB.");else dispatch(m_input);
            m_input.clear();m_oversize=false;
        }else if(m_input.size()>=frameLimit-1){m_oversize=true;m_input.clear();}
        else if(!m_oversize)m_input+=byte;
    }
}
void NetworkTransport::writeOutput() {
    for(int n=0;n<4&&!m_output.isEmpty();++n){const auto count=::write(STDOUT_FILENO,m_output.constData(),size_t(m_output.size()));
        if(count<0){if(errno==EINTR)continue;if(errno==EAGAIN||errno==EWOULDBLOCK)return;m_output.clear();shutdown();QCoreApplication::quit();return;}
        if(count==0)return;m_output.remove(0,count);
    }
    m_writer->setEnabled(!m_output.isEmpty());
    if(m_closing&&m_output.isEmpty()){QCoreApplication::quit();return;}
    if(!m_closing&&m_output.size()<=pipeLimit-replyLimit)m_reader->setEnabled(true);
}
void NetworkTransport::dispatch(const QByteArray &line) {
    QJsonParseError parse;const auto doc=QJsonDocument::fromJson(line,&parse);
    if(parse.error!=QJsonParseError::NoError){reject({},-32700,"parseError","Request is not valid UTF-8 JSON.");return;}
    const auto o=doc.object();const auto id=o["id"];
    if(!doc.isObject()||!StrictJson(line).valid()||!exact(o,{"jsonrpc","id","method","params"})||o["jsonrpc"]!=QJsonValue("2.0")||!id.isDouble()||!o["method"].isString()||!o["params"].isObject()){
        reject({},-32600,"invalidRequest","Expected an exact bounded JSON-RPC request with unique keys and an integer ID.");return;
    }
    const auto method=o["method"].toString();const auto p=o["params"].toObject();
    auto invalid=[&]{reject(id,-32602,"invalidParams","Transport parameters are invalid.");};
    if(p["version"]!=QJsonValue(1)){invalid();return;}
    if(m_closing){reject(id,-32020,"catalogClosed","Metadata transport is closing.");return;}
    if(method=="hello"){
        if(!exact(p,{"version"})){invalid();return;}auto r=base();r.insert("capabilities",QJsonArray{"metadataGetV1"});r.insert("phase","ready");respond(id,r);return;
    }
    if(!token(p["transportSession"])||p["transportSession"]!=QJsonValue(m_session)){reject(id,-32020,"catalogSessionMismatch","Metadata transport session does not match.");return;}
    if(method=="close"){
        if(!exact(p,{"version","transportSession"})){invalid();return;}auto r=base();r.insert("phase","closing");respond(id,r);shutdown(true);return;
    }
    if(method!="get"&&method!="status"&&method!="cancel"&&method!="release"){reject(id,-32601,"methodNotFound","Unknown metadata transport method.");return;}
    if(!(method=="get"?exact(p,{"version","transportSession","requestKey","url","purpose"}):exact(p,{"version","transportSession","requestKey"}))||!token(p["requestKey"])){invalid();return;}
    const auto key=p["requestKey"].toString();auto r=m_requests.value(key);
    if(method=="get"){
        const auto text=p["url"].toString();const auto url=QUrl::fromEncoded(text.toUtf8(),QUrl::StrictMode);
        if(!p["url"].isString()||text.toUtf8().size()>7000||!allowed(url)||p["purpose"]!=QJsonValue("metadata")||text.contains(QChar::Null)||text.contains('\\')||std::any_of(text.cbegin(),text.cend(),[](QChar c){return c.unicode()<=32||c.unicode()==127;})){invalid();return;}
        const auto digest=QString::fromLatin1(QCryptographicHash::hash(text.toUtf8(),QCryptographicHash::Sha256).toHex());
        if(r){if(r->digest!=digest||r->phase=="released"){reject(id,-32020,"catalogKeyConflict","Metadata request key is already bound.");return;}}
        else{
            if(m_requests.size()>=4096){reject(id,-32020,"catalogSessionExhausted","Drain and restart this metadata transport.");return;}
            // Four reserved body slots plus eight URL-only queued requests.
            if(m_storageBlocked){reject(id,-32020,"catalogUnavailable","Preserved spool ownership requires reconciliation.");return;}
            const auto retained=std::count_if(m_requests.cbegin(),m_requests.cend(),[](const auto &entry){return entry->phase!="released";});
            if(m_queue.size()>=8||retained>=12){reject(id,-32020,"catalogBusy","Metadata transport queue is full.");return;}
            r=QSharedPointer<Request>::create();r->key=key;r->url=text;r->digest=digest;m_requests.insert(key,r);m_queue.enqueue(key);pump();
        }
        auto result=base(key);result.insert("phase",r->phase);respond(id,result);return;
    }
    if(!r){reject(id,-32020,"catalogUnknownRequest","Unknown metadata request key.");return;}
    if(method=="status"){respond(id,status(*r));return;}
    const bool pending=r->phase=="queued"||r->phase=="active";
    auto result=base(key);
    if(method=="cancel"){
        result.insert("state",r->phase=="released"?"released":pending?"requested":"alreadyTerminal");
        if(pending){m_queue.removeAll(key);terminate(r,"catalogCancelled","Metadata request cancelled.");}
    }else{
        if(pending){reject(id,-32020,"catalogRequestPending","Metadata request is still pending.");return;}
        removeBody(r);r->phase="released";r->url.clear();r->httpStatus={};r->finalUrl={};r->retryAfter={};r->error={};result.insert("state","released");pump();
    }
    respond(id,result);
}
QJsonObject NetworkTransport::status(const Request &r) const {
    auto o=base(r.key);o.insert("phase",r.phase);
    const bool terminal=r.phase=="succeeded"||r.phase=="failed"||r.phase=="cancelled";
    o.insert("httpStatus",terminal?r.httpStatus:QJsonValue());o.insert("finalUrl",terminal?r.finalUrl:QJsonValue());o.insert("retryAfterSeconds",terminal?r.retryAfter:QJsonValue());
    o.insert("bodyToken",r.phase=="succeeded"?QJsonValue(r.token):QJsonValue());o.insert("bodyBytes",r.phase=="succeeded"?QJsonValue(QString::number(r.bytes)):QJsonValue());o.insert("error",terminal?r.error:QJsonValue());return o;
}
void NetworkTransport::pump(){
    if(m_storageBlocked){
        const auto queued=std::exchange(m_queue,{});
        for(const auto &key:queued)terminate(m_requests.value(key),"transportStorage","Preserved spool ownership blocks admission.");
        return;
    }
    while(!m_closing&&!m_storageBlocked&&m_slots<m_slotLimit&&!m_queue.isEmpty()){
        const auto r=m_requests.value(m_queue.dequeue());if(!r||r->phase!="queued")continue;
        r->slot=true;++m_slots;r->token=newToken();
        struct stat named{},held{};
        if(::lstat(QFile::encodeName(m_directory).constData(),&named)!=0||::fstat(m_directoryHandle.handle(),&held)!=0||!S_ISDIR(named.st_mode)||named.st_dev!=held.st_dev||named.st_ino!=held.st_ino){terminate(r,"transportStorage","Private spool ownership changed.");continue;}
        const auto name=r->token.toLatin1();const int fd=::openat(m_directoryHandle.handle(),name.constData(),O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC,0600);
        if(fd<0){terminate(r,"transportStorage","Cannot create a private metadata body.");continue;}
        struct stat created{};
        if(::fstat(fd,&created)!=0||!regularOwned(created)){::close(fd);m_storageBlocked=true;terminate(r,"transportStorage","Cannot verify private metadata body.");continue;}
        r->device=quint64(created.st_dev);r->inode=quint64(created.st_ino);
        r->file=QSharedPointer<QFile>::create();
        if(!r->file->open(fd,QIODevice::ReadWrite,QFileDevice::AutoCloseHandle)){::close(fd);terminate(r,"transportStorage","Cannot open a private metadata body.");continue;}
        // Publish file identity durably before its first HTTP response can
        // write bytes. A crash before publication leaves only an unknown empty
        // file, which the parent preserves and accounts instead of deleting.
        if(::fsync(r->file->handle())!=0||!writeInventory()){m_storageBlocked=true;terminate(r,"transportStorage","Cannot publish metadata spool ownership.");continue;}
        r->phase="active";r->deadline=new QTimer(this);r->deadline->setSingleShot(true);r->deadline->setInterval(30000);
        connect(r->deadline,&QTimer::timeout,this,[this,r]{terminate(r,"transportTimeout","Metadata request exceeded its deadline.");});r->deadline->start();start(r,QUrl::fromEncoded(r->url.toUtf8(),QUrl::StrictMode));
    }
}
void NetworkTransport::start(const QSharedPointer<Request> &r,const QUrl &url){
    if(m_closing||r->phase!="active")return;
    r->pendingUrl=url;
    if(!m_httpQueue.contains(r->key))m_httpQueue.enqueue(r->key);
    startReady();
}
void NetworkTransport::startReady(){
    m_startTimer.stop();
    while(!m_closing&&!m_httpQueue.isEmpty()){
        const auto now=m_clock.elapsed();
        while(!m_httpStarts.isEmpty()&&now-m_httpStarts.head()>=1000)m_httpStarts.dequeue();
        if(m_httpStarts.size()>=10){m_startTimer.start(int(std::max<qint64>(1,1000-(now-m_httpStarts.head()))));return;}
        const auto r=m_requests.value(m_httpQueue.dequeue());
        if(!r||r->phase!="active"||r->pendingUrl.isEmpty())continue;
        const auto url=std::exchange(r->pendingUrl,{});m_httpStarts.enqueue(now);
        QNetworkRequest request(url);request.setAttribute(QNetworkRequest::RedirectPolicyAttribute,QNetworkRequest::ManualRedirectPolicy);
        request.setAttribute(QNetworkRequest::CookieLoadControlAttribute,QNetworkRequest::Manual);request.setAttribute(QNetworkRequest::CookieSaveControlAttribute,QNetworkRequest::Manual);
        request.setAttribute(QNetworkRequest::AuthenticationReuseAttribute,QNetworkRequest::Manual);request.setAttribute(QNetworkRequest::CacheLoadControlAttribute,QNetworkRequest::AlwaysNetwork);
        request.setRawHeader("Accept","application/json");request.setRawHeader("User-Agent","Modlock/1 metadata");request.setTransferTimeout(10000);
        r->reply=m_network.get(request);r->reply->setReadBufferSize(64*1024);
        r->progressBytes=0;r->progressTimer=new QTimer(this);r->progressTimer->setSingleShot(true);r->progressTimer->setInterval(10000);
        connect(r->progressTimer,&QTimer::timeout,this,[this,r]{terminate(r,"transportTimeout","Metadata request made no progress for 10 seconds.");});
        connect(r->reply,&QNetworkReply::downloadProgress,this,[r](qint64 received,qint64){if(received>r->progressBytes){r->progressBytes=received;if(r->progressTimer)r->progressTimer->start();}});
        r->progressTimer->start();
        connect(r->reply,&QNetworkReply::readyRead,this,[this,r]{consume(r);});
        connect(r->reply,&QNetworkReply::finished,this,[this,r]{finished(r);});
        connect(r->reply,&QNetworkReply::sslErrors,this,[this,r]{terminate(r,"transportTls","TLS validation failed.");});
    }
}
bool NetworkTransport::writeInventory(){
    QJsonArray bodies;
    for(const auto &r:std::as_const(m_requests))if(!r->token.isEmpty()&&r->inode!=0&&r->slot)
        bodies.append(QJsonObject{{"token",r->token},{"device",QString::number(r->device)},{"inode",QString::number(r->inode)}});
    if(bodies.size()>m_slotLimit)return false;
    auto inventory=m_inventory;inventory.insert("bodies",bodies);
    if(m_inventorySequence==std::numeric_limits<quint64>::max())return false;
    const auto sequence=m_inventorySequence+1;
    const auto slot=inventorySlot(QJsonDocument(inventory).toJson(QJsonDocument::Compact),sequence);
    if(slot.isEmpty())return false;
    const int directory=m_directoryHandle.handle();
    const auto file=m_inventoryHandle;
    auto namesOwnedInventory=[directory,&file](){
        struct stat held{},named{};
        return file&&file->isOpen()&&::fstat(file->handle(),&held)==0&&regularOwned(held)&&held.st_size==inventorySize
            &&::fstatat(directory,".inventory",&named,AT_SYMLINK_NOFOLLOW)==0&&regularOwned(named)
            &&held.st_dev==named.st_dev&&held.st_ino==named.st_ino;
    };
    if(!namesOwnedInventory())return false;
    // No active publication creates, renames or unlinks a directory entry.
    // A racing replacement cannot redirect this write away from the held inode.
    const auto offset=qint64((sequence-1)%2)*inventorySlotSize;
    if(!file->seek(offset)||file->write(slot)!=slot.size()||!file->flush()||::fsync(file->handle())!=0)return false;
    // Persist the newly created body's directory entry before HTTP admission.
    if(::fsync(directory)!=0||!namesOwnedInventory())return false;
    m_inventorySequence=sequence;m_inventory=inventory;return true;
}
void NetworkTransport::consume(const QSharedPointer<Request> &r){
    if(r->phase!="active"||!r->reply)return;
    while(r->reply->bytesAvailable()>0){
        const auto bytes=r->reply->read(std::min<qint64>(64*1024,bodyLimit-r->bytes+1));
        if(bytes.isEmpty())break;
        if(r->progressTimer)r->progressTimer->start();
        if(r->bytes+bytes.size()>bodyLimit){terminate(r,"transportBodyLimit","Decoded metadata body exceeds 5 MiB.");return;}
        if(!r->file||r->file->write(bytes)!=bytes.size()){terminate(r,"transportStorage","Cannot write metadata body.");return;}r->bytes+=bytes.size();
    }
}
void NetworkTransport::finished(const QSharedPointer<Request> &r){
    if(r->phase!="active"||!r->reply)return;consume(r);if(r->phase!="active"||!r->reply)return;
    if(r->progressTimer){r->progressTimer->stop();r->progressTimer->deleteLater();r->progressTimer=nullptr;}
    auto *reply=r->reply.data();const auto http=reply->attribute(QNetworkRequest::HttpStatusCodeAttribute).toInt();const auto final=reply->url();
    if(allowed(final))r->finalUrl=QString::fromUtf8(final.toEncoded());if(http>=100&&http<=599)r->httpStatus=http;
    const auto retry=reply->rawHeader("Retry-After");bool numeric=false;const auto seconds=retry.toLongLong(&numeric);
    if(numeric&&seconds>=0&&seconds<=86400)r->retryAfter=seconds;
    else if(!retry.isEmpty()){const auto date=QDateTime::fromString(QString::fromLatin1(retry),Qt::RFC2822Date);if(date.isValid())r->retryAfter=std::clamp<qint64>(QDateTime::currentDateTimeUtc().secsTo(date),0,86400);}
    if(http>=300&&http<=399&&reply->attribute(QNetworkRequest::RedirectionTargetAttribute).isValid()){
        const auto destination=final.resolved(reply->attribute(QNetworkRequest::RedirectionTargetAttribute).toUrl());
        if(!allowed(destination)||++r->redirects>3){terminate(r,"transportRedirect","Metadata redirect is not allowed.");return;}
        reply->disconnect(this);reply->deleteLater();r->reply=nullptr;
        if(!r->file->resize(0)||!r->file->seek(0)){terminate(r,"transportStorage","Cannot reset metadata spool.");return;}
        r->bytes=0;r->httpStatus={};r->finalUrl={};r->retryAfter={};start(r,destination);return;
    }
    // HTTP rejection responses retain their bounded body for Rust classification.
    if(reply->error()==QNetworkReply::TimeoutError||reply->error()==QNetworkReply::OperationCanceledError){terminate(r,"transportTimeout","Metadata request made no progress for 10 seconds.");return;}
    if(!allowed(final)||http<100||http>599||(reply->error()!=QNetworkReply::NoError&&(http<400||reply->error()<200))){terminate(r,"transportNetwork","Metadata network request failed.");return;}
    if(!r->file->flush()){terminate(r,"transportStorage","Cannot finish metadata body.");return;}
    struct stat st{};if(::fstat(r->file->handle(),&st)!=0||!regularOwned(st)||st.st_size!=r->bytes){terminate(r,"transportStorage","Metadata body ownership or size changed.");return;}
    r->file->close();r->file.reset();r->phase="succeeded";
    if(r->deadline){r->deadline->stop();r->deadline->deleteLater();r->deadline=nullptr;}reply->disconnect(this);reply->deleteLater();r->reply=nullptr;
}
void NetworkTransport::removeBody(const QSharedPointer<Request> &r){
    if(r->file){r->file->close();r->file.reset();}
    if(!r->token.isEmpty()){
        struct stat st{};const auto name=r->token.toLatin1();
        if(::fstatat(m_directoryHandle.handle(),name.constData(),&st,AT_SYMLINK_NOFOLLOW)==0){
            if(!regularOwned(st)||quint64(st.st_dev)!=r->device||quint64(st.st_ino)!=r->inode||::unlinkat(m_directoryHandle.handle(),name.constData(),0)!=0)m_storageBlocked=true;
        }else if(errno!=ENOENT)m_storageBlocked=true;
        r->token.clear();
    }
    if(r->slot){r->slot=false;--m_slots;}r->bytes=0;
}
void NetworkTransport::terminate(const QSharedPointer<Request> &r,const char *kind,const char *message){
    if(r->phase!="queued"&&r->phase!="active")return;
    m_httpQueue.removeAll(r->key);r->pendingUrl=QUrl{};
    r->phase=QByteArray(kind)=="catalogCancelled"?"cancelled":"failed";r->error=errorValue(kind,message);
    if(r->deadline){r->deadline->stop();r->deadline->deleteLater();r->deadline=nullptr;}
    if(r->progressTimer){r->progressTimer->stop();r->progressTimer->deleteLater();r->progressTimer=nullptr;}
    if(r->reply){r->reply->disconnect(this);r->reply->abort();r->reply->deleteLater();r->reply=nullptr;}removeBody(r);pump();
}
void NetworkTransport::shutdown(bool replyPending){
    if(m_closing)return;m_closing=true;if(m_reader)m_reader->setEnabled(false);m_queue.clear();m_httpQueue.clear();m_startTimer.stop();
    for(const auto &r:std::as_const(m_requests)){terminate(r,"catalogCancelled","Metadata transport closed.");removeBody(r);}
    if(!replyPending&&m_output.isEmpty())QCoreApplication::quit();else if(m_writer)m_writer->setEnabled(true);
}
