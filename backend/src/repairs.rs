//! Derived repair artifacts; publication into the game belongs exclusively to Apply.
use crate::{native::NativeError, package, paths, store::Store};
use json::{JsonValue, object};
use std::os::unix::fs::MetadataExt;
use std::{
    fs,
    path::{Path, PathBuf},
};
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub fn choices(doc: &JsonValue, profile: &str) -> Result<JsonValue,NativeError> {
    Ok(object! {automatic:doc["fixAutomatically"].as_bool().unwrap_or(true),mods:JsonValue::from(crate::profile_edit::ordered_mod_ids(&doc["profiles"][profile])?.iter().filter_map(|id|doc["profiles"][profile]["mods"].members().find(|m|m["source"]["id"]==*id)).filter(|m|m["enabled"]==true).map(|m|object!{modId:m["source"]["id"].clone(),enabled:m["fixEnabled"].as_bool().unwrap_or(doc["fixAutomatically"].as_bool().unwrap_or(true))}).collect::<Vec<_>>())})
}
pub fn source(
    store: &Store,
    manifest: &JsonValue,
    file: &package::FileRef,
) -> Result<PathBuf, NativeError> {
    if manifest["repairs"]["file"]["id"] == file.id {
        Ok(crate::storage::path(store.root(), "cache/repairs")?
            .join(format!("{}.vpk", file.sha256)))
    } else {
        store.payload_path(&file.id)
    }
}
pub struct Review {
    pub receipt: JsonValue,
    pub evidence: Vec<(PathBuf, paths::Fingerprint)>,
    pub payload_hashes: std::collections::HashMap<PathBuf, crate::generation::ReviewHash>,
    pub(crate) resources: Vec<PathBuf>,
}
pub fn review(
    store: &Store,
    doc: &JsonValue,
    profile: &str,
    game: &Path,
    progress: &mut dyn FnMut(crate::launch_preparation::PreparationProgress),
) -> Result<Review, NativeError> {
    let choices = choices(doc, profile)?;
    let c = game.join("game/citadel");
    let cache = crate::storage::path(store.root(), "cache/repairs")?;
    let mut inputs = Vec::new();
    let mut evidence = Vec::new();
    let mut payload_hashes = std::collections::HashMap::new();
    let mut enabled = std::collections::BTreeSet::new();
    let ordered=crate::profile_edit::ordered_mod_ids(&doc["profiles"][profile])?;
    for row in ordered.iter().filter_map(|id|doc["profiles"][profile]["mods"].members().find(|m|m["source"]["id"]==*id)).filter(|m|m["enabled"]==true)
    {
        let id = row["source"]["id"]
            .as_str()
            .ok_or_else(NativeError::params)?
            .to_owned();
        if row["fixEnabled"]
            .as_bool()
            .unwrap_or(doc["fixAutomatically"].as_bool().unwrap_or(true))
        {
            enabled.insert(id.clone());
        }
        let mut vpks = Vec::new();
        for p in package::selected_packages_exact(row, package::layout_version(doc)?)?.iter()
        {
            let mut members = Vec::new();
            for (name, f) in std::iter::once(("pak01_dir.vpk".to_owned(), &p.directory)).chain(
                p.companions
                    .iter()
                    .map(|(i, f)| (format!("pak01_{i:03}.vpk"), f)),
            ) {
                let path = crate::health::source_path(store, row, f)?;
                let proof = crate::generation::ReviewHash::capture(&path)?;
                let fp = proof.fingerprint.clone();
                if fp.sha256 != f.sha256 || fp.size != f.size {
                    return Err(NativeError::new("stalePreview", "Mod input changed."));
                }
                evidence.push((path.clone(), fp));
                payload_hashes.insert(path.clone(), proof);
                members.push((name, path));
            }
            vpks.push(members);
        }
        inputs.push((id, vpks));
    }
    let mut stamps = Vec::new();
    for dir in [&c, &c.with_file_name("core")] {
        if dir.is_dir() {
            crate::health::collect_stamps(dir, &mut stamps)?;
        }
    }
    // Capture membership and identities from the same traversal. The Apply
    // recheck still makes a fresh traversal before any game write.
    let mut resources = Vec::new();
    for (path, _) in stamps {
        if path.is_file() && resource_file(&path) {
            evidence.push((path.clone(), resource_fingerprint(&path)?));
            resources.push(path);
        }
    }
    let game_build = crate::health::build(game);
    let manifest = game
        .parent()
        .and_then(Path::parent)
        .map(|p| p.join("appmanifest_1422450.acf"));
    if let Some(p) = manifest.filter(|p| p.exists()) {
        evidence.push((p.clone(), paths::fingerprint(&p)?));
    }
    let key = paths::digest_bytes(
        format!(
            "repair92bebec-v2:{}:{game_build:?}:{evidence:?}",
            choices.dump()
        )
        .as_bytes(),
    )?;
    let receipt_path = cache.join(format!("{key}.json"));
    if let Ok(bytes) = paths::read_bounded(&receipt_path, paths::MAX_JSON_BYTES) {
        if let Ok(r) = json::parse(std::str::from_utf8(&bytes).unwrap_or("")) {
            if r["key"] == key {
                if r["file"].is_null() {
                    return Ok(Review {
                        receipt: r,
                        evidence,
                        payload_hashes,
                        resources,
                    });
                }
                let f = package::decode_file(&r["file"])?;
                let p = source(store, &object! {repairs:r.clone()}, &f)?;
                if crate::generation::verify_file(&p, &f).is_ok() {
                    evidence.push((p.clone(), paths::fingerprint(&p)?));
                    return Ok(Review {
                        receipt: r,
                        evidence,
                        payload_hashes,
                        resources,
                    });
                }
            }
        }
    }
    paths::private_dir(&store.root().join("cache"))?;
    paths::private_dir(&cache)?;
    let scratch = Scratch(cache.join(paths::random_id()?));
    paths::private_dir(&scratch.0)?;
    let mut engine_inputs = Vec::new();
    for (id, packages) in inputs {
        let mut vpks = Vec::new();
        for (i, members) in packages.into_iter().enumerate() {
            let dir = scratch.0.join(format!("{}-{i}", engine_inputs.len()));
            paths::private_dir(&dir)?;
            for (name, path) in members {
                std::os::unix::fs::symlink(path, dir.join(name))?;
            }
            vpks.push(dir.join("pak01_dir.vpk"));
        }
        engine_inputs.push((id, vpks));
    }
    let output = scratch.0.join("overlay.vpk");
    let result = if enabled.is_empty() {
        serde_json::json!({"analysis":{},"result":{"hasOverlay":false}})
    } else {
        health_engine::repair_with_progress(
            &c,
            engine_inputs,
            enabled,
            &output,
            &scratch.0.join("resources"),
            {
                let cell = std::cell::RefCell::new(progress);
                move |checked, total| {
                    (cell.borrow_mut())(crate::launch_preparation::PreparationProgress::Repairing {
                        checked,
                        total,
                    })
                }
            },
        )
        .map_err(|e| NativeError::new("repairBuildFailed", &e.to_string()))?
    };
    let report = json::parse(&result.to_string()).map_err(|_| NativeError::params())?;
    let file = if report["result"]["hasOverlay"] == true {
        let fp = paths::fingerprint(&output)?;
        let f = package::FileRef {
            id: fp.sha256[..32].into(),
            sha256: fp.sha256.clone(),
            size: fp.size,
            source_name: "pak01_dir.vpk".into(),
        };
        let dest = cache.join(format!("{}.vpk", fp.sha256));
        if dest.exists() {
            crate::generation::verify_file(&dest, &f)?;
        } else {
            fs::rename(&output, &dest)?;
            paths::make_readonly(&dest)?;
            paths::sync_dir(&cache)?;
        }
        evidence.push((dest.clone(), paths::fingerprint(&dest)?));
        package::encode_file(&f)
    } else {
        JsonValue::Null
    };
    let r = object! {key:key,gameBuild:game_build,choices:choices,file:file,analysis:report["analysis"].clone(),mods:report["mods"].clone()};
    for (p, fp) in &evidence {
        if (if fp.sha256.is_empty() {
            resource_fingerprint(p)?
        } else {
            paths::fingerprint(p)?
        }) != *fp
        {
            return Err(NativeError::new(
                "stalePreview",
                "Repair inputs changed while building.",
            ));
        }
    }
    let tmp = scratch.0.join("receipt.json");
    fs::write(&tmp, r.dump())?;
    paths::open_regular(&tmp)?.sync_all()?;
    fs::rename(tmp, receipt_path)?;
    paths::sync_dir(&cache)?;
    Ok(Review {
        receipt: r,
        evidence,
        payload_hashes,
        resources,
    })
}

