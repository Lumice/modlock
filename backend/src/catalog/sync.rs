//! Adapted 2026-10-28 from pinned upstream catalog/sync.rs (GPL-3.0).
//! Cooperative state machine: one provider request/page publication per step.
use super::{
    contract as c,
    provider::{ProviderDocument, ProviderRead, ProviderRecord, SubmissionKeyV1},
    store::{now, storage, CatalogStore, ValidatedPage},
};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::collections::{BTreeSet, VecDeque};
pub const INCREMENTAL_SECONDS: u64 = 20 * 60;
pub const FULL_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const COLLECTION_SECONDS: u64 = 6 * 60 * 60;
pub const MEMBER_SECONDS: u64 = 7 * 24 * 60 * 60;
const TYPES: [&str; 3] = ["mod", "sound", "wip"];
#[derive(Clone)]
pub struct SyncCursor {
    pub full: bool,
    pub snapshot: String,
    pub started: u64,
    pub type_index: usize,
    pub page: u32,
    pub seen: u64,
    pub total: Option<u64>,
    pub high: u64,
    pub newest: u64,
    pub phase: String,
}
impl SyncCursor {
    fn json(&self) -> JsonValue {
        object! {version:1,full:self.full,snapshot:self.snapshot.clone(),startedAt:self.started,typeIndex:self.type_index,page:self.page,seen:self.seen.to_string(),total:self.total.map(|n|n.to_string()),highWater:self.high,newest:self.newest,phase:self.phase.clone()}
    }
    fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        let cursor = Self::decode(v)?;
        cursor.validate()?;
        Ok(cursor)
    }
    // Prepared successors receive the next type's stored high-water in publish;
    // only that internal path decodes before enforcing final relationships.
    fn decode(v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(
            v,
            &[
                "version",
                "full",
                "snapshot",
                "startedAt",
                "typeIndex",
                "page",
                "seen",
                "total",
                "highWater",
                "newest",
                "phase",
            ],
        )
        .map_err(|_| storage())?;
        if v["version"] != 1 || !c::typed(v) {
            return Err(storage());
        }
        let t = v["typeIndex"]
            .as_usize()
            .filter(|n| *n <= 3)
            .ok_or_else(storage)?;
        let page = v["page"]
            .as_u32()
            .filter(|n| (1..=250).contains(n))
            .ok_or_else(storage)?;
        let phase = v["phase"]
            .as_str()
            .filter(|s| matches!(*s, "index" | "confirm" | "retire" | "done"))
            .ok_or_else(storage)?;
        let cursor = Self {
            full: v["full"].as_bool().ok_or_else(storage)?,
            snapshot: c::token(&v["snapshot"]).map_err(|_| storage())?.into(),
            started: v["startedAt"].as_u64().ok_or_else(storage)?,
            type_index: t,
            page,
            seen: c::u64_text(&v["seen"]).map_err(|_| storage())?,
            total: if v["total"].is_null() {
                None
            } else {
                Some(c::u64_text(&v["total"]).map_err(|_| storage())?)
            },
            high: v["highWater"].as_u64().ok_or_else(storage)?,
            newest: v["newest"].as_u64().ok_or_else(storage)?,
            phase: phase.into(),
        };
        Ok(cursor)
    }
    fn validate(&self) -> Result<(), NativeError> {
        if self.started > 253402300799
            || self.high > 253402300799
            || self.newest > 253402300799
            || self.newest < self.high
        {
            return Err(storage());
        }
        match self.phase.as_str() {
            "index" => {
                if self.type_index >= TYPES.len()
                    || self.seen < (self.page - 1) as u64
                    || self.seen > 50 * (self.page - 1) as u64
                {
                    return Err(storage());
                }
                if self.page == 1 && (self.seen != 0 || self.newest != self.high)
                    || self.page > 1 && self.total.is_none()
                {
                    return Err(storage());
                }
            }
            "confirm" | "retire" | "done" => {
                if self.type_index != TYPES.len()
                    || self.page != 1
                    || self.seen != 0
                    || self.total.is_some()
                    || matches!(self.phase.as_str(), "confirm" | "retire") && !self.full
                {
                    return Err(storage());
                }
            }
            _ => return Err(storage()),
        }
        Ok(())
    }
}
pub struct SyncState {
    pub cursor: SyncCursor,
    pending: Option<(ProviderRead, Vec<ProviderRecord>, u64, bool)>,
    pub summary_pending: bool,
    hydrate: usize,
    pub publication: Option<ValidatedPage>,
    pub waiting: Option<ProviderRead>,
    confirm: VecDeque<SubmissionKeyV1>,
    pub throttled: bool,
    pub progress: JsonValue,
    confirm_total: Option<[u64; 3]>,
    confirm_completed: [u64; 3],
    confirm_targets: BTreeSet<SubmissionKeyV1>,
    confirmed_keys: BTreeSet<SubmissionKeyV1>,
    retire_total: Option<[u64; 3]>,
    retire_completed: [u64; 3],
    saved_full: Option<SyncCursor>,
    pages_published: usize,
    pub yielded: bool,
}
impl SyncState {
    pub fn open(
        store: &mut CatalogStore,
        force_refresh: bool,
        force_reconcile: bool,
    ) -> Result<Self, NativeError> {
        let time = now();
        let active = store
            .state("sync_cursor")?
            .map(|s| {
                if !crate::protocol::integer_numbers(s.as_bytes()) {
                    return Err(storage());
                }
                super::provider::parse_bounded(s.as_bytes(), 16 * 1024).map_err(|_| storage())
            })
            .transpose()?
            .map(|v| SyncCursor::parse(&v))
            .transpose()?;
        let full = force_reconcile
            || store
                .state_time("last_full_sync_at")?
                .is_none_or(|n| time.saturating_sub(n) >= FULL_SECONDS)
            || !store.state_flag("coverage_complete")?;
        let resume = active.filter(|c| c.phase != "done" && !force_reconcile);
        let throttled = resume.is_none()
            && !force_refresh
            && !force_reconcile
            && !full
            && store
                .state_time("last_incremental_at")?
                .is_some_and(|n| time.saturating_sub(n) < INCREMENTAL_SECONDS);
        let cursor = if let Some(cursor) = resume {
            cursor
        } else {
            let high = high_water(store, TYPES[0])?;
            SyncCursor {
                full,
                snapshot: c::new_token()?,
                started: time,
                type_index: 0,
                page: 1,
                seen: 0,
                total: None,
                high,
                newest: high,
                phase: if throttled { "done" } else { "index" }.into(),
            }
        };
        // Latest metadata is independent of full coverage. Never overwrite the
        // durable full cursor while refreshing the newest pages.
        let saved_full = cursor.full.then(|| cursor.clone());
        let cursor = if saved_full.is_some() {
            SyncCursor {
                full: false,
                snapshot: c::new_token()?,
                started: time,
                type_index: 0,
                page: 1,
                seen: 0,
                total: None,
                high: high_water(store, TYPES[0])?,
                newest: high_water(store, TYPES[0])?,
                phase: "index".into(),
            }
        } else {
            cursor
        };
        let progress = if cursor.phase == "done" {
            JsonValue::Null
        } else {
            progress_value(
                &format!(
                    "{}-{}",
                    cursor.phase,
                    TYPES[if cursor.phase == "index" {
                        cursor.type_index
                    } else {
                        0
                    }]
                ),
                if cursor.phase == "index" {
                    cursor.seen
                } else {
                    0
                },
                if cursor.phase == "index" {
                    cursor.total
                } else {
                    None
                },
            )
        };
        Ok(Self {
            cursor,
            pending: None,
            summary_pending: false,
            hydrate: 0,
            publication: None,
            waiting: None,
            confirm: VecDeque::new(),
            throttled,
            progress,
            confirm_total: None,
            confirm_completed: [0; 3],
            confirm_targets: BTreeSet::new(),
            confirmed_keys: BTreeSet::new(),
            retire_total: None,
            retire_completed: [0; 3],
            saved_full,
            pages_published: 0,
            yielded: false,
        })
    }
    pub fn next(&mut self) -> Option<ProviderRead> {
        if self.waiting.is_some()
            || self.publication.is_some()
            || self.summary_pending
            || self.cursor.phase == "done"
            || self.cursor.phase == "retire"
            || self.cursor.full && self.pages_published >= 12 && self.cursor.phase == "index"
        {
            return None;
        }
        let read = if let Some((_, records, _, _)) = &self.pending {
            {
                let mut count = (records.len() - self.hydrate).min(40);
                loop {
                    let read = ProviderRead::Hydrate {
                        keys: records[self.hydrate..self.hydrate + count]
                            .iter()
                            .map(|r| r.key.clone())
                            .collect(),
                    };
                    if super::provider::Provider::request(read.clone()).is_ok() {
                        break read;
                    }
                    count = count.checked_sub(1)?;
                    if count == 0 {
                        return None;
                    }
                }
            }
        } else if self.cursor.phase == "confirm" {
            ProviderRead::Profile(self.confirm.front()?.clone())
        } else {
            ProviderRead::Index {
                submission_type: TYPES.get(self.cursor.type_index)?.to_string(),
                page: self.cursor.page,
                latest: !self.cursor.full,
            }
        };
        self.progress = match &read {
            ProviderRead::Index {
                submission_type, ..
            } => progress_value(
                &format!("index-{submission_type}"),
                self.cursor.seen,
                self.cursor.total,
            ),
            ProviderRead::Hydrate { keys } => progress_value(
                &format!("hydrate-{}", keys[0].submission_type),
                self.hydrate as u64,
                self.pending.as_ref().map(|p| p.1.len() as u64),
            ),
            ProviderRead::Profile(key) => {
                let i = type_index(&key.submission_type)?;
                progress_value(
                    &format!("confirm-{}", key.submission_type),
                    self.confirm_completed[i],
                    self.confirm_total.map(|totals| totals[i]),
                )
            }
            _ => return None,
        };
        self.waiting = Some(read.clone());
        Some(read)
    }
    pub fn pending_local_transition(&self) -> Result<(), NativeError> {
        if self.waiting.is_some() {
            return Err(storage());
        }
        // Publishing the final incremental page does not also commit its final
        // timestamp checkpoint. The next bounded local step must run that work.
        if self.cursor.phase == "done" && !self.throttled {
            self.cursor.validate()?;
            return Ok(());
        }
        if self.cursor.full && self.pages_published >= 12 && self.cursor.phase == "index" {
            return Ok(());
        }
        if self.summary_pending
            || self.publication.is_some()
            || self.cursor.phase == "retire"
            || self.cursor.phase == "confirm" && self.confirm.is_empty()
        {
            return Ok(());
        }
        Err(NativeError::new(
            "catalogStorage",
            "Catalog sync has no valid network read or local transition; its cursor was preserved.",
        ))
    }
    pub fn accept(&mut self, doc: ProviderDocument) -> Result<(), NativeError> {
        let read = self.waiting.take().ok_or_else(storage)?;
        match (read, doc) {
            (
                expected,
                ProviderDocument::Page {
                    read,
                    records,
                    total,
                    complete,
                },
            ) if expected == read => {
                // Offset pages describe a live index, not an immutable snapshot.
                // Counts can grow/shrink between requests and across resumes.
                if records.is_empty() && !complete {
                    return Err(NativeError::new(
                        "catalogIncomplete",
                        "GameBanana returned an empty page. Try refreshing again.",
                    ));
                }
                self.cursor.total = Some(total);
                if self.cursor.page == 250 && !complete {
                    return Err(incomplete());
                }
                self.progress = progress_value(
                    &format!("index-{}", TYPES[self.cursor.type_index]),
                    self.cursor.seen,
                    Some(total),
                );
                self.pending = Some((read, records, total, complete));
                self.summary_pending = true;
                self.hydrate = 0;
                self.prepare_publication()?;
            }
            (
                ProviderRead::Hydrate { keys: expected },
                ProviderDocument::Hydration { keys, values },
            ) if expected == keys && keys.len() == values.len() => {
                let (_, records, _, _) = self.pending.as_mut().ok_or_else(storage)?;
                for (key, value) in keys.iter().zip(values) {
                    let record = records
                        .get_mut(self.hydrate)
                        .filter(|r| &r.key == key)
                        .ok_or_else(storage)?;
                    for (k, v) in value.entries() {
                        record.data[k] = v.clone();
                    }
                    record.data["hydrated"] = true.into();
                    record.data["isMap"] =
                        (key.submission_type == "mod" && record.data["category"] == "Maps").into();
                    self.hydrate += 1;
                }
                if records.iter().map(|r| r.data.dump().len()).sum::<usize>() > 5 * 1024 * 1024 {
                    return Err(incomplete());
                }
                self.progress = progress_value(
                    &format!("hydrate-{}", keys[0].submission_type),
                    self.hydrate as u64,
                    Some(records.len() as u64),
                );
                self.prepare_publication()?;
            }
            (ProviderRead::Profile(key), ProviderDocument::Profile(record))
                if key == record.key =>
            {
                if self.cursor.phase == "confirm" {
                    if self.confirm.pop_front().as_ref() != Some(&key) {
                        return Err(storage());
                    }
                    self.publication = Some(ValidatedPage {
                        records: vec![record],
                        cursor_name: "sync_cursor".into(),
                        cursor: self.cursor.json(),
                        snapshot: self.cursor.snapshot.clone(),
                        states: Vec::new(),
                    });
                } else {
                    let (_, records, _, _) = self.pending.as_mut().ok_or_else(storage)?;
                    if records.get(self.hydrate).is_none_or(|r| r.key != key) {
                        return Err(storage());
                    }
                    records[self.hydrate] = record;
                    self.hydrate += 1;
                    if records
                        .iter()
                        .map(|r| {
                            r.data.dump().len() + r.detail.as_ref().map_or(0, |d| d.dump().len())
                        })
                        .sum::<usize>()
                        > 10 * 1024 * 1024
                    {
                        return Err(incomplete());
                    }
                    self.prepare_publication()?;
                }
            }
            _ => return Err(super::provider::invalid()),
        }
        Ok(())
    }
    fn prepare_publication(&mut self) -> Result<(), NativeError> {
        let Some((_, records, _, complete)) = &self.pending else {
            return Ok(());
        };
        if self.hydrate < records.len() {
            return Ok(());
        }
        let mut next = self.cursor.clone();
        next.seen = next
            .seen
            .checked_add(records.len() as u64)
            .ok_or_else(incomplete)?;
        let crossed = !next.full
            && next.high > 0
            && records.iter().any(|r| {
                r.data["remoteUpdatedAt"]
                    .as_u64()
                    .is_some_and(|n| n < next.high.saturating_sub(3600))
            });
        next.newest = records
            .iter()
            .filter_map(|r| r.data["remoteUpdatedAt"].as_u64())
            .max()
            .unwrap_or(next.newest)
            .max(next.newest);
        if *complete || crossed || self.saved_full.is_some() && next.page >= 2 {
            next.type_index += 1;
            next.page = 1;
            next.seen = 0;
            next.total = None;
            if next.type_index == 3 {
                next.phase = if next.full { "confirm" } else { "done" }.into();
            }
        } else {
            next.page = next
                .page
                .checked_add(1)
                .filter(|n| *n <= 250)
                .ok_or_else(incomplete)?;
        }
        self.publication = Some(ValidatedPage {
            records: records.clone(),
            cursor_name: "sync_cursor".into(),
            cursor: next.json(),
            snapshot: next.snapshot.clone(),
            states: Vec::new(),
        });
        Ok(())
    }
    pub fn publish(&mut self, store: &mut CatalogStore) -> Result<(), NativeError> {
        let Some(mut page) = self.publication.take() else {
            return Ok(());
        };
        let previous = self.cursor.clone();
        let mut next = SyncCursor::decode(&page.cursor)?;
        // Repeated identities across offset pages are normal after insertions.
        // Upserts are idempotent; absence still requires explicit confirmation.
        if self.saved_full.is_some() {
            page.cursor_name = "latest_cursor".into();
            page.snapshot = format!("fresh:{}", previous.snapshot);
        }
        if next.type_index != previous.type_index && next.type_index < 3 {
            next.high = high_water(store, TYPES[next.type_index])?;
            next.newest = next.high;
        }
        next.validate()?;
        page.cursor = next.json();
        // High-water changes only after every hydrated page of that type. A
        // resumed page therefore reuses the original cutoff, including overlap.
        let ended_type = previous.phase == "index" && next.type_index != previous.type_index;
        let newest = if ended_type {
            self.pending
                .as_ref()
                .map(|(_, rs, _, _)| {
                    rs.iter()
                        .filter_map(|r| r.data["remoteUpdatedAt"].as_u64())
                        .max()
                        .unwrap_or(previous.newest)
                        .max(previous.newest)
                })
                .unwrap_or(previous.newest)
        } else {
            0
        };
        if ended_type && (self.saved_full.is_none() || self.pending.as_ref().is_some_and(|p| p.3)) {
            page.states.push((
                format!("high_{}", TYPES[previous.type_index]),
                newest.to_string(),
            ));
        }
        let confirmation_keys = if previous.phase == "confirm" {
            let keys = page
                .records
                .iter()
                .map(|r| r.key.clone())
                .collect::<Vec<_>>();
            for key in &keys {
                self.confirmation_type(key)?;
            }
            keys
        } else {
            Vec::new()
        };
        let published_count = page.records.len() as u64;
        store.apply_page(page)?;
        if previous.phase == "confirm" {
            for key in confirmation_keys {
                self.record_confirmation(&key)?;
            }
        } else if previous.phase == "index" {
            self.progress = progress_value(
                &format!("index-{}", TYPES[previous.type_index]),
                previous.seen + published_count,
                previous.total,
            );
        }
        self.cursor = next;
        if previous.full {
            self.pages_published += 1;
        }
        self.pending = None;
        Ok(())
    }
    pub fn local_step(&mut self, store: &mut CatalogStore) -> Result<bool, NativeError> {
        if self.summary_pending {
            let records = self.pending.as_ref().ok_or_else(storage)?.1.clone();
            // Early summaries carry partial provenance and do not mark snapshot
            // coverage or advance the durable cursor. Existing hydrated rows win.
            let published = self.cursor.seen + records.len() as u64;
            store.apply_page(ValidatedPage {
                records,
                cursor_name: if self.saved_full.is_some() {
                    "latest_cursor"
                } else {
                    "sync_cursor"
                }
                .into(),
                cursor: self.cursor.json(),
                snapshot: format!("pending:{}", self.cursor.snapshot),
                states: Vec::new(),
            })?;
            self.progress = progress_value(
                &format!("index-{}", TYPES[self.cursor.type_index]),
                published,
                self.cursor.total,
            );
            self.summary_pending = false;
            return Ok(false);
        }
        if self.publication.is_some() {
            self.publish(store)?;
            return Ok(false);
        }
        if self.cursor.phase == "done" && self.saved_full.is_some() {
            store.set_state("last_incremental_at", &now().to_string())?;
            self.cursor = self.saved_full.take().ok_or_else(storage)?;
            store.set_state("sync_cursor", &self.cursor.json().dump())?;
            return Ok(false);
        }
        // Yield only between atomic pages. The next refresh starts with latest
        // updates and then resumes exactly this durable coverage cursor.
        if self.cursor.full && self.pages_published >= 12 && self.cursor.phase == "index" {
            self.yielded = true;
            self.progress = progress_value(
                &format!("index-{}", TYPES[self.cursor.type_index]),
                self.cursor.seen,
                self.cursor.total,
            );
            return Ok(true);
        }
        if self.cursor.phase == "confirm" && self.confirm.is_empty() {
            // Check every type's bound before any destructive retirement.
            for t in TYPES {
                let row=store.db.query("SELECT count(*) FROM submission WHERE submission_type=?1 AND snapshot!=?2 AND tombstoned=0",&[t,&self.cursor.snapshot]).map_err(|_|storage())?.ok_or_else(storage)?;
                if row[0].parse::<u64>().map_err(|_| storage())? > 400 {
                    return Err(incomplete());
                }
            }
            // Public foreground proof can reopen a counted candidate without
            // changing its index-seen snapshot. Reconcile actual eligible keys
            // before a refill; count unique identities, never repeat attempts.
            let rows=store.db.query_rows("SELECT submission_type,submission_id FROM submission WHERE snapshot!=?1 AND tombstoned=0 AND slug NOT IN (SELECT slug FROM tombstone_candidate WHERE snapshot=?1) ORDER BY submission_type,slug LIMIT 1201",&[&self.cursor.snapshot],1200,64*1024)?;
            let eligible=rows.into_iter().map(|r|SubmissionKeyV1::parse(&object!{provider:"gamebanana",submissionType:r[0].clone(),submissionId:r[1].clone()}).map_err(|_|storage())).collect::<Result<Vec<_>,_>>()?;
            self.reconcile_confirmations(&eligible)?;
            self.confirm.extend(eligible.into_iter().take(40));
            if self.confirm.is_empty() {
                self.cursor.phase = "retire".into();
                store.set_state("sync_cursor", &self.cursor.json().dump())?;
            }
            return Ok(false);
        }
        if self.cursor.phase == "retire" {
            if self.retire_total.is_none() {
                let mut totals = [0; 3];
                for (i, t) in TYPES.iter().enumerate() {
                    let row=store.db.query("SELECT count(*) FROM tombstone_candidate c JOIN submission s ON s.slug=c.slug WHERE c.snapshot=?1 AND s.submission_type=?2",&[&self.cursor.snapshot,t]).map_err(|_|storage())?.ok_or_else(storage)?;
                    totals[i] = row[0].parse().map_err(|_| storage())?;
                }
                self.retire_total = Some(totals);
                self.progress = progress_value("retire-mod", 0, Some(totals[0]));
            }
            let rows = store
                .db
                .query_rows(
                    "SELECT c.slug,c.status,s.submission_type FROM tombstone_candidate c JOIN submission s ON s.slug=c.slug WHERE c.snapshot=?1 ORDER BY s.submission_type,c.slug LIMIT 40",
                    &[&self.cursor.snapshot],
                    40,
                    4096,
                )
                .map_err(|_| storage())?;
            if !rows.is_empty() {
                store.admit()?;
                let revision = store.revision.checked_add(1).ok_or_else(storage)?;
                let tx = store.db.transaction().map_err(|_| storage())?;
                let retired_types = rows.iter().map(|r| r[2].clone()).collect::<Vec<_>>();
                for r in rows {
                    tx.db.execute("INSERT INTO visibility_status(provider,submission_type,submission_id,status,checked_at) SELECT provider,submission_type,submission_id,?2,?3 FROM submission WHERE slug=?1 ON CONFLICT(provider,submission_type,submission_id) DO UPDATE SET status=excluded.status,checked_at=excluded.checked_at",&[&r[0],&r[1],&now().to_string()]).map_err(|_|storage())?;
                    tx.db
                        .execute(
                            "UPDATE submission SET tombstoned=1 WHERE slug=?1 AND snapshot!=?2",
                            &[&r[0], &self.cursor.snapshot],
                        )
                        .map_err(|_| storage())?;
                    tx.db
                        .execute(
                            "DELETE FROM tombstone_candidate WHERE snapshot=?1 AND slug=?2",
                            &[&self.cursor.snapshot, &r[0]],
                        )
                        .map_err(|_| storage())?;
                }
                tx.db
                    .execute(
                        "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                        &[&revision.to_string()],
                    )
                    .map_err(|_| storage())?;
                tx.commit().map_err(|_| storage())?;
                store.revision = revision;
                for t in retired_types {
                    let i = type_index(&t).ok_or_else(storage)?;
                    self.retire_completed[i] += 1;
                    self.progress = progress_value(
                        &format!("retire-{t}"),
                        self.retire_completed[i],
                        self.retire_total.map(|totals| totals[i]),
                    );
                }
                return Ok(false);
            }
            self.cursor.phase = "done".into();
        }
        if self.cursor.phase == "done" && !self.throttled {
            let mut states = vec![
                ("sync_cursor", self.cursor.json().dump()),
                ("last_incremental_at", now().to_string()),
            ];
            if self.cursor.full {
                states.push(("last_full_sync_at", now().to_string()));
                states.push(("coverage_complete", "1".into()));
            }
            store.checkpoint(&states)?;
            return Ok(true);
        }
        Ok(self.throttled)
    }
    pub fn confirm_absent(
        &mut self,
        store: &mut CatalogStore,
        key: &SubmissionKeyV1,
        status: &str,
    ) -> Result<(), NativeError> {
        if !matches!(status, "catalogPrivate" | "catalogNotFound")
            || self.cursor.phase != "confirm"
            || self.waiting.as_ref() != Some(&ProviderRead::Profile(key.clone()))
            || self.confirm.front() != Some(key)
        {
            return Err(incomplete());
        }
        self.confirmation_type(key)?;
        store.admit()?;
        store
            .db
            .execute(
                "INSERT OR IGNORE INTO tombstone_candidate(snapshot,slug,status) VALUES(?1,?2,?3)",
                &[&self.cursor.snapshot, &key.slug(), status],
            )
            .map_err(|_| storage())?;
        self.record_confirmation(key)?;
        self.waiting = None;
        self.confirm.pop_front();
        Ok(())
    }
    fn reconcile_confirmations(&mut self, eligible: &[SubmissionKeyV1]) -> Result<(), NativeError> {
        let mut totals = self.confirm_total.unwrap_or([0; 3]);
        for key in eligible {
            if !self.confirm_targets.contains(key) {
                let i = type_index(&key.submission_type).ok_or_else(storage)?;
                if totals[i] >= 400 {
                    return Err(incomplete());
                }
                self.confirm_targets.insert(key.clone());
                totals[i] += 1;
            }
        }
        self.confirm_total = Some(totals);
        let t = self.progress["stage"]
            .as_str()
            .and_then(|s| s.strip_prefix("confirm-"))
            .filter(|t| TYPES.contains(t))
            .unwrap_or("mod");
        let i = type_index(t).ok_or_else(storage)?;
        self.progress = progress_value(
            &format!("confirm-{t}"),
            self.confirm_completed[i],
            Some(totals[i]),
        );
        Ok(())
    }
    fn confirmation_type(&self, key: &SubmissionKeyV1) -> Result<usize, NativeError> {
        if self.confirm_total.is_none() || !self.confirm_targets.contains(key) {
            return Err(storage());
        }
        type_index(&key.submission_type).ok_or_else(storage)
    }
    fn record_confirmation(&mut self, key: &SubmissionKeyV1) -> Result<(), NativeError> {
        let i = self.confirmation_type(key)?;
        if !self.confirmed_keys.contains(key) {
            let next = self.confirm_completed[i]
                .checked_add(1)
                .ok_or_else(storage)?;
            if next > self.confirm_total.ok_or_else(storage)?[i] {
                return Err(storage());
            }
            self.confirmed_keys.insert(key.clone());
            self.confirm_completed[i] = next;
        }
        self.progress = progress_value(
            &format!("confirm-{}", key.submission_type),
            self.confirm_completed[i],
            self.confirm_total.map(|totals| totals[i]),
        );
        Ok(())
    }
    pub fn result(&self) -> JsonValue {
        object! {mode:if self.throttled{"throttled"}else if self.cursor.full{"full"}else{"incremental"},snapshot:self.cursor.snapshot.clone(),complete:self.cursor.phase=="done",startedAt:self.cursor.started,memberMaxAgeSeconds:MEMBER_SECONDS}
    }
}
fn high_water(store: &CatalogStore, t: &str) -> Result<u64, NativeError> {
    store
        .state_time(&format!("high_{t}"))
        .map(|n| n.unwrap_or(0))
}
pub fn incomplete() -> NativeError {
    NativeError::new(
        "catalogIncomplete",
        "Catalog refresh paused. Try again to continue.",
    )
}

