use modlock_helper::snapshot::read_snapshot;
use json::{object, JsonValue};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Source(PathBuf);
impl Source {
    fn raw(bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "modlock-helper-snapshot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("state.json");
        fs::write(&path, bytes).unwrap();
        Self(path)
    }
    fn value(value: &JsonValue) -> Self {
        Self::raw(value.dump().as_bytes())
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}
fn fixture() -> JsonValue {
    json::parse(include_str!("../../tests/fixtures/upstream-state-v33.json")).unwrap()
}
fn warnings(value: &JsonValue, code: &str) -> bool {
    value["warnings"].members().any(|w| w["code"] == code)
}
fn error(path: &Path, profile: Option<&str>, code: i32, kind: &str) {
    let e = read_snapshot(path, profile).unwrap_err();
    assert_eq!(e.code, code);
    assert_eq!(e.kind, kind);
    assert!(!e.message.is_empty());
    assert!(!e.message.contains("SYNTHETIC_SECRET"));
}

#[test]
fn active_working_mods_override_stale_saved_snapshot_and_metadata_is_normalized() {
    let source = Source::value(&fixture());
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["protocolVersion"], 1);
    assert_eq!(v["storeVersion"], 33);
    assert_eq!(v["activeProfileId"], "default");
    assert_eq!(v["selectedProfileId"], "default");
    assert_eq!(
        v["sourcePath"].as_str().unwrap(),
        source.0.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(v["mods"].len(), 4);
    let m = &v["mods"][0];
    assert_eq!(m["id"], "mod_active");
    assert_eq!(m["remoteId"], "123");
    assert_eq!(m["title"], "Ivy — Täht 🌿\nSecond line");
    assert_eq!(m["author"], "Synthetic Author");
    assert_eq!(m["hero"], "Ivy");
    assert_eq!(m["category"], "Model Replacement");
    assert_eq!(m["installOrder"], 0);
    assert_eq!(m["installedVpks"][0], "pak01_dir.vpk");
    assert_eq!(m["missingVpks"][0], "lost.vpk");
    assert_eq!(v["profiles"][0]["modCount"], 4);
    assert_eq!(v["profiles"][0]["isActive"], true);
    assert!(!v.dump().contains("SYNTHETIC_SECRET_DO_NOT_EXPOSE"));
    assert!(!m.has_key("description"));
}

#[test]
fn inactive_profile_uses_own_mods_and_enabled_map_without_activating_it() {
    let source = Source::value(&fixture());
    let v = read_snapshot(&source.0, Some("other")).unwrap();
    assert_eq!(v["activeProfileId"], "default");
    assert_eq!(v["selectedProfileId"], "other");
    assert_eq!(v["mods"].len(), 1);
    assert_eq!(v["mods"][0]["id"], "inactive");
    assert_eq!(v["mods"][0]["hero"], "Wraith");
    assert_eq!(v["mods"][0]["enabled"], true);
    assert_eq!(v["mods"][0]["status"], "downloaded");
    assert!(warnings(&v, "enabledStatusMismatch"));
}

#[test]
fn string_local_config_and_status_fallback_preserve_unknown_and_transient_statuses() {
    let mut f = fixture();
    f["local-config"] = f["local-config"].dump().into();
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"][0]["enabled"], true);
    assert_eq!(v["mods"][1]["enabled"], false);
    assert_eq!(v["mods"][1]["title"], "mod_disabled");
    assert_eq!(v["mods"][1]["author"], "Unknown author");
    assert_eq!(v["mods"][1]["category"], "Uncategorized");
    assert_eq!(v["mods"][1]["hero"], "General");
    assert!(v["mods"][1]["installOrder"].is_null());
    assert_eq!(v["mods"][2]["status"], "installing");
    assert_eq!(v["mods"][2]["enabled"], false);
    assert_eq!(v["mods"][3]["status"], "future-status");
    assert_eq!(v["mods"][3]["enabled"], false);
    assert_eq!(v["mods"][3]["hero"], "Bebop");
    assert!(warnings(&v, "enabledStatusMismatch"));
}

#[test]
fn absent_enabled_entry_falls_back_to_installed_and_absent_local_mods_to_profile() {
    let mut f = fixture();
    let state = &mut f["local-config"]["state"];
    state.remove("localMods");
    state["profiles"]["default"]["mods"][0]["status"] = "installed".into();
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 1);
    assert_eq!(v["mods"][0]["id"], "stale");
    assert_eq!(v["mods"][0]["enabled"], true);
}

