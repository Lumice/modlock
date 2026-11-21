//! Native-owned paths and bounded, nonsymlink source access (Linux).
use crate::native::NativeError;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}},
    path::{Component, Path, PathBuf},
};
pub const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

fn filesystem_id(file: &File) -> std::io::Result<(u64, String)> {
    let mut result = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), result.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let result = unsafe { result.assume_init() };
    let bytes = unsafe {
        std::slice::from_raw_parts((&result.f_fsid as *const libc::fsid_t).cast::<u8>(), std::mem::size_of::<libc::fsid_t>())
    };
    if bytes.iter().all(|v| *v == 0) { return Err(std::io::Error::other("filesystem identity unavailable")); }
    Ok((result.f_type as u64, bytes.iter().map(|b| format!("{b:02x}")).collect()))
}

pub(crate) fn persisted_filesystem_id(file: &File) -> Result<String, NativeError> {
    Ok(filesystem_id(file)?.1)
}

#[cfg(test)]
pub(crate) fn test_volatile_filesystem(file: &File) -> bool {
    filesystem_id(file).is_ok_and(|(kind, _)| matches!(kind, 0x9123_683e | 0x794c_7630))
}

fn identity_values_match(device: u64, inode: u64, fsid: Option<&str>, current_device: u64,
    current_inode: u64, kind: u64, current_fsid: &str, root_fsid: &str) -> bool {
    if inode != current_inode { return false; }
    if let Some(expected) = fsid { return expected == current_fsid; }
    device == current_device || matches!(kind, 0x9123_683e | 0x794c_7630) && current_fsid == root_fsid
}

// Persist fsid with inode for cross-boot identity; legacy device mismatches are
// accepted only for btrfs/overlay objects inside a currently locked root, with
// matching inode and filesystem, while all other evidence remains mandatory.
pub(crate) fn same_persisted_object(root: &Path, root_handle: &File, path: &Path, file: &File,
    device: u64, inode: u64, fsid: Option<&str>) -> Result<bool, NativeError> {
    let root = absolute(root)?;
    let path = absolute(path)?;
    let inside_root=path.starts_with(&root);
    reject_symlinks(&root)?;
    reject_symlinks(&path)?;
    let root_meta = root_handle.metadata()?;
    let named_root = fs::symlink_metadata(&root)?;
    let meta = file.metadata()?;
    let named = fs::symlink_metadata(&path)?;
    if (root_meta.dev(), root_meta.ino()) != (named_root.dev(), named_root.ino())
        || (meta.dev(), meta.ino()) != (named.dev(), named.ino()) || meta.ino() != inode { return Ok(false); }
    if fsid.is_none() && meta.dev() == device { return Ok(true); }
    if fsid.is_none() && !inside_root { return Ok(false); }
    let (kind, current) = filesystem_id(file)?;
    let (_, root_fsid) = filesystem_id(root_handle)?;
    Ok(identity_values_match(device, inode, fsid, meta.dev(), meta.ino(), kind, &current, &root_fsid))
}

