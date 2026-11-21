//! Upstream paths are derived without opening any account or game files.
use crate::snapshot::SnapshotError;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const APP_ID: &str = "dev.stormix.deadlock-mod-manager";

/// Prefer Flatpak's private data directory, then the upstream native XDG store.
/// Empty/relative environment values are ignored according to XDG conventions.
pub fn discover() -> Result<PathBuf, SnapshotError> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let mut paths = Vec::new();
    if let Some(home) = &home {
        paths.push(
            home.join(".var/app")
                .join(APP_ID)
                .join("data")
                .join(APP_ID)
                .join("state.json"),
        );
    }
    if let Some(data) = data.or_else(|| home.as_ref().map(|h| h.join(".local/share"))) {
        paths.push(data.join(APP_ID).join("state.json"));
    }
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(SnapshotError::invalid("Cannot inspect upstream state; check file permissions or choose a readable state.json.")),
        }
    }
    Err(SnapshotError::not_found())
}

pub(crate) fn absolute(path: &Path) -> Result<PathBuf, SnapshotError> {
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    env::current_dir().map(|dir| dir.join(path)).map_err(|_| {
        SnapshotError::invalid("Cannot resolve source path; supply an absolute state.json path.")
    })
}
