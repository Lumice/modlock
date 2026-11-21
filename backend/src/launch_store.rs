//! Same-connection durable launch coordination. No process spawning or RPC surface.
//! Order: retained RootOwnerLease, RootLock, GameLock. Never acquire root while
//! retaining game: callers release game and reacquire both for settlement.
use crate::{game_access::GameLock, launch_preparation::{PreparationPlan, PreparationMode, PreparationProgress, PreparedProof},
    launch_record::{self as r, *}, native::NativeError, paths, prepare_control::PrepareControl, store::{self, RootLock, Store}};
use json::{object, JsonValue};
use std::{fs::{self, File, OpenOptions}, io::Write, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}}, path::{Path, PathBuf}, time::{Duration, Instant}};

pub struct RootOwnerLease { root: PathBuf, directory: File, file: File, token: String, session: Option<(u64,String)> }
impl RootOwnerLease {
    /// Acquire before RootLock; acquiring the stable lease never rotates an epoch.
    pub fn acquire(root: &Path) -> Result<Self,NativeError> {
        let root=paths::absolute(root)?; paths::reject_symlinks(&root)?;
        let directory=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&root)?;
        let file=OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(0x20000|0x800).open(root.join(".launch-owner.lock"))?;
        if !file.metadata()?.is_file() {return Err(r::invalid());}
        extern "C" {fn flock(fd:i32,op:i32)->i32;}
        if unsafe {flock(file.as_raw_fd(),2|4)}!=0 {return Err(NativeError::new("launchOwnerBusy","Another live helper owns launch coordination for this library."));}
        paths::sync_dir(&root)?;
        let lease=Self{root,directory,file,token:paths::random_id()?,session:None}; lease.validate()?; Ok(lease)
    }
    fn validate(&self)->Result<(),NativeError> {
        paths::reject_symlinks(&self.root)?;
        let current=fs::symlink_metadata(&self.root)?; let held=self.directory.metadata()?;
        let named=paths::open_regular(&self.root.join(".launch-owner.lock"))?.metadata()?; let file=self.file.metadata()?;
        if !current.is_dir() || (current.dev(),current.ino())!=(held.dev(),held.ino()) || (named.dev(),named.ino())!=(file.dev(),file.ino()) {return Err(r::invalid());} Ok(())
    }
    pub fn epoch(&self)->Option<u64>{self.session.as_ref().map(|s|s.0)}
}
impl Drop for RootOwnerLease {fn drop(&mut self){extern "C"{fn flock(fd:i32,op:i32)->i32;} unsafe{flock(self.file.as_raw_fd(),8)};}}

/// All fields are private; ready/status JSON cannot reconstruct a live review.
pub struct LiveReview { epoch:u64, id:String, revision:u64, owner:String, plan:PreparationPlan, ready:JsonValue }
pub struct LiveCommit { review:LiveReview, revision:u64 }
pub struct LivePrepared { epoch:u64, id:String, revision:u64, owner:String, proof:PreparedProof }
pub enum CommitAdmission { First(LiveCommit), Replay(JsonValue) }
/// A single in-memory chain authorizes exactly one durable dispatch admission.
pub struct HandoffClaim { root:PathBuf, epoch:u64, id:String, journal:String, revision:u64, owner:String, dispatched:bool, proof:PreparedProof }
/// Returned once, after durable dispatchIntent. Not clonable or deserializable.
pub struct DispatchPermit { claim:HandoffClaim }
/// Distinct from dispatch authority; contains only an admitted cleanup target.
pub struct SettlementClaim { root:PathBuf, journal:JsonValue }
/// Only the later trusted process observer may create these from a complete scan.
/// Neither durable records nor public JSON can construct process evidence.
pub struct FreshIdle { game:PathBuf, observed:Instant }
pub struct ObservedGameCycle { game:PathBuf, observed:Instant, epoch:u64, attempt:String, journal:String, dispatch_revision:u64 }
impl FreshIdle {
    #[allow(dead_code)] // Task4 observer integration; no process scanner is added here.
    pub(crate) fn from_complete_scan(game:PathBuf)->Self{Self{game,observed:Instant::now()}}
    fn check(&self, game:&Path)->Result<(),NativeError>{if self.game!=game || self.observed.elapsed()>Duration::from_secs(2){return Err(r::invalid());}Ok(())}
}
impl ObservedGameCycle {
    #[allow(dead_code)] // Task4 observer must have observed start and complete fresh exit.
    pub(crate) fn from_completed_cycle(permit:&DispatchPermit,game:PathBuf)->Self{
        Self{game,observed:Instant::now(),epoch:permit.claim.epoch,attempt:permit.claim.id.clone(),journal:permit.claim.journal.clone(),dispatch_revision:permit.claim.revision}
    }
}
pub enum TerminalBeforeHandoff { Cancelled, PreparationFailed }

