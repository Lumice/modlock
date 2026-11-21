//! Worker-local native catalog authority; no SQLite Send implementation.
use super::{
    collections::{CollectionListScan, CollectionQuery, MemberCrawl},
    contract as c,
    details::{ResourceQuery, ResourceResult},
    owner::{Executor, Step, Work},
    preferences::CatalogPreferences,
    provider::{Provider, ProviderDocument, ProviderRead, SubmissionKeyV1},
    query::CatalogQueryV1,
    store::CatalogStore,
    sync::{incomplete, CollectionSync, SyncState},
    transport::{MetadataRequest, MetadataTransport},
};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
struct Read {
    read: ProviderRead,
    request: MetadataRequest,
    attempt: u8,
    submitted: bool,
    after: Instant,
}
fn retry_delay(retry_after_seconds: Option<u32>, attempt: u8) -> Duration {
    retry_after_seconds
        .map(|seconds| Duration::from_secs(seconds.min(86400) as u64))
        .unwrap_or_else(|| {
            Duration::from_millis(500u64 << attempt.min(6)).min(Duration::from_secs(30))
        })
}
struct SyncJob {
    key: String,
    state: SyncState,
    read: Option<Read>,
    submissions_done: bool,
    collections: Option<CollectionSync>,
    force_collections: bool,
    network_succeeded: bool,
    ready_result: Option<JsonValue>,
}
enum RefreshQuery {
    Resource(ResourceQuery),
    Collection(CollectionQuery),
}
struct RefreshJob {
    key: String,
    query: RefreshQuery,
    phase: &'static str,
    read: Option<Read>,
    publication: Option<ProviderDocument>,
    crawl: Option<MemberCrawl>,
    description_revision: JsonValue,
}
impl SyncJob {
    fn progress(&self) -> Option<JsonValue> {
        if let Some(c) = self.collections.as_ref().filter(|c| !c.fresh) {
            return Some(super::sync::progress_value("collections", c.seen, c.total));
        }
        (!self.state.progress.is_null()).then(|| self.state.progress.clone())
    }
}
pub struct NativeExecutor {
    pub store: CatalogStore,
    preferences: CatalogPreferences,
    sync: Option<SyncJob>,
    refresh: Option<RefreshJob>,
    refreshes: BTreeMap<String, RefreshJob>,
    collection_lists: BTreeMap<String, CollectionListScan>,
    retry_after: BTreeMap<String, u32>,
    last_progress: Option<(String, JsonValue)>,
    pending_failure: Option<(String, NativeError)>,
}
impl NativeExecutor {
    pub fn open(root: &Path) -> Result<Self, NativeError> {
        Ok(Self {
            store: CatalogStore::open(root)?,
            preferences: CatalogPreferences::open(root)?,
            sync: None,
            refresh: None,
            refreshes: BTreeMap::new(),
            collection_lists: BTreeMap::new(),
            retry_after: BTreeMap::new(),
            last_progress: None,
            pending_failure: None,
        })
    }
    fn envelope(
        &self,
        work: &Work,
        data: JsonValue,
        prefs: Option<u64>,
    ) -> Result<JsonValue, NativeError> {
        Ok(
            object! {identity:object!{kind:work.operation["kind"].clone(),args:work.operation["args"].clone(),catalogRevision:self.store.revision.to_string(),preferencesRevision:prefs.map(|n|n.to_string())},catalogRevision:self.store.revision.to_string(),provenance:self.store.provenance()?,data:data},
        )
    }
    fn local(&mut self, work: &Work) -> Result<JsonValue, NativeError> {
        let a = &work.operation["args"];
        match work.operation["kind"].as_str() {
            Some("query") => {
                let q = CatalogQueryV1::parse(a)?;
                let prefs = self.preferences.snapshot()?;
                let mut data = self.store.query_with_preferences(&q, Some(&prefs))?.data;
                let favorites = data["items"]
                    .members()
                    .map(|s| {
                        let key = SubmissionKeyV1::parse(&s["submission"])
                            .map_err(|_| super::store::storage())?;
                        Ok(object! {submission:key.json(),favorite:prefs.favorites.contains(&key)})
                    })
                    .collect::<Result<Vec<_>, NativeError>>()?;
                data["favorites"] = favorites.into();
                data["preferencesRevision"] = prefs.revision.to_string().into();
                self.envelope(work, data, Some(prefs.revision))
            }
            Some("facets") => {
                let q = CatalogQueryV1::scope(a, false)?;
                self.envelope(work, self.store.facets(&q)?, None)
            }
            Some("authors") => {
                let q = CatalogQueryV1::scope(a, true)?;
                self.envelope(work, self.store.authors(&q)?, None)
            }
            Some(kind @ ("detail" | "files" | "comments" | "changelog")) => {
                let q = ResourceQuery::parse(kind, a)?;
                let result = self.store.read_resource(&q)?;
                self.resource_envelope(work, result, None, false)
            }
            Some("collection") => {
                let q = CollectionQuery::parse(a)?;
                let prefs = self.preferences.snapshot()?;
                let result = self.store.collection_page(&q, &prefs)?;
                self.resource_envelope(work, result, Some(prefs.revision), false)
            }
            Some("favoritesGet") => {
                let k = if a["submission"].is_null() {
                    None
                } else {
                    Some(SubmissionKeyV1::parse(&a["submission"])?)
                };
                let data = self.preferences.get(k)?;
                let revision = c::u64_text(&data["preferencesRevision"])?;
                self.envelope(work, data, Some(revision))
            }
            Some("favoriteSet") => {
                let mut envelope = self.envelope(work, JsonValue::Null, None)?;
                let data = self.preferences.set_favorite(
                    SubmissionKeyV1::parse(&a["submission"])?,
                    a["desired"].as_bool().ok_or_else(NativeError::params)?,
                    c::u64_text(&a["expectedRevision"])?,
                )?;
                let revision = c::u64_text(&data["preferencesRevision"])?;
                envelope["identity"]["preferencesRevision"] = revision.to_string().into();
                envelope["data"] = data;
                Ok(envelope)
            }
            _ => Err(c::error("catalogUnavailable")),
        }
    }
    fn resource_envelope(
        &self,
        work: &Work,
        mut result: ResourceResult,
        prefs: Option<u64>,
        network: bool,
    ) -> Result<JsonValue, NativeError> {
        result.data["refreshRecommended"] = (result.stale
            || matches!(result.data["state"].as_str(), Some("partial" | "missing"))
            || result.data["state"].is_null() && result.incomplete)
            .into();
        result.data["refreshState"] = if network { "completed" } else { "idle" }.into();
        if result.data.has_key("header") {
            result.data["headerSource"] = if result.data["header"].is_null() {
                JsonValue::Null
            } else {
                "cache".into()
            };
            result.data["descriptionSource"] = if result.data["descriptionFetchedAt"].is_null() {
                JsonValue::Null
            } else {
                if network { "network" } else { "cache" }.into()
            };
            result.data["membershipSource"] = if result.data["lastMemberSyncAt"].is_null() {
                JsonValue::Null
            } else {
                if network { "network" } else { "cache" }.into()
            };
        }
        let mixed = network && result.data.has_key("header") && !result.data["header"].is_null();
        let mut value = self.envelope(work, result.data, prefs)?;
        value["provenance"]["source"] = if mixed {
            "mixed"
        } else if network {
            "network"
        } else {
            "cache"
        }
        .into();
        value["provenance"]["fetchedAt"] = result.fetched.into();
        value["provenance"]["stale"] = result.stale.into();
        if work.operation["kind"] == "collections" {
            // Header refresh belongs to shared sync, so its retained failure and
            // coverage context remain applicable even when old filtering works.
            let incomplete = result.incomplete || value["provenance"]["incomplete"] == true;
            value["provenance"]["incomplete"] = incomplete.into();
            if incomplete || !value["provenance"]["refreshError"].is_null() {
                value["data"]["refreshRecommended"] = true.into();
            }
        } else {
            value["provenance"]["incomplete"] = result.incomplete.into();
            // Independent resources expose refresh failure on their own receipt.
            value["provenance"]["refreshError"] = JsonValue::Null;
        }
        Ok(value)
    }
    fn collections_step(&mut self, work: &Work) -> Result<Option<JsonValue>, NativeError> {
        if !self.collection_lists.contains_key(&work.request_key) {
            self.store.projection_step();
            let Some(scan) = self.store.start_collection_list(&work.operation["args"])? else {
                return Ok(None);
            };
            self.collection_lists.insert(work.request_key.clone(), scan);
        }
        let complete = self
            .store
            .scan_collection_list(self.collection_lists.get_mut(&work.request_key).unwrap())?;
        if !complete {
            return Ok(None);
        }
        let scan = self.collection_lists.remove(&work.request_key).unwrap();
        let result = self.store.collection_list(scan)?;
        self.resource_envelope(work, result, None, false).map(Some)
    }
    fn refresh_step(
        &mut self,
        work: &Work,
        transport: &mut MetadataTransport,
    ) -> Result<Option<JsonValue>, NativeError> {
        if self
            .refresh
            .as_ref()
            .is_some_and(|j| j.key != work.request_key)
        {
            return Ok(None);
        }
        if self.refresh.is_none() {
            let kind = work.operation["kind"]
                .as_str()
                .ok_or_else(NativeError::params)?;
            let query = if kind == "collection" {
                RefreshQuery::Collection(CollectionQuery::parse(&work.operation["args"])?)
            } else {
                RefreshQuery::Resource(ResourceQuery::parse(kind, &work.operation["args"])?)
            };
            // Same-resource crawls share durable staging/mode state. Serialize
            // those identities only; different resources and sync still rotate.
            if self
                .refreshes
                .values()
                .any(|job| match (&query, &job.query) {
                    (RefreshQuery::Collection(a), RefreshQuery::Collection(b)) => a.id == b.id,
                    (RefreshQuery::Resource(a), RefreshQuery::Resource(b)) => a.key == b.key,
                    _ => false,
                })
            {
                return Ok(None);
            }
            if let RefreshQuery::Resource(q) = &query {
                if self.sync.as_ref().is_some_and(|job| {
                    job.read
                        .as_ref()
                        .is_some_and(|r| matches!(&r.read,ProviderRead::Profile(k) if *k==q.key))
                        || job.state.publication.as_ref().is_some_and(|p| {
                            p.records
                                .iter()
                                .any(|r| r.key == q.key && r.detail.is_some())
                        })
                }) {
                    return Ok(None);
                }
            }
            let revision = if let RefreshQuery::Collection(q) = &query {
                self.store
                    .collection_header(&q.id)?
                    .ok_or_else(|| c::error("catalogNotFound"))?["remoteUpdatedAt"]
                    .clone()
            } else {
                JsonValue::Null
            };
            self.refresh = Some(RefreshJob {
                key: work.request_key.clone(),
                query,
                phase: if kind == "collection" {
                    "description"
                } else {
                    "profile"
                },
                read: None,
                publication: None,
                crawl: None,
                description_revision: revision,
            });
        }
        if self.refresh.as_ref().unwrap().phase == "done" {
            let job = self.refresh.take().unwrap();
            return match job.query {
                RefreshQuery::Resource(q) => self
                    .resource_envelope(work, self.store.read_resource(&q)?, None, true)
                    .map(Some),
                RefreshQuery::Collection(q) => {
                    let prefs = self.preferences.snapshot()?;
                    self.resource_envelope(
                        work,
                        self.store.collection_page(&q, &prefs)?,
                        Some(prefs.revision),
                        true,
                    )
                    .map(Some)
                }
            };
        }
        let job = self.refresh.as_mut().unwrap();
        if let Some(doc) = job.publication.take() {
            match doc {
                ProviderDocument::Profile(record) => {
                    self.store.publish_profile(record)?;
                    let RefreshQuery::Resource(q) = &job.query else {
                        return Err(super::store::storage());
                    };
                    job.phase = if q.kind == "detail" {
                        "done"
                    } else if q.kind == "files" {
                        "files"
                    } else {
                        "activity"
                    };
                }
                ProviderDocument::Files(files) => {
                    let RefreshQuery::Resource(q) = &job.query else {
                        return Err(super::store::storage());
                    };
                    self.store
                        .put_resource("files", &q.identity(), &files, true)?;
                    job.phase = "done";
                }
                ProviderDocument::Activity(activity) => {
                    let RefreshQuery::Resource(q) = &job.query else {
                        return Err(super::store::storage());
                    };
                    let complete = !activity["items"]
                        .members()
                        .any(|e| e["linksTruncated"] == true);
                    self.store
                        .put_resource(&q.kind, &q.identity(), &activity, complete)?;
                    job.phase = "done";
                }
                ProviderDocument::CollectionDescription(description) => {
                    let RefreshQuery::Collection(q) = &job.query else {
                        return Err(super::store::storage());
                    };
                    self.store
                        .save_description(&q.id, &job.description_revision, &description)?;
                    job.crawl = Some(self.store.open_members(&q.id)?);
                    job.phase = "members";
                }
                ProviderDocument::Members {
                    collection_id,
                    page,
                    members,
                    total,
                    complete,
                } => {
                    let RefreshQuery::Collection(q) = &job.query else {
                        return Err(super::store::storage());
                    };
                    if q.id != collection_id {
                        return Err(super::provider::invalid());
                    }
                    if self.store.stage_members(
                        job.crawl.as_mut().ok_or_else(super::store::storage)?,
                        page,
                        members,
                        total,
                        complete,
                    )? {
                        job.phase = "done";
                    }
                }
                _ => return Err(super::store::storage()),
            }
            return Ok(None);
        }
        if let Some(completion) = job
            .read
            .as_ref()
            .and_then(|r| transport.poll_for(&r.request.request_id))
        {
            let pending = job
                .read
                .as_mut()
                .ok_or_else(|| c::error("catalogTransport"))?;
            if completion.request_id != pending.request.request_id || completion.error.is_some() {
                return Err(c::error("catalogTransport"));
            }
            if completion.final_url.as_deref() != Some(pending.request.url.as_str()) {
                return Err(super::provider::invalid());
            }
            let status = completion
                .http_status
                .ok_or_else(|| c::error("catalogTransport"))?;
            if status == 429 || (500..=599).contains(&status) {
                let seconds = completion.retry_after_seconds.unwrap_or(1).min(86400);
                self.retry_after.insert(work.request_key.clone(), seconds);
                if pending.attempt >= 3 || seconds > 30 {
                    return Err(c::error(if status == 429 {
                        "catalogRateLimited"
                    } else {
                        "catalogTransport"
                    }));
                }
                let delay = retry_delay(completion.retry_after_seconds, pending.attempt);
                pending.attempt += 1;
                pending.submitted = false;
                pending.after = Instant::now() + delay;
                pending.request = Provider::request(pending.read.clone())?;
                return Ok(None);
            }
            if status == 404 {
                if let ProviderRead::Profile(key) = &pending.read {
                    self.store.mark_unavailable(key, "catalogNotFound")?;
                    job.phase = "done";
                    job.read = None;
                    return Ok(None);
                }
                return Err(c::error("catalogNotFound"));
            }
            let hidden_status = status == 400
                && matches!(&pending.read,ProviderRead::Activity{kind,..} if kind=="comments");
            if status == 403 {
                return Err(c::error("catalogRefused"));
            }
            if !(200..=299).contains(&status) && !hidden_status {
                return Err(super::provider::invalid());
            }
            let body = completion
                .body
                .ok_or_else(|| c::error("catalogTransport"))?;
            let doc = Provider::normalize(pending.read.clone(), &body);
            drop(body);
            match doc {
                Ok(doc) => {
                    if hidden_status
                        && !matches!(&doc,ProviderDocument::Activity(v) if v["state"]=="hidden")
                    {
                        return Err(super::provider::invalid());
                    }
                    job.publication = Some(doc);
                }
                Err(e) if e.kind == "catalogPrivate" => {
                    if let ProviderRead::Profile(key) = &pending.read {
                        self.store.mark_unavailable(key, "catalogPrivate")?;
                        job.phase = "done";
                    } else {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
            job.read = None;
            return Ok(None);
        }
        if job.read.is_none() {
            let read = match &job.query {
                RefreshQuery::Resource(q) => match job.phase {
                    "profile" => ProviderRead::Profile(q.key.clone()),
                    "files" => ProviderRead::Files(q.key.clone()),
                    "activity" => ProviderRead::Activity {
                        key: q.key.clone(),
                        kind: q.kind.clone(),
                        page: q.page,
                    },
                    _ => return Err(super::store::storage()),
                },
                RefreshQuery::Collection(q) => match job.phase {
                    "description" => ProviderRead::CollectionDescription(q.id.clone()),
                    "members" => ProviderRead::Members {
                        collection_id: q.id.clone(),
                        page: job
                            .crawl
                            .as_ref()
                            .ok_or_else(super::store::storage)?
                            .next_page(),
                    },
                    _ => return Err(super::store::storage()),
                },
            };
            let request = Provider::request(read.clone())?;
            job.read = Some(Read {
                read,
                request,
                attempt: 0,
                submitted: false,
                after: Instant::now(),
            });
        }
        if let Some(read) = &mut job.read {
            if !read.submitted && Instant::now() >= read.after {
                match transport.submit(read.request.clone()) {
                    Ok(()) => read.submitted = true,
                    Err(e) if e.kind == "catalogBusy" => {}
                    Err(_) => return Err(c::error("catalogTransport")),
                }
            }
        }
        Ok(None)
    }
    fn sync_step(
        &mut self,
        work: &Work,
        transport: &mut MetadataTransport,
    ) -> Result<Option<JsonValue>, NativeError> {
        if self.pending_failure.is_some() {
            return Err(NativeError::new(
                "catalogUnavailable",
                "A previous sync failure is being retained; wait for its terminal result.",
            ));
        }
        if let Some(job) = &self.sync {
            if job.key != work.request_key {
                return Err(NativeError::new(
                    "catalogUnavailable",
                    "A catalog sync already owns the bounded sync lane.",
                ));
            }
        } else {
            let a = &work.operation["args"];
            if a["clearCache"] == true {
                if self.refresh.is_some() || !self.refreshes.is_empty() || !self.collection_lists.is_empty() {
                    return Err(NativeError::new("catalogUnavailable", "Wait for the current catalog requests to finish, then try clearing again."));
                }
                self.store.clear_cache()?;
            }
            self.sync = Some(SyncJob {
                key: work.request_key.clone(),
                state: SyncState::open(
                    &mut self.store,
                    a["forceRefresh"]
                        .as_bool()
                        .ok_or_else(NativeError::params)?,
                    a["forceReconcile"]
                        .as_bool()
                        .ok_or_else(NativeError::params)?,
                )?,
                read: None,
                submissions_done: false,
                collections: None,
                force_collections: a["forceRefresh"] == true,
                network_succeeded: false,
                ready_result: None,
            });
            if a["forceRefresh"] == true || a["forceReconcile"] == true {
                self.store.retry_search_maintenance();
            }
        }
        if self.sync.as_ref().is_some_and(|j| j.ready_result.is_some()) {
            let job = self.sync.as_ref().unwrap();
            if job.network_succeeded && !job.state.throttled {
                self.store.successful_refresh()?;
            }
            let mut job = self.sync.take().unwrap();
            self.last_progress = job.progress().map(|p| (work.request_key.clone(), p));
            return self
                .envelope(work, job.ready_result.take().unwrap(), None)
                .map(Some);
        }
        let job = self.sync.as_mut().unwrap();
        if let Some(completion) = job
            .read
            .as_ref()
            .and_then(|r| transport.poll_for(&r.request.request_id))
        {
            let pending = job
                .read
                .as_mut()
                .ok_or_else(|| c::error("catalogTransport"))?;
            if completion.request_id != pending.request.request_id {
                return Err(c::error("catalogTransport"));
            }
            if completion.error.is_some() {
                return Err(c::error("catalogTransport"));
            }
            if completion.final_url.as_deref() != Some(pending.request.url.as_str()) {
                return Err(super::provider::invalid());
            }
            let status = completion
                .http_status
                .ok_or_else(|| c::error("catalogTransport"))?;
            if status == 429 || (500..=599).contains(&status) {
                self.retry_after.insert(
                    work.request_key.clone(),
                    completion.retry_after_seconds.unwrap_or(1).min(86400),
                );
                if pending.attempt >= 3 {
                    return Err(c::error(if status == 429 {
                        "catalogRateLimited"
                    } else {
                        "catalogTransport"
                    }));
                }
                let delay = retry_delay(completion.retry_after_seconds, pending.attempt);
                // Longer provider pauses are surfaced, never silently slept on
                // the worker. Retry backoff itself stays at most30 seconds.
                if delay > Duration::from_secs(30) {
                    return Err(c::error("catalogRateLimited"));
                }
                pending.attempt += 1;
                pending.submitted = false;
                pending.after = Instant::now() + delay;
                pending.request = Provider::request(pending.read.clone())?;
                return Ok(None);
            }
            if status == 404 && job.state.cursor.phase == "confirm" {
                if let ProviderRead::Profile(key) = &pending.read {
                    job.state
                        .confirm_absent(&mut self.store, key, "catalogNotFound")?;
                    job.network_succeeded = true;
                    job.read = None;
                    return Ok(None);
                }
            }
            if status == 403 {
                return Err(c::error("catalogRefused"));
            }
            if status == 404 {
                return Err(c::error("catalogNotFound"));
            }
            if !(200..=299).contains(&status) {
                return Err(super::provider::invalid());
            }
            let body = completion
                .body
                .ok_or_else(|| c::error("catalogTransport"))?;
            let doc = Provider::normalize(pending.read.clone(), &body);
            drop(body);
            match doc {
                Ok(doc) => {
                    if let Some(collections) = &mut job.collections {
                        collections.accept(doc, &mut self.store)?;
                    } else {
                        job.state.accept(doc)?;
                    }
                }
                Err(e) if e.kind == "catalogPrivate" && job.state.cursor.phase == "confirm" => {
                    if let ProviderRead::Profile(key) = &pending.read {
                        job.state
                            .confirm_absent(&mut self.store, key, "catalogPrivate")?;
                    } else {
                        return Err(incomplete());
                    }
                }
                Err(e) => return Err(e),
            }
            job.network_succeeded = true;
            job.read = None;
            // Publication is a separate step so local foreground reads rotate
            // between decoding and the bounded page transaction.
            return Ok(None);
        }
        if job.read.is_none() {
            if !job.submissions_done && job.state.local_step(&mut self.store)? {
                job.submissions_done = true;
                if job.state.yielded {
                    if work.operation["args"]["clearCache"] == true {
                        // Reset is one explicit resync. Resume the durable full
                        // cursor between bounded slices, without clearing again.
                        job.state = SyncState::open(&mut self.store, true, false)?;
                        job.submissions_done = false;
                        return Ok(None);
                    }
                    // Publish the successful bounded refresh immediately. An
                    // independent collection crawl cannot hide fresh submissions.
                    let mut data = job.state.result();
                    data["collections"] = CollectionSync::open(&self.store, false)?.result();
                    job.ready_result = Some(data);
                    return Ok(None);
                }
                job.collections = Some(CollectionSync::open(&self.store, job.force_collections)?);
            }
            if job.submissions_done && job.collections.as_ref().is_some_and(|c| c.complete) {
                let mut data = job.state.result();
                data["collections"] = job.collections.as_ref().unwrap().result();
                job.ready_result = Some(data);
                return Ok(None);
            }
            let next = if let Some(collections) = &job.collections {
                collections.next()
            } else {
                job.state.next()
            };
            if next.is_none() {
                if job.collections.is_some() {
                    return Err(super::store::storage());
                }
                job.state.pending_local_transition()?;
            }
            if let Some(read) = next {
                let request = Provider::request(read.clone())?;
                job.read = Some(Read {
                    read,
                    request,
                    attempt: 0,
                    submitted: false,
                    after: Instant::now(),
                });
            }
        }
        if let Some(read) = &mut job.read {
            if !read.submitted && Instant::now() >= read.after {
                if let ProviderRead::Profile(key) = &read.read {
                    if self
                        .refreshes
                        .values()
                        .any(|j| matches!(&j.query,RefreshQuery::Resource(q) if q.key==*key))
                    {
                        return Ok(None);
                    }
                }
                match transport.submit(read.request.clone()) {
                    Ok(()) => read.submitted = true,
                    Err(e) if e.kind == "catalogBusy" => {}
                    Err(_) => return Err(c::error("catalogTransport")),
                }
            }
        }
        Ok(None)
    }
}
impl Executor for NativeExecutor {
    fn progress(&self, key: &str) -> Option<JsonValue> {
        self.sync
            .as_ref()
            .filter(|j| j.key == key)
            .and_then(SyncJob::progress)
            .or_else(|| {
                self.last_progress
                    .as_ref()
                    .filter(|(k, _)| k == key)
                    .map(|(_, p)| p.clone())
            })
    }
    fn retry_after(&self, key: &str) -> Option<u32> {
        self.retry_after.get(key).copied()
    }
    fn idle(&mut self) -> Result<(), NativeError> {
        self.store.projection_step();
        self.store.maintain_search()
    }
    fn step(&mut self, work: &Work, transport: &mut MetadataTransport) -> Step {
        let kind = work.operation["kind"].as_str().unwrap_or("");
        if kind == "collections" {
            return match self.collections_step(work) {
                Ok(Some(value)) => Step::Complete(Ok(value)),
                Ok(None) => Step::Pending,
                Err(error) => {
                    self.collection_lists.remove(&work.request_key);
                    Step::Complete(Err(error))
                }
            };
        }
        if matches!(
            kind,
            "detail" | "files" | "comments" | "changelog" | "collection"
        ) && work.operation["args"]["refresh"] == true
        {
            self.refresh = self.refreshes.remove(&work.request_key);
            return match self.refresh_step(work, transport) {
                Ok(Some(v)) => Step::Complete(Ok(v)),
                Ok(None) => {
                    if let Some(job) = self.refresh.take() {
                        self.refreshes.insert(work.request_key.clone(), job);
                    }
                    Step::Pending
                }
                Err(e) => {
                    if self
                        .refresh
                        .as_ref()
                        .is_some_and(|j| j.key == work.request_key)
                    {
                        if let Some(job) = self.refresh.take() {
                            if let Some(r) = job.read {
                                transport.discard(r.request.request_id);
                            }
                        }
                    }
                    Step::Complete(Err(e))
                }
            };
        }
        if work.operation["kind"] != "sync" {
            return Step::Complete(self.local(work));
        }
        if self
            .pending_failure
            .as_ref()
            .is_some_and(|(key, _)| key == &work.request_key)
        {
            let (_, error) = self.pending_failure.take().unwrap();
            // Failure persistence is its own bounded worker step. The original
            // error remains authoritative even if this checkpoint fails.
            let _ = self.store.persist_refresh_failure();
            return Step::Complete(Err(error));
        }
        match self.sync_step(work, transport) {
            Ok(Some(v)) => Step::Complete(Ok(v)),
            Ok(None) => Step::Pending,
            Err(e) => {
                let owns = self
                    .sync
                    .as_ref()
                    .is_some_and(|j| j.key == work.request_key);
                let record = owns || (self.sync.is_none() && self.pending_failure.is_none());
                if owns {
                    if let Some(job) = self.sync.take() {
                        self.last_progress = job.progress().map(|p| (work.request_key.clone(), p));
                        if let Some(r) = job.read {
                            transport.discard(r.request.request_id);
                        }
                    }
                }
                if record {
                    let retry = self.retry_after(&work.request_key);
                    self.store.remember_refresh_failure(&e, retry);
                    self.pending_failure = Some((work.request_key.clone(), e));
                    Step::Pending
                } else {
                    Step::Complete(Err(e))
                }
            }
        }
    }
    fn cancel(&mut self, work: &Work, transport: &mut MetadataTransport) {
        self.collection_lists.remove(&work.request_key);
        if let Some(job) = self.refreshes.remove(&work.request_key) {
            if let Some(r) = job.read {
                transport.discard(r.request.request_id);
            }
        }
        if self
            .refresh
            .as_ref()
            .is_some_and(|j| j.key == work.request_key)
        {
            if let Some(job) = self.refresh.take() {
                if let Some(r) = job.read {
                    transport.discard(r.request.request_id);
                }
            }
        }
        if self
            .pending_failure
            .as_ref()
            .is_some_and(|(k, _)| k == &work.request_key)
        {
            let _ = self.store.persist_refresh_failure();
            self.pending_failure = None;
        }
        if self
            .sync
            .as_ref()
            .is_some_and(|j| j.key == work.request_key)
        {
            if let Some(job) = self.sync.take() {
                self.last_progress = job.progress().map(|p| (work.request_key.clone(), p));
                if let Some(r) = job.read {
                    transport.discard(r.request.request_id);
                }
            }
            self.store
                .remember_refresh_failure(&c::error("catalogCancelled"), None);
            let _ = self.store.persist_refresh_failure();
        }
    }
}

#[cfg(test)]
mod settings_review_tests {
    use super::*;
    #[test]
    fn settings_reset_sync_finishes_full_cursor_and_collections() {
        let root=std::env::temp_dir().join(format!("settings-sync-{}",c::new_token().unwrap()));
        std::fs::create_dir_all(root.join("cache/catalog")).unwrap();
        let mut executor=NativeExecutor::open(&root).unwrap();
        let mut transport=MetadataTransport::new(root.join("transport"),c::new_token().unwrap()).unwrap();
        let work=Work{request_key:c::new_token().unwrap(),operation:object!{kind:"sync",args:object!{forceRefresh:true,forceReconcile:true,clearCache:true}},cancelled:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))};
        let row=json::parse(include_str!("../../tests/fixtures/catalog/large-gallery.json")).unwrap();
        let mut final_result=None;
        let mut highest_page=0;
        let mut saw_progress=false;
        // Supply provider documents before polling: no network child is started.
        for _ in 0..800 {
            if let Some(job)=executor.sync.as_mut() {
                if let Some(pending)=job.read.take() {
                    let read=pending.read;
                    match &read {
                        ProviderRead::Collections{page} => job.collections.as_mut().unwrap().accept(ProviderDocument::Collections{page:*page,records:vec![],total:0,complete:true},&mut executor.store).unwrap(),
                        ProviderRead::Index{submission_type,page,latest} => {
                            let document=if submission_type=="mod" && !latest {
                                highest_page=highest_page.max(*page);
                                let body=object!{_aMetadata:object!{_nRecordCount:14,_bIsComplete:*page>=14,_nPerpage:50},_aRecords:vec![row.clone()]}.dump();
                                Provider::normalize(read.clone(),body.as_bytes()).unwrap()
                            }else{ProviderDocument::Page{read:read.clone(),records:vec![],total:0,complete:true}};
                            job.state.accept(document).unwrap();
                        },
                        ProviderRead::Hydrate{keys} => job.state.accept(ProviderDocument::Hydration{keys:keys.clone(),values:keys.iter().map(|_|object!{}).collect()}).unwrap(),
                        _ => panic!("unexpected provider read"),
                    }
                    job.network_succeeded=true;
                }
            }
            saw_progress |= executor.progress(&work.request_key).is_some();
            if let Some(result)=executor.sync_step(&work,&mut transport).unwrap(){final_result=Some(result);break;}
        }
        let result=final_result.expect("reset sync did not finish");
        assert!(highest_page>12,"reset stopped at the first bounded crawl slice");
        assert_eq!(result["data"]["complete"],true,"{result}");
        assert_eq!(result["data"]["collections"]["headersComplete"],true,"{result}");
        assert!(executor.store.state_flag("coverage_complete").unwrap());
        assert!(saw_progress);
        drop(transport);drop(executor);
        std::fs::remove_dir_all(root).unwrap();
    }
}
