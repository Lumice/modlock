//! Schema 1 native document, durable operation results and apply journals.
//! Every write requires the matching RootLock; readers never create a library.
use crate::{native::NativeError, paths, snapshot, sqlite::Database};
use json::{object, JsonValue};
use std::{
    collections::HashMap,
    fs::{File, Metadata, OpenOptions},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::{Path, PathBuf},
};
extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
#[derive(Debug)]
pub struct RootLock {
    root: PathBuf,
    _file: File,
    directory: File,
}
impl RootLock {
    pub fn acquire(root: &Path) -> Result<Self, NativeError> {
        let root = paths::absolute(root)?;
        paths::private_dir(&root)?;
        paths::reject_symlinks(&root)?;
        let directory = OpenOptions::new().read(true).custom_flags(0x10000 | 0x20000).open(&root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(0x20000 | 0x800)
            .open(root.join(".writer.lock"))?;
        if !file.metadata()?.is_file() {
            return Err(NativeError::new(
                "unsafePath",
                "Native lock must be a regular file.",
            ));
        }
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err(NativeError::new(
                "nativeBusy",
                "Another native manager is writing this library; retry after it finishes.",
            ));
        }
        let lock = Self { root, _file: file, directory };
        lock.validate()?;
        Ok(lock)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn acquire_existing(root: &Path) -> Result<Self, NativeError> {
        let root=paths::absolute(root)?;paths::reject_symlinks(&root)?;
        let directory=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&root)?;
        let file=OpenOptions::new().read(true).write(true).custom_flags(0x20000|0x800).open(root.join(".writer.lock"))?;
        if !file.metadata()?.is_file() || unsafe {flock(file.as_raw_fd(),2|4)}!=0 {
            return Err(NativeError::new("nativeBusy","Native library lock is unavailable."));
        }
        let lock=Self{root,_file:file,directory};lock.validate()?;Ok(lock)
    }
    pub(crate) fn validate(&self) -> Result<(), NativeError> {
        paths::reject_symlinks(&self.root)?;
        let directory = std::fs::symlink_metadata(&self.root)?;
        let held = self.directory.metadata()?;
        let named = paths::open_regular(&self.root.join(".writer.lock"))?.metadata()?;
        let lock = self._file.metadata()?;
        if !directory.is_dir() || (directory.dev(), directory.ino()) != (held.dev(), held.ino())
            || (named.dev(), named.ino()) != (lock.dev(), lock.ino()) {
            return Err(NativeError::new("nativeBusy", "Native library lock identity changed."));
        }
        Ok(())
    }
    pub(crate) fn directory(&self) -> &File { &self.directory }
}
impl Drop for RootLock {
    fn drop(&mut self) {
        unsafe { flock(self._file.as_raw_fd(), 8) };
    }
}
pub struct Store {
    root: PathBuf,
    db: Database,
}
/// Exact remote operation evidence; an error returned by `replay` is not proof
/// that a commit-bearing job can release its original claims.
pub(crate) enum ExactRemoteOutcome { Absent, Succeeded(JsonValue), Failed(NativeError) }
pub(crate) fn safe_remote_failure(error:&NativeError)->bool{
    paths::safe_id(&error.kind)&&error.message.len()<=512&&!error.message.is_empty()
        &&!error.message.chars().any(char::is_control)&&!error.message.contains('/')&&!error.message.contains('\\')
}
pub fn canonical(value: &JsonValue) -> String {
    match value {
        JsonValue::Object(_) => {
            let mut entries: Vec<_> = value.entries().collect();
            entries.sort_by_key(|(k, _)| *k);
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(k, v)| format!("{}:{}", JsonValue::from(k).dump(), canonical(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        JsonValue::Array(_) => format!(
            "[{}]",
            value.members().map(canonical).collect::<Vec<_>>().join(",")
        ),
        _ => value.dump(),
    }
}
pub fn revision(value: &str) -> Result<u64, NativeError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(NativeError::params());
    }
    value.parse().map_err(|_| NativeError::params())
}
fn parse(s: &str) -> Result<JsonValue, NativeError> {
    json::parse(s).map_err(|_| {
        NativeError::new(
            "nativeDatabase",
            "Native document is malformed; preserve the database for recovery.",
        )
    })
}
fn parse_journal(s:&str)->Result<JsonValue,NativeError>{
    let value=parse(s)?;
    if crate::remote::recovery::is_remote_kind(&value["kind"]) && value.dump()!=s {
        return Err(NativeError::new("recoveryRequired","The remote journal encoding is ambiguous."));
    }
    Ok(value)
}
impl Store {
    /// Hold one SQLite read snapshot while the recovery inspector binds its
    /// journal, document and outcome observations. No writer is admitted here.
    pub(crate) fn inspect_recovery<T>(&self, inspect:impl FnOnce(&Self)->Result<T,NativeError>)->Result<T,NativeError>{
        self.db.execute("BEGIN",&[])?;
        // Validate the history columns even for an empty journal set, without
        // materializing an unrelated operation's potentially large result.
        let result=self.db.query("SELECT id,input,result FROM operations LIMIT 0",&[]).and_then(|_|inspect(self));
        let ended=self.db.execute("ROLLBACK",&[]);
        match result {Ok(value)=>{ended?;Ok(value)},Err(error)=>{let _=ended;Err(error)}}
    }
    /// Distinguish an actual absent row from a present but unreadable document.
    pub(crate) fn recovery_document(&self)->Result<Option<JsonValue>,NativeError>{
        if self.db.query("SELECT body FROM library WHERE id=1",&[])?.is_none(){return Ok(None);}
        match self.document(){Ok(d)=>Ok(Some(d)),Err(e) if e.kind=="nativeNotImported"=>Ok(None),Err(e)=>Err(e)}
    }
    pub fn open_history(root: &Path) -> Result<Self, NativeError> {
        let root = paths::absolute(root)?;
        if !root.join("library.sqlite3").is_file() {
            return Err(NativeError::not_imported());
        }
        paths::reject_symlinks(&root)?;
        Ok(Self {
            db: Database::open(&root.join("library.sqlite3"), false)?,
            root,
        })
    }
    pub fn open(root: &Path) -> Result<Self, NativeError> {
        let s = Self::open_history(root)?;
        s.document()?;
        Ok(s)
    }
    pub fn open_writer(lock: &RootLock) -> Result<Self, NativeError> {
        lock.validate()?;
        let root = lock.root.clone();
        paths::reject_symlinks(&root)?;
        let db = Database::open(&root.join("library.sqlite3"), true)?;
        db.query("PRAGMA journal_mode=DELETE", &[])?;
        db.execute("PRAGMA synchronous=FULL", &[])?;
        db.execute("CREATE TABLE IF NOT EXISTS library (id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL)",&[])?;
        db.execute("CREATE TABLE IF NOT EXISTS operations (id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL)",&[])?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS journals (id TEXT PRIMARY KEY, body TEXT NOT NULL)",
            &[],
        )?;
        let store = Self { root, db };
        // Downgrade protection becomes durable before writers.
        let tx=store.db.transaction()?;
        if let Some(doc)=store.recovery_document()? {
            let disk=store.persisted_document(&doc)?;
            store.db.execute("UPDATE library SET body=? WHERE id=1",&[&disk])?;
        } else {
            store.db.execute("INSERT OR IGNORE INTO library(id,body) VALUES(1,?)",&[&object!{storeVersion:5,freshImport:true}.dump()])?;
        }
        tx.commit()?;
        store.create_launch_tables()?;
        Ok(store)
    }
    /// Bootstrap's logical Store root remains the locked final root. The
    /// candidate authority supplies the retained file for read-only images;
    /// creation is entirely in memory until the owner exports the result.
    pub(crate) fn bootstrap_candidate(lock: &RootLock, candidate: &crate::library_bootstrap::Candidate, create: bool) -> Result<(Self,Option<Metadata>), NativeError> {
        candidate.check_owned(lock)?;
        let outcome=(|| {
            let (db,before)=if create {
                candidate.check_empty_database(lock)?;
                (Database::bootstrap_memory()?,None)
            } else {
                let (image,before)=candidate.read_image(lock)?;
                (Database::bootstrap_image(&image)?,Some(before))
            };
            if create {
                db.bootstrap_limits()?;
                db.execute("CREATE TABLE library (id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL)", &[])?;
                db.execute("CREATE TABLE operations (id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL)", &[])?;
                db.execute("CREATE TABLE journals (id TEXT PRIMARY KEY, body TEXT NOT NULL)", &[])?;
                db.execute("CREATE TABLE bootstrap_origin (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL CHECK(version=1), root TEXT NOT NULL, operationId TEXT NOT NULL, input TEXT NOT NULL)", &[])?;
            }
            let store = Self { root: lock.root.clone(), db };
            if create { store.create_launch_tables()?; }
            Ok((store,before))
        })();
        // Check on both success and failure, after any failed SQLite
        // constructor has closed its in-memory connection.
        candidate.check_owned(lock)?;
        outcome
    }
    pub(crate) fn bootstrap_serialize(self) -> Result<Vec<u8>,NativeError> {
        if !self.db.maintenance_autocommit() {
            return Err(NativeError::new("bootstrapStorage","The bootstrap transaction did not close."));
        }
        let image=self.db.bootstrap_serialize(1024*1024)?;
        self.db.bootstrap_close()?;
        Ok(image)
    }
    pub(crate) fn bootstrap_close(self) -> Result<(),NativeError> { self.db.bootstrap_close() }
    pub(crate) fn bootstrap_database(&self) -> &Database { &self.db }
    pub(crate) fn bootstrap_final(root: &Path) -> Result<Self, NativeError> {
        Ok(Self { root: root.to_owned(), db: Database::bootstrap_readonly(&root.join("library.sqlite3"))? })
    }
    pub(crate) fn launch_database(&self) -> &Database { &self.db }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn locked(&self, lock: &RootLock) -> Result<(), NativeError> {
        if self.root != lock.root {
            return Err(NativeError::new(
                "nativeBusy",
                "Mutation lock belongs to a different native library.",
            ));
        }
        lock.validate()
    }
    pub fn document(&self) -> Result<JsonValue, NativeError> {
        let row = self
            .db
            .query("SELECT body FROM library WHERE id=1", &[])?
            .ok_or_else(NativeError::not_imported)?;
        let mut d = parse(&row[0])?;
        if d["storeVersion"]==5 && d["freshImport"]==true {return Err(NativeError::not_imported());}
        // Store5 is an on-disk capability barrier, independent of package and
        // transaction formats. Never expose it as a package/layout upgrade.
        if d["storeVersion"] == 5 {
            let mut version = d.remove("storageDocumentVersion");
            if version==5 {version=d.remove("gameDocumentVersion");}
            if !matches!(version.as_u32(), Some(1 | 2 | 3 | 4)) {
                return Err(NativeError::new("unsupportedVersion", "This library was changed by a newer Modlock. Update the app to open it."));
            }
            d["storeVersion"] = version;
        }
        crate::package::document_version(&d)?;
        crate::local_import::validate_document_files(&d)?;
        revision(d["revision"].as_str().ok_or_else(NativeError::params)?)?;
        crate::deployment_metadata::admit(&self.root,&d)?;
        Ok(d)
    }
    /// Every document write preserves the barrier, including rollback.
    fn persisted_document(&self, document: &JsonValue) -> Result<String, NativeError> {
        let mut disk = document.clone();
        disk["storageDocumentVersion"] = 5.into();
        disk["gameDocumentVersion"] = document["storeVersion"].clone();
        disk["storeVersion"] = 5.into();
        Ok(disk.dump())
    }
    /// Payload authority is independent of the small library database.
    pub fn storage_root(&self) -> Result<PathBuf, NativeError> {
        let row = self.db.query("SELECT body FROM library WHERE id=1", &[])?;
        let Some(row) = row else { return Ok(self.root.clone()) };
        let doc = parse(&row[0])?;
        if doc["storageLocation"].is_null() { return Ok(self.root.clone()) }
        crate::storage::location(&doc["storageLocation"])
    }
    pub fn payload_path(&self, id: &str) -> Result<PathBuf, NativeError> {
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(NativeError::new(
                "unsafePath",
                "Native payload identity is invalid.",
            ));
        }
        Ok(self.storage_root()?.join("payloads").join(format!("{id}.vpk")))
    }
    pub fn snapshot(&self,profile:Option<&str>)->Result<JsonValue,NativeError>{self.snapshot_with(profile,None)}
    pub(crate) fn snapshot_locked(&self,profile:Option<&str>,root:&RootLock,game:&crate::game_access::GameLock)->Result<JsonValue,NativeError>{
        self.locked(root)?;let doc=self.document()?;game.validate(&Path::new(doc["gamePath"].as_str().ok_or_else(NativeError::params)?).join("game/citadel"))?;
        self.snapshot_with(profile,Some(game))
    }
    fn snapshot_with(&self, profile: Option<&str>,authority:Option<&crate::game_access::GameLock>) -> Result<JsonValue, NativeError> {
        let mut doc = self.document()?;
        crate::deployment_metadata::validate(&doc)?;
        let original = doc.clone();
        if self
            .db
            .query("SELECT id FROM journals LIMIT 1", &[])?
            .is_some()
        {
            doc["recoveryRequired"] = true.into()
        }
        let folder_unavailable = doc["gamePath"].as_str().filter(|p|!p.is_empty()).is_some_and(|p|crate::installed::install_path(Path::new(p)).is_err());
        let external_gameinfo = doc["recoveryRequired"] != true && (doc["gameFolderChanged"]==true || folder_unavailable
            || crate::apply::check_external_mode(&self.root,&doc,true,authority).unwrap_or(false));
        let lost_applied = doc["gameFolderChanged"]==true || crate::apply::check_snapshot(&self.root,&doc,authority).is_err();
        if lost_applied {
            doc["recoveryRequired"] = (!external_gameinfo).into();
            doc["applied"] = JsonValue::Null;
            doc["canRestore"] = false.into();
        }
        let mut result = snapshot_document(&doc, profile)?;
        result["launchFingerprint"]["gameBuild"]=doc["gamePath"].as_str().filter(|p|!p.is_empty()).and_then(|p|crate::health::build(Path::new(p))).map(JsonValue::from).unwrap_or(JsonValue::Null);
        // Live eligibility is display state, never part of a durable commit result.
        if let Some(reason)=crate::apply::check_restore_baseline_reason(&doc,authority) {
            result["canRestore"]=false.into();
            if !result["warnings"].members().any(|w|w["message"].as_str()==Some(reason.as_str())) {
                result["warnings"].push(object!{code:"unverifiedRestore",message:reason}).map_err(|_|NativeError::params())?;
            }
        }
        if doc["applied"]["manifest"].has_key("repairs") && crate::health::build(Path::new(doc["gamePath"].as_str().unwrap_or(""))) .map(JsonValue::from).unwrap_or(JsonValue::Null)!=doc["applied"]["manifest"]["repairs"]["gameBuild"] {result["pendingChanges"]=true.into();result["pendingStatus"]="pending".into();result["pendingReason"]="gameBuildChanged".into();}
        if lost_applied {
            result["pendingChanges"] = true.into();
            result["pendingStatus"] = "unknown".into();
            result["pendingReason"] = if external_gameinfo {"externalGameinfoChange"} else {crate::deployment_intent::UNVERIFIED_REASON}.into();
            for row in result["mods"].members_mut() {
                row["pendingApply"] = true.into();
                row["pendingStatus"] = "unknown".into();
                row["pendingReason"] = crate::deployment_intent::UNVERIFIED_REASON.into();
            }
        }
        if !lost_applied {
            // Malformed/partial/foreign evidence receives no launch-only exemption.
            if let Ok(Some(projection)) = self.launch_snapshot_projection(&original) {
                result["launchBarrier"] = projection;
                if crate::protocol::native_value_bytes(&result).is_none() {
                    return Err(NativeError::new("nativeResponseLimit", "Launch snapshot exceeds the readable response budget."));
                }
            }
        }
        Ok(result)
    }
    /// Returns a previously committed result before checking revisions. Inputs must
    /// include method and all semantic params; JSON object key order is irrelevant.
    pub fn replay(
        &self,
        operation_id: &str,
        input: &JsonValue,
    ) -> Result<Option<JsonValue>, NativeError> {
        if !paths::safe_id(operation_id) {
            return Err(NativeError::params());
        }
        match self.db.query(
            "SELECT input,result FROM operations WHERE id=?",
            &[operation_id],
        )? {
            None => {
                if let Some(journal) = self.journal(operation_id)? {
                    if journal.has_key("input") && canonical(&journal["input"]) != canonical(input)
                    {
                        return Err(NativeError::new(
                            "operationIdConflict",
                            "This unfinished operation ID belongs to different inputs.",
                        ));
                    }
                }
                Ok(None)
            }
            Some(row) => {
                if row[0] != canonical(input) {
                    return Err(NativeError::new(
                        "operationIdConflict",
                        "This operation ID already belongs to different inputs; use a new ID.",
                    ));
                }
                let outcome = parse(&row[1])?;
                if outcome["status"] == "failed" {
                    return Err(NativeError::new(
                        outcome["error"]["kind"]
                            .as_str()
                            .ok_or_else(NativeError::params)?,
                        outcome["error"]["message"]
                            .as_str()
                            .ok_or_else(NativeError::params)?,
                    ));
                }
                Ok(Some(outcome["result"].clone()))
            }
        }
    }
    pub(crate) fn exact_remote_outcome(&self, input:&crate::remote::contract::RemoteCommitInputV1) -> Result<ExactRemoteOutcome,NativeError> {
        self.exact_remote_outcome_parts(input.operation_id(),input.json(),input.params())
    }
    pub(crate) fn exact_any_remote_outcome(&self, input:&crate::remote::contract::RemoteCommitInput) -> Result<ExactRemoteOutcome,NativeError> {
        self.exact_remote_outcome_parts(input.operation_id(),input.json(),input.params())
    }
    fn exact_remote_outcome_parts(&self,id:&str,input:&JsonValue,params:&JsonValue)->Result<ExactRemoteOutcome,NativeError>{
        let Some(row)=self.db.query("SELECT input,result FROM operations WHERE id=?",&[id])? else {return Ok(ExactRemoteOutcome::Absent);};
        if row[0]!=canonical(input) {return Err(NativeError::new("operationIdConflict","This operation ID belongs to different inputs."));}
        let outcome=parse(&row[1])?;
        // These rows are produced by this Store in one exact serialization.
        // Reject duplicate/changed encodings rather than interpreting them as proof.
        if outcome.dump()!=row[1] {return Err(NativeError::new("recoveryRequired","The remote operation outcome is ambiguous."));}
        if outcome.is_object() && outcome.len()==2 && outcome["status"]=="succeeded" && outcome["result"].is_object() {
            let result=&outcome["result"];
            crate::remote::contract::RemoteResultV1::parse(crate::remote::contract::RemoteMethod::Commit,result.clone()).map_err(|_|NativeError::new("recoveryRequired","The remote success result is ambiguous."))?;
            let next=revision(params["expectedRevision"].as_str().ok_or_else(NativeError::params)?)?.checked_add(1).ok_or_else(NativeError::params)?;
            if result["storeVersion"]!=4||result["selectedProfileId"]!=params["profileId"]||result["revision"]!=next.to_string(){return Err(NativeError::new("recoveryRequired","The remote success result is ambiguous."));}
            return Ok(ExactRemoteOutcome::Succeeded(outcome["result"].clone()));
        }
        if outcome.is_object()&&outcome.len()==2&&outcome["status"]=="failed"&&outcome["error"].is_object()&&outcome["error"].len()==2
            &&outcome["error"].has_key("kind")&&outcome["error"].has_key("message"){
            let kind=outcome["error"]["kind"].as_str().ok_or_else(NativeError::params)?;
            let message=outcome["error"]["message"].as_str().ok_or_else(NativeError::params)?;
            let error=NativeError::new(kind,message);if safe_remote_failure(&error){return Ok(ExactRemoteOutcome::Failed(error));}
        }
        Err(NativeError::new("recoveryRequired","The remote operation outcome is ambiguous."))
    }
    /// Read only the new recovery owner's exact success evidence. Historical
    /// recovery replay continues to use the existing, unchanged codec.
    pub(crate) fn exact_remote_recovery_success(&self,input:&crate::remote::contract::RemoteRecoveryInputV1)->Result<Option<JsonValue>,NativeError>{
        let Some(row)=self.db.query("SELECT input,result FROM operations WHERE id=?",&[input.operation_id()])? else{return Ok(None);};
        if row[0]!=canonical(input.json()){return Err(NativeError::new("operationIdConflict","This recovery ID belongs to different inputs."));}
        let outcome=parse(&row[1])?;
        if outcome.dump()!=row[1]||!outcome.is_object()||outcome.len()!=2||outcome["status"]!="succeeded"||!outcome.has_key("result"){
            return Err(NativeError::new("recoveryRequired","The remote recovery outcome is ambiguous."));
        }
        let result=&outcome["result"];
        crate::remote::contract::RemoteResultV1::parse(crate::remote::contract::RemoteMethod::Commit,result.clone())
            .map_err(|_|NativeError::new("recoveryRequired","The remote recovery result is ambiguous."))?;
        if result["revision"]!=input.expected_revision().to_string(){return Err(NativeError::new("recoveryRequired","The remote recovery revision is ambiguous."));}
        Ok(Some(result.clone()))
    }
    /// Atomically stores the next document and terminal result. Use expected=0 only
    /// for initial import. The caller holds RootLock across any related file work.
    pub fn commit(
        &self,
        lock: &RootLock,
        expected: u64,
        operation_id: &str,
        input: &JsonValue,
        document: JsonValue,
        selected: Option<&str>,
    ) -> Result<JsonValue, NativeError> {
        self.commit_with_bulk_result(lock, expected, operation_id, input, document, selected, None)
    }
    /// Persist the summary in the same receipt transaction as the new snapshot.
    pub(crate) fn commit_with_bulk_result(
        &self, lock: &RootLock, expected: u64, operation_id: &str, input: &JsonValue,
        document: JsonValue, selected: Option<&str>, summary: Option<JsonValue>,
    ) -> Result<JsonValue, NativeError> {
        self.commit_with_payload_proof(lock,expected,operation_id,input,document,selected,summary,None)
    }
    pub(crate) fn commit_verified_payloads(&self,lock:&RootLock,expected:u64,operation_id:&str,input:&JsonValue,document:JsonValue,selected:Option<&str>,verified:&crate::remote::recovery::Verified)->Result<JsonValue,NativeError>{
        self.commit_with_payload_proof(lock,expected,operation_id,input,document,selected,None,Some(verified))
    }
    pub(crate) fn commit_with_payload_proof(
        &self, lock: &RootLock, expected: u64, operation_id: &str, input: &JsonValue,
        mut document: JsonValue, selected: Option<&str>, summary: Option<JsonValue>, verified:Option<&crate::remote::recovery::Verified>,
    ) -> Result<JsonValue, NativeError> {
        self.locked(lock)?;
        if !paths::safe_id(operation_id) {
            return Err(NativeError::params());
        }
        let tx = self.db.transaction()?;
        if let Some(result) = self.replay(operation_id, input)? {
            return Ok(result);
        }
        let current = match self.document() {
            Ok(d) => revision(d["revision"].as_str().ok_or_else(NativeError::params)?)?,
            Err(e) if e.kind == "nativeNotImported" => 0,
            Err(e) => return Err(e),
        };
        if current != expected {
            return Err(NativeError::new(
                "staleRevision",
                "The native library changed; refresh and review before trying again.",
            ));
        }
        document["revision"] = expected
            .checked_add(1)
            .ok_or_else(|| NativeError::new("nativeDatabase", "Native revision limit reached."))?
            .to_string()
            .into();
        ensure_readable(&document)?;
        crate::deployment_metadata::admit(&self.root,&document)?;
        let mut result = snapshot_document(&document, selected)?;
        if let Some(summary) = summary { result["bulkResult"] = summary; }
        admit_outcome(operation_id,input,&object!{status:"succeeded",result:result.clone()})?;
        // Keep creator inode receipts independent of profile reachability.
        self.record_payload_ownership(lock,&document,verified)?;
        let body = self.persisted_document(&document)?;
        let output = object! {status:"succeeded",result:result.clone()}.dump();
        let input = canonical(input);
        self.db.execute("INSERT INTO library(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",&[&body])?;
        self.db.execute(
            "INSERT INTO operations(id,input,result) VALUES(?,?,?)",
            &[operation_id, &input, &output],
        )?;
        tx.commit()?;
        Ok(result)
    }
    /// Only live, fully reverified preparation may record success at the same
    /// revision. This is deliberately not a general same-revision commit API.
    pub(crate) fn record_prepared_verification(
        &self, lock: &RootLock, game_lock: &crate::game_access::GameLock,
        expected: u64, id: &str, input: &JsonValue,
        plan: &crate::launch_preparation::VerifiedPreparationPlan,
        control: &crate::prepare_control::PrepareControl,
        progress: &mut dyn FnMut(crate::launch_preparation::PreparationProgress),
    ) -> Result<JsonValue, NativeError> {
        // Historical outcomes stay queryable independently of current state.
        if let Some(result) = self.replay(id, input)? { return Ok(result); }
        self.locked(lock)?;
        plan.admit_input(expected, id, input)?;
        control.check()?;
        progress(crate::launch_preparation::PreparationProgress::Verifying);
        let tx = self.db.transaction()?;
        let doc = self.document()?;
        if revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)? != expected {
            return Err(NativeError::new("staleRevision", "Library changed before prepared verification."));
        }
        ensure_readable(&doc)?;
        crate::deployment_metadata::admit(self.root(), &doc)?;
        let result = snapshot_document(&doc, Some(plan.profile_id()))?;
        let outcome = object!{status:"succeeded",result:result.clone()};
        admit_outcome(id, input, &outcome)?;
        // The live review has verified content. Confirm its document and locks
        // before recording; the final file-set check runs at dispatch admission.
        plan.revalidate(self, lock, game_lock)?;
        control.check()?;
        progress(crate::launch_preparation::PreparationProgress::Committing);
        self.db.execute("INSERT INTO operations(id,input,result) VALUES(?,?,?)",
            &[id, &canonical(input), &outcome.dump()])?;
        tx.commit()?;
        Ok(result)
    }
    /// Complete an admitted recovery attempt without rewriting the document.
    /// The result and removal of this recovery journal share one SQLite transaction.
    pub(crate) fn complete_recovery(
        &self,
        lock: &RootLock,
        expected: u64,
        id: &str,
        input: &JsonValue,
    ) -> Result<JsonValue, NativeError> {
        self.locked(lock)?;
        if !paths::safe_id(id) {
            return Err(NativeError::params());
        }
        let tx = self.db.transaction()?;
        let attempt=self.journal(id)?;
        if let Some(result) = self.replay(id, input)? {
            if let Some(attempt)=attempt {
                if attempt["kind"]!="recovery" || canonical(&attempt["input"])!=canonical(input) {
                    return Err(NativeError::new("operationIdConflict","Recovery attempt belongs to different inputs."));
                }
                if attempt["version"]==6 {
                    crate::apply::admit_metadata_recovery(self,id,&attempt)?;
                    if self.journals()?.members().any(|e|e["body"]["kind"]!="recovery")
                        || !matches_recorded_snapshot(&self.document()?,&result)? {
                        return Err(NativeError::new("recoveryRequired","Vanilla recovery outcome no longer describes the current library."));
                    }
                }
                self.clear_journal(lock, id)?;
            }
            tx.commit()?;
            return Ok(result);
        }
        let attempt=attempt.ok_or_else(||NativeError::new("recoveryRequired","Recovery attempt is missing."))?;
        if attempt["kind"]!="recovery" || canonical(&attempt["input"])!=canonical(input) {
            return Err(NativeError::new("operationIdConflict","Recovery attempt belongs to different inputs."));
        }
        if matches!(attempt["version"].as_u32(),Some(4|5|6)) { crate::apply::admit_metadata_recovery(self,id,&attempt)?; }
        if attempt["mode"]=="general" && self.journals()?.members().any(|e|e["body"]["kind"]!="recovery") {
            return Err(NativeError::new("recoveryRequired","A writer still requires recovery."));
        }
        let doc = self.document()?;
        if revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)? != expected {
            return Err(NativeError::new(
                "staleRevision",
                "Library changed during recovery.",
            ));
        }
        ensure_readable(&doc)?;
        ensure_internal(input)?;
        let result = snapshot_document(&doc, None)?;
        let outcome = object! {status:"succeeded",result:result.clone()};
        admit_outcome(id,input,&outcome)?;
        self.db.execute(
            "INSERT INTO operations(id,input,result) VALUES(?,?,?)",
            &[id, &canonical(input), &outcome.dump()],
        )?;
        self.clear_journal(lock, id)?;
        tx.commit()?;
        Ok(result)
    }
    /// Finalize a target-bound remote recovery as one transaction. The main
    /// outcome, recovery outcome and both journals cannot be observed apart.
    pub(crate) fn complete_remote_recovery(&self,lock:&RootLock,attempt:&crate::remote::contract::RemoteRecoveryInputV1)->Result<JsonValue,NativeError>{
        self.locked(lock)?;let mut verified=crate::remote::recovery::Verified::new();
        let (original,mode,result)=crate::remote::recovery::terminal(self,lock,attempt,&mut verified)?;
        let tx=self.db.transaction()?;
        if let Some(result)=self.replay(attempt.operation_id(),attempt.json())?{return Ok(result);}
        let (check,check_mode,check_result)=crate::remote::recovery::terminal(self,lock,attempt,&mut verified)?;
        if canonical(check.json())!=canonical(original.json())||check_mode!=mode||check_result!=result{return Err(NativeError::new("recoveryRequired","Remote recovery evidence changed."));}
        match (mode,self.exact_any_remote_outcome(&original)?) {
            ("rollback",ExactRemoteOutcome::Absent)=>{
                let failure=failure_outcome(&crate::remote::recovery::rollback_error());admit_outcome(original.operation_id(),original.json(),&failure)?;
                self.db.execute("INSERT INTO operations(id,input,result) VALUES(?,?,?)",&[original.operation_id(),&canonical(original.json()),&failure.dump()])?;
            },
            ("committed",ExactRemoteOutcome::Succeeded(_))=>{},
            _=>return Err(NativeError::new("recoveryRequired","Remote operation evidence changed during recovery.")),
        }
        let outcome=object!{status:"succeeded",result:result.clone()};admit_outcome(attempt.operation_id(),attempt.json(),&outcome)?;
        self.db.execute("INSERT INTO operations(id,input,result) VALUES(?,?,?)",&[attempt.operation_id(),&canonical(attempt.json()),&outcome.dump()])?;
        self.clear_journal(lock,original.operation_id())?;self.clear_journal(lock,attempt.operation_id())?;tx.commit()?;Ok(result)
    }
    /// Complete only game5 retirement bookkeeping after its captured trees were
    /// removed. The exact committed image and journal remain together until this
    /// transaction atomically prunes absent retired entries and clears that main.
    /// The revision and the already-durable public outcome never change.
    pub(crate) fn complete_game5_retirement(&self, lock:&RootLock, id:&str,
        journal:&JsonValue, pruned:&JsonValue) -> Result<(),NativeError> {
        self.complete_shard_retirement(lock,id,journal,pruned,false)
    }
    pub(crate) fn complete_game6_retirement(&self, lock:&RootLock, id:&str,
        journal:&JsonValue, pruned:&JsonValue) -> Result<(),NativeError> {
        self.complete_shard_retirement(lock,id,journal,pruned,true)
    }
    fn complete_shard_retirement(&self, lock:&RootLock, id:&str,
        journal:&JsonValue, pruned:&JsonValue, vanilla:bool) -> Result<(),NativeError> {
        self.locked(lock)?;
        let fail=||NativeError::new("recoveryRequired","Game retirement evidence changed; preserve its journal.");
        let tx=self.db.transaction()?;
        if journal["kind"]!="game" || journal["version"]!=(if vanilla {6} else {5}) || self.journal(id)?.as_ref()!=Some(journal) {
            return Err(fail());
        }
        if vanilla { crate::apply::admit_game6(self,id,journal)?; }
        else { crate::apply::admit_game5(self,id,journal)?; }
        let mut committed=journal["document"].clone();
        committed["revision"]=revision(journal["expectedRevision"].as_str().ok_or_else(fail)?)?
            .checked_add(1).ok_or_else(fail)?.to_string().into();
        if self.document()?!=committed || self.operation(id)?["status"]!="succeeded"
            || !pruned["retiredGenerations"].is_array() {return Err(fail());}
        let mut original=committed.clone(); let mut replacement=pruned.clone();
        original.remove("retiredGenerations"); replacement.remove("retiredGenerations");
        if original!=replacement {return Err(fail());}
        // Ordered subset only. No checkpoint can be invented, changed, or
        // removed while its generation root still exists (including symlinks).
        let mut retained=pruned["retiredGenerations"].members().peekable();
        let c=Path::new(committed["gamePath"].as_str().ok_or_else(fail)?).join("game/citadel/deadlock_native");
        for checkpoint in committed["retiredGenerations"].members() {
            if retained.peek().is_some_and(|row|**row==*checkpoint) {retained.next();continue;}
            let view=crate::generation::decode(&checkpoint["manifest"])?;
            if checkpoint["generation"]==committed["applied"]["generation"]
                || checkpoint["generation"]==committed["previous"]["applied"]["generation"] {return Err(fail());}
            paths::verify_absent(&c.join(view.generation))?;
        }
        if retained.next().is_some() {return Err(fail());}
        crate::deployment_metadata::admit(&self.root,pruned)?; ensure_readable(pruned)?;
        let result=self.replay(id,&journal["input"])?.ok_or_else(fail)?;
        if !matches_recorded_snapshot(&committed,&result)? || !matches_recorded_snapshot(pruned,&result)? {return Err(fail());}
        admit_success(id,&journal["input"],pruned)?;
        if vanilla {
            // Recovery6's already-durable final digest must match this exact
            // atomic prune/clear image. Null never survives a successful main.
            let journals=self.journals()?;
            crate::font_transaction::admit_set(self,&journals)?;
            let digest=paths::digest_bytes(canonical(pruned).as_bytes())?;
            for entry in journals.members().filter(|e|e["body"]["kind"]=="recovery" && e["body"]["version"]==6) {
                if entry["body"]["mainOperationId"]!=id || entry["body"]["afterDocumentSha256"]!=digest {return Err(fail());}
            }
        }
        self.db.execute("UPDATE library SET body=? WHERE id=1", &[&self.persisted_document(pruned)?])?;
        self.clear_journal(lock,id)?;
        tx.commit()
    }
    /// Restore only journal-proven metadata after filesystem rollback. Preserve
    /// the logical revision; general Recovery4 records the current snapshot only.
    pub(crate) fn complete_font_rollback(&self, lock:&RootLock, id:&str, input:&JsonValue,
        before:&JsonValue, restored:&JsonValue) -> Result<(),NativeError> {
        self.locked(lock)?; ensure_readable(restored)?;
        crate::deployment_metadata::admit(&self.root,restored)?;
        let tx=self.db.transaction()?;
        if self.document()?!=*before || self.operation(id)?["status"]=="succeeded" {
            return Err(NativeError::new("recoveryRequired","Rollback library evidence changed."));
        }
        let mut original=before.clone(); let mut replacement=restored.clone();
        original.remove("fonts");replacement.remove("fonts");
        if original!=replacement{return Err(NativeError::params())}
        self.db.execute("UPDATE library SET body=? WHERE id=1", &[&self.persisted_document(restored)?])?;
        let outcome=failure_outcome(&crate::font_transaction::rollback_error());
        admit_outcome(id,input,&outcome)?;
        self.db.execute("INSERT INTO operations(id,input,result) VALUES(?,?,?)", &[id,&canonical(input),&outcome.dump()])?;
        self.clear_journal(lock,id)?;tx.commit()
    }
    /// Reserve a valid rejected mutation without changing the library revision.
    pub fn record_failure(
        &self,
        lock: &RootLock,
        id: &str,
        input: &JsonValue,
        error: &NativeError,
    ) -> Result<(), NativeError> {
        self.locked(lock)?;
        if !paths::safe_id(id) {
            return Err(NativeError::params());
        }
        let tx = self.db.transaction()?;
        let fingerprint = canonical(input);
        if let Some(row) = self
            .db
            .query("SELECT input FROM operations WHERE id=?", &[id])?
        {
            if row[0] != fingerprint {
                return Err(NativeError::new(
                    "operationIdConflict",
                    "This operation ID belongs to different inputs.",
                ));
            }
            return Ok(());
        }
        let outcome=object!{status:"failed",error:object!{kind:error.kind.clone(),message:error.message.clone()}}.dump();
        self.db.execute(
            "INSERT INTO operations(id,input,result) VALUES(?,?,?)",
            &[id, &fingerprint, &outcome],
        )?;
        tx.commit()
    }
    pub fn operation(&self, id: &str) -> Result<JsonValue, NativeError> {
        if !paths::safe_id(id) {
            return Err(NativeError::params());
        }
        if let Some(row) = self
            .db
            .query("SELECT result FROM operations WHERE id=?", &[id])?
        {
            let mut outcome = parse(&row[0])?;
            outcome["operationId"] = id.into();
            return Ok(outcome);
        }
        if self.journal(id)?.is_some() {
            return Ok(object! {operationId:id,status:"recoveryRequired"});
        }
        Ok(object! {operationId:id,status:"unknown"})
    }
    pub fn journal(&self, id: &str) -> Result<Option<JsonValue>, NativeError> {
        self.db
            .query("SELECT body FROM journals WHERE id=?", &[id])?
            .map(|r| parse_journal(&r[0]))
            .transpose()
    }
    pub fn journals(&self) -> Result<JsonValue, NativeError> {
        let mut result = JsonValue::new_array();
        let mut previous = String::new();
        while let Some(row) = self.db.query(
            "SELECT id,body FROM journals WHERE id>? ORDER BY id LIMIT 1",
            &[&previous],
        )? {
            if result.len() >= 1024 {
                return Err(NativeError::new(
                    "nativeDatabase",
                    "Too many unfinished journals; preserve the store for recovery.",
                ));
            }
            result
                .push(object! {id:row[0].clone(),body:parse_journal(&row[1])?})
                .map_err(|_| NativeError::params())?;
            previous = row[0].clone();
        }
        Ok(result)
    }
    pub fn write_journal(
        &self,
        lock: &RootLock,
        id: &str,
        body: &JsonValue,
    ) -> Result<(), NativeError> {
        self.locked(lock)?;
        if !paths::safe_id(id) {
            return Err(NativeError::params());
        }
        ensure_internal(body)?;
        self.db.execute("INSERT INTO journals(id,body) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",&[id,&body.dump()])?;
        Ok(())
    }
    pub fn clear_journal(&self, lock: &RootLock, id: &str) -> Result<(), NativeError> {
        self.locked(lock)?;
        self.db.execute("DELETE FROM journals WHERE id=?", &[id])?;
        Ok(())
    }
}
/// A native library must remain readable in every view and in the larger
/// durable operation envelope. Reserve mutable display fields at their longest
/// representation so Apply, Restore, recovery and revision growth cannot consume
/// unbudgeted bytes. Profile edits are checked again with the projected summaries.
pub fn ensure_readable(doc: &JsonValue) -> Result<(), NativeError> {
    crate::performance::validate_selection(doc)?;
    crate::package::document_version(doc)?;
    crate::local_import::validate_document_files(doc)?;
    ensure_internal(doc)?;
    crate::deployment_metadata::validate(doc)?;
    // Catalog detail is lazy on the wire, but every stored catalog must fit its
    // exact bounded detail projection before any future mutation commits it.
    let layout = crate::package::layout_version(doc)?;
    let expected=revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?;
    for (profile_id,profile) in doc["profiles"].entries() {
        for record in profile["mods"].members().filter(|r|r.has_key("options")) {
            crate::local_content::option_sources::detail_envelope(record,layout,profile_id,expected)?;
        }
    }
    // Generated profile IDs have 32 ASCII bytes. Imported IDs may be longer or
    // expand under JSON escaping; compare serialized byte lengths, not chars.
    let longest_id = doc["profiles"]
        .entries()
        .map(|(id, _)| JsonValue::from(id))
        .chain(std::iter::once(JsonValue::from("f".repeat(32))))
        .max_by_key(|id| id.dump().len())
        .unwrap();
    let limit = || {
        NativeError::new("nativeResponseLimit", "This projected library exceeds the 32 MiB readable native response budget. Reduce the library or profile metadata before retrying; no library or game transaction was committed.")
    };
    let mut summaries = profile_summaries(doc)?;
    for summary in summaries.members_mut() {
        summary["isActive"] = false.into();
    }
    let mut common = snapshot_envelope(doc, longest_id.as_str(), summaries);
    common["revision"] = u64::MAX.to_string().into();
    common["activeProfileId"] = longest_id.clone();
    common["appliedProfileId"] = longest_id;
    for key in ["pendingChanges", "canRestore", "recoveryRequired"] {
        common[key] = false.into();
    }
    common["pendingStatus"]="selectionRequired".into();
    common["pendingReason"]=crate::deployment_intent::longest_pending_reason().into();
    // Count shared metadata/summaries only once. Per-profile content replaces
    // the two empty arrays (four bytes); no quadratic summary work on edits.
    let common_bytes = crate::protocol::native_value_bytes(&common).ok_or_else(limit)? - 4;
    for (id, _) in doc["profiles"].entries() {
        let (mut mods, warnings, _) = profile_content(doc, Some(id))?;
        for row in mods.members_mut() {
            row["enabled"] = false.into();
            row["appliedEnabled"] = false.into();
            row["pendingApply"] = false.into();
            row["selectionSaved"] = false.into();
            row["selectionReady"] = false.into();
            row["pendingStatus"] = "selectionRequired".into();
            row["pendingReason"] = crate::deployment_intent::longest_pending_reason().into();
        }
        let mods_bytes = crate::protocol::native_value_bytes(&mods).ok_or_else(limit)?;
        let warning_bytes = crate::protocol::native_value_bytes(&warnings).ok_or_else(limit)?;
        if common_bytes + mods_bytes + warning_bytes > crate::protocol::NATIVE_SNAPSHOT_BUDGET {
            return Err(limit());
        }
    }
    Ok(())
}

