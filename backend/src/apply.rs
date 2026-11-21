//! Recoverable game transactions. The store lock and a flock on the existing
//! citadel directory span validation, staging, filesystem commit and DB commit.
use crate::{
    gameinfo,
    generation::{self, generation_files},
    native::NativeError,
    package, paths,
    store::{self, RootLock, Store},
    vpk,
};
use json::{object, JsonValue};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
#[path = "apply_shards.rs"]
mod shards;
const MAX_GAMEINFO: u64 = 4 * 1024 * 1024;
fn err(kind: &str, msg: &str) -> NativeError {
    NativeError::new(kind, msg)
}
fn external() -> NativeError {
    err("externalChange","Game files differ from the verified setup; preserve the backup and reconcile external changes before continuing.")
}
fn recovery_with_cause(phase:&str,cause:NativeError)->NativeError {
    let mut error=recovery();
    error.message.push_str(&format!(" Cause during {phase}: {}: {}",cause.kind,cause.message));
    error
}
fn recovery() -> NativeError {
    err("recoveryRequired","An unfinished game operation needs explicit recovery; its journal and exact backup have been preserved.")
}
fn string<'a>(v: &'a JsonValue, k: &str) -> Result<&'a str, NativeError> {
    v[k].as_str().ok_or_else(NativeError::params)
}
fn rev(d: &JsonValue) -> Result<u64, NativeError> {
    store::revision(string(d, "revision")?)
}
fn text(path: &Path) -> Result<String, NativeError> {
    String::from_utf8(paths::read_bounded(path, MAX_GAMEINFO)?).map_err(|_| {
        err(
            "invalidGameinfo",
            "Gameinfo is not valid UTF-8; no changes were made.",
        )
    })
}
pub(crate) fn citadel(game: &Path) -> Result<PathBuf, NativeError> {
    let game = paths::lexical_absolute(game)?;
    let c = game.join("game/citadel");
    paths::reject_symlinks(&c)?;
    if !c.is_dir() {
        return Err(err(
            "unsafePath",
            "Game directory must be an ordinary directory.",
        ));
    }
    Ok(c)
}
use crate::game_access::{GameLock, ensure_game_idle as game_guard};
#[cfg(test)]
use crate::game_access::game_guard_at;
fn workspace(c: &Path, root: &Path) -> Result<bool, NativeError> {
    let w = c.join("deadlock_native");
    if fs::symlink_metadata(&w).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(false);
    }
    paths::reject_symlinks(&w)?;
    let owner = paths::read_bounded(&w.join("owner.json"), 4096).map_err(|_| {
        err(
            "foreignWorkspace",
            "The native game workspace has no matching ownership record; it cannot be adopted.",
        )
    })?;
    let v = json::parse(std::str::from_utf8(&owner).map_err(|_| external())?)
        .map_err(|_| external())?;
    if v != object! {version:1,root:root.to_string_lossy().as_ref()} {
        return Err(err(
            "foreignWorkspace",
            "This game workspace belongs to another native library.",
        ));
    }
    Ok(true)
}
fn verify_file(path: &Path, p: &JsonValue) -> Result<(), NativeError> {
    let fp = paths::fingerprint(path)?;
    if p["sha256"] != fp.sha256 || p["size"] != fp.size {
        return Err(external());
    }
    Ok(())
}
pub(crate) fn identity(m: &fs::Metadata) -> JsonValue {
    object! {size:m.len(),device:m.dev(),inode:m.ino(),modifiedSeconds:m.mtime(),modifiedNanos:m.mtime_nsec()}
}
fn verify_cached(
    path: &Path,
    p: &JsonValue,
    recorded: &JsonValue,
    fast: bool,
) -> Result<(), NativeError> {
    if fast {return if recorded.is_object() && identity(&paths::open_regular(path)?.metadata()?)==*recorded {Ok(())}else{Err(external())};}
    verify_file(path, p)
}
fn generation_dir(c: &Path, a: &JsonValue) -> Result<PathBuf, NativeError> {
    let id = string(a, "generation")?;
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(external());
    }
    Ok(c.join("deadlock_native").join(id))
}
fn verify_generation_mode(
    c: &Path,
    root: &Path,
    a: &JsonValue,
    fast: bool,
) -> Result<(), NativeError> {
    if a.is_null() {
        return Ok(());
    }
    generation::packages(&a["manifest"])?; // Do not infer shard ownership here.
    if !workspace(c, root)? {
        return Err(external());
    }
    let dir = generation_dir(c, a)?;
    paths::reject_symlinks(&dir)?;
    if a["manifest"]["storeRoot"] != root.to_string_lossy().as_ref()
        || a["manifest"]["generation"] != a["generation"]
    {
        return Err(external());
    }
    if a["manifest"]["version"] == 2 {
        if !generation::same_owner(&fs::metadata(&dir)?, &a["directoryIdentity"]) {
            return Err(external());
        }
        for name in generation_files(&a["manifest"])?
            .into_iter()
            .map(|f| f.name)
            .chain(std::iter::once("manifest.json".to_owned()))
        {
            let id = &a["identities"][&name];
            if id["device"].as_u64().is_none()
                || id["inode"].as_u64().is_none()
                || id["size"].as_u64().is_none()
                || id["modifiedSeconds"].as_i64().is_none()
                || id["modifiedNanos"].as_i64().is_none()
            {
                return Err(external());
            }
        }
    }
    generation::verify_generation_files(&dir, &a["manifest"], &a["identities"], fast)
}

fn verify_prior_source(p: &JsonValue, fast: bool) -> Result<(), NativeError> {
    let path = Path::new(string(p, "sourcePath")?);
    if p["proofKind"]=="mount" {
        if !complete_tree(p) {return Err(old_baseline_error());}
        if p.len()!=5 || !path.is_absolute() || !(p["files"].is_array() || p["files"].is_null()) {return Err(external());}
        let changed=||err("externalGameinfoChange","A previous mod folder changed or is missing. Preserve its files and review the setup again before putting it back in the game.");
        if p["files"].is_null(){return paths::verify_absent(path).map_err(|_|changed());}
        // This proof names a directory, possibly nested under addons*. It has
        // no game mount spelling; compare its recorded tree directly.
        if !crate::installed::matches_mount_files(path,&p["files"],true).map_err(|_|changed())? {return Err(changed());}
        if !fast { for file in p["files"].members(){verify_prior_source(file,false)?;} }
        return Ok(());
    }
    if p["proofKind"]=="directory" {
        paths::reject_symlinks(path)?;let m=fs::symlink_metadata(path)?;
        return if m.is_dir() && identity(&m)==p["identity"] {Ok(())}else{Err(external())};
    }
    match p["state"].as_str() {
        Some("missing") => paths::verify_absent(path).map_err(|_| external()),
        Some("unverifiable") => Err(err(
            "priorSetupUnavailable",
            "A prior manifest-owned source package could not be verified safely.",
        )),
        Some("present") | None => {
            if p["proofVersion"] == 2 {
                let m = paths::open_regular(path).and_then(|f| Ok(f.metadata()?)).map_err(|_| external())?;
                if !p["identity"].is_object() || identity(&m) != p["identity"] {
                    return Err(external());
                }
            }
            // Remount evidence must prove bytes even if inode, size and mtime match.
            if fast {verify_cached(path,p,&p["identity"],true).map_err(|_|external())}else{verify_file(path,p)}
        }
        _ => Err(external()),
    }
}
fn verify_engine_proof(files: &JsonValue, fast: bool) -> Result<(), NativeError> {
    if !files.is_array() {
        return Err(external());
    }
    let mut records = HashMap::new();
    let mut checked_folders=HashSet::new();
    for p in files.members() {
        if p["proofKind"]=="mount" {verify_prior_source(p,fast)?;continue;}
        let path = string(p, "sourcePath")?;
        if p["proofVersion"] != 2
            || p["member"].as_bool().is_none()
            || p["directory"].as_bool().is_none()
        {
            return Err(external());
        }
        if let Some(old) = records.insert(path, p) {
            let mut left = old.clone();
            let mut right = p.clone();
            left.remove("member");
            right.remove("member");
            if left != right {
                return Err(external());
            }
            continue;
        }
        if p["installedOrigin"]==true && !files.members().any(|m|m["proofKind"]=="mount" && m["files"].members().any(|f|f["sourcePath"]==p["sourcePath"])) {
            let folder=Path::new(path).parent().ok_or_else(external)?;
            if checked_folders.insert(folder.to_path_buf()) {
            let expected:HashSet<_>=files.members().filter(|r|r["proofKind"]!="mount" && r["state"]!="missing").filter_map(|r|r["sourcePath"].as_str()).filter(|p|Path::new(p).parent()==Some(folder)).map(PathBuf::from).collect();
            paths::reject_symlinks(folder)?;
            let actual:HashSet<_>=fs::read_dir(folder)?.map(|e|e.map(|e|e.path())).collect::<Result<Vec<_>,_>>()?.into_iter().filter(|p|p.extension().is_some_and(|e|e=="vpk")).collect();
            if actual!=expected {return Err(external());}
            }
        }
        let covered=files.members().any(|m|m["proofKind"]=="mount" && m["files"].members().any(|f|f["sourcePath"]==p["sourcePath"] && f["sha256"]==p["sha256"]));
        verify_prior_source(p, fast || covered)?;
    }
    for p in records.values() {
        if p["proofKind"]=="directory" {continue;}
        if p["proofKind"] == "bytes" {
            // Opaque upstream files are evidence only, never native payloads.
            // They must have a full identity and digest and are never parsed.
            if p["state"] != "present" || p["directory"] != false
                || !p["dependencies"].is_null()
            {
                return Err(external());
            }
            continue;
        }
        if !p["proofKind"].is_null() {
            return Err(external());
        }
        if p["state"] == "present" && p["directory"] == true {
            if !p["dependencies"].is_array() {
                return Err(external());
            }
            let path = Path::new(string(p, "sourcePath")?);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(external)?;
            let stem = stem.strip_suffix("_dir").unwrap_or(stem);
            let d = vpk::inspect_directory(path)?;
            let expected = JsonValue::from(
                vpk::required_archives(&d)
                    .into_iter()
                    .map(|i| {
                        path.with_file_name(format!("{stem}_{i:03}.vpk"))
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect::<Vec<_>>(),
            );
            if expected != p["dependencies"] {
                return Err(external());
            }
            for dep in p["dependencies"].members() {
                if !records.contains_key(dep.as_str().ok_or_else(external)?) {
                    return Err(external());
                }
            }
        } else if p["state"] == "present" && p["member"] != true {
            return Err(external());
        }
    }
    Ok(())
}
fn prior_mount_evidence(
    doc: &JsonValue,
    c: &Path,
    replaced: &HashSet<String>,
    before: &str,
    authority:Option<&GameLock>,
) -> Result<JsonValue, NativeError> {
    let incomplete = || {
        err("priorSetupUnavailable","Prior source-file evidence is unavailable. Review and commit a fresh upstream import in a new native library directory before applying; existing libraries cannot be replaced by import. Repair unsafe ownership or unreadable source files first.")
    };
    let sources: HashMap<_, _> = doc["importLedger"]["sourcePayloads"]
        .members()
        .filter_map(|p| {
            p["sourcePath"]
                .as_str()
                .map(|path| (PathBuf::from(path), p))
        })
        .collect();
    let mut owners = HashSet::new();
    let mut result = JsonValue::new_array();
    for manifest in doc["importLedger"]["manifests"].members() {
        let base = Path::new(string(manifest, "path")?)
            .parent()
            .ok_or_else(incomplete)?;
        let relative = base
            .strip_prefix(c)
            .map_err(|_| incomplete())?
            .to_str()
            .ok_or_else(incomplete)?;
        let suffix = relative.strip_prefix("addons").ok_or_else(incomplete)?;
        let relevant = (1..=10).any(|shard| {
            replaced.contains(
                &format!(
                    "citadel/{}{suffix}",
                    if shard == 1 {
                        "addons".to_owned()
                    } else {
                        format!("addons{shard}")
                    }
                )
                .to_ascii_lowercase(),
            )
        });
        if !relevant {
            continue;
        }
        for (remote, entry) in manifest["body"]["mods"].entries() {
            // Upstream can retain original download names after cleaning an
            // inactive mod. Empty current/disabled arrays declare no mounted
            // package; original names alone must not create phantom ownership.
            if entry["enabled"] == false
                && ["currentVpks", "disabledVpks"]
                    .iter()
                    .all(|key| entry[*key].is_array() && entry[*key].is_empty())
            {
                continue;
            }
            let (enabled, shard, names) =
                crate::import::validated_names(entry, remote).map_err(|_| incomplete())?;
            let directory = if enabled && shard > 1 {
                c.join(format!("addons{shard}{suffix}"))
            } else {
                base.to_owned()
            };
            let mount = format!(
                "citadel/{}",
                directory
                    .strip_prefix(c)
                    .map_err(|_| incomplete())?
                    .to_string_lossy()
            )
            .to_ascii_lowercase();
            if !replaced.contains(&mount) {
                continue;
            }
            for name in names {
                let path = directory.join(name);
                if !owners.insert(path.clone()) {
                    return Err(err(
                        "duplicateOwnership",
                        "Prior manifest entries ambiguously claim the same source package.",
                    ));
                }
                let original=*sources.get(&path).ok_or_else(incomplete)?;
                let (p,_) = baseline_view(original,c,authority)?;
                if package::layout_version(doc)? == 2 {
                    if doc["importLedger"]["upstreamEvidenceVersion"] != 2
                        || (p["engineComplete"] != true && p["priorEvidenceComplete"] != true)
                        || !p["engineFiles"].is_array()
                        || p["engineFiles"].is_empty()
                    {
                        return Err(incomplete());
                    }
                    if p["engineFiles"][0]["sourcePath"] != p["sourcePath"] {
                        return Err(incomplete());
                    }
                    verify_engine_proof(&p["engineFiles"], false)?;
                    for (index,record) in p["engineFiles"].members().enumerate() {
                        if record["proofVersion"] != 2 {
                            return Err(incomplete());
                        }
                        verify_prior_source(record, false)?;
                        result
                            .push(original["engineFiles"][index].clone())
                            .map_err(|_| NativeError::params())?;
                    }
                } else {
                    verify_prior_source(&p, false)?;
                    if p["state"] != "missing" {vpk::inspect(&path).map_err(|_| incomplete())?;}
                    result.push(original.clone()).map_err(|_| NativeError::params())?;
                }
            }
        }
    }
    for recorded in crate::installed::origins(doc)? {
        if replaced.contains(&string(&recorded,"mount")?.to_ascii_lowercase()) {
            let (origin,_)=baseline_view(&recorded,c,authority)?;
            crate::installed::verify_origin(&origin,c,true)?;
            let files=if complete_tree(&recorded) {recorded["files"].clone()}else{crate::installed::mount_proof(Path::new(string(&origin,"directory")?))?};
            result.push(object!{treeComplete:true,treeVersion:1,proofKind:"mount",sourcePath:origin["directory"].clone(),files:files.clone()}).map_err(|_|NativeError::params())?;
            for file in files.members() { if !result.members().any(|old|old["sourcePath"]==file["sourcePath"]) { let mut proof = file.clone(); proof["installedOrigin"] = true.into(); result.push(proof).map_err(|_|NativeError::params())?; } }
        }
    }
    capture_repair_mounts(&mut result,c,before)?;
    Ok(result)
}
fn capture_repair_mounts(result:&mut JsonValue,c:&Path,before:&str)->Result<(),NativeError>{
    for mount in gameinfo::mounts(before)?.into_iter().filter(|m| gameinfo::normalize_mount(m).as_deref()==Some("citadel/dmm_localization_default")) {
        let dir=gameinfo::mount_directory(c,&mount)?;
        // Missing mounts are also evidence: never remount a subsequently created folder.
        result.push(object!{treeComplete:true,treeVersion:1,proofKind:"mount",sourcePath:dir.to_str(),files:if dir.try_exists()? {crate::installed::mount_proof(&dir)?} else {JsonValue::Null}}).map_err(|_|NativeError::params())?;
    }
    Ok(())
}
fn admitted_restore_setup(before:&JsonValue,after:&JsonValue)->Result<JsonValue,NativeError>{
    if before!=after{return Err(recovery());}Ok(before.clone())
}
pub(crate) fn restore_review(store:&Store)->Result<JsonValue,NativeError>{
    refuse_previous_folder(&store.document()?)?;
    Ok(object!{restoreReview:true})
}
pub(crate) fn refuse_previous_folder(doc:&JsonValue)->Result<(),NativeError>{
    if doc["gameFolderChanged"]==true {
        return Err(err("previousGameFolder",&format!("This restore point belongs to the previous game folder ({}). Apply again to set up mods in the new folder.",string(&doc["previousGameFolders"][doc["previousGameFolders"].len()-1],"gamePath")?)));
    } Ok(())
}
#[cfg(test)]
thread_local! {pub(crate) static BEFORE_FOLDER_COMMIT:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
pub(crate) fn ensure_folder_change_idle(store:&Store)->Result<(),NativeError>{
    if !store.journals()?.is_empty() {
        return Err(err("recoveryRequired","Finish or recover the current operation before changing the game folder."));
    }
    Ok(())
}
pub(crate) fn change_game_folder(store:&Store,doc:&mut JsonValue,game:&Path)->Result<GameLock,NativeError>{
    ensure_folder_change_idle(store)?;
    // Compare spellings before installation admission resolves any links.
    let chosen=paths::lexical_absolute(game)?;
    // A saved spelling that can't be compared (e.g. older data with "..") is
    // simply "different": the user can still choose a folder.
    let saved=doc["gamePath"].as_str().filter(|p|!p.is_empty()).and_then(|p|paths::lexical_absolute(Path::new(p)).ok());
    let game=crate::installed::install_path(&chosen)?;
    let c=citadel(&game)?; gameinfo::mounts(&text(&c.join("gameinfo.gi"))?)?;
    let guard=GameLock::acquire(&c)?;game_guard()?;
    // Same folder only if the spelling matches AND it still resolves to itself.
    // A saved folder later swapped for a link to another install is a change.
    if saved.as_ref()==Some(&chosen) && saved.as_deref()==Some(game.as_path()){
        return Ok(guard);
    }
    let had_game=doc["gamePath"].as_str().is_some_and(|p|!p.is_empty());
    // Archive verbatim, including old evidence. Never rebase it into new ownership.
    let mut archive=doc.clone();archive.remove("previousGameFolders");
    if !doc["previousGameFolders"].is_array(){doc["previousGameFolders"]=JsonValue::new_array();}
    if had_game {doc["previousGameFolders"].push(archive).map_err(|_|NativeError::params())?;}
    let mut unmanaged=JsonValue::new_array();
    if workspace(&c,store.root())? {for e in fs::read_dir(c.join("deadlock_native"))? {
        let e=e?;let name=e.file_name().into_string().map_err(|_|external())?;
        if name!="owner.json" {if !package::hex(&name,32)||!e.file_type()?.is_dir(){return Err(external());} unmanaged.push(name).map_err(|_|NativeError::params())?;}
    }}
    let retained=gameinfo::mounts(&text(&c.join("gameinfo.gi"))?)?.into_iter().filter(|m|m.starts_with("citadel/deadlock_native/")).collect::<Vec<_>>();
    for m in &retained {let tail=m.strip_prefix("citadel/deadlock_native/").ok_or_else(external)?;let id=tail.split('/').next().ok_or_else(external)?;if !package::hex(id,32) || gameinfo::normalize_mount(m).as_deref()!=Some(m.as_str()) || tail.split('/').any(|p|p.is_empty() || p==".." || p=="."){return Err(external());}}
    doc["unmanagedGameMounts"]=retained.into();doc["unmanagedGameGenerations"]=unmanaged;
    // Save the validated real location (links resolved by install_path), so a
    // later Apply never rejects a symlink spelling. Sameness stays lexical above.
    doc["gamePath"]=game.to_string_lossy().as_ref().into();
    doc["gameFolderChanged"]=had_game.into();doc["canRestore"]=false.into();
    for key in ["applied","previous","restoredSetup"] {doc[key]=JsonValue::Null;}
    doc["fonts"]=crate::fonts::FontState::default().encode()?;
    doc["retiredGenerations"]=JsonValue::new_array();
    doc["importLedger"]["manifests"]=JsonValue::new_array();
    Ok(guard)
}
pub(super) fn unmanaged_mounts(doc:&JsonValue)->Vec<String>{doc["unmanagedGameMounts"].members().filter_map(|m|m.as_str().map(str::to_owned)).collect()}

// A temporary comparison view, never persisted as upgraded legacy proof.
// Only the held Citadel admits legacy device drift. Paths outside it require
// their own recorded fsid, or the original strict device number.
fn baseline_view(value:&JsonValue,c:&Path,authority:Option<&GameLock>)->Result<(JsonValue,bool),NativeError>{
    fn visit(v:&mut JsonValue,c:&Path,authority:Option<&GameLock>,drift:&mut bool)->Result<(),NativeError>{
        if v.is_object(){
            let pair=if v["sourcePath"].is_string() && v["identity"].is_object(){Some(("sourcePath","identity"))}
                else if v["directory"].is_string() && v["directoryIdentity"].is_object(){Some(("directory","directoryIdentity"))}else{None};
            if let Some((path_key,id_key))=pair {
                let path=PathBuf::from(string(v,path_key)?);let old=v[id_key].clone();
                if !generation::owner_matches(&path,&old,if path.starts_with(c){authority}else{None})?{return Err(external());}
                let m=fs::symlink_metadata(&path)?;
                *drift|=old["device"].as_u64()!=Some(m.dev());
                // Some historical file rows repeat metadata outside identity.
                // Require consistency before adjusting the comparison view.
                for key in ["device","inode","size","modifiedSeconds","modifiedNanos"] {
                    if v.has_key(key) && v[key]!=old[key]{return Err(external());}
                }
                v[id_key]["device"]=m.dev().into();v[id_key].remove("fsid");
                if v.has_key("device"){v["device"]=m.dev().into();}
            }
            for (_,child) in v.entries_mut(){visit(child,c,authority,drift)?;}
        }else if v.is_array(){for child in v.members_mut(){visit(child,c,authority,drift)?;}}
        Ok(())
    }
    let mut view=value.clone();let mut drift=false;visit(&mut view,c,authority,&mut drift)?;Ok((view,drift))
}
fn capture_baseline_fsids(v:&mut JsonValue)->Result<(),NativeError>{
    if v.is_object(){
        let pair=if v["sourcePath"].is_string() && v["identity"].is_object(){Some(("sourcePath","identity"))}
            else if v["directory"].is_string() && v["directoryIdentity"].is_object(){Some(("directory","directoryIdentity"))}else{None};
        if let Some((path_key,id_key))=pair {
            let path=Path::new(string(v,path_key)?);paths::reject_symlinks(path)?;
            let file=OpenOptions::new().read(true).custom_flags(0x20000).open(path)?;
            if !generation::same_owner(&file.metadata()?,&v[id_key]){return Err(external());}
            v[id_key]["fsid"]=paths::persisted_filesystem_id(&file)?.into();
        }
        for (_,child) in v.entries_mut(){capture_baseline_fsids(child)?;}
    }else if v.is_array(){for child in v.members_mut(){capture_baseline_fsids(child)?;}}
    Ok(())
}

fn check_rollback_upstream(setup:&JsonValue,c:&Path,authority:Option<&GameLock>)->Result<(),NativeError>{
    let mut evidence=setup.clone();evidence.remove("gameinfo");check_upstream(&evidence, false, c, authority)
}

fn check_upstream(setup: &JsonValue, fast: bool, c: &Path, authority: Option<&GameLock>) -> Result<(), NativeError> {
    let (view, drift) = baseline_view(setup, c, authority)?;
    let setup = &view;
    let fast = fast && !drift;
    verify_repair_mounts(setup,c)?;
    if setup["gameinfo"].is_string()
        && setup["applied"].is_null()
        && setup["upstreamEvidenceVersion"] != 1
        && setup["upstreamEvidenceVersion"] != 2
    {
        return Err(err("priorSetupUnavailable","This older prior setup has no complete source-footprint proof; preserve its backup for explicit reconciliation."));
    }
    if setup["upstreamEvidenceVersion"] == 2 {
        return verify_engine_proof(&setup["upstreamPayloads"], fast);
    }
    for p in setup["upstreamPayloads"].members() {
        verify_prior_source(p, fast)?;
        if p["proofKind"]=="mount" {continue;}
        if p["state"] != "missing" {
            vpk::inspect(Path::new(string(p, "sourcePath")?)).map_err(|_| {
                err(
                    "priorSetupUnavailable",
                    "Legacy prior proof requires a complete standalone directory.",
                )
            })?;
        }
    }
    Ok(())
}
pub(crate) fn complete_tree(record:&JsonValue)->bool {record["treeComplete"]==true && record["treeVersion"]==1}
fn old_baseline_error()->NativeError {err("unverifiedRestore","This restore point was made by an earlier version and can't be checked fully. Apply your current setup to create a new restore point.")}
fn needs_folder_baseline(mount:&str)->bool {mount.to_ascii_lowercase().strip_prefix("citadel/").and_then(|p|p.split('/').next()).is_some_and(|s|s.starts_with("addons"))}
// New checkpoints capture the mounted folders now, independently of profiles.
fn capture_checkpoint_baselines(setup:&mut JsonValue,c:&Path)->Result<(),NativeError>{
    let mut origins=JsonValue::new_array();
    if let Some(text)=setup["gameinfo"].as_str(){for mount in gameinfo::mounts(text)?.into_iter().filter(|m|needs_folder_baseline(&gameinfo::normalize_mount(m).unwrap_or_default())) {
        let dir=gameinfo::mount_directory(c,&mount)?;
        if !dir.try_exists()? {
            origins.push(object!{treeComplete:true,treeVersion:1,mount:gameinfo::normalize_mount(&mount).ok_or_else(external)?,directory:dir.to_str(),directoryMissing:true,files:JsonValue::Null}).map_err(|_|recovery())?;
        } else {
        origins.push(object!{treeComplete:true,treeVersion:1,mount:gameinfo::normalize_mount(&mount).ok_or_else(external)?,directory:dir.to_str(),directoryIdentity:object!{device:fs::metadata(&dir)?.dev(),inode:fs::metadata(&dir)?.ino()},files:crate::installed::mount_proof(&dir)?}).map_err(|_|recovery())?;
        }
    }}
    setup["installedGameOrigins"]=origins;capture_baseline_fsids(&mut setup["installedGameOrigins"])?;Ok(())
}
pub(crate) fn legacy_baseline_reason(doc:&JsonValue)->Option<String>{
    if doc["canRestore"]!=true {return None;}
    let setup=&doc["previous"];
    let text=setup["gameinfo"].as_str()?;
    let mounts=gameinfo::mounts(text).ok()?;
    // Recorded eligibility must be deterministic during commit/recovery. Live
    // repair contents are checked by restore_baseline_reason and pre-operation verification.
    if let Some(game)=doc["gamePath"].as_str() {
        let c=Path::new(game).join("game/citadel");
        for mount in mounts.iter().filter(|m|gameinfo::normalize_mount(m).as_deref()==Some("citadel/dmm_localization_default")) {
            let exact=gameinfo::mount_path(&c,mount).ok()?;
            if !setup["upstreamPayloads"].members().any(|p|complete_tree(p) && p["proofKind"]=="mount" && p["sourcePath"].as_str()==exact.to_str()) {return Some(old_baseline_error().message);}
        }
    }
    mounts.into_iter().filter_map(|m|gameinfo::normalize_mount(&m)).any(|m|needs_folder_baseline(&m) && !setup["installedGameOrigins"].members().any(|o|complete_tree(o) && o["mount"].as_str().is_some_and(|v|v.eq_ignore_ascii_case(&m))))
        .then(||old_baseline_error().message)
}
pub(crate) fn restore_baseline_reason(doc:&JsonValue)->Option<String>{check_restore_baseline_reason(doc,None)}
pub(crate) fn check_restore_baseline_reason(doc:&JsonValue,authority:Option<&GameLock>)->Option<String>{
    if doc["canRestore"]!=true {return None;}
    let checked=(||{let c=citadel(Path::new(string(doc,"gamePath")?))?;check_installed_baselines(&doc["previous"], &c, true, authority)})();
    checked.err().map(|e|e.message)
}
fn verify_repair_mounts(setup:&JsonValue,c:&Path)->Result<(),NativeError>{
    if let Some(text)=setup["gameinfo"].as_str() {
        for mount in gameinfo::mounts(text).map_err(|_|external())?.iter().filter(|m|gameinfo::normalize_mount(m).as_deref()==Some("citadel/dmm_localization_default")) {
            let exact=gameinfo::mount_path(c,mount)?;
            let proof=setup["upstreamPayloads"].members().find(|p|complete_tree(p) && p["proofKind"]=="mount" && p["sourcePath"].as_str()==exact.to_str()).ok_or_else(old_baseline_error)?;
            gameinfo::mount_directory(c,mount)?;
            verify_prior_source(proof,true)?;
        }
    }
    Ok(())
}

fn check_installed_baselines(setup: &JsonValue, c: &Path, fast: bool, authority: Option<&GameLock>) -> Result<(), NativeError> {
    let (view, drift) = baseline_view(setup, c, authority)?;
    let setup = &view;
    let fast = fast && !drift;
    verify_repair_mounts(setup,c)?;
    let Some(text)=setup["gameinfo"].as_str() else{return Ok(())};
    let mounts=gameinfo::mounts(text).map_err(|_|external())?;
    for mount in mounts.iter().filter(|m|gameinfo::normalize_mount(m).is_some_and(|m|needs_folder_baseline(&m))) {
        if let Some(origin)=setup["installedGameOrigins"].members().find(|o|o["mount"].as_str().is_some_and(|m|gameinfo::normalize_mount(m)==gameinfo::normalize_mount(mount))) {
            if !complete_tree(origin) {return Err(old_baseline_error());}
            let exact=gameinfo::mount_path(c,mount)?;
            if origin["directory"].as_str()!=exact.to_str() {return Err(err("unverifiedRestore","The restore point recorded a different folder spelling from its game mount. Its files cannot be checked safely."));}
            gameinfo::mount_directory(c,mount)?;
            crate::installed::verify_origin(origin,c,fast)?;
            continue;
        }
        return Err(old_baseline_error());
    }
    Ok(())
}
pub fn check_applied(root: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<(), NativeError> {
    check_state(root, doc, false, false, authority)
}
pub fn check_snapshot(root: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<(), NativeError> {
    check_state(root, doc, true, true, authority)
}
fn check_state(root: &Path, doc: &JsonValue, fast: bool, upstream_fast:bool, authority: Option<&GameLock>) -> Result<(), NativeError> {
    if doc["restoredSetup"].is_object() && !doc["applied"].is_null() {
        let c=citadel(Path::new(string(doc,"gamePath")?))?;
        check_installed_baselines(&doc["restoredSetup"], &c, true, authority)?;
        check_upstream(&doc["restoredSetup"], upstream_fast, &c, authority)?;
    }
    if doc["applied"].is_null() {
        if doc["restoredSetup"].is_object() {
            let c = citadel(Path::new(string(doc, "gamePath")?))?;
            if text(&c.join("gameinfo.gi"))? != string(&doc["restoredSetup"], "gameinfo")? {
                return Err(external());
            }
            check_installed_baselines(&doc["restoredSetup"], &c, true, authority)?;
            check_upstream(&doc["restoredSetup"], upstream_fast, &c, authority)?;
        }
        return Ok(());
    }
    let c = citadel(Path::new(string(doc, "gamePath")?))?;
    if doc["restoredSetup"].is_null() && gameinfo::mounts(string(&doc["applied"],"gameinfo")?)?.iter().any(|m|m=="citadel/addons") {
        return Err(external());
    }
    if text(&c.join("gameinfo.gi"))? != string(&doc["applied"], "gameinfo")? {
        return Err(external());
    }
    shards::check_checkpoint(&c, root, &doc["applied"], fast, authority)
}
/// A completed checkpoint with a changed gameinfo is external drift, never
/// authority to replay its old before image. Payload ownership must still verify.
pub(crate) fn check_external(root: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<bool, NativeError> {
    check_external_mode(root,doc,false, authority)
}
pub(crate) fn check_external_mode(root: &Path, doc: &JsonValue, fast: bool, authority: Option<&GameLock>) -> Result<bool, NativeError> {
    if doc["gameFolderChanged"]==true {return Ok(true);}
    let checkpoint=if !doc["applied"].is_null() { &doc["applied"] } else { &doc["restoredSetup"] };
    if !checkpoint.is_object() { return Ok(false); }
    let c=citadel(Path::new(string(doc,"gamePath")?))?;
    if text(&c.join("gameinfo.gi"))? == string(checkpoint,"gameinfo")? { return Ok(false); }
    shards::check_checkpoint(&c,root,&doc["applied"],fast, authority)?;
    shards::check_checkpoint(&c,root,&doc["previous"]["applied"],fast, authority)?;
    Ok(true)
}
fn external_gameinfo_error() -> NativeError {
    err("externalGameinfoChange", "Deadlock's game files changed after Apply. Apply again to use the current files; Restore and Recover cannot safely restore the older game files.")
}
fn check_base(root: &Path, doc: &JsonValue, authority: Option<&GameLock>) -> Result<(),NativeError> {
    if check_external(root,doc, authority)? { Ok(()) } else { check_applied(root,doc, authority) }
}
pub fn ensure_idle(store: &Store) -> Result<(), NativeError> {
    if store.journals()?.len() != 0 {
        return Err(recovery());
    }
    match store.document() {
        Ok(doc)=>crate::deployment_metadata::admit(store.root(),&doc)?,
        Err(e) if e.kind=="nativeNotImported"=>{},
        Err(e)=>return Err(e),
    }
    Ok(())
}
fn game_files_changed() -> NativeError {
    err("stalePreview", "The game files changed during Apply. Review again.")
}
fn addons_file_set(c: &Path) -> Result<Vec<String>, NativeError> {
    let dir = gameinfo::mount_directory(c, "citadel/addons")?;
    if !dir.try_exists()? { return Ok(vec![]); }
    let mut entries = fs::read_dir(dir)?.map(|e| {
        let e = e?;
        Ok(format!("{}:{}", e.file_name().to_string_lossy(), e.file_type()?.is_file()))
    }).collect::<Result<Vec<_>, std::io::Error>>()?;
    entries.sort();
    Ok(entries)
}
#[derive(Clone)]
pub struct ApplyPlan {
    hashes: paths::HashMemo,
    payload_stamps: Vec<(PathBuf,String)>,
    pub root: PathBuf,
    pub summary: JsonValue,
    revision: u64,
    game: PathBuf,
    profile: String,
    before: String,
    after: String,
    generation: String,
    version: u32,
    intent: crate::deployment_intent::DeploymentIntent,
    payload_evidence: Vec<(PathBuf, paths::Fingerprint)>,
    enabled: JsonValue,
    gameinfo_fp: paths::Fingerprint,
    manifest_evidence: Vec<(PathBuf, paths::Fingerprint)>,
    upstream_payloads: JsonValue,
    upstream_live: JsonValue,
    installed_origins: Vec<JsonValue>,
    layout: crate::deployment_layout::DeploymentLayout,
    vanilla: bool,
    repairs: JsonValue,
    repair_resources: Vec<PathBuf>,
    takeover_files: Option<Vec<String>>,
}
impl ApplyPlan {
    pub fn review(store: &Store, profile: &str, game: &Path) -> Result<Self, NativeError> {
        Self::review_with_guard(store, profile, game, None, false, &mut |_| {}, None)
    }
    pub fn review_locked(store:&Store,root_lock:&RootLock,profile:&str,game:&Path)->Result<Self,NativeError>{
        store.locked(root_lock)?;Self::review_with_guard(store,profile,game,None,false,&mut |_|{},Some(root_lock))
    }
    pub(crate) fn review_shards_locked(store: &Store, profile: &str, game: &Path, guard: &GameLock, root_lock:&RootLock, progress:&mut dyn FnMut(crate::launch_preparation::PreparationProgress)) -> Result<Self, NativeError> {
        Self::review_with_guard(store, profile, game, Some(guard), false, progress,Some(root_lock))
    }
    pub(crate) fn review_vanilla_locked(store: &Store, game: &Path, guard: &GameLock,root_lock:&RootLock) -> Result<Self, NativeError> {
        let doc = store.document()?;
        let intent = crate::deployment_intent::DeploymentIntent::empty_active(&doc)?;
        Self::review_with_guard(store, &intent.profile_id, game, Some(guard), true, &mut |_| {},Some(root_lock))
    }
    fn review_with_guard(store: &Store, profile: &str, game: &Path, guard: Option<&GameLock>, vanilla: bool, progress:&mut dyn FnMut(crate::launch_preparation::PreparationProgress),root_lock:Option<&RootLock>) -> Result<Self, NativeError> {
        let _phase=paths::ApplyPhase::new("preview");
        let _hash_scope=paths::HashMemo::default().enter();
        store.seed_apply_hashes()?;
        ensure_idle(store)?;
        let doc = store.document()?;
        store::ensure_readable(&doc)?;
        let version = package::layout_version(&doc)?;
        let intent = if vanilla { crate::deployment_intent::DeploymentIntent::empty_active(&doc)? }
            else { crate::deployment_intent::DeploymentIntent::capture(&doc,profile)? };
        let layout = shards::placement(&intent,&doc["profiles"][profile])?;
        if !doc["profiles"][profile].is_object() {
            return Err(err(
                "unknownProfile",
                "Requested native profile does not exist.",
            ));
        }
        let game = paths::lexical_absolute(game)?;
        if paths::lexical_absolute(Path::new(string(&doc,"gamePath")?))? != game {
            return Err(err(
                "gamePathMismatch",
                "Apply must use the installation whose manifests were imported.",
            ));
        }
        let c = citadel(&game).and_then(|c| {
            gameinfo::mounts(&text(&c.join("gameinfo.gi"))?)?;
            Ok(c)
        }).map_err(|_| err("invalidGamePath", "This folder is missing, unreadable, or is not a Deadlock installation. Choose the Steam Deadlock folder."))?;
        let owned_guard;
        let guard = match guard {
            Some(guard) => guard,
            None => { owned_guard = GameLock::acquire(&c)?; &owned_guard }
        };
        guard.validate(&c)?;
        game_guard()?;
        let font_authority=root_lock.map(|r|crate::fonts::FontAuthority::new(store,r,guard)).transpose()?;
        crate::fonts::verify_current_with(store,&doc,font_authority.as_ref())?;
        workspace(&c, store.root())?;
        check_base(store.root(), &doc, Some(guard))?;
        let before = text(&c.join("gameinfo.gi"))?;
        let gameinfo_fp = paths::fingerprint(&c.join("gameinfo.gi"))?;
        let mut known = HashSet::new();
        let mut evidence = Vec::new();
        for m in doc["importLedger"]["manifests"].members() {
            let path = PathBuf::from(string(m, "path")?);
            let parent = path.parent().ok_or_else(external)?;
            let relative = parent.strip_prefix(&c).map_err(|_| external())?;
            let name = relative.to_str().ok_or_else(external)?;
            let bytes = paths::read_bounded(&path, 8 * 1024 * 1024)?;
            let current = json::parse(std::str::from_utf8(&bytes).map_err(|_| external())?)
                .map_err(|_| external())?;
            if current != m["body"] {
                return Err(external());
            }
            evidence.push((path.clone(), paths::fingerprint(&path)?));
            known.insert(format!("citadel/{name}").to_ascii_lowercase());
            // Only shards explicitly present in the imported manifest have ownership.
            for (_, entry) in m["body"]["mods"].entries() {
                if let Some(shard) = entry["shard"].as_u64().filter(|s| *s > 1 && *s <= 10) {
                    let suffix = name.strip_prefix("addons").ok_or_else(external)?;
                    known.insert(format!("citadel/addons{shard}{suffix}").to_ascii_lowercase());
                }
            }
        }
        let mut installed_origins=Vec::new();
        for origin in crate::installed::origins_for_apply(&doc,profile)? {
            if doc["applied"].is_null() {
                let (view,_)=baseline_view(&origin,&c,Some(guard))?;
                crate::installed::verify_origin(&view,&c,false)?;
                installed_origins.push(view);
            }
            known.insert(string(&origin,"mount")?.to_ascii_lowercase());
        }
        let repair_review=if !vanilla {Some(crate::repairs::review(store,&doc,profile,&game,progress)?)}else{None};
        let repair_resources=repair_review.as_ref().map(|r|r.resources.clone()).unwrap_or_default();
        let repairs=repair_review.as_ref().map(|r|r.receipt.clone()).unwrap_or(JsonValue::Null);
        // Reviewed Apply explicitly replaces the upstream mount, preserving its folder.
        if !vanilla {known.insert("citadel/dmm_localization_default".into());}
        let generation = paths::random_id()?;
        let mut manifest = shards::manifest(store.root(), &generation, version, &layout);
        if !repairs.is_null(){manifest["repairs"]=repairs.clone();}
        let (after, replaced) = gameinfo::replace_many_unmanaged(&before, &known, &shards::present_mounts(&before,&doc["applied"])?, &generation::mounts(&manifest)?, &unmanaged_mounts(&doc))?;
        let preset = if vanilla { "" } else { doc["performancePreset"].as_str().unwrap_or("") };
        let after = crate::performance::overlay(&after, preset)?;
        let replaced_normalized: HashSet<_> = replaced
            .iter()
            .filter_map(|m| gameinfo::normalize_mount(m))
            .collect();
        let external_change = !doc["applied"].is_null() && check_external(store.root(),&doc, Some(guard))?;
        let upstream_payloads = if doc["applied"].is_null() || external_change {
            { let mut scoped=doc.clone();scoped["activeProfileId"]=profile.into();prior_mount_evidence(&scoped, &c, &replaced_normalized, &before,Some(guard))? }
        } else {
            let mut proof=JsonValue::new_array();
            capture_repair_mounts(&mut proof,&c,&before)?;
            proof
        };
        // Keep durable records verbatim; only this same-boot review frame is normalized.
        let (upstream_live,_)=baseline_view(&upstream_payloads,&c,Some(guard))?;
        let selected_count = if vanilla { 0 } else { doc["profiles"][profile]["mods"]
            .members()
            .filter(|m| m["enabled"] == true)
            .try_fold(0usize, |n, m| {
                n.checked_add(package::selected_packages_exact(m, version)?.len())
                    .ok_or_else(NativeError::params)
            })? };
        let records: HashMap<_,_> = doc["profiles"][profile]["mods"].members()
            .map(|m|Ok((string(&m["source"],"id")?,m))).collect::<Result<_,NativeError>>()?;
        let mods = crate::profile_edit::ordered_mod_ids(&doc["profiles"][profile])?;
        let mut files = JsonValue::new_array();
        let (mut payload_evidence, payload_hashes) = repair_review
            .map(|r| (r.evidence, r.payload_hashes)).unwrap_or_default();
        let mut enabled = JsonValue::new_object();
        let mut owners = HashMap::<String, usize>::new();
        let mut resources: HashMap<String, (u32, String)> = HashMap::new();
        let mut warnings = JsonValue::new_array();
        if external_change {
            warnings.push(object!{code:"externalGameinfoChange",message:"The game files changed after the last Apply. Review the current setup before applying again."}).map_err(|_|NativeError::params())?;
        }
        if !doc["unmanagedGameGenerations"].is_empty() {
            warnings.push(object!{code:"unmanagedMovedGenerations",message:"Existing native mod folders moved with this installation. They stay in place as unmanaged files; their existing mounts stay active. This app will not remove or claim them."}).map_err(|_|NativeError::params())?;
        }
        for old in doc["previousGameFolders"].members() {
            if let Some(path)=old["gamePath"].as_str() {
                if let Ok(c)=citadel(Path::new(path)) {if text(&c.join("gameinfo.gi")).is_ok_and(|s|gameinfo::mounts(&s).is_ok_and(|m|m.iter().any(|s|s.starts_with("citadel/deadlock_native/")))) {
                    warnings.push(object!{code:"oldInstallKeepsMods",message:format!("The old installation ({path}) keeps its mods until restored there.")}).map_err(|_|NativeError::params())?;
                }}
            }
        }
        let takeover_files = if replaced.iter().any(|m|m == "citadel/addons") { Some(addons_file_set(&c)?) } else { None };
        if replaced.iter().any(|m|m == "citadel/addons") {
            let dir=gameinfo::mount_directory(&c,"citadel/addons")?;
            let count=if dir.try_exists()? {
                fs::read_dir(&dir)?.try_fold(0usize,|n,e| -> Result<usize,NativeError> {
                    let e=e?;Ok(n+usize::from(e.file_type()?.is_file() && e.path().extension().is_some_and(|s|s=="vpk")))
                })?
            } else {0};
            warnings.push(object!{code:"originalManagerTakeover",message:format!("Turn off the original Deadlock Mod Manager's mods (citadel/addons, {count} files). The files stay where they are; Restore puts this back.")}).map_err(|_|NativeError::params())?;
        }
        if upstream_payloads.members().any(|p| p["state"] == "missing") {
            warnings.push(object!{code:"priorMissingFiles",message:"The previous setup has missing engine files. Restore preserves that exact incomplete setup."}).map_err(|_|NativeError::params())?;
        }
        let mut omitted_conflicts = 0usize;
        let mut total = 0u64;
        for mod_id in mods {
            let m = records.get(mod_id.as_str()).ok_or_else(NativeError::params)?;
            let id = string(&m["source"], "id")?;
            let on = !vanilla && m["enabled"].as_bool().unwrap_or(false);
            enabled[id] = on.into();
            if !on {
                continue;
            }
            let name: String = m["source"]["name"]
                .as_str()
                .unwrap_or(id)
                .chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect();
            for (flag, code, message) in [
                (
                    "isBlacklisted",
                    "blacklisted",
                    "is marked incompatible by imported metadata",
                ),
                (
                    "isObsolete",
                    "obsolete",
                    "is marked obsolete by imported metadata",
                ),
                (
                    "usesCriticalPaths",
                    "criticalPaths",
                    "modifies critical paths according to imported metadata",
                ),
            ] {
                if m["source"][flag] == true {
                    let reason: String = if flag == "isBlacklisted" {
                        m["source"]["blacklistReason"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(512)
                            .collect()
                    } else {
                        String::new()
                    };
                    warnings
                        .push(object! {code:code,message:format!("{name} {message}. {reason}")})
                        .map_err(|_| NativeError::params())?;
                }
            }
            let packages = package::selected_packages_exact(m, version)?;
            if m["available"] != true || packages.is_empty() {
                return Err(NativeError::new(
                    "unavailablePayload",
                    format!("{name} has unavailable packages; disable it or repair the import."),
                ));
            }
            let mut references = HashSet::new();
            for p in packages {
                if !references.insert((p.directory.id.clone(), p.directory.source_name.clone())) {
                    return Err(err(
                        "duplicateOwnership",
                        "A mod repeats the same source package reference.",
                    ));
                }
                let (entries, proof) =
                    generation::verify_package_evidence_with_review(&p, version, |f| store.payload_path(&f.id), &payload_hashes)?;
                payload_evidence.extend(proof);
                let key = package::family_key(&p)?;
                if let Some(index) = owners.get(&key).copied() {
                    let prior = &mut files[index];
                    if !prior["modIds"].members().any(|m| m == id) {
                        prior["modIds"]
                            .push(id)
                            .map_err(|_| NativeError::params())?;
                    }
                    continue;
                }
                owners.insert(key, files.len());
                for entry in entries {
                    if let Some((crc, other)) = resources.get(&entry.path) {
                        if *crc != entry.crc {
                            if entry.path.ends_with(".vmdl_c") {
                                return Err(NativeError::new(
                                    "modelConflict",
                                    format!(
                                        "{other} and {name} contain differing model {}.",
                                        entry.path
                                    ),
                                ));
                            }
                            if warnings.len() < 400 {
                                warnings.push(object!{code:"fileConflict",message:format!("{other} wins over {name} for {} (lower package order).",entry.path)}).map_err(|_|NativeError::params())?;
                            } else {
                                omitted_conflicts += 1;
                            }
                        }
                    } else {
                        resources.insert(entry.path, (entry.crc, name.to_owned()));
                    }
                }
                for f in std::iter::once(&p.directory).chain(p.companions.values()) {
                    total = total.checked_add(f.size).ok_or_else(NativeError::params)?;
                }
                let mut file = if version == 1 {
                    package::encode_file(&p.directory)
                } else {
                    package::encode_packages(&[p])[0].clone()
                };
                file["modId"] = id.into();
                file["modIds"] = json::array![id];
                files.push(file).map_err(|_| NativeError::params())?;
            }
        }
        if omitted_conflicts > 0 {
            warnings.push(object!{code:"additionalConflicts",message:format!("{omitted_conflicts} additional overlapping resources follow the same deterministic package order.")}).map_err(|_|NativeError::params())?;
        }
        let companion_count = if version == 2 {
            files.members().try_fold(0usize, |n, p| {
                n.checked_add(p["companions"].len())
                    .ok_or_else(NativeError::params)
            })?
        } else {
            0
        };
        let physical_count = files
            .len()
            .checked_add(companion_count)
            .ok_or_else(NativeError::params)?;
        let mut summary = object! {packageCount:selected_count,directoryCount:files.len(),physicalFileCount:physical_count,companionFileCount:companion_count,profileId:profile,fileCount:files.len(),totalBytes:total,gamePath:game.to_string_lossy().as_ref(),replacedMounts:replaced,warnings:warnings};
        summary["repairs"]=repairs.clone();
        summary["repairModCount"]=repairs["mods"].members().filter(|m|m["repairs"].as_u32().unwrap_or(0)>0).count().into();
        summary["repairSymptoms"]=repairs["mods"].members().flat_map(|m|m["fixSymptoms"].members().filter_map(|s|s.as_str().map(str::to_owned))).collect::<std::collections::BTreeSet<_>>().into_iter().collect::<Vec<_>>().into();
        summary["upstreamRepairMigration"]=if !vanilla && before.to_ascii_lowercase().contains("citadel/dmm_localization_default") {"replaceMountPreserveFolder".into()}else{JsonValue::Null};
        summary["performance"] = crate::performance::review(&before, preset)?;
        shards::summary(&mut summary, &layout)?;
        if !repairs["file"].is_null(){summary["physicalFileCount"]=(summary["physicalFileCount"].as_u64().unwrap_or(0)+1).into();summary["totalBytes"]=(summary["totalBytes"].as_u64().unwrap_or(0)+repairs["file"]["size"].as_u64().unwrap_or(0)).into();}
        if vanilla {
            summary["mode"]="vanilla".into();
            summary["profileName"]=doc["profiles"][profile]["name"].clone();
            summary["installedMounts"]=generation::mounts(&shards::manifest(store.root(), &generation, version, &layout))?.into();
            summary["fontsPreserved"]=true.into();
            store::ensure_internal(&summary)?;
        }
        Ok(Self {
            hashes: paths::HashMemo::capture(),
            payload_stamps: payload_evidence.iter().map(|(p,_)|Ok((p.clone(),paths::content_stamp(&paths::open_regular(p)?)?))).collect::<Result<_,NativeError>>()?,
            root: store.root().to_owned(),
            summary,
            revision: rev(&doc)?,
            game,
            profile: profile.into(),
            before,
            after,
            generation,
            version,
            intent,
            payload_evidence,
            enabled,
            gameinfo_fp,
            manifest_evidence: evidence,
            upstream_payloads,
            upstream_live,
            installed_origins,
            layout,
            vanilla,
            repairs,
            repair_resources,
            takeover_files,
        })
    }
    /// Full source evidence for preparation, never a fast snapshot shortcut.
    pub(crate) fn preparation_evidence(&self, store: &Store, guard: &GameLock,root_lock:&RootLock) -> Result<JsonValue, NativeError> {
        let _scope=self.hashes.enter();
        if self.root != store.root() { return Err(err("stalePreview", "Preparation belongs to another library.")); }
        let c = citadel(&self.game)?;
        guard.validate(&c)?;
        ensure_idle(store)?;
        self.recheck(store)?;
        let doc = store.document()?;
        store::ensure_readable(&doc)?;
        let intent = if self.vanilla { crate::deployment_intent::DeploymentIntent::empty_active(&doc)? }
            else { crate::deployment_intent::DeploymentIntent::capture(&doc, &self.profile)? };
        if intent != self.intent {
            return Err(err("stalePreview", "Desired preparation changed since review."));
        }
        verify_fonts(store, &doc,root_lock,Some(guard))?;
        if !check_external(store.root(),&doc, Some(guard))? && !doc["applied"].is_null() && doc["applied"]["gameinfoSha256"] != self.gameinfo_fp.sha256 {
            return Err(external());
        }
        shards::verify_preparation_namespace(store, &c, &doc, Some(guard))?;
        if self.vanilla { check_upstream(&doc["previous"], false, &c, Some(guard))?; }
        let fingerprints = |entries: &[(PathBuf, paths::Fingerprint)]| JsonValue::from(entries.iter()
            .map(|(path, fp)| object!{path:path.to_string_lossy().as_ref(),identity:crate::fonts::fingerprint_json(fp)})
            .collect::<Vec<_>>());
        let evidence = object!{intent:self.intent.encode(),sources:fingerprints(&self.payload_evidence),
            importedManifests:fingerprints(&self.manifest_evidence),gameinfo:crate::fonts::fingerprint_json(&self.gameinfo_fp)};
        store::ensure_internal(&evidence)?;
        guard.validate(&c)?;
        Ok(evidence)
    }
    pub(crate) fn verify_prepared_result(&self, doc: &JsonValue) -> Result<(), NativeError> {
        if doc["applied"]["generation"] != self.generation || doc["applied"]["gameinfo"] != self.after
            || crate::deployment_intent::applied_intent(&doc["applied"])? != Some(self.intent.clone()) {
            return Err(err("stalePreview", "Current deployment is not this preparation's Apply result."));
        }
        Ok(())
    }
    fn recheck_takeover(&self, c: &Path) -> Result<(), NativeError> {
        if let Some(expected) = &self.takeover_files {
            if &addons_file_set(c).map_err(|_| game_files_changed())? != expected { return Err(game_files_changed()); }
        }
        Ok(())
    }
    fn recheck(&self, store: &Store) -> Result<(), NativeError> {
        let _phase=paths::ApplyPhase::new("identity-recheck");
        if rev(&store.document()?)? != self.revision {
            return Err(err("staleRevision", "Library changed since Apply review."));
        }
        if !self.repairs.is_null() && crate::repairs::resource_membership(&self.game)?!=self.repair_resources {return Err(err("stalePreview","Game resource membership changed since review."));}
        let c = citadel(&self.game)?;
        self.recheck_takeover(&c)?;
        if paths::fingerprint(&c.join("gameinfo.gi"))? != self.gameinfo_fp
            || text(&c.join("gameinfo.gi"))? != self.before
        {
            return Err(err("stalePreview", "Gameinfo changed since Apply review."));
        }
        for (path, fp) in &self.manifest_evidence {
            if paths::fingerprint(path)? != *fp {
                return Err(err(
                    "stalePreview",
                    "Imported ownership manifest changed since review.",
                ));
            }
        }
        for (path,stamp) in &self.payload_stamps {
            if paths::content_stamp(&paths::open_regular(path)?)?!=*stamp {return Err(err("stalePreview","A package member changed since review."));}
        }
        for (path, fp) in &self.payload_evidence {
            let m = paths::open_regular(path)?.metadata()?;
            if m.len() != fp.size || m.dev() != fp.device || m.ino() != fp.inode
                || m.mtime() != fp.modified_seconds || m.mtime_nsec() != fp.modified_nanos {
                return Err(err("stalePreview", "A package member changed since review."));
            }
        }
        // Review captured every selected binding, including content aliases,
        // in payload_evidence after package validation. Unchanged identities
        // retain that validation without rehashing and reparsing each family.
        // These comparison frames were captured under the review's GameLock;
        // subsequent live checks remain strict and never normalize new drift.
        for origin in &self.installed_origins {crate::installed::verify_origin(origin,&c,true)?;}
        for p in self.upstream_live.members() {
            verify_prior_source(p, false)?;
        }
        game_guard()?;
        Ok(())
    }
    pub(crate) fn repair_receipt(&self)->JsonValue{self.repairs.clone()}
    fn manifest(&self, store: &Store) -> JsonValue {
        let mut manifest=shards::manifest(store.root(), &self.generation, self.version, &self.layout);
        if !self.repairs.is_null(){manifest["repairs"]=self.repairs.clone();}
        manifest
    }
    pub fn apply(
        &self,
        store: &Store,
        lock: &RootLock,
        expected: u64,
        id: &str,
        input: &JsonValue,
    ) -> Result<JsonValue, NativeError> {
        if let Some(result) = store.replay(id,input)? { return Ok(result); }
        if input["params"]["deploymentLayout"] != "shards-v1" {
            return Err(err("stalePreview", "Apply layout acknowledgement does not match the reviewed plan."));
        }
        let _scope=self.hashes.enter(); self.recheck(store)?; store.locked(lock)?;
        store.remember_apply_hashes(&self.payload_evidence)?;
        shards::apply(self, store, lock, expected, id, input)
    }
    /// Caller retains RootLock then this exact GameLock through later handoff admission.
    pub fn apply_locked(
        &self, store: &Store, lock: &RootLock, game_lock: &GameLock,
        expected: u64, id: &str, input: &JsonValue,
        control: &crate::prepare_control::PrepareControl,
        progress: &mut dyn FnMut(crate::launch_preparation::PreparationProgress),
    ) -> Result<JsonValue, NativeError> {
        if let Some(result) = store.replay(id, input)? { return Ok(result); }
        if input["params"]["deploymentLayout"] != "shards-v1" {
            return Err(err("stalePreview", "Apply requires the reviewed shards-v1 acknowledgement."));
        }
        let _scope=self.hashes.enter();
        self.recheck(store)?; store.locked(lock)?; store.remember_apply_hashes(&self.payload_evidence)?;
        shards::apply_locked(self, store, lock, game_lock, expected, id, input, control, progress)
    }
}

fn new_file(path: &Path) -> Result<File, NativeError> {
    paths::reject_symlinks(path.parent().ok_or_else(NativeError::params)?)?;
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(0x20000)
        .open(path)?)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), NativeError> {
    let mut f = new_file(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    paths::sync_dir(path.parent().unwrap())
}
fn prepare(
    store: &Store,
    lock: &RootLock,
    id: &str,
    input: &JsonValue,
    expected: u64,
    c: &Path,
    before: &str,
    after: &str,
    doc: JsonValue,
    workspace_existed: bool,
) -> Result<JsonValue, NativeError> {
    // Refuse an unreadable result before journaling or any game transaction.
    store::ensure_readable(&doc)?;
    // Exact backup is durable in SQLite before ANY target-filesystem mutation.
    let mut journal = object! {kind:"game",version:doc["storeVersion"].clone(),input:input.clone(),expectedRevision:expected.to_string(),citadel:c.to_string_lossy().as_ref(),before:before,after:after,beforeSha256:paths::digest_bytes(before.as_bytes())?,afterSha256:paths::digest_bytes(after.as_bytes())?,document:doc,workspaceExisted:workspace_existed,temp:format!(".native-gameinfo-{}",paths::random_id()?),generation:JsonValue::Null};
    if journal["version"] == 4 {
        journal["beforeDocument"] = store.document()?;
        journal["deploymentIntent"] = journal["document"]["applied"]["deploymentIntent"].clone();
        journal["desiredSignature"] = journal["document"]["applied"]["desiredSignature"].clone();
    }
    if journal["version"] == 3 || journal["version"] == 4 {
        journal["libraryVersion"] = journal["version"].clone();
        journal["packageVersion"] = package::layout_version(&journal["document"] )?.into();
    }
    if workspace_existed {
        journal["workspaceIdentity"]=generation::capture_identity(&c.join("deadlock_native"))?;
    }
    journal["fontStateDigest"] = crate::font_transaction::state_digest(&journal["document"] )?.into();
    admit_journal(&journal)?;
    if journal["version"]==4 { admit_game4(store,id,&journal)?; }
    store::admit_success(id,input,&reserved_game_document(&journal)?)?;
    store::admit_outcome(id,input,&store::failure_outcome(&game_rollback_error()))?;
    store.write_journal(lock, id, &journal)?;
    // Read back the committed backup, rather than trusting an in-memory copy.
    if store.journal(id)?.as_ref() != Some(&journal) {
        return Err(recovery());
    }
    Ok(journal)
}
/// Count the real serialization with all late fields at their maximum widths.
fn admit_journal(j: &JsonValue) -> Result<(), NativeError> {
    let projected=reserved_game_journal(j)?;
    store::ensure_readable(&projected["document"])?;
    store::ensure_internal(&projected)
}
fn reserved_game_journal(j: &JsonValue) -> Result<JsonValue,NativeError> {
    if matches!(j["version"].as_u32(),Some(5|6)) { return shards::reserved(j); }
    let mut projected = j.clone();
    let max = object! {size:u64::MAX,device:u64::MAX,inode:u64::MAX,modifiedSeconds:i64::MIN,modifiedNanos:999_999_999,fsid:"ffffffffffffffff"};
    if !j["generation"].is_null() {
        let mut ids = JsonValue::new_object();
        let mut staged = JsonValue::new_array();
        for (name,size) in generation_files(&j["document"]["applied"]["manifest"])?
            .into_iter()
            .map(|f| (f.name,f.payload.size))
            .chain(std::iter::once(("manifest.json".to_owned(),j["document"]["applied"]["manifest"].dump().len() as u64)))
        {
            ids[&name] = max.clone();
            ids[&name]["size"]=size.into();
            staged
                .push(object! {name:name,device:u64::MAX,inode:u64::MAX,fsid:"ffffffffffffffff",complete:false})
                .map_err(|_| NativeError::params())?;
        }
        projected["document"]["applied"]["identities"] = ids;
        projected["stagedFiles"] = staged;
        projected["generationIdentity"] = max.clone();
        if j["document"]["applied"]["manifest"]["version"]==2 {
            projected["document"]["applied"]["directoryIdentity"] = max.clone();
        }
    }
    // Apply records the workspace identity after creating it. Reserve it even
    // when the initial backup was written before that directory existed.
    projected["workspaceIdentity"] = max.clone();
    projected["tempIdentity"] = max;
    projected["tempIdentity"]["size"] = 0.into();
    Ok(projected)
}
fn reserved_game_document(j: &JsonValue) -> Result<JsonValue,NativeError> {
    let mut doc=reserved_game_journal(j)?["document"].clone();
    doc["revision"]=store::revision(string(j,"expectedRevision")?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
    Ok(doc)
}
fn game_rollback_error() -> NativeError {
    err("operationInterrupted","Operation interrupted before gameinfo commit; the exact prior setup remains active.")
}
fn exact_keys(value: &JsonValue, required: &[&str], optional: &[&str]) -> Result<(), NativeError> {
    if !value.is_object() || required.iter().any(|k| !value.has_key(k))
        || value.entries().any(|(k,_)| !required.contains(&k) && !optional.contains(&k)) {
        return Err(recovery());
    }
    Ok(())
}

fn game_identity(value: &JsonValue) -> Result<(), NativeError> {
    exact_keys(value, &["size", "device", "inode", "modifiedSeconds", "modifiedNanos"], &["fsid"])?;
    if !generation::valid_fsid(value) {return Err(recovery());}
    if value["size"].as_u64().is_none() || value["device"].as_u64().is_none()
        || value["inode"].as_u64().filter(|n| *n != 0).is_none()
        || value["modifiedSeconds"].as_i64().is_none()
        || value["modifiedNanos"].as_i64().filter(|n| (0..1_000_000_000).contains(n)).is_none() {
        return Err(recovery());
    }
    Ok(())
}

fn operation_binding(id: &str, input: &JsonValue, expected: &JsonValue, method: &str) -> Result<(), NativeError> {
    exact_keys(input, &["method", "params"], &[])?;
    if !paths::safe_id(id) || input["method"] != method || !input["params"].is_object()
        || input["params"]["operationId"] != id || input["params"]["expectedRevision"] != *expected {
        return Err(recovery());
    }
    store::revision(expected.as_str().ok_or_else(recovery)?)?;
    Ok(())
}

/// Retention can drop absent old generations, but cannot invent or edit one.
fn retired_transition(before: &JsonValue, after: &JsonValue, appended: &JsonValue) -> Result<(), NativeError> {
    if !after.is_array() || (!before.is_null() && !before.is_array()) { return Err(recovery()); }
    let mut rows: Vec<_> = after.members().collect();
    if !appended.is_null() && rows.pop() != Some(appended) { return Err(recovery()); }
    let mut old = before.members();
    for row in rows {
        if !old.any(|v| v == row) { return Err(recovery()); }
    }
    Ok(())
}

/// Pure admission of retained package checkpoints. Missing legacy intent stays
/// unknown; a newer/malformed generation never becomes cleanup authority.
pub(crate) fn admit_game_document(root: &Path, doc: &JsonValue) -> Result<(),NativeError> {
    generation::require_flat_document(doc)?;
    store::ensure_readable(doc)?;
    let layout=package::layout_version(doc)?;
    if !doc["retiredGenerations"].is_null() && !doc["retiredGenerations"].is_array() { return Err(recovery()); }
    for a in std::iter::once(&doc["applied"]).chain(std::iter::once(&doc["previous"]["applied"]))
        .chain(doc["retiredGenerations"].members()) {
        if a.is_null() { continue; }
        generation_files(&a["manifest"])?;
        if a["manifest"]["version"].as_u32()!=Some(layout) || a["manifest"]["generation"]!=a["generation"]
            || a["manifest"]["storeRoot"].as_str()!=root.to_str() { return Err(recovery()); }
    }
    Ok(())
}

/// Store4's complete metadata contract. This performs no filesystem access.
pub(crate) fn admit_game4(store: &Store, id: &str, j: &JsonValue) -> Result<(), NativeError> {
    generation::require_flat_document(&store.document()?)?;
    let required = ["kind", "version", "input", "expectedRevision", "citadel", "before", "after", "beforeSha256", "afterSha256", "document", "workspaceExisted", "temp", "generation", "libraryVersion", "packageVersion", "fontStateDigest", "deploymentIntent", "desiredSignature", "beforeDocument"];
    exact_keys(j, &required, &["generationIdentity", "stagedFiles", "tempIdentity", "workspaceIdentity"])?;
    let before = &j["beforeDocument"];
    let after = &j["document"];
    let layout = package::layout_version(before)?;
    if j.has_key("workspaceIdentity") {game_identity(&j["workspaceIdentity"])?;}
    if j["kind"] != "game" || j["version"] != 4 || j["libraryVersion"] != 4
        || package::document_version(before)? != 4 || package::document_version(after)? != 4
        || package::layout_version(after)? != layout || j["packageVersion"].as_u32() != Some(layout)
        || before["revision"] != j["expectedRevision"] || after["revision"] != j["expectedRevision"]
        || j["workspaceExisted"].as_bool().is_none()
        || !string(j,"temp")?.strip_prefix(".native-gameinfo-").is_some_and(|s| package::hex(s,32)) {
        return Err(recovery());
    }
    let game = Path::new(string(before,"gamePath")?);
    if !game.is_absolute() || paths::absolute(game)? != game
        || game.join("game/citadel").to_str() != j["citadel"].as_str() { return Err(recovery()); }
    for key in ["before", "after"] {
        if string(j,key)?.len() > MAX_GAMEINFO as usize { return Err(recovery()); }
    }
    verify_journal_bytes(j)?;
    admit_game_document(store.root(),before)?;
    admit_game_document(store.root(),after)?;
    if j["fontStateDigest"] != crate::font_transaction::state_digest(before)?
        || j["fontStateDigest"] != crate::font_transaction::state_digest(after)?
        || j["deploymentIntent"] != after["applied"]["deploymentIntent"]
        || j["desiredSignature"] != after["applied"]["desiredSignature"] {
        return Err(recovery());
    }
    let apply = j["input"]["method"] == "native.apply";
    operation_binding(id,&j["input"],&j["expectedRevision"],if apply {"native.apply"} else {"native.restore"})?;
    let mut projected = before.clone();
    if apply {
        if !j["input"]["params"]["token"].as_str().is_some_and(|s| package::hex(s,32))
            || !j["generation"].as_str().is_some_and(|s| package::hex(s,32))
            || after["applied"]["generation"] != j["generation"] { return Err(recovery()); }
        let a = &after["applied"];
        exact_keys(a,&["deploymentIntent","desiredSignature","profileId","enabled","generation","manifest","gameinfo","gameinfoSha256"],&["identities","directoryIdentity"])?;
        let intent = crate::deployment_intent::applied_intent(a)?.ok_or_else(recovery)?;
        if crate::deployment_intent::DeploymentIntent::capture(before,&intent.profile_id)? != intent
            || a["gameinfo"] != j["after"] || a["gameinfoSha256"] != j["afterSha256"] { return Err(recovery()); }
        let mut enabled = JsonValue::new_object();
        for m in before["profiles"][&intent.profile_id]["mods"].members() {
            enabled[string(&m["source"],"id")?] = m["enabled"].as_bool().unwrap_or(false).into();
        }
        if a["enabled"] != enabled { return Err(recovery()); }
        exact_keys(&a["manifest"],&["version","storeRoot","generation",if layout==1 {"files"} else {"packages"}],&[])?;
        let families=generation::packages(&a["manifest"])?;
        for (row,family) in a["manifest"][if layout==1 {"files"} else {"packages"}].members().zip(&families) {
            let mut exact=if layout==1 {package::encode_file(&family.directory)} else {package::encode_packages(std::slice::from_ref(family))[0].clone()};
            exact["modId"]=row["modId"].clone(); exact["modIds"]=row["modIds"].clone();
            if exact!=*row { return Err(recovery()); }
        }
        let previous = &after["previous"];
        exact_keys(previous,&["applied","activeProfileId","gameinfo","gameinfoSha256","upstreamPayloads","upstreamEvidenceVersion"],&["installedGameOrigins","installedBaselineUnavailable"])?;
        if previous["applied"] != before["applied"] || previous["activeProfileId"] != before["activeProfileId"]
            || previous["gameinfo"] != j["before"] || previous["gameinfoSha256"] != j["beforeSha256"]
            || previous["upstreamEvidenceVersion"].as_u32() != Some(layout) || !previous["upstreamPayloads"].is_array()
            || std::iter::once(&before["applied"]).chain(std::iter::once(&before["previous"]["applied"]))
                .chain(before["retiredGenerations"].members()).any(|a| !a.is_null() && a["generation"]==j["generation"]) {
            return Err(recovery());
        }
        retired_transition(&before["retiredGenerations"],&after["retiredGenerations"],&before["previous"]["applied"])?;
        projected["applied"] = a.clone(); projected["previous"] = previous.clone();
        projected["activeProfileId"] = intent.profile_id.into(); projected["restoredSetup"] = JsonValue::Null;
        projected["canRestore"] = true.into();
    } else {
        if !j["generation"].is_null() || j["workspaceExisted"] != true || before["canRestore"] != true
            || !before["previous"].is_object() || j["after"] != before["previous"]["gameinfo"]
            || j["afterSha256"] != before["previous"]["gameinfoSha256"] { return Err(recovery()); }
        retired_transition(&before["retiredGenerations"],&after["retiredGenerations"],&before["applied"])?;
        projected["applied"] = before["previous"]["applied"].clone();
        projected["activeProfileId"] = before["previous"]["activeProfileId"].clone();
        if after["restoredSetup"].is_null() && (before["previous"]["applied"].is_null() || !before["previous"]["upstreamPayloads"].is_empty()){return Err(recovery());}
        projected["restoredSetup"] = if after["restoredSetup"].is_object() {string(j,"citadel")?;admitted_restore_setup(&before["previous"],&after["restoredSetup"])?} else {JsonValue::Null};
        projected["previous"] = JsonValue::Null; projected["canRestore"] = false.into();
    }
    projected["retiredGenerations"] = after["retiredGenerations"].clone();
    if j["generation"].is_string() && projected.has_key("gameFolderChanged") {projected["gameFolderChanged"]=false.into();}
    projected["recoveryRequired"] = false.into();
    if projected != *after { return Err(recovery()); }
    for (doc,key,digest) in [(before,"before","beforeSha256"),(after,"after","afterSha256")] {
        let state=if !doc["applied"].is_null() {&doc["applied"]} else {&doc["restoredSetup"]};
        if !state.is_null() && (state["gameinfo"]!=j[key] || state["gameinfoSha256"]!=j[digest]) {
            return Err(recovery());
        }
    }
    admit_game4_staging(j)?;
    let current = store.document()?;
    let outcome = store.operation(id)?;
    if outcome["status"] == "succeeded" {
        let result=store.replay(id,&j["input"])?.ok_or_else(recovery)?;
        let mut committed = after.clone();
        committed["revision"] = rev(before)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
        if current != committed || !j.has_key("tempIdentity") || !store::matches_recorded_snapshot(&committed,&result)? { return Err(recovery()); }
    } else {
        // A failed game rollback can retain its journal after failure recording.
        // replay checks the input even for an existing failed outcome.
        if outcome["status"] == "failed" {
            let failure = store.replay(id,&j["input"]).err().ok_or_else(recovery)?;
            let expected=game_rollback_error();
            if failure.kind!=expected.kind || failure.message!=expected.message { return Err(recovery()); }
        }
        if current != *before { return Err(recovery()); }
    }
    admit_journal(j)
}

fn admit_game4_staging(j: &JsonValue) -> Result<(),NativeError> {
    if j.has_key("tempIdentity") {
        game_identity(&j["tempIdentity"])?;
        if j["tempIdentity"]["size"]!=0 { return Err(recovery()); }
    }
    if j["generation"].is_null() {
        if j.has_key("generationIdentity") || j.has_key("stagedFiles") { return Err(recovery()); }
        return Ok(());
    }
    let a = &j["document"]["applied"];
    let files = generation_files(&a["manifest"])?;
    let names: Vec<_> = files.iter().map(|f|f.name.as_str()).chain(std::iter::once("manifest.json")).collect();
    if j.has_key("generationIdentity") { game_identity(&j["generationIdentity"])?; }
    if j.has_key("stagedFiles") {
        if !j["stagedFiles"].is_array() || j["stagedFiles"].is_empty() || j["stagedFiles"].len()>names.len()
            || !j.has_key("generationIdentity") { return Err(recovery()); }
        for (i,row) in j["stagedFiles"].members().enumerate() {
            exact_keys(row,&["name","device","inode","complete"],&["fsid"])?;
            if !generation::valid_fsid(row) {return Err(recovery());}
            if row["name"]!=names[i] || row["device"].as_u64().is_none() || row["inode"].as_u64().filter(|n|*n!=0).is_none()
                || row["complete"].as_bool().is_none() || i+1<j["stagedFiles"].len() && row["complete"]!=true { return Err(recovery()); }
        }
    }
    let captured = a.has_key("identities");
    if captured {
        if !a["identities"].is_object() || a["identities"].len()!=names.len()
            || j["stagedFiles"].len()!=names.len() || j["stagedFiles"].members().any(|r|r["complete"]!=true) { return Err(recovery()); }
        for (i,name) in names.iter().enumerate() {
            let identity = &a["identities"][*name]; game_identity(identity)?;
            if identity["device"]!=j["stagedFiles"][i]["device"] || identity["inode"]!=j["stagedFiles"][i]["inode"]
                || identity["size"].as_u64()!=Some(if i<files.len() {files[i].payload.size} else {a["manifest"].dump().len() as u64}) { return Err(recovery()); }
        }
    }
    if a["manifest"]["version"]==2 && captured {
        game_identity(&a["directoryIdentity"])?;
        if a["directoryIdentity"]["device"]!=j["generationIdentity"]["device"]
            || a["directoryIdentity"]["inode"]!=j["generationIdentity"]["inode"] { return Err(recovery()); }
    } else if a.has_key("directoryIdentity") { return Err(recovery()); }
    if j.has_key("tempIdentity") && !captured { return Err(recovery()); }
    Ok(())
}

pub(crate) fn admit_game6(store: &Store, id: &str, j: &JsonValue) -> Result<(), NativeError> { shards::admit6(store,id,j) }
pub(crate) fn admit_game5(store: &Store, id: &str, j: &JsonValue) -> Result<(), NativeError> { shards::admit(store,id,j) }
fn validate_game_journal(store: &Store, id: &str, j: &JsonValue) -> Result<(), NativeError> {
    if j["version"] == 6 { return shards::admit6(store,id,j); }
    if j["version"] == 5 { return shards::admit(store,id,j); }
    generation::require_flat_document(&store.document()?)?;
    for key in ["document","beforeDocument","requestedDocument"] {generation::require_flat_document(&j[key])?;}
    if j["version"] == 4 { admit_game4(store,id,j)?; }
    let journal_version = j["version"].as_u32().filter(|v| matches!(v, 1 | 2 | 3 | 4)).ok_or_else(recovery)?;
    let library_version = package::document_version(&j["document"]).map_err(|_| recovery())?;
    let version = package::layout_version(&j["document"]).map_err(|_| recovery())?;
    if j["kind"] != "game" || library_version != journal_version
        || (journal_version >= 3 && (j["libraryVersion"].as_u32() != Some(journal_version) || j["packageVersion"].as_u32() != Some(version)))
        || (journal_version < 3 && (j.has_key("libraryVersion") || j.has_key("packageVersion")))
    {
        return Err(recovery());
    }
    if j.has_key("fontStateDigest") {
        if j["fontStateDigest"] != crate::font_transaction::state_digest(&j["document"])?
            || j["fontStateDigest"] != crate::font_transaction::state_digest(&store.document()?)? {return Err(recovery())}
    } else if crate::fonts::FontState::decode(&store.document()?)?.installed.is_some()
        || crate::fonts::FontState::decode(&j["document"])?.installed.is_some() {
        return Err(recovery());
    }
    store::revision(string(j, "expectedRevision")?)?;
    if j["workspaceExisted"].as_bool().is_none()
        || !string(j, "temp")?
            .strip_prefix(".native-gameinfo-")
            .is_some_and(|id| package::hex(id, 32))
    {
        return Err(recovery());
    }
    if citadel(Path::new(string(&j["document"], "gamePath")?))?
        .to_string_lossy()
        .as_ref()
        != string(j, "citadel")?
    {
        return Err(recovery());
    }
    for a in std::iter::once(&j["document"]["applied"])
        .chain(std::iter::once(&j["document"]["previous"]["applied"]))
        .chain(j["document"]["retiredGenerations"].members())
    {
        if a.is_null() {
            continue;
        }
        generation_files(&a["manifest"])?;
        if a["manifest"]["version"] != version
            || a["manifest"]["generation"] != a["generation"]
            || a["manifest"]["storeRoot"] != store.root().to_string_lossy().as_ref()
        {
            return Err(recovery());
        }
    }
    if !j["generation"].is_null() && j["generation"] != j["document"]["applied"]["generation"] {
        return Err(recovery());
    }
    let mut allowed = if j["generation"].is_null() {
        HashSet::new()
    } else {
        generation_files(&j["document"]["applied"]["manifest"])?
            .into_iter()
            .map(|f| f.name)
            .collect()
    };
    if !j["generation"].is_null() {
        allowed.insert("manifest.json".into());
    }
    if j.has_key("stagedFiles") && !j["stagedFiles"].is_array() {
        return Err(recovery());
    }
    for f in j["stagedFiles"].members() {
        if !allowed.remove(string(f, "name")?)
            || f["device"].as_u64().is_none()
            || f["inode"].as_u64().is_none()
            || (journal_version >= 2 && f["complete"].as_bool().is_none())
        {
            return Err(recovery());
        }
    }
    for key in ["generationIdentity", "tempIdentity"] {
        if j.has_key(key)
            && (!j[key].is_object()
                || j[key]["device"].as_u64().is_none()
                || j[key]["inode"].as_u64().is_none())
        {
            return Err(recovery());
        }
    }
    verify_journal_bytes(j)?;
    admit_journal(j)
}
fn establish_workspace(c: &Path, root: &Path, j: &JsonValue) -> Result<(), NativeError> {
    if workspace(c, root).unwrap_or(false) {
        return Ok(());
    }
    let w = c.join("deadlock_native");
    if j["workspaceExisted"] != false {
        return Err(external());
    }
    if w.exists() {
        return Err(external());
    }
    fs::create_dir(&w)?;
    paths::sync_dir(c)?;
    write_new(
        &w.join("owner.json"),
        object! {version:1,root:root.to_string_lossy().as_ref()}
            .dump()
            .as_bytes(),
    )?;
    Ok(())
}
#[cfg(test)]
thread_local! {pub(super) static BEFORE_FINAL_RENAME:std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>>=const {std::cell::RefCell::new(None)};}
fn commit_files(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &JsonValue,
    checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>,
 authority: Option<&GameLock>) -> Result<JsonValue, NativeError> {
    validate_game_journal(store, id, j)?;
    verify_fonts(store, &store.document()?,lock,authority)?;
    let mut journal = j.clone();
    let c = PathBuf::from(string(j, "citadel")?);
    let live = c.join("gameinfo.gi");
    let live_birth = identity(&paths::open_regular(&live)?.metadata()?);
    verify_journal_bytes(j)?;
    let before = string(j, "before")?;
    let after = string(j, "after")?;
    if text(&live)? != before {
        return Err(external());
    }
    checkpoint("beforeTemp")?;
    game_guard()?;
    let temp = c.join(string(j, "temp")?);
    let mut temp_file = new_file(&temp)?;
    temp_file.sync_all()?;
    paths::sync_dir(&c)?;
    let temp_birth=generation::capture_identity(&temp)?;
    if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) {
        journal["tempIdentity"] = temp_birth.clone();
        store.write_journal(lock, id, &journal)?;
    }
    temp_file.write_all(after.as_bytes())?;
    temp_file.sync_all()?;
    let permissions = paths::open_regular(&live)?.metadata()?.permissions();
    fs::set_permissions(&temp, permissions)?;
    paths::open_regular(&temp)?.sync_all()?;
    checkpoint("beforeRename")?;
    if let Err(e)=(|| -> Result<(),NativeError> {
        if j["input"]["method"]=="native.restore" {check_snapshot(store.root(),&store.document()?, authority)?;} else {check_applied(store.root(), &store.document()?, authority)?;}
        check_generation_full(&c, store.root(), &j["document"]["applied"], authority)?;
        check_installed_baselines(&j["document"]["restoredSetup"], &c, false, authority)?;
        check_upstream(&j["document"]["restoredSetup"], false, &c, authority)?;
        check_upstream(&j["document"]["previous"], false, &c, authority)?;
        Ok(())
    })() {
        game_guard()?;
        plan_rollback_staging(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(e);
    }
    if journal["tempIdentity"].is_object()
        && !generation::same_owner(
            &paths::open_regular(&temp)?.metadata()?,
            &journal["tempIdentity"],
        )
    {
        return Err(external());
    }
    if text(&live)? != before {
        return Err(external());
    }
    game_guard()?;
    verify_fonts(store, &store.document()?,lock,authority)?;
    if let Err(e)=check_installed_baselines(&journal["document"]["previous"], &c, true, authority) {
        // Still before publication: remove only journal-owned staging, preserving
        // every foreign addon file and the unchanged before image.
        game_guard()?;
        plan_rollback_staging(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(e);
    }
    #[cfg(test)]
    BEFORE_FINAL_RENAME.with(|h|if let Some(h)=h.borrow_mut().take(){h(&temp);});
    if !generation::same_owner(&paths::open_regular(&temp)?.metadata()?,&temp_birth) || text(&temp)?!=after {return Err(external());}
    check_generation(&c,store.root(),&journal["document"]["applied"],true, authority)?;
    check_generation(&c,store.root(),&journal["document"]["previous"]["applied"],true, authority)?;
    // The previous-baseline check (including the addons file set) runs below so a
    // late mismatch rolls back and clears the journal instead of returning early.
    if check_installed_baselines(&journal["document"]["restoredSetup"], &c, true, authority).is_err()
        || check_upstream(&journal["document"]["previous"], true, &c, authority).is_err()
        || check_upstream(&journal["document"]["restoredSetup"], true, &c, authority).is_err()
        || check_installed_baselines(&journal["document"]["previous"], &c, true, authority).is_err()
        || identity(&paths::open_regular(&live)?.metadata()?) != live_birth || text(&live)? != before {
        game_guard()?;
        plan_rollback_staging(store,&c,&journal, authority)?.execute()?;
        store.record_failure(lock,id,&journal["input"],&game_rollback_error())?;
        store.clear_journal(lock,id)?;
        return Err(game_files_changed());
    }
    paths::replace_admitted(&temp,&live,&journal["beforeIdentity"],journal["beforeSha256"].as_str().map(str::to_owned))?;
    paths::sync_dir(&c)?;
    checkpoint("renamed")?;
    finish(store, lock, id, &journal, checkpoint, authority)
}
fn verify_journal_bytes(j: &JsonValue) -> Result<(), NativeError> {
    for key in ["before", "after"] {
        if j[format!("{key}Sha256")].as_str()
            != Some(paths::digest_bytes(string(j, key)?.as_bytes())?.as_str())
        {
            return Err(external());
        }
    }
    Ok(())
}
fn finish(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &JsonValue,
    checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>,
 authority: Option<&GameLock>) -> Result<JsonValue, NativeError> {
    if matches!(j["version"].as_u32(),Some(5|6)) { return shards::finish5(store,lock,id,j, authority); }
    validate_game_journal(store, id, j)?;
    verify_fonts(store, &store.document()?,lock,authority)?;
    let c = PathBuf::from(string(j, "citadel")?);
    verify_journal_bytes(j)?;
    if text(&c.join("gameinfo.gi"))? != string(j, "after")? {
        return Err(external());
    }
    if j["tempIdentity"].is_object() && !generation::owner_matches(&c.join("gameinfo.gi"),&j["tempIdentity"],authority)? {
        return Err(external());
    }
    check_state(store.root(), &j["document"],j["generation"].is_null(),true, authority)?;
    check_generation_full(&c, store.root(), &j["document"]["previous"]["applied"], authority)?;
    if store.operation(id)?["status"]!="succeeded" {check_rollback_upstream(&j["document"]["previous"],&c, authority)?;}
    checkpoint("beforeDatabase")?;
    let result = store.commit(
        lock,
        store::revision(string(j, "expectedRevision")?)?,
        id,
        &j["input"],
        j["document"].clone(),
        None,
    )?;
    checkpoint("database")?;
    prune_retired(store, &c, &j["document"], authority);
    cleanup_restored_workspace(store,&c,j,checkpoint, authority)?;
    store.clear_journal(lock, id)?;
    Ok(result)
}
pub fn restore(
    store: &Store,
    lock: &RootLock,
    expected: u64,
    id: &str,
    input: &JsonValue,
) -> Result<JsonValue, NativeError> {
    if let Some(result) = store.replay(id,input)? { return Ok(result); }
    refuse_previous_folder(&store.document()?)?;
    if store.journals()?.is_empty() && check_external(store.root(),&store.document()?, None)? { return Err(external_gameinfo_error()); }
    if crate::deployment_metadata::retained_shards(&store.document()?) {
        return shards::restore(store, lock, expected, id, input);
    }
    restore_with(store, lock, expected, id, input, &mut |_| Ok(()))
}
fn restore_with(
    store: &Store,
    lock: &RootLock,
    expected: u64,
    id: &str,
    input: &JsonValue,
    checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>,
) -> Result<JsonValue, NativeError> {
    if let Some(result) = store.replay(id,input)? { return Ok(result); }
    store::ensure_readable(&store.document()?)?;
    ensure_idle(store)?;
    generation::require_flat_document(&store.document()?)?;
    let mut doc = store.document()?;
    if rev(&doc)? != expected {
        return Err(err("staleRevision", "Library changed before Restore."));
    }
    if doc["canRestore"] != true || !doc["previous"].is_object() {
        return Err(err(
            "nothingToRestore",
            "There is no verified previous setup to restore.",
        ));
    }
    let c = citadel(Path::new(string(&doc, "gamePath")?))?;
    let _game_lock = GameLock::acquire(&c)?;
    game_guard()?;
    verify_fonts(store, &doc,lock,Some(&_game_lock))?;
    check_applied(store.root(), &doc, Some(&_game_lock))?;
    check_generation(&c, store.root(), &doc["previous"]["applied"],true, Some(&_game_lock))?;
    check_installed_baselines(&doc["previous"], &c, true, Some(&_game_lock))?;
    check_upstream(&doc["previous"], true, &c, Some(&_game_lock))?;
    let before = text(&c.join("gameinfo.gi"))?;
    let after = string(&doc["previous"], "gameinfo")?.to_owned();
    if doc["previous"]["gameinfoSha256"].as_str()
        != Some(paths::digest_bytes(after.as_bytes())?.as_str())
    {
        return Err(external());
    }
    let old_applied = doc["applied"].clone();
    retire(&mut doc, old_applied, &c)?;
    doc["restoredSetup"] = if doc["previous"].is_object() {
        doc["previous"].clone()
    } else {
        JsonValue::Null
    };
    doc["applied"] = doc["previous"]["applied"].clone();
    doc["activeProfileId"] = doc["previous"]["activeProfileId"].clone();
    doc["previous"] = JsonValue::Null;
    doc["canRestore"] = false.into();
    doc["recoveryRequired"] = false.into();
    let j = prepare(
        store, lock, id, input, expected, &c, &before, &after, doc, true,
    )?;
    checkpoint("journal").map_err(|e| recovery_with_cause("Restore",e))?;
    match commit_files(store, lock, id, &j, checkpoint, Some(&_game_lock)) {
        Ok(result)=>Ok(result),
        Err(e)=>{
            if store.journal(id)?.is_none() {return Err(e);}
            // A second idle scan can fail transiently after the backup is
            // journaled. With no temp or store mutation there is no filesystem
            // work to recover; preserve the guard refusal and allow retry.
            if matches!(e.kind.as_str(),"gameGuardUnavailable"|"gameRunning")
                && !path_exists(&c.join(string(&j,"temp")?))?
                && text(&c.join("gameinfo.gi"))?==before
                && rev(&store.document()?)?==expected
                && store.journal(id)?.as_ref()==Some(&j) {
                store.clear_journal(lock,id)?;
                return Err(e);
            }
            Err(recovery_with_cause("Restore",e))
        }
    }
}
fn retire(doc: &mut JsonValue, old: JsonValue, c: &Path) -> Result<(), NativeError> {
    let mut retained = JsonValue::new_array();
    for a in doc["retiredGenerations"].members() {
        if generation_dir(c, a)?.try_exists()? {
            retained
                .push(a.clone())
                .map_err(|_| NativeError::params())?;
        }
    }
    if !old.is_null() {
        retained.push(old).map_err(|_| NativeError::params())?;
    }
    doc["retiredGenerations"] = retained;
    Ok(())
}
/// Best-effort bounded retention after the new setup is durably committed. The
/// document keeps exact ownership metadata across crashes or incomplete cleanup.
#[cfg(test)]
thread_local! {static BEFORE_FLAT_UNLINK:std::cell::RefCell<Option<Box<dyn FnMut(&Path)>>>=std::cell::RefCell::new(None);}
pub(super) fn retirement_owned(a:&JsonValue)->Result<bool,NativeError>{
    Ok(a["manifest"]["version"]!=1 && a["directoryIdentity"].is_object()
        && generation_files(&a["manifest"])?.iter().map(|f|f.name.as_str()).chain(std::iter::once("manifest.json")).all(|n|a["identities"][n]["device"].is_number() && a["identities"][n]["inode"].is_number()))
}
fn prune_retired(store: &Store, c: &Path, doc: &JsonValue, authority: Option<&GameLock>) {
    for a in doc["retiredGenerations"].members() {
        if a["generation"] == doc["applied"]["generation"]
            || a["generation"] == doc["previous"]["applied"]["generation"]
        {
            continue;
        }
        let _ = (|| -> Result<(), NativeError> {
            workspace(c, store.root())?;
            let dir = generation_dir(c, a)?;
            if !path_exists(&dir)? {
                return Ok(());
            }
            paths::reject_symlinks(&dir)?;
            if !retirement_owned(a)? {return Ok(());}
            check_generation(&c, store.root(), a,true, None)?;
            let mut known = HashMap::new();
            for file in generation_files(&a["manifest"])? {
                known.insert(file.name, package::encode_file(&file.payload));
            }
            let mut remove = Vec::new();
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "manifest.json" {
                    if paths::read_bounded(&path, paths::MAX_JSON_BYTES)?
                        != a["manifest"].dump().as_bytes()
                    {
                        return Err(external());
                    }
                } else if let Some(p) = known.get(&name) {
                    verify_file(&path, p)?;
                } else {
                    return Err(external());
                }
                remove.push(path);
            }
            let mut names=remove.iter().filter_map(|p|p.file_name().map(|s|s.to_owned())).collect::<HashSet<_>>();
            for p in remove {
                #[cfg(test)]
                BEFORE_FLAT_UNLINK.with(|h|if let Some(h)=h.borrow_mut().as_mut(){h(&p);});
                let actual=fs::read_dir(&dir)?.map(|e|e.map(|e|e.file_name())).collect::<Result<HashSet<_>,_>>()?;
                if actual!=names || !generation::owner_matches(&dir,&a["directoryIdentity"],authority)?
                    || !generation::metadata_matches(&p,&a["identities"][p.file_name().unwrap().to_str().ok_or_else(external)?],authority)? {return Err(external());}
                let name=p.file_name().unwrap().to_str().ok_or_else(external)?;
                let sha=if name=="manifest.json"{paths::digest_bytes(a["manifest"].dump().as_bytes())?}else{string(&known[name],"sha256")?.to_owned()};
                if !paths::DestructiveFile::admitted(p.clone(),&a["identities"][name],Some(sha))?.unlink()?{return Ok(());}
                names.remove(p.file_name().unwrap());
            }
            fs::remove_dir(&dir)?;
            paths::sync_dir(dir.parent().unwrap())
        })();
    }
}
/// The terminal Restore journal retains the directory identity until cleanup
/// completes, including a crash after owner unlink but before directory removal.
fn cleanup_restored_workspace(store:&Store,c:&Path,j:&JsonValue,checkpoint:&mut impl FnMut(&str)->Result<(),NativeError>, authority: Option<&GameLock>)->Result<(),NativeError> {
    let doc=&j["document"];
    if !j["generation"].is_null() || !doc["applied"].is_null() || !doc["previous"]["applied"].is_null()
        || !j["workspaceIdentity"].is_object() {return Ok(());}
    let w=c.join("deadlock_native");
    if !path_exists(&w)? {return Ok(());}
    if !generation::owner_matches(&w,&j["workspaceIdentity"],authority)? {return Err(external());}
    let names=fs::read_dir(&w)?.map(|e|e.map(|e|e.file_name())).collect::<Result<Vec<_>,_>>()?;
    if names.is_empty() {fs::remove_dir(&w)?;return paths::sync_dir(c);}
    if names.len()!=1 || names[0]!="owner.json" {return Ok(());}
    workspace(c,store.root())?;
    let marker=paths::DestructiveFile::admitted(w.join("owner.json"),&j["workspaceIdentity"],Some(paths::digest_bytes(object!{version:1,root:store.root().to_string_lossy().as_ref()}.dump().as_bytes())?))?;
    if !marker.unlink()? {return Ok(());}
    paths::sync_dir(&w)?;
    checkpoint("workspaceMarkerRemoved")?;
    fs::remove_dir(&w)?;
    paths::sync_dir(c)
}
/// Reclaim only the uncommitted paths recorded in this operation. Validate the
/// entire set first so external files or changed bytes preserve all evidence.
fn path_exists(path: &Path) -> Result<bool, NativeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            paths::reject_symlinks(path)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(path)?;
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}
enum GameRollback {
    Flat(RollbackCleanup),
    Shards(shards::Cleanup),
}
impl GameRollback {
    fn execute(self) -> Result<(),NativeError> {
        match self { Self::Flat(plan)=>plan.execute(), Self::Shards(plan)=>plan.execute().map(|_|()) }
    }
}
struct RollbackCleanup {
    remove: Vec<paths::DestructiveFile>,
    marker: Option<paths::DestructiveFile>,
    directory: Option<PathBuf>,
    workspace: PathBuf,
    remove_empty_workspace: bool,
    citadel: PathBuf,
}
impl RollbackCleanup {
    fn execute(self) -> Result<(), NativeError> {
        let mut retained=Vec::new();
        for file in self.remove {
            if !file.unlink()? {retained.push(file.path);}
        }
        if let Some(dir) = self.directory {
            if !retained.iter().any(|p|p.starts_with(&dir)) {
                fs::remove_dir(&dir)?;
                paths::sync_dir(&self.workspace)?;
            }
        }
        if self.remove_empty_workspace && !retained.iter().any(|p|p.starts_with(&self.workspace)) {
            paths::reject_symlinks(&self.workspace)?;
            let removed=if let Some(marker)=self.marker {marker.unlink()?}else{true};
            if removed {fs::remove_dir(&self.workspace)?;}
        }
        paths::sync_dir(&self.citadel)
    }
}
fn plan_rollback_staging(
    store: &Store,
    c: &Path,
    j: &JsonValue,
 authority: Option<&GameLock>) -> Result<RollbackCleanup, NativeError> {
    let temp_name = string(j, "temp")?;
    let suffix = temp_name
        .strip_prefix(".native-gameinfo-")
        .ok_or_else(external)?;
    if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(external());
    }
    let temp = c.join(temp_name);
    let mut remove = Vec::new();
    let mut directory = None;
    let mut marker = None;
    let w=c.join("deadlock_native");
    let owned_new_workspace=j["workspaceExisted"]==false && j["workspaceIdentity"].is_object()
        && path_exists(&w)? && generation::owner_matches(&w,&j["workspaceIdentity"],authority)?;
    if path_exists(&temp)? {
        if j["tempIdentity"].is_object() {
            if !generation::owner_matches(&temp,&j["tempIdentity"],authority)?
            {
                return Err(external());
            }
        } else if (matches!(j["version"].as_u32(), Some(2 | 3 | 4))) || text(&temp)? != string(j, "after")? {
            return Err(external());
        }
        remove.push(paths::DestructiveFile::admitted(temp,&j["tempIdentity"],Some(paths::digest_bytes(string(j,"after")?.as_bytes())?))?);
    }
    if !j["generation"].is_null() {
        let a = &j["document"]["applied"];
        let dir = generation_dir(c, a)?;
        if path_exists(&dir)? {
            paths::reject_symlinks(&dir)?;
            if j["generationIdentity"].is_object() {
                if !generation::owner_matches(&dir,&j["generationIdentity"],authority)? {
                    return Err(external());
                }
            } else if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) {
                // The durable journal already names this unpredictable generation,
                // checked absent before journaling. Before identity publication,
                // only an empty directory in our owned workspace is reclaimable.
                if !owned_new_workspace {workspace(c,store.root())?;}
                if j.has_key("stagedFiles") || fs::read_dir(&dir)?.next().is_some() {
                    return Err(external());
                }
            }
            let mut allowed = HashMap::new();
            for file in generation_files(&a["manifest"])? {
                allowed.insert(file.name, package::encode_file(&file.payload));
            }
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                let recorded = j["stagedFiles"]
                    .members()
                    .find(|entry| entry["name"].as_str() == Some(name.as_str()));
                if name != "manifest.json" && !allowed.contains_key(&name) {
                    return Err(external());
                }
                let metadata = paths::open_regular(&path)?.metadata()?;
                if let Some(recorded) = recorded {
                    if !generation::owner_matches(&path,recorded,authority)? {
                        return Err(external());
                    }
                } else if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) {
                    return Err(external());
                }
                let complete = recorded.is_none_or(|r| r["complete"] == true);
                if complete {
                    if name == "manifest.json" {
                        if paths::read_bounded(&path, paths::MAX_JSON_BYTES)?
                            != a["manifest"].dump().as_bytes()
                        {
                            return Err(external());
                        }
                    } else {
                        verify_file(&path, &allowed[&name])?;
                    }
                }
                let hash=if name=="manifest.json" {paths::digest_bytes(a["manifest"].dump().as_bytes())?}else{string(&allowed[&name],"sha256")?.to_owned()};
                remove.push(paths::DestructiveFile::admitted(path,recorded.unwrap_or(&JsonValue::Null),Some(hash))?);
            }
            directory = Some(dir);
        }
    }
    // A recorded directory identity also covers interrupted workspace cleanup.
    let mut remove_empty_workspace = j["version"] == 1
        && j["workspaceExisted"] == false && w.try_exists()?
        && fs::read_dir(&w)?.next().is_none();
    if owned_new_workspace {
        for entry in fs::read_dir(&w)? {
            let path=entry?.path();
            if Some(&path)==directory.as_ref() {continue;}
            if path!=w.join("owner.json") {return Err(external());}
            workspace(c,store.root())?;
            marker=Some(paths::DestructiveFile::admitted(path,&j["workspaceIdentity"],Some(paths::digest_bytes(object!{version:1,root:store.root().to_string_lossy().as_ref()}.dump().as_bytes())?))?);
        }
        remove_empty_workspace=true;
    } else if w.try_exists()? && !remove_empty_workspace {
        workspace(c, store.root())?;
    }
    Ok(RollbackCleanup {
        remove,
        marker,
        directory,
        workspace: w,
        remove_empty_workspace,
        citadel: c.to_path_buf(),
    })
}

/// Recovery4 retains its flat/current-revision domain. Recovery5 import-only
/// adds Store4 retained3 and preserves the same exact current-revision contract.
pub(crate) fn admit_import_recovery(store: &Store, j: &JsonValue) -> Result<(),NativeError> {
    let fields=["kind","version","mode","input","expectedRevision","libraryVersion","packageVersion"];
    if !j.is_object() || j.len()!=fields.len() || fields.iter().any(|k|!j.has_key(k))
        || j["kind"]!="recovery" || !matches!(j["version"].as_u32(),Some(4|5)) || j["mode"]!="localImportOnly"
        || !j["input"].is_object() {return Err(recovery());}
    store::revision(string(j,"expectedRevision")?)?;
    let doc=store.document()?;
    crate::deployment_metadata::domain(&doc,j["version"]==5)?;
    if j["libraryVersion"].as_u32()!=Some(package::document_version(&doc)?)
        || j["packageVersion"].as_u32()!=Some(package::layout_version(&doc)?)
        || store::revision(string(j,"expectedRevision")?)?!=rev(&doc)? {return Err(recovery());}
    Ok(())
}

/// Recovery6 exists only for a captured game6 target. Once the main journal is
/// gone, its exact durable terminal outcome must still describe the live Store.
fn admit_vanilla_recovery(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    exact_keys(j,&["kind","version","mode","input","expectedRevision","libraryVersion","packageVersion",
        "mainOperationId","mainInput","beforeDocumentSha256","afterDocumentSha256","gameinfo"],&[])?;
    if j["kind"]!="recovery" || j["version"]!=6 || j["mode"]!="general" {return Err(recovery());}
    operation_binding(id,&j["input"],&j["expectedRevision"],"native.recover")?;
    exact_keys(&j["input"]["params"],&["operationId","expectedRevision"],&[])?;
    let gi=&j["gameinfo"];
    exact_keys(gi,&["beforeSha256","beforeIdentity","afterSha256","afterIdentity"],&[])?;
    game_identity(&gi["beforeIdentity"])?;
    if !package::hex(string(gi,"beforeSha256")?,64) || !package::hex(string(gi,"afterSha256")?,64) {return Err(recovery());}
    if !gi["afterIdentity"].is_null() {
        game_identity(&gi["afterIdentity"])?;
        if gi["afterIdentity"]["size"]!=0 {return Err(recovery());}
    }
    let main_id=string(j,"mainOperationId")?;
    if main_id==id {return Err(recovery());}
    let main_input=&j["mainInput"];
    admit_vanilla_input(main_id,main_input,&main_input["params"]["expectedRevision"])?;
    let start=store::revision(string(&main_input["params"],"expectedRevision")?)?;
    let next=start.checked_add(1).ok_or_else(recovery)?;
    let expected=store::revision(string(j,"expectedRevision")?)?;
    let doc=store.document()?;
    let current=rev(&doc)?;
    let observed=j["libraryVersion"].as_u32().filter(|v|matches!(v,1|2|3|4)).ok_or_else(recovery)?;
    if !j["afterDocumentSha256"].is_null() && !j["afterDocumentSha256"].as_str().is_some_and(|s|package::hex(s,64)) {return Err(recovery());}
    if !package::hex(string(j,"beforeDocumentSha256")?,64)
        || ![start,next].contains(&expected) || ![expected,next].contains(&current)
        || j["packageVersion"].as_u32()!=Some(package::layout_version(&doc)?)
        || observed<3 && j["packageVersion"].as_u32()!=Some(observed)
        || current==expected && package::document_version(&doc)?!=observed
        || current==next && doc["storeVersion"]!=4 {return Err(recovery());}
    crate::deployment_metadata::admit(store.root(),&doc)?;
    store::ensure_readable(&doc)?;
    if let Some(main)=store.journal(main_id)? {
        admit_game6(store,main_id,&main)?;
        if j["gameinfo"]!=vanilla_recovery_gameinfo(&main)
            || j["afterDocumentSha256"]!=shards::recovery_after_digest(store,&main)?
            || main["input"]!=*main_input || j["beforeDocumentSha256"]!=paths::digest_bytes(store::canonical(&main["beforeDocument"]).as_bytes())?
            || expected==start && j["libraryVersion"]!=main["libraryVersionBefore"]
            || expected==next && j["libraryVersion"]!=main["libraryVersionAfter"] {return Err(recovery());}
    } else {
        match store.operation(main_id)?["status"].as_str() {
            Some("succeeded") => {
                if gi["afterIdentity"].is_null() || gi["afterSha256"]!=doc["applied"]["gameinfoSha256"]
                    || j["afterDocumentSha256"]!=paths::digest_bytes(store::canonical(&doc).as_bytes())?
                    || current!=next || doc["applied"]["manifest"]["version"]!=3
                    || crate::deployment_intent::applied_intent(&doc["applied"])?!=Some(crate::deployment_intent::DeploymentIntent::empty_active(&doc)?)
                    || !store::matches_recorded_snapshot(&doc,&store.replay(main_id,main_input)?.ok_or_else(recovery)?)? {return Err(recovery());}
            }
            Some("failed") => {
                let failure=store.replay(main_id,main_input).err().ok_or_else(recovery)?;
                let rollback=game_rollback_error();
                if current!=start || expected!=start || failure.kind!=rollback.kind || failure.message!=rollback.message
                    || j["beforeDocumentSha256"]!=paths::digest_bytes(store::canonical(&doc).as_bytes())? {return Err(recovery());}
            }
            _=>return Err(recovery()),
        }
    }
    store::ensure_internal(j)
}
fn vanilla_recovery_gameinfo(main:&JsonValue) -> JsonValue {
    object!{beforeSha256:main["beforeSha256"].clone(),beforeIdentity:main["beforeIdentity"].clone(),
        afterSha256:main["afterSha256"].clone(),afterIdentity:main["tempIdentity"].clone()}
}
/// Called only after acquisition of the recovery target's exact GameLock.
fn verify_vanilla_recovery_gameinfo(store:&Store,j:&JsonValue, authority: Option<&GameLock>) -> Result<(),NativeError> {
    let doc=store.document()?;
    let path=citadel(Path::new(string(&doc,"gamePath")?))?.join("gameinfo.gi");
    let gi=&j["gameinfo"];
    let meta=paths::open_regular(&path)?.metadata()?;
    let succeeded=store.operation(string(j,"mainOperationId")?)?["status"]=="succeeded";
    let digest=if succeeded {
        if gi["afterIdentity"].is_null() || !generation::owner_matches(&path,&gi["afterIdentity"],authority)? {return Err(external());}
        &gi["afterSha256"]
    } else {
        if !generation::metadata_matches(&path,&gi["beforeIdentity"],authority)? {return Err(external());}
        &gi["beforeSha256"]
    };
    if *digest!=paths::digest_bytes(text(&path)?.as_bytes())? {return Err(external());}
    Ok(())
}
fn admit_vanilla_input(id: &str, input: &JsonValue, expected: &JsonValue) -> Result<(),NativeError> {
    operation_binding(id,input,expected,"native.vanilla")?;
    exact_keys(&input["params"],&["operationId","expectedRevision","token","deploymentLayout"],&[])?;
    if input["params"]["deploymentLayout"]!="shards-v1"
        || !input["params"]["token"].as_str().is_some_and(|s|package::hex(s,32)) {return Err(recovery());}
    Ok(())
}
/// A general attempt grants no filesystem or replacement-document authority.
pub(crate) fn admit_general_recovery(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    if j["version"]==6 { return admit_vanilla_recovery(store,id,j); }
    exact_keys(j,&["kind","version","mode","input","expectedRevision","libraryVersion","packageVersion"],&[])?;
    let outer=j["version"].as_u32().filter(|v|matches!(v,4|5)).ok_or_else(recovery)?;
    if j["kind"]!="recovery" || j["mode"]!="general" {return Err(recovery());}
    operation_binding(id,&j["input"],&j["expectedRevision"],"native.recover")?;
    let doc=store.document()?;
    let expected=store::revision(string(j,"expectedRevision")?)?;
    let current=rev(&doc)?;
    let observed=j["libraryVersion"].as_u32().filter(|v|matches!(v,1|2|3|4)).ok_or_else(recovery)?;
    let live=package::document_version(&doc)?;
    if j["packageVersion"].as_u32()!=Some(package::layout_version(&doc)?)
        || current!=expected && expected.checked_add(1)!=Some(current) {return Err(recovery());}
    if outer==4 {
        if observed!=4 || live!=4 {return Err(recovery());}
        admit_game_document(store.root(),&doc)?;
    } else {
        crate::deployment_metadata::admit(store.root(),&doc)?;
        // General5 may outlive a first game5 migration's main journal. At e,
        // rollback preserves the observed library format; at e+1 only the
        // proven Store4 generation3 result can migrate an older format.
        if observed<3 && j["packageVersion"].as_u32()!=Some(observed) {return Err(recovery());}
        if current==expected {
            if live!=observed {return Err(recovery());}
        } else if live!=4 || observed<4 && doc["applied"]["manifest"]["version"]!=3 {
            return Err(recovery());
        }
    }
    store::ensure_internal(j)
}

pub(crate) fn admit_metadata_recovery(store: &Store, id: &str, j: &JsonValue) -> Result<(),NativeError> {
    match j["mode"].as_str() {
        Some("localImportOnly")=>{
            if j["version"]==5 {operation_binding(id,&j["input"],&j["expectedRevision"],"native.recover")?;}
            admit_import_recovery(store,j)
        },
        Some("general")=>admit_general_recovery(store,id,j),
        _=>Err(recovery()),
    }
}

pub fn recover(
    store: &Store,
    lock: &RootLock,
    expected: u64,
    id: &str,
    input: &JsonValue,
) -> Result<JsonValue, NativeError> {
    if store.journals()?.members().any(|e|e["body"]["kind"]=="maintenance") {
        return Err(NativeError::new("maintenanceResumeRequired", "Open Settings, then Maintenance, and resume the recorded cleanup."));
    }
    if let Some(result) = store.replay(id, input)? {
        return Ok(result);
    }
    if store.journals()?.members().any(|e| e["body"]["kind"]=="launchHandoff") {
        return Err(crate::launch_record::barrier());
    }
    let remote_journals=store.journals()?;
    if remote_journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])) {
        // A remote journal may have appeared after the service's read-only
        // classification. Retry through its dedicated asynchronous owner.
        return Err(NativeError::new("outcomeUnknown","Remote recovery must be admitted by its worker; retry the same operation."));
    }
    if store
        .document()
        .is_err_and(|e| e.kind == "nativeNotImported")
    {
        if expected != 0 {
            return Err(err(
                "staleRevision",
                "Initial import recovery requires revision 0.",
            ));
        }
        let journals=store.journals()?;
        crate::font_transaction::admit_set(store,&journals)?;
        if journals.members().any(|e|e["body"]["kind"]!="import"){return Err(recovery())}
        crate::import::reconcile_imports(store, lock)?;
        return Err(NativeError::not_imported());
    }
    let initial = store.document()?;
    if store.journals()?.is_empty() && check_external(store.root(),&initial, None)? { return Err(external_gameinfo_error()); }
    store::ensure_readable(&initial)?;
    let existing = store.journal(id)?;
    if existing.is_none() && rev(&initial)? != expected {
        return Err(err(
            "staleRevision",
            "Library changed before recovery; refresh first.",
        ));
    }
    let journals = store.journals()?;
    crate::font_transaction::admit_set(store,&journals)?;
    let has_game = journals.members().any(|e| e["body"]["kind"] == "game" || e["body"]["kind"] == "gameFonts");
    let import_only = journals.members().any(|e| {
        e["body"]["kind"] == "import" && matches!(e["body"]["version"].as_u32(),Some(3|4|5|6|7))
            || e["body"]["kind"] == "recovery" && e["body"]["mode"] == "localImportOnly"
    }) && !has_game;
    let vanilla_target = if let Some(main)=journals.members().find(|e|e["body"]["kind"]=="game" && e["body"]["version"]==6) {
        Some(object!{mainOperationId:main["id"].clone(),mainInput:main["body"]["input"].clone(),
            beforeDocumentSha256:paths::digest_bytes(store::canonical(&main["body"]["beforeDocument"]).as_bytes())?,
            afterDocumentSha256:shards::recovery_after_digest(store,&main["body"])?,gameinfo:vanilla_recovery_gameinfo(&main["body"])})
    } else { journals.members().find(|e|e["body"]["kind"]=="recovery" && e["body"]["version"]==6)
        .map(|e|object!{mainOperationId:e["body"]["mainOperationId"].clone(),mainInput:e["body"]["mainInput"].clone(),
            beforeDocumentSha256:e["body"]["beforeDocumentSha256"].clone(),afterDocumentSha256:e["body"]["afterDocumentSha256"].clone(),gameinfo:e["body"]["gameinfo"].clone()}) };
    let shard_recovery=crate::deployment_metadata::retained_shards(&initial) || journals.members().any(|e|
        matches!((e["body"]["kind"].as_str(),e["body"]["version"].as_u32()),
            (Some("import"),Some(7))|(Some("gameFonts"),Some(4))|(Some("game"),Some(5|6))|(Some("recovery"),Some(5|6))));
    let modern_recovery = shard_recovery || initial["storeVersion"]==4 || journals.members().any(|e|
        e["body"]["kind"]=="import" && matches!(e["body"]["version"].as_u32(),Some(6|7)) || e["body"]["kind"]=="recovery" && matches!(e["body"]["version"].as_u32(),Some(4|5|6)));
    let general_recovery = (shard_recovery || initial["storeVersion"]==4) && (!import_only || has_game
        || journals.members().any(|e|e["body"]["kind"]=="recovery" && e["body"]["mode"]=="general"));
    if modern_recovery && !general_recovery && (!import_only || has_game) {return Err(recovery());}
    if existing.is_none() && journals.len()>=1024 { return Err(recovery()); }
    let mut game_paths = std::collections::BTreeSet::new();
    if general_recovery && (vanilla_target.is_some() || !initial["applied"].is_null() || initial["restoredSetup"].is_object()
        || crate::fonts::FontState::decode(&initial)?.installed.is_some()) {
        game_paths.insert(citadel(Path::new(string(&initial,"gamePath")?))?);
    }
    for entry in journals.members() {
        let j = &entry["body"];
        match j["kind"].as_str() {
            Some("game") => {
                validate_game_journal(store, string(entry,"id")?, j)?;
                let c = citadel(Path::new(string(&j["document"], "gamePath")?))?;
                if c.to_string_lossy().as_ref() != string(j, "citadel")? {
                    return Err(external());
                }
                game_paths.insert(c);
            }
            Some("gameFonts") => {
                crate::font_transaction::admit(store,j)?;
                game_paths.insert(citadel(Path::new(string(j,"game")?))?);
            }
            Some("recovery") => {
                if matches!(j["version"].as_u32(),Some(4|5|6)) {admit_metadata_recovery(store,string(entry,"id")?,j)?;}
                else if initial["storeVersion"]==4 || (j["version"] != initial["storeVersion"] && !(import_only && j["version"] == 1))
                    || !j["input"].is_object()
                {
                    return Err(recovery());
                }
                store::revision(string(j, "expectedRevision")?)?;
                if !matches!(j["version"].as_u32(),Some(4|5|6)) && j.has_key("mode") && (j["mode"] != "localImportOnly" || has_game) {
                    return Err(recovery());
                }
            }
            Some("import") => {
                if !j["version"].is_null()
                    && j["version"] != 1
                    && j["version"] != 2
                    && j["version"] != 3
                    && j["version"] != 4
                    && j["version"] != 5
                    && j["version"] != 6
                    && j["version"] != 7
                {
                    return Err(recovery());
                }
            }
            _ => return Err(recovery()),
        }
    }
    if let Some(j) = &existing {
        if j["kind"] != "recovery"
            || store::canonical(&j["input"]) != store::canonical(input)
            || store::revision(string(j, "expectedRevision")?)? != expected
        {
            return Err(err(
                "operationIdConflict",
                "Recovery operation ID belongs to different inputs.",
            ));
        }
    }
    let _game_locks = game_paths
        .iter()
        .map(|c| GameLock::acquire(c))
        .collect::<Result<Vec<_>, _>>()?;
    let main_c=initial["gamePath"].as_str().map(|p|Path::new(p).join("game/citadel"));
    let authority=main_c.as_ref().and_then(|c|_game_locks.iter().find(|g|g.validate(c).is_ok()));
    game_guard()?;
    // Complete ownership/live-state preflight before the first journal, outcome or unlink.
    // Current generation3 uses the same complete tree proof as game5 completion.
    if general_recovery && shard_recovery && !journals.members().any(|e|e["body"]["kind"]=="game") {
        check_base(store.root(),&initial, authority)?;
    }
    let imports = crate::import::preflight_imports(store,lock)?;
    let mut claims = imports.claims.clone();
    let fonts = crate::font_transaction::preflight(store,&journals,&mut claims,lock,&_game_locks).map_err(|_|recovery())?;
    let mut games = Vec::new();
    for entry in journals.members().filter(|e| e["body"]["kind"] == "game") {
        let original_id = string(entry, "id")?;
        let j = &entry["body"];
        let c = PathBuf::from(string(j, "citadel")?);
        let authority=Some(_game_locks.iter().find(|g|g.validate(&c).is_ok()).ok_or_else(recovery)?);
        if original_id == id {
            return Err(err(
                "operationIdConflict",
                "Recovery needs its own operation ID.",
            ));
        }
        verify_journal_bytes(j)?;
        verify_fonts(store,&store.document()?,lock,authority)?;
        let mut owned = if matches!(j["version"].as_u32(),Some(5|6)) {shards::recovery_claims(&c,j)?}
            else {vec![c.join("gameinfo.gi"), c.join(string(j, "temp")?)]};
        if !matches!(j["version"].as_u32(),Some(5|6)) && !j["generation"].is_null() {
            let dir = generation_dir(&c, &j["document"]["applied"])?;
            owned.push(dir.join("manifest.json"));
            for file in generation_files(&j["document"]["applied"]["manifest"])? {
                owned.push(dir.join(file.name));
            }
            owned.push(dir);
        }
        for path in owned {
            if !claims.insert(path) {
                return Err(recovery());
            }
        }
        let live = text(&c.join("gameinfo.gi"))?;
        let rollback = if matches!(j["version"].as_u32(),Some(5|6)) {
            if live==string(j,"after")? {
                if store.operation(original_id)?["status"]=="failed" {return Err(external());}
                shards::forward_preflight(store,&c,j, authority)?;
                None
            } else if live==string(j,"before")? {
                if store.operation(original_id)?["status"]=="succeeded" {return Err(external());}
                Some(GameRollback::Shards(shards::rollback_live(store,&c,j, authority)?))
            } else {return Err(external());}
        } else if live == string(j, "after")? {
            if j["tempIdentity"].is_object() && (store.operation(original_id)?["status"]=="failed"
                || !generation::owner_matches(&c.join("gameinfo.gi"),&j["tempIdentity"],authority)?) {
                return Err(external());
            }
            check_applied(store.root(), &j["document"], authority)?;
            check_generation_full(&c, store.root(), &j["document"]["previous"]["applied"], authority)?;
            if store.operation(original_id)?["status"]!="succeeded" {check_rollback_upstream(&j["document"]["previous"],&c, authority)?;}
            None
        } else if live == string(j, "before")? {
            if store.operation(original_id)?["status"] == "succeeded" {
                return Err(external());
            }
            check_generation_full(&c, store.root(), &store.document()?["applied"], authority)?;
            check_rollback_upstream(&j["document"]["previous"],&c, authority)?;
            Some(GameRollback::Flat(plan_rollback_staging(store, &c, j, authority)?))
        } else {
            return Err(external());
        };
        games.push((original_id.to_owned(), j.clone(), rollback));
    }
    // Reserve every actual terminal root and complete durable envelope before
    // any cleanup. Main journals alone remain filesystem authority.
    imports.reserve_outcomes()?;
    let mut terminal_documents=vec![initial.clone()];
    for plan in &fonts { terminal_documents.push(plan.reserve_terminal()?); }
    for (main_id,j,rollback) in &games {
        if rollback.is_some() {
            store::admit_outcome(main_id,&j["input"],&store::failure_outcome(&game_rollback_error()))?;
        } else {
            let doc=reserved_game_document(j)?;
            store::admit_success(main_id,&j["input"],&doc)?;
            terminal_documents.push(doc);
            if matches!(j["version"].as_u32(),Some(5|6)) {
                let c=PathBuf::from(string(j,"citadel")?);
                let authority=Some(_game_locks.iter().find(|g|g.validate(&c).is_ok()).ok_or_else(recovery)?);
                terminal_documents.push(shards::reserve_retirement(store,&c,j, authority)?);
            }
        }
    }
    if modern_recovery {
        for doc in &terminal_documents {
            for entry in journals.members().filter(|e|e["body"]["kind"]=="recovery") {
                store::admit_success(string(entry,"id")?,&entry["body"]["input"],doc)?;
            }
            store::admit_success(id,input,doc)?;
        }
    }
    if existing.is_none() {
        let mut attempt = object! {kind:"recovery",version:initial["storeVersion"].clone(),input:input.clone(),expectedRevision:expected.to_string()};
        if import_only && !general_recovery {
            attempt["mode"] = "localImportOnly".into();
        }
        if modern_recovery {
            attempt["version"]=if vanilla_target.is_some() {6.into()} else if shard_recovery {5.into()} else {4.into()};
            if let Some(target)=vanilla_target {
                for (key,value) in target.entries() { attempt[key]=value.clone(); }
            }
            attempt["libraryVersion"]=package::document_version(&initial)?.into();
            attempt["packageVersion"]=package::layout_version(&initial)?.into();
            if general_recovery { attempt["mode"]="general".into(); }
            admit_metadata_recovery(store,id,&attempt)?;
        }
        store.write_journal(lock, id, &attempt)?;
    } else if import_only && !general_recovery && existing.as_ref().unwrap()["mode"].is_null() {
        let mut attempt = existing.unwrap();
        attempt["mode"] = "localImportOnly".into();
        store.write_journal(lock, id, &attempt)?;
    }
    imports.execute(store, lock)?;
    for plan in fonts { let c=plan.game_citadel()?;let guard=_game_locks.iter().find(|g|g.validate(&c).is_ok()).ok_or_else(recovery)?;plan.execute(store,lock,guard).map_err(|_|recovery())?; }
    for (original_id, j, rollback) in games {
        let c=Path::new(string(&j,"citadel")?);
        let authority=Some(_game_locks.iter().find(|g|g.validate(c).is_ok()).ok_or_else(recovery)?);
        if let Some(plan) = rollback {
            plan.execute()?;
            store.record_failure(lock,&original_id,&j["input"],&game_rollback_error())?;
            store.clear_journal(lock, &original_id)?;
        } else {
            finish(store, lock, &original_id, &j, &mut |_| Ok(()), authority)?;
        }
    }
    let remaining=store.journals()?;
    crate::font_transaction::admit_set(store,&remaining)?;
    if remaining.members().any(|e|e["body"]["kind"]!="recovery") { return Err(recovery()); }
    if general_recovery {
        let doc=store.document()?;
        store::ensure_readable(&doc)?;
        // Rolling back a re-Apply restores its observed external baseline,
        // which intentionally differs from the older persisted checkpoint.
        check_base(store.root(),&doc, authority)?;
        verify_fonts(store,&doc,lock,authority)?;
        if remaining.members().any(|e|e["body"]["version"]==6) {
            let c=citadel(Path::new(string(&doc,"gamePath")?))?;
            shards::verify_preparation_namespace(store,&c,&doc, authority)?;
            check_upstream(&doc["previous"], false, &c, authority)?;
            for entry in remaining.members().filter(|e|e["body"]["version"]==6) {
                verify_vanilla_recovery_gameinfo(store,&entry["body"], authority)?;
            }
        }
    }
    let mut entries: Vec<_> = remaining.members().cloned().collect();
    entries.sort_by_key(|e| e["id"] == id);
    let mut result = None;
    for entry in entries {
        let recovery_id = string(&entry, "id")?;
        let j = &entry["body"];
        if j["kind"] != "recovery" {
            return Err(recovery());
        }
        let snapshot = if import_only || matches!(j["version"].as_u32(),Some(4|5|6)) && j["mode"]=="general" {
            store.complete_recovery(lock, rev(&store.document()?)?, recovery_id, &j["input"])?
        } else {
            let mut doc = store.document()?;
            check_applied(store.root(), &doc, authority)?;
            verify_fonts(store, &doc,lock,authority)?;
            doc["recoveryRequired"] = false.into();
            let result = store.commit(lock, rev(&doc)?, recovery_id, &j["input"], doc, None)?;
            store.clear_journal(lock, recovery_id)?;
            result
        };
        if recovery_id == id {
            result = Some(snapshot);
        }
    }
    result.ok_or_else(recovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{import, Fixture, game_input};
    // Synthetic historical fixtures, never product flat deployment writers.
    // Actual old-helper compatibility remains a separate executable proof.
    fn historical_flat_checkpoint(st:&Store,c:&Path,plan:&ApplyPlan,id:&str,before:&str,version:u32)->JsonValue {
        let mut rows=JsonValue::new_array();
        for family in &plan.layout.families {
            let mut row=if version==1 {package::encode_file(&family.package.directory)}
                else {package::encode_packages(std::slice::from_ref(&family.package))[0].clone()};
            row["modId"]=family.mod_id.clone().into();row["modIds"]=family.mod_ids.clone().into();rows.push(row).unwrap();
        }
        let mut manifest=object!{version:version,storeRoot:st.root().to_str(),generation:id};
        manifest[if version==1 {"files"} else {"packages"}]=rows;
        let dir=c.join("deadlock_native").join(id);fs::create_dir_all(&dir).unwrap();
        fs::write(c.join("deadlock_native/owner.json"),object!{version:1,root:st.root().to_str()}.dump()).unwrap();
        for file in generation_files(&manifest).unwrap(){
            let path=dir.join(file.name);fs::copy(st.payload_path(&file.payload.id).unwrap(),&path).unwrap();
            // Retained flat writers created ordinary writable copies.
            fs::set_permissions(&path,std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        }
        fs::write(dir.join("manifest.json"),manifest.dump()).unwrap();
        let after=gameinfo::replace(before,&HashSet::from(["citadel/addons".into()]),None,&format!("citadel/deadlock_native/{id}")).unwrap().0;
        let mut applied=object!{profileId:"default",enabled:plan.enabled.clone(),generation:id,manifest:manifest.clone(),gameinfo:after.clone(),gameinfoSha256:paths::digest_bytes(after.as_bytes()).unwrap(),identities:generation::capture_file_identities(&dir,&manifest).unwrap()};
        // Gen1 historically has no directory birth proof. Do not invent it.
        if version==2 {applied["directoryIdentity"]=generation::capture_identity(&dir).unwrap();}
        applied
    }
    fn historical_flat_restore_setup(version:u32,two:bool)->(Fixture,RootLock,Store) {
        let(f,lock,st,plan)=restore_source_setup(version);
        let c=citadel(&f.game).unwrap();let before=text(&c.join("gameinfo.gi")).unwrap();
        let a=historical_flat_checkpoint(&st,&c,&plan,&"a".repeat(32),&before,version);
        let b=if two {historical_flat_checkpoint(&st,&c,&plan,&"b".repeat(32),&before,version)}else{a.clone()};
        let previous_gi=if two {a["gameinfo"].as_str().unwrap().to_owned()}else{before.clone()};
        let mut previous=object!{applied:if two {a}else{JsonValue::Null},activeProfileId:"default",gameinfo:previous_gi.clone(),gameinfoSha256:paths::digest_bytes(previous_gi.as_bytes()).unwrap(),upstreamPayloads:plan.upstream_payloads.clone(),upstreamEvidenceVersion:version};
        capture_checkpoint_baselines(&mut previous,&c).unwrap();
        let mut doc=st.document().unwrap();doc["applied"]=b.clone();doc["previous"]=previous;doc["canRestore"]=true.into();doc["revision"]=if two {"3"}else{"2"}.into();
        item93_write_document(&st,&doc);fs::write(c.join("gameinfo.gi"),b["gameinfo"].as_str().unwrap()).unwrap();
        assert_eq!(doc["storeVersion"],version);assert!(!crate::deployment_metadata::retained_shards(&doc));
        check_snapshot(st.root(),&doc,None).unwrap();
        (f,lock,st)
    }
    fn historical_restore_setup()->(Fixture,RootLock,Store) {historical_flat_restore_setup(2,false)}
    // Clear every fault hook this round uses, including unwinding callbacks.
    struct FaultHooks;
    impl Drop for FaultHooks {fn drop(&mut self){
        shards::TEST_CHECKPOINT.with(|h|*h.borrow_mut()=None);
        shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=None);
        BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=None);
        BEFORE_FLAT_UNLINK.with(|h|*h.borrow_mut()=None);
        shards::BEFORE_UNLINK.with(|h|*h.borrow_mut()=None);
    }}
    fn game_evidence(root:&Path)->std::collections::BTreeMap<PathBuf,JsonValue> {
        fn visit(root:&Path,path:&Path,out:&mut std::collections::BTreeMap<PathBuf,JsonValue>) {
            let metadata=fs::symlink_metadata(path).unwrap();
            let value=if metadata.is_dir(){object!{kind:"directory",identity:generation::capture_identity(path).unwrap()}}
                else {object!{kind:"file",identity:generation::capture_identity(path).unwrap(),sha256:paths::fingerprint(path).unwrap().sha256}};
            out.insert(path.strip_prefix(root).unwrap().to_owned(),value);
            if metadata.is_dir(){for entry in fs::read_dir(path).unwrap(){visit(root,&entry.unwrap().path(),out);}}
        }
        let mut out=std::collections::BTreeMap::new();visit(root,root,&mut out);out
    }
    #[test]
    fn fix1_historical_flat_restore_recovers_at_transaction_boundaries() {
        for version in [1,2] {for boundary in ["journal","beforeRename","renamed"] {
            let(f,lock,st)=historical_flat_restore_setup(version,true);
            let before_doc=st.document().unwrap();let a=before_doc["previous"]["applied"].clone();let b=before_doc["applied"].clone();
            let c=citadel(&f.game).unwrap();let a_dir=generation_dir(&c,&a).unwrap();let b_dir=generation_dir(&c,&b).unwrap();
            let a_evidence=game_evidence(&a_dir);let b_evidence=game_evidence(&b_dir);
            let before=fs::read(c.join("gameinfo.gi")).unwrap();let mut hit=false;
            let input=game_input("native.restore",3,"flat-restore",&object!{});
            let error=restore_with(&st,&lock,3,"flat-restore",&input,&mut |at|{
                if at==boundary {hit=true;Err(err("nativeIo","historical flat Restore crash"))}else{Ok(())}
            }).unwrap_err();
            assert!(hit,"flat{version} {boundary}: retained executor boundary missing");
            assert_eq!(error.kind,"recoveryRequired");
            let j=st.journal("flat-restore").unwrap().unwrap();
            assert_eq!(j["version"],version);assert!(j["generation"].is_null());
            assert_eq!(j["input"]["method"],"native.restore");
            assert_eq!(j["document"]["applied"]["generation"],a["generation"]);
            assert_eq!(st.document().unwrap(),before_doc);assert_eq!(st.operation("flat-restore").unwrap()["status"],"recoveryRequired");
            assert_eq!(game_evidence(&a_dir),a_evidence);assert_eq!(game_evidence(&b_dir),b_evidence);
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),if boundary=="renamed" {a["gameinfo"].as_str().unwrap().as_bytes()}else{&before});
            drop(st);drop(lock);
            let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
            recover(&st,&lock,3,"flat-recover",&game_input("native.recover",3,"flat-recover",&object!{})).unwrap();
            assert!(st.journals().unwrap().is_empty());assert_eq!(st.operation("flat-recover").unwrap()["status"],"succeeded");
            assert_eq!(game_evidence(&a_dir),a_evidence);
            let doc=st.document().unwrap();assert_eq!(doc["storeVersion"],version);
            if boundary=="renamed" {
                assert_eq!(doc["applied"],a);assert_eq!(doc["canRestore"],false);assert!(doc["previous"].is_null());
                assert_eq!(st.operation("flat-restore").unwrap()["status"],"succeeded");
                assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),a["gameinfo"].as_str().unwrap().as_bytes());
                if version==1 {assert_eq!(game_evidence(&b_dir),b_evidence);}else{assert!(!b_dir.exists());}
            } else {
                assert_eq!(doc["applied"],b);assert_eq!(doc["previous"],before_doc["previous"]);assert_eq!(doc["canRestore"],true);
                assert_eq!(game_evidence(&b_dir),b_evidence);assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),before);
                assert_eq!(st.operation("flat-restore").unwrap()["status"],"failed");
                assert_eq!(st.replay("flat-restore",&input).unwrap_err().kind,"operationInterrupted");
                let revision=rev(&doc).unwrap();
                restore(&st,&lock,revision,"flat-retry",&game_input("native.restore",revision,"flat-retry",&object!{})).unwrap();
                assert_eq!(st.operation("flat-retry").unwrap()["status"],"succeeded");assert!(st.journals().unwrap().is_empty());
                assert_eq!(st.document().unwrap()["applied"],a);assert_eq!(game_evidence(&a_dir),a_evidence);
                if version==1 {assert_eq!(game_evidence(&b_dir),b_evidence);}else{assert!(!b_dir.exists());}
            }
            check_applied(st.root(),&st.document().unwrap(),None).unwrap();
        }}
    }
    #[test]
    fn fix1_current_unrecorded_directory_crash_preserves_unknown_ownership() {
        for occurrence in [1,2] {for foreign in [false,true] {
            let(f,lock,st,plan)=setup();let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();
            let dir=c.join("deadlock_native").join(&plan.generation);
            let interrupted=if occurrence==1 {dir.clone()}else{dir.join("shard01")};
            let mut seen=0;let target=interrupted.clone();
            let error=apply_at(&plan,&st,&lock,1,"unrecorded",&object!{},move|at|{
                if at=="directoryCreatedUnrecorded" {seen+=1;if seen==occurrence {
                    if foreign {fs::write(target.join("foreign"),b"unowned after mkdir").unwrap();}
                    return Err(err("nativeIo","crash before directory birth identity persistence"));
                }}Ok(())
            }).unwrap_err();
            assert_eq!(error.kind,"recoveryRequired");assert!(interrupted.is_dir());
            let j=st.journal("unrecorded").unwrap().unwrap();
            let relative=interrupted.strip_prefix(&dir).unwrap().to_str().unwrap();
            assert_eq!(j["stagedNodes"].len(),occurrence-1);
            assert!(!j["stagedNodes"].members().any(|row|row["name"]==relative),"interrupted directory has no durable owner");
            assert!(!j["document"]["applied"].has_key("directoryIdentity"));
            let evidence=game_evidence(&c);let operation=st.operation("unrecorded").unwrap();assert_eq!(operation["status"],"recoveryRequired");
            drop(st);drop(lock);
            let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
            assert_eq!(recover(&st,&lock,1,"recover",&game_input("native.recover",1,"recover",&object!{})).unwrap_err().kind,"externalChange");
            assert_eq!(game_evidence(&c),evidence);assert_eq!(st.document().unwrap(),doc);
            assert_eq!(st.journal("unrecorded").unwrap(),Some(j));assert_eq!(st.operation("unrecorded").unwrap(),operation);
            assert_eq!(st.operation("recover").unwrap()["status"],"unknown");
        }}
    }
    #[test]
    fn fix1_historical_flat_publication_rechecks_late_external_changes() {
        for mutation in ["upstream-content","previous-identity","unknown-member"] {
            let _hooks=FaultHooks;
            let two=mutation!="upstream-content";let expected=if two {3}else{2};
            let(f,lock,st)=historical_flat_restore_setup(2,two);let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();
            let original=fs::read(c.join("gameinfo.gi")).unwrap();
            let previous=generation_dir(&c,if two {&doc["previous"]["applied"]}else{&doc["applied"]}).unwrap();
            let target=match mutation {"upstream-content"=>c.join("addons/renamed-directory.vpk"),"previous-identity"=>previous.join("pak01_dir.vpk"),_=>previous.join("foreign")};
            let changed=target.clone();
            let mut rewrite=Some(move||{
                if mutation=="unknown-member" {fs::write(&changed,b"unknown user bytes").unwrap();}
                else {
                    let metadata=fs::metadata(&changed).unwrap();let mut bytes=fs::read(&changed).unwrap();
                    if mutation=="previous-identity" {let replacement=changed.with_extension("new");fs::write(&replacement,&bytes).unwrap();fs::rename(&replacement,&changed).unwrap();}
                    else {let last=bytes.len()-1;bytes[last]^=1;fs::write(&changed,&bytes).unwrap();}
                    File::options().write(true).open(&changed).unwrap().set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap())).unwrap();
                }
            });
            let mut changed_evidence=None;
            let mut owned_temp=None;
            let error=restore_with(&st,&lock,expected,"flat-late",&game_input("native.restore",expected,"flat-late",&object!{}),&mut |at|{
                if at=="beforeRename" {
                    rewrite.take().unwrap()();
                    changed_evidence=Some(paths::fingerprint(&target).unwrap());
                    let journal=st.journal("flat-late").unwrap().unwrap();
                    let temp=c.join(journal["temp"].as_str().unwrap());
                    assert!(temp.is_file(),"owned temporary file must exist before publication");
                    owned_temp=Some(temp);
                }Ok(())
            }).unwrap_err();
            assert!(rewrite.is_none(),"retained beforeRename hook must execute after preflight");
            let changed_evidence=changed_evidence.expect("capture injected evidence before rollback");
            assert!(!owned_temp.expect("capture owned temporary path").exists(),"refusal must remove its owned temporary file");
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),original);assert_eq!(st.document().unwrap(),doc);
            assert_eq!(error.kind,"externalChange");assert!(st.journals().unwrap().is_empty());
            assert_eq!(st.operation("flat-late").unwrap()["status"],"failed");
            assert_eq!(paths::fingerprint(&target).unwrap(),changed_evidence);assert_eq!(st.document().unwrap(),doc);
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),original);
        }
    }
    #[test]
    fn fix1_historical_flat_retirement_rechecks_every_unlink() {
        // Family gen2 contains three payload members and a manifest. Race each
        // unlink in turn, including after earlier verified deletions succeeded.
        for occurrence in 0..4 {for mutation in ["unknown-member","replacement","content"] {
            let _hooks=FaultHooks;
            let(f,lock,st)=historical_flat_restore_setup(2,true);let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();
            let retired=doc["applied"].clone();let dir=generation_dir(&c,&retired).unwrap();let target_dir=dir.clone();
            let changed=std::rc::Rc::new(std::cell::RefCell::new(None));let observed=changed.clone();let mut seen=0;
            BEFORE_FLAT_UNLINK.with(|h|*h.borrow_mut()=Some(Box::new(move|path|{
                if seen==occurrence {
                    let target=if mutation=="unknown-member"{target_dir.join("foreign")}else{path.to_owned()};
                    if mutation=="unknown-member" {fs::write(&target,b"unknown user bytes").unwrap();}
                    else {
                        let metadata=fs::metadata(&target).unwrap();let mut bytes=fs::read(&target).unwrap();
                        if mutation=="replacement" {let replacement=target_dir.join("external-new");fs::write(&replacement,&bytes).unwrap();fs::rename(replacement,&target).unwrap();}
                        else {let last=bytes.len()-1;bytes[last]^=1;fs::write(&target,&bytes).unwrap();}
                        if mutation=="replacement" {File::options().write(true).open(&target).unwrap().set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap())).unwrap();}
                    }
                    *observed.borrow_mut()=Some((target.clone(),paths::fingerprint(&target).unwrap()));
                }seen+=1;
            })));
            restore(&st,&lock,3,"flat-retire",&game_input("native.restore",3,"flat-retire",&object!{})).unwrap();
            let changed=changed.borrow();let(target,evidence)=changed.as_ref().expect("retained per-unlink hook must run");
            assert_eq!(paths::fingerprint(target).unwrap(),*evidence,"{occurrence} {mutation}: changed bytes must survive");
            let after=st.document().unwrap();assert_eq!(after["applied"],doc["previous"]["applied"]);assert_eq!(after["canRestore"],false);
            assert!(after["retiredGenerations"].members().any(|a|*a==retired),"cleanup refusal must retain ownership evidence");
            assert!(st.journals().unwrap().is_empty());assert_eq!(st.operation("flat-retire").unwrap()["status"],"succeeded");
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),doc["previous"]["gameinfo"].as_str().unwrap().as_bytes());
            check_applied(st.root(),&after,None).unwrap();
            // Only pre-race verified files may have been deleted. We do not
            // claim that they all survive best-effort retirement.
            assert_eq!(fs::read_dir(&dir).unwrap().count(),4-occurrence+usize::from(mutation=="unknown-member"));
        }}
    }
    fn apply_at(plan:&ApplyPlan,store:&Store,lock:&RootLock,expected:u64,id:&str,input:&JsonValue,
        checkpoint:impl FnMut(&str)->Result<(),NativeError>+'static)->Result<JsonValue,NativeError>{
        struct Reset;
        impl Drop for Reset {fn drop(&mut self){shards::TEST_CHECKPOINT.with(|h|*h.borrow_mut()=None);}}
        shards::TEST_CHECKPOINT.with(|h|*h.borrow_mut()=Some(Box::new(checkpoint)));
        let _reset=Reset;
        plan.apply(store,lock,expected,id, &game_input("native.apply", expected, id, input))
    }
    fn restore_at(store:&Store,lock:&RootLock,expected:u64,id:&str,input:&JsonValue,
        checkpoint:impl FnMut(&str)->Result<(),NativeError>+'static)->Result<JsonValue,NativeError>{
        struct Reset;
        impl Drop for Reset {fn drop(&mut self){shards::TEST_CHECKPOINT.with(|h|*h.borrow_mut()=None);}}
        shards::TEST_CHECKPOINT.with(|h|*h.borrow_mut()=Some(Box::new(checkpoint)));
        let _reset=Reset;
        restore(store,lock,expected,id, &game_input("native.restore", expected, id, input))
    }
    #[test]
    fn sharded_modlock_mount_is_unknown_before_writes() {
        let(f,_lock,store,_)=setup();let c=citadel(&f.game).unwrap();let gi=c.join("gameinfo.gi");
        let original=text(&gi).unwrap();let foreign=original.replace("Game citadel\n","Game citadel/modlock/foreign/shard01\n Game citadel\n");
        assert_ne!(foreign,original);fs::write(&gi,&foreign).unwrap();
        let folder=c.join("modlock/foreign/shard01");fs::create_dir_all(&folder).unwrap();fs::write(folder.join("keep"),b"unowned").unwrap();
        let before=store.document().unwrap();
        assert_eq!(ApplyPlan::review(&store,"default",&f.game).err().unwrap().kind,"unknownMount");
        assert_eq!(text(&gi).unwrap(),foreign);assert_eq!(fs::read(folder.join("keep")).unwrap(),b"unowned");
        assert!(!c.join("deadlock_native").exists());assert!(store.journals().unwrap().is_empty());assert_eq!(store.document().unwrap(),before);
    }

    #[test]
    fn round3_publication_rehashes_current_restore() {
        let _hooks=FaultHooks;
        {
            let(f,lock,st,_)=setup();let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let mut input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"apply",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            {input["params"]["deploymentLayout"]="shards-v1".into();}
            plan.apply(&st,&lock,1,"apply", &game_input("native.apply", 1, "apply", &input)).unwrap();let gi=f.game.join("game/citadel/gameinfo.gi");let before=text(&gi).unwrap();
            let path=f.game.join("game/citadel/addons/pak01_dir.vpk");
            let rewrite=move||{let m=fs::metadata(&path).unwrap();let mut bytes=fs::read(&path).unwrap();let n=bytes.len()-1;bytes[n]^=1;fs::write(&path,bytes).unwrap();File::options().write(true).open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(m.modified().unwrap())).unwrap();};
            let input=object!{method:"native.restore",params:object!{dataDir:f.data.to_str(),operationId:"restore",expectedRevision:"2"}};
            let result={shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(rewrite)));restore(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &input))};
            assert!(result.is_err(),"publication must hash content, sharded=shards-v1");assert_eq!(text(&gi).unwrap(),before);
        }
    }
    #[test]
    fn round3_mixed_restore_preserves_repair_proof() {
        let(f,lock,st,_)=setup();
        let input=|id:&str,r:&str|object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1"))).unwrap();
        let c=citadel(&f.game).unwrap(); let gi=c.join("gameinfo.gi");
        let dir=c.join("dmm_localization_default"); fs::create_dir(&dir).unwrap();fs::write(dir.join("old.vpk"),b"old").unwrap();
        let old=text(&gi).unwrap().replace("Game citadel\n","Game citadel/dmm_localization_default\n Game citadel\n");fs::write(&gi,old).unwrap();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"second", &game_input("native.apply", 2, "second", &input("second","2"))).unwrap();
        let before=text(&gi).unwrap();
        shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(move||fs::write(dir.join("extra.vpk"),b"changed").unwrap())));
        let mut i=input("restore","3");i["method"]="native.restore".into();
        assert!(restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &i)).is_err(),"changed repair proof must refuse Restore");
        assert_eq!(text(&gi).unwrap(),before);
    }
    #[test]
    fn apply_review_rehashes_payload_changed_with_restored_mtime() {
        let (_f, _lock, st, _) = setup_padded(1024);
        let doc = st.document().unwrap();
        let version = package::layout_version(&doc).unwrap();
        let row = &doc["profiles"]["default"]["mods"][0];
        let package = package::selected_packages_exact(row, version).unwrap().remove(0);
        let payload = st.payload_path(&package.directory.id).unwrap();
        let proof = generation::ReviewHash::capture(&payload).unwrap();
        let old = fs::metadata(&payload).unwrap();
        let mut bytes = fs::read(&payload).unwrap();
        let last = bytes.len() - 1; bytes[last] ^= 1;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&payload, bytes).unwrap();
        File::options().write(true).open(&payload).unwrap().set_times(fs::FileTimes::new().set_modified(old.modified().unwrap())).unwrap();
        let proofs = HashMap::from([(payload.clone(), proof)]);
        assert!(generation::verify_package_evidence_with_review(&package, version, |f| st.payload_path(&f.id), &proofs).is_err());
    }
    #[test]
    fn apply_review_cached_repairs_needs_no_writable_cache() {
        let (f, _lock, st, _) = setup();
        ApplyPlan::review(&st,"default",&f.game).unwrap();
        let cache = crate::storage::path(st.root(), "cache/repairs").unwrap();
        let permissions = fs::metadata(&cache).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o500)).unwrap();
        let result = ApplyPlan::review(&st,"default",&f.game);
        fs::set_permissions(&cache, permissions).unwrap();
        assert!(result.is_ok(), "cached review must not write to the repair cache");
    }
    fn item93_renumber(v:&mut JsonValue) {
        if v.is_object() {
            if let Some(d)=v["device"].as_u64() {v["device"]=(d+1).into();}
            let keys=v.entries().map(|(k,_)|k.to_owned()).collect::<Vec<_>>();
            for k in keys {item93_renumber(&mut v[&k]);}
        } else if v.is_array() {for i in 0..v.len(){item93_renumber(&mut v[i]);}}
    }
    fn item93_write_document(st:&Store,doc:&JsonValue) {
        let db=crate::sqlite::Database::open(&st.root().join("library.sqlite3"),true).unwrap();
        db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();
    }
    #[test]
    fn item93_b1_legacy_gameinfo_temp_requires_full_image(){item93_legacy_cleanup_cases(false);}
    #[test]
    fn item93_b2_legacy_incomplete_generation_requires_full_hash(){item93_legacy_cleanup_cases(true);}
    fn item93_legacy_cleanup_cases(partial:bool){
        for restoring in if partial{vec![false]}else{vec![false,true]}{for drift in [true,false]{for matching in [false,true]{
            let (f,lock,st,plan)=setup();let c=citadel(&f.game).unwrap();if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){continue;}
            let mut r=1;
            if restoring {plan.apply(&st,&lock,1,"seed-cleanup", &game_input("native.apply", 1, "seed-cleanup", &object!{method:"native.apply",params:object!{operationId:"seed-cleanup",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}})).unwrap();r=2;}
            let input=object!{method:if restoring{"native.restore"}else{"native.apply"},params:object!{operationId:"cleanup-main",expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef"}};
            let stop=if partial{"copyChunk"}else{"beforeRename"};
            let crash=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
                let checkpoint=move |at:&str|{if at==stop{panic!("interrupted cleanup fixture");}Ok(())};
                if restoring{restore_at(&st,&lock,r,"cleanup-main", &game_input("native.restore", r, "cleanup-main", &input),checkpoint)}else{apply_at(&ApplyPlan::review(&st,"default",&f.game).unwrap(),&st,&lock,r,"cleanup-main", &game_input("native.apply", r, "cleanup-main", &input),checkpoint)}
            }));assert!(crash.is_err());
            let mut j=st.journal("cleanup-main").unwrap().unwrap();
            let path=if partial{generation_dir(&c,&j["document"]["applied"]).unwrap().join(j["stagedNodes"][2]["name"].as_str().unwrap())}else{c.join(j["temp"].as_str().unwrap())};
            let id=if partial{&mut j["stagedNodes"][2]}else{&mut j["tempIdentity"]};id.remove("fsid");if drift{id["device"]=(id["device"].as_u64().unwrap()+1).into();}
            let mut bytes=fs::read(&path).unwrap();if !matching{bytes[0]^=1;fs::write(&path,&bytes).unwrap();}
            st.write_journal(&lock,"cleanup-main",&j).unwrap();
            let result=recover(&st,&lock,r,"cleanup-recover", &game_input("native.recover", r, "cleanup-recover", &object!{method:"native.recover",params:object!{operationId:"cleanup-recover",expectedRevision:r.to_string()}}));
            if !matching {assert!(result.is_err());assert!(path.exists(),"different bytes must never be deleted: partial={partial} restoring={restoring}");assert_eq!(fs::read(&path).unwrap(),bytes);assert!(st.journal("cleanup-main").unwrap().is_some());}
            else {result.as_ref().unwrap();assert!(!path.exists(),"matching bytes and same-device legacy cleanup stay supported");}
            if result.is_ok(){assert!(st.journals().unwrap().is_empty());assert_ne!(st.document().unwrap()["recoveryRequired"],true);}
        }}}
    }

    #[test]
    fn item93_new_generation_and_journal_capture_fsid() {
        {
            let (f,lock,st,_)=setup();
            let mut input=object!{method:"native.apply",params:object!{operationId:"fsid93",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            {input["params"]["deploymentLayout"]="shards-v1".into();}
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let interrupted=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                {shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("power loss"))));plan.apply(&st,&lock,1,"fsid93", &game_input("native.apply", 1, "fsid93", &input)).unwrap();}
            }));assert!(interrupted.is_err());
            let j=st.journal("fsid93").unwrap().unwrap();let a=&j["document"]["applied"];
            assert!(a["directoryIdentity"]["fsid"].as_str().is_some_and(|s|package::hex(s,16)),"new generation root fsid");
            for (_,id) in a["identities"].entries().chain(a["shardIdentities"].entries()) {assert!(id["fsid"].as_str().is_some_and(|s|package::hex(s,16)));}
            assert!(j["tempIdentity"]["fsid"].as_str().is_some_and(|s|package::hex(s,16)),"journal temp fsid");
            let rows={&j["stagedNodes"]};for row in rows.members(){assert!(row["fsid"].as_str().is_some_and(|s|package::hex(s,16)));}
        }
    }

    #[test]
    fn item93_reboot_journal_recovery(){item93_journal_cases(false)}
    #[test]
    fn item93_vanilla_reboot_journal_recovery(){item93_journal_cases(true)}
    fn item93_journal_cases(vanilla:bool) {
        fn edit(v:&mut JsonValue,legacy:bool){
            if v.is_object(){if v.has_key("device"){v["device"]=(v["device"].as_u64().unwrap()+1).into();if legacy{v.remove("fsid");}}
                let keys=v.entries().map(|(k,_)|k.to_owned()).collect::<Vec<_>>();for k in keys{edit(&mut v[&k],legacy);}}
            else if v.is_array(){for i in 0..v.len(){edit(&mut v[i],legacy);}}
        }
        {for legacy in [false,true] {for forward in [false,true] {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){continue;}
            if vanilla{for (id,r) in [("seed-a93",1),("seed-b93",2)]{let input=object!{method:"native.apply",params:object!{operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,r,id, &game_input("native.apply", r, id, &input)).unwrap();}}
            let start=rev(&st.document().unwrap()).unwrap();
            let mut input=object!{method:if vanilla{"native.vanilla"}else{"native.apply"},params:object!{operationId:"journal93",expectedRevision:start.to_string(),token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            let plan=if vanilla{let guard=GameLock::acquire(&c).unwrap();ApplyPlan::review_vanilla_locked(&st,&f.game,&guard,&lock).unwrap()}else{ApplyPlan::review(&st,"default",&f.game).unwrap()};
            let interrupted=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
                {if forward{shards::REVIEW7_AFTER_DB.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("power loss"))));}else{shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("power loss"))));}plan.apply(&st,&lock,start,"journal93", &game_input("native.apply", start, "journal93", &input)).unwrap();}
            }));assert!(interrupted.is_err());
            let mut j=st.journal("journal93").unwrap().unwrap();
            for key in ["beforeIdentity","tempIdentity","stagedNodes","stagedFiles","generationIdentity","workspaceIdentity"]{if j.has_key(key){edit(&mut j[key],legacy);}}
            for key in ["document","beforeDocument"]{if j[key].is_object(){for field in ["applied","retiredGenerations"]{edit(&mut j[key][field],legacy);}if j[key]["previous"].is_object(){edit(&mut j[key]["previous"]["applied"],legacy);}}}
            let mut doc=st.document().unwrap();for field in ["applied","retiredGenerations"]{edit(&mut doc[field],legacy);}if doc["previous"].is_object(){edit(&mut doc["previous"]["applied"],legacy);}item93_write_document(&st,&doc);st.write_journal(&lock,"journal93",&j).unwrap();
            let r=rev(&doc).unwrap();
            for field in ["inode","fsid"] {
                let mut foreign=j.clone();foreign["tempIdentity"][field]=if field=="inode"{(foreign["tempIdentity"][field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};
                st.write_journal(&lock,"journal93",&foreign).unwrap();let before=st.document().unwrap();
                assert!(recover(&st,&lock,r,"foreign93", &game_input("native.recover", r, "foreign93", &object!{method:"native.recover",params:object!{operationId:"foreign93",expectedRevision:r.to_string()}})).is_err(),"sharded=shards-v1 legacy={legacy} forward={forward} field={field}");
                assert_eq!(st.document().unwrap(),before);assert_eq!(st.journal("journal93").unwrap().unwrap(),foreign);
                st.write_journal(&lock,"journal93",&j).unwrap();
            }
            if vanilla {
                let retired=&j["document"]["retiredGenerations"][0];let root=generation_dir(&c,retired).unwrap();
                if root.exists(){let parked=c.join("retirement-evidence93");fs::rename(&root,&parked).unwrap();fs::create_dir(&root).unwrap();
                    assert!(recover(&st,&lock,r,"foreign-tree93", &game_input("native.recover", r, "foreign-tree93", &object!{method:"native.recover",params:object!{operationId:"foreign-tree93",expectedRevision:r.to_string()}})).is_err());assert_eq!(st.document().unwrap(),doc);assert_eq!(st.journal("journal93").unwrap().unwrap(),j);
                    fs::remove_dir(&root).unwrap();fs::rename(&parked,&root).unwrap();}
            }
            recover(&st,&lock,r,"recover93", &game_input("native.recover", r, "recover93", &object!{method:"native.recover",params:object!{operationId:"recover93",expectedRevision:r.to_string()}})).unwrap_or_else(|e|panic!("sharded=shards-v1 legacy={legacy} forward={forward}: {e:?}"));
            assert!(st.journals().unwrap().is_empty());assert_ne!(st.document().unwrap()["recoveryRequired"],true);
        }}}
    }

    #[test]
    fn item93_single_apply_reboot_restores_original_baselines(){
        {for legacy in [false,true]{
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){continue;}
            let before=text(&c.join("gameinfo.gi")).unwrap();
            let mut input=object!{method:"native.apply",params:object!{operationId:"single93",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"single93", &game_input("native.apply", 1, "single93", &input)).unwrap();
            let mut doc=st.document().unwrap();assert!(doc["previous"]["installedGameOrigins"][0]["directoryIdentity"]["fsid"].is_string());
            if legacy {fn strip(v:&mut JsonValue){if v.is_object(){v.remove("fsid");for (_,child) in v.entries_mut(){strip(child);}}else if v.is_array(){for child in v.members_mut(){strip(child);}}}strip(&mut doc["previous"]);}
            item93_renumber(&mut doc["applied"]);item93_renumber(&mut doc["previous"]);item93_renumber(&mut doc["importLedger"]);item93_write_document(&st,&doc);
            for field in ["inode","fsid"] {
                let mut foreign=doc.clone();let id=&mut foreign["previous"]["installedGameOrigins"][0]["directoryIdentity"];
                id[field]=if field=="inode"{(id[field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};item93_write_document(&st,&foreign);
                assert!(restore(&st,&lock,2,"foreign-baseline93", &game_input("native.restore", 2, "foreign-baseline93", &object!{method:"native.restore",params:object!{operationId:"foreign-baseline93",expectedRevision:"2"}})).is_err());assert_eq!(st.document().unwrap(),foreign);assert!(st.journals().unwrap().is_empty());
            }
            item93_write_document(&st,&doc);let guard=GameLock::acquire(&c).unwrap();assert_eq!(st.snapshot_locked(None,&lock,&guard).unwrap()["canRestore"],true);assert_eq!(st.document().unwrap(),doc);drop(guard);
            restore(&st,&lock,2,"single-restore93", &game_input("native.restore", 2, "single-restore93", &object!{method:"native.restore",params:object!{operationId:"single-restore93",expectedRevision:"2"}})).unwrap();assert_eq!(text(&c.join("gameinfo.gi")).unwrap(),before);
            let guard=GameLock::acquire(&c).unwrap();let prepared=ApplyPlan::review_shards_locked(&st,"default",&f.game,&guard,&lock,&mut |_|{}).unwrap();prepared.preparation_evidence(&st,&guard,&lock).unwrap();drop(guard);
            let mut input=object!{method:"native.apply",params:object!{operationId:"single-again93",expectedRevision:"3",token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,3,"single-again93", &game_input("native.apply", 3, "single-again93", &input)).unwrap();assert!(st.journals().unwrap().is_empty());
        }}
    }

    #[test]
    fn item93_review_r4_installed_origin_drift_uses_review_authority(){
        {
            let (f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){return;}
            let dir=c.join("addons");let m=fs::metadata(&dir).unwrap();let mut origin=object!{treeComplete:true,treeVersion:1,gamePath:f.game.to_str(),mount:"citadel/addons",directory:dir.to_str(),directoryIdentity:object!{device:m.dev(),inode:m.ino()},files:crate::installed::mount_proof(&dir).unwrap()};item93_renumber(&mut origin);
            let mut doc=st.document().unwrap();doc["importLedger"]["manifests"]=json::array![];doc["profiles"]["default"]["mods"][0]["source"]["installedGameOrigins"]=json::array![origin.clone()];item93_write_document(&st,&doc);
            for target in ["directory","file"]{for field in ["inode","fsid"]{
                let mut foreign=doc.clone();let o=&mut foreign["profiles"]["default"]["mods"][0]["source"]["installedGameOrigins"][0];let id=if target=="directory"{&mut o["directoryIdentity"]}else{&mut o["files"][0]["identity"]};id[field]=if field=="inode"{(id[field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};item93_write_document(&st,&foreign);
                assert!(ApplyPlan::review(&st,"default",&f.game).is_err());assert_eq!(st.document().unwrap(),foreign);assert!(st.journals().unwrap().is_empty());
            }}
            item93_write_document(&st,&doc);let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let mut input=object!{method:"native.apply",params:object!{operationId:"origin-r4",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            plan.apply(&st,&lock,1,"origin-r4", &game_input("native.apply", 1, "origin-r4", &input)).unwrap();let after=st.document().unwrap();let saved=after["previous"]["upstreamPayloads"].members().find(|r|r["proofKind"]=="mount" && r["sourcePath"]==origin["directory"]).unwrap();assert_eq!(saved["files"],origin["files"]);assert!(st.journals().unwrap().is_empty());
        }
    }

    #[test]
    fn item93_review_r2_apply_preserves_legacy_source_records(){
        {
            let (f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){return;}
            let mut doc=st.document().unwrap();item93_renumber(&mut doc["importLedger"]["sourcePayloads"]);let original=doc["importLedger"]["sourcePayloads"].clone();item93_write_document(&st,&doc);
            let mut input=object!{method:"native.apply",params:object!{operationId:"preserve-r2",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"preserve-r2", &game_input("native.apply", 1, "preserve-r2", &input)).unwrap();
            let after=st.document().unwrap();let rows=&after["previous"]["upstreamPayloads"];let mut checked=0;
            for saved in rows.members(){
                let recorded=if package::layout_version(&doc).unwrap()==2 {original.members().flat_map(|p|p["engineFiles"].members()).find(|r|r["sourcePath"]==saved["sourcePath"])}else{original.members().find(|r|r["sourcePath"]==saved["sourcePath"])}.unwrap();
                assert_eq!(saved,recorded,"durable legacy proof must remain verbatim");checked+=1;
            }
            assert!(checked>0);assert_eq!(after["importLedger"]["sourcePayloads"],original);
        }
    }

    #[test]
    fn item93_pre_fsid_writable_checkpoint_snapshot_stays_applied(){
        for reboot in [false,true]{
        use std::os::unix::fs::PermissionsExt;
        let (f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
        let input=object!{method:"native.apply",params:object!{operationId:"pre93",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"pre93", &game_input("native.apply", 1, "pre93", &input)).unwrap();
        let mut doc=st.document().unwrap();let a=&mut doc["applied"];let dir=generation_dir(&c,a).unwrap();
        // Pre-93 identities contain only dev/inode/size/mtime. Older valid
        // generations (and the relocated review fixture) may be owner-writable.
        a["directoryIdentity"].remove("fsid");
        for (_,id) in a["shardIdentities"].entries_mut(){id.remove("fsid");}
        for (name,id) in a["identities"].entries_mut(){id.remove("fsid");fs::set_permissions(dir.join(name),fs::Permissions::from_mode(0o600)).unwrap();}
        if reboot{item93_renumber(a);}
        item93_write_document(&st,&doc);drop(st);drop(lock);
        let snapshot=crate::native::NativeService::new().dispatch("native.snapshot",&object!{dataDir:f.data.to_str()}).unwrap();
        // Device drift without fsid is admitted only on btrfs/overlay; other
        // filesystems must keep refusing it.
        if !reboot||paths::test_volatile_filesystem(&File::open(&c).unwrap()) {
            assert_ne!(snapshot["recoveryRequired"],true,"{snapshot}");
            assert_eq!(snapshot["pendingChanges"],false,"{snapshot}");
            for row in snapshot["mods"].members(){assert_eq!(row["pendingApply"],false,"{row}");assert_eq!(row["appliedEnabled"],row["enabled"],"{row}");}
        } else {assert_eq!(snapshot["recoveryRequired"],true,"{snapshot}");assert_eq!(snapshot["pendingChanges"],true,"{snapshot}");}
        let st=Store::open(&f.data).unwrap();assert_eq!(st.document().unwrap(),doc,"read admission must not upgrade older proof");
        // The accepted same-device case makes tamper checks meaningful even
        // where the simulated reboot is already refused (for example tmpfs).
        {
            let name=generation::decode(&doc["applied"]["manifest"]).unwrap().files[0].name.clone();let path=dir.join(&name);let bytes=fs::read(&path).unwrap();
            for field in ["inode","fsid"] {
                let mut foreign=doc.clone();let id=&mut foreign["applied"]["identities"][&name];id[field]=if field=="inode"{(id[field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};item93_write_document(&st,&foreign);
                let value=crate::native::NativeService::new().dispatch("native.snapshot",&object!{dataDir:f.data.to_str()}).unwrap();assert_eq!(value["pendingChanges"],true);assert_eq!(fs::read(&path).unwrap(),bytes);
            }
            item93_write_document(&st,&doc);let m=fs::metadata(&path).unwrap();let mut foreign=bytes.clone();let n=foreign.len()-1;foreign[n]^=1;fs::write(&path,&foreign).unwrap();File::options().write(true).open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(m.modified().unwrap())).unwrap();
            let value=crate::native::NativeService::new().dispatch("native.snapshot",&object!{dataDir:f.data.to_str()}).unwrap();assert_eq!(value["pendingChanges"],true,"writable legacy admission must verify bytes");assert_eq!(fs::read(&path).unwrap(),foreign);
        }
        }
    }

    #[test]
    fn item93_legacy_snapshot_uses_ordered_game_authority(){
        for drift in [false,true]{
        let(f,lock,st,_)=setup();let input=object!{method:"native.apply",params:object!{operationId:"snapshot93",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"snapshot93", &game_input("native.apply", 1, "snapshot93", &input)).unwrap();
        let supported=paths::test_volatile_filesystem(&File::open(citadel(&f.game).unwrap()).unwrap());
        let mut doc=st.document().unwrap();fn legacy(v:&mut JsonValue,drift:bool){if v.is_object(){if let Some(d)=v["device"].as_u64(){if drift{v["device"]=(d+1).into();}v.remove("fsid");}let keys=v.entries().map(|(k,_)|k.to_owned()).collect::<Vec<_>>();for k in keys{legacy(&mut v[&k],drift);}}}
        legacy(&mut doc["applied"],drift);item93_write_document(&st,&doc);drop(st);drop(lock);
        let mut service=crate::native::NativeService::new();let value=service.dispatch("native.snapshot",&object!{dataDir:f.data.to_str()}).unwrap();
        // Drift without fsid is admitted only under held-root btrfs/overlay authority.
        assert_eq!(value["recoveryRequired"]==true,drift&&!supported,"drift={drift} supported={supported}: {value}");
        assert_eq!(value["pendingChanges"].as_bool(),Some(drift&&!supported),"drift={drift} supported={supported}: {value}");
        assert_eq!(Store::open(&f.data).unwrap().document().unwrap(),doc,"read admission does not upgrade old proof");
        }
    }

    #[test]
    fn item93_reboot_apply_restore_play_runtime() {
        {
            let (f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
            if !paths::test_volatile_filesystem(&File::open(&c).unwrap()) {return;}
            let input=|id:&str,r:u64|{let mut v=object!{method:"native.apply",params:object!{operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef"}};{v["params"]["deploymentLayout"]="shards-v1".into();}v};
            for (id,r) in [("first93",1),("second93",2)] {
                ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,r,id, &game_input("native.apply", r, id, &input(id,r))).unwrap();
            }
            let mut doc=st.document().unwrap();
            for key in ["applied","retiredGenerations"] {item93_renumber(&mut doc[key]);}if doc["previous"].is_object(){item93_renumber(&mut doc["previous"]["applied"]);}
            item93_write_document(&st,&doc);
            let guard=GameLock::acquire(&c).unwrap();let control=crate::prepare_control::PrepareControl::never_cancelled();
            let play=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).unwrap();
            let request=play.operation_input("play93").unwrap();
            play.execute(&st,&lock,&guard,3,"play93",&request,&control,&mut |_|{}).unwrap().proof.revalidate(&st,&lock,&guard).unwrap();drop(guard);
            let r=rev(&st.document().unwrap()).unwrap();
            restore(&st,&lock,r,"restore93", &game_input("native.restore", r, "restore93", &object!{method:"native.restore",params:object!{operationId:"restore93",expectedRevision:r.to_string()}})).unwrap();
            let r=r+1;
            let layout=true;
            let mut request=input("again93",r);if layout{request["params"]["deploymentLayout"]="shards-v1".into();}
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,r,"again93", &game_input("native.apply", r, "again93", &request)).unwrap();
            assert_eq!(st.document().unwrap()["recoveryRequired"],false);assert!(st.journals().unwrap().is_empty());
            let good=st.document().unwrap();let current=rev(&good).unwrap();
            for field in ["inode","fsid"] {
                let mut foreign=good.clone();
                foreign["applied"]["directoryIdentity"][field]=if field=="inode" {(foreign["applied"]["directoryIdentity"][field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};
                item93_write_document(&st,&foreign);
                assert!(ApplyPlan::review(&st,"default",&f.game).is_err());
                assert!(restore(&st,&lock,current,"foreign93", &game_input("native.restore", current, "foreign93", &object!{method:"native.restore",params:object!{operationId:"foreign93",expectedRevision:current.to_string()}})).is_err());
                let guard=GameLock::acquire(&c).unwrap();assert!(crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).is_err());drop(guard);
                item93_write_document(&st,&good);
            }
        }
    }

    #[test]
    fn item93_checkpoint_device_change_keeps_complete_tree_checks() {
        use std::os::unix::fs::PermissionsExt;
        let (f,lock,st,_)=setup();
        let input=object!{method:"native.apply",params:object!{operationId:"device93",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"device93", &game_input("native.apply", 1, "device93", &input)).unwrap();
        let c=citadel(&f.game).unwrap();
        let guard=GameLock::acquire(&c).unwrap();
        let original=st.document().unwrap()["applied"].clone();
        let mut old=original.clone();
        fn renumber(v:&mut JsonValue) {
            if v.is_object() {
                if let Some(d)=v["device"].as_u64() {v["device"]=(d+1).into();v.remove("fsid");}
                let keys=v.entries().map(|(k,_)|k.to_owned()).collect::<Vec<_>>();
                for k in keys {renumber(&mut v[&k]);}
            }
        }
        renumber(&mut old);
        if !paths::test_volatile_filesystem(&File::open(&c).unwrap()) {return;}
        assert!(shards::check_checkpoint(&c,st.root(),&old,false, None).is_err(),"unlocked legacy comparison stays strict");
        shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).unwrap();
        assert_ne!(old["directoryIdentity"],original["directoryIdentity"],"no evidence is rewritten");
        let mut foreign=old.clone();foreign["directoryIdentity"]["inode"]=(fs::metadata(generation_dir(&c,&old).unwrap()).unwrap().ino()+1).into();
        assert!(shards::verify_checkpoint_locked(&c,st.root(),&foreign,false,&guard).is_err());
        let mut foreign=old.clone();foreign["directoryIdentity"]["fsid"]="0000000000000000".into();
        assert!(shards::verify_checkpoint_locked(&c,st.root(),&foreign,false,&guard).is_err());
        let dir=generation_dir(&c,&old).unwrap();
        fs::write(dir.join("foreign"),b"foreign").unwrap();
        assert!(shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).is_err());
        fs::remove_file(dir.join("foreign")).unwrap();
        let m=fs::metadata(&dir).unwrap();
        old["directoryIdentity"]["modifiedSeconds"]=m.mtime().into();
        old["directoryIdentity"]["modifiedNanos"]=m.mtime_nsec().into();
        old["directoryIdentity"]["size"]=m.len().into();
        shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).unwrap();
        let view=generation::decode(&old["manifest"]).unwrap();let path=dir.join(&view.files[0].name);
        fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();
        shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).unwrap();
        let mut modern=old.clone();modern["identities"][&view.files[0].name]["fsid"]=paths::persisted_filesystem_id(&File::open(&path).unwrap()).unwrap().into();
        assert!(shards::verify_checkpoint_locked(&c,st.root(),&modern,false,&guard).is_err(),"new-format mode checks remain strict");
        fs::set_permissions(&path,fs::Permissions::from_mode(0o444)).unwrap();
        shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).unwrap();
        let mut bytes=fs::read(&path).unwrap();let last=bytes.len()-1;bytes[last]^=1;
        fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();fs::write(&path,bytes).unwrap();
        fs::set_permissions(&path,fs::Permissions::from_mode(0o444)).unwrap();
        assert!(shards::verify_checkpoint_locked(&c,st.root(),&old,false,&guard).is_err());
    }

    #[test]
    fn item104_apply_hashes_each_payload_at_most_once() {
        let (f, lock, st, _) = setup_padded(1024 * 1024);
        let input=|id:&str,r:&str|object!{method:"native.apply",params:object!{operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"initial104", &game_input("native.apply", 1, "initial104", &input("initial104","1"))).unwrap();
        let old=st.document().unwrap()["applied"].clone();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"again104", &game_input("native.apply", 2, "again104", &input("again104","2"))).unwrap();
        let new=st.document().unwrap()["applied"].clone();
        for file in generation::decode(&new["manifest"]).unwrap().files {
            let c=citadel(&f.game).unwrap();
            let old_path=generation_dir(&c,&old).unwrap().join(&file.name);
            let new_path=generation_dir(&c,&new).unwrap().join(&file.name);
            if fs::metadata(&old_path).unwrap().ino()==fs::metadata(&new_path).unwrap().ino(){
                paths::HASH_READS.with(|r|assert!(r.borrow().get(&old_path).copied().unwrap_or(0)+r.borrow().get(&new_path).copied().unwrap_or(0)<=1,"linked aliases must share one content hash"));
            }
        }
        paths::HASH_READS.with(|r|for(p,n)in r.borrow().iter().filter(|(p,_)|p.extension().is_some_and(|e|e=="vpk")){assert!(*n<=1,"{} hashed {} times",p.display(),n);});
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,3,"third104", &game_input("native.apply", 3, "third104", &input("third104","3"))).unwrap();
        paths::HASH_READS.with(|r|for(p,n)in r.borrow().iter().filter(|(p,_)|p.extension().is_some_and(|e|e=="vpk")){assert!(*n<=1,"{} hashed {} times during retirement",p.display(),n);});

    }
    #[test]
    fn item104_f6_moved_storage_reuses_verified_payloads_and_repairs() {
        let (f,lock,st,_)=setup_padded(1024);
        drop(st);drop(lock);
        let destination=f.root.join("bulk-drive");fs::create_dir(&destination).unwrap();
        let mut params=object!{dataDir:f.data.to_str(),destination:destination.to_str()};
        let review=crate::storage::dispatch("native.storage.review",&params).unwrap();
        params["token"]=review["token"].clone();params["operationId"]="move-f6".into();
        crate::storage::dispatch("native.storage.move",&params).unwrap();
        let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let revision=rev(&st.document().unwrap()).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=object!{method:"native.apply",params:object!{operationId:"apply-f6",expectedRevision:revision.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&st,&lock,revision,"apply-f6", &game_input("native.apply", revision, "apply-f6", &input)).unwrap();
        let bulk=st.storage_root().unwrap();assert_ne!(bulk,st.root());
        assert!(plan.payload_evidence.iter().any(|(path,_)|path.starts_with(bulk.join("payloads"))));
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let fresh=Store::open(&f.data).unwrap();
        ApplyPlan::review(&fresh,"default",&f.game).unwrap();
        paths::HASH_READS.with(|r|assert!(!r.borrow().keys().any(|path|path.starts_with(&bulk)),"moved immutable files were hashed again: {:?}",r.borrow()));
        // Exercise the repair-cache root too, including a real matching receipt.
        let bytes=crate::test_support::vpk("scripts/f6.vdata_c",0);
        let sha=paths::digest_bytes(&bytes).unwrap();let cache=bulk.join("cache/repairs");paths::private_dir(&cache).unwrap();
        let repair=cache.join(format!("{sha}.vpk"));fs::write(&repair,bytes).unwrap();paths::make_readonly(&repair).unwrap();
        let stamp=paths::content_stamp(&paths::open_regular(&repair).unwrap()).unwrap();
        st.launch_database().execute("INSERT INTO apply_verified_files(path,sha256,stamp) VALUES(?,?,?)",&[&repair.to_string_lossy(),&sha,&stamp]).unwrap();
        let _scope=paths::HashMemo::default().enter();fresh.seed_apply_hashes().unwrap();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        assert_eq!(paths::fingerprint(&repair).unwrap().sha256,sha);
        paths::HASH_READS.with(|r|assert!(!r.borrow().contains_key(&repair),"moved repair receipt must seed its verified content"));
    }
    #[test]
    fn item104_f7_retired_receipt_rows_stay_bounded() {
        let (f,lock,st,_)=setup_padded(1024);
        let mut first_paths=Vec::new();let mut steady_count=None;
        for revision in 1..=5 {
            let id=format!("apply-f7-{revision}");
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let input=object!{method:"native.apply",params:object!{operationId:id.clone(),expectedRevision:revision.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            plan.apply(&st,&lock,revision,&id, &game_input("native.apply", revision, &id, &input)).unwrap();
            let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();
            if revision==1 {
                first_paths=generation::decode(&doc["applied"]["manifest"]).unwrap().files.iter().map(|file|generation_dir(&c,&doc["applied"]).unwrap().join(&file.name)).collect();
            }
            let db=st.launch_database();
            let count=db.query("SELECT COUNT(*) FROM apply_verified_files",&[]).unwrap().unwrap()[0].parse::<usize>().unwrap();
            if revision==2 {steady_count=Some(count);}
            if revision>=3 {
                for path in &first_paths {
                    assert!(!path.exists());
                    assert!(db.query("SELECT path FROM apply_verified_files WHERE path=?",&[&path.to_string_lossy()]).unwrap().is_none(),"retired receipt remains: {}",path.display());
                }
                assert_eq!(Some(count),steady_count,"receipt count must stop growing after two retained generations");
            }
            for a in [&doc["applied"],&doc["previous"]["applied"]].into_iter().filter(|a|!a.is_null()) {
                for file in generation::decode(&a["manifest"]).unwrap().files {
                    let path=generation_dir(&c,a).unwrap().join(file.name);
                    assert!(db.query("SELECT path FROM apply_verified_files WHERE path=?",&[&path.to_string_lossy()]).unwrap().is_some(),"live receipt must survive: {}",path.display());
                }
            }
        }
    }
    #[test]
    fn item104_readonly_receipt_reuses_content_across_reviews() {
        let (f,lock,st,_)=setup_padded(1024);
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let source=plan.payload_evidence.iter().find(|(p,_)|p.parent()==Some(st.root().join("payloads").as_path())).unwrap().0.clone();
        let input=object!{method:"native.apply",params:object!{operationId:"receipt104",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&st,&lock,1,"receipt104", &game_input("native.apply", 1, "receipt104", &input)).unwrap();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let reopened=Store::open(st.root()).unwrap();
        ApplyPlan::review(&reopened,"default",&f.game).unwrap();
        paths::HASH_READS.with(|r|assert_eq!(r.borrow().get(&source).copied().unwrap_or(0),0,"an unchanged readonly receipt survives a fresh Store"));
    }
    #[test]
    fn item104_final_gameinfo_guard_rejects_restored_mtime_edit() {
        use std::os::unix::fs::PermissionsExt;
        let (f,lock,st,_)=setup_padded(1024);
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let c=citadel(&f.game).unwrap();let before=fs::read(c.join("gameinfo.gi")).unwrap();
        let file=generation::decode(&plan.manifest(&st)).unwrap().files[0].name.clone();
        let path=c.join("deadlock_native").join(&plan.generation).join(file);
        BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(Box::new(move |_|{
            let old=fs::metadata(&path).unwrap();let mut bytes=fs::read(&path).unwrap();let last=bytes.len()-1;bytes[last]^=1;
            fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();fs::write(&path,bytes).unwrap();
            File::options().write(true).open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(old.modified().unwrap())).unwrap();
        })));
        let input=object!{method:"native.apply",params:object!{operationId:"final104",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        assert!(plan.apply(&st,&lock,1,"final104", &game_input("native.apply", 1, "final104", &input)).is_err());
        BEFORE_FINAL_RENAME.with(|h|assert!(h.borrow().is_none()));
        assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),before);
    }
    #[test]
    fn item104_play_after_apply_reads_only_identities() {
        let (f,lock,st,_)=setup_padded(8*1024*1024);
        let input=object!{method:"native.apply",params:object!{operationId:"play104",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"play104", &game_input("native.apply", 1, "play104", &input)).unwrap();
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let control=crate::prepare_control::PrepareControl::never_cancelled();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let start=std::time::Instant::now();let bytes=crate::game_access::test_file_rchar();
        let plan=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).unwrap();
        assert!(matches!(plan,crate::launch_preparation::PreparationPlan::Verify(_)));
        let request=plan.operation_input("verify104").unwrap();
        let prepared=plan.execute(&st,&lock,&guard,2,"verify104",&request,&control,&mut |_|{}).unwrap();
        prepared.proof.revalidate(&st,&lock,&guard).unwrap();
        let reads=crate::game_access::test_file_rchar()-bytes;
        eprintln!("ITEM104_PLAY ms={:.3} rchar={reads}",start.elapsed().as_secs_f64()*1000.0);
        assert!(reads<1024*1024,"unchanged Play must not reread the 8 MiB payload: {reads}");
        paths::HASH_READS.with(|r|assert!(!r.borrow().keys().any(|p|p.extension().is_some_and(|e|e=="vpk")),"unchanged Play content hashes: {:?}",r.borrow()));
    }
    #[test]
    fn item104_apply_refuses_ctime_change_after_preview() {
        use std::os::unix::fs::PermissionsExt;
        let (f, lock, st, _) = setup_padded(1024);
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let path=&plan.payload_evidence[0].0;
        let old=fs::metadata(path).unwrap();
        let mut bytes=fs::read(path).unwrap();let last=bytes.len()-1;bytes[last]^=1;
        fs::set_permissions(path,fs::Permissions::from_mode(0o600)).unwrap();fs::write(path,bytes).unwrap();
        File::options().write(true).open(path).unwrap().set_times(fs::FileTimes::new().set_modified(old.modified().unwrap())).unwrap();
        let input=object!{method:"native.apply",params:object!{operationId:"changed104",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        assert_eq!(plan.apply(&st,&lock,1,"changed104", &game_input("native.apply", 1, "changed104", &input)).unwrap_err().kind,"stalePreview");
    }
    #[test]
    fn apply_review_hashes_unchanged_payload_once() {
        let (f, _lock, st, _) = setup_padded(1024 * 1024);
        ApplyPlan::review(&st,"default",&f.game).unwrap();
        let doc = st.document().unwrap();
        let version = package::layout_version(&doc).unwrap();
        let row = &doc["profiles"]["default"]["mods"][0];
        let payload = st.payload_path(&package::selected_packages_exact(row, version).unwrap()[0].directory.id).unwrap();
        paths::HASH_READS.with(|reads| reads.borrow_mut().clear());
        ApplyPlan::review(&st,"default",&f.game).unwrap();
        paths::HASH_READS.with(|reads| assert_eq!(reads.borrow().get(&payload).copied().unwrap_or(0), 1, "the same payload must be hashed once per warm review"));
    }
    #[test]
    fn apply_review_scans_game_resources_once() {
        let (f, _lock, st, _) = setup();
        ApplyPlan::review(&st,"default",&f.game).unwrap();
        crate::health::STAMP_WALKS.with(|n| n.set(0));
        crate::repairs::resource_membership(&f.game).unwrap();
        let expected = crate::health::STAMP_WALKS.with(|n| n.replace(0));
        let plan = ApplyPlan::review(&st,"default",&f.game).unwrap();
        let actual = crate::health::STAMP_WALKS.with(|n| n.get());
        assert_eq!(actual, expected, "review must capture resources in one walk");
        fs::write(f.game.join("game/citadel/new-resource.vpk"), b"new").unwrap();
        assert_eq!(plan.recheck(&st).unwrap_err().kind, "stalePreview");
    }
    #[test]
    fn review6_single_8mib_sharded_restore_io() {
        fn rchar()->u64 {crate::game_access::test_file_rchar()}
        let(f,lock,st,_)=setup_padded(8*1024*1024);
        let input=|id:&str,r:&str|object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1"))).unwrap();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"second", &game_input("native.apply", 2, "second", &input("second","2"))).unwrap();
        let doc=st.document().unwrap();
        let lengths:Vec<u64>=[&doc["applied"],&doc["previous"]["applied"]].iter().map(|a|generation::decode(&a["manifest"]).unwrap().files.iter().map(|f|f.payload.size).sum()).collect();
        let mut i=input("restore","3");i["method"]="native.restore".into();
        let before=rchar();restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &i)).unwrap();let reads=rchar()-before;
        assert!(reads<=4*(lengths[0]+lengths[1]));
        eprintln!("REVIEW6_IO reads={} current={} target={} over_one={:.3} over_both={:.3}",reads,lengths[0],lengths[1],reads as f64/lengths[0] as f64,reads as f64/(lengths[0]+lengths[1]) as f64);
    }
    #[test]
    fn review6_retirement_drift_at_publish() {
        let(f,lock,st,_)=setup();
        let input=|id:&str,r:&str|object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1"))).unwrap();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"second", &game_input("native.apply", 2, "second", &input("second","2"))).unwrap();
        let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();let live=c.join("gameinfo.gi");let before=text(&live).unwrap();
        let relative=generation::decode(&doc["applied"]["manifest"]).unwrap().files[0].name.clone();
        let path=generation_dir(&c,&doc["applied"]).unwrap().join(relative);
        shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(move||{
            let metadata=fs::metadata(&path).unwrap();let modified=metadata.modified().unwrap();let permissions=metadata.permissions();use std::os::unix::fs::PermissionsExt;fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();let mut bytes=fs::read(&path).unwrap();let last=bytes.len()-1;bytes[last]^=1;fs::write(&path,bytes).unwrap();
            fs::File::options().write(true).open(&path).unwrap().set_times(fs::FileTimes::new().set_modified(modified)).unwrap();fs::set_permissions(&path,permissions).unwrap();
        })));
        let mut i=input("restore","3");i["method"]="native.restore".into();let e=restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &i)).unwrap_err();
        let after=text(&live).unwrap();let pending=st.journals().unwrap().len();
        eprintln!("REVIEW6_RETIRE error={} live_changed={} pending_journals={}",e.kind,after!=before,pending);
        assert_eq!(after,before);assert!(pending>0);
    }
    #[test]
    fn round5_strong_checkpoint_includes_repair() {
        let(f,lock,st,_)=setup();let mut p=ApplyPlan::review(&st,"default",&f.game).unwrap();attach_synthetic_repair(&mut p,&st);
        let i=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"repair",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        p.apply(&st,&lock,1,"repair", &game_input("native.apply", 1, "repair", &i)).unwrap();let doc=st.document().unwrap();let c=citadel(&f.game).unwrap();let path=generation_dir(&c,&doc["applied"]).unwrap().join("repairs/pak01_dir.vpk");
        paths::HASH_READS.with(|r|r.borrow_mut().clear());shards::check_checkpoint(&c,st.root(),&doc["applied"],false, None).unwrap();
        paths::HASH_READS.with(|r|assert_eq!(r.borrow().get(&path).copied().unwrap_or(0),1,"strong checkpoint must hash repair bytes once"));
    }
    #[test]
    fn round5_restore_read_bound() {
        fn rchar()->u64 {crate::game_access::test_file_rchar()}
        let mut results=Vec::new();
        {
            let(f,lock,st,_)=family_test_setup_padded(4*1024*1024);
            let input=|id:&str,r:&str|{let mut i=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef"}};{i["params"]["deploymentLayout"]="shards-v1".into();}i};
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &{let mut i=input("first","1");{}i})).unwrap();
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"second", &game_input("native.apply", 2, "second", &input("second","2"))).unwrap();
            let doc=st.document().unwrap();let bytes:u64=[&doc["applied"],&doc["previous"]["applied"]].into_iter().flat_map(|a|generation::decode(&a["manifest"]).unwrap().files).map(|f|f.payload.size).sum();
            let mut i=input("restore","3");i["method"]="native.restore".into();
            let before=rchar();restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &i)).unwrap();let read=rchar()-before;
            eprintln!("ROUND5 Restore shards-v1: read={read} bytes={bytes} amplification={:.3}",read as f64/bytes as f64);
            results.push((read,bytes));
        }
        assert!(results.iter().all(|(r,b)|*r<=4*b),"Restore bounds: {results:?}");
    }
    #[test]
    fn round5_checkpoint_survives_profile_removal() {
        let f=Fixture::new();let c=citadel(&f.game).unwrap();let dir=c.join("addons");
        let rows=crate::installed::mount_proof(&dir).unwrap();
        let origin=object!{gamePath:f.game.to_str(),mount:"citadel/addons",directory:dir.to_str(),files:rows.clone()};
        let mut setup=object!{gameinfo:"GameInfo { FileSystem { SearchPaths { Game citadel/addons } } }",upstreamEvidenceVersion:2,upstreamPayloads:rows};
        fs::write(dir.join("extra.vpk"),b"extra").unwrap();
        assert!(check_installed_baselines(&setup, &c, true, None).is_err(),"missing legacy baseline must refuse");
        setup["installedGameOrigins"]=json::array![origin];
        assert!(check_installed_baselines(&setup, &c, true, None).is_err(),"owned baseline must detect added VPK after profile removal");
    }
    #[test]
    fn round4_mixed_post_restore_drift() {
        let(f,lock,st,_)=setup();
        let input=|id:&str,r:&str|object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1"))).unwrap();
        let c=citadel(&f.game).unwrap();let gi=c.join("gameinfo.gi");let dir=c.join("dmm_localization_default");fs::create_dir(&dir).unwrap();fs::write(dir.join("old.vpk"),b"old").unwrap();
        fs::write(&gi,text(&gi).unwrap().replace("Game citadel\n","Game citadel/dmm_localization_default\n Game citadel\n")).unwrap();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"second", &game_input("native.apply", 2, "second", &input("second","2"))).unwrap();
        let target=st.document().unwrap()["previous"]["applied"].clone();let target_dir=generation_dir(&c,&target).unwrap();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let mut i=input("restore","3");i["method"]="native.restore".into();restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &i)).unwrap();
        paths::HASH_READS.with(|r|for(p,n)in r.borrow().iter().filter(|(p,_)|p.starts_with(&target_dir)||p.starts_with(&dir)){assert!(*n<=2,"{} hashed {} times",p.display(),n);});
        fs::write(dir.join("extra.vpk"),b"drift after Restore").unwrap();
        let doc=st.document().unwrap();
        assert!(check_snapshot(st.root(),&doc, None).is_err(),"mixed snapshot must verify restored repair proof");
        assert!(check_applied(st.root(),&doc, None).is_err(),"mixed Play/forward verification must verify restored repair proof");
    }
    #[test]
    fn round4_legacy_overlap_membership() {
        let f=Fixture::new();let dir=f.game.join("game/citadel/addons");fs::write(dir.join("other.vpk"),b"other").unwrap();let mut rows:JsonValue=crate::installed::mount_proof(&dir).unwrap().members().filter(|r|r["sourcePath"].as_str().unwrap().ends_with(".vpk")).cloned().collect::<Vec<_>>().into();
        rows[0]["installedOrigin"]=true.into(); // old writer deduplicated the manifest-owned row
        verify_engine_proof(&rows,false).expect("manifest overlap belongs to the same baseline");
        for row in rows.members_mut(){row.remove("installedOrigin");}
        let mut setup=object!{gameinfo:"GameInfo { FileSystem { SearchPaths { Game citadel/addons } } }",upstreamEvidenceVersion:2,upstreamPayloads:rows};
        fs::write(dir.join("extra.vpk"),b"added").unwrap();
        assert!(check_installed_baselines(&setup, &citadel(&f.game).unwrap(), true, None).is_err(),"all-overlap baseline must reject added VPK");
        setup.remove("gameinfo");
    }
    #[test]
    fn round4_foreign_game_repair_is_not_proof() {
        let f=Fixture::new();let c=citadel(&f.game).unwrap();
        let dir=f.root.join("other/game/citadel/dmm_localization_default");fs::create_dir_all(&dir).unwrap();
        let setup=object!{gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/dmm_localization_default\n Game citadel\n }\n }\n}\n",upstreamEvidenceVersion:2,upstreamPayloads:json::array![object!{proofKind:"mount",sourcePath:dir.to_str(),files:json::array![]}]};
        assert!(check_upstream(&setup, false, &c, None).is_err(),"another game's repair folder is not current-game evidence");
    }
    #[test]
    fn round3_nested_repair_name_is_not_proof() {
        let f=Fixture::new();let c=citadel(&f.game).unwrap();let dir=c.join("addons/dmm_localization_default");fs::create_dir_all(&dir).unwrap();
        let setup=object!{gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/dmm_localization_default\n Game citadel\n }\n }\n}\n",upstreamEvidenceVersion:2,upstreamPayloads:json::array![object!{proofKind:"mount",sourcePath:dir.to_str(),files:json::array![]}]};
        assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err(),"nested same-name mount cannot prove root repair");
    }
    #[test]
    fn round3_restore_cache_rejects_restored_mtime_rewrite() {
        let f=Fixture::new();let p=f.root.join("bytes");fs::write(&p,b"AAAA").unwrap();let m=fs::metadata(&p).unwrap();
        let proof=object!{sha256:paths::fingerprint(&p).unwrap().sha256,size:4};verify_file(&p,&proof).unwrap();
        fs::write(&p,b"BBBB").unwrap();fs::File::options().write(true).open(&p).unwrap().set_times(fs::FileTimes::new().set_modified(m.modified().unwrap())).unwrap();
        assert!(verify_file(&p,&proof).is_err(),"same-size rewrite with restored mtime must be hashed");
    }
    #[test]
    fn restore_rechecks_old_folder_at_publish_boundary() {
        let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let dir=c.join("dmm_localization_default");fs::create_dir(&dir).unwrap();fs::write(dir.join("old.vpk"),b"old").unwrap();
        let gi=c.join("gameinfo.gi");let before=text(&gi).unwrap().replace("Game citadel\n","Game citadel/dmm_localization_default\n Game citadel\n");fs::write(&gi,&before).unwrap();
        let input=|id:&str,r:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1","native.apply"))).unwrap();let native=text(&gi).unwrap();
        shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(move||{fs::write(dir.join("extra.vpk"),b"added").unwrap();})));
        assert!(restore(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &input("restore","2","native.restore"))).is_err());
        assert_eq!(text(&gi).unwrap(),native,"changed folder must fail before gameinfo rename");
    }
    #[test]
    fn legacy_2778_interrupted_apply_journal_rolls_back() {
        for forward in [false,true] {
        let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let repair=c.join("dmm_localization_default");fs::create_dir(&repair).unwrap();fs::write(repair.join("old.vpk"),b"old repair").unwrap();
        fs::write(c.join("gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel/dmm_localization_default\n Game citadel\n }\n }\n}\n").unwrap();
        let input=|id:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let guard=GameLock::acquire(&c).unwrap();
        let stopped=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,1,"first",&input("first","native.apply"),&crate::prepare_control::PrepareControl::never_cancelled(),&mut |p|if matches!(p,crate::launch_preparation::PreparationProgress::Committing){panic!("stop");})));
        assert!(stopped.is_err());drop(guard);let mut j=st.journals().unwrap()[0]["body"].clone();
        j["document"]["previous"]["upstreamPayloads"]=j["document"]["previous"]["upstreamPayloads"].members().filter(|p|p["proofKind"]!="mount").cloned().collect::<Vec<_>>().into();
        if forward {
            let temp=c.join(j["temp"].as_str().unwrap());let mut out=new_file(&temp).unwrap();j["tempIdentity"]=identity(&out.metadata().unwrap());
            out.write_all(j["after"].as_str().unwrap().as_bytes()).unwrap();out.sync_all().unwrap();fs::rename(&temp,c.join("gameinfo.gi")).unwrap();
        }
        st.write_journal(&lock,"first",&j).unwrap();recover(&st,&lock,1,"recover", &game_input("native.recover", 1, "recover", &input("recover","native.recover"))).unwrap();assert!(st.journals().unwrap().is_empty());
            }
    }
    #[test]
    fn mixed_native_repair_apply_interruption_recovery() {
        let(f,lock,st,_)=setup();let input=|id:&str,r:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let mut first=ApplyPlan::review(&st,"default",&f.game).unwrap();attach_synthetic_repair(&mut first,&st);first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1","native.apply"))).unwrap();
        let c=citadel(&f.game).unwrap();let path=c.join("gameinfo.gi");let old=text(&path).unwrap().replace("Game citadel\n", "Game citadel/dmm_localization_default\n Game citadel\n");
        let dir=c.join("dmm_localization_default");fs::create_dir(&dir).unwrap();fs::write(dir.join("old.vpk"),b"old repair").unwrap();fs::write(&path,&old).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let guard=GameLock::acquire(&c).unwrap();
        let stopped=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,2,"again",&input("again","2","native.apply"),&crate::prepare_control::PrepareControl::never_cancelled(),&mut |p|if matches!(p,crate::launch_preparation::PreparationProgress::Committing){panic!("stop");})));
        assert!(stopped.is_err());drop(guard);
        recover(&st,&lock,2,"recover", &game_input("native.recover", 2, "recover", &input("recover","2","native.recover"))).unwrap();assert_eq!(text(&path).unwrap(),old);
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();plan.apply(&st,&lock,2,"final", &game_input("native.apply", 2, "final", &input("final","2","native.apply"))).unwrap();
        restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &input("restore","3","native.restore"))).unwrap();assert_eq!(text(&path).unwrap(),old);
    }
    #[test]
    fn snapshot_mount_proof_has_zero_hashes() {
        let f=Fixture::new();let dir=f.game.join("game/citadel/addons");
        let files=crate::installed::mount_proof(&dir).unwrap();
        let p=object!{treeComplete:true,treeVersion:1,proofKind:"mount",sourcePath:dir.to_str(),files:files};
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        verify_prior_source(&p,true).unwrap();
        paths::HASH_READS.with(|r|assert!(r.borrow().is_empty()));
    }
    #[test]
    fn legacy_2778_origin_intact_restores_added_vpk_refused() {
        let f=Fixture::new();let dir=f.game.join("game/citadel/addons");
        let mut rows:JsonValue=crate::installed::mount_proof(&dir).unwrap().members().filter(|r|r["sourcePath"].as_str().unwrap().ends_with(".vpk")).cloned().collect::<Vec<_>>().into();
        for p in rows.members_mut(){p["installedOrigin"]=true.into();}
        let setup=object!{upstreamEvidenceVersion:2,upstreamPayloads:rows,gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n"};
        check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).expect("2778c01 per-file evidence must remain usable");
        fs::write(dir.join("added.vpk"),b"extra").unwrap();
        assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err());
    }
    #[test]
    fn legacy_upstream_repair_mount_without_proof_is_refused() {
        let f=Fixture::new();
        let setup=object!{upstreamEvidenceVersion:2,upstreamPayloads:json::array![],gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/dmm_localization_default\n Game citadel\n }\n }\n}\n"};
        assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err());
    }
    #[test]
    fn interrupted_repair_copy_rolls_back_from_recorded_cache_source() {
        let(f,lock,st,_)=setup();let mut plan=ApplyPlan::review(&st,"default",&f.game).unwrap();attach_synthetic_repair_padded(&mut plan,&st,128*1024);
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"repair",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let stopped=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,1,"repair",&input,&crate::prepare_control::PrepareControl::never_cancelled(),&mut |event| {
            if matches!(event,crate::launch_preparation::PreparationProgress::Copying{..}) {
                let journals=st.journals().unwrap();let rows=&journals[0]["body"]["stagedNodes"];
                if rows.members().last().is_some_and(|r|r["name"].as_str().is_some_and(|n|n.contains("repairs/") )) {panic!("repair copy boundary");}
            }
        })));
        assert!(stopped.is_err());drop(guard);
        let j=st.journals().unwrap()[0]["body"].clone();let n=j["stagedNodes"].len()-1;
        let path=f.game.join("game/citadel/deadlock_native").join(&plan.generation).join(j["stagedNodes"][n]["name"].as_str().unwrap());
        assert_eq!(j["stagedNodes"][n]["complete"],false);
        let size=fs::metadata(&path).unwrap().len();assert!(size>0 && size<plan.repairs["file"]["size"].as_u64().unwrap());
        recover(&st,&lock,1,"recover", &game_input("native.recover", 1, "recover", &object!{method:"native.recover",params:object!{operationId:"recover",expectedRevision:"1"}})).unwrap();
        assert!(!path.exists());assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn partial_external_mount_removal_can_reapply_and_restore() {
        let(f,lock,st,_)=setup();let mut first=ApplyPlan::review(&st,"default",&f.game).unwrap();attach_synthetic_repair(&mut first,&st);
        let input=|id:&str,r:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first","1","native.apply"))).unwrap();
        let full=st.document().unwrap();
        let path=f.game.join("game/citadel/gameinfo.gi");let current=text(&path).unwrap().lines().filter(|l|!l.contains("/shard01")).collect::<Vec<_>>().join("\n");fs::write(&path,&current).unwrap();
        let again=ApplyPlan::review(&st,"default",&f.game).unwrap();again.apply(&st,&lock,2,"again", &game_input("native.apply", 2, "again", &input("again","2","native.apply"))).unwrap();
        restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &input("restore","3","native.restore"))).unwrap();assert_eq!(text(&path).unwrap(),current);
        assert!(crate::deployment_intent::PendingState::compare(&st.document().unwrap(),"default").unwrap().pending,"partial mount needs Apply");
        crate::launch_preparation::assert_partial_play_requires_apply(first,full,st.document().unwrap());
    }
    #[test]
    fn restore_rechecks_installed_mount_membership_and_bytes() {
        for addition in [true,false] {
            let f=Fixture::new();let c=f.game.join("game/citadel");let dir=c.join("addons");
            let files=crate::installed::mount_proof(&dir).unwrap();
            let mut proofs=files.clone();for p in proofs.members_mut(){p["installedOrigin"]=true.into();}
            proofs.push(object!{treeComplete:true,treeVersion:1,proofKind:"mount",sourcePath:dir.to_str(),files:files.clone()}).unwrap();
            let setup=object!{upstreamEvidenceVersion:2,upstreamPayloads:proofs,gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n"};
            if addition {fs::write(dir.join("extra.vpk"),b"extra").unwrap();}
            else {let path=Path::new(setup["upstreamPayloads"][0]["sourcePath"].as_str().unwrap());let mut bytes=fs::read(path).unwrap();bytes[0]^=1;let file=std::fs::OpenOptions::new().write(true).open(path).unwrap();let modified=file.metadata().unwrap().modified().unwrap();fs::write(path,bytes).unwrap();file.set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();}
            assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err());
        }
    }
    #[test]
    fn round13_case_differing_mounts_refused() {
        { for token in [false,true] {
            for (literal,actual) in [("citadel/dmm_localization_default","DMM_Localization_Default"),("citadel/addons","Addons"),("Citadel/dmm_localization_default","dmm_localization_default"),("citadel/nested/dmm_localization_default","Nested/dmm_localization_default")] {
                let f=Fixture::new(); let c=citadel(&f.game).unwrap();
                let mount=if token {literal.replacen("citadel/","|gameinfo_path|",1)}else{literal.into()};
                // The nested case exercises the shared resolver directly; ordinary repair/addons exercise Apply.
                if literal.contains("nested") {
                    fs::create_dir_all(c.join(actual)).unwrap();
                    assert!(gameinfo::mount_directory(&c,&mount).is_err(),"accepted {mount} -> {actual}");continue;
                }
                if actual=="Addons" {fs::rename(c.join("addons"),c.join(actual)).unwrap();}
                else {fs::create_dir_all(c.join(actual)).unwrap();}
                let before=format!("GameInfo\n{{\n FileSystem\n {{\n SearchPaths\n {{\n Game {mount}\n Game citadel\n }}\n }}\n}}\n");
                fs::write(c.join("gameinfo.gi"),&before).unwrap();
                let mut service=crate::native::NativeService::new();import(&mut service,&f);
                let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
                assert!(ApplyPlan::review(&st,"default",&f.game).is_err(),"accepted {mount} -> {actual}, sharded=shards-v1");
                assert_eq!(text(&c.join("gameinfo.gi")).unwrap(),before);assert!(st.journals().unwrap().is_empty());
            }
        }}
    }
    #[test]
    fn round13_exact_mount_apply_restore_and_entry_types() {
        {for token in [false,true] {for repair in [false,true] {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
            let name=if repair {"dmm_localization_default"}else{"addons"};let dir=c.join(name);
            if repair {fs::create_dir(&dir).unwrap();fs::write(dir.join("loose.dat"),b"original").unwrap();}
            let mount=if token {format!("|gameinfo_path|{name}")}else{format!("citadel/{name}")};
            let gi=c.join("gameinfo.gi");let before=format!("GameInfo\n{{\n FileSystem\n {{\n SearchPaths\n {{\n Game {mount}\n Game citadel\n }}\n }}\n}}\n");fs::write(&gi,&before).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let mut input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            {}
            plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();let applied=text(&gi).unwrap();
            let renamed=c.join(name.to_ascii_uppercase());fs::rename(&dir,&renamed).unwrap();
            assert!(restore(&st,&lock,2,"refused", &game_input("native.restore", 2, "refused", &object!{method:"native.restore",params:object!{operationId:"refused",expectedRevision:"2",dataDir:f.data.to_str()}})).is_err());assert_eq!(text(&gi).unwrap(),applied);assert!(st.journals().unwrap().is_empty());
            fs::rename(&renamed,&dir).unwrap();
            restore(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &object!{method:"native.restore",params:object!{operationId:"restore",expectedRevision:"2",dataDir:f.data.to_str()}})).unwrap();assert_eq!(text(&gi).unwrap(),before);
        }}}
        let f=Fixture::new();let c=citadel(&f.game).unwrap();
        for exact_present in [false,true] {for kind in ["file","symlink","directory"] {
            let dir=c.join("dmm_localization_default");if exact_present {fs::create_dir(&dir).unwrap();}
            let other=c.join("DMM_Localization_Default");
            match kind {"file"=>fs::write(&other,b"foreign").unwrap(),"symlink"=>std::os::unix::fs::symlink("missing-target",&other).unwrap(),_=>fs::create_dir(&other).unwrap()}
            let error=gameinfo::mount_directory(&c,"citadel/dmm_localization_default").unwrap_err();assert_eq!(error.kind,"ambiguousMount");assert!(error.message.contains("differs only in capital letters"));
            if kind=="directory" {fs::remove_dir(&other).unwrap();}else{fs::remove_file(&other).unwrap();}if exact_present {fs::remove_dir(&dir).unwrap();}
        }}
        assert_eq!(gameinfo::mount_directory(&c,"citadel/truly_missing/child").unwrap(),c.join("truly_missing/child"));
    }
    #[test]
    fn round13_committed_recovery_after_repair_change() {
        {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let dir=c.join("dmm_localization_default");fs::create_dir(&dir).unwrap();
            let file=dir.join("loose.dat");fs::write(&file,b"ORIGINAL").unwrap();
            let gi=c.join("gameinfo.gi");let before=text(&gi).unwrap().replace("Game citadel\n","Game citadel/dmm_localization_default\n Game citadel\n");fs::write(&gi,&before).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            {shards::REVIEW7_AFTER_DB.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("committed interruption"))));}
            let crash=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                {plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input))}
            }));assert!(crash.is_err());assert_eq!(st.operation("first").unwrap()["status"],"succeeded");assert_eq!(st.journals().unwrap().len(),1);
            let doc=st.document().unwrap();let recorded=store::snapshot_document(&doc,None).unwrap();fs::write(&file,b"USER CHANGE").unwrap();
            assert_eq!(store::snapshot_document(&doc,None).unwrap(),recorded,"recorded snapshot depends on live repair folder");
            let recovery_input=object!{method:"native.recover",params:object!{operationId:"recover",expectedRevision:"2",dataDir:f.data.to_str()}};let result=recover(&st,&lock,2,"recover", &game_input("native.recover", 2, "recover", &recovery_input)).unwrap();
            assert!(st.journals().unwrap().is_empty());assert_eq!(st.operation("first").unwrap()["status"],"succeeded");assert_eq!(st.replay("recover",&recovery_input).unwrap(),Some(result));assert_eq!(fs::read(&file).unwrap(),b"USER CHANGE");
            assert!(restore(&st,&lock,3,"restore", &game_input("native.restore", 3, "restore", &object!{method:"native.restore",params:object!{operationId:"restore",expectedRevision:"2",dataDir:f.data.to_str()}})).is_err());
        }
    }
    #[test]
    fn round12_literal_repair_proof() {
        for token in [false,true] {
            let f=Fixture::new();let c=citadel(&f.game).unwrap();let dir=c.join("DMM_Localization_Default");
            fs::create_dir(&dir).unwrap();fs::write(dir.join("loose.dat"),b"ORIGINAL").unwrap();
            let mount=if token {"|gameinfo_path|DMM_Localization_Default"}else{"citadel/DMM_Localization_Default"};
            let before=format!("GameInfo\n{{\n FileSystem\n {{\n SearchPaths\n {{\n Game {mount}\n Game citadel\n }}\n }}\n}}\n");
            let mut proofs=JsonValue::new_array();capture_repair_mounts(&mut proofs,&c,&before).unwrap();
            assert_eq!(proofs[0]["sourcePath"].as_str(),dir.to_str());assert!(proofs[0]["files"].is_array());
            let setup=object!{gameinfo:before,upstreamPayloads:proofs};verify_repair_mounts(&setup,&c).unwrap();
            fs::write(dir.join("loose.dat"),b"USER EDIT").unwrap();assert!(verify_repair_mounts(&setup,&c).is_err());
        }
    }
    #[test]
    fn upstream_repair_mount_changes_block_restore() {
        let f=Fixture::new();let c=f.game.join("game/citadel");let dir=c.join("dmm_localization_default");fs::create_dir(&dir).unwrap();fs::write(dir.join("pak01_dir.vpk"),b"overlay").unwrap();
        let mut doc=object!{};doc["importLedger"]["manifests"]=json::array![];
        let proof=prior_mount_evidence(&doc,&c,&HashSet::from(["citadel/dmm_localization_default".into()]),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/dmm_localization_default\n Game citadel\n }\n }\n}\n",None).unwrap();
        assert!(!proof.is_empty(),"upstream repair mount must be recorded");
        let setup=object!{upstreamEvidenceVersion:2,upstreamPayloads:proof.clone()};check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).unwrap();
        let legacy=object!{upstreamEvidenceVersion:1,upstreamPayloads:proof.clone()};check_upstream(&legacy, false, &citadel(&f.game).unwrap(), None).unwrap();
        fs::write(dir.join("pak01_dir.vpk"),b"changed").unwrap();assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err());
        fs::remove_dir_all(&dir).unwrap();assert!(check_upstream(&setup, false, &citadel(&f.game).unwrap(), None).is_err());
    }
    #[test]
    fn installed_journal_proof_requires_every_identity_field() {
        let f = Fixture::new();
        let path = f.game.join("game/citadel/origin.vpk");
        fs::write(&path, b"origin bytes").unwrap();
        let fp = paths::fingerprint(&path).unwrap();
        let proof = object!{sourcePath:path.to_string_lossy().as_ref(),state:"present",proofVersion:2,
            proofKind:"bytes",installedOrigin:true,size:fp.size,sha256:fp.sha256,
            identity:identity(&paths::open_regular(&path).unwrap().metadata().unwrap())};
        verify_prior_source(&proof, false).unwrap();
        for field in ["size", "device", "inode", "modifiedSeconds", "modifiedNanos"] {
            let mut changed = proof.clone();
            changed["identity"][field] = JsonValue::Null;
            assert_eq!(verify_prior_source(&changed, false).unwrap_err().kind, "externalChange", "{field}");
        }
        fs::write(&path, b"changed data").unwrap();
        assert_eq!(verify_prior_source(&proof, false).unwrap_err().kind, "externalChange");
    }

    #[test]
    fn round11_case_mount_baselines() {
        {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let live=c.join("gameinfo.gi");
            let original=text(&live).unwrap().replace("citadel/addons","citadel/Addons");fs::write(&live,&original).unwrap();
            fs::create_dir(c.join("Addons")).unwrap();fs::write(c.join("Addons/loose.txt"),b"UPPERCASE ORIGINAL").unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let mut input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"ambiguous",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};{input["params"]["deploymentLayout"]="shards-v1".into();}
            assert!(plan.apply(&st,&lock,1,"ambiguous", &game_input("native.apply", 1, "ambiguous", &input)).is_err(),"ambiguous folders must refuse layout=shards-v1");
            assert_eq!(text(&live).unwrap(),original);
        }
    }
    #[test]
    fn round11_restore_rejects_wrong_case_and_ambiguity() {
        {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let mut input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            {input["params"]["deploymentLayout"]="shards-v1".into();}
            plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();
            let mut doc=st.document().unwrap();
            // An intermediate writer recorded lowercase files for an uppercase mount.
            doc["previous"]["gameinfo"]=doc["previous"]["gameinfo"].as_str().unwrap().replace("citadel/addons","citadel/Addons").into();
            let reason=restore_baseline_reason(&doc).expect("wrong path casing cannot restore");
            assert!(reason.contains("spelling"),"{reason}");
            let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();
            db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();
            assert_eq!(st.snapshot(None).unwrap()["canRestore"],false);
            assert!(restore(&st,&lock,2,"wrong-case", &game_input("native.restore", 2, "wrong-case", &object!{})).is_err());
            doc["previous"]["gameinfo"]=doc["previous"]["gameinfo"].as_str().unwrap().replace("citadel/Addons","citadel/addons").into();
            fs::create_dir(c.join("Addons")).unwrap();fs::write(c.join("Addons/loose.txt"),b"CHANGED AFTER APPLY").unwrap();
            db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();
            assert_eq!(st.snapshot(None).unwrap()["canRestore"],false);
            assert!(restore(&st,&lock,2,"ambiguous", &game_input("native.restore", 2, "ambiguous", &object!{})).is_err());
            assert_eq!(fs::read(c.join("Addons/loose.txt")).unwrap(),b"CHANGED AFTER APPLY");
        }
    }
    #[test]
    fn round11_exact_case_capture_and_historical_mismatch() {
        let f=Fixture::new();let c=citadel(&f.game).unwrap();
        fs::rename(c.join("addons"),c.join("Addons")).unwrap();
        fs::write(c.join("Addons/loose.txt"),b"ORIGINAL").unwrap();
        let mut setup=object!{gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/Addons\n Game citadel\n }\n }\n}\n"};
        capture_checkpoint_baselines(&mut setup,&c).unwrap();
        assert_eq!(setup["installedGameOrigins"][0]["directory"],c.join("Addons").to_str().unwrap());
        check_installed_baselines(&setup, &c, true, None).unwrap();
        setup["installedGameOrigins"][0]["directory"]=c.join("addons").to_str().unwrap().into();
        let doc=object!{canRestore:true,gamePath:f.game.to_str(),previous:setup};
        assert!(restore_baseline_reason(&doc).is_some());
        for spelling in ["citadel/Addons", "|gameinfo_path|Addons", "citadel\\Addons"] {
            let mut setup=object!{gameinfo:format!("GameInfo\n{{\n FileSystem\n {{\n SearchPaths\n {{\n Game {spelling}\n Game citadel\n }}\n }}\n}}\n")};
            capture_checkpoint_baselines(&mut setup,&c).unwrap();
            check_installed_baselines(&setup, &c, true, None).unwrap();
            fs::write(c.join("Addons/loose.txt"),b"CHANGED AFTER APPLY").unwrap();
            assert!(check_installed_baselines(&setup, &c, true, None).is_err());
            fs::write(c.join("Addons/loose.txt"),b"ORIGINAL").unwrap();
        }
    }
    #[test]
    fn review7_legacy_addon_suffix_requires_checkpoint_evidence() {
        for folder in ["addons","addons2","addons_profile","addons2_profile"] {
            let setup=object!{gameinfo:format!("GameInfo\n{{\n FileSystem\n {{\n SearchPaths\n {{\n Game citadel/{folder}\n Game citadel\n }}\n }}\n}}\n"),upstreamEvidenceVersion:2,upstreamPayloads:json::array![]};
            assert_eq!(check_installed_baselines(&setup, Path::new("/fixture/citadel"), true, None).unwrap_err().kind,"unverifiedRestore","{folder}");
        }
        let setup=object!{gameinfo:"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n"};
        check_installed_baselines(&setup, Path::new("/fixture/citadel"), true, None).unwrap();
    }
    #[test]
    fn review7_checkpoint_folder_drift_before_publication() {
        {
            let(f,lock,st,_)=setup();
            let c=citadel(&f.game).unwrap();let live=c.join("gameinfo.gi");let original=text(&live).unwrap();
            let extra=c.join("addons/late-added_dir.vpk");
            let i=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            let result={
                let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
                let added=extra.clone();shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(move||fs::write(added,b"late external mod").unwrap())));
                plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &i))
            };
            assert!(result.is_err(),"changed captured baseline must refuse sharded=shards-v1");
            assert!(extra.exists());assert_eq!(text(&live).unwrap(),original);
            assert_eq!(st.document().unwrap()["revision"],"1");
            assert!(st.journals().unwrap().is_empty(),"owned staging rolled back");
        }
    }
    #[test]
    fn review7_recorded_snapshot_ignores_later_build_metadata() {
        let(f,_lock,st,_)=setup();let mut doc=st.document().unwrap();
        let steam=f.root.join("steam");fs::create_dir(&steam).unwrap();
        doc["gamePath"]=steam.join("common/Game").to_str().unwrap().into();
        let before=store::snapshot_document(&doc,None).unwrap();
        fs::write(steam.join("appmanifest_1422450.acf"),"\"buildid\" \"123456\"").unwrap();
        assert_eq!(store::snapshot_document(&doc,None).unwrap(),before);
        let mut historical=before.clone();historical["launchFingerprint"]["gameBuild"]=123456.into();
        assert!(store::matches_recorded_snapshot(&doc,&historical).unwrap());
        historical["revision"]="999".into();
        assert!(!store::matches_recorded_snapshot(&doc,&historical).unwrap());
    }
    #[test]
    fn review7_recovery_after_checkpoint_folder_change() {
        {
        let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let i=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        {shards::REVIEW7_AFTER_DB.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("crash after database commit"))));}
        let crash=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            {plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &i))}
        }));assert!(crash.is_err());
        assert_eq!(st.operation("first").unwrap()["status"],"succeeded");assert_eq!(st.journals().unwrap().len(),1);
        let added=c.join("addons/user-added.vpk");fs::write(&added,b"a later user file").unwrap();let revision=st.document().unwrap()["revision"].as_str().unwrap().to_owned();
        let input=object!{method:"native.recover",params:object!{dataDir:f.data.to_str(),operationId:"recover",expectedRevision:revision.clone()}};
        recover(&st,&lock,revision.parse().unwrap(),"recover", &game_input("native.recover", revision.parse().unwrap(), "recover", &input)).unwrap();
        assert!(added.exists());assert!(st.journals().unwrap().is_empty());
        }
    }
    #[test]
    fn reviewer_sharded_late_live_gameinfo_change() {
        let(f,lock,st,_)=setup();let live=citadel(&f.game).unwrap().join("gameinfo.gi");
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"race",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let target=live.clone();shards::BEFORE_PUBLISH.with(|h|*h.borrow_mut()=Some(Box::new(move||fs::write(target,b"OUTSIDE CHANGE").unwrap())));
        assert!(plan.apply(&st,&lock,1,"race", &game_input("native.apply", 1, "race", &input)).is_err());
        assert_eq!(fs::read(&live).unwrap(),b"OUTSIDE CHANGE");
    }
    #[test]
    fn round8_current_retirement_rechecks_each_unlink() {
        let _hooks=FaultHooks;
        for add in [false,true] {
            let(f,lock,st,plan)=setup();let i=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &i)).unwrap();let doc=st.document().unwrap();let a=doc["applied"].clone();let c=citadel(&f.game).unwrap();let dir=generation_dir(&c,&a).unwrap();
            let mut retired=doc;retired["applied"]=JsonValue::Null;retired["previous"]["applied"]=JsonValue::Null;retired["retiredGenerations"]=json::array![a];
            fs::write(c.join("gameinfo.gi"),retired["previous"]["gameinfo"].as_str().unwrap()).unwrap();
            let target=dir.clone();shards::BEFORE_UNLINK.with(|h|*h.borrow_mut()=Some(Box::new(move||{
                if add {fs::write(target.join("foreign"),b"keep").unwrap();}else{let p=target.join("manifest.json");let bytes=fs::read(&p).unwrap();let replacement=target.join("replacement");fs::write(&replacement,bytes).unwrap();fs::rename(replacement,p).unwrap();}
            })));
            assert!(shards::maintenance_retire(&st,&retired).is_err());assert!(dir.join("manifest.json").exists());
            if add {
                assert!(dir.join("shard01/pak01_dir.vpk").exists());
                assert_eq!(fs::read(dir.join("foreign")).unwrap(),b"keep");
            } else {
                // Enumeration can visit a verified payload before the changed
                // marker. Only the changed file must be preserved in that case.
                let bytes=fs::read(dir.join("manifest.json")).unwrap();
                assert_eq!(bytes,retired["retiredGenerations"][0]["manifest"].dump().as_bytes());
                assert_ne!(fs::metadata(dir.join("manifest.json")).unwrap().ino(),retired["retiredGenerations"][0]["identities"]["manifest.json"]["inode"].as_u64().unwrap());
            }
        }
    }
    #[test]
    fn round10_current_live_identity_at_publication() {
        let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let live=c.join("gameinfo.gi");let before=fs::read(&live).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let path=live.clone();
        BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(Box::new(move|_|{
            let replacement=path.with_extension("foreign");fs::write(&replacement,fs::read(&path).unwrap()).unwrap();fs::rename(replacement,&path).unwrap();
        })));
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"race",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
        assert!(plan.apply(&st,&lock,1,"race", &game_input("native.apply", 1, "race", &input)).is_err());
        BEFORE_FINAL_RENAME.with(|h|assert!(h.borrow().is_none()));assert_eq!(fs::read(live).unwrap(),before);
    }
    #[test]
    fn round9_publication_final_checks() {
        {for mutation in [0,1,2] {
            let(f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();let before=fs::read(c.join("gameinfo.gi")).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let dir=c.join("addons");
            BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(Box::new(move|temp|{
                if mutation==0 {let replacement=temp.with_extension("foreign");fs::write(&replacement,fs::read(temp).unwrap()).unwrap();fs::rename(replacement,temp).unwrap();}
                else if mutation==1 {fs::write(dir.join("foreign"),b"keep").unwrap();}
                else {let native=temp.parent().unwrap().join("deadlock_native");for e in fs::read_dir(native).unwrap(){let p=e.unwrap().path();if p.is_dir(){fs::write(p.join("foreign"),b"keep").unwrap();}}}
            })));
            let mut input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"race",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            {input["params"]["deploymentLayout"]="shards-v1".into();}
            assert!(plan.apply(&st,&lock,1,"race", &game_input("native.apply", 1, "race", &input)).is_err(),"layout=shards-v1 mutation={mutation}");
            BEFORE_FINAL_RENAME.with(|h|assert!(h.borrow().is_none(),"injection boundary must be reached"));
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(),before);
            if mutation==1 {assert!(st.journals().unwrap().is_empty(),"addon race must clear journal");}
        }}
    }
    #[test]
    fn round9_sharded_unlink_parent_and_membership() {
        for swap in [false,true] {
            let(f,lock,st,_)=setup();let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();let a=st.document().unwrap()["applied"].clone();let c=citadel(&f.game).unwrap();let dir=generation_dir(&c,&a).unwrap();
            shards::test_cleanup_parent(&st,&c,&a,&dir,swap);
        }
    }
    #[test]
    fn item48_saved_spelling_first_apply_can_restore() {
        for suffix in ["/"] {
            let(f,lock,st,_)=setup();
            let original=fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
            let mut doc=st.document().unwrap();
            let spelling=if suffix=="/" {format!("{}/",f.game.display())} else {format!("{}/../{}",f.game.display(),f.game.file_name().unwrap().to_str().unwrap())};
            doc["gamePath"]=spelling.into();
            st.commit(&lock,1,"saved-spelling",&object!{},doc,None).unwrap();
            let mut doc=st.document().unwrap();
            let held=change_game_folder(&st,&mut doc,&f.game).unwrap();
            assert_ne!(doc["gameFolderChanged"],true);
            assert!(doc["previousGameFolders"].is_null());
            drop(held);
            st.commit(&lock,2,"folder-spelling",&object!{},doc,None).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            plan.apply(&st,&lock,3,"apply", &game_input("native.apply", 3, "apply", &object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"apply",expectedRevision:"3",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}})).unwrap();
            let snap=st.snapshot(None).unwrap();
            assert_eq!(snap["canRestore"],true,"{snap}");
            assert_eq!(snap["pendingChanges"],false,"{snap}");
            restore(&st,&lock,4,"restore", &game_input("native.restore", 4, "restore", &object!{method:"native.restore",params:object!{dataDir:f.data.to_str(),operationId:"restore",expectedRevision:"4"}})).unwrap();
            assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),original);
        }
    }
    #[test]
    fn item48_symlink_target_is_folder_change() {
        let(f,lock,st,_)=setup();let alias=f.root.join("saved-link");
        std::os::unix::fs::symlink(&f.game,&alias).unwrap();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"apply", &game_input("native.apply", 1, "apply", &object!{method:"native.apply",params:object!{}})).unwrap();
        let mut doc=st.document().unwrap();doc["gamePath"]=alias.to_str().unwrap().into();
        st.commit(&lock,2,"save-alias",&object!{},doc,None).unwrap();
        drop(st);drop(lock);let mut svc=crate::native::NativeService::new();
        svc.dispatch("native.profile",&object!{dataDir:f.data.to_str(),action:"useGameFolder",gamePath:f.game.to_str(),operationId:"canonical",expectedRevision:"3"}).unwrap();
        let st=Store::open(&f.data).unwrap();let doc=st.document().unwrap();
        assert_eq!(doc["gamePath"],f.game.to_str().unwrap());
        assert_eq!(doc["gameFolderChanged"],true);
        assert!(doc["applied"].is_null());
        assert!(doc["previousGameFolders"][0]["applied"].is_object());
        assert_eq!(restore_review(&st).unwrap_err().kind,"previousGameFolder");
    }
    #[test]
    fn item48_trailing_slash_keeps_checkpoint() {
        let(f,lock,st,_)=setup();
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"apply", &game_input("native.apply", 1, "apply", &object!{method:"native.apply",params:object!{}})).unwrap();
        let mut doc=st.document().unwrap();assert!(doc["previous"].is_object());let before=doc.clone();
        change_game_folder(&st,&mut doc,Path::new(&format!("{}/",f.game.display()))).unwrap();
        assert_eq!(doc,before);
    }
    #[test]
    fn item48_any_journal_blocks_folder_change() {
        for kind in ["font","localImport","remote","game","restore"] {
            let(f,lock,st,_)=setup();let before=st.document().unwrap();
            st.write_journal(&lock,"unfinished",&object!{kind:kind}).unwrap();
            let mut doc=before.clone();
            let error=change_game_folder(&st,&mut doc,&f.game).err().unwrap();
            assert_eq!(error.message,"Finish or recover the current operation before changing the game folder.");
            assert_eq!(doc,before);
            drop(st);drop(lock);
            let error=crate::native::NativeService::new().dispatch("native.profile",&object!{dataDir:f.data.to_str(),action:"useGameFolder",gamePath:f.game.to_str(),operationId:"change",expectedRevision:"1"}).unwrap_err();
            assert_eq!(error.message,"Finish or recover the current operation before changing the game folder.");
        }
    }
    #[test]
    fn item48_first_folder_has_no_previous_restore_explanation() {
        let(f,_lock,st,_)=setup();let mut doc=st.document().unwrap();
        doc["gamePath"]=JsonValue::Null;
        doc["profiles"]["default"]["mods"]=json::array![];
        doc["profiles"]["other"]["mods"]=json::array![];
        change_game_folder(&st,&mut doc,&f.game).unwrap();
        let result=store::snapshot_document(&doc,None).unwrap();
        assert!(!result["warnings"].members().any(|w|w["code"]=="previousGameFolder"));
        assert_eq!(doc["previousGameFolders"].len(),0);
    }
    #[test]
    fn item47_folder_identity_rechecked_before_commit() {
        let(f,lock,st,_)=setup();let new=f.root.join("new");
        assert!(std::process::Command::new("cp").args(["-a","--"]).arg(&f.game).arg(&new).status().unwrap().success());
        let c=new.join("game/citadel");let moved=new.join("game/replaced");
        BEFORE_FOLDER_COMMIT.with(|h|*h.borrow_mut()=Some(Box::new(move|| {
            fs::rename(&c,&moved).unwrap();
            assert!(std::process::Command::new("cp").args(["-a","--"]).arg(&moved).arg(&c).status().unwrap().success());
            crate::installed::install_path(c.parent().unwrap().parent().unwrap()).unwrap();
        })));
        drop(st);drop(lock);let mut svc=crate::native::NativeService::new();
        assert_eq!(svc.dispatch("native.profile",&object!{dataDir:f.data.to_str(),action:"useGameFolder",gamePath:new.to_str(),operationId:"folder",expectedRevision:"1"}).unwrap_err().kind,"nativeBusy");
        assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["gamePath"],f.game.to_str().unwrap());
    }
    #[test]
    fn item47_partial_folder_rejected() {
        let(f,_lock,st,_)=setup(); let partial=f.root.join("partial");
        fs::create_dir_all(partial.join("game/citadel")).unwrap();
        fs::copy(f.game.join("game/citadel/gameinfo.gi"),partial.join("game/citadel/gameinfo.gi")).unwrap();
        let mut doc=st.document().unwrap(); let before=doc.clone();
        assert_eq!(change_game_folder(&st,&mut doc,&partial).err().unwrap().kind,"invalidGamePath");
        assert_eq!(doc,before);
        fs::copy(f.game.join("game/citadel/pak01_dir.vpk"),partial.join("game/citadel/pak01_dir.vpk")).unwrap();
        assert!(change_game_folder(&st,&mut doc,&partial).is_err(),"core VPK alone is insufficient");
        fs::remove_file(partial.join("game/citadel/pak01_dir.vpk")).unwrap();
        fs::create_dir_all(partial.join("game/bin/win64")).unwrap();
        assert!(change_game_folder(&st,&mut doc,&partial).is_err(),"binary folder alone is insufficient");
        assert_eq!(doc,before);
    }
    #[test]
    fn item47_same_folder_preserves_checkpoint() {
        let(f,lock,st,plan)=setup();
        plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
        let original=st.document().unwrap();
        for path in [format!("{}/",f.game.display())] {
            let mut doc=original.clone();change_game_folder(&st,&mut doc,Path::new(&path)).unwrap();
            assert_eq!(doc,original,"{path}");assert_eq!(doc["canRestore"],true);
        }
        // ".." may cross a link to a different install; never compare it lexically.
        let mut doc=original.clone();
        let e=change_game_folder(&st,&mut doc,Path::new(&format!("{}/../game-root",f.game.display()))).err().expect("'..' must be refused");
        assert_eq!(e.kind,"unsafePath");assert_eq!(doc,original);
    }
    #[test]
    fn item48_link_parent_cannot_alias_another_install() {
        // /base/link -> /other/sub ; "/base/link/../game-root" is really /other/game-root.
        let(f,lock,st,plan)=setup();
        plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
        let original=st.document().unwrap();
        let base=f.game.parent().unwrap();
        let other=base.join("other");fs::create_dir_all(other.join("sub")).unwrap();
        std::os::unix::fs::symlink(other.join("sub"),base.join("link")).unwrap();
        let aliased=base.join("link/..").join(f.game.file_name().unwrap());
        let mut doc=original.clone();
        assert!(change_game_folder(&st,&mut doc,&aliased).is_err());
        assert_eq!(doc,original,"checkpoint must not be kept for a different install");
    }
    #[test]
    fn item48_saved_folder_swapped_for_link_is_a_change() {
        let(f,lock,st,plan)=setup();
        plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
        let original=st.document().unwrap();
        // Move the real install away and leave a link to a different (copied) install at /A.
        let a=fs::canonicalize(&f.game).unwrap();
        let moved=a.parent().unwrap().join("install-b");
        fs::rename(&a,&moved).unwrap();
        std::os::unix::fs::symlink(&moved,&a).unwrap();
        let mut doc=original.clone();
        drop(change_game_folder(&st,&mut doc,&a).unwrap());
        assert_eq!(doc["gameFolderChanged"],true,"a link at the saved path must count as a folder change");
        assert_eq!(doc["canRestore"],false);
        assert!(doc["applied"].is_null());
        assert_eq!(doc["gamePath"].as_str().unwrap(),moved.to_str().unwrap());
    }
    #[test]
    fn item48_symlink_choice_saves_real_location() {
        let(f,_lock,st,_plan)=setup();
        let link=f.game.parent().unwrap().join("game-link");
        std::os::unix::fs::symlink(&f.game,&link).unwrap();
        let mut doc=st.document().unwrap();
        drop(change_game_folder(&st,&mut doc,&link).unwrap());
        assert_eq!(doc["gamePath"].as_str().unwrap(),fs::canonicalize(&f.game).unwrap().to_str().unwrap());
    }
    #[test]
    fn item47_old_partial_root_does_not_block_change() {
        for partial in [false,true] {
            let(f,lock,st,plan)=setup();
            plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
            fs::rename(f.game.join("game/citadel"),f.root.join("moved-citadel")).unwrap();
            if partial {fs::create_dir_all(f.game.join("game/citadel")).unwrap();}
            assert_eq!(st.snapshot(None).unwrap()["recoveryRequired"],false);
            st.write_journal(&lock,"interrupted",&object!{version:1}).unwrap();
            assert_eq!(st.snapshot(None).unwrap()["recoveryRequired"],true);
        }
    }
    #[test]
    fn item47_folder_result_is_fresh() {
        let(f,lock,st,_)=setup();let new=f.root.join("new");
        assert!(std::process::Command::new("cp").args(["-a","--"]).arg(&f.game).arg(&new).status().unwrap().success());
        drop(st);drop(lock);let mut svc=crate::native::NativeService::new();
        let result=svc.dispatch("native.profile",&object!{dataDir:f.data.to_str(),action:"useGameFolder",gamePath:new.to_str(),operationId:"folder",expectedRevision:"1"}).unwrap();
        assert_eq!(result["pendingReason"],"externalGameinfoChange");
        assert!(result["warnings"].members().any(|w|w["message"].as_str().unwrap_or("").starts_with("This restore point belongs to the previous game folder (")));
    }
    #[test]
    fn item47_current_staging_refusal_preserves_evidence() {
        let(f,lock,st,plan)=setup();let gi=f.game.join("game/citadel/gameinfo.gi");let before=fs::read(&gi).unwrap();
        let target=f.game.join("game/citadel/addons/pak01_dir.vpk");
        let result=apply_at(&plan,&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}}),move |phase|{
            if phase=="beforeRename" {fs::write(&target,b"changed").unwrap();} Ok(())
        });
        let error=result.unwrap_err();assert_eq!(error.kind,"recoveryRequired");
        assert!(st.journal("first").unwrap().is_some());assert_eq!(st.document().unwrap()["revision"],"1");assert_eq!(fs::read(gi).unwrap(),before);
    }
    #[test]
    fn item46_current_publication_upstream_refusal() { item46_current_refusal(false); }
    #[test]
    fn item46_current_restore_clean_refusal() { item46_current_refusal(true); }
    fn item46_current_refusal(restoring:bool) {
            let(f,lock,st,plan)=setup();
            let input=object!{method:"native.apply",params:object!{operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef"}};
            if restoring {plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();}
            let live=f.game.join("game/citadel/gameinfo.gi");let before=fs::read(&live).unwrap();
            let target=f.game.join("game/citadel/addons/pak01_dir.vpk");
            BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(Box::new(move |_|fs::write(target,b"external edit").unwrap())));
            let result=if restoring {restore_at(&st,&lock,2,"undo", &game_input("native.restore", 2, "undo", &object!{method:"native.restore",params:object!{operationId:"undo",expectedRevision:"2"}}),|_|Ok(()))}
                else {plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input))};
            let error=result.unwrap_err();assert_eq!(error.kind,"recoveryRequired","restoring={restoring}: {error:?}");
            assert_eq!(st.journals().unwrap().len(),1);assert_eq!(fs::read(live).unwrap(),before);
    }
    #[test]
    fn item46_moved_folder() {
        for keep in [false,true] {
            let(f,lock,st,_)=setup();
            let input=object!{method:"native.apply",params:object!{operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();
            let new=f.game.with_file_name("MovedDeadlock");
            assert!(std::process::Command::new("cp").args(["-a","--"]).arg(&f.game).arg(&new).status().unwrap().success());
            if !keep {fs::remove_dir_all(&f.game).unwrap();
                assert_eq!(st.snapshot(None).unwrap()["recoveryRequired"],false);
            }drop(st);drop(lock);
            let mut svc=crate::native::NativeService::new();
            let mut p=object!{dataDir:f.data.to_str(),action:"useGameFolder",gamePath:new.join("invalid").to_str(),operationId:"folder",expectedRevision:"2"};
            assert!(svc.dispatch("native.profile",&p).is_err());
            assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["gamePath"],f.game.to_str().unwrap());
            p["gamePath"]=new.to_str().unwrap().into();p["operationId"]="valid-folder".into();svc.dispatch("native.profile",&p).unwrap();
            let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
            assert_eq!(restore(&st,&lock,3,"old", &game_input("native.restore", 3, "old", &object!{})).unwrap_err().message,format!("This restore point belongs to the previous game folder ({}). Apply again to set up mods in the new folder.",f.game.display()));
            assert_eq!(st.snapshot(None).unwrap()["pendingReason"],"externalGameinfoChange");
            let plan=ApplyPlan::review(&st,"default",&new).unwrap();
            assert!(plan.summary["warnings"].members().any(|w|w["code"]=="unmanagedMovedGenerations"));
            assert_eq!(plan.summary["warnings"].members().any(|w|w["code"]=="oldInstallKeepsMods"),keep);
            let prior=fs::read_dir(new.join("game/citadel/deadlock_native")).unwrap().map(|e|e.unwrap().path()).collect::<Vec<_>>();
            let mut input=input.clone();input["params"]["operationId"]="fresh".into();input["params"]["expectedRevision"]="3".into();
            plan.apply(&st,&lock,3,"fresh", &game_input("native.apply", 3, "fresh", &input)).unwrap();
            check_snapshot(st.root(),&st.document().unwrap(), None).unwrap();
            let next=ApplyPlan::review(&st,"default",&new).unwrap();
            input["params"]["operationId"]="repeat".into();input["params"]["expectedRevision"]="4".into();
            next.apply(&st,&lock,4,"repeat", &game_input("native.apply", 4, "repeat", &input)).unwrap();
            restore(&st,&lock,5,"fresh-restore", &game_input("native.restore", 5, "fresh-restore", &object!{method:"native.restore",params:object!{operationId:"fresh-restore",expectedRevision:"5",deploymentLayout:"shards-v1"}})).unwrap();
            for path in prior {assert!(path.exists());}
        }
    }
    #[test]
    fn item46_play_drift_requires_review() {
        let(f,lock,st,_)=setup();
        let input=object!{method:"native.apply",params:object!{operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();
        let gi=f.game.join("game/citadel/gameinfo.gi");let before=text(&gi).unwrap().replace("Game citadel", "Game citadel/addons\n Game citadel");fs::write(&gi,&before).unwrap();
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let result=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&crate::prepare_control::PrepareControl::never_cancelled(),&mut |_|{});
        assert_eq!(result.err().expect("Play must stop").message,"The game files were changed by another mod manager. Review the changes to continue.");
        assert_eq!(text(&gi).unwrap(),before);assert!(st.journals().unwrap().is_empty());assert_eq!(st.document().unwrap()["revision"],"2");
    }
    #[test]
    fn item115_diagnostics_current_apply_restore_kinds_unchanged(){
        for restore_test in [false,true] {for code in [0,28,13] {
            let(_f,lock,st,p)=setup();if restore_test{p.apply(&st,&lock,1,"seed", &game_input("native.apply", 1, "seed", &object!{})).unwrap();}
            let inject=move |at:&str|if at==if restore_test{"beforeRename"}else{"staged"}{Err(if code==0{err("gameRunning","Close Deadlock first.")}else if code==13{crate::sqlite::test_full_error()}else{std::io::Error::from_raw_os_error(code).into()})}else{Ok(())};
            let result=if restore_test{restore_at(&st,&lock,2,"failure", &game_input("native.restore", 2, "failure", &object!{}),inject)}else{apply_at(&p,&st,&lock,1,"failure", &game_input("native.apply", 1, "failure", &object!{}),inject)};
            let e=result.unwrap_err();assert_eq!(e.kind,"recoveryRequired");assert!(e.message.contains(if code==0{"gameRunning"}else{"Not enough disk space"}));assert!(st.journal("failure").unwrap().is_some());
        }}
    }
    #[test]
    fn item115_diagnostics_enospc_keeps_io_kind(){let e=NativeError::from(std::io::Error::from_raw_os_error(28));assert_eq!(e.kind,"nativeIo");assert!(e.message.contains("Not enough disk space"));}
    fn setup() -> (Fixture, RootLock, Store, ApplyPlan) {
        setup_padded(0)
    }
    #[test]
    fn item94_unchanged_play_does_not_rehash_vpks_after_review() {
        let (f, lock, st, _) = setup();
        let input=object!{method:"native.apply",params:object!{operationId:"initial",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"initial", &game_input("native.apply", 1, "initial", &input)).unwrap();
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let control=crate::prepare_control::PrepareControl::never_cancelled();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let plan=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).unwrap();
        assert!(matches!(plan,crate::launch_preparation::PreparationPlan::Verify(_)));
        let input=plan.operation_input("prepare").unwrap();
        let prepared=plan.execute(&st,&lock,&guard,2,"prepare",&input,&control,&mut |_|{}).unwrap();
        prepared.proof.revalidate(&st,&lock,&guard).unwrap();
        let doc=st.document().unwrap();
        let generation=doc["applied"]["generation"].as_str().unwrap();
        let dir=f.game.join("game/citadel/deadlock_native").join(generation);
        let deployed=generation::decode(&doc["applied"]["manifest"]).unwrap().files;
        assert!(!deployed.is_empty(),"the fixture must deploy a VPK");
        paths::HASH_READS.with(|r| {
            let reads=r.borrow();
            for file in deployed {let path=dir.join(file.name);assert_eq!(reads.get(&path).copied().unwrap_or(0),0,"{} retains its verified Apply receipt",path.display());}
        });
    }
    #[test]
    fn item94_changed_deployed_mtime_refuses_dispatch_without_rehash() {
        let (f, lock, st, _) = setup();
        let input=object!{method:"native.apply",params:object!{operationId:"initial",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,1,"initial", &game_input("native.apply", 1, "initial", &input)).unwrap();
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let control=crate::prepare_control::PrepareControl::never_cancelled();
        let plan=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).unwrap();
        let input=plan.operation_input("prepare").unwrap();
        let prepared=plan.execute(&st,&lock,&guard,2,"prepare",&input,&control,&mut |_|{}).unwrap();
        let generation=st.document().unwrap()["applied"]["generation"].as_str().unwrap().to_owned();
        let path=f.game.join("game/citadel/deadlock_native").join(generation).join("shard01/pak01_dir.vpk");
        let file=fs::File::open(&path).unwrap();
        let modified=file.metadata().unwrap().modified().unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified+std::time::Duration::from_secs(1))).unwrap();
        paths::HASH_READS.with(|r|r.borrow_mut().clear());
        let error=prepared.proof.revalidate(&st,&lock,&guard).err().expect("changed identity must stop dispatch");
        assert_eq!(error.kind,"stalePreview");
        paths::HASH_READS.with(|r| assert!(!r.borrow().contains_key(&path),"changed VPK must be refused without rehashing"));
    }
    #[test]
    fn launch_proof_accepts_hashless_only_game_resource_paths() {
        let (f, lock, st, _) = setup();
        let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let control=crate::prepare_control::PrepareControl::never_cancelled();
        let plan=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&f.game,&control,&mut |_|{}).unwrap();
        let input=plan.operation_input("prepare").unwrap();
        let prepared=plan.execute(&st,&lock,&guard,1,"prepare",&input,&control,&mut |_|{}).unwrap();
        let proof=prepared.proof.evidence().clone();
        crate::launch_record::proof(&proof).unwrap();
        let fingerprint=object!{sha256:"",size:52,device:50,inode:7,modifiedSeconds:1,modifiedNanos:2};
        for name in ["game/citadel/pak01_dir.vpk", "game/core/resource.vpk", "game/citadel/materials/nested/resource.vpk", "game/core/addons.vpk"] {
            let mut test=proof.clone();
            test["selectedSources"]["sources"]=json::array![object!{path:f.game.join(name).to_str(),identity:fingerprint.clone()}];
            crate::launch_record::proof(&test).expect("game resource may use metadata only");
        }
        for path in [f.data.join("payloads/a.vpk"),f.data.join("cache/repairs/b.vpk"),
            f.game.parent().unwrap().join("appmanifest_1422450.acf"),f.game.join("game-other/resource.vpk"),f.game.join("game"),
            f.game.join("game/other/resource.vpk"),f.game.join("game/citadel/addons/pak01_dir.vpk"),
            f.game.join("game/core/nested/deadlock_native/resource.vpk"),f.game.join("game/citadel/deadlock_native_old/resource.vpk"),
            f.game.join("game/citadel/dmm_localization_default/resource.vpk"),f.game.join("game/citadel/gameinfo.gi"),
            f.game.join("game/core/nested/.native-gameinfo-temp")] {
            let mut test=proof.clone();
            test["selectedSources"]["sources"]=json::array![object!{path:path.to_str(),identity:fingerprint.clone()}];
            assert!(crate::launch_record::proof(&test).is_err(),"hashless source outside game resources: {}",path.display());
            test["selectedSources"]["sources"][0]["identity"]["sha256"]="a".repeat(64).into();
            crate::launch_record::proof(&test).expect("hashed sources remain valid");
        }
        let mut imported=proof.clone();
        imported["selectedSources"]["importedManifests"]=json::array![object!{path:f.game.join("game/core/resource.vpk").to_str(),identity:fingerprint.clone()}];
        assert!(crate::launch_record::proof(&imported).is_err(), "imported manifests still require hashes");
        let samples = [
            ("game/citadel/materials/nested/resource.vpk", true),
            ("game/core/addons.vpk", true),
            ("game/citadel/addons/pak01_dir.vpk", false),
            ("game/core/nested/deadlock_native/resource.vpk", false),
            ("game/citadel/deadlock_native_old/resource.vpk", false),
            ("game/citadel/dmm_localization_default/resource.vpk", false),
            ("game/core/nested/.native-gameinfo-temp", false),
        ];
        for (name, _) in samples {
            let path=f.game.join(name);fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(path,b"resource").unwrap();
        }
        let members=crate::repairs::resource_membership(&f.game).unwrap();
        for (name, expected) in samples {
            let path=f.game.join(name);
            assert_eq!(members.contains(&path), expected, "membership: {name}");
            assert_eq!(crate::repairs::resource_path(&f.game,&path), expected, "launch evidence: {name}");
        }
    }
    fn setup_padded(padding: usize) -> (Fixture, RootLock, Store, ApplyPlan) {
        let f = Fixture::new();
        if padding > 0 {
            let path = f.game.join("game/citadel/addons/pak01_dir.vpk");
            let mut bytes = fs::read(&path).unwrap();
            bytes.resize(bytes.len() + padding, 0);
            fs::write(path, bytes).unwrap();
        }
        fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        let mut s = crate::native::NativeService::new();
        import(&mut s, &f);
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        let plan = ApplyPlan::review(&st, "default", &f.game).unwrap();
        (f, lock, st, plan)
    }
    fn incremental_fixture() -> (Fixture, RootLock, Store, u64) {
        let (f,lock,st,_)=setup_padded(1_000_000);
        let mut bytes=crate::test_support::vpk("models/incremental.vmdl_c",42);bytes.resize(bytes.len()+500_000,0);
        let id="11111111111111111111111111111111";
        fs::write(st.payload_path(id).unwrap(),&bytes).unwrap();
        let payload=package::FileRef{id:id.into(),sha256:paths::digest_bytes(&bytes).unwrap(),size:bytes.len() as u64,source_name:"incremental_dir.vpk".into()};
        let mut doc=st.document().unwrap();let layout=package::layout_version(&doc).unwrap();
        let mut mod_b=doc["profiles"]["default"]["mods"][0].clone();
        mod_b["source"]["id"]="incremental-b".into();mod_b["source"]["remoteId"]="incremental-b".into();mod_b["source"]["installOrder"]=1.into();mod_b["enabled"]=false.into();
        if layout==1 {mod_b["payloads"]=json::array![package::encode_file(&payload)];}
        else {mod_b["packages"]=package::encode_packages(&[package::PackageRef{directory:payload,companions:Default::default()}]);}
        doc["profiles"]["default"]["mods"].push(mod_b).unwrap();
        st.commit(&lock,1,"add-second",&object!{},doc,None).unwrap();
        (f,lock,st,bytes.len() as u64)
    }
    #[test]
    fn incremental_toggle_copies_only_changed_mod_bytes() {
        let (f,lock,st,changed_size)=incremental_fixture();
        let input=|id:&str,r:u64|object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"first", &game_input("native.apply", 2, "first", &input("first",2))).unwrap();
        let first=st.document().unwrap()["applied"].clone();let first_view=generation::decode(&first["manifest"]).unwrap();
        let first_file=citadel(&f.game).unwrap().join("deadlock_native").join(&first_view.generation).join(&first_view.files[0].name);
        let original_inode=fs::metadata(&first_file).unwrap().ino();
        let mut doc=st.document().unwrap();doc["profiles"]["default"]["mods"][1]["enabled"]=true.into();
        st.commit(&lock,3,"enable-second",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        shards::COPIED_BYTES.with(|n|n.set(0));
        plan.apply(&st,&lock,4,"second", &game_input("native.apply", 4, "second", &input("second",4))).unwrap();
        assert_eq!(shards::COPIED_BYTES.with(|n|n.get()),changed_size,"only the newly enabled payload should be copied");
        assert_eq!(fs::metadata(&first_file).unwrap().ino(),original_inode,"the Restore generation must remain intact");
        let second=st.document().unwrap()["applied"].clone();let view=generation::decode(&second["manifest"]).unwrap();
        assert_eq!(view.families.iter().map(|f|f.mod_id.as_str()).collect::<Vec<_>>(),vec!["local-a","incremental-b"]);
        let retained=citadel(&f.game).unwrap().join("deadlock_native").join(&view.generation).join(&view.files[0].name);
        assert_eq!(fs::metadata(retained).unwrap().ino(),original_inode,"unchanged bytes must be reused");
        let mut doc=st.document().unwrap();doc["profiles"]["default"]["mods"][1]["enabled"]=false.into();st.commit(&lock,5,"disable-second",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();shards::COPIED_BYTES.with(|n|n.set(0));
        plan.apply(&st,&lock,6,"third", &game_input("native.apply", 6, "third", &input("third",6))).unwrap();
        assert_eq!(shards::COPIED_BYTES.with(|n|n.get()),0,"disabling must not copy the unchanged mod");
    }
    #[test]
    fn incremental_link_boundaries_recover_and_restore_exact() {
        for boundary in ["reserved","linked","complete"] {
            let (f,lock,st,_)=incremental_fixture();
            let gi=f.game.join("game/citadel/gameinfo.gi");let original=fs::read(&gi).unwrap();
            let original_addon=fs::read(f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap();
            let input=|method:&str,id:&str,r:u64|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"first", &game_input("native.apply", 2, "first", &input("native.apply","first",2))).unwrap();
            let prior=fs::read(&gi).unwrap();
            let mut doc=st.document().unwrap();doc["profiles"]["default"]["mods"][1]["enabled"]=true.into();st.commit(&lock,3,"enable-second",&object!{},doc,None).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
            shards::LINK_BOUNDARY.with(|h|*h.borrow_mut()=Some(Box::new(move |at|if at==boundary {panic!("interrupt at link boundary")})));
            let stopped=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,4,"second",&input("native.apply","second",4),&crate::prepare_control::PrepareControl::never_cancelled(),&mut |_|{})));
            shards::LINK_BOUNDARY.with(|h|*h.borrow_mut()=None);drop(guard);assert!(stopped.is_err(),"boundary {boundary} did not interrupt");
            recover(&st,&lock,4,"recover", &game_input("native.recover", 4, "recover", &input("native.recover","recover",4))).unwrap();
            assert_eq!(fs::read(&gi).unwrap(),prior,"rollback changed the previous gameinfo at {boundary}");
            let revision=rev(&st.document().unwrap()).unwrap();
            restore(&st,&lock,revision,"restore", &game_input("native.restore", revision, "restore", &input("native.restore","restore",revision))).unwrap();
            assert_eq!(fs::read(&gi).unwrap(),original,"Restore changed original gameinfo at {boundary}");
            assert_eq!(fs::read(f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap(),original_addon);
        }
    }
    #[test]
    fn incremental_link_refuses_changed_source_membership() {
        let (f,lock,st,_)=incremental_fixture();let gi=f.game.join("game/citadel/gameinfo.gi");
        let input=|method:&str,id:&str,r:u64|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"first", &game_input("native.apply", 2, "first", &input("native.apply","first",2))).unwrap();
        let prior=fs::read(&gi).unwrap();let applied=st.document().unwrap()["applied"].clone();let view=generation::decode(&applied["manifest"]).unwrap();
        let foreign=citadel(&f.game).unwrap().join("deadlock_native").join(&view.generation).join("shard01/foreign.vpk");let added=foreign.clone();
        let mut doc=st.document().unwrap();doc["profiles"]["default"]["mods"][1]["enabled"]=true.into();st.commit(&lock,3,"enable-second",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        shards::LINK_BOUNDARY.with(|h|*h.borrow_mut()=Some(Box::new(move |at|if at=="reserved" {fs::write(&added,b"foreign").unwrap();})));
        assert!(plan.apply(&st,&lock,4,"second", &game_input("native.apply", 4, "second", &input("native.apply","second",4))).is_err());
        shards::LINK_BOUNDARY.with(|h|*h.borrow_mut()=None);
        assert_eq!(fs::read(&gi).unwrap(),prior);assert_eq!(fs::read(&foreign).unwrap(),b"foreign");
        assert!(recover(&st,&lock,4,"recover", &game_input("native.recover", 4, "recover", &input("native.recover","recover",4))).is_err());
        assert_eq!(fs::read(&foreign).unwrap(),b"foreign","recovery must preserve unowned additions");
    }
    #[test]
    fn incremental_link_unavailable_falls_back_to_copy() {
        let (f,lock,st,changed_size)=incremental_fixture();let gi=f.game.join("game/citadel/gameinfo.gi");
        let input=|method:&str,id:&str,r:u64|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"first", &game_input("native.apply", 2, "first", &input("native.apply","first",2))).unwrap();
        let previous=fs::read(&gi).unwrap();
        let mut doc=st.document().unwrap();doc["profiles"]["default"]["mods"][1]["enabled"]=true.into();st.commit(&lock,3,"enable-second",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        shards::COPIED_BYTES.with(|n|n.set(0));shards::FORCE_LINK_UNAVAILABLE.with(|v|v.set(true));
        let result=plan.apply(&st,&lock,4,"second", &game_input("native.apply", 4, "second", &input("native.apply","second",4)));
        shards::FORCE_LINK_UNAVAILABLE.with(|v|v.set(false));result.unwrap();
        assert!(shards::COPIED_BYTES.with(|n|n.get())>changed_size);
        restore(&st,&lock,5,"restore", &game_input("native.restore", 5, "restore", &input("native.restore","restore",5))).unwrap();assert_eq!(fs::read(gi).unwrap(),previous);
    }
    fn attach_synthetic_repair(plan:&mut ApplyPlan,st:&Store){attach_synthetic_repair_padded(plan,st,0);}
    fn attach_synthetic_repair_padded(plan:&mut ApplyPlan,st:&Store,padding:usize){
        let mut bytes=crate::test_support::vpk("scripts/repaired.vdata_c",0);bytes.resize(bytes.len()+padding,0);
        let sha=paths::digest_bytes(&bytes).unwrap();let cache=st.root().join("cache/repairs");paths::private_dir(&cache).unwrap();let path=cache.join(format!("{sha}.vpk"));fs::write(&path,&bytes).unwrap();
        let file=package::FileRef{id:sha[..32].into(),sha256:sha,size:bytes.len() as u64,source_name:"pak01_dir.vpk".into()};
        let old=generation::mounts(&plan.manifest(st)).unwrap();
        plan.repairs["file"]=package::encode_file(&file);
        plan.repairs["mods"]=json::array![object!{modId:plan.intent.mods[0].mod_id.clone(),repairs:1}];
        plan.after=gameinfo::replace_many(&plan.before,&HashSet::from(["citadel/addons".into(),"citadel/dmm_localization_default".into()]),&[],&generation::mounts(&plan.manifest(st)).unwrap()).unwrap().0;
        assert!(!old.is_empty());plan.payload_evidence.push((path.clone(),paths::fingerprint(&path).unwrap()));
    }
    #[test]
    fn repair_overlay_transaction_restore_and_recovery(){
        for interrupt in [0,1,2,3]{
        let(f,lock,st,_)=setup();let path=f.game.join("game/citadel/gameinfo.gi");
        let before=text(&path).unwrap().replace("Game citadel/addons","Game citadel/dmm_localization_default\n Game citadel/addons");fs::write(&path,&before).unwrap();
        let upstream=f.game.join("game/citadel/dmm_localization_default");fs::create_dir(&upstream).unwrap();fs::write(upstream.join("opaque"),b"upstream-owned").unwrap();
        let mut plan=ApplyPlan::review(&st,"default",&f.game).unwrap();attach_synthetic_repair(&mut plan,&st);
        let input=|method:&str,id:&str,r:&str|object!{method:method,params:object!{dataDir:f.data.to_str().unwrap(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        if interrupt!=0{let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,1,"repair",&input("native.apply","repair","1"),&crate::prepare_control::PrepareControl::never_cancelled(),&mut |p|if matches!(p,crate::launch_preparation::PreparationProgress::Committing) && interrupt==1 || matches!(p,crate::launch_preparation::PreparationProgress::Copying{files_done:1,..}) && interrupt==2 || matches!(p,crate::launch_preparation::PreparationProgress::Copying{files_done:2,..}) && interrupt==3 {panic!("interrupt")})));assert!(caught.is_err());drop(guard);recover(&st,&lock,1,"recover", &game_input("native.recover", 1, "recover", &input("native.recover","recover","1"))).unwrap();assert_eq!(text(&path).unwrap(),before);
        }else{plan.apply(&st,&lock,1,"repair", &game_input("native.apply", 1, "repair", &input("native.apply","repair","1"))).unwrap();let applied=text(&path).unwrap();assert!(applied.find("/repairs").unwrap()<applied.find("/shard01").unwrap());check_applied(st.root(),&st.document().unwrap(), None).unwrap();
        for(status,expected)in [("fixable","fixed"),("partlyFixable","partlyFixed")]{let view=object!{report:object!{gameBuild:JsonValue::Null,mods:json::array![object!{modId:plan.intent.mods[0].mod_id.clone(),status:status}]}};assert_eq!(crate::health::project(&st,&st.document().unwrap(),view).unwrap()["report"]["mods"][0]["status"],expected);}
        restore(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &input("native.restore","restore","2"))).unwrap();assert_eq!(text(&path).unwrap(),before);}
        assert_eq!(fs::read(upstream.join("opaque")).unwrap(),b"upstream-owned");assert!(st.journals().unwrap().is_empty());
        }
    }
    #[test]
    fn item45_addons_takeover_restore_and_recovery() {
        for interrupt in [true, false] {
            let (f,lock,st,first)=setup();
            let input=|id:&str,r:u64,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
            let mut doc=st.document().unwrap();doc["importLedger"]["manifests"]=json::array![];
            st.commit(&lock,2,"clear-ledger",&object!{},doc,None).unwrap();
            let gi=f.game.join("game/citadel/gameinfo.gi");
            let before="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
            fs::write(&gi,before).unwrap();
            let addons=f.game.join("game/citadel/addons");
            let proof=crate::installed::mount_proof(&addons).unwrap();
            for mount in ["citadel/addons_foreign","Citadel/addons","citadel/Addons","citadel/addons/"] {
                fs::write(&gi,before.replace("citadel/addons",mount)).unwrap();
                assert_eq!(ApplyPlan::review(&st,"default",&f.game).err().unwrap().kind,"unknownMount");
            }
            for key in ["game","GAME","Mod"] {
                fs::write(&gi,before.replace("Game citadel/addons",&format!("{key} citadel/addons"))).unwrap();
                assert_eq!(ApplyPlan::review(&st,"default",&f.game).err().unwrap().kind,"unknownMount");
            }
            fs::write(&gi,before.replace("Game citadel\n","Game citadel/addons_foreign\n Game citadel\n")).unwrap();
            assert_eq!(ApplyPlan::review(&st,"default",&f.game).err().unwrap().kind,"unknownMount");
            fs::write(&gi,before).unwrap();
            let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
            assert!(plan.summary["warnings"].members().any(|w|w["code"]=="originalManagerTakeover" && w["message"].as_str().unwrap().contains("The files stay where they are; Restore puts this back.")));
            if interrupt {
                let guard=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
                let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,3,"again",&input("again",3,"native.apply"),&crate::prepare_control::PrepareControl::never_cancelled(),&mut |p|if matches!(p,crate::launch_preparation::PreparationProgress::Committing){panic!("interrupt")})));
                assert!(caught.is_err());drop(guard);
                recover(&st,&lock,3,"recover", &game_input("native.recover", 3, "recover", &input("recover",3,"native.recover"))).unwrap();
            } else {
                plan.apply(&st,&lock,3,"again", &game_input("native.apply", 3, "again", &input("again",3,"native.apply"))).unwrap();
                assert!(!text(&gi).unwrap().contains("citadel/addons"));
                check_applied(st.root(),&st.document().unwrap(), None).unwrap();
                restore(&st,&lock,4,"restore", &game_input("native.restore", 4, "restore", &input("restore",4,"native.restore"))).unwrap();
                assert_eq!(text(&gi).unwrap(),before);
                let next=ApplyPlan::review(&st,"default",&f.game).unwrap();
                next.apply(&st,&lock,5,"next", &game_input("native.apply", 5, "next", &input("next",5,"native.apply"))).unwrap();
                let applied=text(&gi).unwrap();fs::write(&gi,applied.replace("Game citadel","Game citadel/addons\n Game citadel")).unwrap();
                assert!(check_external(st.root(),&st.document().unwrap(), None).unwrap());
                let mixed=text(&gi).unwrap();
                let again=ApplyPlan::review(&st,"default",&f.game).unwrap();
                assert!(again.summary["warnings"].members().any(|w|w["code"]=="originalManagerTakeover"));
                again.apply(&st,&lock,6,"mixed", &game_input("native.apply", 6, "mixed", &input("mixed",6,"native.apply"))).unwrap();
                check_applied(st.root(),&st.document().unwrap(), None).unwrap();
                restore(&st,&lock,7,"restore-mixed", &game_input("native.restore", 7, "restore-mixed", &input("restore-mixed",7,"native.restore"))).unwrap();
                assert_eq!(text(&gi).unwrap(),mixed);
                assert_eq!(crate::installed::mount_proof(&addons).unwrap(),proof);
                assert!(st.journals().unwrap().is_empty());
                continue;
            }
            assert_eq!(text(&gi).unwrap(),before);
            assert_eq!(crate::installed::mount_proof(&addons).unwrap(),proof);
            assert!(st.journals().unwrap().is_empty());
        }
    }

    #[test]
    fn item45_recovery_after_gameinfo_commit() {
        let(f,lock,st,first)=setup();
        first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
        let mut doc=st.document().unwrap();doc["importLedger"]["manifests"]=json::array![];
        st.commit(&lock,2,"clear",&object!{},doc,None).unwrap();
        let c=citadel(&f.game).unwrap();let gi=c.join("gameinfo.gi");
        let before=text(&gi).unwrap().replace("Game citadel","Game citadel/addons\n Game citadel");
        fs::write(&gi,&before).unwrap();
        let proof=crate::installed::mount_proof(&c.join("addons")).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=|id:&str,r:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        shards::REVIEW7_AFTER_DB.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("power loss after commit"))));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply(&st,&lock,3,"again", &game_input("native.apply", 3, "again", &input("again","3","native.apply"))))).is_err());
        assert!(!st.journals().unwrap().is_empty());
        recover(&st,&lock,4,"recover", &game_input("native.recover", 4, "recover", &input("recover","4","native.recover"))).unwrap();
        check_applied(st.root(),&st.document().unwrap(), None).unwrap();
        let revision=rev(&st.document().unwrap()).unwrap();
        restore(&st,&lock,revision,"restore", &game_input("native.restore", revision, "restore", &input("restore",&revision.to_string(),"native.restore"))).unwrap();
        assert_eq!(text(&gi).unwrap(),before);
        assert_eq!(crate::installed::mount_proof(&c.join("addons")).unwrap(),proof);
        assert!(st.journals().unwrap().is_empty());
    }

    #[test]
    fn item45_missing_addons_restore_records_absence() {
        let(f,lock,st,first)=setup();
        first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &object!{method:"native.apply",params:object!{}})).unwrap();
        let mut doc=st.document().unwrap();doc["importLedger"]["manifests"]=json::array![];
        st.commit(&lock,2,"clear",&object!{},doc,None).unwrap();
        let c=citadel(&f.game).unwrap();fs::remove_dir_all(c.join("addons")).unwrap();
        let before="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
        fs::write(c.join("gameinfo.gi"),before).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=|id:&str,r:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:r,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&st,&lock,3,"again", &game_input("native.apply", 3, "again", &input("again","3","native.apply"))).unwrap();
        let applied=text(&c.join("gameinfo.gi")).unwrap();
        fs::create_dir(c.join("addons")).unwrap();
        assert_eq!(restore(&st,&lock,4,"refused", &game_input("native.restore", 4, "refused", &input("refused","4","native.restore"))).unwrap_err().kind,"externalChange");
        assert_eq!(text(&c.join("gameinfo.gi")).unwrap(),applied);
        fs::remove_dir(c.join("addons")).unwrap();
        restore(&st,&lock,4,"restore", &game_input("native.restore", 4, "restore", &input("restore","4","native.restore"))).unwrap();
        assert_eq!(text(&c.join("gameinfo.gi")).unwrap(),before);
        assert!(!c.join("addons").exists());
    }

    #[test]
    fn external_gameinfo_reapply_preserves_current_bytes() {
        for keep_mount in [false,true] {
        let (f,lock,st,_)=setup();
        let path=f.game.join("game/citadel/gameinfo.gi");
        let input=|id:&str, revision:u64, method:&str| object!{method:method,params:object!{dataDir:f.data.to_str().unwrap(),operationId:id,expectedRevision:revision.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first",1,"native.apply"))).unwrap();
        let original_generation=st.document().unwrap()["applied"]["generation"].as_str().unwrap().to_owned();
        let current=include_str!("../data/interop-external-current.gi");
        let current=if keep_mount { format!("// game update\n{}",text(&path).unwrap()) } else {current.to_owned()};
        fs::write(&path,&current).unwrap();
        let snapshot=st.snapshot(None).unwrap();
        assert_eq!(snapshot["recoveryRequired"],false);
        assert_eq!(snapshot["pendingReason"],"externalGameinfoChange");
        assert_eq!(snapshot["canRestore"],false);
        st.write_journal(&lock,"unfinished",&object!{kind:"game"}).unwrap();
        assert_eq!(st.snapshot(None).unwrap()["recoveryRequired"],true);
        st.clear_journal(&lock,"unfinished").unwrap();
        assert_eq!(restore(&st,&lock,2,"unsafe-restore", &game_input("native.restore", 2, "unsafe-restore", &input("unsafe-restore",2,"native.restore"))).unwrap_err().kind,"externalGameinfoChange");
        assert_eq!(recover(&st,&lock,2,"unsafe-recover", &game_input("native.recover", 2, "unsafe-recover", &input("unsafe-recover",2,"native.recover"))).unwrap_err().kind,"externalGameinfoChange");
        assert_eq!(text(&path).unwrap(),current);
        assert!(st.journals().unwrap().is_empty());
        fs::write(&path,current.replace("citadel/dmm_localization_default", "citadel/addons_foreign")).unwrap();
        if !keep_mount {
            assert_eq!(ApplyPlan::review(&st,"default",&f.game).err().unwrap().kind,"unknownMount");
        }
        fs::write(&path,&current).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        plan.apply(&st,&lock,2,"again", &game_input("native.apply", 2, "again", &input("again",2,"native.apply"))).unwrap();
        if !keep_mount {
            assert!(!text(&path).unwrap().contains("citadel/dmm_localization_default"));
            assert!(!f.game.join("game/citadel/deadlock_native").join(&original_generation).exists());
        }
        restore(&st,&lock,3,"restore-current", &game_input("native.restore", 3, "restore-current", &input("restore-current",3,"native.restore"))).unwrap();
        assert_eq!(text(&path).unwrap(),current);
        }
    }

    #[test]
    fn external_gameinfo_interrupted_reapply_keeps_current_baseline() {
        let (f,lock,st,_)=setup();
        let input=|id:&str,revision:u64,method:&str| object!{method:method,params:object!{dataDir:f.data.to_str().unwrap(),operationId:id,expectedRevision:revision.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let first=ApplyPlan::review(&st,"default",&f.game).unwrap();
        first.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input("first",1,"native.apply"))).unwrap();
        let current=include_str!("../data/interop-external-current.gi");
        let path=f.game.join("game/citadel/gameinfo.gi");
        fs::write(&path,current).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let game_lock=GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let interrupted=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plan.apply_locked(&st,&lock,&game_lock,2,"interrupted",&input("interrupted",2,"native.apply"),
                &crate::prepare_control::PrepareControl::never_cancelled(),&mut |progress| {
                    if matches!(progress,crate::launch_preparation::PreparationProgress::Committing) {panic!("simulated interruption");}
                }).unwrap();
        }));
        assert!(interrupted.is_err()); drop(game_lock);
        assert!(!st.journals().unwrap().is_empty());
        recover(&st,&lock,2,"recover-reapply", &game_input("native.recover", 2, "recover-reapply", &input("recover-reapply",2,"native.recover"))).unwrap();
        assert_eq!(text(&path).unwrap(),current);
        assert!(st.journals().unwrap().is_empty());
        assert_eq!(st.snapshot(None).unwrap()["pendingReason"],"externalGameinfoChange");
    }

    #[test]
    fn external_gameinfo_does_not_hide_damaged_generation() {
        let (f,lock,st,_)=setup();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"first",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&st,&lock,1,"first", &game_input("native.apply", 1, "first", &input)).unwrap();
        fs::write(f.game.join("game/citadel/gameinfo.gi"),include_str!("../data/interop-external-current.gi")).unwrap();
        let generation=st.document().unwrap()["applied"]["generation"].as_str().unwrap().to_owned();
        let manifest=f.game.join("game/citadel/deadlock_native").join(generation).join("manifest.json");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&manifest,fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(manifest,"{}").unwrap();
        let snapshot=st.snapshot(None).unwrap();
        assert_eq!(snapshot["recoveryRequired"],true);
        assert_ne!(snapshot["pendingReason"],"externalGameinfoChange");
    }

    #[test]
    fn interop_native_mount_is_valid_syntax_but_fails_upstream_modded_predicate() {
        let applied=include_str!("../data/interop-native-applied.gi");
        let current=include_str!("../data/interop-external-current.gi");
        // Upstream validate_gameinfo_syntax only checks SearchPaths/braces.
        for text in [applied,current] {
            let block=&text[text.find("SearchPaths").unwrap()..];
            assert!(block.contains('{') && block.contains('}'));
        }
        // validate_gameinfo_patch(false) requires this literal substring,
        // independently of whitespace and quotes around the native mount.
        assert!(!applied.contains("citadel/addons"));
        assert!(!applied.replace('"', "").contains("citadel/addons"));
        assert!(current.contains("citadel/addons"));
        let owned=HashSet::from(["citadel/deadlock_native/00112233445566778899aabbccddeeff/shard01".to_owned()]);
        let mount=owned.iter().next().unwrap();
        for text in [applied.to_owned(),applied.replace(&format!("\"{mount}\""),mount)] {
            gameinfo::replace_many(&text,&owned,&owned.iter().cloned().collect::<Vec<_>>(),&[]).unwrap();
        }
    }

    #[test]
    fn item45_unselected_performance_apply_restore_exact() {
        {
            let (f, lock, st, _) = setup();
            let before = crate::performance::overlay(include_str!("../data/perf-stock.gi"), "dmm-clean").unwrap().replace("ConVars", "EngineFixture\n{\n IndexBufferPoolSizeMB 32 // dmm-perf was 16\n}\nConVars");
            let gi = f.game.join("game/citadel/gameinfo.gi");
            fs::write(&gi, &before).unwrap();
            let plan = ApplyPlan::review(&st,"default",&f.game).unwrap();
            assert_eq!(plan.summary["performance"]["overlayAction"], "preserve");
            let input = object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"perf",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            { plan.apply(&st,&lock,1,"perf", &game_input("native.apply", 1, "perf", &input)).unwrap(); }
            assert_eq!(st.snapshot(Some("default")).unwrap()["pendingStatus"],"current");
            let lines = |s: &str| s.split_inclusive('\n').filter(|s| s.contains("dmm-perf")).collect::<String>();
            assert_eq!(lines(&text(&gi).unwrap()), lines(&before));
            restore(&st,&lock,2,"undo", &game_input("native.restore", 2, "undo", &object!{method:"native.restore",params:object!{dataDir:f.data.to_str(),operationId:"undo",expectedRevision:"2"}})).unwrap();
            assert_eq!(fs::read(&gi).unwrap(), before.as_bytes());
        }
    }
    #[test]
    fn item45_takeover_membership_pinned_to_review() {
        let (f, _lock, st, _) = setup();
        let gi = f.game.join("game/citadel/gameinfo.gi");
        fs::write(&gi, "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        let plan = ApplyPlan::review(&st,"default",&f.game).unwrap();
        fs::write(gi.parent().unwrap().join("addons/new.vpk"), b"foreign").unwrap();
        assert_eq!(plan.recheck(&st).unwrap_err().message, "The game files changed during Apply. Review again.");
    }
    #[test]
    fn item45_final_addons_change_aborts_before_rename() {
        let (f,lock,st,_)=setup();let c=citadel(&f.game).unwrap();
        let live=c.join("gameinfo.gi");
        let before="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
        fs::write(&live,before).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        let added=c.join("addons/late.vpk");let target=added.clone();
        BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(Box::new(move |_|fs::write(target,b"foreign").unwrap())));
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"late",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        assert_eq!(plan.apply(&st,&lock,1,"late", &game_input("native.apply", 1, "late", &input)).unwrap_err().message,"The game files changed during Apply. Review again.");
        assert_eq!(fs::read(&live).unwrap(),before.as_bytes());assert!(added.exists());assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn item45_final_live_change_aborts_before_rename() {
        {
            let (f,lock,st,_)=setup();
            let live=citadel(&f.game).unwrap().join("gameinfo.gi");
            let plan={ApplyPlan::review(&st,"default",&f.game).unwrap()};
            let target=live.clone();
            let hook:Box<dyn FnOnce(&Path)>=Box::new(move |_|fs::write(target,b"LATE EXTERNAL EDIT").unwrap());
            BEFORE_FINAL_RENAME.with(|h|*h.borrow_mut()=Some(hook));
            let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"late",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            let result={plan.apply(&st,&lock,1,"late", &game_input("native.apply", 1, "late", &input))};
            assert_eq!(result.unwrap_err().message,"The game files changed during Apply. Review again.");
            assert_eq!(fs::read(live).unwrap(),b"LATE EXTERNAL EDIT");
            assert!(st.journals().unwrap().is_empty());
            assert_eq!(st.document().unwrap()["revision"],"1");
        }
    }
    #[test]
    fn item45_invalid_game_path_is_actionable() {
        let (f, _lock, st, _) = setup();
        let gi = f.game.join("game/citadel/gameinfo.gi");
        for invalid in [None, Some("not a game")] {
            if let Some(value) = invalid { fs::write(&gi,value).unwrap(); } else { fs::remove_file(&gi).unwrap(); }
            assert_eq!(ApplyPlan::review(&st,"default",&f.game).err().unwrap().kind, "invalidGamePath");
        }
    }
    #[test]
    fn review_f2_version5_empty_selection_journal_recovers_preserved_overlay() {
        for removed in [false,true] {
        let (f,lock,st,_) = setup();
        let before = crate::performance::overlay(include_str!("../data/perf-stock.gi"), "dmm-clean").unwrap();
        let gi = f.game.join("game/citadel/gameinfo.gi");
        fs::write(&gi, &before).unwrap();
        let mut doc = st.document().unwrap();
        doc["performancePreset"] = "".into();
        st.commit(&lock,1,"select-old-empty",&object!{},doc,None).unwrap();
        let mut plan = ApplyPlan::review(&st,"default",&f.game).unwrap();
        if removed { plan.after = crate::performance::patch::strip_overlay(&plan.after).0; }
        let input = object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"old-empty",expectedRevision:"2",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let guard = GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||plan.apply_locked(&st,&lock,&guard,2,"old-empty",&input,&crate::prepare_control::PrepareControl::new(),&mut |at| {
            if matches!(at,crate::launch_preparation::PreparationProgress::Committing) { panic!("interrupted historical journal"); }
        })));
        assert!(stopped.is_err());
        drop(guard);
        let journal = st.journal("old-empty").unwrap().unwrap();
        assert_eq!(journal["version"],5);
        assert_eq!(journal["beforeDocument"]["performancePreset"],"");
        assert_eq!(journal["after"].as_str().unwrap().contains("dmm-perf"), !removed);
        recover(&st,&lock,2,"recover-old-empty", &game_input("native.recover", 2, "recover-old-empty", &object!{method:"native.recover",params:object!{dataDir:f.data.to_str(),operationId:"recover-old-empty",expectedRevision:"2"}})).unwrap();
        assert_eq!(st.journals().unwrap().len(),0);
        assert_eq!(fs::read(&gi).unwrap(),before.as_bytes());
        }
    }

    #[test]
    fn unify_none_uses_reviewed_apply_and_exact_restore() {
        let (f,lock,st,_) = setup();
        let gi=f.game.join("game/citadel/gameinfo.gi");
        fs::write(&gi,include_str!("../data/perf-stock.gi")).unwrap();
        let mut doc=st.document().unwrap();doc["performancePreset"]="dmm-clean".into();
        st.commit(&lock,1,"select-dmm",&object!{},doc,None).unwrap();
        let input=|id:&str,revision:&str,method:&str|object!{method:method,params:object!{dataDir:f.data.to_str(),operationId:id,expectedRevision:revision,token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"apply-dmm", &game_input("native.apply", 2, "apply-dmm", &input("apply-dmm","2","native.apply"))).unwrap();
        let previous=fs::read(&gi).unwrap();
        let mut doc=st.document().unwrap();doc["performancePreset"]="none".into();
        st.commit(&lock,3,"select-none",&object!{},doc,None).unwrap();
        let plan=ApplyPlan::review(&st,"default",&f.game).unwrap();
        assert_eq!(plan.summary["performance"]["overlayAction"],"remove");
        plan.apply(&st,&lock,4,"apply-none", &game_input("native.apply", 4, "apply-none", &input("apply-none","4","native.apply"))).unwrap();
        assert!(!text(&gi).unwrap().contains("dmm-perf"));
        assert_eq!(st.snapshot(Some("default")).unwrap()["pendingStatus"],"current");
        assert!(st.journals().unwrap().is_empty());
        restore(&st,&lock,5,"undo-none", &game_input("native.restore", 5, "undo-none", &input("undo-none","5","native.restore"))).unwrap();
        assert_eq!(fs::read(&gi).unwrap(),previous);
    }

    #[test]
    fn roundtrip_none_refuses_reviewer_input_without_writes() {
        let (f,lock,st,_) = setup();
        let gi=f.game.join("game/citadel/gameinfo.gi");
        fs::write(&gi,include_str!("../data/perf-stock.gi")).unwrap();
        let mut doc=st.document().unwrap();doc["performancePreset"]="dmm-clean".into();
        st.commit(&lock,1,"select-dmm",&object!{},doc,None).unwrap();
        let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str(),operationId:"apply-dmm",expectedRevision:"2",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        ApplyPlan::review(&st,"default",&f.game).unwrap().apply(&st,&lock,2,"apply-dmm", &game_input("native.apply", 2, "apply-dmm", &input)).unwrap();
        let applied=text(&gi).unwrap();
        let edited=applied.replace(crate::performance::patch::END_MARKER,&format!("user_custom KEEP_ME\n// personal comment\n{}\ncl_simulate_dormant_entities 1 user_other 2",crate::performance::patch::END_MARKER));
        fs::write(&gi,&edited).unwrap();
        let mut doc=st.document().unwrap();doc["performancePreset"]="none".into();
        st.commit(&lock,3,"select-none",&object!{},doc,None).unwrap();
        let saved=st.document().unwrap();
        let result=ApplyPlan::review(&st,"default",&f.game).err().expect("must refuse edited performance settings");
        assert_eq!(result.message,"Performance settings were edited outside the app. They have been left unchanged.");
        assert_eq!(fs::read(&gi).unwrap(),edited.as_bytes());
        assert_eq!(st.document().unwrap(),saved);
        assert!(st.journals().unwrap().is_empty());
    }

    #[test]
    fn performance_apply_restore_and_interrupted_apply_are_exact() {
        {for boundary in ["none","renamed","database"] {
            let (f,lock,st,_)=setup();
            let before=include_str!("../data/perf-stock.gi");
            fs::write(f.game.join("game/citadel/gameinfo.gi"),before).unwrap();
            let mut doc=st.document().unwrap();doc["performancePreset"]="dmm-clean".into();
            st.commit(&lock,1,"select",&object!{},doc,None).unwrap();
            let p=ApplyPlan::review(&st,"default",&f.game).unwrap();
            assert_eq!(p.summary["performance"]["presetId"],"dmm-clean");
            let input=object!{method:"native.apply",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"perf-apply",expectedRevision:"2",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            let r={
                if boundary=="none" {p.apply(&st,&lock,2,"perf-apply", &game_input("native.apply", 2, "perf-apply", &input))} else {
                    let c=citadel(&f.game).unwrap();let guard=GameLock::acquire(&c).unwrap();
                    let crash=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||p.apply_locked(&st,&lock,&guard,2,"perf-apply",&input,&crate::prepare_control::PrepareControl::new(),&mut |at|{if matches!(at,crate::launch_preparation::PreparationProgress::Committing){panic!("abrupt stop with staged preset")}})));
                    assert!(crash.is_err());Err(recovery())
                }
            };
            if boundary=="none" {r.unwrap_or_else(|e|panic!("sharded=shards-v1 {e:?}"));}else{r.unwrap_err();recover(&st,&lock,rev(&st.document().unwrap()).unwrap(),"perf-recover", &game_input("native.recover", rev(&st.document().unwrap()).unwrap(), "perf-recover", &object!{method:"native.recover",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"perf-recover",expectedRevision:st.document().unwrap()["revision"].clone()}})).unwrap();}
            let doc=st.document().unwrap();
            if !doc["applied"].is_null(){assert!(text(&f.game.join("game/citadel/gameinfo.gi")).unwrap().contains("dmm-perf"));restore(&st,&lock,rev(&doc).unwrap(),"perf-restore", &game_input("native.restore", rev(&doc).unwrap(), "perf-restore", &object!{method:"native.restore",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"perf-restore",expectedRevision:doc["revision"].clone()}})).unwrap();}
            assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),before.as_bytes());
        }}
    }
    #[test]
    fn opaque_prior_evidence_survives_strict_journal_recovery() {
        for (boundary, tamper) in [("staged", false), ("renamed", false), ("database", false), ("renamed", true)] {
            let f = Fixture::new();
            let bad = f.game.join("game/citadel/addons/broken.vpk");
            fs::write(&bad, b"opaque prior bytes").unwrap();
            let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
            manifest["mods"]["broken"] = object! {enabled:true,shard:1,currentVpks:json::array!["broken.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["broken.vpk"]};
            f.write_manifest("", manifest);
            let before = "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
            fs::write(f.game.join("game/citadel/gameinfo.gi"), before).unwrap();
            import(&mut crate::native::NativeService::new(), &f);
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let p = ApplyPlan::review(&st, "default", &f.game).unwrap();
            apply_at(&p,&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {method:"native.apply"}), move |at| {
                if at == boundary { Err(err("nativeIo", "injected interruption")) } else { Ok(()) }
            }).unwrap_err();
            if tamper { fs::write(&bad, b"changed").unwrap(); }
            drop(st);
            drop(lock);
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let r = recover(&st, &lock, rev(&st.document().unwrap()).unwrap(), "recover", &game_input("native.recover", rev(&st.document().unwrap()).unwrap(), "recover", &object! {method:"native.recover"}));
            if tamper {
                assert_eq!(r.unwrap_err().kind, "externalChange");
            } else {
                r.unwrap();
                assert!(st.journals().unwrap().is_empty());
                if !st.document().unwrap()["applied"].is_null() {
                    restore(&st, &lock, rev(&st.document().unwrap()).unwrap(), "restore", &game_input("native.restore", rev(&st.document().unwrap()).unwrap(), "restore", &object! {})).unwrap();
                }
                assert_eq!(text(&f.game.join("game/citadel/gameinfo.gi")).unwrap(), before);
                assert_eq!(fs::read(&bad).unwrap(), b"opaque prior bytes");
            }
        }
    }
    #[test]
    fn restore_guard_failure_before_temp_does_not_leave_recovery_journal() {
        let (f,lock,st)=historical_restore_setup();
        let before=fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        let e=restore_with(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &object!{}),&mut |at| {
            if at=="beforeTemp" {Err(err("gameGuardUnavailable","injected incomplete scan"))}else{Ok(())}
        }).unwrap_err();
        assert_eq!(e.kind,"gameGuardUnavailable");
        assert!(st.journals().unwrap().is_empty());
        assert_eq!(rev(&st.document().unwrap()).unwrap(),2);
        assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),before);
        restore(&st,&lock,2,"retry", &game_input("native.restore", 2, "retry", &object!{})).unwrap();
    }
    #[test]
    fn current_recorded_directory_boundary_recovers_but_unknown_entries_do_not() {
        for unexpected in [false,true] {
            let (f,lock,st,p)=setup();
            let generation=p.generation.clone();let game=f.game.clone();
            apply_at(&p,&st,&lock,1,"apply", &game_input("native.apply", 1, "apply", &object!{}),move |at| {
                if at=="generationCreated" {
                    if unexpected { fs::write(game.join("game/citadel/deadlock_native").join(&generation).join("foreign"),b"keep").unwrap(); }
                    Err(err("nativeIo","stop"))
                } else {Ok(())}
            }).unwrap_err();
            let result=recover(&st,&lock,1,"recover", &game_input("native.recover", 1, "recover", &object!{}));
            if unexpected {assert_eq!(result.unwrap_err().kind,"externalChange");}
            else {result.unwrap();assert!(st.journals().unwrap().is_empty());}
        }
    }
    #[test]
    fn restore_removes_owned_workspace_and_recovers_marker_unlink_boundary() {
        for boundary in ["none","workspaceMarkerRemoved"] {
            let (f,lock,st)=historical_restore_setup();
            let r=restore_with(&st,&lock,2,"restore", &game_input("native.restore", 2, "restore", &object!{}),&mut |at| {
                if at==boundary {Err(err("nativeIo","stop"))}else{Ok(())}
            });
            if boundary!="none" {r.unwrap_err();recover(&st,&lock,3,"recover", &game_input("native.recover", 3, "recover", &object!{})).unwrap();}else{r.unwrap();}
            assert!(!f.game.join("game/citadel/deadlock_native").exists());
        }
    }
    #[test]
    fn persisted_boundaries_recover_before_or_after_without_guessing() {
        for boundary in [
            "journal",
            "stageStart",
            "stageFile",
            "staged",
            "beforeRename",
            "renamed",
            "beforeDatabase",
            "database",
        ] {
            let (f, lock, st, p) = setup();
            let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
            let result = apply_at(&p,
                &st,
                &lock,
                1,
                "apply", &game_input("native.apply", 1, "apply", &object! {method:"native.apply"}),
                move |at| {
                    if at == boundary {
                        Err(err("nativeIo", "injected interruption"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result.unwrap_err().kind, "recoveryRequired");
            assert_eq!(st.journals().unwrap().len(), 1);
            drop(st);
            drop(lock);
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let current = rev(&st.document().unwrap()).unwrap();
            recover(
                &st,
                &lock,
                current,
                "recover", &game_input("native.recover", current, "recover", &object! {method:"native.recover"}),
            )
            .unwrap();
            assert_eq!(st.journals().unwrap().len(), 0);
            let d = st.document().unwrap();
            if matches!(boundary, "renamed" | "beforeDatabase" | "database") {
                assert_eq!(d["applied"]["profileId"], "default");
                check_applied(st.root(), &d, None).unwrap();
                assert_eq!(st.operation("apply").unwrap()["status"], "succeeded");
            } else {
                assert!(d["applied"].is_null());
                assert_eq!(
                    fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                    before
                );
                assert_eq!(st.operation("apply").unwrap()["status"], "failed");
            }
        }
    }
    #[test]
    fn external_edit_during_interruption_preserves_journal_and_all_external_bytes() {
        let (f, lock, st, p) = setup();
        apply_at(&p,&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}), move |at| {
            if at == "renamed" {
                Err(err("nativeIo", "stop"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        let path = f.game.join("game/citadel/gameinfo.gi");
        fs::write(&path, "external config").unwrap();
        assert_eq!(
            recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {}))
                .unwrap_err()
                .kind,
            "externalChange"
        );
        assert!(st.journal("apply").unwrap().is_some());
        assert_eq!(fs::read(path).unwrap(), b"external config");
    }
    #[test]
    fn oversized_projected_state_rejects_apply_restore_and_recovery_before_journaling() {
        for action in ["apply", "restore", "recover"] {
            let (f, lock, st, plan) = setup();
            if action == "restore" {
                plan.apply(&st, &lock, 1, "first-apply", &game_input("native.apply", 1, "first-apply", &object! {}))
                    .unwrap();
            }
            // Seed an older/unqualified store directly. The production commit
            // guard must not be the only check before a game transaction.
            let db = crate::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
            let mut doc = st.document().unwrap();
            doc["sourcePath"] = "x".repeat(crate::protocol::MAX_RESPONSE_BYTES).into();
            db.execute("UPDATE library SET body=? WHERE id=1", &[&doc.dump()])
                .unwrap();
            let before = text(&f.game.join("game/citadel/gameinfo.gi")).unwrap();
            let revision = rev(&doc).unwrap();
            let result = match action {
                "apply" => plan.apply(&st, &lock, revision, action, &game_input("native.apply", revision, action, &object! {})),
                "restore" => restore(&st, &lock, revision, action, &game_input("native.restore", revision, action, &object! {})),
                _ => recover(&st, &lock, revision, action, &game_input("native.recover", revision, action, &object! {})),
            };
            assert_eq!(
                result.err().map(|e| e.kind).as_deref(),
                Some("nativeResponseLimit"),
                "{action}"
            );
            assert_eq!(st.document().unwrap(), doc);
            assert_eq!(st.journals().unwrap().len(), 0, "{action}");
            assert_eq!(
                text(&f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                before
            );
            if action != "restore" {
                assert!(!f.game.join("game/citadel/deadlock_native").exists());
            }
        }
    }
    #[test]
    fn backup_database_failure_prevents_first_game_mutation() {
        let (f, lock, st, p) = setup();
        let db = crate::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
        db.execute("CREATE TRIGGER deny_journal BEFORE INSERT ON journals BEGIN SELECT RAISE(FAIL, 'test backup unavailable'); END", &[]).unwrap();
        let r = p.apply(&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}));
        assert!(r.is_err());
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
        assert_eq!(
            text(&f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            p.before
        );
    }
    #[test]
    fn recovery_reclaims_recorded_staging_but_preserves_unknown_files() {
        for unknown in [false, true] {
            let (f, lock, st, p) = setup();
            apply_at(&p,
                &st,
                &lock,
                1,
                "apply", &game_input("native.apply", 1, "apply", &object! {method:"native.apply"}),
                move |at| {
                    if at == "staged" {
                        Err(err("nativeIo", "stop"))
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            let dir = f
                .game
                .join("game/citadel/deadlock_native")
                .join(&p.generation);
            if unknown {
                fs::write(dir.join("unknown"), "keep").unwrap();
            }
            let result = recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {}));
            if unknown {
                assert_eq!(result.unwrap_err().kind, "externalChange");
                assert!(st.journal("apply").unwrap().is_some());
                assert_eq!(fs::read(dir.join("unknown")).unwrap(), b"keep");
            } else {
                result.unwrap();
                assert!(
                    !dir.exists(),
                    "uncommitted owned generation must be reclaimed"
                );
            }
        }
    }
    #[test]
    fn unfinished_operation_rejects_changed_input_and_mutations() {
        let (f, lock, st, p) = setup();
        apply_at(&p,
            &st,
            &lock,
            1,
            "apply", &game_input("native.apply", 1, "apply", &object! {method:"native.apply",params:object!{token:"0123456789abcdef0123456789abcdef"}}),
            move |at| {
                if at == "journal" {
                    Err(err("nativeIo", "stop"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(
            st.replay(
                "apply",
                &object! {method:"native.apply",params:object!{token:"changed"}}
            )
            .unwrap_err()
            .kind,
            "operationIdConflict"
        );
        drop(st);
        drop(lock);
        let mut service = crate::native::NativeService::new();
        let mut params = crate::test_support::mutation(&f, "1", "disable");
        params["profileId"] = "default".into();
        params["modId"] = "local-a".into();
        params["enabled"] = false.into();
        assert_eq!(
            service
                .dispatch("native.setEnabled", &params)
                .unwrap_err()
                .kind,
            "recoveryRequired"
        );
    }
    #[test]
    fn recovery_itself_replays_after_original_commit_and_own_database_failure() {
        let (f, lock, st, p) = setup();
        apply_at(&p,
            &st,
            &lock,
            1,
            "apply", &game_input("native.apply", 1, "apply", &object! {method:"native.apply"}),
            move |at| {
                if at == "renamed" {
                    Err(err("nativeIo", "stop"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        let db = crate::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
        db.execute("CREATE TRIGGER deny_recover BEFORE INSERT ON operations WHEN NEW.id='recover' BEGIN SELECT RAISE(FAIL, 'test interrupted recovery'); END",&[]).unwrap();
        let input = object! {method:"native.recover",params:object!{operationId:"recover",expectedRevision:"1"}};
        assert!(recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &input)).is_err());
        assert_eq!(st.operation("apply").unwrap()["status"], "succeeded");
        db.execute("DROP TRIGGER deny_recover", &[]).unwrap();
        let result = recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &input)).unwrap();
        assert_eq!(result["appliedProfileId"], "default");
        assert_eq!(st.journals().unwrap().len(), 0);
        assert_eq!(st.replay("recover", &input).unwrap(), Some(result));
    }
    #[test]
    fn interruption_during_payload_copy_reclaims_only_recorded_file_identity() {
        let (f, lock, st, p) = setup_padded(256 * 1024);
        let result = apply_at(&p,&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}), move |at| {
            if at == "copyChunk" {
                Err(err("nativeIo", "stop mid-copy"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err().kind, "recoveryRequired");
        let dir = f
            .game
            .join("game/citadel/deadlock_native")
            .join(&p.generation);
        assert!(fs::metadata(dir.join("shard01/pak01_dir.vpk")).unwrap().len() < 256 * 1024);
        recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {})).unwrap();
        assert!(!dir.exists());
    }
    fn family_test_setup() -> (Fixture, RootLock, Store, ApplyPlan) {family_test_setup_padded(128*1024)}
    fn family_test_setup_padded(padding:usize) -> (Fixture, RootLock, Store, ApplyPlan) {
        let f = Fixture::new();
        let paths = crate::test_support::family_fixture(&f, false);
        let bytes = crate::test_support::family_vpk(
            1,
            &[
                crate::test_support::FamilyEntry {
                    path: "models/a.vmdl_c",
                    crc: 7,
                    archive: 0,
                    offset: 0,
                    length: 3,
                    preload: b"pre",
                },
                crate::test_support::FamilyEntry {
                    path: "materials/a.vmat_c",
                    crc: 8,
                    archive: 17,
                    offset: 0,
                    length: 4,
                    preload: b"",
                },
            ],
            &vec![0; padding],
        );
        fs::write(&paths[0], bytes).unwrap();
        fs::write(&paths[1], vec![1; padding]).unwrap();
        fs::write(&paths[2], vec![2; padding]).unwrap();
        fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        import(&mut crate::native::NativeService::new(), &f);
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        let p = ApplyPlan::review(&st, "default", &f.game).unwrap();
        (f, lock, st, p)
    }
    #[test]
    fn family_recovery_preserves_foreign_identity_and_unknown_files() {
        for unknown in [false, true] {
            let (f, lock, st, p) = family_test_setup();
            let mut copied = 0;
            apply_at(&p,&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}), move |at| {
                if at == "stageFile" {
                    copied += 1;
                    if copied == 2 {
                        return Err(err("nativeIo", "stop"));
                    }
                }
                Ok(())
            })
            .unwrap_err();
            let dir = f
                .game
                .join("game/citadel/deadlock_native")
                .join(&p.generation);
            let member = dir.join("shard01/pak01_000.vpk");
            let bytes = fs::read(&member).unwrap();
            if unknown {
                fs::write(dir.join("unknown"), b"foreign").unwrap();
            } else {
                fs::rename(&member, dir.join("saved-inode")).unwrap();
                fs::write(&member, &bytes).unwrap();
                fs::remove_file(dir.join("saved-inode")).unwrap();
            }
            assert!(recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {})).is_err());
            assert!(st.journal("apply").unwrap().is_some());
            assert_eq!(fs::read(&member).unwrap(), bytes);
            assert!(dir.join("shard01/pak01_dir.vpk").exists());
        }
    }
    #[test]
    fn family_recovery_boundary_matrix() {
        for boundary in [
            "journal",
            "stageStart",
            "copyChunk",
            "stageFile",
            "manifestCreated",
            "staged",
            "beforeRename",
            "renamed",
            "beforeDatabase",
            "database",
        ] {
            for occurrence in if boundary == "copyChunk" {
                vec![1, 4, 6]
            } else if boundary == "stageFile" {
                vec![1, 2, 3]
            } else {
                vec![1]
            } {
                let (f, lock, st, p) = family_test_setup();
                let mut sources: Vec<_> = [
                    "renamed-directory.vpk",
                    "unrelated-a.vpk",
                    "unrelated-b.vpk",
                ]
                .into_iter()
                .map(|n| {
                    let path = f.game.join("game/citadel/addons").join(n);
                    let bytes = fs::read(&path).unwrap();
                    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
                    (path, bytes, mtime)
                })
                .collect();
                for path in [f.source.clone(), f.manifest_path("")] {
                    let bytes = fs::read(&path).unwrap();
                    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
                    sources.push((path, bytes, mtime));
                }
                let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
                let mut seen = 0;
                apply_at(&p,&st, &lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}), move |at| {
                    if at == boundary {
                        seen += 1;
                        if seen == occurrence {
                            return Err(err("nativeIo", "stop"));
                        }
                    }
                    Ok(())
                })
                .unwrap_err();
                drop(st);
                drop(lock);
                let lock = RootLock::acquire(&f.data).unwrap();
                let st = Store::open_writer(&lock).unwrap();
                recover(
                    &st,
                    &lock,
                    rev(&st.document().unwrap()).unwrap(),
                    "recover", &game_input("native.recover", rev(&st.document().unwrap()).unwrap(), "recover", &object! {}),
                )
                .unwrap();
                if matches!(boundary, "renamed" | "beforeDatabase" | "database") {
                    check_applied(st.root(), &st.document().unwrap(), None).unwrap();
                    assert_eq!(st.operation("apply").unwrap()["status"], "succeeded");
                } else {
                    assert_eq!(
                        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                        before
                    );
                    assert_eq!(st.operation("apply").unwrap()["status"], "failed");
                }
                for (path, bytes, mtime) in sources {
                    assert_eq!(fs::read(&path).unwrap(), bytes);
                    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), mtime);
                }
            }
        }
    }
    fn restore_source_setup(version:u32)->(Fixture,RootLock,Store,ApplyPlan) {
        if version == 2 {
            family_test_setup()
        } else {
            let f = Fixture::new();
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let mut doc=json::parse(r#"{"storeVersion":1,"revision":"0","sourcePath":"/historical/state.json","gamePath":"","activeProfileId":"default","profiles":{"default":{"id":"default","name":"Historical","protected":true,"mods":[{"source":{"id":"old","remoteId":"old","name":"Old standalone"},"enabled":true,"available":true,"payloads":[{"id":"44444444444444444444444444444444","sha256":"","size":0,"sourceName":"old_dir.vpk"}]}]}},"importLedger":{"manifests":[],"sourcePayloads":[]},"applied":null,"canRestore":false,"recoveryRequired":false}"#).unwrap();
            let bytes = crate::test_support::vpk("materials/old.vmat_c", 3);
            let file = &mut doc["profiles"]["default"]["mods"][0]["payloads"][0];
            file["sha256"] = paths::digest_bytes(&bytes).unwrap().into();
            file["size"] = bytes.len().into();
            paths::private_dir(&f.data.join("payloads")).unwrap();
            fs::write(
                st.payload_path(file["id"].as_str().unwrap()).unwrap(),
                bytes,
            )
            .unwrap();
            doc["gamePath"] = f.game.to_str().unwrap().into();
            st.commit(&lock, 0, "import", &object! {}, doc, None)
                .unwrap();
            fs::write(
                f.game.join("game/citadel/gameinfo.gi"),
                "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n",
            )
            .unwrap();
            let plan = ApplyPlan::review(&st, "default", &f.game).unwrap();
            (f, lock, st, plan)
        }
    }
    fn interrupted_current_shard_restore_case(version: u32, boundary: &'static str) {
        let(f,lock,st,first)=restore_source_setup(version);
        first.apply(&st, &lock, 1, "apply-a", &game_input("native.apply", 1, "apply-a", &object! {})).unwrap();
        let a = st.document().unwrap()["applied"].clone();
        let after_a = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        ApplyPlan::review(&st, "default", &f.game)
            .unwrap()
            .apply(&st, &lock, 2, "apply-b", &game_input("native.apply", 2, "apply-b", &object! {}))
            .unwrap();
        let b = st.document().unwrap()["applied"].clone();
        let before_restore = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        let c = f.game.join("game/citadel");
        let mut preserved = Vec::new();
        for applied in [&a, &b] {
            let dir = generation_dir(&c, applied).unwrap();
            for name in generation::decode(&applied["manifest"])
                .unwrap().files
                .into_iter()
                .map(|f| f.name)
                .chain(std::iter::once("manifest.json".into()))
            {
                let path = dir.join(name);
                preserved.push((path.clone(), paths::fingerprint(&path).unwrap()));
            }
        }
        assert_eq!(
            restore_at(&st, &lock, 3, "restore-fault", &game_input("native.restore", 3, "restore-fault", &object! {}), move |at| {
                if at == boundary {
                    Err(err("nativeIo", "interrupted Restore"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .kind,
            "recoveryRequired"
        );
        assert!(st.journal("restore-fault").unwrap().unwrap()["generation"].is_null());
        drop(st);
        drop(lock);
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        recover(&st, &lock, 3, "recover-restore", &game_input("native.recover", 3, "recover-restore", &object! {})).unwrap();
        assert_eq!(st.journals().unwrap().len(), 0);
        let doc = st.document().unwrap();
        assert_eq!(doc["storeVersion"], 4);
        assert_eq!(doc["packageVersion"], version);
        if boundary == "renamed" {
            assert_eq!(doc["applied"]["generation"], a["generation"]);
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(), after_a);
            assert_eq!(doc["canRestore"], false);
            assert_eq!(
                st.operation("restore-fault").unwrap()["status"],
                "succeeded"
            );
            check_applied(st.root(), &doc, None).unwrap();
        } else {
            for (path, fp) in preserved {
                assert_eq!(
                    paths::fingerprint(&path).unwrap(),
                    fp,
                    "current shards package{version} {boundary}: committed generation changed"
                );
            }
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(), before_restore);
            assert_eq!(doc["applied"]["generation"], b["generation"]);
            assert_eq!(doc["previous"]["applied"]["generation"], a["generation"]);
            assert_eq!(doc["canRestore"], true);
            assert_eq!(st.operation("restore-fault").unwrap()["status"], "failed");
            restore(&st, &lock, rev(&doc).unwrap(), "restore-retry", &game_input("native.restore", rev(&doc).unwrap(), "restore-retry", &object! {})).unwrap();
            assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(), after_a);
            check_applied(st.root(), &st.document().unwrap(), None).unwrap();
        }
    }
    #[test]
    fn interrupted_current_shard_restore_package1_preserves_both_generations() {
        for boundary in ["journal", "beforeRename"] {
            interrupted_current_shard_restore_case(1, boundary);
        }
    }
    #[test]
    fn interrupted_current_shard_restore_package2_preserves_both_generations() {
        for boundary in ["journal", "beforeRename"] {
            interrupted_current_shard_restore_case(2, boundary);
        }
    }
    #[test]
    fn interrupted_current_shard_restore_after_rename_finishes_existing_generation() {
        for version in [1, 2] {
            interrupted_current_shard_restore_case(version, "renamed");
        }
    }
    #[test]
    fn literal_historical_store1_generation_and_game_journal_recover() {
        for (committed, replaced) in [(false, false), (true, false), (false, true)] {
            let f = Fixture::new();
            let before =
                "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n";
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let mut initial=json::parse(r#"{"storeVersion":1,"revision":"0","sourcePath":"/historical/state.json","gamePath":"","activeProfileId":"default","profiles":{"default":{"id":"default","name":"Historical","protected":true,"mods":[]}},"importLedger":{"manifests":[],"sourcePayloads":[]},"applied":null,"canRestore":false,"recoveryRequired":false}"#).unwrap();
            initial["gamePath"] = f.game.to_str().unwrap().into();
            st.commit(&lock, 0, "import", &object! {}, initial, None)
                .unwrap();
            let gen = "1".repeat(32);
            let c = f.game.join("game/citadel");
            let dir = c.join("deadlock_native").join(&gen);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                c.join("deadlock_native/owner.json"),
                object! {version:1,root:f.data.to_str().unwrap()}.dump(),
            )
            .unwrap();
            let bytes = crate::test_support::vpk("materials/old.vmat_c", 3);
            fs::write(dir.join("pak01_dir.vpk"), &bytes).unwrap();
            let mut manifest=json::parse(r#"{"version":1,"storeRoot":"","generation":"11111111111111111111111111111111","files":[{"id":"22222222222222222222222222222222","sha256":"","size":0,"sourceName":"old_dir.vpk","modId":"old","modIds":["old"]}]}"#).unwrap();
            manifest["storeRoot"] = f.data.to_str().unwrap().into();
            manifest["files"][0]["sha256"] = paths::digest_bytes(&bytes).unwrap().into();
            manifest["files"][0]["size"] = bytes.len().into();
            fs::write(dir.join("manifest.json"), manifest.dump()).unwrap();
            let after = before.replace(
                "Game citadel",
                &format!("Game citadel/deadlock_native/{gen}\n Game citadel"),
            );
            let mut j=json::parse(r#"{"kind":"game","version":1,"input":{},"expectedRevision":"1","citadel":"","before":"","after":"","beforeSha256":"","afterSha256":"","document":{},"workspaceExisted":true,"temp":".native-gameinfo-33333333333333333333333333333333","generation":"11111111111111111111111111111111"}"#).unwrap();
            let mut doc = st.document().unwrap();
            doc["applied"] = object! {profileId:"default",enabled:object!{},generation:gen,manifest:manifest,gameinfo:after.clone(),gameinfoSha256:paths::digest_bytes(after.as_bytes()).unwrap()};
            j["document"] = doc;
            j["citadel"] = c.to_str().unwrap().into();
            j["before"] = before.into();
            j["after"] = after.clone().into();
            j["beforeSha256"] = paths::digest_bytes(before.as_bytes()).unwrap().into();
            j["afterSha256"] = paths::digest_bytes(after.as_bytes()).unwrap().into();
            fs::write(
                c.join("gameinfo.gi"),
                if committed { after.as_str() } else { before },
            )
            .unwrap();
            if replaced {
                let path = dir.join("pak01_dir.vpk");
                let m = fs::metadata(&path).unwrap();
                j["stagedFiles"] =
                    json::array![object! {name:"pak01_dir.vpk",device:m.dev(),inode:m.ino()}];
                fs::rename(&path, f.root.join("old-inode")).unwrap();
                fs::write(&path, &bytes).unwrap();
            }
            st.write_journal(&lock, "historical", &j).unwrap();
            if replaced {
                assert!(recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {})).is_err());
                assert!(dir.join("pak01_dir.vpk").exists());
                assert!(st.journal("historical").unwrap().is_some());
                continue;
            }
            recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {})).unwrap();
            assert_eq!(st.document().unwrap()["storeVersion"], 1);
            assert_eq!(st.journals().unwrap().len(), 0);
            if committed {
                check_applied(st.root(), &st.document().unwrap(), None).unwrap();
                assert_eq!(st.operation("historical").unwrap()["status"], "succeeded");
            } else {
                assert!(!dir.exists());
                assert_eq!(fs::read(c.join("gameinfo.gi")).unwrap(), before.as_bytes());
            }
            let p = ApplyPlan::review(&st, "default", &f.game).unwrap();
            let revision = rev(&st.document().unwrap()).unwrap();
            p.apply(&st, &lock, revision, "legacy-apply", &game_input("native.apply", revision, "legacy-apply", &object! {}))
                .unwrap();
            let doc = st.document().unwrap();
            assert_eq!(doc["applied"]["manifest"]["version"], 3);
            assert_eq!(doc["packageVersion"], 1);
            restore(
                &st,
                &lock,
                rev(&doc).unwrap(),
                "legacy-restore", &game_input("native.restore", rev(&doc).unwrap(), "legacy-restore", &object! {}),
            )
            .unwrap();
            assert_eq!(st.document().unwrap()["storeVersion"], 4);
            assert_eq!(st.document().unwrap()["packageVersion"], 1);
        }
    }
    #[test]
    fn family_disk_write_failure_recovers_recorded_partial_companion() {
        const CHILD: &str = "DMM_TEST_FAMILY_DISK_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "apply::tests::family_disk_write_failure_recovers_recorded_partial_companion",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        #[repr(C)]
        struct Limit {
            cur: u64,
            max: u64,
        }
        unsafe extern "C" {
            fn getrlimit(resource: i32, limit: *mut Limit) -> i32;
            fn setrlimit(resource: i32, limit: *const Limit) -> i32;
            fn signal(sig: i32, handler: usize) -> usize;
        }
        // Above the current shard journal database but below the 2 MiB companion,
        // so only the companion copy is cut short.
        const PARTIAL: u64 = 1024 * 1024;
        let (f, lock, st, p) = family_test_setup_padded(2 * 1024 * 1024);
        let mut old = Limit { cur: 0, max: 0 };
        unsafe {
            assert_eq!(getrlimit(1, &mut old), 0);
            signal(25, 1);
        }
        let mut copied = 0;
        let result = apply_at(&p,&st, &lock, 1, "disk-failure", &game_input("native.apply", 1, "disk-failure", &object! {}), move |at| {
            if at == "stageFile" {
                copied += 1;
                if copied == 1 {
                    unsafe {
                        assert_eq!(
                            setrlimit(
                                1,
                                &Limit {
                                    cur: PARTIAL,
                                    max: old.max
                                }
                            ),
                            0
                        );
                    }
                }
            }
            Ok(())
        });
        unsafe {
            assert_eq!(setrlimit(1, &old), 0);
        }
        assert_eq!(result.unwrap_err().kind, "recoveryRequired");
        let dir = f
            .game
            .join("game/citadel/deadlock_native")
            .join(&p.generation);
        assert_eq!(
            fs::metadata(dir.join("shard01/pak01_000.vpk")).unwrap().len(),
            PARTIAL
        );
        recover(&st, &lock, 1, "recover", &game_input("native.recover", 1, "recover", &object! {})).unwrap();
        assert!(!dir.exists());
    }
    #[test]
    fn game_guard_matches_only_executable_or_argv_zero() {
        assert!(crate::launch_process::game_name(b"Z:\\game\\deadlock.exe\0-x"));
        assert!(crate::launch_process::game_name(b"/game/Deadlock.EXE\0"));
        assert!(!crate::launch_process::game_name(b"/bin/sh\0deadlock.exe\0"));
        assert!(!crate::launch_process::game_name(b"/wrapper/deadlock.exe-helper\0"));
    }
    #[test]
    fn process_scan_tolerates_long_command_lines() {
        let f = Fixture::new();
        let proc_root = f.root.join("proc");
        let process = proc_root.join("321");
        fs::create_dir_all(&process).unwrap();
        let fields = vec!["0"; 20].join(" ");
        fs::write(process.join("stat"), format!("321 (rustc) S {fields}")).unwrap();
        fs::write(process.join("status"), "Tgid:\t321\nUid:\t1000\t1000\t1000\t1000\n").unwrap();
        std::os::unix::fs::symlink("/usr/bin/rustc", process.join("exe")).unwrap();
        let mut cmdline = b"/usr/bin/rustc\0".to_vec();
        cmdline.extend(std::iter::repeat_n(b'x', 40_000));
        fs::write(process.join("cmdline"), cmdline).unwrap();
        assert!(crate::launch_process::scan_at(&proc_root).complete);
        game_guard_at(&proc_root).unwrap();
    }
    #[test]
    fn real_process_scan_completes_despite_other_users_processes() {
        // /proc/1 (systemd) hides its exe link from ordinary users.
        assert!(crate::launch_process::scan_at(Path::new("/proc")).complete);
    }
    #[test]
    fn process_scan_blocks_game_executable_and_proton_argv_zero_but_not_wrappers() {
        let f = Fixture::new();
        let proc_root = f.root.join("proc");
        let process = proc_root.join("123");
        fs::create_dir_all(&process).unwrap();
        let fields = vec!["0"; 20].join(" ");
        fs::write(process.join("stat"), format!("123 (x) S {fields}")).unwrap();
        fs::write(process.join("status"), "Tgid:\t123\nUid:\t1000\t1000\t1000\t1000\n").unwrap();
        std::os::unix::fs::symlink("/bin/wrapper", process.join("exe")).unwrap();
        fs::write(process.join("cmdline"), b"/bin/wrapper\0deadlock.exe\0").unwrap();
        game_guard_at(&proc_root).unwrap();
        fs::write(process.join("cmdline"), b"Z:\\game\\deadlock.exe\0").unwrap();
        assert_eq!(game_guard_at(&proc_root).unwrap_err().kind, "gameRunning");
        fs::write(process.join("cmdline"), b"MainThrd\0").unwrap();
        fs::remove_file(process.join("exe")).unwrap();
        std::os::unix::fs::symlink("/game/deadlock.exe", process.join("exe")).unwrap();
        assert_eq!(game_guard_at(&proc_root).unwrap_err().kind, "gameRunning");
    }
    #[test]
    fn directory_lock_coordinates_different_library_roots() {
        let (f, _lock, st, p) = setup();
        let _game = GameLock::acquire(&citadel(&f.game).unwrap()).unwrap();
        assert_eq!(
            p.apply(&st, &_lock, 1, "apply", &game_input("native.apply", 1, "apply", &object! {}))
                .unwrap_err()
                .kind,
            "nativeBusy"
        );
    }
    #[test]
    fn mixed_local_and_game_recovery_preflights_live_ownership_before_any_write() {
        for boundary in ["beforeRename", "renamed"] {
            let (f, lock, store, game) = setup();
            apply_at(&game,
                &store,
                &lock,
                1,
                "z-game", &game_input("native.apply", 1, "z-game", &object! {}),
                move |at| {
                    if at == boundary {
                        Err(err("nativeIo", "fixture crash"))
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            let game_journal = store.journal("z-game").unwrap().unwrap();
            store.clear_journal(&lock, "z-game").unwrap();
            let source = f.root.join("local.vpk");
            fs::write(&source, crate::test_support::vpk("local.bin", 99)).unwrap();
            let prepared = crate::local_content::prepare_local(
                crate::local_content::LocalPrepareRequest {
                    root: f.data.clone(),
                    profile_id: "default".into(),
                    expected_revision: 1,
                    roots: vec![source],
                    artwork: None,
                },
                &crate::prepare_control::PrepareControl::new(),
            )
            .unwrap();
            let local = crate::local_content::review_local(
                &prepared,
                crate::local_content::LocalSelection {
                    candidate_ids: vec![prepared.candidates[0].id.clone()],
                    metadata: prepared.default_metadata(),
                },
            )
            .unwrap();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::local_import::commit_local(
                    &local,
                    &lock,
                    1,
                    "a-local",
                    &object! {local:true},
                    &mut |at| {
                        if at == "published" {
                            panic!("fixture abrupt stop")
                        };
                        Ok(())
                    },
                )
            }));
            store.write_journal(&lock, "z-game", &game_journal).unwrap();
            let c = f.game.join("game/citadel");
            if boundary == "beforeRename" {
                let temp = c.join(game_journal["temp"].as_str().unwrap());
                let bytes = fs::read(&temp).unwrap();
                let replacement = c.join("fixture-replaced-temp");
                fs::write(&replacement, bytes).unwrap();
                fs::rename(replacement, &temp).unwrap();
            } else {
                fs::write(c.join("gameinfo.gi"), "foreign live gameinfo").unwrap();
            }
            let journals = store.journals().unwrap();
            let document = store.document().unwrap();
            let local_journal = store.journal("a-local").unwrap().unwrap();
            let stage = f.data.join(local_journal["stage"].as_str().unwrap());
            let files = fs::read_dir(&stage)
                .unwrap()
                .map(|e| {
                    let p = e.unwrap().path();
                    let bytes = fs::read(&p).unwrap();
                    (p, bytes)
                })
                .collect::<Vec<_>>();
            assert!(recover(&store, &lock, 1, "recover-mixed", &game_input("native.recover", 1, "recover-mixed", &object! {})).is_err());
            assert_eq!(store.document().unwrap(), document);
            assert_eq!(store.journals().unwrap(), journals);
            assert_eq!(
                store.operation("recover-mixed").unwrap()["status"],
                "unknown"
            );
            assert_eq!(
                store.operation("a-local").unwrap()["status"],
                "recoveryRequired"
            );
            for (path, bytes) in files {
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
        }
    }
}

// Maintenance shares the captured-tree retirement planner and unlink executor.
pub(crate) fn maintenance_rows(store: &Store) -> Result<JsonValue, NativeError> {
    shards::maintenance_rows(store)
}
pub(crate) fn maintenance_retire(store: &Store, doc: &JsonValue) -> Result<JsonValue, NativeError> {
    shards::maintenance_retire(store, doc)
}
pub(crate) fn maintenance_verify(store: &Store, checkpoint: &JsonValue) -> bool {
    let Ok(doc)=store.document() else {return false};
    let Some(game)=doc["gamePath"].as_str().filter(|s|!s.is_empty()) else {return checkpoint.is_null()};
    shards::check_checkpoint(&Path::new(game).join("game/citadel"),store.root(),checkpoint,false, None).is_ok()
}

pub fn verify_applied(root: &Path, doc: &JsonValue) -> Result<(), NativeError> { check_applied(root,doc, None) }

fn check_generation(c:&Path,root:&Path,a:&JsonValue,fast:bool,authority:Option<&GameLock>)->Result<(),NativeError>{shards::check_checkpoint(c,root,a,fast,authority)}

fn check_generation_full(c:&Path,root:&Path,a:&JsonValue,authority:Option<&GameLock>)->Result<(),NativeError>{shards::check_checkpoint(c,root,a,false,authority)}

fn verify_fonts(store:&Store,doc:&JsonValue,root:&RootLock,game:Option<&GameLock>)->Result<(),NativeError>{
    match game{Some(game)=>crate::fonts::verify_current_locked(store,doc,root,game),None=>crate::fonts::verify_current(store,doc)}
}
