//! Reviewed, metadata-only profile edits. No preparation, payload reads, game
//! resources or filesystem journals belong to this path.
use crate::{native::NativeError, package, paths, store::{self, RootLock, Store}};
use json::{object, JsonValue};
use std::{collections::{BTreeMap, HashMap, HashSet}, path::{Path, PathBuf}};

#[derive(Clone, Debug)]
pub enum ProfileEditChange {
    Selection {mod_id:String, option_ids:Vec<String>},
    Order {mod_ids:Vec<String>},
}
fn keys(v:&JsonValue, names:&[&str]) -> Result<(),NativeError> {
    if !v.is_object() || v.len()!=names.len() || v.entries().any(|(k,_)|!names.contains(&k)) {return Err(NativeError::params());} Ok(())
}
impl ProfileEditChange {
    pub fn decode(v:&JsonValue) -> Result<Self,NativeError> {
        match v["kind"].as_str() {
            Some("selection") => {
                keys(v,&["kind","modId","optionIds"])?;
                let mod_id=v["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(NativeError::params)?.to_owned();
                if !v["optionIds"].is_array() || !(1..=99).contains(&v["optionIds"].len()) {return Err(NativeError::params());}
                let option_ids=v["optionIds"].members().map(|s|s.as_str().filter(|s|package::hex(s,32)).map(str::to_owned).ok_or_else(NativeError::params)).collect::<Result<Vec<_>,_>>()?;
                if option_ids.windows(2).any(|w|w[0]>=w[1]) {return Err(NativeError::params());}
                Ok(Self::Selection{mod_id,option_ids})
            }
            Some("order") => {
                keys(v,&["kind","modIds"])?;
                if !v["modIds"].is_array() {return Err(NativeError::params());}
                let mod_ids=v["modIds"].members().map(|s|s.as_str().filter(|s|paths::safe_id(s)).map(str::to_owned).ok_or_else(NativeError::params)).collect::<Result<Vec<_>,_>>()?;
                let mut seen=HashSet::new(); if mod_ids.iter().any(|s|!seen.insert(s)) {return Err(NativeError::params());}
                Ok(Self::Order{mod_ids})
            }
            _=>Err(NativeError::params()),
        }
    }
    pub fn encode(&self) -> JsonValue {
        match self {
            Self::Selection{mod_id,option_ids}=>object!{kind:"selection",modId:mod_id.clone(),optionIds:JsonValue::from(option_ids.clone())},
            Self::Order{mod_ids}=>object!{kind:"order",modIds:JsonValue::from(mod_ids.clone())},
        }
    }
}
/// Stable legacy ordering includes every record, including disabled/unavailable
/// entries. Missing/non-i64 ranks fall back to the original array position.
pub fn ordered_mod_ids(profile:&JsonValue) -> Result<Vec<String>,NativeError> {
    if !profile.is_object() || !profile["mods"].is_array() {return Err(NativeError::params());}
    let mut rows=Vec::new(); let mut seen=HashSet::new();
    for (index,record) in profile["mods"].members().enumerate() {
        let id=record["source"]["id"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(NativeError::params)?;
        if !record.is_object() || !record["source"].is_object() || !seen.insert(id) {return Err(NativeError::params());}
        rows.push((record["source"]["installOrder"].as_i64().unwrap_or(index as i64),index,id.to_owned()));
    }
    rows.sort_by_key(|(rank,index,_)|(*rank,*index));
    Ok(rows.into_iter().map(|(_,_,id)|id).collect())
}
fn catalog(record:&JsonValue, layout:u32) -> Result<crate::options::OptionCatalog,NativeError> {
    crate::local_content::option_sources::catalog_for_record(record,layout)
}
fn selection(record:&JsonValue, layout:u32) -> Result<JsonValue,NativeError> {
    let c=catalog(record,layout)?;
    let mut choices:Vec<_>=c.entries.iter().filter(|e|c.selected_ids.binary_search(&e.id).is_ok()).collect();
    choices.sort_by(|a,b|(&a.option_name,&a.id).cmp(&(&b.option_name,&b.id)));
    Ok(object!{modId:record["source"]["id"].clone(),choices:JsonValue::from(choices.into_iter().map(|e|object!{optionId:e.id.clone(),optionName:e.option_name.clone()}).collect::<Vec<_>>())})
}
fn selected_count(profile:&JsonValue, layout:u32) -> Result<(Option<usize>,Option<String>),NativeError> {
    let mut total=0usize;
    for record in profile["mods"].members().filter(|m|m["enabled"].as_bool().unwrap_or(false)) {
        match package::selected_packages_exact(record,layout) {
            Ok(packages)=>total=total.checked_add(packages.len()).ok_or_else(NativeError::params)?,
            Err(e) if e.kind=="invalidLocalSelection"=>return Ok((None,Some(format!("Mod {} has no valid saved selection; choose ready options before Apply.",record["source"]["id"].as_str().unwrap_or("unknown"))))),
            Err(e)=>return Err(e),
        }
    }
    Ok((Some(total),if total>99 {Some("The enabled selection exceeds 99 directory packages before cross-mod deduplication; reduce it before Apply.".into())}else{None}))
}
/// Presentation only, from this pinned owned record. IDs remain authoritative.
pub(crate) fn mod_title(profile:&JsonValue, id:&str) -> String {
    profile["mods"].members().find(|m|m["source"]["id"]==id)
        .and_then(|m|m["source"]["name"].as_str())
        .map(|s|s.chars().filter(|c|!c.is_control()).take(256).collect::<String>())
        .filter(|s|!s.trim().is_empty()).unwrap_or_else(||id.to_owned())
}
fn capacity_issue(profile:&JsonValue,id:&str,reason:&str,group:Option<usize>) -> JsonValue {
    object!{modId:id,title:mod_title(profile,id),reason:reason,groupSize:group}
}
fn capacity(doc:&JsonValue,profile_id:&str) -> Result<JsonValue,NativeError> {
    use crate::deployment_layout::LayoutOutcome;
    let profile=&doc["profiles"][profile_id];
    let version=package::layout_version(doc)?;
    let records:HashMap<_,_>=profile["mods"].members().filter_map(|m|m["source"]["id"].as_str().map(|id|(id,m))).collect();
    // Unknown selections cannot be represented by zero or a partial count.
    for id in ordered_mod_ids(profile)? {
        let record=records.get(id.as_str()).ok_or_else(NativeError::params)?;
        if record["enabled"]!=true {continue;}
        let problem=if record["available"]!=true {Some("Choose ready files for this included mod before Apply.")}
            else {match package::selected_packages_exact(record,version) {
                Ok(_)=>None,
                Err(e) if matches!(e.kind.as_str(),"invalidLocalSelection"|"optionNeedsPreparation"|"unavailablePayload")=>Some("Choose ready files for this included mod before Apply."),
                Err(e)=>return Err(e),
            }};
        if let Some(reason)=problem {return Ok(object!{status:"SelectionRequired",counts:JsonValue::Null,shards:JsonValue::Null,partial:false,issue:capacity_issue(profile,&id,reason,None)});}
    }
    let intent=crate::deployment_intent::DeploymentIntent::capture(doc,profile_id)?;
    let (status,counts,shards,partial,issue)=match crate::deployment_layout::plan(&intent)? {
        LayoutOutcome::Fits(layout)=>("Fits",Some(layout.counts),Some(layout.shards),false,JsonValue::Null),
        LayoutOutcome::ExceedsCapacity(row)=>("ExceedsCapacity",Some(row.counts),Some(row.partial_shards),true,
            capacity_issue(profile,&row.first_failing_mod_id,"This ordered group cannot fit within ten folders. Reduce the selected files or review the install order.",Some(row.first_failing_group_size))),
        LayoutOutcome::SelectionRequired(row)=>("SelectionRequired",row.counts,None,false,capacity_issue(profile,&row.mod_id,row.reason,None)),
    };
    Ok(object!{status:status,counts:counts.map(|c|object!{selectedReferences:c.selected_references,uniqueFamilies:c.unique_families,companionFiles:c.companion_files,physicalFiles:c.physical_files,totalBytes:c.total_bytes}),
        shards:shards.map(|rows|JsonValue::from(rows.iter().map(|s|object!{index:s.index,directoryCount:s.families.len()}).collect::<Vec<_>>())),partial:partial,issue:issue})
}
fn enabled_ids(profile:&JsonValue) -> Result<Vec<String>,NativeError> {
    let enabled:HashSet<_>=profile["mods"].members().filter(|m|m["enabled"].as_bool().unwrap_or(false)).filter_map(|m|m["source"]["id"].as_str()).collect();
    Ok(ordered_mod_ids(profile)?.into_iter().filter(|id|enabled.contains(id.as_str())).collect())
}
fn retained(record:&JsonValue, layout:u32) -> Result<BTreeMap<String,String>,NativeError> {
    let mut result=BTreeMap::new();
    for (file,role) in crate::local_import::record_files(record,layout)? {
        let value=store::canonical(&object!{file:package::encode_file(&file),role:role});
        if result.insert(file.id,value.clone()).is_some_and(|old|old!=value) {return Err(NativeError::params());}
    }
    Ok(result)
}

pub struct ProfileEditPlan {
    root:PathBuf,
    profile_id:String,
    expected_revision:u64,
    before_document:JsonValue,
    after_document:JsonValue,
    change:ProfileEditChange,
    sharded_capacity:bool,
    summary:JsonValue,
}
impl ProfileEditPlan {
    pub fn root(&self)->&Path {&self.root}
    pub fn profile_id(&self)->&str {&self.profile_id}
    pub fn expected_revision(&self)->u64 {self.expected_revision}
    pub fn summary(&self)->&JsonValue {&self.summary}
    pub fn review(store:&Store, profile_id:&str, expected:u64, change:ProfileEditChange) -> Result<Self,NativeError> {
        Self::review_capacity(store,profile_id,expected,change,false)
    }
    pub fn review_capacity(store:&Store, profile_id:&str, expected:u64, change:ProfileEditChange, sharded_capacity:bool) -> Result<Self,NativeError> {
        // Re-admit even directly constructed changes through the strict codec.
        let change=ProfileEditChange::decode(&change.encode())?;
        if !paths::safe_id(profile_id) {return Err(NativeError::params());}
        crate::apply::ensure_idle(store)?;
        let before=store.document()?;
        if before["revision"]!=expected.to_string() {return Err(NativeError::new("staleRevision","Library changed; review the profile edit again."));}
        let layout=package::layout_version(&before)?;
        let profile=&before["profiles"][profile_id];
        if !profile.is_object() {return Err(NativeError::new("unknownProfile","Requested native profile does not exist."));}
        let before_ids=ordered_mod_ids(profile)?;
        let before_enabled=enabled_ids(profile)?;
        let mut after=before.clone();
        let (kind,before_selection,after_selection,affected)=match &change {
            ProfileEditChange::Selection{mod_id,option_ids}=> {
                let index=profile["mods"].members().position(|m|m["source"]["id"]==mod_id.as_str()).ok_or_else(||NativeError::new("unknownMod","Requested mod does not belong to this profile."))?;
                let original=&profile["mods"][index];
                let mut c=catalog(original,layout)?;
                for id in option_ids {
                    if !c.entries.iter().any(|e|e.id==*id && e.package.is_some()) {return Err(NativeError::new("optionNeedsPreparation","Only retained ready options can be selected; prepare and attach unavailable choices first."));}
                }
                let before_selection=selection(original,layout)?;
                c.selected_ids=option_ids.clone(); c.validate_selection_save()?;
                let record=&mut after["profiles"][profile_id]["mods"][index];
                record.remove("payloads");record.remove("packages"); record["options"]=c.encode(layout)?;
                // Migration changes the selection envelope, never ownership,
                // historical source/font data, or profile placement.
                if retained(original,layout)?!=retained(record,layout)? || original["source"]!=record["source"] {return Err(NativeError::new("invalidLocalSelection","Selection migration would change retained bindings; prepare a fresh import instead."));}
                let selected=selection(record,layout)?;
                let affected=if original["enabled"].as_bool().unwrap_or(false) {vec![mod_id.clone()]}else{Vec::new()};
                ("selection",before_selection,selected,affected)
            }
            ProfileEditChange::Order{mod_ids}=> {
                let mut records:HashMap<_,_>=profile["mods"].members().map(|m|(m["source"]["id"].as_str().unwrap().to_owned(),m.clone())).collect();
                if mod_ids.len()!=records.len() || mod_ids.iter().any(|id|!records.contains_key(id)) {return Err(NativeError::new("invalidProfileOrder","Supply every mod ID in this profile exactly once, including disabled and unavailable mods."));}
                let mut ordered=Vec::new();
                for (rank,id) in mod_ids.iter().enumerate() {
                    let mut record=records.remove(id).ok_or_else(NativeError::params)?;
                    record["source"]["installOrder"]=rank.into();ordered.push(record);
                }
                after["profiles"][profile_id]["mods"]=JsonValue::from(ordered);
                let after_enabled=enabled_ids(&after["profiles"][profile_id])?;
                let affected=if before_enabled!=after_enabled {after_enabled}else{Vec::new()};
                ("order",JsonValue::Null,JsonValue::Null,affected)
            }
        };
        let old_version=package::document_version(&before)?;
        after["storeVersion"]=4.into();after["packageVersion"]=layout.into();
        if old_version<3 {after["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
        // A first migration or normalization is real metadata work. Automatic
        // revision growth alone never makes an otherwise unchanged edit Saveable.
        let has_changes=after!=before;
        let affected=if has_changes {affected}else{Vec::new()};
        after["revision"]=expected.checked_add(1).ok_or_else(NativeError::params)?.to_string().into();
        let mut summary=object!{version:1,kind:kind,hasChanges:has_changes,canCommit:has_changes,before:object!{modIds:JsonValue::from(before_ids),selection:before_selection},after:object!{modIds:JsonValue::from(ordered_mod_ids(&after["profiles"][profile_id])?),selection:after_selection},affectedEnabledModIds:JsonValue::from(affected),precedence:object!{status:"requiresApplyReview",message:"Lower package order wins overlapping resources. Detailed conflicts and model incompatibilities require Apply review."},payloadValidation:"requiresApplyValidation",message:"Saved changes require Apply"};
        if sharded_capacity {
            summary["version"]=2.into();
            summary["capacity"]=object!{deploymentLayout:"shards-v1",perModLimit:99,perShardLimit:99,maxShards:10,before:capacity(&before,profile_id)?,after:capacity(&after,profile_id)?};
        } else {
            let (count_before,issue_before)=selected_count(profile,layout)?;
            let (count_after,issue_after)=selected_count(&after["profiles"][profile_id],layout)?;
            summary["packageConstraint"]=object!{limit:99,selectedBefore:count_before,selectedAfter:count_after,beforeIssue:issue_before,afterIssue:issue_after};
        }
        store::ensure_readable(&after)?;
        store::ensure_internal(&object!{beforeDocument:before.clone(),afterDocument:after.clone(),change:change.encode(),summary:summary.clone()})?;
        Ok(Self{root:store.root().into(),profile_id:profile_id.into(),expected_revision:expected,before_document:before,after_document:after,change,sharded_capacity,summary})
    }
    pub fn commit(&self, store:&Store, lock:&RootLock, expected:u64, operation_id:&str, input:&JsonValue) -> Result<JsonValue,NativeError> {
        if let Some(result)=store.replay(operation_id,input)? {return Ok(result);}
        crate::apply::ensure_idle(store)?;
        if store.root()!=self.root || lock.root()!=self.root || expected!=self.expected_revision || store.document()?!=self.before_document {return Err(NativeError::new("stalePreview","Profile review changed; review the edit again."));}
        if self.summary["canCommit"]!=true {return Err(NativeError::new("noProfileChanges","This reviewed edit has no metadata changes to save."));}
        // Rebuild from original data and requested change before the single
        // metadata transaction; this never opens managed payloads.
        let checked=Self::review_capacity(store,&self.profile_id,expected,self.change.clone(),self.sharded_capacity)?;
        if checked.after_document!=self.after_document || checked.summary!=self.summary {return Err(NativeError::new("stalePreview","Profile review changed; review the edit again."));}
        store.commit(lock,expected,operation_id,input,self.after_document.clone(),Some(&self.profile_id))
    }
}