/// Older results recorded the then-live Steam build ID. It is display evidence,
/// not transaction authority; retain that recorded value during replay checks.
pub(crate) fn matches_recorded_snapshot(doc:&JsonValue,result:&JsonValue)->Result<bool,NativeError>{
    matches_recorded_snapshot_for(doc,result,None)
}

pub(crate) fn matches_recorded_snapshot_for(doc:&JsonValue,result:&JsonValue,profile:Option<&str>)->Result<bool,NativeError>{
    let mut expected=snapshot_document(doc,profile)?;
    expected["launchFingerprint"]["gameBuild"]=result["launchFingerprint"]["gameBuild"].clone();
    Ok(expected==*result)
}

/// Materialize the recorded document without live filesystem reads. Durable
/// operation results and their recovery comparisons must remain deterministic.
pub fn snapshot_document(
    doc: &JsonValue,
    requested: Option<&str>,
) -> Result<JsonValue, NativeError> {
    crate::deployment_metadata::validate(doc)?;
    let profiles = &doc["profiles"];
    let active = doc["activeProfileId"].as_str();
    let selected = requested
        .or(active)
        .or_else(|| profiles.entries().next().map(|(id, _)| id));
    if let Some(id) = selected {
        if !profiles[id].is_object() {
            return Err(NativeError::new(
                "unknownProfile",
                "Requested native profile does not exist.",
            ));
        }
    }
    let mut result = snapshot_envelope(doc, selected, profile_summaries(doc)?);
    let (mods, warnings, pending) = profile_content(doc, selected)?;
    result["mods"] = mods;
    result["warnings"] = warnings;
    if let Some(reason)=crate::apply::legacy_baseline_reason(doc) {
        result["canRestore"]=false.into();
        result["warnings"].push(object!{code:"unverifiedRestore",message:reason}).map_err(|_|NativeError::params())?;
    }
    result["pendingChanges"] = pending.into();
    if let Some(id)=selected {
        let state=crate::deployment_intent::PendingState::compare(doc,id)?;
        result["pendingStatus"]=state.status.into();
        result["pendingReason"]=state.reason.into();
    }
    if let Some(text)=doc["applied"]["gameinfo"].as_str() {
        let current=crate::performance::patch::read_overlay(text).map(|o|o.config_id).unwrap_or_default();
        if doc.has_key("performancePreset") && current!=crate::performance::selection(doc) {result["pendingChanges"]=true.into();result["pendingStatus"]="pending".into();result["pendingReason"]="Performance config selection changed.".into();}
    } else if !crate::performance::selection(doc).is_empty(){result["pendingChanges"]=true.into();result["pendingStatus"]="pending".into();result["pendingReason"]="Performance config selection changed.".into();}
    if doc["gameFolderChanged"]==true {
        result["pendingChanges"]=true.into();
        result["pendingStatus"]="unknown".into();
        result["pendingReason"]="externalGameinfoChange".into();
        if let Err(e)=crate::apply::refuse_previous_folder(doc) {
            result["warnings"].push(object!{code:"previousGameFolder",message:e.message}).map_err(|_|NativeError::params())?;
        }
    }
    Ok(result)
}

