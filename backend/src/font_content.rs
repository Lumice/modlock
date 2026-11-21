//! Optional bundled TrueType assets. Provenance is descriptive; only sealed
//! local-content inputs authorize reads. This module neither discovers roots
//! nor installs fonts, and performs no archive decoding.
use crate::{archive_content::PreparedArchive, archive_process::{self, ProcessBudget},
    archive_quarantine::QuarantineLease, local_content::font_sources::{FontInput, FontInputKind, FontSourceFile},
    native::NativeError, paths::{self, Fingerprint}, prepare_control::{self, PrepareControl}, vpk};
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, File}, io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt, path::{Path, PathBuf}, sync::Arc};

pub const MAX_FONT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FONT_JOB_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_FONT_CANDIDATES: usize = 256;
const MAX_RECORDS: usize = 4096;
const MAX_UNAVAILABLE: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontContainer {
    Raw { root_ordinal: usize, relative_path: String },
    Archive { root_ordinal: usize, archive_relative_path: String, archive_sha256: String,
        member_index: u32, decoder_path: String, normalized_path: String },
}
impl FontContainer {
    fn relative_path(&self) -> &str {
        match self { Self::Raw { relative_path, .. } => relative_path,
            Self::Archive { normalized_path, .. } => normalized_path }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontOrigin {
    UnavailableVpk { container: FontContainer, candidate_id: String, members: Vec<VpkFontMemberOrigin> },
    Loose { container: FontContainer },
    Vpk { container: FontContainer, candidate_id: String, resource_path: String,
        resource_index: usize, archive_index: u16, resource_crc: u32, members: Vec<VpkFontMemberOrigin> },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpkFontMemberOrigin {
    pub archive_index: Option<u16>, pub container: FontContainer, pub sha256: String, pub size: u64,
}

/// Canonical source identity, never an I/O path. JSON here encodes typed values
/// only; no evidence or provenance is decoded by the producer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FontSourceKey(String);
impl FontSourceKey {
    pub(crate) fn container(origin: &FontContainer) -> Self {
        Self(match origin {
            FontContainer::Raw { root_ordinal, relative_path } => json::array!["raw", *root_ordinal, relative_path.as_str()].dump(),
            FontContainer::Archive { root_ordinal, archive_relative_path, archive_sha256, member_index, .. } =>
                json::array!["archive", *root_ordinal, archive_relative_path.as_str(), archive_sha256.as_str(), *member_index].dump(),
        })
    }
    pub(crate) fn vpk(candidate_id: &str, index: usize) -> Self { Self(json::array!["vpk", candidate_id, index].dump()) }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontAdmission { Available, Unavailable(&'static str) }
#[derive(Default)]
pub struct FontBudget { admitted: BTreeMap<FontSourceKey, u64>, bytes: u64, process: ProcessBudget }
impl FontBudget {
    pub fn new() -> Self { Self::default() }
    /// Pre-coalescing policy. Previously charged keys are idempotent; callers
    /// retain this same budget across owner pre-admission and discovery.
    pub fn admit(&mut self, key: &FontSourceKey, size: u64) -> Result<FontAdmission, NativeError> {
        if let Some(previous) = self.admitted.get(key) {
            return if *previous == size { Ok(FontAdmission::Available) } else { Err(stale()) };
        }
        if size > MAX_FONT_BYTES { return Ok(FontAdmission::Unavailable("Font exceeds 16 MiB.")); }
        if self.admitted.len() >= MAX_FONT_CANDIDATES { return Ok(FontAdmission::Unavailable("Font source count exceeds 256.")); }
        let Some(total) = self.bytes.checked_add(size).filter(|n| *n <= MAX_FONT_JOB_BYTES) else {
            return Ok(FontAdmission::Unavailable("Font bytes exceed 128 MiB per job."));
        };
        self.admitted.insert(key.clone(), size); self.bytes = total;
        Ok(FontAdmission::Available)
    }
    pub fn admitted_count(&self) -> usize { self.admitted.len() }
    pub fn admitted_bytes(&self) -> u64 { self.bytes }
}

pub struct PreparedFont {
    path: PathBuf, fingerprint: Fingerprint, lease: Arc<QuarantineLease>,
    // Keep archive producer/manifest authority shared for reused members.
    archive: Option<Arc<PreparedArchive>>,
    sources: Vec<FontSourceFile>,
}
impl PreparedFont {
    pub fn path(&self) -> &Path { &self.path }
    pub fn fingerprint(&self) -> &Fingerprint { &self.fingerprint }
    pub fn lease(&self) -> &Arc<QuarantineLease> { &self.lease }
    pub fn archive(&self) -> Option<&Arc<PreparedArchive>> { self.archive.as_ref() }
    pub(crate) fn source_files(&self) -> &[FontSourceFile] { &self.sources }
}
impl std::fmt::Debug for PreparedFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedFont").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}
#[derive(Clone, Debug)]
pub struct FontCandidate {
    pub id: String, pub target_name: String, pub display_name: String, pub origins: Vec<FontOrigin>,
    pub prepared: Option<Arc<PreparedFont>>, pub unavailable_reason: Option<String>,
    name_collision: bool,
}
fn invalid(message: &str) -> NativeError { NativeError::new("fontUnavailable", message) }
fn stale() -> NativeError { NativeError::new("stalePreview", "Font source authority changed or disappeared; prepare again.") }
fn folded(value: &str) -> String { value.chars().flat_map(char::to_uppercase).flat_map(char::to_lowercase).collect() }

/// Match actual consecutive components, not a substring. The owner supplies
/// source-relative or archive-normalized paths; VPK parsing validates its path.
pub(crate) fn bundled_ttf(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() >= 3 && parts.windows(2).any(|p| p[0].eq_ignore_ascii_case("panorama") && p[1].eq_ignore_ascii_case("fonts"))
        && parts.last().is_some_and(|name| name.to_ascii_lowercase().ends_with(".ttf"))
}
fn target_name(path: &str) -> Result<&str, NativeError> {
    let name = path.rsplit('/').next().ok_or_else(|| invalid("Missing font filename."))?;
    if !paths::safe_name(name) || name.len() > 240 || !name.to_ascii_lowercase().ends_with(".ttf") {
        return Err(invalid("Unsafe or excessive font target name."));
    }
    Ok(name)
}
fn u16at(bytes: &[u8], at: usize) -> Result<u16, NativeError> {
    let b = bytes.get(at..at.checked_add(2).ok_or_else(|| invalid("Invalid font offset."))?)
        .ok_or_else(|| invalid("Truncated font structure."))?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}
fn u32at(bytes: &[u8], at: usize) -> Result<u32, NativeError> {
    let b = bytes.get(at..at.checked_add(4).ok_or_else(|| invalid("Invalid font offset."))?)
        .ok_or_else(|| invalid("Truncated font structure."))?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}
fn name_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && value.chars().count() <= 256
        && value.chars().all(|c| !c.is_control() && matches!(c as u32, 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff))
}
/// Validate every name record range before decoding any preferred string.
/// Format 1's language-tag records follow the name records per OpenType.
fn font_name(data: &[u8], filename: &str) -> Result<String, NativeError> {
    let format = u16at(data, 0)?;
    let count = usize::from(u16at(data, 2)?);
    let storage = usize::from(u16at(data, 4)?);
    if format > 1 || count > MAX_RECORDS { return Err(invalid("Unsupported or excessive name records.")); }
    let mut records_end = 6 + 12 * count;
    if records_end > data.len() { return Err(invalid("Truncated name records.")); }
    if format == 1 {
        let languages = usize::from(u16at(data, records_end)?);
        if languages > MAX_RECORDS { return Err(invalid("Excessive language records.")); }
        let start = records_end + 2; records_end = start + 4 * languages;
        if records_end > data.len() { return Err(invalid("Truncated language records.")); }
        for index in 0..languages { name_span(data, storage, start + 4 * index)?; }
    }
    if storage < records_end || storage > data.len() { return Err(invalid("Invalid name string storage.")); }
    let mut preferred = Vec::new();
    for index in 0..count {
        let at = 6 + 12 * index;
        let platform = u16at(data, at)?; let encoding = u16at(data, at + 2)?;
        let language = u16at(data, at + 4)?; let id = u16at(data, at + 6)?;
        let text = name_span(data, storage, at + 8)?;
        let unicode = platform == 0 || platform == 3 && matches!(encoding, 0 | 1 | 10);
        if unicode && text.len() % 2 != 0 { return Err(invalid("Malformed Unicode name record.")); }
        if !unicode || !matches!(id, 1 | 4) { continue; }
        let rank = if platform == 3 && language & 0x03ff == 9 { 0 } else if platform == 0 { 1 } else { 2 };
        preferred.push(((id, rank, platform, encoding, language, index), text));
    }
    preferred.sort_by_key(|(rank, _)| *rank);
    for (_, bytes) in preferred {
        // Each record is at most u16::MAX bytes. Reject an overlong decoded
        // name instead of truncating it into another family name.
        let decoded: Result<String, _> = char::decode_utf16(bytes.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]]))).collect();
        let Ok(decoded) = decoded else { continue; };
        let value = decoded.trim();
        if value.len() > 1024 || value.chars().count() > 256 { return Err(invalid("Font family name exceeds supported bounds.")); }
        if name_text(value) { return Ok(value.to_owned()); }
    }
    let fallback = filename.get(..filename.len().saturating_sub(4)).unwrap_or("").trim();
    if !name_text(fallback) { return Err(invalid("No usable font family or filename fallback.")); }
    Ok(fallback.to_owned())
}
fn name_span(data: &[u8], storage: usize, at: usize) -> Result<&[u8], NativeError> {
    let length = usize::from(u16at(data, at)?); let offset = usize::from(u16at(data, at + 2)?);
    data.get(storage + offset..storage + offset + length).ok_or_else(|| invalid("Name record exceeds string storage."))
}

// The parser resolves glyph IDs. This preflight only bounds subtable slices
// and supplies codepoints from their declared coverage, avoiding enumeration
// of potentially enormous format 12/13 Unicode ranges.
fn usable_cmap(data: &[u8], glyphs: usize) -> Result<bool, NativeError> {
    let count = usize::from(u16at(data, 2)?);
    if u16at(data, 0)? != 0 || count == 0 || count > MAX_RECORDS || 4 + 8 * count > data.len() {
        return Err(invalid("Invalid character-map records."));
    }
    let mut seen = BTreeSet::new();
    let mut usable = false;
    for index in 0..count {
        let at = 4 + index * 8;
        let platform = u16at(data, at)?; let encoding = u16at(data, at + 2)?;
        let offset = u32at(data, at + 4)? as usize;
        if offset < 4 + 8 * count { return Err(invalid("Character map overlaps its directory.")); }
        let tail = data.get(offset..).ok_or_else(|| invalid("Character-map offset exceeds table."))?;
        let format = u16at(tail, 0)?;
        let length = match format {
            0 | 2 | 4 | 6 => usize::from(u16at(tail, 2)?),
            8 | 10 | 12 | 13 => u32at(tail, 4)? as usize,
            14 => u32at(tail, 2)? as usize,
            _ => continue,
        };
        let sub = tail.get(..length).filter(|b| b.len() >= 4).ok_or_else(|| invalid("Character-map span exceeds table."))?;
        if !seen.insert((offset, platform, encoding)) || !(platform == 0 || platform == 3 && matches!(encoding, 1 | 10)) { continue; }
        // Parsing a bounded standalone subtable makes declared length binding;
        // the complete Face parser otherwise sees the remainder of cmap.
        use ttf_parser::cmap::{Format, Subtable};
        let parsed = match format {
            0 => ttf_parser::cmap::Subtable0::parse(sub).map(Format::ByteEncodingTable),
            2 => ttf_parser::cmap::Subtable2::parse(sub).map(Format::HighByteMappingThroughTable),
            4 => ttf_parser::cmap::Subtable4::parse(sub).map(Format::SegmentMappingToDeltaValues),
            6 => ttf_parser::cmap::Subtable6::parse(sub).map(Format::TrimmedTableMapping),
            10 => ttf_parser::cmap::Subtable10::parse(sub).map(Format::TrimmedArray),
            12 => ttf_parser::cmap::Subtable12::parse(sub).map(Format::SegmentedCoverage),
            13 => ttf_parser::cmap::Subtable13::parse(sub).map(Format::ManyToOneRangeMappings),
            _ => continue,
        }.ok_or_else(|| invalid("Malformed Unicode character map."))?;
        let parsed = Subtable { platform_id: if platform == 0 { ttf_parser::PlatformId::Unicode } else { ttf_parser::PlatformId::Windows }, encoding_id: encoding, format: parsed };
        let mapped = |code: u32| char::from_u32(code).is_some() && parsed.glyph_index(code).is_some_and(|id| id.0 != 0 && usize::from(id.0) < glyphs);
        if matches!(format, 12 | 13) {
            let groups = u32at(sub, 12)? as usize;
            let mut previous = None;
            for group in 0..groups {
                let pos = 16 + 12 * group;
                let first = u32at(sub, pos)?; let last = u32at(sub, pos + 4)?;
                if first > last || last > 0x10ffff || previous.is_some_and(|p| first <= p) {
                    return Err(invalid("Invalid Unicode character-map coverage."));
                }
                previous = Some(last);
                usable |= mapped(first) || first < last && mapped(first + 1) || mapped(last);
            }
        } else if !usable {
            let (first, count) = match format {
                0 => (0, 256),
                2 | 4 => (0, 65536),
                6 => (u32::from(u16at(sub, 6)?), u32::from(u16at(sub, 8)?)),
                10 => (u32at(sub, 12)?, u32at(sub, 16)?),
                _ => unreachable!(),
            };
            let end = first.checked_add(count).filter(|end| *end <= 0x110000).ok_or_else(|| invalid("Invalid Unicode coverage range."))?;
            usable = (first..end).any(mapped);
        }
    }
    Ok(usable)
}

/// Bounded TrueType subset admission, not a complete sanitizer or proof of
/// game rendering compatibility. Parser success alone never admits a font.
pub fn validate_ttf(bytes: &[u8], filename: &str) -> Result<String, NativeError> {
    target_name(filename)?;
    if bytes.len() as u64 > MAX_FONT_BYTES || bytes.len() < 12 { return Err(invalid("Invalid font size.")); }
    if u32at(bytes, 0)? != 0x0001_0000 { return Err(invalid("Only single-face TrueType sfnt fonts are supported.")); }
    let count = usize::from(u16at(bytes, 4)?);
    if count == 0 || count > MAX_RECORDS { return Err(invalid("Invalid sfnt table count.")); }
    let directory_end = 12 + 16 * count;
    if directory_end > bytes.len() { return Err(invalid("Truncated sfnt directory.")); }
    let mut tables = BTreeMap::new(); let mut spans = Vec::new();
    for index in 0..count {
        let at = 12 + 16 * index;
        let tag: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
        let offset = usize::try_from(u32at(bytes, at + 8)?).map_err(|_| invalid("Invalid table offset."))?;
        let size = usize::try_from(u32at(bytes, at + 12)?).map_err(|_| invalid("Invalid table size."))?;
        let end = offset.checked_add(size).filter(|end| *end <= bytes.len()).ok_or_else(|| invalid("Table exceeds font bytes."))?;
        if offset % 4 != 0 || offset < directory_end || tables.insert(tag, &bytes[offset..end]).is_some() {
            return Err(invalid("Duplicate, unaligned, or directory-overlapping sfnt table."));
        }
        if size != 0 { spans.push((offset, end)); }
    }
    spans.sort_unstable();
    if spans.windows(2).any(|s| s[0].1 > s[1].0) { return Err(invalid("Overlapping sfnt tables.")); }
    for tag in [b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"glyf", b"loca"] {
        if !tables.contains_key(tag) { return Err(invalid("Missing required TrueType table.")); }
    }
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|_| invalid("Malformed TrueType face."))?;
    let parsed = face.tables();
    if parsed.glyf.is_none() || parsed.hmtx.is_none() || parsed.cmap.is_none() || parsed.os2.is_none() || parsed.post.is_none() {
        return Err(invalid("Unusable TrueType outline, metrics, cmap, OS/2, or post table."));
    }
    let head = tables[b"head"]; let hhea = tables[b"hhea"]; let maxp = tables[b"maxp"];
    if head.len() < 54 || u32at(head, 12)? != 0x5f0f_3cf5 || u32at(maxp, 0)? != 0x0001_0000 || maxp.len() < 32 || u32at(hhea, 0)? != 0x0001_0000 {
        return Err(invalid("Malformed TrueType header or maximum profile."));
    }
    let glyphs = usize::from(face.number_of_glyphs());
    let metrics = usize::from(u16at(hhea, 34)?);
    if glyphs == 0 || metrics == 0 || metrics > glyphs || tables[b"hmtx"].len() < 4 * metrics + 2 * (glyphs - metrics) {
        return Err(invalid("Incomplete horizontal metrics."));
    }
    let width = match u16at(head, 50)? { 0 => 2, 1 => 4, _ => return Err(invalid("Invalid loca format.")) };
    let loca = tables[b"loca"]; let glyf = tables[b"glyf"];
    if loca.len() < (glyphs + 1) * width { return Err(invalid("Incomplete glyph locations.")); }
    let mut previous = 0usize; let mut outlined = false;
    for index in 0..=glyphs {
        let offset = if width == 2 { usize::from(u16at(loca, index * 2)?) * 2 } else { u32at(loca, index * 4)? as usize };
        if offset < previous || offset > glyf.len() { return Err(invalid("Invalid glyph span.")); }
        if index != 0 && offset != previous {
            if offset - previous < 10 { return Err(invalid("Truncated glyph header.")); }
            // A usable simple outline suffices for admission. Avoid expanding
            // arbitrary composite graphs just to establish outline presence.
            if !outlined && (u16at(glyf, previous)? as i16) > 0 {
                outlined = face.glyph_bounding_box(ttf_parser::GlyphId((index - 1) as u16)).is_some();
            }
        }
        previous = offset;
    }
    let cmap = tables[b"cmap"];
    if !usable_cmap(cmap, glyphs)? || !outlined {
        return Err(invalid("No usable Unicode character map or TrueType outline."));
    }
    font_name(tables[b"name"], filename)
}

