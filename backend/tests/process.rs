use json::object;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "modlock-helper-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            include_bytes!("../../tests/fixtures/upstream-state-v33.json"),
        )
        .unwrap();
        path
    }
    fn call(&self, xdg: Option<&str>, params: json::JsonValue) -> json::JsonValue {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_modlock-helper"));
        cmd.env("HOME", &self.0)
            .env_remove("XDG_DATA_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(xdg) = xdg {
            cmd.env("XDG_DATA_HOME", self.0.join(xdg));
        }
        let mut child = cmd.spawn().unwrap();
        writeln!(
            child.stdin.take().unwrap(),
            "{}",
            object! {jsonrpc:"2.0",id:1,method:"library.snapshot",params:params}.dump()
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            output.stderr.is_empty(),
            "no source data or routine diagnostics on stderr"
        );
        json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap()
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn discovers_flatpak_before_xdg_native_and_never_changes_source() {
    let home = Home::new();
    let flatpak = home.write(".var/app/dev.stormix.deadlock-mod-manager/data/dev.stormix.deadlock-mod-manager/state.json");
    home.write("xdg/dev.stormix.deadlock-mod-manager/state.json");
    let before = fs::read(&flatpak).unwrap();
    let mtime = fs::metadata(&flatpak).unwrap().modified().unwrap();
    let r = home.call(Some("xdg"), object! {});
    assert_eq!(r["result"]["sourcePath"], flatpak.to_str().unwrap());
    assert_eq!(fs::read(&flatpak).unwrap(), before);
    assert_eq!(fs::metadata(&flatpak).unwrap().modified().unwrap(), mtime);
    assert!(!r.dump().contains("SYNTHETIC_SECRET_DO_NOT_EXPOSE"));
    fs::remove_file(&flatpak).unwrap();
    let r = home.call(Some("xdg"), object! {});
    assert!(r["result"]["sourcePath"]
        .as_str()
        .unwrap()
        .ends_with("xdg/dev.stormix.deadlock-mod-manager/state.json"));
}
#[test]
fn discovers_home_native_when_xdg_absent_and_missing_sources_are_actionable() {
    let home = Home::new();
    let r = home.call(None, object! {});
    assert_eq!(r["error"]["data"]["kind"], "sourceNotFound");
    let path = home.write(".local/share/dev.stormix.deadlock-mod-manager/state.json");
    let r = home.call(None, object! {});
    assert_eq!(r["result"]["sourcePath"], path.to_str().unwrap());
    let r = home.call(
        None,
        object! {statePath:home.0.join("missing.json").to_str().unwrap()},
    );
    assert_eq!(r["error"]["code"], -32010);
    // An XDG override must not accidentally read HOME's native store.
    let r = home.call(Some("empty-xdg"), object! {});
    assert_eq!(r["error"]["code"], -32010);
}