fn profile_summaries(doc: &JsonValue) -> Result<JsonValue, NativeError> {
    let mut summaries = JsonValue::new_array();
    let active = doc["activeProfileId"].as_str();
    for (id, p) in doc["profiles"].entries() {
        summaries.push(object!{id:id,name:p["name"].clone(),isActive:Some(id)==active,modCount:p["mods"].len(),protected:p["protected"].as_bool().unwrap_or(false)}).map_err(|_|NativeError::params())?;
    }
    Ok(summaries)
}

fn snapshot_envelope(doc: &JsonValue, selected: Option<&str>, summaries: JsonValue) -> JsonValue {
    let mut result=object! {protocolVersion:1,sourceKind:"native",sourcePath:doc["sourcePath"].clone(),storeVersion:doc["storeVersion"].clone(),revision:doc["revision"].clone(),activeProfileId:doc["activeProfileId"].clone(),selectedProfileId:selected,profiles:summaries,mods:JsonValue::new_array(),warnings:JsonValue::new_array(),pendingChanges:false,pendingStatus:"current",pendingReason:JsonValue::Null,gamePath:doc["gamePath"].clone(),appliedProfileId:doc["applied"]["profileId"].clone(),canRestore:doc["canRestore"].as_bool().unwrap_or(false),recoveryRequired:doc["recoveryRequired"].as_bool().unwrap_or(false)};
    result["storageRoot"]=doc["storageLocation"]["path"].clone();
    result["launchFingerprint"]=object!{performance:crate::performance::selection(doc),generation:doc["applied"]["generation"].clone(),gameBuild:doc["applied"]["manifest"]["repairs"]["gameBuild"].clone()};
    result["fixAutomatically"]=doc["fixAutomatically"].as_bool().unwrap_or(true).into();
    result["bisect"] = crate::bisect::view(doc);
    if matches!(doc["storeVersion"].as_u32(),Some(3|4)) {result["packageVersion"]=doc["packageVersion"].clone();result["fonts"]=doc["fonts"].clone();}
    result
}

