//! Explicit schema-33 import. Upstream metadata and payloads are read-only.
use crate::{
    native::NativeError,
    package::{self, FileRef, PackageRef, SourceAssociation},
    paths::{self, Fingerprint},
    snapshot,
    store::{RootLock, Store},
    vpk,
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const MAX_MANIFEST: u64 = 8 * 1024 * 1024;
#[derive(Clone)]
pub struct PayloadSource {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub id: String,
}
#[derive(Clone)]
pub struct ImportPlan {
    pub root: PathBuf,
    pub source: PathBuf,
    pub game: Option<PathBuf>,
    pub document: JsonValue,
    pub payloads: BTreeMap<String, PayloadSource>,
    pub evidence: Vec<String>,
    pub summary: JsonValue,
}
fn invalid() -> NativeError {
    NativeError::new(
        "invalidState",
        "The source library schema is malformed; choose an intact version 33 state.json.",
    )
}
fn manifest_error() -> NativeError {
    NativeError::new("invalidManifest","A .dmm.json manifest is malformed; repair it with the upstream manager before reviewing import.")
}
fn json_file(path: &Path, limit: u64) -> Result<(JsonValue, Fingerprint), NativeError> {
    let mut f = paths::open_regular(path)?;
    if f.metadata()?.len() > limit {
        return Err(NativeError::new(
            "sourceTooLarge",
            "Import metadata exceeds the supported size limit.",
        ));
    }
    let before = paths::fingerprint_file(&mut f)?;
    f.seek(SeekFrom::Start(0))?;
    let mut b = Vec::new();
    (&mut f).take(limit + 1).read_to_end(&mut b)?;
    if b.len() as u64 > limit {
        return Err(invalid());
    }
    let after = paths::fingerprint_file(&mut f)?;
    if before != after {
        return Err(NativeError::new(
            "stalePreview",
            "Source changed while reading; review again.",
        ));
    }
    let value =
        json::parse(std::str::from_utf8(&b).map_err(|_| invalid())?).map_err(|_| invalid())?;
    Ok((value, after))
}
fn marker(path: &Path) -> String {
    format!(
        "{}:{:?}",
        path.display(),
        fs::symlink_metadata(path)
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                (
                    m.len(),
                    m.mtime(),
                    m.mtime_nsec(),
                    m.dev(),
                    m.ino(),
                    m.mode(),
                )
            })
            .ok()
    )
}
fn fingerprint_marker(path: &Path, fp: &Fingerprint) -> String {
    format!("{}:{fp:?}", path.display())
}
fn list(v: &JsonValue) -> Option<Vec<&str>> {
    if !v.is_array() {
        return None;
    }
    v.members().map(|x| x.as_str()).collect()
}
pub fn validated_associations(
    entry: &JsonValue,
    remote: &str,
) -> Result<(bool, u64, Vec<(String, String)>), String> {
    let unavailable = || {
        "Manifest filename associations are incomplete or unsafe; repair the mod with the upstream manager.".to_owned()
    };
    let enabled = entry["enabled"].as_bool().ok_or_else(unavailable)?;
    let shard = entry["shard"]
        .as_u64()
        .filter(|n| (1..=10).contains(n))
        .ok_or_else(unavailable)?;
    let current = list(&entry["currentVpks"]).ok_or_else(unavailable)?;
    let disabled = list(&entry["disabledVpks"]).ok_or_else(unavailable)?;
    let original = list(&entry["originalVpkNames"]).ok_or_else(unavailable)?;
    if original.is_empty()
        || original.len() > 100_000
        || original
            .iter()
            .any(|s| !paths::safe_name(s) || !s.to_ascii_lowercase().ends_with(".vpk"))
        || !paths::safe_name(remote)
    {
        return Err(unavailable());
    }
    let selected = if enabled {
        if !disabled.is_empty() {
            return Err(unavailable());
        }
        &current
    } else {
        if !current.is_empty() {
            return Err(unavailable());
        }
        &disabled
    };
    if selected.len() != original.len() {
        return Err(unavailable());
    }
    let mut originals = HashSet::new();
    if original
        .iter()
        .any(|n| !originals.insert(n.to_ascii_lowercase()))
    {
        return Err(unavailable());
    }
    let mut seen = HashSet::new();
    for (i, name) in selected.iter().enumerate() {
        if !paths::safe_name(name)
            || !name.to_ascii_lowercase().ends_with(".vpk")
            || !seen.insert(name.to_ascii_lowercase())
            || (!enabled && *name != format!("{remote}_{}", original[i]))
        {
            return Err(unavailable());
        }
    }
    Ok((
        enabled,
        shard,
        original
            .iter()
            .zip(selected)
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
    ))
}
pub(crate) fn validated_names<'a>(
    entry: &'a JsonValue,
    remote: &str,
) -> Result<(bool, u64, Vec<&'a str>), String> {
    let (enabled, shard, _) = validated_associations(entry, remote)?;
    Ok((
        enabled,
        shard,
        list(
            &entry[if enabled {
                "currentVpks"
            } else {
                "disabledVpks"
            }],
        )
        .unwrap(),
    ))
}
fn fingerprint_json(path: &Path, fp: &Fingerprint) -> JsonValue {
    object! {sourcePath:path.to_string_lossy().as_ref(),state:"present",sha256:fp.sha256.clone(),size:fp.size,device:fp.device,inode:fp.inode,modifiedSeconds:fp.modified_seconds,modifiedNanos:fp.modified_nanos}
}
/// Capture conservative engine lookup proof independently of logical importability.
fn capture_engine_evidence(
    associations: &[SourceAssociation],
    sources: &mut BTreeMap<PathBuf, JsonValue>,
    evidence: &mut Vec<String>,
) -> Result<(), NativeError> {
    let names: HashMap<_, _> = associations
        .iter()
        .enumerate()
        .map(|(i, a)| (a.original_name.as_str(), i))
        .collect();
    let mut claimed = HashSet::new();
    for a in associations {
        if let Ok(d) = vpk::inspect_directory(&a.source_path) {
            let stem = &a.original_name[..a.original_name.len() - 4];
            let stem = stem.strip_suffix("_dir").unwrap_or(stem);
            for index in vpk::required_archives(&d) {
                if let Some(i) = names.get(format!("{stem}_{index:03}.vpk").as_str()) {
                    claimed.insert(*i);
                }
            }
        }
    }
    let mut total_records = 0usize;
    for (i, a) in associations.iter().enumerate() {
        let mut pending = vec![(a.source_path.clone(), claimed.contains(&i))];
        let mut seen = HashSet::new();
        let mut files = JsonValue::new_array();
        let mut complete = true;
        let mut engine_complete = true;
        while let Some((path, member)) = pending.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            total_records += 1;
            if total_records > 100_000 {
                return Err(invalid());
            }
            evidence.push(marker(&path));
            let mut record = match paths::fingerprint(&path) {
                Ok(fp) => {
                    evidence.push(fingerprint_marker(&path, &fp));
                    fingerprint_json(&path, &fp)
                }
                Err(_) => {
                    if paths::verify_absent(&path).is_ok() {
                        object! {sourcePath:path.to_string_lossy().as_ref(),state:"missing"}
                    } else {
                        object! {sourcePath:path.to_string_lossy().as_ref(),state:"unverifiable"}
                    }
                }
            };
            record["proofVersion"] = 2.into();
            record["member"] = member.into();
            record["directory"] = false.into();
            if record["state"] == "present" {
                record["identity"] = object! {size:record["size"].clone(),device:record["device"].clone(),inode:record["inode"].clone(),modifiedSeconds:record["modifiedSeconds"].clone(),modifiedNanos:record["modifiedNanos"].clone()};
                match vpk::inspect_directory(&path) {
                    Ok(d) => {
                        record["directory"] = true.into();
                        let stem = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .ok_or_else(invalid)?;
                        let stem = stem.strip_suffix("_dir").unwrap_or(stem);
                        let mut deps = JsonValue::new_array();
                        for index in vpk::required_archives(&d) {
                            let dep = path.with_file_name(format!("{stem}_{index:03}.vpk"));
                            deps.push(dep.to_string_lossy().as_ref())
                                .map_err(|_| invalid())?;
                            pending.push((dep, true));
                        }
                        record["dependencies"] = deps;
                    }
                    Err(_) => {
                        let mut header = [0u8; 4];
                        let has_header = paths::open_regular(&path)
                            .and_then(|mut f| {
                                f.read_exact(&mut header)?;
                                Ok(())
                            })
                            .is_ok()
                            && header == 0x55aa1234u32.to_le_bytes();
                        if !member || has_header {
                            // Mount replacement preserves this file; Restore needs its
                            // exact bytes, not a successful VPK interpretation.
                            record["proofKind"] = "bytes".into();
                            engine_complete = false;
                        }
                    }
                }
                let after = paths::fingerprint(&path)?;
                if record["sha256"] != after.sha256
                    || record["size"] != after.size
                    || record["device"] != after.device
                    || record["inode"] != after.inode
                    || record["modifiedSeconds"] != after.modified_seconds
                    || record["modifiedNanos"] != after.modified_nanos
                {
                    return Err(NativeError::new(
                        "stalePreview",
                        "A prior engine file changed while reviewing.",
                    ));
                }
            }
            if record["state"] == "unverifiable" {
                complete = false;
            }
            files.push(record).map_err(|_| invalid())?;
        }
        if let Some(p) = sources.get_mut(&a.source_path) {
            let first = &files[0];
            if p["state"] == "present" {
                for key in [
                    "sha256",
                    "size",
                    "device",
                    "inode",
                    "modifiedSeconds",
                    "modifiedNanos",
                ] {
                    if p[key] != first[key] {
                        return Err(NativeError::new(
                            "stalePreview",
                            "A prior source changed during directory inspection.",
                        ));
                    }
                }
            }
            p["engineFiles"] = files;
            p["engineComplete"] = (complete && engine_complete).into();
            p["priorEvidenceComplete"] = complete.into();
        }
    }
    Ok(())
}
impl ImportPlan {
    pub fn review(
        root: &Path,
        source: &Path,
        game_override: Option<&Path>,
    ) -> Result<Self, NativeError> {
        let root = paths::absolute(root)?;
        let source = paths::absolute(source)?;
        let (raw, source_fp) = json_file(&source, paths::MAX_JSON_BYTES)?;
        let config = if let Some(s) = raw["local-config"].as_str() {
            json::parse(s).map_err(|_| invalid())?
        } else {
            raw["local-config"].clone()
        };
        if !config.is_object() || !config["version"].is_number() {
            return Err(invalid());
        }
        if config["version"] != 33 {
            return Err(NativeError::new(
                "unsupportedVersion",
                "Only upstream source schema 33 is supported; no files were copied.",
            ));
        }
        let state = &config["state"];
        if !state.is_object() || !state["profiles"].is_object() || state["profiles"].len() > 1000 {
            return Err(invalid());
        }
        let game = game_override
            .or_else(|| {
                state["gamePath"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(Path::new)
            })
            .map(paths::lexical_absolute)
            .transpose()?;
        // Archive library objects only. Never copy root/global account, session or integrations.
        let mut archive = object! {profiles:state["profiles"].clone()};
        for key in ["localMods", "groups", "activeProfileId"] {
            if state.has_key(key) {
                archive[key] = state[key].clone()
            }
        }
        let mut evidence = vec![fingerprint_marker(&source, &source_fp)];
        let mut profiles = JsonValue::new_object();
        let mut payloads = BTreeMap::<String, PayloadSource>::new();
        let mut source_payloads = BTreeMap::<PathBuf, JsonValue>::new();
        let mut manifests = HashMap::<PathBuf, Option<JsonValue>>::new();
        let mut manifest_bytes = 0u64;
        let active = state["activeProfileId"].as_str();
        let mut mod_count = 0usize;
        let mut available_count = 0usize;
        let mut associations_ledger = JsonValue::new_array();
        let mut ids: Vec<_> = state["profiles"].entries().map(|(id, _)| id).collect();
        ids.sort_unstable();
        for id in ids {
            let profile = &state["profiles"][id];
            if id.trim().is_empty() || !profile.is_object() {
                return Err(invalid());
            }
            let source_mods = if Some(id) == active && state.has_key("localMods") {
                if !state["localMods"].is_array() {
                    return Err(invalid());
                }
                &state["localMods"]
            } else {
                &profile["mods"]
            };
            if !source_mods.is_array() {
                return Err(invalid());
            }
            let folder = match &profile["folderName"] {
                JsonValue::Null => Some(""),
                value => value
                    .as_str()
                    .filter(|s| s.is_empty() || paths::safe_name(s)),
            };
            let base = match (&game, folder) {
                (Some(g), Some(f)) => Some(g.join("game/citadel/addons").join(f)),
                _ => None,
            };
            let manifest = if let Some(base) = &base {
                let path = base.join(".dmm.json");
                if !manifests.contains_key(&path) {
                    if path.try_exists().unwrap_or(false) && paths::reject_symlinks(&path).is_ok() {
                        let (m, fp) = json_file(&path, MAX_MANIFEST).map_err(|e| {
                            if e.kind == "unsafePath" {
                                e
                            } else {
                                manifest_error()
                            }
                        })?;
                        manifest_bytes = manifest_bytes
                            .checked_add(fp.size)
                            .ok_or_else(manifest_error)?;
                        if manifest_bytes > paths::MAX_JSON_BYTES {
                            return Err(manifest_error());
                        }
                        if !m.is_object() || !m["version"].is_number() {
                            return Err(manifest_error());
                        }
                        if m["version"] != 3 {
                            return Err(NativeError::new(
                                "unsupportedVersion",
                                "Only manifest schema 3 is supported; no files were copied.",
                            ));
                        }
                        if !m["mods"].is_object() {
                            return Err(manifest_error());
                        }
                        evidence.push(fingerprint_marker(&path, &fp));
                        manifests.insert(path.clone(), Some(m));
                    } else {
                        evidence.push(marker(&path));
                        manifests.insert(path.clone(), None);
                    }
                }
                manifests.get(&path).and_then(Option::as_ref)
            } else {
                None
            };
            // Resolve ownership globally within the profile manifest, including entries not
            // present in saved profile data. Duplicate file ownership is never guessed.
            let mut owners = HashMap::<PathBuf, usize>::new();
            let mut folded_owners = HashMap::<String, usize>::new();
            if let (Some(manifest), Some(base)) = (manifest, &base) {
                for (_, e) in manifest["mods"].entries() {
                    let sh = e["shard"].as_u64().unwrap_or(1);
                    for key in ["currentVpks", "disabledVpks"] {
                        if let Some(names) = list(&e[key]) {
                            for n in names {
                                if paths::safe_name(n) {
                                    let directory = if key == "currentVpks" && sh > 1 {
                                        game.as_ref()
                                            .unwrap()
                                            .join(format!("game/citadel/addons{sh}"))
                                            .join(folder.unwrap())
                                    } else {
                                        base.clone()
                                    };
                                    *owners.entry(directory.join(n)).or_default() += 1;
                                    *folded_owners
                                        .entry(
                                            directory
                                                .join(n)
                                                .to_string_lossy()
                                                .to_ascii_lowercase(),
                                        )
                                        .or_default() += 1;
                                }
                            }
                        }
                    }
                }
            }
            // Prior mounted state is independent of whether a VPK can be imported.
            // Include every manifest path, including entries absent from the library,
            // missing packages and valid companions of unsupported packages.
            let mut owned_paths: Vec<_> = owners.keys().collect();
            owned_paths.sort();
            for path in owned_paths {
                evidence.push(marker(path));
                let state = match paths::fingerprint(path) {
                    Ok(fp) => {
                        evidence.push(fingerprint_marker(path, &fp));
                        fingerprint_json(path, &fp)
                    }
                    Err(_) => {
                        if paths::verify_absent(path).is_ok() {
                            object! {sourcePath:path.to_string_lossy().as_ref(),state:"missing"}
                        } else {
                            object! {sourcePath:path.to_string_lossy().as_ref(),state:"unverifiable"}
                        }
                    }
                };
                source_payloads.insert(path.clone(), state);
            }
            if let (Some(manifest), Some(base)) = (manifest, &base) {
                for (remote, entry) in manifest["mods"].entries() {
                    if let Ok((enabled, shard, pairs)) = validated_associations(entry, remote) {
                        let dir = if enabled && shard > 1 {
                            game.as_ref()
                                .unwrap()
                                .join(format!("game/citadel/addons{shard}"))
                                .join(folder.unwrap())
                        } else {
                            base.clone()
                        };
                        let a: Vec<_> = pairs
                            .into_iter()
                            .map(|(original, current)| SourceAssociation {
                                original_name: original,
                                source_path: dir.join(current),
                            })
                            .collect();
                        capture_engine_evidence(&a, &mut source_payloads, &mut evidence)?;
                    }
                }
            }
            let mut mods = JsonValue::new_array();
            let mut mod_ids = HashSet::new();
            for (index, raw_mod) in source_mods.members().enumerate() {
                mod_count += 1;
                if mod_count > 100_000 {
                    return Err(invalid());
                }
                let mut raw_mod = raw_mod.clone();
                let valid = raw_mod.is_object()
                    && ["id", "remoteId", "status"]
                        .iter()
                        .all(|key| snapshot::text(&raw_mod[*key]).is_some());
                let mut reason = if !valid {
                    Some("Unrecognized mod metadata; restore its stable ID, remote ID and status in the upstream manager.".to_owned())
                } else {
                    None
                };
                if !valid {
                    let original = raw_mod;
                    raw_mod = object! {id:format!("unrecognized-{index}"),remoteId:format!("unrecognized-{index}"),name:"Unrecognized library entry",status:"unknown",original:original}
                }
                let mod_id = raw_mod["id"].as_str().ok_or_else(invalid)?.to_owned();
                if !mod_ids.insert(mod_id) {
                    return Err(invalid());
                }
                let remote = raw_mod["remoteId"].as_str().ok_or_else(invalid)?;
                let desired = profile["enabledMods"][remote]["enabled"]
                    .as_bool()
                    .unwrap_or(raw_mod["status"] == "installed");
                let hidden = snapshot::preview_hidden(&raw_mod, remote, state);
                let mut packages = Vec::<PackageRef>::new();
                if reason.is_none() {
                    let resolved = (|| -> Result<Vec<SourceAssociation>, String> {
                        let base = base.as_ref().ok_or_else(|| {
                            if folder.is_none() {
                                "Profile folderName is unsafe; repair it in the upstream manager."
                                    .to_owned()
                            } else {
                                "Choose the game installation path to locate recognized VPKs."
                                    .to_owned()
                            }
                        })?;
                        let m=manifest.ok_or_else(||"No .dmm.json ownership manifest exists for this profile; repair with the upstream manager.".to_owned())?;
                        let entry = &m["mods"][remote];
                        if !entry.is_object() {
                            return Err(
                                "No exact remote-ID ownership exists in this profile manifest."
                                    .to_owned(),
                            );
                        }
                        let (enabled, shard, names) = validated_associations(entry, remote)?;
                        let dir = if enabled && shard > 1 {
                            game.as_ref()
                                .unwrap()
                                .join(format!("game/citadel/addons{shard}"))
                                .join(folder.unwrap())
                        } else {
                            base.clone()
                        };
                        let mut paths = Vec::new();
                        for (original, n) in names {
                            let p = dir.join(&n);
                            associations_ledger.push(object!{profileId:id,remoteId:remote,originalName:original.clone(),currentName:n,sourcePath:p.to_string_lossy().as_ref(),enabled:enabled,shard:shard}).map_err(|_|"Invalid association ledger".to_owned())?;
                            if owners.get(&p) != Some(&1)
                                || folded_owners.get(&p.to_string_lossy().to_ascii_lowercase())
                                    != Some(&1)
                            {
                                return Err("Multiple manifest entries claim the same VPK; repair ownership before importing.".to_owned());
                            }
                            paths.push(SourceAssociation {
                                original_name: original,
                                source_path: p,
                            })
                        }
                        Ok(paths)
                    })();
                    match resolved {
                        Err(e) => reason = Some(e),
                        Ok(files) => {
                            let verified = (|| -> Result<Vec<PackageRef>, NativeError> {
                                let mut fingerprints = BTreeMap::new();
                                for a in &files {
                                    let fp = paths::fingerprint(&a.source_path)?;
                                    evidence.push(fingerprint_marker(&a.source_path, &fp));
                                    fingerprints.insert(a.source_path.clone(), fp);
                                }
                                let families = package::resolve_source_packages(&files)?;
                                for a in &files {
                                    if paths::fingerprint(&a.source_path)?
                                        != fingerprints[&a.source_path]
                                    {
                                        return Err(NativeError::new(
                                            "stalePreview",
                                            "A family member changed while reviewing.",
                                        ));
                                    }
                                }
                                let mut reference =
                                    |a: &SourceAssociation| -> Result<FileRef, NativeError> {
                                        let fp = &fingerprints[&a.source_path];
                                        let key = format!("{}:{}", fp.sha256, fp.size);
                                        if !payloads.contains_key(&key) {
                                            payloads.insert(
                                                key.clone(),
                                                PayloadSource {
                                                    path: a.source_path.clone(),
                                                    fingerprint: fp.clone(),
                                                    id: paths::random_id()?,
                                                },
                                            );
                                        }
                                        Ok(FileRef {
                                            id: payloads[&key].id.clone(),
                                            sha256: fp.sha256.clone(),
                                            size: fp.size,
                                            source_name: a.original_name.clone(),
                                        })
                                    };
                                families
                                    .iter()
                                    .map(|family| {
                                        Ok(PackageRef {
                                            directory: reference(&family.directory)?,
                                            companions: family
                                                .companions
                                                .iter()
                                                .map(|(i, a)| Ok((*i, reference(a)?)))
                                                .collect::<Result<_, NativeError>>()?,
                                        })
                                    })
                                    .collect()
                            })();
                            match verified {
                                Ok(p) => packages = p,
                                Err(e) if e.message == vpk::DUPLICATE_ENTRY_MESSAGE => reason = Some(e.message),
                                Err(_) => reason = Some("This mod’s files are missing, incomplete or can’t be read safely, so it can’t be imported automatically. Add it again from Browse.".to_owned()),
                            }
                        }
                    }
                }
                let available = reason.is_none();
                if available {
                    available_count += 1
                }
                mods.push(object!{source:raw_mod,enabled:desired,available:available,unavailableReason:reason,previewHidden:hidden,packages:package::encode_packages(&packages)}).map_err(|_|invalid())?;
            }
            profiles[id] = object! {id:id,name:profile["name"].as_str().filter(|s|!s.trim().is_empty()).unwrap_or(id),source:profile.clone(),mods:mods,protected:id=="default"};
        }
        if !profiles.has_key("default") {
            profiles["default"] = object! {id:"default",name:"Default",source:object!{},mods:json::array![],protected:true}
        }
        let active = active
            .filter(|id| profiles.has_key(id))
            .unwrap_or("default");
        let game_string = game
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let manifest_ledger = JsonValue::from(
            manifests
                .iter()
                .filter_map(|(path, value)| {
                    value
                        .as_ref()
                        .map(|m| object! {path:path.to_string_lossy().as_ref(),body:m.clone()})
                })
                .collect::<Vec<_>>(),
        );
        let mut reachable = HashSet::new();
        let mut companions = HashSet::new();
        let mut families = HashSet::new();
        let mut package_count = 0u64;
        for (_, profile) in profiles.entries() {
            for m in profile["mods"].members() {
                for p in crate::options::retained_packages(m, 2)? {
                    package_count = package_count.checked_add(1).ok_or_else(invalid)?;
                    families.insert(package::family_key(&p)?);
                    reachable.insert(p.directory.id);
                    for c in p.companions.values() {
                        reachable.insert(c.id.clone());
                        companions.insert(c.id.clone());
                    }
                }
            }
        }
        payloads.retain(|_, p| reachable.contains(&p.id));
        let total_bytes = payloads.values().try_fold(0u64, |n, p| {
            n.checked_add(p.fingerprint.size).ok_or_else(invalid)
        })?;
        let ledger = object! {upstreamEvidenceVersion:2,packageEvidenceVersion:2,associations:associations_ledger,sourcePath:source.to_string_lossy().as_ref(),sourceSha256:source_fp.sha256.clone(),sourceSize:source_fp.size,manifests:manifest_ledger,sourcePayloads:JsonValue::from(source_payloads.into_values().collect::<Vec<_>>()),payloads:JsonValue::from(payloads.values().map(|p|object!{id:p.id.clone(),sourcePath:p.path.to_string_lossy().as_ref(),sha256:p.fingerprint.sha256.clone(),size:p.fingerprint.size}).collect::<Vec<_>>())};
        let document = object! {storeVersion:2,revision:"0",sourcePath:source.to_string_lossy().as_ref(),gamePath:game_string.clone(),activeProfileId:active,profiles:profiles,importArchive:archive,importLedger:ledger,applied:JsonValue::Null,canRestore:false,recoveryRequired:false};
        let mut warnings = JsonValue::new_array();
        if available_count < mod_count {
            warnings.push(object!{code:"unavailablePayloads",message:"Some imported entries have unavailable payloads; inspect their reasons before applying."}).map_err(|_|invalid())?
        }
        let mut profile_summaries = JsonValue::new_array();
        let mut unavailable = JsonValue::new_array();
        let truncated = std::cell::Cell::new(false);
        let bounded = |s: &str| {
            if s.chars().nth(1024).is_some() {
                truncated.set(true);
            }
            s.chars().take(1024).collect::<String>()
        };
        for (id, profile) in document["profiles"].entries() {
            let available = profile["mods"]
                .members()
                .filter(|m| m["available"] == true)
                .count();
            if profile_summaries.len() < 256 {
                profile_summaries.push(object!{id:bounded(id),name:bounded(profile["name"].as_str().unwrap_or(id)),modCount:profile["mods"].len(),availableModCount:available,unavailableModCount:profile["mods"].len()-available}).map_err(|_|invalid())?;
            } else {
                truncated.set(true);
            }
            for m in profile["mods"].members().filter(|m| m["available"] != true) {
                if unavailable.len() < 512 {
                    unavailable.push(object!{profileId:bounded(id),modId:bounded(m["source"]["id"].as_str().unwrap_or("")),title:bounded(m["source"]["name"].as_str().unwrap_or("Unnamed entry")),reason:bounded(m["unavailableReason"].as_str().unwrap_or("Unavailable payload"))}).map_err(|_|invalid())?;
                } else {
                    truncated.set(true);
                }
            }
        }
        let summary = object! {packageCount:package_count,directoryCount:families.len(),physicalFileCount:payloads.len(),companionFileCount:companions.len(),profileCount:document["profiles"].len(),modCount:mod_count,availableModCount:available_count,unavailableModCount:mod_count-available_count,totalBytes:total_bytes,warnings:warnings,gamePath:game_string,profiles:profile_summaries,unavailableMods:unavailable,summariesTruncated:truncated.get()};
        Ok(Self {
            root,
            source,
            game,
            document,
            payloads,
            evidence,
            summary,
        })
    }
    /// Validate review again before and after copying. IDs stay from the private
    /// preview plan; re-review creates fresh IDs and is compared by evidence only.
    pub fn commit(
        self,
        lock: &RootLock,
        operation_id: &str,
        input: &JsonValue,
    ) -> Result<JsonValue, NativeError> {
        self.commit_with(lock, operation_id, input, &mut |_| Ok(()))
    }
    /// Narrow deterministic fault seam, also used by copy/publication tests.
    pub fn commit_with(
        self,
        lock: &RootLock,
        operation_id: &str,
        input: &JsonValue,
        checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>,
    ) -> Result<JsonValue, NativeError> {
        if lock.root() != self.root {
            return Err(NativeError::params());
        }
        match Store::open_history(&self.root) {
            Ok(history) => {
                if let Some(result)=history.replay(operation_id,input)? {return Ok(result);}
                match history.document() {
                    Ok(doc)=>package::require_legacy_writer(&doc)?,
                    Err(e) if e.kind=="nativeNotImported"=>{},
                    Err(e)=>return Err(e),
                }
            }
            Err(e) if e.kind=="nativeNotImported"=>{},
            Err(e)=>return Err(e),
        }
        let check = || -> Result<(), NativeError> {
            let fresh =
                Self::review(&self.root, &self.source, self.game.as_deref()).map_err(|_| {
                    NativeError::new(
                        "stalePreview",
                        "Import source changed since review; preview it again.",
                    )
                })?;
            if fresh.evidence != self.evidence {
                return Err(NativeError::new(
                    "stalePreview",
                    "Import source or payloads changed since review; preview again.",
                ));
            }
            Ok(())
        };
        check()?;
        crate::store::ensure_readable(&self.document)?;
        let store = Store::open_writer(lock)?;
        // Fresh import has no game transaction dispatcher. Unknown or game
        // journals must not be bypassed, even if other import journals are valid.
        if store
            .journals()?
            .members()
            .any(|j| j["body"]["kind"] != "import")
        {
            return Err(NativeError::new("recoveryRequired", "Fresh import cannot bypass an unfinished non-import transaction; preserve its journal."));
        }
        reconcile_imports(&store, lock)?;
        let payload_root = crate::storage::path(&self.root, "payloads")?;
        let stage = crate::storage::path(&self.root, format!(".import-family-{}", paths::random_id()?))?;
        let mut packages = Vec::new();
        for (_, profile) in self.document["profiles"].entries() {
            for m in profile["mods"].members() {
                packages.extend(crate::options::retained_packages(m, 2)?);
            }
        }
        let owner = object! {version:2,kind:"import",operationId:operation_id,stage:stage.file_name().unwrap().to_str().unwrap(),nonce:paths::random_id()?,storageFsid:paths::persisted_filesystem_id(&fs::File::open(crate::storage::root(store.root())?)?)?};
        let mut journal = object! {kind:"import",version:2,stage:stage.file_name().unwrap().to_str().unwrap(),input:input.clone(),owner:owner.clone(),stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,packages:package::encode_packages(&packages),payloads:JsonValue::from(self.payloads.values().map(|p|object!{id:p.id.clone(),sha256:p.fingerprint.sha256.clone(),size:p.fingerprint.size,sourceName:p.path.file_name().unwrap().to_string_lossy().as_ref(),device:JsonValue::Null,inode:JsonValue::Null,complete:false}).collect::<Vec<_>>())};
        let mut admission = journal.clone();
        for k in ["stageDevice", "stageInode", "ownerDevice", "ownerInode"] {
            admission[k] = u64::MAX.into();
        }
        for f in admission["payloads"].members_mut() {
            f["device"] = u64::MAX.into();
            f["inode"] = u64::MAX.into();
        }
        crate::store::ensure_internal(&admission)?;
        paths::private_dir(&payload_root)?;
        store.write_journal(lock, operation_id, &journal)?;
        let result = (|| {
            fs::create_dir(&stage)?;
            paths::sync_dir(&crate::storage::root(&self.root)?)?;
            let m = fs::metadata(&stage)?;
            journal["stageDevice"] = m.dev().into();
            journal["stageInode"] = m.ino().into();
            store.write_journal(lock, operation_id, &journal)?;
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(stage.join("owner.json"))?;
            marker.write_all(owner.dump().as_bytes())?;
            marker.sync_all()?;
            paths::sync_dir(&stage)?;
            let m = marker.metadata()?;
            journal["ownerDevice"] = m.dev().into();
            journal["ownerInode"] = m.ino().into();
            store.write_journal(lock, operation_id, &journal)?;
            for (index, payload) in self.payloads.values().enumerate() {
                let mut source = paths::open_regular(&payload.path)?;
                if paths::fingerprint_file(&mut source)? != payload.fingerprint {
                    return Err(NativeError::new(
                        "stalePreview",
                        "A family member changed since review.",
                    ));
                }
                source.seek(SeekFrom::Start(0))?;
                let dest = stage.join(format!("{}.vpk", payload.id));
                let mut out = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&dest)?;
                out.sync_all()?;
                paths::sync_dir(&stage)?;
                checkpoint("createdUnrecorded")?;
                let m = out.metadata()?;
                journal["payloads"][index]["device"] = m.dev().into();
                journal["payloads"][index]["inode"] = m.ino().into();
                store.write_journal(lock, operation_id, &journal)?;
                checkpoint("created")?;
                std::io::copy(&mut source, &mut out)?;
                out.sync_all()?;
                let fp = paths::fingerprint_file(&mut out)?;
                if fp.sha256 != payload.fingerprint.sha256
                    || fp.size != payload.fingerprint.size
                    || paths::fingerprint_file(&mut source)? != payload.fingerprint
                {
                    return Err(NativeError::new(
                        "stalePreview",
                        "Copied family member differs from review.",
                    ));
                }
                journal["payloads"][index]["complete"] = true.into();
                store.write_journal(lock, operation_id, &journal)?;
                checkpoint("memberCopied")?;
            }
            for p in journal["payloads"].members() {
                verify_complete_copy(
                    &stage.join(format!(
                        "{}.vpk",
                        p["id"].as_str().ok_or_else(NativeError::params)?
                    )),
                    p,
                )?;
            }
            for p in &packages {
                let directory =
                    vpk::inspect_directory(&stage.join(format!("{}.vpk", p.directory.id)))?;
                let members = p
                    .companions
                    .iter()
                    .map(|(i, f)| (*i, stage.join(format!("{}.vpk", f.id))))
                    .collect();
                vpk::validate_family(&directory, &members)?;
            }
            for p in self.payloads.values() {
                paths::make_readonly(&stage.join(format!("{}.vpk", p.id)))?;
            }
            check()?;
            paths::sync_dir(&stage)?;
            for p in self.payloads.values() {
                let name = format!("{}.vpk", p.id);
                fs::hard_link(stage.join(&name), payload_root.join(&name))?;
                paths::sync_dir(&payload_root)?;
                checkpoint("published")?;
            }
            paths::sync_dir(&crate::storage::root(&self.root)?)?;
            for p in journal["payloads"].members() {
                verify_complete_copy(
                    &store.payload_path(p["id"].as_str().ok_or_else(NativeError::params)?)?,
                    p,
                )?;
            }
            match store.document() {
                Ok(_) => {
                    return Err(NativeError::new(
                        "nativeAlreadyImported",
                        "A native library already exists; it cannot be replaced by import.",
                    ))
                }
                Err(e) if e.kind == "nativeNotImported" => {}
                Err(e) => return Err(e),
            }
            let result = store.commit(lock, 0, operation_id, input, self.document.clone(), None)?;
            checkpoint("database")?;
            Ok(result)
        })();
        // Re-read the durable journal: failed persistence never grants ownership.
        let durable = store
            .journal(operation_id)?
            .ok_or_else(NativeError::params)?;
        let committed = store.operation(operation_id)?["status"] == "succeeded";
        if cleanup_import(&store, lock, &durable, committed).is_err() {
            return Err(NativeError::new(
                "recoveryRequired",
                "Import staging needs explicit recovery; preserve its ownership journal.",
            ));
        }
        store.clear_journal(lock, operation_id)?;
        result
    }
}

