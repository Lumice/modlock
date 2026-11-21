#include "libraryrows.h"
#include "localartworkprovider.h"
#include "fontdata.h"
#include "launchdata.h"
#include "catalogdata.h"
#include "librarymediacontroller.h"

#include <QJsonArray>
#include <QDebug>
#include <QDateTime>
#include <QRegularExpression>
#include <QJsonDocument>
#include <QCryptographicHash>
#include <QSet>
#include <QUrl>
#include <algorithm>
#include <cmath>
#include <utility>

namespace {
bool strings(const QJsonValue &value, QStringList &result) {
    if (!value.isArray()) return false;
    for (const auto &entry : value.toArray()) {
        if (!entry.isString()) return false;
        result.append(entry.toString());
    }
    return true;
}
bool fields(const QJsonObject &object, std::initializer_list<const char *> names) {
    for (const auto *name : names) if (!object.value(QLatin1String(name)).isString()) return false;
    return true;
}
bool integer(const QJsonValue &value) {
    return value.isDouble() && std::isfinite(value.toDouble()) && std::floor(value.toDouble()) == value.toDouble();
}
QStringList previews(const QJsonValue &value) {
    QStringList result;
    QSet<QString> seen;
    for (const auto &entry : value.toArray()) {
        if (!entry.isString()) continue;
        const auto source = entry.toString();
        if (source.size() > 4096 || std::any_of(source.cbegin(), source.cend(), [](QChar c) {
            return c.category() == QChar::Other_Control;
        })) continue;
        const QUrl url(source, QUrl::StrictMode);
        if (!url.isValid() || url.scheme() != QStringLiteral("https") || url.host().isEmpty()
            || !url.userInfo().isEmpty() || url.authority().contains('@') || seen.contains(source)) continue;
        seen.insert(source); result.append(source);
        if (result.size() == 32) break;
    }
    return result;
}
}

QVariantMap LibraryMod::details() const {
    return {{"addedAt",addedAt},{"updatedAt",updatedAt},{"sizeBytes",sizeBytes},{"variantName",variantName},{"fileName",fileName},{"authorId",authorId},{"id", id}, {"remoteSubmission",remoteSubmission}, {"title", title}, {"author", author}, {"hero", hero}, {"kind", kind},
        {"status", status}, {"fixEnabled", fixEnabled}, {"enabled", enabled}, {"appliedEnabled", appliedEnabled},
        {"available", available}, {"pendingApply",pendingApply},{"pendingStatus",pendingStatus},{"pendingReason",pendingReason},{"selectionSaved",selectionSaved},{"selectionReady",selectionReady},{"linkEligible",linkEligible},{"linkReason",linkReason},{"optionCount",optionCount},{"selectedOptionIds",selectedOptionIds}, {"unavailableReason", unavailableReason}, {"installOrder", installOrder},
        {"installedVpks", installedVpks}, {"missingVpks", missingVpks}, {"fonts",fonts}, {"fontDiagnostics",fontDiagnostics}, {"fontStatus",fontStatus}, {"fontInstall",fontInstall}, {"fontOwned",fontOwned},
        {"images", localPreview.isEmpty() ? images : QStringList{localPreview}}, {"localArtwork",localArtwork}, {"previewHidden", previewHidden}};
}

