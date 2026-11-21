#![allow(dead_code)]
use json::{object, JsonValue};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
pub struct Fixture {
    pub root: PathBuf,
    pub source: PathBuf,
    pub game: PathBuf,
    pub data: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "deadlock-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("upstream/state.json");
        let game = root.join("game-root");
        let data = root.join("native");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(game.join("game/citadel/addons")).unwrap();
        fs::create_dir_all(game.join("game/bin/win64")).unwrap();
        fs::write(game.join("game/citadel/pak01_dir.vpk"), vpk("scripts/core.vdata_c",0)).unwrap();
        let mut f = Self {
            root,
            source,
            game,
            data,
        };
        f.reset();
        f
    }
    pub fn reset(&mut self) {
        let m = object! {id:"local-a",remoteId:"remote-a",name:"Working A",status:"installed",installOrder:0,variants:json::array![object!{fileId:9}],unknownMod:object!{preserve:true},images:json::array!["https://example.invalid/a.png"]};
        let inactive = object! {id:"local-b",remoteId:"remote-b",name:"Disabled B",status:"disabled",installOrder:1};
        let state = object! {activeProfileId:"default",localMods:json::array![m.clone()],groups:json::array![object!{id:"group-a",mods:json::array!["local-a"]}],token:"AUTH_SENTINEL",account:object!{private:"AUTH_SENTINEL"},profiles:object!{
          default:object!{id:"default",name:"Default",mods:json::array![object!{id:"stale",remoteId:"stale",status:"installed"}],enabledMods:object!{"remote-a":object!{enabled:true}},groups:json::array![object!{id:"saved-group"}],unknownProfile:object!{preserve:true}},
          other:object!{id:"other",name:"Other",folderName:"other-folder",mods:json::array![inactive],enabledMods:object!{"remote-b":object!{enabled:false}}}
        }};
        self.write_state(
            object! {"local-config":object!{version:33,state:state},session:"AUTH_SENTINEL"},
        );
        fs::write(
            self.game.join("game/citadel/addons/pak01_dir.vpk"),
            vpk("models/a.vmdl_c", 7),
        )
        .unwrap();
        fs::create_dir_all(self.game.join("game/citadel/addons/other-folder")).unwrap();
        fs::write(
            self.game
                .join("game/citadel/addons/other-folder/remote-b_b_dir.vpk"),
            vpk("models/b.vmdl_c", 8),
        )
        .unwrap();
        // Upstream manifest v3 filename and disabled convention are source-backed.
        self.write_manifest("", object!{version:3,mods:object!{"remote-a":object!{enabled:true,order:0,shard:1,currentVpks:json::array!["pak01_dir.vpk"],disabledVpks:json::array![],originalVpkNames:json::array!["a_dir.vpk"]}}});
        self.write_manifest("other-folder",object!{version:3,mods:object!{"remote-b":object!{enabled:false,order:1,shard:1,currentVpks:json::array![],disabledVpks:json::array!["remote-b_b_dir.vpk"],originalVpkNames:json::array!["b_dir.vpk"]}}});
    }
    pub fn state(&self) -> JsonValue {
        json::parse(&fs::read_to_string(&self.source).unwrap()).unwrap()
    }
    pub fn write_state(&self, v: JsonValue) {
        fs::write(&self.source, v.dump()).unwrap()
    }
    pub fn manifest_path(&self, folder: &str) -> PathBuf {
        self.game
            .join("game/citadel/addons")
            .join(folder)
            .join(".dmm.json")
    }
    pub fn write_manifest(&self, folder: &str, v: JsonValue) {
        fs::write(self.manifest_path(folder), v.dump()).unwrap()
    }
    pub fn params(&self) -> JsonValue {
        object! {dataDir:self.data.to_str().unwrap()}
    }
    pub fn preview_params(&self) -> JsonValue {
        object! {dataDir:self.data.to_str().unwrap(),statePath:self.source.to_str().unwrap(),gamePath:self.game.to_str().unwrap()}
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
pub fn vpk(path: &str, crc: u32) -> Vec<u8> {
    let (folder, file) = path.rsplit_once('/').unwrap_or((" ", path));
    let (name, ext) = file.rsplit_once('.').unwrap();
    let mut tree = Vec::new();
    for s in [ext, folder, name] {
        tree.extend_from_slice(s.as_bytes());
        tree.push(0)
    }
    tree.extend_from_slice(&crc.to_le_bytes());
    tree.extend_from_slice(&0u16.to_le_bytes());
    tree.extend_from_slice(&0x7fffu16.to_le_bytes());
    tree.extend_from_slice(&0u32.to_le_bytes());
    tree.extend_from_slice(&0u32.to_le_bytes());
    tree.extend_from_slice(&0xffffu16.to_le_bytes());
    tree.extend_from_slice(&[0, 0, 0]);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x55aa1234u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&(tree.len() as u32).to_le_bytes());
    bytes.extend(tree);
    bytes
}
pub fn rpc(method: &str, params: JsonValue) -> JsonValue {
    let request = object! {jsonrpc:"2.0",id:1,method:method,params:params}.dump() + "\n";
    let mut out = Vec::new();
    modlock_helper::protocol::serve(std::io::Cursor::new(request), &mut out).unwrap();
    json::parse(std::str::from_utf8(&out).unwrap()).unwrap()
}

