//! Validated generation names and complete physical-family verification.
use crate::{
    native::NativeError,
    package::{self, FileRef, PackageRef},
    paths, vpk,
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug)]
pub struct GenerationFile {
    pub name: String,
    pub payload: FileRef,
}
#[derive(Clone, Debug)]
pub struct GenerationFamily {
    pub package: PackageRef,
    pub mod_id: String,
    pub mod_ids: Vec<String>,
    /// None for historical flat generations; otherwise 1..=10.
    pub shard_index: Option<u8>,
    pub slot: u8,
}
#[derive(Clone, Debug)]
pub struct GenerationDirectory {
    /// Empty string denotes the generation root. Children are immediate names.
    pub relative_path: String,
    pub children: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct GenerationView {
    pub version: u32,
    pub package_version: u32,
    pub generation: String,
    pub store_root: PathBuf,
    pub families: Vec<GenerationFamily>,
    /// Payload files only; manifest.json is included in root membership.
    pub files: Vec<GenerationFile>,
    /// Root first, followed by shard directories in mount order.
    pub directories: Vec<GenerationDirectory>,
    pub mounts: Vec<String>,
}
fn exact_keys(v: &JsonValue, fields: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len()!=fields.len() || fields.iter().any(|k|!v.has_key(k)) { return Err(invalid()); }
    Ok(())
}
/// Historical transaction domains are fixed even if a pure reader learns new
/// layouts. Call for current, before, after and requested replacement documents.
pub(crate) fn require_flat_document(doc: &JsonValue) -> Result<(), NativeError> {
    if !doc["retiredGenerations"].is_null() && !doc["retiredGenerations"].is_array() { return Err(invalid()); }
    for a in std::iter::once(&doc["applied"]).chain(std::iter::once(&doc["previous"]["applied"]))
        .chain(doc["retiredGenerations"].members()) {
        if !a.is_null() && !matches!(a["manifest"]["version"].as_u32(),Some(1|2)) {
            return Err(NativeError::new("unsupportedVersion", "This transaction requires flat generations; shard transaction support is not available in this writer."));
        }
    }
    Ok(())
}
/// Pure immutable view. This grants no filesystem capture, staging or deletion
/// authority: historical filesystem helpers below deliberately remain flat-only.
pub fn decode(manifest: &JsonValue) -> Result<GenerationView, NativeError> {
    let version = manifest["version"].as_u32().ok_or_else(invalid)?;
    let mut families = Vec::new();
    let package_version;
    if version == 3 {
        crate::store::ensure_internal(manifest)?;
        exact_keys(manifest, if manifest.has_key("repairs") {&["version","packageVersion","storeRoot","generation","shards","repairs"]} else {&["version","packageVersion","storeRoot","generation","shards"]})?;
        package_version = manifest["packageVersion"].as_u32().filter(|v|matches!(v,1|2)).ok_or_else(invalid)?;
        let shards = &manifest["shards"];
        if !shards.is_array() || shards.is_empty() || shards.len()>crate::deployment_layout::MAX_SHARDS { return Err(invalid()); }
        let mut refs = HashMap::new();
        let mut content = HashSet::new();
        for (i, shard) in shards.members().enumerate() {
            exact_keys(shard, &["index","packages"])?;
            let rows = &shard["packages"];
            if shard["index"].as_usize()!=Some(i+1) || !rows.is_array() || rows.len()>crate::deployment_layout::PER_SHARD_LIMIT
                || rows.is_empty() && shards.len()!=1 { return Err(invalid()); }
            for (slot, row) in rows.members().enumerate() {
                exact_keys(row, &["directory","companions","modId","modIds"])?;
                exact_keys(&row["directory"], &["id","sha256","size","sourceName"])?;
                if !row["companions"].is_array() { return Err(invalid()); }
                let mut previous_index = None;
                for c in row["companions"].members() {
                    exact_keys(c, &["archiveIndex","file"])?;
                    exact_keys(&c["file"], &["id","sha256","size","sourceName"])?;
                    let index = c["archiveIndex"].as_u16().filter(|i|*i!=0x7fff).ok_or_else(invalid)?;
                    if previous_index.is_some_and(|old|old>=index) { return Err(invalid()); }
                    previous_index = Some(index);
                }
                let package = package::legacy_packages(&object!{packages:json::array![row.clone()]},2)?.pop().ok_or_else(invalid)?;
                if package_version==1 && !package.companions.is_empty() { return Err(invalid()); }
                package::validate_refs(&package,&mut refs)?;
                if !content.insert(package::family_key(&package)?) { return Err(invalid()); }
                let mod_id = row["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(invalid)?.to_owned();
                if !row["modIds"].is_array() || row["modIds"].is_empty() || row["modIds"][0]!=mod_id { return Err(invalid()); }
                let mut owners = HashSet::new();
                let mod_ids = row["modIds"].members().map(|id| {
                    let id=id.as_str().filter(|s|paths::safe_id(s)).ok_or_else(invalid)?;
                    if !owners.insert(id) { return Err(invalid()); }
                    Ok(id.to_owned())
                }).collect::<Result<Vec<_>,NativeError>>()?;
                families.push(GenerationFamily {package, mod_id, mod_ids, shard_index:Some((i+1) as u8), slot:(slot+1) as u8});
            }
        }
    } else {
        // Preserve the historical decoder and its flat99 admission verbatim.
        let legacy = packages(manifest)?;
        package_version = version;
        let rows = &manifest[if version==1 {"files"} else {"packages"}];
        for (i, package) in legacy.into_iter().enumerate() {
            families.push(GenerationFamily {package, mod_id:rows[i]["modId"].as_str().unwrap_or_default().into(),
                mod_ids:rows[i]["modIds"].members().filter_map(|v|v.as_str().map(str::to_owned)).collect(), shard_index:None, slot:(i+1) as u8});
        }
    }
    let generation = manifest["generation"].as_str().filter(|s|package::hex(s,32)).ok_or_else(invalid)?.to_owned();
    let store_root = PathBuf::from(manifest["storeRoot"].as_str().filter(|s|Path::new(s).is_absolute() && !s.contains('\0')).ok_or_else(invalid)?);
    let base = format!("citadel/deadlock_native/{generation}");
    let mut directories = vec![GenerationDirectory {relative_path:String::new(), children:vec!["manifest.json".into()]}];
    let mut mounts:Vec<String> = if version==3 {
        (1..=manifest["shards"].len()).map(|i| {
            let name=format!("shard{i:02}");
            directories[0].children.push(name.clone());
            directories.push(GenerationDirectory {relative_path:name.clone(), children:Vec::new()});
            format!("{base}/{name}")
        }).collect()
    } else {vec![base]};
    let mut files=Vec::new();
    for family in &families {
        let dir_index=family.shard_index.unwrap_or(0) as usize;
        let directory=&mut directories[dir_index];
        let mut push_file=|name:String,payload:&FileRef| {
            directory.children.push(name.clone());
            files.push(GenerationFile {name:if directory.relative_path.is_empty(){name}else{format!("{}/{name}",directory.relative_path)},payload:payload.clone()});
        };
        push_file(format!("pak{:02}_dir.vpk",family.slot),&family.package.directory);
        for (index,file) in &family.package.companions {push_file(format!("pak{:02}_{index:03}.vpk",family.slot),file);}
    }
    if manifest.has_key("repairs") {
        let r=&manifest["repairs"];
        if version!=3 || !r.is_object() || !r["choices"].is_object() || !r["key"].as_str().is_some_and(|s|package::hex(s,64)) {return Err(invalid());}
        if !r["file"].is_null(){
            let payload=package::decode_file(&r["file"])?;
            directories[0].children.push("repairs".into());
            directories.push(GenerationDirectory{relative_path:"repairs".into(),children:vec!["pak01_dir.vpk".into()]});
            files.push(GenerationFile{name:"repairs/pak01_dir.vpk".into(),payload});
            mounts.insert(0,format!("citadel/deadlock_native/{generation}/repairs"));
        }
    }
    Ok(GenerationView {version,package_version,generation,store_root,families,files,directories,mounts})
}
pub fn mounts(manifest: &JsonValue) -> Result<Vec<String>, NativeError> { Ok(decode(manifest)?.mounts) }

/// Exact metadata key sets for future generation3 checkpoint handlers. Root
/// identity is separate; shard/file maps use only paths derived by decode.
/// Metadata equality alone is never filesystem ownership or cleanup authority.
pub fn validate_tree_identities(view: &GenerationView, root: &JsonValue, directories: &JsonValue, files: &JsonValue) -> Result<(),NativeError> {
    fn identity_shape(v:&JsonValue)->Result<(),NativeError> {
        let mut base=v.clone();base.remove("fsid");
        exact_keys(&base,&["size","device","inode","modifiedSeconds","modifiedNanos"])?;
        if !valid_fsid(v) {return Err(invalid());}
        if v["size"].as_u64().is_none() || v["device"].as_u64().is_none() || v["inode"].as_u64().is_none()
            || v["modifiedSeconds"].as_i64().is_none() || !v["modifiedNanos"].as_i64().is_some_and(|n|(0..1_000_000_000).contains(&n)) { return Err(invalid()); }
        Ok(())
    }
    if view.version!=3 || !directories.is_object() || Some(directories.len())!=view.directories.len().checked_sub(1)
        || !files.is_object() || files.len()!=view.files.len()+1 { return Err(invalid()); }
    identity_shape(root)?;
    for directory in view.directories.iter().skip(1) {identity_shape(&directories[&directory.relative_path])?;}
    identity_shape(&files["manifest.json"])?;
    for file in &view.files {
        identity_shape(&files[&file.name])?;
        if files[&file.name]["size"].as_u64()!=Some(file.payload.size) {return Err(invalid());}
    }
    Ok(())
}
fn invalid() -> NativeError {
    NativeError::new(
        "externalChange",
        "Generation files or ownership evidence differ from the verified setup.",
    )
}
/// Frozen flat-only reader used by historical transaction/filesystem handlers.
/// New layout consumers use decode and its explicit package_version instead.
pub(crate) fn packages(manifest: &JsonValue) -> Result<Vec<PackageRef>, NativeError> {
    let version = manifest["version"].as_u32().ok_or_else(invalid)?;
    if !manifest["generation"]
        .as_str()
        .is_some_and(|s| package::hex(s, 32))
        || !manifest["storeRoot"]
            .as_str()
            .is_some_and(|s| Path::new(s).is_absolute())
    {
        return Err(invalid());
    }
    let records = match version {
        1 if !manifest.has_key("packages") => object! {payloads:manifest["files"].clone()},
        2 if !manifest.has_key("files") => object! {packages:manifest["packages"].clone()},
        _ => return Err(invalid()),
    };
    for entry in manifest["packages"].members() {
        let id = entry["modId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(invalid)?;
        if !entry["modIds"].is_array() || !entry["modIds"].members().any(|v| v == id) {
            return Err(invalid());
        }
        let mut ids = HashSet::new();
        for value in entry["modIds"].members() {
            let id = value
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(invalid)?;
            if !ids.insert(id) {
                return Err(invalid());
            }
        }
    }
    let p = package::legacy_packages(&records, version)?;
    if p.len() > 99 {
        return Err(invalid());
    }
    Ok(p)
}
pub fn generation_files(manifest: &JsonValue) -> Result<Vec<GenerationFile>, NativeError> {
    let mut files = Vec::new();
    for (i, p) in packages(manifest)?.into_iter().enumerate() {
        files.push(GenerationFile {
            name: format!("pak{:02}_dir.vpk", i + 1),
            payload: p.directory,
        });
        for (index, file) in p.companions {
            files.push(GenerationFile {
                name: format!("pak{:02}_{index:03}.vpk", i + 1),
                payload: file,
            });
        }
    }
    Ok(files)
}
pub(crate) fn identity(m: &fs::Metadata) -> JsonValue {
    object! {size:m.len(),device:m.dev(),inode:m.ino(),modifiedSeconds:m.mtime(),modifiedNanos:m.mtime_nsec()}
}
pub(crate) fn same_owner(m: &fs::Metadata, id: &JsonValue) -> bool {
    id["device"].as_u64() == Some(m.dev()) && id["inode"].as_u64() == Some(m.ino())
}
pub(crate) fn capture_identity(path:&Path)->Result<JsonValue,NativeError>{
    paths::reject_symlinks(path)?;
    let file=fs::File::open(path)?;let mut id=identity(&file.metadata()?);
    id["fsid"]=paths::persisted_filesystem_id(&file)?.into();Ok(id)
}
pub(crate) fn valid_fsid(id:&JsonValue)->bool {
    !id.has_key("fsid") || id["fsid"].as_str().is_some_and(|s|package::hex(s,16))
}
pub(crate) fn owner_matches(path:&Path,id:&JsonValue,authority:Option<&crate::game_access::GameLock>)->Result<bool,NativeError>{
    if !valid_fsid(id) {return Ok(false);}
    if let Some(guard)=authority {return persisted_owner(guard,path,id);}
    paths::reject_symlinks(path)?;
    let file=fs::File::open(path)?;let m=file.metadata()?;
    Ok(id["inode"].as_u64()==Some(m.ino()) && if let Some(fsid)=id["fsid"].as_str(){
        paths::persisted_filesystem_id(&file)?==fsid
    }else{id["device"].as_u64()==Some(m.dev())})
}
pub(crate) fn metadata_matches(path:&Path,id:&JsonValue,authority:Option<&crate::game_access::GameLock>)->Result<bool,NativeError>{
    if !owner_matches(path,id,authority)? {return Ok(false);}
    let m=fs::symlink_metadata(path)?;let mut checked=id.clone();
    checked["device"]=m.dev().into();checked.remove("fsid");
    Ok(identity(&m)==checked)
}

/// Durable ownership needs a path and the lock covering it. Live review
/// evidence continues to use `same_owner` and the complete metadata identity.
pub(crate) fn persisted_owner(lock: &crate::game_access::GameLock, path: &Path, id: &JsonValue) -> Result<bool, NativeError> {
    lock.same_persisted_object(path, id)
}

#[cfg(test)]
#[test]
fn item93_generation_device_change_is_bound_to_game_authority() {
    let root = std::env::temp_dir().join(format!("generation-device-{}", paths::random_id().unwrap()));
    fs::create_dir(&root).unwrap();
    let path=root.join("payload"); fs::write(&path,b"payload").unwrap();
    let lock=crate::game_access::GameLock::acquire(&root).unwrap();
    let file=paths::open_regular(&path).unwrap();
    let mut old=identity(&file.metadata().unwrap());
    old["device"]=(file.metadata().unwrap().dev()+1).into();
    if paths::test_volatile_filesystem(&file) {
        assert!(persisted_owner(&lock,&path,&old).unwrap());
    }
    old["fsid"]=paths::persisted_filesystem_id(&file).unwrap().into();
    assert!(persisted_owner(&lock,&path,&old).unwrap());
    assert!(!same_owner(&file.metadata().unwrap(),&old), "live evidence remains strict");
    old["inode"]=(file.metadata().unwrap().ino()+1).into();
    assert!(!persisted_owner(&lock,&path,&old).unwrap());
    old["inode"]=file.metadata().unwrap().ino().into();old["fsid"]="0000000000000000".into();
    assert!(!persisted_owner(&lock,&path,&old).unwrap());
    let outside=root.with_extension("outside");fs::write(&outside,b"foreign").unwrap();
    let outside_file=paths::open_regular(&outside).unwrap();
    let mut outside_id=identity(&outside_file.metadata().unwrap());
    outside_id["fsid"]=paths::persisted_filesystem_id(&outside_file).unwrap().into();
    assert!(!persisted_owner(&lock,&outside,&outside_id).unwrap(), "authority is path bounded even with fsid");
    fs::remove_file(outside).unwrap();
    let moved=root.with_extension("moved");fs::rename(&root,&moved).unwrap();fs::create_dir(&root).unwrap();
    assert!(persisted_owner(&lock,&moved.join("payload"),&old).is_err(), "renamed lock root loses authority");
    drop(lock);fs::remove_dir_all(root).unwrap();fs::remove_dir_all(moved).unwrap();
}

pub(crate) fn verify_file(path: &Path, file: &FileRef) -> Result<(), NativeError> {
    let fp = paths::fingerprint(path)?;
    if fp.sha256 != file.sha256 || fp.size != file.size {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn verify_package(
    p: &PackageRef,
    version: u32,
    path: impl Fn(&FileRef) -> Result<PathBuf, NativeError>,
) -> Result<Vec<vpk::VpkEntry>, NativeError> {
    Ok(verify_package_evidence(p, version, path)?.0)
}
pub(crate) fn verify_package_evidence(
    p: &PackageRef,
    version: u32,
    path: impl Fn(&FileRef) -> Result<PathBuf, NativeError>,
) -> Result<(Vec<vpk::VpkEntry>, Vec<(PathBuf, paths::Fingerprint)>), NativeError> {
    verify_package_evidence_with_review(p, version, path, &HashMap::new())
}

/// An in-memory proof from a hash performed during this review only.
pub(crate) struct ReviewHash {
    pub fingerprint: paths::Fingerprint,
    change: (i64, i64),
}
impl ReviewHash {
    pub fn capture(path: &Path) -> Result<Self, NativeError> {
        let mut file = paths::open_regular(path)?;
        let before = file.metadata()?;
        let fingerprint = paths::fingerprint_file(&mut file)?;
        let after = file.metadata()?;
        if (before.ctime(), before.ctime_nsec()) != (after.ctime(), after.ctime_nsec()) {
            return Err(invalid());
        }
        Ok(Self { fingerprint, change: (after.ctime(), after.ctime_nsec()) })
    }
    fn matches(&self, m: &fs::Metadata) -> bool {
        let f = &self.fingerprint;
        m.len() == f.size && m.dev() == f.device && m.ino() == f.inode
            && m.mtime() == f.modified_seconds && m.mtime_nsec() == f.modified_nanos
            && (m.ctime(), m.ctime_nsec()) == self.change
    }
}
pub(crate) fn verify_package_evidence_with_review(
    p: &PackageRef,
    version: u32,
    path: impl Fn(&FileRef) -> Result<PathBuf, NativeError>,
    reviewed: &HashMap<PathBuf, ReviewHash>,
) -> Result<(Vec<vpk::VpkEntry>, Vec<(PathBuf, paths::Fingerprint)>), NativeError> {
    // This is a package layout, never the library schema barrier.
    if !matches!(version,1|2) {return Err(invalid());}
    let before = std::iter::once(&p.directory)
        .chain(p.companions.values())
        .map(|f| {
            let path = path(f)?;
            let proof = reviewed.get(&path);
            let reuse = match proof {
                Some(proof) => proof.matches(&paths::open_regular(&path)?.metadata()?),
                None => false,
            };
            let fp = if reuse { proof.unwrap().fingerprint.clone() } else { paths::fingerprint(&path)? };
            if fp.sha256 != f.sha256 || fp.size != f.size {
                return Err(invalid());
            }
            Ok((fp, reuse))
        })
        .collect::<Result<Vec<_>, NativeError>>()?;
    let directory = path(&p.directory)?;
    let entries = if version == 1 {
        vpk::inspect(&directory)?
    } else {
        let parsed = vpk::inspect_directory(&directory)?;
        let members = p
            .companions
            .iter()
            .map(|(i, f)| Ok((*i, path(f)?)))
            .collect::<Result<BTreeMap<_, _>, NativeError>>()?;
        vpk::validate_family(&parsed, &members)?;
        parsed.resources.into_iter().map(|r| r.entry).collect()
    };
    let mut evidence = Vec::new();
    for (file, (fp, reused)) in std::iter::once(&p.directory).chain(p.companions.values()).zip(before) {
        let path = path(file)?;
        let m = paths::open_regular(&path)?.metadata()?;
        if m.len() != fp.size || m.dev() != fp.device || m.ino() != fp.inode
            || m.mtime() != fp.modified_seconds || m.mtime_nsec() != fp.modified_nanos
            || reused && !reviewed.get(&path).is_some_and(|proof| proof.matches(&m)) {
            return Err(invalid());
        }
        evidence.push((path, fp));
    }
    Ok((entries, evidence))
}

pub fn capture_file_identities(
    directory: &Path,
    manifest: &JsonValue,
) -> Result<JsonValue, NativeError> {
    let mut ids = JsonValue::new_object();
    for f in generation_files(manifest)? {
        ids[&f.name] = capture_identity(&directory.join(&f.name))?;
    }
    ids["manifest.json"] =
        capture_identity(&directory.join("manifest.json"))?;
    Ok(ids)
}
pub fn verify_generation_files(
    directory: &Path,
    manifest: &JsonValue,
    identities: &JsonValue,
    fast: bool,
) -> Result<(), NativeError> {
    paths::reject_symlinks(directory)?;
    let files = generation_files(manifest)?;
    let mut expected = HashSet::from(["manifest.json".to_owned()]);
    let mut unchanged = true;
    for f in &files {
        let path = directory.join(&f.name);
        let m = paths::open_regular(&path)?.metadata()?;
        let recorded = &identities[&f.name];
        if recorded.is_object() && !owner_matches(&path,recorded,None)? {
            return Err(invalid());
        }
        let cached = fast && recorded.is_object() && metadata_matches(&path,recorded,None)?;
        if !cached {
            verify_file(&path, &f.payload)?;
            unchanged = false;
        }
        expected.insert(f.name.clone());
    }
    let marker = directory.join("manifest.json");
    let m = paths::open_regular(&marker)?.metadata()?;
    if identities["manifest.json"].is_object() && !owner_matches(&marker,&identities["manifest.json"],None)? {
        return Err(invalid());
    }
    let bytes = paths::read_bounded(&marker, paths::MAX_JSON_BYTES)?;
    let value =
        json::parse(std::str::from_utf8(&bytes).map_err(|_| invalid())?).map_err(|_| invalid())?;
    if value != *manifest {
        return Err(invalid());
    }
    for e in fs::read_dir(directory)? {
        if !expected.remove(&e?.file_name().to_string_lossy().into_owned()) {
            return Err(invalid());
        }
    }
    if !expected.is_empty() {
        return Err(invalid());
    }
    if !unchanged {
        let mut members_by_name = files.iter();
        for p in packages(manifest)? {
            let path = directory.join(&members_by_name.next().ok_or_else(invalid)?.name);
            if manifest["version"] == 1 {
                vpk::inspect(&path)?;
            } else {
                let parsed = vpk::inspect_directory(&path)?;
                let mut members = BTreeMap::new();
                for index in p.companions.keys() {
                    members.insert(
                        *index,
                        directory.join(&members_by_name.next().ok_or_else(invalid)?.name),
                    );
                }
                vpk::validate_family(&parsed, &members)?;
            }
        }
    }
    Ok(())
}
