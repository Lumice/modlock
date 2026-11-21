mod support;
use modlock_helper::{
    native::NativeService,
    protocol::MAX_RESPONSE_BYTES,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
use support::*;

/// Pad `sourcePath` to the largest size the store still accepts, so any growth
/// of a profile crosses the readable-response limit.
fn pad_to_budget(doc: &mut JsonValue) {
    let (mut low, mut high) = (0usize, MAX_RESPONSE_BYTES);
    while low + 1 < high {
        let mid = (low + high) / 2;
        doc["sourcePath"] = "x".repeat(mid).into();
        if modlock_helper::store::ensure_readable(doc).is_ok() { low = mid; } else { high = mid; }
    }
    doc["sourcePath"] = "x".repeat(low).into();
}

#[test]
fn store2_internal_document_and_journal_limits_precede_files() {
    use modlock_helper::import::ImportPlan;
    let f = Fixture::new();
    family_fixture(&f, false);
    let mut plan = ImportPlan::review(&f.data, &f.source, Some(&f.game)).unwrap();
    plan.document["unknownLargeProvenance"] = "x".repeat(64 * 1024 * 1024).into();
    let lock = RootLock::acquire(&f.data).unwrap();
    assert_eq!(
        plan.commit(&lock, "internal-limit", &object! {})
            .unwrap_err()
            .kind,
        "sourceTooLarge"
    );
    assert!(!f.data.join("payloads").exists());
    assert!(!f.data.join("library.sqlite3").exists());
    let store = Store::open_writer(&lock).unwrap();
    let journal = object! {kind:"import",unknownLargeEvidence:"x".repeat(64*1024*1024)};
    assert_eq!(
        store
            .write_journal(&lock, "large-journal", &journal)
            .unwrap_err()
            .kind,
        "sourceTooLarge"
    );
    assert_eq!(store.journals().unwrap().len(), 0);
}

#[test]
fn store2_every_profile_and_outcome_is_admitted_before_commit() {
    let f = Fixture::new();
    family_fixture(&f, false);
    let mut service = NativeService::new();
    let snap = import(&mut service, &f);
    assert_eq!(snap["storeVersion"], 2);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let original = store.document().unwrap();
    let mut doc = original.clone();
    doc["profiles"]["hidden"] = doc["profiles"]["default"].clone();
    doc["profiles"]["hidden"]["mods"][0]["source"]["name"] = "x".repeat(MAX_RESPONSE_BYTES).into();
    assert_eq!(
        store
            .commit(&lock, 1, "large-family", &object! {}, doc, Some("default"))
            .unwrap_err()
            .kind,
        "nativeResponseLimit"
    );
    assert_eq!(store.document().unwrap(), original);
    assert_eq!(
        store.operation("large-family").unwrap()["status"],
        "unknown"
    );
    assert_eq!(store.journals().unwrap().len(), 0);
}

#[test]
fn oversized_unselected_profile_rejects_store_commit_without_revision_or_outcome() {
    let f = Fixture::new();
    import(&mut NativeService::new(), &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let original = store.document().unwrap();
    let mut doc = original.clone();
    doc["profiles"]["other"]["mods"][0]["source"]["name"] = "x".repeat(MAX_RESPONSE_BYTES).into();
    let result = store.commit(&lock, 1, "oversized", &object! {}, doc, Some("default"));
    assert_eq!(result.unwrap_err().kind, "nativeResponseLimit");
    assert_eq!(store.document().unwrap(), original);
    assert_eq!(store.operation("oversized").unwrap()["status"], "unknown");
    assert_eq!(store.journals().unwrap().len(), 0);
}

#[test]
fn real_helper_rejects_100000_mod_import_before_commit_and_reconciles_failure() {
    let f = Fixture::new();
    let mods = JsonValue::from(
        (0..100_000)
            .map(|n| object! {id:format!("m{n}"),remoteId:format!("r{n}"),status:"disabled"})
            .collect::<Vec<_>>(),
    );
    f.write_state(object!{"local-config":object!{version:33,state:object!{activeProfileId:"default",profiles:object!{default:object!{name:"Default",mods:mods}}}}});
    assert!(std::fs::metadata(&f.source).unwrap().len() < 6 * 1024 * 1024);
    let before = std::fs::read(&f.source).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_modlock-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut call = |method: &str, params: JsonValue| {
        writeln!(input, "{}", object!{jsonrpc:"2.0",id:json::parse("-9223372036854775808").unwrap(),method:method,params:params}.dump()).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert!(line.len() <= MAX_RESPONSE_BYTES);
        json::parse(&line).unwrap()
    };
    let preview = call(
        "native.importPreview",
        object! {dataDir:f.data.to_str().unwrap(),statePath:f.source.to_str().unwrap()},
    );
    assert!(preview["result"]["token"].is_string());
    let mut params = mutation(&f, "0", &"o".repeat(64));
    params["token"] = preview["result"]["token"].clone();
    let reply = call("native.importCommit", params.clone());
    assert_eq!(
        reply["error"]["data"]["kind"],
        "nativeResponseLimit",
        "{}",
        reply.dump()
    );
    assert_eq!(
        call("native.importCommit", params)["error"]["data"]["kind"],
        "nativeResponseLimit"
    );
    assert_eq!(
        call("native.snapshot", f.params())["error"]["data"]["kind"],
        "nativeNotImported"
    );
    let outcome = call(
        "native.operation",
        object! {dataDir:f.data.to_str().unwrap(),operationId:"o".repeat(64)},
    );
    assert_eq!(outcome["result"]["status"], "failed");
    assert_eq!(outcome["result"]["error"]["kind"], "nativeResponseLimit");
    drop(input);
    assert!(child.wait().unwrap().success());
    let store = Store::open_history(&f.data).unwrap();
    assert_eq!(store.document().unwrap_err().kind, "nativeNotImported");
    assert_eq!(store.journals().unwrap().len(), 0);
    assert!(!f.data.join("payloads").exists());
    assert!(!f.game.join("game/citadel/deadlock_native").exists());
    assert_eq!(std::fs::read(&f.source).unwrap(), before);
}

#[test]
fn family_escaped_future_applied_fields_and_operation_envelope_are_budgeted_before_commit() {
    // The current snapshot fits the wire, but two future profile-ID fields and
    // the operation/framing reserve do not. Escaping, not character count, matters.
    let f = Fixture::new();
    family_fixture(&f, false);
    let id = "\u{1}".repeat(400_000);
    let title = "x".repeat(MAX_RESPONSE_BYTES - 7_000_000);
    import(&mut NativeService::new(), &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let original = store.document().unwrap();
    let mut document = original.clone();
    document["profiles"]["default"]["mods"][0]["source"]["name"] = title.into();
    document["profiles"][&id] = object! {name:"Other",mods:json::array![]};
    let snapshot = modlock_helper::store::snapshot_document(&document, Some("default")).unwrap();
    assert!(snapshot.dump().len() + 1000 < MAX_RESPONSE_BYTES);
    let result = store.commit(&lock, 1, "growth", &object! {}, document, Some("default"));
    assert_eq!(
        result.err().map(|e| e.kind).as_deref(),
        Some("nativeResponseLimit")
    );
    assert_eq!(store.document().unwrap(), original);
}

#[test]
fn near_budget_success_is_readable_on_real_wire_and_profile_growth_is_atomic() {
    let f = Fixture::new();
    let mut service = NativeService::new();
    import(&mut service, &f);
    let id = "s".repeat(64);
    {
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let mut doc = store.document().unwrap();
        doc["sourcePath"] = "".into();
        pad_to_budget(&mut doc);
        let result = store
            .commit(&lock, 1, &id, &object! {}, doc, Some("default"))
            .unwrap();
        assert!(result.dump().len() > MAX_RESPONSE_BYTES - 70 * 1024);
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_modlock-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for method in ["native.snapshot", "native.operation"] {
        let mut params = f.params();
        if method == "native.operation" {
            params["operationId"] = id.clone().into();
        }
        writeln!(input, "{}", object!{jsonrpc:"2.0",id:json::parse("-9223372036854775808").unwrap(),method:method,params:params}.dump()).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert!(line.len() > MAX_RESPONSE_BYTES - 70 * 1024 && line.len() <= MAX_RESPONSE_BYTES);
        let response = json::parse(&line).unwrap();
        assert!(!response.has_key("error"));
        if method == "native.operation" {
            assert_eq!(response["result"]["operationId"], id.as_str());
            assert_eq!(response["result"]["status"], "succeeded");
            assert_eq!(response["result"]["result"]["revision"], "2");
        } else {
            assert_eq!(response["result"]["revision"], "2");
        }
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    for action in ["rename", "copy", "create"] {
        let mut params = mutation(&f, "2", action);
        params["action"] = action.into();
        params["profileId"] = "other".into();
        params["name"] = "n".repeat(80).into();
        let result = service.dispatch("native.profile", &params);
        assert_eq!(
            result.err().map(|e| e.kind).as_deref(),
            Some("nativeResponseLimit"),
            "{action}"
        );
        let store = Store::open(&f.data).unwrap();
        assert_eq!(store.document().unwrap()["revision"], "2");
        assert_eq!(store.document().unwrap()["profiles"].len(), 2);
        assert_eq!(
            store.document().unwrap()["profiles"]["other"]["name"],
            "Other"
        );
        assert_eq!(store.operation(action).unwrap()["status"], "failed");
        assert_eq!(store.journals().unwrap().len(), 0);
    }
}

#[test]
fn thousands_of_empty_profiles_remain_readable() {
    let mut doc = object! {storeVersion:1,revision:"1",sourcePath:"/synthetic",gamePath:"",activeProfileId:"p0",profiles:object!{},applied:JsonValue::Null};
    for n in 0..2000 {
        doc["profiles"][format!("p{n}")] =
            object! {name:format!("Profile {n}"),mods:json::array![],protected:false};
    }
    let started = std::time::Instant::now();
    modlock_helper::store::ensure_readable(&doc).unwrap();
    println!("2000-profile admission: {:?}", started.elapsed());
    for id in ["p0", "p1999"] {
        let snapshot = modlock_helper::store::snapshot_document(&doc, Some(id)).unwrap();
        assert_eq!(snapshot["profiles"].len(), 2000);
        assert_eq!(snapshot["selectedProfileId"], id);
        assert_eq!(snapshot["mods"].len(), 0);
    }
}

fn local_plan(f: &Fixture, revision: u64) -> modlock_helper::local_content::LocalImportPlan {
    use modlock_helper::{local_content::*, prepare_control::PrepareControl};
    let path = f.root.join("quoted-\"local.vpk");
    std::fs::write(&path, vpk("local.bin", 91)).unwrap();
    let p = prepare_local(
        LocalPrepareRequest {
            root: f.data.clone(),
            profile_id: "default".into(),
            expected_revision: revision,
            roots: vec![path],
            artwork: None,
        },
        &PrepareControl::new(),
    )
    .unwrap();
    review_local(
        &p,
        LocalSelection {
            candidate_ids: vec![p.candidates[0].id.clone()],
            metadata: p.default_metadata(),
        },
    )
    .unwrap()
}
#[test]
fn local_late_identity_journal_admission_rejects_before_publication() {
    use modlock_helper::{local_import::commit_local, native::NativeError};
    let f = Fixture::new();
    import(&mut NativeService::new(), &f);
    let p = local_plan(&f, 1);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let before = store.document().unwrap();
    let input = object! {padding:""};
    let mut initial = 0usize;
    let mut maximum = 0usize;
    assert!(
        commit_local(&p, &lock, 1, "limit-local", &input, &mut |at| {
            if at == "journal" {
                let mut j = store.journal("limit-local")?.unwrap();
                initial = j.dump().len();
                for key in ["stageDevice", "stageInode", "ownerDevice", "ownerInode"] {
                    j[key] = u64::MAX.into();
                }
                for file in j["files"].members_mut() {
                    file["device"] = u64::MAX.into();
                    file["inode"] = u64::MAX.into();
                }
                maximum = j.dump().len();
                return Err(NativeError::new("nativeIo", "stop before files"));
            }
            Ok(())
        })
        .is_err()
    );
    assert!(maximum > initial);
    let padding = 64 * 1024 * 1024 - initial - (maximum - initial) / 2;
    let escaped = "\u{1}".repeat(padding / 6) + &"x".repeat(padding % 6);
    let input = object! {padding:escaped};
    assert!(input.dump().len() < 64 * 1024 * 1024);
    assert!(initial + padding < 64 * 1024 * 1024);
    assert!(maximum + padding > 64 * 1024 * 1024);
    let error = commit_local(&p, &lock, 1, "limit-local", &input, &mut |_| {
        panic!("oversized journal passed admission")
    })
    .unwrap_err();
    assert_eq!(error.kind, "sourceTooLarge");
    assert_eq!(store.document().unwrap(), before);
    assert_eq!(store.journals().unwrap().len(), 0);
    assert_eq!(store.operation("limit-local").unwrap()["status"], "unknown");
    assert!(store.snapshot(Some("other")).is_ok());
}
#[test]
fn local_import_near_limit_all_profile_snapshot_rejection_is_atomic() {
    let f = Fixture::new();
    let mut service = NativeService::new();
    import(&mut service, &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut doc = store.document().unwrap();
    doc["sourcePath"] = "".into();
    pad_to_budget(&mut doc);
    store
        .commit(&lock, 1, "near-local-limit", &object! {}, doc, None)
        .unwrap();
    let before = store.document().unwrap();
    for (_, p) in before["profiles"].entries() {
        assert!(p.is_object())
    }
    let plan = local_plan(&f, 2);
    assert_eq!(
        modlock_helper::local_import::commit_local(
            &plan,
            &lock,
            2,
            "local-too-large",
            &object! {},
            &mut |_| panic!("snapshot limit passed publication")
        )
        .unwrap_err()
        .kind,
        "nativeResponseLimit"
    );
    assert_eq!(store.document().unwrap(), before);
    assert_eq!(store.journals().unwrap().len(), 0);
    assert_eq!(
        store.operation("local-too-large").unwrap()["status"],
        "unknown"
    );
    for id in ["default", "other"] {
        assert!(store.snapshot(Some(id)).unwrap().dump().len() < MAX_RESPONSE_BYTES);
    }
}