#[test]
fn malformed_mods_and_duplicate_ids_are_skipped_with_warnings() {
    let mut f = fixture();
    f["local-config"]["state"]["localMods"] = json::array![
        object! {id:"first",remoteId:"r",status:"installed"},
        object! {id:"first",remoteId:"duplicate",status:"installed"},
        object! {remoteId:"missing-id",status:"installed"},
        object! {id:"",remoteId:"empty-id",status:"installed"},
        object! {id:123,remoteId:"numeric-id",status:"installed"},
        object! {id:"missing-remote",status:"installed"},
        object! {id:"missing-status",remoteId:"r2"},
        JsonValue::Null,
        object! {id:"good",remoteId:"g",status:"downloaded"}
    ];
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 2);
    assert_eq!(v["mods"][1]["id"], "good");
    assert!(warnings(&v, "invalidMod"));
    assert!(warnings(&v, "duplicateModId"));
    assert_eq!(v["profiles"][0]["modCount"], 2);
}

#[test]
fn partial_optional_metadata_is_dropped_and_warned_without_invented_values() {
    let mut f = fixture();
    f["local-config"]["state"]["localMods"] = json::array![object! {
        id:"partial",remoteId:"p",status:"installed",installedVpks:json::array!["valid.vpk",42],
        missingVpks:"invalid",installOrder:"invalid",heroOverride:null,detectedHero:"",hero:"Mo & Krill"
    }];
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 1);
    assert_eq!(v["mods"][0]["hero"], "Mo & Krill");
    assert_eq!(v["mods"][0]["installedVpks"].len(), 1);
    assert_eq!(v["mods"][0]["missingVpks"].len(), 0);
    assert!(v["mods"][0]["installOrder"].is_null());
    assert!(!v["warnings"].is_empty());
}

#[test]
fn valid_empty_libraries_and_profileless_store_are_empty() {
    let mut f = fixture();
    f["local-config"]["state"]["localMods"] = json::array![];
    let source = Source::value(&f);
    assert_eq!(read_snapshot(&source.0, None).unwrap()["mods"].len(), 0);
    f["local-config"]["state"]["profiles"] = object! {};
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 0);
    assert_eq!(v["profiles"].len(), 0);
    assert!(v["selectedProfileId"].is_null());
}

#[test]
fn missing_active_id_falls_back_honestly_but_unknown_requested_profile_errors() {
    let mut f = fixture();
    f["local-config"]["state"]["activeProfileId"] = "gone".into();
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["activeProfileId"], "gone");
    assert_eq!(v["selectedProfileId"], "default");
    assert_eq!(v["mods"][0]["id"], "stale");
    assert!(warnings(&v, "activeProfileMissing"));
    error(&source.0, Some("missing"), -32013, "unknownProfile");
}

#[test]
fn missing_malformed_unsupported_and_nonregular_sources_return_useful_errors() {
    let source = Source::raw(b"{");
    error(&source.0, None, -32011, "invalidState");
    error(
        &source.0.parent().unwrap().join("missing"),
        None,
        -32010,
        "sourceNotFound",
    );
    error(source.0.parent().unwrap(), None, -32011, "invalidState");
    for bytes in [
        b"".as_slice(),
        b"{}",
        b"[]",
        b"{\"local-config\":\"SYNTHETIC_SECRET_DO_NOT_EXPOSE\"}",
        b"{\"local-config\":{\"version\":33,\"state\":{\"profiles\":[]}}}",
    ] {
        let source = Source::raw(bytes);
        error(&source.0, None, -32011, "invalidState");
    }
    let mut f = fixture();
    f["local-config"]["version"] = 34.into();
    let source = Source::value(&f);
    error(&source.0, None, -32012, "unsupportedVersion");
}

#[test]
fn oversized_source_is_rejected_and_refresh_reads_fresh_without_modification() {
    let source = Source::value(&fixture());
    let before = fs::read(&source.0).unwrap();
    let modified = fs::metadata(&source.0).unwrap().modified().unwrap();
    read_snapshot(&source.0, None).unwrap();
    assert_eq!(fs::read(&source.0).unwrap(), before);
    assert_eq!(
        fs::metadata(&source.0).unwrap().modified().unwrap(),
        modified
    );
    let mut f = fixture();
    f["local-config"]["state"]["localMods"] = json::array![];
    fs::write(&source.0, f.dump()).unwrap();
    assert_eq!(read_snapshot(&source.0, None).unwrap()["mods"].len(), 0);
    fs::OpenOptions::new()
        .write(true)
        .open(&source.0)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    error(&source.0, None, -32014, "sourceTooLarge");
}

#[test]
fn large_integral_install_order_is_preserved_without_overflow() {
    let mut f = fixture();
    f["local-config"]["state"]["localMods"][0]["installOrder"] =
        json::parse("-9223372036854775808").unwrap();
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert!(v["mods"][0]["installOrder"]
        .dump()
        .starts_with("-9223372036854775808"));
}

#[test]
fn repeated_malformed_entries_have_bounded_diagnostics() {
    let mut f = fixture();
    f["local-config"]["state"]["localMods"] = JsonValue::from(vec![JsonValue::Null; 10000]);
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 0);
    assert!(v["warnings"].len() <= 257);
    assert!(warnings(&v, "additionalWarningsOmitted"));
}

