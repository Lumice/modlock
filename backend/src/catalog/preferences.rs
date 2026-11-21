//! Durable root-wide preferences; independent of evictable catalog and library.
use super::{
    contract as c,
    provider::{parse_bounded, SubmissionKeyV1},
};
use crate::{native::NativeError, paths};
use json::{object, JsonValue};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
const LIMIT: u64 = 1024 * 1024;
pub struct PreferenceSnapshot {
    pub revision: u64,
    pub favorites: BTreeSet<SubmissionKeyV1>,
}
pub struct CatalogPreferences {
    root: PathBuf,
    directory: File,
}
impl CatalogPreferences {
    pub fn open(root: &Path) -> Result<Self, NativeError> {
        paths::reject_symlinks(root)?;
        let root = fs::canonicalize(root)?;
        let directory = File::open(&root)?;
        Ok(Self { root, directory })
    }
    fn lock(&self) -> Result<File, NativeError> {
        let current = fs::metadata(&self.root)?;
        let held = self.directory.metadata()?;
        if (current.dev(), current.ino()) != (held.dev(), held.ino())
            || fs::canonicalize(&self.root)? != self.root
        {
            return Err(storage());
        }
        let path = self.root.join(".catalog-preferences-v1.lock");
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(0x20000 | 0x800)
            .open(&path)?;
        let m = f.metadata()?;
        if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 {
            return Err(storage());
        }
        unsafe extern "C" {
            fn flock(fd: i32, op: i32) -> i32;
        }
        if unsafe { flock(f.as_raw_fd(), 2 | 4) } != 0 {
            return Err(NativeError::new("catalogUnavailable","Another preferences writer holds the short catalog preference lock; retry after reading the current value."));
        }
        let named = fs::symlink_metadata(path)?;
        if (named.dev(), named.ino()) != (m.dev(), m.ino()) {
            return Err(storage());
        }
        Ok(f)
    }
    fn read(&self) -> Result<PreferenceSnapshot, NativeError> {
        let path = self.root.join("catalog-preferences-v1.json");
        let bytes = match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PreferenceSnapshot {
                    revision: 0,
                    favorites: BTreeSet::new(),
                })
            }
            Err(_) => return Err(storage()),
            Ok(m) => {
                if !m.is_file() || m.nlink() != 1 || m.len() > LIMIT {
                    return Err(storage());
                }
                paths::read_bounded(&path, LIMIT).map_err(|_| storage())?
            }
        };
        if !crate::protocol::integer_numbers(&bytes) {
            return Err(storage());
        }
        let v = parse_bounded(&bytes, LIMIT as usize).map_err(|_| storage())?;
        c::exact(&v, &["version", "revision", "favorites"]).map_err(|_| storage())?;
        if v["version"] != 1 || !v["favorites"].is_array() || v["favorites"].len() > 10_000 {
            return Err(storage());
        }
        let revision = c::u64_text(&v["revision"]).map_err(|_| storage())?;
        let mut favorites = BTreeSet::new();
        let mut previous = None;
        for entry in v["favorites"].members() {
            let k = SubmissionKeyV1::parse(entry).map_err(|_| storage())?;
            if previous.as_ref().is_some_and(|p| p >= &k) {
                return Err(storage());
            }
            previous = Some(k.clone());
            favorites.insert(k);
        }
        Ok(PreferenceSnapshot {
            revision,
            favorites,
        })
    }
    pub fn snapshot(&self) -> Result<PreferenceSnapshot, NativeError> {
        let _lock = self.lock()?;
        self.read()
    }
    pub fn get(&self, key: Option<SubmissionKeyV1>) -> Result<JsonValue, NativeError> {
        let s = self.snapshot()?;
        Ok(
            object! {preferencesRevision:s.revision.to_string(),submission:key.as_ref().map(SubmissionKeyV1::json),favorite:key.as_ref().map(|k|s.favorites.contains(k))},
        )
    }
    pub fn set_favorite(
        &self,
        key: SubmissionKeyV1,
        desired: bool,
        expected_revision: u64,
    ) -> Result<JsonValue, NativeError> {
        let _lock = self.lock()?;
        let mut s = self.read()?;
        if s.revision != expected_revision {
            return Err(NativeError::new(
                "catalogStalePreferences",
                "Favorites changed; read the current value and revision before a fresh action.",
            ));
        }
        let changed = s.favorites.contains(&key) != desired;
        if changed {
            if desired {
                s.favorites.insert(key.clone());
            } else {
                s.favorites.remove(&key);
            }
            if s.favorites.len() > 10_000 {
                return Err(c::error("catalogFull"));
            }
            s.revision = s.revision.checked_add(1).ok_or_else(storage)?;
            let doc = object! {version:1,revision:s.revision.to_string(),favorites:s.favorites.iter().map(SubmissionKeyV1::json).collect::<Vec<_>>()};
            let bytes = crate::store::canonical(&doc).into_bytes();
            if bytes.len() > LIMIT as usize {
                return Err(c::error("catalogFull"));
            }
            super::store::reserve(&self.root)?;
            let temporary = self.root.join(".catalog-preferences-v1.replace");
            // A previous unknown replacement is preserved, never overwritten or
            // replayed. Its presence blocks further writes pending reconciliation.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(0x20000)
                .open(&temporary)
                .map_err(|_| storage())?;
            file.write_all(&bytes).map_err(|_| storage())?;
            file.sync_all().map_err(|_| storage())?;
            fs::rename(&temporary, self.root.join("catalog-preferences-v1.json"))
                .map_err(|_| storage())?;
            self.directory.sync_all().map_err(|_| storage())?;
        }
        Ok(
            object! {preferencesRevision:s.revision.to_string(),submission:key.json(),favorite:desired,changed:changed},
        )
    }
}
fn storage() -> NativeError {
    NativeError::new("catalogStorage","Catalog preferences or preserved replacement are unavailable; read the authoritative revision before another action.")
}