bool LibrarySnapshot::decode(const QJsonObject &object, LibrarySnapshot &result) {
    const bool native = object.value("sourceKind") == QJsonValue("native");
    const auto version = object.value("storeVersion").toInteger(-1);
    if ((!native && object.contains("sourceKind") && object.value("sourceKind") != QJsonValue("upstream"))
        || object.value("protocolVersion").toInteger(-1) != 1 || (native ? version != 1 && version != 2 && version != 3 && version != 4 : version != 33)
        || !fields(object, {"sourcePath"})
        || !(object.value("activeProfileId").isString() || object.value("activeProfileId").isNull())
        || !(object.value("selectedProfileId").isString() || object.value("selectedProfileId").isNull())
        || !object.value("profiles").isArray() || !object.value("mods").isArray()
        || !object.value("warnings").isArray()) return false;
    LibrarySnapshot snapshot;
    if (native) {
        const auto revision = object.value("revision").toString();
        bool validRevision = false; const auto number = revision.toULongLong(&validRevision);
        if (!object.value("revision").isString() || !validRevision || QString::number(number) != revision
            || !object.value("pendingChanges").isBool() || !object.value("canRestore").isBool()
            || !object.value("recoveryRequired").isBool() || !object.value("gamePath").isString()
            || !(object.value("appliedProfileId").isString() || object.value("appliedProfileId").isNull())) return false;
        snapshot.sourceKind = "native"; snapshot.revision = revision;
        snapshot.gamePath = object.value("gamePath").toString();
        if(object.contains("storageRoot") && !object["storageRoot"].isNull() && (!object["storageRoot"].isString() || !object["storageRoot"].toString().startsWith('/') || object["storageRoot"].toString().contains(QChar::Null)))return false;
        snapshot.storageRoot = object.value("storageRoot").toString();
        snapshot.appliedProfileId = object.value("appliedProfileId").toString();
        snapshot.launchFingerprint=object.value("launchFingerprint").toObject().toVariantMap();
        snapshot.fixAutomatically=object.value("fixAutomatically").toBool(true);
        snapshot.pendingChanges = object.value("pendingChanges").toBool();
        const auto pending=[](const QJsonObject &o){return QStringList{"current","pending","unknown","selectionRequired"}.contains(o["pendingStatus"].toString())&&(o["pendingReason"].isNull()||o["pendingReason"].isString());};
        if(object.contains("bisect")){
            const auto b=object["bisect"].toObject();
            if(!object["bisect"].isObject() || !b["active"].isBool())return false;
            if(b["active"].toBool() && (!b["profileId"].isString() || !b["round"].isDouble() || !b["suspectsCount"].isDouble() || !b["testedMods"].isArray() || !b["result"].isArray() || !b["done"].isBool() || !b["profileChanged"].isBool() || !b["expectedRemainingLaunches"].isDouble() || !b["maximumRemainingLaunches"].isDouble()))return false;
            snapshot.bisect=b.toVariantMap();
        }
        if(object.contains("pendingStatus")){if(!pending(object))return false;snapshot.pendingStatus=object["pendingStatus"].toString();snapshot.pendingReason=object["pendingReason"].toString();}

        snapshot.canRestore = object.value("canRestore").toBool();
        snapshot.recoveryRequired = object.value("recoveryRequired").toBool();
        if(object.contains("launchBarrier")) {
            // Malformed optional metadata remains raw recovery evidence; it grants no exemption.
            const auto barrier=object["launchBarrier"].toObject();
            if(LaunchData::barrier(barrier,barrier["rootPath"].toString())) snapshot.launchBarrier=barrier;
        }
        if (version == 3 || version == 4) {
            if ((object["packageVersion"] != QJsonValue(1) && object["packageVersion"] != QJsonValue(2))
                || !FontData::global(object["fonts"],snapshot.gamePath,snapshot.canUndoFonts,snapshot.fontOwners)) return false;
            auto ownerMods = snapshot.fontOwners.keys();
            std::sort(ownerMods.begin(),ownerMods.end());
            for(const auto &mod:ownerMods) {
                QSet<QString> ids;
                for(const auto &owner:snapshot.fontOwners.value(mod)) {
                    auto row=owner.toMap();
                    ids.insert(row.value("fontId").toString());
                    row.insert("modId",mod);
                    snapshot.fontOwnerRows.append(row);
                }
                snapshot.fontOwnerIds.insert(mod,ids);
            }
        } else if (object.contains("fonts") || object.contains("packageVersion")) return false;
    }
    snapshot.sourcePath = object.value("sourcePath").toString();
    snapshot.activeProfileId = object.value("activeProfileId").toString();
    snapshot.selectedProfileId = object.value("selectedProfileId").toString();
    QSet<QString> profileIds, modIds;
    for (const auto &value : object.value("profiles").toArray()) {
        if (!value.isObject()) return false;
        const auto profile = value.toObject();
        const auto id = profile.value("id").toString();
        if (!fields(profile, {"id", "name"}) || id.isEmpty() || profileIds.contains(id)
            || !profile.value("isActive").isBool() || !integer(profile.value("modCount"))
            || profile.value("modCount").toDouble() < 0) return false;
        profileIds.insert(id);
        snapshot.profiles.append(QVariantMap{{"id", id}, {"name", profile.value("name").toString()},
            {"isActive", profile.value("isActive").toBool()}, {"modCount", profile.value("modCount").toVariant()}, {"protected", profile.value("protected").toBool()}});
    }
    if (!(profileIds.isEmpty() && snapshot.selectedProfileId.isEmpty()) && !profileIds.contains(snapshot.selectedProfileId)) return false;
    for (const auto &value : object.value("mods").toArray()) {
        if (!value.isObject()) return false;
        const auto mod = value.toObject();
        const auto id = mod.value("id").toString();
        const auto order = mod.value("installOrder");
        LibraryMod row;
        if (!fields(mod, {"id", "remoteId", "title", "author", "hero", "category", "status"})
            || id.isEmpty() || modIds.contains(id) || !mod.value("enabled").isBool()
            || (!order.isNull() && !integer(order))
            || !strings(mod.value("installedVpks"), row.installedVpks)
            || !strings(mod.value("missingVpks"), row.missingVpks)) return false;
        if (native) {
            if (!mod.value("appliedEnabled").isBool() || !mod.value("available").isBool()
                || !(mod.value("unavailableReason").isString() || mod.value("unavailableReason").isNull())) return false;
            row.appliedEnabled = mod.value("appliedEnabled").toBool(); row.available = mod.value("available").toBool();
            row.unavailableReason = mod.value("unavailableReason").toString();
            row.pendingApply=mod["enabled"].toBool()!=row.appliedEnabled;
            row.selectionReady=row.available;
            if(mod.contains("linkEligible")) {if(!mod["linkEligible"].isBool() || !mod["linkReason"].isString())return false;row.linkEligible=mod["linkEligible"].toBool();row.linkReason=mod["linkReason"].toString();}
            if(mod.contains("pendingStatus")) {
                if(!QStringList{"current","pending","unknown","selectionRequired"}.contains(mod["pendingStatus"].toString()) || (!mod["pendingReason"].isNull()&&!mod["pendingReason"].isString()) || !mod["pendingApply"].isBool() || !mod["selectionSaved"].isBool())return false;
                row.pendingStatus=mod["pendingStatus"].toString();row.pendingReason=mod["pendingReason"].toString();row.pendingApply=mod["pendingApply"].toBool();row.selectionSaved=mod["selectionSaved"].toBool();
            }
            if(mod.contains("selectionReady")) {if(!mod["selectionReady"].isBool())return false;row.selectionReady=mod["selectionReady"].toBool();}
            if(mod.contains("optionCount")) {
                if(!mod["optionCount"].isNull()&&(!integer(mod["optionCount"])||mod["optionCount"].toDouble()<0||mod["optionCount"].toDouble()>1024))return false;
                row.optionCount=mod["optionCount"].toVariant();
                if(!mod["selectedOptionIds"].isNull()&&!strings(mod["selectedOptionIds"],row.selectedOptionIds))return false;
                if(row.selectedOptionIds.size()>99)return false;
                QString last;for(const auto &id:row.selectedOptionIds){if(!FontData::hex(id,32)||(!last.isEmpty()&&id<=last))return false;last=id;}
            }
        }
        for (const auto *key : {"addedAt", "updatedAt"}) {
            const auto value = mod.value(QLatin1String(key));
            if (!value.isUndefined() && !value.isNull()) {
                if (!value.isString() || value.toString().size() > 40 || !QDateTime::fromString(value.toString(), Qt::ISODate).isValid()) return false;
                (QString::fromLatin1(key) == "addedAt" ? row.addedAt : row.updatedAt) = value.toString();
            }
        }
        const auto bytes = mod.value("sizeBytes");
        if (!bytes.isUndefined() && !bytes.isNull()) {
            if (!integer(bytes) || bytes.toDouble() < 0 || bytes.toDouble() > 9007199254740991.0) return false;
            row.sizeBytes = bytes.toVariant();
        }
        for (const auto *key : {"variantName", "fileName", "authorId"}) {
            const auto value = mod.value(QLatin1String(key));
            if (value.isUndefined() || value.isNull()) continue;
            const auto text = value.toString();
            if (!value.isString() || text.size() > 262144 || std::any_of(text.cbegin(), text.cend(), [](QChar c) { return c.category() == QChar::Other_Control; })) return false;
            if (QString::fromLatin1(key) == "variantName") row.variantName = value.toString();
            else if (QString::fromLatin1(key) == "fileName") row.fileName = value.toString();
            else { if (!value.toString().isEmpty() && !QRegularExpression("^[1-9][0-9]{0,19}$").match(value.toString()).hasMatch()) return false; row.authorId = value.toString(); }
        }
        modIds.insert(id);
        // remoteId is the producer’s explicit GameBanana compatibility slug.
        // Local row IDs are never interpreted as provider identities.
        if(native && mod.contains("remoteSubmission")) {
            if(!mod["remoteSubmission"].isNull()) {
                const auto key=SubmissionKey::decode(mod["remoteSubmission"].toObject());if(!key)return false;
                row.remoteSubmission=key->presentation();
            }
        } else if(const auto key=SubmissionKey::compatibilityReference(mod["remoteId"].toString()))row.remoteSubmission=key->presentation();
        row.id = id; row.title = mod.value("title").toString(); row.author = mod.value("author").toString();
        row.hero = mod.value("hero").toString(); row.kind = mod.value("category").toString();
        if(mod.contains("heroDetectionEligible") && !mod["heroDetectionEligible"].isBool())return false;
        row.heroDetectionEligible=mod["heroDetectionEligible"].toBool();
        row.fixEnabled=mod.value("fixEnabled").toBool(true);
        row.status = mod.value("status").toString(); row.enabled = mod.value("enabled").toBool();
        if (mod.contains("fonts")) {
            double bytes = 0;
            if (!native || (version != 3 && version != 4) || !FontData::refs(mod["fonts"],row.fonts,bytes)) return false;
        }
        if (mod.contains("fontDiagnostics")) {
            if (!native || (version != 3 && version != 4) || !FontData::candidates(mod["fontDiagnostics"],true)) return false;
            row.fontDiagnostics=mod["fontDiagnostics"].toArray().toVariantList();
        }
        row.fontStatus=snapshot.fontOwners.value(id);
        row.fontOwned=row.fontStatus;
        QHash<QString,QVariantMap> owned;
        for (const auto &f:row.fontStatus) owned.insert(f.toMap().value("fontId").toString(),f.toMap());
        for (const auto &f:row.fonts) {
            const auto ref=f.toMap(); const auto asset=ref.value("asset").toMap();
            const auto assetId=asset.value("id").toString();
            auto status=owned.value(assetId,QVariantMap{{"fontId",assetId},{"targetName",ref.value("targetName")},{"displayName",ref.value("displayName")},{"status","stashed"},{"ownerCount",0}});
            if (!owned.contains(assetId)) row.fontStatus.append(status);
            status.insert("origins",ref.value("origins")); row.fontInstall.append(status);
        }
        if(!row.fonts.isEmpty()) snapshot.fontOfferMods.insert(id,QVariantMap{{"title",row.title},{"fontInstall",row.fontInstall}});
        row.installOrder = order.toVariant();
        if (native && mod.contains("localArtwork") && !mod.value("localArtwork").isNull()) {
            if (!mod.value("localArtwork").isObject() || !LocalArtworkProvider::validOwned(mod.value("localArtwork").toObject())) return false;
            row.localArtwork = mod.value("localArtwork").toObject().toVariantMap();
        }
        row.images = previews(mod.value("images"));
        row.previewHidden = mod.value("previewHidden").toBool(false);
        snapshot.mods.append(std::move(row));
    }
    for (const auto &value : object.value("warnings").toArray()) {
        if (!value.isObject() || !fields(value.toObject(), {"code", "message"})) {
            qWarning() << "Ignoring unsupported library warning shape" << value.type();
            continue;
        }
        const auto warning = value.toObject();
        snapshot.warningCodes.append(warning.value("code").toString());
        snapshot.warnings.append(warning.value("message").toString());
    }
    result = std::move(snapshot);
    return true;
}

