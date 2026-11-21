//! Bounded, identity-bound font publication. RootLock precedes GameLock.
//! Journal paths are derived from typed targets and native IDs only.
use crate::{
    fonts::{
        self, CheckedFontImage, FontCheckpoint, FontImage, FontPlan, FontState, FontTarget,
        StoredFontImage,
    },
    game_access::{ensure_game_idle, GameLock},
    native::NativeError,
    package::{self, FileRef},
    paths,
    prepare_control::{self, PrepareControl},
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
const MAX_BYTES: u64 = 264 * 1024 * 1024;
const MAX_METADATA: usize = 8 * 1024 * 1024;
fn recovery() -> NativeError {
    NativeError::new("recoveryRequired","Font transaction evidence is incomplete or changed; preserve journals and backups and reconcile before continuing.")
}
fn limit() -> NativeError {
    NativeError::new(
        "fontLimit",
        "Font transaction exceeds its bounded target, byte or metadata budget.",
    )
}
fn s<'a>(v: &'a JsonValue, k: &str) -> Result<&'a str, NativeError> {
    v[k].as_str().ok_or_else(recovery)
}
fn max(t: &FontTarget) -> u64 {
    match t {
        FontTarget::Configuration => 4 * 1024 * 1024,
        FontTarget::Font(_) => 16 * 1024 * 1024,
    }
}
fn directory(game: &Path) -> PathBuf {
    game.join("game/citadel/panorama/fonts")
}
fn absent(path: &Path) -> Result<bool, NativeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(path)?;
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}
fn metadata_inode(m: &fs::Metadata) -> Result<JsonValue, NativeError> {
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o7000 != 0 {
        return Err(recovery());
    }
    Ok(object! {device:m.dev(),inode:m.ino()})
}
fn check_inode(path: &Path, id: &JsonValue, authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    metadata_inode(&fs::symlink_metadata(path)?)?;
    if !id.is_object() || !fonts::persisted_font_owner(path,id,authority)? {
        return Err(recovery());
    }
    Ok(())
}
fn created_inode(file: &File, path: &Path) -> Result<JsonValue, NativeError> {
    // Only the descriptor returned by create_new grants creation authority.
    let mut id = metadata_inode(&file.metadata()?)?;id["fsid"]=paths::persisted_filesystem_id(file)?.into();
    check_inode(path, &id, None)?;
    Ok(id)
}
fn check_created_image(
    file: &File,
    path: &Path,
    created: &JsonValue,
    actual: &fonts::CheckedFontFile,
) -> Result<(), NativeError> {
    let m = file.metadata()?;
    let fp = actual.identity();
    let mut base=created.clone();base.remove("fsid");
    if metadata_inode(&m)? != base || !crate::generation::valid_fsid(created)
        || created["fsid"].as_str().is_some_and(|id|paths::persisted_filesystem_id(file).map_or(true,|actual|actual!=id))
        || fp.device != m.dev()
        || fp.inode != m.ino()
        || fp.size != m.len()
        || fp.modified_seconds != m.mtime()
        || fp.modified_nanos != m.mtime_nsec()
        || actual.mode() != m.mode() & 0o777
    {
        return Err(recovery());
    }
    check_inode(path, created, None)
}
fn fp_json(v: &fonts::FontFingerprint) -> JsonValue {
    fonts::fingerprint_json(v)
}
fn digest(bytes: &[u8]) -> Result<String, NativeError> {
    prepare_control::digest_bounded_controlled(
        bytes,
        16 * 1024 * 1024,
        &PrepareControl::never_cancelled(),
    )
}
fn image_intent(bytes: Option<&[u8]>, mode: u32) -> Result<JsonValue, NativeError> {
    Ok(match bytes {
        None => object! {kind:"absent"},
        Some(bytes) => {
            object! {kind:"present",backup:package::encode_file(&FileRef{id:paths::random_id()?,sha256:digest(bytes)?,size:bytes.len() as u64,source_name:"font-image.blob".into()}),mode:mode,identity:JsonValue::Null,created:JsonValue::Null}
        }
    })
}
fn image_ref(v: &JsonValue, t: &FontTarget) -> Result<Option<FileRef>, NativeError> {
    match v["kind"].as_str() {
        Some("absent") if v.len() == 1 => Ok(None),
        Some("present") => {
            if v.len() != 5 || v["mode"].as_u32().filter(|m| *m <= 0o777).is_none() {
                return Err(recovery());
            }
            let r = package::decode_file(&v["backup"])?;
            if r.size > max(t) {
                return Err(limit());
            }
            if !v["identity"].is_null() {
                let fp = fonts::fingerprint_decode(&v["identity"], max(t))?;
                if fp.sha256 != r.sha256 || fp.size != r.size {
                    return Err(recovery());
                }
            }
            valid_inode(&v["created"])?;
            Ok(Some(r))
        }
        _ => Err(recovery()),
    }
}
fn valid_inode(v: &JsonValue) -> Result<(), NativeError> {
    if !v.is_null()
        && (!v.is_object()
            || v.len() != 2+usize::from(v.has_key("fsid")) || !crate::generation::valid_fsid(v)
            || v["device"].as_u64().is_none()
            || v["inode"].as_u64().filter(|i| *i != 0).is_none())
    {
        return Err(recovery());
    }
    Ok(())
}
fn stored(
    v: &JsonValue,
    t: &FontTarget,
    placeholder: bool,
) -> Result<StoredFontImage, NativeError> {
    Ok(match image_ref(v, t)? {
        None => StoredFontImage::Absent,
        Some(backup) => {
            let identity = if placeholder && v["identity"].is_null() {
                fonts::FontFingerprint { fsid:Some("ffffffffffffffff".into()),
                    sha256: backup.sha256.clone(),
                    size: backup.size,
                    device: u64::MAX,
                    inode: u64::MAX,
                    modified_seconds: i64::MIN,
                    modified_nanos: 999_999_999,
                }
            } else {
                fonts::fingerprint_decode(&v["identity"], max(t))?
            };
            StoredFontImage::Present {
                backup,
                identity,
                mode: v["mode"].as_u32().ok_or_else(recovery)?,
            }
        }
    })
}
fn checkpoint(j: &JsonValue, placeholder: bool) -> Result<Option<FontCheckpoint>, NativeError> {
    if j["undo"] == true {
        return Ok(None);
    }
    Ok(Some(FontCheckpoint {
        transaction_id: s(j, "transactionId")?.into(),
        prior: FontState::decode(&j["beforeDocument"])?.installed,
        images: j["targets"]
            .members()
            .map(|r| {
                let target = fonts::target_decode(&r["target"])?;
                Ok(FontImage {
                    image: stored(&r["before"], &target, placeholder)?,
                    target,
                })
            })
            .collect::<Result<_, NativeError>>()?,
    }))
}
fn project(j: &JsonValue, placeholder: bool) -> Result<JsonValue, NativeError> {
    let mut doc = j["requestedDocument"].clone();
    let mut state = FontState::decode(&doc)?;
    state.previous = checkpoint(j, placeholder)?;
    if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) { fonts::retain_directories(&mut state); }
    doc["fonts"] = state.encode()?;
    Ok(doc)
}
fn metadata_limit(j: &JsonValue) -> Result<(), NativeError> {
    struct Meter(usize);
    impl Write for Meter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::FileTooLarge,
                    "font journal metadata limit",
                ));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    j.write(&mut Meter(MAX_METADATA)).map_err(|_| limit())?;
    store::ensure_internal(j)
}
fn persist(store: &Store, lock: &RootLock, id: &str, j: &JsonValue) -> Result<(), NativeError> {
    metadata_limit(j)?;
    store.write_journal(lock, id, j)?;
    if store.journal(id)?.as_ref() != Some(j) {
        return Err(recovery());
    }
    Ok(())
}