fn parse(body:&str)->Result<JsonValue,NativeError>{
    let value=json::parse(body).map_err(|_|r::invalid())?;
    // New table records are canonical on disk. This narrow roundtrip also rejects
    // duplicate-key spellings without changing the shared/historical JSON parser.
    if store::canonical(&value)!=body{return Err(r::invalid());}Ok(value)
}
fn next(n:u64)->Result<u64,NativeError>{n.checked_add(1).ok_or_else(r::invalid)}
fn conflict()->NativeError{NativeError::new("launchIdentityConflict","This launch identity was already admitted with different input.")}
fn stale()->NativeError{NativeError::new("staleLaunchEpoch","This launch session is stale; open an explicit new owner session.")}
fn capacity()->NativeError{NativeError::new("launchSessionLimit","This launch session has reached its 64-operation limit; explicitly open a new session with a new owner.")}
fn default_settings()->JsonValue{object!{version:1,revision:"0",additionalArguments:""}}
fn live_owner(store:&Store,lock:&RootLock,lease:&RootOwnerLease,epoch:u64)->Result<JsonValue,NativeError>{
    store.locked(lock)?; lease.validate()?;
    if store.root()!=lease.root || lease.epoch()!=Some(epoch){return Err(stale());}
    let m=store.launch_meta()?.ok_or_else(stale)?;
    if u(&m,"epoch")?!=epoch || m["ownerToken"]!=lease.token {return Err(stale());} Ok(m)
}
fn bound_owner(store:&Store,lock:&RootLock,lease:&RootOwnerLease,epoch:u64,token:&str)->Result<(),NativeError>{
    live_owner(store,lock,lease,epoch)?; if token!=lease.token{return Err(stale());} Ok(())
}
fn bump(a:&mut JsonValue)->Result<(),NativeError>{a["recordRevision"]=next(u(a,"recordRevision")?)?.to_string().into();Ok(())}
fn require_revision(a:&JsonValue, revision:u64)->Result<(),NativeError>{if u(a,"recordRevision")?!=revision{return Err(NativeError::new("staleLaunchRecord","Launch state changed; refresh its exact record before continuing."));}Ok(())}
fn require_settings(store:&Store,a:&JsonValue)->Result<(),NativeError>{
    let settings=store.launch_settings()?;
    if settings["revision"]!=a["beginInput"]["params"]["settingsRevision"] || digest(&settings)?!=s(a,"settingsSha256")?{return Err(NativeError::new("staleLaunchSettings","Launch arguments changed; start a new launch review."));}Ok(())
}
fn reserve_attempt(a:&JsonValue)->Result<(),NativeError>{
    r::attempt(a)?; r::query(a)?;
    let mut widest=a.clone(); widest["recordRevision"]=u64::MAX.to_string().into();widest["state"]="preparing".into();
    // Fixed terminal outcomes and later handoff fields are reserved before each write.
    widest["handoff"]=object!{journalId:"x".repeat(64),stage:"dispatchIntent"};
    widest["outcome"]=object!{status:"cancelled",reason:"steamIntentCancelled"};
    bounded(&widest,CONTROL_LIMIT-1024)
}
impl Store {
    pub(crate) fn create_launch_tables(&self)->Result<(),NativeError>{
        let db=self.launch_database();
        db.execute("CREATE TABLE IF NOT EXISTS launch_meta(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)",&[])?;
        db.execute("CREATE TABLE IF NOT EXISTS launch_settings(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)",&[])?;
        db.execute("CREATE TABLE IF NOT EXISTS launch_attempts(epoch TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(epoch,id))",&[])?;
        db.execute("CREATE TABLE IF NOT EXISTS launch_settings_ops(epoch TEXT NOT NULL,id TEXT NOT NULL,input TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(epoch,id))",&[])
    }
    fn has_launch_schema(&self)->Result<bool,NativeError>{Ok(self.launch_database().query("SELECT name FROM sqlite_master WHERE type='table' AND name='launch_meta'",&[])?.is_some())}
    fn launch_meta(&self)->Result<Option<JsonValue>,NativeError>{
        if !self.has_launch_schema()?{return Ok(None);}
        self.launch_database().query("SELECT body FROM launch_meta WHERE id=1",&[])?.map(|row|{let m=parse(&row[0])?;r::meta(&m)?;Ok(m)}).transpose()
    }
    /// Read-only status never acquires a lease, initializes tables or opens an epoch.
    pub fn launch_session(&self)->Result<JsonValue,NativeError>{
        let m=self.launch_meta()?;
        Ok(object!{version:1,epoch:m.as_ref().map(|v|v["epoch"].clone()).unwrap_or(JsonValue::Null),
            admitted:m.as_ref().map(|v|v["admitted"].clone()).unwrap_or("0".into()),settingsWrites:m.as_ref().map(|v|v["settingsWrites"].clone()).unwrap_or("0".into())})
    }
    pub fn launch_owner_open(&self,lock:&RootLock,lease:&mut RootOwnerLease,open_id:&str)->Result<JsonValue,NativeError>{
        self.locked(lock)?; lease.validate()?;
        if lease.root!=self.root() || !paths::safe_id(open_id){return Err(NativeError::params());}
        self.document()?;
        if let Some((epoch,id))=&lease.session {
            if id!=open_id{return Err(conflict());} live_owner(self,lock,lease,*epoch)?; return self.launch_session();
        }
        let old=self.launch_meta()?;
        if let Some(old)=old.as_ref().filter(|v|v["openId"]==open_id) {
            if old["ownerToken"]!=lease.token{return Err(conflict());}
            lease.session=Some((u(old,"epoch")?,open_id.into()));return self.launch_session();
        }
        let epoch=next(old.as_ref().map(|v|u(v,"epoch")).transpose()?.unwrap_or(0))?;
        let m=object!{version:1,epoch:epoch.to_string(),ownerToken:lease.token.clone(),openId:open_id,admitted:"0",settingsWrites:"0"};r::meta(&m)?;
        let settings=self.launch_settings()?; r::settings(&settings)?;
        let tx=self.launch_database().transaction()?;
        tx.db.execute("INSERT INTO launch_settings(id,body) VALUES(1,?) ON CONFLICT(id) DO NOTHING",&[&store::canonical(&settings)])?;
        tx.db.execute("INSERT INTO launch_meta(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",&[&store::canonical(&m)])?;tx.commit()?;
        lease.session=Some((epoch,open_id.into()));self.launch_session()
    }
    pub fn launch_settings(&self)->Result<JsonValue,NativeError>{
        if !self.has_launch_schema()?{return Ok(default_settings());}
        let row=self.launch_database().query("SELECT body FROM launch_settings WHERE id=1",&[])?;
        let v=match row {Some(row)=>parse(&row[0])?,None=>{
            if self.launch_meta()?.is_some(){return Err(r::invalid());}default_settings()
        }};
        r::settings(&v)?; bounded(&v,CONTROL_LIMIT-256)?;Ok(v)
    }
    /// Replay is a revision/digest receipt, never a historical copy of raw arguments.
    pub fn launch_settings_set(&self,lock:&RootLock,lease:&RootOwnerLease,epoch:u64,id:&str,expected:u64,raw:&str)->Result<JsonValue,NativeError>{
        validate_arguments(raw)?; if !paths::safe_id(id){return Err(NativeError::params());}
        let mut m=live_owner(self,lock,lease,epoch)?;
        let input=object!{method:"native.launch.settings.set",params:object!{epoch:epoch.to_string(),operationId:id,expectedSettingsRevision:expected.to_string(),argumentsSha256:paths::digest_bytes(raw.as_bytes())?}};
        if let Some(row)=self.launch_database().query("SELECT input,result FROM launch_settings_ops WHERE epoch=? AND id=?",&[&epoch.to_string(),id])? {
            if parse(&row[0])?!=input{return Err(conflict());} let result=parse(&row[1])?;
            exact(&result,&["version","revision","settingsSha256"])?;
            if result["version"]!=1 || u(&result,"revision")?!=next(expected)?{return Err(r::invalid());}hex(&result,"settingsSha256",64)?;
            let expected_settings=object!{version:1,revision:next(expected)?.to_string(),additionalArguments:raw};
            if result["settingsSha256"]!=digest(&expected_settings)?{return Err(r::invalid());}return Ok(result);
        }
        if u(&m,"settingsWrites")?>=64{return Err(capacity());}
        let current=self.launch_settings()?;if u(&current,"revision")?!=expected{return Err(NativeError::new("staleLaunchSettings","Launch settings changed; reload before saving."));}
        let settings=object!{version:1,revision:next(expected)?.to_string(),additionalArguments:raw};r::settings(&settings)?;bounded(&settings,CONTROL_LIMIT-256)?;
        let result=object!{version:1,revision:settings["revision"].clone(),settingsSha256:digest(&settings)?};bounded(&input,CONTROL_LIMIT-256)?;bounded(&result,CONTROL_LIMIT-256)?;
        m["settingsWrites"]=next(u(&m,"settingsWrites")?)?.to_string().into();
        let tx=self.launch_database().transaction()?;
        tx.db.execute("INSERT INTO launch_settings_ops(epoch,id,input,result) VALUES(?,?,?,?)",&[&epoch.to_string(),id,&store::canonical(&input),&store::canonical(&result)])?;
        tx.db.execute("UPDATE launch_settings SET body=? WHERE id=1",&[&store::canonical(&settings)])?;
        tx.db.execute("UPDATE launch_meta SET body=? WHERE id=1",&[&store::canonical(&m)])?;tx.commit()?;Ok(result)
    }
    /// Explicit read-only mutation reconciliation, including stale epochs. This
    /// receipt identifies the exact raw-byte digest, never returns historical args.
    pub fn launch_settings_operation(&self,epoch:u64,id:&str)->Result<JsonValue,NativeError>{
        if epoch==0 || !paths::safe_id(id){return Err(NativeError::params());}
        let row=if self.has_launch_schema()?{self.launch_database().query("SELECT input,result FROM launch_settings_ops WHERE epoch=? AND id=?",&[&epoch.to_string(),id])?}else{None};
        let Some(row)=row else{return Ok(object!{version:1,epoch:epoch.to_string(),operationId:id,status:"unknown"});};
        let input=parse(&row[0])?;let receipt=parse(&row[1])?;
        exact(&input,&["method","params"])?;let p=&input["params"];
        exact(p,&["epoch","operationId","expectedSettingsRevision","argumentsSha256"])?;
        if input["method"]!="native.launch.settings.set" || p["epoch"]!=epoch.to_string() || p["operationId"]!=id{return Err(r::invalid());}
        hex(p,"argumentsSha256",64)?;let revision=next(u(p,"expectedSettingsRevision")?)?;
        exact(&receipt,&["version","revision","settingsSha256"])?;hex(&receipt,"settingsSha256",64)?;
        if receipt["version"]!=1 || u(&receipt,"revision")?!=revision{return Err(r::invalid());}
        let response=object!{version:1,epoch:epoch.to_string(),operationId:id,status:"succeeded",input:input,receipt:receipt};bounded(&response,CONTROL_LIMIT-256)?;Ok(response)
    }
    fn launch_attempt(&self,epoch:u64,id:&str)->Result<Option<JsonValue>,NativeError>{
        if !paths::safe_id(id) || epoch==0{return Err(NativeError::params());}
        if !self.has_launch_schema()?{return Ok(None);}
        self.launch_database().query("SELECT body FROM launch_attempts WHERE epoch=? AND id=?",&[&epoch.to_string(),id])?.map(|row|{
            let a=parse(&row[0])?;r::attempt(&a)?;if a["epoch"]!=epoch.to_string() || a["attemptId"]!=id{return Err(r::invalid());}Ok(a)
        }).transpose()
    }
    fn required_attempt(&self,epoch:u64,id:&str)->Result<JsonValue,NativeError>{self.launch_attempt(epoch,id)?.ok_or_else(r::invalid)}
    pub fn launch_query(&self,epoch:u64,id:&str)->Result<JsonValue,NativeError>{
        match self.launch_attempt(epoch,id)?{Some(a)=>r::query(&a),None=>Ok(object!{version:1,epoch:epoch.to_string(),attemptId:id,state:"unknown"})}
    }
    /// The exact begin object is persisted; retries return facts and no live review.
    pub fn launch_begin(&self,lock:&RootLock,lease:&RootOwnerLease,input:&JsonValue)->Result<JsonValue,NativeError>{
        r::begin(input)?;let p=&input["params"];let epoch=u(p,"epoch")?;let id=s(p,"attemptId")?;
        let mut m=live_owner(self,lock,lease,epoch)?;
        if let Some(a)=self.launch_attempt(epoch,id)? {if a["beginInput"]!=*input{return Err(conflict());}return r::query(&a);}
        if u(&m,"admitted")?>=64{return Err(capacity());}
        crate::apply::ensure_idle(self)?;let doc=self.document()?;
        if doc["recoveryRequired"]!=false{return Err(r::barrier());}
        if doc["revision"]!=p["expectedRevision"] || doc["activeProfileId"]!=p["profileId"]{return Err(NativeError::new("staleRevision","Launch requires the current explicitly named active profile."));}
        let settings=self.launch_settings()?;if settings["revision"]!=p["settingsRevision"]{return Err(NativeError::new("staleLaunchSettings","Launch settings changed; refresh first."));}
        let a=object!{version:1,epoch:epoch.to_string(),attemptId:id,beginInput:input.clone(),commitInput:JsonValue::Null,recordRevision:"0",state:"admitted",settingsSha256:digest(&settings)?,ready:JsonValue::Null,preparation:JsonValue::Null,handoff:JsonValue::Null,outcome:JsonValue::Null};reserve_attempt(&a)?;
        m["admitted"]=next(u(&m,"admitted")?)?.to_string().into();
        let tx=self.launch_database().transaction()?;
        tx.db.execute("INSERT INTO launch_attempts(epoch,id,body) VALUES(?,?,?)",&[&epoch.to_string(),id,&store::canonical(&a)])?;
        tx.db.execute("UPDATE launch_meta SET body=? WHERE id=1",&[&store::canonical(&m)])?;tx.commit()?;r::query(&a)
    }
    fn save_attempt(&self,a:&JsonValue)->Result<(),NativeError>{reserve_attempt(a)?;self.launch_database().execute("UPDATE launch_attempts SET body=? WHERE epoch=? AND id=?",&[&store::canonical(a),s(a,"epoch")?,s(a,"attemptId")?])}
    pub fn launch_ready(&self,lock:&RootLock,lease:&RootOwnerLease,epoch:u64,id:&str,expected_record:u64,plan:PreparationPlan,operation_id:&str)->Result<LiveReview,NativeError>{
        live_owner(self,lock,lease,epoch)?;let mut a=self.required_attempt(epoch,id)?;require_revision(&a,expected_record)?;require_settings(self,&a)?;
        let b=&a["beginInput"]["params"];
        let mode=match plan.mode(){PreparationMode::Modded=>"modded",PreparationMode::Vanilla=>"vanilla"};
        if a["state"]!="admitted" || plan.root()!=self.root() || plan.profile_id()!=s(b,"profileId")? || plan.expected_revision()!=u(b,"expectedRevision")? || b["mode"]!=mode{return Err(r::invalid());}
        if self.operation(operation_id)?["status"]!="unknown" || self.journal(operation_id)?.is_some(){return Err(conflict());}
        let input=plan.operation_input(operation_id)?;let changed=!matches!(plan,PreparationPlan::Verify(_));
        // A Verify review states actual work. Preserve all existing warnings, while
        // excluding prospective Apply copy counts and generation/mount projections.
        let summary=if changed{plan.summary().clone()}else{object!{profileId:plan.profile_id(),gamePath:plan.game().to_str().ok_or_else(r::invalid)?,mode:mode,
            physicalFileCount:0,totalBytes:0,replacedMounts:JsonValue::new_array(),installedMounts:JsonValue::new_array(),
            retainedMounts:crate::generation::mounts(&self.document()?["applied"]["manifest"] )?,warnings:plan.summary()["warnings"].clone(),fontsPreserved:true}};
        let ready=object!{reviewToken:plan.token(),preparationOperationId:operation_id,preparationInput:input,changed:changed,summary:summary};
        a["ready"]=ready.clone();a["state"]="ready".into();bump(&mut a)?;
        // Reserve the complete commit/preparation/status before any preparation can run.
        let mut widest=a.clone();widest["state"]="preparing".into();widest["commitInput"]=commit_input(epoch,id,&ready,s(&a["beginInput"]["params"],"settingsRevision")?);
        widest["preparation"]=object!{operationId:operation_id,input:ready["preparationInput"].clone(),resultingRevision:u64::MAX.to_string(),changed:changed,proofSha256:"f".repeat(64),gameinfoSha256:"f".repeat(64)};
        widest["recordRevision"]=u64::MAX.to_string().into();widest["handoff"]=object!{journalId:"x".repeat(64),stage:"dispatchIntent"};widest["outcome"]=object!{status:"cancelled",reason:"steamIntentCancelled"};bounded(&widest,CONTROL_LIMIT-1024)?;
        self.save_attempt(&a)?;Ok(LiveReview{epoch,id:id.into(),revision:u(&a,"recordRevision")?,owner:lease.token.clone(),plan,ready})
    }
    /// Exact duplicate commit queries existing facts without consuming or minting work.
    pub fn launch_commit_replay(&self,lock:&RootLock,lease:&RootOwnerLease,input:&JsonValue)->Result<Option<JsonValue>,NativeError>{
        r::commit(input)?;let p=&input["params"];live_owner(self,lock,lease,u(p,"epoch")?)?;
        let a=self.required_attempt(u(p,"epoch")?,s(p,"attemptId")?)?;
        if a["commitInput"].is_null(){return Ok(None);}if a["commitInput"]!=*input{return Err(conflict());}Ok(Some(r::query(&a)?))
    }
    pub fn launch_commit(&self,lock:&RootLock,lease:&RootOwnerLease,review:LiveReview,input:&JsonValue)->Result<CommitAdmission,NativeError>{
        if let Some(q)=self.launch_commit_replay(lock,lease,input)?{return Ok(CommitAdmission::Replay(q));}
        bound_owner(self,lock,lease,review.epoch,&review.owner)?;
        let mut a=self.required_attempt(review.epoch,&review.id)?;require_revision(&a,review.revision)?;require_settings(self,&a)?;
        if a["state"]!="ready" || a["ready"]!=review.ready || *input!=review.commit_input(&a)?{return Err(conflict());}
        a["commitInput"]=input.clone();a["state"]="preparing".into();bump(&mut a)?;self.save_attempt(&a)?;
        Ok(CommitAdmission::First(LiveCommit{review,revision:u(&a,"recordRevision")?}))
    }
    pub fn launch_finish_before_handoff(&self,lock:&RootLock,lease:&RootOwnerLease,epoch:u64,id:&str,expected_record:u64,reason:TerminalBeforeHandoff)->Result<JsonValue,NativeError>{
        live_owner(self,lock,lease,epoch)?;let mut a=self.required_attempt(epoch,id)?;require_revision(&a,expected_record)?;
        if !a["handoff"].is_null() || a["state"]=="terminal"{return Err(r::invalid());}
        a["outcome"]=match reason{TerminalBeforeHandoff::Cancelled=>object!{status:"cancelled",reason:"beforeDispatch"},TerminalBeforeHandoff::PreparationFailed=>object!{status:"failed",reason:"preparationFailed"}};
        a["state"]="terminal".into();bump(&mut a)?;self.save_attempt(&a)?;r::query(&a)
    }
}
fn commit_input(epoch:u64,id:&str,ready:&JsonValue,settings_revision:&str)->JsonValue{
    object!{method:"native.launch.commit",params:object!{epoch:epoch.to_string(),attemptId:id,reviewToken:ready["reviewToken"].clone(),preparationOperationId:ready["preparationOperationId"].clone(),preparationInput:ready["preparationInput"].clone(),settingsRevision:settings_revision}}
}
impl LiveReview {
    fn commit_input(&self,a:&JsonValue)->Result<JsonValue,NativeError>{Ok(commit_input(self.epoch,&self.id,&self.ready,s(&a["beginInput"]["params"],"settingsRevision")?))}
    pub fn ready(&self)->&JsonValue{&self.ready}
}
impl LiveCommit {
    /// Consumes the first admission and its actual opaque plan. Public PreparedSetup
    /// fields are never used as handoff authority.
    pub fn prepare(self,store:&Store,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,control:&PrepareControl,progress:&mut dyn FnMut(PreparationProgress))->Result<LivePrepared,NativeError>{
        let review=self.review;bound_owner(store,lock,lease,review.epoch,&review.owner)?;
        let mut a=store.required_attempt(review.epoch,&review.id)?;require_revision(&a,self.revision)?;require_settings(store,&a)?;
        if a["state"]!="preparing"{return Err(r::invalid());}
        let setup=review.plan.execute(store,lock,game,review.plan.expected_revision(),s(&review.ready,"preparationOperationId")?,&review.ready["preparationInput"],control,progress)?;
        let proof=setup.proof;let p=proof.evidence();r::proof(p)?;
        a["preparation"]=object!{operationId:review.ready["preparationOperationId"].clone(),input:review.ready["preparationInput"].clone(),resultingRevision:p["revision"].clone(),changed:review.ready["changed"].clone(),proofSha256:proof.sha256(),gameinfoSha256:p["selectedSources"]["gameinfo"]["sha256"].clone()};
        a["state"]="prepared".into();bump(&mut a)?;store.save_attempt(&a)?;
        Ok(LivePrepared{epoch:review.epoch,id:review.id,revision:u(&a,"recordRevision")?,owner:review.owner,proof})
    }
}
fn game_path(j:&JsonValue)->Result<PathBuf,NativeError>{Ok(PathBuf::from(s(&j["binding"],"gamePath")?))}
fn citadel(j:&JsonValue)->Result<PathBuf,NativeError>{Ok(game_path(j)?.join("game/citadel"))}
fn check_directories_for(j:&JsonValue,game:Option<&GameLock>)->Result<(),NativeError>{
    let b=&j["binding"];
    let root=PathBuf::from(s(b,"rootPath")?);
    paths::reject_symlinks(&root)?;
    let root_handle=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&root)?;
    for (p,k) in [(PathBuf::from(s(b,"rootPath")?),"rootIdentity"),(game_path(j)?,"gameIdentity"),(citadel(j)?,"citadelIdentity")] {
        paths::reject_symlinks(&p)?;let m=fs::symlink_metadata(&p)?;
        let id=&b[k];
        let matches=if k=="citadelIdentity" && game.is_some() && !id.has_key("fsid") {
            game.unwrap().validate(&p)?;
            let held=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&p)?;
            paths::same_persisted_object(&p,&held,&p,&held,id["device"].as_u64().ok_or_else(r::invalid)?,id["inode"].as_u64().ok_or_else(r::invalid)?,None)?
        } else if id.has_key("fsid") || k=="rootIdentity" && game.is_some() {
            let fsid=if id.has_key("fsid") {Some(id["fsid"].as_str().ok_or_else(r::invalid)?)} else {None};
            let held=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&p)?;
            paths::same_persisted_object(&root,&root_handle,&p,&held,id["device"].as_u64().ok_or_else(r::invalid)?,id["inode"].as_u64().ok_or_else(r::invalid)?,fsid)?
        } else {id["device"].as_u64()==Some(m.dev()) && id["inode"].as_u64()==Some(m.ino())};
        if !m.is_dir() || !matches{return Err(r::invalid());}
    } Ok(())
}
fn check_directories(j:&JsonValue)->Result<(),NativeError>{check_directories_for(j,None)}
fn pair_digest(j:&JsonValue)->Result<String,NativeError>{digest(&object!{reservation:j["reservation"].clone(),binding:j["binding"].clone(),proofSha256:j["proofSha256"].clone()})}
/// None means proven absence. A present file always requires captured identity;
/// only the temp may contain an exact prefix after interrupted write_all.
fn owned_image(j:&JsonValue,temp:bool,game:Option<&GameLock>)->Result<Option<JsonValue>,NativeError>{
    let c=citadel(j)?;let r=&j["reservation"];let name=if temp{s(r,"tempName")?}else{MARKER};let path=c.join(name);
    match fs::symlink_metadata(&path){Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(e.into()),Ok(m)=>{
        if !m.is_file(){return Err(r::invalid());}
    }}
    let mut f=paths::open_regular(&path)?;
    let identity=&r["identity"];
    let matches=|f:&File|->Result<bool,NativeError>{
        let m=f.metadata()?;
        if !m.is_file() || u(identity,"uid")?!=u64::from(m.uid()) {return Ok(false);}
        if identity.has_key("fsid") || game.is_some() {
            if let Some(guard)=game {guard.validate(&c)?;}
            let root=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&c)?;
            let fsid=if identity.has_key("fsid"){Some(s(identity,"fsid")?)}else{None};
            paths::same_persisted_object(&c,&root,&path,f,u(identity,"device")?,u(identity,"inode")?,fsid)
        } else {Ok(file_identity(&m)==*identity && fs::symlink_metadata(&path).is_ok_and(|named|file_identity(&named)==*identity))}
    };
    if !matches(&f)?{return Err(r::invalid());}
    use std::io::Read;
    let mut bytes=Vec::new();(&mut f).take((MARKER_LIMIT+1) as u64).read_to_end(&mut bytes)?;
    let expected=store::canonical(&r::marker(j));
    if bytes.len()>MARKER_LIMIT || (if temp{!expected.as_bytes().starts_with(&bytes)}else{expected.as_bytes()!=bytes}){return Err(r::invalid());}
    if !matches(&f)?{return Err(r::invalid());}
    Ok(Some(object!{size:bytes.len().to_string(),sha256:paths::digest_bytes(&bytes)?}))
}
fn complete_pair(j:&JsonValue)->Result<(),NativeError>{
    check_directories(j)?;
    if owned_image(j,false,None)?.is_none() || owned_image(j,true,None)?.is_some(){return Err(r::invalid());}Ok(())
}
fn cleanup_pair(j:&JsonValue,game:&GameLock)->Result<(),NativeError>{
    check_directories_for(j,Some(game))?;let temp=owned_image(j,true,Some(game))?;let final_image=owned_image(j,false,Some(game))?;
    if j["stage"]=="settling" {
        let x=&j["settlement"];
        if temp.as_ref().is_some_and(|v|*v!=x["tempImage"]) || final_image.is_some() && x["finalPresent"]!=true{return Err(r::invalid());}
    }else if j["stage"]=="reserved" {
        if temp.is_none() && final_image.is_none(){return Err(r::invalid());}
        // If both hard links exist the temp must also contain the complete bytes.
        if final_image.is_some() && temp.as_ref().is_some_and(|v|v["sha256"]!=j["reservation"]["bytesSha256"]){return Err(r::invalid());}
    }else if final_image.is_none() || temp.is_some(){return Err(r::invalid());}
    Ok(())
}
fn unlink_owned(j:&JsonValue,temp:bool,game:&GameLock)->Result<(),NativeError>{
    let image=owned_image(j,temp,Some(game))?;
    if let Some(image)=image{
        let x=&j["settlement"];
        if temp && image!=x["tempImage"] || !temp && x["finalPresent"]!=true{return Err(r::invalid());}
        let name=if temp{s(&j["reservation"],"tempName")?}else{MARKER};
        // Launch records encode numbers as strings; retain their original
        // admission strength when passing them to the shared file executor.
        let recorded=&j["reservation"]["identity"];
        let mut admitted=object!{device:u(recorded,"device")?,inode:u(recorded,"inode")?};
        if recorded.has_key("fsid"){admitted["fsid"]=recorded["fsid"].clone();}
        paths::DestructiveFile::admitted(citadel(j)?.join(name),&admitted,Some(s(&j["reservation"],"bytesSha256")?.to_owned()))?.unlink_required()?;
    }
    paths::sync_dir(&citadel(j)?)
}
fn reserve_handoff_budget(store:&Store,a:&JsonValue,j:&JsonValue)->Result<(),NativeError>{
    reserve_attempt(a)?;r::journal(j)?;
    let mut widest=j.clone();widest["recordRevision"]=u64::MAX.to_string().into();widest["stage"]="dispatchIntent".into();
    widest["dispatchEvidence"]=object!{bootId:"ffffffff-ffff-ffff-ffff-ffffffffffff",wallSeconds:u64::MAX.to_string(),bootMillis:u64::MAX.to_string()};
    widest["reservation"]["identity"]=object!{device:u64::MAX.to_string(),inode:u64::MAX.to_string(),uid:u32::MAX.to_string(),fsid:"f".repeat(16)};
    widest["settlement"]=object!{reason:"steamIntentCancelled",expectedRecordRevision:u64::MAX.to_string(),pairSha256:"f".repeat(64),steamIntentCancelled:false,
        tempImage:object!{size:u64::MAX.to_string(),sha256:"f".repeat(64)},finalPresent:false};
    store::ensure_internal(&object!{id:j["id"].clone(),body:widest.clone()})?;
    let doc=store.document()?;store::ensure_readable(&doc)?;
    let projection=r::projection(&widest);
    // Materialize the complete actual response in every profile, not a size delta
    // against an unrelated or transient recovery document.
    for (profile,_) in doc["profiles"].entries(){
        let mut snapshot=store::snapshot_document(&doc,Some(profile))?;
        snapshot["recoveryRequired"]=true.into();snapshot["launchBarrier"]=projection.clone();
        if crate::protocol::native_value_bytes(&snapshot).is_none(){return Err(NativeError::new("nativeResponseLimit","Launch barrier would exceed the readable snapshot budget."));}
    }
    Ok(())
}
fn boot_id()->Option<String>{
    let id=fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim().to_owned();
    if id.len()==36 && id.bytes().enumerate().all(|(i,c)|if [8,13,18,23].contains(&i){c==b'-'}else{c.is_ascii_hexdigit()}) {Some(id)}else{None}
}
fn boot_millis()->Option<u64>{
    #[repr(C)] struct Timespec {seconds:std::os::raw::c_long,nanos:std::os::raw::c_long}
    unsafe extern "C" {fn clock_gettime(clock:i32,time:*mut Timespec)->i32;}
    let mut time=Timespec{seconds:0,nanos:0};
    if unsafe{clock_gettime(7,&mut time)}!=0 || time.seconds<0 || !(0..1_000_000_000).contains(&time.nanos) {return None;}
    (time.seconds as u64).checked_mul(1000)?.checked_add(time.nanos as u64/1_000_000)
}
impl Store {
    fn save_pair(&self,a:&JsonValue,j:&JsonValue,insert:bool)->Result<(),NativeError>{
        reserve_attempt(a)?;r::journal(j)?;r::record_binding(j,a)?;
        let tx=self.launch_database().transaction()?;
        if insert {tx.db.execute("INSERT INTO journals(id,body) VALUES(?,?)",&[s(j,"id")?,&store::canonical(j)])?;}
        else {tx.db.execute("UPDATE journals SET body=? WHERE id=?",&[&store::canonical(j),s(j,"id")?])?;}
        tx.db.execute("UPDATE launch_attempts SET body=? WHERE epoch=? AND id=?",&[&store::canonical(a),s(a,"epoch")?,s(a,"attemptId")?])?;tx.commit()
    }
    fn admitted_pair(&self,journal_id:&str)->Result<(JsonValue,JsonValue),NativeError>{
        let journals=self.journals()?;
        if journals.len()!=1 || journals[0]["id"]!=journal_id{return Err(r::invalid());}
        let row=self.launch_database().query("SELECT body FROM journals WHERE id=?",&[journal_id])?.ok_or_else(r::invalid)?;
        let j=parse(&row[0])?;r::journal(&j)?;
        if j["id"]!=journal_id || s(&j["binding"],"rootPath")?!=self.root().to_str().ok_or_else(r::invalid)?{return Err(r::invalid());}
        let a=self.required_attempt(u(&j,"epoch")?,s(&j,"attemptId")?)?;r::record_binding(&j,&a)?;Ok((a,j))
    }
    /// Full live proof is consumed, never reconstructed from the stored journal.
    /// Caller retains RootLock -> GameLock from preparation through this call.
    pub fn reserve_handoff(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,prepared:LivePrepared,journal_id:&str)->Result<HandoffClaim,NativeError>{
        bound_owner(self,lock,lease,prepared.epoch,&prepared.owner)?;crate::apply::ensure_idle(self)?;
        let mut a=self.required_attempt(prepared.epoch,&prepared.id)?;require_revision(&a,prepared.revision)?;require_settings(self,&a)?;
        if a["state"]!="prepared" || !paths::safe_id(journal_id) || a["preparation"]["operationId"]==journal_id || self.operation(journal_id)?["status"]!="unknown"{return Err(conflict());}
        let p=prepared.proof.evidence();let b=object!{rootPath:p["rootPath"].clone(),gamePath:p["gamePath"].clone(),rootIdentity:p["directories"]["root"].clone(),gameIdentity:p["directories"]["game"].clone(),citadelIdentity:p["directories"]["citadel"].clone(),
            profileId:p["profileId"].clone(),mode:p["mode"].clone(),resultingRevision:p["revision"].clone(),settingsRevision:a["beginInput"]["params"]["settingsRevision"].clone(),settingsSha256:a["settingsSha256"].clone(),
            preparationOperationId:a["preparation"]["operationId"].clone(),preparationInputSha256:digest(&a["preparation"]["input"])?,gameinfoSha256:p["selectedSources"]["gameinfo"]["sha256"].clone()};
        let c=PathBuf::from(s(&b,"gamePath")?).join("game/citadel");game.validate(&c)?;paths::verify_absent(&c.join(MARKER))?;
        bump(&mut a)?;a["state"]="handoff".into();a["handoff"]=object!{journalId:journal_id,stage:"reserved"};
        let temp=format!(".deadlock-native-launch-{}.tmp",paths::random_id()?);
        let mut j=object!{kind:"launchHandoff",version:1,id:journal_id,epoch:prepared.epoch.to_string(),attemptId:prepared.id.clone(),recordRevision:a["recordRevision"].clone(),stage:"reserved",binding:b,preparedProof:p.clone(),proofSha256:prepared.proof.sha256(),
            reservation:object!{tempName:temp.clone(),finalName:MARKER,identity:object!{device:u64::MAX.to_string(),inode:u64::MAX.to_string(),uid:u32::MAX.to_string(),fsid:"f".repeat(16)},bytesSha256:"f".repeat(64)},settlement:JsonValue::Null};
        j["reservation"]["bytesSha256"]=digest(&r::marker(&j))?.into();
        r::current_binding(self,&j,&a,&self.document()?)?;reserve_handoff_budget(self,&a,&j)?;
        // No marker exists before the complete widest record/status/response budget.
        // A crash before DB capture leaves an unrecorded temp: never adopt it.
        let mut f=OpenOptions::new().write(true).read(true).create_new(true).mode(0o600).custom_flags(0x20000).open(c.join(&temp))?;
        let mut identity=file_identity(&f.metadata()?);if !f.metadata()?.is_file(){return Err(r::invalid());}
        identity["fsid"]=paths::persisted_filesystem_id(&f)?.into();
        j["reservation"]["identity"]=identity;
        self.save_pair(&a,&j,true)?; // DB reservation precedes every write/publication.
        f.write_all(store::canonical(&r::marker(&j)).as_bytes())?;f.set_permissions(fs::Permissions::from_mode(0o400))?;f.sync_all()?;
        check_directories(&j)?;game.validate(&c)?;
        if owned_image(&j,true,None)?.is_none_or(|v|v["sha256"]!=j["reservation"]["bytesSha256"]){return Err(r::invalid());}
        fs::hard_link(c.join(&temp),c.join(MARKER))?; // no replacement, immutable complete bytes
        if owned_image(&j,false,None)?.is_none(){return Err(r::invalid());}
        // This is publication cleanup, whose exact captured temp is already durable.
        if owned_image(&j,true,None)?.is_none_or(|v|v["sha256"]!=j["reservation"]["bytesSha256"]){return Err(r::invalid());}
        fs::remove_file(c.join(&temp))?;paths::sync_dir(&c)?;complete_pair(&j)?;
        bump(&mut a)?;a["handoff"]["stage"]="armed".into();j["recordRevision"]=a["recordRevision"].clone();j["stage"]="armed".into();self.save_pair(&a,&j,false)?;
        Ok(HandoffClaim{root:self.root().into(),epoch:prepared.epoch,id:prepared.id,journal:journal_id.into(),revision:u(&a,"recordRevision")?,owner:prepared.owner,dispatched:false,proof:prepared.proof})
    }
    /// Exact JSON status only; barriers survive owner replacement and old epochs.
    pub fn launch_barriers(&self)->Result<JsonValue,NativeError>{
        let journals=self.journals()?;let mut rows=JsonValue::new_array();
        for e in journals.members().filter(|e|e["body"]["kind"]=="launchHandoff"){
            let (a,j)=self.admitted_pair(s(e,"id")?)?;
            let legacy=j["stage"]=="dispatchIntent" && !j.has_key("dispatchEvidence");
            let marker_time=if legacy {citadel(&j).ok().and_then(|c|fs::symlink_metadata(c.join(MARKER)).ok()).map(|m|m.mtime()).filter(|t|*t>0)} else {None};
            rows.push(object!{version:1,rootPath:self.root().to_str().ok_or_else(r::invalid)?,journalId:j["id"].clone(),epoch:j["epoch"].clone(),attemptId:j["attemptId"].clone(),recordRevision:a["recordRevision"].clone(),stage:j["stage"].clone(),pairSha256:pair_digest(&j)?,legacy:legacy,markerTime:marker_time.map(JsonValue::from).unwrap_or(JsonValue::Null)}).map_err(|_|r::invalid())?;
        }
        let result=object!{version:1,barriers:rows};bounded(&result,CONTROL_LIMIT-256)?;Ok(result)
    }
    pub(crate) fn launch_snapshot_projection(&self,doc:&JsonValue)->Result<Option<JsonValue>,NativeError>{
        if doc["recoveryRequired"]!=false{return Ok(None);}
        let journals=self.journals()?;
        if journals.len()!=1 || journals[0]["body"]["kind"]!="launchHandoff"{return Ok(None);}
        let (a,j)=self.admitted_pair(s(&journals[0],"id")?)?;r::current_binding(self,&j,&a,doc)?;
        check_directories(&j)?;
        // A DB-only or partially published reservation cannot confirm preparation.
        if j["stage"]=="reserved" || j["stage"]=="settling"{return Ok(None);}complete_pair(&j)?;
        let p=r::projection(&j);bounded(&p,CONTROL_LIMIT-256)?;Ok(Some(p))
    }
    pub fn admit_dispatch(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,mut claim:HandoffClaim,idle:FreshIdle)->Result<DispatchPermit,NativeError>{
        bound_owner(self,lock,lease,claim.epoch,&claim.owner)?;
        let (mut a,mut j)=self.admitted_pair(&claim.journal)?;claim.validate_record(self,&a,&j)?;require_settings(self,&a)?;
        if claim.dispatched || j["stage"]!="armed"{return Err(r::invalid());}
        game.validate(&citadel(&j)?)?;complete_pair(&j)?;
        r::current_binding(self,&j,&a,&self.document()?)?;idle.check(&game_path(&j)?)?;
        claim.proof.revalidate(self,lock,game)?;
        idle.check(&game_path(&j)?)?;
        bump(&mut a)?;a["handoff"]["stage"]="dispatchIntent".into();j["stage"]="dispatchIntent".into();j["recordRevision"]=a["recordRevision"].clone();
        // Persist a boot identity before the syscall so a crash after spawn can
        // never make this new record look like a legacy handoff.
        j["dispatchEvidence"]=object!{bootId:boot_id().map(JsonValue::from).unwrap_or(JsonValue::Null),wallSeconds:JsonValue::Null,bootMillis:JsonValue::Null};
        self.save_pair(&a,&j,false)?;
        claim.dispatched=true;claim.revision=u(&a,"recordRevision")?;
        // Coordinator must serialize cancellation with this one-shot call. Once
        // returned, any lost/unknown syscall result is durable dispatch uncertainty.
        Ok(DispatchPermit{claim})
    }
    /// Record evidence only after the spawn syscall succeeded. Missing evidence
    /// remains conservative if the process exits before this durable write.
    pub(crate) fn record_dispatch(&self,lock:&RootLock,lease:&RootOwnerLease,permit:&DispatchPermit)->Result<(),NativeError>{
        bound_owner(self,lock,lease,permit.claim.epoch,&permit.claim.owner)?;
        let (a,mut j)=self.admitted_pair(&permit.claim.journal)?;
        permit.claim.validate_record(self,&a,&j)?;
        if j["stage"]!="dispatchIntent" {return Err(r::invalid());}
        let id=boot_id().ok_or_else(r::invalid)?;
        if !j["dispatchEvidence"]["bootId"].is_null() && j["dispatchEvidence"]["bootId"]!=id {return Err(r::invalid());}
        let ticks=boot_millis().ok_or_else(r::invalid)?;
        let wall=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|r::invalid())?.as_secs();
        j["dispatchEvidence"]=object!{bootId:id,wallSeconds:wall.to_string(),bootMillis:ticks.to_string()};
        self.save_pair(&a,&j,false)
    }
    /// Claim from admitted history grants only exact cleanup access, never spawn.
    pub fn settlement_claim(&self,lock:&RootLock,lease:&RootOwnerLease,journal_id:&str,expected_record:u64,pair_sha256:&str)->Result<SettlementClaim,NativeError>{
        let epoch=lease.epoch().ok_or_else(stale)?;live_owner(self,lock,lease,epoch)?;
        let (a,j)=self.admitted_pair(journal_id)?;require_revision(&a,expected_record)?;
        if pair_digest(&j)?!=pair_sha256{return Err(conflict());}
        Ok(SettlementClaim{root:self.root().into(),journal:j})
    }
    /// Fresh complete idle scan + explicit acknowledgement are required for
    /// dispatch uncertainty. Idle alone cannot disprove delayed Steam requests.
    pub fn resolve_launch(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle,steam_intent_cancelled:bool)->Result<JsonValue,NativeError>{
        if !steam_intent_cancelled{return Err(NativeError::new("steamCancellationRequired","Acknowledge cancellation of pending Steam launch intent before releasing this reservation."));}
        self.settle(lock,lease,game,claim,idle,"steamIntentCancelled")
    }
    /// A previous boot drops pending Steam requests; on this boot only elapsed
    /// CLOCK_BOOTTIME since successful spawn can prove the timeout.
    pub(crate) fn handoff_expired(&self,journal_id:&str)->Result<bool,NativeError>{
        let (_,j)=self.admitted_pair(journal_id)?;
        if j["stage"]!="dispatchIntent" {return Ok(false);}
        complete_pair(&j)?;
        if j.has_key("dispatchEvidence") {
            let evidence=&j["dispatchEvidence"];
            let Some(current_boot)=boot_id() else{return Ok(false);};
            let Some(dispatch_boot)=evidence["bootId"].as_str() else{return Ok(false);};
            if dispatch_boot!=current_boot {return Ok(true);}
            if evidence["bootMillis"].is_null() {return Ok(false);}
            let Some(now)=boot_millis() else{return Ok(false);};
            return Ok(now.checked_sub(u(evidence,"bootMillis")?).is_some_and(|elapsed|elapsed>=180_000));
        }
        Ok(false)
    }
    /// A restarted observer can associate a game only with a completed spawn
    /// timestamp from this boot; legacy and incomplete records lack that proof.
    pub(crate) fn handoff_can_observe(&self,journal_id:&str)->Result<bool,NativeError>{
        let (_,j)=self.admitted_pair(journal_id)?;
        if j["stage"]!="dispatchIntent" || !j.has_key("dispatchEvidence") {return Ok(false);}
        let e=&j["dispatchEvidence"];
        let (Some(id),Some(now))=(boot_id(),boot_millis()) else{return Ok(false);};
        if e["bootId"]!=id || e["bootMillis"].is_null() {return Ok(false);}
        Ok(now>=u(e,"bootMillis")?)
    }
    pub(crate) fn settle_timed_out(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle)->Result<JsonValue,NativeError>{
        if !self.handoff_expired(s(&claim.journal,"id")?)? {return Err(r::invalid());}
        self.settle(lock,lease,game,claim,idle,"startTimedOut")
    }
    pub(crate) fn settle_reobserved(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle)->Result<JsonValue,NativeError>{
        if claim.journal["stage"]!="dispatchIntent" || !self.handoff_can_observe(s(&claim.journal,"id")?)? {return Err(r::invalid());}
        self.settle(lock,lease,game,claim,idle,"gameCycleCompleted")
    }
    /// An intact durable reserved/armed stage proves no dispatch admission.
    pub fn settle_before_dispatch(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle)->Result<JsonValue,NativeError>{
        if !matches!(s(&claim.journal,"stage")?,"reserved"|"armed") && !(claim.journal["stage"]=="settling" && claim.journal["settlement"]["reason"]=="beforeDispatch"){return Err(r::invalid());}
        self.settle(lock,lease,game,claim,idle,"beforeDispatch")
    }
    pub(crate) fn settle_failed_before_dispatch(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle)->Result<JsonValue,NativeError>{
        if !matches!(s(&claim.journal,"stage")?,"reserved"|"armed") {return Err(r::invalid());}
        self.settle(lock,lease,game,claim,idle,"preDispatchFailed")
    }
    /// Resume only a previously durable cleanup reason; cannot synthesize it.
    pub fn resume_settlement(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle)->Result<JsonValue,NativeError>{
        if claim.journal["stage"]!="settling"{return Err(r::invalid());}
        let reason=s(&claim.journal["settlement"],"reason")?.to_owned();self.settle(lock,lease,game,claim,idle,&reason)
    }
    fn settle(&self,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,claim:SettlementClaim,idle:FreshIdle,reason:&str)->Result<JsonValue,NativeError>{
        live_owner(self,lock,lease,lease.epoch().ok_or_else(stale)?)?;
        if claim.root!=self.root(){return Err(r::invalid());}
        let (mut a,mut j)=self.admitted_pair(s(&claim.journal,"id")?)?;
        if j!=claim.journal{return Err(NativeError::new("staleLaunchRecord","Launch cleanup state changed; refresh the exact barrier pair."));}
        game.validate(&citadel(&j)?)?;idle.check(&game_path(&j)?)?;cleanup_pair(&j,game)?;
        let terminal_revision=next(if j["stage"]=="settling" {u(&a,"recordRevision")?}else{next(u(&a,"recordRevision")?)?})?;
        let status=match reason{"beforeDispatch"=>"cancelled","spawnFailed"|"preDispatchFailed"=>"failed","gameCycleCompleted"=>"observed","steamIntentCancelled"|"startTimedOut"=>"resolved",_=>return Err(r::invalid())};
        let mut terminal=a.clone();terminal["recordRevision"]=terminal_revision.to_string().into();terminal["state"]="terminal".into();terminal["handoff"]["stage"]="settled".into();
        terminal["outcome"]=object!{status:status,reason:reason};reserve_attempt(&terminal)?;r::query(&terminal)?;
        if j["stage"]!="settling"{
            let temp=owned_image(&j,true,Some(game))?.unwrap_or(JsonValue::Null);let present=owned_image(&j,false,Some(game))?.is_some();
            let x=object!{reason:reason,expectedRecordRevision:j["recordRevision"].clone(),pairSha256:pair_digest(&j)?,steamIntentCancelled:reason=="steamIntentCancelled",tempImage:temp,finalPresent:present};
            bump(&mut a)?;a["handoff"]["stage"]="settling".into();j["recordRevision"]=a["recordRevision"].clone();j["stage"]="settling".into();j["settlement"]=x;
            // Acknowledgement/observation reason and exact partial images become
            // durable before either unlink; absence is legal only after this write.
            self.save_pair(&a,&j,false)?;
        }else if j["settlement"]["reason"]!=reason{return Err(conflict());}
        cleanup_pair(&j,game)?;unlink_owned(&j,true,game)?;unlink_owned(&j,false,game)?;
        if owned_image(&j,true,Some(game))?.is_some() || owned_image(&j,false,Some(game))?.is_some(){return Err(r::invalid());}
        a=terminal;
        let tx=self.launch_database().transaction()?;
        tx.db.execute("UPDATE launch_attempts SET body=? WHERE epoch=? AND id=?",&[&store::canonical(&a),s(&a,"epoch")?,s(&a,"attemptId")?])?;
        tx.db.execute("DELETE FROM journals WHERE id=?",&[s(&j,"id")?])?;tx.commit()?;r::query(&a)
    }
}
impl HandoffClaim {
    fn validate_record(&self,store:&Store,a:&JsonValue,j:&JsonValue)->Result<(),NativeError>{
        if self.root!=store.root() || a["epoch"]!=self.epoch.to_string() || a["attemptId"]!=self.id || j["id"]!=self.journal{return Err(r::invalid());}require_revision(a,self.revision)
    }
    pub(crate) fn validate_game(&self,store:&Store,c:&Path)->Result<(),NativeError>{
        let (a,j)=store.admitted_pair(&self.journal)?;self.validate_record(store,&a,&j)?;
        if citadel(&j)?!=c{return Err(r::invalid());}complete_pair(&j)
    }
}
impl SettlementClaim {
    pub(crate) fn validate_game(&self,store:&Store,game:&GameLock)->Result<(),NativeError>{
        if self.root!=store.root(){return Err(r::invalid());}
        let (_,j)=store.admitted_pair(s(&self.journal,"id")?)?;
        game.validate(&citadel(&self.journal)?)?;
        if j!=self.journal{return Err(r::invalid());}cleanup_pair(&self.journal,game)
    }
}
impl DispatchPermit {
    pub fn claim(&self)->&HandoffClaim{&self.claim}
    /// Restricted to the later trusted launcher after a definitive spawn failure.
    /// A timeout, child exit, EOF or missing game observation is not no-spawn proof.
    #[allow(dead_code)]
    pub(crate) fn settle_spawn_failed(self,store:&Store,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,idle:FreshIdle)->Result<JsonValue,NativeError>{
        let (a,j)=store.admitted_pair(&self.claim.journal)?;self.claim.validate_record(store,&a,&j)?;
        store.settle(lock,lease,game,SettlementClaim{root:self.claim.root,journal:j},idle,"spawnFailed")
    }
    pub fn settle_game_cycle(self,store:&Store,lock:&RootLock,lease:&RootOwnerLease,game:&GameLock,idle:FreshIdle,cycle:ObservedGameCycle)->Result<JsonValue,NativeError>{
        let (a,j)=store.admitted_pair(&self.claim.journal)?;self.claim.validate_record(store,&a,&j)?;
        if cycle.game!=game_path(&j)? || cycle.observed.elapsed()>Duration::from_secs(2) || cycle.epoch!=self.claim.epoch
            || cycle.attempt!=self.claim.id || cycle.journal!=self.claim.journal || cycle.dispatch_revision!=self.claim.revision {return Err(r::invalid());}
        store.settle(lock,lease,game,SettlementClaim{root:self.claim.root,journal:j},idle,"gameCycleCompleted")
    }
}

