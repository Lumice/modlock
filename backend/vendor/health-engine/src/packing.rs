//! Deterministic inline VPK v1 writer. No external tools or archive members.
use crate::errors::Error;
use std::{collections::BTreeMap, fs, io::Write, path::Path};
pub fn pack_directory(root: &Path, output: &Path) -> Result<usize, Error> {
    let mut files = BTreeMap::new();
    let mut todo = vec![root.to_owned()];
    while let Some(dir) = todo.pop() {
        for e in fs::read_dir(dir)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                todo.push(e.path());
            } else if e.file_type()?.is_file() {
                let name = e
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(name, fs::read(e.path())?);
            }
        }
    }
    let mut tree: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<u8>>>> = BTreeMap::new();
    for (name, bytes) in files {
        let (folder, base) = name.rsplit_once('/').unwrap_or((" ", name.as_str()));
        let (stem, ext) = base.rsplit_once('.').unwrap_or((base, " "));
        tree.entry(ext.into())
            .or_default()
            .entry(folder.into())
            .or_default()
            .insert(stem.into(), bytes);
    }
    let mut index = Vec::new();
    let mut data = Vec::new();
    let mut count = 0;
    fn word(out: &mut Vec<u8>, s: &str) {
        out.extend(s.as_bytes());
        out.push(0);
    }
    for (ext, dirs) in tree {
        word(&mut index, &ext);
        for (dir, entries) in dirs {
            word(&mut index, &dir);
            for (name, bytes) in entries {
                word(&mut index, &name);
                let mut crc = !0u32;
                for b in &bytes {
                    crc ^= *b as u32;
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ (0xedb88320u32 & (0u32.wrapping_sub(crc & 1)));
                    }
                }
                index.extend((!crc).to_le_bytes());
                index.extend(0u16.to_le_bytes());
                index.extend(0x7fffu16.to_le_bytes());
                index.extend(
                    u32::try_from(data.len())
                        .map_err(|_| Error::ModInvalid("Overlay too large".into()))?
                        .to_le_bytes(),
                );
                index.extend(
                    u32::try_from(bytes.len())
                        .map_err(|_| Error::ModInvalid("Resource too large".into()))?
                        .to_le_bytes(),
                );
                index.extend(0xffffu16.to_le_bytes());
                data.extend(bytes);
                count += 1;
            }
            index.push(0);
        }
        index.push(0);
    }
    index.push(0);
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    out.write_all(&0x55aa1234u32.to_le_bytes())?;
    out.write_all(&1u32.to_le_bytes())?;
    out.write_all(&(index.len() as u32).to_le_bytes())?;
    out.write_all(&index)?;
    out.write_all(&data)?;
    out.sync_all()?;
    Ok(count)
}
