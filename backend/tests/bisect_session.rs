mod support;
use modlock_helper::{
    native::NativeService,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use support::*;
fn doc(f: &Fixture) -> JsonValue {
    Store::open(&f.data).unwrap().document().unwrap()
}
fn request(f: &Fixture, id: &str) -> JsonValue {
    mutation(f, doc(f)["revision"].as_str().unwrap(), id)
}
fn seed_options_and_order(f: &Fixture, s: &mut NativeService) {
    let mut p = f.params();
    p["profileId"] = "default".into();
    p["modId"] = "local-a".into();
    p["expectedRevision"] = doc(f)["revision"].clone();
    let detail = s.dispatch("native.options.get", &p).unwrap();
    p.remove("modId");
    p["change"] = object! {kind:"selection",modId:"local-a",optionIds:detail["detail"]["catalog"]["selectedIds"].clone()};
    let review = s.dispatch("native.profileEdit.preview", &p).unwrap();
    let mut p = request(f, "save-options");
    p["profileId"] = "default".into();
    p["token"] = review["token"].clone();
    s.dispatch("native.profileEdit.commit", &p).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut d = store.document().unwrap();
    let revision = d["revision"].as_str().unwrap().parse().unwrap();
    let mut second = d["profiles"]["other"]["mods"][0].clone();
    second["source"]["installOrder"] = (-9).into();
    let first = d["profiles"]["default"]["mods"][0].clone();
    assert!(first["options"]["selectedIds"].len() > 0);
    d["profiles"]["default"]["mods"] = json::array![first, second];
    store
        .commit(
            &lock,
            revision,
            "seed-order",
            &object! {seed:true},
            d,
            Some("default"),
        )
        .unwrap();
}
fn start(f: &Fixture, s: &mut NativeService) {
    let mut p = request(f, "start-bisect");
    p["profileId"] = "default".into();
    p["modIds"] = json::array!["local-a"];
    s.dispatch("native.bisect.start", &p).unwrap();
}
#[test]
fn persists_replays_restores_exact_profile() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    seed_options_and_order(&f, &mut s);
    let original = doc(&f)["profiles"].clone();
    start(&f, &mut s);
    drop(s);
    let mut s = NativeService::new();
    let state = s.dispatch("native.bisect.get", &f.params()).unwrap();
    assert_eq!(state["active"], true);
    assert_eq!(state["round"], 1);
    assert_eq!(state["testedMods"].len(), 0);
    assert_eq!(doc(&f)["profiles"]["default"]["mods"][0]["enabled"], false);
    let mut p = request(&f, "answer-bisect");
    p["answer"] = "fine".into();
    let result = s.dispatch("native.bisect.answer", &p).unwrap();
    assert_eq!(result["bisect"]["done"], false);
    assert_eq!(s.dispatch("native.bisect.answer", &p).unwrap(), result);
    let mut p = request(&f, "answer-two");
    p["answer"] = "broken".into();
    let result = s.dispatch("native.bisect.answer", &p).unwrap();
    assert_eq!(result["bisect"]["done"], true);
    assert_eq!(doc(&f)["profiles"], original);
    let mut p = request(&f, "finish-bisect");
    p["keepDisabled"] = true.into();
    s.dispatch("native.bisect.finish", &p).unwrap();
    let mut want = original;
    want["default"]["mods"][0]["enabled"] = false.into();
    assert_eq!(doc(&f)["profiles"], want);
}
#[test]
fn cancel_after_profile_edit_restores_original() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    import(&mut s, &f);
    seed_options_and_order(&f, &mut s);
    let original = doc(&f)["profiles"].clone();
    start(&f, &mut s);
    {
        let lock = RootLock::acquire(&f.data).unwrap();
        let store = Store::open_writer(&lock).unwrap();
        let mut d = store.document().unwrap();
        let revision = d["revision"].as_str().unwrap().parse().unwrap();
        d["profiles"]["default"]["name"] = "Edited".into();
        store
            .commit(
                &lock,
                revision,
                "edit",
                &object! {edit:true},
                d,
                Some("default"),
            )
            .unwrap();
    }
    assert_eq!(
        s.dispatch("native.bisect.get", &f.params()).unwrap()["profileChanged"],
        true
    );
    let mut p = request(&f, "refuse-answer");
    p["answer"] = "unsure".into();
    assert_eq!(
        s.dispatch("native.bisect.answer", &p).unwrap_err().kind,
        "bisectProfileChanged"
    );
    let mut p = request(&f, "cancel-bisect");
    p["keepDisabled"] = false.into();
    s.dispatch("native.bisect.finish", &p).unwrap();
    assert_eq!(doc(&f)["profiles"], original);
}

