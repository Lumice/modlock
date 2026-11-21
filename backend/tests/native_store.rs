mod support;
use json::object;
use support::*;
#[test]
fn mutation_notifications_have_no_side_effects() {
    let f = Fixture::new();
    let request=object!{jsonrpc:"2.0",method:"native.profile",params:object!{dataDir:f.data.to_str().unwrap(),action:"create",name:"Oops",expectedRevision:"0",operationId:"op"}}.dump()+"\n";
    let mut out = Vec::new();
    modlock_helper::protocol::serve(std::io::Cursor::new(request), &mut out).unwrap();
    assert!(out.is_empty());
    assert!(!f.data.exists());
}
use modlock_helper::{
    native::NativeService,
    store::{RootLock, Store},
};

#[test]
fn fresh_import_refuses_unknown_and_game_journals_before_cleanup_or_copy() {
    for kind in ["unknown", "game"] {
        let f = Fixture::new();
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let stage = f.data.join(".import-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        std::fs::create_dir(&stage).unwrap();
        store.write_journal(&lock,"a-import",&object!{kind:"import",stage:stage.file_name().unwrap().to_str().unwrap(),input:object!{},payloads:json::array![]}).unwrap();
        store
            .write_journal(
                &lock,
                "z-foreign",
                &object! {kind:kind,version:99,input:object!{original:true}},
            )
            .unwrap();
        let journals = store.journals().unwrap();
        let outcomes = [
            store.operation("a-import").unwrap(),
            store.operation("z-foreign").unwrap(),
        ];
        drop(store);
        drop(lock);
        let mut service = NativeService::new();
        let preview = preview(&mut service, &f);
        let mut q = mutation(&f, "0", "blocked-import");
        q["token"] = preview["token"].clone();
        for _ in 0..2 {
            assert_eq!(
                service
                    .dispatch("native.importCommit", &q)
                    .unwrap_err()
                    .kind,
                "recoveryRequired"
            );
        }
        let store = Store::open_history(&f.data).unwrap();
        assert_eq!(store.document().unwrap_err().kind, "nativeNotImported");
        assert!(stage.exists());
        assert!(!f.data.join("payloads").exists());
        assert_eq!(store.journals().unwrap(), journals);
        assert_eq!(
            [
                store.operation("a-import").unwrap(),
                store.operation("z-foreign").unwrap()
            ],
            outcomes
        );
        assert_eq!(
            store.operation("blocked-import").unwrap()["status"],
            "unknown"
        );
    }
}

#[test]
fn import_reconciler_leaves_legacy_game_recovery_journals_for_their_dispatcher() {
    let f = Fixture::new();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let db =
        modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
    db.execute("INSERT INTO library(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", &[LEGACY_STORE1])
        .unwrap();
    drop(db);
    let stage = f.data.join(".import-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    std::fs::create_dir(&stage).unwrap();
    store.write_journal(&lock,"old-import",&object!{kind:"import",stage:stage.file_name().unwrap().to_str().unwrap(),input:object!{},payloads:json::array![]}).unwrap();
    let game = object! {kind:"game",version:1,input:object!{legacy:true}};
    let recovery = object! {kind:"recovery",version:1,input:object!{legacy:true}};
    store.write_journal(&lock, "game", &game).unwrap();
    store.write_journal(&lock, "recovery", &recovery).unwrap();
    modlock_helper::import::reconcile_imports(&store, &lock).unwrap();
    assert_eq!(store.journal("game").unwrap(), Some(game));
    assert_eq!(store.journal("recovery").unwrap(), Some(recovery));
    assert!(!stage.exists());
    assert_eq!(store.document().unwrap()["storeVersion"], 1);
    assert_eq!(
        store.operation("old-import").unwrap()["error"]["kind"],
        "operationInterrupted"
    );
}
#[test]
fn desired_state_reopens_revision_checks_and_operation_retries_are_durable() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let mut q = mutation(&f, "1", "toggle");
    q["profileId"] = "default".into();
    q["modId"] = "local-a".into();
    q["enabled"] = false.into();
    let v = s.dispatch("native.setEnabled", &q).unwrap();
    assert_eq!(v["revision"], "2");
    assert_eq!(v["mods"][0]["enabled"], false);
    assert_eq!(v["mods"][0]["appliedEnabled"], false);
    let mut reopened = NativeService::new();
    assert_eq!(reopened.dispatch("native.setEnabled", &q).unwrap(), v);
    q["operationId"] = "stale".into();
    assert_eq!(
        reopened.dispatch("native.setEnabled", &q).unwrap_err().kind,
        "staleRevision"
    );
    q["operationId"] = "toggle".into();
    q["enabled"] = true.into();
    assert_eq!(
        reopened.dispatch("native.setEnabled", &q).unwrap_err().kind,
        "operationIdConflict"
    );
    let mut q = f.params();
    q["operationId"] = "toggle".into();
    let op = reopened.dispatch("native.operation", &q).unwrap();
    assert_eq!(op["status"], "succeeded");
    assert_eq!(op["result"]["revision"], "2");
    q["operationId"] = "missing".into();
    assert_eq!(
        reopened.dispatch("native.operation", &q).unwrap()["status"],
        "unknown"
    );
}
#[test]
fn profiles_copy_independent_choices_and_protect_default() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let mut q = mutation(&f, "1", "copy");
    q["action"] = "copy".into();
    q["profileId"] = "default".into();
    q["name"] = "Copy".into();
    let v = s.dispatch("native.profile", &q).unwrap();
    let id = v["selectedProfileId"].as_str().unwrap().to_owned();
    assert_ne!(id, "default");
    assert_eq!(v["mods"].len(), 1);
    let mut q = mutation(&f, "2", "toggle-copy");
    q["profileId"] = id.clone().into();
    q["modId"] = "local-a".into();
    q["enabled"] = false.into();
    s.dispatch("native.setEnabled", &q).unwrap();
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap()["mods"][0]["enabled"],
        true
    );
    let mut q = mutation(&f, "3", "rename");
    q["action"] = "rename".into();
    q["profileId"] = id.clone().into();
    q["name"] = "Renamed".into();
    let v = s.dispatch("native.profile", &q).unwrap();
    assert!(v["profiles"].members().any(|p| p["name"] == "Renamed"));
    let mut q = mutation(&f, "4", "protect");
    q["action"] = "remove".into();
    q["profileId"] = "default".into();
    assert_eq!(
        s.dispatch("native.profile", &q).unwrap_err().kind,
        "protectedProfile"
    );
    q["profileId"] = id.into();
    q["operationId"] = "remove-copy".into();
    let v = s.dispatch("native.profile", &q).unwrap();
    assert_eq!(v["revision"], "5");
    assert_eq!(v["profiles"].len(), 2);
    let mut q = mutation(&f, "5", "create");
    q["action"] = "create".into();
    q["name"] = "Empty".into();
    let v = s.dispatch("native.profile", &q).unwrap();
    assert_eq!(v["mods"].len(), 0);
    assert_eq!(v["activeProfileId"], "default");
}
#[test]
fn store_root_lock_contends_across_instances_and_releases_on_drop() {
    let f = Fixture::new();
    let lock = RootLock::acquire(&f.data).unwrap();
    assert_eq!(RootLock::acquire(&f.data).unwrap_err().kind, "nativeBusy");
    drop(lock);
    assert!(RootLock::acquire(&f.data).is_ok());
}
#[test]
fn canonical_revisions_and_safe_operation_ids_are_enforced() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    for revision in ["01", "-1", "1.0", "18446744073709551616"] {
        let mut q = mutation(&f, revision, "rename");
        q["action"] = "rename".into();
        q["profileId"] = "default".into();
        q["name"] = "Test".into();
        assert_eq!(
            s.dispatch("native.profile", &q).unwrap_err().kind,
            "invalidParams"
        );
    }
    for id in ["", "../escape", "a b"] {
        let mut q = mutation(&f, "1", id);
        q["action"] = "rename".into();
        q["profileId"] = "default".into();
        q["name"] = "Test".into();
        assert_eq!(
            s.dispatch("native.profile", &q).unwrap_err().kind,
            "invalidParams"
        );
    }
    assert_eq!(
        Store::open(&f.data).unwrap().document().unwrap()["revision"],
        "1"
    );
}
#[test]
fn applied_state_is_independent_of_choices_and_protects_applied_profile() {
    // Apply for real: a checkpoint is only accepted with its generation files.
    let f = Fixture::new();
    let mut s = NativeService::new();
    std::fs::write(
        f.game.join("game/citadel/gameinfo.gi"),
        "\"GameInfo\"\r\n{\r\n FileSystem\r\n {\r\n  SearchPaths\r\n  {\r\n   Game citadel\r\n   Game core\r\n  }\r\n }\r\n}\r\n",
    )
    .unwrap();
    import(&mut s, &f);
    let mut p = f.params();
    p["profileId"] = "default".into();
    p["gamePath"] = f.game.to_str().unwrap().into();
    let review = s.dispatch("native.applyPreview", &support::current_layout(&p)).unwrap();
    let mut q = mutation(&f, "1", "apply-default");
    q["token"] = review["token"].clone();
    let v = s.dispatch("native.apply", &support::current_layout(&q)).unwrap();
    let id = v["mods"][0]["id"].clone();
    assert_eq!(v["mods"][0]["appliedEnabled"], true);
    assert_eq!(v["pendingChanges"], false);
    let revision = v["revision"].as_str().unwrap().to_owned();

    let mut q = mutation(&f, &revision, "toggle-off");
    q["profileId"] = "default".into();
    q["modId"] = id.clone();
    q["enabled"] = false.into();
    let v = s.dispatch("native.setEnabled", &q).unwrap();
    assert_eq!(v["mods"][0]["enabled"], false);
    assert_eq!(v["mods"][0]["appliedEnabled"], true);
    assert_eq!(v["pendingChanges"], true);
    assert_eq!(v["canRestore"], true);
    let revision = v["revision"].as_str().unwrap().to_owned();

    let mut q = mutation(&f, &revision, "remove-applied");
    q["action"] = "remove".into();
    q["profileId"] = "default".into();
    assert_eq!(
        s.dispatch("native.profile", &q).unwrap_err().kind,
        "protectedProfile"
    );
    let mut q = mutation(&f, &revision, "toggle-on");
    q["profileId"] = "default".into();
    q["modId"] = id;
    q["enabled"] = true.into();
    let v = s.dispatch("native.setEnabled", &q).unwrap();
    assert_eq!(v["mods"][0]["appliedEnabled"], true);
    assert_eq!(v["pendingChanges"], false);
}
#[test]
fn journals_are_durable_and_failed_commit_rolls_back_revision_and_history() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    store
        .write_journal(
            &lock,
            "apply-pending",
            &object! {phase:"prepared",gamePath:"synthetic"},
        )
        .unwrap();
    assert_eq!(
        store.operation("apply-pending").unwrap()["status"],
        "recoveryRequired"
    );
    let doc = store.document().unwrap();
    assert!(store
        .commit(&lock, 1, "rollback", &object! {}, doc, Some("missing"))
        .is_err());
    assert_eq!(store.document().unwrap()["revision"], "1");
    assert_eq!(store.operation("rollback").unwrap()["status"], "unknown");
    drop(store);
    let store = Store::open_writer(&lock).unwrap();
    assert_eq!(
        store.journal("apply-pending").unwrap().unwrap()["phase"],
        "prepared"
    );
    assert_eq!(store.journals().unwrap().len(), 1);
    store.clear_journal(&lock, "apply-pending").unwrap();
    assert!(store.journal("apply-pending").unwrap().is_none());
}
#[test]
fn rpc_mutation_is_rejected_when_other_instance_holds_native_lock() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let _lock = RootLock::acquire(&f.data).unwrap();
    let mut q = mutation(&f, "1", "contended");
    q["action"] = "rename".into();
    q["profileId"] = "default".into();
    q["name"] = "Blocked".into();
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_modlock-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        "{}",
        object! {jsonrpc:"2.0",id:1,method:"native.profile",params:q}.dump()
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let r = json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(r["error"]["data"]["kind"], "nativeBusy");
    assert_eq!(
        Store::open(&f.data).unwrap().document().unwrap()["revision"],
        "1"
    );
}
#[test]
fn rejected_mutations_reserve_ids_replay_terminal_errors_and_preserve_revision() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let mut q = mutation(&f, "1", "rejected");
    q["action"] = "remove".into();
    q["profileId"] = "default".into();
    assert_eq!(
        s.dispatch("native.profile", &q).unwrap_err().kind,
        "protectedProfile"
    );
    let mut reopened = NativeService::new();
    let mut read = f.params();
    read["operationId"] = "rejected".into();
    let op = reopened.dispatch("native.operation", &read).unwrap();
    assert_eq!(op["status"], "failed");
    assert_eq!(op["error"]["kind"], "protectedProfile");
    assert_eq!(
        Store::open(&f.data).unwrap().document().unwrap()["revision"],
        "1"
    );
    assert_eq!(
        reopened.dispatch("native.profile", &q).unwrap_err().kind,
        "protectedProfile"
    );
    q["profileId"] = "other".into();
    assert_eq!(
        reopened.dispatch("native.profile", &q).unwrap_err().kind,
        "operationIdConflict"
    );
}
#[test]
fn pending_journal_marks_snapshot_recovery_required_without_changing_desired_state() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    store
        .write_journal(&lock, "pending", &object! {phase:"prepared"})
        .unwrap();
    assert_eq!(store.snapshot(None).unwrap()["recoveryRequired"], true);
    assert_eq!(store.snapshot(None).unwrap()["mods"][0]["enabled"], true);
}
#[test]
fn malformed_mutation_does_not_reserve_operation_id_even_with_stale_revision() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let mut q = mutation(&f, "0", "malformed");
    q["action"] = "rename".into();
    q["profileId"] = "default".into();
    assert_eq!(
        s.dispatch("native.profile", &q).unwrap_err().kind,
        "invalidParams"
    );
    let mut read = f.params();
    read["operationId"] = "malformed".into();
    assert_eq!(
        s.dispatch("native.operation", &read).unwrap()["status"],
        "unknown"
    );
}
#[test]
fn local_mod_choices_remain_independent_when_two_entries_share_remote_id() {
    let f = Fixture::new();
    let mut state = f.state();
    let mut duplicate = state["local-config"]["state"]["localMods"][0].clone();
    duplicate["id"] = "local-a-variant".into();
    state["local-config"]["state"]["localMods"]
        .push(duplicate)
        .unwrap();
    f.write_state(state);
    let mut s = NativeService::new();
    import(&mut s, &f);
    let mut q = mutation(&f, "1", "one-variant");
    q["profileId"] = "default".into();
    q["modId"] = "local-a".into();
    q["enabled"] = false.into();
    let v = s.dispatch("native.setEnabled", &q).unwrap();
    assert_eq!(v["mods"][0]["enabled"], false);
    assert_eq!(v["mods"][1]["enabled"], true);
}
#[test]
fn store1_adapter_is_read_only_and_keeps_unavailable_entries() {
    let f = Fixture::new();
    let lock = RootLock::acquire(&f.data).unwrap();
    let st = Store::open_writer(&lock).unwrap();
    drop(st);
    let db =
        modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
    db.execute("INSERT INTO library(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", &[LEGACY_STORE1])
        .unwrap();
    drop(db);
    drop(lock);
    let before = std::fs::read(f.data.join("library.sqlite3")).unwrap();
    let mut s = NativeService::new();
    let snap = s.dispatch("native.snapshot", &f.params()).unwrap();
    assert_eq!(snap["storeVersion"], 1);
    assert_eq!(snap["mods"][0]["available"], false);
    assert_eq!(
        std::fs::read(f.data.join("library.sqlite3")).unwrap(),
        before
    );
    let mut q = mutation(&f, "1", "legacy-edit");
    q["profileId"] = "default".into();
    q["modId"] = "old".into();
    q["enabled"] = false.into();
    s.dispatch("native.setEnabled", &q).unwrap();
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(doc["storeVersion"], 1);
    assert!(doc["profiles"]["default"]["mods"][0].has_key("payloads"));
    assert_eq!(doc["profiles"]["default"]["mods"][0]["available"], false);
    assert_eq!(doc["unknown"]["keep"], true);
}

