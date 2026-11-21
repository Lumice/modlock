//! Ordered collection reads, adapted 2026-10-28 from pinned catalog/collections.rs,
//! client.rs and models.rs, GPL-3.0 (UPSTREAM-LICENSE).
use super::{
    contract as c,
    details::ResourceResult,
    preferences::PreferenceSnapshot,
    provider::{self as p, SubmissionKeyV1},
    store::{now, storage, CatalogStore},
};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::collections::BTreeMap;
pub const MEMBER_TTL: u64 = 7 * 86400;
const PROJECTION_LIMIT: usize = 32 * 1024 * 1024;
const ENTRY_OVERHEAD: usize = 384;
const BASE_CHARGE: usize = 64 * 1024;
struct HeaderProjection {
    name: String,
    author: String,
    nsfw: bool,
    content_known: bool,
    count: u64,
}
pub struct CollectionProjection {
    entries: BTreeMap<String, HeaderProjection>,
    order: Vec<String>,
    charged: usize,
    cursor: String,
    pub ready: bool,
    generation: u64,
    error: Option<(String, String)>,
}
impl Default for CollectionProjection {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: Vec::new(),
            charged: BASE_CHARGE,
            cursor: String::new(),
            ready: false,
            generation: 0,
            error: None,
        }
    }
}
fn compact(header: &JsonValue) -> Result<(String, HeaderProjection), NativeError> {
    let id = p::positive(&header["collectionId"]).map_err(|_| storage())?;
    let bounded = |name: &str| {
        header[name]
            .as_str()
            .filter(|s| s.len() <= 1024 && !s.contains('\0'))
            .map(str::to_owned)
            .ok_or_else(storage)
    };
    let nsfw = header["isNsfw"].as_bool().ok_or_else(storage)?;
    let content_known = if header["contentRatingKnown"].is_null() {
        false
    } else {
        header["contentRatingKnown"].as_bool().ok_or_else(storage)?
    };
    Ok((
        id,
        HeaderProjection {
            name: bounded("name")?,
            author: bounded("author")?,
            nsfw,
            content_known,
            count: c::u64_text(&header["itemCount"]).map_err(|_| storage())?,
        },
    ))
}
impl CollectionProjection {
    fn charge(id: &str, entry: &HeaderProjection) -> usize {
        // Three ID byte charges cover map key, sorted index and replacement
        // index. BASE_CHARGE reserves cursor/eight bounded scan controls; fixed
        // entry overhead charges index/map bookkeeping, not an OS RSS promise.
        ENTRY_OVERHEAD + 3 * id.len() + entry.name.len() + entry.author.len()
    }
    fn insert(&mut self, id: String, entry: HeaderProjection) -> Result<(), NativeError> {
        let prior = self.entries.get(&id).map_or(0, |e| Self::charge(&id, e));
        let charged = self
            .charged
            .checked_sub(prior)
            .and_then(|n| n.checked_add(Self::charge(&id, &entry)))
            .filter(|n| *n <= PROJECTION_LIMIT)
            .ok_or_else(|| c::error("catalogIncomplete"))?;
        if self.entries.len() == 12500 && !self.entries.contains_key(&id) {
            return Err(c::error("catalogIncomplete"));
        }
        self.entries.insert(id, entry);
        self.charged = charged;
        Ok(())
    }
    fn sorted(&mut self) {
        let mut order = self.entries.keys().cloned().collect::<Vec<_>>();
        order.sort_by(|a, b| {
            self.entries[b]
                .count
                .cmp(&self.entries[a].count)
                .then_with(|| a.cmp(b))
        });
        self.order = order;
    }
    fn fail(&mut self, error: NativeError) {
        self.error = Some((error.kind, error.message));
        self.ready = false;
        self.entries = BTreeMap::new();
        self.order = Vec::new();
        self.cursor = String::new();
        self.charged = BASE_CHARGE;
    }
    fn check(&self) -> Result<(), NativeError> {
        if let Some((kind, message)) = &self.error {
            return Err(NativeError::new(kind, message));
        }
        Ok(())
    }
    pub fn committed(&mut self, records: &[JsonValue]) {
        if self.error.is_some() {
            return;
        }
        let result = (|| {
            for record in records {
                let (id, entry) = compact(record)?;
                self.insert(id, entry)?;
            }
            self.generation = self.generation.checked_add(1).ok_or_else(storage)?;
            if self.ready {
                self.sorted();
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(error);
        }
    }
}
pub struct CollectionListQuery {
    pub search: String,
    pub hide_nsfw: bool,
    pub page: u32,
    pub size: u32,
}
impl CollectionListQuery {
    pub fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(v, &["search", "hideNsfw", "page", "pageSize"])?;
        let (page, size) = page(v)?;
        let search = v["search"]
            .as_str()
            .filter(|s| s.len() <= 512 && !s.contains('\0'))
            .ok_or_else(NativeError::params)?;
        let hide_nsfw = v["hideNsfw"].as_bool().ok_or_else(NativeError::params)?;
        Ok(Self {
            search: search.trim().to_lowercase(),
            hide_nsfw,
            page,
            size,
        })
    }
}
pub struct CollectionListScan {
    pub query: CollectionListQuery,
    generation: u64,
    position: usize,
    total: u64,
    selected: Vec<String>,
}
#[derive(Clone)]
pub struct CollectionQuery {
    pub id: String,
    pub page: u32,
    pub size: u32,
}
pub fn page(v: &JsonValue) -> Result<(u32, u32), NativeError> {
    let page = v["page"]
        .as_u32()
        .filter(|n| *n < 12500)
        .ok_or_else(NativeError::params)?;
    let size = v["pageSize"]
        .as_u32()
        .filter(|n| (1..=50).contains(n))
        .ok_or_else(NativeError::params)?;
    Ok((page, size))
}
impl CollectionQuery {
    pub fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(v, &["collectionId", "page", "pageSize", "refresh"])?;
        let (page, size) = page(v)?;
        if !v["refresh"].is_boolean() {
            return Err(NativeError::params());
        }
        Ok(Self {
            id: p::positive(&v["collectionId"])?,
            page,
            size,
        })
    }
}
#[derive(Clone)]
pub struct MemberCrawl {
    pub value: JsonValue,
}
impl MemberCrawl {
    pub fn id(&self) -> &str {
        self.value["collectionId"].as_str().unwrap()
    }
    pub fn next_page(&self) -> u32 {
        self.value["page"].as_u32().unwrap()
    }
    pub fn token(&self) -> &str {
        self.value["crawl"].as_str().unwrap()
    }
}
fn number(s: &str) -> Result<u64, NativeError> {
    c::u64_text(&s.into()).map_err(|_| storage())
}
fn header_contract(mut header: JsonValue) -> Result<JsonValue, NativeError> {
    if !header.is_object() {
        return Err(storage());
    }
    for name in [
        "headerFetchedAt",
        "descriptionFetchedAt",
        "descriptionRevision",
    ] {
        if !header.has_key(name) {
            header[name] = JsonValue::Null;
        }
    }
    if !header.has_key("contentRatingKnown") {
        header["contentRatingKnown"] = false.into();
    }
    header["textFormat"] = "plain".into();
    header["description"] = p::plain_blocks(header["description"].as_str().ok_or_else(storage)?).into();
    // Native description carries the chosen text; the legacy markup slot stays empty.
    header["text"] = "".into();
    c::exact(
        &header,
        &[
            "collectionId",
            "name",
            "description",
            "text",
            "profileUrl",
            "coverUrl",
            "author",
            "authorRemoteId",
            "avatarUrl",
            "itemCount",
            "likes",
            "isNsfw",
            "contentRatingKnown",
            "remoteAddedAt",
            "remoteUpdatedAt",
            "itemsSyncedAt",
            "images",
            "headerFetchedAt",
            "descriptionFetchedAt",
            "descriptionRevision",
            "textFormat",
        ],
    )
    .map_err(|_| storage())?;
    Ok(header)
}
impl CatalogStore {
    pub fn collection_header(&self, id: &str) -> Result<Option<JsonValue>, NativeError> {
        self.db
            .query(
                "SELECT payload FROM collection WHERE collection_id=?1",
                &[id],
            )
            .map_err(|_| storage())?
            .map(|r| {
                header_contract(
                    p::parse_bounded(r[0].as_bytes(), 512 * 1024).map_err(|_| storage())?,
                )
            })
            .transpose()
    }
    /// Builds one bounded slice. Errors belong to collection search, while
    /// ordinary cache reads keep their existing ownership/readability checks.
    pub fn projection_step(&mut self) {
        if self.collection_projection.ready || self.collection_projection.error.is_some() {
            return;
        }
        let result = (|| {
            let rows=self.db.query_rows("SELECT collection_id,payload,expected_count FROM collection WHERE collection_id>?1 ORDER BY collection_id LIMIT 8",&[&self.collection_projection.cursor],8,4*1024*1024+1024).map_err(|e|if matches!(e.kind.as_str(),"catalogResponseTooLarge"|"catalogIncomplete"){e}else{storage()})?;
            let finished = rows.len() < 8;
            for row in rows {
                let header =
                    p::parse_bounded(row[1].as_bytes(), 512 * 1024).map_err(|_| storage())?;
                let (id, entry) = compact(&header)?;
                if id != row[0] || entry.count != number(&row[2])? {
                    return Err(storage());
                }
                self.collection_projection.cursor = id.clone();
                self.collection_projection.insert(id, entry)?;
            }
            if finished {
                self.collection_projection.sorted();
                self.collection_projection.generation = self
                    .collection_projection
                    .generation
                    .checked_add(1)
                    .ok_or_else(storage)?;
                self.collection_projection.ready = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.collection_projection.fail(error);
        }
    }
    pub fn start_collection_list(
        &self,
        v: &JsonValue,
    ) -> Result<Option<CollectionListScan>, NativeError> {
        let query = CollectionListQuery::parse(v)?;
        self.collection_projection.check()?;
        if !self.collection_projection.ready {
            return Ok(None);
        }
        Ok(Some(CollectionListScan {
            query,
            generation: self.collection_projection.generation,
            position: 0,
            total: 0,
            selected: Vec::new(),
        }))
    }
    pub fn scan_collection_list(&self, scan: &mut CollectionListScan) -> Result<bool, NativeError> {
        self.collection_projection.check()?;
        if !self.collection_projection.ready
            || scan.generation != self.collection_projection.generation
        {
            return Err(c::error("catalogIncomplete"));
        }
        let end = std::cmp::min(scan.position + 128, self.collection_projection.order.len());
        let offset = scan.query.page as u64 * scan.query.size as u64;
        for id in &self.collection_projection.order[scan.position..end] {
            let entry = &self.collection_projection.entries[id];
            if scan.query.hide_nsfw && (!entry.content_known || entry.nsfw) {
                continue;
            }
            if !scan.query.search.is_empty()
                && !entry.name.to_lowercase().contains(&scan.query.search)
                && !entry.author.to_lowercase().contains(&scan.query.search)
            {
                continue;
            }
            if scan.total >= offset && scan.selected.len() < scan.query.size as usize {
                scan.selected.push(id.clone());
            }
            scan.total += 1;
        }
        scan.position = end;
        Ok(end == self.collection_projection.order.len())
    }
    pub fn collection_list(&self, scan: CollectionListScan) -> Result<ResourceResult, NativeError> {
        if scan.generation != self.collection_projection.generation
            || !self.collection_projection.ready
            || scan.position != self.collection_projection.order.len()
        {
            return Err(c::error("catalogIncomplete"));
        }
        let page = scan.query.page;
        let size = scan.query.size;
        let rows = if scan.selected.is_empty() {
            Vec::new()
        } else {
            let placeholders = (1..=scan.selected.len())
                .map(|n| format!("?{n}"))
                .collect::<Vec<_>>()
                .join(",");
            self.db.query_rows(&format!("SELECT payload,expected_count,remote_updated_at,coalesce(items_synced_at,''),coalesce(items_count,''),coalesce(items_version,''),(SELECT count(*) FROM collection_member m WHERE m.collection_id=collection.collection_id) FROM collection WHERE collection_id IN ({placeholders}) ORDER BY expected_count DESC,collection_id"),&scan.selected.iter().map(String::as_str).collect::<Vec<_>>(),50,800*1024)?
        };
        if rows.len() != scan.selected.len() {
            return Err(storage());
        }
        let mut items = Vec::new();
        let mut fetched = None;
        let mut stale = false;
        for r in rows {
            let header = header_contract(
                p::parse_bounded(r[0].as_bytes(), 512 * 1024).map_err(|_| storage())?,
            )?;
            let header_time = header["headerFetchedAt"].as_u64();
            fetched = match (fetched, header_time) {
                (Some(a), Some(b)) => Some(std::cmp::min(a, b)),
                (_, None) => None,
                (None, Some(t)) if items.is_empty() => Some(t),
                (None, Some(_)) => None,
            };
            stale |= header_time.is_none_or(|t| now().saturating_sub(t) >= 21600);
            let expected = number(&r[1])?;
            let version = number(&r[2])?;
            let count = number(&r[6])?;
            let time = if r[3].is_empty() {
                None
            } else {
                Some(number(&r[3])?)
            };
            let saved_count = if r[4].is_empty() {
                None
            } else {
                Some(number(&r[4])?)
            };
            let saved_version = if r[5].is_empty() {
                None
            } else {
                Some(number(&r[5])?)
            };
            let member_stale = time.is_none_or(|t| now().saturating_sub(t) >= MEMBER_TTL)
                || saved_count != Some(expected)
                || saved_version != Some(version);
            items.push(object!{header:header,expectedCount:expected.to_string(),fetchedMembershipCount:count.to_string(),lastMemberSyncAt:time,memberHeaderRevision:saved_version,membershipStale:member_stale,membershipIncomplete:time.is_none()||count!=expected,availabilityChecked:false});
        }
        let total = scan.total;
        let cursor = self
            .state("collection_cursor")?
            .map(|s| super::sync::CollectionSync::parse_cursor(&s))
            .transpose()?;
        let complete = cursor.as_ref().is_some_and(|v| v.complete);
        if items.is_empty() {
            fetched = self.state_time("last_collection_sync_at")?;
            stale = fetched.is_none_or(|t| now().saturating_sub(t) >= 21600);
        }
        Ok(ResourceResult {
            data: object! {items:items,total:total.to_string(),page:page,pageSize:size},
            fetched,
            incomplete: !complete,
            stale,
        })
    }

    pub fn collection_page(
        &self,
        q: &CollectionQuery,
        prefs: &PreferenceSnapshot,
    ) -> Result<ResourceResult, NativeError> {
        let Some(header) = self.collection_header(&q.id)? else {
            return Ok(ResourceResult {
                data: object! {state:"missing",header:JsonValue::Null,members:Vec::<JsonValue>::new(),page:q.page,pageSize:q.size,expectedCount:JsonValue::Null,fetchedMembershipCount:"0",lastMemberSyncAt:JsonValue::Null,memberHeaderRevision:JsonValue::Null,membershipStale:true,membershipIncomplete:true,unavailableOrMissingMemberCount:JsonValue::Null,descriptionFetchedAt:JsonValue::Null,descriptionStale:true,preferencesRevision:prefs.revision.to_string(),favorites:Vec::<JsonValue>::new()},
                fetched: None,
                incomplete: true,
                stale: true,
            });
        };
        let row=self.db.query("SELECT expected_count,remote_updated_at,coalesce(items_synced_at,''),coalesce(items_count,''),coalesce(items_version,''),(SELECT count(*) FROM collection_member m WHERE m.collection_id=collection.collection_id) FROM collection WHERE collection_id=?1",&[&q.id]).map_err(|_|storage())?.ok_or_else(storage)?;
        let expected = number(&row[0])?;
        let version = number(&row[1])?;
        let count = number(&row[5])?;
        if count > 3750 || version > 253402300799 {
            return Err(storage());
        }
        let time = if row[2].is_empty() {
            None
        } else {
            Some(number(&row[2])?)
        };
        let saved_count = if row[3].is_empty() {
            None
        } else {
            Some(number(&row[3])?)
        };
        let saved_version = if row[4].is_empty() {
            None
        } else {
            Some(number(&row[4])?)
        };
        let stale = time.is_none_or(|t| now().saturating_sub(t) >= MEMBER_TTL)
            || saved_count != Some(expected)
            || saved_version != Some(version);
        let rows=self.db.query_rows(&format!("SELECT position,provider,submission_type,submission_id FROM collection_member WHERE collection_id=?1 ORDER BY position LIMIT {} OFFSET {}",q.size,q.page as u64*q.size as u64),&[&q.id],50,16*1024)?;
        let mut members = Vec::new();
        let mut member_bytes = 0usize;
        let mut favorites = Vec::new();
        let mut incomplete = time.is_none() || count != expected || saved_count != Some(count);
        let unresolved=self.db.query("SELECT count(*) FROM collection_member m LEFT JOIN submission s ON m.provider='gamebanana' AND s.provider=m.provider AND s.submission_type=m.submission_type AND s.submission_id=m.submission_id LEFT JOIN visibility_status v ON v.provider=m.provider AND v.submission_type=m.submission_type AND v.submission_id=m.submission_id WHERE m.collection_id=?1 AND (m.provider!='gamebanana' OR s.submission_id IS NULL OR s.tombstoned!=0 OR v.status IS NOT NULL)",&[&q.id]).map_err(|_|storage())?.ok_or_else(storage)?;
        let unresolved = number(&unresolved[0])?;
        incomplete |= unresolved > 0;
        for r in rows {
            let position = number(&r[0])?;
            if r[1] == "native-placeholder" {
                incomplete = true;
                members.push(object!{position:position.to_string(),submission:JsonValue::Null,state:"unidentified",reason:if r[2]=="private"{"catalogPrivate"}else{"unsupportedOrRedacted"},summary:JsonValue::Null});
                continue;
            }
            let key=SubmissionKeyV1::parse(&object!{provider:"gamebanana",submissionType:r[2].clone(),submissionId:r[3].clone()}).map_err(|_|storage())?;
            if !matches!(r[1].as_str(), "gamebanana" | "native-private-member") {
                return Err(storage());
            }
            let unavailable = if r[1] == "native-private-member" {
                Some("catalogPrivate".into())
            } else {
                self.unavailable(&key)?
            };
            let summary = if unavailable.is_none() {
                self.get(&key)?
            } else {
                None
            };
            let state = if unavailable.is_some() {
                "unavailable"
            } else if summary.is_none() {
                "missing"
            } else {
                "ready"
            };
            incomplete |= state != "ready";
            favorites.push(object! {submission:key.json(),favorite:prefs.favorites.contains(&key)});
            let member = object! {position:position.to_string(),submission:key.json(),state:state,reason:unavailable,summary:summary};
            member_bytes = member_bytes
                .checked_add(member.dump().len())
                .ok_or_else(storage)?;
            members.push(member);
            if member_bytes > 850 * 1024 {
                return Err(c::error("catalogResponseTooLarge"));
            }
        }
        let desc_time = header["descriptionFetchedAt"].as_u64();
        let desc_stale = desc_time.is_none_or(|t| now().saturating_sub(t) >= MEMBER_TTL)
            || header["descriptionRevision"].as_u64() != Some(version);
        let header_time = header["headerFetchedAt"].as_u64();
        Ok(ResourceResult {
            data: object! {state:"ready",header:header,members:members,page:q.page,pageSize:q.size,expectedCount:expected.to_string(),fetchedMembershipCount:count.to_string(),lastMemberSyncAt:time,memberHeaderRevision:saved_version,membershipStale:stale,membershipIncomplete:incomplete,unavailableOrMissingMemberCount:unresolved.to_string(),descriptionFetchedAt:desc_time,descriptionStale:desc_stale,preferencesRevision:prefs.revision.to_string(),favorites:favorites},
            fetched: time,
            incomplete: incomplete || desc_stale,
            stale: stale
                || desc_stale
                || header_time.is_none_or(|t| now().saturating_sub(t) >= 21600),
        })
    }
    pub fn open_members(&self, id: &str) -> Result<MemberCrawl, NativeError> {
        let row = self
            .db
            .query(
                "SELECT expected_count,remote_updated_at FROM collection WHERE collection_id=?1",
                &[id],
            )
            .map_err(|_| storage())?
            .ok_or_else(|| c::error("catalogNotFound"))?;
        let expected = number(&row[0])?;
        let revision = number(&row[1])?;
        if expected > 3750 {
            return Err(c::error("catalogIncomplete"));
        }
        if let Some(raw) = self.state(&format!("members:{id}"))? {
            if !crate::protocol::integer_numbers(raw.as_bytes()) {
                return Err(storage());
            }
            let v = p::parse_bounded(raw.as_bytes(), 4096).map_err(|_| storage())?;
            c::exact(
                &v,
                &[
                    "crawl",
                    "collectionId",
                    "page",
                    "seen",
                    "total",
                    "expectedCount",
                    "headerRevision",
                ],
            )
            .map_err(|_| storage())?;
            c::token(&v["crawl"]).map_err(|_| storage())?;
            if v["collectionId"] != id || v["page"].as_u32().is_none_or(|n| !(2..=250).contains(&n))
            {
                return Err(storage());
            }
            let seen = c::u64_text(&v["seen"]).map_err(|_| storage())?;
            let old_expected = c::u64_text(&v["expectedCount"]).map_err(|_| storage())?;
            let old_version = c::u64_text(&v["headerRevision"]).map_err(|_| storage())?;
            let total = if v["total"].is_null() {
                None
            } else {
                Some(c::u64_text(&v["total"]).map_err(|_| storage())?)
            };
            let page = v["page"].as_u32().unwrap();
            if old_version > 253402300799
                || old_expected > 3750
                || seen > old_expected
                || total.is_some_and(|t| t != old_expected)
                || page == 1 && (seen != 0 || total.is_some())
                || page > 1
                    && (seen < (page as u64 - 1)
                        || seen > 15 * (page as u64 - 1)
                        || total.is_none())
            {
                return Err(storage());
            }
            if old_expected == expected && old_version == revision {
                let staged = self
                    .db
                    .query(
                        "SELECT count(*),coalesce(min(position),0),coalesce(max(position),0) FROM member_staging WHERE crawl=?1 AND collection_id=?2",
                        &[v["crawl"].as_str().unwrap(), id],
                    )
                    .map_err(|_| storage())?
                    .ok_or_else(storage)?;
                if number(&staged[0])? != seen
                    || number(&staged[1])? != 0
                    || seen > 0 && number(&staged[2])? != seen - 1
                {
                    return Err(storage());
                }
                return Ok(MemberCrawl { value: v });
            }
        }
        Ok(MemberCrawl {
            value: object! {crawl:c::new_token()?,collectionId:id,page:1,seen:"0",total:JsonValue::Null,expectedCount:expected.to_string(),headerRevision:revision.to_string()},
        })
    }
    pub fn stage_members(
        &mut self,
        crawl: &mut MemberCrawl,
        page: u32,
        members: Vec<JsonValue>,
        total: u64,
        complete: bool,
    ) -> Result<bool, NativeError> {
        let id = crawl.id().to_owned();
        let seen = c::u64_text(&crawl.value["seen"])?;
        let expected = c::u64_text(&crawl.value["expectedCount"])?;
        let next = seen.checked_add(members.len() as u64).ok_or_else(storage)?;
        if page != crawl.next_page()
            || members.len() > 15
            || total != expected
            || next > total
            || complete && next != total
            || !complete && (members.is_empty() || page >= 250)
        {
            return Err(p::invalid());
        }
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        let current = tx
            .db
            .query(
                "SELECT expected_count,remote_updated_at FROM collection WHERE collection_id=?1",
                &[&id],
            )
            .map_err(|_| storage())?
            .ok_or_else(storage)?;
        if current[0] != expected.to_string()
            || current[1] != crawl.value["headerRevision"].as_str().unwrap()
        {
            return Err(c::error("catalogIncomplete"));
        }
        if page == 1 {
            tx.db
                .execute("DELETE FROM member_staging WHERE collection_id=?1", &[&id])
                .map_err(|_| storage())?;
        }
        for (offset, m) in members.iter().enumerate() {
            let position = (seen + offset as u64).to_string();
            let (provider, t, key) = if m["submission"].is_null() {
                (
                    "native-placeholder".to_owned(),
                    if m["reason"] == "catalogPrivate" {
                        "private"
                    } else {
                        "unknown"
                    }
                    .to_owned(),
                    position.clone(),
                )
            } else {
                let k = SubmissionKeyV1::parse(&m["submission"])?;
                (
                    if m["state"] == "unavailable" {
                        "native-private-member"
                    } else {
                        "gamebanana"
                    }
                    .into(),
                    k.submission_type,
                    k.submission_id,
                )
            };
            // Enforce identity uniqueness across pages even when visibility changed.
            if provider!="native-placeholder"&&tx.db.query("SELECT 1 FROM member_staging WHERE crawl=?1 AND collection_id=?2 AND submission_type=?3 AND submission_id=?4",&[crawl.token(),&id,&t,&key]).map_err(|_|storage())?.is_some(){return Err(p::invalid());}
            tx.db.execute("INSERT INTO member_staging(crawl,collection_id,position,provider,submission_type,submission_id) VALUES(?1,?2,?3,?4,?5,?6)",&[crawl.token(),&id,&position,&provider,&t,&key]).map_err(|_|storage())?;
        }
        let mut successor = crawl.value.clone();
        successor["page"] = (page + 1).into();
        successor["seen"] = next.to_string().into();
        successor["total"] = total.to_string().into();
        if complete {
            tx.db
                .execute(
                    "DELETE FROM collection_member WHERE collection_id=?1",
                    &[&id],
                )
                .map_err(|_| storage())?;
            tx.db.execute("INSERT INTO collection_member SELECT collection_id,position,provider,submission_type,submission_id FROM member_staging WHERE crawl=?1 AND collection_id=?2 ORDER BY position",&[crawl.token(),&id]).map_err(|_|storage())?;
            tx.db.execute("UPDATE collection SET items_synced_at=?2,items_count=?3,items_version=?4 WHERE collection_id=?1",&[&id,&now().to_string(),&expected.to_string(),crawl.value["headerRevision"].as_str().unwrap()]).map_err(|_|storage())?;
            tx.db
                .execute("DELETE FROM member_staging WHERE collection_id=?1", &[&id])
                .map_err(|_| storage())?;
            tx.db
                .execute(
                    "DELETE FROM catalog_state WHERE name=?1",
                    &[&format!("members:{id}")],
                )
                .map_err(|_| storage())?;
        } else {
            tx.db.execute("INSERT INTO catalog_state(name,value) VALUES(?1,?2) ON CONFLICT(name) DO UPDATE SET value=excluded.value",&[&format!("members:{id}"),&successor.dump()]).map_err(|_|storage())?;
        }
        tx.db
            .execute(
                "UPDATE catalog_state SET value=?1 WHERE name='revision'",
                &[&revision.to_string()],
            )
            .map_err(|_| storage())?;
        tx.commit().map_err(|_| storage())?;
        self.revision = revision;
        crawl.value = successor;
        Ok(complete)
    }
    pub fn save_description(
        &mut self,
        id: &str,
        version: &JsonValue,
        description: &JsonValue,
    ) -> Result<(), NativeError> {
        let mut header = self
            .collection_header(id)?
            .ok_or_else(|| c::error("catalogNotFound"))?;
        if header["remoteUpdatedAt"] != *version {
            return Err(c::error("catalogIncomplete"));
        }
        header["description"] = description["description"].clone();
        header["text"] = "".into();
        header["descriptionFetchedAt"] = now().into();
        header["descriptionRevision"] = version.as_u64().unwrap_or(0).into();
        header["textFormat"] = "plain".into();
        if !c::bounded(&header, 512 * 1024) {
            return Err(c::error("catalogResponseTooLarge"));
        }
        self.admit()?;
        let revision = self.revision.checked_add(1).ok_or_else(storage)?;
        let tx = self.db.transaction().map_err(|_| storage())?;
        tx.db
            .execute(
                "UPDATE collection SET payload=?2 WHERE collection_id=?1",
                &[id, &header.dump()],
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
        Ok(())
    }
}
