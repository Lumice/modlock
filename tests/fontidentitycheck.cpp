#include "fontdata.h"
#include <QCoreApplication>
#include <QJsonArray>
#include <cstdio>
int main(int argc,char **argv) {
    QCoreApplication app(argc,argv);
    const QJsonObject identity{{"device",47},{"inode",42},{"fsid","1234567890abcdef"}};
    const QJsonObject fingerprint{{"device",47},{"inode",43},{"fsid","1234567890abcdef"},{"size",12},{"sha256",QString(64,'a')},{"modifiedSeconds",1},{"modifiedNanos",0}};
    QJsonObject footprint{{"rootPath","/fixture/library"},{"rootIdentity",identity},{"gamePath","/fixture/game"},{"gameIdentity",identity},{"directory",identity},{"directoryCreated",false},{"files",QJsonArray{}},{"conf",fingerprint},{"confMode",420},{"ownedBlock",QJsonValue::Null}};
    auto accepts=[](const QJsonObject &f){bool undo=false;QHash<QString,QVariantList> owners;return FontData::global(QJsonObject{{"version",1},{"installed",f},{"previous",QJsonValue::Null}},"/fixture/game",undo,owners);};
    if (!accepts(footprint)) {std::fputs("valid font fsids refused\n",stderr);return 1;}
    for (const auto *key:{"rootIdentity","gameIdentity","directory","conf"}) {
        for (const QJsonValue bad:{QJsonValue(4),QJsonValue("bad"),QJsonValue(QJsonValue::Null)}) {
            auto f=footprint;auto id=f[key].toObject();id["fsid"]=bad;f[key]=id;
            if(accepts(f)){std::fputs("malformed font fsid accepted\n",stderr);return 1;}
        }
        auto id=footprint[key].toObject();id.remove("fsid");footprint[key]=id;
    }
    if (!accepts(footprint)) {std::fputs("legacy font identities refused\n",stderr);return 1;}
    std::puts("font fsid schema checks passed");return 0;
}
