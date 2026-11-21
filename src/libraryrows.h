#pragma once

#include <QAbstractListModel>
#include <QJsonObject>
#include <QVariantMap>
#include <QSet>
#include "catalogdata.h"

class LibraryMediaController;

struct LibraryMod {
    QString id, title, author, hero, kind, status;
    QString addedAt, updatedAt, authorId, variantName, fileName;
    QVariant sizeBytes;
    bool heroDetectionEligible = false;
    bool fixEnabled = true;
    bool enabled = false, appliedEnabled = false, available = true, pendingApply = false, selectionSaved = false, selectionReady = true;
    QString pendingStatus, pendingReason;
    bool linkEligible = false;
    QString linkReason;
    QVariant optionCount;
    QStringList selectedOptionIds;
    QString unavailableReason;
    QVariant installOrder;
    QVariantMap localArtwork, remoteSubmission;
    QVariantList fonts, fontDiagnostics, fontStatus, fontInstall, fontOwned;
    QString localPreview;
    QStringList installedVpks, missingVpks, images;
    bool previewHidden = false;
    quint64 mediaRevision = 0;
    QByteArray mediaFingerprint;
    bool operator==(const LibraryMod &) const = default;
    QVariantMap details() const;
};

struct LibrarySnapshot {
    QString sourcePath, activeProfileId, selectedProfileId;
    QString sourceKind = "upstream", revision, gamePath, storageRoot, appliedProfileId;
    bool pendingChanges = false, canRestore = false, recoveryRequired = false;
    QString pendingStatus,pendingReason;
    QJsonObject launchBarrier;
    QVariantMap bisect, launchFingerprint;
    bool fixAutomatically = true;
    bool canUndoFonts = false;
    QHash<QString, QVariantList> fontOwners;
    QHash<QString, QSet<QString>> fontOwnerIds;
    QVariantList fontOwnerRows;
    QVariantMap fontOfferMods;
    QVariantList profiles;
    QStringList warnings, warningCodes;
    QList<LibraryMod> mods;
    static bool decode(const QJsonObject &object, LibrarySnapshot &snapshot);
};

Q_DECLARE_METATYPE(LibrarySnapshot)

class LibraryRows final : public QAbstractListModel {
public:
    enum Role { ModId = Qt::UserRole + 1, Title, Hero, Kind, ModEnabled, Author, Status, ModPreview, ModPreviewHidden, ModAppliedEnabled, ModAvailable, ModUnavailableReason, ModSelectionReady, ModPendingStatus, ModPendingApply, ModSelectionSaved, MediaRevision, ModHealthStatus, ModHealthSymptoms, ModUpdateStatus, AddedAt, UpdatedAt, SizeBytes, VariantName, FileName, AuthorId };
    explicit LibraryRows(QObject *parent = nullptr) : QAbstractListModel(parent) {}
    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    void replace(QList<LibraryMod> mods);
    bool setDetectedHero(const QString &id, const QString &hero);
    bool setUpdates(const QVariantList &rows);
    void setHealth(const QVariantMap &report);
    bool setEnabled(const QString &id, bool enabled);
    QVariantMap details(const QString &id) const;
    QVariantList homeRecent() const;
    QVariantList homeEnabled() const;
    int rowForId(const QString &id) const { return m_byId.value(id, -1); }
    std::optional<SubmissionKey> mediaSubmission(const QString &id) const;
    void setMediaSource(LibraryMediaController *source) { m_media = source; }
    bool notifyMediaChanged(const QString &id);
    int enabledCount() const { return m_enabledCount; }
    QStringList categories() const { return m_categories; }
    QStringList heroes() const { return m_heroes; }
    QStringList types() const { return m_types; }
private:
    QList<LibraryMod> m_mods;
    QHash<QString,QVariantMap> m_health, m_updates;
    QHash<QString, int> m_byId;
    int m_enabledCount = 0;
    QStringList m_categories;
    QStringList m_heroes, m_types;
    LibraryMediaController *m_media = nullptr;
    QVariantMap effectiveDetails(const LibraryMod &mod) const;
    QVariantMap homeRecord(int row) const;
    QByteArray mediaFingerprint(const LibraryMod &mod) const;
};
