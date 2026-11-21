//! Bounded Valve package directory inspection. No discovery, extraction or writes.
use crate::{native::NativeError, paths, prepare_control::PrepareControl};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub(crate) const DUPLICATE_ENTRY_MESSAGE: &str = "This mod’s file contains a repeated filename, so it can’t be imported automatically. Try adding a fresh copy from Browse.";
const MAX_TREE: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpkEntry {
    pub path: String,
    pub crc: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLocation {
    pub archive_index: u16,
    pub offset: u64,
    pub length: u64,
    pub preload: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpkResource {
    pub entry: VpkEntry,
    /// Original decoded/assembled spelling; entry.path remains normalized.
    pub original_path: String,
    pub location: ResourceLocation,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VpkDirectory {
    pub resources: Vec<VpkResource>,
    pub inline_offset: u64,
    pub inline_size: u64,
}
fn invalid() -> NativeError {
    NativeError::new("unsupportedVpk","VPK directory or required archive members are malformed, missing, or exceed supported bounds; provide a complete package family (legacy libraries require standalone directories).")
}
struct Tree<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Tree<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NativeError> {
        let end = self.at.checked_add(n).ok_or_else(invalid)?;
        let b = self.bytes.get(self.at..end).ok_or_else(invalid)?;
        self.at = end;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, NativeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, NativeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<&'a str, NativeError> {
        let start = self.at;
        let len = self
            .bytes
            .get(start..)
            .ok_or_else(invalid)?
            .iter()
            .take(4097)
            .position(|b| *b == 0)
            .ok_or_else(invalid)?;
        self.at += len + 1;
        std::str::from_utf8(&self.bytes[start..start + len]).map_err(|_| invalid())
    }
}
fn normalized(path: &str) -> Result<String, NativeError> {
    let p = path.replace('\\', "/").to_lowercase();
    if p.len() > 4096
        || p.starts_with('/')
        || p.contains(':')
        || p.chars().any(char::is_control)
        || p.split('/').any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid());
    }
    Ok(p)
}
pub fn inspect(path: &Path) -> Result<Vec<VpkEntry>, NativeError> {
    let directory = inspect_directory(path)?;
    if directory
        .resources
        .iter()
        .any(|resource| resource.location.archive_index != 0x7fff)
    {
        return Err(invalid());
    }
    Ok(directory.resources.into_iter().map(|r| r.entry).collect())
}
pub fn inspect_directory(path: &Path) -> Result<VpkDirectory, NativeError> {
    inspect_directory_controlled(path, &PrepareControl::never_cancelled())
}
pub fn inspect_directory_controlled(
    path: &Path,
    control: &PrepareControl,
) -> Result<VpkDirectory, NativeError> {
    control.check()?;
    let mut f = paths::open_regular(path)?;
    inspect_held_directory_controlled(&mut f, control)
}

/// Inspect an already admitted file while its caller retains the owning lease.
/// No path is reconstructed from a display name or from the file descriptor.
pub(crate) fn inspect_held_directory_controlled(
    f: &mut File,
    control: &PrepareControl,
) -> Result<VpkDirectory, NativeError> {
    control.check()?;
    f.seek(SeekFrom::Start(0))?;
    let file_size = f.metadata()?.len();
    let mut header = [0u8; 12];
    f.read_exact(&mut header).map_err(|_| invalid())?;
    let u32at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
    if u32at(0) != 0x55aa1234 {
        return Err(invalid());
    }
    let version = u32at(4);
    let tree_size = u32at(8) as usize;
    if tree_size == 0 || tree_size > MAX_TREE {
        return Err(invalid());
    }
    let (header_size, data_size) = match version {
        1 => (
            12u64,
            file_size
                .checked_sub(12)
                .and_then(|size| size.checked_sub(tree_size as u64))
                .ok_or_else(invalid)?,
        ),
        2 => {
            let mut extra = [0u8; 16];
            f.read_exact(&mut extra).map_err(|_| invalid())?;
            let section: Vec<u64> = extra
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as u64)
                .collect();
            let start = 28u64.checked_add(tree_size as u64).ok_or_else(invalid)?;
            let expected = section
                .iter()
                .try_fold(start, |n, s| n.checked_add(*s).ok_or_else(invalid))?;
            if expected != file_size {
                return Err(invalid());
            }
            (28, section[0])
        }
        _ => return Err(invalid()),
    };
    let inline_offset = header_size
        .checked_add(tree_size as u64)
        .ok_or_else(invalid)?;
    if inline_offset > file_size {
        return Err(invalid());
    }
    let mut bytes = vec![0; tree_size];
    f.read_exact(&mut bytes).map_err(|_| invalid())?;
    let mut t = Tree {
        bytes: &bytes,
        at: 0,
    };
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    loop {
        control.check()?;
        let ext = t.string()?;
        if ext.is_empty() {
            break;
        }
        if ext != " " && (ext.contains(['/', '\\', '.']) || ext.chars().any(char::is_control)) {
            return Err(invalid());
        }
        loop {
            control.check()?;
            let dir = t.string()?;
            if dir.is_empty() {
                break;
            }
            loop {
                control.check()?;
                let name = t.string()?;
                if name.is_empty() {
                    break;
                }
                if name.contains(['/', '\\']) {
                    return Err(invalid());
                }
                let crc = t.u32()?;
                let preload = t.u16()? as usize;
                let archive = t.u16()?;
                let offset = t.u32()? as u64;
                let length = t.u32()? as u64;
                if t.u16()? != 0xffff
                    || (length != 0
                        && archive == 0x7fff
                        && offset.checked_add(length).is_none_or(|end| end > data_size))
                {
                    return Err(invalid());
                }
                let preload = t.take(preload)?.to_vec();
                let base = if dir == " " {
                    name.to_owned()
                } else {
                    format!("{dir}/{name}")
                };
                let full = if ext == " " {
                    base
                } else {
                    format!("{base}.{ext}")
                };
                let p = normalized(&full)?;
                if !seen.insert(p.clone()) {
                    return Err(NativeError::new("unsupportedVpk", DUPLICATE_ENTRY_MESSAGE));
                }
                if entries.len() >= MAX_ENTRIES {
                    return Err(invalid());
                }
                entries.push(VpkResource {
                    entry: VpkEntry { path: p, crc },
                    original_path: full.replace('\\', "/"),
                    location: ResourceLocation {
                        archive_index: archive,
                        offset,
                        length,
                        preload,
                    },
                });
            }
        }
    }
    if t.at != bytes.len() {
        return Err(invalid());
    }
    Ok(VpkDirectory {
        resources: entries,
        inline_offset,
        inline_size: data_size,
    })
}

