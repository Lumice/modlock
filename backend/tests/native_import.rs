mod support;
use support::*;
#[test]
fn native_reads_and_import_preview_never_create_owned_library() {
    let f = Fixture::new();
    assert_eq!(
        rpc("native.snapshot", f.params())["error"]["data"]["kind"],
        "nativeNotImported"
    );
    assert!(!f.data.exists());
    let r = rpc("native.importPreview", f.preview_params());
    assert_eq!(r["result"]["profileCount"], 2, "{}", r.dump());
    assert_eq!(r["result"]["modCount"], 2);
    assert!(!f.data.exists());
}
use modlock_helper::{native::NativeService, store::Store};
use json::{object, JsonValue};
use std::fs;
#[test]
fn family_changed_staged_member_never_commits_a_library() {
    use modlock_helper::{import::ImportPlan, store::RootLock};
    for replace in [false, true] {
        let f = Fixture::new();
        family_fixture(&f, false);
        let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        let mut changed = false;
        let result = plan.commit_with(&lock, "stage-changed", &object! {}, &mut |at| {
            if at != "memberCopied" || changed {
                return Ok(());
            }
            let store = Store::open_history(&f.data).unwrap();
            let j = store.journal("stage-changed").unwrap().unwrap();
            if let Some(p) = j["payloads"]
                .members()
                .find(|p| p["complete"] == true && p["size"] == 3)
            {
                let path = f
                    .data
                    .join(j["stage"].as_str().unwrap())
                    .join(format!("{}.vpk", p["id"].as_str().unwrap()));
                if replace {
                    fs::rename(&path, path.with_extension("keep")).unwrap();
                    fs::write(path, b"one").unwrap();
                } else {
                    fs::write(path, b"NEW").unwrap();
                }
                changed = true;
            }
            Ok(())
        });
        assert!(changed);
        assert_eq!(result.unwrap_err().kind, "recoveryRequired");
        assert!(
            Store::open_history(&f.data).unwrap().document().is_err(),
            "changed staged member must not become an authoritative library"
        );
    }
}
#[test]
fn family_recovery_preserves_unknown_replaced_and_undurable_members() {
    use modlock_helper::{import::ImportPlan, native::NativeError, store::RootLock};
    for case in ["unknown", "replaced", "undurable"] {
        let f = Fixture::new();
        family_fixture(&f, false);
        let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        let mut interrupted = false;
        let error = plan
            .commit_with(&lock, "interrupted-family", &object! {}, &mut |at| {
                let target = if case == "undurable" {
                    "createdUnrecorded"
                } else {
                    "memberCopied"
                };
                if at != target || interrupted {
                    return Ok(());
                }
                interrupted = true;
                let store = Store::open_history(&f.data).unwrap();
                let j = store.journal("interrupted-family").unwrap().unwrap();
                let stage = f.data.join(j["stage"].as_str().unwrap());
                if case == "unknown" {
                    fs::write(stage.join("unknown"), b"keep").unwrap();
                }
                if case == "replaced" {
                    let id = j["payloads"]
                        .members()
                        .find(|p| p["complete"] == true)
                        .unwrap()["id"]
                        .as_str()
                        .unwrap();
                    let path = stage.join(format!("{id}.vpk"));
                    let bytes = fs::read(&path).unwrap();
                    fs::rename(&path, stage.join("original.keep")).unwrap();
                    fs::write(path, bytes).unwrap();
                }
                Err(NativeError::new("nativeIo", "interrupted fixture"))
            })
            .unwrap_err();
        assert_eq!(error.kind, "recoveryRequired", "{case}");
        let store = Store::open_writer(&lock).unwrap();
        let journal = store.journal("interrupted-family").unwrap().unwrap();
        let stage = f.data.join(journal["stage"].as_str().unwrap());
        let names: Vec<_> = fs::read_dir(&stage)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        for _ in 0..2 {
            assert_eq!(
                modlock_helper::import::reconcile_imports(&store, &lock)
                    .unwrap_err()
                    .kind,
                "recoveryRequired"
            );
            assert_eq!(fs::read_dir(&stage).unwrap().count(), names.len());
        }
        assert_eq!(
            store.operation("interrupted-family").unwrap()["status"],
            "recoveryRequired"
        );
        if case == "unknown" {
            fs::remove_file(stage.join("unknown")).unwrap();
            modlock_helper::import::reconcile_imports(&store, &lock).unwrap();
            assert!(!stage.exists());
            assert_eq!(
                store.operation("interrupted-family").unwrap()["error"]["kind"],
                "operationInterrupted"
            );
        }
    }
}

#[test]
fn family_committed_cleanup_recovers_without_a_legacy_game_journal() {
    use modlock_helper::{import::ImportPlan, native::NativeError, store::RootLock};
    let f = Fixture::new();
    family_fixture(&f, false);
    let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let error = plan
        .commit_with(&lock, "committed-family", &object! {}, &mut |at| {
            if at != "database" {
                return Ok(());
            }
            let store = Store::open_history(&f.data).unwrap();
            let journal = store.journal("committed-family").unwrap().unwrap();
            let stage = f.data.join(journal["stage"].as_str().unwrap());
            let first = journal["payloads"][0]["id"].as_str().unwrap();
            fs::remove_file(stage.join(format!("{first}.vpk"))).unwrap();
            fs::write(stage.join("unknown"), b"keep").unwrap();
            Err(NativeError::new("nativeIo", "after commit"))
        })
        .unwrap_err();
    assert_eq!(error.kind, "recoveryRequired");
    let store = Store::open_history(&f.data).unwrap();
    assert_eq!(
        store.operation("committed-family").unwrap()["status"],
        "succeeded"
    );
    let journal = store.journal("committed-family").unwrap().unwrap();
    let stage = f.data.join(journal["stage"].as_str().unwrap());
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 3);
    fs::remove_file(stage.join("unknown")).unwrap();
    drop(store);
    drop(lock);
    let mut service = NativeService::new();
    let q = mutation(&f, "1", "recover-family");
    let snap = service.dispatch("native.recover", &q).unwrap();
    assert_eq!(snap["storeVersion"], 2);
    assert!(!stage.exists());
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 3);
    assert_eq!(service.dispatch("native.recover", &q).unwrap(), snap);
}
#[test]
fn family_association_ownership_allows_hardlinks_and_rejects_ambiguity() {
    for case in [
        "hardlink",
        "duplicate-original",
        "case-original",
        "case-current",
        "orphan",
        "double-claim",
        "parseable-member",
    ] {
        let f = Fixture::new();
        let files = family_fixture(&f, false);
        let mut m = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        match case {
            "hardlink" => {
                fs::remove_file(&files[1]).unwrap();
                fs::hard_link(&files[2], &files[1]).unwrap();
            }
            "duplicate-original" => {
                m["mods"]["remote-a"]["originalVpkNames"][1] = "skin_dir.vpk".into()
            }
            "case-original" => m["mods"]["remote-a"]["originalVpkNames"][1] = "SKIN_DIR.VPK".into(),
            "case-current" => {
                let mut second = m["mods"]["remote-a"].clone();
                for name in second["currentVpks"].members_mut() {
                    *name = name.as_str().unwrap().to_ascii_uppercase().into();
                }
                m["mods"]["extra"] = second;
            }
            "orphan" => {
                fs::write(files[0].parent().unwrap().join("orphan.vpk"), b"opaque").unwrap();
                m["mods"]["remote-a"]["currentVpks"]
                    .push("orphan.vpk")
                    .unwrap();
                m["mods"]["remote-a"]["originalVpkNames"]
                    .push("skin_999.vpk")
                    .unwrap();
            }
            "double-claim" => {
                let path = files[0].parent().unwrap().join("second.vpk");
                fs::copy(&files[0], &path).unwrap();
                m["mods"]["remote-a"]["currentVpks"]
                    .push("second.vpk")
                    .unwrap();
                m["mods"]["remote-a"]["originalVpkNames"]
                    .push("skin.vpk")
                    .unwrap();
            }
            _ => {
                fs::write(&files[1], vpk("models/embedded.vmdl_c", 1)).unwrap();
            }
        }
        f.write_manifest("", m);
        let mut service = NativeService::new();
        let p = preview(&mut service, &f);
        let allowed = matches!(case, "hardlink" | "parseable-member");
        assert_eq!(
            p["availableModCount"],
            if allowed { 1 } else { 0 },
            "{case}: {}",
            p.dump()
        );
        if allowed {
            assert_eq!(p["packageCount"], 1);
            assert_eq!(
                p["physicalFileCount"],
                if case == "hardlink" { 2 } else { 3 }
            );
            import(&mut service, &f);
            let doc = Store::open(&f.data).unwrap().document().unwrap();
            let evidence = &doc["importLedger"]["sourcePayloads"];
            for path in &files {
                assert!(evidence
                    .members()
                    .any(|e| e["sourcePath"].as_str() == path.to_str()));
            }
        }
    }
}

