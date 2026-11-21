#include "remotecontroller.h"
#include <QDateTime>
#include "libraryrows.h"
#include "librarymodel.h"
#include "launchdata.h"
#include "remotedata.h"
#include <QCryptographicHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QUuid>
#include <algorithm>
#include <limits>
#include <utility>

namespace {
using C = BackendClient::Call;
QString freshId() { return QUuid::createUuid().toString(QUuid::Id128); }
bool sameCapturedRequest(QJsonObject request,const QJsonObject &captured) {
    request.remove("version");request.remove("dataDir");request.remove("sessionId");
    return !captured.isEmpty() && request==captured;
}
bool definiteStartRejection(const QString &kind,bool replacement) {
    // These errors are returned before the job reservation can succeed. The
    // owner returns them on an exact Start replay only after matching its input.
    return (replacement && (kind=="eligibilityUnknown" || kind=="staleReview"))
        || QStringList{"needsSelection","staleRevision","invalidSelection",
        "providerDenied","providerNotFound","providerMalformed","providerUnavailable","hostNotAllowed",
        "transportTls","transportNetwork","transportTimeout","transportFraming","transportChecksum",
        "transportStorage","storageFull"}.contains(kind);
}
bool metadata(const QJsonObject &o) {
    if(o.size()!=4 || !o.contains("name") || !o.contains("author") || !o.contains("category") || !o.contains("description"))return false;
    for(const auto *key:{"name","author","category","description"}) {
        const auto v=o[key];if(!v.isString())return false;
        const auto s=v.toString();const int max=QString::fromLatin1(key)=="description"?16384:QString::fromLatin1(key)=="category"?128:256;
        if(s.toUtf8().size()>max || (QString::fromLatin1(key)=="name" && s.isEmpty()))return false;
        for(qsizetype i=0;i<s.size();++i) {
            const auto c=s[i];
            if(c.isHighSurrogate()) {if(++i>=s.size() || !s[i].isLowSurrogate())return false;}
            else if(c.isLowSurrogate() || (c.category()==QChar::Other_Control && !(QString::fromLatin1(key)=="description" && (c=='\n'||c=='\t'))))return false;
        }
    }
    return true;
}
bool knownError(const QString &kind) {
    return QStringList{"eligibilityUnknown","providerDenied","providerNotFound","providerMalformed","providerUnavailable","needsSelection",
        "hostNotAllowed","transportTls","transportNetwork","transportTimeout","transportFraming","transportChecksum","transportStorage",
        "claimAmbiguous","storageFull","layoutUnsupported","archiveUnsupported","archiveRejected","archiveLimit","invalidSelection",
        "staleRevision","staleReview","duplicateSubmission","launchBusy","recoveryRequired","outcomeUnknown","remoteBusy",
        "controlReceiptExpired","jobIdConflict","cancelTooLate","jobAbsent","operationIdConflict","invalidParams","staleObservation"}.contains(kind);
}
QString controlSequence(const QVariantMap &job) {
    const auto last=job.value("lastControl").toMap().value("controlSeq",QStringLiteral("0")).toString();bool ok=false;const auto n=last.toULongLong(&ok);
    return ok && QString::number(n)==last && n<std::numeric_limits<quint64>::max()?QString::number(n+1):QString();
}
QString replacementMessage(const QString &kind,const QString &message) {
    if(kind=="eligibilityUnknown")return message.contains(QStringLiteral("skip"),Qt::CaseInsensitive)
        ? QObject::tr("Skipped-file choices could not be checked. You can use Force update or Reinstall.")
        : QObject::tr("This mod cannot be updated or reinstalled from its saved download.");
    if(kind=="staleObservation")return QObject::tr("Download details expired. Check them again.");
    if(kind=="staleRevision")return QObject::tr("The profile changed. Check the mod's downloads again.");
    if(kind=="staleReview")return QObject::tr("This review is no longer current. Review the choices again.");
    if(kind=="invalidSelection")return QObject::tr("That file can no longer be used for this action.");
    if(kind=="providerNotFound")return QObject::tr("The provider did not list this download.");
    if(kind.startsWith(QStringLiteral("provider")) || kind=="eligibilityUnknown")return QObject::tr("The provider check did not complete. Try again later.");
    if(kind=="storageFull")return QObject::tr("There is not enough room to continue this download.");
    return QObject::tr("The download action could not be completed. Check the mod and try again.");
}
}

RemoteController::RemoteController(LibraryModel *model, BackendClient *client)
    : QObject(model), m_model(model), m_client(client) {
    connect(this, &RemoteController::changed, this, &RemoteController::advanceInstall, Qt::QueuedConnection);
    m_poll.setInterval(1500);
    connect(&m_poll,&QTimer::timeout,this,&RemoteController::pulse);
    connect(client,&BackendClient::remoteReady,this,&RemoteController::received);
    connect(client,&BackendClient::remoteRejected,this,&RemoteController::rejected);
    connect(client,&BackendClient::remoteSnapshot,this,[this](C call,const QJsonObject &request,const LibrarySnapshot &){
        if(!current(request))return;m_inFlight.remove(requestIdentity(call,request));if(request["operationId"]==m_replayParams["operationId"])m_replayParams={};
    });
    connect(client,&BackendClient::capabilitiesReady,this,[this]{if(m_visible || (m_shutdown && m_client->remoteOwnerOwned()) || m_model->remoteMarkerPending())pulse();emit changed();});
    connect(client,&BackendClient::retired,this,[this]{
        m_inFlight.clear();m_openReady=false;m_session.clear();m_addListCurrent=m_replaceListCurrent=false;m_resolveAdmitted=false;m_prepareAdmitted=false;
        m_selectionToken.clear();m_files.clear();m_fileEvidence.clear();m_filePageEvidence.clear();resetReview();m_prepareKey.clear();m_prepareParams={};m_selectable.clear();m_seenCandidates.clear();m_candidateEvidence.clear();m_candidateNames.clear();m_selectedIds.clear();
        for(auto it=m_installStages.begin();it!=m_installStages.end();++it) {
            if(it.value()=="candidates" || it.value()=="review" || it.value()=="choice") {
                it.value()="stopped";++m_failureSequences[it.key()];m_installErrors[it.key()]=tr("Connection interrupted. Try again.");
            }
        }
        if(!m_replayParams.isEmpty())m_replayParams["reviewToken"]=QJsonValue(QJsonValue::Null);
        m_replaceTarget.clear();m_replaceSkip.clear();
        if(!m_skipParams.isEmpty()){m_skipParams={};m_error=tr("The skipped-file choice could not be confirmed. Check it again.");}m_skipHeldResolve.clear();
        if(!m_visible && !m_replaceAction.isEmpty()){m_replaceAction.clear();m_replaceParams={};m_resolveParams={};m_resolveKey.clear();}
        else if(!m_replaceAction.isEmpty() && !m_shutdown)requestReplacementObservation();
        if(!m_shutdown && (m_visible || m_model->remoteMarkerPending()))m_phase=tr("Reconnect to check retained downloads.");
        emit changed();
    });
    connect(client,&BackendClient::lifecycleChanged,this,[this]{emit changed();});
    connect(model,&LibraryModel::nativeChanged,this,[this]{emit changed();});
}
bool RemoteController::available() const {
    return !m_shutdown && m_model->nativeMode() && !m_client->localSession() && !m_client->retiring()
        && m_client->isRunning() && m_client->nativeRoot()==m_root && m_client->readyForRequests();
}
bool RemoteController::replacementAvailable() const {return available();}
bool RemoteController::canResolve() const {return available() && m_model->remoteAdmissionAvailable() && m_resolveKey.isEmpty() && m_prepareKey.isEmpty() && !m_committing && m_downloadQueue.isEmpty() && m_resolveReleases.size()+int(!m_skipHeldResolve.isEmpty())<8;}
bool RemoteController::canPrepare() const {return available() && m_model->remoteAdmissionAvailable() && m_job.value("state")=="downloaded"
    && !m_committing && m_prepareKey.isEmpty() && m_releaseKey.isEmpty() && m_job.value("profileId").toString()==m_model->viewedProfileId() && hasConsent()
    && (!replacementJob() || (installTargetCurrent()));}