fn resource_fingerprint(p: &Path) -> Result<paths::Fingerprint, NativeError> {
    let m = paths::open_regular(p)?.metadata()?;
    Ok(paths::Fingerprint {
        sha256: String::new(),
        size: m.len(),
        modified_seconds: m.mtime(),
        modified_nanos: m.mtime_nsec(),
        device: m.dev(),
        inode: m.ino(),
    })
}

fn resource_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name != "gameinfo.gi" && !name.to_string_lossy().starts_with(".native-gameinfo-"))
}

pub(crate) fn resource_path(game: &Path, path: &Path) -> bool {
    use std::path::Component;
    let Ok(relative) = path.strip_prefix(game.join("game")) else { return false; };
    let mut parts = relative.components();
    if !matches!(parts.next(), Some(Component::Normal(name)) if name == "citadel" || name == "core") { return false; }
    let rest: Vec<_> = parts.collect();
    let Some((last, parents)) = rest.split_last() else { return false; };
    matches!(last, Component::Normal(_)) && resource_file(path)
        && parents.iter().all(|part| matches!(part, Component::Normal(name) if crate::health::resource_directory(name)))
}

/// Membership proof excludes only transaction-owned output directories/gameinfo.
pub(crate) fn resource_membership(game:&Path)->Result<Vec<PathBuf>,NativeError>{
 let c=game.join("game/citadel");let mut stamps=Vec::new();for dir in [&c,&c.with_file_name("core")]{if dir.is_dir(){crate::health::collect_stamps(dir,&mut stamps)?;}}
 Ok(stamps.into_iter().map(|(p,_)|p).filter(|p|p.is_file() && resource_file(p)).collect())
}