/// Collection headers have an independent six-hour cursor. Ordered member
/// publication belongs to Task3 and keeps the old completed member set meanwhile.
pub struct CollectionSync {
    pub page: u32,
    pub seen: u64,
    pub total: Option<u64>,
    pub snapshot: String,
    pub complete: bool,
    pub fresh: bool,
}
impl CollectionSync {
    pub fn open(store: &CatalogStore, force: bool) -> Result<Self, NativeError> {
        let fresh = !force
            && store
                .state_time("last_collection_sync_at")?
                .is_some_and(|n| now().saturating_sub(n) < COLLECTION_SECONDS);
        let mut state = Self {
            page: 1,
            seen: 0,
            total: None,
            snapshot: c::new_token()?,
            complete: fresh,
            fresh,
        };
        if let Some(raw) = store.state("collection_cursor")? {
            let saved = Self::parse_cursor(&raw)?;
            if !fresh && !saved.complete {
                state = saved;
            }
        }
        Ok(state)
    }
    pub(crate) fn parse_cursor(raw: &str) -> Result<Self, NativeError> {
        if !crate::protocol::integer_numbers(raw.as_bytes()) {
            return Err(storage());
        }
        let v = super::provider::parse_bounded(raw.as_bytes(), 16 * 1024).map_err(|_| storage())?;
        c::exact(&v, &["page", "seen", "total", "snapshot", "complete"]).map_err(|_| storage())?;
        let page = v["page"]
            .as_u32()
            .filter(|n| (1..=250).contains(n))
            .ok_or_else(storage)?;
        let seen = c::u64_text(&v["seen"]).map_err(|_| storage())?;
        let total = c::u64_text(&v["total"]).map_err(|_| storage())?;
        let complete = v["complete"].as_bool().ok_or_else(storage)?;
        // Saved non-final cursors point at the successor page; final cursors
        // retain the last page. An empty final page is an emitted valid state.
        let covered_pages = if complete { page } else { page - 1 };
        if seen > total
            || complete && seen != total
            || !complete && page == 1
            || seen < (page - 1) as u64
            || seen > 50 * covered_pages as u64
        {
            return Err(storage());
        }
        Ok(Self {
            page,
            seen,
            total: Some(total),
            snapshot: c::token(&v["snapshot"]).map_err(|_| storage())?.into(),
            complete,
            fresh: false,
        })
    }
    pub fn next(&self) -> Option<ProviderRead> {
        (!self.complete).then(|| ProviderRead::Collections { page: self.page })
    }
    pub fn accept(
        &mut self,
        doc: ProviderDocument,
        store: &mut CatalogStore,
    ) -> Result<(), NativeError> {
        let ProviderDocument::Collections {
            page,
            records,
            total,
            complete,
        } = doc
        else {
            return Err(super::provider::invalid());
        };
        if page != self.page
            || self.total.is_some_and(|n| n != total)
            || page == 250 && !complete
            || records.is_empty() && (!complete || total != self.seen)
        {
            return Err(incomplete());
        }
        let seen = self
            .seen
            .checked_add(records.len() as u64)
            .ok_or_else(incomplete)?;
        if seen > total || complete && seen != total {
            return Err(NativeError::new("catalogInvalidProviderResponse","Collection page counts contradict the declared provider total; the page was not published."));
        }
        let cursor = object! {page:if complete{page}else{page+1},seen:seen.to_string(),total:total.to_string(),snapshot:self.snapshot.clone(),complete:complete};
        store.apply_collections(&records, &self.snapshot, &cursor, complete)?;
        self.seen = seen;
        self.total = Some(total);
        self.complete = complete;
        if !complete {
            self.page += 1;
        }
        Ok(())
    }
    pub fn result(&self) -> JsonValue {
        object! {state:if self.fresh{"fresh"}else{"complete"},snapshot:self.snapshot.clone(),headersComplete:self.complete,membersComplete:false,memberMaxAgeSeconds:MEMBER_SECONDS}
    }
}

