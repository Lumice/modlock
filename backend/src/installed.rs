//! Read-only Steam discovery and installed package inventory.
use crate::{native::NativeError, paths, store::Store, vpk};
use json::{object, JsonValue};
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

// Small bounded KeyValues reader; preserves nesting and quoted escapes.
fn kv(text: &str) -> Result<JsonValue, NativeError> {
    let mut tokens = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1
            }
            continue;
        }
        if matches!(b[i], b'{' | b'}') {
            tokens.push((b[i] as char).to_string());
            i += 1;
            continue;
        }
        let mut out = Vec::new();
        if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                    if i == b.len() {
                        return Err(NativeError::params());
                    }
                }
                out.push(b[i]);
                i += 1
            }
            if i == b.len() {
                return Err(NativeError::params());
            }
            i += 1
        } else {
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}') {
                out.push(b[i]);
                i += 1
            }
        }
        tokens.push(String::from_utf8(out).map_err(|_| NativeError::params())?);
    }
    fn block(t: &[String], i: &mut usize, depth: usize) -> Result<JsonValue, NativeError> {
        if depth > 32 {
            return Err(NativeError::params());
        }
        let mut out = JsonValue::new_object();
        while *i < t.len() && t[*i] != "}" {
            let key = t[*i].clone();
            *i += 1;
            if *i == t.len() {
                return Err(NativeError::params());
            }
            let value = if t[*i] == "{" {
                *i += 1;
                let v = block(t, i, depth + 1)?;
                if t.get(*i).map(String::as_str) != Some("}") {
                    return Err(NativeError::params());
                }
                *i += 1;
                v
            } else {
                let v = t[*i].clone().into();
                *i += 1;
                v
            };
            if out.has_key(&key) {
                return Err(NativeError::params());
            }
            out[&key] = value;
        }
        Ok(out)
    }
    let mut n = 0;
    let out = block(&tokens, &mut n, 0)?;
    if n != tokens.len() {
        return Err(NativeError::params());
    }
    Ok(out)
}
fn read_kv(p: &Path) -> Result<JsonValue, NativeError> {
    let text = fs::read(p)?;
    if text.len() > 8 * 1024 * 1024 {
        return Err(NativeError::params());
    }
    kv(std::str::from_utf8(&text).map_err(|_| NativeError::params())?)
}
pub fn valid_game(p: &Path) -> bool {
    crate::apply::citadel(p).is_ok_and(|c| paths::open_regular(&c.join("gameinfo.gi")).is_ok())
}
/// Shared admission/detection identity; resolve user spellings before safe access.
pub(crate) fn install_path(p: &Path) -> Result<PathBuf, NativeError> {
    let invalid=||NativeError::new("invalidGamePath","This is not a complete Deadlock installation. Choose the Steam Deadlock folder.");
    let p=fs::canonicalize(p).map_err(|_|invalid())?;
    if !valid_game(&p) || paths::open_regular(&p.join("game/citadel/pak01_dir.vpk")).is_err()
        || !["win64","linuxsteamrt64"].iter().any(|n| {
            let bin=p.join("game/bin").join(n);
            paths::reject_symlinks(&bin).is_ok() && bin.is_dir()
        }) {return Err(invalid());}
    Ok(p)
}
pub fn detect(home: &Path) -> JsonValue {
    let mut libraries = BTreeSet::new();
    for suffix in [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        // Snap Steam (Ubuntu's default package).
        "snap/steam/common/.local/share/Steam",
    ] {
        let root = home.join(suffix);
        libraries.insert(root.clone());
        if let Ok(v) = read_kv(&root.join("steamapps/libraryfolders.vdf")) {
            for (_, entry) in v["libraryfolders"].entries() {
                if let Some(p) = entry["path"].as_str().or_else(|| entry.as_str()) {
                    if Path::new(p).is_absolute() {
                        libraries.insert(PathBuf::from(p));
                    }
                }
            }
        }
    }
    let mut found = BTreeSet::new();
    for library in libraries {
        let manifest = read_kv(&library.join("steamapps/appmanifest_1422450.acf")).ok();
        let dir = manifest
            .as_ref()
            .and_then(|v| v["AppState"]["installdir"].as_str())
            .unwrap_or("Deadlock");
        if Path::new(dir).components().count() != 1 || dir == ".." {
            continue;
        }
        let p = library.join("steamapps/common").join(dir);
        if let Ok(p) = install_path(&p) {
            found.insert(p.to_string_lossy().into_owned());
        }
    }
    let candidates = found.into_iter().collect::<Vec<_>>();
    object! {detectedGamePath:candidates.first().cloned().unwrap_or_default(),candidates:candidates}
}
fn dirs(c: &Path) -> Result<Vec<PathBuf>, NativeError> {
    let mut dirs = Vec::new();
    for n in 1..=10 {
        let p = c.join(if n == 1 {
            "addons".into()
        } else {
            format!("addons{n}")
        });
        if !p.exists() {
            continue;
        }
        paths::reject_symlinks(&p)?;
        dirs.push(p.clone());
        for e in fs::read_dir(&p)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                paths::reject_symlinks(&e.path())?;
                dirs.push(e.path())
            }
        }
    }
    Ok(dirs)
}
fn manifest(dir: &Path) -> JsonValue {
    paths::read_bounded(&dir.join(".dmm.json"), 8 * 1024 * 1024)
        .ok()
        .and_then(|b| json::parse(&String::from_utf8_lossy(&b)).ok())
        .unwrap_or(JsonValue::Null)
}
fn suggestion(v: &JsonValue, name: &str) -> JsonValue {
    let matches = v["mods"]
        .entries()
        .filter(|(_, m)| {
            ["currentVpks", "disabledVpks"]
                .iter()
                .any(|k| m[*k].members().any(|x| x == name))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return JsonValue::Null;
    }
    let (id, m) = matches[0];
    let (kind, num) = id
        .strip_prefix("snd-")
        .map(|x| ("sounds", x))
        .unwrap_or(("mods", id));
    let link = if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
        format!("https://gamebanana.com/{kind}/{num}")
    } else {
        String::new()
    };
    object! {modId:id,name:m["name"].as_str().unwrap_or(id),thumbnail:JsonValue::Null,basis:"manifest",link:link,requiresConfirmation:true}
}
fn upstream(source: Option<&Path>) -> JsonValue {
    let path = source.map(Path::to_path_buf);
    let Some(p) = path else {
        return JsonValue::Null;
    };
    let Ok(b) = paths::read_bounded(&p, paths::MAX_JSON_BYTES) else {
        return JsonValue::Null;
    };
    let Ok(v) = json::parse(&String::from_utf8_lossy(&b)) else {
        return JsonValue::Null;
    };
    let config = if let Some(s) = v["local-config"].as_str() {
        json::parse(s).unwrap_or(JsonValue::Null)
    } else {
        v["local-config"].clone()
    };
    config["state"].clone()
}
/// Metadata only: never changes source paths or publication destinations.
pub(crate) fn original_package_name(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    let v = manifest(dir);
    let mut names = Vec::new();
    for (_, m) in v["mods"].entries() {
        for k in ["currentVpks", "disabledVpks"] {
            for (index, item) in m[k].members().enumerate() {
                if item == name {
                    if let Some(n) = m["originalVpkNames"][index]
                        .as_str()
                        .filter(|s| paths::safe_name(s) && s.ends_with(".vpk"))
                    {
                        names.push(n.to_owned());
                    }
                }
            }
        }
    }
    if names.len() == 1 {
        names.pop()
    } else {
        None
    }
}
pub fn scan(game: &Path, root: Option<&Path>) -> Result<JsonValue, NativeError> {
    scan_with_source(game, root, None)
}
pub fn scan_with_source(
    game: &Path,
    root: Option<&Path>,
    source: Option<&Path>,
) -> Result<JsonValue, NativeError> {
    if !valid_game(game) {
        return Err(NativeError::new(
            "invalidGamePath",
            "Choose a valid Deadlock installation.",
        ));
    }
    let c = crate::apply::citadel(game)?;
    let doc = root
        .map(Store::open)
        .transpose()?
        .map(|s| s.document())
        .transpose()?;
    let state = upstream(source.or_else(|| {
        doc.as_ref()
            .and_then(|d| d["sourcePath"].as_str())
            .map(Path::new)
    }));
    let mut library = std::collections::BTreeMap::new();
    for m in state["localMods"].members().chain(
        state["profiles"]
            .entries()
            .flat_map(|(_, p)| p["mods"].members()),
    ) {
        if let Some(id) = m["remoteId"].as_str().or(m["id"].as_str()) {
            library.entry(id.to_owned()).or_insert_with(|| m.clone());
        }
    }
    let mut items: Vec<JsonValue> = Vec::new();
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    for dir in dirs(&c)? {
        let manifest = manifest(&dir);
        let mut entries = fs::read_dir(&dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with("_dir.vpk") {
                continue;
            }
            paths::reject_symlinks(&p)?;
            let fp = paths::fingerprint(&p)?;
            let mut files = vec![p.to_string_lossy().into_owned()];
            let mut size = fp.size;
            let mut issue = JsonValue::Null;
            match vpk::inspect_directory(&p) {
                Ok(d) => {
                    let stem = name.strip_suffix("_dir.vpk").unwrap();
                    for index in vpk::required_archives(&d) {
                        let companion = dir.join(format!("{stem}_{index:03}.vpk"));
                        match paths::fingerprint(&companion) {
                            Ok(f) => {
                                size += f.size;
                                files.push(companion.to_string_lossy().into_owned())
                            }
                            Err(_) => issue = "Missing or unsafe companion".into(),
                        }
                    }
                }
                Err(e) if e.message == vpk::DUPLICATE_ENTRY_MESSAGE => issue = e.message.into(),
                Err(_) => issue = "This mod’s file can’t be read safely. Add it again from Browse.".into(),
            }
            let already = doc.as_ref().is_some_and(|d| {
                d["profiles"].entries().any(|(_, profile)| {
                    profile["mods"].members().any(|m| {
                        crate::local_import::record_files(
                            m,
                            crate::package::layout_version(d).unwrap_or(4),
                        )
                        .is_ok_and(|refs| refs.iter().any(|(f, _)| f.sha256 == fp.sha256))
                    })
                })
            });
            // Native generations are outside addons; never claim arbitrary addon files as owned.
            items.push(object!{id:paths::digest_bytes(format!("{}\0{}",p.display(),fp.sha256).as_bytes())?,path:p.to_string_lossy().as_ref(),files:files,sizeBytes:size,sha256:fp.sha256,suggestedMatch:suggestion(&manifest,&name),alreadyInLibrary:already,error:issue});
            let mut row = items.pop().unwrap();
            let mut matched = row["suggestedMatch"].clone();
            // Local UUID prefix is verified against an exact upstream library ID.
            if matched.is_null() {
                let candidates = library
                    .keys()
                    .filter(|id| id.starts_with("local-") && name.starts_with(&format!("{id}_")))
                    .collect::<Vec<_>>();
                if candidates.len() == 1 {
                    matched = object! {modId:candidates[0].as_str(),name:candidates[0].as_str(),thumbnail:JsonValue::Null,basis:"mod manager library",link:"",requiresConfirmation:true};
                }
            }
            if let Some(m) = library.get(matched["modId"].as_str().unwrap_or("")) {
                if let Some(n) = m["name"].as_str() {
                    matched["name"] = n.into();
                }
                matched["author"] = m["author"].as_str().map(str::trim)
                    .filter(|s| !s.eq_ignore_ascii_case("unknown")).unwrap_or("").into();
                // Only the existing provider image host, never local/data URLs.
                matched["thumbnail"] = m["images"]
                    .members()
                    .find(|i| {
                        i.as_str()
                            .is_some_and(|s| s.starts_with("https://images.gamebanana.com/"))
                    })
                    .cloned()
                    .unwrap_or(JsonValue::Null);
            }
            if matched.is_object() && !matched["author"].is_string() {
                matched["author"] = "".into();
            }
            row["suggestedMatch"] = matched.clone();
            row["packages"] = json::array![row["path"].clone()];
            if let Some(id) = matched["modId"].as_str() {
                let key = format!("{}\0{id}", dir.display());
                if let Some(index) = groups.get(&key).copied() {
                    let group = &mut items[index];
                    for file in row["files"].members() {
                        group["files"].push(file.clone()).unwrap();
                    }
                    group["packages"].push(row["path"].clone()).unwrap();
                    group["sizeBytes"] = (group["sizeBytes"].as_u64().unwrap() + size).into();
                    group["alreadyInLibrary"] =
                        (group["alreadyInLibrary"] == true && already).into();
                    if !row["error"].is_null() {
                        group["error"] = row["error"].clone();
                    }
                    continue;
                }
                groups.insert(key, items.len());
            }
            items.push(row);
            if items.len() > 4096 {
                return Err(NativeError::new(
                    "installedScanLimit",
                    "Too many installed packages.",
                ));
            }
        }
    }
    Ok(object! {gamePath:game.to_string_lossy().as_ref(),items:items})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vdf_nested_escapes_and_missing() {
        let v = kv(r#""libraryfolders" { "0" {"path" "/a" "apps" {"1422450" "1"}} "1" "/b" }"#)
            .unwrap();
        assert_eq!(v["libraryfolders"]["0"]["path"], "/a");
        assert!(v["libraryfolders"]["1"]["apps"]["1422450"].is_null());
        assert!(kv("a {").is_err());
        assert_eq!(kv(r#"a "C:\\Steam""#).unwrap()["a"], r"C:\Steam");
    }
    #[test]
    fn flatpak_and_multiple_libraries() {
        let base = std::env::temp_dir().join(format!("dmm-detect-{}", paths::random_id().unwrap()));
        let steam = base.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps");
        let library = base.join("library");
        fs::create_dir_all(&steam).unwrap();
        fs::create_dir_all(library.join("steamapps/common/Custom/game/citadel")).unwrap();
        fs::write(steam.join("libraryfolders.vdf"),format!(r#""libraryfolders" {{ "0" {{ "path" "{}" "apps" {{"1422450" "1"}} }} "1" "/missing" }}"#,library.display())).unwrap();
        fs::write(
            library.join("steamapps/appmanifest_1422450.acf"),
            r#""AppState" {"installdir" "Custom"}"#,
        )
        .unwrap();
        fs::write(
            library.join("steamapps/common/Custom/game/citadel/gameinfo.gi"),
            "fixture",
        )
        .unwrap();
        fs::create_dir_all(library.join("steamapps/common/Custom/game/bin/win64")).unwrap();
        fs::write(library.join("steamapps/common/Custom/game/citadel/pak01_dir.vpk"),b"fixture").unwrap();
        assert_eq!(detect(&base)["candidates"].len(), 1);
        fs::remove_dir_all(base).unwrap();
    }
    #[test]
    fn snap_steam_library() {
        let base = std::env::temp_dir().join(format!("dmm-detect-{}", paths::random_id().unwrap()));
        let game = base.join("snap/steam/common/.local/share/Steam/steamapps/common/Deadlock");
        fs::create_dir_all(game.join("game/citadel")).unwrap();
        fs::create_dir_all(game.join("game/bin/linuxsteamrt64")).unwrap();
        fs::write(game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
        fs::write(game.join("game/citadel/pak01_dir.vpk"), b"fixture").unwrap();
        assert_eq!(detect(&base)["candidates"].len(), 1, "Snap Steam's default library is found");
        fs::remove_dir_all(base).unwrap();
    }
}

/// Whole mount baseline captured during local preparation, never during Apply.
pub(crate) fn capture_origins(
    request: &crate::local_content::LocalPrepareRequest,
) -> Result<JsonValue, NativeError> {
    let doc = match Store::open(&request.root) {
        Ok(store) => store.document()?,
        Err(e) if e.kind == "nativeNotImported" => return Ok(json::array![]),
        Err(e) => return Err(e),
    };
    let game = doc["gamePath"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            request
                .roots
                .iter()
                .flat_map(|p| p.ancestors())
                .find(|p| valid_game(p))
                .map(Path::to_path_buf)
        });
    let Some(game) = game else {
        return Ok(json::array![]);
    };
    let c = game.join("game/citadel");
    let mut mounts = BTreeSet::new();
    for root in &request.roots {
        if let Some(parent) = root.parent() {
            if let Ok(relative) = parent.strip_prefix(&c) {
                let parts = relative.components().collect::<Vec<_>>();
                if (1..=2).contains(&parts.len()) {
                    let shard = parts[0].as_os_str().to_string_lossy();
                    if (1..=10).any(|n| {
                        shard
                            == if n == 1 {
                                "addons".into()
                            } else {
                                format!("addons{n}")
                            }
                    }) {
                        mounts.insert(parent.to_path_buf());
                    }
                }
            }
        }
    }
    let mut result = Vec::new();
    for dir in mounts {
        let files = mount_proof(&dir)?;
        result.push(object!{treeComplete:true,treeVersion:1,gamePath:game.to_string_lossy().as_ref(),mount:format!("citadel/{}",dir.strip_prefix(&c).unwrap().to_string_lossy()),directory:dir.to_string_lossy().as_ref(),files:files});
    }
    Ok(result.into())
}
// Enumerate every loadable entry. Never follow unknown filesystem objects.
fn mount_entries(dir:&Path, files:&mut Vec<JsonValue>, hash:bool)->Result<(),NativeError>{
    paths::reject_symlinks(dir)?;
    let mut entries=fs::read_dir(dir)?.collect::<Result<Vec<_>,_>>()?;
    entries.sort_by_key(|e|e.file_name());
    for e in entries {
        let p=e.path();let source=p.to_str().ok_or_else(NativeError::params)?;let m=fs::symlink_metadata(&p)?;
        if !m.is_file() && !m.is_dir(){return Err(NativeError::new("externalChange","A mod folder contains an unsupported file type."));}
        let id=crate::apply::identity(&m);
        if m.is_dir(){
            files.push(object!{sourcePath:source,state:"present",proofVersion:2,proofKind:"directory",member:true,directory:true,identity:id});
            if files.len()>100_000{return Err(NativeError::params());}
            mount_entries(&p,files,hash)?;
        }else{
            let mut row=object!{sourcePath:source,state:"present",proofVersion:2,proofKind:"bytes",member:true,directory:false,size:m.len(),device:m.dev(),inode:m.ino(),modifiedSeconds:m.mtime(),modifiedNanos:m.mtime_nsec(),identity:id};
            if hash {row["sha256"]=paths::fingerprint(&p)?.sha256.into();}
            files.push(row);
        }
        if files.len()>100_000{return Err(NativeError::params());}
    }
    Ok(())
}
pub(crate) fn mount_proof(dir:&Path)->Result<JsonValue,NativeError>{
    let mut files=Vec::new();mount_entries(dir,&mut files,true)?;Ok(files.into())
}
/// Only origins of the requested setup can authorize adoption. Once applied,
/// native checkpoint ownership replaces historical import origins entirely.
pub(crate) fn origins_for_apply(doc:&JsonValue,profile:&str)->Result<Vec<JsonValue>,NativeError>{
    if !doc["applied"].is_null(){return Ok(Vec::new());}
    let mut found=std::collections::BTreeMap::new();
    for m in doc["profiles"][profile]["mods"].members() {
        for origin in m["source"]["installedGameOrigins"].members() {
            if doc["previousGameFolders"].is_array() && origin["gamePath"]!=doc["gamePath"] {continue;}
            let mount=origin["mount"].as_str().ok_or_else(NativeError::params)?;
            if let Some(old)=found.insert(mount.to_owned(),origin.clone()) {
                if old!=*origin {return Err(NativeError::new("externalChange","Installed mount baselines disagree."));}
            }
        }
    }
    Ok(found.into_values().collect())
}
pub(crate) fn origins(doc:&JsonValue)->Result<Vec<JsonValue>,NativeError>{
    origins_for_apply(doc,doc["activeProfileId"].as_str().unwrap_or("default"))
}
pub(crate) fn verify_origin(origin: &JsonValue, c: &Path, fast: bool) -> Result<(), NativeError> {
    let mount = origin["mount"].as_str().ok_or_else(NativeError::params)?;
    // Older rows store a normalized mount; use the recorded directory spelling
    // only after proving it names that same mount.
    let recorded=Path::new(origin["directory"].as_str().ok_or_else(NativeError::params)?);
    let relative=recorded.strip_prefix(c).map_err(|_|NativeError::params())?;
    let exact_mount=format!("citadel/{}",relative.to_string_lossy());
    if crate::gameinfo::normalize_mount(&exact_mount)!=crate::gameinfo::normalize_mount(mount) {return Err(NativeError::params());}
    let dir=crate::gameinfo::mount_directory(c,&exact_mount)?;
    if origin["directoryMissing"] == true {
        if !(origin["treeComplete"]==true && origin["treeVersion"]==1 && origin["files"].is_null() && origin["directoryIdentity"].is_null()) || dir.try_exists()? {
            return Err(NativeError::new("externalChange","A previously absent addon folder appeared. Review the setup again before restoring it."));
        }
        return Ok(());
    }
    if origin["directoryIdentity"].is_object() {
        paths::reject_symlinks(&dir)?;
        let m=fs::metadata(&dir)?;
        if object!{device:m.dev(),inode:m.ino()}!=origin["directoryIdentity"] {
            return Err(NativeError::new("externalChange","Installed addon folder changed since the restore point was created."));
        }
    }
    // Re-enumerate the complete mount even on the identity path: additions,
    // removals, symlinks and replacements must still invalidate the proof.
    let unchanged = matches_mount_files(&dir,&origin["files"],fast)?;
    if dir.to_string_lossy() != origin["directory"].as_str().unwrap_or("")
        || !unchanged
    {
        return Err(NativeError::new(
            "externalChange",
            "Installed addon files changed since import; preserve them and review again.",
        ));
    }
    Ok(())
}

/// Compare a directory's recorded contents without inventing a game mount.
pub(crate) fn matches_mount_files(dir:&Path,files:&JsonValue,fast:bool)->Result<bool,NativeError>{
    Ok(if fast {
        let mut entries=Vec::new();mount_entries(dir,&mut entries,false)?;
        entries.len()==files.len() && entries.iter().zip(files.members()).all(|(entry,proof)|
            entry["sourcePath"]==proof["sourcePath"] && entry["directory"]==proof["directory"] && entry["identity"]==proof["identity"])
    } else { mount_proof(&dir)? == *files })
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use crate::{
        apply::{restore, ApplyPlan},
        local_content::{prepare_local, review_local, LocalPrepareRequest, LocalSelection},
        native::NativeService,
        prepare_control::PrepareControl,
        store::{RootLock, Store},
        test_support::{import, vpk, Fixture},
    };
    #[test]
    fn review7_other_profile_origins_do_not_block_apply() {
        let a=object!{mount:"citadel/addons",files:json::array!["a"]};
        let b=object!{mount:"citadel/addons",files:json::array!["b"]};
        let mut doc=object!{activeProfileId:"other",profiles:object!{
            selected:object!{mods:json::array![object!{source:object!{installedGameOrigins:json::array![a.clone()]}}]},
            other:object!{mods:json::array![object!{source:object!{installedGameOrigins:json::array![a.clone(),b.clone()]}}]}}};
        assert_eq!(origins_for_apply(&doc,"selected").unwrap(),vec![a]);
        assert!(origins_for_apply(&doc,"other").is_err());
        doc["applied"]=object!{profileId:"selected"};
        assert!(origins_for_apply(&doc,"other").unwrap().is_empty());
    }
    #[test]
    fn round8_recursive_mount_baseline() {
        let f=Fixture::new();let c=f.game.join("game/citadel");let dir=c.join("addons");
        fs::create_dir_all(dir.join("materials/models")).unwrap();fs::write(dir.join("materials/models/loose.vmat_c"),b"material").unwrap();fs::write(dir.join("UPPER.VPK"),b"upper").unwrap();
        let files=mount_proof(&dir).unwrap();assert!(files.members().any(|r|r["sourcePath"].as_str().unwrap().ends_with("loose.vmat_c")));assert!(files.members().any(|r|r["proofKind"]=="directory"));
        let origin=object!{treeComplete:true,treeVersion:1,mount:"citadel/addons",directory:dir.to_str(),files:files};verify_origin(&origin,&c,true).unwrap();verify_origin(&origin,&c,false).unwrap();
        fs::write(dir.join("materials/models/loose.vmat_c"),b"changed!").unwrap();assert!(verify_origin(&origin,&c,true).is_err());assert!(verify_origin(&origin,&c,false).is_err());
        std::os::unix::fs::symlink("UPPER.VPK",dir.join("link")).unwrap();assert!(mount_proof(&dir).is_err());fs::remove_file(dir.join("link")).unwrap();
        let name=std::ffi::CString::new(dir.join("fifo").to_str().unwrap()).unwrap();unsafe extern "C" {fn mkfifo(path:*const std::ffi::c_char,mode:u32)->i32;} unsafe{assert_eq!(mkfifo(name.as_ptr(),0o600),0);};assert!(mount_proof(&dir).is_err());
    }
    #[test]
    fn origin_identity_recheck_rejects_external_changes() {
        let f = Fixture::new();
        let c = f.game.join("game/citadel");
        let dir = c.join("addons");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pak01_dir.vpk");
        fs::write(&path, b"original").unwrap();
        let origin = object!{treeComplete:true,treeVersion:1,mount:"citadel/addons",directory:dir.to_string_lossy().as_ref(),files:mount_proof(&dir).unwrap()};
        verify_origin(&origin, &c, false).unwrap();
        verify_origin(&origin, &c, true).unwrap();
        let added = dir.join("extra.vpk");
        fs::write(&added, b"extra").unwrap();
        assert_eq!(verify_origin(&origin, &c, true).unwrap_err().kind, "externalChange");
        fs::remove_file(added).unwrap();
        // Replacing the inode is a change even with identical bytes and mtime.
        let old = paths::open_regular(&path).unwrap();
        let modified = old.metadata().unwrap().modified().unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"original").unwrap();
        paths::open_regular(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
        assert_eq!(verify_origin(&origin, &c, true).unwrap_err().kind, "externalChange");
        fs::remove_file(&path).unwrap();
        assert_eq!(verify_origin(&origin, &c, true).unwrap_err().kind, "externalChange");
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        assert_eq!(verify_origin(&origin, &c, true).unwrap_err().kind, "externalChange");
    }

    #[test]
    fn shards_profiles_manifest_and_owned_scan() {
        let f = Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
        let dir = f.game.join("game/citadel/addons10/profile");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pak99_dir.vpk"), vpk("models/z.vmdl_c", 12)).unwrap();
        fs::write(
            dir.join(".dmm.json"),
            r#"{"version":3,"mods":{"123":{"currentVpks":["pak99_dir.vpk"],"name":"Example"}}}"#,
        )
        .unwrap();
        let items = scan(&f.game, None).unwrap();
        assert!(items["items"]
            .members()
            .any(|i| i["suggestedMatch"]["modId"] == "123"));
        let mut service = NativeService::new();
        import(&mut service, &f);
        assert!(scan(&f.game, Some(&f.data)).unwrap()["items"]
            .members()
            .any(|i| i["alreadyInLibrary"] == true));
        // A companion cannot independently appear as an import candidate.
        fs::write(dir.join("pak99_000.vpk"), b"companion").unwrap();
        assert_eq!(
            scan(&f.game, None).unwrap()["items"].len(),
            items["items"].len()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("pak99_dir.vpk"), dir.join("pak88_dir.vpk"))
                .unwrap();
            assert!(scan(&f.game, None).is_err());
        }
    }
    #[test]
    fn grouped_manifest_and_backups() {
        let f = Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
        let dir = f.game.join("game/citadel/addons/group");
        fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["pak08_dir.vpk", "pak09_dir.vpk"].iter().enumerate() {
            fs::write(dir.join(name), vpk("models/test.vmdl_c", 12 + i as u32)).unwrap();
        }
        fs::write(dir.join(".dmm.json"),r#"{"version":3,"mods":{"667748":{"currentVpks":["pak08_dir.vpk","pak09_dir.vpk"],"originalVpkNames":["first.vpk","second.vpk"]}}}"#).unwrap();
        let backup = f
            .game
            .join("game/citadel/addons-backups/addons-backup-2026/profile");
        fs::create_dir_all(&backup).unwrap();
        fs::copy(dir.join(".dmm.json"), backup.join(".dmm.json")).unwrap();
        fs::copy(dir.join("pak08_dir.vpk"), backup.join("pak08_dir.vpk")).unwrap();
        let local = "local-64460b1c-22fc-49ba-9ee1-b12ab34eb961";
        fs::write(
            dir.join(format!("{local}_pak01_dir.vpk")),
            vpk("models/local.vmdl_c", 13),
        )
        .unwrap();
        let state = f.data.with_file_name("upstream.json");
        fs::write(&state,object!{"local-config":object!{state:object!{localMods:json::array![object!{id:local,remoteId:local,name:"Local fixture",author:"Creator",images:json::array!["data:unsafe"]},object!{remoteId:"667748",name:"Grouped fixture",author:"Author",images:json::array!["https://images.gamebanana.com/img/ss/mods/test.jpg"]}]}}}.dump()).unwrap();
        let result = scan_with_source(&f.game, None, Some(&state)).unwrap();
        let local_row = result["items"]
            .members()
            .find(|r| r["suggestedMatch"]["modId"] == local)
            .unwrap();
        assert_eq!(local_row["suggestedMatch"]["name"], "Local fixture");
        assert_eq!(local_row["suggestedMatch"]["link"], "");
        assert!(local_row["suggestedMatch"]["thumbnail"].is_null());

        let rows = result["items"]
            .members()
            .filter(|r| r["suggestedMatch"]["modId"] == "667748")
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["files"].len(), 2);
        assert_eq!(rows[0]["packages"].len(), 2);
        assert_eq!(rows[0]["suggestedMatch"]["name"], "Grouped fixture");
        assert_eq!(rows[0]["suggestedMatch"]["requiresConfirmation"], true);
        let mut service = NativeService::new();
        import(&mut service, &f);
        let prepared = prepare_local(
            LocalPrepareRequest {
                root: f.data.clone(),
                profile_id: "default".into(),
                expected_revision: 1,
                roots: rows[0]["packages"]
                    .members()
                    .map(|p| PathBuf::from(p.as_str().unwrap()))
                    .collect(),
                artwork: None,
            },
            &PrepareControl::never_cancelled(),
        )
        .unwrap();
        let plan = review_local(
            &prepared,
            LocalSelection {
                candidate_ids: prepared
                    .candidates
                    .iter()
                    .filter(|c| c.package.is_some())
                    .map(|c| c.id.clone())
                    .collect(),
                metadata: prepared.default_metadata(),
            },
        )
        .unwrap();
        assert_eq!(plan.package_refs().unwrap().len(), 2);
        assert_eq!(
            plan.package_refs().unwrap()[0].directory.source_name,
            "first.vpk"
        );
        assert_eq!(
            plan.package_refs().unwrap()[1].directory.source_name,
            "second.vpk"
        );
        let lock = RootLock::acquire(&f.data).unwrap();
        let before = Store::open_writer(&lock).unwrap().document().unwrap()["profiles"]["default"]
            ["mods"]
            .len();
        crate::local_import::commit_local(
            &plan,
            &lock,
            1,
            "grouped-import",
            &object! {},
            &mut |_| Ok(()),
        )
        .unwrap();
        let document = Store::open_writer(&lock).unwrap().document().unwrap();
        assert_eq!(document["profiles"]["default"]["mods"].len(), before + 1);

        assert!(result["items"]
            .members()
            .all(|r| !r["path"].as_str().unwrap().contains("addons-backups")));
    }
    #[test]
    fn sharded_apply_adopts_mount_owned_only_by_installed_origins() {
        for changed_after_review in [false,true] {
            let f = Fixture::new();
            let base="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n";
            fs::write(f.game.join("game/citadel/gameinfo.gi"), base).unwrap();
            let mut service = NativeService::new();
            import(&mut service, &f);
            let dir = f.game.join("game/citadel/addons4/imported");
            fs::create_dir_all(&dir).unwrap();
            let original = dir.join("pak02_dir.vpk");
            let bytes = vpk("models/installed.vmdl_c", 41);
            fs::write(&original, &bytes).unwrap();
            // The old manager's mount, absent from the import ledger.
            let before = base.replace(" Game citadel\n", " Game citadel/addons4/imported\n Game citadel\n");
            fs::write(f.game.join("game/citadel/gameinfo.gi"), &before).unwrap();
            let control = PrepareControl::never_cancelled();
            let prepared = prepare_local(LocalPrepareRequest { root: f.data.clone(), profile_id: "default".into(), expected_revision: 1, roots: vec![original.clone()], artwork: None }, &control).unwrap();
            let mut ids = prepared.candidates.iter().filter(|c| c.package.is_some()).map(|c| c.id.clone()).collect::<Vec<_>>();
            ids.sort();
            let plan = review_local(&prepared, LocalSelection { candidate_ids: ids, metadata: prepared.default_metadata() }).unwrap();
            let lock = RootLock::acquire(&f.data).unwrap();
            crate::local_import::commit_local(&plan, &lock, 1, "installed-import", &object! {}, &mut |_| Ok(())).unwrap();
            drop(lock);
            service.dispatch("native.setEnabled",&object!{dataDir:f.data.to_str().unwrap(),profileId:"default",modId:plan.mod_id.clone(),enabled:true,expectedRevision:"2",operationId:"enable-installed"}).unwrap();
            let lock = RootLock::acquire(&f.data).unwrap();
            let store = Store::open_writer(&lock).unwrap();
            let apply = ApplyPlan::review(&store,"default",&f.game).unwrap();
            let gameinfo=f.game.join("game/citadel/gameinfo.gi");
            let reviewed_gameinfo=fs::read(&gameinfo).unwrap();
            if changed_after_review {
                let held=paths::open_regular(&original).unwrap();
                let metadata=held.metadata().unwrap();
                let modified=metadata.modified().unwrap();
                let changed=vpk("models/installed.vmdl_c",42);
                assert_eq!(changed.len(),bytes.len());
                fs::write(&original,&changed).unwrap();
                held.set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
                assert_eq!(fs::metadata(&original).unwrap().modified().unwrap(),modified);
                assert_eq!(fs::metadata(&original).unwrap().len(),metadata.len());
            }
            let result=apply.apply(&store,&lock,3,"installed-apply",&object!{method:"native.apply",params:object!{operationId:"installed-apply",expectedRevision:"3",deploymentLayout:"shards-v1",token:paths::random_id().unwrap()}});
            if changed_after_review {
                assert!(result.is_err(),"Apply must reject changed origin bytes even when size and mtime match");
                assert_eq!(fs::read(&gameinfo).unwrap(),reviewed_gameinfo,"Apply must refuse before changing gameinfo.gi");
                assert_eq!(store.document().unwrap()["revision"],"3");
                assert_eq!(fs::read(&original).unwrap(),vpk("models/installed.vmdl_c",42));
                continue;
            }
            result.unwrap();
            let mounted = fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap();
            assert!(!mounted.contains("Game citadel/addons4/imported"));
            assert_eq!(fs::read(&original).unwrap(), bytes);
            restore(&store,&lock,4,"installed-restore",&object!{method:"native.restore",params:object!{operationId:"installed-restore",expectedRevision:"4"}}).unwrap();
            assert_eq!(fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap(), before);
            assert_eq!(fs::read(&original).unwrap(), bytes);
        }
    }
    #[test]
    fn local_import_adopts_prior_mount_and_restore_preserves_bytes() {
        let f = Fixture::new();
        let before="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
        fs::write(f.game.join("game/citadel/gameinfo.gi"), before).unwrap();
        let mut service = NativeService::new();
        import(&mut service, &f);
        let dir = f.game.join("game/citadel/addons4/imported");
        fs::create_dir_all(&dir).unwrap();
        let original = dir.join("pak02_dir.vpk");
        let bytes = vpk("models/installed.vmdl_c", 41);
        fs::write(&original, &bytes).unwrap();
        // Add an unowned mount that is absent from the upstream import ledger.
        let before = before.replace(
            " Game citadel\n",
            " Game citadel/addons4/imported\n Game citadel\n",
        );
        fs::write(f.game.join("game/citadel/gameinfo.gi"), &before).unwrap();
        let control = PrepareControl::never_cancelled();
        let prepared = prepare_local(
            LocalPrepareRequest {
                root: f.data.clone(),
                profile_id: "default".into(),
                expected_revision: 1,
                roots: vec![original.clone()],
                artwork: None,
            },
            &control,
        )
        .unwrap();
        assert_eq!(prepared.evidence["installedGameOrigins"].len(), 1);
        let plan = review_local(
            &prepared,
            LocalSelection {
                candidate_ids: prepared
                    .candidates
                    .iter()
                    .filter(|c| c.package.is_some())
                    .map(|c| c.id.clone())
                    .collect(),
                metadata: {
                    let mut m = prepared.default_metadata();
                    m.link = "https://gamebanana.com/mods/123".into();
                    m
                },
            },
        )
        .unwrap();
        let second=f.game.join("game/citadel/addons4/imported/pak03_dir.vpk");
        fs::write(&second,vpk("models/other.vmdl_c",42)).unwrap();
        // Re-prepare both after the mount inventory changes.
        let mut request=prepared.request.clone();request.roots=vec![original.clone(),second.clone()];
        let combined=prepare_local(request,&control).unwrap();
        let mut children=Vec::new();
        for (index,path) in [original.clone(),second].iter().enumerate() {
            let mut child=combined.clone();child.candidates.retain(|c|c.package.as_ref().is_some_and(|p|p.directory.source_path==*path));
            let mut ids=child.candidates.iter().map(|c|c.id.clone()).collect::<Vec<_>>();ids.sort();
            let child_plan=review_local(&child,LocalSelection{candidate_ids:ids,metadata:plan.selection.metadata.clone()}).unwrap();
            children.push((index.to_string(),child_plan));
        }
        let mut group=children[0].1.clone();group.group=children;
        let plan=group.group[0].1.clone();
        let lock = RootLock::acquire(&f.data).unwrap();
        crate::local_import::commit_local(
            &group,
            &lock,
            1,
            "installed-import",
            &object! {},
            &mut |_| Ok(()),
        )
        .unwrap();
        drop(lock);
        service.dispatch("native.setEnabled",&object!{dataDir:f.data.to_str().unwrap(),profileId:"default",modId:plan.mod_id.clone(),enabled:true,expectedRevision:"2",operationId:"enable-installed"}).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let doc = store.document().unwrap();
        let row = doc["profiles"]["default"]["mods"]
            .members()
            .find(|m| m["source"]["id"] == plan.mod_id)
            .unwrap();
        assert_eq!(row["source"]["nativeRemoteLink"]["version"], 2);
        assert!(row["source"]["nativeRemoteLink"]["receipt"].is_null());
        let apply = ApplyPlan::review(&store, "default", &f.game).unwrap();
        apply.apply(&store,&lock,3,"installed-apply",&object!{method:"native.apply",params:object!{operationId:"installed-apply",expectedRevision:"3",deploymentLayout:"shards-v1",token:paths::random_id().unwrap()}}).unwrap();
        let mounted = fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        assert!(!mounted.contains("Game citadel/addons4/imported"));
        assert_eq!(fs::read(&original).unwrap(), bytes);
        restore(&store,&lock,4,"installed-restore",&object!{method:"native.restore",params:object!{operationId:"installed-restore",expectedRevision:"4"}}).unwrap();
        assert_eq!(
            fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            before
        );
        assert_eq!(fs::read(&original).unwrap(), bytes);
        fs::write(&original, b"changed").unwrap();
        assert!(ApplyPlan::review(&store, "default", &f.game).is_err());
    }
}

/// Association-only v2 never supplies download/update authority or a receipt.
pub(crate) fn confirmed_link_overlay(url: &str) -> Option<JsonValue> {
    let rest = url.strip_prefix("https://gamebanana.com/")?;
    let (kind, id) = rest.split_once('/')?;
    if id.is_empty()
        || id.len() > 19
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.parse::<u64>().ok()? == 0
    {
        return None;
    }
    let kind = match kind {
        "mods" => "mod",
        "sounds" => "sound",
        _ => return None,
    };
    Some(
        object! {version:2,association:"userSelected",submission:object!{provider:"gamebanana",submissionType:kind,submissionId:id},receipt:JsonValue::Null},
    )
}
#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn association_requires_explicit_valid_gamebanana_link() {
        assert_eq!(
            confirmed_link_overlay("https://gamebanana.com/mods/123").unwrap()["submission"]
                ["submissionId"],
            "123"
        );
        assert_eq!(
            confirmed_link_overlay("https://gamebanana.com/sounds/456").unwrap()["submission"]
                ["submissionType"],
            "sound"
        );
        for url in [
            "",
            "https://gamebanana.com.evil/mods/123",
            "https://gamebanana.com/mods/123?guess=true",
            "https://gamebanana.com/mods/0",
        ] {
            assert!(confirmed_link_overlay(url).is_none())
        }
    }
}

pub(crate) fn initial_game_path(
    before: &JsonValue,
    record: &JsonValue,
) -> Result<Option<String>, NativeError> {
    if before["gamePath"] != "" {
        return Ok(None);
    }
    let mut game = None;
    for origin in record["source"]["installedGameOrigins"].members() {
        let p = origin["gamePath"]
            .as_str()
            .ok_or_else(NativeError::params)?;
        if !Path::new(p).is_absolute() || game.as_ref().is_some_and(|old| old != p) {
            return Err(NativeError::params());
        }
        game = Some(p.to_owned());
    }
    Ok(game)
}

#[cfg(test)]
mod bootstrap_tests {
    use super::*;
    use crate::{
        local_content::*,
        native::NativeService,
        prepare_control::PrepareControl,
        store::RootLock,
        test_support::{vpk, Fixture},
    };
    #[test]
    fn installed_import_binds_fresh_library_with_reviewed_origin() {
        let f = Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
        let mut service = NativeService::new();
        service.dispatch("native.library.initialize",&object!{version:1,dataDir:f.data.to_str().unwrap(),operationId:paths::random_id().unwrap(),expectedRevision:"0"}).unwrap();
        let path = f.game.join("game/citadel/addons/pak55_dir.vpk");
        fs::write(&path, vpk("models/new.vmdl_c", 31)).unwrap();
        let store = Store::open(&f.data).unwrap();
        let revision =
            crate::store::revision(store.document().unwrap()["revision"].as_str().unwrap())
                .unwrap();
        drop(store);
        let prepared = prepare_local(
            LocalPrepareRequest {
                root: f.data.clone(),
                profile_id: "default".into(),
                expected_revision: revision,
                roots: vec![path],
                artwork: None,
            },
            &PrepareControl::new(),
        )
        .unwrap();
        let plan = review_local(
            &prepared,
            LocalSelection {
                candidate_ids: prepared
                    .candidates
                    .iter()
                    .filter(|c| c.package.is_some())
                    .map(|c| c.id.clone())
                    .collect(),
                metadata: prepared.default_metadata(),
            },
        )
        .unwrap();
        assert_eq!(
            plan.summary["installedGameOrigins"][0]["gamePath"],
            f.game.to_str().unwrap()
        );
        let lock = RootLock::acquire(&f.data).unwrap();
        crate::local_import::commit_local(
            &plan,
            &lock,
            revision,
            "installed-fresh",
            &object! {},
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            Store::open_writer(&lock).unwrap().document().unwrap()["gamePath"],
            f.game.to_str().unwrap()
        );
    }
    #[test]
    fn required_companion_is_grouped_and_missing_member_reported() {
        use crate::test_support::{family_vpk, FamilyEntry};
        let f = Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
        let dir = f.game.join("game/citadel/addons3");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pak01_dir.vpk"),
            family_vpk(
                1,
                &[FamilyEntry {
                    path: "models/a.vmdl_c",
                    crc: 1,
                    archive: 0,
                    offset: 0,
                    length: 4,
                    preload: b"",
                }],
                b"",
            ),
        )
        .unwrap();
        let row = scan(&f.game, None).unwrap()["items"]
            .members()
            .find(|x| x["path"].as_str().unwrap().contains("addons3"))
            .unwrap()
            .clone();
        assert!(!row["error"].is_null());
        fs::write(dir.join("pak01_000.vpk"), b"test").unwrap();
        let row = scan(&f.game, None).unwrap()["items"]
            .members()
            .find(|x| x["path"].as_str().unwrap().contains("addons3"))
            .unwrap()
            .clone();
        assert!(row["error"].is_null());
        assert_eq!(row["files"].len(), 2);
    }
}
