//! Independently located bulk storage. SQLite and all transaction journals remain
//! on the original filesystem. A durable manifest precedes copy and deletion.
use crate::{
    native::NativeError,
    paths,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
#[cfg(test)]
thread_local! {static BEFORE_SOURCE_UNLINK:std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>>=const {std::cell::RefCell::new(None)};static COPY_CREATED:std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>>=const {std::cell::RefCell::new(None)};static BEFORE_COPY_OPEN:std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>>=const {std::cell::RefCell::new(None)};}
const DIRS: &[&str] = &[
    "payloads",
    "archives",
    "artwork",
    "fonts",
    "font-backups",
    "cache/repairs",
    "remote-skips.v1.json",
];
fn changed() -> NativeError {
    NativeError::new(
        "storageChanged",
        "Storage changed. Preserve both folders and review storage recovery in Settings.",
    )
}
fn identity(path: &Path) -> Result<JsonValue, NativeError> {
    paths::reject_symlinks(path)?;
    let file=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(path)?;
    let m=file.metadata()?;let named=fs::symlink_metadata(path)?;
    if (m.dev(),m.ino())!=(named.dev(),named.ino()){return Err(changed());}
    Ok(object! {device:m.dev(),inode:m.ino(),fsid:paths::persisted_filesystem_id(&file)?})
}
fn identity_matches(path:&Path,id:&JsonValue,lock:Option<&RootLock>)->Result<bool,NativeError>{
    if !id.is_object() || id.len()!=if id.has_key("fsid"){3}else{2} || !id.has_key("device") || !id.has_key("inode"){return Ok(false);}
    let (Some(device),Some(inode))=(id["device"].as_u64(),id["inode"].as_u64())else{return Ok(false)};
    let fsid=if id.has_key("fsid"){Some(id["fsid"].as_str().filter(|s|crate::package::hex(s,16)).ok_or_else(changed)?)}else{None};
    paths::reject_symlinks(path)?;
    let file=OpenOptions::new().read(true).custom_flags(0x20000|0x800).open(path)?;let m=file.metadata()?;
    if let Some(lock)=lock {lock.validate()?;return paths::same_persisted_object(lock.root(),lock.directory(),path,&file,device,inode,fsid);}
    if fsid.is_none() && m.dev()!=device{return Ok(false);}
    paths::same_persisted_object(path,&file,path,&file,device,inode,fsid)
}
pub fn location(value: &JsonValue) -> Result<PathBuf, NativeError> {
    location_with_lock(value,None)
}
pub(crate) fn location_with_lock(value:&JsonValue,lock:Option<&RootLock>)->Result<PathBuf,NativeError>{
    let path = paths::absolute(Path::new(value["path"].as_str().ok_or_else(changed)?))?;
    if !identity_matches(&path,&value["identity"],lock)? || !path.is_dir() {
        return Err(changed());
    }
    Ok(path)
}
/// No library means the caller is constructing a new library or a synthetic
/// filename. Existing libraries never fall back when the mounted store is lost.
pub(crate) fn root(root: &Path) -> Result<PathBuf, NativeError> {
    if !root.join("library.sqlite3").try_exists()? {
        return Ok(root.to_owned());
    }
    Store::open_history(root)?.storage_root()
}
pub(crate) fn path(root: &Path, name: impl AsRef<Path>) -> Result<PathBuf, NativeError> {
    Ok(self::root(root)?.join(name))
}
fn files(base: &Path) -> Result<JsonValue, NativeError> {
    fn walk(
        base: &Path,
        p: &Path,
        out: &mut JsonValue,
        budget: &mut usize,
    ) -> Result<(), NativeError> {
        if *budget == 0 {
            return Err(changed());
        }
        *budget -= 1;
        paths::reject_symlinks(p)?;
        let m = fs::symlink_metadata(p)?;
        if m.is_dir() {
            for e in fs::read_dir(p)? {
                walk(base, &e?.path(), out, budget)?;
            }
        } else if m.is_file() {
            let fp = paths::fingerprint(p)?;
            out.push(object!{name:p.strip_prefix(base).map_err(|_|changed())?.to_str().ok_or_else(changed)?,identity:identity(p)?,fingerprint:crate::fonts::fingerprint_json(&fp),mode:m.mode() & 0o777,size:fp.size,sha256:fp.sha256}) .map_err(|_|changed())?;
        } else {
            return Err(changed());
        }
        Ok(())
    }
    let mut result = JsonValue::new_array();
    let mut budget = 100_000;
    for name in DIRS {
        let p = base.join(name);
        if p.try_exists()? {
            walk(base, &p, &mut result, &mut budget)?;
        }
    }
    let mut rows = result.members().cloned().collect::<Vec<_>>();
    rows.sort_by_key(|r| r["name"].as_str().unwrap_or("").to_owned());
    Ok(rows.into())
}
fn bytes(rows: &JsonValue) -> u64 {
    rows.members().filter_map(|r| r["size"].as_u64()).sum()
}
fn free(p: &Path) -> Result<u64, NativeError> {
    Ok(crate::remote::owner::disk_space(p)?.1)
}
fn info(p: &Path) -> Result<JsonValue, NativeError> {
    fn size(p: &Path, budget: &mut usize) -> Result<u64, NativeError> {
        if *budget == 0 {
            return Err(changed());
        }
        *budget -= 1;
        let m = fs::symlink_metadata(p)?;
        if m.file_type().is_symlink() {
            return Ok(0);
        }
        if m.is_file() {
            return Ok(m.len());
        }
        let mut total = 0u64;
        if m.is_dir() {
            for entry in fs::read_dir(p)? {
                total = total
                    .checked_add(size(&entry?.path(), budget)?)
                    .ok_or_else(changed)?;
            }
        }
        Ok(total)
    }
    let total = size(p, &mut 100_000)?;
    let (used, available) = crate::remote::owner::disk_space(p)?;
    Ok(object! {path:p.to_str().ok_or_else(changed)?,bytes:total,driveUsedBytes:used,freeBytes:available})
}
/// Probe the actual mount, rather than infer POSIX behavior from its name.
fn probe(target: &Path) -> Result<(), NativeError> {
    let dir = target.join(format!(".storage-probe-{}", paths::random_id()?));
    paths::private_dir(&dir)?;
    let probe_identity = identity(&dir)?;
    let mut owned = std::collections::BTreeMap::new();
    let result = (|| {
        let a = dir.join("case");
        let b = dir.join("CASE");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&a)?;
        owned.insert("case".to_owned(), identity(&a)?);
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        if file.metadata()?.mode() & 0o777 != 0o600
            || fs::metadata(&dir)?.mode() & 0o777 != 0o700
            || fs::metadata(target)?.uid() != unsafe { geteuid() }
        {
            return Err(changed());
        }
        file.write_all(b"storage probe")?;
        file.sync_all()?;
        let other = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&b)?;
        owned.insert("CASE".to_owned(), identity(&b)?);
        other.sync_all()?;
        let link = dir.join("link");
        fs::hard_link(&a, &link)?;
        owned.insert("link".to_owned(), identity(&link)?);
        if identity(&a)? != identity(&link)? {
            return Err(changed());
        }
        fs::rename(&a, dir.join("renamed"))?;
        let original = owned.remove("case").ok_or_else(changed)?;
        owned.insert("renamed".to_owned(), original);
        paths::sync_dir(&dir)?;
        paths::sync_dir(target)?;
        Ok(())
    })();
    cleanup_probe(&dir, &probe_identity, &mut owned)?;
    paths::sync_dir(target)?;
    result.map_err(|_:NativeError|NativeError::new("storageUnsupported", "This folder must support case-sensitive names, hard links, rename, file sync and directory sync. Check drive permissions and mount options."))
}
fn cleanup_probe(dir:&Path,probe_identity:&JsonValue,owned:&mut std::collections::BTreeMap<String,JsonValue>)->Result<(),NativeError>{
    // Recheck the directory and every remaining child before each unlink.
    for name in owned.keys().cloned().collect::<Vec<_>>() {
        if identity(&dir)? != *probe_identity { return Err(changed()); }
        let actual = fs::read_dir(&dir)?.map(|e| {
            let e=e?; Ok((e.file_name().to_string_lossy().into_owned(),identity(&e.path())?))
        }).collect::<Result<std::collections::BTreeMap<_,_>,NativeError>>()?;
        if actual != *owned { return Err(changed()); }
        fs::remove_file(dir.join(&name))?;
        owned.remove(&name);
    }
    if identity(&dir)? != *probe_identity { return Err(changed()); }
    fs::remove_dir(&dir)?;
    Ok(())
}
fn validate(store: &Store, target: &Path) -> Result<PathBuf, NativeError> {
    let target = paths::absolute(target)?;
    paths::reject_symlinks(&target)?;
    let old = store.storage_root()?;
    let doc = store.document()?;
    let game = doc["gamePath"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(Path::new);
    if !target.is_dir()
        || target.starts_with(store.root())
        || store.root().starts_with(&target)
        || target.starts_with(&old)
        || old.starts_with(&target)
        || game.is_some_and(|g| target.starts_with(g) || g.starts_with(&target))
        || fs::read_dir(&target)?.next().is_some()
    {
        return Err(NativeError::new(
            "storageInvalid",
            "Choose an existing empty folder outside the library and Deadlock folders.",
        ));
    }
    for base in [store.root(), old.as_path()] {
        for entry in fs::read_dir(base)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".import-")
                || name.starts_with(".remote-claim-")
                || name.starts_with(".archive-")
                || name.starts_with(".local-quarantine-")
                || name.starts_with(".remote-quarantine-")
                || name.starts_with(".remote-skips-v1-")
            {
                return Err(NativeError::new(
                    "recoveryRequired",
                    "Finish downloads and archive recovery before moving storage.",
                ));
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        for game in crate::installed::detect(Path::new(&home))["candidates"]
            .members()
            .filter_map(|v| v.as_str())
        {
            if target.starts_with(game) || Path::new(game).starts_with(&target) {
                return Err(NativeError::new(
                    "storageInvalid",
                    "Choose a folder outside Deadlock.",
                ));
            }
        }
    }
    probe(&target)?;
    Ok(target)
}
// Check checkpoint backups before creating any move attempt. A legacy backup
// outside a provable filesystem identity must not fail only after the copy.
fn verify_move_font_backups(doc:&JsonValue,source:&Path)->Result<(),NativeError>{
    let state=crate::fonts::FontState::decode(doc)?;
    if let Some(checkpoint)=state.previous {
        for image in checkpoint.images {
            if let crate::fonts::StoredFontImage::Present{backup,identity,..}=image.image {
                let path=source.join(format!("font-backups/{}.blob",backup.id));
                let actual=crate::fonts::transaction_read(&path,crate::font_content::MAX_FONT_BYTES)?;
                if identity.fsid.is_none() && identity.device!=actual.identity().device {
                    return Err(NativeError::new("storageChanged","This older font Undo backup cannot be verified after the filesystem changed. Preserve the backup and resolve its font checkpoint before moving storage."));
                }
                if !crate::fonts::fingerprint_matches(&path,actual.identity(),&identity,None)? || actual.mode()!=0o400 {return Err(changed());}
            }
        }
    }
    Ok(())
}
fn review(store: &Store, target: &Path) -> Result<JsonValue, NativeError> {
    crate::apply::ensure_idle(store)?;
    crate::game_access::ensure_game_idle()?;
    let source = store.storage_root()?;
    verify_move_font_backups(&store.document()?,&source)?;
    let target = validate(store, target)?;
    let rows = files(&source)?;
    let required = bytes(&rows)
        .checked_add(64 * 1024 * 1024)
        .ok_or_else(changed)?;
    if free(&target)? < required {
        return Err(NativeError::new(
            "storageFull",
            "The destination needs room for the complete copy plus 64 MB of headroom.",
        ));
    }
    let mut report = object! {source:info(&source)?,destination:info(&target)?,sourceIdentity:identity(&source)?,destinationIdentity:identity(&target)?,bytesToMove:bytes(&rows),files:rows,revision:store.document()?["revision"].clone()};
    let authority = object! {source:source.to_str(),destination:target.to_str(),sourceIdentity:report["sourceIdentity"].clone(),destinationIdentity:report["destinationIdentity"].clone(),files:report["files"].clone(),revision:report["revision"].clone()};
    report["token"] = paths::digest_bytes(store::canonical(&authority).as_bytes())?.into();
    Ok(report)
}
fn verify(path: &Path, row: &JsonValue, source: bool, lock:&RootLock) -> Result<(), NativeError> {
    if source && !identity_matches(path,&row["identity"],Some(lock))? {
        return Err(changed());
    }
    let f = paths::fingerprint(path)?;
    if row["mode"].as_u32().is_some_and(|mode|fs::metadata(path).map_or(true,|m|m.mode() & 0o777 != mode)) {return Err(changed());}
    if row["sha256"] != f.sha256 || row["size"] != f.size {
        return Err(changed());
    }
    Ok(())
}
fn progress(phase: &str, done: u64, total: u64) {
    eprintln!(
        "{}",
        object! {storageProgress:phase,done:done,total:total}.dump()
    );
}
fn finish(
    st: &Store,
    lock: &RootLock,
    id: &str,
    j: &mut JsonValue,
) -> Result<JsonValue, NativeError> {
    if j["kind"] != "storageMove" || j["version"] != 1 || j["input"]["params"]["operationId"] != id
    {
        return Err(changed());
    }
    let source = location_with_lock(&j["source"],Some(lock))?;
    let destination = location_with_lock(&j["destination"],Some(lock))?;
    let active = st.document()?["storageLocation"].clone();
    let switched = active == j["destination"] || (j["copyDirectory"].is_object() && active == j["copyDirectory"]);
    if !switched && st.document()? != j["before"] { return Err(changed()); }
    if !switched {verify_move_font_backups(&j["before"],&source)?;}
    let target = if switched {
        location_with_lock(&active,Some(lock))?
    } else {
        // Historical identities are never authority to mutate an earlier copy.
        keep_attempt(st, j)?;
        let dir = destination.join(format!(".storage-copy-{}", paths::random_id()?));
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        paths::sync_dir(&destination)?;
        j["copyDirectory"] = object!{path:dir.to_str(),identity:identity(&dir)?};
        for row in j["files"].members_mut() {
            row.remove("copyIdentity"); row.remove("copyIntent"); row.remove("copied");
        }
        // Durable directory identity precedes the creation of every copied file.
        st.write_journal(lock, id, j)?;
        dir
    };
    if !switched && !j["files"].members().any(|r|r["name"]=="remote-skips.v1.json") {
        if let Some(skip)=files(&source)?.members().find(|r|r["name"]=="remote-skips.v1.json") {
            j["files"].push(skip.clone()).map_err(|_|changed())?;st.write_journal(lock,id,j)?;
        }
    }
    let total = bytes(&j["files"]);
    let mut copied = 0u64;
    progress("copy", 0, total);
    for n in 0..j["files"].len() {
        location_with_lock(&j["source"],Some(lock))?;location_with_lock(&j["destination"],Some(lock))?;
        crate::game_access::ensure_game_idle()?;
        let row = j["files"][n].clone();
        let name = Path::new(row["name"].as_str().ok_or_else(changed)?);
        if !DIRS.iter().any(|d| name.starts_with(d))
            || !name.is_relative()
            || name
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(changed());
        }
        let src = source.join(name);
        let dst = target.join(name);
        if !switched {
            verify(&src, &row, true,lock)?;
            location_with_lock(&j["copyDirectory"],Some(lock))?;
            paths::private_dir(dst.parent().ok_or_else(changed)?)?;
            let mut out = OpenOptions::new().write(true).create_new(true).mode(0o600)
                .custom_flags(0x20000).open(&dst)?;
            #[cfg(test)]
            COPY_CREATED.with(|h|if let Some(h)=h.borrow_mut().take(){h(&dst);});
            let born = out.metadata()?;
            let birth = object!{device:born.dev(),inode:born.ino(),fsid:paths::persisted_filesystem_id(&out)?};
            j["files"][n]["copyIdentity"] = birth.clone();
            st.write_journal(lock,id,j)?;
            #[cfg(test)]
            BEFORE_COPY_OPEN.with(|h|if let Some(h)=h.borrow_mut().take(){h(&dst);});
            if !identity_matches(&dst,&birth,Some(lock))? { return Err(changed()); }
            let mut input = paths::open_regular(&src)?;
            let mut buffer = [0u8; 1024 * 1024];
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 { break; }
                out.write_all(&buffer[..count])?;
            }
            out.set_permissions(fs::Permissions::from_mode(row["mode"].as_u32().unwrap_or(fs::metadata(&src)?.mode() & 0o777)))?;
            out.sync_all()?;
            paths::sync_dir(dst.parent().ok_or_else(changed)?)?;
            verify(&src,&row,true,lock)?;
            j["files"][n]["copied"] = true.into();
            st.write_journal(lock,id,j)?;
        }
        copied += row["size"].as_u64().ok_or_else(changed)?;
        progress("copy", copied, total);
        verify(&dst, &row, false,lock)?;
        if !identity_matches(&dst,&j["files"][n]["copyIdentity"],Some(lock))? {
            return Err(changed());
        }
    }
    progress("switch", total, total);
    if !switched {
        let current = files(&source)?;
        let mut expected = j["files"].clone();
        for r in expected.members_mut() {
            r.remove("copyIdentity");
            r.remove("copied");
            r.remove("copyIntent");
        }
        for r in expected.members_mut() {
            let fresh=current.members().find(|v|v["name"]==r["name"]).ok_or_else(changed)?;
            let path=source.join(r["name"].as_str().ok_or_else(changed)?);
            if !identity_matches(&path,&r["identity"],Some(lock))? {return Err(changed());}
            if !r["fingerprint"].is_null() {
                let old=&r["fingerprint"];let next=&fresh["fingerprint"];
                if old["device"]!=r["identity"]["device"] || old["inode"]!=r["identity"]["inode"]
                    || ["sha256","size","inode","modifiedSeconds","modifiedNanos"].iter().any(|k|old[*k]!=next[*k]) {return Err(changed());}
            }
            r["identity"]=fresh["identity"].clone();
            r["fingerprint"]=fresh["fingerprint"].clone();
            if r["mode"].is_null(){r["mode"]=fresh["mode"].clone();}
        }
        if current != expected {
            return Err(changed());
        }
        let mut doc = j["before"].clone();
        doc["storageLocation"] = j["copyDirectory"].clone();
        // Rebind only checkpoint backup images whose old identity was captured
        // in this move journal. Content and mode were verified above.
        fn rebind(v:&mut JsonValue,rows:&JsonValue,target:&Path,source:&Path)->Result<(),NativeError>{
            if v.is_object() {
                if v["kind"]=="present" && v["backup"]["id"].is_string() {
                    let name=format!("font-backups/{}.blob",v["backup"]["id"].as_str().unwrap());
                    let row=rows.members().find(|r|r["name"]==name).ok_or_else(changed)?;
                    let mut old=v["identity"].clone();
                    if old.has_key("fsid") {
                        if !crate::generation::valid_fsid(&old){return Err(changed());}
                        let file=paths::open_regular(&source.join(&name))?;
                        if old["fsid"].as_str()!=Some(paths::persisted_filesystem_id(&file)?.as_str()){return Err(changed());}
                        // The stored fsid proves the filesystem; preserve all
                        // inode/content/timestamp evidence in this comparison.
                        old.remove("fsid");old["device"]=file.metadata()?.dev().into();
                    }
                    if !row["fingerprint"].is_null() && old!=row["fingerprint"] {return Err(changed());}
                    if row["fingerprint"].is_null() && old!=crate::fonts::fingerprint_json(&paths::fingerprint(&source.join(&name))?) {return Err(changed());}
                    let captured=crate::fonts::transaction_read(&target.join(name),crate::font_content::MAX_FONT_BYTES)?;
                    v["identity"]=crate::fonts::fingerprint_json(captured.identity());
                }
                for (_,child) in v.entries_mut(){rebind(child,rows,target,source)?;}
            } else if v.is_array(){for child in v.members_mut(){rebind(child,rows,target,source)?;}}
            Ok(())
        }
        rebind(&mut doc,&j["files"],&target,&source)?;
        st.commit(
            lock,
            store::revision(doc["revision"].as_str().ok_or_else(changed)?)?,
            id,
            &j["input"],
            doc,
            None,
        )?;
    }
    progress("cleanup", total, total);
    // Capture each live source parent's identity and child set. Recheck both,
    // plus the file identity and full content, immediately before every unlink.
    let mut parents = std::collections::BTreeMap::new();
    for row in j["files"].members() {
        let src = source.join(row["name"].as_str().ok_or_else(changed)?);
        if src.try_exists()? {
            let parent = src.parent().ok_or_else(changed)?.to_owned();
            parents.entry(parent.clone()).or_insert((identity(&parent)?, children(&parent)?));
        }
    }
    for row in j["files"].members() {
        location_with_lock(&j["source"],Some(lock))?;
        location_with_lock(&st.document()?["storageLocation"],Some(lock))?;
        crate::game_access::ensure_game_idle()?;
        let src = source.join(row["name"].as_str().ok_or_else(changed)?);
        if src.try_exists()? {
            verify(&target.join(row["name"].as_str().ok_or_else(changed)?),row,false,lock)?;
            #[cfg(test)]
            BEFORE_SOURCE_UNLINK.with(|h|if let Some(h)=h.borrow_mut().take(){h(&src);});
            verify(&src,row,true,lock)?;
            let parent = src.parent().ok_or_else(changed)?;
            let (owner, names) = parents.get_mut(parent).ok_or_else(changed)?;
            if identity(parent)? != *owner || children(parent)? != *names { return Err(changed()); }
            fs::remove_file(&src)?;
            names.remove(src.file_name().ok_or_else(changed)?);
            paths::sync_dir(parent)?;
        }
    }
    st.clear_journal(lock, id)?;
    info(&target)
}
fn cancel(st:&Store,lock:&RootLock,id:&str,j:&JsonValue)->Result<JsonValue,NativeError>{
    if j["kind"]!="storageMove" || j["version"]!=1 || st.document()?!=j["before"] {return Err(NativeError::new("storageChanged","This move has already switched locations. Resume it to finish."));}
    let source=location_with_lock(&j["source"],Some(lock))?;
    keep_attempt(st,j)?;
    let left = kept(st)?;
    st.clear_journal(lock,id)?;
    let messages = left.members().map(|r| {
        leftover_caution(r)
    }).collect::<Vec<_>>().join("\n");
    Ok(object!{current:info(&source)?,leftBehind:left,message:format!("Move cancelled. The source location is still active.\n{messages}")})
}
fn leftover_caution(row:&JsonValue)->String {
    let mb=row["bytes"].as_u64().unwrap_or(0).div_ceil(1024*1024);
    format!("Unfinished copy left in {} ({} MB). It may contain files you changed after the move stopped — check it before deleting.",row["path"].as_str().unwrap_or("unknown"),mb)
}
fn children(path:&Path)->Result<std::collections::BTreeSet<std::ffi::OsString>,NativeError>{
    fs::read_dir(path)?.map(|e|e.map(|e|e.file_name()).map_err(NativeError::from)).collect()
}
fn keep_attempt(st:&Store,j:&JsonValue)->Result<(),NativeError>{
    let db=st.launch_database();
    db.execute("CREATE TABLE IF NOT EXISTS storage_leftovers (path TEXT PRIMARY KEY)",&[])?;
    // The selected destination also covers old writers' directly published files.
    // This is a display-only catalog, never deletion authority.
    let mut paths = Vec::new();
    if let Some(base)=j["destination"]["path"].as_str(){
        for name in DIRS { paths.push(Path::new(base).join(name)); }
    }
    if let Some(base)=j["destination"]["path"].as_str() {
        // Also display an orphan from a crash between mkdir and journal sync.
        if let Ok(entries)=fs::read_dir(base) {
            for entry in entries {
                let entry=entry?;
                if entry.file_name().to_string_lossy().starts_with(".storage-copy-"){paths.push(entry.path());}
            }
        }
    }
    if let Some(path)=j["copyDirectory"]["path"].as_str(){paths.push(PathBuf::from(path));}
    for path in paths {
        if fs::symlink_metadata(&path).is_ok() {
            db.execute("INSERT OR IGNORE INTO storage_leftovers VALUES (?)",&[path.to_str().ok_or_else(changed)?])?;
        }
    }
    Ok(())
}
pub(crate) fn kept(st:&Store)->Result<JsonValue,NativeError>{
    let db=st.launch_database();
    if db.query("SELECT name FROM sqlite_master WHERE type='table' AND name='storage_leftovers'",&[])?.is_none(){return Ok(JsonValue::new_array());}
    let mut rows=JsonValue::new_array();
    for row in db.query_rows("SELECT path FROM storage_leftovers ORDER BY path",&[],100_000,16*1024*1024)? {
        let path=Path::new(&row[0]);
        if fs::symlink_metadata(path).is_ok() {
            let mut item=info(path).unwrap_or_else(|_|object!{path:row[0].as_str(),bytes:JsonValue::Null});
            item["kind"]="storageCopy".into();item["removable"]=false.into();
            item["reason"]=format!("Kept. {}",leftover_caution(&item)).into();
            rows.push(item).map_err(|_|changed())?;
        }
    }
    Ok(rows)
}

