mod support;
use modlock_helper::{
    local_content::*,
    local_import::commit_local,
    native::{NativeError, NativeService},
    paths,
    prepare_control::PrepareControl,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use std::{fs, path::PathBuf};
use support::{import, mutation, vpk, Fixture};
fn setup() -> (Fixture, LocalImportPlan) {
    let f = Fixture::new();
    import(&mut NativeService::new(), &f);
    let source = f.root.join("local.vpk");
    fs::write(&source, vpk("models/local.vmdl_c", 91)).unwrap();
    let plan = plan(&f, vec![source], 1, None);
    (f, plan)
}
fn plan(
    f: &Fixture,
    roots: Vec<PathBuf>,
    revision: u64,
    artwork: Option<PathBuf>,
) -> LocalImportPlan {
    let prepared = prepare_local(
        LocalPrepareRequest {
            root: f.data.clone(),
            profile_id: "default".into(),
            expected_revision: revision,
            roots,
            artwork,
        },
        &PrepareControl::new(),
    )
    .unwrap();
    let mut ids = prepared
        .candidates
        .iter()
        .filter(|c| c.package.is_some())
        .map(|c| c.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    review_local(
        &prepared,
        LocalSelection {
            candidate_ids: ids,
            metadata: prepared.default_metadata(),
        },
    )
    .unwrap()
}
fn input() -> JsonValue {
    object! {method:"native.local.commit",params:object!{operationId:"local-op"}}
}
#[test]
fn local_additive_store2_preserves_every_prior_record() {
    let (f, p) = setup();
    assert!(p.artwork_ref().is_none());
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let before = s.document().unwrap();
    let game = paths::fingerprint(&f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap();
    let result = commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    assert_eq!(result["revision"], "2");
    let after = s.document().unwrap();
    for key in ["importArchive", "importLedger", "applied", "previous"] {
        assert_eq!(before[key], after[key], "{key}");
    }
    assert_eq!(before["profiles"]["other"], after["profiles"]["other"]);
    assert_eq!(
        before["profiles"]["default"]["mods"][0],
        after["profiles"]["default"]["mods"][0]
    );
    let added = &after["profiles"]["default"]["mods"][1];
    assert_eq!(added["enabled"], false);
    assert_eq!(added["source"]["status"], "downloaded");
    assert!(p.mod_id.starts_with("local-"));
    assert_eq!(added["source"]["id"], p.mod_id);
    assert_eq!(added["source"]["remoteId"], p.summary["modId"]);
    assert_eq!(
        paths::fingerprint(&f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap(),
        game
    );
    assert_eq!(
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| panic!(
            "replay writes"
        ))
        .unwrap(),
        result
    );
    assert_eq!(
        commit_local(
            &p,
            &lock,
            1,
            "local-op",
            &object! {changed:true},
            &mut |_| Ok(())
        )
        .unwrap_err()
        .kind,
        "operationIdConflict"
    );
}
#[test]
fn local_checkpoint_failures_preserve_revision_or_authoritative_success() {
    for point in [
        "journal",
        "stageCreatedUnrecorded",
        "stageRecorded",
        "memberCreatedUnrecorded",
        "memberRecorded",
        "memberCopied",
        "sourcesRevalidated",
        "published",
        "database",
        "cleanup",
    ] {
        let (f, p) = setup();
        let lock = RootLock::acquire(&f.data).unwrap();
        let s = Store::open_writer(&lock).unwrap();
        let before = s.document().unwrap();
        let mut reached = false;
        let result = commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == point {
                reached = true;
                Err(NativeError::new("nativeIo", "fixture disk failure"))
            } else {
                Ok(())
            }
        });
        assert!(reached, "{point}");
        if ["database", "cleanup"].contains(&point) {
            assert!(result.is_ok(), "{point}: {result:?}");
            assert_eq!(s.operation("local-op").unwrap()["status"], "succeeded");
        } else {
            assert!(result.is_err(), "{point}");
            assert_eq!(s.document().unwrap(), before, "{point}");
        }
        if point == "cleanup" {
            assert_eq!(s.snapshot(None).unwrap()["recoveryRequired"], true);
        }
        if ["stageCreatedUnrecorded", "memberCreatedUnrecorded"].contains(&point) {
            assert_eq!(s.journals().unwrap().len(), 1, "{point}");
        } else if s.journals().unwrap().len() > 0 {
            let revision = if ["database", "cleanup"].contains(&point) {
                2
            } else {
                1
            };
            modlock_helper::apply::recover(
                &s,
                &lock,
                revision,
                "recover-local",
                &object! {method:"native.recover"},
            )
            .unwrap();
            assert_eq!(
                s.document().unwrap()["revision"].as_str().unwrap(),
                revision.to_string()
            );
        }
    }
}
#[test]
fn local_recovery_after_process_death_does_not_edit_library() {
    let (f, p) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let before = s.document().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == "published" {
                panic!("fixture process death")
            };
            Ok(())
        })
    }));
    assert!(result.is_err());
    assert_eq!(s.journals().unwrap().len(), 1);
    let out = modlock_helper::apply::recover(
        &s,
        &lock,
        1,
        "recover-local",
        &object! {method:"native.recover"},
    )
    .unwrap();
    assert_eq!(out["revision"], "1");
    assert_eq!(s.document().unwrap(), before);
    assert_eq!(s.journals().unwrap().len(), 0);
    assert_eq!(s.operation("local-op").unwrap()["status"], "failed");
}
fn reviewed_job(s: &mut NativeService, f: &Fixture, path: &std::path::Path) -> JsonValue {
    let id = "11111111111111111111111111111111";
    s.dispatch("native.local.prepare",&object!{dataDir:f.data.to_str().unwrap(),jobId:id,profileId:"default",expectedRevision:"1",roots:json::array![path.to_str().unwrap()]}).unwrap();
    let params = object! {dataDir:f.data.to_str().unwrap(),jobId:id};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = s.dispatch("native.local.status", &params).unwrap();
        if status["phase"] == "ready" {
            let mut bulk = params.clone();
            bulk["includeCandidates"] = true.into();
            let status = s.dispatch("native.local.status", &bulk).unwrap();
            let mut ids = status["candidateSummary"]["defaultSelectedCandidateIds"]
                .members()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            ids.sort();
            return s.dispatch("native.local.review",&object!{dataDir:f.data.to_str().unwrap(),jobId:id,candidateIds:JsonValue::from(ids),metadata:status["candidateSummary"]["defaultMetadata"].clone()}).unwrap();
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn local_native_commit_replays_without_job_and_late_cancel_reports_success() {
    let (f, p) = setup();
    let mut s = NativeService::new();
    let dto = reviewed_job(&mut s, &f, &p.prepared.request.roots[0]);
    let mut params = mutation(&f, "1", "local-rpc");
    params["jobId"] = dto["jobId"].clone();
    params["token"] = dto["token"].clone();
    params["profileId"] = "default".into();
    let result = s.dispatch("native.local.commit", &params).unwrap();
    assert_eq!(result["revision"], "2");
    let job = object! {dataDir:f.data.to_str().unwrap(),jobId:dto["jobId"].clone()};
    assert_eq!(
        s.dispatch("native.local.cancel", &job).unwrap()["phase"],
        "succeeded"
    );
    s.dispatch("native.local.dismiss", &job).unwrap();
    drop(s);
    assert_eq!(
        NativeService::new()
            .dispatch("native.local.commit", &params)
            .unwrap(),
        result
    );
}
#[test]
fn local_store1_standalone_preserves_v1_and_rejects_external_family() {
    let f = Fixture::new();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let db =
        modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
    db.execute(
        "INSERT INTO library(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",
        &[support::LEGACY_STORE1],
    )
    .unwrap();
    drop(db);
    let before = s.document().unwrap();
    let standalone = f.root.join("standalone.vpk");
    fs::write(&standalone, vpk("local.bin", 1)).unwrap();
    let p = plan(&f, vec![standalone], 1, None);
    commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    let after = s.document().unwrap();
    // Local imports now publish an options catalog, which needs Store4; the
    // existing layout-1 record is carried over unchanged.
    assert_eq!(after["storeVersion"], 4);
    assert_eq!(
        before["profiles"]["default"]["mods"][0],
        after["profiles"]["default"]["mods"][0]
    );
    assert_eq!(
        modlock_helper::package::mod_packages(&after["profiles"]["default"]["mods"][1], 1)
            .unwrap()
            .len(),
        1
    );
    let directory = f.root.join("family_dir.vpk");
    fs::write(
        &directory,
        support::family_vpk(
            1,
            &[support::FamilyEntry {
                path: "external.bin",
                crc: 2,
                archive: 0,
                offset: 0,
                length: 1,
                preload: b"",
            }],
            b"",
        ),
    )
    .unwrap();
    fs::write(f.root.join("family_000.vpk"), b"a").unwrap();
    let p = plan(&f, vec![directory], 2, None);
    assert_eq!(
        commit_local(&p, &lock, 2, "external-op", &object! {}, &mut |_| panic!(
            "published unsupported family"
        ))
        .unwrap_err()
        .kind,
        "unsupportedLocalPackageForStore"
    );
    assert_eq!(s.document().unwrap(), after);
    assert_eq!(s.journals().unwrap().len(), 0);
}
#[test]
fn local_dedup_has_distinct_content_and_ownership_counts() {
    let (f, _) = setup();
    let folder = f.root.join("duplicates");
    fs::create_dir(&folder).unwrap();
    let directory = support::family_vpk(
        1,
        &[support::FamilyEntry {
            path: "distinct.bin",
            crc: 2,
            archive: 0,
            offset: 0,
            length: 1,
            preload: b"",
        }],
        b"",
    );
    for name in ["a", "b"] {
        fs::write(folder.join(format!("{name}_dir.vpk")), &directory).unwrap();
        fs::write(folder.join(format!("{name}_000.vpk")), &directory).unwrap();
    }
    let p = plan(&f, vec![folder.clone()], 1, None);
    let mut service = NativeService::new();
    let review = reviewed_job(&mut service, &f, &folder);
    assert_eq!(review["selectedPackageCount"], 2);
    // Equal bytes under different file names get separate stored copies.
    assert_eq!(review["storedPackageCount"], 1);
    assert_eq!(
        review["uniqueCopyBytes"].as_u64(),
        Some(4 * directory.len() as u64)
    );
    assert_eq!(
        review["logicalPackageBytes"].as_u64(),
        Some(4 * directory.len() as u64)
    );
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let previous = fs::read_dir(f.data.join("payloads"))
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            (p.clone(), paths::fingerprint(&p).unwrap())
        })
        .collect::<Vec<_>>();
    commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    let doc = s.document().unwrap();
    let packages =
        modlock_helper::package::mod_packages(&doc["profiles"]["default"]["mods"][1], 2).unwrap();
    assert_eq!(packages.len(), 1);
    // All valid choices (a and b) are retained, and each directory and companion
    // name gets its own stored copy.
    assert_ne!(packages[0].directory.id, packages[0].companions[&0].id);
    assert_eq!(
        fs::read_dir(f.data.join("payloads")).unwrap().count(),
        previous.len() + 4
    );
    let again = plan(&f, vec![folder], 2, None);
    assert_eq!(
        commit_local(
            &again,
            &lock,
            2,
            "duplicate-op",
            &object! {},
            &mut |_| panic!("duplicate publication")
        )
        .unwrap_err()
        .kind,
        "duplicateLocalContent"
    );
    for (p, fp) in previous {
        assert_eq!(paths::fingerprint(&p).unwrap(), fp)
    }
}
#[test]
fn local_artwork_survives_original_deletion_copy_and_hidden_projection() {
    let (f, p) = setup();
    let image = f.root.join("art.png");
    let mut png = vec![0u8; 33];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    png[8..12].copy_from_slice(&13u32.to_be_bytes());
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1u32.to_be_bytes());
    png[20..24].copy_from_slice(&1u32.to_be_bytes());
    png[24] = 8;
    png[25] = 6;
    fs::write(&image, png).unwrap();
    let p = plan(&f, p.prepared.request.roots.clone(), 1, Some(image.clone()));
    let p = review_local(
        &p.prepared,
        LocalSelection {
            candidate_ids: p.selection.candidate_ids.clone(),
            metadata: LocalMetadata {
                name: "Täht local".into(),
                author: "Fixture author".into(),
                category: "Cosmetic".into(),
                link: "https://example.invalid/local".into(),
                description: "Line one\nLine two".into(),
            },
        },
    )
    .unwrap();
    {
        let lock = RootLock::acquire(&f.data).unwrap();
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    }
    for source in &p.prepared.request.roots {
        fs::remove_file(source).unwrap();
    }
    fs::remove_file(image).unwrap();
    let mut service = NativeService::new();
    let snapshot = service.dispatch("native.snapshot", &f.params()).unwrap();
    let row = snapshot["mods"]
        .members()
        .find(|r| r["id"].as_str() == Some(&p.mod_id))
        .unwrap();
    assert_eq!(row["localArtwork"]["format"], "png");
    assert!(!row["localArtwork"].has_key("path"));
    assert!(!row["images"]
        .members()
        .any(|v| v.as_str().is_some_and(|s| s.starts_with('/'))));
    let image_id = row["localArtwork"]["id"].as_str().unwrap();
    assert!(f
        .data
        .join("artwork")
        .join(format!("{image_id}.image"))
        .is_file());
    let mut params = mutation(&f, "2", "copy-profile");
    params["action"] = "copy".into();
    params["profileId"] = "default".into();
    params["name"] = "Local copy".into();
    service.dispatch("native.profile", &params).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let mut d = s.document().unwrap();
    let original = d["profiles"]["default"]["mods"][1].clone();
    for (key, value) in [
        ("name", "Täht local"),
        ("author", "Fixture author"),
        ("category", "Cosmetic"),
        ("link", "https://example.invalid/local"),
        ("description", "Line one\nLine two"),
    ] {
        assert_eq!(original["source"][key], value);
    }
    assert!(original["source"]["createdAt"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
    assert!(d["profiles"]
        .entries()
        .any(|(id, profile)| id != "default" && profile["mods"].members().any(|r| r == &original)));
    d["profiles"]["default"]["mods"][1]["previewHidden"] = true.into();
    let snap = s
        .commit(&lock, 3, "hide", &object! {}, d, Some("default"))
        .unwrap();
    assert!(snap["mods"]
        .members()
        .find(|r| r["id"].as_str() == Some(&p.mod_id))
        .unwrap()["localArtwork"]
        .is_null());
}
#[test]
fn local_source_mutation_after_copy_cleans_owned_outputs() {
    let (f, p) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let before = s.document().unwrap();
    let result = commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
        if at == "memberCopied" {
            fs::write(&p.prepared.request.roots[0], vpk("changed.bin", 3)).unwrap();
        }
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(s.document().unwrap(), before);
    assert_eq!(s.journals().unwrap().len(), 0);
    assert_eq!(
        fs::read(&p.prepared.request.roots[0]).unwrap(),
        vpk("changed.bin", 3)
    );
}
#[test]
fn local_all_crash_boundaries_recover_only_proven_ownership() {
    for point in [
        "journal",
        "stageCreatedUnrecorded",
        "stageRecorded",
        "memberCreatedUnrecorded",
        "memberRecorded",
        "memberCopied",
        "sourcesRevalidated",
        "published",
        "database",
        "cleanup",
    ] {
        let (f, p) = setup();
        let lock = RootLock::acquire(&f.data).unwrap();
        let s = Store::open_writer(&lock).unwrap();
        let before = s.document().unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
                if at == point {
                    panic!("simulated abrupt writer death")
                };
                Ok(())
            })
        }));
        let journals = s.journals().unwrap();
        let revision = if ["database", "cleanup"].contains(&point) {
            2
        } else {
            1
        };
        let result = modlock_helper::apply::recover(
            &s,
            &lock,
            revision,
            "recover-crash",
            &object! {method:"native.recover"},
        );
        if ["stageCreatedUnrecorded", "memberCreatedUnrecorded"].contains(&point) {
            assert!(result.is_err(), "{point}");
            assert_eq!(s.journals().unwrap(), journals);
            assert_eq!(s.operation("recover-crash").unwrap()["status"], "unknown");
        } else {
            assert!(result.is_ok(), "{point}: {result:?}");
            assert_eq!(s.journals().unwrap().len(), 0);
            if revision == 1 {
                assert_eq!(s.document().unwrap(), before)
            } else {
                assert_eq!(
                    s.document().unwrap()["profiles"]["default"]["mods"].len(),
                    2
                );
                assert_eq!(
                    s.replay("local-op", &input()).unwrap().unwrap()["revision"],
                    "2"
                );
            }
        }
    }
}
#[test]
fn local_import_only_recovery_mode_survives_last_import_cleanup_and_historical_attempt() {
    let (f, _) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let before = s.document().unwrap();
    let raw = format!("\n  {}  \n", before.dump());
    let db =
        modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
    db.execute("UPDATE library SET body=? WHERE id=1", &[&raw])
        .unwrap();
    s.write_journal(
        &lock,
        "old-helper-attempt",
        &object! {kind:"recovery",version:1,input:object!{old:true},expectedRevision:"1"},
    )
    .unwrap();
    s.write_journal(&lock,"current-attempt",&object!{kind:"recovery",version:2,mode:"localImportOnly",input:object!{current:true},expectedRevision:"1"}).unwrap();
    modlock_helper::apply::recover(&s, &lock, 1, "current-attempt", &object! {current:true})
        .unwrap();
    assert_eq!(s.document().unwrap(), before);
    assert_eq!(
        db.query("SELECT body FROM library WHERE id=1", &[])
            .unwrap()
            .unwrap()[0],
        raw
    );
    assert_eq!(s.journals().unwrap().len(), 0);
    assert_eq!(
        s.operation("old-helper-attempt").unwrap()["status"],
        "succeeded"
    );
}
#[test]
fn local_cross_journal_retained_and_absent_claims_prevent_all_cleanup() {
    let (f, p) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == "database" {
                panic!("crash")
            };
            Ok(())
        })
    }));
    let local = s.journal("local-op").unwrap().unwrap();
    let file = local["files"][0].clone();
    s.write_journal(&lock,"z-legacy",&object!{kind:"import",version:1,input:object!{},stage:".import-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",payloads:json::array![file]}).unwrap();
    let journals = s.journals().unwrap();
    assert!(
        modlock_helper::apply::recover(&s, &lock, 2, "recover-conflict", &object! {}).is_err()
    );
    assert_eq!(s.journals().unwrap(), journals);
    assert_eq!(
        s.operation("recover-conflict").unwrap()["status"],
        "unknown"
    );
    assert!(f.data.join(local["stage"].as_str().unwrap()).exists());
    assert_eq!(s.operation("local-op").unwrap()["status"], "succeeded");
}
#[test]
fn local_missing_recorded_owner_is_ambiguous_even_with_matching_bytes() {
    let (f, p) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == "stageRecorded" {
                panic!("crash")
            };
            Ok(())
        })
    }));
    let journal = s.journal("local-op").unwrap().unwrap();
    let stage = f.data.join(journal["stage"].as_str().unwrap());
    fs::remove_file(stage.join("owner.json")).unwrap();
    assert!(modlock_helper::apply::recover(&s, &lock, 1, "recover-owner", &object! {}).is_err());
    assert!(stage.exists());
    assert_eq!(s.journal("local-op").unwrap(), Some(journal));
    assert_eq!(s.operation("recover-owner").unwrap()["status"], "unknown");
}
#[test]
fn local_real_process_death_at_each_boundary_is_recoverable_or_preserved() {
    if let Ok(root) = std::env::var("LOCAL_FIXTURE_CRASH_ROOT") {
        let root = PathBuf::from(root);
        let f = Fixture {
            source: root.join("upstream/state.json"),
            game: root.join("game-root"),
            data: root.join("native"),
            root,
        };
        let p = plan(&f, vec![f.root.join("local.vpk")], 1, None);
        let lock = RootLock::acquire(&f.data).unwrap();
        let point = std::env::var("LOCAL_FIXTURE_CRASH_POINT").unwrap();
        let _ = commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == point {
                std::process::exit(86)
            }
            Ok(())
        });
        panic!("checkpoint not reached");
    }
    for point in [
        "journal",
        "stageCreatedUnrecorded",
        "stageRecorded",
        "memberCreatedUnrecorded",
        "memberRecorded",
        "memberCopied",
        "sourcesRevalidated",
        "published",
        "database",
        "cleanup",
    ] {
        let (f, _) = setup();
        let before = Store::open(&f.data).unwrap().document().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_real_process_death_at_each_boundary_is_recoverable_or_preserved",
            ])
            .env("LOCAL_FIXTURE_CRASH_ROOT", &f.root)
            .env("LOCAL_FIXTURE_CRASH_POINT", point)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86), "{point}");
        let lock = RootLock::acquire(&f.data).unwrap();
        let s = Store::open_writer(&lock).unwrap();
        let journals = s.journals().unwrap();
        let revision = if ["database", "cleanup"].contains(&point) {
            2
        } else {
            1
        };
        let result =
            modlock_helper::apply::recover(&s, &lock, revision, "recover-process", &object! {});
        if ["stageCreatedUnrecorded", "memberCreatedUnrecorded"].contains(&point) {
            assert!(result.is_err());
            assert_eq!(s.journals().unwrap(), journals);
        } else {
            assert!(result.is_ok(), "{point}: {result:?}");
            assert_eq!(s.journals().unwrap().len(), 0);
            if revision == 1 {
                assert_eq!(s.document().unwrap(), before)
            } else {
                assert_eq!(
                    s.document().unwrap()["profiles"]["default"]["mods"].len(),
                    2
                )
            }
        }
    }
}
#[test]
fn local_apply_uses_existing_family_engine() {
    let (f, _) = setup();
    fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
    let directory = f.root.join("split_dir.vpk");
    fs::write(
        &directory,
        support::family_vpk(
            1,
            &[support::FamilyEntry {
                path: "new.bin",
                crc: 9,
                archive: 0,
                offset: 0,
                length: 3,
                preload: b"",
            }],
            b"",
        ),
    )
    .unwrap();
    fs::write(f.root.join("split_000.vpk"), b"abc").unwrap();
    let p = plan(&f, vec![directory], 1, None);
    {
        let lock = RootLock::acquire(&f.data).unwrap();
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    }
    let mut service = NativeService::new();
    let preview=service.dispatch("native.applyPreview", &support::current_layout(&object!{dataDir:f.data.to_str().unwrap(),profileId:"default",gamePath:f.game.to_str().unwrap()})).unwrap();
    let mut params = mutation(&f, "2", "apply-before");
    params["token"] = preview["token"].clone();
    service.dispatch("native.apply", &support::current_layout(&params)).unwrap();
    let prior = Store::open(&f.data).unwrap().document().unwrap()["applied"].clone();
    let prior_generation = f
        .game
        .join("game/citadel/deadlock_native")
        .join(prior["generation"].as_str().unwrap());
    let prior_files = modlock_helper::generation::decode(&prior["manifest"]).unwrap().files.into_iter()
        .map(|file| prior_generation.join(file.name))
        .chain(std::iter::once(prior_generation.join("manifest.json")))
        .map(|p| (p.clone(), paths::fingerprint(&p).unwrap())).collect::<Vec<_>>();
    let mut enabled = mutation(&f, "3", "enable-local");
    enabled["profileId"] = "default".into();
    enabled["modId"] = p.mod_id.clone().into();
    enabled["enabled"] = true.into();
    service.dispatch("native.setEnabled", &enabled).unwrap();
    let preview=service.dispatch("native.applyPreview", &support::current_layout(&object!{dataDir:f.data.to_str().unwrap(),profileId:"default",gamePath:f.game.to_str().unwrap()})).unwrap();
    let mut params = mutation(&f, "4", "apply-local");
    params["token"] = preview["token"].clone();
    service.dispatch("native.apply", &support::current_layout(&params)).unwrap();
    for (p, fp) in prior_files {
        assert_eq!(paths::fingerprint(&p).unwrap(), fp);
    }
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(doc["previous"]["applied"], prior);
    assert!(doc["applied"]["manifest"].dump().contains("000.vpk"));
    service
        .dispatch("native.restore", &mutation(&f, "5", "restore-local"))
        .unwrap();
    assert_eq!(
        Store::open(&f.data).unwrap().document().unwrap()["applied"],
        prior
    );
}
#[test]
fn local_altered_journal_cannot_claim_a_prior_library_payload() {
    use std::os::unix::fs::MetadataExt;
    let (f, p) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    let before = s.document().unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_local(&p, &lock, 1, "local-op", &input(), &mut |at| {
            if at == "journal" {
                panic!("crash")
            };
            Ok(())
        })
    }));
    let mut j = s.journal("local-op").unwrap().unwrap();
    let old = before["profiles"]["default"]["mods"][0]["packages"][0]["directory"].clone();
    let old_path = s.payload_path(old["id"].as_str().unwrap()).unwrap();
    let fp = paths::fingerprint(&old_path).unwrap();
    let meta = fs::metadata(&old_path).unwrap();
    j["record"]["packages"][0]["directory"] = old.clone();
    let mut row = old;
    row["role"] = "payload".into();
    row["device"] = meta.dev().into();
    row["inode"] = meta.ino().into();
    row["complete"] = true.into();
    j["files"] = json::array![row];
    s.write_journal(&lock, "local-op", &j).unwrap();
    let journals = s.journals().unwrap();
    assert!(modlock_helper::apply::recover(&s, &lock, 1, "recover-forged", &object! {}).is_err());
    assert_eq!(paths::fingerprint(&old_path).unwrap(), fp);
    assert_eq!(s.document().unwrap(), before);
    assert_eq!(s.journals().unwrap(), journals);
}

