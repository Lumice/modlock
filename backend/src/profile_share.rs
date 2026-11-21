//! Self-contained, bounded interchange. No service or filesystem authority.
use crate::{
    native::NativeError,
    paths,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::collections::HashSet;
pub const MAX_BYTES: usize = 16 * 1024;
fn invalid() -> NativeError {
    NativeError::new(
        "invalidSharedProfile",
        "Paste a profile code or choose a profile file (at most 16 KiB, 128 mods).",
    )
}
fn keys(v: &JsonValue, a: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len() != a.len() || v.entries().any(|(k, _)| !a.contains(&k)) {
        return Err(invalid());
    }
    Ok(())
}
fn label(v: &JsonValue) -> bool {
    v.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 256
            && !s.chars().any(char::is_control)
            && !s.starts_with(['/', '\\'])
            && !s.contains(':')
    })
}
fn positive(v: &JsonValue) -> bool {
    v.as_str().is_some_and(|s| {
        !s.is_empty()
            && s.len() <= 19
            && !s.starts_with('0')
            && s.bytes().all(|b| b.is_ascii_digit())
            && s.parse::<u64>().is_ok()
    })
}
// zlib is already a platform dependency. Both encoded and expanded input are bounded.
#[link(name = "z")]
unsafe extern "C" {
    fn compress2(
        out: *mut u8,
        len: *mut std::ffi::c_ulong,
        input: *const u8,
        size: std::ffi::c_ulong,
        level: i32,
    ) -> i32;
    fn uncompress2(
        out: *mut u8,
        len: *mut std::ffi::c_ulong,
        input: *const u8,
        size: *mut std::ffi::c_ulong,
    ) -> i32;
}
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut bits, mut count) = (0u32, 0);
    for b in bytes {
        bits = (bits << 8) | *b as u32;
        count += 8;
        while count >= 6 {
            count -= 6;
            out.push(ALPHABET[((bits >> count) & 63) as usize] as char);
        }
    }
    if count > 0 {
        out.push(ALPHABET[((bits << (6 - count)) & 63) as usize] as char);
    }
    out
}
fn compact(v: &JsonValue) -> Result<String, NativeError> {
    let raw = store::canonical(v);
    decode(&raw)?;
    let mut out = vec![0u8; MAX_BYTES + 1024];
    let mut len = out.len() as std::ffi::c_ulong;
    if unsafe { compress2(out.as_mut_ptr(), &mut len, raw.as_ptr(), raw.len() as _, 9) } != 0 {
        return Err(invalid());
    }
    let code = format!("DLMM1-{}", base64(&out[..len as usize]));
    if code.len() > MAX_BYTES {
        return Err(invalid());
    }
    Ok(code)
}
fn expand(code: &str) -> Result<String, NativeError> {
    let Some(encoded) = code.strip_prefix("DLMM1-") else {
        return Ok(code.to_owned());
    };
    let mut bytes = Vec::new();
    let (mut bits, mut count) = (0u32, 0);
    for b in encoded.bytes() {
        let n = ALPHABET.iter().position(|a| *a == b).ok_or_else(invalid)?;
        bits = (bits << 6) | n as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    if base64(&bytes) != encoded {
        return Err(invalid());
    }
    let mut out = vec![0u8; MAX_BYTES + 1];
    let mut len = out.len() as std::ffi::c_ulong;
    let mut input_len = bytes.len() as std::ffi::c_ulong;
    if unsafe { uncompress2(out.as_mut_ptr(), &mut len, bytes.as_ptr(), &mut input_len) } != 0
        || len as usize > MAX_BYTES
        || input_len as usize != bytes.len()
    {
        return Err(invalid());
    }
    out.truncate(len as usize);
    String::from_utf8(out).map_err(|_| invalid())
}
pub fn decode(code: &str) -> Result<JsonValue, NativeError> {
    if code.len() > MAX_BYTES {
        return Err(invalid());
    }
    let expanded = expand(code)?;
    let code = expanded.as_str();
    if code.len() > MAX_BYTES
        || !crate::protocol::unique_keys(code.as_bytes())
        || !crate::protocol::integer_numbers(code.as_bytes())
    {
        return Err(invalid());
    }
    let v = json::parse(code).map_err(|_| invalid())?;
    keys(&v, &["format", "version", "mods", "notShareable"])?;
    if v["format"] != "dlmm-profile"
        || v["version"] != 1
        || !v["mods"].is_array()
        || v["mods"].len() > 128
        || !v["notShareable"].is_array()
        || v["notShareable"].len() > 128
    {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    for m in v["mods"].members() {
        keys(m, &["submission", "fileId", "options", "enabled"])?;
        keys(
            &m["submission"],
            &["provider", "submissionType", "submissionId"],
        )?;
        if m["submission"]["provider"] != "gamebanana"
            || !matches!(
                m["submission"]["submissionType"].as_str(),
                Some("mod" | "sound" | "wip")
            )
            || !positive(&m["submission"]["submissionId"])
            || !(positive(&m["fileId"]) || m["fileId"] == "latest")
            || !m["enabled"].is_boolean()
            || !m["options"].is_array()
            || m["options"].is_empty()
            || m["options"].len() > 99
        {
            return Err(invalid());
        }
        if !seen.insert(store::canonical(
            &object! {submission:m["submission"].clone(),fileId:m["fileId"].clone()},
        )) {
            return Err(invalid());
        }
        let mut names = HashSet::new();
        for n in m["options"].members() {
            if !label(n) || !names.insert(n.as_str().unwrap()) {
                return Err(invalid());
            }
        }
    }
    if v["notShareable"].members().any(|s| !label(s)) {
        return Err(invalid());
    }
    Ok(v)
}
fn payload_choice(e: &crate::options::OptionEntry) -> Option<String> {
    use sha2::{Digest, Sha256};
    let family = crate::package::family_key(e.package.as_ref()?).ok()?;
    Some(format!("payload-{:x}", Sha256::digest(family.as_bytes())))
}
fn reference(doc: &JsonValue, r: &JsonValue, layout: u32) -> Result<JsonValue, NativeError> {
    let receipt = crate::remote::replacement_target::receipt(r);
    if submission(r).is_null() {
        return Err(invalid());
    }
    let c = crate::local_content::option_sources::catalog_for_record(r, layout)?;
    let mut names: Vec<_> = c
        .entries
        .iter()
        .filter(|e| c.selected_ids.contains(&e.id))
        .map(|e| {
            if crate::remote::replacement_target::association_submission(&r["source"]).is_some() {
                payload_choice(e).unwrap_or_else(|| e.option_name.clone())
            } else {
                e.option_name.clone()
            }
        })
        .collect();
    names.sort();
    Ok(
        object! {submission:submission(r),fileId:if positive(&receipt["rootFileId"]){receipt["rootFileId"].clone()}else{doc["_shareResolved"][cache_identity(r,layout)?.as_str()].as_str().unwrap_or("latest").into()},options:JsonValue::from(names),enabled:r["enabled"].clone()},
    )
}
pub fn encode(
    doc: &JsonValue,
    profile: &str,
    include_names: bool,
) -> Result<JsonValue, NativeError> {
    let p = &doc["profiles"][profile];
    let order = crate::profile_edit::ordered_mod_ids(p)?;
    let layout = crate::package::layout_version(doc)?;
    let mut v =
        object! {format:"dlmm-profile",version:1,mods:json::array![],notShareable:json::array![]};
    let mut omitted = 0;
    for id in order {
        let r = p["mods"]
            .members()
            .find(|r| r["source"]["id"] == id)
            .ok_or_else(invalid)?;
        let value = reference(doc, r, layout);
        let valid = value.as_ref().ok().is_some_and(|r| {
            let mut test = v.clone();
            test["mods"] = json::array![r.clone()];
            decode(&test.dump()).is_ok()
        });
        if valid {
            v["mods"].push(value.unwrap()).map_err(|_| invalid())?
        } else {
            omitted += 1;
            if include_names {
                let n = r["source"]["name"].clone();
                if label(&n) {
                    v["notShareable"].push(n).map_err(|_| invalid())?;
                }
            }
        }
    }
    let code = compact(&v)?;
    Ok(
        object! {code:code,omitted:omitted,rows:export_rows(doc, profile, &v),count:order_count(doc,profile)},
    )
}
fn order_count(doc: &JsonValue, profile: &str) -> usize {
    doc["profiles"][profile]["mods"].len()
}
fn export_rows(doc: &JsonValue, profile: &str, shared: &JsonValue) -> JsonValue {
    let mut rows = json::array![];
    if let Ok(order) = crate::profile_edit::ordered_mod_ids(&doc["profiles"][profile]) {
        for id in order {
            if let Some(r) = doc["profiles"][profile]["mods"]
                .members()
                .find(|r| r["source"]["id"] == id)
            {
                let submission = submission(r);
                let reference = shared["mods"]
                    .members()
                    .find(|m| m["submission"] == submission);
                let _ = rows.push(object! {submission:submission.clone(),title:r["source"]["name"].clone(),thumbnailUrl:"",latest:reference.is_some_and(|m|m["fileId"]=="latest"),omitted:reference.is_none()});
            }
        }
    }
    rows
}
fn submission(r: &JsonValue) -> JsonValue {
    crate::remote::replacement_target::association_submission(&r["source"])
        .unwrap_or_else(|| crate::remote::replacement_target::receipt(r)["submission"].clone())
}
fn matching(doc: &JsonValue, w: &JsonValue) -> Option<JsonValue> {
    let layout = crate::package::layout_version(doc).ok()?;
    for (_, p) in doc["profiles"].entries() {
        for r in p["mods"].members() {
            if r["available"] != true {
                continue;
            }
            if submission(r) != w["submission"] {
                continue;
            }
            let receipt = crate::remote::replacement_target::receipt(r);
            let file = if positive(&receipt["rootFileId"]) {
                receipt["rootFileId"].clone()
            } else {
                match cache_identity(r, layout) {
                    Ok(k) => doc["_shareResolved"][k.as_str()].clone(),
                    Err(_) => continue,
                }
            };
            if (w["fileId"] != "latest" && file != w["fileId"])
                || (w["fileId"] == "latest" && !positive(&receipt["rootFileId"]))
            {
                continue;
            }
            let Ok(mut c) = crate::local_content::option_sources::catalog_for_record(r, layout)
            else {
                continue;
            };
            let mut ids = Vec::new();
            let mut exact = true;
            for name in w["options"].members() {
                let entries: Vec<_> = c
                    .entries
                    .iter()
                    .filter(|e| {
                        e.package.is_some()
                            && (e.option_name == name.as_str().unwrap()
                                || payload_choice(e).as_deref() == name.as_str())
                    })
                    .collect();
                if entries.len() != 1 {
                    exact = false;
                    break;
                }
                ids.push(entries[0].id.clone());
            }
            if !exact {
                continue;
            }
            ids.sort();
            c.selected_ids = ids;
            let Ok(options) = c.encode(layout) else {
                continue;
            };
            let mut copy = r.clone();
            copy.remove("packages");
            copy.remove("payloads");
            copy["options"] = options;
            copy["enabled"] = w["enabled"].clone();
            return Some(copy);
        }
    }
    None
}
pub fn review(doc: &JsonValue, code: &str) -> Result<JsonValue, NativeError> {
    let v = decode(code)?;
    let mut rows = json::array![];
    let mut ready = true;
    for (i, m) in v["mods"].members().enumerate() {
        let found = matching(doc, m).is_some();
        ready &= found;
        rows.push(object!{order:i+1,submission:m["submission"].clone(),fileId:m["fileId"].clone(),latest:m["fileId"]=="latest",title:format!("GameBanana mod {}",m["submission"]["submissionId"].as_str().unwrap_or("")),options:m["options"].clone(),status:if found{"inLibrary"}else if doc["profiles"].entries().any(|(_,p)|p["mods"].members().any(|r|{let receipt=crate::remote::replacement_target::receipt(r);receipt["submission"]==m["submission"]&&receipt["rootFileId"]==m["fileId"]})){"unavailable"}else{"downloadRequired"}}).map_err(|_|invalid())?;
    }
    Ok(
        object! {rows:rows,canCreate:ready,notShareable:v["notShareable"].clone(),revision:doc["revision"].clone()},
    )
}
pub fn dispatch(method: &str, p: &JsonValue) -> Result<JsonValue, NativeError> {
    let root = paths::data_root(p)?;
    match method {
        "native.profileShare.export" => {
            keys(p, &["dataDir", "profileId", "includeNames"])?;
            if !p["includeNames"].is_boolean() {
                return Err(invalid());
            }
            let profile = p["profileId"].as_str().ok_or_else(invalid)?;
            let doc = prepare(&root, Store::open(&root)?.document()?, Some(profile))?;
            let mut result = encode(&doc, profile, p["includeNames"] == true)?;
            decorate(&root, &mut result);
            Ok(result)
        }
        "native.profileShare.review" => {
            keys(p, &["dataDir", "code"])?;
            let doc = prepare(&root, Store::open(&root)?.document()?, None)?;
            let mut result = review(&doc, p["code"].as_str().ok_or_else(invalid)?)?;
            decorate(&root, &mut result);
            Ok(result)
        }
        "native.profileShare.create" => {
            keys(
                p,
                &["dataDir", "code", "name", "expectedRevision", "operationId"],
            )?;
            let code = p["code"].as_str().ok_or_else(invalid)?;
            let shared = decode(code)?;
            let name = p["name"]
                .as_str()
                .filter(|s| {
                    !s.trim().is_empty() && s.len() <= 120 && !s.chars().any(char::is_control)
                })
                .ok_or_else(invalid)?;
            let id = p["operationId"]
                .as_str()
                .filter(|s| paths::safe_id(s))
                .ok_or_else(invalid)?;
            let expected = store::revision(p["expectedRevision"].as_str().ok_or_else(invalid)?)?;
            crate::game_access::ensure_game_idle()?;
            let lock = RootLock::acquire(&root)?;
            let store = Store::open_writer(&lock)?;
            let input = crate::native::operation_input(method, p);
            if let Some(r) = store.replay(id, &input)? {
                return Ok(r);
            }
            crate::apply::ensure_idle(&store)?;
            crate::game_access::ensure_game_idle()?;
            let mut doc = prepare(&root, store.document()?, None)?;
            if doc["revision"] != expected.to_string() {
                return Err(NativeError::new(
                    "staleRevision",
                    "Library changed. Review the profile again.",
                ));
            }
            let mut mods = json::array![];
            let mut ids = HashSet::new();
            for (i, m) in shared["mods"].members().enumerate() {
                let mut r = matching(&doc, m).ok_or_else(|| {
                    NativeError::new(
                        "sharedDownloadRequired",
                        "Review downloads for the missing files, then review this profile again.",
                    )
                })?;
                if !ids.insert(r["source"]["id"].as_str().ok_or_else(invalid)?.to_owned()) {
                    return Err(invalid());
                }
                r["source"]["installOrder"] = i.into();
                mods.push(r).map_err(|_| invalid())?;
            }
            let layout = crate::package::layout_version(&doc)?;
            if !mods.is_empty() && doc["storeVersion"].as_u32().unwrap_or(0) < 4 {
                if doc["storeVersion"].as_u32().unwrap_or(0) < 3 {
                    doc["fonts"] =
                        object! {version:1,installed:JsonValue::Null,previous:JsonValue::Null};
                }
                doc["storeVersion"] = 4.into();
                doc["packageVersion"] = layout.into();
            }
            let next = paths::random_id()?;
            doc["profiles"][&next] =
                object! {id:next.clone(),name:name,protected:false,source:object!{},mods:mods};
            doc.remove("_shareResolved");
            store.commit(&lock, expected, id, &input, doc, Some(&next))
        }
        _ => Err(invalid()),
    }
}
// Sharing metadata is kept separately from receipts: it grants no replacement authority.
pub(crate) fn cache_identity(r: &JsonValue, layout: u32) -> Result<String, NativeError> {
    use sha2::{Digest, Sha256};
    let c = crate::local_content::option_sources::catalog_for_record(r, layout)?;
    let hashes: Vec<_> = c
        .entries
        .iter()
        .filter(|e| c.selected_ids.contains(&e.id))
        .filter_map(|e| e.package.as_ref().map(|p| crate::package::family_key(p)))
        .collect::<Result<_, _>>()?;
    Ok(format!(
        "profile-share:{}",
        format!(
            "{:x}",
            Sha256::digest(
                store::canonical(
                    &object! {submission:submission(r),hashes:JsonValue::from(hashes)}
                )
                .as_bytes()
            )
        )
    ))
}
fn exact_file(
    responses: &JsonValue,
    sub: &JsonValue,
    sha: &str,
    files: &JsonValue,
) -> Option<String> {
    let mut found = HashSet::new();
    for response in responses.members() {
        let v = &response["matchedVpk"];
        if response["match"]["matchType"] == "sha256"
            && response["match"]["certainty"] == 100
            && v["sha256"] == sha
            && v["provider"] == "gamebanana"
            && v["submissionType"] == sub["submissionType"]
            && v["submissionId"] == sub["submissionId"]
            && positive(&v["fileId"])
            && files.members().any(|f| f["fileId"] == v["fileId"])
        {
            found.insert(v["fileId"].as_str()?.to_owned());
        }
    }
    if found.len() == 1 {
        found.into_iter().next()
    } else {
        None
    }
}
fn fetch(url: &str, body: Option<String>) -> Option<JsonValue> {
    use std::{
        io::Read,
        process::{Command, Stdio},
    };
    let mut cmd = Command::new("curl");
    cmd.args([
        "--silent",
        "--fail",
        "--proto",
        "=https",
        "--max-time",
        "5",
        "--max-filesize",
        "1048576",
    ]);
    if let Some(body) = body {
        cmd.args(["-H", "Content-Type: application/json", "--data", &body]);
    }
    let mut child = cmd
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut bytes = Vec::new();
    let read = child.stdout.take()?.take(1048577).read_to_end(&mut bytes);
    if bytes.len() > 1048576 {
        let _ = child.kill();
    }
    let status = child.wait().ok()?;
    if read.is_err() || !status.success() || bytes.len() > 1048576 {
        return None;
    }
    json::parse(std::str::from_utf8(&bytes).ok()?).ok()
}
fn prepare(
    root: &std::path::Path,
    mut doc: JsonValue,
    network_profile: Option<&str>,
) -> Result<JsonValue, NativeError> {
    use crate::catalog::{
        provider::{Provider, ProviderDocument, ProviderRead, SubmissionKeyV1},
        store::CatalogStore,
    };
    crate::game_access::ensure_game_idle()?;
    paths::private_dir(&root.join("cache/catalog"))?;
    let mut cache = CatalogStore::open(root)?;
    let layout = crate::package::layout_version(&doc)?;
    let mut resolved = object! {};
    let mut requests = 0;
    let started = std::time::Instant::now();
    for (profile, p) in doc["profiles"].entries() {
        for r in p["mods"].members() {
            let Some(sub) = crate::remote::replacement_target::association_submission(&r["source"])
            else {
                continue;
            };
            let Ok(identity) = cache_identity(r, layout) else {
                continue;
            };
            if resolved.has_key(&identity) {
                continue;
            }
            if let Some(file) = cache
                .state(&identity)?
                .filter(|f| positive(&JsonValue::from(f.as_str())))
            {
                resolved[&identity] = file.into();
                continue;
            }
            resolved[&identity] = "latest".into();
            if network_profile != Some(profile)
                || requests >= 128
                || started.elapsed().as_secs() >= 15
            {
                continue;
            }
            requests += 1;
            let key = SubmissionKeyV1::parse(&sub)?;
            let read = ProviderRead::Files(key.clone());
            let files = cache
                .resource("files", &format!("{}:all", key.slug()))
                .ok()
                .flatten()
                .filter(|r| r.complete)
                .map(|r| r.payload)
                .or_else(|| {
                    let request = Provider::request(read.clone()).ok()?;
                    let raw = fetch(&request.url, None)?;
                    match Provider::normalize(read, raw.dump().as_bytes()).ok()? {
                        ProviderDocument::Files(f) => Some(f),
                        _ => None,
                    }
                });
            let Some(files) = files else { continue };
            let c = crate::local_content::option_sources::catalog_for_record(r, layout)?;
            let mut candidates = HashSet::new();
            let mut all = !c.selected_ids.is_empty();
            for entry in c.entries.iter().filter(|e| c.selected_ids.contains(&e.id)) {
                if started.elapsed().as_secs() >= 15 {
                    all = false;
                    break;
                }
                let Some(package) = &entry.package else {
                    all = false;
                    break;
                };
                if !package.companions.is_empty() {
                    all = false;
                    break;
                }
                let sha = &package.directory.sha256;
                let response = fetch(
                    "https://api.deadlockmods.app/api/v2/vpk-analyse-hashes",
                    Some(object! {sha256:sha.as_str(),contentSignature:""}.dump()),
                );
                match response.and_then(|v| exact_file(&v, &sub, sha, &files)) {
                    Some(f) => {
                        candidates.insert(f);
                    }
                    None => {
                        all = false;
                        break;
                    }
                }
            }
            if all && candidates.len() == 1 {
                let file = candidates.into_iter().next().unwrap();
                cache.set_state(&identity, &file)?;
                resolved[&identity] = file.into();
            }
        }
    }
    doc["_shareResolved"] = resolved;
    Ok(doc)
}
fn decorate(root: &std::path::Path, result: &mut JsonValue) {
    use crate::catalog::{provider::SubmissionKeyV1, store::CatalogStore};
    let Ok(cache) = CatalogStore::open(root) else {
        return;
    };
    for row in result["rows"].members_mut() {
        if row["title"].as_str().is_none() {
            row["title"] = format!(
                "GameBanana mod {}",
                row["submission"]["submissionId"].as_str().unwrap_or("")
            )
            .into();
        }
        let Ok(key) = SubmissionKeyV1::parse(&row["submission"]) else {
            continue;
        };
        if cache.unavailable(&key).ok().flatten().is_some() && row["status"] == "downloadRequired" {
            row["status"] = "unavailable".into();
        }
        if let Ok(Some(summary)) = cache.get(&key) {
            row["title"] = summary["name"].clone();
            if summary["isNsfw"] == false {
                row["thumbnailUrl"] = summary["thumbnailUrl"].clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> JsonValue {
        object! {format:"dlmm-profile",version:1,mods:json::array![object!{submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"123"},fileId:"456",options:json::array!["default"],enabled:true}],notShareable:json::array![]}
    }
    fn document() -> JsonValue {
        use crate::{
            options::{OptionCatalog, OptionEntry},
            package::{FileRef, PackageRef},
        };
        let mut entry = OptionEntry {
            id: "a".repeat(32),
            option_name: "default".into(),
            package: Some(PackageRef {
                directory: FileRef {
                    id: "b".repeat(32),
                    sha256: "c".repeat(64),
                    size: 32,
                    source_name: "pak01_dir.vpk".into(),
                },
                companions: Default::default(),
            }),
            expected_family: None,
            archive_binding: None,
            source_label: "download".into(),
            unavailable_reason: None,
        };
        entry.expected_family = Some(crate::options::ExpectedFamily::from_package(
            entry.package.as_ref().unwrap(),
        ));
        let c = OptionCatalog {
            entries: vec![entry],
            selected_ids: vec!["a".repeat(32)],
            retained_archives: vec![],
            attachments: vec![],
        };
        let r = object! {source:object!{id:"remote-one",name:"Example",installOrder:9,remoteReceipt:object!{submission:sample()["mods"][0]["submission"].clone(),rootFileId:"456"}},available:true,enabled:true,options:c.encode(1).unwrap()};
        object! {storeVersion:4,packageVersion:1,revision:"7",profiles:object!{default:object!{mods:json::array![r,object!{source:object!{id:"local-one",name:"Private name",installOrder:1},enabled:true,available:false}]}}}
    }
    #[test]
    fn profile_share_export_privacy_and_opt_in() {
        let d = document();
        let v = encode(&d, "default", false).unwrap();
        assert_eq!(v["omitted"], 1);
        assert!(!v["code"].as_str().unwrap().contains("Private name"));
        let shared = decode(v["code"].as_str().unwrap()).unwrap();
        assert_eq!(shared["mods"][0]["fileId"], "456");
        let v = encode(&d, "default", true).unwrap();
        assert_eq!(
            decode(v["code"].as_str().unwrap()).unwrap()["notShareable"][0],
            "Private name"
        );
    }
    #[test]
    fn profile_share_existing_choices() {
        let d = document();
        let r = review(&d, &sample().dump()).unwrap();
        assert_eq!(r["rows"][0]["status"], "inLibrary");
        assert_eq!(r["canCreate"], true);
        let mut latest = sample()["mods"][0].clone();
        latest["fileId"] = "latest".into();
        assert!(matching(&d, &latest).is_some());
        let mut wanted = sample()["mods"][0].clone();
        wanted["enabled"] = false.into();
        let copied = matching(&d, &wanted).unwrap();
        assert_eq!(copied["enabled"], false);
        assert_eq!(copied["options"]["selectedIds"][0], "a".repeat(32));
        wanted["options"][0] = "missing".into();
        assert!(matching(&d, &wanted).is_none());
        let mut code = sample();
        code["mods"][0] = wanted;
        let reviewed = review(&d, &code.dump()).unwrap();
        assert_eq!(reviewed["rows"][0]["status"], "unavailable");
        assert_eq!(reviewed["canCreate"], false);
    }
    #[test]
    fn profile_share_rejects_duplicates_and_bounds() {
        let mut v = sample();
        let duplicate = v["mods"][0].clone();
        v["mods"].push(duplicate).unwrap();
        assert!(decode(&v.dump()).is_err());
        assert!(decode(r#"{"format":"dlmm-profile","format":"dlmm-profile","version":1,"mods":[],"notShareable":[]}"#).is_err());
        let mut v = sample();
        v["mods"][0]["fileId"] = "0".into();
        assert!(decode(&v.dump()).is_err());
    }
    #[test]
    fn profile_share_roundtrip() {
        let v = sample();
        assert_eq!(decode(&v.dump()).unwrap(), v);
    }
    #[test]
    fn profile_share_malformed_oversized() {
        for s in ["{", "hosted-code", &" ".repeat(MAX_BYTES + 1)] {
            assert!(decode(s).is_err())
        }
    }
    #[test]
    fn profile_share_foreign_hosts_and_schema() {
        for (k, s) in [
            ("submissionId", "https://evil.example/123"),
            ("provider", "evil.example"),
        ] {
            let mut v = sample();
            v["mods"][0]["submission"][k] = s.into();
            assert!(decode(&v.dump()).is_err())
        }
        let mut v = sample();
        v["mods"][0]["url"] = "https://gamebanana.com".into();
        assert!(decode(&v.dump()).is_err());
        v = sample();
        v["mods"][0]["options"][0] = "/home/private".into();
        assert!(decode(&v.dump()).is_err());
    }
    #[test]
    fn profile_share_review_requires_download() {
        let doc = object! {storeVersion:4,profiles:object!{}};
        let r = review(&doc, &sample().dump()).unwrap();
        assert_eq!(r["rows"][0]["status"], "downloadRequired");
        assert_eq!(r["canCreate"], false);
    }
    #[test]
    fn profile_share_compact_roundtrip_limits_and_canonical() {
        let v = sample();
        let code = compact(&v).unwrap();
        assert!(code.starts_with("DLMM1-"));
        assert_eq!(decode(&code).unwrap(), v);
        assert_eq!(compact(&decode(&code).unwrap()).unwrap(), code);
        assert!(decode(&(code.clone() + "=")).is_err());
        let raw = " ".repeat(MAX_BYTES + 1);
        let mut compressed = vec![0u8; MAX_BYTES];
        let mut len = compressed.len() as _;
        assert_eq!(
            unsafe {
                compress2(
                    compressed.as_mut_ptr(),
                    &mut len,
                    raw.as_ptr(),
                    raw.len() as _,
                    9,
                )
            },
            0
        );
        assert!(decode(&format!("DLMM1-{}", base64(&compressed[..len as usize]))).is_err());
        assert!(compact(&object!{format:"dlmm-profile",version:1,mods:json::array![],notShareable:JsonValue::from(vec!["x".repeat(256);128])}).is_err());
        let mut bytes = compressed[..len as usize].to_vec();
        bytes.push(0);
        assert!(decode(&format!("DLMM1-{}", base64(&bytes))).is_err());
    }
    #[test]
    fn profile_share_exact_resolution_fixtures() {
        let sub = sample()["mods"][0]["submission"].clone();
        let sha = "a".repeat(64);
        let response = object! {matchedVpk:object!{provider:"gamebanana",submissionType:"mod",submissionId:"123",fileId:"456",sha256:sha.clone()},match:object!{matchType:"sha256",certainty:100}};
        let files = json::array![object! {fileId:"456"}, object! {fileId:"789"}];
        assert_eq!(
            exact_file(&json::array![response.clone()], &sub, &sha, &files),
            Some("456".into())
        );
        let mut weak = response.clone();
        weak["match"]["certainty"] = 90.into();
        assert!(exact_file(&json::array![weak], &sub, &sha, &files).is_none());
        assert!(exact_file(
            &json::array![response.clone()],
            &sub,
            &"b".repeat(64),
            &files
        )
        .is_none());
        assert!(exact_file(&json::array![response.clone()], &sub, &sha, &json::array![]).is_none());
        let mut other = response.clone();
        other["matchedVpk"]["fileId"] = "789".into();
        assert!(exact_file(&json::array![response, other], &sub, &sha, &files).is_none());
    }
    #[test]
    fn profile_share_association_fallback_and_cached_offline() {
        let mut d = document();
        let r = &mut d["profiles"]["default"]["mods"][0];
        r["source"].remove("remoteReceipt");
        r["source"]["nativeRemoteLink"] = object! {version:2,association:"userSelected",submission:sample()["mods"][0]["submission"].clone(),receipt:JsonValue::Null};
        let code = encode(&d, "default", false).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(decode(&code).unwrap()["mods"][0]["fileId"], "latest");
        assert_eq!(review(&d, &code).unwrap()["rows"][0]["latest"], true);
        let identity = cache_identity(&d["profiles"]["default"]["mods"][0], 1).unwrap();
        let root = std::env::temp_dir().join(format!("dlmm-share-{}", paths::random_id().unwrap()));
        paths::private_dir(&root.join("cache/catalog")).unwrap();
        {
            let mut cache = crate::catalog::store::CatalogStore::open(&root).unwrap();
            cache.set_state(&identity, "456").unwrap();
        }
        let prepared = prepare(&root, d.clone(), None).unwrap();
        let shared = encode(&prepared, "default", false).unwrap();
        assert_eq!(
            decode(shared["code"].as_str().unwrap()).unwrap()["mods"][0]["fileId"],
            "456"
        );
        assert_eq!(
            review(&prepared, shared["code"].as_str().unwrap()).unwrap()["rows"][0]["status"],
            "inLibrary"
        );
        let mut downloaded = d.clone();
        downloaded["profiles"]["default"]["mods"][0]["source"].remove("nativeRemoteLink");
        downloaded["profiles"]["default"]["mods"][0]["source"]["remoteReceipt"] =
            object! {submission:sample()["mods"][0]["submission"].clone(),rootFileId:"456"};
        downloaded["profiles"]["default"]["mods"][0]["options"]["entries"][0]["optionName"] =
            "different download filename".into();
        assert_eq!(
            review(&downloaded, shared["code"].as_str().unwrap()).unwrap()["rows"][0]["status"],
            "inLibrary"
        );
        d["profiles"]["default"]["mods"][0]["options"]["selectedIds"] = json::array![];
        assert_ne!(
            cache_identity(&d["profiles"]["default"]["mods"][0], 1).unwrap(),
            identity
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
