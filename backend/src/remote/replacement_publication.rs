//! Distinct replacement commit authority over the shared immutable byte writer.
use super::{contract::RemoteCommitInput, preparation::{RemotePlanPurpose,RemotePublicationPlan},
    publication::{authority_digest,wire_success}, recovery, replacement_contract::{ReplacementCommitInputV1,ReplacementPurpose},
    replacement_skips,replacement_target};
use crate::{local_import::{self,CopySet}, native::NativeError, options, package, paths,
    prepare_control::PrepareControl, store::{self,ExactRemoteOutcome,RootLock,Store}};
use json::{object,JsonValue};
use std::{collections::HashSet,os::unix::fs::MetadataExt,time::Instant};

fn unknown()->NativeError{NativeError::new("outcomeUnknown","The replacement outcome is not yet known; retain its operation identity.")}
fn stale()->NativeError{NativeError::new("staleReview","The replacement review is no longer current.")}

pub(crate) struct ReplacementPublisher;
impl ReplacementPublisher {
    pub(crate) fn commit(lock:&RootLock,plan:&RemotePublicationPlan,input:&ReplacementCommitInputV1)->Result<JsonValue,NativeError>{
        let mut attempted=false;let result=Self::commit_inner(lock,plan,input,&mut attempted);
        let rejected=match result{Ok(value)=>return Ok(value),Err(error) if attempted=>return Err(error),Err(error)=>error};
        let store=Store::open_writer(lock).map_err(|_|unknown())?;let typed=RemoteCommitInput::Replace(input.clone());
        match store.exact_any_remote_outcome(&typed).map_err(|_|unknown())?{
            ExactRemoteOutcome::Succeeded(value)=>Ok(value),ExactRemoteOutcome::Failed(error)=>Err(error),
            ExactRemoteOutcome::Absent=>{
                if store.journal(input.operation_id()).map_err(|_|unknown())?.is_some(){return Err(unknown());}
                let failure=if store::safe_remote_failure(&rejected){rejected}else{recovery::rollback_error()};
                store.record_failure(lock,input.operation_id(),input.json(),&failure).map_err(|_|unknown())?;
                match store.exact_any_remote_outcome(&typed).map_err(|_|unknown())?{ExactRemoteOutcome::Failed(error)=>Err(error),_=>Err(unknown())}
            },
        }
    }
    fn commit_inner(lock:&RootLock,plan:&RemotePublicationPlan,input:&ReplacementCommitInputV1,attempted:&mut bool)->Result<JsonValue,NativeError>{
        let typed=RemoteCommitInput::Replace(input.clone());let store=Store::open_writer(lock)?;
        match store.exact_any_remote_outcome(&typed)?{ExactRemoteOutcome::Succeeded(value)=>return Ok(value),ExactRemoteOutcome::Failed(error)=>return Err(error),ExactRemoteOutcome::Absent=>{}}
        if store.journal(input.operation_id())?.is_some(){return Err(NativeError::new("recoveryRequired","The replacement has an unresolved journal."));}
        crate::apply::ensure_idle(&store)?;recovery::require_clear_staging(store.root())?;
        if lock.root()!=plan.prepared.root()||Instant::now()>=plan.expires_at{return Err(stale());}
        plan.validate_commit(&typed)?;plan.prepared.validate_live(&PrepareControl::never_cancelled())?;
        let (pin,after,candidate_ids,option_ids)=match &plan.purpose{
            RemotePlanPurpose::Replace{pin,after_record_digest,candidate_ids,option_ids}=>(pin,after_record_digest,candidate_ids,option_ids),_=>return Err(stale()),
        };
        let before=store.document()?;if before["revision"]!=plan.expected_revision.to_string(){return Err(NativeError::new("staleRevision","The pinned profile revision changed."));}
        let target=replacement_target::read(store.root(),&plan.profile_id,&pin.mod_id,plan.expected_revision)?;
        if target.digest!=pin.target_digest||crate::local_content::option_sources::record_digest(&plan.record)?!=*after{return Err(stale());}
        if pin.purpose==ReplacementPurpose::Update{
            let receipt=replacement_target::receipt(&plan.record);
            replacement_skips::check_update(&plan.prepared.authority().store_lease,lock,&plan.profile_id,&pin.mod_id,&pin.target_digest,
                receipt["rootFileId"].as_str().ok_or_else(stale)?,input.params()["consentFingerprint"].as_str().ok_or_else(stale)?)?;
        }
        let layout=package::layout_version(&before)?;if package::document_version(&before)?!=4 && pin.purpose!=ReplacementPurpose::Link{return Err(stale());}
        let catalog=options::for_record(&plan.record,layout)?;catalog.validate_selection_save()?;
        if pin.purpose==ReplacementPurpose::Append {replacement_target::verify_retained(store.root(),&target.record,layout)?;}
        let packages=catalog.entries.iter().filter(|e|e.package.as_ref().is_some_and(|p|plan.copies.contains_key(&p.directory.id)))
            .map(|e|e.package.clone().ok_or_else(stale)).collect::<Result<Vec<_>,_>>()?;
        let ids=plan.copies.keys().cloned().collect::<HashSet<_>>();replacement_target::reject_payload_collisions(store.root(),&before,layout,&ids)?;
        let mut rows=Vec::new();let mut copies=CopySet::new();let mut bytes=0u64;
        for (id,(file,path,fp)) in &plan.copies{
            if id!=&file.id||file.size!=fp.size||file.sha256!=fp.sha256{return Err(stale());}
            bytes=bytes.checked_add(file.size).filter(|n|*n<=super::contract::MAX_PAYLOAD_BYTES&&*n<=137589948416).ok_or_else(stale)?;
            let row=object!{id:file.id.clone(),sha256:file.sha256.clone(),size:file.size,sourceName:file.source_name.clone(),role:"payload",device:JsonValue::Null,inode:JsonValue::Null,complete:false};
            paths::verify_absent(&local_import::destination(store.root(),&row)?)?;rows.push(row);
            copies.insert(id.clone(),(file.clone(),path.clone(),"payload".into(),fp.clone()));
        }
        let stage_name=format!(".import-remote-replacement-{}",paths::random_id()?);let stage=crate::storage::path(store.root(), &stage_name)?;paths::verify_absent(&stage)?;
        let meta=lock.directory().metadata()?;
        let library_before=package::document_version(&before)?;
        let linking=pin.purpose==ReplacementPurpose::Link;
        let kind=if linking {"remotePayloadLink"} else {"remotePayloadReplace"};
        let action=if linking {"linkRemotePayloadMod"} else {"replaceRemotePayloadMod"};
        let mut journal=object!{kind:kind,version:1,action:action,mode:plan.prepared.mode(),operationId:input.operation_id(),input:input.json().clone(),expectedRevision:plan.expected_revision.to_string(),targetProfileId:plan.profile_id.clone(),beforeDigest:paths::digest_bytes(before.dump().as_bytes())?,afterDigest:paths::digest_bytes(plan.projected_document.dump().as_bytes())?,beforeDocument:before,libraryVersionBefore:library_before,libraryVersionAfter:4,packageVersion:layout,modId:pin.mod_id.clone(),beforeRecord:target.record,record:plan.record.clone(),candidateIds:JsonValue::from(candidate_ids.clone()),optionIds:JsonValue::from(option_ids.clone()),stage:stage_name.clone(),owner:object!{kind:kind,version:1,action:action,operationId:input.operation_id(),stage:stage_name,nonce:paths::random_id()?,rootDevice:meta.dev().to_string(),rootInode:meta.ino().to_string(),authorityDigest:""},stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,files:JsonValue::from(rows),reusedFiles:json::array![],cleanup:object!{mode:"none",cursor:"0",intent:JsonValue::Null}};
        journal["owner"]["rootFsid"]=paths::persisted_filesystem_id(lock.directory())?.into();
        journal["owner"]["storageFsid"]=paths::persisted_filesystem_id(&std::fs::File::open(crate::storage::root(store.root())?)?)?.into();
        journal["owner"]["authorityDigest"]=authority_digest(&journal)?.into();let admitted=recovery::admit_any(&store,&journal)?;
        if admitted.projected!=plan.projected_document||admitted.files.len()!=copies.len()||admitted.files.iter().any(|(id,file)|copies.get(id).is_none_or(|(f,_,_,_)|f!=file)){return Err(stale());}
        let snapshot=store::snapshot_document(&admitted.projected,Some(&plan.profile_id))?;wire_success(&snapshot)?;
        store::admit_outcome(input.operation_id(),input.json(),&object!{status:"succeeded",result:snapshot})?;
        store::admit_outcome(input.operation_id(),input.json(),&store::failure_outcome(&recovery::rollback_error()))?;
        let mut maximum=journal.clone();for k in ["stageDevice","stageInode","ownerDevice","ownerInode"]{maximum[k]=u64::MAX.into();}
        for row in maximum["files"].members_mut(){row["device"]=u64::MAX.into();row["inode"]=u64::MAX.into();row["complete"]=true.into();}
        maximum["cleanup"]=object!{mode:"rollback",cursor:u64::MAX.to_string(),intent:object!{kind:"stageFile",fileId:"f".repeat(32),device:u64::MAX,inode:u64::MAX}};store::ensure_internal(&maximum)?;
        let recovery_input=object!{method:"native.recover",params:object!{operationId:"x".repeat(64),expectedRevision:u64::MAX.to_string()}};
        store::ensure_internal(&object!{kind:"remotePayloadReplaceRecovery",version:1,operationId:"x".repeat(64),input:recovery_input.clone(),expectedRevision:u64::MAX.to_string(),targetOperationId:input.operation_id(),targetInputSha256:"f".repeat(64)})?;
        for document in [&admitted.projected,&journal["beforeDocument"]]{let result=store::snapshot_document(document,None)?;wire_success(&result)?;store::admit_outcome(&"x".repeat(64),&recovery_input,&object!{status:"succeeded",result:result})?;}
        plan.prepared.validate_live(&PrepareControl::never_cancelled())?;if Instant::now()>=plan.expires_at{return Err(stale());}
        *attempted=true;store.write_journal(lock,input.operation_id(),&journal)?;
        let journal_before=journal["beforeRecord"].clone();
        let mut verified=recovery::Verified::new();
        let result=local_import::write_immutable_copies_verified(&store,lock,plan.expected_revision,input.operation_id(),input.json(),plan.projected_document.clone(),&plan.profile_id,&stage,copies,packages,journal,&mut verified,
            &mut || {
                plan.prepared.validate_live(&PrepareControl::never_cancelled())?;
                if pin.purpose==ReplacementPurpose::Append {replacement_target::verify_retained(store.root(),&journal_before,layout)?;}
                Ok(())
            },&mut |_|Ok(()));
        match store.exact_any_remote_outcome(&typed)?{
            ExactRemoteOutcome::Succeeded(value)=>{if let Ok(Some(j))=store.journal(input.operation_id()){let _=recovery::cleanup_committed_verified(&store,lock,&j,&mut verified);}Ok(value)},
            ExactRemoteOutcome::Absent=>{let _=result;Err(unknown())},ExactRemoteOutcome::Failed(error)=>Err(error),
        }
    }
}
