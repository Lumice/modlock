//! Immutable bundled font references. History coordinates never grant file authority.
use crate::{
    font_config,
    font_content::{
        FontCandidate, FontContainer, FontOrigin, VpkFontMemberOrigin, MAX_FONT_BYTES,
        MAX_FONT_JOB_BYTES,
    },
    native::NativeError,
    package::{self, FileRef},
    paths,
    prepare_control::{self, PrepareControl},
    store::{self, Store},
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Clone, Debug)]
pub struct FontRef {
    pub asset: FileRef,
    pub target_name: String,
    pub display_name: String,
    pub origins: Vec<FontOrigin>,
}
fn invalid() -> NativeError {
    NativeError::new(
        "invalidFontRef",
        "Bundled font references are malformed or inconsistent.",
    )
}
pub fn font_path(root: &Path, id: &str) -> Result<PathBuf, NativeError> {
    if !package::hex(id, 32) {
        return Err(invalid());
    }
    Ok(crate::storage::path(root.as_ref(), "fonts")?.join(format!("{id}.ttf")))
}
pub(crate) fn container_json(c: &FontContainer) -> JsonValue {
    match c {
        FontContainer::Raw {
            root_ordinal,
            relative_path,
        } => object! {kind:"raw",rootOrdinal:*root_ordinal,relativePath:relative_path.clone()},
        FontContainer::Archive {
            root_ordinal,
            archive_relative_path,
            archive_sha256,
            member_index,
            decoder_path,
            normalized_path,
        } => {
            object! {kind:"archive",rootOrdinal:*root_ordinal,archiveRelativePath:archive_relative_path.clone(),archiveSha256:archive_sha256.clone(),memberIndex:*member_index,decoderPath:decoder_path.clone(),normalizedPath:normalized_path.clone()}
        }
    }
}
fn member_json(m: &VpkFontMemberOrigin) -> JsonValue {
    object! {archiveIndex:m.archive_index,container:container_json(&m.container),sha256:m.sha256.clone(),size:m.size}
}
pub(crate) fn origin_json(o: &FontOrigin) -> JsonValue {
    match o {
        FontOrigin::Loose { container } => {
            object! {kind:"loose",container:container_json(container)}
        }
        FontOrigin::Vpk {
            container,
            candidate_id,
            resource_path,
            resource_index,
            archive_index,
            resource_crc,
            members,
        } => {
            object! {kind:"vpk",container:container_json(container),candidateId:candidate_id.clone(),resourcePath:resource_path.clone(),resourceIndex:*resource_index,archiveIndex:*archive_index,resourceCrc:*resource_crc,members:JsonValue::from(members.iter().map(member_json).collect::<Vec<_>>())}
        }
        FontOrigin::UnavailableVpk {
            container,
            candidate_id,
            members,
        } => {
            object! {kind:"unavailableVpk",container:container_json(container),candidateId:candidate_id.clone(),members:JsonValue::from(members.iter().map(member_json).collect::<Vec<_>>())}
        }
    }
}
pub(crate) fn candidate_json(c: &FontCandidate) -> JsonValue {
    object! {id:c.id.clone(),targetName:c.target_name.clone(),displayName:c.display_name.clone(),origins:JsonValue::from(c.origins.iter().map(origin_json).collect::<Vec<_>>()),available:c.prepared.is_some() && c.unavailable_reason.is_none(),unavailableReason:c.unavailable_reason.clone(),sha256:c.prepared.as_ref().map(|f|f.fingerprint().sha256.clone()),size:c.prepared.as_ref().map(|f|f.fingerprint().size)}
}
pub(crate) fn encode_ref(f: &FontRef) -> JsonValue {
    object! {asset:package::encode_file(&f.asset),targetName:f.target_name.clone(),displayName:f.display_name.clone(),origins:JsonValue::from(f.origins.iter().map(origin_json).collect::<Vec<_>>())}
}
fn text<'a>(v: &'a JsonValue, k: &str, max: usize) -> Result<&'a str, NativeError> {
    v[k].as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(invalid)
}
fn relative(s: &str) -> bool {
    !s.starts_with('/')
        && s.split('/').count() <= 32
        && s.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
fn source_relative(v: &JsonValue, key: &str) -> Result<String, NativeError> {
    // Raw filesystem spelling is history. Ancestor names may contain controls;
    // JSON encodes them without granting any authority to reopen the path.
    let value = v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && relative(s))
        .ok_or_else(invalid)?;
    Ok(value.to_owned())
}
fn container(v: &JsonValue) -> Result<FontContainer, NativeError> {
    let root_ordinal = v["rootOrdinal"]
        .as_usize()
        .filter(|n| *n < 64)
        .ok_or_else(invalid)?;
    match v["kind"].as_str() {
        Some("raw") => {
            let p = source_relative(v, "relativePath")?;
            Ok(FontContainer::Raw {
                root_ordinal,
                relative_path: p,
            })
        }
        Some("archive") => {
            let p = source_relative(v, "archiveRelativePath")?;
            let n = text(v, "normalizedPath", 4096)?;
            let sha = text(v, "archiveSha256", 64)?;
            if !relative(n)
                || !package::hex(sha, 64)
                || (!v["archiveId"].is_null()
                    && !v["archiveId"].as_str().is_some_and(|s| package::hex(s, 32)))
            {
                return Err(invalid());
            }
            Ok(FontContainer::Archive {
                root_ordinal,
                archive_relative_path: p,
                archive_sha256: sha.into(),
                member_index: v["memberIndex"]
                    .as_u32()
                    .filter(|n| *n <= i32::MAX as u32)
                    .ok_or_else(invalid)?,
                decoder_path: text(v, "decoderPath", 4096)?.into(),
                normalized_path: n.into(),
            })
        }
        _ => Err(invalid()),
    }
}
fn decode_origin(v: &JsonValue) -> Result<FontOrigin, NativeError> {
    let c = container(&v["container"])?;
    if v["kind"] == "loose" {
        return Ok(FontOrigin::Loose { container: c });
    }
    if v["kind"] != "vpk"
        || !v["members"].is_array()
        || v["members"].is_empty()
        || v["members"].len() > 2
    {
        return Err(invalid());
    }
    let id = text(v, "candidateId", 64)?;
    if !package::hex(id, 64) {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let members = v["members"]
        .members()
        .map(|m| {
            let index = if m["archiveIndex"].is_null() {
                None
            } else {
                Some(m["archiveIndex"].as_u16().ok_or_else(invalid)?)
            };
            if !seen.insert(index) {
                return Err(invalid());
            }
            let sha = text(m, "sha256", 64)?;
            if !package::hex(sha, 64) {
                return Err(invalid());
            }
            Ok(VpkFontMemberOrigin {
                archive_index: index,
                container: container(&m["container"])?,
                sha256: sha.into(),
                size: m["size"]
                    .as_u64()
                    .filter(|n| *n <= crate::archive_content::MAX_FILE_BYTES)
                    .ok_or_else(invalid)?,
            })
        })
        .collect::<Result<Vec<_>, NativeError>>()?;
    if members
        .first()
        .is_none_or(|m| m.archive_index.is_some() || m.container != c)
        || members
            .windows(2)
            .any(|w| w[0].archive_index >= w[1].archive_index)
        || members
            .iter()
            .filter_map(|m| m.archive_index)
            .any(|i| Some(i) != v["archiveIndex"].as_u16() || i == 0x7fff)
    {
        return Err(invalid());
    }
    Ok(FontOrigin::Vpk {
        container: c,
        candidate_id: id.into(),
        resource_path: text(v, "resourcePath", 4096)?.into(),
        resource_index: v["resourceIndex"]
            .as_usize()
            .filter(|n| *n < 100_000)
            .ok_or_else(invalid)?,
        archive_index: v["archiveIndex"].as_u16().ok_or_else(invalid)?,
        resource_crc: v["resourceCrc"].as_u32().ok_or_else(invalid)?,
        members,
    })
}
pub fn stored_refs(record: &JsonValue) -> Result<Vec<FontRef>, NativeError> {
    let local = &record["source"]["localImport"];
    if local["fonts"].is_null() {
        if local["version"] == 3 {
            return Err(invalid());
        }
        return Ok(Vec::new());
    }
    if record["source"]["kind"] != "local"
        || local["version"] != 3
        || !local["fonts"].is_array()
        || local["fonts"].is_empty()
        || local["fonts"].len() > 256
    {
        return Err(invalid());
    }
    let mut ids = BTreeSet::new();
    let mut bytes = 0u64;
    let mut refs = Vec::new();
    let mut origin_count = 0usize;
    for v in local["fonts"].members() {
        let asset = package::decode_file(&v["asset"])?;
        let target = text(v, "targetName", 240)?;
        let display = text(v, "displayName", 1024)?;
        if asset.size > MAX_FONT_BYTES
            || !ids.insert(asset.id.clone())
            || !paths::safe_name(target)
            || !target.to_ascii_lowercase().ends_with(".ttf")
            || asset.source_name != target
            || display.chars().count() > 256
            || !display
                .chars()
                .all(|c| matches!(c as u32,0x20..=0xd7ff|0xe000..=0xfffd|0x10000..=0x10ffff))
            || !v["origins"].is_array()
            || v["origins"].is_empty()
            || v["origins"].len() > 256
        {
            return Err(invalid());
        }
        bytes = bytes
            .checked_add(asset.size)
            .filter(|n| *n <= MAX_FONT_JOB_BYTES)
            .ok_or_else(invalid)?;
        let origins = v["origins"]
            .members()
            .map(decode_origin)
            .collect::<Result<Vec<_>, _>>()?;
        origin_count = origin_count
            .checked_add(origins.len())
            .filter(|n| *n <= 256)
            .ok_or_else(invalid)?;
        for (i, origin) in origins.iter().enumerate() {
            if origins[..i].contains(origin) {
                return Err(invalid());
            }
            let path = match origin {
                FontOrigin::Loose {
                    container: FontContainer::Raw { relative_path, .. },
                } => relative_path,
                FontOrigin::Loose {
                    container:
                        FontContainer::Archive {
                            normalized_path, ..
                        },
                } => normalized_path,
                FontOrigin::Vpk {
                    resource_path,
                    candidate_id,
                    ..
                } => {
                    if !local["selectedCandidateIds"]
                        .members()
                        .any(|v| v == candidate_id.as_str())
                    {
                        return Err(invalid());
                    }
                    resource_path
                }
                FontOrigin::UnavailableVpk { .. } => return Err(invalid()),
            };
            if !crate::font_content::bundled_ttf(path)
                || path.rsplit('/').next().map(folded) != Some(folded(target))
            {
                return Err(invalid());
            }
        }
        for origin in v["origins"].members() {
            for c in std::iter::once(&origin["container"])
                .chain(origin["members"].members().map(|m| &m["container"]))
            {
                if c["kind"] == "archive"
                    && !local["archives"].members().any(|a| {
                        a["rootOrdinal"] == c["rootOrdinal"]
                            && a["relativePath"] == c["archiveRelativePath"]
                            && a["file"]["id"] == c["archiveId"]
                            && a["file"]["sha256"] == c["archiveSha256"]
                    })
                {
                    return Err(invalid());
                }
            }
        }
        refs.push(FontRef {
            asset,
            target_name: target.into(),
            display_name: display.into(),
            origins,
        });
    }
    if refs.windows(2).any(|w| w[0].asset.id >= w[1].asset.id) {
        return Err(invalid());
    }
    Ok(refs)
}
/// Display-only diagnostics have no file authority and never become FontRefs.
pub(crate) fn diagnostics(record: &JsonValue) -> Result<Option<&JsonValue>, NativeError> {
    let value = &record["source"]["localImport"]["fontDiagnostics"];
    if value.is_null() {
        return Ok(None);
    }
    if !value.is_array() || value.len() > 512 {
        return Err(invalid());
    }
    for row in value.members() {
        if !row["id"].as_str().is_some_and(|s| package::hex(s, 32))
            || row["available"] != false
            || !row["sha256"].is_null()
            || !row["size"].is_null()
            || !row["targetName"].as_str().is_some_and(|s| s.len() <= 4096)
            || !row["displayName"].as_str().is_some_and(|s| s.len() <= 4096)
            || !row["unavailableReason"]
                .as_str()
                .is_some_and(|s| s.len() <= 4096)
            || !row["origins"].is_array()
            || row["origins"].len() > 1
        {
            return Err(invalid());
        }
    }
    Ok(Some(value))
}
fn folded(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

pub const MAX_INSTALLED_FONTS: usize = 256;
/// Shared review/checkpoint limit; Task4 must also enforce it in its writer.
pub const MAX_FONT_TRANSACTION_TARGETS: usize = 257;
pub const MAX_FONT_OWNERS: usize = 65_536;
pub const MAX_FONT_STATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TARGET_ENTRIES: usize = 10_000;
pub const MAX_REVIEW_READ_BYTES: u64 = 512 * 1024 * 1024;
fn state_error() -> NativeError {
    NativeError::new("invalidFontState", "Global font ownership or checkpoint metadata is malformed; preserve the library for recovery.")
}
fn changed() -> NativeError {
    NativeError::new("externalFontChange", "A reviewed font, configuration, source, or parent directory changed; preserve the files and review again.")
}
fn limit() -> NativeError {
    NativeError::new(
        "fontLimit",
        "Font review exceeds its bounded file, owner, inventory, metadata or byte budget.",
    )
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct FontFingerprint {
    pub sha256:String,pub size:u64,pub device:u64,pub inode:u64,
    pub modified_seconds:i64,pub modified_nanos:i64,pub fsid:Option<String>,
}
impl From<paths::Fingerprint> for FontFingerprint {
    fn from(f:paths::Fingerprint)->Self{Self{sha256:f.sha256,size:f.size,device:f.device,inode:f.inode,modified_seconds:f.modified_seconds,modified_nanos:f.modified_nanos,fsid:None}}
}
pub(crate) trait FingerprintJson {fn json(&self)->JsonValue;}
impl FingerprintJson for paths::Fingerprint {
    fn json(&self)->JsonValue {object!{sha256:self.sha256.clone(),size:self.size,device:self.device,inode:self.inode,modifiedSeconds:self.modified_seconds,modifiedNanos:self.modified_nanos}}
}
impl FingerprintJson for FontFingerprint {
    fn json(&self)->JsonValue {let mut v=object!{sha256:self.sha256.clone(),size:self.size,device:self.device,inode:self.inode,modifiedSeconds:self.modified_seconds,modifiedNanos:self.modified_nanos};if let Some(fsid)=&self.fsid{v["fsid"]=fsid.clone().into();}v}
}
fn optional_fsid(v:&JsonValue)->Result<Option<String>,NativeError>{
    if !generation_fsid_valid(v){return Err(state_error());}Ok(v["fsid"].as_str().map(str::to_owned))
}
fn generation_fsid_valid(v:&JsonValue)->bool{crate::generation::valid_fsid(v)}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) fsid: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FontOwner {
    pub(crate) mod_id: String,
    pub(crate) asset_id: String,
    pub(crate) display_name: String,
}
impl FontOwner {
    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }
    pub fn asset_id(&self) -> &str {
        &self.asset_id
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontFileOwnership {
    Created,
    Borrowed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledFont {
    pub(crate) target_name: String,
    pub(crate) ownership: FontFileOwnership,
    pub(crate) identity: FontFingerprint,
    pub(crate) mode: u32,
    pub(crate) owners: Vec<FontOwner>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontBlock {
    pub(crate) root_id: String,
    pub(crate) id: String,
    pub(crate) names: Vec<String>,
    pub(crate) bytes: Vec<u8>,
}
impl FontBlock {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFootprint {
    pub(crate) root_path: PathBuf,
    pub(crate) root_identity: DirectoryIdentity,
    pub(crate) game_path: PathBuf,
    pub(crate) game_identity: DirectoryIdentity,
    pub(crate) directory: Option<DirectoryIdentity>,
    pub(crate) directory_created: bool,
    pub(crate) files: Vec<InstalledFont>,
    pub(crate) conf: Option<FontFingerprint>,
    pub(crate) conf_mode: Option<u32>,
    pub(crate) owned_block: Option<FontBlock>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FontTarget {
    Configuration,
    Font(String),
}
#[derive(Clone, Debug)]
pub enum StoredFontImage {
    Absent,
    Present {
        backup: FileRef,
        identity: FontFingerprint,
        mode: u32,
    },
}
#[derive(Clone, Debug)]
pub struct FontImage {
    pub(crate) target: FontTarget,
    pub(crate) image: StoredFontImage,
}
#[derive(Clone, Debug)]
pub struct FontCheckpoint {
    pub(crate) transaction_id: String,
    pub(crate) prior: Option<FontFootprint>,
    pub(crate) images: Vec<FontImage>,
}
#[derive(Clone, Debug, Default)]
pub struct FontState {
    pub(crate) installed: Option<FontFootprint>,
    pub(crate) previous: Option<FontCheckpoint>,
}
pub(crate) fn dir_json(d: &DirectoryIdentity) -> JsonValue {
     {let mut v=object! {device:d.device,inode:d.inode};if let Some(fsid)=&d.fsid{v["fsid"]=fsid.clone().into();}v}
}
pub(crate) fn dir_decode(v: &JsonValue) -> Result<DirectoryIdentity, NativeError> {
    Ok(DirectoryIdentity { fsid:optional_fsid(v)?,
        device: v["device"].as_u64().ok_or_else(state_error)?,
        inode: v["inode"]
            .as_u64()
            .filter(|n| *n != 0)
            .ok_or_else(state_error)?,
    })
}
pub(crate) fn fingerprint_json(f:&impl FingerprintJson)->JsonValue {f.json()}
pub(crate) fn fingerprint_decode(
    v: &JsonValue,
    max: u64,
) -> Result<FontFingerprint, NativeError> {
    Ok(FontFingerprint { fsid:optional_fsid(v)?,
        sha256: v["sha256"]
            .as_str()
            .filter(|s| package::hex(s, 64))
            .ok_or_else(state_error)?
            .into(),
        size: v["size"]
            .as_u64()
            .filter(|n| *n <= max)
            .ok_or_else(state_error)?,
        device: v["device"].as_u64().ok_or_else(state_error)?,
        inode: v["inode"]
            .as_u64()
            .filter(|n| *n != 0)
            .ok_or_else(state_error)?,
        modified_seconds: v["modifiedSeconds"].as_i64().ok_or_else(state_error)?,
        modified_nanos: v["modifiedNanos"]
            .as_i64()
            .filter(|n| (0..1_000_000_000).contains(n))
            .ok_or_else(state_error)?,
    })
}
fn mode(v: &JsonValue) -> Result<u32, NativeError> {
    v.as_u32().filter(|n| *n <= 0o777).ok_or_else(state_error)
}
fn target_name(s: &str) -> bool {
    s.len() <= 240 && paths::safe_name(s) && s.to_ascii_lowercase().ends_with(".ttf")
}
fn family(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 1024
        && s.chars().count() <= 256
        && !s.chars().any(char::is_control)
        && font_config::xml_chars(s)
}
fn canonical_path(v: &JsonValue) -> Result<PathBuf, NativeError> {
    let s = v
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(state_error)?;
    let p = Path::new(s);
    if !p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || p.as_os_str() != paths::absolute(p)?.as_os_str()
    {
        return Err(state_error());
    }
    Ok(p.to_owned())
}
fn owner_json(o: &FontOwner) -> JsonValue {
    object! {modId:o.mod_id.clone(),assetId:o.asset_id.clone(),displayName:o.display_name.clone()}
}
fn block_json(b: &FontBlock) -> JsonValue {
    object! {rootId:b.root_id.clone(),id:b.id.clone(),names:JsonValue::from(b.names.clone()),bytes:String::from_utf8_lossy(&b.bytes).as_ref()}
}
fn block_decode(v: &JsonValue) -> Result<FontBlock, NativeError> {
    if !v["names"].is_array() || v["names"].len() > MAX_FONT_OWNERS {
        return Err(state_error());
    }
    let b = FontBlock {
        root_id: text(v, "rootId", 64)?.into(),
        id: text(v, "id", 32)?.into(),
        names: v["names"]
            .members()
            .map(|n| {
                n.as_str()
                    .filter(|s| family(s))
                    .map(str::to_owned)
                    .ok_or_else(state_error)
            })
            .collect::<Result<_, _>>()?,
        bytes: v["bytes"]
            .as_str()
            .filter(|s| s.len() <= font_config::MAX_CONF_BYTES)
            .ok_or_else(state_error)?
            .as_bytes()
            .to_vec(),
    };
    font_config::validate_block(&b)?;
    Ok(b)
}
fn footprint_json(f: &FontFootprint) -> JsonValue {
    object! {rootPath:f.root_path.to_string_lossy().as_ref(),rootIdentity:dir_json(&f.root_identity),gamePath:f.game_path.to_string_lossy().as_ref(),gameIdentity:dir_json(&f.game_identity),directory:f.directory.as_ref().map(dir_json),directoryCreated:f.directory_created,
    files:JsonValue::from(f.files.iter().map(|f|object! {targetName:f.target_name.clone(),ownership:match f.ownership {FontFileOwnership::Created=>"created",FontFileOwnership::Borrowed=>"borrowed"},identity:fingerprint_json(&f.identity),mode:f.mode,owners:JsonValue::from(f.owners.iter().map(owner_json).collect::<Vec<_>>())}).collect::<Vec<_>>()),
    conf:f.conf.as_ref().map(fingerprint_json),confMode:f.conf_mode,ownedBlock:f.owned_block.as_ref().map(block_json)}
}
fn footprint_decode(v: &JsonValue) -> Result<FontFootprint, NativeError> {
    if !v["files"].is_array() || v["files"].len() > MAX_INSTALLED_FONTS {
        return Err(state_error());
    }
    let mut files = Vec::new();
    let mut owner_ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for row in v["files"].members() {
        let target = row["targetName"]
            .as_str()
            .filter(|s| target_name(s))
            .ok_or_else(state_error)?;
        if !names.insert(folded(target))
            || !row["owners"].is_array()
            || row["owners"].is_empty()
            || row["owners"].len() > MAX_FONT_OWNERS
        {
            return Err(state_error());
        }
        let identity = fingerprint_decode(&row["identity"], MAX_FONT_BYTES)?;
        total = total
            .checked_add(identity.size)
            .filter(|n| *n <= MAX_FONT_JOB_BYTES)
            .ok_or_else(state_error)?;
        let mut owners = Vec::new();
        for o in row["owners"].members() {
            let mod_id = o["modId"]
                .as_str()
                .filter(|s| paths::safe_id(s))
                .ok_or_else(state_error)?
                .to_owned();
            let asset_id = o["assetId"]
                .as_str()
                .filter(|s| package::hex(s, 32))
                .ok_or_else(state_error)?
                .to_owned();
            if !owner_ids.insert((mod_id.clone(), asset_id.clone()))
                || owner_ids.len() > MAX_FONT_OWNERS
            {
                return Err(state_error());
            }
            owners.push(FontOwner {
                mod_id,
                asset_id,
                display_name: o["displayName"]
                    .as_str()
                    .filter(|s| family(s))
                    .ok_or_else(state_error)?
                    .into(),
            });
        }
        if owners.windows(2).any(|w| w[0] >= w[1]) {
            return Err(state_error());
        }
        files.push(InstalledFont {
            target_name: target.into(),
            ownership: match row["ownership"].as_str() {
                Some("created") => FontFileOwnership::Created,
                Some("borrowed") => FontFileOwnership::Borrowed,
                _ => return Err(state_error()),
            },
            identity,
            mode: mode(&row["mode"])?,
            owners,
        });
    }
    if files
        .windows(2)
        .any(|w| w[0].target_name >= w[1].target_name)
    {
        return Err(state_error());
    }
    let f = FontFootprint {
        root_path: canonical_path(&v["rootPath"])?,
        root_identity: dir_decode(&v["rootIdentity"])?,
        game_path: canonical_path(&v["gamePath"])?,
        game_identity: dir_decode(&v["gameIdentity"])?,
        directory: if v["directory"].is_null() {
            None
        } else {
            Some(dir_decode(&v["directory"])?)
        },
        directory_created: v["directoryCreated"].as_bool().ok_or_else(state_error)?,
        files,
        conf: if v["conf"].is_null() {
            None
        } else {
            Some(fingerprint_decode(
                &v["conf"],
                font_config::MAX_CONF_BYTES as u64,
            )?)
        },
        conf_mode: if v["confMode"].is_null() {
            None
        } else {
            Some(mode(&v["confMode"])?)
        },
        owned_block: if v["ownedBlock"].is_null() {
            None
        } else {
            Some(block_decode(&v["ownedBlock"])?)
        },
    };
    if f.conf.is_some() != f.conf_mode.is_some()
        || (f.directory.is_none()
            && (f.directory_created || !f.files.is_empty() || f.conf.is_some()))
    {
        return Err(state_error());
    }
    let names: Vec<String> = f
        .files
        .iter()
        .flat_map(|f| f.owners.iter().map(|o| o.display_name.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if f.owned_block.as_ref().is_some_and(|b| b.names != names)
        || (f.owned_block.is_some() && (f.conf.is_none() || names.is_empty()))
        || (f.conf.is_some() && !names.is_empty() && f.owned_block.is_none())
    {
        return Err(state_error());
    }
    Ok(f)
}
pub(crate) fn target_json(t: &FontTarget) -> JsonValue {
    match t {
        FontTarget::Configuration => object! {kind:"configuration"},
        FontTarget::Font(name) => object! {kind:"font",name:name.clone()},
    }
}
pub(crate) fn target_decode(v: &JsonValue) -> Result<FontTarget, NativeError> {
    match v["kind"].as_str() {
        Some("configuration") if !v.has_key("name") => Ok(FontTarget::Configuration),
        Some("font") => Ok(FontTarget::Font(
            v["name"]
                .as_str()
                .filter(|s| target_name(s))
                .ok_or_else(state_error)?
                .into(),
        )),
        _ => Err(state_error()),
    }
}
pub fn backup_path(root: &Path, id: &str) -> Result<PathBuf, NativeError> {
    if !package::hex(id, 32) {
        return Err(state_error());
    }
    Ok(crate::storage::path(root.as_ref(), "font-backups")?.join(format!("{id}.blob")))
}
impl FontState {
    pub fn decode(doc: &JsonValue) -> Result<Self, NativeError> {
        match doc["storeVersion"].as_u32() {
            Some(1 | 2) => return Ok(Self::default()),
            Some(3 | 4) => (),
            _ => return Err(state_error()),
        }
        let v = &doc["fonts"];
        if v["version"] != 1
            || !v.is_object()
            || v.len() != 3
            || !v.has_key("installed")
            || !v.has_key("previous")
            || v.dump().len() > MAX_FONT_STATE_BYTES
        {
            return Err(state_error());
        }
        let installed = if v["installed"].is_null() {
            None
        } else {
            Some(footprint_decode(&v["installed"])?)
        };
        let previous = if v["previous"].is_null() {
            None
        } else {
            let p = &v["previous"];
            let transaction_id = p["transactionId"]
                .as_str()
                .filter(|s| package::hex(s, 32))
                .ok_or_else(state_error)?
                .to_owned();
            let prior = if p["prior"].is_null() {
                None
            } else {
                Some(footprint_decode(&p["prior"])?)
            };
            if !p["images"].is_array() || p["images"].len() > MAX_FONT_TRANSACTION_TARGETS {
                return Err(state_error());
            }
            let mut images = Vec::new();
            let mut ids = BTreeSet::new();
            let mut total = 0u64;
            for row in p["images"].members() {
                let target = target_decode(&row["target"])?;
                let image = match row["image"]["kind"].as_str() {
                    Some("absent") => StoredFontImage::Absent,
                    Some("present") => {
                        let backup = package::decode_file(&row["image"]["backup"])?;
                        let max = match target {
                            FontTarget::Configuration => font_config::MAX_CONF_BYTES as u64,
                            FontTarget::Font(_) => MAX_FONT_BYTES,
                        };
                        let identity = fingerprint_decode(&row["image"]["identity"], max)?;
                        if backup.size != identity.size
                            || backup.sha256 != identity.sha256
                            || !ids.insert(backup.id.clone())
                        {
                            return Err(state_error());
                        }
                        total = total
                            .checked_add(backup.size)
                            .filter(|n| {
                                *n <= MAX_FONT_JOB_BYTES + font_config::MAX_CONF_BYTES as u64
                            })
                            .ok_or_else(state_error)?;
                        StoredFontImage::Present {
                            backup,
                            identity,
                            mode: mode(&row["image"]["mode"])?,
                        }
                    }
                    _ => return Err(state_error()),
                };
                images.push(FontImage { target, image });
            }
            if images.windows(2).any(|w| w[0].target >= w[1].target) {
                return Err(state_error());
            }
            Some(FontCheckpoint {
                transaction_id,
                prior,
                images,
            })
        };
        if previous.is_some() && installed.is_none() {
            return Err(state_error());
        }
        for footprint in installed
            .iter()
            .chain(previous.iter().filter_map(|p| p.prior.as_ref()))
        {
            if doc["gamePath"].as_str() != footprint.game_path.to_str() {
                return Err(state_error());
            }
            if let Some(current) = &installed {
                if current.root_path != footprint.root_path
                    || current.root_identity != footprint.root_identity
                    || current.game_identity != footprint.game_identity
                {
                    return Err(state_error());
                }
            }
        }
        let result = Self {
            installed,
            previous,
        };
        validate_checkpoint(&result)?;
        Ok(result)
    }
    pub fn encode(&self) -> Result<JsonValue, NativeError> {
        if self
            .previous
            .as_ref()
            .is_some_and(|p| p.images.len() > MAX_FONT_TRANSACTION_TARGETS)
        {
            return Err(limit());
        }
        let previous = self.previous.as_ref().map(|p| object! {transactionId:p.transaction_id.clone(),prior:p.prior.as_ref().map(footprint_json),images:JsonValue::from(p.images.iter().map(|i|object! {target:target_json(&i.target),image:match &i.image {StoredFontImage::Absent=>object!{kind:"absent"},StoredFontImage::Present{backup,identity,mode}=>object!{kind:"present",backup:package::encode_file(backup),identity:fingerprint_json(identity),mode:*mode}}}).collect::<Vec<_>>())});
        let value = object! {version:1,installed:self.installed.as_ref().map(footprint_json),previous:previous};
        if value.dump().len() > MAX_FONT_STATE_BYTES {
            return Err(limit());
        }
        Ok(value)
    }
}
/// Newly emitted state never grants target-directory deletion authority. A
/// checkpoint remembers file/configuration images; its directory is observed.
pub(crate) fn retain_directories(state: &mut FontState) {
    if let Some(current) = &mut state.installed {
        current.directory_created = false;
        if let Some(prior) = state.previous.as_mut().and_then(|p| p.prior.as_mut()) {
            prior.directory_created = false;
            prior.directory = current.directory.clone();
        }
    }
}

pub(crate) fn validate_state(doc: &JsonValue) -> Result<(), NativeError> {
    FontState::decode(doc).map(|_| ())
}

fn validate_checkpoint(state: &FontState) -> Result<(), NativeError> {
    let Some(checkpoint) = &state.previous else {
        return Ok(());
    };
    let current = state.installed.as_ref().ok_or_else(state_error)?;
    let prior = checkpoint.prior.as_ref();
    if let Some(prior) = prior {
        match (&prior.directory, &current.directory) {
            (Some(a), Some(b))
                if a == b && prior.directory_created == current.directory_created =>
            {
                ()
            }
            (None, Some(_)) if current.directory_created => (),
            (None, None) => (),
            _ => return Err(state_error()),
        }
    }
    let images: BTreeMap<_, _> = checkpoint
        .images
        .iter()
        .map(|i| (&i.target, &i.image))
        .collect();
    let mut expected = BTreeSet::new();
    let names: BTreeSet<_> = current
        .files
        .iter()
        .chain(prior.into_iter().flat_map(|f| f.files.iter()))
        .map(|f| f.target_name.clone())
        .collect();
    for name in names {
        let old = prior.and_then(|f| f.files.iter().find(|f| f.target_name == name));
        let new = current.files.iter().find(|f| f.target_name == name);
        let target = FontTarget::Font(name);
        let changed_bytes = match (old, new) {
            (Some(a), Some(b)) => {
                if a.ownership != b.ownership {
                    return Err(state_error());
                }
                let changed = a.identity.sha256 != b.identity.sha256
                    || a.identity.size != b.identity.size
                    || a.mode != b.mode;
                if !changed && a.identity != b.identity {
                    return Err(state_error());
                }
                if changed && a.ownership == FontFileOwnership::Borrowed {
                    return Err(state_error());
                }
                changed
            }
            (Some(a), None) => a.ownership == FontFileOwnership::Created,
            (None, Some(b)) => b.ownership == FontFileOwnership::Created,
            (None, None) => false,
        };
        if changed_bytes {
            expected.insert(target.clone());
            match (images.get(&target), old) {
                (Some(StoredFontImage::Absent), None) => (),
                (Some(StoredFontImage::Present { backup, mode, .. }), Some(old))
                    if backup.sha256 == old.identity.sha256
                        && backup.size == old.identity.size
                        && *mode == old.mode =>
                {
                    ()
                }
                _ => return Err(state_error()),
            }
        }
    }
    if let Some(prior) = prior {
        let changed_conf = match (&prior.conf, &current.conf) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                let changed = a.sha256 != b.sha256
                    || a.size != b.size
                    || prior.conf_mode != current.conf_mode;
                if !changed && a != b {
                    return Err(state_error());
                }
                changed
            }
            _ => true,
        };
        if changed_conf {
            if current
                .owned_block
                .as_ref()
                .is_some_and(|b| b.id != checkpoint.transaction_id)
            {
                return Err(state_error());
            }
            expected.insert(FontTarget::Configuration);
            match (images.get(&FontTarget::Configuration), &prior.conf) {
                (Some(StoredFontImage::Absent), None) => (),
                (Some(StoredFontImage::Present { backup, mode, .. }), Some(old))
                    if backup.sha256 == old.sha256
                        && backup.size == old.size
                        && Some(*mode) == prior.conf_mode =>
                {
                    ()
                }
                _ => return Err(state_error()),
            }
        }
    } else if images.contains_key(&FontTarget::Configuration) {
        // First ownership transaction can back up an existing unknown conf;
        // missing conf is never synthesized, so an absent preimage is invalid.
        if !matches!(
            images[&FontTarget::Configuration],
            StoredFontImage::Present { .. }
        ) || current.conf.is_none()
            || current.owned_block.is_none()
            || current
                .owned_block
                .as_ref()
                .is_some_and(|b| b.id != checkpoint.transaction_id)
        {
            return Err(state_error());
        }
        expected.insert(FontTarget::Configuration);
    } else if current.owned_block.is_some() {
        return Err(state_error());
    }
    if images.keys().any(|target| !expected.contains(*target)) {
        return Err(state_error());
    }
    Ok(())
}

/// Complete immutable bytes, admitted from one nonsymlink descriptor and checked
/// against the named entry again after the bounded read and memory hash.
#[derive(Clone, Debug)]
pub struct CheckedFontFile {
    pub(crate) path: PathBuf,
    pub(crate) identity: FontFingerprint,
    pub(crate) mode: u32,
    pub(crate) bytes: Arc<[u8]>,
}
impl CheckedFontFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn identity(&self) -> &FontFingerprint {
        &self.identity
    }
    pub fn mode(&self) -> u32 {
        self.mode
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Clone, Debug)]
pub enum CheckedFontImage {
    Absent,
    Present(CheckedFontFile),
}
impl CheckedFontImage {
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Absent => None,
            Self::Present(f) => Some(f.bytes()),
        }
    }
}
#[derive(Clone, Debug)]
pub struct FontFileChange {
    pub(crate) target: FontTarget,
    pub(crate) before: CheckedFontImage,
    pub(crate) after: Option<Arc<[u8]>>,
    pub(crate) mode: u32,
}
impl FontFileChange {
    pub fn target(&self) -> &FontTarget {
        &self.target
    }
    pub fn before(&self) -> &CheckedFontImage {
        &self.before
    }
    pub fn after(&self) -> Option<&[u8]> {
        self.after.as_deref()
    }
    pub fn mode(&self) -> u32 {
        self.mode
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontDirectoryEvidence {
    pub(crate) parents: Vec<(PathBuf, DirectoryIdentity)>,
    pub(crate) directory: Option<DirectoryIdentity>,
    pub(crate) names: Vec<String>,
}
struct ReadBudget {
    bytes: u64,
}
impl ReadBudget {
    fn new() -> Self {
        Self { bytes: 0 }
    }
    fn charge(&mut self, bytes: u64) -> Result<(), NativeError> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= MAX_REVIEW_READ_BYTES)
            .ok_or_else(limit)?;
        Ok(())
    }
}
fn meta_key(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u64, u32) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.nlink(),
        m.mode(),
    )
}
pub(crate) fn directory_identity(path: &Path) -> Result<DirectoryIdentity, NativeError> {
    paths::reject_symlinks(path)?;
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() {
        return Err(changed());
    }
    Ok(DirectoryIdentity { fsid:Some(paths::persisted_filesystem_id(&fs::File::open(path)?)?),
        device: m.dev(),
        inode: m.ino(),
    })
}
fn read_file(
    path: &Path,
    max: u64,
    budget: &mut ReadBudget,
    control: &PrepareControl,
) -> Result<CheckedFontFile, NativeError> {
    control.check()?;
    let mut f = paths::open_regular(path)?;
    let before = f.metadata()?;
    if before.nlink() != 1 || before.mode() & 0o7000 != 0 {
        return Err(changed());
    }
    if before.len() > max {
        return Err(limit());
    }
    budget.charge(before.len())?;
    let mut bytes = vec![0; before.len() as usize];
    for chunk in bytes.chunks_mut(64 * 1024) {
        control.check()?;
        f.read_exact(chunk).map_err(|_| changed())?;
    }
    let mut sentinel = [0];
    if f.read(&mut sentinel)? != 0 || meta_key(&before) != meta_key(&f.metadata()?) {
        return Err(changed());
    }
    let sha256 = prepare_control::digest_bounded_controlled(&bytes, max as usize, control)?;
    paths::reject_symlinks(path)?;
    if meta_key(&before) != meta_key(&f.metadata()?)
        || meta_key(&before) != meta_key(&fs::symlink_metadata(path)?)
    {
        return Err(changed());
    }
    Ok(CheckedFontFile {
        path: path.into(),
        identity: FontFingerprint { fsid:Some(paths::persisted_filesystem_id(&f)?),
            sha256,
            size: before.len(),
            device: before.dev(),
            inode: before.ino(),
            modified_seconds: before.mtime(),
            modified_nanos: before.mtime_nsec(),
        },
        mode: before.mode() & 0o777,
        bytes: bytes.into(),
    })
}
pub(crate) fn transaction_read(path: &Path, max: u64) -> Result<CheckedFontFile, NativeError> {
    read_file(path, max, &mut ReadBudget::new(), &PrepareControl::never_cancelled())
}

fn verify_source_parents(
    parents: &BTreeMap<PathBuf, DirectoryIdentity>,
) -> Result<(), NativeError> {
    for (path, expected) in parents {
        if directory_identity(path)? != *expected {
            return Err(changed());
        }
    }
    Ok(())
}
fn read_source(
    root: &Path,
    root_identity: &DirectoryIdentity,
    parents: &mut BTreeMap<PathBuf, DirectoryIdentity>,
    path: &Path,
    max: u64,
    budget: &mut ReadBudget,
    control: &PrepareControl,
) -> Result<CheckedFontFile, NativeError> {
    if directory_identity(root)? != *root_identity {
        return Err(changed());
    }
    // Called only with native font_path/backup_path results. Retain the actual
    // intermediate ancestors, never history/provenance paths or display names.
    let mut parent = path.parent().ok_or_else(changed)?;
    let bulk_root=crate::storage::root(root)?;
    while parent != bulk_root {
        if !parent.starts_with(&bulk_root) {
            return Err(changed());
        }
        let actual = directory_identity(parent)?;
        if let Some(expected) = parents.get(parent) {
            if *expected != actual {
                return Err(changed());
            }
        } else {
            parents.insert(parent.to_owned(), actual);
        }
        parent = parent.parent().ok_or_else(changed)?;
    }
    verify_source_parents(parents)?;
    let file = read_file(path, max, budget, control)?;
    verify_source_parents(parents)?;
    if directory_identity(root)? != *root_identity {
        return Err(changed());
    }
    Ok(file)
}
fn read_optional(
    path: &Path,
    max: u64,
    budget: &mut ReadBudget,
    control: &PrepareControl,
) -> Result<CheckedFontImage, NativeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(CheckedFontImage::Present(read_file(
            path, max, budget, control,
        )?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(path)?;
            Ok(CheckedFontImage::Absent)
        }
        Err(e) => Err(e.into()),
    }
}
fn target_directory(game: &Path) -> PathBuf {
    game.join("game/citadel/panorama/fonts")
}
pub(crate) fn target_path(game: &Path, target: &FontTarget) -> Result<PathBuf, NativeError> {
    Ok(target_directory(game).join(match target {
        FontTarget::Configuration => "fonts.conf",
        FontTarget::Font(name) if target_name(name) => name,
        _ => return Err(state_error()),
    }))
}
fn directory_evidence(root: &Path, game: &Path) -> Result<FontDirectoryEvidence, NativeError> {
    let directory = target_directory(game);
    let parent = directory.parent().ok_or_else(changed)?;
    let mut parents = BTreeMap::new();
    // Every existing ancestor is pinned; only the final fonts directory may be
    // missing. Task4 must journal its creation against these exact parents.
    for end in [root, parent] {
        for p in end.ancestors() {
            parents.insert(p.to_owned(), directory_identity(p)?);
        }
    }
    let mut names = Vec::new();
    let identity = match fs::symlink_metadata(&directory) {
        Ok(m) => {
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err(changed());
            }
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                if names.len() >= MAX_TARGET_ENTRIES {
                    return Err(limit());
                }
                let name = entry.file_name().into_string().map_err(|_| changed())?;
                names.push(name);
            }
            names.sort();
            let identity = DirectoryIdentity { fsid:Some(paths::persisted_filesystem_id(&fs::File::open(&directory)?)?),
                device: m.dev(),
                inode: m.ino(),
            };
            if directory_identity(&directory)? != identity {
                return Err(changed());
            }
            Some(identity)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(&directory)?;
            None
        }
        Err(e) => return Err(e.into()),
    };
    Ok(FontDirectoryEvidence {
        parents: parents.into_iter().collect(),
        directory: identity,
        names,
    })
}
fn root_id(
    root: &Path,
    identity: &DirectoryIdentity,
    control: &PrepareControl,
) -> Result<String, NativeError> {
    let value=if let Some(fsid)=&identity.fsid{object!{path:root.to_string_lossy().as_ref(),fsid:fsid.clone(),inode:identity.inode}}else{object!{path:root.to_string_lossy().as_ref(),device:identity.device,inode:identity.inode}};
    prepare_control::digest_controlled(
        value.dump()
            .as_bytes(),
        control,
    )
}
fn pin_game(doc: &JsonValue, game: &Path) -> Result<PathBuf, NativeError> {
    let game = paths::absolute(game)?;
    paths::reject_symlinks(&game)?;
    if fs::canonicalize(&game)? != game || doc["gamePath"].as_str() != game.to_str() {
        return Err(NativeError::new(
            "gamePathMismatch",
            "Font changes must use the canonical game installation recorded in this library.",
        ));
    }
    Ok(game)
}
pub(crate) struct FontAuthority<'a>{pub root:&'a crate::store::RootLock,pub game:&'a crate::game_access::GameLock}
impl<'a> FontAuthority<'a>{
    pub fn new(store:&Store,root:&'a crate::store::RootLock,game:&'a crate::game_access::GameLock)->Result<Self,NativeError>{store.locked(root)?;Ok(Self{root,game})}
    fn matches(&self,path:&Path,id:&JsonValue)->Result<bool,NativeError>{
        self.root.validate()?;
        if path.starts_with(self.root.root()) {
            paths::reject_symlinks(path)?;let file=fs::File::open(path)?;
            return paths::same_persisted_object(self.root.root(),self.root.directory(),path,&file,
                id["device"].as_u64().ok_or_else(state_error)?,id["inode"].as_u64().ok_or_else(state_error)?,id["fsid"].as_str());
        }
        if path.join("game/citadel").is_dir() {return self.game.font_game_anchor_matches(path,id);}
        self.game.same_persisted_object(path,id).or_else(|e|if id["fsid"].as_str().is_some(){crate::generation::owner_matches(path,id,None)}else{Err(e)})
    }
}
pub(crate) fn persisted_font_owner(path:&Path,id:&JsonValue,authority:Option<&FontAuthority>)->Result<bool,NativeError>{
    if !crate::generation::valid_fsid(id){return Ok(false);}
    if let Some(authority)=authority {
        if authority.matches(path,id)? {return Ok(true);}
    }
    // Recorded fsid permits strict ownership reads outside the mutation roots;
    // no legacy device fallback is granted there.
    crate::generation::owner_matches(path,id,None)
}
pub(crate) fn directory_matches(path:&Path,expected:&DirectoryIdentity,authority:Option<&FontAuthority>)->Result<bool,NativeError>{
    paths::reject_symlinks(path)?;if !fs::symlink_metadata(path)?.is_dir(){return Ok(false);}
    persisted_font_owner(path,&dir_json(expected),authority)
}
pub(crate) fn fingerprint_matches(path:&Path,actual:&FontFingerprint,expected:&FontFingerprint,authority:Option<&FontAuthority>)->Result<bool,NativeError>{
    Ok(actual.sha256==expected.sha256 && actual.size==expected.size && actual.modified_seconds==expected.modified_seconds
        && actual.modified_nanos==expected.modified_nanos && persisted_font_owner(path,&fingerprint_json(expected),authority)?)
}
fn live_fingerprint_matches(actual:&FontFingerprint,expected:&FontFingerprint)->bool{
    actual.sha256==expected.sha256 && actual.size==expected.size && actual.device==expected.device && actual.inode==expected.inode
        && actual.modified_seconds==expected.modified_seconds && actual.modified_nanos==expected.modified_nanos
        && expected.fsid.as_ref().is_none_or(|id|actual.fsid.as_ref()==Some(id))
}
fn check_persisted_file(actual:&CheckedFontImage,expected:Option<(&FontFingerprint,u32)>,authority:Option<&FontAuthority>)->Result<(),NativeError>{
    match (actual,expected){
        (CheckedFontImage::Absent,None)=>Ok(()),
        (CheckedFontImage::Present(file),Some((id,mode))) if file.mode==mode && fingerprint_matches(&file.path,&file.identity,id,authority)?=>Ok(()),
        _=>Err(changed()),
    }
}
fn check_file(
    actual: &CheckedFontImage,
    expected: Option<(&FontFingerprint, u32)>,
) -> Result<(), NativeError> {
    match (actual, expected) {
        (CheckedFontImage::Absent, None) => Ok(()),
        (CheckedFontImage::Present(file), Some((fp, mode)))
            if live_fingerprint_matches(&file.identity,fp) && file.mode == mode =>
        {
            Ok(())
        }
        _ => Err(changed()),
    }
}
struct Observed {
    directory: FontDirectoryEvidence,
    files: BTreeMap<String, CheckedFontFile>,
    conf: CheckedFontImage,
}
fn observe(
    root: &Path,
    game: &Path,
    footprint: Option<&FontFootprint>,
    budget: &mut ReadBudget,
    control: &PrepareControl,
    authority:Option<&FontAuthority>,
) -> Result<Observed, NativeError> {
    let directory = directory_evidence(root, game)?;
    let conf = read_optional(
        &target_path(game, &FontTarget::Configuration)?,
        font_config::MAX_CONF_BYTES as u64,
        budget,
        control,
    )?;
    let mut files = BTreeMap::new();
    if let Some(f) = footprint {
        if f.root_path != root
            || f.game_path != game
            || !directory_matches(root,&f.root_identity,authority)?
            || !directory_matches(game,&f.game_identity,authority)?
            || match (&f.directory,&directory.directory){(None,None)=>false,(Some(expected),Some(_))=>!directory_matches(&target_directory(game),expected,authority)?,_=>true}
        {
            return Err(changed());
        }
        check_persisted_file(&conf,f.conf.as_ref().zip(f.conf_mode),authority)?;
        for file in &f.files {
            if directory
                .names
                .iter()
                .filter(|name| folded(name) == folded(&file.target_name))
                .count()
                != 1
            {
                return Err(changed());
            }
            let actual = read_file(
                &target_path(game, &FontTarget::Font(file.target_name.clone()))?,
                MAX_FONT_BYTES,
                budget,
                control,
            )?;
            if !fingerprint_matches(&actual.path,&actual.identity,&file.identity,authority)? || actual.mode != file.mode {
                return Err(changed());
            }
            files.insert(file.target_name.clone(), actual);
        }
        // Bind the namespace to its original root identity. Admission never
        // substitutes a current device number into this digest.
        let expected_root_id=root_id(root,&f.root_identity,control)?;
        if f.owned_block.as_ref().is_some_and(|b|b.root_id!=expected_root_id){return Err(changed());}
    }
    // Even a no-op plan checks the XML/native block before retaining authority.
    font_config::patch_fonts_conf(
        conf.bytes(),
        footprint.and_then(|f| f.owned_block.as_ref()),
        footprint.and_then(|f| f.owned_block.as_ref()),
    )?;
    if directory_evidence(root, game)? != directory {
        return Err(changed());
    }
    Ok(Observed {
        directory,
        files,
        conf,
    })
}
pub(crate) fn verify_current_with(store: &Store, doc: &JsonValue,authority:Option<&FontAuthority>) -> Result<(), NativeError> {
    let state = FontState::decode(doc)?;
    if let Some(f) = &state.installed {
        let game = pin_game(doc, &f.game_path)?;
        observe(
            store.root(),
            &game,
            Some(f),
            &mut ReadBudget::new(),
            &PrepareControl::never_cancelled(),authority,
        )?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum FontChange {
    Install {
        mod_id: String,
        font_ids: Vec<String>,
    },
    Remove {
        mod_id: String,
        font_ids: Vec<String>,
    },
    Undo,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedFont {
    pub target_name: String,
    pub ownership: FontFileOwnership,
    pub sha256: String,
    pub size: u64,
    pub mode: u32,
    pub owners: Vec<FontOwner>,
}
impl From<&InstalledFont> for PlannedFont {
    fn from(f: &InstalledFont) -> Self {
        Self {
            target_name: f.target_name.clone(),
            ownership: f.ownership.clone(),
            sha256: f.identity.sha256.clone(),
            size: f.identity.size,
            mode: f.mode,
            owners: f.owners.clone(),
        }
    }
}
/// Transaction consumers receive only a fully reviewed plan. Display summaries
/// never provide destinations, source handles or a partial committable selection.
#[derive(Clone)]
pub struct FontPlan {
    root: PathBuf,
    game: PathBuf,
    revision: u64,
    transaction_id: String,
    before_state: FontState,
    directory: FontDirectoryEvidence,
    changes: Vec<FontFileChange>,
    after_files: Vec<PlannedFont>,
    after_block: Option<FontBlock>,
    after_conf: Option<Arc<[u8]>>,
    after_conf_mode: Option<u32>,
    create_directory: bool,
    remove_directory: bool,
    directory_created: bool,
    undo: bool,
    root_identity: DirectoryIdentity,
    game_identity: DirectoryIdentity,
    document: JsonValue,
    observed: BTreeMap<PathBuf, CheckedFontImage>,
    sources: Vec<CheckedFontFile>,
    source_parents: BTreeMap<PathBuf, DirectoryIdentity>,
    can_commit: bool,
    summary: JsonValue,
}
fn selected_refs(
    doc: &JsonValue,
    mod_id: &str,
    ids: &[String],
) -> Result<Vec<FontRef>, NativeError> {
    if !paths::safe_id(mod_id) || ids.is_empty() || ids.len() > 256 {
        return Err(NativeError::params());
    }
    let selected: BTreeSet<_> = ids.iter().collect();
    if selected.len() != ids.len() || selected.iter().any(|s| !package::hex(s, 32)) {
        return Err(NativeError::params());
    }
    let mut canonical = None;
    let mut refs = None;
    for (_, profile) in doc["profiles"].entries() {
        let mut occurrences = 0;
        for record in profile["mods"]
            .members()
            .filter(|r| r["source"]["id"] == mod_id)
        {
            occurrences += 1;
            if occurrences > 1 {
                return Err(state_error());
            }
            let all = stored_refs(record)?;
            let encoded = &record["source"]["localImport"]["fonts"];
            if canonical.as_ref().is_some_and(|c| c != encoded) {
                return Err(state_error());
            }
            canonical = Some(encoded.clone());
            refs = Some(all);
        }
    }
    let refs: Vec<_> = refs
        .ok_or_else(|| {
            NativeError::new(
                "unknownFontOwner",
                "The stable source mod no longer exists in this library.",
            )
        })?
        .into_iter()
        .filter(|f| selected.contains(&f.asset.id))
        .collect();
    if refs.len() != ids.len() {
        return Err(NativeError::new(
            "unknownFont",
            "One or more selected immutable font assets are missing or ambiguous.",
        ));
    }
    Ok(refs)
}
fn collision(rows: &mut BTreeMap<String, Vec<String>>, id: &str, reason: &str) {
    let reasons = rows.entry(id.into()).or_default();
    if !reasons.iter().any(|r| r == reason) {
        reasons.push(reason.into());
    }
}
fn planned_limits(files: &[PlannedFont]) -> Result<(), NativeError> {
    if files.len() > MAX_INSTALLED_FONTS {
        return Err(limit());
    }
    let mut total = 0u64;
    let mut owners = 0usize;
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for f in files {
        total = total
            .checked_add(f.size)
            .filter(|n| *n <= MAX_FONT_JOB_BYTES)
            .ok_or_else(limit)?;
        owners = owners
            .checked_add(f.owners.len())
            .filter(|n| *n <= MAX_FONT_OWNERS)
            .ok_or_else(limit)?;
        if f.size > MAX_FONT_BYTES
            || f.owners.is_empty()
            || !names.insert(folded(&f.target_name))
            || f.owners
                .iter()
                .any(|o| !ids.insert((o.mod_id.clone(), o.asset_id.clone())))
        {
            return Err(state_error());
        }
    }
    Ok(())
}
fn ownership_delta(
    before: &[PlannedFont],
    after: &[PlannedFont],
    changes: &BTreeMap<FontTarget, FontFileChange>,
) -> Result<JsonValue, NativeError> {
    let before: BTreeMap<_, _> = before.iter().map(|f| (&f.target_name, f)).collect();
    let after: BTreeMap<_, _> = after.iter().map(|f| (&f.target_name, f)).collect();
    let names: BTreeSet<_> = before.keys().chain(after.keys()).copied().collect();
    let mut rows = Vec::new();
    for name in names {
        let old: BTreeSet<_> = before.get(name).into_iter().flat_map(|f| &f.owners).collect();
        let new: BTreeSet<_> = after.get(name).into_iter().flat_map(|f| &f.owners).collect();
        let added: Vec<_> = new.difference(&old).map(|o| owner_json(o)).collect();
        let released: Vec<_> = old.difference(&new).map(|o| owner_json(o)).collect();
        if added.is_empty() && released.is_empty() { continue; }
        let disposition = match changes.get(&FontTarget::Font(name.clone())) {
            Some(c) if c.after.is_some() => "writeExact",
            Some(_) => "restoreAbsence",
            None if before.get(name).or_else(|| after.get(name))
                .is_some_and(|f| f.ownership == FontFileOwnership::Borrowed) => "preserveBorrowed",
            None => "preserveFile",
        };
        rows.push(object! {targetName:name.clone(),added:JsonValue::from(added),released:JsonValue::from(released),
            beforeCount:old.len(),afterCount:new.len(),disposition:disposition});
    }
    if rows.len() > 512 { return Err(limit()); }
    Ok(JsonValue::from(rows))
}

fn planned_metadata_limit(
    root: &Path,
    root_identity: &DirectoryIdentity,
    game: &Path,
    game_identity: &DirectoryIdentity,
    directory: Option<DirectoryIdentity>,
    directory_created: bool,
    files: &[PlannedFont],
    conf: Option<&[u8]>,
    conf_mode: Option<u32>,
    block: Option<&FontBlock>,
    prior: Option<&FontFootprint>,
    changes: &BTreeMap<FontTarget, FontFileChange>,
    transaction_id: &str,
    undo: bool,
) -> Result<(), NativeError> {
    // Reserve serialized capacity for real post-write inode/timestamps, backup
    // refs and the complete previous footprint before offering a commit. The
    // placeholders are size estimates only and never leave this function.
    let placeholder = |sha: String, size: u64| FontFingerprint { fsid:None,
        sha256: sha,
        size,
        device: u64::MAX,
        inode: u64::MAX,
        modified_seconds: i64::MIN,
        modified_nanos: 999_999_999,
    };
    let projected = FontFootprint {
        root_path: root.into(),
        root_identity: root_identity.clone(),
        game_path: game.into(),
        game_identity: game_identity.clone(),
        directory,
        directory_created,
        files: files
            .iter()
            .map(|f| InstalledFont {
                target_name: f.target_name.clone(),
                ownership: f.ownership.clone(),
                identity: placeholder(f.sha256.clone(), f.size),
                mode: f.mode,
                owners: f.owners.clone(),
            })
            .collect(),
        conf: conf.map(|b| placeholder("0".repeat(64), b.len() as u64)),
        conf_mode,
        owned_block: block.cloned(),
    };
    let previous = if undo {
        None
    } else {
        Some(FontCheckpoint {
            transaction_id: transaction_id.into(),
            prior: prior.cloned(),
            images: changes
                .values()
                .map(|c| FontImage {
                    target: c.target.clone(),
                    image: match &c.before {
                        CheckedFontImage::Absent => StoredFontImage::Absent,
                        CheckedFontImage::Present(f) => StoredFontImage::Present {
                            backup: FileRef {
                                id: "0".repeat(32),
                                sha256: f.identity.sha256.clone(),
                                size: f.identity.size,
                                source_name: "x".repeat(255),
                            },
                            identity: placeholder(f.identity.sha256.clone(), f.identity.size),
                            mode: f.mode,
                        },
                    },
                })
                .collect(),
        })
    };
    let mut state = FontState {
        installed: if undo && prior.is_none() {
            None
        } else {
            Some(projected)
        },
        previous,
    };
    retain_directories(&mut state);
    state.encode()?;
    Ok(())
}
impl FontPlan {
    pub fn review(store: &Store, game: &Path, change: FontChange) -> Result<Self, NativeError> {
        Self::review_controlled(store, game, change, &PrepareControl::never_cancelled())
    }
    pub fn review_controlled(store:&Store,game:&Path,change:FontChange,control:&PrepareControl)->Result<Self,NativeError>{Self::review_with(store,game,change,control,None)}
    fn review_with(
        store: &Store,
        game: &Path,
        change: FontChange,
        control: &PrepareControl,
        authority:Option<&FontAuthority>,
    ) -> Result<Self, NativeError> {
        crate::apply::ensure_idle(store)?;
        let doc = store.document()?;
        crate::store::ensure_readable(&doc)?;
        if doc["recoveryRequired"] == true {
            return Err(changed());
        }
        let game = pin_game(&doc, game)?;
        let root = store.root().to_owned();
        if root.to_str().is_none() || fs::canonicalize(&root)? != root {
            return Err(changed());
        }
        let revision = store::revision(doc["revision"].as_str().ok_or_else(state_error)?)?;
        let before_state = FontState::decode(&doc)?;
        let root_identity = directory_identity(&root)?;
        let game_identity = directory_identity(&game)?;
        let namespace_identity=before_state.installed.as_ref().map(|f|&f.root_identity).unwrap_or(&root_identity);
        let native_root_id = before_state.installed.as_ref().and_then(|f|f.owned_block.as_ref()).map(|b|Ok(b.root_id.clone())).unwrap_or_else(||root_id(&root,namespace_identity,control))?;
        let mut budget = ReadBudget::new();
        let current = observe(
            &root,
            &game,
            before_state.installed.as_ref(),
            &mut budget,
            control,authority,
        )?;
        let mut observed = BTreeMap::new();
        observed.insert(
            target_path(&game, &FontTarget::Configuration)?,
            current.conf.clone(),
        );
        for file in current.files.values() {
            observed.insert(file.path.clone(), CheckedFontImage::Present(file.clone()));
        }
        let mut after_files: Vec<PlannedFont> = before_state
            .installed
            .iter()
            .flat_map(|f| f.files.iter().map(PlannedFont::from))
            .collect();
        let mut sources = Vec::new();
        let mut source_parents = BTreeMap::new();
        let mut changes = BTreeMap::<FontTarget, FontFileChange>::new();
        let mut collisions = BTreeMap::new();
        let mut warnings = BTreeSet::<String>::new();
        let mut rows = Vec::new();
        let transaction_id = paths::random_id()?;
        // Target directories are observations, never deletion authority.
        let remove_directory = false;
        let directory_created = false;
        let undo = matches!(change, FontChange::Undo);
        let (action, mod_id, requested_ids) = match &change {
            FontChange::Install { mod_id, font_ids } => {
                ("install", Some(mod_id.clone()), font_ids.clone())
            }
            FontChange::Remove { mod_id, font_ids } => {
                ("remove", Some(mod_id.clone()), font_ids.clone())
            }
            FontChange::Undo => ("undo", None, Vec::new()),
        };
        let mut after_block = None;
        let mut after_conf = current.conf.bytes().map(Arc::<[u8]>::from);
        let mut after_conf_mode = match &current.conf {
            CheckedFontImage::Absent => None,
            CheckedFontImage::Present(f) => Some(f.mode),
        };
        match &change {
            FontChange::Install { mod_id, font_ids } => {
                let refs = selected_refs(&doc, mod_id, font_ids)?;
                // Diagnose every selected pair before projecting any writes.
                for a in &refs {
                    for b in &refs {
                        if a.asset.id != b.asset.id
                            && folded(&a.target_name) == folded(&b.target_name)
                            && (a.asset.sha256 != b.asset.sha256 || a.asset.size != b.asset.size)
                        {
                            collision(&mut collisions,&a.asset.id,"Selected fonts request the same case-folded basename with different bytes.");
                        }
                    }
                }
                for font in refs {
                    control.check()?;
                    let source = read_source(
                        &root,
                        &root_identity,
                        &mut source_parents,
                        &font_path(&root, &font.asset.id)?,
                        MAX_FONT_BYTES,
                        &mut budget,
                        control,
                    )?;
                    if source.identity.sha256 != font.asset.sha256
                        || source.identity.size != font.asset.size
                    {
                        return Err(changed());
                    }
                    let owner = FontOwner {
                        mod_id: mod_id.clone(),
                        asset_id: font.asset.id.clone(),
                        display_name: font.display_name.clone(),
                    };
                    let key = folded(&font.target_name);
                    let siblings: Vec<_> = current
                        .directory
                        .names
                        .iter()
                        .filter(|name| folded(name) == key)
                        .cloned()
                        .collect();
                    if siblings.len() > 1 {
                        collision(
                            &mut collisions,
                            &font.asset.id,
                            "Multiple case-folded target siblings make this basename ambiguous.",
                        );
                    }
                    let mut target = siblings
                        .first()
                        .cloned()
                        .unwrap_or_else(|| font.target_name.clone());
                    let existing_index = after_files
                        .iter()
                        .position(|f| folded(&f.target_name) == key);
                    if let Some(i) = existing_index {
                        target = after_files[i].target_name.clone();
                    }
                    let target_key = FontTarget::Font(target.clone());
                    let path = target_path(&game, &target_key)?;
                    let before = if let Some(image) = observed.get(&path) {
                        Some(image.clone())
                    } else {
                        match fs::symlink_metadata(&path) {
                            Ok(m)
                                if !m.is_file()
                                    || m.file_type().is_symlink()
                                    || m.nlink() != 1
                                    || m.mode() & 0o7000 != 0 =>
                            {
                                collision(&mut collisions,&font.asset.id,"The target is a symlink, hardlink, special file, or has unsupported permission bits.");
                                None
                            }
                            Ok(m) if m.len() > MAX_FONT_BYTES => {
                                collision(&mut collisions,&font.asset.id,"The unknown target exceeds the font byte limit and cannot be adopted or replaced.");
                                None
                            }
                            _ => {
                                let image =
                                    read_optional(&path, MAX_FONT_BYTES, &mut budget, control)?;
                                observed.insert(path.clone(), image.clone());
                                Some(image)
                            }
                        }
                    };
                    let identical = before.as_ref().is_some_and(|b|matches!(b,CheckedFontImage::Present(f) if f.identity.sha256 == font.asset.sha256 && f.identity.size == font.asset.size));
                    let mut disposition = "create";
                    if let Some(i) = existing_index {
                        let file = &after_files[i];
                        if file.sha256 != font.asset.sha256 || file.size != font.asset.size {
                            if file.ownership == FontFileOwnership::Borrowed {
                                collision(
                                    &mut collisions,
                                    &font.asset.id,
                                    "Borrowed bytes may never be overwritten or adopted.",
                                );
                            } else if file.owners.iter().any(|o| o.mod_id != *mod_id) {
                                collision(
                                    &mut collisions,
                                    &font.asset.id,
                                    "Different bytes are still owned by another mod.",
                                );
                            } else {
                                disposition = "replace";
                            }
                        } else {
                            disposition = if file.ownership == FontFileOwnership::Borrowed {
                                "borrow"
                            } else {
                                "share"
                            };
                        }
                    } else if matches!(before, Some(CheckedFontImage::Present(_))) {
                        if identical {
                            disposition = "borrow";
                        } else {
                            collision(
                                &mut collisions,
                                &font.asset.id,
                                "Unknown target bytes differ; this file will not be overwritten.",
                            );
                        }
                    }
                    let before_sha = before.as_ref().and_then(|b| match b {
                        CheckedFontImage::Present(f) => Some(f.identity.sha256.clone()),
                        CheckedFontImage::Absent => None,
                    });
                    let before_size = before.as_ref().and_then(|b| match b {
                        CheckedFontImage::Present(f) => Some(f.identity.size),
                        CheckedFontImage::Absent => None,
                    });
                    let released_owners = if disposition == "replace" {
                        existing_index
                            .map(|i| {
                                after_files[i]
                                    .owners
                                    .iter()
                                    .map(owner_json)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    if !collisions.contains_key(&font.asset.id) {
                        if let Some(i) = existing_index {
                            let f = &mut after_files[i];
                            if disposition == "replace" {
                                // Explicit reviewed replacement releases this source mod's
                                // former asset owners, while preserving its exact old bytes.
                                f.owners.clear();
                                f.sha256 = font.asset.sha256.clone();
                                f.size = font.asset.size;
                            }
                            if !f
                                .owners
                                .iter()
                                .any(|o| o.mod_id == owner.mod_id && o.asset_id == owner.asset_id)
                            {
                                f.owners.push(owner);
                                f.owners.sort();
                            }
                        } else {
                            let file_mode = match &before {
                                Some(CheckedFontImage::Present(f)) => f.mode,
                                _ => 0o644,
                            };
                            after_files.push(PlannedFont {
                                target_name: target.clone(),
                                ownership: if identical {
                                    FontFileOwnership::Borrowed
                                } else {
                                    FontFileOwnership::Created
                                },
                                sha256: font.asset.sha256.clone(),
                                size: font.asset.size,
                                mode: file_mode,
                                owners: vec![owner],
                            });
                        }
                        if disposition == "create" || disposition == "replace" {
                            let before = before.ok_or_else(changed)?;
                            let file_mode = match &before {
                                CheckedFontImage::Present(f) => f.mode,
                                CheckedFontImage::Absent => 0o644,
                            };
                            changes.insert(
                                target_key.clone(),
                                FontFileChange {
                                    target: target_key,
                                    before,
                                    after: Some(source.bytes.clone()),
                                    mode: file_mode,
                                },
                            );
                        }
                    }
                    rows.push(object! {fontId:font.asset.id.clone(),targetName:target,displayName:font.display_name,disposition:if collisions.contains_key(&font.asset.id){"blocked"}else{disposition},size:font.asset.size,sha256:font.asset.sha256,beforeSha256:before_sha,beforeSize:before_size,releasedOwners:JsonValue::from(released_owners)});
                    sources.push(source);
                }
            }
            FontChange::Remove { mod_id, font_ids } => {
                if !paths::safe_id(mod_id)
                    || font_ids.is_empty()
                    || font_ids.len() > 256
                    || font_ids.iter().any(|id| !package::hex(id, 32))
                    || font_ids.iter().collect::<BTreeSet<_>>().len() != font_ids.len()
                {
                    return Err(NativeError::params());
                }
                // Removal remains possible after a mod leaves the library; the
                // reviewed global owner pair is authority, not profile copies.
                for id in font_ids {
                    let Some(i) = after_files.iter().position(|f| {
                        f.owners
                            .iter()
                            .any(|o| o.mod_id == *mod_id && o.asset_id == *id)
                    }) else {
                        collision(
                            &mut collisions,
                            id,
                            "The selected global font owner is not installed.",
                        );
                        continue;
                    };
                    let f = &mut after_files[i];
                    f.owners
                        .retain(|o| !(o.mod_id == *mod_id && o.asset_id == *id));
                    let last = f.owners.is_empty();
                    let delete = last && f.ownership == FontFileOwnership::Created;
                    rows.push(object! {fontId:id.clone(),targetName:f.target_name.clone(),remainingOwnerCount:f.owners.len(),disposition:if delete{"delete"}else if last{"releaseBorrowed"}else{"releaseOwner"}});
                    if delete {
                        let target = FontTarget::Font(f.target_name.clone());
                        changes.insert(
                            target.clone(),
                            FontFileChange {
                                before: observed
                                    .get(&target_path(&game, &target)?)
                                    .ok_or_else(changed)?
                                    .clone(),
                                target,
                                after: None,
                                mode: f.mode,
                            },
                        );
                    }
                    if last {
                        after_files.remove(i);
                    }
                }
            }
            FontChange::Undo => {
                let checkpoint = before_state.previous.as_ref().ok_or_else(|| {
                    NativeError::new(
                        "nothingToUndo",
                        "There is no previous font transaction to undo.",
                    )
                })?;
                after_files = checkpoint
                    .prior
                    .iter()
                    .flat_map(|f| f.files.iter().map(PlannedFont::from))
                    .collect();
                after_block = checkpoint
                    .prior
                    .as_ref()
                    .and_then(|f| f.owned_block.clone());
                for image in &checkpoint.images {
                    if let FontTarget::Font(name) = &image.target {
                        let siblings: Vec<_> = current
                            .directory
                            .names
                            .iter()
                            .filter(|n| folded(n) == folded(name))
                            .collect();
                        if siblings.len() > 1 || siblings.first().is_some_and(|n| *n != name) {
                            return Err(changed());
                        }
                    }
                    let path = target_path(&game, &image.target)?;
                    let max = match image.target {
                        FontTarget::Configuration => font_config::MAX_CONF_BYTES as u64,
                        FontTarget::Font(_) => MAX_FONT_BYTES,
                    };
                    let before = if let Some(v) = observed.get(&path) {
                        v.clone()
                    } else {
                        let v = read_optional(&path, max, &mut budget, control)?;
                        observed.insert(path.clone(), v.clone());
                        v
                    };
                    let (after, file_mode) = match &image.image {
                        StoredFontImage::Absent => (None, 0o644),
                        StoredFontImage::Present {
                            backup,
                            identity,
                            mode,
                        } => {
                            let file = read_source(
                                &root,
                                &root_identity,
                                &mut source_parents,
                                &backup_path(&root, &backup.id)?,
                                max,
                                &mut budget,
                                control,
                            )?;
                            if !fingerprint_matches(&file.path,&file.identity,identity,authority)?
                                || file.identity.sha256 != backup.sha256
                                || file.identity.size != backup.size
                                || file.mode != 0o400
                            {
                                return Err(changed());
                            }
                            let bytes = file.bytes.clone();
                            sources.push(file);
                            (Some(bytes), *mode)
                        }
                    };
                    if let FontTarget::Font(name) = &image.target {
                        let prior = checkpoint
                            .prior
                            .as_ref()
                            .and_then(|f| f.files.iter().find(|f| f.target_name == *name));
                        let current = before_state
                            .installed
                            .as_ref()
                            .and_then(|f| f.files.iter().find(|f| f.target_name == *name));
                        if prior.is_some_and(|f| f.ownership == FontFileOwnership::Borrowed)
                            || current.is_some_and(|f| f.ownership == FontFileOwnership::Borrowed)
                        {
                            return Err(state_error());
                        }
                        match (&after, prior) {
                            (None, None) if current.is_some() => (),
                            (Some(bytes), Some(prior))
                                if bytes.len() as u64 == prior.identity.size
                                    && prepare_control::digest_bounded_controlled(
                                        bytes,
                                        MAX_FONT_BYTES as usize,
                                        control,
                                    )? == prior.identity.sha256
                                    && file_mode == prior.mode =>
                            {
                                ()
                            }
                            _ => return Err(state_error()),
                        }
                        if current.is_none() && !matches!(before, CheckedFontImage::Absent) {
                            return Err(changed());
                        }
                    } else {
                        after_conf = after.clone();
                        after_conf_mode = after.as_ref().map(|_| file_mode);
                    }
                    rows.push(object!{target:target_json(&image.target),disposition:if after.is_some(){"restoreExact"}else{"restoreAbsence"}});
                    changes.insert(
                        image.target.clone(),
                        FontFileChange {
                            target: image.target.clone(),
                            before,
                            after,
                            mode: file_mode,
                        },
                    );
                }
                // Files whose owners alone changed need no image, but are still
                // checked before restoring their former ownership (incl. borrowed).
                for planned in &after_files {
                    let siblings: Vec<_> = current
                        .directory
                        .names
                        .iter()
                        .filter(|n| folded(n) == folded(&planned.target_name))
                        .collect();
                    if siblings.len() > 1
                        || siblings.first().is_some_and(|n| **n != planned.target_name)
                    {
                        return Err(changed());
                    }
                    let target = FontTarget::Font(planned.target_name.clone());
                    if !changes.contains_key(&target) {
                        let path = target_path(&game, &target)?;
                        let image = if let Some(v) = observed.get(&path) {
                            v.clone()
                        } else {
                            let v = read_optional(&path, MAX_FONT_BYTES, &mut budget, control)?;
                            observed.insert(path, v.clone());
                            v
                        };
                        let prior = checkpoint
                            .prior
                            .as_ref()
                            .and_then(|f| {
                                f.files
                                    .iter()
                                    .find(|f| f.target_name == planned.target_name)
                            })
                            .ok_or_else(state_error)?;
                        check_persisted_file(&image,Some((&prior.identity,prior.mode)),authority)?;
                    }
                }
                // Undo never reconstructs a former configuration from markers.
                // The exact stored image must also admit its former owned block.
                font_config::patch_fonts_conf(
                    after_conf.as_deref(),
                    after_block.as_ref(),
                    after_block.as_ref(),
                )?;
                if let Some(prior) = &checkpoint.prior {
                    match (&after_conf, &prior.conf) {
                        (None, None) => (),
                        (Some(bytes), Some(fp))
                            if bytes.len() as u64 == fp.size
                                && prepare_control::digest_bounded_controlled(
                                    bytes,
                                    font_config::MAX_CONF_BYTES,
                                    control,
                                )? == fp.sha256
                                && after_conf_mode == prior.conf_mode =>
                        {
                            ()
                        }
                        _ => return Err(state_error()),
                    }
                }
            }
        }
        after_files.sort_by(|a, b| a.target_name.cmp(&b.target_name));
        planned_limits(&after_files)?;
        // Different filenames sharing names remain installable; there is no
        // assumed rendering precedence or equivalence of faces.
        let mut family_targets = BTreeMap::<String, BTreeSet<String>>::new();
        for f in &after_files {
            for owner in &f.owners {
                family_targets
                    .entry(folded(&owner.display_name))
                    .or_default()
                    .insert(f.target_name.clone());
            }
        }
        for targets in family_targets.values().filter(|t| t.len() > 1) {
            warnings.insert(format!("Different font filenames share a family name: {}. Game font matching and rendering precedence are unqualified.",targets.iter().cloned().collect::<Vec<_>>().join(", ")));
        }
        if !undo {
            let names: Vec<_> = after_files
                .iter()
                .flat_map(|f| f.owners.iter().map(|o| o.display_name.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let old = before_state
                .installed
                .as_ref()
                .and_then(|f| f.owned_block.as_ref());
            if current.conf.bytes().is_none() {
                warnings.insert(font_config::MISSING_CONF_WARNING.into());
            }
            if !names.is_empty() && current.conf.bytes().is_some() {
                // Keep byte-identical blocks on owner-only changes whose aggregate
                // pattern set is unchanged; other changes get this review's ID.
                after_block = if old.is_some_and(|b| b.names == names) {
                    old.cloned()
                } else {
                    Some(font_config::render_block(
                        &native_root_id,
                        &transaction_id,
                        names,
                        current
                            .conf
                            .bytes()
                            .is_some_and(|b| b.windows(2).any(|w| w == b"\r\n")),
                    )?)
                };
            }
            after_conf =
                font_config::patch_fonts_conf(current.conf.bytes(), old, after_block.as_ref())?
                    .map(Arc::from);
            if current.conf.bytes() != after_conf.as_deref() {
                changes.insert(
                    FontTarget::Configuration,
                    FontFileChange {
                        target: FontTarget::Configuration,
                        before: current.conf.clone(),
                        after: after_conf.clone(),
                        mode: after_conf_mode.unwrap_or(0o644),
                    },
                );
            }
        }
        let create_directory = current.directory.directory.is_none() && !after_files.is_empty();
        warnings.insert("Game font directories are retained after removal, rollback and undo, including directories ensured by this change. Exact restoration applies to files and configuration, not directory absence.".into());
        for row in &mut rows {
            if let Some(id) = row["fontId"].as_str().map(str::to_owned) {
                row["collisions"] =
                    JsonValue::from(collisions.get(&id).cloned().unwrap_or_default());
                row["available"] = (!collisions.contains_key(&id)).into();
            }
        }
        let original_files: Vec<_> = before_state
            .installed
            .iter()
            .flat_map(|f| f.files.iter().map(PlannedFont::from))
            .collect();
        let has_changes = undo
            || after_files != original_files
            || !changes.is_empty()
            || create_directory
            || remove_directory;
        if !has_changes {
            warnings.insert("The requested font ownership already matches the installed setup; there is no font change to commit.".into());
        }
        if changes.len() > MAX_FONT_TRANSACTION_TARGETS {
            return Err(limit());
        }
        let can_commit = collisions.is_empty() && has_changes;
        if can_commit {
            planned_metadata_limit(
                &root,
                &root_identity,
                &game,
                &game_identity,
                if remove_directory {
                    None
                } else if create_directory {
                    Some(DirectoryIdentity { fsid:None,
                        device: u64::MAX,
                        inode: u64::MAX,
                    })
                } else {
                    current.directory.directory.clone()
                },
                directory_created,
                &after_files,
                after_conf.as_deref(),
                after_conf_mode,
                after_block.as_ref(),
                if undo {
                    before_state
                        .previous
                        .as_ref()
                        .and_then(|p| p.prior.as_ref())
                } else {
                    before_state.installed.as_ref()
                },
                &changes,
                &transaction_id,
                undo,
            )?;
        }
        let total_bytes: u64 = after_files.iter().map(|f| f.size).sum();
        let write_bytes: u64 = changes
            .values()
            .filter_map(|c| c.after.as_ref())
            .map(|b| b.len() as u64)
            .sum();
        let backup_bytes: u64 = changes
            .values()
            .filter_map(|c| c.before.bytes())
            .map(|b| b.len() as u64)
            .sum();
        // Exact app-owned pattern projection from already admitted typed state.
        // Unknown game patterns stay outside this display-only projection.
        let configuration_before = current.conf.bytes().is_some();
        let configuration_after = after_conf.is_some();
        let before_names = if configuration_before {
            before_state.installed.as_ref().and_then(|f| f.owned_block.as_ref())
                .map(|b| b.names.clone()).unwrap_or_default()
        } else { Vec::new() };
        let after_names = if configuration_after {
            after_block.as_ref().map(|b| b.names.clone()).unwrap_or_default()
        } else { Vec::new() };
        let pattern_changes = object! {configurationBefore:configuration_before,configurationAfter:configuration_after,
            before:JsonValue::from(before_names),after:JsonValue::from(after_names)};
        let summary = object! {version:1,action:action,modId:mod_id,fontIds:JsonValue::from(requested_ids),gamePath:game.to_string_lossy().as_ref(),revision:revision.to_string(),canCommit:can_commit,hasChanges:has_changes,fonts:JsonValue::from(rows),collisions:JsonValue::from(collisions.iter().map(|(id,reasons)|object!{fontId:id.clone(),reasons:JsonValue::from(reasons.clone())}).collect::<Vec<_>>()),warnings:JsonValue::from(warnings.into_iter().collect::<Vec<_>>()),installedFileCount:after_files.len(),totalFontBytes:total_bytes,writeBytes:write_bytes,backupBytes:backup_bytes,createDirectory:create_directory,removeDirectory:remove_directory,patternChanges:pattern_changes,ownerChanges:ownership_delta(&original_files, &after_files, &changes)?};
        if summary.dump().len() > MAX_FONT_STATE_BYTES {
            return Err(limit());
        }
        verify_source_parents(&source_parents)?;
        if directory_evidence(&root, &game)? != current.directory || store.document()? != doc {
            return Err(changed());
        }
        // A blocked display projection carries no writable changes or proposed
        // owner state. Task4 must obtain a fresh plan after exclusions.
        if !can_commit {
            changes.clear();
            after_files.clear();
            after_block = None;
            after_conf = None;
        }
        Ok(Self {
            root,
            game,
            revision,
            transaction_id,
            before_state,
            directory: current.directory,
            changes: changes.into_values().collect(),
            after_files,
            after_block,
            after_conf,
            after_conf_mode,
            create_directory,
            remove_directory,
            directory_created,
            undo,
            root_identity,
            game_identity,
            document: doc,
            observed,
            sources,
            source_parents,
            can_commit,
            summary,
        })
    }
    pub fn summary(&self) -> &JsonValue {
        &self.summary
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn before_state(&self) -> &FontState {
        &self.before_state
    }
    pub fn directory_evidence(&self) -> &FontDirectoryEvidence {
        &self.directory
    }
    pub fn creates_directory(&self) -> bool {
        self.can_commit && self.create_directory
    }
    pub fn removes_directory(&self) -> bool {
        self.can_commit && self.remove_directory
    }
    pub fn is_undo(&self) -> bool {
        self.undo
    }
    /// Projection is journal intent only; changed identities are rebound from
    /// recorded publication evidence before this becomes a committed footprint.
    pub(crate) fn transaction_projection(&self) -> Result<Option<FontFootprint>, NativeError> {
        self.checked_changes()?;
        if self.undo && self.before_state.previous.as_ref().is_some_and(|p| p.prior.is_none()) {
            return Ok(None);
        }
        let fp = |bytes: &[u8]| -> Result<FontFingerprint, NativeError> {
            Ok(FontFingerprint { fsid:None,sha256: prepare_control::digest_bounded_controlled(bytes,
                MAX_FONT_BYTES as usize, &PrepareControl::never_cancelled())?,size:bytes.len() as u64,
                device:u64::MAX,inode:u64::MAX,modified_seconds:i64::MIN,modified_nanos:999_999_999})
        };
        let identity = |target: &FontTarget| -> Result<Option<FontFingerprint>,NativeError> {
            if let Some(c)=self.changes.iter().find(|c| &c.target==target) { return c.after.as_deref().map(fp).transpose(); }
            if let Some(old)=self.recorded_before(target){return Ok(Some(old.clone()));}
            Ok(match self.observed.get(&target_path(&self.game,target)?) {
                Some(CheckedFontImage::Present(f))=>Some(f.identity.clone()),Some(CheckedFontImage::Absent)=>None,
                None=>return Err(changed())})
        };
        Ok(Some(FontFootprint {root_path:self.root.clone(),root_identity:self.before_state.installed.as_ref().map(|f|f.root_identity.clone()).unwrap_or_else(||self.root_identity.clone()),
            game_path:self.game.clone(),game_identity:self.before_state.installed.as_ref().map(|f|f.game_identity.clone()).unwrap_or_else(||self.game_identity.clone()),
            directory:if self.remove_directory {None} else if self.create_directory {Some(DirectoryIdentity{ fsid:None,device:u64::MAX,inode:u64::MAX})} else {self.before_state.installed.as_ref().and_then(|f|f.directory.clone()).or_else(||self.directory.directory.clone())},
            directory_created:self.directory_created,files:self.after_files.iter().map(|f|Ok(InstalledFont{
                target_name:f.target_name.clone(),ownership:f.ownership.clone(),identity:identity(&FontTarget::Font(f.target_name.clone()))?.ok_or_else(changed)?,mode:f.mode,owners:f.owners.clone()
            })).collect::<Result<Vec<_>,NativeError>>()?,conf:identity(&FontTarget::Configuration)?,conf_mode:self.after_conf_mode,owned_block:self.after_block.clone()}))
    }
    pub(crate) fn document(&self) -> &JsonValue {
        &self.document
    }
    pub fn can_commit(&self) -> bool {
        self.can_commit
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn game(&self) -> &Path {
        &self.game
    }
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }
    pub fn checked_changes(&self) -> Result<&[FontFileChange], NativeError> {
        if self.changes.len() > MAX_FONT_TRANSACTION_TARGETS {
            return Err(limit());
        }
        if !self.can_commit {
            return Err(NativeError::new(
                "blockedFonts",
                "Exclude blocked fonts and review the complete request again.",
            ));
        }
        Ok(&self.changes)
    }
    /// Task4 calls this under both root/game locks immediately before journaling
    /// and writing. No retained JSON token grants filesystem authority.
    pub fn revalidate(
        &self,
        store: &Store,
        expected: u64,
        control: &PrepareControl,
    ) -> Result<(), NativeError> {
        self.checked_changes()?;
        crate::apply::ensure_idle(store)?;
        if store.root() != self.root
            || expected != self.revision
            || store.document()? != self.document
        {
            return Err(changed());
        }
        self.revalidate_evidence(control)?;
        if store.document()? != self.document {
            return Err(changed());
        }
        Ok(())
    }
    /// Also usable after Task4 journals/backups in the private root but before
    /// the first game mutation. The caller must separately match its own journal
    /// and unchanged document while holding both locks.
    pub fn revalidate_evidence(&self, control: &PrepareControl) -> Result<(), NativeError> {
        self.checked_changes()?;
        verify_source_parents(&self.source_parents)?;
        if directory_identity(&self.root)? != self.root_identity
            || directory_identity(&self.game)? != self.game_identity
            || directory_evidence(&self.root, &self.game)? != self.directory
        {
            return Err(changed());
        }
        let mut budget = ReadBudget::new();
        for (path, image) in &self.observed {
            let max = if path.file_name().is_some_and(|n| n == "fonts.conf") {
                font_config::MAX_CONF_BYTES as u64
            } else {
                MAX_FONT_BYTES
            };
            let actual = read_optional(path, max, &mut budget, control)?;
            let expected = match image {
                CheckedFontImage::Absent => None,
                CheckedFontImage::Present(f) => Some((&f.identity, f.mode)),
            };
            check_file(&actual, expected)?;
        }
        for source in &self.sources {
            verify_source_parents(&self.source_parents)?;
            let max = if source.identity.size <= font_config::MAX_CONF_BYTES as u64 {
                font_config::MAX_CONF_BYTES as u64
            } else {
                MAX_FONT_BYTES
            };
            let actual = read_file(&source.path, max, &mut budget, control)?;
            if actual.identity != source.identity || actual.mode != source.mode {
                return Err(changed());
            }
            verify_source_parents(&self.source_parents)?;
        }
        verify_source_parents(&self.source_parents)?;
        if directory_evidence(&self.root, &self.game)? != self.directory {
            return Err(changed());
        }
        Ok(())
    }
    /// After Task4 has durably applied all checked images, capture real new file
    /// identities. Never persist preview fingerprints as newly written identity.
    /// The borrowed directory observation and written-file identities must come
    /// from the journal; neither pathname observation nor byte equality grants
    /// ownership of an external replacement.
    pub(crate) fn installed_after(
        &self,
        observed: Option<&DirectoryIdentity>,
        written: &BTreeMap<FontTarget, FontFingerprint>,
        control: &PrepareControl,
    ) -> Result<Option<FontFootprint>, NativeError> {
        self.checked_changes()?;
        let expected_written: BTreeSet<_> = self
            .changes
            .iter()
            .filter(|c| c.after.is_some())
            .map(|c| &c.target)
            .collect();
        if written.keys().collect::<BTreeSet<_>>() != expected_written {
            return Err(changed());
        }
        for (path, identity) in &self.directory.parents {
            if directory_identity(path)? != *identity {
                return Err(changed());
            }
        }
        let now = directory_evidence(&self.root, &self.game)?;
        let expected_directory = if self.remove_directory {
            None
        } else if self.create_directory {
            Some(observed.ok_or_else(changed)?.clone())
        } else {
            self.directory.directory.clone()
        };
        if now.directory != expected_directory
            || directory_identity(&self.game)? != self.game_identity
        {
            return Err(changed());
        }
        let mut expected_names: BTreeSet<_> = self.directory.names.iter().cloned().collect();
        for change in &self.changes {
            let name = match &change.target {
                FontTarget::Configuration => "fonts.conf",
                FontTarget::Font(name) => name,
            };
            if change.after.is_some() {
                expected_names.insert(name.to_owned());
            } else {
                expected_names.remove(name);
            }
        }
        if now.names.iter().cloned().collect::<BTreeSet<_>>() != expected_names {
            return Err(changed());
        }
        let mut budget = ReadBudget::new();
        let mut files = Vec::new();
        for planned in &self.after_files {
            let actual = read_file(
                &target_path(&self.game, &FontTarget::Font(planned.target_name.clone()))?,
                MAX_FONT_BYTES,
                &mut budget,
                control,
            )?;
            if actual.identity.sha256 != planned.sha256
                || actual.identity.size != planned.size
                || actual.mode != planned.mode
            {
                return Err(changed());
            }
            if let Some(written) = written.get(&FontTarget::Font(planned.target_name.clone())) {
                if actual.identity != *written {
                    return Err(changed());
                }
            }
            // Unwritten and borrowed files must retain their reviewed identities.
            if !self
                .changes
                .iter()
                .any(|c| c.target == FontTarget::Font(planned.target_name.clone()))
            {
                if let Some(image) = self.observed.get(&actual.path) {
                    check_file(
                        &CheckedFontImage::Present(actual.clone()),
                        match image {
                            CheckedFontImage::Present(f) => Some((&f.identity, f.mode)),
                            CheckedFontImage::Absent => None,
                        },
                    )?;
                }
            }
            files.push(InstalledFont {
                target_name: planned.target_name.clone(),
                ownership: planned.ownership.clone(),
                identity:if !self.changes.iter().any(|c|c.target==FontTarget::Font(planned.target_name.clone())) {self.recorded_before(&FontTarget::Font(planned.target_name.clone())).cloned().unwrap_or(actual.identity)}else{actual.identity},
                mode: actual.mode,
                owners: planned.owners.clone(),
            });
        }
        let conf = read_optional(
            &target_path(&self.game, &FontTarget::Configuration)?,
            font_config::MAX_CONF_BYTES as u64,
            &mut budget,
            control,
        )?;
        if conf.bytes() != self.after_conf.as_deref() {
            return Err(changed());
        }
        if let Some(written) = written.get(&FontTarget::Configuration) {
            if !matches!(&conf,CheckedFontImage::Present(f) if f.identity == *written) {
                return Err(changed());
            }
        } else if !self
            .changes
            .iter()
            .any(|c| c.target == FontTarget::Configuration)
        {
            let expected = match self
                .observed
                .get(&target_path(&self.game, &FontTarget::Configuration)?)
            {
                Some(CheckedFontImage::Present(f)) => Some((&f.identity, f.mode)),
                Some(CheckedFontImage::Absent) => None,
                None => return Err(changed()),
            };
            check_file(&conf, expected)?;
        }
        let (conf, conf_mode) = match conf {
            CheckedFontImage::Absent => (None, None),
            CheckedFontImage::Present(f) => {
                if Some(f.mode) != self.after_conf_mode {
                    return Err(changed());
                }
                (Some(if !self.changes.iter().any(|c|c.target==FontTarget::Configuration){self.recorded_before(&FontTarget::Configuration).cloned().unwrap_or(f.identity)}else{f.identity}),Some(f.mode))
            }
        };
        if directory_evidence(&self.root, &self.game)? != now {
            return Err(changed());
        }
        for (path, identity) in &self.directory.parents {
            if directory_identity(path)? != *identity {
                return Err(changed());
            }
        }
        if self.undo
            && self
                .before_state
                .previous
                .as_ref()
                .is_some_and(|p| p.prior.is_none())
        {
            return Ok(None);
        }
        let f = FontFootprint {
            root_path: self.root.clone(),
            root_identity:self.before_state.installed.as_ref().map(|f|f.root_identity.clone()).unwrap_or_else(||self.root_identity.clone()),
            game_path: self.game.clone(),
            game_identity: self.before_state.installed.as_ref().map(|f|f.game_identity.clone()).unwrap_or_else(||self.game_identity.clone()),
            directory:if self.create_directory {now.directory}else{self.before_state.installed.as_ref().and_then(|f|f.directory.clone()).or(now.directory)},
            directory_created: self.directory_created,
            files,
            conf,
            conf_mode,
            owned_block: self.after_block.clone(),
        };
        footprint_decode(&footprint_json(&f))?;
        Ok(Some(f))
    }
}

// Legacy admission uses only the ordered locks retained by the caller.
pub(crate) fn verify_current_locked(store:&Store,doc:&JsonValue,root:&crate::store::RootLock,game:&crate::game_access::GameLock)->Result<(),NativeError>{
    let authority=FontAuthority::new(store,root,game)?;verify_current_with(store,doc,Some(&authority))
}
impl FontPlan {
    pub(crate) fn recorded_parent<'a>(&'a self,path:&Path,current:&'a DirectoryIdentity)->&'a DirectoryIdentity{
        if let Some(old)=&self.before_state.installed {if path==self.root{return &old.root_identity;}if path==self.game{return &old.game_identity;}}current
    }
    pub(crate) fn recorded_directory(&self)->Option<&DirectoryIdentity>{self.before_state.installed.as_ref().and_then(|f|f.directory.as_ref()).or(self.directory.directory.as_ref())}
    pub(crate) fn recorded_before(&self,target:&FontTarget)->Option<&FontFingerprint>{
        let old=self.before_state.installed.as_ref()?;
        match target{FontTarget::Configuration=>old.conf.as_ref(),FontTarget::Font(name)=>old.files.iter().find(|f|f.target_name==*name).map(|f|&f.identity)}
    }
    pub(crate) fn review_locked(store:&Store,game:&Path,change:FontChange,root:&crate::store::RootLock,guard:&crate::game_access::GameLock)->Result<Self,NativeError>{
        let authority=FontAuthority::new(store,root,guard)?;guard.validate(&game.join("game/citadel"))?;Self::review_with(store,game,change,&PrepareControl::never_cancelled(),Some(&authority))
    }
}
#[cfg(test)]
mod reboot_tests {
    use super::*;
    use crate::{store::RootLock,game_access::GameLock,test_support::{Fixture,import}};
    fn write_doc(st:&Store,doc:&JsonValue){let db=crate::sqlite::Database::open(&st.root().join("library.sqlite3"),true).unwrap();db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();}
    #[test]
    fn item93_font_legacy_reboot_preserves_block_and_anchor(){font_reboot(true,false,false)}
    #[test]
    fn item93_font_fsid_reboot_preserves_block_and_anchor(){font_reboot(false,false,false)}
    #[test]
    fn item93_font_borrowed_legacy_reboot(){font_reboot(true,true,false)}
    #[test]
    fn item93_font_reinstall_retains_legacy_namespace(){font_reboot(true,false,true)}
    fn font_reboot(legacy_format:bool,borrowed:bool,reinstall:bool) {
        let f=Fixture::new();import(&mut crate::native::NativeService::new(),&f);
        let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let game=f.game.clone();let c=game.join("game/citadel");
        fs::write(c.join("gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        if !paths::test_volatile_filesystem(&fs::File::open(&c).unwrap()){return;}
        let dir=c.join("panorama/fonts");fs::create_dir_all(&dir).unwrap();
        let font=dir.join("demo.ttf");fs::write(&font,b"font bytes retained by the recorded hash").unwrap();
        let mut old_root=directory_identity(&f.data).unwrap();old_root.device+=1;if legacy_format{old_root.fsid=None;}
        let block=font_config::render_block(&root_id(&f.data,&old_root,&PrepareControl::never_cancelled()).unwrap(),"1234567890abcdef1234567890abcdef" ,vec!["Demo".into()],false).unwrap();
        let bytes=font_config::patch_fonts_conf(Some(b"<fontconfig></fontconfig>\n"),None,Some(&block)).unwrap().unwrap();fs::write(dir.join("fonts.conf"),&bytes).unwrap();
        let footprint=FontFootprint{root_path:f.data.clone(),root_identity:directory_identity(&f.data).unwrap(),game_path:game.clone(),game_identity:directory_identity(&game).unwrap(),directory:Some(directory_identity(&dir).unwrap()),directory_created:false,
            files:vec![InstalledFont{target_name:"demo.ttf".into(),ownership:if borrowed{FontFileOwnership::Borrowed}else{FontFileOwnership::Created},identity:transaction_read(&font,MAX_FONT_BYTES).unwrap().identity,mode:0o644,owners:vec![FontOwner{mod_id:"a".into(),asset_id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),display_name:"Demo".into()},FontOwner{mod_id:"b".into(),asset_id:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),display_name:"Demo".into()}]}],
            conf:Some(transaction_read(&dir.join("fonts.conf"),font_config::MAX_CONF_BYTES as u64).unwrap().identity),conf_mode:Some(0o644),owned_block:Some(block.clone())};
        let mut doc=st.document().unwrap();doc["packageVersion"]=package::layout_version(&doc).unwrap().into();doc["storeVersion"]=4.into();doc["fonts"]=FontState{installed:Some(footprint),previous:None}.encode().unwrap();
        fn legacy(v:&mut JsonValue,strip:bool){if v.is_object(){if let Some(d)=v["device"].as_u64(){v["device"]=(d+1).into();if strip{v.remove("fsid");}}let keys=v.entries().map(|(k,_)|k.to_owned()).collect::<Vec<_>>();for k in keys{legacy(&mut v[&k],strip);}}else if v.is_array(){for i in 0..v.len(){legacy(&mut v[i],strip);}}}
        legacy(&mut doc["fonts"],legacy_format);write_doc(&st,&doc);
        let guard=GameLock::acquire(&c).unwrap();verify_current_locked(&st,&doc,&lock,&guard).unwrap();
        let change=||FontChange::Remove{mod_id:"a".into(),font_ids:vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()]};
        for target in ["rootIdentity","gameIdentity","directory","font","conf"] {for field in ["inode","fsid"]{
            let mut foreign=doc.clone();let id=if target=="font"{&mut foreign["fonts"]["installed"]["files"][0]["identity"]}else{&mut foreign["fonts"]["installed"][target]};
            id[field]=if field=="inode"{(id[field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};
            write_doc(&st,&foreign);assert!(verify_current_locked(&st,&foreign,&lock,&guard).is_err(),"target={target} field={field}");assert!(FontPlan::review_locked(&st,&game,change(),&lock,&guard).is_err());write_doc(&st,&doc);
        }}
        let plan=FontPlan::review_locked(&st,&game,change(),&lock,&guard).unwrap();drop(guard);
        let r=store::revision(doc["revision"].as_str().unwrap()).unwrap();plan.commit(&st,&lock,r,"font93",&object!{method:"native.fonts.commit",params:object!{operationId:"font93",expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef"}}).unwrap();
        let after=st.document().unwrap();assert_eq!(after["fonts"]["installed"]["gameIdentity"],doc["fonts"]["installed"]["gameIdentity"],"legacy anchor is not upgraded");
        assert_eq!(FontState::decode(&after).unwrap().installed.unwrap().owned_block,Some(block));assert_eq!(fs::read(dir.join("fonts.conf")).unwrap(),bytes);assert!(st.journals().unwrap().is_empty());
        let r=r+1;
        let request=object!{method:"native.apply",params:object!{operationId:"font-apply93",expectedRevision:r.to_string(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        let snapshot=crate::apply::ApplyPlan::review_locked(&st,&lock,"default",&game).unwrap().apply(&st,&lock,r,"font-apply93",&request).unwrap();assert_ne!(snapshot["recoveryRequired"],true);
        let guard=GameLock::acquire(&c).unwrap();let control=PrepareControl::never_cancelled();
        let play=crate::launch_preparation::PreparationPlan::review(&st,&lock,&guard,"default",&game,&control,&mut |_|{}).unwrap();let input=play.operation_input("font-play93").unwrap();
        let prepared=play.execute(&st,&lock,&guard,r+1,"font-play93",&input,&control,&mut |_|{}).unwrap();assert_ne!(prepared.snapshot["recoveryRequired"],true);prepared.proof.revalidate(&st,&lock,&guard).unwrap();drop(guard);
        let snapshot=crate::apply::restore(&st,&lock,r+1,"font-restore93",&object!{method:"native.restore",params:object!{operationId:"font-restore93",expectedRevision:(r+1).to_string()}}).unwrap();assert_ne!(snapshot["recoveryRequired"],true);
        let guard=GameLock::acquire(&c).unwrap();let remove=FontPlan::review_locked(&st,&game,FontChange::Remove{mod_id:"b".into(),font_ids:vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()]},&lock,&guard).unwrap();drop(guard);
        let input=object!{method:"native.fonts.commit",params:object!{operationId:"font-remove93",expectedRevision:(r+2).to_string(),token:"0123456789abcdef0123456789abcdef"}};
        if legacy_format {remove.commit(&st,&lock,r+2,"font-remove93",&input).unwrap();}
        else {
            crate::font_transaction::BEFORE_DATABASE.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("power loss"))));
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||remove.commit(&st,&lock,r+2,"font-remove93",&input))).is_err());
            let mut j=st.journal("font-remove93").unwrap().unwrap();
            for key in ["beforeDocument","requestedDocument"]{legacy(&mut j[key]["fonts"],false);}
            for key in ["parents","directoryBefore","directoryCreated","directoryRestored","directoryObserved","backupDirectory","targets"]{legacy(&mut j[key],false);}
            let mut before=st.document().unwrap();legacy(&mut before["fonts"],false);write_doc(&st,&before);st.write_journal(&lock,"font-remove93",&j).unwrap();
            for field in ["inode","fsid"] {
                let mut foreign=j.clone();foreign["targets"][0]["before"]["created"][field]=if field=="inode"{(foreign["targets"][0]["before"]["created"][field].as_u64().unwrap()+1).into()}else{"0000000000000000".into()};
                st.write_journal(&lock,"font-remove93",&foreign).unwrap();assert!(crate::apply::recover(&st,&lock,r+2,"font-foreign93",&object!{method:"native.recover",params:object!{operationId:"font-foreign93",expectedRevision:(r+2).to_string()}}).is_err());assert_eq!(st.document().unwrap(),before);st.write_journal(&lock,"font-remove93",&j).unwrap();
            }
            crate::apply::recover(&st,&lock,r+2,"font-recover93",&object!{method:"native.recover",params:object!{operationId:"font-recover93",expectedRevision:(r+2).to_string()}}).unwrap();
        }
        assert_eq!(font.exists(),borrowed);
        if reinstall {
            // An already-validated immutable asset in a disabled local record.
            // The install transaction still reads and checks its exact bytes.
            let asset_id="cccccccccccccccccccccccccccccccc";let bytes=b"immutable replacement font";
            paths::private_dir(&f.data.join("fonts")).unwrap();let asset=font_path(&f.data,asset_id).unwrap();fs::write(&asset,bytes).unwrap();paths::make_readonly(&asset).unwrap();
            let reference=FontRef{asset:FileRef{id:asset_id.into(),sha256:paths::fingerprint(&asset).unwrap().sha256,size:bytes.len() as u64,source_name:"reinstall.ttf".into()},target_name:"reinstall.ttf".into(),display_name:"Reinstalled".into(),origins:vec![FontOrigin::Loose{container:FontContainer::Raw{root_ordinal:0,relative_path:"panorama/fonts/reinstall.ttf".into()}}]};
            let mut doc=st.document().unwrap();let mut record=doc["profiles"]["default"]["mods"][0].clone();record["source"]["id"]="reinstall93".into();record["source"]["kind"]="local".into();record["source"]["localImport"]=object!{version:3,fonts:json::array![encode_ref(&reference)]};record["enabled"]=false.into();doc["profiles"]["default"]["mods"].push(record).unwrap();store::ensure_readable(&doc).unwrap();write_doc(&st,&doc);
            let guard=GameLock::acquire(&c).unwrap();let plan=FontPlan::review_locked(&st,&game,FontChange::Install{mod_id:"reinstall93".into(),font_ids:vec![asset_id.into()]},&lock,&guard).unwrap();drop(guard);
            plan.commit(&st,&lock,r+3,"font-reinstall93",&object!{method:"native.fonts.commit",params:object!{operationId:"font-reinstall93",expectedRevision:(r+3).to_string(),token:"0123456789abcdef0123456789abcdef"}}).unwrap();
            let guard=GameLock::acquire(&c).unwrap();verify_current_locked(&st,&st.document().unwrap(),&lock,&guard).unwrap();
            let installed=FontState::decode(&st.document().unwrap()).unwrap().installed.unwrap();assert_eq!(installed.owned_block.unwrap().root_id,doc["fonts"]["previous"]["prior"]["ownedBlock"]["rootId"].as_str().unwrap());assert_eq!(fs::read(dir.join("reinstall.ttf")).unwrap(),bytes);assert!(st.journals().unwrap().is_empty());return;
        }
        let guard=GameLock::acquire(&c).unwrap();let undo=FontPlan::review_locked(&st,&game,FontChange::Undo,&lock,&guard).unwrap();drop(guard);
        undo.commit(&st,&lock,r+3,"font-undo93",&object!{method:"native.fonts.commit",params:object!{operationId:"font-undo93",expectedRevision:(r+3).to_string(),token:"0123456789abcdef0123456789abcdef"}}).unwrap();
        assert_eq!(fs::read(dir.join("fonts.conf")).unwrap(),bytes);assert!(font.exists());assert!(st.journals().unwrap().is_empty());
    }
}

pub fn verify_current(store:&Store,doc:&JsonValue)->Result<(),NativeError>{verify_current_with(store,doc,None)}