fn profile_content(
    doc: &JsonValue,
    selected: Option<&str>,
) -> Result<(JsonValue, JsonValue, bool), NativeError> {
    let mut mods = JsonValue::new_array();
    let mut warnings = JsonValue::new_array();
    let applied_profile = doc["applied"]["profileId"].as_str();
    let mut pending = false;
    if let Some(id) = selected {
        let p = &doc["profiles"][id];
        let state = crate::deployment_intent::PendingState::compare(doc,id)?;
        let by_id: HashMap<&str, &JsonValue> = p["mods"].members()
            .filter_map(|r| r["source"]["id"].as_str().map(|id| (id, r))).collect();
        let raw = JsonValue::from(crate::profile_edit::ordered_mod_ids(p)?.iter()
            .map(|id| by_id[id.as_str()]["source"].clone()).collect::<Vec<_>>());
        let mut enabled = JsonValue::new_object();
        for m in p["mods"].members() {
            if let Some(remote) = m["source"]["remoteId"].as_str() {
                enabled[remote] = object! {enabled:m["enabled"].clone()}
            }
        }
        mods =
            snapshot::normalize_native_mods(&raw, &enabled, &doc["previewPolicy"], &mut warnings);
        for m in mods.members_mut() {
            let record = by_id
                .get(m["id"].as_str().unwrap_or(""))
                .ok_or_else(NativeError::params)?;
            let was = applied_profile == Some(id)
                && doc["applied"]["enabled"][m["id"].as_str().unwrap_or("")]
                    .as_bool()
                    .unwrap_or(false);
            // Presentation only: never use provider release dates as library dates.
            let source = &record["source"];
            m["heroDetectionEligible"] = (["heroOverride", "detectedHero", "hero"].iter().all(|k| source[*k].as_str().is_none_or(|s| s.trim().is_empty()))).into();
            m["addedAt"] = source["createdAt"].clone();
            m["updatedAt"] = record["updatedAt"].clone();
            m["authorId"] = source["authorId"].as_str().filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) && !s.starts_with('0')).into();
            m["sizeBytes"] = JsonValue::Null;
            m["variantName"] = "".into();
            m["fileName"] = "".into();
            if let Ok(packages) = crate::package::selected_packages_exact(record, crate::package::layout_version(doc)?) {
                let mut seen = std::collections::HashSet::new();
                let bytes = packages.iter().flat_map(|p| std::iter::once(&p.directory).chain(p.companions.values()))
                    .filter(|f| seen.insert(f.id.clone())).try_fold(0u64, |n, f| n.checked_add(f.size));
                m["sizeBytes"] = bytes.filter(|n| *n <= 9_007_199_254_740_991).into();
                m["fileName"] = packages.iter().map(|p| p.directory.source_name.as_str()).collect::<Vec<_>>().join(", ").into();
            }
            if let Ok(options) = crate::options::for_record(record, crate::package::layout_version(doc)?) {
                if options.entries.len() > 1 {
                    m["variantName"] = options.entries.iter().filter(|e| options.selected_ids.contains(&e.id))
                        .map(|e| e.option_name.as_str()).filter(|n| *n != "default" && !n.starts_with("payload-") && *n != "Original variant" && !n.strip_prefix("pak").and_then(|s| s.strip_suffix("_dir.vpk").or_else(|| s.strip_suffix(".vpk"))).is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
                        .collect::<Vec<_>>().join(", ").into();
                }
            }
            m["fixEnabled"]=record["fixEnabled"].as_bool().unwrap_or(doc["fixAutomatically"].as_bool().unwrap_or(true)).into();
            m["enabled"] = record["enabled"].clone();
            m["appliedEnabled"] = was.into();
            let row_pending = state.mod_pending(m["id"].as_str().unwrap_or(""),record["enabled"]==true);
            m["pendingApply"] = row_pending.into();
            let row_status = if state.row_unknown(record["enabled"] == true) { "unknown" }
                else if state.status == "selectionRequired" && record["enabled"] == true { "selectionRequired" }
                else if row_pending { "pending" } else { "current" };
            m["pendingStatus"] = row_status.into();
            m["pendingReason"] = if row_status == "unknown" { Some(crate::deployment_intent::UNKNOWN_REASON) }
                else if row_status == "selectionRequired" { state.reason } else { None }.into();
            let selection_ready = crate::package::selected_packages_exact(record,crate::package::layout_version(doc)?).is_ok();
            m["selectionSaved"] = (record["enabled"]!=true && selection_ready).into();
            m["selectionReady"] = selection_ready.into();
            let eligibility=crate::remote::replacement_target::admit((*record).clone(),crate::package::layout_version(doc)?,id,m["id"].as_str().unwrap_or(""),revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)?);
            if record["source"].has_key("nativeRemoteLink") {
                m["remoteSubmission"]=eligibility.as_ref().ok().and_then(|t|t.submission.as_ref()).map(|s|s.json())
                    .or_else(||crate::remote::replacement_target::association_submission(&record["source"])).unwrap_or(JsonValue::Null);
            }
            m["linkEligible"]=eligibility.as_ref().is_ok_and(|t|t.submission.is_none()).into();
            m["linkReason"]=eligibility.err().map(|e|e.message).unwrap_or_default().into();
            m["optionCount"] = if record.has_key("options") { record["options"]["entries"].len().into() } else { JsonValue::Null };
            m["selectedOptionIds"] = if record.has_key("options") { record["options"]["selectedIds"].clone() } else { JsonValue::Null };
            m["previewHidden"] = record["previewHidden"].clone();
            m["available"] = record["available"].clone();
            m["unavailableReason"] = record["unavailableReason"].clone();
            let local = &record["source"]["localImport"];
            if matches!(doc["storeVersion"].as_u32(),Some(3|4)) {
                if !crate::fonts::stored_refs(record)?.is_empty() {m["fonts"]=local["fonts"].clone();}
                if let Some(diagnostics)=crate::fonts::diagnostics(record)? {m["fontDiagnostics"]=diagnostics.clone();}
            }
            if record["previewHidden"] != true && !local["artwork"].is_null() {
                let file = crate::package::decode_file(&local["artwork"])?;
                let format = local["artworkFormat"]
                    .as_str()
                    .filter(|v| matches!(*v, "png" | "jpeg"))
                    .ok_or_else(NativeError::params)?;
                m["localArtwork"] =
                    object! {id:file.id,sha256:file.sha256,size:file.size,format:format};
            }
        }
        pending = state.pending;
    }
    Ok((mods, warnings, pending))
}

