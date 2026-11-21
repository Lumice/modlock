//! Bounded read-only local VPK source preparation. Evidence remains live authority.
use crate::{
    archive_content::{ArchiveBudget, PreparedArchive},
    archive_quarantine::QuarantineLease,
    native::NativeError,
    package::{self, FileRef, PackageRef, SourceAssociation, SourcePackage},
    paths::{self, Fingerprint},
    prepare_control::{digest_controlled, fingerprint_admitted, PrepareControl, PrepareProgress},
    store, vpk,
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};
mod archive_sources;
pub(crate) mod option_sources;
pub(crate) mod font_sources;
#[cfg(test)]
#[path = "../tests/local_archive/consumer.rs"]
pub(crate) mod archive_test_support;

const MAX_ENTRIES: usize = 10_000;
const MAX_CANDIDATES: usize = 1_024;
const MAX_MEMBER: u64 = 16 * 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct LocalPrepareRequest {
    pub root: PathBuf,
    pub profile_id: String,
    pub expected_revision: u64,
    pub roots: Vec<PathBuf>,
    pub artwork: Option<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct LocalMetadata {
    pub name: String,
    pub author: String,
    pub category: String,
    pub link: String,
    pub description: String,
}
#[derive(Clone, Debug)]
pub struct LocalSelection {
    pub candidate_ids: Vec<String>,
    pub metadata: LocalMetadata,
}
#[derive(Clone, Debug)]
pub struct LocalPrepared {
    // A returned review/commit plan must keep the shared slot even after its
    // job owner or an otherwise empty raw quarantine is released.
    preparation_slot: Option<Arc<crate::remote::preparation_slot::PreparationSlotLease>>,
    archives: Vec<Arc<PreparedArchive>>,
    fonts: Arc<Vec<crate::font_content::FontCandidate>>,
    font_sources: Arc<Vec<font_sources::FontSourceFile>>,
    fingerprints: Arc<BTreeMap<PathBuf,Fingerprint>>,
    content_stamps: Arc<BTreeMap<PathBuf,String>>,
    pub request: LocalPrepareRequest,
    pub evidence: Arc<JsonValue>,
    pub candidates: Vec<LocalCandidate>,
    pub artwork: Option<LocalArtworkSource>,
}
#[derive(Clone, Debug)]
pub struct LocalCandidate {
    pub id: String,
    pub root_ordinal: usize,
    pub relative_path: String,
    pub archive_relative_path: Option<String>,
    pub option_name: String,
    pub package: Option<SourcePackage>,
    pub unavailable_reason: Option<String>,
    pub(crate) observed_companions: BTreeMap<u16, SourceAssociation>,
}
#[derive(Clone, Debug)]
pub struct LocalArtworkSource {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
    pub format: String,
}
#[derive(Clone, Debug)]
pub struct LocalImportPlan {
    pub group: Vec<(String, LocalImportPlan)>,
    pub prepared: LocalPrepared,
    pub selection: LocalSelection,
    pub token: String,
    pub mod_id: String,
    pub summary: JsonValue,
    package_refs: Vec<PackageRef>,
    retained_refs: Vec<PackageRef>,
    catalog: crate::options::OptionCatalog,
    artwork_ref: Option<FileRef>,
    archive_refs: Vec<FileRef>,
    selected_archives: Vec<Arc<PreparedArchive>>,
    font_refs: Vec<crate::fonts::FontRef>,
    selected_fonts: Vec<Arc<crate::font_content::PreparedFont>>,
}
fn bounds() -> NativeError {
    NativeError::new(
        "localSourceTooLarge",
        "Local sources exceed the supported preparation bounds.",
    )
}
fn stale() -> NativeError {
    NativeError::new(
        "stalePreview",
        "Local source inventory or participating files changed; review again.",
    )
}
fn unavailable() -> NativeError {
    NativeError::new(
        "invalidLocalSelection",
        "Some selected packages cannot be imported. Choose the mod’s files again.",
    )
}
fn cancelled(e: &NativeError) -> bool {
    e.kind == "localPreparationCancelled"
}
fn bounded_json(v: &JsonValue, limit: u64) -> Result<(), NativeError> {
    struct Counter(u64);
    impl std::io::Write for Counter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(b.len() as u64)
                .ok_or_else(|| std::io::Error::other("bound"))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    v.write(&mut Counter(limit)).map_err(|_| bounds())
}
fn local_path(p: &Path) -> Result<&str, NativeError> {
    let s = p
        .to_str()
        .filter(|s| s.len() <= 4096)
        .ok_or_else(NativeError::params)?;
    if !p.is_absolute() || s.split('/').any(|c| c == "." || c == "..") {
        return Err(NativeError::params());
    }
    paths::absolute(p)?;
    Ok(s)
}
fn request_json(r: &LocalPrepareRequest) -> JsonValue {
    object! {root:r.root.to_str().unwrap_or(""),profileId:r.profile_id.clone(),expectedRevision:r.expected_revision,roots:JsonValue::from(r.roots.iter().map(|p|p.to_str().unwrap_or("")).collect::<Vec<_>>()),artwork:r.artwork.as_ref().and_then(|p|p.to_str()).map(JsonValue::from).unwrap_or(JsonValue::Null)}
}
fn admit(r: &LocalPrepareRequest, control: &PrepareControl) -> Result<(), NativeError> {
    local_path(&r.root)?;
    match fs::symlink_metadata(&r.root) {
        Ok(_) => paths::reject_symlinks(&r.root)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => paths::verify_absent(&r.root)?,
        Err(e) => return Err(e.into()),
    }
    if !paths::safe_id(&r.profile_id) || r.roots.is_empty() || r.roots.len() > 64 {
        return Err(NativeError::params());
    }
    for p in &r.roots {
        control.check()?;
        local_path(p)?;
        if p.starts_with(&r.root) || r.root.starts_with(p) {
            return Err(NativeError::new(
                "unsafeLocalRoot",
                "Import sources must be separate from native storage and its ancestors.",
            ));
        }
        paths::reject_symlinks(p)?;
        let m = fs::symlink_metadata(p)?;
        if !m.is_file() && !m.is_dir() {
            return Err(NativeError::params());
        }
    }
    if let Some(p) = &r.artwork {
        local_path(p)?;
        if p.starts_with(&r.root) {
            return Err(NativeError::new(
                "unsafeLocalRoot",
                "Artwork must be separate from native storage.",
            ));
        }
    }
    bounded_json(&request_json(r), 64 * 1024 - 512)
}
#[derive(Clone)]
struct Found {
    ordinal: usize,
    relative: String,
    path: PathBuf,
    archive: Option<String>,
}
impl Found {
    fn name(&self) -> &str {
        Path::new(&self.relative)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
    }
}
struct Scan<'a> {
    control: &'a PrepareControl,
    inventory: Vec<JsonValue>,
    provenance: Vec<JsonValue>,
    sources: BTreeMap<PathBuf, Found>,
    bytes: u64,
    unique: HashSet<(u64, u64)>,
    archives: Vec<Found>,
    budget: Option<ArchiveBudget>,
    fonts: Vec<font_sources::PendingFont>,
    font_budget: Option<crate::font_content::FontBudget>,
}
fn file_identity(m: &fs::Metadata) -> JsonValue {
    object! {device:m.dev(),inode:m.ino(),size:m.len(),modifiedSeconds:m.mtime(),modifiedNanos:m.mtime_nsec(),mode:m.mode(),changedSeconds:m.ctime(),changedNanos:m.ctime_nsec()}
}
fn exclusion(path: &Path, m: &fs::Metadata) -> Option<&'static str> {
    if m.file_type().is_symlink() {
        return Some("symlink");
    }
    if m.is_dir() {
        return None;
    }
    if !m.is_file() {
        return Some("specialFile");
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "vpk" => None,
        "zip" | "rar" | "7z" | "tar" | "gz" => Some("archiveUnsupported"),
        "ttf" | "otf" | "woff" | "woff2" => Some("fontUnsupported"),
        _ => Some("unsupportedFile"),
    }
}
fn add_bytes(total: u64, size: u64) -> Result<u64, NativeError> {
    if size > MAX_MEMBER {
        return Err(bounds());
    }
    total
        .checked_add(size)
        .filter(|n| *n <= MAX_TOTAL)
        .ok_or_else(bounds)
}
impl Scan<'_> {
    fn entry(
        &mut self,
        ordinal: usize,
        relative: String,
        path: PathBuf,
        depth: usize,
        allow_directory: bool,
    ) -> Result<(), NativeError> {
        self.control.check()?;
        if depth > 32 || self.inventory.len() >= MAX_ENTRIES {
            return Err(bounds());
        }
        local_path(&path)?;
        let m = fs::symlink_metadata(&path)?;
        let archive = self.budget.is_some() && m.is_file() && archive_sources::supported(&path);
        let font = self.font_budget.is_some() && m.is_file() && crate::font_content::bundled_ttf(&relative);
        let excluded = if archive || font {
            None
        } else if m.is_dir() && !allow_directory {
            Some("requiredCompanionNotRegular")
        } else {
            exclusion(&path, &m)
        };
        if let Some(budget) = &mut self.budget {
            if !archive {
                budget.admit_raw_entry(
                    &relative,
                    if excluded.is_none() && m.is_file() {
                        Some(m.len())
                    } else {
                        None
                    },
                )?;
            }
        }
        self.inventory.push(object!{rootOrdinal:ordinal,relativePath:relative.clone(),path:path.to_str().unwrap(),kind:if m.is_dir(){"directory"}else if m.is_file(){"file"}else if m.file_type().is_symlink(){"symlink"}else{"special"},identity:file_identity(&m),exclusion:excluded.map(JsonValue::from).unwrap_or(JsonValue::Null)});
        self.control.report(PrepareProgress {
            phase: "enumerating".into(),
            files_done: self.inventory.len() as u64,
            ..Default::default()
        });
        if m.is_dir() && allow_directory {
            paths::reject_symlinks(&path)?;
            let mut children = Vec::new();
            for entry in fs::read_dir(&path)? {
                self.control.check()?;
                let entry = entry?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| NativeError::params())?;
                children.push((name, entry.path()));
                if children.len() + self.inventory.len() > MAX_ENTRIES {
                    return Err(bounds());
                }
            }
            children.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, p) in children {
                let rel = if relative == "." {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                self.entry(ordinal, rel, p, depth + 1, true)?;
            }
        } else if archive {
            self.archives.push(Found {
                ordinal,
                relative,
                path,
                archive: None,
            });
        } else if font {
            if self.unique.insert((m.dev(),m.ino())) {self.bytes=add_bytes(self.bytes,m.len())?;}
            // Global entry/input admission above precedes optional font admission.
            let origin=crate::font_content::FontContainer::Raw{root_ordinal:ordinal,relative_path:relative.clone()};
            let reason=match self.font_budget.as_mut().unwrap().admit(&crate::font_content::FontSourceKey::container(&origin),m.len())? {
                crate::font_content::FontAdmission::Available=>None,
                crate::font_content::FontAdmission::Unavailable(_)=>Some(if m.len()>crate::font_content::MAX_FONT_BYTES {"fontTooLarge"}else{"fontBudgetExceeded"}.to_owned()),
            };
            if let Some(reason)=&reason {self.inventory.last_mut().unwrap()["exclusion"]=reason.clone().into();}
            self.fonts.push(font_sources::PendingFont{found:Found{ordinal,relative,path,archive:None},reason});
        } else if excluded.is_none() {
            if self.unique.insert((m.dev(), m.ino())) {
                self.bytes = add_bytes(self.bytes, m.len())?;
            } else if m.len() > MAX_MEMBER {
                return Err(bounds());
            }
            self.provenance.push(object!{rootOrdinal:ordinal,relativePath:relative.clone(),path:path.to_str().unwrap()});
            self.sources.entry(path.clone()).or_insert(Found {
                ordinal,
                relative,
                path,
                archive: None,
            });
        }
        Ok(())
    }
}
fn fp_json(path: &Path, f: &Fingerprint) -> JsonValue {
    object! {path:path.to_str().unwrap(),sha256:f.sha256.clone(),size:f.size,modifiedSeconds:f.modified_seconds,modifiedNanos:f.modified_nanos,device:f.device,inode:f.inode}
}
fn fp_decode(v: &JsonValue) -> Result<Fingerprint, NativeError> {
    Ok(Fingerprint {
        sha256: v["sha256"]
            .as_str()
            .filter(|s| package::hex(s, 64))
            .ok_or_else(stale)?
            .into(),
        size: v["size"].as_u64().ok_or_else(stale)?,
        modified_seconds: v["modifiedSeconds"].as_i64().ok_or_else(stale)?,
        modified_nanos: v["modifiedNanos"].as_i64().ok_or_else(stale)?,
        device: v["device"].as_u64().ok_or_else(stale)?,
        inode: v["inode"].as_u64().ok_or_else(stale)?,
    })
}
fn sibling_names(name: &str, d: &vpk::VpkDirectory) -> Vec<String> {
    let stem = &name[..name.len() - 4];
    let stem = stem.strip_suffix("_dir").unwrap_or(stem);
    vpk::required_archives(d)
        .into_iter()
        .map(|i| format!("{stem}_{i:03}.vpk"))
        .collect()
}
pub(crate) fn prepare_local_job(
    request: LocalPrepareRequest,
    control: &PrepareControl,
    lease: Arc<QuarantineLease>,
) -> Result<LocalPrepared, NativeError> {
    prepare_impl(request, control, Some(lease))
}
pub fn prepare_local(
    request: LocalPrepareRequest,
    control: &PrepareControl,
) -> Result<LocalPrepared, NativeError> {
    prepare_impl(request, control, None)
}
fn prepare_impl(
    request: LocalPrepareRequest,
    control: &PrepareControl,
    lease: Option<Arc<QuarantineLease>>,
) -> Result<LocalPrepared, NativeError> {
    control.check()?;
    admit(&request, control)?;
    let mut budget = lease.as_ref().map(|_| ArchiveBudget::new());
    if let Some(budget) = &mut budget {
        budget.admit_roots(request.roots.len())?;
    }
    let mut scan = Scan {
        control,
        inventory: Vec::new(),
        provenance: Vec::new(),
        sources: BTreeMap::new(),
        bytes: 0,
        unique: HashSet::new(),
        archives: Vec::new(),
        budget,
        fonts: Vec::new(),
        font_budget: lease.as_ref().map(|_|crate::font_content::FontBudget::new()),
    };
    for (ordinal, path) in request.roots.iter().enumerate() {
        control.check()?;
        let relative = if path.is_dir() {
            ".".into()
        } else {
            path.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(NativeError::params)?
                .into()
        };
        scan.entry(ordinal, relative, path.clone(), 0, true)?;
    }
    // Explicit files expand only to exact required sibling names, never sidecars.
    let explicit = scan
        .sources
        .values()
        .filter(|s| request.roots[s.ordinal].is_file())
        .cloned()
        .collect::<Vec<_>>();
    for source in explicit {
        control.check()?;
        match vpk::inspect_directory_controlled(&source.path, control) {
            Ok(d) => {
                for name in sibling_names(source.path.file_name().unwrap().to_str().unwrap(), &d) {
                    control.check()?;
                    let path = source.path.parent().unwrap().join(&name);
                    if !scan.inventory.iter().any(|e| {
                        e["rootOrdinal"].as_usize() == Some(source.ordinal)
                            && e["path"].as_str() == path.to_str()
                    }) {
                        match fs::symlink_metadata(&path) {
                            Ok(_) => scan.entry(source.ordinal, name, path, 0, false)?,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                if scan.inventory.len() >= MAX_ENTRIES {
                                    return Err(bounds());
                                }
                                if let Some(budget) = &mut scan.budget {
                                    budget.admit_raw_entry(&name, None)?;
                                }
                                scan.inventory.push(object!{rootOrdinal:source.ordinal,relativePath:name,path:path.to_str().unwrap(),kind:"missing",exclusion:"requiredCompanionMissing"});
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
            }
            Err(e) if cancelled(&e) => return Err(e),
            Err(_) => {}
        }
    }
    let archives = if let Some(lease) = &lease {
        archive_sources::prepare(&mut scan, lease.clone())?
    } else {
        Vec::new()
    };
    assemble_prepared(request, control, scan, archives, lease)
}
fn assemble_prepared(
    request: LocalPrepareRequest,
    control: &PrepareControl,
    mut scan: Scan<'_>,
    archives: Vec<Arc<PreparedArchive>>,
    lease: Option<Arc<QuarantineLease>>,
) -> Result<LocalPrepared, NativeError> {
    let archive_job = scan.budget.is_some();
    let mut found = scan.sources.values().cloned().collect::<Vec<_>>();
    found.extend(archive_sources::found(&archives));
    let candidates = resolve_families(found, control, scan.budget.as_mut())?;
    // Hash every independent participating path, even hard links, after size admission.
    let mut fingerprints = BTreeMap::new();
    for pending in &scan.fonts {
        if pending.reason.is_none() { fingerprints.insert(pending.found.path.clone(), None); }
    }
    for archive in &archives {
        for member in archive.extracted_members() {
            fingerprints.insert(
                member.path().to_path_buf(),
                Some(member.fingerprint().clone()),
            );
        }
    }
    for c in &candidates {
        if let Some(p) = &c.package {
            for a in std::iter::once(&p.directory).chain(p.companions.values()) {
                fingerprints.entry(a.source_path.clone()).or_insert(None);
            }
        }
    }
    let total_files = fingerprints.len() as u64;
    control.report(PrepareProgress {
        phase: "hashing".into(),
        files_done: 0,
        files_total: Some(total_files),
        bytes_done: 0,
        bytes_total: None,
    });
    control.check()?;
    // Reject all currently observable changes and bounds before any source hash.
    verify_inventory(&scan.inventory, control)?;
    let mut admitted = BTreeMap::new();
    let mut unique = HashSet::new();
    let mut unique_bytes = 0u64;
    let mut total_hash_bytes = 0u64;
    for (path, fp) in &fingerprints {
        control.check()?;
        if fp.is_some() {
            continue;
        }
        let metadata = paths::open_regular(path)?.metadata()?;
        if metadata.len() > MAX_MEMBER {
            return Err(bounds());
        }
        if unique.insert((metadata.dev(), metadata.ino())) {
            unique_bytes = add_bytes(unique_bytes, metadata.len())?;
        }
        let prior = scan
            .inventory
            .iter()
            .find(|e| e["path"].as_str() == path.to_str())
            .ok_or_else(stale)?;
        if file_identity(&metadata) != prior["identity"] {
            return Err(stale());
        }
        total_hash_bytes = total_hash_bytes
            .checked_add(metadata.len())
            .ok_or_else(bounds)?;
        admitted.insert(path.clone(), metadata);
    }
    let mut done = fingerprints.values().filter(|fp| fp.is_some()).count() as u64;
    let mut byte_done = fingerprints
        .values()
        .flatten()
        .map(|fp| fp.size)
        .sum::<u64>();
    total_hash_bytes += byte_done;
    for (path, fp) in &mut fingerprints {
        control.check()?;
        if fp.is_some() {
            continue;
        }
        control.report(PrepareProgress {
            phase: "hashing".into(),
            files_done: done,
            files_total: Some(total_files),
            bytes_done: byte_done,
            bytes_total: Some(total_hash_bytes),
        });
        let f = fingerprint_admitted(path, control, &admitted[path], MAX_MEMBER)?;
        byte_done += f.size;
        done += 1;
        *fp = Some(f);
    }
    let artwork = match &request.artwork {
        Some(path) => Some(artwork_source(path, control)?),
        None => None,
    };
    let (fonts,font_sources)=if let Some(lease)=lease {
        let result=font_sources::discover(&mut scan,&archives,&candidates,&fingerprints,lease.clone())?;
        lease.validate_sealed()?;
        result
    } else {(Vec::new(),Vec::new())};
    // Directory identities cover additions/removals while a scan was in progress;
    // file identities bind inspection and hashing to the same source member.
    verify_inventory(&scan.inventory, control)?;
    scan.inventory.sort_by_key(|v| {
        (
            v["rootOrdinal"].as_usize().unwrap(),
            v["relativePath"].as_str().unwrap().to_owned(),
        )
    });
    scan.provenance
        .extend(archive_sources::provenance(&archives));
    let mut evidence = object! {version:if archive_job {3}else{1},scope:request_json(&request),inventory:JsonValue::from(scan.inventory),provenance:JsonValue::from(scan.provenance),fingerprints:JsonValue::from(fingerprints.iter().map(|(p,f)|fp_json(p,f.as_ref().unwrap())).collect::<Vec<_>>()),uniqueSourceBytes:scan.bytes,artwork:artwork.as_ref().map(|a|fp_json(&a.path,&a.fingerprint)).unwrap_or(JsonValue::Null)};
    if archive_job {
        evidence["archives"] = archive_sources::evidence(&archives);
        evidence["fonts"] = font_sources::evidence(&fonts,&font_sources);
        evidence["archiveRoles"] = archive_sources::roles(&archives);
    }
    evidence["installedGameOrigins"] = crate::installed::capture_origins(&request)?;
    bounded_json(&evidence, paths::MAX_JSON_BYTES)?;
    control.check()?;
    // Private, transient proof only. Never reconstruct this from persisted provenance.
    let mut content_stamps=BTreeMap::new();
    for path in fingerprints.keys().cloned()
        .chain(archives.iter().flat_map(|a|[a.input().original_path.clone(),a.snapshot_path().to_owned()]))
        .chain(artwork.iter().map(|a|a.path.clone())) {
        content_stamps.insert(path.clone(),paths::content_stamp(&paths::open_regular(&path)?)?);
    }
    for origin in evidence["installedGameOrigins"].members() {
        for row in origin["files"].members().filter(|r|r["directory"]!=true) {
            let path=PathBuf::from(row["sourcePath"].as_str().ok_or_else(stale)?);
            content_stamps.insert(path.clone(),paths::content_stamp(&paths::open_regular(&path)?)?);
        }
    }
    let prepared = LocalPrepared {
        preparation_slot: None,
        content_stamps: Arc::new(content_stamps),
        archives,
        fonts: Arc::new(fonts),
        font_sources: Arc::new(font_sources),
        fingerprints: Arc::new(fingerprints.into_iter().map(|(p,f)|(p,f.unwrap())).collect()),
        request,
        evidence: Arc::new(evidence),
        candidates,
        artwork,
    };
    bounded_json(&prepared.review_dto(), 32 * 1024 * 1024 - 65536)?;
    bounded_json(&prepared_record(&prepared), paths::MAX_JSON_BYTES)?;
    control.report(PrepareProgress {
        phase: "ready".into(),
        files_done: done,
        files_total: Some(total_files),
        bytes_done: byte_done,
        bytes_total: Some(byte_done),
    });
    Ok(prepared)
}
fn resolve_families(
    found: Vec<Found>,
    control: &PrepareControl,
    budget: Option<&mut ArchiveBudget>,
) -> Result<Vec<LocalCandidate>, NativeError> {
    resolve_families_impl(found,control,budget,false)
}
/// Remote members already have owner-created scratch paths. Their logical
/// namespace is the normalized relative parent, never the claim directory.
pub(crate) fn resolve_remote_families(found: Vec<(String,PathBuf)>, control: &PrepareControl, budget: &mut ArchiveBudget) -> Result<Vec<LocalCandidate>,NativeError> {
    resolve_families_impl(found.into_iter().map(|(relative,path)|Found{ordinal:0,relative,path,archive:Some(String::new())}).collect(),control,Some(budget),true)
}
fn resolve_families_impl(
    found: Vec<Found>,
    control: &PrepareControl,
    mut budget: Option<&mut ArchiveBudget>,
    remote: bool,
) -> Result<Vec<LocalCandidate>, NativeError> {
    // Raw namespaces preserve the accepted physical-parent behavior. Archive
    // namespaces retain all three original source coordinates, never flat paths.
    let mut namespaces: BTreeMap<(Option<(usize, String)>, PathBuf), Vec<Found>> = BTreeMap::new();
    for source in &found {
        namespaces
            .entry((
                source.archive.as_ref().map(|a| (source.ordinal, a.clone())),
                if source.archive.is_some() {
                    Path::new(&source.relative).parent().unwrap().into()
                } else {
                    source.path.parent().unwrap().into()
                },
            ))
            .or_default()
            .push(source.clone());
    }
    let mut candidates = Vec::new();
    for sources in namespaces.values_mut() {
        control.check()?;
        sources.sort_by(|a, b| a.relative.cmp(&b.relative));
        let mut names: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, s) in sources.iter().enumerate() {
            control.check()?;
            let name = s.name();
            names.entry(name.to_ascii_lowercase()).or_default().push(i);
        }
        let mut order = (0..sources.len()).collect::<Vec<_>>();
        order.sort_by_key(|i| {
            let n = sources[*i].name();
            (!n.to_ascii_lowercase().ends_with("_dir.vpk"), n.len(), *i)
        });
        let mut claims: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut dependencies: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut observed: HashMap<usize, BTreeMap<u16, usize>> = HashMap::new();
        let mut problems = HashSet::new();
        let mut parse_errors = HashMap::new();
        let mut logical_count = 0;
        for i in order {
            control.check()?;
            if claims.contains_key(&i) {
                continue;
            }
            logical_count += 1;
            if candidates.len() + logical_count > MAX_CANDIDATES {
                return Err(bounds());
            }
            let s = &sources[i];
            let name = s.name();
            if names[&name.to_ascii_lowercase()].len() != 1 {
                problems.insert(i);
            }
            // Keep only ownership and parse errors, never all resource trees.
            // Claimed opaque members are not independently parsed as directories.
            let directory = match vpk::inspect_directory_controlled(&s.path, control) {
                Ok(directory) => directory,
                Err(e) if cancelled(&e) => return Err(e),
                Err(e) => {
                    parse_errors.insert(i, e);
                    continue;
                }
            };
            {
                for (index,required) in vpk::required_archives(&directory).into_iter().zip(sibling_names(name, &directory)) {
                    control.check()?;
                    match names.get(&required.to_ascii_lowercase()) {
                        Some(members) => {
                            if members.len() != 1 {
                                problems.insert(i);
                            }
                            for member in members {
                                if *member == i {
                                    problems.insert(i);
                                    continue;
                                }
                                claims.entry(*member).or_default().push(i);
                                dependencies.entry(i).or_default().push(*member);
                                if sources[*member].name() != required {
                                    problems.insert(i);
                                } else if members.len()==1 {
                                    observed.entry(i).or_default().insert(index,*member);
                                }
                            }
                        }
                        None => {
                            problems.insert(i);
                        }
                    }
                }
            }
        }
        for owners in claims.values() {
            if owners.len() > 1 {
                problems.extend(owners);
            }
        }
        for (i, s) in sources.iter().enumerate() {
            control.check()?;
            if claims.contains_key(&i) {
                continue;
            }
            if candidates.len() >= MAX_CANDIDATES {
                return Err(bounds());
            }
            let mut package = None;
            let mut reason = None;
            if problems.contains(&i) {
                reason = Some("incompleteOrAmbiguousFamily".into());
            } else if let Some(e) = parse_errors.get(&i) {
                reason = Some(e.kind.clone());
            } else {
                let association = |index: usize| SourceAssociation {
                    original_name: sources[index].name().into(),
                    source_path: sources[index].path.clone(),
                };
                let mut associations = vec![association(i)];
                for j in dependencies.get(&i).into_iter().flatten() {
                    associations.push(association(*j));
                }
                match package::resolve_source_packages_controlled(&associations, control) {
                    Ok(mut p) if p.len() == 1 => package = p.pop(),
                    Ok(_) => reason = Some("ambiguousFamily".into()),
                    Err(e) if cancelled(&e) => return Err(e),
                    Err(e) => reason = Some(e.kind),
                }
            }
            control.check()?;
            if let Some(budget) = budget.as_deref_mut() {
                budget.admit_candidate()?;
            }
            let coordinates = match &s.archive {
                Some(archive) => json::array![s.ordinal, archive.clone(), s.relative.clone()],
                None => json::array![s.ordinal, s.relative.clone()],
            };
            let id = if remote {
                use sha2::{Digest, Sha256};
                format!("{:x}",Sha256::digest(store::canonical(&coordinates).as_bytes()))
            } else { digest_controlled(store::canonical(&coordinates).as_bytes(), control)? };
            candidates.push(LocalCandidate {
                id,
                root_ordinal: s.ordinal,
                relative_path: s.relative.clone(),
                archive_relative_path: s.archive.clone(),
                option_name: s.name().into(),
                package,
                unavailable_reason: reason,
                observed_companions: observed.get(&i).into_iter().flat_map(|items|items.iter()).map(|(index,member)|(*index,SourceAssociation{original_name:sources[*member].name().into(),source_path:sources[*member].path.clone()})).collect(),
            });
        }
    }
    candidates.sort_by(|a, b| {
        (a.root_ordinal, &a.archive_relative_path, &a.relative_path).cmp(&(
            b.root_ordinal,
            &b.archive_relative_path,
            &b.relative_path,
        ))
    });
    disambiguate(&mut candidates);
    Ok(candidates)
}
fn verify_inventory(inventory: &[JsonValue], control: &PrepareControl) -> Result<(), NativeError> {
    for entry in inventory {
        control.check()?;
        let path = Path::new(entry["path"].as_str().ok_or_else(stale)?);
        if entry["kind"] == "missing" {
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(stale()),
            }
        } else {
            let m = fs::symlink_metadata(path).map_err(|_| stale())?;
            if file_identity(&m) != entry["identity"] {
                return Err(stale());
            }
        }
    }
    Ok(())
}
fn disambiguate(candidates: &mut [LocalCandidate]) {
    let mut counts = HashMap::new();
    for c in candidates.iter() {
        *counts.entry(c.option_name.to_lowercase()).or_insert(0usize) += 1;
    }
    let mut used = HashSet::new();
    for c in candidates {
        let original = c.option_name.clone();
        let duplicate = counts[&original.to_lowercase()] > 1;
        let suffix = if original.ends_with("_dir.vpk") {
            "_dir.vpk"
        } else if original.ends_with(".vpk") {
            ".vpk"
        } else {
            ""
        };
        let base = &original[..original.len() - suffix.len()];
        let qualifier = Path::new(&c.relative_path)
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(48)
            .collect::<String>();
        let qualifier = if qualifier.is_empty() {
            format!("root{}", c.root_ordinal + 1)
        } else {
            qualifier
        };
        let mut serial = 1;
        loop {
            let extra = if duplicate {
                format!(
                    " ({qualifier}){}",
                    if serial > 1 {
                        format!(" {serial}")
                    } else {
                        String::new()
                    }
                )
            } else if serial > 1 {
                format!(" ({serial})")
            } else {
                String::new()
            };
            let mut stem = base.to_owned();
            while stem.len() + extra.len() + suffix.len() > 255 {
                stem.pop();
            }
            let name = format!("{stem}{extra}{suffix}");
            if used.insert(name.to_lowercase()) {
                c.option_name = name;
                break;
            }
            serial += 1;
        }
    }
}
impl LocalPrepared {
    fn recheck_content(&self,path:&Path)->Result<(),NativeError>{
        if self.content_stamps.get(path)!=Some(&paths::content_stamp(&paths::open_regular(path)?)?) {return Err(stale());}
        Ok(())
    }

    pub(crate) fn retain_preparation_slot(&mut self, slot: Arc<crate::remote::preparation_slot::PreparationSlotLease>) {
        self.preparation_slot = Some(slot);
    }
    pub fn font_candidates(&self) -> &[crate::font_content::FontCandidate] { &self.fonts }
    pub fn archive_sources(&self) -> &[Arc<PreparedArchive>] {
        &self.archives
    }
    pub fn source_provenance(&self) -> JsonValue {
        self.evidence["provenance"].clone()
    }
    pub fn default_metadata(&self) -> LocalMetadata {
        let name = self
            .candidates
            .iter()
            .find(|c| c.package.is_some())
            .or_else(|| self.candidates.first())
            .map(|c| {
                Path::new(&c.relative_path)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            })
            .unwrap_or("Local content");
        let name = name.strip_suffix(".vpk").unwrap_or(name);
        let name = name.strip_suffix("_dir").unwrap_or(name);
        LocalMetadata {
            name: name.into(),
            author: String::new(),
            category: "Uncategorized".into(),
            link: String::new(),
            description: String::new(),
        }
    }
    pub fn review_dto(&self) -> JsonValue {
        let mut dto=object! {profileId:self.request.profile_id.clone(),expectedRevision:self.request.expected_revision,candidates:JsonValue::from(self.candidates.iter().map(|c|object!{id:c.id.clone(),rootOrdinal:c.root_ordinal,relativePath:c.relative_path.clone(),archiveRelativePath:c.archive_relative_path.clone(),optionName:c.option_name.clone(),available:c.package.is_some(),unavailableReason:c.unavailable_reason.clone().map(JsonValue::from).unwrap_or(JsonValue::Null),companionCount:c.package.as_ref().map(|p|p.companions.len()).unwrap_or(0),members:c.package.as_ref().map(|p|JsonValue::from(std::iter::once((None,&p.directory)).chain(p.companions.iter().map(|(i,a)|(Some(*i),a))).map(|(i,a)|self.member_dto(i,a)).collect::<Vec<_>>())).unwrap_or(JsonValue::Null)}).collect::<Vec<_>>()),inventory:self.evidence["inventory"].clone(),provenance:self.evidence["provenance"].clone(),artwork:self.artwork.as_ref().map(|a|object!{path:a.path.to_str().unwrap(),format:a.format.clone(),size:a.fingerprint.size}).unwrap_or(JsonValue::Null),metadata:metadata_json(&self.default_metadata())};
        if self.evidence["version"]==3 {dto["fontCandidates"]=JsonValue::from(self.fonts.iter().map(crate::fonts::candidate_json).collect::<Vec<_>>());}
        dto
    }
    fn member_dto(&self, index: Option<u16>, association: &SourceAssociation) -> JsonValue {
        let mut dto = object! {archiveIndex:index,sourceName:association.original_name.clone(),sourcePath:association.source_path.to_str().unwrap()};
        if let Ok(fp) = self.fingerprint_for(&association.source_path) {
            dto["sha256"] = fp.sha256.into();
            dto["size"] = fp.size.into();
        }
        if let Some((archive, member)) =
            archive_sources::member_for(&self.archives, &association.source_path)
        {
            dto["sourcePath"] = archive.input().original_path.to_str().unwrap().into();
            dto["archiveRelativePath"] = archive.input().relative_path.clone().into();
            dto["archive"] = archive_sources::member_provenance(archive, member.member());
        }
        dto
    }
    pub fn fingerprint_for(&self, path: &Path) -> Result<Fingerprint, NativeError> {
        if self.evidence["version"]==3 {return self.fingerprints.get(path).cloned().ok_or_else(stale);}
        self.evidence["fingerprints"]
            .members()
            .find(|f| f["path"].as_str() == path.to_str())
            .ok_or_else(stale)
            .and_then(fp_decode)
    }
}
fn prepared_record(prepared: &LocalPrepared) -> JsonValue {
    let association = |a: &SourceAssociation| object! {originalName:a.original_name.clone(),sourcePath:a.source_path.to_str().unwrap()};
    object! {request:request_json(&prepared.request),evidence:prepared.evidence.as_ref().clone(),candidates:JsonValue::from(prepared.candidates.iter().map(|c|object!{id:c.id.clone(),rootOrdinal:c.root_ordinal,relativePath:c.relative_path.clone(),archiveRelativePath:c.archive_relative_path.clone(),optionName:c.option_name.clone(),unavailableReason:c.unavailable_reason.clone().map(JsonValue::from).unwrap_or(JsonValue::Null),package:c.package.as_ref().map(|p|object!{directory:association(&p.directory),companions:JsonValue::from(p.companions.iter().map(|(i,a)|object!{archiveIndex:*i,file:association(a)}).collect::<Vec<_>>())}).unwrap_or(JsonValue::Null)}).collect::<Vec<_>>()),artwork:prepared.artwork.as_ref().map(|a|object!{path:a.path.to_str().unwrap(),fingerprint:fp_json(&a.path,&a.fingerprint),format:a.format.clone()}).unwrap_or(JsonValue::Null)}
}
fn metadata_json(m: &LocalMetadata) -> JsonValue {
    object! {name:m.name.clone(),author:m.author.clone(),category:m.category.clone(),link:m.link.clone(),description:m.description.clone()}
}
fn metadata_valid(m: &LocalMetadata) -> bool {
    [
        (m.name.as_str(), 256, false),
        (m.author.as_str(), 256, false),
        (m.category.as_str(), 128, false),
        (m.link.as_str(), 4096, false),
        (m.description.as_str(), 16384, true),
    ]
    .iter()
    .all(|(s, max, multiline)| {
        s.len() <= *max
            && !s
                .chars()
                .any(|c| c.is_control() && !(*multiline && matches!(c, '\n' | '\t')))
    }) && !m.name.trim().is_empty()
        && valid_link(&m.link)
}
fn valid_link(link: &str) -> bool {
    if link.is_empty() {
        return true;
    }
    let Some(rest) = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
    else {
        return false;
    };
    if !link.is_ascii()
        || link
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\')
    {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || !valid_uri_suffix(&rest[authority.len()..])
    {
        return false;
    }
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let Some((ip, tail)) = after.split_once(']') else {
            return false;
        };
        if ip.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        let port = if tail.is_empty() {
            None
        } else {
            let Some(p) = tail.strip_prefix(':') else {
                return false;
            };
            Some(p)
        };
        (ip, port)
    } else {
        let (host, port) = authority
            .split_once(':')
            .map(|(h, p)| (h, Some(p)))
            .unwrap_or((authority, None));
        if host.is_empty()
            || host.len() > 253
            || !host.split('.').all(|s| {
                !s.is_empty()
                    && s.len() <= 63
                    && !s.starts_with('-')
                    && !s.ends_with('-')
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return false;
        }
        (host, port)
    };
    !host.is_empty()
        && port.is_none_or(|p| {
            !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
        })
}
// RFC 3986 path pchar plus /, and query/fragment's additional ?.
fn valid_uri_suffix(suffix: &str) -> bool {
    let bytes = suffix.as_bytes();
    let mut at = 0;
    let mut section = 0;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'%' => {
                if !bytes
                    .get(at + 1..at + 3)
                    .is_some_and(|pair| pair.iter().all(u8::is_ascii_hexdigit))
                {
                    return false;
                }
                at += 3;
                continue;
            }
            b'#' => {
                if section == 2 {
                    return false;
                }
                section = 2;
            }
            b'?' => {
                if section == 0 {
                    section = 1;
                }
            }
            byte if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&byte) => {}
            _ => return false,
        }
        at += 1;
    }
    true
}
fn selected(
    prepared: &LocalPrepared,
    selection: &LocalSelection,
) -> Result<Vec<SourcePackage>, NativeError> {
    if selection.candidate_ids.len() > 99 {
        return Err(NativeError::new("invalidLocalSelection", "This mod has more than 99 packages and can’t be imported."));
    }
    if selection.candidate_ids.is_empty()
        || selection.candidate_ids.windows(2).any(|w| w[0] >= w[1])
    {
        return Err(unavailable());
    }
    let ids = selection.candidate_ids.iter().collect::<BTreeSet<_>>();
    let mut sources = Vec::new();
    for c in &prepared.candidates {
        if ids.contains(&c.id) {
            sources.push(c.package.clone().ok_or_else(unavailable)?);
        }
    }
    if sources.len() != ids.len() {
        return Err(unavailable());
    }
    Ok(sources)
}
pub fn review_local(
    prepared: &LocalPrepared,
    selection: LocalSelection,
) -> Result<LocalImportPlan, NativeError> {
    if !metadata_valid(&selection.metadata) {
        return Err(NativeError::new(
            "invalidLocalMetadata",
            "Check the mod details. Shorten long fields and use an http:// or https:// link.",
        ));
    }
    let sources = selected(prepared, &selection)?;
    let retained_sources = prepared.candidates.iter().filter_map(|c| c.package.clone()).collect::<Vec<_>>();
    for p in &retained_sources {
        let associations = std::iter::once(p.directory.clone())
            .chain(p.companions.values().cloned())
            .collect::<Vec<_>>();
        let resolved = package::resolve_source_packages(&associations)?;
        if resolved.len() != 1 {
            return Err(unavailable());
        }
    }
    // Allocate once across all ready occurrences. Selected views borrow these IDs.
    let retained_refs = allocate_package_refs(prepared, &retained_sources)?;
    let package_refs = prepared.candidates.iter().filter(|c| c.package.is_some()).zip(&retained_refs)
        .filter(|(c,_)| selection.candidate_ids.contains(&c.id)).map(|(_,p)|p.clone()).collect::<Vec<_>>();
    // Artwork owns a separate destination even when its bytes equal a VPK member.
    let artwork_ref = prepared
        .artwork
        .as_ref()
        .map(|art| -> Result<FileRef, NativeError> {
            Ok(FileRef {
                id: paths::random_id()?,
                sha256: art.fingerprint.sha256.clone(),
                size: art.fingerprint.size,
                source_name: art
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(NativeError::params)?
                    .into(),
            })
        })
        .transpose()?;
    let attached=crate::font_content::project(&prepared.fonts,&selection.candidate_ids.iter().map(String::as_str).collect());
    let mut font_pairs=attached.iter().filter_map(|c|c.prepared.as_ref().map(|p|(c,p))).map(|(c,p)|{
        Ok((crate::fonts::FontRef{asset:FileRef{id:paths::random_id()?,sha256:p.fingerprint().sha256.clone(),size:p.fingerprint().size,source_name:c.target_name.clone()},target_name:c.target_name.clone(),display_name:c.display_name.clone(),origins:c.origins.clone()},p.clone()))
    }).collect::<Result<Vec<_>,NativeError>>()?;
    font_pairs.sort_by(|a,b|a.0.asset.id.cmp(&b.0.asset.id));
    let (font_refs,selected_fonts):(Vec<_>,Vec<_>)=font_pairs.into_iter().unzip();
    let selected_archives = archive_sources::selected(prepared, &retained_sources, &attached)?;
    // Each contributing archive occurrence owns its original, independently of
    // equal bytes in another archive, payload, or artwork destination.
    let archive_refs = selected_archives.iter().map(|archive| {
        let fp = archive.snapshot_fingerprint();
        let file = FileRef {
            id: paths::random_id()?,
            sha256: fp.sha256.clone(),
            size: fp.size,
            source_name: archive.input().original_path.file_name()
                .and_then(|s| s.to_str()).ok_or_else(NativeError::params)?.into(),
        };
        package::decode_file(&package::encode_file(&file))?;
        Ok(file)
    }).collect::<Result<Vec<_>, NativeError>>()?;
    let mut catalog = crate::options::OptionCatalog { entries: Vec::new(), selected_ids: Vec::new(),
        retained_archives: Vec::new(), attachments: Vec::new() };
    for ((candidate, source), package) in prepared.candidates.iter().filter(|c|c.package.is_some())
        .zip(&retained_sources).zip(&retained_refs) {
        let id = paths::random_id()?;
        if selection.candidate_ids.contains(&candidate.id) {catalog.selected_ids.push(id.clone());}
        catalog.entries.push(crate::options::OptionEntry {
            id, option_name: candidate.option_name.clone(), package: Some(package.clone()),
            expected_family: Some(crate::options::ExpectedFamily::from_package(package)),
            archive_binding: archive_sources::binding(&selected_archives, &archive_refs, source)?,
            source_label: candidate.relative_path.clone(), unavailable_reason: None,
        });
    }
    catalog.selected_ids.sort();
    // Archive refs initially belong to the historical localImport inventory;
    // later attachments use their own explicit archive coordinate namespace.
    catalog.encode(2)?;
    catalog.validate_selection_save()?;
    let token = paths::random_id()?;
    let mod_id = format!("local-{}", paths::random_id()?);
    let mut summary = prepared.review_dto();
    // Publication history and option refs carry the same reviewed display names;
    // source associations/fingerprints continue to record actual disk paths.
    for (candidate,package) in summary["candidates"].members_mut().filter(|c|c["available"]==true).zip(&retained_refs) {
        for (member,file) in candidate["members"].members_mut().zip(std::iter::once(&package.directory).chain(package.companions.values())) {
            member["sourceName"]=file.source_name.clone().into();
        }
    }
    summary["metadata"] = metadata_json(&selection.metadata);
    summary["selectedCandidateIds"] = JsonValue::from(selection.candidate_ids.clone());
    summary["packageCount"] = sources.len().into();
    summary["token"] = token.clone().into();
    summary["modId"] = mod_id.clone().into();
    summary["status"] = "downloaded".into();
    summary["enabled"] = false.into();
    archive_sources::bind_review(&mut summary, &selected_archives, &archive_refs)?;
    if prepared.evidence["version"]==3 {
        summary["fontCandidates"]=JsonValue::from(attached.iter().map(crate::fonts::candidate_json).collect::<Vec<_>>());
        // Invalid-only discovery remains diagnostic history and allocates no refs.
        summary["fontDiagnostics"]=JsonValue::from(attached.iter().filter(|f|f.prepared.is_none()).chain(prepared.fonts.iter().filter(|f|f.origins.is_empty())).map(crate::fonts::candidate_json).collect::<Vec<_>>());
    }
    if !font_refs.is_empty() {
        summary["fonts"]=JsonValue::from(font_refs.iter().map(crate::fonts::encode_ref).collect::<Vec<_>>());
        font_sources::bind_refs(&mut summary["fonts"],&selected_archives,&archive_refs)?;
    }
    let mut unique = HashSet::new();
    let mut payload_bytes = 0u64;
    let mut logical_bytes = 0u64;
    for p in &catalog.packages(false)? {
        for f in std::iter::once(&p.directory).chain(p.companions.values()) {
            logical_bytes = logical_bytes.checked_add(f.size).ok_or_else(bounds)?;
            if unique.insert(&f.id) {
                payload_bytes = payload_bytes.checked_add(f.size).ok_or_else(bounds)?;
            }
        }
    }
    let archive_bytes = archive_refs.iter().try_fold(0u64, |sum, f|
        sum.checked_add(f.size).ok_or_else(bounds))?;
    let font_bytes=font_refs.iter().try_fold(0u64,|n,f|n.checked_add(f.asset.size).ok_or_else(bounds))?;
    let copy_bytes = payload_bytes.checked_add(archive_bytes).and_then(|n|n.checked_add(font_bytes))
        .and_then(|n| n.checked_add(artwork_ref.as_ref().map_or(0, |f| f.size)))
        .ok_or_else(bounds)?;
    // Input (loose + compressed) and expanded content have separate 64 GiB
    // budgets. Do not incorrectly apply the old raw-only limit to their sum.
    if logical_bytes > 2 * MAX_TOTAL || payload_bytes > 2 * MAX_TOTAL || archive_bytes > MAX_TOTAL
        || font_bytes>crate::font_content::MAX_FONT_JOB_BYTES || copy_bytes > 137589948416 {
        return Err(bounds());
    }
    let selected_bytes = package_refs.iter().flat_map(|p|std::iter::once(&p.directory).chain(p.companions.values()))
        .try_fold(0u64, |n,f|n.checked_add(f.size).ok_or_else(bounds))?;
    if selected_bytes > 137438953472 {return Err(bounds());}
    summary["installedGameOrigins"] = prepared.evidence["installedGameOrigins"].clone();
    summary["logicalPackageBytes"] = selected_bytes.into();
    summary["selectedLogicalPackageBytes"] = selected_bytes.into();
    summary["retainedLogicalPackageBytes"] = logical_bytes.into();
    summary["selectedPackageCount"] = package_refs.len().into();
    summary["retainedPackageCount"] = retained_refs.iter().map(package::family_key).collect::<Result<BTreeSet<_>,_>>()?.len().into();
    summary["inactiveOptionCount"] = (catalog.entries.len()-catalog.selected_ids.len()).into();
    summary["uniquePayloadBytes"] = payload_bytes.into();
    summary["retainedArchiveBytes"] = archive_bytes.into();
    summary["uniqueCopyBytes"] = copy_bytes.into();
    if prepared.evidence["version"]==3 {summary["uniqueFontBytes"]=font_bytes.into();}
    bounded_json(&summary, 32 * 1024 * 1024 - 65536)?;
    bounded_json(
        &object! {prepared:prepared_record(prepared),selection:object!{candidateIds:JsonValue::from(selection.candidate_ids.clone()),metadata:metadata_json(&selection.metadata)},token:token.clone(),modId:mod_id.clone(),summary:summary.clone(),packageRefs:package::encode_packages(&package_refs),retainedPackageRefs:package::encode_packages(&retained_refs),options:catalog.encode(2)?,artworkRef:artwork_ref.as_ref().map(package::encode_file),archiveRefs:JsonValue::from(archive_refs.iter().map(package::encode_file).collect::<Vec<_>>())},
        paths::MAX_JSON_BYTES,
    )?;
    Ok(LocalImportPlan {
        group: Vec::new(),
        prepared: prepared.clone(),
        selection,
        token,
        mod_id,
        summary,
        package_refs,
        retained_refs,
        catalog,
        artwork_ref,
        archive_refs,
        selected_archives,
        font_refs,
        selected_fonts,
    })
}
fn allocate_package_refs(
    prepared: &LocalPrepared,
    sources: &[SourcePackage],
) -> Result<Vec<PackageRef>, NativeError> {
    let mut ids: HashMap<(String, u64, String), String> = HashMap::new();
    sources
        .iter()
        .map(|p| {
            let original=if !prepared.evidence["installedGameOrigins"].is_empty() {
                crate::installed::original_package_name(&p.directory.source_path)
            } else {None};
            let mut file = |a: &SourceAssociation| -> Result<FileRef, NativeError> {
                let fp = prepared.fingerprint_for(&a.source_path)?;
                let key = (fp.sha256.clone(), fp.size, format!("{}\0{}",a.original_name,original.as_deref().unwrap_or("")));
                if !ids.contains_key(&key) {
                    ids.insert(key.clone(), paths::random_id()?);
                }
                Ok(FileRef {
                    id: ids[&key].clone(),
                    sha256: fp.sha256,
                    size: fp.size,
                    source_name: if a.source_path==p.directory.source_path {original.clone().unwrap_or_else(||a.original_name.clone())}else if let Some(name)=&original {
                        let stem=name.strip_suffix(".vpk").unwrap();let stem=stem.strip_suffix("_dir").unwrap_or(stem);
                        p.companions.iter().find(|(_,c)|c.source_path==a.source_path).map(|(i,_)|format!("{stem}_{i:03}.vpk")).unwrap_or_else(||a.original_name.clone())
                    }else{a.original_name.clone()},
                })
            };
            Ok(PackageRef {
                directory: file(&p.directory)?,
                companions: p
                    .companions
                    .iter()
                    .map(|(i, a)| Ok((*i, file(a)?)))
                    .collect::<Result<_, NativeError>>()?,
            })
        })
        .collect()
}
impl LocalImportPlan {
    pub fn font_refs(&self) -> &[crate::fonts::FontRef] { &self.font_refs }
    pub(crate) fn font_history(&self) -> Result<JsonValue,NativeError> {
        let mut rows=JsonValue::from(self.font_refs.iter().map(crate::fonts::encode_ref).collect::<Vec<_>>());
        font_sources::bind_refs(&mut rows,&self.selected_archives,&self.archive_refs)?;
        Ok(rows)
    }
    pub(crate) fn selected_font_sources(&self) -> &[Arc<crate::font_content::PreparedFont>] { &self.selected_fonts }
    /// Clones review-time IDs; this never allocates new destination authority.
    pub fn archive_refs(&self) -> Vec<FileRef> {
        self.archive_refs.clone()
    }
    /// Typed sources paired exactly with archive_refs(), in retained input order.
    /// The compatibility name means selected for retention, including inactive VPKs.
    pub fn selected_archive_sources(&self) -> Vec<Arc<PreparedArchive>> {
        self.selected_archives.clone()
    }
    /// Publication provenance uses original coordinates, never quarantine names.
    pub(crate) fn source_fingerprints(&self) -> Result<JsonValue, NativeError> {
        let sources = self.retained_sources()?;
        let paths = sources.iter().flat_map(|p| std::iter::once(&p.directory.source_path)
            .chain(p.companions.values().map(|s| &s.source_path))).collect::<BTreeSet<_>>();
        let mut rows = Vec::new();
        for path in paths {
            let fp = self.prepared.fingerprint_for(path)?;
            if let Some((archive, member)) = archive_sources::member_for(&self.selected_archives, path) {
                let file = self.selected_archives.iter().zip(&self.archive_refs)
                    .find(|(a, _)| std::ptr::eq(a.as_ref(), archive))
                    .map(|(_, f)| f).ok_or_else(stale)?;
                let mut provenance = archive_sources::member_provenance(archive, member.member());
                provenance["id"] = file.id.clone().into();
                provenance["normalizedPath"] = member.member().normalized_path.clone().into();
                rows.push(object! {path:archive.input().original_path.to_str().unwrap(),
                    rootOrdinal:archive.input().root_ordinal,archiveRelativePath:archive.input().relative_path.clone(),
                    archive:provenance,sha256:fp.sha256,size:fp.size});
            } else {
                rows.push(fp_json(path, &fp));
            }
        }
        Ok(JsonValue::from(rows))
    }
    pub fn retained_sources(&self) -> Result<Vec<SourcePackage>, NativeError> {
        Ok(self.prepared.candidates.iter().filter_map(|c| c.package.clone()).collect())
    }
    pub fn retained_package_refs(&self) -> Result<Vec<PackageRef>, NativeError> {
        Ok(self.retained_refs.clone())
    }
    pub(crate) fn option_catalog(&self) -> &crate::options::OptionCatalog { &self.catalog }
    pub fn selected_sources(&self) -> Result<Vec<SourcePackage>, NativeError> {
        selected(&self.prepared, &self.selection)
    }
    pub fn package_refs(&self) -> Result<Vec<PackageRef>, NativeError> {
        Ok(self.package_refs.clone())
    }
    /// Clones the reviewed artwork destination authority without I/O or ID allocation.
    pub fn artwork_ref(&self) -> Option<FileRef> {
        self.artwork_ref.clone()
    }
}
pub fn revalidate_local(
    prepared: &LocalPrepared,
    control: &PrepareControl,
) -> Result<(), NativeError> {
    control.check()?;
    for origin in prepared.evidence["installedGameOrigins"].members() {
        let c=Path::new(origin["gamePath"].as_str().ok_or_else(stale)?).join("game/citadel");
        crate::installed::verify_origin(origin,&c,true).map_err(|_|stale())?;
    }
    if prepared.evidence["version"] == 2 || prepared.evidence["version"] == 3 {
        return (|| {
            archive_sources::revalidate(prepared, control)?;
            if prepared.evidence["version"]==3 {
                if prepared.evidence["archiveRoles"]!=archive_sources::roles(&prepared.archives) {return Err(stale());}
                font_sources::revalidate(prepared,control)?;
            }
            Ok(())
        })().map_err(|e:NativeError| {
            if cancelled(&e) {
                e
            } else {
                stale()
            }
        });
    }
    if prepared.evidence["scope"]!=request_json(&prepared.request) {return Err(stale());}
    let inventory=prepared.evidence["inventory"].members().cloned().collect::<Vec<_>>();
    verify_inventory(&inventory,control)?;
    for path in prepared.content_stamps.keys() {control.check()?;prepared.recheck_content(path)?;}
    control.check()
}
fn invalid_art() -> NativeError {
    NativeError::new("invalidLocalArtwork","Choose a bounded PNG or JPEG image with supported dimensions; decode failure requires preparing again without artwork.")
}
fn dimensions(width: u32, height: u32) -> Result<(), NativeError> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 16_777_216
    {
        Err(invalid_art())
    } else {
        Ok(())
    }
}
fn artwork_source(
    path: &Path,
    control: &PrepareControl,
) -> Result<LocalArtworkSource, NativeError> {
    control.check()?;
    let mut f = paths::open_regular(path)?;
    let before = f.metadata()?;
    if before.len() > 16 * 1024 * 1024 {
        return Err(invalid_art());
    }
    let mut bytes = Vec::new();
    loop {
        control.check()?;
        let mut chunk = [0u8; 65536];
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > 16 * 1024 * 1024 {
            return Err(invalid_art());
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = if ext == "png" {
        if bytes.len() < 33
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || &bytes[8..12] != 13u32.to_be_bytes().as_slice()
            || &bytes[12..16] != b"IHDR"
            || !matches!(bytes[24], 1 | 2 | 4 | 8 | 16)
            || !matches!(bytes[25], 0 | 2 | 3 | 4 | 6)
            || bytes[26] != 0
            || bytes[27] != 0
            || bytes[28] > 1
        {
            return Err(invalid_art());
        }
        dimensions(
            u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
            u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
        )?;
        "png"
    } else if matches!(ext.as_str(), "jpg" | "jpeg") {
        jpeg_dimensions(&bytes, control)?;
        "jpeg"
    } else {
        return Err(invalid_art());
    };
    let fingerprint = fingerprint_admitted(path, control, &before, 16 * 1024 * 1024)?;
    if file_identity(&before) != file_identity(&f.metadata()?)
        || file_identity(&before) != file_identity(&paths::open_regular(path)?.metadata()?)
    {
        return Err(stale());
    }
    Ok(LocalArtworkSource {
        path: path.into(),
        fingerprint,
        format: format.into(),
    })
}
fn jpeg_dimensions(bytes: &[u8], control: &PrepareControl) -> Result<(), NativeError> {
    if bytes.len() < 4 || bytes[..2] != [0xff, 0xd8] {
        return Err(invalid_art());
    }
    let mut at = 2;
    while at < bytes.len() {
        control.check()?;
        if bytes[at] != 0xff {
            return Err(invalid_art());
        }
        while bytes.get(at) == Some(&0xff) {
            at += 1;
        }
        let marker = *bytes.get(at).ok_or_else(invalid_art)?;
        at += 1;
        if matches!(marker, 0 | 0xd8 | 0xd9 | 0xda) {
            return Err(invalid_art());
        }
        if marker == 1 || matches!(marker, 0xd0..=0xd7) {
            continue;
        }
        let length = u16::from_be_bytes(
            bytes
                .get(at..at + 2)
                .ok_or_else(invalid_art)?
                .try_into()
                .unwrap(),
        ) as usize;
        if length < 2 {
            return Err(invalid_art());
        }
        let segment = bytes
            .get(at..at.checked_add(length).ok_or_else(invalid_art)?)
            .ok_or_else(invalid_art)?;
        if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
            if segment.len() < 8
                || segment[2] != 8
                || segment[7] == 0
                || segment.len() != 8 + 3 * segment[7] as usize
            {
                return Err(invalid_art());
            }
            return dimensions(
                u16::from_be_bytes([segment[5], segment[6]]) as u32,
                u16::from_be_bytes([segment[3], segment[4]]) as u32,
            );
        }
        at += length;
    }
    Err(invalid_art())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{family_vpk, vpk, FamilyEntry, Fixture};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    fn req(f: &Fixture, roots: Vec<PathBuf>) -> LocalPrepareRequest {
        LocalPrepareRequest {
            root: f.data.clone(),
            profile_id: "default".into(),
            expected_revision: 1,
            roots,
            artwork: None,
        }
    }
    #[test]
    fn local_cancel_enumeration_at_barrier_is_bounded() {
        let f = Fixture::new();
        let root = f.root.join("scan");
        fs::create_dir(&root).unwrap();
        for i in 0..20 {
            fs::write(root.join(format!("{i}.txt")), b"keep").unwrap();
        }
        let request = req(&f, vec![root]);
        let c = PrepareControl::new();
        let b = Arc::new(Barrier::new(2));
        let checks = Arc::new(AtomicUsize::new(0));
        let cb = b.clone();
        let n = checks.clone();
        c.check_hook(Arc::new(move || {
            if n.fetch_add(1, Ordering::SeqCst) == 9 {
                cb.wait();
                cb.wait();
            }
        }));
        let worker_c = c.clone();
        let worker = std::thread::spawn(move || prepare_local(request, &worker_c));
        b.wait();
        assert_eq!(c.progress().phase, "enumerating");
        c.request_cancel();
        b.wait();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind,
            "localPreparationCancelled"
        );
        assert_eq!(checks.load(Ordering::SeqCst), 10);
        assert!(!f.data.exists());
    }
    #[test]
    fn local_cancel_parser_loop_at_barrier_is_bounded() {
        let f = Fixture::new();
        let path = f.root.join("parser.vpk");
        let names = (0..1000).map(|i| format!("{i}.bin")).collect::<Vec<_>>();
        let entries = names
            .iter()
            .map(|s| FamilyEntry {
                path: s,
                crc: 1,
                archive: 0x7fff,
                offset: 0,
                length: 0,
                preload: b"",
            })
            .collect::<Vec<_>>();
        fs::write(&path, family_vpk(1, &entries, b"")).unwrap();
        let c = PrepareControl::new();
        let b = Arc::new(Barrier::new(2));
        let checks = Arc::new(AtomicUsize::new(0));
        let cb = b.clone();
        let n = checks.clone();
        c.check_hook(Arc::new(move || {
            if n.fetch_add(1, Ordering::SeqCst) == 9 {
                cb.wait();
                cb.wait();
            }
        }));
        let worker_c = c.clone();
        let worker =
            std::thread::spawn(move || vpk::inspect_directory_controlled(&path, &worker_c));
        b.wait();
        c.request_cancel();
        b.wait();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind,
            "localPreparationCancelled"
        );
        assert_eq!(checks.load(Ordering::SeqCst), 10);
        assert!(!f.data.exists());
    }
    #[test]
    fn local_source_changed_between_parse_and_hash_is_stale() {
        let f = Fixture::new();
        let path = f.root.join("race.vpk");
        fs::write(&path, vpk("a.bin", 1)).unwrap();
        let c = PrepareControl::new();
        let observed = c.clone();
        let changed = Arc::new(AtomicUsize::new(0));
        let n = changed.clone();
        let source = path.clone();
        c.check_hook(Arc::new(move || {
            if observed.progress().phase == "hashing" && n.fetch_add(1, Ordering::SeqCst) == 0 {
                fs::write(&source, vpk("different.bin", 99)).unwrap();
            }
        }));
        let result = prepare_local(req(&f, vec![path]), &c);
        c.check_hook(Arc::new(|| {}));
        assert_eq!(result.unwrap_err().kind, "stalePreview");
    }
    #[test]
    fn local_metadata_field_and_artwork_dimension_boundaries() {
        let mut metadata = LocalMetadata {
            name: "x".repeat(256),
            author: "x".repeat(256),
            category: "x".repeat(128),
            link: format!("https://a/{}", "x".repeat(4086)),
            description: "雪".repeat(5461) + "x",
        };
        assert_eq!(metadata.link.len(), 4096);
        assert_eq!(metadata.description.len(), 16384);
        assert!(metadata_valid(&metadata));
        let mut invalid = metadata.clone();
        invalid.author.push('x');
        assert!(!metadata_valid(&invalid));
        invalid = metadata.clone();
        invalid.category.push('x');
        assert!(!metadata_valid(&invalid));
        invalid = metadata.clone();
        invalid.link.push('x');
        assert!(!metadata_valid(&invalid));
        invalid = metadata.clone();
        invalid.description.push('x');
        assert!(!metadata_valid(&invalid));
        metadata.name.push('\n');
        assert!(!metadata_valid(&metadata));
        assert!(dimensions(8192, 2048).is_ok());
        assert!(dimensions(8192, 2049).is_err());
        assert!(dimensions(8193, 1).is_err());
        assert!(dimensions(0, 1).is_err());
        let jpeg = vec![
            0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 32, 0, 32, 1, 1, 0x11, 0, 0xff, 0xd9,
        ];
        assert!(jpeg_dimensions(&jpeg, &PrepareControl::new()).is_ok());
        let mut oversize = jpeg.clone();
        oversize[9..11].copy_from_slice(&8193u16.to_be_bytes());
        assert!(jpeg_dimensions(&oversize, &PrepareControl::new()).is_err());
        let mut malformed = jpeg;
        malformed[5] = 0;
        assert!(jpeg_dimensions(&malformed, &PrepareControl::new()).is_err());
    }
    #[test]
    fn local_checked_source_totals_and_escaping_budget() {
        assert_eq!(
            add_bytes(MAX_TOTAL - MAX_MEMBER, MAX_MEMBER).unwrap(),
            MAX_TOTAL
        );
        assert!(add_bytes(MAX_TOTAL, 1).is_err());
        assert!(add_bytes(u64::MAX, 1).is_err());
        assert!(add_bytes(0, MAX_MEMBER + 1).is_err());
        let value = JsonValue::from("\"\\\t\n雪".repeat(100));
        let actual = value.dump().len() as u64;
        assert!(bounded_json(&value, actual).is_ok());
        assert!(bounded_json(&value, actual - 1).is_err());
    }
    fn source_hash_race(case: &str) {
        let f = Fixture::new();
        let root = f.root.join("admission");
        fs::create_dir(&root).unwrap();
        let paths = if case == "total" {
            let paths = (0..5)
                .map(|i| root.join(format!("p{i}.vpk")))
                .collect::<Vec<_>>();
            for path in &paths {
                fs::write(path, vpk("a.bin", 1)).unwrap();
            }
            paths
        } else {
            fs::write(
                root.join("pak_dir.vpk"),
                family_vpk(
                    1,
                    &[FamilyEntry {
                        path: "a.bin",
                        crc: 1,
                        archive: 0,
                        offset: 0,
                        length: 1,
                        preload: b"",
                    }],
                    b"",
                ),
            )
            .unwrap();
            let member = root.join("pak_000.vpk");
            fs::write(&member, b"x").unwrap();
            vec![member]
        };
        let c = PrepareControl::new();
        let observed = c.clone();
        let changed = Arc::new(AtomicUsize::new(0));
        let count = changed.clone();
        let paths_to_change = paths.clone();
        let case = case.to_owned();
        c.check_hook(Arc::new(move || {
            if observed.progress().phase == "hashing" && count.fetch_add(1, Ordering::SeqCst) == 0 {
                if case == "replace" {
                    let path = &paths_to_change[0];
                    fs::rename(path, path.with_extension("keep")).unwrap();
                    fs::write(path, vpk("a.bin", 1)).unwrap();
                } else {
                    for (i, path) in paths_to_change.iter().enumerate() {
                        let size = if case == "total" {
                            if i < 4 {
                                MAX_MEMBER
                            } else {
                                1
                            }
                        } else {
                            MAX_MEMBER + 1
                        };
                        fs::OpenOptions::new()
                            .write(true)
                            .open(path)
                            .unwrap()
                            .set_len(size)
                            .unwrap();
                    }
                }
            }
            if observed.hashes_started() > 0 {
                observed.request_cancel();
            }
        }));
        let result = prepare_local(req(&f, vec![root]), &c);
        c.check_hook(Arc::new(|| {}));
        assert_eq!(
            c.hashes_started(),
            0,
            "A known changed/oversized source must not start a SHA child"
        );
        assert!(matches!(
            result.unwrap_err().kind.as_str(),
            "stalePreview" | "localSourceTooLarge"
        ));
        assert!(!f.data.exists());
    }
    #[test]
    fn local_enlarged_member_is_rejected_before_sha() {
        source_hash_race("enlarge");
    }
    #[test]
    fn local_replaced_member_is_rejected_before_sha() {
        source_hash_race("replace");
    }
    #[test]
    fn local_changed_unique_total_is_rejected_before_sha() {
        source_hash_race("total");
    }
    #[test]
    fn local_artwork_enlarged_before_hash_is_rejected_without_sha() {
        let f = Fixture::new();
        let path = f.root.join("cover.png");
        let mut bytes = vec![0u8; 33];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[8..12].copy_from_slice(&13u32.to_be_bytes());
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&32u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&32u32.to_be_bytes());
        bytes[24] = 8;
        bytes[25] = 6;
        fs::write(&path, bytes).unwrap();
        let c = PrepareControl::new();
        let observed = c.clone();
        let check = Arc::new(AtomicUsize::new(0));
        let n = check.clone();
        let changed = path.clone();
        c.check_hook(Arc::new(move || {
            if n.fetch_add(1, Ordering::SeqCst) == 3 {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&changed)
                    .unwrap()
                    .set_len(16 * 1024 * 1024 + 1)
                    .unwrap();
            }
            if observed.hashes_started() > 0 {
                observed.request_cancel();
            }
        }));
        let result = artwork_source(&path, &c);
        c.check_hook(Arc::new(|| {}));
        assert_eq!(
            c.hashes_started(),
            0,
            "Artwork size/identity must be bound before spawning SHA"
        );
        assert!(matches!(
            result.unwrap_err().kind.as_str(),
            "stalePreview" | "invalidLocalArtwork" | "localSourceTooLarge"
        ));
        assert!(!f.data.exists());
    }

    #[test]
    fn local_strict_link_suffix_admission() {
        for accepted in [
            "https://example.invalid/a%20b/%E9%9B%AA?q=a%2Fb&x=1#part%20two",
            "http://[::1]:8080/a:@!$&'()*+,;=-._~?query/?#fragment/?",
        ] {
            assert!(valid_link(accepted), "{accepted}");
        }
        for rejected in [
            "https://example.invalid/%GG",
            "https://example.invalid/%",
            "https://example.invalid/%0",
            "https://example.invalid/<bad>",
            "https://example.invalid/a|b",
            "https://example.invalid/a[b]",
            "https://example.invalid/#one#two",
            "https://example.invalid/?q=\"bad\"",
        ] {
            assert!(!valid_link(rejected), "{rejected}");
        }
    }
    #[test]
    fn archive_preparation_job_evidence_is_v2_and_raw_api_is_compatible() {
        use crate::{archive_quarantine::QuarantineAuthority, local_job_store::LocalJobStore};
        let f = Fixture::new();
        fs::create_dir(&f.data).unwrap();
        let db = LocalJobStore::open(&f.data, true).unwrap().unwrap();
        let lease = QuarantineLease::create(
            &QuarantineAuthority::for_held_jobs_store(&db),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let source = f.root.join("raw.vpk");
        fs::write(&source, vpk("a.bin", 1)).unwrap();
        let r = req(&f, vec![source.clone()]);
        let raw = prepare_local(r.clone(), &PrepareControl::new()).unwrap();
        let job = prepare_local_job(r, &PrepareControl::new(), lease.clone()).unwrap();
        assert_eq!(raw.evidence["version"], 1);
        assert_eq!(job.evidence["version"], 3);
        assert_eq!(job.candidates[0].id, raw.candidates[0].id);
        revalidate_local(&job, &PrepareControl::new()).unwrap();
        fs::write(&source, vpk("b.bin", 2)).unwrap();
        assert_eq!(
            revalidate_local(&job, &PrepareControl::new())
                .unwrap_err()
                .kind,
            "stalePreview"
        );
        drop(job);
        lease
            .cleanup(&QuarantineAuthority::for_held_jobs_store(&db))
            .unwrap();
    }
    #[test]
    fn archive_preparation_explicit_and_folder_archives_are_not_silently_excluded() {
        use crate::{archive_quarantine::QuarantineAuthority, local_job_store::LocalJobStore};
        for folder in [false, true] {
            let f = Fixture::new();
            fs::create_dir(&f.data).unwrap();
            let db = LocalJobStore::open(&f.data, true).unwrap().unwrap();
            let lease = QuarantineLease::create(
                &QuarantineAuthority::for_held_jobs_store(&db),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap();
            let dir = f.root.join("sources");
            fs::create_dir(&dir).unwrap();
            let archive = dir.join("broken.zip");
            fs::write(&archive, b"not a zip").unwrap();
            let selected = if folder { dir } else { archive };
            let result = prepare_local_job(
                req(&f, vec![selected]),
                &PrepareControl::new(),
                lease.clone(),
            );
            assert!(
                result.is_err(),
                "archive must be admitted to its guarded worker, not silently excluded"
            );
            drop(result);
            lease
                .cleanup(&QuarantineAuthority::for_held_jobs_store(&db))
                .unwrap();
        }
    }
}

#[cfg(test)]
mod item104_tests {
 use super::*;
 #[test]
 fn item104_revalidation_reuses_content_and_detects_preserved_mtime_rewrite() {
  let f=crate::test_support::Fixture::new();
  crate::test_support::import(&mut crate::native::NativeService::new(),&f);
  let path=f.root.join("local_dir.vpk");fs::write(&path,crate::test_support::vpk("models/test.vmdl_c",1)).unwrap();
  let prepared=prepare_local(LocalPrepareRequest{root:f.data.clone(),profile_id:"default".into(),expected_revision:1,roots:vec![path.clone()],artwork:None},&PrepareControl::new()).unwrap();
  let control=PrepareControl::new();revalidate_local(&prepared,&control).unwrap();
  assert_eq!(control.hashes_started(),0,"revalidation must check identities without reading content again");
  let file=paths::open_regular(&path).unwrap();let times=fs::FileTimes::new().set_modified(file.metadata().unwrap().modified().unwrap());
  fs::write(&path,crate::test_support::vpk("models/test.vmdl_c",2)).unwrap();file.set_times(times).unwrap();
  assert!(revalidate_local(&prepared,&control).is_err(),"ctime detects a rewrite with preserved size and mtime");
 }
}

#[cfg(test)]
mod item104_font_scan_tests {
 use super::*;
 #[test]
 fn item104_font_scan_reuses_the_producers_vpk_hash() {
  use crate::archive_quarantine::QuarantineAuthority;
  let f=crate::archive_content::test_support::Fixture::new();let db=f.store();
  let authority=QuarantineAuthority::for_held_jobs_store(&db);
  let lease=QuarantineLease::create(&authority,"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
  let path=f.root.join("large_dir.vpk");fs::write(&path,crate::test_support::vpk("models/test.vmdl_c",1)).unwrap();fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(2*1024*1024).unwrap();
  let source=font_sources::FontSourceFile::raw(path.clone(),paths::fingerprint(&path).unwrap(),0,"large_dir.vpk".into());
  let inputs=vec![font_sources::FontInput::vpk("candidate".into(),source,BTreeMap::new())];
  let rchar=||fs::read_to_string("/proc/self/io").unwrap().lines().find_map(|l|l.strip_prefix("rchar: ").and_then(|v|v.parse::<u64>().ok())).unwrap();
  let before=rchar();crate::font_content::discover_fonts(&inputs,lease.clone(),&mut crate::font_content::FontBudget::new(),&PrepareControl::new()).unwrap();
  let bytes=rchar()-before;assert!(bytes<1_000_000,"font scan reread {bytes} bytes of an already hashed VPK");
  let held=paths::open_regular(&path).unwrap();let times=fs::FileTimes::new().set_modified(held.metadata().unwrap().modified().unwrap());
  std::io::Write::write_all(&mut fs::OpenOptions::new().write(true).open(&path).unwrap(),b"changed").unwrap();held.set_times(times).unwrap();
  assert!(crate::font_content::discover_fonts(&inputs,lease,&mut crate::font_content::FontBudget::new(),&PrepareControl::new()).is_err());
 }
}