#[test]
fn finish_done_preserves_later_profile_edits() {
 for keep in [false,true] {
    let f=Fixture::new(); let mut s=NativeService::new(); import(&mut s,&f);
    seed_options_and_order(&f,&mut s); start(&f,&mut s);
    for (id,answer) in [("fine","fine"),("broken","broken")] {
      let mut p=request(&f,id); p["answer"]=answer.into(); s.dispatch("native.bisect.answer",&p).unwrap();
    }
    assert_eq!(doc(&f)["bisect"]["done"],true);
    {let lock=RootLock::acquire(&f.data).unwrap(); let st=Store::open_writer(&lock).unwrap();let mut d=st.document().unwrap();let rev=d["revision"].as_str().unwrap().parse().unwrap();d["profiles"]["default"]["name"]="Later edit".into();st.commit(&lock,rev,"later-edit",&object!{},d,None).unwrap();}
    assert_eq!(s.dispatch("native.bisect.get",&f.params()).unwrap()["profileChanged"],true);
    let mut p=request(&f,"finish");p["keepDisabled"]=keep.into();s.dispatch("native.bisect.finish",&p).unwrap();
    assert_eq!(doc(&f)["profiles"]["default"]["name"],"Later edit");
 }
}
#[test]
fn repair_choices_follow_install_order() {
 let f=Fixture::new();let mut s=NativeService::new();import(&mut s,&f);seed_options_and_order(&f,&mut s);
 let mut d=doc(&f);d["profiles"]["default"]["mods"][1]["enabled"]=true.into();let intent=modlock_helper::deployment_intent::DeploymentIntent::capture(&d,"default").unwrap();
 let choices=modlock_helper::repairs::choices(&d,"default").unwrap();
 assert_eq!(choices["mods"].members().map(|m|m["modId"].as_str().unwrap()).collect::<Vec<_>>(),intent.mods.iter().map(|m|m.mod_id.as_str()).collect::<Vec<_>>());
}

#[test]
fn finish_after_culprit_or_profile_removed() {
 for profile_removed in [false,true] {
    let f=Fixture::new();let mut s=NativeService::new();import(&mut s,&f);seed_options_and_order(&f,&mut s);start(&f,&mut s);
    for (id,answer) in [("fine","fine"),("broken","broken")] {let mut p=request(&f,id);p["answer"]=answer.into();s.dispatch("native.bisect.answer",&p).unwrap();}
    {let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let mut d=st.document().unwrap();let rev=d["revision"].as_str().unwrap().parse().unwrap();
    if profile_removed {d["profiles"].remove("default");d["activeProfileId"]="other".into();}else{d["profiles"]["default"]["mods"]=json::array![];}
    st.commit(&lock,rev,"remove",&object!{},d,None).unwrap();}
    assert!(!s.dispatch("native.bisect.get",&f.params()).unwrap()["finishNotice"].as_str().unwrap().is_empty());
    let mut p=request(&f,"finish");p["keepDisabled"]=true.into();s.dispatch("native.bisect.finish",&p).unwrap();assert!(doc(&f)["bisect"].is_null());
    if profile_removed {assert!(doc(&f)["profiles"]["default"].is_null());}
 }
}
