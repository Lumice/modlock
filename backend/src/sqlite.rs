//! Narrow system SQLite FFI. Text parameters are copied by SQLite; statements,
//! connections and unfinished transactions are released by RAII. No registry crates.
use crate::{native::NativeError, paths};
use std::{
    ffi::{c_char, c_int, c_void, CString},
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    ptr,
};
#[repr(C)]
struct Sqlite {
    _opaque: [u8; 0],
}
#[repr(C)]
struct Stmt {
    _opaque: [u8; 0],
}
#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open_v2(
        name: *const c_char,
        db: *mut *mut Sqlite,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut Sqlite) -> c_int;
    fn sqlite3_get_autocommit(db: *mut Sqlite) -> c_int;
    fn sqlite3_busy_timeout(db: *mut Sqlite, ms: c_int) -> c_int;
    fn sqlite3_limit(db: *mut Sqlite, category: c_int, value: c_int) -> c_int;
    fn sqlite3_malloc64(size: u64) -> *mut c_void;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_serialize(db: *mut Sqlite, schema: *const c_char, size: *mut i64, flags: u32) -> *mut u8;
    fn sqlite3_deserialize(db: *mut Sqlite, schema: *const c_char, data: *mut u8, size: i64, capacity: i64, flags: u32) -> c_int;
    fn sqlite3_prepare_v2(
        db: *mut Sqlite,
        sql: *const c_char,
        n: c_int,
        stmt: *mut *mut Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_finalize(stmt: *mut Stmt) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut Stmt,
        index: c_int,
        text: *const c_char,
        n: c_int,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut Stmt) -> c_int;
    fn sqlite3_column_count(stmt: *mut Stmt) -> c_int;
    fn sqlite3_column_bytes(stmt: *mut Stmt, col: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut Stmt, col: c_int) -> *const u8;
}
const MAX_TEXT: usize = 64 * 1024 * 1024;
fn check(rc: c_int) -> Result<(), NativeError> {
    match rc {
        0 => Ok(()),
        13 => Err(NativeError::new("nativeDatabase","Not enough disk space to save the library (SQLITE_FULL 13).")),
        5 | 6 => Err(NativeError::new(
            "nativeBusy",
            "Native database is busy; retry when the other operation finishes.",
        )),
        _ => Err(NativeError::new(
            "nativeDatabase",
            "Native database operation failed; check storage and permissions.",
        )),
    }
}
pub struct Database {
    raw: *mut Sqlite,
}
impl Database {
    /// The unpublished candidate is built privately in memory. SQLite never
    /// opens the candidate pathname for writing; the owner exports the image
    /// through its retained create-new descriptor after the transaction closes.
    pub(crate) fn bootstrap_memory() -> Result<Self, NativeError> {
        let mut raw=ptr::null_mut();
        let rc=unsafe { sqlite3_open_v2(c":memory:".as_ptr(), &mut raw, 2|4, ptr::null()) };
        if rc!=0 {
            if !raw.is_null() { unsafe { sqlite3_close(raw); } }
            check(rc)?;
        }
        let db=Self {raw};
        // Both fresh candidates and read-only resumed images use this private
        // connection, so bound text and SQL before any image is deserialized.
        unsafe {
            sqlite3_limit(raw, 0, 64 * 1024);
            sqlite3_limit(raw, 1, 64 * 1024);
        }
        check(unsafe { sqlite3_busy_timeout(raw, 100) })?;
        db.execute("PRAGMA temp_store=MEMORY", &[])?;
        if db.query("PRAGMA temp_store", &[])?.as_ref().and_then(|r|r.first()).map(String::as_str)!=Some("2")
            || db.query("PRAGMA journal_mode=MEMORY", &[])?.as_ref().and_then(|r|r.first()).map(String::as_str)!=Some("memory") {
            return Err(NativeError::new("bootstrapStorage","Cannot establish a private in-memory bootstrap database."));
        }
        Ok(db)
    }
    /// Candidate bytes may be incomplete or adversarial. SQLite documents an
    /// extra 20 bytes of allocation for malformed deserializations. READONLY
    /// prevents writes; FREEONCLOSE transfers the allocation even on failure.
    pub(crate) fn bootstrap_image(image: &[u8]) -> Result<Self, NativeError> {
        let db=Self::bootstrap_memory()?;
        let size=i64::try_from(image.len()).map_err(|_|NativeError::params())?;
        let capacity=size.checked_add(20).ok_or_else(NativeError::params)?;
        let data=unsafe { sqlite3_malloc64(capacity as u64).cast::<u8>() };
        if data.is_null() { return Err(NativeError::new("bootstrapStorage","Cannot allocate a bounded bootstrap image.")); }
        unsafe {
            ptr::copy_nonoverlapping(image.as_ptr(),data,image.len());
            ptr::write_bytes(data.add(image.len()),0,20);
        }
        let rc=unsafe { sqlite3_deserialize(db.raw,c"main".as_ptr(),data,size,capacity,1|4) };
        check(rc)?;
        Ok(db)
    }
    pub(crate) fn bootstrap_serialize(&self, limit: usize) -> Result<Vec<u8>, NativeError> {
        let mut size=0i64;
        let data=unsafe { sqlite3_serialize(self.raw,c"main".as_ptr(),&mut size,0) };
        if data.is_null() { return Err(NativeError::new("bootstrapStorage","Cannot serialize the bootstrap candidate.")); }
        let result=match usize::try_from(size) {
            Ok(n) if n>0 && n<=limit => Ok(unsafe { std::slice::from_raw_parts(data,n) }.to_vec()),
            _ => Err(NativeError::new("bootstrapLimit","Bootstrap database exceeds its fixed storage budget.")),
        };
        unsafe { sqlite3_free(data.cast()); }
        result
    }
    pub(crate) fn bootstrap_close(mut self) -> Result<(), NativeError> {
        let rc=unsafe { sqlite3_close(self.raw) };
        if rc==0 { self.raw=ptr::null_mut(); }
        check(rc)
    }
    /// Bootstrap has already rejected hot/WAL headers and sidecars. Immutable
    /// read-only mode additionally forbids SQLite from creating shared-memory or
    /// recovery files if a path changes during that admission window. The caller
    /// must recheck its held file/directory identities and metadata after reads.
    pub(crate) fn bootstrap_readonly(path: &Path) -> Result<Self, NativeError> {
        paths::reject_symlinks(path)?;
        let encoded: String = path.as_os_str().as_encoded_bytes().iter().map(|b|format!("%{b:02X}")).collect();
        let name=CString::new(format!("file:{encoded}?mode=ro&immutable=1")).map_err(|_|NativeError::params())?;
        let mut raw=ptr::null_mut();
        let rc=unsafe{sqlite3_open_v2(name.as_ptr(),&mut raw,1|0x40|0x01000000,ptr::null())};
        if rc!=0 {
            if !raw.is_null(){unsafe{sqlite3_close(raw)};}
            check(rc)?;
        }
        let db=Self{raw};
        unsafe{sqlite3_limit(raw,0,MAX_TEXT as c_int);}
        check(unsafe{sqlite3_busy_timeout(raw,100)})?;
        Ok(db)
    }
    pub fn open(path: &Path, create: bool) -> Result<Self, NativeError> {
        if path.exists() {
            paths::reject_symlinks(path)?;
        } else if !create {
            return Err(NativeError::not_imported());
        } else {
            if let Some(parent) = path.parent() {
                paths::reject_symlinks(parent)?;
            }
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(0x20000)
                .open(path)?;
        }
        let name =
            CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| NativeError::params())?;
        let mut raw = ptr::null_mut();
        let rc = unsafe {
            sqlite3_open_v2(
                name.as_ptr(),
                &mut raw,
                (if create { 2 | 4 } else { 1 }) | 0x01000000,
                ptr::null(),
            )
        };
        if rc != 0 {
            if !raw.is_null() {
                unsafe { sqlite3_close(raw) };
            }
            check(rc)?;
        }
        let db = Self { raw };
        unsafe {
            sqlite3_limit(raw, 0, MAX_TEXT as c_int);
        }
        check(unsafe { sqlite3_busy_timeout(raw, 100) })?;
        Ok(db)
    }
    /// Per-connection bounds for the independent catalog; library connections
    /// retain their existing limits. No SQLite process-wide configuration.
    pub fn catalog_limits(&self) {
        unsafe {
            sqlite3_limit(self.raw, 0, 1024 * 1024);
            sqlite3_limit(self.raw, 1, 1024 * 1024);
            sqlite3_limit(self.raw, 9, 512);
        }
    }
    /// A fixed empty in-memory bootstrap database is page-capped before schema work.
    /// Retained legacy candidate journals are separately bounded by admission.
    pub(crate) fn bootstrap_limits(&self) -> Result<(), NativeError> {
        let limit = || NativeError::new("bootstrapLimit", "Bootstrap database exceeds its fixed storage budget.");
        let row = self.query("PRAGMA page_size", &[])?.ok_or_else(limit)?;
        let size = row[0].parse::<u64>().map_err(|_|limit())?;
        if !(512..=65536).contains(&size) || !size.is_power_of_two() { return Err(limit()); }
        let pages = (1024 * 1024u64).checked_div(size).filter(|n|*n>0).ok_or_else(limit)?;
        let row = self.query(&format!("PRAGMA max_page_count={pages}"), &[])?.ok_or_else(limit)?;
        if row[0].parse::<u64>().ok() != Some(pages) { return Err(limit()); }
        Ok(())
    }
    fn prepare(&self, sql: &str, params: &[&str]) -> Result<Statement<'_>, NativeError> {
        self.prepare_checked(sql,params,check)
    }
    /// The classifier consumes each numeric SQLite result at its failing call,
    /// before statement finalization/rollback can change connection error state.
    fn prepare_checked(&self,sql:&str,params:&[&str],classify:fn(c_int)->Result<(),NativeError>)->Result<Statement<'_>,NativeError>{
        let sql = CString::new(sql).map_err(|_| NativeError::params())?;
        let mut raw = ptr::null_mut();
        let rc=unsafe{sqlite3_prepare_v2(self.raw,sql.as_ptr(),-1,&mut raw,ptr::null_mut())};
        if let Err(error)=classify(rc){
            // The original failure is already owned before any cleanup call.
            if !raw.is_null(){unsafe{sqlite3_finalize(raw)};}
            return Err(error);
        }
        let s = Statement { raw, _db: self };
        for (i, p) in params.iter().enumerate() {
            if p.len() > MAX_TEXT {
                return Err(NativeError::new(
                    "sourceTooLarge",
                    "Native database text exceeds the size limit.",
                ));
            }
            // SQLITE_TRANSIENT (-1) tells SQLite to copy this borrowed buffer immediately.
            let transient = unsafe {
                std::mem::transmute::<isize, Option<unsafe extern "C" fn(*mut c_void)>>(-1)
            };
            let rc=unsafe {
                sqlite3_bind_text(
                    raw,
                    (i + 1) as c_int,
                    p.as_ptr().cast(),
                    p.len() as c_int,
                    transient,
                )
            };
            classify(rc)?;
        }
        Ok(s)
    }
    pub fn execute(&self, sql: &str, params: &[&str]) -> Result<(), NativeError> {
        let s = self.prepare(sql, params)?;
        let rc = unsafe { sqlite3_step(s.raw) };
        if rc == 101 {
            Ok(())
        } else {
            check(rc)?;
            Err(NativeError::new(
                "nativeDatabase",
                "Unexpected native database result.",
            ))
        }
    }
    pub fn query(&self, sql: &str, params: &[&str]) -> Result<Option<Vec<String>>, NativeError> {
        let s = self.prepare(sql, params)?;
        match unsafe { sqlite3_step(s.raw) } {
            101 => Ok(None),
            100 => {
                let n = unsafe { sqlite3_column_count(s.raw) };
                let mut values = Vec::new();
                for col in 0..n {
                    let len = unsafe { sqlite3_column_bytes(s.raw, col) };
                    if len < 0 || len as usize > MAX_TEXT {
                        return Err(NativeError::new(
                            "nativeDatabase",
                            "Native database value exceeds the size limit.",
                        ));
                    }
                    let ptr = unsafe { sqlite3_column_text(s.raw, col) };
                    if ptr.is_null() {
                        return Err(NativeError::new(
                            "nativeDatabase",
                            "Unexpected null native database value.",
                        ));
                    }
                    let b = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
                    values.push(
                        std::str::from_utf8(b)
                            .map_err(|_| {
                                NativeError::new(
                                    "nativeDatabase",
                                    "Native database text is invalid.",
                                )
                            })?
                            .to_owned(),
                    )
                }
                Ok(Some(values))
            }
            rc => {
                check(rc)?;
                Err(NativeError::new(
                    "nativeDatabase",
                    "Unexpected native database result.",
                ))
            }
        }
    }
    /// Bounded catalog reader. No global limits or connection transfer.
    pub fn query_rows(&self, sql: &str, params: &[&str], max_rows: usize, max_bytes: usize) -> Result<Vec<Vec<String>>, NativeError> {
        let s = self.prepare(sql, params)?;
        let mut rows = Vec::new(); let mut bytes = 0usize;
        loop {
            match unsafe { sqlite3_step(s.raw) } {
                101 => return Ok(rows),
                100 => {
                    if rows.len() >= max_rows { return Err(NativeError::new("catalogIncomplete", "Catalog result exceeds its bounded row coverage.")); }
                    let mut row = Vec::new();
                    for col in 0..unsafe { sqlite3_column_count(s.raw) } {
                        let len = unsafe { sqlite3_column_bytes(s.raw, col) };
                        if len < 0 { return Err(NativeError::new("catalogStorage", "Invalid catalog column length.")); }
                        bytes = bytes.checked_add(len as usize).filter(|n| *n <= max_bytes).ok_or_else(|| NativeError::new("catalogResponseTooLarge", "Catalog result exceeds its bounded byte budget."))?;
                        let ptr = unsafe { sqlite3_column_text(s.raw, col) };
                        if ptr.is_null() { return Err(NativeError::new("catalogStorage", "Unexpected null catalog column.")); }
                        row.push(std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len as usize) }).map_err(|_| NativeError::new("catalogStorage", "Invalid catalog text."))?.to_owned());
                    }
                    rows.push(row);
                },
                rc => { check(rc)?; return Err(NativeError::new("catalogStorage", "Unexpected catalog result.")); },
            }
        }
    }
    /// Optional catalog indexing alone uses these classified write results.
    /// Capture before statement/transaction cleanup can replace SQLite's error.
    pub fn maintenance_execute(&self,sql:&str,params:&[&str])->Result<(),NativeError>{
        let s=self.prepare_checked(sql,params,maintenance_check)?;
        let rc=unsafe{sqlite3_step(s.raw)};
        if rc==101{Ok(())}else{Err(maintenance_error(rc))}
    }
    pub fn maintenance_autocommit(&self)->bool{unsafe{sqlite3_get_autocommit(self.raw)!=0}}
    pub fn maintenance_transaction(&self)->Result<Transaction<'_>,NativeError>{
        self.maintenance_execute("BEGIN IMMEDIATE",&[])?;
        Ok(Transaction{db:self,committed:false})
    }
    pub fn transaction(&self) -> Result<Transaction<'_>, NativeError> {
        self.execute("BEGIN IMMEDIATE", &[])?;
        Ok(Transaction {
            db: self,
            committed: false,
        })
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        if !self.raw.is_null() { unsafe { sqlite3_close(self.raw) }; }
    }
}
struct Statement<'a> {
    raw: *mut Stmt,
    _db: &'a Database,
}
impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe { sqlite3_finalize(self.raw) };
    }
}
pub struct Transaction<'a> {
    pub db: &'a Database,
    committed: bool,
}
impl Transaction<'_> {
    pub fn maintenance_commit(mut self)->Result<(),NativeError>{
        self.db.maintenance_execute("COMMIT",&[])?;self.committed=true;Ok(())
    }
    pub fn commit(mut self) -> Result<(), NativeError> {
        self.db.execute("COMMIT", &[])?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.db.execute("ROLLBACK", &[]);
        }
    }
}

fn maintenance_error(rc:c_int)->NativeError{
    match rc&255{
        13=>NativeError::new("catalogFull","Search indexing paused because the catalog database cannot grow within its storage limit."),
        5|6|7|8|10|14|18=>NativeError::new("catalogMaintenanceWrite","Search indexing paused after a temporary write, memory, or bounded-size refusal; cached browsing remains available."),
        _=>NativeError::new("catalogStorage","Search maintenance encountered invalid or unreadable catalog storage."),
    }
}

fn maintenance_check(rc:c_int)->Result<(),NativeError>{
    if rc==0{Ok(())}else{Err(maintenance_error(rc))}
}

#[cfg(test)]
pub(crate) fn test_full_error()->NativeError {check(13).unwrap_err()}
#[cfg(test)]
#[test]
fn item115_diagnostics_sqlite_full_keeps_database_kind(){let e=test_full_error();assert_eq!(e.kind,"nativeDatabase");assert!(e.message.contains("Not enough disk space"));assert!(e.message.contains("SQLITE_FULL"));}