fn exact_keys(v: &JsonValue, keys: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len() != keys.len() || keys.iter().any(|k| !v.has_key(k)) {
        return Err(recovery());
    }
    Ok(())
}
fn state_shape(footprint: Option<fonts::FontFootprint>, observed_directory: bool) -> Result<JsonValue, NativeError> {
    let mut v = FontState {
        installed: footprint,
        previous: None,
    }
    .encode()?;
    if !v["installed"].is_null() {
        if observed_directory {
            v["installed"]["directory"] = JsonValue::Null;
            v["installed"]["directoryCreated"] = false.into();
        }
        for f in v["installed"]["files"].members_mut() {f["identity"].remove("fsid");
            for k in ["device", "inode", "modifiedSeconds", "modifiedNanos"] {
                f["identity"][k] = 0.into();
            }
        }
        if !v["installed"]["conf"].is_null() {v["installed"]["conf"].remove("fsid");
            for k in ["device", "inode", "modifiedSeconds", "modifiedNanos"] {
                v["installed"]["conf"][k] = 0.into();
            }
        }
        if !v["installed"]["directory"].is_null() {
            v["installed"]["directory"] = object! {device:0,inode:0};
        }
    }
    Ok(v)
}
fn transition_contract(j: &JsonValue) -> Result<(), NativeError> {
    let before = FontState::decode(&j["beforeDocument"])?;
    let after = FontState::decode(&project(j, true)?)?;
    if j["undo"] == true {
        let previous = before.previous.as_ref().ok_or_else(recovery)?;
        if state_shape(previous.prior.clone(), matches!(j["version"].as_u32(), Some(2 | 3 | 4)))? != state_shape(after.installed.clone(), matches!(j["version"].as_u32(), Some(2 | 3 | 4)))?
            || previous.images.len() != j["targets"].len()
        {
            return Err(recovery());
        }
        for (saved, row) in previous.images.iter().zip(j["targets"].members()) {
            if fonts::target_json(&saved.target) != row["target"] {
                return Err(recovery());
            }
            match (&saved.image, image_ref(&row["after"], &saved.target)?) {
                (StoredFontImage::Absent, None) => (),
                (
                    StoredFontImage::Present {
                        backup,
                        mode: permissions,
                        ..
                    },
                    Some(r),
                ) if backup.sha256 == r.sha256
                    && backup.size == r.size
                    && *permissions == mode(&row["after"]) =>
                {
                    ()
                }
                _ => return Err(recovery()),
            }
        }
    }
    let before_files = before
        .installed
        .as_ref()
        .map(|f| f.files.as_slice())
        .unwrap_or(&[]);
    let after_files = after
        .installed
        .as_ref()
        .map(|f| f.files.as_slice())
        .unwrap_or(&[]);
    for row in j["targets"].members() {
        let t = fonts::target_decode(&row["target"])?;
        let old = match &t {
            FontTarget::Font(name) => before_files
                .iter()
                .find(|f| f.target_name == *name)
                .map(|f| (&f.identity, f.mode)),
            FontTarget::Configuration => before
                .installed
                .as_ref()
                .and_then(|f| f.conf.as_ref().map(|fp| (fp, f.conf_mode.unwrap_or(0)))),
        };
        if let Some((fp, m)) = old {
            if row["expectedBefore"] != fp_json(fp) || mode(&row["before"]) != m {
                return Err(recovery());
            }
        } else if (matches!(t, FontTarget::Font(_)) || before.installed.is_some())
            && !row["expectedBefore"].is_null()
        {
            return Err(recovery());
        }
        if let FontTarget::Font(name) = &t {
            if before_files.iter().chain(after_files.iter()).any(|f| {
                f.target_name == *name && f.ownership == fonts::FontFileOwnership::Borrowed
            }) {
                return Err(recovery());
            }
        }
        let new = match &t {
            FontTarget::Font(name) => after_files
                .iter()
                .find(|f| f.target_name == *name)
                .map(|f| (&f.identity, f.mode)),
            FontTarget::Configuration => after
                .installed
                .as_ref()
                .and_then(|f| f.conf.as_ref().map(|fp| (fp, f.conf_mode.unwrap_or(0)))),
        };
        match (new, image_ref(&row["after"], &t)?) {
            (Some((fp, m)), Some(r))
                if fp.sha256 == r.sha256 && fp.size == r.size && m == mode(&row["after"]) =>
            {
                ()
            }
            (None, None) => (),
            // Undo to the initial unobserved state can restore its original conf.
            (None, Some(_))
                if j["undo"] == true
                    && after.installed.is_none()
                    && t == FontTarget::Configuration =>
            {
                ()
            }
            _ => return Err(recovery()),
        }
    }
    Ok(())
}

