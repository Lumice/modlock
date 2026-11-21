//! Adapted 2026-10-28 from pinned upstream catalog/query.rs, GPL-3.0.
//! SQL uses bound values; FTS retains quoted AND-prefix terms and bm25 ordering.
use super::{
    contract as c,
    preferences::PreferenceSnapshot,
    provider::positive,
    store::{storage, CatalogStore},
};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::collections::BTreeSet;
const QUERY_KEYS: &[&str] = &[
    "search",
    "categories",
    "heroes",
    "authorRemoteId",
    "excludeFilters",
    "isAudio",
    "isMap",
    "hideNsfw",
    "nsfwOnly",
    "hideObsolete",
    "updatedAfter",
    "addedAfter",
    "addedBefore",
    "favoritesOnly",
    "includeWips",
    "submissionType",
    "sort",
    "page",
    "pageSize",
];
const SCOPE_KEYS: &[&str] = &[
    "authorRemoteId",
    "isAudio",
    "isMap",
    "includeWips",
    "submissionType",
];
#[derive(Clone)]
pub struct CatalogQueryV1 {
    pub value: JsonValue,
}
pub struct CatalogPageV1 {
    pub data: JsonValue,
}
impl CatalogQueryV1 {
    pub fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(v, QUERY_KEYS)?;
        validate_fields(v)?;
        Ok(Self { value: v.clone() })
    }
    pub fn defaults() -> JsonValue {
        object! {search:"",categories:Vec::<String>::new(),heroes:Vec::<String>::new(),authorRemoteId:JsonValue::Null,excludeFilters:false,isAudio:JsonValue::Null,isMap:JsonValue::Null,hideNsfw:false,nsfwOnly:false,hideObsolete:false,updatedAfter:JsonValue::Null,addedAfter:JsonValue::Null,addedBefore:JsonValue::Null,favoritesOnly:false,includeWips:false,submissionType:JsonValue::Null,sort:"default",page:0,pageSize:50}
    }
    pub fn scope(v: &JsonValue, authors: bool) -> Result<Self, NativeError> {
        let mut keys = SCOPE_KEYS.to_vec();
        if authors {
            keys.extend(["search", "hideNsfw"]);
        }
        c::exact(v, &keys)?;
        let mut d = Self::defaults();
        for (k, v) in v.entries() {
            d[k] = v.clone();
        }
        validate_fields(&d)?;
        Ok(Self { value: d })
    }
}
fn validate_fields(v: &JsonValue) -> Result<(), NativeError> {
    if !c::typed(v)
        || v["search"]
            .as_str()
            .is_none_or(|s| s.len() > 512 || s.contains('\0'))
    {
        return Err(NativeError::params());
    }
    for key in ["categories", "heroes"] {
        let mut set = BTreeSet::new();
        if !v[key].is_array() || v[key].len() > 64 {
            return Err(NativeError::params());
        }
        for s in v[key].members() {
            let s = s
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 128 && !s.contains('\0'))
                .ok_or_else(NativeError::params)?;
            if !set.insert(s) {
                return Err(NativeError::params());
            }
        }
    }
    if !v["authorRemoteId"].is_null() {
        positive(&v["authorRemoteId"])?;
    }
    for k in [
        "excludeFilters",
        "hideNsfw",
        "nsfwOnly",
        "hideObsolete",
        "favoritesOnly",
        "includeWips",
    ] {
        if !v[k].is_boolean() {
            return Err(NativeError::params());
        }
    }
    for k in ["isAudio", "isMap"] {
        if !v[k].is_null() && !v[k].is_boolean() {
            return Err(NativeError::params());
        }
    }
    if v["hideNsfw"] == true && v["nsfwOnly"] == true {
        return Err(NativeError::params());
    }
    for k in ["updatedAfter", "addedAfter", "addedBefore"] {
        if !v[k].is_null() && v[k].as_u64().is_none_or(|n| n > 253402300799) {
            return Err(NativeError::params());
        }
    }
    if v["addedAfter"]
        .as_u64()
        .zip(v["addedBefore"].as_u64())
        .is_some_and(|(a, b)| a > b)
    {
        return Err(NativeError::params());
    }
    if !v["submissionType"].is_null()
        && !matches!(v["submissionType"].as_str(), Some("mod" | "sound" | "wip"))
    {
        return Err(NativeError::params());
    }
    if !matches!(
        v["sort"].as_str(),
        Some("default" | "lastUpdated" | "downloadCount" | "rating" | "releaseDate")
    ) || v["page"].as_u32().is_none_or(|n| n > 1_000_000)
        || v["pageSize"]
            .as_u32()
            .is_none_or(|n| !(1..=100).contains(&n))
    {
        return Err(NativeError::params());
    }
    Ok(())
}
pub fn validate_operation(op: &JsonValue) -> Result<(), NativeError> {
    let args = &op["args"];
    match op["kind"].as_str() {
        Some("query") => {
            CatalogQueryV1::parse(args)?;
        }
        Some("facets") => {
            CatalogQueryV1::scope(args, false)?;
        }
        Some("authors") => {
            CatalogQueryV1::scope(args, true)?;
        }
        Some("favoritesGet") => {
            c::exact(args, &["submission"])?;
            if !args["submission"].is_null() {
                super::provider::SubmissionKeyV1::parse(&args["submission"])?;
            }
        }
        Some("favoriteSet") => {
            c::exact(args, &["submission", "desired", "expectedRevision"])?;
            super::provider::SubmissionKeyV1::parse(&args["submission"])?;
            if !args["desired"].is_boolean() {
                return Err(NativeError::params());
            }
            c::u64_text(&args["expectedRevision"])?;
        }
        Some("sync") => {
            if args.has_key("clearCache") {
                c::exact(args, &["forceRefresh", "forceReconcile", "clearCache"])?;
                if args["clearCache"] != true || args["forceRefresh"] != true || args["forceReconcile"] != true { return Err(NativeError::params()); }
            } else { c::exact(args, &["forceRefresh", "forceReconcile"])?; }
            if !args["forceRefresh"].is_boolean() || !args["forceReconcile"].is_boolean() {
                return Err(NativeError::params());
            }
        }
        Some(kind @ ("detail" | "files" | "comments" | "changelog")) => {
            super::details::ResourceQuery::parse(kind, args)?;
        }
        Some("collection") => {
            super::collections::CollectionQuery::parse(args)?;
        }
        Some("collections") => {
            super::collections::CollectionListQuery::parse(args)?;
        }
        _ => return Err(NativeError::params()),
    }
    Ok(())
}
pub fn fts_query(s: &str) -> Option<String> {
    let terms = s
        .split_whitespace()
        .map(|s| format!("\"{}\"*", s.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}
struct Filter {
    sql: String,
    params: Vec<String>,
}
impl Filter {
    fn add(&mut self, expr: &str, value: impl ToString) {
        self.params.push(value.to_string());
        self.sql
            .push_str(&format!(" AND {expr}?{}", self.params.len()));
    }
    fn values(&mut self, column: &str, values: &[String]) -> String {
        let mut p = Vec::new();
        for v in values {
            self.params.push(v.clone());
            p.push(format!("?{}", self.params.len()));
        }
        format!("{column} IN ({})", p.join(","))
    }
    fn refs(&self) -> Vec<&str> {
        self.params.iter().map(String::as_str).collect()
    }
}
fn filter(q: &CatalogQueryV1, prefs: Option<&PreferenceSnapshot>, search: Option<&str>) -> Filter {
    let v = &q.value;
    let mut f = Filter {
        sql: "s.tombstoned=0".into(),
        params: Vec::new(),
    };
    if let Some(t) = v["submissionType"].as_str() {
        f.add("s.submission_type=", t);
    } else if v["includeWips"] != true {
        f.sql.push_str(" AND s.submission_type!='wip'");
    }
    if let Some(search) = search {
        f.params.push(search.into());
        f.sql.push_str(&format!(" AND (s.provider,s.submission_type,s.submission_id) IN (SELECT provider,submission_type,submission_id FROM submission_fts WHERE submission_fts MATCH ?{})",f.params.len()));
    }
    if let Some(a) = v["authorRemoteId"].as_str() {
        f.add("s.author_remote_id=", a);
    }
    let exclude = v["excludeFilters"] == true;
    let cats = v["categories"]
        .members()
        .filter_map(JsonValue::as_str)
        .collect::<Vec<_>>();
    if !cats.is_empty() {
        let named = cats
            .iter()
            .filter(|s| **s != "Other/Misc")
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut expr = if named.is_empty() {
            "0".into()
        } else {
            f.values("s.category", &named)
        };
        if cats.contains(&"Other/Misc") {
            expr=format!("({expr} OR s.category NOT IN ('Maps','Skins','Gameplay Modifications','HUD','Model Replacement','Music','Abilities','Weapons','VOs','Killsounds','Killstreak Music'))");
        }
        f.sql.push_str(&format!(
            " AND {}({expr})",
            if exclude { "NOT " } else { "" }
        ));
    }
    let heroes = v["heroes"]
        .members()
        .filter_map(JsonValue::as_str)
        .collect::<Vec<_>>();
    if !heroes.is_empty() {
        let named = heroes
            .iter()
            .filter(|s| **s != "None")
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut expr = if named.is_empty() {
            "0".into()
        } else {
            f.values("s.hero", &named)
        };
        if heroes.contains(&"None") {
            expr = format!("({expr} OR s.hero='')");
        }
        f.sql.push_str(&format!(
            " AND {}({expr})",
            if exclude { "NOT " } else { "" }
        ));
    }
    for (key, column) in [("isAudio", "s.is_audio="), ("isMap", "s.is_map=")] {
        if let Some(b) = v[key].as_bool() {
            f.add(column, if b { 1 } else { 0 });
        }
    }
    for (key, expr) in [
        ("hideNsfw", "s.is_nsfw=0"),
        ("nsfwOnly", "s.is_nsfw=1"),
        ("hideObsolete", "s.is_obsolete=0"),
    ] {
        if v[key] == true {
            f.sql.push_str(&format!(" AND {expr}"));
        }
    }
    for (key, col) in [
        ("updatedAfter", "s.updated_at>="),
        ("addedAfter", "s.added_at>="),
        ("addedBefore", "s.added_at<"),
    ] {
        if let Some(n) = v[key].as_u64() {
            f.add(col, n);
        }
    }
    if v["favoritesOnly"] == true {
        let ids = prefs
            .map(|p| {
                p.favorites
                    .iter()
                    .map(|k| format!("'{}'", k.slug()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        f.sql.push_str(&format!(
            " AND s.slug IN ({})",
            if ids.is_empty() {
                "NULL".into()
            } else {
                ids.join(",")
            }
        ));
    }
    f
}
impl CatalogStore {
    pub fn query(&mut self, q: &CatalogQueryV1) -> Result<CatalogPageV1, NativeError> {
        self.query_with_preferences(q, None)
    }
    pub fn query_with_preferences(
        &mut self,
        q: &CatalogQueryV1,
        prefs: Option<&PreferenceSnapshot>,
    ) -> Result<CatalogPageV1, NativeError> {
        let search = fts_query(q.value["search"].as_str().ok_or_else(NativeError::params)?);
        if search.is_some() && !self.search_available()? {
            return Err(self.search_unavailable_error());
        }
        let f = filter(q, prefs, search.as_deref());
        let total = self
            .db
            .query(
                &format!("SELECT count(*) FROM submission s WHERE {}", f.sql),
                &f.refs(),
            )
            .map_err(|_| storage())?
            .ok_or_else(storage)?[0]
            .parse::<u64>()
            .map_err(|_| storage())?;
        let page = q.value["page"].as_u64().ok_or_else(NativeError::params)?;
        let size = q.value["pageSize"]
            .as_u64()
            .ok_or_else(NativeError::params)?;
        let offset = page.checked_mul(size).ok_or_else(NativeError::params)?;
        let mut params = f.params.clone();
        let (join, order) = if q.value["sort"] == "default" && search.is_some() {
            params.push(search.unwrap());
            (format!(" JOIN submission_fts ON submission_fts.provider=s.provider AND submission_fts.submission_type=s.submission_type AND submission_fts.submission_id=s.submission_id AND submission_fts MATCH ?{}",params.len()),"bm25(submission_fts) ASC".to_string())
        } else {
            (
                String::new(),
                match q.value["sort"].as_str() {
                    Some("lastUpdated") => "s.updated_at DESC",
                    Some("rating") => "s.likes DESC",
                    Some("releaseDate") => "s.added_at DESC",
                    _ => "s.download_count DESC",
                }
                .into(),
            )
        };
        let rows=self.db.query_rows(&format!("SELECT s.payload FROM submission s{join} WHERE {} ORDER BY {order},s.slug ASC,s.provider,s.submission_type,s.submission_id LIMIT {size} OFFSET {offset}",f.sql),&params.iter().map(String::as_str).collect::<Vec<_>>(),100,900*1024).map_err(|e|if matches!(e.kind.as_str(),"catalogResponseTooLarge"|"catalogIncomplete"){e}else{storage()})?;
        let items = rows
            .into_iter()
            .map(|r| json::parse(&r[0]).map_err(|_| storage()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CatalogPageV1 {
            data: object! {items:items,total:total.to_string(),page:page,pageSize:size},
        })
    }
    pub fn facets(&self, q: &CatalogQueryV1) -> Result<JsonValue, NativeError> {
        let f = filter(q, None, None);
        let rows=self.db.query_rows(&format!("SELECT DISTINCT s.category,s.hero FROM submission s WHERE {} ORDER BY s.category,s.hero LIMIT 513",f.sql),&f.refs(),512,192*1024)?;
        let mut result=Vec::new();
        for r in rows {
            let artwork=if r[1].is_empty() {None} else {
                self.db.query("SELECT payload FROM submission WHERE hero=?1 AND category='Skins' AND tombstoned=0 ORDER BY CAST(download_count AS INTEGER) DESC,slug ASC LIMIT 1", &[r[1].as_str()])?
            };
            let payload=artwork.and_then(|r|json::parse(&r[0]).ok()).unwrap_or(JsonValue::Null);
            result.push(object!{category:r[0].clone(),hero:if r[1].is_empty(){JsonValue::Null}else{r[1].clone().into()},thumbnail:payload["thumbnailUrl"].as_str().unwrap_or(""),masked:payload["isNsfw"]==true || payload["visibilityKnown"]!=true});
        }
        Ok(JsonValue::from(result))
    }
    pub fn authors(&self, q: &CatalogQueryV1) -> Result<JsonValue, NativeError> {
        let Some(search) = fts_query(q.value["search"].as_str().unwrap_or("")) else {
            return Ok(JsonValue::new_array());
        };
        if !self.search_available()? {
            return Err(self.search_unavailable_error());
        }
        let mut scope = q.clone();
        scope.value["authorRemoteId"] = JsonValue::Null;
        let f = filter(&scope, None, None);
        let matching = filter(&scope, None, Some(&format!("author : ({search})")));
        let mut params = f.params.clone();
        let shift = params.len();
        let mut matches = matching.sql;
        for n in (1..=matching.params.len()).rev() {
            matches = matches.replace(&format!("?{n}"), &format!("?{}#", n + shift));
        }
        matches = matches.replace('#', "");
        params.extend(matching.params);
        let name_scope = f.sql.replace("s.", "z.");
        let sql=format!("SELECT s.author_remote_id,(SELECT z.author FROM submission z WHERE z.author_remote_id=s.author_remote_id AND {name_scope} ORDER BY z.updated_at DESC,z.slug LIMIT 1),count(*),sum(s.download_count) FROM submission s WHERE {} AND s.author_remote_id!='' AND s.author_remote_id IN (SELECT s.author_remote_id FROM submission s WHERE {matches}) GROUP BY s.author_remote_id ORDER BY sum(s.download_count) DESC,2,s.author_remote_id LIMIT 5",f.sql);
        let rows = self
            .db
            .query_rows(
                &sql,
                &params.iter().map(String::as_str).collect::<Vec<_>>(),
                5,
                16 * 1024,
            )
            .map_err(|_| storage())?;
        Ok(JsonValue::from(rows.into_iter().map(|r|object!{authorRemoteId:r[0].clone(),name:r[1].clone(),submissionCount:r[2].parse::<u64>().unwrap_or(0),downloadCount:r[3].clone()}).collect::<Vec<_>>()))
    }
}