#[test]
fn store2_rejects_malformed_package_records() {
    let file = object! {id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",sha256:"b".repeat(64),size:3,sourceName:"a.vpk"};
    let base = object! {directory:file.clone(),companions:json::array![object!{archiveIndex:0,file:file.clone()}]};
    for case in [
        "valid",
        "shared",
        "duplicate",
        "inline",
        "fraction",
        "overflow",
        "badsize",
        "negative",
        "id",
        "hash",
        "mixed",
        "future",
        "inconsistent",
    ] {
        let f = Fixture::new();
        let mut doc = json::parse(LEGACY_STORE1).unwrap();
        doc["storeVersion"] = 2.into();
        let m = &mut doc["profiles"]["default"]["mods"][0];
        m.remove("payloads");
        m["packages"] = json::array![base.clone()];
        match case {
            "shared" => {
                m["packages"][0]["companions"]
                    .push(object! {archiveIndex:17,file:file.clone()})
                    .unwrap();
            }
            "duplicate" => {
                m["packages"][0]["companions"]
                    .push(object! {archiveIndex:0,file:file.clone()})
                    .unwrap();
            }
            "inline" => m["packages"][0]["companions"][0]["archiveIndex"] = 32767.into(),
            "fraction" => m["packages"][0]["companions"][0]["archiveIndex"] = 0.5.into(),
            "overflow" => m["packages"][0]["companions"][0]["archiveIndex"] = 65536.into(),
            "badsize" => m["packages"][0]["directory"]["size"] = 0.5.into(),
            "negative" => m["packages"][0]["directory"]["size"] = (-1).into(),
            "id" => m["packages"][0]["directory"]["id"] = "../bad".into(),
            "hash" => m["packages"][0]["directory"]["sha256"] = "bad".into(),
            "mixed" => m["payloads"] = json::array![],
            "inconsistent" => m["packages"][0]["directory"]["size"] = 4.into(),
            "future" => doc["storeVersion"] = 3.into(),
            _ => {}
        }
        let lock = RootLock::acquire(&f.data).unwrap();
        drop(Store::open_writer(&lock).unwrap());
        let db =
            modlock_helper::sqlite::Database::open(&f.data.join("library.sqlite3"), true).unwrap();
        db.execute("INSERT INTO library(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", &[&doc.dump()])
            .unwrap();
        drop(db);
        drop(lock);
        let result = Store::open(&f.data);
        assert_eq!(result.is_ok(), matches!(case, "valid" | "shared"), "{case}");
    }
}

#[test]
fn store2_profile_copy_preserves_family() {
    let f = Fixture::new();
    family_fixture(&f, false);
    let mut service = NativeService::new();
    import(&mut service, &f);
    let mut q = mutation(&f, "1", "copy");
    q["action"] = "copy".into();
    q["profileId"] = "default".into();
    q["name"] = "Copy".into();
    let result = service.dispatch("native.profile", &q).unwrap();
    assert_eq!(result["storeVersion"], 2);
    let doc = Store::open(&f.data).unwrap().document().unwrap();
    assert_eq!(
        doc["profiles"][result["selectedProfileId"].as_str().unwrap()]["mods"],
        doc["profiles"]["default"]["mods"]
    );
}
#[test]
fn store2_game_operations_are_enabled_and_recovery_replays() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    std::fs::write(
        f.game.join("game/citadel/gameinfo.gi"),
        "GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n",
    )
    .unwrap();
    let mut q = f.params();
    q["profileId"] = "default".into();
    q["gamePath"] = f.game.to_str().unwrap().into();
    let preview = s.dispatch("native.applyPreview", &support::current_layout(&q)).unwrap();
    assert_eq!(preview["packageCount"], 1);
    let mut q = mutation(&f, "1", "apply-v2");
    q["token"] = preview["token"].clone();
    s.dispatch("native.apply", &support::current_layout(&q)).unwrap();
    s.dispatch("native.restore", &mutation(&f, "2", "restore-v2"))
        .unwrap();
    let q = mutation(&f, "3", "recover-v2");
    let recovered = s.dispatch("native.recover", &q).unwrap();
    assert_eq!(recovered["storeVersion"], 4);
    assert_eq!(s.dispatch("native.recover", &q).unwrap(), recovered);
    assert_eq!(Store::open(&f.data).unwrap().journals().unwrap().len(), 0);
}
#[test]
fn package_adapter_rejects_legacy_family_layout_and_future_version() {
    let file = object! {id:"a".repeat(32),sha256:"b".repeat(64),size:1,sourceName:"legacy.vpk",companions:json::array![object!{archiveIndex:0}]};
    let record = object! {available:false,payloads:json::array![file]};
    assert!(modlock_helper::package::mod_packages(&record, 1).is_err());
    assert_eq!(
        modlock_helper::package::mod_packages(&record, 3)
            .unwrap_err()
            .kind,
        "unsupportedVersion"
    );
    let valid = object! {available:false,payloads:json::array![object!{id:"a".repeat(32),sha256:"b".repeat(64),size:1,sourceName:"legacy.vpk"}]};
    let adapted = modlock_helper::package::mod_packages(&valid, 1).unwrap();
    assert_eq!(adapted.len(), 1);
    assert!(adapted[0].companions.is_empty());
}

#[test]
fn store2_recovery_refuses_malformed_game_unknown_journals_and_stale_revision() {
    for kind in ["game", "unknown"] {
        let f = Fixture::new();
        let mut service = NativeService::new();
        import(&mut service, &f);
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let original = store.document().unwrap();
        store
            .write_journal(&lock, "foreign", &object! {kind:kind,version:2})
            .unwrap();
        drop(store);
        drop(lock);
        let stale = mutation(&f, "0", "stale-recover");
        assert_eq!(
            service.dispatch("native.recover", &stale).unwrap_err().kind,
            "staleRevision"
        );
        let q = mutation(&f, "1", "refuse-recover");
        assert_eq!(
            service.dispatch("native.recover", &q).unwrap_err().kind,
            "recoveryRequired"
        );
        let store = Store::open_history(&f.data).unwrap();
        assert_eq!(store.document().unwrap(), original);
        assert_eq!(store.journals().unwrap().len(), 1);
        assert_eq!(
            store.operation("refuse-recover").unwrap()["status"],
            "unknown"
        );
    }
}
