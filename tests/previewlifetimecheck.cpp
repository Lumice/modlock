#include "previewnetworkfactory.h"
#include <QGuiApplication>
#include <QElapsedTimer>
#include <QTemporaryDir>
#include <QThread>
#include <cstdio>
#include <functional>
#include <memory>

static void check(bool ok, const char *message) {
    if (!ok) { std::fprintf(stderr,"FAIL preview lifetime: %s\n",message); std::exit(1); }
}
int main(int argc, char **argv) {
    QGuiApplication app(argc,argv);
    const bool raceOnly=app.arguments().contains("--race");
    // A stopped service can complete synchronously before requestImageResponse returns.
    auto bridge=std::make_shared<PreviewBridge>();bridge->stopped=true;
    PreviewImageProvider stopped(bridge);
    if(!raceOnly) {
        auto *response=stopped.requestImageResponse("missing",{});
        int finished=0;
        QObject::connect(response,&QQuickImageResponse::finished,&app,[&]{++finished;});
        QCoreApplication::processEvents();
        check(finished==1,"completion was emitted before the caller could connect");
        delete response;
    }
    for(int i=0;i<1000;++i) {
        auto *response=stopped.requestImageResponse("missing",{});
        delete response;
    }
    QCoreApplication::processEvents();
    QTemporaryDir root;check(root.isValid(),"temporary root unavailable");
    PreviewNetworkFactory factory(root.path()+"/previews",root.path()+"/library");
    std::unique_ptr<PreviewImageProvider> provider(factory.imageProvider());
    if(!raceOnly) {
        const auto token=factory.acquire(QUrl(),false);
        auto *response=provider->requestImageResponse(token,{});
        bool finished=false;
        QObject::connect(response,&QQuickImageResponse::finished,&app,[&]{
            check(QThread::currentThread()==response->thread(),"response completed on the service thread");finished=true;
        },Qt::DirectConnection);
        QElapsedTimer wait;wait.start();
        while(!finished&&wait.elapsed()<5000){QCoreApplication::processEvents();QThread::msleep(1);}
        check(finished,"response did not finish");delete response;factory.release(token);
    }
    if(!raceOnly) {
        // QML owner identity stays a PreviewStatus front-door input; each acquire's fresh token owns status.
        // Synthetic deliveries below are emitted on the factory signal; the unsafe-URL publication is the
        // real service path. No artwork bytes, decode or network traffic happen in this fixture.
        const auto tokenOf=[](const PreviewStatus &s){
            const auto text=s.imageSource().toString();const QString prefix="image://remoteartwork/";
            return text.startsWith(prefix)?text.mid(prefix.size()):QString();
        };
        const auto waitFor=[&](const std::function<bool()> &done){
            QElapsedTimer wait;wait.start();
            while(!done()&&wait.elapsed()<5000){QCoreApplication::processEvents();QThread::msleep(1);}
            return done();
        };
        const QUrl inert("http://preview.invalid/owner.png"); // Refused by the service before any request.
        PreviewStatus first,second;
        first.setSource(inert);first.setContextKey("owner-a");
        check(first.state()==PreviewStatus::Hidden&&tokenOf(first).isEmpty(),"inactive owner acquired a token");
        first.setActive(true);
        second.setSource(inert);second.setContextKey("owner-b");second.setActive(true);
        const auto oldToken=tokenOf(first);
        check(first.state()==PreviewStatus::Loading&&!oldToken.isEmpty(),"active owner did not acquire");
        check(!tokenOf(second).isEmpty()&&tokenOf(second)!=oldToken,"same-URL owners share a request token");
        first.setContextKey("owner-c");
        const auto currentToken=tokenOf(first);
        check(first.state()==PreviewStatus::Loading&&first.detail().isEmpty()&&!currentToken.isEmpty()
              &&currentToken!=oldToken,"same-URL owner change did not rotate the request token");
        emit factory.status(oldToken,inert,PreviewStatus::Refreshed,"stale owner");
        QCoreApplication::processEvents();
        check(first.state()==PreviewStatus::Loading&&first.detail().isEmpty(),"released token published after owner change");
        emit factory.status(currentToken,inert,PreviewStatus::Cached,"current owner");
        check(first.state()==PreviewStatus::Cached&&first.detail()=="current owner","current token did not publish");
        check(second.state()==PreviewStatus::Loading,"another owner's token changed this owner");
        first.setActive(false);
        check(first.state()==PreviewStatus::Hidden&&tokenOf(first).isEmpty(),"hidden owner kept its request");
        emit factory.status(currentToken,inert,PreviewStatus::Refreshed,"after hide");
        check(first.state()==PreviewStatus::Hidden,"hidden owner accepted its released token");
        first.setActive(true);
        check(first.state()==PreviewStatus::Loading&&!tokenOf(first).isEmpty()&&tokenOf(first)!=currentToken,
              "revealed owner reused a released token");
        // Real service publication for the current token only; same URL with another owner stays loading.
        std::unique_ptr<QQuickImageResponse> real(provider->requestImageResponse(tokenOf(second),{}));
        check(waitFor([&]{return second.state()==PreviewStatus::Unavailable;}),"service status did not reach its owner");
        QCoreApplication::processEvents();
        check(first.state()==PreviewStatus::Loading,"service status reached a different same-URL owner");
        first.setSource({});
        check(first.state()==PreviewStatus::Missing&&tokenOf(first).isEmpty(),"cleared source kept its request");
        // Front-door bounds: UTF-8 bytes of the owner key and encoded URL bytes, before acquire.
        PreviewStatus bounded;bounded.setActive(true);bounded.setSource(inert);
        bounded.setContextKey(QString(341,QChar(0x754c))+"a");
        check(bounded.contextKey().toUtf8().size()==1024&&bounded.state()==PreviewStatus::Loading&&!tokenOf(bounded).isEmpty(),
              "1024-byte owner key was refused");
        bounded.setContextKey(QString(341,QChar(0x754c))+"ab");
        check(bounded.contextKey().size()<1024&&bounded.state()==PreviewStatus::Unavailable&&tokenOf(bounded).isEmpty()
              &&bounded.detail().contains("control limit"),"1025-byte UTF-8 owner key was not refused before acquire");
        bounded.setContextKey("short");
        check(bounded.state()==PreviewStatus::Loading&&!tokenOf(bounded).isEmpty(),"bounded owner did not recover");
        const QString prefix="http://preview.invalid/";
        bounded.setSource(QUrl(prefix+QString(7000-prefix.size(),'a')));
        check(bounded.state()==PreviewStatus::Loading,"7000-byte URL was refused");
        bounded.setSource(QUrl(prefix+QString(7001-prefix.size(),'a')));
        check(bounded.state()==PreviewStatus::Unavailable&&tokenOf(bounded).isEmpty(),"7001-byte URL was not refused before acquire");
    }
    for(int i=0;i<10000;++i) {
        const auto token=factory.acquire(QUrl(),false);
        if(token.isEmpty()){QCoreApplication::processEvents();continue;}
        auto *response=provider->requestImageResponse(token,{});
        response->cancel();delete response;factory.release(token);
        if(i%16==0)QCoreApplication::processEvents();
    }
    factory.shutdown();QCoreApplication::processEvents();
    std::puts("PASS preview response completion, affinity, cancellation and destruction");
}