fn matches(meta: &fs::Metadata, fp: &Fingerprint) -> bool {
    meta.is_file() && (meta.len(), meta.dev(), meta.ino(), meta.mtime(), meta.mtime_nsec()) ==
        (fp.size, fp.device, fp.inode, fp.modified_seconds, fp.modified_nanos)
}
fn source_file(source: &FontSourceFile) -> Result<File, NativeError> {
    let file = paths::open_regular(source.path()).map_err(|_| stale())?;
    if !matches(&file.metadata().map_err(|_| stale())?, source.fingerprint()) || !source.matches_content_stamp(&file)? { return Err(stale()); }
    Ok(file)
}
fn verify_source(source: &FontSourceFile, _budget: &mut FontBudget, control: &PrepareControl) -> Result<(), NativeError> {
    control.check()?;
    if let Some(archive) = source.archive() { archive.lease().validate_sealed()?; }
    // Only the local-content producer constructs this handle, after hashing.
    // source_file also checks fsid, ctime and mode against that private proof.
    source_file(source)?;
    Ok(())
}
fn read_loose(source: &FontSourceFile, control: &PrepareControl) -> Result<Vec<u8>, NativeError> {
    control.check()?;
    if let Some(archive) = source.archive() { archive.lease().validate_sealed()?; }
    let mut file = source_file(source)?;
    let length = source.fingerprint().size;
    if length > MAX_FONT_BYTES { return Err(invalid("Font exceeds 16 MiB.")); }
    let mut bytes = Vec::new(); bytes.try_reserve_exact(length as usize).map_err(|_| invalid("Font allocation unavailable."))?;
    let mut buffer = [0u8; 64 * 1024];
    while bytes.len() < length as usize {
        control.check()?;
        let count = (length as usize - bytes.len()).min(buffer.len());
        let n = file.read(&mut buffer[..count]).map_err(|_| stale())?;
        if n == 0 { return Err(stale()); } bytes.extend_from_slice(&buffer[..n]);
    }
    if file.read(&mut buffer[..1]).map_err(|_| stale())? != 0 || !matches(&file.metadata()?, source.fingerprint()) { return Err(stale()); }
    source_file(source)?;
    Ok(bytes)
}
fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes { crc ^= u32::from(*byte); for _ in 0..8 { crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1)); } }
    !crc
}
fn seal(bytes: &[u8], sha256: String, sources: &[FontSourceFile], budget: &mut FontBudget, lease: &Arc<QuarantineLease>, control: &PrepareControl) -> Result<Arc<PreparedFont>, NativeError> {
    // All optional validation and admission precedes the first durable claim.
    let claim = lease.allocate(1, control)?.pop().ok_or_else(NativeError::params)?;
    let mut output = lease.open_claim(claim)?;
    for chunk in bytes.chunks(64 * 1024) { control.check()?; output.file.write_all(chunk)?; }
    output.file.sync_all()?;
    let meta = output.file.metadata()?;
    output.file.seek(SeekFrom::Start(0))?;
    let mut output_hash = Vec::new();
    archive_process::run_with_input("sha256sum", &[], &mut output_hash, 128,
        Some((&mut output.file, bytes.len() as u64)), &mut budget.process, control)?;
    if output_hash != format!("{sha256}  -\n").as_bytes() { return Err(stale()); }
    if meta.len() != bytes.len() as u64 { return Err(stale()); }
    let fingerprint = Fingerprint { sha256, size: meta.len(), device: meta.dev(), inode: meta.ino(), modified_seconds: meta.mtime(), modified_nanos: meta.mtime_nsec() };
    if !matches(&paths::open_regular(&output.path)?.metadata()?, &fingerprint) { return Err(stale()); }
    lease.complete_batch(&[output.name])?;
    Ok(Arc::new(PreparedFont { path: output.path, fingerprint, lease: lease.clone(), archive: None, sources: sources.to_vec() }))
}

