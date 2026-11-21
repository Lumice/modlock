//! Native service. Upstream discovery remains read-only; all edits target the
//! distinct native store. Only implemented capabilities are advertised.
use crate::{
    apply::ApplyPlan,
    import::ImportPlan,
    local_content::{LocalMetadata, LocalPrepareRequest, LocalSelection},
    local_jobs::LocalJobs,
    paths,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{collections::HashMap, path::Path, sync::{Arc,Mutex}};
#[derive(Clone, Debug)]
pub struct NativeError {
    pub kind: String,
    pub message: String,
}
impl NativeError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
    pub fn params() -> Self {
        Self::new(
            "invalidParams",
            "Native parameters are missing, invalid, or exceed supported bounds.",
        )
    }
    pub fn not_imported() -> Self {
        Self::new(
            "nativeNotImported",
            "No native library has been imported; review an upstream import first.",
        )
    }
}
impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for NativeError {}
impl From<std::io::Error> for NativeError {
    fn from(error: std::io::Error) -> Self {
        let message=if error.raw_os_error()==Some(28) {"Not enough disk space (ENOSPC 28)."}
            else {"Cannot access native or source files; check paths, available storage and permissions."};
        Self::new("nativeIo",message)
    }
}
pub const CAPABILITIES: &[&str] = &[
    "library.snapshot",
    "native.snapshot",
    "native.importPreview",
    "native.importCommit",
    "native.setEnabled",
    "native.setFixEnabled",
    "native.setFixAutomatically",
    "native.bulkLibrary.v1",
    "native.bisect.v1",
    "native.health.v1",
    "native.maintenance.v1",
    "native.storage.v1",
    "native.hero.detect",
    "native.modUpdates.v1",
    "native.installedScan.v1",
    "native.installedGroupImport.v1",
    "native.setModsEnabled",
    "native.removeModsFromProfile",
    "native.deleteMods",
    "native.profile",
    "native.operation",
    "native.applyPreview",
    "native.apply",
    "native.restore",
    "native.recover",
    "native.local.prepare",
    "native.local.status",
    "native.local.review",
    "native.local.cancel",
    "native.local.dismiss",
    "native.local.commit",
    "native.local.archives.v1",
    "native.fonts.v1",
    "native.perfConfig.v1",
    "native.autoexec.v1", "native.crosshair.v1",
    "native.options.v1",
    "native.profileEdit.v1",
    "native.profileShare.v1",
    "native.deploymentShards.v1",
    "native.launch.v1",
    "native.catalog.v1",
    "native.libraryBootstrap.v1",
    "native.remotePayload.v1",
    "native.remoteReplace.v1",
];
#[derive(Default)]
pub struct NativeService {
    health: crate::health::Owner,
    config_reviews: HashMap<String, crate::configs::Review>,
    launch_owner: Option<crate::launch_owner::LaunchOwner>,
    catalog_owner: Option<crate::catalog::CatalogOwner>,
    remote_owner: Option<crate::remote::owner::RemoteOwner>,
    remote_recovery: Option<crate::remote::recovery::RecoveryWorker>,
    remote_recovery_envelope: bool,
    launch_pool: Option<Arc<crate::launch_owner::ChildPool>>,
    launch_monitor: Arc<Mutex<crate::launch_process::Monitor>>,
    launch_monitor_root: Option<std::path::PathBuf>,
    previews: HashMap<String, ImportPlan>,
    apply_previews: HashMap<String, ApplyPlan>,
    local_jobs: Option<LocalJobs>,
    font_preview: Option<(String, crate::fonts::FontPlan)>,
    profile_edit_preview: Option<(String, crate::profile_edit::ProfileEditPlan)>,
}
impl Drop for NativeService {
    fn drop(&mut self) {
        // Start catalog shutdown before existing launch-owner field retirement
        // joins its cooperative writer. No library initialization on EOF.
        if let Some(catalog) = &self.catalog_owner { catalog.request_close(); }
        if let Some(recovery) = &mut self.remote_recovery { recovery.retire(); }
        self.remote_recovery=None;
        if let Some(remote) = &mut self.remote_owner { remote.begin_close(); }
        if let Some(pool) = &self.launch_pool { pool.shutdown(); }
    }
}
fn required<'a>(p: &'a JsonValue, key: &str) -> Result<&'a str, NativeError> {
    p[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(NativeError::params)
}
fn optional<'a>(p: &'a JsonValue, key: &str) -> Result<Option<&'a str>, NativeError> {
    if !p.has_key(key) {
        Ok(None)
    } else {
        required(p, key).map(Some)
    }
}
fn mutation(p: &JsonValue) -> Result<(u64, &str), NativeError> {
    let revision = store::revision(required(p, "expectedRevision")?)?;
    let id = required(p, "operationId")?;
    if !paths::safe_id(id) {
        return Err(NativeError::params());
    }
    Ok((revision, id))
}
fn validate_mutation(method: &str, params: &JsonValue) -> Result<(), NativeError> {
    mutation(params)?;
    match method {
        "native.bisect.start" => {
            if params.len()!=5 || !paths::safe_id(required(params,"profileId")?) || !params["modIds"].is_array() || params["modIds"].is_empty() || params["modIds"].len()>256 {return Err(NativeError::params())}
            let mut ids=std::collections::BTreeSet::new();
            for id in params["modIds"].members(){let id=id.as_str().filter(|id|paths::safe_id(id)).ok_or_else(NativeError::params)?;if !ids.insert(id){return Err(NativeError::params())}}
        }
        "native.bisect.answer" => {if params.len()!=4 || !matches!(params["answer"].as_str(),Some("broken"|"fine"|"unsure"|"undo")){return Err(NativeError::params())}}
        "native.bisect.finish" => {if params.len()!=4 || !params["keepDisabled"].is_boolean(){return Err(NativeError::params())}}
        "native.library.initialize" => {
            crate::library_bootstrap::validate(method, params)?;
        }
        "native.profileEdit.commit" => {
            if params.len()!=5 || params.entries().any(|(k,_)|!["dataDir","operationId","expectedRevision","profileId","token"].contains(&k))
                || !paths::safe_id(required(params,"profileId")?) || !crate::package::hex(required(params,"token")?,32) {
                return Err(NativeError::params());
            }
        }
        "native.local.commit" | "native.options.attach" => {
            required(params, "jobId")?;
            required(params, "token")?;
            required(params, "profileId")?;
            if method=="native.options.attach" {required(params,"modId")?;}
        }
        "native.importCommit" | "native.apply" | "native.fonts.commit" => {
            required(params, "token")?;
        }
        "native.performance.select" => { if !matches!(params.len(),4|5) || params.entries().any(|(k,_)|!["dataDir","operationId","expectedRevision","presetId","profileId"].contains(&k)) || (params.has_key("profileId")&&!params["profileId"].is_string()) || !params["presetId"].is_string() {return Err(NativeError::params());} }
        "native.restore" | "native.recover" => {}
        "native.setModsEnabled" | "native.removeModsFromProfile" | "native.deleteMods" => {
            let toggle = method == "native.setModsEnabled";
            if params.len() != if toggle { 6 } else { 5 }
                || params.entries().any(|(k,_)| !["dataDir","profileId","modIds","enabled","expectedRevision","operationId"].contains(&k))
                || (!toggle && params.has_key("enabled"))
                || !paths::safe_id(required(params,"profileId")?)
                || !params["modIds"].is_array() || params["modIds"].is_empty() || params["modIds"].len()>256 {
                return Err(NativeError::params());
            }
            let mut ids = std::collections::BTreeSet::new();
            for id in params["modIds"].members() {
                let id = id.as_str().filter(|id| paths::safe_id(id)).ok_or_else(NativeError::params)?;
                if !ids.insert(id) { return Err(NativeError::params()); }
            }
            if toggle { params["enabled"].as_bool().ok_or_else(NativeError::params)?; }
        }
        "native.setEnabled" | "native.setFixEnabled" | "native.setFixAutomatically" => {
            if method != "native.setFixAutomatically" { required(params, "profileId")?; required(params, "modId")?; }
            params["enabled"]
                .as_bool()
                .ok_or_else(NativeError::params)?;
        }
        "native.profile" => match required(params, "action")? {
            "useGameFolder" => { required(params,"gamePath")?; }
            "create" => {
                profile_name(params)?;
            }
            "copy" | "rename" => {
                required(params, "profileId")?;
                profile_name(params)?;
            }
            "remove" => {
                required(params, "profileId")?;
            }
            _ => return Err(NativeError::params()),
        },
        _ => return Err(NativeError::params()),
    }
    Ok(())
}
pub fn operation_input(method: &str, p: &JsonValue) -> JsonValue {
    let mut params = p.clone();
    params.remove("dataDir");
    object! {method:method,params:params}
}
impl NativeService {
    /// This is memory-only and cannot create an owner or dispatch malformed work.
    pub(crate) fn revoke_remote_review_attempt(&mut self, params: &JsonValue) {
        let Ok((root,session,job))=crate::remote::contract::review_revocation_target(params) else{return;};
        if let Some(owner)=self.remote_owner.as_ref().filter(|owner|owner.root()==root&&owner.session()==&session){owner.revoke_review_attempt(&job,params);}
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub(crate) fn dispatch_recovery_envelope(&mut self,params:&JsonValue,strict:bool)->Result<JsonValue,NativeError>{
        self.remote_recovery_envelope=strict;let result=self.dispatch("native.recover",params);self.remote_recovery_envelope=false;result
    }
    pub(crate) fn retiring(&mut self) -> bool {
        if self.remote_recovery.as_mut().is_some_and(|w|!w.retirement_proven()){return false;}
        let remote_live=self.remote_owner.as_mut().is_some_and(|owner|!owner.is_closed());
        if !remote_live&&self.launch_owner.as_ref().is_some_and(|owner|owner.retiring()) {
            self.remote_recovery=None;return true;
        }
        false
    }
    fn remote_recovery_dispatch(&mut self,params:&JsonValue,root:&Path)->Result<Option<JsonValue>,NativeError>{
        let (_,id)=mutation(params)?;let input=operation_input("native.recover",params);
        let history=match Store::open_history(root){Ok(s)=>s,Err(e) if e.kind=="nativeNotImported"=>return Ok(None),Err(e)=>return Err(e)};
        if let Some(worker)=self.remote_recovery.as_mut(){
            if worker.root()!=root{return Err(NativeError::new("remoteBusy","Recovery retains another library root."));}
            if !self.remote_recovery_envelope{return Err(NativeError::params());}
            let attempt=crate::remote::contract::RemoteRecoveryInputV1::parse(input.clone())?;
            if worker.operation_id()==id&&!worker.matches(&attempt){return Err(NativeError::new("operationIdConflict","This recovery ID belongs to different inputs."));}
            if !worker.matches(&attempt){
                if let Some(result)=history.replay(id,&input)?{return Ok(Some(result));}
                return Err(NativeError::new("remoteBusy","Another remote recovery is still active."));
            }
            use crate::remote::recovery::RecoveryObservation;
            match worker.observe(){
                RecoveryObservation::Terminal(value)=>{self.remote_recovery=None;return Ok(Some(value));},
                RecoveryObservation::RejectedBeforeDispatch(error)=>{self.remote_recovery=None;return Err(error);},
                RecoveryObservation::Unknown=>worker.retry_unknown(),
                RecoveryObservation::Running=>{},
            }
            return Err(NativeError::new("outcomeUnknown","Recovery remains unconfirmed; retain and retry the same operation."));
        }
        let journals=history.journals()?;
        if !journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])){
            if self.remote_owner.as_ref().is_some_and(|r|r.has_admission()){return Err(NativeError::new("outcomeUnknown","A remote publication is still being reconciled."));}
            return Ok(None);
        }
        if !self.remote_recovery_envelope{return Err(NativeError::params());}
        let attempt=crate::remote::contract::RemoteRecoveryInputV1::parse(input)?;
        if journals.members().any(|e|crate::remote::recovery::is_attempt(&e["body"]["kind"])&&e["id"]!=attempt.operation_id()){
            return Err(NativeError::new("recoveryRequired","Retry the existing remote recovery operation ID."));
        }
        let mains=journals.members().filter(|e|crate::remote::recovery::is_main(&e["body"]["kind"])).collect::<Vec<_>>();
        if mains.len()!=1{return Err(NativeError::new("recoveryRequired","The remote journal set is incomplete."));}
        let target=crate::remote::contract::RemoteCommitInput::parse(mains[0]["body"]["input"].clone()).map_err(|_|NativeError::new("recoveryRequired","The remote journal input is ambiguous."))?;
        if mains[0]["id"]!=target.operation_id()||attempt.operation_id()==target.operation_id(){return Err(NativeError::new("operationIdConflict","Recovery needs a distinct operation ID."));}
        let recorded_attempt=journals.members().any(|e|e["id"]==attempt.operation_id());
        let custody=if let Some(owner)=self.remote_owner.as_mut().filter(|owner|owner.root()==root){
            if owner.is_closed(){None}else{
                Some(owner.recovery_custody(&target).ok_or_else(||NativeError::new("outcomeUnknown","The matching remote owner is still retiring its publication worker."))?)
            }
        }else{None};
        let worker=crate::remote::recovery::RecoveryWorker::start(root.to_path_buf(),attempt,target,journals,custody,recorded_attempt)?;
        self.remote_recovery=Some(worker);
        Err(NativeError::new("outcomeUnknown","Recovery is still running; retain and retry the same operation."))
    }
    pub fn dispatch(&mut self, method: &str, params: &JsonValue) -> Result<JsonValue, NativeError> {
        if !params.is_object() {
            return Err(NativeError::params());
        }
        if method == "native.hero.detect" { return crate::hero::dispatch(params); }
        if method == "native.modUpdates.get" { return crate::mod_updates::dispatch(params); }
        if matches!(method,"native.health.check"|"native.health.get") {return self.health.dispatch(method,params);}
        if method == "native.performance.inspect" {let bytes=paths::read_bounded(&std::path::Path::new(required(params,"gamePath")?).join("game/citadel/gameinfo.gi"),4*1024*1024)?;return crate::performance::detect(std::str::from_utf8(&bytes).map_err(|_|NativeError::params())?);}
        if method == "native.game.detect" { return Ok(crate::installed::detect(&std::path::PathBuf::from(std::env::var_os("HOME").ok_or_else(NativeError::params)?))); }
        if method == "native.installed.scan" {
            let game=std::path::PathBuf::from(required(params,"gamePath")?);
            let root=if params.has_key("dataDir") {Some(paths::data_root(params)?)}else{None};
            let source=params["sourcePath"].as_str().filter(|s|!s.is_empty()).map(std::path::PathBuf::from).or_else(||crate::discovery::discover().ok());
            return crate::installed::scan_with_source(&game,root.as_deref(),source.as_deref());
        }
        if method!="native.recover" && method!="native.recovery.inspect"
            && self.remote_recovery.as_mut().is_some_and(|w|w.retirement_proven()){self.remote_recovery=None;}
        if self.remote_recovery.as_ref().is_some_and(|w|paths::data_root(params).map_or(true,|root|w.root()!=root)) {
            return Err(NativeError::new("remoteBusy","Recovery retains another library root."));
        }
        // Recovery replay is independent of launch and transient owner state.
        if method=="native.recover" {
            let (_,id)=mutation(params)?;let root=paths::data_root(params)?;
            if self.remote_owner.as_mut().is_some_and(|r|r.root()!=root&&!r.is_closed()){return Err(NativeError::new("remoteBusy","Download work retains another library root."));}
            if self.launch_owner.as_ref().is_some_and(|l|l.root()!=root){return Err(NativeError::new("launchRootBusy","The launch owner retains another library root."));}
            match Store::open_history(&root){
                Ok(history)=>{
                    let journals=history.journals()?;
                    let identifiable=self.remote_recovery.is_some()||journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"]));
                    if identifiable&&!self.remote_recovery_envelope{return Err(NativeError::params());}
                    let input=operation_input(method,params);
                    let active_same_id=self.remote_recovery.as_ref().is_some_and(|w|w.operation_id()==id);
                    if active_same_id {
                        let attempt=crate::remote::contract::RemoteRecoveryInputV1::parse(input.clone())?;
                        if !self.remote_recovery.as_ref().unwrap().matches(&attempt){return Err(NativeError::new("operationIdConflict","This recovery ID belongs to different inputs."));}
                    }else if let Some(result)=history.replay(id,&input)?{return Ok(result);}
                },
                Err(e) if e.kind=="nativeNotImported"&&self.remote_recovery.is_none()=>{},
                Err(e)=>return Err(e),
            }
        }
        if method.starts_with("native.library.") {
            let root = crate::library_bootstrap::validate(method, params)?;
            if method == "native.library.initialize" { crate::library_bootstrap::safe_root(&root)?; }
        }
        // Catalog admission precedes launch write exclusion. It never opens the
        // native Store or takes RootLock, and all root IO belongs to its worker.
        if method.starts_with("native.catalog.") {
            let root = crate::catalog::contract::validate(method, params)?;
            if let Some(remote)=self.remote_owner.as_mut(){if remote.root()!=root&&!remote.is_closed(){return Err(NativeError::new("remoteBusy","Retire the remote owner before changing libraries."));}}
            if let Some(launch) = &self.launch_owner {
                if launch.root() != root { return Err(NativeError::new("launchRootBusy", "Catalog must use the launch owner's data root.")); }
            }
            if self.catalog_owner.is_none() {
                if method != "native.catalog.open" { return Err(crate::catalog::contract::error("catalogSessionMismatch")); }
                self.catalog_owner = Some(crate::catalog::CatalogOwner::new(root)?);
            }
            let catalog = self.catalog_owner.as_mut().unwrap();
            if let Some(launch) = &self.launch_owner { catalog.constrain_launch_root(launch.root())?; }
            return catalog.dispatch(method, params);
        }
        if method.starts_with("native.remote.") {
            if matches!(method,"native.remote.review"|"native.remote.replaceReview") {self.revoke_remote_review_attempt(params);}
            let root=crate::remote::contract::validate_request(method,params)?;
            if let Some(catalog)=&self.catalog_owner{catalog.constrain_launch_root(&root)?;}
            let session=crate::remote::contract::SessionId::parse(params["sessionId"].as_str().ok_or_else(NativeError::params)?)?;
            let pool=self.launch_pool.clone();let retire_old=self.launch_owner.as_mut().is_some_and(|l|l.root()!=root&&l.retiring()&&l.joined()&&!pool.as_ref().is_some_and(|p|p.retained_for(l.root())));
            if retire_old&&method=="native.remote.open"{self.launch_owner=None;}
            let launch_busy=if let Some(launch)=self.launch_owner.as_mut(){if launch.root()!=root{return Err(NativeError::new("launchRootBusy","Remote work must use the launch owner's data root."));}if launch.retiring(){!launch.joined()}else{launch.blocks_writes()}}else{false};
            let recovery_busy=self.remote_recovery.is_some();
            let replace=self.remote_owner.as_mut().is_some_and(|r|(r.root()!=root||r.session()!=&session)&&r.is_closed());
            if replace&&method=="native.remote.open"{self.remote_owner=None;}
            if self.remote_owner.is_none(){if method!="native.remote.open"{return Err(NativeError::new("remoteBusy","Open the remote owner first."));}self.remote_owner=Some(crate::remote::owner::RemoteOwner::open_gated(root.clone(),session,launch_busy||recovery_busy)?);}
            let remote=self.remote_owner.as_mut().unwrap();if remote.root()!=root||remote.session().as_str()!=params["sessionId"].as_str().unwrap(){return Err(NativeError::new("remoteBusy","Retire the current remote owner before changing its root or session."));}
            remote.set_launch_blocked(launch_busy||recovery_busy);
            if (launch_busy||recovery_busy)&&remote.is_new_gated(method,params){return Err(NativeError::new(if recovery_busy{"remoteBusy"}else{"launchBusy"},"The library is using its writer admission."));}
            let mut result=remote.dispatch(method,params);
            if method=="native.remote.close"&&self.remote_recovery.is_some() {if let Ok(value)=&mut result{value["phase"]="waiting".into();}}
            return result;
        }
        if method.starts_with("native.launch.") || method.starts_with("native.game.") {
            crate::launch_owner::validate(method, params)?;
            let root = paths::data_root(params)?;
            if let Some(catalog) = &self.catalog_owner { catalog.constrain_launch_root(&root)?; }
            if let Some(remote)=self.remote_owner.as_mut(){if remote.root()!=root&&!remote.is_closed(){return Err(NativeError::new("launchRootBusy","Retire the remote owner before changing libraries."));}}
            let launch_gated=matches!(method,"native.launch.open"|"native.launch.session"|"native.launch.settings.get"|"native.launch.settings.set"|"native.launch.barriers"|"native.launch.begin"|"native.launch.commit"|"native.launch.resolve");
            if launch_gated&&(self.remote_owner.as_ref().is_some_and(|r|r.has_admission())||self.remote_recovery.is_some())&&!self.launch_owner.as_ref().is_some_and(|l|l.has_control(params)){return Err(NativeError::new("remoteBusy","Remote admission must finish before new launch work."));}
            // A closed worker cannot service queued history reads. Recreate only
            // after join proves its lease is gone; this grants no launch intent.
            let reopen = matches!(method,"native.launch.open"|"native.launch.session"|"native.launch.settings.get"|"native.launch.settings.operation"|"native.launch.query"|"native.launch.barriers");
            if reopen&&self.launch_owner.as_mut().is_some_and(|l|l.retiring()&&l.joined()){
                if self.launch_monitor_root.as_ref().is_some_and(|old|old!=&root&&self.launch_pool.as_ref().is_some_and(|p|p.retained_for(old))){return Err(NativeError::new("launchRootBusy","A launcher from the previous library is still retained."));}
                let pool=self.launch_pool.as_ref().ok_or_else(||NativeError::new("launchBusy","Launcher ownership is unavailable."))?.clone();
                let replacement=crate::launch_owner::LaunchOwner::new(root.clone(),pool,self.launch_monitor.clone())?;
                self.launch_monitor_root=Some(root.clone());self.launch_owner=Some(replacement);
            }
            if self.launch_owner.is_none()&&matches!(method,"native.game.status"|"native.game.stop")&&self.launch_monitor_root.as_ref()==Some(&root){
                let mut monitor=self.launch_monitor.lock().unwrap_or_else(|p|p.into_inner());return if method=="native.game.status"{monitor.status()}else{monitor.stop(required(params,"sessionToken")?)};
            }
            if self.launch_owner.is_none() {
                if self.launch_monitor_root.as_ref().is_some_and(|old|old!=&root&&self.launch_pool.as_ref().is_some_and(|p|p.retained_for(old))){return Err(NativeError::new("launchRootBusy","A launcher from the previous library is still retained."));}
                if self.launch_pool.is_none(){self.launch_pool=Some(crate::launch_owner::ChildPool::new()?);}
                self.launch_monitor_root=Some(root.clone());
                self.launch_owner = Some(crate::launch_owner::LaunchOwner::new(root,self.launch_pool.as_ref().unwrap().clone(),self.launch_monitor.clone())?);
            }
            return self.launch_owner.as_mut().unwrap().dispatch(method, params);
        }
        // Ordinary native routes must honor the same retained remote-root
        // authority as the dedicated catalog, launch, and remote routes.
        if let Some(remote) = self.remote_owner.as_mut() {
            if remote.root() != paths::data_root(params)? && !remote.is_closed() {
                return Err(NativeError::new("remoteBusy", "Download work is still using the current library."));
            }
        }
        if method=="native.recovery.inspect" {
            crate::catalog::contract::exact(params,&["version","dataDir"])?;
            if params["version"]!=1 || !crate::catalog::contract::typed(params){return Err(NativeError::params());}
            let root=crate::catalog::contract::root(&params["dataDir"])?;
            if let Some(owner)=&self.launch_owner {if owner.root()!=root{return Err(NativeError::new("launchRootBusy","Recovery inspection requires the retained library root."));}}
            let mut result=object!{version:1,root:params["dataDir"].clone(),state:"waiting",expectedRevision:JsonValue::Null,
                targetOperationId:JsonValue::Null,targetInputSha256:JsonValue::Null,existingRecovery:JsonValue::Null,
                error:object!{kind:"recoveryRequired",message:"Recovery context cannot be safely inspected; preserve the existing evidence.",retryAfterSeconds:JsonValue::Null}};
            enum Inspection {NotRemote,Legacy(&'static str),Remote((String,String,String,Option<JsonValue>))}
            let inspected=(|| {
                let history=Store::open_history(&root)?;
                history.inspect_recovery(|history| {
                    let journals=history.journals()?;
                    if !journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])) {
                        let document=history.recovery_document()?;
                        crate::font_transaction::admit_set(history,&journals)?;
                        if document.is_none() {
                            if journals.members().any(|e|e["body"]["kind"]!="import"
                                || (e["body"].has_key("version") && !matches!(e["body"]["version"].as_u32(),Some(1|2|3|4|5)))) {
                                return Err(NativeError::new("recoveryRequired","The unfinished journal set is not an admitted legacy import."));
                            }
                            return Ok(Inspection::Legacy(if journals.len()==0{"legacyUnimported"}else{"legacyImportRecovery"}));
                        }
                        return Ok(Inspection::NotRemote);
                    }
                    crate::remote::recovery::inspect_context(history,&journals).map(Inspection::Remote)
                })
            })();
            match inspected {
                Ok(Inspection::NotRemote)=>{result["state"]="notRemote".into();result["error"]=JsonValue::Null;},
                Ok(Inspection::Legacy(state))=>{result["state"]=state.into();result["expectedRevision"]="0".into();result["error"]=JsonValue::Null;},
                Ok(Inspection::Remote((revision,target,digest,existing)))=>{
                    result["state"]="ready".into();result["expectedRevision"]=revision.into();
                    result["targetOperationId"]=target.into();result["targetInputSha256"]=digest.into();
                    result["existingRecovery"]=existing.unwrap_or(JsonValue::Null);result["error"]=JsonValue::Null;
                },
                Err(_)=>{},
            }
            return Ok(result);
        }
        if let Some(owner) = &self.launch_owner {
            if owner.root() != paths::data_root(params)? {
                return Err(NativeError::new("launchRootBusy", "Retire the launch owner before changing libraries."));
            }
            if owner.busy() && method == "native.snapshot" {
                return Err(NativeError::new("launchBusy", "Preparation is still running; poll launch status before refreshing the library."));
            }
            if owner.blocks_writes() && !matches!(method, "native.snapshot" | "native.operation" | "native.local.status" | "native.options.status" | "native.local.cancel" | "native.options.cancel") {
                return Err(NativeError::new("launchBusy", "The launch owner is using this library; reconcile or cancel its active attempt first."));
            }
        }
        if method=="native.recover" {if let Some(result)=self.remote_recovery_dispatch(params,&paths::data_root(params)?)?{return Ok(result);}}
        if self.remote_recovery.is_some()&&(method.starts_with("native.bisect.")||matches!(method,"native.library.initialize"|"native.importCommit"|"native.local.commit"|"native.options.attach"|"native.profileEdit.commit"|"native.fonts.commit"|"native.performance.select"|"native.setEnabled" | "native.setFixEnabled" | "native.setFixAutomatically"|"native.setModsEnabled"|"native.removeModsFromProfile"|"native.deleteMods"|"native.profile"|"native.apply"|"native.restore")){
            return Err(NativeError::new("remoteBusy","Remote recovery is still active."));
        }
        if method.starts_with("native.storage.") {
            if self.remote_recovery.is_some() { return Err(NativeError::new("remoteBusy", "Remote recovery is active.")); }
            return crate::storage::dispatch(method, params);
        }
        if method.starts_with("native.maintenance.") {
            if self.remote_recovery.is_some() { return Err(NativeError::new("remoteBusy", "Remote recovery is still active.")); }
            return crate::maintenance::dispatch(method, params);
        }
        if method.starts_with("native.profileShare.") {
            if self.remote_recovery.is_some() { return Err(NativeError::new("remoteBusy", "Remote recovery is still active.")); }
            return crate::profile_share::dispatch(method, params);
        }
        // Inspection is a read-only route, including when this session retains
        // a local-jobs owner. Do not run the shared writer cleanup afterward.
        if method == "native.library.inspect" {
            return self.dispatch_inner(method, params, None);
        }
        if method.starts_with("native.config.") {
            let root=paths::data_root(params)?;
            let action=method.strip_prefix("native.config.").unwrap();
            let kind=params["kind"].as_str().ok_or_else(NativeError::params)?;
            if !matches!(kind,"autoexec"|"crosshair"){return Err(NativeError::params())}
            let store=Store::open(&root)?;
            return match action {
                "get"=>crate::configs::view(&store,kind),
                "decode"=>crate::configs::decode(required(params,"code")?),
                "encode"=>Ok(crate::configs::encode(&params["values"])?.into()),
                "preview"=>{let (r,dto)=crate::configs::review(&store,kind,&params["input"],params["undo"]==true)?;if self.config_reviews.len()>=8{self.config_reviews.clear()}self.config_reviews.insert(r.token.clone(),r);Ok(dto)},
                "commit"=>{let token=required(params,"token")?;let r=self.config_reviews.get(token).ok_or_else(NativeError::params)?.clone();if r.body["config"]!=kind{return Err(NativeError::params())}let lock=RootLock::acquire(&root)?;let store=Store::open_writer(&lock)?;crate::configs::commit(&store,&lock,&r)?;self.config_reviews.remove(token);crate::configs::view(&store,kind)},
                _=>Err(NativeError::params())
            };
        }
        let mutating = method.starts_with("native.bisect.") && method!="native.bisect.get" || matches!(
            method,
            "native.library.initialize"
                | "native.importCommit"
                | "native.local.commit"
                | "native.options.attach"
                | "native.profileEdit.commit"
                | "native.fonts.commit"
                | "native.performance.select"
                | "native.setEnabled" | "native.setFixEnabled" | "native.setFixAutomatically"
                | "native.setModsEnabled"
                | "native.removeModsFromProfile"
                | "native.deleteMods"
                | "native.profile"
                | "native.apply"
                | "native.restore"
                | "native.recover"
        );
        let lock = if mutating {
            validate_mutation(method, params)?;
            Some(RootLock::acquire(&paths::data_root(params)?)?)
        } else {
            None
        };
        let result = self.dispatch_inner(method, params, lock.as_ref());
        if let (Some(lock), Err(error)) = (&lock, &result) {
            // Bootstrap rejection must never open/create/modify the final DB.
            // Its only durable receipt lives in a successfully published candidate.
            if method != "native.library.initialize" {
            if !matches!(
                error.kind.as_str(),
                "invalidParams"
                    | "operationIdConflict"
                    | "outcomeUnknown"
                    | "nativeBusy"
                    | "nativeDatabase"
                    | "nativeIo"
                    | "recoveryRequired"
                    | "localJobOutcomeUnknown"
                    | "unsupportedVersion"
            ) {
                let (_, id) = mutation(params)?;
                let history = Store::open_writer(lock)?;
                if history.journals()?.len() != 0 {
                    return result;
                }
                history.record_failure(lock, id, &operation_input(method, params), error)?;
            }
            }
        }
        if method=="native.recover"&&result.as_ref().is_err_and(|e|e.kind=="outcomeUnknown"){return result;}
        if let Ok(value) = &result {
            if let (Some(jobs), Some(revision)) = (&mut self.local_jobs, value["revision"].as_str())
            {
                if crate::local_jobs::canonical_root(&paths::data_root(params)?)? == jobs.root() {
                    if let Ok(revision) = store::revision(revision) {
                        jobs.note_revision(revision);
                    }
                }
            }
        }
        drop(lock);
        if let Some(jobs) = &mut self.local_jobs {
            // Private cleanup status cannot change the durable writer result.
            let _ = jobs.after_writer_cleanup();
        }
        result
    }
    fn dispatch_local(
        &mut self,
        method: &str,
        params: &JsonValue,
    ) -> Result<JsonValue, NativeError> {
        let allowed: &[&str] = match method {
            "native.local.prepare" => &[
                "dataDir",
                "jobId",
                "profileId",
                "expectedRevision",
                "roots",
                "artworkPath",
            ],
            "native.local.status" => &["dataDir", "jobId", "includeCandidates"],
            "native.local.review" => &["dataDir", "jobId", "candidateIds", "metadata", "groupRows"],
            "native.local.cancel" | "native.local.dismiss" => &["dataDir", "jobId"],
            _ => return Err(NativeError::new("methodNotFound", "Unknown native method.")),
        };
        if params.entries().any(|(key, _)| !allowed.contains(&key)) {
            return Err(NativeError::params());
        }
        let root = crate::local_jobs::canonical_root(&paths::data_root(params)?)?;
        if self
            .local_jobs
            .as_ref()
            .is_some_and(|jobs| jobs.root() != root)
        {
            return Err(NativeError::params());
        }
        let id = if method == "native.local.status" {
            optional(params, "jobId")?
        } else {
            Some(required(params, "jobId")?)
        };
        if let Some(id) = id {
            crate::local_jobs::job_id(id)?;
        }
        let include = if params.has_key("includeCandidates") {
            params["includeCandidates"]
                .as_bool()
                .ok_or_else(NativeError::params)?
        } else {
            false
        };
        if include && id.is_none() {
            return Err(NativeError::params());
        }
        let request = if method == "native.local.prepare" {
            if !params["roots"].is_array() {
                return Err(NativeError::params());
            }
            Some(LocalPrepareRequest {
                root: root.clone(),
                profile_id: required(params, "profileId")?.into(),
                expected_revision: store::revision(required(params, "expectedRevision")?)?,
                roots: params["roots"]
                    .members()
                    .map(|v| {
                        v.as_str()
                            .filter(|s| !s.is_empty())
                            .map(std::path::PathBuf::from)
                            .ok_or_else(NativeError::params)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                artwork: optional(params, "artworkPath")?.map(std::path::PathBuf::from),
            })
        } else {
            None
        };
        let selection = if method == "native.local.review" && !params.has_key("groupRows") {
            let m = &params["metadata"];
            if !params["candidateIds"].is_array()
                || !m.is_object()
                || m.len() != 5
                || m.entries().any(|(k, _)| {
                    !["name", "author", "category", "link", "description"].contains(&k)
                })
            {
                return Err(NativeError::params());
            }
            let field = |k: &str| {
                m[k].as_str()
                    .map(str::to_owned)
                    .ok_or_else(NativeError::params)
            };
            Some(LocalSelection {
                candidate_ids: params["candidateIds"]
                    .members()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(NativeError::params)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                metadata: LocalMetadata {
                    name: field("name")?,
                    author: field("author")?,
                    category: field("category")?,
                    link: field("link")?,
                    description: field("description")?,
                },
            })
        } else {
            None
        };
        if self.local_jobs.is_none() {
            self.local_jobs = LocalJobs::open(&root)?;
            if self.local_jobs.is_none() {
                if let Some(request) = &request {
                    self.local_jobs = Some(LocalJobs::create(request)?);
                } else if id.is_none() {
                    return Ok(json::array![]);
                } else {
                    return Err(NativeError::new(
                        "unknownLocalJob",
                        "This local job is unknown or was dismissed.",
                    ));
                }
            }
        }
        let jobs = self.local_jobs.as_mut().unwrap();
        match method {
            "native.local.prepare" => jobs.start(request.unwrap(), id.unwrap()),
            "native.local.status" => jobs.status(id, include),
            "native.local.review" => if params.has_key("groupRows") {jobs.review_group(id.unwrap(),&params["groupRows"])} else {jobs.review(id.unwrap(), selection.unwrap())},
            "native.local.cancel" => jobs.cancel(id.unwrap()),
            "native.local.dismiss" => jobs.dismiss(id.unwrap()),
            _ => unreachable!(),
        }
    }
    fn dispatch_options(&mut self, method: &str, params: &JsonValue) -> Result<JsonValue,NativeError> {
        use crate::local_content::option_sources::{self, OptionsPrepareRequest, OptionSource};
        use crate::local_jobs::JobKind;
        let allowed: &[&str]=match method {
            "native.options.get"=>&["dataDir","profileId","modId","expectedRevision"],
            "native.options.prepare"=>&["dataDir","profileId","modId","expectedRevision","jobId","source"],
            "native.options.review"=>&["dataDir","jobId","bindings"],
            "native.options.status"=>&["dataDir","jobId","includeCandidates"],
            "native.options.cancel"|"native.options.dismiss"=>&["dataDir","jobId"],
            _=>return Err(NativeError::new("methodNotFound","Unknown options method.")),
        };
        if params.entries().any(|(k,_)|!allowed.contains(&k)) {return Err(NativeError::params());}
        let root=crate::local_jobs::canonical_root(&paths::data_root(params)?)?;
        if method=="native.options.get" {
            let profile=required(params,"profileId")?;let mod_id=required(params,"modId")?;
            let expected=store::revision(required(params,"expectedRevision")?)?;
            let (record,layout)=option_sources::pinned_record(&root,profile,mod_id,expected)?;
            return option_sources::detail_envelope(&record,layout,profile,expected);
        }
        if self.local_jobs.as_ref().is_some_and(|j|j.root()!=root) {return Err(NativeError::params());}
        let id=if method=="native.options.status" {optional(params,"jobId")?}else{Some(required(params,"jobId")?)};
        if let Some(id)=id {crate::local_jobs::job_id(id)?;}
        let include=if params.has_key("includeCandidates") {params["includeCandidates"].as_bool().ok_or_else(NativeError::params)?}else{false};
        if include && id.is_none() {return Err(NativeError::params());}
        let request=if method=="native.options.prepare" {Some(OptionsPrepareRequest {root:root.clone(),profile_id:required(params,"profileId")?.into(),mod_id:required(params,"modId")?.into(),
            expected_revision:store::revision(required(params,"expectedRevision")?)?,source:OptionSource::decode(&params["source"])?})}else{None};
        let bindings=if method=="native.options.review" {Some(option_sources::decode_bindings(&params["bindings"])?)}else{None};
        if self.local_jobs.is_none() {
            self.local_jobs=LocalJobs::open(&root)?;
            if self.local_jobs.is_none() {
                if let Some(r)=&request {self.local_jobs=Some(LocalJobs::create_options(r)?);}
                else if id.is_none() {return Ok(json::array![]);}
                else {return Err(NativeError::new("unknownLocalJob","This options job is unknown or was dismissed."));}
            }
        }
        let jobs=self.local_jobs.as_mut().unwrap();
        match method {
            "native.options.prepare"=>jobs.start_options(request.unwrap(),id.unwrap()),
            "native.options.status"=>jobs.status_for(JobKind::Options,id,include),
            "native.options.review"=>jobs.review_options(id.unwrap(),bindings.unwrap()),
            "native.options.cancel"=>jobs.cancel_for(JobKind::Options,id.unwrap()),
            "native.options.dismiss"=>jobs.dismiss_for(JobKind::Options,id.unwrap()),
            _=>unreachable!(),
        }
    }
    fn dispatch_inner(
        &mut self,
        method: &str,
        params: &JsonValue,
        lock: Option<&RootLock>,
    ) -> Result<JsonValue, NativeError> {
        if !params.is_object() {
            return Err(NativeError::params());
        }
        let root = paths::data_root(params)?;
        if method=="native.deleteMods" {return crate::maintenance::delete_mods(params,lock.ok_or_else(NativeError::params)?);}
        if method.starts_with("native.local.") && method != "native.local.commit" {
            return self.dispatch_local(method, params);
        }
        if method.starts_with("native.options.") && method!="native.options.attach" {return self.dispatch_options(method,params);}
        match method {
            "native.library.inspect" => crate::library_bootstrap::BootstrapState::inspect(&root),
            "native.library.initialize" => crate::library_bootstrap::initialize(
                lock.ok_or_else(NativeError::params)?, required(params,"operationId")?, &operation_input(method,params)),
            "native.profileEdit.preview" => {
                let sharded_capacity=match params["capacityLayout"].as_str() {Some("shards-v1")=>true,None if !params.has_key("capacityLayout")=>false,_=>return Err(NativeError::params())};
                if params.len()!=(if sharded_capacity {5}else{4}) || params.entries().any(|(k,_)|!["dataDir","profileId","expectedRevision","change","capacityLayout"].contains(&k)) {return Err(NativeError::params());}
                let profile_id=required(params,"profileId")?;
                if !paths::safe_id(profile_id) {return Err(NativeError::params());}
                let expected=store::revision(required(params,"expectedRevision")?)?;
                let change=crate::profile_edit::ProfileEditChange::decode(&params["change"])?;
                let _lock=RootLock::acquire(&root)?;
                let store=Store::open(&root)?;
                // One replaceable bounded review, never a per-token map.
                self.profile_edit_preview=None;
                let plan=if sharded_capacity {crate::profile_edit::ProfileEditPlan::review_capacity(&store,profile_id,expected,change,true)?}
                    else {crate::profile_edit::ProfileEditPlan::review(&store,profile_id,expected,change)?};
                let token=paths::random_id()?;
                let response=object!{token:token.clone(),profileId:profile_id,expectedRevision:expected.to_string(),summary:plan.summary().clone()};
                if crate::protocol::native_value_bytes(&response).is_none() {return Err(NativeError::new("nativeResponseLimit","Profile review exceeds the response limit."));}
                self.profile_edit_preview=Some((token,plan));
                Ok(response)
            }
            "native.profileEdit.commit" => {
                let (expected,id)=mutation(params)?;
                let input=operation_input(method,params);
                let lock=lock.ok_or_else(NativeError::params)?;
                let store=Store::open_writer(lock)?;
                // Durable canonical input authority precedes session tokens,
                // profile lookup, revision checks and any private jobs cleanup.
                if let Some(result)=store.replay(id,&input)? {return Ok(result);}
                crate::apply::ensure_idle(&store)?;
                let token=required(params,"token")?;
                let profile=required(params,"profileId")?;
                let (_,plan)=self.profile_edit_preview.as_ref().filter(|(t,p)|t==token && p.root()==store.root() && p.profile_id()==profile && p.expected_revision()==expected)
                    .ok_or_else(||NativeError::new("stalePreview","Profile review is unavailable; review the edit again."))?;
                let result=plan.commit(&store,lock,expected,id,&input);
                self.profile_edit_preview=None;
                result
            }
            "native.snapshot" => {
                let store=Store::open(&root)?;let profile=optional(params,"profileId")?;
                let snapshot=store.snapshot(profile)?;
                if (snapshot["recoveryRequired"]!=true && !snapshot["warnings"].members().any(|w|w["code"]=="unverifiedRestore")) || !store.journals()?.is_empty(){return Ok(snapshot);}
                // Legacy admission is read-only and holds the same ordered
                // authority as a writer. Busy or missing roots remain unverified.
                let Ok(root_lock)=RootLock::acquire_existing(&root) else{return Ok(snapshot)};
                let doc=store.document()?;let Some(game)=doc["gamePath"].as_str() else{return Ok(snapshot)};
                let Ok(game_lock)=crate::game_access::GameLock::acquire(&Path::new(game).join("game/citadel")) else{return Ok(snapshot)};
                store.snapshot_locked(profile,&root_lock,&game_lock)
            },
            "native.performance.get" => crate::performance::view(&Store::open(&root)?.document()?),
            "native.performance.select" => {
                let (expected,id)=mutation(params)?;let input=operation_input(method,params);
                let lock=lock.ok_or_else(NativeError::params)?;let store=Store::open_writer(lock)?;
                if let Some(result)=store.replay(id,&input)? {return Ok(result);}
                crate::apply::ensure_idle(&store)?;
                let mut doc=store.document()?;let preset=params["presetId"].as_str().ok_or_else(NativeError::params)?;
                if !preset.is_empty()&&!crate::performance::catalog()?["presets"].members().any(|p|p["id"]==preset){return Err(NativeError::params());}
                crate::game_access::ensure_game_idle()?;
                doc["performancePreset"]=if preset.is_empty() {"none"} else {preset}.into();
                store.commit(lock,expected,id,&input,doc,optional(params,"profileId")?)
            },
            "native.operation" => {
                let id = required(params, "operationId")?;
                if !paths::safe_id(id) {
                    return Err(NativeError::params());
                }
                match Store::open_history(&root) {
                    Ok(s) => s.operation(id),
                    Err(e) if e.kind == "nativeNotImported" => {
                        Ok(object! {operationId:id,status:"unknown"})
                    }
                    Err(e) => Err(e),
                }
            }
            "native.fonts.preview" => {
                if params.entries().any(|(k,_)| !["dataDir","gamePath","expectedRevision","change"].contains(&k)) {return Err(NativeError::params())}
                let expected=store::revision(required(params,"expectedRevision")?)?;
                let change=&params["change"];
                if !change.is_object(){return Err(NativeError::params())}
                let change=match required(change,"kind")? {
                    "undo" if change.len()==1=>crate::fonts::FontChange::Undo,
                    kind @ ("install"|"remove") if change.len()==3 && change["fontIds"].is_array()=>{
                        if change["fontIds"].len()>256{return Err(NativeError::params())}
                        let mod_id=required(change,"modId")?.to_owned();
                        let font_ids=change["fontIds"].members().map(|id|id.as_str().map(str::to_owned).ok_or_else(NativeError::params)).collect::<Result<Vec<_>,_>>()?;
                        if kind=="install"{crate::fonts::FontChange::Install{mod_id,font_ids}}else{crate::fonts::FontChange::Remove{mod_id,font_ids}}
                    },_=>return Err(NativeError::params())};
                let store=Store::open(&root)?;
                store::ensure_readable(&store.document()?)?;
                // Release the previous bounded plan before retaining a new one.
                self.font_preview=None;
                let game=Path::new(required(params,"gamePath")?);
                let root_lock=RootLock::acquire_existing(&root)?;
                let _game_lock=crate::game_access::GameLock::acquire(&game.join("game/citadel"))?;
                crate::game_access::ensure_game_idle()?;
                let plan=crate::fonts::FontPlan::review_locked(&store,game,change,&root_lock,&_game_lock)?;
                if plan.revision()!=expected{return Err(NativeError::new("staleRevision","Library changed before font review."))}
                let token=paths::random_id()?;
                let response=object!{token:token.clone(),expectedRevision:plan.revision().to_string(),gamePath:plan.game().to_str(),summary:plan.summary().clone()};
                if crate::protocol::native_value_bytes(&response).is_none(){return Err(NativeError::new("nativeResponseLimit","Font review exceeds the response limit."))}
                self.font_preview=Some((token,plan));Ok(response)
            }
            "native.fonts.commit" => {
                if params.entries().any(|(k,_)| !["dataDir","operationId","expectedRevision","token"].contains(&k)){return Err(NativeError::params())}
                let (expected,id)=mutation(params)?;let token=required(params,"token")?;
                if !crate::package::hex(token,32){return Err(NativeError::params())}
                let input=operation_input(method,params);let lock=lock.ok_or_else(NativeError::params)?;
                let store=Store::open_writer(lock)?;
                // Canonical durable token/revision input takes precedence over
                // the session's single replaceable, uncommitted review plan.
                if let Some(result)=store.replay(id,&input)?{return Ok(result)}
                store::ensure_readable(&store.document()?)?;
                crate::apply::ensure_idle(&store)?;
                let (_,plan)=self.font_preview.as_ref().filter(|(t,p)|t==token&&p.root()==store.root()).ok_or_else(||NativeError::new("stalePreview","Font review is unavailable; review the change again."))?;
                let result=plan.commit(&store,lock,expected,id,&input);
                self.font_preview=None;
                result
            }
            "native.options.attach" => {
                if params.entries().any(|(k,_)|!["dataDir","operationId","expectedRevision","profileId","modId","jobId","token"].contains(&k)) {return Err(NativeError::params());}
                let (expected,id)=mutation(params)?;let input=operation_input(method,params);
                let lock=lock.ok_or_else(NativeError::params)?;let store=Store::open_writer(lock)?;
                // Durable replay precedes opening/reconciling the private job DB.
                if let Some(result)=store.replay(id,&input)? {return Ok(result);}
                let job_id=required(params,"jobId")?;
                if self.local_jobs.as_ref().is_some_and(|j|j.root()!=root) {return Err(NativeError::params());}
                let jobs=self.local_jobs.as_mut().ok_or_else(||NativeError::new("unknownLocalJob","No live options review exists; prepare again."))?;
                let plan=jobs.reserve_attachment(lock,job_id,required(params,"token")?,required(params,"profileId")?,required(params,"modId")?,expected,id)?;
                let outcome=crate::local_import::commit_attachment(&plan,lock,expected,id,&input);drop(plan);
                let outcome=match store.replay(id,&input) {
                    Ok(Some(result))=>Ok(result),Ok(None)=>outcome,
                    Err(_)=>return Err(NativeError::new("localJobOutcomeUnknown","The attachment outcome could not be read; reconcile its operation ID.")),
                };
                let finished=jobs.finish_commit(job_id,&outcome);
                if outcome.is_ok() {return outcome;}
                if finished.is_err() {return Err(NativeError::new("localJobOutcomeUnknown","The attachment may have committed; reconcile its operation ID."));}
                outcome
            }
            "native.local.commit" => {
                if params.entries().any(|(k, _)| {
                    ![
                        "dataDir",
                        "jobId",
                        "token",
                        "profileId",
                        "expectedRevision",
                        "operationId",
                    ]
                    .contains(&k)
                }) {
                    return Err(NativeError::params());
                }
                let (expected, id) = mutation(params)?;
                let input = operation_input(method, params);
                let lock = lock.ok_or_else(NativeError::params)?;
                let store = Store::open_writer(lock)?;
                // Durable operation authority precedes all transient plan/job authority.
                if let Some(result) = store.replay(id, &input)? {
                    return Ok(result);
                }
                let job_id = required(params, "jobId")?;
                crate::local_jobs::job_id(job_id)?;
                if self.local_jobs.as_ref().is_some_and(|j| j.root() != root) {
                    return Err(NativeError::params());
                }
                // Restart cannot reconstruct a live review token. Durable replay
                // above remains available without private jobs cleanup under this lock.
                let jobs = self.local_jobs.as_mut().ok_or_else(|| {
                    NativeError::new("unknownLocalJob", "No local preparation job exists.")
                })?;
                let plan = jobs.reserve_commit(
                    lock,
                    job_id,
                    required(params, "token")?,
                    required(params, "profileId")?,
                    expected,
                    id,
                )?;
                let outcome = crate::local_import::commit_local(
                    &plan,
                    lock,
                    expected,
                    id,
                    &input,
                    &mut |_| Ok(()),
                );
                drop(plan);
                // Never manufacture a rejected outcome after library commit.
                let outcome = match store.replay(id, &input) {
                    Ok(Some(result)) => Ok(result),
                    Ok(None) => outcome,
                    Err(_) => return Err(NativeError::new("localJobOutcomeUnknown", "The local operation outcome could not be read; reconcile its operation ID before retrying.")),
                };
                if jobs.finish_commit(job_id, &outcome).is_err() {
                    return Err(NativeError::new("localJobOutcomeUnknown","The local operation may have committed; reconcile its operation ID before retrying."));
                }
                outcome
            }
            "native.importPreview" => {
                let state = required(params, "statePath")?;
                let game = optional(params, "gamePath")?;
                match Store::open(&root) {
                    Ok(_) => {
                        return Err(NativeError::new(
                            "nativeAlreadyImported",
                            "A native library already exists; import cannot replace it.",
                        ))
                    }
                    Err(e) if e.kind == "nativeNotImported" => {}
                    Err(e) => return Err(e),
                }
                let plan = ImportPlan::review(&root, Path::new(state), game.map(Path::new))?;
                let token = paths::random_id()?;
                let mut summary = plan.summary.clone();
                summary["token"] = token.clone().into();
                if crate::protocol::native_value_bytes(&summary).is_none() {
                    return Err(NativeError::new(
                        "nativeResponseLimit",
                        "Import review exceeds the readable response limit.",
                    ));
                }
                if self.previews.len() >= 8 {
                    self.previews.clear()
                }
                self.previews.insert(token, plan);
                Ok(summary)
            }
            "native.importCommit" => {
                let (expected, id) = mutation(params)?;
                let input = operation_input(method, params);
                let token = required(params, "token")?;
                let lock = lock.ok_or_else(NativeError::params)?;
                if let Ok(store) = Store::open_history(&root) {
                    if let Some(result) = store.replay(id, &input)? {
                        return Ok(result);
                    }
                    match store.document() {
                        Ok(_) => {
                            return Err(NativeError::new(
                                "nativeAlreadyImported",
                                "A native library already exists; import cannot replace it.",
                            ))
                        }
                        Err(e) if e.kind == "nativeNotImported" => {}
                        Err(e) => return Err(e),
                    }
                }
                if expected != 0 {
                    return Err(NativeError::new(
                        "staleRevision",
                        "Initial import requires revision 0.",
                    ));
                }
                let plan = self
                    .previews
                    .get(token)
                    .filter(|p| p.root == root)
                    .ok_or_else(|| {
                        NativeError::new(
                            "stalePreview",
                            "Import preview is unavailable; review the source again.",
                        )
                    })?
                    .clone();
                let result = plan.commit(lock, id, &input)?;
                self.previews.remove(token);
                Ok(result)
            }
            "native.applyPreview" => {
                let store = Store::open(&root)?;
                if params["restoreReview"]==true {return crate::apply::restore_review(&store);}
                if params["deploymentLayout"] != "shards-v1" {
                    return Err(NativeError::new("unsupportedDeploymentLayout", "New Apply reviews require deploymentLayout shards-v1. Review again with the current app."));
                }
                let root_lock=RootLock::acquire_existing(&root)?;
                let plan = ApplyPlan::review_locked(
                    &store,&root_lock,
                    required(params, "profileId")?,
                    Path::new(required(params, "gamePath")?),
                )?;
                let token = paths::random_id()?;
                let mut summary = plan.summary.clone();
                summary["token"] = token.clone().into();
                if crate::protocol::native_value_bytes(&summary).is_none() {
                    return Err(NativeError::new(
                        "nativeResponseLimit",
                        "Apply review exceeds the readable response limit.",
                    ));
                }
                if self.apply_previews.len() >= 8 {
                    self.apply_previews.clear();
                }
                self.apply_previews.insert(token, plan);
                Ok(summary)
            }
            "native.apply" | "native.restore" | "native.recover" => {
                let (expected, id) = mutation(params)?;
                let input = operation_input(method, params);
                let lock = lock.ok_or_else(NativeError::params)?;
                let store = Store::open_writer(lock)?;
                if let Some(result) = store.replay(id, &input)? {
                    return Ok(result);
                }
                if method == "native.recover" {
                    match store.document() {
                        Err(e) if e.kind == "nativeNotImported" => {
                            return recover_import_only(&store, lock, expected, id, &input, None)
                        }
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
                if method!="native.recover" {store::ensure_readable(&store.document()?)?;}
                match method {
                    "native.apply" => {
                        let token = required(params, "token")?;
                        let plan = self.apply_previews.get(token).ok_or_else(|| {
                            NativeError::new(
                                "stalePreview",
                                "Apply review is unavailable; review again.",
                            )
                        })?;
                        let result = plan.apply(&store, lock, expected, id, &input)?;
                        self.apply_previews.remove(token);
                        Ok(result)
                    }
                    "native.restore" => crate::apply::restore(&store, lock, expected, id, &input),
                    _ => {if store.journals()?.members().any(|e|e["body"]["kind"]=="configWrite"){crate::configs::recover(&store,lock)?;store.snapshot(optional(params,"profileId")?)}else{crate::apply::recover(&store, lock, expected, id, &input)}},
                }
            }
            "native.bisect.get" => {let store=Store::open(&paths::data_root(params)?)?; Ok(crate::bisect::view(&store.document()?))}
            "native.bisect.start" | "native.bisect.answer" | "native.bisect.finish" => {
                let (expected,id)=mutation(params)?;let input=operation_input(method,params);
                let lock=lock.ok_or_else(NativeError::params)?;let store=Store::open_writer(lock)?;
                crate::bisect::change(&store,lock,method,params,expected,id,&input)
            }
            "native.setEnabled" | "native.setFixEnabled" | "native.setFixAutomatically" | "native.profile" | "native.setModsEnabled" | "native.removeModsFromProfile" => {
                let (expected, id) = mutation(params)?;
                let input = operation_input(method, params);
                let lock = lock.ok_or_else(NativeError::params)?;
                let store = Store::open_writer(lock)?;
                if let Some(result) = store.replay(id, &input)? {
                    return Ok(result);
                }
                if method=="native.profile" && params["action"]=="useGameFolder" {
                    crate::apply::ensure_folder_change_idle(&store)?;
                }
                crate::apply::ensure_idle(&store)?;
                let mut doc = store.document()?;
                if store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?
                    != expected
                {
                    return Err(NativeError::new(
                        "staleRevision",
                        "The native library changed; refresh before editing.",
                    ));
                }
                if matches!(method,"native.setModsEnabled"|"native.removeModsFromProfile") {
                    let profile = required(params,"profileId")?;
                    if !doc["profiles"][profile].is_object() {
                        return Err(NativeError::new("unknownProfile","Requested native profile does not exist."));
                    }
                    let remove = method == "native.removeModsFromProfile";
                    if remove {crate::game_access::ensure_game_idle()?;}
                    let enabled = params["enabled"].as_bool().unwrap_or(false);
                    let mut changed = 0;
                    let mut skipped = json::array![];
                    for mod_id in params["modIds"].members().map(|id|id.as_str().unwrap()) {
                        let index = doc["profiles"][profile]["mods"].members().position(|m|m["source"]["id"]==mod_id);
                        let reason = if index.is_none() { Some("unknownMod") }
                            else { None };
                        if let Some(reason) = reason { skipped.push(object!{id:mod_id,reason:reason}).map_err(|_|NativeError::params())?; continue; }
                        let index = index.unwrap();
                        if remove { doc["profiles"][profile]["mods"].array_remove(index); changed += 1; continue; }
                        match crate::bisect::set_enabled(&mut doc,profile,mod_id,enabled) {
                            Ok(true)=>changed+=1, Ok(false)=>{},
                            Err(e) if e.kind=="invalidLocalSelection"=>{skipped.push(object!{id:mod_id,reason:"invalidLocalSelection"}).map_err(|_|NativeError::params())?;},
                            Err(e)=>return Err(e),
                        }
                    }
                    return store.commit_with_bulk_result(lock,expected,id,&input,doc,Some(profile),Some(object!{changed:changed,skipped:skipped}));
                }
                if method == "native.setFixAutomatically" {
                    doc["fixAutomatically"] = params["enabled"].clone();
                    return store.commit(lock,expected,id,&input,doc,None);
                }
                let selected = if matches!(method,"native.setEnabled"|"native.setFixEnabled"|"native.setFixAutomatically") {
                    let layout = crate::package::layout_version(&doc)?;
                    let profile = required(params, "profileId")?;
                    let mod_id = required(params, "modId")?;
                    let enabled = params["enabled"]
                        .as_bool()
                        .ok_or_else(NativeError::params)?;
                    if !doc["profiles"][profile].is_object() {
                        return Err(NativeError::new(
                            "unknownProfile",
                            "Requested native profile does not exist.",
                        ));
                    }
                    let target = doc["profiles"][profile]["mods"]
                        .members_mut()
                        .find(|m| m["source"]["id"] == mod_id)
                        .ok_or_else(|| {
                            NativeError::new(
                                "unknownMod",
                                "Requested mod does not belong to this profile.",
                            )
                        })?;
                    if enabled && method=="native.setEnabled" {
                        let empty = || NativeError::new("invalidLocalSelection","Choose a nonempty set of ready local options before enabling this mod.");
                        if target.has_key("options") {
                            crate::options::for_record(target,layout)?.validate_selection_save().map_err(|_|empty())?;
                        } else if target["source"]["kind"]=="local" && target["source"]["localImport"]["selectedCandidateIds"].is_array()
                            && target["source"]["localImport"]["selectedCandidateIds"].is_empty() {
                            return Err(empty());
                        }
                    }
                    target[if method=="native.setFixEnabled" {"fixEnabled"} else {"enabled"}] = enabled.into();
                    Some(profile.to_owned())
                } else {
                    let action = required(params, "action")?;
                    match action {
                        "useGameFolder" => {
                            let guard=crate::apply::change_game_folder(&store,&mut doc,Path::new(required(params,"gamePath")?))?;
                            #[cfg(test)]
                            crate::apply::BEFORE_FOLDER_COMMIT.with(|h|if let Some(h)=h.borrow_mut().take(){h();});
                            let game=crate::installed::install_path(Path::new(doc["gamePath"].as_str().ok_or_else(NativeError::params)?))?;
                            guard.validate(&crate::apply::citadel(&game)?)?;
                            return store.commit(lock, expected, id, &input, doc, None);
                        }
                        "create" | "copy" => {
                            let name = profile_name(params)?;
                            let next = paths::random_id()?;
                            let mut profile = if action == "copy" {
                                let source = required(params, "profileId")?;
                                if !doc["profiles"][source].is_object() {
                                    return Err(NativeError::new(
                                        "unknownProfile",
                                        "Requested source profile does not exist.",
                                    ));
                                }
                                doc["profiles"][source].clone()
                            } else {
                                object! {source:object!{},mods:json::array![]}
                            };
                            profile["id"] = next.clone().into();
                            profile["name"] = name.into();
                            profile["protected"] = false.into();
                            doc["profiles"][&next] = profile;
                            Some(next)
                        }
                        "rename" => {
                            let profile = required(params, "profileId")?;
                            let name = profile_name(params)?;
                            if !doc["profiles"][profile].is_object() {
                                return Err(NativeError::new(
                                    "unknownProfile",
                                    "Requested native profile does not exist.",
                                ));
                            }
                            doc["profiles"][profile]["name"] = name.into();
                            Some(profile.to_owned())
                        }
                        "remove" => {
                            let profile = required(params, "profileId")?;
                            if !doc["profiles"][profile].is_object() {
                                return Err(NativeError::new(
                                    "unknownProfile",
                                    "Requested native profile does not exist.",
                                ));
                            }
                            if doc["profiles"][profile]["protected"]
                                .as_bool()
                                .unwrap_or(false)
                                || doc["applied"]["profileId"] == profile
                                || doc["previous"]["applied"]["profileId"] == profile
                                || doc["previous"]["activeProfileId"] == profile
                            {
                                return Err(NativeError::new(
                                    "protectedProfile",
                                    "The default, applied, or restorable profile cannot be removed.",
                                ));
                            }
                            doc["profiles"].remove(profile);
                            if doc["activeProfileId"] == profile {
                                doc["activeProfileId"] = "default".into()
                            }
                            None
                        }
                        _ => return Err(NativeError::params()),
                    }
                };
                store.commit(lock, expected, id, &input, doc, selected.as_deref())
            }
            _ => Err(NativeError::new(
                "methodNotFound",
                "Unknown or unimplemented native method.",
            )),
        }
    }
}
fn profile_name(params: &JsonValue) -> Result<&str, NativeError> {
    required(params, "name").and_then(|name| {
        if name.len() <= 256 && !name.chars().any(char::is_control) {
            Ok(name)
        } else {
            Err(NativeError::params())
        }
    })
}

fn recover_import_only(
    store: &Store,
    lock: &RootLock,
    expected: u64,
    id: &str,
    input: &JsonValue,
    doc: Option<JsonValue>,
) -> Result<JsonValue, NativeError> {
    let current = doc
        .as_ref()
        .map(|d| store::revision(d["revision"].as_str().ok_or_else(NativeError::params)?))
        .transpose()?
        .unwrap_or(0);
    if current != expected {
        return Err(NativeError::new(
            "staleRevision",
            "Library changed before import recovery; refresh first.",
        ));
    }
    if let Some(d) = &doc {
        store::ensure_readable(d)?;
        if !d["applied"].is_null() {
            return Err(NativeError::new(
                "unsupportedPackageFormat",
                "Package-family game recovery requires the complete deployment helper.",
            ));
        }
    }
    for j in store.journals()?.members() {
        if j["body"]["kind"] != "import" {
            return Err(NativeError::new(
                "recoveryRequired",
                "This helper can only reconcile import journals for this library format.",
            ));
        }
    }
    crate::import::reconcile_imports(store, lock)?;
    match doc {
        Some(d) => store.commit(lock, expected, id, input, d, None),
        None => Err(NativeError::not_imported()),
    }
}

#[cfg(test)]
#[test]
fn item115_small_io_messages_pass_validators(){
    for code in [5,13,2,30,28]{let e=NativeError::from(std::io::Error::from_raw_os_error(code));assert_eq!(e.kind,"nativeIo");
        if code!=28 {assert_eq!(e.message,"Cannot access native or source files; check paths, available storage and permissions.");}
        else {assert!(e.message.contains("Not enough disk space"));}
        assert!(crate::store::safe_remote_failure(&e));assert!(e.message.len()<=256);assert!(!e.message.contains("://"));
        crate::remote::contract::validate_error_value(&object!{kind:"transportStorage",message:e.message,retryAfterSeconds:JsonValue::Null}).unwrap();
    }
}