pub fn preview(s: &mut modlock_helper::native::NativeService, f: &Fixture) -> JsonValue {
    s.dispatch("native.importPreview", &f.preview_params())
        .unwrap()
}
pub fn import(s: &mut modlock_helper::native::NativeService, f: &Fixture) -> JsonValue {
    let p = preview(s, f);
    let mut q = f.params();
    q["token"] = p["token"].clone();
    q["operationId"] = "import-1".into();
    q["expectedRevision"] = "0".into();
    s.dispatch("native.importCommit", &q).unwrap()
}
pub fn mutation(f: &Fixture, revision: &str, id: &str) -> JsonValue {
    let mut q = f.params();
    q["expectedRevision"] = revision.into();
    q["operationId"] = id.into();
    q
}

// Frozen before Store2 import implementation. Compatibility tests must not derive
// legacy data from the current import serializer.
pub const LEGACY_STORE1: &str = r#"{"storeVersion":1,"revision":"1","sourcePath":"/legacy/state.json","gamePath":"","activeProfileId":"default","profiles":{"default":{"id":"default","name":"Default","protected":true,"unknown":{"keep":true},"mods":[{"source":{"id":"old","remoteId":"old","name":"Old unavailable family","status":"installed","unknown":{"keep":true}},"enabled":true,"available":false,"unavailableReason":"External chunks unsupported","previewHidden":true,"payloads":[]}]}},"importArchive":{"unknown":{"keep":true}},"importLedger":{"sourcePayloads":[]},"applied":null,"canRestore":false,"recoveryRequired":false,"unknown":{"keep":true}}"#;

