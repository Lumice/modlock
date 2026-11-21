mod support;
use modlock_helper::{
    native::NativeService,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use std::fs;
use support::*;
const BEFORE: &str = "// keep exact\r\n\"GameInfo\"\r\n{\r\n FileSystem\r\n {\r\n  SearchPaths\r\n  {\r\n   Game citadel/addons // old\r\n   Game_LowViolence citadel_lv\r\n   Game citadel\r\n   Mod citadel\r\n   Game core\r\n  }\r\n }\r\n}\r\n";
fn setup() -> (Fixture, NativeService) {
    let f = Fixture::new();
    let mut s = NativeService::new();
    fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
    import(&mut s, &f);
    (f, s)
}
fn review(
    s: &mut NativeService,
    f: &Fixture,
    profile: &str,
) -> Result<JsonValue, modlock_helper::native::NativeError> {
    let mut p = f.params();
    p["profileId"] = profile.into();
    p["gamePath"] = f.game.to_str().unwrap().into();
    s.dispatch("native.applyPreview", &support::current_layout(&p))
}
fn apply(s: &mut NativeService, f: &Fixture, profile: &str, rev: &str, id: &str) -> JsonValue {
    let r = review(s, f, profile).unwrap();
    let mut p = mutation(f, rev, id);
    p["token"] = r["token"].clone();
    s.dispatch("native.apply", &support::current_layout(&p)).unwrap()
}
fn refuse_reapply_preserving(s:&mut NativeService,f:&Fixture,profile:&str,revision:&str,id:&str,kind:&str){
    let before=tree_bytes(&f.game);let document=Store::open(&f.data).unwrap().document().unwrap();
    let result=review(s,f,profile).and_then(|review| {
        let mut request=mutation(f,revision,id);request["token"]=review["token"].clone();
        s.dispatch("native.apply",&support::current_layout(&request))
    });
    assert_eq!(result.unwrap_err().kind,kind);
    let store=Store::open(&f.data).unwrap();assert_eq!(store.document().unwrap(),document);
    assert!(store.journals().unwrap().is_empty());assert_eq!(tree_bytes(&f.game),before);
}
fn edit(f: &Fixture, action: impl FnOnce(&mut JsonValue)) {
    let lock = RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let mut d = st.document().unwrap();
    action(&mut d);
    st.commit(&lock, 1, "test-edit", &object! {}, d, None)
        .unwrap();
}
#[test]
fn apply_places_verified_bytes_preserves_unknown_files_and_restores_exact_gameinfo() {
    let (f, mut s) = setup();
    let original = fs::read(f.game.join("game/citadel/addons/pak01_dir.vpk")).unwrap();
    fs::write(f.game.join("game/citadel/addons/unknown.vpk"), b"leave me").unwrap();
    let r = review(&mut s, &f, "default").unwrap();
    assert_eq!(r["fileCount"], 1);
    assert_eq!(r["replacedMounts"][0], "citadel/addons");
    assert!(!f.game.join("game/citadel/deadlock_native").exists());
    let applied = apply(&mut s, &f, "default", "1", "apply");
    assert_eq!(applied["appliedProfileId"], "default");
    assert_eq!(applied["pendingChanges"], false);
    assert_eq!(applied["canRestore"], true);
    let d = Store::open(&f.data).unwrap().document().unwrap();
    let generation = d["applied"]["generation"].as_str().unwrap();
    assert_eq!(
        fs::read(
            f.game
                .join("game/citadel/deadlock_native")
                .join(generation)
                .join("shard01/pak01_dir.vpk")
        )
        .unwrap(),
        original
    );
    let after = fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    assert!(after.contains(
        "Game_LowViolence citadel_lv\r\n   Game citadel\r\n   Mod citadel\r\n   Game core"
    ));
    assert!(!after.contains("Game citadel/addons"));
    let restored = s
        .dispatch("native.restore", &mutation(&f, "2", "restore"))
        .unwrap();
    assert!(restored["appliedProfileId"].is_null());
    assert_eq!(restored["mods"][0]["enabled"], true);
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        BEFORE.as_bytes()
    );
    assert_eq!(
        fs::read(f.game.join("game/citadel/addons/unknown.vpk")).unwrap(),
        b"leave me"
    );
}
#[test]
fn empty_profile_apply_disables_old_mount_and_switches_actual_profile() {
    let (f, mut s) = setup();
    let r = apply(&mut s, &f, "other", "1", "empty");
    assert_eq!(r["activeProfileId"], "other");
    assert_eq!(r["appliedProfileId"], "other");
    assert_eq!(r["pendingChanges"], false);
    assert!(!fs::read_to_string(f.game.join("game/citadel/gameinfo.gi"))
        .unwrap()
        .contains("Game citadel/addons"));
}
#[test]
fn stale_review_and_external_change_never_overwrite() {
    for what in ["revision", "gameinfo", "payload"] {
        let (f, mut s) = setup();
        let r = review(&mut s, &f, "default").unwrap();
        match what {
            "revision" => {
                let mut p = mutation(&f, "1", "disable");
                p["profileId"] = "default".into();
                p["modId"] = "local-a".into();
                p["enabled"] = false.into();
                s.dispatch("native.setEnabled", &p).unwrap();
            }
            "gameinfo" => fs::write(f.game.join("game/citadel/gameinfo.gi"), "external").unwrap(),
            _ => {
                let st = Store::open(&f.data).unwrap();
                let d = st.document().unwrap();
                fs::remove_file(
                    st.payload_path(
                        d["profiles"]["default"]["mods"][0]["packages"][0]["directory"]["id"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap();
            }
        }
        let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        let mut p = mutation(&f, "1", "apply");
        p["token"] = r["token"].clone();
        assert!(s.dispatch("native.apply", &support::current_layout(&p)).is_err());
        assert_eq!(
            fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            before
        );
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
    }
}
#[test]
fn unknown_or_ambiguous_mounts_and_malformed_config_block() {
    for bad in [
        BEFORE.replace("citadel/addons", "citadel/addons/unknown"),
        BEFORE.replace("  }\r\n", "  }\r\n  SearchPaths {}\r\n"),
        "GameInfo { FileSystem { SearchPaths {".into(),
    ] {
        let (f, mut s) = setup();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), &bad).unwrap();
        assert!(review(&mut s, &f, "default").is_err());
        assert_eq!(
            fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            bad
        );
    }
}
#[test]
fn unavailable_duplicate_and_per_mod_limit_block_before_staging() {
    for case in ["unavailable", "duplicate", "cap"] {
        let (f, mut s) = setup();
        edit(&f, |d| {
            let m = &mut d["profiles"]["default"]["mods"][0];
            match case {
                "unavailable" => m["available"] = false.into(),
                "duplicate" => {
                    let p = m["packages"][0].clone();
                    m["packages"].push(p).unwrap();
                }
                _ => {
                    let p = m["packages"][0].clone();
                    m["packages"] = JsonValue::from(vec![p; 100]);
                }
            }
        });
        assert_eq!(
            review(&mut s, &f, "default").unwrap_err().kind,
            match case {
                "unavailable" => "unavailablePayload",
                "duplicate" => "selectionRequired",
                _ => "selectionRequired",
            }
        );
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
    }
}
#[test]
fn snapshot_detects_external_game_changes_and_restore_refuses() {
    for what in ["gameinfo", "generation"] {
        let (f, mut s) = setup();
        apply(&mut s, &f, "default", "1", "apply");
        if what == "gameinfo" {
            fs::write(f.game.join("game/citadel/gameinfo.gi"), "external").unwrap();
        } else {
            let d = Store::open(&f.data).unwrap().document().unwrap();
            fs::remove_file(
                f.game
                    .join("game/citadel/deadlock_native")
                    .join(d["applied"]["generation"].as_str().unwrap())
                    .join("shard01/pak01_dir.vpk"),
            )
            .unwrap();
        }
        let r = s.dispatch("native.snapshot", &f.params()).unwrap();
        if what == "gameinfo" {
            // Another tool rewrote gameinfo.gi while our generation is intact:
            // not an interrupted operation, the user is offered Apply again.
            assert_eq!(r["recoveryRequired"], false);
            assert_eq!(r["pendingReason"], "externalGameinfoChange");
        } else {
            assert_eq!(r["recoveryRequired"], true);
        }
        assert!(r["appliedProfileId"].is_null());
        assert!(s
            .dispatch("native.restore", &mutation(&f, "2", "restore"))
            .is_err());
    }
}
#[test]
fn apply_retry_replays_after_restart() {
    let (f, mut s) = setup();
    let r = review(&mut s, &f, "default").unwrap();
    let mut p = mutation(&f, "1", "apply");
    p["token"] = r["token"].clone();
    let one = s.dispatch("native.apply", &support::current_layout(&p)).unwrap();
    let mut other = NativeService::new();
    assert_eq!(other.dispatch("native.apply", &support::current_layout(&p)).unwrap(), one);
    p["token"] = "other".into();
    assert_eq!(
        other.dispatch("native.apply", &support::current_layout(&p)).unwrap_err().kind,
        "operationIdConflict"
    );
}
#[test]
fn unknown_native_workspace_and_other_store_are_not_adopted() {
    let (f, mut s) = setup();
    fs::create_dir(f.game.join("game/citadel/deadlock_native")).unwrap();
    fs::write(f.game.join("game/citadel/deadlock_native/unknown"), "keep").unwrap();
    assert!(review(&mut s, &f, "default").is_err());
    assert_eq!(
        fs::read(f.game.join("game/citadel/deadlock_native/unknown")).unwrap(),
        b"keep"
    );
}
#[test]
fn order_conflicts_and_identical_resources_use_real_vpk_trees() {
    for (resource, crc, want) in [
        ("models/a.vmdl_c", 99, "modelConflict"),
        ("models/a.vmdl_c", 7, "ok"),
        ("materials/x.vmat_c", 99, "warn"),
    ] {
        let f = Fixture::new();
        let mut s = NativeService::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        let first = if want == "warn" {
            "materials/x.vmat_c"
        } else {
            "models/a.vmdl_c"
        };
        fs::write(
            f.game.join("game/citadel/addons/pak01_dir.vpk"),
            vpk(first, 7),
        )
        .unwrap();
        fs::write(
            f.game.join("game/citadel/addons/pak02_dir.vpk"),
            vpk(resource, crc),
        )
        .unwrap();
        let mut state = f.state();
        let mods = &mut state["local-config"]["state"]["localMods"];
        mods[0]["installOrder"] = 50.into();
        mods.push(object!{id:"local-c",remoteId:"remote-c",name:"Earlier C",status:"installed",installOrder:2}).unwrap();
        f.write_state(state);
        let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        manifest["mods"]["remote-c"] = object! {enabled:true,order:1,shard:1,currentVpks:json::array!["pak02_dir.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["c_dir.vpk"]};
        f.write_manifest("", manifest);
        import(&mut s, &f);
        let r = review(&mut s, &f, "default");
        if want == "modelConflict" {
            let e = r.unwrap_err();
            assert_eq!(e.kind, want);
            assert!(e.message.contains("Working A") && e.message.contains("Earlier C"));
            continue;
        }
        if want == "ok" {
            let r = r.unwrap();
            assert_eq!(r["warnings"].members().filter(|w|w["code"]=="fileConflict").count(), 0);
            assert!(r["warnings"].members().any(|w|w["code"]=="originalManagerTakeover"));
            assert_eq!(r["fileCount"], 1);
            let result = apply(&mut s, &f, "default", "1", "identical");
            assert!(result["mods"]
                .members()
                .all(|m| m["appliedEnabled"] == true));
            let doc = Store::open(&f.data).unwrap().document().unwrap();
            assert_eq!(
                doc["applied"]["manifest"]["shards"][0]["packages"][0]["modId"],
                "local-c"
            );
            assert_eq!(
                doc["applied"]["manifest"]["shards"][0]["packages"][0]["modIds"],
                json::array!["local-c", "local-a"]
            );
            continue;
        }
        let r = r.unwrap();
        let conflicts=r["warnings"].members().filter(|w|w["code"]=="fileConflict").collect::<Vec<_>>();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0]["message"].as_str().unwrap().contains("Earlier C wins"));
        assert!(r["warnings"].members().any(|w|w["code"]=="originalManagerTakeover"));
        apply(&mut s, &f, "default", "1", "ordered");
        let d = Store::open(&f.data).unwrap().document().unwrap();
        let dir = f
            .game
            .join("game/citadel/deadlock_native")
            .join(d["applied"]["generation"].as_str().unwrap());
        assert_eq!(
            fs::read(dir.join("shard01/pak01_dir.vpk")).unwrap(),
            vpk(resource, crc)
        );
    }
}
#[test]
fn manifest_changes_symlinks_and_payload_corruption_are_rejected() {
    for kind in ["manifest", "symlink", "corrupt"] {
        let (f, mut s) = setup();
        match kind {
            "manifest" => f.write_manifest("", object! {version:3,mods:object!{}}),
            "symlink" => {
                let p = f.game.join("game/citadel/gameinfo.gi");
                fs::remove_file(&p).unwrap();
                std::os::unix::fs::symlink(&f.source, &p).unwrap();
            }
            _ => {
                let st = Store::open(&f.data).unwrap();
                let d = st.document().unwrap();
                let p = st
                    .payload_path(
                        d["profiles"]["default"]["mods"][0]["packages"][0]["directory"]["id"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                fs::remove_file(&p).unwrap();
                fs::write(p, "invalid").unwrap();
            }
        }
        assert!(review(&mut s, &f, "default").is_err());
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
    }
}
#[test]
fn disabled_choices_apply_empty_then_restore_previous_generation() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "first");
    let previous = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    let mut p = mutation(&f, "2", "disable");
    p["profileId"] = "default".into();
    p["modId"] = "local-a".into();
    p["enabled"] = false.into();
    s.dispatch("native.setEnabled", &p).unwrap();
    let v = apply(&mut s, &f, "default", "3", "disabled");
    assert_eq!(v["mods"][0]["appliedEnabled"], false);
    let r = s
        .dispatch("native.restore", &mutation(&f, "4", "restore"))
        .unwrap();
    assert_eq!(r["mods"][0]["enabled"], false);
    assert_eq!(r["mods"][0]["appliedEnabled"], true);
    assert_eq!(r["pendingChanges"], true);
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        previous
    );
}
#[test]
fn same_game_other_library_and_root_lock_contention_block() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "apply");
    let other = f.root.join("other-native");
    let mut p = f.preview_params();
    p["dataDir"] = other.to_str().unwrap().into();
    let v = s.dispatch("native.importPreview", &p).unwrap();
    let q = object! {dataDir:other.to_str().unwrap(),token:v["token"].clone(),expectedRevision:"0",operationId:"import-other"};
    s.dispatch("native.importCommit", &q).unwrap();
    let q = object! {dataDir:other.to_str().unwrap(),profileId:"default",gamePath:f.game.to_str().unwrap()};
    assert_eq!(
        s.dispatch("native.applyPreview", &support::current_layout(&q)).unwrap_err().kind,
        "foreignWorkspace"
    );
    let _lock = RootLock::acquire(&f.data).unwrap();
    assert_eq!(
        s.dispatch("native.restore", &mutation(&f, "2", "restore"))
            .unwrap_err()
            .kind,
        "nativeBusy"
    );
}
#[test]
fn healthy_generation_retention_is_current_plus_previous_and_unknown_files_survive() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "one");
    let first = Store::open(&f.data).unwrap().document().unwrap()["applied"]["generation"]
        .as_str()
        .unwrap()
        .to_owned();
    apply(&mut s, &f, "other", "2", "two");
    apply(&mut s, &f, "default", "3", "three");
    assert!(!f
        .game
        .join("game/citadel/deadlock_native")
        .join(first)
        .exists());
    let previous = Store::open(&f.data).unwrap().document().unwrap()["previous"]["applied"]
        ["generation"]
        .as_str()
        .unwrap()
        .to_owned();
    let dir = f.game.join("game/citadel/deadlock_native").join(previous);
    fs::write(dir.join("unknown"), "keep").unwrap();
    refuse_reapply_preserving(&mut s,&f,"other","4","four","externalChange");
    assert_eq!(fs::read(dir.join("unknown")).unwrap(), b"keep");
}
#[test]
fn restorable_profile_cannot_be_removed_and_previous_generation_is_verified() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "other", "1", "one");
    apply(&mut s, &f, "default", "2", "two");
    let mut p = mutation(&f, "3", "remove");
    p["action"] = "remove".into();
    p["profileId"] = "other".into();
    assert_eq!(
        s.dispatch("native.profile", &p).unwrap_err().kind,
        "protectedProfile"
    );
    let d = Store::open(&f.data).unwrap().document().unwrap();
    let dir = f
        .game
        .join("game/citadel/deadlock_native")
        .join(d["previous"]["applied"]["generation"].as_str().unwrap());
    fs::write(dir.join("unknown"), "external").unwrap();
    let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    assert!(s
        .dispatch("native.restore", &mutation(&f, "3", "restore"))
        .is_err());
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        before
    );
}
#[test]
fn corrupted_restore_backup_is_rejected_before_game_mutation() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "apply");
    let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let mut d = st.document().unwrap();
    d["previous"]["gameinfo"] = "corrupted backup".into();
    st.commit(&lock, 2, "corrupt", &object! {}, d, None)
        .unwrap();
    drop(st);
    drop(lock);
    assert_eq!(
        s.dispatch("native.restore", &mutation(&f, "3", "restore"))
            .unwrap_err()
            .kind,
        "externalChange"
    );
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        before
    );
}
#[test]
fn reviewed_known_compatibility_flags_remain_visible_as_bounded_warnings() {
    let (f, mut s) = setup();
    edit(&f, |d| {
        let m = &mut d["profiles"]["default"]["mods"][0]["source"];
        m["isBlacklisted"] = true.into();
        m["blacklistReason"] = "Known crash on this hero".into();
        m["isObsolete"] = true.into();
        m["usesCriticalPaths"] = true.into();
    });
    let r = review(&mut s, &f, "default").unwrap();
    let warnings = &r["warnings"];
    assert!(warnings.members().any(|w| w["code"] == "blacklisted"
        && w["message"]
            .as_str()
            .unwrap()
            .contains("Known crash on this hero")));
    assert!(warnings.members().any(|w| w["code"] == "obsolete"));
    assert!(warnings.members().any(|w| w["code"] == "criticalPaths"));
}
#[test]
fn first_restore_refuses_changed_upstream_packages_in_previous_mount() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "apply");
    let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    fs::write(
        f.game.join("game/citadel/addons/pak01_dir.vpk"),
        vpk("models/a.vmdl_c", 123),
    )
    .unwrap();
    assert_eq!(
        s.dispatch("native.restore", &mutation(&f, "2", "restore"))
            .unwrap_err()
            .kind,
        "externalChange"
    );
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        before
    );
}
#[test]
fn deduplicated_payloads_retain_each_prior_source_mount_fingerprint() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let second = f
        .game
        .join("game/citadel/addons/other-folder/remote-b_b_dir.vpk");
    fs::write(&second, vpk("models/a.vmdl_c", 7)).unwrap();
    fs::write(
        f.game.join("game/citadel/gameinfo.gi"),
        BEFORE.replace(
            "Game citadel/addons // old",
            "Game citadel/addons/other-folder // old",
        ),
    )
    .unwrap();
    import(&mut s, &f);
    apply(&mut s, &f, "default", "1", "apply");
    fs::write(second, vpk("models/a.vmdl_c", 123)).unwrap();
    assert_eq!(
        s.dispatch("native.restore", &mutation(&f, "2", "restore"))
            .unwrap_err()
            .kind,
        "externalChange"
    );
}
#[test]
fn warm_snapshot_and_strong_metadata_check_use_in_process_hashes() {
    use std::{
        io::Write,
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };
    let (f, mut s, _) = family_setup(true);
    apply(&mut s, &f, "default", "1", "apply");
    let bin = f.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let log = f.root.join("hashes");
    let script = bin.join("sha256sum");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nreadlink /proc/self/fd/0 >> '{}'\nexec /usr/bin/sha256sum\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    let call = || {
        let mut c = Command::new(env!("CARGO_BIN_EXE_modlock-helper"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(
            c.stdin.take().unwrap(),
            "{}",
            object! {jsonrpc:"2.0",id:1,method:"native.snapshot",params:f.params()}.dump()
        )
        .unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success());
        json::parse(std::str::from_utf8(&o.stdout).unwrap()).unwrap()
    };
    assert_eq!(call()["result"]["appliedProfileId"], "default");
    assert!(!fs::read_to_string(&log)
        .unwrap_or_default()
        .contains(".vpk"));
    let d = Store::open(&f.data).unwrap().document().unwrap();
    let file = f
        .game
        .join("game/citadel/deadlock_native")
        .join(d["applied"]["generation"].as_str().unwrap())
        .join("shard01/pak01_017.vpk");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let mut bytes = fs::read(&file).unwrap();
    bytes[0] ^= 1;
    fs::write(file, bytes).unwrap();
    assert_eq!(call()["result"]["recoveryRequired"], true);
    // Strong verification uses the vendored hasher; no subprocess is needed.
    assert!(!fs::read_to_string(log).unwrap_or_default().contains(".vpk"));
}
#[test]
fn unavailable_prior_packages_are_verified_independently_of_importability() {
    for case in [
        "unsupported",
        "split",
        "missing",
        "partial-valid-companion",
        "partial-missing-companion",
        "partial-unsupported-companion",
    ] {
        let f = Fixture::new();
        let mut s = NativeService::new();
        let first = f.game.join("game/citadel/addons/pak01_dir.vpk");
        let second = f.game.join("game/citadel/addons/pak02_dir.vpk");
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        match case {
            "unsupported" => fs::write(&first, b"unsupported old package").unwrap(),
            "split" => {
                let mut bytes = vpk("models/a.vmdl_c", 7);
                bytes[34..36].copy_from_slice(&0u16.to_le_bytes());
                bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
                fs::write(&first, bytes).unwrap();
            }
            "missing" => fs::remove_file(&first).unwrap(),
            _ => {
                if case == "partial-unsupported-companion" {
                    fs::write(&second, b"unsupported companion").unwrap();
                }
                let mut manifest =
                    json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
                manifest["mods"]["remote-a"]["currentVpks"]
                    .push("pak02_dir.vpk")
                    .unwrap();
                manifest["mods"]["remote-a"]["originalVpkNames"]
                    .push("b_dir.vpk")
                    .unwrap();
                f.write_manifest("", manifest);
            }
        }
        fs::write(
            f.game.join("game/citadel/addons/unknown.vpk"),
            b"preserve unknown",
        )
        .unwrap();
        let imported = import(&mut s, &f);
        assert_eq!(imported["mods"][0]["available"], false, "{case}");
        apply(&mut s, &f, "other", "1", "empty");
        let active = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        let target = if matches!(case, "partial-missing-companion" | "partial-unsupported-companion") {
            second
        } else {
            first
        };
        fs::write(target, vpk("models/new.vmdl_c", 44)).unwrap();
        assert_eq!(
            s.dispatch("native.restore", &mutation(&f, "2", "restore"))
                .unwrap_err()
                .kind,
            "externalChange",
            "{case}"
        );
        assert_eq!(
            fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            active
        );
        assert_eq!(
            fs::read(f.game.join("game/citadel/addons/unknown.vpk")).unwrap(),
            b"preserve unknown"
        );
    }
}
#[test]
fn shared_payload_with_inconsistent_fingerprint_is_rejected() {
    let (f, mut s) = setup();
    let lock = RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let mut d = st.document().unwrap();
    let mut other = d["profiles"]["default"]["mods"][0].clone();
    other["source"]["id"] = "another".into();
    other["packages"][0]["directory"]["sha256"] = "0".repeat(64).into();
    d["profiles"]["default"]["mods"].push(other).unwrap();
    assert_eq!(
        st.commit(&lock, 1, "invalid", &object! {}, d, None)
            .unwrap_err()
            .kind,
        "invalidPackage"
    );
    drop(st);
    drop(lock);
    assert!(review(&mut s, &f, "default").is_ok());
    assert!(!f.game.join("game/citadel/deadlock_native").exists());
}
#[test]
fn unchanged_unavailable_prior_setup_restores_but_incomplete_or_unsafe_evidence_blocks_apply() {
    for case in ["missing", "unsupported", "incomplete-ledger", "symlink"] {
        let f = Fixture::new();
        let mut s = NativeService::new();
        let original = f.game.join("game/citadel/addons/pak01_dir.vpk");
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        match case {
            "missing" => fs::remove_file(&original).unwrap(),
            "unsupported" => fs::write(&original, b"unsupported original").unwrap(),
            "symlink" => {
                fs::remove_file(&original).unwrap();
                std::os::unix::fs::symlink(&f.source, &original).unwrap();
            }
            _ => {}
        }
        import(&mut s, &f);
        if case == "incomplete-ledger" {
            edit(&f, |d| {
                d["importLedger"]["sourcePayloads"] = JsonValue::new_array()
            });
        }
        if matches!(case, "incomplete-ledger" | "symlink") {
            assert_eq!(
                review(&mut s, &f, "other").unwrap_err().kind,
                "priorSetupUnavailable"
            );
            assert_eq!(
                fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                BEFORE.as_bytes()
            );
            assert!(!f.game.join("game/citadel/deadlock_native").exists());
        } else {
            apply(&mut s, &f, "other", "1", "empty");
            s.dispatch("native.restore", &mutation(&f, "2", "restore"))
                .unwrap();
            assert_eq!(
                fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                BEFORE.as_bytes()
            );
            assert_eq!(
                s.dispatch("native.snapshot", &f.params()).unwrap()["recoveryRequired"],
                false
            );
        }
    }
}
#[test]
fn split_prior_footprint_is_rejected_even_when_only_unlisted_archive_changes() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    let directory = f.game.join("game/citadel/addons/pak01_dir.vpk");
    let chunk = f.game.join("game/citadel/addons/pak01_000.vpk");
    fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
    let mut bytes = vpk("models/a.vmdl_c", 7);
    bytes[34..36].copy_from_slice(&0u16.to_le_bytes());
    bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
    fs::write(&directory, &bytes).unwrap();
    fs::write(&chunk, b"initial external chunk").unwrap();
    import(&mut s, &f);
    fs::write(&chunk, b"changed external chunk").unwrap();
    assert_eq!(
        review(&mut s, &f, "other").unwrap_err().kind,
        "externalChange"
    );
    assert_eq!(fs::read(directory).unwrap(), bytes);
    assert_eq!(fs::read(chunk).unwrap(), b"changed external chunk");
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        BEFORE.as_bytes()
    );
    assert!(!f.game.join("game/citadel/deadlock_native").exists());
}
#[test]
fn restore_refuses_legacy_prior_setup_without_complete_footprint_proof() {
    let (f, mut s) = setup();
    apply(&mut s, &f, "default", "1", "apply");
    let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    let mut d = st.document().unwrap();
    d["previous"].remove("upstreamEvidenceVersion");
    st.commit(&lock, 2, "legacy", &object! {}, d, None).unwrap();
    drop(st);
    drop(lock);
    assert_eq!(
        s.dispatch("native.restore", &mutation(&f, "3", "restore"))
            .unwrap_err()
            .kind,
        "priorSetupUnavailable"
    );
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        before
    );
}
#[test]
fn cleaned_inactive_manifest_entry_does_not_block_verified_active_packages() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
    let mut state = f.state();
    state["local-config"]["state"]["localMods"].push(object!{id:"cleaned",remoteId:"cleaned",name:"Cleaned inactive",status:"disabled",installOrder:2}).unwrap();
    f.write_state(state);
    let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
    manifest["mods"]["cleaned"] = object! {enabled:false,order:2,shard:1,currentVpks:json::array![],disabledVpks:json::array![],originalVpkNames:json::array!["old_a_dir.vpk","old_b_dir.vpk"]};
    f.write_manifest("", manifest);
    let imported = import(&mut s, &f);
    let cleaned = imported["mods"]
        .members()
        .find(|m| m["id"] == "cleaned")
        .unwrap();
    assert_eq!(cleaned["available"], false);
    assert_eq!(cleaned["enabled"], false);
    let r = review(&mut s, &f, "default").unwrap();
    assert_eq!(r["fileCount"], 1);
    let applied = apply(&mut s, &f, "default", "1", "apply");
    assert_eq!(applied["appliedProfileId"], "default");
    s.dispatch("native.restore", &mutation(&f, "2", "restore"))
        .unwrap();
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        BEFORE.as_bytes()
    );
}

