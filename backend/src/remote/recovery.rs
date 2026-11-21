//! Remote payload journal admission and identity-specific cleanup.
use super::contract::{self,RemoteCommitInput,RemoteCommitInputV1,RemoteRecoveryInputV1};
use crate::{native::NativeError,package::{self,FileRef},paths,store::{self,ExactRemoteOutcome,RootLock,Store}};
use json::{object,JsonValue};
use std::{collections::{BTreeMap,BTreeSet,HashSet},fs::{self,File,OpenOptions},io::Read,os::{fd::{AsRawFd,FromRawFd},unix::{ffi::OsStrExt,fs::{MetadataExt,OpenOptionsExt}}},path::{Path,PathBuf}};

unsafe extern "C" {fn geteuid()->u32;fn openat(fd:i32,name:*const i8,flags:i32,...)->i32;fn unlinkat(fd:i32,name:*const i8,flags:i32)->i32;}
fn recovery()->NativeError{NativeError::new("recoveryRequired","Remote payload ownership is incomplete or changed; preserve its journal and files.")}
pub(crate) fn rollback_error()->NativeError{NativeError::new("operationInterrupted","Interrupted remote payload publication was rolled back; review and retry with a new operation ID.")}
fn keys(v:&JsonValue,names:&[&str])->Result<(),NativeError>{if !v.is_object()||v.len()!=names.len()||names.iter().any(|k|!v.has_key(k)){Err(recovery())}else{Ok(())}}
fn pair(a:&JsonValue,b:&JsonValue)->Result<Option<(u64,u64)>,NativeError>{match (a.as_u64(),b.as_u64()){(Some(a),Some(b))=>Ok(Some((a,b))),(None,None) if a.is_null()&&b.is_null()=>Ok(None),_=>Err(recovery())}}
fn digest(j:&JsonValue)->Result<String,NativeError>{let mut v=j.clone();for k in ["owner","cleanup","stageDevice","stageInode","ownerDevice","ownerInode"]{v.remove(k);}for f in v["files"].members_mut(){for k in ["device","inode","complete"]{f.remove(k);}}paths::digest_bytes(store::canonical(&v).as_bytes())}
fn stage_name(j:&JsonValue)->Result<&str,NativeError>{let prefix=match j["kind"].as_str(){Some("remotePayloadImport")=>".import-remote-payload-",Some("remotePayloadReplace"|"remotePayloadLink")=>".import-remote-replacement-",_=>return Err(recovery())};j["stage"].as_str().filter(|s|s.strip_prefix(prefix).is_some_and(|x|package::hex(x,32))).ok_or_else(recovery)}
fn id<'a>(v:&'a JsonValue,k:&str)->Result<&'a str,NativeError>{v[k].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)}
fn uint_text(v:&JsonValue)->Result<u64,NativeError>{store::revision(v.as_str().ok_or_else(recovery)?).map_err(|_|recovery())}