#[cfg(test)]
#[test]
fn item93_launch_settlement_checks_filesystem_identity() {
    let f=crate::test_support::Fixture::new();
    let _lock=RootLock::acquire(&f.data).unwrap();
    let game=GameLock::acquire(&f.game.join("game/citadel")).unwrap();
    let identity=|p:&Path|{let m=fs::metadata(p).unwrap();object!{device:m.dev(),inode:m.ino()}};
    let mut root=identity(&f.data);
    root["fsid"]=paths::persisted_filesystem_id(&File::open(&f.data).unwrap()).unwrap().into();
    root["device"]=(fs::metadata(&f.data).unwrap().dev()+1).into();
    let mut journal=object!{binding:object!{rootPath:f.data.to_str().unwrap(),gamePath:f.game.to_str().unwrap(),rootIdentity:root,gameIdentity:identity(&f.game),citadelIdentity:identity(&f.game.join("game/citadel"))}};
    assert!(check_directories_for(&journal,Some(&game)).is_ok());
    journal["binding"]["rootIdentity"]["fsid"]="0000000000000001".into();
    assert!(check_directories_for(&journal,Some(&game)).is_err(),"a forged filesystem id must not authorize settlement");
    journal["binding"]["rootIdentity"]["fsid"]=paths::persisted_filesystem_id(&File::open(&f.data).unwrap()).unwrap().into();
    for (key, path) in [("gameIdentity",f.game.clone()),("citadelIdentity",f.game.join("game/citadel"))] {
        journal["binding"][key]["fsid"]=paths::persisted_filesystem_id(&File::open(&path).unwrap()).unwrap().into();
        journal["binding"][key]["device"]=(fs::metadata(&path).unwrap().dev()+1).into();
    }
    assert!(check_directories_for(&journal,Some(&game)).is_ok());
    journal["binding"]["citadelIdentity"]["fsid"]="0000000000000001".into();
    assert!(check_directories_for(&journal,Some(&game)).is_err());
    if paths::test_volatile_filesystem(&File::open(f.game.join("game/citadel")).unwrap()) {
        journal["binding"]["citadelIdentity"].remove("fsid");
        assert!(check_directories_for(&journal,Some(&game)).is_ok());
        assert!(check_directories_for(&journal,None).is_err());
    }
}

