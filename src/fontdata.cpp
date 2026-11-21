#include "fontdata.h"
#include <QDir>
#include <QJsonArray>
#include <QJsonDocument>
#include <algorithm>
#include <cmath>

namespace {
constexpr double fontBytes = 16777216., totalBytes = 134217728.;
bool keys(const QJsonObject &o, std::initializer_list<const char *> names) {
    if (o.size() != qsizetype(names.size())) return false;
    for (const auto *n : names) if (!o.contains(QLatin1String(n))) return false;
    return true;
}
bool text(const QJsonValue &v, int max = 4096, bool required = true, bool controls = true) {
    const auto s = v.toString();
    if (!v.isString() || s.toUtf8().size() > max || (required && s.isEmpty())) return false;
    if (controls && std::any_of(s.cbegin(), s.cend(), [](QChar c) { return c.category() == QChar::Other_Control; })) return false;
    return true;
}
bool number(const QJsonValue &v, double max = 18446744073709551615., double min = 0) {
    return v.isDouble() && std::isfinite(v.toDouble()) && v.toDouble() >= min && v.toDouble() <= max
        && v.toDouble() < 18446744073709551616. && std::floor(v.toDouble()) == v.toDouble();
}
bool name(const QJsonValue &v, bool font = true) {
    const auto s = v.toString();
    return text(v, font ? 240 : 255) && s != "." && s != ".." && !s.contains('/') && !s.contains('\\')
        && (!font || s.endsWith(".ttf", Qt::CaseInsensitive));
}
bool family(const QJsonValue &v) {
    if (!text(v, 1024) || v.toString().toUcs4().size() > 256) return false;
    for (const auto c : v.toString().toUcs4()) if (!(c >= 0x20 && c <= 0xd7ff) && !(c >= 0xe000 && c <= 0xfffd) && !(c >= 0x10000 && c <= 0x10ffff)) return false;
    return true;
}
bool relative(const QJsonValue &v, bool controls = true) {
    if (!text(v, 4096, true, controls) || v.toString().startsWith('/')) return false;
    const auto parts = v.toString().split('/');
    if (parts.size() > 32) return false;
    for (const auto &p : parts) if (p.isEmpty() || p == "." || p == "..") return false;
    return true;
}
bool path(const QJsonValue &v) { return text(v, 4096, true, false) && QDir::isAbsolutePath(v.toString()) && QDir::cleanPath(v.toString()) == v.toString(); }
bool strings(const QJsonValue &v, int count = 65536, int bound = 4096) {
    if (!v.isArray() || v.toArray().size() > count) return false;
    for (const auto &s : v.toArray()) if (!text(s, bound, false, false)) return false;
    return true;
}
bool warnings(const QJsonValue &v) {
    // Individual native warnings may aggregate many targets. Their text is
    // bounded by the already admitted 8 MiB summary, not the generic string cap.
    if (!v.isArray() || v.toArray().size() > 65536) return false;
    for (const auto &warning : v.toArray()) if (!warning.isString()) return false;
    return true;
}
void appendWarningRows(const QString &warning, QVariantList &rows) {
    qsizetype begin = 0;
    do {
        qsizetype end = begin;
        for (int scalars = 0; end < warning.size() && scalars < 1024; ++scalars) {
            const bool pair = warning.at(end).isHighSurrogate() && end + 1 < warning.size()
                && warning.at(end + 1).isLowSurrogate();
            end += pair ? 2 : 1;
        }
        rows.append(QVariantMap{{"message", warning.mid(begin, end - begin)}});
        begin = end;
    } while (begin < warning.size());
}
bool file(const QJsonValue &v, double max) {
    const auto o = v.toObject();
    return v.isObject() && keys(o, {"id","sha256","size","sourceName"}) && FontData::hex(o["id"],32) && FontData::hex(o["sha256"],64) && number(o["size"], max) && name(o["sourceName"],false);
}
bool container(const QJsonValue &v, bool stored, QSet<QString> *archiveIds, const QJsonObject &context) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!number(o["rootOrdinal"],63)) return false;
    if (context.contains("sources")) {
        bool found = false;
        for (const auto &s : context["sources"].toArray()) if (s.toObject()["rootOrdinal"] == o["rootOrdinal"]) found = true;
        if (!found) return false;
    }
    if (o["kind"] == QJsonValue("raw")) return keys(o,{"kind","rootOrdinal","relativePath"}) && relative(o["relativePath"],false);
    if (o["kind"] != QJsonValue("archive") || o.size() != 7 + o.contains("archiveId")
        || !relative(o["archiveRelativePath"],false) || !FontData::hex(o["archiveSha256"],64)
        || !number(o["memberIndex"],2147483647.) || !text(o["decoderPath"]) || !relative(o["normalizedPath"])
        || (stored && !FontData::hex(o["archiveId"],32)) || (o.contains("archiveId") && !FontData::hex(o["archiveId"],32))) return false;
    if (stored && context.contains("archives")) {
        bool matched = false;
        for (const auto &a : context["archives"].toArray()) {
            const auto r = a.toObject(); const auto f = r["file"].toObject();
            if (r["rootOrdinal"] == o["rootOrdinal"] && r["relativePath"] == o["archiveRelativePath"] && f["id"] == o["archiveId"] && f["sha256"] == o["archiveSha256"]) matched = true;
        }
        if (!matched) return false;
    }
    if (archiveIds && stored) archiveIds->insert(o["archiveId"].toString());
    return true;
}
bool origin(const QJsonValue &v, bool stored, bool diagnostic, QSet<QString> *archiveIds, const QJsonObject &context) {
    if (!v.isObject()) return false;
    const auto o = v.toObject(); const auto kind = o["kind"].toString();
    if (!container(o["container"],stored,archiveIds,context)) return false;
    if (kind == "loose") return keys(o,{"kind","container"});
    const bool unavailable = kind == "unavailableVpk";
    if ((kind != "vpk" && !(diagnostic && unavailable)) || !FontData::hex(o["candidateId"],64)
        || !o["members"].isArray() || o["members"].toArray().isEmpty() || o["members"].toArray().size() > (unavailable ? 10000 : 2)) return false;
    if (context.contains("selectedCandidateIds") && !context["selectedCandidateIds"].toArray().contains(o["candidateId"])) return false;
    if (unavailable ? !keys(o,{"kind","container","candidateId","members"})
        : !keys(o,{"kind","container","candidateId","resourcePath","resourceIndex","archiveIndex","resourceCrc","members"})
          || !text(o["resourcePath"]) || !number(o["resourceIndex"],99999) || !number(o["archiveIndex"],65535) || !number(o["resourceCrc"],4294967295.)) return false;
    int last = -2;
    for (const auto &m : o["members"].toArray()) {
        const auto r = m.toObject(); const int index = r["archiveIndex"].isNull() ? -1 : r["archiveIndex"].toInt(-2);
        if (!m.isObject() || !keys(r,{"archiveIndex","container","sha256","size"}) || (index != -1 && !number(r["archiveIndex"],65535))
            || index <= last || !container(r["container"],stored,archiveIds,context) || !FontData::hex(r["sha256"],64) || !number(r["size"],17179869184.)) return false;
        if (last == -2 && (index != -1 || r["container"] != o["container"])) return false;
        if (!unavailable && index >= 0 && (index != o["archiveIndex"].toInt() || index == 0x7fff)) return false;
        last = index;
    }
    return true;
}
bool origins(const QJsonValue &v, bool stored, bool diagnostic, int &count, QSet<QString> *archives, const QJsonObject &context) {
    if (!v.isArray() || v.toArray().size() > (diagnostic ? 1 : 256) || (stored && v.toArray().isEmpty())) return false;
    QSet<QByteArray> seen;
    for (const auto &o : v.toArray()) {
        const auto bytes = QJsonDocument(o.toObject()).toJson(QJsonDocument::Compact);
        if (!origin(o,stored,diagnostic,archives,context) || seen.contains(bytes) || ++count > (diagnostic ? 512 : 256)) return false;
        seen.insert(bytes);
    }
    return true;
}
bool identity(const QJsonValue &v) {
    const auto o=v.toObject(); auto shape=o;shape.remove("fsid");
    return v.isObject() && (!o.contains("fsid") || FontData::hex(o["fsid"],16)) && keys(shape,{"device","inode"}) && number(o["device"]) && number(o["inode"],18446744073709551615.,1);
}
bool fingerprint(const QJsonValue &v, double max) {
    const auto o=v.toObject(); auto shape=o;shape.remove("fsid");
    return v.isObject() && (!o.contains("fsid") || FontData::hex(o["fsid"],16)) && keys(shape,{"sha256","size","device","inode","modifiedSeconds","modifiedNanos"}) && FontData::hex(o["sha256"],64)
        && number(o["size"],max) && number(o["device"]) && number(o["inode"],18446744073709551615.,1)
        && number(o["modifiedSeconds"],9223372036854775807.,-9223372036854775808.) && o["modifiedSeconds"].toDouble()<9223372036854775808. && number(o["modifiedNanos"],999999999);
}
bool owner(const QJsonValue &v) {
    const auto o=v.toObject(); return v.isObject() && keys(o,{"modId","assetId","displayName"}) && FontData::safeId(o["modId"]) && FontData::hex(o["assetId"],32) && family(o["displayName"]);
}
bool target(const QJsonValue &v) {
    const auto o=v.toObject(); return v.isObject() && ((o["kind"] == QJsonValue("configuration") && keys(o,{"kind"})) || (o["kind"] == QJsonValue("font") && keys(o,{"kind","name"}) && name(o["name"])));
}
QString targetKey(const QJsonObject &o) { return o["kind"] == QJsonValue("configuration") ? QStringLiteral("0") : "1"+o["name"].toString(); }
bool footprint(const QJsonValue &v, const QString &game, QHash<QString,QVariantList> *index) {
    if (!v.isObject()) return false;
    const auto o=v.toObject();
    if (!keys(o,{"rootPath","rootIdentity","gamePath","gameIdentity","directory","directoryCreated","files","conf","confMode","ownedBlock"})
        || !path(o["rootPath"]) || !path(o["gamePath"]) || o["gamePath"].toString()!=game || !identity(o["rootIdentity"]) || !identity(o["gameIdentity"])
        || !(o["directory"].isNull() || identity(o["directory"])) || !o["directoryCreated"].isBool() || !o["files"].isArray() || o["files"].toArray().size()>256
        || o["conf"].isNull()!=o["confMode"].isNull() || (!o["conf"].isNull() && (!fingerprint(o["conf"],4194304.) || !number(o["confMode"],511)))) return false;
    QSet<QString> pairs,names,families; QByteArray previous; double total=0;
    for (const auto &f:o["files"].toArray()) {
        const auto r=f.toObject(); const auto n=r["targetName"].toString();
        if (!f.isObject() || !keys(r,{"targetName","ownership","identity","mode","owners"}) || !name(r["targetName"]) || n.toUtf8()<=previous || names.contains(n.toUpper().toLower())
            || (r["ownership"]!=QJsonValue("created") && r["ownership"]!=QJsonValue("borrowed")) || !fingerprint(r["identity"],fontBytes) || !number(r["mode"],511)
            || !r["owners"].isArray() || r["owners"].toArray().isEmpty() || r["owners"].toArray().size()>65536) return false;
        previous=n.toUtf8(); names.insert(n.toUpper().toLower()); total+=r["identity"].toObject()["size"].toDouble(); if(total>totalBytes) return false;
        QString lastMod,lastAsset; bool firstOwner=true;
        for (const auto &a:r["owners"].toArray()) {
            const auto own=a.toObject(); const auto pair=own["modId"].toString()+":"+own["assetId"].toString();
            const auto mod=own["modId"].toString(),asset=own["assetId"].toString();
            if (!owner(a) || pairs.contains(pair) || (!firstOwner && (mod<lastMod || (mod==lastMod && asset<=lastAsset))) || pairs.size()>=65536) return false;
            pairs.insert(pair); lastMod=mod; lastAsset=asset; firstOwner=false; families.insert(own["displayName"].toString());
            if(index) (*index)[own["modId"].toString()].append(QVariantMap{{"fontId",own["assetId"].toString()},{"targetName",n},{"displayName",own["displayName"].toString()},{"status",r["ownership"]==QJsonValue("borrowed")?"borrowed":"installed"},{"ownerCount",r["owners"].toArray().size()}});
        }
    }
    if (o["directory"].isNull() && (o["directoryCreated"].toBool() || !names.isEmpty() || !o["conf"].isNull())) return false;
    if (!o["ownedBlock"].isNull()) {
        const auto b=o["ownedBlock"].toObject();
        if (!o["ownedBlock"].isObject() || !keys(b,{"rootId","id","names","bytes"}) || !FontData::hex(b["rootId"],64) || !FontData::hex(b["id"],32)
            || !strings(b["names"],65536,1024) || !text(b["bytes"],4194304,false,false) || o["conf"].isNull() || families.isEmpty()) return false;
        QSet<QString> blockNames; QByteArray prior;
        for(const auto &s:b["names"].toArray()) { const auto encoded=s.toString().toUtf8(); if(!family(s) || (!prior.isEmpty() && encoded<=prior)) return false; prior=encoded; blockNames.insert(s.toString()); }
        if(blockNames!=families) return false;
    } else if (!o["conf"].isNull() && !families.isEmpty()) return false;
    return true;
}
}

