//! Private, bounded preferences for exact replacement targets.
use super::{authority::FreshAuthorization, contract::SessionId, job_store::RemoteStoreLease,
    replacement_contract, replacement_target};
use crate::{catalog::contract as common, native::NativeError, paths, protocol,
    store::{self, RootLock}};
use json::{object, JsonValue};
use std::{ffi::CString, fs::{File, Metadata}, io::{self, Read, Write},
    os::{fd::{AsRawFd, FromRawFd}, unix::fs::MetadataExt}, time::Instant};

const NAME: &str = "remote-skips.v1.json";
const LIMIT: usize = 1024 * 1024;
const ENTRIES: usize = 1024;
const O_NOFOLLOW: i32 = 0x20000;
const O_CLOEXEC: i32 = 0x80000;
const O_NONBLOCK: i32 = 0x800;
unsafe extern "C" {
    fn geteuid() -> u32;
    fn openat(dirfd: i32, name: *const i8, flags: i32, ...) -> i32;
    fn renameat2(olddirfd: i32, oldpath: *const i8, newdirfd: i32, newpath: *const i8, flags: u32) -> i32;
    fn unlinkat(dirfd: i32, name: *const i8, flags: i32) -> i32;
}
fn unavailable() -> NativeError {
    NativeError::new("outcomeUnknown", "The saved skip preference is unavailable; preserve its evidence.")
}
fn conflict() -> NativeError {
    NativeError::new("staleReview", "The saved skip preference changed; observe it again.")
}
fn full() -> NativeError {
    NativeError::new("storageFull", "The saved skip preference limit is full.")
}
#[derive(Clone, PartialEq, Eq)]
struct Stamp { device:u64, inode:u64, size:u64, mtime:i64, mtime_ns:i64, ctime:i64, ctime_ns:i64, mode:u32, uid:u32, links:u64 }
impl Stamp {
    fn from(m: &Metadata) -> Self { Self {device:m.dev(),inode:m.ino(),size:m.len(),mtime:m.mtime(),
        mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec(),mode:m.mode(),uid:m.uid(),links:m.nlink()} }
}
fn owned(m: &Metadata) -> bool {
    m.is_file() && m.uid() == unsafe { geteuid() } && m.nlink() == 1 && m.mode() & 0o077 == 0
}
fn safe_root(lease:&RemoteStoreLease)->Result<(),NativeError>{
    lease.validate().map_err(|_|unavailable())?;let root=lease.directory().metadata().map_err(|_|unavailable())?;
    if !root.is_dir()||root.uid()!=unsafe{geteuid()}||root.mode()&0o022!=0{return Err(unavailable());}Ok(())
}
fn open_name(lease: &RemoteStoreLease, name: &str, flags: i32, mode: u32) -> io::Result<File> {
    let name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let fd = unsafe { openat(lease.directory().as_raw_fd(), name.as_ptr(), flags | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK, mode) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { File::from_raw_fd(fd) }) }
}
fn empty(lease: &RemoteStoreLease) -> Result<JsonValue, NativeError> {
    let (device, inode) = lease.identity();
    let fsid=paths::persisted_filesystem_id(lease.root_directory()).map_err(|_|unavailable())?;
    Ok(object!{version:1,rootIdentity:object!{device:device.to_string(),inode:inode.to_string(),fsid:fsid},entries:JsonValue::new_array()})
}
fn admit(value: &JsonValue, lease: &RemoteStoreLease) -> Result<(), NativeError> {
    common::exact(value, &["version", "rootIdentity", "entries"]).map_err(|_| unavailable())?;
    let root = &value["rootIdentity"];
    if root.has_key("fsid") {common::exact(root,&["device","inode","fsid"])} else {common::exact(root,&["device","inode"])}.map_err(|_|unavailable())?;
    let identity = (common::u64_text(&root["device"]).map_err(|_| unavailable())?,
        common::u64_text(&root["inode"]).map_err(|_| unavailable())?);
    let fsid=if root.has_key("fsid") {Some(root["fsid"].as_str().filter(|s|crate::package::hex(s,16)).ok_or_else(unavailable)?)} else {None};
    if value["version"] != 1 || !paths::same_persisted_object(lease.root(),lease.root_directory(),lease.root(),lease.root_directory(),identity.0,identity.1,fsid).map_err(|_|unavailable())? || !value["entries"].is_array()
        || value["entries"].len() > ENTRIES { return Err(unavailable()); }
    let mut previous: Option<(String,String)> = None;
    for row in value["entries"].members() {
        common::exact(row, &["profileId", "modId", "targetDigest", "fileId", "candidateFingerprint"]).map_err(|_| unavailable())?;
        let profile = row["profileId"].as_str().filter(|s| paths::safe_id(s)).ok_or_else(unavailable)?;
        let id = row["modId"].as_str().filter(|s| paths::safe_id(s)).ok_or_else(unavailable)?;
        replacement_contract::skip_entry(&object!{targetDigest:row["targetDigest"].clone(),fileId:row["fileId"].clone(),candidateFingerprint:row["candidateFingerprint"].clone()})
            .map_err(|_| unavailable())?;
        let key = (profile.to_owned(), id.to_owned());
        if previous.as_ref().is_some_and(|p| p >= &key) { return Err(unavailable()); }
        previous = Some(key);
    }
    Ok(())
}
struct Snapshot { document: JsonValue, stamp: Option<Stamp> }
fn load(lease: &RemoteStoreLease) -> Result<Snapshot, NativeError> {
    load_named(lease,NAME)
}
fn load_named(lease: &RemoteStoreLease,name:&str) -> Result<Snapshot, NativeError> {
    safe_root(lease)?;
    let mut file = match open_name(lease, name, 0, 0) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Snapshot {document:empty(lease)?,stamp:None}),
        Err(_) => return Err(unavailable()),
    };
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !owned(&metadata) || metadata.len() as usize > LIMIT { return Err(unavailable()); }
    let stamp = Stamp::from(&metadata);
    let mut bytes = Vec::new();
    (&mut file).take((LIMIT + 1) as u64).read_to_end(&mut bytes).map_err(|_| unavailable())?;
    if bytes.len() > LIMIT || !protocol::unique_keys(&bytes) || !protocol::integer_numbers(&bytes)
        || Stamp::from(&file.metadata().map_err(|_| unavailable())?) != stamp { return Err(unavailable()); }
    let named = open_name(lease, name, 0, 0).map_err(|_| unavailable())?;
    if !owned(&named.metadata().map_err(|_| unavailable())?) || Stamp::from(&named.metadata().map_err(|_| unavailable())?) != stamp {
        return Err(unavailable());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
    let document = json::parse(text).map_err(|_| unavailable())?;
    admit(&document, lease)?; safe_root(lease)?;
    Ok(Snapshot { document, stamp:Some(stamp) })
}
fn entry(document: &JsonValue, profile: &str, mod_id: &str) -> JsonValue {
    document["entries"].members().find(|r| r["profileId"] == profile && r["modId"] == mod_id)
        .map(|r| object!{targetDigest:r["targetDigest"].clone(),fileId:r["fileId"].clone(),candidateFingerprint:r["candidateFingerprint"].clone()})
        .unwrap_or(JsonValue::Null)
}
/// The caller holds both the remote lease and RootLock, in that order.
pub(crate) fn observe(lease: &RemoteStoreLease, lock: &RootLock, profile: &str, mod_id: &str, digest: &str) -> JsonValue {
    if lock.root() != lease.root() || lock.validate().is_err() { return object!{state:"unavailable",entry:JsonValue::Null}; }
    match load(lease) {
        Ok(snapshot) => { let value = entry(&snapshot.document, profile, mod_id);
            object!{state:if value.is_null(){"none"}else if value["targetDigest"]==digest{"present"}else{"stale"},entry:value} },
        Err(_) => object!{state:"unavailable",entry:JsonValue::Null},
    }
}
pub(crate) fn check_update(lease:&RemoteStoreLease,lock:&RootLock,profile:&str,mod_id:&str,digest:&str,
    file_id:&str,fingerprint:&str)->Result<(),NativeError>{
    let skip=observe(lease,lock,profile,mod_id,digest);
    if skip["state"]=="unavailable"{return Err(NativeError::new("eligibilityUnknown","The saved skip preference is unavailable; choose Force or observe it again."));}
    if skip["state"]=="present"&&skip["entry"]["fileId"]==file_id&&skip["entry"]["candidateFingerprint"]==fingerprint{
        return Err(NativeError::new("needsSelection","This candidate is skipped; clear the preference or choose Force."));
    }Ok(())
}
/// A single attempt's full intended document is kept only while its exact
/// request remains pending. It permits authoritative readback after an error.
pub(crate) struct PendingWrite { intended: JsonValue, displaced:Option<(String,(u64,u64))>, poisoned:bool }
pub(crate) enum WriteResult { Done(JsonValue), Uncertain(PendingWrite) }

