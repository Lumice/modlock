//! Private, versioned local choices. Descriptions never grant filesystem authority.
use crate::{
    native::NativeError,
    package::{self, FileRef, PackageRef},
    paths, protocol, store,
};
use json::{object, JsonValue};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub(crate) const MAX_OPTIONS: usize = 1024;
const MAX_MEMBERS: usize = 10_000;
const MAX_MEMBER_BYTES: u64 = 17_179_869_184;
const MAX_PACKAGE_BYTES: u64 = 137_438_953_472;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpectedFile {
    pub source_name: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpectedFamily {
    pub directory: ExpectedFile,
    pub companions: BTreeMap<u16, ExpectedFile>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveMemberBinding {
    pub archive_index: Option<u16>,
    pub member_index: u32,
    pub normalized_path: String,
    pub decoder_path: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveBinding {
    pub file: FileRef,
    pub members: Vec<ArchiveMemberBinding>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetainedArchive {
    pub file: FileRef,
    pub attachment_id: String,
    pub root_ordinal: u32,
    pub relative_path: String,
    pub format: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attachment {
    pub id: String,
    pub source_kind: String,
    pub source_label: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OptionEntry {
    pub id: String,
    pub option_name: String,
    pub package: Option<PackageRef>,
    pub expected_family: Option<ExpectedFamily>,
    pub archive_binding: Option<ArchiveBinding>,
    pub source_label: String,
    pub unavailable_reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OptionCatalog {
    pub entries: Vec<OptionEntry>,
    pub selected_ids: Vec<String>,
    pub retained_archives: Vec<RetainedArchive>,
    pub attachments: Vec<Attachment>,
}
pub(crate) fn invalid() -> NativeError {
    NativeError::new(
        "invalidOptions",
        "Local choice metadata is incomplete, ambiguous, or exceeds its supported bounds.",
    )
}
fn keys(v: &JsonValue, names: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len() != names.len() || names.iter().any(|k| !v.has_key(k)) {
        return Err(invalid());
    }
    Ok(())
}
fn text(v: &JsonValue, key: &str, max: usize, empty: bool) -> Result<String, NativeError> {
    v[key]
        .as_str()
        .filter(|s| (empty || !s.is_empty()) && s.len() <= max && !s.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(invalid)
}
fn id(v: &JsonValue, key: &str) -> Result<String, NativeError> {
    v[key]
        .as_str()
        .filter(|s| package::hex(s, 32))
        .map(str::to_owned)
        .ok_or_else(invalid)
}
pub(crate) fn relative(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 4096
        && !s.contains(['\\', '\0'])
        && !s.chars().any(char::is_control)
        && s.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
        && !s.split('/').next().is_some_and(|c| c.contains(':'))
}
fn archive_coordinate(s: &str) -> bool {
    relative(s)
        && !s.contains([':', '\u{fffd}'])
        && s.split('/').count() <= 32
        && !s.split('/').any(|c| c == "(nil)")
        && !s
            .as_bytes()
            .windows(3)
            .any(|b| b[0] == b'%' && b[1].is_ascii_hexdigit() && b[2].is_ascii_hexdigit())
}
fn archive_fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}
fn file(v: &JsonValue) -> Result<FileRef, NativeError> {
    keys(v, &["id", "sha256", "size", "sourceName"])?;
    package::decode_file(v)
}
fn expected_file(v: &JsonValue) -> Result<ExpectedFile, NativeError> {
    keys(v, &["sourceName", "sha256", "size"])?;
    let source_name = text(v, "sourceName", 255, false)?;
    if !paths::safe_name(&source_name) || !source_name.to_ascii_lowercase().ends_with(".vpk") {
        return Err(invalid());
    }
    Ok(ExpectedFile {
        source_name,
        sha256: v["sha256"]
            .as_str()
            .filter(|s| package::hex(s, 64))
            .ok_or_else(invalid)?
            .into(),
        size: v["size"]
            .as_u64()
            .filter(|n| *n <= MAX_MEMBER_BYTES)
            .ok_or_else(invalid)?,
    })
}
fn expected_file_json(f: &ExpectedFile) -> JsonValue {
    object! {sourceName:f.source_name.clone(),sha256:f.sha256.clone(),size:f.size}
}
fn index(v: &JsonValue) -> Result<u16, NativeError> {
    v.as_u16().filter(|i| *i != 0x7fff).ok_or_else(invalid)
}
impl ExpectedFamily {
    pub(crate) fn from_package(p: &PackageRef) -> Self {
        let convert = |f: &FileRef| ExpectedFile {
            source_name: f.source_name.clone(),
            sha256: f.sha256.clone(),
            size: f.size,
        };
        Self {
            directory: convert(&p.directory),
            companions: p.companions.iter().map(|(i, f)| (*i, convert(f))).collect(),
        }
    }
    pub(crate) fn decode(v: &JsonValue) -> Result<Self, NativeError> {
        keys(v, &["directory", "companions"])?;
        if !v["companions"].is_array() || v["companions"].len() >= MAX_MEMBERS {
            return Err(invalid());
        }
        let mut companions = BTreeMap::new();
        let mut previous = None;
        for c in v["companions"].members() {
            keys(c, &["archiveIndex", "file"])?;
            let i = index(&c["archiveIndex"])?;
            if previous.is_some_and(|p| p >= i) {
                return Err(invalid());
            }
            previous = Some(i);
            companions.insert(i, expected_file(&c["file"])?);
        }
        let result = Self {
            directory: expected_file(&v["directory"])?,
            companions,
        };
        let mut names = HashSet::new();
        for f in std::iter::once(&result.directory).chain(result.companions.values()) {
            if !names.insert(f.source_name.to_ascii_lowercase()) {
                return Err(invalid());
            }
        }
        if !result.companions.is_empty() {
            let name = &result.directory.source_name;
            let stem = &name[..name.len() - 4];
            let stem = stem.strip_suffix("_dir").unwrap_or(stem);
            if result
                .companions
                .iter()
                .any(|(i, f)| f.source_name != format!("{stem}_{i:03}.vpk"))
            {
                return Err(invalid());
            }
        }
        Ok(result)
    }
    pub(crate) fn encode(&self) -> JsonValue {
        object! {directory:expected_file_json(&self.directory),companions:JsonValue::from(self.companions.iter().map(|(i,f)|object!{archiveIndex:*i,file:expected_file_json(f)}).collect::<Vec<_>>())}
    }
}
fn decode_package(v: &JsonValue, layout: u32) -> Result<PackageRef, NativeError> {
    keys(v, &["directory", "companions"])?;
    file(&v["directory"])?;
    if !v["companions"].is_array() || v["companions"].len() >= MAX_MEMBERS {
        return Err(invalid());
    }
    for c in v["companions"].members() {
        keys(c, &["archiveIndex", "file"])?;
        file(&c["file"])?;
    }
    let mut packages = package::legacy_packages(&object! {packages:json::array![v.clone()]}, 2)?;
    let p = packages.pop().ok_or_else(invalid)?;
    // Layout1 has no external archive companions, even inside a catalog.
    if layout == 1 && !p.companions.is_empty() {
        return Err(invalid());
    }
    Ok(p)
}
impl ArchiveBinding {
    pub(crate) fn decode(v: &JsonValue, expected: &ExpectedFamily) -> Result<Self, NativeError> {
        keys(v, &["file", "members"])?;
        let archive = file(&v["file"])?;
        if archive.size > 68_719_476_736
            || !v["members"].is_array()
            || v["members"].len() != expected.companions.len() + 1
        {
            return Err(invalid());
        }
        let mut members = Vec::new();
        let mut physical = HashSet::new();
        let mut normalized = HashSet::new();
        let mut decoder = HashSet::new();
        for (m, (i, f)) in v["members"].members().zip(
            std::iter::once((None, &expected.directory))
                .chain(expected.companions.iter().map(|(i, f)| (Some(*i), f))),
        ) {
            keys(
                m,
                &[
                    "archiveIndex",
                    "memberIndex",
                    "normalizedPath",
                    "decoderPath",
                ],
            )?;
            let archive_index = if m["archiveIndex"].is_null() {
                None
            } else {
                Some(index(&m["archiveIndex"])?)
            };
            let member_index = m["memberIndex"]
                .as_u32()
                .filter(|n| *n < MAX_MEMBERS as u32)
                .ok_or_else(invalid)?;
            let normalized_path = text(m, "normalizedPath", 4096, false)?;
            let decoder_path = text(m, "decoderPath", 4096, false)?;
            if archive_index != i
                || !archive_coordinate(&normalized_path)
                || !archive_coordinate(&decoder_path.replace('\\', "/"))
                || decoder_path.replace('\\', "/") != normalized_path
                || normalized_path.rsplit('/').next() != Some(f.source_name.as_str())
                || !physical.insert(member_index)
                || !normalized.insert(archive_fold(&normalized_path))
                || !decoder.insert(decoder_path.clone())
            {
                return Err(invalid());
            }
            members.push(ArchiveMemberBinding {
                archive_index,
                member_index,
                normalized_path,
                decoder_path,
            });
        }
        // A family has one physical parent, not merely matching member basenames.
        let parent = |s: &str| s.rsplit_once('/').map(|(p, _)| p.to_owned());
        if members
            .iter()
            .any(|m| parent(&m.normalized_path) != parent(&members[0].normalized_path))
        {
            return Err(invalid());
        }
        Ok(Self {
            file: archive,
            members,
        })
    }
    pub(crate) fn encode(&self) -> JsonValue {
        object! {file:package::encode_file(&self.file),members:JsonValue::from(self.members.iter().map(|m|object!{archiveIndex:m.archive_index,memberIndex:m.member_index,normalizedPath:m.normalized_path.clone(),decoderPath:m.decoder_path.clone()}).collect::<Vec<_>>())}
    }
}
fn envelope(v: &JsonValue) -> Result<(), NativeError> {
    if v["version"].as_u32() != Some(1) {
        return Err(NativeError::new(
            "unsupportedVersion",
            "Unsupported local option catalog version.",
        ));
    }
    keys(
        v,
        &[
            "version",
            "entries",
            "selectedIds",
            "retainedArchives",
            "attachments",
        ],
    )?;
    for (key, max) in [
        ("entries", MAX_OPTIONS),
        ("selectedIds", 99),
        ("retainedArchives", MAX_OPTIONS),
        ("attachments", MAX_OPTIONS),
    ] {
        if !v[key].is_array() || v[key].len() > max {
            return Err(invalid());
        }
    }
    if protocol::native_value_bytes(v).is_none() {
        return Err(invalid());
    }
    Ok(())
}
fn archives(v: &JsonValue) -> Result<(Vec<RetainedArchive>, Vec<Attachment>), NativeError> {
    envelope(v)?;
    let mut attachments = Vec::new();
    let mut attachment_ids = HashSet::new();
    for a in v["attachments"].members() {
        keys(a, &["id", "sourceKind", "sourceLabel"])?;
        let id = id(a, "id")?;
        let source_kind = text(a, "sourceKind", 32, false)?;
        if !attachment_ids.insert(id.clone())
            || !matches!(source_kind.as_str(), "chosenRoots" | "retainedArchive")
        {
            return Err(invalid());
        }
        attachments.push(Attachment {
            id,
            source_kind,
            source_label: text(a, "sourceLabel", 4096, false)?,
        });
    }
    let mut retained = Vec::new();
    let mut ids = HashSet::new();
    let mut coordinates = HashSet::new();
    let mut total = 0u64;
    for a in v["retainedArchives"].members() {
        keys(
            a,
            &[
                "file",
                "attachmentId",
                "rootOrdinal",
                "relativePath",
                "format",
            ],
        )?;
        let file = file(&a["file"])?;
        let attachment_id = id(a, "attachmentId")?;
        let root_ordinal = a["rootOrdinal"]
            .as_u32()
            .filter(|i| *i < 64)
            .ok_or_else(invalid)?;
        let relative_path = text(a, "relativePath", 4096, false)?;
        let format = text(a, "format", 16, false)?;
        total = total.checked_add(file.size).ok_or_else(invalid)?;
        if total > 68_719_476_736
            || !attachment_ids.contains(&attachment_id)
            || !ids.insert(file.id.clone())
            || !relative(&relative_path)
            || relative_path.rsplit('/').next() != Some(file.source_name.as_str())
            || !coordinates.insert((attachment_id.clone(), root_ordinal, relative_path.clone()))
            || !matches!(format.as_str(), "Zip" | "7-Zip" | "RAR" | "RAR 5")
        {
            return Err(invalid());
        }
        retained.push(RetainedArchive {
            file,
            attachment_id,
            root_ordinal,
            relative_path,
            format,
        });
    }
    Ok((retained, attachments))
}
impl OptionCatalog {
    /// Structural codec; `for_record` additionally proves historical archive ownership.
    pub(crate) fn decode(v: &JsonValue, layout: u32) -> Result<Self, NativeError> {
        if !matches!(layout, 1 | 2) {
            return Err(invalid());
        }
        let (retained_archives, attachments) = archives(v)?;
        let mut entries = Vec::new();
        let mut ids = HashSet::new();
        let mut payloads: HashMap<String, FileRef> = HashMap::new();
        for e in v["entries"].members() {
            keys(
                e,
                &[
                    "id",
                    "optionName",
                    "package",
                    "expectedFamily",
                    "archiveBinding",
                    "sourceLabel",
                    "unavailableReason",
                ],
            )?;
            let id = id(e, "id")?;
            if !ids.insert(id.clone()) {
                return Err(invalid());
            }
            let expected_family = if e["expectedFamily"].is_null() {
                None
            } else {
                Some(ExpectedFamily::decode(&e["expectedFamily"])?)
            };
            let package = if e["package"].is_null() {
                None
            } else {
                Some(decode_package(&e["package"], layout)?)
            };
            let archive_binding = if e["archiveBinding"].is_null() {
                None
            } else {
                Some(ArchiveBinding::decode(
                    &e["archiveBinding"],
                    expected_family.as_ref().ok_or_else(invalid)?,
                )?)
            };
            let unavailable_reason = if e["unavailableReason"].is_null() {
                None
            } else {
                Some(text(e, "unavailableReason", 4096, false)?)
            };
            if package.is_some() != unavailable_reason.is_none() {
                return Err(invalid());
            }
            if let Some(p) = &package {
                if expected_family.as_ref() != Some(&ExpectedFamily::from_package(p)) {
                    return Err(invalid());
                }
                for f in std::iter::once(&p.directory).chain(p.companions.values()) {
                    if payloads
                        .insert(f.id.clone(), f.clone())
                        .is_some_and(|old| old != *f)
                    {
                        return Err(invalid());
                    }
                }
            }
            entries.push(OptionEntry {
                id,
                option_name: text(e, "optionName", 1024, false)?,
                package,
                expected_family,
                archive_binding,
                source_label: text(e, "sourceLabel", 4096, true)?,
                unavailable_reason,
            });
        }
        let mut selected_ids = Vec::new();
        for s in v["selectedIds"].members() {
            let s = s
                .as_str()
                .filter(|s| package::hex(s, 32))
                .ok_or_else(invalid)?
                .to_owned();
            if selected_ids.last().is_some_and(|p| p >= &s)
                || !entries.iter().any(|e| e.id == s && e.package.is_some())
            {
                return Err(invalid());
            }
            selected_ids.push(s);
        }
        for a in &retained_archives {
            if payloads.contains_key(&a.file.id) {
                return Err(invalid());
            }
        }
        let result = Self {
            entries,
            selected_ids,
            retained_archives,
            attachments,
        };
        let logical = result
            .packages(false)?
            .iter()
            .flat_map(|p| std::iter::once(&p.directory).chain(p.companions.values()))
            .try_fold(0u64, |n, f| n.checked_add(f.size).ok_or_else(invalid))?;
        if logical > MAX_PACKAGE_BYTES {
            return Err(invalid());
        }
        result.validate_bindings()?;
        Ok(result)
    }
    fn validate_bindings(&self) -> Result<(), NativeError> {
        let mut files = HashMap::new();
        let mut physical = HashMap::new();
        let mut coordinates = HashMap::new();
        for e in &self.entries {
            if let Some(binding) = &e.archive_binding {
                if files
                    .insert(&binding.file.id, &binding.file)
                    .is_some_and(|old| old != &binding.file)
                {
                    return Err(invalid());
                }
                let expected = e.expected_family.as_ref().ok_or_else(invalid)?;
                for (m, f) in binding
                    .members
                    .iter()
                    .zip(std::iter::once(&expected.directory).chain(expected.companions.values()))
                {
                    let value = (m, f);
                    if physical
                        .insert((&binding.file.id, m.member_index), value)
                        .is_some_and(|old| old != value)
                        || coordinates
                            .insert(
                                (&binding.file.id, archive_fold(&m.normalized_path)),
                                m.member_index,
                            )
                            .is_some_and(|old| old != m.member_index)
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn encode(&self, layout: u32) -> Result<JsonValue, NativeError> {
        let v = object! {version:1,entries:JsonValue::from(self.entries.iter().map(|e|object!{id:e.id.clone(),optionName:e.option_name.clone(),package:e.package.as_ref().map(|p|package::encode_packages(std::slice::from_ref(p))[0].clone()),expectedFamily:e.expected_family.as_ref().map(ExpectedFamily::encode),archiveBinding:e.archive_binding.as_ref().map(ArchiveBinding::encode),sourceLabel:e.source_label.clone(),unavailableReason:e.unavailable_reason.clone()}).collect::<Vec<_>>()),selectedIds:JsonValue::from(self.selected_ids.clone()),retainedArchives:JsonValue::from(self.retained_archives.iter().map(|a|object!{file:package::encode_file(&a.file),attachmentId:a.attachment_id.clone(),rootOrdinal:a.root_ordinal,relativePath:a.relative_path.clone(),format:a.format.clone()}).collect::<Vec<_>>()),attachments:JsonValue::from(self.attachments.iter().map(|a|object!{id:a.id.clone(),sourceKind:a.source_kind.clone(),sourceLabel:a.source_label.clone()}).collect::<Vec<_>>())};
        Self::decode(&v, layout)?;
        Ok(v)
    }
    /// Metadata Save uses this stricter check; decoding preserves literal empty history.
    pub(crate) fn validate_selection_save(&self) -> Result<(), NativeError> {
        if self.selected_ids.is_empty()
            || self.selected_ids.len() > 99
            || self.selected_ids.windows(2).any(|w| w[0] >= w[1])
            || self.selected_ids.iter().any(|id| {
                !self
                    .entries
                    .iter()
                    .any(|e| e.id == *id && e.package.is_some())
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn packages(&self, selected: bool) -> Result<Vec<PackageRef>, NativeError> {
        let mut rows: Vec<_> = self
            .entries
            .iter()
            .filter(|e| !selected || self.selected_ids.binary_search(&e.id).is_ok())
            .collect();
        rows.sort_by(|a, b| (&a.option_name, &a.id).cmp(&(&b.option_name, &b.id)));
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for e in rows {
            if let Some(p) = &e.package {
                // Selected aliases coalesce by content; retention keeps distinct native refs.
                let key = if selected {
                    package::family_key(p)?
                } else {
                    store::canonical(&package::encode_packages(std::slice::from_ref(p)))
                };
                if seen.insert(key) {
                    result.push(p.clone());
                }
            }
        }
        Ok(result)
    }
}
pub(crate) fn owned_archive_refs(record: &JsonValue) -> Result<Vec<FileRef>, NativeError> {
    if !record.has_key("options") {
        return Ok(Vec::new());
    }
    Ok(archives(&record["options"])?
        .0
        .into_iter()
        .map(|a| a.file)
        .collect())
}
pub(crate) fn for_record(record: &JsonValue, layout: u32) -> Result<OptionCatalog, NativeError> {
    if record.has_key("payloads")
        || record.has_key("packages")
        || !record["source"]["id"].as_str().is_some_and(paths::safe_id)
    {
        return Err(invalid());
    }
    let catalog = OptionCatalog::decode(&record["options"], layout)?;
    let archives = crate::local_import::stored_archive_refs(record)?;
    let mut roles = HashMap::new();
    for a in &archives {
        roles.insert(a.id.clone(), (a.clone(), "archive"));
    }
    for e in &catalog.entries {
        if let Some(b) = &e.archive_binding {
            if !archives.contains(&b.file) {
                return Err(invalid());
            }
        }
        if let Some(p) = &e.package {
            for f in std::iter::once(&p.directory).chain(p.companions.values()) {
                let value = (f.clone(), "payload");
                if roles
                    .insert(f.id.clone(), value.clone())
                    .is_some_and(|old| old != value)
                {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(catalog)
}
pub(crate) fn selected_packages(
    record: &JsonValue,
    layout: u32,
) -> Result<Vec<PackageRef>, NativeError> {
    for_record(record, layout)?.packages(true)
}
pub(crate) fn retained_packages(
    record: &JsonValue,
    layout: u32,
) -> Result<Vec<PackageRef>, NativeError> {
    if record.has_key("options") {
        for_record(record, layout)?.packages(false)
    } else {
        package::legacy_packages(record, layout)
    }
}
/// Exact lazy detail contract, shared by future get/preview/publication admission.
pub(crate) fn detail(record: &JsonValue, layout: u32) -> Result<JsonValue, NativeError> {
    let legacy = !record.has_key("options");
    let catalog = crate::local_content::option_sources::catalog_for_record(record, layout)?;
    let rows = catalog.encode(layout)?;
    let selected = catalog.packages(true)?;
    let retained = catalog.packages(false)?;
    let logical = |packages: &[PackageRef]| -> Result<u64, NativeError> {
        packages
            .iter()
            .flat_map(|p| std::iter::once(&p.directory).chain(p.companions.values()))
            .try_fold(0u64, |n, f| n.checked_add(f.size).ok_or_else(invalid))
    };
    let distinct = |packages: &[PackageRef]| -> Result<usize, NativeError> {
        Ok(packages
            .iter()
            .map(package::family_key)
            .collect::<Result<BTreeSet<_>, _>>()?
            .len())
    };
    let mut unique = HashMap::new();
    for (f, role) in crate::local_import::record_files(record, layout)? {
        let value = (f.clone(), role);
        if unique
            .insert(f.id.clone(), value.clone())
            .is_some_and(|old| old != value)
        {
            return Err(invalid());
        }
    }
    let role_bytes = |role: &str| -> Result<u64, NativeError> {
        unique
            .values()
            .filter(|(_, r)| *r == role)
            .try_fold(0u64, |n, (f, _)| n.checked_add(f.size).ok_or_else(invalid))
    };
    let copy = unique
        .values()
        .try_fold(0u64, |n, (f, _)| n.checked_add(f.size).ok_or_else(invalid))?;
    let availability = JsonValue::from(catalog.entries.iter().map(|e| object!{
        optionId:e.id.clone(),state:if e.package.is_some() {"ready"} else if e.archive_binding.is_some() {"needsPreparation"} else if e.expected_family.is_some() {"needsSource"} else {"unavailable"}
    }).collect::<Vec<_>>());
    let result = object! {version:1,modId:record["source"]["id"].clone(),layout:layout,legacy:legacy,catalog:rows,availability:availability,totals:object!{optionCount:catalog.entries.len(),selectedCount:catalog.selected_ids.len(),activeFamilies:distinct(&selected)?,retainedFamilies:distinct(&retained)?,unavailableCount:catalog.entries.iter().filter(|e|e.package.is_none()).count(),selectedLogicalBytes:logical(&selected)?,retainedLogicalBytes:logical(&retained)?,uniquePayloadBytes:role_bytes("payload")?,retainedArchiveBytes:role_bytes("archive")?,artworkBytes:role_bytes("artwork")?,fontBytes:role_bytes("font")?,uniqueCopyBytes:copy}};
    if protocol::native_value_bytes(&result).is_none() {
        return Err(NativeError::new(
            "nativeResponseLimit",
            "Local option detail exceeds the readable native response budget.",
        ));
    }
    Ok(result)
}
