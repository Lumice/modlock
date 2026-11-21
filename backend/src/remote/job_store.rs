//! Root-scoped remote history. The connection never leaves its owning worker.
use super::contract::{self, ConsentFile, ErrorV1, JobId, MemberReceiptV1, RemoteStatusV1, SessionId, SubmissionKeyV1, TransferEpoch};
use super::replacement_contract::{ReplacementCommitInputV1,ReplacementPurpose};
use crate::{catalog::contract as common, native::NativeError, package, paths, protocol, sqlite::Database, store};
use json::{object, JsonValue};
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, File, OpenOptions}, io::Read, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}, sync::Arc};

const DB: &str = "remote-jobs.sqlite3";
const LOCK: &str = ".remote-jobs.lock";
/// Reserved for all remote scratch, registry, owner, and staging artifacts.
pub(crate) const REMOTE_QUARANTINE_PREFIX: &[u8] = b".remote-quarantine-";
const MAX_DB: u64 = 16 * 1024 * 1024;
const MAX_ROW: usize = 128 * 1024;
const MAX_ACTIVE: usize = 8;
const MAX_TOMBSTONES: usize = 32;
const SCHEMA: &str = "CREATE TABLE jobs(id TEXT PRIMARY KEY,input TEXT NOT NULL,body TEXT NOT NULL)";
unsafe extern "C" { fn flock(fd:i32, op:i32)->i32; fn geteuid()->u32; }
fn bad() -> NativeError { NativeError::new("recoveryRequired", "Remote history or ownership evidence is incomplete; preserve it for reconciliation.") }
fn busy() -> NativeError { NativeError::new("remoteBusy", "Another remote owner retains this library.") }
fn full() -> NativeError { NativeError::new("storageFull", "Remote history cannot admit another complete record.") }
fn uncertain() -> NativeError { NativeError::new("outcomeUnknown", "The remote transaction outcome requires reconciliation.") }
fn decimal(v:&JsonValue)->Result<u64,NativeError> { common::u64_text(v).map_err(|_|bad()) }
fn positive(v:&JsonValue)->Result<u64,NativeError> { let n=decimal(v)?; if n==0 {Err(bad())} else {Ok(n)} }
fn token(v:&JsonValue)->Result<(),NativeError> { if v.as_str().is_some_and(|s| package::hex(s,32)) {Ok(())} else {Err(bad())} }
fn hash(v:&JsonValue,n:usize)->Result<(),NativeError> { if v.as_str().is_some_and(|s|package::hex(s,n)) {Ok(())} else {Err(bad())} }
fn maybe_decimal(v:&JsonValue)->Result<Option<u64>,NativeError> { if v.is_null(){Ok(None)}else{decimal(v).map(Some)} }
fn maybe_hash(v:&JsonValue)->Result<(),NativeError> { if v.is_null(){Ok(())}else{hash(v,64)} }
fn exact(v:&JsonValue, keys:&[&str])->Result<(),NativeError> { common::exact(v,keys).map_err(|_|bad()) }
fn read_json(text:&str)->Result<JsonValue,NativeError> {
    if text.len()>MAX_ROW || !protocol::unique_keys(text.as_bytes()) || !protocol::integer_numbers(text.as_bytes()) {return Err(bad());}
    json::parse(text).map_err(|_|bad())
}
fn metadata_id(m:&fs::Metadata)->(u64,u64) {(m.dev(),m.ino())}
fn regular(m:&fs::Metadata)->bool {m.is_file() && m.uid()==unsafe{geteuid()} && m.nlink()==1 && m.mode()&0o077==0}
fn no_sidecars(root:&Path)->Result<(),NativeError> {
    for name in ["remote-jobs.sqlite3-wal","remote-jobs.sqlite3-shm","remote-jobs.sqlite3-journal"] {
        match fs::symlink_metadata(root.join(name)) {Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{}, _=>return Err(bad())}
    }
    for entry in fs::read_dir(root)? {
        let entry=entry?; let name=entry.file_name(); let bytes=name.as_encoded_bytes();
        if bytes.starts_with(b"remote-jobs.sqlite3-") && !matches!(bytes,b"remote-jobs.sqlite3-wal"|b"remote-jobs.sqlite3-shm"|b"remote-jobs.sqlite3-journal") {return Err(bad());}
    }
    Ok(())
}
fn database_exists(root:&Path)->Result<bool,NativeError> {
    match fs::symlink_metadata(root.join(DB)) {Ok(m) if regular(&m)=>Ok(true),Ok(_)=>Err(bad()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(false),Err(_)=>Err(bad())}
}
fn namespace_present(root:&Path,include_claims:bool)->Result<bool,NativeError>{
    let bulk=crate::storage::root(root)?;let root=bulk.as_path();
    for entry in fs::read_dir(root)? {
        let name=entry?.file_name();let bytes=name.as_encoded_bytes();
        if bytes.starts_with(REMOTE_QUARANTINE_PREFIX)||(include_claims&&bytes.starts_with(b".remote-claim-")){return Ok(true);}
    }
    Ok(false)
}
pub(crate) fn quarantine_namespace_present(root:&Path)->Result<bool,NativeError>{namespace_present(root,false)}
pub(crate) fn empty_namespace(root:&Path)->Result<bool,NativeError>{
    no_sidecars(root)?;if database_exists(root)?{return Ok(false);}
    Ok(!namespace_present(root,true)?)
}
fn header(root:&Path)->Result<(),NativeError> {
    let p=root.join(DB);let mut f=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(&p).map_err(|_|bad())?;
    let held=f.metadata().map_err(|_|bad())?;let named=fs::symlink_metadata(&p).map_err(|_|bad())?;
    if !regular(&held)||!regular(&named)||metadata_id(&held)!=metadata_id(&named)||held.len()>MAX_DB||held.len()<100{return Err(bad());}
    let mut h=[0u8;100];f.read_exact(&mut h).map_err(|_|bad())?;
    if &h[..16]!=b"SQLite format 3\0"||h[18]!=1||h[19]!=1{return Err(bad());}
    let p=u16::from_be_bytes([h[16],h[17]]);let size=if p==1{65536}else{p as u64};
    if !(512..=65536).contains(&size)||!size.is_power_of_two()||held.len()%size!=0{return Err(bad());}
    Ok(())
}

/// Only the root lease is shared; SQLite remains worker-owned.
pub(crate) struct RemoteStoreLease { root:PathBuf, root_dir:File, bulk_dir:File, bulk_path:PathBuf, bulk_identity:(u64,u64), lock:File, identity:(u64,u64), lock_identity:(u64,u64) }
impl RemoteStoreLease {
    pub(crate) fn acquire(root:&Path)->Result<Self,NativeError>{
        Self::acquire_mode(root,true)
    }
    pub(crate) fn acquire_existing(root:&Path)->Result<Self,NativeError>{Self::acquire_mode(root,false)}
    fn acquire_mode(root:&Path,create:bool)->Result<Self,NativeError>{
        let root=paths::absolute(root)?;paths::reject_symlinks(&root)?;
        let root_dir=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&root).map_err(|_|bad())?;
        let rm=root_dir.metadata().map_err(|_|bad())?;if !rm.is_dir()||rm.uid()!=unsafe{geteuid()}{return Err(bad());}
        let lock=OpenOptions::new().read(true).write(true).create(create).mode(0o600).custom_flags(0x20000|0x800).open(root.join(LOCK)).map_err(|_|bad())?;
        let lm=lock.metadata().map_err(|_|bad())?;let named=fs::symlink_metadata(root.join(LOCK)).map_err(|_|bad())?;
        if !regular(&lm)||!regular(&named)||metadata_id(&lm)!=metadata_id(&named){return Err(bad());}
        if unsafe{flock(lock.as_raw_fd(),2|4)}!=0{return Err(busy());}
        let bulk_path=crate::storage::root(&root)?; let bulk_dir=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&bulk_path)?;let bulk_identity=metadata_id(&bulk_dir.metadata()?);
        let lease=Self{root,root_dir,bulk_dir,bulk_path,bulk_identity,lock,identity:metadata_id(&rm),lock_identity:metadata_id(&lm)};lease.validate()?;Ok(lease)
    }
    pub(crate) fn validate(&self)->Result<(),NativeError>{
        paths::reject_symlinks(&self.bulk_path)?;
        if metadata_id(&fs::symlink_metadata(&self.bulk_path)?)!=self.bulk_identity || metadata_id(&self.bulk_dir.metadata()?)!=self.bulk_identity {return Err(bad())}
        paths::reject_symlinks(&self.root).map_err(|_|bad())?;
        let held=self.root_dir.metadata().map_err(|_|bad())?;let named=fs::symlink_metadata(&self.root).map_err(|_|bad())?;
        let l=self.lock.metadata().map_err(|_|bad())?;let ln=fs::symlink_metadata(self.root.join(LOCK)).map_err(|_|bad())?;
        if !named.is_dir()||metadata_id(&held)!=self.identity||metadata_id(&named)!=self.identity||!regular(&l)||!regular(&ln)||metadata_id(&l)!=self.lock_identity||metadata_id(&ln)!=self.lock_identity{return Err(bad());}Ok(())
    }
    pub(crate) fn identity(&self)->(u64,u64){self.identity}
    pub(crate) fn root(&self)->&Path{&self.root}
    pub(crate) fn root_directory(&self)->&File{&self.root_dir}
    pub(crate) fn bulk_path(&self)->&Path{&self.bulk_path}
    pub(crate) fn directory(&self)->&File{&self.bulk_dir}
}

