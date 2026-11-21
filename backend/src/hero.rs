// GPL-3.0: adapted from upstream packages/hero-parser; see UPSTREAM-LICENSE.
//! Read-only hero inference. Only bounded VPK directories are read, never payloads.
use crate::native::NativeError;
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::LazyLock,
};
static MAPPING: LazyLock<JsonValue> = LazyLock::new(|| {
    json::parse(include_str!("../data/hero-mapping.json")).expect("compiled hero mapping")
});

pub fn display_for_internal(name: &str) -> Option<String> {
    let name = name.to_lowercase();
    MAPPING
        .members()
        .find(|r| r["internal"] == name.as_str())
        .map(|r| r["display"].as_str().unwrap().to_string())
}

pub fn detect_paths(paths: &[String]) -> JsonValue {
    let mut counts = BTreeMap::<String, (String, usize)>::new();
    let mut names = BTreeSet::new();
    let mut critical = BTreeSet::new();
    for path in paths {
        let path = path.replace('\\', "/").to_lowercase();
        if [
            "scripts/abilities.vdata_c",
            "scripts/heroes.vdata_c",
            "scripts/generic_data.vdata_c",
        ]
        .contains(&path.as_str())
        {
            critical.insert(path.clone());
        }
        for prefix in [
            "models/heroes_wip/",
            "models/heroes_staging/",
            "models/heroes/",
        ] {
            if let Some(rest) = path.strip_prefix(prefix) {
                if let Some((name, _)) = rest.split_once('/') {
                    if !name.is_empty() {
                        names.insert(name.to_string());
                        if let Some(row) = MAPPING.members().find(|r| r["internal"] == name) {
                            counts
                                .entry(row["key"].as_str().unwrap().into())
                                .or_insert((row["display"].as_str().unwrap().into(), 0))
                                .1 += 1;
                        }
                    }
                }
            }
        }
    }
    let maximum = counts.values().map(|(_, n)| *n).max().unwrap_or(0);
    let winners: Vec<_> = counts.iter().filter(|(_, (_, n))| *n == maximum).collect();
    let (hero, display) = if winners.len() == 1 {
        (winners[0].0.clone().into(), winners[0].1 .0.clone().into())
    } else {
        (JsonValue::Null, JsonValue::Null)
    };
    object! {hero:hero,heroDisplay:display,category:if winners.len()==1 {"hero"} else {"other"},internalNames:names.into_iter().collect::<Vec<_>>(),usesCriticalPaths:!critical.is_empty(),criticalPaths:critical.into_iter().collect::<Vec<_>>()}
}

pub fn dispatch(params: &JsonValue) -> Result<JsonValue, NativeError> {
    if params.has_key("modId") {
        return detect_mod(params);
    }
    if !params.is_object() || params.len() != 1 {
        return Err(NativeError::params());
    }
    let path = params["vpkPath"]
        .as_str()
        .filter(|p| !p.is_empty() && p.len() <= 4096)
        .ok_or_else(NativeError::params)?;
    let path = Path::new(path);
    if !path.is_absolute() || path.extension().and_then(|s| s.to_str()) != Some("vpk") {
        return Err(NativeError::params());
    }
    let directory = crate::vpk::inspect_directory(path)?;
    Ok(detect_paths(
        &directory
            .resources
            .into_iter()
            .map(|r| r.entry.path)
            .collect::<Vec<_>>(),
    ))
}

