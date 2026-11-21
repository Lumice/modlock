//! Additive immutable local publication. Durable journals are the only cleanup authority.
use crate::{
    local_content::{revalidate_local, LocalImportPlan},
    native::NativeError,
    package::{self, FileRef},
    paths,
    prepare_control::PrepareControl,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
fn recovery() -> NativeError {
    NativeError::new(
        "recoveryRequired",
        "Local import ownership is incomplete or changed; preserve its journal and files.",
    )
}
fn exists(p: &Path) -> Result<bool, NativeError> {
    match fs::symlink_metadata(p) {
        Ok(_) => {
            paths::reject_symlinks(p)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(p)?;
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}
fn identity(p: &Path, d: &JsonValue, i: &JsonValue) -> Result<(), NativeError> {
    let m = paths::open_regular(p)?.metadata()?;
    if d.as_u64() != Some(m.dev()) || i.as_u64() != Some(m.ino()) {
        return Err(recovery());
    }
    Ok(())
}
fn recorded_identity(lock:&RootLock,p:&Path,d:&JsonValue,i:&JsonValue,fsid:Option<&str>,directory:bool)->Result<(),NativeError>{
    let file=if directory {fs::File::open(p)?} else {paths::open_regular(p)?};
    let meta=file.metadata()?;
    if meta.is_dir()!=directory || !paths::same_persisted_object(lock.root(),lock.directory(),p,&file,d.as_u64().ok_or_else(recovery)?,i.as_u64().ok_or_else(recovery)?,fsid)? {return Err(recovery());}
    Ok(())
}
fn verify_recorded(lock:&RootLock,p:&Path,f:&JsonValue,fsid:Option<&str>)->Result<(),NativeError>{
    let mut file=paths::open_regular(p)?;let metadata=file.metadata()?;let admitted=f["size"].as_u64().ok_or_else(recovery)?;
    let device=f["device"].as_u64().ok_or_else(recovery)?;let inode=f["inode"].as_u64().ok_or_else(recovery)?;
    if !paths::same_persisted_object(lock.root(),lock.directory(),p,&file,device,inode,fsid)? || metadata.len()>admitted || f["complete"]==true && metadata.len()!=admitted {return Err(recovery());}
    if f["complete"]==true {let fp=paths::fingerprint_file(&mut file)?;if f["sha256"]!=fp.sha256||admitted!=fp.size{return Err(recovery());}}
    if !paths::same_persisted_object(lock.root(),lock.directory(),p,&file,device,inode,fsid)? {return Err(recovery());}
    Ok(())
}
pub(crate) fn destination(root: &Path, f: &JsonValue) -> Result<PathBuf, NativeError> {
    let r = package::decode_file(f)?;
    match f["role"].as_str() {
        Some("payload") => Ok(crate::storage::path(root.as_ref(), "payloads")?.join(format!("{}.vpk", r.id))),
        Some("artwork") => Ok(crate::storage::path(root.as_ref(), "artwork")?.join(format!("{}.image", r.id))),
        Some("archive") => Ok(crate::storage::path(root.as_ref(), "archives")?.join(format!("{}.archive", r.id))),
        Some("font") => crate::fonts::font_path(root, &r.id),
        _ => Err(recovery()),
    }
}
pub(crate) fn staged(stage: &Path, f: &JsonValue) -> Result<PathBuf, NativeError> {
    Ok(stage.join(
        destination(Path::new("/"), f)?
            .file_name()
            .ok_or_else(recovery)?,
    ))
}
pub(crate) fn verify(p: &Path, f: &JsonValue) -> Result<(), NativeError> {
    let mut file = paths::open_regular(p)?;
    let metadata = file.metadata()?;
    let admitted = f["size"].as_u64().ok_or_else(recovery)?;
    if f["device"].as_u64() != Some(metadata.dev())
        || f["inode"].as_u64() != Some(metadata.ino())
        || metadata.len() > admitted
        || (f["complete"] == true && metadata.len() != admitted) {
        return Err(recovery());
    }
    if f["complete"] == true {
        let fp = paths::fingerprint_file(&mut file)?;
        if f["sha256"] != fp.sha256 || admitted != fp.size {
            return Err(recovery());
        }
    }
    identity(p, &f["device"], &f["inode"])
}
// Duplicate identity is descriptive only: it never opens a source path. Keep
// occurrence coordinates and complete expected families, but remove allocated IDs.
fn duplicate_identity(record: &JsonValue, layout: u32) -> Result<String, NativeError> {
    fn strip_ids(v: &mut JsonValue) {
        if v.is_object() {
            v.remove("id"); v.remove("archiveId"); v.remove("sourcePath");
            for (_, child) in v.entries_mut() {strip_ids(child);}
        } else if v.is_array() {for child in v.members_mut() {strip_ids(child);}}
    }
    let catalog = crate::options::for_record(record, layout)?;
    let mut families = catalog.entries.iter().filter(|e|e.package.is_some()).map(|e| {
        object!{optionName:e.option_name.clone(),sourceLabel:e.source_label.clone(),expectedFamily:e.expected_family.as_ref().map(crate::options::ExpectedFamily::encode)}
    }).collect::<Vec<_>>();
    families.sort_by_key(store::canonical);
    let local = &record["source"]["localImport"];
    let mut candidates = JsonValue::from(local["candidates"].members().filter(|c|c["available"]==true).cloned().collect::<Vec<_>>());
    strip_ids(&mut candidates);
    let mut fonts = local["fonts"].members().cloned().collect::<Vec<_>>();
    for f in &mut fonts {strip_ids(f);}
    fonts.sort_by_key(store::canonical);
    let normalized_rows = |key: &str| {
        let mut rows=local[key].members().cloned().collect::<Vec<_>>();
        for row in &mut rows {strip_ids(row);}
        rows.sort_by_key(store::canonical);
        JsonValue::from(rows)
    };
    Ok(store::canonical(&object!{families:JsonValue::from(families),candidates:candidates,
        selectedCandidateIds:local["selectedCandidateIds"].clone(),fonts:JsonValue::from(fonts),
        fontCandidates:normalized_rows("fontCandidates"),fontDiagnostics:normalized_rows("fontDiagnostics")}))
}
// Gregorian civil date from days since Unix epoch; avoids locale/process state.
fn created_at() -> Result<String, NativeError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| NativeError::params())?
        .as_secs();
    let z = (seconds / 86400) as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    ))
}
fn authority_digest(body: &JsonValue) -> Result<String, NativeError> {
    let mut value = body.clone();
    for key in [
        "owner",
        "stageDevice",
        "stageInode",
        "ownerDevice",
        "ownerInode",
    ] {
        value.remove(key);
    }
    for file in value["files"].members_mut() {
        for key in ["device", "inode", "complete"] {
            file.remove(key);
        }
    }
    paths::digest_bytes(store::canonical(&value).as_bytes())
}
fn stale_source() -> NativeError {
    NativeError::new("staleLocalSource", "A copied source differs from its reviewed identity, size or bytes.")
}
fn source_identity(file: &fs::File, expected: &paths::Fingerprint) -> Result<(), NativeError> {
    let m = file.metadata()?;
    if m.len() != expected.size || m.dev() != expected.device || m.ino() != expected.inode
        || m.mtime() != expected.modified_seconds || m.mtime_nsec() != expected.modified_nanos {
        return Err(stale_source());
    }
    Ok(())
}
// Transient proof for this writer only; recovery still hashes durable files.
fn live_stamp(file:&fs::File)->Result<(u64,u64,u64,i64,i64,i64,i64,String),NativeError>{
    let m=file.metadata()?;Ok((m.dev(),m.ino(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec(),paths::persisted_filesystem_id(file)?))
}
fn recheck_source(file: &mut fs::File, path: &Path, expected: &paths::Fingerprint) -> Result<(), NativeError> {
    source_identity(file, expected)?;
    source_identity(&paths::open_regular(path)?, expected)
}
/// At most the admitted size is ever written; the extra EOF byte is read only.
/// A growing source cannot make the staging allocation grow without a bound.
fn copy_exact(source: &mut fs::File, out: &mut fs::File, size: u64,
              checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>) -> Result<(), NativeError> {
    source.seek(SeekFrom::Start(0))?;
    let mut remaining = size;
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = source.read(&mut buffer[..limit])?;
        if read == 0 { return Err(stale_source()); }
        out.write_all(&buffer[..read])?;
        remaining -= read as u64;
        checkpoint("memberCopyChunk")?;
    }
    if source.read(&mut buffer[..1])? != 0 { return Err(stale_source()); }
    Ok(())
}
pub fn commit_local(
    plan: &LocalImportPlan,
    lock: &RootLock,
    expected: u64,
    operation_id: &str,
    input: &JsonValue,
    checkpoint: &mut impl FnMut(&str) -> Result<(), NativeError>,
) -> Result<JsonValue, NativeError> {
    if lock.root() != plan.prepared.request.root
        || expected != plan.prepared.request.expected_revision
    {
        return Err(NativeError::params());
    }
    let store = Store::open_writer(lock)?;
    if let Some(result) = store.replay(operation_id, input)? { return Ok(result); }
    crate::apply::ensure_idle(&store)?;
    let before = store.document()?;
    if before["revision"] != expected.to_string() { return Err(NativeError::new("staleRevision","Prepare again.")); }
    let mut publication = build_local(plan, &store, &before, expected, operation_id, input)?;
    if !plan.group.is_empty() {
        if plan.group.len()>64 { return Err(NativeError::params()); }
        let mut records=Vec::new(); let mut document=before.clone(); let mut copies=CopySet::new(); let mut packages=Vec::new();
        for (_,child) in &plan.group {
            if !child.group.is_empty() || child.prepared.request.root!=lock.root() || child.prepared.request.profile_id!=plan.prepared.request.profile_id || child.prepared.request.expected_revision!=expected {return Err(NativeError::params());}
            let part=build_local(child,&store,&document,expected,operation_id,input)?;
            records.push(part.journal["record"].clone()); document=part.document;
            document["revision"]=expected.to_string().into();
            for (id,copy) in part.copies { if copies.insert(id,copy).is_some() {return Err(NativeError::params());} }
            packages.extend(part.packages);
        }
        let total=copies.values().try_fold(0u64,|n,(f,_,_,_)|n.checked_add(f.size).ok_or_else(NativeError::params))?;
        if total>137438953472 {return Err(NativeError::params());}
        document["revision"]=(expected+1).to_string().into();
        let j=&mut publication.journal;
        j["action"]="addMods".into(); j["owner"]["action"]="addMods".into();
        j.remove("record");j.remove("modId");j["records"]=records.into();
        j["afterDigest"]=paths::digest_bytes(document.dump().as_bytes())?.into();
        j["files"]=copies.values().map(|(f,_,role,_)|{let mut r=package::encode_file(f);r["role"]=role.clone().into();r["device"]=JsonValue::Null;r["inode"]=JsonValue::Null;r["complete"]=false.into();r}).collect::<Vec<_>>().into();
        j["owner"]["authorityDigest"]=authority_digest(j)?.into();
        publication.document=document;publication.copies=copies;publication.packages=packages;
        admit_group(&publication.journal)?;
    }
    let profile=&plan.prepared.request.profile_id;
    let stage=crate::storage::path(store.root(), publication.journal["stage"].as_str().ok_or_else(recovery)?)?;
    execute_publication(&store,lock,expected,operation_id,input,publication.document,profile,&stage,publication.copies,publication.packages,publication.journal,
        &mut || {revalidate_local(&plan.prepared,&PrepareControl::never_cancelled())?;for (_,p) in &plan.group {if !std::sync::Arc::ptr_eq(&p.prepared.evidence,&plan.prepared.evidence) {revalidate_local(&p.prepared,&PrepareControl::never_cancelled())?;}} Ok(())},checkpoint)
}
struct Publication { document:JsonValue, copies:CopySet, packages:Vec<package::PackageRef>, journal:JsonValue }
fn build_local(plan:&LocalImportPlan, store:&Store, before:&JsonValue, expected:u64, operation_id:&str,input:&JsonValue)->Result<Publication,NativeError> {
    let mut document=before.clone();
    let profile = &plan.prepared.request.profile_id;
    if !document["profiles"][profile].is_object() {
        return Err(NativeError::new(
            "unknownProfile",
            "The pinned profile no longer exists.",
        ));
    }
    let library_before = package::document_version(&document)?;
    let version = package::layout_version(&document)?;
    let font_refs = plan.font_refs();
    let font_sources = plan.selected_font_sources();
    let library_after = 4;
    document["storeVersion"] = library_after.into();
    document["packageVersion"] = version.into();
    if library_before < 3 { document["fonts"] = object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null}; }
    let sources = plan.retained_sources()?;
    let refs = plan.retained_package_refs()?;
    let selected_refs = plan.package_refs()?;
    let mut unique = BTreeSet::new();
    let mut packages = Vec::new();
    let archive_sources = plan.selected_archive_sources();
    let archive_refs = plan.archive_refs();
    if sources.len() != refs.len() || archive_sources.len() != archive_refs.len() || font_refs.len()!=font_sources.len() {
        return Err(NativeError::params());
    }
    let journal_version = if crate::deployment_metadata::retained_shards(&before) {7} else {6};
    let mut copies = BTreeMap::<String, (FileRef, PathBuf, String, paths::Fingerprint)>::new();
    for (source, p) in sources.iter().zip(&refs) {
        if version == 1
            && (!p.companions.is_empty()
                || crate::vpk::inspect(&source.directory.source_path).is_err())
        {
            return Err(NativeError::new(
                "unsupportedLocalPackageForStore",
                "This legacy library can import only legacy-compatible standalone VPKs.",
            ));
        }
        if unique.insert(store::canonical(&package::encode_packages(std::slice::from_ref(p)))) {
            packages.push(p.clone());
        }
        for (r, s) in std::iter::once((&p.directory, &source.directory))
            .chain(p.companions.iter().map(|(i, r)| (r, &source.companions[i])))
        {
            copies.entry(r.id.clone()).or_insert((
                r.clone(),
                s.source_path.clone(),
                "payload".into(),
                plan.prepared.fingerprint_for(&s.source_path)?,
            ));
        }
    }
    let mut maximum_order: Option<i64> = None;
    for (index, m) in document["profiles"][profile]["mods"].members().enumerate() {
        let order = m["source"]["installOrder"]
            .as_i64()
            .unwrap_or(i64::try_from(index).map_err(|_| NativeError::params())?);
        maximum_order = Some(maximum_order.map_or(order, |old| old.max(order)));
    }
    let order = maximum_order.unwrap_or(-1).checked_add(1).ok_or_else(|| {
        NativeError::new(
            "localInstallOrderLimit",
            "No representable append order remains.",
        )
    })?;
    let mut provenance = plan.summary.clone();
    provenance.remove("token");
    provenance["version"] = if !font_refs.is_empty() {3} else if archive_refs.is_empty() { 1 } else { 2 }.into();
    provenance["selectedPackageCount"] = selected_refs.len().into();
    provenance["storedPackageCount"] = selected_refs.iter().map(package::family_key).collect::<Result<BTreeSet<_>,_>>()?.len().into();
    if !font_refs.is_empty() {provenance["fonts"]=plan.font_history()?;} else {provenance.remove("fonts");}
    // Only the local-content owner decodes its private evidence.
    provenance["fingerprints"] = plan.source_fingerprints()?;
    if let Some(art) = &plan.prepared.artwork {
        let f = plan.artwork_ref().ok_or_else(NativeError::params)?;
        provenance["artwork"] = package::encode_file(&f);
        provenance["artworkFormat"] = art.format.clone().into();
        provenance["sourceArtwork"] = object! {path:art.path.to_str().ok_or_else(NativeError::params)?,sha256:art.fingerprint.sha256.clone(),size:art.fingerprint.size,device:art.fingerprint.device,inode:art.fingerprint.inode,modifiedSeconds:art.fingerprint.modified_seconds,modifiedNanos:art.fingerprint.modified_nanos,format:art.format.clone()};
        if copies.insert(f.id.clone(), (f, art.path.clone(), "artwork".into(), art.fingerprint.clone())).is_some() {
            return Err(NativeError::params());
        }
    }
    if !archive_refs.is_empty() {
        let mut archive_records = Vec::new();
        for (archive, file) in archive_sources.iter().zip(&archive_refs) {
            let fp = archive.snapshot_fingerprint();
            if file.sha256 != fp.sha256 || file.size != fp.size {
                return Err(NativeError::params());
            }
            archive_records.push(object! {
                rootOrdinal:archive.input().root_ordinal,relativePath:archive.input().relative_path.clone(),
                originalPath:archive.input().original_path.to_str().ok_or_else(NativeError::params)?,
                format:archive.format(),file:package::encode_file(file)
            });
            if copies.insert(file.id.clone(), (file.clone(), archive.snapshot_path().into(), "archive".into(), fp.clone())).is_some() {
                return Err(NativeError::params());
            }
        }
        provenance["archives"] = JsonValue::from(archive_records);
    }
    for (font,source) in font_refs.iter().zip(font_sources) {
        if copies.insert(font.asset.id.clone(),(font.asset.clone(),source.path().into(),"font".into(),source.fingerprint().clone())).is_some() {return Err(NativeError::params());}
    }
    let required_bytes = copies.values().try_fold(0u64, |sum, (file, _, _, fp)| {
        if file.sha256 != fp.sha256 || file.size != fp.size { return Err(NativeError::params()); }
        sum.checked_add(file.size).ok_or_else(NativeError::params)
    })?;
    let typed_bytes=|role: &str|copies.values().filter(|(_,_,r,_)|r==role).try_fold(0u64,|n,(f,_,_,_)|n.checked_add(f.size).ok_or_else(NativeError::params));
    let font_bytes=typed_bytes("font")?;
    let logical_bytes=selected_refs.iter().flat_map(|p|std::iter::once(&p.directory).chain(p.companions.values())).try_fold(0u64,|n,f|n.checked_add(f.size).ok_or_else(NativeError::params))?;
    let retained_logical=packages.iter().flat_map(|p|std::iter::once(&p.directory).chain(p.companions.values())).try_fold(0u64,|n,f|n.checked_add(f.size).ok_or_else(NativeError::params))?;
    if plan.summary["retainedLogicalPackageBytes"].as_u64()!=Some(retained_logical) || retained_logical>137438953472
        || plan.summary["selectedLogicalPackageBytes"].as_u64()!=Some(logical_bytes)
        || plan.summary["uniqueCopyBytes"].as_u64() != Some(required_bytes) || required_bytes>137589948416
        || plan.summary["logicalPackageBytes"].as_u64()!=Some(logical_bytes) || logical_bytes>137438953472
        || plan.summary["uniquePayloadBytes"].as_u64()!=Some(typed_bytes("payload")?) || typed_bytes("payload")?>137438953472
        || plan.summary["retainedArchiveBytes"].as_u64()!=Some(typed_bytes("archive")?) || typed_bytes("archive")?>68719476736
        || font_bytes>134217728 || (!plan.summary["uniqueFontBytes"].is_null() && plan.summary["uniqueFontBytes"].as_u64()!=Some(font_bytes))
        || (!font_refs.is_empty() && plan.summary["uniqueFontBytes"].as_u64()!=Some(font_bytes)) {
        return Err(NativeError::params());
    }
    reject_existing_claims(&before, version, &copies.keys().cloned().collect(), None)?;
    let m = &plan.selection.metadata;
    let mut record = object! {source:object!{id:plan.mod_id.clone(),remoteId:plan.mod_id.clone(),kind:"local",name:m.name.clone(),author:m.author.clone(),category:m.category.clone(),link:m.link.clone(),description:m.description.clone(),status:"downloaded",installedVpks:json::array![],installOrder:order,createdAt:created_at()?,localImport:provenance},available:true,previewHidden:false,enabled:false};
    if !plan.prepared.evidence["installedGameOrigins"].is_empty() { record["source"]["installedGameOrigins"] = plan.prepared.evidence["installedGameOrigins"].clone(); }
    if let Some(link) = crate::installed::confirmed_link_overlay(&m.link) { record["source"]["nativeRemoteLink"] = link; }
    if let Some(game) = crate::installed::initial_game_path(&before,&record)? { document["gamePath"] = game.into(); }
    record["options"] = plan.option_catalog().encode(version)?;
    crate::options::for_record(&record, version)?.validate_selection_save()?;
    let duplicate = duplicate_identity(&record,version)?;
    for existing in document["profiles"][profile]["mods"].members().filter(|m|m["source"]["kind"]=="local") {
        if duplicate_identity(existing,version).is_ok_and(|identity|identity==duplicate) {
            return Err(NativeError::new("duplicateLocalContent","This complete local choice and initial font inventory already exists in the pinned profile."));
        }
    }
    document["profiles"][profile]["mods"]
        .push(record.clone())
        .map_err(|_| NativeError::params())?;
    let next = expected.checked_add(1).ok_or_else(NativeError::params)?;
    document["revision"] = next.to_string().into();
    store::ensure_readable(&document)?;
    store::ensure_internal(input)?;
    store::ensure_internal(
        &object! {operationId:operation_id,input:input.clone(),outcome:object!{status:"succeeded",result:store::snapshot_document(&document,Some(profile))?}},
    )?;
    let stage_name = format!(".import-local-{}", paths::random_id()?);
    let owner = object! {kind:"import",version:journal_version,operationId:operation_id,stage:stage_name.clone(),nonce:paths::random_id()?,storageFsid:paths::persisted_filesystem_id(&fs::File::open(crate::storage::root(store.root())?)?)?};
    let mut files = JsonValue::new_array();
    for (f, _, role, _) in copies.values() {
        let mut row = package::encode_file(f);
        row["role"] = role.clone().into();
        row["device"] = JsonValue::Null;
        row["inode"] = JsonValue::Null;
        row["complete"] = false.into();
        paths::verify_absent(&destination(store.root(), &row)?)?;
        files.push(row).map_err(|_| NativeError::params())?;
    }
    let mut journal = object! {kind:"import",version:journal_version,operationId:operation_id,input:input.clone(),expectedRevision:expected.to_string(),targetProfileId:profile.clone(),beforeDigest:paths::digest_bytes(before.dump().as_bytes())?,storeVersion:version,record:record,stage:stage_name,owner:owner.clone(),stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,files:files};
    if matches!(journal_version,6|7) {
        journal.remove("storeVersion");
        journal["libraryVersionBefore"]=library_before.into();
        journal["libraryVersionAfter"]=library_after.into();
        journal["packageVersion"]=version.into();
    }
    journal["action"] = "addMod".into();
    journal["modId"] = plan.mod_id.clone().into();
    journal["reusedFiles"] = JsonValue::new_array();
    journal["beforeDocument"] = before.clone();
    journal["afterDigest"] = paths::digest_bytes(document.dump().as_bytes())?.into();
    journal["owner"]["action"] = "addMod".into();
    journal["owner"]["authorityDigest"] = authority_digest(&journal)?.into();
    admit_add_record(&journal)?;
    Ok(Publication{document,copies,packages,journal})
}