int LibraryRows::rowCount(const QModelIndex &parent) const { return parent.isValid() ? 0 : m_mods.size(); }
QVariant LibraryRows::data(const QModelIndex &index, int role) const {
    if (!index.isValid() || index.column() != 0 || index.row() < 0 || index.row() >= m_mods.size()) return {};
    const auto &mod = m_mods[index.row()];
    switch (role) {
    case AddedAt: return mod.addedAt;
    case UpdatedAt: return mod.updatedAt;
    case SizeBytes: return mod.sizeBytes;
    case VariantName: return mod.variantName;
    case FileName: return mod.fileName;
    case AuthorId: return effectiveDetails(mod).value("authorId");
    case ModId: return mod.id;
    case Title: return mod.title;
    case Hero: return mod.hero;
    case Kind: return mod.kind;
    case ModEnabled: return mod.enabled;
    case Author: return mod.author;
    case Status: return mod.status;
    case ModPreview: {
        if(!mod.localPreview.isEmpty())return mod.localPreview;
        if(!mod.images.isEmpty())return mod.images.value(0);
        if(m_media&&mod.localArtwork.isEmpty()){
            const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(mod.remoteSubmission));
            if(key){const auto media=m_media->projection(mod.id,*key);if(media)return media->images.value(0);}
        }
        return QString();
    }
    case ModPreviewHidden: {
        if(mod.previewHidden)return true;
        if(m_media&&mod.localArtwork.isEmpty()&&mod.localPreview.isEmpty()&&mod.images.isEmpty()){
            const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(mod.remoteSubmission));
            if(key){const auto media=m_media->projection(mod.id,*key);if(media)return media->masked;}
        }
        return false;
    }
    case ModAppliedEnabled: return mod.appliedEnabled;
    case ModAvailable: return mod.available;
    case ModUnavailableReason: return mod.unavailableReason;
    case ModSelectionReady:return mod.selectionReady;
    case ModPendingStatus:return mod.pendingStatus;
    case ModPendingApply:return mod.pendingApply;
    case ModSelectionSaved:return mod.selectionSaved;
    case ModUpdateStatus:return m_updates.value(mod.id).value("status").toString();
    case ModHealthStatus:return m_health.value(mod.id).value("status");
    case ModHealthSymptoms:return m_health.value(mod.id).value("symptoms");
    case MediaRevision:return QVariant::fromValue(mod.mediaRevision);
    default: return {};
    }
}
QHash<int, QByteArray> LibraryRows::roleNames() const {
    return {{AddedAt,"addedAt"},{UpdatedAt,"updatedAt"},{SizeBytes,"sizeBytes"},{VariantName,"variantName"},{FileName,"fileName"},{AuthorId,"authorId"},{ModId, "modId"}, {Title, "modTitle"}, {Hero, "heroName"}, {Kind, "modKind"},
        {ModEnabled, "modEnabled"}, {Author, "modAuthor"}, {Status, "modStatus"},
        {ModPreview, "modPreview"}, {ModPreviewHidden, "modPreviewHidden"},
        {ModAppliedEnabled, "modAppliedEnabled"}, {ModAvailable, "modAvailable"}, {ModUnavailableReason, "modUnavailableReason"},{ModSelectionReady,"modSelectionReady"},{ModPendingStatus,"modPendingStatus"},{ModPendingApply,"modPendingApply"},{ModSelectionSaved,"modSelectionSaved"},{MediaRevision,"mediaRevision"},{ModUpdateStatus,"modUpdateStatus"},{ModHealthStatus,"modHealthStatus"},{ModHealthSymptoms,"modHealthSymptoms"}};
}
void LibraryRows::replace(QList<LibraryMod> mods) {
    const bool sameRows=mods.size()==m_mods.size() && std::equal(mods.cbegin(),mods.cend(),m_mods.cbegin(),
        [](const LibraryMod &a,const LibraryMod &b){return a.id==b.id;});
    for(auto &mod:mods) {
        mod.mediaFingerprint=mediaFingerprint(mod);
        const auto old=m_byId.constFind(mod.id);
        if(old!=m_byId.cend()) {
            const auto &previous=m_mods[*old];
            mod.mediaRevision=previous.mediaRevision+(mod.mediaFingerprint!=previous.mediaFingerprint);
        }
    }
    if(sameRows && mods==m_mods)return;
    QList<std::pair<int,QList<int>>> changes;
    if(sameRows) {
        const auto roles=roleNames();
        for(int i=0;i<mods.size();++i) {
            if(mods[i]==m_mods[i])continue;
            QHash<int,QVariant> before;
            for(auto role=roles.cbegin();role!=roles.cend();++role)before.insert(role.key(),data(index(i),role.key()));
            m_mods[i]=std::move(mods[i]);
            QList<int> changed;
            for(auto role=roles.cbegin();role!=roles.cend();++role)
                if(before.value(role.key())!=data(index(i),role.key()))changed.append(role.key());
            changes.append({i,changed});
        }
    } else {
        beginResetModel();m_mods=std::move(mods);
    }
    m_byId.clear(); m_enabledCount = 0; m_categories.clear();
    QSet<QString> categories, heroes, types;
    for (int i = 0; i < m_mods.size(); ++i) {
        auto &mod = m_mods[i];
        m_byId.insert(mod.id, i);
        if (mod.enabled) ++m_enabledCount;
        categories.insert(mod.kind);
        if(!mod.hero.isEmpty()) categories.insert(mod.hero);
        if(!mod.hero.isEmpty()) heroes.insert(mod.hero);
        if(!mod.kind.isEmpty() && mod.kind.compare("General", Qt::CaseInsensitive) != 0 && mod.kind.compare("Uncategorized", Qt::CaseInsensitive) != 0) types.insert(mod.kind);
    }
    m_categories = categories.values();
    m_categories.sort(Qt::CaseInsensitive);
    m_heroes = heroes.values(); m_heroes.sort(Qt::CaseInsensitive);
    m_types = types.values(); m_types.sort(Qt::CaseInsensitive);
    if(!sameRows)endResetModel();
    else for(const auto &[row,roles]:changes)emit dataChanged(index(row),index(row),roles);
}
bool LibraryRows::setDetectedHero(const QString &id, const QString &hero) {
    const int row=m_byId.value(id,-1);
    if(row<0 || hero.isEmpty() || !m_mods[row].heroDetectionEligible)return false;
    m_mods[row].hero=hero;m_mods[row].heroDetectionEligible=false;
    if(!m_categories.contains(hero)){m_categories.append(hero);m_categories.sort(Qt::CaseInsensitive);}
    if(!m_heroes.contains(hero)){m_heroes.append(hero);m_heroes.sort(Qt::CaseInsensitive);}
    emit dataChanged(index(row),index(row),{Hero});return true;
}
bool LibraryRows::setEnabled(const QString &id, bool enabled) {
    const auto found = m_byId.constFind(id);
    if (found == m_byId.cend() || m_mods[*found].enabled == enabled) return false;
    auto &mod = m_mods[*found]; mod.enabled = enabled;
    m_enabledCount += enabled ? 1 : -1;
    emit dataChanged(index(*found), index(*found), {ModEnabled});
    return true;
}
QVariantMap LibraryRows::details(const QString &id) const {
    const auto found = m_byId.constFind(id);
    return found == m_byId.cend() ? QVariantMap{} : effectiveDetails(m_mods[*found]);
}
QVariantMap LibraryRows::homeRecord(int row) const {
    const auto idx = index(row);
    const auto image = data(idx, ModPreview).toString();
    return {{"id", m_mods[row].id}, {"title", m_mods[row].title},
            {"images", image.isEmpty() ? QStringList{} : QStringList{image}},
            {"previewHidden", data(idx, ModPreviewHidden).toBool()}};
}
QVariantList LibraryRows::homeEnabled() const {
    QVariantList result;
    for (int i = 0; i < m_mods.size() && result.size() < 4; ++i)
        if (m_mods[i].enabled) result.append(homeRecord(i));
    return result;
}
QVariantList LibraryRows::homeRecent() const {
    struct Candidate { int row; QDateTime added; };
    QList<Candidate> best;
    for (int i = 0; i < m_mods.size(); ++i) {
        if (m_mods[i].addedAt.isEmpty()) continue;
        const Candidate candidate{i, QDateTime::fromString(m_mods[i].addedAt, Qt::ISODate)};
        auto at = best.begin();
        while (at != best.end() && !(candidate.added > at->added)) ++at;
        if (at != best.end() || best.size() < 5) {
            best.insert(at, candidate);
            if (best.size() > 5) best.removeLast();
        }
    }
    QVariantList result;
    for (const auto &candidate : best) result.append(homeRecord(candidate.row));
    return result;
}
std::optional<SubmissionKey> LibraryRows::mediaSubmission(const QString &id) const {
    const auto found=m_byId.constFind(id);if(found==m_byId.cend())return {};
    const auto &mod=m_mods[*found];
    return SubmissionKey::decode(QJsonObject::fromVariantMap(mod.remoteSubmission));
}
QVariantMap LibraryRows::effectiveDetails(const LibraryMod &mod) const {
    auto result=mod.details();
    result.insert("update", m_updates.value(mod.id));
    result.insert("health", m_health.value(mod.id));
    if(!m_media)return result;
    const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(mod.remoteSubmission));
    if(!key)return result;
    const auto media=m_media->projection(mod.id,*key);
    if(media){
        if(!media->authorId.isEmpty())result.insert("authorId",media->authorId);
        if(mod.localArtwork.isEmpty() && mod.localPreview.isEmpty() && mod.images.isEmpty()) {
            result.insert("images",media->images);result.insert("previewHidden",mod.previewHidden||media->masked);
        }
    }
    return result;
}
QByteArray LibraryRows::mediaFingerprint(const LibraryMod &mod) const {
    if(!m_media)return {};
    const auto key=SubmissionKey::decode(QJsonObject::fromVariantMap(mod.remoteSubmission));if(!key)return {};
    const auto media=m_media->projection(mod.id,*key);if(!media)return {};
    QJsonArray value;value.append(media->authorId);value.append(mod.previewHidden||media->masked);for(const auto &url:media->images)value.append(url);
    return QCryptographicHash::hash(QJsonDocument(value).toJson(QJsonDocument::Compact),QCryptographicHash::Sha256);
}
bool LibraryRows::notifyMediaChanged(const QString &id){
    const auto found=m_byId.constFind(id);if(found==m_byId.cend())return false;
    auto &mod=m_mods[*found];const auto fingerprint=mediaFingerprint(mod);
    if(mod.mediaFingerprint==fingerprint)return false;
    mod.mediaFingerprint=fingerprint;++mod.mediaRevision;
    emit dataChanged(index(*found),index(*found),{ModPreview,ModPreviewHidden,MediaRevision});return true;
}

