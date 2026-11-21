use super::*;
#[test]
fn catalog_and_stock_roundtrip() {
    let c = catalog().unwrap();
    assert_eq!(c["presets"].len(), 8);
    let stock = include_str!("../../data/perf-stock.gi");
    for p in c["presets"].members() {
        let patched = overlay(stock, p["id"].as_str().unwrap()).unwrap();
        assert_eq!(patch::strip_overlay(&patched).0, stock);
        assert_eq!(detect(&patched).unwrap()["presetId"], p["id"]);
    }
}
#[test]
fn deny_and_guard_rules() {
    let c = catalog().unwrap();
    for p in [
        vec!["ConVars", "sv_cheats"],
        vec!["ConVars", "host_timescale"],
        vec!["SceneSystem", "foo"],
        vec!["FileSystem", "foo"],
    ] {
        assert!(!writable(
            &c,
            &p.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        ));
    }
}
#[test]
fn foreign_detection() {
    let s = include_str!("../../data/perf-stock.gi").replace(
        "ConVars",
        "// ===== deadtune managed convars\n// ===== end deadtune managed convars\nConVars",
    );
    assert_eq!(detect(&s).unwrap()["state"], "foreign");
    assert!(overlay(&s, "dmm-clean").is_err());
}

#[test]
fn journal_identity_signed_minimum_roundtrips() {
    let v = JsonValue::from(i64::MIN);
    assert_eq!(v.as_i64(), Some(i64::MIN));
    assert_eq!(JsonValue::from(u64::MAX).as_i64(), None);
}

#[test]
fn mixed_line_endings_strip_exactly() {
    let stock = include_str!("../../data/perf-stock.gi");
    for text in [
        stock.replace("\n", "\r\n"),
        stock.replacen("\n", "\r\n", 7).trim_end().to_string(),
    ] {
        let patched = overlay(&text, "dmm-clean").unwrap();
        assert_eq!(patch::strip_overlay(&patched).0, text);
    }
}

#[test]
fn item45_no_selection_preserves_tagged_engine_values() {
    let before = include_str!("../../data/perf-stock.gi").replace("ConVars", "EngineFixture\n{\n IndexBufferPoolSizeMB 32 // dmm-perf was 16\n}\nConVars");
    assert_eq!(overlay(&before, "").unwrap(), before);
    assert_eq!(overlay(&before, "none").unwrap(), before);
    let review = review(&before, "dmm-clean").unwrap();
    assert!(review["changes"].members().any(|c| c["name"] == "IndexBufferPoolSizeMB" && c["from"] == "32" && c["to"] == "16"));
}

#[test]
fn unify_none_stock_catalog_roundtrips_exactly_with_outside_tags() {
    let stock=include_str!("../../data/perf-stock.gi");
    for base in [stock.to_owned(),stock.replace('\n',"\r\n")] {
        for preset in catalog().unwrap()["presets"].members() {
            let applied=overlay(&base,preset["id"].as_str().unwrap()).unwrap();
            assert!(applied.split(patch::END_MARKER).nth(1).unwrap().contains("dmm-perf"));
            assert_eq!(overlay(&applied,"none").unwrap(),base);
            assert_eq!(overlay(&applied,"").unwrap(),applied);
        }
    }
}
#[test]
fn unify_none_marker_on_setting_line_is_preserved_or_refused() {
    let base="GameInfo\n{\n ConVars\n {\n }\n}\n";
    let applied=overlay(base,"dmm-clean").unwrap();
    let malformed=applied.replace(patch::BEGIN_MARKER,&format!("user_custom KEEP_ME // dmm-perf was BEFORE {}",patch::BEGIN_MARKER));
    for id in ["none","dmm-clean"] {
        if let Ok(result)=overlay(&malformed,id) { assert!(result.contains("user_custom KEEP_ME"),"{id} removed user setting"); }
    }
}
#[test]
fn unify_none_foreign_tag_refuses() {
    let stock=include_str!("../../data/perf-stock.gi");
    let foreign=format!("{stock}\ngpu_level \"7\" // dmm-perf was \"2\" // grimoire-perf was \"1\"\n");
    assert!(!patch::detect_foreign(&foreign).is_empty());
    assert!(overlay(&foreign,"none").is_err());
}
#[test]
fn unify_none_obeys_shared_stripped_verification() {
    let base="GameInfo\n{\n ConVars\n {\n }\n}\n";
    let applied=overlay(base,"dmm-clean").unwrap();
    let edited=applied.replace(patch::END_MARKER,&format!("{{\n{}",patch::END_MARKER));
    let stripped=patch::strip_overlay(&edited).0;
    assert!(patch::verify_stripped(&edited,&stripped).is_err());
    assert!(overlay(&edited,"none").is_err());
    assert!(overlay(&edited,"eskay").is_err());
}

#[test]
fn roundtrip_none_refuses_in_block_edits_only() {
    let applied=overlay(include_str!("../../data/perf-stock.gi"),"dmm-clean").unwrap();
    let edited=applied.replace(patch::END_MARKER,&format!("user_custom KEEP_ME\n// personal comment\n{}",patch::END_MARKER));
    let result=overlay(&edited,"none").unwrap_err();
    assert_eq!(result.message,"Performance settings were edited outside the app. They have been left unchanged.");
}
#[test]
fn roundtrip_none_refuses_reviewer_shared_line_scenario() {
    let applied=overlay(include_str!("../../data/perf-stock.gi"),"dmm-clean").unwrap();
    let edited=applied.replace(patch::END_MARKER,&format!("user_custom KEEP_ME\n// personal comment\n{}\ncl_simulate_dormant_entities 1 user_other 2",patch::END_MARKER));
    for preset in catalog().unwrap()["presets"].members() { assert!(overlay(&edited,preset["id"].as_str().unwrap()).is_err()); }
    let result=overlay(&edited,"none").unwrap_err();
    assert_eq!(result.message,"Performance settings were edited outside the app. They have been left unchanged.");
}

#[test]
fn view_distinguishes_explicit_none_from_inherited_selection() {
    let inherited = view(&json::object! {gamePath:""}).unwrap();
    let explicit = view(&json::object! {gamePath:"", performancePreset:"none"}).unwrap();
    assert_eq!(inherited["selection"], "");
    assert_eq!(explicit["selection"], "");
    assert_eq!(inherited["explicitSelection"], false);
    assert_eq!(explicit["explicitSelection"], true);
}

#[test]
fn preset_review_keeps_original_explanations_without_dumped_help() {
    let c = catalog().unwrap();
    assert!(c["convars"].members().all(|row| !row.has_key("help") && !row.has_key("enumValues")));
    let data = view(&json::object! {gamePath:""}).unwrap();
    let entry = data["presets"]
        .members()
        .flat_map(|preset| preset["entries"].members())
        .find(|entry| entry["key"] == "ConVars/fps_max_tools")
        .expect("preset tools frame-rate limit");
    assert!(entry["description"].as_str().unwrap().contains("Use 0 to disable the extra cap."));
}

#[test]
fn none_over_older_revision_overlay_returns_performance_outdated() {
    let applied = overlay(include_str!("../../data/perf-stock.gi"), "dmm-clean").unwrap();
    let older = applied.replace(&format!("rev={CATALOG_REV}"), "rev=92bebec");
    assert_ne!(older, applied);
    let err = overlay(&older, "none").unwrap_err();
    assert_eq!(err.kind, "performanceOutdated");
    assert_eq!(
        err.message,
        "Your game has a different version of these performance settings. To remove them, close Deadlock, choose this preset again and apply it, then choose None and apply again."
    );
}
