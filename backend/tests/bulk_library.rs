mod support;
use modlock_helper::{
    native::NativeService,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use support::*;

fn setup() -> (Fixture, NativeService) {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut d = store.document().unwrap();
    let mut b = d["profiles"]["default"]["mods"][0].clone();
    b["source"]["id"] = "second".into();
    d["profiles"]["other"]["mods"] = json::array![d["profiles"]["default"]["mods"][0].clone(), b];
    store
        .commit(&lock, 1, "seed", &object! {seed:true}, d, Some("other"))
        .unwrap();
    (f, s)
}
fn request(f: &Fixture, id: &str) -> JsonValue {
    let mut p = mutation(f, "2", id);
    p["profileId"] = "other".into();
    p["modIds"] = json::array!["local-a", "second"];
    p
}
#[test]
fn batch_revision_replay_and_other_profiles() {
    let (f, mut s) = setup();
    let mut p = request(&f, "batch");
    p["enabled"] = false.into();
    let before = s.dispatch("native.snapshot", &f.params()).unwrap();
    let result = s.dispatch("native.setModsEnabled", &p).unwrap();
    assert_eq!(result["revision"], "3");
    assert_eq!(result["bulkResult"]["changed"], 2);
    assert!(result["mods"].members().all(|m| m["enabled"] == false));
    assert_eq!(
        NativeService::new()
            .dispatch("native.setModsEnabled", &p)
            .unwrap(),
        result
    );
    let mut op = f.params();
    op["operationId"] = "batch".into();
    assert_eq!(
        s.dispatch("native.operation", &op).unwrap()["result"],
        result
    );
    p["enabled"] = true.into();
    assert_eq!(
        s.dispatch("native.setModsEnabled", &p).unwrap_err().kind,
        "operationIdConflict"
    );
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap()["mods"],
        before["mods"]
    );
}
#[test]
fn batch_bounds_and_validation() {
    let (f, mut s) = setup();
    for ids in [
        json::array![],
        json::array!["../bad"],
        json::array!["local-a", "local-a"],
        json::array![3],
        JsonValue::from(vec!["a"; 257]),
    ] {
        let mut p = request(&f, "invalid");
        p["enabled"] = false.into();
        p["modIds"] = ids;
        assert_eq!(
            s.dispatch("native.setModsEnabled", &p).unwrap_err().kind,
            "invalidParams"
        );
    }
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap()["revision"],
        "2"
    );
}
#[test]
fn removal_including_default_profile() {
    let (f, mut s) = setup();
    let p = request(&f, "remove");
    let v = s.dispatch("native.removeModsFromProfile", &p).unwrap();
    assert_eq!(v["bulkResult"]["changed"], 2);
    assert_eq!(v["mods"].len(), 0);
    assert_eq!(v["revision"], "3");
    assert_eq!(
        NativeService::new()
            .dispatch("native.removeModsFromProfile", &p)
            .unwrap(),
        v
    );
    let mut p = mutation(&f, "3", "protected");
    p["profileId"] = "default".into();
    p["modIds"] = json::array!["local-a"];
    let v = s.dispatch("native.removeModsFromProfile", &p).unwrap();
    assert_eq!(v["bulkResult"]["changed"], 1);
    assert!(v["bulkResult"]["skipped"].is_empty());
    assert_eq!(v["mods"].len(), 0);
}

#[test]
fn partial_selection_skips_and_noop_counts() {
    let (f, mut s) = setup();
    {
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let mut d = store.document().unwrap();
        d["packageVersion"] = d["storeVersion"].clone();
        d["storeVersion"] = 4.into();
        d["fonts"] = object! {version:1,installed:JsonValue::Null,previous:JsonValue::Null};
        let target = &mut d["profiles"]["other"]["mods"][1];
        target.remove("payloads");
        target.remove("packages");
        target["options"] = object! {version:1,entries:json::array![],selectedIds:json::array![],retainedArchives:json::array![],attachments:json::array![]};
        d["profiles"]["other"]["mods"][0]["enabled"] = false.into();
        d["profiles"]["other"]["mods"][1]["enabled"] = false.into();
        store
            .commit(&lock, 2, "choices", &object! {seed:2}, d, Some("other"))
            .unwrap();
    }
    let mut p = request(&f, "partial");
    p["expectedRevision"] = "3".into();
    p["enabled"] = true.into();
    p["modIds"].push("absent").unwrap();
    let v = s.dispatch("native.setModsEnabled", &p).unwrap();
    assert_eq!(v["revision"], "4");
    assert_eq!(v["bulkResult"]["changed"], 1);
    assert_eq!(
        v["bulkResult"]["skipped"],
        json::array![
            object! {id:"second",reason:"invalidLocalSelection"},
            object! {id:"absent",reason:"unknownMod"}
        ]
    );
    p["operationId"] = "noop".into();
    p["expectedRevision"] = "4".into();
    assert_eq!(
        s.dispatch("native.setModsEnabled", &p).unwrap()["bulkResult"]["changed"],
        0
    );
}
#[test]
fn removal_from_applied_and_restorable_profiles_keeps_checkpoints() {
    for restorable in [false, true] {
        let f = Fixture::new();
        let mut s = NativeService::new();
        std::fs::write(f.game.join("game/citadel/gameinfo.gi"), "\"GameInfo\"\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n Game core\n }\n }\n}\n").unwrap();
        import(&mut s, &f);
        let mut revision = "1".to_owned();
        for (i, profile) in ["other", "default"].iter().take(if restorable { 2 } else { 1 }).enumerate() {
            let mut p = f.params();p["profileId"]=(*profile).into();p["gamePath"]=f.game.to_str().unwrap().into();
            let review=s.dispatch("native.applyPreview", &support::current_layout(&p)).unwrap();
            let mut p=mutation(&f,&revision,&format!("apply-{i}"));p["token"]=review["token"].clone();
            revision=s.dispatch("native.apply", &support::current_layout(&p)).unwrap()["revision"].as_str().unwrap().to_owned();
        }
        let before=Store::open(&f.data).unwrap().document().unwrap();
        let mut p=mutation(&f,&revision,"remove");p["profileId"]="other".into();p["modIds"]=json::array!["local-b"];
        let result=s.dispatch("native.removeModsFromProfile",&p).unwrap();
        assert_eq!(result["bulkResult"]["changed"],1);
        assert!(result["mods"].is_empty());
        let after=Store::open(&f.data).unwrap().document().unwrap();
        assert_eq!(after["applied"],before["applied"]);
        assert_eq!(after["previous"],before["previous"]);
    }
}

#[test]
fn maximum_batch_and_stale_revision() {
    let (f, mut s) = setup();
    let mut p = request(&f, "maximum");
    p["enabled"] = false.into();
    for i in 0..254 {
        p["modIds"].push(format!("absent-{i}")).unwrap();
    }
    let v = s.dispatch("native.setModsEnabled", &p).unwrap();
    assert_eq!(v["bulkResult"]["changed"], 2);
    assert_eq!(v["bulkResult"]["skipped"].len(), 254);
    p["operationId"] = "stale-batch".into();
    assert_eq!(
        s.dispatch("native.setModsEnabled", &p).unwrap_err().kind,
        "staleRevision"
    );
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap()["revision"],
        "3"
    );
}