pub(crate) fn failure_outcome(error: &NativeError) -> JsonValue {
    object!{status:"failed",error:object!{kind:error.kind.clone(),message:error.message.clone()}}
}

pub(crate) fn admit_outcome(id: &str, input: &JsonValue, outcome: &JsonValue) -> Result<(),NativeError> {
    if !paths::safe_id(id) { return Err(NativeError::params()); }
    ensure_internal(&object!{operationId:id,input:input.clone(),outcome:outcome.clone()})
}

pub(crate) fn admit_success(id: &str, input: &JsonValue, doc: &JsonValue) -> Result<(),NativeError> {
    ensure_readable(doc)?;
    admit_outcome(id,input,&object!{status:"succeeded",result:snapshot_document(doc,None)?})
}

/// Admit private JSON before filesystem publication, independently of wire DTOs.
pub(crate) fn ensure_internal(value: &JsonValue) -> Result<(), NativeError> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(b.len() as u64)
                .ok_or_else(|| std::io::Error::other("internal JSON limit"))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    value
        .write(&mut Counter(paths::MAX_JSON_BYTES))
        .map_err(|_| {
            NativeError::new(
                "sourceTooLarge",
                "Native document or journal exceeds the 64 MiB internal JSON limit.",
            )
        })
}

impl Store {
    fn record_payload_ownership(&self,lock:&RootLock,document:&JsonValue,verified:Option<&crate::remote::recovery::Verified>)->Result<(),NativeError>{
        use std::os::unix::fs::MetadataExt;
        for entry in self.journals()?.members() {
            let j=&entry["body"];
            if j["kind"]=="storageMove" && document["storageLocation"]==j["copyDirectory"]
                && self.document()?["storageLocation"]!=j["copyDirectory"]
                && self.db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='payload_inode_receipts'",&[])?.is_some() {
                let source=crate::storage::location_with_lock(&j["source"],Some(lock))?;let target=crate::storage::location_with_lock(&document["storageLocation"],Some(lock))?;
                for row in j["files"].members().filter(|r|r["copied"]==true) {
                    let Some(name)=row["name"].as_str() else{continue};
                    let Some(id)=name.strip_prefix("payloads/").and_then(|n|n.strip_suffix(".vpk")).filter(|id|crate::package::hex(id,32)) else{continue};
                    let Some(receipt)=self.db.query("SELECT sha256,identity FROM payload_inode_receipts WHERE id=?",&[id])? else{continue};
                    let old=paths::open_regular(&source.join(name))?.metadata()?;
                    let new_file=paths::open_regular(&target.join(name))?;let new=new_file.metadata()?;
                    let copy=&row["copyIdentity"];
                    let (Some(device),Some(inode))=(copy["device"].as_u64(),copy["inode"].as_u64()) else{continue};
                    let copy_fsid=if copy.has_key("fsid"){Some(copy["fsid"].as_str().filter(|s|crate::package::hex(s,16)).ok_or_else(NativeError::params)?)}else{None};
                    if receipt[0]!=row["sha256"].as_str().unwrap_or("") || !self.payload_owned_with_lock(id,&receipt[0],old.len(),Some(lock))?
                        || !paths::same_persisted_object(lock.root(),lock.directory(),&target.join(name),&new_file,device,inode,copy_fsid)? {continue;}
                    let fp=paths::fingerprint(&target.join(name))?;
                    if row["sha256"]!=fp.sha256 || row["size"]!=fp.size || (fp.device,fp.inode,fp.size,fp.modified_seconds,fp.modified_nanos)!=(new.dev(),new.ino(),new.len(),new.mtime(),new.mtime_nsec()) {continue;}
                    let mut identity=crate::apply::identity(&new);
                    identity["fsid"]=paths::persisted_filesystem_id(&new_file)?.into();
                    self.db.execute("UPDATE payload_inode_receipts SET identity=? WHERE id=?",&[&identity.dump(),id])?;
                }
            }
            if !matches!(j["kind"].as_str(),Some("import"|"remotePayloadImport"|"remotePayloadReplace"|"remotePayloadLink")){continue;}
            for v in j["files"].members().chain(j["payloads"].members()) {
                if v["complete"]!=true || !matches!(v["role"].as_str(),None|Some("payload")){continue;}
                let (Some(id),Some(sha),Some(size))=(v["id"].as_str(),v["sha256"].as_str(),v["size"].as_u64()) else{continue};
                if !crate::package::hex(id,32) || !crate::package::hex(sha,64){continue;}
                let path=self.payload_path(id)?;
                if !path.try_exists()?{continue;}
                let file=paths::open_regular(&path)?;let m=file.metadata()?;
                // Only created inodes in the admitted writer journal grant a receipt.
                // Reused files and arbitrary referenced names grant no ownership.
                if v["device"]!=m.dev() || v["inode"]!=m.ino() || m.len()!=size {continue;}
                if !verified.map(|p|p.matches_content(id,sha,&file)).transpose()?.unwrap_or(false) {
                    let fp=paths::fingerprint(&path)?;
                    if fp.sha256!=sha || (fp.device,fp.inode,fp.size,fp.modified_seconds,fp.modified_nanos)!=(m.dev(),m.ino(),m.len(),m.mtime(),m.mtime_nsec()) {continue;}
                }
                self.db.execute("CREATE TABLE IF NOT EXISTS payload_inode_receipts (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, identity TEXT NOT NULL)",&[])?;
                let mut identity=crate::apply::identity(&m);
                identity["fsid"]=paths::persisted_filesystem_id(&file)?.into();
                self.db.execute("INSERT OR REPLACE INTO payload_inode_receipts(id,sha256,identity) VALUES(?,?,?)",&[id,sha,&identity.dump()])?;
            }
        }
        Ok(())
    }
    // Content receipts grant no ownership. Older metadata is never upgraded
    // without a hash in this operation; every cache hit checks the full stamp.
    pub(crate) fn seed_apply_hashes(&self)->Result<(),NativeError>{
        if self.db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='apply_verified_files'",&[])?.is_none(){return Ok(());}
        let mut previous=String::new();
        let doc=self.document()?;
        let bulk=self.storage_root()?;
        let game=doc["gamePath"].as_str().map(|p|Path::new(p).join("game/citadel/deadlock_native"));
        while let Some(row)=self.db.query("SELECT path,sha256,stamp FROM apply_verified_files WHERE path>? ORDER BY path LIMIT 1",&[&previous])? {
            previous=row[0].clone();
            let path=Path::new(&row[0]);
            if !crate::package::hex(&row[1],64) || !(path.starts_with(bulk.join("payloads")) || path.starts_with(bulk.join("cache/repairs")) || game.as_ref().is_some_and(|g|path.starts_with(g))) {continue;}
            if path.try_exists()? {paths::seed_hash(path,&row[2],&row[1])?;}
        }
        Ok(())
    }
    /// Seed only an admitted deletion candidate; unrelated stale paths must
    /// never block a library deletion. Ownership is checked by the caller.
    pub(crate) fn seed_payload_hash(&self,id:&str)->Result<(),NativeError>{
        if self.db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='apply_verified_files'",&[])?.is_none(){return Ok(());}
        let path=self.payload_path(id)?;
        if let Some(row)=self.db.query("SELECT sha256,stamp FROM apply_verified_files WHERE path=?",&[&path.to_string_lossy()])? {
            if crate::package::hex(&row[0],64) {paths::seed_hash(&path,&row[1],&row[0])?;}
        }
        Ok(())
    }
    fn remember_apply_file(&self,path:&Path,sha:&str,size:u64)->Result<(),NativeError>{
        let file=paths::open_regular(path)?;
        if file.metadata()?.mode() & 0o777 != 0o400 || !paths::HashMemo::verified(path,sha,size)? {return Ok(());}
        self.db.execute("CREATE TABLE IF NOT EXISTS apply_verified_files(path TEXT PRIMARY KEY,sha256 TEXT NOT NULL,stamp TEXT NOT NULL)",&[])?;
        self.db.execute("INSERT OR REPLACE INTO apply_verified_files(path,sha256,stamp) VALUES(?,?,?)",&[&path.to_string_lossy(),sha,&paths::content_stamp(&file)?])?;
        Ok(())
    }
    pub(crate) fn remember_apply_hashes(&self,evidence:&[(PathBuf,paths::Fingerprint)])->Result<(),NativeError>{
        for (path,fp) in evidence {
            let Some(id)=path.file_stem().and_then(|s|s.to_str()).filter(|id|crate::package::hex(id,32)) else{continue};
            if *path==self.payload_path(id)? && self.payload_owned(id,&fp.sha256,fp.size)? {self.remember_apply_file(path,&fp.sha256,fp.size)?;}
        }
        Ok(())
    }
    pub(crate) fn remember_deployment_hashes(&self,doc:&JsonValue)->Result<(),NativeError>{
        // Retirement has removed expired generation paths. Receipts grant no
        // ownership: discard only cache rows, preserving every live file/receipt.
        // Batch pruning and replacement receipts in one SQLite transaction.
        // Classify receipt paths from the admitted document, without probing
        // former mounts. Game receipts remain valid outside the bulk store.
        let bulk=doc["storageLocation"]["path"].as_str().map(Path::new).unwrap_or(self.root());
        let payloads=bulk.join("payloads");let repairs=bulk.join("cache/repairs");
        let game=Path::new(doc["gamePath"].as_str().ok_or_else(NativeError::params)?).join("game/citadel/deadlock_native");
        let transaction=self.db.transaction()?;
        if self.db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='apply_verified_files'",&[])?.is_some(){
            let mut previous=String::new();
            while let Some(row)=self.db.query("SELECT path FROM apply_verified_files WHERE path>? ORDER BY path LIMIT 1",&[&previous])? {
                previous=row[0].clone();
                let path=Path::new(&row[0]);
                let current=path.starts_with(&payloads) || path.starts_with(&repairs) || path.starts_with(&game);
                // An unreadable receipt is merely a cache miss, never a write barrier.
                if !current || !path.try_exists().unwrap_or(false) {
                    self.db.execute("DELETE FROM apply_verified_files WHERE path=?",&[&row[0]])?;
                }
            }
        }
        for a in [&doc["applied"],&doc["previous"]["applied"]].into_iter().filter(|a|a["manifest"]["version"]==3){
            let view=crate::generation::decode(&a["manifest"])?;
            for f in view.files {
                self.remember_apply_file(&game.join(&view.generation).join(&f.name),&f.payload.sha256,f.payload.size)?;
                if f.name=="repairs/pak01_dir.vpk" {
                    let source=crate::repairs::source(self,&a["manifest"],&f.payload)?;
                    self.remember_apply_file(&source,&f.payload.sha256,f.payload.size)?;
                }
            }
        }
        transaction.commit()
    }
    pub(crate) fn payload_owned(&self,id:&str,sha:&str,size:u64)->Result<bool,NativeError>{
        self.payload_owned_with_lock(id,sha,size,None)
    }
    pub(crate) fn payload_owned_with_lock(&self,id:&str,sha:&str,size:u64,lock:Option<&RootLock>)->Result<bool,NativeError>{
        if self.db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='payload_inode_receipts'",&[])?.is_some() {
            if let Some(row)=self.db.query("SELECT identity,sha256 FROM payload_inode_receipts WHERE id=?",&[id])? {
                let path=self.payload_path(id)?;let file=paths::open_regular(&path)?;let m=file.metadata()?;
                let identity=parse(&row[0])?;
                if !identity.is_object() || identity.len()!=if identity.has_key("fsid"){6}else{5} || !["size","device","inode","modifiedSeconds","modifiedNanos"].iter().all(|k|identity.has_key(k)){return Ok(false);}
                let fsid=if identity.has_key("fsid") {Some(identity["fsid"].as_str().filter(|s|crate::package::hex(s,16)).ok_or_else(NativeError::params)?)}else{None};
                let (Some(device),Some(inode))=(identity["device"].as_u64(),identity["inode"].as_u64()) else{return Ok(false)};
                if row[1]!=sha || m.len()!=size || identity["size"]!=m.len() || identity["modifiedSeconds"]!=m.mtime() || identity["modifiedNanos"]!=m.mtime_nsec(){return Ok(false);}
                if let Some(lock)=lock {self.locked(lock)?;return paths::same_persisted_object(self.root(),lock.directory(),&path,&file,device,inode,fsid);}
                if fsid.is_none() { return Ok(device==m.dev() && inode==m.ino()); }
                let root=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(self.root())?;
                return paths::same_persisted_object(self.root(),&root,&path,&file,device,inode,fsid);
            }
        }
        Ok(false)
    }

    pub(crate) fn maintenance_referenced(&self,id:&str)->Result<bool,NativeError>{
        let mut doc=self.document()?;doc.remove("importLedger");
        Ok(doc.dump().contains(id)
            || self.journals()?.members().filter(|j|j["body"]["kind"]!="maintenance").any(|j|j.dump().contains(id)))
    }
}