#[test]
fn local_artwork_and_equal_companion_bytes_keep_separate_ownership() {
    let (f, _) = setup();
    let image = f.root.join("cover.png");
    let mut png = vec![0u8; 33];
    png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    png[8..12].copy_from_slice(&13u32.to_be_bytes());
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1u32.to_be_bytes());
    png[20..24].copy_from_slice(&1u32.to_be_bytes());
    png[24] = 8;
    png[25] = 6;
    fs::write(&image, &png).unwrap();
    let source = f.root.join("artfamily_dir.vpk");
    fs::write(
        &source,
        support::family_vpk(
            1,
            &[support::FamilyEntry {
                path: "art.bin",
                crc: 4,
                archive: 0,
                offset: 0,
                length: 33,
                preload: b"",
            }],
            b"",
        ),
    )
    .unwrap();
    fs::write(f.root.join("artfamily_000.vpk"), &png).unwrap();
    let p = plan(&f, vec![source], 1, Some(image));
    let reviewed_artwork = p.artwork_ref().unwrap();
    assert_eq!(p.artwork_ref(), Some(reviewed_artwork.clone()));
    assert_eq!(p.clone().artwork_ref(), Some(reviewed_artwork.clone()));
    let mut detached = p.artwork_ref().unwrap();
    detached.id = "f".repeat(32);
    assert_eq!(p.artwork_ref(), Some(reviewed_artwork.clone()));
    let fresh = review_local(&p.prepared, p.selection.clone()).unwrap();
    assert_ne!(fresh.token, p.token);
    assert_ne!(fresh.artwork_ref().unwrap().id, reviewed_artwork.id);
    let reviewed_companion = &p.package_refs().unwrap()[0].companions[&0];
    assert_eq!(reviewed_artwork.sha256, reviewed_companion.sha256);
    assert_ne!(reviewed_artwork.id, reviewed_companion.id);
    let lock = RootLock::acquire(&f.data).unwrap();
    let s = Store::open_writer(&lock).unwrap();
    commit_local(&p, &lock, 1, "local-op", &input(), &mut |_| Ok(())).unwrap();
    let doc = s.document().unwrap();
    let added = &doc["profiles"]["default"]["mods"][1];
    let art = &added["source"]["localImport"]["artwork"];
    assert_eq!(art["id"].as_str(), Some(reviewed_artwork.id.as_str()));
    assert_eq!(
        art["sha256"].as_str(),
        Some(reviewed_artwork.sha256.as_str())
    );
    assert_eq!(art["size"].as_u64(), Some(reviewed_artwork.size));
    assert_eq!(
        art["sourceName"].as_str(),
        Some(reviewed_artwork.source_name.as_str())
    );
    fs::remove_file(&p.prepared.artwork.as_ref().unwrap().path).unwrap();
    assert_eq!(p.artwork_ref(), Some(reviewed_artwork));
    let layout = modlock_helper::package::layout_version(&doc).unwrap();
    let packages = modlock_helper::package::mod_packages(added, layout).unwrap();
    let companion = &packages[0].companions[&0];
    assert_eq!(art["sha256"].as_str(), Some(companion.sha256.as_str()));
    assert_ne!(art["id"].as_str(), Some(companion.id.as_str()));
    assert_eq!(
        fs::read(
            f.data
                .join("artwork")
                .join(format!("{}.image", art["id"].as_str().unwrap()))
        )
        .unwrap(),
        png
    );
    assert_eq!(
        fs::read(s.payload_path(&companion.id).unwrap()).unwrap(),
        png
    );
}
#[test]
fn local_append_order_matches_apply_fallback_and_overflow_refuses_before_files() {
    for value in [
        JsonValue::Null,
        JsonValue::from(-5i64),
        JsonValue::from(i64::MAX),
    ] {
        let (f, _) = setup();
        let lock = RootLock::acquire(&f.data).unwrap();
        let s = Store::open_writer(&lock).unwrap();
        let mut d = s.document().unwrap();
        d["profiles"]["default"]["mods"][0]["source"]["installOrder"] = value.clone();
        s.commit(&lock, 1, "order-fixture", &object! {}, d, None)
            .unwrap();
        let before = s.document().unwrap();
        let p = plan(&f, vec![f.root.join("local.vpk")], 2, None);
        let outcome = commit_local(&p, &lock, 2, "local-op", &input(), &mut |_| Ok(()));
        if value.as_i64() == Some(i64::MAX) {
            assert_eq!(outcome.unwrap_err().kind, "localInstallOrderLimit");
            assert_eq!(s.document().unwrap(), before);
            assert_eq!(s.journals().unwrap().len(), 0);
        } else {
            outcome.unwrap();
            assert_eq!(
                s.document().unwrap()["profiles"]["default"]["mods"][1]["source"]["installOrder"]
                    .as_i64(),
                Some(if value.is_null() { 1 } else { -4 })
            );
        }
    }
}