#[derive(Clone)]
pub(crate) struct RemoteJobInputV1 {
    pub job_id:JobId,pub submission:SubmissionKeyV1,pub root_file:ConsentFile,pub profile_id:String,
    pub initial_revision:String,pub accept_archived:bool,pub consent_fingerprint:String,
    pub submission_name:String,pub profile_name_at_start:String,
}
impl RemoteJobInputV1 {
    fn input(&self)->JsonValue {object!{version:1,jobId:self.job_id.as_str(),submission:self.submission.json(),rootFileId:self.root_file.file_id.clone(),profileId:self.profile_id.clone(),initialRevision:self.initial_revision.clone(),acceptArchived:self.accept_archived,consentFingerprint:self.consent_fingerprint.clone()}}
    fn body(&self,root:(u64,u64),fsid:&str)->JsonValue {object!{version:1,jobId:self.job_id.as_str(),rootIdentity:object!{device:root.0.to_string(),inode:root.1.to_string(),fsid:fsid},profileNameAtStart:self.profile_name_at_start.clone(),state:"resolving",sequence:"0",lastControl:JsonValue::Null,selection:object!{submissionName:self.submission_name.clone(),rootFile:self.root_file.json()},cohort:object!{fingerprint:JsonValue::Null,complete:false,files:JsonValue::from(vec![self.root_file.json()])},claims:JsonValue::new_array(),prepare:JsonValue::Null,commit:JsonValue::Null,error:JsonValue::Null,cleanupPending:false}}
}
#[derive(Clone)]
pub(crate) struct ReplacementJobInputV2 {
    pub base:RemoteJobInputV1, pub purpose:ReplacementPurpose, pub mod_id:String, pub target_digest:String,
}
impl ReplacementJobInputV2 {
    fn input(&self)->JsonValue {
        let mut value=self.base.input();value["version"]=2.into();value["purpose"]=self.purpose.as_str().into();
        value["modId"]=self.mod_id.clone().into();value["targetDigest"]=self.target_digest.clone().into();value
    }
    fn body(&self,root:(u64,u64),fsid:&str)->JsonValue {let mut value=self.base.body(root,fsid);value["version"]=2.into();value}
}
#[derive(Clone)]
pub(crate) struct JobSnapshot {pub input:JsonValue,pub body:JsonValue}
pub(crate) struct StartReceipt {pub snapshot:JobSnapshot,pub created:bool}
pub(crate) struct ReconcileResult {pub waiting:bool,pub jobs:Vec<JobSnapshot>}
pub(crate) enum JobTransition { Body(JsonValue), Control{seq:String,action:String,result:String,body:JsonValue} }

