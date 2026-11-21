//! One service-thread job owner and one read-only preparation worker.
use crate::{
    apply,
    archive_quarantine::{
        inspect_quarantines, recover_quarantines, QuarantineAuthority, QuarantineLease,
    },
    local_content::{
        self, LocalImportPlan, LocalMetadata, LocalPrepareRequest, LocalPrepared, LocalSelection,
    },
    local_job_store::{self, LocalJobStore},
    native::NativeError,
    package, paths,
    prepare_control::PrepareControl,
    remote::preparation_slot::{PreparationSlot, PreparationSlotLease},
    protocol,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use crate::local_content::option_sources::{self, OptionsPrepareRequest, OptionSource, OptionAttachmentPlan, OptionBinding, PreparedOptions};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobKind { Import, Options }
#[derive(Clone, Debug, PartialEq, Eq)]
enum JobPurpose {
    Import,
    Options { profile_id: String, mod_id: String, before_record_digest: String },
}
fn job_purpose(body: &JsonValue) -> Result<JobPurpose,NativeError> {
    match body["kind"].as_str() {
        Some("localImport")=>Ok(JobPurpose::Import),
        Some("localOptions")=>Ok(JobPurpose::Options {
            profile_id:body["profileId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(local_job_store::corrupt)?.into(),
            mod_id:body["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(local_job_store::corrupt)?.into(),
            before_record_digest:body["beforeRecordDigest"].as_str().filter(|s|package::hex(s,64)).ok_or_else(local_job_store::corrupt)?.into(),
        }),
        _=>Err(local_job_store::corrupt()),
    }
}
fn purpose(body: &JsonValue) -> JobKind { if body["kind"]=="localOptions" { JobKind::Options } else { JobKind::Import } }
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
};
const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
fn error(kind: &str, message: &str) -> NativeError {
    NativeError::new(kind, message)
}
fn unknown() -> NativeError {
    error(
        "unknownLocalJob",
        "This local job is unknown or was dismissed.",
    )
}
fn not_ready() -> NativeError {
    error(
        "localJobNotReady",
        "This local job has no live ready review; prepare a new attempt.",
    )
}
fn preparing(phase: &str) -> bool {
    matches!(
        phase,
        "enumerating"
            | "inspecting"
            | "hashing"
            | "copyingArchives"
            | "listingArchives"
            | "extractingArchives"
            | "cancelRequested"
    )
}
fn terminal(phase: &str) -> bool {
    matches!(
        phase,
        "cancelled" | "interrupted" | "failed" | "succeeded" | "recoveryRequired"
    )
}
fn phase(body: &JsonValue) -> &str {
    body["phase"].as_str().unwrap_or("")
}
pub(crate) fn job_id(id: &str) -> Result<(), NativeError> {
    if package::hex(id, 32) {
        Ok(())
    } else {
        Err(NativeError::params())
    }
}
pub(crate) fn canonical_root(root: &Path) -> Result<PathBuf, NativeError> {
    Ok(paths::absolute(root)?.components().collect())
}
fn path_text(path: &Path) -> Result<&str, NativeError> {
    let s = path
        .to_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0'))
        .ok_or_else(NativeError::params)?;
    if !path.is_absolute() || s.split('/').any(|x| x == "." || x == "..") {
        return Err(NativeError::params());
    }
    Ok(s)
}
fn admitted_input(r: &LocalPrepareRequest) -> Result<String, NativeError> {
    path_text(&r.root)?;
    if !paths::safe_id(&r.profile_id) || r.roots.is_empty() || r.roots.len() > 64 {
        return Err(NativeError::params());
    }
    let roots = r
        .roots
        .iter()
        .map(|p| path_text(p))
        .collect::<Result<Vec<_>, _>>()?;
    let artwork = r.artwork.as_ref().map(|p| path_text(p)).transpose()?;
    let input = object! {root:r.root.to_str().unwrap(),profileId:r.profile_id.clone(),expectedRevision:r.expected_revision.to_string(),roots:JsonValue::from(roots),artwork:artwork};
    let input = store::canonical(&input);
    if input.len() > 64 * 1024 {
        return Err(NativeError::params());
    }
    Ok(input)
}
fn profile(root: &Path, id: &str, expected: u64) -> Result<(String, u64), NativeError> {
    let library = Store::open(root)?;
    apply::ensure_idle(&library)?;
    let doc = library.document()?;
    let revision = store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?;
    if revision != expected {
        return Err(error(
            "staleRevision",
            "The pinned native revision changed; prepare a new review.",
        ));
    }
    if !doc["profiles"][id].is_object() {
        return Err(error(
            "unknownProfile",
            "The pinned destination profile does not exist.",
        ));
    }
    Ok((
        doc["profiles"][id]["name"]
            .as_str()
            .unwrap_or(id)
            .to_owned(),
        revision,
    ))
}
fn trim_last(s: &mut JsonValue) {
    let value = s.as_str().unwrap_or("");
    let value = value.strip_suffix('…').unwrap_or(value);
    let keep = value.len() / 2;
    let mut at = keep;
    while !value.is_char_boundary(at) {
        at -= 1;
    }
    *s = format!("{}…", &value[..at]).into();
}
fn bounded(mut body: JsonValue) -> Result<JsonValue, NativeError> {
    for _ in 0..32 {
        if body.dump().len() <= 4096 {
            return Ok(body);
        }
        if body["error"]["message"]
            .as_str()
            .is_some_and(|s| s.len() > 3)
        {
            trim_last(&mut body["error"]["message"]);
        } else if body["profileNameAtStart"]
            .as_str()
            .is_some_and(|s| s.len() > 3)
        {
            trim_last(&mut body["profileNameAtStart"]);
        } else {
            return Err(error(
                "localStatusLimit",
                "Mandatory local job status exceeds its size limit.",
            ));
        }
    }
    Err(error(
        "localStatusLimit",
        "Local job status exceeds its size limit.",
    ))
}
fn next(mut body: JsonValue) -> Result<JsonValue, NativeError> {
    let n = store::revision(
        body["sequence"]
            .as_str()
            .ok_or_else(local_job_store::corrupt)?,
    )?
    .checked_add(1)
    .ok_or_else(local_job_store::corrupt)?;
    body["sequence"] = n.to_string().into();
    body["canCancel"] = matches!(
        phase(&body),
        "enumerating"
            | "inspecting"
            | "hashing"
            | "copyingArchives"
            | "listingArchives"
            | "extractingArchives"
            | "ready"
    )
    .into();
    bounded(body)
}
fn fail_body(mut body: JsonValue, p: &str, e: Option<&NativeError>) -> JsonValue {
    body["phase"] = p.into();
    body["error"] = e
        .map(|e| object! {kind:e.kind.clone(),message:e.message.clone()})
        .unwrap_or(JsonValue::Null);
    body
}
struct Job {
    input: String,
    body: JsonValue,
    prepared: Option<LocalPrepared>,
    plan: Option<LocalImportPlan>,
}
enum PreparedJob {
    Import(LocalPrepared),
    Options(PreparedOptions),
}
impl PreparedJob {
    fn kind(&self) -> JobKind {
        match self {Self::Import(_)=>JobKind::Import,Self::Options(_)=>JobKind::Options}
    }
}
struct Worker {
    id: String,
    control: PrepareControl,
    handle: Option<JoinHandle<Result<PreparedJob, NativeError>>>,
    result: Option<Result<PreparedJob, NativeError>>,
}
pub struct LocalJobs {
    root: PathBuf,
    db: LocalJobStore,
    jobs: BTreeMap<String, Job>,
    worker: Option<Worker>,
    option_plans: BTreeMap<String,OptionAttachmentPlan>,
    option_prepared: BTreeMap<String,PreparedOptions>,
    observed: Option<u64>,
    leases: BTreeMap<String, Arc<QuarantineLease>>,
    cleanup_targets: BTreeMap<String, JsonValue>,
    pending_registry: bool,
    cleanup_error: Option<NativeError>,
    slot: Option<Arc<PreparationSlotLease>>,
    #[cfg(test)]
    before_return: Option<std::sync::Arc<dyn Fn(&PrepareControl) + Send + Sync>>,
}
/// Read-only version admission, not recovery or an idle-journal requirement.
/// Admit the complete set before changing jobs or private quarantine state;
/// supported pending transactions still reconcile through their durable outcome.
fn admitted_library(root: &Path) -> Result<Store, NativeError> {
    let library = Store::open(root)?;
    let journals=library.journals()?;
    crate::font_transaction::admit_set(&library,&journals)?;
    if journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])) {
        return Err(NativeError::new("recoveryRequired","Remote payload recovery owns this journal set."));
    }
    Ok(library)
}
impl LocalJobs {
    /// Attaches existing history only. A missing database has no side effects.
    pub fn open(root: &Path) -> Result<Option<Self>, NativeError> {
        let root = canonical_root(root)?;
        // Reject a newer library barrier before history reconciliation or any
        // private quarantine cleanup. Missing history remains side-effect free.
        if root.join("local-jobs.sqlite3").try_exists()? {admitted_library(&root)?;}
        let Some(db) = LocalJobStore::open(&root, false)? else {
            return Ok(None);
        };
        let slot = Arc::new(PreparationSlot::try_acquire(&root)?);
        Self::load(root, db, slot).map(Some)
    }
    /// Called only for an explicit prepare. Validate the existing library before creating history.
    pub fn create(request: &LocalPrepareRequest) -> Result<Self, NativeError> {
        admitted_input(request)?;
        let root = canonical_root(&request.root)?;
        admitted_library(&root)?;
        let lock = RootLock::acquire(&root)?;
        let (name, _) = profile(&root, &request.profile_id, request.expected_revision)?;
        bounded(initial(request, "00000000000000000000000000000000", &name))?;
        let slot = Arc::new(PreparationSlot::try_acquire(&root)?);
        let db = LocalJobStore::open(&root, true)?.ok_or_else(local_job_store::corrupt)?;
        drop(lock);
        Self::load(root, db, slot)
    }
    pub(crate) fn create_options(request: &OptionsPrepareRequest) -> Result<Self,NativeError> {
        let root=canonical_root(&request.root)?;
        admitted_library(&root)?;
        let lock=RootLock::acquire(&root)?;
        option_sources::pinned_record(&root,&request.profile_id,&request.mod_id,request.expected_revision)?;
        let slot=Arc::new(PreparationSlot::try_acquire(&root)?);
        let db=LocalJobStore::open(&root,true)?.ok_or_else(local_job_store::corrupt)?;
        drop(lock); Self::load(root,db,slot)
    }
    fn load(root: PathBuf, db: LocalJobStore, slot: Arc<PreparationSlotLease>) -> Result<Self, NativeError> {
        admitted_library(&root)?;
        let rows = db.load()?;
        let mut jobs = BTreeMap::new();
        // Validate the entire retained set before any recovery writes.
        for (id, input, body) in rows {
            validate_record_version(&root, &id, &input, &body, db.version()?)?;
            if db.version()? == 1
                && matches!(
                    phase(&body),
                    "copyingArchives" | "listingArchives" | "extractingArchives"
                )
            {
                return Err(local_job_store::corrupt());
            }
            jobs.insert(
                id,
                Job {
                    input,
                    body,
                    prepared: None,
                    plan: None,
                },
            );
        }
        let mut updates = Vec::new();
        for (id, j) in &jobs {
            let p = phase(&j.body);
            let body = if preparing(p) || p == "ready" {
                Some(fail_body(
                    j.body.clone(),
                    "interrupted",
                    Some(&error(
                        "localPreparationInterrupted",
                        "The previous session ended; prepare a new attempt.",
                    )),
                ))
            } else if p == "committing" {
                Some(reconcile_body(&root, &j.body, None)?)
            } else {
                None
            };
            if let Some(body) = body {
                updates.push((id.clone(), j.input.clone(), next(body)?));
            }
        }
        if !updates.is_empty() {
            db.save(&updates)?;
            for (id, _, body) in updates {
                jobs.get_mut(&id).unwrap().body = body;
            }
        }
        // All rows, including committing operations, were reconciled before cleanup.
        let cleanup_error =
            recover_quarantines(&QuarantineAuthority::for_held_jobs_store(&db)).err();
        let mut result = Self {
            root,
            db,
            jobs,
            worker: None,
            option_plans: BTreeMap::new(), option_prepared: BTreeMap::new(),
            observed: None,
            leases: BTreeMap::new(),
            cleanup_targets: BTreeMap::new(),
            pending_registry: cleanup_error.is_some(),
            cleanup_error,
            slot: Some(slot),
            #[cfg(test)]
            before_return: None,
        };
        result.release_idle_slot();
        Ok(result)
    }
    fn ensure_slot(&mut self) -> Result<Arc<PreparationSlotLease>, NativeError> {
        if let Some(slot) = &self.slot { slot.validate()?; return Ok(slot.clone()); }
        let slot = Arc::new(PreparationSlot::try_acquire(&self.root)?);
        self.slot = Some(slot.clone());
        Ok(slot)
    }
    fn release_idle_slot(&mut self) {
        if self.worker.is_none() && self.leases.is_empty() && self.cleanup_targets.is_empty()
            && !self.pending_registry && self.option_plans.is_empty() && self.option_prepared.is_empty()
            && self.jobs.values().all(|j| j.prepared.is_none() && j.plan.is_none() && phase(&j.body) != "committing")
            && self.slot.as_ref().is_some_and(|slot| Arc::strong_count(slot) == 1) { self.slot = None; }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// No I/O on unrelated native successes; persist this observation on the next local response.
    pub fn note_revision(&mut self, revision: u64) {
        // Mutation replays carry historical snapshots. They must not overwrite
        // a newer revision already observed in this same bound library.
        let latest = self
            .jobs
            .values()
            .filter_map(|j| {
                j.body["observedRevision"]
                    .as_str()
                    .and_then(|r| store::revision(r).ok())
            })
            .chain(self.observed)
            .max()
            .unwrap_or(0);
        if revision >= latest {
            self.observed = Some(revision);
        }
    }
    fn admit_history(&self) -> Result<(), NativeError> {
        let version=self.db.version()?;
        if !matches!(version,1|2|3) {return Err(local_job_store::corrupt());}
        for (id,input,body) in self.db.load()? {validate_record_version(&self.root,&id,&input,&body,version)?;}
        Ok(())
    }
    fn require_purpose(&self, id: &str, expected: JobKind) -> Result<(),NativeError> {
        let job=self.jobs.get(id).ok_or_else(unknown)?;
        job_purpose(&job.body)?;
        if purpose(&job.body)!=expected {return Err(unknown());} Ok(())
    }
    fn persist(&mut self, id: &str, body: JsonValue) -> Result<(), NativeError> {
        admitted_library(&self.root)?;
        self.admit_history()?;
        let job = self.jobs.get(id).ok_or_else(unknown)?;
        let body = bounded(body)?;
        self.db
            .save(&[(id.into(), job.input.clone(), body.clone())])?;
        self.jobs.get_mut(id).unwrap().body = body;
        Ok(())
    }
    fn observe(&mut self) -> Result<(), NativeError> {
        let _slot = self.ensure_slot()?;
        admitted_library(&self.root)?;
        self.admit_history()?;
        if let Some(revision) = self.observed {
            let mut updates = Vec::new();
            for (id, j) in &self.jobs {
                if j.body["observedRevision"] != revision.to_string() {
                    let mut b = j.body.clone();
                    b["observedRevision"] = revision.to_string().into();
                    updates.push((id.clone(), j.input.clone(), next(b)?));
                }
            }
            if !updates.is_empty() {
                self.db.save(&updates)?;
                for (id, _, b) in updates {
                    self.jobs.get_mut(&id).unwrap().body = b;
                }
            }
            self.observed = None;
        }
        let Some(w) = self.worker.as_mut() else {
            return Ok(());
        };
        if w.handle.as_ref().is_some_and(|h| h.is_finished()) {
            w.result = Some(w.handle.take().unwrap().join().unwrap_or_else(|_| {
                Err(error(
                    "localPreparationFailed",
                    "The preparation worker stopped unexpectedly.",
                ))
            }));
        }
        let id = w.id.clone();
        let mut body = self.jobs[&id].body.clone();
        if phase(&body) != "cancelRequested" {
            let p = w.control.progress();
            if matches!(
                p.phase.as_str(),
                "enumerating"
                    | "inspecting"
                    | "hashing"
                    | "copyingArchives"
                    | "listingArchives"
                    | "extractingArchives"
            ) {
                body["phase"] = p.phase.into();
            }
            body["filesDone"] = p.files_done.min(10_000).into();
            body["filesTotal"] = p.files_total.map(|n| n.min(10_000)).into();
            body["bytesDone"] = p.bytes_done.min(MAX_BYTES).into();
            body["bytesTotal"] = p.bytes_total.map(|n| n.min(MAX_BYTES)).into();
        }
        if let Some(result) = &w.result {
            body = if phase(&body) == "cancelRequested" {
                fail_body(body, "cancelled", None)
            } else {
                match result {
                    Ok(prepared) if prepared.kind()==purpose(&body) => fail_body(body, "ready", None),
                    Ok(_) => fail_body(body,"failed",Some(&local_job_store::corrupt())),
                    Err(e) if e.kind == "localPreparationCancelled" => {
                        fail_body(body, "cancelled", None)
                    }
                    Err(e) => fail_body(body, "failed", Some(e)),
                }
            };
            if phase(&body) == "ready" {
                self.persist(&id, next(body)?)?;
                let w = self.worker.take().unwrap();
                match w.result.unwrap()? {
                    PreparedJob::Import(prepared)=>self.jobs.get_mut(&id).unwrap().prepared=Some(prepared),
                    PreparedJob::Options(prepared)=>{self.option_prepared.insert(id,prepared);},
                }
            } else {
                // Keep durable intent and result authority until the write succeeds.
                let mut pending = self.jobs[&id].body.clone();
                pending["phase"] = "cancelRequested".into();
                self.persist(&id, next(pending)?)?;
                self.cleanup_targets.insert(id.clone(), body);
                drop(self.worker.take());
            }
        } else if body != self.jobs[&id].body {
            self.persist(&id, next(body)?)?;
        }
        Ok(())
    }
    pub fn start(
        &mut self,
        request: LocalPrepareRequest,
        id: &str,
    ) -> Result<JsonValue, NativeError> {
        self.start_shared(request, id, None)
    }
    pub(crate) fn start_options(&mut self, request: OptionsPrepareRequest, id: &str) -> Result<JsonValue, NativeError> {
        self.start_shared(request.local_request(),id,Some(request))
    }
    fn start_shared(&mut self, mut request: LocalPrepareRequest, id: &str, option: Option<OptionsPrepareRequest>) -> Result<JsonValue,NativeError> {
        let slot = self.ensure_slot()?;
        let result = self.start_with_slot(&mut request, id, option, slot);
        self.release_idle_slot();
        result
    }
    fn start_with_slot(&mut self, request: &mut LocalPrepareRequest, id: &str, option: Option<OptionsPrepareRequest>, slot: Arc<PreparationSlotLease>) -> Result<JsonValue,NativeError> {
        job_id(id)?;
        if self.jobs.contains_key(id) {self.require_purpose(id,if option.is_some(){JobKind::Options}else{JobKind::Import})?;}
        request.root = canonical_root(&request.root)?;
        if request.root != self.root {
            return Err(NativeError::params());
        }
        let (input, digest) = if let Some(option)=&option {
            let (record,_)=option_sources::pinned_record(&self.root,&option.profile_id,&option.mod_id,option.expected_revision)?;
            let digest=option_sources::record_digest(&record)?;
            (options_input(option,&digest)?,Some(digest))
        } else {(admitted_input(request)?,None)};
        self.observe()?;
        self.after_writer_cleanup()?;
        if let Some(e) = &self.cleanup_error {
            return Err(e.clone());
        }
        if option.is_some() {self.db.upgrade_for_options()?;} else {self.db.upgrade_for_prepare()?;}
        if let Some(j) = self.jobs.get(id) {
            if j.input != input {
                return Err(error(
                    "jobIdConflict",
                    "This job ID belongs to different admitted input.",
                ));
            }
            return Ok(j.body.clone());
        }
        if self.worker.is_some()
            || !self.cleanup_targets.is_empty()
            || self.jobs.values().any(|j| phase(&j.body) == "committing")
        {
            return Err(error(
                "nativeBusy",
                "A local preparation or import is already active.",
            ));
        }
        if self.jobs.len() >= 8 {
            return Err(error(
                "localJobLimit",
                "Dismiss a terminal local job before starting another; eight are retained.",
            ));
        }
        let admission = (|| {
            let claims = inspect_quarantines(&QuarantineAuthority::for_held_jobs_store(&self.db))?;
            if claims.len() != self.leases.len() {
                return Err(error("recoveryRequired", "Quarantine claims changed since this session began; preserve them for recovery."));
            }
            for claim in claims {
                let lease = self.leases.get(&claim.job_id).ok_or_else(|| {
                    error(
                        "recoveryRequired",
                        "An unknown quarantine claim blocks new preparation.",
                    )
                })?;
                if lease.nonce()? != claim.nonce {
                    return Err(error(
                        "recoveryRequired",
                        "Quarantine ownership changed; prepare only after recovery.",
                    ));
                }
                lease.validate_sealed()?;
            }
            Ok(())
        })();
        if let Err(e) = admission {
            self.pending_registry = true;
            self.cleanup_error = Some(e.clone());
            return Err(e);
        }
        let lock = RootLock::acquire(&self.root)?;
        let (name, revision) = profile(&self.root, &request.profile_id, request.expected_revision)?;
        let mut body = initial(request, id, &name);
        let mut owned=None;
        if let Some(option)=&option {
            let (record,layout)=option_sources::pinned_record(&self.root,&option.profile_id,&option.mod_id,option.expected_revision)?;
            if Some(option_sources::record_digest(&record)?)!=digest {return Err(not_ready());}
            if let OptionSource::RetainedArchive{archive_id}=&option.source {
                owned=Some(option_sources::resolve_owned(&self.root,&record,layout,archive_id)?);
            }
            body["kind"]="localOptions".into(); body["modId"]=option.mod_id.clone().into();
            body["beforeRecordDigest"]=digest.clone().unwrap().into();
        }
        let body=bounded(body)?;
        self.db.save(&[(id.into(), input.clone(), body.clone())])?;
        self.jobs.insert(
            id.into(),
            Job {
                input,
                body: body.clone(),
                prepared: None,
                plan: None,
            },
        );
        drop(lock);
        self.observed = Some(revision);
        let lease = match QuarantineLease::create(
            &QuarantineAuthority::for_held_jobs_store(&self.db),
            id,
        ) {
            Ok(lease) => lease,
            Err(e) => {
                self.pending_registry = true;
                self.cleanup_targets.insert(
                    id.into(),
                    fail_body(self.jobs[id].body.clone(), "failed", Some(&e)),
                );
                self.persist(
                    id,
                    next(fail_body(
                        self.jobs[id].body.clone(),
                        "cancelRequested",
                        Some(&e),
                    ))?,
                )?;
                self.after_writer_cleanup()?;
                return Err(e);
            }
        };
        self.leases.insert(id.into(), lease.clone());
        let control = PrepareControl::new();
        let worker_control = control.clone();
        let options_job=option.is_some();
        let request = request.clone();
        #[cfg(test)]
        let hook = self.before_return.clone();
        let handle = thread::Builder::new()
            .name("local-prepare".into())
            .spawn(move || {
                let result = if options_job {
                    option_sources::prepare_options(request,owned,&worker_control,lease).map(|mut prepared| {
                        prepared.retain_preparation_slot(slot); PreparedJob::Options(prepared)
                    })
                } else {
                    local_content::prepare_local_job(request,&worker_control,lease).map(|mut prepared| {
                        prepared.retain_preparation_slot(slot); PreparedJob::Import(prepared)
                    })
                };
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook(&worker_control);
                }
                result
            });
        self.track_worker(id, control, handle)
    }
    fn track_worker(
        &mut self,
        id: &str,
        control: PrepareControl,
        handle: std::io::Result<JoinHandle<Result<PreparedJob, NativeError>>>,
    ) -> Result<JsonValue, NativeError> {
        let body = self.jobs[id].body.clone();
        match handle {
            Ok(handle) => {
                self.worker = Some(Worker {
                    id: id.into(),
                    control,
                    handle: Some(handle),
                    result: None,
                });
                Ok(body)
            }
            Err(e) => {
                // Retain a failed start exactly like a joined reader result. If
                // persistence fails, a later local request retries the transition
                // instead of leaving an apparently running job with no worker.
                self.worker = Some(Worker {
                    id: id.into(),
                    control,
                    handle: None,
                    result: Some(Err(e.into())),
                });
                self.observe()?;
                Ok(self.jobs[id].body.clone())
            }
        }
    }
    pub fn status(
        &mut self,
        id: Option<&str>,
        include_candidates: bool,
    ) -> Result<JsonValue, NativeError> {
        self.status_for(JobKind::Import,id,include_candidates)
    }
    pub(crate) fn status_for(&mut self, expected_purpose: JobKind, id: Option<&str>, include_candidates: bool) -> Result<JsonValue,NativeError> {
        if let Some(id) = id { job_id(id)?; self.require_purpose(id,expected_purpose)?; }
        if include_candidates && id.is_none() {
            return Err(NativeError::params());
        }
        self.observe()?;
        self.after_writer_cleanup()?;
        if let Some(id) = id {
            let job = self.jobs.get(id).ok_or_else(unknown)?;
            let mut body = self.visible_body(&job.body);
            if include_candidates {
                let prepared=match expected_purpose {
                    JobKind::Import=>job.prepared.as_ref(),
                    JobKind::Options=>self.option_prepared.get(id).map(PreparedOptions::prepared),
                };
                let p=prepared.filter(|_|phase(&body)=="ready").ok_or_else(not_ready)?;
                let mut dto = if expected_purpose==JobKind::Options {
                    object!{candidates:p.review_dto()["candidates"].clone()}
                } else {candidate_dto(p, &p.review_dto())?};
                if expected_purpose==JobKind::Import {
                dto["defaultMetadata"] = metadata(&p.default_metadata());
                let ids = p
                    .candidates
                    .iter()
                    .filter(|c| c.package.is_some())
                    .map(|c| c.id.clone())
                    .collect::<Vec<_>>();
                dto["defaultSelectedCandidateIds"] = if ids.len() <= 99 {
                    JsonValue::from(ids)
                } else {
                    json::array![]
                };
                }
                body["candidateSummary"] = dto;
                bulk(&body)?;
            }
            Ok(body)
        } else {
            Ok(JsonValue::from(
                self.jobs
                    .values()
                    .filter(|j| purpose(&j.body)==expected_purpose)
                    .map(|j| self.visible_body(&j.body))
                    .collect::<Vec<_>>(),
            ))
        }
    }
    pub fn review(
        &mut self,
        id: &str,
        selection: LocalSelection,
    ) -> Result<JsonValue, NativeError> {
        job_id(id)?;
        self.require_purpose(id,JobKind::Import)?;
        self.observe()?;
        if self.worker.is_some() {
            return Err(error(
                "nativeBusy",
                "Wait for active preparation before reviewing another job.",
            ));
        }
        let j = self.jobs.get(id).ok_or_else(unknown)?;
        if phase(&j.body) != "ready" {
            return Err(not_ready());
        }
        let expected = store::revision(j.body["expectedRevision"].as_str().unwrap())?;
        let profile_id = j.body["profileId"].as_str().unwrap().to_owned();
        // Every review attempt supersedes the old token, including invalid form input.
        self.jobs.get_mut(id).unwrap().plan = None;
        let lock = RootLock::acquire(&self.root)?;
        let library = Store::open(&self.root)?;
        let doc = library.document()?;
        let revision = store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?;
        self.observed = Some(revision);
        self.observe()?;
        profile(&self.root, &profile_id, expected)?;
        let prepared = self.jobs[id].prepared.as_ref().ok_or_else(not_ready)?;
        let plan = local_content::review_local(prepared, selection)?;
        let mut dto = candidate_dto(&plan.prepared, &plan.summary)?;
        dto["jobId"] = id.into();
        dto["token"] = plan.token.clone().into();
        dto["profileId"] = profile_id.clone().into();
        dto["profileNameAtStart"] = self.jobs[id].body["profileNameAtStart"].clone();
        dto["expectedRevision"] = expected.to_string().into();
        dto["metadata"] = metadata(&plan.selection.metadata);
        dto["selectedCandidateIds"] = JsonValue::from(plan.selection.candidate_ids.clone());
        let refs = plan.package_refs()?;
        let family_keys = refs
            .iter()
            .map(package::family_key)
            .collect::<Result<HashSet<_>, _>>()?;
        dto["modId"] = plan.mod_id.clone().into();
        dto["selectedPackageCount"] = refs.len().into();
        dto["storedPackageCount"] = family_keys.len().into();
        if family_keys.len() != refs.len() {
            dto["warnings"].push(object!{kind:"duplicateSelectedFamilies",message:"Equal selected content families share one deployment slot. Retained source-name bindings may use separate managed files."}).map_err(|_|NativeError::params())?;
        }
        let version = package::layout_version(&doc)?;
        let overlaps = doc["profiles"][&profile_id]["mods"].members().any(|m| {
            package::mod_packages(m, version).is_ok_and(|ps| {
                ps.iter()
                    .any(|p| package::family_key(p).is_ok_and(|k| family_keys.contains(&k)))
            })
        });
        if overlaps {
            dto["warnings"].push(object!{kind:"existingDuplicateContent",message:"Selected content overlaps an existing mod; existing mods will be preserved."}).map_err(|_|NativeError::params())?;
        }
        for field in ["inactiveOptionCount", "retainedPackageCount", "retainedLogicalPackageBytes", "selectedLogicalPackageBytes", "uniqueCopyBytes", "logicalPackageBytes", "uniquePayloadBytes", "retainedArchiveBytes", "uniqueFontBytes", "fonts", "fontCandidates", "fontDiagnostics"] {
            dto[field] = plan.summary[field].clone();
        }
        if plan.summary["inactiveOptionCount"].as_u64().is_some_and(|n|n>0) {
            dto["warnings"].push(object!{kind:"inactiveChoicesRetained",message:"Inactive choices consume managed storage. Retaining their VPKs does not discover or install their bundled fonts; later font discovery is not available."}).map_err(|_|NativeError::params())?;
        }
        dto["archives"] = plan.summary["archives"].clone();
        bulk(&dto)?;
        let b = next(self.jobs[id].body.clone())?;
        self.persist(id, b)?;
        self.jobs.get_mut(id).unwrap().plan = Some(plan);
        drop(lock);
        Ok(dto)
    }
    pub fn review_group(&mut self,id:&str,rows:&JsonValue)->Result<JsonValue,NativeError> {
        self.observe()?; self.require_purpose(id,JobKind::Import)?;
        self.jobs.get_mut(id).ok_or_else(unknown)?.plan=None;
        if self.worker.is_some() || phase(&self.jobs[id].body)!="ready" {return Err(not_ready());}
        profile(&self.root,self.jobs[id].body["profileId"].as_str().ok_or_else(not_ready)?,store::revision(self.jobs[id].body["expectedRevision"].as_str().ok_or_else(not_ready)?)?)?;
        if !rows.is_array() || rows.is_empty() || rows.len()>64 {return Err(NativeError::params());}
        let prepared=self.jobs[id].prepared.as_ref().ok_or_else(not_ready)?.clone();
        let mut plans=Vec::new();let mut out=Vec::new();let mut used=HashSet::new();let mut row_ids=HashSet::new();
        for row in rows.members() {
            let row_id=row["id"].as_str().filter(|s|!s.is_empty() && s.len()<=256).ok_or_else(NativeError::params)?;
            if !row_ids.insert(row_id) || !row["rootOrdinals"].is_array() {return Err(NativeError::params());}
            let ordinals=row["rootOrdinals"].members().map(|v|v.as_usize().filter(|i|*i<prepared.request.roots.len()).ok_or_else(NativeError::params)).collect::<Result<HashSet<_>,_>>()?;
            if ordinals.is_empty() || ordinals.iter().any(|i|!used.insert(*i)) {return Err(NativeError::params());}
            let mut child=prepared.clone();child.candidates.retain(|c|ordinals.contains(&c.root_ordinal));
            let m=&row["metadata"];let field=|k:&str|m[k].as_str().map(str::to_owned).ok_or_else(NativeError::params);
            let row_metadata=LocalMetadata{name:field("name")?,author:field("author")?,category:field("category")?,description:field("description")?,link:field("link")?};
            let mut ids=child.candidates.iter().filter(|c|c.package.is_some()).map(|c|c.id.clone()).collect::<Vec<_>>();
            ids.sort(); ids.dedup();
            let files=child.review_dto()["candidates"].clone();
            // Installed rows use explicit loose-file roots. Count observed files even
            // when a damaged family cannot become an available candidate.
            let mut loose_files=child.evidence["inventory"].members().filter(|e|e["kind"]=="file" && e["rootOrdinal"].as_usize().is_some_and(|i|ordinals.contains(&i)));
            let bytes=if ordinals.iter().all(|i|prepared.request.roots[*i].extension().is_some_and(|e|e=="vpk")) {
                loose_files.try_fold(0u64,|n,e|n.checked_add(e["identity"]["size"].as_u64().unwrap_or(0)).ok_or_else(NativeError::params))?
            } else {
                files.members().flat_map(|c|c["members"].members()).try_fold(0u64,|n,m|n.checked_add(m["size"].as_u64().unwrap_or(0)).ok_or_else(NativeError::params))?
            };
            let problem=child.candidates.iter().find_map(|c|c.unavailable_reason.clone());
            let reviewed=if problem.is_some() || ids.is_empty() {Err(error("invalidLocalSelection",if problem.is_some() {"Some package files are missing, damaged, or cannot be read. Check the mod’s files and try again."} else {"No packages are available to import."}))} else {local_content::review_local(&child,LocalSelection{candidate_ids:ids,metadata:row_metadata})};
            let plan=match reviewed {Ok(p)=>p,Err(e)=>{out.push(object!{id:row_id,modId:JsonValue::Null,metadata:m.clone(),files:files.clone(),bytes:bytes,problem:object!{kind:e.kind,message:e.message}});continue;}};
            out.push(object!{id:row_id,modId:plan.mod_id.clone(),metadata:metadata(&plan.selection.metadata),files:plan.summary["candidates"].clone(),bytes:bytes,problem:JsonValue::Null});
            plans.push((row_id.to_owned(),plan));
        }
        let valid=plans.len()==rows.len();
        let mut dto=object!{jobId:id,profileId:prepared.request.profile_id.clone(),expectedRevision:prepared.request.expected_revision.to_string(),groupRows:out.clone()};
        if valid {
            let selection=plans[0].1.selection.clone();
            dto=self.review(id,selection)?;
            let mut plan=self.jobs.get_mut(id).unwrap().plan.take().ok_or_else(not_ready)?;
            dto["modId"]=plans[0].1.mod_id.clone().into();
            plan.group=plans;dto["groupRows"]=out.into();
            bulk(&dto)?;
            self.jobs.get_mut(id).unwrap().plan=Some(plan);
        }
        Ok(dto)
    }
    pub fn cancel(&mut self, id: &str) -> Result<JsonValue, NativeError> {
        self.cancel_for(JobKind::Import,id)
    }
    pub(crate) fn cancel_for(&mut self, expected_purpose: JobKind, id: &str) -> Result<JsonValue,NativeError> {
        self.require_purpose(id,expected_purpose)?;
        job_id(id)?;
        // Linearize accepted cancellation before consuming an unobserved successful result.
        let j = self.jobs.get(id).ok_or_else(unknown)?;
        let p = phase(&j.body);
        if p == "committing" {
            return Err(error(
                "cancelTooLate",
                "The import writer is already committing and cannot be cancelled.",
            ));
        }
        if terminal(p) {
            self.observe()?;
            return Ok(self.jobs[id].body.clone());
        }
        if p != "cancelRequested" {
            let body = next(fail_body(j.body.clone(), "cancelRequested", None))?;
            self.persist(id, body)?;
        }
        self.cleanup_targets.insert(
            id.into(),
            fail_body(self.jobs[id].body.clone(), "cancelled", None),
        );
        self.option_plans.remove(id);
        self.option_prepared.remove(id);
        let j = self.jobs.get_mut(id).unwrap();
        j.plan = None;
        j.prepared = None;
        if let Some(w) = &self.worker {
            if w.id == id {
                w.control.request_cancel();
            }
        }
        self.after_writer_cleanup()?;
        Ok(self.visible_body(&self.jobs[id].body))
    }
    /// The caller holds this same writer lock through reserve and the import transaction.
    pub fn reserve_commit(
        &mut self,
        lock: &RootLock,
        id: &str,
        token: &str,
        profile_id: &str,
        expected: u64,
        operation_id: &str,
    ) -> Result<LocalImportPlan, NativeError> {
        if lock.root() != self.root || !paths::safe_id(operation_id) {
            return Err(NativeError::params());
        }
        job_id(id)?;
        self.require_purpose(id,JobKind::Import)?;
        self.observe()?;
        if self.worker.is_some() || self.jobs.values().any(|j| phase(&j.body) == "committing") {
            return Err(error("nativeBusy", "A local operation is active."));
        }
        let j = self.jobs.get(id).ok_or_else(unknown)?;
        let plan = j
            .plan
            .as_ref()
            .filter(|p| {
                phase(&j.body) == "ready"
                    && p.token == token
                    && p.prepared.request.profile_id == profile_id
                    && p.prepared.request.expected_revision == expected
            })
            .ok_or_else(not_ready)?
            .clone();
        let mut body = j.body.clone();
        let doc = Store::open(&self.root)?.document()?;
        let observed = store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?;
        self.note_revision(observed);
        self.observe()?;
        profile(&self.root, profile_id, expected)?;
        body["observedRevision"] = observed.to_string().into();
        body["phase"] = "committing".into();
        body["operationId"] = operation_id.into();
        self.persist(id, next(body)?)?;
        self.option_plans.remove(id);
        self.option_prepared.remove(id);
        let j = self.jobs.get_mut(id).unwrap();
        j.plan = None;
        j.prepared = None;
        Ok(plan)
    }
    pub(crate) fn review_options(&mut self, id: &str, bindings: Vec<OptionBinding>) -> Result<JsonValue,NativeError> {
        job_id(id)?; self.require_purpose(id,JobKind::Options)?;
        self.observe()?;
        if self.worker.is_some() {return Err(error("nativeBusy","Wait for active preparation before reviewing another job."));}
        let body=self.jobs[id].body.clone();
        if phase(&body)!="ready" {return Err(not_ready());}
        self.option_plans.remove(id);
        let lock=RootLock::acquire(&self.root)?;
        let profile=body["profileId"].as_str().unwrap(); let mod_id=body["modId"].as_str().unwrap();
        let expected=store::revision(body["expectedRevision"].as_str().unwrap())?;
        let (before,layout)=option_sources::pinned_record(&self.root,profile,mod_id,expected)?;
        if body["beforeRecordDigest"]!=option_sources::record_digest(&before)? {return Err(not_ready());}
        let prepared=self.option_prepared.get(id).ok_or_else(not_ready)?;
        let plan=option_sources::review(prepared,&before,layout,&bindings)?;
        option_sources::detail_envelope(&plan.record,layout,profile,expected.checked_add(1).ok_or_else(NativeError::params)?)?;
        let response=object!{token:plan.token.clone(),profileId:profile,modId:mod_id,expectedRevision:expected.to_string(),summary:plan.summary.clone()};
        bulk(&response)?;
        self.persist(id,next(body)?)?;
        self.option_plans.insert(id.into(),plan);
        drop(lock); Ok(response)
    }
    pub(crate) fn reserve_attachment(&mut self, lock: &RootLock, id: &str, token: &str,
        profile: &str, mod_id: &str, expected: u64, operation: &str) -> Result<OptionAttachmentPlan,NativeError> {
        if lock.root()!=self.root || !paths::safe_id(operation) {return Err(NativeError::params());}
        job_id(id)?; self.require_purpose(id,JobKind::Options)?;
        self.observe()?;
        if self.worker.is_some() || self.jobs.values().any(|j|phase(&j.body)=="committing") {return Err(error("nativeBusy","A local operation is active."));}
        let body=&self.jobs[id].body;
        if phase(body)!="ready" || body["profileId"]!=profile || body["modId"]!=mod_id || body["expectedRevision"]!=expected.to_string() {return Err(not_ready());}
        let plan=self.option_plans.get(id).filter(|p|p.token==token).ok_or_else(not_ready)?.clone();
        let (record,_)=option_sources::pinned_record(&self.root,profile,mod_id,expected)?;
        if record!=plan.before_record || body["beforeRecordDigest"]!=option_sources::record_digest(&record)? {return Err(not_ready());}
        option_sources::revalidate(&plan.prepared,plan.owned.as_ref(),&PrepareControl::new())?;
        let mut body=body.clone(); body["phase"]="committing".into(); body["operationId"]=operation.into();
        self.persist(id,next(body)?)?;
        self.option_plans.remove(id); self.option_prepared.remove(id);
        self.jobs.get_mut(id).unwrap().prepared=None;
        Ok(plan)
    }
    pub fn finish_commit(
        &mut self,
        id: &str,
        outcome: &Result<JsonValue, NativeError>,
    ) -> Result<JsonValue, NativeError> {
        job_id(id)?;
        let j = self.jobs.get(id).ok_or_else(unknown)?;
        if phase(&j.body) != "committing" {
            return Err(not_ready());
        }
        let mut body = match outcome {
            Ok(_) => fail_body(j.body.clone(), "succeeded", None),
            Err(e) => reconcile_body(&self.root, &j.body, Some(e))?,
        };
        if let Ok(value) = outcome {
            if let Some(r) = value["revision"].as_str() {
                store::revision(r)?;
                body["observedRevision"] = r.into();
            }
        }
        let body = next(body)?;
        self.persist(id, body.clone())?;
        Ok(body)
    }
    fn visible_body(&self, body: &JsonValue) -> JsonValue {
        let mut body = body.clone();
        if let Some(e) = &self.cleanup_error {
            body["cleanupError"] = object! {kind:e.kind.clone(),message:e.message.clone()};
        }
        body
    }
    /// Serial owner only, with no main library writer lock held. Cleanup errors
    /// are status, never an alternate outcome for an already committed operation.
    pub(crate) fn after_writer_cleanup(&mut self) -> Result<(), NativeError> {
        let slot = self.ensure_slot()?;
        admitted_library(&self.root)?;
        self.admit_history()?;
        if self.worker.is_some() {
            return Ok(());
        }
        let committing = self
            .jobs
            .iter()
            .filter(|(_, j)| phase(&j.body) == "committing")
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in committing {
            // A returned commit plan may still hold the lease in direct callers.
            if self
                .leases
                .get(&id)
                .is_some_and(|l| Arc::strong_count(l) > 1)
            {
                continue;
            }
            let body = reconcile_body(&self.root, &self.jobs[&id].body, None)?;
            self.persist(&id, next(body)?)?;
        }
        // Factory errors can leave registry authority without a live Arc. Retry
        // the durable pending transition before consuming that authority as well.
        for id in self.cleanup_targets.keys().cloned().collect::<Vec<_>>() {
            let mut body = self.jobs[&id].body.clone();
            if phase(&body) != "cancelRequested" {
                body["phase"] = "cancelRequested".into();
                body = next(body)?;
            }
            self.persist(&id, body)?;
        }
        let releasable = self
            .leases
            .iter()
            .filter(|(id, lease)| {
                Arc::strong_count(lease) == 1
                    && self.jobs.get(*id).is_some_and(|j| {
                        terminal(phase(&j.body))
                            || phase(&j.body) == "cancelRequested"
                            || (phase(&j.body) == "ready"
                                && j.prepared
                                    .as_ref()
                                    .is_some_and(|p| p.archive_sources().is_empty() && p.font_candidates().iter().all(|f|f.prepared.is_none())))
                    })
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in releasable {
            // Persist the current state before consuming our only live authority.
            self.persist(&id, self.jobs[&id].body.clone())?;
            let lease = self.leases.remove(&id).unwrap();
            if let Err(e) = lease.cleanup(&QuarantineAuthority::for_held_jobs_store(&self.db)) {
                self.pending_registry = true;
                self.cleanup_error = Some(e);
            }
        }
        if self.pending_registry && self.leases.is_empty() {
            match recover_quarantines(&QuarantineAuthority::for_held_jobs_store(&self.db)) {
                Ok(_) => {
                    self.pending_registry = false;
                    self.cleanup_error = None;
                }
                Err(e) => self.cleanup_error = Some(e),
            }
        }
        if !self.pending_registry {
            let ids = self
                .cleanup_targets
                .keys()
                .filter(|id| !self.leases.contains_key(*id))
                .cloned()
                .collect::<Vec<_>>();
            for id in ids {
                let mut body = self.jobs[&id].body.clone();
                body["phase"] = self.cleanup_targets[&id]["phase"].clone();
                body["error"] = self.cleanup_targets[&id]["error"].clone();
                self.persist(&id, next(body)?)?;
                self.cleanup_targets.remove(&id);
            }
        }
        drop(slot);
        self.release_idle_slot();
        Ok(())
    }
    pub fn dismiss(&mut self, id: &str) -> Result<JsonValue, NativeError> {
        self.dismiss_for(JobKind::Import,id)
    }
    pub(crate) fn dismiss_for(&mut self, expected_purpose: JobKind, id: &str) -> Result<JsonValue,NativeError> {
        self.require_purpose(id,expected_purpose)?;
        job_id(id)?;
        self.observe()?;
        self.after_writer_cleanup()?;
        if let Some(e) = &self.cleanup_error {
            return Err(e.clone());
        }
        let j = self.jobs.get(id).ok_or_else(unknown)?;
        if !terminal(phase(&j.body)) {
            return Err(error(
                "localJobNotTerminal",
                "Cancel or finish this local job before dismissing it.",
            ));
        }
        if let Some(op) = j.body["operationId"].as_str() {
            if Store::open_history(&self.root)?.journal(op)?.is_some() {
                return Err(error(
                    "recoveryRequired",
                    "Resolve the import journal before dismissing this job.",
                ));
            }
        }
        if self.leases.contains_key(id) || self.cleanup_targets.contains_key(id) || self.pending_registry {
            return Err(error(
                "recoveryRequired",
                "Release retained local plans and finish quarantine cleanup before dismissing this job.",
            ));
        }
        self.db.delete(id)?;
        self.jobs.remove(id);
        self.option_plans.remove(id); self.option_prepared.remove(id);
        Ok(object! {jobId:id,dismissed:true})
    }
}
impl Drop for LocalJobs {
    fn drop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.control.request_cancel();
            if let Some(h) = w.handle.take() {
                drop(h.join());
            }
            drop(w.result.take());
        }
        self.option_plans.clear();
        self.option_prepared.clear();
        for j in self.jobs.values_mut() {
            j.plan = None;
            j.prepared = None;
        }
        // Releasing in-memory handles does not unlink their durable claims.
        // Unknown artifacts stop both interrupted-job persistence and cleanup.
        if admitted_library(&self.root).is_err() || self.admit_history().is_err() {return;}
        let ids = self
            .jobs
            .iter()
            .filter(|(_, j)| preparing(phase(&j.body)) || phase(&j.body) == "ready")
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            if let Ok(b) = next(fail_body(
                self.jobs[&id].body.clone(),
                "interrupted",
                Some(&error(
                    "localPreparationInterrupted",
                    "The helper session ended; prepare a new attempt.",
                )),
            )) {
                if self.persist(&id, b).is_err() {
                    return;
                }
            }
        }
        let _ = self.after_writer_cleanup();
    }
}
/// Reading an outcome never changes the library. A lookup error is not absence.
fn reconcile_body(
    root: &Path,
    body: &JsonValue,
    returned: Option<&NativeError>,
) -> Result<JsonValue, NativeError> {
    let library = admitted_library(root)?;
    let id = body["operationId"]
        .as_str()
        .ok_or_else(local_job_store::corrupt)?;
    let operation = library.operation(id)?;
    Ok(match operation["status"].as_str() {
        Some("succeeded") => {
            let mut result = fail_body(body.clone(), "succeeded", None);
            if let Some(revision) = operation["result"]["revision"].as_str() {
                store::revision(revision)?; result["observedRevision"] = revision.into();
            }
            result
        }
        Some("failed") => fail_body(body.clone(), "failed", Some(&error(
            operation["error"]["kind"].as_str().ok_or_else(local_job_store::corrupt)?,
            operation["error"]["message"].as_str().ok_or_else(local_job_store::corrupt)?))),
        Some("recoveryRequired") => fail_body(body.clone(), "recoveryRequired", Some(&error("recoveryRequired", "The import has an unresolved journal; use native recovery."))),
        Some("unknown") => match returned {
            Some(e) => fail_body(body.clone(), "failed", Some(e)),
            None => fail_body(body.clone(), "interrupted", Some(&error("localOutcomeUnknown", "No durable outcome or journal was found. The outcome is unknown; never retry this writer automatically."))),
        },
        _ => return Err(local_job_store::corrupt()),
    })
}