#[test]
fn installed_group_atomic_disabled_links_and_rollback() {
    let (f, a) = setup();
    let source = f.root.join("second.vpk");
    fs::write(&source, vpk("models/second.vmdl_c", 92)).unwrap();
    let mut b = plan(&f, vec![source], 1, None);
    b.selection.metadata.link = "https://gamebanana.com/mods/456".into();
    let mut group = a.clone();
    group.group = vec![("a".into(), a), ("b".into(), b)];
    let lock = RootLock::acquire(&f.data).unwrap();
    let before = Store::open_writer(&lock).unwrap().document().unwrap();
    let payloads_before=fs::read_dir(f.data.join("payloads")).unwrap().count();
    let mut published = 0;
    assert!(commit_local(&group,&lock,1,"group-fail",&object!{},&mut |point| {
        if point=="published" {published+=1; if published==1 {return Err(NativeError::new("injected","fail"));}}
        Ok(())
    }).is_err());
    assert_eq!(Store::open_writer(&lock).unwrap().document().unwrap(),before);
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(),payloads_before);
    commit_local(&group,&lock,1,"group-ok",&object!{},&mut |_|Ok(())).unwrap();
    let after=Store::open_writer(&lock).unwrap().document().unwrap();
    let rows=after["profiles"]["default"]["mods"].members().skip(before["profiles"]["default"]["mods"].len()).collect::<Vec<_>>();
    assert_eq!(rows.len(),2);
    assert!(rows.iter().all(|r|r["enabled"]==false));
    assert!(rows[0]["source"]["nativeRemoteLink"].is_null());
    assert_eq!(rows[1]["source"]["nativeRemoteLink"]["version"],2);
    assert_eq!(after["revision"],"2");
}

