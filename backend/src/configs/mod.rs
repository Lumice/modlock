//! Reviewed independent game configuration writes. Upstream 92bebec, GPL-3.0.
mod hero;
#[cfg(test)]
mod tests;
use crate::{
    game_access::{ensure_game_idle, GameLock},
    native::NativeError,
    paths,
    store::{RootLock, Store},
};
use json::{object, JsonValue};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
fn bad() -> NativeError {
    NativeError::new(
        "configInvalid",
        "Invalid, changed or unsupported configuration; refresh and review again.",
    )
}
pub const HEROES: &[&str] = &[
    "Default",
    "Abrams",
    "Apollo",
    "Baba",
    "Bebop",
    "Billy",
    "Calico",
    "Celeste",
    "Deadman Danny",
    "Doorman",
    "Drifter",
    "Dynamo",
    "Graves",
    "Grey Talon",
    "Haze",
    "Holliday",
    "Infernus",
    "Ivy",
    "Kelvin",
    "Lady Geist",
    "Lash",
    "McGinnis",
    "Mina",
    "Mirage",
    "Mo & Krill",
    "Nurse Harrow",
    "Paige",
    "Paradox",
    "Pocket",
    "Rat King",
    "Rem",
    "Seven",
    "Shiv",
    "Silver",
    "Sinclair",
    "Solomon",
    "Venator",
    "Victor",
    "Vindicta",
    "Violet",
    "Viscous",
    "Vyper",
    "Warden",
    "Wraith",
    "Wrecker",
    "Yamato",
];
const LIMIT: u64 = 1024 * 1024;
const MARKERS: [(&str, &str); 2] = [
    (
        "// === Deadlock Mod Manager - Crosshair Settings (DO NOT EDIT) ===",
        "// === End Crosshair Settings ===",
    ),
    (
        "// === Deadlock Mod Manager - Map Command (DO NOT EDIT) ===",
        "// === End Map Command ===",
    ),
];
pub fn split(s: &str) -> Result<(String, Vec<JsonValue>), NativeError> {
    let mut spans = Vec::new();
    for (start, end) in MARKERS {
        let a = s.match_indices(start).collect::<Vec<_>>();
        let b = s.match_indices(end).collect::<Vec<_>>();
        if a.len() != b.len() || a.len() > 1 {
            return Err(bad());
        }
        if let (Some(a), Some(b)) = (a.first(), b.first()) {
            if b.0 < a.0 {
                return Err(bad());
            }
            spans.push((a.0, b.0 + end.len()));
        }
    }
    spans.sort();
    if spans.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err(bad());
    }
    let mut free = s.to_string();
    let sections = spans
        .iter()
        .map(|(a, b)| object! {content:s[*a..*b].to_string(),start:*a,end:*b,readOnly:true})
        .collect();
    for (a, b) in spans.into_iter().rev() {
        free.replace_range(a..b, "");
    }
    Ok((free, sections))
}
fn merge(original: &str, free: &str) -> Result<String, NativeError> {
    if MARKERS
        .iter()
        .any(|(a, b)| free.contains(a) || free.contains(b))
    {
        return Err(bad());
    }
    let (old, sections) = split(original)?;
    if old == free {
        return Ok(original.into());
    }
    let mut s = free.to_string();
    for section in sections {
        if !s.is_empty() && !s.ends_with('\n') {
            s.push_str(if original.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            });
        }
        s.push_str(section["content"].as_str().ok_or_else(bad)?);
    }
    Ok(s)
}
pub fn defaults() -> JsonValue {
    object! {gap:0,width:4,height:16,pipOpacity:1,dotOpacity:1,dotOutlineOpacity:1,
    color:object!{r:255,g:255,b:255}, outlineColor:object!{r:0,g:0,b:0},
    pipBorder:false,pipGapStatic:false,hero:"Default",dotSize:2,dotOutlineWidth:0,
    dotOutlineGap:0,pipOutlineWidth:0,pipOutlineGap:0,pipOutlineOpacity:1,
    gapCollapse:false,themed:false,overrides:object!{}}
}
pub fn ranges() -> JsonValue {
    let mut r = object! {};
    for (k, min, max, step) in [
        ("gap", -40., 50., 1.),
        ("width", 0., 100., 0.1),
        ("height", 0., 100., 0.1),
        ("pipOpacity", 0., 1., 0.01),
        ("dotOpacity", 0., 1., 0.01),
        ("dotOutlineOpacity", 0., 1., 0.01),
        ("pipOutlineOpacity", 0., 1., 0.01),
        ("dotSize", 1., 100., 1.),
        ("dotOutlineWidth", 0., 10., 1.),
        ("dotOutlineGap", 0., 100., 1.),
        ("pipOutlineWidth", 0., 10., 1.),
        ("pipOutlineGap", 0., 100., 1.),
        ("color.r", 0., 255., 1.),
        ("color.g", 0., 255., 1.),
        ("color.b", 0., 255., 1.),
        ("outlineColor.r", 0., 255., 1.),
        ("outlineColor.g", 0., 255., 1.),
        ("outlineColor.b", 0., 255., 1.),
    ] {
        r[k] = object! {min:min,max:max,step:step};
    }
    r
}
fn field<'a>(v: &'a JsonValue, key: &str) -> &'a JsonValue {
    if let Some((group, channel)) = key.split_once('.') {
        &v[group][channel]
    } else {
        &v[key]
    }
}
fn put(v: &mut JsonValue, key: &str, value: JsonValue) {
    if let Some((group, channel)) = key.split_once('.') {
        v[group][channel] = value;
    } else {
        v[key] = value;
    }
}
pub fn validate(v: &JsonValue) -> Result<(), NativeError> {
    if !v.is_object() || v.entries().any(|(k, _)| !defaults().has_key(k)) {
        return Err(bad());
    }
    for (k, r) in ranges().entries() {
        let n = field(v, k).as_f64().ok_or_else(bad)?;
        let step = r["step"].as_f64().unwrap();
        if !n.is_finite()
            || n < r["min"].as_f64().unwrap()
            || n > r["max"].as_f64().unwrap()
            || (n / step - (n / step).round()).abs() > 1e-6
        {
            return Err(bad());
        }
    }
    if v["color"].len() != 3 || v["outlineColor"].len() != 3 {
        return Err(bad());
    }
    for k in ["pipBorder", "pipGapStatic", "gapCollapse", "themed"] {
        if v[k].as_bool().is_none() {
            return Err(bad());
        }
    }
    if !v["hero"].as_str().is_some_and(|h| HEROES.contains(&h)) || !v["overrides"].is_object() {
        return Err(bad());
    }
    for (name, values) in v["overrides"].entries() {
        if !name.starts_with("hero_")
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !values.is_object()
        {
            return Err(bad());
        }
        let mut effective = defaults();
        for (k, value) in values.entries() {
            if matches!(k, "hero" | "overrides") || !effective.has_key(k) {
                return Err(bad());
            }
            // Colour overrides may contain only one channel; merge each independently.
            if matches!(k, "color" | "outlineColor") {
                for (c, n) in value.entries() {
                    effective[k][c] = n.clone();
                }
            } else {
                effective[k] = value.clone();
            }
        }
        validate(&effective)?;
    }
    Ok(())
}
pub fn encode(v: &JsonValue) -> Result<String, NativeError> {
    validate(v)?;
    let mut parts = vec![];
    for k in [
        "gap",
        "width",
        "height",
        "pipOpacity",
        "pipBorder",
        "pipGapStatic",
        "dotOpacity",
        "dotOutlineOpacity",
    ] {
        parts.push(if k == "pipBorder" {
            JsonValue::from(v["pipOutlineWidth"].as_f64().unwrap() > 0.).dump()
        } else {
            v[k].dump()
        });
    }
    for k in ["r", "g", "b"] {
        parts.push(v["color"][k].dump())
    }
    parts.push(
        v["hero"]
            .as_str()
            .unwrap()
            .replace(' ', "%20")
            .replace('&', "%26"),
    );
    Ok(parts.join(";"))
}
pub fn decode(s: &str) -> Result<JsonValue, NativeError> {
    let s = s
        .split("edit=")
        .nth(1)
        .unwrap_or(s)
        .split('&')
        .next()
        .unwrap_or(s);
    let parts = s.split(';').collect::<Vec<_>>();
    if parts.len() != 12 {
        return Err(bad());
    }
    let mut v = defaults();
    for (i, k) in [
        "gap",
        "width",
        "height",
        "pipOpacity",
        "pipBorder",
        "pipGapStatic",
        "dotOpacity",
        "dotOutlineOpacity",
    ]
    .iter()
    .enumerate()
    {
        v[*k] = json::parse(parts[i]).map_err(|_| bad())?;
    }
    for (i, k) in ["r", "g", "b"].iter().enumerate() {
        v["color"][*k] = json::parse(parts[i + 8]).map_err(|_| bad())?;
    }
    v["hero"] = parts[11].replace("%20", " ").replace("%26", "&").into();
    v["pipOutlineWidth"] = if v["pipBorder"] == true { 1 } else { 0 }.into();
    validate(&v)?;
    Ok(v)
}
const KEYS: &[(&str, &str)] = &[
    ("color.r", "crosshair_color_r"),
    ("color.g", "crosshair_color_g"),
    ("color.b", "crosshair_color_b"),
    ("pipOutlineWidth", "crosshair_pip_outline_border"),
    ("pipOutlineGap", "crosshair_pip_outline_gap"),
    ("pipOutlineOpacity", "crosshair_pip_outline_opacity"),
    ("dotSize", "crosshair_dot_size"),
    ("dotOutlineWidth", "crosshair_dot_outline_border"),
    ("dotOutlineGap", "crosshair_dot_outline_gap"),
    ("outlineColor.r", "crosshair_outline_color_r"),
    ("outlineColor.g", "crosshair_outline_color_g"),
    ("outlineColor.b", "crosshair_outline_color_b"),
    ("gapCollapse", "crosshair_pip_gap_collapse"),
    ("themed", "crosshair_themed"),
    ("pipGapStatic", "crosshair_pip_gap_static"),
    ("pipOpacity", "crosshair_pip_opacity"),
    ("width", "crosshair_pip_width"),
    ("height", "crosshair_pip_height"),
    ("gap", "crosshair_pip_gap"),
    ("dotOpacity", "crosshair_dot_opacity"),
    ("dotOutlineOpacity", "crosshair_dot_outline_opacity"),
];
fn read_scope(settings: &hero::HeroSettings<'_>, v: &mut JsonValue) -> Result<(), NativeError> {
    for &(k, key) in KEYS {
        if let Some(raw) = settings.get(key)? {
            put(v, k, json::parse(raw.trim_matches('"')).map_err(|_| bad())?);
        }
    }
    Ok(())
}
fn read_values(s: &str) -> Result<JsonValue, NativeError> {
    let settings = hero::HeroSettings::parse(s)?;
    let mut v = defaults();
    read_scope(&settings, &mut v)?;
    for name in settings.hero_names() {
        let mut values = object! {};
        read_scope(&settings.scope(&name)?, &mut values)?;
        if values.len() > 0 {
            v["overrides"][&name] = values;
        }
    }
    validate(&v)?;
    Ok(v)
}
fn scope_changes(v: &JsonValue, sparse: bool) -> BTreeMap<String, Option<String>> {
    KEYS.iter()
        .map(|&(k, key)| {
            let value = field(v, k);
            (
                key.into(),
                if sparse && value.is_null() {
                    None
                } else {
                    Some(format!("\"{}\"", value.dump()))
                },
            )
        })
        .collect()
}
fn update_values(s: &str, v: &JsonValue) -> Result<String, NativeError> {
    validate(v)?;
    let settings = hero::HeroSettings::parse(s)?;
    let old_names = settings.hero_names();
    let mut result = settings.update(&scope_changes(v, false))?;
    let mut names = old_names;
    for (name, _) in v["overrides"].entries() {
        if !names.iter().any(|n| n == name) {
            names.push(name.into());
        }
    }
    for name in names {
        result = hero::HeroSettings::parse(&result)?
            .update_hero(&name, &scope_changes(&v["overrides"][&name], true))?;
    }
    Ok(result)
}
fn filename(kind: &str) -> Result<&'static str, NativeError> {
    match kind {
        "autoexec" => Ok("autoexec.cfg"),
        "crosshair" => Ok("citadel_hero_settings.lst"),
        _ => Err(bad()),
    }
}
fn target(game: &Path, kind: &str) -> Result<PathBuf, NativeError> {
    let p = paths::absolute(game)?
        .join("game/citadel/cfg")
        .join(filename(kind)?);
    paths::reject_symlinks(p.parent().ok_or_else(bad)?)?;
    Ok(p)
}
fn read(p: &Path) -> Result<Option<String>, NativeError> {
    read_limit(p, LIMIT)
}
fn read_limit(p: &Path, limit: u64) -> Result<Option<String>, NativeError> {
    match fs::symlink_metadata(p) {
        Ok(m) => {
            if !m.is_file() || m.nlink() != 1 || m.mode() & 0o7000 != 0 {
                return Err(bad());
            }
            Ok(Some(
                String::from_utf8(paths::read_bounded(p, limit)?).map_err(|_| bad())?,
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths::verify_absent(p)?;
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
fn image(v: &JsonValue) -> Result<Option<String>, NativeError> {
    if v.is_null() {
        Ok(None)
    } else {
        Ok(Some(
            v.as_str()
                .filter(|s| s.len() <= LIMIT as usize)
                .ok_or_else(bad)?
                .into(),
        ))
    }
}
fn atomic(p: &Path, s: Option<&str>, mode: u32) -> Result<(), NativeError> {
    paths::reject_symlinks(p.parent().ok_or_else(bad)?)?;
    read_limit(p, 16 * LIMIT)?;
    if let Some(s) = s {
        let temp = p
            .parent()
            .unwrap()
            .join(format!(".dmm-config-{}", paths::random_id()?));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(0x20000)
            .open(&temp)?;
        f.write_all(s.as_bytes())?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
        fs::rename(&temp, p)?;
    } else if p.exists() {
        fs::remove_file(p)?;
    }
    paths::sync_dir(p.parent().unwrap())
}
fn checkpoint(store: &Store, kind: &str) -> Result<PathBuf, NativeError> {
    filename(kind)?;
    Ok(store.root().join(format!("config-{kind}-undo.json")))
}
fn saved(store: &Store, kind: &str) -> Result<Option<JsonValue>, NativeError> {
    read_limit(&checkpoint(store, kind)?, 16 * LIMIT)?
        .map(|s| json::parse(&s).map_err(|_| bad()))
        .transpose()
}
fn game(store: &Store) -> Result<PathBuf, NativeError> {
    let d = store.document()?;
    Ok(paths::absolute(Path::new(
        d["gamePath"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(bad)?,
    ))?)
}
#[derive(Clone)]
pub struct Review {
    pub root: PathBuf,
    pub body: JsonValue,
    pub token: String,
}
// Scope names come from the game's recorded hero bindings, not model folder
// aliases. Model paths are used only to obtain a player-facing display name.
fn hero_scopes(settings: &hero::HeroSettings<'_>) -> JsonValue {
    let bindings = json::parse(include_str!(
        "../../vendor/health-engine/assets/hero-model-bindings.json"
    ))
    .expect("compiled hero bindings");
    let mut scopes = BTreeMap::<String, String>::new();
    let mut labels = std::collections::BTreeSet::new();
    for (key, models) in bindings["bindings"].entries() {
        let display =
            crate::hero::display_for_internal(key.trim_start_matches("hero_")).or_else(|| {
                models
                    .members()
                    .filter_map(|model| {
                        let path = Path::new(model.as_str()?);
                        crate::hero::display_for_internal(path.file_stem()?.to_str()?).or_else(
                            || {
                                crate::hero::display_for_internal(
                                    path.parent()?.file_name()?.to_str()?,
                                )
                            },
                        )
                    })
                    .next()
            });
        let mut name = display.unwrap_or_else(|| key.to_string());
        if !labels.insert(name.clone()) {
            name = format!("{name} ({key})");
            labels.insert(name.clone());
        }
        scopes.insert(key.into(), name);
    }
    for key in settings.hero_names() {
        scopes.entry(key.clone()).or_insert(key);
    }
    let mut rows = scopes
        .into_iter()
        .map(|(key, name)| object! {key:key,name:name})
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    rows.insert(0, object! {key:"",name:"All heroes"});
    rows.into()
}
pub fn view(store: &Store, kind: &str) -> Result<JsonValue, NativeError> {
    let game = game(store)?;
    let p = target(&game, kind)?;
    let s = read(&p)?;
    let mut v = object! {path:p.to_str(),canUndo:saved(store,kind)?.is_some(),available:true};
    if kind == "autoexec" {
        let (text, sections) = split(s.as_deref().unwrap_or(""))?;
        v["text"] = text.into();
        v["managedSections"] = sections.into();
    } else {
        v["values"] = read_values(s.as_deref().unwrap_or(hero::EMPTY_SETTINGS))?;
        v["defaults"] = defaults();
        v["heroScopes"] = hero_scopes(&hero::HeroSettings::parse(
            s.as_deref().unwrap_or(hero::EMPTY_SETTINGS),
        )?);
        v["heroes"] = HEROES
            .iter()
            .map(|h| JsonValue::from(*h))
            .collect::<Vec<_>>()
            .into();
        v["ranges"] = ranges();
    }
    Ok(v)
}
pub fn review(
    store: &Store,
    kind: &str,
    input: &JsonValue,
    undo: bool,
) -> Result<(Review, JsonValue), NativeError> {
    ensure_game_idle()?;
    if !store.journals()?.is_empty() {
        return Err(bad());
    }
    let game = game(store)?;
    let p = target(&game, kind)?;
    let before = read(&p)?;
    let after = if undo {
        let c = saved(store, kind)?.ok_or_else(bad)?;
        if c["gamePath"].as_str() != game.to_str() || image(&c["after"])? != before {
            return Err(bad());
        }
        image(&c["before"])?
    } else {
        Some(if kind == "autoexec" {
            merge(
                before.as_deref().unwrap_or(""),
                input.as_str().ok_or_else(bad)?,
            )?
        } else {
            update_values(before.as_deref().unwrap_or(hero::EMPTY_SETTINGS), input)?
        })
    };
    if after.as_ref().is_some_and(|s| s.len() > LIMIT as usize) {
        return Err(bad());
    }
    let mode = fs::symlink_metadata(&p)
        .map(|m| m.mode() & 0o777)
        .unwrap_or(0o644);
    let mut body = object! {kind:"configWrite",version:1,config:kind,gamePath:game.to_str(),before:before.clone(),after:after.clone(),mode:mode,undo:undo,revision:store.document()?["revision"].clone()};
    if kind == "crosshair" {
        let ap = target(&game, "autoexec")?;
        let ab = read(&ap)?;
        let aa = if undo {
            let c = saved(store, kind)?.ok_or_else(bad)?;
            if image(&c["autoexecAfter"])? != ab {
                return Err(bad());
            }
            image(&c["autoexecBefore"])?
        } else {
            ab.as_ref()
                .map(|s| {
                    let (_, sections) = split(s)?;
                    let mut out = s.clone();
                    for section in sections.into_iter().rev() {
                        if section["content"]
                            .as_str()
                            .unwrap()
                            .starts_with(MARKERS[0].0)
                        {
                            out.replace_range(
                                section["start"].as_usize().unwrap()
                                    ..section["end"].as_usize().unwrap(),
                                "",
                            );
                        }
                    }
                    Ok::<String, NativeError>(out)
                })
                .transpose()?
        };
        body["autoexecBefore"] = ab.into();
        body["autoexecAfter"] = aa.into();
        body["autoexecMode"] = fs::symlink_metadata(ap)
            .map(|m| m.mode() & 0o777)
            .unwrap_or(0o644)
            .into();
    }
    let token = paths::random_id()?;
    let mut dto = object! {token:token.clone(),kind:kind,path:p.to_str(),before:before,after:after,undo:undo,writeModel:"immediate",diff:object!{format:"exactReplacement",before:body["before"].clone(),after:body["after"].clone()}};
    dto["additionalDiff"] = if kind == "crosshair" {
        object! {path:target(&game,"autoexec")?.to_str(),before:body["autoexecBefore"].clone(),after:body["autoexecAfter"].clone()}
    } else {
        JsonValue::Null
    };
    Ok((
        Review {
            root: store.root().into(),
            body,
            token,
        },
        dto,
    ))
}
pub fn admit(store: &Store, j: &JsonValue) -> Result<(), NativeError> {
    if j["kind"] != "configWrite"
        || j["version"] != 1
        || j["gamePath"].as_str() != game(store)?.to_str()
        || j["revision"] != store.document()?["revision"]
        || j["undo"].as_bool().is_none()
        || j["mode"].as_u32().filter(|m| *m <= 0o777).is_none()
    {
        return Err(bad());
    }
    filename(j["config"].as_str().ok_or_else(bad)?)?;
    if j["config"] == "crosshair" {
        image(&j["autoexecBefore"])?;
        image(&j["autoexecAfter"])?;
        if j["autoexecMode"].as_u32().filter(|m| *m <= 0o777).is_none() {
            return Err(bad());
        }
    }
    image(&j["before"])?;
    image(&j["after"])?;
    Ok(())
}
pub fn commit(store: &Store, lock: &RootLock, r: &Review) -> Result<(), NativeError> {
    if r.root != store.root() || !store.journals()?.is_empty() {
        return Err(bad());
    }
    admit(store, &r.body)?;
    let g = game(store)?;
    let _guard = GameLock::acquire(&g.join("game/citadel"))?;
    ensure_game_idle()?;
    let p = target(&g, r.body["config"].as_str().ok_or_else(bad)?)?;
    if read(&p)? != image(&r.body["before"])? {
        return Err(bad());
    }
    if r.body["config"] == "crosshair"
        && read(&target(&g, "autoexec")?)? != image(&r.body["autoexecBefore"])?
    {
        return Err(bad());
    }
    store.write_journal(lock, &r.token, &r.body)?;
    finish(store, lock, &r.token, &r.body)
}
fn finish(store: &Store, lock: &RootLock, id: &str, j: &JsonValue) -> Result<(), NativeError> {
    admit(store, j)?;
    ensure_game_idle()?;
    let kind = j["config"].as_str().ok_or_else(bad)?;
    let p = target(&game(store)?, kind)?;
    let current = read(&p)?;
    let before = image(&j["before"])?;
    let after = image(&j["after"])?;
    if current != before && current != after {
        return Err(NativeError::new(
            "recoveryRequired",
            "Config changed outside the reviewed transaction; preserve the journal.",
        ));
    }
    if kind == "crosshair" {
        let ac = read(&target(&game(store)?, "autoexec")?)?;
        if ac != image(&j["autoexecBefore"])? && ac != image(&j["autoexecAfter"])? {
            return Err(bad());
        }
    }
    if current != after {
        atomic(&p, after.as_deref(), j["mode"].as_u32().ok_or_else(bad)?)?;
    }
    if kind == "crosshair" {
        let ap = target(&game(store)?, "autoexec")?;
        let ab = image(&j["autoexecBefore"])?;
        let aa = image(&j["autoexecAfter"])?;
        let ac = read(&ap)?;
        if ac != ab && ac != aa {
            return Err(bad());
        }
        if ac != aa {
            atomic(
                &ap,
                aa.as_deref(),
                j["autoexecMode"].as_u32().ok_or_else(bad)?,
            )?;
        }
    }
    let cp = checkpoint(store, kind)?;
    if j["undo"] == true {
        atomic(&cp, None, 0o600)?
    } else {
        atomic(&cp, Some(&j.dump()), 0o600)?
    }
    store.clear_journal(lock, id)
}
pub fn recover(store: &Store, lock: &RootLock) -> Result<(), NativeError> {
    let journals = store.journals()?;
    if journals.len() != 1 {
        return Err(bad());
    }
    let e = &journals[0];
    admit(store, &e["body"])?;
    let _guard = GameLock::acquire(&game(store)?.join("game/citadel"))?;
    finish(store, lock, e["id"].as_str().ok_or_else(bad)?, &e["body"])
}
