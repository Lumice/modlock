use super::*;
#[test]
fn sections_roundtrip_bom_crlf() {
    let s = format!(
        "\u{feff}echo a\r\n{}\r\nx\r\n{}\r\necho b\r\n",
        MARKERS[0].0, MARKERS[0].1
    );
    let (free, sections) = split(&s).unwrap();
    assert_eq!(merge(&s, &free).unwrap(), s);
    assert_eq!(sections.len(), 1);
    assert!(!free.contains("DO NOT EDIT"));
    assert!(split(MARKERS[0].0).is_err());
}
#[test]
fn upstream_url_vector() {
    let c = defaults();
    let code = "0;4;16;1;false;false;1;1;255;255;255;Default";
    assert_eq!(encode(&c).unwrap(), code);
    assert_eq!(decode(code).unwrap(), c);
    assert!(decode("NaN;4;16;1;false;false;1;1;255;255;255;Default").is_err());
}
fn fixture() -> crate::test_support::Fixture {
    let f = crate::test_support::Fixture::new();
    let mut svc = crate::native::NativeService::new();
    crate::test_support::import(&mut svc, &f);
    fs::create_dir_all(f.game.join("game/citadel/cfg")).unwrap();
    f
}
#[test]
fn missing_commit_undo_and_recovery() {
    let f = fixture();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let (r, dto) = review(&store, "autoexec", &"echo test\r\n".into(), false).unwrap();
    assert!(dto["before"].is_null());
    commit(&store, &lock, &r).unwrap();
    assert_eq!(view(&store, "autoexec").unwrap()["text"], "echo test\r\n");
    let (u, _) = review(&store, "autoexec", &JsonValue::Null, true).unwrap();
    commit(&store, &lock, &u).unwrap();
    assert!(!target(&f.game, "autoexec").unwrap().exists());
    let (r, _) = review(&store, "autoexec", &"recover".into(), false).unwrap();
    store.write_journal(&lock, &r.token, &r.body).unwrap();
    crate::font_transaction::admit_set(&store, &store.journals().unwrap()).unwrap();
    recover(&store, &lock).unwrap();
    assert_eq!(
        read(&target(&f.game, "autoexec").unwrap())
            .unwrap()
            .unwrap(),
        "recover"
    );
    assert!(store.journals().unwrap().is_empty());
}
#[test]
fn crosshair_preserves_heroes_and_undoes_both_files() {
    let f = fixture();
    let p = target(&f.game, "crosshair").unwrap();
    let original = hero::EMPTY_SETTINGS
        .replace(
            "heroes = {}",
            "heroes = { hero_ivy = { custom = \"keep\" } }",
        )
        .replace('\n', "\r\n");
    fs::write(&p, &original).unwrap();
    let ap = target(&f.game, "autoexec").unwrap();
    let a = format!(
        "\u{feff}echo keep\r\n{}\r\nlegacy\r\n{}\r\n",
        MARKERS[0].0, MARKERS[0].1
    );
    fs::write(&ap, &a).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut v = defaults();
    v["color"]["r"] = 22.into();
    let (r, dto) = review(&store, "crosshair", &v, false).unwrap();
    assert_eq!(dto["additionalDiff"]["before"], a.as_str());
    commit(&store, &lock, &r).unwrap();
    assert_eq!(view(&store, "crosshair").unwrap()["values"], v);
    assert!(fs::read_to_string(&p).unwrap().contains("hero_ivy"));
    assert!(!fs::read_to_string(&ap).unwrap().contains("DO NOT EDIT"));
    let (u, _) = review(&store, "crosshair", &JsonValue::Null, true).unwrap();
    commit(&store, &lock, &u).unwrap();
    assert_eq!(fs::read(&p).unwrap(), original.as_bytes());
    assert_eq!(fs::read(&ap).unwrap(), a.as_bytes());
}
#[test]
fn confinement_and_stale_reviews() {
    use std::os::unix::fs::symlink;
    let f = fixture();
    let outside = f.root.join("outside");
    fs::write(&outside, "sentinel").unwrap();
    let p = target(&f.game, "autoexec").unwrap();
    symlink(&outside, &p).unwrap();
    assert!(read(&p).is_err());
    fs::remove_file(&p).unwrap();
    assert!(target(&f.game, "../../outside").is_err());
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let (r, _) = review(&store, "autoexec", &"new".into(), false).unwrap();
    fs::write(&p, "external").unwrap();
    assert!(commit(&store, &lock, &r).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "sentinel");
    assert!(store.journals().unwrap().is_empty());
}
#[test]
fn running_game_refusal_uses_shared_scan() {
    let f = fixture();
    let proc = f.root.join("proc");
    fs::create_dir(&proc).unwrap();
    fs::create_dir(proc.join("123")).unwrap();
    fs::write(proc.join("123/cmdline"), b"/game/bin/win64/deadlock.exe\0").unwrap();
    fs::write(
        proc.join("123/stat"),
        format!("123 (deadlock) S {} 42", vec!["0"; 18].join(" ")),
    )
    .unwrap();
    fs::write(
        proc.join("123/status"),
        "Tgid:\t123\nUid:\t1000\t1000\t1000\t1000\n",
    )
    .unwrap();
    assert_eq!(
        crate::game_access::game_guard_at(&proc).unwrap_err().kind,
        "gameRunning"
    );
}
#[test]
fn invalid_ranges_and_managed_injection() {
    let mut c = defaults();
    c["width"] = 100.1.into();
    assert!(validate(&c).is_err());
    assert!(merge("", MARKERS[1].0).is_err());
    assert!(split(&format!(
        "{}{}{}{}",
        MARKERS[0].0, MARKERS[1].0, MARKERS[0].1, MARKERS[1].1
    ))
    .is_err());
}

