//! Bounded archive-to-flat-file preparation. Decoder paths are decoded display
//! provenance; the exact identity is (snapshot SHA-256, parser XADIndex).
use crate::{
    archive_process::{self, ProcessBudget, BUFFER},
    archive_quarantine::{QuarantineFile, QuarantineLease},
    native::NativeError,
    paths::{self, Fingerprint},
    prepare_control::PrepareControl,
};
use json::JsonValue;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
#[path = "../tests/archive_boundary/support.rs"]
pub(crate) mod test_support;
#[cfg(test)]
#[path = "../tests/archive_boundary/content.rs"]
mod tests;

pub const MAX_INPUT_ROOTS: usize = 64;
pub const MAX_ENTRIES: u64 = 10_000;
pub const MAX_CANDIDATES: u64 = 1_024;
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_JOB_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_LISTING: u64 = 8 * 1024 * 1024;
const MAX_RETAINED_LISTINGS: u64 = 16 * 1024 * 1024;

fn unsupported(reason: &str) -> NativeError { NativeError::new("archiveContractUnsupported", reason) }
fn reject(reason: &str) -> NativeError { NativeError::new("archiveRejected", reason) }
fn stale() -> NativeError { NativeError::new("stalePreview", "Archive source or quarantine identity changed; prepare again.") }
fn add(counter: &mut u64, value: u64, maximum: u64, reason: &str) -> Result<(), NativeError> {
    *counter = counter.checked_add(value).filter(|v| *v <= maximum).ok_or_else(|| archive_process::limit(reason))?;
    Ok(())
}

/// One budget per whole preparation job, including raw inputs. A failed
/// admission consumes previous charges; never reuse it to retry failed work.
#[derive(Default)]
pub struct ArchiveBudget {
    roots: usize,
    entries: u64,
    candidates: u64,
    input_bytes: u64,
    declared_bytes: u64,
    emitted_bytes: u64,
    listing_bytes: u64,
    process: ProcessBudget,
}
impl ArchiveBudget {
    pub fn new() -> Self { Self::default() }
    pub fn admit_roots(&mut self, count: usize) -> Result<(), NativeError> {
        self.roots = self.roots.checked_add(count).filter(|v| *v <= MAX_INPUT_ROOTS).ok_or_else(|| archive_process::limit("At most 64 input roots are supported."))?;
        Ok(())
    }
    /// Raw directory/exclusion entries pass None; raw regular files pass size.
    /// Archive inputs are charged by prepare_archive, so do not charge twice.
    pub fn admit_raw_entry(&mut self, relative_path: &str, size: Option<u64>) -> Result<(), NativeError> {
        // Raw filesystem semantics remain owned by local_content. Its root
        // directory marker is "."; archive traversal rules do not apply to it.
        if relative_path.len() > 4096 || relative_path.split('/').count() > 32 {
            return Err(archive_process::limit("Input path exceeds the shared path/depth bounds."));
        }
        add(&mut self.entries, 1, MAX_ENTRIES, "Raw plus archive entry count exceeds 10,000.")?;
        if let Some(size) = size {
            if size > MAX_FILE_BYTES { return Err(archive_process::limit("An input exceeds 16 GiB.")); }
            add(&mut self.input_bytes, size, MAX_JOB_BYTES, "Compressed plus loose input exceeds 64 GiB.")?;
        }
        Ok(())
    }
    /// Charge once per resolved package, including raw and archive families.
    pub fn admit_candidate(&mut self) -> Result<(), NativeError> {
        add(&mut self.candidates, 1, MAX_CANDIDATES, "Candidate package count exceeds 1,024.")
    }
    pub fn entries(&self) -> u64 { self.entries }
    pub fn input_bytes(&self) -> u64 { self.input_bytes }
    pub fn declared_bytes(&self) -> u64 { self.declared_bytes }
    pub fn emitted_bytes(&self) -> u64 { self.emitted_bytes }
}