/// Pure journal admission, used before private jobs may reconcile or clean up.
pub(crate) fn admit(store: &Store, j: &JsonValue) -> Result<(), NativeError> {
    admit_inner(store,j).map_err(|_|recovery())
}
fn admit_inner(store: &Store, j: &JsonValue) -> Result<(), NativeError> {
    crate::deployment_metadata::domain(&store.document()?,j["version"]==4)?;
    for key in ["beforeDocument","requestedDocument","document"] {
        if j.has_key(key) {
            crate::deployment_metadata::domain(&j[key],j["version"]==4)?;
            if j["version"]==4 {crate::deployment_metadata::admit(store.root(),&j[key])?;}
        }
    }
    metadata_limit(j)?;
    let mut journal_keys = vec![
            "kind",
            "version",
            "libraryVersion",
            "root",
            "game",
            "transactionId",
            "expectedRevision",
            "input",
            "beforeDocument",
            "requestedDocument",
            "undo",
            "targets",
            "phase",
            "parents",
            "names",
            "directoryBefore",
            "directoryCreated",
            "directoryRestored",
            "createDirectory",
            "removeDirectory",
            "removeIntent",
            "backupDirectory",
        ];
    if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) { journal_keys.push("directoryObserved"); }
    exact_keys(j, &journal_keys)?;
    exact_keys(&j["input"], &["method", "params"])?;
    exact_keys(
        &j["input"]["params"],
        &["operationId", "expectedRevision", "token"],
    )?;
    if !paths::safe_id(s(&j["input"]["params"], "operationId")?) {
        return Err(recovery());
    }
    if j["kind"] != "gameFonts"
        || !matches!(j["version"].as_u32(), Some(1 | 2 | 3 | 4))
        || (matches!(j["version"].as_u32(),Some(3|4)) && j["libraryVersion"] != 4)
        || (!matches!(j["version"].as_u32(),Some(3|4)) && j["libraryVersion"] != 3)
        || !j["input"].is_object()
        || j["input"]["method"] != "native.fonts.commit"
        || j["undo"].as_bool().is_none()
        || !matches!(
            j["phase"].as_str(),
            Some("staging" | "writing" | "rollback" | "ready")
        )
        || !package::hex(s(j, "transactionId")?, 32)
        || s(j, "root")? != store.root().to_str().ok_or_else(recovery)?
        || !j["targets"].is_array()
        || j["targets"].len() > 257
        || !j["parents"].is_array()
        || !j["names"].is_array()
        || j["names"].len() > 10000
        || j["createDirectory"].as_bool().is_none()
        || j["removeDirectory"].as_bool().is_none()
        || j["removeIntent"].as_bool().is_none()
    {
        return Err(recovery());
    }
    // v1 directory transitions used post-mkdir path identities as ownership.
    // Preserve all their evidence rather than performing guessed cleanup.
    if j["removeDirectory"] != false || j["removeIntent"] != false
        || !j["directoryCreated"].is_null() || !j["directoryRestored"].is_null()
        || j["version"] == 1 && j["createDirectory"] != false
    { return Err(recovery()); }
    if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) && !j["directoryObserved"].is_null() {
        fonts::dir_decode(&j["directoryObserved"])?;
        if j["createDirectory"] != true || !j["directoryBefore"].is_null() {
            return Err(recovery());
        }
    }
    let expected = store::revision(s(j, "expectedRevision")?)?;
    if j["input"]["params"]["expectedRevision"] != expected.to_string()
        || !package::hex(s(&j["input"]["params"], "token")?, 32)
    {
        return Err(recovery());
    }
    let before = &j["beforeDocument"];
    let requested = &j["requestedDocument"];
    store::ensure_readable(before)?;
    store::ensure_readable(requested)?;
    if j["version"]==3 { crate::apply::admit_game_document(store.root(),before)?; }
    if store::revision(s(before, "revision")?)? != expected
        || requested["revision"] != before["revision"]
        || (matches!(j["version"].as_u32(),Some(3|4)) && (requested["storeVersion"] != 4 || before["storeVersion"] != 4))
        || (!matches!(j["version"].as_u32(),Some(3|4)) && requested["storeVersion"] != 3)
        || requested["gamePath"] != before["gamePath"]
        || s(j, "game")? != s(before, "gamePath")?
    {
        return Err(recovery());
    }
    let mut stripped_before = before.clone();
    let mut stripped_requested = requested.clone();
    for d in [&mut stripped_before, &mut stripped_requested] {
        d.remove("fonts");
        d.remove("storeVersion");
        d.remove("packageVersion");
    }
    if stripped_before != stripped_requested
        || package::layout_version(before)? != package::layout_version(requested)?
    {
        return Err(recovery());
    }
    let game = Path::new(s(j, "game")?);
    if paths::absolute(game)? != game {
        return Err(recovery());
    }
    let mut required = BTreeSet::new();
    for p in [store.root(), directory(game).parent().ok_or_else(recovery)?] {
        for parent in p.ancestors() {
            required.insert(parent.to_path_buf());
        }
    }
    let mut parents = BTreeSet::new();
    for p in j["parents"].members() {
        let path = PathBuf::from(s(p, "path")?);
        fonts::dir_decode(&p["identity"])?;
        if !parents.insert(path) {
            return Err(recovery());
        }
    }
    if parents != required {
        return Err(recovery());
    }
    for k in [
        "directoryBefore",
        "directoryCreated",
        "directoryRestored",
        "backupDirectory",
    ] {
        if !j[k].is_null() {
            fonts::dir_decode(&j[k])?;
        }
    }
    if j["createDirectory"] == true
        && (!j["directoryBefore"].is_null() || j["removeDirectory"] == true)
    {
        return Err(recovery());
    }
    let names = j["names"]
        .members()
        .map(|n| {
            n.as_str()
                .filter(|n| paths::safe_name(n))
                .ok_or_else(recovery)
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.windows(2).any(|w| w[0] >= w[1]) {
        return Err(recovery());
    }
    let mut targets = BTreeSet::new();
    let mut ids = HashSet::new();
    let mut total = 0u64;
    let mut previous = None;
    let mut temps = HashSet::new();
    for row in j["targets"].members() {
        exact_keys(
            row,
            &[
                "target",
                "before",
                "after",
                "expectedBefore",
                "temp",
                "tempCreated",
                "published",
                "writeIntent",
                "rollbackTemp",
                "rollbackCreated",
                "restored",
                "rollbackIntent",
            ],
        )?;
        let t = fonts::target_decode(&row["target"])?;
        if row["target"] != fonts::target_json(&t) {
            return Err(recovery());
        }
        if previous.as_ref().is_some_and(|p| p >= &t) || !targets.insert(t.clone()) {
            return Err(recovery());
        }
        previous = Some(t.clone());
        let before = image_ref(&row["before"], &t)?;
        let after = image_ref(&row["after"], &t)?;
        if before.is_none() && after.is_none() {
            return Err(recovery());
        }
        for r in [before.as_ref(), after.as_ref()].into_iter().flatten() {
            if !ids.insert(r.id.clone()) {
                return Err(recovery());
            }
            total = total
                .checked_add(r.size)
                .filter(|n| *n <= MAX_BYTES)
                .ok_or_else(limit)?;
        }
        if let Some(r) = before {
            let fp = fonts::fingerprint_decode(&row["expectedBefore"], max(&t))?;
            if fp.sha256 != r.sha256 || fp.size != r.size {
                return Err(recovery());
            }
        } else if !row["expectedBefore"].is_null() {
            return Err(recovery());
        }
        for key in ["temp", "rollbackTemp"] {
            let name = s(row, key)?;
            if !name
                .strip_prefix(".native-font-")
                .is_some_and(|v| package::hex(v, 32))
                || !temps.insert(name)
                || names.contains(&name)
            {
                return Err(recovery());
            }
        }
        for key in ["tempCreated", "rollbackCreated"] {
            valid_inode(&row[key])?;
        }
        for key in ["published", "restored"] {
            if !row[key].is_null() {
                let fp = fonts::fingerprint_decode(&row[key], max(&t))?;
                let image = if key == "published" {
                    &row["after"]
                } else {
                    &row["before"]
                };
                let created = if key == "published" {
                    &row["tempCreated"]
                } else {
                    &row["rollbackCreated"]
                };
                let r = image_ref(image, &t)?.ok_or_else(recovery)?;
                if r.sha256 != fp.sha256
                    || r.size != fp.size
                    || created["device"].as_u64() != Some(fp.device)
                    || created["inode"].as_u64() != Some(fp.inode)
                {
                    return Err(recovery());
                }
            }
        }
        if row["writeIntent"].as_bool().is_none() || row["rollbackIntent"].as_bool().is_none() {
            return Err(recovery());
        }
    }
    let next = project(j, true)?;
    store::ensure_readable(&next)?;
    transition_contract(j)?;
    // Re-admit every immutable ID across native roles, including the new backup set.
    let claimed = document_refs(before)?;
    if ids.iter().any(|id| claimed.contains_key(id)) {
        return Err(recovery());
    }
    let before_state = FontState::decode(before)?;
    let next_state = FontState::decode(&next)?;
    for f in before_state
        .installed
        .iter()
        .chain(next_state.installed.iter())
    {
        for (path, identity) in [
            (&f.root_path, &f.root_identity),
            (&f.game_path, &f.game_identity),
        ] {
            if !j["parents"].members().any(|p| {
                p["path"].as_str() == path.to_str() && p["identity"] == fonts::dir_json(identity)
            }) {
                return Err(recovery());
            }
        }
        if f.root_path != store.root()
            || f.game_path != game
            || f.files.len() > 256
            || f.files
                .iter()
                .try_fold(0u64, |n, f| n.checked_add(f.identity.size))
                .filter(|n| *n <= 128 * 1024 * 1024)
                .is_none()
        {
            return Err(limit());
        }
    }
    if before_state.installed.as_ref().is_some_and(|f| {
        f.directory
            .as_ref()
            .map(fonts::dir_json)
            .unwrap_or(JsonValue::Null)
            != j["directoryBefore"]
    }) {
        return Err(recovery());
    }
    if matches!(j["version"].as_u32(),Some(3|4)) {
        if let (Some(old),Some(next))=(&before_state.installed,&next_state.installed) {
            for file in &next.files {
                if !targets.contains(&FontTarget::Font(file.target_name.clone())) {
                    if let Some(previous)=old.files.iter().find(|f|f.target_name==file.target_name) {
                        if file.identity!=previous.identity || file.mode!=previous.mode { return Err(recovery()); }
                    }
                }
            }
            if !targets.contains(&FontTarget::Configuration) && (next.conf!=old.conf || next.conf_mode!=old.conf_mode) { return Err(recovery()); }
        }
        admit_font3_progress(j)?;
        admit_current(store,j)?;
    }
    Ok(())
}

fn admit_current(store: &Store, j: &JsonValue) -> Result<bool,NativeError> {
    let id=s(&j["input"]["params"],"operationId")?;
    let status=store.operation(id)?;
    if status["status"]=="failed" { return Err(recovery()); }
    let succeeded=status["status"]=="succeeded";
    let mut expected=if succeeded {rebound_document(j,false)?} else {j["beforeDocument"].clone()};
    if succeeded {
        let result=store.replay(id,&j["input"])?.ok_or_else(recovery)?;
        expected["revision"]=store::revision(s(j,"expectedRevision")?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
        if !store::matches_recorded_snapshot(&expected,&result)? { return Err(recovery()); }
    }
    if store.document()?!=expected { return Err(recovery()); }
    Ok(succeeded)
}

fn admit_font3_progress(j: &JsonValue) -> Result<(),NativeError> {
    let staging=j["phase"]=="staging";
    for row in j["targets"].members() {
        for key in ["before","after"] {
            let image=&row[key];
            if image["kind"]=="present" {
                exact_keys(image,&["kind","backup","mode","identity","created"])?;
                exact_keys(&image["backup"],&["id","sha256","size","sourceName"])?;
                if !image["identity"].is_null() {
                    let mut base=image["identity"].clone();base.remove("fsid");exact_keys(&base,&["sha256","size","device","inode","modifiedSeconds","modifiedNanos"])?;if !crate::generation::valid_fsid(&image["identity"]){return Err(recovery());}
                }
                if !image["identity"].is_null() && (image["created"].is_null()
                    || image["identity"]["device"]!=image["created"]["device"] || image["identity"]["inode"]!=image["created"]["inode"]) {
                    return Err(recovery());
                }
                // Staging can be interrupted before its blobs were sealed;
                // rollback then only retains unchanged target images.
                if !staging && j["phase"]!="rollback" && image["identity"].is_null() { return Err(recovery()); }
            }
        }
        for key in ["expectedBefore","published","restored"] {
            if !row[key].is_null() { let mut base=row[key].clone();base.remove("fsid");exact_keys(&base,&["sha256","size","device","inode","modifiedSeconds","modifiedNanos"])?;if !crate::generation::valid_fsid(&row[key]){return Err(recovery());} }
        }
        for (intent,created,written,image) in [("writeIntent","tempCreated","published","after"),("rollbackIntent","rollbackCreated","restored","before")] {
            if row[intent]!=true && (!row[created].is_null() || !row[written].is_null())
                || row[image]["kind"]=="absent" && (!row[created].is_null() || !row[written].is_null())
                || staging && row[intent]!=false
                || intent=="rollbackIntent" && j["phase"]!="rollback" && row[intent]!=false {
                return Err(recovery());
            }
        }
        if j["phase"]=="ready" && (row["writeIntent"]!=true || row["after"]["kind"]=="present" && row["published"].is_null()) {
            return Err(recovery());
        }
    }
    if staging && !j["directoryObserved"].is_null() { return Err(recovery()); }
    if j["phase"]=="ready" && rebound_document(j,false)?!=j["requestedDocument"] { return Err(recovery()); }
    Ok(())
}

/// Entire journal namespace admission before any private cleanup. Filesystem
/// ownership is additionally preflighted by recover under the shared game locks.
pub(crate) fn admit_set(store: &Store, journals: &JsonValue) -> Result<(), NativeError> {
    admit_set_inner(store, journals).map_err(|_| recovery())
}
fn admit_set_inner(store: &Store, journals: &JsonValue) -> Result<(), NativeError> {
    if journals.members().any(|e|e["body"]["kind"]=="configWrite") {
        if journals.len()!=1{return Err(recovery())}return crate::configs::admit(store,&journals[0]["body"]);
    }
    if journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])) {
        return crate::remote::recovery::admit_set(store,journals);
    }
    let current=match store.document() {
        Ok(doc)=>{ crate::deployment_metadata::admit(store.root(),&doc)?; store::ensure_readable(&doc)?; Some(doc) },
        Err(e) if e.kind=="nativeNotImported"=>None,
        Err(e)=>return Err(e),
    };
    let mut ids = HashSet::new();
    let mut font_count = 0;
    let mut package_count = 0;
    let mut import_only_attempt = false;
    let mut general_attempt = false;
    for entry in journals.members() {
        let j = &entry["body"];
        let game5=j["kind"]=="game" && j["version"]==5;
        let game6=j["kind"]=="game" && j["version"]==6;
        let shard_outer=matches!((j["kind"].as_str(),j["version"].as_u32()),
            (Some("game"),Some(5|6))|(Some("import"),Some(7))|(Some("gameFonts"),Some(4))|(Some("recovery"),Some(5|6)));
        if let Some(doc)=&current {
            if game5 || game6 || j["kind"]=="recovery" && matches!(j["version"].as_u32(),Some(5|6)) && j["mode"]=="general" {
                crate::deployment_metadata::admit(store.root(),doc)?;
            } else {crate::deployment_metadata::domain(doc,shard_outer)?;}
        }
        for key in ["document","beforeDocument","requestedDocument"] {
            // game5/game6 alone admit an exact prospective8 applied checkpoint; its
            // producer/decoder reserves a persisted11 projection separately.
            if (game5 || game6) && key=="document" {continue;}
            if shard_outer {if j.has_key(key) {crate::deployment_metadata::admit(store.root(),&j[key])?;}}
            else {crate::generation::require_flat_document(&j[key])?;}
        }
        let id = s(entry, "id")?;
        if !paths::safe_id(id) {
            return Err(recovery());
        }
        let modern_import = j["kind"]=="import" && matches!(j["version"].as_u32(),Some(6|7));
        let import_recovery = j["kind"]=="recovery" && matches!(j["version"].as_u32(),Some(4|5|6));
        let modern_game = j["kind"]=="game" && matches!(j["version"].as_u32(),Some(4|5|6));
        let modern_font = j["kind"]=="gameFonts" && matches!(j["version"].as_u32(),Some(3|4));
        if current.as_ref().is_some_and(|d|d["storeVersion"]==4)
            && (j["kind"]=="game" && !modern_game || j["kind"]=="gameFonts" && !modern_font
                || j["kind"]=="recovery" && !import_recovery) { return Err(recovery()); }
        // Store4 does not widen any old outer artifact's document semantics.
        // This namespace gate also dominates private job reconciliation/cleanup.
        if ["document","beforeDocument","requestedDocument"].iter().any(|key| {
            let doc = &j[*key];
            if doc.has_key("storeVersion") {
                !matches!(doc["storeVersion"].as_u32(),Some(1|2|3)) && !((modern_import || modern_game || modern_font) && doc["storeVersion"].as_u32()==Some(4)) || package::layout_version(doc).is_err()
            } else {doc.has_key("packageVersion")}
        })
            || ["libraryVersion","libraryVersionBefore","libraryVersionAfter"].iter().any(|key|
                j.has_key(key) && !matches!(j[*key].as_u32(),Some(1|2|3)) && !((modern_import || import_recovery || modern_game || modern_font) && j[*key].as_u32()==Some(4)))
            || j.has_key("packageVersion") && !matches!(j["packageVersion"].as_u32(),Some(1|2)) {
            return Err(recovery());
        }
        match j["kind"].as_str() {
            Some("gameFonts") => {
                admit(store, j)?;
                font_count += 1;
                if j["input"]["params"]["operationId"] != id {
                    return Err(recovery());
                }
                for row in j["targets"].members() {
                    let target = fonts::target_decode(&row["target"])?;
                    for key in ["before", "after"] {
                        if let Some(f) = image_ref(&row[key], &target)? {
                            if !ids.insert(f.id) {
                                return Err(recovery());
                            }
                        }
                    }
                }
            }
            Some("game") => {
                if game6 {crate::apply::admit_game6(store,id,j)?;}
                else if game5 {crate::apply::admit_game5(store,id,j)?;}
                else if modern_game { crate::apply::admit_game4(store,id,j)?; }
                else if !matches!(j["version"].as_u32(), Some(1 | 2 | 3)) {
                    return Err(recovery());
                }
                package_count += 1;
            }
            Some("recovery") => {
                if import_recovery {
                    crate::apply::admit_metadata_recovery(store,id,j)?;
                    import_only_attempt |= j["mode"]=="localImportOnly";
                    general_attempt |= j["mode"]=="general";
                }
                else if !matches!(j["version"].as_u32(), Some(1 | 2 | 3)) || !j["input"].is_object()
                    || j.has_key("mode") && j["mode"] != "localImportOnly" {
                    return Err(recovery());
                }
            }
            Some("import") => {
                if j.has_key("version") && !matches!(j["version"].as_u32(), Some(1 | 2 | 3 | 4 | 5 | 6 | 7))
                {
                    return Err(recovery());
                }
                let local = if modern_import || matches!(j["version"].as_u32(), Some(3 | 4 | 5)) {
                    if modern_import {
                        if j["operationId"]!=id {return Err(recovery());}
                        crate::local_import::admit_modern_claims(store,j)?;
                    } else {
                        crate::local_import::admit_legacy_record(j)?;
                    }
                    j["files"]
                        .members()
                        .map(package::decode_file)
                        .collect::<Result<Vec<_>, _>>()?
                        .into_iter()
                        .map(|r| r.id)
                        .collect::<HashSet<_>>()
                } else {
                    // Older import journals list payloads as {id,sha256,size}
                    // without a source name; only their IDs are needed here.
                    if !j["payloads"].is_array() {return Err(recovery());}
                    j["payloads"]
                        .members()
                        .map(|p| p["id"].as_str().filter(|s| package::hex(s, 32)).map(str::to_owned).ok_or_else(recovery))
                        .collect::<Result<HashSet<_>, _>>()?
                };
                for file_id in local {
                    if !ids.insert(file_id) {
                        return Err(recovery());
                    }
                }
            }
            _ => return Err(recovery()),
        }
    }
    // Both game writers replace the same whole-document revision. Concurrent
    // game journals cannot be ordered safely even with disjoint target paths.
    if font_count > 1 || font_count != 0 && package_count != 0 || import_only_attempt && (font_count!=0 || package_count!=0)
        || (general_attempt || journals.members().any(|e|e["body"]["kind"]=="game" && matches!(e["body"]["version"].as_u32(),Some(4|5|6)))) && package_count>1 {
        return Err(recovery());
    }
    // A pending game6 may pair only with Recovery6 aimed at this exact main.
    // Historical general attempts retain their original game/font domains.
    for attempt in journals.members().filter(|e|e["body"]["kind"]=="recovery" && e["body"]["mode"]=="general") {
        let a=&attempt["body"];
        for main in journals.members().filter(|e|matches!(e["body"]["kind"].as_str(),Some("game"|"gameFonts"))) {
            let vanilla=main["body"]["kind"]=="game" && main["body"]["version"]==6;
            if (a["version"]==6)!=vanilla || vanilla && (a["mainOperationId"]!=main["id"] || a["mainInput"]!=main["body"]["input"]) {
                return Err(recovery());
            }
        }
    }
    if general_attempt {
        let current=store.document()?;
        let current_revision=store::revision(s(&current,"revision")?)?;
        for main in journals.members().filter(|e|matches!(e["body"]["kind"].as_str(),Some("game"|"gameFonts"))) {
            let j=&main["body"];
            if j["kind"]=="game" && matches!(j["version"].as_u32(),Some(5|6)) {
                if j["libraryVersionAfter"]!=4 {return Err(recovery());}
            } else if j["libraryVersion"]!=4 { return Err(recovery()); }
            let writer_revision=store::revision(s(j,"expectedRevision")?)?;
            if current_revision==writer_revision && journals.members().filter(|e|e["body"]["mode"]=="general")
                .any(|e|e["body"]["expectedRevision"]!=j["expectedRevision"]) { return Err(recovery()); }
        }
    }
    Ok(())
}

