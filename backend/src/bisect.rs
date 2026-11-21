//! Broken-mod search adapted from upstream Deadlock Mod Manager search.ts.
//! GPL-3.0: https://github.com/Stormix/deadlock-mod-manager
use crate::{
    native::NativeError,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};

fn ids(v: &JsonValue) -> Vec<String> {
    v.members()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
fn key(v: &[String]) -> String {
    let mut v = v.to_vec();
    v.sort();
    JsonValue::from(v).dump()
}
pub mod search;
use search::{create, next, record};
pub fn view(doc: &JsonValue) -> JsonValue {
    let s = &doc["bisect"];
    if !s.is_object() {
        return object! {active:false};
    }
    let a = &s["search"];
    let high = a["high"].as_i32().unwrap_or(0);
    let low = a["low"].as_i32().unwrap_or(-1).max(0);
    let count = a["found"].len() + (high - low).max(0) as usize;
    let depth = (count.max(2) as f64).log2().ceil() as usize;
    let profile=&doc["profiles"][s["profileId"].as_str().unwrap_or("")];
    let finish_notice=if !profile.is_object(){"This profile was removed. End the search to close the session."}
        else if s["result"].members().any(|id|!profile["mods"].members().any(|m|m["source"]["id"]==*id)){"A found mod was removed. Ending the search will skip disabling it."}else{""};
    object! {finishNotice:finish_notice,active:true,profileId:s["profileId"].clone(),round:s["round"].clone(),suspectsCount:count,
    testedMods:s["testedMods"].clone(),result:s["result"].clone(),done:s["done"].clone(),
    expectedRemainingLaunches:if s["done"]==true {0}else{depth+2},maximumRemainingLaunches:if s["done"]==true {0}else{2*depth+3},
    profileChanged:store::canonical(&doc["profiles"][s["profileId"].as_str().unwrap_or("")])!=store::canonical(&s["expectedProfile"])}
}
/// Shared enabled-state mutation used by the existing bulk route and the search.
pub(crate) fn set_enabled(
    doc: &mut JsonValue,
    profile: &str,
    id: &str,
    enabled: bool,
) -> Result<bool, NativeError> {
    let layout = crate::package::layout_version(doc)?;
    let target = doc["profiles"][profile]["mods"]
        .members_mut()
        .find(|m| m["source"]["id"] == id)
        .ok_or_else(|| NativeError::new("unknownMod", "Mod no longer belongs to this profile."))?;
    if enabled {
        let invalid = if target.has_key("options") {
            crate::options::for_record(target, layout)?
                .validate_selection_save()
                .is_err()
        } else {
            target["source"]["kind"] == "local"
                && target["source"]["localImport"]["selectedCandidateIds"].is_array()
                && target["source"]["localImport"]["selectedCandidateIds"].is_empty()
        };
        if invalid {
            return Err(NativeError::new(
                "invalidLocalSelection",
                "Choose valid options before enabling this mod.",
            ));
        }
    }
    let changed = target["enabled"] != enabled;
    target["enabled"] = enabled.into();
    Ok(changed)
}
pub fn change(
    store: &Store,
    lock: &RootLock,
    method: &str,
    p: &JsonValue,
    expected: u64,
    id: &str,
    input: &JsonValue,
) -> Result<JsonValue, NativeError> {
    if let Some(result) = store.replay(id, input)? {
        return Ok(result);
    }
    crate::apply::ensure_idle(store)?;
    crate::game_access::ensure_game_idle()?;
    let mut doc = store.document()?;
    if store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)? != expected {
        return Err(NativeError::new(
            "staleRevision",
            "Refresh before continuing the search.",
        ));
    }
    let profile;
    if method == "native.bisect.start" {
        if doc["bisect"].is_object() {
            return Err(NativeError::new(
                "bisectActive",
                "End the current search first.",
            ));
        }
        profile = p["profileId"]
            .as_str()
            .ok_or_else(NativeError::params)?
            .to_owned();
        if !doc["profiles"][&profile].is_object() {
            return Err(NativeError::params());
        }
        let candidates = ids(&p["modIds"]);
        let original = doc["profiles"][&profile].clone();
        for id in &candidates {
            set_enabled(&mut doc, &profile, id, true)?;
        }
        // Validate selections without changing the original setup snapshot.
        doc["profiles"][&profile] = original.clone();
        doc["bisect"] = object! {profileId:profile.clone(),originalProfile:original,candidates:candidates.clone(),search:create(candidates),round:0,done:false};
    } else {
        profile = doc["bisect"]["profileId"]
            .as_str()
            .ok_or_else(|| NativeError::new("bisectInactive", "There is no active search."))?
            .to_owned();
        if method == "native.bisect.finish" {
            if p["keepDisabled"] == true && doc["bisect"]["done"] != true {
                return Err(NativeError::params());
            }
            if doc["profiles"][&profile].is_object() && doc["bisect"]["done"] != true {
                doc["profiles"][&profile] = doc["bisect"]["originalProfile"].clone();
            }
            if p["keepDisabled"] == true {
                for id in ids(&doc["bisect"]["result"]) {
                    if doc["profiles"][&profile]["mods"].members().any(|m|m["source"]["id"]==id) {
                        set_enabled(&mut doc, &profile, &id, false)?;
                    } else { eprintln!("Search ended: the found mod was removed; there is nothing to disable."); }
                }
            }
            doc.remove("bisect");
            let present=doc["profiles"][&profile].is_object();
            return store.commit(lock, expected, id, input, doc, if present {Some(&profile)}else{None});
        }
        if view(&doc)["profileChanged"] == true {
            return Err(NativeError::new("bisectProfileChanged","The profile changed during the search. End the search to restore the original setup."));
        }
        if p["answer"] == "undo" {
            let previous = doc["bisect"]["previousRound"].clone();
            if !previous.is_object() { return Err(NativeError::params()); }
            doc["profiles"][&profile] = previous["expectedProfile"].clone();
            doc["bisect"] = previous;
            return store.commit(lock, expected, id, input, doc, Some(&profile));
        }
        if doc["bisect"]["done"] == true {
            return Err(NativeError::params());
        }
        let mut previous = doc["bisect"].clone();
        previous.remove("previousRound");
        doc["bisect"]["previousRound"] = previous;
        let config = ids(&doc["bisect"]["testedMods"]);
        record(
            &mut doc["bisect"]["search"],
            &config,
            p["answer"].as_str().ok_or_else(NativeError::params)?,
        );
    }
    let (search, config, done) = next(doc["bisect"]["search"].clone());
    doc["bisect"]["search"] = search;
    doc["bisect"]["done"] = done.into();
    doc["bisect"]["result"] = if done {
        config.clone().into()
    } else {
        json::array![]
    };
    doc["bisect"]["testedMods"] = config.clone().into();
    doc["bisect"]["round"] =
        (doc["bisect"]["round"].as_usize().unwrap_or(0) + usize::from(!done)).into();
    if done {
        doc["profiles"][&profile] = doc["bisect"]["originalProfile"].clone();
    } else {
        for candidate in ids(&doc["bisect"]["candidates"]) {
            set_enabled(&mut doc, &profile, &candidate, config.contains(&candidate))?;
        }
    }
    doc["bisect"]["expectedProfile"] = doc["profiles"][&profile].clone();
    store.commit(lock, expected, id, input, doc, Some(&profile))
}
