mod support;
use modlock_helper::{local_content::*, paths, prepare_control::*};
use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt},
    path::PathBuf,
};
use support::{family_vpk, vpk, FamilyEntry, Fixture};
fn request(f: &Fixture, roots: Vec<PathBuf>) -> LocalPrepareRequest {
    LocalPrepareRequest {
        root: f.data.clone(),
        profile_id: "default".into(),
        expected_revision: 7,
        roots,
        artwork: None,
    }
}
fn prepare(f: &Fixture, roots: Vec<PathBuf>) -> LocalPrepared {
    prepare_local(request(f, roots), &PrepareControl::new()).unwrap()
}
fn folder(f: &Fixture, name: &str) -> PathBuf {
    let p = f.root.join(name);
    fs::create_dir_all(&p).unwrap();
    p
}
fn standalone(p: &std::path::Path) {
    fs::write(p, vpk("a.bin", 1)).unwrap();
}
fn external(p: &std::path::Path, archive: u16, length: u32) {
    fs::write(
        p,
        family_vpk(
            1,
            &[FamilyEntry {
                path: "a.bin",
                crc: 1,
                archive,
                offset: 0,
                length,
                preload: b"p",
            }],
            b"",
        ),
    )
    .unwrap();
}
fn selection(p: &LocalPrepared) -> LocalSelection {
    let mut ids = p
        .candidates
        .iter()
        .filter(|c| c.package.is_some())
        .map(|c| c.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    LocalSelection {
        candidate_ids: ids,
        metadata: p.default_metadata(),
    }
}
#[test]
fn local_two_roots_same_basename_keeps_both_relative_sources() {
    let f = Fixture::new();
    let a = folder(&f, "a");
    let b = folder(&f, "b");
    standalone(&a.join("same_dir.vpk"));
    standalone(&b.join("same_dir.vpk"));
    let before = [
        paths::fingerprint(&a.join("same_dir.vpk")).unwrap(),
        paths::fingerprint(&b.join("same_dir.vpk")).unwrap(),
    ];
    let p = prepare(&f, vec![a.clone(), b.clone()]);
    assert_eq!(p.candidates.len(), 2);
    assert_ne!(p.candidates[0].id, p.candidates[1].id);
    assert_ne!(p.candidates[0].option_name, p.candidates[1].option_name);
    let plan = review_local(&p, selection(&p)).unwrap();
    let sources = plan.selected_sources().unwrap();
    assert_eq!(sources[0].directory.source_path, a.join("same_dir.vpk"));
    assert_eq!(sources[1].directory.source_path, b.join("same_dir.vpk"));
    assert_eq!(sources[0].directory.original_name, "same_dir.vpk");
    assert_eq!(
        plan.package_refs().unwrap()[0].directory.source_name,
        "same_dir.vpk"
    );
    revalidate_local(&p, &PrepareControl::new()).unwrap();
    assert_eq!(
        before,
        [
            paths::fingerprint(&a.join("same_dir.vpk")).unwrap(),
            paths::fingerprint(&b.join("same_dir.vpk")).unwrap()
        ]
    );
    assert!(!f.data.exists());
}
#[test]
fn local_exact_companion_zero_is_not_an_option() {
    let f = Fixture::new();
    let a = folder(&f, "split");
    external(&a.join("pak_dir.vpk"), 0, 1);
    standalone(&a.join("pak_000.vpk"));
    let p = prepare(&f, vec![a.join("pak_dir.vpk")]);
    assert_eq!(p.candidates.len(), 1);
    let package = p.candidates[0].package.as_ref().unwrap();
    assert_eq!(package.companions.len(), 1);
    assert_eq!(package.companions[&0].source_path, a.join("pak_000.vpk"));
    let p = prepare(&f, vec![a]);
    assert_eq!(p.candidates.len(), 1);
}
#[test]
fn local_no_dir_and_preload_only_match_family_rules() {
    let f = Fixture::new();
    let a = folder(&f, "nodir");
    external(&a.join("pak.vpk"), 17, 1);
    fs::write(a.join("pak_017.vpk"), b"x").unwrap();
    external(&a.join("preload.vpk"), 0, 0);
    let p = prepare(&f, vec![a]);
    assert_eq!(p.candidates.len(), 2);
    assert!(p.candidates.iter().all(|c| c.package.is_some()));
    assert_eq!(
        p.candidates
            .iter()
            .find(|c| c.relative_path == "preload.vpk")
            .unwrap()
            .package
            .as_ref()
            .unwrap()
            .companions
            .len(),
        0
    );
}
#[test]
fn local_opaque_missing_and_case_ambiguous_are_unavailable() {
    let f = Fixture::new();
    let a = folder(&f, "bad");
    fs::write(a.join("opaque.vpk"), b"opaque").unwrap();
    external(&a.join("missing_dir.vpk"), 0, 2);
    external(&a.join("case_dir.vpk"), 0, 1);
    fs::write(a.join("case_000.vpk"), b"x").unwrap();
    fs::write(a.join("CASE_000.vpk"), b"x").unwrap();
    let p = prepare(&f, vec![a]);
    assert!(p
        .candidates
        .iter()
        .all(|c| c.package.is_none() && c.unavailable_reason.is_some()));
    assert!(review_local(
        &p,
        LocalSelection {
            candidate_ids: vec![p.candidates[0].id.clone()],
            metadata: p.default_metadata()
        }
    )
    .is_err());
}
#[test]
fn local_overlap_hardlink_and_unknown_sibling_accounting() {
    let f = Fixture::new();
    let a = folder(&f, "overlap");
    let sub = a.join("sub");
    fs::create_dir(&sub).unwrap();
    standalone(&sub.join("one.vpk"));
    fs::hard_link(sub.join("one.vpk"), sub.join("two.vpk")).unwrap();
    fs::write(sub.join("unknown.txt"), b"keep").unwrap();
    let p = prepare(&f, vec![a.clone(), sub.clone(), sub.join("one.vpk")]);
    assert_eq!(p.candidates.len(), 2);
    assert_eq!(p.evidence["provenance"].len(), 5); // each of the two files' overlapping root evidence, plus explicit one
    assert!(p.review_dto().dump().contains("unsupportedFile"));
    fs::rename(sub.join("two.vpk"), sub.join("renamed.vpk")).unwrap();
    assert_eq!(
        revalidate_local(&p, &PrepareControl::new())
            .unwrap_err()
            .kind,
        "stalePreview"
    );
}
#[test]
fn local_multiple_directories_have_distinct_ids_and_source_order() {
    let f = Fixture::new();
    let a = folder(&f, "many");
    for name in ["z.vpk", "a.vpk", "b.vpk"] {
        standalone(&a.join(name));
    }
    let p = prepare(&f, vec![a]);
    assert_eq!(
        p.candidates
            .iter()
            .map(|c| c.relative_path.as_str())
            .collect::<Vec<_>>(),
        ["a.vpk", "b.vpk", "z.vpk"]
    );
    assert_eq!(
        p.candidates
            .iter()
            .map(|c| c.id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    let plan = review_local(&p, selection(&p)).unwrap();
    assert_eq!(
        plan.selected_sources()
            .unwrap()
            .iter()
            .map(|p| p.directory.original_name.as_str())
            .collect::<Vec<_>>(),
        ["a.vpk", "b.vpk", "z.vpk"]
    );
    let mut unsorted = selection(&p);
    unsorted.candidate_ids.reverse();
    assert!(review_local(&p, unsorted).is_err());
}
#[test]
fn local_archive_font_and_symlink_exclusions_are_visible() {
    let f = Fixture::new();
    let a = folder(&f, "exclude");
    standalone(&a.join("ok.vpk"));
    for n in ["a.zip", "a.ttf", ".origin"] {
        fs::write(a.join(n), b"keep").unwrap();
    }
    symlink(a.join("ok.vpk"), a.join("link.vpk")).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(a.join("pipe"))
        .status()
        .unwrap()
        .success());
    let originals = ["ok.vpk", "a.zip", "a.ttf", ".origin"]
        .map(|name| paths::fingerprint(&a.join(name)).unwrap());
    let p = prepare(&f, vec![a.clone()]);
    assert_eq!(
        originals,
        ["ok.vpk", "a.zip", "a.ttf", ".origin"]
            .map(|name| paths::fingerprint(&a.join(name)).unwrap())
    );
    let dto = p.review_dto().dump();
    for reason in [
        "archiveUnsupported",
        "fontUnsupported",
        "symlink",
        "unsupportedFile",
        "specialFile",
    ] {
        assert!(dto.contains(reason), "{dto}");
    }
    assert!(prepare_local(
        request(&f, vec![a.join("link.vpk")]),
        &PrepareControl::new()
    )
    .is_err());
    assert_eq!(
        prepare_local(request(&f, vec![f.root.clone()]), &PrepareControl::new())
            .unwrap_err()
            .kind,
        "unsafeLocalRoot"
    );
}
#[test]
fn local_bounds_fail_without_partial_review() {
    let f = Fixture::new();
    let a = folder(&f, "bounds");
    standalone(&a.join("ok.vpk"));
    assert!(prepare_local(request(&f, vec![a.clone(); 64]), &PrepareControl::new()).is_ok());
    assert!(prepare_local(request(&f, vec![a.clone(); 65]), &PrepareControl::new()).is_err());
    let mut deepest = a.clone();
    for _ in 0..32 {
        deepest.push("d");
        fs::create_dir(&deepest).unwrap();
    }
    let p = prepare(&f, vec![a.clone()]);
    assert_eq!(p.candidates.len(), 1);
    fs::create_dir(deepest.join("too-deep")).unwrap();
    assert!(prepare_local(request(&f, vec![a.clone()]), &PrepareControl::new()).is_err());
    fs::remove_dir_all(a.join("d")).unwrap();
    for i in 0..99 {
        standalone(&a.join(format!("p{i:03}.vpk")));
    }
    let p = prepare(&f, vec![a.clone()]);
    let mut s = selection(&p);
    assert_eq!(s.candidate_ids.len(), 100);
    assert!(review_local(&p, s.clone()).is_err());
    s.candidate_ids.pop();
    assert!(review_local(&p, s).is_ok());
    let huge = a.join("huge.vpk");
    fs::File::create(&huge)
        .unwrap()
        .set_len(16 * 1024 * 1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        prepare_local(request(&f, vec![a]), &PrepareControl::new())
            .unwrap_err()
            .kind,
        "localSourceTooLarge"
    );
    assert!(!f.data.exists());
}
#[test]
fn local_changed_inventory_and_participating_hash_are_stale() {
    let f = Fixture::new();
    let a = folder(&f, "changes");
    standalone(&a.join("a.vpk"));
    let p = prepare(&f, vec![a.clone()]);
    fs::write(a.join("new.txt"), b"new").unwrap();
    assert_eq!(
        revalidate_local(&p, &PrepareControl::new())
            .unwrap_err()
            .kind,
        "stalePreview"
    );
    fs::remove_file(a.join("new.txt")).unwrap();
    let p = prepare(&f, vec![a.clone()]);
    let path = a.join("a.vpk");
    let before = fs::metadata(&path).unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert_eq!(
        revalidate_local(&p, &PrepareControl::new())
            .unwrap_err()
            .kind,
        "stalePreview"
    );
    assert!(before.ino() > 0);
    assert!(!f.data.exists());
}
#[test]
fn local_metadata_and_artwork_budget_validation() {
    let f = Fixture::new();
    let a = folder(&f, "metadata");
    standalone(&a.join("thing_dir.vpk"));
    let p = prepare(&f, vec![a.clone()]);
    assert_eq!(p.default_metadata().name, "thing");
    let mut s = selection(&p);
    s.metadata.description = "\"\\\t\n雪".repeat(1000);
    s.metadata.name = "雪".repeat(85);
    s.metadata.link = "https://example.invalid:443/path?q=x#part".into();
    assert!(review_local(&p, s.clone()).is_ok());
    for bad in [
        "http://",
        "https://user@host/path",
        "https://host:bad/",
        "https://host a/",
        "file:///tmp/a",
        "https://[bad]/",
    ] {
        let mut bad_s = s.clone();
        bad_s.metadata.link = bad.into();
        assert!(review_local(&p, bad_s).is_err(), "{bad}");
    }
    s.metadata.name.push('雪');
    assert!(review_local(&p, s).is_err());
    let png = a.join("cover.png");
    fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    let mut r = request(&f, vec![a.clone()]);
    r.artwork = Some(png.clone());
    assert!(prepare_local(r.clone(), &PrepareControl::new()).is_err());
    let mut valid = vec![0u8; 33];
    valid[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    valid[8..12].copy_from_slice(&13u32.to_be_bytes());
    valid[12..16].copy_from_slice(b"IHDR");
    valid[16..20].copy_from_slice(&32u32.to_be_bytes());
    valid[20..24].copy_from_slice(&32u32.to_be_bytes());
    valid[24] = 8;
    valid[25] = 6;
    fs::write(&png, &valid).unwrap();
    assert!(prepare_local(r.clone(), &PrepareControl::new()).is_ok());
    valid[16..20].copy_from_slice(&8193u32.to_be_bytes());
    fs::write(&png, &valid).unwrap();
    assert!(prepare_local(r.clone(), &PrepareControl::new()).is_err());
    fs::File::create(&png)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(prepare_local(r, &PrepareControl::new()).is_err());
    let jpeg = a.join("cover.jpg");
    fs::write(&jpeg, [0xff, 0xd8, 0xff]).unwrap();
    let mut r = request(&f, vec![a]);
    r.artwork = Some(jpeg);
    assert!(prepare_local(r, &PrepareControl::new()).is_err());
}
#[test]
fn local_pre_cancel_propagates_without_storage() {
    let f = Fixture::new();
    let a = folder(&f, "cancel");
    standalone(&a.join("a.vpk"));
    let c = PrepareControl::new();
    c.request_cancel();
    assert_eq!(
        prepare_local(request(&f, vec![a]), &c).unwrap_err().kind,
        "localPreparationCancelled"
    );
    assert!(!f.data.exists());
}
#[test]
fn local_entry_candidate_and_total_byte_boundaries() {
    let f = Fixture::new();
    let a = folder(&f, "entry-limit");
    for i in 0..9999 {
        fs::write(a.join(format!("{i:05}.txt")), b"").unwrap();
    }
    assert_eq!(
        prepare(&f, vec![a.clone()]).evidence["inventory"].len(),
        10000
    );
    fs::write(a.join("overflow.txt"), b"").unwrap();
    assert_eq!(
        prepare_local(request(&f, vec![a]), &PrepareControl::new())
            .unwrap_err()
            .kind,
        "localSourceTooLarge"
    );
    let a = folder(&f, "candidate-limit");
    for i in 0..1024 {
        fs::write(a.join(format!("{i:04}.vpk")), b"opaque").unwrap();
    }
    assert_eq!(prepare(&f, vec![a.clone()]).candidates.len(), 1024);
    fs::write(a.join("overflow.vpk"), b"opaque").unwrap();
    assert_eq!(
        prepare_local(request(&f, vec![a]), &PrepareControl::new())
            .unwrap_err()
            .kind,
        "localSourceTooLarge"
    );
    let a = folder(&f, "total-limit");
    for i in 0..4 {
        fs::File::create(a.join(format!("{i}.vpk")))
            .unwrap()
            .set_len(16 * 1024 * 1024 * 1024)
            .unwrap();
    }
    assert_eq!(
        prepare(&f, vec![a.clone()]).evidence["uniqueSourceBytes"]
            .as_u64()
            .unwrap(),
        64 * 1024 * 1024 * 1024
    );
    fs::write(a.join("overflow.vpk"), b"x").unwrap();
    assert_eq!(
        prepare_local(request(&f, vec![a]), &PrepareControl::new())
            .unwrap_err()
            .kind,
        "localSourceTooLarge"
    );
    assert!(!f.data.exists());
}
#[test]
fn local_explicit_file_scope_ignores_unknown_siblings_and_keeps_claimed_evidence() {
    let f = Fixture::new();
    let a = folder(&f, "scope");
    external(&a.join("pak_dir.vpk"), 0, 1);
    fs::write(a.join("pak_000.vpk"), b"a").unwrap();
    fs::write(a.join("unrelated.txt"), b"keep").unwrap();
    let p = prepare(&f, vec![a.join("pak_dir.vpk")]);
    fs::write(a.join("unrelated.txt"), b"changed").unwrap();
    revalidate_local(&p, &PrepareControl::new()).unwrap();
    fs::write(a.join("pak_000.vpk"), b"b").unwrap();
    assert_eq!(
        revalidate_local(&p, &PrepareControl::new())
            .unwrap_err()
            .kind,
        "stalePreview"
    );
}
#[test]
fn local_required_companion_directory_is_excluded_without_descending() {
    let f = Fixture::new();
    let a = folder(&f, "not-a-member");
    external(&a.join("pak_dir.vpk"), 0, 1);
    fs::create_dir(a.join("pak_000.vpk")).unwrap();
    standalone(&a.join("pak_000.vpk/hidden.vpk"));
    let p = prepare(&f, vec![a.join("pak_dir.vpk")]);
    assert_eq!(p.candidates.len(), 1);
    assert!(p.candidates[0].package.is_none());
    assert!(p
        .review_dto()
        .dump()
        .contains("requiredCompanionNotRegular"));
    assert!(!p.review_dto().dump().contains("hidden.vpk"));
}
#[test]
fn local_storage_refs_are_fresh_per_plan_stable_and_dedup_only_within_plan() {
    let f = Fixture::new();
    let a = folder(&f, "fresh-refs");
    standalone(&a.join("one.vpk"));
    standalone(&a.join("two.vpk"));
    let p = prepare(&f, vec![a]);
    let first = review_local(&p, selection(&p)).unwrap();
    let second = review_local(&p, selection(&p)).unwrap();
    let refs = first.package_refs().unwrap();
    assert_eq!(refs, first.package_refs().unwrap());
    assert_eq!(refs, first.clone().package_refs().unwrap());
    // Equal bytes under different names get separate IDs.
    assert_ne!(refs[0].directory.id, refs[1].directory.id);
    assert_ne!(refs[0].directory.source_name, refs[1].directory.source_name);
    let other = second.package_refs().unwrap();
    let ids = refs
        .iter()
        .flat_map(|p| std::iter::once(&p.directory).chain(p.companions.values()))
        .map(|f| f.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert!(other
        .iter()
        .flat_map(|p| std::iter::once(&p.directory).chain(p.companions.values()))
        .all(|f| !ids.contains(f.id.as_str())));
    assert_eq!(
        first
            .prepared
            .candidates
            .iter()
            .map(|c| &c.id)
            .collect::<Vec<_>>(),
        second
            .prepared
            .candidates
            .iter()
            .map(|c| &c.id)
            .collect::<Vec<_>>()
    );
    assert!(!f.data.exists());
}
