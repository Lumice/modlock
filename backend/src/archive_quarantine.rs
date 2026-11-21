//! Durable, flat quarantine ownership. No SQLite, recursive removal, or Drop I/O.
//! Callers serialize factory/recovery/cleanup with the private jobs-store lease. Workers
//! retain an Arc; cleanup requires exclusive Arc ownership after worker reap.
use crate::{native::NativeError, paths, local_job_store::LocalJobStore, prepare_control::PrepareControl};
use json::{object, JsonValue};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::{ffi::OsStrExt, fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}}},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const PREFIX: &str = ".local-quarantine-";
const MAX_CLAIMS: usize = 20_001;
const MAX_REGISTRY: u64 = 8 * 1024 * 1024;
const MAX_REGISTRY_IO: u64 = 64 * 1024 * 1024;
const OWNER: &str = ".owner";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Domain { Local, Remote }
impl Domain {
    fn prefix(self) -> &'static str {
        match self { Self::Local => PREFIX, Self::Remote => std::str::from_utf8(crate::remote::job_store::REMOTE_QUARANTINE_PREFIX).expect("static ASCII prefix") }
    }
    fn kind(self) -> &'static str { match self { Self::Local => "localArchiveQuarantine", Self::Remote => "remoteArchiveQuarantine" } }
}