// A single request reads at most 32 directories / 100,000 combined entries.
// Cache includes root and payload hashes; failures are retried, never cached.
fn detect_mod(params: &JsonValue) -> Result<JsonValue, NativeError> {
    use std::sync::Mutex;
    static CACHE: LazyLock<Mutex<BTreeMap<String, JsonValue>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));
    if params.len() != 5 || params["version"] != 1 {
        return Err(NativeError::params());
    }
    let root = params["dataDir"].as_str().ok_or_else(NativeError::params)?;
    let store = crate::store::Store::open(Path::new(root))?;
    let doc = store.document()?;
    if params["expectedRevision"] != doc["revision"] {
        return Err(NativeError::new(
            "revisionConflict",
            "Library changed during hero detection.",
        ));
    }
    let id = params["modId"].as_str().ok_or_else(NativeError::params)?;
    let profile = params["profileId"]
        .as_str()
        .ok_or_else(NativeError::params)?;
    let record = doc["profiles"][profile]["mods"]
        .members()
        .find(|m| m["source"]["id"] == id)
        .ok_or_else(NativeError::params)?;
    let packages =
        crate::package::selected_packages_exact(record, crate::package::layout_version(&doc)?)?;
    if packages.len() > 32 {
        return Err(NativeError::params());
    }
    let fingerprint = packages
        .iter()
        .map(|p| {
            format!(
                "{}:{}:{}",
                p.directory.id, p.directory.sha256, p.directory.size
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let key = format!("{root}|{fingerprint}");
    let cached = CACHE.lock().unwrap().get(&key).cloned();
    let detection = if let Some(value) = cached {
        value
    } else {
        let mut paths = Vec::new();
        for package in packages {
            let directory =
                crate::vpk::inspect_directory(&store.payload_path(&package.directory.id)?)?;
            if paths.len() + directory.resources.len() > 100_000 {
                return Err(NativeError::params());
            }
            paths.extend(directory.resources.into_iter().map(|r| r.entry.path));
        }
        let value = detect_paths(&paths);
        let mut cache = CACHE.lock().unwrap();
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(key, value.clone());
        value
    };
    Ok(
        object! {modId:id,revision:doc["revision"].clone(),fingerprint:fingerprint,heroDisplay:detection["heroDisplay"].clone()},
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{rpc, vpk, Fixture};
    #[test]
    fn hero_mod_rpc_cache_and_revision_binding() {
        let f = Fixture::new();
        std::fs::write(
            f.game.join("game/citadel/addons/pak01_dir.vpk"),
            vpk("models/heroes/archer/model.vmdl_c", 0),
        )
        .unwrap();
        std::fs::write(
            f.game.join("game/citadel/addons/pak02_dir.vpk"),
            vpk("models/heroes/bebop/model.vmdl_c", 0),
        )
        .unwrap();
        let mut manifest =
            json::parse(&std::fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        manifest["mods"]["remote-a"]["currentVpks"]
            .push("pak02_dir.vpk")
            .unwrap();
        manifest["mods"]["remote-a"]["originalVpkNames"]
            .push("b_dir.vpk")
            .unwrap();
        f.write_manifest("", manifest);
        let mut service = crate::native::NativeService::new();
        let snapshot = crate::test_support::import(&mut service, &f);
        let mut params = object! {version:1,dataDir:f.data.to_str().unwrap(),profileId:"default",modId:"local-a",expectedRevision:snapshot["revision"].clone()};
        let first = dispatch(&params).unwrap();
        assert!(
            first["heroDisplay"].is_null(),
            "multi-package tie must remain unknown"
        );
        assert!(!first["fingerprint"].as_str().unwrap().is_empty());
        assert_eq!(dispatch(&params).unwrap(), first);
        params["expectedRevision"] = "0".into();
        assert!(dispatch(&params).is_err());
    }
    #[test]
    fn hero_mapping_all_aliases() {
        assert!(!MAPPING.is_empty());
        let fixture = Fixture::new();
        let package = fixture.root.join("mapping.vpk");
        for row in MAPPING.members() {
            let path = format!(
                "models/heroes_staging/{}/model.vmdl_c",
                row["internal"].as_str().unwrap()
            );
            assert_eq!(detect_paths(&[path.clone()])["hero"], row["key"]);
            std::fs::write(&package, vpk(&path, 0)).unwrap();
            assert_eq!(
                dispatch(&object! {vpkPath:package.to_str().unwrap()}).unwrap()["hero"],
                row["key"]
            );
        }
    }
    #[test]
    fn hero_counts_ties_unknown_and_critical() {
        let paths = [
            "models/heroes/archer/a.vmdl_c",
            "models/heroes_wip/grey_talon/b.vmdl_c",
            "models/heroes/bebop/a.vmdl_c",
            "scripts/heroes.vdata_c",
        ]
        .map(String::from);
        let result = detect_paths(&paths);
        assert_eq!(result["heroDisplay"], "Grey Talon");
        assert_eq!(result["usesCriticalPaths"], true);
        assert!(detect_paths(&paths[1..3])["hero"].is_null());
        assert!(detect_paths(&["models/heroes/new_hero/a.vmdl_c".into()])["hero"].is_null());
        assert!(detect_paths(&["sounds/archer/a.wav".into()])["hero"].is_null());
    }
    #[test]
    fn hero_rpc_synthetic_vpk_and_invalid_inputs() {
        let f = Fixture::new();
        let path = f.root.join("hero.vpk");
        std::fs::write(&path, vpk("models/heroes_staging/gigawatt/model.vmdl_c", 0)).unwrap();
        let params = object! {vpkPath:path.to_str().unwrap()};
        let result = rpc("native.hero.detect", params.clone());
        assert_eq!(result["result"]["hero"], "Seven", "{}", result.dump());
        assert!(!f.data.exists());
        let mut invalid = params.clone();
        invalid["extra"] = true.into();
        assert!(rpc("native.hero.detect", invalid).has_key("error"));
        let linked = f.root.join("linked.vpk");
        std::os::unix::fs::symlink(&path, &linked).unwrap();
        assert!(rpc(
            "native.hero.detect",
            object! {vpkPath:linked.to_str().unwrap()}
        )
        .has_key("error"));
        assert!(rpc("native.hero.detect", object! {vpkPath:"relative.vpk"}).has_key("error"));
        std::fs::write(&path, b"invalid").unwrap();
        assert!(rpc("native.hero.detect", params).has_key("error"));
    }
}