/// All referenced immutable IDs carry one role and one complete FileRef.
pub(crate) fn document_refs(
    doc: &JsonValue,
) -> Result<BTreeMap<String, (FileRef, &'static str)>, NativeError> {
    let mut refs = BTreeMap::new();
    let version = package::layout_version(doc)?;
    for (_, p) in doc["profiles"].entries() {
        for record in p["mods"].members() {
            for (f, role) in crate::local_import::record_files(record, version)? {
                insert_ref(&mut refs, f, role)?;
            }
        }
    }
    for f in backup_refs(doc)? {
        insert_ref(&mut refs, f, "fontBackup")?;
    }
    Ok(refs)
}
fn insert_ref(
    map: &mut BTreeMap<String, (FileRef, &'static str)>,
    f: FileRef,
    role: &'static str,
) -> Result<(), NativeError> {
    let value = (f.clone(), role);
    if map
        .insert(f.id, value.clone())
        // Imported payloads may share immutable bytes under different source
        // names. Names are provenance; digest, size and role bind ownership.
        .is_some_and(|old| old.1 != role || old.0.sha256 != f.sha256 || old.0.size != f.size
            || (role != "payload" && old.0.source_name != f.source_name))
    {
        return Err(recovery());
    }
    Ok(())
}
pub(crate) fn backup_refs(doc: &JsonValue) -> Result<Vec<FileRef>, NativeError> {
    let state = FontState::decode(doc)?;
    Ok(state
        .previous
        .into_iter()
        .flat_map(|p| p.images)
        .filter_map(|i| match i.image {
            StoredFontImage::Present { backup, .. } => Some(backup),
            _ => None,
        })
        .collect())
}

fn verify_parents(j: &JsonValue, authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    for p in j["parents"].members() {
        if !fonts::directory_matches(Path::new(s(p,"path")?),&fonts::dir_decode(&p["identity"])?,authority)?
        {
            return Err(recovery());
        }
    }
    if !j["backupDirectory"].is_null()
        && !fonts::directory_matches(&crate::storage::path(Path::new(s(j,"root")?),"font-backups")?,&fonts::dir_decode(&j["backupDirectory"])?,authority)?
    {
        return Err(recovery());
    }
    Ok(())
}
fn verify_blob(
    root: &Path,
    v: &JsonValue,
    t: &FontTarget,
 authority: Option<&fonts::FontAuthority>) -> Result<Option<fonts::CheckedFontFile>, NativeError> {
    let Some(r) = image_ref(v, t)? else {
        return Ok(None);
    };
    let p = fonts::backup_path(root, &r.id)?;
    if v["identity"].is_null() {
        if !absent(&p)? {
            check_inode(&p, &v["created"], authority)?;
        }
        return Ok(None);
    }
    let f = fonts::transaction_read(&p, max(t))?;
    if !fonts::fingerprint_matches(&p,&f.identity,&fonts::fingerprint_decode(&v["identity"],max(t))?,authority)? || f.mode != 0o400 {
        return Err(recovery());
    }
    check_inode(&p, &v["created"], authority)?;
    Ok(Some(f))
}
fn write_new(path: &Path) -> Result<File, NativeError> {
    paths::verify_absent(path)?;
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(0x20000 | 0x800)
        .open(path)?)
}
fn seal_blob(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &mut JsonValue,
    index: usize,
    key: &str,
    bytes: Option<&[u8]>,
 authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    let Some(bytes) = bytes else { return Ok(()) };
    let t = fonts::target_decode(&j["targets"][index]["target"])?;
    let r = image_ref(&j["targets"][index][key], &t)?.ok_or_else(recovery)?;
    let path = fonts::backup_path(store.root(), &r.id)?;
    verify_parents(j, authority)?;
    let mut f = write_new(&path)?;
    j["targets"][index][key]["created"] = created_inode(&f, &path)?;
    persist(store, lock, id, j)?;
    f.write_all(bytes)?;
    f.set_permissions(fs::Permissions::from_mode(0o400))?;
    f.sync_all()?;
    paths::sync_dir(path.parent().unwrap())?;
    let actual = fonts::transaction_read(&path, max(&t))?;
    check_created_image(&f, &path, &j["targets"][index][key]["created"], &actual)?;
    if actual.identity.sha256 != r.sha256 || actual.identity.size != r.size || actual.mode != 0o400
    {
        return Err(recovery());
    }
    j["targets"][index][key]["identity"] = fp_json(&actual.identity);
    persist(store, lock, id, j)
}
fn matches(path: &Path, fp: &JsonValue, mode: u32, t: &FontTarget, authority: Option<&fonts::FontAuthority>) -> Result<bool, NativeError> {
    if absent(path)? {
        return Ok(fp.is_null());
    }
    if fp.is_null() {
        return Ok(false);
    }
    let f = fonts::transaction_read(path, max(t))?;
    Ok(fonts::fingerprint_matches(path,&f.identity,&fonts::fingerprint_decode(fp,max(t))?,authority)? && f.mode==mode)
}
fn mode(v: &JsonValue) -> u32 {
    v["mode"].as_u32().unwrap_or(0)
}
fn check_temp(dir: &Path, row: &JsonValue, rollback: bool, authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    let (name, created, published) = if rollback {
        ("rollbackTemp", "rollbackCreated", "restored")
    } else {
        ("temp", "tempCreated", "published")
    };
    let path = dir.join(s(row, name)?);
    if !absent(&path)? {
        check_inode(&path, &row[created], authority)?;
        if !row[published].is_null() {
            let t = fonts::target_decode(&row["target"])?;
            let image = if rollback {
                &row["before"]
            } else {
                &row["after"]
            };
            if !matches(&path, &row[published], mode(image), &t, authority)? {
                return Err(recovery());
            }
        }
    }
    Ok(())
}
fn temp_removal(dir:&Path,row:&JsonValue,rollback:bool,authority:Option<&fonts::FontAuthority>)->Result<paths::DestructiveFile,NativeError>{
    let (name,created,written,image)=if rollback{("rollbackTemp","rollbackCreated","restored","before")}else{("temp","tempCreated","published","after")};
    let path=dir.join(s(row,name)?);check_inode(&path,&row[created],authority)?;
    let target=fonts::target_decode(&row["target"])?;
    let hash=if let Some(sha)=row[written]["sha256"].as_str(){Some(sha.to_owned())}else{image_ref(&row[image],&target)?.map(|r|r.sha256)};
    paths::DestructiveFile::admitted(path,&row[created],hash)
}
fn inventory(dir: &Path) -> Result<BTreeSet<String>, NativeError> {
    if absent(dir)? {
        return Ok(BTreeSet::new());
    }
    let mut names = BTreeSet::new();
    for e in fs::read_dir(dir)? {
        let name = e?.file_name().into_string().map_err(|_| recovery())?;
        if names.len() >= 10000 {
            return Err(limit());
        }
        names.insert(name);
    }
    Ok(names)
}

