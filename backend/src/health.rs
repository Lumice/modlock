//! Derived compatibility evidence. No game or payload writes.
use crate::{native::NativeError, package, paths, store::Store};
use json::{object, JsonValue};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Default)]
pub struct Owner {
    state: Option<Arc<Mutex<JsonValue>>>,
    binding: String,
    key: String,
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn stamp(p: &Path) -> Result<String, NativeError> {
    paths::reject_symlinks(p)?;
    let m = fs::metadata(p)?;
    Ok(format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        p.display(),
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec()
    ))
}
pub(crate) fn build(game: &Path) -> Option<u32> {
    let p = game.parent()?.parent()?.join("appmanifest_1422450.acf");
    let s = fs::read_to_string(p).ok()?;
    let values: Vec<_> = s.split('"').collect();
    values
        .windows(3)
        .find(|v| v[0] == "buildid")
        .and_then(|v| v[2].parse().ok())
}
impl Owner {
    pub fn dispatch(&mut self, method: &str, p: &JsonValue) -> Result<JsonValue, NativeError> {
        if p.entries()
            .any(|(k, _)| !["dataDir", "profileId"].contains(&k))
        {
            return Err(NativeError::params());
        }
        if p.has_key("profileId") && !p["profileId"].is_string() {
            return Err(NativeError::params());
        }
        let root = paths::data_root(p)?;
        let store = Store::open(&root)?;
        let doc = store.document()?;
        let profile = doc["activeProfileId"]
            .as_str()
            .ok_or_else(NativeError::params)?;
        if p["profileId"].as_str().is_some_and(|v| v != profile) {
            return Err(NativeError::new(
                "healthProfileChanged",
                "Check the active profile.",
            ));
        }
        let game = PathBuf::from(doc["gamePath"].as_str().ok_or_else(NativeError::params)?);
        let citadel = game.join("game/citadel");
        let version = build(&game);
        let binding = format!("{}:{profile}:{}", root.display(), doc["revision"].dump());
        if let Some(s) = &self.state {
            if self.binding == binding && s.lock().unwrap()["running"] == true {
                return project(&store,&doc,s.lock().unwrap().clone());
            }
            if s.lock().unwrap()["running"] == true {
                return Err(NativeError::new(
                    "healthBusy",
                    "A health check is already running.",
                ));
            }
        }
        let mut stamps = Vec::new();
        let mut key = format!("health92bebec-v1:{binding}:{version:?}");
        let mut packages = Vec::new();
        let mut unavailable = JsonValue::new_array();
        for row in doc["profiles"][profile]["mods"].members() {
            let id = row["source"]["id"]
                .as_str()
                .ok_or_else(NativeError::params)?
                .to_owned();
            if row["enabled"] == false && row["available"] == false {
                unavailable.push(object!{modId:id, title:row["source"]["name"].clone(), reason:row["unavailableReason"].clone()}).map_err(|_| NativeError::params())?;
                continue;
            }
            let ps = package::selected_packages_exact(row, package::layout_version(&doc)?)?;
            let mut files = Vec::new();
            for pkg in ps {
                let directory = source_path(&store, row, &pkg.directory)?;
                let st = stamp(&directory)?;
                key.push_str(&st);
                stamps.push((directory.clone(), st));
                let mut companions = Vec::new();
                for (index, file) in pkg.companions {
                    let path = source_path(&store, row, &file)?;
                    let st = stamp(&path)?;
                    key.push_str(&st);
                    stamps.push((path.clone(), st));
                    companions.push((index, path));
                }
                files.push((directory, companions));
            }
            packages.push((id, files));
        }
        // In-place game verification updates must invalidate results even if buildid stays unchanged.
        for directory in [&citadel, &citadel.with_file_name("core")] {
            if directory.is_dir() {
                collect_stamps(directory, &mut stamps)?;
            }
        }
        for path in [
            game.parent()
                .and_then(Path::parent)
                .unwrap_or(&game)
                .join("appmanifest_1422450.acf"),
            citadel.join("steam.inf"),
        ] {
            if path.exists() {
                stamps.push((path.clone(), stamp(&path)?));
            }
        }
        for (_, st) in &stamps {
            key.push_str(st);
        }
        let key = paths::digest_bytes(key.as_bytes())?;
        if method == "native.health.get" && self.binding == binding && self.key == key {
            if let Some(s) = &self.state {
                return project(&store,&doc,s.lock().unwrap().clone());
            }
        }
        let cache = root.join("cache/mod-health.json");
        let previous = paths::read_bounded(&cache, 16 * 1024 * 1024)
            .ok()
            .and_then(|b| json::parse(std::str::from_utf8(&b).ok()?).ok());
        if previous.as_ref().is_some_and(|v| v["key"] == key) {
            let mut v = previous.unwrap()["view"].clone();
            v["cached"] = true.into();
            return project(&store,&doc,v);
        }
        if method == "native.health.get" {
            return Ok(
                object! {running:false,checked:0,total:packages.len(),report:JsonValue::Null,cached:false},
            );
        }
        let state = Arc::new(Mutex::new(
            object! {running:true,checked:0,total:packages.len(),report:JsonValue::Null,cached:false},
        ));
        self.state = Some(state.clone());
        self.binding = binding;
        self.key = key.clone();
        std::thread::spawn(move || {
            let run = || -> Result<JsonValue, NativeError> {
                paths::private_dir(&root.join("cache"))?;
                let scratch = Scratch(
                    root.join("cache")
                        .join(format!("health-{}", paths::random_id()?)),
                );
                paths::private_dir(&scratch.0)?;
                let mut inputs = Vec::new();
                for (id, ps) in packages {
                    let mut vpks = Vec::new();
                    for (i, (directory, companions)) in ps.into_iter().enumerate() {
                        if companions.is_empty() {
                            vpks.push(directory)
                        } else {
                            let dir = scratch.0.join(format!("{}-{i}", inputs.len()));
                            paths::private_dir(&dir)?;
                            let dest = dir.join("pak01_dir.vpk");
                            std::os::unix::fs::symlink(directory, &dest)?;
                            for (index, path) in companions {
                                std::os::unix::fs::symlink(
                                    path,
                                    dir.join(format!("pak01_{index:03}.vpk")),
                                )?;
                            }
                            vpks.push(dest);
                        }
                    }
                    inputs.push((id, vpks));
                }
                let observed: Option<Vec<u8>> = previous.as_ref().and_then(|v| {
                    v["observed"]
                        .is_array()
                        .then(|| {
                            v["observed"]
                                .members()
                                .map(|n| n.as_u8())
                                .collect::<Option<Vec<_>>>()
                        })
                        .flatten()
                });
                let (report, observed) = health_engine::check_with_history(
                    &citadel,
                    inputs,
                    version,
                    observed.as_deref(),
                    |n, t| {
                        let mut s = state.lock().unwrap();
                        s["checked"] = n.into();
                        s["total"] = t.into();
                    },
                )
                .map_err(|e| NativeError::new("healthAnalysisFailed", e.to_string()))?;
                for (path, expected) in &stamps {
                    if stamp(path)? != *expected {
                        return Err(NativeError::new(
                            "healthInputsChanged",
                            "Game or mod files changed during the check. Check again.",
                        ));
                    }
                }
                let mut report =
                    json::parse(&report.to_string()).map_err(|_| NativeError::params())?;
                let prev = previous
                    .as_ref()
                    .map(|v| {
                        if v["view"]["report"]["gameBuild"] == report["gameBuild"] {
                            v["view"]["report"]["previousGameBuild"].clone()
                        } else {
                            v["view"]["report"]["gameBuild"].clone()
                        }
                    })
                    .unwrap_or(JsonValue::Null);
                report["previousGameBuild"] = prev;
                report["unavailableMods"] = unavailable;
                let checked_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .to_string();
                for row in report["mods"].members_mut() {
                    row["checkedAt"] = checked_at.clone().into();
                }
                let view = object! {running:false,checked:report["mods"].len(),total:report["mods"].len(),report:report,cached:false};
                let temp = scratch.0.join("result.json");
                fs::write(
                    &temp,
                    object! {key:key,view:view.clone(),observed:JsonValue::from(observed)}.dump(),
                )?;
                paths::open_regular(&temp)?.sync_all()?;
                fs::rename(&temp, &cache)?;
                paths::sync_dir(cache.parent().unwrap())?;
                Ok(view)
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run));
            let view = match result {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    object! {running:false,checked:0,total:0,report:JsonValue::Null,error:object!{kind:e.kind,message:e.message}}
                }
                Err(_) => {
                    object! {running:false,checked:0,total:0,report:JsonValue::Null,error:object!{kind:"healthAnalysisFailed",message:"Compatibility analysis stopped unexpectedly."}}
                }
            };
            *state.lock().unwrap() = view;
        });
        Ok(self.state.as_ref().unwrap().lock().unwrap().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_rpc_cache_and_build_invalidation() {
        let mut f = crate::test_support::Fixture::new();
        f.game = f.root.join("steamapps/common/Deadlock");
        fs::create_dir_all(f.game.join("game/citadel/addons/other-folder")).unwrap();
        f.reset();
        fs::write(
            f.game.join("game/citadel/addons/pak01_dir.vpk"),
            crate::test_support::vpk("sounds/test.wav", 0),
        )
        .unwrap();
        let manifest = f.root.join("steamapps/appmanifest_1422450.acf");
        fs::write(&manifest, "\"buildid\" \"1\"").unwrap();
        let mut service = crate::native::NativeService::new();
        let preview = service
            .dispatch("native.importPreview", &f.preview_params())
            .unwrap();
        let mut params = f.params();
        params["token"] = preview["token"].clone();
        params["expectedRevision"] = "0".into();
        params["operationId"] = "healthimport".into();
        service.dispatch("native.importCommit", &params).unwrap();
        let revision = Store::open(&f.data).unwrap().document().unwrap()["revision"].clone();
        let wait = |service: &mut crate::native::NativeService| {
            for _ in 0..500 {
                let v = service.dispatch("native.health.get", &f.params()).unwrap();
                if v["running"] != true {
                    return v;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("health did not settle")
        };
        service
            .dispatch("native.health.check", &f.params())
            .unwrap();
        let v = wait(&mut service);
        assert!(v["error"].is_null(), "{}", v.dump());
        assert_eq!(v["report"]["mods"][0]["status"], "notCovered");
        assert_eq!(v["report"]["gameBuild"], 1);
        let mut second = crate::native::NativeService::new();
        assert_eq!(
            second.dispatch("native.health.get", &f.params()).unwrap()["cached"],
            true
        );
        fs::write(&manifest, "\"buildid\" \"2\"").unwrap();
        assert!(second.dispatch("native.health.get", &f.params()).unwrap()["report"].is_null());
        second.dispatch("native.health.check", &f.params()).unwrap();
        let updated = wait(&mut second);
        assert_eq!(updated["report"]["gameBuild"], 2);
        assert_eq!(updated["report"]["previousGameBuild"], 1);
        assert_eq!(
            Store::open(&f.data).unwrap().document().unwrap()["revision"],
            revision
        );
    }
    #[test]
    fn health_checks_available_mods_and_reports_missing_disabled_mods() {
        let f = crate::test_support::Fixture::new();
        let mut source = f.state();
        source["local-config"]["state"]["localMods"].push(object!{id:"missing",remoteId:"missing",name:"Missing mod",status:"disabled"}).unwrap();
        f.write_state(source);
        let mut service = crate::native::NativeService::new();
        crate::test_support::import(&mut service, &f);
        let before=Store::open(&f.data).unwrap().document().unwrap();
        let rows=&before["profiles"]["default"]["mods"];
        assert_eq!(rows.len(),2);
        assert_eq!(rows[1]["available"],false);
        assert_eq!(rows[1]["enabled"],false);
        service.dispatch("native.health.check",&f.params()).unwrap();
        let result=(0..500).find_map(|_| {
            let v=service.dispatch("native.health.get",&f.params()).unwrap();
            if v["running"]==true {std::thread::sleep(std::time::Duration::from_millis(10));None} else {Some(v)}
        }).expect("health check did not finish");
        assert!(result["error"].is_null(),"{}",result.dump());
        assert_eq!(result["report"]["mods"].len(),1);
        assert_eq!(result["report"]["mods"][0]["modId"],"local-a");
        assert_eq!(result["report"]["unavailableMods"][0]["modId"],"missing");
        assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["revision"],before["revision"]);
    }
    #[test]
    fn repair_preference_persistence_and_original_health(){
        let f=crate::test_support::Fixture::new();let mut service=crate::native::NativeService::new();crate::test_support::import(&mut service,&f);
        let store=Store::open(&f.data).unwrap();let doc=store.document().unwrap();let id=doc["profiles"]["default"]["mods"][0]["source"]["id"].clone();
        let mut p=f.params();p["profileId"]="default".into();p["modId"]=id.clone();p["enabled"]=false.into();p["operationId"]="repair-choice".into();p["expectedRevision"]="1".into();
        service.dispatch("native.setFixEnabled",&p).unwrap();assert_eq!(service.dispatch("native.setFixEnabled",&p).unwrap()["revision"],"2");
        let doc=Store::open(&f.data).unwrap().document().unwrap();assert_eq!(doc["profiles"]["default"]["mods"][0]["fixEnabled"],false);
        assert!(!crate::deployment_intent::DeploymentIntent::capture(&doc,"default").unwrap().mods[0].fix_enabled);
        let view=object!{running:false,report:object!{gameBuild:1,mods:json::array![object!{modId:id.clone(),status:"partlyFixable"}]}};
        assert_eq!(project(&store,&doc,view).unwrap()["report"]["mods"][0]["status"],"originalFiles");
        let mut p=f.params();p["enabled"]=false.into();p["operationId"]="repair-auto".into();p["expectedRevision"]="2".into();service.dispatch("native.setFixAutomatically",&p).unwrap();assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["fixAutomatically"],false);
    }
    #[test]
    fn health_build_from_manifest() {
        let p = std::env::temp_dir().join(format!("health-build-{}", paths::random_id().unwrap()));
        fs::create_dir_all(p.join("common/Game")).unwrap();
        fs::write(
            p.join("appmanifest_1422450.acf"),
            "\"AppState\" {\"buildid\" \"6841\"}",
        )
        .unwrap();
        assert_eq!(build(&p.join("common/Game")), Some(6841));
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn health_uncovered_and_malformed() {
        let p =
            std::env::temp_dir().join(format!("health-synthetic-{}", paths::random_id().unwrap()));
        fs::create_dir_all(&p).unwrap();
        let modpath = p.join("mod.vpk");
        fs::write(&modpath, crate::test_support::vpk("sound/test.wav", 0)).unwrap();
        let (report, observed) = health_engine::check_with_history(
            &p,
            vec![("m".into(), vec![modpath.clone()])],
            Some(1),
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(report["mods"][0]["status"], "notCovered");
        assert!(!observed.is_empty());
        fs::write(&modpath, b"bad").unwrap();
        assert!(
            health_engine::check(&p, vec![("m".into(), vec![modpath])], Some(1), |_, _| {})
                .is_err()
        );
        fs::remove_dir_all(p).unwrap();
    }
}

#[cfg(test)]
thread_local! { pub(crate) static STAMP_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(crate) fn resource_directory(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name != "addons" && name != "deadlock_native"
        && !name.starts_with("deadlock_native_") && !name.starts_with("dmm_")
}
pub(crate) fn collect_stamps(dir: &Path, out: &mut Vec<(PathBuf, String)>) -> Result<(), NativeError> {
    #[cfg(test)]
    STAMP_WALKS.with(|n| n.set(n.get()+1));
    out.push((dir.to_owned(), stamp(dir)?));
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let t = e.file_type()?;
        if t.is_dir() {
            if resource_directory(e.file_name().as_os_str()) {
                collect_stamps(&e.path(), out)?;
            }
        } else if t.is_file() {
            let path = e.path();
            out.push((path.clone(), stamp(&path)?));
        }
    }
    Ok(())
}

pub(crate) fn source_path(
    store: &Store,
    row: &JsonValue,
    file: &package::FileRef,
) -> Result<PathBuf, NativeError> {
    let payload = store.payload_path(&file.id)?;
    if payload.exists() {
        return Ok(payload);
    }
    for origin in row["source"]["installedGameOrigins"].members() {
        for member in origin["files"].members() {
            if member["sha256"] == file.sha256 {
                let path = PathBuf::from(
                    member["sourcePath"]
                        .as_str()
                        .ok_or_else(NativeError::params)?,
                );
                if paths::fingerprint(&path)?.sha256 == file.sha256 {
                    return Ok(path);
                }
            }
        }
    }
    Err(NativeError::new(
        "healthMissingPayload",
        "A mod's files are missing. Reimport this mod before checking it.",
    ))
}

pub(crate) fn project(store:&Store,doc:&JsonValue,mut view:JsonValue)->Result<JsonValue,NativeError>{
 view["fixAutomatically"]=doc["fixAutomatically"].as_bool().unwrap_or(true).into();
 let receipt=&doc["applied"]["manifest"]["repairs"];
 let verified=!receipt["file"].is_null() && receipt["gameBuild"]==view["report"]["gameBuild"] && crate::apply::verify_applied(store.root(),doc).is_ok();
 for row in view["report"]["mods"].members_mut(){
 let record=doc["profiles"][doc["activeProfileId"].as_str().unwrap_or("")]["mods"].members().find(|m|m["source"]["id"]==row["modId"]);
 let fix=record.and_then(|r|r["fixEnabled"].as_bool()).unwrap_or(doc["fixAutomatically"].as_bool().unwrap_or(true));row["fixEnabled"]=fix.into();
 if matches!(row["status"].as_str(),Some("fixable"|"partlyFixable")){
 if !fix{row["status"]="originalFiles".into();}
 else if verified && receipt["mods"].members().any(|m|m["modId"]==row["modId"]&&m["repairs"].as_u32().unwrap_or(0)>0) && receipt["choices"]["mods"].members().any(|m|m["modId"]==row["modId"]&&m["enabled"]==true){row["status"]=if row["status"]=="partlyFixable"{"partlyFixed"}else{"fixed"}.into();}
 }
 }
 Ok(view)
}