fn verify_complete_copy(path: &Path, record: &JsonValue) -> Result<(), NativeError> {
    let fp = paths::fingerprint(path)?;
    if record["complete"] != true
        || record["device"].as_u64() != Some(fp.device)
        || record["inode"].as_u64() != Some(fp.inode)
        || record["sha256"] != fp.sha256
        || record["size"].as_u64() != Some(fp.size)
    {
        return Err(NativeError::new(
            "recoveryRequired",
            "A copied family member changed before publication; preserve its ownership journal.",
        ));
    }
    Ok(())
}

/// Reconcile only paths named by a durable journal, on explicit import retry or
/// recovery. Unknown paths never become native-owned by matching a filename.
pub fn reconcile_imports(store: &Store, lock: &RootLock) -> Result<(), NativeError> {
    preflight_imports(store,lock)?.execute(store, lock)
}
pub(crate) struct ImportRecovery {
    entries: Vec<(String, JsonValue, bool, ImportCleanup)>,
    pub(crate) claims: HashSet<PathBuf>,
}
impl ImportRecovery {
    pub(crate) fn reserve_outcomes(&self) -> Result<(),NativeError> {
        for (id,input,committed,_) in &self.entries {
            if !committed { crate::store::admit_outcome(id,input,&crate::store::failure_outcome(&interrupted_import()))?; }
        }
        Ok(())
    }
    pub(crate) fn execute(self, store: &Store, lock: &RootLock) -> Result<(), NativeError> {
        self.reserve_outcomes()?;
        for (id, input, committed, plan) in self.entries {
            plan.execute(store)?;
            if !committed {
                store.record_failure(lock,&id,&input,&interrupted_import())?;
            }
            store.clear_journal(lock, &id)?;
        }
        Ok(())
    }
}
fn interrupted_import() -> NativeError {
    NativeError::new("operationInterrupted","Interrupted import copies were reconciled; review and retry with a new operation ID.")
}
pub(crate) fn preflight_imports(store: &Store, lock: &RootLock) -> Result<ImportRecovery, NativeError> {
    store.locked(lock)?;
    match store.document() {
        Ok(_)=>{},
        Err(e) if e.kind=="nativeNotImported"=>{},
        Err(e)=>return Err(e),
    }
    let journals=store.journals()?;
    crate::font_transaction::admit_set(store,&journals)?;
    if journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])) {
        return Err(NativeError::new("recoveryRequired","Remote payload recovery owns this journal set."));
    }
    let mut claims = HashSet::new();
    let mut claimed_ids = HashSet::new();
    let mut entries = Vec::new();
    for entry in journals.members() {
        let body = &entry["body"];
        if body["kind"] != "import" {
            continue;
        }
        let id = entry["id"].as_str().ok_or_else(NativeError::params)?;
        if matches!(body["version"].as_u32(),Some(3|4|5|6|7)) && body["operationId"] != id {
            return Err(NativeError::new(
                "recoveryRequired",
                "Local journal operation identity changed.",
            ));
        }
        let committed = store.operation(id)?["status"] == "succeeded";
        let plan = plan_import_cleanup(store, lock, body, committed)?;
        let files = if matches!(body["version"].as_u32(),Some(3|4|5|6|7)) { &body["files"] } else { &body["payloads"] };
        let ids = files.members().map(|f| f["id"].as_str().ok_or_else(NativeError::params))
            .collect::<Result<HashSet<_>, _>>()?;
        for id in ids {
            if !claimed_ids.insert(id.to_owned()) {
                return Err(NativeError::new("recoveryRequired", "Import journals claim the same immutable file ID; preserve their files."));
            }
        }
        for path in &plan.claims {
            if !claims.insert(path.clone()) {
                return Err(NativeError::new(
                    "recoveryRequired",
                    "Import journals contain overlapping ownership; preserve their files.",
                ));
            }
        }
        entries.push((id.to_owned(), body["input"].clone(), committed, plan));
    }
    Ok(ImportRecovery { entries, claims })
}

