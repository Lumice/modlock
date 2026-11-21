//! Explicit empty-library creation. Inspection is read-only; only a held RootLock
//! may commit a bounded private candidate and publish it without replacement.
use crate::{native::{NativeError, operation_input}, paths, store::{self, RootLock, Store}};
use json::{object, JsonValue};
use std::{ffi::CString, fs::{self, File, Metadata, OpenOptions}, io::{self, Read}, os::{fd::{AsRawFd, FromRawFd}, unix::fs::{FileExt, MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}};

const DB: &str = "library.sqlite3";
pub(crate) const SIDES: [&str; 3] = ["library.sqlite3-journal", "library.sqlite3-wal", "library.sqlite3-shm"];
const DB_LIMIT: u64 = 1024 * 1024;
const JOURNAL_LIMIT: u64 = 2 * DB_LIMIT;
const TOTAL_LIMIT: u64 = 24 * DB_LIMIT;
const CONTROL_LIMIT: usize = 64 * 1024;
const CANDIDATE_SCHEMA: [(&str,&str);8] = [
    ("library","CREATE TABLE library (id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL)"),
    ("operations","CREATE TABLE operations (id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL)"),
    ("journals","CREATE TABLE journals (id TEXT PRIMARY KEY, body TEXT NOT NULL)"),
    ("bootstrap_origin","CREATE TABLE bootstrap_origin (id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL CHECK(version=1), root TEXT NOT NULL, operationId TEXT NOT NULL, input TEXT NOT NULL)"),
    ("launch_meta","CREATE TABLE launch_meta(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)"),
    ("launch_settings","CREATE TABLE launch_settings(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)"),
    ("launch_attempts","CREATE TABLE launch_attempts(epoch TEXT NOT NULL,id TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(epoch,id))"),
    ("launch_settings_ops","CREATE TABLE launch_settings_ops(epoch TEXT NOT NULL,id TEXT NOT NULL,input TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(epoch,id))"),
];
extern "C" {
    fn geteuid() -> u32;
    fn linkat(oldfd: i32, old: *const i8, newfd: i32, new: *const i8, flags: i32) -> i32;
    fn mkdirat(fd: i32, name: *const i8, mode: u32) -> i32;
    fn openat(fd: i32, name: *const i8, flags: i32, ...) -> i32;
}
fn error(kind: &str) -> NativeError {
    let message = match kind {
        "bootstrapLimit" => "Bootstrap artifact limits reached. Review/recover the listed native bootstrap artifacts or choose a different empty native data root.",
        "bootstrapStorage" => "Bootstrap artifacts cannot be safely inspected. Preserve them for recovery or choose a different empty native data root.",
        "bootstrapOutcomeUnknown" => "The library may have been published. Retain the operation ID and retry initialize with exactly the same input to reconcile it.",
        "nativeAlreadyInitialized" => "This native library is already initialized; refresh its current snapshot.",
        "orphanLibrarySidecar" => "Native database sidecars exist without a library. Preserve them for recovery.",
        "unknownRootEntry" => "Unrecognized native root entries prevent creation. Preserve and review the listed paths or choose a different empty native data root.",
        "externalChange" => "Bootstrap path identities changed. Preserve all library and candidate artifacts for reconciliation.",
        "unsafeRoot" => "The native root must be an ordinary owned directory with safe nonsymlink ancestors.",
        _ => "The existing native library cannot be read safely. Preserve it and its sidecars for recovery.",
    };
    NativeError::new(kind, message)
}
fn hex_id(s: &str) -> bool { s.len()==32 && s.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
pub(crate) fn candidate_name(s: &str) -> bool { s.strip_prefix(".bootstrap-").is_some_and(hex_id) }
fn identity(m: &Metadata) -> (u64,u64) { (m.dev(),m.ino()) }
fn stamp(m:&Metadata)->(u64,u64,u64,i64,i64,i64,i64) { (m.dev(),m.ino(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()) }
fn owned(m: &Metadata) -> bool { m.uid()==unsafe {geteuid()} && !m.file_type().is_symlink() }
fn meta(path: &Path) -> Result<Option<Metadata>, NativeError> {
    match fs::symlink_metadata(path) { Ok(m)=>Ok(Some(m)), Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(None), Err(_)=>Err(error("bootstrapStorage")) }
}
/// No creation, including for a missing root. Existing ancestors are checked.
pub(crate) fn safe_root(root: &Path) -> Result<bool, NativeError> {
    match meta(root)? {
        Some(m) if m.is_dir() && owned(&m) => { paths::reject_symlinks(root).map_err(|_|error("unsafeRoot"))?; Ok(true) },
        Some(_) => Err(error("unsafeRoot")),
        None => { paths::verify_absent(root).map_err(|_|error("unsafeRoot"))?; Ok(false) }
    }
}
pub(crate) fn validate(method: &str, p: &JsonValue) -> Result<PathBuf, NativeError> {
    let keys: &[&str] = match method {
        "native.library.inspect"=>&["version","dataDir"],
        "native.library.initialize"=>&["version","dataDir","operationId","expectedRevision"],
        _=>return Err(NativeError::params()),
    };
    if !p.is_object() || p.len()!=keys.len() || keys.iter().any(|k|!p.has_key(k)) || p["version"].as_u32()!=Some(1)
        || !p["dataDir"].as_str().is_some_and(|s|!s.is_empty()) {
        return Err(NativeError::params());
    }
    if method=="native.library.initialize" && (p["expectedRevision"]!="0" || !p["operationId"].as_str().is_some_and(hex_id)) {
        return Err(NativeError::params());
    }
    paths::data_root(p)
}
pub(crate) struct Candidate { path: PathBuf, directory: File, database: Option<File>, reusable_shape: bool, bytes: u64 }
impl Candidate {
    fn directory_path(&self) -> PathBuf { PathBuf::from(format!("/proc/self/fd/{}",self.directory.as_raw_fd())) }
    /// Establish the directory relative to the held root, then retain that
    /// directory for every candidate file operation and publication check.
    fn create(lock:&RootLock,name:&str) -> Result<Self,NativeError> {
        if !candidate_name(name) {return Err(NativeError::params());}
        lock.validate().map_err(|_|error("externalChange"))?;
        let name=CString::new(name).map_err(|_|NativeError::params())?;
        if unsafe{mkdirat(lock.directory().as_raw_fd(),name.as_ptr(),0o700)}!=0 {
            let failure=io::Error::last_os_error();
            lock.validate().map_err(|_|error("externalChange"))?;
            return Err(failure.into());
        }
        let fd=unsafe{openat(lock.directory().as_raw_fd(),name.as_ptr(),0x10000|0x20000|0x80000)};
        if fd<0 {
            lock.validate().map_err(|_|error("externalChange"))?;
            // The just-created directory could not be pinned; do not adopt an
            // entry observed later under the same name.
            return Err(error("externalChange"));
        }
        let directory=unsafe{File::from_raw_fd(fd)};
        let path=lock.root().join(name.to_str().map_err(|_|NativeError::params())?);
        let candidate=Self {path,directory,database:None,reusable_shape:true,bytes:0};
        candidate.check_owned(lock)?;
        let held=candidate.directory.metadata()?;
        if held.mode()&0o077!=0 || fs::read_dir(candidate.directory_path())?.next().is_some() {return Err(error("externalChange"));}
        Ok(candidate)
    }
    fn observe(path: &Path) -> Result<Self, NativeError> {
        let named=meta(path)?.ok_or_else(||error("externalChange"))?;
        if !named.is_dir() || !owned(&named) || named.mode()&0o077!=0 { return Err(error("bootstrapStorage")); }
        let directory=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(path)?;
        if identity(&named)!=identity(&directory.metadata()?) { return Err(error("externalChange")); }
        let mut bytes=0u64; let mut database=None; let mut reusable_shape=true;
        let directory_path=PathBuf::from(format!("/proc/self/fd/{}",directory.as_raw_fd()));
        for (n,entry) in fs::read_dir(directory_path)?.enumerate() {
            if n>=2 { return Err(error("bootstrapStorage")); }
            let entry=entry?; let m=fs::symlink_metadata(entry.path())?;
            if !m.is_file() || !owned(&m) { return Err(error("bootstrapStorage")); }
            bytes=bytes.checked_add(m.len()).ok_or_else(||error("bootstrapLimit"))?;
            match entry.file_name().to_str() {
                Some(DB) => {
                    if m.len()>DB_LIMIT { return Err(error("bootstrapLimit")); }
                    let file=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(entry.path())?;
                    if identity(&file.metadata()?)!=identity(&m) { return Err(error("externalChange")); }
                    database=Some(file);
                },
                Some("library.sqlite3-journal") => {
                    if m.len()>JOURNAL_LIMIT { return Err(error("bootstrapLimit")); }
                    // Even a zero-byte leftover journal is preserved, never recovered.
                    reusable_shape=false;
                },
                _ => { reusable_shape=false; if m.len()>JOURNAL_LIMIT {return Err(error("bootstrapLimit"));} },
            }
        }
        let candidate=Self {path:path.to_owned(),directory,database,reusable_shape,bytes};
        candidate.check()?; Ok(candidate)
    }
    fn check(&self) -> Result<(), NativeError> {
        paths::reject_symlinks(&self.path).map_err(|_|error("externalChange"))?;
        let m=fs::symlink_metadata(&self.path).map_err(|_|error("externalChange"))?;
        let directory=self.directory.metadata().map_err(|_|error("externalChange"))?;
        if !m.is_dir() || !owned(&m) || !owned(&directory) || m.mode()&0o077!=0 || identity(&m)!=identity(&directory) {return Err(error("externalChange"));}
        if let Some(file)=&self.database {
            let named=fs::symlink_metadata(self.path.join(DB)).map_err(|_|error("externalChange"))?;
            let anchored=fs::symlink_metadata(self.directory_path().join(DB)).map_err(|_|error("externalChange"))?;
            let held=file.metadata().map_err(|_|error("externalChange"))?;
            if !named.is_file() || !anchored.is_file() || !held.is_file() || !owned(&named) || !owned(&anchored) || !owned(&held)
                || identity(&named)!=identity(&held) || identity(&anchored)!=identity(&held) || held.len()>DB_LIMIT {
                return Err(error("externalChange"));
            }
        }
        Ok(())
    }
    pub(crate) fn check_owned(&self,lock:&RootLock) -> Result<(),NativeError> {
        if self.path.parent()!=Some(lock.root()) || !self.path.file_name().and_then(|s|s.to_str()).is_some_and(candidate_name) {return Err(NativeError::params());}
        lock.validate().map_err(|_|error("externalChange"))?;
        self.check()
    }
    pub(crate) fn check_empty_database(&self,lock:&RootLock) -> Result<(),NativeError> {
        self.check_owned(lock)?;
        let m=self.database.as_ref().ok_or_else(||error("externalChange"))?.metadata()?;
        if m.nlink()!=1 || m.len()!=0 {return Err(error("externalChange"));}
        Ok(())
    }
    fn create_database(&mut self,lock:&RootLock) -> Result<(),NativeError> {
        self.check_owned(lock)?;
        if self.database.is_some() {return Err(error("externalChange"));}
        let fd=unsafe{openat(self.directory.as_raw_fd(),c"library.sqlite3".as_ptr(),2|0x40|0x80|0x20000|0x80000,0o600u32)};
        if fd<0 {
            let failure=io::Error::last_os_error();
            self.check_owned(lock)?;
            if failure.kind()==io::ErrorKind::AlreadyExists {return Err(error("externalChange"));}
            return Err(failure.into());
        }
        self.database=Some(unsafe{File::from_raw_fd(fd)});
        self.check_owned(lock)?;
        let m=self.database.as_ref().unwrap().metadata()?;
        if m.nlink()!=1 || m.len()!=0 {return Err(error("externalChange"));}
        Ok(())
    }
    fn clean_shape(&self) -> Result<bool,NativeError> {
        let mut found=false;
        for (n,entry) in fs::read_dir(self.directory_path())?.enumerate() {
            if n>=2 {return Ok(false);}
            let entry=entry?;
            if entry.file_name()!=DB || found {return Ok(false);}
            found=true;
        }
        Ok(found)
    }
    /// Read only the retained inode, and reject a changed image or any sidecar.
    pub(crate) fn read_image(&self,lock:&RootLock) -> Result<(Vec<u8>,Metadata),NativeError> {
        self.check_owned(lock)?;
        if !self.reusable_shape || !self.clean_shape()? {return Err(error("bootstrapStorage"));}
        for side in SIDES {if meta(&self.directory_path().join(side))?.is_some(){return Err(error("bootstrapStorage"));}}
        let file=self.database.as_ref().ok_or_else(||error("bootstrapStorage"))?;
        let before=file.metadata()?;
        if before.nlink()!=1 || before.len()<100 || before.len()>DB_LIMIT {return Err(error("bootstrapStorage"));}
        let mut image=vec![0u8;before.len() as usize];
        let read=file.read_exact_at(&mut image,0);
        if stamp(&before)!=stamp(&file.metadata()?) {return Err(error("externalChange"));}
        self.check_owned(lock)?;
        read?;
        if &image[..16]!=b"SQLite format 3\0" || image[18]!=1 || image[19]!=1 {return Err(error("bootstrapStorage"));}
        let page_size=match u16::from_be_bytes([image[16],image[17]]) {1=>65536usize,n=>n as usize};
        let pages=u32::from_be_bytes([image[28],image[29],image[30],image[31]]) as usize;
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two()
            || pages.checked_mul(page_size)!=Some(image.len()) || pages==0 || image[24..28]!=image[92..96] {
            return Err(error("bootstrapStorage"));
        }
        Ok((image,before))
    }
    fn write_image(&self,lock:&RootLock,image:&[u8]) -> Result<(),NativeError> {
        self.check_owned(lock)?;
        let file=self.database.as_ref().ok_or_else(||error("bootstrapStorage"))?;
        let before=file.metadata()?;
        if before.nlink()!=1 || before.len()!=0 || image.is_empty() || image.len() as u64>DB_LIMIT {return Err(error("externalChange"));}
        let result=file.write_all_at(image,0);
        self.check_owned(lock)?;
        let after=file.metadata()?;
        if after.nlink()!=1 || (result.is_ok() && after.len()!=image.len() as u64) {return Err(error("externalChange"));}
        result.map_err(Into::into)
    }
}
struct Admission { candidates: Vec<Candidate>, reason: Option<&'static str>, blocked: Vec<String>, truncated: bool, count: usize, bytes:u64 }
impl Admission {
    fn block_path(&mut self,path: String) {
        if !path.is_empty() && self.blocked.len()<8 && path.len()<=512 {self.blocked.push(path);} else {self.truncated=true;}
    }
    fn reject(&mut self,reason:&'static str,path:String) {if self.reason.is_none(){self.reason=Some(reason);} self.block_path(path);}
}
fn scan(root:&Path, final_exists:bool) -> Result<Admission,NativeError> {
    let mut a=Admission {candidates:Vec::new(),reason:None,blocked:Vec::new(),truncated:false,count:0,bytes:0};
    let mut total=0u64;
    for (n,entry) in fs::read_dir(root)?.enumerate() {
        if n>=256 {a.reject("bootstrapLimit",String::new());a.truncated=true;break;}
        let entry=entry?;let name=entry.file_name();let text=name.to_string_lossy().into_owned();
        let m=fs::symlink_metadata(entry.path())?;
        if name.to_str().is_some_and(candidate_name) {
            a.count+=1;
            if a.count>8 {a.reject("bootstrapLimit",text);continue;}
            a.block_path(text.clone());
            match Candidate::observe(&entry.path()) {
                Ok(c)=>{total=total.checked_add(c.bytes).ok_or_else(||error("bootstrapLimit"))?;a.candidates.push(c);},
                Err(e)=>{if !final_exists {a.reject(if e.kind=="bootstrapLimit" {"bootstrapLimit"} else {"bootstrapStorage"},text);}}
            }
            continue;
        }
        if final_exists {continue;}
        if SIDES.contains(&text.as_str()) {a.reject("orphanLibrarySidecar",text);continue;}
        let allowed=match name.to_str() {
            Some("cache")=>m.is_dir() && owned(&m),
            Some(".writer.lock"|"catalog-preferences-v1.json"|".catalog-preferences-v1.lock"|".catalog-preferences-v1.replace")=>m.is_file() && owned(&m) && m.nlink()==1,
            _=>false,
        };
        if !allowed {a.reject("unknownRootEntry",text);}
    }
    if total>TOTAL_LIMIT {a.reject("bootstrapLimit",String::new());}
    a.bytes=total;
    Ok(a)
}
fn cold_file(path:&Path)->Result<(File,Metadata),NativeError> {
    let mut file=paths::open_regular(path)?;let before=file.metadata()?;
    let mut header=[0u8;100];file.read_exact(&mut header).map_err(|_|error("unreadableLibrary"))?;
    if &header[..16]!=b"SQLite format 3\0" || header[18]!=1 || header[19]!=1 {return Err(error("unreadableLibrary"));}
    if stamp(&before)!=stamp(&file.metadata()?) {return Err(error("externalChange"));}
    Ok((file,before))
}
fn unchanged(path:&Path,file:&File,before:&Metadata)->Result<(),NativeError> {
    let named=fs::symlink_metadata(path)?;
    if !named.is_file() || !owned(&named) || stamp(&named)!=stamp(before) || stamp(&file.metadata()?)!=stamp(before) {return Err(error("externalChange"));} Ok(())
}
struct FinalStore { store:Store, file:File, before:Metadata }
fn final_store(root:&Path) -> Result<Option<FinalStore>,NativeError> {
    let Some(m)=meta(&root.join(DB))? else {return Ok(None)};
    if !m.is_file() || !owned(&m) {return Err(error("unsafeRoot"));}
    // A hot DELETE journal or WAL/SHM needs recovery, not read-only bootstrap
    // discovery (SQLite can otherwise create a WAL shared-memory sidecar).
    for side in SIDES {if meta(&root.join(side))?.is_some(){return Err(error("unreadableLibrary"));}}
    let (file,before)=cold_file(&root.join(DB))?;
    let store=Store::bootstrap_final(root)?;
    store::ensure_readable(&store.document()?)?;
    unchanged(&root.join(DB),&file,&before)?;
    for side in SIDES {if meta(&root.join(side))?.is_some(){return Err(error("unreadableLibrary"));}}
    Ok(Some(FinalStore{store,file,before}))
}
pub struct BootstrapState;
impl BootstrapState {
    pub fn inspect(root:&Path) -> Result<JsonValue,NativeError> {
        let root=paths::absolute(root)?;
        let root_text=root.to_str().ok_or_else(NativeError::params)?;
        let mut result=object!{version:1,root:root_text,state:"absent",canCreate:true,revision:JsonValue::Null,reason:JsonValue::Null,candidates:0,blockedPaths:JsonValue::new_array(),pathsTruncated:false};
        match safe_root(&root) {
            Ok(false)=>return Ok(result),Ok(true)=>{},Err(e)=>{result["state"]="blocked".into();result["canCreate"]=false.into();result["reason"]=if e.kind=="unsafeRoot" {"unsafeRoot"} else {"bootstrapStorage"}.into();return Ok(result);}
        }
        let exists=meta(&root.join(DB))?.is_some();
        let admission=scan(&root,exists);
        if let Ok(a)=&admission {
            result["candidates"]=a.count.into();result["blockedPaths"]=a.blocked.clone().into();result["pathsTruncated"]=a.truncated.into();
        }
        if exists {
            result["canCreate"]=false.into();
            match final_store(&root) {
                Ok(Some(s))=>{let doc=s.store.document()?;unchanged(&root.join(DB),&s.file,&s.before)?;result["state"]="initialized".into();result["reason"]="existingLibrary".into();result["revision"]=doc["revision"].clone();},
                other=>{
                    let kind=match other {Err(e) if e.kind=="unsupportedVersion"=>"unsupportedLibrary",Err(e) if e.kind=="unsafeRoot"=>"unsafeRoot",_=>"unreadableLibrary"};
                    result["state"]="blocked".into();result["reason"]=kind.into();
                    // A final database diagnosis takes priority over retained
                    // candidates in the small presentation-only artifact list.
                    let mut blocked=vec![DB.to_owned()];
                    for side in SIDES {if meta(&root.join(side))?.is_some(){blocked.push(side.to_owned());}}
                    let mut truncated=result["pathsTruncated"].as_bool().unwrap_or(false);
                    for path in result["blockedPaths"].members().filter_map(JsonValue::as_str) {if blocked.len()<8 {blocked.push(path.to_owned());}else{truncated=true;break;}}
                    result["pathsTruncated"]=truncated.into();
                    result["blockedPaths"]=blocked.into();
                },
            }
            return Ok(result);
        }
        let reason=match admission {Ok(a)=>a.reason.or(if a.count>=8 || a.bytes>TOTAL_LIMIT-JOURNAL_LIMIT-DB_LIMIT {Some("bootstrapLimit")}else{None}),Err(_)=>Some("bootstrapStorage")};
        if let Some(reason)=reason {result["state"]="blocked".into();result["canCreate"]=false.into();result["reason"]=reason.into();}
        Ok(result)
    }
}
fn empty_document() -> JsonValue {
    object!{storeVersion:2,revision:"0",sourcePath:"",gamePath:"",activeProfileId:"default",profiles:object!{default:object!{id:"default",name:"Default",source:object!{},mods:JsonValue::new_array(),protected:true}},importArchive:object!{},importLedger:object!{},applied:JsonValue::Null,canRestore:false,recoveryRequired:false}
}
fn small(value:&JsonValue)->Result<(),NativeError> {
    if crate::protocol::native_value_bytes(value).is_none_or(|n|n+256>CONTROL_LIMIT){return Err(error("bootstrapLimit"));} Ok(())
}
/// Only complete, cold, fixed empty candidates with matching root provenance
/// participate in replay. Malformed/unfinished candidates stay untouched.
fn complete(lock:&RootLock,c:&Candidate)->Result<Option<(String,JsonValue,JsonValue)>,NativeError> {
    c.check_owned(lock)?;
    if !c.reusable_shape || c.database.as_ref().is_none_or(|f|f.metadata().map_or(true,|m|m.nlink()!=1)) {return Ok(None);}
    let read=||->Result<(String,JsonValue,JsonValue),NativeError>{
        let (s,before)=Store::bootstrap_candidate(lock,c,false)?;
        let before=before.ok_or_else(||error("bootstrapStorage"))?;
        let db=s.bootstrap_database();
        if db.query("PRAGMA integrity_check(1)",&[])?.as_ref().and_then(|r|r.first()).map(String::as_str)!=Some("ok") {return Err(error("bootstrapStorage"));}
        let row=db.query("SELECT version,root,operationId,input FROM bootstrap_origin WHERE id=1",&[])?.ok_or_else(||error("bootstrapStorage"))?;
        if row[0]!="1" || row[1]!=lock.root().to_str().ok_or_else(NativeError::params)? || !hex_id(&row[2]) {return Err(error("bootstrapStorage"));}
        let input=json::parse(&row[3]).map_err(|_|error("bootstrapStorage"))?;
        let p=&input["params"];
        if input.len()!=2 || input["method"]!="native.library.initialize" || !p.is_object() || p.len()!=3
            || p["version"].as_u32()!=Some(1) || p["operationId"]!=row[2] || p["expectedRevision"]!="0" || store::canonical(&input)!=row[3] {return Err(error("bootstrapStorage"));}
        for (table,count) in [("library",1),("operations",1),("journals",0),("bootstrap_origin",1),("launch_meta",0),("launch_settings",0),("launch_attempts",0),("launch_settings_ops",0)] {
            if db.query(&format!("SELECT count(*) FROM {table}"),&[])?.as_ref().map(|r|r[0].as_str())!=Some(if count==1 {"1"}else{"0"}) {return Err(error("bootstrapStorage"));}
        }
        // Reusing a lookalike table without normal keys/checks would publish a
        // library that later writers cannot safely update. Keep schema admission
        // local to fixed bootstrap candidates, without migrating unknown stores.
        let normalize=|s:&str|s.chars().filter(|c|!c.is_ascii_whitespace()).collect::<String>().to_ascii_lowercase().replace("ifnotexists","");
        for (name,expected) in CANDIDATE_SCHEMA {
            let row=db.query("SELECT sql FROM sqlite_master WHERE type='table' AND name=?",&[name])?.ok_or_else(||error("bootstrapStorage"))?;
            if normalize(&row[0])!=normalize(expected) {return Err(error("bootstrapStorage"));}
        }
        if db.query("SELECT count(*) FROM sqlite_master WHERE type='table'",&[])?.as_ref().map(|r|r[0].as_str())!=Some("8")
            || db.query("SELECT name FROM sqlite_master WHERE type IN ('trigger','view') OR (type='index' AND sql IS NOT NULL) LIMIT 1",&[])?.is_some() {return Err(error("bootstrapStorage"));}
        let mut doc=empty_document();doc["revision"]="1".into();
        if s.document()?!=doc {return Err(error("bootstrapStorage"));}
        store::ensure_readable(&doc)?;
        let result=s.replay(&row[2],&input)?.ok_or_else(||error("bootstrapStorage"))?;
        if result!=store::snapshot_document(&doc,None)? {return Err(error("bootstrapStorage"));}
        small(&result)?;small(&object!{operationId:row[2].clone(),input:input.clone(),outcome:object!{status:"succeeded",result:result.clone()}})?;
        let output=(row[2].clone(),input,result);
        s.bootstrap_close()?;
        let file=c.database.as_ref().ok_or_else(||error("externalChange"))?;
        if stamp(&before)!=stamp(&file.metadata()?) {return Err(error("externalChange"));}
        c.check_owned(lock)?;
        if !c.clean_shape()? {return Err(error("bootstrapStorage"));}
        for side in SIDES {if meta(&c.directory_path().join(side))?.is_some(){return Err(error("bootstrapStorage"));}}
        Ok(output)
    };
    let value=read();c.check_owned(lock)?;
    match value {Ok(value)=>Ok(Some(value)),Err(e) if e.kind=="externalChange"=>Err(e),Err(_)=>Ok(None)}
}
fn authoritative(lock:&RootLock,id:&str,input:&JsonValue)->Result<Option<JsonValue>,NativeError> {
    lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
    let Some(s)=final_store(lock.root()).map_err(|_|error("bootstrapOutcomeUnknown"))? else{
        lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
        return Ok(None)
    };
    // A conflict or an existing-library result is settled only if the final
    // authority used for replay is still the same cold file afterwards.
    let replay=s.store.replay(id,input);
    lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
    unchanged(&lock.root().join(DB),&s.file,&s.before).map_err(|_|error("bootstrapOutcomeUnknown"))?;
    for side in SIDES {
        if meta(&lock.root().join(side)).map_err(|_|error("bootstrapOutcomeUnknown"))?.is_some() {
            return Err(error("bootstrapOutcomeUnknown"));
        }
    }
    let result=replay.map_err(|e|if e.kind=="operationIdConflict" {e}else{error("bootstrapOutcomeUnknown")})?.ok_or_else(||error("nativeAlreadyInitialized"))?;
    // Historical result remains historical. The paired UI must fetch a fresh
    // current snapshot after it confirms this receipt.
    paths::sync_dir(lock.root()).map_err(|_|error("bootstrapOutcomeUnknown"))?;
    lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
    s.store.snapshot(None).map_err(|_|error("bootstrapOutcomeUnknown"))?;
    unchanged(&lock.root().join(DB),&s.file,&s.before).map_err(|_|error("bootstrapOutcomeUnknown"))?;
    for side in SIDES {
        if meta(&lock.root().join(side)).map_err(|_|error("bootstrapOutcomeUnknown"))?.is_some() {
            return Err(error("bootstrapOutcomeUnknown"));
        }
    }
    Ok(Some(result))
}
fn publish(lock:&RootLock,c:&Candidate,id:&str,input:&JsonValue)->Result<JsonValue,NativeError> {
    c.check_owned(lock)?;
    let file=c.database.as_ref().ok_or_else(||error("bootstrapStorage"))?;
    if file.metadata()?.nlink()!=1 || file.metadata()?.dev()!=lock.directory().metadata()?.dev() {return Err(error("externalChange"));}
    let synced=(||->Result<(),NativeError>{file.sync_all()?;c.directory.sync_all()?;Ok(())})();
    c.check_owned(lock)?;synced?;
    // Read the retained inode after SQLite is closed and file/directory durability.
    let before=file.metadata()?;
    let (_,candidate_input,_)=complete(lock,c)?.ok_or_else(||error("bootstrapStorage"))?;
    if candidate_input!=*input {return Err(error("externalChange"));}
    if stamp(&before)!=stamp(&file.metadata()?) {return Err(error("externalChange"));}
    if meta(&lock.root().join(DB)).map_err(|_|error("bootstrapOutcomeUnknown"))?.is_some() {
        return authoritative(lock,id,input)?.ok_or_else(||error("bootstrapOutcomeUnknown"));
    }
    for side in SIDES {if meta(&lock.root().join(side))?.is_some(){return Err(error("orphanLibrarySidecar"));}}
    c.check_owned(lock)?;
    if stamp(&before)!=stamp(&file.metadata()?) {return Err(error("externalChange"));}
    // Linux procfs resolves this generated path to the held file itself, so a
    // swapped candidate basename can never become the publication source.
    let source=CString::new(format!("/proc/self/fd/{}",file.as_raw_fd())).map_err(|_|NativeError::params())?;
    let linked=unsafe{linkat(-100,source.as_ptr(),lock.directory().as_raw_fd(),c"library.sqlite3".as_ptr(),0x400)};
    if linked!=0 {
        let err=io::Error::last_os_error();
        // Some filesystems can return an error after a remotely completed link.
        // Consult final authority for every link error, not only EEXIST.
        lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
        if meta(&lock.root().join(DB)).map_err(|_|error("bootstrapOutcomeUnknown"))?.is_some() {
            return authoritative(lock,id,input).map_err(|e|if matches!(e.kind.as_str(),"nativeAlreadyInitialized"|"operationIdConflict"){e}else{error("bootstrapOutcomeUnknown")})?.ok_or_else(||error("bootstrapOutcomeUnknown"));
        }
        if err.kind()==io::ErrorKind::AlreadyExists {return Err(error("bootstrapOutcomeUnknown"));}
        c.check_owned(lock)?;
        if file.metadata()?.nlink()!=1 {return Err(error("externalChange"));}
        return Err(error("bootstrapStorage"));
    }
    // Everything after a successful link is potentially committed. Never unlink
    // final, even if an identity, fsync or final read becomes ambiguous.
    let confirmed=||->Result<JsonValue,NativeError>{
        c.check_owned(lock)?;
        let final_meta=fs::symlink_metadata(lock.root().join(DB))?;
        let held=file.metadata()?;
        if identity(&final_meta)!=identity(&held) || held.nlink()!=2 || before.len()!=held.len()
            || before.mtime()!=held.mtime() || before.mtime_nsec()!=held.mtime_nsec() {return Err(error("externalChange"));}
        paths::sync_dir(lock.root())?;
        let result=authoritative(lock,id,input)?.ok_or_else(||error("bootstrapOutcomeUnknown"))?;
        c.check_owned(lock)?;
        if identity(&fs::symlink_metadata(lock.root().join(DB))?)!=identity(&file.metadata()?) {return Err(error("externalChange"));}
        Ok(result)
    };
    let result=confirmed().map_err(|_|error("bootstrapOutcomeUnknown"))?;
    // An unlink by basename cannot atomically require the held inode. Keep the
    // candidate link and directory instead of risking deletion of a replacement.
    Ok(result)
}
pub fn initialize(lock:&RootLock,operation_id:&str,input:&JsonValue)->Result<JsonValue,NativeError> {
    if !hex_id(operation_id) {return Err(NativeError::params());}
    let mut params=input["params"].clone();params["dataDir"]=lock.root().to_str().ok_or_else(NativeError::params)?.into();
    if validate("native.library.initialize",&params)?!=lock.root() || operation_input("native.library.initialize",&params)!=*input || params["operationId"]!=operation_id {return Err(NativeError::params());}
    safe_root(lock.root()).map_err(|_|error("bootstrapOutcomeUnknown"))?;
    lock.validate().map_err(|_|error("bootstrapOutcomeUnknown"))?;
    if let Some(result)=authoritative(lock,operation_id,input)? {return Ok(result);}
    let a=scan(lock.root(),false)?;
    if let Some(reason)=a.reason {return Err(error(reason));}
    for c in &a.candidates {
        if let Some((id,stored,_))=complete(lock,c)? {
            if id==operation_id {
                if store::canonical(&stored)!=store::canonical(input) {return Err(NativeError::new("operationIdConflict","This complete bootstrap candidate belongs to different inputs."));}
                return publish(lock,c,operation_id,input);
            }
        }
    }
    if a.count>=8 || a.bytes>TOTAL_LIMIT-JOURNAL_LIMIT-DB_LIMIT {return Err(error("bootstrapLimit"));}
    let name=format!(".bootstrap-{}",paths::random_id()?);
    let mut candidate=Candidate::create(lock,&name)?;
    let root_sync=lock.directory().sync_all();
    candidate.check_owned(lock)?;root_sync?;
    let prepared=(||->Result<(),NativeError>{
        let doc=empty_document();store::ensure_readable(&doc)?;
        let mut committed=doc.clone();committed["revision"]="1".into();
        let snapshot=store::snapshot_document(&committed,None)?;
        small(&snapshot)?;small(&object!{operationId:operation_id,input:input.clone(),outcome:object!{status:"succeeded",result:snapshot}})?;
        candidate.create_database(lock)?;
        let image=(||->Result<Vec<u8>,NativeError>{
            let (s,_)=Store::bootstrap_candidate(lock,&candidate,true)?;
            candidate.check_owned(lock)?;
            let db=s.bootstrap_database();
            db.execute("INSERT INTO bootstrap_origin(id,version,root,operationId,input) VALUES(1,1,?,?,?)",&[lock.root().to_str().ok_or_else(NativeError::params)?,operation_id,&store::canonical(input)])?;
            candidate.check_owned(lock)?;
            s.commit(lock,0,operation_id,input,doc,None)?;
            candidate.check_owned(lock)?;
            s.bootstrap_serialize()
        })();
        candidate.check_owned(lock)?;
        candidate.write_image(lock,&image?)
    })();
    candidate.check_owned(lock)?;
    prepared?;
    publish(lock,&candidate,operation_id,input)
}