impl FontPlan {
    pub fn commit(
        &self,
        store: &Store,
        lock: &RootLock,
        expected: u64,
        id: &str,
        input: &JsonValue,
    ) -> Result<JsonValue, NativeError> {
        if let Some(result) = store.replay(id, input)? {
            return Ok(result);
        }
        crate::apply::ensure_idle(store)?;
        store::ensure_readable(&store.document()?)?;
        if input["params"]["operationId"] != id {
            return Err(NativeError::params());
        }
        store.locked(lock)?;
        let _game_lock = GameLock::acquire(&self.game().join("game/citadel"))?;
        let font_authority=fonts::FontAuthority::new(store,lock,&_game_lock)?;let authority=Some(&font_authority);
        ensure_game_idle()?;
        let control = PrepareControl::never_cancelled();
        self.revalidate(store, expected, &control)?;
        let changes = self.checked_changes()?;
        if changes.len() > 257 {
            return Err(limit());
        }
        let mut rows = JsonValue::new_array();
        let mut total = 0u64;
        for c in changes {
            for b in [c.before().bytes(), c.after().map(|b| b.as_ref())]
                .into_iter()
                .flatten()
            {
                if b.len() as u64 > max(c.target()) {
                    return Err(limit());
                }
                total = total
                    .checked_add(b.len() as u64)
                    .filter(|n| *n <= MAX_BYTES)
                    .ok_or_else(limit)?;
            }
            rows.push(object!{target:fonts::target_json(c.target()),before:image_intent(c.before().bytes(),match c.before(){CheckedFontImage::Present(f)=>f.mode(),_=>0})?,after:image_intent(c.after().map(|b|b.as_ref()),c.mode())?,expectedBefore:self.recorded_before(c.target()).map(fp_json).unwrap_or_else(||match c.before(){CheckedFontImage::Present(f)=>fp_json(f.identity()),_=>JsonValue::Null}),temp:format!(".native-font-{}",paths::random_id()?),tempCreated:JsonValue::Null,published:JsonValue::Null,writeIntent:false,rollbackTemp:format!(".native-font-{}",paths::random_id()?),rollbackCreated:JsonValue::Null,restored:JsonValue::Null,rollbackIntent:false}).map_err(|_|limit())?;
        }
        let mut requested = self.document().clone();
        let package_version = package::layout_version(&requested)?;
        let library_version = if self.document()["storeVersion"] == 4 { 4 } else { 3 };
        requested["storeVersion"] = library_version.into();
        requested["packageVersion"] = package_version.into();
        requested["fonts"] = FontState {
            installed: self.transaction_projection()?,
            previous: None,
        }
        .encode()?;
        let mut j = object! {kind:"gameFonts",version:if crate::deployment_metadata::retained_shards(self.document()) {4} else if library_version == 4 {3} else {2},libraryVersion:library_version,root:self.root().to_str(),game:self.game().to_str(),transactionId:self.transaction_id(),expectedRevision:expected.to_string(),input:input.clone(),beforeDocument:self.document().clone(),requestedDocument:requested,undo:self.is_undo(),targets:rows,phase:"staging",parents:JsonValue::from(self.directory_evidence().parents.iter().map(|(p,i)|object!{path:p.to_str(),identity:fonts::dir_json(self.recorded_parent(p,i))}).collect::<Vec<_>>()),names:self.directory_evidence().names.clone(),directoryBefore:self.recorded_directory().map(fonts::dir_json),directoryCreated:JsonValue::Null,directoryRestored:JsonValue::Null,directoryObserved:JsonValue::Null,createDirectory:self.creates_directory(),removeDirectory:self.removes_directory(),removeIntent:false,backupDirectory:JsonValue::Null};
        admit(store, &j)?;
        let mut outcomes=reserve_recovery_documents(&j)?;
        outcomes[0]["revision"]=expected.checked_add(1).ok_or_else(recovery)?.to_string().into();
        store::admit_success(id,input,&outcomes[0])?;
        store::admit_outcome(id,input,&store::failure_outcome(&rollback_error()))?;
        persist(store, lock, id, &j)?;
        let run = (|| {
            paths::private_dir(&crate::storage::path(store.root(), "font-backups")?)?;
            j["backupDirectory"] = fonts::dir_json(&fonts::directory_identity(
                &crate::storage::path(store.root(), "font-backups")?,
            )?);
            persist(store, lock, id, &j)?;
            for (i, c) in changes.iter().enumerate() {
                seal_blob(store, lock, id, &mut j, i, "before", c.before().bytes(), authority)?;
                seal_blob(
                    store,
                    lock,
                    id,
                    &mut j,
                    i,
                    "after",
                    c.after().map(|b| b.as_ref()),
                 authority)?;
            }
            j["requestedDocument"] = project(&j, false)?;
            admit(store, &j)?;
            persist(store, lock, id, &j)?;
            self.revalidate_evidence(&control)?;
            if store.document()? != *self.document() {
                return Err(recovery());
            }
            ensure_game_idle()?;
            verify_parents(&j, authority)?;
            for row in j["targets"].members() {
                let t = fonts::target_decode(&row["target"])?;
                for key in ["before", "after"] {
                    if image_ref(&row[key], &t)?.is_some()
                        && verify_blob(store.root(), &row[key], &t, authority)?.is_none()
                    {
                        return Err(recovery());
                    }
                }
            }
            j["phase"] = "writing".into();
            persist(store, lock, id, &j)?;
            let dir = directory(self.game());
            if self.creates_directory() {
                paths::verify_absent(&dir)?;
                fs::create_dir(&dir)?;
                // This identity pins an observed, borrowed directory only. It
                // never authorizes removal, including when another writer raced mkdir.
                j["directoryObserved"] = fonts::dir_json(&fonts::directory_identity(&dir)?);
                verify_parents(&j, authority)?;
                check_directory(&j, authority)?;
                if !inventory(&dir)?.is_empty() { return Err(recovery()); }
                persist(store, lock, id, &j)?;
                paths::sync_dir(dir.parent().unwrap())?;
            }
            for i in 0..changes.len() {
                publish(store, lock, id, &mut j, i, false, authority)?;
            }
            let written = written_map(&j, "published")?;
            let observed = if j["directoryObserved"].is_null() {
                None
            } else {
                Some(fonts::dir_decode(&j["directoryObserved"])?)
            };
            let installed = self.installed_after(observed.as_ref(), &written, &control)?;
            let mut doc = project(&j, false)?;
            let mut state = FontState::decode(&doc)?;
            state.installed = installed;
            fonts::retain_directories(&mut state);
            doc["fonts"] = state.encode()?;
            store::ensure_readable(&doc)?;
            j["requestedDocument"] = doc;
            j["phase"] = "ready".into();
            persist(store, lock, id, &j)?;
            finish(store, lock, id, &j, authority)
        })();
        run.map_err(|_|recovery())
    }
}

