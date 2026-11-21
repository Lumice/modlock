#include "catalogdata.h"
#include "launchdata.h"
#include <QJsonArray>
#include <QJsonDocument>
#include <QSet>
#include <QUrl>
#include <algorithm>
#include <cmath>
#include <limits>

namespace {
bool keys(const QJsonObject &o, std::initializer_list<const char *> names) {
    return o.size() == qsizetype(names.size()) && std::all_of(names.begin(), names.end(), [&o](auto k) { return o.contains(QLatin1String(k)); });
}
bool text(const QJsonValue &v, qsizetype cap, bool nullable = false) {
    return (nullable && v.isNull()) || (v.isString() && v.toString().toUtf8().size() <= cap && !v.toString().contains(QChar::Null));
}
bool one(const QJsonValue &v, const char *values) { return v.isString() && QString::fromLatin1(values).split('|').contains(v.toString()); }
bool number(const QJsonValue &v, qint64 low, qint64 high) { return v.isDouble() && std::isfinite(v.toDouble()) && v.toDouble() == double(v.toInteger(-1)) && v.toInteger(-1) >= low && v.toInteger(-1) <= high; }
bool decimal(const QJsonValue &v, bool positive = false) {
    if (!v.isString()) return false;
    const auto s = v.toString(); bool ok = false;
    const auto n = s.toULongLong(&ok);
    return ok && s == QString::number(n) && (!positive || n != 0);
}
bool timestamp(const QJsonValue &v) { return v.isNull() || number(v, 0, 253402300799LL); }
bool https(const QJsonValue &v, bool nullable = false, int limit = 7000) {
    if (nullable && v.isNull()) return true;
    if (!text(v, limit)) return false;
    const auto s = v.toString();
    if (std::any_of(s.cbegin(), s.cend(), [](QChar c) { return c.unicode() <= 32 || c.unicode() >= 127 || c == '\\' || c == '@'; })) return false;
    if(!s.startsWith("https://"))return false;
    const auto authority=s.mid(8).section('/',0,0).section('?',0,0).section('#',0,0);
    if(authority.isEmpty()||authority.startsWith('.')||authority.endsWith('.')||std::any_of(authority.cbegin(),authority.cend(),[](QChar c){return !((c>='a'&&c<='z')||(c>='A'&&c<='Z')||(c>='0'&&c<='9')||c=='.'||c=='-');}))return false;
    const QUrl u(s, QUrl::StrictMode);
    return u.isValid() && u.scheme() == "https" && !u.host().isEmpty() && u.userInfo().isEmpty() && !u.authority().contains('@');
}
// Validate again at the process boundary, including aggregate work limits.
bool richDescription(const QJsonValue &value) {
    if (!value.isArray() || value.toArray().size() > 256 || QJsonDocument(value.toArray()).toJson(QJsonDocument::Compact).size()>256*1024) return false;
    int count=0, bytes=0, images=0;
    auto link=[](const QJsonValue &v) { auto s=v.toString(); return https(s.startsWith("http://") ? QJsonValue("https://"+s.mid(7)) : v,false,2048) && !s.contains('"') && !s.contains('\'') && !s.contains('<') && !s.contains('>'); };
    auto runs=[&](const QJsonValue &v) {
        if (!v.isArray()) return false;
        for (const auto &x:v.toArray()) {
            if (!x.isObject() || ++count>2048) return false;
            auto r=x.toObject();
            if (!text(r["text"],65536)) return false;
            bytes+=r["text"].toString().toUtf8().size(); if(bytes>65536)return false;
            r.remove("text");
            for(auto key:{"bold","italic","underline","strike","code"})if(r.contains(key)){if(!r[key].isBool())return false;r.remove(key);}
            if(r.contains("color")){const auto c=r.take("color").toString();if(c.size()!=7||c[0]!='#'||!std::all_of(c.cbegin()+1,c.cend(),[](QChar c){return (c>='0'&&c<='9')||(c>='a'&&c<='f');}))return false;}
            if(r.contains("link")&&!link(r.take("link")))return false;
            if(!r.isEmpty())return false;
        }
        return true;
    };
    for(const auto &x:value.toArray()) {
        if(!x.isObject())return false;auto b=x.toObject();const auto type=b.take("type").toString();
        if(type=="paragraph"||type=="quote"||type=="heading") {
            if(type=="heading"&&!number(b.take("level"),1,3))return false;
            if(!runs(b.take("runs")))return false;
        } else if(type=="list") {
            if(!b.take("ordered").isBool())return false;
            const auto items=b.take("items");if(!items.isArray()||items.toArray().size()>2048)return false;
            for(const auto &item:items.toArray())if(!runs(item))return false;
        } else if(type=="image") {
            if(++images>16)return false;const auto u=b.take("url");
            if(!https(u,false,2048)||!(u.toString().startsWith("https://images.gamebanana.com/")||u.toString().startsWith("https://puu.sh/"))||!text(b.take("alt"),4096))return false;
            for(auto key:{"width","height"})if(b.contains(key)&&!number(b.take(key),1,8192))return false;
        } else if(type!="rule")return false;
        if(!b.isEmpty())return false;
    }
    return true;
}
bool strings(const QJsonValue &v, int count, int bytes, bool urls = false, bool unique = false) {
    if (!v.isArray() || v.toArray().size() > count) return false;
    QSet<QString> seen;
    for (const auto &s : v.toArray()) {
        if (!(urls ? https(s, false, bytes) : text(s, bytes)) || (unique && (s.toString().isEmpty() || seen.contains(s.toString())))) return false;
        seen.insert(s.toString());
    }
    return true;
}
bool error(const QJsonValue &v) {
    const auto o = v.toObject();
    return v.isObject() && keys(o, {"kind","message","retryAfterSeconds"})
        && one(o["kind"], "catalogCancelled|catalogUnavailable|catalogStorage|catalogFull|catalogIncomplete|catalogSearchUnavailable|catalogPrivate|catalogNotFound|catalogInvalidProviderResponse|catalogResponseTooLarge|catalogRateLimited|catalogRefused|catalogTransport|catalogStalePreferences")
        && text(o["message"], 256) && (o["retryAfterSeconds"].isNull() || number(o["retryAfterSeconds"], 0, 86400));
}
bool summary(const QJsonValue &v, CatalogSummary &s) {
    const auto o = v.toObject();
    const auto k = SubmissionKey::decode(o["submission"]);
    if (!v.isObject() || !keys(o, {"submission","name","description","author","authorRemoteId","profileUrl","category","hero","isAudio","isMap","isNsfw","visibilityKnown","isObsolete","hasFiles","downloadCount","likes","images","thumbnailUrl","audioUrl","tags","remoteAddedAt","remoteUpdatedAt","filesUpdatedAt","developmentState","completionPercentage","hydrated"})
        || !k || !text(o["name"],1024) || !text(o["description"],128*1024) || !text(o["author"],1024) || (!o["authorRemoteId"].isNull() && !decimal(o["authorRemoteId"],true))
        || o["profileUrl"] != QJsonValue(k->source()) || !text(o["category"],128) || !text(o["hero"],128,true)
        || !decimal(o["downloadCount"]) || !decimal(o["likes"]) || !strings(o["images"],32,7000,true) || !https(o["thumbnailUrl"],true) || !https(o["audioUrl"],true)
        || !strings(o["tags"],64,256) || !timestamp(o["remoteAddedAt"]) || !timestamp(o["remoteUpdatedAt"]) || !timestamp(o["filesUpdatedAt"])
        || !text(o["developmentState"],128,true) || (!o["completionPercentage"].isNull() && !number(o["completionPercentage"],0,100))
        || QJsonDocument(o).toJson(QJsonDocument::Compact).size()>192*1024) return false;
    for (auto b : {"isAudio","isMap","isNsfw","visibilityKnown","isObsolete","hasFiles","hydrated"}) if (!o[b].isBool()) return false;
    if ((!o["visibilityKnown"].toBool() && !o["isNsfw"].toBool()) || o["isAudio"].toBool() != (k->type == "sound") || o["isMap"].toBool() != (k->type == "mod" && o["category"] == QJsonValue("Maps"))) return false;
    s.key=*k; s.title=o["name"].toString(); s.description=o["description"].toString(); s.author=o["author"].toString(); s.authorId=o["authorRemoteId"].toString();
    s.category=o["category"].toString(); s.hero=o["hero"].toString(); s.thumbnail=o["thumbnailUrl"].toString(); s.audio=o["audioUrl"].toString(); s.source=k->source();
    for(const auto &i:o["images"].toArray()) s.images.append(i.toString());
    for(const auto &i:o["tags"].toArray()) s.tags.append(i.toString());
    s.nsfw=o["isNsfw"].toBool(); s.visibilityKnown=o["visibilityKnown"].toBool(); s.obsolete=o["isObsolete"].toBool(); s.hydrated=o["hydrated"].toBool();
    s.metadata={{"downloads",o["downloadCount"].toString()},{"likes",o["likes"].toString()},{"addedAt",o["remoteAddedAt"].toVariant()},{"updatedAt",o["remoteUpdatedAt"].toVariant()},{"filesUpdatedAt",o["filesUpdatedAt"].toVariant()},{"developmentState",o["developmentState"].toVariant()},{"completionPercentage",o["completionPercentage"].toVariant()},{"hasFiles",o["hasFiles"].toBool()}};
    return true;
}
CatalogRow row(const CatalogSummary &s) {
    return {s.key.identity(),s.title,s.author,s.category,s.hero,s.thumbnail,"ready",{},s.key,false,s.nsfw||!s.visibilityKnown,s.favoriteKnown,s.favorite,s.presentation()};
}
bool links(const QJsonValue &v) {
    if (!v.isArray() || v.toArray().size()>32) return false;
    QSet<QString> seen;
    for (const auto &x:v.toArray()) { const auto o=x.toObject(); if(!x.isObject()||!keys(o,{"label","url"})||!text(o["label"],256)||!https(o["url"],false,2048)||seen.contains(o["url"].toString()))return false; seen.insert(o["url"].toString()); }
    return true;
}
bool header(const QJsonValue &v, QVariantMap &p) {
    const auto o=v.toObject();
    if(!v.isObject()||!keys(o,{"collectionId","name","description","text","profileUrl","coverUrl","author","authorRemoteId","avatarUrl","itemCount","likes","isNsfw","contentRatingKnown","remoteAddedAt","remoteUpdatedAt","itemsSyncedAt","images","headerFetchedAt","descriptionFetchedAt","descriptionRevision","textFormat"})
       ||!decimal(o["collectionId"],true)||!text(o["name"],1024)||!text(o["description"],128*1024)||o["text"]!=QJsonValue("")||o["textFormat"]!=QJsonValue("plain")
       ||o["profileUrl"]!=QJsonValue("https://gamebanana.com/collections/"+o["collectionId"].toString())||!https(o["coverUrl"],true)||!https(o["avatarUrl"],true)
       ||!text(o["author"],1024)||(!o["authorRemoteId"].isNull()&&!decimal(o["authorRemoteId"],true))||!decimal(o["itemCount"])||!decimal(o["likes"])||!o["isNsfw"].isBool()||!o["contentRatingKnown"].isBool()
       ||!strings(o["images"],32,7000,true)||!o["itemsSyncedAt"].isNull()||(!o["contentRatingKnown"].toBool()&&!o["isNsfw"].toBool())||QJsonDocument(o).toJson(QJsonDocument::Compact).size()>512*1024)return false;
    for(auto t:{"remoteAddedAt","remoteUpdatedAt","itemsSyncedAt","headerFetchedAt","descriptionFetchedAt","descriptionRevision"})if(!timestamp(o[t]))return false;
    p=o.toVariantMap(); return true;
}
bool favorites(const QJsonValue &v, QList<CatalogRow> &rows) {
    if(!v.isArray()||v.toArray().size()>100)return false;
    QHash<QString,bool> values;
    for(const auto &x:v.toArray()){const auto o=x.toObject();const auto k=SubmissionKey::decode(o["submission"]);if(!x.isObject()||!keys(o,{"submission","favorite"})||!k||!o["favorite"].isBool()||values.contains(k->identity()))return false;values.insert(k->identity(),o["favorite"].toBool());}
    QSet<QString> expected;
    for(auto &r:rows)if(r.key.valid()){expected.insert(r.key.identity());if(!values.contains(r.key.identity()))return false;r.favoriteKnown=true;r.favorite=values.value(r.key.identity());r.metadata.insert("favoriteKnown",true);r.metadata.insert("favorite",r.favorite);}
    return expected.size()==values.size();
}
bool provenance(const QJsonValue &v, QVariantMap &p) {
    const auto original=v.toObject();
    auto o=original;
    const auto progress=o.take("syncProgress");
    const auto completed=o.take("lastCompletedAt");
    if(!completed.isUndefined()&&!timestamp(completed))return false;
    if(!progress.isUndefined()&&!progress.isNull()){
        const auto p=progress.toObject();
        if(!progress.isObject()||!keys(p,{"stage","completed","total"})||!text(p["stage"],64)||!decimal(p["completed"])||(!p["total"].isNull()&&!decimal(p["total"])))return false;
    }
    const auto a=o["accounting"].toObject();
    if(!v.isObject()||!keys(o,{"source","sourcePin","fetchedAt","lastSuccessfulSyncAt","stale","incomplete","refreshError","searchAvailable","searchState","accounting"})
       ||!one(o["source"],"cache|network|mixed")||o["sourcePin"]!=QJsonValue("976637e2d63329ffef6f87eca40e2aea681acc94")||!timestamp(o["fetchedAt"])||!timestamp(o["lastSuccessfulSyncAt"])
       ||!o["stale"].isBool()||!o["incomplete"].isBool()||(!o["refreshError"].isNull()&&!error(o["refreshError"]))||!o["searchAvailable"].isBool()||!one(o["searchState"],"unavailable|rebuilding|available")
       ||o["searchAvailable"].toBool()!=(o["searchState"]==QJsonValue("available"))||!o["accounting"].isObject()||!keys(a,{"databaseBytes","sidecarBytes","spoolAndControlBytes","reserveBytes","aggregateQuota"})
       ||!decimal(a["databaseBytes"])||!decimal(a["sidecarBytes"])||!decimal(a["spoolAndControlBytes"])||a["reserveBytes"]!=QJsonValue("1073741824")||a["aggregateQuota"]!=QJsonValue(false))return false;
    p=original.toVariantMap();return true;
}
bool pageCount(const QJsonArray &items,const QJsonValue &total,int page,int size,int base=0) {
    if(!decimal(total))return false;
    const auto count=total.toString().toULongLong();const auto offset=quint64(page-base)*quint64(size);
    return quint64(items.size())==(offset>=count?0:std::min(quint64(size),count-offset));
}
bool refresh(const QJsonObject &o, CatalogDecodedResult &r) { if(!o["refreshRecommended"].isBool()||!one(o["refreshState"],"idle|completed"))return false;r.refreshRecommended=o["refreshRecommended"].toBool();return true; }
bool resource(const QJsonObject &o, const CatalogRequestIdentity &i, CatalogDecodedResult &r) {
    const auto k=SubmissionKey::decode(o["submission"]);const auto expected=SubmissionKey::decode(i.args["submission"]);
    if(!keys(o,{"state","reason","submission","resource","refreshRecommended","refreshState"})||!k||!expected||*k!=*expected||!one(o["state"],"ready|partial|missing|unavailable")||!refresh(o,r))return false;
    r.page=i.args["page"].toInt(1);r.submission=*k;r.state=o["state"].toString();r.detail={{"state",r.state},{"reason",o["reason"].toVariant()},{"submission",k->presentation()}};
    if(r.state=="missing"||r.state=="unavailable")return o["resource"].isNull()&&(r.state=="missing"?o["reason"].isNull():one(o["reason"],"catalogPrivate|catalogNotFound"));
    if(!o["reason"].isNull()||!o["resource"].isObject())return false;
    const auto d=o["resource"].toObject();
    if(QJsonDocument(d).toJson(QJsonDocument::Compact).size()>512*1024)return false;
    if(i.kind=="detail") {
        CatalogSummary s;
        if(!summary(d["summary"],s)||s.key!=*k||d["textFormat"]!=QJsonValue("plain"))return false;
        if(r.state=="partial"){if(!keys(d,{"summary","textFormat"}))return false;r.detail.insert("summary",s.presentation());return true;}
        const auto m=d["metadata"].toObject();
        auto legacy=d; legacy.remove("richDescription");
        if(d.contains("richDescription")&&!richDescription(d["richDescription"]))return false;
        if(!keys(legacy,{"summary","description","links","linksTruncated","textFormat","public","contentWarnings","dependencies","metadata","sourcePin"})||!text(d["description"],128*1024)||!links(d["links"])||!d["linksTruncated"].isBool()||d["public"]!=QJsonValue(true)||d["sourcePin"]!=QJsonValue("976637e2d63329ffef6f87eca40e2aea681acc94")
           ||!d["contentWarnings"].isArray()||d["contentWarnings"].toArray().size()>64||!d["dependencies"].isArray()||d["dependencies"].toArray().size()>64||!d["metadata"].isObject()||!keys(m,{"donationLinks","mapName"})||!text(m["mapName"],128*1024,true)||!m["donationLinks"].isArray()||m["donationLinks"].toArray().size()>32)return false;
        for(const auto &x:d["contentWarnings"].toArray()){const auto w=x.toObject();if(!x.isObject()||!keys(w,{"code","label"})||!text(w["code"],128)||!text(w["label"],1024))return false;}
        for(const auto &x:d["dependencies"].toArray()){const auto w=x.toObject();if(!x.isObject()||!keys(w,{"label","url","remoteId","level"})||!text(w["label"],1024)||!https(w["url"],true)||(!w["remoteId"].isNull()&&!decimal(w["remoteId"],true))||(!w["level"].isNull()&&!one(w["level"],"required|recommended")))return false;}
        for(const auto &x:m["donationLinks"].toArray()){const auto w=x.toObject();if(!x.isObject()||!keys(w,{"url","platform"})||!https(w["url"])||!one(w["platform"],"Ko-fi|Patreon|Buy Me a Coffee|PayPal|Liberapay|Open Collective|GitHub Sponsors"))return false;}
        r.detail=d.toVariantMap();r.detail.insert("summary",s.presentation());r.detail.insert("state",r.state);return true;
    }
    if(r.state!="ready")return false;
    const auto actual=SubmissionKey::decode(d["submission"]);if(!actual||*actual!=*k||!d["items"].isArray()||d["page"]!=i.args["page"])return false;
    r.page=d["page"].toInt();r.total=d["total"].toString();
    if(i.kind=="files") {
        if(!keys(d,{"submission","items","total","page","pageSize","metadataOnly"})||!decimal(d["total"])||d["total"].toString().toULongLong()>128||d["pageSize"]!=QJsonValue(20)||d["metadataOnly"]!=QJsonValue(true)||d["items"].toArray().size()>20)return false;
        QSet<QString> ids;
        for(const auto &x:d["items"].toArray()){const auto f=x.toObject();if(!x.isObject()||!keys(f,{"fileId","name","size","createdAt","downloadUrl","md5Checksum","description","isArchived"})||!decimal(f["fileId"],true)||ids.contains(f["fileId"].toString())||!text(f["name"],512)||!decimal(f["size"])||!timestamp(f["createdAt"])||!https(f["downloadUrl"],true)||(!f["md5Checksum"].isNull()&&!LaunchData::hex(f["md5Checksum"],32))||!text(f["description"],4096)||!f["isArchived"].isBool())return false;ids.insert(f["fileId"].toString());}
        if(!pageCount(d["items"].toArray(),d["total"],r.page,20,1))return false;
        r.pageSize=20;
    } else {
        const auto size=i.kind=="comments"?15:20;
        if(!keys(d,{"submission","kind","page","pageSize","state","items","total","hasMore","textFormat"})||d["kind"]!=QJsonValue(i.kind)||d["pageSize"]!=QJsonValue(size)||!one(d["state"],"ready|hidden")||!d["hasMore"].isBool()||d["textFormat"]!=QJsonValue("plain")||d["items"].toArray().size()>size)return false;
        if(d["state"]==QJsonValue("hidden")){if(i.kind!="comments"||!d["items"].toArray().isEmpty()||!d["total"].isNull()||d["hasMore"]!=QJsonValue(false))return false;}
        else {
            if(!decimal(d["total"]))return false;
            const auto total=d["total"].toString().toULongLong(),offset=quint64(r.page-1)*quint64(size),publicCount=quint64(d["items"].toArray().size());
            if(offset>total||publicCount>total-offset)return false;
            if(i.kind=="comments"&&((d["hasMore"].toBool()&&publicCount==0)||(!d["hasMore"].toBool()&&offset+publicCount!=total)))return false;
        }
        QSet<QString> ids;
        for(const auto &x:d["items"].toArray()) {
            const auto a=x.toObject();
            if(!x.isObject()||!text(a["text"],128*1024)||!links(a["links"])||!a["linksTruncated"].isBool()||!timestamp(a["createdAt"]))return false;
            QString id;
            if(i.kind=="comments"){
                const auto author=a["author"].toObject();const auto score=a["score"].toString();bool ok=false;const auto n=score.toLongLong(&ok);
                if(!keys(a,{"commentId","text","links","linksTruncated","createdAt","score","replyCount","pinned","author"})||!decimal(a["commentId"],true)||!a["score"].isString()||!ok||score!=QString::number(n)||!decimal(a["replyCount"])||!a["pinned"].isBool()||!a["author"].isObject()||!keys(author,{"id","name","avatarUrl","profileUrl","title"})||(!author["id"].isNull()&&!decimal(author["id"],true))||!text(author["name"],1024)||!text(author["title"],1024)||!https(author["avatarUrl"],true)||!https(author["profileUrl"],true))return false;
                for(auto field:{"avatarUrl","profileUrl"})if(!author[field].isNull()){const auto host=QUrl(author[field].toString()).host();if(host!="gamebanana.com"&&!host.endsWith(".gamebanana.com"))return false;}
                id=a["commentId"].toString();
            }else{
                if(!keys(a,{"updateId","name","version","text","links","linksTruncated","createdAt","changes"})||!decimal(a["updateId"],true)||!text(a["name"],1024)||!text(a["version"],256,true)||!a["changes"].isArray()||a["changes"].toArray().size()>128)return false;
                for(const auto &y:a["changes"].toArray()){const auto c=y.toObject();if(!y.isObject()||!keys(c,{"category","text"})||!text(c["category"],128)||!text(c["text"],4096))return false;}id=a["updateId"].toString();
            }
            if(ids.contains(id))return false;ids.insert(id);
        }
        r.pageSize=size;
    }
    r.detail=d.toVariantMap();r.detail.insert("resourceState",r.state);return true;
}
bool domain(const QJsonObject &e,const CatalogRequestIdentity &i,CatalogDecodedResult &r) {
    const auto ident=e["identity"].toObject();
    if(!keys(e,{"identity","catalogRevision","provenance","data"})||!e["identity"].isObject()||!keys(ident,{"kind","args","catalogRevision","preferencesRevision"})||ident["kind"]!=QJsonValue(i.kind)||ident["args"]!=QJsonValue(i.args)||!decimal(e["catalogRevision"])||ident["catalogRevision"]!=e["catalogRevision"]||!provenance(e["provenance"],r.provenance))return false;
    r.revision=e["catalogRevision"].toString();r.preferencesRevision=ident["preferencesRevision"].toString();
    const bool prefs=i.kind=="query"||i.kind=="collection"||i.kind=="favoritesGet"||i.kind=="favoriteSet";
    if(prefs?!decimal(ident["preferencesRevision"]):!ident["preferencesRevision"].isNull())return false;
    const auto d=e["data"].toObject();
    if(i.kind=="facets"){
        if(!e["data"].isArray()||e["data"].toArray().size()>512)return false;
        for(const auto &x:e["data"].toArray()){const auto f=x.toObject();if(!x.isObject()||!(keys(f,{"category","hero"}) || (keys(f,{"category","hero","thumbnail","masked"}) && text(f["thumbnail"],4096) && f["masked"].isBool()))||!text(f["category"],128)||!text(f["hero"],128,true))return false;if(!r.categories.contains(f["category"].toString()))r.categories.append(f["category"].toString());const auto hero=f["hero"].isNull()?QStringLiteral("None"):f["hero"].toString();if(!r.heroes.contains(hero))r.heroes.append(hero);if(f.contains("thumbnail"))r.heroArtwork.insert(hero,QVariantMap{{"thumbnail",f["thumbnail"].toString()},{"masked",f["masked"].toBool()}});}return true;
    }
    if(i.kind=="authors"){
        if(!e["data"].isArray()||e["data"].toArray().size()>5)return false;
        for(const auto &x:e["data"].toArray()){const auto a=x.toObject();if(!x.isObject()||!keys(a,{"authorRemoteId","name","submissionCount","downloadCount"})||!decimal(a["authorRemoteId"],true)||!text(a["name"],1024)||!number(a["submissionCount"],0,37500)||!decimal(a["downloadCount"]))return false;r.authors.append(a.toVariantMap());}return true;
    }
    if(!e["data"].isObject())return false;
    if(i.kind=="query") {
        if(!keys(d,{"items","total","page","pageSize","favorites","preferencesRevision"})||!d["items"].isArray()||!decimal(d["total"])||d["page"]!=i.args["page"]||d["pageSize"]!=i.args["pageSize"]||d["preferencesRevision"]!=ident["preferencesRevision"]||d["items"].toArray().size()>i.args["pageSize"].toInt())return false;
        QSet<QString> seen;
        for(const auto &x:d["items"].toArray()){CatalogSummary s;if(!summary(x,s)||seen.contains(s.key.identity()))return false;seen.insert(s.key.identity());r.rows.append(row(s));}
        if(!favorites(d["favorites"],r.rows)||d["total"].toString().toULongLong()>37500||!pageCount(d["items"].toArray(),d["total"],d["page"].toInt(),d["pageSize"].toInt()))return false;r.page=d["page"].toInt();r.pageSize=d["pageSize"].toInt();r.total=d["total"].toString();return true;
    }
    if(i.kind=="detail"||i.kind=="files"||i.kind=="comments"||i.kind=="changelog")return resource(d,i,r);
    if(i.kind=="favoritesGet"||i.kind=="favoriteSet"){
        const bool set=i.kind=="favoriteSet";
        if(!(set?keys(d,{"preferencesRevision","submission","favorite","changed"}):keys(d,{"preferencesRevision","submission","favorite"}))||d["preferencesRevision"]!=ident["preferencesRevision"]||d["submission"]!=i.args["submission"])return false;
        if(d["submission"].isNull())return !set&&d["favorite"].isNull();
        const auto k=SubmissionKey::decode(d["submission"]);if(!k||!d["favorite"].isBool()||(set&&(!d["changed"].isBool()||d["favorite"]!=i.args["desired"])))return false;
        if(set){const auto old=i.args["expectedRevision"].toString().toULongLong();const bool changed=d["changed"].toBool();if((changed&&old==std::numeric_limits<quint64>::max())||d["preferencesRevision"].toString()!=QString::number(old+quint64(changed)))return false;}
        r.submission=*k;r.favoriteKnown=true;r.favorite=d["favorite"].toBool();r.changed=d["changed"].toBool();return true;
    }
    if(i.kind=="collections"){
        if(!keys(d,{"items","total","page","pageSize","refreshRecommended","refreshState"})||!d["items"].isArray()||!decimal(d["total"])||d["page"]!=i.args["page"]||d["pageSize"]!=i.args["pageSize"]||d["items"].toArray().size()>i.args["pageSize"].toInt()||!refresh(d,r))return false;
        QSet<QString> seen;
        for(const auto &x:d["items"].toArray()) { const auto c=x.toObject();QVariantMap h;
            if(!x.isObject()||!keys(c,{"header","expectedCount","fetchedMembershipCount","lastMemberSyncAt","memberHeaderRevision","membershipStale","membershipIncomplete","availabilityChecked"})||!header(c["header"],h)||!decimal(c["expectedCount"])||c["expectedCount"].toString()!=h.value("itemCount").toString()||!decimal(c["fetchedMembershipCount"])||c["fetchedMembershipCount"].toString().toULongLong()>3750||!timestamp(c["lastMemberSyncAt"])||!timestamp(c["memberHeaderRevision"])||!c["membershipStale"].isBool()||!c["membershipIncomplete"].isBool()||c["availabilityChecked"]!=QJsonValue(false))return false;
            const auto id=h.value("collectionId").toString();if(seen.contains(id))return false;seen.insert(id);
            auto m=c.toVariantMap();m.insert("header",h);r.rows.append({"collection:"+id,h.value("name").toString(),h.value("author").toString(),{},{},h.value("coverUrl").toString(),"ready",{}, {},true,h.value("isNsfw").toBool()||!h.value("contentRatingKnown").toBool(),false,false,m});
        }
        if(d["total"].toString().toULongLong()>12500||!pageCount(d["items"].toArray(),d["total"],d["page"].toInt(),d["pageSize"].toInt()))return false;
        r.page=d["page"].toInt();r.pageSize=d["pageSize"].toInt();r.total=d["total"].toString();return true;
    }
    if(i.kind=="collection"){
        if(!keys(d,{"state","header","members","page","pageSize","expectedCount","fetchedMembershipCount","lastMemberSyncAt","memberHeaderRevision","membershipStale","membershipIncomplete","unavailableOrMissingMemberCount","descriptionFetchedAt","descriptionStale","preferencesRevision","favorites","refreshRecommended","refreshState","headerSource","descriptionSource","membershipSource"})||!one(d["state"],"missing|ready")||d["page"]!=i.args["page"]||d["pageSize"]!=i.args["pageSize"]||!d["members"].isArray()||d["members"].toArray().size()>i.args["pageSize"].toInt()||!decimal(d["fetchedMembershipCount"])||(!d["expectedCount"].isNull()&&!decimal(d["expectedCount"]))||(!d["unavailableOrMissingMemberCount"].isNull()&&!decimal(d["unavailableOrMissingMemberCount"]))||!timestamp(d["lastMemberSyncAt"])||!timestamp(d["memberHeaderRevision"])||!timestamp(d["descriptionFetchedAt"])||!d["membershipStale"].isBool()||!d["membershipIncomplete"].isBool()||!d["descriptionStale"].isBool()||d["preferencesRevision"]!=ident["preferencesRevision"]||!refresh(d,r))return false;
        for(auto source:{"headerSource","descriptionSource","membershipSource"})if(!d[source].isNull()&&!one(d[source],source==QString("headerSource")?"cache":"cache|network"))return false;
        QVariantMap h;
        if(d["state"]==QJsonValue("ready")){if(!header(d["header"],h)||h.value("collectionId").toString()!=i.args["collectionId"].toString()||h.value("itemCount").toString()!=d["expectedCount"].toString()||d["descriptionFetchedAt"].toVariant()!=h.value("descriptionFetchedAt")||d["headerSource"]!=QJsonValue("cache"))return false;}
        else if(!d["header"].isNull()||!d["members"].toArray().isEmpty()||!d["expectedCount"].isNull()||d["fetchedMembershipCount"]!=QJsonValue("0")||d["membershipStale"]!=QJsonValue(true)||d["membershipIncomplete"]!=QJsonValue(true))return false;
        QSet<QString> seen;quint64 previous=0;bool first=true;
        for(const auto &x:d["members"].toArray()){const auto m=x.toObject();if(!x.isObject()||!keys(m,{"position","submission","state","reason","summary"})||!decimal(m["position"])||!one(m["state"],"ready|missing|unavailable|unidentified"))return false;
            const auto position=m["position"].toString().toULongLong();if((!first&&position!=previous+1)||(first&&position!=quint64(i.args["page"].toInt())*quint64(i.args["pageSize"].toInt())))return false;first=false;previous=position;
            const auto state=m["state"].toString();const auto k=SubmissionKey::decode(m["submission"]);CatalogRow cr;cr.identity="member:"+m["position"].toString();cr.state=state;cr.reason=m["reason"].toString();
            if(state=="unidentified"){if(!m["submission"].isNull()||!m["summary"].isNull()||!one(m["reason"],"catalogPrivate|unsupportedOrRedacted"))return false;cr.title=QStringLiteral("Unsupported or redacted member");}
            else {if(!k||seen.contains(k->identity()))return false;seen.insert(k->identity());cr.key=*k;cr.identity=k->identity();
                if(state=="ready"){CatalogSummary s;if(!m["reason"].isNull()||!summary(m["summary"],s)||s.key!=*k)return false;cr=row(s);}
                else {if(!m["summary"].isNull()||(state=="missing"?!m["reason"].isNull():!one(m["reason"],"catalogPrivate|catalogNotFound")))return false;cr.title=k->identity();}
            }
            r.rows.append(cr);
        }
        if(!favorites(d["favorites"],r.rows)||d["fetchedMembershipCount"].toString().toULongLong()>3750||!pageCount(d["members"].toArray(),d["fetchedMembershipCount"],d["page"].toInt(),d["pageSize"].toInt())||(!d["unavailableOrMissingMemberCount"].isNull()&&d["unavailableOrMissingMemberCount"].toString().toULongLong()>d["fetchedMembershipCount"].toString().toULongLong()))return false;r.detail=d.toVariantMap();r.detail.remove("members");r.detail.remove("favorites");r.detail.insert("header",h);r.state=d["state"].toString();r.total=d["fetchedMembershipCount"].toString();r.page=d["page"].toInt();r.pageSize=d["pageSize"].toInt();return true;
    }
    if(i.kind=="sync"){
        const auto c=d["collections"].toObject();
        return keys(d,{"mode","snapshot","complete","startedAt","memberMaxAgeSeconds","collections"})&&one(d["mode"],"full|incremental|throttled")&&LaunchData::hex(d["snapshot"],32)&&d["complete"].isBool()&&timestamp(d["startedAt"])&&d["memberMaxAgeSeconds"]==QJsonValue(604800)&&d["collections"].isObject()&&keys(c,{"state","snapshot","headersComplete","membersComplete","memberMaxAgeSeconds"})&&one(c["state"],"fresh|complete")&&LaunchData::hex(c["snapshot"],32)&&c["headersComplete"].isBool()&&c["membersComplete"]==QJsonValue(false)&&c["memberMaxAgeSeconds"]==QJsonValue(604800);
    }
    return false;
}
}
bool SubmissionKey::valid() const { return QStringList{"mod","sound","wip"}.contains(type)&&decimal(QJsonValue(id),true); }
QString SubmissionKey::source() const { return valid()?QStringLiteral("https://gamebanana.com/")+(type=="mod"?"mods":type=="sound"?"sounds":"wips")+"/"+id:QString(); }
QJsonObject SubmissionKey::json() const { return {{"provider","gamebanana"},{"submissionType",type},{"submissionId",id}}; }
QVariantMap SubmissionKey::presentation() const { return {{"provider","gamebanana"},{"submissionType",type},{"submissionId",id}}; }
std::optional<SubmissionKey> SubmissionKey::decode(const QJsonValue &v) { const auto o=v.toObject();SubmissionKey k{o["submissionType"].toString(),o["submissionId"].toString()};if(!v.isObject()||!keys(o,{"provider","submissionType","submissionId"})||o["provider"]!=QJsonValue("gamebanana")||!k.valid())return {};return k; }
std::optional<SubmissionKey> SubmissionKey::compatibilityReference(const QString &s) {
    SubmissionKey k;
    if(s.startsWith("snd-"))k={"sound",s.mid(4)};else if(s.startsWith("wip-"))k={"wip",s.mid(4)};else k={"mod",s};
    if(!k.valid())return {};return k;
}
QJsonObject CatalogQuery::json() const {
    const auto time=[](const std::optional<qint64> &v){return v?QJsonValue(*v):QJsonValue(QJsonValue::Null);};
    const auto boolean=[](const std::optional<bool> &v){return v?QJsonValue(*v):QJsonValue(QJsonValue::Null);};
    return {{"search",search},{"categories",QJsonArray::fromStringList(categories)},{"heroes",QJsonArray::fromStringList(heroes)},{"authorRemoteId",author.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(author)},{"excludeFilters",exclude},{"isAudio",boolean(audio)},{"isMap",boolean(map)},{"hideNsfw",hideNsfw},{"nsfwOnly",nsfwOnly},{"hideObsolete",hideObsolete},{"updatedAfter",time(updatedAfter)},{"addedAfter",time(addedAfter)},{"addedBefore",time(addedBefore)},{"favoritesOnly",favoritesOnly},{"includeWips",includeWips},{"submissionType",type.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(type)},{"sort",sort},{"page",page},{"pageSize",pageSize}};
}
QJsonObject CatalogQuery::scope() const { const auto q=json();QJsonObject s;for(auto key:{"authorRemoteId","isAudio","isMap","includeWips","submissionType"})s.insert(key,q[key]);return s; }
QVariantMap CatalogSummary::presentation() const { auto p=metadata;p.insert("submission",key.presentation());p.insert("identity",key.identity());p.insert("title",title);p.insert("description",description);p.insert("author",author);p.insert("authorId",authorId);p.insert("category",category);p.insert("hero",hero);p.insert("thumbnail",thumbnail);p.insert("audioUrl",audio);p.insert("profileUrl",source);p.insert("images",images);p.insert("tags",tags);p.insert("masked",nsfw||!visibilityKnown);p.insert("visibilityKnown",visibilityKnown);p.insert("obsolete",obsolete);p.insert("hydrated",hydrated);p.insert("favoriteKnown",favoriteKnown);p.insert("favorite",favorite);return p; }