/// Only non-inline references with remaining data require a companion.
pub fn required_archives(directory: &VpkDirectory) -> BTreeSet<u16> {
    directory
        .resources
        .iter()
        .filter(|r| r.location.length != 0)
        .map(|r| r.location.archive_index)
        .filter(|index| *index != 0x7fff)
        .collect()
}

pub fn validate_family(
    directory: &VpkDirectory,
    companions: &BTreeMap<u16, PathBuf>,
) -> Result<(), NativeError> {
    validate_family_controlled(directory, companions, &PrepareControl::never_cancelled())
}
pub(crate) fn validate_family_controlled(
    directory: &VpkDirectory,
    companions: &BTreeMap<u16, PathBuf>,
    control: &PrepareControl,
) -> Result<(), NativeError> {
    control.check()?;
    if directory.resources.len() > MAX_ENTRIES
        || directory
            .inline_offset
            .checked_add(directory.inline_size)
            .is_none()
        || required_archives(directory) != companions.keys().copied().collect()
    {
        return Err(invalid());
    }
    let mut sizes = BTreeMap::new();
    for (index, path) in companions {
        control.check()?;
        sizes.insert(*index, paths::open_regular(path)?.metadata()?.len());
    }
    for resource in &directory.resources {
        control.check()?;
        let location = &resource.location;
        if location.length == 0 {
            continue;
        }
        let size = if location.archive_index == 0x7fff {
            directory.inline_size
        } else {
            *sizes.get(&location.archive_index).ok_or_else(invalid)?
        };
        check_range(location.offset, location.length, size)?;
    }
    Ok(())
}

fn check_range(offset: u64, length: u64, size: u64) -> Result<(), NativeError> {
    if offset.checked_add(length).is_none_or(|end| end > size) {
        return Err(invalid());
    }
    Ok(())
}

/// Read one resource only after checking its combined preload/data size.
pub fn read_resource(
    directory_path: &Path,
    directory: &VpkDirectory,
    companions: &BTreeMap<u16, PathBuf>,
    resource_index: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, NativeError> {
    let location = &directory
        .resources
        .get(resource_index)
        .ok_or_else(invalid)?
        .location;
    let size = usize::try_from(location.length)
        .ok()
        .and_then(|n| n.checked_add(location.preload.len()))
        .filter(|n| *n <= max_bytes)
        .ok_or_else(invalid)?;
    validate_family(directory, companions)?;
    // Validate the directory path even when the selected resource is external.
    let directory_file = paths::open_regular(directory_path)?;
    check_range(
        directory.inline_offset,
        directory.inline_size,
        directory_file.metadata()?.len(),
    )?;
    if location.length == 0 {
        return Ok(location.preload.clone());
    }
    let (mut file, origin) = if location.archive_index == 0x7fff {
        (directory_file, directory.inline_offset)
    } else {
        let file = paths::open_regular(
            companions
                .get(&location.archive_index)
                .ok_or_else(invalid)?,
        )?;
        // Recheck the selected opened member, rather than relying on an earlier open.
        check_range(location.offset, location.length, file.metadata()?.len())?;
        (file, 0)
    };
    file.seek(SeekFrom::Start(
        origin.checked_add(location.offset).ok_or_else(invalid)?,
    ))?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| invalid())?;
    bytes.resize(size, 0);
    bytes[..location.preload.len()].copy_from_slice(&location.preload);
    file.read_exact(&mut bytes[location.preload.len()..])
        .map_err(|_| invalid())?;
    Ok(bytes)
}