fn validate_claim(c:&JsonValue,files:&BTreeMap<String,ConsentFile>)->Result<u64,NativeError>{
    let keys=["fileId","claimId","state","dirDevice","dirInode","fileDevice","fileInode","leaseDevice","leaseInode","uid","linkCount","durableOffset","prefixSha256","strongEtag","effectiveUriSha256","total","epoch","receipt","cleanup"];
    if c.has_key("claimFsid") {let mut with_fsid=keys.to_vec();with_fsid.push("claimFsid");exact(c,&with_fsid)?;if !c["claimFsid"].is_null(){hash(&c["claimFsid"],16)?;if c["dirDevice"].is_null(){return Err(bad());}}}else{exact(c,&keys)?;}
    let file=c["fileId"].as_str().ok_or_else(bad)?;positive(&c["fileId"])?;let consent=files.get(file).ok_or_else(bad)?;token(&c["claimId"])?;
    let state=c["state"].as_str().filter(|s|matches!(*s,"intent"|"owned"|"partial"|"sealed"|"cleaning"|"cleaned")).ok_or_else(bad)?;
    for k in ["dirDevice","dirInode","fileDevice","fileInode","leaseDevice","leaseInode","uid","linkCount","durableOffset","total"] {maybe_decimal(&c[k])?;}
    for (a,b) in [("dirDevice","dirInode"),("fileDevice","fileInode"),("leaseDevice","leaseInode")] {if c[a].is_null()!=c[b].is_null(){return Err(bad());}}
    if c["dirDevice"].is_null() && (!c["fileDevice"].is_null()||!c["leaseDevice"].is_null()) || c["fileDevice"].is_null()&&!c["leaseDevice"].is_null(){return Err(bad());}
    if c["fileDevice"].is_null()!=c["uid"].is_null()||c["fileDevice"].is_null()!=c["linkCount"].is_null()||state=="intent"&&!c["leaseDevice"].is_null(){return Err(bad());}
    maybe_hash(&c["prefixSha256"])?;maybe_hash(&c["effectiveUriSha256"])?;
    if !c["epoch"].is_null(){TransferEpoch::parse(c["epoch"].as_str().ok_or_else(bad)?).map_err(|_|bad())?;}
    if !c["strongEtag"].is_null(){let s=c["strongEtag"].as_str().ok_or_else(bad)?.as_bytes();if s.len()<2||s.len()>1024||s[0]!=b'"'||s[s.len()-1]!=b'"'||!s[1..s.len()-1].iter().all(|x|*x==0x21||(0x23..=0x7e).contains(x)){return Err(bad());}}
    if !c["strongEtag"].is_null()&&c["effectiveUriSha256"].is_null(){return Err(bad());}
    let offset=maybe_decimal(&c["durableOffset"])?.unwrap_or(0);let total=maybe_decimal(&c["total"])?;
    if offset>contract::MAX_MEMBER_BYTES||total.is_some_and(|n|n>contract::MAX_MEMBER_BYTES||offset>n){return Err(bad());}
    if offset>0 && (c["prefixSha256"].is_null()||c["epoch"].is_null()){return Err(bad());}
    if matches!(state,"owned"|"partial"|"sealed") && (c["dirDevice"].is_null()||c["fileDevice"].is_null()||c["leaseDevice"].is_null()||c["uid"].is_null()||c["linkCount"]!="1"||c["durableOffset"].is_null()){return Err(bad());}
    if maybe_decimal(&c["uid"])?.is_some_and(|n|n>u32::MAX as u64){return Err(bad());}
    if state=="partial"&&offset==0{return Err(bad());}
    if !c["receipt"].is_null(){let r=MemberReceiptV1::parse(&c["receipt"]).map_err(|_|bad())?;if &r.file!=consent||r.measured_size.parse::<u64>().ok()!=Some(offset)||total!=Some(offset)||!matches!(state,"sealed"|"cleaning"|"cleaned"){return Err(bad());}}
    if state=="sealed"&&c["receipt"].is_null(){return Err(bad());}
    let cleanup=&c["cleanup"];exact(cleanup,&["cursor","intent"])?;let cursor=decimal(&cleanup["cursor"])?;if cursor>3{return Err(bad());}
    if !cleanup["intent"].is_null(){let intent=&cleanup["intent"];exact(intent,&["kind","device","inode"])?;let (kind,dev,ino)=match cursor{0=>("data","fileDevice","fileInode"),1=>("lease","leaseDevice","leaseInode"),2=>("directory","dirDevice","dirInode"),_=>return Err(bad())};
        if intent["kind"]!=kind||c[dev].is_null()||c[ino].is_null()||intent["device"]!=c[dev]||intent["inode"]!=c[ino]{return Err(bad());}decimal(&intent["device"])?;decimal(&intent["inode"])?;}
    if !matches!(state,"cleaning"|"cleaned")&&(cursor!=0||!cleanup["intent"].is_null()){return Err(bad());}
    if state=="cleaned"&&(cursor!=3||!cleanup["intent"].is_null()){return Err(bad());}
    Ok(offset)
}
pub(crate) fn terminal_clean(body:&JsonValue)->bool {
    matches!(body["state"].as_str(),Some("installed"|"cancelled"|"abandoned"))
        && body["cleanupPending"]==false && body["claims"].members().all(|c|c["state"]=="cleaned")
}
fn tombstone(body:&JsonValue)->bool {terminal_clean(body)&&body["lastControl"]["action"]=="dismiss"&&body["lastControl"]["result"]=="accepted"}
fn validate_record(id:&str,input:&JsonValue,body:&JsonValue,lease:&RemoteStoreLease)->Result<(),NativeError>{
    let replacement=input["version"]==2;
    if replacement {
        exact(input,&["version","jobId","submission","rootFileId","profileId","initialRevision","acceptArchived","consentFingerprint","purpose","modId","targetDigest"])?;
        ReplacementPurpose::parse(&input["purpose"]).map_err(|_|bad())?;
        if !input["modId"].as_str().is_some_and(|s|paths::safe_id(s)){return Err(bad());}hash(&input["targetDigest"],64)?;
    } else {exact(input,&["version","jobId","submission","rootFileId","profileId","initialRevision","acceptArchived","consentFingerprint"])?;}
    if !(input["version"]==1||replacement)||input["jobId"]!=id||!package::hex(id,32)||!input["profileId"].as_str().is_some_and(paths::safe_id)||input["acceptArchived"].as_bool().is_none(){return Err(bad());}
    let submission=SubmissionKeyV1::parse(&input["submission"]).map_err(|_|bad())?;positive(&input["rootFileId"])?;decimal(&input["initialRevision"])?;hash(&input["consentFingerprint"],64)?;
    exact(body,&["version","jobId","rootIdentity","profileNameAtStart","state","sequence","lastControl","selection","cohort","claims","prepare","commit","error","cleanupPending"])?;
    if body["version"]!=input["version"]||body["jobId"]!=id||body["cleanupPending"].as_bool().is_none(){return Err(bad());}
    let identity=&body["rootIdentity"];
    if identity.has_key("fsid") {exact(identity,&["device","inode","fsid"])?;} else {exact(identity,&["device","inode"])?;}
    let fsid=if identity.has_key("fsid") {Some(identity["fsid"].as_str().filter(|s|package::hex(s,16)).ok_or_else(bad)?)} else {None};
    if !paths::same_persisted_object(lease.root(),&lease.root_dir,lease.root(),&lease.root_dir,decimal(&identity["device"])?,decimal(&identity["inode"])?,fsid).map_err(|_|bad())? {return Err(bad());}
    if !body["profileNameAtStart"].as_str().is_some_and(|s|!s.is_empty()&&s.len()<=256&&!s.chars().any(char::is_control)){return Err(bad());}
    if !matches!(body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily"|"sealing"|"downloaded"|"preparing"|"ready"|"reviewing"|"committing"|"installed"|"paused"|"failed"|"needsSelection"|"cancelRequested"|"cancelled"|"abandoned")){return Err(bad());}
    let sequence=decimal(&body["sequence"])?;
    if !body["lastControl"].is_null(){let c=&body["lastControl"];exact(c,&["controlSeq","action","result"])?;if positive(&c["controlSeq"])? > sequence{return Err(bad());}if !matches!(c["action"].as_str(),Some("pause"|"resume"|"retry"|"cancel"|"abandon"|"dismiss"))||!matches!(c["result"].as_str(),Some("accepted"|"rejected")){return Err(bad());}}
    let selection=&body["selection"];exact(selection,&["submissionName","rootFile"])?;let name=selection["submissionName"].as_str().filter(|s|!s.is_empty()&&s.len()<=256&&!s.chars().any(char::is_control)).ok_or_else(bad)?;
    let selected=ConsentFile::parse(&selection["rootFile"]).map_err(|_|bad())?;
    if selected.file_id!=input["rootFileId"].as_str().unwrap()||contract::candidate_fingerprint(&submission,name,&selected)!=input["consentFingerprint"].as_str().unwrap()||selected.archived()&&input["acceptArchived"]!=true{return Err(bad());}
    let cohort=&body["cohort"];exact(cohort,&["fingerprint","complete","files"])?;if cohort["complete"].as_bool().is_none()||!cohort["files"].is_array()||cohort["files"].len()==0||cohort["files"].len()>contract::MAX_MEMBERS{return Err(bad());}
    let mut files=BTreeMap::new();let mut ordered=Vec::new();for f in cohort["files"].members(){let f=ConsentFile::parse(f).map_err(|_|bad())?;if f.list!=selected.list||files.insert(f.file_id.clone(),f.clone()).is_some(){return Err(bad());}ordered.push(f);}
    if ordered[0]!=selected{return Err(bad());}maybe_hash(&cohort["fingerprint"])?;
    if cohort["complete"]==true {if cohort["fingerprint"].is_null()||contract::cohort_fingerprint(&submission,&selected.file_id,&ordered).map_err(|_|bad())?!=cohort["fingerprint"].as_str().unwrap()||ordered.iter().any(ConsentFile::archived)&&input["acceptArchived"]!=true{return Err(bad());}} else if !cohort["fingerprint"].is_null()||ordered.len()!=1{return Err(bad());}
    if !body["claims"].is_array()||body["claims"].len()>contract::MAX_MEMBERS{return Err(bad());}let mut ids=BTreeSet::new();let mut claims=BTreeSet::new();let mut bytes=0u64;
    for c in body["claims"].members(){if !ids.insert(c["fileId"].as_str().ok_or_else(bad)?.to_owned())||!claims.insert(c["claimId"].as_str().ok_or_else(bad)?.to_owned()){return Err(bad());}bytes=bytes.checked_add(validate_claim(c,&files)?).filter(|n|*n<=contract::MAX_COHORT_BYTES).ok_or_else(bad)?;}
    if !body["error"].is_null(){ErrorV1::parse(&body["error"]).map_err(|_|bad())?;}
    if !body["prepare"].is_null(){let p=&body["prepare"];exact(p,&["prepareKey","profileId","expectedRevision","acknowledgeRebase"])?;token(&p["prepareKey"])?;if p["profileId"]!=input["profileId"]||p["acknowledgeRebase"].as_bool().is_none(){return Err(bad());}let revision=decimal(&p["expectedRevision"])?;
        if replacement && (revision<decimal(&input["initialRevision"])? || revision!=decimal(&input["initialRevision"])? && p["acknowledgeRebase"]!=true){return Err(bad());}}
    if !body["commit"].is_null(){let c=&body["commit"];
        if replacement {
            let input_commit=ReplacementCommitInputV1::parse(c.clone()).map_err(|_|bad())?;let p=input_commit.params();
            if p["jobId"]!=id||p["profileId"]!=input["profileId"]||p["modId"]!=input["modId"]||p["purpose"]!=input["purpose"]
                ||p["initialRevision"]!=input["initialRevision"]||p["targetDigest"]!=input["targetDigest"]
                ||p["consentFingerprint"]!=input["consentFingerprint"]||p["cohortFingerprint"]!=cohort["fingerprint"]
                ||body["prepare"].is_null()||p["expectedRevision"]!=body["prepare"]["expectedRevision"]{return Err(bad());}
        } else {
            exact(c,&["method","params"])?;if c["method"]!="native.remote.commit"{return Err(bad());}let p=&c["params"];exact(p,&["operationId","jobId","profileId","expectedRevision","consentFingerprint","cohortFingerprint","selectionDigest"])?;if p["jobId"]!=id||p["profileId"]!=input["profileId"]||p["consentFingerprint"]!=input["consentFingerprint"]||p["cohortFingerprint"]!=cohort["fingerprint"]||!p["operationId"].as_str().is_some_and(paths::safe_id){return Err(bad());}decimal(&p["expectedRevision"])?;hash(&p["cohortFingerprint"],64)?;hash(&p["selectionDigest"],64)?;
        }}
    if matches!(body["state"].as_str(),Some("downloaded"|"preparing"|"ready"|"reviewing"|"committing"|"installed")){
        if cohort["complete"]!=true||ordered.iter().any(|f|!body["claims"].members().any(|c|c["fileId"].as_str()==Some(f.file_id.as_str())&&!c["receipt"].is_null()&&matches!(c["state"].as_str(),Some("sealed"|"cleaning"|"cleaned")))){return Err(bad());}
    }
    if matches!(body["state"].as_str(),Some("committing"|"installed"))&&body["commit"].is_null(){return Err(bad());}
    if matches!(body["state"].as_str(),Some("cancelled"|"abandoned"|"installed"))&&body["claims"].members().any(|c|c["state"]!="cleaned")&&body["cleanupPending"]!=true{return Err(bad());}
    if tombstone(body) && (!matches!(body["state"].as_str(),Some("installed"|"cancelled"|"abandoned"))||body["cleanupPending"]!=false||body["claims"].members().any(|c|c["state"]!="cleaned")){return Err(bad());}
    Ok(())
}

pub(crate) struct RemoteJobStore { lease:Arc<RemoteStoreLease>, db:Database, db_file:File, db_identity:(u64,u64) }
impl RemoteJobStore {
    /// Attaching a missing history is read-only and does not create a lock or database.
    pub(crate) fn open_existing(root:&Path)->Result<Option<Self>,NativeError>{
        let root=paths::absolute(root)?;paths::reject_symlinks(&root).map_err(|_|bad())?;
        if !database_exists(&root)? {return Ok(None);}
        Self::open_inner(&root,false).map(Some)
    }
    /// Call only after the library/profile admission has succeeded under RootLock.
    pub(crate) fn open(root:&Path)->Result<Self,NativeError>{Self::open_inner(root,true)}
    fn open_inner(root:&Path,create:bool)->Result<Self,NativeError>{
        let lease=Arc::new(RemoteStoreLease::acquire(root)?);Self::open_with_lease(lease,create)
    }
    pub(crate) fn open_with_lease(lease:Arc<RemoteStoreLease>,create:bool)->Result<Self,NativeError>{
        let root=lease.root();lease.validate()?;no_sidecars(root)?;
        let exists=database_exists(root)?;if !exists&&!create{return Err(bad());}
        if !exists&&namespace_present(root,true)?{return Err(bad());}
        if exists {
            header(root)?;
            let probe=Database::open(&root.join(DB),false).map_err(|_|bad())?;
            Self::admit_schema(&probe)?;Self::admit_rows(&probe,&lease)?;
            drop(probe);no_sidecars(root)?;lease.validate()?;
        }
        let db=Database::open(&root.join(DB),true).map_err(|_|bad())?;
        let db_file=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(root.join(DB)).map_err(|_|bad())?;
        let held=db_file.metadata().map_err(|_|bad())?;let named=fs::symlink_metadata(root.join(DB)).map_err(|_|bad())?;
        if !regular(&held)||!regular(&named)||metadata_id(&held)!=metadata_id(&named){return Err(bad());}let db_identity=metadata_id(&held);
        let mode=db.query("PRAGMA journal_mode=DELETE",&[])?.ok_or_else(bad)?;if mode!=["delete"]{return Err(bad());}
        db.execute("PRAGMA synchronous=FULL",&[])?;
        if !exists {let tx=db.transaction()?;db.execute(SCHEMA,&[])?;db.execute("PRAGMA user_version=1",&[])?;tx.commit()?;paths::sync_dir(root)?;}
        Self::admit_schema(&db)?;Self::admit_rows(&db,&lease)?;
        let page=Self::page_size(&db)?;let pages=MAX_DB/page;
        let result=db.query(&format!("PRAGMA max_page_count={pages}"),&[])?.ok_or_else(bad)?;
        if result.first().and_then(|s|s.parse::<u64>().ok())!=Some(pages){return Err(full());}
        let store=Self{lease,db,db_file,db_identity};store.validate_database()?;Ok(store)
    }
    fn page_size(db:&Database)->Result<u64,NativeError>{
        let p=db.query("PRAGMA page_size",&[])?.ok_or_else(bad)?[0].parse::<u64>().map_err(|_|bad())?;
        if !(512..=65536).contains(&p)||!p.is_power_of_two(){return Err(bad());}
        let pages=db.query("PRAGMA page_count",&[])?.ok_or_else(bad)?[0].parse::<u64>().map_err(|_|bad())?;
        if pages.checked_mul(p).is_none_or(|n|n>MAX_DB){return Err(bad());}Ok(p)
    }
    fn admit_schema(db:&Database)->Result<(),NativeError>{
        if db.query("PRAGMA user_version",&[])?.as_deref()!=Some(&["1".to_owned()][..]){return Err(bad());}
        let row=db.query("SELECT sql FROM sqlite_master WHERE type='table' AND name='jobs'",&[])?.ok_or_else(bad)?;
        if row.first().map(String::as_str)!=Some(SCHEMA){return Err(bad());}
        let extra=db.query("SELECT count(*) FROM sqlite_master WHERE NOT (type='table' AND name='jobs' AND tbl_name='jobs') AND NOT (type='index' AND name='sqlite_autoindex_jobs_1' AND tbl_name='jobs' AND sql IS NULL)",&[])?.ok_or_else(bad)?;
        let index=db.query("SELECT count(*) FROM sqlite_master WHERE type='index' AND name='sqlite_autoindex_jobs_1' AND tbl_name='jobs' AND sql IS NULL",&[])?.ok_or_else(bad)?;
        if extra!=["0"]||index!=["1"]{return Err(bad());}Self::page_size(db)?;Ok(())
    }
    fn admit_rows(db:&Database,lease:&RemoteStoreLease)->Result<Vec<(i64,JobSnapshot)>,NativeError>{
        let invalid=db.query("SELECT count(*) FROM jobs WHERE typeof(id)!='text' OR typeof(input)!='text' OR typeof(body)!='text' OR length(CAST(id AS BLOB))!=32 OR length(CAST(input AS BLOB))>131072 OR length(CAST(body AS BLOB))>131072 OR rowid<=0",&[])?.ok_or_else(bad)?;
        if invalid!=["0"]{return Err(bad());}
        let rows=db.query_rows("SELECT CAST(rowid AS TEXT),id,input,body FROM jobs ORDER BY rowid",&[],MAX_ACTIVE+MAX_TOMBSTONES,(MAX_ACTIVE+MAX_TOMBSTONES)*(2*MAX_ROW+128)).map_err(|_|bad())?;
        let count=db.query("SELECT count(*) FROM jobs",&[])?.ok_or_else(bad)?[0].parse::<usize>().map_err(|_|bad())?;if count!=rows.len(){return Err(bad());}
        let mut out=Vec::new();let mut active=0;let mut old=0;
        for row in rows {if row.len()!=4{return Err(bad());}let rid=row[0].parse::<i64>().map_err(|_|bad())?;let input=read_json(&row[2])?;let body=read_json(&row[3])?;validate_record(&row[1],&input,&body,lease)?;admit_status(&input,&body,lease.root())?;
            if terminal_clean(&body){old+=1}else{active+=1}out.push((rid,JobSnapshot{input,body}));}
        if active>MAX_ACTIVE||old>MAX_ACTIVE+MAX_TOMBSTONES{return Err(bad());}Ok(out)
    }
    fn validate_database(&self)->Result<(),NativeError>{
        self.lease.validate()?;let held=self.db_file.metadata().map_err(|_|bad())?;let named=fs::symlink_metadata(self.lease.root().join(DB)).map_err(|_|bad())?;
        if !regular(&held)||!regular(&named)||metadata_id(&held)!=self.db_identity||metadata_id(&named)!=self.db_identity||held.len()>MAX_DB{return Err(bad());}Ok(())
    }
    fn admitted(&self)->Result<Vec<(i64,JobSnapshot)>,NativeError>{if !self.db.maintenance_autocommit(){return Err(uncertain());}self.validate_database()?;no_sidecars(self.lease.root())?;Self::admit_schema(&self.db)?;let rows=Self::admit_rows(&self.db,&self.lease)?;self.validate_database()?;Ok(rows)}
    pub(crate) fn lease(&self)->Arc<RemoteStoreLease>{self.lease.clone()}
    pub(crate) fn snapshots(&self)->Result<Vec<JobSnapshot>,NativeError>{Ok(self.admitted()?.into_iter().map(|(_,j)|j).collect())}
    pub(crate) fn get(&self,id:&JobId)->Result<Option<JobSnapshot>,NativeError>{Ok(self.admitted()?.into_iter().map(|(_,j)|j).find(|j|j.input["jobId"]==id.as_str()))}
    /// Runs only through the worker-owned connection and held jobs lease.
    /// No payload, claim, or recovery artifact is deleted here.
    pub(crate) fn purge_terminal(&mut self, id:Option<&JobId>, keep:usize)->Result<(),NativeError>{
        let rows=self.admitted()?;
        if let Some(id)=id {
            if !rows.iter().any(|(_,j)|j.input["jobId"]==id.as_str()&&terminal_clean(&j.body)){return Err(bad());}
        }
        let eligible=rows.iter().filter(|(_,j)|terminal_clean(&j.body)).collect::<Vec<_>>();
        let excess=eligible.len().saturating_sub(keep);
        let tx=self.db.transaction().map_err(|_|uncertain())?;
        for (index,(rid,j)) in eligible.into_iter().enumerate(){
            if index<excess || id.is_some_and(|id|j.input["jobId"]==id.as_str()) {
                self.lease.validate()?;
                self.db.execute("DELETE FROM jobs WHERE rowid=?",&[&rid.to_string()]).map_err(|_|uncertain())?;
            }
        }
        tx.commit().map_err(|_|uncertain())?;self.validate_database()?;no_sidecars(self.lease.root())?;Ok(())
    }
    pub(crate) fn reserve_start(&mut self,input:&RemoteJobInputV1)->Result<StartReceipt,NativeError>{
        let fsid=paths::persisted_filesystem_id(&self.lease.root_dir)?;self.reserve(&input.job_id,input.input(),input.body(self.lease.identity(),&fsid))
    }
    pub(crate) fn reserve_replacement_start(&mut self,input:&ReplacementJobInputV2)->Result<StartReceipt,NativeError>{
        let fsid=paths::persisted_filesystem_id(&self.lease.root_dir)?;self.reserve(&input.base.job_id,input.input(),input.body(self.lease.identity(),&fsid))
    }
    fn reserve(&mut self,job_id:&JobId,value:JsonValue,body:JsonValue)->Result<StartReceipt,NativeError>{
        let rows=self.admitted()?;validate_record(job_id.as_str(),&value,&body,&self.lease)?;
        admit_status(&value,&body,self.lease.root())?;
        if let Some((_,old))=rows.iter().find(|(_,j)|j.input["jobId"]==job_id.as_str()){
            if store::canonical(&old.input)!=store::canonical(&value){return Err(NativeError::new("jobIdConflict","This job ID belongs to different remote input."));}
            return Ok(StartReceipt{snapshot:old.clone(),created:false});
        }
        if rows.iter().filter(|(_,j)|!terminal_clean(&j.body)).count()>=MAX_ACTIVE{return Err(full());}
        let i=store::canonical(&value);let b=store::canonical(&body);if i.len()>MAX_ROW||b.len()>MAX_ROW{return Err(full());}
        let tx=self.db.transaction().map_err(|_|uncertain())?;self.db.execute("INSERT INTO jobs(id,input,body) VALUES(?,?,?)",&[job_id.as_str(),&i,&b]).map_err(|_|uncertain())?;tx.commit().map_err(|_|uncertain())?;self.validate_database().map_err(|_|uncertain())?;no_sidecars(self.lease.root()).map_err(|_|uncertain())?;
        Ok(StartReceipt{snapshot:JobSnapshot{input:value,body},created:true})
    }
    pub(crate) fn transition(&mut self,job:&JobId,transition:JobTransition)->Result<JobSnapshot,NativeError>{
        let rows=self.admitted()?;let (_,old)=rows.iter().find(|(_,j)|j.input["jobId"]==job.as_str()).ok_or_else(||NativeError::new("jobAbsent","No retained remote job has this identity."))?;
        let mut body=match &transition {JobTransition::Body(b)=>b.clone(),JobTransition::Control{body,..}=>body.clone()};
        if tombstone(&old.body){return Err(NativeError::new("controlReceiptExpired","This remote job has already been dismissed."));}
        for key in ["version","jobId","rootIdentity","profileNameAtStart","selection"]{if body[key]!=old.body[key]{return Err(bad());}}
        // A null commit is negative-admission evidence only if an earlier
        // durable reservation can never be removed or replaced.
        if !old.body["commit"].is_null() && body["commit"]!=old.body["commit"]{return Err(bad());}
        if old.body["cohort"]["complete"]==true&&body["cohort"]!=old.body["cohort"]{return Err(bad());}
        for prior in old.body["claims"].members(){
            let next=body["claims"].members().find(|c|c["fileId"]==prior["fileId"]).ok_or_else(bad)?;
            if next["claimId"]!=prior["claimId"]{return Err(bad());}
            for key in ["dirDevice","dirInode","fileDevice","fileInode","leaseDevice","leaseInode","claimFsid","uid","linkCount","receipt"] {if !prior[key].is_null()&&prior[key]!=next[key]{return Err(bad());}}
            if decimal(&next["cleanup"]["cursor"])?<decimal(&prior["cleanup"]["cursor"])?{return Err(bad());}
            let before=maybe_decimal(&prior["durableOffset"])?.unwrap_or(0);let after=maybe_decimal(&next["durableOffset"])?.unwrap_or(0);
            if after<before&&!(after==0&&old.body["state"]=="resolving"&&next["state"]=="owned"&&next["prefixSha256"].is_null()&&next["strongEtag"].is_null()&&next["effectiveUriSha256"].is_null()&&next["total"].is_null()&&prior["receipt"].is_null()){return Err(bad());}
            if prior["epoch"]==next["epoch"]&&!prior["total"].is_null()&&!next["total"].is_null()&&prior["total"]!=next["total"]{return Err(bad());}
        }
        if old.body["state"]=="cancelRequested"&&!matches!(body["state"].as_str(),Some("cancelRequested"|"cancelled")){return Err(bad());}
        if matches!(&transition,JobTransition::Body(_))&&matches!(old.body["state"].as_str(),Some("paused"|"failed"|"needsSelection"|"cancelled"|"abandoned"|"installed"))&&body["state"]!=old.body["state"]{return Err(bad());}
        let seq=decimal(&old.body["sequence"])?.checked_add(1).ok_or_else(bad)?;body["sequence"]=seq.to_string().into();
        if let JobTransition::Control{seq,action,result,..}=&transition {
            let n=store::revision(seq).map_err(|_|bad())?;let last=if old.body["lastControl"].is_null(){0}else{decimal(&old.body["lastControl"]["controlSeq"])?};
            if n<=last {return Err(NativeError::new("controlReceiptExpired","The requested control receipt is no longer current."));}
            if n!=last.checked_add(1).ok_or_else(bad)?||!matches!(action.as_str(),"pause"|"resume"|"retry"|"cancel"|"abandon"|"dismiss")||!matches!(result.as_str(),"accepted"|"rejected"){return Err(NativeError::params());}
            body["lastControl"]=object!{controlSeq:seq.clone(),action:action.clone(),result:result.clone()};
        } else if body["lastControl"]!=old.body["lastControl"]{return Err(bad());}
        validate_record(job.as_str(),&old.input,&body,&self.lease)?;admit_status(&old.input,&body,self.lease.root())?;let text=store::canonical(&body);if text.len()>MAX_ROW{return Err(full());}
        let dismissal_rowid=if tombstone(&body) || terminal_clean(&body)&&!terminal_clean(&old.body){Some(rows.iter().map(|(n,_)|*n).max().unwrap_or(0).checked_add(1).ok_or_else(full)?)}else{None};
        let tx=self.db.transaction().map_err(|_|uncertain())?;
        self.db.execute("UPDATE jobs SET body=? WHERE id=?",&[&text,job.as_str()]).map_err(|_|uncertain())?;
        if let Some(next)=dismissal_rowid{
            self.db.execute("UPDATE jobs SET rowid=? WHERE id=?",&[&next.to_string(),job.as_str()]).map_err(|_|uncertain())?;
            let old_tombs:Vec<_>=rows.iter().filter(|(_,j)|tombstone(&j.body)).collect();
            if old_tombs.len()>=MAX_TOMBSTONES {let (rid,_)=old_tombs.iter().min_by_key(|(n,_)|*n).ok_or_else(bad)?;self.db.execute("DELETE FROM jobs WHERE rowid=?",&[&rid.to_string()]).map_err(|_|uncertain())?;}
        }
        tx.commit().map_err(|_|uncertain())?;self.validate_database().map_err(|_|uncertain())?;no_sidecars(self.lease.root()).map_err(|_|uncertain())?;
        if terminal_clean(&body){self.purge_terminal(None,20)?;}
        Ok(JobSnapshot{input:old.input.clone(),body})
    }
    /// Task 4 supplies the shared preparation slot and scratch reconciliation.
    pub(crate) fn reconcile(&mut self)->Result<ReconcileResult,NativeError>{
        let rows=self.admitted()?;let waiting=!rows.is_empty()||self.remote_namespace_present()?;
        Ok(ReconcileResult{waiting,jobs:rows.into_iter().map(|(_,j)|j).collect()})
    }
    fn remote_namespace_present(&self)->Result<bool,NativeError>{
        self.lease.validate()?;namespace_present(self.lease.root(),true)
    }
}

pub(crate) fn status(snapshot:&JobSnapshot,root:&Path,session:&SessionId,progress:Option<JsonValue>)->Result<JsonValue,NativeError>{
    let input=&snapshot.input;let body=&snapshot.body;let mut members=Vec::new();
    for f in body["cohort"]["files"].members(){let c=body["claims"].members().find(|c|c["fileId"]==f["fileId"]);
        members.push(object!{fileId:f["fileId"].clone(),originalName:f["originalName"].clone(),state:c.map(|c|c["state"].clone()).unwrap_or_else(||"pending".into()),durableOffset:c.map(|c|if c["durableOffset"].is_null(){"0".into()}else{c["durableOffset"].clone()}).unwrap_or_else(||"0".into()),total:c.map(|c|c["total"].clone()).unwrap_or(JsonValue::Null),receipt:c.map(|c|c["receipt"].clone()).unwrap_or(JsonValue::Null)});
    }
    let p=&body["prepare"];let commit=&body["commit"];
    let value=object!{version:1,root:root.to_str().ok_or_else(bad)?,sessionId:session.as_str(),jobId:input["jobId"].clone(),submission:input["submission"].clone(),profileId:input["profileId"].clone(),profileNameAtStart:body["profileNameAtStart"].clone(),initialRevision:input["initialRevision"].clone(),reviewRevision:if p.is_null(){JsonValue::Null}else{p["expectedRevision"].clone()},sequence:body["sequence"].clone(),state:body["state"].clone(),progress:progress.unwrap_or(JsonValue::Null),cohort:object!{fingerprint:body["cohort"]["fingerprint"].clone(),complete:body["cohort"]["complete"].clone(),members:JsonValue::from(members)},prepareKey:if p.is_null(){JsonValue::Null}else{p["prepareKey"].clone()},review:JsonValue::Null,operationId:if commit.is_null(){JsonValue::Null}else{commit["params"]["operationId"].clone()},lastControl:body["lastControl"].clone(),error:body["error"].clone(),cleanupPending:body["cleanupPending"].clone()};
    Ok(RemoteStatusV1::parse(value).map_err(|_|bad())?.json())
}
pub(crate) fn is_tombstone(body:&JsonValue)->bool{tombstone(body)}
fn admit_status(input:&JsonValue,body:&JsonValue,root:&Path)->Result<(),NativeError>{
    let session=SessionId::parse("00000000000000000000000000000000").map_err(|_|bad())?;
    let snapshot=JobSnapshot{input:input.clone(),body:body.clone()};let value=status(&snapshot,root,&session,None).map_err(|_|full())?;
    let (method,value)=if input["version"]==2{("native.remote.replaceStatus",object!{version:1,root:root.to_str().ok_or_else(bad)?,sessionId:session.as_str(),purpose:input["purpose"].clone(),modId:input["modId"].clone(),targetDigest:input["targetDigest"].clone(),status:value,review:JsonValue::Null})}else{("native.remote.status",value)};
    contract::validate_result(method,&value).map_err(|_|bad())?;
    let envelope=object!{jsonrpc:"2.0",id:i64::MIN,result:value};if !contract::result_envelope_ok(method,&envelope){return Err(full());}Ok(())
}

#[cfg(test)]
#[test]
fn item93_legacy_remote_history_survives_btrfs_device_change() {
    let fixture = crate::test_support::Fixture::new();
    fs::create_dir(&fixture.data).unwrap();
    if !paths::test_volatile_filesystem(&File::open(&fixture.data).unwrap()) { return; }
    let submission = SubmissionKeyV1 { submission_type: "mod".into(), submission_id: "42".into() };
    let file = ConsentFile { file_id: "1".into(), original_name: "sample.vpk".into(), declared_size: Some("52".into()), created_at: None, md5: None, list: "current".into(), archive_flag: None };
    let input = RemoteJobInputV1 {
        job_id: JobId::parse("11111111111111111111111111111111").unwrap(),
        consent_fingerprint: contract::candidate_fingerprint(&submission, "Sample", &file),
        submission, root_file: file, profile_id: "default".into(), initial_revision: "1".into(),
        accept_archived: false, submission_name: "Sample".into(), profile_name_at_start: "Default".into(),
    };
    let mut store = RemoteJobStore::open(&fixture.data).unwrap();
    store.reserve_start(&input).unwrap();
    drop(store);
    let db = Database::open(&fixture.data.join(DB), true).unwrap();
    let row = db.query("SELECT body FROM jobs WHERE id=?", &[input.job_id.as_str()]).unwrap().unwrap();
    let mut body = json::parse(&row[0]).unwrap();
    assert_eq!(body["rootIdentity"]["fsid"].as_str().unwrap().len(), 16);
    body["rootIdentity"].remove("fsid");
    body["rootIdentity"]["device"] = (fs::metadata(&fixture.data).unwrap().dev() + 1).to_string().into();
    db.execute("UPDATE jobs SET body=? WHERE id=?", &[&store::canonical(&body), input.job_id.as_str()]).unwrap();
    drop(db);
    let reopened = RemoteJobStore::open_existing(&fixture.data);
    assert!(reopened.is_ok(), "legacy history should be admitted on btrfs when the root inode is unchanged");
    assert_eq!(reopened.ok().flatten().unwrap().snapshots().unwrap().len(), 1);
}

#[cfg(test)]
#[test]
fn item106_terminal_retention_and_dismiss() {
    let fixture=crate::test_support::Fixture::new();fs::create_dir(&fixture.data).unwrap();
    let mut db=RemoteJobStore::open(&fixture.data).unwrap();
    let submission=SubmissionKeyV1{submission_type:"mod".into(),submission_id:"42".into()};
    let file=ConsentFile{file_id:"1".into(),original_name:"sample.vpk".into(),declared_size:Some("52".into()),created_at:None,md5:None,list:"current".into(),archive_flag:None};
    // Purging must refuse active and cleanup-pending jobs, and retain both
    // even while automatic retention removes older completed records.
    let mut last=None;
    for n in 0..=25 {
        let id=JobId::parse(&format!("{:032x}",n)).unwrap();
        let input=RemoteJobInputV1{job_id:id.clone(),consent_fingerprint:contract::candidate_fingerprint(&submission,"Sample",&file),submission:submission.clone(),root_file:file.clone(),profile_id:"default".into(),initial_revision:"1".into(),accept_archived:false,submission_name:"Sample".into(),profile_name_at_start:"Default".into()};
        let mut row=db.reserve_start(&input).expect("finished jobs must not fill active slots").snapshot;
        assert!(db.purge_terminal(Some(&id),0).is_err());
        if n==0 {continue;}
        if n==1 {
            row.body["state"]="cancelled".into();row.body["cleanupPending"]=true.into();
            row=db.transition(&id,JobTransition::Body(row.body)).unwrap();
            assert!(db.purge_terminal(Some(&id),0).is_err());
            assert!(db.get(&id).unwrap().is_some());row.body["cleanupPending"]=false.into();
        }
        row.body["state"]="cancelled".into();
        row=db.transition(&id,JobTransition::Body(row.body)).unwrap();last=Some((id,row));
    }
    assert_eq!(db.snapshots().unwrap().len(),21);
    let oldest=JobId::parse(&format!("{:032x}",0)).unwrap();
    let mut row=db.get(&oldest).unwrap().unwrap();row.body["state"]="cancelled".into();
    db.transition(&oldest,JobTransition::Body(row.body)).unwrap();
    assert!(db.get(&oldest).unwrap().is_some(),"retain the latest finish, even if it started first");
    assert_eq!(db.snapshots().unwrap().len(),20);
    let (id,row)=last.unwrap();
    db.transition(&id,JobTransition::Control{seq:"1".into(),action:"dismiss".into(),result:"accepted".into(),body:row.body}).unwrap();
    assert!(is_tombstone(&db.get(&id).unwrap().unwrap().body),"Dismiss must hide the job while retaining its control receipt");
    db.purge_terminal(Some(&id),20).unwrap();assert!(db.get(&id).unwrap().is_none());
    let mut protected=Vec::new();
    for n in 100..102 {
        let id=JobId::parse(&format!("{:032x}",n)).unwrap();
        let input=RemoteJobInputV1{job_id:id.clone(),consent_fingerprint:contract::candidate_fingerprint(&submission,"Sample",&file),submission:submission.clone(),root_file:file.clone(),profile_id:"default".into(),initial_revision:"1".into(),accept_archived:false,submission_name:"Sample".into(),profile_name_at_start:"Default".into()};
        let mut row=db.reserve_start(&input).unwrap().snapshot;
        if n==101 {row.body["state"]="cancelled".into();row.body["cleanupPending"]=true.into();db.transition(&id,JobTransition::Body(row.body)).unwrap();}
        protected.push(id);
    }
    db.purge_terminal(None,0).unwrap();assert_eq!(db.snapshots().unwrap().len(),2);
    for id in protected {assert!(db.get(&id).unwrap().is_some());}

}

#[cfg(test)]
#[test]
fn item106_failed_dismiss_is_replayable_and_hidden() {
    use super::owner::RemoteOwner;
    use std::time::{Duration,Instant};
    let fixture=crate::test_support::Fixture::new();
    let mut service=crate::native::NativeService::new();crate::test_support::import(&mut service,&fixture);
    let submission=SubmissionKeyV1{submission_type:"mod".into(),submission_id:"42".into()};
    let file=ConsentFile{file_id:"1".into(),original_name:"sample.vpk".into(),declared_size:Some("52".into()),created_at:None,md5:None,list:"current".into(),archive_flag:None};
    let id=JobId::parse("11111111111111111111111111111111").unwrap();
    let input=RemoteJobInputV1{job_id:id.clone(),consent_fingerprint:contract::candidate_fingerprint(&submission,"Sample",&file),submission,root_file:file,profile_id:"default".into(),initial_revision:"1".into(),accept_archived:false,submission_name:"Sample".into(),profile_name_at_start:"Default".into()};
    let mut db=RemoteJobStore::open(&fixture.data).unwrap();let mut row=db.reserve_start(&input).unwrap().snapshot;
    row.body["state"]="failed".into();db.transition(&id,JobTransition::Body(row.body)).unwrap();drop(db);
    let session=SessionId::parse("22222222222222222222222222222222").unwrap();
    let mut owner=RemoteOwner::open(fixture.data.clone(),session.clone()).unwrap();
    let base=object!{version:1,dataDir:fixture.data.to_str().unwrap(),sessionId:session.as_str()};
    let mut control=base.clone();control["jobId"]=id.as_str().into();control["controlSeq"]="1".into();control["action"]="dismiss".into();
    let deadline=Instant::now()+Duration::from_secs(5);
    loop {
        assert!(Instant::now()<deadline,"dismiss never settled");
        if let Ok(result)=owner.dispatch("native.remote.control",&control){assert_eq!(result["state"],"accepted");break;}
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(owner.dispatch("native.remote.control",&control).unwrap()["state"],"accepted");
    loop {
        assert!(Instant::now()<deadline,"dismissed job remains visible");
        if let Ok(result)=owner.dispatch("native.remote.list",&base){assert_eq!(result["jobs"].len(),0);break;}
        std::thread::sleep(Duration::from_millis(10));
    }
    owner.begin_close();
}
