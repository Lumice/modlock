#pragma once
#include "catalogdata.h"
#include <QAbstractListModel>
#include <QtQml/qqmlregistration.h>

class CatalogModel final : public QAbstractListModel {
    Q_OBJECT
    QML_ELEMENT
    QML_UNCREATABLE("Owned by CatalogController")
    Q_PROPERTY(int count READ count NOTIFY countChanged)
public:
    enum Role { Identity=Qt::UserRole+1, Submission, Title, Author, Category, Hero, Thumbnail, ContentMasked, FavoriteKnown, Favorite, Provenance, Collection, State, Reason, Metadata };
    explicit CatalogModel(QObject *parent=nullptr):QAbstractListModel(parent){}
    int rowCount(const QModelIndex &parent={}) const override {return parent.isValid()?0:m_rows.size();}
    int count() const {return m_rows.size();}
    QVariant data(const QModelIndex &,int) const override;
    QHash<int,QByteArray> roleNames() const override;
    void replace(QList<CatalogRow>,const QVariantMap &,const QString &preferencesRevision={});
    void favorite(const SubmissionKey &,bool,const QString &preferencesRevision);
    QString favoriteRevision(const SubmissionKey &) const;
    void clearFavorites();
    Q_INVOKABLE int indexOf(const QString &) const;
    Q_INVOKABLE QVariantMap item(const QString &) const;
    Q_INVOKABLE QString identityAt(int row) const { return row >= 0 && row < m_rows.size() ? m_rows[row].identity : QString(); }
signals:
    void countChanged();
private:
    QList<CatalogRow> m_rows;
    QVariantMap m_provenance;
};