bool RemoteController::canReleasePreparation() const {return available() && !m_committing && !m_prepareKey.isEmpty() && m_releaseKey.isEmpty();}
void RemoteController::loadRoot(const QString &root) {
    m_installStages.clear();m_installErrors.clear();m_failureSequences.clear();m_installMetadata.clear();m_installCandidates.clear();
    m_pickedFiles.clear();m_downloadQueue.clear();m_appendParams={};m_appendTarget.clear();m_queueProfile.clear();m_queueMod.clear();m_nextReplacementPurpose.clear();m_queueJob.clear();m_collectFiles=false;m_choosingFiles=false;m_installResultMods.clear();m_installCompleted.clear();m_installTargets.clear();
    m_poll.stop();m_root=root;m_session.clear();m_phase="idle";m_error.clear();m_noticeError.clear();m_submission={};m_resolveParams={};m_prepareParams={};m_reviewParams={};m_replayParams={};
    m_pendingStarts.clear();m_pendingControls.clear();m_startPins.clear();m_consents.clear();m_statuses.clear();m_inFlight.clear();m_selectable.clear();m_seenCandidates.clear();m_resolveReleases.clear();m_fileEvidence.clear();m_filePageEvidence.clear();m_candidateEvidence.clear();m_candidateNames.clear();
    m_downloadsRequested=false;m_continuingJob.clear();m_deferredActions.clear();m_jobs.clear();m_addJobs.clear();m_replaceJobs.clear();m_files.clear();m_candidates.clear();m_members.clear();m_diagnostics.clear();m_job.clear();m_review.clear();m_selectedIds.clear();
    m_replaceParams={};m_skipParams={};m_confirmationParams={};m_replaceTarget.clear();m_replaceSkip.clear();m_replaceAction.clear();m_replaceProfile.clear();m_replaceMod.clear();m_replaceRevision.clear();m_skipHeldResolve.clear();m_replayCall=C::RemoteCommit;m_seenFilePages.clear();m_lastFilePage=-1;m_savedFileObservation.clear();m_replaceObservationAge.invalidate();m_replaceExpiryNotified=false;
    m_resolveKey.clear();m_selectionToken.clear();m_jobId.clear();m_commitJobId.clear();m_prepareKey.clear();m_reviewKey.clear();m_memberCandidate.clear();m_releaseJob.clear();m_releaseKey.clear();m_reconcileJob.clear();
    m_filePage=m_candidatePage=m_memberPage=m_diagnosticPage=0;m_fileHasMore=m_candidateHasMore=m_memberHasMore=m_diagnosticHasMore=false;
    m_visible=m_shutdown=m_retiring=m_openReady=m_resolveAdmitted=m_prepareAdmitted=m_reviewAdmitted=m_committing=m_candidatesLoaded=m_addListCurrent=m_replaceListCurrent=false;emit changed();
}
void RemoteController::refreshDownloads() {
    if(!available())return;m_error.clear();m_poll.start();pulse();emit changed();
}
void RemoteController::showDownloads() {
    if(!available())return;m_downloadsRequested=true;m_visible=true;m_error.clear();m_poll.start();pulse();emit changed();
}
QString RemoteController::requestIdentity(C call,QJsonObject params) const {
    params.remove("version");params.remove("dataDir");params.remove("sessionId");
    return QString::number(int(call))+QString::fromUtf8(QJsonDocument(params).toJson(QJsonDocument::Compact));
}
void RemoteController::send(C call,QJsonObject params) {
    if(m_session.isEmpty() || !m_client->isRunning() || m_client->retiring())return;
    params["sessionId"]=m_session;
    const auto key=requestIdentity(call,params);if(m_inFlight.contains(key))return;
    if(call==C::RemoteReplaceTarget && !m_replaceParams.isEmpty() && params["requestKey"]==m_replaceParams["requestKey"] && !m_replaceObservationAge.isValid())m_replaceObservationAge.start();
    m_inFlight.insert(key);m_client->nativeRequest(call,params);
}
bool RemoteController::current(const QJsonObject &request) const {
    return !m_session.isEmpty() && request["sessionId"]==QJsonValue(m_session) && request["dataDir"]==QJsonValue(m_root)
        && m_client->nativeRoot()==m_root;
}
bool RemoteController::sameJob(const QJsonObject &request) const { return request["jobId"]==QJsonValue(m_jobId); }
bool RemoteController::currentPreparation(const QString &job,const QString &key) const {
    return !m_shutdown && !m_committing && m_releaseKey.isEmpty() && job==m_jobId && !key.isEmpty() && key==m_prepareKey && m_job.value("prepareKey").toString()==key;
}
void RemoteController::pulse() {
    if(!m_replaceTarget.isEmpty() && m_replaceObservationAge.isValid() && m_replaceObservationAge.hasExpired(300000) && !m_replaceExpiryNotified) {
        m_replaceExpiryNotified=true;m_error=m_prepareKey.isEmpty()?tr("Download details expired. Check them again.")
            :tr("Download details expired. Close this review and check the download again.");emit changed();
    }
    if(m_shutdown && !m_client->remoteOwnerOwned() && !m_model->remoteMarkerPending()) {m_retiring=false;m_poll.stop();emit changed();return;}
    if(m_root.isEmpty() || m_client->retiring() || !m_client->isRunning() || m_client->nativeRoot()!=m_root || !m_client->readyForRequests())return;
    if(m_shutdown && !m_model->remoteMarkerPending()) {
        if(m_client->remoteOwnerOwned() && !m_session.isEmpty()) {m_retiring=true;m_phase=tr("Waiting for downloads to stop safely…");send(C::RemoteClose);emit changed();}
        return;
    }
    if(m_session.isEmpty())m_session=freshId();
    if(!m_openReady) {send(C::RemoteOpen);return;}
    send(C::RemoteList);
    send(C::RemoteReplaceList);
    if(!m_replaceParams.isEmpty() && !m_shutdown)send(C::RemoteReplaceTarget,m_replaceParams);
    if(!m_skipParams.isEmpty() && !m_shutdown)send(C::RemoteReplaceSkip,m_skipParams);
    if(!m_confirmationParams.isEmpty())send(C::RemoteReplaceTarget,m_confirmationParams);
    if(!m_appendParams.isEmpty())send(C::RemoteReplaceTarget,m_appendParams);
    if(!m_resolveParams.isEmpty()) {
        if(!m_resolveAdmitted)send(C::RemoteResolve,m_resolveParams);
        else send(C::RemoteResolveStatus,{{"requestKey",m_resolveKey},{"page",m_filePage}});
    }
    for(const auto &key:std::as_const(m_resolveReleases))send(C::RemoteResolveRelease,{{"requestKey",key}});
    if(!m_prepareParams.isEmpty() && !m_prepareAdmitted && m_releaseKey.isEmpty() && !m_committing)send(C::RemotePrepare,m_prepareParams);
    if(!m_reviewParams.isEmpty() && !m_reviewAdmitted && m_releaseKey.isEmpty() && !m_committing)send(replacementJob()?C::RemoteReplaceReview:C::RemoteReview,m_reviewParams);
    if(!m_replayParams.isEmpty())send(m_replayCall,m_replayParams);
    if(!m_releaseKey.isEmpty())send(C::RemotePrepareRelease,{{"jobId",m_releaseJob},{"prepareKey",m_releaseKey}});
    // A status error can consume an unadmitted Start's retained error. Poll the
    // exact Start until the matching admitted job appears in the current
    // owner's purpose-specific list; only then ask for its fully bound status.
    const auto pendingIds=m_pendingStarts.keys();
    for(const auto &id:pendingIds) {
        const auto start=m_pendingStarts.value(id);if(start.isEmpty())continue;
        send(start.contains("targetDigest")?C::RemoteReplaceStart:C::RemoteStart,start);
        if(m_pendingStarts.contains(id))requestJobStatus(id);
    }
    for(auto it=m_pendingControls.cbegin();it!=m_pendingControls.cend();++it)send(C::RemoteControl,it.value());
    for(auto it=m_installStages.cbegin();it!=m_installStages.cend();++it)
        if(it.value()=="download" && it.key()!=m_jobId)requestJobStatus(it.key());
    for(const auto &id:m_deferredActions.keys())if(id!=m_jobId)requestJobStatus(id);
    if(!m_jobId.isEmpty())requestJobStatus(m_jobId);
    if(!m_reconcileJob.isEmpty() && m_reconcileJob!=m_jobId)requestJobStatus(m_reconcileJob);
}
bool RemoteController::pendingStartListed(const QString &id) const {
    const auto pin=m_startPins.value(id);if(pin.isEmpty())return false;
    const bool replacement=pin.contains("targetDigest");
    if(replacement?!m_replaceListCurrent:!m_addListCurrent)return false;
    const auto &rows=replacement?m_replaceJobs:m_addJobs;
    for(const auto &v:rows) {
        const auto row=v.toMap();
        if(row.value("jobId").toString()==id && row.value("profileId").toString()==pin["profileId"].toString()
            && QJsonObject::fromVariantMap(row.value("submission").toMap())==pin["submission"].toObject()
            && (!replacement || (row.value("purpose").toString()==pin["purpose"].toString()
                && row.value("modId").toString()==pin["modId"].toString()
                && row.value("targetDigest").toString()==pin["targetDigest"].toString())))return true;
    }
    return false;
}
void RemoteController::requestJobStatus(const QString &id) {
    if(m_pendingStarts.contains(id) && !pendingStartListed(id))return;
    send(statusCall(id),{{"jobId",id}});
}
BackendClient::Call RemoteController::statusCall(const QString &id) const {
    if(m_startPins.value(id).contains("targetDigest") || m_pendingStarts.value(id).contains("targetDigest") || (id==m_reconcileJob && m_model->replacementMarkerPending()))return C::RemoteReplaceStatus;
    if(!m_statuses.value(id).value("purpose").toString().isEmpty())return C::RemoteReplaceStatus;
    for(const auto &v:m_replaceJobs)if(v.toMap().value("jobId").toString()==id)return C::RemoteReplaceStatus;
    return C::RemoteStatus;
}
bool RemoteController::replacementContextCurrent() const {
    return !m_replaceAction.isEmpty() && m_replaceProfile==m_model->viewedProfileId() && m_replaceRevision==m_model->revision()
        && m_replaceTarget.value("profileId").toString()==m_replaceProfile && m_replaceTarget.value("modId").toString()==m_replaceMod
        && m_replaceTarget.value("revision").toString()==m_replaceRevision && m_replaceParams.isEmpty()
        && m_replaceObservationAge.isValid() && !m_replaceObservationAge.hasExpired(300000)
        && (m_replaceAction=="resume" || (replacementJob() && !m_jobId.isEmpty() && m_job.value("modId").toString()==m_replaceMod)
            || (m_batchReplacement && m_replaceAction=="update") || m_replaceMod==m_model->selectedId());
}
QVariantMap RemoteController::replacementSubmission() const {
    const auto receipt=m_replaceTarget.value("receipt").toMap();
    if(!receipt.isEmpty())return receipt.value("submission").toMap();
    if(m_replaceTarget.value("modId").toString()!=m_replaceMod)return {};
    const auto mod=m_model->modDetails(m_replaceMod);
    return mod.value("linkEligible").toBool()?QVariantMap{}:mod.value("remoteSubmission").toMap();
}
bool RemoteController::replacementFileSelection() const {
    return (m_replaceAction=="update" || m_replaceAction=="check")
        && m_replaceTarget.value("receipt").toMap().isEmpty() && !replacementSubmission().isEmpty();
}
void RemoteController::requestReplacementObservation() {
    if(m_replaceProfile.isEmpty() || m_replaceMod.isEmpty() || m_replaceRevision.isEmpty())return;
    m_replaceObservationAge.invalidate();m_replaceExpiryNotified=false;
    m_replaceParams={{"requestKey",freshId()},{"profileId",m_replaceProfile},{"modId",m_replaceMod},{"expectedRevision",m_replaceRevision}};
}
bool RemoteController::beginReplacement(const QString &modId,const QString &action) {
    return beginReplacementFor(modId,action,false);
}
bool RemoteController::beginBatchReplacement(const QString &modId) {
    return beginReplacementFor(modId,QStringLiteral("update"),true);
}
bool RemoteController::beginReplacementFor(const QString &modId,const QString &action,bool batch) {
    if(!replacementAvailable() || !m_model->remoteAdmissionAvailable() || !m_prepareKey.isEmpty() || !m_releaseKey.isEmpty()
        || !m_resolveKey.isEmpty() || !m_skipParams.isEmpty() || !QStringList{"check","update","reinstall","link"}.contains(action)
        || (!batch && modId!=m_model->selectedId()) || modId.isEmpty() || m_model->modDetails(modId).isEmpty())return false;
    m_downloadsRequested=false;m_batchReplacement=batch;m_replaceAction=action;m_replaceProfile=m_model->viewedProfileId();m_replaceMod=modId;m_replaceRevision=m_model->revision();
    m_jobId.clear();m_job.clear();m_replaceTarget.clear();m_replaceSkip.clear();m_error.clear();m_visible=!batch || !m_model->m_updateBatchConfirmed;m_phase=tr("Checking this mod's download details…");
    requestReplacementObservation();m_poll.start();pulse();emit changed();return true;
}
bool RemoteController::refreshReplacementJob() {
    if(!replacementJob() || !replacementAvailable() || !m_model->remoteAdmissionAvailable() || m_resolveReleases.size()+int(!m_skipHeldResolve.isEmpty())>=8
        || m_job.value("profileId").toString()!=m_model->viewedProfileId() || (!m_skipParams.isEmpty() && m_job.value("purpose")=="update")
        || !m_prepareKey.isEmpty() || !m_releaseKey.isEmpty())return false;
    renewResolution();
    m_batchReplacement=false;m_replaceAction=QStringLiteral("resume");m_replaceProfile=m_model->viewedProfileId();m_replaceMod=m_job.value("modId").toString();m_replaceRevision=m_model->revision();
    m_replaceTarget.clear();m_replaceSkip.clear();m_error.clear();requestReplacementObservation();m_poll.start();pulse();emit changed();return true;
}
bool RemoteController::refreshReplacementObservation() {
    if(!replacementActive() || !replacementAvailable() || !m_model->remoteAdmissionAvailable()
        || !m_replaceParams.isEmpty() || m_resolveReleases.size()+int(!m_skipHeldResolve.isEmpty())>=8)return false;
    if(replacementJob())return refreshReplacementJob();
    if((!m_batchReplacement && m_replaceMod!=m_model->selectedId()) || m_replaceProfile!=m_model->viewedProfileId())return false;
    renewResolution();
    m_replaceRevision=m_model->revision();m_replaceTarget.clear();m_replaceSkip.clear();m_error.clear();
    requestReplacementObservation();m_poll.start();pulse();emit changed();return true;
}
void RemoteController::confirmReplacementTarget(const QVariantMap &marker,const QString &revision) {
    const auto object=QJsonObject::fromVariantMap(marker);
    if(!RemoteData::replacementMarker(object,m_root) || !LaunchData::decimal(QJsonValue(revision)))return;
    m_confirmationParams={{"requestKey",freshId()},{"profileId",object["profileId"]},{"modId",object["modId"]},{"expectedRevision",revision}};
    m_poll.start();pulse();
}
bool RemoteController::resolveInstallLink(const QVariantMap &submission, const QString &fileId) {
    if(!resolve(submission))return false;
    m_linkedFileId=fileId;emit changed();return true;
}
bool RemoteController::resolve(const QVariantMap &submissionValue) {
    const auto s=QJsonObject::fromVariantMap(submissionValue);
    if(!canResolve() || !RemoteData::submission(s) || (replacementActive() && (!replacementContextCurrent()
        || ((m_replaceAction!="link" && m_job.value("purpose")!="link") && s!=QJsonObject::fromVariantMap(replacementSubmission())))))return false;
    m_queueJob.clear();m_downloadsRequested=false;m_linkedFileId.clear();
    m_visible=replacementActive() && (!m_batchReplacement || !m_model->m_updateBatchConfirmed);m_collectFiles=!replacementActive() || replacementFileSelection();m_choosingFiles=false;m_pickedFiles.clear();m_candidates.clear();m_submission=s;m_resolveKey=freshId();m_resolveParams={{"requestKey",m_resolveKey},{"submission",s}};
    m_resolveAdmitted=false;m_selectionToken.clear();m_files.clear();m_fileEvidence.clear();m_filePageEvidence.clear();m_seenFilePages.clear();m_lastFilePage=-1;m_savedFileObservation.clear();m_filePage=0;m_fileHasMore=false;m_error.clear();m_phase=tr("Resolving fresh files…");
    m_poll.start();pulse();emit changed();return true;
}
bool RemoteController::loadFiles(int pageNumber) {
    if(m_shutdown || m_resolveKey.isEmpty() || pageNumber<0 || pageNumber>=16 || (pageNumber>m_filePage && !m_fileHasMore))return false;
    m_filePage=pageNumber;m_files.clear();send(C::RemoteResolveStatus,{{"requestKey",m_resolveKey},{"page",m_filePage}});emit changed();return true;
}
bool RemoteController::start(const QString &fileId,const QString &fingerprint,bool acceptArchived) {
    if(replacementActive() || !available() || !m_model->remoteAdmissionAvailable() || m_selectionToken.isEmpty() || m_pendingStarts.size()>=8)return false;
    const auto profile=m_model->viewedProfileId(), revision=m_model->revision();
    for(auto it=m_pendingStarts.cbegin();it!=m_pendingStarts.cend();++it) {
        if(!it.value().contains("targetDigest") && it.value()["profileId"]==QJsonValue(profile)
            && m_startPins.value(it.key())["submission"].toObject()==m_submission) {
            m_error=tr("The previous download is still being checked. Wait for its result.");emit changed();return false;
        }
    }
    QVariantMap selected;
    for(const auto &v:m_files) {const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==fileId && row.value("candidateFingerprint").toString()==fingerprint){selected=row;break;}}
    if(selected.isEmpty() || ((selected.value("list")=="archived" || selected.value("archiveFlag")==true || selected.value("archiveFlag").isNull()) && !acceptArchived))return false;
    const auto job=freshId();
    QJsonObject p{{"jobId",job},{"selectionToken",m_selectionToken},{"candidateFingerprint",fingerprint},{"fileId",fileId},{"profileId",profile},{"expectedRevision",revision},{"acceptArchived",acceptArchived}};
    auto pin=p;pin["submission"]=m_submission;m_startPins.insert(job,pin);
    const auto detail=m_model->catalog()->detail();
    auto summary=detail.value("summary").toMap();
    if(QJsonObject::fromVariantMap(summary.value("submission").toMap())==m_submission && !detail.value("description").toString().isEmpty())
        summary["description"]=detail.value("description");
    if(QJsonObject::fromVariantMap(summary.value("submission").toMap())!=m_submission) {
        summary.clear();
        auto *catalog=m_model->catalog()->model();
        for(int row=0;row<catalog->rowCount();++row) {
            const auto item=catalog->data(catalog->index(row),CatalogModel::Metadata).toMap();
            if(QJsonObject::fromVariantMap(item.value("submission").toMap())==m_submission){summary=item;break;}
        }
    }
    QVariantMap form;
    for(const auto *key:{"name","author","category","description"}) {
        auto value=summary.value(QString::fromLatin1(key)=="name"?"title":key).toString();
        if(QString::fromLatin1(key)=="name" && value.trimmed().isEmpty())value=selected.value("submissionName").toString();
        if(QString::fromLatin1(key)=="name" && value.trimmed().isEmpty())value=selected.value("originalName").toString();
        if(QString::fromLatin1(key)=="name" && value.trimmed().isEmpty())value=tr("Downloaded mod");
        const int limit=QString::fromLatin1(key)=="description"?16384:QString::fromLatin1(key)=="category"?128:256;
        QString clean;
        for(const auto c:value)if(c.category()!=QChar::Other_Control || (QString::fromLatin1(key)=="description" && (c=='\n'||c=='\t')))clean+=c;
        const auto encoded=clean.toUtf8();
        if(encoded.size()>limit) {
            auto end=limit;
            while((quint8(encoded[end]) & 0xc0)==0x80)--end;
            clean=QString::fromUtf8(encoded.constData(),end);
        }
        if(QString::fromLatin1(key)=="name" && clean.trimmed().isEmpty())clean=tr("Downloaded mod");
        form.insert(key,clean);
    }
    m_installMetadata.insert(job,form);m_installStages.insert(job,"download");
    if(m_downloadQueue.isEmpty())m_queueJob=job;
    m_pendingStarts.insert(job,p);m_consents.insert(job,fingerprint);
    // A fresh background download must not detach a live preparation key from
    // the job that owns it. The new job can be selected after admission.
    if(m_prepareKey.isEmpty() && !m_committing){m_jobId=job;m_job.clear();}
    m_error.clear();m_phase=tr("Starting download…");
    if(!m_appendTarget.isEmpty()) {
        m_installTargets[job]=m_appendTarget;
        p["purpose"]=m_nextReplacementPurpose.isEmpty()?QStringLiteral("append"):m_nextReplacementPurpose;p["modId"]=m_appendTarget.value("modId").toString();p["targetDigest"]=m_appendTarget.value("recordDigest").toString();
        pin=p;pin["submission"]=m_submission;m_startPins[job]=pin;m_pendingStarts[job]=p;
        if(m_jobId==job){m_job["purpose"]=p["purpose"].toString();m_job["modId"]=p["modId"].toString();m_job["targetDigest"]=p["targetDigest"].toString();}
    }
    m_appendTarget.clear();
    m_nextReplacementPurpose.clear();
    send(p.contains("purpose")?C::RemoteReplaceStart:C::RemoteStart,p);emit changed();return true;
}
bool RemoteController::startReplacement(const QString &fileId,const QString &fingerprint,const QString &purpose,bool acceptArchived) {
    if(!replacementAvailable() || !m_model->remoteAdmissionAvailable() || !replacementContextCurrent()
        || m_selectionToken.isEmpty() || m_pendingStarts.size()>=8 || m_replaceAction=="resume"
        || !QStringList{"update","force","reinstall","link"}.contains(purpose))return false;
    for(const auto &pending:std::as_const(m_pendingStarts)) {
        if(pending.contains("targetDigest") && pending["profileId"]==QJsonValue(m_replaceProfile) && pending["modId"]==QJsonValue(m_replaceMod)) {
            m_error=tr("The previous download is still being checked. Wait for its result.");emit changed();return false;
        }
    }
    if((m_replaceAction=="link")!=(purpose=="link"))return false;
    if((m_replaceAction=="reinstall")!=(purpose=="reinstall"))return false;
    QVariantMap selected;
    for(const auto &v:m_files) {const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==fileId
        && row.value("candidateFingerprint").toString()==fingerprint){selected=row;break;}}
    if(selected.isEmpty() || ((selected.value("list")=="archived" || selected.value("archiveFlag")==true || selected.value("archiveFlag").isNull()) && !acceptArchived))return false;
    const auto skip=m_replaceSkip.value("entry").toMap();const auto state=m_replaceSkip.value("state").toString();
    const bool matches=state=="present" && skip.value("fileId").toString()==fileId && skip.value("candidateFingerprint").toString()==fingerprint;
    const bool uncertain=state=="unavailable" || !m_skipParams.isEmpty();
    if((purpose=="update" && (matches || uncertain)) || (purpose=="force" && !matches && !uncertain))return false;
    if(purpose=="reinstall" && fileId!=m_replaceTarget.value("receipt").toMap().value("rootFileId").toString())return false;
    const auto job=freshId();
    QJsonObject p{{"jobId",job},{"selectionToken",m_selectionToken},{"candidateFingerprint",fingerprint},{"fileId",fileId},
        {"profileId",m_replaceProfile},{"modId",m_replaceMod},{"expectedRevision",m_replaceRevision},
        {"targetDigest",m_replaceTarget.value("recordDigest").toString()},{"purpose",purpose},{"acceptArchived",acceptArchived}};
    auto pin=p;pin["submission"]=m_submission;m_startPins.insert(job,pin);
    m_pendingStarts.insert(job,p);m_consents.insert(job,fingerprint);
    if(m_prepareKey.isEmpty() && !m_committing){m_jobId=job;m_job.clear();m_job["purpose"]=purpose;m_job["modId"]=m_replaceMod;m_job["targetDigest"]=m_replaceTarget.value("recordDigest");}
    { // Each explicit replacement action continues through the reviewed install path.
        m_installTargets[job]=m_replaceTarget;
        m_installMetadata[job]=m_replaceTarget.value("metadata").toMap();
        m_installStages[job]="download";
        m_queueProfile=m_replaceProfile;
        if(m_downloadQueue.isEmpty())m_queueJob=job;
        m_replaceAction.clear();m_replaceParams={};m_replaceTarget.clear();m_replaceSkip.clear();m_visible=false;
    }
    m_error.clear();m_phase=tr("Starting download…");send(C::RemoteReplaceStart,p);emit changed();return true;
}
bool RemoteController::changeSkip(const QString &fileId,const QString &fingerprint,bool clear) {
    if(!replacementAvailable() || !m_model->remoteAdmissionAvailable() || !replacementContextCurrent() || !m_skipParams.isEmpty()
        || m_replaceSkip.value("state")=="unavailable")return false;
    QVariantMap selected;
    if(!clear)for(const auto &v:m_files) {const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==fileId
        && row.value("candidateFingerprint").toString()==fingerprint){selected=row;break;}}
    if((clear && m_replaceSkip.value("entry").toMap().isEmpty()) || (!clear && (selected.isEmpty() || m_selectionToken.isEmpty())))return false;
    const auto old=m_replaceSkip.value("entry").toMap();
    const QJsonValue expected=old.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(QJsonObject::fromVariantMap(old));
    const QJsonValue desired=clear?QJsonValue(QJsonValue::Null):QJsonValue(QJsonObject{{"fileId",fileId},{"candidateFingerprint",fingerprint}});
    m_skipParams={{"requestKey",freshId()},{"profileId",m_replaceProfile},{"modId",m_replaceMod},{"expectedRevision",m_replaceRevision},
        {"targetDigest",m_replaceTarget.value("recordDigest").toString()},{"expectedSkip",expected},{"desiredSkip",desired},
        {"selectionToken",clear?QJsonValue(QJsonValue::Null):QJsonValue(m_selectionToken)}};
    m_error.clear();m_phase=tr("Saving the skipped-file choice…");send(C::RemoteReplaceSkip,m_skipParams);emit changed();return true;
}
bool RemoteController::bindConsent(const QString &jobId,const QString &fileId,const QString &fingerprint,bool acknowledge) {
    if(jobId!=m_jobId || m_job.isEmpty() || m_submission!=QJsonObject::fromVariantMap(m_job.value("submission").toMap()))return false;
    if(replacementJob() && !installTargetCurrent())return false;
    const auto members=m_job.value("cohort").toMap().value("members").toList();
    if(members.isEmpty() || members.first().toMap().value("fileId").toString()!=fileId)return false;
    for(const auto &v:m_files) {const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==fileId && row.value("candidateFingerprint").toString()==fingerprint){
        if((row.value("list")=="archived" || row.value("archiveFlag")==true || row.value("archiveFlag").isNull()) && !acknowledge)return false;
        if(replacementJob() && m_consents.contains(jobId) && m_consents.value(jobId)!=fingerprint) {
            m_error=tr("File details changed. Start a new action.");emit changed();return false;
        }
        m_consents[jobId]=fingerprint;
        if(m_continuingJob==jobId && m_installMetadata.value(jobId).isEmpty())
            m_installMetadata[jobId]={{"name",row.value("submissionName",installName(jobId))},{"author",""},{"category",""},{"description",""}};
        emit changed();return true;}}
    return false;
}
bool RemoteController::selectJob(const QString &jobId) {
    if(!LaunchData::hex(jobId,32) || !m_skipParams.isEmpty())return false;
    const bool known=m_pendingStarts.contains(jobId) || std::any_of(m_jobs.cbegin(),m_jobs.cend(),[&](const QVariant &v){return v.toMap().value("jobId").toString()==jobId;});
    if(!known)return false;
    if(jobId==m_jobId){requestJobStatus(jobId);return true;}
    m_continuingJob.clear();
    if(!m_prepareKey.isEmpty() && !m_committing && !m_releaseKey.isEmpty())return false;
    if(jobId!=m_jobId && !m_prepareKey.isEmpty() && !m_committing){m_releaseJob=m_jobId;m_releaseKey=m_prepareKey;send(C::RemotePrepareRelease,{{"jobId",m_releaseJob},{"prepareKey",m_releaseKey}});}
    if(replacementActive() || statusCall(jobId)==C::RemoteReplaceStatus) {
        releaseResolution();m_replaceAction.clear();m_replaceParams={};m_replaceTarget.clear();m_replaceSkip.clear();
    }
    if(QStringList{"candidates","review","choice"}.contains(m_installStages.value(m_jobId))) {
        m_installStages[m_jobId]="stopped";
        ++m_failureSequences[m_jobId];m_installErrors[m_jobId]=tr("This download needs another review. Try again.");
    }
    m_jobId=jobId;m_job=m_statuses.value(jobId);m_prepareKey.clear();m_prepareParams={};m_prepareAdmitted=false;m_candidatesLoaded=false;resetReview();m_selectedIds.clear();m_selectable.clear();m_seenCandidates.clear();m_candidateEvidence.clear();m_candidateNames.clear();m_candidates.clear();m_members.clear();m_diagnostics.clear();
    m_error.clear();requestJobStatus(jobId);emit changed();return true;
}
QString RemoteController::nextControlSeq() const {return controlSequence(m_job);}
bool RemoteController::control(const QString &job,const QString &seq,const QString &action) {
    if(seq.isEmpty()) {
        const bool known=std::any_of(m_jobs.cbegin(),m_jobs.cend(),[&](const QVariant &v){return v.toMap().value("jobId")==job;});
        if(m_shutdown || !known || !QStringList{"pause","resume","retry","cancel","dismiss","install"}.contains(action) || m_deferredActions.contains(job) || m_pendingControls.contains(job))return false;
        m_deferredActions[job]=action;requestJobStatus(job);return true;
    }
    const auto status=job==m_jobId?m_job:m_statuses.value(job);
    if(m_shutdown || (job!=m_jobId && status.isEmpty()) || seq!=controlSequence(status) || !QStringList{"pause","resume","retry","cancel","abandon","dismiss"}.contains(action) || m_pendingControls.contains(job))return false;
    if(m_installStages.contains(job)) {
        if(QStringList{"cancel","abandon","dismiss"}.contains(action)) {
            m_installStages.remove(job);clearInstallQueue(job);
        } else m_installStages[job]=(action=="retry" || action=="resume")?"download":"stopped";
        m_installErrors.remove(job);
    }
    const QJsonObject p{{"jobId",job},{"controlSeq",seq},{"action",action}};m_pendingControls.insert(job,p);m_error.clear();if(action=="retry")emit retryStarted(job);send(C::RemoteControl,p);emit changed();return true;
}
void RemoteController::continueDownloaded() {
    if(m_job.value("state")!="downloaded" || !available() || !m_model->remoteAdmissionAvailable())return;
    if(m_job.value("profileId").toString()!=m_model->viewedProfileId()) {
        m_error=tr("Switch to this download's profile to add it to your library.");emit changed();return;
    }
    m_installErrors.remove(m_jobId);
    if(!hasConsent() || (replacementJob() && !installTargetCurrent())) {
        m_continuingJob=m_jobId;renewResolution();
        const bool started=replacementJob()?refreshReplacementJob():resolve(m_job.value("submission").toMap());
        if(!started){m_continuingJob.clear();m_error=tr("Wait for other work to finish, then try adding this download again.");emit changed();}
        return;
    }
    if(replacementJob() && !automaticReplacementJob()) {
        m_installTargets[m_jobId]=m_replaceTarget;m_installMetadata[m_jobId]=m_replaceTarget.value("metadata").toMap();
    }
    if(m_installMetadata.value(m_jobId).isEmpty())m_installMetadata[m_jobId]={{"name",installName(m_jobId)},{"author",""},{"category",""},{"description",""}};
    m_continuingJob.clear();m_choosingFiles=false;m_error.clear();
    m_replaceAction.clear();m_replaceParams={};m_replaceTarget.clear();m_replaceSkip.clear();releaseResolution();
    m_installStages[m_jobId]="download";advanceInstall();emit changed();
}
bool RemoteController::prepare(const QString &job,bool acknowledgeRebase) {
    if(!available() || !m_model->remoteAdmissionAvailable() || m_committing || job!=m_jobId || m_job.value("state")!="downloaded" || !m_prepareKey.isEmpty() || !m_releaseKey.isEmpty()
        || m_job.value("profileId").toString()!=m_model->viewedProfileId() || m_consents.value(job).isEmpty()
        || (replacementJob() && !installTargetCurrent()))return false;
    const auto initial=m_job.value("initialRevision").toString();
    if(initial!=m_model->revision() && !acknowledgeRebase)return false;
    m_prepareKey=freshId();m_prepareParams={{"jobId",job},{"profileId",m_model->viewedProfileId()},{"expectedRevision",m_model->revision()},{"acknowledgeRebase",acknowledgeRebase},{"prepareKey",m_prepareKey}};
    m_prepareAdmitted=false;m_candidatesLoaded=false;resetReview();m_selectedIds.clear();m_selectable.clear();m_seenCandidates.clear();m_candidateEvidence.clear();m_candidateNames.clear();m_candidates.clear();m_error.clear();m_installErrors.remove(job);m_phase=tr("Preparing all available choices…");send(C::RemotePrepare,m_prepareParams);emit changed();return true;
}
bool RemoteController::releasePreparationForRetry() {
    if(!canReleasePreparation() || m_job.value("state")!="downloaded")return false;
    m_releaseJob=m_jobId;m_releaseKey=m_prepareKey;
    m_phase=tr("Waiting for the preparation attempt to be released…");
    send(C::RemotePrepareRelease,{{"jobId",m_releaseJob},{"prepareKey",m_releaseKey}});emit changed();return true;
}
bool RemoteController::loadCandidates(const QString &job,const QString &key,int pageNumber) {
    if(!currentPreparation(job,key) || pageNumber<0 || pageNumber>=128 || (pageNumber>m_candidatePage && !m_candidateHasMore))return false;
    m_candidatePage=pageNumber;m_candidates.clear();m_candidateHasMore=false;send(C::RemoteCandidates,{{"jobId",job},{"prepareKey",key},{"page",pageNumber}});emit changed();return true;
}
bool RemoteController::loadMembers(const QString &job,const QString &key,const QString &candidateId,int pageNumber) {
    if(!currentPreparation(job,key) || !m_seenCandidates.contains(candidateId) || pageNumber<0 || pageNumber>=1250
        || (candidateId==m_memberCandidate && pageNumber>m_memberPage && !m_memberHasMore))return false;
    m_memberCandidate=candidateId;m_memberPage=pageNumber;m_members.clear();m_memberHasMore=false;send(C::RemoteCandidateMembers,{{"jobId",job},{"prepareKey",key},{"candidateId",candidateId},{"page",pageNumber}});emit changed();return true;
}
bool RemoteController::loadDiagnostics(const QString &job,const QString &key,int pageNumber) {
    if(!currentPreparation(job,key) || pageNumber<0 || pageNumber>=1250 || (pageNumber>m_diagnosticPage && !m_diagnosticHasMore))return false;
    m_diagnosticPage=pageNumber;m_diagnostics.clear();m_diagnosticHasMore=false;send(C::RemoteDiagnostics,{{"jobId",job},{"prepareKey",key},{"page",pageNumber}});emit changed();return true;
}
void RemoteController::setSelectedIds(QStringList ids) {
    std::sort(ids.begin(),ids.end());ids.removeDuplicates();
    if(ids.size()>99 || std::any_of(ids.cbegin(),ids.cend(),[this](const QString &id){return !m_selectable.contains(id);})) {
        m_error=tr("Choose at most 99 available choices from the current preparation.");emit changed();return;
    }
    if(ids==m_selectedIds)return;m_selectedIds=ids;invalidateReview();emit changed();
}
QStringList RemoteController::selectedChoiceNames() const {
    QStringList names;for(const auto &id:m_selectedIds)names.append(m_candidateNames.value(id,tr("Selected choice")));return names;
}
void RemoteController::resetReview() {m_review.clear();m_reviewKey.clear();m_reviewParams={};m_reviewAdmitted=false;}
void RemoteController::invalidateReview() {resetReview();emit changed();}
bool RemoteController::reviewSelection(const QString &job,const QStringList &ids,const QVariantMap &metadataValue) {
    auto sorted=ids;std::sort(sorted.begin(),sorted.end());sorted.removeDuplicates();const auto form=QJsonObject::fromVariantMap(metadataValue);
    if(!available() || !m_model->remoteAdmissionAvailable() || !currentPreparation(job,m_prepareKey) || sorted!=m_selectedIds || sorted.isEmpty() || sorted.size()>99
        || (replacementJob() ? (!installTargetCurrent()
            || m_job.value("reviewRevision").toString()!=m_prepareParams["expectedRevision"].toString()
            || m_job.value("profileId").toString()!=m_prepareParams["profileId"].toString()) : !metadata(form)))return false;
    m_reviewKey=freshId();m_review.clear();QJsonArray selected;for(const auto &id:sorted)selected.append(id);
    m_reviewParams={{"jobId",job},{"profileId",m_prepareParams["profileId"]},{"expectedRevision",m_prepareParams["expectedRevision"]},{"requestKey",m_reviewKey},{"candidateIds",selected}};
    if(replacementJob()){m_reviewParams["modId"]=m_job.value("modId").toString();m_reviewParams["targetDigest"]=m_job.value("targetDigest").toString();}
    else m_reviewParams["metadata"]=form;
    m_reviewAdmitted=false;m_error.clear();m_phase=tr("Reviewing selected choices…");send(replacementJob()?C::RemoteReplaceReview:C::RemoteReview,m_reviewParams);emit changed();return true;
}
bool RemoteController::canCommit() const {
    return available() && m_model->remoteAdmissionAvailable() && !m_committing && m_releaseKey.isEmpty() && !m_reviewKey.isEmpty()
        && m_review.value("requestKey").toString()==m_reviewKey && m_review.value("phase")=="ready"
        && m_job.value("prepareKey").toString()==m_prepareKey && m_job.value("cohort").toMap().value("complete").toBool()
        && !m_consents.value(m_jobId).isEmpty() && m_prepareParams["profileId"]==QJsonValue(m_model->viewedProfileId())
        && m_prepareParams["expectedRevision"]==QJsonValue(m_model->revision())
        && (!replacementJob() || (installTargetCurrent()
            && m_job.value("reviewRevision").toString()==m_prepareParams["expectedRevision"].toString()
            && m_job.value("profileId").toString()==m_prepareParams["profileId"].toString()
            && (m_job.value("purpose")!="update" || m_skipParams.isEmpty())));
}
bool RemoteController::commit(const QString &job) {
    if(job!=m_jobId || !canCommit())return false;
    m_committing=true;m_commitJobId=job;
    if(m_installStages.contains(job))m_installResultMods[job]=replacementJob()?m_job.value("modId").toString():m_review.value("modId").toString();
    const bool accepted=replacementJob()?m_model->dispatchReplacementCommit(m_job,m_review,m_consents.value(job),m_prepareParams["expectedRevision"].toString()):m_model->dispatchRemoteCommit(job,m_prepareParams["profileId"].toString(),m_prepareParams["expectedRevision"].toString(),
        m_review.value("reviewToken").toString(),m_review.value("modId").toString(),m_consents.value(job),
        m_job.value("cohort").toMap().value("fingerprint").toString(),m_review.value("selectionDigest").toString());
    if(!accepted){m_committing=false;m_commitJobId.clear();}else m_phase=replacementJob()?tr("Saving file choices…"):tr("Saving the disabled library entry…");emit changed();return accepted;
}
void RemoteController::releaseResolution() {
    if(!m_resolveKey.isEmpty()){m_resolveReleases.insert(m_resolveKey);send(C::RemoteResolveRelease,{{"requestKey",m_resolveKey}});m_resolveParams={};m_resolveKey.clear();m_selectionToken.clear();m_files.clear();m_fileEvidence.clear();m_filePageEvidence.clear();}
    emit changed();
}
void RemoteController::renewResolution() {
    if(m_resolveKey.isEmpty())return;
    if(!m_skipParams.isEmpty() && m_skipHeldResolve.isEmpty()) {
        m_skipHeldResolve=m_resolveKey;m_resolveParams={};m_resolveKey.clear();m_selectionToken.clear();m_files.clear();m_fileEvidence.clear();m_filePageEvidence.clear();
    } else releaseResolution();
}
void RemoteController::releaseSkipResolution() {
    if(m_skipHeldResolve.isEmpty())return;
    const auto key=std::exchange(m_skipHeldResolve,{});m_resolveReleases.insert(key);send(C::RemoteResolveRelease,{{"requestKey",key}});
}
void RemoteController::invalidateReplacementResolution() {
    renewResolution();m_replaceTarget.clear();m_replaceSkip.clear();m_savedFileObservation.clear();m_seenFilePages.clear();m_lastFilePage=-1;
}
void RemoteController::close() {
    m_continuingJob.clear();m_visible=false;m_choosingFiles=false;
    if(m_model->batchUpdateReview())m_model->cancelRemainingUpdates();
    else if(m_model->updatingCount()>0) {emit changed();return;}
    if(!m_downloadQueue.isEmpty()) {emit changed();return;}
    const auto stage=m_installStages.value(m_jobId);
    const bool installing=stage=="candidates" || stage=="choice" || stage=="review";
    if(!m_prepareKey.isEmpty() && !m_committing && !installing) {m_releaseJob=m_jobId;m_releaseKey=m_prepareKey;m_prepareParams={};m_prepareAdmitted=false;
        send(C::RemotePrepareRelease,{{"jobId",m_releaseJob},{"prepareKey",m_releaseKey}});m_prepareKey.clear();resetReview();m_selectedIds.clear();m_selectable.clear();m_seenCandidates.clear();m_candidateEvidence.clear();m_candidateNames.clear();}
    if(m_skipParams.isEmpty()){releaseResolution();m_replaceParams={};m_replaceAction.clear();m_replaceTarget.clear();m_replaceSkip.clear();}
    emit changed();
}
void RemoteController::shutdown() {m_shutdown=true;m_visible=false;m_retiring=m_client->remoteOwnerOwned();if(m_retiring || m_model->remoteMarkerPending()){m_poll.start();pulse();}emit changed();}
void RemoteController::commitSettled(bool success,const QString &message) {
    const auto id=std::exchange(m_commitJobId,{});
    const bool replacement=m_replayCall==C::RemoteReplaceCommit;
    m_committing=false;m_reconcileJob.clear();m_replayParams={};
    if(!id.isEmpty() && (!replacement || m_installTargets.contains(id))) {
        if(success){
            if(id==m_jobId){m_prepareKey.clear();m_prepareParams={};resetReview();m_visible=false;}
            m_installStages[id]="done";m_installCompleted[id]=QDateTime::currentMSecsSinceEpoch();m_appendTarget.clear();
        } else {
            m_installStages[id]="stopped";
            ++m_failureSequences[id];m_installErrors[id]=message.isEmpty()?tr("Could not add this download. Try again."):message;
            clearInstallQueue(id);
        }
    }
    m_error=message;m_phase=success?(replacement?tr("File choices saved."):tr("Added to the library, disabled.")):tr("The saved operation was checked.");emit changed();
}
void RemoteController::reconcileStatus(const QString &job) {if(!job.isEmpty()){m_reconcileJob=job;m_poll.start();pulse();}}
void RemoteController::replayCommit(const QVariantMap &marker,const QString &token) {
    m_commitJobId=marker.value("jobId").toString();
    m_replayParams={{"jobId",marker.value("jobId").toString()},{"profileId",marker.value("profileId").toString()},
        {"expectedRevision",marker.value("expectedRevision").toString()},{"reviewToken",token.isEmpty()?QJsonValue(QJsonValue::Null):QJsonValue(token)},
        {"operationId",marker.value("operationId").toString()},{"consentFingerprint",marker.value("consentFingerprint").toString()},
        {"cohortFingerprint",marker.value("cohortFingerprint").toString()},{"selectionDigest",marker.value("selectionDigest").toString()}};
    m_replayCall=marker.value("method").toInt()==int(C::RemoteReplaceCommit)?C::RemoteReplaceCommit:C::RemoteCommit;
    if(m_replayCall==C::RemoteReplaceCommit)for(const auto *key:{"modId","purpose","initialRevision","targetDigest","afterRecordDigest"})
        m_replayParams[QLatin1String(key)]=marker.value(QLatin1String(key)).toString();
    m_poll.start();pulse();
}
void RemoteController::received(C call,const QJsonObject &request,const QVariantMap &result) {
    if(!current(request))return;
    m_inFlight.remove(requestIdentity(call,request));
    switch(call) {
    case C::RemoteOpen: {
        if(m_shutdown && !m_model->remoteMarkerPending())break;
        const auto phase=result.value("phase").toString();
        if(phase=="ready") {m_openReady=true;m_phase=tr("Downloads ready.");}
        else if(phase=="failed") {const auto e=result.value("error").toMap();m_error=replacementActive()?replacementMessage(e.value("kind").toString(),e.value("message").toString()):e.value("kind").toString()+": "+e.value("message").toString();m_phase=tr("Downloads unavailable.");}
        else m_phase=tr("Opening downloads and checking retained work…");
        break;
    }
    case C::RemoteReplaceTarget:
        if(!m_appendParams.isEmpty() && request["requestKey"]==m_appendParams["requestKey"]) {
            if(result.value("phase")=="pending")break;
            const auto expected=m_appendParams;m_appendParams={};
            if(result.value("phase")!="ready" || expected["profileId"]!=QJsonValue(m_model->viewedProfileId())
                || expected["expectedRevision"]!=QJsonValue(m_model->revision())) {
                m_error=tr("The library changed. Choose the files again to continue.");m_downloadQueue.clear();m_visible=true;break;
            }
            m_appendTarget=result.value("target").toMap();startNextFile();break;
        }
        if(request["requestKey"]==m_confirmationParams["requestKey"] && !m_confirmationParams.isEmpty()) {
            if(result.value("phase")!="pending"){m_confirmationParams={};emit replacementTargetEvidence(request,result);}
            break;
        }
        if(request["requestKey"]!=m_replaceParams["requestKey"] || m_replaceParams.isEmpty())break;
        if(result.value("phase")=="pending")break;
        m_replaceParams={};
        if(result.value("phase")=="failed") {
            const auto e=result.value("error").toMap();m_replaceTarget.clear();m_replaceSkip.clear();m_error=replacementMessage(e.value("kind").toString(),e.value("message").toString());m_phase=tr("Download actions are unavailable for this mod.");break;
        }
        if(m_replaceProfile!=m_model->viewedProfileId() || m_replaceRevision!=m_model->revision()
            || (m_replaceAction!="resume" && !m_batchReplacement && m_replaceMod!=m_model->selectedId())) {
            m_replaceTarget.clear();m_replaceSkip.clear();m_error=tr("The selected mod or profile changed. Check again.");break;
        }
        m_replaceTarget=result.value("target").toMap();m_replaceSkip=result.value("skip").toMap();
        if(m_replaceAction=="resume" && m_replaceTarget.value("recordDigest")!=m_job.value("targetDigest")) {
            m_replaceTarget.clear();m_error=tr("This mod changed since the download began. Start a new action.");break;
        }
        if(m_replaceSkip.value("state")=="unavailable")m_phase=tr("Skipped-file choices are unavailable. You can still choose Force update or Reinstall.");
        else m_phase=tr("Checking available files…");
        if(m_replaceAction!="resume" && ((m_replaceAction=="link")!=replacementSubmission().isEmpty())) {
            m_replaceTarget.clear();m_error=tr("Choose Link to GameBanana for an unlinked mod, or Update for a linked mod.");break;
        }
        if(m_replaceAction=="reinstall" && m_replaceTarget.value("receipt").toMap().isEmpty()) {
            m_error=tr("Version unknown, update to the latest file");break;
        }
        if(m_resolveKey.isEmpty() && m_replaceAction=="resume" && m_job.value("purpose")=="link")resolve(m_job.value("submission").toMap());
        if(m_resolveKey.isEmpty() && !replacementSubmission().isEmpty())resolve(replacementSubmission());
        break;
    case C::RemoteReplaceSkip:
        if(!sameCapturedRequest(request,m_skipParams) || result.value("phase")=="pending")break;
        if(result.value("phase")!="ready" && result.value("phase")!="failed")break;
        m_skipParams={};releaseSkipResolution();m_error.clear();
        if(result.value("phase")=="failed") {const auto e=result.value("error").toMap();m_error=replacementMessage(e.value("kind").toString(),e.value("message").toString());}
        else m_phase=tr("Skipped-file choice saved.");
        m_replaceTarget.clear();m_replaceSkip.clear();
        if(m_visible)requestReplacementObservation();else {releaseResolution();m_replaceAction.clear();m_replaceParams={};}
        break;
    case C::RemoteResolve:
        if(request["requestKey"]==QJsonValue(m_resolveKey))m_resolveAdmitted=true;break;
    case C::RemoteResolveStatus:
        if(request["requestKey"]!=QJsonValue(m_resolveKey) || request["page"].toInteger(-1)!=m_filePage)break;
        if(result.value("phase")=="ready") {
            const auto rows=result.value("candidates").toList();
            bool matches=!(result.value("hasMore").toBool() && (m_filePage==15 || rows.isEmpty()));
            // Display text must not become repeated-page or file identity evidence.
            QJsonArray identityRows;
            for(const auto &v:rows) {auto row=QJsonObject::fromVariantMap(v.toMap());row.remove("description");identityRows.append(row);}
            QByteArray pageDigest;
            if(replacementActive()) {
                pageDigest=QCryptographicHash::hash(LaunchData::canonical(QJsonObject{{"candidates",identityRows},{"hasMore",result.value("hasMore").toBool()}}),QCryptographicHash::Sha256);
                if(m_filePageEvidence.contains(m_filePage) && m_filePageEvidence.value(m_filePage)!=pageDigest)matches=false;
            }
            if(!m_selectionToken.isEmpty() && m_selectionToken!=result.value("selectionToken").toString())matches=false;
            auto evidence=m_fileEvidence;QSet<QString> pageIds;
            for(const auto &v:rows) {
                const auto row=v.toMap(),key=row.value("fileKey").toMap();const auto id=key.value("fileId").toString();
                auto identity=QJsonObject::fromVariantMap(row);identity.remove("description");
                const auto digest=QCryptographicHash::hash(LaunchData::canonical(identity),QCryptographicHash::Sha256);
                if(QJsonObject::fromVariantMap(key.value("submission").toMap())!=m_submission || pageIds.contains(id)
                    || (evidence.contains(id) && evidence.value(id)!=qMakePair(m_filePage,digest))) {matches=false;break;}
                pageIds.insert(id);evidence.insert(id,qMakePair(m_filePage,digest));
            }
            if(evidence.size()>128)matches=false;
            if(!matches){m_error=replacementActive()?tr("The file check changed unexpectedly. Check again."):tr("Fresh file evidence does not match this submission.");if(replacementActive())invalidateReplacementResolution();break;}
            m_fileEvidence=evidence;
            if(replacementActive())m_filePageEvidence.insert(m_filePage,pageDigest);
            if(replacementActive() && !m_replaceTarget.value("receipt").toMap().isEmpty()) {
                const auto saved=m_replaceTarget.value("receipt").toMap().value("rootFileId").toString();
                for(const auto &v:rows){const auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId").toString()==saved)
                    m_savedFileObservation=(row.value("list")=="archived" || row.value("archiveFlag")==true)?QStringLiteral("archived"):QStringLiteral("listed");}
                m_seenFilePages.insert(m_filePage);if(!result.value("hasMore").toBool())m_lastFilePage=m_filePage;
                if(m_savedFileObservation.isEmpty() && m_lastFilePage>=0) {bool complete=true;for(int i=0;i<=m_lastFilePage;++i)if(!m_seenFilePages.contains(i))complete=false;
                    if(complete)m_savedFileObservation=QStringLiteral("absent");}
            }
            if(m_collectFiles)m_files.append(rows);else m_files=rows;
            m_selectionToken=result.value("selectionToken").toString();m_fileHasMore=result.value("hasMore").toBool();m_phase=tr("Choose a file to install.");
            if(m_collectFiles) {
                if(m_fileHasMore) {++m_filePage;send(C::RemoteResolveStatus,{{"requestKey",m_resolveKey},{"page",m_filePage}});}
                else {
                    m_collectFiles=false;m_resolveParams={};
                    m_resolutionRevision=m_model->revision();
                    if(!m_continuingJob.isEmpty()) {m_visible=true;m_choosingFiles=true;}
                    else if(!m_downloadQueue.isEmpty()) {
                        bool same=true;
                        for(const auto &v:m_downloadQueue) {
                            const auto old=v.toMap();bool found=false;
                            for(const auto &fresh:m_files) {
                                const auto row=fresh.toMap();
                                if(row.value("fileKey")==old.value("fileKey") && row.value("candidateFingerprint")==old.value("candidateFingerprint")) {found=true;break;}
                            }
                            if(!found){same=false;break;}
                        }
                        if(same)startNextFile();
                        else {m_downloadQueue.clear();m_visible=true;m_choosingFiles=true;m_error=tr("The selected files changed on GameBanana. Choose the files again.");}
                    } else if(m_files.size()==1 && !replacementFileSelection() && m_linkedFileId.isEmpty()) {
                        const auto file=m_files.first().toMap();
                        if(file.value("list")!="archived" && file.value("archiveFlag")==false
                            && (m_linkedFileId.isEmpty() || file.value("fileKey").toMap().value("fileId")==m_linkedFileId)) {
                            pickFile(file.value("fileKey").toMap().value("fileId").toString(),true);
                            startSelectedFiles();
                        } else {m_visible=true;m_choosingFiles=true;}
                    } else {
                        QVariantMap newest;
                        for(const auto &value:m_files) {
                            const auto file=value.toMap();
                            if(!m_linkedFileId.isEmpty() && file.value("fileKey").toMap().value("fileId").toString()!=m_linkedFileId)continue;
                            if(file.value("list")=="archived" || file.value("archiveFlag")!=false)continue;
                            if(newest.isEmpty() || file.value("createdAt").toLongLong()>newest.value("createdAt").toLongLong())newest=file;
                        }
                        if(!newest.isEmpty())pickFile(newest.value("fileKey").toMap().value("fileId").toString(),true);
                        m_visible=true;m_choosingFiles=true;
                    }
                }
            }
        } else if(!m_selectionToken.isEmpty()) {
            m_error=replacementActive()?tr("The file check could not be confirmed. Check again."):tr("Fresh file evidence moved backwards; resolve it again.");
            if(replacementActive())invalidateReplacementResolution();else {m_selectionToken.clear();m_files.clear();}
        } else if(result.value("phase")=="failed") {
            m_collectFiles=false;m_downloadQueue.clear();m_visible=true;
            const auto e=result.value("error").toMap();m_error=replacementActive()?replacementMessage(e.value("kind").toString(),e.value("message").toString()):e.value("kind").toString()+": "+e.value("message").toString();m_phase=tr("File resolution failed.");
            if(replacementActive())invalidateReplacementResolution();
        }
        break;
    case C::RemoteResolveRelease:m_resolveReleases.remove(request["requestKey"].toString());break;
    case C::RemoteStart:case C::RemoteReplaceStart:break;
    case C::RemoteList:case C::RemoteReplaceList: {
        auto rows=result.value("jobs").toList();QSet<QString> ids;bool valid=true;
        const bool replacement=call==C::RemoteReplaceList;
        const auto &other=replacement?m_addJobs:m_replaceJobs;
        for(const auto &v:other)ids.insert(v.toMap().value("jobId").toString());
        for(auto &v:rows) {
            auto row=v.toMap();const auto id=row.value("jobId").toString();
            if(ids.contains(id)){valid=false;break;}
            ids.insert(id);
            QVariantMap prior;
            for(const auto &old:m_jobs)if(old.toMap().value("jobId").toString()==id){prior=old.toMap();break;}
            const auto status=m_statuses.value(id);
            const auto matches=[&](const QVariantMap &old){return old.isEmpty() || (old.value("profileId")==row.value("profileId") && old.value("submission")==row.value("submission")
                && old.value("purpose")==row.value("purpose") && old.value("modId")==row.value("modId") && old.value("targetDigest")==row.value("targetDigest"));};
            if(!matches(prior) || !matches(status)) {valid=false;break;}
            if(!prior.isEmpty() && prior.value("sequence").toString().toULongLong()>row.value("sequence").toString().toULongLong())row=prior;
            if(!status.isEmpty() && status.value("sequence").toString().toULongLong()>row.value("sequence").toString().toULongLong()) {
                for(const auto *key:{"state","sequence","cleanupPending"})row[QLatin1String(key)]=status.value(QLatin1String(key));
            }
            v=row;
        }
        if(ids.size()>8)valid=false;
        if(valid) {
            if(replacement){m_replaceJobs=rows;m_replaceListCurrent=true;}else {m_addJobs=rows;m_addListCurrent=true;}
            m_jobs=m_addJobs;for(const auto &v:m_replaceJobs)m_jobs.append(v);
            for(auto it=m_statuses.begin();it!=m_statuses.end();) {
                if(!ids.contains(it.key()) && it.key()!=m_jobId && it.key()!=m_reconcileJob && !m_pendingStarts.contains(it.key()))it=m_statuses.erase(it);else ++it;
            }
            for(auto it=m_consents.begin();it!=m_consents.end();) {
                if(!ids.contains(it.key()) && it.key()!=m_jobId && !m_pendingStarts.contains(it.key()))it=m_consents.erase(it);else ++it;
            }
            for(auto it=m_startPins.begin();it!=m_startPins.end();) {
                if(!ids.contains(it.key()) && it.key()!=m_jobId && it.key()!=m_reconcileJob && !m_pendingStarts.contains(it.key()))it=m_startPins.erase(it);else ++it;
            }
        } else m_error=tr("Download history contains inconsistent identities.");
        break;
    }
    case C::RemoteStatus:acceptStatus(result);break;
    case C::RemoteReplaceStatus: {
        auto status=result.value("status").toMap();status["purpose"]=result.value("purpose");status["modId"]=result.value("modId");
        status["targetDigest"]=result.value("targetDigest");status["review"]=result.value("review");acceptStatus(status);break;
    }
    case C::RemoteControl: {
        const auto id=request["jobId"].toString();const auto p=m_pendingControls.value(id);
        if(!p.isEmpty() && p["controlSeq"]==request["controlSeq"] && p["action"]==request["action"]) {
            m_pendingControls.remove(id);
            if(result.value("state")=="rejected")m_error=tr("The download control was rejected. Check the job's current state.");
            requestJobStatus(id);
        }
        break;
    }
    case C::RemotePrepare:
        if(request["prepareKey"]==QJsonValue(m_prepareKey) && sameJob(request)){m_prepareAdmitted=true;requestJobStatus(m_jobId);}break;
    case C::RemoteCandidates:
        if(sameJob(request) && request["prepareKey"]==QJsonValue(m_prepareKey) && request["page"].toInteger(-1)==m_candidatePage) {
            const auto rows=result.value("candidates").toList();QSet<QString> pageIds;auto evidence=m_candidateEvidence;
            bool valid=!(result.value("hasMore").toBool() && (m_candidatePage==127 || rows.isEmpty()));
            for(const auto &v:rows) {
                const auto row=v.toMap();const auto id=row.value("candidateId").toString();
                const auto digest=QCryptographicHash::hash(LaunchData::canonical(QJsonObject::fromVariantMap(row)),QCryptographicHash::Sha256);
                if(pageIds.contains(id) || (evidence.contains(id) && evidence.value(id)!=qMakePair(m_candidatePage,digest))){valid=false;break;}
                pageIds.insert(id);evidence.insert(id,qMakePair(m_candidatePage,digest));
            }
            if(!valid || evidence.size()>1024) {m_error=tr("Choice identities exceed the supported bounds or changed between pages.");break;}
            m_candidateEvidence=evidence;m_candidates=rows;m_candidateHasMore=result.value("hasMore").toBool();m_seenCandidates.unite(pageIds);
            for(const auto &v:m_candidates)if(v.toMap().value("available").toBool()) {const auto row=v.toMap();const auto id=row.value("candidateId").toString();
                m_selectable.insert(id);m_candidateNames[id]=row.value("optionName").toString().isEmpty()?row.value("sourceLabel").toString():row.value("optionName").toString();}
            if(m_installStages.value(m_jobId)=="candidates") {
                m_installCandidates.append(rows);
                if(m_candidateHasMore)loadCandidates(m_jobId,m_prepareKey,m_candidatePage+1);
                else {
                    m_candidates=m_installCandidates;
                    m_installStages[m_jobId]="choice";
                    if(m_selectable.size()==1) {
                        setSelectedIds(m_selectable.values());
                        addSelection(m_installMetadata.value(m_jobId));
                    } else if(!m_selectable.isEmpty())m_visible=true;
                    else m_error=tr("No usable variants were found in this archive.");
                }
            }
        }
        break;
    case C::RemoteCandidateMembers:
        if(sameJob(request) && request["prepareKey"]==QJsonValue(m_prepareKey) && request["candidateId"]==QJsonValue(m_memberCandidate) && request["page"].toInteger(-1)==m_memberPage) {
            const auto rows=result.value("members").toList();
            if(result.value("hasMore").toBool() && (m_memberPage==1249 || rows.isEmpty())) {m_error=tr("Member pagination exceeds its supported bounds.");break;}
            m_members=rows;m_memberHasMore=result.value("hasMore").toBool();
        }
        break;
    case C::RemoteDiagnostics:
        if(sameJob(request) && request["prepareKey"]==QJsonValue(m_prepareKey) && request["page"].toInteger(-1)==m_diagnosticPage) {
            const auto rows=result.value("diagnostics").toList();
            if(result.value("hasMore").toBool() && (m_diagnosticPage==1249 || rows.isEmpty())) {m_error=tr("Diagnostic pagination exceeds its supported bounds.");break;}
            m_diagnostics=rows;m_diagnosticHasMore=result.value("hasMore").toBool();
        }
        break;
    case C::RemoteReview:case C::RemoteReplaceReview:if(request["requestKey"]==QJsonValue(m_reviewKey) && sameJob(request)){m_reviewAdmitted=true;requestJobStatus(m_jobId);}break;
    case C::RemoteCommit:case C::RemoteReplaceCommit:if(request["operationId"]==m_replayParams["operationId"])m_replayParams={};break;
    case C::RemotePrepareRelease:
        if(request["jobId"]==QJsonValue(m_releaseJob) && request["prepareKey"]==QJsonValue(m_releaseKey)
            && (result.value("state")=="released" || result.value("state")=="stale")) {
            if(m_releaseJob==m_jobId && m_releaseKey==m_prepareKey) {
                m_prepareKey.clear();m_prepareParams={};m_prepareAdmitted=false;m_candidatesLoaded=false;resetReview();
                m_selectedIds.clear();m_selectable.clear();m_seenCandidates.clear();m_candidateEvidence.clear();m_candidateNames.clear();m_candidates.clear();m_members.clear();m_diagnostics.clear();
                m_phase=tr("Preparation released. You can prepare again when ready.");
            }
            m_releaseJob.clear();m_releaseKey.clear();
        }
        break;
    case C::RemoteClose:
        if(result.value("phase")=="closed") {m_retiring=false;m_openReady=false;m_phase=tr("Downloads stopped safely.");m_poll.stop();}
        else m_phase=tr("Waiting for downloads and recovery to stop safely…");
        break;
    default:break;
    }
    emit changed();
}
void RemoteController::acceptStatus(const QVariantMap &incoming) {
    const auto id=incoming.value("jobId").toString();auto previous=m_statuses.value(id);auto next=incoming;
    if(!previous.isEmpty()) {
        if(incoming.value("profileId")!=previous.value("profileId") || incoming.value("submission")!=previous.value("submission")
            || incoming.value("initialRevision")!=previous.value("initialRevision") || incoming.value("purpose")!=previous.value("purpose")
            || incoming.value("modId")!=previous.value("modId") || incoming.value("targetDigest")!=previous.value("targetDigest")) {m_error=tr("Download identity changed unexpectedly.");return;}
        const auto oldSequence=previous.value("sequence").toString().toULongLong(), sequence=incoming.value("sequence").toString().toULongLong();
        if(sequence<oldSequence)return;
        if(sequence==oldSequence) {
            auto oldDurable=previous,newDurable=incoming;
            for(const auto *key:{"sessionId","progress","review"}) {oldDurable.remove(QLatin1String(key));newDurable.remove(QLatin1String(key));}
            if(oldDurable!=newDurable){m_error=tr("Download status changed without a durable sequence advance.");return;}
        }
        const auto oldProgress=previous.value("progress").toMap(), progress=incoming.value("progress").toMap();
        if(!oldProgress.isEmpty() && !progress.isEmpty()) {
            const bool sameEpoch=oldProgress.value("epoch")==progress.value("epoch");
            if(sameEpoch && progress.value("volatileSequence").toString().toULongLong()<oldProgress.value("volatileSequence").toString().toULongLong())return;
            if(!sameEpoch && sequence==oldSequence)next["progress"]=oldProgress;
        }
    }
    for(const auto &v:m_jobs) {
        const auto row=v.toMap();if(row.value("jobId").toString()!=id)continue;
        if(row.value("profileId")!=incoming.value("profileId") || row.value("submission")!=incoming.value("submission")
            || row.value("purpose")!=incoming.value("purpose") || row.value("modId")!=incoming.value("modId") || row.value("targetDigest")!=incoming.value("targetDigest")) {
            m_error=tr("Download history and status disagree on their identity.");return;
        }
        if(row.value("sequence").toString().toULongLong()>incoming.value("sequence").toString().toULongLong())return;
    }
    const auto pending=m_pendingStarts.value(id);
    if(!pending.isEmpty() && (pending["profileId"]!=QJsonValue(incoming.value("profileId").toString())
        || (pending.contains("targetDigest") && (pending["purpose"]!=QJsonValue(incoming.value("purpose").toString())
            || pending["modId"]!=QJsonValue(incoming.value("modId").toString()) || pending["targetDigest"]!=QJsonValue(incoming.value("targetDigest").toString())
            || pending["expectedRevision"]!=QJsonValue(incoming.value("initialRevision").toString()))))) {m_error=tr("Download admission returned a different target.");return;}
    const auto pin=m_startPins.value(id);
    if(!pin.isEmpty()) {
        const auto members=incoming.value("cohort").toMap().value("members").toList();
        const bool replacement=pin.contains("targetDigest");
        if(pin["profileId"]!=QJsonValue(incoming.value("profileId").toString()) || pin["expectedRevision"]!=QJsonValue(incoming.value("initialRevision").toString())
            || pin["submission"]!=QJsonObject::fromVariantMap(incoming.value("submission").toMap())
            || (replacement && (pin["purpose"]!=QJsonValue(incoming.value("purpose").toString()) || pin["modId"]!=QJsonValue(incoming.value("modId").toString())
                || pin["targetDigest"]!=QJsonValue(incoming.value("targetDigest").toString())))
            || (!replacement && !incoming.value("purpose").toString().isEmpty())
            || (!members.isEmpty() && pin["fileId"]!=QJsonValue(members.first().toMap().value("fileId").toString()))) {
            m_error=replacement?tr("Download admission returned a different target or file."):tr("Download admission returned different file details.");return;
        }
    }
    const bool wasPending=m_pendingStarts.contains(id);
    if(next.value("state")=="failed" && (previous.value("state")!="failed" || next.value("sequence")!=previous.value("sequence")))++m_failureSequences[id];
    m_statuses[id]=next;m_pendingStarts.remove(id);
    if(QStringList{"cancelled","abandoned"}.contains(next.value("state").toString()) && m_installStages.contains(id)) {
        m_installStages.remove(id);clearInstallQueue(id);
    }
    if(wasPending && m_downloadQueue.isEmpty() && !replacementActive() && !m_queueProfile.isEmpty()) {releaseResolution();m_queueProfile.clear();}
    if(id==m_jobId) {
        if(wasPending)m_error.clear();
        m_job=next;
        const auto state=next.value("state").toString();
        m_poll.setInterval(QStringList{"downloaded","preparing","ready","reviewing","committing"}.contains(state)?100:1500);
        if(!m_prepareKey.isEmpty() && m_job.value("prepareKey").toString()==m_prepareKey) {
            const auto state=m_job.value("state").toString();
            if((state=="ready" || state=="reviewing") && !m_candidatesLoaded && !m_shutdown) {m_candidatesLoaded=true;loadCandidates(id,m_prepareKey,0);}
            const auto r=m_job.value("review").toMap();
            if(!m_reviewKey.isEmpty() && r.value("requestKey").toString()==m_reviewKey) {
                QStringList reviewed;for(const auto &id:r.value("summary").toMap().value("selectedCandidateIds").toList())reviewed.append(id.toString());
                bool digestMatches=true;
                if(r.value("phase")=="ready") {
                    QJsonArray input;
                    if(replacementJob()) {
                        input.append(QStringLiteral("remoteReplaceSelection/v1"));input.append(m_job.value("purpose").toString());
                        input.append(m_job.value("profileId").toString());input.append(m_job.value("modId").toString());input.append(m_job.value("targetDigest").toString());
                        input.append(m_job.value("initialRevision").toString());input.append(m_prepareParams["expectedRevision"]);
                        input.append(m_consents.value(id));input.append(m_job.value("cohort").toMap().value("fingerprint").toString());
                        input.append(QJsonArray::fromVariantList(r.value("summary").toMap().value("selectedCandidateIds").toList()));
                        input.append(QJsonArray::fromVariantList(r.value("summary").toMap().value("selectedOptionIds").toList()));input.append(r.value("afterRecordDigest").toString());
                    } else {
                        input.append(QStringLiteral("remoteSelection/v1"));
                        input.append(QJsonArray::fromVariantList(r.value("summary").toMap().value("selectedOptionIds").toList()));
                        input.append(m_reviewParams["metadata"]);
                    }
                    const auto digest=QCryptographicHash::hash(LaunchData::canonical(input),QCryptographicHash::Sha256).toHex();
                    digestMatches=QString::fromLatin1(digest)==r.value("selectionDigest").toString();
                }
                if(r.value("phase")=="ready" && (reviewed!=m_selectedIds || !digestMatches)) {
                    m_review.clear();m_error=tr("Review no longer matches the selected choices.");
                } else {
                    m_review=r;
                    if(r.value("phase")=="failed") {
                        const auto error=r.value("error").toMap();
                        m_error=replacementJob()?replacementMessage(error.value("kind").toString(),error.value("message").toString()):error.value("kind").toString()+": "+error.value("message").toString();
                    }
                }
            } else if(m_reviewAdmitted && !m_reviewKey.isEmpty())m_review.clear();
        }
        const auto e=m_job.value("error").toMap();if(!e.isEmpty())m_error=replacementJob()?replacementMessage(e.value("kind").toString(),e.value("message").toString()):e.value("kind").toString()+": "+e.value("message").toString();
    }
    if(m_deferredActions.contains(id)) {
        const auto action=m_deferredActions.take(id);
        if(action=="install") {if(selectJob(id))continueDownloaded();}
        else if(action=="retry" && next.value("state")!="failed")retryInstall(id);
        else {const auto seq=controlSequence(next);if(!seq.isEmpty())control(id,seq,action=="dismiss" && next.value("state")!="failed"?QStringLiteral("cancel"):action);}
    }
    emit statusEvidence(next);
}
void RemoteController::rejected(C call,const QJsonObject &request,const QString &kind,const QString &message) {
    if(!current(request))return;m_inFlight.remove(requestIdentity(call,request));
    if(call==C::RemoteOpen && m_shutdown && !m_model->remoteMarkerPending())return;
    const bool unconfirmed=kind=="remoteBusy" || kind=="nativeBusy" || kind=="outcomeUnknown" || kind=="launchBusy";
    if(call==C::RemoteReplaceTarget && !m_appendParams.isEmpty() && request["requestKey"]==m_appendParams["requestKey"]) {
        if(!unconfirmed){m_appendParams={};m_downloadQueue.clear();m_error=tr("These files could not be added to the existing mod. Check the library and try again.");m_visible=true;}
        emit changed();return;
    }
    if(call==C::RemoteReplaceTarget && !m_confirmationParams.isEmpty() && request["requestKey"]==m_confirmationParams["requestKey"]) {
        m_confirmationParams={};emit replacementTargetRejected(request,kind,message);emit changed();return;
    }
    if(call==C::RemoteStart || call==C::RemoteReplaceStart) {
        const auto id=request["jobId"].toString();
        const auto pending=m_pendingStarts.value(id);const bool replacement=pending.contains("targetDigest");
        if(!sameCapturedRequest(request,pending) || !m_startPins.contains(id) || replacement!=(call==C::RemoteReplaceStart))return;
        if(definiteStartRejection(kind,replacement)) {
            const bool automatic=m_installStages.remove(id)>0;
            if(automatic)clearInstallQueue(id);
            m_pendingStarts.remove(id);m_startPins.remove(id);m_consents.remove(id);
            if(id==m_jobId){m_job.clear();m_jobId.clear();}
            if(m_selectionToken==request["selectionToken"].toString()) {
                if(!replacement)renewResolution();
                else if(m_replaceProfile==request["profileId"].toString() && m_replaceMod==request["modId"].toString()
                    && m_replaceTarget.value("recordDigest").toString()==request["targetDigest"].toString()) {
                    renewResolution();m_replaceTarget.clear();m_replaceSkip.clear();
                }
            }
            m_error=automatic && !message.isEmpty()?message:replacement?replacementMessage(kind,message):tr("This download could not be started. Resolve its files again.");
            if(automatic)m_noticeError=m_error;
            m_phase=tr("This download could not be started. Check the files again.");
        } else {
            m_error=tr("The download start is not yet confirmed. Checking the same request…");
            if(m_installStages.contains(id))m_noticeError=m_error;
            m_phase=tr("Checking whether the download started…");
        }
        emit changed();return;
    } else if(call==C::RemoteReplaceTarget && !m_replaceParams.isEmpty() && request["requestKey"]==m_replaceParams["requestKey"]) {
        if(!unconfirmed){m_replaceParams={};m_replaceTarget.clear();m_replaceSkip.clear();m_error=kind=="staleObservation"?tr("Download details expired. Check again."):message;}
    } else if(call==C::RemoteReplaceSkip) {
        if(!sameCapturedRequest(request,m_skipParams))return;
        // The owner expires a same-input skip fact only after it is no longer
        // in flight or pending. Its old outcome is unknown: observe again,
        // without calling it saved or failed. Every other RPC error retains
        // the exact request and its owed resolution until a typed result.
        if(kind=="staleObservation") {
            m_skipParams={};releaseSkipResolution();m_replaceTarget.clear();m_replaceSkip.clear();
            m_error=tr("The skipped-file choice could not be confirmed. Check it again.");
            if(m_visible)requestReplacementObservation();else {releaseResolution();m_replaceAction.clear();m_replaceParams={};}
        } else {
            m_error=tr("The skipped-file choice is not yet confirmed. Checking the same request…");
        }
        emit changed();return;
    } else if(call==C::RemoteStatus || call==C::RemoteReplaceStatus) {
        const auto id=request["jobId"].toString();
        if(m_pendingStarts.contains(id)) {
            // The last list no longer justifies another status poll after an
            // inconclusive status. Require a fresh list for this session.
            if(m_pendingStarts.value(id).contains("targetDigest"))m_replaceListCurrent=false;else m_addListCurrent=false;
            m_error=tr("The download start is not yet confirmed. Checking the same request…");
            m_phase=tr("Checking whether the download started…");emit changed();return;
        }
    } else if((call==C::RemoteCommit || call==C::RemoteReplaceCommit) && request["operationId"]==m_replayParams["operationId"]) {
        m_replayParams={};
    } else if((call==C::RemoteReview || call==C::RemoteReplaceReview) && !unconfirmed && request["requestKey"]==QJsonValue(m_reviewKey)) {
        m_reviewParams={};m_reviewAdmitted=false;
    } else if(call==C::RemotePrepare && !unconfirmed && request["prepareKey"]==QJsonValue(m_prepareKey) && sameJob(request)) {
        m_releaseJob=m_jobId;m_releaseKey=m_prepareKey;
        send(C::RemotePrepareRelease,{{"jobId",m_releaseJob},{"prepareKey",m_releaseKey}});
    } else if(call==C::RemoteResolve && !unconfirmed && request["requestKey"]==QJsonValue(m_resolveKey)) {
        m_resolveParams={};m_resolveAdmitted=false;
    } else if(call==C::RemotePrepareRelease && kind=="cancelTooLate" && request["jobId"]==QJsonValue(m_releaseJob) && request["prepareKey"]==QJsonValue(m_releaseKey)) {
        m_releaseKey.clear();m_releaseJob.clear();m_committing=true;
    } else if(call==C::RemoteClose) m_phase=tr("Waiting for downloads and recovery to stop safely…");
    if(!unconfirmed && kind!="jobAbsent" && call!=C::RemoteCommit && call!=C::RemoteReplaceCommit)
        m_error=(int(call)>=int(C::RemoteReplaceTarget) || replacementActive() || replacementJob())?replacementMessage(kind,message)
            :knownError(kind)?kind+": "+message:tr("The helper returned unrecognized error evidence; the action remains unconfirmed.");
    else if(unconfirmed && m_error.isEmpty())m_phase=tr("Waiting for the download owner to confirm this action…");
    if(unconfirmed && (call==C::RemoteCandidates || call==C::RemoteCandidateMembers || call==C::RemoteDiagnostics)) {
        QTimer::singleShot(1500,this,[this,call,request]{
            if(!m_shutdown && current(request) && sameJob(request) && request["prepareKey"]==QJsonValue(m_prepareKey))send(call,request);
        });
    }
    emit changed();
}

