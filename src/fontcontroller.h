#pragma once
#include <QObject>
#include <QVariantMap>
#include <QSet>
#include <QJsonObject>
#include <QtQml/qqmlregistration.h>
class LibraryModel;
struct LibrarySnapshot;

class FontController final : public QObject {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by LibraryModel")
    Q_PROPERTY(QVariantMap review READ review NOTIFY changed)
    Q_PROPERTY(QVariantMap offer READ offer NOTIFY changed)
    Q_PROPERTY(QVariantList installedOwners READ installedOwners NOTIFY changed)
    Q_PROPERTY(bool managerVisible READ managerVisible NOTIFY changed)
    Q_PROPERTY(QVariantList assets READ assets NOTIFY changed)
    Q_PROPERTY(QString modId READ modId NOTIFY changed)
    Q_PROPERTY(QString gamePath READ gamePath NOTIFY changed)
    Q_PROPERTY(QString kind READ kind NOTIFY changed)
    Q_PROPERTY(QString error READ error NOTIFY changed)
    Q_PROPERTY(bool busy READ busy NOTIFY changed)
    Q_PROPERTY(bool available READ available NOTIFY changed)
    Q_PROPERTY(bool canUndo READ canUndo NOTIFY changed)
    Q_PROPERTY(bool canCommit READ canCommit NOTIFY changed)
    Q_PROPERTY(bool visible READ visible NOTIFY changed)
public:
    explicit FontController(LibraryModel *library);
    QVariantMap review() const { return m_review; }
    QVariantMap offer() const { return m_offer; }
    QVariantList assets() const { return m_assets; }
    QString modId() const { return m_mod; }
    QString gamePath() const { return m_game; }
    QString kind() const { return m_kind; }
    QString error() const { return m_error; }
    bool busy() const;
    bool available() const;
    bool canUndo() const;
    bool canCommit() const;
    bool visible() const { return m_visible; }
    QVariantList installedOwners() const;
    bool managerVisible() const { return m_managerVisible; }
    Q_INVOKABLE void showInstalled();
    Q_INVOKABLE void closeInstalled();
    Q_INVOKABLE bool open(const QString &modId, const QString &kind);
    Q_INVOKABLE bool openOffer();
    Q_INVOKABLE void dismissOffer();
    Q_INVOKABLE bool preview(const QString &modId, const QStringList &fontIds, const QString &kind);
    Q_INVOKABLE bool previewUndo();
    Q_INVOKABLE bool commit();
    Q_INVOKABLE void close();
    Q_INVOKABLE void discardReview();
    void syncContext();
    void acceptReview(const QVariantMap &review);
    void rejected(const QString &message);
    void finished(bool success, const QString &message = {});
    void seedOffer(const LibrarySnapshot &snapshot, const QString &modId);
    void seedOfferData(const QString &game, const QString &modId, const QVariantMap &details);
    void publishOffer();
signals:
    void changed();
    void selectionReset();
private:
    bool dispatch(const QJsonObject &change);
    bool pin(const QString &modId, const QString &kind, const QVariantMap &details);
    LibraryModel *m_library;
    QVariantMap m_review, m_offer, m_pendingOffer;
    QVariantList m_assets;
    QString m_root, m_game, m_revision, m_mod, m_kind, m_error;
    QSet<QString> m_requestedIds;
    bool m_visible = false, m_waiting = false, m_managerVisible = false;
};
