//! Exact replacement journal admission. Cleanup is shared with the remote owner.
use super::{contract::RemoteCommitInput, recovery::{self,AdmittedRemoteJournal},
    replacement_contract::{self,ReplacementCommitInputV1,ReplacementPurpose}, replacement_target};
use crate::{catalog::contract as common, native::NativeError, package::{self,FileRef}, paths,
    store::{self,RootLock,Store}};
use json::{object,JsonValue};
use std::collections::{BTreeMap,HashSet};

fn invalid()->NativeError{NativeError::new("recoveryRequired","Replacement ownership or before and after evidence is incomplete; preserve it for recovery.")}
fn keys(v:&JsonValue,names:&[&str])->Result<(),NativeError>{common::exact(v,names).map_err(|_|invalid())}

pub(super) fn admit(store:&Store,j:&JsonValue)->Result<AdmittedRemoteJournal,NativeError>{
    admit_with_lock(store,j,None)
}
pub(super) fn admit_with_lock(store:&Store,j:&JsonValue,lock:Option<&RootLock>)->Result<AdmittedRemoteJournal,NativeError>{
    keys(j,&["kind","version","action","mode","operationId","input","expectedRevision","targetProfileId","beforeDigest","afterDigest","beforeDocument","libraryVersionBefore","libraryVersionAfter","packageVersion","modId","beforeRecord","record","candidateIds","optionIds","stage","owner","stageDevice","stageInode","ownerDevice","ownerInode","files","reusedFiles","cleanup"])?;
    let linking=j["kind"]=="remotePayloadLink";
    if (!linking && j["kind"]!="remotePayloadReplace")||j["version"]!=1||j["action"]!=if linking {"linkRemotePayloadMod"} else {"replaceRemotePayloadMod"}
        ||!matches!(j["mode"].as_str(),Some("rawVpk"|"archiveVpk"))||(!linking && j["libraryVersionBefore"]!=4)||j["libraryVersionAfter"]!=4
        ||!j["reusedFiles"].is_array()||!j["reusedFiles"].is_empty(){return Err(invalid());}
    let input=ReplacementCommitInputV1::parse(j["input"].clone()).map_err(|_|invalid())?;let p=input.params();
    let profile=p["profileId"].as_str().ok_or_else(invalid)?;let mod_id=p["modId"].as_str().ok_or_else(invalid)?;
    if j["operationId"]!=input.operation_id()||j["targetProfileId"]!=profile||j["modId"]!=mod_id
        ||j["expectedRevision"]!=p["expectedRevision"]{return Err(invalid());}
    let expected=common::u64_text(&p["expectedRevision"]).map_err(|_|invalid())?;
    let purpose=ReplacementPurpose::parse(&p["purpose"]).map_err(|_|invalid())?;
    let appending=purpose==ReplacementPurpose::Append;
    if linking!=(purpose==ReplacementPurpose::Link){return Err(invalid());}
    let before=&j["beforeDocument"];let layout=package::layout_version(before).map_err(|_|invalid())?;
    if j["libraryVersionBefore"].as_u32()!=Some(package::document_version(before).map_err(|_|invalid())?)||j["packageVersion"].as_u32()!=Some(layout)
        ||before["revision"]!=expected.to_string()||j["beforeDigest"]!=paths::digest_bytes(before.dump().as_bytes())?{return Err(invalid());}
    crate::deployment_metadata::admit(store.root(),before).map_err(|_|invalid())?;store::ensure_readable(before).map_err(|_|invalid())?;
    let rows=&before["profiles"][profile]["mods"];if !rows.is_array(){return Err(invalid());}
    let mut matching=rows.members().enumerate().filter(|(_,r)|r["source"]["id"]==mod_id);
    let (index,old)=matching.next().ok_or_else(invalid)?;if matching.next().is_some()||*old!=j["beforeRecord"]{return Err(invalid());}
    let target=replacement_target::admit(old.clone(),layout,profile,mod_id,expected).map_err(|_|invalid())?;
    if p["targetDigest"]!=target.digest{return Err(invalid());}
    let record=&j["record"];let next=replacement_target::admit(record.clone(),layout,profile,mod_id,expected).map_err(|_|invalid())?;
    if (if linking {target.submission.is_some() || next.submission.is_none()} else {next.submission!=target.submission})||p["afterRecordDigest"]!=next.digest
        || (if appending {replacement_target::receipt(record)!=replacement_target::receipt(old)}
            else {replacement_target::receipt(record)["cohortFingerprint"]!=p["cohortFingerprint"]}){return Err(invalid());}
    if ReplacementPurpose::parse(&p["purpose"]).map_err(|_|invalid())?==ReplacementPurpose::Reinstall
        &&replacement_target::receipt(record)["rootFileId"]!=target.root_file_id{return Err(invalid());}
    let mut expected_record=replacement_target::replace_record(old,record["options"].clone(),replacement_target::receipt(record).clone(),purpose);
    if !linking && record.has_key("updatedAt") {
        if !replacement_target::valid_library_date(&record["updatedAt"]) { return Err(invalid()); }
        expected_record["updatedAt"] = record["updatedAt"].clone();
    }
    if expected_record!=*record{return Err(invalid());}
    let choices=crate::options::for_record(record,layout).map_err(|_|invalid())?;
    if appending {
        let old_choices=crate::options::for_record(old,layout).map_err(|_|invalid())?;
        // A journal may own only the newly downloaded payloads. Every old
        // option, including its payload identity, must survive byte-for-byte.
        let mut fresh=choices.clone();
        if fresh.entries.len()<=old_choices.entries.len()
            ||fresh.entries[..old_choices.entries.len()]!=old_choices.entries {return Err(invalid());}
        fresh.entries.drain(..old_choices.entries.len());
        fresh.selected_ids=j["optionIds"].members().map(|v|v.as_str().map(str::to_owned).ok_or_else(invalid)).collect::<Result<_,_>>()?;
        fresh.validate_selection_save().map_err(|_|invalid())?;
        if replacement_target::append_options(old,&fresh,layout).map_err(|_|invalid())?!=record["options"] {return Err(invalid());}
    }
    if (!appending && JsonValue::from(choices.selected_ids.clone())!=j["optionIds"])
        ||replacement_contract::selection_digest(&input,&j["candidateIds"],&j["optionIds"]).map_err(|_|invalid())?!=p["selectionDigest"]{return Err(invalid());}
    let mut files=BTreeMap::<String,FileRef>::new();
    let mut retained=BTreeMap::new();
    for (f,role) in crate::local_import::record_files(old,layout).map_err(|_|invalid())? {
        if retained.insert(f.id.clone(),(f.clone(),role)).is_some_and(|old|old!=(f,role)){return Err(invalid());}
    }
    for (f,role) in crate::local_import::record_files(record,layout).map_err(|_|invalid())?{
        // Artwork remains on the old record and is never owned, copied or
        // removed by this payload replacement transaction.
        if role=="artwork" {
            if retained.get(&f.id)!=Some(&(f,role)){return Err(invalid());}
            continue;
        }
        if role!="payload"||files.insert(f.id.clone(),f.clone()).is_some_and(|old|old!=f){return Err(invalid());}
    }
    if appending {
        for (id,(f,role)) in retained {
            if role=="artwork" {continue;}
            if role!="payload" || files.remove(&id)!=Some(f) {return Err(invalid());}
        }
    }
    let ids=files.keys().cloned().collect::<HashSet<_>>();
    replacement_target::reject_payload_collisions(store.root(),before,layout,&ids).map_err(|_|invalid())?;
    recovery::admit_ownership(store,j,&files,lock)?;
    let mut projected=before.clone();replacement_target::transition(&mut projected,layout).map_err(|_|invalid())?;projected["profiles"][profile]["mods"][index]=record.clone();
    projected["revision"]=expected.checked_add(1).ok_or_else(invalid)?.to_string().into();
    if j["afterDigest"]!=paths::digest_bytes(projected.dump().as_bytes())?{return Err(invalid());}
    store::ensure_readable(&projected).map_err(|_|invalid())?;store::ensure_internal(j)?;
    store::admit_outcome(input.operation_id(),input.json(),&object!{status:"succeeded",result:store::snapshot_document(&projected,Some(profile))?})?;
    Ok(AdmittedRemoteJournal{input:RemoteCommitInput::Replace(input),projected,files})
}