bool CatalogData::operation(const QString &kind,const QJsonObject &a) {
    const auto scope=[&]{return (a["authorRemoteId"].isNull()||decimal(a["authorRemoteId"],true))&&(a["isAudio"].isNull()||a["isAudio"].isBool())&&(a["isMap"].isNull()||a["isMap"].isBool())&&a["includeWips"].isBool()&&(a["submissionType"].isNull()||one(a["submissionType"],"mod|sound|wip"));};
    if(kind=="query")return keys(a,{"search","categories","heroes","authorRemoteId","excludeFilters","isAudio","isMap","hideNsfw","nsfwOnly","hideObsolete","updatedAfter","addedAfter","addedBefore","favoritesOnly","includeWips","submissionType","sort","page","pageSize"})&&scope()&&text(a["search"],512)&&strings(a["categories"],64,128,false,true)&&strings(a["heroes"],64,128,false,true)&&a["excludeFilters"].isBool()&&a["hideNsfw"].isBool()&&a["nsfwOnly"].isBool()&&!(a["hideNsfw"].toBool()&&a["nsfwOnly"].toBool())&&a["hideObsolete"].isBool()&&a["favoritesOnly"].isBool()&&timestamp(a["updatedAfter"])&&timestamp(a["addedAfter"])&&timestamp(a["addedBefore"])&&(a["addedAfter"].isNull()||a["addedBefore"].isNull()||a["addedAfter"].toInteger()<=a["addedBefore"].toInteger())&&one(a["sort"],"default|lastUpdated|downloadCount|rating|releaseDate")&&number(a["page"],0,1000000)&&number(a["pageSize"],1,100);
    if(kind=="facets")return keys(a,{"authorRemoteId","isAudio","isMap","includeWips","submissionType"})&&scope();
    if(kind=="authors")return keys(a,{"authorRemoteId","isAudio","isMap","includeWips","submissionType","search","hideNsfw"})&&scope()&&text(a["search"],512)&&a["hideNsfw"].isBool();
    if(kind=="sync")return (keys(a,{"forceRefresh","forceReconcile"}) || (keys(a,{"forceRefresh","forceReconcile","clearCache"}) && a["clearCache"]==QJsonValue(true) && a["forceRefresh"]==QJsonValue(true) && a["forceReconcile"]==QJsonValue(true)))&&a["forceRefresh"].isBool()&&a["forceReconcile"].isBool();
    if(kind=="favoritesGet")return keys(a,{"submission"})&&(a["submission"].isNull()||SubmissionKey::decode(a["submission"]).has_value());
    if(kind=="favoriteSet")return keys(a,{"submission","desired","expectedRevision"})&&SubmissionKey::decode(a["submission"]).has_value()&&a["desired"].isBool()&&decimal(a["expectedRevision"]);
    if(kind=="collections")return keys(a,{"search","hideNsfw","page","pageSize"})&&text(a["search"],512)&&a["hideNsfw"].isBool()&&number(a["page"],0,12499)&&number(a["pageSize"],1,50);
    if(kind=="collection")return keys(a,{"collectionId","page","pageSize","refresh"})&&decimal(a["collectionId"],true)&&number(a["page"],0,12499)&&number(a["pageSize"],1,50)&&a["refresh"].isBool();
    if(kind=="detail")return keys(a,{"submission","refresh"})&&SubmissionKey::decode(a["submission"]).has_value()&&a["refresh"].isBool();
    if(kind=="files"||kind=="comments"||kind=="changelog")return keys(a,{"submission","page","refresh"})&&SubmissionKey::decode(a["submission"]).has_value()&&number(a["page"],1,kind=="files"?7:500)&&a["refresh"].isBool();
    return false;
}
CatalogDecodedResult CatalogData::decode(const QJsonObject &o,const CatalogRequestIdentity &i) {
    CatalogDecodedResult r;
    if(o["version"]!=QJsonValue(1)||!LaunchData::root(o["root"])||!text(o["root"],4096)||!LaunchData::hex(o["catalogSession"],32)||(i.rootBound&&o["root"]!=QJsonValue(i.root))||(i.call!=45&&o["catalogSession"]!=QJsonValue(i.session)))return r;
    r.root=o["root"].toString();r.session=o["catalogSession"].toString();r.phase=o["phase"].toString();r.kind=i.kind;r.key=o["requestKey"].toString();
    if(i.call==45||(i.call==47&&i.key.isEmpty())){
        if(!keys(o,{"version","root","catalogSession","phase","error"})||!one(o["phase"],"opening|ready|failed|closing|closed")||(r.phase=="failed"?!error(o["error"]):!o["error"].isNull()))return r;
        if(r.phase=="failed"){r.errorKind=o["error"].toObject()["kind"].toString();r.error=o["error"].toObject()["message"].toString();}r.valid=true;return r;
    }
    if(i.call==50){r.valid=keys(o,{"version","root","catalogSession","phase"})&&one(o["phase"],"closing|closed");return r;}
    if(o["requestKey"]!=QJsonValue(i.key)||!LaunchData::hex(o["requestKey"],32))return r;
    if(i.call==48||i.call==49){r.state=o["state"].toString();r.valid=keys(o,{"version","root","catalogSession","requestKey","state"})&&one(o["state"],i.call==49?"released":"requested|alreadyTerminal|released");return r;}
    if(o["viewGeneration"]!=QJsonValue(i.generation)||!decimal(o["viewGeneration"])||!LaunchData::hex(o["inputDigest"],64)||(!i.digest.isEmpty()&&o["inputDigest"]!=QJsonValue(i.digest)))return r;
    r.generation=i.generation;r.digest=o["inputDigest"].toString();
    if(i.call==46){r.valid=keys(o,{"version","root","catalogSession","requestKey","viewGeneration","inputDigest","phase"})&&one(o["phase"],"queued|running|succeeded|failed|cancelled");return r;}
    if(!keys(o,{"version","root","catalogSession","requestKey","viewGeneration","inputDigest","operation","phase","progress","result","error"})||!o["operation"].isObject()||!one(o["phase"],"queued|running|succeeded|failed|cancelled|released"))return r;
    const auto op=o["operation"].toObject();
    if(!keys(op,{"kind","args"})||op["kind"]!=QJsonValue(i.kind))return r;
    if(r.phase=="released"){r.valid=op["args"].isNull()&&o["progress"].isNull()&&o["result"].isNull()&&o["error"].isNull();return r;}
    if(op["args"]!=QJsonValue(i.args)||!operation(i.kind,i.args))return r;
    if(!o["progress"].isNull()){const auto p=o["progress"].toObject();if(!o["progress"].isObject()||!keys(p,{"stage","completed","total"})||!one(p["stage"],"index-mod|index-sound|index-wip|hydrate-mod|hydrate-sound|hydrate-wip|confirm-mod|confirm-sound|confirm-wip|retire-mod|retire-sound|retire-wip|collections")||!decimal(p["completed"])||(!p["total"].isNull()&&(!decimal(p["total"])||p["completed"].toString().toULongLong()>p["total"].toString().toULongLong()))||QJsonDocument(p).toJson(QJsonDocument::Compact).size()>256)return r;r.progress=p.toVariantMap();}
    if(r.phase=="queued"||r.phase=="running"){r.valid=o["result"].isNull()&&o["error"].isNull();return r;}
    if(r.phase=="succeeded"){r.valid=o["result"].isObject()&&o["error"].isNull()&&domain(o["result"].toObject(),i,r);return r;}
    if(!o["result"].isNull()||!error(o["error"])||(r.phase=="cancelled"&&o["error"].toObject()["kind"]!=QJsonValue("catalogCancelled")))return r;
    r.errorKind=o["error"].toObject()["kind"].toString();r.error=o["error"].toObject()["message"].toString();r.valid=true;return r;
}

bool CatalogData::richDescriptionValid(const QJsonValue &v) { return richDescription(v); }