#[cfg(test)]
#[path = "../tests/archive_boundary/quarantine.rs"]
mod tests;
fn recovery() -> NativeError {
    NativeError::new("recoveryRequired", "Quarantine ownership is incomplete or changed; preserve its registry and files for recovery.")
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity { device: u64, inode: u64, uid: u32, mode: u32 }
impl Identity {
    fn of(meta: &fs::Metadata) -> Self {
        Self { device: meta.dev(), inode: meta.ino(), uid: meta.uid(), mode: meta.mode() }
    }
    fn json(&self) -> JsonValue { object! {device:self.device,inode:self.inode,uid:self.uid,mode:self.mode} }
    fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        Ok(Self { device:v["device"].as_u64().ok_or_else(recovery)?, inode:v["inode"].as_u64().ok_or_else(recovery)?, uid:v["uid"].as_u32().ok_or_else(recovery)?, mode:v["mode"].as_u32().ok_or_else(recovery)? })
    }
}
#[derive(Clone)]
struct Claim { name: String, identity: Option<Identity>, complete: bool }
#[derive(Clone, PartialEq, Eq)]
struct CleanupIntent { kind: String, name: String, identity: Identity }
#[derive(Clone)]
struct Registry {
    domain: Domain,
    root: PathBuf,
    root_identity: Identity,
    root_fsid: Option<String>,
    job_id: String,
    nonce: String,
    scratch_identity: Option<Identity>,
    cleanup_started: bool,
    ambiguous: bool,
    claims: Vec<Claim>,
    registry_identity: Option<Identity>,
    written_bytes: u64,
    cleanup_intents: Vec<CleanupIntent>,
}
fn remote_cleanup_budget(state:&Registry, extra:&[String])->Result<(),NativeError>{
    if state.domain!=Domain::Remote{return Ok(());}
    let mut projected=state.clone();let largest=Identity{device:u64::MAX,inode:u64::MAX,uid:u32::MAX,mode:u32::MAX};
    for claim in &mut projected.claims{if claim.identity.is_none(){claim.identity=Some(largest.clone());}}
    projected.claims.extend(extra.iter().map(|name|Claim{name:name.clone(),identity:Some(largest.clone()),complete:true}));
    projected.cleanup_started=true;
    projected.cleanup_intents=projected.claims.iter().filter_map(|c|c.identity.clone().map(|identity|CleanupIntent{kind:"file".into(),name:c.name.clone(),identity}))
        .chain(projected.scratch_identity.clone().map(|identity|CleanupIntent{kind:"directory".into(),name:projected.scratch_name(),identity})).collect();
    let bytes=projected.json().dump().len() as u64+32;
    if bytes>MAX_REGISTRY||state.written_bytes.checked_add(bytes).is_none_or(|n|n>MAX_REGISTRY_IO){return Err(NativeError::new("archiveLimitExceeded","Remote quarantine cleanup cannot fit its durable budget."));}Ok(())
}
impl Registry {
    fn scratch_name(&self) -> String { format!("{}{}", self.domain.prefix(), self.nonce) }
    fn registry_name(&self) -> String { format!("{}.json", self.scratch_name()) }
    fn json(&self) -> JsonValue {
        let mut value = object! {
            version:1, kind:self.domain.kind(), root:self.root.to_str().unwrap(),
            rootIdentity:self.root_identity.json(), jobId:self.job_id.clone(), nonce:self.nonce.clone(),
            scratchName:self.scratch_name(), ownerName:OWNER,
            scratchIdentity:self.scratch_identity.as_ref().map(Identity::json).unwrap_or(JsonValue::Null),
            state:if self.ambiguous { "ambiguous" } else if self.cleanup_started { "cleanupStarted" } else if self.scratch_identity.is_some() { "owned" } else { "planned" },
            writtenBytes:self.written_bytes,
            claims:JsonValue::Array(self.claims.iter().map(|c| object! {
                name:c.name.clone(), identity:c.identity.as_ref().map(Identity::json).unwrap_or(JsonValue::Null),
                state:if c.complete {"complete"} else if c.identity.is_some() {"owned"} else {"intent"}
            }).collect())
        };
        if let Some(fsid)=&self.root_fsid {value["rootFsid"]=fsid.as_str().into();}
        if self.domain == Domain::Remote {
            value["cleanupIntents"] = JsonValue::Array(self.cleanup_intents.iter().map(|i| object! {
                kind:i.kind.clone(), name:i.name.clone(), identity:i.identity.json()
            }).collect());
        }
        value
    }
    fn parse(root: &Path, name: &str, value: &JsonValue, identity: Identity, domain: Domain) -> Result<Self, NativeError> {
        let root_fsid=if value.has_key("rootFsid") {Some(value["rootFsid"].as_str().filter(|v|crate::package::hex(v,16)).ok_or_else(recovery)?.to_owned())} else {None};
        let nonce = value["nonce"].as_str().ok_or_else(recovery)?;
        let job_id = value["jobId"].as_str().filter(|v| paths::safe_id(v)).ok_or_else(recovery)?;
        if value["version"].as_u32() != Some(1) || value["kind"].as_str() != Some(domain.kind())
            || value["root"].as_str() != root.to_str() || nonce.len() != 32
            || !nonce.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || name != format!("{}{nonce}.json",domain.prefix())
            || value["scratchName"].as_str() != Some(format!("{}{nonce}",domain.prefix()).as_str())
            || value["ownerName"].as_str() != Some(OWNER)
            || !value["claims"].is_array() || value["claims"].len() > MAX_CLAIMS
        { return Err(recovery()); }
        let state = value["state"].as_str().ok_or_else(recovery)?;
        if !matches!(state, "planned" | "owned" | "cleanupStarted" | "ambiguous") { return Err(recovery()); }
        let scratch_identity = if value["scratchIdentity"].is_null() { None } else { Some(Identity::parse(&value["scratchIdentity"])?) };
        if state == "planned" && scratch_identity.is_some() || state == "owned" && scratch_identity.is_none() { return Err(recovery()); }
        let mut names = HashSet::new();
        let mut claims = Vec::new();
        for c in value["claims"].members() {
            let name = c["name"].as_str().filter(|n| valid_claim(n)).ok_or_else(recovery)?;
            if !names.insert(name.to_owned()) { return Err(recovery()); }
            let identity = if c["identity"].is_null() { None } else { Some(Identity::parse(&c["identity"])?) };
            let complete = match c["state"].as_str() {
                Some("intent") if identity.is_none() => false,
                Some("owned") if identity.is_some() => false,
                Some("complete") if identity.is_some() => true,
                _ => return Err(recovery()),
            };
            claims.push(Claim { name:name.to_owned(), identity, complete });
        }
        if !claims.is_empty() && claims[0].name != OWNER { return Err(recovery()); }
        let mut cleanup_intents = Vec::new();
        if domain == Domain::Remote {
            let keys = ["version","kind","root","rootIdentity","jobId","nonce","scratchName","ownerName","scratchIdentity","state","writtenBytes","claims","cleanupIntents"];
            if !value.is_object() || value.len()!=keys.len()+usize::from(root_fsid.is_some()) || keys.iter().any(|k|!value.has_key(k)) || !value["cleanupIntents"].is_array() || value["cleanupIntents"].len()>MAX_CLAIMS+1 { return Err(recovery()); }
            let identity_shape=|v:&JsonValue|v.is_object()&&v.len()==4&&["device","inode","uid","mode"].iter().all(|key|v.has_key(key));
            if !identity_shape(&value["rootIdentity"]) || !value["scratchIdentity"].is_null()&&!identity_shape(&value["scratchIdentity"]) {return Err(recovery());}
            for item in value["claims"].members(){if !item.is_object()||item.len()!=3||["name","identity","state"].iter().any(|key|!item.has_key(key))||!item["identity"].is_null()&&!identity_shape(&item["identity"]){return Err(recovery());}}
            for item in value["cleanupIntents"].members() {
                if !item.is_object() || item.len()!=3 || !item.has_key("kind") || !item.has_key("name") || !identity_shape(&item["identity"]) { return Err(recovery()); }
                cleanup_intents.push(CleanupIntent { kind:item["kind"].as_str().ok_or_else(recovery)?.into(), name:item["name"].as_str().ok_or_else(recovery)?.into(), identity:Identity::parse(&item["identity"])? });
            }
            let expected: Vec<_> = if state == "cleanupStarted" { claims.iter().filter_map(|c|c.identity.clone().map(|identity|CleanupIntent{kind:"file".into(),name:c.name.clone(),identity})).chain(scratch_identity.clone().map(|identity|CleanupIntent{kind:"directory".into(),name:format!("{}{nonce}",domain.prefix()),identity})).collect() } else { Vec::new() };
            if cleanup_intents != expected { return Err(recovery()); }
        }
        Ok(Self { domain, root:root.to_owned(), root_identity:Identity::parse(&value["rootIdentity"])?, root_fsid, job_id:job_id.to_owned(), nonce:nonce.to_owned(), scratch_identity, cleanup_started:state == "cleanupStarted", ambiguous:state == "ambiguous", claims, registry_identity:Some(identity), written_bytes:value["writtenBytes"].as_u64().filter(|v| *v <= MAX_REGISTRY_IO).ok_or_else(recovery)?, cleanup_intents })
    }
}
fn valid_claim(name: &str) -> bool {
    name == OWNER || name.strip_prefix("file-").is_some_and(|n| n.len() == 8 && n.bytes().all(|b| b.is_ascii_digit()))
}
fn anchor(file: &File) -> PathBuf { PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())) }
fn directory(path: &Path) -> Result<File, NativeError> {
    Ok(OpenOptions::new().read(true).custom_flags(0x10000 | 0x20000).open(path)?)
}
fn optional_identity(path: &Path, directory: bool) -> Result<Option<Identity>, NativeError> {
    match fs::symlink_metadata(path) {
        Ok(m) if (directory && m.is_dir() || !directory && m.is_file() && m.nlink() == 1)
            && !m.file_type().is_symlink() => Ok(Some(Identity::of(&m))),
        Ok(_) => Err(recovery()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn read_remote_registry(path:&Path,identity:&Identity)->Result<Vec<u8>,NativeError>{
    let mut file=paths::open_regular(path).map_err(|_|recovery())?;let before=file.metadata().map_err(|_|recovery())?;
    if Identity::of(&before)!=*identity||before.len()>MAX_REGISTRY{return Err(recovery());}
    let mut bytes=Vec::new();(&mut file).take(MAX_REGISTRY+1).read_to_end(&mut bytes).map_err(|_|recovery())?;
    let after=file.metadata().map_err(|_|recovery())?;
    if bytes.len() as u64!=before.len()||Identity::of(&after)!=*identity||optional_identity(path,false)?.as_ref()!=Some(identity)
        ||(after.len(),after.mtime(),after.mtime_nsec(),after.ctime(),after.ctime_nsec())!=(before.len(),before.mtime(),before.mtime_nsec(),before.ctime(),before.ctime_nsec()){return Err(recovery());}
    Ok(bytes)
}
fn rebind_identity(root:&Path,root_fd:&File,path:&Path,identity:&mut Identity,fsid:Option<&str>,is_dir:bool)->Result<(),NativeError>{
    let named=match optional_identity(path,is_dir)? {Some(v)=>v,None=>return Ok(())};
    let file=if is_dir {directory(path)?} else {paths::open_regular(path)?};
    let m=file.metadata()?;
    if named.uid!=identity.uid || named.mode!=identity.mode
        || !paths::same_persisted_object(root,root_fd,path,&file,identity.device,identity.inode,fsid).map_err(|_|recovery())?
        || Identity::of(&m)!=named {return Err(recovery());}
    identity.device=m.dev();
    Ok(())
}
fn rebind_registry(root_fd:&File,state:&mut Registry)->Result<(),NativeError>{
    let root=state.root.clone();let fsid=state.root_fsid.clone();
    rebind_identity(&root,root_fd,&root,&mut state.root_identity,fsid.as_deref(),true)?;
    let scratch=root.join(state.scratch_name());
    if let Some(identity)=&mut state.scratch_identity {
        rebind_identity(&root,root_fd,&scratch,identity,fsid.as_deref(),true)?;
    }
    for claim in &mut state.claims {
        if let Some(identity)=&mut claim.identity {
            rebind_identity(&root,root_fd,&scratch.join(&claim.name),identity,fsid.as_deref(),false)?;
        }
    }
    if state.domain==Domain::Remote && state.cleanup_started {
        for intent in &mut state.cleanup_intents {
            if intent.kind=="directory" {
                intent.identity=state.scratch_identity.clone().ok_or_else(recovery)?;
            } else {
                intent.identity=state.claims.iter().find(|c|c.name==intent.name).and_then(|c|c.identity.clone()).ok_or_else(recovery)?;
            }
        }
    }
    Ok(())
}
fn validate_root(root_fd: &File, registry: &Registry) -> Result<(), NativeError> {
    paths::reject_symlinks(&registry.root)?;
    if Identity::of(&root_fd.metadata()?) != registry.root_identity
        || optional_identity(&registry.root, true)?.as_ref() != Some(&registry.root_identity)
    { return Err(recovery()); }
    Ok(())
}
fn persist(root_fd: &File, registry: &mut Registry) -> Result<(), NativeError> {
    validate_root(root_fd, registry)?;
    let root = anchor(root_fd);
    let target = root.join(registry.registry_name());
    if optional_identity(&target, false)? != registry.registry_identity { return Err(recovery()); }
    // Reserved headroom permits the cleanup transition even after admission stops.
    let estimate = registry.json().dump().len() as u64 + 32;
    registry.written_bytes = registry.written_bytes.checked_add(estimate).filter(|v| *v <= MAX_REGISTRY_IO).ok_or_else(|| NativeError::new("archiveLimitExceeded", "Quarantine registry write budget exceeded."))?;
    let bytes = registry.json().dump();
    if bytes.len() as u64 > MAX_REGISTRY { return Err(recovery()); }
    if registry.domain==Domain::Remote && crate::remote::owner::free_bytes(&registry.root)? < 1024u64*1024*1024+bytes.len() as u64 {
        return Err(NativeError::new("storageFull","Preparation requires free storage headroom."));
    }
    let temporary = root.join(format!("{}.next", registry.registry_name()));
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(&temporary)?;
    file.write_all(bytes.as_bytes())?;
    file.sync_all()?;
    let identity = Identity::of(&file.metadata()?);
    if optional_identity(&target, false)? != registry.registry_identity { return Err(recovery()); }
    fs::rename(&temporary, &target)?;
    // Remember the new inode even when directory durability fails: a caller may
    // retry safely, but a restart must use the on-disk manifest as its authority.
    registry.registry_identity = Some(identity);
    root_fd.sync_all()?;
    Ok(())
}
fn record_ambiguity(root_fd: &File, registry: &mut Registry, error: &NativeError) {
    if error.kind == "recoveryRequired" && !registry.ambiguous {
        registry.ambiguous = true;
        // Best effort only: a replaced registry/root must never be overwritten.
        // The original evidence already represents the incomplete identity gap.
        let _ = persist(root_fd, registry);
    }
}

/// Serial-thread authority borrowed from the private jobs lease, never carried
/// into worker threads or retained by QuarantineLease. The root is store-derived.
pub(crate) struct QuarantineAuthority<'a> { store: &'a LocalJobStore }
impl<'a> QuarantineAuthority<'a> {
    pub(crate) fn for_held_jobs_store(store: &'a LocalJobStore) -> Self { Self { store } }
    fn root(&self) -> &Path { self.store.root() }
}

/// Only this module creates/decodes registry evidence. Dropping a lease preserves
/// disk evidence; successful explicit cleanup (or restart recovery) removes it.
pub struct QuarantineLease { root_fd: File, scratch_fd: File, state: Mutex<Registry> }
impl QuarantineLease {
    pub(crate) fn create(lock: &QuarantineAuthority<'_>, job_id: &str) -> Result<Arc<Self>, NativeError> {
        Self::create_in(lock.root(), job_id, paths::random_id()?, Domain::Local)
    }
    fn create_in(root: &Path, job_id: &str, nonce: String, domain: Domain) -> Result<Arc<Self>, NativeError> {
        let bulk = crate::storage::root(root)?; let root = bulk.as_path();
        if !paths::safe_id(job_id) || root.to_str().is_none() { return Err(NativeError::params()); }
        paths::reject_symlinks(root)?;
        let root_fd = directory(root)?;
        let mut state = Registry { domain, root:root.to_owned(), root_identity:Identity::of(&root_fd.metadata()?), root_fsid:Some(paths::persisted_filesystem_id(&root_fd)?), job_id:job_id.to_owned(), nonce, scratch_identity:None, cleanup_started:false, ambiguous:false, claims:Vec::new(), registry_identity:None, written_bytes:0, cleanup_intents:Vec::new() };
        let scratch = anchor(&root_fd).join(state.scratch_name());
        if optional_identity(&scratch, true)?.is_some() { return Err(recovery()); }
        // Durable absent-create intent precedes mkdir. A crash between mkdir and
        // identity persistence is deliberately ambiguous and never auto-deleted.
        persist(&root_fd, &mut state)?;
        fs::DirBuilder::new().mode(0o700).create(&scratch)?;
        root_fd.sync_all()?;
        let scratch_fd = directory(&scratch)?;
        if Some(paths::persisted_filesystem_id(&scratch_fd)?)!=state.root_fsid {return Err(recovery());}
        state.scratch_identity = Some(Identity::of(&scratch_fd.metadata()?));
        persist(&root_fd, &mut state)?;
        let lease = Arc::new(Self { root_fd, scratch_fd, state:Mutex::new(state) });
        let mut owner = lease.open_claim(lease.allocate_names(&[OWNER.to_owned()], None)?.remove(0))?;
        let contents = format!("{}\n{job_id}\n{}\n", domain.kind(), lease.nonce()?);
        owner.admit_write(contents.len())?;
        owner.file.write_all(contents.as_bytes())?;
        owner.file.sync_all()?;
        lease.complete_batch(&[owner.name.clone()])?;
        Ok(lease)
    }
    pub fn job_id(&self) -> Result<String, NativeError> { Ok(self.state.lock().map_err(|_| recovery())?.job_id.clone()) }
    pub fn nonce(&self) -> Result<String, NativeError> { Ok(self.state.lock().map_err(|_| recovery())?.nonce.clone()) }
    pub fn scratch_path(&self) -> Result<PathBuf, NativeError> {
        let state = self.state.lock().map_err(|_| recovery())?;
        Ok(state.root.join(state.scratch_name()))
    }
    pub fn registry_path(&self) -> Result<PathBuf, NativeError> {
        let state = self.state.lock().map_err(|_| recovery())?;
        Ok(state.root.join(state.registry_name()))
    }
    fn validate(&self, state: &Registry) -> Result<(), NativeError> {
        validate_root(&self.root_fd, state)?;
        if state.cleanup_started || state.ambiguous
            || Some(Identity::of(&self.scratch_fd.metadata()?)) != state.scratch_identity
            || optional_identity(&anchor(&self.root_fd).join(state.scratch_name()), true)? != state.scratch_identity
        { return Err(recovery()); }
        Ok(())
    }
    /// Read-only review admission: the complete lease must still be sealed and
    /// every owned directory/registry/file identity must match its durable claim.
    pub(crate) fn validate_sealed(&self) -> Result<(), NativeError> {
        let state = self.state.lock().map_err(|_| recovery())?;
        self.validate(&state)?;
        if state.claims.iter().any(|claim| !claim.complete) {
            return Err(recovery());
        }
        remote_cleanup_budget(&state,&[])?;
        preflight(&self.root_fd, &state)
    }
    pub(crate) fn allocate(&self, count: usize, control: &PrepareControl) -> Result<Vec<QuarantineClaim>, NativeError> {
        control.check()?;
        if count == 0 { return Ok(Vec::new()); }
        let first = self.state.lock().map_err(|_| recovery())?.claims.len();
        if count > MAX_CLAIMS || first.checked_add(count).is_none_or(|n| n > MAX_CLAIMS) { return Err(NativeError::new("archiveLimitExceeded", "Too many quarantine files.")); }
        let names: Vec<_> = (first..first + count).map(|n| format!("file-{n:08}")).collect();
        self.allocate_names(&names, Some(control))
    }
    fn allocate_names(&self, names: &[String], control: Option<&PrepareControl>) -> Result<Vec<QuarantineClaim>, NativeError> {
        let mut state = self.state.lock().map_err(|_| recovery())?;
        self.validate(&state)?;
        if state.written_bytes > MAX_REGISTRY_IO - 4 * MAX_REGISTRY { return Err(NativeError::new("archiveLimitExceeded", "Quarantine registry admission budget exceeded.")); }
        remote_cleanup_budget(&state,names)?;
        let base = anchor(&self.scratch_fd);
        let existing: HashSet<_> = state.claims.iter().map(|c| c.name.as_str()).collect();
        for name in names {
            if let Some(control) = control { control.check()?; }
            if !valid_claim(name) || existing.contains(name.as_str())
                || optional_identity(&base.join(name), false)?.is_some() { return Err(recovery()); }
        }
        let first = state.claims.len();
        state.claims.extend(names.iter().map(|name| Claim { name:name.clone(), identity:None, complete:false }));
        persist(&self.root_fd, &mut state)?;
        let mut files = Vec::with_capacity(names.len());
        for (offset, name) in names.iter().enumerate() {
            if let Some(control) = control { control.check()?; }
            let file = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(base.join(name))?;
            if Some(paths::persisted_filesystem_id(&file)?)!=state.root_fsid {return Err(recovery());}
            file.sync_all()?;
            state.claims[first + offset].identity = Some(Identity::of(&file.metadata()?));
            files.push(QuarantineClaim { path:state.root.join(state.scratch_name()).join(name), name:name.clone() });
        }
        self.scratch_fd.sync_all()?;
        // All identities are durable before a caller may write any member bytes.
        persist(&self.root_fd, &mut state)?;
        Ok(files)
    }
    pub(crate) fn open_claim(&self, claim: QuarantineClaim) -> Result<QuarantineFile, NativeError> {
        let state = self.state.lock().map_err(|_| recovery())?;
        self.validate(&state)?;
        let recorded = state.claims.iter().find(|c| c.name == claim.name && !c.complete).ok_or_else(recovery)?;
        let file = OpenOptions::new().read(true).write(true).custom_flags(0x20000 | 0x800).open(anchor(&self.scratch_fd).join(&claim.name))?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.len() != 0 || meta.nlink() != 1 || Some(Identity::of(&meta)) != recorded.identity { return Err(recovery()); }
        Ok(QuarantineFile { name:claim.name, path:claim.path, file, remote_root:if state.domain==Domain::Remote{Some(state.root.clone())}else{None} })
    }
    pub(crate) fn complete_batch(&self, names: &[String]) -> Result<(), NativeError> {
        if names.is_empty() { return Ok(()); }
        let mut state = self.state.lock().map_err(|_| recovery())?;
        self.validate(&state)?;
        let mut remaining: HashSet<_> = names.iter().map(String::as_str).collect();
        if remaining.len() != names.len() { return Err(recovery()); }
        for claim in &mut state.claims {
            if !remaining.remove(claim.name.as_str()) { continue; }
            if claim.identity.is_none() || optional_identity(&anchor(&self.scratch_fd).join(&claim.name), false)? != claim.identity { return Err(recovery()); }
            claim.complete = true;
        }
        if !remaining.is_empty() { return Err(recovery()); }
        persist(&self.root_fd, &mut state)
    }
    /// Consume the last Arc only after prepared results and the worker are gone.
    /// A busy result leaves disk evidence untouched, and other Arc owners alive.
    pub(crate) fn cleanup(self: Arc<Self>, lock: &QuarantineAuthority<'_>) -> Result<(), NativeError> {
        self.cleanup_for(lock.root(), Domain::Local, None)
    }
    fn cleanup_for(self: Arc<Self>, root: &Path, domain: Domain, identity: Option<(&str, &str)>) -> Result<(), NativeError> {
        let bulk = crate::storage::root(root)?; let root = bulk.as_path();
        let lease = Arc::try_unwrap(self).map_err(|_| NativeError::new("nativeBusy", "Quarantine is still retained by a preparation worker or result."))?;
        let mut state = lease.state.into_inner().map_err(|_| recovery())?;
        if root != state.root || state.domain != domain || identity.is_some_and(|(job,nonce)|job!=state.job_id||nonce!=state.nonce) { return Err(recovery()); }
        if let Err(error) = preflight(&lease.root_fd, &state) {
            record_ambiguity(&lease.root_fd, &mut state, &error);
            return Err(error);
        }
        cleanup_registry(&lease.root_fd, &mut state)
    }
}
pub(crate) struct QuarantineFile { pub(crate) name:String, pub(crate) path:PathBuf, pub(crate) file:File, remote_root:Option<PathBuf> }
impl QuarantineFile {
    pub(crate) fn admit_write(&self, bytes:usize)->Result<(),NativeError>{
        if let Some(root)=&self.remote_root {
            let required=(1024u64*1024*1024).checked_add(bytes as u64).ok_or_else(recovery)?;
            if crate::remote::owner::free_bytes(root)?<required{return Err(NativeError::new("storageFull","Preparation requires free storage headroom."));}
        }
        Ok(())
    }
}
pub(crate) struct QuarantineClaim { name:String, path:PathBuf }

fn preflight(root_fd: &File, state: &Registry) -> Result<(), NativeError> {
    validate_root(root_fd, state)?;
    if state.ambiguous || optional_identity(&anchor(root_fd).join(state.registry_name()), false)? != state.registry_identity { return Err(recovery()); }
    let scratch = anchor(root_fd).join(state.scratch_name());
    let actual = optional_identity(&scratch, true)?;
    if actual.is_none() {
        let intent = state.scratch_identity.as_ref().is_some_and(|identity|state.cleanup_intents.contains(&CleanupIntent{kind:"directory".into(),name:state.scratch_name(),identity:identity.clone()}));
        if state.scratch_identity.is_none() && state.claims.is_empty() || state.cleanup_started && (state.domain == Domain::Local || intent) { return Ok(()); }
        return Err(recovery());
    }
    if actual != state.scratch_identity { return Err(recovery()); }
    let dir = directory(&scratch)?;
    let base = anchor(&dir);
    let known: HashSet<_> = state.claims.iter().map(|c| c.name.as_str()).collect();
    let mut count = 0usize;
    for entry in fs::read_dir(&base)? {
        let entry = entry?;
        count += 1;
        if count > MAX_CLAIMS || !entry.file_name().to_str().is_some_and(|n| known.contains(n)) { return Err(recovery()); }
    }
    for claim in &state.claims {
        match (optional_identity(&base.join(&claim.name), false)?, &claim.identity) {
            (None, None) => {}, // absent-create intent never made a file
            (None, Some(expected)) if state.cleanup_started && (state.domain == Domain::Local || state.cleanup_intents.contains(&CleanupIntent{kind:"file".into(),name:claim.name.clone(),identity:expected.clone()})) => {},
            (Some(actual), Some(expected)) if &actual == expected => {},
            _ => return Err(recovery()),
        }
        if state.domain == Domain::Remote && claim.name == OWNER && optional_identity(&base.join(OWNER), false)?.is_some() {
            let mut file=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(base.join(OWNER)).map_err(|_|recovery())?;
            if Some(Identity::of(&file.metadata()?))!=claim.identity { return Err(recovery()); }
            let mut contents=String::new();
            (&mut file).take(257).read_to_string(&mut contents).map_err(|_|recovery())?;
            if contents!=format!("{}\n{}\n{}\n",state.domain.kind(),state.job_id,state.nonce)
                || Some(Identity::of(&file.metadata()?))!=claim.identity { return Err(recovery()); }
        }
    }
    Ok(())
}
fn cleanup_registry(root_fd: &File, state: &mut Registry) -> Result<(), NativeError> {
    preflight(root_fd, state)?;
    if !state.cleanup_started {
        state.cleanup_started = true;
        if state.domain == Domain::Remote {
            state.cleanup_intents = state.claims.iter().filter_map(|c|c.identity.clone().map(|identity|CleanupIntent{kind:"file".into(),name:c.name.clone(),identity}))
                .chain(state.scratch_identity.clone().map(|identity|CleanupIntent{kind:"directory".into(),name:state.scratch_name(),identity})).collect();
        }
        persist(root_fd, state)?;
    }
    // Also retry durability when resuming a previously failed directory sync.
    root_fd.sync_all()?;
    let root = anchor(root_fd);
    let scratch = root.join(state.scratch_name());
    if optional_identity(&scratch, true)?.is_some() {
        let dir = directory(&scratch)?;
        let base = anchor(&dir);
        // Owner is always last. Every unlink rechecks its recorded identity.
        for claim in state.claims.iter().filter(|c| c.name != OWNER).chain(state.claims.iter().filter(|c| c.name == OWNER)) {
            if let Some(actual) = optional_identity(&base.join(&claim.name), false)? {
                if Some(actual) != claim.identity { return Err(recovery()); }
                fs::remove_file(base.join(&claim.name))?;
            }
        }
        dir.sync_all()?;
        if optional_identity(&scratch, true)? != state.scratch_identity { return Err(recovery()); }
        fs::remove_dir(&scratch)?; // refuses unknown files; never recursive
        root_fd.sync_all()?;
    }
    if optional_identity(&root.join(state.registry_name()), false)? != state.registry_identity { return Err(recovery()); }
    fs::remove_file(root.join(state.registry_name()))?;
    root_fd.sync_all()?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct RecoveredQuarantine { pub job_id: String, pub nonce: String }
/// Preflight every manifest and every claim before deleting anything. Unknown
/// scratch directories, .next files, malformed manifests and incomplete identity
/// gaps refuse the whole recovery set. The caller owns subsequent SQLite writes.
fn preflight_quarantines(
    root: &Path,
    domain: Domain,
    record_ambiguities: bool,
) -> Result<(File, Vec<Registry>), NativeError> {
    let bulk = crate::storage::root(root)?; let root = bulk.as_path();
    paths::reject_symlinks(root)?;
    let root_fd = directory(root)?;
    let mut registries = Vec::new();
    let mut scratch_names = HashSet::new();
    let mut names = HashSet::new();
    let mut scanned = 0usize;
    let mut registry_bytes = 0u64;
    for entry in fs::read_dir(root)? {
        scanned += 1;
        if scanned > 20_000 { return Err(recovery()); }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            if name.as_bytes().starts_with(domain.prefix().as_bytes()) { return Err(recovery()); }
            continue;
        };
        if !name.starts_with(domain.prefix()) { continue; }
        if names.len() >= 128 { return Err(recovery()); }
        names.insert(name.to_owned());
        if name.ends_with(".json") {
            if registries.len() >= 16 { return Err(recovery()); }
            let path = root.join(name);
            let identity = optional_identity(&path, false)?.ok_or_else(recovery)?;
            let bytes = if domain==Domain::Remote{read_remote_registry(&path,&identity)?}else{paths::read_bounded(&path, MAX_REGISTRY)?};
            registry_bytes = registry_bytes.checked_add(bytes.len() as u64).filter(|n| *n <= MAX_REGISTRY_IO).ok_or_else(recovery)?;
            crate::archive_content::check_json_depth(&bytes, 64).map_err(|_| recovery())?;
            if domain == Domain::Remote && (!crate::protocol::unique_keys(&bytes) || !crate::protocol::integer_numbers(&bytes)) { return Err(recovery()); }
            let value = json::parse(std::str::from_utf8(&bytes).map_err(|_| recovery())?).map_err(|_| recovery())?;
            let mut state = Registry::parse(root, name, &value, identity, domain)?;
            rebind_registry(&root_fd,&mut state)?;
            scratch_names.insert(state.scratch_name());
            if let Err(error) = preflight(&root_fd, &state) {
                if record_ambiguities {
                    record_ambiguity(&root_fd, &mut state, &error);
                }
                return Err(error);
            }
            registries.push(state);
        }
    }
    if names.iter().any(|n| !scratch_names.contains(n) && !registries.iter().any(|r| r.registry_name() == *n)) { return Err(recovery()); }
    // A second complete-set preflight narrows changes during manifest loading.
    for state in &registries { preflight(&root_fd, state)?; }
    Ok((root_fd, registries))
}
/// Bounded read-only complete-set admission while other jobs retain live leases.
/// The caller compares typed job/nonce claims; this never consumes their files.
pub(crate) fn inspect_quarantines(
    lock: &QuarantineAuthority<'_>,
) -> Result<Vec<RecoveredQuarantine>, NativeError> {
    let (_, registries) = preflight_quarantines(lock.root(), Domain::Local, false)?;
    Ok(registries.into_iter().map(|state| RecoveredQuarantine {
        job_id: state.job_id,
        nonce: state.nonce,
    }).collect())
}
pub(crate) fn recover_quarantines(lock: &QuarantineAuthority<'_>) -> Result<Vec<RecoveredQuarantine>, NativeError> {
    let (root_fd, registries) = preflight_quarantines(lock.root(), Domain::Local, true)?;
    let mut recovered = Vec::new();
    for mut state in registries {
        cleanup_registry(&root_fd, &mut state)?;
        recovered.push(RecoveredQuarantine { job_id:state.job_id, nonce:state.nonce });
    }
    Ok(recovered)
}

pub(crate) fn create_remote(authority: &crate::remote::quarantine::RemoteQuarantineAuthority, slot: &crate::remote::preparation_slot::PreparationSlotLease) -> Result<Arc<QuarantineLease>, NativeError> {
    authority.validate(slot)?;
    QuarantineLease::create_in(authority.root(), authority.job.as_str(), authority.nonce.as_str().into(), Domain::Remote)
}
pub(crate) fn cleanup_remote(lease: Arc<QuarantineLease>, authority: &crate::remote::quarantine::RemoteQuarantineAuthority, slot: &crate::remote::preparation_slot::PreparationSlotLease) -> Result<(), NativeError> {
    authority.validate(slot)?;
    lease.cleanup_for(authority.root(), Domain::Remote, Some((authority.job.as_str(), authority.nonce.as_str())))
}
pub(crate) fn inspect_remote(authority: &crate::remote::quarantine::RemoteQuarantineAuthority, slot: &crate::remote::preparation_slot::PreparationSlotLease) -> Result<Vec<RecoveredQuarantine>, NativeError> {
    authority.validate(slot)?;
    let (_, registries) = preflight_quarantines(authority.root(), Domain::Remote, false)?;
    Ok(registries.into_iter().map(|r|RecoveredQuarantine{job_id:r.job_id,nonce:r.nonce}).collect())
}
pub(crate) fn recover_remote(authority: &crate::remote::quarantine::RemoteQuarantineAuthority, slot: &crate::remote::preparation_slot::PreparationSlotLease, admitted: &[(String, String)]) -> Result<Vec<RecoveredQuarantine>, NativeError> {
    authority.validate(slot)?;
    // Inspect the complete remote namespace without modifying ambiguous or
    // unknown registries before the owner has matched every job and nonce.
    let (root_fd, registries) = preflight_quarantines(authority.root(), Domain::Remote, false)?;
    if registries.iter().any(|r|!admitted.iter().any(|(job,nonce)|job==&r.job_id&&nonce==&r.nonce)) { return Err(recovery()); }
    let mut result=Vec::new();
    for mut state in registries { cleanup_registry(&root_fd,&mut state)?; result.push(RecoveredQuarantine{job_id:state.job_id,nonce:state.nonce}); }
    Ok(result)
}