pub(crate) type CopySet = BTreeMap<String,(FileRef,PathBuf,String,paths::Fingerprint)>;
/// The shared byte writer receives only fresh copies. Dependencies never enter it.
fn execute_publication(store: &Store, lock: &RootLock, expected: u64, operation_id: &str, input: &JsonValue,
    document: JsonValue, profile: &str, stage: &Path, copies: CopySet, packages: Vec<package::PackageRef>, journal: JsonValue,
    revalidate: &mut impl FnMut()->Result<(),NativeError>, checkpoint: &mut impl FnMut(&str)->Result<(),NativeError>) -> Result<JsonValue,NativeError> {
    let _hash_scope=paths::HashMemo::default().enter();
    let mut admission = journal.clone();
    for k in ["stageDevice", "stageInode", "ownerDevice", "ownerInode"] {
        admission[k] = u64::MAX.into();
    }
    for f in admission["files"].members_mut() {
        f["device"] = u64::MAX.into();
        f["inode"] = u64::MAX.into();
    }
    store::ensure_internal(&admission)?;
    revalidate()?;
    store.write_journal(lock, operation_id, &journal)?;
    let result = write_immutable_copies(store,lock,expected,operation_id,input,document,profile,stage,copies,packages,journal,
        revalidate,checkpoint);
    let durable = store.journal(operation_id)?.ok_or_else(recovery)?;
    let committed = store.operation(operation_id)?["status"] == "succeeded";
    let cleanup = (|| {
        checkpoint("cleanup")?;
        // Even immediate cleanup preflights the complete journal set. Never
        // partially unlink this operation before discovering a conflicting claim.
        crate::import::preflight_imports(store,lock)?;
        plan_local_cleanup(store, lock, &durable, committed)?.execute(store)?;
        if committed {
            let mut evidence=Vec::new();
            for row in durable["files"].members().filter(|r|r["role"]=="payload") {
                let path=destination(store.root(),row)?;
                evidence.push((path.clone(),paths::fingerprint(&path)?));
            }
            store.remember_apply_hashes(&evidence)?;
        }
        store.clear_journal(lock, operation_id)
    })();
    // A late cleanup/checkpoint error cannot turn a committed operation into a rejection.
    if committed {
        return store.replay(operation_id, input)?.ok_or_else(|| {
            NativeError::new(
                "localJobOutcomeUnknown",
                "The committed local operation needs reconciliation.",
            )
        });
    }
    cleanup.map_err(|_| recovery())?;
    result
}