struct Discovery { candidates: Vec<FontCandidate>, unavailable: usize, omitted: u64 }
impl Discovery {
    fn unavailable(&mut self, filename: &str, origin: FontOrigin, reason: &str, admitted: bool) -> Result<(), NativeError> {
        if !admitted {
            if self.unavailable >= MAX_UNAVAILABLE - 1 { self.omitted = self.omitted.saturating_add(1); return Ok(()); }
            self.unavailable += 1;
        }
        self.candidates.push(FontCandidate { id: paths::random_id()?, target_name: filename.to_owned(), display_name: filename.to_owned(),
            origins: vec![origin], prepared: None, unavailable_reason: Some(reason.to_owned()), name_collision: false });
        Ok(())
    }
    fn accept(&mut self, filename: &str, bytes: &[u8], origin: FontOrigin, reused: Option<&FontSourceFile>,
        known_sha: Option<String>, sources: &[FontSourceFile], budget: &mut FontBudget, lease: &Arc<QuarantineLease>, control: &PrepareControl) -> Result<(), NativeError> {
        control.check()?;
        let display_name = match validate_ttf(bytes, filename) {
            Ok(name) => name, Err(error) => return self.unavailable(filename, origin, &error.message, true),
        };
        let sha = match known_sha { Some(sha) => sha, None => prepare_control::digest_bounded_controlled(bytes, MAX_FONT_BYTES as usize, control)? };
        if let Some(source) = reused { if sha != source.fingerprint().sha256 { return Err(stale()); } }
        let name_key = folded(filename);
        if let Some(existing) = self.candidates.iter_mut().find(|c| folded(&c.target_name) == name_key && c.prepared.as_ref().is_some_and(|p| p.fingerprint.sha256 == sha && p.fingerprint.size == bytes.len() as u64)) {
            let retained = Arc::get_mut(existing.prepared.as_mut().unwrap()).ok_or_else(NativeError::params)?;
            for source in sources {
                if let Some(previous) = retained.sources.iter().find(|s| s.key() == source.key()) {
                    if previous.fingerprint() != source.fingerprint() || previous.path() != source.path() { return Err(stale()); }
                } else { retained.sources.push(source.clone()); }
            }
            if !existing.origins.contains(&origin) { existing.origins.push(origin); }
            return Ok(());
        }
        let prepared = if let Some(source) = reused.filter(|s| s.archive().is_some()) {
            let archive = source.archive().unwrap();
            Arc::new(PreparedFont { path: source.path().to_owned(), fingerprint: source.fingerprint().clone(), lease: archive.lease().clone(), archive: Some(archive.clone()), sources: sources.to_vec() })
        } else { seal(bytes, sha, sources, budget, lease, control)? };
        self.candidates.push(FontCandidate { id: paths::random_id()?, target_name: filename.to_owned(), display_name,
            origins: vec![origin], prepared: Some(prepared), unavailable_reason: None, name_collision: false });
        Ok(())
    }
}