#[test]
fn installed_group_recovers_interrupted_publication() {
    for boundary in ["memberCopied","published","database"] {
        let (f,a)=setup();let source=f.root.join("second.vpk");fs::write(&source,vpk("models/second.vmdl_c",93)).unwrap();
        let b=plan(&f,vec![source],1,None);let mut group=a.clone();group.group=vec![("a".into(),a),("b".into(),b)];
        let lock=RootLock::acquire(&f.data).unwrap();let store=Store::open_writer(&lock).unwrap();let before=store.document().unwrap();
        let crashed=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||commit_local(&group,&lock,1,"group-crash",&object!{},&mut |point|{if point==boundary {panic!("death")};Ok(())})));
        assert!(crashed.is_err());
        let revision=if boundary=="database" {2}else{1};
        modlock_helper::apply::recover(&store,&lock,revision,"recover-group",&object!{}).unwrap();
        let after=store.document().unwrap();
        assert_eq!(after["profiles"]["default"]["mods"].len(),before["profiles"]["default"]["mods"].len()+if boundary=="database" {2}else{0});
        assert!(store.journals().unwrap().is_empty());
    }
}

#[test]
fn installed_group_one_job_review_links_drop_and_commit() {
    let (f,p)=setup();let source=f.root.join("second.vpk");fs::write(&source,vpk("models/group.vmdl_c",95)).unwrap();
    let mut service=NativeService::new();let id="22222222222222222222222222222222";
    service.dispatch("native.local.prepare",&object!{dataDir:f.data.to_str().unwrap(),jobId:id,profileId:"default",expectedRevision:"1",roots:json::array![p.prepared.request.roots[0].to_str().unwrap(),source.to_str().unwrap()]}).unwrap();
    let status_params=object!{dataDir:f.data.to_str().unwrap(),jobId:id};
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
    loop {let status=service.dispatch("native.local.status",&status_params).unwrap();if status["phase"]=="ready" {break;}assert!(std::time::Instant::now()<deadline);std::thread::sleep(std::time::Duration::from_millis(5));}
    let row=|id:&str,ordinal:usize,link:&str|object!{id:id,rootOrdinals:json::array![ordinal],metadata:object!{name:id,author:"Author",category:"Skins",description:"Description",link:link}};
    let mut params=status_params.clone();params["groupRows"]=json::array![row("first",0,""),row("second",1,"https://gamebanana.com/mods/987")];
    let review=service.dispatch("native.local.review",&params).unwrap();assert_eq!(review["groupRows"].len(),2);
    assert_eq!(service.dispatch("native.local.status",&object!{dataDir:f.data.to_str().unwrap()}).unwrap().len(),1);
    // An invalid edited link blocks the entire group and invalidates the old token.
    params["groupRows"][1]["metadata"]["link"]="invalid".into();
    let bad=service.dispatch("native.local.review",&params).unwrap();assert!(bad["token"].is_null());assert!(!bad["groupRows"][1]["problem"].is_null());
    params["groupRows"]=json::array![row("second",1,"https://gamebanana.com/mods/987")];
    let review=service.dispatch("native.local.review",&params).unwrap();
    let mut commit=mutation(&f,"1","group-rpc");commit["jobId"]=id.into();commit["profileId"]="default".into();commit["token"]=review["token"].clone();
    service.dispatch("native.local.commit",&commit).unwrap();
    let document=Store::open(&f.data).unwrap().document().unwrap();
    let record=document["profiles"]["default"]["mods"].members().find(|m|m["source"]["name"]=="second").unwrap();
    assert_eq!(record["enabled"],false);assert_eq!(record["source"]["author"],"Author");assert_eq!(record["source"]["description"],"Description");
    assert_eq!(record["source"]["nativeRemoteLink"]["submission"]["submissionId"],"987");
    assert_eq!(modlock_helper::package::mod_packages(record,2).unwrap().len(),1);
}

