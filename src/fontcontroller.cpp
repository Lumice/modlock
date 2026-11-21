#include "fontcontroller.h"
#include "fontdata.h"
#include "librarymodel.h"
#include "libraryrows.h"
#include "backendclient.h"
#include <QJsonArray>
#include <utility>

FontController::FontController(LibraryModel *library) : QObject(library), m_library(library) {
    connect(library,&LibraryModel::nativeChanged,this,&FontController::syncContext);
    connect(library,&LibraryModel::loadingChanged,this,&FontController::changed);
}
bool FontController::busy() const { return m_waiting || m_library->busy() || m_library->loading(); }
bool FontController::available() const { return m_library->fontsAvailable(); }
bool FontController::canUndo() const { return available() && m_library->m_canUndoFonts; }
QVariantList FontController::installedOwners() const { return m_library->m_fontOwnerRows; }
void FontController::showInstalled() {
    if (!available() || busy()) return;
    m_root=m_library->dataDir(); m_game=m_library->gamePath(); m_revision=m_library->revision();
    m_review.clear(); m_visible=false; m_managerVisible=true; emit changed();
}
void FontController::closeInstalled() { m_managerVisible=false; emit changed(); }
bool FontController::canCommit() const {
    return available() && !m_waiting && m_review.value("exactPatterns").toBool() && m_review.value("exactOwners").toBool() && m_review.value("summary").toMap().value("canCommit").toBool()
        && m_root==m_library->dataDir() && m_game==m_library->gamePath() && m_revision==m_library->revision();
}
void FontController::syncContext() {
    if (m_managerVisible && (m_root!=m_library->dataDir() || m_game!=m_library->gamePath())) m_managerVisible=false;
    if (m_root!=m_library->dataDir() || m_game!=m_library->gamePath() || m_revision!=m_library->revision()) {
        m_review.clear();
        if (!m_waiting && m_visible) m_error=tr("The library or game changed. Review the selected fonts again.");
    }
    if (!m_offer.isEmpty() && (m_offer.value("root")!=m_library->dataDir() || m_offer.value("gamePath")!=m_library->gamePath())) m_offer.clear();
    emit changed();
}
bool FontController::pin(const QString &id,const QString &kind,const QVariantMap &details) {
    if (!available() || (kind!="install" && kind!="remove") || !FontData::safeId(id) || details.isEmpty()) return false;
    m_root=m_library->dataDir(); m_game=m_library->gamePath(); m_revision=m_library->revision(); m_mod=id; m_kind=kind;
    m_review.clear(); m_error.clear();
    m_assets=details.value(kind=="install" ? "fontInstall" : "fontOwned").toList();
    m_visible=true; emit changed(); emit selectionReset(); return true;
}
bool FontController::open(const QString &id,const QString &kind) {
    if (busy()) return false;
    const auto details=kind=="remove"
        ? QVariantMap{{"fontOwned",m_library->m_fontOwners.value(id)}} : m_library->modDetails(id);
    if (kind=="remove" && details.value("fontOwned").toList().isEmpty()) return false;
    if (!pin(id,kind,details)) return false;
    m_managerVisible=false; emit changed(); return true;
}
bool FontController::openOffer() {
    if (m_offer.isEmpty() || m_offer.value("root")!=m_library->dataDir() || m_offer.value("gamePath")!=m_library->gamePath()) return false;
    const auto offer=m_offer;
    if (!pin(offer.value("modId").toString(),"install",offer)) return false;
    dismissOffer(); return true;
}
void FontController::dismissOffer() { m_offer.clear(); emit changed(); }
bool FontController::dispatch(const QJsonObject &change) {
    m_review.clear(); m_error.clear();
    if (!available()) { emit changed(); return false; }
    m_root=m_library->dataDir(); m_game=m_library->gamePath(); m_revision=m_library->revision();
    m_waiting=true; m_visible=true; emit changed();
    if (!m_library->dispatchFontsPreview(m_root,m_game,m_revision,change)) { m_waiting=false; emit changed(); return false; }
    return true;
}
bool FontController::preview(const QString &id,const QStringList &ids,const QString &kind) {
    // Malformed replacements cannot leave an actionable old token in the UI.
    m_review.clear(); emit changed();
    if (!available() || (kind!="install" && kind!="remove") || !FontData::safeId(id) || ids.isEmpty() || ids.size()>256) return false;
    if (m_mod!=id || m_kind!=kind || m_root!=m_library->dataDir() || m_game!=m_library->gamePath()) {
        if (!open(id,kind)) return false;
    }
    QSet<QString> allowed,seen;
    if(kind=="remove") allowed=m_library->m_fontOwnerIds.value(id);
    else for (const auto &f:m_assets) allowed.insert(f.toMap().value("fontId").toString());
    for (const auto &f:ids) { if(!FontData::hex(f,32) || !allowed.contains(f) || seen.contains(f)) return false; seen.insert(f); }
    m_mod=id; m_kind=kind;
    m_requestedIds=seen;
    return dispatch({{"kind",kind},{"modId",id},{"fontIds",QJsonArray::fromStringList(ids)}});
}
bool FontController::previewUndo() {
    if (!canUndo()) return false;
    m_kind="undo"; m_mod.clear(); m_assets.clear(); m_requestedIds.clear(); return dispatch({{"kind","undo"}});
}
void FontController::acceptReview(const QVariantMap &review) {
    m_waiting=false;
    const auto summary=review.value("summary").toMap();
    QSet<QString> ids;
    for(const auto &id:summary.value("fontIds").toList()) ids.insert(id.toString());
    if (m_root!=m_library->dataDir() || m_game!=m_library->gamePath() || m_revision!=m_library->revision()
        || review.value("expectedRevision")!=m_revision || review.value("gamePath")!=m_game
        || summary.value("action")!=m_kind || summary.value("modId").toString()!=m_mod || ids!=m_requestedIds) {
        rejected(tr("The returned font review does not match the pinned library, game and mod.")); return;
    }
    m_review=review; emit changed();
}
void FontController::rejected(const QString &message) { m_waiting=false; m_review.clear(); m_error=message; emit changed(); }
bool FontController::commit() {
    if (!canCommit()) return false;
    const auto token=m_review.value("token").toString();
    if (!m_library->dispatchFontsCommit(m_root,m_game,m_revision,m_mod,token)) return false;
    m_review.clear(); m_waiting=true; emit changed(); return true;
}
void FontController::finished(bool success,const QString &message) {
    m_waiting=false; m_review.clear(); m_error=message; if(success) m_visible=false; emit changed();
}
void FontController::close() { if(m_waiting) return; m_visible=false; m_review.clear(); emit changed(); }
void FontController::discardReview() { if(m_waiting) return; m_review.clear(); emit changed(); }
void FontController::seedOffer(const LibrarySnapshot &snapshot,const QString &id) {
    seedOfferData(snapshot.gamePath,id,snapshot.fontOfferMods.value(id).toMap());
}
void FontController::seedOfferData(const QString &game,const QString &id,const QVariantMap &details) {
    m_pendingOffer.clear();
    if(!details.value("fontInstall").toList().isEmpty()) m_pendingOffer={{"root",m_library->dataDir()},{"gamePath",game},{"modId",id},{"title",details.value("title")},{"fontInstall",details.value("fontInstall")}};
}
void FontController::publishOffer() { m_offer=std::exchange(m_pendingOffer,{}); syncContext(); }
