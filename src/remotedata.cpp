#include "remotedata.h"
#include "launchdata.h"
#include <QCryptographicHash>
#include <QJsonArray>
#include <QSet>
#include <algorithm>
#include <limits>

namespace RemoteData {
namespace {
using C = BackendClient::Call;
constexpr quint64 memberLimit = 16ULL * 1024 * 1024 * 1024;
constexpr quint64 cohortLimit = 64ULL * 1024 * 1024 * 1024;
constexpr quint64 payloadLimit = 128ULL * 1024 * 1024 * 1024;

bool keys(const QJsonObject &o, std::initializer_list<const char *> names) {
    return o.size() == qsizetype(names.size()) && std::all_of(names.begin(), names.end(),
        [&o](const char *name) { return o.contains(QLatin1String(name)); });
}

bool one(const QJsonValue &v, const char *values) {
    return v.isString() && QString::fromLatin1(values).split('|').contains(v.toString());
}
bool text(const QJsonValue &v, int max, bool nonempty = false) {
    if (!v.isString()) return false;
    const auto s = v.toString();
    if ((nonempty && s.isEmpty()) || s.toUtf8().size() > max) return false;
    for (qsizetype i = 0; i < s.size(); ++i) {
        const auto c = s[i];
        if (c.isHighSurrogate()) { if (++i >= s.size() || !s[i].isLowSurrogate()) return false; }
        else if (c.isLowSurrogate() || c.category() == QChar::Other_Control) return false;
    }
    return true;
}
bool name(const QJsonValue &v) {
    if (!text(v, 255, true)) return false;
    const auto s = v.toString();
    return s != "." && s != ".." && !s.contains('/') && !s.contains('\\');
}
bool message(const QJsonValue &v, int max = 512) {
    return text(v, max) && !v.toString().contains('/') && !v.toString().contains('\\');
}
bool dec(const QJsonValue &v, quint64 max = std::numeric_limits<quint64>::max(), bool positive = false) {
    return LaunchData::decimal(v, positive) && v.toString().toULongLong() <= max;
}
bool nullableDec(const QJsonValue &v, quint64 max = std::numeric_limits<quint64>::max()) {
    return v.isNull() || dec(v, max);
}
bool nullableHex(const QJsonValue &v, int n) { return v.isNull() || LaunchData::hex(v, n); }
bool page(const QJsonValue &v) {
    return v.isDouble() && v.toDouble() == double(v.toInteger(-1)) && v.toInteger(-1) >= 0
        && quint64(v.toInteger()) <= std::numeric_limits<quint32>::max();
}
bool error(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    return keys(o, {"kind", "message", "retryAfterSeconds"})
        && one(o["kind"], "eligibilityUnknown|providerDenied|providerNotFound|providerMalformed|providerUnavailable|needsSelection|hostNotAllowed|transportTls|transportNetwork|transportTimeout|transportFraming|transportChecksum|transportStorage|claimAmbiguous|storageFull|layoutUnsupported|archiveUnsupported|archiveRejected|archiveLimit|invalidSelection|staleRevision|staleReview|duplicateSubmission|launchBusy|recoveryRequired|outcomeUnknown|remoteBusy|controlReceiptExpired|jobIdConflict|cancelTooLate")
        && message(o["message"]) && nullableDec(o["retryAfterSeconds"], 86400);
}
bool optionalError(const QJsonValue &v) { return v.isNull() || error(v); }
bool file(const QJsonObject &o) {
    return dec(o["fileId"], std::numeric_limits<quint64>::max(), true) && name(o["originalName"])
        && nullableDec(o["declaredSize"]) && nullableDec(o["createdAt"]) && nullableHex(o["md5"], 32)
        && one(o["list"], "current|archived") && (o["archiveFlag"].isNull() || o["archiveFlag"].isBool());
}
bool receipt(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!keys(o, {"fileId", "originalName", "declaredSize", "createdAt", "md5", "list", "archiveFlag", "measuredSize", "sha256", "providerChecksum"})
        || !file(o) || !dec(o["measuredSize"], memberLimit, true) || !LaunchData::hex(o["sha256"], 64)
        || !one(o["providerChecksum"], "matched|absent") || (o["md5"].isNull() != (o["providerChecksum"] == QJsonValue("absent")))) return false;
    return o["declaredSize"].isNull() || o["declaredSize"] == QJsonValue("0") || o["declaredSize"] == o["measuredSize"];
}
bool resolved(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject(), fk = o["fileKey"].toObject();
    auto identity=o;identity.remove("description");
    const bool valid=(!o.contains("description") || text(o["description"],1024)) && keys(identity, {"fileKey", "submissionName", "originalName", "declaredSize", "createdAt", "md5", "list", "archiveFlag", "candidateFingerprint"})
        && o["fileKey"].isObject() && keys(fk, {"submission", "fileId"}) && submission(fk["submission"])
        && dec(fk["fileId"], std::numeric_limits<quint64>::max(), true) && text(o["submissionName"], 256, true)
        && name(o["originalName"]) && nullableDec(o["declaredSize"]) && nullableDec(o["createdAt"])
        && nullableHex(o["md5"], 32) && one(o["list"], "current|archived")
        && (o["archiveFlag"].isNull() || o["archiveFlag"].isBool()) && LaunchData::hex(o["candidateFingerprint"], 64);
    if(!valid)return false;
    const QJsonObject consent{{"fileId",fk["fileId"]},{"originalName",o["originalName"]},{"declaredSize",o["declaredSize"]},
        {"createdAt",o["createdAt"]},{"md5",o["md5"]},{"list",o["list"]},{"archiveFlag",o["archiveFlag"]}};
    const QJsonArray input{QStringLiteral("remoteConsent/v1"),fk["submission"],o["submissionName"],consent};
    const auto digest=QCryptographicHash::hash(LaunchData::canonical(input),QCryptographicHash::Sha256).toHex();
    return o["candidateFingerprint"]==QJsonValue(QString::fromLatin1(digest));
}
bool sortedHashes(const QJsonValue &v, int length) {
    if (!v.isArray() || v.toArray().isEmpty() || v.toArray().size() > 99) return false;
    QString previous;
    for (const auto &x : v.toArray()) {
        if (!LaunchData::hex(x, length) || (!previous.isEmpty() && previous >= x.toString())) return false;
        previous = x.toString();
    }
    return true;
}
bool metadata(const QJsonValue &v) {
    if(!v.isObject())return false;
    const auto o=v.toObject();
    if(!keys(o,{"name","author","category","description"}) || !text(o["name"],256,true)
        || !text(o["author"],256) || !text(o["category"],128) || !o["description"].isString())return false;
    const auto description=o["description"].toString();
    if(description.toUtf8().size()>16384)return false;
    for(qsizetype i=0;i<description.size();++i) {
        const auto c=description[i];
        if(c.isHighSurrogate()) {if(++i>=description.size() || !description[i].isLowSurrogate())return false;}
        else if(c.isLowSurrogate() || (c.category()==QChar::Other_Control && c!='\n' && c!='\t'))return false;
    }
    return true;
}
bool summary(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!keys(o, {"selectedCandidateIds", "selectedOptionIds", "readyCount", "selectedCount", "retainedCount", "uniquePayloadBytes", "retainedLogicalBytes", "sourceBytes", "diagnosticCount"})
        || !sortedHashes(o["selectedCandidateIds"], 64) || !sortedHashes(o["selectedOptionIds"], 32)
        || !dec(o["readyCount"], 1024) || !dec(o["selectedCount"], 99) || !dec(o["retainedCount"], 1024)
        || !dec(o["uniquePayloadBytes"], payloadLimit, true) || !dec(o["retainedLogicalBytes"], payloadLimit)
        || !dec(o["sourceBytes"], cohortLimit, true) || !dec(o["diagnosticCount"], 10000)) return false;
    const auto ready = o["readyCount"].toString().toULongLong(), selected = o["selectedCount"].toString().toULongLong();
    const auto retained = o["retainedCount"].toString().toULongLong();
    return selected == quint64(o["selectedCandidateIds"].toArray().size()) && selected == quint64(o["selectedOptionIds"].toArray().size())
        && ready >= selected && ready - selected == retained && ((retained == 0) == (o["retainedLogicalBytes"] == QJsonValue("0")));
}
bool review(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!keys(o, {"requestKey", "phase", "reviewToken", "selectionDigest", "modId", "summary", "error"})
        || !LaunchData::hex(o["requestKey"], 32) || !one(o["phase"], "pending|ready|failed")
        || !nullableHex(o["reviewToken"], 32) || !nullableHex(o["selectionDigest"], 64)
        || !(o["modId"].isNull() || LaunchData::id(o["modId"])) || !(o["summary"].isNull() || summary(o["summary"]))
        || !optionalError(o["error"])) return false;
    if (o["phase"] == QJsonValue("ready")) return !o["reviewToken"].isNull() && !o["selectionDigest"].isNull() && !o["modId"].isNull() && !o["summary"].isNull() && o["error"].isNull();
    return o["reviewToken"].isNull() && o["selectionDigest"].isNull() && o["modId"].isNull() && o["summary"].isNull()
        && ((o["phase"] == QJsonValue("failed")) != o["error"].isNull());
}
bool status(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!keys(o, {"version", "root", "sessionId", "jobId", "submission", "profileId", "profileNameAtStart", "initialRevision", "reviewRevision", "sequence", "state", "progress", "cohort", "prepareKey", "review", "operationId", "lastControl", "error", "cleanupPending"})
        || o["version"] != QJsonValue(1) || !LaunchData::root(o["root"]) || !LaunchData::hex(o["sessionId"], 32)
        || !LaunchData::hex(o["jobId"], 32) || !submission(o["submission"]) || !LaunchData::id(o["profileId"])
        || !text(o["profileNameAtStart"], 256, true) || !dec(o["initialRevision"]) || !nullableDec(o["reviewRevision"])
        || !dec(o["sequence"]) || !one(o["state"], "resolving|transferring|discoveringFamily|sealing|downloaded|preparing|ready|reviewing|committing|installed|paused|failed|needsSelection|cancelRequested|cancelled|abandoned")
        || !nullableHex(o["prepareKey"], 32) || !(o["review"].isNull() || review(o["review"]))
        || !(o["operationId"].isNull() || LaunchData::id(o["operationId"])) || !optionalError(o["error"]) || !o["cleanupPending"].isBool()) return false;
    if (!o["progress"].isNull()) {
        const auto p = o["progress"].toObject();
        if (!o["progress"].isObject() || !keys(p, {"epoch", "volatileSequence", "bytesWritten", "durableOffset", "total"})
            || !LaunchData::hex(p["epoch"], 32) || !dec(p["volatileSequence"]) || !dec(p["bytesWritten"], memberLimit)
            || !dec(p["durableOffset"], memberLimit) || !nullableDec(p["total"], memberLimit)) return false;
        const auto written = p["bytesWritten"].toString().toULongLong(), durable = p["durableOffset"].toString().toULongLong();
        if (durable > written || (!p["total"].isNull() && written > p["total"].toString().toULongLong())) return false;
    }
    const auto c = o["cohort"].toObject();
    if (!o["cohort"].isObject() || !keys(c, {"fingerprint", "complete", "members"}) || !nullableHex(c["fingerprint"], 64)
        || !c["complete"].isBool() || !c["members"].isArray() || c["members"].toArray().size() > 64
        || (c["complete"].toBool() && (c["fingerprint"].isNull() || c["members"].toArray().isEmpty()))) return false;
    QSet<QString> ids; quint64 observed = 0, previousCompanion = 0;
    QJsonArray consentFiles;bool haveReceipts=true;int position=0;
    for (const auto &v : c["members"].toArray()) {
        const auto m = v.toObject();
        if (!v.isObject() || !keys(m, {"fileId", "originalName", "state", "durableOffset", "total", "receipt"})
            || !dec(m["fileId"], std::numeric_limits<quint64>::max(), true) || ids.contains(m["fileId"].toString())
            || !name(m["originalName"]) || !text(m["state"], 32, true) || !dec(m["durableOffset"], memberLimit)
            || !nullableDec(m["total"], memberLimit) || !(m["receipt"].isNull() || receipt(m["receipt"]))) return false;
        ids.insert(m["fileId"].toString());
        const auto numericId=m["fileId"].toString().toULongLong();
        if(position>1 && numericId<=previousCompanion)return false;
        if(position>0)previousCompanion=numericId;
        ++position;
        const auto durable = m["durableOffset"].toString().toULongLong();
        if (!m["total"].isNull() && durable > m["total"].toString().toULongLong()) return false;
        if (!m["receipt"].isNull()) {
            const auto r = m["receipt"].toObject();
            if (r["fileId"] != m["fileId"] || r["originalName"] != m["originalName"] || r["measuredSize"] != m["durableOffset"] || r["measuredSize"] != m["total"]) return false;
            consentFiles.append(QJsonObject{{"fileId",r["fileId"]},{"originalName",r["originalName"]},{"declaredSize",r["declaredSize"]},
                {"createdAt",r["createdAt"]},{"md5",r["md5"]},{"list",r["list"]},{"archiveFlag",r["archiveFlag"]}});
        } else haveReceipts=false;
        if (durable > cohortLimit - observed) return false;
        observed += durable;
    }
    if(c["complete"].toBool() && haveReceipts && !consentFiles.isEmpty()) {
        const auto first=c["members"].toArray().first().toObject()["fileId"];
        const QJsonArray input{QStringLiteral("remoteCohort/v1"),o["submission"],first,consentFiles};
        const auto digest=QCryptographicHash::hash(LaunchData::canonical(input),QCryptographicHash::Sha256).toHex();
        if(c["fingerprint"]!=QJsonValue(QString::fromLatin1(digest)))return false;
    }
    if (!o["lastControl"].isNull()) {
        const auto control = o["lastControl"].toObject();
        if (!o["lastControl"].isObject() || !keys(control, {"controlSeq", "action", "result"}) || !dec(control["controlSeq"])
            || !one(control["action"], "pause|resume|retry|cancel|abandon|dismiss") || !one(control["result"], "accepted|rejected")) return false;
    }
    return true;
}
bool candidate(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    if (!keys(o, {"candidateId", "optionId", "optionName", "sourceLabel", "available", "unavailableReason", "memberCount", "logicalBytes"})
        || !LaunchData::hex(o["candidateId"], 64) || !nullableHex(o["optionId"], 32) || !text(o["optionName"], 1024)
        || !name(o["sourceLabel"]) || !o["available"].isBool() || !(o["unavailableReason"].isNull() || message(o["unavailableReason"]))
        || !dec(o["memberCount"], 10000) || !nullableDec(o["logicalBytes"], payloadLimit)) return false;
    if (o["available"].toBool()) return !o["optionId"].isNull() && !o["logicalBytes"].isNull() && o["logicalBytes"] != QJsonValue("0") && o["memberCount"] != QJsonValue("0") && o["unavailableReason"].isNull();
    return o["optionId"].isNull() && o["logicalBytes"].isNull() && !o["unavailableReason"].isNull();
}
bool member(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    return keys(o, {"name", "role", "index", "measuredSize"}) && name(o["name"]) && one(o["role"], "directory|companion")
        && nullableDec(o["index"]) && dec(o["measuredSize"], memberLimit, true) && ((o["role"] == QJsonValue("directory")) == o["index"].isNull());
}
bool diagnostic(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    return keys(o, {"kind", "candidateId", "count", "message"}) && one(o["kind"], "excludedNonVpk|nestedArchive|incompleteFamily|invalidFamily|unsupportedType")
        && nullableHex(o["candidateId"], 64) && dec(o["count"], 10000) && message(o["message"]);
}
bool rows(const QJsonValue &v, bool (*check)(const QJsonValue &)) {
    if (!v.isArray() || v.toArray().size() > 8) return false;
    for (const auto &x : v.toArray()) if (!check(x)) return false;
    return true;
}
bool replacementMod(const QJsonValue &v) {
    return LaunchData::id(v);
}
bool purpose(const QJsonValue &v) { return one(v,"update|force|reinstall|link|append"); }
bool skipEntry(const QJsonValue &v, bool desired=false) {
    if(!v.isObject())return false;
    const auto o=v.toObject();
    return (desired ? keys(o,{"fileId","candidateFingerprint"}) : keys(o,{"targetDigest","fileId","candidateFingerprint"}))
        && (desired || LaunchData::hex(o["targetDigest"],64))
        && dec(o["fileId"],std::numeric_limits<quint64>::max(),true) && LaunchData::hex(o["candidateFingerprint"],64);
}
bool replacementTarget(const QJsonValue &v, const QJsonObject &request) {
    if(!v.isObject())return false;
    const auto o=v.toObject(), r=o["receipt"].toObject();
    if(!keys(o,{"profileId","modId","revision","recordDigest","receipt","enabled","previewHidden","available","installOrder","metadata","optionCount","selectedOptionIds"})
        || o["profileId"]!=request["profileId"] || o["modId"]!=request["modId"] || !replacementMod(o["modId"]) || o["revision"]!=request["expectedRevision"]
        || !LaunchData::hex(o["recordDigest"],64) || !(o["receipt"].isNull() || (o["receipt"].isObject()
        && keys(r,{"version","submission","rootFileId","cohortFingerprint"}) && r["version"]==QJsonValue(1)
        && submission(r["submission"]) && dec(r["rootFileId"],std::numeric_limits<quint64>::max(),true)
        && LaunchData::hex(r["cohortFingerprint"],64))) || !o["enabled"].isBool() || !o["previewHidden"].isBool()
        || !o["available"].isBool() || !o["installOrder"].isDouble()
        || QJsonValue(o["installOrder"].toInteger(std::numeric_limits<qint64>::min()))!=o["installOrder"]
        || !metadata(o["metadata"]) || !dec(o["optionCount"],1024,true) || !o["selectedOptionIds"].isArray()
        || o["selectedOptionIds"].toArray().size()>99 || quint64(o["selectedOptionIds"].toArray().size())>o["optionCount"].toString().toULongLong())return false;
    QString previous;
    for(const auto &id:o["selectedOptionIds"].toArray()) {
        if(!LaunchData::hex(id,32) || (!previous.isEmpty() && previous>=id.toString()))return false;
        previous=id.toString();
    }
    return true;
}
bool replacementSkip(const QJsonValue &v) {
    if(!v.isObject())return false;
    const auto o=v.toObject();
    return keys(o,{"state","entry"}) && one(o["state"],"none|present|stale|unavailable")
        && ((o["state"]==QJsonValue("none") || o["state"]==QJsonValue("unavailable")) ? o["entry"].isNull() : skipEntry(o["entry"]));
}
bool replacementReview(const QJsonValue &v) {
    if(!v.isObject())return false;
    const auto o=v.toObject();
    if(!keys(o,{"requestKey","phase","reviewToken","selectionDigest","afterRecordDigest","summary","error"})
        || !LaunchData::hex(o["requestKey"],32) || !one(o["phase"],"pending|ready|failed")
        || !nullableHex(o["reviewToken"],32) || !nullableHex(o["selectionDigest"],64) || !nullableHex(o["afterRecordDigest"],64)
        || !(o["summary"].isNull() || summary(o["summary"])) || !optionalError(o["error"]))return false;
    if(o["phase"]==QJsonValue("ready"))return !o["reviewToken"].isNull() && !o["selectionDigest"].isNull()
        && !o["afterRecordDigest"].isNull() && !o["summary"].isNull() && o["error"].isNull();
    return o["reviewToken"].isNull() && o["selectionDigest"].isNull() && o["afterRecordDigest"].isNull() && o["summary"].isNull()
        && ((o["phase"]==QJsonValue("failed")) != o["error"].isNull());
}
}

