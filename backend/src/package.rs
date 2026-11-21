//! Logical VPK packages. Ownership comes only from explicit manifest associations.
use crate::{native::NativeError, paths, prepare_control::PrepareControl, store, vpk};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRef {
    pub id: String,
    pub sha256: String,
    pub size: u64,
    pub source_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRef {
    pub directory: FileRef,
    pub companions: BTreeMap<u16, FileRef>,
}
#[derive(Clone, Debug)]
pub struct SourceAssociation {
    pub original_name: String,
    pub source_path: PathBuf,
}
#[derive(Clone, Debug)]
pub struct SourcePackage {
    pub directory: SourceAssociation,
    pub companions: BTreeMap<u16, SourceAssociation>,
}

fn invalid() -> NativeError {
    NativeError::new(
        "invalidPackage",
        "VPK package references or exact manifest associations are incomplete or inconsistent.",
    )
}
pub(crate) fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn decode_file(v: &JsonValue) -> Result<FileRef, NativeError> {
    if !v.is_object()
        || ["companions", "packages", "directory", "archiveIndex"]
            .iter()
            .any(|k| v.has_key(k))
    {
        return Err(invalid());
    }
    let id = v["id"]
        .as_str()
        .filter(|s| hex(s, 32))
        .ok_or_else(invalid)?;
    let sha = v["sha256"]
        .as_str()
        .filter(|s| hex(s, 64))
        .ok_or_else(invalid)?;
    let size = v["size"].as_u64().ok_or_else(invalid)?;
    let name = v["sourceName"]
        .as_str()
        .filter(|s| paths::safe_name(s))
        .ok_or_else(invalid)?;
    Ok(FileRef {
        id: id.into(),
        sha256: sha.into(),
        size,
        source_name: name.into(),
    })
}
pub(crate) fn encode_file(f: &FileRef) -> JsonValue {
    object! {id:f.id.clone(),sha256:f.sha256.clone(),size:f.size,sourceName:f.source_name.clone()}
}
pub fn mod_packages(record: &JsonValue, version: u32) -> Result<Vec<PackageRef>, NativeError> {
    if record.has_key("options") {
        return crate::options::selected_packages(record, version);
    }
    legacy_packages(record, version)
}
/// Exact bindings returned by the selected deployment view for intent proof.
/// Catalog selection already coalesces content aliases; retain its chosen refs.
pub fn selected_packages_exact(record: &JsonValue, layout: u32) -> Result<Vec<PackageRef>, NativeError> {
    let empty = || NativeError::new("invalidLocalSelection", "Choose a nonempty set of ready local options before Apply; prepare unavailable choices first.");
    if record.has_key("options") {
        let catalog = crate::options::for_record(record, layout)?;
        catalog.validate_selection_save().map_err(|_| empty())?;
        return catalog.packages(true);
    }
    if record["source"]["kind"] == "local"
        && record["source"]["localImport"]["selectedCandidateIds"].is_array()
        && record["source"]["localImport"]["selectedCandidateIds"].is_empty() {
        return Err(empty());
    }
    let packages = legacy_packages(record, layout)?;
    if packages.is_empty() {return Err(empty());}
    Ok(packages)
}
/// Immutable generation decoders and untouched records use this exact legacy view.
pub(crate) fn legacy_packages(record: &JsonValue, version: u32) -> Result<Vec<PackageRef>, NativeError> {
    let mut result = Vec::new();
    match version {
        1 => {
            if record.has_key("packages") || !record["payloads"].is_array() {
                return Err(invalid());
            }
            for f in record["payloads"].members() {
                result.push(PackageRef {
                    directory: decode_file(f)?,
                    companions: BTreeMap::new(),
                });
            }
        }
        2 => {
            if record.has_key("payloads") || !record["packages"].is_array() {
                return Err(invalid());
            }
            for p in record["packages"].members() {
                if !p.is_object() || !p["companions"].is_array() {
                    return Err(invalid());
                }
                let directory = decode_file(&p["directory"])?;
                let mut companions = BTreeMap::new();
                for c in p["companions"].members() {
                    let i = c["archiveIndex"]
                        .as_u16()
                        .filter(|i| *i != 0x7fff)
                        .ok_or_else(invalid)?;
                    if companions.insert(i, decode_file(&c["file"])?).is_some() {
                        return Err(invalid());
                    }
                }
                result.push(PackageRef {
                    directory,
                    companions,
                });
            }
        }
        _ => {
            return Err(NativeError::new(
                "unsupportedVersion",
                "Unsupported native library version.",
            ))
        }
    }
    let mut ids = HashMap::new();
    for p in &result {
        validate_refs(p, &mut ids)?;
    }
    Ok(result)
}
pub(crate) fn validate_refs(
    p: &PackageRef,
    ids: &mut HashMap<String, (String, u64)>,
) -> Result<(), NativeError> {
    if p.companions.contains_key(&0x7fff) {
        return Err(invalid());
    }
    for f in std::iter::once(&p.directory).chain(p.companions.values()) {
        decode_file(&encode_file(f))?;
        let value = (f.sha256.clone(), f.size);
        if ids
            .insert(f.id.clone(), value.clone())
            .is_some_and(|old| old != value)
        {
            return Err(invalid());
        }
    }
    Ok(())
}
pub fn encode_packages(packages: &[PackageRef]) -> JsonValue {
    JsonValue::from(packages.iter().map(|p|object!{directory:encode_file(&p.directory),companions:JsonValue::from(p.companions.iter().map(|(i,f)|object!{archiveIndex:*i,file:encode_file(f)}).collect::<Vec<_>>())}).collect::<Vec<_>>())
}
pub fn family_key(p: &PackageRef) -> Result<String, NativeError> {
    validate_refs(p, &mut HashMap::new())?;
    let v = json::array![
        p.directory.sha256.clone(),
        p.directory.size,
        JsonValue::from(
            p.companions
                .iter()
                .map(|(i, f)| json::array![*i, f.sha256.clone(), f.size])
                .collect::<Vec<_>>()
        )
    ];
    paths::digest_bytes(store::canonical(&v).as_bytes())
}
pub(crate) fn document_version(doc: &JsonValue) -> Result<u32, NativeError> {
    let version = doc["storeVersion"]
        .as_u32()
        .filter(|v| matches!(v, 1 | 2 | 3 | 4))
        .ok_or_else(|| {
            NativeError::new("unsupportedVersion", "Unsupported native library version.")
        })?;
    let layout = layout_version(doc)?;
    if version >= 3 { crate::fonts::validate_state(doc)?; }
    let mut ids = HashMap::new();
    for (_, profile) in doc["profiles"].entries() {
        for record in profile["mods"].members() {
            if version < 3 && !crate::fonts::stored_refs(record)?.is_empty() { return Err(invalid()); }
            if version < 4 && record.has_key("options") { return Err(invalid()); }
            for p in crate::options::retained_packages(record, layout)? {
                validate_refs(&p, &mut ids)?;
            }
        }
    }
    Ok(version)
}

/// Library barriers never change immutable package/generation layout.
pub fn layout_version(doc: &JsonValue) -> Result<u32, NativeError> {
    match doc["storeVersion"].as_u32() {
        Some(v @ (1 | 2)) if doc["packageVersion"].is_null() => Ok(v),
        Some(3 | 4) => doc["packageVersion"].as_u32().filter(|v|matches!(v,1|2)).ok_or_else(invalid),
        _ => Err(NativeError::new("unsupportedVersion","Unsupported native library or package layout version.")),
    }
}

/// Store4 writes require the new outer journal formats, delivered separately.
/// Call only after durable operation replay and before acquiring game resources.
pub(crate) fn require_legacy_writer(doc: &JsonValue) -> Result<(), NativeError> {
    if document_version(doc)? == 4 {
        return Err(NativeError::new("unsupportedVersion", "This helper can read Store4 choices, but this writer requires the Store4 transaction implementation."));
    }
    Ok(())
}

pub fn resolve_source_packages(
    associations: &[SourceAssociation],
) -> Result<Vec<SourcePackage>, NativeError> {
    resolve_source_packages_controlled(associations, &PrepareControl::never_cancelled())
}
pub fn resolve_source_packages_controlled(
    associations: &[SourceAssociation],
    control: &PrepareControl,
) -> Result<Vec<SourcePackage>, NativeError> {
    control.check()?;
    if associations.is_empty() || associations.len() > 100_000 {
        return Err(invalid());
    }
    let mut names = HashMap::new();
    let mut paths_seen = HashSet::new();
    for (i, a) in associations.iter().enumerate() {
        control.check()?;
        if !paths::safe_name(&a.original_name)
            || !a.original_name.to_ascii_lowercase().ends_with(".vpk")
            || names
                .insert(a.original_name.to_ascii_lowercase(), i)
                .is_some()
            || !paths_seen.insert(a.source_path.clone())
        {
            return Err(invalid());
        }
    }
    let mut claimed = HashSet::new();
    let mut roots = HashSet::new();
    let mut result = Vec::new();
    // Logical _dir candidates establish claims first, before an opaque member can
    // be mistaken for an independent directory simply because it has VPK bytes.
    let mut order: Vec<_> = associations.iter().enumerate().collect();
    order.sort_by_key(|(i, a)| {
        (
            !a.original_name.to_ascii_lowercase().ends_with("_dir.vpk"),
            a.original_name.len(),
            *i,
        )
    });
    for (i, a) in order {
        control.check()?;
        if claimed.contains(&i) {
            continue;
        }
        let directory = vpk::inspect_directory_controlled(&a.source_path, control)?;
        let required = vpk::required_archives(&directory);
        let mut companions = BTreeMap::new();
        let mut member_paths = BTreeMap::new();
        for index in required {
            control.check()?;
            let stem = &a.original_name[..a.original_name.len() - 4];
            let stem = stem.strip_suffix("_dir").unwrap_or(stem);
            let name = format!("{stem}_{index:03}.vpk");
            let member = *names.get(&name.to_ascii_lowercase()).ok_or_else(invalid)?;
            // Case-insensitive lookup detects ambiguity but never invents a spelling.
            if associations[member].original_name != name
                || member == i
                || roots.contains(&member)
                || !claimed.insert(member)
            {
                return Err(invalid());
            }
            companions.insert(index, associations[member].clone());
            member_paths.insert(index, associations[member].source_path.clone());
        }
        vpk::validate_family_controlled(&directory, &member_paths, control)?;
        roots.insert(i);
        result.push((
            i,
            SourcePackage {
                directory: a.clone(),
                companions,
            },
        ));
    }
    if roots.len() + claimed.len() != associations.len() {
        return Err(invalid());
    }
    result.sort_by_key(|(i, _)| *i);
    Ok(result.into_iter().map(|(_, p)| p).collect())
}
