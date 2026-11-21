//! One root-scoped, nonblocking owner for local and remote preparation work.
use crate::{native::NativeError, paths};
use std::{fs::{self, File, OpenOptions}, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}, sync::Arc};

unsafe extern "C" { fn flock(fd: i32, operation: i32) -> i32; fn geteuid() -> u32; }

pub(crate) struct PreparationSlot;

#[derive(Clone, Debug)]
pub(crate) struct PreparationSlotLease {
    inner: Arc<SlotInner>,
}
#[derive(Debug)]
struct SlotInner {
    root: PathBuf,
    directory: File,
    lock: File,
    root_identity: (u64, u64),
    lock_identity: (u64, u64),
}

fn identity(m: &fs::Metadata) -> (u64, u64) { (m.dev(), m.ino()) }
fn busy() -> NativeError { NativeError::new("nativeBusy", "Another preparation or its cleanup is active for this library.") }
fn safe_file(m: &fs::Metadata) -> bool {
    m.is_file() && m.uid() == unsafe { geteuid() } && m.nlink() == 1 && m.mode() & 0o077 == 0
}

impl PreparationSlot {
    pub(crate) fn try_acquire(root: &Path) -> Result<PreparationSlotLease, NativeError> {
        Self::acquire_mode(root,true)
    }
    pub(crate) fn acquire_existing(root: &Path) -> Result<PreparationSlotLease, NativeError> {
        Self::acquire_mode(root,false)
    }
    fn acquire_mode(root: &Path,create:bool) -> Result<PreparationSlotLease, NativeError> {
        let root = paths::absolute(root)?;
        paths::reject_symlinks(&root)?;
        let directory = OpenOptions::new().read(true).custom_flags(0x10000 | 0x20000).open(&root)?;
        let root_meta = directory.metadata()?;
        if !root_meta.is_dir() || root_meta.uid() != unsafe { geteuid() } { return Err(busy()); }
        let lock = OpenOptions::new().read(true).write(true).create(create).truncate(false)
            .mode(0o600).custom_flags(0x20000 | 0x800).open(root.join(".preparation.lock"))?;
        let meta = lock.metadata()?;
        if !safe_file(&meta) { return Err(busy()); }
        // A child spawned by another thread keeps a copy of this descriptor
        // until its exec closes it, so a just-released lock can look held for
        // a moment. Retry briefly before reporting a real owner as busy.
        let mut attempts = 0;
        while unsafe { flock(lock.as_raw_fd(), 2 | 4) } != 0 {
            attempts += 1;
            if attempts >= 40 { return Err(busy()); }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let lease = PreparationSlotLease { inner: Arc::new(SlotInner { root, directory, lock, root_identity: identity(&root_meta), lock_identity: identity(&meta) }) };
        lease.validate()?;
        Ok(lease)
    }
}

impl PreparationSlotLease {
    pub(crate) fn root(&self) -> &Path { &self.inner.root }
    pub(crate) fn identity(&self) -> (u64, u64) { self.inner.root_identity }
    pub(crate) fn validate(&self) -> Result<(), NativeError> {
        paths::reject_symlinks(&self.inner.root).map_err(|_| busy())?;
        let held_root = self.inner.directory.metadata().map_err(|_| busy())?;
        let named_root = fs::symlink_metadata(&self.inner.root).map_err(|_| busy())?;
        let held_lock = self.inner.lock.metadata().map_err(|_| busy())?;
        let named_lock = fs::symlink_metadata(self.inner.root.join(".preparation.lock")).map_err(|_| busy())?;
        if !held_root.is_dir() || !named_root.is_dir() || identity(&held_root) != self.inner.root_identity
            || identity(&named_root) != self.inner.root_identity || !safe_file(&held_lock) || !safe_file(&named_lock)
            || identity(&held_lock) != self.inner.lock_identity || identity(&named_lock) != self.inner.lock_identity { return Err(busy()); }
        Ok(())
    }
}