#[test]
fn preview_images_preserve_working_order_filter_unsafe_metadata_and_leave_source_untouched() {
    let mut f = fixture();
    let longest = format!("https://example.invalid/{}", "a".repeat(4072));
    assert_eq!(longest.len(), 4096);
    f["local-config"]["state"]["localMods"][0]["images"] = json::array![
        "https://example.invalid/first.png",
        "https://example.invalid/second.jpg?size=640",
        "https://example.invalid/first.png",
        "http://example.invalid/insecure.png",
        "data:image/png;base64,SYNTHETIC_SECRET",
        "file:///tmp/image.png",
        "javascript:bad()",
        "",
        "https://",
        "https://user:password@example.invalid/image.png",
        "https://example.invalid/control\n.png",
        42,
        null,
        longest.clone(),
        format!("{longest}a")
    ];
    f["local-config"]["state"]["profiles"]["default"]["mods"][0]["images"] =
        json::array!["https://example.invalid/stale.png"];
    let source = Source::value(&f);
    let before = fs::read(&source.0).unwrap();
    let modified = fs::metadata(&source.0).unwrap().modified().unwrap();
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(
        v["mods"][0]["images"],
        json::array![
            "https://example.invalid/first.png",
            "https://example.invalid/second.jpg?size=640",
            longest
        ]
    );
    assert_eq!(v["mods"][0]["previewHidden"], false);
    assert_eq!(v["mods"][1]["images"], json::array![]);
    assert!(!v.dump().contains("SYNTHETIC_SECRET"));
    assert_eq!(fs::read(&source.0).unwrap(), before);
    assert_eq!(
        fs::metadata(&source.0).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn preview_images_are_bounded_after_filtering_and_malformed_arrays_are_empty() {
    let mut f = fixture();
    let mut images = json::array![null, "http://example.invalid/no.png"];
    for i in 0..40 {
        let url = format!("https://example.invalid/{i}.png");
        images.push(url.clone()).unwrap();
        images.push(url).unwrap();
    }
    f["local-config"]["state"]["localMods"][0]["images"] = images;
    f["local-config"]["state"]["localMods"][1]["images"] =
        "https://example.invalid/not-an-array".into();
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, None).unwrap();
    assert_eq!(v["mods"].len(), 4);
    assert_eq!(v["mods"][0]["images"].len(), 32);
    assert_eq!(v["mods"][0]["images"][0], "https://example.invalid/0.png");
    assert_eq!(v["mods"][0]["images"][31], "https://example.invalid/31.png");
    assert_eq!(v["mods"][1]["images"], json::array![]);
}

#[test]
fn preview_masking_respects_source_preferences_without_exporting_them() {
    let cases = [
        (object! {}, JsonValue::Null, true),
        (object! {hideNSFW:true,disableBlur:true}, true.into(), true),
        (object! {disableBlur:true}, JsonValue::Null, false),
        (object! {}, true.into(), false),
        (object! {}, false.into(), true),
        (object! {rememberPerItemOverrides:false}, true.into(), true),
        (object! {rememberPerItemOverrides:true}, true.into(), false),
        (
            object! {disableBlur:"invalid",rememberPerItemOverrides:"invalid"},
            "invalid".into(),
            true,
        ),
    ];
    for (settings, visible_override, expected_hidden) in cases {
        let mut f = fixture();
        let state = &mut f["local-config"]["state"];
        state["localMods"][0]["isNSFW"] = true.into();
        state["localMods"][0]["images"] = json::array!["https://example.invalid/flagged.png"];
        state["localMods"][1]["isNSFW"] = false.into();
        state["nsfwSettings"] = settings;
        state["perItemNSFWOverrides"] =
            object! {"123":visible_override,"private":"SYNTHETIC_SECRET"};
        let source = Source::value(&f);
        let v = read_snapshot(&source.0, None).unwrap();
        assert_eq!(v["mods"][0]["previewHidden"], expected_hidden);
        assert_eq!(
            v["mods"][0]["images"][0],
            "https://example.invalid/flagged.png"
        );
        assert_eq!(v["mods"][1]["previewHidden"], false);
        assert!(!v.dump().contains("nsfwSettings"));
        assert!(!v.dump().contains("perItemNSFWOverrides"));
        assert!(!v.dump().contains("isNSFW"));
        assert!(!v.dump().contains("SYNTHETIC_SECRET"));
    }
}

#[test]
fn inactive_preview_uses_saved_images_and_global_masking_policy() {
    let mut f = fixture();
    let state = &mut f["local-config"]["state"];
    state["nsfwSettings"] = object! {hideNSFW:true};
    state["profiles"]["other"]["mods"][0]["isNSFW"] = true.into();
    state["profiles"]["other"]["mods"][0]["images"] =
        json::array!["https://example.invalid/inactive.png"];
    let source = Source::value(&f);
    let v = read_snapshot(&source.0, Some("other")).unwrap();
    assert_eq!(
        v["mods"][0]["images"][0],
        "https://example.invalid/inactive.png"
    );
    assert_eq!(v["mods"][0]["previewHidden"], true);
}
