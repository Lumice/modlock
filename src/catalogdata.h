#pragma once
#include <QJsonObject>
#include <QVariantMap>
#include <QMetaType>
#include <QStringList>
#include <optional>

struct SubmissionKey {
    QString type, id;
    bool valid() const;
    QString identity() const { return valid() ? type + ":" + id : QString(); }
    QString source() const;
    QJsonObject json() const;
    QVariantMap presentation() const;
    static std::optional<SubmissionKey> decode(const QJsonValue &);
    static std::optional<SubmissionKey> compatibilityReference(const QString &);
    bool operator==(const SubmissionKey &) const = default;
};
struct CatalogQuery {
    QString search, author, type = "mod", sort = "default";
    QStringList categories, heroes;
    bool exclude = false, hideNsfw = true, nsfwOnly = false, hideObsolete = false, favoritesOnly = false, includeWips = false;
    std::optional<bool> audio, map;
    std::optional<qint64> updatedAfter, addedAfter, addedBefore;
    int page = 0, pageSize = 50;
    QJsonObject json() const;
    QJsonObject scope() const;
};
struct CatalogSummary {
    SubmissionKey key;
    QString title, description, author, authorId, category, hero, thumbnail, audio, source;
    QStringList images, tags;
    bool nsfw = true, visibilityKnown = false, obsolete = false, hydrated = false, favoriteKnown = false, favorite = false;
    QVariantMap metadata;
    QVariantMap presentation() const;
};
struct CatalogRow {
    QString identity, title, author, category, hero, thumbnail, state, reason;
    SubmissionKey key;
    bool collection = false, masked = true, favoriteKnown = false, favorite = false;
    QVariantMap metadata;
};
// Immutable request snapshot attached to the physical JSON-RPC request. It is
// independent of the library's profile/snapshot generation.
struct CatalogRequestIdentity {
    int call = 45;
    QString root, session, key, generation, digest, kind;
    QJsonObject args;
    bool rootBound = false;
};
struct CatalogDecodedResult {
    bool valid = false;
    QString root, session, key, generation, digest, phase, state, kind, errorKind, error;
    QVariantMap progress, provenance, detail, heroArtwork;
    QList<CatalogRow> rows;
    QStringList categories, heroes;
    QVariantList authors;
    QString revision, preferencesRevision, total;
    int page = 0, pageSize = 50;
    bool refreshRecommended = false, favoriteKnown = false, favorite = false, changed = false;
    SubmissionKey submission;
};
Q_DECLARE_METATYPE(CatalogDecodedResult)
namespace CatalogData {
bool richDescriptionValid(const QJsonValue &);
CatalogDecodedResult decode(const QJsonObject &, const CatalogRequestIdentity &);
bool operation(const QString &, const QJsonObject &);
}