#[cfg(test)]
#[test]
fn item93_launch_marker_uses_stable_filesystem_identity() {
    let f=crate::test_support::Fixture::new();
    let c=f.game.join("game/citadel");
    let _guard=GameLock::acquire(&c).unwrap();
    let path=c.join(MARKER);
    let mut j=object!{id:"handoff",epoch:"1",attemptId:"attempt",binding:object!{gamePath:f.game.to_str().unwrap()},proofSha256:"proof",reservation:object!{tempName:"temp"}};
    std::fs::write(&path,store::canonical(&r::marker(&j))).unwrap();
    let file=paths::open_regular(&path).unwrap();let m=file.metadata().unwrap();
    j["reservation"]["identity"]=object!{device:(m.dev()+1).to_string(),inode:m.ino().to_string(),uid:m.uid().to_string(),fsid:paths::persisted_filesystem_id(&file).unwrap()};
    assert!(owned_image(&j,false,None).unwrap().is_some());
    j["reservation"]["identity"]["fsid"]="0000000000000001".into();
    assert!(owned_image(&j,false,None).is_err());
    if paths::test_volatile_filesystem(&File::open(&c).unwrap()) {
        j["reservation"]["identity"].remove("fsid");
        assert!(owned_image(&j,false,None).is_err(),"legacy device changes require the game lock");
        assert!(owned_image(&j,false,Some(&_guard)).unwrap().is_some(),"legacy marker inside the locked Citadel remains owned");
        j["reservation"]["identity"]["inode"]=(m.ino()+1).to_string().into();
        assert!(owned_image(&j,false,Some(&_guard)).is_err(),"a changed inode remains unowned");
        j["reservation"]["identity"]["inode"]=m.ino().to_string().into();
        j["reservation"]["identity"]["uid"]=(m.uid()+1).to_string().into();
        assert!(owned_image(&j,false,Some(&_guard)).is_err(),"a changed owner remains unowned");
    }
}

