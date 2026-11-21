use super::test_support::{command, Fixture};
use super::*;
use crate::archive_quarantine::QuarantineAuthority;
use json::{array, object};
use std::process::Command;

fn entry(index: u32, path: &str, size: u64) -> JsonValue {
    object! {XADIndex:index, XADFileName:path, XADFileSize:size}
}
fn listing(entries: Vec<JsonValue>) -> JsonValue {
    object! {lsarFormatVersion:2, lsarContents:JsonValue::Array(entries), lsarFormatName:"Zip", lsarEncoding:"UTF-8", lsarConfidence:1, lsarProperties:object!{XADVolumes:array!["/snapshot"]}}
}
fn admitted(
    value: &JsonValue,
    budget: &mut ArchiveBudget,
) -> Result<Vec<ArchiveMember>, NativeError> {
    manifest(
        &envelope(value.dump().as_bytes(), Path::new("/snapshot"))?,
        budget,
    )
}

#[test]
fn paths_and_collisions_fail_before_extraction() {
    for path in [
        "",
        "/a.vpk",
        "\\a.vpk",
        "C:a.vpk",
        "a/../b.vpk",
        "./a.vpk",
        "a//b.vpk",
        "a\0.vpk",
        "a\n.vpk",
        "a/�.vpk",
        "a/%ff.vpk",
        "a/(nil)/b.vpk",
        "a.vpk/",
    ] {
        assert!(normalize_path(path, false).is_err(), "accepted {path:?}");
    }
    assert_eq!(
        normalize_path("mods\\skin.vpk", false).unwrap(),
        "mods/skin.vpk"
    );
    assert_eq!(normalize_path("mods\\", true).unwrap(), "mods");
    assert!(normalize_path(&"é".repeat(2049), false).is_err());
    assert!(normalize_path(&vec!["a"; 33].join("/"), false).is_err());
    for (a, b) in [
        ("a.vpk", "a.vpk"),
        ("A.vpk", "a.vpk"),
        ("Straße.vpk", "STRASSE.vpk"),
        ("Dir/a.vpk", "dir/b.vpk"),
        ("a.vpk", "a.vpk/b.vpk"),
        ("a.vpk/b.vpk", "a.vpk"),
    ] {
        assert_eq!(
            admitted(
                &listing(vec![entry(0, a, 1), entry(1, b, 1)]),
                &mut ArchiveBudget::new()
            )
            .unwrap_err()
            .kind,
            "archiveRejected"
        );
    }
}

#[test]
fn strict_envelope_and_member_metadata() {
    let base = listing(vec![entry(7, "a.vpk", 15)]);
    for (key, value) in [
        ("lsarFormatVersion", 1.into()),
        ("lsarContents", object! {}),
        ("lsarProperties", JsonValue::Null),
        ("lsarFormatName", "Tar".into()),
        ("lsarEncoding", "help".into()),
        ("lsarConfidence", 2.into()),
        ("lsarError", 0.into()),
        ("lsarInnerFormatName", "Zip".into()),
    ] {
        let mut bad = base.clone();
        bad[key] = value;
        assert!(
            admitted(&bad, &mut ArchiveBudget::new()).is_err(),
            "accepted {key}"
        );
    }
    for (key, value) in [
        ("XADIsEncrypted", true.into()),
        ("XADIsCorrupted", true.into()),
        ("XADVolumeScanningFailed", true.into()),
        ("XADVolumes", array!["/snapshot", "/volume2"]),
        ("XADVolumes", array!["/other"]),
    ] {
        let mut bad = base.clone();
        bad["lsarProperties"][key] = value;
        assert!(
            admitted(&bad, &mut ArchiveBudget::new()).is_err(),
            "accepted {key}"
        );
    }
    for (key, value) in [
        ("XADIndex", (-1).into()),
        ("XADIndex", 2147483648u64.into()),
        ("XADFileSize", (-1).into()),
        ("XADFileSize", 1.5.into()),
        ("XADIsDirectory", "true".into()),
        ("XADIsLink", 2.into()),
        ("XADPosixPermissions", "regular".into()),
        ("XADLinkDestination", true.into()),
        ("XADIsEncrypted", true.into()),
    ] {
        let mut bad = base.clone();
        bad["lsarContents"][0][key] = value;
        assert!(
            admitted(&bad, &mut ArchiveBudget::new()).is_err(),
            "accepted {key}"
        );
    }
    let mut missing = entry(0, "a.vpk", 0);
    missing.remove("XADFileSize");
    assert!(member(&missing).is_err());
    missing["XADIsDirectory"] = true.into();
    assert_eq!(member(&missing).unwrap().size, 0);
    assert!(admitted(
        &listing(vec![entry(3, "a.vpk", 1), entry(3, "b.vpk", 1)]),
        &mut ArchiveBudget::new()
    )
    .is_err());
    for key in [
        "XADIsLink",
        "XADIsHardLink",
        "XADIsFIFO",
        "XADIsBlockDevice",
        "XADIsCharacterDevice",
        "XADIsResourceFork",
    ] {
        let mut special = entry(0, "special.vpk", 12);
        special[key] = 1.into();
        assert_eq!(
            member(&special).unwrap().kind,
            ArchiveKind::Rejected("linkOrSpecialEntry".into())
        );
    }
    let mut special = entry(0, "socket.vpk", 12);
    special["XADPosixPermissions"] = 0o140600.into();
    assert!(matches!(
        member(&special).unwrap().kind,
        ArchiveKind::Rejected(_)
    ));
}

