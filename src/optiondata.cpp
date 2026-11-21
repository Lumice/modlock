#include "optiondata.h"
#include "fontdata.h"
#include <QJsonArray>
#include <QJsonDocument>
#include <QSet>
#include <QLocale>
#include <cmath>
#include <algorithm>
namespace {
bool text(const QJsonValue &v,int max=4096,bool required=false) { const auto s=v.toString();return v.isString() && (!required || !s.isEmpty()) && s.toUtf8().size()<=max && std::none_of(s.cbegin(),s.cend(),[](QChar c){return c.category()==QChar::Other_Control;}); }
bool hex(const QJsonValue &v,int n=32) { return v.isString() && FontData::hex(v.toString(),n); }
bool id(const QJsonValue &v) { const auto s=v.toString(); return text(v,64,true) && std::all_of(s.cbegin(),s.cend(),[](QChar c){return c.unicode()<128 && (c.isLetterOrNumber() || c=='_' || c=='-');}); }
bool number(const QJsonValue &v,double max=137589948416.) {return v.isDouble() && std::isfinite(v.toDouble()) && v.toDouble()>=0 && v.toDouble()<=max && v.toDouble()<18446744073709551616. && std::floor(v.toDouble())==v.toDouble();}
bool companionIndex(const QJsonValue &v) { return number(v,65535) && v.toInt()!=32767; }
bool decimal(const QJsonValue &v) { bool ok=false; auto n=v.toString().toULongLong(&ok);return v.isString() && ok && QString::number(n)==v.toString();}
bool diagnostic(const QJsonValue &v) {auto o=v.toObject();return v.isObject() && o.size()==2 && text(o["kind"],256,true) && text(o["message"]);}
bool totals(const QJsonValue &v) {
    auto o=v.toObject(); if(!v.isObject() || o.size()!=12) return false;
    for(const auto *k:{"optionCount","selectedCount","activeFamilies","retainedFamilies","unavailableCount","selectedLogicalBytes","retainedLogicalBytes","uniquePayloadBytes","retainedArchiveBytes","artworkBytes","fontBytes","uniqueCopyBytes"}) if(!number(o[k]))return false;
    return o["optionCount"].toDouble()<=1024 && o["selectedCount"].toDouble()<=99 && o["unavailableCount"].toDouble()<=o["optionCount"].toDouble();
}
bool file(const QJsonValue &v,bool ref,double max=17179869184.) {auto o=v.toObject(); return v.isObject() && o.size()==(ref?4:3) && (!ref || hex(o["id"])) && hex(o["sha256"],64) && number(o["size"],max) && text(o["sourceName"],255,true);}
bool family(const QJsonValue &v,bool ref) {
    auto o=v.toObject(); if(!v.isObject() || o.size()!=2 || !file(o["directory"],ref) || !o["companions"].isArray() || o["companions"].toArray().size()>9999)return false;
    QSet<int> seen;
    for(const auto &c:o["companions"].toArray()){auto m=c.toObject(); if(!c.isObject() || m.size()!=2 || !companionIndex(m["archiveIndex"]) || seen.contains(m["archiveIndex"].toInt()) || !file(m["file"],ref))return false;seen.insert(m["archiveIndex"].toInt());} return true;
}
bool detail(const QJsonObject &o,QVariantMap &out) {
    auto d=o["detail"].toObject(),catalog=d["catalog"].toObject();
    if(o.size()!=5 || o["version"]!=QJsonValue(1) || !id(o["profileId"]) || !id(o["modId"]) || !decimal(o["expectedRevision"]) || d.size()!=7 || d["version"]!=QJsonValue(1) || d["modId"]!=o["modId"] || (d["layout"]!=QJsonValue(1)&&d["layout"]!=QJsonValue(2)) || !d["legacy"].isBool() || !totals(d["totals"]) || catalog.size()!=5 || catalog["version"]!=QJsonValue(1))return false;
    for(const auto *key:{"entries","selectedIds","retainedArchives","attachments"})if(!catalog[key].isArray() || catalog[key].toArray().size()>1024)return false;
    if(!d["availability"].isArray())return false;
    QMap<QString,QString> availability; QSet<QString> selected,seen;
    for(const auto &v:d["availability"].toArray()){auto a=v.toObject(); if(!v.isObject()||a.size()!=2||!hex(a["optionId"]) || availability.contains(a["optionId"].toString()) || !QStringList{"ready","needsPreparation","needsSource","unavailable"}.contains(a["state"].toString()))return false; availability.insert(a["optionId"].toString(),a["state"].toString());}
    QString last;
    for(const auto &v:catalog["selectedIds"].toArray()){if(!hex(v)||(!last.isEmpty()&&v.toString()<=last)||selected.size()>=99)return false; last=v.toString();selected.insert(last);}
    QVariantList rows;
    for(const auto &v:catalog["entries"].toArray()){
        auto e=v.toObject(); auto optionId=e["id"].toString();auto state=availability.value(optionId);
        if(!v.isObject()||e.size()!=7||!hex(e["id"])||seen.contains(optionId)||state.isEmpty()||!text(e["optionName"],1024,true)||!text(e["sourceLabel"])||(!e["unavailableReason"].isNull()&&!text(e["unavailableReason"],4096,true)))return false;
        seen.insert(optionId);
        if(!e["expectedFamily"].isNull()&&!family(e["expectedFamily"],false))return false;
        if(!e["package"].isNull()&&!family(e["package"],true))return false;
        if((state=="ready")!=e["package"].isObject() || (selected.contains(optionId)&&state!="ready"))return false;
        QString archiveId;
        if(!e["archiveBinding"].isNull()){
            auto b=e["archiveBinding"].toObject();if(!e["archiveBinding"].isObject()||b.size()!=2||!file(b["file"],true,68719476736.)||!b["members"].isArray()||b["members"].toArray().size()>10000)return false;
            archiveId=b["file"].toObject()["id"].toString();
            for(const auto &m:b["members"].toArray()){auto a=m.toObject();if(!m.isObject()||a.size()!=4||(!a["archiveIndex"].isNull()&&!companionIndex(a["archiveIndex"]))||!number(a["memberIndex"],9999)||!text(a["normalizedPath"],4096,true)||!text(a["decoderPath"],4096,true))return false;}
        }
        const auto expected=e["expectedFamily"].toObject();
        rows.append(QVariantMap{{"id",optionId},{"optionName",e["optionName"].toString()},{"state",state},{"selected",selected.contains(optionId)},{"companionCount",expected["companions"].toArray().size()},{"archiveId",archiveId},{"sourceLabel",e["sourceLabel"].toString()},{"reason",e["unavailableReason"].toString()}});
    }
    if(d["totals"].toObject()["optionCount"].toInt()!=rows.size() || d["totals"].toObject()["selectedCount"].toInt()!=selected.size())return false;
    if(seen.size()!=availability.size() || !std::all_of(selected.cbegin(),selected.cend(),[&](const QString &s){return seen.contains(s);}))return false;
    for(const auto &v:catalog["retainedArchives"].toArray()){auto a=v.toObject();if(!v.isObject()||a.size()!=5||!file(a["file"],true,68719476736.)||!hex(a["attachmentId"])||!number(a["rootOrdinal"],63)||!text(a["relativePath"],4096,true)||!text(a["format"],32,true))return false;}
    for(const auto &v:catalog["attachments"].toArray()){auto a=v.toObject();if(!v.isObject()||a.size()!=3||!hex(a["id"])||!QStringList{"chosenRoots","retainedArchive"}.contains(a["sourceKind"].toString())||!text(a["sourceLabel"],4096,true))return false;}
    out={{"version",1},{"profileId",o["profileId"].toString()},{"modId",o["modId"].toString()},{"expectedRevision",o["expectedRevision"].toString()},{"detail",QVariantMap{{"rows",rows},{"selectedIds",catalog["selectedIds"].toArray().toVariantList()},{"totals",d["totals"].toObject().toVariantMap()},{"legacy",d["legacy"].toBool()}}}};return true;
}
bool status(const QJsonValue &v,QVariantMap &out){
    auto o=v.toObject(),small=o;small.remove("candidateSummary");small.remove("cleanupError");
    const QStringList phases{"enumerating","inspecting","hashing","copyingArchives","listingArchives","extractingArchives","ready","cancelRequested","cancelled","interrupted","failed","committing","succeeded","recoveryRequired"};
    if(!v.isObject()||small.size()!=18||QJsonDocument(small).toJson(QJsonDocument::Compact).size()>4096 || !hex(o["jobId"])||o["kind"]!=QJsonValue("localOptions")||o["jobVersion"]!=QJsonValue(1)||!id(o["profileId"])||!id(o["modId"])||!hex(o["beforeRecordDigest"],64)||!text(o["profileNameAtStart"])||!decimal(o["expectedRevision"])||!decimal(o["observedRevision"])||!decimal(o["sequence"])||o["sequence"]==QJsonValue("0")||!phases.contains(o["phase"].toString())||!o["canCancel"].isBool()||(!o["operationId"].isNull()&&!id(o["operationId"]))||(!o["error"].isNull()&&!diagnostic(o["error"]))||(o.contains("cleanupError")&&!diagnostic(o["cleanupError"])))return false;
    const bool cancellable=QStringList{"enumerating","inspecting","hashing","copyingArchives","listingArchives","extractingArchives","ready"}.contains(o["phase"].toString());
    if(o["canCancel"].toBool()!=cancellable)return false;
    for(const auto *k:{"filesDone","bytesDone"})if(!number(o[k],18446744073709551615.))return false;
    for(const auto *k:{"filesTotal","bytesTotal"})if(!o[k].isNull()&&!number(o[k],18446744073709551615.))return false;
    out=small.toVariantMap();if(o.contains("cleanupError"))out.insert("cleanupError",o["cleanupError"].toObject().toVariantMap());
    if(o.contains("candidateSummary")){
        auto summary=o["candidateSummary"].toObject();if(o["phase"]!=QJsonValue("ready")||summary.size()!=1||!summary["candidates"].isArray()||summary["candidates"].toArray().size()>1024)return false;
        QVariantList candidates; QSet<QString> seen;int count=0;
        for(const auto &v:summary["candidates"].toArray()){
            auto c=v.toObject();if(!v.isObject()||!hex(c["id"],64)||seen.contains(c["id"].toString())||!number(c["rootOrdinal"],63)||!text(c["relativePath"],4096,true)||(!c["archiveRelativePath"].isNull()&&!text(c["archiveRelativePath"],4096,true))||!text(c["optionName"],4096,true)||!c["available"].isBool()||(!c["unavailableReason"].isNull()&&!text(c["unavailableReason"]))||!number(c["companionCount"],9999))return false;
            seen.insert(c["id"].toString());
            if(c["available"].toBool()!=c["members"].isArray())return false;
            if(!c["members"].isNull()&&!c["members"].isArray())return false;
            for(const auto &m:c["members"].toArray()){
                auto f=m.toObject();if(++count>10000||!m.isObject()||(!f["archiveIndex"].isNull()&&!companionIndex(f["archiveIndex"]))||!text(f["sourceName"],4096,true)||!text(f["sourcePath"],4096,true)||!hex(f["sha256"],64)||!number(f["size"],17179869184.))return false;
                if(f.contains("archive")){auto a=f["archive"].toObject();if(!f["archive"].isObject()||!text(f["archiveRelativePath"],4096,true)||!hex(a["sha256"],64)||!number(a["index"],9999)||!text(a["decoderPath"],4096,true)||!text(a["memberPath"],4096,true)||!text(a["format"],32,true)||!text(a["encoding"],128,true))return false;}
            }
            candidates.append(QVariantMap{{"id",c["id"].toString()},{"optionName",c["optionName"].toString()},{"relativePath",c["relativePath"].toString()},{"rootOrdinal",c["rootOrdinal"].toInt()},{"archiveRelativePath",c["archiveRelativePath"].toString()},{"available",c["available"].toBool()},{"companionCount",c["companionCount"].toInt()},{"reason",c["unavailableReason"].toString()}});
        }
        out.insert("candidateSummary",QVariantMap{{"candidates",candidates}});
    }return true;
}
bool modIds(const QJsonValue &v){if(!v.isArray())return false;QSet<QString> seen;for(const auto &x:v.toArray()){if(!id(x)||seen.contains(x.toString()))return false;seen.insert(x.toString());}return true;}
bool selection(const QJsonValue &v){if(v.isNull())return true;auto o=v.toObject();if(!v.isObject()||o.size()!=2||!id(o["modId"])||!o["choices"].isArray()||o["choices"].toArray().size()>99)return false;QSet<QString> seen;for(const auto &c:o["choices"].toArray()){auto r=c.toObject();if(!c.isObject()||r.size()!=2||!hex(r["optionId"])||seen.contains(r["optionId"].toString())||!text(r["optionName"],1024,true))return false;seen.insert(r["optionId"].toString());}return true;}
bool integer(const QJsonValue &v) {return v.isDouble() && v.toInteger(-1)>=0;}
bool layoutCounts(const QJsonValue &v) {
    auto o=v.toObject();if(!v.isObject()||o.size()!=5)return false;
    for(const auto *k:{"selectedReferences","uniqueFamilies","companionFiles","physicalFiles","totalBytes"})if(!integer(o[k]))return false;
    const auto unique=o["uniqueFamilies"].toInteger(),physical=o["physicalFiles"].toInteger();
    return unique<=o["selectedReferences"].toInteger() && physical>=unique && physical-unique==o["companionFiles"].toInteger();
}
bool shardRows(const QJsonValue &v,qint64 &total) {
    if(!v.isArray()||v.toArray().isEmpty()||v.toArray().size()>10)return false;
    int index=0;total=0;
    for(const auto &row:v.toArray()) {auto r=row.toObject();if(!row.isObject()||r.size()!=2||r["index"]!=QJsonValue(++index)||!number(r["directoryCount"],99))return false;total+=r["directoryCount"].toInteger();}
    for(const auto &row:v.toArray())if(row.toObject()["directoryCount"]==QJsonValue(0) && !(v.toArray().size()==1&&total==0))return false;
    return true;
}
bool capacityOutcome(const QJsonValue &v) {
    auto o=v.toObject();if(!v.isObject()||o.size()!=5||!o["partial"].isBool())return false;
    const auto status=o["status"].toString();auto issue=o["issue"].toObject();
    if(status=="SelectionRequired"||status=="ExceedsCapacity") {
        if(!o["issue"].isObject()||issue.size()!=4||!id(issue["modId"])||!text(issue["title"],4096,true)||!text(issue["reason"],4096,true))return false;
    }else if(status!="Fits"||!o["issue"].isNull())return false;
    if(status=="SelectionRequired")return !o["partial"].toBool()&&o["counts"].isNull()&&o["shards"].isNull()&&issue["groupSize"].isNull();
    qint64 placed=0;if(!layoutCounts(o["counts"])||!shardRows(o["shards"],placed))return false;
    const auto counts=o["counts"].toObject();
    if(status=="ExceedsCapacity")return o["partial"].toBool()&&o["shards"].toArray().size()==10&&number(issue["groupSize"],99)&&issue["groupSize"].toInteger()>0&&placed>0&&placed<counts["uniqueFamilies"].toInteger()&&issue["groupSize"].toInteger()<=counts["uniqueFamilies"].toInteger()-placed&&o["shards"].toArray().last().toObject()["directoryCount"].toInteger()+issue["groupSize"].toInteger()>99;
    return !o["partial"].toBool()&&placed==counts["uniqueFamilies"].toInteger()&&(placed!=0||(counts["selectedReferences"]==QJsonValue(0)&&counts["physicalFiles"]==QJsonValue(0)&&counts["totalBytes"]==QJsonValue(0)));
}
}
namespace OptionData {
bool validPerformanceChanges(const QJsonValue &value) {
    if(value.isUndefined())return true;
    if(!value.isArray()||value.toArray().size()>4096)return false;
    for(const auto &v:value.toArray()) {
        const auto c=v.toObject();
        if(!v.isObject()||c.size()!=3||!text(c["name"],512,true)
            ||!(c["from"].isNull()||text(c["from"],4096))||!(c["to"].isNull()||text(c["to"],4096)))return false;
    }
    return true;
}
bool decodeApplyReview(const QJsonValue &v,QVariantMap &out) {
    auto o=v.toObject();
    // Mod repairs (fix for this patch) travel as four optional fields; check
    // them like the launch summary does, then validate the rest unchanged.
    QVariantMap repairInfo;
    qint64 repairFiles=0; // a built repair package is one extra physical file
    if(o.contains("repairs")) {
        const auto r=o.take("repairs");repairFiles=r.toObject().value("file").isObject()?1:0;const auto n=o.take("repairModCount");const auto symptoms=o.take("repairSymptoms");const auto migration=o.take("upstreamRepairMigration");
        if(!(r.isNull()||r.isObject())||!integer(n)||!symptoms.isArray()||!(migration.isNull()||migration==QJsonValue("replaceMountPreserveFolder")))return false;
        for(const auto &s:symptoms.toArray())if(!text(s,256,true))return false;
        repairInfo={{"repairModCount",n.toInteger()},{"repairSymptoms",symptoms.toVariant()},{"upstreamRepairMigration",migration.toVariant()}};
    }
    if(!v.isObject()||o.size()!=(o.contains("performance")?16:15)||o["summaryVersion"]!=QJsonValue(2)||o["deploymentLayout"]!=QJsonValue("shards-v1")||!hex(o["token"])||!id(o["profileId"])||!text(o["gamePath"],4096,true))return false;
    for(const auto *k:{"packageCount","directoryCount","physicalFileCount","companionFileCount","fileCount","totalBytes"})if(!integer(o[k]))return false;
    const auto dirs=o["directoryCount"].toInteger(),physical=o["physicalFileCount"].toInteger();qint64 placed=0;
    if(dirs>990||dirs>o["packageCount"].toInteger()||physical-repairFiles<dirs||physical-repairFiles-dirs!=o["companionFileCount"].toInteger()||o["fileCount"].toInteger()!=dirs||!shardRows(o["shards"],placed)||placed!=dirs||o["shardCount"]!=QJsonValue(o["shards"].toArray().size()))return false;
    if(dirs==0&&(o["packageCount"]!=QJsonValue(0)||physical!=0||o["totalBytes"]!=QJsonValue(0)))return false;
    if(!o["replacedMounts"].isArray()||!o["warnings"].isArray())return false;
    for(const auto &m:o["replacedMounts"].toArray())if(!text(m,4096,true))return false;
    for(const auto &w:o["warnings"].toArray()){auto row=w.toObject();if(!w.isObject()||row.size()!=2||!text(row["code"],256,true)||!text(row["message"],16384,true))return false;}
    if(o.contains("performance")) {
        auto p=o["performance"].toObject();
        if(!validPerformanceChanges(p.take("changes")))return false;
        if(!o["performance"].isObject()||p.size()!=8||!text(p["presetId"],256)||!(p["presetName"].isNull()||text(p["presetName"],512))
            ||!integer(p["entryCount"])||!integer(p["omittedCount"])||!p["detected"].isObject()
            ||!QStringList{"remove","replace","preserve"}.contains(p["overlayAction"].toString())||p["foreignPolicy"]!=QJsonValue("preserve-and-refuse")||p["restorePolicy"]!=QJsonValue("exact-prior-bytes"))return false;
    }
    out=o.toVariantMap();out.insert("copyBytesText",QLocale().toString(o["totalBytes"].toInteger()));
    for(auto it=repairInfo.cbegin();it!=repairInfo.cend();++it)out.insert(it.key(),it.value());
    return true;
}
bool optionsCall(BackendClient::Call c){using C=BackendClient::Call;switch(c){case C::OptionsGet:case C::OptionsPrepare:case C::OptionsStatus:case C::OptionsCancel:case C::OptionsDismiss:case C::OptionsReview:case C::OptionsAttach:return true;default:return false;}}
bool importCall(BackendClient::Call c){using C=BackendClient::Call;switch(c){case C::LocalPrepare:case C::LocalStatus:case C::LocalReview:case C::LocalCancel:case C::LocalCommit:case C::LocalDismiss:return true;default:return false;}}
bool localCall(BackendClient::Call c){return optionsCall(c)||importCall(c);}
bool decodeReply(const QJsonValue &v,QVariantMap &out){
    if(v.isArray()) {if(v.toArray().size()>8)return false;QVariantList rows;QSet<QString> seen;for(const auto &x:v.toArray()){QVariantMap row;if(!status(x,row)||row.contains("candidateSummary")||seen.contains(row.value("jobId").toString()))return false;seen.insert(row.value("jobId").toString());rows.append(row);}out={{"jobs",rows}};return true;}
    if(!v.isObject())return false;auto o=v.toObject();
    if(o.contains("detail"))return detail(o,out);
    if(o.value("kind")==QJsonValue("localOptions"))return status(v,out);
    if(o.size()==2&&hex(o["jobId"])&&o["dismissed"]==QJsonValue(true)){out=o.toVariantMap();return true;}
    auto s=o["summary"].toObject();
    if(!hex(o["token"])||!id(o["profileId"])||!decimal(o["expectedRevision"])||!o["summary"].isObject())return false;
    if(s.contains("attachedOptionCount")){
        if(o.size()!=5||!id(o["modId"])||s.size()!=5||!number(s["attachedOptionCount"],1024)||s["attachedOptionCount"].toInt()==0||!number(s["newPayloadBytes"])||!number(s["newArchiveBytes"])||!number(s["newWriteBytes"])||s["newWriteBytes"].toDouble()!=s["newPayloadBytes"].toDouble()+s["newArchiveBytes"].toDouble()||!totals(s["resultingTotals"]))return false;
    }else{
        const bool sharded=s["version"]==QJsonValue(2);
        if(o.size()!=4||s.size()!=11||(!sharded&&s["version"]!=QJsonValue(1))||!QStringList{"selection","order"}.contains(s["kind"].toString())||!s["hasChanges"].isBool()||!s["canCommit"].isBool()||s["canCommit"]!=s["hasChanges"]||!modIds(s["affectedEnabledModIds"])||s["payloadValidation"]!=QJsonValue("requiresApplyValidation")||!text(s["message"]))return false;
        for(const auto *k:{"before","after"}){auto b=s[k].toObject();if(!s[k].isObject()||b.size()!=2||!modIds(b["modIds"])||!selection(b["selection"])||(s["kind"]==QJsonValue("order")&&!b["selection"].isNull()))return false;}
        auto constraint=s.value("packageConstraint").toObject(),precedence=s["precedence"].toObject();
        if(precedence.size()!=2||precedence["status"]!=QJsonValue("requiresApplyReview")||!text(precedence["message"]))return false;
        if(sharded) {
            auto c=s["capacity"].toObject();if(s.contains("packageConstraint")||!s["capacity"].isObject()||c.size()!=6||c["deploymentLayout"]!=QJsonValue("shards-v1")||c["perModLimit"]!=QJsonValue(99)||c["perShardLimit"]!=QJsonValue(99)||c["maxShards"]!=QJsonValue(10)||!capacityOutcome(c["before"])||!capacityOutcome(c["after"]))return false;
            for(const auto *key:{"before","after"}) {
                const auto outcome=c[key].toObject();const auto ids=s[key].toObject()["modIds"].toArray();
                if(!outcome["issue"].isNull()&&!ids.contains(outcome["issue"].toObject()["modId"]))return false;
                if(!outcome["counts"].isNull()&&outcome["counts"].toObject()["selectedReferences"].toInteger()>qint64(ids.size())*99)return false;
            }
        }else{
            if(s.contains("capacity")||constraint.size()!=5||constraint["limit"]!=QJsonValue(99))return false;
            for(const auto *k:{"selectedBefore","selectedAfter"})if(!constraint[k].isNull()&&!number(constraint[k],1000000))return false;
            for(const auto *k:{"beforeIssue","afterIssue"})if(!constraint[k].isNull()&&!text(constraint[k],16384))return false;
        }
    }
    out=o.toVariantMap();
    if(s.contains("canCommit")) {
        if(s["version"]==QJsonValue(2))for(const auto *key:{"before","after"}) {
            const auto counts=s["capacity"].toObject()[key].toObject()["counts"];
            out.insert(QString::fromLatin1(key)=="before"?"capacityBeforeBytesText":"capacityAfterBytesText",counts.isNull()?QString():QLocale().toString(counts.toObject()["totalBytes"].toInteger()));
        }
        QVariantList comparison;const auto before=s["before"].toObject(),after=s["after"].toObject();
        if(s["kind"]==QJsonValue("order")) {
            const auto old=before["modIds"].toArray(),next=after["modIds"].toArray();
            if(old.size()!=next.size())return false;
            for(qsizetype i=0;i<next.size();++i)comparison.append(QVariantMap{{"position",i+1},{"before",old[i].toString()},{"after",next[i].toString()}});
        }else{
            QMap<QString,QString> old,next;for(const auto &v:before["selection"].toObject()["choices"].toArray()){auto c=v.toObject();old.insert(c["optionId"].toString(),c["optionName"].toString());}
            for(const auto &v:after["selection"].toObject()["choices"].toArray()){auto c=v.toObject();next.insert(c["optionId"].toString(),c["optionName"].toString());}
            QSet<QString> ids;for(auto it=old.cbegin();it!=old.cend();++it)ids.insert(it.key());for(auto it=next.cbegin();it!=next.cend();++it)ids.insert(it.key());auto sorted=ids.values();sorted.sort();
            for(const auto &id:sorted)comparison.append(QVariantMap{{"id",id},{"before",old.value(id)},{"after",next.value(id)}});
        }
        out.insert("comparisonRows",comparison);
    }
    return true;
}
}
