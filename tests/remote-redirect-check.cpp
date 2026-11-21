#include "remoteuri.h"
#include "remoteworker.h"
#include <QElapsedTimer>
#include <QJsonArray>
#include <QJsonObject>
#include <QObject>
#include <QSet>
#include <QSocketNotifier>
#include <QTimer>
#include <QCoreApplication>
#include <memory>
#define private public
#include "remotetransfer.h"
#undef private
#include <cstdlib>
#include <cerrno>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <unistd.h>
#define check(ok) do { if(!(ok)) { fprintf(stderr,"check failed at %d\n",__LINE__); std::abort(); } } while(false)
int main(int argc,char **argv) {
    // The redirect command opens a TLS socket. Deny socket creation before Qt
    // starts threads so this state-transition test cannot contact the provider.
    sock_filter filters[]={
        BPF_STMT(BPF_LD|BPF_W|BPF_ABS,0),
        BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_socket,0,1),
        BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ERRNO|EPERM),
        BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ALLOW),
    };
    sock_fprog policy{static_cast<unsigned short>(sizeof(filters)/sizeof(filters[0])),filters};
    check(prctl(PR_SET_NO_NEW_PRIVS,1,0,0,0)==0);
    check(prctl(PR_SET_SECCOMP,SECCOMP_MODE_FILTER,&policy)==0);
    QCoreApplication app(argc,argv);
    auto makeStart=[](const QString &requestUrl,const QJsonArray &allowed,const QJsonValue &denied) {
        return QJsonObject{
            {"version",1},{"kind","start"},{"epoch",QString(32,'a')},
            {"requestUrl",requestUrl},{"allowedHosts",allowed},{"deniedHosts",denied},
            {"offset","0"},{"ifRange",QJsonValue::Null},{"uriSha256",QJsonValue::Null},{"byteCap","1000"},
            {"claimIdentity",QJsonObject{{"device","1"},{"inode","2"},{"uid",QString::number(geteuid())}}}
        };
    };

    // start() validation for deniedHosts and disjointness (all invalid cases fail closed before worker spawn).
    {
        RemoteTransfer t;
        check(!t.start(makeStart("https://gamebanana.com/dl/1",QJsonArray{"gamebanana.com","files.gamebanana.com"},QJsonValue::Null)));
        check(!t.start(makeStart("https://gamebanana.com/dl/1",QJsonArray{"gamebanana.com","files.gamebanana.com"},QJsonArray{"files.gamebanana.com"})));
        check(!t.start(makeStart("https://filecache25.gamebanana.com/a.rar",QJsonArray{"gamebanana.com","files.gamebanana.com"},QJsonArray{"filecache25.gamebanana.com"})));
        check(!t.start(makeStart("https://gamebanana.com/dl/1",QJsonArray{"gamebanana.com"},QJsonArray{"evil.example"})));
        QJsonArray tooMany;
        for(int i=1;i<=129;++i) tooMany.append(QString("filecache%1.gamebanana.com").arg(i));
        check(!t.start(makeStart("https://gamebanana.com/dl/1",QJsonArray{"gamebanana.com"},tooMany)));
    }

    for(const QByteArray host:{QByteArray("files.gamebanana.com"),QByteArray("evil.example"),QByteArray("files.gamebanana.com.evil.example")}) {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri("https://gamebanana.com/dl/1809951",t.current_));
        t.allowedHosts_={"gamebanana.com","files.gamebanana.com"};
        t.status_=302;t.location_="https://"+host+"/mods/a.zip";
        bool accepted=t.headersComplete();
        check(accepted==(host=="files.gamebanana.com"));
        if(!accepted) check(t.output_.contains("hostNotAllowed")&&!t.output_.contains("transportFraming"));
    }

    // Canonical redirect chain: gamebanana.com -> files.gamebanana.com -> unlisted filecache28, then exact body allow.
    {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri("https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",t.current_));
        t.allowedHosts_={"gamebanana.com","files.gamebanana.com","filecache40.gamebanana.com"};
        t.deniedHosts_={"filecache25.gamebanana.com"};
        t.status_=302;t.location_="https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar";
        check(t.headersComplete());
        check(t.redirect_.effective=="https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar");

        // Drive the production redirect transition; it must capture the exact
        // target itself. Only the subsequent response headers are synthetic.
        check(t.command(QJsonObject{{"version",1},{"kind","allow"},{"epoch",QString(32,'a')},{"data",QJsonObject{
            {"mode","redirect"},{"url",QString::fromLatin1(t.redirect_.effective)},{"rangeStart",QJsonValue::Null},{"ifRange",QJsonValue::Null}
        }}}));
        check(t.redirects_==1);
        check(t.current_.effective=="https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar");
        check(t.canonicalRedirectTarget_==t.current_.effective);
        t.closeSocket(true);
        t.status_=200;
        check(t.headersComplete());

        // Path/query substitution on allow(body) fails closed.
        check(!t.command(QJsonObject{{"version",1},{"kind","allow"},{"epoch",QString(32,'a')},{"data",QJsonObject{
            {"mode","body"},{"url","https://filecache28.gamebanana.com/mods/other.rar"},{"rangeStart",QJsonValue::Null},{"ifRange",QJsonValue::Null}
        }}}));
        check(!t.command(QJsonObject{{"version",1},{"kind","allow"},{"epoch",QString(32,'a')},{"data",QJsonObject{
            {"mode","body"},{"url","https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar?x=1"},{"rangeStart",QJsonValue::Null},{"ifRange",QJsonValue::Null}
        }}}));
        RemoteWireUri exact;
        check(remoteWireUri("https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",exact));
        check(t.targetAllowed(exact));
        check(t.command(QJsonObject{{"version",1},{"kind","allow"},{"epoch",QString(32,'a')},{"data",QJsonObject{
            {"mode","body"},{"url","https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar"},{"rangeStart",QJsonValue::Null},{"ifRange",QJsonValue::Null}
        }}}));
        check(t.phase_==RemoteTransfer::Phase::Body);
    }

    // Denied host (terminated) must fail closed even when matching filecache<N>.gamebanana.com.
    {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri("https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",t.current_));
        t.allowedHosts_={"gamebanana.com","files.gamebanana.com"};
        t.deniedHosts_={"filecache25.gamebanana.com"};
        t.status_=302;t.location_="https://filecache25.gamebanana.com/mods/girlfailure_silver_v2.rar";
        check(!t.headersComplete());
        check(t.output_.contains("hostNotAllowed"));
    }

    // Non-canonical origins (api.gamebanana.com, listed filecache40, or unlisted canonical cache itself) cannot authorize unlisted caches.
    for(const QByteArray origin:{
        QByteArray("https://api.gamebanana.com/dl/1"),
        QByteArray("https://filecache40.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache28.gamebanana.com/mods/a.rar"),
    }) {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri(origin,t.current_));
        t.allowedHosts_={"gamebanana.com","api.gamebanana.com","files.gamebanana.com","filecache40.gamebanana.com"};
        if(t.current_.host=="filecache28.gamebanana.com") t.canonicalRedirectTarget_=t.current_.effective;
        t.status_=302;t.location_="https://filecache29.gamebanana.com/mods/a.rar";
        check(!t.headersComplete());
        check(t.output_.contains("hostNotAllowed"));
    }

    // Same-host relative redirect out of an unlisted canonical cache also fails closed.
    {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri("https://filecache28.gamebanana.com/mods/a.rar",t.current_));
        t.allowedHosts_={"gamebanana.com","files.gamebanana.com"};
        t.canonicalRedirectTarget_=t.current_.effective;
        t.status_=302;t.location_="/mods/other.rar";
        check(!t.headersComplete());
        check(t.output_.contains("hostNotAllowed"));
    }

    // Adversarial hostnames and non-https schemes from canonical origin fail closed.
    for(const QByteArray bad:{
        QByteArray("https://cdn.gamebanana.com/mods/a.rar"),
        QByteArray("https://evil.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache28a.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache-28.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache1234567.gamebanana.com/mods/a.rar"),
        QByteArray("https://sub.filecache28.gamebanana.com/mods/a.rar"),
        QByteArray("https://filecache28.gamebanana.com:8443/mods/a.rar"),
        QByteArray("https://user@filecache28.gamebanana.com/mods/a.rar"),
        QByteArray("https://127.0.0.1/mods/a.rar"),
        QByteArray("http://files.gamebanana.com/a.zip"),
    }) {
        RemoteTransfer t;
        t.epoch_=QByteArray(32,'a');
        check(remoteWireUri("https://files.gamebanana.com/mods/a.rar",t.current_));
        t.allowedHosts_={"gamebanana.com","files.gamebanana.com"};
        t.status_=302;t.location_=bad;
        check(!t.headersComplete());
        check(t.output_.contains("hostNotAllowed"));
    }

    RemoteTransfer t; t.status_=302;t.location_="http://files.gamebanana.com/a.zip";
    check(remoteWireUri("https://gamebanana.com/dl/1",t.current_));
    t.allowedHosts_={"files.gamebanana.com"};check(!t.headersComplete());
}
