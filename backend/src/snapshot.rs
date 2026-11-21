//! Read and normalize only explicitly allowed library fields. Never persist state.
use json::{object, JsonValue};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct SnapshotError {
    pub code: i32,
    pub kind: &'static str,
    pub message: &'static str,
}
impl SnapshotError {
    pub(crate) fn invalid(message: &'static str) -> Self {
        Self {
            code: -32011,
            kind: "invalidState",
            message,
        }
    }
    pub(crate) fn not_found() -> Self {
        Self {code:-32010,kind:"sourceNotFound",message:"Upstream state.json was not found; choose its path or open the upstream manager first."}
    }
    fn too_large() -> Self {
        Self {
            code: -32014,
            kind: "sourceTooLarge",
            message:
                "Upstream state exceeds the 64 MiB limit; choose a smaller supported state.json.",
        }
    }
}
impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for SnapshotError {}

fn io_error(error: io::Error) -> SnapshotError {
    if error.kind() == io::ErrorKind::NotFound {
        SnapshotError::not_found()
    } else {
        SnapshotError::invalid(
            "Cannot read upstream state; check file permissions and choose a readable state.json.",
        )
    }
}
fn warning(warnings: &mut JsonValue, code: &str, message: &str) {
    // Avoid large amplification from many tiny malformed entries.
    if warnings.len() >= 256 {
        if warnings.len() == 256 {
            warnings.push(object!{code:"additionalWarningsOmitted",message:"More warnings were omitted; inspect the upstream state in its original manager."}).expect("warnings array");
        }
        return;
    }
    // Messages are static explanations. Never echo malformed source values.
    warnings
        .push(object! {code:code,message:message})
        .expect("warnings array");
}
pub(crate) fn text(value: &JsonValue) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}

fn filenames(value: &JsonValue, warnings: &mut JsonValue) -> JsonValue {
    if value.is_null() {
        return JsonValue::new_array();
    }
    if !value.is_array() {
        warning(
            warnings,
            "invalidMetadata",
            "Invalid VPK filename metadata was omitted; no files were scanned.",
        );
        return JsonValue::new_array();
    }
    let mut result = JsonValue::new_array();
    for item in value.members() {
        if let Some(name) = text(item) {
            result.push(name).expect("filename array");
        } else {
            warning(
                warnings,
                "invalidMetadata",
                "Invalid VPK filename metadata was omitted; no files were scanned.",
            );
        }
    }
    result
}

fn images(value: &JsonValue) -> JsonValue {
    let mut result = JsonValue::new_array();
    let mut seen = HashSet::new();
    // This is a bounded whitelist, not a URL parser. Qt validates each saved
    // string with QUrl StrictMode before exposing it as a preview source.
    for entry in value.members() {
        let Some(url) = entry.as_str() else { continue };
        if url.len() > 4096
            || !url
                .get(..8)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
            || url.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            continue;
        }
        let authority = url[8..].split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() || authority.contains(['@', '\\']) || !seen.insert(url) {
            continue;
        }
        result.push(url).expect("images array");
        if result.len() == 32 {
            break;
        }
    }
    result
}

pub(crate) fn preview_hidden(item: &JsonValue, remote_id: &str, state: &JsonValue) -> bool {
    if !item["isNSFW"].as_bool().unwrap_or(false) {
        return false;
    }
    let settings = &state["nsfwSettings"];
    settings["hideNSFW"].as_bool().unwrap_or(false)
        || (!settings["disableBlur"].as_bool().unwrap_or(false)
            && !(settings["rememberPerItemOverrides"]
                .as_bool()
                .unwrap_or(true)
                && state["perItemNSFWOverrides"][remote_id]
                    .as_bool()
                    .unwrap_or(false)))
}

fn normalize_mods(
    mods: &JsonValue,
    enabled_map: &JsonValue,
    state: &JsonValue,
    warnings: &mut JsonValue,
) -> JsonValue {
    normalize_mods_inner(mods, enabled_map, state, warnings, true)
}

