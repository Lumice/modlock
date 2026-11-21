//! Native detail/files read contracts, adapted 2026-10-28, upstream GPL-3.0.
use super::{
    contract as c,
    provider::SubmissionKeyV1,
    store::{now, storage, CatalogStore},
};
use crate::native::NativeError;
use json::{object, JsonValue};
pub struct CachedResource {
    pub payload: JsonValue,
    pub fetched: u64,
    pub complete: bool,
}
pub struct ResourceResult {
    pub data: JsonValue,
    pub fetched: Option<u64>,
    pub incomplete: bool,
    pub stale: bool,
}
#[derive(Clone)]
pub struct ResourceQuery {
    pub kind: String,
    pub key: SubmissionKeyV1,
    pub page: u32,
}
impl ResourceQuery {
    pub fn parse(kind: &str, v: &JsonValue) -> Result<Self, NativeError> {
        c::exact(
            v,
            if kind == "detail" {
                &["submission", "refresh"]
            } else {
                &["submission", "page", "refresh"]
            },
        )?;
        if !matches!(kind, "detail" | "files" | "comments" | "changelog")
            || !v["refresh"].is_boolean()
        {
            return Err(NativeError::params());
        }
        let page = if kind == "detail" {
            1
        } else {
            v["page"]
                .as_u32()
                .filter(|n| (1..=if kind == "files" { 7 } else { 500 }).contains(n))
                .ok_or_else(NativeError::params)?
        };
        Ok(Self {
            kind: kind.into(),
            key: SubmissionKeyV1::parse(&v["submission"])?,
            page,
        })
    }
    pub fn identity(&self) -> String {
        if self.kind == "detail" {
            self.key.slug()
        } else if self.kind == "files" {
            format!("{}:all", self.key.slug())
        } else {
            format!("{}:{}", self.key.slug(), self.page)
        }
    }
}
impl CatalogStore {
    pub fn read_resource(&self, q: &ResourceQuery) -> Result<ResourceResult, NativeError> {
        if let Some(status) = self.unavailable(&q.key)? {
            let checked = self.unavailable_checked_at(&q.key)?;
            return Ok(ResourceResult {
                data: object! {state:"unavailable",reason:status,submission:q.key.json(),resource:JsonValue::Null},
                fetched: Some(checked),
                incomplete: false,
                stale: now().saturating_sub(checked) >= 3600,
            });
        }
        let cached = if q.kind == "comments" {
            match self.resource("comments", &format!("{}:hidden", q.key.slug()))? {
                Some(row) => Some(row),
                None => self.resource(&q.kind, &q.identity())?,
            }
        } else {
            self.resource(&q.kind, &q.identity())?
        };
        if let Some(row) = cached {
            let mut payload = row.payload;
            let mut links_incomplete = false;
            let mut legacy_activity = false;
            if q.kind == "detail" {
                if SubmissionKeyV1::parse(&payload["summary"]["submission"])
                    .map_err(|_| storage())?
                    != q.key
                {
                    return Err(storage());
                }
                let description = payload["summary"]["description"]
                    .as_str()
                    .ok_or_else(storage)?;
                payload["summary"]["description"] = super::provider::plain(description).into();
            }
            // Earlier normalized profiles retained markup for storage only.
            // Never send that field across the native rendering boundary.
            if q.kind == "detail" {
                let legacy_markup = payload.remove("descriptionMarkup");
                if !legacy_markup.is_null()
                    || !payload.has_key("links")
                    || !payload.has_key("linksTruncated")
                {
                    return Ok(ResourceResult {
                        data: object! {state:"partial",reason:JsonValue::Null,submission:q.key.json(),resource:object!{summary:payload["summary"].clone(),textFormat:"plain"}},
                        fetched: Some(row.fetched),
                        incomplete: true,
                        stale: now().saturating_sub(row.fetched) >= 3600,
                    });
                }
                if !payload.has_key("description") {
                    payload["description"] = payload["summary"]["description"].clone();
                }
                payload["textFormat"] = "plain".into();
                payload.remove("files");
                if !payload.has_key("richDescription") {
                    payload["richDescription"] = JsonValue::new_array();
                }
                c::exact(
                    &payload,
                    &[
                        "summary",
                        "description",
                        "richDescription",
                        "links",
                        "linksTruncated",
                        "textFormat",
                        "public",
                        "contentWarnings",
                        "dependencies",
                        "metadata",
                        "sourcePin",
                    ],
                )
                .map_err(|_| storage())?;
                if payload["public"] != true
                    || payload["sourcePin"] != super::provider::SOURCE_PIN
                    || !payload["description"].is_string()
                    || !payload["contentWarnings"].is_array()
                    || !payload["dependencies"].is_array()
                {
                    return Err(storage());
                }
                super::links::validate(&payload)?;
                links_incomplete = payload["linksTruncated"] == true;
                c::exact(&payload["metadata"], &["donationLinks", "mapName"])
                    .map_err(|_| storage())?;
            }
            if matches!(q.kind.as_str(), "comments" | "changelog") {
                c::exact(
                    &payload,
                    &[
                        "submission",
                        "kind",
                        "page",
                        "pageSize",
                        "state",
                        "items",
                        "total",
                        "hasMore",
                        "textFormat",
                    ],
                )
                .map_err(|_| storage())?;
                if SubmissionKeyV1::parse(&payload["submission"]).map_err(|_| storage())? != q.key
                    || payload["kind"] != q.kind.as_str()
                    || !payload["items"].is_array()
                    || !payload["hasMore"].is_boolean()
                    || payload["textFormat"] != "plain"
                    || !matches!(payload["state"].as_str(), Some("ready" | "hidden"))
                {
                    return Err(storage());
                }
            }
            if matches!(q.kind.as_str(), "comments" | "changelog") {
                for index in 0..payload["items"].len() {
                    let entry = &mut payload["items"][index];
                    if !entry.is_object() {
                        return Err(storage());
                    }
                    if !entry.has_key("links") || !entry.has_key("linksTruncated") {
                        entry["links"] = JsonValue::new_array();
                        entry["linksTruncated"] = true.into();
                        legacy_activity = true;
                    }
                    super::links::validate(entry)?;
                    links_incomplete |= entry["linksTruncated"] == true;
                }
            }
            if q.kind == "comments" && payload["state"] == "hidden" {
                payload["page"] = q.page.into();
            }
            if q.kind == "files" {
                if !payload.is_array() {
                    return Err(storage());
                }
                if payload.len() > 128 {
                    return Err(storage());
                }
                for file in payload.members() {
                    c::exact(
                        file,
                        &[
                            "fileId",
                            "name",
                            "size",
                            "createdAt",
                            "downloadUrl",
                            "md5Checksum",
                            "description",
                            "isArchived",
                        ],
                    )
                    .map_err(|_| storage())?;
                    super::provider::positive(&file["fileId"]).map_err(|_| storage())?;
                    c::u64_text(&file["size"]).map_err(|_| storage())?;
                    if !file["name"].is_string()
                        || !file["description"].is_string()
                        || !file["isArchived"].is_boolean()
                    {
                        return Err(storage());
                    }
                }
                let items = payload
                    .members()
                    .skip((q.page as usize - 1) * 20)
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>();
                payload = object! {submission:q.key.json(),items:items,total:payload.len().to_string(),page:q.page,pageSize:20,metadataOnly:true};
            }
            return Ok(ResourceResult {
                data: object! {state:"ready",reason:JsonValue::Null,submission:q.key.json(),resource:payload},
                fetched: Some(row.fetched),
                incomplete: !row.complete || links_incomplete,
                stale: legacy_activity || now().saturating_sub(row.fetched) >= 3600,
            });
        }
        let fallback = if q.kind == "detail" {
            self.get(&q.key)?.map(|mut summary| {
                summary["description"] =
                    super::provider::plain(summary["description"].as_str().unwrap_or("")).into();
                summary
            })
        } else {
            None
        };
        let fetched = if fallback.is_some() {
            self.summary_age(&q.key)?
        } else {
            None
        };
        Ok(ResourceResult {
            data: object! {state:if fallback.is_some(){"partial"}else{"missing"},reason:JsonValue::Null,submission:q.key.json(),resource:fallback.map(|s|object!{summary:s,textFormat:"plain"})},
            fetched,
            incomplete: true,
            stale: fetched.is_none_or(|t| now().saturating_sub(t) >= 3600),
        })
    }
}
