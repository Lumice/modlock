//! Publication of an owned, reviewed remote payload plan into Store4.
use super::{contract::{self,RemoteCommitInputV1},owner::NoAdmissionProof,preparation::RemotePublicationPlan,recovery::{self,RemoteRecovery}};
use crate::{local_import::{self,CopySet},native::NativeError,package,paths,prepare_control::PrepareControl,store::{self,ExactRemoteOutcome,RootLock,Store}};
use json::{object,JsonValue};
use std::{os::unix::fs::MetadataExt,time::Instant};

fn unknown()->NativeError{NativeError::new("outcomeUnknown","The remote publication outcome is not yet known; retain its operation identity.")}
fn stale()->NativeError{NativeError::new("staleReview","The remote review is no longer current.")}
pub(super) fn authority_digest(j:&JsonValue)->Result<String,NativeError>{let mut v=j.clone();for key in ["owner","cleanup","stageDevice","stageInode","ownerDevice","ownerInode"]{v.remove(key);}for f in v["files"].members_mut(){for key in ["device","inode","complete"]{f.remove(key);}}paths::digest_bytes(store::canonical(&v).as_bytes())}
pub(super) fn wire_success(value:&JsonValue)->Result<(),NativeError>{let envelope=object!{jsonrpc:"2.0",id:i64::MIN,result:value.clone()};if crate::protocol::native_value_bytes(&envelope).is_some_and(|n|n.checked_add(1).is_some_and(|n|n<=crate::protocol::MAX_RESPONSE_BYTES)){Ok(())}else{Err(NativeError::new("nativeResponseLimit","The complete remote result cannot be represented."))}}