#[derive(Default)]
pub(crate) struct ImportCleanup {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) owner: Option<PathBuf>,
    pub(crate) stage: Option<PathBuf>,
    pub(crate) sync_dirs: std::collections::BTreeSet<PathBuf>,
    pub(crate) claims: HashSet<PathBuf>,
}
impl ImportCleanup {
    pub(crate) fn execute(self, store: &Store) -> Result<(), NativeError> {
        for path in self.files {
            let m=paths::open_regular(&path)?.metadata()?;
            fs::remove_file(path)?;
            paths::acknowledge_link_change(m.dev(),m.ino())?;
        }
        for parent in self.sync_dirs {
            paths::sync_dir(&parent)?;
        }
        if let Some(owner) = self.owner {
            fs::remove_file(&owner)?;
            paths::sync_dir(owner.parent().ok_or_else(NativeError::params)?)?;
        }
        if let Some(stage) = self.stage {
            fs::remove_dir(stage)?;
            paths::sync_dir(&crate::storage::root(store.root())?)?;
        }
        Ok(())
    }
}
fn cleanup_import(store: &Store, lock: &RootLock, body: &JsonValue, committed: bool) -> Result<(), NativeError> {
    plan_import_cleanup(store, lock, body, committed)?.execute(store)
}
pub(crate) fn plan_import_cleanup(
    store: &Store,
    lock: &RootLock,
    body: &JsonValue,
    committed: bool,
) -> Result<ImportCleanup, NativeError> {
    // Initial import may have no document; a retained newer layout is never
    // within a historical cleanup domain even through a direct caller.
    match store.document() {
        Ok(doc)=>crate::deployment_metadata::domain(&doc,body["version"]==7)?,
        Err(e) if e.kind=="nativeNotImported"=>{},
        Err(e)=>return Err(e),
    }
    for key in ["document","beforeDocument","requestedDocument"] {
        if body["version"]==7 { if body.has_key(key) {crate::deployment_metadata::admit(store.root(),&body[key])?;} }
        else {crate::generation::require_flat_document(&body[key])?;}
    }
    if matches!(body["version"].as_u32(),Some(3|4|5|6|7)) {
        return crate::local_import::plan_local_cleanup(store, lock, body, committed);
    }
    let mut plan = match &body["version"] {
        JsonValue::Null if !body.has_key("version") => {
            plan_legacy_import_cleanup(store, body, committed)
        }
        v if *v == 1 => plan_legacy_import_cleanup(store, body, committed),
        v if *v == 2 => plan_family_import_cleanup(store, lock, body, committed),
        _ => Err(NativeError::new(
            "recoveryRequired",
            "Unsupported import journal version; preserve its files.",
        )),
    }?;
    let stage = crate::storage::path(store.root(), body["stage"].as_str().ok_or_else(NativeError::params)?)?;
    plan.claims
        .extend([stage.clone(), stage.join("owner.json")]);
    for f in body["payloads"].members() {
        let id = f["id"].as_str().ok_or_else(NativeError::params)?;
        let dest = store.payload_path(id)?;
        plan.claims
            .insert(stage.join(dest.file_name().ok_or_else(NativeError::params)?));
        plan.claims.insert(dest);
    }
    Ok(plan)
}