fn family_setup(mounted: bool) -> (Fixture, NativeService, Vec<std::path::PathBuf>) {
    let f = Fixture::new();
    let paths = family_fixture(&f, false);
    fs::write(
        f.game.join("game/citadel/gameinfo.gi"),
        if mounted {
            BEFORE
        } else {
            "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n"
        },
    )
    .unwrap();
    let mut s = NativeService::new();
    import(&mut s, &f);
    (f, s, paths)
}
#[test]
fn family_apply_generates_engine_names_and_equivalent_resources() {
    use modlock_helper::vpk;
    use std::collections::BTreeMap;
    let (f, mut s, source) = family_setup(false);
    let before: Vec<_> = source
        .iter()
        .map(|p| {
            (
                fs::read(p).unwrap(),
                fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    let r = review(&mut s, &f, "default").unwrap();
    assert_eq!(
        (
            r["packageCount"].as_u64(),
            r["directoryCount"].as_u64(),
            r["physicalFileCount"].as_u64(),
            r["companionFileCount"].as_u64()
        ),
        (Some(1), Some(1), Some(3), Some(2))
    );
    assert_eq!(
        r["totalBytes"].as_u64().unwrap(),
        before.iter().map(|(b, _)| b.len() as u64).sum::<u64>()
    );
    let mut request = mutation(&f, "1", "family");
    request["token"] = r["token"].clone();
    let applied = s.dispatch("native.apply", &support::current_layout(&request)).unwrap();
    s = NativeService::new();
    assert_eq!(s.dispatch("native.apply", &support::current_layout(&request)).unwrap(), applied);
    let d = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(d["applied"]["manifest"]["version"], 3);
    let dir = f
        .game
        .join("game/citadel/deadlock_native")
        .join(d["applied"]["generation"].as_str().unwrap());
    let members = BTreeMap::from([
        (0, dir.join("shard01/pak01_000.vpk")),
        (17, dir.join("shard01/pak01_017.vpk")),
    ]);
    let directory = vpk::inspect_directory(&dir.join("shard01/pak01_dir.vpk")).unwrap();
    assert_eq!(
        vpk::read_resource(&dir.join("shard01/pak01_dir.vpk"), &directory, &members, 0, 100).unwrap(),
        b"preone"
    );
    assert_eq!(
        vpk::read_resource(&dir.join("shard01/pak01_dir.vpk"), &directory, &members, 1, 100).unwrap(),
        b"two!"
    );
    assert_eq!(fs::read_dir(dir.join("shard01")).unwrap().count(), 3);
    assert!(dir.join("manifest.json").is_file());
    s.dispatch("native.restore", &mutation(&f, "2", "restore"))
        .unwrap();
    for (p, (bytes, mtime)) in source.iter().zip(before) {
        assert_eq!(fs::read(p).unwrap(), bytes);
        assert_eq!(fs::metadata(p).unwrap().modified().unwrap(), mtime);
    }
}
#[test]
fn whole_family_coalescing_preserves_mods_and_priority() {
    let (f, mut s, _) = family_setup(false);
    edit(&f, |d| {
        let original = d["profiles"]["default"]["mods"][0].clone();
        let mut equal = original.clone();
        equal["source"]["id"] = "earlier".into();
        equal["source"]["installOrder"] = (-1).into();
        let mut different = original.clone();
        different["source"]["id"] = "different".into();
        let bytes = b"NEW";
        let id = "a".repeat(32);
        fs::write(f.data.join("payloads").join(format!("{id}.vpk")), bytes).unwrap();
        different["packages"][0]["companions"][0]["file"]["id"] = id.into();
        different["packages"][0]["companions"][0]["file"]["sha256"] =
            modlock_helper::paths::digest_bytes(bytes).unwrap().into();
        d["profiles"]["default"]["mods"].push(equal).unwrap();
        d["profiles"]["default"]["mods"].push(different).unwrap();
    });
    let r = review(&mut s, &f, "default").unwrap();
    assert_eq!(r["packageCount"], 3);
    assert_eq!(r["fileCount"], 2);
    assert_eq!(r["physicalFileCount"], 6);
    apply(&mut s, &f, "default", "2", "family");
    let d = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(
        d["applied"]["manifest"]["shards"][0]["packages"][0]["modIds"],
        json::array!["earlier", "local-a"]
    );
    assert_eq!(
        d["applied"]["manifest"]["shards"][0]["packages"][1]["modIds"],
        json::array!["different"]
    );
}
#[test]
fn companions_do_not_consume_directory_capacity() {
    for count in [99, 100] {
        let (f, mut s, _) = family_setup(false);
        edit(&f, |d| {
            let original = d["profiles"]["default"]["mods"][0].clone();
            let mut mods = JsonValue::new_array();
            for i in 0..count {
                let mut m = original.clone();
                m["source"]["id"] = format!("m{i}").into();
                let id = format!("{i:032x}");
                let bytes = format!("{i:03}");
                fs::write(
                    f.data.join("payloads").join(format!("{id}.vpk")),
                    bytes.as_bytes(),
                )
                .unwrap();
                m["packages"][0]["companions"][0]["file"]["id"] = id.into();
                m["packages"][0]["companions"][0]["file"]["sha256"] =
                    modlock_helper::paths::digest_bytes(bytes.as_bytes())
                        .unwrap()
                        .into();
                mods.push(m).unwrap();
            }
            let mut unavailable = original;
            unavailable["enabled"] = false.into();
            unavailable["available"] = false.into();
            unavailable["source"]["id"] = "off".into();
            unavailable["packages"] = json::array![];
            mods.push(unavailable).unwrap();
            d["profiles"]["default"]["mods"] = mods;
        });
        let r = review(&mut s, &f, "default");
        let r = r.unwrap();
        assert_eq!(r["packageCount"], count);
        assert_eq!(r["directoryCount"], count);
        assert_eq!(r["physicalFileCount"], count * 3);
        assert_eq!(r["shardCount"], if count == 99 {1} else {2});
        assert_eq!(Store::open(&f.data).unwrap().journals().unwrap().len(), 0);
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
    }
}

#[test]
fn logical_import_mapping_does_not_prove_prior_engine_footprint() {
    let (f, mut s, _) = family_setup(true);
    let r = review(&mut s, &f, "default").unwrap();
    assert!(r["warnings"]
        .members()
        .any(|w| w["code"] == "priorMissingFiles"));
    apply(&mut s, &f, "default", "1", "apply");
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(doc["previous"]["upstreamEvidenceVersion"], 2);
    assert!(doc["previous"]["upstreamPayloads"]
        .members()
        .any(|p| p["sourcePath"]
            .as_str()
            .unwrap()
            .ends_with("renamed-directory_000.vpk")
            && p["state"] == "missing"));
    s.dispatch("native.restore", &mutation(&f, "2", "restore"))
        .unwrap();
    assert_eq!(
        fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
        BEFORE.as_bytes()
    );
}
#[test]
fn prior_unlisted_companion_is_evidence_not_adoption() {
    let f = Fixture::new();
    family_fixture(&f, false);
    let extra = f.game.join("game/citadel/addons/renamed-directory_000.vpk");
    fs::write(&extra, b"old engine bytes").unwrap();
    let mtime = fs::metadata(&extra).unwrap().modified().unwrap();
    fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
    let mut s = NativeService::new();
    import(&mut s, &f);
    assert_eq!(fs::read_dir(f.data.join("payloads")).unwrap().count(), 3);
    apply(&mut s, &f, "default", "1", "apply");
    s.dispatch("native.restore", &mutation(&f, "2", "restore"))
        .unwrap();
    assert_eq!(fs::read(&extra).unwrap(), b"old engine bytes");
    assert_eq!(fs::metadata(&extra).unwrap().modified().unwrap(), mtime);
}
#[test]
fn prior_absence_changes_block_apply_and_restore() {
    for after in [false, true] {
        let (f, mut s, _) = family_setup(true);
        let r = review(&mut s, &f, "default").unwrap();
        if after {
            let mut p = mutation(&f, "1", "apply");
            p["token"] = r["token"].clone();
            s.dispatch("native.apply", &support::current_layout(&p)).unwrap();
        }
        fs::write(
            f.game.join("game/citadel/addons/renamed-directory_017.vpk"),
            b"new",
        )
        .unwrap();
        let live = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        let result = if after {
            s.dispatch("native.restore", &mutation(&f, "2", "restore"))
        } else {
            let mut p = mutation(&f, "1", "apply");
            p["token"] = r["token"].clone();
            s.dispatch("native.apply", &support::current_layout(&p))
        };
        assert!(result.is_err());
        assert_eq!(
            fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            live
        );
    }
}
#[test]
fn parseable_claimed_companion_tracks_its_own_prior_dependencies() {
    for present in [false, true] {
        let f = Fixture::new();
        let paths = family_fixture(&f, false);
        let nested = family_vpk(
            1,
            &[FamilyEntry {
                path: "materials/nested.vmat_c",
                crc: 9,
                archive: 3,
                offset: 0,
                length: 1,
                preload: b"",
            }],
            b"",
        );
        fs::write(&paths[1], nested).unwrap();
        let renamed = paths[1].with_file_name("pak42_dir.vpk");
        fs::rename(&paths[1], &renamed).unwrap();
        let mut m = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        m["mods"]["remote-a"]["currentVpks"][1] = "pak42_dir.vpk".into();
        f.write_manifest("", m);
        let extra = renamed.with_file_name("pak42_003.vpk");
        if present {
            fs::write(&extra, b"x").unwrap();
        }
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        let mut s = NativeService::new();
        import(&mut s, &f);
        assert_eq!(review(&mut s, &f, "default").unwrap()["packageCount"], 1);
        apply(&mut s, &f, "default", "1", "apply");
        fs::write(&extra, b"changed").unwrap();
        assert_eq!(
            s.dispatch("native.restore", &mutation(&f, "2", "restore"))
                .unwrap_err()
                .kind,
            "externalChange"
        );
    }
}

#[test]
fn family_restore_and_retention_check_every_member() {
    for state in [
        "current",
        "previous",
        "retired-missing",
        "retired-replaced",
        "retired-unknown",
    ] {
        let (f, mut s, _) = family_setup(false);
        apply(&mut s, &f, "default", "1", "one");
        let first = Store::open(&f.data).unwrap().document().unwrap()["applied"]["generation"]
            .as_str()
            .unwrap()
            .to_owned();
        if state != "current" {
            apply(&mut s, &f, "default", "2", "two");
        }
        let dir = f.game.join("game/citadel/deadlock_native").join(first);
        let member = dir.join("shard01/pak01_017.vpk");
        if state == "retired-missing" {
            fs::remove_file(&member).unwrap();
        } else if state == "retired-replaced" {
            let bytes = fs::read(&member).unwrap();
            fs::rename(&member, dir.join("old")).unwrap();
            fs::write(&member, &bytes).unwrap();
            fs::remove_file(dir.join("old")).unwrap();
        } else if state == "retired-unknown" {
            fs::write(dir.join("unknown"), b"keep").unwrap();
        } else {
            fs::set_permissions(&member, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .unwrap();
            fs::write(&member, b"changed").unwrap();
        }
        if state.starts_with("retired") {
            // Current transactions refuse unverifiable retained checkpoints
            // before retiring anything. An unknown file never grants cleanup.
            refuse_reapply_preserving(&mut s,&f,"default","3","three",
                if state=="retired-missing" {"nativeIo"}else{"externalChange"});
            assert!(dir.join("shard01/pak01_dir.vpk").exists());
            assert!(dir.join("shard01/pak01_000.vpk").exists());
        } else {
            let before = fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
            assert!(s
                .dispatch(
                    "native.restore",
                    &mutation(&f, if state == "current" { "2" } else { "3" }, "restore")
                )
                .is_err());
            assert_eq!(
                fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
                before
            );
        }
    }
}
#[test]
fn family_stale_companion_identity_after_review_blocks_commit() {
    let (f, mut s, _) = family_setup(true);
    let r = review(&mut s, &f, "default").unwrap();
    let st = Store::open(&f.data).unwrap();
    let d = st.document().unwrap();
    let id = d["profiles"]["default"]["mods"][0]["packages"][0]["companions"][1]["file"]["id"]
        .as_str()
        .unwrap();
    let p = st.payload_path(id).unwrap();
    let saved = p.with_extension("saved");
    fs::rename(&p, &saved).unwrap();
    fs::write(&p, fs::read(&saved).unwrap()).unwrap();
    let mut q = mutation(&f, "1", "apply");
    q["token"] = r["token"].clone();
    assert_eq!(
        s.dispatch("native.apply", &support::current_layout(&q)).unwrap_err().kind,
        "stalePreview"
    );
    assert_eq!(st.journals().unwrap().len(), 0);
    assert!(!f.game.join("game/citadel/deadlock_native").exists());
}
#[test]
fn unverifiable_prior_family_blocks_without_game_write() {
    for case in ["malformed", "symlink", "unsafe-parent", "manifest-only"] {
        let f = Fixture::new();
        let paths = family_fixture(&f, false);
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        match case {
            "malformed" => fs::write(&paths[0], b"invalid directory").unwrap(),
            "symlink" => {
                fs::remove_file(&paths[0]).unwrap();
                std::os::unix::fs::symlink(&f.source, &paths[0]).unwrap();
            }
            "unsafe-parent" => {
                let addons = f.game.join("game/citadel/addons");
                let saved = f.game.join("game/citadel/saved");
                fs::rename(&addons, &saved).unwrap();
                std::os::unix::fs::symlink(saved, addons).unwrap();
            }
            _ => {
                let mut m = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
                m["mods"]["ghost"] = object! {enabled:true,shard:1,currentVpks:json::array!["ghost.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["ghost_dir.vpk"]};
                f.write_manifest("", m);
                fs::write(paths[0].with_file_name("ghost.vpk"), b"unparseable").unwrap();
            }
        }
        let mut s = NativeService::new();
        import(&mut s, &f);
        let result = review(&mut s, &f, "default");
        if case == "manifest-only" {
            assert!(result.is_ok(), "opaque manifest-owned files need only byte proof");
        } else {
            assert!(result.is_err(), "{case}");
        }
        assert_eq!(
            fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),
            BEFORE.as_bytes()
        );
        assert!(!f.game.join("game/citadel/deadlock_native").exists());
    }
}

#[test]
fn family_maximum_archive_and_preload_only_members_use_exact_engine_names() {
    let f = Fixture::new();
    let paths = family_fixture(&f, false);
    fs::write(
        &paths[0],
        family_vpk(
            1,
            &[
                FamilyEntry {
                    path: "materials/max.vmat_c",
                    crc: 1,
                    archive: 65535,
                    offset: 0,
                    length: 3,
                    preload: b"p",
                },
                FamilyEntry {
                    path: "materials/pre.vmat_c",
                    crc: 2,
                    archive: 17,
                    offset: u32::MAX,
                    length: 0,
                    preload: b"only",
                },
            ],
            b"",
        ),
    )
    .unwrap();
    let mut m = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
    m["mods"]["remote-a"]["currentVpks"] = json::array!["renamed-directory.vpk", "unrelated-a.vpk"];
    m["mods"]["remote-a"]["originalVpkNames"] = json::array!["skin.vpk", "skin_65535.vpk"];
    f.write_manifest("", m);
    fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
    let mut s = NativeService::new();
    import(&mut s, &f);
    assert_eq!(
        review(&mut s, &f, "default").unwrap()["physicalFileCount"],
        2
    );
    apply(&mut s, &f, "default", "1", "apply");
    let d = Store::open(&f.data).unwrap().document().unwrap();
    let dir = f
        .game
        .join("game/citadel/deadlock_native")
        .join(d["applied"]["generation"].as_str().unwrap());
    assert_eq!(fs::read(dir.join("shard01/pak01_65535.vpk")).unwrap(), b"one");
    assert!(!dir.join("shard01/pak01_017.vpk").exists());
    assert_eq!(fs::read_dir(dir.join("shard01")).unwrap().count(), 2);
    assert!(dir.join("manifest.json").is_file());
    let parsed = modlock_helper::vpk::inspect_directory(&dir.join("shard01/pak01_dir.vpk")).unwrap();
    let members = std::collections::BTreeMap::from([(65535, dir.join("shard01/pak01_65535.vpk"))]);
    assert_eq!(
        modlock_helper::vpk::read_resource(&dir.join("shard01/pak01_dir.vpk"), &parsed, &members, 1, 100)
            .unwrap(),
        b"only"
    );
}

#[test]
fn opaque_prior_bytes_allow_apply_restore_and_detect_changes() {
    for case in ["restore", "apply-tamper", "restore-tamper", "old-import"] {
        let f = Fixture::new();
        fs::write(f.game.join("game/citadel/gameinfo.gi"), BEFORE).unwrap();
        let bad = f.game.join("game/citadel/addons/broken_dir.vpk");
        fs::write(&bad, b"invalid VPK family").unwrap();
        let mut manifest = json::parse(&fs::read_to_string(f.manifest_path("")).unwrap()).unwrap();
        manifest["mods"]["broken"] = object! {enabled:true,shard:1,currentVpks:json::array!["broken_dir.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["broken_dir.vpk"]};
        f.write_manifest("", manifest);
        let mut state = f.state();
        state["local-config"]["state"]["localMods"].push(object! {id:"broken",remoteId:"broken",name:"Broken",status:"installed",installOrder:1}).unwrap();
        state["local-config"]["state"]["profiles"]["default"]["enabledMods"]["broken"] = object! {enabled:false};
        f.write_state(state);
        let original = tree_bytes(&f.game);
        let mut s = NativeService::new();
        let imported = import(&mut s, &f);
        assert_eq!(imported["mods"].len(), 2);
        assert_eq!(imported["mods"].members().filter(|m| m["available"] == true).count(), 1);
        let doc = Store::open(&f.data).unwrap().document().unwrap();
        let opaque = doc["importLedger"]["sourcePayloads"].members().find(|p| p["sourcePath"].as_str() == bad.to_str()).unwrap();
        assert_eq!(opaque["engineComplete"], false);
        assert_eq!(opaque["priorEvidenceComplete"], true);
        assert_eq!(opaque["engineFiles"][0]["proofKind"], "bytes");
        if case == "old-import" {
            edit(&f, |d| {
                for p in d["importLedger"]["sourcePayloads"].members_mut() {
                    p.remove("priorEvidenceComplete");
                    for r in p["engineFiles"].members_mut() {
                        if r["proofKind"] == "bytes" {
                            r.remove("proofKind");
                            r["state"] = "unverifiable".into();
                        }
                    }
                }
            });
            let e = review(&mut s, &f, "default").unwrap_err();
            assert_eq!(e.kind, "priorSetupUnavailable");
            assert!(e.message.contains("import"));
            continue;
        }
        if case == "apply-tamper" {
            fs::write(&bad, b"changed").unwrap();
            assert_eq!(review(&mut s, &f, "default").unwrap_err().kind, "externalChange");
            continue;
        }
        apply(&mut s, &f, "default", "1", "apply");
        // Reopen service to exercise durable prior evidence, not preview memory.
        let mut s = NativeService::new();
        if case == "restore-tamper" {
            fs::write(&bad, b"changed").unwrap();
            assert_eq!(s.dispatch("native.restore", &mutation(&f, "2", "restore")).unwrap_err().kind, "externalChange");
        } else {
            s.dispatch("native.restore", &mutation(&f, "2", "restore")).unwrap();
            let mut restored=tree_bytes(&f.game);
            let workspace=std::path::PathBuf::from("game/citadel/deadlock_native");
            assert_eq!(restored.remove(&workspace),Some(None));
            let owner=restored.remove(&workspace.join("owner.json")).unwrap().unwrap();
            assert_eq!(json::parse(std::str::from_utf8(&owner).unwrap()).unwrap(),object!{version:1,root:f.data.to_str()});
            assert_eq!(restored, original);
        }
    }
}
fn tree_bytes(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    fn visit(base: &std::path::Path, dir: &std::path::Path, out: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(base).unwrap().to_owned();
            if path.is_dir() {
                out.insert(relative, None);
                visit(base, &path, out);
            } else {
                out.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    visit(root, root, &mut out);
    out
}
