//! Archive assembly and evidence stay within the local_content authority owner.
use super::*;
use crate::archive_content::{self, ArchiveInput, ArchiveKind, ArchiveMember, ExtractedMember};

pub(super) fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "zip" | "rar" | "7z"))
}
pub(super) fn prepare(
    scan: &mut Scan<'_>,
    lease: Arc<QuarantineLease>,
) -> Result<Vec<Arc<PreparedArchive>>, NativeError> {
    let mut archives = Vec::new();
    for found in &scan.archives {
        scan.control.check()?;
        let metadata = paths::open_regular(&found.path)?.metadata()?;
        let prior = scan
            .inventory
            .iter()
            .find(|e| {
                e["rootOrdinal"].as_usize() == Some(found.ordinal)
                    && e["relativePath"] == found.relative
            })
            .ok_or_else(stale)?;
        if file_identity(&metadata) != prior["identity"] {
            return Err(stale());
        }
        let fingerprint = fingerprint_admitted(&found.path, scan.control, &metadata, MAX_MEMBER)?;
        let archive = archive_content::prepare_archive_selected(
            ArchiveInput {
                root_ordinal: found.ordinal,
                relative_path: found.relative.clone(),
                original_path: found.path.clone(),
                fingerprint,
            },
            lease.clone(),
            scan.budget.as_mut().unwrap(),
            &mut font_sources::Selector { budget: scan.font_budget.as_mut().ok_or_else(stale)? },
            scan.control,
        )?;
        archives.push(Arc::new(archive));
    }
    Ok(archives)
}
pub(super) fn found(archives: &[Arc<PreparedArchive>]) -> Vec<Found> {
    archives
        .iter()
        .flat_map(|archive| {
            archive.extracted_members().iter().filter(|m|m.role()==archive_content::ArchiveContentRole::Vpk).map(|member| Found {
                ordinal: archive.input().root_ordinal,
                relative: member.member().normalized_path.clone(),
                path: member.path().into(),
                archive: Some(archive.input().relative_path.clone()),
            })
        })
        .collect()
}
pub(super) fn selected(prepared: &LocalPrepared, sources: &[SourcePackage], fonts: &[crate::font_content::FontCandidate]) -> Result<Vec<Arc<PreparedArchive>>,NativeError> {
    let contributors=font_sources::contributing_archives(fonts)?;
    let selected_paths = sources.iter().flat_map(|p|
        std::iter::once(p.directory.source_path.as_path()).chain(p.companions.values().map(|s| s.source_path.as_path())))
        .collect::<HashSet<_>>();
    Ok(prepared.archives.iter().filter(|archive|
        archive.extracted_members().iter().any(|m| selected_paths.contains(m.path())) || contributors.iter().any(|a|Arc::ptr_eq(a,archive)))
        .cloned().collect())
}
/// Build native archive membership only from live owner-produced handles.
pub(super) fn binding(archives: &[Arc<PreparedArchive>], refs: &[FileRef], source: &SourcePackage)
    -> Result<Option<crate::options::ArchiveBinding>, NativeError> {
    let Some((archive, _)) = member_for(archives, &source.directory.source_path) else {return Ok(None)};
    let file = archives.iter().zip(refs).find(|(a,_)|std::ptr::eq(a.as_ref(),archive))
        .map(|(_,f)|f.clone()).ok_or_else(stale)?;
    let members = std::iter::once((None, &source.directory))
        .chain(source.companions.iter().map(|(i,s)|(Some(*i),s)))
        .map(|(archive_index,s)| {
            let (owner, member) = member_for(archives, &s.source_path).ok_or_else(stale)?;
            if !std::ptr::eq(owner,archive) {return Err(stale());}
            Ok(crate::options::ArchiveMemberBinding { archive_index, member_index: member.member().index,
                normalized_path: member.member().normalized_path.clone(), decoder_path: member.member().decoder_path.clone() })
        }).collect::<Result<Vec<_>,NativeError>>()?;
    Ok(Some(crate::options::ArchiveBinding {file,members}))
}
/// Enrich display provenance inside its logical owner, never decode it as I/O authority.
pub(super) fn bind_review(summary: &mut JsonValue, archives: &[Arc<PreparedArchive>], refs: &[FileRef]) -> Result<(), NativeError> {
    if archives.is_empty() { return Ok(()); }
    summary["archives"] = JsonValue::from(archives.iter().zip(refs).map(|(a, f)| object! {
        rootOrdinal:a.input().root_ordinal,relativePath:a.input().relative_path.clone(),
        originalPath:a.input().original_path.to_str().unwrap(),format:a.format(),file:package::encode_file(f)
    }).collect::<Vec<_>>());
    for (archive, file) in archives.iter().zip(refs) {
        for row in summary["provenance"].members_mut() {
            if row["rootOrdinal"].as_usize() == Some(archive.input().root_ordinal)
                && row["archiveRelativePath"] == archive.input().relative_path {
                row["archive"]["id"] = file.id.clone().into();
                row["archive"]["normalizedPath"] = row["archive"]["memberPath"].clone();
            }
        }
        for candidate in summary["candidates"].members_mut() {
            if candidate["rootOrdinal"].as_usize() == Some(archive.input().root_ordinal)
                && candidate["archiveRelativePath"] == archive.input().relative_path {
                for member in candidate["members"].members_mut() {
                    member["archive"]["id"] = file.id.clone().into();
                    member["archive"]["normalizedPath"] = member["archive"]["memberPath"].clone();
                }
            }
        }
    }
    Ok(())
}
pub(super) fn member_provenance(archive: &PreparedArchive, member: &ArchiveMember) -> JsonValue {
    object! {sha256:archive.snapshot_fingerprint().sha256.clone(),index:member.index,decoderPath:member.decoder_path.clone(),memberPath:member.normalized_path.clone(),format:archive.format(),encoding:archive.encoding()}
}
pub(super) fn provenance(archives: &[Arc<PreparedArchive>]) -> Vec<JsonValue> {
    archives.iter().flat_map(|archive| archive.members().iter().map(|member| {
        object! {rootOrdinal:archive.input().root_ordinal,relativePath:member.normalized_path.clone(),path:archive.input().original_path.to_str().unwrap(),archiveRelativePath:archive.input().relative_path.clone(),archive:member_provenance(archive,member),kind:match member.kind {ArchiveKind::Directory=>"directory",_=>"file"},exclusion:match &member.kind {ArchiveKind::Rejected(reason)=>Some(reason.clone()),_=>None}}
    })).collect()
}
pub(super) fn evidence(archives: &[Arc<PreparedArchive>]) -> JsonValue {
    JsonValue::from(archives.iter().map(|archive| object! {
        rootOrdinal:archive.input().root_ordinal,relativePath:archive.input().relative_path.clone(),
        original:fp_json(&archive.input().original_path,&archive.input().fingerprint),
        snapshot:fp_json(archive.snapshot_path(),archive.snapshot_fingerprint()),
        format:archive.format(),encoding:archive.encoding(),
        manifest:JsonValue::from(archive.members().iter().map(|m| object! {index:m.index,decoderPath:m.decoder_path.clone(),memberPath:m.normalized_path.clone(),size:m.size,kind:match m.kind {ArchiveKind::Regular=>"regular",ArchiveKind::Directory=>"directory",ArchiveKind::Rejected(_)=>"rejected"},reason:match &m.kind {ArchiveKind::Rejected(r)=>Some(r.clone()),_=>None}}).collect::<Vec<_>>()),
        staged:JsonValue::from(archive.extracted_members().iter().map(|m| object! {index:m.member().index,fingerprint:fp_json(m.path(),m.fingerprint())}).collect::<Vec<_>>())
    }).collect::<Vec<_>>())
}
pub(super) fn roles(archives: &[Arc<PreparedArchive>]) -> JsonValue {
    JsonValue::from(archives.iter().map(|a|object!{rootOrdinal:a.input().root_ordinal,relativePath:a.input().relative_path.clone(),members:JsonValue::from(a.extracted_members().iter().map(|m|object!{index:m.member().index,role:match m.role(){archive_content::ArchiveContentRole::Vpk=>"vpk",archive_content::ArchiveContentRole::BundledTtf=>"font"}}).collect::<Vec<_>>())}).collect::<Vec<_>>())
}
pub(super) fn member_for<'a>(
    archives: &'a [Arc<PreparedArchive>],
    path: &Path,
) -> Option<(&'a PreparedArchive, &'a ExtractedMember)> {
    archives.iter().find_map(|archive| {
        archive
            .extracted_members()
            .iter()
            .find(|m| m.path() == path)
            .map(|m| (archive.as_ref(), m))
    })
}
pub(super) fn verify_fingerprint(
    path: &Path,
    expected: &Fingerprint,
    control: &PrepareControl,
) -> Result<(), NativeError> {
    let metadata = paths::open_regular(path)?.metadata()?;
    if metadata.len() != expected.size
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
        || metadata.mtime() != expected.modified_seconds
        || metadata.mtime_nsec() != expected.modified_nanos
    {
        return Err(stale());
    }
    if fingerprint_admitted(path, control, &metadata, MAX_MEMBER)? != *expected {
        return Err(stale());
    }
    Ok(())
}
fn validate_leases(archives: &[Arc<PreparedArchive>]) -> Result<(), NativeError> {
    let mut seen = HashSet::new();
    for archive in archives {
        if seen.insert(Arc::as_ptr(archive.lease())) {
            archive.lease().validate_sealed()?;
        }
    }
    Ok(())
}
pub(super) fn revalidate(
    prepared: &LocalPrepared,
    control: &PrepareControl,
) -> Result<(), NativeError> {
    admit(&prepared.request, control)?;
    validate_leases(&prepared.archives)?;
    if prepared.evidence["scope"] != request_json(&prepared.request)
        || prepared.evidence["archives"] != evidence(&prepared.archives)
    {
        return Err(stale());
    }
    let inventory = prepared.evidence["inventory"]
        .members()
        .cloned()
        .collect::<Vec<_>>();
    verify_inventory(&inventory, control)?;
    if prepared.evidence["version"]==3 {
        let encoded=JsonValue::from(prepared.fingerprints.iter().map(|(p,f)|fp_json(p,f)).collect::<Vec<_>>());
        if prepared.evidence["fingerprints"]!=encoded {return Err(stale());}
    }
    // The retained preparation owns content proofs; every member still needs
    // an exact fsid/inode/size/mtime/ctime match and the inventory/lease guards.
    for path in prepared.content_stamps.keys() {
        control.check()?;
        prepared.recheck_content(path)?;
    }
    verify_inventory(&inventory, control)?;
    validate_leases(&prepared.archives)?;
    control.check()
}

