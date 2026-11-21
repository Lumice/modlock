//! Adapted 2026-10-28 from Deadlock Mod Manager, GPL-3.0 (UPSTREAM-LICENSE).
//! Producer pin 976637e2d63329ffef6f87eca40e2aea681acc94: client/models/normalization.
use super::{contract as c, heroes, transport::MetadataRequest};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::collections::BTreeSet;
pub const SOURCE_PIN: &str = "976637e2d63329ffef6f87eca40e2aea681acc94";
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubmissionKeyV1 {
    pub submission_type: String,
    pub submission_id: String,
}
impl SubmissionKeyV1 {
    pub fn parse(v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(v, &["provider", "submissionType", "submissionId"])?;
        if v["provider"] != "gamebanana" {
            return Err(NativeError::params());
        }
        let t = v["submissionType"]
            .as_str()
            .filter(|t| matches!(*t, "mod" | "sound" | "wip"))
            .ok_or_else(NativeError::params)?;
        let id = positive(&v["submissionId"])?;
        Ok(Self {
            submission_type: t.into(),
            submission_id: id,
        })
    }
    pub fn from_slug(slug: &str) -> Result<Self, NativeError> {
        let (t, id) = if let Some(id) = slug.strip_prefix("snd-") {
            ("sound", id)
        } else if let Some(id) = slug.strip_prefix("wip-") {
            ("wip", id)
        } else {
            ("mod", slug)
        };
        Self::parse(&object! {provider:"gamebanana",submissionType:t,submissionId:id})
    }
    pub fn json(&self) -> JsonValue {
        object! {provider:"gamebanana",submissionType:self.submission_type.clone(),submissionId:self.submission_id.clone()}
    }
    pub fn slug(&self) -> String {
        match self.submission_type.as_str() {
            "sound" => format!("snd-{}", self.submission_id),
            "wip" => format!("wip-{}", self.submission_id),
            _ => self.submission_id.clone(),
        }
    }
    pub fn model(&self) -> &str {
        model(&self.submission_type)
    }
    pub fn source(&self) -> String {
        format!(
            "https://gamebanana.com/{}/{}",
            match self.submission_type.as_str() {
                "sound" => "sounds",
                "wip" => "wips",
                _ => "mods",
            },
            self.submission_id
        )
    }
}
pub fn positive(v: &JsonValue) -> Result<String, NativeError> {
    let n = c::u64_text(v)?;
    if n == 0 {
        return Err(NativeError::params());
    }
    Ok(n.to_string())
}
pub fn model(t: &str) -> &str {
    match t {
        "mod" => "Mod",
        "sound" => "Sound",
        "wip" => "Wip",
        _ => "Collection",
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderRead {
    Index {
        submission_type: String,
        page: u32,
        latest: bool,
    },
    Profile(SubmissionKeyV1),
    Files(SubmissionKeyV1),
    Activity {
        key: SubmissionKeyV1,
        kind: String,
        page: u32,
    },
    CollectionDescription(String),
    Hydrate {
        keys: Vec<SubmissionKeyV1>,
    },
    Collections {
        page: u32,
    },
    Members {
        collection_id: String,
        page: u32,
    },
}
#[derive(Clone)]
pub struct ProviderRecord {
    pub key: SubmissionKeyV1,
    pub data: JsonValue,
    pub detail: Option<JsonValue>,
}
pub enum ProviderDocument {
    Page {
        read: ProviderRead,
        records: Vec<ProviderRecord>,
        total: u64,
        complete: bool,
    },
    Profile(ProviderRecord),
    Files(JsonValue),
    Activity(JsonValue),
    CollectionDescription(JsonValue),
    Hydration {
        keys: Vec<SubmissionKeyV1>,
        values: Vec<JsonValue>,
    },
    Collections {
        page: u32,
        records: Vec<JsonValue>,
        total: u64,
        complete: bool,
    },
    Members {
        collection_id: String,
        page: u32,
        members: Vec<JsonValue>,
        total: u64,
        complete: bool,
    },
}
pub struct Provider;
pub fn invalid() -> NativeError {
    NativeError::new(
        "catalogInvalidProviderResponse",
        "Provider metadata is invalid or does not match the requested identity.",
    )
}
impl Provider {
    pub fn request(read: ProviderRead) -> Result<MetadataRequest, NativeError> {
        let url = match read {
            ProviderRead::Index {
                submission_type,
                page,
                latest,
            } => {
                if !matches!(submission_type.as_str(), "mod" | "sound" | "wip")
                    || !(1..=250).contains(&page)
                {
                    return Err(NativeError::params());
                }
                format!("https://gamebanana.com/apiv11/{}/Index?_nPerpage=50&_aFilters%5BGeneric_Game%5D=20948&_nPage={page}&_sSort={}",model(&submission_type),if latest{"Generic_LatestModified"}else{"Generic_Oldest"})
            }
            ProviderRead::Hydrate { keys } => {
                if keys.is_empty()
                    || keys.len() > 40
                    || keys
                        .iter()
                        .any(|k| k.submission_type != keys[0].submission_type)
                {
                    return Err(NativeError::params());
                }
                let mut url = "https://api.gamebanana.com/Core/Item/Data?".to_string();
                for (i, k) in keys.iter().enumerate() {
                    SubmissionKeyV1::parse(&k.json())?;
                    if i > 0 {
                        url.push('&');
                    }
                    url.push_str(&format!("itemtype%5B%5D={}&itemid%5B%5D={}&fields%5B%5D=name%2Cdownloads%2CCategory%28%29.name%2CRootCategory%28%29.name%2Cdescription%2Ctext%2CFiles%28%29.aFiles%28%29%2CUrl%28%29.sProfileUrl%28%29",k.model(),k.submission_id));
                }
                if url.len() > 7000 {
                    return Err(NativeError::params());
                }
                url
            }
            ProviderRead::Files(k) => {
                SubmissionKeyV1::parse(&k.json())?;
                format!(
                    "https://gamebanana.com/apiv11/{}/{}/DownloadPage",
                    k.model(),
                    k.submission_id
                )
            }
            ProviderRead::Activity { key, kind, page } => {
                SubmissionKeyV1::parse(&key.json())?;
                if !(1..=500).contains(&page) || !matches!(kind.as_str(), "comments" | "changelog")
                {
                    return Err(NativeError::params());
                }
                format!(
                    "https://gamebanana.com/apiv11/{}/{}/{}?_nPage={page}&_nPerpage={}",
                    key.model(),
                    key.submission_id,
                    if kind == "comments" {
                        "Posts"
                    } else {
                        "Updates"
                    },
                    if kind == "comments" { 15 } else { 20 }
                )
            }
            ProviderRead::CollectionDescription(id) => {
                positive(&id.clone().into())?;
                format!("https://api.gamebanana.com/Core/Item/Data?itemtype%5B%5D=Collection&itemid%5B%5D={id}&fields%5B%5D=description%2Ctext")
            }
            ProviderRead::Profile(k) => {
                SubmissionKeyV1::parse(&k.json())?;
                format!(
                    "https://gamebanana.com/apiv11/{}/{}/ProfilePage",
                    k.model(),
                    k.submission_id
                )
            }
            ProviderRead::Collections { page } => {
                if !(1..=250).contains(&page) {
                    return Err(NativeError::params());
                }
                format!("https://gamebanana.com/apiv11/Collection/Index?_nPerpage=50&_aFilters%5BGeneric_Game%5D=20948&_nPage={page}&_sSort=Generic_Oldest")
            }
            ProviderRead::Members {
                collection_id,
                page,
            } => {
                positive(&collection_id.clone().into())?;
                if !(1..=250).contains(&page) {
                    return Err(NativeError::params());
                }
                format!(
                    "https://gamebanana.com/apiv11/Collection/{collection_id}/Items?_nPage={page}"
                )
            }
        };
        Ok(MetadataRequest {
            request_id: c::new_token()?,
            url,
        })
    }
    pub fn normalize(read: ProviderRead, body: &[u8]) -> Result<ProviderDocument, NativeError> {
        let v = parse_bounded(body, c::BODY_LIMIT)?;
        if let ProviderRead::Activity { key, kind, page } = &read {
            return super::activity::normalize(key, kind, *page, &v)
                .map(ProviderDocument::Activity);
        }
        if v.has_key("error") || v.has_key("error_code") || v.has_key("_sErrorCode") {
            return Err(invalid());
        }
        if let ProviderRead::Files(_) = &read {
            public(&v)?;
            if !v.has_key("_aFiles") && !v.has_key("_aArchivedFiles") {
                return Err(invalid());
            }
            let mut files = normalize_files(&v["_aFiles"], false)?;
            for f in normalize_files(&v["_aArchivedFiles"], true)?.members() {
                files.push(f.clone()).map_err(|_| invalid())?;
            }
            if files.len() > 128 {
                return Err(invalid());
            }
            let mut ids = BTreeSet::new();
            for f in files.members() {
                if !ids.insert(f["fileId"].as_str().ok_or_else(invalid)?) {
                    return Err(invalid());
                }
            }
            if !c::bounded(&files, 512 * 1024) {
                return Err(c::error("catalogResponseTooLarge"));
            }
            return Ok(ProviderDocument::Files(files));
        }
        if let ProviderRead::CollectionDescription(id) = &read {
            let fields = if v.is_array() && v.len() == 1 && v[0].is_array() {
                &v[0]
            } else {
                &v
            };
            if !fields.is_array() || fields.len() != 2 {
                return Err(invalid());
            }
            let short = fields[0]
                .as_str()
                .filter(|s| s.len() <= 128 * 1024)
                .ok_or_else(invalid)?;
            let long = fields[1]
                .as_str()
                .filter(|s| s.len() <= 128 * 1024)
                .ok_or_else(invalid)?;
            return Ok(ProviderDocument::CollectionDescription(
                object! {collectionId:id.clone(),description:plain_blocks(if long.is_empty(){short}else{long}),textFormat:"plain"},
            ));
        }
        if let ProviderRead::Hydrate { keys } = &read {
            let entries = if v.is_object() && keys.len() == 1 {
                vec![&v]
            } else {
                if !v.is_array() {
                    return Err(invalid());
                }
                v.members().collect::<Vec<_>>()
            };
            if entries.len() != keys.len() {
                return Err(invalid());
            }
            let mut out = Vec::new();
            for (key, r) in keys.iter().zip(entries) {
                let field = |i: usize, name: &str| -> &JsonValue {
                    if r.is_array() {
                        &r[i]
                    } else {
                        &r[name]
                    }
                };
                if !r.is_array() && !r.is_object() || r.is_array() && r.len() != 8 {
                    return Err(invalid());
                }
                if field(7, "Url().sProfileUrl()").as_str() != Some(key.source().as_str()) {
                    return Err(invalid());
                }
                game(r)?;
                public(r)?;
                let name = field(0, "name")
                    .as_str()
                    .filter(|s| !s.trim().is_empty() && s.len() <= 1024)
                    .ok_or_else(invalid)?;
                let downloads = field(1, "downloads")
                    .as_u64()
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or_else(invalid)?;
                let category = field(2, "Category().name").as_str().unwrap_or("");
                let root = field(3, "RootCategory().name").as_str().unwrap_or("");
                if category.len() > 128 || root.len() > 128 {
                    return Err(invalid());
                }
                let desc = field(4, "description").as_str().unwrap_or("");
                let text = field(5, "text").as_str().unwrap_or("");
                if desc.len() > 128 * 1024 || text.len() > 128 * 1024 {
                    return Err(invalid());
                }
                let files = field(6, "Files().aFiles()");
                if !files.is_array() && !files.is_object() || files.len() > 128 {
                    return Err(invalid());
                }
                let dates = if files.is_array() {
                    files.members().collect::<Vec<_>>()
                } else {
                    files.entries().map(|(_, v)| v).collect()
                };
                let latest = dates
                    .iter()
                    .filter_map(|f| timestamp(&f["_tsDateAdded"]).as_u64())
                    .max();
                out.push(object!{name:name.trim(),downloadCount:downloads.to_string(),category:if !root.trim().is_empty(){root.trim()}else if !category.trim().is_empty(){category.trim()}else{"Other"},hero:heroes::resolve(root,category,name),description:plain_blocks(if text.is_empty(){desc}else{text}),filesUpdatedAt:latest});
            }
            return Ok(ProviderDocument::Hydration {
                keys: keys.clone(),
                values: out,
            });
        }
        if let ProviderRead::Profile(k) = &read {
            return Ok(ProviderDocument::Profile(normalize_record(&v, k, true)?));
        }
        let records = v["_aRecords"].members().collect::<Vec<_>>();
        let max = if matches!(read, ProviderRead::Members { .. }) {
            15
        } else {
            50
        };
        if !v["_aRecords"].is_array() || records.len() > max {
            return Err(invalid());
        }
        let total = v["_aMetadata"]["_nRecordCount"]
            .as_u64()
            .ok_or_else(invalid)?;
        let complete = v["_aMetadata"]["_bIsComplete"]
            .as_bool()
            .ok_or_else(invalid)?;
        if let Some(n) = v["_aMetadata"]["_nPerpage"].as_u64() {
            if n > max as u64 {
                return Err(invalid());
            }
        }
        let mut seen = BTreeSet::new();
        match &read {
            ProviderRead::Collections { page } => {
                let mut out = Vec::new();
                for r in records {
                    let id = row_id(r)?;
                    if !seen.insert(id.clone()) || r["_sModelName"] != "Collection" {
                        return Err(invalid());
                    }
                    public(r)?;
                    game(r)?;
                    let source = format!("https://gamebanana.com/collections/{id}");
                    source_match(r, &source)?;
                    let images = images(r)?;
                    out.push(object!{collectionId:id,name:required(r,"_sName",1024)?,description:"",text:"",profileUrl:source,coverUrl:images.1,author:author(r)?,authorRemoteId:remote_id(&r["_aSubmitter"]["_idRow"]),avatarUrl:optional_url(&r["_aSubmitter"]["_sAvatarUrl"])?,itemCount:collection_item_count(r)?.to_string(),likes:count(r,"_nLikeCount")?.to_string(),isNsfw:optional_bool(r,"_bHasContentRatings",true)?,contentRatingKnown:r["_bHasContentRatings"].is_boolean(),remoteAddedAt:timestamp(&r["_tsDateAdded"]),remoteUpdatedAt:timestamp(&r["_tsDateModified"]),itemsSyncedAt:JsonValue::Null,images:images.0});
                }
                Ok(ProviderDocument::Collections {
                    page: *page,
                    records: out,
                    total,
                    complete,
                })
            }
            ProviderRead::Members {
                collection_id,
                page,
            } => {
                let mut members = Vec::new();
                for r in records {
                    let t = match r["_sModelName"].as_str() {
                        Some("Mod") => Some("mod"),
                        Some("Sound") => Some("sound"),
                        Some("Wip") => Some("wip"),
                        _ => None,
                    };
                    let private = optional_bool(r, "_bIsPrivate", false)?
                        || optional_bool(r, "_bIsTrashed", false)?
                        || optional_bool(r, "_bIsWithheld", false)?;
                    if t.is_none() || r["_idRow"].is_null() {
                        members.push(object!{submission:JsonValue::Null,state:"unidentified",reason:if private{"catalogPrivate"}else{"unsupportedOrRedacted"}});
                        continue;
                    }
                    let k = SubmissionKeyV1 {
                        submission_type: t.unwrap().into(),
                        submission_id: row_id(r)?,
                    };
                    if !seen.insert(k.slug()) {
                        return Err(invalid());
                    }
                    source_match(r, &k.source())?;
                    game(r)?;
                    members.push(object!{submission:k.json(),state:if private{"unavailable"}else{"known"},reason:if private{JsonValue::from("catalogPrivate")}else{JsonValue::Null}});
                }
                Ok(ProviderDocument::Members {
                    collection_id: collection_id.clone(),
                    page: *page,
                    members,
                    total,
                    complete,
                })
            }
            ProviderRead::Index {
                submission_type, ..
            } => {
                let mut out = Vec::new();
                for r in records {
                    let k = SubmissionKeyV1 {
                        submission_type: submission_type.clone(),
                        submission_id: row_id(r)?,
                    };
                    if !seen.insert(k.slug()) {
                        return Err(invalid());
                    }
                    out.push(normalize_record(r, &k, false)?);
                }
                Ok(ProviderDocument::Page {
                    read,
                    records: out,
                    total,
                    complete,
                })
            }
            _ => Err(invalid()),
        }
    }
}
/// Scan before allocation: at most 50,000 structural/value tokens, depth16,
/// string token256KiB. Raw and normalized sizes are separate memory bounds.
pub fn parse_bounded(bytes: &[u8], limit: usize) -> Result<JsonValue, NativeError> {
    if bytes.len() > limit {
        return Err(NativeError::new(
            "catalogResponseTooLarge",
            "Provider metadata exceeds the bounded body size.",
        ));
    }
    let (mut i, mut nodes, mut depth) = (0, 0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                nodes += 1;
                i += 1;
                let start = i;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if b == b'\\' {
                        i += 1;
                    } else if b == b'"' {
                        break;
                    }
                }
                if i - start > 256 * 1024 {
                    return Err(invalid());
                }
            }
            b'{' | b'[' => {
                nodes += 1;
                depth += 1;
                i += 1;
                if depth > 16 {
                    return Err(invalid());
                }
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1).ok_or_else(invalid)?;
                i += 1;
            }
            b',' | b':' => {
                nodes += 1;
                i += 1;
            }
            _ => i += 1,
        }
        if nodes
            > if limit == 1024 * 1024 {
                250_000
            } else {
                50_000
            }
        {
            return Err(invalid());
        }
    }
    if !crate::protocol::unique_keys(bytes) {
        return Err(invalid());
    }
    json::parse(std::str::from_utf8(bytes).map_err(|_| invalid())?).map_err(|_| invalid())
}
pub(crate) fn row_id(v: &JsonValue) -> Result<String, NativeError> {
    v["_idRow"]
        .as_u64()
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .ok_or_else(invalid)
}
fn game(v: &JsonValue) -> Result<(), NativeError> {
    if !v["_aGame"].is_null() && v["_aGame"]["_idRow"].as_u64() != Some(20948) {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn public(v: &JsonValue) -> Result<(), NativeError> {
    for k in ["_bIsPrivate", "_bIsTrashed", "_bIsWithheld"] {
        if !v[k].is_null() && v[k].as_bool().is_none() {
            return Err(invalid());
        }
        if v[k] == true {
            return Err(NativeError::new(
                "catalogPrivate",
                "This submission is no longer public.",
            ));
        }
    }
    Ok(())
}
fn source_match(v: &JsonValue, expected: &str) -> Result<(), NativeError> {
    if !v["_sProfileUrl"].is_null()
        && v["_sProfileUrl"] != ""
        && v["_sProfileUrl"].as_str() != Some(expected)
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn https(s: &str) -> bool {
    if s.len() > 7000 || s.bytes().any(|b| b <= 32 || b >= 127) || s.contains(['\\', '@']) {
        return false;
    }
    let Some(rest) = s.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        && !host.starts_with('.')
        && !host.ends_with('.')
}
pub(crate) fn optional_url(v: &JsonValue) -> Result<JsonValue, NativeError> {
    if v.is_null() || v == "" {
        return Ok(JsonValue::Null);
    }
    let s = v.as_str().filter(|s| https(s)).ok_or_else(invalid)?;
    Ok(s.into())
}
fn required(v: &JsonValue, k: &str, max: usize) -> Result<String, NativeError> {
    Ok(v[k]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= max && !s.contains('\0'))
        .ok_or_else(invalid)?
        .trim()
        .to_owned())
}
pub(crate) fn text(v: &JsonValue, k: &str, max: usize) -> Result<String, NativeError> {
    if v[k].is_null() {
        return Ok(String::new());
    }
    let s = v[k]
        .as_str()
        .filter(|s| s.len() <= max && !s.contains('\0'))
        .ok_or_else(invalid)?;
    Ok(s.into())
}
// The Collection Index can report -1 for an empty collection even when its
// Items endpoint declares a complete page with zero records.
// Keep this exception local: other counts and negative values stay invalid.
fn collection_item_count(v: &JsonValue) -> Result<u64, NativeError> {
    if v["_nItemCount"].as_i64() == Some(-1) {
        Ok(0)
    } else {
        count(v, "_nItemCount")
    }
}
pub(crate) fn count(v: &JsonValue, k: &str) -> Result<u64, NativeError> {
    if v[k].is_null() {
        Ok(0)
    } else {
        v[k].as_u64()
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or_else(invalid)
    }
}
pub fn timestamp(v: &JsonValue) -> JsonValue {
    v.as_u64()
        .filter(|n| *n > 0 && *n <= 253402300799)
        .map(JsonValue::from)
        .unwrap_or(JsonValue::Null)
}
fn remote_id(v: &JsonValue) -> JsonValue {
    v.as_u64()
        .filter(|n| *n > 0)
        .map(|n| JsonValue::from(n.to_string()))
        .unwrap_or(JsonValue::Null)
}
fn author(v: &JsonValue) -> Result<String, NativeError> {
    let s = text(&v["_aSubmitter"], "_sName", 1024)?;
    Ok(if s.trim().is_empty() {
        "Unknown".into()
    } else {
        s.trim().into()
    })
}
fn images(v: &JsonValue) -> Result<(JsonValue, JsonValue), NativeError> {
    let a = &v["_aPreviewMedia"]["_aImages"];
    if !a.is_null() && !a.is_array() {
        return Err(invalid());
    }
    let mut out = JsonValue::new_array();
    let mut thumb = JsonValue::Null;
    // The provider gallery can exceed the 32-image DTO limit. Validate every
    // entry within the bounded provider body, but retain only the first 32.
    for (index, r) in a.members().enumerate() {
        let base = text(r, "_sBaseUrl", 6000)?;
        let file = required(r, "_sFile", 512)?;
        if file.contains(['?', '#']) || file.split('/').any(|p| p == "..") || !https(&base) {
            return Err(invalid());
        }
        let u = format!(
            "{}/{}",
            base.trim_end_matches('/'),
            file.trim_start_matches('/')
        );
        if !https(&u) {
            return Err(invalid());
        }
        if index < 32 {
            out.push(u).map_err(|_| invalid())?;
        }
        if index == 0 {
            if let Some(s) = r["_sFile530"].as_str() {
                if s.contains(['?', '#']) || s.split('/').any(|p| p == "..") {
                    return Err(invalid());
                }
                let u = format!(
                    "{}/{}",
                    base.trim_end_matches('/'),
                    s.trim_start_matches('/')
                );
                thumb = optional_url(&u.into())?;
            }
        }
    }
    Ok((out, thumb))
}
/// Like `plain`, but keeps the text's shape: block tags become line breaks,
/// list items become bullets, and runs of blank space collapse. For
/// descriptions and comments, which are shown as paragraphs.
pub(crate) fn plain_blocks(s: &str) -> String {
    let mut out = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
            tag.clear();
        } else if c == '>' && in_tag {
            in_tag = false;
            let name: String = tag.trim_start_matches('/').chars()
                .take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            let closing = tag.starts_with('/');
            match name.as_str() {
                "br" => out.push('\n'),
                "li" if !closing => out.push_str("\n• "),
                "p" | "div" | "ul" | "ol" | "blockquote" | "tr" | "table" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hr" => out.push_str("\n\n"),
                _ => out.push(' '),
            }
        } else if in_tag {
            if tag.len() < 64 { tag.push(c); }
        } else {
            out.push(c);
        }
    }
    // A stray "<" that never closed was text, not a tag.
    if in_tag { out.push('‹'); out.push_str(&tag); }
    let text = out.replace("&nbsp;", " ").replace("&quot;", "\"").replace("&#39;", "'").replace("&#039;", "'")
        .replace("&apos;", "'").replace("&lt;", "‹").replace("&gt;", "›").replace("&amp;", "&")
        .replace('<', "‹").replace('>', "›");
    // Collapse spaces within lines and blank lines between paragraphs.
    let mut lines: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) { continue; }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) { lines.pop(); }
    lines.join("\n")
}
pub(crate) fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        if c == '<' {
            tag = true;
            out.push(' ');
        } else if c == '>' {
            tag = false;
        } else if !tag {
            out.push(c);
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace('<', "‹")
        .replace('>', "›")
}
pub(crate) fn optional_bool(v: &JsonValue, k: &str, default: bool) -> Result<bool, NativeError> {
    if v[k].is_null() {
        Ok(default)
    } else {
        v[k].as_bool().ok_or_else(invalid)
    }
}
fn optional_container(v: &JsonValue, k: &str, array: bool) -> Result<(), NativeError> {
    if !v[k].is_null()
        && !(if array {
            v[k].is_array()
        } else {
            v[k].is_object()
        })
    {
        return Err(invalid());
    }
    Ok(())
}
fn normalize_record(
    v: &JsonValue,
    k: &SubmissionKeyV1,
    profile: bool,
) -> Result<ProviderRecord, NativeError> {
    if row_id(v)? != k.submission_id
        || (!v["_sModelName"].is_null() && v["_sModelName"].as_str() != Some(k.model()))
        || (!profile && v["_sModelName"].as_str() != Some(k.model()))
    {
        return Err(invalid());
    }
    game(v)?;
    source_match(v, &k.source())?;
    // Profiles have a mandatory public source URL; absent optional visibility is
    // retained as unknown rather than inferred safe in the normalized DTO.
    if profile && v["_sProfileUrl"].as_str() != Some(k.source().as_str()) {
        return Err(invalid());
    }
    public(v)?;
    optional_container(v, "_aPreviewMedia", false)?;
    optional_container(&v["_aPreviewMedia"], "_aMetadata", false)?;
    optional_container(v, "_aSubmitter", false)?;
    optional_container(v, "_aContentRatings", false)?;
    for field in [
        "_aCategory",
        "_aRootCategory",
        "_aSuperCategory",
        "_aSubCategory",
    ] {
        optional_container(v, field, false)?;
    }
    if profile {
        if !v["_aFiles"].is_null() && !v["_aFiles"].is_array() && !v["_aFiles"].is_object() {
            return Err(invalid());
        }
        optional_container(v, "_aRequirements", true)?;
        optional_container(&v["_aSubmitter"], "_aDonationMethods", true)?;
    }
    let name = required(v, "_sName", 1024)?;
    let mut category = "Other".to_string();
    for field in if profile {
        vec!["_aSuperCategory", "_aRootCategory", "_aCategory"]
    } else {
        vec!["_aRootCategory"]
    } {
        let s = text(&v[field], "_sName", 128)?;
        if !s.trim().is_empty() {
            category = s.trim().into();
            break;
        }
    }
    let sub = text(
        &v[if profile {
            "_aCategory"
        } else {
            "_aSubCategory"
        }],
        "_sName",
        128,
    )?;
    let hero = heroes::resolve(&category, &sub, &name);
    let visibility = text(v, "_sInitialVisibility", 64)?;
    let ratings = &v["_aContentRatings"];
    let nsfw = matches!(visibility.as_str(), "hide" | "warn")
        || ratings
            .entries()
            .any(|(key, _)| ["st", "sa", "sc", "ft", "lp", "pn", "nu"].contains(&key));
    let mut tags = JsonValue::new_array();
    let entries = if v["_aTags"].is_object() {
        v["_aTags"].entries().map(|(_, v)| v).collect::<Vec<_>>()
    } else {
        v["_aTags"].members().collect()
    };
    if entries.len() > 64 {
        return Err(invalid());
    }
    for t in entries {
        let s = if let Some(s) = t.as_str() {
            s.to_owned()
        } else {
            format!("{} {}", text(t, "_sTitle", 128)?, text(t, "_sValue", 128)?)
        };
        if s.len() > 256 {
            return Err(invalid());
        }
        if !s.trim().is_empty() {
            tags.push(s.trim()).map_err(|_| invalid())?;
        }
    }
    let (image, thumbnail) = images(v)?;
    let audio = optional_url(&v["_aPreviewMedia"]["_aMetadata"]["_sAudioUrl"])?;
    let added = timestamp(&v["_tsDateAdded"]);
    let updated = timestamp(&v["_tsDateModified"]);
    let added = if added.is_null() {
        updated.clone()
    } else {
        added
    };
    let updated = if updated.is_null() {
        added.clone()
    } else {
        updated
    };
    let markup = if profile {
        let t = text(v, "_sText", 128 * 1024)?;
        if t.is_empty() {
            text(v, "_sDescription", 128 * 1024)?
        } else {
            t
        }
    } else {
        String::new()
    };
    let mut data = object! {submission:k.json(),name:name,description:plain_blocks(&markup),author:author(v)?,authorRemoteId:remote_id(&v["_aSubmitter"]["_idRow"]),profileUrl:k.source(),category:category.clone(),hero:hero,isAudio:k.submission_type=="sound",isMap:k.submission_type=="mod"&&category=="Maps",isNsfw:nsfw||!matches!(visibility.as_str(),"show"|"hide"|"warn"),visibilityKnown:matches!(visibility.as_str(),"show"|"hide"|"warn"),isObsolete:optional_bool(v,"_bIsObsolete",false)?,hasFiles:optional_bool(v,"_bHasFiles",v["_aFiles"].len()>0)?,downloadCount:count(v,"_nDownloadCount")?.to_string(),likes:count(v,"_nLikeCount")?.to_string(),images:image,thumbnailUrl:thumbnail,audioUrl:audio,tags:tags,remoteAddedAt:added,remoteUpdatedAt:updated,filesUpdatedAt:JsonValue::Null,developmentState:if v["_sDevelopmentState"].is_null(){JsonValue::Null}else{text(v,"_sDevelopmentState",128)?.into()},completionPercentage:v["_iCompletionPercentage"].as_u8().map(|n|n.min(100)),hydrated:profile};
    let detail = if profile {
        let files = normalize_files(&v["_aFiles"], false)?;
        data["filesUpdatedAt"] = files
            .members()
            .filter_map(|f| f["createdAt"].as_u64())
            .max()
            .into();
        let mut dependencies = JsonValue::new_array();
        if v["_aRequirements"].len() > 64 {
            return Err(invalid());
        }
        for r in v["_aRequirements"].members() {
            let label = r[0].as_str().unwrap_or("");
            if label.len() > 1024 || r.len() > 16 {
                return Err(invalid());
            }
            let meta = r
                .members()
                .skip(2)
                .map(|s| s.as_str().unwrap_or("").trim().to_lowercase())
                .collect::<Vec<_>>();
            let labels = meta.iter().any(|s| {
                [
                    "required",
                    "recommended",
                    "uninstalled",
                    "absent",
                    "disabled",
                    "off",
                    "false",
                    "installed",
                    "present",
                    "enabled",
                    "on",
                    "true",
                ]
                .contains(&s.as_str())
            });
            if if labels {
                meta.iter().any(|s| {
                    ["uninstalled", "absent", "disabled", "off", "false"].contains(&s.as_str())
                })
            } else {
                meta.get(1)
                    .is_some_and(|s| ["2", "4", "6", "8", "10"].contains(&s.as_str()))
            } {
                continue;
            }
            let level = if labels {
                meta.iter()
                    .find(|s| matches!(s.as_str(), "required" | "recommended"))
                    .cloned()
            } else {
                match meta.first().map(String::as_str) {
                    Some("1") => Some("required".into()),
                    Some("2") => Some("recommended".into()),
                    _ => None,
                }
            };
            let url = optional_url(&r[1])?;
            let remote = url
                .as_str()
                .and_then(|s| s.strip_prefix("https://gamebanana.com/mods/"))
                .and_then(|s| positive(&s.into()).ok());
            dependencies
                .push(object! {label:plain(label),url:url,remoteId:remote,level:level})
                .map_err(|_| invalid())?;
        }
        let mut donation = JsonValue::new_array();
        if v["_aSubmitter"]["_aDonationMethods"].len() > 32 {
            return Err(invalid());
        }
        for d in v["_aSubmitter"]["_aDonationMethods"].members() {
            if d["_bIsUrl"] == true {
                let url = text(d, "_sValue", 7000)?;
                let title = text(d, "_sTitle", 128)?;
                if let Some(link) = donation_link(&url, &title) {
                    if !donation.members().any(|d| d["url"] == link["url"]) {
                        donation.push(link).map_err(|_| invalid())?;
                    }
                }
            }
        }
        for word in markup.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
        {
            if let Some(link) = donation_link(word, "") {
                if donation.len() >= 32 {
                    break;
                }
                if !donation.members().any(|d| d["url"] == link["url"]) {
                    donation.push(link).map_err(|_| invalid())?;
                }
            }
        }
        let (links, links_truncated) = super::links::project(&markup);
        Some(
            object! {summary:data.clone(),description:plain_blocks(&markup),richDescription:super::rich_description::project(&markup),links:links,linksTruncated:links_truncated,textFormat:"plain",public:true,contentWarnings:content_warnings(ratings)?,files:files,dependencies:dependencies,metadata:object!{donationLinks:donation,mapName:extract_map_name(&markup)},sourcePin:SOURCE_PIN},
        )
    } else {
        None
    };
    if !c::bounded(&data, 192 * 1024) || detail.as_ref().is_some_and(|d| !c::bounded(d, 512 * 1024))
    {
        return Err(invalid());
    }
    Ok(ProviderRecord {
        key: k.clone(),
        data,
        detail,
    })
}