fn plan_legacy_import_cleanup(
    store: &Store,
    body: &JsonValue,
    committed: bool,
) -> Result<ImportCleanup, NativeError> {
    let unsafe_stage = || {
        NativeError::new("recoveryRequired", "Import staging contains unrecognized or changed ownership evidence; preserve it for recovery.")
    };
    let name = body["stage"].as_str().ok_or_else(unsafe_stage)?;
    let suffix = name.strip_prefix(".import-").ok_or_else(unsafe_stage)?;
    if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(unsafe_stage());
    }
    let stage = crate::storage::path(store.root(), name)?;
    if !stage.try_exists()? {
        return Ok(ImportCleanup::default());
    }
    paths::reject_symlinks(&stage)?;
    // An interruption immediately after mkdir leaves only this known empty dir.
    if fs::read_dir(&stage)?.next().is_none() {
        return Ok(ImportCleanup {
            stage: Some(stage),
            ..ImportCleanup::default()
        });
    }
    let bytes = paths::read_bounded(&stage.join("owner.json"), paths::MAX_JSON_BYTES)?;
    if json::parse(std::str::from_utf8(&bytes).map_err(|_| unsafe_stage())?)
        .map_err(|_| unsafe_stage())?
        != *body
    {
        return Err(unsafe_stage());
    }
    let mut allowed = HashSet::new();
    allowed.insert("owner.json".to_owned());
    let mut files = Vec::new();
    for payload in body["payloads"].members() {
        let id = payload["id"].as_str().ok_or_else(unsafe_stage)?;
        let destination = store.payload_path(id)?;
        let name = format!("{id}.vpk");
        allowed.insert(name.clone());
        let source = stage.join(name);
        if source.try_exists()? {
            paths::open_regular(&source)?;
        }
        if destination.try_exists()? {
            paths::open_regular(&destination)?;
            if source.try_exists()? {
                let a = fs::metadata(&source)?;
                let b = fs::metadata(&destination)?;
                if a.dev() != b.dev() || a.ino() != b.ino() {
                    return Err(unsafe_stage());
                }
            } else if committed {
                let fp = paths::fingerprint(&destination)?;
                if payload["sha256"].as_str() != Some(fp.sha256.as_str())
                    || payload["size"].as_u64() != Some(fp.size)
                {
                    return Err(unsafe_stage());
                }
            } else {
                return Err(unsafe_stage());
            }
        }
        if !committed && destination.try_exists()? {
            files.push(destination);
        }
        if source.try_exists()? {
            files.push(source);
        }
    }
    // Complete all ownership checks before deleting even one known file.
    for entry in fs::read_dir(&stage)? {
        let entry = entry?;
        if !allowed.contains(&entry.file_name().to_string_lossy().into_owned()) {
            return Err(unsafe_stage());
        }
    }
    Ok(ImportCleanup {
        files,
        owner: Some(stage.join("owner.json")),
        stage: Some(stage.clone()),
        sync_dirs: [crate::storage::path(store.root(), "payloads")?, stage.clone()]
            .into_iter()
            .filter(|p| p.exists())
            .collect(),
        claims: HashSet::new(),
    })
}