#[cfg(test)]
mod tests {
    use super::super::archive_test_support::assembled;
    use super::*;
    use crate::{
        archive_content::test_support::{sealed_archive, Fixture},
        archive_quarantine::QuarantineAuthority,
        test_support::{family_vpk, vpk, FamilyEntry},
    };
    #[test]
    fn archive_consumer_replaced_scratch_with_same_member_inodes_invalidates_review() {
        let f = Fixture::new();
        let db = f.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&db);
        let lease =
            QuarantineLease::create(&authority, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let original = f.root.join("input.zip");
        fs::write(&original, b"isolated archive transport fixture").unwrap();
        let bytes = vpk("a.bin", 1);
        let archive = sealed_archive(
            lease.clone(),
            &original,
            0,
            "input.zip",
            &[("folder/a.vpk", &bytes)],
        );
        let prepared = assembled(archive, db.root());
        revalidate_local(&prepared, &PrepareControl::new()).unwrap();
        let scratch = lease.scratch_path().unwrap();
        let old = f.root.join("old-scratch");
        fs::rename(&scratch, &old).unwrap();
        fs::create_dir(&scratch).unwrap();
        for entry in fs::read_dir(&old).unwrap() {
            let entry = entry.unwrap();
            fs::rename(entry.path(), scratch.join(entry.file_name())).unwrap();
        }
        let result = revalidate_local(&prepared, &PrepareControl::new());
        assert!(
            result.is_err(),
            "same member inodes in a replacement scratch directory must not preserve a live review"
        );
    }
    #[test]
    fn archive_consumer_families_use_original_names_and_coordinate_namespaces() {
        let f = Fixture::new();
        let db = f.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&db);
        let lease =
            QuarantineLease::create(&authority, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        let original = f.root.join("a.zip");
        fs::write(&original, b"isolated transport").unwrap();
        let directory = family_vpk(
            1,
            &[FamilyEntry {
                path: "a.bin",
                crc: 1,
                archive: 0,
                offset: 0,
                length: 3,
                preload: b"",
            }],
            b"",
        );
        let a = sealed_archive(
            lease.clone(),
            &original,
            0,
            "a.zip",
            &[
                ("folder/skin_dir.vpk", &directory),
                ("folder/skin_000.vpk", b"abc"),
            ],
        );
        let b = sealed_archive(
            lease.clone(),
            &original,
            1,
            "b.zip",
            &[("folder/skin_dir.vpk", &directory)],
        );
        let mut budget = ArchiveBudget::new();
        budget.admit_roots(2).unwrap();
        let candidates = resolve_families(
            found(&[b.clone(), a.clone()]),
            &PrepareControl::new(),
            Some(&mut budget),
        )
        .unwrap();
        assert_eq!(candidates.len(), 2, "claimed companions are not options");
        assert_eq!(
            candidates[0].id,
            paths::digest_bytes(b"[0,\"a.zip\",\"folder/skin_dir.vpk\"]").unwrap()
        );
        assert_eq!(
            candidates[1].id,
            paths::digest_bytes(b"[1,\"b.zip\",\"folder/skin_dir.vpk\"]").unwrap()
        );
        let family = candidates[0].package.as_ref().unwrap();
        assert_eq!(family.directory.original_name, "skin_dir.vpk");
        assert_eq!(family.companions[&0].original_name, "skin_000.vpk");
        assert_eq!(
            candidates[1].unavailable_reason.as_deref(),
            Some("incompleteOrAmbiguousFamily")
        );
        let p = assembled(a.clone(), db.root());
        let dto = p.review_dto();
        let member = &dto["candidates"][0]["members"][0];
        assert_eq!(member["sourcePath"], original.to_str().unwrap());
        assert_eq!(member["archive"]["index"], 0);
        assert_eq!(member["archive"]["decoderPath"], "folder/skin_dir.vpk");
        assert_eq!(member["archive"]["sha256"], a.snapshot_fingerprint().sha256);
        assert!(!p.source_provenance().dump().contains(".local-quarantine-"));
        let plan = review_local(
            &p,
            LocalSelection {
                candidate_ids: vec![p.candidates[0].id.clone()],
                metadata: p.default_metadata(),
            },
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            &p.archive_sources()[0],
            &plan.prepared.archive_sources()[0]
        ));
        assert!(
            std::ptr::eq(&p.evidence["archives"], &plan.prepared.evidence["archives"]),
            "immutable manifest evidence must be shared across review clones"
        );
        let control=PrepareControl::new();
        revalidate_local(&p, &control).unwrap();
        revalidate_local(&p, &control).unwrap();
        assert_eq!(control.hashes_started(),0,"retained archive proof must avoid rehashing");
        drop((p, plan, a, b));
        lease.cleanup(&authority).unwrap();
    }
    #[test]
    fn archive_consumer_missing_original_or_changed_sealed_data_invalidates_review() {
        for change in [
            "missingOriginal",
            "changedOriginal",
            "changedMember",
            "unknownEntry",
            "hardlink",
        ] {
            let f = Fixture::new();
            let db = f.store();
            let authority = QuarantineAuthority::for_held_jobs_store(&db);
            let lease =
                QuarantineLease::create(&authority, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
            let original = f.root.join("input.zip");
            fs::write(&original, b"transport").unwrap();
            let bytes = vpk("a.bin", 1);
            let a = sealed_archive(
                lease.clone(),
                &original,
                0,
                "input.zip",
                &[("a.vpk", &bytes)],
            );
            let p = assembled(a.clone(), db.root());
            match change {
                "missingOriginal" => fs::remove_file(&original).unwrap(),
                "changedOriginal" => fs::write(&original, b"changed").unwrap(),
                "changedMember" => fs::write(a.extracted_members()[0].path(), b"changed").unwrap(),
                "unknownEntry" => {
                    fs::write(lease.scratch_path().unwrap().join("unknown"), b"keep").unwrap()
                }
                _ => fs::hard_link(a.extracted_members()[0].path(), f.root.join("alias")).unwrap(),
            }
            assert_eq!(
                revalidate_local(&p, &PrepareControl::new())
                    .unwrap_err()
                    .kind,
                "stalePreview",
                "{change}"
            );
        }
    }
}