#[cfg(test)]
thread_local! {pub(crate) static TEST_PRE_JOURNAL_EIO:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
pub(crate) struct RemotePublisher;
impl RemotePublisher {
    pub(super) fn settle_unadmitted(lock: &RootLock, proof: &NoAdmissionProof) -> Result<JsonValue, NativeError> {
        let input = proof.input();
        let lease = proof.lease();
        lease.validate().map_err(|_| unknown())?;
        if lock.root() != lease.root() { return Err(unknown()); }

        let store = Store::open_writer(lock).map_err(|_| unknown())?;
        match store.exact_any_remote_outcome(input).map_err(|_| unknown())? {
            ExactRemoteOutcome::Succeeded(value) => return Ok(value),
            ExactRemoteOutcome::Failed(error) => return Err(error),
            ExactRemoteOutcome::Absent => {},
        }
        if store.journal(input.operation_id()).map_err(|_| unknown())?.is_some() { return Err(unknown()); }
        crate::apply::ensure_idle(&store).map_err(|_| unknown())?;
        lease.validate().map_err(|_| unknown())?;

        let rejected = stale();
        store.record_failure(lock, input.operation_id(), input.json(), &rejected).map_err(|_| unknown())?;
        lease.validate().map_err(|_| unknown())?;
        match store.exact_any_remote_outcome(input).map_err(|_| unknown())? {
            ExactRemoteOutcome::Failed(error) => Err(error),
            _ => Err(unknown()),
        }
    }
    pub(crate) fn replay(lock:&RootLock,input:&RemoteCommitInputV1)->Result<Option<JsonValue>,NativeError>{
        let store=Store::open_writer(lock)?;let value=store.replay(input.operation_id(),input.json())?;
        if value.is_none(){if let Some(j)=store.journal(input.operation_id())?{if !j.has_key("input")||store::canonical(&j["input"])!=store::canonical(input.json()){return Err(NativeError::new("operationIdConflict","This operation ID belongs to different inputs."));}return Err(NativeError::new("recoveryRequired","The remote operation has an unresolved journal."));}}
        Ok(value)
    }
    pub(crate) fn commit(lock:&RootLock,plan:&RemotePublicationPlan,input:&RemoteCommitInputV1)->Result<JsonValue,NativeError>{
        let mut attempted=false;
        let result=Self::commit_inner(lock,plan,input,&mut attempted);
        let rejected=match result{Ok(value)=>return Ok(value),Err(error) if attempted=>return Err(error),Err(error)=>error};
        // This branch is reached only before the first journal write was even
        // attempted by this frozen owner. Record a definite rejection with the
        // exact input; a failed/uncertain write remains unknown.
        let store=Store::open_writer(lock).map_err(|_|unknown())?;
        match store.exact_remote_outcome(input).map_err(|_|unknown())?{
            ExactRemoteOutcome::Succeeded(value)=>Ok(value),
            ExactRemoteOutcome::Failed(error)=>Err(error),
            ExactRemoteOutcome::Absent=>{
                if store.journal(input.operation_id()).map_err(|_|unknown())?.is_some(){return Err(unknown());}
                let failure=if store::safe_remote_failure(&rejected){rejected}else{recovery::rollback_error()};store.record_failure(lock,input.operation_id(),input.json(),&failure).map_err(|_|unknown())?;
                match store.exact_remote_outcome(input).map_err(|_|unknown())?{ExactRemoteOutcome::Failed(error)=>Err(error),_=>Err(unknown())}
            },
        }
    }
    fn commit_inner(lock:&RootLock,plan:&RemotePublicationPlan,input:&RemoteCommitInputV1,journal_attempted:&mut bool)->Result<JsonValue,NativeError>{
        #[cfg(test)] if TEST_PRE_JOURNAL_EIO.with(|v|v.replace(false)){return Err(std::io::Error::from_raw_os_error(5).into());}
        if !matches!(plan.purpose,super::preparation::RemotePlanPurpose::Add){return Err(stale());}
        let store=Store::open_writer(lock)?;if let Some(value)=store.replay(input.operation_id(),input.json())?{return Ok(value);}
        if store.journal(input.operation_id())?.is_some(){return Err(NativeError::new("recoveryRequired","The remote operation has an unresolved journal."));}
        crate::apply::ensure_idle(&store)?;if lock.root()!=plan.prepared.root()||Instant::now()>=plan.expires_at{return Err(stale());}
        plan.prepared.validate_commit_input(input)?;plan.prepared.validate_live(&PrepareControl::never_cancelled())?;
        if input.params()["profileId"]!=plan.profile_id||input.params()["expectedRevision"]!=plan.expected_revision.to_string()||input.params()["selectionDigest"]!=plan.selection_digest{return Err(stale());}
        let before=store.document()?;if before["revision"]!=plan.expected_revision.to_string(){return Err(NativeError::new("staleRevision","The pinned profile revision changed."));}
        if !before["profiles"][&plan.profile_id].is_object(){return Err(stale());}
        super::preparation::reject_duplicate(&before,&plan.profile_id,plan.prepared.submission())?;
        let layout=package::layout_version(&before)?;let before_version=package::document_version(&before)?;
        let catalog=crate::options::for_record(&plan.record,layout)?;catalog.validate_selection_save()?;
        let packages=catalog.entries.iter().map(|e|e.package.clone().ok_or_else(stale)).collect::<Result<Vec<_>,_>>()?;
        let mut rows=Vec::new();let mut copies=CopySet::new();let mut bytes=0u64;
        for (id,(file,path,fp)) in &plan.copies {
            if id!=&file.id||file.size!=fp.size||file.sha256!=fp.sha256{return Err(stale());}bytes=bytes.checked_add(file.size).filter(|n|*n<=contract::MAX_PAYLOAD_BYTES&&*n<=137589948416).ok_or_else(stale)?;
            let row=object!{id:file.id.clone(),sha256:file.sha256.clone(),size:file.size,sourceName:file.source_name.clone(),role:"payload",device:JsonValue::Null,inode:JsonValue::Null,complete:false};
            paths::verify_absent(&local_import::destination(store.root(),&row)?)?;rows.push(row);
            copies.insert(id.clone(),(file.clone(),path.clone(),"payload".into(),fp.clone()));
        }
        let stage_name=format!(".import-remote-payload-{}",paths::random_id()?);let stage=crate::storage::path(store.root(), &stage_name)?;paths::verify_absent(&stage)?;
        let meta=lock.directory().metadata()?;
        let mut journal=object!{kind:"remotePayloadImport",version:1,action:"addRemotePayloadMod",mode:plan.prepared.mode(),operationId:input.operation_id(),input:input.json().clone(),expectedRevision:plan.expected_revision.to_string(),targetProfileId:plan.profile_id.clone(),beforeDigest:paths::digest_bytes(before.dump().as_bytes())?,afterDigest:paths::digest_bytes(plan.projected_document.dump().as_bytes())?,beforeDocument:before,libraryVersionBefore:before_version,libraryVersionAfter:4,packageVersion:layout,modId:plan.mod_id.clone(),record:plan.record.clone(),stage:stage_name.clone(),owner:object!{kind:"remotePayloadImport",version:1,action:"addRemotePayloadMod",operationId:input.operation_id(),stage:stage_name,nonce:paths::random_id()?,rootDevice:meta.dev().to_string(),rootInode:meta.ino().to_string(),authorityDigest:""},stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,files:JsonValue::from(rows),reusedFiles:json::array![],cleanup:object!{mode:"none",cursor:"0",intent:JsonValue::Null}};
        journal["owner"]["rootFsid"]=paths::persisted_filesystem_id(lock.directory())?.into();
        journal["owner"]["storageFsid"]=paths::persisted_filesystem_id(&std::fs::File::open(crate::storage::root(store.root())?)?)?.into();
        journal["owner"]["authorityDigest"]=authority_digest(&journal)?.into();let admitted=RemoteRecovery::admit(&store,&journal)?;
        if admitted.projected!=plan.projected_document||admitted.files.len()!=copies.len()||admitted.files.iter().any(|(id,file)|copies.get(id).is_none_or(|(f,_,_,_)|f!=file)){return Err(stale());}
        let snapshot=store::snapshot_document(&admitted.projected,Some(&plan.profile_id))?;wire_success(&snapshot)?;
        store::admit_outcome(input.operation_id(),input.json(),&object!{status:"succeeded",result:snapshot.clone()})?;
        store::admit_outcome(input.operation_id(),input.json(),&store::failure_outcome(&recovery::rollback_error()))?;
        let mut maximum=journal.clone();for k in ["stageDevice","stageInode","ownerDevice","ownerInode"]{maximum[k]=u64::MAX.into();}for row in maximum["files"].members_mut(){row["device"]=u64::MAX.into();row["inode"]=u64::MAX.into();row["complete"]=true.into();}
        maximum["cleanup"]=object!{mode:"rollback",cursor:u64::MAX.to_string(),intent:object!{kind:"stageFile",fileId:"f".repeat(32),device:u64::MAX,inode:u64::MAX}};store::ensure_internal(&maximum)?;
        let recovery_input=object!{method:"native.recover",params:object!{operationId:"x".repeat(64),expectedRevision:u64::MAX.to_string()}};
        store::ensure_internal(&object!{kind:"remotePayloadRecovery",version:1,operationId:"x".repeat(64),input:recovery_input.clone(),expectedRevision:u64::MAX.to_string(),targetOperationId:input.operation_id(),targetInputSha256:"f".repeat(64)})?;
        store::admit_outcome(&"x".repeat(64),&recovery_input,&object!{status:"succeeded",result:store::snapshot_document(&admitted.projected,None)?})?;
        store::admit_outcome(&"x".repeat(64),&recovery_input,&object!{status:"succeeded",result:store::snapshot_document(&journal["beforeDocument"],None)?})?;
        wire_success(&store::snapshot_document(&admitted.projected,None)?)?;wire_success(&store::snapshot_document(&journal["beforeDocument"],None)?)?;
        plan.prepared.validate_live(&PrepareControl::never_cancelled())?;if Instant::now()>=plan.expires_at{return Err(stale());}
        *journal_attempted=true;store.write_journal(lock,input.operation_id(),&journal)?;
        let mut verified=recovery::Verified::new();
        let result=local_import::write_immutable_copies_verified(&store,lock,plan.expected_revision,input.operation_id(),input.json(),plan.projected_document.clone(),&plan.profile_id,&stage,copies,packages,journal,&mut verified,
            &mut ||plan.prepared.validate_live(&PrepareControl::never_cancelled()),&mut |_|Ok(()));
        match store.exact_remote_outcome(input)?{
            ExactRemoteOutcome::Succeeded(value)=>{if let Ok(Some(j))=store.journal(input.operation_id()){let _=recovery::cleanup_committed_verified(&store,lock,&j,&mut verified);}Ok(value)},
            ExactRemoteOutcome::Absent=>{let _=result;Err(unknown())},
            ExactRemoteOutcome::Failed(error)=>Err(error),
        }
    }
}