#[test]
fn shared_budgets_include_exclusions_and_checked_boundaries() {
    let mut budget = ArchiveBudget::new();
    budget.admit_roots(64).unwrap();
    assert!(budget.admit_roots(1).is_err());
    budget.admit_raw_entry(".", None).unwrap();
    let mut directory = entry(0, "dir/", 0);
    directory["XADIsDirectory"] = true.into();
    let members = admitted(
        &listing(vec![
            directory,
            entry(1, "notes.txt", 12),
            entry(2, "nested.zip", 14),
            entry(3, "a.vpk", 15),
        ]),
        &mut budget,
    )
    .unwrap();
    assert_eq!((budget.entries(), budget.declared_bytes()), (5, 41));
    // The manifest only admits physical entries; nested archives and other
    // non-VPK files are excluded later by the content selector.
    assert_eq!(members[2].kind, ArchiveKind::Regular);
    budget.entries = MAX_ENTRIES - 1;
    budget.admit_raw_entry("a", None).unwrap();
    assert!(budget.admit_raw_entry("b", None).is_err());
    for _ in 0..1024 {
        budget.admit_candidate().unwrap();
    }
    assert!(budget.admit_candidate().is_err());
    let mut budget = ArchiveBudget::new();
    for _ in 0..4 {
        budget.admit_raw_entry("a", Some(MAX_FILE_BYTES)).unwrap();
    }
    assert_eq!(budget.input_bytes(), MAX_JOB_BYTES);
    assert!(budget.admit_raw_entry("a", Some(1)).is_err());
    assert!(member(&entry(0, "a.vpk", MAX_FILE_BYTES + 1)).is_err());
    let mut budget = ArchiveBudget::new();
    budget.declared_bytes = MAX_JOB_BYTES - 1;
    admitted(&listing(vec![entry(0, "excluded.txt", 1)]), &mut budget).unwrap();
    assert!(admitted(&listing(vec![entry(0, "excluded.txt", 1)]), &mut budget).is_err());
    let mut counter = u64::MAX;
    assert!(add(&mut counter, 1, u64::MAX, "overflow").is_err());
    assert!(parse(format!("{}0{}", "[".repeat(65), "]".repeat(65)).as_bytes()).is_err());
    assert!(parse(format!("{}0{}", "[".repeat(64), "]".repeat(64)).as_bytes()).is_ok());
    assert!(parse(br#"{"escaped":"[{\"}]"}"#).is_ok());
    assert!(parse(b"\xff").is_err());
}

// Independent real-engine characterization. This deliberately uses Command,
// not the production runner: it establishes CLI schema/bytes only, even inside
// a sandbox whose uid mapping prevents production executable admission.
#[test]
fn installed_zip_and_solid_7z_contract() {
    let fixture = Fixture::new();
    fixture.archives();
    for (name, format, count) in [("valid.zip", "Zip", 5), ("solid.7z", "7-Zip", 2)] {
        let source = fixture.root.join(name);
        let output = command(
            Command::new("/usr/bin/lsar")
                .args(["-ja", "-nr"])
                .arg(&source),
        );
        let listed = envelope(&output.stdout, &source).unwrap();
        assert_eq!(listed.format, format);
        let members = manifest(&listed, &mut ArchiveBudget::new()).unwrap();
        assert_eq!(members.len(), count);
        if name == "solid.7z" {
            assert!(members.iter().any(|m| flag(
                &listed.value["lsarContents"][m.index as usize],
                "XADIsSolid"
            )
            .unwrap()));
        }
        for member in members
            .iter()
            .filter(|m| m.kind == ArchiveKind::Regular && m.normalized_path.ends_with(".vpk"))
        {
            let output = command(Command::new("/usr/bin/unar").args(args(
                &["-o", "-", "-nr", "-i", "-k", "skip"],
                &source,
                Some(member.index),
                Some(&listed.encoding),
            )));
            let expected: &[u8] = if member.normalized_path.ends_with("skin_dir.vpk") {
                b"archive-payload\n"
            } else {
                b"companion\n"
            };
            assert_eq!(output.stdout, expected);
            let checked = command(Command::new("/usr/bin/lsar").args(args(
                &["-ja", "-nr", "-t", "-i"],
                &source,
                Some(member.index),
                Some(&listed.encoding),
            )));
            let checked = envelope(&checked.stdout, &source).unwrap();
            assert_eq!(checked.value["lsarContents"].len(), 1);
            assert_eq!(
                super::member(&checked.value["lsarContents"][0]).unwrap(),
                *member
            );
            assert_eq!(
                checked.value["lsarContents"][0]["lsarTestResult"].as_str(),
                Some("ok")
            );
        }
    }
    for name in ["traversal.zip", "collision.zip"] {
        let source = fixture.root.join(name);
        let output = command(
            Command::new("/usr/bin/lsar")
                .args(["-ja", "-nr"])
                .arg(&source),
        );
        assert!(manifest(
            &envelope(&output.stdout, &source).unwrap(),
            &mut ArchiveBudget::new()
        )
        .is_err());
    }
    let source = fixture.root.join("link.zip");
    let output = command(
        Command::new("/usr/bin/lsar")
            .args(["-ja", "-nr"])
            .arg(&source),
    );
    assert_eq!(
        manifest(
            &envelope(&output.stdout, &source).unwrap(),
            &mut ArchiveBudget::new()
        )
        .unwrap()[0]
            .kind,
        ArchiveKind::Rejected("linkOrSpecialEntry".into())
    );
}

#[test]
fn installed_zip_checksum_failure_is_not_stdout_success() {
    let fixture = Fixture::new();
    fixture.archives();
    let source = fixture.root.join("corrupt.zip");
    let output = command(
        Command::new("/usr/bin/unar")
            .args(["-o", "-", "-nr", "-i", "-k", "skip"])
            .arg(&source)
            .arg("0"),
    );
    assert_eq!(output.stdout, b"Archive-payload\n");
    let checked = Command::new("/usr/bin/lsar")
        .args(["-ja", "-nr", "-t", "-i"])
        .arg(&source)
        .arg("0")
        .output()
        .unwrap();
    let value = parse(&checked.stdout).unwrap();
    assert_eq!(
        value["lsarContents"][0]["lsarTestResult"].as_str(),
        Some("wrong_checksum")
    );
}

#[test]
fn installed_genuine_rar_index_integrity_and_link_exclusion() {
    let fixture = Fixture::new();
    fixture.archives();
    let source = fixture.root.join("genuine.rar");
    assert!(fs::read(&source).unwrap().starts_with(b"Rar!\x1a\x07\x00"));
    let output = command(
        Command::new("/usr/bin/lsar")
            .args(["-ja", "-nr"])
            .arg(&source),
    );
    let listed = envelope(&output.stdout, &source).unwrap();
    assert_eq!(listed.format, "RAR");
    let members = manifest(&listed, &mut ArchiveBudget::new()).unwrap();
    assert_eq!(members.len(), 5);
    assert_eq!(members[0].normalized_path, "test.txt");
    assert_eq!(members[0].size, 20);
    assert_eq!(
        members[1].kind,
        ArchiveKind::Rejected("linkOrSpecialEntry".into())
    );
    for index in [0, 2] {
        let output = command(Command::new("/usr/bin/unar").args(args(
            &["-o", "-", "-nr", "-i", "-k", "skip"],
            &source,
            Some(index),
            Some(&listed.encoding),
        )));
        assert_eq!(output.stdout, b"test text document\r\n");
        let checked = command(Command::new("/usr/bin/lsar").args(args(
            &["-ja", "-nr", "-t", "-i"],
            &source,
            Some(index),
            Some(&listed.encoding),
        )));
        let checked = envelope(&checked.stdout, &source).unwrap();
        assert_eq!(checked.value["lsarContents"].len(), 1);
        assert_eq!(
            checked.value["lsarContents"][0]["XADIndex"].as_u32(),
            Some(index)
        );
        assert_eq!(
            checked.value["lsarContents"][0]["lsarTestResult"].as_str(),
            Some("ok")
        );
    }
}

#[test]
fn installed_short_oversize_and_encrypted_fixtures_cannot_verify() {
    let fixture = Fixture::new();
    fixture.archives();
    for name in [
        "short-output.zip",
        "oversize-output.zip",
        "truncated.zip",
        "encrypted.7z",
    ] {
        let source = fixture.root.join(name);
        let checked = Command::new("/usr/bin/lsar")
            .args(["-ja", "-nr", "-t", "-i"])
            .arg(&source)
            .arg("0")
            .output()
            .unwrap();
        let verified = checked.status.success()
            && parse(&checked.stdout).is_ok_and(|v| {
                v["lsarContents"].len() == 1
                    && v["lsarContents"][0]["lsarTestResult"].as_str() == Some("ok")
            });
        assert!(!verified, "damaged/encrypted fixture verified: {name}");
    }
}

#[test]
fn source_changes_are_rejected_before_decoder_spawn() {
    for replacement in [false, true] {
        let fixture = Fixture::new();
        let original = fixture.root.join("source.zip");
        fs::write(&original, b"original archive bytes").unwrap();
        let input = ArchiveInput {
            root_ordinal: 0,
            relative_path: "source.zip".into(),
            original_path: original.clone(),
            fingerprint: paths::fingerprint(&original).unwrap(),
        };
        if replacement {
            fs::rename(&original, fixture.root.join("retained-original")).unwrap();
        }
        fs::write(&original, b"changed archive").unwrap();
        let store = fixture.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&store);
        let lease = QuarantineLease::create(&authority, "changed-source").unwrap();
        let mut budget = ArchiveBudget::new();
        budget.admit_roots(1).unwrap();
        let error = prepare_archive(input, lease.clone(), &mut budget, &PrepareControl::new())
            .err()
            .unwrap();
        assert_eq!(error.kind, "stalePreview");
        assert_eq!(fs::read(&original).unwrap(), b"changed archive");
        lease.cleanup(&authority).unwrap();
    }
}

#[test]
#[ignore = "host qualification: requires root-owned /usr/bin and decoder/SHA executables; run --ignored archive_content::tests::host_"]
fn host_prepare_retains_bytes_provenance_and_rejects_damaged_inputs() {
    let fixture = Fixture::new();
    fixture.archives();
    let store = fixture.store();
    let authority = QuarantineAuthority::for_held_jobs_store(&store);
    for name in [
        "valid.zip",
        "solid.7z",
        "corrupt.zip",
        "short-output.zip",
        "oversize-output.zip",
        "truncated.zip",
        "encrypted.7z",
    ] {
        let original = fixture.root.join(name);
        let bytes = fs::read(&original).unwrap();
        let input = ArchiveInput {
            root_ordinal: 0,
            relative_path: name.into(),
            original_path: original.clone(),
            fingerprint: paths::fingerprint(&original).unwrap(),
        };
        let fingerprint = input.fingerprint.clone();
        let lease = QuarantineLease::create(&authority, "archive-test").unwrap();
        let scratch = lease.scratch_path().unwrap();
        let mut budget = ArchiveBudget::new();
        budget.admit_roots(1).unwrap();
        let result = prepare_archive(input, lease.clone(), &mut budget, &PrepareControl::new());
        if ["valid.zip", "solid.7z"].contains(&name) {
            let prepared = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(fs::read(prepared.snapshot_path()).unwrap(), bytes);
            assert_eq!(prepared.snapshot_fingerprint().sha256, fingerprint.sha256);
            assert_eq!(prepared.extracted_members().len(), 2);
            for extracted in prepared.extracted_members() {
                assert_eq!(extracted.path().parent(), Some(scratch.as_path()));
                let expected: &[u8] =
                    if extracted.member().normalized_path.ends_with("skin_dir.vpk") {
                        b"archive-payload\n"
                    } else {
                        b"companion\n"
                    };
                assert_eq!(fs::read(extracted.path()).unwrap(), expected);
                assert_eq!(
                    extracted.fingerprint().sha256,
                    paths::digest_bytes(expected).unwrap()
                );
                assert!(prepared.members().iter().any(|m| m == extracted.member()));
            }
            assert_eq!(budget.emitted_bytes(), 26);
            drop(prepared);
        } else {
            let error = result.err().expect("damaged input accepted");
            assert!(
                matches!(
                    error.kind.as_str(),
                    "archiveRejected" | "archiveDecodeFailed" | "archiveLimitExceeded"
                ),
                "{name}: {error:?}"
            );
        }
        assert_eq!(fs::read(&original).unwrap(), bytes);
        lease.cleanup(&authority).unwrap();
        assert!(!scratch.exists());
    }
}

#[test]
fn item103_snapshot_hashes_during_copy_only() {
    let f=Fixture::new();let store=f.store();let original=f.root.join("source.vpk");
    fs::write(&original,b"verified content").unwrap();
    let input=ArchiveInput{root_ordinal:0,relative_path:"source.vpk".into(),original_path:original.clone(),fingerprint:paths::fingerprint(&original).unwrap()};
    let lease=QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&store),"item103").unwrap();
    let mut output=lease.open_claim(lease.allocate(1,&PrepareControl::new()).unwrap().remove(0)).unwrap();
    DIGEST_BYTES.with(|n|n.set(0));
    snapshot(&input,&mut output,&mut ProcessBudget::default(),&PrepareControl::new()).unwrap();
    assert_eq!(fs::read(&output.path).unwrap(),b"verified content");
    DIGEST_BYTES.with(|n|assert_eq!(n.get(),0,"snapshot must verify while copying, without separate full-file hash reads"));
}