#[cfg(test)]
#[test]
fn item48_legacy_receipt_does_not_prove_unrecorded_or_changed_fields() {
    let f=crate::test_support::Fixture::new();
    crate::test_support::import(&mut crate::native::NativeService::new(),&f);
    let st=Store::open(&f.data).unwrap();let mut doc=st.document().unwrap();
    let old=snapshot_document(&doc,None).unwrap();
    doc["gameFolderChanged"]=true.into();doc["previousGameFolders"]=json::array![object!{gamePath:"/fixture/old"}];
    assert!(!matches_recorded_snapshot(&doc,&old).unwrap());
    for key in ["revision","canRestore","pendingReason","warnings"] {
        let mut bad=old.clone();bad[key]="changed".into();
        assert!(!matches_recorded_snapshot(&doc,&bad).unwrap(),"accepted changed {key}");
        let mut missing=old.clone();missing.remove(key);
        assert!(!matches_recorded_snapshot(&doc,&missing).unwrap(),"accepted missing {key}");
    }
    assert_eq!(doc["gameFolderChanged"],true);
    assert!(doc["applied"].is_null());
}

#[cfg(test)]
#[test]
fn item93_legacy_payload_receipt_survives_btrfs_device_change() {
    let fixture=crate::test_support::Fixture::new();
    crate::test_support::import(&mut crate::native::NativeService::new(),&fixture);
    let lock=RootLock::acquire(&fixture.data).unwrap();
    if !paths::test_volatile_filesystem(lock.directory()) { return; }
    let store=Store::open_writer(&lock).unwrap();
    let id="11111111111111111111111111111111";
    let path=store.payload_path(id).unwrap();
    std::fs::write(&path,b"owned payload").unwrap();
    let fp=paths::fingerprint(&path).unwrap();
    let mut receipt=crate::apply::identity(&std::fs::metadata(&path).unwrap());
    receipt["device"]=(fp.device+1).into();
    store.db.execute("CREATE TABLE IF NOT EXISTS payload_inode_receipts (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, identity TEXT NOT NULL)",&[]).unwrap();
    store.db.execute("INSERT INTO payload_inode_receipts(id,sha256,identity) VALUES(?,?,?)",&[id,&fp.sha256,&receipt.dump()]).unwrap();
    assert!(store.payload_owned_with_lock(id,&fp.sha256,fp.size,Some(&lock)).unwrap(),"the locked btrfs root and intact inode preserve ownership");
    assert!(!store.payload_owned(id,&fp.sha256,fp.size).unwrap(),"a legacy mismatch needs the lock");
    receipt["fsid"]=paths::persisted_filesystem_id(&paths::open_regular(&path).unwrap()).unwrap().into();
    store.db.execute("UPDATE payload_inode_receipts SET identity=? WHERE id=?",&[&receipt.dump(),id]).unwrap();
    assert!(store.payload_owned(id,&fp.sha256,fp.size).unwrap(),"a modern receipt uses its filesystem id even during a read-only scan");
    assert!(!store.payload_owned_with_lock(id,&"0".repeat(64),fp.size,Some(&lock)).unwrap(),"the receipt hash remains mandatory");
    receipt["inode"]=(fp.inode+1).into();
    store.db.execute("UPDATE payload_inode_receipts SET identity=? WHERE id=?",&[&receipt.dump(),id]).unwrap();
    assert!(!store.payload_owned_with_lock(id,&fp.sha256,fp.size,Some(&lock)).unwrap(),"a changed inode is never adopted");
}

