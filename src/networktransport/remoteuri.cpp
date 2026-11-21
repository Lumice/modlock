#include "remoteuri.h"
#include <QJsonDocument>
#include <QJsonParseError>
#include <QJsonValue>
#include <limits>

namespace {
bool alphaNum(char c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9'); }
bool lowerNum(char c) { return (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9'); }
bool label(const QByteArray &s) {
    if (s.isEmpty() || s.size() > 63 || !lowerNum(s.front()) || !lowerNum(s.back())) return false;
    for (char c : s) if (!lowerNum(c) && c != '-') return false;
    return true;
}
bool hostAllowed(const QByteArray &h) {
    if (h == "gamebanana.com" || h == "api.gamebanana.com") return true;
    static const QByteArray suffix = ".gamebanana.com";
    return h.endsWith(suffix) && label(h.left(h.size() - suffix.size()));
}
int hex(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}
bool tailChar(char c) { return alphaNum(c) || QByteArray("-._~!$&'()*+,;=:@/?%").contains(c); }
bool dots(const QByteArray &path) {
    for (const QByteArray &part : path.split('/')) if (part == "." || part == "..") return true;
    return false;
}
QByteArray removeDots(QByteArray in) {
    QByteArray out;
    while (!in.isEmpty()) {
        if (in.startsWith("../")) in.remove(0,3);
        else if (in.startsWith("./")) in.remove(0,2);
        else if (in.startsWith("/./")) in.remove(0,2);
        else if (in == "/.") in = "/";
        else if (in.startsWith("/../") || in == "/..") {
            if (in == "/..") in = "/"; else in.remove(0,3);
            const qsizetype p=out.lastIndexOf('/'); if (p<0) out.clear(); else out.truncate(p);
        } else if (in == "." || in == "..") in.clear();
        else {
            qsizetype end=in.startsWith('/') ? in.indexOf('/',1) : in.indexOf('/');
            if (end<0) end=in.size(); out.append(in.constData(),end); in.remove(0,end);
        }
    }
    return out;
}
// The frozen keys are ASCII and emitted without escapes by the Rust codec.
// Reject escaped keys, duplicate keys, noninteger numbers and excessive nesting
// before Qt's DOM parser can collapse duplicate object members.
class FrameScanner {
    const QByteArray &s; qsizetype p=0; int depth=0;
    void ws() { while (p<s.size() && (s[p]==' '||s[p]=='\t'||s[p]=='\r'||s[p]=='\n')) ++p; }
    bool string(QByteArray *key=nullptr) {
        if (p>=s.size() || s[p++]!='"') return false;
        QByteArray result;
        while (p<s.size()) {
            const unsigned char c=s[p++];
            if (c=='"') { if (key) *key=result; return true; }
            if (c<0x20) return false;
            if (c=='\\') {
                if (key || p>=s.size()) return false;
                const char e=s[p++];
                if (e=='u') { for(int n=0;n<4;++n) { if(p>=s.size() || !((s[p]>='0'&&s[p]<='9')||(s[p]>='a'&&s[p]<='f')||(s[p]>='A'&&s[p]<='F'))) return false; ++p; } }
                else if (!QByteArray("\"\\/bfnrt").contains(e)) return false;
            } else if (key) { if (c>0x7e) return false; result.append(char(c)); }
        }
        return false;
    }
    bool value() {
        ws(); if (++depth>16 || p>=s.size()) return false;
        bool ok=false; const char c=s[p];
        if (c=='{') {
            ++p; ws(); QSet<QByteArray> keys;
            if (p<s.size() && s[p]=='}') {++p;ok=true;}
            else while (p<s.size()) {
                QByteArray key; if (!string(&key)||keys.contains(key)) break; keys.insert(key); ws();
                if (p>=s.size()||s[p++]!=':'||!value()) break; ws();
                if (p<s.size()&&s[p]=='}') {++p;ok=true;break;}
                if (p>=s.size()||s[p++]!=',') break; ws();
            }
        } else if (c=='[') {
            ++p;ws();if (p<s.size()&&s[p]==']') {++p;ok=true;}
            else while (p<s.size()) {if(!value()) break;ws();if(p<s.size()&&s[p]==']'){++p;ok=true;break;}if(p>=s.size()||s[p++]!=',') break;}
        } else if (c=='"') ok=string();
        else if (s.mid(p,4)=="true" || s.mid(p,4)=="null") {p+=4;ok=true;}
        else if (s.mid(p,5)=="false") {p+=5;ok=true;}
        else {
            if(s[p]=='-') ++p;
            if (p<s.size() && s[p]=='0') {++p;ok=true;}
            else if(p<s.size()&&s[p]>='1'&&s[p]<='9') {do{++p;}while(p<s.size()&&s[p]>='0'&&s[p]<='9');ok=true;}
        }
        --depth; return ok;
    }
public:
    explicit FrameScanner(const QByteArray &bytes):s(bytes) {}
    bool scan() {if (!value()) return false;ws();return p==s.size();}
};
}
bool remoteWireUri(const QByteArray &input, RemoteWireUri &u) {
    if (input.size()>4096 || !input.startsWith("https://")) return false;
    for (unsigned char c : input) if (c<=32 || c>=127 || c=='\\' || c=='#') return false;
    const QByteArray rest=input.mid(8); qsizetype split=rest.size();
    const auto slash=rest.indexOf('/'); const auto query=rest.indexOf('?');
    if(slash>=0) split=qMin(split,slash); if(query>=0) split=qMin(split,query);
    const QByteArray authority=rest.left(split); QByteArray h=authority;
    if (authority.contains('@')||authority.contains('%')||authority.contains('[')||authority.contains(']')) return false;
    const auto colon=authority.indexOf(':');
    if(colon>=0) {if(authority.mid(colon+1)!="443"||authority.indexOf(':',colon+1)>=0) return false;h=authority.left(colon);}
    if(!hostAllowed(h)) return false;
    QByteArray tail=rest.mid(split);
    for(qsizetype i=0;i<tail.size();++i) {
        if(!tailChar(tail[i])) return false;
        if(tail[i]=='%') {
            if(i+2>=tail.size()) return false;const int a=hex(tail[i+1]),b=hex(tail[i+2]);if(a<0||b<0)return false;
            const unsigned char c=unsigned(a*16+b);
            if(c<=32||c>=127||alphaNum(char(c))||QByteArray("-._~\\/#?%@").contains(char(c)))return false;i+=2;
        }
    }
    if(tail.isEmpty()||tail.startsWith('?'))tail.prepend('/');
    u={"https://"+authority+tail,h,authority,tail}; return u.effective.size()<=4096;
}
bool remoteResolveLocation(const RemoteWireUri &base, const QByteArray &location, RemoteWireUri &uri) {
    if(location.size()>4096)return false;
    for(unsigned char c:location)if(c<=32||c>=127||c=='\\'||c=='#')return false;
    if(location.startsWith("https://")||location.startsWith("//")) {
        const QByteArray absolute=location.startsWith("//") ? "https:"+location : location;
        const QByteArray rest=absolute.mid(8);qsizetype split=rest.size();auto s=rest.indexOf('/');auto q=rest.indexOf('?');if(s>=0)split=qMin(split,s);if(q>=0)split=qMin(split,q);
        if(dots(rest.mid(split).split('?').first()))return false;return remoteWireUri(absolute,uri);
    }
    if(!location.startsWith('/')&&!location.startsWith('?')&&location.split('/').first().contains(':'))return false;
    const auto q=location.indexOf('?');const QByteArray path=q<0?location:location.left(q);
    const auto bq=base.target.indexOf('?');const QByteArray current=bq<0?base.target:base.target.left(bq);
    QByteArray combined;
    if(path.isEmpty())combined=current;
    else if(path.startsWith('/'))combined=removeDots(path);
    else {const auto slash=current.lastIndexOf('/');if(slash<0)return false;combined=removeDots(current.left(slash+1)+path);}
    QByteArray result="https://"+base.authority+combined;
    if(q>=0)result+=location.mid(q);else if(path.isEmpty()&&bq>=0)result+=base.target.mid(bq);
    return remoteWireUri(result,uri);
}
bool remoteCanonicalRedirectOrigin(const QByteArray &host) {
    return host=="gamebanana.com"||host=="files.gamebanana.com";
}
bool remoteCanonicalCacheHost(const QByteArray &host) {
    static const QByteArray suffix=".gamebanana.com";
    if(!host.endsWith(suffix))return false;
    const QByteArray labelPart=host.left(host.size()-suffix.size());
    static const QByteArray prefix="filecache";
    if(!labelPart.startsWith(prefix))return false;
    const QByteArray digits=labelPart.mid(prefix.size());
    if(digits.isEmpty()||digits.size()>6)return false;
    for(char c:digits)if(c<'0'||c>'9')return false;
    return true;
}
bool remoteStrongEtag(const QByteArray &s) {
    if(s.size()<2||s.size()>1024||s.front()!='"'||s.back()!='"')return false;
    for(qsizetype i=1;i<s.size()-1;++i) {const unsigned char c=s[i];if(c!=0x21&&(c<0x23||c>0x7e))return false;} return true;
}
bool remoteDecimal(const QJsonValue &v, quint64 &n) {
    if(!v.isString())return false;const QByteArray s=v.toString().toLatin1();if(s.isEmpty()||(s.size()>1&&s[0]=='0'))return false;
    quint64 x=0;for(char c:s){if(c<'0'||c>'9'||x>(std::numeric_limits<quint64>::max()-quint64(c-'0'))/10)return false;x=x*10+quint64(c-'0');}n=x;return true;
}
bool remoteHex(const QByteArray &s,int length) {if(s.size()!=length)return false;for(char c:s)if(!((c>='0'&&c<='9')||(c>='a'&&c<='f')))return false;return true;}
bool remoteParseFrame(const QByteArray &frame,QJsonObject &object) {
    if(frame.isEmpty()||frame.size()>16384||!frame.endsWith('\n')||QString::fromUtf8(frame).toUtf8()!=frame||!FrameScanner(frame).scan())return false;
    QJsonParseError e;const auto doc=QJsonDocument::fromJson(frame,&e);if(e.error!=QJsonParseError::NoError||!doc.isObject())return false;object=doc.object();return true;
}
bool remoteExactKeys(const QJsonObject &object,std::initializer_list<const char *> keys) {
    if(object.size()!=qsizetype(keys.size()))return false;for(const char *key:keys)if(!object.contains(QLatin1String(key)))return false;return true;
}