#[test]
fn family_physical_members_do_not_consume_the_99_directory_limit() {
    let f = Fixture::new();
    family_fixture(&f, false);
    let root = f.game.join("game/citadel/addons");
    let mut current = vec!["many.vpk".to_owned()];
    let mut original = vec!["many_dir.vpk".to_owned()];
    let names: Vec<_> = (0..100).map(|i| format!("models/{i}.vmdl_c")).collect();
    let entries: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, n)| FamilyEntry {
            path: n,
            crc: i as u32,
            archive: i as u16,
            offset: 0,
            length: 1,
            preload: b"",
        })
        .collect();
    fs::write(root.join("many.vpk"), family_vpk(1, &entries, b"")).unwrap();
    for i in 0..100 {
        let name = format!("arbitrary-{i}.vpk");
        fs::write(root.join(&name), [i as u8]).unwrap();
        current.push(name);
        original.push(format!("many_{i:03}.vpk"));
    }
    f.write_manifest("",object!{version:3,mods:object!{"remote-a":object!{enabled:true,shard:1,currentVpks:JsonValue::from(current),disabledVpks:json::array![],originalVpkNames:JsonValue::from(original)}}});
    let mut service = NativeService::new();
    let p = preview(&mut service, &f);
    assert_eq!(p["packageCount"], 1);
    assert_eq!(p["physicalFileCount"], 101);
    import(&mut service, &f);
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 101);
}

#[test]
fn unknown_import_journal_version_preflights_before_any_cleanup() {
    let f = Fixture::new();
    let lock = modlock_helper::store::RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let stage = f.data.join(".import-0123456789abcdef0123456789abcdef");
    fs::create_dir(&stage).unwrap();
    store.write_journal(&lock,"a-legacy",&object!{kind:"import",stage:stage.file_name().unwrap().to_str().unwrap(),input:object!{},payloads:json::array![]}).unwrap();
    store.write_journal(&lock,"z-future",&object!{kind:"import",version:99,stage:".import-family-0123456789abcdef0123456789abcdef",input:object!{},payloads:json::array![]}).unwrap();
    assert_eq!(
        modlock_helper::import::reconcile_imports(&store, &lock)
            .unwrap_err()
            .kind,
        "recoveryRequired"
    );
    assert!(stage.exists());
    assert_eq!(store.journals().unwrap().len(), 2);
}