void LibraryRows::setHealth(const QVariantMap &report) {
    QHash<QString,QVariantMap> health;
    for(const auto &v:report.value("mods").toList()){const auto row=v.toMap();health.insert(row.value("modId").toString(),row);}
    const auto previous=std::exchange(m_health,std::move(health));
    for(int i=0;i<m_mods.size();++i) {
        const auto before=previous.value(m_mods[i].id),after=m_health.value(m_mods[i].id);
        QList<int> roles;
        if(before.value("status")!=after.value("status"))roles.append(ModHealthStatus);
        if(before.value("symptoms")!=after.value("symptoms"))roles.append(ModHealthSymptoms);
        if(!roles.isEmpty())emit dataChanged(index(i),index(i),roles);
    }
}

bool LibraryRows::setUpdates(const QVariantList &rows) {
    QHash<QString,QVariantMap> updates;
    for(const auto &v:rows){const auto row=v.toMap();updates.insert(row.value("modId").toString(),row);}
    const auto previous=std::exchange(m_updates,std::move(updates));
    bool membershipChanged=false;
    for(int i=0;i<m_mods.size();++i) {
        const auto before=previous.value(m_mods[i].id).value("status").toString();
        const auto after=m_updates.value(m_mods[i].id).value("status").toString();
        if(before==after)continue;
        membershipChanged|=(before=="available")!=(after=="available");
        emit dataChanged(index(i),index(i),{ModUpdateStatus});
    }
    return membershipChanged;
}
