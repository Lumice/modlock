//! Native-owned catalog v1. SQLite is opened, used and dropped on owner worker.
use super::{
    contract as c,
    provider::{ProviderRecord, SubmissionKeyV1, SOURCE_PIN},
};
use crate::{native::NativeError, paths, sqlite::Database};
use json::{object, JsonValue};
use std::{
    ffi::CString,
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub type CatalogRevision = u64;
pub const DB_LIMIT: u64 = 256 * 1024 * 1024;
pub const MAX_SUMMARIES: usize = 37_500;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(253402300799)
}
pub fn storage() -> NativeError {
    NativeError::new(
        "catalogStorage",
        "Catalog storage failed; the database and recovery artifacts were preserved.",
    )
}
pub fn reserve(path: &Path) -> Result<(), NativeError> {
    #[repr(C)]
    struct Stat {
        bsize: std::ffi::c_ulong,
        frsize: std::ffi::c_ulong,
        blocks: u64,
        bfree: u64,
        bavail: u64,
        files: u64,
        ffree: u64,
        favail: u64,
        fsid: std::ffi::c_ulong,
        flag: std::ffi::c_ulong,
        namemax: std::ffi::c_ulong,
        spare: [i32; 6],
    }
    unsafe extern "C" {
        fn statvfs(path: *const std::ffi::c_char, result: *mut Stat) -> i32;
    }
    let p = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| storage())?;
    let mut stat = std::mem::MaybeUninit::<Stat>::uninit();
    if unsafe { statvfs(p.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(storage());
    }
    let stat = unsafe { stat.assume_init() };
    if stat.bavail.checked_mul(stat.frsize as u64).unwrap_or(0) < 1024 * 1024 * 1024 {
        return Err(NativeError::new(
            "catalogFull",
            "Catalog writes require a 1 GiB free-space admission reserve.",
        ));
    }
    Ok(())
}
/// One already normalized page and its durable successor cursor. No network
/// operation occurs with the transaction open.
pub struct ValidatedPage {
    pub records: Vec<ProviderRecord>,
    pub cursor_name: String,
    pub cursor: JsonValue,
    pub snapshot: String,
    pub states: Vec<(String, String)>,
}
pub struct CatalogStore {
    pub(crate) db: Database,
    root: PathBuf,
    path: PathBuf,
    database_file: File,
    pub fts: bool,
    pub revision: u64,
    refresh_error: JsonValue,
    search_paused: bool,
    search_error: JsonValue,
    pub(crate) collection_projection: super::collections::CollectionProjection,
}
impl CatalogStore {
    pub fn open(root: &Path) -> Result<Self, NativeError> {
        let cache = root.join("cache");
        paths::private_dir(&cache)?;
        let path = cache.join("catalog-v1.sqlite3");
        let exists = match fs::symlink_metadata(&path) {
            Ok(m) => {
                if !m.is_file() || m.nlink() != 1 || m.len() > DB_LIMIT {
                    return Err(storage());
                }
                let mut header = [0u8; 100];
                paths::open_regular(&path)?
                    .read_exact(&mut header)
                    .map_err(|_| storage())?;
                if &header[..16] != b"SQLite format 3\0"
                    || u32::from_be_bytes(header[60..64].try_into().unwrap()) != 1
                    || u32::from_be_bytes(header[68..72].try_into().unwrap()) != 0x44434c31
                    || u16::from_be_bytes(header[16..18].try_into().unwrap()) != 4096
                {
                    return Err(storage());
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(storage()),
        };
        // Unknown sidecars for an absent DB must not be attached to a new cache.
        for suffix in ["-wal", "-shm", "-journal"] {
            let side = PathBuf::from(format!("{}{suffix}", path.display()));
            match fs::symlink_metadata(&side) {
                Ok(m) => {
                    if !exists || suffix != "-journal" || !m.is_file() || m.nlink() != 1 {
                        return Err(NativeError::new(
                            "catalogStorage",
                            format!("Preserved unsupported catalog artifact under root: cache/catalog-v1.sqlite3{suffix}"),
                        ));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(storage()),
            }
        }
        reject_unknown_artifacts(&cache)?;
        reserve(&cache)?;
        let expected = fs::symlink_metadata(&path).ok().map(|m| (m.dev(), m.ino()));
        let db = Database::open(&path, true).map_err(|_| storage())?;
        let database_file = paths::open_regular(&path)?;
        let held = database_file.metadata()?;
        if expected.is_some_and(|id| id != (held.dev(), held.ino())) || held.nlink() != 1 {
            return Err(storage());
        }
        db.catalog_limits();
        if !exists {
            db.execute("PRAGMA page_size=4096", &[])
                .map_err(|_| storage())?;
        }
        if scalar(&db, "PRAGMA page_size")? != 4096 {
            return Err(storage());
        }
        if scalar(&db, "PRAGMA max_page_count=65536")? != 65536
            || scalar(&db, "PRAGMA page_count")? > 65536
        {
            return Err(storage());
        }
        if db
            .query("PRAGMA journal_mode=DELETE", &[])
            .map_err(|_| storage())?
            .as_ref()
            .and_then(|r| r.first())
            .map(String::as_str)
            != Some("delete")
        {
            return Err(storage());
        }
        for sql in [
            "PRAGMA synchronous=FULL",
            "PRAGMA temp_store=MEMORY",
            "PRAGMA cache_size=-8192",
            "PRAGMA foreign_keys=ON",
        ] {
            db.execute(sql, &[]).map_err(|_| storage())?;
        }
        if scalar(&db, "PRAGMA synchronous")? != 2 || scalar(&db, "PRAGMA temp_store")? != 2 {
            return Err(storage());
        }
        if !exists {
            let tx = db.transaction().map_err(|_| storage())?;
            for sql in SCHEMA {
                tx.db.execute(sql, &[]).map_err(|_| storage())?;
            }
            tx.db
                .execute("PRAGMA application_id=1145261105", &[])
                .map_err(|_| storage())?;
            tx.db
                .execute("PRAGMA user_version=1", &[])
                .map_err(|_| storage())?;
            tx.commit().map_err(|_| storage())?;
            paths::sync_dir(&cache)?;
        }
        if scalar(&db, "PRAGMA application_id")? != 0x44434c31
            || scalar(&db, "PRAGMA user_version")? != 1
        {
            return Err(storage());
        }
        // Only absence of the FTS compile capability is a search-specific
        // limitation. Corruption/schema/I/O failures remain storage errors.
        let fts = scalar(&db, "SELECT sqlite_compileoption_used('ENABLE_FTS5')")? == 1;
        let revision_row = db
            .query("SELECT value FROM catalog_state WHERE name='revision'", &[])
            .map_err(|_| storage())?
            .ok_or_else(storage)?;
        let revision = c::u64_text(&revision_row[0].as_str().into()).map_err(|_| storage())?;
        let refresh_error = db
            .query(
                "SELECT value FROM catalog_state WHERE name='last_refresh_error'",
                &[],
            )
            .map_err(|_| storage())?
            .map(|r| super::provider::parse_bounded(r[0].as_bytes(), 1024).map_err(|_| storage()))
            .transpose()?
            .unwrap_or(JsonValue::Null);
        validate_refresh_error(&refresh_error)?;
        let store = Self {
            db,
            root: root.to_owned(),
            path,
            database_file,
            fts,
            revision,
            refresh_error,
            search_paused: false,
            search_error: JsonValue::Null,
            collection_projection: super::collections::CollectionProjection::default(),
        };
        store.state_flag("coverage_complete")?;
        store.state_flag("fts_dirty")?;
        if fts {
            store.admit()?;
            store.db.execute("CREATE VIRTUAL TABLE IF NOT EXISTS submission_fts USING fts5(provider UNINDEXED,submission_type UNINDEXED,submission_id UNINDEXED,name,author,description,tokenize='unicode61 remove_diacritics 2')",&[]).map_err(|_|storage())?;
        }
        if store.fts
            && store
                .db
                .query("PRAGMA quick_check(1)", &[])
                .map_err(|_| storage())?
                .as_ref()
                .and_then(|r| r.first())
                .map(String::as_str)
                != Some("ok")
        {
            return Err(storage());
        }
        Ok(store)
    }
    /// Clear provider display data in one SQLite transaction. Library payloads,
    /// favorites and download receipts live outside this database.
    pub fn clear_cache(&mut self) -> Result<(), NativeError> {
        self.admit()?;
        let revision=self.revision.checked_add(1).ok_or_else(storage)?;
        let tx=self.db.transaction().map_err(|_|storage())?;
        for table in ["submission","detail","file","activity","collection","collection_member","tombstone_candidate","member_staging","catalog_state"] {
            tx.db.execute(&format!("DELETE FROM {table}"),&[]).map_err(|_|storage())?;
        }
        if self.fts { tx.db.execute("DELETE FROM submission_fts",&[]).map_err(|_|storage())?; }
        tx.db.execute("INSERT INTO catalog_state VALUES('revision',?1),('fts_dirty','1'),('coverage_complete','0')",&[&revision.to_string()]).map_err(|_|storage())?;
        tx.commit().map_err(|_|storage())?;
        self.revision=revision;
        self.refresh_error=JsonValue::Null;
        self.search_paused=false;
        self.search_error=JsonValue::Null;
        self.collection_projection=super::collections::CollectionProjection::default();
        Ok(())
    }
    pub fn search_available(&self) -> Result<bool, NativeError> {
        Ok(self.fts && !self.search_paused && !self.state_flag("fts_dirty")?)
    }
    /// Idle maintenance publishes at most50 FTS rows then yields. Search stays
    /// unavailable until completion; non-search reads/favorites stay runnable.
    pub fn maintain_search(&mut self) -> Result<(), NativeError> {
        if !self.fts || self.search_paused {
            return Ok(());
        }
        if self.search_available()? {
            self.search_error = JsonValue::Null;
            return Ok(());
        }
        self.maintenance_readable()?;
        let after = self.state("fts_rebuild_after")?.unwrap_or_default();
        let rows = self
            .db
            .query_rows(
                "SELECT slug,payload FROM submission WHERE slug>?1 ORDER BY slug LIMIT 50",
                &[&after],
                50,
                10 * 1024 * 1024,
            )
            .map_err(|_| storage())?;
        let mut input = Vec::new();
        for row in &rows {
            let data = super::provider::parse_bounded(row[1].as_bytes(), 192 * 1024)
                .map_err(|_| storage())?;
            let key = SubmissionKeyV1::parse(&data["submission"]).map_err(|_| storage())?;
            for field in ["name", "author", "description"] {
                if data[field].as_str().is_none() {
                    return Err(storage());
                }
            }
            input.push((key, data));
        }
        let result = (|| {
            self.admit()?;
            let tx = self.db.maintenance_transaction()?;
            for (key, data) in &input {
                index_with(tx.db, key, data, true)?;
            }
            if let Some(row) = rows.last() {
                tx.db.maintenance_execute("INSERT INTO catalog_state(name,value) VALUES('fts_rebuild_after',?1) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[&row[0]])?;
            } else {
                tx.db.maintenance_execute(
                    "UPDATE catalog_state SET value='0' WHERE name='fts_dirty'",
                    &[],
                )?;
                tx.db.maintenance_execute(
                    "DELETE FROM catalog_state WHERE name='fts_rebuild_after'",
                    &[],
                )?;
            }
            tx.maintenance_commit()
        })();
        match result {
            Ok(()) => {
                if rows.is_empty() {
                    self.search_error = JsonValue::Null;
                }
                Ok(())
            }
            Err(error)
                if matches!(
                    error.kind.as_str(),
                    "catalogFull" | "catalogMaintenanceWrite"
                ) =>
            {
                // Rollback has already run; unresolved transactions, ownership
                // loss, or unreadable authoritative rows remain fatal.
                self.maintenance_readable()?;
                self.search_paused = true;
                self.search_error = c::failure(
                    if error.kind == "catalogFull" {
                        "catalogFull"
                    } else {
                        "catalogStorage"
                    },
                    &format!("Search indexing paused: {}", error.message),
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
    fn maintenance_readable(&self) -> Result<(), NativeError> {
        paths::reject_symlinks(&self.path)?;
        let named = fs::symlink_metadata(&self.path)?;
        let held = self.database_file.metadata()?;
        if (named.dev(), named.ino()) != (held.dev(), held.ino())
            || held.nlink() != 1
            || held.len() > DB_LIMIT
            || !self.db.maintenance_autocommit()
        {
            return Err(storage());
        }
        self.state_flag("coverage_complete")?;
        self.state_flag("fts_dirty")?;
        let revision = self.state("revision")?.ok_or_else(storage)?;
        if c::u64_text(&revision.into()).map_err(|_| storage())? != self.revision {
            return Err(storage());
        }
        let rows = self
            .db
            .query_rows(
                "SELECT payload FROM submission ORDER BY slug LIMIT 1",
                &[],
                1,
                192 * 1024,
            )
            .map_err(|_| storage())?;
        for row in rows {
            let data = super::provider::parse_bounded(row[0].as_bytes(), 192 * 1024)
                .map_err(|_| storage())?;
            SubmissionKeyV1::parse(&data["submission"]).map_err(|_| storage())?;
        }
        Ok(())
    }
    pub fn retry_search_maintenance(&mut self) {
        self.search_paused = false;
    }
    pub fn search_unavailable_error(&self) -> NativeError {
        NativeError::new(
            "catalogSearchUnavailable",
            if self.search_paused {
                "Search indexing is paused after a write refusal; cached browsing remains available. Explicit refresh retries indexing."
            } else if !self.fts {
                "This SQLite connection does not provide the required FTS5 search capability."
            } else {
                "Search indexing is rebuilding; cached browsing remains available."
            },
        )
    }
    pub fn state(&self, name: &str) -> Result<Option<String>, NativeError> {
        Ok(self
            .db
            .query("SELECT value FROM catalog_state WHERE name=?1", &[name])
            .map_err(|_| storage())?
            .map(|v| v[0].clone()))
    }
    pub fn state_time(&self, name: &str) -> Result<Option<u64>, NativeError> {
        self.state(name)?
            .map(|s| {
                let n = c::u64_text(&s.into()).map_err(|_| storage())?;
                if n > 253402300799 {
                    return Err(storage());
                }
                Ok(n)
            })
            .transpose()
    }
    pub fn state_flag(&self, name: &str) -> Result<bool, NativeError> {
        match self.state(name)?.as_deref() {
            Some("1") => Ok(true),
            Some("0") => Ok(false),
            _ => Err(storage()),
        }
    }
    pub fn checkpoint(&mut self, states: &[(&str, String)]) -> Result<(), NativeError> {
        if states.len() > 4
            || states
                .iter()
                .any(|(k, v)| k.len() > 64 || v.len() > 16 * 1024)
        {
            return Err(storage());
        }
        self.admit()?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        for (key, value) in states {
            tx.db.execute("INSERT INTO catalog_state(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[key,value]).map_err(|_|storage())?;
        }
        tx.commit().map_err(|_| storage())
    }
    pub fn set_state(&mut self, name: &str, value: &str) -> Result<(), NativeError> {
        self.admit()?;
        self.db.execute("INSERT INTO catalog_state(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[name,value]).map_err(|_|storage())
    }
    pub fn admit(&self) -> Result<(), NativeError> {
        paths::reject_symlinks(&self.path)?;
        let named = fs::symlink_metadata(&self.path)?;
        let held = self.database_file.metadata()?;
        if (named.dev(), named.ino()) != (held.dev(), held.ino()) || held.nlink() != 1 {
            return Err(storage());
        }
        if fs::metadata(&self.path)?.len() > DB_LIMIT
            || scalar(&self.db, "PRAGMA page_count")? > 65536
            || scalar(&self.db, "PRAGMA max_page_count")? != 65536
        {
            return Err(storage());
        }
        reserve(&self.root.join("cache"))?;
        let _ = self.accounting()?;
        Ok(())
    }
    pub fn accounting(&self) -> Result<JsonValue, NativeError> {
        let mut bytes = 0u64;
        let mut files = 0usize;
        let mut stack = vec![self.root.join("cache/catalog")];
        while let Some(p) = stack.pop() {
            for e in fs::read_dir(p)? {
                let e = e?;
                let m = fs::symlink_metadata(e.path())?;
                files += 1;
                if files > 16384 || m.file_type().is_symlink() {
                    return Err(storage());
                }
                if m.is_dir() {
                    stack.push(e.path());
                } else if m.is_file() {
                    bytes = bytes.checked_add(m.len()).ok_or_else(storage)?;
                } else {
                    return Err(storage());
                }
            }
        }
        reject_unknown_artifacts(&self.root.join("cache"))?;
        let mut sidecars = 0u64;
        for suffix in ["-journal", "-wal", "-shm"] {
            match fs::symlink_metadata(format!("{}{suffix}", self.path.display())) {
                Ok(m) => {
                    if !m.is_file() || m.nlink() != 1 {
                        return Err(storage());
                    }
                    sidecars += m.len();
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(storage()),
            }
        }
        Ok(
            object! {databaseBytes:fs::metadata(&self.path)?.len().to_string(),sidecarBytes:sidecars.to_string(),spoolAndControlBytes:bytes.to_string(),reserveBytes:"1073741824",aggregateQuota:false},
        )
    }
    pub fn apply_page(&mut self, page: ValidatedPage) -> Result<CatalogRevision, NativeError> {
        if page.records.len() > 50
            || page.states.len() > 4
            || page
                .states
                .iter()
                .any(|(k, v)| k.len() > 64 || v.len() > 16 * 1024)
            || page.cursor_name.len() > 64
            || !c::bounded(&page.cursor, 16 * 1024)
        {
            return Err(storage());
        }
        let bytes = page
            .records
            .iter()
            .map(|r| r.data.dump().len() + r.detail.as_ref().map_or(0, |d| d.dump().len()))
            .sum::<usize>();
        if bytes > 10 * 1024 * 1024 {
            return Err(c::error("catalogResponseTooLarge"));
        }
        self.admit()?;
        self.evict(bytes as u64)?;
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        for record in &page.records {
            if SubmissionKeyV1::parse(&record.data["submission"]).map_err(|_| storage())?
                != record.key
            {
                return Err(storage());
            }
            let unavailable=tx.db.query("SELECT 1 FROM visibility_status WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2",&[&record.key.submission_type,&record.key.submission_id]).map_err(|_|storage())?.is_some();
            if unavailable && record.detail.is_none() {
                if !page.snapshot.starts_with("pending:") {
                    tx.db
                        .execute(
                            "UPDATE submission SET snapshot=?2 WHERE slug=?1",
                            &[&record.key.slug(), &page.snapshot],
                        )
                        .map_err(|_| storage())?;
                }
                continue;
            }
            if record.detail.is_some() {
                // A fresh identity-bound public profile supersedes older staged
                // absence evidence for this identity during an overlapping crawl.
                tx.db
                    .execute(
                        "DELETE FROM tombstone_candidate WHERE slug=?1",
                        &[&record.key.slug()],
                    )
                    .map_err(|_| storage())?;
                tx.db.execute("DELETE FROM visibility_status WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2",&[&record.key.submission_type,&record.key.submission_id]).map_err(|_|storage())?;
            }
            let prior = tx
                .db
                .query(
                    "SELECT payload FROM submission WHERE slug=?1",
                    &[&record.key.slug()],
                )
                .map_err(|_| storage())?
                .map(|r| json::parse(&r[0]).map_err(|_| storage()))
                .transpose()?;
            if record.data["hydrated"] != true
                && prior.as_ref().is_some_and(|v| v["hydrated"] == true)
            {
                continue;
            }
            // Latest-page refreshes update payloads without stealing full-crawl
            // coverage markers (including when the row has not been seen yet).
            let snapshot = if page.snapshot.starts_with("fresh:") {
                tx.db
                    .query(
                        "SELECT snapshot FROM submission WHERE slug=?1",
                        &[&record.key.slug()],
                    )
                    .map_err(|_| storage())?
                    .map(|r| r[0].clone())
                    .unwrap_or_else(|| page.snapshot.clone())
            } else {
                page.snapshot.clone()
            };
            upsert(tx.db, record, &snapshot, now())?;
            if self.fts {
                index(tx.db, &record.key, &record.data)?;
            }
        }
        if scalar(tx.db, "SELECT count(*) FROM submission")? > MAX_SUMMARIES as u64 {
            return Err(c::error("catalogIncomplete"));
        }
        tx.db.execute("INSERT INTO catalog_state(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[&page.cursor_name,&page.cursor.dump()]).map_err(|_|storage())?;
        for (key, value) in &page.states {
            tx.db.execute("INSERT INTO catalog_state(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[key,value]).map_err(|_|storage())?;
        }
        tx.db
            .execute(
                "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                &[&revision.to_string()],
            )
            .map_err(|_| storage())?;
        if !self.fts {
            tx.db
                .execute(
                    "UPDATE catalog_state SET value='1' WHERE name='fts_dirty'",
                    &[],
                )
                .map_err(|_| storage())?;
            tx.db
                .execute(
                    "DELETE FROM catalog_state WHERE name='fts_rebuild_after'",
                    &[],
                )
                .map_err(|_| storage())?;
        }
        tx.commit().map_err(|_| storage())?;
        self.revision = revision;
        Ok(revision)
    }
    fn evict(&mut self, incoming: u64) -> Result<(), NativeError> {
        // Conservative page admission: retain summary/member coverage. Evict
        // bounded LRU payload rows first; no VACUUM or summary deletion.
        loop {
            let pages = scalar(&self.db, "PRAGMA page_count")?;
            let free = scalar(&self.db, "PRAGMA freelist_count")?;
            let available = (65536 - pages + free) * 4096;
            if available >= incoming.saturating_mul(3).saturating_add(1024 * 1024) {
                return Ok(());
            }
            self.admit()?;
            let candidates=self.db.query_rows("SELECT kind,identity FROM resource ORDER BY accessed_at,kind,identity LIMIT 50",&[],50,64*1024).map_err(|_|storage())?;
            if candidates.is_empty() {
                return Err(NativeError::new("catalogIncomplete","Catalog coverage cannot fit within the 256 MiB cache; prior coverage was retained."));
            }
            let tx = self.db.transaction().map_err(|_| storage())?;
            for row in candidates {
                tx.db
                    .execute(
                        &format!(
                            "DELETE FROM {} WHERE kind=?1 AND identity=?2",
                            resource_table(&row[0])?
                        ),
                        &[&row[0], &row[1]],
                    )
                    .map_err(|_| storage())?;
            }
            tx.commit().map_err(|_| storage())?;
            return Err(NativeError::new("catalogIncomplete","One bounded payload eviction batch completed; resume sync to continue publication with prior coverage retained."));
        }
    }
    pub fn get(&self, key: &SubmissionKeyV1) -> Result<Option<JsonValue>, NativeError> {
        if let Some(status) = self.unavailable(key)? {
            return Err(c::error(&status));
        }
        self.db.query("SELECT payload FROM submission WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2 AND tombstoned=0",&[&key.submission_type,&key.submission_id]).map_err(|_|storage())?.map(|r|json::parse(&r[0]).map_err(|_|storage())).transpose()
    }
    /// Cache reads never need write admission. Access-time writes are omitted;
    /// insertion/fetch time remains the conservative eviction age.
    pub fn resource(
        &self,
        kind: &str,
        identity: &str,
    ) -> Result<Option<super::details::CachedResource>, NativeError> {
        resource_table(kind)?;
        let key = SubmissionKeyV1::from_slug(identity.split(':').next().unwrap_or(""))?;
        if let Some(status) = self.unavailable(&key)? {
            return Err(c::error(&status));
        }
        self.db.query("SELECT payload,fetched_at,complete,source_pin FROM resource WHERE kind=?1 AND identity=?2",&[kind,identity]).map_err(|_|storage())?.map(|r|{
            let fetched=r[1].parse::<u64>().ok().filter(|n|*n<=253402300799).ok_or_else(storage)?;
            if !matches!(r[2].as_str(),"0"|"1")||r[3]!=SOURCE_PIN{return Err(storage());}
            let payload=super::provider::parse_bounded(r[0].as_bytes(),512*1024).map_err(|_|storage())?;
            Ok(super::details::CachedResource{payload,fetched,complete:r[2]=="1"})
        }).transpose()
    }
    pub fn summary_age(&self, key: &SubmissionKeyV1) -> Result<Option<u64>, NativeError> {
        self.db
            .query(
                "SELECT fetched_at FROM submission WHERE slug=?1 AND tombstoned=0",
                &[&key.slug()],
            )
            .map_err(|_| storage())?
            .map(|r| {
                r[0].parse::<u64>()
                    .ok()
                    .filter(|n| *n <= 253402300799)
                    .ok_or_else(storage)
            })
            .transpose()
    }
    pub fn publish_profile(&mut self, record: ProviderRecord) -> Result<(), NativeError> {
        // Preserve a full-crawl's seen snapshot; an independent detail read is
        // not index coverage and must not replace its reconciliation evidence.
        let snapshot = self
            .db
            .query(
                "SELECT snapshot FROM submission WHERE slug=?1",
                &[&record.key.slug()],
            )
            .map_err(|_| storage())?
            .map(|r| r[0].clone())
            .unwrap_or_else(|| "detail".into());
        self.apply_page(ValidatedPage {
            records: vec![record],
            cursor_name: "last_detail_read".into(),
            cursor: JsonValue::Null,
            snapshot,
            states: vec![],
        })?;
        Ok(())
    }
    /// Task3 calls only after identity-bound ProfilePage nonpublic response or
    /// authoritative HTTP404 at that exact request URL. Weak index/bulk data
    /// cannot clear this state or revive old cached detail.
    pub fn mark_unavailable(
        &mut self,
        key: &SubmissionKeyV1,
        status: &str,
    ) -> Result<u64, NativeError> {
        if !matches!(status, "catalogPrivate" | "catalogNotFound") {
            return Err(NativeError::params());
        }
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        tx.db.execute("INSERT INTO visibility_status(provider,submission_type,submission_id,status,checked_at) VALUES('gamebanana',?1,?2,?3,?4) ON CONFLICT(provider,submission_type,submission_id) DO UPDATE SET status=excluded.status,checked_at=excluded.checked_at",&[&key.submission_type,&key.submission_id,status,&now().to_string()]).map_err(|_|storage())?;
        tx.db
            .execute(
                "UPDATE submission SET tombstoned=1 WHERE slug=?1",
                &[&key.slug()],
            )
            .map_err(|_| storage())?;
        tx.db
            .execute(
                "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                &[&revision.to_string()],
            )
            .map_err(|_| storage())?;
        tx.commit().map_err(|_| storage())?;
        self.revision = revision;
        Ok(revision)
    }
    pub fn unavailable(&self, key: &SubmissionKeyV1) -> Result<Option<String>, NativeError> {
        Ok(self.db.query("SELECT status FROM visibility_status WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2",&[&key.submission_type,&key.submission_id]).map_err(|_|storage())?.map(|r|r[0].clone()))
    }
    pub fn unavailable_checked_at(&self, key: &SubmissionKeyV1) -> Result<u64, NativeError> {
        let row=self.db.query("SELECT checked_at FROM visibility_status WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2",&[&key.submission_type,&key.submission_id]).map_err(|_|storage())?.ok_or_else(storage)?;
        c::u64_text(&row[0].as_str().into()).map_err(|_| storage())
    }
    pub fn put_resource(
        &mut self,
        kind: &str,
        identity: &str,
        payload: &JsonValue,
        complete: bool,
    ) -> Result<u64, NativeError> {
        let table = resource_table(kind)?;
        let key = SubmissionKeyV1::from_slug(identity.split(':').next().unwrap_or(""))?;
        if let Some(status) = self.unavailable(&key)? {
            return Err(c::error(&status));
        }
        if identity.len() > 256 {
            return Err(NativeError::params());
        }
        if !c::bounded(payload, 512 * 1024) {
            return Err(c::error("catalogResponseTooLarge"));
        }
        self.admit()?;
        let effective_identity = if kind == "comments" && payload["state"] == "hidden" {
            format!("{}:hidden", key.slug())
        } else {
            identity.to_owned()
        };
        let identity = effective_identity.as_str();
        let bytes = payload.dump();
        self.evict(bytes.len() as u64)?;
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        if kind == "comments" {
            if payload["state"] == "hidden" {
                tx.db
                    .execute(
                        "DELETE FROM activity WHERE kind='comments' AND identity LIKE ?1",
                        &[&format!("{}:%", key.slug())],
                    )
                    .map_err(|_| storage())?;
            } else {
                tx.db
                    .execute(
                        "DELETE FROM activity WHERE kind='comments' AND identity=?1",
                        &[&format!("{}:hidden", key.slug())],
                    )
                    .map_err(|_| storage())?;
            }
        }
        tx.db.execute(&format!("INSERT INTO {table}(kind,identity,payload,fetched_at,accessed_at,complete,source_pin) VALUES(?1,?2,?3,?4,?4,?5,?6) ON CONFLICT(kind,identity) DO UPDATE SET payload=excluded.payload,fetched_at=excluded.fetched_at,accessed_at=excluded.accessed_at,complete=excluded.complete,source_pin=excluded.source_pin"),&[kind,identity,&bytes,&now().to_string(),if complete{"1"}else{"0"},SOURCE_PIN]).map_err(|_|storage())?;
        tx.db
            .execute(
                "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                &[&revision.to_string()],
            )
            .map_err(|_| storage())?;
        tx.commit().map_err(|_| storage())?;
        self.revision = revision;
        Ok(revision)
    }
    pub fn apply_collections(
        &mut self,
        records: &[JsonValue],
        snapshot: &str,
        cursor: &JsonValue,
        complete: bool,
    ) -> Result<u64, NativeError> {
        if records.len() > 50
            || records.iter().any(|r| !c::bounded(r, 32 * 1024))
            || !c::bounded(cursor, 4096)
        {
            return Err(storage());
        }
        self.admit()?;
        self.evict(records.iter().map(|r| r.dump().len() as u64).sum())?;
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        for r in records {
            let id = r["collectionId"].as_str().ok_or_else(storage)?;
            if tx
                .db
                .query(
                    "SELECT 1 FROM collection WHERE collection_id=?1 AND last_seen=?2",
                    &[id, snapshot],
                )
                .map_err(|_| storage())?
                .is_some()
            {
                return Err(super::sync::incomplete());
            }
            let mut payload = r.clone();
            payload["headerFetchedAt"] = now().into();
            payload["textFormat"] = "plain".into();
            if let Some(prior) = tx
                .db
                .query(
                    "SELECT payload FROM collection WHERE collection_id=?1",
                    &[id],
                )
                .map_err(|_| storage())?
            {
                let prior = super::provider::parse_bounded(prior[0].as_bytes(), 512 * 1024)
                    .map_err(|_| storage())?;
                for name in ["description", "descriptionFetchedAt", "descriptionRevision"] {
                    if prior.has_key(name) {
                        payload[name] = prior[name].clone();
                    }
                }
            }
            tx.db.execute("INSERT INTO collection(collection_id,payload,expected_count,remote_updated_at,last_seen) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(collection_id) DO UPDATE SET payload=excluded.payload,expected_count=excluded.expected_count,remote_updated_at=excluded.remote_updated_at,last_seen=excluded.last_seen",&[id,&payload.dump(),r["itemCount"].as_str().ok_or_else(storage)?,&r["remoteUpdatedAt"].as_u64().unwrap_or(0).to_string(),snapshot]).map_err(|_|storage())?;
        }
        if scalar(tx.db, "SELECT count(*) FROM collection")? > 12_500 {
            return Err(super::sync::incomplete());
        }
        tx.db.execute("INSERT INTO catalog_state(name,value) VALUES('collection_cursor',?1) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[&cursor.dump()]).map_err(|_|storage())?;
        if complete {
            tx.db.execute("INSERT INTO catalog_state(name,value) VALUES('last_collection_sync_at',?1) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[&now().to_string()]).map_err(|_|storage())?;
        }
        tx.db
            .execute(
                "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                &[&revision.to_string()],
            )
            .map_err(|_| storage())?;
        tx.commit().map_err(|_| storage())?;
        self.revision = revision;
        self.collection_projection.committed(records);
        Ok(revision)
    }
    /// Remember the original failure before attempting any further storage.
    pub fn remember_refresh_failure(&mut self, error: &NativeError, retry: Option<u32>) {
        let kind = if c::TERMINAL_ERRORS.contains(&error.kind.as_str()) {
            error.kind.as_str()
        } else if matches!(error.kind.as_str(), "nativeIo" | "nativeDatabase") {
            "catalogStorage"
        } else {
            "catalogTransport"
        };
        self.refresh_error = c::failure(kind, &error.message);
        self.refresh_error["retryAfterSeconds"] = retry.map(|n| n.min(86400)).into();
    }
    pub fn persist_refresh_failure(&mut self) -> Result<(), NativeError> {
        self.checkpoint(&[
            ("last_refresh_error", self.refresh_error.dump()),
            ("last_refresh_attempt_at", now().to_string()),
        ])
    }
    pub fn successful_refresh(&mut self) -> Result<(), NativeError> {
        self.checkpoint(&[
            ("last_refresh_error", "null".into()),
            ("last_refresh_attempt_at", now().to_string()),
            ("last_successful_sync_at", now().to_string()),
        ])?;
        self.refresh_error = JsonValue::Null;
        Ok(())
    }
    pub fn provenance(&self) -> Result<JsonValue, NativeError> {
        let updated = self.state_time("last_incremental_at")?;
        let successful = self.state_time("last_successful_sync_at")?;
        let cursor = self
            .state("sync_cursor")?
            .map(|raw| {
                super::provider::parse_bounded(raw.as_bytes(), 16 * 1024).map_err(|_| storage())
            })
            .transpose()?;
        let active = cursor.as_ref().is_some_and(|v| v["phase"] != "done");
        let progress = cursor
            .as_ref()
            .filter(|_| active)
            .map(|v| {
                super::sync::progress_value(
                    "catalog",
                    v["seen"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
                    v["total"].as_str().and_then(|s| s.parse().ok()),
                )
            })
            .unwrap_or(JsonValue::Null);
        let refresh_error = if self.refresh_error.is_null() {
            &self.search_error
        } else {
            &self.refresh_error
        };
        Ok(
            object! {syncProgress:progress,lastCompletedAt:self.state_time("last_full_sync_at")?,source:"cache",sourcePin:SOURCE_PIN,fetchedAt:updated,lastSuccessfulSyncAt:successful,stale:updated.is_none_or(|n|now().saturating_sub(n)>=super::sync::INCREMENTAL_SECONDS),incomplete:!self.state_flag("coverage_complete")?||active||!refresh_error.is_null(),refreshError:refresh_error.clone(),searchAvailable:self.search_available()?,searchState:if !self.fts||self.search_paused{"unavailable"}else if self.search_available()?{"available"}else{"rebuilding"},accounting:self.accounting()?},
        )
    }
}
fn index(db: &Database, key: &SubmissionKeyV1, data: &JsonValue) -> Result<(), NativeError> {
    index_with(db, key, data, false)
}
fn index_with(
    db: &Database,
    key: &SubmissionKeyV1,
    data: &JsonValue,
    maintenance: bool,
) -> Result<(), NativeError> {
    let execute = |sql: &str, params: &[&str]| -> Result<(), NativeError> {
        if maintenance {
            db.maintenance_execute(sql, params)
        } else {
            db.execute(sql, params).map_err(|_| storage())
        }
    };
    execute("DELETE FROM submission_fts WHERE provider='gamebanana' AND submission_type=?1 AND submission_id=?2",&[&key.submission_type,&key.submission_id])?;
    execute("INSERT INTO submission_fts(provider,submission_type,submission_id,name,author,description) VALUES('gamebanana',?1,?2,?3,?4,?5)",&[&key.submission_type,&key.submission_id,data["name"].as_str().ok_or_else(storage)?,data["author"].as_str().ok_or_else(storage)?,data["description"].as_str().ok_or_else(storage)?])
}

fn upsert(db: &Database, r: &ProviderRecord, snapshot: &str, time: u64) -> Result<(), NativeError> {
    let d = &r.data;
    let strs = [
        r.key.submission_type.clone(),
        r.key.submission_id.clone(),
        r.key.slug(),
        d["name"].as_str().ok_or_else(storage)?.into(),
        d["author"].as_str().ok_or_else(storage)?.into(),
        d["authorRemoteId"].as_str().unwrap_or("").into(),
        d["category"].as_str().ok_or_else(storage)?.into(),
        d["hero"].as_str().unwrap_or("").into(),
        if d["isAudio"] == true { "1" } else { "0" }.into(),
        if d["isMap"] == true { "1" } else { "0" }.into(),
        if d["isNsfw"] == true || d["visibilityKnown"] != true {
            "1"
        } else {
            "0"
        }
        .into(),
        if d["isObsolete"] == true { "1" } else { "0" }.into(),
        d["downloadCount"].as_str().ok_or_else(storage)?.into(),
        d["likes"].as_str().ok_or_else(storage)?.into(),
        d["remoteAddedAt"].as_u64().unwrap_or(0).to_string(),
        d["remoteUpdatedAt"].as_u64().unwrap_or(0).to_string(),
        d.dump(),
        snapshot.into(),
        time.to_string(),
    ];
    let params = strs.iter().map(String::as_str).collect::<Vec<_>>();
    db.execute("INSERT INTO submission(provider,submission_type,submission_id,slug,name,author,author_remote_id,category,hero,is_audio,is_map,is_nsfw,is_obsolete,download_count,likes,added_at,updated_at,payload,snapshot,fetched_at,tombstoned) VALUES('gamebanana',?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,0) ON CONFLICT(provider,submission_type,submission_id) DO UPDATE SET name=excluded.name,author=excluded.author,author_remote_id=excluded.author_remote_id,category=excluded.category,hero=excluded.hero,is_audio=excluded.is_audio,is_map=excluded.is_map,is_nsfw=excluded.is_nsfw,is_obsolete=excluded.is_obsolete,download_count=excluded.download_count,likes=excluded.likes,added_at=excluded.added_at,updated_at=excluded.updated_at,payload=excluded.payload,snapshot=excluded.snapshot,fetched_at=excluded.fetched_at,tombstoned=0",&params).map_err(|_|storage())?;
    if let Some(detail) = &r.detail {
        let bytes = detail.dump();
        db.execute("INSERT INTO detail(kind,identity,payload,fetched_at,accessed_at,complete,source_pin) VALUES('detail',?1,?2,?3,?3,?5,?4) ON CONFLICT(kind,identity) DO UPDATE SET payload=excluded.payload,fetched_at=excluded.fetched_at,accessed_at=excluded.accessed_at,complete=excluded.complete,source_pin=excluded.source_pin",&[&r.key.slug(),&bytes,&time.to_string(),SOURCE_PIN,if detail["linksTruncated"]==true{"0"}else{"1"}]).map_err(|_|storage())?;
    }
    Ok(())
}
pub(crate) fn scalar(db: &Database, sql: &str) -> Result<u64, NativeError> {
    db.query(sql, &[])
        .map_err(|_| storage())?
        .and_then(|r| r.first().and_then(|s| s.parse().ok()))
        .ok_or_else(storage)
}
const SCHEMA:&[&str]=&[
"CREATE TABLE catalog_state(name TEXT PRIMARY KEY,value TEXT NOT NULL)",
"INSERT INTO catalog_state VALUES('revision','0'),('fts_dirty','1'),('coverage_complete','0')",
"CREATE TABLE submission(provider TEXT NOT NULL CHECK(provider='gamebanana'),submission_type TEXT NOT NULL CHECK(submission_type IN ('mod','sound','wip')),submission_id TEXT NOT NULL,slug TEXT NOT NULL UNIQUE,name TEXT NOT NULL,author TEXT NOT NULL,author_remote_id TEXT NOT NULL,category TEXT NOT NULL,hero TEXT NOT NULL,is_audio INTEGER NOT NULL,is_map INTEGER NOT NULL,is_nsfw INTEGER NOT NULL,is_obsolete INTEGER NOT NULL,download_count INTEGER NOT NULL,likes INTEGER NOT NULL,added_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,payload TEXT NOT NULL,snapshot TEXT NOT NULL,fetched_at INTEGER NOT NULL,tombstoned INTEGER NOT NULL,PRIMARY KEY(provider,submission_type,submission_id))",
"CREATE INDEX submission_scope ON submission(submission_type,tombstoned,download_count)",
"CREATE TABLE detail(kind TEXT NOT NULL CHECK(kind='detail'),identity TEXT NOT NULL,payload TEXT NOT NULL,fetched_at INTEGER NOT NULL,accessed_at INTEGER NOT NULL,complete INTEGER NOT NULL,source_pin TEXT NOT NULL,PRIMARY KEY(kind,identity))",
"CREATE TABLE file(kind TEXT NOT NULL CHECK(kind='files'),identity TEXT NOT NULL,payload TEXT NOT NULL,fetched_at INTEGER NOT NULL,accessed_at INTEGER NOT NULL,complete INTEGER NOT NULL,source_pin TEXT NOT NULL,PRIMARY KEY(kind,identity))",
"CREATE TABLE activity(kind TEXT NOT NULL CHECK(kind IN ('comments','changelog')),identity TEXT NOT NULL,payload TEXT NOT NULL,fetched_at INTEGER NOT NULL,accessed_at INTEGER NOT NULL,complete INTEGER NOT NULL,source_pin TEXT NOT NULL,PRIMARY KEY(kind,identity))",
"CREATE VIEW resource AS SELECT * FROM detail UNION ALL SELECT * FROM file UNION ALL SELECT * FROM activity",
"CREATE INDEX detail_lru ON detail(accessed_at,identity)",
"CREATE INDEX file_lru ON file(accessed_at,identity)",
"CREATE INDEX activity_lru ON activity(accessed_at,kind,identity)",
"CREATE TABLE collection(collection_id TEXT PRIMARY KEY,payload TEXT NOT NULL,expected_count INTEGER NOT NULL,remote_updated_at INTEGER NOT NULL,items_synced_at INTEGER,items_count INTEGER,items_version INTEGER,last_seen TEXT NOT NULL)",
"CREATE TABLE collection_member(collection_id TEXT NOT NULL,position INTEGER NOT NULL,provider TEXT NOT NULL,submission_type TEXT NOT NULL,submission_id TEXT NOT NULL,PRIMARY KEY(collection_id,position),UNIQUE(collection_id,provider,submission_type,submission_id))",
"CREATE TABLE visibility_status(provider TEXT NOT NULL,submission_type TEXT NOT NULL,submission_id TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('catalogPrivate','catalogNotFound')),checked_at INTEGER NOT NULL,PRIMARY KEY(provider,submission_type,submission_id))",
"CREATE TABLE tombstone_candidate(snapshot TEXT NOT NULL,slug TEXT NOT NULL,status TEXT NOT NULL,PRIMARY KEY(snapshot,slug))",
"CREATE TABLE member_staging(crawl TEXT NOT NULL,collection_id TEXT NOT NULL,position INTEGER NOT NULL,provider TEXT NOT NULL,submission_type TEXT NOT NULL,submission_id TEXT NOT NULL,PRIMARY KEY(crawl,collection_id,position),UNIQUE(crawl,collection_id,provider,submission_type,submission_id))"
];

fn resource_table(kind: &str) -> Result<&'static str, NativeError> {
    match kind {
        "detail" => Ok("detail"),
        "files" => Ok("file"),
        "comments" | "changelog" => Ok("activity"),
        _ => Err(NativeError::params()),
    }
}

fn reject_unknown_artifacts(cache: &Path) -> Result<(), NativeError> {
    for (index, entry) in fs::read_dir(cache)?.enumerate() {
        if index >= 4096 {
            return Err(storage());
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(storage)?;
        if name.starts_with("catalog-v1.sqlite3")
            && !matches!(
                name,
                "catalog-v1.sqlite3"
                    | "catalog-v1.sqlite3-journal"
                    | "catalog-v1.sqlite3-wal"
                    | "catalog-v1.sqlite3-shm"
            )
        {
            let bounded = name.chars().take(96).collect::<String>();
            return Err(NativeError::new(
                "catalogStorage",
                format!("Preserved unknown database artifact under root/cache: {bounded}"),
            ));
        }
    }
    Ok(())
}

fn validate_refresh_error(value: &JsonValue) -> Result<(), NativeError> {
    if value.is_null() {
        return Ok(());
    }
    c::exact(value, &["kind", "message", "retryAfterSeconds"]).map_err(|_| storage())?;
    if !value["kind"]
        .as_str()
        .is_some_and(|k| c::TERMINAL_ERRORS.contains(&k))
        || !value["message"]
            .as_str()
            .is_some_and(|s| s.len() <= 256 && !s.chars().any(char::is_control))
        || !value["retryAfterSeconds"].is_null()
            && value["retryAfterSeconds"]
                .as_u32()
                .is_none_or(|n| n > 86400)
    {
        return Err(storage());
    }
    Ok(())
}

#[cfg(test)]
mod backlog_tests {
    use super::*;
    #[test]
    fn settings_clear_catalog_resets_cache_without_library_or_preferences() {
        let root = std::env::temp_dir().join(format!("catalog-clear-{}",c::new_token().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(root.join("cache/catalog")).unwrap();
        let library = root.join("library-marker");
        std::fs::write(&library,b"keep library").unwrap();
        let mut store=CatalogStore::open(&root).unwrap();
        store.set_state("sync_cursor","old cursor").unwrap();
        store.db.execute("INSERT INTO visibility_status VALUES('gamebanana','mod','123','catalogPrivate',1)",&[]).unwrap();
        store.db.execute("INSERT INTO visibility_status VALUES('gamebanana','sound','456','catalogNotFound',2)",&[]).unwrap();
        store.db.execute("INSERT INTO detail VALUES('detail','old','{}',1,1,1,'pin')",&[]).unwrap();
        let preferences=root.join("catalog-preferences-v1.json");
        std::fs::write(&preferences,b"keep favorites").unwrap();
        let revision=store.revision;
        store.clear_cache().unwrap();
        assert!(store.state("sync_cursor").unwrap().is_none());
        assert_eq!(store.unavailable(&super::super::provider::SubmissionKeyV1{submission_type:"mod".into(),submission_id:"123".into()}).unwrap().as_deref(),Some("catalogPrivate"));
        assert_eq!(scalar(&store.db,"SELECT count(*) FROM visibility_status").unwrap(),2);
        assert_eq!(store.unavailable(&super::super::provider::SubmissionKeyV1{submission_type:"sound".into(),submission_id:"456".into()}).unwrap().as_deref(),Some("catalogNotFound"));
        assert_eq!(scalar(&store.db,"SELECT count(*) FROM detail").unwrap(),0);
        assert_eq!(store.revision,revision+1);
        assert!(!store.state_flag("coverage_complete").unwrap());
        assert_eq!(std::fs::read(&library).unwrap(),b"keep library");
        assert_eq!(std::fs::read(&preferences).unwrap(),b"keep favorites");
        let sync=super::super::sync::SyncState::open(&mut store,true,true).unwrap();
        assert_eq!(sync.cursor.page,1);
        assert_eq!(sync.cursor.phase,"index");
        assert!(!sync.throttled);
        drop(store);
        let reopened=CatalogStore::open(&root).unwrap();
        assert_eq!(reopened.revision,revision+1);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