/// File mutations consume already-admitted ownership. Keep its legacy device
/// provenance separate from the current inode used for the final race check.
#[derive(Clone)]
pub(crate) struct DestructiveFile {
    pub path: PathBuf,
    device: u64,
    inode: u64,
    legacy_drift: bool,
    recorded_hash: Option<String>,
}
impl DestructiveFile {
    pub(crate) fn admitted(path:PathBuf, admitted:&json::JsonValue, recorded_hash:Option<String>)->Result<Self,NativeError>{
        let m=open_regular(&path)?.metadata()?;
        Ok(Self{path,device:m.dev(),inode:m.ino(),legacy_drift:!admitted.has_key("fsid")
            && admitted["device"].as_u64().is_some_and(|d|d!=m.dev()),recorded_hash})
    }
    fn allowed(&self)->Result<bool,NativeError>{
        let mut file=open_regular(&self.path)?;let m=file.metadata()?;
        if (m.dev(),m.ino())!=(self.device,self.inode){return Err(Self::unproven());}
        if !self.legacy_drift {return Ok(true);}
        let matches=if let Some(expected)=&self.recorded_hash {
            // Always read full bytes here; a preflight or memoized digest is
            // not authority to destroy a legacy device-reconciled file.
            let stamp=content_stamp(&file)?;
            use sha2::{Digest,Sha256};let mut hash=Sha256::new();let mut buffer=[0u8;65536];
            loop {let n=file.read(&mut buffer)?;if n==0{break;}hash.update(&buffer[..n]);}
            let named=fs::symlink_metadata(&self.path)?;
            if content_stamp(&file)?!=stamp || (named.dev(),named.ino())!=(self.device,self.inode){return Err(Self::unproven());}
            format!("{:x}",hash.finalize())==*expected
        }else{false};
        if !matches {eprintln!("Recovery left behind {}: older filesystem identity has no matching recorded content proof.",self.path.display());}
        Ok(matches)
    }
    fn unproven()->NativeError{NativeError::new("recoveryRequired","An older file could not be verified and was left in place. It cannot be deleted or replaced safely; preserve it before retrying.")}
    pub(crate) fn unlink(&self)->Result<bool,NativeError>{
        if !self.allowed()? {return Ok(false);}
        fs::remove_file(&self.path)?;Ok(true)
    }
    pub(crate) fn unlink_required(&self)->Result<(),NativeError>{
        if self.unlink()? {Ok(())}else{Err(Self::unproven())}
    }
}
/// A required replacement cannot proceed by discarding an unproven old file.
pub(crate) fn replace_admitted(source:&Path,destination:&Path,admitted:&json::JsonValue,recorded_hash:Option<String>)->Result<(),NativeError>{
    if fs::symlink_metadata(destination).is_ok(){
        if !DestructiveFile::admitted(destination.to_owned(),admitted,recorded_hash)?.allowed()? {return Err(DestructiveFile::unproven());}
    }else{verify_absent(destination)?;}
    fs::rename(source,destination)?;Ok(())
}

#[cfg(test)]
#[test]
fn item93_shared_legacy_destruction_checks_full_bytes_without_memo(){
    for replace in [false,true]{for drift in [false,true]{for proof in ["missing","wrong","matching"]{
        let f=crate::test_support::Fixture::new();let path=f.root.join("old-file");let source=f.root.join("replacement");
        fs::write(&path,b"saved file bytes").unwrap();fs::set_permissions(&path,fs::Permissions::from_mode(0o400)).unwrap();fs::write(&source,b"replacement bytes").unwrap();
        let m=fs::metadata(&path).unwrap();let id=json::object!{device:m.dev()+u64::from(drift),inode:m.ino()};
        let hash=match proof{"missing"=>None,"wrong"=>Some(digest_bytes(b"different recorded bytes").unwrap()),_=>Some(digest_bytes(b"saved file bytes").unwrap())};
        let _scope=HashMemo::default().enter();
        if let Some(sha)=&hash{let stamp=content_stamp(&open_regular(&path).unwrap()).unwrap();seed_hash(&path,&stamp,sha).unwrap();}
        let allowed=!drift || proof=="matching";
        if replace {
            let result=replace_admitted(&source,&path,&id,hash);
            assert_eq!(result.is_ok(),allowed);
            assert_eq!(fs::read(&path).unwrap(),if allowed{b"replacement bytes".as_slice()}else{b"saved file bytes".as_slice()});
            assert_eq!(source.exists(),!allowed);
        }else{
            let removed=DestructiveFile::admitted(path.clone(),&id,hash).unwrap().unlink().unwrap();assert_eq!(removed,allowed);
            if !allowed{assert_eq!(fs::read(&path).unwrap(),b"saved file bytes");}
        }
    }}}
}