bool recoveryInspection(const QJsonValue &value,const QJsonObject &request,QVariantMap &out) {
    if(!keys(request,{"version","dataDir"}) || request["version"]!=QJsonValue(1)
        || !LaunchData::root(request["dataDir"]) || !value.isObject())return false;
    const auto o=value.toObject();
    if(!keys(o,{"version","root","state","expectedRevision","targetOperationId","targetInputSha256","existingRecovery","error"})
        || o["version"]!=QJsonValue(1) || o["root"]!=request["dataDir"])return false;
    const auto state=o["state"].toString();
    if(state=="ready") {
        if(!dec(o["expectedRevision"]) || !LaunchData::id(o["targetOperationId"]) || !LaunchData::hex(o["targetInputSha256"],64) || !o["error"].isNull())return false;
        if(!o["existingRecovery"].isNull()) {
            const auto input=o["existingRecovery"].toObject(), params=input["params"].toObject();
            if(!o["existingRecovery"].isObject() || !keys(input,{"method","params"}) || input["method"]!=QJsonValue("native.recover")
                || !input["params"].isObject() || !keys(params,{"operationId","expectedRevision"})
                || !LaunchData::id(params["operationId"]) || params["operationId"]==o["targetOperationId"]
                || params["expectedRevision"]!=o["expectedRevision"])return false;
        }
    } else if(state=="notRemote" || state=="waiting") {
        if(!o["expectedRevision"].isNull() || !o["targetOperationId"].isNull() || !o["targetInputSha256"].isNull() || !o["existingRecovery"].isNull()
            || (state=="notRemote" ? !o["error"].isNull() : !error(o["error"])))return false;
    } else if(state=="legacyImportRecovery" || state=="legacyUnimported") {
        if(o["expectedRevision"]!=QJsonValue("0") || !o["targetOperationId"].isNull() || !o["targetInputSha256"].isNull()
            || !o["existingRecovery"].isNull() || !o["error"].isNull())return false;
    } else return false;
    out=o.toVariantMap();return true;
}
bool call(C c) { return (int(c) >= int(C::RemoteOpen) && int(c) <= int(C::RemotePrepareRelease))
    || (int(c) >= int(C::RemoteReplaceTarget) && int(c) <= int(C::RemoteReplaceCommit)); }
