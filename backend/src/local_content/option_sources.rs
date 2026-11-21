//! Owner-local, read-only legacy evidence projection. No historical path is opened.
use crate::{
    native::NativeError,
    options::{self, ArchiveBinding, ExpectedFamily, OptionCatalog, OptionEntry},
    package, paths, store,
};
use json::{object, JsonValue};
use std::collections::{BTreeMap, HashSet};

fn label(v: &JsonValue, fallback: &str, limit: usize) -> String {
    let source = v.as_str().filter(|s| !s.is_empty()).unwrap_or(fallback);
    let mut result = String::new();
    for c in source.chars().filter(|c| !c.is_control()) {
        if result.len() + c.len_utf8() > limit {
            break;
        }
        result.push(c);
    }
    if result.is_empty() {
        fallback.into()
    } else {
        result
    }
}
/// IDs belong to the stable mod and occurrence, never a profile, revision or path lookup.
/// The domain-separated canonical digest is truncated to 128 bits; callers reject collisions.
pub(crate) fn legacy_id(mod_id: &str, identity: &JsonValue) -> Result<String, NativeError> {
    if !paths::safe_id(mod_id) {
        return Err(options::invalid());
    }
    Ok(paths::digest_bytes(
        store::canonical(&json::array![
            "native.localOptions.legacy.v1",
            mod_id,
            identity.clone()
        ])
        .as_bytes(),
    )?[..32]
        .into())
}
fn expected(row: &JsonValue) -> Option<ExpectedFamily> {
    if !row["members"].is_array() || row["members"].is_empty() || row["members"].len() > 10_000 {
        return None;
    }
    let mut directory = None;
    let mut companions = BTreeMap::new();
    for member in row["members"].members() {
        let file = object! {sourceName:member["sourceName"].clone(),sha256:member["sha256"].clone(),size:member["size"].clone()};
        if !member.has_key("archiveIndex") {
            return None;
        }
        if member["archiveIndex"].is_null() {
            if directory.replace(file).is_some() {
                return None;
            }
        } else {
            let index = member["archiveIndex"].as_u16()?;
            if index == 0x7fff || companions.insert(index, file).is_some() {
                return None;
            }
        }
    }
    if row["companionCount"].as_usize() != Some(companions.len()) {
        return None;
    }
    let family = ExpectedFamily::decode(&object!{directory:directory?,companions:JsonValue::from(companions.into_iter().map(|(i,f)|object!{archiveIndex:i,file:f}).collect::<Vec<_>>())}).ok()?;
    if row["relativePath"].as_str()?.rsplit('/').next()
        != Some(family.directory.source_name.as_str())
    {
        return None;
    }
    Some(family)
}
fn archive_binding(
    record: &JsonValue,
    row: &JsonValue,
    expected: &ExpectedFamily,
) -> Option<ArchiveBinding> {
    // Only this exact owner's historical retained archive coordinate grants a binding.
    let refs = crate::local_import::stored_archive_refs(record).ok()?;
    let owner = record["source"]["localImport"]["archives"]
        .members()
        .find(|a| {
            a["rootOrdinal"] == row["rootOrdinal"]
                && a["relativePath"] == row["archiveRelativePath"]
        })?;
    let file = package::decode_file(&owner["file"]).ok()?;
    if !refs.contains(&file) {
        return None;
    }
    let mut members = BTreeMap::new();
    for member in row["members"].members() {
        let a = &member["archive"];
        if a["id"] != file.id
            || a["sha256"] != file.sha256
            || a["format"] != owner["format"]
            || member["archiveRelativePath"] != owner["relativePath"]
            || a["normalizedPath"] != a["memberPath"]
        {
            return None;
        }
        let key = if member["archiveIndex"].is_null() {
            if a["normalizedPath"] != row["relativePath"] {
                return None;
            }
            None
        } else {
            Some(member["archiveIndex"].as_u16()?)
        };
        if members.insert(key,object!{archiveIndex:member["archiveIndex"].clone(),memberIndex:a["index"].clone(),normalizedPath:a["normalizedPath"].clone(),decoderPath:a["decoderPath"].clone()}).is_some() {return None;}
    }
    ArchiveBinding::decode(&object!{file:package::encode_file(&file),members:JsonValue::from(members.into_values().collect::<Vec<_>>())},expected).ok()
}
/// Read the exact owner's current catalog, projecting only an absent envelope.
/// A present malformed catalog must never acquire legacy fallback authority.
pub(crate) fn catalog_for_record(record: &JsonValue, layout: u32) -> Result<OptionCatalog, NativeError> {
    if record.has_key("options") {
        options::for_record(record, layout)
    } else {
        project_legacy(record, layout)
    }
}
pub(crate) fn project_legacy(
    record: &JsonValue,
    layout: u32,
) -> Result<OptionCatalog, NativeError> {
    if record.has_key("options") {
        return Err(options::invalid());
    }
    let mod_id = record["source"]["id"]
        .as_str()
        .ok_or_else(options::invalid)?;
    let packages = package::legacy_packages(record, layout)?;
    let local = &record["source"]["localImport"];
    let historical = &local["selectedCandidateIds"];
    let local_history =
        record["source"]["kind"] == "local" && matches!(local["version"].as_u32(), Some(1 | 2 | 3));
    let literal_empty = local_history && historical.is_array() && historical.is_empty();
    let mut catalog = OptionCatalog {
        entries: Vec::new(),
        selected_ids: Vec::new(),
        retained_archives: Vec::new(),
        attachments: Vec::new(),
    };
    let mut used_ids = HashSet::new();
    let mut represented = HashSet::new();
    let mut selected_families = HashSet::new();
    let local_rows = local_history && local["candidates"].is_array();
    if local_rows {
        if local["candidates"].len() > options::MAX_OPTIONS {
            return Err(options::invalid());
        }
        let mut candidate_ids = BTreeMap::new();
        for row in local["candidates"].members() {
            if let Some(id) = row["id"].as_str() {
                *candidate_ids.entry(id).or_insert(0usize) += 1;
            }
        }
        for (ordinal, row) in local["candidates"].members().enumerate() {
            // Include the complete descriptive occurrence, not just its label/hash.
            let id = legacy_id(
                mod_id,
                &object! {kind:"candidate",ordinal:ordinal,row:row.clone()},
            )?;
            if !used_ids.insert(id.clone()) {
                return Err(options::invalid());
            }
            let candidate_id = row["id"].as_str();
            let coordinates = if row["archiveRelativePath"].is_null() {
                json::array![row["rootOrdinal"].clone(), row["relativePath"].clone()]
            } else {
                json::array![
                    row["rootOrdinal"].clone(),
                    row["archiveRelativePath"].clone(),
                    row["relativePath"].clone()
                ]
            };
            let identity_complete = candidate_id
                .is_some_and(|s| package::hex(s, 64) && candidate_ids.get(s) == Some(&1))
                && row["rootOrdinal"].as_u32().is_some_and(|i| i < 64)
                && row["relativePath"].as_str().is_some_and(options::relative)
                && (row["archiveRelativePath"].is_null()
                    || row["archiveRelativePath"]
                        .as_str()
                        .is_some_and(options::relative))
                && row["id"] == paths::digest_bytes(store::canonical(&coordinates).as_bytes())?;
            let historically_valid = row["available"] == true && row["unavailableReason"].is_null();
            let expected_family = if identity_complete && historically_valid {
                expected(row)
            } else {
                None
            };
            let binding = expected_family
                .as_ref()
                .and_then(|e| archive_binding(record, row, e));
            let mut matches = Vec::new();
            if historically_valid {
                if let Some(e) = &expected_family {
                    for p in &packages {
                        if ExpectedFamily::from_package(p) == *e && !matches.contains(p) {
                            matches.push(p.clone());
                        }
                    }
                }
            }
            let package = if matches.len() == 1 {
                matches.pop()
            } else {
                None
            };
            if let Some(p) = &package {
                represented.insert(store::canonical(&package::encode_packages(
                    std::slice::from_ref(p),
                )));
                if !literal_empty && historical.members().any(|s| s.as_str() == candidate_id) {
                    catalog.selected_ids.push(id.clone());
                    selected_families.insert(package::family_key(p)?);
                }
            }
            let reason = if package.is_some() {
                None
            } else if matches.len() > 1 {
                Some("Historical family matches multiple distinct native bindings; its occurrence is ambiguous.".into())
            } else if !row["unavailableReason"].is_null() {
                Some(label(
                    &row["unavailableReason"],
                    "Historical candidate is unavailable.",
                    4096,
                ))
            } else if expected_family.is_none() {
                Some("Historical occurrence or complete family names, indices, sizes and hashes are missing or ambiguous; use a fresh import.".into())
            } else if binding.is_some() {
                Some(
                    "Family bytes were not retained; its exact owned archive requires preparation."
                        .into(),
                )
            } else {
                Some("Family bytes were not retained; choose a source proving every expected member.".into())
            };
            catalog.entries.push(OptionEntry {
                id,
                option_name: label(&row["optionName"], "Historical choice", 1024),
                package,
                expected_family,
                archive_binding: binding,
                source_label: label(&row["relativePath"], "Historical source", 4096),
                unavailable_reason: reason,
            });
        }
    }
    // Unmatched native refs remain independently retained. They do not prove that
    // any incomplete historical candidate describes these bytes.
    for p in &packages {
        let encoded = package::encode_packages(std::slice::from_ref(p))[0].clone();
        let key = store::canonical(&package::encode_packages(std::slice::from_ref(p)));
        let family = package::family_key(p)?;
        let needs_selection = !literal_empty && !selected_families.contains(&family);
        if represented.contains(&key) && !needs_selection {
            continue;
        }
        let id = legacy_id(mod_id, &object! {kind:"nativeFamily",package:encoded})?;
        if !used_ids.insert(id.clone()) {
            return Err(options::invalid());
        }
        if needs_selection {
            catalog.selected_ids.push(id.clone());
            selected_families.insert(family);
        }
        catalog.entries.push(OptionEntry {
            id,
            option_name: p.directory.source_name.clone(),
            package: Some(p.clone()),
            expected_family: Some(ExpectedFamily::from_package(p)),
            archive_binding: None,
            source_label: "Retained native family".into(),
            unavailable_reason: None,
        });
        represented.insert(key);
    }
    if catalog.entries.is_empty() {
        catalog.entries.push(OptionEntry {
            id:legacy_id(mod_id,&object!{kind:"unavailableRecord",source:record["source"].clone()})?,
            option_name:label(&record["source"]["name"],"Historical choice",1024),
            package:None,expected_family:None,archive_binding:None,source_label:"Historical source".into(),
            unavailable_reason:Some(label(&record["unavailableReason"],"No complete retained native family or historical candidate evidence is available; use a fresh import.",4096)),
        });
    }
    catalog.selected_ids.sort();
    catalog.encode(layout)?;
    Ok(catalog)
}