fn retire_displaced(lease:&RemoteStoreLease,name:&str,identity:(u64,u64))->Result<(),NativeError>{
    safe_root(lease)?;let file=open_name(lease,name,0,0).map_err(|_|unavailable())?;
    let held=file.metadata().map_err(|_|unavailable())?;let named=open_name(lease,name,0,0).map_err(|_|unavailable())?.metadata().map_err(|_|unavailable())?;
    if !owned(&held)||!owned(&named)||(held.dev(),held.ino())!=identity||(named.dev(),named.ino())!=identity{return Err(unavailable());}
    let name=CString::new(name).map_err(|_|unavailable())?;
    if unsafe{unlinkat(lease.directory().as_raw_fd(),name.as_ptr(),0)}!=0{return Err(unavailable());}Ok(())
}

pub(crate) fn reconcile(lease: &RemoteStoreLease, lock: &RootLock, pending: &mut PendingWrite,
    profile: &str, mod_id: &str) -> Result<Option<JsonValue>, NativeError>
{
    if lock.root() != lease.root() { return Err(unavailable()); }
    if pending.poisoned{return Err(unavailable());}
    lock.validate().map_err(|_| unavailable())?;
    let current = load(lease)?;
    if current.document != pending.intended { return Ok(None); }
    if let Some((name,identity))=pending.displaced.clone(){retire_displaced(lease,&name,identity)?;pending.displaced=None;}
    lease.directory().sync_all().map_err(|_| unavailable())?;
    let checked = load(lease)?;
    if checked.document != pending.intended { return Ok(None); }
    Ok(Some(entry(&checked.document, profile, mod_id)))
}