/// Write an already journaled immutable set. Callers admit their own exact
/// authority and own their journal/outcome/cleanup protocols.
pub(crate) fn write_immutable_copies(store: &Store, lock: &RootLock, expected: u64, operation_id: &str, input: &JsonValue,
    document: JsonValue, profile: &str, stage: &Path, copies: CopySet, packages: Vec<package::PackageRef>, journal: JsonValue,
    revalidate: &mut impl FnMut()->Result<(),NativeError>, checkpoint: &mut impl FnMut(&str)->Result<(),NativeError>) -> Result<JsonValue,NativeError> {
    write_immutable_copies_verified(store,lock,expected,operation_id,input,document,profile,stage,copies,packages,journal,&mut crate::remote::recovery::Verified::new(),revalidate,checkpoint)
}
pub(crate) fn write_immutable_copies_verified(store: &Store, lock: &RootLock, expected: u64, operation_id: &str, input: &JsonValue,
    document: JsonValue, profile: &str, stage: &Path, copies: CopySet, packages: Vec<package::PackageRef>, mut journal: JsonValue,
    verified_content:&mut crate::remote::recovery::Verified, revalidate: &mut impl FnMut()->Result<(),NativeError>, checkpoint: &mut impl FnMut(&str)->Result<(),NativeError>) -> Result<JsonValue,NativeError> {
    (|| {
        checkpoint("journal")?;
        for row in journal["files"].members() {
            let dest = destination(store.root(), row)?;
            paths::private_dir(dest.parent().ok_or_else(NativeError::params)?)?;
        }
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&stage)?;
        }
        paths::sync_dir(&crate::storage::root(store.root())?)?;
        checkpoint("stageCreatedUnrecorded")?;
        let meta = fs::metadata(&stage)?;
        journal["stageDevice"] = meta.dev().into();
        journal["stageInode"] = meta.ino().into();
        store.write_journal(lock, operation_id, &journal)?;
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stage.join("owner.json"))?;
        marker.write_all(journal["owner"].dump().as_bytes())?;
        marker.sync_all()?;
        paths::sync_dir(&stage)?;
        let meta = marker.metadata()?;
        journal["ownerDevice"] = meta.dev().into();
        journal["ownerInode"] = meta.ino().into();
        store.write_journal(lock, operation_id, &journal)?;
        checkpoint("stageRecorded")?;
        let mut verified = std::collections::BTreeMap::new();
        for (i, (f, path, _, fp)) in copies.values().enumerate() {
            let mut source = paths::open_regular(path)?;
            recheck_source(&mut source, path, fp)?;
            let dest = staged(&stage, &journal["files"][i])?;
            let mut out = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&dest)?;
            out.sync_all()?;
            paths::sync_dir(&stage)?;
            checkpoint("memberCreatedUnrecorded")?;
            let meta = out.metadata()?;
            journal["files"][i]["device"] = meta.dev().into();
            journal["files"][i]["inode"] = meta.ino().into();
            store.write_journal(lock, operation_id, &journal)?;
            checkpoint("memberRecorded")?;
            copy_exact(&mut source, &mut out, f.size, checkpoint)?;
            out.sync_all()?;
            let copied_metadata = out.metadata()?;
            if copied_metadata.len() != f.size
                || journal["files"][i]["device"].as_u64() != Some(copied_metadata.dev())
                || journal["files"][i]["inode"].as_u64() != Some(copied_metadata.ino()) {
                return Err(recovery());
            }
            let copied = paths::fingerprint_file(&mut out)?;
            if copied.sha256 != f.sha256
                || copied.size != f.size
            {
                return Err(NativeError::new(
                    "staleLocalSource",
                    "Copied bytes differ from the reviewed source.",
                ));
            }
            recheck_source(&mut source, path, fp)?;
            verified.insert(f.id.clone(), live_stamp(&out)?);
            journal["files"][i]["complete"] = true.into();
            store.write_journal(lock, operation_id, &journal)?;
            checkpoint("memberCopied")?;
        }
        for p in &packages {
            let directory =
                crate::vpk::inspect_directory(&stage.join(format!("{}.vpk", p.directory.id)))?;
            let companions = p
                .companions
                .iter()
                .map(|(i, f)| (*i, stage.join(format!("{}.vpk", f.id))))
                .collect();
            crate::vpk::validate_family(&directory, &companions)?;
        }
        revalidate()?;
        if journal["action"]=="attachOptions" {verify_dependencies(store,&journal)?;}
        store::ensure_readable(&document)?;
        checkpoint("sourcesRevalidated")?;
        for row in journal["files"].members() {
            let path = staged(&stage, row)?;
            let file=paths::open_regular(&path)?;
            if verified.get(row["id"].as_str().ok_or_else(recovery)?)!=Some(&live_stamp(&file)?) {return Err(stale_source());}
            paths::make_readonly(&path)?;
            let file=paths::open_regular(&path)?;file.sync_all()?;
            verified.insert(row["id"].as_str().ok_or_else(recovery)?.to_owned(),live_stamp(&file)?);
        }
        paths::sync_dir(&stage)?;
        for row in journal["files"].members() {
            let dest = destination(store.root(), row)?;
            let source=staged(&stage,row)?;
            if verified.get(row["id"].as_str().ok_or_else(recovery)?)!=Some(&live_stamp(&paths::open_regular(&source)?)?) {return Err(stale_source());}
            fs::hard_link(&source, &dest)?;
            let file=paths::open_regular(&source)?;
            verified_content.remember_verified(row["id"].as_str().ok_or_else(recovery)?,&file,row["sha256"].as_str().ok_or_else(recovery)?)?;
            // Link creation changes ctime; the verified stamp above and our own
            // link are the only authority for carrying the digest forward.
            paths::seed_hash(&source,&paths::content_stamp(&file)?,row["sha256"].as_str().ok_or_else(recovery)?)?;
            verified.insert(row["id"].as_str().ok_or_else(recovery)?.to_owned(),live_stamp(&file)?);
            paths::sync_dir(dest.parent().ok_or_else(NativeError::params)?)?;
            checkpoint("published")?;
        }
        for row in journal["files"].members() {
            let file=paths::open_regular(&destination(store.root(),row)?)?;
            if verified.get(row["id"].as_str().ok_or_else(recovery)?)!=Some(&live_stamp(&file)?) {return Err(stale_source());}
        }
        let result = store.commit_verified_payloads(lock, expected, operation_id, input, document, Some(profile),verified_content)?;
        checkpoint("database")?;
        Ok(result)
    })()
}

