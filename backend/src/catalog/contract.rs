//! Version 1 common wire contract. Domain producers validate their own args/data.
use crate::native::NativeError;
use json::{object, JsonValue};
use sha2::{Digest, Sha256};
use std::path::{Component, PathBuf};

pub const FRAME_LIMIT: usize = 65_536;
pub const CONTROL_LIMIT: usize = 16_384;
pub const TERMINAL_LIMIT: usize = 1_048_576;
pub const WRAPPER_RESERVE: usize = 128;
pub const RECEIPTS: usize = 8;
pub const SESSION_KEYS: usize = 512;
pub const RESULT_STORAGE: usize = 8 * 1024 * 1024;
pub const BODY_LIMIT: usize = 5 * 1024 * 1024;
pub const PIPE_LIMIT: usize = 256 * 1024;
pub const STDERR_LIMIT: usize = 16 * 1024;
pub const OPERATIONS: &[&str] = &["query", "facets", "authors", "detail", "files", "comments", "changelog", "collections", "collection", "sync", "favoritesGet", "favoriteSet"];
pub const TERMINAL_ERRORS: &[&str] = &["catalogCancelled", "catalogUnavailable", "catalogStorage", "catalogFull", "catalogIncomplete", "catalogSearchUnavailable", "catalogPrivate", "catalogNotFound", "catalogInvalidProviderResponse", "catalogResponseTooLarge", "catalogRateLimited", "catalogRefused", "catalogTransport", "catalogStalePreferences"];
pub const TRANSPORT_ERRORS: &[&str] = &["transportTimeout", "transportTls", "transportNetwork", "transportRedirect", "transportBodyLimit", "transportStorage", "transportProtocol", "catalogCancelled"];

pub fn exact(value: &JsonValue, keys: &[&str]) -> Result<(), NativeError> {
    if !value.is_object() || value.len() != keys.len() || keys.iter().any(|k| !value.has_key(k)) { return Err(NativeError::params()); }
    Ok(())
}
pub fn token(value: &JsonValue) -> Result<&str, NativeError> {
    value.as_str().filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).ok_or_else(NativeError::params)
}
pub fn u64_text(value: &JsonValue) -> Result<u64, NativeError> {
    let s = value.as_str().ok_or_else(NativeError::params)?;
    if s.is_empty() || s.len() > 20 || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit()) { return Err(NativeError::params()); }
    s.parse().map_err(|_| NativeError::params())
}
pub fn root(value: &JsonValue) -> Result<PathBuf, NativeError> {
    let s = value.as_str().filter(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0')).ok_or_else(NativeError::params)?;
    let p = PathBuf::from(s);
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) { return Err(NativeError::params()); }
    Ok(p.components().collect())
}
/// getrandom is a bounded kernel call, not dispatch-thread filesystem access.
pub fn new_token() -> Result<String, NativeError> {
    unsafe extern "C" { fn getrandom(buf: *mut u8, count: usize, flags: u32) -> isize; }
    let mut bytes = [0u8; 16];
    if unsafe { getrandom(bytes.as_mut_ptr(), bytes.len(), 1) } != 16 { return Err(NativeError::new("catalogUnavailable", "Cannot allocate a catalog session identity.")); }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
pub fn digest(value: &JsonValue) -> String { format!("{:x}", Sha256::digest(crate::store::canonical(value).as_bytes())) }
/// Reject float fields even if the legacy parser can coerce them to integers.
pub fn typed(value: &JsonValue) -> bool {
    match value {
        JsonValue::Number(n) => { let (_, _, exponent) = n.as_parts(); exponent == 0 },
        JsonValue::Array(a) => a.iter().all(typed),
        JsonValue::Object(o) => o.iter().all(|(_, v)| typed(v)),
        _ => true,
    }
}
pub fn validate(method: &str, p: &JsonValue) -> Result<PathBuf, NativeError> {
    let keys: &[&str] = match method {
        "native.catalog.open" => &["version", "dataDir"],
        "native.catalog.begin" => &["version", "dataDir", "catalogSession", "requestKey", "viewGeneration", "operation"],
        "native.catalog.status" | "native.catalog.cancel" | "native.catalog.release" => &["version", "dataDir", "catalogSession", "requestKey"],
        "native.catalog.close" => &["version", "dataDir", "catalogSession"],
        _ => return Err(NativeError::new("methodNotFound", "Unknown catalog method.")),
    };
    exact(p, keys)?;
    if p["version"] != 1 || !typed(p) { return Err(NativeError::params()); }
    let root = root(&p["dataDir"])?;
    if method != "native.catalog.open" { token(&p["catalogSession"])?; }
    if p.has_key("requestKey") && !(method == "native.catalog.status" && p["requestKey"].is_null()) { token(&p["requestKey"])?; }
    if method == "native.catalog.begin" {
        u64_text(&p["viewGeneration"])?;
        exact(&p["operation"], &["kind", "args"])?;
        if !p["operation"]["args"].is_object() || !p["operation"]["kind"].as_str().is_some_and(|k| OPERATIONS.contains(&k)) { return Err(NativeError::params()); }
    }
    Ok(root)
}
pub fn failure(kind: &str, message: &str) -> JsonValue {
    let kind = if TERMINAL_ERRORS.contains(&kind) || TRANSPORT_ERRORS.contains(&kind) { kind } else { "catalogUnavailable" };
    let message = if message.len() <= 256 && !message.chars().any(char::is_control) { message } else { "Catalog work could not complete." };
    object! {kind:kind,message:message,retryAfterSeconds:JsonValue::Null}
}
pub fn bounded(value: &JsonValue, budget: usize) -> bool { crate::protocol::native_value_bytes(value).is_some_and(|n| n <= budget) }
pub fn error(kind: &str) -> NativeError { NativeError::new(kind, match kind {
    "catalogBusy" => "Catalog capacity is retained; release a completed request and retry.",
    "catalogRootBusy" => "Close the current catalog session before changing roots.",
    "catalogSessionMismatch" => "This catalog session is no longer current.",
    "catalogKeyConflict" => "This request key is already bound to different or released input.",
    "catalogUnknownRequest" => "The catalog request key is unknown.",
    "catalogRequestPending" => "Wait for a terminal request before releasing it.",
    "catalogSessionExhausted" => "Drain, close and reopen this catalog session before admitting new keys.",
    "responseTooLarge" => "The complete catalog response exceeds its wire limit.",
    "catalogClosed" => "The catalog session is closing or closed.",
    _ => "Catalog is unavailable; Library remains usable.",
}) }

/// Producers supply only this cached, bounded common progress object.
pub const PROGRESS_RESERVE:usize=256;
pub const PROGRESS_STAGES:&[&str]=&["index-mod","index-sound","index-wip","hydrate-mod","hydrate-sound","hydrate-wip","confirm-mod","confirm-sound","confirm-wip","retire-mod","retire-sound","retire-wip","collections"];
pub fn progress(value:&JsonValue)->Result<(),NativeError>{
    exact(value,&["stage","completed","total"])?;
    if !value["stage"].as_str().is_some_and(|s|PROGRESS_STAGES.contains(&s)){return Err(NativeError::params());}
    let completed=u64_text(&value["completed"])?;
    if !value["total"].is_null()&&completed>u64_text(&value["total"])?{return Err(NativeError::params());}
    if !bounded(value,PROGRESS_RESERVE){return Err(NativeError::params());}Ok(())
}