#[test]
fn all_supported_import_journals_are_validated_before_any_cleanup() {
    use modlock_helper::{paths, store::RootLock};
    use std::os::unix::fs::MetadataExt;
    for earlier_version in [1, 2] {
        for damage in ["stage-identity", "member-identity", "unknown-file"] {
            let f = Fixture::new();
            let lock = RootLock::acquire(&f.data).unwrap();
            let store = Store::open_writer(&lock).unwrap();
            let first = f.data.join(if earlier_version == 1 {
                ".import-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            } else {
                ".import-family-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            });
            let later = f
                .data
                .join(".import-family-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
            for (id, stage, version) in [
                ("a-valid", &first, earlier_version),
                ("z-changed", &later, 2),
            ] {
                fs::create_dir(stage).unwrap();
                let name = stage.file_name().unwrap().to_str().unwrap();
                let body = if version == 1 {
                    object! {kind:"import",stage:name,input:object!{},payloads:json::array![]}
                } else {
                    let file_id = if id == "a-valid" {
                        "c".repeat(32)
                    } else {
                        "d".repeat(32)
                    };
                    let member = stage.join(format!("{file_id}.vpk"));
                    fs::write(&member, vpk("models/a.vmdl_c", 1)).unwrap();
                    let fp = paths::fingerprint(&member).unwrap();
                    let file =
                        object! {id:file_id,sha256:fp.sha256,size:fp.size,sourceName:"owned.vpk"};
                    let owner = object! {kind:"import",version:2,operationId:id,stage:name,nonce:"e".repeat(32)};
                    fs::write(stage.join("owner.json"), owner.dump()).unwrap();
                    let sm = fs::metadata(stage).unwrap();
                    let om = fs::metadata(stage.join("owner.json")).unwrap();
                    let mut owned = file.clone();
                    owned["device"] = fp.device.into();
                    owned["inode"] = fp.inode.into();
                    owned["complete"] = true.into();
                    object! {kind:"import",version:2,stage:name,input:object!{},owner:owner,stageDevice:sm.dev(),stageInode:sm.ino(),ownerDevice:om.dev(),ownerInode:om.ino(),packages:json::array![object!{directory:file.clone(),companions:json::array![]}],payloads:json::array![owned]}
                };
                if version == 1 {
                    fs::write(stage.join("owner.json"), body.dump()).unwrap();
                }
                store.write_journal(&lock, id, &body).unwrap();
            }
            let mut bad = store.journal("z-changed").unwrap().unwrap();
            match damage {
                "stage-identity" => bad["stageInode"] = 0.into(),
                "member-identity" => bad["payloads"][0]["inode"] = 0.into(),
                _ => {
                    fs::write(later.join("unknown"), b"preserve").unwrap();
                }
            }
            store.write_journal(&lock, "z-changed", &bad).unwrap();
            let journals = store.journals().unwrap();
            let outcomes = [
                store.operation("a-valid").unwrap(),
                store.operation("z-changed").unwrap(),
            ];
            let preserved: Vec<_> = [&first, &later]
                .into_iter()
                .flat_map(|dir| {
                    fs::read_dir(dir).unwrap().map(|e| {
                        let p = e.unwrap().path();
                        let b = fs::read(&p).unwrap();
                        let m = fs::metadata(&p).unwrap();
                        (p, b, m.ino(), m.modified().unwrap())
                    })
                })
                .collect();
            for _ in 0..2 {
                assert_eq!(
                    modlock_helper::import::reconcile_imports(&store, &lock)
                        .unwrap_err()
                        .kind,
                    "recoveryRequired"
                );
                assert!(
                    first.exists(),
                    "earlier v{earlier_version} stage changed before {damage} refusal"
                );
                assert!(later.exists());
                assert_eq!(store.journals().unwrap(), journals);
                assert_eq!(
                    [
                        store.operation("a-valid").unwrap(),
                        store.operation("z-changed").unwrap()
                    ],
                    outcomes
                );
                for (p, b, ino, mtime) in &preserved {
                    assert_eq!(&fs::read(p).unwrap(), b);
                    let m = fs::metadata(p).unwrap();
                    assert_eq!(m.ino(), *ino);
                    assert_eq!(m.modified().unwrap(), *mtime);
                }
            }
            match damage {
                "stage-identity" => bad["stageInode"] = fs::metadata(&later).unwrap().ino().into(),
                "member-identity" => {
                    let id = bad["payloads"][0]["id"].as_str().unwrap();
                    bad["payloads"][0]["inode"] = fs::metadata(later.join(format!("{id}.vpk")))
                        .unwrap()
                        .ino()
                        .into();
                }
                _ => std::fs::remove_file(later.join("unknown")).unwrap(),
            }
            store.write_journal(&lock, "z-changed", &bad).unwrap();
            modlock_helper::import::reconcile_imports(&store, &lock).unwrap();
            assert!(!first.exists());
            assert!(!later.exists());
            assert_eq!(store.journals().unwrap().len(), 0);
            for id in ["a-valid", "z-changed"] {
                assert_eq!(
                    store.operation(id).unwrap()["error"]["kind"],
                    "operationInterrupted"
                );
            }
        }
    }
}
#[test]
fn family_optional_dir_stem_and_preload_only_are_available() {
    for preload_only in [false, true] {
        let f = Fixture::new();
        let files = family_fixture(&f, false);
        let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        if preload_only {
            fs::write(
                &files[0],
                family_vpk(
                    1,
                    &[FamilyEntry {
                        path: "models/only.vmdl_c",
                        crc: 1,
                        archive: 0,
                        offset: u32::MAX,
                        length: 0,
                        preload: b"preload",
                    }],
                    b"",
                ),
            )
            .unwrap();
            manifest["mods"]["remote-a"]["currentVpks"] =
                json::array![files[0].file_name().unwrap().to_str().unwrap()];
            manifest["mods"]["remote-a"]["originalVpkNames"] = json::array!["skin.vpk"];
        } else {
            // The companion precedes its directory; positional manifest order
            // cannot accidentally turn it into an independently parsed package.
            manifest["mods"]["remote-a"]["currentVpks"] = json::array![
                files[1].file_name().unwrap().to_str().unwrap(),
                files[0].file_name().unwrap().to_str().unwrap(),
                files[2].file_name().unwrap().to_str().unwrap()
            ];
            manifest["mods"]["remote-a"]["originalVpkNames"] =
                json::array!["skin_000.vpk", "skin.vpk", "skin_017.vpk"];
        }
        f.write_manifest("", manifest);
        let mut service = NativeService::new();
        let p = preview(&mut service, &f);
        assert_eq!(p["availableModCount"], 1, "{}", p.dump());
        assert_eq!(p["packageCount"], 1);
        assert_eq!(p["physicalFileCount"], if preload_only { 1 } else { 3 });
        let snap = import(&mut service, &f);
        assert_eq!(snap["mods"][0]["available"], true);
    }
}
#[test]
fn changed_companion_invalidates_import_preview() {
    for change in ["bytes", "missing", "symlink", "inode"] {
        let f = Fixture::new();
        let files = family_fixture(&f, false);
        let mut service = NativeService::new();
        let p = preview(&mut service, &f);
        match change {
            "bytes" => fs::write(&files[1], b"NEW").unwrap(),
            "missing" => fs::remove_file(&files[1]).unwrap(),
            "symlink" => {
                fs::remove_file(&files[1]).unwrap();
                std::os::unix::fs::symlink(&files[2], &files[1]).unwrap();
            }
            _ => {
                fs::rename(&files[1], files[1].with_extension("old")).unwrap();
                fs::write(&files[1], b"one").unwrap();
            }
        }
        let mut q = mutation(&f, "0", "changed-family");
        q["token"] = p["token"].clone();
        assert_eq!(
            service
                .dispatch("native.importCommit", &q)
                .unwrap_err()
                .kind,
            "stalePreview"
        );
        assert!(Store::open(&f.data).is_err());
    }
}

#[test]
fn family_import_faults_keep_only_owned_files() {
    use modlock_helper::{import::ImportPlan, native::NativeError, store::RootLock};
    for point in ["created", "memberCopied", "published"] {
        let f = Fixture::new();
        family_fixture(&f, false);
        let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        fs::create_dir_all(f.data.join("payloads")).unwrap();
        fs::write(f.data.join("payloads/unknown.vpk"), b"keep").unwrap();
        let error = plan
            .commit_with(&lock, "fault", &object! {}, &mut |at| {
                if at == point {
                    Err(NativeError::new("nativeIo", "injected"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.kind, "nativeIo");
        let st = Store::open_writer(&lock).unwrap();
        assert!(st.document().is_err());
        assert_eq!(st.journals().unwrap().len(), 0);
        assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 1);
        assert_eq!(
            fs::read(f.data.join("payloads/unknown.vpk")).unwrap(),
            b"keep"
        );
    }
}

#[test]
fn family_identity_includes_companions() {
    let f = Fixture::new();
    let files = family_fixture(&f, false);
    let mut state = f.state();
    let mut second = state["local-config"]["state"]["localMods"][0].clone();
    second["id"] = "second".into();
    second["remoteId"] = "second".into();
    state["local-config"]["state"]["localMods"]
        .push(second)
        .unwrap();
    f.write_state(state);
    let root = files[0].parent().unwrap();
    for (from, to) in files
        .iter()
        .zip(["second-dir.vpk", "second-zero.vpk", "second-other.vpk"])
    {
        fs::copy(from, root.join(to)).unwrap();
    }
    let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
    manifest["mods"]["second"] = object! {enabled:true,shard:1,currentVpks:json::array!["second-dir.vpk","second-zero.vpk","second-other.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["skin_dir.vpk","skin_000.vpk","skin_017.vpk"]};
    f.write_manifest("", manifest);
    let mut service = NativeService::new();
    let p = preview(&mut service, &f);
    assert_eq!(p["packageCount"], 2);
    assert_eq!(p["directoryCount"], 1);
    assert_eq!(p["physicalFileCount"], 3);
    fs::write(root.join("second-zero.vpk"), b"NEW").unwrap();
    let p = preview(&mut service, &f);
    assert_eq!(p["directoryCount"], 2);
    assert_eq!(p["physicalFileCount"], 4);
    fs::remove_file(root.join("second-other.vpk")).unwrap();
    let p = preview(&mut service, &f);
    assert_eq!(p["availableModCount"], 1);
    assert_eq!(p["physicalFileCount"], 3);
    import(&mut service, &f);
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 3);
    let copied_bytes: u64 = fs::read_dir(f.data.join("payloads"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert_eq!(p["totalBytes"].as_u64(), Some(copied_bytes));
}
#[test]
fn imports_enabled_disabled_renamed_family_from_exact_associations() {
    for disabled in [false, true] {
        let f = Fixture::new();
        let files = family_fixture(&f, disabled);
        let preserved: Vec<_> = files
            .iter()
            .chain([&f.source, &f.manifest_path("")])
            .map(|p| {
                (
                    p.clone(),
                    fs::read(p).unwrap(),
                    fs::metadata(p).unwrap().modified().unwrap(),
                )
            })
            .collect();
        let mut service = NativeService::new();
        let review = preview(&mut service, &f);
        assert_eq!(review["availableModCount"], 1, "{}", review.dump());
        assert_eq!(review["packageCount"], 1);
        assert_eq!(review["directoryCount"], 1);
        assert_eq!(review["physicalFileCount"], 3);
        assert_eq!(review["companionFileCount"], 2);
        assert_eq!(
            review["totalBytes"].as_u64(),
            Some(files.iter().map(|p| fs::metadata(p).unwrap().len()).sum())
        );
        let snap = import(&mut service, &f);
        assert_eq!(snap["storeVersion"], 2);
        let store = Store::open(&f.data).unwrap();
        let doc = store.document().unwrap();
        let package = &doc["profiles"]["default"]["mods"][0]["packages"][0];
        assert_eq!(package["companions"][0]["archiveIndex"], 0);
        assert_eq!(package["companions"][1]["archiveIndex"], 17);
        assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 3);
        for entry in fs::read_dir(f.data.join("payloads")).unwrap() {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o400
            );
        }
        assert_eq!(doc["importLedger"]["packageEvidenceVersion"], 2);
        let ledger = doc["importLedger"].dump();
        for (i, p) in files.iter().enumerate() {
            assert!(ledger.contains(p.to_str().unwrap()));
            assert!(ledger.contains(["skin_dir.vpk", "skin_000.vpk", "skin_017.vpk"][i]));
        }
        for (p, b, t) in preserved {
            assert_eq!(fs::read(&p).unwrap(), b);
            assert_eq!(fs::metadata(p).unwrap().modified().unwrap(), t);
        }
    }
}

#[test]
fn family_import_preserves_ids_profiles_provenance_and_unavailable() {
    let f = Fixture::new();
    family_fixture(&f, false);
    let mut state = f.state();
    let m = state["local-config"]["state"]["localMods"][0].clone();
    state["local-config"]["state"]["localMods"].push(object!{id:"missing",remoteId:"missing",name:"Missing",status:"disabled",unknown:object!{keep:true}}).unwrap();
    f.write_state(state);
    let mut service = NativeService::new();
    let snap = import(&mut service, &f);
    assert_eq!(snap["storeVersion"], 2);
    assert_eq!(snap["mods"][0]["id"], "local-a");
    assert_eq!(snap["mods"][0]["title"], "Working A");
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(doc["profiles"]["default"]["mods"][0]["source"], m);
    assert_eq!(doc["profiles"]["default"]["mods"][1]["available"], false);
    assert_eq!(doc["profiles"]["default"]["mods"][1]["packages"].len(), 0);
    assert_eq!(
        doc["profiles"]["default"]["mods"][1]["source"]["unknown"]["keep"],
        true
    );
    let mut q = mutation(&f, "1", "copy-family");
    q["action"] = "copy".into();
    q["profileId"] = "default".into();
    q["name"] = "Copied".into();
    let copied = service.dispatch("native.profile", &q).unwrap();
    let after = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(
        after["profiles"][copied["selectedProfileId"].as_str().unwrap()]["mods"],
        doc["profiles"]["default"]["mods"]
    );
}
#[test]
fn imports_all_profiles_working_precedence_archive_and_immutable_copies() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let before = fs::read(&f.source).unwrap();
    let mtime = fs::metadata(&f.source).unwrap().modified().unwrap();
    let original = f.game.join("game/citadel/addons/pak01_dir.vpk");
    let bytes = fs::read(&original).unwrap();
    let pm = fs::metadata(&original).unwrap().modified().unwrap();
    let p = preview(&mut s, &f);
    assert_eq!(p["availableModCount"], 2);
    assert_eq!(p["unavailableModCount"], 0);
    let v = import(&mut s, &f);
    assert_eq!(v["sourceKind"], "native");
    assert_eq!(v["revision"], "1");
    assert_eq!(v["mods"][0]["id"], "local-a");
    assert_eq!(v["mods"][0]["title"], "Working A");
    assert_eq!(v["mods"][0]["enabled"], true);
    assert_eq!(v["mods"][0]["appliedEnabled"], false);
    assert_eq!(v["appliedProfileId"], JsonValue::Null);
    assert_eq!(v["pendingChanges"], true);
    let store = Store::open(&f.data).unwrap();
    let doc = store.document().unwrap();
    assert!(!doc.dump().contains("AUTH_SENTINEL"));
    assert_eq!(
        doc["importArchive"]["profiles"]["default"]["unknownProfile"]["preserve"],
        true
    );
    assert_eq!(
        doc["importArchive"]["localMods"][0]["variants"][0]["fileId"],
        9
    );
    assert_eq!(doc["importArchive"]["groups"][0]["id"], "group-a");
    assert_eq!(
        doc["importArchive"]["profiles"]["default"]["mods"][0]["id"],
        "stale"
    );
    let payload = &doc["profiles"]["default"]["mods"][0]["packages"][0]["directory"];
    let copy = store.payload_path(payload["id"].as_str().unwrap()).unwrap();
    assert_eq!(fs::read(copy).unwrap(), bytes);
    assert_eq!(fs::read(&f.source).unwrap(), before);
    assert_eq!(fs::metadata(&f.source).unwrap().modified().unwrap(), mtime);
    assert_eq!(fs::read(original.clone()).unwrap(), bytes);
    assert_eq!(fs::metadata(original).unwrap().modified().unwrap(), pm);
    let mut q = f.params();
    q["profileId"] = "other".into();
    let other = s.dispatch("native.snapshot", &q).unwrap();
    assert_eq!(other["mods"][0]["id"], "local-b");
    assert_eq!(other["mods"][0]["available"], true);
    assert_eq!(other["mods"][0]["enabled"], false);
    assert_eq!(other["activeProfileId"], "default");
}
#[test]
fn source_manifest_or_payload_edits_invalidate_review_without_committed_import() {
    for part in ["source", "manifest", "payload"] {
        let f = Fixture::new();
        let mut s = NativeService::new();
        let p = preview(&mut s, &f);
        match part {
            "source" => {
                let mut state = f.state();
                state["local-config"]["state"]["localMods"][0]["name"] = "Edited".into();
                f.write_state(state)
            }
            "manifest" => {
                let path = f.manifest_path("");
                let mut v = json::parse(&fs::read_to_string(&path).unwrap()).unwrap();
                v["mods"]["remote-a"]["order"] = 8.into();
                fs::write(path, v.dump()).unwrap()
            }
            _ => fs::write(
                f.game.join("game/citadel/addons/pak01_dir.vpk"),
                vpk("models/a.vmdl_c", 9),
            )
            .unwrap(),
        }
        let mut q = mutation(&f, "0", "import-1");
        q["token"] = p["token"].clone();
        assert_eq!(
            s.dispatch("native.importCommit", &q).unwrap_err().kind,
            "stalePreview"
        );
        assert!(Store::open(&f.data).is_err());
    }
}
#[test]
fn malformed_versions_and_manifest_fail_before_copying() {
    for (part, version) in [("source", 34), ("manifest", 4)] {
        let f = Fixture::new();
        if part == "source" {
            let mut v = f.state();
            v["local-config"]["version"] = version.into();
            f.write_state(v)
        } else {
            f.write_manifest("", object! {version:version,mods:object!{}})
        };
        let mut s = NativeService::new();
        assert_eq!(
            s.dispatch("native.importPreview", &f.preview_params())
                .unwrap_err()
                .kind,
            "unsupportedVersion"
        );
        assert!(!f.data.exists());
    }
    let f = Fixture::new();
    fs::write(f.manifest_path(""), b"{").unwrap();
    let mut s = NativeService::new();
    assert_eq!(
        s.dispatch("native.importPreview", &f.preview_params())
            .unwrap_err()
            .kind,
        "invalidManifest"
    );
    assert!(!f.data.exists());
}
#[test]
fn missing_unsafe_ambiguous_split_and_symlink_payloads_stay_visible_unavailable() {
    for case in [
        "missing",
        "unsafe",
        "ambiguous",
        "split",
        "symlink",
        "folder",
    ] {
        let f = Fixture::new();
        let current = f.game.join("game/citadel/addons/pak01_dir.vpk");
        let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        match case {
            "missing" => fs::remove_file(&current).unwrap(),
            "unsafe" => {
                manifest["mods"]["remote-a"]["currentVpks"][0] = "../escape.vpk".into();
                f.write_manifest("", manifest)
            }
            "ambiguous" => {
                manifest["mods"]["remote-other"] = manifest["mods"]["remote-a"].clone();
                f.write_manifest("", manifest)
            }
            "split" => {
                let mut b = vpk("models/a.vmdl_c", 7);
                let at = 12 + "vmdl_c".len() + 1 + "models".len() + 1 + "a".len() + 1 + 6;
                b[at..at + 2].copy_from_slice(&0u16.to_le_bytes());
                b[at + 6..at + 10].copy_from_slice(&1u32.to_le_bytes());
                fs::write(&current, b).unwrap()
            }
            "symlink" => {
                fs::remove_file(&current).unwrap();
                std::os::unix::fs::symlink(&f.source, &current).unwrap()
            }
            _ => {
                let mut v = f.state();
                v["local-config"]["state"]["profiles"]["default"]["folderName"] =
                    "../outside".into();
                f.write_state(v)
            }
        }
        let mut s = NativeService::new();
        let p = preview(&mut s, &f);
        assert_eq!(p["unavailableModCount"], 1, "{case}: {}", p.dump());
        let v = import(&mut s, &f);
        assert_eq!(v["mods"][0]["available"], false);
        assert!(!v["mods"][0]["unavailableReason"]
            .as_str()
            .unwrap()
            .is_empty());
    }
}
#[test]
fn copy_failure_and_invalid_token_never_commit_a_library() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let mut q = mutation(&f, "0", "bad");
    q["token"] = "unknown".into();
    assert_eq!(
        s.dispatch("native.importCommit", &q).unwrap_err().kind,
        "stalePreview"
    );
    let p = preview(&mut s, &f);
    fs::create_dir_all(&f.data).unwrap();
    fs::write(f.data.join("payloads"), b"blocks destination").unwrap();
    q["token"] = p["token"].clone();
    q["operationId"] = "copy-failed".into();
    assert_eq!(
        s.dispatch("native.importCommit", &q).unwrap_err().kind,
        "nativeIo"
    );
    assert!(Store::open(&f.data).is_err());
}
#[test]
fn reusable_vpk_parser_checks_standalone_bounds_and_normalized_paths() {
    let f = Fixture::new();
    let p = f.root.join("inspect.vpk");
    fs::write(&p, vpk("Models/A.vmdl_c", 42)).unwrap();
    let e = modlock_helper::vpk::inspect(&p).unwrap();
    assert_eq!(e[0].path, "models/a.vmdl_c");
    assert_eq!(e[0].crc, 42);
    for b in [vpk("../a.vmdl_c", 1), vec![0; 12], {
        let mut b = vpk("models/a.vmdl_c", 1);
        b.pop();
        b
    }] {
        fs::write(&p, b).unwrap();
        assert!(modlock_helper::vpk::inspect(&p).is_err());
    }
}
#[test]
fn preview_masking_preserves_source_decisions_without_global_settings() {
    for visible in [true, false] {
        let f = Fixture::new();
        let mut state = f.state();
        state["local-config"]["state"]["localMods"][0]["isNSFW"] = true.into();
        state["local-config"]["state"]["nsfwSettings"] =
            object! {disableBlur:false,rememberPerItemOverrides:true,private:"AUTH_SENTINEL"};
        state["local-config"]["state"]["perItemNSFWOverrides"] =
            object! {"remote-a":visible,private:"AUTH_SENTINEL"};
        f.write_state(state);
        let mut s = NativeService::new();
        let v = import(&mut s, &f);
        assert_eq!(v["mods"][0]["previewHidden"], !visible);
        let mut reopened = NativeService::new();
        let v = reopened.dispatch("native.snapshot", &f.params()).unwrap();
        assert_eq!(v["mods"][0]["previewHidden"], !visible);
        let doc = Store::open(&f.data).unwrap().document().unwrap();
        assert!(!doc.has_key("nsfwSettings"));
        assert!(!doc.dump().contains("AUTH_SENTINEL"));
        assert!(!doc.dump().contains("perItemNSFWOverrides"));
    }
}
#[test]
fn enabled_payloads_use_recorded_shard_with_profile_suffix_and_disabled_prefix_is_exact() {
    let f = Fixture::new();
    let mut state = f.state();
    state["local-config"]["state"]["profiles"]["default"]["folderName"] = "active-folder".into();
    f.write_state(state);
    fs::create_dir_all(f.game.join("game/citadel/addons/active-folder")).unwrap();
    let mut m = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
    m["mods"]["remote-a"]["shard"] = 3.into();
    f.write_manifest("active-folder", m);
    fs::create_dir_all(f.game.join("game/citadel/addons3/active-folder")).unwrap();
    fs::rename(
        f.game.join("game/citadel/addons/pak01_dir.vpk"),
        f.game
            .join("game/citadel/addons3/active-folder/pak01_dir.vpk"),
    )
    .unwrap();
    let mut s = NativeService::new();
    assert_eq!(preview(&mut s, &f)["availableModCount"], 2);
    let mut m = json::parse(&fs::read_to_string(f.manifest_path("other-folder")).unwrap()).unwrap();
    m["mods"]["remote-b"]["disabledVpks"][0] = "local-b_b_dir.vpk".into();
    f.write_manifest("other-folder", m);
    assert_eq!(preview(&mut s, &f)["unavailableModCount"], 1);
}
#[test]
fn absent_manifest_and_unknown_mods_remain_visible_and_native_default_is_created() {
    let f = Fixture::new();
    fs::remove_file(f.manifest_path("")).unwrap();
    let mut state = f.state();
    state["local-config"]["state"]["localMods"]
        .push(object! {futureMod:object!{preserve:true}})
        .unwrap();
    f.write_state(state);
    let mut s = NativeService::new();
    let v = import(&mut s, &f);
    assert_eq!(v["mods"].len(), 2);
    assert_eq!(v["mods"][0]["available"], false);
    let f = Fixture::new();
    let mut v = f.state();
    v["local-config"]["state"]["profiles"].remove("default");
    v["local-config"]["state"]["localMods"] = json::array![];
    f.write_state(v);
    let mut s = NativeService::new();
    let v = import(&mut s, &f);
    assert_eq!(v["activeProfileId"], "default");
    assert!(v["profiles"].members().any(|p| p["id"] == "default"));
}
#[test]
fn import_retry_after_service_restart_is_idempotent_and_cannot_replace_owned_library() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let p = preview(&mut s, &f);
    let mut q = mutation(&f, "0", "import-1");
    q["token"] = p["token"].clone();
    let v = s.dispatch("native.importCommit", &q).unwrap();
    let mut reopened = NativeService::new();
    assert_eq!(reopened.dispatch("native.importCommit", &q).unwrap(), v);
    q["operationId"] = "import-2".into();
    assert_eq!(
        reopened
            .dispatch("native.importCommit", &q)
            .unwrap_err()
            .kind,
        "nativeAlreadyImported"
    );
    q["operationId"] = "import-1".into();
    q["token"] = "different".into();
    assert_eq!(
        reopened
            .dispatch("native.importCommit", &q)
            .unwrap_err()
            .kind,
        "operationIdConflict"
    );
}
#[test]
fn vpk_parser_rejects_external_chunks_duplicate_normalized_entries_and_ranges() {
    let f = Fixture::new();
    let path = f.root.join("test.vpk");
    let mut split = vpk("models/a.vmdl_c", 1);
    let archive = 12 + 7 + 7 + 2 + 4 + 2;
    split[archive..archive + 2].copy_from_slice(&0u16.to_le_bytes());
    let mut range = vpk("models/a.vmdl_c", 1);
    range[archive + 6..archive + 10].copy_from_slice(&9u32.to_le_bytes());
    let mut traversal = vpk("models/a.vmdl_c", 1);
    traversal[12 + 7..12 + 7 + 6].copy_from_slice(b"../bad");
    for bytes in [split, range, traversal] {
        fs::write(&path, bytes).unwrap();
        assert!(modlock_helper::vpk::inspect(&path).is_err());
    }
    let single = vpk("models/a.vmdl_c", 1);
    let mut duplicate = single[..single.len() - 3].to_vec();
    let mut entry = single[26..single.len() - 3].to_vec();
    entry[0] = b'A';
    duplicate.append(&mut entry);
    duplicate.extend([0, 0, 0]);
    let tree_len = (duplicate.len() - 12) as u32;
    duplicate[8..12].copy_from_slice(&tree_len.to_le_bytes());
    fs::write(&path, duplicate).unwrap();
    assert_eq!(
        modlock_helper::vpk::inspect(&path).unwrap_err().kind,
        "unsupportedVpk"
    );
    let b = vpk("models/a.vmdl_c", 1);
    let mut v2 = Vec::new();
    v2.extend_from_slice(&b[0..4]);
    v2.extend_from_slice(&2u32.to_le_bytes());
    v2.extend_from_slice(&b[8..12]);
    v2.extend_from_slice(&[0; 16]);
    v2.extend_from_slice(&b[12..]);
    fs::write(&path, &v2).unwrap();
    assert_eq!(
        modlock_helper::vpk::inspect(&path).unwrap()[0].path,
        "models/a.vmdl_c"
    );
    v2[12..16].copy_from_slice(&1u32.to_le_bytes());
    fs::write(&path, v2).unwrap();
    assert!(modlock_helper::vpk::inspect(&path).is_err());
}
#[test]
fn symlinked_manifest_keeps_profile_unavailable_and_never_follows_it() {
    let f = Fixture::new();
    let path = f.manifest_path("");
    fs::rename(&path, f.root.join("external-manifest")).unwrap();
    std::os::unix::fs::symlink(f.root.join("external-manifest"), path).unwrap();
    let mut s = NativeService::new();
    let v = import(&mut s, &f);
    assert_eq!(v["mods"][0]["available"], false);
    assert!(!f.data.join("payloads").join("external-manifest").exists());
}
#[test]
fn actual_rpc_session_keeps_preview_tokens_private_and_commits_then_reopens() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };
    let f = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_modlock-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut call = |method: &str, params: JsonValue| -> JsonValue {
        writeln!(
            input,
            "{}",
            object! {jsonrpc:"2.0",id:1,method:method,params:params}.dump()
        )
        .unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        json::parse(&line).unwrap()
    };
    let preview = call("native.importPreview", f.preview_params());
    assert!(!preview.dump().contains("AUTH_SENTINEL"));
    assert!(!preview["result"].has_key("source"));
    let mut q = mutation(&f, "0", "import-rpc");
    q["token"] = preview["result"]["token"].clone();
    let v = call("native.importCommit", q);
    assert_eq!(v["result"]["revision"], "1", "{}", v.dump());
    drop(call);
    drop(input);
    assert!(child.wait().unwrap().success());
    assert_eq!(
        rpc("native.snapshot", f.params())["result"]["mods"][0]["available"],
        true
    );
}
#[test]
fn import_ledger_retains_payload_provenance_without_private_global_fields() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(
        doc["importLedger"]["sourcePath"],
        f.source.to_str().unwrap()
    );
    assert_eq!(
        doc["importLedger"]["sourceSha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(doc["importLedger"]["payloads"].len(), 2);
    assert!(!doc["importLedger"].dump().contains("AUTH_SENTINEL"));
}

#[test]
fn native_database_and_payloads_are_private_regular_files() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    assert_eq!(
        fs::metadata(f.data.join("library.sqlite3"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let store = Store::open(&f.data).unwrap();
    let doc = store.document().unwrap();
    let id = doc["profiles"]["default"]["mods"][0]["packages"][0]["directory"]["id"]
        .as_str()
        .unwrap();
    assert_eq!(
        fs::metadata(store.payload_path(id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
}
#[test]
fn file_size_limit_forces_a_real_partial_copy_failure_without_committed_library() {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    #[repr(C)]
    struct Limit {
        current: u64,
        maximum: u64,
    }
    extern "C" {
        fn setrlimit(resource: i32, limit: *const Limit) -> i32;
        fn signal(number: i32, handler: usize) -> usize;
    }
    let f = Fixture::new();
    let original = f.game.join("game/citadel/addons/pak01_dir.vpk");
    let mut bytes = fs::read(&original).unwrap();
    bytes.resize(512 * 1024, 0);
    fs::write(&original, &bytes).unwrap();
    let mtime = fs::metadata(&original).unwrap().modified().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_modlock-helper"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Test-child limits only: regular-file writes fail after 256 KiB, while RPC
    // pipes and read-only sha256sum continue normally. No production failpoint.
    unsafe {
        cmd.pre_exec(|| {
            signal(25, 1);
            let limit = Limit {
                current: 256 * 1024,
                maximum: 256 * 1024,
            };
            if setrlimit(1, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut call = |method: &str, params: JsonValue| -> JsonValue {
        writeln!(
            input,
            "{}",
            object! {jsonrpc:"2.0",id:1,method:method,params:params}.dump()
        )
        .unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        json::parse(&line).unwrap()
    };
    let p = call("native.importPreview", f.preview_params());
    let mut q = mutation(&f, "0", "partial-copy");
    q["token"] = p["result"]["token"].clone();
    let result = call("native.importCommit", q);
    assert_eq!(
        result["error"]["data"]["kind"],
        "nativeIo",
        "{}",
        result.dump()
    );
    drop(call);
    drop(input);
    assert!(child.wait().unwrap().success());
    assert!(Store::open(&f.data).is_err());
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 0);
    assert!(!fs::read_dir(&f.data).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".import-")));
    assert_eq!(fs::read(&original).unwrap(), bytes);
    assert_eq!(fs::metadata(&original).unwrap().modified().unwrap(), mtime);
}

#[test]
fn blank_and_whitespace_status_entries_remain_visible_as_unavailable() {
    for status in ["", " ", "\t\n"] {
        let f = Fixture::new();
        let mut state = f.state();
        state["local-config"]["state"]["localMods"][0]["status"] = status.into();
        f.write_state(state);
        let mut service = NativeService::new();
        let reviewed = preview(&mut service, &f);
        assert_eq!(reviewed["modCount"], 2);
        assert_eq!(reviewed["availableModCount"], 1);
        assert_eq!(reviewed["unavailableModCount"], 1);
        let committed = import(&mut service, &f);
        assert_eq!(committed["mods"].len(), 1);
        assert_eq!(committed["profiles"][0]["modCount"], 1);
        assert_eq!(committed["mods"][0]["available"], false);
        assert!(!committed["mods"][0]["unavailableReason"]
            .as_str()
            .unwrap()
            .is_empty());
        assert!(!committed["warnings"]
            .members()
            .any(|w| w["code"] == "invalidMod"));
        let document = Store::open(&f.data).unwrap().document().unwrap();
        assert_eq!(document["importArchive"]["localMods"][0]["status"], status);
        assert_eq!(
            document["profiles"]["default"]["mods"][0]["source"]["original"]["status"],
            status
        );
    }
}
#[test]
fn explicit_import_retry_reconciles_only_recorded_orphan_staging() {
    for unknown in [false, true] {
        let f = Fixture::new();
        let mut service = NativeService::new();
        let preview = preview(&mut service, &f);
        let stage_name = ".import-0123456789abcdef0123456789abcdef";
        let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let stage = f.data.join(stage_name);
        let payload_root = f.data.join("payloads");
        let lock = modlock_helper::store::RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let body = object! {kind:"import",stage:stage_name,input:object!{method:"native.importCommit",params:object!{}},payloads:json::array![object!{id:id}]};
        store
            .write_journal(&lock, "interrupted-import", &body)
            .unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("owner.json"), body.dump()).unwrap();
        fs::create_dir(&payload_root).unwrap();
        fs::write(stage.join(format!("{id}.vpk")), b"incomplete copied bytes").unwrap();
        fs::hard_link(
            stage.join(format!("{id}.vpk")),
            payload_root.join(format!("{id}.vpk")),
        )
        .unwrap();
        fs::write(payload_root.join("unknown.vpk"), b"keep").unwrap();
        if unknown {
            fs::write(stage.join("unknown"), b"keep").unwrap();
        }
        drop(store);
        drop(lock);
        let mut p = mutation(&f, "0", "retry-import");
        p["token"] = preview["token"].clone();
        let result = service.dispatch("native.importCommit", &p);
        if unknown {
            assert_eq!(result.unwrap_err().kind, "recoveryRequired");
            assert_eq!(fs::read(stage.join("unknown")).unwrap(), b"keep");
            assert!(stage.join(format!("{id}.vpk")).exists());
        } else {
            result.unwrap();
            assert!(!stage.exists());
            assert!(!payload_root.join(format!("{id}.vpk")).exists());
            assert_eq!(Store::open(&f.data).unwrap().journals().unwrap().len(), 0);
        }
        assert_eq!(fs::read(payload_root.join("unknown.vpk")).unwrap(), b"keep");
    }
}
#[test]
fn no_document_recovery_clears_owned_import_journal_and_keeps_source_only_state() {
    let f = Fixture::new();
    let lock = modlock_helper::store::RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let body = object! {kind:"import",stage:".import-0123456789abcdef0123456789abcdef",input:object!{method:"native.importCommit",params:object!{}},payloads:json::array![]};
    st.write_journal(&lock, "lost-import", &body).unwrap();
    drop(st);
    drop(lock);
    let mut s = NativeService::new();
    let p = mutation(&f, "0", "recover-import");
    assert_eq!(
        s.dispatch("native.recover", &p).unwrap_err().kind,
        "nativeNotImported"
    );
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap_err().kind,
        "nativeNotImported"
    );
    let st = Store::open_history(&f.data).unwrap();
    assert_eq!(st.journals().unwrap().len(), 0);
    assert_eq!(
        st.operation("lost-import").unwrap()["error"]["kind"],
        "operationInterrupted"
    );
    assert_eq!(st.operation("recover-import").unwrap()["status"], "failed");
    assert_eq!(
        s.dispatch("native.recover", &p).unwrap_err().kind,
        "nativeNotImported"
    );
}
#[test]
fn committed_import_cleanup_resumes_after_one_stage_hardlink_was_removed() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let lock = modlock_helper::store::RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let doc = st.document().unwrap();
    let p = doc["importLedger"]["payloads"][0].clone();
    let name = ".import-0123456789abcdef0123456789abcdef";
    let stage = f.data.join(name);
    fs::create_dir(&stage).unwrap();
    let body = object! {kind:"import",stage:name,input:object!{},payloads:json::array![p.clone()]};
    fs::write(stage.join("owner.json"), body.dump()).unwrap();
    st.write_journal(&lock, "import-1", &body).unwrap();
    modlock_helper::import::reconcile_imports(&st, &lock).unwrap();
    assert!(!stage.exists());
    assert!(st.payload_path(p["id"].as_str().unwrap()).unwrap().exists());
    assert_eq!(st.journals().unwrap().len(), 0);
}
#[test]
fn prior_source_evidence_includes_unsupported_missing_and_manifest_only_packages() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let root = f.game.join("game/citadel/addons");
    fs::write(root.join("pak01_dir.vpk"), b"unsupported").unwrap();
    let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
    manifest["mods"]["remote-a"]["currentVpks"]
        .push("pak02_dir.vpk")
        .unwrap();
    manifest["mods"]["remote-a"]["originalVpkNames"]
        .push("b_dir.vpk")
        .unwrap();
    manifest["mods"]["not-in-library"] = object! {enabled:true,order:1,shard:1,currentVpks:json::array!["pak03_dir.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["c_dir.vpk"]};
    fs::write(root.join("pak03_dir.vpk"), b"manifest only").unwrap();
    f.write_manifest("", manifest);
    import(&mut s, &f);
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    let sources = &doc["importLedger"]["sourcePayloads"];
    for (name, state) in [
        ("pak01_dir.vpk", "present"),
        ("pak02_dir.vpk", "missing"),
        ("pak03_dir.vpk", "present"),
    ] {
        let source = sources
            .members()
            .find(|p| p["sourcePath"].as_str() == root.join(name).to_str())
            .expect("every manifest-owned path needs evidence");
        assert_eq!(source["state"], state);
        if state == "present" {
            assert_eq!(source["sha256"].as_str().unwrap().len(), 64);
        }
    }
}

#[test]
fn review_exposes_profile_and_unavailable_explanations_before_copying() {
    let f = Fixture::new();
    fs::remove_file(f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap();
    let mut s = NativeService::new();
    let r = preview(&mut s, &f);
    assert_eq!(r["profiles"].len(), 2);
    assert_eq!(r["unavailableMods"][0]["profileId"], "default");
    assert_eq!(r["unavailableMods"][0]["modId"], "local-a");
    assert!(!r["unavailableMods"][0]["reason"]
        .as_str()
        .unwrap()
        .is_empty());
    assert_eq!(r["summariesTruncated"], false);
    let snap = import(&mut s, &f);
    assert_eq!(snap["profiles"][0]["protected"], true);
}

#[test]
fn review_marks_bounded_text_and_entry_summaries_as_shortened() {
    for count in [1, 513] {
        let f = Fixture::new();
        let mut state = f.state();
        state["local-config"]["state"]["profiles"]["default"]["name"] = "x".repeat(2048).into();
        let mods = (0..count).map(|i| object!{id:format!("missing-{i}"),remoteId:format!("missing-{i}"),name:"y".repeat(2048),status:"disabled"}).collect::<Vec<_>>();
        state["local-config"]["state"]["localMods"] = JsonValue::from(mods);
        f.write_state(state);
        let mut service = NativeService::new();
        let r = preview(&mut service, &f);
        assert_eq!(r["unavailableModCount"], count);
        assert_eq!(r["unavailableMods"].len(), count.min(512));
        assert!(r["profiles"][0]["name"].as_str().unwrap().chars().count() <= 1024);
        assert_eq!(r["summariesTruncated"], true);
    }
}

#[test]
fn renamed_vpk_and_duplicate_filename_import_explanation() {
    use modlock_helper::import::ImportPlan;
    let f = Fixture::new();
    fs::write(f.game.join("game/citadel/gameinfo.gi"), "fixture").unwrap();
    let dir = f.game.join("game/citadel/addons");
    f.write_manifest("", object!{version:3,mods:object!{"remote-a":object!{
        enabled:true,shard:1,currentVpks:json::array!["pak11_dir.vpk"],
        disabledVpks:json::array![],originalVpkNames:json::array!["pak03_dir.vpk"]}}});
    let valid = support::vpk("readme.txt", 1);
    fs::write(dir.join("pak11_dir.vpk"), &valid).unwrap();
    let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
    assert_eq!(plan.document["profiles"]["default"]["mods"][0]["available"], true);
    let scanned = modlock_helper::installed::scan(&f.game, None).unwrap();
    assert!(scanned["items"].members().any(|i| i["path"].as_str().unwrap().ends_with("pak11_dir.vpk") && i["error"].is_null()));
    let tree = &valid[12..];
    let duplicate = [&tree[..tree.len()-1], tree].concat();
    let bytes = support::vpk_with_tree(1, &duplicate, &[], [&[], &[], &[]]);
    fs::write(dir.join("pak11_dir.vpk"), bytes).unwrap();
    let plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
    let m = &plan.document["profiles"]["default"]["mods"][0];
    assert_eq!(m["available"], false);
    let reason = m["unavailableReason"].as_str().unwrap();
    assert!(reason.contains("repeated filename") && reason.contains("Browse"), "{reason}");
    let scanned = modlock_helper::installed::scan(&f.game, None).unwrap();
    let row = scanned["items"].members().find(|i| i["path"].as_str().unwrap().ends_with("pak11_dir.vpk")).unwrap();
    assert!(row["error"].as_str().unwrap().contains("repeated filename"));
}