#[derive(Clone, Debug)]
pub struct ArchiveInput {
    pub root_ordinal: usize,
    pub relative_path: String,
    pub original_path: PathBuf,
    pub fingerprint: Fingerprint,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveKind { Regular, Directory, Rejected(String) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveMember {
    pub index: u32,
    pub decoder_path: String,
    pub normalized_path: String,
    pub size: u64,
    pub kind: ArchiveKind,
}
#[derive(Clone, Debug)]
pub struct ExtractedMember { member: ArchiveMember, path: PathBuf, fingerprint: Fingerprint, role: ArchiveContentRole }
impl ExtractedMember {
    pub fn role(&self) -> ArchiveContentRole { self.role }
    pub fn member(&self) -> &ArchiveMember { &self.member }
    pub fn path(&self) -> &Path { &self.path }
    pub fn fingerprint(&self) -> &Fingerprint { &self.fingerprint }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveContentRole { Vpk, BundledTtf }
pub enum ArchiveSelection { Extract(ArchiveContentRole), Exclude(&'static str) }
pub struct ArchiveRegularMember {
    pub index: u32, pub decoder_path: String, pub normalized_path: String, pub size: u64,
}
pub trait ArchiveMemberSelector {
    fn select(&mut self, input: &ArchiveInput, member: &ArchiveRegularMember) -> Result<ArchiveSelection, NativeError>;
}
struct VpkSelector;
impl ArchiveMemberSelector for VpkSelector {
    fn select(&mut self, _: &ArchiveInput, m: &ArchiveRegularMember) -> Result<ArchiveSelection, NativeError> {
        Ok(if m.normalized_path.to_ascii_lowercase().ends_with(".vpk") {
            ArchiveSelection::Extract(ArchiveContentRole::Vpk)
        } else if [".zip", ".rar", ".7z"].iter().any(|e| m.normalized_path.to_ascii_lowercase().ends_with(e)) {
            ArchiveSelection::Exclude("nestedArchiveNotExpanded")
        } else { ArchiveSelection::Exclude("notVpk") })
    }
}
pub struct PreparedArchive {
    input: ArchiveInput,
    snapshot_path: PathBuf,
    snapshot_fingerprint: Fingerprint,
    format: String,
    encoding: String,
    manifest: Vec<ArchiveMember>,
    extracted: Vec<ExtractedMember>,
    listing: Vec<u8>,
    lease: Arc<QuarantineLease>,
}
impl std::fmt::Debug for PreparedArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedArchive")
            .field("input", &self.input)
            .field("format", &self.format)
            .field("member_count", &self.manifest.len())
            .field("extracted_count", &self.extracted.len())
            .finish_non_exhaustive()
    }
}
impl PreparedArchive {
    pub fn input(&self) -> &ArchiveInput { &self.input }
    pub fn snapshot_path(&self) -> &Path { &self.snapshot_path }
    pub fn snapshot_fingerprint(&self) -> &Fingerprint { &self.snapshot_fingerprint }
    pub fn format(&self) -> &str { &self.format }
    pub fn encoding(&self) -> &str { &self.encoding }
    pub fn members(&self) -> &[ArchiveMember] { &self.manifest }
    pub fn extracted_members(&self) -> &[ExtractedMember] { &self.extracted }
    pub fn retained_listing_bytes(&self) -> usize { self.listing.len() }
    pub fn lease(&self) -> &Arc<QuarantineLease> { &self.lease }
}

/// Depth admission occurs before the generic JSON parser allocates a tree.
/// Quotes and escapes are recognized; syntax validation remains the parser's job.
pub(crate) fn check_json_depth(bytes: &[u8], maximum: usize) -> Result<(), NativeError> {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped { escaped = false; }
            else if byte == b'\\' { escaped = true; }
            else if byte == b'"' { quoted = false; }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => { depth += 1; if depth > maximum { return Err(archive_process::limit("Archive JSON depth exceeds 64.")); } },
                b'}' | b']' => { depth = depth.checked_sub(1).ok_or_else(|| unsupported("Malformed archive JSON."))?; },
                _ => {},
            }
        }
    }
    if depth != 0 || quoted { return Err(unsupported("Incomplete archive JSON.")); }
    Ok(())
}
fn parse(bytes: &[u8]) -> Result<JsonValue, NativeError> {
    check_json_depth(bytes, 64)?;
    json::parse(std::str::from_utf8(bytes).map_err(|_| unsupported("Archive listing is not UTF-8."))?).map_err(|_| unsupported("Archive listing JSON is malformed."))
}
fn flag(value: &JsonValue, name: &str) -> Result<bool, NativeError> {
    if !value.has_key(name) { return Ok(false); }
    // CSJSONPrinter emits true/false for NSNumber objCType "c", otherwise its
    // numeric description. Admit only the exact boolean domain in either form.
    if let Some(boolean) = value[name].as_bool() { return Ok(boolean); }
    match value[name].as_u64() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(unsupported("Archive flag has an unsupported representation.")),
    }
}
fn normalize_path(path: &str, directory: bool) -> Result<String, NativeError> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(|c| c.is_control() || c == '\u{fffd}')
        || path.starts_with(['/', '\\']) || path.contains(':')
    { return Err(reject("Archive path is absolute, undecodable, contains controls, or exceeds 4,096 UTF-8 bytes.")); }
    // XADString falls back to "%hh" for undecodable bytes. A literal escape
    // spelling is indistinguishable in lsar JSON, so both cases fail closed.
    if path.as_bytes().windows(3).any(|b| b[0] == b'%' && b[1].is_ascii_hexdigit() && b[2].is_ascii_hexdigit()) {
        return Err(unsupported("Archive member name contains an ambiguous undecodable-byte escape."));
    }
    let converted = path.replace('\\', "/");
    let normalized = if directory { converted.strip_suffix('/').unwrap_or(&converted) } else { &converted };
    let mut count = 0usize;
    for part in normalized.split('/') {
        count += 1;
        if part.is_empty() || part == "." || part == ".." || part == "(nil)" { return Err(reject("Archive path contains empty, undecodable, or traversal components.")); }
    }
    if count > 32 { return Err(archive_process::limit("Archive path depth exceeds 32.")); }
    Ok(normalized.to_owned())
}
fn folded(path: &str) -> String { path.chars().flat_map(char::to_uppercase).flat_map(char::to_lowercase).collect() }

