//! Pure desired-package intent. Exact source bindings and effective content
//! identity are deliberately separate. This module never opens payloads or
//! invents an applied checkpoint for a legacy library.
use crate::{native::NativeError, package::{self, PackageRef}, paths, profile_edit, store};
use json::{object, JsonValue};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntentMod {
    pub mod_id: String,
    pub fix_enabled: bool,
    pub packages: Vec<PackageRef>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentIntent {
    pub package_version: u32,
    pub profile_id: String,
    pub mods: Vec<IntentMod>,
}
fn invalid() -> NativeError {
    NativeError::new("invalidDeploymentIntent", "Deployment intent or its semantic signature is invalid.")
}
fn keys(v: &JsonValue, names: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len()!=names.len() || v.entries().any(|(k,_)| !names.contains(&k)) {return Err(invalid());}
    Ok(())
}
impl DeploymentIntent {
    /// Explicit empty deployment for the active profile; desired payload readiness
    /// is deliberately outside this action's prerequisites.
    pub fn empty_active(doc: &JsonValue) -> Result<Self, NativeError> {
        package::document_version(doc)?;
        let profile_id = doc["activeProfileId"].as_str().filter(|s|paths::safe_id(s))
            .filter(|s|doc["profiles"][*s].is_object()).ok_or_else(invalid)?;
        Self::decode(&Self { package_version:package::layout_version(doc)?,
            profile_id:profile_id.into(), mods:Vec::new() }.encode())
    }
    pub fn capture(doc: &JsonValue, profile_id: &str) -> Result<Self, NativeError> {
        package::document_version(doc)?;
        let layout = package::layout_version(doc)?;
        let profile = &doc["profiles"][profile_id];
        if !paths::safe_id(profile_id) || !profile.is_object() {return Err(NativeError::new("unknownProfile", "Requested native profile does not exist."));}
        let records:HashMap<_,_>=profile["mods"].members().filter_map(|r|r["source"]["id"].as_str().map(|id|(id,r))).collect();
        let mut mods = Vec::new();
        for id in profile_edit::ordered_mod_ids(profile)? {
            let record = records.get(id.as_str()).ok_or_else(invalid)?;
            if record["enabled"].as_bool().unwrap_or(false) {
                mods.push(IntentMod {mod_id:id,fix_enabled:record["fixEnabled"].as_bool().unwrap_or(doc["fixAutomatically"].as_bool().unwrap_or(true)),packages:package::selected_packages_exact(record,layout)?});
            }
        }
        let result = Self {package_version:layout,profile_id:profile_id.into(),mods};
        // Use the strict same codec for captured and persisted proof.
        Self::decode(&result.encode())
    }
    pub fn encode(&self) -> JsonValue {
        object! {version:1,packageVersion:self.package_version,profileId:self.profile_id.clone(),mods:JsonValue::from(self.mods.iter().map(|m|{let mut row=object!{modId:m.mod_id.clone(),packages:package::encode_packages(&m.packages)};if !m.fix_enabled{row["fixEnabled"]=false.into();}row}).collect::<Vec<_>>())}
    }
    pub fn decode(v: &JsonValue) -> Result<Self, NativeError> {
        keys(v,&["version","packageVersion","profileId","mods"])?;
        if v["version"]!=1 {return Err(NativeError::new("unsupportedVersion","Unsupported deployment intent version."));}
        let package_version=v["packageVersion"].as_u32().filter(|n|matches!(n,1|2)).ok_or_else(invalid)?;
        let profile_id=v["profileId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(invalid)?.to_owned();
        if !v["mods"].is_array() {return Err(invalid());}
        store::ensure_internal(v)?;
        let mut mods=Vec::new(); let mut ids=HashSet::new(); let mut refs=HashMap::new();
        for row in v["mods"].members() {
            keys(row,if row.has_key("fixEnabled"){&["modId","packages","fixEnabled"]}else{&["modId","packages"]})?;
            if row.has_key("fixEnabled") && row["fixEnabled"].as_bool().is_none(){return Err(invalid());}
            let mod_id=row["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(invalid)?.to_owned();
            if !ids.insert(mod_id.clone()) || !row["packages"].is_array() || row["packages"].is_empty() || row["packages"].len()>1024 {return Err(invalid());}
            for p in row["packages"].members() {
                keys(p,&["directory","companions"])?;
                keys(&p["directory"],&["id","sha256","size","sourceName"])?;
                if !p["companions"].is_array() {return Err(invalid());}
                for c in p["companions"].members() {
                    keys(c,&["archiveIndex","file"])?;
                    keys(&c["file"],&["id","sha256","size","sourceName"])?;
                }
            }
            let packages=package::legacy_packages(&object!{packages:row["packages"].clone()},2)?;
            if package_version==1 && packages.iter().any(|p|!p.companions.is_empty()) {return Err(invalid());}
            for p in &packages {package::validate_refs(p,&mut refs)?;}
            mods.push(IntentMod{mod_id,packages,fix_enabled:row["fixEnabled"].as_bool().unwrap_or(true)});
        }
        Ok(Self{package_version,profile_id,mods})
    }
    /// Ordered enabled IDs and ordered coalesced complete family content keys.
    /// Revisions, labels, concrete native IDs/names, inactive options and fonts
    /// cannot create an effective package change.
    pub fn semantic_projection(&self) -> Result<JsonValue,NativeError> {
        Self::decode(&self.encode())?;
        let mut mods=Vec::new();
        for m in &self.mods {
            let mut seen=HashSet::new(); let mut family_keys=Vec::new();
            for p in &m.packages {let key=package::family_key(p)?; if seen.insert(key.clone()) {family_keys.push(key);}}
            let mut row=object!{modId:m.mod_id.clone(),familyKeys:JsonValue::from(family_keys)};if !m.fix_enabled{row["fixEnabled"]=false.into();}mods.push(row);
        }
        Ok(object!{version:1,packageVersion:self.package_version,profileId:self.profile_id.clone(),mods:JsonValue::from(mods)})
    }
    pub fn signature(&self) -> Result<String,NativeError> {
        paths::digest_bytes(store::canonical(&self.semantic_projection()?).as_bytes())
    }
    pub fn validate_signature(&self, signature: &str) -> Result<(),NativeError> {
        if !package::hex(signature,64) || self.signature()?!=signature {return Err(invalid());} Ok(())
    }
}

/// Missing evidence is legacy uncertainty, never reconstructed from desired state.
pub fn applied_intent(applied: &JsonValue) -> Result<Option<DeploymentIntent>, NativeError> {
    let sharded = applied["manifest"]["version"] == 3;
    let has_intent = applied.has_key("deploymentIntent");
    let has_signature = applied.has_key("desiredSignature");
    if !has_intent && !has_signature {
        if applied.is_null() || matches!(applied["manifest"]["version"].as_u32(),Some(1|2)) {return Ok(None);}
        return Err(invalid());
    }
    if !has_intent || !has_signature { return Err(invalid()); }
    let intent = DeploymentIntent::decode(&applied["deploymentIntent"])?;
    intent.validate_signature(applied["desiredSignature"].as_str().ok_or_else(invalid)?)?;
    if (!sharded && (intent.mods.iter().try_fold(0usize,|n,m|n.checked_add(m.packages.len())).filter(|n|*n<=99).is_none()
        || applied["manifest"]["version"].as_u32() != Some(intent.package_version)))
        || applied["profileId"] != intent.profile_id
        || applied["manifest"]["generation"] != applied["generation"]
        || !applied["enabled"].is_object() { return Err(invalid()); }
    let enabled: HashSet<&str> = intent.mods.iter().map(|m| m.mod_id.as_str()).collect();
    for (id, value) in applied["enabled"].entries() {
        let on = value.as_bool().ok_or_else(invalid)?;
        if on != enabled.contains(id) { return Err(invalid()); }
    }
    if enabled.iter().any(|id| applied["enabled"][*id] != true) { return Err(invalid()); }
    if sharded {
        if applied["manifest"].has_key("repairs") {
            let choices=&applied["manifest"]["repairs"]["choices"]["mods"];
            if !choices.is_array() || choices.len()!=intent.mods.len() || choices.members().zip(&intent.mods).any(|(c,m)|c["modId"]!=m.mod_id||c["enabled"].as_bool()!=Some(m.fix_enabled)){return Err(invalid());}
        }
        let view = crate::generation::decode(&applied["manifest"])?;
        let crate::deployment_layout::LayoutOutcome::Fits(layout) = crate::deployment_layout::plan(&intent)? else { return Err(invalid()); };
        if view.package_version != intent.package_version || view.families.len()!=layout.families.len()
            || view.mounts.len()!=layout.shards.len()+usize::from(!applied["manifest"]["repairs"]["file"].is_null()) { return Err(invalid()); }
        for (actual, expected) in view.families.iter().zip(&layout.families) {
            if actual.package!=expected.package || actual.mod_id!=expected.mod_id || actual.mod_ids!=expected.mod_ids
                || actual.shard_index!=Some(expected.shard_index) || actual.slot!=expected.slot { return Err(invalid()); }
        }
        // Reuse the conservative parser, discarding its pure replacement text.
        // Its recognized sequence must be the exact derived native vector: no
        // aliases, foreign mounts, duplicates, missing entries or reordering.
        let gameinfo = applied["gameinfo"].as_str().filter(|s|s.len()<=4*1024*1024).ok_or_else(invalid)?;
        if applied["gameinfoSha256"] != paths::digest_bytes(gameinfo.as_bytes())? { return Err(invalid()); }
        let known = view.mounts.iter().cloned().collect();
        // Intent describes this checkpoint's mounts, not ownership of other native
        // generations retained as unmanaged after an explicit folder change.
        // Journal transforms and the complete namespace enforce those exclusions.
        let unmanaged=crate::gameinfo::mounts(gameinfo)?.into_iter().filter(|m|m.starts_with("citadel/deadlock_native/") && !view.mounts.contains(m)).collect::<Vec<_>>();
        let present=crate::gameinfo::mounts(gameinfo)?.into_iter().filter(|m|view.mounts.contains(m)).collect::<Vec<_>>();
        let (_, mut actual_mounts) = crate::gameinfo::replace_many_unmanaged(gameinfo,&known,&present,&view.mounts,&unmanaged)?;
        // A retained Restore target may combine native mounts and the original
        // manager's exact addons line. New Apply output is checked separately
        // against its journal transform and preparation namespace.
        actual_mounts.retain(|m|m != "citadel/addons");
        let expected_mounts=if applied.has_key("mountedSubset") {
            let subset=&applied["mountedSubset"];
            if !subset.is_array() || subset.is_empty() || subset.len()>=view.mounts.len() {return Err(invalid());}
            let chosen=subset.members().map(|v|v.as_str().map(str::to_owned).ok_or_else(invalid)).collect::<Result<Vec<_>,_>>()?;
            let canonical=view.mounts.iter().filter(|m|chosen.contains(m)).cloned().collect::<Vec<_>>();
            if chosen!=canonical {return Err(invalid());} chosen
        }else{view.mounts.clone()};
        if actual_mounts!=expected_mounts { return Err(invalid()); }
        return Ok(Some(intent));
    }
    // Rebuild the ordered, coalesced generation ownership from immutable intent,
    // retaining the first concrete binding exactly as the generation writer does.
    let mut keys = HashMap::<String, usize>::new();
    let mut families = Vec::new();
    let mut owners: Vec<Vec<String>> = Vec::new();
    for row in &intent.mods {
        let mut concrete=HashSet::new();
        for family in &row.packages {
            if !concrete.insert((&family.directory.id,&family.directory.source_name)) { return Err(invalid()); }
            let key = package::family_key(family)?;
            if let Some(index) = keys.get(&key).copied() {
                if !owners[index].contains(&row.mod_id) { owners[index].push(row.mod_id.clone()); }
            } else {
                keys.insert(key, families.len());
                families.push(family.clone());
                owners.push(vec![row.mod_id.clone()]);
            }
        }
    }
    let manifest = &applied["manifest"];
    let actual = crate::generation::packages(manifest)?;
    let rows = &manifest[if intent.package_version == 1 {"files"} else {"packages"}];
    if actual != families || rows.len() != owners.len() { return Err(invalid()); }
    for (row, ids) in rows.members().zip(owners.iter()) {
        if row["modId"] != ids[0] || !row["modIds"].is_array()
            || row["modIds"].members().map(|v| v.as_str()).collect::<Vec<_>>()
                != ids.iter().map(|v| Some(v.as_str())).collect::<Vec<_>>() { return Err(invalid()); }
    }
    Ok(Some(intent))
}

/// Validate stored proof before a display projection can discard a checkpoint.
/// This checks only retained applied evidence, never mutable desired selection.
pub(crate) fn validate_retained_proof(doc: &JsonValue) -> Result<(),NativeError> {
    if !doc["retiredGenerations"].is_null() && !doc["retiredGenerations"].is_array() {return Err(invalid());}
    for applied in std::iter::once(&doc["applied"]).chain(std::iter::once(&doc["previous"]["applied"]))
        .chain(doc["retiredGenerations"].members()) {
        let intent=applied_intent(applied)?;
        if applied["manifest"]["version"]==3 && (doc["storeVersion"]!=4
            || intent.as_ref().map(|i|i.package_version)!=Some(package::layout_version(doc)?)) { return Err(invalid()); }
    }
    Ok(())
}

pub const UNKNOWN_REASON: &str = "Apply review required: this checkpoint has no deployment intent.";
pub const SELECTION_REASON: &str = "Select ready packages for every enabled mod, then review Apply.";
pub const UNVERIFIED_REASON: &str = "Apply review required: the recorded game setup could not be verified.";
pub fn longest_pending_reason() -> &'static str {
    [UNKNOWN_REASON,SELECTION_REASON,UNVERIFIED_REASON].into_iter().max_by_key(|s|s.len()).unwrap()
}
pub struct PendingState {
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub pending: bool,
    desired: Option<JsonValue>,
    applied: Option<JsonValue>,
    absent: bool,
}
impl PendingState {
    pub fn compare(doc: &JsonValue, profile: &str) -> Result<Self, NativeError> {
        let applied = applied_intent(&doc["applied"])?;
        let absent = doc["applied"].is_null();
        let desired = if doc["profiles"][profile]["mods"].members().any(|m|m["enabled"]==true && m["available"]!=true) {
            Err(NativeError::new("unavailablePayload",SELECTION_REASON))
        } else { DeploymentIntent::capture(doc, profile) };
        let mut result = Self {status:"current",reason:None,pending:false,desired:None,
            applied:applied.as_ref().map(DeploymentIntent::semantic_projection).transpose()?,absent};
        match desired {
            Ok(intent) => {
                result.desired = Some(intent.semantic_projection()?);
                if !absent && applied.is_none() {
                    result.status="unknown"; result.reason=Some(UNKNOWN_REASON); result.pending=true;
                } else {
                    result.pending = match applied {Some(a)=>intent.signature()?!=a.signature()?,None=>!intent.mods.is_empty()};
                    if result.pending { result.status="pending"; }
                }
            }
            Err(e) if matches!(e.kind.as_str(),"invalidLocalSelection"|"optionNeedsPreparation"|"unavailablePayload") => {
                result.status="selectionRequired";result.reason=Some(SELECTION_REASON);result.pending=true;
            }
            Err(e) => return Err(e),
        }
        if !absent && doc["applied"]["manifest"].has_key("repairs") && doc["applied"]["manifest"]["repairs"]["choices"]!=crate::repairs::choices(doc,profile)? {result.pending=true;result.status="pending";}
        if doc["applied"]["mountedSubset"].is_array(){result.pending=true;result.status="pending";}
        Ok(result)
    }
    pub fn row_unknown(&self, enabled: bool) -> bool {
        self.status == "unknown" || (self.status == "selectionRequired" && !enabled && !self.absent && self.applied.is_none())
    }
    pub fn mod_pending(&self, id: &str, enabled: bool) -> bool {
        if self.row_unknown(enabled) { return true; }
        if self.status == "selectionRequired" {
            if enabled { return true; }
            return self.applied.as_ref().is_some_and(|a| a["mods"].members().any(|m| m["modId"] == id));
        }
        let Some(desired)=&self.desired else {return false};
        if self.absent {return enabled;}
        let Some(applied)=&self.applied else {return true};
        let a=applied["mods"].members().enumerate().find(|(_,m)|m["modId"]==id);
        let d=desired["mods"].members().enumerate().find(|(_,m)|m["modId"]==id);
        match (a,d) {
            (None,None)=>false,
            (Some((ai,a)),Some((di,d)))=>{
                // Relative precedence can change even at the same numeric
                // position: A,B,C -> C,B,A changes B's two relationships.
                let prior:HashSet<_>=applied["mods"].members().take(ai).filter_map(|m|m["modId"].as_str()).collect();
                let next:HashSet<_>=desired["mods"].members().take(di).filter_map(|m|m["modId"].as_str()).collect();
                prior!=next || a!=d || applied["profileId"]!=desired["profileId"] || applied["packageVersion"]!=desired["packageVersion"]
            }
            _=>true,
        }
    }
}