QString method(C c) {
    switch (c) {
    case C::RemoteOpen:return QStringLiteral("native.remote.open");
    case C::RemoteResolve:return QStringLiteral("native.remote.resolve");
    case C::RemoteResolveStatus:return QStringLiteral("native.remote.resolveStatus");
    case C::RemoteResolveRelease:return QStringLiteral("native.remote.resolveRelease");
    case C::RemoteStart:return QStringLiteral("native.remote.start");
    case C::RemoteList:return QStringLiteral("native.remote.list");
    case C::RemoteStatus:return QStringLiteral("native.remote.status");
    case C::RemoteControl:return QStringLiteral("native.remote.control");
    case C::RemotePrepare:return QStringLiteral("native.remote.prepare");
    case C::RemoteCandidates:return QStringLiteral("native.remote.candidates");
    case C::RemoteCandidateMembers:return QStringLiteral("native.remote.candidateMembers");
    case C::RemoteDiagnostics:return QStringLiteral("native.remote.diagnostics");
    case C::RemoteReview:return QStringLiteral("native.remote.review");
    case C::RemoteCommit:return QStringLiteral("native.remote.commit");
    case C::RemoteClose:return QStringLiteral("native.remote.close");
    case C::RemotePrepareRelease:return QStringLiteral("native.remote.prepareRelease");
    case C::RemoteReplaceTarget:return QStringLiteral("native.remote.replaceTarget");
    case C::RemoteReplaceSkip:return QStringLiteral("native.remote.replaceSkip");
    case C::RemoteReplaceStart:return QStringLiteral("native.remote.replaceStart");
    case C::RemoteReplaceList:return QStringLiteral("native.remote.replaceList");
    case C::RemoteReplaceStatus:return QStringLiteral("native.remote.replaceStatus");
    case C::RemoteReplaceReview:return QStringLiteral("native.remote.replaceReview");
    case C::RemoteReplaceCommit:return QStringLiteral("native.remote.replaceCommit");
    default:return {};
    }
}
bool submission(const QJsonValue &v) {
    if (!v.isObject()) return false;
    const auto o = v.toObject();
    return keys(o, {"provider", "submissionType", "submissionId"}) && o["provider"] == QJsonValue("gamebanana")
        && one(o["submissionType"], "mod|sound|wip") && dec(o["submissionId"], std::numeric_limits<quint64>::max(), true);
}
bool request(C c,const QJsonObject &p) {
    if(!call(c) || p["version"]!=QJsonValue(1) || !LaunchData::root(p["dataDir"]) || !LaunchData::hex(p["sessionId"],32))return false;
    const auto job=[&]{return LaunchData::hex(p["jobId"],32);};
    const auto key=[&]{return LaunchData::hex(p["requestKey"],32);};
    const auto prepare=[&]{return LaunchData::hex(p["prepareKey"],32);};
    const auto profile=[&]{return LaunchData::id(p["profileId"]) && dec(p["expectedRevision"]);};
    switch(c) {
    case C::RemoteOpen:case C::RemoteList:case C::RemoteClose:
        return keys(p,{"version","dataDir","sessionId"});
    case C::RemoteResolve:
        return keys(p,{"version","dataDir","sessionId","requestKey","submission"}) && key() && submission(p["submission"]);
    case C::RemoteResolveStatus:
        return keys(p,{"version","dataDir","sessionId","requestKey","page"}) && key() && page(p["page"]) && p["page"].toInteger()<16;
    case C::RemoteResolveRelease:
        return keys(p,{"version","dataDir","sessionId","requestKey"}) && key();
    case C::RemoteStart:
        return keys(p,{"version","dataDir","sessionId","jobId","selectionToken","candidateFingerprint","fileId","profileId","expectedRevision","acceptArchived"})
            && job() && LaunchData::hex(p["selectionToken"],32) && LaunchData::hex(p["candidateFingerprint"],64)
            && dec(p["fileId"],std::numeric_limits<quint64>::max(),true) && profile() && p["acceptArchived"].isBool();
    case C::RemoteStatus:
        return keys(p,{"version","dataDir","sessionId","jobId"}) && job();
    case C::RemoteControl:
        return keys(p,{"version","dataDir","sessionId","jobId","controlSeq","action"}) && job() && dec(p["controlSeq"])
            && one(p["action"],"pause|resume|retry|cancel|abandon|dismiss");
    case C::RemotePrepare:
        return keys(p,{"version","dataDir","sessionId","jobId","profileId","expectedRevision","acknowledgeRebase","prepareKey"})
            && job() && profile() && p["acknowledgeRebase"].isBool() && prepare();
    case C::RemoteCandidates:case C::RemoteDiagnostics:
        return keys(p,{"version","dataDir","sessionId","jobId","prepareKey","page"}) && job() && prepare() && page(p["page"])
            && p["page"].toInteger()<(c==C::RemoteCandidates?128:1250);
    case C::RemoteCandidateMembers:
        return keys(p,{"version","dataDir","sessionId","jobId","prepareKey","candidateId","page"}) && job() && prepare()
            && LaunchData::hex(p["candidateId"],64) && page(p["page"]) && p["page"].toInteger()<1250;
    case C::RemoteReview:
        return keys(p,{"version","dataDir","sessionId","jobId","profileId","expectedRevision","requestKey","candidateIds","metadata"})
            && job() && profile() && key() && sortedHashes(p["candidateIds"],64) && metadata(p["metadata"]);
    case C::RemoteCommit:
        return keys(p,{"version","dataDir","sessionId","jobId","profileId","expectedRevision","reviewToken","operationId","consentFingerprint","cohortFingerprint","selectionDigest"})
            && job() && profile() && nullableHex(p["reviewToken"],32) && LaunchData::id(p["operationId"])
            && LaunchData::hex(p["consentFingerprint"],64) && LaunchData::hex(p["cohortFingerprint"],64) && LaunchData::hex(p["selectionDigest"],64);
    case C::RemotePrepareRelease:
        return keys(p,{"version","dataDir","sessionId","jobId","prepareKey"}) && job() && prepare();
    case C::RemoteReplaceTarget:
        return keys(p,{"version","dataDir","sessionId","requestKey","profileId","modId","expectedRevision"})
            && key() && profile() && LaunchData::id(p["modId"]);
    case C::RemoteReplaceSkip:
        return keys(p,{"version","dataDir","sessionId","requestKey","profileId","modId","expectedRevision","targetDigest","expectedSkip","desiredSkip","selectionToken"})
            && key() && profile() && replacementMod(p["modId"]) && LaunchData::hex(p["targetDigest"],64)
            && (p["expectedSkip"].isNull() || skipEntry(p["expectedSkip"]))
            && (p["desiredSkip"].isNull() ? p["selectionToken"].isNull() : skipEntry(p["desiredSkip"],true) && LaunchData::hex(p["selectionToken"],32));
    case C::RemoteReplaceStart:
        return keys(p,{"version","dataDir","sessionId","jobId","selectionToken","candidateFingerprint","fileId","profileId","modId","expectedRevision","targetDigest","purpose","acceptArchived"})
            && job() && profile() && replacementMod(p["modId"]) && LaunchData::hex(p["selectionToken"],32)
            && LaunchData::hex(p["candidateFingerprint"],64) && dec(p["fileId"],std::numeric_limits<quint64>::max(),true)
            && LaunchData::hex(p["targetDigest"],64) && purpose(p["purpose"]) && p["acceptArchived"].isBool();
    case C::RemoteReplaceList:
        return keys(p,{"version","dataDir","sessionId"});
    case C::RemoteReplaceStatus:
        return keys(p,{"version","dataDir","sessionId","jobId"}) && job();
    case C::RemoteReplaceReview:
        return keys(p,{"version","dataDir","sessionId","jobId","profileId","modId","expectedRevision","targetDigest","requestKey","candidateIds"})
            && job() && profile() && replacementMod(p["modId"]) && LaunchData::hex(p["targetDigest"],64) && key() && sortedHashes(p["candidateIds"],64);
    case C::RemoteReplaceCommit:
        return keys(p,{"version","dataDir","sessionId","jobId","profileId","modId","purpose","initialRevision","expectedRevision","targetDigest","afterRecordDigest","reviewToken","operationId","consentFingerprint","cohortFingerprint","selectionDigest"})
            && job() && profile() && replacementMod(p["modId"]) && purpose(p["purpose"]) && dec(p["initialRevision"])
            && p["initialRevision"].toString().toULongLong()<=p["expectedRevision"].toString().toULongLong()
            && LaunchData::hex(p["targetDigest"],64) && LaunchData::hex(p["afterRecordDigest"],64) && nullableHex(p["reviewToken"],32)
            && LaunchData::id(p["operationId"]) && LaunchData::hex(p["consentFingerprint"],64)
            && LaunchData::hex(p["cohortFingerprint"],64) && LaunchData::hex(p["selectionDigest"],64);
    default:return false;
    }
}
bool marker(const QJsonObject &o, const QString &root) {
    auto x = o; const auto recovery = x.take("recoveryId"), recoveryRevision=x.take("recoveryExpectedRevision");
    const auto version=x["remoteMarkerVersion"];
    const bool pair=version==QJsonValue(2) ? ((recovery.isUndefined() && recoveryRevision.isUndefined())
        || (LaunchData::id(recovery) && dec(recoveryRevision)))
        : version==QJsonValue(1) && recoveryRevision.isUndefined() && (recovery.isUndefined() || LaunchData::id(recovery));
    return pair && (recovery.isUndefined() || recovery!=x["operationId"])
        && keys(x, {"remoteMarkerVersion", "method", "root", "profileId", "jobId", "expectedRevision", "operationId", "modId", "consentFingerprint", "cohortFingerprint", "selectionDigest"})
        && x["method"] == QJsonValue(int(C::RemoteCommit))
        && x["root"] == QJsonValue(root) && LaunchData::root(x["root"]) && LaunchData::id(x["profileId"])
        && LaunchData::hex(x["jobId"], 32) && dec(x["expectedRevision"]) && LaunchData::id(x["operationId"])
        && LaunchData::id(x["modId"]) && LaunchData::hex(x["consentFingerprint"], 64)
        && LaunchData::hex(x["cohortFingerprint"], 64) && LaunchData::hex(x["selectionDigest"], 64);
}
bool replacementMarker(const QJsonObject &o,const QString &root) {
    auto x=o;const auto recovery=x.take("recoveryId"), revision=x.take("recoveryExpectedRevision");
    const bool pair=(recovery.isUndefined() && revision.isUndefined()) || (LaunchData::id(recovery) && dec(revision));
    return pair && (recovery.isUndefined() || recovery!=x["operationId"])
        && keys(x,{"remoteReplacementMarkerVersion","method","root","jobId","purpose","profileId","modId","initialRevision","expectedRevision","targetDigest","afterRecordDigest","operationId","consentFingerprint","cohortFingerprint","selectionDigest"})
        && x["remoteReplacementMarkerVersion"]==QJsonValue(1) && x["method"]==QJsonValue(int(C::RemoteReplaceCommit))
        && x["root"]==QJsonValue(root) && LaunchData::root(x["root"]) && LaunchData::hex(x["jobId"],32)
        && purpose(x["purpose"]) && LaunchData::id(x["profileId"]) && replacementMod(x["modId"])
        && dec(x["initialRevision"]) && dec(x["expectedRevision"]) && x["initialRevision"].toString().toULongLong()<=x["expectedRevision"].toString().toULongLong() && LaunchData::hex(x["targetDigest"],64)
        && LaunchData::hex(x["afterRecordDigest"],64) && LaunchData::id(x["operationId"])
        && LaunchData::hex(x["consentFingerprint"],64) && LaunchData::hex(x["cohortFingerprint"],64) && LaunchData::hex(x["selectionDigest"],64);
}
QJsonObject replacementInput(const QJsonObject &marker) {
    QJsonObject params;
    for(const auto *key:{"operationId","jobId","profileId","modId","purpose","initialRevision","expectedRevision","targetDigest","afterRecordDigest","consentFingerprint","cohortFingerprint","selectionDigest"})
        params.insert(QLatin1String(key),marker.value(QLatin1String(key)));
    return {{"method",QStringLiteral("native.remote.replaceCommit")},{"params",params}};
}
bool decode(C c, const QJsonValue &v, const QJsonObject &request, RemoteReply &out) {
    if (!RemoteData::request(c,request) || !v.isObject()) return false;
    const auto o = v.toObject();
    if (o["version"] != QJsonValue(1) || o["root"] != request["dataDir"] || o["sessionId"] != request["sessionId"]) return false;
    for (const auto *key : {"jobId", "requestKey", "prepareKey", "candidateId", "operationId", "page", "controlSeq"})
        if (request.contains(QLatin1String(key)) && o.contains(QLatin1String(key)) && request[key] != o[key]) return false;
    bool ok = false;
    switch (c) {
    case C::RemoteOpen:case C::RemoteClose:
        ok = keys(o, {"version", "root", "sessionId", "phase", "error"}) && optionalError(o["error"])
            && one(o["phase"], c == C::RemoteOpen ? "opening|ready|waiting|failed" : "closing|waiting|closed");
        if(ok && c==C::RemoteOpen)ok=(o["phase"]==QJsonValue("failed"))!=o["error"].isNull();
        break;
    case C::RemoteResolve:case C::RemoteStart:case C::RemoteReview:
        ok = c == C::RemoteResolve ? keys(o, {"version", "root", "sessionId", "requestKey", "state"}) && LaunchData::hex(o["requestKey"], 32)
            : c == C::RemoteStart ? keys(o, {"version", "root", "sessionId", "jobId", "state"}) && LaunchData::hex(o["jobId"], 32)
            : keys(o, {"version", "root", "sessionId", "jobId", "requestKey", "state"}) && LaunchData::hex(o["jobId"], 32) && LaunchData::hex(o["requestKey"], 32);
        ok = ok && o["state"] == QJsonValue("pending"); break;
    case C::RemoteResolveStatus:
        ok = keys(o, {"version", "root", "sessionId", "requestKey", "phase", "page", "hasMore", "selectionToken", "candidates", "error"})
            && LaunchData::hex(o["requestKey"], 32) && one(o["phase"], "pending|ready|failed") && page(o["page"]) && o["hasMore"].isBool()
            && nullableHex(o["selectionToken"], 32) && rows(o["candidates"], resolved) && optionalError(o["error"]);
        if (ok && o["phase"] == QJsonValue("ready")) ok = !o["selectionToken"].isNull() && o["error"].isNull();
        else if (ok) ok = o["selectionToken"].isNull() && o["candidates"].toArray().isEmpty() && !o["hasMore"].toBool()
            && ((o["phase"] == QJsonValue("failed")) != o["error"].isNull());
        break;
    case C::RemoteResolveRelease:
        ok = keys(o, {"version", "root", "sessionId", "requestKey", "released"}) && LaunchData::hex(o["requestKey"], 32) && o["released"].isBool(); break;
    case C::RemoteList:
        ok = keys(o, {"version", "root", "sessionId", "jobs"}) && o["jobs"].isArray() && o["jobs"].toArray().size() <= 8;
        if (ok) for (const auto &v : o["jobs"].toArray()) { const auto j = v.toObject();
            if (!v.isObject() || !keys(j, {"jobId", "submission", "profileId", "state", "sequence", "cleanupPending"})
                || !LaunchData::hex(j["jobId"], 32) || !submission(j["submission"]) || !LaunchData::id(j["profileId"])
                || !one(j["state"], "resolving|transferring|discoveringFamily|sealing|downloaded|preparing|ready|reviewing|committing|installed|paused|failed|needsSelection|cancelRequested|cancelled|abandoned")
                || !dec(j["sequence"]) || !j["cleanupPending"].isBool()) { ok = false; break; } }
        break;
    case C::RemoteStatus: ok = status(v); break;
    case C::RemoteControl:
        ok = keys(o, {"version", "root", "sessionId", "jobId", "controlSeq", "state"}) && LaunchData::hex(o["jobId"], 32)
            && dec(o["controlSeq"]) && one(o["state"], "accepted|rejected"); break;
    case C::RemotePrepare:case C::RemotePrepareRelease:
        ok = keys(o, {"version", "root", "sessionId", "jobId", "prepareKey", "state"}) && LaunchData::hex(o["jobId"], 32)
            && LaunchData::hex(o["prepareKey"], 32) && one(o["state"], c == C::RemotePrepare ? "pending" : "pending|released|stale"); break;
    case C::RemoteCandidates:case C::RemoteCandidateMembers:case C::RemoteDiagnostics:
        ok = c == C::RemoteCandidates ? keys(o, {"version", "root", "sessionId", "jobId", "prepareKey", "page", "hasMore", "candidates"}) && rows(o["candidates"], candidate)
            : c == C::RemoteCandidateMembers ? keys(o, {"version", "root", "sessionId", "jobId", "prepareKey", "candidateId", "page", "hasMore", "members"}) && LaunchData::hex(o["candidateId"], 64) && rows(o["members"], member)
            : keys(o, {"version", "root", "sessionId", "jobId", "prepareKey", "page", "hasMore", "diagnostics"}) && rows(o["diagnostics"], diagnostic);
        ok = ok && LaunchData::hex(o["jobId"], 32) && LaunchData::hex(o["prepareKey"], 32) && page(o["page"]) && o["hasMore"].isBool(); break;
    case C::RemoteCommit:
        ok = keys(o, {"version", "root", "sessionId", "jobId", "operationId", "state"}) && LaunchData::hex(o["jobId"], 32)
            && LaunchData::id(o["operationId"]) && o["state"] == QJsonValue("pending"); break;
    case C::RemoteReplaceTarget:
        ok=keys(o,{"version","root","sessionId","requestKey","phase","target","skip","error"})
            && o["requestKey"]==request["requestKey"] && one(o["phase"],"pending|ready|failed");
        if(ok && o["phase"]==QJsonValue("ready")) {
            ok=replacementTarget(o["target"],request) && replacementSkip(o["skip"]) && o["error"].isNull();
            if(ok) {const auto skip=o["skip"].toObject();const auto state=skip["state"].toString();
                if(state=="present" || state=="stale")ok=(skip["entry"].toObject()["targetDigest"]==o["target"].toObject()["recordDigest"])==(state=="present");}
        }
        else if(ok)ok=o["target"].isNull() && o["skip"].isNull()
            && (o["phase"]==QJsonValue("pending") ? o["error"].isNull() : error(o["error"]));
        break;
    case C::RemoteReplaceSkip:
        ok=keys(o,{"version","root","sessionId","requestKey","phase","entry","error"})
            && o["requestKey"]==request["requestKey"] && one(o["phase"],"pending|ready|failed");
        if(ok && o["phase"]==QJsonValue("ready")) {
            ok=o["error"].isNull() && (o["entry"].isNull() || skipEntry(o["entry"]));
            if(ok && request["desiredSkip"].isNull())ok=o["entry"].isNull();
            else if(ok) {const auto e=o["entry"].toObject(), desired=request["desiredSkip"].toObject();
                ok=skipEntry(o["entry"]) && e["targetDigest"]==request["targetDigest"] && e["fileId"]==desired["fileId"] && e["candidateFingerprint"]==desired["candidateFingerprint"];}
        } else if(ok)ok=o["entry"].isNull() && (o["phase"]==QJsonValue("pending") ? o["error"].isNull() : error(o["error"]));
        break;
    case C::RemoteReplaceStart:
        ok=keys(o,{"version","root","sessionId","jobId","state"}) && o["jobId"]==request["jobId"] && o["state"]==QJsonValue("pending");break;
    case C::RemoteReplaceReview:
        ok=keys(o,{"version","root","sessionId","jobId","requestKey","state"}) && o["jobId"]==request["jobId"]
            && o["requestKey"]==request["requestKey"] && o["state"]==QJsonValue("pending");break;
    case C::RemoteReplaceCommit:
        ok=keys(o,{"version","root","sessionId","jobId","operationId","state"}) && o["jobId"]==request["jobId"]
            && o["operationId"]==request["operationId"] && o["state"]==QJsonValue("pending");break;
    case C::RemoteReplaceList:
        ok=keys(o,{"version","root","sessionId","jobs"}) && o["jobs"].isArray() && o["jobs"].toArray().size()<=8;
        if(ok)for(const auto &v:o["jobs"].toArray()) {const auto j=v.toObject();
            if(!v.isObject() || !keys(j,{"jobId","submission","profileId","state","sequence","cleanupPending","purpose","modId","targetDigest"})
                || !LaunchData::hex(j["jobId"],32) || !submission(j["submission"]) || !LaunchData::id(j["profileId"])
                || !one(j["state"],"resolving|transferring|discoveringFamily|sealing|downloaded|preparing|ready|reviewing|committing|installed|paused|failed|needsSelection|cancelRequested|cancelled|abandoned")
                || !dec(j["sequence"]) || !j["cleanupPending"].isBool() || !purpose(j["purpose"])
                || !replacementMod(j["modId"]) || !LaunchData::hex(j["targetDigest"],64)) {ok=false;break;}}
        break;
    case C::RemoteReplaceStatus: {
        const auto s=o["status"].toObject();
        ok=keys(o,{"version","root","sessionId","purpose","modId","targetDigest","status","review"})
            && purpose(o["purpose"]) && replacementMod(o["modId"]) && LaunchData::hex(o["targetDigest"],64)
            && status(o["status"]) && s["root"]==o["root"] && s["sessionId"]==o["sessionId"] && s["jobId"]==request["jobId"]
            && s["review"].isNull() && (o["review"].isNull() || replacementReview(o["review"]));break;
    }
    default: break;
    }
    if (ok) out.value = o.toVariantMap();
    return ok;
}
}