#[test]
fn installed_many_package_manifest_group_reviews_and_saves() {
    let f = Fixture::new();
    import(&mut NativeService::new(), &f);
    fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
    let addons = f.game.join("game/citadel/addons");
    let mut names = Vec::new();
    let mut bytes = 0u64;
    for i in 1..=20 {
        let name = format!("721867_pak{i:02}_dir.vpk");
        let mut directory = vpk("models/many.vmdl_c", i);
        // One external archive reference per package, with a four-byte companion.
        let archive_at = directory.len() - 15;
        directory[archive_at..archive_at + 2].copy_from_slice(&0u16.to_le_bytes());
        directory[archive_at + 6..archive_at + 10].copy_from_slice(&4u32.to_le_bytes());
        bytes += directory.len() as u64 + 4;
        fs::write(addons.join(&name), directory).unwrap();
        fs::write(addons.join(format!("721867_pak{i:02}_000.vpk")), b"body").unwrap();
        names.push(name);
    }
    f.write_manifest("", object!{version:3,mods:object!{"721867":object!{enabled:false,shard:1,currentVpks:json::array![],disabledVpks:JsonValue::from(names),originalVpkNames:json::array![]}}});
    let mut state = f.state();
    state["local-config"]["state"]["localMods"].push(object!{id:"721867",remoteId:"721867",name:"Many",author:"Unknown"}).unwrap();
    f.write_state(state);
    let scan = modlock_helper::installed::scan_with_source(&f.game, Some(&f.data), Some(&f.source)).unwrap();
    let row = scan["items"].members().find(|r|r["suggestedMatch"]["modId"]=="721867").unwrap();
    assert_eq!(row["suggestedMatch"]["author"], "");
    assert_eq!(row["packages"].len(), 20);
    assert_eq!(row["files"].len(), 40);
    assert_eq!(row["sizeBytes"].as_u64(), Some(bytes));
    let mut service = NativeService::new();
    let id = "33333333333333333333333333333333";
    service.dispatch("native.local.prepare", &object!{dataDir:f.data.to_str().unwrap(),jobId:id,profileId:"default",expectedRevision:"1",roots:row["packages"].clone()}).unwrap();
    let params = object!{dataDir:f.data.to_str().unwrap(),jobId:id};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = service.dispatch("native.local.status", &params).unwrap();
        if status["phase"]=="ready" {break;}
        assert_ne!(status["phase"], "failed", "{status}");
        assert!(std::time::Instant::now()<deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut review_params = params.clone();
    review_params["groupRows"] = json::array![object!{id:"many",rootOrdinals:JsonValue::from((0..20).collect::<Vec<usize>>()),metadata:object!{name:"Many",author:"",category:"Skins",description:"",link:""}}];
    let review = service.dispatch("native.local.review", &review_params).unwrap();
    assert!(review["groupRows"][0]["problem"].is_null(), "{review}");
    assert_eq!(review["groupRows"][0]["bytes"].as_u64(), Some(bytes));
    assert_eq!(review["groupRows"][0]["metadata"]["author"], "");
    review_params["groupRows"][0]["metadata"]["link"] = "invalid".into();
    let bad = service.dispatch("native.local.review", &review_params).unwrap();
    assert_eq!(bad["groupRows"][0]["problem"]["kind"], "invalidLocalMetadata");
    assert_eq!(bad["groupRows"][0]["bytes"].as_u64(), Some(bytes));
    review_params["groupRows"][0]["metadata"]["link"] = "".into();
    let review = service.dispatch("native.local.review", &review_params).unwrap();
    let mut commit = mutation(&f,"1","many-group");
    commit["jobId"] = id.into(); commit["profileId"] = "default".into(); commit["token"] = review["token"].clone();
    service.dispatch("native.local.commit", &commit).unwrap();
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    let saved = doc["profiles"]["default"]["mods"].members().find(|r|r["source"]["name"]=="Many").unwrap();
    assert_eq!(modlock_helper::package::mod_packages(saved,2).unwrap().len(),20);
    assert_eq!(saved["enabled"],false);
}

#[test]
fn local_missing_author_defaults_to_empty() {
    let (_, p) = setup();
    assert_eq!(p.prepared.default_metadata().author, "");
}

#[test]
fn local_package_limit_explains_why_import_is_blocked() {
    let (_, p) = setup();
    let ids = (0..100).map(|i|format!("{i:064x}")).collect();
    let error = review_local(&p.prepared, LocalSelection {
        candidate_ids: ids, metadata: p.selection.metadata.clone(),
    }).unwrap_err();
    assert_eq!(error.kind, "invalidLocalSelection");
    assert!(error.message.contains("more than 99 packages"), "{}", error.message);
    assert!(!error.message.contains("candidate"));
}

#[test]
fn installed_unavailable_row_keeps_observed_bytes_and_readable_problem() {
    let (f, p) = setup();
    fs::write(&p.prepared.request.roots[0], b"broken VPK").unwrap();
    let mut service = NativeService::new();
    let id = "44444444444444444444444444444444";
    service.dispatch("native.local.prepare", &object!{dataDir:f.data.to_str().unwrap(),jobId:id,profileId:"default",expectedRevision:"1",roots:json::array![p.prepared.request.roots[0].to_str().unwrap()]}).unwrap();
    let mut params = object!{dataDir:f.data.to_str().unwrap(),jobId:id};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = service.dispatch("native.local.status", &params).unwrap();
        if status["phase"]=="ready" {break;}
        assert_ne!(status["phase"], "failed", "{status}");
        assert!(std::time::Instant::now()<deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    params["groupRows"] = json::array![object!{id:"broken",rootOrdinals:json::array![0],metadata:object!{name:"Broken",author:"",category:"",description:"",link:""}}];
    let review = service.dispatch("native.local.review", &params).unwrap();
    let row = &review["groupRows"][0];
    assert_eq!(row["bytes"].as_u64(), Some(10));
    assert_eq!(row["problem"]["kind"], "invalidLocalSelection");
    assert!(row["problem"]["message"].as_str().unwrap().contains("package files"));
    assert!(review["token"].is_null());
}

#[test]
fn item48_old_receipt_recovers_without_folder_change() {
    let(f,_)=setup();
    let source=f.root.join("additional.vpk");fs::write(&source,vpk("models/new.vmdl_c",93)).unwrap();
    let p=plan(&f,vec![source],1,None);
    let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        commit_local(&p,&lock,1,"local-op",&input(),&mut |at|{if at=="database" {panic!("interrupted before cleanup");}Ok(())})
    })).is_err());
    assert_eq!(st.operation("local-op").unwrap()["status"],"succeeded");
    // An older receipt has the same projection when no folder change occurred.
    let mut old_projection=st.document().unwrap();old_projection.remove("gameFolderChanged");
    let old_result=modlock_helper::store::snapshot_document(&old_projection,Some("default")).unwrap();
    let db=modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();
    db.execute("UPDATE operations SET result=? WHERE id='local-op'",&[&object!{status:"succeeded",result:old_result}.dump()]).unwrap();drop(db);
    modlock_helper::apply::recover(&st,&lock,2,"recover-old",&object!{method:"native.recover"}).unwrap();
    assert!(st.journals().unwrap().is_empty());drop(st);drop(lock);
    assert_eq!(Store::open(&f.data).unwrap().snapshot(None).unwrap()["recoveryRequired"],false);
}

