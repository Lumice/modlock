#include "remotetransfer.h"
#include <QCoreApplication>
#include <QCryptographicHash>
#include <QJsonDocument>
#include <QNetworkProxy>
#include <QRegularExpression>
#include <QSslConfiguration>
#include <QSslSocket>
#include <algorithm>
#include <cerrno>
#include <fcntl.h>
#include <limits>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>

namespace {
constexpr quint64 MemberLimit=16ULL*1024*1024*1024;
constexpr quint64 CheckpointBytes=8ULL*1024*1024;
constexpr quint64 PipeLimit=32ULL*1024*1024;
constexpr qsizetype FrameLimit=16*1024, QueueLimit=64*1024, Quantum=64*1024;
constexpr qsizetype WorkerQueueLimit=4*1024*1024;
bool redirectStatus(int status) {return status==301||status==302||status==303||status==307||status==308;}
bool token(char c) {return (c>='a'&&c<='z')||(c>='A'&&c<='Z')||(c>='0'&&c<='9')||QByteArray("!#$%&'*+-.^_`|~").contains(c);}
bool asciiValue(const QByteArray &s) {for(unsigned char c:s)if(c!='\t'&&(c<32||c>126))return false;return true;}
QByteArray trimOws(QByteArray s) {while(!s.isEmpty()&&(s.front()==' '||s.front()=='\t'))s.remove(0,1);while(!s.isEmpty()&&(s.back()==' '||s.back()=='\t'))s.chop(1);return s;}
bool parseUnsigned(const QByteArray &s,quint64 &n,int base=10) {
    if(s.isEmpty())return false;quint64 v=0;
    for(char c:s) {int d=-1;if(c>='0'&&c<='9')d=c-'0';else if(base==16&&c>='a'&&c<='f')d=c-'a'+10;else if(base==16&&c>='A'&&c<='F')d=c-'A'+10;
        if(d<0||d>=base||v>(std::numeric_limits<quint64>::max()-quint64(d))/quint64(base))return false;v=v*quint64(base)+quint64(d);}
    n=v;return true;
}
QJsonValue nullableNumber(bool has,quint64 value) {return has?QJsonValue(QString::number(value)):QJsonValue(QJsonValue::Null);}
QJsonValue nullableBytes(const QByteArray &value) {return value.isNull()?QJsonValue(QJsonValue::Null):QJsonValue(QString::fromLatin1(value));}
bool validReason(const QJsonValue &v,int limit) {
    if(!v.isString())return false;const QByteArray s=v.toString().toUtf8();if(s.size()>limit)return false;
    for(unsigned char c:s)if(c<32||c==127)return false;return true;
}
}
RemoteTransfer::RemoteTransfer(QObject *parent):QObject(parent),inputNotifier_(0,QSocketNotifier::Read,this),outputNotifier_(1,QSocketNotifier::Write,this) {
    outputNotifier_.setEnabled(false);
    QObject::connect(&inputNotifier_,&QSocketNotifier::activated,this,[this]{inputReady();});
    QObject::connect(&outputNotifier_,&QSocketNotifier::activated,this,[this]{outputReady();});
    timer_.setInterval(250);QObject::connect(&timer_,&QTimer::timeout,this,[this]{tick();});timer_.start();
    absolute_.start();activity_.start();checkpointClock_.start();progressClock_.start();
}
RemoteTransfer::~RemoteTransfer() {closeSocket(true);if(worker_)worker_->stop();}
void RemoteTransfer::inputReady() {
    char bytes[8192];int budget=64*1024;
    while(budget>0) {
        const ssize_t n=read(0,bytes,std::min<int>(sizeof(bytes),budget));
        if(n==0){inputNotifier_.setEnabled(false);stopQuietly();return;}
        if(n<0){if(errno==EINTR)continue;if(errno==EAGAIN||errno==EWOULDBLOCK)break;inputNotifier_.setEnabled(false);stopQuietly();return;}
        inputTotal_+=quint64(n);budget-=int(n);if(inputTotal_>PipeLimit||input_.size()+n>QueueLimit){fail("transportFraming");return;}
        input_.append(bytes,n);
        while(true) {
            const auto end=input_.indexOf('\n');if(end<0){if(input_.size()>=FrameLimit)fail("transportFraming");break;}
            if(end+1>FrameLimit){fail("transportFraming");return;}
            const QByteArray frame=input_.left(end+1);input_.remove(0,end+1);QJsonObject object;
            if(!remoteParseFrame(frame,object)){fail("transportFraming");return;}receive(object);
            if(phase_==Phase::Terminal)return;
        }
    }
}
void RemoteTransfer::outputReady() {
    while(outputPos_<output_.size()) {
        const ssize_t n=write(1,output_.constData()+outputPos_,size_t(output_.size()-outputPos_));
        if(n<0){if(errno==EINTR)continue;if(errno==EAGAIN||errno==EWOULDBLOCK){outputNotifier_.setEnabled(true);return;}output_.clear();outputPos_=0;outputNotifier_.setEnabled(false);stopQuietly();return;}
        if(n==0){stopQuietly();return;}outputPos_+=n;
    }
    output_.clear();outputPos_=0;outputNotifier_.setEnabled(false);maybeQuit();
}
bool RemoteTransfer::enqueueFrame(const char *kind,const QJsonObject &data,bool advisory) {
    if(epoch_.isEmpty())return false;
    const QJsonObject object{{"version",1},{"kind",QLatin1String(kind)},{"epoch",QString::fromLatin1(epoch_)},{"data",data}};
    QByteArray bytes=QJsonDocument(object).toJson(QJsonDocument::Compact);bytes+='\n';
    if(bytes.size()>FrameLimit)return false;
    if(outputPos_>0){output_.remove(0,outputPos_);outputPos_=0;}
    if(advisory&&(output_.size()>QueueLimit-2*FrameLimit||outputTotal_+quint64(bytes.size())>PipeLimit-FrameLimit))return true;
    if(output_.size()+bytes.size()>QueueLimit||outputTotal_+quint64(bytes.size())>PipeLimit)return false;
    outputTotal_+=quint64(bytes.size());output_+=bytes;outputNotifier_.setEnabled(true);return true;
}
void RemoteTransfer::receive(const QJsonObject &frame) {
    if(phase_==Phase::Start) {if(!start(frame))fail("transportFraming");return;}
    if(!command(frame))fail("transportFraming");
}
bool RemoteTransfer::validateFds(const QJsonObject &id,quint64 offset) {
    Q_UNUSED(offset);
    return remoteExactKeys(id,{"device","inode","uid"})&&remoteDecimal(id.value("device"),device_)
        &&remoteDecimal(id.value("inode"),inode_)&&remoteDecimal(id.value("uid"),uid_)&&uid_==geteuid();
}
bool RemoteTransfer::redirectTargetAllowed(const QByteArray &fromHost,const QByteArray &toHost) const {
    if(deniedHosts_.contains(fromHost)||deniedHosts_.contains(toHost))return false;
    if(!allowedHosts_.contains(fromHost))return false;
    if(allowedHosts_.contains(toHost))return true;
    return remoteCanonicalRedirectOrigin(fromHost)&&remoteCanonicalCacheHost(toHost);
}
bool RemoteTransfer::targetAllowed(const RemoteWireUri &uri) const {
    if(deniedHosts_.contains(uri.host))return false;
    if(allowedHosts_.contains(uri.host))return true;
    return !canonicalRedirectTarget_.isEmpty()&&uri.effective==canonicalRedirectTarget_&&remoteCanonicalCacheHost(uri.host);
}
bool RemoteTransfer::start(const QJsonObject &o) {
    if(!remoteExactKeys(o,{"version","kind","epoch","requestUrl","allowedHosts","deniedHosts","offset","ifRange","uriSha256","byteCap","claimIdentity"})||o.value("version")!=1||o.value("kind")!="start"||!o.value("epoch").isString()||!o.value("requestUrl").isString()||!o.value("allowedHosts").isArray()||!o.value("deniedHosts").isArray()||!o.value("claimIdentity").isObject())return false;
    epoch_=o.value("epoch").toString().toLatin1();if(!remoteHex(epoch_,32))return false;
    if(!remoteDecimal(o.value("offset"),initialOffset_)||!remoteDecimal(o.value("byteCap"),byteCap_)||!byteCap_||byteCap_>MemberLimit||initialOffset_>=byteCap_)return false;
    allowedHosts_.clear();deniedHosts_.clear();canonicalRedirectTarget_.clear();
    const auto hosts=o.value("allowedHosts").toArray();if(hosts.isEmpty()||hosts.size()>64)return false;
    for(const auto &value:hosts) {if(!value.isString())return false;const QByteArray host=value.toString().toLatin1();RemoteWireUri test;if(!remoteWireUri("https://"+host,test)||test.host!=host||allowedHosts_.contains(host))return false;allowedHosts_.insert(host);}
    const auto denied=o.value("deniedHosts").toArray();if(denied.size()>128)return false;
    for(const auto &value:denied) {if(!value.isString())return false;const QByteArray host=value.toString().toLatin1();RemoteWireUri test;if(!remoteWireUri("https://"+host,test)||test.host!=host||allowedHosts_.contains(host)||deniedHosts_.contains(host))return false;deniedHosts_.insert(host);}
    if(!remoteWireUri(o.value("requestUrl").toString().toUtf8(),current_)||deniedHosts_.contains(current_.host)||!allowedHosts_.contains(current_.host))return false;
    if(initialOffset_==0) {if(!o.value("ifRange").isNull()||!o.value("uriSha256").isNull())return false;}
    else {
        if(!o.value("ifRange").isString()||!o.value("uriSha256").isString())return false;
        ifRange_=o.value("ifRange").toString().toLatin1();uriDigest_=o.value("uriSha256").toString().toLatin1();
        if(!remoteStrongEtag(ifRange_)||!remoteHex(uriDigest_,64))return false;
    }
    if(!validateFds(o.value("claimIdentity").toObject(),initialOffset_))return false;
    admitted_=checkpointOffset_=acknowledgedOffset_=initialOffset_;
    phase_=Phase::Prefix;
    try {worker_=std::make_unique<RemoteTransferWorker>(this,[this]{workerWake();},initialOffset_,device_,inode_,uid_);} catch(...) {return false;}
    return true;
}
bool RemoteTransfer::command(const QJsonObject &o) {
    if(!remoteExactKeys(o,{"version","kind","epoch","data"})||o.value("version")!=1||!o.value("kind").isString()||o.value("epoch").toString().toLatin1()!=epoch_||!o.value("data").isObject())return false;
    const QString kind=o.value("kind").toString();const QJsonObject d=o.value("data").toObject();
    if(kind=="abort") {
        if(!remoteExactKeys(d,{"reason"})||!d.value("reason").isString())return false;
        const QString reason=d.value("reason").toString();if(reason!="pause"&&reason!="cancel"&&reason!="close"&&reason!="failure")return false;stopQuietly();return true;
    }
    if(kind=="deny") {if(phase_!=Phase::Decision||!remoteExactKeys(d,{"reason"})||!validReason(d.value("reason"),512))return false;stopQuietly();return true;}
    if(kind=="checkpointAck") {
        if(phase_!=Phase::Checkpoint||!remoteExactKeys(d,{"offset","prefixSha256"}))return false;quint64 at=0;
        if(!remoteDecimal(d.value("offset"),at)||at!=checkpointOffset_||at!=admitted_||!remoteHex(checkpointHash_,64)
           ||!d.value("prefixSha256").isString()||d.value("prefixSha256").toString().toLatin1()!=checkpointHash_)return false;
        acknowledgedOffset_=at;worker_->acknowledge();checkpointClock_.restart();activity_.restart();
        if(framed_){phase_=Phase::Finishing;worker_->finish();}else{phase_=Phase::Body;schedulePump();}return true;
    }
    if(kind!="allow"||phase_!=Phase::Decision||!remoteExactKeys(d,{"mode","url","rangeStart","ifRange"})||!d.value("mode").isString()||!d.value("url").isString())return false;
    const QByteArray approvedText=d.value("url").toString().toUtf8();
    RemoteWireUri approved;if(!remoteWireUri(approvedText,approved)||approved.effective!=approvedText)return false;
    const QString mode=d.value("mode").toString();bool nextRange=false;
    if(mode=="redirect") {
        if(ranged_||!redirectStatus(status_))return false;
        if(approved.effective!=redirect_.effective||redirects_>=3||!redirectTargetAllowed(current_.host,approved.host))return false;
        nextRange=initialOffset_>0&&QCryptographicHash::hash(approved.effective,QCryptographicHash::Sha256).toHex()==uriDigest_;
    } else if(mode=="body") {
        if(approved.effective!=current_.effective||!targetAllowed(approved)||(ranged_&&status_!=206)||(!ranged_&&status_!=200)||(!ranged_&&initialOffset_!=0))return false;
        nextRange=ranged_;
    } else return false;
    if(nextRange) {quint64 range=0;if(!remoteDecimal(d.value("rangeStart"),range)||range!=initialOffset_||!d.value("ifRange").isString()||d.value("ifRange").toString().toLatin1()!=ifRange_)return false;}
    else if(!d.value("rangeStart").isNull()||!d.value("ifRange").isNull())return false;
    activity_.restart();
    if(mode=="redirect") {++redirects_;closeSocket(true);network_.clear();canonicalRedirectTarget_=allowedHosts_.contains(approved.host)?QByteArray():approved.effective;startRequest(approved,nextRange);}
    else {phase_=Phase::Body;checkpointClock_.restart();schedulePump();}
    return true;
}
void RemoteTransfer::startRequest(const RemoteWireUri &uri,bool ranged) {
    current_=uri;ranged_=ranged;network_.clear();headers_=QJsonArray();critical_.clear();location_.clear();etag_=QByteArray();
    controlBytes_=fieldCount_=status_=0;chunkOverhead_=bodyBytes_=0;contentLength_=rangeEnd_=total_=chunkRemaining_=0;
    hasLength_=hasTotal_=hasTransfer_=framed_=shortRange_=sawStatus_=sawCloseNotify_=networkEof_=false;chunk_=Chunk::Size;
    if(allowedHosts_.contains(uri.host))canonicalRedirectTarget_.clear();
    if(!targetAllowed(uri)||(initialOffset_>0&&ranged!=(QCryptographicHash::hash(uri.effective,QCryptographicHash::Sha256).toHex()==uriDigest_))){fail("hostNotAllowed");return;}
    auto *socket=new QSslSocket(this);socket_=socket;phase_=Phase::Connecting;
    socket->setProxy(QNetworkProxy(QNetworkProxy::NoProxy));socket->setPeerVerifyMode(QSslSocket::VerifyPeer);socket->setPeerVerifyName(QString::fromLatin1(uri.host));socket->setReadBufferSize(Quantum);
    QSslConfiguration cfg=socket->sslConfiguration();cfg.setAllowedNextProtocols({QByteArray("http/1.1")});socket->setSslConfiguration(cfg);
    QObject::connect(socket,&QSslSocket::encrypted,this,[this,socket] {
        if(socket_!=socket||phase_!=Phase::Connecting)return;
        const QSslConfiguration cfg=socket->sslConfiguration();const QByteArray selected=cfg.nextNegotiatedProtocol();
        if((cfg.nextProtocolNegotiationStatus()==QSslConfiguration::NextProtocolNegotiationNegotiated&&selected!="http/1.1")||
           (cfg.nextProtocolNegotiationStatus()!=QSslConfiguration::NextProtocolNegotiationNegotiated&&!selected.isNull())){fail("transportTls");return;}
        QByteArray request="GET "+current_.target+" HTTP/1.1\r\nHost: "+current_.authority+"\r\nAccept-Encoding: identity\r\nConnection: close\r\n";
        if(ranged_)request+="Range: bytes="+QByteArray::number(initialOffset_)+"-\r\nIf-Range: "+ifRange_+"\r\n";
        request+="\r\n";if(request.size()>8192||socket->write(request)!=request.size()){fail("transportNetwork");return;}
        phase_=Phase::Headers;activity_.restart();schedulePump();
    });
    QObject::connect(socket,&QSslSocket::readyRead,this,[this,socket]{if(socket_==socket)schedulePump();});
    QObject::connect(socket,&QSslSocket::sslErrors,this,[this,socket](const QList<QSslError>&){if(socket_==socket)fail("transportTls");});
    QObject::connect(socket,&QSslSocket::alertReceived,this,[this,socket](QSsl::AlertLevel level,QSsl::AlertType type,const QString&){
        if(socket_!=socket)return;if(type==QSsl::AlertType::CloseNotify)sawCloseNotify_=true;else if(level==QSsl::AlertLevel::Fatal)fail("transportTls");
    });
    QObject::connect(socket,&QAbstractSocket::errorOccurred,this,[this,socket](QAbstractSocket::SocketError error){
        if(socket_!=socket)return;if(error==QAbstractSocket::RemoteHostClosedError){networkEof_=true;schedulePump();}else fail("transportNetwork");
    });
    QObject::connect(socket,&QAbstractSocket::disconnected,this,[this,socket]{if(socket_==socket){networkEof_=true;schedulePump();}});
    socket->connectToHostEncrypted(QString::fromLatin1(uri.host),443,QString::fromLatin1(uri.host));
}
void RemoteTransfer::schedulePump() {
    if(pumpScheduled_||phase_==Phase::Terminal)return;pumpScheduled_=true;
    QMetaObject::invokeMethod(this,[this]{pumpScheduled_=false;networkPump();},Qt::QueuedConnection);
}
bool RemoteTransfer::readMore() {
    if(!socket_||socket_->bytesAvailable()<=0||network_.size()>=Quantum)return false;
    qsizetype amount=qMin<qsizetype>(Quantum-network_.size(),socket_->bytesAvailable());
    if(phase_==Phase::Headers || (phase_==Phase::Body&&framing_==Framing::Chunked&&chunk_==Chunk::Trailers)) {
        const qsizetype remain=8192-controlBytes_;
        if(network_.size()>remain){fail("transportFraming");return false;}
        amount=qMin(amount,remain-network_.size());
    } else if(phase_==Phase::Body&&framing_==Framing::Chunked&&chunk_==Chunk::Size) {
        if(network_.size()>1024){fail("transportFraming");return false;}
        amount=qMin(amount,qsizetype(1025)-network_.size());
    }
    if(amount<=0)return false;const QByteArray bytes=socket_->read(amount);if(bytes.isEmpty())return false;
    network_+=bytes;activity_.restart();return true;
}
int RemoteTransfer::line(QByteArray &result,bool control) {
    const qsizetype end=network_.indexOf('\n');
    if(end<0) {
        for(qsizetype i=0;i<network_.size();++i)if(network_[i]=='\r'&&i+1<network_.size())return -1;
        if(control ? network_.size()>=8192-controlBytes_ : network_.size()>1024)return -1;return 0;
    }
    if(end==0||network_[end-1]!='\r')return -1;
    for(qsizetype i=0;i<end-1;++i)if(network_[i]=='\r')return -1;
    const qsizetype count=end+1;
    if(control){if(controlBytes_+count>8192)return -1;controlBytes_+=int(count);}
    else {if(count>1026||chunkOverhead_+quint64(count)>64ULL*1024*1024)return -1;chunkOverhead_+=quint64(count);}
    result=network_.left(end-1);network_.remove(0,count);return 1;
}
bool RemoteTransfer::statusLine(const QByteArray &s) {
    if(s.size()<13||!s.startsWith("HTTP/1.1 ")||!asciiValue(s)||s[9]<'0'||s[9]>'9'||s[10]<'0'||s[10]>'9'||s[11]<'0'||s[11]>'9'||s[12]!=' ')return false;
    status_=(s[9]-'0')*100+(s[10]-'0')*10+(s[11]-'0');
    return status_>=200&&status_<=599;
}
bool RemoteTransfer::fieldLine(const QByteArray &s,bool trailer) {
    if(s.isEmpty()||s.startsWith(' ')||s.startsWith('\t')||++fieldCount_>64)return false;
    const qsizetype colon=s.indexOf(':');if(colon<=0)return false;
    const QByteArray name=s.left(colon),value=s.mid(colon+1),lower=name.toLower(),trimmed=trimOws(value);
    for(char c:name)if(!token(c))return false;if(!asciiValue(value))return false;
    const bool critical=lower=="content-length"||lower=="transfer-encoding"||lower=="content-encoding"||lower=="content-range"||lower=="etag"||lower=="location"||lower=="content-type";
    if(critical){if(trailer||critical_.contains(lower))return false;critical_.insert(lower);}
    if(trailer)return true;
    headers_.append(QJsonObject{{"name",QString::fromLatin1(name)},{"value",QString::fromLatin1(value)}});
    if(lower=="content-length") {if(!parseUnsigned(trimmed,contentLength_))return false;hasLength_=true;}
    else if(lower=="transfer-encoding") {if(trimmed.toLower()!="chunked")return false;hasTransfer_=true;}
    else if(lower=="content-encoding") {if(trimmed.toLower()!="identity")return false;}
    else if(lower=="etag") {
        if(remoteStrongEtag(trimmed))etag_=trimmed;
        else if(!(trimmed.startsWith("W/")&&remoteStrongEtag(trimmed.mid(2))))return false;
    } else if(lower=="location")location_=trimmed;
    else if(lower=="content-type"&&trimmed.toLower().startsWith("multipart/"))return false;
    else if(lower=="content-range") {
        if(!trimmed.startsWith("bytes "))return false;const QByteArray range=trimmed.mid(6);
        const auto dash=range.indexOf('-'),slash=range.indexOf('/');quint64 start=0,end=0,total=0;
        if(dash<=0||slash<=dash+1||range.indexOf('/',slash+1)>=0||!parseUnsigned(range.left(dash),start)||!parseUnsigned(range.mid(dash+1,slash-dash-1),end)||!parseUnsigned(range.mid(slash+1),total)||start!=initialOffset_||end<start||end>=total||total>byteCap_)return false;
        rangeEnd_=end;total_=total;hasTotal_=true;
    }
    return true;
}
bool RemoteTransfer::headersComplete() {
    if(hasLength_&&hasTransfer_)return false;
    if(redirectStatus(status_)) {
        if(ranged_||location_.isEmpty())return false;
        if(!remoteResolveLocation(current_,location_,redirect_)||!redirectTargetAllowed(current_.host,redirect_.host)){
            fprintf(stderr,"Remote redirect authority rejected host: %s\n",redirect_.host.constData());
            fail("hostNotAllowed");return false;
        }
    } else if(status_==200) {
        if(ranged_||initialOffset_!=0||critical_.contains("content-range"))return false;
        if(hasLength_){if(contentLength_>byteCap_)return false;total_=contentLength_;hasTotal_=true;}
    } else if(status_==206) {
        if(!ranged_||!hasTotal_||etag_!=ifRange_||!critical_.contains("content-range"))return false;
        const quint64 length=rangeEnd_-initialOffset_+1;if(hasLength_&&contentLength_!=length)return false;
        shortRange_=rangeEnd_+1<total_;
    } else return false;
    framing_=hasTransfer_?Framing::Chunked:(hasLength_?Framing::Length:Framing::Close);
    const QJsonObject data{{"url",QString::fromLatin1(current_.effective)},{"status",QString::number(status_)},{"headers",headers_},{"identityEstablished",true}};
    if(!enqueueFrame("headers",data))return false;phase_=Phase::Decision;return true;
}
bool RemoteTransfer::chunkSize(const QByteArray &line,quint64 &size) {
    qsizetype i=0;while(i<line.size()&&((line[i]>='0'&&line[i]<='9')||(line[i]>='a'&&line[i]<='f')||(line[i]>='A'&&line[i]<='F')))++i;
    if(i==0||!parseUnsigned(line.left(i),size,16))return false;
    while(i<line.size()) {
        while(i<line.size()&&(line[i]==' '||line[i]=='\t'))++i;
        if(i>=line.size()||line[i++]!=';')return false;while(i<line.size()&&(line[i]==' '||line[i]=='\t'))++i;
        const qsizetype begin=i;while(i<line.size()&&token(line[i]))++i;if(i==begin)return false;
        while(i<line.size()&&(line[i]==' '||line[i]=='\t'))++i;
        if(i<line.size()&&line[i]=='=') {
            ++i;while(i<line.size()&&(line[i]==' '||line[i]=='\t'))++i;
            if(i<line.size()&&line[i]=='"') {
                ++i;bool closed=false;while(i<line.size()) {const unsigned char c=line[i++];if(c=='"'){closed=true;break;}if(c=='\\'){if(i>=line.size())return false;const unsigned char escaped=line[i++];if(escaped!='\t'&&(escaped<32||escaped>126))return false;}else if(c!='\t'&&(c<32||c>126))return false;}if(!closed)return false;
            } else {const auto beginValue=i;while(i<line.size()&&token(line[i]))++i;if(i==beginValue)return false;}
        }
    }
    return true;
}
bool RemoteTransfer::remainingPayload(quint64 &remaining) const {
    if(admitted_>byteCap_)return false;
    remaining=byteCap_-admitted_;
    if(status_!=206)return true;
    // A 206 is bounded by the approved segment, even when its HTTP body has
    // no Content-Length. The preceding comparisons make end + 1 safe.
    if(!hasTotal_||total_>byteCap_||rangeEnd_<initialOffset_||rangeEnd_>=total_
       ||admitted_<initialOffset_||rangeEnd_==std::numeric_limits<quint64>::max())return false;
    const quint64 end=rangeEnd_+1;
    if(admitted_>end)return false;
    remaining=std::min(remaining,end-admitted_);
    return true;
}
void RemoteTransfer::networkPump() {
    if(!socket_||!(phase_==Phase::Headers||phase_==Phase::Body))return;
    qsizetype processed=0;int steps=0;
    while(processed<Quantum && steps++<512 && (phase_==Phase::Headers||phase_==Phase::Body)) {
        const qsizetype before=network_.size();bool advanced=false;
        if(phase_==Phase::Headers) {
            QByteArray l;const int state=line(l,true);if(state<0){fail("transportFraming");return;}
            if(state>0){advanced=true;if(!sawStatus_){if(!statusLine(l)){fail("transportFraming");return;}sawStatus_=true;}
                else if(l.isEmpty()){if(!headersComplete()){fail("transportFraming");return;}}
                else if(!fieldLine(l,false)){fail("transportFraming");return;}}
        } else if(framing_==Framing::Chunked&&(chunk_==Chunk::Size||chunk_==Chunk::Trailers)) {
            QByteArray l;const bool trailers=chunk_==Chunk::Trailers;const int state=line(l,trailers);if(state<0){fail("transportFraming");return;}
            if(state>0){advanced=true;if(trailers){if(l.isEmpty()){chunk_=Chunk::Complete;bodyComplete();}else if(!fieldLine(l,true)){fail("transportFraming");return;}}
                else {quint64 length=0,remaining=0;if(!chunkSize(l,length)||!remainingPayload(remaining)||length>remaining){fail("transportFraming");return;}chunkRemaining_=length;chunk_=length?Chunk::Data:Chunk::Trailers;}}
        } else if(framing_==Framing::Chunked&&chunk_==Chunk::DataCrlf) {
            if(network_.size()>=2){if(!network_.startsWith("\r\n")||chunkOverhead_+2>64ULL*1024*1024){fail("transportFraming");return;}network_.remove(0,2);chunkOverhead_+=2;chunk_=Chunk::Size;advanced=true;}
        } else {
            if(framing_==Framing::Length&&bodyBytes_==contentLength_){bodyComplete();advanced=true;}
            else if(!network_.isEmpty()) {
                const auto state=worker_->state();
                quint64 remaining=0;
                if(!remainingPayload(remaining)||admitted_<checkpointOffset_||admitted_-checkpointOffset_>CheckpointBytes
                   ||state.charged<0||state.charged>WorkerQueueLimit){fail("transportFraming");return;}
                // Close-delimited input has no non-payload framing to defer:
                // reject an already buffered excess before handing any of it off.
                if(status_==206&&framing_==Framing::Close&&quint64(network_.size())>remaining){fail("transportFraming");return;}
                const quint64 remainingCheckpoint=CheckpointBytes-(admitted_-checkpointOffset_);
                quint64 amount=std::min<quint64>(quint64(network_.size()),std::min(remaining,remainingCheckpoint));
                amount=std::min<quint64>(amount,quint64(WorkerQueueLimit-state.charged));
                if(framing_==Framing::Length)amount=std::min(amount,contentLength_-bodyBytes_);
                if(framing_==Framing::Chunked)amount=std::min(amount,chunkRemaining_);
                if(amount==0) {
                    if(remaining==0){fail("transportFraming");return;}
                    if(remainingCheckpoint==0)beginCheckpoint();
                    return;
                }
                QByteArray data=network_.left(qsizetype(amount));if(!worker_->enqueue(std::move(data)))return;
                network_.remove(0,qsizetype(amount));admitted_+=amount;bodyBytes_+=amount;
                if(framing_==Framing::Chunked){chunkRemaining_-=amount;if(chunkRemaining_==0)chunk_=Chunk::DataCrlf;}
                advanced=true;
                if(framing_==Framing::Length&&bodyBytes_==contentLength_)bodyComplete();
                else if(admitted_-checkpointOffset_==CheckpointBytes)beginCheckpoint();
            }
        }
        processed+=qMax<qsizetype>(0,before-network_.size());
        if(!(phase_==Phase::Headers||phase_==Phase::Body))return;
        if(advanced)continue;
        if(readMore())continue;
        if(networkEof_) {
            if(phase_==Phase::Body&&framing_==Framing::Close&&network_.isEmpty()&&sawCloseNotify_)bodyComplete();
            else if(network_.isEmpty()||!socket_||socket_->bytesAvailable()==0)fail("transportFraming");
        }
        return;
    }
    if(phase_==Phase::Headers||phase_==Phase::Body)schedulePump();
}
void RemoteTransfer::bodyComplete() {
    if(framed_)return;
    if(!bodyBytes_||!network_.isEmpty()||(status_==206&&bodyBytes_!=rangeEnd_-initialOffset_+1)||(framing_==Framing::Close&&!sawCloseNotify_)){fail("transportFraming");return;}
    if(admitted_<checkpointOffset_||acknowledgedOffset_>admitted_){fail("transportFraming");return;}
    framed_=true;closeSocket(false);
    if(shortRange_&&acknowledgedOffset_!=admitted_) {
        // checkpointOffset_ may describe an emitted event; only a matching ACK
        // advances acknowledgedOffset_ and permits direct finalization.
        if(admitted_==checkpointOffset_){fail("transportFraming");return;}
        beginCheckpoint();
    } else if(admitted_-checkpointOffset_>=CheckpointBytes)beginCheckpoint();
    else {phase_=Phase::Finishing;worker_->finish();}
}
void RemoteTransfer::beginCheckpoint() {
    if(phase_!=Phase::Body||admitted_==checkpointOffset_)return;
    phase_=Phase::Checkpoint;checkpointHash_.clear();worker_->checkpoint();
}
void RemoteTransfer::workerWake() {
    if(!worker_)return;const auto state=worker_->state();
    if(state.activity!=lastActivityCount_){lastActivityCount_=state.activity;activity_.restart();}
    if(phase_==Phase::Terminal){if(state.done)maybeQuit();return;}
    if(state.failed){fail(state.identityFailed?"claimAmbiguous":"transportStorage");return;}
    if(phase_==Phase::Prefix&&state.ready) {
        const bool range=initialOffset_>0&&QCryptographicHash::hash(current_.effective,QCryptographicHash::Sha256).toHex()==uriDigest_;
        startRequest(current_,range);return;
    }
    if(phase_==Phase::Checkpoint&&state.checkpointReady&&checkpointHash_.isEmpty()) {
        if(state.written!=admitted_){fail("transportStorage");return;}
        checkpointOffset_=state.written;checkpointHash_=state.checkpointHash;
        const QByteArray digest=QCryptographicHash::hash(current_.effective,QCryptographicHash::Sha256).toHex();
        QJsonObject d{{"offset",QString::number(checkpointOffset_)},{"prefixSha256",QString::fromLatin1(checkpointHash_)},
                      {"strongEtag",nullableBytes(etag_)},{"effectiveUriSha256",QString::fromLatin1(digest)},{"total",nullableNumber(hasTotal_,total_)}};
        if(!enqueueFrame("checkpoint",d)){fail("transportFraming");return;}
    }
    if(phase_==Phase::Body&&state.written!=lastProgress_&&progressClock_.elapsed()>=1000) {
        const QJsonObject d{{"bytesWritten",QString::number(state.written)},{"total",nullableNumber(hasTotal_,total_)}};
        if(!enqueueFrame("progress",d,true)){fail("transportFraming");return;}lastProgress_=state.written;progressClock_.restart();
    }
    if(phase_==Phase::Body)schedulePump();
    if(phase_==Phase::Finishing&&state.done) {
        if(!framed_||state.written!=admitted_||!remoteHex(state.md5,32)||!remoteHex(state.sha256,64)){fail("transportStorage");return;}
        const QJsonObject d{{"bytesWritten",QString::number(state.written)},{"md5",QString::fromLatin1(state.md5)},{"sha256",QString::fromLatin1(state.sha256)},{"networkError","NoError"}};
        if(!enqueueFrame("finished",d)){fail("transportFraming");return;}terminalSent_=true;phase_=Phase::Terminal;inputNotifier_.setEnabled(false);maybeQuit();
    }
}
void RemoteTransfer::tick() {
    if(phase_==Phase::Terminal){maybeQuit();return;}
    if(absolute_.elapsed()>=6LL*60*60*1000||activity_.elapsed()>=30000){fail("transportTimeout");return;}
    if(phase_==Phase::Body&&admitted_>checkpointOffset_&&checkpointClock_.elapsed()>=5000)beginCheckpoint();
}
void RemoteTransfer::closeSocket(bool abort) {
    auto *socket=socket_;socket_=nullptr;if(!socket)return;socket->disconnect(this);
    if(abort){socket->abort();socket->deleteLater();}
    else {QObject::connect(socket,&QAbstractSocket::disconnected,socket,&QObject::deleteLater);socket->disconnectFromHost();if(socket->state()==QAbstractSocket::UnconnectedState)socket->deleteLater();}
}
void RemoteTransfer::fail(const char *kind) {
    if(phase_==Phase::Terminal)return;
    if(!epoch_.isEmpty()) {terminalSent_=enqueueFrame("error",QJsonObject{{"kind",QLatin1String(kind)}});}
    phase_=Phase::Terminal;inputNotifier_.setEnabled(false);closeSocket(true);if(worker_)worker_->stop();maybeQuit();
}
void RemoteTransfer::stopQuietly() {
    if(phase_==Phase::Terminal)return;quiet_=true;phase_=Phase::Terminal;inputNotifier_.setEnabled(false);closeSocket(true);if(worker_)worker_->stop();maybeQuit();
}
void RemoteTransfer::maybeQuit() {
    if(phase_!=Phase::Terminal||(worker_&&!worker_->state().done)||outputPos_<output_.size())return;
    QCoreApplication::quit();
}