pub(crate) fn discover_fonts(inputs: &[FontInput], lease: Arc<QuarantineLease>, font_budget: &mut FontBudget,
    control: &PrepareControl) -> Result<Vec<FontCandidate>, NativeError> {
    control.check()?; lease.validate_sealed()?;
    let mut result = Discovery { candidates: Vec::new(), unavailable: 0, omitted: 0 };
    let mut processed = BTreeSet::new();
    for input in inputs {
        control.check()?;
        match input.kind() {
            FontInputKind::Unavailable { origin, filename, reason } => {
                result.unavailable(filename, FontOrigin::Loose { container: origin.clone() }, reason, false)?;
            }
            FontInputKind::Loose(source) => {
                source_file(source)?; // metadata only, before size policy or any hashing
                let path = source.origin().relative_path();
                let filename = path.rsplit('/').next().unwrap_or(path);
                let origin = FontOrigin::Loose { container: source.origin().clone() };
                let key = source.key();
                if !bundled_ttf(path) { result.unavailable(filename, origin, "TTF is outside panorama/fonts.", false)?; continue; }
                let admission = font_budget.admit(&key, source.fingerprint().size)?;
                if let FontAdmission::Unavailable(reason) = admission { result.unavailable(filename, origin, reason, false)?; continue; }
                if !processed.insert(key) { continue; }
                let bytes = match read_loose(source, control) { Ok(bytes) => bytes,
                    Err(error) if error.kind == "fontUnavailable" => { result.unavailable(filename, origin, &error.message, true)?; continue; },
                    Err(error) => return Err(error),
                };
                // Hash even invalid admitted bytes so source replacement cannot
                // be downgraded into an optional validation exclusion.
                let sha = prepare_control::digest_bounded_controlled(&bytes, MAX_FONT_BYTES as usize, control)?;
                if sha != source.fingerprint().sha256 { return Err(stale()); }
                result.accept(filename, &bytes, origin, Some(source), Some(sha), std::slice::from_ref(source), font_budget, &lease, control)?;
                source_file(source)?;
            }
            FontInputKind::Vpk { candidate_id, directory, companions } => {
                discover_vpk(candidate_id, directory, companions, &lease, font_budget, control, &mut result, &mut processed)?;
            }
        }
    }
    if result.omitted != 0 {
        result.candidates.push(FontCandidate { id: paths::random_id()?, target_name: String::new(),
            display_name: "Additional font exclusions".to_owned(), origins: Vec::new(), prepared: None,
            unavailable_reason: Some(format!("{} additional unavailable font sources omitted from bounded metadata.", result.omitted)), name_collision: false });
    }
    // Equal spelling/fold with distinct bytes remains visible and unavailable
    // for selection; the sealed assets/provenance are retained, never replaced.
    mark_collisions(&mut result.candidates);
    control.check()?; lease.validate_sealed()?;
    Ok(result.candidates)
}
fn mark_collisions(candidates: &mut [FontCandidate]) {
    let mut names: BTreeMap<String, BTreeSet<(String, u64)>> = BTreeMap::new();
    for candidate in candidates.iter() { if let Some(font) = &candidate.prepared {
        names.entry(folded(&candidate.target_name)).or_default().insert((font.fingerprint.sha256.clone(), font.fingerprint.size));
    } }
    for candidate in candidates { if candidate.unavailable_reason.is_none() && names.get(&folded(&candidate.target_name)).is_some_and(|values| values.len() > 1) {
        candidate.name_collision = true;
        candidate.unavailable_reason = Some("Different font bytes share the same target filename; choose a nonconflicting source.".to_owned());
    } }
}