#[test]
fn item93_local_import_recovery_survives_btrfs_device_change() {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::ffi::OsStrExt;
    let (f,p)=setup();
    let path=std::ffi::CString::new(f.data.as_os_str().as_bytes()).unwrap();
    let mut stat=std::mem::MaybeUninit::<libc::statfs>::uninit();
    assert_eq!(unsafe{libc::statfs(path.as_ptr(),stat.as_mut_ptr())},0);
    if unsafe{stat.assume_init()}.f_type as u64 != 0x9123_683e {return;}
    let lock=RootLock::acquire(&f.data).unwrap();let s=Store::open_writer(&lock).unwrap();
    commit_local(&p,&lock,1,"local-op",&input(),&mut |at| if at=="cleanup" {Err(NativeError::new("nativeIo","pause cleanup"))} else {Ok(())}).unwrap();
    let mut journal=s.journal("local-op").unwrap().unwrap();
    journal["owner"].remove("storageFsid");
    let old_device=fs::metadata(&f.data).unwrap().dev()+1;
    for key in ["stageDevice","ownerDevice"] {if !journal[key].is_null(){journal[key]=old_device.into();}}
    for row in journal["files"].members_mut(){if !row["device"].is_null(){row["device"]=old_device.into();}}
    fs::write(f.data.join(journal["stage"].as_str().unwrap()).join("owner.json"),journal["owner"].dump()).unwrap();
    s.write_journal(&lock,"local-op",&journal).unwrap();
    modlock_helper::apply::recover(&s,&lock,2,"recover-local-93",&object!{method:"native.recover"}).unwrap();
    assert!(s.journals().unwrap().is_empty());
}

#[test]
fn item104_local_publication_reads_each_staged_payload_once() {
 let (f,_)=setup();let source=f.root.join("large_dir.vpk");fs::write(&source,vpk("models/large.vmdl_c",77)).unwrap();fs::OpenOptions::new().write(true).open(&source).unwrap().set_len(2*1024*1024).unwrap();let p=plan(&f,vec![source.clone()],1,None);
 // Count actual read syscalls as well as SHA reads, including immediate cleanup.
 let rchar=|| fs::read_to_string("/proc/thread-self/io").unwrap().lines().find_map(|l|l.strip_prefix("rchar: ").and_then(|n|n.parse::<u64>().ok())).unwrap();
 let lock=RootLock::acquire(&f.data).unwrap();let before=rchar();
 commit_local(&p,&lock,1,"local-op",&input(),&mut |_| Ok(())).unwrap();
 let bytes=rchar()-before;
 assert!(bytes < 1_000_000+fs::metadata(source).unwrap().len()*3,"publication read {bytes} bytes");
}