#[cfg(test)]
#[test]
fn item93_persisted_identity_requires_inode_fsid_and_bounded_legacy_fallback() {
    let check = |device, inode, fsid, current_device, current_inode, kind, current, root| {
        identity_values_match(device, inode, fsid, current_device, current_inode, kind, current, root)
    };
    assert!(check(47, 42, Some("a"), 38, 42, 0x9123_683e, "a", "a"));
    assert!(!check(47, 42, Some("a"), 47, 42, 0x9123_683e, "b", "b"));
    assert!(!check(47, 42, Some("a"), 38, 43, 0x9123_683e, "a", "a"));
    assert!(check(47, 42, None, 38, 42, 0x9123_683e, "a", "a"));
    assert!(check(47, 42, None, 38, 42, 0x794c_7630, "a", "a"));
    assert!(!check(47, 42, None, 38, 42, 0xef53, "a", "a"));
    assert!(!check(47, 42, None, 38, 42, 0x9123_683e, "a", "b"));
}
pub fn absolute(path: &Path) -> Result<PathBuf, NativeError> {
    let p = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if p.components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(NativeError::new(
            "unsafePath",
            "Paths must not contain traversal components.",
        ));
    }
    Ok(p)
}
/// Absolute lexical spelling only; never follow links to establish identity.
pub(crate) fn lexical_absolute(path: &Path) -> Result<PathBuf, NativeError> {
    let absolute = std::path::absolute(path)?;
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            // ".." after a link means a different place than the lexical parent,
            // so a game path containing it can't be compared safely: refuse it.
            Component::ParentDir => {
                return Err(NativeError::new("unsafePath", "Paths must not contain traversal components."));
            }
            Component::CurDir => {}
            _ => result.push(part.as_os_str()),
        }
    }
    Ok(result)
}
fn bootstrap_marker(m:&json::JsonValue,root:&str)->bool {
    let hex32=|v:&json::JsonValue|v.as_str().is_some_and(|s|s.len()==32&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)));
    let p=&m["params"];
    m.is_object()&&m.len()==6&&m["bootstrapVersion"].as_f64()==Some(1.0)&&m["root"].as_str()==Some(root)
        &&hex32(&m["operationId"])&&m["method"].as_f64()==Some(52.0)&&m["expectedRevision"].as_str()==Some("0")
        &&p.is_object()&&p.len()==3&&p["version"].as_f64()==Some(1.0)&&p["operationId"]==m["operationId"]&&p["expectedRevision"]==m["expectedRevision"]
}
/// LibraryModel admits a bootstrap marker only as Qt's compact serialization
/// (sorted keys, no whitespace, no duplicates). With every field already
/// fixed by `bootstrap_marker`, that byte form is fully determined.
fn bootstrap_marker_bytes(text:&str,root:&str)->bool {
    let Ok(m)=json::parse(text) else {return false};
    if !bootstrap_marker(&m,root) {return false;}
    let id=m["operationId"].as_str().unwrap_or_default();
    let canonical=format!(r#"{{"bootstrapVersion":1,"expectedRevision":"0","method":52,"operationId":"{id}","params":{{"expectedRevision":"0","operationId":"{id}","version":1}},"root":{}}}"#,json::stringify(root));
    text==canonical
}
pub fn data_root(params: &json::JsonValue) -> Result<PathBuf, NativeError> {
    if params.has_key("dataDir") {
        return absolute(Path::new(
            params["dataDir"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(NativeError::params)?,
        ));
    }
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(
            std::env::var_os("HOME")
                .ok_or_else(|| NativeError::new("nativeIo", "HOME is not configured."))?,
        )
        .join(".local/share"),
    };
    let modern=base.join("modlock");
    let legacy=base.join("deadlock-native");
    let has_library=|root:&Path| -> Result<bool,NativeError> {
        match std::fs::symlink_metadata(root.join("library.sqlite3")) {
            Ok(m) if m.is_file() => return Ok(true),
            Ok(_) => return Err(NativeError::new("unsafePath",format!("The library in {} is not a regular file.",root.display()))),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
        // Unpublished creation/recovery is still this root's library. Reuse
        // bootstrap's sidecars and LibraryModel::markerPath's existing key.
        for side in crate::library_bootstrap::SIDES {
            match std::fs::symlink_metadata(root.join(side)) {
                Ok(_) => return Ok(true),
                Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(e.into()),
            }
        }
        let config=std::env::var_os("XDG_CONFIG_HOME").filter(|s|!s.is_empty()).map(PathBuf::from)
            .or_else(||std::env::var_os("HOME").map(|home|PathBuf::from(home).join(".config")));
        if let Some(config)=config {
            let config=std::fs::canonicalize(&config).unwrap_or(config); // Same config-home resolution as AppPaths.
            let key=digest_bytes(lexical_absolute(root)?.to_string_lossy().as_bytes())?;
            let marker=config.join("dl-mod-manager/state/pending").join(format!("{key}.json"));
            match std::fs::symlink_metadata(&marker) {
                Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
                // An unreadable state folder must not silently pick the other root.
                Err(e) => return Err(NativeError::new(if e.kind()==std::io::ErrorKind::PermissionDenied {"settingsPermissionDenied"} else {"nativeIo"},format!("Can't check for an unfinished library setup in {}: {e}",marker.parent().unwrap_or(&marker).display()))),
                Ok(m) if !m.is_file() => {}, // Not a marker the app writes.
                Ok(_) => {
                    // No-follow, nonblocking open with a descriptor type check, so a
                    // concurrent swap can't redirect the read or hang on a FIFO.
                    let mut bytes=Vec::new();
                    let read=OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(&marker).and_then(|f|{
                        use std::io::Read;
                        if !f.metadata()?.is_file() {return Ok(false);}
                        f.take(65537).read_to_end(&mut bytes)?;Ok(true)
                    });
                    match read {
                        Ok(true) => {},
                        Ok(false) => return Ok(false),
                        Err(e) if e.raw_os_error()==Some(40) => return Ok(false), // ELOOP: swapped for a symlink.
                        Err(e) => return Err(NativeError::new(if e.kind()==std::io::ErrorKind::PermissionDenied {"settingsPermissionDenied"} else {"nativeIo"},format!("Can't read {}: {e}",marker.display()))),
                    }
                    // Only a valid bootstrap marker for this exact root is
                    // evidence of an unpublished library (same rule as
                    // LibraryModel's bootstrapMarker). Ordinary operation,
                    // cleared, foreign or malformed markers stay with the UI.
                    let root_text=lexical_absolute(root)?.to_string_lossy().into_owned();
                    if bytes.len()<=65536 && String::from_utf8(bytes).ok().is_some_and(|text|bootstrap_marker_bytes(&text,&root_text)) {return Ok(true);}
                }
            }
        }
        Ok(false)
    };
    match (has_library(&modern)?,has_library(&legacy)?) {
        (true,true)=>Err(NativeError::new("ambiguousLibrary",format!("Libraries exist in both {} and {}. Choose which library to open with --data-dir.",modern.display(),legacy.display()))),
        (false,true)=>absolute(&legacy),
        _=>absolute(&modern),
    }
}
pub fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\', '\0'])
        && !value.chars().any(char::is_control)
}
pub fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
/// Every existing component must be an ordinary directory/file, never a symlink.
pub fn reject_symlinks(path: &Path) -> Result<(), NativeError> {
    let path = absolute(path)?;
    let mut p = PathBuf::new();
    for c in path.components() {
        p.push(c);
        let m = fs::symlink_metadata(&p)?;
        if m.file_type().is_symlink() {
            return Err(NativeError::new(
                "unsafePath",
                "A source or native path contains a symlink; choose an ordinary owned directory.",
            ));
        }
    }
    Ok(())
}
pub fn open_regular(path: &Path) -> Result<File, NativeError> {
    reject_symlinks(path)?;
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800)
        .open(path)?;
    if !f.metadata()?.is_file() {
        return Err(NativeError::new(
            "unsafePath",
            "Expected a regular nonsymlink file.",
        ));
    }
    Ok(f)
}
/// Verify absence without following a dangling symlink or an unsafe ancestor.
pub fn verify_absent(path: &Path) -> Result<(), NativeError> {
    let path = absolute(path)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(NativeError::new(
                "externalChange",
                "A previously missing source package now exists.",
            ))
        }
        Err(e) => return Err(e.into()),
    }
    let mut ancestor = path.parent();
    while let Some(parent) = ancestor {
        match fs::symlink_metadata(parent) {
            Ok(m) => {
                reject_symlinks(parent)?;
                if !m.is_dir() {
                    return Err(NativeError::new(
                        "unsafePath",
                        "A missing package has a non-directory ancestor.",
                    ));
                }
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ancestor = parent.parent(),
            Err(e) => return Err(e.into()),
        }
    }
    Err(NativeError::params())
}
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, NativeError> {
    let f = open_regular(path)?;
    if f.metadata()?.len() > limit {
        return Err(NativeError::new(
            "sourceTooLarge",
            "Input exceeds the supported size limit.",
        ));
    }
    let mut b = Vec::new();
    f.take(limit + 1).read_to_end(&mut b)?;
    if b.len() as u64 > limit {
        return Err(NativeError::new(
            "sourceTooLarge",
            "Input exceeds the supported size limit.",
        ));
    }
    Ok(b)
}
pub fn private_dir(path: &Path) -> Result<(), NativeError> {
    private_dir_with_sync(&absolute(path)?, &mut sync_dir)
}
fn private_dir_with_sync(
    path: &Path,
    sync_parent: &mut impl FnMut(&Path) -> Result<(), NativeError>,
) -> Result<(), NativeError> {
    let exists = path.exists();
    if exists {
        reject_symlinks(path)?;
        if !path.is_dir() {
            return Err(NativeError::new(
                "nativeIo",
                "Native directory path is occupied by a file.",
            ));
        }
    }
    // The filesystem root has no parent entry. Every other existing entry may
    // have survived a failed mkdir/sync attempt, including an ancestor of path.
    let Some(parent) = path.parent() else {
        return if exists {
            Ok(())
        } else {
            Err(NativeError::params())
        };
    };
    private_dir_with_sync(parent, sync_parent)?;
    if !exists {
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        builder.create(path)?;
    }
    // Retry this sync even when path already exists. Ancestors are established
    // first, so no descendant creation can bypass an earlier failed sync.
    sync_parent(parent)
}