#[test]
fn nondefault_url_and_partial_crosshair_recovery() {
    let code = "5;0;8.3;1;false;true;0;0;22;232;19;Mo%20%26%20Krill";
    let v = decode(code).unwrap();
    assert_eq!(encode(&v).unwrap(), code);
    let f = fixture();
    let ap = target(&f.game, "autoexec").unwrap();
    let a = format!("echo keep\n{}\nlegacy\n{}", MARKERS[0].0, MARKERS[0].1);
    fs::write(&ap, &a).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let (r, _) = review(&store, "crosshair", &v, false).unwrap();
    store.write_journal(&lock, &r.token, &r.body).unwrap();
    let p = target(&f.game, "crosshair").unwrap();
    atomic(&p, r.body["after"].as_str(), 0o644).unwrap();
    recover(&store, &lock).unwrap();
    assert!(!fs::read_to_string(ap).unwrap().contains("DO NOT EDIT"));
    assert_eq!(
        view(&store, "crosshair").unwrap()["values"]["color"],
        v["color"]
    );
}

#[test]
fn service_api_review_commit_and_undo() {
    let f = fixture();
    let mut svc = crate::native::NativeService::new();
    let mut p = f.params();
    p["kind"] = "autoexec".into();
    let view = svc.dispatch("native.config.get", &p).unwrap();
    assert_eq!(view["text"], "");
    p["input"] = "echo through service".into();
    let review = svc.dispatch("native.config.preview", &p).unwrap();
    assert_eq!(review["diff"]["after"], "echo through service");
    p["token"] = review["token"].clone();
    let saved = svc.dispatch("native.config.commit", &p).unwrap();
    assert_eq!(saved["canUndo"], true);
    p["undo"] = true.into();
    let review = svc.dispatch("native.config.preview", &p).unwrap();
    p["token"] = review["token"].clone();
    let undone = svc.dispatch("native.config.commit", &p).unwrap();
    assert_eq!(undone["text"], "");
    assert_eq!(undone["canUndo"], false);
}

#[test]
fn expanded_crosshair_review_roundtrip() {
    let f = fixture();
    let p = target(&f.game, "crosshair").unwrap();
    let original = hero::EMPTY_SETTINGS
        .replace(
            "base = {}",
            "base = { crosshair_dot_size = \"7\" crosshair_themed = \"true\" unknown = \"keep\" }",
        )
        .replace(
            "heroes = {}",
            "heroes = { hero_inferno = { crosshair_dot_size = \"5\" custom = \"untouched\" } }",
        );
    fs::write(&p, &original).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut v = view(&store, "crosshair").unwrap()["values"].clone();
    assert_eq!(v["dotSize"], 7);
    assert_eq!(v["themed"], true);
    assert_eq!(v["overrides"]["hero_inferno"]["dotSize"], 5);
    v["dotOutlineWidth"] = 2.into();
    v["pipOutlineGap"] = 3.into();
    v["pipOutlineOpacity"] = 0.37.into();
    v["outlineColor"]["r"] = 123.into();
    v["gapCollapse"] = true.into();
    v["overrides"]["hero_inferno"]["dotSize"] = 9.into();
    v["overrides"]["hero_doorman"] = object! {themed:true,dotSize:7};
    let (r, _) = review(&store, "crosshair", &v, false).unwrap();
    commit(&store, &lock, &r).unwrap();
    assert_eq!(view(&store, "crosshair").unwrap()["values"], v);
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.contains("unknown = \"keep\""));
    assert!(text.contains("custom = \"untouched\""));
    let (r, _) = review(&store, "crosshair", &JsonValue::Null, true).unwrap();
    commit(&store, &lock, &r).unwrap();
    assert_eq!(fs::read_to_string(p).unwrap(), original);
}