pub(crate) fn normalize_native_mods(
    mods: &JsonValue,
    enabled_map: &JsonValue,
    state: &JsonValue,
    warnings: &mut JsonValue,
) -> JsonValue {
    normalize_mods_inner(mods, enabled_map, state, warnings, false)
}
fn normalize_mods_inner(
    mods: &JsonValue,
    enabled_map: &JsonValue,
    state: &JsonValue,
    warnings: &mut JsonValue,
    warn_mismatch: bool,
) -> JsonValue {
    let mut result = JsonValue::new_array();
    let mut seen = HashSet::new();
    if !mods.is_array() {
        warning(
            warnings,
            "invalidModList",
            "Profile mods are missing or invalid; no library entries were invented.",
        );
        return result;
    }
    if !enabled_map.is_null() && !enabled_map.is_object() {
        warning(
            warnings,
            "invalidEnabledMap",
            "Invalid profile enabled map; mod status is used as the fallback.",
        );
    }
    for item in mods.members() {
        let (Some(id), Some(remote_id), Some(status)) = (
            text(&item["id"]),
            text(&item["remoteId"]),
            text(&item["status"]),
        ) else {
            warning(
                warnings,
                "invalidMod",
                "A mod with missing or invalid ID, remote ID, or status was skipped.",
            );
            continue;
        };
        if !item.is_object() {
            warning(warnings, "invalidMod", "An invalid mod entry was skipped.");
            continue;
        }
        if !seen.insert(id) {
            warning(
                warnings,
                "duplicateModId",
                "A duplicate mod ID was skipped; the first valid entry is shown.",
            );
            continue;
        }
        let fallback_enabled = status == "installed";
        let entry = &enabled_map[remote_id];
        let explicit = entry["enabled"].as_bool();
        if !entry.is_null() && explicit.is_none() {
            warning(
                warnings,
                "invalidEnabledEntry",
                "An invalid enabled entry was ignored; mod status is used as the fallback.",
            );
        }
        if warn_mismatch && explicit.is_some_and(|enabled| enabled != fallback_enabled) {
            warning(warnings,"enabledStatusMismatch","Profile enabled state disagrees with mod status; the explicit enabled flag is shown.");
        }
        let hero = text(&item["heroOverride"])
            .or_else(|| text(&item["detectedHero"]))
            .or_else(|| text(&item["hero"]))
            .unwrap_or("General");
        // Inspect number parts instead of converting through the dependency's
        // signed integer conversion, which overflows for i64::MIN in debug.
        let install_order = if item["installOrder"]
            .as_number()
            .is_some_and(|n| n.as_parts().2 == 0)
        {
            item["installOrder"].clone()
        } else {
            JsonValue::Null
        };
        if !item["installOrder"].is_null() && install_order.is_null() {
            warning(
                warnings,
                "invalidMetadata",
                "Invalid installation order was omitted.",
            );
        }
        let installed_vpks = filenames(&item["installedVpks"], warnings);
        let missing_vpks = filenames(&item["missingVpks"], warnings);
        let images = images(&item["images"]);
        let preview_hidden = preview_hidden(item, remote_id, state);
        result
            .push(object! {
                id:id,remoteId:remote_id,title:text(&item["name"]).unwrap_or(id),
                author:text(&item["author"]).unwrap_or("Unknown author"),hero:hero,
                category:text(&item["category"]).unwrap_or("Uncategorized"),status:status,
                enabled:explicit.unwrap_or(fallback_enabled),installOrder:install_order,
                installedVpks:installed_vpks,missingVpks:missing_vpks,
                images:images,previewHidden:preview_hidden
            })
            .expect("mods array");
    }
    result
}