fn written_map(
    j: &JsonValue,
    key: &str,
) -> Result<BTreeMap<FontTarget, fonts::FontFingerprint>, NativeError> {
    let mut map = BTreeMap::new();
    for r in j["targets"].members() {
        if !r[key].is_null() {
            let t = fonts::target_decode(&r["target"])?;
            map.insert(t.clone(), fonts::fingerprint_decode(&r[key], max(&t))?);
        }
    }
    Ok(map)
}
fn live_before(path: &Path, row: &JsonValue, t: &FontTarget, authority: Option<&fonts::FontAuthority>) -> Result<bool, NativeError> {
    matches(path, &row["expectedBefore"], mode(&row["before"]), t, authority)
}
fn live_after(path: &Path, row: &JsonValue, t: &FontTarget, authority: Option<&fonts::FontAuthority>) -> Result<bool, NativeError> {
    if row["after"]["kind"] == "absent" {
        Ok(row["writeIntent"] == true && absent(path)?)
    } else if row["published"].is_null() {
        Ok(false)
    } else {
        matches(path, &row["published"], mode(&row["after"]), t, authority)
    }
}
fn live_restored(path: &Path, row: &JsonValue, t: &FontTarget, authority: Option<&fonts::FontAuthority>) -> Result<bool, NativeError> {
    if row["before"]["kind"] == "absent" {
        Ok(row["rollbackIntent"] == true && absent(path)?)
    } else if row["restored"].is_null() {
        Ok(false)
    } else {
        matches(path, &row["restored"], mode(&row["before"]), t, authority)
    }
}
fn check_directory(j: &JsonValue, authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    let dir = directory(Path::new(s(j, "game")?));
    let expected = if matches!(j["version"].as_u32(), Some(2 | 3 | 4)) && !j["directoryObserved"].is_null() {
        &j["directoryObserved"]
    } else {
        &j["directoryBefore"]
    };
    if absent(&dir)? {
        return if expected.is_null() { Ok(()) } else { Err(recovery()) };
    }
    if expected.is_null() || !fonts::directory_matches(&dir,&fonts::dir_decode(expected)?,authority)? {
        return Err(recovery());
    }
    Ok(())
}
fn publish(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &mut JsonValue,
    i: usize,
    rollback: bool,
 authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    verify_parents(j, authority)?;
    check_directory(j, authority)?;
    ensure_game_idle()?;
    let row = j["targets"][i].clone();
    let t = fonts::target_decode(&row["target"])?;
    let game = Path::new(s(j, "game")?);
    let path = fonts::target_path(game, &t)?;
    let dir = directory(game);
    if rollback && (live_before(&path, &row, &t, authority)? || live_restored(&path, &row, &t, authority)?) {
        return Ok(());
    }
    if !rollback && !live_before(&path, &row, &t, authority)? {
        return Err(recovery());
    }
    if rollback && !live_after(&path, &row, &t, authority)? {
        return Err(recovery());
    }
    let (image_key, temp_key, created_key, written_key, intent_key) = if rollback {
        (
            "before",
            "rollbackTemp",
            "rollbackCreated",
            "restored",
            "rollbackIntent",
        )
    } else {
        ("after", "temp", "tempCreated", "published", "writeIntent")
    };
    j["targets"][i][intent_key] = true.into();
    persist(store, lock, id, j)?;
    if image_ref(&row[image_key], &t)?.is_some() {
        let bytes = verify_blob(store.root(), &row[image_key], &t, authority)?.ok_or_else(recovery)?;
        let temp = dir.join(s(&row, temp_key)?);
        if !absent(&temp)? {
            check_temp(&dir, &row, rollback, authority)?;
            temp_removal(&dir,&row,rollback,authority)?.unlink_required()?;
            paths::sync_dir(&dir)?;
        }
        let mut f = write_new(&temp)?;
        j["targets"][i][created_key] = created_inode(&f, &temp)?;
        j["targets"][i][written_key] = JsonValue::Null;
        persist(store, lock, id, j)?;
        f.write_all(bytes.bytes())?;
        f.set_permissions(fs::Permissions::from_mode(mode(&row[image_key])))?;
        f.sync_all()?;
        paths::sync_dir(&dir)?;
        let actual = fonts::transaction_read(&temp, max(&t))?;
        check_created_image(&f, &temp, &j["targets"][i][created_key], &actual)?;
        if actual.bytes() != bytes.bytes() || actual.mode() != mode(&row[image_key]) {
            return Err(recovery());
        }
        j["targets"][i][written_key] = fp_json(actual.identity());
        persist(store, lock, id, j)?;
        verify_parents(j, authority)?;
        check_directory(j, authority)?;
        ensure_game_idle()?;
        check_inode(&temp, &j["targets"][i][created_key], authority)?;
        if !matches(
            &temp,
            &j["targets"][i][written_key],
            mode(&row[image_key]),
            &t,
         authority)? {
            return Err(recovery());
        }
        if rollback {
            if !live_after(&path, &row, &t, authority)? {
                return Err(recovery());
            }
        } else if !live_before(&path, &row, &t, authority)? {
            return Err(recovery());
        }
        let expected=&row[if rollback{"published"}else{"expectedBefore"}];
        paths::replace_admitted(&temp,&path,expected,expected["sha256"].as_str().map(str::to_owned))?;
        paths::sync_dir(&dir)?;
    } else {
        verify_parents(j, authority)?;
        check_directory(j, authority)?;
        ensure_game_idle()?;
        if rollback {
            if !live_after(&path, &row, &t, authority)? {
                return Err(recovery());
            }
        } else if !live_before(&path, &row, &t, authority)? {
            return Err(recovery());
        }
        if !absent(&path)? {
            let expected=&row[if rollback{"published"}else{"expectedBefore"}];
            paths::DestructiveFile::admitted(path.clone(),expected,expected["sha256"].as_str().map(str::to_owned))?.unlink_required()?;
            paths::sync_dir(&dir)?;
        }
    }
    persist(store, lock, id, j)
}
fn rebound_document(j: &JsonValue, rollback: bool) -> Result<JsonValue, NativeError> {
    let doc = if rollback {
        j["beforeDocument"].clone()
    } else {
        project(j, false)?
    };
    let map = written_map(j, if rollback { "restored" } else { "published" })?;
    let observed = if matches!(j["version"].as_u32(),Some(2|3|4)) && !j["directoryObserved"].is_null() {
        Some(fonts::dir_decode(&j["directoryObserved"])?)
    } else { None };
    rebind_document(doc,&map,observed.as_ref(),rollback,matches!(j["version"].as_u32(),Some(2|3|4)))
}

/// Shared by real completion and pre-execution reservation of the same full
/// document. Identity placeholders never become persisted ownership evidence.
fn rebind_document(mut doc: JsonValue, map: &BTreeMap<FontTarget,fonts::FontFingerprint>,
    observed: Option<&fonts::DirectoryIdentity>, rollback: bool, retain: bool) -> Result<JsonValue,NativeError> {
    let mut state = FontState::decode(&doc)?;
    // A rollback can restore an unchanged historical checkpoint file to a new
    // inode. Rebind that same logical image in both current and prior footprints.
    let old = state.installed.clone();
    if let Some(f) = &mut state.installed {
        for file in &mut f.files {
            if let Some(fp) = map.get(&FontTarget::Font(file.target_name.clone())) {
                file.identity = fp.clone();
            }
        }
        if let Some(fp) = map.get(&FontTarget::Configuration) {
            f.conf = Some(fp.clone());
        }
        let identity = observed.cloned().or_else(||f.directory.clone());
        f.directory = identity;
    }
    if rollback {
        if let (Some(prior), Some(old), Some(current)) = (
            state.previous.as_mut().and_then(|p| p.prior.as_mut()),
            old.as_ref(),
            state.installed.as_ref(),
        ) {
            for file in &mut prior.files {
                if old
                    .files
                    .iter()
                    .any(|o| o.target_name == file.target_name && o.identity == file.identity)
                {
                    if let Some(fp) = map.get(&FontTarget::Font(file.target_name.clone())) {
                        file.identity = fp.clone()
                    }
                }
            }
            if prior.conf == old.conf {
                prior.conf = current.conf.clone();
            }
            if prior.directory == old.directory {
                prior.directory = current.directory.clone();
            }
        }
    }
    if matches!(doc["storeVersion"].as_u32(), Some(3 | 4)) {
        if retain { fonts::retain_directories(&mut state); }
        doc["fonts"] = state.encode()?;
    }
    store::ensure_readable(&doc)?;
    Ok(doc)
}

fn widest_fingerprint(r: &FileRef) -> fonts::FontFingerprint {
    fonts::FontFingerprint { fsid:Some("ffffffffffffffff".into()),sha256:r.sha256.clone(),size:r.size,device:u64::MAX,inode:u64::MAX,
        modified_seconds:i64::MIN,modified_nanos:999_999_999}
}

/// Complete reachable roots, including rollback's rebinding of matching prior
/// footprints, and the actual late-field journal shape. No surrogate checkpoint.
pub(crate) fn reserve_recovery_documents(j: &JsonValue) -> Result<Vec<JsonValue>,NativeError> {
    let mut reserve=j.clone();
    let identity=object!{device:u64::MAX,inode:u64::MAX,fsid:"ffffffffffffffff"};
    let mut forward=BTreeMap::new(); let mut rollback=BTreeMap::new();
    for row in reserve["targets"].members_mut() {
        let t=fonts::target_decode(&row["target"])?;
        for (key,written,created) in [("before","restored","rollbackCreated"),("after","published","tempCreated")] {
            if let Some(r)=image_ref(&row[key],&t)? {
                let fp=widest_fingerprint(&r);
                row[key]["created"]=identity.clone(); row[key]["identity"]=fp_json(&fp);
                row[written]=fp_json(&fp); row[created]=identity.clone();
                if key=="before" {rollback.insert(t.clone(),fp);} else {forward.insert(t.clone(),fp);}
            }
        }
        // false is the widest serialized boolean.
        row["writeIntent"]=false.into(); row["rollbackIntent"]=false.into();
    }
    reserve["backupDirectory"]=identity.clone();
    reserve["phase"]="rollback".into();
    let observed=if matches!(j["version"].as_u32(),Some(2|3|4)) && j["createDirectory"]==true {
        reserve["directoryObserved"]=identity;
        Some(fonts::dir_decode(&reserve["directoryObserved"])?)
    } else if matches!(j["version"].as_u32(),Some(2|3|4)) && !j["directoryObserved"].is_null() {
        Some(fonts::dir_decode(&j["directoryObserved"])?)
    } else {None};
    let retain=matches!(j["version"].as_u32(),Some(2|3|4));
    let next=rebind_document(project(&reserve,true)?,&forward,observed.as_ref(),false,retain)?;
    let restored=rebind_document(j["beforeDocument"].clone(),&rollback,observed.as_ref(),true,retain)?;
    reserve["requestedDocument"]=next.clone();
    metadata_limit(&reserve)?;
    store::ensure_readable(&next)?; store::ensure_readable(&restored)?;
    Ok(vec![next,restored])
}

