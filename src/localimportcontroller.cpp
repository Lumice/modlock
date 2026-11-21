#include "localimportcontroller.h"
#include "librarymodel.h"
#include "localartworkprovider.h"
#include "fontdata.h"
#include "optiondata.h"
#include <QDir>
#include <QJsonArray>
#include <QJsonDocument>
#include <QSet>
#include <QUuid>
#include <algorithm>
#include <cmath>
namespace {
bool str(const QJsonValue &v, int bound, bool nonempty = false) {
    return v.isString() && (!nonempty || !v.toString().isEmpty()) && v.toString().toUtf8().size() <= bound;
}
bool hex(const QJsonValue &v, int size) {
    const auto s = v.toString();
    return v.isString() && s.size() == size &&
           std::all_of(s.cbegin(), s.cend(), [](QChar c) { return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'); });
}
bool number(const QJsonValue &v, double bound = 68719476736.) {
    return v.isDouble() && std::isfinite(v.toDouble()) && v.toDouble() >= 0 && v.toDouble() <= bound &&
           std::floor(v.toDouble()) == v.toDouble();
}
bool decimal(const QJsonValue &v) {
    bool ok;
    auto n = v.toString().toULongLong(&ok);
    return v.isString() && ok && QString::number(n) == v.toString();
}
bool metadata(const QJsonValue &v) {
    const auto o = v.toObject();
    return v.isObject() && str(o["name"], 256, true) && str(o["author"], 256) && str(o["category"], 128) &&
           str(o["link"], 4096) && str(o["description"], 16384);
}
bool ids(const QJsonValue &v, const QSet<QString> &available) {
    if (!v.isArray() || v.toArray().size() > 99)
        return false;
    QSet<QString> seen;
    for (const auto &id : v.toArray()) {
        if (!hex(id, 64) || !available.contains(id.toString()) || seen.contains(id.toString()))
            return false;
        seen.insert(id.toString());
    }
    return true;
}
bool absent(const QJsonValue &v) {
    return v.isUndefined() || v.isNull();
}
bool archiveFormat(const QJsonValue &v) {
    return v.isString() && QStringList{"Zip", "7-Zip", "RAR", "RAR 5"}.contains(v.toString());
}
bool diagnostic(const QJsonValue &v) {
    const auto e = v.toObject();
    return v.isObject() && e.size() == 2 && str(e["kind"], 256, true) && str(e["message"], 4096);
}
bool archiveMember(const QJsonValue &v, const QJsonValue &relative) {
    if (absent(v))
        return absent(relative);
    const auto a = v.toObject();
    return v.isObject() && a.size() == 6 + a.contains("id") + a.contains("normalizedPath") &&
           str(relative, 4096, true) && hex(a["sha256"], 64) &&
           number(a["index"], 2147483647.) && str(a["decoderPath"], 4096, true) &&
           str(a["memberPath"], 4096, true) && archiveFormat(a["format"]) && str(a["encoding"], 128, true) &&
           (!a.contains("id") || hex(a["id"], 32)) &&
           (!a.contains("normalizedPath") ||
            (str(a["normalizedPath"], 4096, true) && a["normalizedPath"] == a["memberPath"]));
}
bool candidates(const QJsonObject &o, bool confirmed, bool allRetained = false) {
    if (!o["sources"].isArray() || o["sources"].toArray().isEmpty() || o["sources"].toArray().size() > 64 ||
        !o["candidates"].isArray() || o["candidates"].toArray().size() > 1024 || !o["exclusions"].isArray() ||
        o["exclusions"].toArray().size() > 10000 || !o["warnings"].isArray() || o["warnings"].toArray().size() > 10000)
        return false;
    QSet<int> roots;
    for (const auto &v : o["sources"].toArray()) {
        auto r = v.toObject();
        if (!v.isObject() || !number(r["rootOrdinal"], 63) || !str(r["path"], 4096, true) ||
            !QDir::isAbsolutePath(r["path"].toString()) ||
            (r["kind"] != QJsonValue("file") && r["kind"] != QJsonValue("folder")) ||
            roots.contains(r["rootOrdinal"].toInt()))
            return false;
        roots.insert(r["rootOrdinal"].toInt());
    }
    QMap<QString, QJsonObject> archives;
    QSet<QString> archiveCoordinates;
    double archiveBytes = 0;
    if (o.contains("archives") && !o["archives"].isNull()) {
        if (!confirmed || !o["archives"].isArray() || o["archives"].toArray().isEmpty() ||
            o["archives"].toArray().size() > 10000)
            return false;
        for (const auto &v : o["archives"].toArray()) {
            const auto a = v.toObject();
            const auto f = a["file"].toObject();
            const auto coordinate = QString::number(a["rootOrdinal"].toInt()) + ":" + a["relativePath"].toString();
            const auto name = f["sourceName"].toString();
            if (!v.isObject() || a.size() != 5 || !number(a["rootOrdinal"], 63) || !roots.contains(a["rootOrdinal"].toInt()) ||
                !str(a["relativePath"], 4096, true) || !str(a["originalPath"], 4096, true) ||
                !QDir::isAbsolutePath(a["originalPath"].toString()) || !archiveFormat(a["format"]) ||
                !a["file"].isObject() || f.size() != 4 || !hex(f["id"], 32) || !hex(f["sha256"], 64) ||
                !number(f["size"]) || !str(f["sourceName"], 255, true) || name == "." || name == ".." ||
                name.contains('/') || name.contains('\\') ||
                std::any_of(name.cbegin(), name.cend(), [](QChar c) { return c.category() == QChar::Other_Control; }) ||
                name != a["originalPath"].toString().section('/', -1) || archives.contains(f["id"].toString()) ||
                archiveCoordinates.contains(coordinate))
                return false;
            archives.insert(f["id"].toString(), a);
            archiveCoordinates.insert(coordinate);
            archiveBytes += f["size"].toDouble();
            if (archiveBytes > 68719476736.)
                return false;
        }
    }
    const auto matchesArchive = [&archives](const QJsonObject &row) {
        const auto a = row["archive"].toObject();
        if (!a.contains("id"))
            return true; // Unselected archives have no retained destination ID.
        const auto retained = archives.value(a["id"].toString());
        return !retained.isEmpty() && retained["rootOrdinal"] == row["rootOrdinal"] &&
               retained["relativePath"] == row["archiveRelativePath"] &&
               retained["originalPath"] == row["sourcePath"] && retained["format"] == a["format"] &&
               retained["file"].toObject()["sha256"] == a["sha256"];
    };
    QSet<QString> seen, available, selectedArchiveIds;
    const auto selectedIds = o["selectedCandidateIds"].toArray();
    int memberCount = 0;
    QSet<QByteArray> retainedFamilies, selectedFamilies, exactFamilies;
    QMap<QByteArray,double> payloads;
    double selectedLogical=0,retainedLogical=0;
    const auto bytes=[](const QJsonArray &a){return QJsonDocument(a).toJson(QJsonDocument::Compact);};
    for (const auto &v : o["candidates"].toArray()) {
        auto c = v.toObject();
        if (!v.isObject() || !hex(c["id"], 64) || seen.contains(c["id"].toString()) || !number(c["rootOrdinal"], 63) ||
            !roots.contains(c["rootOrdinal"].toInt()) || !str(c["relativePath"], 4096, true) ||
            (!absent(c["archiveRelativePath"]) && !str(c["archiveRelativePath"], 4096, true)) ||
            !str(c["optionName"], 4096, true) || !c["available"].isBool() ||
            !(c["unavailableReason"].isNull() || str(c["unavailableReason"], 4096)) || !c["members"].isArray())
            return false;
        seen.insert(c["id"].toString());
        if (c["available"].toBool())
            available.insert(c["id"].toString());
        bool directory = false;
        QSet<int> indexes;
        for (const auto &m : c["members"].toArray()) {
            if (++memberCount > 10000)
                return false;
            auto f = m.toObject();
            if (!m.isObject() || !str(f["sourcePath"], 4096, true) ||
                !QDir::isAbsolutePath(f["sourcePath"].toString()) || !str(f["sourceName"], 4096, true) ||
                !hex(f["sha256"], 64) || !number(f["size"], 17179869184.))
                return false;
            if (!archiveMember(f["archive"], f["archiveRelativePath"]) ||
                (!absent(c["archiveRelativePath"]) != !absent(f["archive"])) ||
                (!absent(f["archive"]) && c["archiveRelativePath"] != f["archiveRelativePath"]))
                return false;
            auto member = f;
            member.insert("rootOrdinal", c["rootOrdinal"]);
            if (!matchesArchive(member))
                return false;
            if (confirmed && (allRetained ? c["available"].toBool() : selectedIds.contains(c["id"])) && !absent(f["archive"])) {
                const auto archive = f["archive"].toObject();
                if (!archive.contains("id") || !archive.contains("normalizedPath"))
                    return false;
                selectedArchiveIds.insert(archive["id"].toString());
            }
            if (f["role"] == QJsonValue("directory")) {
                if (directory || !f["archiveIndex"].isNull())
                    return false;
                directory = true;
            } else if (f["role"] == QJsonValue("companion")) {
                if (!number(f["archiveIndex"], 65535) || indexes.contains(f["archiveIndex"].toInt()))
                    return false;
                indexes.insert(f["archiveIndex"].toInt());
            } else
                return false;
        }
        if (c["available"].toBool() && !directory)
            return false;
        if(confirmed && allRetained && c["available"].toBool()) {
            QJsonArray dir,exactDir;QMap<int,QJsonArray> companions,exactCompanions;double logical=0;
            for(const auto &v:c["members"].toArray()) {
                const auto f=v.toObject();const QJsonArray content{f["sha256"],f["size"]};const QJsonArray exact{f["sha256"],f["size"],f["sourceName"]};
                logical+=f["size"].toDouble();payloads.insert(bytes(exact),f["size"].toDouble());
                if(f["role"]==QJsonValue("directory")){dir=content;exactDir=exact;}
                else{companions.insert(f["archiveIndex"].toInt(),content);exactCompanions.insert(f["archiveIndex"].toInt(),exact);}
            }
            QJsonArray parts,exactParts;for(auto it=companions.cbegin();it!=companions.cend();++it){parts.append(QJsonArray{it.key(),it.value()});exactParts.append(QJsonArray{it.key(),exactCompanions.value(it.key())});}
            const auto family=bytes(QJsonArray{dir,parts}),exact=bytes(QJsonArray{exactDir,exactParts});
            retainedFamilies.insert(family);if(selectedIds.contains(c["id"])){selectedFamilies.insert(family);selectedLogical+=logical;}
            if(!exactFamilies.contains(exact)){retainedLogical+=logical;exactFamilies.insert(exact);}
        }
    }
    for (const auto &v : o["exclusions"].toArray()) {
        auto r = v.toObject();
        if (!v.isObject() || !number(r["rootOrdinal"], 63) || !roots.contains(r["rootOrdinal"].toInt()) ||
            !str(r["relativePath"], 4096) || !str(r["reason"], 4096, true) ||
            !archiveMember(r["archive"], r["archiveRelativePath"]) ||
            (!absent(r["archive"]) &&
             (!str(r["sourcePath"], 4096, true) || !QDir::isAbsolutePath(r["sourcePath"].toString()) ||
              !matchesArchive(r))))
            return false;
    }
    for (const auto &v : o["warnings"].toArray()) {
        auto w = v.toObject();
        if (!v.isObject() || !str(w["kind"], 256, true) || !str(w["message"], 4096))
            return false;
    }
    double fontBytes = 0;
    if (!absent(o["fontCandidates"]) && !FontData::candidates(o["fontCandidates"],false,o)) return false;
    if (!absent(o["fontDiagnostics"]) && (!confirmed || !FontData::candidates(o["fontDiagnostics"],true,o))) return false;
    if (!absent(o["fonts"])) {
        QVariantList fonts;
        if (!confirmed || !FontData::refs(o["fonts"],fonts,fontBytes,&selectedArchiveIds,o)) return false;
    }
    if (!absent(o["uniqueFontBytes"]) && (!confirmed || !number(o["uniqueFontBytes"],134217728.) || o["uniqueFontBytes"].toDouble()!=fontBytes)) return false;
    if (fontBytes > 0 && !o.contains("uniqueFontBytes")) return false;
    if (!o["sourceArtwork"].isNull() && !LocalArtworkProvider::validSource(o["sourceArtwork"].toObject()))
        return false;
    if (!confirmed)
        return metadata(o["defaultMetadata"]) && ids(o["defaultSelectedCandidateIds"], available);
    const bool storageTotals = o.contains("uniquePayloadBytes") || o.contains("retainedArchiveBytes") ||
                               o.contains("archives");
    if (storageTotals &&
        (!o.contains("archives") || !number(o["uniquePayloadBytes"], 137438953472.) ||
         !number(o["retainedArchiveBytes"], 68719476736.) ||
         o["retainedArchiveBytes"].toDouble() != archiveBytes || selectedArchiveIds.size() != archives.size() ||
         o["uniquePayloadBytes"].toDouble() > o[allRetained ? "retainedLogicalPackageBytes" : "logicalPackageBytes"].toDouble() ||
         o["uniqueCopyBytes"].toDouble() != o["uniquePayloadBytes"].toDouble() + archiveBytes +
             fontBytes + (o["sourceArtwork"].isNull() ? 0 : o["sourceArtwork"].toObject()["size"].toDouble())))
        return false;
    double payloadBytes=0;for(const auto size:payloads)payloadBytes+=size;
    if (allRetained && (!number(o["inactiveOptionCount"],1024) || o["inactiveOptionCount"].toInt()!=available.size()-selectedIds.size()
        || !number(o["retainedPackageCount"],1024) || o["retainedPackageCount"].toInt()<1
        || !number(o["retainedLogicalPackageBytes"],137438953472.) || !number(o["selectedLogicalPackageBytes"],137438953472.)
        || o["selectedLogicalPackageBytes"]!=o["logicalPackageBytes"] || o["logicalPackageBytes"].toDouble()!=selectedLogical
        || o["retainedLogicalPackageBytes"].toDouble()!=retainedLogical || o["uniquePayloadBytes"].toDouble()!=payloadBytes
        || o["retainedPackageCount"].toInt()!=retainedFamilies.size() || o["storedPackageCount"].toInt()!=selectedFamilies.size())) return false;
    return hex(o["jobId"], 32) && str(o["token"], 256, true) && o["modId"].toString().startsWith("local-") &&
           hex(o["modId"].toString().sliced(6), 32) && str(o["profileId"], 256, true) &&
           str(o["profileNameAtStart"], 4096) && decimal(o["expectedRevision"]) && metadata(o["metadata"]) &&
           ids(o["selectedCandidateIds"], available) && number(o["selectedPackageCount"], 99) &&
           o["selectedPackageCount"].toInt() == o["selectedCandidateIds"].toArray().size() &&
           number(o["storedPackageCount"], 99) && o["storedPackageCount"].toInt() > 0 &&
           o["storedPackageCount"].toInt() <= o["selectedPackageCount"].toInt() &&
           number(o["uniqueCopyBytes"], 137589948416.) && number(o["logicalPackageBytes"], 137438953472.);
}
bool status(const QJsonObject &o) {
    static const QStringList phases{"enumerating", "inspecting", "hashing", "copyingArchives", "listingArchives", "extractingArchives", "ready",
                                    "cancelRequested", "cancelled",  "interrupted",     "failed",
                                    "committing",      "succeeded",  "recoveryRequired"};
    auto small = o;
    small.remove("candidateSummary");
    small.remove("cleanupError"); // Separately bounded transient diagnostic, outside the persisted status body.
    if (QJsonDocument(small).toJson(QJsonDocument::Compact).size() > 4096 || !hex(o["jobId"], 32) ||
        o["kind"] != QJsonValue("localImport") || o["jobVersion"] != QJsonValue(1) || !str(o["profileId"], 256, true) ||
        !str(o["profileNameAtStart"], 4096) || !decimal(o["expectedRevision"]) || !decimal(o["observedRevision"]) ||
        !decimal(o["sequence"]) || !phases.contains(o["phase"].toString()) || !o["canCancel"].isBool() ||
        !(o["operationId"].isNull() || hex(o["operationId"], 32)))
        return false;
    if (o["sequence"] == QJsonValue("0") ||
        o["canCancel"].toBool() !=
            QStringList{"enumerating", "inspecting", "hashing", "copyingArchives", "listingArchives", "extractingArchives", "ready"}.contains(o["phase"].toString()))
        return false;
    for (const auto *key : {"filesDone", "filesTotal"})
        if (!o[key].isNull() && !number(o[key], 10000))
            return false;
    for (const auto *key : {"filesDone", "bytesDone"})
        if (!number(o[key]))
            return false;
    for (const auto *key : {"filesTotal", "bytesTotal"})
        if (!o[key].isNull() && !number(o[key]))
            return false;
    if ((!o["error"].isNull() && !diagnostic(o["error"])) ||
        (o.contains("cleanupError") && !diagnostic(o["cleanupError"])))
        return false;
    return !o.contains("candidateSummary") || (o["phase"] == QJsonValue("ready") && o["candidateSummary"].isObject() &&
                                               candidates(o["candidateSummary"].toObject(), false));
}
bool terminal(const QString &s) {
    return QStringList{"cancelled", "interrupted", "failed", "succeeded", "recoveryRequired"}.contains(s);
}
} // namespace
bool LocalImportController::decodeReply(const QJsonValue &value, QVariantMap &out, bool allRetained) {
    if (value.isArray()) {
        if (value.toArray().size() > 8)
            return false;
        QSet<QString> seen;
        for (const auto &v : value.toArray()) {
            if (!v.isObject() || !status(v.toObject()) || v.toObject().contains("candidateSummary") ||
                seen.contains(v.toObject()["jobId"].toString()))
                return false;
            seen.insert(v.toObject()["jobId"].toString());
        }
        out = {{"jobs", value.toArray().toVariantList()}};
        return true;
    }
    if (!value.isObject())
        return false;
    const auto o = value.toObject();
    if(o.contains("groupRows")) {
        if(!hex(o["jobId"],32) || !str(o["profileId"],256,true) || !decimal(o["expectedRevision"]) || !o["groupRows"].isArray() || o["groupRows"].toArray().isEmpty() || o["groupRows"].toArray().size()>64)return false;
        QSet<QString> ids;
        for(const auto &v:o["groupRows"].toArray()){const auto row=v.toObject();const auto m=row["metadata"].toObject();if(!v.isObject() || !str(row["id"],256,true) || ids.contains(row["id"].toString()) || !row["files"].isArray() || !number(row["bytes"],137589948416.0) || !str(m["name"],256,true) || !str(m["author"],256) || !str(m["category"],128) || !str(m["description"],16384) || !str(m["link"],4096) || (!row["problem"].isNull() && !row["problem"].isObject()))return false;ids.insert(row["id"].toString());}
        if(o.contains("token") && (!hex(o["token"],32) || !candidates(o,true,allRetained)))return false;
        out=o.toVariantMap();return true;
    }
    const bool valid = o.contains("dismissed") ? hex(o["jobId"], 32) && o["dismissed"] == QJsonValue(true)
                       : o.contains("token")   ? candidates(o, true, allRetained)
                                               : status(o);
    if (valid)
        out = o.toVariantMap();
    return valid;
}
LocalImportController::LocalImportController(LibraryModel *library)
    : QObject(library), m_library(library), m_client(new BackendClient(this)) {
    m_poll.setInterval(250);
    m_poll.setSingleShot(true);
    m_retirementPoll.setInterval(250);
    connect(&m_retirementPoll,&QTimer::timeout,this,&LocalImportController::driveRetirement);
    connect(&m_poll, &QTimer::timeout, this, &LocalImportController::poll);
    connect(m_client, &BackendClient::capabilitiesReady, this, [this] {
        updateContext();
        emit changed();
        refreshHistory();
    });
    connect(m_client, &BackendClient::lifecycleChanged, this, &LocalImportController::updateContext);
    connect(m_client, &BackendClient::localReady, this, [this](BackendClient::Call call,const QVariantMap &result) {
        if(OptionData::optionsCall(call)) {
            if(!m_optionsPending || call!=m_optionsCall) return;
            m_optionsPending=false; finishCancellation(call,result.value("jobId").toString());
            // A queued cancel takes the shared lane before response consumers can request another read.
            drainCancellations(); emit optionsReady(call,result); emit changed(); return;
        }
        if(OptionData::importCall(call)) {finishCancellation(call,result.value("jobId").toString());accept(call,result);drainCancellations();}
    });
    connect(m_client, &BackendClient::rejected, this,
            [this](BackendClient::Call c, const QString &kind, const QString &message) {
                if(OptionData::optionsCall(c)) {
                    if(c!=BackendClient::Call::OptionsAttach) {m_optionsPending=false;finishCancellation(c);drainCancellations();emit optionsRejected(c,kind+": "+message);emit changed();} return;
                }
                if (c != BackendClient::Call::LocalCommit && OptionData::importCall(c)) {
                    finishCancellation(c);fail(c,kind+": "+message);drainCancellations();
                }
            });
    connect(m_client, &BackendClient::failed, this, [this](const QString &message) {
        if(!m_retirementActive){resetCancellations();emit optionsSessionReset();}
        m_optionsPending=false;
        m_small = m_bulk = false;
        m_commitSession = false;
        m_error = message;
        if (!m_retirementActive && m_jobs.contains(m_job))
            m_jobs[m_job].review.remove("token");
        m_poll.stop();
        emit changed();
    });
    connect(m_client, &BackendClient::retired, this, [this] {
        if(!m_retirementActive){resetCancellations();emit optionsSessionReset();}
        m_optionsPending=m_optionsWriting=false;
        m_small = m_bulk = false;
        m_commitSession = false;
        syncContext();
        if(m_retirementActive)driveRetirement();
    });
    connect(library, &LibraryModel::nativeChanged, this, &LocalImportController::syncContext);
    connect(library, &LibraryModel::sourceChanged, this, &LocalImportController::updateContext);
    connect(library, &LibraryModel::loadingChanged, this, &LocalImportController::updateContext);
}
void LocalImportController::updateContext() {
    const QVariantList identity{m_library->dataDir(), m_library->helperPath(),
                                QString::number(m_library->loadEpoch()), QString::number(m_client->sessionGeneration()),
                                m_library->viewedProfileId(), m_library->revision(),
                                m_library->nativeMode(), m_library->loading()};
    if (identity == m_contextIdentity)
        return;
    m_contextIdentity = identity;
    m_contextId = QUuid::createUuid().toString(QUuid::Id128);
    emit contextChanged();
}
void LocalImportController::syncContext() {
    if (m_root != m_library->dataDir() || m_helper != m_library->helperPath()) {
        m_client->stop();resetCancellations();
        m_optionsPending=m_optionsWriting=m_optionsBlock=m_optionsHistoryKnown=false; m_optionsCount=0; emit optionsSessionReset();
        m_jobs.clear();
        m_job.clear();
        m_historyRefresh = true;
        m_small = m_bulk = m_commit = m_commitSession = false;
        m_root = m_library->dataDir();
        m_helper = m_library->helperPath();
    }
    if (!m_retirementActive && m_library->nativeMode() && !m_client->isRunning() && !m_client->retiring() && !m_helper.isEmpty())
        m_client->start(m_helper, {}, m_root, true, true);
    if (!m_retirementActive && !m_job.isEmpty() && m_jobs.contains(m_job) &&
        m_jobs[m_job].status.value("expectedRevision").toString() != m_library->revision() && !m_commit)
        m_jobs[m_job].review.remove("token");
    updateContext();
    drainCancellations();
    if (phase() == "ready" && !m_job.isEmpty())
        requestCandidates();
    emit changed();
}
bool LocalImportController::controlAvailable() const {
    return m_client->readyForRequests() && !m_retirementActive && m_library->nativeMode() && !m_library->launch()->writeGate() && !m_library->shuttingDown() && !m_library->outcomeUnknown() && !m_library->recoveryRequired();
}
bool LocalImportController::writable() const {
    return controlAvailable() && !m_library->writeBusy() && !m_library->awaitingEditSnapshot();
}
void LocalImportController::beginRetirement(const QString &root,quint64 epoch) {
    if(m_retirementActive || epoch!=m_library->loadEpoch() || (root!=m_root && (m_client->isRunning() || m_client->retiring())))return;
    m_retirementActive=true;
    m_retirementRoot=root;
    m_retirementEpoch=epoch;
    m_retirementSession=m_client->sessionGeneration();
    m_retirementStopped=false;
    m_poll.stop();
    m_retirementPoll.start();
    driveRetirement();
    emit changed();
}
bool LocalImportController::retirementReady(const QString &root,quint64 epoch) const {
    return m_retirementActive && m_retirementRoot==root && m_retirementEpoch==epoch
        && !m_client->isRunning() && !m_client->retiring() && !m_client->writing();
}
void LocalImportController::driveRetirement() {
    if(!m_retirementActive || m_retirementEpoch!=m_library->loadEpoch())return;
    if(!m_retirementStopped && m_client->isRunning() && m_retirementRoot==m_root
        && m_client->sessionGeneration()==m_retirementSession && !m_client->writing()) {
        m_retirementStopped=true;
        m_client->stop();
    }
    if(retirementReady(m_retirementRoot,m_retirementEpoch))m_retirementPoll.stop();
    emit changed();
}
void LocalImportController::resumeAfterRetirement() {
    m_retirementPoll.stop();
    m_retirementActive=false;
    m_retirementStopped=false;
    m_retirementRoot.clear();
    m_retirementEpoch=0;
    m_retirementSession=0;
}
bool LocalImportController::canPrepare() const {
    return !cancelling() && !m_historyRefresh && writable() && m_optionsHistoryKnown && !m_optionsPending && !m_optionsWriting && !m_optionsBlock && !m_small && !m_bulk && !busy() && phase() != "ready" && m_jobs.size()+m_optionsCount < 8 &&
           m_client->isRunning();
}
bool LocalImportController::busy() const {
    return m_commit || (!m_job.isEmpty() && !terminal(phase()) && phase() != "ready");
}
bool LocalImportController::canCancel() const { return canCancelJob(m_job); }

bool LocalImportController::canReview() const {
    return !cancelling() && m_client->isRunning() && !m_optionsPending && !m_optionsWriting && writable() && !m_small && !m_bulk && !m_commit && phase() == "ready" &&
           m_jobs.value(m_job).initialized;
}
bool LocalImportController::canCommit() const {
    return !cancelling() && m_client->isRunning() && !m_optionsPending && !m_optionsWriting && writable() && !m_library->busy() && !m_small && !m_bulk && !m_commit &&
           phase() == "ready" && !review().value("token").toString().isEmpty() &&
           m_jobs.value(m_job).status.value("expectedRevision").toString() == m_library->revision();
}
bool LocalImportController::canDismiss() const {
    return !cancelling() && m_client->isRunning() && !m_optionsPending && !m_optionsWriting && writable() && !m_small && !m_bulk && !m_commit && m_jobs.contains(m_job) &&
           terminal(phase());
}
QString LocalImportController::phase() const {
    return m_commit ? QStringLiteral("committing") : m_jobs.value(m_job).status.value("phase").toString();
}
QString LocalImportController::destinationName() const {
    return m_jobs.value(m_job).status.value("profileNameAtStart").toString();
}
bool LocalImportController::presentationReady() const {
    return phase() == "ready" && m_jobs.contains(m_job) && m_jobs.value(m_job).initialized;
}
bool LocalImportController::cancelPending() const {
    if (m_job.isEmpty())
        return false;
    if (m_cancelInFlight == m_job && m_cancelCall == BackendClient::Call::LocalCancel)
        return true;
    return std::any_of(m_cancellations.cbegin(), m_cancellations.cend(), [this](const Cancellation &c) {
        return c.call == BackendClient::Call::LocalCancel && c.jobId == m_job;
    });
}
bool LocalImportController::draftCurrent() const {
    return presentationReady() && !cancelPending() && !m_library->loading() && m_library->nativeMode() &&
           m_jobs.value(m_job).status.value("expectedRevision").toString() == m_library->revision();
}
QString LocalImportController::draftGeneration() const {
    return m_jobs.contains(m_job) ? QString::number(m_jobs.value(m_job).edits) : QString();
}
QVariantMap LocalImportController::review() const {
    return m_jobs.value(m_job).review;
}
QVariantList LocalImportController::jobs() const {
    QVariantList out;
    for (const auto &j : m_jobs)
        out.append(j.status);
    return out;
}
QVariantMap LocalImportController::metadataDraft() const {
    return m_jobs.value(m_job).metadata;
}
QStringList LocalImportController::selectionDraft() const {
    return m_jobs.value(m_job).selection;
}
void LocalImportController::setVisible(bool value) {
    if (value == m_visible)
        return;
    m_visible = value;
    if (value)
        poll();
    emit changed();
}
void LocalImportController::setMetadataDraft(const QVariantMap &value) {
    if (!m_jobs.contains(m_job) || m_commit || value == metadataDraft())
        return;
    auto &j = m_jobs[m_job];
    j.metadata = value;
    ++j.edits;
    j.review.remove("token");
    emit changed();
}
void LocalImportController::setSelectionDraft(const QStringList &value) {
    if (!m_jobs.contains(m_job) || m_commit || value == selectionDraft())
        return;
    auto &j = m_jobs[m_job];
    j.selection = value;
    ++j.edits;
    j.review.remove("token");
    emit changed();
}
QString LocalImportController::startSourcePicker() {
    updateContext();
    m_pickerTicket.clear();
    m_pickerContext.clear();
    if (m_library->loading() || !canPrepare()) {
        m_error = tr("Import is unavailable right now. Wait for current work to finish, then choose files again.");
        emit changed();
        return {};
    }
    m_pickerTicket = QUuid::createUuid().toString(QUuid::Id128);
    m_pickerContext = m_contextId;
    return m_pickerTicket;
}
void LocalImportController::cancelSourcePicker(const QString &ticket) {
    if (!ticket.isEmpty() && ticket == m_pickerTicket) {
        m_pickerTicket.clear();
        m_pickerContext.clear();
    }
}
bool LocalImportController::preparePicked(const QString &ticket, const QList<QUrl> &roots, const QUrl &artwork) {
    updateContext();
    const bool matches = !ticket.isEmpty() && ticket == m_pickerTicket && m_pickerContext == m_contextId;
    if (!ticket.isEmpty() && ticket == m_pickerTicket) {
        m_pickerTicket.clear();
        m_pickerContext.clear();
    }
    if (!matches) {
        m_error = tr("The library or viewed profile changed while the picker was open. Choose files again.");
        emit changed();
        return false;
    }
    if (m_library->loading() || !canPrepare()) {
        m_error = tr("Import is unavailable right now. Wait for current work to finish, then choose files again.");
        emit changed();
        return false;
    }
    return prepare(roots, artwork);
}
bool LocalImportController::prepareDrop(const QList<QUrl> &roots) {
    updateContext();
    if (m_library->loading() || !canPrepare()) {
        m_error = tr("Import is unavailable right now. Wait for current work to finish, then drop the files again.");
        emit changed();
        return false;
    }
    return prepare(roots);
}
bool LocalImportController::prepare(const QList<QUrl> &roots, const QUrl &artwork) {
    updateContext();
    if (!canPrepare())
        return false;
    QJsonArray paths;
    const auto path = [](const QUrl &u) {
        auto p = u.toLocalFile();
        return u.isValid() && u.isLocalFile() && u.host().isEmpty() && QDir::isAbsolutePath(p) &&
                       p.toUtf8().size() <= 4096 && !p.contains(QChar::Null)
                   ? p
                   : QString();
    };
    if (roots.isEmpty() || roots.size() > 64) {
        m_error = tr("Choose 1–64 local files or folders.");
        emit changed();
        return false;
    }
    for (const auto &u : roots) {
        auto p = path(u);
        if (p.isEmpty()) {
            m_error = tr("Only absolute local file and folder URLs are accepted.");
            emit changed();
            return false;
        }
        paths.append(p);
    }
    const auto art = artwork.isEmpty() ? QString() : path(artwork);
    if (!artwork.isEmpty() && art.isEmpty()) {
        m_error = tr("Artwork must be a local PNG or JPEG file.");
        emit changed();
        return false;
    }
    const auto id = QUuid::createUuid().toString(QUuid::Id128);
    const auto profile = m_library->viewedProfileId();
    QString name = profile;
    for (const auto &p : m_library->profiles())
        if (p.toMap().value("id").toString() == profile)
            name = p.toMap().value("name").toString();
    QJsonObject params{
        {"jobId", id}, {"profileId", profile}, {"expectedRevision", m_library->revision()}, {"roots", paths}};
    if (!art.isEmpty())
        params.insert("artworkPath", art);
    if (QJsonDocument(params).toJson(QJsonDocument::Compact).size() > 60 * 1024) {
        m_error = tr("The selected paths exceed the request limit.");
        emit changed();
        return false;
    }
    m_job = id;
    Job job;
    job.status = {{"jobId", id},
                  {"profileId", profile},
                  {"profileNameAtStart", name},
                  {"expectedRevision", m_library->revision()},
                  {"phase", "enumerating"},
                  {"canCancel", true},
                  {"sequence", "0"}};
    m_jobs.insert(id, job);
    m_small = true;
    m_error.clear();
    emit preparationStarted(id, m_contextId);
    m_client->nativeRequest(BackendClient::Call::LocalPrepare, params);
    m_poll.start();
    emit changed();
    return true;
}
void LocalImportController::refreshHistory() {
    m_historyRefresh = true;
    m_poll.start();
}
void LocalImportController::poll() {
    m_poll.start();
    drainCancellations();
    if (cancelling() || !controlAvailable() || m_optionsPending || m_optionsWriting || m_small || m_bulk || m_commit ||
        !m_client->isRunning() || m_client->writing())
        return;
    const bool followUp = std::any_of(m_jobs.cbegin(), m_jobs.cend(), [](const Job &job) {
        const auto phase = job.status.value("phase").toString();
        return !terminal(phase) && phase != "ready";
    });
    // One bounded purpose-specific list observes every retained Import, without
    // changing the selected editor job or depending on dialog visibility.
    if (m_historyRefresh || followUp) {
        m_small = true;
        m_client->nativeRequest(BackendClient::Call::LocalStatus);
    } else if (m_visible && !m_job.isEmpty() && phase() == "ready") {
        requestCandidates();
    }
}
void LocalImportController::requestCandidates() {
    if (cancelling() || m_small || m_optionsPending || m_optionsWriting || m_bulk || !writable() || m_jobs[m_job].initialized)
        return;
    m_bulk = true;
    m_client->nativeRequest(BackendClient::Call::LocalStatus, {{"jobId", m_job}, {"includeCandidates", true}});
}
void LocalImportController::accept(BackendClient::Call call, const QVariantMap &r) {
    m_poll.start();
    if (r.contains("jobs")) {
        m_small = false;
        drainCancellations();
        QMap<QString, Job> next;
        for (const auto &v : r.value("jobs").toList()) {
            const auto s = v.toMap();
            const auto id = s.value("jobId").toString();
            auto job = m_jobs.value(id);
            if (!job.status.isEmpty() &&
                (s.value("profileId") != job.status.value("profileId") ||
                 s.value("expectedRevision") != job.status.value("expectedRevision") ||
                 s.value("sequence").toString().toULongLong() < job.status.value("sequence").toString().toULongLong())) {
                m_historyRefresh = true;
                m_error = tr("Import history changed its pinned identity or sequence.");
                emit changed();
                return;
            }
            job.status = s;
            if (job.durableSuccess) {
                job.status.insert("phase", "succeeded");
                job.status.insert("canCancel", false);
            }
            if (terminal(job.status.value("phase").toString()))
                job.review.remove("token");
            next.insert(id, job);
        }
        m_jobs = next;
        if (!m_jobs.contains(m_job))
            m_job.clear();
        m_historyRefresh = false;
        emit changed();
        return;
    }
    const auto id = r.value("jobId").toString();
    if (call == BackendClient::Call::LocalReview || r.contains("candidateSummary"))
        m_bulk = false;
    else
        m_small = false;
    if (!m_jobs.contains(id)) {
        emit changed();
        return;
    }
    auto &job = m_jobs[id];
    if (call != BackendClient::Call::LocalDismiss &&
        (r.value("profileId") != job.status.value("profileId") ||
         r.value("expectedRevision") != job.status.value("expectedRevision"))) {
        m_error = tr("Local job response changed its pinned destination or revision.");
        job.review.remove("token");
        emit changed();
        return;
    }
    if (call == BackendClient::Call::LocalDismiss) {
        m_jobs.remove(id);
        if (m_job == id)
            m_job.clear();
        refreshHistory();
        emit changed();
        return;
    }
    if (call == BackendClient::Call::LocalReview) {
        if (canCancelJob(id) && job.edits == m_reviewEdits && r.value("expectedRevision").toString() == m_library->revision())
            {job.review = r;
             for(auto &draft:job.groupRows){auto row=draft.toMap();for(const auto &v:r.value("groupRows").toList()){const auto reviewed=v.toMap();if(reviewed.value("id")==row.value("id")){for(const auto *field:{"files","bytes","problem","modId"})row.insert(field,reviewed.value(field));}}draft=row;}}
    } else if (r.value("sequence").toString().toULongLong() >= job.status.value("sequence").toString().toULongLong()) {
        job.status = r;
        job.status.remove("candidateSummary");
        if (job.durableSuccess) {
            job.status.insert("phase", "succeeded");
            job.status.insert("canCancel", false);
        }
        const auto summary = r.value("candidateSummary").toMap();
        if (!summary.isEmpty() && !job.initialized) {
            job.review = summary;
            job.review.insert("expectedRevision", r.value("expectedRevision"));
            job.review.insert("profileId", r.value("profileId"));
            job.metadata = summary.value("defaultMetadata").toMap();
            job.selection = summary.value("defaultSelectedCandidateIds").toStringList();
            job.initialized = true;
        }
        if (terminal(job.status.value("phase").toString()))
            job.review.remove("token");
        const auto e = r.value("error").toMap();
        if (!e.isEmpty() && !job.durableSuccess)
            m_error = e.value("kind").toString() + ": " + e.value("message").toString();
    }
    const auto art = job.review.value("sourceArtwork").toMap();
    if (!art.isEmpty())
        job.review.insert("artworkUrl", LocalArtworkProvider::admitSource(QJsonObject::fromVariantMap(art)));
    drainCancellations();
    if (id == m_job && phase() == "ready") requestCandidates();
    if (call != BackendClient::Call::LocalReview && id==m_job && job.initialized && !job.groupRows.isEmpty() && !job.groupReviewed && !m_bulk)
        reviewCurrentDraft();
    emit changed();
}
bool LocalImportController::reviewSelection(const QStringList &ids, const QVariantMap &metadata) {
    if (!canReview())
        return false;
    setSelectionDraft(ids);
    setMetadataDraft(metadata);
    auto sorted = ids;
    sorted.sort();
    sorted.removeDuplicates();
    m_reviewEdits = m_jobs[m_job].edits;
    m_bulk = true;
    m_error.clear();
    m_client->nativeRequest(BackendClient::Call::LocalReview, {{"jobId", m_job},
                                                               {"candidateIds", QJsonArray::fromStringList(sorted)},
                                                               {"metadata", QJsonObject::fromVariantMap(metadata)}});
    emit changed();
    return true;
}
bool LocalImportController::reviewCurrentDraft() {
    if (!groupRows().isEmpty()) {
        if (!canReview()) return false;
        m_jobs[m_job].groupReviewed=true;
        m_reviewEdits=m_jobs[m_job].edits; m_bulk=true; m_error.clear();
        m_client->nativeRequest(BackendClient::Call::LocalReview,{{"jobId",m_job},{"groupRows",QJsonArray::fromVariantList(groupRows())}});
        emit changed(); return true;
    }
    return reviewSelection(selectionDraft(), metadataDraft());
}
bool LocalImportController::cancel() { return cancelJob(m_job); }

bool LocalImportController::commit() {
    if (!canCommit())
        return false;
    const auto r = review();
    m_commit = true;
    m_commitSession = true;
    m_poll.stop();
    emit changed();
    if (!m_library->dispatchLocalCommit(m_client, m_jobs[m_job].status.value("profileId").toString(),
                                        m_jobs[m_job].status.value("expectedRevision").toString(),
                                        {{"jobId", m_job}, {"token", r.value("token").toString()}}, r.value("modId").toString())) {
        m_commit = false;
        m_commitSession = false;
        m_poll.start();
        emit changed();
        return false;
    }
    m_jobs[m_job].review.remove("token");
    return true;
}
bool LocalImportController::dismiss(const QString &id) {
    if (cancelling() || !m_client->isRunning() || !writable() || m_optionsPending || m_optionsWriting || m_small || m_bulk || m_commit || !m_jobs.contains(id) ||
        !terminal(m_jobs[id].status.value("phase").toString()))
        return false;
    m_small = true;
    m_client->nativeRequest(BackendClient::Call::LocalDismiss, {{"jobId", id}});
    emit changed();
    return true;
}
void LocalImportController::showJob(const QString &id) {
    if (m_jobs.contains(id) && !busy()) {
        m_job = id;
        emit changed();
    }
}
void LocalImportController::fail(BackendClient::Call call, const QString &message) {
    m_poll.start();
    if (call == BackendClient::Call::LocalCancel || call == BackendClient::Call::LocalDismiss)
        m_historyRefresh = true;
    if (call == BackendClient::Call::LocalReview)
        m_bulk = false;
    else
        m_small = m_bulk = false;
    m_error = message;
    if (call == BackendClient::Call::LocalPrepare) {
        m_jobs[m_job].status.insert("phase", "failed");
        m_jobs[m_job].status.insert("canCancel", false);
    }
    emit changed();
}
void LocalImportController::commitConfirmed(bool success, const QString &message) {
    m_poll.start();
    const bool refresh = success && m_commitSession && m_client->isRunning() && !m_client->writing() &&
                         m_client->nativeRoot() == m_root && m_client->readyForRequests() &&
                         m_jobs.contains(m_job) && !m_small && !m_bulk;
    if(success && !groupRows().isEmpty()) {
        QVariantList added,skipped;QSet<QString> kept;
        for(const auto &v:review().value("groupRows").toList()){const auto r=v.toMap();added.append(r.value("modId"));kept.insert(r.value("id").toString());}
        for(const auto &v:m_jobs[m_job].originalGroupRows){const auto id=v.toMap().value("id").toString();if(!kept.contains(id))skipped.append(QVariantMap{{"id",id},{"reason","dropped"}});}
        m_jobs[m_job].result={{"added",added},{"skipped",skipped}};
    }
    m_commitSession = false;
    m_commit = false;
    m_error = m_library->outcomeUnknown() ? QString() : message;
    if (m_jobs.contains(m_job))
        m_jobs[m_job].diagnostic = message;
    if (success && m_library->viewedProfileId() == m_jobs.value(m_job).status.value("profileId").toString())
        m_library->setSelectedId(review().value("modId").toString());
    if (m_jobs.contains(m_job)) {
        m_jobs[m_job].durableSuccess = success;
        m_jobs[m_job].status.insert("phase", success                       ? "succeeded"
                                             : m_library->outcomeUnknown() ? "recoveryRequired"
                                                                           : "failed");
        m_jobs[m_job].status.insert("canCancel", false);
    }
    if (refresh) {
        // This small, read-only request uses the writer's still-pinned session.
        // It observes private cleanup after publication without replaying a writer.
        m_small = true;
        m_client->nativeRequest(BackendClient::Call::LocalStatus, {{"jobId", m_job}});
    }
    emit changed();
}

bool LocalImportController::optionsAvailable() const {
    return m_client->isRunning() && writable();
}
bool LocalImportController::optionsRequest(BackendClient::Call call,QJsonObject params) {
    if(cancelling() || !OptionData::optionsCall(call) || (call==BackendClient::Call::OptionsAttach || call==BackendClient::Call::OptionsCancel) || !optionsAvailable() || m_optionsPending || m_optionsWriting || m_small || m_bulk || m_commit) return false;
    if(call==BackendClient::Call::OptionsPrepare && (m_historyRefresh || busy() || phase()=="ready" || m_optionsBlock || m_jobs.size()+m_optionsCount>=8 || m_library->busy()))return false;
    m_optionsCall=call;m_optionsPending=true;
    m_client->nativeRequest(call,params);emit changed();return m_optionsPending;
}
bool LocalImportController::attachOptions(const QString &profile,const QString &revision,QJsonObject params,const QString &modId) {
    if(cancelling() || !optionsAvailable() || m_optionsPending || m_optionsWriting || m_small || m_bulk || m_commit)return false;
    m_optionsWriting=true;
    if(!m_library->dispatchOptionsAttach(m_client,profile,revision,params,modId)){m_optionsWriting=false;return false;}
    emit changed();return true;
}
void LocalImportController::optionsFinished(){m_optionsWriting=false;emit changed();}
void LocalImportController::setOptionsHistory(int count,bool blocksPreparation,bool known){if(m_optionsCount==count&&m_optionsBlock==blocksPreparation&&m_optionsHistoryKnown==known)return;m_optionsCount=count;m_optionsBlock=blocksPreparation;m_optionsHistoryKnown=known;emit changed();}

bool LocalImportController::canCancelJob(const QString &id) const {
    return m_jobs.contains(id) && m_jobs[id].status.value("canCancel").toBool()
        && canQueueCancellation(BackendClient::Call::LocalCancel,id);
}
bool LocalImportController::cancelJob(const QString &id) {
    if(!canCancelJob(id))return false;
    m_jobs[id].review.remove("token");
    return queueCancellation(BackendClient::Call::LocalCancel,id);
}
bool LocalImportController::optionsBusy() const { return m_optionsPending || cancelling(); }
bool LocalImportController::canCancelOptionsJob(const QString &id) const {
    return canQueueCancellation(BackendClient::Call::OptionsCancel,id);
}
bool LocalImportController::cancelOptionsJob(const QString &id) {
    return queueCancellation(BackendClient::Call::OptionsCancel,id);
}
bool LocalImportController::cancelling() const { return !m_cancellations.isEmpty() || !m_cancelInFlight.isEmpty(); }
bool LocalImportController::canQueueCancellation(BackendClient::Call call,const QString &id) const {
    if(call!=BackendClient::Call::LocalCancel && call!=BackendClient::Call::OptionsCancel)return false;
    if(!controlAvailable() || m_commit || m_optionsWriting || m_client->writing() || !m_client->isRunning()
        || !FontData::hex(id,32) || m_cancelInFlight==id || m_cancellations.size()+(m_cancelInFlight.isEmpty()?0:1)>=8)return false;
    return std::none_of(m_cancellations.cbegin(),m_cancellations.cend(),[&](const Cancellation &c){return c.jobId==id;});
}
bool LocalImportController::queueCancellation(BackendClient::Call call,const QString &id) {
    if(!canQueueCancellation(call,id))return false;
    // Capture purpose and job now; later profile/editor switches cannot retarget cancellation.
    m_cancellations.append({call,id});
    if (call == BackendClient::Call::LocalCancel) emit cancellationRequested(id);
    drainCancellations();emit changed();return true;
}
void LocalImportController::drainCancellations() {
    if(m_cancellations.isEmpty() || !m_cancelInFlight.isEmpty() || m_small || m_bulk || m_optionsPending
        || m_commit || m_optionsWriting || m_client->writing() || !controlAvailable() || !m_client->isRunning())return;
    const auto next=m_cancellations.takeFirst();
    m_cancelInFlight=next.jobId;m_cancelCall=next.call;
    if(next.call==BackendClient::Call::OptionsCancel){m_optionsCall=next.call;m_optionsPending=true;}
    else m_small=true;
    m_client->nativeRequest(next.call,{{"jobId",next.jobId}});
}
void LocalImportController::finishCancellation(BackendClient::Call call,const QString &id) {
    if(call==m_cancelCall && (id.isEmpty() || id==m_cancelInFlight))m_cancelInFlight.clear();
}
void LocalImportController::resetCancellations() {m_cancellations.clear();m_cancelInFlight.clear();}

QVariantMap LocalImportController::progress() const {
    const auto s=m_jobs.value(m_job).status;
    return {{"filesDone",s.value("filesDone",0)},{"bytesDone",s.value("bytesDone",0)},{"bytesTotal",s.value("bytesTotal",0).isNull()?QVariant(0):s.value("bytesTotal",0)}};
}
bool LocalImportController::prepareGroup(const QList<QUrl> &roots,const QVariantList &rows) {
    if(rows.isEmpty() || rows.size()>64 || !prepare(roots))return false;
    m_jobs[m_job].groupRows=rows;m_jobs[m_job].originalGroupRows=rows;emit changed();return true;
}
bool LocalImportController::setGroupLink(const QString &id,const QString &link) {
    if(!canReview())return false;
    auto &job=m_jobs[m_job];
    for(auto &v:job.groupRows){auto r=v.toMap();if(r.value("id").toString()!=id)continue;auto m=r.value("metadata").toMap();m.insert("link",link);r.insert("metadata",m);v=r;job.review.remove("token");++job.edits;emit changed();return true;}return false;
}
bool LocalImportController::dropGroupRow(const QString &id) {
    if(!canReview() || groupRows().size()<2)return false;
    auto &job=m_jobs[m_job];for(int i=0;i<job.groupRows.size();++i)if(job.groupRows[i].toMap().value("id").toString()==id){job.groupRows.removeAt(i);job.review.remove("token");++job.edits;emit changed();return true;}return false;
}