pub(crate) struct AdmittedRemoteJournal {pub input:RemoteCommitInput,pub projected:JsonValue,pub files:BTreeMap<String,FileRef>}
pub(crate) struct RemoteRecovery;
impl RemoteRecovery {
    pub(crate) fn admit(store:&Store,j:&JsonValue)->Result<AdmittedRemoteJournal,NativeError>{
        Self::admit_with_lock(store,j,None)
    }
    fn admit_with_lock(store:&Store,j:&JsonValue,lock:Option<&RootLock>)->Result<AdmittedRemoteJournal,NativeError>{
        keys(j,&["kind","version","action","mode","operationId","input","expectedRevision","targetProfileId","beforeDigest","afterDigest","beforeDocument","libraryVersionBefore","libraryVersionAfter","packageVersion","modId","record","stage","owner","stageDevice","stageInode","ownerDevice","ownerInode","files","reusedFiles","cleanup"])?;
        if j["kind"]!="remotePayloadImport"||j["version"]!=1||j["action"]!="addRemotePayloadMod"||!matches!(j["mode"].as_str(),Some("rawVpk"|"archiveVpk"))||j["libraryVersionAfter"]!=4||!j["reusedFiles"].is_array()||!j["reusedFiles"].is_empty(){return Err(recovery());}
        let input=RemoteCommitInputV1::parse(j["input"].clone()).map_err(|_|recovery())?;
        let op=id(j,"operationId")?;let profile=id(j,"targetProfileId")?;let mod_id=id(j,"modId")?;
        if input.operation_id()!=op||input.params()["profileId"]!=profile||input.params()["expectedRevision"]!=j["expectedRevision"]||!mod_id.strip_prefix("remote-").is_some_and(|s|package::hex(s,32)){return Err(recovery());}
        let expected=uint_text(&j["expectedRevision"])?;let before=&j["beforeDocument"];
        let before_version=package::document_version(before)?;let layout=package::layout_version(before)?;
        if j["libraryVersionBefore"].as_u32()!=Some(before_version)||j["packageVersion"].as_u32()!=Some(layout)||before["revision"]!=expected.to_string()
            ||!before["profiles"][profile]["mods"].is_array()||j["beforeDigest"]!=paths::digest_bytes(before.dump().as_bytes())?{return Err(recovery());}
        crate::deployment_metadata::admit(store.root(),before)?;store::ensure_readable(before)?;
        let record=&j["record"];keys(record,&["source","available","previewHidden","enabled","options"])?;
        let source=&record["source"];keys(source,&["id","remoteId","kind","name","author","category","link","description","status","installedVpks","installOrder","createdAt","remoteReceipt"])?;
        if source["id"]!=mod_id||source["kind"]!="remote"||source["link"]!=""||source["status"]!="downloaded"||!source["installedVpks"].is_array()||!source["installedVpks"].is_empty()
            ||record["available"]!=true||record["previewHidden"]!=false||record["enabled"]!=false||source["installOrder"].as_i64().is_none()||source["createdAt"].as_str().is_none()
            ||before["profiles"].entries().any(|(_,p)|p["mods"].members().any(|r|r["source"]["id"]==mod_id)){return Err(recovery());}
        let receipt=&source["remoteReceipt"];keys(receipt,&["version","submission","rootFileId","cohortFingerprint"])?;
        let submission=contract::SubmissionKeyV1::parse(&receipt["submission"]).map_err(|_|recovery())?;
        if receipt["version"]!=1||uint_text(&receipt["rootFileId"])?==0||receipt["cohortFingerprint"]!=input.params()["cohortFingerprint"]
            ||source["remoteId"]!=crate::catalog::provider::SubmissionKeyV1::parse(&submission.json()).map_err(|_|recovery())?.slug(){return Err(recovery());}
        let catalog=crate::options::for_record(record,layout)?;catalog.validate_selection_save()?;
        if catalog.entries.is_empty()||catalog.entries.iter().any(|e|e.package.is_none()||e.expected_family.is_none()||e.archive_binding.is_some()||e.unavailable_reason.is_some())||!catalog.retained_archives.is_empty()||!catalog.attachments.is_empty(){return Err(recovery());}
        let metadata=object!{name:source["name"].clone(),author:source["author"].clone(),category:source["category"].clone(),description:source["description"].clone()};
        if contract::selection_digest(&catalog.selected_ids,&metadata).map_err(|_|recovery())?!=input.params()["selectionDigest"]{return Err(recovery());}
        let mut max_order=None::<i64>;for (i,row) in before["profiles"][profile]["mods"].members().enumerate(){let n=row["source"]["installOrder"].as_i64().unwrap_or(i64::try_from(i).map_err(|_|recovery())?);max_order=Some(max_order.map_or(n,|m|m.max(n)));}
        if source["installOrder"].as_i64()!=max_order.unwrap_or(-1).checked_add(1){return Err(recovery());}
        super::preparation::reject_duplicate(before,profile,&submission)?;
        let mut files=BTreeMap::new();for (f,role) in crate::local_import::record_files(record,layout)?{if role!="payload"{return Err(recovery());}if files.insert(f.id.clone(),f.clone()).is_some_and(|old|old!=f){return Err(recovery());}}
        crate::local_import::reject_existing_claims(before,layout,&files.keys().cloned().collect::<HashSet<_>>(),None)?;
        admit_ownership(store,j,&files,lock)?;
        let mut projected=before.clone();projected["storeVersion"]=4.into();projected["packageVersion"]=layout.into();if before_version<3{projected["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
        projected["profiles"][profile]["mods"].push(record.clone()).map_err(|_|recovery())?;projected["revision"]=expected.checked_add(1).ok_or_else(recovery)?.to_string().into();
        if j["afterDigest"]!=paths::digest_bytes(projected.dump().as_bytes())?{return Err(recovery());}store::ensure_readable(&projected)?;store::ensure_internal(j)?;
        store::admit_outcome(op,input.json(),&object!{status:"succeeded",result:store::snapshot_document(&projected,Some(profile))?})?;
        Ok(AdmittedRemoteJournal{input:RemoteCommitInput::Add(input),projected,files})
    }
}

/// Shared physical staging and cleanup envelope. The purpose-specific decoder
/// has already admitted the complete before/after authority and exact keys.
pub(super) fn admit_ownership(store:&Store,j:&JsonValue,files:&BTreeMap<String,FileRef>,lock:Option<&RootLock>)->Result<(),NativeError>{
    if files.is_empty()||!j["files"].is_array()||j["files"].len()!=files.len(){return Err(recovery());}
    let mut prior=None::<String>;let mut bytes=0u64;
    for row in j["files"].members(){keys(row,&["id","sha256","size","sourceName","role","device","inode","complete"])?;let f=package::decode_file(row)?;
        if prior.as_ref().is_some_and(|p|p>=&f.id)||files.get(&f.id)!=Some(&f)||row["role"]!="payload"||!row["complete"].is_boolean(){return Err(recovery());}
        prior=Some(f.id.clone());bytes=bytes.checked_add(f.size).filter(|n|*n<=contract::MAX_PAYLOAD_BYTES&&*n<=137589948416).ok_or_else(recovery)?;
        if pair(&row["device"],&row["inode"])?.is_none()&&row["complete"]!=false{return Err(recovery());}
        if !row["device"].is_null()&&j["ownerDevice"].is_null(){return Err(recovery());}
    }
    let stage=stage_name(j)?;let owner=&j["owner"];
    let owner_keys=["kind","version","action","operationId","stage","nonce","rootDevice","rootInode","authorityDigest"];
    if owner.has_key("rootFsid") || owner.has_key("storageFsid") {
        let mut with_fsid=owner_keys.to_vec();with_fsid.extend(["rootFsid","storageFsid"]);keys(owner,&with_fsid)?;
        for k in ["rootFsid","storageFsid"] {if !owner[k].as_str().is_some_and(|s|package::hex(s,16)){return Err(recovery());}}
    } else {keys(owner,&owner_keys)?;}
    let root=fs::symlink_metadata(store.root())?;
    let (device,inode)=(uint_text(&owner["rootDevice"])?,uint_text(&owner["rootInode"])?);
    let root_matches=if let Some(lock)=lock {
        store.locked(lock)?;paths::same_persisted_object(store.root(),lock.directory(),store.root(),lock.directory(),device,inode,owner["rootFsid"].as_str())?
    } else if let Some(fsid)=owner["rootFsid"].as_str() {
        paths::reject_symlinks(store.root())?;let held=File::open(store.root())?;let m=held.metadata()?;
        (m.dev(),m.ino())==(root.dev(),root.ino()) && m.ino()==inode && paths::persisted_filesystem_id(&held).is_ok_and(|v|v==fsid)
    } else {(root.dev(),root.ino())==(device,inode)};
    if !root.is_dir()||owner["kind"]!=j["kind"]||owner["version"]!=1||owner["action"]!=j["action"]||owner["operationId"]!=j["operationId"]||owner["stage"]!=stage
        ||!owner["nonce"].as_str().is_some_and(|s|package::hex(s,32))||!root_matches||owner["authorityDigest"]!=digest(j)?{return Err(recovery());}
    if pair(&j["stageDevice"],&j["stageInode"])?.is_none()&&!j["ownerDevice"].is_null(){return Err(recovery());}pair(&j["ownerDevice"],&j["ownerInode"])?;
    let cleanup=&j["cleanup"];keys(cleanup,&["mode","cursor","intent"])?;let mode=cleanup["mode"].as_str().ok_or_else(recovery)?;let cursor=uint_text(&cleanup["cursor"])?;
    let count=match mode{"none"=>0,"committed"=>files.len()+2,"rollback"=>files.len()*2+2,_=>return Err(recovery())};
    if cursor>count as u64||mode=="none"&&(cursor!=0||!cleanup["intent"].is_null()){return Err(recovery());}
    if !cleanup["intent"].is_null(){let intent=&cleanup["intent"];keys(intent,&["kind","fileId","device","inode"])?;let item=queue(j,mode)?.get(cursor as usize).cloned().ok_or_else(recovery)?;if intent_value(&item)?!=*intent{return Err(recovery());}}
    Ok(())
}
pub(super) fn admit_any(store:&Store,j:&JsonValue)->Result<AdmittedRemoteJournal,NativeError>{
    admit_any_with_lock(store,j,None)
}
fn admit_any_with_lock(store:&Store,j:&JsonValue,lock:Option<&RootLock>)->Result<AdmittedRemoteJournal,NativeError>{
    match j["kind"].as_str(){Some("remotePayloadImport")=>RemoteRecovery::admit_with_lock(store,j,lock),Some("remotePayloadReplace"|"remotePayloadLink")=>super::replacement_recovery::admit_with_lock(store,j,lock),_=>Err(recovery())}
}
pub(crate) fn is_main(kind:&JsonValue)->bool{matches!(kind.as_str(),Some("remotePayloadImport"|"remotePayloadReplace"|"remotePayloadLink"))}
pub(crate) fn is_attempt(kind:&JsonValue)->bool{matches!(kind.as_str(),Some("remotePayloadRecovery"|"remotePayloadReplaceRecovery"|"remotePayloadLinkRecovery"))}
pub(crate) fn is_remote_kind(kind:&JsonValue)->bool{is_main(kind)||is_attempt(kind)}
fn attempt_kind(j:&JsonValue)->Result<&'static str,NativeError>{match j["kind"].as_str(){Some("remotePayloadImport")=>Ok("remotePayloadRecovery"),Some("remotePayloadLink")=>Ok("remotePayloadLinkRecovery"),Some("remotePayloadReplace")=>Ok("remotePayloadReplaceRecovery"),_=>Err(recovery())}}
pub(super) fn require_clear_staging(root:&Path)->Result<(),NativeError>{
    for entry in fs::read_dir(crate::storage::root(root)?)?{let name=entry?.file_name();let name=name.as_bytes();
        if name.starts_with(b".import-remote-payload-")||name.starts_with(b".import-remote-replacement-"){return Err(recovery());}}
    Ok(())
}

#[derive(Clone)]
struct Item {kind:&'static str,file_id:Option<String>,device:Option<u64>,inode:Option<u64>}
#[derive(Clone,PartialEq,Eq)]
struct Stamp {dev:u64,ino:u64,size:u64,mtime:i64,mtime_ns:i64,ctime:i64,ctime_ns:i64,mode:u32,uid:u32,links:u64}
impl Stamp {fn new(m:&fs::Metadata)->Self{Self{dev:m.dev(),ino:m.ino(),size:m.len(),mtime:m.mtime(),mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec(),mode:m.mode(),uid:m.uid(),links:m.nlink()}}}
pub(crate) struct Verified(BTreeMap<String,Stamp>,BTreeMap<String,String>);
impl Verified {
    /// Only the live writer may seed this after a content check or its own
    /// chmod/link. Reopened recovery always starts with an empty map.
    pub(crate) fn remember_verified(&mut self,id:&str,file:&File,sha:&str)->Result<(),NativeError>{self.insert(id.to_owned(),Stamp::new(&file.metadata()?));self.1.insert(id.to_owned(),sha.to_owned());Ok(())}
    pub(crate) fn matches_content(&self,id:&str,sha:&str,file:&File)->Result<bool,NativeError>{Ok(self.1.get(id).is_some_and(|s|s==sha)&&self.get(id)==Some(&Stamp::new(&file.metadata()?)))}
    pub(crate) fn new()->Self{Self(BTreeMap::new(),BTreeMap::new())}fn get(&self,id:&str)->Option<&Stamp>{self.0.get(id)}fn insert(&mut self,id:String,stamp:Stamp){self.0.insert(id,stamp);}}
fn locked_identity(lock:&RootLock,path:&Path,file:&File,device:u64,inode:u64,fsid:Option<&str>)->Result<bool,NativeError>{
    lock.validate()?;
    paths::same_persisted_object(lock.root(),lock.directory(),path,file,device,inode,fsid)
}
fn verify_file(lock:&RootLock,path:&Path,row:&JsonValue,fsid:Option<&str>,verified:&mut Verified)->Result<(),NativeError>{
    let mut file=paths::open_regular(path)?;let before=Stamp::new(&file.metadata()?);let size=row["size"].as_u64().ok_or_else(recovery)?;
    let (device,inode)=pair(&row["device"],&row["inode"])?.ok_or_else(recovery)?;
    if !locked_identity(lock,path,&file,device,inode,fsid)?||before.size>size||row["complete"]==true&&before.size!=size{return Err(recovery());}
    let id=row["id"].as_str().ok_or_else(recovery)?;
    if row["complete"]==true&&verified.get(id)!=Some(&before){let fp=paths::fingerprint_file(&mut file)?;let after=Stamp::new(&file.metadata()?);let named=Stamp::new(&paths::open_regular(path)?.metadata()?);
        if before!=after||before!=named||fp.sha256!=row["sha256"]||fp.size!=size{return Err(recovery());}verified.insert(id.to_owned(),after);verified.1.insert(id.to_owned(),row["sha256"].as_str().ok_or_else(recovery)?.to_owned());
    }else if Stamp::new(&paths::open_regular(path)?.metadata()?)!=before{return Err(recovery());}Ok(())
}
fn queue(j:&JsonValue,mode:&str)->Result<Vec<Item>,NativeError>{
    let mut items=Vec::new();
    let mut append=|kind|{for row in j["files"].members(){items.push(Item{kind,file_id:row["id"].as_str().map(str::to_owned),device:row["device"].as_u64(),inode:row["inode"].as_u64()});}};
    if mode=="rollback"{append("destination");}if matches!(mode,"rollback"|"committed"){append("stageFile");
        items.push(Item{kind:"owner",file_id:None,device:j["ownerDevice"].as_u64(),inode:j["ownerInode"].as_u64()});
        items.push(Item{kind:"stageDir",file_id:None,device:j["stageDevice"].as_u64(),inode:j["stageInode"].as_u64()});}
    Ok(items)
}
fn intent_value(item:&Item)->Result<JsonValue,NativeError>{Ok(object!{kind:item.kind,fileId:item.file_id.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null),device:item.device.ok_or_else(recovery)?,inode:item.inode.ok_or_else(recovery)?})}
fn open_directory(path:&Path)->Result<Option<File>,NativeError>{
    match fs::symlink_metadata(path){Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{paths::verify_absent(path)?;Ok(None)},Err(e)=>Err(e.into()),Ok(m)=>{
        if !m.is_dir()||m.file_type().is_symlink(){return Err(recovery());}paths::reject_symlinks(path)?;
        let f=OpenOptions::new().read(true).custom_flags(0x10000|0x20000|0x800).open(path)?;let held=f.metadata()?;if (held.dev(),held.ino())!=(m.dev(),m.ino()){return Err(recovery());}Ok(Some(f))}}
}
fn ordinary(path:&Path)->Result<Option<File>,NativeError>{match fs::symlink_metadata(path){Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{paths::verify_absent(path)?;Ok(None)},Err(e)=>Err(e.into()),Ok(_)=>Ok(Some(paths::open_regular(path)?))}}
fn matches_identity(lock:&RootLock,path:&Path,file:&File,item:&Item,fsid:Option<&str>)->Result<bool,NativeError>{
    match (item.device,item.inode){(Some(device),Some(inode))=>locked_identity(lock,path,file,device,inode,fsid),_=>Ok(false)}
}
fn item_path(root:&Path,stage:&Path,item:&Item)->Result<PathBuf,NativeError>{Ok(match item.kind{
    "destination"=>crate::storage::path(root.as_ref(), "payloads")?.join(format!("{}.vpk",item.file_id.as_deref().ok_or_else(recovery)?)),
    "stageFile"=>stage.join(format!("{}.vpk",item.file_id.as_deref().ok_or_else(recovery)?)),"owner"=>stage.join("owner.json"),"stageDir"=>stage.to_path_buf(),_=>return Err(recovery())})}
fn allowed_missing(item:&Item,index:usize,cursor:usize,intent:&JsonValue,known_unlinked:bool)->bool{
    item.device.is_none()||index<cursor||index==cursor&&!intent.is_null()&&intent_value(item).is_ok_and(|v|v==*intent)||item.kind=="destination"&&known_unlinked
}
fn state(store:&Store,j:&JsonValue,a:&AdmittedRemoteJournal)->Result<&'static str,NativeError>{
    let doc=store.document()?;let outcome=store.exact_any_remote_outcome(&a.input)?;
    let mode=if doc==j["beforeDocument"] {if !matches!(outcome,ExactRemoteOutcome::Absent){return Err(recovery());}"rollback"}
    else if doc==a.projected {let result=match outcome{ExactRemoteOutcome::Succeeded(result)=>result,_=>return Err(recovery())};
        if !store::matches_recorded_snapshot_for(&a.projected,&result,Some(id(j,"targetProfileId")?))?{return Err(recovery());}"committed"}
    else{return Err(recovery());};
    if j["cleanup"]["mode"]!="none"&&j["cleanup"]["mode"]!=mode{return Err(recovery());}Ok(mode)
}
/// Admit the sole remote main and its optional exact recovery attempt before a
/// caller can enter any of the historical journal cleanup paths.
pub(crate) fn admit_set(store:&Store,journals:&JsonValue)->Result<(),NativeError>{
    let lock=RootLock::acquire_existing(store.root()).ok();
    admit_set_with_lock(store,journals,lock.as_ref())
}
fn admit_set_with_lock(store:&Store,journals:&JsonValue,lock:Option<&RootLock>)->Result<(),NativeError>{
    if !journals.is_array(){return Err(recovery());}let mains=journals.members().filter(|e|is_main(&e["body"]["kind"])).collect::<Vec<_>>();
    let attempts=journals.members().filter(|e|is_attempt(&e["body"]["kind"])).collect::<Vec<_>>();
    if mains.len()!=1||attempts.len()>1||journals.len()!=mains.len()+attempts.len(){return Err(recovery());}
    let main=mains[0];let j=&main["body"];if main["id"]!=j["operationId"]{return Err(recovery());}let admitted=admit_any_with_lock(store,j,lock)?;
    if let Some(entry)=attempts.first(){let a=&entry["body"];keys(a,&["kind","version","operationId","input","expectedRevision","targetOperationId","targetInputSha256"])?;
        let input=RemoteRecoveryInputV1::parse(a["input"].clone()).map_err(|_|recovery())?;
        if a["kind"]!=attempt_kind(j)?||a["version"]!=1||a["operationId"]!=input.operation_id()||entry["id"]!=a["operationId"]||a["operationId"]==j["operationId"]
            ||a["expectedRevision"]!=input.expected_revision().to_string()||a["targetOperationId"]!=j["operationId"]||a["targetInputSha256"]!=paths::digest_bytes(store::canonical(admitted.input.json()).as_bytes())?{return Err(recovery());}}
    Ok(())
}

/// Read only the structurally admitted context. This does not verify payloads
/// or acquire the worker's publication/cleanup authority.
pub(crate) fn inspect_context(store:&Store,journals:&JsonValue)->Result<(String,String,String,Option<JsonValue>),NativeError>{
    let lock=RootLock::acquire_existing(store.root()).ok();
    admit_set_with_lock(store,journals,lock.as_ref())?;
    let main=journals.members().find(|entry|is_main(&entry["body"]["kind"])).ok_or_else(recovery)?;
    let admitted=admit_any_with_lock(store,&main["body"],lock.as_ref())?;
    state(store,&main["body"],&admitted)?;
    let document=store.document()?;
    store::ensure_readable(&document)?;
    let revision=uint_text(&document["revision"])?;
    let existing=journals.members().find(|entry|is_attempt(&entry["body"]["kind"]))
        .map(|entry|entry["body"]["input"].clone());
    if let Some(input)=&existing {
        let attempt=RemoteRecoveryInputV1::parse(input.clone()).map_err(|_|recovery())?;
        if attempt.expected_revision()!=revision || store.operation(attempt.operation_id())?["status"]!="recoveryRequired" {return Err(recovery());}
    }
    Ok((revision.to_string(),admitted.input.operation_id().to_owned(),
        paths::digest_bytes(store::canonical(admitted.input.json()).as_bytes())?,existing))
}
fn unchanged_set(store:&Store,j:&JsonValue,a:&AdmittedRemoteJournal)->Result<(),NativeError>{
    let journals=store.journals()?;let mut main=0;let mut attempt=0;
    for e in journals.members(){let body=&e["body"];
        match body["kind"].as_str(){
            Some("remotePayloadImport"|"remotePayloadReplace"|"remotePayloadLink")=>{main+=1;if e["id"]!=j["operationId"]||*body!=*j{return Err(recovery());}},
            Some("remotePayloadRecovery"|"remotePayloadReplaceRecovery"|"remotePayloadLinkRecovery")=>{attempt+=1;keys(body,&["kind","version","operationId","input","expectedRevision","targetOperationId","targetInputSha256"])?;
                let input=RemoteRecoveryInputV1::parse(body["input"].clone()).map_err(|_|recovery())?;
                if body["kind"]!=attempt_kind(j)?||body["version"]!=1||e["id"]!=body["operationId"]||body["operationId"]!=input.operation_id()||body["operationId"]==j["operationId"]
                    ||body["expectedRevision"]!=input.expected_revision().to_string()||body["targetOperationId"]!=j["operationId"]||body["targetInputSha256"]!=paths::digest_bytes(store::canonical(a.input.json()).as_bytes())?{return Err(recovery());}},
            _=>return Err(recovery()),
        }
    }
    if main!=1||attempt>1||journals.len()!=main+attempt{return Err(recovery());}Ok(())
}
/// Validate every claimed remaining object and all remote stage names before
/// changing an intent, unlinking, or recording a recovery operation.
fn preflight(store:&Store,lock:&RootLock,j:&JsonValue,a:&AdmittedRemoteJournal,mode:&str,verified:&mut Verified)->Result<(),NativeError>{
    store.locked(lock)?;
    unchanged_set(store,j,a)?;
    if state(store,j,a)?!=mode{return Err(recovery());}
    let root=store.root();
    let stage=crate::storage::path(root, stage_name(j)?)?;
    let mut found=false;
    for entry in fs::read_dir(crate::storage::root(root)?)? {
        let entry=entry?;
        let name=entry.file_name();
        let name=name.as_bytes();
        if name.starts_with(b".import-remote-payload-")||name.starts_with(b".import-remote-replacement-"){
            if name!=stage_name(j)?.as_bytes()||found{return Err(recovery());}
            found=true;
        }
    }
    let stage_fd=open_directory(&stage)?;
    if found!=stage_fd.is_some(){return Err(recovery());}
    let cursor=if j["cleanup"]["mode"]=="none"{0}else{usize::try_from(uint_text(&j["cleanup"]["cursor"])?).map_err(|_|recovery())?};
    let intent=&j["cleanup"]["intent"];
    let items=queue(j,mode)?;
    let all_complete=j["files"].members().all(|f|f["complete"]==true);
    let storage_fsid=j["owner"]["storageFsid"].as_str();
    if !locked_identity(lock,root,lock.directory(),uint_text(&j["owner"]["rootDevice"])?,uint_text(&j["owner"]["rootInode"])?,j["owner"]["rootFsid"].as_str())?{return Err(recovery());}
    if let Some(dir)=&stage_fd {
        let m=dir.metadata()?;
        let (device,inode)=pair(&j["stageDevice"],&j["stageInode"])?.ok_or_else(recovery)?;
        if m.uid()!=unsafe{geteuid()}||m.mode()&0o077!=0||!locked_identity(lock,&stage,dir,device,inode,storage_fsid)?{return Err(recovery());}
        let mut allowed=BTreeSet::new();
        allowed.insert(b"owner.json".to_vec());
        for f in j["files"].members(){
            allowed.insert(format!("{}.vpk",f["id"].as_str().ok_or_else(recovery)?).into_bytes());
        }
        for entry in fs::read_dir(&stage)?{
            let entry=entry?;
            if !allowed.contains(entry.file_name().as_bytes()){return Err(recovery());}
        }
    }
    for (i,item) in items.iter().enumerate(){
        let path=item_path(root,&stage,item)?;
        if item.kind=="stageDir"{
            if stage_fd.is_some()&&i<cursor||stage_fd.is_none()&&!allowed_missing(item,i,cursor,intent,false){return Err(recovery());}
            continue;
        }
        let file=ordinary(&path)?;
        if let Some(file)=file {
            let m=file.metadata()?;
            if item.device.is_none()||!matches_identity(lock,&path,&file,item,storage_fsid)?||m.uid()!=unsafe{geteuid()}||m.mode()&0o077!=0||i<cursor{return Err(recovery());}
            if item.kind=="owner"{
                if m.nlink()!=1{return Err(recovery());}
                let mut text=String::new();
                file.take(65537).read_to_string(&mut text)?;
                if text!=j["owner"].dump(){return Err(recovery());}
            } else{
                let row=j["files"].members().find(|f|f["id"]==item.file_id.as_deref().unwrap_or("")).ok_or_else(recovery)?;
                verify_file(lock,&path,row,storage_fsid,verified)?;
            }
        // A complete staged cohort may precede its first destination link.
        // Rollback still verifies every staged inode and rejects unknown files.
        }else if !allowed_missing(item,i,cursor,intent,!all_complete || mode=="rollback"){return Err(recovery());}
    }
    // Committed destinations remain library-owned. For every payload check
    // inode equality and link counts against the complete known pair.
    for row in j["files"].members(){
        let file_id=row["id"].as_str().ok_or_else(recovery)?;
        let dest=crate::storage::path(root.as_ref(), "payloads")?.join(format!("{file_id}.vpk"));
        let staged=stage.join(format!("{file_id}.vpk"));
        let d=ordinary(&dest)?;
        let s=ordinary(&staged)?;
        if mode=="committed"&&d.is_none()||!all_complete&&d.is_some(){return Err(recovery());}
        let n=u64::from(d.is_some())+u64::from(s.is_some());
        for (f,path) in [(d.as_ref(),&dest),(s.as_ref(),&staged)]{
            if let Some(f)=f {
                let m=f.metadata()?;let (device,inode)=pair(&row["device"],&row["inode"])?.ok_or_else(recovery)?;
                if !locked_identity(lock,path,f,device,inode,storage_fsid)?||m.nlink()!=n{return Err(recovery());}
            }
        }
        if let Some(d)=d {
            let m=d.metadata()?;
            if m.uid()!=unsafe{geteuid()}||m.mode()&0o077!=0{return Err(recovery());}
            verify_file(lock,&dest,row,storage_fsid,verified)?;
        }
    }
    store.locked(lock)?;
    Ok(())
}
fn unlink_item(store:&Store,lock:&RootLock,j:&JsonValue,item:&Item,verified:&mut Verified)->Result<(),NativeError>{
    let stage=crate::storage::path(store.root(), stage_name(j)?)?;let path=item_path(store.root(),&stage,item)?;
    let storage_fsid=j["owner"]["storageFsid"].as_str();
    let (parent,name,is_dir)=match item.kind{"destination"=>(crate::storage::path(store.root(), "payloads")?,format!("{}.vpk",item.file_id.as_deref().ok_or_else(recovery)?),false),"stageFile"=>(stage.clone(),format!("{}.vpk",item.file_id.as_deref().ok_or_else(recovery)?),false),"owner"=>(stage.clone(),"owner.json".into(),false),"stageDir"=>(crate::storage::root(store.root())?,stage_name(j)?.to_owned(),true),_=>return Err(recovery())};
    let dir=open_directory(&parent)?.ok_or_else(recovery)?;let name=std::ffi::CString::new(name).map_err(|_|recovery())?;
    let fd=unsafe{openat(dir.as_raw_fd(),name.as_ptr(),0x20000|0x800|if is_dir{0x10000}else{0})};
    if fd<0 {if std::io::Error::last_os_error().kind()==std::io::ErrorKind::NotFound{dir.sync_all()?;return Ok(());}return Err(recovery());}
    let held=unsafe{File::from_raw_fd(fd)};let m=held.metadata()?;let named=fs::symlink_metadata(&path)?;let parent_named=fs::symlink_metadata(&parent)?;let parent_held=dir.metadata()?;
    if !matches_identity(lock,&path,&held,item,storage_fsid)?||(named.dev(),named.ino())!=(m.dev(),m.ino())||named.file_type().is_symlink()||m.is_dir()!=is_dir
        ||(parent_named.dev(),parent_named.ino())!=(parent_held.dev(),parent_held.ino()){return Err(recovery());}
    if parent==stage {let (device,inode)=pair(&j["stageDevice"],&j["stageInode"])?.ok_or_else(recovery)?;if !locked_identity(lock,&parent,&dir,device,inode,storage_fsid)?{return Err(recovery());}}
    if is_dir {if fs::read_dir(&path)?.next().is_some(){return Err(recovery());}}
    else if item.kind=="owner"{let mut bytes=String::new();(&held).take(65537).read_to_string(&mut bytes)?;if bytes!=j["owner"].dump(){return Err(recovery());}}
    else{let row=j["files"].members().find(|f|f["id"]==item.file_id.as_deref().unwrap_or("")).ok_or_else(recovery)?;verify_file(lock,&path,row,storage_fsid,verified)?;if row["complete"]==true&&verified.get(item.file_id.as_deref().unwrap_or(""))!=Some(&Stamp::new(&m)){return Err(recovery());}}
    store.locked(lock)?;if unsafe{unlinkat(dir.as_raw_fd(),name.as_ptr(),if is_dir{0x200}else{0})}!=0{return Err(recovery());}dir.sync_all()?;
    // Unlink changes the verified inode's ctime/link count. Keep the held
    // descriptor proof across our own unlink, including for the remaining link.
    if matches!(item.kind,"stageFile"|"destination") {
        let after=held.metadata()?;
        if (after.dev(),after.ino(),after.len(),after.mtime(),after.mtime_nsec())!=(m.dev(),m.ino(),m.len(),m.mtime(),m.mtime_nsec()) || after.nlink()+1!=m.nlink(){return Err(recovery());}
        let id=item.file_id.as_deref().ok_or_else(recovery)?;
        if let Some(sha)=verified.1.get(id).cloned(){verified.remember_verified(id,&held,&sha)?;}
    }
    store.locked(lock)?;Ok(())
}
fn cleanup(store:&Store,lock:&RootLock,mut j:JsonValue,mode:&str,verified:&mut Verified)->Result<(),NativeError>{
    let op=id(&j,"operationId")?.to_owned();
    admit_set_with_lock(store,&store.journals()?,Some(lock))?;
    let a=admit_any_with_lock(store,&j,Some(lock))?;
    preflight(store,lock,&j,&a,mode,verified)?;
    if j["cleanup"]["mode"]=="none"{
        j["cleanup"]["mode"]=mode.into();
        store.write_journal(lock,&op,&j)?;
    }
    loop{
        preflight(store,lock,&j,&a,mode,verified)?;
        let cursor=usize::try_from(uint_text(&j["cleanup"]["cursor"])?).map_err(|_|recovery())?;
        let items=queue(&j,mode)?;
        let Some(item)=items.get(cursor) else{return Ok(());};
        if item.device.is_some()&&j["cleanup"]["intent"].is_null(){
            j["cleanup"]["intent"]=intent_value(item)?;
            store.write_journal(lock,&op,&j)?;
            continue;
        }
        if item.device.is_some(){unlink_item(store,lock,&j,item,verified)?;}
        j["cleanup"]["cursor"]=(cursor+1).to_string().into();
        j["cleanup"]["intent"]=JsonValue::Null;
        store.write_journal(lock,&op,&j)?;
    }
}

impl RemoteRecovery {
    pub(crate) fn recover(lock:&RootLock,attempt:&RemoteRecoveryInputV1)->Result<JsonValue,NativeError>{
        let store=Store::open_writer(lock)?;if let Some(result)=store.replay(attempt.operation_id(),attempt.json())?{return Ok(result);}
        let journals=store.journals()?;admit_set_with_lock(&store,&journals,Some(lock))?;
        let main=journals.members().find(|e|is_main(&e["body"]["kind"])).ok_or_else(recovery)?;let j=main["body"].clone();let a=admit_any_with_lock(&store,&j,Some(lock))?;
        if attempt.operation_id()==a.input.operation_id(){return Err(recovery());}
        if journals.members().any(|e|is_attempt(&e["body"]["kind"])&&e["id"]!=attempt.operation_id()){
            return Err(NativeError::new("recoveryRequired","The existing recovery attempt must be retried with its original operation ID."));
        }
        let existing=store.journal(attempt.operation_id())?;if let Some(existing)=&existing{if existing["kind"]!=attempt_kind(&j)?||store::canonical(&existing["input"])!=store::canonical(attempt.json()){return Err(NativeError::new("operationIdConflict","Recovery operation belongs to different inputs."));}}
        if store::revision(store.document()?["revision"].as_str().ok_or_else(recovery)?)?!=attempt.expected_revision(){return Err(NativeError::new("staleRevision","Library changed before recovery."));}
        let mode=state(&store,&j,&a)?;let mut verified=Verified::new();preflight(&store,lock,&j,&a,mode,&mut verified)?;
        let result=store::snapshot_document(&store.document()?,None)?;store::admit_outcome(attempt.operation_id(),attempt.json(),&object!{status:"succeeded",result:result.clone()})?;
        store::admit_outcome(a.input.operation_id(),a.input.json(),&store::failure_outcome(&rollback_error()))?;
        let envelope=object!{jsonrpc:"2.0",id:i64::MIN,result:result};if !crate::protocol::native_value_bytes(&envelope).is_some_and(|n|n+1<=crate::protocol::MAX_RESPONSE_BYTES){return Err(recovery());}
        if existing.is_none(){let target=object!{kind:attempt_kind(&j)?,version:1,operationId:attempt.operation_id(),input:attempt.json().clone(),expectedRevision:attempt.expected_revision().to_string(),targetOperationId:a.input.operation_id(),targetInputSha256:paths::digest_bytes(store::canonical(a.input.json()).as_bytes())?};
            store::ensure_internal(&target)?;store.write_journal(lock,attempt.operation_id(),&target)?;}
        cleanup(&store,lock,j,mode,&mut verified)?;store.complete_remote_recovery(lock,attempt)
    }
}
/// A committed publisher may retire its own links; failure leaves its durable
/// success and journal intact for explicit recovery.
pub(crate) fn cleanup_committed_verified(store:&Store,lock:&RootLock,j:&JsonValue,verified:&mut Verified)->Result<(),NativeError>{
    cleanup(store,lock,j.clone(),"committed",verified)?;
    let current=store.journal(id(j,"operationId")?)?.ok_or_else(recovery)?;let a=admit_any_with_lock(store,&current,Some(lock))?;preflight(store,lock,&current,&a,"committed",verified)?;
    if current["cleanup"]["cursor"]!=queue(&current,"committed")?.len().to_string()||!current["cleanup"]["intent"].is_null(){return Err(recovery());}
    store.clear_journal(lock,a.input.operation_id())
}
pub(crate) fn cleanup_committed(store:&Store,lock:&RootLock,j:&JsonValue)->Result<(),NativeError>{
    cleanup_committed_verified(store,lock,j,&mut Verified::new())
}
/// Called from the Store transaction immediately before writing both terminal
/// outcomes and removing the two exact journals.
pub(crate) fn terminal(store:&Store,lock:&RootLock,attempt:&RemoteRecoveryInputV1,verified:&mut Verified)->Result<(RemoteCommitInput,&'static str,JsonValue),NativeError>{
    let journals=store.journals()?;admit_set_with_lock(store,&journals,Some(lock))?;
    let main=journals.members().find(|e|is_main(&e["body"]["kind"])).ok_or_else(recovery)?;let recovery_entry=journals.members().find(|e|e["id"]==attempt.operation_id()).ok_or_else(recovery)?;
    if recovery_entry["body"]["kind"]!=attempt_kind(&main["body"])?||store::canonical(&recovery_entry["body"]["input"])!=store::canonical(attempt.json()){return Err(recovery());}
    let j=&main["body"];let a=admit_any_with_lock(store,j,Some(lock))?;let mode=state(store,j,&a)?;preflight(store,lock,j,&a,mode,verified)?;
    if j["cleanup"]["mode"]!=mode||j["cleanup"]["cursor"]!=queue(j,mode)?.len().to_string()||!j["cleanup"]["intent"].is_null()
        ||store::revision(store.document()?["revision"].as_str().ok_or_else(recovery)?)?!=attempt.expected_revision(){return Err(recovery());}
    Ok((a.input,mode,store::snapshot_document(&store.document()?,None)?))
}

/// Owner-held custody survives a worker error or unwind. The worker publishes
/// each acquired lease here before it can reach recovery mutation.
struct RecoveryCustody {
    lease:Option<std::sync::Arc<super::job_store::RemoteStoreLease>>,
    slot:Option<super::preparation_slot::PreparationSlotLease>,
    _borrowed:Option<super::owner::RecoveryBorrow>,
    dispatched:bool,
    rejected_before_dispatch:bool,
    binding_proven:bool,
}

pub(crate) enum RecoveryObservation {
    Running,
    Terminal(JsonValue),
    RejectedBeforeDispatch(NativeError),
    Unknown,
}

/// A terminal result binds both exact operation inputs and the absence of their
/// remote journals. Absence alone is never a failure or retirement proof.
fn exact_terminal(store:&Store,input:&RemoteRecoveryInputV1,target:&RemoteCommitInput,
    main:&JsonValue,admitted:&AdmittedRemoteJournal)->Result<Option<JsonValue>,NativeError>{
    let Some(result)=store.exact_remote_recovery_success(input)? else{return Ok(None);};
    let document=match store.exact_any_remote_outcome(target)?{
        ExactRemoteOutcome::Succeeded(original)=>{
            if !store::matches_recorded_snapshot_for(&admitted.projected,&original,Some(id(main,"targetProfileId")?))?{return Err(recovery());}
            &admitted.projected
        },
        ExactRemoteOutcome::Failed(error)=>{
            let rollback=rollback_error();if error.kind!=rollback.kind||error.message!=rollback.message{return Err(recovery());}
            &main["beforeDocument"]
        },
        ExactRemoteOutcome::Absent=>return Err(recovery()),
    };
    if store.document()?!=*document||!store::matches_recorded_snapshot(document,&result)?{return Err(recovery());}
    let journals=store.journals()?;
    if journals.members().any(|e|e["id"]==input.operation_id()||e["id"]==target.operation_id()
        ||is_remote_kind(&e["body"]["kind"])){return Err(recovery());}
    Ok(Some(result))
}
fn mark_pre_dispatch_rejection(store:&Store,lock:&RootLock,input:&RemoteRecoveryInputV1,error:&NativeError,custody:&std::sync::Arc<std::sync::Mutex<RecoveryCustody>>)->Result<(),NativeError>{
    if custody.lock().unwrap_or_else(|p|p.into_inner()).dispatched {return Err(recovery());}
    if store.operation(input.operation_id())?["status"]!="unknown" || store.journal(input.operation_id())?.is_some(){return Err(recovery());}
    // The worker holds the admitted lease, slot and RootLock. No memory lock
    // crosses the durable operation write or the exact replay verification.
    store.record_failure(lock,input.operation_id(),input.json(),error)?;
    match store.replay(input.operation_id(),input.json()) {
        Err(stored) if stored.kind==error.kind && stored.message==error.message=>{},
        _=>return Err(recovery()),
    }
    custody.lock().unwrap_or_else(|p|p.into_inner()).rejected_before_dispatch=true;
    Ok(())
}

fn run_recovery(root:&Path,input:&RemoteRecoveryInputV1,target:&RemoteCommitInput,initial_journals:&JsonValue,
    custody:&std::sync::Arc<std::sync::Mutex<RecoveryCustody>>)->Result<JsonValue,NativeError>{
    let held_lease={custody.lock().unwrap_or_else(|p|p.into_inner()).lease.clone()};
    let lease=match held_lease {Some(lease)=>lease,None=>{
        let lease=std::sync::Arc::new(super::job_store::RemoteStoreLease::acquire_existing(root)?);
        custody.lock().unwrap_or_else(|p|p.into_inner()).lease=Some(lease.clone());lease
    }};
    let held_slot={custody.lock().unwrap_or_else(|p|p.into_inner()).slot.clone()};
    let slot=match held_slot {Some(slot)=>slot,None=>{
        let slot=super::preparation_slot::PreparationSlot::acquire_existing(root)?;
        custody.lock().unwrap_or_else(|p|p.into_inner()).slot=Some(slot.clone());slot
    }};
    lease.validate()?;slot.validate()?;
    if lease.root()!=root||slot.root()!=root||lease.identity()!=slot.identity(){return Err(recovery());}
    let lock=RootLock::acquire(root)?;
    let root_meta=lock.directory().metadata()?;
    if (root_meta.dev(),root_meta.ino())!=lease.identity(){return Err(recovery());}
    let store=Store::open_writer(&lock)?;
    // Re-admit the captured complete set before using any target binding. This
    // remains possible if a concurrent terminal transaction cleared it after
    // the service's read-only classification.
    admit_set_with_lock(&store,initial_journals,Some(&lock))?;
    let initial_main=initial_journals.members().find(|e|e["id"]==target.operation_id()&&is_main(&e["body"]["kind"])).ok_or_else(recovery)?;
    let initial=admit_any_with_lock(&store,&initial_main["body"],Some(&lock))?;
    if store::canonical(initial.input.json())!=store::canonical(target.json()){return Err(recovery());}
    if let Some(attempt)=initial_journals.members().find(|e|is_attempt(&e["body"]["kind"])){
        if attempt["id"]!=input.operation_id(){let error=recovery();mark_pre_dispatch_rejection(&store,&lock,input,&error,custody)?;return Err(error);}
        if store::canonical(&attempt["body"]["input"])!=store::canonical(input.json()){return Err(recovery());}
        let mut held=custody.lock().unwrap_or_else(|p|p.into_inner());held.dispatched=true;held.binding_proven=true;
    }
    if let Some(result)=exact_terminal(&store,input,target,&initial_main["body"],&initial)?{
        if custody.lock().unwrap_or_else(|p|p.into_inner()).binding_proven{return Ok(result);}
        return Err(NativeError::new("outcomeUnknown","Recovery target binding requires reconciliation."));
    }
    let journals=store.journals()?;
    // A previously recorded recovery attempt is already admitted even if this
    // worker has not yet entered the mutation function.
    if journals.members().any(|e|e["id"]==input.operation_id()){
        custody.lock().unwrap_or_else(|p|p.into_inner()).dispatched=true;
    }
    admit_set_with_lock(&store,&journals,Some(&lock))?;
    let main=journals.members().find(|e|e["id"]==target.operation_id()&&is_main(&e["body"]["kind"])).ok_or_else(recovery)?;
    let admitted=admit_any_with_lock(&store,&main["body"],Some(&lock))?;
    if store::canonical(admitted.input.json())!=store::canonical(target.json()){return Err(recovery());}
    if journals.members().any(|e|is_attempt(&e["body"]["kind"])&&e["id"]!=input.operation_id()){
        let error=recovery();mark_pre_dispatch_rejection(&store,&lock,input,&error,custody)?;return Err(error);
    }
    if let Some(attempt)=journals.members().find(|e|is_attempt(&e["body"]["kind"])){
        if store::canonical(&attempt["body"]["input"])!=store::canonical(input.json()){return Err(recovery());}
        custody.lock().unwrap_or_else(|p|p.into_inner()).binding_proven=true;
    }
    if store::revision(store.document()?["revision"].as_str().ok_or_else(recovery)?)?!=input.expected_revision(){
        let error=NativeError::new("staleRevision","Library changed before recovery.");
        mark_pre_dispatch_rejection(&store,&lock,input,&error,custody)?;
        return Err(error);
    }
    // From this point even an error or unwind is unconfirmed until the exact
    // recovery and target outcomes and journal set establish completion.
    {let mut held=custody.lock().unwrap_or_else(|p|p.into_inner());held.dispatched=true;held.binding_proven=true;}
    drop(store);
    let result=RemoteRecovery::recover(&lock,input);
    let history=Store::open_history(root);
    match history.and_then(|store|exact_terminal(&store,input,target,&initial_main["body"],&initial)){
        Ok(Some(value))=>Ok(value),
        _=>Err(result.err().unwrap_or_else(||NativeError::new("outcomeUnknown","The recovery outcome requires reconciliation."))),
    }
}

/// One root-bound recovery owner. A joined thread is not itself retirement.
pub(crate) struct RecoveryWorker {
    root:PathBuf,
    input:RemoteRecoveryInputV1,
    target:RemoteCommitInput,
    initial_journals:std::sync::Arc<JsonValue>,
    custody:std::sync::Arc<std::sync::Mutex<RecoveryCustody>>,
    handle:Option<std::thread::JoinHandle<Result<JsonValue,NativeError>>>,
    result:Option<Result<JsonValue,NativeError>>,
}
impl RecoveryWorker {
    pub(crate) fn start(root:PathBuf,input:RemoteRecoveryInputV1,target:RemoteCommitInput,initial_journals:JsonValue,
        borrowed:Option<super::owner::RecoveryBorrow>,
        recorded_attempt:bool)->Result<Self,NativeError>{
        let (lease,slot)=borrowed.as_ref().map(|b|(Some(b.lease.clone()),Some(b.slot.clone()))).unwrap_or((None,None));
        let mut owner=Self{root,input,target,initial_journals:std::sync::Arc::new(initial_journals),custody:std::sync::Arc::new(std::sync::Mutex::new(RecoveryCustody{lease,slot,_borrowed:borrowed,dispatched:recorded_attempt,rejected_before_dispatch:false,binding_proven:false})),handle:None,result:None};
        if let Err(error)=owner.spawn(){
            owner.result=Some(Err(error));
        }
        Ok(owner)
    }
    fn spawn(&mut self)->Result<(),NativeError>{
        let root=self.root.clone();let input=self.input.clone();let target=self.target.clone();let initial_journals=self.initial_journals.clone();let custody=self.custody.clone();
        let handle=std::thread::Builder::new().name("remote-recovery".into())
            .spawn(move||run_recovery(&root,&input,&target,&initial_journals,&custody))
            .map_err(|_|NativeError::new("remoteBusy","The recovery worker could not start."))?;
        self.result=None;self.handle=Some(handle);Ok(())
    }
    pub(crate) fn root(&self)->&Path{&self.root}
    pub(crate) fn matches(&self,input:&RemoteRecoveryInputV1)->bool{store::canonical(self.input.json())==store::canonical(input.json())}
    pub(crate) fn operation_id(&self)->&str{self.input.operation_id()}
    fn joined_result(&mut self,block:bool){
        if self.handle.as_ref().is_some_and(|h|block||h.is_finished()){
            let result=self.handle.take().unwrap().join().unwrap_or_else(|_|Err(NativeError::new("outcomeUnknown","The recovery worker ended without an outcome proof.")));
            self.result=Some(result);
        }
    }
    /// Returns terminal/rejection only after actual join and authoritative
    /// proof; unknown keeps the exact input and custody charged.
    pub(crate) fn observe(&mut self)->RecoveryObservation{
        self.joined_result(false);
        if self.handle.is_some(){return RecoveryObservation::Running;}
        match &self.result {
            Some(Ok(value))=>RecoveryObservation::Terminal(value.clone()),
            Some(Err(error)) if self.custody.lock().unwrap_or_else(|p|p.into_inner()).rejected_before_dispatch=>RecoveryObservation::RejectedBeforeDispatch(error.clone()),
            _=>RecoveryObservation::Unknown,
        }
    }
    pub(crate) fn retry_unknown(&mut self){
        if matches!(self.observe(),RecoveryObservation::Unknown){let _=self.spawn();}
    }
    pub(crate) fn retirement_proven(&mut self)->bool{
        matches!(self.observe(),RecoveryObservation::Terminal(_)|RecoveryObservation::RejectedBeforeDispatch(_))
    }
    /// EOF/Drop cannot discard uncertain custody. Continue the same bounded
    /// operation until joined terminal or proved pre-dispatch rejection.
    pub(crate) fn retire(&mut self){
        loop {
            self.joined_result(true);
            if self.retirement_proven(){return;}
            std::thread::sleep(std::time::Duration::from_millis(250));
            self.retry_unknown();
        }
    }
}
impl Drop for RecoveryWorker{fn drop(&mut self){self.retire();}}