pub(crate) fn rollback_error() -> NativeError {
    NativeError::new("operationInterrupted","Interrupted font transaction was rolled back to its exact previous bytes and modes.")
}
#[cfg(test)]
thread_local! {pub(crate) static BEFORE_DATABASE:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
#[cfg(test)]
thread_local! {pub(crate) static AFTER_DATABASE:std::cell::RefCell<Option<Box<dyn FnOnce()>>>=const {std::cell::RefCell::new(None)};}
fn finish(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &JsonValue,
 authority: Option<&fonts::FontAuthority>) -> Result<JsonValue, NativeError> {
    verify_parents(j, authority)?;
    check_directory(j, authority)?;
    ensure_game_idle()?;
    let dir = directory(Path::new(s(j, "game")?));
    if j["createDirectory"] == true && (j["directoryObserved"].is_null() || absent(&dir)?)
    {
        return Err(recovery());
    }
    let mut expected_names = j["names"]
        .members()
        .map(|n| n.as_str().map(str::to_owned).ok_or_else(recovery))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for row in j["targets"].members() {
        let t = fonts::target_decode(&row["target"])?;
        let p = fonts::target_path(Path::new(s(j, "game")?), &t)?;
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(recovery)?;
        if row["after"]["kind"] == "absent" {
            expected_names.remove(name);
        } else {
            expected_names.insert(name.into());
        }
    }
    if inventory(&dir)? != expected_names {
        return Err(recovery());
    }
    let doc = rebound_document(j, false)?;
    fonts::verify_current_with(store,&doc,authority)?;
    // Even undo to an unobserved footprint must validate every journal target.
    for row in j["targets"].members() {
        let t = fonts::target_decode(&row["target"])?;
        if !live_after(&fonts::target_path(Path::new(s(j, "game")?), &t)?, row, &t, authority)? {
            return Err(recovery());
        }
    }
    #[cfg(test)]
    BEFORE_DATABASE.with(|h|if let Some(h)=h.borrow_mut().take(){h();});
    let result = store.commit(
        lock,
        store::revision(s(j, "expectedRevision")?)?,
        id,
        &j["input"],
        doc,
        None,
    )?;
    #[cfg(test)]
    AFTER_DATABASE.with(|h| {if let Some(h)=h.borrow_mut().take(){h();}});
    cleanup_temps(j, authority)?;
    store.clear_journal(lock, id)?;
    Ok(result)
}
fn cleanup_temps(j: &JsonValue, authority: Option<&fonts::FontAuthority>) -> Result<(), NativeError> {
    let dir = directory(Path::new(s(j, "game")?));
    verify_parents(j, authority)?;
    check_directory(j, authority)?;
    // Complete cleanup-set preflight before its first unlink; read errors reject.
    for row in j["targets"].members() {
        check_temp(&dir, row, false, authority)?;
        check_temp(&dir, row, true, authority)?;
    }
    for row in j["targets"].members() {
        for key in ["temp", "rollbackTemp"] {
            let p = dir.join(s(row, key)?);
            if !absent(&p)? {
                check_temp(&dir,row,key=="rollbackTemp",authority)?;
                temp_removal(&dir,row,key=="rollbackTemp",authority)?.unlink()?;
                paths::sync_dir(&dir)?;
            }
        }
    }
    Ok(())
}

pub(crate) struct FontRecovery {
    id: String,
    journal: JsonValue,
    forward: bool,
    committed: bool,
}
impl FontRecovery {
    pub(crate) fn reserve_terminal(&self) -> Result<JsonValue,NativeError> {
        let mut docs=reserve_recovery_documents(&self.journal)?;
        let doc=if self.committed || self.forward {
            let mut next=docs.remove(0);
            next["revision"]=store::revision(s(&self.journal,"expectedRevision")?)?.checked_add(1).ok_or_else(recovery)?.to_string().into();
            store::admit_success(&self.id,&self.journal["input"],&next)?;
            next
        } else {
            store::admit_outcome(&self.id,&self.journal["input"],&store::failure_outcome(&rollback_error()))?;
            docs.remove(1)
        };
        Ok(doc)
    }
    pub(crate) fn game_citadel(&self)->Result<PathBuf,NativeError>{Ok(Path::new(s(&self.journal,"game")?).join("game/citadel"))}
    pub(crate) fn execute(mut self, store: &Store, lock: &RootLock,game:&GameLock) -> Result<(), NativeError> {
        let font_authority=fonts::FontAuthority::new(store,lock,game)?;let authority=Some(&font_authority);
        self.reserve_terminal()?;
        if self.committed || self.forward {
            finish(store, lock, &self.id, &self.journal, authority)?;
            return Ok(());
        }
        self.journal["phase"] = "rollback".into();
        persist(store, lock, &self.id, &self.journal)?;
        let dir = directory(Path::new(s(&self.journal, "game")?));
        for i in (0..self.journal["targets"].len()).rev() {
            publish(store, lock, &self.id, &mut self.journal, i, true, authority)?;
        }
        cleanup_temps(&self.journal, authority)?;
        verify_parents(&self.journal, authority)?;
        check_directory(&self.journal, authority)?;
        ensure_game_idle()?;
        let original_names = self.journal["names"]
            .members()
            .map(|n| n.as_str().map(str::to_owned).ok_or_else(recovery))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if inventory(&dir)? != original_names {
            return Err(recovery());
        }
        let doc = rebound_document(&self.journal, true)?;
        fonts::verify_current_with(store,&doc,authority)?;
        for row in self.journal["targets"].members() {
            let t = fonts::target_decode(&row["target"])?;
            let p = fonts::target_path(Path::new(s(&self.journal, "game")?), &t)?;
            if !live_before(&p, row, &t, authority)? && !live_restored(&p, row, &t, authority)? {
                return Err(recovery());
            }
        }
        store.complete_font_rollback(
            lock,
            &self.id,
            &self.journal["input"],
            &self.journal["beforeDocument"],
            &doc,
        )?;
        Ok(())
    }
}

/// Complete-set filesystem admission. This returns plans only; no journal,
/// private cleanup, outcome or unlink occurs until every caller preflight passes.
pub(crate) fn preflight(
    store: &Store,
    journals: &JsonValue,
    claims: &mut HashSet<PathBuf>,
    lock:&RootLock,guards:&[GameLock],
) -> Result<Vec<FontRecovery>, NativeError> {
    admit_set(store, journals)?;
    if journals.members().any(|e|crate::remote::recovery::is_remote_kind(&e["body"]["kind"])){return Err(recovery());}
    let mut plans = Vec::new();
    let current = store.document()?;
    let mut global_ids = document_refs(&current)?;
    for e in journals
        .members()
        .filter(|e| e["body"]["kind"] == "gameFonts")
    {
        let j = &e["body"];
        admit(store, j)?;
        let game=Path::new(s(j,"game")?);let c=game.join("game/citadel");
        let guard=guards.iter().find(|g|g.validate(&c).is_ok()).ok_or_else(recovery)?;
        let font_authority=fonts::FontAuthority::new(store,lock,guard)?;let authority=Some(&font_authority);
        verify_parents(j, authority)?;
        check_directory(j, authority)?;
        let id = s(e, "id")?;
        let game = Path::new(s(j, "game")?);
        let dir = directory(game);
        if !claims.insert(dir.clone()) {
            return Err(recovery());
        }
        let status = store.operation(id)?;
        if status["status"] == "failed" {
            return Err(recovery());
        }
        let succeeded = status["status"] == "succeeded";
        if succeeded {
            store.replay(id, &j["input"])?.ok_or_else(recovery)?;
            let mut expected = rebound_document(j, false)?;
            expected["revision"] = store::revision(s(j, "expectedRevision")?)?
                .checked_add(1)
                .ok_or_else(recovery)?
                .to_string()
                .into();
            if current != expected {
                return Err(recovery());
            }
        } else if current != j["beforeDocument"] {
            return Err(recovery());
        }
        let mut all_after = j["phase"] != "staging"
            && j["phase"] != "rollback"
            && (j["createDirectory"] != true || !j["directoryObserved"].is_null() && !absent(&dir)?);
        let mut names = inventory(&dir)?;
        let mut expected_names = j["names"]
            .members()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(recovery))
            .collect::<Result<BTreeSet<_>, _>>()?;
        for row in j["targets"].members() {
            let t = fonts::target_decode(&row["target"])?;
            let path = fonts::target_path(game, &t)?;
            for p in [
                &path,
                &dir.join(s(row, "temp")?),
                &dir.join(s(row, "rollbackTemp")?),
            ] {
                if !claims.insert(p.clone()) {
                    return Err(recovery());
                }
            }
            for key in ["before", "after"] {
                if let Some(r) = image_ref(&row[key], &t)? {
                    // Current committed checkpoint may reference this same before blob.
                    if let Some(old) = global_ids.get(&r.id) {
                        if !succeeded || key != "before" || old != &(r.clone(), "fontBackup") {
                            return Err(recovery());
                        }
                    } else {
                        global_ids.insert(r.id.clone(), (r.clone(), "fontBackup"));
                    }
                    if !claims.insert(fonts::backup_path(store.root(), &r.id)?) {
                        return Err(recovery());
                    }
                    verify_blob(store.root(), &row[key], &t, authority)?;
                }
            }
            check_temp(&dir, row, false, authority)?;
            check_temp(&dir, row, true, authority)?;
            let before = live_before(&path, row, &t, authority)?;
            let after = live_after(&path, row, &t, authority)?;
            let restored = live_restored(&path, row, &t, authority)?;
            if !before && !after && !restored {
                return Err(recovery());
            }
            all_after &= after;
            if !before
                && !restored
                && row["before"]["kind"] == "present"
                && row["before"]["identity"].is_null()
            {
                return Err(recovery());
            }
            if after && row["after"]["kind"] == "present" && row["after"]["identity"].is_null() {
                return Err(recovery());
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(recovery)?;
            names.remove(name);
            expected_names.remove(name);
            names.remove(s(row, "temp")?);
            names.remove(s(row, "rollbackTemp")?);
        }
        if names != expected_names {
            return Err(recovery());
        }
        // Check every unaffected current target, including borrowed files. No
        // summary/history path can extend the set of mutation targets.
        let state = FontState::decode(&j["beforeDocument"])?;
        if let Some(f) = state.installed {
            let targets = j["targets"]
                .members()
                .map(|r| fonts::target_decode(&r["target"]))
                .collect::<Result<BTreeSet<_>, _>>()?;
            for file in f.files {
                let t = FontTarget::Font(file.target_name);
                if !targets.contains(&t)
                    && !matches(
                        &fonts::target_path(game, &t)?,
                        &fp_json(&file.identity),
                        file.mode,
                        &t,
                     authority)?
                {
                    return Err(recovery());
                }
            }
            if !targets.contains(&FontTarget::Configuration)
                && !matches(
                    &fonts::target_path(game, &FontTarget::Configuration)?,
                    &f.conf.as_ref().map(fp_json).unwrap_or(JsonValue::Null),
                    f.conf_mode.unwrap_or(0),
                    &FontTarget::Configuration,
                 authority)?
            {
                return Err(recovery());
            }
        }
        if all_after {
            let doc = rebound_document(j, false)?;
            fonts::verify_current_with(store,&doc,authority)?;
        }
        if succeeded && !all_after {
            return Err(recovery());
        }
        plans.push(FontRecovery {
            id: id.into(),
            journal: j.clone(),
            forward: all_after,
            committed: succeeded,
        });
    }
    // The central writer cannot legitimately produce concurrent font changes.
    if plans.len() > 1 {
        return Err(recovery());
    }
    Ok(plans)
}

pub(crate) fn state_digest(doc: &JsonValue) -> Result<String, NativeError> {
    let state = FontState::decode(doc)?.encode()?;
    metadata_digest(store::canonical(&state).as_bytes())
}

// Canonical font state is bounded metadata already in memory, and journal
// admission hashes it twice per durable step. Hash it with the vendored
// SHA-256 instead of a sha256sum child. The digest is the same lowercase
// hex, and the bound and its error match digest_bounded_controlled.
fn metadata_digest(bytes: &[u8]) -> Result<String, NativeError> {
    if bytes.len() > MAX_METADATA {
        return Err(NativeError::new(
            "nativeIo",
            "Cannot fingerprint payload; invalid or excessive sha256sum output.",
        ));
    }
    paths::digest_bytes(bytes)
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn item93_review_r1_legacy_temp_requires_recorded_bytes_before_unlink(){
        use crate::test_support::{Fixture,import};
        for proof in ["different","missing","matching"] {
            let f=Fixture::new();import(&mut crate::native::NativeService::new(),&f);
            let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let c=f.game.join("game/citadel");
            if !paths::test_volatile_filesystem(&File::open(&c).unwrap()){continue;}
            let dir=directory(&f.game);fs::create_dir_all(&dir).unwrap();let temp=dir.join("legacy-temp");let bytes=b"foreign font bytes";fs::write(&temp,bytes).unwrap();
            let m=fs::metadata(&temp).unwrap();let created=object!{device:m.dev()+1,inode:m.ino()};
            let image=if proof=="missing"{object!{kind:"absent"}}else{image_intent(Some(if proof=="matching"{bytes}else{b"recorded app bytes"}),0o644).unwrap()};
            let j=object!{version:4,game:f.game.to_str(),parents:json::array![],directoryObserved:fonts::dir_json(&fonts::directory_identity(&dir).unwrap()),targets:json::array![object!{target:object!{kind:"font",name:"demo.ttf"},temp:"legacy-temp",tempCreated:created.clone(),published:JsonValue::Null,after:image.clone(),rollbackTemp:"rollback-temp"}]};
            let guard=GameLock::acquire(&c).unwrap();let authority=fonts::FontAuthority::new(&st,&lock,&guard).unwrap();
            let result=cleanup_temps(&j,Some(&authority));
            if proof=="matching"{result.unwrap();assert!(!temp.exists());}else{result.unwrap();assert_eq!(fs::read(&temp).unwrap(),bytes,"{proof}: legacy inode alone must never authorize unlink");}
            assert!(st.journals().unwrap().is_empty());
        }
    }

    // The sha256sum path that state_digest used before; other callers keep it.
    fn reference_digest(bytes: &[u8]) -> Result<String, NativeError> {
        prepare_control::digest_bounded_controlled(bytes, MAX_METADATA, &PrepareControl::never_cancelled())
    }

    #[test]
    fn state_digest_matches_sha256sum_reference() {
        use crate::fonts::{directory_identity, FontFootprint};
        use crate::test_support::{import, Fixture};
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        import(&mut crate::native::NativeService::new(), &f);
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        // Same valid state shape as the storage-move font fixture: real
        // directory identities and a read-only backup blob.
        let dir = f.game.join("game/citadel/panorama/fonts");
        fs::create_dir_all(&dir).unwrap();
        paths::private_dir(&f.data.join("font-backups")).unwrap();
        let id = "abcdefabcdefabcdefabcdefabcdefab";
        let blob = f.data.join(format!("font-backups/{id}.blob"));
        fs::write(&blob, b"<fontconfig></fontconfig>\n").unwrap();
        fs::set_permissions(&blob, fs::Permissions::from_mode(0o400)).unwrap();
        let fp = paths::fingerprint(&blob).unwrap();
        let current = FontFootprint { root_path: f.data.clone(), root_identity: directory_identity(&f.data).unwrap(), game_path: f.game.clone(),
            game_identity: directory_identity(&f.game).unwrap(), directory: Some(directory_identity(&dir).unwrap()), directory_created: false,
            files: vec![], conf: None, conf_mode: None, owned_block: None };
        let mut prior = current.clone();
        prior.conf = Some(fp.clone().into());
        prior.conf_mode = Some(0o644);
        let checkpoint = FontCheckpoint { transaction_id: "1234567890abcdef1234567890abcdef".into(), prior: Some(prior),
            images: vec![FontImage { target: FontTarget::Configuration, image: StoredFontImage::Present {
                backup: FileRef { id: id.into(), sha256: fp.sha256.clone(), size: fp.size, source_name: "fonts.conf".into() },
                identity: fp.into(), mode: 0o644 } }] };
        // An imported library before font state existed decodes as empty.
        let legacy = st.document().unwrap();
        let mut base = legacy.clone();
        base["packageVersion"] = package::layout_version(&base).unwrap().into();
        base["storeVersion"] = 4.into();
        base["fonts"] = FontState::default().encode().unwrap();
        let mut installed = base.clone();
        installed["fonts"] = FontState { installed: Some(current.clone()), previous: None }.encode().unwrap();
        let mut previous = base.clone();
        previous["fonts"] = FontState { installed: Some(current), previous: Some(checkpoint) }.encode().unwrap();
        let mut cases = Vec::new();
        for (name, doc) in [("legacy", &legacy), ("none", &base), ("installed", &installed), ("previous", &previous)] {
            let state = FontState::decode(doc).unwrap();
            // Record the shape actually hashed; a missing case is a failure, not a skip.
            assert_eq!((state.installed.is_some(), state.previous.is_some()),
                match name { "legacy" | "none" => (false, false), "installed" => (true, false), _ => (true, true) }, "{name}");
            let canonical = store::canonical(&state.encode().unwrap());
            let digest = state_digest(doc).unwrap();
            assert_eq!(digest, reference_digest(canonical.as_bytes()).unwrap(), "{name}");
            assert!(digest.len() == 64 && digest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "{name}");
            cases.push(name);
        }
        assert_eq!(cases, ["legacy", "none", "installed", "previous"]);
        assert_ne!(state_digest(&installed).unwrap(), state_digest(&previous).unwrap());
        let mut raw: Vec<Vec<u8>> = vec![Vec::new(), b"abc".to_vec(), "Font \u{e9}\u{4e2d}\u{1f600}".as_bytes().to_vec(), vec![0xa5; 4096]];
        raw.push((0..MAX_METADATA - 1).map(|i| (i * 31 % 251) as u8).collect());
        raw.push((0..MAX_METADATA).map(|i| (i * 17 % 253) as u8).collect());
        for bytes in &raw {
            assert_eq!(metadata_digest(bytes).unwrap(), reference_digest(bytes).unwrap(), "{} bytes", bytes.len());
        }
        // Standard SHA-256 known answers.
        assert_eq!(metadata_digest(b"").unwrap(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(metadata_digest(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        eprintln!("STATE_DIGEST_CASES {} RAW_SIZES {:?}", cases.join(","), raw.iter().map(Vec::len).collect::<Vec<_>>());
    }

    #[test]
    fn metadata_digest_enforces_bound() {
        let edge = vec![b'x'; MAX_METADATA];
        assert_eq!(metadata_digest(&edge).unwrap(), reference_digest(&edge).unwrap());
        let over = vec![b'x'; MAX_METADATA + 1];
        let new = metadata_digest(&over).unwrap_err();
        let old = reference_digest(&over).unwrap_err();
        assert_eq!((new.kind.as_str(), new.message.as_str()), (old.kind.as_str(), old.message.as_str()));
        assert_eq!(new.kind, "nativeIo");
    }

    #[test]
    fn shared_payload_names_are_metadata_but_bytes_and_roles_are_evidence() {
        let first = FileRef { id: "a".repeat(32), sha256: "b".repeat(64), size: 7, source_name: "skin_000.vpk".into() };
        let mut refs = BTreeMap::new();
        insert_ref(&mut refs, first.clone(), "payload").unwrap();
        let mut alias = first.clone();
        alias.source_name = "other_000.vpk".into();
        insert_ref(&mut refs, alias, "payload").unwrap();
        let mut changed_hash = first.clone(); changed_hash.sha256 = "c".repeat(64);
        let mut changed_size = first.clone(); changed_size.size += 1;
        for (changed, role) in [(changed_hash, "payload"), (changed_size, "payload"), (first.clone(), "fontBackup")] {
            let mut isolated = BTreeMap::new();
            insert_ref(&mut isolated, first.clone(), "payload").unwrap();
            assert!(insert_ref(&mut isolated, changed, role).is_err());
        }
        let mut backups = BTreeMap::new();
        insert_ref(&mut backups, first.clone(), "fontBackup").unwrap();
        let mut renamed_backup = first; renamed_backup.source_name = "other.blob".into();
        assert!(insert_ref(&mut backups, renamed_backup, "fontBackup").is_err());
    }
}