fn plan_family_import_cleanup(
    store: &Store,
    lock: &RootLock,
    body: &JsonValue,
    committed: bool,
) -> Result<ImportCleanup, NativeError> {
    let unsafe_stage = || {
        NativeError::new("recoveryRequired","Family staging has incomplete or changed ownership evidence; preserve it for recovery.")
    };
    let name = body["stage"].as_str().ok_or_else(unsafe_stage)?;
    if !name
        .strip_prefix(".import-family-")
        .is_some_and(|id| package::hex(id, 32))
        || !body["payloads"].is_array()
        || !body["owner"].is_object()
    {
        return Err(unsafe_stage());
    }
    let packages = package::legacy_packages(&object! {packages:body["packages"].clone()}, 2)
        .map_err(|_| unsafe_stage())?;
    let mut expected = BTreeMap::new();
    for p in &packages {
        for f in std::iter::once(&p.directory).chain(p.companions.values()) {
            expected.insert(f.id.clone(), (f.sha256.clone(), f.size));
        }
    }
    let mut declared = HashSet::new();
    for p in body["payloads"].members() {
        let f = package::decode_file(p).map_err(|_| unsafe_stage())?;
        if !declared.insert(f.id.clone())
            || expected.get(&f.id) != Some(&(f.sha256, f.size))
            || !p["complete"].is_boolean()
        {
            return Err(unsafe_stage());
        }
    }
    if declared.len() != expected.len() {
        return Err(unsafe_stage());
    }
    let stage = crate::storage::path(store.root(), name)?;
    let fsid=if body["owner"].has_key("storageFsid") {Some(body["owner"]["storageFsid"].as_str().filter(|s|package::hex(s,16)).ok_or_else(unsafe_stage)?)} else {None};
    let exists = |p: &Path| -> Result<bool, NativeError> {
        match fs::symlink_metadata(p) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                paths::verify_absent(p)?;
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    };
    let identity = |p: &Path, device: &JsonValue, inode: &JsonValue| -> Result<(), NativeError> {
        let f = paths::open_regular(p)?;
        if !paths::same_persisted_object(lock.root(),lock.directory(),p,&f,device.as_u64().ok_or_else(unsafe_stage)?,inode.as_u64().ok_or_else(unsafe_stage)?,fsid)? {
            return Err(unsafe_stage());
        }
        Ok(())
    };
    let stage_exists = exists(&stage)?;
    if stage_exists {
        paths::reject_symlinks(&stage)?;
        let dir=fs::File::open(&stage)?;let m=dir.metadata()?;
        if !m.is_dir()
            || !paths::same_persisted_object(lock.root(),lock.directory(),&stage,&dir,body["stageDevice"].as_u64().ok_or_else(unsafe_stage)?,body["stageInode"].as_u64().ok_or_else(unsafe_stage)?,fsid)?
        {
            return Err(unsafe_stage());
        }
        if exists(&stage.join("owner.json"))? {
            identity(
                &stage.join("owner.json"),
                &body["ownerDevice"],
                &body["ownerInode"],
            )?;
            let bytes = paths::read_bounded(&stage.join("owner.json"), paths::MAX_JSON_BYTES)?;
            if json::parse(std::str::from_utf8(&bytes).map_err(|_| unsafe_stage())?)
                .map_err(|_| unsafe_stage())?
                != body["owner"]
            {
                return Err(unsafe_stage());
            }
        } else if fs::read_dir(&stage)?.next().is_some() {
            return Err(unsafe_stage());
        }
    }
    let mut files = Vec::new();
    let mut allowed = HashSet::from(["owner.json".to_owned()]);
    for p in body["payloads"].members() {
        let id = p["id"].as_str().ok_or_else(unsafe_stage)?;
        let filename = format!("{id}.vpk");
        allowed.insert(filename.clone());
        let src = stage.join(filename);
        let dst = store.payload_path(id)?;
        let src_exists = exists(&src)?;
        let dst_exists = exists(&dst)?;
        for path in [src_exists.then_some(&src), dst_exists.then_some(&dst)]
            .into_iter()
            .flatten()
        {
            identity(path, &p["device"], &p["inode"])?;
            if p["complete"] == true {
                let fp = paths::fingerprint(path)?;
                if p["sha256"] != fp.sha256 || p["size"].as_u64() != Some(fp.size) {
                    return Err(unsafe_stage());
                }
            }
        }
        if dst_exists && p["complete"] != true {
            return Err(unsafe_stage());
        }
        if committed && (!dst_exists || p["complete"] != true) {
            return Err(unsafe_stage());
        }
        if !committed && dst_exists {
            files.push(dst);
        }
        if src_exists {
            files.push(src);
        }
    }
    if stage_exists {
        for entry in fs::read_dir(&stage)? {
            if !allowed.contains(&entry?.file_name().to_string_lossy().into_owned()) {
                return Err(unsafe_stage());
            }
        }
    }
    Ok(ImportCleanup {
        files,
        owner: if stage_exists && exists(&stage.join("owner.json"))? {
            Some(stage.join("owner.json"))
        } else {
            None
        },
        stage: stage_exists.then_some(stage.clone()),
        sync_dirs: [crate::storage::path(store.root(), "payloads")?, stage.clone()]
            .into_iter()
            .filter(|p| p.exists())
            .collect(),
        claims: HashSet::new(),
    })
}
