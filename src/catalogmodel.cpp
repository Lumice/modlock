#include "catalogmodel.h"
QVariant CatalogModel::data(const QModelIndex &i,int role) const {
    if(!i.isValid()||i.row()<0||i.row()>=m_rows.size())return {};
    const auto &r=m_rows[i.row()];
    switch(role){case Identity:return r.identity;case Submission:return r.key.valid()?r.key.presentation():QVariantMap{};case Title:return r.title;case Author:return r.author;case Category:return r.category;case Hero:return r.hero;case Thumbnail:return r.thumbnail;case ContentMasked:return r.masked;case FavoriteKnown:return r.favoriteKnown;case Favorite:return r.favorite;case Provenance:return m_provenance;case Collection:return r.collection;case State:return r.state;case Reason:return r.reason;case Metadata:return r.metadata;default:return {};}
}
QHash<int,QByteArray> CatalogModel::roleNames() const {return {{Identity,"identity"},{Submission,"submission"},{Title,"title"},{Author,"author"},{Category,"category"},{Hero,"hero"},{Thumbnail,"thumbnail"},{ContentMasked,"contentMasked"},{FavoriteKnown,"favoriteKnown"},{Favorite,"favorite"},{Provenance,"provenance"},{Collection,"collection"},{State,"state"},{Reason,"reason"},{Metadata,"metadata"}};}
void CatalogModel::replace(QList<CatalogRow> rows,const QVariantMap &p,const QString &preferencesRevision){beginResetModel();for(auto &r:rows)if(r.favoriteKnown)r.metadata.insert("favoriteRevision",preferencesRevision);m_rows=std::move(rows);m_provenance=p;endResetModel();emit countChanged();}
void CatalogModel::favorite(const SubmissionKey &key,bool value,const QString &preferencesRevision){for(int n=0;n<m_rows.size();++n)if(m_rows[n].key==key){auto &r=m_rows[n];r.favoriteKnown=true;r.favorite=value;r.metadata.insert("favoriteKnown",true);r.metadata.insert("favorite",value);r.metadata.insert("favoriteRevision",preferencesRevision);emit dataChanged(index(n),index(n),{FavoriteKnown,Favorite,Metadata});}}
void CatalogModel::clearFavorites(){for(auto &r:m_rows){r.favoriteKnown=false;r.metadata.insert("favoriteKnown",false);r.metadata.remove("favoriteRevision");}if(!m_rows.isEmpty())emit dataChanged(index(0),index(m_rows.size()-1),{FavoriteKnown,Metadata});}
int CatalogModel::indexOf(const QString &identity) const {for(int n=0;n<m_rows.size();++n)if(m_rows[n].identity==identity)return n;return -1;}
QVariantMap CatalogModel::item(const QString &identity) const {const auto n=indexOf(identity);if(n<0)return {};const auto &r=m_rows[n];return {{"identity",r.identity},{"submission",r.key.valid()?r.key.presentation():QVariantMap{}},{"collection",r.collection},{"state",r.state},{"metadata",r.metadata}};}

QString CatalogModel::favoriteRevision(const SubmissionKey &key) const {for(const auto &r:m_rows)if(r.key==key&&r.favoriteKnown)return r.metadata.value("favoriteRevision").toString();return {};}