pub fn sync_dir(path: &Path) -> Result<(), NativeError> {
    File::open(path)?.sync_all()?;
    Ok(())
}
pub fn random_id() -> Result<String, NativeError> {
    let mut b = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub sha256: String,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanos: i64,
    pub device: u64,
    pub inode: u64,
}
fn identity(m: &fs::Metadata) -> (u64, i64, i64, u64, u64) {
    (m.len(), m.mtime(), m.mtime_nsec(), m.dev(), m.ino())
}
#[cfg(test)]
thread_local! {
    pub(crate) static HASH_READS: std::cell::RefCell<std::collections::HashMap<PathBuf, usize>> = std::cell::RefCell::new(std::collections::HashMap::new());
}
/// Opt-in diagnostics for qualification; emitted only to helper stderr.
pub(crate) struct ApplyPhase(Option<(&'static str,std::time::Instant,u64)>);
fn apply_rchar()->u64 {fs::read_to_string("/proc/self/io").ok().and_then(|s|s.lines().find_map(|l|l.strip_prefix("rchar: ").and_then(|v|v.parse().ok()))).unwrap_or(0)}
impl ApplyPhase {
    pub(crate) fn new(name:&'static str)->Self {Self(std::env::var_os("DL_APPLY_TRACE").map(|_|(name,std::time::Instant::now(),apply_rchar())))}
}
impl Drop for ApplyPhase {fn drop(&mut self){if let Some((name,start,bytes))=&self.0{eprintln!("APPLY_PHASE {} ms={:.3} rchar={}",name,start.elapsed().as_secs_f64()*1000.0,apply_rchar().saturating_sub(*bytes));}}}
// Content proofs live only within a reviewed Apply. Every hit still opens the
// nonsymlink file and checks filesystem, inode, size, mtime and ctime.
#[derive(Clone, Default)]
pub(crate) struct HashMemo(std::collections::HashMap<PathBuf, (String, Fingerprint)>);
thread_local! { static APPLY_HASHES: std::cell::RefCell<Option<HashMemo>> = const { std::cell::RefCell::new(None) }; }
pub(crate) struct HashScope(Option<HashMemo>);
impl HashMemo {
    pub(crate) fn enter(&self) -> HashScope {
        HashScope(APPLY_HASHES.with(|v|v.replace(Some(self.clone()))))
    }
    pub(crate) fn capture() -> Self { APPLY_HASHES.with(|v|v.borrow().clone().unwrap_or_default()) }
    pub(crate) fn verified(path:&Path, sha:&str, size:u64)->Result<bool,NativeError>{
        let file=open_regular(path)?;let stamp=content_stamp(&file)?;
        Ok(APPLY_HASHES.with(|v|v.borrow().as_ref().is_some_and(|m|m.0.values().any(|(s,f)|*s==stamp && f.sha256==sha && f.size==size))))
    }
    pub(crate) fn recheck_if_known(path:&Path)->Result<(),NativeError>{
        APPLY_HASHES.with(|v|match v.borrow().as_ref(){
            Some(memo) if memo.0.contains_key(path)=>memo.recheck(path),
            _=>Ok(()),
        })
    }
    pub(crate) fn recheck(&self, path: &Path) -> Result<(), NativeError> {
        let file=open_regular(path)?;
        if self.0.get(path).is_none_or(|(stamp,_)| !content_stamp(&file).is_ok_and(|s|s==*stamp)) {
            return Err(NativeError::new("stalePreview", "A package file changed since review. Review again."));
        }
        Ok(())
    }
}
impl Drop for HashScope { fn drop(&mut self) { APPLY_HASHES.with(|v|{v.replace(self.0.take());}); } }
pub(crate) fn content_stamp(file: &File) -> Result<String, NativeError> {
    let m=file.metadata()?;
    Ok(format!("{}:{}:{}:{}:{}:{}:{}:{}:{}",persisted_filesystem_id(file)?,m.ino(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec(),m.mode(),m.dev()))
}
pub(crate) fn seed_hash(path: &Path, stamp: &str, sha: &str) -> Result<(), NativeError> {
    let file=open_regular(path)?;let m=file.metadata()?;
    if m.mode() & 0o777 != 0o400 || content_stamp(&file)?!=stamp {return Ok(());}
    let fp=Fingerprint{sha256:sha.into(),size:m.len(),modified_seconds:m.mtime(),modified_nanos:m.mtime_nsec(),device:m.dev(),inode:m.ino()};
    APPLY_HASHES.with(|v|if let Some(memo)=v.borrow_mut().as_mut(){memo.0.insert(path.to_owned(),(stamp.into(),fp));});
    Ok(())
}
// Our own link/unlink changes ctime without changing bytes. Refresh only
// already verified aliases whose remaining content identity still matches.
pub(crate) fn acknowledge_link_change(device:u64,inode:u64)->Result<(),NativeError>{
    APPLY_HASHES.with(|v|->Result<(),NativeError>{
        if let Some(memo)=v.borrow_mut().as_mut(){
            for (path,(stamp,fp)) in memo.0.iter_mut().filter(|(_,(_,f))|f.device==device && f.inode==inode){
                if !path.try_exists()?{continue;}
                let file=open_regular(path)?;let m=file.metadata()?;
                if (m.dev(),m.ino(),m.len(),m.mtime(),m.mtime_nsec())!=(fp.device,fp.inode,fp.size,fp.modified_seconds,fp.modified_nanos){return Err(NativeError::new("stalePreview","A linked payload changed."));}
                *stamp=content_stamp(&file)?;
            }
        }
        Ok(())
    })
}
pub fn fingerprint(path: &Path) -> Result<Fingerprint, NativeError> {
    let mut file = open_regular(path)?;
    fingerprint_file(&mut file)
}
pub fn fingerprint_file(file: &mut File) -> Result<Fingerprint, NativeError> {
    let path=fs::read_link(format!("/proc/self/fd/{}",file.as_raw_fd()))?;
    let stamp=content_stamp(file)?;
    if let Some(fp)=APPLY_HASHES.with(|v|v.borrow().as_ref().and_then(|m|m.0.values().find(|(s,_)|*s==stamp).map(|(_,f)|f.clone()))) {
        APPLY_HASHES.with(|v|if let Some(m)=v.borrow_mut().as_mut(){m.0.insert(path,(stamp,fp.clone()));});
        return Ok(fp);
    }
    #[cfg(test)]
    {
        use std::os::fd::AsRawFd;
        if let Ok(path) = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
            HASH_READS.with(|reads| *reads.borrow_mut().entry(path).or_default() += 1);
        }
    }
    let before = file.metadata()?;
    file.seek(SeekFrom::Start(0))?;
    use sha2::{Digest,Sha256};
    let mut hash=Sha256::new();let mut buffer=[0u8;65536];
    loop {let n=std::io::Read::read(file,&mut buffer)?;if n==0{break;}hash.update(&buffer[..n]);}
    let sha=format!("{:x}",hash.finalize());
    if identity(&before) != identity(&file.metadata()?) {
        return Err(NativeError::new(
            "stalePreview",
            "A source file changed while being read; review again.",
        ));
    }
    let fp=Fingerprint {
        sha256: sha,
        size: before.len(),
        modified_seconds: before.mtime(),
        modified_nanos: before.mtime_nsec(),
        device: before.dev(),
        inode: before.ino(),
    };
    if content_stamp(file)?!=stamp {return Err(NativeError::new("stalePreview","A source file changed while being read; review again."));}
    APPLY_HASHES.with(|v|if let Some(m)=v.borrow_mut().as_mut(){m.0.insert(path,(stamp,fp.clone()));});
    Ok(fp)
}
/// Hash bounded in-memory metadata without writing a preview artifact.
pub fn digest_bytes(bytes: &[u8]) -> Result<String, NativeError> {
    use sha2::{Digest, Sha256};

    Ok(format!("{:x}", Sha256::digest(bytes)))
}
pub fn make_readonly(path: &Path) -> Result<(), NativeError> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "deadlock-native-path-sync-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn item115_small_hashmemo_rpc_message_has_no_path(){
        let temp=Temporary::new();let path=temp.0.join("private-package.vpk");std::fs::write(&path,b"one").unwrap();
        let memo=HashMemo::default();let error=memo.recheck(&path).unwrap_err();assert_eq!(error.kind,"stalePreview");
        assert_eq!(error.message,"A package file changed since review. Review again.");assert!(!error.message.contains(&path.to_string_lossy().to_string()));assert!(!error.message.contains('/'));
    }
    #[test]
    fn creating_nested_directories_syncs_each_parent_before_creating_next_child() {
        let temp = Temporary::new();
        let first = temp.0.join("ancestor");
        let second = first.join("native");
        let target = second.join("payloads");
        let expected = [
            (temp.0.clone(), first.clone()),
            (first.clone(), second.clone()),
            (second.clone(), target.clone()),
        ];
        let mut synced = Vec::new();
        private_dir_with_sync(&target, &mut |parent| {
            if !parent.starts_with(&temp.0) {
                return sync_dir(parent);
            }
            let index = synced.len();
            assert_eq!(parent, expected[index].0);
            assert!(
                expected[index].1.is_dir(),
                "created entry must exist before its parent is synced"
            );
            if let Some(next) = expected.get(index + 1) {
                assert!(
                    !next.1.exists(),
                    "next child must wait for its ancestor entry to be durable"
                );
            }
            sync_dir(parent)?;
            synced.push(parent.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(synced, vec![temp.0.clone(), first, second]);
        assert!(target.is_dir());
    }

    #[test]
    fn parent_sync_failure_is_returned_before_creating_descendant_directories() {
        let temp = Temporary::new();
        let first = temp.0.join("ancestor");
        let target = first.join("native");
        let error = private_dir_with_sync(&target, &mut |parent| {
            if !parent.starts_with(&temp.0) {
                return sync_dir(parent);
            }
            assert_eq!(parent, temp.0);
            assert!(first.is_dir());
            Err(NativeError::new("nativeIo", "test parent sync failure"))
        })
        .unwrap_err();
        assert_eq!(error.kind, "nativeIo");
        assert!(!target.exists());
    }

    #[test]
    fn direct_root_retry_rechecks_failed_parent_sync_until_it_succeeds() {
        let temp = Temporary::new();
        let target = temp.0.join("native");
        let mut failed_sync_calls = 0;
        for _ in 0..2 {
            let error = private_dir_with_sync(&target, &mut |parent| {
                if parent == temp.0 {
                    failed_sync_calls += 1;
                    assert!(target.is_dir());
                    return Err(NativeError::new(
                        "nativeIo",
                        "persistent parent sync failure",
                    ));
                }
                sync_dir(parent)
            })
            .unwrap_err();
            assert_eq!(error.kind, "nativeIo");
            assert!(
                target.is_dir(),
                "failed sync leaves a directory whose entry still needs syncing"
            );
        }
        assert_eq!(failed_sync_calls, 2);
        let mut successful_sync_calls = 0;
        private_dir_with_sync(&target, &mut |parent| {
            sync_dir(parent)?;
            if parent == temp.0 {
                successful_sync_calls += 1;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            successful_sync_calls, 1,
            "successful retry must establish root-entry durability"
        );
    }

    #[test]
    fn nested_ancestor_retry_syncs_existing_ancestor_before_creating_descendants() {
        let temp = Temporary::new();
        let ancestor = temp.0.join("ancestor");
        let native = ancestor.join("native");
        let target = native.join("payloads");
        let mut failed_sync_calls = 0;
        for _ in 0..2 {
            let error = private_dir_with_sync(&target, &mut |parent| {
                if parent == temp.0 {
                    failed_sync_calls += 1;
                    assert!(ancestor.is_dir());
                    assert!(
                        !native.exists(),
                        "descendants must wait for the ancestor entry sync"
                    );
                    return Err(NativeError::new(
                        "nativeIo",
                        "persistent ancestor sync failure",
                    ));
                }
                sync_dir(parent)
            })
            .unwrap_err();
            assert_eq!(error.kind, "nativeIo");
            assert!(!native.exists());
            assert!(!target.exists());
        }
        assert_eq!(failed_sync_calls, 2);
        let expected = [
            (temp.0.clone(), ancestor.clone()),
            (ancestor.clone(), native.clone()),
            (native.clone(), target.clone()),
        ];
        let mut synced = Vec::new();
        private_dir_with_sync(&target, &mut |parent| {
            if parent.starts_with(&temp.0) {
                let index = synced.len();
                assert_eq!(parent, expected[index].0);
                assert!(expected[index].1.is_dir());
                if let Some(next) = expected.get(index + 1) {
                    assert!(!next.1.exists());
                }
                synced.push(parent.to_owned());
            }
            sync_dir(parent)
        })
        .unwrap();
        assert_eq!(synced, vec![temp.0.clone(), ancestor, native]);
        assert!(target.is_dir());
    }
}