fn status_usage(
    store: &Store,
    mut measure: impl FnMut(&Path) -> Result<JsonValue, NativeError>,
) -> Result<(JsonValue, JsonValue), NativeError> {
    let storage = store.storage_root().and_then(|path| {
        let usage = measure(&path)?;
        Ok((path, usage))
    });
    match storage {
        Ok((path, usage)) if path == store.root() => Ok((usage.clone(), usage)),
        Ok((_, usage)) => Ok((usage, measure(store.root())?)),
        Err(_) => {
            let path = store.document().ok()
                .map(|doc| doc["storageLocation"]["path"].clone())
                .unwrap_or(JsonValue::Null);
            Ok((object! {path:path, unavailable:true}, measure(store.root())?))
        }
    }
}

pub fn dispatch(method: &str, p: &JsonValue) -> Result<JsonValue, NativeError> {
    let data = paths::data_root(p)?;
    if method == "native.storage.status" {
        let st = Store::open(&data)?;
        let journals = st.journals()?;
        let revision = st.document()?["revision"].clone();
        let (current, database) = status_usage(&st, info)?;
        return Ok(object! {
            revision: revision,
            current: current,
            database: database,
            canCancel: journals.members().any(|j| j["body"]["kind"] == "storageMove"
                && j["body"]["before"] == st.document().unwrap_or(JsonValue::Null)),
            resume: journals.members().any(|j| j["body"]["kind"] == "storageMove")
        });
    }
    if method == "native.storage.review" {
        let st=Store::open(&data)?;
        let target=Path::new(p["destination"].as_str().ok_or_else(NativeError::params)?);
        return review(&st,target);
    }
    let lock = RootLock::acquire(&data)?;
    let st = Store::open_writer(&lock)?;
    // Both preparation families share this slot even while a worker has no
    // library journal. The library writer lock excludes commit/Apply/Restore.
    let _slot = crate::remote::preparation_slot::PreparationSlot::try_acquire(&data)?;
    let _remote = crate::remote::job_store::RemoteStoreLease::acquire(&data)?;
    crate::game_access::ensure_game_idle()?;
    if method == "native.storage.resume" || method == "native.storage.cancel" {
        let journals = st.journals()?;
        if journals.len() != 1 {
            return Err(changed());
        }
        let entry = &journals[0];
        let id = entry["id"].as_str().ok_or_else(changed)?;
        if method == "native.storage.cancel" {return cancel(&st,&lock,id,&entry["body"]);}
        return finish(&st, &lock, id, &mut entry["body"].clone());
    }
    let target = Path::new(p["destination"].as_str().ok_or_else(NativeError::params)?);
    let report = review(&st, target)?;
    if method != "native.storage.move" || p["token"] != report["token"] {
        return Err(changed());
    }
    let id = p["operationId"]
        .as_str()
        .filter(|s| paths::safe_id(s))
        .ok_or_else(NativeError::params)?;
    let mut j = object! {kind:"storageMove",version:1,before:st.document()?,source:object!{path:report["source"]["path"].clone(),identity:report["sourceIdentity"].clone()},destination:object!{path:report["destination"]["path"].clone(),identity:report["destinationIdentity"].clone()},files:report["files"].clone(),input:object!{method:method,params:p.clone()}};
    st.write_journal(&lock, id, &j)?;
    finish(&st, &lock, id, &mut j)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round12_probe_keeps_foreign_children_and_replaced_parent() {
        for replace in [false,true] {
            let f=setup();let dir=f.root.join("probe");fs::create_dir(&dir).unwrap();
            fs::write(dir.join("owned"),b"probe").unwrap();let original=identity(&dir).unwrap();
            let mut owned=std::collections::BTreeMap::from([("owned".to_owned(),identity(&dir.join("owned")).unwrap())]);
            if replace { fs::rename(&dir,f.root.join("saved")).unwrap();fs::create_dir(&dir).unwrap();fs::hard_link(f.root.join("saved/owned"),dir.join("owned")).unwrap(); }
            else {fs::write(dir.join("foreign"),b"USER").unwrap();}
            assert!(cleanup_probe(&dir,&original,&mut owned).is_err());
            assert_eq!(fs::read(dir.join("owned")).unwrap(),b"probe");
            if !replace {assert_eq!(fs::read(dir.join("foreign")).unwrap(),b"USER");}
        }
    }
    use super::*;
    use crate::test_support::{import, Fixture};
    fn setup() -> Fixture {
        let f = Fixture::new();
        import(&mut crate::native::NativeService::new(), &f);
        f
    }
    fn p(f: &Fixture) -> JsonValue {
        object! {dataDir:f.data.to_str().unwrap()}
    }
    #[test]
    fn status_usage_measures_shared_root_once() {
        let f = setup();
        let store = Store::open(&f.data).unwrap();
        let mut paths = Vec::new();
        let (current, database) = status_usage(&store, |path| {
            paths.push(path.to_owned());
            info(path)
        }).unwrap();
        assert_eq!(paths, [f.data.clone()]);
        assert_eq!(current, database);
        assert_eq!(database["path"].as_str(), f.data.to_str());

        // A failed storage read cannot stand in for the separate database result.
        let mut attempts = 0;
        let (unavailable, database) = status_usage(&store, |path| {
            attempts += 1;
            if attempts == 1 { Err(changed()) } else { info(path) }
        }).unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(unavailable["unavailable"], true);
        assert_eq!(database["path"].as_str(), f.data.to_str());
        assert!(status_usage(&store, |_| Err(changed())).is_err());
    }

    fn journal(st: &Store, target: &Path) -> JsonValue {
        let r = review(st, target).unwrap();
        object! {kind:"storageMove",version:1,before:st.document().unwrap(),source:object!{path:r["source"]["path"].clone(),identity:r["sourceIdentity"].clone()},destination:object!{path:r["destination"]["path"].clone(),identity:r["destinationIdentity"].clone()},files:r["files"].clone(),input:object!{method:"native.storage.move",params:object!{operationId:"move"}}}
    }
    // Old writer's persisted copyIdentity could describe an adopted user inode.
    // Synthesize its v1 journal shape; no historical executable is needed.
    fn interrupted_user_copy(action: &str, published: bool) {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        let dir=target.join(".storage-copy-legacy");fs::create_dir(&dir).unwrap();
        j["copyDirectory"]=object!{path:dir.to_str(),identity:identity(&dir).unwrap()};
        let legacy=json::parse(include_str!("../../tests/fixtures/storage-legacy-adopted-copy.json")).unwrap();
        j["kind"]=legacy["kind"].clone();j["version"]=legacy["version"].clone();
        for (key,value) in legacy["files"][0].entries(){j["files"][0][key]=value.clone();}
        let row=j["files"][0].clone();let name=row["name"].as_str().unwrap();
        let expected=fs::read(f.data.join(name)).unwrap();
        let user=if published {target.join(name)}else{dir.join("0.copy")};
        fs::create_dir_all(user.parent().unwrap()).unwrap();fs::write(&user,b"USER FILE AFTER INTERRUPTION").unwrap();
        j["files"][0]["copyIntent"]=true.into();j["files"][0]["copyIdentity"]=identity(&user).unwrap();
        j["files"][0]["copied"]=published.into();st.write_journal(&lock,"move",&j).unwrap();
        if action=="cancel" {cancel(&st,&lock,"move",&j).unwrap();}
        else {finish(&st,&lock,"move",&mut j).unwrap();}
        assert_eq!(fs::read(&user).unwrap(),b"USER FILE AFTER INTERRUPTION","recovery must preserve earlier destination contents");
        assert!(dir.exists());
        let kept=crate::maintenance::dispatch("native.maintenance.scan",&p(&f)).unwrap();
        assert!(kept["rows"].members().any(|r|r["path"]==dir.to_str().unwrap() && r["removable"]==false));
        if action=="resume" {assert_eq!(fs::read(st.storage_root().unwrap().join(name)).unwrap(),expected);}
        else {assert_eq!(st.storage_root().unwrap(),f.data);assert!(st.journals().unwrap().is_empty());}
    }
    #[test]
    fn round10_source_final_unlink_checks() {
        for mutation in 0..3 {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
            let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
            st.write_journal(&lock,"move",&j).unwrap();
            BEFORE_SOURCE_UNLINK.with(|h|*h.borrow_mut()=Some(Box::new(move|src|{
                if mutation==0 { fs::set_permissions(src,fs::Permissions::from_mode(0o600)).unwrap();fs::write(src,b"USER EDIT").unwrap(); }
                else if mutation==1 {fs::write(src.parent().unwrap().join("foreign"),b"keep").unwrap();}
                else {let parent=src.parent().unwrap();let held=parent.with_extension("held");fs::rename(parent,&held).unwrap();fs::create_dir(parent).unwrap();for e in fs::read_dir(held).unwrap(){let p=e.unwrap().path();fs::hard_link(&p,parent.join(p.file_name().unwrap())).unwrap();}}
            })));
            assert!(finish(&st,&lock,"move",&mut j).is_err());
            BEFORE_SOURCE_UNLINK.with(|h|assert!(h.borrow().is_none()));
            assert!(f.data.join(j["files"][0]["name"].as_str().unwrap()).exists());
        }
    }
    #[test]
    fn round11_cancel_edited_copy_advice() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        let dir=target.join(".storage-copy-interrupted");fs::create_dir(&dir).unwrap();
        j["copyDirectory"]=object!{path:dir.to_str(),identity:identity(&dir).unwrap()};
        let user=dir.join(j["files"][0]["name"].as_str().unwrap());fs::create_dir_all(user.parent().unwrap()).unwrap();
        fs::write(&user,b"original").unwrap();j["files"][0]["copied"]=true.into();j["files"][0]["copyIdentity"]=identity(&user).unwrap();
        st.write_journal(&lock,"move",&j).unwrap();fs::write(&user,b"MY EDIT AFTER INTERRUPTION").unwrap();
        let report=cancel(&st,&lock,"move",&j).unwrap();
        assert_eq!(fs::read(&user).unwrap(),b"MY EDIT AFTER INTERRUPTION");
        let caution="It may contain files you changed after the move stopped — check it before deleting.";
        assert!(report["message"].as_str().unwrap().contains(caution),"{}",report["message"]);
        assert!(!report["message"].as_str().unwrap().to_lowercase().contains("can delete"));
        let scan=crate::maintenance::dispatch("native.maintenance.scan",&p(&f)).unwrap();
        let row=scan["rows"].members().find(|r|r["path"]==dir.to_str().unwrap()).unwrap();
        assert_eq!(row["removable"],false);assert!(row["reason"].as_str().unwrap().contains(caution));
    }
    #[test] fn round10_storage_legacy_adopted_inode() {for action in ["resume","cancel"] {interrupted_user_copy(action,false);}}
    #[test] fn round10_storage_cancel_user_file() {interrupted_user_copy("cancel",true);}
    #[test] fn round10_storage_resume_edit() {interrupted_user_copy("resume",false);}
    #[test]
    fn round9_cancel_keeps_unproven_directory_and_refuses_after_switch() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        let dir=target.join(".storage-copy-test");fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        j["copyDirectory"]=object!{path:dir.to_str(),identity:identity(&dir).unwrap()};
        fs::rename(&dir,target.join("held")).unwrap();fs::create_dir(&dir).unwrap();fs::write(dir.join("foreign"),b"keep").unwrap();st.write_journal(&lock,"move",&j).unwrap();
        assert!(!cancel(&st,&lock,"move",&j).unwrap()["leftBehind"].is_empty());assert_eq!(fs::read(dir.join("foreign")).unwrap(),b"keep");assert!(st.journals().unwrap().is_empty());
        let mut switched=st.document().unwrap();switched["storageLocation"]=j["destination"].clone();st.commit(&lock,1,"switch",&object!{},switched,None).unwrap();st.write_journal(&lock,"move",&j).unwrap();
        assert!(cancel(&st,&lock,"move",&j).is_err());assert!(!st.journals().unwrap().is_empty());
    }
    #[test]
    fn round9_copy_intent_does_not_own_foreign_inode() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        let dir=target.join(".storage-copy-0123456789abcdef0123456789abcdef");fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        j["copyDirectory"]=object!{path:dir.to_str(),identity:identity(&dir).unwrap()};
        j["files"][0]["copyIntent"]=true.into();
        st.write_journal(&lock,"move",&j).unwrap();
        let foreign=dir.join("0.copy");fs::write(&foreign,b"FOREIGN CONTENT MUST SURVIVE").unwrap();
        let name=j["files"][0]["name"].as_str().unwrap().to_owned();let wanted=fs::read(f.data.join(&name)).unwrap();assert_ne!(wanted,b"FOREIGN CONTENT MUST SURVIVE");

        assert_eq!(fs::read(&foreign).unwrap(),b"FOREIGN CONTENT MUST SURVIVE");
        assert_eq!(fs::read(f.data.join(name)).unwrap(),wanted);
        let owned=dir.join("1.copy");fs::write(&owned,b"OWNED STAGING").unwrap();j["files"][1]["copyIdentity"]=identity(&owned).unwrap();st.write_journal(&lock,"move",&j).unwrap();
        let report=cancel(&st,&lock,"move",&j).unwrap();
        assert!(owned.exists());assert_eq!(fs::read(&foreign).unwrap(),b"FOREIGN CONTENT MUST SURVIVE");
        assert!(!report["leftBehind"].is_empty());assert!(st.journals().unwrap().is_empty());assert_eq!(st.storage_root().unwrap(),f.data);
        drop(st);drop(lock);
        let snap=crate::native::NativeService::new().dispatch("native.snapshot",&p(&f)).unwrap();assert_eq!(snap["recoveryRequired"],false);
    }
    #[test]
    fn review7_unintended_private_candidate_is_kept() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        let dir=target.join(".storage-copy-0123456789abcdef0123456789abcdef");fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        j["copyDirectory"]=object!{path:dir.to_str(),identity:identity(&dir).unwrap()};
        assert!(j["files"][0]["copyIntent"].is_null());
        st.write_journal(&lock,"move",&j).unwrap();
        let foreign=dir.join("0.copy");fs::write(&foreign,b"FOREIGN CONTENT MUST SURVIVE").unwrap();
        let name=j["files"][0]["name"].as_str().unwrap().to_owned();let wanted=fs::read(f.data.join(&name)).unwrap();assert_ne!(wanted,b"FOREIGN CONTENT MUST SURVIVE");
        finish(&st,&lock,"move",&mut j).unwrap();
        assert_eq!(fs::read(&foreign).unwrap(),b"FOREIGN CONTENT MUST SURVIVE");
        assert_eq!(fs::read(st.storage_root().unwrap().join(name)).unwrap(),wanted);
        assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn round3_storage_promotion_never_overwrites_foreign_file() {
        interrupted_user_copy("resume",true);
    }
    #[test]
    fn round3_new_game_proofs_require_downgrade_barrier() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let mut doc=st.document().unwrap();doc["restoredSetup"]=object!{upstreamPayloads:json::array![object!{proofKind:"mount",sourcePath:"/fixture/game/citadel/addons",files:json::array![]}]};
        st.commit(&lock,1,"proof",&object!{},doc,None).unwrap();
        let row=st.launch_database().query("SELECT body FROM library WHERE id=1",&[]).unwrap().unwrap();
        assert_eq!(json::parse(&row[0]).unwrap()["storeVersion"],5,"new game proof requires old-helper refusal");
    }
    #[test]
    fn round3_copy_permission_never_follows_substitution() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);st.write_journal(&lock,"move",&j).unwrap();
        let victim=f.root.join("foreign");fs::write(&victim,b"foreign").unwrap();fs::set_permissions(&victim,fs::Permissions::from_mode(0o400)).unwrap();let v=victim.clone();
        BEFORE_COPY_OPEN.with(|h|*h.borrow_mut()=Some(Box::new(move|p|{fs::remove_file(p).unwrap();std::os::unix::fs::symlink(&v,p).unwrap();})));
        assert!(finish(&st,&lock,"move",&mut j).is_err());assert_eq!(fs::metadata(victim).unwrap().mode()&0o777,0o400,"foreign permissions changed");
    }
    #[test]
    fn round3_legacy_move_with_skip_history() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();
        fs::write(f.data.join("remote-skips.v1.json"),b"history").unwrap();let mut j=journal(&st,&target);
        j["files"]=j["files"].members().filter(|r|r["name"]!="remote-skips.v1.json").cloned().collect::<Vec<_>>().into();
        for r in j["files"].members_mut(){r.remove("mode");r.remove("fingerprint");}
        st.write_journal(&lock,"move",&j).unwrap();finish(&st,&lock,"move",&mut j).unwrap();
        assert_eq!(fs::read(st.storage_root().unwrap().join("remote-skips.v1.json")).unwrap(),b"history");
    }
    #[test]
    fn round3_move_recovers_create_before_identity() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);st.write_journal(&lock,"move",&j).unwrap();
        COPY_CREATED.with(|h|*h.borrow_mut()=Some(Box::new(|_|panic!("crash after create_new"))));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||finish(&st,&lock,"move",&mut j))).is_err());
        j=st.journal("move").unwrap().unwrap();finish(&st,&lock,"move",&mut j).unwrap();assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn round4_empty_staging_intent_resumes() {
        for created in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
            let dir=target.join(".storage-copy-0123456789abcdef0123456789abcdef");
            j["copyDirectory"]=object!{path:dir.to_str(),identity:JsonValue::Null};st.write_journal(&lock,"move",&j).unwrap();
            if created {fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();}
            finish(&st,&lock,"move",&mut j).unwrap();assert_eq!(dir.exists(),created);
        }
    }
    #[test]
    fn round4_writer_open_sets_barrier_before_journal() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let logical=st.document().unwrap();
        st.launch_database().execute("UPDATE library SET body=? WHERE id=1",&[&logical.dump()]).unwrap();drop(st);drop(lock);
        let st=Store::open(&f.data).unwrap();let disk=||{let row=st.launch_database().query("SELECT body FROM library WHERE id=1",&[]).unwrap().unwrap();json::parse(&row[0]).unwrap()};assert_ne!(disk()["storeVersion"],5);
        let target=f.root.join("review-only");fs::create_dir(&target).unwrap();
        dispatch("native.storage.review",&object!{dataDir:f.data.to_str(),destination:target.to_str()}).unwrap();
        assert_ne!(disk()["storeVersion"],5,"storage review must remain read-only");drop(st);
        let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let row=st.launch_database().query("SELECT body FROM library WHERE id=1",&[]).unwrap().unwrap();let disk=json::parse(&row[0]).unwrap();assert_eq!(disk["storageDocumentVersion"],5);assert_eq!(st.journals().unwrap().len(),0);
    }
    #[test]
    fn round4_cleanup_missing_staging_resumes() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);st.write_journal(&lock,"move",&j).unwrap();
        finish(&st,&lock,"move",&mut j).unwrap();
        // Exact old crash window: staging removed, journal still durable.
        st.write_journal(&lock,"move",&j).unwrap();
        finish(&st,&lock,"move",&mut j).expect("missing staging after switch must resume");
    }
    #[test]
    fn unfinished_skip_update_blocks_storage_move() {
        let f=setup();let st=Store::open(&f.data).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();
        fs::write(f.data.join(".remote-skips-v1-pending"),b"preserve").unwrap();
        assert!(validate(&st,&target).is_err());
    }
    #[test]
    fn font_undo_survives_storage_move(){font_undo_move(false,None)}
    #[test]
    fn item93_font_fsid_undo_survives_storage_move(){font_undo_move(true,None)}
    #[test]
    fn item93_review_r5_legacy_backup_refuses_before_copy(){
        for resume in [false,true]{font_undo_move(false,Some(resume));}
    }
    fn font_undo_move(stable:bool,legacy_refusal:Option<bool>) {
        use crate::fonts::*;
        use std::os::unix::fs::PermissionsExt;
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let dir=f.game.join("game/citadel/panorama/fonts");fs::create_dir_all(&dir).unwrap();
        paths::private_dir(&f.data.join("font-backups")).unwrap();let id="abcdefabcdefabcdefabcdefabcdefab";let path=f.data.join(format!("font-backups/{id}.blob"));
        let bytes=b"<fontconfig></fontconfig>\n";fs::write(&path,bytes).unwrap();fs::set_permissions(&path,fs::Permissions::from_mode(0o400)).unwrap();let fp:FontFingerprint=if stable{transaction_read(&path,crate::font_content::MAX_FONT_BYTES).unwrap().identity}else{paths::fingerprint(&path).unwrap().into()};
        let current=FontFootprint{root_path:f.data.clone(),root_identity:directory_identity(&f.data).unwrap(),game_path:f.game.clone(),game_identity:directory_identity(&f.game).unwrap(),directory:Some(directory_identity(&dir).unwrap()),directory_created:false,files:vec![],conf:None,conf_mode:None,owned_block:None};
        let mut prior=current.clone();prior.conf=Some(fp.clone().into());prior.conf_mode=Some(0o644);
        let state=FontState{installed:Some(current),previous:Some(FontCheckpoint{transaction_id:"1234567890abcdef1234567890abcdef".into(),prior:Some(prior),images:vec![FontImage{target:FontTarget::Configuration,image:StoredFontImage::Present{backup:crate::package::FileRef{id:id.into(),sha256:fp.sha256.clone(),size:fp.size,source_name:"fonts.conf".into()},identity:fp.into(),mode:0o644}}]})};
        let mut doc=st.document().unwrap();doc["packageVersion"]=crate::package::layout_version(&doc).unwrap().into();doc["storeVersion"]=3.into();doc["fonts"]=state.encode().unwrap();st.commit(&lock,1,"seed-font",&object!{},doc,None).unwrap();
        FontPlan::review(&st,&f.game,FontChange::Undo).unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        if stable || legacy_refusal.is_some(){
            let mut doc=st.document().unwrap();let id=&mut doc["fonts"]["previous"]["images"][0]["image"]["identity"];id["device"]=(id["device"].as_u64().unwrap()+1).into();
            let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();j["before"]=doc;
        }
        if let Some(resume)=legacy_refusal {
            let before=st.document().unwrap();let original=fs::read(&path).unwrap();
            let result=if resume{finish(&st,&lock,"move",&mut j)}else{review(&st,&target)};
            assert!(result.is_err(),"legacy backup must be refused before copying");
            assert!(fs::read_dir(&target).unwrap().next().is_none(),"no copied payload or copy directory may exist");assert!(j["copyDirectory"].is_null());assert_eq!(st.document().unwrap(),before);assert_eq!(fs::read(&path).unwrap(),original);assert!(st.journals().unwrap().is_empty());return;
        }
        st.write_journal(&lock,"move",&j).unwrap();finish(&st,&lock,"move",&mut j).unwrap();
        if stable {let mut doc=st.document().unwrap();let identity=&mut doc["fonts"]["previous"]["images"][0]["image"]["identity"];assert!(identity["fsid"].is_string(),"moved backup retains boot-stable identity");identity["device"]=(identity["device"].as_u64().unwrap()+1).into();let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();db.execute("UPDATE library SET body=? WHERE id=1",&[&doc.dump()]).unwrap();}
        let undo=FontPlan::review(&st,&f.game,FontChange::Undo).unwrap();let input=object!{method:"native.fonts.commit",params:object!{operationId:"undo",expectedRevision:"3",token:paths::random_id().unwrap()}};
        undo.commit(&st,&lock,3,"undo",&input).unwrap();assert_eq!(fs::read(dir.join("fonts.conf")).unwrap(),bytes);
    }
    #[test]
    fn item48_old_font_receipt_recovers_without_folder_change() {
        use crate::fonts::*;
        use std::os::unix::fs::PermissionsExt;
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n").unwrap();
        let new=f.game.clone();
        st.commit(&lock,1,"seed",&object!{},st.document().unwrap(),None).unwrap();
        let dir=new.join("game/citadel/panorama/fonts");fs::create_dir_all(&dir).unwrap();
        paths::private_dir(&f.data.join("font-backups")).unwrap();let id="abcdefabcdefabcdefabcdefabcdefab";let path=f.data.join(format!("font-backups/{id}.blob"));
        let bytes=b"<fontconfig></fontconfig>\n";fs::write(&path,bytes).unwrap();fs::set_permissions(&path,fs::Permissions::from_mode(0o400)).unwrap();let fp=paths::fingerprint(&path).unwrap();
        let current=FontFootprint{root_path:f.data.clone(),root_identity:directory_identity(&f.data).unwrap(),game_path:new.clone(),game_identity:directory_identity(&new).unwrap(),directory:Some(directory_identity(&dir).unwrap()),directory_created:false,files:vec![],conf:None,conf_mode:None,owned_block:None};
        let mut prior=current.clone();prior.conf=Some(fp.clone().into());prior.conf_mode=Some(0o644);
        let state=FontState{installed:Some(current),previous:Some(FontCheckpoint{transaction_id:"1234567890abcdef1234567890abcdef".into(),prior:Some(prior),images:vec![FontImage{target:FontTarget::Configuration,image:StoredFontImage::Present{backup:crate::package::FileRef{id:id.into(),sha256:fp.sha256.clone(),size:fp.size,source_name:"fonts.conf".into()},identity:fp.into(),mode:0o644}}]})};
        let mut doc=st.document().unwrap();doc["packageVersion"]=crate::package::layout_version(&doc).unwrap().into();doc["storeVersion"]=4.into();doc["fonts"]=state.encode().unwrap();st.commit(&lock,2,"seed-font",&object!{},doc,None).unwrap();
        let undo=FontPlan::review(&st,&new,FontChange::Undo).unwrap();
        let input=object!{method:"native.fonts.commit",params:object!{operationId:"undo",expectedRevision:"3",token:paths::random_id().unwrap()}};
        crate::font_transaction::AFTER_DATABASE.with(|h|*h.borrow_mut()=Some(Box::new(||panic!("crash before font cleanup"))));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||undo.commit(&st,&lock,3,"undo",&input))).is_err());
        assert_eq!(st.operation("undo").unwrap()["status"],"succeeded");
        let mut old=st.document().unwrap();old.remove("gameFolderChanged");
        let receipt=crate::store::snapshot_document(&old,None).unwrap();
        st.launch_database().execute("UPDATE operations SET result=? WHERE id='undo'",&[&object!{status:"succeeded",result:receipt}.dump()]).unwrap();
        crate::apply::recover(&st,&lock,4,"recover-font",&object!{method:"native.recover",params:object!{operationId:"recover-font",expectedRevision:"4"}}).unwrap();
        assert!(st.journals().unwrap().is_empty());assert_eq!(fs::read(dir.join("fonts.conf")).unwrap(),bytes);
        drop(st);drop(lock);
        assert_eq!(Store::open(&f.data).unwrap().snapshot(None).unwrap()["recoveryRequired"],false);
    }
    #[test]
    fn move_includes_remote_skip_history() {
        let f=setup();fs::write(f.data.join("remote-skips.v1.json"),b"history").unwrap();
        assert!(files(&f.data).unwrap().members().any(|r|r["name"]=="remote-skips.v1.json"));
    }
    #[test]
    fn move_preserves_readonly_backups_and_skipped_updates() {
        use std::os::unix::fs::PermissionsExt;
        let f=setup(); let lock=RootLock::acquire(&f.data).unwrap(); let st=Store::open_writer(&lock).unwrap();
        paths::private_dir(&f.data.join("font-backups")).unwrap();
        let backup=f.data.join("font-backups/test.blob");fs::write(&backup,b"backup").unwrap();fs::set_permissions(&backup,fs::Permissions::from_mode(0o400)).unwrap();
        fs::write(f.data.join("remote-skips.v1.json"),b"{\"version\":1}").unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);st.write_journal(&lock,"move",&j).unwrap();finish(&st,&lock,"move",&mut j).unwrap();
        assert_eq!(fs::metadata(st.storage_root().unwrap().join("font-backups/test.blob")).unwrap().mode()&0o777,0o400);
        assert!(st.storage_root().unwrap().join("remote-skips.v1.json").exists());
    }
    #[test]
    fn storage_move_rebinds_payload_inode_receipt() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let id="abcdefabcdefabcdefabcdefabcdefab";let path=st.payload_path(id).unwrap();fs::write(&path,b"owned").unwrap();let fp=paths::fingerprint(&path).unwrap();
        let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();db.execute("CREATE TABLE IF NOT EXISTS payload_inode_receipts (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, identity TEXT NOT NULL)",&[]).unwrap();db.execute("INSERT INTO payload_inode_receipts VALUES(?,?,?)",&[id,&fp.sha256,&crate::apply::identity(&fs::metadata(&path).unwrap()).dump()]).unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);st.write_journal(&lock,"move",&j).unwrap();finish(&st,&lock,"move",&mut j).unwrap();
        assert!(st.payload_owned(id,&fp.sha256,fp.size).unwrap(),"moving an owned inode must transfer its receipt");
    }
    #[test]
    fn legacy_2778_storage_journal_resumes() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        for row in j["files"].members_mut(){row.remove("mode");row.remove("fingerprint");}
        st.write_journal(&lock,"move",&j).unwrap();finish(&st,&lock,"move",&mut j).unwrap();
    }
    #[test]
    fn move_refuses_foreign_empty_file_after_intent() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let target=f.root.join("move");fs::create_dir(&target).unwrap();let mut j=journal(&st,&target);
        j["files"][0]["copyIntent"]=true.into();st.write_journal(&lock,"move",&j).unwrap();
        let dst=target.join(j["files"][0]["name"].as_str().unwrap());paths::private_dir(dst.parent().unwrap()).unwrap();OpenOptions::new().write(true).create_new(true).mode(0o600).open(&dst).unwrap().sync_all().unwrap();
        finish(&st,&lock,"move",&mut j).unwrap();
        assert_eq!(fs::metadata(&dst).unwrap().len(),0);
        assert!(st.storage_root().unwrap().join(j["files"][0]["name"].as_str().unwrap()).exists());
    }
    #[test]
    fn storage_barrier_only_for_move_journal_or_relocated_library() {
        let f = setup();
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        let disk = || -> JsonValue {
            json::parse(&st.launch_database().query("SELECT body FROM library WHERE id=1", &[]).unwrap().unwrap()[0]).unwrap()
        };
        let original = st.document().unwrap();
        assert_eq!(disk()["storeVersion"], 5);
        assert_eq!(disk()["storageDocumentVersion"],5);
        let target = f.root.join("other-drive");
        fs::create_dir(&target).unwrap();
        let mut j = journal(&st, &target);
        st.write_journal(&lock, "move", &j).unwrap();
        assert_eq!(disk()["storeVersion"], 5);
        assert_eq!(disk()["gameDocumentVersion"], original["storeVersion"]);
        assert_eq!(st.document().unwrap(), original);
        // Removing a pre-copy journal at the default location removes its barrier.
        st.clear_journal(&lock, "move").unwrap();
        assert_eq!(disk()["storeVersion"], 5);
        st.write_journal(&lock, "move", &j).unwrap();
        finish(&st, &lock, "move", &mut j).unwrap();
        assert!(st.journals().unwrap().is_empty());
        assert_eq!(disk()["storeVersion"], 5);
        assert_eq!(st.document().unwrap()["storeVersion"], original["storeVersion"]);
        // Subsequent writes and Apply/Restore must not strip the disk barrier.
        let mut doc = st.document().unwrap();
        doc["gamePath"] = "".into();
        st.commit(&lock, 2, "later", &object!{}, doc, None).unwrap();
        assert_eq!(disk()["storeVersion"], 5);
    }
    #[test]
    fn storage_validation_rejects_overlap_nonempty_symlink_and_stale_review() {
        let f = setup();
        let st = Store::open(&f.data).unwrap();
        assert!(validate(&st, &f.data).is_err());
        assert!(validate(&st, &f.game).is_err());
        let target = f.root.join("other-drive");
        fs::create_dir(&target).unwrap();
        let link = f.root.join("linked");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(validate(&st, &link).is_err());
        let r = review(&st, &target).unwrap();
        fs::write(target.join("foreign"), b"keep").unwrap();
        assert!(review(&st, &target).is_err());
        assert!(!r["token"].is_null());
        assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn storage_move_routes_payloads_and_maintenance_and_missing_mount_fails_closed() {
        let f = setup();
        let target = f.root.join("other-drive");
        fs::create_dir(&target).unwrap();
        let mut params = p(&f);
        params["destination"] = target.to_str().unwrap().into();
        let r = dispatch("native.storage.review", &params).unwrap();
        params["token"] = r["token"].clone();
        params["operationId"] = "move".into();
        dispatch("native.storage.move", &params).unwrap();
        let st = Store::open(&f.data).unwrap();
        assert!(st.storage_root().unwrap().starts_with(&target));
        assert!(st.journals().unwrap().is_empty());
        let orphan = st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();
        fs::write(&orphan, b"unused").unwrap();
        let r = crate::maintenance::dispatch("native.maintenance.scan", &p(&f)).unwrap();
        assert_eq!(r["count"], 0);
        assert!(r["rows"].members().any(|r|r["removable"]==false && r["id"]=="abcdefabcdefabcdefabcdefabcdefab"));
        let mut clean = p(&f);
        clean["token"] = r["token"].clone();
        clean["expectedRevision"] = r["revision"].clone();
        clean["operationId"] = "gc".into();
        assert!(crate::maintenance::dispatch("native.maintenance.clean", &clean).is_err());
        assert!(orphan.exists());
        fs::rename(&target, f.root.join("unmounted")).unwrap();
        assert!(st.payload_path("abcdefabcdefabcdefabcdefabcdefab").is_err());
    }
    #[test]
    fn item93_legacy_storage_move_recovers_changed_source_device() {
        let f=setup();let target=f.root.join("other-drive");fs::create_dir(&target).unwrap();
        let lock=RootLock::acquire(&f.data).unwrap();
        if !paths::test_volatile_filesystem(lock.directory()){return;}
        let st=Store::open_writer(&lock).unwrap();let mut j=journal(&st,&target);
        let old=fs::metadata(&f.data).unwrap().dev()+1;
        j["source"]["identity"].remove("fsid");j["source"]["identity"]["device"]=old.into();
        for row in j["files"].members_mut(){
            row["identity"].remove("fsid");row["identity"]["device"]=old.into();
            if !row["fingerprint"].is_null(){row["fingerprint"]["device"]=old.into();}
        }
        st.write_journal(&lock,"move",&j).unwrap();
        finish(&st,&lock,"move",&mut j).unwrap();
        assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn storage_recovery_before_copy_partial_copy_after_copy_and_switch() {
        for boundary in 0..5 {
            let f = setup();
            let target = f.root.join("other-drive");
            fs::create_dir(&target).unwrap();
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let mut j = journal(&st, &target);
            st.write_journal(&lock, "move", &j).unwrap();
            if boundary > 0 {
                for n in 0..j["files"].len() {
                    let row = j["files"][n].clone();
                    let name = row["name"].as_str().unwrap();
                    let dst = target.join(name);
                    paths::private_dir(dst.parent().unwrap()).unwrap();
                    fs::write(
                        &dst,
                        if boundary == 1 {
                            b"partial".to_vec()
                        } else {
                            fs::read(f.data.join(name)).unwrap()
                        },
                    )
                    .unwrap();
                    fs::set_permissions(&dst,fs::Permissions::from_mode(row["mode"].as_u32().unwrap())).unwrap();
                    j["files"][n]["copyIdentity"] = identity(&dst).unwrap();
                    if boundary > 1 {
                        j["files"][n]["copied"] = true.into();
                    }
                }
                st.write_journal(&lock, "move", &j).unwrap();
            }
            if boundary > 2 {
                let mut doc = j["before"].clone();
                doc["storageLocation"] = j["destination"].clone();
                st.commit(&lock, 1, "move", &j["input"], doc, None).unwrap();
            }
            if boundary > 3 {
                fs::remove_file(f.data.join(j["files"][0]["name"].as_str().unwrap())).unwrap();
            }
            drop(st);
            drop(lock);
            dispatch("native.storage.resume", &p(&f)).unwrap();
            let st = Store::open(&f.data).unwrap();
            assert!(st.storage_root().unwrap().starts_with(&target));
            assert!(st.journals().unwrap().is_empty());
        }
    }
    #[test]
    fn storage_refuses_busy_library_and_preserves_changed_or_unclaimed_files() {
        let f = setup();
        let target = f.root.join("other-drive");
        fs::create_dir(&target).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        assert!(RootLock::acquire(&f.data).is_err());
        let st = Store::open_writer(&lock).unwrap();
        let mut j = journal(&st, &target);
        st.write_journal(&lock, "move", &j).unwrap();
        let row = j["files"][0].clone();
        let name = row["name"].as_str().unwrap();
        let dst = target.join(name);
        paths::private_dir(dst.parent().unwrap()).unwrap();
        fs::write(&dst, b"unclaimed").unwrap();
        finish(&st, &lock, "move", &mut j).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"unclaimed");
        assert!(st.storage_root().unwrap().join(name).exists());
        assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn storage_move_then_apply_restore() {
        let f = setup();
        let target = f.root.join("other-drive");
        fs::create_dir(&target).unwrap();
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        let mut j = journal(&st, &target);
        st.write_journal(&lock, "move", &j).unwrap();
        finish(&st, &lock, "move", &mut j).unwrap();
        fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
        let plan = crate::apply::ApplyPlan::review(&st,"default",&f.game).unwrap();
        let input = object! {method:"native.apply",params:object!{operationId:"apply",expectedRevision:"2",dataDir:f.data.to_str().unwrap(),token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
        plan.apply(&st, &lock, 2, "apply", &input).unwrap();
        crate::apply::restore(&st,&lock,3,"restore",&object!{method:"native.restore",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"restore",expectedRevision:"3"}}).unwrap();
    }
}

#[cfg(test)]
#[test]
fn item93_storage_location_uses_recorded_filesystem_identity() {
    let fixture=crate::test_support::Fixture::new();
    let path=&fixture.root;
    let mut id=identity(path).unwrap();
    id["fsid"]=paths::persisted_filesystem_id(&std::fs::File::open(path).unwrap()).unwrap().into();
    id["device"]=(fs::metadata(path).unwrap().dev()+1).into();
    let mut location=object!{path:path.to_str().unwrap(),identity:id};
    assert!(self::location(&location).is_ok(),"filesystem ID and inode identify the storage location after a device change");
    location["identity"]["fsid"]="0000000000000001".into();
    assert!(self::location(&location).is_err(),"a mismatched filesystem ID is not a match");
}