fn type_index(t: &str) -> Option<usize> {
    TYPES.iter().position(|name| *name == t)
}
pub fn progress_value(stage: &str, completed: u64, total: Option<u64>) -> JsonValue {
    // Live totals may shrink or offset pages may overlap. Keep the UI's
    // bounded progress contract without discarding actual cursor progress.
    let completed = total.map_or(completed, |n| completed.min(n));
    object! {stage:stage,completed:completed.to_string(),total:total.map(|n|n.to_string())}
}

#[cfg(test)]
mod refresh_regressions {
    use super::*;
    fn store() -> (std::path::PathBuf, CatalogStore) {
        let root =
            std::env::temp_dir().join(format!("catalog-refresh-{}", c::new_token().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(root.join("cache/catalog")).unwrap();
        let store = CatalogStore::open(&root).unwrap();
        (root, store)
    }
    #[test]
    fn shrinking_live_total_keeps_progress_within_ui_contract() {
        let progress = progress_value("index-mod", 101, Some(100));
        assert_eq!(progress["completed"], "100");
    }
    #[test]
    fn browse_cache_becomes_stale_at_incremental_interval() {
        let (root, mut store) = store();
        store
            .set_state(
                "last_incremental_at",
                &(now() - INCREMENTAL_SECONDS + 60).to_string(),
            )
            .unwrap();
        assert_eq!(store.provenance().unwrap()["stale"], false);
        store
            .set_state(
                "last_incremental_at",
                &(now() - INCREMENTAL_SECONDS).to_string(),
            )
            .unwrap();
        assert_eq!(store.provenance().unwrap()["stale"], true);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn resumed_page_accepts_live_total_drift() {
        let (root, mut store) = store();
        let mut state = SyncState::open(&mut store, true, false).unwrap();
        state.cursor = state.saved_full.take().unwrap();
        state.cursor.page = 14;
        state.cursor.seen = 650;
        state.cursor.total = Some(5424);
        let read = state.next().unwrap();
        let row = json::parse(include_str!(
            "../../tests/fixtures/catalog/large-gallery.json"
        ))
        .unwrap();
        let body=object!{_aMetadata:object!{_nRecordCount:5551,_bIsComplete:false,_nPerpage:50},_aRecords:vec![row]}.dump();
        state
            .accept(super::super::provider::Provider::normalize(read, body.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(state.cursor.total, Some(5551));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn repeated_offset_rows_make_bounded_durable_progress() {
        let (root, mut store) = store();
        let mut state = SyncState::open(&mut store, true, false).unwrap();
        state.cursor = state.saved_full.take().unwrap();
        let row = json::parse(include_str!(
            "../../tests/fixtures/catalog/large-gallery.json"
        ))
        .unwrap();
        let mut yielded = false;
        for _ in 0..100 {
            if state.local_step(&mut store).unwrap() {
                yielded = true;
                break;
            }
            if let Some(read) = state.next() {
                match &read {
                    ProviderRead::Index { .. } => {
                        let body=object!{_aMetadata:object!{_nRecordCount:100,_bIsComplete:false,_nPerpage:50},_aRecords:vec![row.clone()]}.dump();
                        state
                            .accept(
                                super::super::provider::Provider::normalize(read, body.as_bytes())
                                    .unwrap(),
                            )
                            .unwrap();
                    }
                    ProviderRead::Hydrate { keys } => state
                        .accept(ProviderDocument::Hydration {
                            keys: keys.clone(),
                            values: keys.iter().map(|_| object! {}).collect(),
                        })
                        .unwrap(),
                    _ => panic!("unexpected read"),
                }
            }
        }
        assert!(yielded);
        assert_eq!(state.cursor.page, 13);
        assert!(!state.result()["complete"].as_bool().unwrap());
        let reopened = SyncState::open(&mut store, true, false).unwrap();
        assert_eq!(reopened.saved_full.unwrap().page, 13);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn full_crawl_reaches_completion_after_all_types() {
        let (root, mut store) = store();
        let mut state = SyncState::open(&mut store, true, false).unwrap();
        state.cursor = state.saved_full.take().unwrap();
        let mut complete = false;
        for _ in 0..20 {
            if state.local_step(&mut store).unwrap() {
                complete = true;
                break;
            }
            if let Some(read) = state.next() {
                state
                    .accept(ProviderDocument::Page {
                        read,
                        records: vec![],
                        total: 0,
                        complete: true,
                    })
                    .unwrap();
            }
        }
        assert!(complete);
        assert!(store.state_flag("coverage_complete").unwrap());
        assert!(store.state_time("last_full_sync_at").unwrap().is_some());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn new_and_updated_rows_publish_while_full_coverage_is_incomplete() {
        let (root, mut store) = store();
        let mut full = SyncState::open(&mut store, true, false)
            .unwrap()
            .saved_full
            .unwrap();
        full.page = 14;
        full.seen = 650;
        full.total = Some(5424);
        store.set_state("sync_cursor", &full.json().dump()).unwrap();
        let mut row = json::parse(include_str!(
            "../../tests/fixtures/catalog/large-gallery.json"
        ))
        .unwrap();
        let key = SubmissionKeyV1::from_slug("900000001").unwrap();
        let ProviderDocument::Profile(prior) = super::super::provider::Provider::normalize(
            ProviderRead::Profile(key),
            row.dump().as_bytes(),
        )
        .unwrap() else {
            panic!("profile")
        };
        store.publish_profile(prior).unwrap();
        store
            .db
            .execute("UPDATE submission SET snapshot=?1", &[&full.snapshot])
            .unwrap();
        row["_sName"] = "Updated title".into();
        let mut new = row.clone();
        new["_idRow"] = 900000002.into();
        new["_sProfileUrl"] = "https://gamebanana.com/mods/900000002".into();
        new["_sName"] = "New mod".into();
        let mut state = SyncState::open(&mut store, true, false).unwrap();
        for _ in 0..40 {
            state.local_step(&mut store).unwrap();
            if state.saved_full.is_none() {
                break;
            }
            if let Some(read) = state.next() {
                match &read {
                    ProviderRead::Index {
                        submission_type, ..
                    } => {
                        let rows = if submission_type == "mod" {
                            vec![row.clone(), new.clone()]
                        } else {
                            vec![]
                        };
                        let body=object!{_aMetadata:object!{_nRecordCount:rows.len(),_bIsComplete:true,_nPerpage:50},_aRecords:rows}.dump();
                        state
                            .accept(
                                super::super::provider::Provider::normalize(read, body.as_bytes())
                                    .unwrap(),
                            )
                            .unwrap();
                    }
                    ProviderRead::Hydrate { keys } => {
                        state
                            .accept(ProviderDocument::Hydration {
                                keys: keys.clone(),
                                values: keys.iter().map(|_| object! {}).collect(),
                            })
                            .unwrap();
                    }
                    _ => panic!("unexpected read"),
                }
            }
        }
        assert!(state.saved_full.is_none());
        assert_eq!(
            store.state("sync_cursor").unwrap().unwrap(),
            full.json().dump()
        );
        assert!(!store.state_flag("coverage_complete").unwrap());
        for (id, name) in [("900000001", "Updated title"), ("900000002", "New mod")] {
            let row = store
                .db
                .query(
                    "SELECT name,snapshot FROM submission WHERE submission_id=?1",
                    &[id],
                )
                .unwrap()
                .unwrap();
            assert_eq!(row[0], name);
            if id == "900000001" {
                assert_eq!(row[1], full.snapshot);
            }
        }
        assert!(store.state_time("last_incremental_at").unwrap().is_some());
        assert_eq!(store.provenance().unwrap()["stale"], false);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn refresh_reads_latest_before_resuming_full_crawl() {
        let (root, mut store) = store();
        let mut full = SyncState::open(&mut store, true, false).unwrap().cursor;
        full.full = true;
        full.page = 14;
        full.seen = 650;
        full.total = Some(5424);
        store.set_state("sync_cursor", &full.json().dump()).unwrap();
        let mut state = SyncState::open(&mut store, true, false).unwrap();
        assert!(matches!(
            state.next(),
            Some(ProviderRead::Index {
                page: 1,
                latest: true,
                ..
            })
        ));
        assert_eq!(
            store.state("sync_cursor").unwrap().unwrap(),
            full.json().dump()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