/// Stored provenance is metadata, never a source path for publication/recovery.
/// Only validated FileRefs identify native archive destinations.
pub(crate) fn stored_archive_refs(record: &JsonValue) -> Result<Vec<FileRef>, NativeError> {
    let mut refs = historical_archive_refs(record)?;
    for file in crate::options::owned_archive_refs(record)? {
        if let Some(existing) = refs.iter().find(|f| f.id == file.id) {
            // Reuse is the same complete native ref; attachment coordinates live
            // in a separate namespace and cannot replace historical provenance.
            if *existing != file { return Err(recovery()); }
        } else {
            refs.push(file);
        }
    }
    let bytes = refs.iter().try_fold(0u64, |n,f| n.checked_add(f.size).ok_or_else(recovery))?;
    if record.has_key("options") && bytes > 68_719_476_736 { return Err(recovery()); }
    Ok(refs)
}
pub(crate) fn historical_archive_refs(record: &JsonValue) -> Result<Vec<FileRef>, NativeError> {
    let local = &record["source"]["localImport"];
    if local["archives"].is_null() {
        if local["version"] == 2 { return Err(recovery()); }
        return Ok(Vec::new());
    }
    if record["source"]["kind"] != "local" || !(local["version"] == 2 || local["version"] == 3)
        || !local["archives"].is_array() || local["archives"].is_empty() {
        return Err(recovery());
    }
    let mut ids = HashSet::new();
    let mut coordinates = HashSet::new();
    let mut refs = Vec::new();
    for a in local["archives"].members() {
        let f = package::decode_file(&a["file"]).map_err(|_| recovery())?;
        let ordinal = a["rootOrdinal"].as_usize().filter(|n| *n < 64).ok_or_else(recovery)?;
        let relative = a["relativePath"].as_str().filter(|s| !s.is_empty()).ok_or_else(recovery)?;
        let original = Path::new(a["originalPath"].as_str().ok_or_else(recovery)?);
        if !ids.insert(f.id.clone()) || !coordinates.insert((ordinal, relative.to_owned()))
            || Path::new(relative).components().any(|c| !matches!(c, std::path::Component::Normal(_)))
            || !original.is_absolute() || original.file_name().and_then(|s| s.to_str()) != Some(f.source_name.as_str())
            || !a["format"].as_str().is_some_and(|s| matches!(s, "Zip" | "7-Zip" | "RAR" | "RAR 5")) {
            return Err(recovery());
        }
        refs.push(f);
    }
    Ok(refs)
}
pub(crate) fn record_files(record: &JsonValue, version: u32) -> Result<Vec<(FileRef, &'static str)>, NativeError> {
    let mut refs = Vec::new();
    for p in crate::options::retained_packages(record, version)? {
        refs.push((p.directory, "payload"));
        refs.extend(p.companions.into_values().map(|f| (f, "payload")));
    }
    if !record["source"]["localImport"]["artwork"].is_null() {
        refs.push((package::decode_file(&record["source"]["localImport"]["artwork"])?, "artwork"));
    }
    refs.extend(stored_archive_refs(record)?.into_iter().map(|f| (f, "archive")));
    refs.extend(crate::fonts::stored_refs(record)?.into_iter().map(|f|(f.asset,"font")));
    Ok(refs)
}
/// Profile clones may share immutable files; distinct storage roles may never
/// share an ID. Package Apply ownership continues to use only mod_packages().
pub(crate) fn validate_document_files(doc: &JsonValue) -> Result<(), NativeError> {
    let version = package::layout_version(doc)?;
    let mut ids = BTreeMap::new();
    let mut names = BTreeMap::new();
    let mut ambiguous_names = HashSet::new();
    let mut catalog_ids = HashSet::new();
    let mut font_mods = BTreeMap::new();
    let mut occurrences = Vec::new();
    let mut font_owners = BTreeMap::new();
    for (profile_id, profile) in doc["profiles"].entries() {
        for record in profile["mods"].members() {
            let local=&record["source"]["localImport"];
            if !local.is_null() && !matches!(local["version"].as_u32(),Some(1|2|3)) {return Err(recovery());}
            let fonts=crate::fonts::stored_refs(record)?;
            crate::fonts::diagnostics(record)?;
            // Keep empty sets too: an ID may become font-bearing in a later
            // record/profile. Entirely legacy IDs retain their existing rules.
            if let Some(mod_id)=record["source"]["id"].as_str() {
                occurrences.push((profile_id,mod_id,&local["fonts"]));
            }
            if !fonts.is_empty() {
                if !matches!(doc["storeVersion"].as_u32(),Some(3|4)) {return Err(recovery());}
                let mod_id=record["source"]["id"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
                // A profile copy preserves the same mod/asset identity and full ref set.
                if font_mods.insert(mod_id,local["fonts"].clone()).is_some_and(|old|old!=local["fonts"]) {return Err(recovery());}
                for font in fonts {
                    if font_owners.insert(font.asset.id,mod_id).is_some_and(|old|old!=mod_id) {return Err(recovery());}
                }
            }
            for (f, role) in record_files(record, version)? {
                if record.has_key("options") {catalog_ids.insert(f.id.clone());}
                if names.insert(f.id.clone(),f.source_name.clone()).is_some_and(|old|old!=f.source_name) {
                    ambiguous_names.insert(f.id.clone());
                }
                let value = (f.sha256, f.size, role);
                if ids.insert(f.id, value.clone()).is_some_and(|old| old != value) {
                    return Err(recovery());
                }
            }
            if record.has_key("options") {
                // Per-record role and copy limits include inactive payloads and
                // the historical artwork/fonts, not only the selected subset.
                let mut unique = BTreeMap::new();
                for (f,role) in record_files(record,version)? {
                    let value=(f.clone(),role);
                    if unique.insert(f.id.clone(),value.clone()).is_some_and(|old|old!=value) {return Err(recovery());}
                }
                let bytes = |role: &str| unique.values().filter(|(_,r)|*r==role).try_fold(0u64,|n,(f,_)|n.checked_add(f.size).ok_or_else(recovery));
                let total = unique.values().try_fold(0u64,|n,(f,_)|n.checked_add(f.size).ok_or_else(recovery))?;
                if bytes("payload")?>137438953472 || bytes("archive")?>68719476736 || bytes("font")?>134217728 || total>137589948416 {return Err(recovery());}
            }
        }
    }
    for f in crate::font_transaction::backup_refs(doc)? {
        if names.insert(f.id.clone(),f.source_name.clone()).is_some_and(|old|old!=f.source_name) {
            ambiguous_names.insert(f.id.clone());
        }
        let value=(f.sha256,f.size,"fontBackup");
        if ids.insert(f.id,value.clone()).is_some_and(|old|old!=value){return Err(recovery());}
    }
    // Old imports admitted content aliases with different source names. Keep
    // their original read contract, but no catalog may reuse an ambiguous ref.
    if catalog_ids.iter().any(|id|ambiguous_names.contains(id)) {return Err(recovery());}
    let mut profile_font_mods=HashSet::new();
    for (profile_id,mod_id,font_set) in occurrences {
        if let Some(expected)=font_mods.get(mod_id) {
            if font_set!=expected || !profile_font_mods.insert((profile_id,mod_id)) {return Err(recovery());}
        }
    }
    Ok(())
}
pub(crate) fn reject_existing_claims(doc: &JsonValue, version: u32, ids: &HashSet<String>,
                          committed_record: Option<(&str, &JsonValue)>) -> Result<(), NativeError> {
    if crate::font_transaction::backup_refs(doc)?.iter().any(|f|ids.contains(&f.id)){return Err(recovery());}
    let mut skipped = false;
    for (profile_id, profile) in doc["profiles"].entries() {
        for existing in profile["mods"].members() {
            if !skipped && committed_record.is_some_and(|(id, record)| id == profile_id && existing == record) {
                skipped = true;
                continue;
            }
            if record_files(existing, version)?.iter().any(|(f, _)| ids.contains(&f.id)) {
                return Err(recovery());
            }
        }
    }
    if committed_record.is_some() && !skipped { return Err(recovery()); }
    Ok(())
}

/// Read-only old-format admission shared by namespace/cleanup owners. This must
/// run before private jobs may persist observations or release quarantine data.
pub(crate) fn admit_legacy_record(body: &JsonValue) -> Result<(u32, u32, u32), NativeError> {
    for key in ["document","beforeDocument","requestedDocument"] {crate::generation::require_flat_document(&body[key])?;}
    let outer = body["version"].as_u32().filter(|v| matches!(v, 3 | 4 | 5)).ok_or_else(recovery)?;
    if body["kind"] != "import" || body["owner"]["kind"] != "import"
        || body["owner"]["version"].as_u32() != Some(outer)
        || !body["input"].is_object() || !body["files"].is_array()
        || body.has_key("libraryVersion") || body["record"].has_key("options") {
        return Err(recovery());
    }
    let (layout, before, after) = if outer == 5 {
        let layout = body["packageVersion"].as_u32().filter(|v| matches!(v, 1 | 2)).ok_or_else(recovery)?;
        let before = body["libraryVersionBefore"].as_u32().filter(|v| matches!(v, 1 | 2 | 3)).ok_or_else(recovery)?;
        let after = body["libraryVersionAfter"].as_u32().filter(|v| *v == 3).ok_or_else(recovery)?;
        // Omitted or null storeVersion was admitted by the original v5 decoder.
        if !body["storeVersion"].is_null() || before != 3 && before != layout {return Err(recovery());}
        (layout, before, after)
    } else {
        let layout = body["storeVersion"].as_u32().filter(|v| matches!(v, 1 | 2)).ok_or_else(recovery)?;
        // These fields belong to v5, not to raw3/archive4. Reject explicit
        // malformed values as well as attempts to smuggle a later meaning in.
        if ["packageVersion", "libraryVersionBefore", "libraryVersionAfter"].iter().any(|key| body.has_key(key)) {
            return Err(recovery());
        }
        (layout, layout, layout)
    };
    package::legacy_packages(&body["record"], layout).map_err(|_| recovery())?;
    Ok((layout, before, after))
}

/// Exact import6/7 addMod admission, shared by the early namespace gate and
/// filesystem cleanup. Other actions need their own decoder and authority.
pub(crate) fn admit_add_record(body: &JsonValue) -> Result<(u32,u32,u32), NativeError> {
    crate::deployment_metadata::domain(&body["beforeDocument"],body["version"]==7)?;
    fn keys(v: &JsonValue, fields: &[&str]) -> Result<(),NativeError> {
        if !v.is_object() || v.len()!=fields.len() || fields.iter().any(|k|!v.has_key(k)) {return Err(recovery());}
        Ok(())
    }
    keys(body,&["kind","version","action","operationId","input","expectedRevision","targetProfileId",
        "beforeDigest","afterDigest","beforeDocument","libraryVersionBefore","libraryVersionAfter","packageVersion",
        "modId","record","stage","owner","stageDevice","stageInode","ownerDevice","ownerInode","files","reusedFiles"])?;
    owner_keys(&body["owner"],&["kind","version","action","operationId","stage","nonce","authorityDigest"])?;
    let layout=body["packageVersion"].as_u32().filter(|v|matches!(v,1|2)).ok_or_else(recovery)?;
    let before_version=body["libraryVersionBefore"].as_u32().filter(|v|matches!(v,1|2|3|4)).ok_or_else(recovery)?;
    let before=&body["beforeDocument"];
    let record=&body["record"];
    let profile=body["targetProfileId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let mod_id=body["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let op=body["operationId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let stage=body["stage"].as_str().filter(|s|s.strip_prefix(".import-local-").is_some_and(|id|package::hex(id,32))).ok_or_else(recovery)?;
    let expected=store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?;
    if body["kind"]!="import" || !matches!(body["version"].as_u32(),Some(6|7)) || body["action"]!="addMod"
        || body["owner"]["kind"]!="import" || body["owner"]["version"]!=body["version"]
        || body["owner"]["action"]!="addMod" || body["owner"]["stage"]!=stage || body["owner"]["operationId"]!=op
        || !body["owner"]["nonce"].as_str().is_some_and(|id|package::hex(id,32))
        || body["owner"]["authorityDigest"]!=authority_digest(body)?
        || body["libraryVersionAfter"].as_u32()!=Some(4) || !body["input"].is_object()
        || !body["files"].is_array() || !body["reusedFiles"].is_array() || !body["reusedFiles"].is_empty()
        || package::document_version(before)?!=before_version || package::layout_version(before)?!=layout
        || before["revision"]!=expected.to_string() || !before["profiles"][profile]["mods"].is_array()
        || body["beforeDigest"]!=paths::digest_bytes(before.dump().as_bytes())?
        || record["source"]["id"]!=mod_id || record["source"]["kind"]!="local" || record["enabled"]!=false
        || before["profiles"].entries().any(|(_,p)|p["mods"].members().any(|m|m["source"]["id"]==mod_id)) {
        return Err(recovery());
    }
    for (a,b) in [("stageDevice","stageInode"),("ownerDevice","ownerInode")] {
        if !((body[a].is_null() && body[b].is_null()) || (body[a].as_u64().is_some() && body[b].as_u64().is_some())) {return Err(recovery());}
    }
    if !body["ownerDevice"].is_null() && body["stageDevice"].is_null() {return Err(recovery());}
    let catalog=crate::options::for_record(record,layout)?;
    if !record.has_key("options") || !catalog.attachments.is_empty() || !catalog.retained_archives.is_empty() {return Err(recovery());}
    catalog.validate_selection_save()?;
    let mut projected=before.clone();
    projected["storeVersion"]=4.into(); projected["packageVersion"]=layout.into();
    if before_version<3 {projected["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
    if let Some(game) = crate::installed::initial_game_path(before,record)? { projected["gamePath"] = game.into(); }
    projected["profiles"][profile]["mods"].push(record.clone()).map_err(|_|recovery())?;
    projected["revision"]=expected.checked_add(1).ok_or_else(recovery)?.to_string().into();
    if body["afterDigest"]!=paths::digest_bytes(projected.dump().as_bytes())? {return Err(recovery());}
    store::ensure_readable(before)?;
    store::ensure_readable(&projected)?;
    store::ensure_internal(body)?;
    store::ensure_internal(&object!{operationId:op,input:body["input"].clone(),outcome:object!{status:"succeeded",result:store::snapshot_document(&projected,Some(profile))?}})?;
    let mut refs=BTreeMap::new();
    for (file,role) in record_files(record,layout)? {
        let binding=(file.clone(),role);
        if refs.insert(file.id.clone(),binding.clone()).is_some_and(|old|old!=binding) {return Err(recovery());}
    }
    reject_existing_claims(before,layout,&refs.keys().cloned().collect(),None)?;
    let mut seen=HashSet::new();
    for row in body["files"].members() {
        keys(row,&["id","sha256","size","sourceName","role","device","inode","complete"])?;
        let file=package::decode_file(row)?;
        let role=row["role"].as_str().ok_or_else(recovery)?;
        if !seen.insert(file.id.clone()) || refs.get(&file.id)!=Some(&(file,role)) || !row["complete"].is_boolean()
            || !row["device"].is_null() && body["ownerDevice"].is_null()
            || !((row["device"].is_null() && row["inode"].is_null() && row["complete"]==false)
                || (row["device"].as_u64().is_some() && row["inode"].as_u64().is_some())) {return Err(recovery());}
    }
    if seen.len()!=refs.len() {return Err(recovery());}
    let local=&record["source"]["localImport"];
    if !local["candidates"].is_array() || local["candidates"].len()>1024 || !local["selectedCandidateIds"].is_array() {return Err(recovery());}
    let selected=local["selectedCandidateIds"].members().map(|id|id.as_str().filter(|id|package::hex(id,64)).ok_or_else(recovery))
        .collect::<Result<Vec<_>,_>>()?;
    if selected.is_empty() || selected.len()>99 || selected.windows(2).any(|w|w[0]>=w[1]) {return Err(recovery());}
    let candidates=local["candidates"].members().filter(|c|c["available"]==true).collect::<Vec<_>>();
    if candidates.len()!=catalog.entries.len() {return Err(recovery());}
    let mut initial_logical=0u64;
    let mut candidate_ids=HashSet::new();
    for (candidate,entry) in candidates.iter().zip(&catalog.entries) {
        let candidate_id=candidate["id"].as_str().filter(|id|package::hex(id,64)).ok_or_else(recovery)?;
        let package=entry.package.as_ref().ok_or_else(recovery)?;
        let initially_selected=selected.binary_search(&candidate_id).is_ok();
        if !candidate_ids.insert(candidate_id) || candidate["optionName"]!=entry.option_name || candidate["relativePath"]!=entry.source_label
            || initially_selected!=catalog.selected_ids.binary_search(&entry.id).is_ok()
            || candidate["members"].len()!=1+package.companions.len() {return Err(recovery());}
        for ((index,file),member) in std::iter::once((None,&package.directory))
            .chain(package.companions.iter().map(|(i,f)|(Some(*i),f))).zip(candidate["members"].members()) {
            if member["archiveIndex"]!=index.map(JsonValue::from).unwrap_or(JsonValue::Null)
                || member["sourceName"]!=file.source_name || member["sha256"]!=file.sha256 || member["size"].as_u64()!=Some(file.size) {return Err(recovery());}
            if initially_selected {initial_logical=initial_logical.checked_add(file.size).ok_or_else(recovery)?;}
        }
    }
    if selected.iter().any(|id|!candidate_ids.contains(id)) || local["logicalPackageBytes"].as_u64()!=Some(initial_logical) {return Err(recovery());}
    let detail=crate::options::detail(record,layout)?;
    let totals=&detail["totals"];
    for (field,max) in [("uniquePayloadBytes",137438953472u64),("retainedArchiveBytes",68719476736),
        ("artworkBytes",16777216),("fontBytes",134217728),("uniqueCopyBytes",137589948416)] {
        if !totals[field].as_u64().is_some_and(|n|n<=max) {return Err(recovery());}
    }
    let archives=stored_archive_refs(record)?;
    let fonts=crate::fonts::stored_refs(record)?;
    if local["version"]!=if !fonts.is_empty(){3}else if archives.is_empty(){1}else{2} {return Err(recovery());}
    for (history,total) in [("retainedLogicalPackageBytes","retainedLogicalBytes"),("uniquePayloadBytes","uniquePayloadBytes"),
        ("retainedArchiveBytes","retainedArchiveBytes"),("uniqueCopyBytes","uniqueCopyBytes"),("retainedPackageCount","retainedFamilies"),("storedPackageCount","activeFamilies")] {
        if local[history]!=totals[total] {return Err(recovery());}
    }
    if local["selectedPackageCount"].as_usize()!=Some(catalog.selected_ids.len())
        || local["inactiveOptionCount"].as_usize()!=Some(catalog.entries.len()-catalog.selected_ids.len())
        || local["selectedLogicalPackageBytes"]!=local["logicalPackageBytes"]
        || !local["logicalPackageBytes"].as_u64().is_some_and(|n|n<=137438953472)
        || (!local["uniqueFontBytes"].is_null() || totals["fontBytes"]!=0) && local["uniqueFontBytes"]!=totals["fontBytes"] {return Err(recovery());}
    Ok((layout,before_version,4))
}
/// Exact one-record replacement; all other profile copies remain byte-for-byte.
fn project_attachment(body: &JsonValue) -> Result<JsonValue,NativeError> {
    let before=&body["beforeDocument"];
    let profile=body["targetProfileId"].as_str().ok_or_else(recovery)?;
    let mod_id=body["modId"].as_str().ok_or_else(recovery)?;
    let indexes=before["profiles"][profile]["mods"].members().enumerate().filter(|(_,r)|r["source"]["id"]==mod_id).map(|(i,_)|i).collect::<Vec<_>>();
    if indexes.len()!=1 || before["profiles"][profile]["mods"][indexes[0]]!=body["beforeRecord"] {return Err(recovery());}
    let mut after=before.clone(); after["storeVersion"]=4.into();after["packageVersion"]=body["packageVersion"].clone();
    if package::document_version(before)?<3 {after["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
    after["profiles"][profile]["mods"][indexes[0]]=body["record"].clone();
    after["revision"]=store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
    Ok(after)
}
fn exact_keys(value: &JsonValue, keys: &[&str]) -> Result<(),NativeError> {
    if !value.is_object() || value.len()!=keys.len() || keys.iter().any(|k|!value.has_key(k)) {return Err(recovery());} Ok(())
}
fn owner_keys(value:&JsonValue,keys:&[&str])->Result<(),NativeError>{
    let mut fields=keys.to_vec();
    if value.has_key("storageFsid"){fields.push("storageFsid");if !value["storageFsid"].as_str().is_some_and(|s|package::hex(s,16)){return Err(recovery());}}
    exact_keys(value,&fields)
}
/// This decoder grants fresh-write authority independently of addMod.
pub(crate) fn admit_attachment_record(body: &JsonValue) -> Result<(u32,u32,u32),NativeError> {
    crate::deployment_metadata::domain(&body["beforeDocument"],body["version"]==7)?;
    exact_keys(body,&["kind","version","action","operationId","input","expectedRevision","targetProfileId","modId",
        "beforeDigest","afterDigest","beforeDocument","beforeRecord","record","libraryVersionBefore","libraryVersionAfter","packageVersion",
        "stage","owner","stageDevice","stageInode","ownerDevice","ownerInode","files","reusedFiles"])?;
    owner_keys(&body["owner"],&["kind","version","action","operationId","stage","nonce","authorityDigest"])?;
    let layout=body["packageVersion"].as_u32().filter(|v|matches!(v,1|2)).ok_or_else(recovery)?;
    let before_version=body["libraryVersionBefore"].as_u32().filter(|v|matches!(v,1|2|3|4)).ok_or_else(recovery)?;
    let before=&body["beforeDocument"]; let record=&body["record"]; let prior=&body["beforeRecord"];
    let profile=body["targetProfileId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let mod_id=body["modId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let op=body["operationId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(recovery)?;
    let stage=body["stage"].as_str().filter(|s|s.strip_prefix(".import-local-").is_some_and(|id|package::hex(id,32))).ok_or_else(recovery)?;
    let expected=store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?;
    if body["kind"]!="import" || !matches!(body["version"].as_u32(),Some(6|7)) || body["action"]!="attachOptions" || body["libraryVersionAfter"]!=4
        || body["owner"]["kind"]!="import" || body["owner"]["version"]!=body["version"] || body["owner"]["action"]!="attachOptions"
        || body["owner"]["operationId"]!=op || body["owner"]["stage"]!=stage
        || !body["owner"]["nonce"].as_str().is_some_and(|id|package::hex(id,32))
        || body["owner"]["authorityDigest"]!=authority_digest(body)? || !body["input"].is_object()
        || !body["files"].is_array() || body["files"].is_empty() || !body["reusedFiles"].is_array()
        || package::document_version(before)?!=before_version || package::layout_version(before)?!=layout
        || before["revision"]!=expected.to_string() || body["beforeDigest"]!=paths::digest_bytes(before.dump().as_bytes())?
        || prior["source"]["id"]!=mod_id || record["source"]["id"]!=mod_id {return Err(recovery());}
    exact_keys(&body["input"],&["method","params"])?;
    let params=&body["input"]["params"];
    exact_keys(params,&["operationId","expectedRevision","profileId","modId","jobId","token"])?;
    if body["input"]["method"]!="native.options.attach" || params["operationId"]!=op || params["expectedRevision"]!=expected.to_string()
        || params["profileId"]!=profile || params["modId"]!=mod_id
        || !params["jobId"].as_str().is_some_and(|id|package::hex(id,32))
        || !params["token"].as_str().is_some_and(|id|package::hex(id,32)) {return Err(recovery());}
    for (a,b) in [("stageDevice","stageInode"),("ownerDevice","ownerInode")] {
        if !((body[a].is_null() && body[b].is_null()) || (body[a].as_u64().is_some() && body[b].as_u64().is_some())) {return Err(recovery());}
    }
    if !body["ownerDevice"].is_null() && body["stageDevice"].is_null() {return Err(recovery());}
    // The only legacy representation migration is packages/payloads -> options.
    let mut old_metadata=prior.clone(); let mut new_metadata=record.clone();
    for k in ["options","packages","payloads"] {old_metadata.remove(k);}
    new_metadata.remove("options");
    if old_metadata!=new_metadata || !record.has_key("options") {return Err(recovery());}
    let old=crate::local_content::option_sources::catalog_for_record(prior,layout)?;
    let new=crate::options::for_record(record,layout)?;
    if old.selected_ids!=new.selected_ids || old.entries.len()!=new.entries.len()
        || !new.attachments.starts_with(&old.attachments) || !new.retained_archives.starts_with(&old.retained_archives) {return Err(recovery());}
    let mut attached=0;
    for (a,b) in old.entries.iter().zip(&new.entries) {
        if a==b {continue;}
        if a.package.is_some() || b.package.is_none() || a.expected_family.is_none()
            || a.id!=b.id || a.option_name!=b.option_name || a.source_label!=b.source_label || a.expected_family!=b.expected_family
            || b.unavailable_reason.is_some() {return Err(recovery());}
        // Raw exact rebindings preserve the old archive evidence; a newly retained
        // archive may supply a new exact binding without changing the expected family.
        if let Some(binding)=&b.archive_binding {
            if a.archive_binding.as_ref()!=Some(binding) && !new.retained_archives[old.retained_archives.len()..].iter().any(|r|r.file==binding.file) {return Err(recovery());}
        } else if a.archive_binding.is_some() {return Err(recovery());}
        attached+=1;
    }
    if attached==0 {return Err(recovery());}
    let added_archives=&new.retained_archives[old.retained_archives.len()..];
    let added_attachments=&new.attachments[old.attachments.len()..];
    if added_archives.is_empty() {
        if !added_attachments.is_empty() {return Err(recovery());}
    } else {
        if added_attachments.len()!=1 || added_attachments[0].source_kind!="chosenRoots" {return Err(recovery());}
        for archive in added_archives {
            if archive.attachment_id!=added_attachments[0].id
                || !old.entries.iter().zip(&new.entries).any(|(a,b)|a.package.is_none() && b.package.is_some()
                    && b.archive_binding.as_ref().is_some_and(|binding|binding.file==archive.file)) {return Err(recovery());}
        }
    }
    let refs=|record: &JsonValue|->Result<BTreeMap<String,(FileRef,&'static str)>,NativeError> {
        let mut out=BTreeMap::new(); for (file,role) in record_files(record,layout)? {
            if out.insert(file.id.clone(),(file.clone(),role)).is_some_and(|v|v!=(file,role)) {return Err(recovery());}
        } Ok(out)
    };
    let prior_refs=refs(prior)?; let after_refs=refs(record)?;
    if prior_refs.iter().any(|(id,f)|after_refs.get(id)!=Some(f)) {return Err(recovery());}
    let mut seen=HashSet::new(); let mut fresh=HashSet::new();
    for row in body["reusedFiles"].members() {
        exact_keys(row,&["id","sha256","size","sourceName","role"])?;
        let f=package::decode_file(row)?;let role=row["role"].as_str().ok_or_else(recovery)?;
        if !seen.insert(f.id.clone()) || prior_refs.get(&f.id)!=Some(&(f.clone(),role)) || after_refs.get(&f.id)!=Some(&(f,role)) {return Err(recovery());}
    }
    if seen.len()!=prior_refs.len() {return Err(recovery());}
    for row in body["files"].members() {
        exact_keys(row,&["id","sha256","size","sourceName","role","device","inode","complete"])?;
        let f=package::decode_file(row)?;let role=row["role"].as_str().filter(|r|matches!(*r,"payload"|"archive")).ok_or_else(recovery)?;
        if !seen.insert(f.id.clone()) || !fresh.insert(f.id.clone()) || after_refs.get(&f.id)!=Some(&(f,role)) || !row["complete"].is_boolean()
            || !row["device"].is_null() && body["ownerDevice"].is_null()
            || !((row["device"].is_null() && row["inode"].is_null() && row["complete"]==false)
                || (row["device"].as_u64().is_some() && row["inode"].as_u64().is_some())) {return Err(recovery());}
    }
    if seen.len()!=after_refs.len() {return Err(recovery());}
    reject_existing_claims(before,layout,&fresh,None)?;
    let after=project_attachment(body)?;
    if body["afterDigest"]!=paths::digest_bytes(after.dump().as_bytes())? {return Err(recovery());}
    store::ensure_readable(before)?;store::ensure_readable(&after)?;store::ensure_internal(body)?;
    crate::local_content::option_sources::detail_envelope(record,layout,profile,expected+1)?;
    store::ensure_internal(&object!{operationId:op,input:body["input"].clone(),outcome:object!{status:"succeeded",result:store::snapshot_document(&after,Some(profile))?}})?;
    Ok((layout,before_version,4))
}
fn group_records(body:&JsonValue)->Vec<&JsonValue> {
    if body["action"]=="addMods" {body["records"].members().collect()} else {vec![&body["record"]]}
}
fn admit_group(body:&JsonValue)->Result<(u32,u32,u32),NativeError> {
    if !body["records"].is_array() || body["records"].is_empty() || body["records"].len()>64 || body.has_key("record") || body.has_key("modId")
        || body["owner"]["action"]!="addMods" || body["owner"]["authorityDigest"]!=authority_digest(body)? {return Err(recovery());}
    let mut before=body["beforeDocument"].clone();let original=before.clone();
    let profile=body["targetProfileId"].as_str().ok_or_else(recovery)?;
    let layout=package::layout_version(&before)?;let mut seen=HashSet::new();let mut bytes=0u64;
    for record in body["records"].members() {
        let refs=record_files(record,layout)?;
        let ids=refs.iter().map(|(f,_)|f.id.clone()).collect::<HashSet<_>>();
        if ids.iter().any(|id|!seen.insert(id.clone())) {return Err(recovery());}
        let mut child=body.clone();child.remove("records");child["action"]="addMod".into();child["owner"]["action"]="addMod".into();
        child["record"]=record.clone();child["modId"]=record["source"]["id"].clone();child["beforeDocument"]=before.clone();
        child["libraryVersionBefore"]=package::document_version(&before)?.into();child["beforeDigest"]=paths::digest_bytes(before.dump().as_bytes())?.into();
        child["files"]=body["files"].members().filter(|f|f["id"].as_str().is_some_and(|id|ids.contains(id))).cloned().collect::<Vec<_>>().into();
        for f in child["files"].members() {bytes=bytes.checked_add(f["size"].as_u64().ok_or_else(recovery)?).ok_or_else(recovery)?;}
        before["storeVersion"]=4.into();before["packageVersion"]=layout.into();
        if package::document_version(&original)?<3 {before["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
        if let Some(game)=crate::installed::initial_game_path(&before,record)? {before["gamePath"]=game.into();}
        before["profiles"][profile]["mods"].push(record.clone()).map_err(|_|recovery())?;
        before["revision"]=(store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?+1).to_string().into();
        child["afterDigest"]=paths::digest_bytes(before.dump().as_bytes())?.into();child["owner"]["authorityDigest"]=authority_digest(&child)?.into();
        admit_add_record(&child)?;
        before["revision"]=body["expectedRevision"].clone();
    }
    before["revision"]=(store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?+1).to_string().into();
    if body["files"].len()!=seen.len() || bytes>137438953472 || body["beforeDigest"]!=paths::digest_bytes(original.dump().as_bytes())? || body["afterDigest"]!=paths::digest_bytes(before.dump().as_bytes())? {return Err(recovery());}
    Ok((layout,package::document_version(&original)?,4))
}
fn reject_group_claims(doc:&JsonValue,body:&JsonValue,committed:bool)->Result<(),NativeError> {
    let mut others=doc.clone();
    if committed {
        let profile=body["targetProfileId"].as_str().ok_or_else(recovery)?;
        let records=group_records(body);
        let kept=others["profiles"][profile]["mods"].members().filter(|m|!records.contains(m)).cloned().collect::<Vec<_>>();
        if kept.len()+records.len()!=others["profiles"][profile]["mods"].len() {return Err(recovery());}
        others["profiles"][profile]["mods"]=kept.into();
    }
    let ids=body["files"].members().map(|f|f["id"].as_str().map(str::to_owned).ok_or_else(recovery)).collect::<Result<HashSet<_>,_>>()?;
    reject_existing_claims(&others,package::layout_version(doc)?,&ids,None)
}
pub(crate) fn admit_modern_record(body: &JsonValue) -> Result<(u32,u32,u32),NativeError> {
    match body["action"].as_str() {Some("addMods")=>admit_group(body),Some("addMod")=>admit_add_record(body),Some("attachOptions")=>admit_attachment_record(body),_=>Err(recovery())}
}
fn verify_dependencies(store: &Store, body: &JsonValue) -> Result<(),NativeError> {
    for row in body["reusedFiles"].members() {
        let file=package::decode_file(row)?; let path=destination(store.root(),row)?;
        let mut descriptor=paths::open_regular(&path)?;
        if descriptor.metadata()?.len()!=file.size {return Err(recovery());}
        let fp=paths::fingerprint_file(&mut descriptor)?;
        if fp.size!=file.size || fp.sha256!=file.sha256 {return Err(recovery());}
        let now=paths::open_regular(&path)?.metadata()?;
        if now.dev()!=fp.device || now.ino()!=fp.inode || now.len()!=fp.size {return Err(recovery());}
    } Ok(())
}
pub(crate) fn commit_attachment(plan: &crate::local_content::option_sources::OptionAttachmentPlan, lock: &RootLock,
    expected: u64, operation_id: &str, input: &JsonValue) -> Result<JsonValue,NativeError> {
    let store=Store::open_writer(lock)?;
    if let Some(result)=store.replay(operation_id,input)? {return Ok(result);}
    if lock.root()!=plan.prepared.request.root || expected!=plan.prepared.request.expected_revision {return Err(NativeError::params());}
    crate::apply::ensure_idle(&store)?;
    let profile=&plan.prepared.request.profile_id; let mod_id=plan.before_record["source"]["id"].as_str().ok_or_else(recovery)?;
    let (current,layout)=crate::local_content::option_sources::pinned_record(store.root(),profile,mod_id,expected)?;
    if current!=plan.before_record {return Err(NativeError::new("stalePreview","The pinned record changed; prepare again."));}
    let before=store.document()?;
    let mut copies=CopySet::new();
    if plan.sources.len()!=plan.packages.len() || plan.archives.len()!=plan.archive_refs.len() {return Err(recovery());}
    for (source,package) in plan.sources.iter().zip(&plan.packages) {
        if layout==1 && (!package.companions.is_empty() || crate::vpk::inspect(&source.directory.source_path).is_err()) {return Err(NativeError::new("unsupportedLocalPackageForStore","This legacy layout requires a compatible standalone VPK."));}
        for (file,source) in std::iter::once((&package.directory,&source.directory)).chain(package.companions.iter().map(|(i,f)|(f,&source.companions[i]))) {
            copies.entry(file.id.clone()).or_insert((file.clone(),source.source_path.clone(),"payload".into(),plan.prepared.fingerprint_for(&source.source_path)?));
        }
    }
    if plan.owned.is_none() {for (archive,file) in plan.archives.iter().zip(&plan.archive_refs) {
        if copies.insert(file.id.clone(),(file.clone(),archive.snapshot_path().into(),"archive".into(),archive.snapshot_fingerprint().clone())).is_some() {return Err(recovery());}
    }}
    for (file,_,_,fp) in copies.values() {if file.size!=fp.size || file.sha256!=fp.sha256 {return Err(recovery());}}
    let mut reused=BTreeMap::new();
    for (file,role) in &plan.dependencies {let mut row=package::encode_file(file);row["role"]=(*role).into();reused.insert(file.id.clone(),row);}
    let files=copies.values().map(|(file,_,role,_)| {let mut row=package::encode_file(file);row["role"]=role.clone().into();row["device"]=JsonValue::Null;row["inode"]=JsonValue::Null;row["complete"]=false.into();row}).collect::<Vec<_>>();
    let stage_name=format!(".import-local-{}",paths::random_id()?); let stage=crate::storage::path(store.root(), &stage_name)?;
    let outer=if crate::deployment_metadata::retained_shards(&before) {7} else {6};
    let mut journal=object!{kind:"import",version:outer,action:"attachOptions",operationId:operation_id,input:input.clone(),expectedRevision:expected.to_string(),targetProfileId:profile.clone(),modId:mod_id,
        libraryVersionBefore:package::document_version(&before)?,libraryVersionAfter:4,packageVersion:layout,beforeDocument:before.clone(),beforeRecord:plan.before_record.clone(),record:plan.record.clone(),
        beforeDigest:paths::digest_bytes(before.dump().as_bytes())?,afterDigest:JsonValue::Null,stage:stage_name.clone(),
        owner:object!{kind:"import",version:outer,action:"attachOptions",operationId:operation_id,stage:stage_name,nonce:paths::random_id()?,authorityDigest:JsonValue::Null},
        stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,
        files:JsonValue::from(files),reusedFiles:JsonValue::from(reused.into_values().collect::<Vec<_>>())};
    journal["owner"]["storageFsid"]=paths::persisted_filesystem_id(&fs::File::open(crate::storage::root(store.root())?)?)?.into();
    let document=project_attachment(&journal)?;
    journal["afterDigest"]=paths::digest_bytes(document.dump().as_bytes())?.into();
    journal["owner"]["authorityDigest"]=authority_digest(&journal)?.into();
    admit_attachment_record(&journal)?;
    verify_dependencies(&store,&journal).map_err(|_|NativeError::new("optionDependencyChanged","An existing retained payload, archive, artwork or font is missing or changed. Attachment preserves it and cannot repair it; restore the exact owned asset before retrying."))?;
    for row in journal["files"].members() {paths::verify_absent(&destination(store.root(),row)?)?;}
    execute_publication(&store,lock,expected,operation_id,input,document,profile,&stage,copies,plan.packages.clone(),journal,
        &mut || crate::local_content::option_sources::revalidate(&plan.prepared,plan.owned.as_ref(),&PrepareControl::never_cancelled()),&mut |_|Ok(()))
}

/// New allocations may not alias any other live owner, even a profile copy.
pub(crate) fn admit_modern_claims(store: &Store, body: &JsonValue) -> Result<(),NativeError> {
    let (layout,before_version,after_version)=admit_modern_record(body)?;
    let doc=store.document()?;
    crate::deployment_metadata::domain(&doc,body["version"]==7)?;
    crate::deployment_metadata::admit(store.root(),&body["beforeDocument"])?;
    let committed=store.operation(body["operationId"].as_str().ok_or_else(recovery)?)?["status"]=="succeeded";
    if package::document_version(&doc)?!=if committed {after_version}else{before_version}
        || package::layout_version(&doc)?!=layout
        || if committed {body["afterDigest"]!=paths::digest_bytes(doc.dump().as_bytes())?} else {body["beforeDocument"]!=doc} {return Err(recovery());}
    let ids=body["files"].members().map(|f|f["id"].as_str().map(str::to_owned).ok_or_else(recovery)).collect::<Result<HashSet<_>,_>>()?;
    if body["action"]=="addMods" {return reject_group_claims(&doc,body,committed);}
    reject_existing_claims(&doc,layout,&ids,committed.then_some((body["targetProfileId"].as_str().ok_or_else(recovery)?,&body["record"])))
}

fn checked_local_cleanup(
    store: &Store,
    lock: &RootLock,
    body: &JsonValue,
    committed: bool,
) -> Result<crate::import::ImportCleanup, NativeError> {
    let modern=matches!(body["version"].as_u32(),Some(6|7));
    let (version, before_version, after_version) = if modern {admit_modern_record(body)?} else {admit_legacy_record(body)?};
    let name = body["stage"]
        .as_str()
        .filter(|s| {
            s.strip_prefix(".import-local-")
                .is_some_and(|s| package::hex(s, 32))
        })
        .ok_or_else(recovery)?;
    let op = body["operationId"]
        .as_str()
        .filter(|s| paths::safe_id(s))
        .ok_or_else(recovery)?;
    if body["kind"] != "import"
        || !(body["version"] == 3 || body["version"] == 4 || body["version"] == 5 || modern)
        || !body["input"].is_object()
        || !body["files"].is_array()
        || body["owner"]["kind"] != "import"
        || body["owner"]["version"] != body["version"]
        || body["owner"]["operationId"] != op
        || body["owner"]["stage"] != name
        || !body["owner"]["nonce"]
            .as_str()
            .is_some_and(|s| package::hex(s, 32))
    {
        return Err(recovery());
    }
    if body["owner"]["authorityDigest"].as_str() != Some(authority_digest(body)?.as_str()) {
        return Err(recovery());
    }
    let v5=body["version"]==5;
    let attachment=modern && body["action"]=="attachOptions";
    if attachment {verify_dependencies(store,body)?;}
    let expected = store::revision(body["expectedRevision"].as_str().ok_or_else(recovery)?)?;
    let profile = body["targetProfileId"].as_str().ok_or_else(recovery)?;
    let record = &body["record"];
    let group=body["action"]=="addMods";
    let records=group_records(body);
    let doc = store.document()?;
    if package::document_version(&doc)? != if committed {after_version}else{before_version}
        || package::layout_version(&doc)?!=version { return Err(recovery()); }
    if committed {
        if store.operation(op)?["status"] != "succeeded"
            || !records.iter().all(|record|doc["profiles"][profile]["mods"].members().any(|r|r==*record))
        {
            return Err(recovery());
        }
        if v5 || modern {
            let result=store.replay(op,&body["input"])?.ok_or_else(recovery)?;
            if store::revision(doc["revision"].as_str().ok_or_else(recovery)?)?!=expected.checked_add(1).ok_or_else(recovery)?
                || !store::matches_recorded_snapshot_for(&doc,&result,Some(profile))? {return Err(recovery());}
        }
    } else if store::revision(doc["revision"].as_str().ok_or_else(recovery)?)? != expected
        || body["beforeDigest"] != paths::digest_bytes(doc.dump().as_bytes())?
    {
        return Err(recovery());
    }
    if modern && (if committed {body["afterDigest"]!=paths::digest_bytes(doc.dump().as_bytes())?} else {body["beforeDocument"]!=doc}) {return Err(recovery());}
    let archive_refs = if group {Vec::new()} else {stored_archive_refs(record)?};
    let font_refs=if group {Vec::new()} else {crate::fonts::stored_refs(record)?};
    if (!v5 && !modern && ((body["version"] == 4) != !archive_refs.is_empty() || !font_refs.is_empty()))
        || v5 && before_version!=3 && font_refs.is_empty()
        || !group && !attachment && record["source"]["localImport"]["version"] != if !font_refs.is_empty(){3}else if archive_refs.is_empty() { 1 } else { 2 } {
        return Err(recovery());
    }
    let mut expected_files = BTreeMap::new();
    let mut expected_names: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for (f, role) in records.iter().map(|r|record_files(r,version)).collect::<Result<Vec<_>,_>>()?.into_iter().flatten() {
        let value = (f.sha256, f.size, role);
        if let Some(old) = expected_files.insert(f.id.clone(), value.clone()) {
            if role != "payload" || old != value { return Err(recovery()); }
        }
        expected_names.entry(f.id).or_default().insert(f.source_name);
    }
    if !group && (v5 || modern && !attachment) {
        let total=|role: &str|expected_files.values().filter(|(_,_,r)|*r==role).try_fold(0u64,|n,(_,size,_)|n.checked_add(*size).ok_or_else(recovery));
        let payload=total("payload")?; let archives=total("archive")?; let artwork=total("artwork")?; let fonts=total("font")?;
        let copies=payload.checked_add(archives).and_then(|n|n.checked_add(artwork)).and_then(|n|n.checked_add(fonts)).ok_or_else(recovery)?;
        let local=&record["source"]["localImport"];
        if payload>137438953472 || archives>68719476736 || artwork>16777216 || fonts>134217728 || copies>137589948416
            || local["uniquePayloadBytes"].as_u64()!=Some(payload) || local["retainedArchiveBytes"].as_u64()!=Some(archives)
            || local["uniqueCopyBytes"].as_u64()!=Some(copies)
            || (!local["uniqueFontBytes"].is_null() || fonts!=0) && local["uniqueFontBytes"].as_u64()!=Some(fonts)
            || !local[if modern {"retainedLogicalPackageBytes"} else {"logicalPackageBytes"}].as_u64().is_some_and(|n|n>=payload && n<=137438953472) {return Err(recovery());}
    }
    if attachment {
        for dependency in body["reusedFiles"].members() {
            let id=dependency["id"].as_str().ok_or_else(recovery)?;
            expected_files.remove(id); expected_names.remove(id);
        }
    }
    // A matching record in another profile is still another owner's claim.
    if group {reject_group_claims(&doc,body,committed)?;} else {reject_existing_claims(&doc, version, &expected_files.keys().cloned().collect(),
        committed.then_some((profile, record)))?;}
    let mut declared = HashSet::new();
    let stage = crate::storage::path(store.root(), name)?;
    let owner = stage.join("owner.json");
    let fsid=if body["owner"].has_key("storageFsid") {Some(body["owner"]["storageFsid"].as_str().filter(|s|package::hex(s,16)).ok_or_else(recovery)?)} else {None};
    let mut plan = crate::import::ImportCleanup::default();
    plan.claims.extend([stage.clone(), owner.clone()]);
    let stage_exists = exists(&stage)?;
    if stage_exists {
        recorded_identity(lock,&stage,&body["stageDevice"],&body["stageInode"],fsid,true)?;
    } else if !committed && !body["stageDevice"].is_null() {
        return Err(recovery());
    }
    let owner_exists = exists(&owner)?;
    if owner_exists {
        recorded_identity(lock,&owner,&body["ownerDevice"],&body["ownerInode"],fsid,false)?;
        if paths::read_bounded(&owner, paths::MAX_JSON_BYTES)? != body["owner"].dump().as_bytes() {
            return Err(recovery());
        }
        plan.owner = Some(owner.clone());
    } else if stage_exists
        && (!body["ownerDevice"].is_null() || fs::read_dir(&stage)?.next().is_some())
    {
        return Err(recovery());
    }
    let mut allowed = HashSet::from(["owner.json".to_owned()]);
    for f in body["files"].members() {
        let r = package::decode_file(f).map_err(|_| recovery())?;
        let role = f["role"].as_str().ok_or_else(recovery)?;
        if !declared.insert(r.id.clone())
            || expected_files.get(&r.id) != Some(&(r.sha256, r.size, role))
            || !expected_names.get(&r.id).is_some_and(|names| names.contains(&r.source_name))
            || !f["complete"].is_boolean()
        {
            return Err(recovery());
        }
        let src = staged(&stage, f)?;
        let dst = destination(store.root(), f)?;
        allowed.insert(src.file_name().unwrap().to_string_lossy().into_owned());
        plan.claims.extend([src.clone(), dst.clone()]);
        let a = exists(&src)?;
        let b = exists(&dst)?;
        for path in [a.then_some(&src), b.then_some(&dst)].into_iter().flatten() {
            verify_recorded(lock,path,f,fsid)?;
        }
        if !committed && b && !a {
            return Err(recovery());
        }
        if (b || committed) && f["complete"] != true || committed && !b {
            return Err(recovery());
        }
        if a {
            plan.files.push(src);
            plan.sync_dirs.insert(stage.clone());
        }
        if b && !committed {
            plan.sync_dirs.insert(dst.parent().unwrap().to_path_buf());
            plan.files.push(dst);
        }
    }
    if declared.len() != expected_files.len() {
        return Err(recovery());
    }
    if stage_exists {
        for entry in fs::read_dir(&stage)? {
            if !allowed.contains(&entry?.file_name().to_string_lossy().into_owned()) {
                return Err(recovery());
            }
        }
        plan.stage = Some(stage);
    }
    Ok(plan)
}

/// Invalid ownership evidence never authorizes partial cleanup.
pub(crate) fn plan_local_cleanup(
    store: &Store,
    lock: &RootLock,
    body: &JsonValue,
    committed: bool,
) -> Result<crate::import::ImportCleanup, NativeError> {
    crate::deployment_metadata::domain(&store.document()?,body["version"]==7).map_err(|_|recovery())?;
    if body["version"]==7 {crate::deployment_metadata::admit(store.root(),&body["beforeDocument"])?;}
    checked_local_cleanup(store, lock, body, committed).map_err(|_| recovery())
}