QVariantList RemoteController::jobs() const {
    QVariantList rows;
    for(const auto &value:m_jobs) {
        auto row=value.toMap();const auto id=row.value("jobId").toString();
        const auto status=m_statuses.value(id);
        if(!status.isEmpty() && status.value("sequence").toString().toULongLong()>=row.value("sequence").toString().toULongLong())row=status;
        row["name"]=installName(id);row["installError"]=m_installErrors.value(id);row["failureSequence"]=m_failureSequences.value(id);
        row["notice"]=m_installStages.contains(id);row["libraryModId"]=m_installResultMods.value(id);
        row["completedAt"]=m_installCompleted.value(id);
        row["choosing"]=m_installStages.value(id)=="choice" || (!m_installStages.contains(id) && id==m_jobId && !m_candidates.isEmpty() && m_reviewKey.isEmpty());
        const auto state=row.value("state").toString();
        row["canContinue"]=state=="downloaded" && (m_installStages.value(id).isEmpty() || m_installStages.value(id)=="stopped");
        if(QStringList{"installed","cancelled","abandoned"}.contains(state) && !row.value("cleanupPending").toBool())continue;
        // Download history never proves that a mod is still in the library.
        row["added"]=false;
        rows.append(row);
    }
    return rows;
}
QString RemoteController::installName(const QString &jobId) const {
    auto name=m_installMetadata.value(jobId).value("name").toString();
    if(name.isEmpty()) {
        const auto mod=m_statuses.value(jobId).value("modId").toString();
        if(!mod.isEmpty())name=m_model->modDetails(mod).value("title").toString();
    }
    if(name.isEmpty()) {
        const auto members=m_statuses.value(jobId).value("cohort").toMap().value("members").toList();
        if(!members.isEmpty())name=members.first().toMap().value("originalName").toString();
    }
    return name.isEmpty()?tr("Downloaded mod"):name;
}
QStringList RemoteController::selectedFileIds() const {
    QStringList ids;for(const auto &v:m_pickedFiles)ids.append(v.toMap().value("fileKey").toMap().value("fileId").toString());return ids;
}
quint64 RemoteController::selectedFileBytes() const {
    quint64 total=0;for(const auto &v:m_pickedFiles)total+=v.toMap().value("declaredSize").toString().toULongLong();return total;
}
void RemoteController::pickFile(const QString &id,bool selected,bool acceptArchived) {
    if(!m_downloadQueue.isEmpty() || !m_appendParams.isEmpty())return;
    for(qsizetype i=0;i<m_pickedFiles.size();++i)if(m_pickedFiles[i].toMap().value("fileKey").toMap().value("fileId")==id){m_pickedFiles.removeAt(i);break;}
    if(selected)for(const auto &v:m_files) {
        auto row=v.toMap();if(row.value("fileKey").toMap().value("fileId")!=id)continue;
        if((row.value("list")=="archived" || row.value("archiveFlag")!=false) && !acceptArchived)break;
        row["acceptArchived"]=acceptArchived;m_pickedFiles.append(row);break;
    }
    emit changed();
}
void RemoteController::pickAllFiles(bool selected) {
    const auto picked=selectedFileIds();
    for(const auto &v:m_files) {
        const auto id=v.toMap().value("fileKey").toMap().value("fileId").toString();
        if(!selected || !picked.contains(id))pickFile(id,selected);
    }
}
bool RemoteController::startSelectedFiles() {
    if(m_pickedFiles.isEmpty() || !m_downloadQueue.isEmpty() || !m_appendParams.isEmpty() || m_selectionToken.isEmpty())return false;
    for(auto it=m_installStages.cbegin();it!=m_installStages.cend();++it)
        if(it.value()!="done" && it.value()!="stopped" && m_startPins.value(it.key())["submission"].toObject()==m_submission) {
            m_error=tr("This mod is already downloading. Wait for it to finish before adding more files.");emit changed();return false;
        }
    m_queueMod.clear();m_nextReplacementPurpose.clear();
    if(replacementActive()) {
        if(!replacementFileSelection() || !replacementContextCurrent())return false;
        m_queueMod=m_replaceMod;m_nextReplacementPurpose="update";m_appendTarget=m_replaceTarget;
        m_replaceAction.clear();m_replaceParams={};m_replaceTarget.clear();m_replaceSkip.clear();
    }
    m_queueJob.clear();m_downloadQueue=m_pickedFiles;m_pickedFiles.clear();m_queueProfile=m_model->viewedProfileId();m_visible=false;m_choosingFiles=false;
    // Preserve provider order, regardless of the order checkboxes were ticked.
    std::sort(m_downloadQueue.begin(),m_downloadQueue.end(),[this](const QVariant &a,const QVariant &b){
        const auto id=[](const QVariant &v){return v.toMap().value("fileKey").toMap().value("fileId");};
        int ai=0,bi=0;for(int i=0;i<m_files.size();++i){if(id(m_files[i])==id(a))ai=i;if(id(m_files[i])==id(b))bi=i;}return ai<bi;
    });
    startNextFile();emit changed();return true;
}
QVariantMap RemoteController::existingSubmissionMod() const {
    QVariantMap match;
    const auto *rows=static_cast<LibraryRows *>(m_model->sourceModel());
    for(int i=0;i<rows->rowCount();++i) {
        const auto row=rows->details(rows->data(rows->index(i),LibraryRows::ModId).toString());
        if(QJsonObject::fromVariantMap(row.value("remoteSubmission").toMap())!=m_submission)continue;
        if(!match.isEmpty())return {{"ambiguous",true}};
        match=row;
    }
    return match;
}
void RemoteController::startNextFile() {
    if(m_downloadQueue.isEmpty() || !m_appendParams.isEmpty() || m_committing || !m_pendingStarts.isEmpty())return;
    if(m_queueProfile!=m_model->viewedProfileId()) {
        m_downloadQueue.clear();m_error=tr("Select the original profile and choose the files again.");emit changed();return;
    }
    if(m_resolutionRevision!=m_model->revision()) {
        // Every completed file advances the library revision. Refresh the
        // remaining selections so long downloads cannot outlive provider
        // authorization. Only identical file fingerprints may continue.
        releaseResolution();m_files.clear();m_candidates.clear();m_fileEvidence.clear();m_filePageEvidence.clear();
        m_filePage=0;m_fileHasMore=false;m_collectFiles=true;m_selectionToken.clear();
        m_resolveKey=freshId();m_resolveParams={{"requestKey",m_resolveKey},{"submission",m_submission}};m_resolveAdmitted=false;
        m_phase=tr("Checking the selected files…");pulse();emit changed();return;
    }
    const auto old=m_queueMod.isEmpty()?existingSubmissionMod():m_model->modDetails(m_queueMod);
    if(!m_queueMod.isEmpty() && (old.isEmpty() || QJsonObject::fromVariantMap(old.value("remoteSubmission").toMap())!=m_submission)) {
        m_downloadQueue.clear();m_error=tr("The selected mod changed. Choose the files again.");m_visible=true;emit changed();return;
    }
    if(old.value("ambiguous").toBool()) {
        m_downloadQueue.clear();m_error=tr("This page has more than one mod in this profile. Choose a mod in Library to manage its files.");m_visible=true;emit changed();return;
    }
    if(!old.isEmpty() && m_appendTarget.isEmpty()) {
        if(!replacementAvailable()) {m_downloadQueue.clear();m_error=tr("Restart the app to add files to this mod.");emit changed();return;}
        m_appendParams={{"requestKey",freshId()},{"profileId",m_queueProfile},{"modId",old.value("id").toString()},{"expectedRevision",m_model->revision()}};
        send(C::RemoteReplaceTarget,m_appendParams);return;
    }
    const auto row=m_downloadQueue.first().toMap();
    if(start(row.value("fileKey").toMap().value("fileId").toString(),row.value("candidateFingerprint").toString(),row.value("acceptArchived").toBool())) {
        // startNextFile admits one pending start; keep its identity when selection changes.
        m_queueJob=m_pendingStarts.cbegin().key();
        m_downloadQueue.removeFirst();
        // The next file must observe the record after this file was saved.
    } else {m_downloadQueue.clear();m_error=tr("The download could not start. Check the files again.");m_visible=true;}
    emit changed();
}
bool RemoteController::installTargetCurrent() const {
    return automaticReplacementJob() ? m_installTargets.value(m_jobId).value("modId")==m_job.value("modId")
        && m_installTargets.value(m_jobId).value("recordDigest")==m_job.value("targetDigest")
        && m_job.value("profileId")==m_model->viewedProfileId() : replacementContextCurrent()
        && m_replaceTarget.value("recordDigest")==m_job.value("targetDigest");
}
bool RemoteController::retryInstall(const QString &id) {
    if(!selectJob(id))return false;
    if(m_job.value("state")=="failed")return control(id,nextControlSeq(),"retry");
    m_error.clear();m_installErrors.remove(id);
    if(!m_prepareKey.isEmpty() && !m_selectedIds.isEmpty()) {const bool started=addSelection(m_installMetadata.value(id));if(started)emit retryStarted(id);return started;}
    m_installStages[id]="download";emit retryStarted(id);emit changed();return true;
}
bool RemoteController::addSelection(const QVariantMap &form) {
    if(replacementJob() && !automaticReplacementJob())return false;
    if(!reviewSelection(m_jobId,m_selectedIds,form)) {
        stopInstall(tr("Could not review this download. Check the selected variants and name, then try again."));return false;
    }
    m_installErrors.remove(m_jobId);
    m_installMetadata[m_jobId]=form;m_installStages[m_jobId]="review";
    return true;
}
void RemoteController::stopInstall(const QString &message) {
    m_installStages[m_jobId]="stopped";++m_failureSequences[m_jobId];m_installErrors[m_jobId]=message;m_error=message;clearInstallQueue(m_jobId);emit changed();
}
void RemoteController::clearInstallQueue(const QString &jobId) {
    if(jobId.isEmpty() || jobId!=m_queueJob)return;
    m_queueJob.clear();
    m_downloadQueue.clear();m_pickedFiles.clear();m_appendParams={};m_appendTarget.clear();
    m_queueProfile.clear();m_queueMod.clear();m_nextReplacementPurpose.clear();
    m_collectFiles=false;releaseResolution();
}
void RemoteController::advanceInstall() {
    if(m_shutdown || m_committing || m_collectFiles || m_choosingFiles || replacementActive() || (replacementJob() && !automaticReplacementJob()))return;
    if(!m_downloadQueue.isEmpty() && m_appendParams.isEmpty()
        && (m_installStages.value(m_queueJob).isEmpty() || m_installStages.value(m_queueJob)=="done")) {startNextFile();return;}
    if(m_prepareKey.isEmpty() && m_error.isEmpty()) {
        for(auto it=m_installStages.cbegin();it!=m_installStages.cend();++it)
            if(it.key()!=m_jobId && it.value()=="download" && m_statuses.value(it.key()).value("state")=="downloaded") {selectJob(it.key());return;}
    }
    auto stage=m_installStages.value(m_jobId);
    if(stage.isEmpty() || stage=="stopped" || stage=="commit" || stage=="done")return;
    if(!m_error.isEmpty()) {stopInstall(m_error);return;}
    if(stage=="choice" || m_pendingControls.contains(m_jobId))return;
    const auto state=m_job.value("state").toString();
    if(QStringList{"cancelRequested","cancelled","abandoned","failed","needsSelection","paused"}.contains(state)) {
        m_installStages[m_jobId]="stopped";return;
    }
    if(stage=="download" && state=="downloaded") {
        if(!canPrepare()) {
            if(m_job.value("profileId").toString()==m_model->viewedProfileId() && hasConsent() && m_releaseKey.isEmpty() && m_prepareKey.isEmpty())return;
            stopInstall(tr("Select this download's profile and continue manually when the library is ready."));return;
        }
        m_installStages[m_jobId]="candidates";m_installCandidates.clear();
        if(!prepare(m_jobId,true))stopInstall(tr("Could not prepare this download. Continue manually."));
    } else if(stage=="review" && m_review.value("phase")=="ready") {
        m_installStages[m_jobId]="commit";
        if(!commit(m_jobId))stopInstall(tr("The library changed. Review the choices again to continue."));
    }
}
