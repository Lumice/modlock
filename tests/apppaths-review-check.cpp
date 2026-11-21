#include "apppaths.h"
#include <QCoreApplication>
#include <QSettings>
#include <cstdio>
int main(int argc,char **argv) {
    QCoreApplication app(argc,argv);
    const QString mode=QString::fromUtf8(argv[1]);
    if(mode=="preferences") {
        QFile old(QDir(AppPaths::configHome()).filePath("dl-mod-manager/settings.conf"));
        if(!old.open(QIODevice::ReadOnly))return 2;
        const auto before=old.readAll();old.close();
        QSettings settings(AppPaths::preferencesLocation().toLocalFile(),QSettings::IniFormat);
        settings.setValue("display/showMaturePreviews",false);settings.sync();
        if(!old.open(QIODevice::ReadOnly))return 2;
        if(old.readAll()!=before){std::fputs("FAIL legacy preferences changed\n",stderr);return 1;}
    } else if(mode=="blocked-predecessor" || mode=="blocked-existing") {
        const auto destination=QDir(AppPaths::configHome()).filePath("modlock/settings.conf");
        QFile existing(destination);QByteArray before;
        if(mode=="blocked-existing"){if(!existing.open(QIODevice::ReadOnly))return 2;before=existing.readAll();existing.close();}
        const auto location=AppPaths::preferencesLocation().toLocalFile();
        if(location!="/dev/null/settings.conf" || AppPaths::preferencesSaveNotice.isEmpty() || !AppPaths::preferencesDefaults.isEmpty()
           || (mode!="blocked-existing" && QFileInfo::exists(destination)))return 3;
        QSettings session(location,QSettings::IniFormat);session.setValue("display/showMaturePreviews",true);session.sync();
        if(mode=="blocked-existing" && (!existing.open(QIODevice::ReadOnly) || existing.readAll()!=before))return 5;
    } else if(mode=="helper") {
        const auto root=AppPaths::dataRoot(QString::fromUtf8(argv[2]));
        if(!root.isEmpty() || !AppPaths::rootSelectionError.contains("--data-dir"))return 4;
    } else {
        const auto actual=mode=="state" ? AppPaths::stateRoot() : AppPaths::dataRoot();
        if(actual!=QString::fromUtf8(argv[2])){std::fprintf(stderr,"FAIL %s: %s\n",argv[1],actual.toUtf8().constData());return 1;}
    }
    std::puts("PASS app paths contract");
}
