//! Desired library removal; captured game checkpoints remain immutable.
use crate::{native::NativeError, options, package::{self, FileRef}, paths, store::Store};
use json::JsonValue;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Removal {
    pub document: JsonValue,
    pub files: BTreeMap<String, FileRef>,
    pub count: usize,
}
pub(crate) fn plan(before: &JsonValue, params: &JsonValue) -> Result<Removal, NativeError> {
    let invalid = NativeError::params;
    let profile=params["profileId"].as_str().filter(|v|paths::safe_id(v)).ok_or_else(invalid)?;
    let ids=&params["modIds"];
    if !ids.is_array() || ids.is_empty() || ids.len()>256 || !before["profiles"][profile].is_object() {return Err(invalid());}
    let mut selected=BTreeSet::new();
    for id in ids.members() {
        let id=id.as_str().filter(|v|paths::safe_id(v)).ok_or_else(invalid)?;
        if !selected.insert(id.to_owned()) {return Err(invalid());}
        if !before["profiles"][profile]["mods"].members().any(|m|m["source"]["id"]==id) {
            return Err(NativeError::new("unknownMod","The selected mod changed. Refresh the library and select it again."));
        }
    }
    let layout=package::layout_version(before)?;
    let mut files:BTreeMap<String,FileRef>=BTreeMap::new();
    let mut document=before.clone();
    for (_, profile) in document["profiles"].entries_mut() {
        let mut kept=Vec::new();
        for record in profile["mods"].members() {
            if !record["source"]["id"].as_str().is_some_and(|id|selected.contains(id)) {kept.push(record.clone());continue;}
            // A broken package record can leave the Library without granting file deletion.
            if let Ok(packages)=options::retained_packages(record,layout) {
                for package in packages {
                    for file in std::iter::once(package.directory).chain(package.companions.into_values()) {
                        if let Some(old)=files.get(&file.id) {
                            if old.sha256!=file.sha256 || old.size!=file.size {return Err(invalid());}
                        }
                        files.insert(file.id.clone(),file);
                    }
                }
            }
        }
        profile["mods"]=kept.into();
    }
    Ok(Removal{document,files,count:selected.len()})
}
pub(crate) fn referenced(store:&Store,document:&JsonValue,id:&str)->Result<bool,NativeError> {
    let mut live=document.clone();live.remove("importLedger");
    Ok(live.dump().contains(id) || store.journals()?.members()
        .filter(|j|j["body"]["kind"]!="maintenance").any(|j|j.dump().contains(id)))
}