use super::{archive_sources, LocalPrepareRequest, LocalPrepared, Scan};
use crate::{archive_content::{self, ArchiveBudget, ArchiveInput}, archive_quarantine::QuarantineLease,
    package::{FileRef, PackageRef, SourcePackage}, prepare_control::PrepareControl, store::Store};
use std::{path::{Path, PathBuf}, sync::Arc, fs::File, os::unix::fs::MetadataExt};

#[derive(Clone, Debug)]
pub(crate) enum OptionSource { Local { roots: Vec<PathBuf> }, RetainedArchive { archive_id: String } }
impl OptionSource {
    pub(crate) fn decode(v: &JsonValue) -> Result<Self, NativeError> {
        if !v.is_object() || v.len()!=2 {return Err(NativeError::params());}
        match v["kind"].as_str() {
            Some("local") if v["roots"].is_array() && (1..=64).contains(&v["roots"].len()) => {
                let roots=v["roots"].members().map(|p| {
                    let p=PathBuf::from(p.as_str().ok_or_else(NativeError::params)?);
                    super::local_path(&p)?; Ok(p)
                }).collect::<Result<Vec<_>,NativeError>>()?;
                Ok(Self::Local { roots })
            }
            Some("retainedArchive") if v["archiveId"].as_str().is_some_and(|id|package::hex(id,32)) =>
                Ok(Self::RetainedArchive { archive_id:v["archiveId"].as_str().unwrap().into() }),
            _=>Err(NativeError::params()),
        }
    }
    pub(crate) fn encode(&self) -> JsonValue { match self {
        Self::Local{roots}=>object!{kind:"local",roots:JsonValue::from(roots.iter().map(|p|p.to_str().unwrap()).collect::<Vec<_>>())},
        Self::RetainedArchive{archive_id}=>object!{kind:"retainedArchive",archiveId:archive_id.clone()},
    }}
}
#[derive(Clone, Debug)]
pub(crate) struct OptionsPrepareRequest {
    pub root: PathBuf, pub profile_id: String, pub mod_id: String,
    pub expected_revision: u64, pub source: OptionSource,
}
impl OptionsPrepareRequest {
    pub(crate) fn local_request(&self) -> LocalPrepareRequest {
        LocalPrepareRequest { root:self.root.clone(), profile_id:self.profile_id.clone(), expected_revision:self.expected_revision,
            roots:match &self.source {OptionSource::Local{roots}=>roots.clone(),_=>Vec::new()}, artwork:None }
    }
}
/// Only the native owner resolver can construct managed source authority.
#[derive(Clone, Debug)]
pub(crate) struct OwnedArchive {
    file: FileRef,
    path: PathBuf,
    descriptor: Arc<File>,
    fingerprint: paths::Fingerprint,
    format: String,
}
impl OwnedArchive {
    fn verify_descriptor(&self) -> Result<(),NativeError> {
        let m=self.descriptor.metadata()?;
        let f=&self.fingerprint;
        if !m.is_file() || m.dev()!=f.device || m.ino()!=f.inode || m.len()!=f.size
            || m.mtime()!=f.modified_seconds || m.mtime_nsec()!=f.modified_nanos {return Err(super::stale());}
        Ok(())
    }
}
pub(crate) fn pinned_record(root: &Path, profile: &str, mod_id: &str, expected: u64) -> Result<(JsonValue,u32),NativeError> {
    if !paths::safe_id(profile) || !paths::safe_id(mod_id) {return Err(NativeError::params());}
    let store=Store::open(root)?; crate::apply::ensure_idle(&store)?;
    let doc=store.document()?;
    if doc["revision"]!=expected.to_string() {return Err(NativeError::new("staleRevision","The pinned revision changed; prepare again."));}
    let mut records=doc["profiles"][profile]["mods"].members().filter(|m|m["source"]["id"]==mod_id);
    let record=records.next().ok_or_else(||NativeError::new("unknownMod","The pinned profile/mod no longer exists."))?.clone();
    if records.next().is_some() {return Err(options::invalid());}
    Ok((record,package::layout_version(&doc)?))
}
pub(crate) fn record_digest(record: &JsonValue) -> Result<String,NativeError> { paths::digest_bytes(record.dump().as_bytes()) }
pub(crate) fn detail_envelope(record: &JsonValue, layout: u32, profile: &str, expected: u64) -> Result<JsonValue,NativeError> {
    let value=object!{version:1,profileId:profile,modId:record["source"]["id"].clone(),expectedRevision:expected.to_string(),detail:options::detail(record,layout)?};
    if crate::protocol::native_value_bytes(&value).is_none() {return Err(NativeError::new("nativeResponseLimit","The pinned options detail exceeds the response limit."));}
    Ok(value)
}
pub(crate) fn resolve_owned(root: &Path, record: &JsonValue, layout: u32, id: &str) -> Result<OwnedArchive,NativeError> {
    if !package::hex(id,32) {return Err(NativeError::params());}
    let catalog=catalog_for_record(record,layout)?;
    let file=crate::local_import::stored_archive_refs(record)?.into_iter().find(|f|f.id==id)
        .filter(|f|catalog.entries.iter().any(|e|e.archive_binding.as_ref().is_some_and(|b|b.file==*f)))
        .ok_or_else(||NativeError::new("invalidOptionSource","That archive is not an exact source owned by this mod."))?;
    let path=crate::storage::path(root.as_ref(), "archives")?.join(format!("{}.archive",file.id));
    let descriptor=Arc::new(paths::open_regular(&path)?);
    let metadata=descriptor.metadata()?;
    if metadata.len()!=file.size {return Err(super::stale());}
    if metadata.len()>super::MAX_MEMBER {return Err(super::bounds());}
    // Expected bytes plus descriptor identity, not a byte-validation proof.
    // Only the cancellable Options worker can produce that proof.
    let fingerprint=paths::Fingerprint {
        sha256:file.sha256.clone(),size:metadata.len(),device:metadata.dev(),inode:metadata.ino(),
        modified_seconds:metadata.mtime(),modified_nanos:metadata.mtime_nsec(),
    };
    let formats=record["source"]["localImport"]["archives"].members()
        .filter(|a|a["file"]["id"]==id).filter_map(|a|a["format"].as_str())
        .chain(catalog.retained_archives.iter().filter(|a|a.file.id==id).map(|a|a.format.as_str()))
        .collect::<HashSet<_>>();
    if formats.len()!=1 {return Err(super::stale());}
    let owned=OwnedArchive {file,path,descriptor,fingerprint,format:(*formats.iter().next().unwrap()).into()};
    owned.verify_descriptor()?; Ok(owned)
}
fn prepare_owned(request: LocalPrepareRequest, owned: OwnedArchive, control: &PrepareControl, lease: Arc<QuarantineLease>) -> Result<LocalPrepared,NativeError> {
    owned.verify_descriptor()?;
    control.report(crate::prepare_control::PrepareProgress {phase:"hashing".into(),..Default::default()});
    archive_sources::verify_fingerprint(&owned.path,&owned.fingerprint,control)?;
    let mut budget=ArchiveBudget::new(); budget.admit_roots(1)?;
    let archive=Arc::new(archive_content::prepare_archive(ArchiveInput {root_ordinal:0,relative_path:owned.file.source_name.clone(),
        original_path:owned.path.clone(),fingerprint:owned.fingerprint.clone()},lease.clone(),&mut budget,control)?);
    // Owner-built inventory; no renderer or historical path is interpreted.
    let metadata=paths::open_regular(&owned.path)?.metadata()?;
    let inventory=vec![object!{rootOrdinal:0,relativePath:owned.file.source_name.clone(),path:owned.path.to_str().unwrap(),kind:"file",identity:super::file_identity(&metadata)}];
    let scan=Scan { control,inventory,provenance:Vec::new(),sources:BTreeMap::new(),bytes:owned.file.size,unique:HashSet::new(),
        archives:Vec::new(),budget:Some(budget),fonts:Vec::new(),font_budget:None };
    // Managed option preparation extracts VPKs only and creates no font discovery authority.
    let prepared=super::assemble_prepared(request,control,scan,vec![archive],None)?;
    lease.validate_sealed()?;
    Ok(prepared)
}
pub(crate) fn revalidate(prepared: &LocalPrepared, owned: Option<&OwnedArchive>, control: &PrepareControl) -> Result<(),NativeError> {
    let Some(owned)=owned else {return super::revalidate_local(prepared,control)};
    archive_sources::verify_fingerprint(&owned.path,&owned.fingerprint,control)?;
    if !prepared.request.roots.is_empty() || prepared.archives.len()!=1 {return Err(super::stale());}
    owned.verify_descriptor()?;
    let archive=&prepared.archives[0];
    if archive.format()!=owned.format || archive.input().original_path!=owned.path || archive.input().fingerprint!=owned.fingerprint
        || archive.input().relative_path!=owned.file.source_name || archive.snapshot_fingerprint().sha256!=owned.file.sha256 {return Err(super::stale());}
    archive.lease().validate_sealed()?;
    archive_sources::verify_fingerprint(archive.snapshot_path(),archive.snapshot_fingerprint(),control)?;
    for (path,fp) in prepared.fingerprints.iter() {archive_sources::verify_fingerprint(path,fp,control)?;}
    archive.lease().validate_sealed()?; control.check()
}
/// A live byte-validation result produced only by the Options preparation worker.
/// Fields are private: neither JSON history nor a metadata-only source resolver
/// can create a ready proof. Review borrows this value; it never refreshes hashes.
#[derive(Clone)]
pub(crate) struct PreparedOptions {
    prepared: LocalPrepared,
    owned: Option<OwnedArchive>,
    lease: Arc<QuarantineLease>,
    identities: BTreeMap<PathBuf, JsonValue>,
}
impl PreparedOptions {
    pub(crate) fn prepared(&self) -> &LocalPrepared { &self.prepared }
    pub(crate) fn retain_preparation_slot(&mut self, slot: Arc<crate::remote::preparation_slot::PreparationSlotLease>) {
        self.prepared.retain_preparation_slot(slot);
    }