#[cfg(test)]
#[test]
fn item93_b6_launch_temp_requires_full_image_under_legacy_drift(){
    for drift in [true,false]{for fsid in [false,true]{for complete in [false,true]{
        let f=crate::test_support::Fixture::new();let _lock=RootLock::acquire(&f.data).unwrap();let c=f.game.join("game/citadel");let game=GameLock::acquire(&c).unwrap();
        if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){continue;}
        let path=c.join(".deadlock-native-launch-test.tmp");
        let mut j=object!{id:"handoff",epoch:"1",attemptId:"attempt",binding:object!{gamePath:f.game.to_str().unwrap()},proofSha256:"proof",reservation:object!{tempName:".deadlock-native-launch-test.tmp"}};
        let expected=store::canonical(&r::marker(&j));let bytes=if complete{expected.as_bytes()}else{&expected.as_bytes()[..expected.len()/2]};fs::write(&path,bytes).unwrap();
        let file=paths::open_regular(&path).unwrap();let m=file.metadata().unwrap();
        j["reservation"]["identity"]=object!{device:(m.dev()+u64::from(drift)).to_string(),inode:m.ino().to_string(),uid:m.uid().to_string()};
        if fsid{j["reservation"]["identity"]["fsid"]=paths::persisted_filesystem_id(&file).unwrap().into();}
        j["reservation"]["bytesSha256"]=paths::digest_bytes(expected.as_bytes()).unwrap().into();
        j["settlement"]=object!{tempImage:owned_image(&j,true,Some(&game)).unwrap().unwrap(),finalPresent:false};
        let result=unlink_owned(&j,true,&game);
        if drift && !fsid && !complete {
            assert!(path.exists(),"a prefix must not authorize legacy-drift deletion");assert_eq!(fs::read(&path).unwrap(),bytes);
            let error=result.unwrap_err();assert_eq!(error.kind,"recoveryRequired");assert!(error.message.contains("left in place"));
        }else{result.unwrap();assert!(!path.exists(),"full images, same-boot legacy and recorded fsid cleanup remain supported");}
    }}}
}