pub fn family_fixture(f: &Fixture, disabled: bool) -> Vec<PathBuf> {
    let originals = ["skin_dir.vpk", "skin_000.vpk", "skin_017.vpk"];
    let names: Vec<String> = if disabled {
        originals.iter().map(|n| format!("remote-a_{n}")).collect()
    } else {
        [
            "renamed-directory.vpk",
            "unrelated-a.vpk",
            "unrelated-b.vpk",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let files = vec![
        family_vpk(
            2,
            &[
                FamilyEntry {
                    path: "models/a.vmdl_c",
                    crc: 7,
                    archive: 0,
                    offset: 0,
                    length: 3,
                    preload: b"pre",
                },
                FamilyEntry {
                    path: "materials/a.vmat_c",
                    crc: 8,
                    archive: 17,
                    offset: 0,
                    length: 4,
                    preload: b"",
                },
            ],
            b"",
        ),
        b"one".to_vec(),
        b"two!".to_vec(),
    ];
    let paths: Vec<_> = names
        .iter()
        .map(|n| f.game.join("game/citadel/addons").join(n))
        .collect();
    for (p, b) in paths.iter().zip(files) {
        fs::write(p, b).unwrap();
    }
    f.write_manifest("", object!{version:3,mods:object!{"remote-a":object!{enabled:!disabled,order:0,shard:1,currentVpks:if disabled {json::array![]}else{JsonValue::from(names.clone())},disabledVpks:if disabled {JsonValue::from(names)}else{json::array![]},originalVpkNames:JsonValue::from(originals.to_vec())}}});
    // One mod/profile makes physical-copy assertions unambiguous.
    let mut state = f.state();
    state["local-config"]["state"]["profiles"].remove("other");
    f.write_state(state);
    paths
}

/// Generated byte-format fixture; companions are deliberately opaque bytes.
pub struct FamilyEntry<'a> {
    pub path: &'a str,
    pub crc: u32,
    pub archive: u16,
    pub offset: u32,
    pub length: u32,
    pub preload: &'a [u8],
}
pub fn family_vpk(version: u32, entries: &[FamilyEntry<'_>], inline: &[u8]) -> Vec<u8> {
    let mut tree = Vec::new();
    for e in entries {
        let (dir, file) = e.path.rsplit_once('/').unwrap_or((" ", e.path));
        let (name, ext) = file.rsplit_once('.').unwrap_or((file, " "));
        for s in [ext, dir, name] {
            tree.extend_from_slice(s.as_bytes());
            tree.push(0);
        }
        tree.extend_from_slice(&e.crc.to_le_bytes());
        tree.extend_from_slice(&(u16::try_from(e.preload.len()).unwrap()).to_le_bytes());
        tree.extend_from_slice(&e.archive.to_le_bytes());
        tree.extend_from_slice(&e.offset.to_le_bytes());
        tree.extend_from_slice(&e.length.to_le_bytes());
        tree.extend_from_slice(&0xffffu16.to_le_bytes());
        tree.extend_from_slice(e.preload);
        tree.extend_from_slice(&[0, 0]);
    }
    tree.push(0);
    vpk_with_tree(version, &tree, inline, [&[], &[], &[]])
}
pub fn vpk_with_tree(version: u32, tree: &[u8], inline: &[u8], trailing: [&[u8]; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x55aa1234u32.to_le_bytes());
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&(u32::try_from(tree.len()).unwrap()).to_le_bytes());
    if version == 2 {
        for size in [
            inline.len(),
            trailing[0].len(),
            trailing[1].len(),
            trailing[2].len(),
        ] {
            bytes.extend_from_slice(&(u32::try_from(size).unwrap()).to_le_bytes());
        }
    }
    bytes.extend_from_slice(tree);
    bytes.extend_from_slice(inline);
    if version == 2 {
        for section in trailing {
            bytes.extend_from_slice(section);
        }
    }
    bytes
}

/// Construct a valid current transaction envelope for tests that used old
/// partial envelopes. Explicit fields remain available for negative tests.
pub fn game_input(method: &str, revision: u64, id: &str, value: &JsonValue) -> JsonValue {
    let mut value=value.clone();
    if !value.has_key("method"){value["method"]=method.into();}
    if !value["params"].is_object(){value["params"]=object!{};}
    for (key,field) in [("operationId",id.to_owned()),("expectedRevision",revision.to_string())] {
        if !value["params"].has_key(key){value["params"][key]=field.into();}
    }
    if method=="native.apply" {
        if !value["params"].has_key("deploymentLayout"){value["params"]["deploymentLayout"]="shards-v1".into();}
        if !value["params"].has_key("token"){value["params"]["token"]="0123456789abcdef0123456789abcdef".into();}
    }
    value
}

/// The supported native Apply protocol always requests and acknowledges shards.
pub fn current_layout(value: &JsonValue) -> JsonValue {
    let mut value=value.clone();
    value["deploymentLayout"]="shards-v1".into();
    value
}