    fn check_freshness(&self, control: Option<&PrepareControl>) -> Result<(), NativeError> {
        self.lease.validate_sealed()?;
        if let Some(owned)=&self.owned {owned.verify_descriptor()?;}
        check_inventory_metadata(&self.prepared,control)?;
        for (path,identity) in &self.identities {
            if let Some(control)=control {control.check()?;}
            let descriptor=paths::open_regular(path)?;
            if source_identity(&descriptor.metadata()?)!=*identity {return Err(super::stale());}
        }
        self.lease.validate_sealed()
    }
}
fn source_identity(metadata: &std::fs::Metadata) -> JsonValue {
    let mut identity=super::file_identity(metadata);
    identity["links"]=metadata.nlink().into();
    identity
}
fn check_inventory_metadata(prepared: &LocalPrepared, control: Option<&PrepareControl>) -> Result<(), NativeError> {
    for root in &prepared.request.roots {
        if let Some(control)=control {control.check()?;}
        paths::reject_symlinks(root)?;
    }
    for entry in prepared.evidence["inventory"].members() {
        if let Some(control)=control {control.check()?;}
        // This is live owner-created inventory, never a historical record path.
        let path=Path::new(entry["path"].as_str().ok_or_else(super::stale)?);
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound && entry["kind"]=="missing" => {},
            Ok(metadata) if entry["kind"]!="missing" && super::file_identity(&metadata)==entry["identity"] => {},
            _=>return Err(super::stale()),
        }
    }
    Ok(())
}
fn capture_identities(prepared: &LocalPrepared, control: &PrepareControl) -> Result<BTreeMap<PathBuf,JsonValue>, NativeError> {
    let mut files=prepared.fingerprints.as_ref().clone();
    for archive in &prepared.archives {
        files.insert(archive.input().original_path.clone(),archive.input().fingerprint.clone());
        files.insert(archive.snapshot_path().into(),archive.snapshot_fingerprint().clone());
    }
    for font in prepared.fonts.iter().filter_map(|candidate|candidate.prepared.as_ref()) {
        files.insert(font.path().into(),font.fingerprint().clone());
    }
    if let Some(artwork)=&prepared.artwork {files.insert(artwork.path.clone(),artwork.fingerprint.clone());}
    let mut identities=BTreeMap::new();
    for (path,fp) in files {
        control.check()?;
        let descriptor=paths::open_regular(&path)?;
        let metadata=descriptor.metadata()?;
        if metadata.len()!=fp.size || metadata.dev()!=fp.device || metadata.ino()!=fp.inode
            || metadata.mtime()!=fp.modified_seconds || metadata.mtime_nsec()!=fp.modified_nanos {return Err(super::stale());}
        identities.insert(path,source_identity(&metadata));
    }
    Ok(identities)
}
/// The caller installs this operation in the one LocalJobs worker and retains
/// its PrepareControl. All byte reads/hashes below use that reachable control.
pub(crate) fn prepare_options(
    request: LocalPrepareRequest,
    owned: Option<OwnedArchive>,
    control: &PrepareControl,
    lease: Arc<QuarantineLease>,
) -> Result<PreparedOptions,NativeError> {
    let prepared=match &owned {
        Some(owned)=>prepare_owned(request,owned.clone(),control,lease.clone())?,
        None=>super::prepare_local_job(request,control,lease.clone())?,
    };
    let mut progress=control.progress();
    progress.phase="hashing".into(); control.report(progress.clone());
    lease.validate_sealed()?;
    check_inventory_metadata(&prepared,Some(control))?;
    // Capture before the final byte pass, then require exactly those same
    // identities afterward. This binds the immutable proof to validated bytes.
    let identities=capture_identities(&prepared,control)?;
    revalidate(&prepared,owned.as_ref(),control)?;
    let proof=PreparedOptions {prepared,owned,lease,identities};
    proof.check_freshness(Some(control))?;
    store::ensure_internal(&object! {
        prepared:super::prepared_record(&proof.prepared),
        identities:JsonValue::from(proof.identities.iter().map(|(path,identity)|
            object!{path:path.to_str().unwrap(),identity:identity.clone()}).collect::<Vec<_>>())
    })?;
    control.check()?;
    progress.phase="ready".into(); control.report(progress);
    Ok(proof)
}