#[cfg(test)]
mod apply_receipt_pruning_tests {
    use super::*;
    use crate::test_support::{Fixture, import};
    use std::{fs, os::unix::fs::PermissionsExt};

    struct BlockedDirectory { path: PathBuf, permissions: fs::Permissions }
    impl BlockedDirectory {
        fn new(path: &Path) -> Self {
            let permissions=fs::metadata(path).unwrap().permissions();
            fs::set_permissions(path,fs::Permissions::from_mode(0)).unwrap();
            Self { path:path.to_owned(),permissions }
        }
    }
    impl Drop for BlockedDirectory {
        fn drop(&mut self) { fs::set_permissions(&self.path,self.permissions.clone()).unwrap(); }
    }
    fn fixture() -> Fixture {
        let f=Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        import(&mut crate::native::NativeService::new(),&f);f
    }
    fn move_bulk(f:&Fixture,name:&str)->PathBuf {
        let destination=f.root.join(name);fs::create_dir(&destination).unwrap();
        let mut params=object!{dataDir:f.data.to_str(),destination:destination.to_str()};
        let review=crate::storage::dispatch("native.storage.review",&params).unwrap();
        params["token"]=review["token"].clone();params["operationId"]=format!("move-{name}").into();
        crate::storage::dispatch("native.storage.move",&params).unwrap();
        Store::open(&f.data).unwrap().storage_root().unwrap()
    }
    fn record_file(store:&Store,path:&Path) {
        let _scope=paths::HashMemo::default().enter();
        let fp=paths::fingerprint(path).unwrap();
        store.remember_apply_file(path,&fp.sha256,fp.size).unwrap();
        assert!(has_receipt(store,path));
    }
    fn has_receipt(store:&Store,path:&Path)->bool {
        store.db.query("SELECT path FROM apply_verified_files WHERE path=?",&[&path.to_string_lossy()]).unwrap().is_some()
    }
    #[test]
    fn item104_f4_pruning_ignores_nontraversable_old_storage() {
        let f=fixture();let old_bulk=move_bulk(&f,"drive-a");
        let lock=RootLock::acquire(&f.data).unwrap();let store=Store::open_writer(&lock).unwrap();
        let plan=crate::apply::ApplyPlan::review(&store,"default",&f.game).unwrap();
        let revision:u64=store.document().unwrap()["revision"].as_str().unwrap().parse().unwrap();
        let input=object!{method:"native.apply",params:object!{operationId:"apply-a",expectedRevision:revision.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&store,&lock,revision,"apply-a",&input).unwrap();
        let view=crate::generation::decode(&store.document().unwrap()["applied"]["manifest"]).unwrap();
        let payload_id=view.files[0].payload.id.clone();
        let old_payload=store.payload_path(&payload_id).unwrap();assert!(old_payload.starts_with(&old_bulk));
        assert!(has_receipt(&store,&old_payload));drop(store);drop(lock);
        let new_bulk=move_bulk(&f,"drive-b");
        let lock=RootLock::acquire(&f.data).unwrap();let store=Store::open_writer(&lock).unwrap();
        let current_payload=store.payload_path(&payload_id).unwrap();assert!(current_payload.starts_with(&new_bulk));
        record_file(&store,&current_payload);
        let game_payload=f.game.join("game/citadel/deadlock_native").join(&view.generation).join(&view.files[0].name);
        assert!(has_receipt(&store,&game_payload));
        let blocked=BlockedDirectory::new(&f.root.join("drive-a"));
        assert_eq!(old_payload.try_exists().unwrap_err().kind(),std::io::ErrorKind::PermissionDenied);
        let result=store.remember_deployment_hashes(&store.document().unwrap());drop(blocked);
        assert!(result.is_ok(),"old storage must never block Apply/recovery receipt housekeeping: {result:?}");
        assert!(!has_receipt(&store,&old_payload));
        assert!(has_receipt(&store,&current_payload));assert!(has_receipt(&store,&game_payload));
        store.remember_deployment_hashes(&store.document().unwrap()).unwrap();
    }
    #[test]
    fn item104_f4_pruning_discards_inaccessible_and_outside_receipts() {
        let f=fixture();let lock=RootLock::acquire(&f.data).unwrap();let store=Store::open_writer(&lock).unwrap();
        let bulk=store.storage_root().unwrap();let blocked_path=bulk.join("cache/repairs/blocked");paths::private_dir(&blocked_path).unwrap();
        let denied=blocked_path.join("proof.vpk");fs::write(&denied,b"fixture").unwrap();paths::make_readonly(&denied).unwrap();record_file(&store,&denied);
        let outside=f.root.join("outside.vpk");fs::write(&outside,b"outside fixture").unwrap();paths::make_readonly(&outside).unwrap();record_file(&store,&outside);
        let current=bulk.join("payloads/ffffffffffffffffffffffffffffffff.vpk");fs::write(&current,b"current fixture").unwrap();paths::make_readonly(&current).unwrap();record_file(&store,&current);
        let blocked=BlockedDirectory::new(&blocked_path);
        assert_eq!(denied.try_exists().unwrap_err().kind(),std::io::ErrorKind::PermissionDenied);
        let result=store.remember_deployment_hashes(&store.document().unwrap());drop(blocked);
        assert!(result.is_ok(),"an inaccessible receipt must expire without blocking Apply/recovery: {result:?}");
        assert!(!has_receipt(&store,&denied));assert!(!has_receipt(&store,&outside));assert!(has_receipt(&store,&current));
        assert_eq!(fs::read(&denied).unwrap(),b"fixture");assert_eq!(fs::read(&outside).unwrap(),b"outside fixture");
    }
}