/// Pure review attachment; shared bytes and their original authority survive.
pub(crate) fn project(candidates: &[FontCandidate], selected: &BTreeSet<&str>) -> Vec<FontCandidate> {
    let mut result = Vec::new();
    for row in candidates {
        let mut row = row.clone();
        row.origins.retain(|o| match o {
            FontOrigin::Loose { .. } => true,
            FontOrigin::Vpk { candidate_id, .. } | FontOrigin::UnavailableVpk { candidate_id, .. } => selected.contains(candidate_id.as_str()),
        });
        if row.origins.is_empty() { continue; } // global aggregate remains discovery evidence only
        if row.name_collision { row.name_collision = false; row.unavailable_reason = None; }
        result.push(row);
    }
    mark_collisions(&mut result);
    result
}

fn discover_vpk(candidate_id: &str, directory: &FontSourceFile, companions: &BTreeMap<u16, FontSourceFile>,
    lease: &Arc<QuarantineLease>, budget: &mut FontBudget, control: &PrepareControl, result: &mut Discovery,
    processed: &mut BTreeSet<FontSourceKey>) -> Result<(), NativeError> {
    verify_source(directory, budget, control)?;
    for source in companions.values() { verify_source(source, budget, control)?; }
    let paths: BTreeMap<_, _> = companions.iter().map(|(index, source)| (*index, source.path().to_owned())).collect();
    let parsed = vpk::inspect_directory_controlled(directory.path(), control)
        .and_then(|parsed| { vpk::validate_family_controlled(&parsed, &paths, control)?; Ok(parsed) });
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            verify_source(directory, budget, control)?;
            for source in companions.values() { verify_source(source, budget, control)?; }
            if error.kind != "unsupportedVpk" { return Err(error); }
            return result.unavailable(directory.origin().relative_path(), FontOrigin::UnavailableVpk {
                container: directory.origin().clone(), candidate_id: candidate_id.to_owned(), members: member_origins(directory, companions),
            }, &error.message, false);
        }
    };
    for (index, resource) in parsed.resources.iter().enumerate() {
        control.check()?;
        if !resource.entry.path.to_ascii_lowercase().ends_with(".ttf") { continue; }
        // Retain the byte-contributing family members for this font, not every
        // unrelated companion in the package. Complete family authority is
        // nevertheless checked before and after discovery.
        let mut contributing = BTreeMap::new();
        if resource.location.length != 0 && resource.location.archive_index != 0x7fff {
            let source = companions.get(&resource.location.archive_index).ok_or_else(stale)?;
            contributing.insert(resource.location.archive_index, source.clone());
        }
        let sources: Vec<_> = std::iter::once(directory).chain(contributing.values()).cloned().collect();
        let filename = resource.original_path.rsplit('/').next().unwrap_or(&resource.original_path);
        let origin = FontOrigin::Vpk { container: directory.origin().clone(), candidate_id: candidate_id.to_owned(),
            resource_path: resource.original_path.clone(), resource_index: index, archive_index: resource.location.archive_index,
            resource_crc: resource.entry.crc, members: member_origins(directory, &contributing) };
        if !bundled_ttf(&resource.entry.path) { result.unavailable(filename, origin, "TTF is outside panorama/fonts.", false)?; continue; }
        let size = resource.location.length.checked_add(resource.location.preload.len() as u64)
            .ok_or_else(|| invalid("Font resource size overflow."))?;
        let key = FontSourceKey::vpk(candidate_id, index);
        let admission = budget.admit(&key, size)?;
        if let FontAdmission::Unavailable(reason) = admission { result.unavailable(filename, origin, reason, false)?; continue; }
        if !processed.insert(key) { continue; }
        source_file(directory)?;
        for source in companions.values() { source_file(source)?; }
        let bytes = vpk::read_resource(directory.path(), &parsed, &paths, index, MAX_FONT_BYTES as usize);
        // Authority loss wins over format/CRC exclusions.
        source_file(directory)?;
        for source in companions.values() { source_file(source)?; }
        control.check()?;
        let bytes = match bytes { Ok(bytes) => bytes, Err(error) if error.kind == "unsupportedVpk" => {
            result.unavailable(filename, origin, &error.message, true)?; continue;
        }, Err(error) => return Err(error) };
        if crc32_ieee(&bytes) != resource.entry.crc { result.unavailable(filename, origin, "VPK font CRC mismatch.", true)?; continue; }
        result.accept(filename, &bytes, origin, None, None, &sources, budget, lease, control)?;
    }
    verify_source(directory, budget, control)?;
    for source in companions.values() { verify_source(source, budget, control)?; }
    Ok(())
}
fn member_origins(directory: &FontSourceFile, companions: &BTreeMap<u16, FontSourceFile>) -> Vec<VpkFontMemberOrigin> {
    std::iter::once((None, directory)).chain(companions.iter().map(|(index, source)| (Some(*index), source)))
        .map(|(archive_index, source)| VpkFontMemberOrigin { archive_index, container: source.origin().clone(),
            sha256: source.fingerprint().sha256.clone(), size: source.fingerprint().size }).collect()
}
