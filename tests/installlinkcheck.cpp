#include "installlink.h"
#include "instancebroker.h"
#include <QCoreApplication>
#include <QProcess>
#include <QTemporaryDir>
#include <QTimer>
#include <QLocalSocket>
#include <QFileInfo>
#include <QStandardPaths>
#include <QCryptographicHash>
#include <cstdio>
int main(int argc,char **argv) {
    QCoreApplication app(argc,argv);
    const QString good="dlmm:https://gamebanana.com/mmdl/12,Mod,42";
    if(app.arguments().size()>1) {
        app.setApplicationName("Modlock");
        InstanceBroker child;return child.start(app.arguments()[1],{good})==InstanceBroker::Forwarded?0:1;
    }
    QVariantMap submission;QString file;
    for(const auto &scheme:QStringList{"dlmm","deadlock-mod-manager","deadlock-modmanager"})
        for(const auto &prefix:QStringList{"", "//"})
            for(const auto &type:QStringList{"Mod","Sound"}) {
                if(!parseInstallLink(scheme+":"+prefix+"https://files.gamebanana.com/mmdl/12,"+type+",42",submission,file)
                    || file!="12" || submission["submissionId"]!="42" || submission["submissionType"]!=type.toLower())return 2;
            }
    for(const auto &bad:QStringList{
        "dlmm:http://gamebanana.com/mmdl/12,Mod,42", "dlmm:https://evil.test/mmdl/12,Mod,42",
        "dlmm:https://gamebanana.com.evil.test/mmdl/12,Mod,42", "dlmm:https://user@gamebanana.com/mmdl/12,Mod,42",
        "dlmm:https://gamebanana.com:444/mmdl/12,Mod,42", "dlmm:https://gamebanana.com/mmdl/12,Map,42",
        "dlmm:https://gamebanana.com/mods/12,Mod,42", "dlmm:https://gamebanana.com/mmdl/12,Mod,4294967296",
        "dlmm:https://gamebanana.com/mmdl/12,Mod,-1", "dlmm:https://gamebanana.com/mmdl/12,Mod,42,extra",
        "dlmm:https://gamebanana.com/mmdl/12#x,Mod,42", "dlmm:https://gamebanana.com/mmdl/%31,Mod,42",
        "dlmm://install?source=divineskins&mod=1&release=2", "dlmm:registerKey,42,secret",
        "other:https://gamebanana.com/mmdl/12,Mod,42", "dlmm:https://gamebanana.com/mmdl/12,Mod,42\n"})
        if(parseInstallLink(bad,submission,file))return 3;
    if(parseInstallLink(QString(9000,'a'),submission,file))return 4;
    app.setApplicationName("Deadlock Native");
    QTemporaryDir root;InstanceBroker server;
    if(server.start(root.path(),{})!=InstanceBroker::Primary)return 5;
    const auto socket=QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation)+"/deadlock-native/"+QString::fromLatin1(QCryptographicHash::hash(root.path().toUtf8(),QCryptographicHash::Sha256).toHex().left(24));
    if(!QFileInfo::exists(socket))return 10;
    if(QFileInfo(socket).permissions() & (QFile::ReadGroup|QFile::WriteGroup|QFile::ReadOther|QFile::WriteOther))return 6;
    int delivered=0;QObject::connect(&server,&InstanceBroker::received,&app,[&](const QStringList &links){if(links==QStringList{good})++delivered;else app.exit(7);});
    QProcess child;
    QObject::connect(&child,&QProcess::finished,&app,[&](int code,QProcess::ExitStatus status){app.exit(code==0 && status==QProcess::NormalExit && delivered==1?0:8);});
    child.start(app.applicationFilePath(),{root.path()});
    QTimer::singleShot(6000,&app,[&]{child.kill();app.exit(9);});
    const auto result=app.exec();if(result==0)std::puts("PASS install link parsing, private socket, two-process forwarding");return result;
}
