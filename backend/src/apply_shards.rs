//! Exact game5 Apply/Restore and game6 Vanilla domains share captured shard
//! machinery. Historical flat helpers remain separate; pure views grant no ownership.
use super::*;
use crate::{deployment_layout::{DeploymentLayout, LayoutOutcome}, deployment_metadata};

pub(super) fn placement(intent: &crate::deployment_intent::DeploymentIntent, profile: &JsonValue) -> Result<DeploymentLayout, NativeError> {
    match crate::deployment_layout::plan(intent)? {
        LayoutOutcome::Fits(layout) => Ok(layout),
        LayoutOutcome::SelectionRequired(row) => Err(NativeError::new("selectionRequired", format!("{} ({}): {}", crate::profile_edit::mod_title(profile,&row.mod_id), row.mod_id, row.reason))),
        LayoutOutcome::ExceedsCapacity(row) => Err(NativeError::new("deploymentCapacity", format!("{} ({}) adds {} unique families that cannot fit in the remaining folder space. Up to ten folders hold 99 families each; ordered groups can leave unused slots. Reduce the selected files or review install order.", crate::profile_edit::mod_title(profile,&row.first_failing_mod_id), row.first_failing_mod_id, row.first_failing_group_size))),
    }
}
pub(super) fn manifest(root: &Path, id: &str, version: u32, layout: &DeploymentLayout) -> JsonValue {
    let rows: Vec<_> = layout.shards.iter().map(|shard| {
        let packages: Vec<_> = layout.families[shard.families.clone()].iter().map(|family| {
            let mut row = package::encode_packages(std::slice::from_ref(&family.package))[0].clone();
            row["modId"] = family.mod_id.clone().into();
            row["modIds"] = family.mod_ids.clone().into();
            row
        }).collect();
        object!{index:shard.index,packages:packages}
    }).collect();
    object!{version:3,packageVersion:version,storeRoot:root.to_string_lossy().as_ref(),generation:id,shards:rows}
}
pub(super) fn summary(value: &mut JsonValue, layout: &DeploymentLayout) -> Result<(), NativeError> {
    let c = &layout.counts;
    if value["packageCount"].as_u64()!=Some(c.selected_references)
        || value["directoryCount"].as_u64()!=Some(c.unique_families)
        || value["fileCount"].as_u64()!=Some(c.unique_families)
        || value["companionFileCount"].as_u64()!=Some(c.companion_files)
        || value["physicalFileCount"].as_u64()!=Some(c.physical_files)
        || value["totalBytes"].as_u64()!=Some(c.total_bytes) { return Err(recovery()); }
    value["summaryVersion"]=2.into();
    value["deploymentLayout"]="shards-v1".into();
    value["shardCount"]=layout.shards.len().into();
    value["shards"]=layout.shards.iter().map(|s|object!{index:s.index,directoryCount:s.families.len()}).collect::<Vec<_>>().into();
    Ok(())
}
pub(super) fn mounts(a: &JsonValue) -> Result<Vec<String>, NativeError> {
    if a.is_null() { Ok(Vec::new()) } else if a["mountedSubset"].is_array() {a["mountedSubset"].members().map(|v|v.as_str().map(str::to_owned).ok_or_else(recovery)).collect()} else { generation::mounts(&a["manifest"]) }
}
// Accept removal of any subset of our mounts, but still let replace_many
// enforce order, duplicates, spelling and unknown addon ownership.
pub(super) fn present_mounts(current: &str, applied: &JsonValue) -> Result<Vec<String>,NativeError> {
    let tokens=gameinfo::mounts(current)?;
    Ok(mounts(applied)?.into_iter().filter(|m|tokens.iter().any(|t|*t==*m)).collect())
}
fn rebase_document(doc: &mut JsonValue, current: &str) -> Result<(),NativeError> {
    let checkpoint=if !doc["applied"].is_null(){&doc["applied"]}else{&doc["restoredSetup"]};
    if !checkpoint.is_object() || checkpoint["gameinfo"]==current {return Ok(());}
    if !doc["applied"].is_null() && !present_mounts(current,&doc["applied"])?.is_empty() {
        // A changed file can still mount part/all of this generation. Retain it
        // as the exact current Restore target, with its existing payload proof.
        let present=present_mounts(current,&doc["applied"])?;
        let complete=generation::mounts(&doc["applied"]["manifest"])?;
        if present!=complete {doc["applied"]["mountedSubset"]=present.into();}else{doc["applied"].remove("mountedSubset");}
        doc["applied"]["gameinfo"]=current.into();
        doc["applied"]["gameinfoSha256"]=paths::digest_bytes(current.as_bytes())?.into();
    } else {
        let old=doc["applied"].clone(); append_retired(doc,old)?;
        doc["applied"]=JsonValue::Null;
    }
    let old=doc["previous"]["applied"].clone(); append_retired(doc,old)?;
    doc["previous"]=JsonValue::Null;
    doc["restoredSetup"]=JsonValue::Null; doc["canRestore"]=false.into();
    Ok(())
}
fn imported_mounts(doc: &JsonValue, c: &Path, profile:&str) -> Result<HashSet<String>, NativeError> {
    let mut result=HashSet::new();
    for row in doc["importLedger"]["manifests"].members() {
        let path=Path::new(string(row,"path")?);
        let name=path.parent().ok_or_else(recovery)?.strip_prefix(c).map_err(|_|recovery())?.to_str().ok_or_else(recovery)?;
        result.insert(format!("citadel/{name}").to_ascii_lowercase());
        for (_,entry) in row["body"]["mods"].entries() {
            if let Some(shard)=entry["shard"].as_u64().filter(|s|*s>1 && *s<=10) {
                let suffix=name.strip_prefix("addons").ok_or_else(recovery)?;
                result.insert(format!("citadel/addons{shard}{suffix}").to_ascii_lowercase());
            }
        }
    }
    // Mounts adopted by installed-game imports are owned exactly as the review counted them.
    for origin in crate::installed::origins_for_apply(doc,profile)? {
        result.insert(string(&origin,"mount")?.to_ascii_lowercase());
    }
    Ok(result)
}
fn immutable(a: &JsonValue) -> JsonValue {
    let mut a=a.clone();
    for k in ["identities","directoryIdentity","shardIdentities"] {a.remove(k);}
    a
}
fn upgrade(doc: &mut JsonValue) -> Result<(),NativeError> {
    let old=package::document_version(doc)?;
    let layout=package::layout_version(doc)?;
    doc["storeVersion"]=4.into(); doc["packageVersion"]=layout.into();
    if old<3 {doc["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
    Ok(())
}
fn append_retired(doc: &mut JsonValue, a: JsonValue) -> Result<(),NativeError> {
    if doc["retiredGenerations"].is_null() {doc["retiredGenerations"]=JsonValue::new_array();}
    if !a.is_null() {doc["retiredGenerations"].push(a).map_err(|_|recovery())?;}
    Ok(())
}
/// Projection is used ONLY for budgets and pure persisted-shape admission.
/// It is never returned as actual captured evidence or written to the Store.
fn projected_document(j: &JsonValue) -> Result<JsonValue,NativeError> {
    let mut doc=j["document"].clone();
    if !j["generation"].is_null() {
        doc["applied"]=deployment_metadata::reserve_shard_checkpoint(&immutable(&doc["applied"]))?;
    }
    Ok(doc)
}
#[derive(Clone)]
struct Node { name: String, directory: bool, payload: Option<package::FileRef> }
fn nodes(a: &JsonValue) -> Result<Vec<Node>,NativeError> {
    let view=generation::decode(&a["manifest"])?;
    if view.version!=3 {return Err(recovery());}
    let mut nodes:Vec<_>=view.directories.into_iter().map(|d|Node{name:d.relative_path,directory:true,payload:None}).collect();
    nodes.extend(view.files.into_iter().map(|f|Node{name:f.name,directory:false,payload:Some(f.payload)}));
    nodes.push(Node{name:"manifest.json".into(),directory:false,payload:None});
    Ok(nodes)
}
fn birth(m: &fs::Metadata) -> JsonValue {object!{device:m.dev(),inode:m.ino()}}
fn max_identity(size: u64) -> JsonValue {object!{size:size,device:u64::MAX,inode:u64::MAX,modifiedSeconds:i64::MIN,modifiedNanos:999_999_999,fsid:"ffffffffffffffff"}}
fn node_record(node: &Node, owner: &JsonValue, complete: bool) -> JsonValue {
    let mut row=object!{kind:if node.directory {"directory"} else {"file"},name:node.name.clone(),device:owner["device"].clone(),inode:owner["inode"].clone()};
    if owner.has_key("fsid") {row["fsid"]=owner["fsid"].clone();}
    if !node.directory {row["complete"]=complete.into();}
    row
}
pub(super) fn reserved(j: &JsonValue) -> Result<JsonValue,NativeError> {
    let mut p=j.clone();
    p["document"]=projected_document(j)?;
    if !j["generation"].is_null() {
        p["stagedNodes"]=nodes(&j["document"]["applied"])?.iter().map(|n|node_record(n,&max_identity(0),false)).collect::<Vec<_>>().into();
    }
    p["tempIdentity"]=max_identity(0);
    // The immutable gameinfo before image uses a real identity, not a late one.
    store::ensure_readable(&p["beforeDocument"])?;
    store::ensure_readable(&p["document"])?;
    store::ensure_internal(&p)?;
    Ok(p)
}
fn admit_staging(j: &JsonValue) -> Result<(),NativeError> {
    if !j["tempIdentity"].is_null() {
        game_identity(&j["tempIdentity"])?;
        if j["tempIdentity"]["size"]!=0 {return Err(recovery());}
    }
    if j["generation"].is_null() {
        if !j["stagedNodes"].is_null() {return Err(recovery());}
        return Ok(());
    }
    let a=&j["document"]["applied"];
    let allowed=nodes(a)?;
    let staged=&j["stagedNodes"];
    if !staged.is_array() || staged.len()>allowed.len() {return Err(recovery());}
    let mut owners=HashSet::new();
    for (i,(row,node)) in staged.members().zip(&allowed).enumerate() {
        exact_keys(row,if node.directory {&["kind","name","device","inode"]} else {&["kind","name","device","inode","complete"]},&["fsid"])?;
        if !generation::valid_fsid(row) || row["name"]!=node.name || row["kind"]!=if node.directory {"directory"} else {"file"}
            || row["device"].as_u64().is_none() || row["inode"].as_u64().filter(|v|*v!=0).is_none()
            || !node.directory && (row["complete"].as_bool().is_none() || i+1<staged.len() && row["complete"]!=true) {return Err(recovery());}
        if !owners.insert((row["device"].as_u64(),row["inode"].as_u64())) {return Err(recovery());}
    }
    if a.has_key("identities") {
        if staged.len()!=allowed.len() || staged.members().any(|r|r["kind"]=="file" && r["complete"]!=true) {return Err(recovery());}
        for (row,node) in staged.members().zip(&allowed) {
            let id=if node.directory {if node.name.is_empty(){&a["directoryIdentity"]}else{&a["shardIdentities"][&node.name]}}else{&a["identities"][&node.name]};
            if id["device"]!=row["device"] || id["inode"]!=row["inode"] {return Err(recovery());}
        }
    } else if !j["tempIdentity"].is_null() {return Err(recovery());}
    Ok(())
}
/// Exact game5 transition admission, including the explicitly prospective eight
/// fields. Only the projection below can enter persisted Store validation.
pub(super) fn admit(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    admit_version(store,id,j,false)
}
pub(super) fn admit6(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    admit_version(store,id,j,true)
}
fn admit_transition(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    match j["version"].as_u32() { Some(5)=>admit(store,id,j), Some(6)=>admit6(store,id,j), _=>Err(recovery()) }
}
fn admit_version(store: &Store, id: &str, j: &JsonValue, vanilla: bool) -> Result<(),NativeError> {
    exact_keys(j,&["kind","version","input","expectedRevision","citadel","before","after","beforeSha256","afterSha256","beforeIdentity","document","beforeDocument","workspaceExisted","temp","generation","libraryVersionBefore","libraryVersionAfter","packageVersion","fontStateDigest","deploymentIntent","desiredSignature","stagedNodes","tempIdentity"],&[])?;
    let before=&j["beforeDocument"]; let after=&j["document"];
    let layout=package::layout_version(before)?;
    if j["kind"]!="game" || j["version"]!=(if vanilla {6} else {5}) || j["libraryVersionBefore"].as_u32()!=Some(package::document_version(before)?)
        || j["libraryVersionAfter"]!=4 || after["storeVersion"]!=4 || package::layout_version(after)?!=layout
        || j["packageVersion"].as_u32()!=Some(layout) || before["revision"]!=j["expectedRevision"] || after["revision"]!=j["expectedRevision"]
        || j["workspaceExisted"].as_bool().is_none()
        || !string(j,"temp")?.strip_prefix(".native-gameinfo-").is_some_and(|s|package::hex(s,32)) {return Err(recovery());}
    let c=Path::new(string(j,"citadel")?);
    let game=Path::new(string(before,"gamePath")?);
    if !game.is_absolute() || paths::absolute(game)?!=game || game.join("game/citadel")!=c {return Err(recovery());}
    for k in ["before","after"] {if string(j,k)?.len()>MAX_GAMEINFO as usize {return Err(recovery());}}
    verify_journal_bytes(j)?; game_identity(&j["beforeIdentity"])?;
    if j["beforeIdentity"]["size"].as_u64()!=Some(string(j,"before")?.len() as u64) {return Err(recovery());}
    deployment_metadata::admit(store.root(),before)?; store::ensure_readable(before)?;
    let apply=vanilla || j["input"]["method"]=="native.apply";
    if vanilla { admit_vanilla_input(id,&j["input"],&j["expectedRevision"])?; }
    operation_binding(id,&j["input"],&j["expectedRevision"],if vanilla {"native.vanilla"} else if apply {"native.apply"} else {"native.restore"})?;
    if j["fontStateDigest"]!=crate::font_transaction::state_digest(before)?
        || j["fontStateDigest"]!=crate::font_transaction::state_digest(after)?
        || j["deploymentIntent"]!=after["applied"]["deploymentIntent"] || j["desiredSignature"]!=after["applied"]["desiredSignature"] {return Err(recovery());}
    let mut projected=before.clone();
    if apply {
        if j["input"]["params"]["deploymentLayout"]!="shards-v1"
            || !j["input"]["params"]["token"].as_str().is_some_and(|s|package::hex(s,32))
            || !j["generation"].as_str().is_some_and(|s|package::hex(s,32)) || after["applied"]["generation"]!=j["generation"] {return Err(recovery());}
        let a=&after["applied"];
        let base=immutable(a);
        deployment_metadata::reserve_shard_checkpoint(&base)?;
        if a.has_key("identities") {
            deployment_metadata::admit(store.root(),after)?;
        } else if *a!=base {return Err(recovery());}
        let intent=crate::deployment_intent::applied_intent(a)?.ok_or_else(recovery)?;
        let expected_intent=if vanilla { crate::deployment_intent::DeploymentIntent::empty_active(before)? }
            else { crate::deployment_intent::DeploymentIntent::capture(before,&intent.profile_id)? };
        if expected_intent!=intent
            || a["gameinfo"]!=j["after"] || a["gameinfoSha256"]!=j["afterSha256"] {return Err(recovery());}
        let mut enabled=JsonValue::new_object();
        for m in before["profiles"][&intent.profile_id]["mods"].members() {enabled[string(&m["source"],"id")?]=(!vanilla && m["enabled"].as_bool().unwrap_or(false)).into();}
        if a["enabled"]!=enabled {return Err(recovery());}
        let mut imported=imported_mounts(before,c,&intent.profile_id)?;
        if !vanilla && a["manifest"].has_key("repairs"){imported.insert("citadel/dmm_localization_default".into());}
        let expected_after=gameinfo::replace_many_unmanaged(string(j,"before")?,&imported,&present_mounts(string(j,"before")?,&before["applied"])?,&mounts(a)?,&unmanaged_mounts(before))?.0;
        // Both legacy writers used an empty string: one preserved the overlay,
        // the other removed it. Admit only either exact historical result.
        let expected_after = if !vanilla && before["performancePreset"] == "" {
            if j["after"] == expected_after { expected_after } else {
                let removed = crate::performance::patch::strip_overlay(&expected_after).0;
                crate::performance::patch::verify_stripped(&expected_after, &removed)?;
                removed
            }
        } else {
            crate::performance::overlay(&expected_after,if vanilla {""}else{before["performancePreset"].as_str().unwrap_or("")})?
        };
        if j["after"]!=expected_after {return Err(recovery());}
        rebase_document(&mut projected,string(j,"before")?)?;
        let effective_before=projected.clone();
        let previous=&after["previous"];
        exact_keys(previous,&["applied","activeProfileId","gameinfo","gameinfoSha256","upstreamPayloads","upstreamEvidenceVersion"],&["installedGameOrigins","installedBaselineUnavailable"])?;
        if previous["applied"]!=effective_before["applied"] || previous["activeProfileId"]!=before["activeProfileId"]
            || previous["gameinfo"]!=j["before"] || previous["gameinfoSha256"]!=j["beforeSha256"]
            || previous["upstreamEvidenceVersion"].as_u32()!=Some(layout) || !previous["upstreamPayloads"].is_array()
             {return Err(recovery());}
        if checkpoints(before).any(|a|!a.is_null() && a["generation"]==j["generation"]) {return Err(recovery());}
        append_retired(&mut projected,effective_before["previous"]["applied"].clone())?;
        upgrade(&mut projected)?;
        projected["applied"]=a.clone(); projected["previous"]=previous.clone();
        projected["activeProfileId"]=intent.profile_id.into(); projected["restoredSetup"]=JsonValue::Null; projected["canRestore"]=true.into();
    } else {
        if !deployment_metadata::retained_shards(before) || !j["generation"].is_null() || !j["stagedNodes"].is_null()
            || j["workspaceExisted"]!=true || before["canRestore"]!=true || !before["previous"].is_object()
            || j["after"]!=before["previous"]["gameinfo"] || j["afterSha256"]!=before["previous"]["gameinfoSha256"] {return Err(recovery());}
        deployment_metadata::admit(store.root(),after)?;
        append_retired(&mut projected,before["applied"].clone())?;
        projected["applied"]=before["previous"]["applied"].clone(); projected["activeProfileId"]=before["previous"]["activeProfileId"].clone();
        if after["restoredSetup"].is_null() && (before["previous"]["applied"].is_null() || !before["previous"]["upstreamPayloads"].is_empty()){return Err(recovery());}
        projected["restoredSetup"]=if after["restoredSetup"].is_object(){admitted_restore_setup(&before["previous"],&after["restoredSetup"])?}else{JsonValue::Null};
        projected["previous"]=JsonValue::Null; projected["canRestore"]=false.into();
    }
    if j["generation"].is_string() && projected.has_key("gameFolderChanged") {projected["gameFolderChanged"]=false.into();}
    projected["recoveryRequired"]=false.into();
    if projected!=*after {return Err(recovery());}
    for (doc,key,digest) in [(before,"before","beforeSha256"),(after,"after","afterSha256")] {
        let state=if !doc["applied"].is_null(){&doc["applied"]}else{&doc["restoredSetup"]};
        // Re-Apply journals capture current bytes independently of the stale checkpoint.
        if !(apply && key=="before") && !state.is_null() && (state["gameinfo"]!=j[key] || state["gameinfoSha256"]!=j[digest]) {return Err(recovery());}
    }
    let budget=projected_document(j)?;
    deployment_metadata::admit(store.root(),&budget)?;
    admit_staging(j)?;
    let current=store.document()?;
    let outcome=store.operation(id)?;
    if outcome["status"]=="succeeded" {
        let result=store.replay(id,&j["input"])?.ok_or_else(recovery)?;
        let mut committed=after.clone(); committed["revision"]=rev(before)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
        deployment_metadata::admit(store.root(),&committed)?;
        if current!=committed || j["tempIdentity"].is_null() || !store::matches_recorded_snapshot(&committed,&result)? {return Err(recovery());}
    } else {
        if outcome["status"]=="failed" {
            let failure=store.replay(id,&j["input"]).err().ok_or_else(recovery)?;
            let expected=game_rollback_error();
            if failure.kind!=expected.kind || failure.message!=expected.message {return Err(recovery());}
        }
        if current!=*before {return Err(recovery());}
    }
    reserved(j)?;
    Ok(())
}
fn checkpoints(doc: &JsonValue) -> impl Iterator<Item=&JsonValue> {
    std::iter::once(&doc["applied"]).chain(std::iter::once(&doc["previous"]["applied"])).chain(doc["retiredGenerations"].members())
}

fn ordinary_directory(path: &Path) -> Result<fs::Metadata,NativeError> {
    paths::reject_symlinks(path)?;
    let m=fs::symlink_metadata(path)?;
    if !m.is_dir() {return Err(external());}
    Ok(m)
}
fn membership(path: &Path, expected: &HashSet<String>) -> Result<(),NativeError> {
    let mut remaining=expected.clone();
    ordinary_directory(path)?;
    for e in fs::read_dir(path)? {
        let e=e?;
        let name=e.file_name().into_string().map_err(|_|external())?;
        if !remaining.remove(&name) {return Err(external());}
        paths::reject_symlinks(&e.path())?;
    }
    if !remaining.is_empty() {return Err(external());}
    Ok(())
}
/// Reconcile only device/fsid ownership under the caller's held game lock.
/// This temporary view grants no new evidence to the persisted checkpoint.
pub(super) fn verify_checkpoint_locked(c: &Path, root: &Path, a: &JsonValue, fast: bool, guard: &GameLock) -> Result<(),NativeError> {
    guard.validate(c)?;
    if a.is_null() {return Ok(());}
    check_checkpoint(c,root,a,fast,Some(guard))
}
pub(super) fn check_checkpoint(c:&Path,root:&Path,a:&JsonValue,fast:bool,authority:Option<&GameLock>)->Result<(),NativeError>{
    if let Some(guard)=authority {guard.validate(c)?;}
    if a.is_null(){return Ok(());}
    let (checked,changed)=checkpoint_view(c,a,authority)?;
    verify_checkpoint_strict(c,root,&checked,fast && !changed)
}
fn checkpoint_view(c:&Path,a:&JsonValue,authority:Option<&GameLock>)->Result<(JsonValue,bool),NativeError>{
    if let Some(guard)=authority {guard.validate(c)?;}
    let view=generation::decode(&a["manifest"])?;
    let dir=generation_dir(c,a)?;
    let mut checked=a.clone();
    let mut changed=false;
    let mut reconcile=|path:&Path,id:&mut JsonValue| -> Result<(),NativeError> {
        if !id.is_object() {return Ok(());}
        if !generation::owner_matches(path,id,authority)? {return Err(external());}
        let m=fs::symlink_metadata(path)?;
        let device_changed=id["device"].as_u64()!=Some(m.dev());
        // Pre-fsid checkpoints did not record immutability. Admit their modes
        // under the same ownership checks, but verify writable legacy bytes.
        if view.version==3 && m.is_file() && id.has_key("fsid") && m.mode() & 0o7222 != 0 {return Err(external());}
        changed |= device_changed || (!id.has_key("fsid") && m.is_file() && m.mode() & 0o222 != 0);
        id["device"]=m.dev().into();
        id.remove("fsid");
        Ok(())
    };
    reconcile(&dir,&mut checked["directoryIdentity"])?;
    for d in view.directories.iter().skip(1) {reconcile(&dir.join(&d.relative_path),&mut checked["shardIdentities"][&d.relative_path])?;}
    for name in view.files.iter().map(|f|f.name.as_str()).chain(std::iter::once("manifest.json")) {
        reconcile(&dir.join(name),&mut checked["identities"][name])?;
    }
    Ok((checked,changed))
}

/// The new dispatcher is explicit: legacy filesystem helpers retain their own
/// flat ceiling; only this branch understands generation3 directory ownership.
pub(super) fn verify_checkpoint(c:&Path,root:&Path,a:&JsonValue,fast:bool)->Result<(),NativeError>{check_checkpoint(c,root,a,fast,None)}
fn verify_checkpoint_strict(c: &Path, root: &Path, a: &JsonValue, fast: bool) -> Result<(),NativeError> {
    if a.is_null() {return Ok(());}
    if a["manifest"]["version"]!=3 {return verify_generation_mode(c,root,a,fast);}
    let view=generation::decode(&a["manifest"])?;
    crate::deployment_intent::applied_intent(a)?.ok_or_else(recovery)?;
    generation::validate_tree_identities(&view,&a["directoryIdentity"],&a["shardIdentities"],&a["identities"])?;
    if view.store_root!=root || view.generation!=string(a,"generation")? || !workspace(c,root)? {return Err(external());}
    checkpoint_records(a)?; // Captured tree nodes have distinct ownership.
    let dir=generation_dir(c,a)?;
    for d in &view.directories {
        let path=dir.join(&d.relative_path);
        let id=if d.relative_path.is_empty(){&a["directoryIdentity"]}else{&a["shardIdentities"][&d.relative_path]};
        if identity(&ordinary_directory(&path)?)!=*id {return Err(external());}
        membership(&path,&d.children.iter().cloned().collect())?;
    }
    for file in &view.files {
        let path=dir.join(&file.name);
        let m=paths::open_regular(&path)?.metadata()?;
        let id=&a["identities"][&file.name];
        if !generation::same_owner(&m,id) {return Err(external());}
        if fast {paths::HashMemo::recheck_if_known(&path)?;}
        if (!fast && file.name=="repairs/pak01_dir.vpk") || (fast && identity(&m)!=*id) {generation::verify_file(&path,&file.payload)?;}
    }
    let marker=dir.join("manifest.json");
    if !generation::same_owner(&paths::open_regular(&marker)?.metadata()?,&a["identities"]["manifest.json"])
        || paths::read_bounded(&marker,paths::MAX_JSON_BYTES)?!=a["manifest"].dump().as_bytes() {return Err(external());}
    // A receipt was created only after complete-family validation in a
    // successful Apply. Unchanged read-only identities retain that proof.
    let mut verified=true;
    for file in &view.files {verified &= paths::HashMemo::verified(&dir.join(&file.name),&file.payload.sha256,file.payload.size)?;}
    // New or changed destination content still receives full-family validation.
    for family in view.families.iter().filter(|_|!fast && !verified) {
        let prefix=format!("shard{:02}/pak{:02}",family.shard_index.ok_or_else(recovery)?,family.slot);
        generation::verify_package(&family.package,view.package_version,|file| {
            if *file==family.package.directory {return Ok(dir.join(format!("{prefix}_dir.vpk")));}
            let index=family.package.companions.iter().find(|(_,f)|*f==file).map(|(i,_)|i).ok_or_else(recovery)?;
            Ok(dir.join(format!("{prefix}_{index:03}.vpk")))
        })?;
    }
    Ok(())
}
fn capture(c: &Path, a: &mut JsonValue, staged: &JsonValue) -> Result<(),NativeError> {
    let allowed=nodes(a)?;
    let dir=generation_dir(c,a)?;
    let mut files=JsonValue::new_object(); let mut directories=JsonValue::new_object();
    if staged.len()!=allowed.len() {return Err(recovery());}
    for (node,row) in allowed.iter().zip(staged.members()) {
        let path=dir.join(&node.name);
        let m=if node.directory {ordinary_directory(&path)?} else {paths::open_regular(&path)?.metadata()?};
        if !generation::same_owner(&m,row) || !node.directory && row["complete"]!=true {return Err(external());}
        if node.directory {
            if node.name.is_empty(){a["directoryIdentity"]=generation::capture_identity(&path)?;}else{directories[&node.name]=generation::capture_identity(&path)?;}
        } else {files[&node.name]=generation::capture_identity(&path)?;}
    }
    a["shardIdentities"]=directories; a["identities"]=files;
    Ok(())
}
/// Check the complete native namespace before a new tree, publication, or any
/// unlink. Unknown generation names remain evidence; they are never adopted.
fn namespace(store: &Store, c: &Path, docs: &[&JsonValue], staged: Option<&str>) -> Result<(),NativeError> {
    let w=c.join("deadlock_native");
    if !path_exists(&w)? {return Ok(());}
    if !workspace(c,store.root())? {return Err(external());}
    ordinary_directory(&w)?;
    let mut allowed=HashSet::from(["owner.json".to_owned()]);
    for doc in docs {for a in checkpoints(doc).filter(|a|!a.is_null()) {allowed.insert(string(a,"generation")?.to_owned());}}
    for doc in docs {for id in doc["unmanagedGameGenerations"].members(){allowed.insert(id.as_str().ok_or_else(external)?.to_owned());}}
    if let Some(id)=staged {allowed.insert(id.to_owned());}
    for entry in fs::read_dir(&w)? {
        let entry=entry?;
        let name=entry.file_name().into_string().map_err(|_|external())?;
        if !allowed.contains(&name) {return Err(external());}
        if name=="owner.json" {paths::open_regular(&entry.path())?;} else {ordinary_directory(&entry.path())?;}
    }
    Ok(())
}
pub(super) fn verify_preparation_namespace(store: &Store, c: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<(),NativeError> {
    namespace(store, c, &[doc], None)?;
    check_checkpoint(c, store.root(), &doc["previous"]["applied"], false, authority)?;
    retired_plan(store, c, doc, authority)?;
    if !doc["applied"].is_null() && !check_external(store.root(),doc, authority)? {
        let mounts = mounts(&doc["applied"])?;
        // No imported addon mounts may remain in a prepared native checkpoint.
        let (_, replaced) = gameinfo::replace_many_unmanaged(&text(&c.join("gameinfo.gi"))?, &HashSet::new(), &mounts, &mounts,&unmanaged_mounts(doc))?;
        if replaced != mounts { return Err(external()); }
    }
    Ok(())
}
fn verify_prefix(path: &Path, expected: &[u8]) -> Result<String,NativeError> {
    let data=paths::read_bounded(path,expected.len() as u64)?;
    if !expected.starts_with(&data) {return Err(external());}
    paths::digest_bytes(&data)
}
fn verify_payload_prefix(store: &Store, path: &Path, file: &package::FileRef, manifest: &JsonValue) -> Result<String,NativeError> {
    let mut actual=paths::open_regular(path)?;
    let size=actual.metadata()?.len();
    if size>file.size {return Err(external());}
    let source_path=crate::repairs::source(store,manifest,file)?;
    let mut source=paths::open_regular(&source_path)?;
    use sha2::{Digest,Sha256};let mut hash=Sha256::new();let mut source_hash=Sha256::new();
    let source_identity=identity(&source.metadata()?);let actual_identity=identity(&actual.metadata()?);
    let mut a=[0u8;65536];let mut b=[0u8;65536];let mut offset=0u64;
    loop {
        let n=source.read(&mut b)?;if n==0{break;}source_hash.update(&b[..n]);
        let compare=size.saturating_sub(offset).min(n as u64) as usize;
        if compare>0 {actual.read_exact(&mut a[..compare])?;if a[..compare]!=b[..compare]{return Err(external());}hash.update(&a[..compare]);}
        offset+=n as u64;
    }
    if offset!=file.size || format!("{:x}",source_hash.finalize())!=file.sha256 || offset<size
        || identity(&actual.metadata()?)!=actual_identity || identity(&source.metadata()?)!=source_identity {return Err(external());}
    Ok(format!("{:x}",hash.finalize()))
}
#[derive(Clone)]
struct Removal {path:PathBuf,directory:bool,owner:JsonValue,sha256:Option<String>,mutation:Option<paths::DestructiveFile>}
pub(super) struct Cleanup {remove:Vec<Removal>}
#[cfg(test)]
thread_local! {pub(super) static BEFORE_UNLINK:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
impl Cleanup {
    pub(super) fn execute(self) -> Result<bool,NativeError> {
        let mut parents=HashMap::new();
        for row in &self.remove {
            let parent=row.path.parent().ok_or_else(recovery)?.to_path_buf();
            if !parents.contains_key(&parent) {
                let owner=if let Some(r)=self.remove.iter().find(|r|r.path==parent && r.directory){r.owner.clone()}else{identity(&ordinary_directory(&parent)?)};
                let children=if self.remove.iter().any(|r|r.path==parent && r.directory){
                    self.remove.iter().filter(|r|r.path.parent()==Some(parent.as_path())).map(|r|r.path.file_name().unwrap().to_string_lossy().into_owned()).collect::<HashSet<_>>()
                }else{fs::read_dir(&parent)?.map(|e|Ok(e?.file_name().to_string_lossy().into_owned())).collect::<Result<HashSet<_>,std::io::Error>>()?};
                parents.insert(parent,(owner,children));
            }
        }
        let mut retained:Vec<PathBuf>=Vec::new();
        for row in self.remove {
            if row.directory && retained.iter().any(|p|p.starts_with(&row.path)){continue;}
            let m=if row.directory {ordinary_directory(&row.path)?} else {paths::open_regular(&row.path)?.metadata()?};
            if !generation::same_owner(&m,&row.owner) {return Err(external());}
            // Rehash immediately before unlink; preflight is not deletion authority.
            if let Some(sha)=&row.sha256 {if paths::fingerprint(&row.path)?.sha256!=*sha {return Err(external());}}
            #[cfg(test)]
            BEFORE_UNLINK.with(|h|if let Some(h)=h.borrow_mut().take(){h();});
            let parent=row.path.parent().ok_or_else(recovery)?;
            let (owner,children)=parents.get_mut(parent).ok_or_else(recovery)?;
            if !generation::same_owner(&ordinary_directory(parent)?,owner){return Err(external());}
            membership(parent,children)?;
            if row.directory {membership(&row.path,&HashSet::new())?;}
            if row.directory {fs::remove_dir(&row.path)?;}else if !row.mutation.as_ref().ok_or_else(recovery)?.unlink()? {retained.push(row.path);continue;}
            if !row.directory {paths::acknowledge_link_change(m.dev(),m.ino())?;}
            children.remove(&row.path.file_name().ok_or_else(recovery)?.to_string_lossy().into_owned());
            paths::sync_dir(parent)?;
        }
        Ok(retained.is_empty())
    }
}
/// Existing typed nodes may be absent after an interrupted cleanup. Every
/// present node must still have its captured identity/content. The whole tree
/// and each exact immediate child set are checked before returning any unlink.
fn partial_tree(store: &Store, c: &Path, a: &JsonValue, staged: &JsonValue, completed: bool, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    partial_tree_mode(store,c,a,staged,completed,true, authority)
}
fn partial_tree_mode(store: &Store, c: &Path, a: &JsonValue, staged: &JsonValue, completed: bool, strong:bool, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    let allowed=nodes(a)?;
    let dir=generation_dir(c,a)?;
    let mut present=HashSet::new(); let mut remove=Vec::new();
    for (i,node) in allowed.iter().enumerate() {
        let path=dir.join(&node.name);
        if !path_exists(&path)? {continue;}
        let row=&staged[i];
        if row.is_null() {return Err(external());}
        let m=if node.directory {ordinary_directory(&path)?}else{paths::open_regular(&path)?.metadata()?};
        if !generation::owner_matches(&path,row,authority)? {return Err(external());}
        let mut partial_hash=None;
        if !node.directory {
            if node.name=="manifest.json" {
                let bytes=a["manifest"].dump();
                if completed || row["complete"]==true {
                    if paths::read_bounded(&path,paths::MAX_JSON_BYTES)?!=bytes.as_bytes(){return Err(external());}
                } else {partial_hash=Some(verify_prefix(&path,bytes.as_bytes())?);}
            } else {
                let payload=node.payload.as_ref().ok_or_else(recovery)?;
                if completed || row["complete"]==true {if strong {generation::verify_file(&path,payload)?;} else if !generation::metadata_matches(&path,&a["identities"][&node.name],authority)? {return Err(external());}}
                else {partial_hash=Some(verify_payload_prefix(store,&path,payload,&a["manifest"])?);}
            }
        }
        present.insert(node.name.clone());
        let recorded_hash=if let Some(file)=&node.payload{file.sha256.clone()}else{paths::digest_bytes(a["manifest"].dump().as_bytes())?};
        let mutation=if node.directory{None}else{Some(paths::DestructiveFile::admitted(path.clone(),row,Some(recorded_hash))?)};
        remove.push(Removal{mutation,sha256:if node.directory {None}else if completed || row["complete"]==true {Some(if let Some(f)=&node.payload {f.sha256.clone()}else{paths::digest_bytes(a["manifest"].dump().as_bytes())?})}else{partial_hash},path,directory:node.directory,owner:identity(&m)});
    }
    for node in allowed.iter().filter(|n|n.directory && present.contains(&n.name)) {
        let children=present.iter().filter_map(|name| {
            if name.is_empty(){return None;}
            let path=Path::new(name);
            if path.parent().and_then(Path::to_str)==Some(node.name.as_str()) {path.file_name().and_then(|s|s.to_str()).map(str::to_owned)}else{None}
        }).collect();
        membership(&dir.join(&node.name),&children)?;
    }
    remove.reverse();
    Ok(Cleanup{remove})
}
fn checkpoint_records(a: &JsonValue) -> Result<JsonValue,NativeError> {
    let mut owners=HashSet::new();
    let mut rows=Vec::new();
    for node in nodes(a)? {
        let id=if node.directory {if node.name.is_empty(){&a["directoryIdentity"]}else{&a["shardIdentities"][&node.name]}}else{&a["identities"][&node.name]};
        game_identity(id)?;
        if !owners.insert((id["device"].as_u64(),id["inode"].as_u64())) {return Err(recovery());}
        rows.push(node_record(&node,id,true));
    }
    Ok(rows.into())
}
fn retired_plan(store: &Store, c: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {retired_plan_mode(store,c,doc,true, authority)}
fn retired_plan_mode(store: &Store, c: &Path, doc: &JsonValue,strong:bool, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    let mut remove=Vec::new(); let mut seen=HashSet::new();
    for a in doc["retiredGenerations"].members() {
        let id=string(a,"generation")?;
        if a["generation"]==doc["applied"]["generation"] || a["generation"]==doc["previous"]["applied"]["generation"] || !seen.insert(id) {return Err(recovery());}
        if a["manifest"]["version"]==3 {
            remove.extend(partial_tree_mode(store,c,a,&checkpoint_records(a)?,true,strong, authority)?.remove);
        } else {
            // Historical gen1 has no durable directory birth identity. Preserve
            // it. Complete legacy gen2 captures are handled within flat bounds.
            let dir=generation_dir(c,a)?;
            if !path_exists(&dir)? {continue;}
            generation::packages(&a["manifest"])?;
            if !retirement_owned(a)? {continue;}
            if !generation::owner_matches(&dir,&a["directoryIdentity"],authority)? {return Err(external());}
            let mut known=HashSet::new(); let mut pending=Vec::new();
            for (name,payload) in generation_files(&a["manifest"])?.into_iter().map(|f|(f.name,Some(f.payload)))
                .chain(std::iter::once(("manifest.json".into(),None))) {
                let path=dir.join(&name);
                if !path_exists(&path)? {continue;}
                if !generation::owner_matches(&path,&a["identities"][&name],authority)? {return Err(external());}
                let sha=if let Some(payload)=payload {if strong {generation::verify_file(&path,&payload)?;} else if !generation::metadata_matches(&path,&a["identities"][&name],authority)? {return Err(external());}payload.sha256}
                else {let bytes=paths::read_bounded(&path,paths::MAX_JSON_BYTES)?;if bytes!=a["manifest"].dump().as_bytes(){return Err(external());}paths::digest_bytes(&bytes)?};
                known.insert(name.clone());
                let owner=identity(&fs::symlink_metadata(&path)?);pending.push(Removal{mutation:Some(paths::DestructiveFile::admitted(path.clone(),&a["identities"][&name],Some(sha.clone()))?),sha256:Some(sha),path,directory:false,owner});
            }
            membership(&dir,&known)?;
            pending.reverse(); remove.extend(pending);
            let owner=identity(&fs::symlink_metadata(&dir)?);remove.push(Removal{mutation:None,sha256:None,path:dir,directory:true,owner});
        }
    }
    Ok(Cleanup{remove})
}
/// Exact reachable retirement completion image. Only roots in this admitted
/// removal plan or already-proven absent roots may leave the retained metadata.
fn retirement_document(c:&Path,doc:&JsonValue,cleanup:&Cleanup) -> Result<JsonValue,NativeError> {
    let mut pruned=doc.clone(); let mut retained=Vec::new();
    let removed:HashSet<_>=cleanup.remove.iter().filter(|r|r.directory).map(|r|&r.path).collect();
    for a in doc["retiredGenerations"].members() {
        let dir=generation_dir(c,a)?;
        if !removed.contains(&dir) && path_exists(&dir)? {retained.push(a.clone());}
    }
    pruned["retiredGenerations"]=retained.into();
    Ok(pruned)
}
/// Both the full committed image and the eventual same-revision pruning image
/// are budgeted before creation, with the same widest late captured identities.
pub(super) fn reserve_retirement(store:&Store,c:&Path,j:&JsonValue, authority: Option<&GameLock>) -> Result<JsonValue,NativeError> {
    let full=reserved_game_document(j)?;
    let cleanup=retired_plan_mode(store,c,&full,false, authority)?;
    let pruned=retirement_document(c,&full,&cleanup)?;
    deployment_metadata::admit(store.root(),&pruned)?;
    store::admit_success(string(&j["input"]["params"],"operationId")?,&j["input"],&pruned)?;
    Ok(pruned)
}
/// Predict only the terminal document digest. This grants no filesystem or
/// cleanup authority: forward/rollback preflight separately verifies every
/// retained object under the admitted GameLock before any mutation.
pub(super) fn recovery_after_digest(store:&Store,j:&JsonValue) -> Result<JsonValue,NativeError> {
    if !j["document"]["applied"].has_key("identities") {return Ok(JsonValue::Null);}
    let c=Path::new(string(j,"citadel")?);
    let mut doc=j["document"].clone();
    doc["revision"]=store::revision(string(j,"expectedRevision")?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
    let mut retained=Vec::new();
    for a in doc["retiredGenerations"].members() {
        let owned=if a["manifest"]["version"]==3 {
            let view=generation::decode(&a["manifest"])?;
            generation::validate_tree_identities(&view,&a["directoryIdentity"],&a["shardIdentities"],&a["identities"])?;
            checkpoint_records(a)?;true
        }else{retirement_owned(a)?};
        // Complete captured inventories predict removal only after the separate
        // physical preflight succeeds. Unowned historical trees stay retained.
        if !owned && path_exists(&generation_dir(c,a)?)? {retained.push(a.clone());}
    }
    doc["retiredGenerations"]=retained.into();
    deployment_metadata::admit(store.root(),&doc)?;
    Ok(paths::digest_bytes(store::canonical(&doc).as_bytes())?.into())
}
pub(super) fn rollback_plan(store: &Store,c:&Path,j:&JsonValue, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    namespace(store,c,&[&j["beforeDocument"],&j["document"]],j["generation"].as_str())?;
    // All retained owners are preflighted even when rollback only deletes the
    // newly staged tree. Retirement plans are discarded on rollback.
    retired_plan(store,c,&j["beforeDocument"], authority)?;
    check_checkpoint(c,store.root(),&j["beforeDocument"]["previous"]["applied"],false, authority)?;
    let mut remove=Vec::new();
    let temp=c.join(string(j,"temp")?);
    if path_exists(&temp)? {
        if j["tempIdentity"].is_null() || !generation::owner_matches(&temp,&j["tempIdentity"],authority)? {return Err(external());}
        let sha=verify_prefix(&temp,string(j,"after")?.as_bytes())?;
        let owner=identity(&fs::symlink_metadata(&temp)?);remove.push(Removal{mutation:Some(paths::DestructiveFile::admitted(temp.clone(),&j["tempIdentity"],Some(paths::digest_bytes(string(j,"after")?.as_bytes())?))?),sha256:Some(sha),path:temp,directory:false,owner});
    }
    if !j["generation"].is_null() {
        remove.extend(partial_tree(store,c,&j["document"]["applied"],&j["stagedNodes"],false, authority)?.remove);
    }
    Ok(Cleanup{remove})
}

fn prepare_transition(store:&Store,lock:&RootLock,id:&str,input:&JsonValue,expected:u64,c:&Path,before:&str,after:&str,doc:JsonValue,owned:bool,generation:Option<&str>,vanilla:bool, authority: Option<&GameLock>) -> Result<JsonValue,NativeError> {
    let before_doc=store.document()?;
    let j=object!{kind:"game",version:if vanilla {6} else {5},input:input.clone(),expectedRevision:expected.to_string(),citadel:c.to_string_lossy().as_ref(),
        before:before,after:after,beforeSha256:paths::digest_bytes(before.as_bytes())?,afterSha256:paths::digest_bytes(after.as_bytes())?,
        beforeIdentity:generation::capture_identity(&c.join("gameinfo.gi"))?,
        libraryVersionBefore:package::document_version(&before_doc)?,libraryVersionAfter:4,packageVersion:package::layout_version(&before_doc)?,
        fontStateDigest:crate::font_transaction::state_digest(&before_doc)?,deploymentIntent:doc["applied"]["deploymentIntent"].clone(),desiredSignature:doc["applied"]["desiredSignature"].clone(),
        document:doc,beforeDocument:before_doc,workspaceExisted:owned,temp:format!(".native-gameinfo-{}",paths::random_id()?),generation:generation,
        stagedNodes:if generation.is_some(){JsonValue::new_array()}else{JsonValue::Null},tempIdentity:JsonValue::Null};
    admit_transition(store,id,&j)?;
    store::admit_success(id,input,&reserved_game_document(&j)?)?;
    store::admit_outcome(id,input,&store::failure_outcome(&game_rollback_error()))?;
    // Recheck the complete namespace and all preexisting ownership before even
    // creating the central journal. No projected identities are persisted.
    namespace(store,c,&[&j["beforeDocument"],&j["document"]],generation)?;
    retired_plan_mode(store,c,&j["beforeDocument"],false, authority)?;
    check_checkpoint(c,store.root(),&j["beforeDocument"]["previous"]["applied"],true, authority)?;
    reserve_retirement(store,c,&j, authority)?;
    if text(&c.join("gameinfo.gi"))?!=before {return Err(external());}
    store.write_journal(lock,id,&j)?;
    if store.journal(id)?.as_ref()!=Some(&j) {return Err(recovery());}
    Ok(j)
}
fn persist(store:&Store,lock:&RootLock,id:&str,j:&JsonValue) -> Result<(),NativeError> {
    // Re-admission also ensures the mutable state remains a legal exact prefix.
    admit_transition(store,id,j)?;
    store.write_journal(lock,id,j)
}
// Fault injection only: tests stop the current executor at durable boundaries.
#[cfg(test)]
thread_local! {pub(super) static TEST_CHECKPOINT:std::cell::RefCell<Option<Box<dyn FnMut(&str)->Result<(),NativeError>>>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
fn test_checkpoint(phase:&str)->Result<(),NativeError>{TEST_CHECKPOINT.with(|h|match h.borrow_mut().as_mut(){Some(h)=>h(phase),None=>Ok(())})}
#[cfg(test)]
thread_local! {pub(super) static COPIED_BYTES:std::cell::Cell<u64>=const {std::cell::Cell::new(0)};}
#[cfg(test)]
thread_local! {pub(super) static LINK_BOUNDARY:std::cell::RefCell<Option<Box<dyn FnMut(&str)>>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
thread_local! {pub(super) static FORCE_LINK_UNAVAILABLE:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
fn hard_link(source:&Path,target:&Path)->std::io::Result<()> {
    #[cfg(test)] if FORCE_LINK_UNAVAILABLE.with(|v|v.get()) {return Err(std::io::Error::from_raw_os_error(18));}
    fs::hard_link(source,target)
}
#[derive(Clone)]
struct ReuseFile {payload:package::FileRef,path:PathBuf,identity:JsonValue,root:PathBuf,root_identity:JsonValue,root_children:HashSet<String>,parent:PathBuf,parent_identity:JsonValue,parent_children:HashSet<String>}
fn reusable_files(c:&Path,before:&JsonValue, authority: Option<&GameLock>)->Result<Vec<ReuseFile>,NativeError> {
    let applied=&before["applied"];
    if applied.is_null() || applied["manifest"]["version"]!=3 {return Ok(Vec::new());}
    let reconciled=checkpoint_view(c,applied,authority)?.0;let applied=&reconciled;
    let view=generation::decode(&applied["manifest"])?;let root=generation_dir(c,applied)?;
    let root_children:HashSet<String>=view.directories[0].children.iter().cloned().collect();
    let mut result=Vec::new();
    for file in view.files {
        // Writable historical checkpoints must be copied into immutable generations.
        if paths::open_regular(&root.join(&file.name))?.metadata()?.mode() & 0o7222 != 0 {continue;}
        let rel=Path::new(&file.name).parent().and_then(Path::to_str).ok_or_else(recovery)?;
        let parent=view.directories.iter().find(|d|d.relative_path==rel).ok_or_else(recovery)?;
        let parent_identity=if rel.is_empty(){applied["directoryIdentity"].clone()}else{applied["shardIdentities"][rel].clone()};
        result.push(ReuseFile{payload:file.payload,path:root.join(&file.name),identity:applied["identities"][&file.name].clone(),root:root.clone(),root_identity:applied["directoryIdentity"].clone(),root_children:root_children.clone(),parent:root.join(rel),parent_identity,parent_children:parent.children.iter().cloned().collect()});
    }
    Ok(result)
}
fn recheck_reuse(source:&ReuseFile)->Result<(),NativeError> {
    if identity(&ordinary_directory(&source.root)?)!=source.root_identity || identity(&ordinary_directory(&source.parent)?)!=source.parent_identity
        || identity(&paths::open_regular(&source.path)?.metadata()?)!=source.identity {return Err(external());}
    membership(&source.root,&source.root_children)?;membership(&source.parent,&source.parent_children)?;
    Ok(())
}
fn recheck_staging_parent(dir:&Path,node:&Node,j:&JsonValue)->Result<(),NativeError> {
    let relative=Path::new(&node.name).parent().and_then(Path::to_str).ok_or_else(recovery)?;
    let owner=j["stagedNodes"].members().find(|row|row["kind"]=="directory" && row["name"]==relative).ok_or_else(recovery)?;
    let parent=dir.join(relative);
    if !generation::same_owner(&ordinary_directory(&parent)?,owner) {return Err(external());}
    let children=j["stagedNodes"].members().filter_map(|row|{
        let name=row["name"].as_str()?;if name.is_empty() || name==node.name {return None;}
        let path=Path::new(name);
        if path.parent().and_then(Path::to_str)==Some(relative) {path.file_name().and_then(|v|v.to_str()).map(str::to_owned)}else{None}
    }).collect();
    membership(&parent,&children)
}
#[cfg(test)]
thread_local! {static TEST_COPY_ERROR:std::cell::Cell<Option<i32>>=const {std::cell::Cell::new(None)};}
fn stage(store:&Store,lock:&RootLock,id:&str,c:&Path,j:&mut JsonValue,
    progress:&mut dyn FnMut(crate::launch_preparation::PreparationProgress), authority: Option<&GameLock>) -> Result<(),NativeError> {
    let _phase=paths::ApplyPhase::new("stage");
    #[cfg(test)] test_checkpoint("stageStart")?;
    establish_workspace(c,store.root(),j)?;
    let dir=generation_dir(c,&j["document"]["applied"])?;
    let allowed=nodes(&j["document"]["applied"])?;
    let files_total = allowed.iter().filter(|n| n.payload.is_some()).count() as u64;
    let bytes_total = allowed.iter().filter_map(|n| n.payload.as_ref()).try_fold(0u64, |n,p| n.checked_add(p.size).ok_or_else(recovery))?;
    let (mut files_done, mut bytes_done) = (0u64, 0u64);
    let reusable=reusable_files(c,&j["beforeDocument"], authority)?;
    let mut used=HashSet::new();
    if files_total != 0 {
        progress(crate::launch_preparation::PreparationProgress::Copying { files_done, files_total, bytes_done, bytes_total });
    }
    for node in &allowed {
        let path=dir.join(&node.name);
        paths::verify_absent(&path)?;
        if node.directory {
            fs::create_dir(&path)?;
            #[cfg(test)] test_checkpoint("directoryCreatedUnrecorded")?;
            let owner=generation::capture_identity(&path)?;
            paths::sync_dir(&path)?; paths::sync_dir(path.parent().ok_or_else(recovery)?)?;
            j["stagedNodes"].push(node_record(node,&owner,true)).map_err(|_|recovery())?;
            // A mkdir whose ownership did not reach this durable write is not
            // adopted after a crash, even if its directory is completely empty.
            persist(store,lock,id,j)?;
            #[cfg(test)] if allowed.iter().filter(|n|n.directory).last().is_some_and(|n|n.name==node.name){test_checkpoint("generationCreated")?;}
            continue;
        }
        if let Some(payload)=&node.payload {
            let target_dev=ordinary_directory(path.parent().ok_or_else(recovery)?)?.dev();
            if let Some(source)=reusable.iter().find(|old| old.payload.sha256==payload.sha256 && old.payload.size==payload.size
                && old.identity["device"].as_u64()==Some(target_dev)
                && !used.contains(&(old.identity["device"].as_u64(),old.identity["inode"].as_u64()))) {
                recheck_reuse(source)?;
                let owner=object!{device:source.identity["device"].clone(),inode:source.identity["inode"].clone()};
                // Reserve the exact inode before linking, so every crash point
                // can remove only the link this transaction owns.
                j["stagedNodes"].push(node_record(node,&owner,false)).map_err(|_|recovery())?;
                persist(store,lock,id,j)?;
                #[cfg(test)] LINK_BOUNDARY.with(|h|if let Some(h)=h.borrow_mut().as_mut(){h("reserved");});
                // Another program can still change an inode after these cheap
                // checks; this is the same external-writer race as publication.
                recheck_reuse(source)?;recheck_staging_parent(&dir,node,j)?;paths::verify_absent(&path)?;
                match hard_link(&source.path,&path) {
                    Ok(())=>{
                        paths::sync_dir(path.parent().ok_or_else(recovery)?)?;
                        #[cfg(test)] LINK_BOUNDARY.with(|h|if let Some(h)=h.borrow_mut().as_mut(){h("linked");});
                        if identity(&paths::open_regular(&path)?.metadata()?)!=source.identity {return Err(external());}
                        // This is the same already verified game inode. A link
                        // changes ctime only; new copied content still hashes below.
                        paths::acknowledge_link_change(source.identity["device"].as_u64().ok_or_else(recovery)?,source.identity["inode"].as_u64().ok_or_else(recovery)?)?;
                        generation::verify_file(&path,payload)?;
                        let source_file=paths::open_regular(&source.path)?;
                        paths::seed_hash(&source.path,&paths::content_stamp(&source_file)?,&payload.sha256)?;
                        let last=j["stagedNodes"].len()-1;j["stagedNodes"][last]["complete"]=true.into();
                        persist(store,lock,id,j)?;
                        #[cfg(test)] LINK_BOUNDARY.with(|h|if let Some(h)=h.borrow_mut().as_mut(){h("complete");});
                        used.insert((source.identity["device"].as_u64(),source.identity["inode"].as_u64()));
                        files_done+=1;bytes_done+=payload.size;
                        progress(crate::launch_preparation::PreparationProgress::Copying{files_done,files_total,bytes_done,bytes_total});
                        continue;
                    }
                    Err(e) if matches!(e.raw_os_error(),Some(1|13|18|31|38|95))=>{
                        paths::verify_absent(&path)?;j["stagedNodes"].pop();persist(store,lock,id,j)?;
                    }
                    Err(e)=>return Err(e.into()),
                }
            }
        }
        let mut out=new_file(&path)?;
        let owner=generation::capture_identity(&path)?;
        out.sync_all()?; paths::sync_dir(path.parent().ok_or_else(recovery)?)?;
        j["stagedNodes"].push(node_record(node,&owner,false)).map_err(|_|recovery())?;
        persist(store,lock,id,j)?;
        #[cfg(test)] if node.payload.is_none(){test_checkpoint("manifestCreated")?;}
        if let Some(payload)=&node.payload {
            let mut source=paths::open_regular(&crate::repairs::source(store,&j["document"]["applied"]["manifest"],payload)?)?;
            #[cfg(test)] if let Some(code)=TEST_COPY_ERROR.with(|v|v.take()) {
                let mut first=[0];source.read_exact(&mut first)?;out.write_all(&first)?;
                return Err(if code==13{crate::sqlite::test_full_error()}else{std::io::Error::from_raw_os_error(code).into()});
            }
            // File-to-file copy can use the kernel's copy path while retaining
            // the already-created journal-owned destination descriptor.
            let mut bounded = (&mut source).take(payload.size.saturating_add(1));
            let mut copied=0u64;let mut buffer=[0u8;65536];
            loop {let n=bounded.read(&mut buffer)?;if n==0{break;}out.write_all(&buffer[..n])?;copied+=n as u64;
                #[cfg(test)] test_checkpoint("copyChunk")?;
                #[cfg(test)] COPIED_BYTES.with(|total|total.set(total.get()+n as u64));
                if copied<payload.size {progress(crate::launch_preparation::PreparationProgress::Copying{files_done,files_total,bytes_done:bytes_done+copied,bytes_total});}
            }
            if copied != payload.size {return Err(external());}
            out.sync_all()?;
            paths::make_readonly(&path)?;
            generation::verify_file(&path,payload)?;
        } else {
            out.write_all(j["document"]["applied"]["manifest"].dump().as_bytes())?;
            out.sync_all()?;
        }
        if !generation::same_owner(&paths::open_regular(&path)?.metadata()?,&owner) {return Err(external());}
        if node.payload.is_none() {paths::make_readonly(&path)?;} out.sync_all()?;
        let last=j["stagedNodes"].len()-1; j["stagedNodes"][last]["complete"]=true.into();
        persist(store,lock,id,j)?;
        if let Some(payload) = &node.payload {
            files_done += 1;
            bytes_done += payload.size;
            #[cfg(test)] test_checkpoint("stageFile")?;
            progress(crate::launch_preparation::PreparationProgress::Copying { files_done, files_total, bytes_done, bytes_total });
        }
    }
    for node in allowed.iter().filter(|n|n.directory).rev() {paths::sync_dir(&dir.join(&node.name))?;}
    paths::sync_dir(dir.parent().ok_or_else(recovery)?)?;
    let staged=j["stagedNodes"].clone();
    capture(c,&mut j["document"]["applied"],&staged)?;
    check_checkpoint(c,store.root(),&j["document"]["applied"],false, authority)?;
    persist(store,lock,id,j)?;
    #[cfg(test)] test_checkpoint("staged")?;
    Ok(())
}
pub(super) fn apply(plan:&ApplyPlan,store:&Store,lock:&RootLock,expected:u64,id:&str,input:&JsonValue) -> Result<JsonValue,NativeError> {
    if let Some(result)=store.replay(id,input)? {return Ok(result);}
    if plan.root!=store.root() || plan.revision!=expected {return Err(err("stalePreview","Apply review does not match this library revision."));}
    ensure_idle(store)?;
    let c=citadel(&plan.game)?; let _game_lock=GameLock::acquire(&c)?;
    apply_locked(plan, store, lock, &_game_lock, expected, id, input,
        &crate::prepare_control::PrepareControl::never_cancelled(), &mut |_| {})
}
pub(super) fn apply_locked(plan:&ApplyPlan,store:&Store,lock:&RootLock,game_lock:&GameLock,expected:u64,id:&str,input:&JsonValue,
    control:&crate::prepare_control::PrepareControl, progress:&mut dyn FnMut(crate::launch_preparation::PreparationProgress)) -> Result<JsonValue,NativeError> {
    if let Some(result)=store.replay(id,input)? {return Ok(result);}
    store.locked(lock)?;
    if plan.root!=store.root() || plan.revision!=expected {return Err(err("stalePreview","Apply review does not match this library revision."));}
    ensure_idle(store)?;
    let c=citadel(&plan.game)?;
    game_lock.validate(&c)?;
    control.check()?;
    progress(crate::launch_preparation::PreparationProgress::Verifying);
    plan.recheck(store)?;
    let mut doc=store.document()?;
    verify_fonts(store,&doc,lock,Some(game_lock))?; check_base(store.root(),&doc, Some(game_lock))?;
    check_checkpoint(&c,store.root(),&doc["previous"]["applied"],false, Some(game_lock))?;
    let owned=workspace(&c,store.root())?;
    paths::verify_absent(&c.join("deadlock_native").join(&plan.generation))?;
    let applied=object!{deploymentIntent:plan.intent.encode(),desiredSignature:plan.intent.signature()?,profileId:plan.profile.clone(),enabled:plan.enabled.clone(),
        generation:plan.generation.clone(),manifest:plan.manifest(store),gameinfo:plan.after.clone(),gameinfoSha256:paths::digest_bytes(plan.after.as_bytes())?};
    rebase_document(&mut doc,&plan.before)?;
    let old=doc["previous"]["applied"].clone(); append_retired(&mut doc,old)?;
    let mut previous=object!{applied:doc["applied"].clone(),activeProfileId:doc["activeProfileId"].clone(),gameinfo:plan.before.clone(),gameinfoSha256:paths::digest_bytes(plan.before.as_bytes())?,upstreamPayloads:plan.upstream_payloads.clone(),upstreamEvidenceVersion:plan.version};
    upgrade(&mut doc)?;
    capture_checkpoint_baselines(&mut previous,&c)?;
    if doc.has_key("gameFolderChanged") {doc["gameFolderChanged"]=false.into();}
    doc["applied"]=applied; doc["previous"]=previous; doc["restoredSetup"]=JsonValue::Null;
    doc["canRestore"]=true.into(); doc["activeProfileId"]=plan.profile.clone().into(); doc["recoveryRequired"]=false.into();
    game_lock.validate(&c)?;
    control.check()?; // Last cancellation boundary: the journal/writer now owns completion.
    let mut j=prepare_transition(store,lock,id,input,expected,&c,&plan.before,&plan.after,doc,owned,Some(&plan.generation),plan.vanilla, Some(game_lock))?;
    #[cfg(test)] test_checkpoint("journal").map_err(|e|recovery_with_cause("journal",e))?;
    (|| {stage(store,lock,id,&c,&mut j,progress, Some(game_lock))?; game_lock.validate(&c)?; publish(store,lock,id,&j,Some(plan),progress, Some(game_lock))})().map_err(|e| if e.message == game_files_changed().message { e } else { recovery_with_cause("Apply",e) })
}
pub(super) fn restore(store:&Store,lock:&RootLock,expected:u64,id:&str,input:&JsonValue) -> Result<JsonValue,NativeError> {
    if let Some(result)=store.replay(id,input)? {return Ok(result);}
    ensure_idle(store)?;
    let mut doc=store.document()?;
    if rev(&doc)?!=expected {return Err(err("staleRevision","Library changed before Restore."));}
    if doc["canRestore"]!=true || !doc["previous"].is_object() {return Err(err("nothingToRestore","There is no verified previous setup to restore."));}
    let c=citadel(Path::new(string(&doc,"gamePath")?))?; let _game_lock=GameLock::acquire(&c)?;
    game_guard()?; verify_fonts(store,&doc,lock,Some(&_game_lock))?; check_applied(store.root(),&doc, Some(&_game_lock))?;
    check_checkpoint(&c,store.root(),&doc["previous"]["applied"],true, Some(&_game_lock))?; check_installed_baselines(&doc["previous"], &c, true, Some(&_game_lock))?; check_upstream(&doc["previous"], true, &c, Some(&_game_lock))?;
    let before=text(&c.join("gameinfo.gi"))?; let after=string(&doc["previous"],"gameinfo")?.to_owned();
    if doc["previous"]["gameinfoSha256"]!=paths::digest_bytes(after.as_bytes())? {return Err(external());}
    let old=doc["applied"].clone(); append_retired(&mut doc,old)?;
    doc["restoredSetup"]=if doc["previous"].is_object(){doc["previous"].clone()}else{JsonValue::Null};
    doc["applied"]=doc["previous"]["applied"].clone(); doc["activeProfileId"]=doc["previous"]["activeProfileId"].clone();
    doc["previous"]=JsonValue::Null; doc["canRestore"]=false.into(); doc["recoveryRequired"]=false.into();
    let j=prepare_transition(store,lock,id,input,expected,&c,&before,&after,doc,true,None,false, Some(&_game_lock))?;
    #[cfg(test)] test_checkpoint("journal").map_err(|e|recovery_with_cause("journal",e))?;
    publish(store,lock,id,&j,None,&mut |_| {}, Some(&_game_lock)).map_err(|e|recovery_with_cause("Restore",e))
}
fn verify_before_live(c:&Path,j:&JsonValue,authority:Option<&GameLock>) -> Result<(),NativeError> {
    let path=c.join("gameinfo.gi");
    if !generation::metadata_matches(&path,&j["beforeIdentity"],authority)? || text(&path)?!=string(j,"before")? {return Err(external());}
    Ok(())
}
#[cfg(test)]
thread_local! {pub(super) static BEFORE_PUBLISH:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
thread_local! {pub(super) static REVIEW7_AFTER_DB:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
fn publish(store:&Store,lock:&RootLock,id:&str,j:&JsonValue,plan:Option<&ApplyPlan>,
    progress:&mut dyn FnMut(crate::launch_preparation::PreparationProgress), authority: Option<&GameLock>) -> Result<JsonValue,NativeError> {
    let _phase=paths::ApplyPhase::new("publish-or-finish");
    admit_transition(store,id,j)?;
    crate::font_transaction::admit_set(store,&store.journals()?)?;
    let c=PathBuf::from(string(j,"citadel")?); let live=c.join("gameinfo.gi");
    #[cfg(test)] test_checkpoint("beforeTemp")?;
    verify_before_live(&c,j, authority)?;
    if j["input"]["method"]=="native.restore" {check_snapshot(store.root(),&j["beforeDocument"], authority)?;} else {check_base(store.root(),&j["beforeDocument"], authority)?;}
    check_checkpoint(&c,store.root(),&j["beforeDocument"]["previous"]["applied"],true, authority)?;
    check_checkpoint(&c,store.root(),&j["document"]["applied"],true, authority)?;
    check_checkpoint(&c,store.root(),&j["document"]["previous"]["applied"],true, authority)?;
    check_upstream(&j["document"]["restoredSetup"], true, &c, authority)?; check_upstream(&j["document"]["previous"], true, &c, authority)?;
    namespace(store,&c,&[&j["beforeDocument"],&j["document"]],j["generation"].as_str())?;
    retired_plan_mode(store,&c,&j["document"],false, authority)?;
    game_guard()?; verify_fonts(store,&store.document()?,lock,authority)?;
    progress(crate::launch_preparation::PreparationProgress::Committing);
    let mut journal=j.clone(); let temp=c.join(string(j,"temp")?);
    let mut out=new_file(&temp)?; let temp_birth=generation::capture_identity(&temp)?;
    out.sync_all()?; paths::sync_dir(&c)?;
    journal["tempIdentity"]=temp_birth;
    persist(store,lock,id,&journal)?;
    out.write_all(string(j,"after")?.as_bytes())?; out.sync_all()?;
    fs::set_permissions(&temp,paths::open_regular(&live)?.metadata()?.permissions())?; out.sync_all()?;
    if !generation::same_owner(&paths::open_regular(&temp)?.metadata()?,&journal["tempIdentity"]) || text(&temp)?!=string(j,"after")? {return Err(external());}
    if let Some(plan)=plan {plan.recheck(store)?;}
    verify_before_live(&c,j, authority)?; if j["input"]["method"]=="native.restore" {check_snapshot(store.root(),&store.document()?, authority)?;} else {check_base(store.root(),&store.document()?, authority)?;}
    check_checkpoint(&c,store.root(),&j["beforeDocument"]["previous"]["applied"],true, authority)?;
    namespace(store,&c,&[&j["beforeDocument"],&j["document"]],j["generation"].as_str())?;
    #[cfg(test)]
    BEFORE_PUBLISH.with(|h|if let Some(h)=h.borrow_mut().take(){h();});
    #[cfg(test)] test_checkpoint("beforeRename")?;
    check_checkpoint(&c,store.root(),&journal["document"]["applied"],false, authority)?;
    check_installed_baselines(&journal["document"]["restoredSetup"], &c, false, authority)?;
    check_upstream(&journal["document"]["restoredSetup"], false, &c, authority)?;
    check_upstream(&journal["document"]["previous"], false, &c, authority)?;
    game_guard()?; verify_fonts(store,&store.document()?,lock,authority)?;
    // Full retired content must verify before publishing gameinfo. The final
    // per-file hash immediately before unlink remains a separate check.
    retired_plan_mode(store,&c,&journal["document"],true, authority)?;
    if let Err(e)=check_installed_baselines(&journal["document"]["previous"], &c, true, authority) {
        // Still before publication: remove only journal-owned staging, preserving
        // every foreign addon file and the unchanged before image.
        game_guard()?;
        rollback_live(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(e);
    }
    verify_before_live(&c,&journal, authority)?;
    #[cfg(test)]
    BEFORE_FINAL_RENAME.with(|h|if let Some(h)=h.borrow_mut().take(){h(&temp);});
    if plan.is_some_and(|plan| plan.recheck_takeover(&c).is_err()) {
        game_guard()?;
        rollback_plan(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(game_files_changed());
    }
    if !generation::same_owner(&paths::open_regular(&temp)?.metadata()?,&journal["tempIdentity"]) || text(&temp)?!=string(j,"after")? {return Err(external());}
    check_checkpoint(&c,store.root(),&journal["document"]["applied"],true, authority)?;
    check_checkpoint(&c,store.root(),&journal["document"]["previous"]["applied"],true, authority)?;
    check_installed_baselines(&journal["document"]["restoredSetup"], &c, true, authority)?;
    check_installed_baselines(&journal["document"]["previous"], &c, true, authority)?;
    check_upstream(&journal["document"]["restoredSetup"], true, &c, authority)?;
    check_upstream(&journal["document"]["previous"], true, &c, authority)?;
    if plan.is_some_and(|plan| plan.recheck_takeover(&c).is_err()) || verify_before_live(&c,&journal, authority).is_err() {
        game_guard()?;
        rollback_plan(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(game_files_changed());
    }
    paths::replace_admitted(&temp,&live,&journal["beforeIdentity"],journal["beforeSha256"].as_str().map(str::to_owned))?; paths::sync_dir(&c)?;
    #[cfg(test)] test_checkpoint("renamed")?;
    finish5(store,lock,id,&journal, authority)
}
pub(super) fn forward_preflight(store:&Store,c:&Path,j:&JsonValue, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    forward_preflight_mode(store,c,j,false, authority)
}
fn forward_preflight_mode(store:&Store,c:&Path,j:&JsonValue,fast:bool, authority: Option<&GameLock>)->Result<Cleanup,NativeError>{
    let live=c.join("gameinfo.gi");
    if j["tempIdentity"].is_null() || !generation::owner_matches(&live,&j["tempIdentity"],authority)?
        || text(&live)?!=string(j,"after")? || path_exists(&c.join(string(j,"temp")?))? {return Err(external());}
    deployment_metadata::admit(store.root(),&j["document"])?;
    namespace(store,c,&[&j["beforeDocument"],&j["document"]],j["generation"].as_str())?;
    check_state(store.root(),&j["document"],fast,fast, authority)?;
    check_checkpoint(c,store.root(),&j["document"]["previous"]["applied"],false, authority)?;
    if store.operation(string(&j["input"]["params"],"operationId")?)?["status"]!="succeeded" {check_rollback_upstream(&j["document"]["previous"],c, authority)?;}
    // The newly retired owner cannot legitimately have cleanup holes until
    // the main database transaction succeeded. Older retired records can.
    if store.operation(string(&j["input"]["params"],"operationId")?)?["status"]!="succeeded" {
        let retired=if matches!(j["input"]["method"].as_str(),Some("native.apply"|"native.vanilla")) {&j["beforeDocument"]["previous"]["applied"]} else {&j["beforeDocument"]["applied"]};
        check_checkpoint(c,store.root(),retired,fast, authority)?;
    }
    retired_plan(store,c,&j["document"], authority)
}
pub(super) fn finish5(store:&Store,lock:&RootLock,id:&str,j:&JsonValue, authority: Option<&GameLock>) -> Result<JsonValue,NativeError> {
    let _phase=paths::ApplyPhase::new("publish-or-finish");
    admit_transition(store,id,j)?;
    crate::font_transaction::admit_set(store,&store.journals()?)?;
    let c=PathBuf::from(string(j,"citadel")?);
    verify_fonts(store,&store.document()?,lock,authority)?;
    let cleanup=forward_preflight_mode(store,&c,j,true, authority)?;
    reserve_retirement(store,&c,j, authority)?;
    game_guard()?;
    #[cfg(test)] test_checkpoint("beforeDatabase")?;
    let result=store.commit(lock,store::revision(string(j,"expectedRevision")?)?,id,&j["input"],j["document"].clone(),None)?;
    #[cfg(test)] test_checkpoint("database")?;
    #[cfg(test)]
    REVIEW7_AFTER_DB.with(|h|if let Some(h)=h.borrow_mut().take(){h();});
    // Keep the successful main journal until retirement is complete. Its exact
    // captured nodes authorize a safe resume with known missing cleanup nodes.
    if !cleanup.execute()? {return Err(NativeError::new("recoveryRequired","Older files were left in place because their content could not be verified. Retirement cannot finish safely; preserve those files before retrying."));}
    let remaining=forward_preflight_mode(store,&c,j,true, authority)?;
    if !remaining.remove.is_empty() {return Err(recovery());}
    let mut committed=j["document"].clone();
    committed["revision"]=store::revision(string(j,"expectedRevision")?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
    let pruned=retirement_document(&c,&committed,&remaining)?;
    store.remember_deployment_hashes(&pruned)?;
    if j["version"]==6 { store.complete_game6_retirement(lock,id,j,&pruned)?; }
    else { store.complete_game5_retirement(lock,id,j,&pruned)?; }
    Ok(result)
}
pub(super) fn recovery_claims(c:&Path,j:&JsonValue) -> Result<Vec<PathBuf>,NativeError> {
    let mut claims=vec![c.join("gameinfo.gi"),c.join(string(j,"temp")?)];
    let mut seen=HashSet::new();
    for a in checkpoints(&j["document"]).chain(checkpoints(&j["beforeDocument"])).filter(|a|!a.is_null()) {
        if !seen.insert(string(a,"generation")?.to_owned()) {continue;}
        let view=generation::decode(&a["manifest"])?; let dir=generation_dir(c,a)?;
        claims.extend(view.directories.iter().map(|d|dir.join(&d.relative_path)));
        claims.extend(view.files.iter().map(|f|dir.join(&f.name)));
        claims.push(dir.join("manifest.json"));
    }
    Ok(claims)
}
pub(super) fn rollback_live(store:&Store,c:&Path,j:&JsonValue, authority: Option<&GameLock>) -> Result<Cleanup,NativeError> {
    verify_before_live(c,j, authority)?;
    check_base(store.root(),&j["beforeDocument"], authority)?;
    check_rollback_upstream(&j["document"]["previous"],c, authority)?;
    rollback_plan(store,c,j, authority)
}

pub(super) fn maintenance_rows(store: &Store) -> Result<JsonValue,NativeError> {
    let doc=store.document()?;
    let Some(game)=doc["gamePath"].as_str().filter(|s|!s.is_empty()) else {return Ok(JsonValue::new_array())};
    let c=Path::new(game).join("game/citadel");
    namespace(store,&c,&[&doc],None)?;
    maintenance_mount_guard(store,&c,&doc, None)?;
    let plan=retired_plan(store,&c,&doc, None)?;
    Ok(plan.remove.iter().map(|r| object!{path:r.path.to_string_lossy().as_ref(),bytes:if r.directory {0}else{fs::symlink_metadata(&r.path).map(|m|m.len()).unwrap_or(0)},kind:"retired",removable:true}).collect::<Vec<_>>().into())
}
pub(super) fn maintenance_retire(store:&Store,doc:&JsonValue)->Result<JsonValue,NativeError>{
    let Some(game)=doc["gamePath"].as_str().filter(|s|!s.is_empty()) else {return Ok(doc.clone())};
    let c=Path::new(game).join("game/citadel");
    namespace(store,&c,&[doc],None)?;
    if !workspace(&c,store.root())? {return Err(external());}
    maintenance_mount_guard(store,&c,doc, None)?;
    let plan=retired_plan(store,&c,doc, None)?;
    game_guard()?;
    plan.execute()?;
    let remaining=retired_plan(store,&c,doc, None)?;
    if !remaining.remove.is_empty(){return Err(recovery());}
    retirement_document(&c,doc,&remaining)
}

fn maintenance_mount_guard(store:&Store,c:&Path,doc:&JsonValue, authority: Option<&GameLock>)->Result<(),NativeError>{
    if check_external(store.root(),doc, authority)?{return Err(external_gameinfo_error());}
    let current=text(&c.join("gameinfo.gi"))?;
    let tokens=gameinfo::mounts(&current)?;
    for a in doc["retiredGenerations"].members(){for mount in mounts(a)?{
        if tokens.iter().any(|t|t.eq_ignore_ascii_case(&mount)){return Err(external());}
    }}
    Ok(())
}

#[cfg(test)]
mod review_tests {
 use super::*;
 use crate::test_support::{Fixture,import};
 fn diagnostics_setup()->(Fixture,RootLock,Store){
    let f=Fixture::new();fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();import(&mut crate::native::NativeService::new(),&f);
    let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();(f,lock,st)
 }
 // A legal current Apply transition supplied to the real preparation routine.
 // No projected identity from this utility is persisted as ownership.
 fn budget_document(st:&Store,plan:&ApplyPlan,c:&Path)->JsonValue {
    let mut doc=st.document().unwrap();
    rebase_document(&mut doc,&plan.before).unwrap();let old=doc["previous"]["applied"].clone();append_retired(&mut doc,old).unwrap();
    let mut previous=object!{applied:doc["applied"].clone(),activeProfileId:doc["activeProfileId"].clone(),gameinfo:plan.before.clone(),gameinfoSha256:paths::digest_bytes(plan.before.as_bytes()).unwrap(),upstreamPayloads:plan.upstream_payloads.clone(),upstreamEvidenceVersion:plan.version};
    capture_checkpoint_baselines(&mut previous,c).unwrap();upgrade(&mut doc).unwrap();
    doc["applied"]=object!{deploymentIntent:plan.intent.encode(),desiredSignature:plan.intent.signature().unwrap(),profileId:plan.profile.clone(),enabled:plan.enabled.clone(),generation:plan.generation.clone(),manifest:plan.manifest(st),gameinfo:plan.after.clone(),gameinfoSha256:paths::digest_bytes(plan.after.as_bytes()).unwrap()};
    doc["previous"]=previous;doc["restoredSetup"]=JsonValue::Null;doc["canRestore"]=true.into();doc["activeProfileId"]=plan.profile.clone().into();doc["recoveryRequired"]=false.into();
    doc
 }
 fn budget_game_evidence(root:&Path)->std::collections::BTreeMap<PathBuf,(JsonValue,Option<String>)> {
    fn visit(root:&Path,p:&Path,out:&mut std::collections::BTreeMap<PathBuf,(JsonValue,Option<String>)>){
        let metadata=fs::symlink_metadata(p).unwrap();
        out.insert(p.strip_prefix(root).unwrap().into(),(generation::capture_identity(p).unwrap(),if metadata.is_file(){Some(paths::fingerprint(p).unwrap().sha256)}else{None}));
        if metadata.is_dir(){for e in fs::read_dir(p).unwrap(){visit(root,&e.unwrap().path(),out);}}
    }
    let mut out=Default::default();visit(root,root,&mut out);out
 }
 fn budget_case(case:&str){
        let(f,lock,st)=diagnostics_setup();let c=citadel(&f.game).unwrap();
        if case=="escaping" {
            // A comment is legal KeyValues, stays under the 4 MiB gameinfo cap,
            // and its backslashes must each count as TWO JSON bytes.
            let mut gi=text(&c.join("gameinfo.gi")).unwrap();gi.push_str("//");gi.push_str(&"\\".repeat(1024*1024));gi.push('\n');
            assert!(gi.len()<MAX_GAMEINFO as usize);gameinfo::mounts(&gi).unwrap();fs::write(c.join("gameinfo.gi"),gi).unwrap();
        }
        let mut initial=st.document().unwrap();initial["importArchive"]["groups"][0]["notes"]="".into();
        st.commit(&lock,1,"metadata-empty",&object!{},initial,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=crate::test_support::game_input("native.apply",2,"sample",&object!{});
        let guard=GameLock::acquire(&c).unwrap();
        // Calibration uses a genuine admitted/journaled transition, then the
        // real rollback path. It is not used as evidence of pre-write refusal.
        let sample=prepare_transition(&st,&lock,"sample",&input,2,&c,&plan.before,&plan.after,budget_document(&st,&plan,&c),false,Some(&plan.generation),false,Some(&guard)).unwrap();
        let base_bytes=sample.dump().len();
        assert_eq!(sample["version"],5);assert!(sample["stagedNodes"].is_empty());assert!(sample["tempIdentity"].is_null());
        drop(guard);
        recover(&st,&lock,2,"rewind",&crate::test_support::game_input("native.recover",2,"rewind",&object!{})).unwrap();
        assert!(st.journals().unwrap().is_empty());assert_eq!(st.operation("sample").unwrap()["status"],"failed");
        let limit=paths::MAX_JSON_BYTES as usize;
        // The notes occur exactly twice, once in each complete document.
        // Pick the late case from the UNPROJECTED size, independently of the
        // reservation implementation that this guard is meant to test.
        let desired=match case {"fits"=>limit-16*1024,"late-identities"=>limit-512,_=>limit+512};
        let notes=(desired-base_bytes)/2;
        let mut doc=st.document().unwrap();let expected=rev(&doc).unwrap();
        doc["importArchive"]["groups"][0]["notes"]="x".repeat(notes).into();
        store::ensure_readable(&doc).unwrap();
        st.commit(&lock,expected,"metadata-large",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let before=st.document().unwrap();let expected=rev(&before).unwrap();
        let prospective=budget_document(&st,&plan,&c);
        // Pure shape validation succeeds with the real maximum-width captures;
        // no invented envelope fields or relaxation of exact schema admission.
        let mut projected_doc=prospective.clone();projected_doc["applied"]=deployment_metadata::reserve_shard_checkpoint(&prospective["applied"]).unwrap();
        deployment_metadata::admit(st.root(),&projected_doc).unwrap();store::ensure_readable(&projected_doc).unwrap();
        let input=crate::test_support::game_input("native.apply",expected,"budget",&object!{});
        let mut accounting=sample.clone();accounting["beforeDocument"]=before.clone();accounting["document"]=prospective.clone();accounting["expectedRevision"]=expected.to_string().into();accounting["input"]=input.clone();accounting["generation"]=plan.generation.clone().into();
        let raw_bytes=accounting.dump().len();
        if case=="late-identities" {assert!(raw_bytes<limit && limit-raw_bytes<1024);}
        if case=="too-large" || case=="escaping" {assert!(raw_bytes>limit);}
        if case=="escaping" {
            let slash_count=std::iter::once(&accounting["before"]).chain(std::iter::once(&accounting["after"]))
                .chain(std::iter::once(&accounting["document"]["previous"]["gameinfo"]))
                .chain(std::iter::once(&accounting["document"]["applied"]["gameinfo"]))
                .map(|v|v.as_str().unwrap().bytes().filter(|b|*b==b'\\').count()).sum::<usize>();
            assert!(raw_bytes-slash_count<limit,"ignoring escaping would wrongly fit");
        }
        let tree=budget_game_evidence(&c);let journals=st.journals().unwrap();
        let evidence_db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),false).unwrap();
        let receipts=evidence_db.query("SELECT group_concat(id || ':' || result, char(10)) FROM operations",&[]).unwrap();
        let guard=GameLock::acquire(&c).unwrap();
        let result=prepare_transition(&st,&lock,"budget",&input,expected,&c,&plan.before,&plan.after,prospective,false,Some(&plan.generation),false,Some(&guard));
        assert_eq!(budget_game_evidence(&c),tree,"{case}: preparation must leave all game bytes/identities unchanged");
        assert_eq!(st.document().unwrap(),before);assert_eq!(evidence_db.query("SELECT group_concat(id || ':' || result, char(10)) FROM operations",&[]).unwrap(),receipts);
        if case=="fits" {
            let j=result.unwrap();assert_eq!(st.journal("budget").unwrap(),Some(j.clone()));
            assert_eq!(j.dump().len(),raw_bytes,"accounting fixture must match actual preparation envelope");
            let reserved=reserved(&j).unwrap();let reserved_bytes=reserved.dump().len();
            assert!(reserved_bytes<=limit && limit-reserved_bytes<32*1024);
            assert_eq!(j["stagedNodes"].len(),0);assert!(!j["document"]["applied"].has_key("identities"));
            assert_eq!(st.operation("budget").unwrap()["status"],"recoveryRequired");
        }else{
            let error=result.err().unwrap_or_else(||panic!("{case}: over-budget journal was admitted"));assert_eq!(error.kind,"sourceTooLarge","{case}");
            assert!(error.message.contains("64 MiB internal JSON limit"),"{case}: {error:?}");
            assert!(st.journals().unwrap()==journals,"{case}: pre-write refusal left a journal");assert_eq!(st.operation("budget").unwrap()["status"],"unknown");
        }
        eprintln!("fix1 budget case={case} raw={raw_bytes} limit={limit} revision={expected}");
 }
 #[test]
 fn fix1_current_preparation_fits_near_limit(){budget_case("fits");}
 #[test]
 fn fix1_current_preparation_refuses_too_large(){budget_case("too-large");}
 #[test]
 fn fix1_current_preparation_reserves_late_identities(){budget_case("late-identities");}
 #[test]
 fn fix1_current_preparation_counts_escaping(){budget_case("escaping");}
 #[test]
 fn item115_diagnostics_late_guard_retains_baseline_kind_and_cause(){
    let(f,lock,st)=diagnostics_setup();let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
    let input=object!{method:"native.apply",params:object!{operationId:"failure",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
    let result=apply_locked(&plan,&st,&lock,&guard,1,"failure",&input,&crate::prepare_control::PrepareControl::never_cancelled(),&mut |p|{
        if matches!(p,crate::launch_preparation::PreparationProgress::Copying{files_done,files_total,..} if files_done==files_total){crate::game_access::TEST_REFUSAL.with(|v|v.set(true));}
    });crate::game_access::TEST_REFUSAL.with(|v|v.set(false));
    let e=result.unwrap_err();assert_eq!(e.kind,"recoveryRequired");assert!(e.message.contains("gameRunning"));assert!(st.journal("failure").unwrap().is_some());
 }
 #[test]
 fn item115_diagnostics_copy_failures_keep_baseline_recovery_kind(){
    for code in [28,13] {
        let(f,lock,st)=diagnostics_setup();let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=object!{method:"native.apply",params:object!{operationId:"failure",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        TEST_COPY_ERROR.with(|v|v.set(Some(code)));let result=plan.apply(&st,&lock,1,"failure",&input);TEST_COPY_ERROR.with(|v|v.set(None));
        let e=result.unwrap_err();assert_eq!(e.kind,"recoveryRequired");assert!(e.message.contains("Not enough disk space"));assert!(e.message.contains(if code==13{"nativeDatabase"}else{"nativeIo"}));assert!(st.journal("failure").unwrap().is_some());
    }
 }
 #[test]
 fn round3_rollback_temp_digest_uses_verified_read() {
    let f=Fixture::new();import(&mut crate::native::NativeService::new(),&f);let st=Store::open(&f.data).unwrap();let c=citadel(&f.game).unwrap();let temp=c.join(".native-gameinfo-test");fs::write(&temp,b"prefix").unwrap();
    let j=object!{beforeDocument:st.document().unwrap(),document:st.document().unwrap(),temp:".native-gameinfo-test",tempIdentity:identity(&fs::metadata(&temp).unwrap()),after:"prefix plus",generation:JsonValue::Null};
    paths::HASH_READS.with(|r|r.borrow_mut().clear());let plan=rollback_plan(&st,&c,&j, None).unwrap();
    assert_eq!(plan.remove[0].sha256.as_deref(),Some(paths::digest_bytes(b"prefix").unwrap().as_str()));
    paths::HASH_READS.with(|r|assert!(!r.borrow().contains_key(&temp),"rollback must not fingerprint temp in a second read"));
 }
 #[test]
 fn retired_hash_is_captured_from_verified_read() {
    let f=Fixture::new();fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();import(&mut crate::native::NativeService::new(),&f);let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
    let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
    plan.apply(&st,&lock,1,"first",&input).unwrap();let mut doc=st.document().unwrap();doc["retiredGenerations"]=json::array![doc["applied"].clone()];doc["applied"]=JsonValue::Null;doc["previous"]=JsonValue::Null;
    paths::HASH_READS.with(|r|r.borrow_mut().clear());retired_plan(&st,&citadel(&f.game).unwrap(),&doc, None).unwrap();
    paths::HASH_READS.with(|r|for(p,n)in r.borrow().iter(){assert!(*n<=1,"{} read {} times",p.display(),n);});
 }
 #[test]
 fn repair_copy_prefix_uses_journal_manifest_source() {
    let f=Fixture::new();import(&mut crate::native::NativeService::new(),&f);let st=Store::open(&f.data).unwrap();
    let bytes=b"repair overlay bytes";let sha=paths::digest_bytes(bytes).unwrap();let file=package::FileRef{id:sha[..32].into(),sha256:sha.clone(),size:bytes.len() as u64,source_name:"pak01_dir.vpk".into()};
    let cache=f.data.join("cache/repairs");paths::private_dir(&cache).unwrap();fs::write(cache.join(format!("{sha}.vpk")),bytes).unwrap();
    let partial=f.root.join("partial.vpk");fs::write(&partial,&bytes[..7]).unwrap();
    // The complete source is the repair cache, never payloads/<id>.vpk.
    assert!(verify_payload_prefix(&st,&partial,&file,&object!{repairs:object!{file:package::encode_file(&file)}}).is_ok());
 }
 #[test]
 fn cleanup_rechecks_content_after_preflight() {
    let f=Fixture::new();let path=f.root.join("retired.vpk");fs::write(&path,b"original").unwrap();let m=fs::metadata(&path).unwrap();let owner=identity(&m);
    let cleanup=Cleanup{remove:vec![Removal{mutation:Some(paths::DestructiveFile::admitted(path.clone(),&owner,None).unwrap()),path:path.clone(),directory:false,owner,sha256:Some(paths::fingerprint(&path).unwrap().sha256)}]};
    fs::write(&path,b"modified").unwrap();assert!(cleanup.execute().is_err());assert!(path.exists());
 }
}

#[cfg(test)]
pub(super) fn test_cleanup_parent(store:&Store,c:&Path,a:&JsonValue,dir:&Path,swap:bool){
    let cleanup=partial_tree(store,c,a,&checkpoint_records(a).unwrap(),true, None).unwrap();
    let file=cleanup.remove.iter().find(|r|!r.directory).unwrap().path.clone();let parent=file.parent().unwrap().to_path_buf();let target=parent.clone();
    BEFORE_UNLINK.with(|h|*h.borrow_mut()=Some(Box::new(move||{
        if swap {let held=target.with_extension("held");fs::rename(&target,&held).unwrap();fs::create_dir(&target).unwrap();for e in fs::read_dir(held).unwrap(){let p=e.unwrap().path();if p.is_file(){fs::hard_link(&p,target.join(p.file_name().unwrap())).unwrap();}else{fs::rename(&p,target.join(p.file_name().unwrap())).unwrap();}}}
        else{fs::write(target.join("foreign"),b"keep").unwrap();}
    })));
    assert!(cleanup.execute().is_err());assert!(file.exists());assert!(dir.exists());
}
