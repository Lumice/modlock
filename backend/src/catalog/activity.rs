//! Adapted 2026-10-28 from pinned GameBanana activity.rs/client.rs, GPL-3.0.
//! Read-only paged native text; never a posting or reply API.
use super::provider::{self as p, SubmissionKeyV1};
use crate::native::NativeError;
use json::{object, JsonValue};
use std::collections::BTreeSet;

pub fn normalize(
    key: &SubmissionKeyV1,
    kind: &str,
    page: u32,
    v: &JsonValue,
) -> Result<JsonValue, NativeError> {
    if kind == "comments" && v["_sErrorCode"] == "COMMENT_MODE_HIDDEN" {
        return Ok(
            object! {submission:key.json(),kind:kind,page:page,pageSize:15,state:"hidden",items:Vec::<JsonValue>::new(),total:JsonValue::Null,hasMore:false,textFormat:"plain"},
        );
    }
    if v.has_key("error") || v.has_key("error_code") || v.has_key("_sErrorCode") {
        return Err(p::invalid());
    }
    let size = if kind == "comments" { 15 } else { 20 };
    let records = &v["_aRecords"];
    let total = v["_aMetadata"]["_nRecordCount"]
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(p::invalid)?;
    let complete = v["_aMetadata"]["_bIsComplete"]
        .as_bool()
        .ok_or_else(p::invalid)?;
    if !records.is_array()
        || records.len() > size
        || v["_aMetadata"]["_nPerpage"]
            .as_u64()
            .is_some_and(|n| n > size as u64)
    {
        return Err(p::invalid());
    }
    let consumed = (page as u64 - 1) * size as u64 + records.len() as u64;
    if consumed > total || complete && consumed != total || !complete && records.len() == 0 {
        return Err(p::invalid());
    }
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for r in records.members() {
        let id = p::row_id(r)?;
        if !seen.insert(id.clone()) {
            return Err(p::invalid());
        }
        if kind == "changelog"
            && (p::optional_bool(r, "_bIsPrivate", false)?
                || p::optional_bool(r, "_bIsTrashed", false)?)
        {
            continue;
        }
        let markup = p::text(r, "_sText", 128 * 1024)?;
        let (links, links_truncated) = super::links::project(&markup);
        let text = p::plain_blocks(&markup);
        if kind == "comments" {
            let poster = &r["_aPoster"];
            if !poster.is_null() && !poster.is_object() {
                return Err(p::invalid());
            }
            let score = if r["_nStampScore"].is_null() {
                0
            } else {
                r["_nStampScore"].as_i64().ok_or_else(p::invalid)?
            };
            let pin = if r["_iPinLevel"].is_null() {
                0
            } else {
                r["_iPinLevel"].as_i64().ok_or_else(p::invalid)?
            };
            items.push(object!{commentId:id,text:text,links:links,linksTruncated:links_truncated,createdAt:p::timestamp(&r["_tsDateAdded"]),score:score.to_string(),replyCount:p::count(r,"_nReplyCount")?.to_string(),pinned:pin>0,author:object!{id:poster["_idRow"].as_u64().filter(|n|*n>0).map(|n|n.to_string()),name:p::plain(&p::text(poster,"_sName",1024)?),avatarUrl:gb_url(&poster["_sAvatarUrl"])?,profileUrl:gb_url(&poster["_sProfileUrl"])?,title:p::plain(&p::text(poster,"_sUserTitle",1024)?)} });
        } else {
            let changes = &r["_aChangeLog"];
            if !changes.is_null() && !changes.is_array() || changes.len() > 128 {
                return Err(p::invalid());
            }
            let changes=changes.members().map(|e|Ok(object!{category:p::plain(&p::text(e,"cat",128)?),text:p::plain(&p::text(e,"text",4096)?)})).collect::<Result<Vec<_>,NativeError>>()?;
            items.push(object!{updateId:id,name:p::plain(&p::text(r,"_sName",1024)?),version:if r["_sVersion"].is_null(){JsonValue::Null}else{p::plain(&p::text(r,"_sVersion",256)?).into()},text:text,links:links,linksTruncated:links_truncated,createdAt:p::timestamp(&r["_tsDateAdded"]),changes:changes});
        }
    }
    let out = object! {submission:key.json(),kind:kind,page:page,pageSize:size,state:"ready",items:items,total:total.to_string(),hasMore:!complete,textFormat:"plain"};
    if !super::contract::bounded(&out, 512 * 1024) {
        return Err(super::contract::error("catalogResponseTooLarge"));
    }
    Ok(out)
}
fn gb_url(v: &JsonValue) -> Result<JsonValue, NativeError> {
    let u = p::optional_url(v)?;
    if let Some(s) = u.as_str() {
        let host = s
            .strip_prefix("https://")
            .unwrap_or("")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if host != "gamebanana.com" && !host.ends_with(".gamebanana.com") {
            return Err(p::invalid());
        }
    }
    Ok(u)
}