pub fn read_snapshot(path: &Path, profile_id: Option<&str>) -> Result<JsonValue, SnapshotError> {
    let path = crate::discovery::absolute(path)?;
    // Reject directories/devices/FIFOs before File::open can block on them.
    let metadata = fs::metadata(&path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err(SnapshotError::invalid(
            "Source must be a regular state.json file.",
        ));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(SnapshotError::too_large());
    }
    let file = File::open(&path).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(SnapshotError::invalid(
            "Source must be a regular state.json file.",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(SnapshotError::too_large());
    }
    let contents = std::str::from_utf8(&bytes).map_err(|_| {
        SnapshotError::invalid("Source is not UTF-8 JSON; choose an intact upstream state.json.")
    })?;
    let root = json::parse(contents).map_err(|_| {
        SnapshotError::invalid(
            "Source contains malformed or truncated JSON; choose an intact upstream state.json.",
        )
    })?;
    let config = match &root["local-config"] {
        JsonValue::Short(_) | JsonValue::String(_) => {
            json::parse(root["local-config"].as_str().unwrap()).map_err(|_| {
                SnapshotError::invalid(
                    "The local-config JSON is malformed; choose an intact upstream state.json.",
                )
            })?
        }
        value if value.is_object() => value.clone(),
        _ => {
            return Err(SnapshotError::invalid(
                "Source does not contain a valid local-config object or JSON string.",
            ))
        }
    };
    let version = config["version"].as_u64().ok_or_else(|| {
        SnapshotError::invalid("The local-config schema version is missing or invalid.")
    })?;
    if version != 33 {
        return Err(SnapshotError{code:-32012,kind:"unsupportedVersion",message:"Only upstream store version 33 is supported; use a compatible state.json. No migration was applied."});
    }
    let state = &config["state"];
    if !state.is_object() || !state["profiles"].is_object() {
        return Err(SnapshotError::invalid(
            "The version 33 state or profiles object is missing or invalid.",
        ));
    }
    let mut warnings = JsonValue::new_array();
    let active_id = text(&state["activeProfileId"]);
    let mut profiles: Vec<(&str, &JsonValue)> = state["profiles"]
        .entries()
        .filter(|(id, profile)| {
            if id.trim().is_empty() || !profile.is_object() {
                warning(
                    &mut warnings,
                    "invalidProfile",
                    "An invalid profile was skipped.",
                );
                false
            } else {
                true
            }
        })
        .collect();
    // Deterministic fallback/list order, with upstream default first naturally.
    profiles.sort_by_key(|(id, _)| *id);
    let active_exists =
        active_id.is_some_and(|active| profiles.iter().any(|(id, _)| *id == active));
    if !active_exists {
        warning(&mut warnings,"activeProfileMissing","Active profile is missing; the first available profile is displayed without activating it.");
    }
    let selected = match profile_id {
        Some(requested) => Some(profiles.iter().find(|(id,_)| *id == requested).map(|(id,_)| *id).ok_or(SnapshotError{code:-32013,kind:"unknownProfile",message:"Requested profile does not exist; refresh and choose an available profile."})?),
        None if active_exists => active_id,
        None => profiles.first().map(|(id,_)| *id),
    };
    let mut summaries = JsonValue::new_array();
    let mut selected_mods = JsonValue::new_array();
    for (id, profile) in profiles {
        let mods = if Some(id) == active_id && state["localMods"].is_array() {
            &state["localMods"]
        } else {
            if Some(id) == active_id && state.has_key("localMods") {
                warning(
                    &mut warnings,
                    "invalidLocalMods",
                    "Active working mods are invalid; the saved profile snapshot is displayed.",
                );
            }
            &profile["mods"]
        };
        let normalized = normalize_mods(mods, &profile["enabledMods"], state, &mut warnings);
        summaries.push(object!{id:id,name:text(&profile["name"]).unwrap_or(id),isActive:Some(id)==active_id,modCount:normalized.len()}).expect("profiles array");
        if Some(id) == selected {
            selected_mods = normalized;
        }
    }
    let source_path = path.canonicalize().map_err(io_error)?;
    let source_path = source_path
        .to_str()
        .ok_or_else(|| SnapshotError::invalid("Source path must be representable as UTF-8."))?;
    Ok(
        object! {protocolVersion:1,sourcePath:source_path,storeVersion:33,activeProfileId:active_id,selectedProfileId:selected,profiles:summaries,mods:selected_mods,warnings:warnings},
    )
}