#[derive(Clone,Debug)]
pub(crate) struct OptionBinding { pub option_id: String, pub candidate_id: String }
pub(crate) fn decode_bindings(value: &JsonValue) -> Result<Vec<OptionBinding>,NativeError> {
    if !value.is_array() || value.is_empty() || value.len()>1024 {return Err(NativeError::params());}
    let mut options=HashSet::new(); let mut candidates=HashSet::new();
    value.members().map(|v| {
        if !v.is_object() || v.len()!=2 {return Err(NativeError::params());}
        let option=v["optionId"].as_str().filter(|id|package::hex(id,32)).ok_or_else(NativeError::params)?;
        let candidate=v["candidateId"].as_str().filter(|id|package::hex(id,64)).ok_or_else(NativeError::params)?;
        if !options.insert(option) || !candidates.insert(candidate) {return Err(NativeError::params());}
        Ok(OptionBinding{option_id:option.into(),candidate_id:candidate.into()})
    }).collect()
}
#[derive(Clone,Debug)]
pub(crate) struct OptionAttachmentPlan {
    pub prepared: LocalPrepared, pub owned: Option<OwnedArchive>, pub token: String,
    pub before_record: JsonValue, pub record: JsonValue, pub summary: JsonValue,
    pub sources: Vec<SourcePackage>, pub packages: Vec<PackageRef>,
    pub archives: Vec<Arc<archive_content::PreparedArchive>>, pub archive_refs: Vec<FileRef>,
    pub dependencies: Vec<(FileRef, &'static str)>,
}
pub(crate) fn review(proof: &PreparedOptions, before: &JsonValue, layout: u32, bindings: &[OptionBinding]) -> Result<OptionAttachmentPlan,NativeError> {
    proof.check_freshness(None)?;
    let prepared=&proof.prepared;
    let owned=proof.owned.as_ref();
    if bindings.is_empty() || bindings.len()>1024 {return Err(NativeError::params());}
    let mut catalog=catalog_for_record(before,layout)?;
    let mut ids=HashSet::new(); let mut candidate_ids=HashSet::new(); let mut sources=Vec::new();
    for b in bindings {
        if !ids.insert(&b.option_id) || !candidate_ids.insert(&b.candidate_id) {return Err(NativeError::params());}
        let entry=catalog.entries.iter().find(|e|e.id==b.option_id).ok_or_else(options::invalid)?;
        if entry.package.is_some() {return Err(NativeError::new("optionAlreadyReady","This choice already has retained bytes; its binding cannot be replaced."));}
        if entry.expected_family.is_none() {return Err(NativeError::new("optionEvidenceIncomplete","This historical choice lacks complete family names, indices, sizes or hashes. Use a fresh import; its identity cannot be rebound safely."));}
        let source=prepared.candidates.iter().find(|c|c.id==b.candidate_id).and_then(|c|c.package.clone())
            .ok_or_else(||NativeError::new("invalidOptionBinding","Choose a freshly prepared complete candidate family."))?;
        sources.push(source);
    }
    let packages=super::allocate_package_refs(prepared,&sources)?;
    let archives=archive_sources::selected(prepared,&sources,&[])?;
    let archive_refs=archives.iter().map(|a| {
        if let Some(owned)=owned {return Ok(owned.file.clone());}
        Ok(FileRef{id:paths::random_id()?,sha256:a.snapshot_fingerprint().sha256.clone(),size:a.snapshot_fingerprint().size,source_name:a.input().relative_path.rsplit('/').next().ok_or_else(super::stale)?.into()})
    }).collect::<Result<Vec<_>,NativeError>>()?;
    for ((binding,source),package) in bindings.iter().zip(&sources).zip(&packages) {
        let entry=catalog.entries.iter_mut().find(|e|e.id==binding.option_id).unwrap();
        if entry.expected_family.as_ref()!=Some(&ExpectedFamily::from_package(package)) {
            return Err(NativeError::new("optionContentMismatch","Every directory and companion name, index, size and hash must match the historical choice. Use a fresh import for different content."));
        }
        let archive_binding=archive_sources::binding(&archives,&archive_refs,source)?;
        if owned.is_some() && entry.archive_binding!=archive_binding {
            return Err(NativeError::new("optionArchiveMismatch","The retained archive digest and every physical member index/path must match the recorded binding."));
        }
        entry.package=Some(package.clone()); entry.unavailable_reason=None;
        if archive_binding.is_some() {entry.archive_binding=archive_binding;}
    }
    if owned.is_none() && !archives.is_empty() {
        let attachment_id=paths::random_id()?;
        catalog.attachments.push(options::Attachment{id:attachment_id.clone(),source_kind:"chosenRoots".into(),source_label:"Reviewed local source attachment".into()});
        for (archive,file) in archives.iter().zip(&archive_refs) {
            catalog.retained_archives.push(options::RetainedArchive{file:file.clone(),attachment_id:attachment_id.clone(),root_ordinal:archive.input().root_ordinal as u32,relative_path:archive.input().relative_path.clone(),format:archive.format().into()});
        }
    }
    let mut record=before.clone(); record.remove("packages"); record.remove("payloads"); record["options"]=catalog.encode(layout)?;
    options::for_record(&record,layout)?;
    let detail=options::detail(&record,layout)?;
    let new_payload=packages.iter().flat_map(|p|std::iter::once(&p.directory).chain(p.companions.values())).map(|f|(f.id.clone(),f.size)).collect::<BTreeMap<_,_>>().values().sum::<u64>();
    let new_archive=if owned.is_some(){0}else{archive_refs.iter().map(|f|f.size).sum()};
    let summary=object!{attachedOptionCount:bindings.len(),newPayloadBytes:new_payload,newArchiveBytes:new_archive,newWriteBytes:new_payload.checked_add(new_archive).ok_or_else(super::bounds)?,resultingTotals:detail["totals"].clone()};
    let mut dependencies=BTreeMap::new();
    for (file,role) in crate::local_import::record_files(before,layout)? {
        if dependencies.insert(file.id.clone(),(file.clone(),role)).is_some_and(|prior|prior!=(file,role)) {return Err(options::invalid());}
    }
    let plan=OptionAttachmentPlan{prepared:prepared.clone(),owned:owned.cloned(),token:paths::random_id()?,before_record:before.clone(),record,summary,sources,packages,archives,archive_refs,dependencies:dependencies.into_values().collect()};
    store::ensure_internal(&object!{before:plan.before_record.clone(),after:plan.record.clone(),prepared:super::prepared_record(prepared),summary:plan.summary.clone(),dependencies:JsonValue::from(plan.dependencies.iter().map(|(file,role)|object!{file:package::encode_file(file),role:*role}).collect::<Vec<_>>())})?;
    Ok(plan)
}