/// Compare-and-set after exact current target and same-session fresh candidate
/// admission. On uncertainty the caller retains the PendingWrite and request.
pub(crate) fn compare_set(lease: &RemoteStoreLease, lock: &RootLock, p: &JsonValue,
    session: &SessionId, fresh: Option<&FreshAuthorization>) -> Result<WriteResult, NativeError>
{
    if lock.root() != lease.root() { return Err(unavailable()); }
    lock.validate().map_err(|_| unavailable())?; safe_root(lease)?;
    let profile = p["profileId"].as_str().ok_or_else(NativeError::params)?;
    let mod_id = p["modId"].as_str().ok_or_else(NativeError::params)?;
    let revision = common::u64_text(&p["expectedRevision"])?;
    let target = replacement_target::read(lease.root(), profile, mod_id, revision)?;
    if p["targetDigest"] != target.digest { return Err(conflict()); }
    let desired = &p["desiredSkip"];
    if !desired.is_null() {
        let auth = fresh.ok_or_else(conflict)?;
        if auth.allowed_hosts(session, Instant::now()).is_none() || target.submission.as_ref() != Some(auth.submission())
            || !auth.candidates().iter().any(|row| row["fileKey"]["fileId"] == desired["fileId"] && row["candidateFingerprint"] == desired["candidateFingerprint"])
        { return Err(conflict()); }
    } else if fresh.is_some() { return Err(NativeError::params()); }
    let before = load(lease)?;
    if entry(&before.document, profile, mod_id) != p["expectedSkip"] { return Err(conflict()); }
    let desired_entry = if desired.is_null() { JsonValue::Null } else {
        object!{targetDigest:target.digest,fileId:desired["fileId"].clone(),candidateFingerprint:desired["candidateFingerprint"].clone()}
    };
    let mut rows = before.document["entries"].members().filter(|r| !(r["profileId"] == profile && r["modId"] == mod_id)).cloned().collect::<Vec<_>>();
    if !desired_entry.is_null() {
        if rows.len() >= ENTRIES { return Err(full()); }
        rows.push(object!{profileId:profile,modId:mod_id,targetDigest:desired_entry["targetDigest"].clone(),fileId:desired_entry["fileId"].clone(),candidateFingerprint:desired_entry["candidateFingerprint"].clone()});
    }
    rows.sort_by(|a,b| (a["profileId"].as_str().unwrap(),a["modId"].as_str().unwrap()).cmp(&(b["profileId"].as_str().unwrap(),b["modId"].as_str().unwrap())));
    let mut document = empty(lease)?; document["entries"] = JsonValue::from(rows);
    admit(&document, lease)?;
    let bytes = store::canonical(&document).into_bytes(); if bytes.len() > LIMIT { return Err(full()); }
    let tmp = format!(".remote-skips-v1-{}", paths::random_id()?);
    // O_WRONLY|O_CREAT|O_EXCL: no pre-existing or unknown temporary is opened.
    let mut file = open_name(lease, &tmp, 0x1 | 0x40 | 0x80, 0o600).map_err(|_| unavailable())?;
    let mut displaced=None;let mut poisoned=false;
    let written = (|| -> Result<(), NativeError> {
        file.write_all(&bytes).map_err(|_| unavailable())?; file.sync_all().map_err(|_| unavailable())?;
        let held = file.metadata().map_err(|_| unavailable())?;
        if !owned(&held) || held.len() != bytes.len() as u64 { return Err(unavailable()); }
        lock.validate().map_err(|_| unavailable())?; safe_root(lease)?;
        if load(lease)?.stamp != before.stamp { return Err(conflict()); }
        if !desired.is_null() && fresh.is_none_or(|a| a.allowed_hosts(session, Instant::now()).is_none()) { return Err(conflict()); }
        let old = CString::new(tmp.as_str()).map_err(|_| unavailable())?;
        let new = CString::new(NAME).map_err(|_| unavailable())?;
        // NOREPLACE protects a missing name. EXCHANGE keeps the prior inode at
        // our private temporary name until it has been proved to be the exact
        // admitted file; an unexpected displaced inode is never discarded.
        let flags = if before.stamp.is_none() { 1 } else { 2 };
        if unsafe { renameat2(lease.directory().as_raw_fd(),old.as_ptr(),lease.directory().as_raw_fd(),new.as_ptr(),flags) } != 0 { return Err(unavailable()); }
        if let Some(prior)=&before.stamp{
            let old=load_named(lease,&tmp);
            let proved=old.as_ref().is_ok_and(|s|s.stamp.as_ref().is_some_and(|stamp|(stamp.device,stamp.inode)==(prior.device,prior.inode))&&s.document==before.document);
            if !proved{poisoned=true;return Err(unavailable());}
            displaced=Some((tmp.clone(),(prior.device,prior.inode)));
            retire_displaced(lease,&tmp,(prior.device,prior.inode))?;displaced=None;
        }
        lease.directory().sync_all().map_err(|_| unavailable())?;
        let after = load(lease)?; if after.document != document { return Err(unavailable()); }
        Ok(())
    })();
    match written {
        Ok(()) => Ok(WriteResult::Done(desired_entry)),
        Err(e) if e.kind == "staleReview" => Err(e),
        Err(_) => Ok(WriteResult::Uncertain(PendingWrite { intended: document,displaced,poisoned })),
    }
}

#[cfg(test)]
#[test]
fn item93_legacy_skip_preferences_survive_device_change() {
    let fixture=crate::test_support::Fixture::new();
    std::fs::create_dir(&fixture.data).unwrap();
    if !paths::test_volatile_filesystem(&File::open(&fixture.data).unwrap()) { return; }
    let lease=RemoteStoreLease::acquire(&fixture.data).unwrap();
    let mut document=empty(&lease).unwrap();
    document["rootIdentity"].remove("fsid");
    document["rootIdentity"]["device"]=(lease.identity().0+1).to_string().into();
    assert!(admit(&document,&lease).is_ok(),"an unchanged root inode remains valid on btrfs");
}
