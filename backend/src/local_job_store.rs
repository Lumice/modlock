//! Separate job metadata. The serial service owns this connection and session lease.
use crate::{native::NativeError, paths, sqlite::Database};
use json::JsonValue;
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}
pub(crate) fn corrupt() -> NativeError {
    NativeError::new("localJobDatabase", "Local job history is invalid; preserve it for inspection. The native library remains available.")
}
fn unsupported() -> NativeError {
    NativeError::new("unsupportedLocalJobVersion", "This local job database format or version is unsupported; preserve it and use a compatible helper.")
}
/// Even a read-only SQLite connection can modify WAL shared memory. Recognize
/// only our private rollback-journal format before giving SQLite any access.
fn check_storage_format(root: &Path, existing: bool) -> Result<(), NativeError> {
    for name in ["local-jobs.sqlite3-wal", "local-jobs.sqlite3-shm"] {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => return Err(unsupported()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if existing {
        let mut file = paths::open_regular(&root.join("local-jobs.sqlite3"))?;
        let mut header = [0u8; 100];
        match file.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(unsupported()),
            Err(e) => return Err(e.into()),
        }
        if &header[..16] != b"SQLite format 3\0" || header[18] != 1 || header[19] != 1 {
            return Err(unsupported());
        }
    }
    Ok(())
}

struct Lease(File);
impl Lease {
    fn acquire(root: &Path) -> Result<Self, NativeError> {
        paths::reject_symlinks(root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(0x20000 | 0x800)
            .open(root.join(".local-jobs.lock"))?;
        if !file.metadata()?.is_file() {
            return Err(corrupt());
        }
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err(NativeError::new(
                "nativeBusy",
                "Another helper owns local preparation for this library.",
            ));
        }
        Ok(Self(file))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe { flock(self.0.as_raw_fd(), 8) };
    }
}
pub(crate) struct LocalJobStore {
    root: PathBuf,
    db: Database,
    _lease: Lease,
    #[cfg(test)]
    pub(crate) fail_writes: bool,
}
impl LocalJobStore {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub fn open(root: &Path, create: bool) -> Result<Option<Self>, NativeError> {
        let path = root.join("local-jobs.sqlite3");
        let exists = match std::fs::symlink_metadata(&path) {
            Ok(m) => {
                if !m.is_file() {
                    return Err(corrupt());
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if !exists && !create {
            return Ok(None);
        }
        let lease = Lease::acquire(root)?;
        // Another helper may have created the database before our lease acquisition.
        let existing = path.try_exists()?;
        check_storage_format(root, existing)?;
        if existing {
            let probe = Database::open(&path, false)?;
            let version = probe
                .query("PRAGMA user_version", &[])?
                .ok_or_else(corrupt)?;
            if version != ["1"] && version != ["2"] && version != ["3"] {
                return Err(unsupported());
            }
            let count=probe.query("SELECT count(*) FROM pragma_table_info('jobs') WHERE (name='id' AND type='TEXT' AND pk=1) OR (name='input' AND type='TEXT' AND \"notnull\"=1 AND pk=0) OR (name='body' AND type='TEXT' AND \"notnull\"=1 AND pk=0)",&[])?.ok_or_else(corrupt)?;
            let columns = probe
                .query("SELECT count(*) FROM pragma_table_info('jobs')", &[])?
                .ok_or_else(corrupt)?;
            let unexpected=probe.query("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND NOT (type='table' AND name='jobs')",&[])?.ok_or_else(corrupt)?;
            if count != ["3"] || columns != ["3"] || unexpected != ["0"] {
                return Err(corrupt());
            }
        }
        let db = Database::open(&path, true)?;
        if !existing {
            db.query("PRAGMA journal_mode=DELETE", &[])?;
            db.execute("PRAGMA synchronous=FULL", &[])?;
            let tx = db.transaction()?;
            db.execute(
                "CREATE TABLE jobs(id TEXT PRIMARY KEY,input TEXT NOT NULL,body TEXT NOT NULL)",
                &[],
            )?;
            db.execute("PRAGMA user_version=2", &[])?;
            tx.commit()?;
            paths::sync_dir(root)?;
        }
        Ok(Some(Self {
            root: paths::absolute(root)?,
            db,
            _lease: lease,
            #[cfg(test)]
            fail_writes: false,
        }))
    }
    pub(crate) fn version(&self) -> Result<u64, NativeError> {
        self.db
            .query("PRAGMA user_version", &[])?
            .ok_or_else(corrupt)?[0]
            .parse()
            .map_err(|_| corrupt())
    }
    /// Only explicit preparation calls this, after all retained records reconcile.
    pub(crate) fn upgrade_for_prepare(&self) -> Result<(), NativeError> {
        if self.version()? == 1 {
            let tx = self.db.transaction()?;
            self.db.execute("PRAGMA user_version=2", &[])?;
            tx.commit()?;
        }
        Ok(())
    }
    pub(crate) fn upgrade_for_options(&self) -> Result<(), NativeError> {
        if self.version()? < 3 {
            let tx = self.db.transaction()?;
            self.db.execute("PRAGMA user_version=3", &[])?;
            tx.commit()?;
        }
        Ok(())
    }
    pub fn load(&self) -> Result<Vec<(String, String, JsonValue)>, NativeError> {
        let mut rows = Vec::new();
        let mut previous = String::new();
        // Check count first, including invalid empty IDs that keyset iteration would skip.
        let count = self
            .db
            .query("SELECT count(*) FROM jobs", &[])?
            .ok_or_else(corrupt)?;
        if count[0].parse::<usize>().map_err(|_| corrupt())? > 8 {
            return Err(corrupt());
        }
        while let Some(row) = self.db.query(
            "SELECT id,input,body FROM jobs WHERE id>? ORDER BY id LIMIT 1",
            &[&previous],
        )? {
            if rows.len() >= 8 || row[1].len() > 64 * 1024 || row[2].len() > 4096 {
                return Err(corrupt());
            }
            let body = json::parse(&row[2]).map_err(|_| corrupt())?;
            previous = row[0].clone();
            rows.push((row[0].clone(), row[1].clone(), body));
        }
        if rows.len() != count[0].parse::<usize>().map_err(|_| corrupt())? {
            return Err(corrupt());
        }
        Ok(rows)
    }
    pub fn save(&self, rows: &[(String, String, JsonValue)]) -> Result<(), NativeError> {
        #[cfg(test)]
        if self.fail_writes {
            return Err(corrupt());
        }
        self.db.execute("PRAGMA synchronous=FULL", &[])?;
        let tx = self.db.transaction()?;
        for (id, input, body) in rows {
            if input.len() > 64 * 1024 || body.dump().len() > 4096 {
                return Err(corrupt());
            }
            self.db.execute("INSERT INTO jobs(id,input,body) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET input=excluded.input,body=excluded.body",&[id,input,&body.dump()])?;
        }
        tx.commit()
    }
    pub fn delete(&self, id: &str) -> Result<(), NativeError> {
        #[cfg(test)]
        if self.fail_writes {
            return Err(corrupt());
        }
        self.db.execute("DELETE FROM jobs WHERE id=?", &[id])
    }
}
