//! Shared game exclusion. Callers acquire the native root lock first.
use crate::{native::NativeError, paths};
use std::{
    fs::{self, File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}},
    path::{Path, PathBuf},
};
fn err(kind: &str, message: &str) -> NativeError {
    NativeError::new(kind, message)
}
pub struct GameLock {
    file: File,
    citadel: PathBuf,
}
impl GameLock {
    pub fn acquire(c: &Path) -> Result<Self, NativeError> {
        let guard = Self::acquire_directory(c)?;
        match fs::symlink_metadata(guard.citadel.join(crate::launch_record::MARKER)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(guard),
            Err(e) => Err(e.into()),
            Ok(_) => Err(crate::launch_record::barrier()),
        }
    }
    pub fn acquire_for_handoff(store: &crate::store::Store, lock: &crate::store::RootLock,
        c: &Path, claim: &crate::launch_store::HandoffClaim) -> Result<Self, NativeError> {
        store.locked(lock)?;
        let guard=Self::acquire_directory(c)?;
        claim.validate_game(store,&guard.citadel)?;
        Ok(guard)
    }
    pub fn acquire_for_settlement(store: &crate::store::Store, lock: &crate::store::RootLock,
        c: &Path, claim: &crate::launch_store::SettlementClaim) -> Result<Self, NativeError> {
        store.locked(lock)?;
        let guard=Self::acquire_directory(c)?;
        claim.validate_game(store,&guard)?;
        Ok(guard)
    }
    fn acquire_directory(c: &Path) -> Result<Self, NativeError> {
        extern "C" {
            fn flock(fd: i32, op: i32) -> i32;
        }
        let c = paths::absolute(c)?;
        paths::reject_symlinks(&c)?;
        let f = OpenOptions::new().read(true).custom_flags(0x10000 | 0x20000).open(&c)?;
        if unsafe { flock(f.as_raw_fd(), 2 | 4) } != 0 {
            return Err(err(
                "nativeBusy",
                "Another native manager is writing this game installation.",
            ));
        }
        let guard = Self { file: f, citadel: c };
        guard.validate(&guard.citadel)?;
        Ok(guard)
    }
    pub(crate) fn same_persisted_object(&self, path: &Path, id: &json::JsonValue) -> Result<bool, NativeError> {
        self.validate(&self.citadel)?;
        let path=paths::absolute(path)?;
        if !path.starts_with(&self.citadel) { return Ok(false); }
        paths::reject_symlinks(&path)?;
        let file=OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(&path)?;
        let Some(device)=id["device"].as_u64() else {return Ok(false)};
        let Some(inode)=id["inode"].as_u64() else {return Ok(false)};
        if id.has_key("fsid") && !id["fsid"].as_str().is_some_and(|s|crate::package::hex(s,16)) {return Ok(false);}
        paths::same_persisted_object(&self.citadel,&self.file,&path,&file,device,inode,id["fsid"].as_str())
    }
    /// Font footprints use the top-level game directory only as an identity
    /// anchor. No write or deletion authority is granted outside Citadel.
    pub(crate) fn font_game_anchor_matches(&self,path:&Path,id:&json::JsonValue)->Result<bool,NativeError>{
        let path=paths::absolute(path)?;
        self.validate(&path.join("game/citadel"))?;
        paths::reject_symlinks(&path)?;
        let file=OpenOptions::new().read(true).custom_flags(0x10000 | 0x20000).open(&path)?;
        let held=file.metadata()?;let named=fs::symlink_metadata(&path)?;
        let fsid=paths::persisted_filesystem_id(&file)?;
        Ok(held.is_dir() && (held.dev(),held.ino())==(named.dev(),named.ino())
            && id["inode"].as_u64()==Some(held.ino()) && crate::generation::valid_fsid(id)
            && id["device"].as_u64().is_some()
            && fsid==paths::persisted_filesystem_id(&self.file)?
            && id["fsid"].as_str().is_none_or(|expected|expected==fsid))
    }
    /// A held descriptor is authority only for this exact, still-current directory.
    pub fn validate(&self, citadel: &Path) -> Result<(), NativeError> {
        let path = paths::absolute(citadel)?;
        paths::reject_symlinks(&path)?;
        let held = self.file.metadata()?;
        let current = fs::symlink_metadata(&path)?;
        if path != self.citadel || !held.is_dir() || !current.is_dir()
            || (held.dev(), held.ino()) != (current.dev(), current.ino()) {
            return Err(err("nativeBusy", "Game lock does not cover this current game directory."));
        }
        Ok(())
    }
}
impl Drop for GameLock {
    fn drop(&mut self) {
        extern "C" {
            fn flock(fd: i32, op: i32) -> i32;
        }
        unsafe { flock(self.file.as_raw_fd(), 8) };
    }
}
#[cfg(test)]
thread_local! {pub(crate) static TEST_REFUSAL:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
pub fn ensure_game_idle() -> Result<(), NativeError> {
    #[cfg(test)] if TEST_REFUSAL.with(|v|v.get()){return Err(err("gameRunning","Close Deadlock before applying, restoring or recovering game files."));}
    #[cfg(test)] let before = test_rchar();
    let result = game_guard_at(Path::new("/proc"));
    #[cfg(test)] {
        let bytes = test_rchar() - before;
        PROCESS_SCAN_RCHAR.with(|total| total.set(total.get() + bytes));
    }
    result
}
pub(crate) fn game_guard_at(proc_root: &Path) -> Result<(), NativeError> {
    // Every preparation/Apply/Verify/publication recheck uses the same bounded,
    // complete leader observation as launch admission. No weak second scan.
    let observation = crate::launch_process::scan_at(proc_root);
    if !observation.complete {
        return Err(err("gameGuardUnavailable",
            "Cannot inspect running processes safely; close the game and check process permissions."));
    }
    if !observation.games.is_empty() {
        return Err(err(
            "gameRunning",
            "Close Deadlock before applying, restoring or recovering game files.",
        ));
    }
    Ok(())
}

#[cfg(test)]
thread_local! { pub(crate) static PROCESS_SCAN_RCHAR: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn test_rchar() -> u64 {
    fs::read_to_string("/proc/thread-self/io").unwrap().lines()
        .find_map(|line| line.strip_prefix("rchar: ").and_then(|n| n.parse().ok())).unwrap()
}

/// File-I/O regression tests must not depend on the desktop's process count.
/// Keep running the real safety scan, but exclude its reads on this test thread.
#[cfg(test)]
pub(crate) fn test_file_rchar() -> u64 {
    test_rchar() - PROCESS_SCAN_RCHAR.with(|total| total.get())
}