fn initial(r: &LocalPrepareRequest, id: &str, name: &str) -> JsonValue {
    object! {jobId:id,kind:"localImport",jobVersion:1,profileId:r.profile_id.clone(),profileNameAtStart:name,expectedRevision:r.expected_revision.to_string(),observedRevision:r.expected_revision.to_string(),sequence:"1",phase:"enumerating",filesDone:0,filesTotal:JsonValue::Null,bytesDone:0,bytesTotal:JsonValue::Null,canCancel:true,operationId:JsonValue::Null,error:JsonValue::Null}
}
fn options_input(request: &OptionsPrepareRequest, digest: &str) -> Result<String,NativeError> {
    if !paths::safe_id(&request.profile_id) || !paths::safe_id(&request.mod_id) || !package::hex(digest,64) {return Err(NativeError::params());}
    let value=object!{root:path_text(&request.root)?,purpose:"options",profileId:request.profile_id.clone(),modId:request.mod_id.clone(),
        expectedRevision:request.expected_revision.to_string(),beforeRecordDigest:digest,source:request.source.encode()};
    let input=store::canonical(&value); if input.len()>64*1024 {return Err(NativeError::params());} Ok(input)
}
fn validate_record_version(root: &Path,id: &str,input: &str,body: &JsonValue,version: u64) -> Result<(),NativeError> {
    if version==1 && matches!(phase(body),"copyingArchives"|"listingArchives"|"extractingArchives") {return Err(local_job_store::corrupt());}
    if body["kind"]!="localOptions" {return validate_record(root,id,input,body);}
    if version!=3 || body.len()!=18 {return Err(local_job_store::corrupt());}
    let JobPurpose::Options{profile_id,mod_id,before_record_digest}=job_purpose(body)? else {return Err(local_job_store::corrupt());};
    let v=json::parse(input).map_err(|_|local_job_store::corrupt())?;
    let request=OptionsPrepareRequest {root:root.into(),profile_id,mod_id,expected_revision:store::revision(body["expectedRevision"].as_str().ok_or_else(local_job_store::corrupt)?)?,
        source:OptionSource::decode(&v["source"]).map_err(|_|local_job_store::corrupt())?};
    if options_input(&request,&before_record_digest)?!=input {return Err(local_job_store::corrupt());}
    // Validate the identical lifecycle fields with the unchanged strict Import codec.
    let mut legacy=body.clone(); legacy.remove("modId");legacy.remove("beforeRecordDigest");legacy["kind"]="localImport".into();
    validate_import_body(id,&legacy)
}
fn validate_import_body(id: &str, b: &JsonValue) -> Result<(), NativeError> {
    let bad = local_job_store::corrupt;
    job_id(id).map_err(|_| bad())?;
    if !b.is_object()
        || b.len() != 16
        || [
            "jobId",
            "kind",
            "jobVersion",
            "profileId",
            "profileNameAtStart",
            "expectedRevision",
            "observedRevision",
            "sequence",
            "phase",
            "filesDone",
            "filesTotal",
            "bytesDone",
            "bytesTotal",
            "canCancel",
            "operationId",
            "error",
        ]
        .iter()
        .any(|key| !b.has_key(key))
        || b["jobId"] != id
        || b["kind"] != "localImport"
        || b["jobVersion"] != 1
        || !b["profileNameAtStart"].is_string()
        || !b["profileId"].as_str().is_some_and(paths::safe_id)
        || b.dump().len() > 4096
    {
        return Err(bad());
    }
    for k in ["expectedRevision", "observedRevision", "sequence"] {
        store::revision(b[k].as_str().ok_or_else(bad)?).map_err(|_| bad())?;
    }
    if b["sequence"] == "0"
        || !(preparing(phase(b))
            || terminal(phase(b))
            || matches!(phase(b), "ready" | "committing"))
    {
        return Err(bad());
    }
    if b["canCancel"].as_bool()
        != Some(matches!(
            phase(b),
            "enumerating"
                | "inspecting"
                | "hashing"
                | "copyingArchives"
                | "listingArchives"
                | "extractingArchives"
                | "ready"
        ))
    {
        return Err(bad());
    }
    for (k, max, nullable) in [
        ("filesDone", 10000, false),
        ("filesTotal", 10000, true),
        ("bytesDone", MAX_BYTES, false),
        ("bytesTotal", MAX_BYTES, true),
    ] {
        if !(nullable && b[k].is_null()) && !b[k].as_u64().is_some_and(|n| n <= max) {
            return Err(bad());
        }
    }
    if !b["operationId"].is_null() && !b["operationId"].as_str().is_some_and(paths::safe_id) {
        return Err(bad());
    }
    if phase(b) == "committing" && b["operationId"].is_null() {
        return Err(bad());
    }
    if !b["error"].is_null()
        && (!b["error"].is_object()
            || b["error"].len() != 2
            || !b["error"]["kind"].is_string()
            || !b["error"]["message"].is_string())
    {
        return Err(bad());
    }
    Ok(())
}
fn validate_record(root: &Path, id: &str, input: &str, b: &JsonValue) -> Result<(), NativeError> {
    validate_import_body(id,b)?;
    let bad=local_job_store::corrupt;
    let value=json::parse(input).map_err(|_|bad())?;
    let roots = value["roots"]
        .members()
        .map(|v| v.as_str().map(PathBuf::from).ok_or_else(bad))
        .collect::<Result<Vec<_>, _>>()?;
    let request = LocalPrepareRequest {
        root: root.into(),
        profile_id: b["profileId"].as_str().unwrap().into(),
        expected_revision: store::revision(b["expectedRevision"].as_str().unwrap())?,
        roots,
        artwork: if value["artwork"].is_null() {
            None
        } else {
            Some(PathBuf::from(value["artwork"].as_str().ok_or_else(bad)?))
        },
    };
    if admitted_input(&request).map_err(|_| bad())? != input {
        return Err(bad());
    }
    Ok(())
}
fn bulk(value: &JsonValue) -> Result<(), NativeError> {
    if protocol::native_value_bytes(value).is_none() {
        Err(error(
            "nativeResponseLimit",
            "The complete local review exceeds the readable response limit.",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn metadata(m: &LocalMetadata) -> JsonValue {
    object! {name:m.name.clone(),author:m.author.clone(),category:m.category.clone(),link:m.link.clone(),description:m.description.clone()}
}
fn candidate_dto(p: &LocalPrepared, source: &JsonValue) -> Result<JsonValue, NativeError> {
    // These are the producer's public display DTO and typed fingerprint lookup, not evidence decoding.
    let inventory = &source["inventory"];
    let mut roots = Vec::new();
    for (i, path) in p.request.roots.iter().enumerate() {
        let entry = inventory
            .members()
            .find(|v| v["rootOrdinal"].as_usize() == Some(i) && v["path"].as_str() == path.to_str())
            .ok_or_else(local_job_store::corrupt)?;
        roots.push(object!{rootOrdinal:i,path:path.to_str().unwrap(),kind:if entry["kind"]=="directory"{"folder"}else{"file"}});
    }
    let mut exclusions=inventory.members().filter(|v|v["exclusion"].is_string()).map(|v|object!{rootOrdinal:v["rootOrdinal"].clone(),relativePath:v["relativePath"].clone(),reason:v["exclusion"].clone()}).collect::<Vec<_>>();
    exclusions.extend(source["provenance"].members().filter(|v| v["archive"].is_object() && v["exclusion"].is_string()).map(|v| object! {rootOrdinal:v["rootOrdinal"].clone(),relativePath:v["relativePath"].clone(),archiveRelativePath:v["archiveRelativePath"].clone(),sourcePath:v["path"].clone(),archive:v["archive"].clone(),reason:v["exclusion"].clone()}));
    let mut candidates = source["candidates"].clone();
    for c in candidates.members_mut() {
        c.remove("companionCount");
        if c["members"].is_null() {
            c["members"] = json::array![];
        }
        for m in c["members"].members_mut() {
            m["role"] = if m["archiveIndex"].is_null() {
                "directory"
            } else {
                "companion"
            }
            .into();
        }
    }
    let artwork=p.artwork.as_ref().map(|a|object!{path:a.path.to_str().unwrap(),format:a.format.clone(),sha256:a.fingerprint.sha256.clone(),size:a.fingerprint.size,device:a.fingerprint.device,inode:a.fingerprint.inode,modifiedSeconds:a.fingerprint.modified_seconds,modifiedNanos:a.fingerprint.modified_nanos}).unwrap_or(JsonValue::Null);
    let mut dto=object! {sources:JsonValue::from(roots),candidates:candidates,exclusions:JsonValue::from(exclusions),warnings:json::array![],sourceArtwork:artwork};
    if source.has_key("fontCandidates") {dto["fontCandidates"]=source["fontCandidates"].clone();}
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, Fixture};
    use std::{
        sync::{Arc, Barrier},
        time::{Duration, Instant},
    };
    pub(super) const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    struct Release(Option<Arc<Barrier>>);
    impl Release {
        fn release(&mut self) {
            if let Some(barrier) = self.0.take() {
                barrier.wait();
            }
        }
    }
    impl Drop for Release {
        fn drop(&mut self) {
            self.release();
        }
    }
    pub(super) fn setup() -> (Fixture, LocalJobs, LocalPrepareRequest) {
        let f = Fixture::new();
        let mut service = crate::native::NativeService::new();
        test_support::import(&mut service, &f);
        let source = f.root.join("prepared.vpk");
        std::fs::write(&source, test_support::vpk("models/job.vmdl_c", 12)).unwrap();
        let r = LocalPrepareRequest {
            root: f.data.clone(),
            profile_id: "default".into(),
            expected_revision: 1,
            roots: vec![source],
            artwork: None,
        };
        let jobs = LocalJobs::create(&r).unwrap();
        (f, jobs, r)
    }
    pub(super) fn finished(jobs: &LocalJobs) {
        let until = Instant::now() + Duration::from_secs(10);
        while !jobs
            .worker
            .as_ref()
            .unwrap()
            .handle
            .as_ref()
            .unwrap()
            .is_finished()
        {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(2));
        }
    }
    pub(super) fn ready(jobs: &mut LocalJobs, r: LocalPrepareRequest) {
        jobs.start(r, ID).unwrap();
        finished(jobs);
        assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "ready");
    }
    pub(super) fn select(jobs: &LocalJobs) -> LocalSelection {
        let p = jobs.jobs[ID].prepared.as_ref().unwrap();
        let mut candidate_ids = p
            .candidates
            .iter()
            .filter(|c| c.package.is_some())
            .map(|c| c.id.clone())
            .collect::<Vec<_>>();
        candidate_ids.sort();
        LocalSelection {
            candidate_ids,
            metadata: p.default_metadata(),
        }
    }
    #[test]
    fn local_job_completed_reader_carries_final_counts() {
        let (_f, mut jobs, r) = setup();
        jobs.start(r, ID).unwrap();
        finished(&jobs);
        let b = jobs.status(Some(ID), false).unwrap();
        assert_eq!(b["phase"], "ready");
        assert_eq!(b["filesDone"], 1);
        assert_eq!(b["filesTotal"], 1);
        assert!(b["bytesDone"].as_u64().unwrap() > 0);
        assert_eq!(b["bytesDone"], b["bytesTotal"]);
    }
    #[test]
    fn local_job_observes_exit_and_late_progress_never_revives_cancel() {
        let (_f, mut jobs, r) = setup();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (a, b) = (entered.clone(), release.clone());
        jobs.before_return = Some(Arc::new(move |control| {
            a.wait();
            b.wait();
            control.report(crate::prepare_control::PrepareProgress {
                phase: "ready".into(),
                files_done: 999,
                ..Default::default()
            });
        }));
        jobs.start(r, ID).unwrap();
        entered.wait();
        let mut release = Release(Some(release));
        assert_ne!(jobs.status(Some(ID), false).unwrap()["phase"], "ready");
        assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelRequested");
        assert_eq!(
            jobs.status(Some(ID), false).unwrap()["phase"],
            "cancelRequested"
        );
        release.release();
        finished(&jobs);
        let b = jobs.status(Some(ID), false).unwrap();
        assert_eq!(b["phase"], "cancelled");
        assert_ne!(b["filesDone"], 999);
        assert!(jobs.worker.is_none());
        assert!(jobs.jobs[ID].prepared.is_none());
    }
    #[test]
    fn local_job_finished_result_survives_database_failure() {
        let (_f, mut jobs, r) = setup();
        jobs.start(r, ID).unwrap();
        finished(&jobs);
        jobs.db.fail_writes = true;
        assert!(jobs.status(Some(ID), false).is_err());
        assert_ne!(phase(&jobs.jobs[ID].body), "ready");
        assert!(jobs.worker.as_ref().unwrap().handle.is_none());
        assert!(jobs.worker.as_ref().unwrap().result.is_some());
        jobs.db.fail_writes = false;
        assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "ready");
        assert!(jobs.worker.is_none());
    }
    #[test]
    fn local_job_persisted_cancel_and_reserve_linearize() {
        let (f, mut jobs, r) = setup();
        ready(&mut jobs, r);
        let selection = select(&jobs);
        let dto = jobs.review(ID, selection.clone()).unwrap();
        let old = dto["token"].as_str().unwrap();
        let new = jobs.review(ID, selection).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        assert!(jobs
            .reserve_commit(&lock, ID, old, "default", 1, "local-op")
            .is_err());
        jobs.db.fail_writes = true;
        assert!(jobs
            .reserve_commit(
                &lock,
                ID,
                new["token"].as_str().unwrap(),
                "default",
                1,
                "local-op"
            )
            .is_err());
        assert_eq!(phase(&jobs.jobs[ID].body), "ready");
        jobs.db.fail_writes = false;
        let plan = jobs
            .reserve_commit(
                &lock,
                ID,
                new["token"].as_str().unwrap(),
                "default",
                1,
                "local-op",
            )
            .unwrap();
        assert_eq!(plan.prepared.request.profile_id, "default");
        assert_eq!(jobs.cancel(ID).unwrap_err().kind, "cancelTooLate");
        jobs.db.fail_writes = true;
        assert!(jobs.finish_commit(ID, &Ok(object! {revision:"2"})).is_err());
        assert_eq!(phase(&jobs.jobs[ID].body), "committing");
        jobs.db.fail_writes = false;
        assert_eq!(
            jobs.finish_commit(ID, &Ok(object! {revision:"2"})).unwrap()["phase"],
            "succeeded"
        );
        assert_eq!(jobs.cancel(ID).unwrap()["phase"], "succeeded");
    }
    #[test]
    fn local_job_stale_reserve_persists_actual_observed_revision() {
        let (f, mut jobs, r) = setup();
        ready(&mut jobs, r);
        let dto = jobs.review(ID, select(&jobs)).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        store
            .commit(
                &lock,
                1,
                "external-edit",
                &object! {},
                store.document().unwrap(),
                None,
            )
            .unwrap();
        assert_eq!(
            jobs.reserve_commit(
                &lock,
                ID,
                dto["token"].as_str().unwrap(),
                "default",
                1,
                "local-op"
            )
            .unwrap_err()
            .kind,
            "staleRevision"
        );
        assert_eq!(
            jobs.status(Some(ID), false).unwrap()["observedRevision"],
            "2"
        );
        assert_eq!(jobs.jobs[ID].body["phase"], "ready");
        assert!(jobs.jobs[ID].plan.is_some());
        assert_eq!(store.operation("local-op").unwrap()["status"], "unknown");
        drop(jobs);
        let mut reopened = LocalJobs::open(&f.data).unwrap().unwrap();
        assert_eq!(
            reopened.status(Some(ID), false).unwrap()["observedRevision"],
            "2"
        );
    }
    #[test]
    fn local_job_cancel_before_reserve_prevents_plan_return() {
        let (f, mut jobs, r) = setup();
        ready(&mut jobs, r);
        let dto = jobs.review(ID, select(&jobs)).unwrap();
        jobs.cancel(ID).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        assert!(jobs
            .reserve_commit(
                &lock,
                ID,
                dto["token"].as_str().unwrap(),
                "default",
                1,
                "never-dispatched"
            )
            .is_err());
        assert_eq!(
            Store::open_history(&f.data)
                .unwrap()
                .operation("never-dispatched")
                .unwrap()["status"],
            "unknown"
        );
    }
    #[test]
    fn local_job_status_budget_preserves_mandatory_fields() {
        let (_f, _jobs, r) = setup();
        let mut b = initial(&r, ID, &"界\u{0001}".repeat(3000));
        b["error"] = object! {kind:"nativeIo",message:"錯\u{0002}".repeat(3000)};
        let out = bounded(b).unwrap();
        assert!(out.dump().len() <= 4096);
        assert_eq!(out["jobId"], ID);
        assert_eq!(out["profileId"], "default");
        assert!(out["profileNameAtStart"].as_str().unwrap().ends_with('…'));
        assert!(out["error"]["message"].as_str().unwrap().ends_with('…'));
        let mut b = initial(&r, ID, "x");
        b["profileId"] = "x".repeat(5000).into();
        assert!(bounded(b).is_err());
    }
    #[test]
    fn local_job_failed_cancel_is_not_acknowledged_or_published() {
        let (_f, mut jobs, r) = setup();
        ready(&mut jobs, r);
        jobs.db.fail_writes = true;
        assert!(jobs.cancel(ID).is_err());
        assert_eq!(phase(&jobs.jobs[ID].body), "ready");
        jobs.db.fail_writes = false;
        assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelled");
    }
    #[test]
    fn local_job_finish_error_uses_authoritative_outcome_before_journal() {
        for (op, expected, journal, rejected) in [
            ("import-1", "succeeded", true, false),
            ("rejected-op", "failed", true, true),
            ("journal-op", "recoveryRequired", true, false),
            ("known-error", "failed", false, false),
        ] {
            let (f, mut jobs, r) = setup();
            ready(&mut jobs, r);
            let dto = jobs.review(ID, select(&jobs)).unwrap();
            let lock = RootLock::acquire(&f.data).unwrap();
            jobs.reserve_commit(&lock, ID, dto["token"].as_str().unwrap(), "default", 1, op)
                .unwrap();
            let library = Store::open_writer(&lock).unwrap();
            if rejected {
                library
                    .record_failure(
                        &lock,
                        op,
                        &object! {},
                        &error("rejected", "durable rejection"),
                    )
                    .unwrap();
            }
            if journal {
                library
                    .write_journal(&lock, op, &object! {kind:"import",stage:".import-0123456789abcdef0123456789abcdef",input:object!{method:"native.importCommit",params:object!{}},payloads:json::array![]})
                    .unwrap();
            }
            let b = jobs
                .finish_commit(ID, &Err(error("nativeIo", "delivery or copy error")))
                .unwrap();
            assert_eq!(b["phase"], expected, "{op}");
            if journal {
                assert_eq!(jobs.dismiss(ID).unwrap_err().kind, "recoveryRequired");
            }
        }
    }
    #[test]
    fn local_job_finish_lookup_failure_retains_committing() {
        let (f, mut jobs, r) = setup();
        ready(&mut jobs, r);
        let dto = jobs.review(ID, select(&jobs)).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        jobs.reserve_commit(
            &lock,
            ID,
            dto["token"].as_str().unwrap(),
            "default",
            1,
            "unresolved-op",
        )
        .unwrap();
        std::fs::rename(
            f.data.join("library.sqlite3"),
            f.data.join("saved-library.sqlite3"),
        )
        .unwrap();
        assert!(jobs
            .finish_commit(ID, &Err(error("nativeIo", "copy error")))
            .is_err());
        assert_eq!(phase(&jobs.jobs[ID].body), "committing");
    }
    #[test]
    fn local_job_missing_nullable_status_fields_are_not_valid_history() {
        let (_f, jobs, r) = setup();
        for key in ["error", "operationId"] {
            let mut b = initial(&r, ID, "Default");
            b.remove(key);
            b["extra"] = JsonValue::Null;
            assert!(validate_record(jobs.root(), ID, &admitted_input(&r).unwrap(), &b).is_err());
        }
    }
    #[test]
    fn local_job_review_does_not_block_another_preparation_and_panic_is_durable() {
        let (_f, mut jobs, r) = setup();
        ready(&mut jobs, r.clone());
        let selection = select(&jobs);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (a, b) = (entered.clone(), release.clone());
        jobs.before_return = Some(Arc::new(move |_| {
            a.wait();
            b.wait();
            panic!("private test worker failure");
        }));
        let other = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        jobs.start(r, other).unwrap();
        entered.wait();
        let mut release = Release(Some(release));
        assert_eq!(jobs.review(ID, selection).unwrap_err().kind, "nativeBusy");
        release.release();
        finished(&jobs);
        let body = jobs.status(Some(other), false).unwrap();
        assert_eq!(body["phase"], "failed");
        assert_eq!(body["error"]["kind"], "localPreparationFailed");
        assert!(jobs.worker.is_none());
    }
    #[test]
    fn local_job_spawn_failure_survives_a_failed_terminal_write() {
        let (_f, mut jobs, r) = setup();
        // The input record is durably accepted before the OS thread-start attempt.
        let input = admitted_input(&r).unwrap();
        let body = initial(&r, ID, "Default");
        jobs.db
            .save(&[(ID.into(), input.clone(), body.clone())])
            .unwrap();
        jobs.jobs.insert(
            ID.into(),
            Job {
                input,
                body,
                prepared: None,
                plan: None,
            },
        );
        jobs.db.fail_writes = true;
        assert!(jobs
            .track_worker(
                ID,
                PrepareControl::new(),
                Err(std::io::Error::other(
                    "private simulated thread start failure"
                ))
            )
            .is_err());
        assert!(jobs
            .worker
            .as_ref()
            .is_some_and(|w| w.handle.is_none() && w.result.is_some()));
        jobs.db.fail_writes = false;
        assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "failed");
        assert!(jobs.worker.is_none());
    }
}

#[cfg(test)]
#[path = "../tests/local_archive/jobs.rs"]
mod archive_tests;