bool FontData::hex(const QJsonValue &v,int length) { const auto s=v.toString(); return v.isString() && s.size()==length && std::all_of(s.cbegin(),s.cend(),[](QChar c){return (c>='0'&&c<='9')||(c>='a'&&c<='f');}); }
bool FontData::safeId(const QJsonValue &v) { const auto s=v.toString(); return text(v,64) && std::all_of(s.cbegin(),s.cend(),[](QChar c){return (c.unicode()<128&&c.isLetterOrNumber())||c=='_'||c=='-';}); }
bool FontData::revision(const QJsonValue &v) { bool ok=false; const auto n=v.toString().toULongLong(&ok); return v.isString() && ok && QString::number(n)==v.toString(); }

bool FontData::refs(const QJsonValue &v,QVariantList &out,double &bytes,QSet<QString> *archives,const QJsonObject &context) {
    if(!v.isArray() || v.toArray().size()>256 || v.toArray().isEmpty()) return false;
    QString previous; int count=0; bytes=0;
    for(const auto &f:v.toArray()) {
        const auto r=f.toObject(); const auto a=r["asset"].toObject(); const auto id=a["id"].toString();
        if(!f.isObject() || !keys(r,{"asset","targetName","displayName","origins"}) || !file(r["asset"],fontBytes) || id<=previous || !name(r["targetName"])
            || a["sourceName"]!=r["targetName"] || !family(r["displayName"]) || !origins(r["origins"],true,false,count,archives,context)) return false;
        previous=id; bytes+=a["size"].toDouble(); if(bytes>totalBytes) return false;
        for(const auto &origin:r["origins"].toArray()) {
            const auto originObject=origin.toObject(), c=originObject["container"].toObject();
            const auto source=originObject["kind"]==QJsonValue("vpk") ? originObject["resourcePath"].toString()
                : c[c["kind"]==QJsonValue("raw")?"relativePath":"normalizedPath"].toString();
            if(!source.endsWith(".ttf",Qt::CaseInsensitive) || source.section('/',-1).toUpper().toLower()!=r["targetName"].toString().toUpper().toLower()) return false;
        }
        out.append(r.toVariantMap());
    }
    return true;
}
bool FontData::candidates(const QJsonValue &v,bool diagnostics,const QJsonObject &context) {
    if(!v.isArray() || v.toArray().size()>512) return false;
    QSet<QString> ids; int validOrigins=0,diagnosticOrigins=0,prepared=0; double bytes=0;
    for(const auto &f:v.toArray()) {
        const auto r=f.toObject(); const bool hasBytes=!r["sha256"].isNull(); const bool available=r["available"].toBool();
        if(!f.isObject() || !keys(r,{"id","targetName","displayName","origins","available","unavailableReason","sha256","size"}) || !hex(r["id"],32) || ids.contains(r["id"].toString())
            || !r["available"].isBool() || !text(r["targetName"],4096,false,false) || !text(r["displayName"],4096,false,false)
            || !(r["unavailableReason"].isNull() || text(r["unavailableReason"],4096,false,false)) || hasBytes!=!r["size"].isNull()
            || (diagnostics && (hasBytes || available || !r["unavailableReason"].isString()))
            || (available && (!hasBytes || !r["unavailableReason"].isNull()))
            || (!hasBytes && !r["unavailableReason"].isString())
            || (hasBytes && (!hex(r["sha256"],64) || !number(r["size"],fontBytes) || !name(r["targetName"]) || !family(r["displayName"]) || r["origins"].toArray().isEmpty()))
            || !origins(r["origins"],false,!hasBytes,hasBytes?validOrigins:diagnosticOrigins,nullptr,context)) return false;
        if(hasBytes) { if(++prepared>256) return false; bytes+=r["size"].toDouble(); if(bytes>totalBytes) return false; }
        ids.insert(r["id"].toString());
    }
    return true;
}
bool FontData::global(const QJsonValue &v,const QString &game,bool &canUndo,QHash<QString,QVariantList> &owners) {
    if(!v.isObject() || QJsonDocument(v.toObject()).toJson(QJsonDocument::Compact).size()>8*1024*1024) return false;
    const auto o=v.toObject();
    if(!keys(o,{"version","installed","previous"}) || o["version"]!=QJsonValue(1) || (!o["installed"].isNull() && !footprint(o["installed"],game,&owners))) return false;
    canUndo=!o["previous"].isNull();
    if(!canUndo) return true;
    const auto p=o["previous"].toObject();
    if(o["installed"].isNull() || !o["previous"].isObject() || !keys(p,{"transactionId","prior","images"}) || !hex(p["transactionId"],32)
        || (!p["prior"].isNull() && !footprint(p["prior"],game,nullptr)) || !p["images"].isArray() || p["images"].toArray().size()>257) return false;
    if (!p["prior"].isNull()) for (const auto *k:{"rootPath","rootIdentity","gameIdentity"}) if(p["prior"].toObject()[k]!=o["installed"].toObject()[k]) return false;
    QSet<QString> ids; QByteArray last; double bytes=0;
    for(const auto &r:p["images"].toArray()) {
        const auto a=r.toObject(), t=a["target"].toObject(), image=a["image"].toObject(); const auto key=targetKey(t).toUtf8();
        if(!r.isObject() || !keys(a,{"target","image"}) || !target(a["target"]) || key<=last || !a["image"].isObject()) return false;
        last=key;
        if(image["kind"]==QJsonValue("absent")) { if(!keys(image,{"kind"})) return false; continue; }
        const double max=t["kind"]==QJsonValue("configuration")?4194304.:fontBytes; const auto b=image["backup"].toObject(), f=image["identity"].toObject();
        if(image["kind"]!=QJsonValue("present") || !keys(image,{"kind","backup","identity","mode"}) || !file(image["backup"],max) || !fingerprint(image["identity"],max)
            || !number(image["mode"],511) || b["sha256"]!=f["sha256"] || b["size"]!=f["size"] || ids.contains(b["id"].toString())) return false;
        ids.insert(b["id"].toString()); bytes+=b["size"].toDouble(); if(bytes>138412032.) return false;
    }
    return true;
}
bool FontData::review(const QJsonValue &v,QVariantMap &out) {
    if(!v.isObject()) return false;
    const auto o=v.toObject(), s=o["summary"].toObject(); auto base=s; base.remove("patternChanges"); base.remove("ownerChanges"); const auto action=s["action"].toString(); const bool undo=action=="undo", install=action=="install";
    if(!keys(o,{"token","expectedRevision","gamePath","summary"}) || !hex(o["token"],32) || !revision(o["expectedRevision"]) || !path(o["gamePath"])
        || !o["summary"].isObject() || QJsonDocument(s).toJson(QJsonDocument::Compact).size()>8*1024*1024
        || !keys(base,{"version","action","modId","fontIds","gamePath","revision","canCommit","hasChanges","fonts","collisions","warnings","installedFileCount","totalFontBytes","writeBytes","backupBytes","createDirectory","removeDirectory"})
        || s["version"]!=QJsonValue(1) || (!undo && !install && action!="remove") || s["revision"]!=o["expectedRevision"] || s["gamePath"]!=o["gamePath"]
        || (undo ? !s["modId"].isNull() : !safeId(s["modId"])) || !s["fontIds"].isArray() || s["fontIds"].toArray().size()>256
        || (undo ? !s["fontIds"].toArray().isEmpty() : s["fontIds"].toArray().isEmpty()) || !s["canCommit"].isBool() || !s["hasChanges"].isBool()
        || !s["createDirectory"].isBool() || !s["removeDirectory"].isBool() || !s["fonts"].isArray() || s["fonts"].toArray().size()>(undo?257:256)
        || !warnings(s["warnings"]) || !s["collisions"].isArray() || s["collisions"].toArray().size()>256 || !number(s["installedFileCount"],256)
        || !number(s["totalFontBytes"],totalBytes) || !number(s["writeBytes"],138412032.) || !number(s["backupBytes"],138412032.)) return false;
    QSet<QString> ids, rows, collisions;
    QHash<QString,QJsonArray> collisionReasons;
    for(const auto &id:s["fontIds"].toArray()) { if(!hex(id,32) || ids.contains(id.toString())) return false; ids.insert(id.toString()); }
    for(const auto &c:s["collisions"].toArray()) {
        const auto r=c.toObject(); if(!c.isObject() || !keys(r,{"fontId","reasons"}) || !hex(r["fontId"],32) || !ids.contains(r["fontId"].toString()) || collisions.contains(r["fontId"].toString()) || !strings(r["reasons"]) || r["reasons"].toArray().isEmpty()) return false;
        collisions.insert(r["fontId"].toString());
        collisionReasons.insert(r["fontId"].toString(),r["reasons"].toArray());
    }
    for(const auto &f:s["fonts"].toArray()) {
        const auto r=f.toObject(); const auto d=r["disposition"].toString();
        if(!f.isObject()) return false;
        if(undo) { if(!keys(r,{"target","disposition"}) || !target(r["target"]) || (d!="restoreExact"&&d!="restoreAbsence") || rows.contains(targetKey(r["target"].toObject()))) return false; rows.insert(targetKey(r["target"].toObject())); continue; }
        const auto id=r["fontId"].toString();
        if(!hex(r["fontId"],32) || !ids.contains(id) || rows.contains(id) || !name(r["targetName"]) || !r["available"].isBool() || !strings(r["collisions"])) return false;
        if(r["available"].toBool()==collisions.contains(id) || r["collisions"].toArray()!=collisionReasons.value(id)) return false;
        rows.insert(id);
        if(install) {
            if(!keys(r,{"fontId","targetName","displayName","disposition","size","sha256","beforeSha256","beforeSize","releasedOwners","collisions","available"})
                || !QStringList{"create","borrow","share","replace","blocked"}.contains(d) || !family(r["displayName"]) || !number(r["size"],fontBytes) || !hex(r["sha256"],64)
                || r["beforeSha256"].isNull()!=r["beforeSize"].isNull() || (!r["beforeSha256"].isNull() && (!hex(r["beforeSha256"],64)||!number(r["beforeSize"],fontBytes)))
                || !r["releasedOwners"].isArray() || r["releasedOwners"].toArray().size()>65536) return false;
            if((d=="blocked")!=collisions.contains(id)) return false;
            QSet<QString> released;
            for(const auto &a:r["releasedOwners"].toArray()) { const auto own=a.toObject(); const auto key=own["modId"].toString()+":"+own["assetId"].toString(); if(!owner(a)||released.contains(key)) return false; released.insert(key); }
        } else if(!keys(r,{"fontId","targetName","remainingOwnerCount","disposition","collisions","available"}) || !number(r["remainingOwnerCount"],65536) || !QStringList{"delete","releaseBorrowed","releaseOwner"}.contains(d)) return false;
    }
    if(s["canCommit"].toBool()!=(s["hasChanges"].toBool() && collisions.isEmpty()) || (!undo && (install ? rows!=ids : (rows|collisions)!=ids))) return false;
    QVariantList displayRows;
    if(s.contains("ownerChanges")) {
        const auto delta=s["ownerChanges"];
        if(!delta.isArray() || delta.toArray().size()>512) return false;
        QByteArray previousTarget;
        QSet<QString> allAdded,allReleased;
        int beforeTotal=0,afterTotal=0,beforeTargets=0,afterTargets=0;
        for(const auto &value:delta.toArray()) {
            const auto row=value.toObject();
            const auto encoded=row["targetName"].toString().toUtf8();
            const auto disposition=row["disposition"].toString();
            if(!value.isObject() || !keys(row,{"targetName","added","released","beforeCount","afterCount","disposition"})
                || !name(row["targetName"]) || encoded<=previousTarget
                || !number(row["beforeCount"],65536) || !number(row["afterCount"],65536)
                || !QStringList{"writeExact","restoreAbsence","preserveBorrowed","preserveFile"}.contains(disposition)) return false;
            previousTarget=encoded;
            QHash<QString,QString> added,released;
            for(const auto *key:{"added","released"}) {
                const bool adding=QLatin1String(key)==QLatin1String("added");
                auto &seen=adding?allAdded:allReleased;
                auto &owners=adding?added:released;
                const auto ownersValue=row[key];
                if(!ownersValue.isArray() || ownersValue.toArray().size()>65536) return false;
                QString previousMod,previousAsset;
                bool first=true;
                for(const auto &entry:ownersValue.toArray()) {
                    const auto o=entry.toObject();
                    const auto mod=o["modId"].toString(),asset=o["assetId"].toString(),pair=mod+":"+asset;
                    if(!owner(entry) || seen.contains(pair) || seen.size()>=65536
                        || (!first && (mod<previousMod || (mod==previousMod && asset<=previousAsset)))) return false;
                    first=false; previousMod=mod; previousAsset=asset;
                    seen.insert(pair); owners.insert(pair,o["displayName"].toString());
                }
            }
            if(added.isEmpty() && released.isEmpty()) return false;
            const auto before=row["beforeCount"].toInt(),after=row["afterCount"].toInt();
            beforeTotal+=before; afterTotal+=after;
            beforeTargets+=before>0; afterTargets+=after>0;
            if(beforeTotal>65536 || afterTotal>65536 || beforeTargets>256 || afterTargets>s["installedFileCount"].toInt()
                || released.size()>before || added.size()>after || before-released.size()+added.size()!=after
                || (disposition=="restoreAbsence" && after!=0) || (disposition=="writeExact" && after==0)) return false;
            for(auto it=added.cbegin();it!=added.cend();++it)
                if(released.contains(it.key()) && released.value(it.key())==it.value()) return false;
            displayRows.append(QVariantMap{{"ownerDelta",true},{"targetName",row["targetName"].toString()},
                {"beforeCount",before},{"afterCount",after},{"addedCount",added.size()},{"releasedCount",released.size()},{"disposition",disposition}});
            for(const auto *key:{"released","added"}) for(const auto &entry:row[key].toArray()) {
                auto ownerRow=entry.toObject().toVariantMap();
                ownerRow.insert("ownerAction",QLatin1String(key)==QLatin1String("added")?"add":"release");
                ownerRow.insert("targetName",row["targetName"].toString());
                displayRows.append(ownerRow);
            }
        }
    }
    bool patternDelta=false;
    if(s.contains("patternChanges")) {
        const auto p=s["patternChanges"].toObject();
        if(!s["patternChanges"].isObject() || !keys(p,{"configurationBefore","configurationAfter","before","after"})
            || !p["configurationBefore"].isBool() || !p["configurationAfter"].isBool()) return false;
        QSet<QString> before,after;
        for(const auto *key:{"before","after"}) {
            const bool prior=QLatin1String(key)==QLatin1String("before");
            const auto values=p[key];
            if(!values.isArray() || values.toArray().size()>65536 || (!p[prior?"configurationBefore":"configurationAfter"].toBool() && !values.toArray().isEmpty())) return false;
            QByteArray previous;
            auto &names=prior?before:after;
            for(const auto &value:values.toArray()) {
                const auto encoded=value.toString().toUtf8();
                if(!family(value) || (!previous.isEmpty() && encoded<=previous)) return false;
                previous=encoded; names.insert(value.toString());
            }
        }
        for(const auto &value:p["before"].toArray()) if(!after.contains(value.toString())) { displayRows.append(QVariantMap{{"patternAction","remove"},{"patternName",value.toString()}}); patternDelta=true; }
        for(const auto &value:p["after"].toArray()) if(!before.contains(value.toString())) { displayRows.append(QVariantMap{{"patternAction","add"},{"patternName",value.toString()}}); patternDelta=true; }
    }
    for(const auto &w:s["warnings"].toArray()) appendWarningRows(w.toString(),displayRows);
    for(const auto &c:s["collisions"].toArray()) {
        const auto collision=c.toObject();
        for(const auto &reason:collision["reasons"].toArray()) displayRows.append(QVariantMap{{"fontId",collision["fontId"].toString()},{"message",reason.toString()}});
    }
    for(const auto &f:s["fonts"].toArray()) displayRows.append(f.toObject().toVariantMap());
    out=o.toVariantMap(); out.insert("displayRows",displayRows); out.insert("exactPatterns",s.contains("patternChanges")); out.insert("exactOwners",s.contains("ownerChanges")); out.insert("patternDelta",patternDelta); return true;
}