// Bounded adaptations of normalization.rs URL/map extraction; only HTTPS is
// exposed by native. Source markup is normalized to native plain text/links.
fn donation_link(value: &str, title: &str) -> Option<JsonValue> {
    let clean = value.trim_end_matches([')', '"', '\'', '>', '.', ',', ';', ':', '!', '?', ']']);
    if !https(clean) {
        return None;
    }
    let rest = clean.strip_prefix("https://")?;
    let rawhost = rest.split('/').next()?;
    let host = rawhost.trim_start_matches("www.").to_lowercase();
    if ![
        "ko-fi.com",
        "patreon.com",
        "buymeacoffee.com",
        "paypal.me",
        "paypal.com",
        "liberapay.com",
        "opencollective.com",
        "github.com",
    ]
    .contains(&host.as_str())
        || host == "github.com" && !rest[rawhost.len()..].starts_with("/sponsors")
    {
        return None;
    }
    let hint = title.to_lowercase();
    let platform = if hint.contains("ko-fi") || hint.contains("kofi") || host == "ko-fi.com" {
        "Ko-fi"
    } else if hint.contains("patreon") || host == "patreon.com" {
        "Patreon"
    } else if hint.contains("buy me a coffee") || host == "buymeacoffee.com" {
        "Buy Me a Coffee"
    } else if hint.contains("paypal") || matches!(host.as_str(), "paypal.me" | "paypal.com") {
        "PayPal"
    } else if host == "liberapay.com" {
        "Liberapay"
    } else if host == "opencollective.com" {
        "Open Collective"
    } else {
        "GitHub Sponsors"
    };
    Some(object! {url:clean,platform:platform})
}
fn extract_map_name(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let mut bare = None;
    for (at, _) in lower.match_indices("map ") {
        if at > 0 && lower.as_bytes()[at - 1].is_ascii_alphanumeric() {
            continue;
        }
        let tail = &lower[at + 4..];
        let name = tail
            .split(|c: char| !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_')
            .next()
            .unwrap_or("");
        if name.len() < 3 || !name.as_bytes()[0].is_ascii_lowercase() {
            continue;
        }
        let prefix = lower[..at].trim_end();
        let quoted = prefix.ends_with(['"', '\'', '`', '>']) || prefix.ends_with("&quot;");
        if quoted {
            return Some(name.into());
        }
        if bare.is_none()
            && ![
                "features",
                "includes",
                "currently",
                "loading",
                "created",
                "queue",
                "using",
                "look",
                "making",
                "overrides",
                "works",
                "featuring",
                "breaking",
                "correctly",
                "again",
            ]
            .contains(&name)
        {
            bare = Some(name.into());
        }
    }
    bare
}

fn content_warnings(v: &JsonValue) -> Result<JsonValue, NativeError> {
    if v.len() > 64 {
        return Err(invalid());
    }
    let mut out = Vec::new();
    for (code, value) in v.entries() {
        if code.len() > 128 {
            return Err(invalid());
        }
        let label = value
            .as_str()
            .filter(|s| s.len() <= 1024)
            .ok_or_else(invalid)?;
        out.push(object! {code:code,label:plain(label)});
    }
    Ok(out.into())
}
pub(crate) fn normalize_files(v: &JsonValue, archived: bool) -> Result<JsonValue, NativeError> {
    if !v.is_null() && !v.is_array() && !v.is_object() || v.len() > 128 {
        return Err(invalid());
    }
    let entries = if v.is_object() {
        v.entries().map(|(_, f)| f).collect::<Vec<_>>()
    } else {
        v.members().collect()
    };
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    for f in entries {
        let id = row_id(f)?;
        if !ids.insert(id.clone()) {
            return Err(invalid());
        }
        let md5 = text(f, "_sMd5Checksum", 32)?;
        if !md5.is_empty() && (md5.len() != 32 || !md5.bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(invalid());
        }
        out.push(object!{fileId:id,name:plain(&text(f,"_sFile",512)?),size:count(f,"_nFilesize")?.to_string(),createdAt:timestamp(&f["_tsDateAdded"]),downloadUrl:optional_url(&f["_sDownloadUrl"])?,md5Checksum:if md5.is_empty(){JsonValue::Null}else{md5.to_ascii_lowercase().into()},description:plain(&text(f,"_sDescription",4096)?),isArchived:archived||optional_bool(f,"_bIsArchived",false)?});
    }
    Ok(out.into())
}

#[cfg(test)]
mod plain_blocks_tests {
    use super::plain_blocks;
    #[test]
    fn keeps_paragraphs_and_lists() {
        let html = "<p>First  line<br>second</p><p></p><ul><li>One</li><li>Two &amp; three</li></ul><h2>Speed</h2>Fast&nbsp;&nbsp;now";
        assert_eq!(plain_blocks(html), "First line\nsecond\n\n• One\n• Two & three\n\nSpeed\n\nFast now");
        assert_eq!(plain_blocks("a <b>bold</b> word"), "a bold word");
        assert_eq!(plain_blocks("x < y"), "x ‹ y");
    }
}