struct Envelope { value: JsonValue, format: String, encoding: String }
fn envelope(bytes: &[u8], snapshot: &Path) -> Result<Envelope, NativeError> {
    let value = parse(bytes)?;
    if !value.is_object() || value["lsarFormatVersion"].as_u32() != Some(2)
        || value.has_key("lsarError") || !value["lsarContents"].is_array()
        || !value["lsarProperties"].is_object()
        || value.has_key("lsarInnerFormatName") || value.has_key("lsarInnerProperties")
    { return Err(unsupported("Unsupported or incomplete lsar JSON envelope; nested expansion is forbidden.")); }
    let format = value["lsarFormatName"].as_str().filter(|s| matches!(*s, "Zip" | "7-Zip" | "RAR" | "RAR 5")).ok_or_else(|| unsupported("Only ZIP, RAR and 7z decoder formats are admitted."))?.to_owned();
    let encoding = value["lsarEncoding"].as_str().filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
        && !s.eq_ignore_ascii_case("help") && !s.eq_ignore_ascii_case("list")).ok_or_else(|| unsupported("Missing or unsupported filename encoding."))?.to_owned();
    if !value["lsarConfidence"].as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)) { return Err(unsupported("Invalid filename encoding confidence.")); }
    let props = &value["lsarProperties"];
    if flag(props, "XADIsEncrypted")? { return Err(reject("Encrypted archives are not supported.")); }
    if flag(props, "XADIsCorrupted")? { return Err(reject("Archive is marked corrupted.")); }
    let _solid = flag(props, "XADIsSolid")?;
    if flag(props, "XADVolumeScanningFailed")? || !props["XADVolumes"].is_array()
        || props["XADVolumes"].len() != 1 || props["XADVolumes"][0].as_str() != snapshot.to_str()
    { return Err(reject("Multipart archives or failed volume scans are not supported.")); }
    Ok(Envelope { value, format, encoding })
}
fn member(value: &JsonValue) -> Result<ArchiveMember, NativeError> {
    if !value.is_object() { return Err(unsupported("Archive member must be an object.")); }
    let index = value["XADIndex"].as_u32().filter(|v| *v <= i32::MAX as u32).ok_or_else(|| unsupported("Archive index is missing, negative, or exceeds i32."))?;
    let decoder_path = value["XADFileName"].as_str().ok_or_else(|| unsupported("Archive member name is not decoded text."))?.to_owned();
    let directory = flag(value, "XADIsDirectory")?;
    let normalized_path = normalize_path(&decoder_path, directory)?;
    let encrypted = flag(value, "XADIsEncrypted")?;
    let corrupted = flag(value, "XADIsCorrupted")?;
    let _solid = flag(value, "XADIsSolid")?;
    let mut special = value.has_key("XADLinkDestination");
    for key in ["XADIsLink", "XADIsHardLink", "XADIsCharacterDevice", "XADIsBlockDevice", "XADIsFIFO", "XADIsResourceFork"] { special |= flag(value, key)?; }
    if value.has_key("XADLinkDestination") && !value["XADLinkDestination"].is_string() { return Err(unsupported("Malformed archive link destination.")); }
    if value.has_key("XADPosixPermissions") {
        let mode = value["XADPosixPermissions"].as_u32().ok_or_else(|| unsupported("Malformed archive POSIX mode."))?;
        let kind = mode & 0o170000;
        special |= kind != 0 && kind != if directory { 0o040000 } else { 0o100000 };
    }
    if encrypted { return Err(reject("Encrypted members are not supported.")); }
    if corrupted { return Err(reject("Archive member is marked corrupted.")); }
    let size = if value.has_key("XADFileSize") {
        value["XADFileSize"].as_u64().ok_or_else(|| unsupported("Archive member size must be a nonnegative integer."))?
    } else if directory || special { 0 } else { return Err(unsupported("Regular archive member has no known size.")); };
    if size > MAX_FILE_BYTES { return Err(archive_process::limit("An archive member exceeds 16 GiB.")); }
    let kind = if special { ArchiveKind::Rejected("linkOrSpecialEntry".into()) }
        else if directory { ArchiveKind::Directory }
        else { ArchiveKind::Regular };
    Ok(ArchiveMember { index, decoder_path, normalized_path, size, kind })
}
fn manifest(envelope: &Envelope, budget: &mut ArchiveBudget) -> Result<Vec<ArchiveMember>, NativeError> {
    let mut indices = HashSet::new();
    let mut names = BTreeMap::new();
    let mut prefixes = BTreeMap::new();
    let mut result = Vec::new();
    for value in envelope.value["lsarContents"].members() {
        add(&mut budget.entries, 1, MAX_ENTRIES, "Raw plus archive entries exceed 10,000.")?;
        let member = member(value)?;
        add(&mut budget.declared_bytes, member.size, MAX_JOB_BYTES, "Declared archive expansion exceeds 64 GiB.")?;
        if !indices.insert(member.index) { return Err(unsupported("Duplicate archive parser index.")); }
        let key = folded(&member.normalized_path);
        if names.insert(key, matches!(member.kind, ArchiveKind::Directory)).is_some() { return Err(reject("Duplicate or case-colliding archive member paths.")); }
        for at in member.normalized_path.match_indices('/').map(|(at, _)| at).chain(std::iter::once(member.normalized_path.len())) {
            let prefix = &member.normalized_path[..at];
            if let Some(previous) = prefixes.insert(folded(prefix), prefix.to_owned()) {
                if previous != prefix { return Err(reject("Archive directory prefixes have conflicting case.")); }
            }
        }
        result.push(member);
    }
    for path in names.keys() {
        for (at, _) in path.match_indices('/') {
            if names.get(&path[..at]) == Some(&false) { return Err(reject("Archive file path is also used as a directory prefix.")); }
        }
    }
    Ok(result)
}
fn args(values: &[&str], snapshot: &Path, index: Option<u32>, encoding: Option<&str>) -> Vec<OsString> {
    let mut result: Vec<OsString> = values.iter().map(OsString::from).collect();
    if let Some(encoding) = encoding { result.extend([OsString::from("-e"), OsString::from(encoding)]); }
    result.push(snapshot.as_os_str().to_owned());
    if let Some(index) = index { result.push(index.to_string().into()); }
    result
}
fn stable_metadata(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.is_file() && b.is_file() && (a.dev(),a.ino(),a.len(),a.mtime(),a.mtime_nsec(),a.ctime(),a.ctime_nsec(),a.mode(),a.nlink())
        == (b.dev(),b.ino(),b.len(),b.mtime(),b.mtime_nsec(),b.ctime(),b.ctime_nsec(),b.mode(),b.nlink())
}
fn fingerprint_matches(meta: &fs::Metadata, fp: &Fingerprint) -> bool {
    meta.is_file() && (meta.dev(),meta.ino(),meta.len(),meta.mtime(),meta.mtime_nsec())
        == (fp.device,fp.inode,fp.size,fp.modified_seconds,fp.modified_nanos)
}
fn fp(meta: &fs::Metadata, sha256: String) -> Fingerprint {
    Fingerprint { sha256, size:meta.len(), modified_seconds:meta.mtime(), modified_nanos:meta.mtime_nsec(), device:meta.dev(), inode:meta.ino() }
}
/// Read at most the exact admitted size plus one byte, checking cancellation
/// between 64 KiB chunks; hashing never invokes an unbounded external reader.
#[cfg(test)]
thread_local! { static DIGEST_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
fn digest(file: &mut File, length: u64, budget: &mut ProcessBudget, control: &PrepareControl) -> Result<String, NativeError> {
    #[cfg(test)]
    DIGEST_BYTES.with(|n|n.set(n.get()+length));
    file.seek(SeekFrom::Start(0))?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() != length { return Err(stale()); }
    let mut bytes = Vec::new();
    archive_process::run_with_input("sha256sum", &[], &mut bytes, 128, Some((file, length)), budget, control)?;
    if !stable_metadata(&before, &file.metadata()?) { return Err(stale()); }
    let text = std::str::from_utf8(&bytes).map_err(|_| unsupported("Malformed SHA-256 output."))?;
    let digest = text.strip_suffix("  -\n").filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())).ok_or_else(|| unsupported("Malformed SHA-256 output."))?;
    Ok(digest.to_owned())
}
fn snapshot(input: &ArchiveInput, output: &mut QuarantineFile, budget: &mut ProcessBudget, control: &PrepareControl) -> Result<Fingerprint, NativeError> {
    let mut source = paths::open_regular(&input.original_path)?;
    snapshot_from_file(&mut source, &input.fingerprint, Some(&input.original_path), output, budget, control)
}
fn snapshot_from_file(source: &mut File, expected: &Fingerprint, named: Option<&Path>, output: &mut QuarantineFile, _budget: &mut ProcessBudget, control: &PrepareControl) -> Result<Fingerprint, NativeError> {
    let before = source.metadata()?;
    if !fingerprint_matches(&before, expected) { return Err(stale()); }
    source.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = before.len();
    let mut buffer = [0u8; BUFFER];
    while remaining != 0 {
        control.check()?;
        let n = source.read(&mut buffer[..remaining.min(BUFFER as u64) as usize])?;
        if n == 0 { return Err(stale()); }
        output.admit_write(n)?;
        output.file.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    control.check()?;
    if source.read(&mut buffer[..1])? != 0
        || !stable_metadata(&before, &source.metadata()?)
        || match named { Some(path) => !stable_metadata(&before,&paths::open_regular(path)?.metadata()?), None => false }
    { return Err(stale()); }
    output.file.sync_all()?;
    let meta = output.file.metadata()?;
    let sha = format!("{:x}",hash.finalize());
    if sha != expected.sha256 || !stable_metadata(&meta, &output.file.metadata()?)
        || !stable_metadata(&meta, &paths::open_regular(&output.path)?.metadata()?) { return Err(stale()); }
    Ok(fp(&meta, sha))
}

/// Snapshot only the caller's held, already-admitted file. The caller retains
/// its claim-directory and lease proofs and revalidates them around this copy.
pub(crate) fn snapshot_held_member(source: &mut File, name: &str, expected: &Fingerprint, lease: &Arc<QuarantineLease>, budget: &mut ArchiveBudget, control: &PrepareControl) -> Result<(PathBuf,Fingerprint),NativeError> {
    budget.admit_raw_entry(name,Some(expected.size))?;
    let mut output=lease.open_claim(lease.allocate(1,control)?.remove(0))?;
    let fingerprint=snapshot_from_file(source,expected,None,&mut output,&mut budget.process,control)?;
    lease.complete_batch(&[output.name.clone()])?;
    Ok((output.path,fingerprint))
}

pub fn prepare_archive(input: ArchiveInput, lease: Arc<QuarantineLease>, budget: &mut ArchiveBudget, control: &PrepareControl) -> Result<PreparedArchive, NativeError> {
    prepare_archive_selected(input, lease, budget, &mut VpkSelector, control)
}
pub fn prepare_archive_selected(input: ArchiveInput, lease: Arc<QuarantineLease>, budget: &mut ArchiveBudget,
    selector: &mut dyn ArchiveMemberSelector, control: &PrepareControl) -> Result<PreparedArchive, NativeError> {
    prepare_archive_impl(input,lease,budget,selector,control,None)
}
pub(crate) fn prepare_archive_held(input: ArchiveInput, source: &mut File, lease: Arc<QuarantineLease>, budget: &mut ArchiveBudget, control: &PrepareControl) -> Result<PreparedArchive, NativeError> {
    prepare_archive_impl(input,lease,budget,&mut VpkSelector,control,Some(source))
}
fn prepare_archive_impl(input: ArchiveInput, lease: Arc<QuarantineLease>, budget: &mut ArchiveBudget,
    selector: &mut dyn ArchiveMemberSelector, control: &PrepareControl, held: Option<&mut File>) -> Result<PreparedArchive, NativeError> {
    control.check()?;
    if input.root_ordinal >= budget.roots || held.is_none() && !input.original_path.is_absolute()
        || input.fingerprint.sha256.len() != 64 || !input.fingerprint.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    { return Err(NativeError::params()); }
    budget.admit_raw_entry(&input.relative_path, Some(input.fingerprint.size))?;
    let mut source = lease.open_claim(lease.allocate(1, control)?.remove(0))?;
    control.report(crate::prepare_control::PrepareProgress {
        phase: "copyingArchives".into(),
        ..Default::default()
    });
    let snapshot_fingerprint = match held {
        Some(file) => snapshot_from_file(file,&input.fingerprint,None,&mut source,&mut budget.process,control)?,
        None => snapshot(&input, &mut source, &mut budget.process, control)?,
    };
    let snapshot_metadata=source.file.metadata()?;
    lease.complete_batch(&[source.name.clone()])?;
    control.report(crate::prepare_control::PrepareProgress {
        phase: "listingArchives".into(),
        ..Default::default()
    });
    let mut listing = Vec::new();
    archive_process::run("lsar", &args(&["-ja", "-nr"], &source.path, None, None), &mut listing, MAX_LISTING, &mut budget.process, control)?;
    add(&mut budget.listing_bytes, listing.len() as u64, MAX_RETAINED_LISTINGS, "Retained archive listings exceed 16 MiB.")?;
    let listing_envelope = envelope(&listing, &source.path)?;
    // Admit the entire physical manifest before any policy selector runs.
    let mut members = manifest(&listing_envelope, budget)?;
    let mut selected = Vec::new();
    for member in &mut members {
        if member.kind != ArchiveKind::Regular { continue; }
        let regular = ArchiveRegularMember { index: member.index, decoder_path: member.decoder_path.clone(),
            normalized_path: member.normalized_path.clone(), size: member.size };
        match selector.select(&input, &regular)? {
            ArchiveSelection::Extract(role) => selected.push((member.clone(), role)),
            ArchiveSelection::Exclude(reason) => member.kind = ArchiveKind::Rejected(reason.into()),
        }
    }
    let outputs = lease.allocate(selected.len(), control)?;
    let mut extracted = Vec::with_capacity(outputs.len());
    let mut completed = Vec::with_capacity(outputs.len());
    control.report(crate::prepare_control::PrepareProgress {
        phase: "extractingArchives".into(),
        ..Default::default()
    });
    for ((member, role), claim) in selected.into_iter().zip(outputs) {
        control.check()?;
        let mut output = lease.open_claim(claim)?;
        if !fingerprint_matches(&source.file.metadata()?, &snapshot_fingerprint)
            || !fingerprint_matches(&paths::open_regular(&source.path)?.metadata()?, &snapshot_fingerprint) { return Err(stale()); }
        let remaining = MAX_JOB_BYTES.checked_sub(budget.emitted_bytes).ok_or_else(|| archive_process::limit("Emitted expansion overflow."))?;
        let mut sink = CountedOutput { output:&mut output, emitted:&mut budget.emitted_bytes };
        let emitted = archive_process::run("unar", &args(&["-o", "-", "-nr", "-i", "-k", "skip"], &source.path, Some(member.index), Some(&listing_envelope.encoding)), &mut sink, member.size.min(remaining), &mut budget.process, control)?;
        if emitted != member.size { return Err(reject("Extracted member length differs from its declared length.")); }
        output.file.sync_all()?;
        let mut integrity = Vec::new();
        archive_process::run("lsar", &args(&["-ja", "-nr", "-t", "-i"], &source.path, Some(member.index), Some(&listing_envelope.encoding)), &mut integrity, MAX_LISTING, &mut budget.process, control)?;
        let checked = envelope(&integrity, &source.path)?;
        if checked.format != listing_envelope.format || checked.encoding != listing_envelope.encoding
            || checked.value["lsarContents"].len() != 1 || self::member(&checked.value["lsarContents"][0])? != member
        { return Err(unsupported("Indexed integrity result does not match the admitted member.")); }
        if checked.value["lsarContents"][0]["lsarTestResult"].as_str() != Some("ok") { return Err(reject("Member has no verifiable checksum or failed archive integrity verification.")); }
        let before = output.file.metadata()?;
        let sha = digest(&mut output.file, member.size, &mut budget.process, control)?;
        if !stable_metadata(&before, &output.file.metadata()?) || !stable_metadata(&before, &paths::open_regular(&output.path)?.metadata()?) { return Err(stale()); }
        extracted.push(ExtractedMember { member:member.clone(), path:output.path, fingerprint:fp(&before,sha), role });
        completed.push(output.name);
    }
    // Content was verified during the snapshot copy. Recheck its identity
    // after decoder reads without reading the complete archive again.
    if !stable_metadata(&snapshot_metadata,&source.file.metadata()?)
        || !stable_metadata(&snapshot_metadata,&paths::open_regular(&source.path)?.metadata()?) { return Err(stale()); }
    lease.complete_batch(&completed)?;
    control.check()?;
    Ok(PreparedArchive { input, snapshot_path:source.path, snapshot_fingerprint, format:listing_envelope.format, encoding:listing_envelope.encoding, manifest:members, extracted, listing, lease })
}
struct CountedOutput<'a> { output: &'a mut QuarantineFile, emitted: &'a mut u64 }
impl Write for CountedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.output.admit_write(bytes.len()).map_err(|_|std::io::Error::other("Quarantine storage headroom is unavailable."))?;
        let n = self.output.file.write(bytes)?;
        *self.emitted = self.emitted.checked_add(n as u64).ok_or_else(|| std::io::Error::other("archive byte counter overflow"))?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.output.file.flush() }
}