#[test]
fn expanded_crosshair_all_keys_and_inheritance() {
    let source = hero::EMPTY_SETTINGS.replace(
        "heroes = {}",
        "heroes = { hero_magician = { crosshair_future_setting = \"preserve\" } }",
    );
    let mut v = defaults();
    v["dotSize"] = 7.into();
    v["dotOutlineWidth"] = 2.into();
    v["dotOutlineGap"] = 3.into();
    v["dotOutlineOpacity"] = 0.33.into();
    v["pipOutlineWidth"] = 4.into();
    v["pipOutlineGap"] = 5.into();
    v["pipOutlineOpacity"] = 0.67.into();
    v["themed"] = true.into();
    v["gapCollapse"] = true.into();
    v["outlineColor"] = object! {r:12,g:34,b:56};
    v["overrides"]["hero_magician"] = object! {dotSize:5,outlineColor:object!{g:77}};
    let updated = update_values(&source, &v).unwrap();
    assert_eq!(read_values(&updated).unwrap(), v);
    for key in [
        "crosshair_dot_size",
        "crosshair_dot_outline_border",
        "crosshair_dot_outline_gap",
        "crosshair_pip_outline_border",
        "crosshair_pip_outline_gap",
        "crosshair_pip_outline_opacity",
        "crosshair_outline_color_r",
        "crosshair_outline_color_g",
        "crosshair_outline_color_b",
        "crosshair_pip_gap_collapse",
        "crosshair_themed",
    ] {
        assert!(updated.contains(key), "missing {key}");
    }
    v["overrides"] = object! {};
    let inherited = update_values(&updated, &v).unwrap();
    assert_eq!(read_values(&inherited).unwrap(), v);
    assert!(inherited.contains("crosshair_future_setting = \"preserve\""));
}

#[test]
fn review_102_picker_uses_scopes_and_keeps_unknown_heroes() {
    let f = fixture();
    let p = target(&f.game, "crosshair").unwrap();
    fs::write(&p, hero::EMPTY_SETTINGS.replace("heroes = {}", "heroes = { hero_atlas = { crosshair_dot_size = \"8\" } hero_review_unknown = { custom = \"keep\" } }")).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let dto = view(&store, "crosshair").unwrap();
    let scopes = dto["heroScopes"].members().collect::<Vec<_>>();
    assert!(scopes
        .iter()
        .any(|row| row["key"] == "hero_atlas" && row["name"] == "Abrams"));
    assert!(scopes
        .iter()
        .any(|row| row["key"] == "hero_review_unknown" && row["name"] == "hero_review_unknown"));
    let mut keys = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for row in scopes {
        assert!(keys.insert(row["key"].dump()));
        assert!(names.insert(row["name"].dump()));
    }
    let bindings = json::parse(include_str!(
        "../../vendor/health-engine/assets/hero-model-bindings.json"
    ))
    .unwrap();
    for (key, _) in bindings["bindings"].entries() {
        assert!(keys.contains(&JsonValue::from(key).dump()), "missing {key}");
    }
}

#[test]
fn review_102_base_only_gains_hero_with_journal_and_undo() {
    let f = fixture();
    let p = target(&f.game, "crosshair").unwrap();
    let original = hero::EMPTY_SETTINGS
        .replace("\theroes = {}\n", "")
        .replace("base = {}", "base = { custom = \"keep\" }")
        .replace('\n', "\r\n");
    fs::write(&p, &original).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let store = Store::open_writer(&lock).unwrap();
    let mut v = view(&store, "crosshair").unwrap()["values"].clone();
    v["overrides"]["hero_atlas"] = object! {dotSize:8};
    let (r, _) = review(&store, "crosshair", &v, false).unwrap();
    commit(&store, &lock, &r).unwrap();
    assert_eq!(view(&store, "crosshair").unwrap()["values"], v);
    assert!(fs::read_to_string(&p)
        .unwrap()
        .contains("custom = \"keep\""));
    let (r, _) = review(&store, "crosshair", &JsonValue::Null, true).unwrap();
    commit(&store, &lock, &r).unwrap();
    assert_eq!(fs::read_to_string(p).unwrap(), original);
}

#[test]
fn review_102_legacy_export_uses_numeric_outline() {
    let mut v = defaults();
    v["pipOutlineWidth"] = 2.into();
    let code = encode(&v).unwrap();
    assert_eq!(code.split(';').nth(4), Some("true"));
    let decoded = decode(&code).unwrap();
    assert_eq!(decoded["pipOutlineWidth"], 1);
    assert_eq!(encode(&decoded).unwrap(), code);
}
