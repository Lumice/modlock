//! Pinned curated catalog and pure overlay transforms. No game-file writer.
mod live;
pub mod patch;
#[cfg(test)]
mod tests;
mod types;
use crate::{native::NativeError, paths};
use json::{object, JsonValue};
use std::collections::{HashMap, HashSet};
fn error(s: impl Into<String>) -> NativeError {
    NativeError::new("performanceConfig", s)
}
pub fn catalog() -> Result<JsonValue, NativeError> {
    let c = json::parse(include_str!("../../data/perf-catalog.json"))
        .map_err(|_| error("Invalid bundled catalog"))?;
    if c["schemaVersion"] != 1
        || !c["presets"].is_array()
        || !c["convars"].is_array()
        || !c["stock"].is_array()
    {
        return Err(error("Invalid catalog schema"));
    }
    let mut ids = HashSet::new();
    for p in c["presets"].members() {
        if !p["id"]
            .as_str()
            .is_some_and(|s| paths::safe_id(s) && ids.insert(s))
            || !p["name"].is_string()
            || !p["entries"].is_array()
        {
            return Err(error("Invalid preset"));
        }
        let mut keys = HashSet::new();
        for e in p["entries"].members() {
            let path = entry_path(e)?;
            if path.len() < 2
                || !keys.insert(live::path_key(&path))
                || !(e["value"].is_null()
                    || e["value"].as_str().is_some_and(patch::is_writable_value))
            {
                return Err(error("Invalid catalog entry"));
            }
        }
    }
    Ok(c)
}
fn entry_path(e: &JsonValue) -> Result<Vec<String>, NativeError> {
    let p = e["path"]
        .members()
        .map(|v| {
            v.as_str()
                .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
                .map(String::from)
                .ok_or_else(|| error("Invalid entry path"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if p.is_empty() || !e["path"].is_array() {
        return Err(error("Invalid entry path"));
    }
    Ok(p)
}
pub fn writable(c: &JsonValue, path: &[String]) -> bool {
    if path.len() < 2 || !path.iter().all(|s| patch::is_writable_key(s)) {
        return false;
    }
    let first = &path[0];
    if !first.eq_ignore_ascii_case("ConVars") {
        return false;
    }
    if c["rules"]["guardedSections"]
        .members()
        .any(|s| s.as_str().is_some_and(|s| s.eq_ignore_ascii_case(first)))
    {
        return false;
    }
    if c["rules"]["excludedSections"].members().any(|s| {
        s.as_str().is_some_and(|s| {
            let p: Vec<_> = s.split('/').collect();
            p.len() <= path.len() && p.iter().zip(path).all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
    }) {
        return false;
    }
    for r in c["rules"]["denied"].members() {
        if r["path"].is_array()
            && r["path"].len() == path.len()
            && r["path"]
                .members()
                .zip(path)
                .all(|(a, b)| a.as_str().is_some_and(|a| a.eq_ignore_ascii_case(b)))
        {
            return false;
        }
        if first.eq_ignore_ascii_case("ConVars") {
            if let Some(pattern) = r["pattern"].as_str() {
                if regex::RegexBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map_or(true, |r| r.is_match(&path[1]))
                {
                    return false;
                }
            }
        }
    }
    true
}
pub fn validate_selection(doc: &JsonValue) -> Result<(), NativeError> {
    if !doc.has_key("performancePreset") {
        return Ok(());
    }
    let id = doc["performancePreset"]
        .as_str()
        .ok_or_else(|| error("Invalid saved performance selection"))?;
    if !id.is_empty() && id != "none" && !catalog()?["presets"].members().any(|p| p["id"] == id) {
        return Err(error("Unknown saved performance preset"));
    }
    Ok(())
}
pub fn selection(doc: &JsonValue) -> &str {
    match doc["performancePreset"].as_str().unwrap_or("") { "none" => "", id => id }
}
const CATALOG_REV: &str = "7c7c90cc25";
pub fn overlay(text: &str, id: &str) -> Result<String, NativeError> {
    if id.is_empty() { return Ok(text.to_owned()); }
    if !patch::detect_foreign(text).is_empty() {
        return Err(error("Foreign performance overlay is preserved. Remove it with its owning tool before Apply."));
    }
    let (base, applied) = patch::strip_overlay(text);
    patch::verify_stripped(text, &base)?;
    if patch::own_leftover(&base).is_some() {
        return Err(error("Performance settings contain mixed markers. They have been left unchanged."));
    }
    if id == "none" {
        let Some(applied) = applied else { return Ok(text.to_owned()); };
        if !applied.rev.is_empty() && applied.rev != CATALOG_REV {
            return Err(NativeError::new(
                "performanceOutdated",
                "Your game has a different version of these performance settings. To remove them, close Deadlock, choose this preset again and apply it, then choose None and apply again.",
            ));
        }
        let edited = || NativeError::new("performanceChanged", "Performance settings were edited outside the app. They have been left unchanged.");
        let recreated = apply_preset(&base, &applied.config_id).map_err(|_| edited())?;
        if recreated != text { return Err(edited()); }
        return Ok(base);
    }
    apply_preset(&base, id)
}
fn apply_preset(base: &str, id: &str) -> Result<String, NativeError> {
    let c = catalog()?;
    let p = c["presets"]
        .members()
        .find(|p| p["id"] == id)
        .ok_or_else(|| error("Unknown performance preset"))?;
    let entries = p["entries"]
        .members()
        .map(|e| {
            Ok(types::ResolvedEntry {
                path: entry_path(e)?,
                value: e["value"].as_str().map(String::from),
            })
        })
        .collect::<Result<Vec<_>, NativeError>>()?;
    let plan = patch::OverlayPlan {
        config_id: id,
        name: p["name"].as_str().unwrap(),
        rev: CATALOG_REV,
        entries: entries.iter().filter(|e| writable(&c, &e.path)).collect(),
        credits: vec![p["author"].as_str().unwrap_or("").into()],
    };
    patch::apply_overlay(base, &plan)
}
pub fn detect(text: &str) -> Result<JsonValue, NativeError> {
    let foreign = patch::detect_foreign(text);
    let old = patch::read_overlay(text);
    let (base, _) = patch::strip_overlay(text);
    let values = patch::scalar_values(&patch::parse_sites(&base)?);
    let c = catalog()?;
    // Like upstream analyze: use the file's PGIVersion, otherwise closest stock.
    let stock = c["stock"].members().min_by_key(|b| {
        let pgi = values.get("pgiversion");
        let same = pgi.is_some_and(|p| {
            b["pgiVersion"].as_str() == Some(p.as_str()) || b["pgiVersion"].to_string() == *p
        });
        let distance = b["entries"]
            .members()
            .filter(|e| {
                let p = entry_path(e).unwrap_or_default();
                values.get(&live::path_key(&p)).map(String::as_str) != e["value"].as_str()
            })
            .count();
        (!same, distance)
    });
    let mut changes = JsonValue::new_array();
    let observable = |path: &[String]| {
        if path.is_empty() {
            return false;
        }
        let joined = path.join("/");
        !c["rules"]["excludedSections"].members().any(|s| {
            s.as_str().is_some_and(|s| {
                joined.eq_ignore_ascii_case(s)
                    || joined
                        .to_ascii_lowercase()
                        .starts_with(&(s.to_ascii_lowercase() + "/"))
            })
        }) && (path[0].eq_ignore_ascii_case("ConVars")
            || c["sectionCategories"].has_key(&path[0].to_ascii_lowercase())
            || path[0].eq_ignore_ascii_case("DisallowGameInfoConditionals"))
    };
    if let Some(stock) = stock {
        let expected: HashMap<_, _> = stock["entries"]
            .members()
            .filter_map(|e| {
                Some((
                    live::path_key(&entry_path(e).ok()?),
                    e["value"].as_str()?.to_string(),
                ))
            })
            .collect();
        for (key, value) in &values {
            let path: Vec<_> = key.split('/').map(String::from).collect();
            if observable(&path)
                && expected
                    .get(key)
                    .is_none_or(|v| !live::values_equal(v, value))
            {
                changes
                    .push(object! {key:key.as_str(),value:value.as_str()})
                    .unwrap();
            }
        }
        for (key, value) in &expected {
            let path: Vec<_> = key.split('/').map(String::from).collect();
            if observable(&path) && !values.contains_key(key) {
                changes
                    .push(object! {key:key.as_str(),value:JsonValue::Null,from:value.as_str()})
                    .unwrap();
            }
        }
    }
    let actual = patch::scalar_values(&patch::parse_sites(text)?);
    let matched = c["presets"].members().find(|p| {
        p["entries"]
            .members()
            .filter(|e| entry_path(e).is_ok_and(|p| writable(&c, &p)))
            .all(|e| {
                let path = entry_path(e).unwrap();
                match e["value"].as_str() {
                    Some(v) => actual
                        .get(&live::path_key(&path))
                        .is_some_and(|a| live::values_equal(a, v)),
                    None => !actual.contains_key(&live::path_key(&path)),
                }
            })
    });
    let state = if !foreign.is_empty() {
        "foreign"
    } else if matched.is_some() {
        "preset"
    } else if changes.is_empty() {
        "stock"
    } else {
        "custom"
    };
    Ok(
        object! {state:state,presetId:matched.map(|p|p["id"].clone()),markerId:old.as_ref().map(|o|o.config_id.as_str()),revision:old.as_ref().map(|o|o.rev.as_str()),handEdited:old.as_ref().map(|o|o.hand_edited.clone()),foreign:foreign.iter().map(|f|object!{tool:format!("{:?}",f.tool),label:f.label.clone(),lineCount:f.line_count}).collect::<Vec<_>>(),customEntries:changes,baseBuild:stock.map(|s|s["build"].clone())},
    )
}
pub fn view(doc: &JsonValue) -> Result<JsonValue, NativeError> {
    let c = catalog()?;
    let text = doc["gamePath"]
        .as_str()
        .filter(|s| !s.is_empty())
        .and_then(|game| {
            paths::read_bounded(
                &std::path::Path::new(game).join("game/citadel/gameinfo.gi"),
                4 * 1024 * 1024,
            )
            .ok()
        })
        .and_then(|b| String::from_utf8(b).ok());
    let sites = text.as_deref().and_then(|s| patch::parse_sites(s).ok());
    let values = sites
        .as_deref()
        .map(patch::scalar_values)
        .unwrap_or_default();
    let detected = text
        .as_deref()
        .map(detect)
        .transpose()
        .unwrap_or_else(|e| Some(object! {state:"unreadable",message:e.message}))
        .unwrap_or(object! {state:"unavailable"});
    let mut presets = JsonValue::new_array();
    for p in c["presets"].members() {
        let mut entries = JsonValue::new_array();
        let mut groups = HashMap::<String, usize>::new();
        for e in p["entries"].members() {
            let path = entry_path(e)?;
            let key = path.join("/");
            let meta = c["convars"].members().find(|m| {
                path[0].eq_ignore_ascii_case("ConVars")
                    && m["name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&path[1]))
            });
            let cat = meta.and_then(|m| m["category"].as_str()).unwrap_or("other");
            *groups.entry(cat.into()).or_default() += 1;
            entries.push(object!{key:key,label:meta.and_then(|m|m["label"].as_str()).unwrap_or(path.last().unwrap()),from:values.get(&live::path_key(&path)).cloned(),to:e["value"].clone(),description:meta.and_then(|m|m["description"].as_str()).unwrap_or(""),category:cat,allowed:writable(&c,&path),omittedReason:if writable(&c,&path){JsonValue::Null}else{"Only safe ConVars entries are applied; engine sections and denied keys are omitted.".into()}}).unwrap();
        }
        let mut categories: Vec<_> = groups.into_iter().collect();
        categories.sort();
        presets.push(object!{id:p["id"].clone(),name:p["name"].clone(),author:p["author"].clone(),source:p["source"].clone(),description:p["blurb"].clone(),entryCount:entries.len(),entries:entries,categoryGroups:categories.into_iter().map(|(id,n)|object!{label:c["categories"].members().find(|c|c["id"]==id.as_str()).map(|c|c["label"].clone()),id:id,count:n}).collect::<Vec<_>>()}).unwrap();
    }
    Ok(object! {presets:presets,selection:selection(doc),explicitSelection:!doc["performancePreset"].as_str().unwrap_or("").is_empty(),detected:detected,catalogCommit:CATALOG_REV})
}
pub fn review(before: &str, id: &str) -> Result<JsonValue, NativeError> {
    let state = detect(before)?;
    let c = catalog()?;
    let p = c["presets"].members().find(|p| p["id"] == id);
    let mut allowed = 0;
    let mut omitted = 0;
    if let Some(p) = p {
        for e in p["entries"].members() {
            if writable(&c, &entry_path(e)?) {
                allowed += 1
            } else {
                omitted += 1
            }
        }
    }
    let after = overlay(before, id)?;
    let old_sites = patch::parse_sites(before)?;
    let new_sites = patch::parse_sites(&after)?;
    let old_values = patch::scalar_values(&old_sites);
    let new_values = patch::scalar_values(&new_sites);
    let mut changes = Vec::new();
    let mut seen = HashSet::new();
    for site in old_sites.iter().chain(&new_sites) {
        let key = live::path_key(&site.path);
        if seen.insert(key.clone()) && old_values.get(&key) != new_values.get(&key) {
            changes.push(object!{name:site.path.last().cloned(),from:old_values.get(&key).cloned(),to:new_values.get(&key).cloned()});
        }
    }
    Ok(
        object! {changes:changes,presetId:if id=="none"{""}else{id},presetName:p.map(|p|p["name"].clone()),entryCount:allowed,omittedCount:omitted,detected:state,overlayAction:if id=="none"{"remove"}else if id.is_empty(){"preserve"}else{"replace"},foreignPolicy:"preserve-and-refuse",restorePolicy:"exact-prior-bytes"},
    )
}
