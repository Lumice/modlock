//! Conservative maintenance: captured Apply retirement and receipt-aware payload GC.
//! Unknown names and symlinks are evidence, never deletion authority.
//! Cleanup checks the reviewed SHA-256 again immediately before each unlink.
//! This detects in-place edits after preflight; it is a point-in-time check,
//! not a filesystem lock against a concurrent external writer.
use crate::{
    apply, game_access,
    native::NativeError,
    paths,
    store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{fs, os::unix::fs::MetadataExt, path::Path};
fn blocked() -> NativeError {
    NativeError::new(
        "maintenanceChanged",
        "The folders changed. Check again before cleaning up.",
    )
}
fn size(p: &Path, budget: &mut usize) -> Result<u64, NativeError> {
    if *budget == 0 {
        return Err(NativeError::new(
            "maintenanceLimit",
            "Too many files to review safely.",
        ));
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
        for e in fs::read_dir(p)? {
            total = total.saturating_add(size(&e?.path(), budget)?);
        }
    }
    Ok(total)
}
fn row(path: &Path, kind: &str, removable: bool) -> Result<JsonValue, NativeError> {
    Ok(
        object! {path:path.to_string_lossy().as_ref(),kind:kind,removable:removable,bytes:size(path,&mut 100_000).ok()},
    )
}
fn payloads(store: &Store, lock: Option<&RootLock>) -> Result<Vec<JsonValue>, NativeError> {
    let dir = crate::storage::path(store.root(), "payloads")?;
    let mut rows = Vec::new();
    if !dir.exists() {
        return Ok(rows);
    }
    paths::reject_symlinks(&dir)?;
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name();
        let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".vpk")) else {
            continue;
        };
        if !crate::package::hex(id, 32) {
            continue;
        }
        let path = e.path();
        let m = paths::open_regular(&path)?.metadata()?;
        let fp = paths::fingerprint(&path)?;
        let owned=store.payload_owned_with_lock(id,&fp.sha256,fp.size,lock)?;
        if owned && store.maintenance_referenced(id)? {continue;}
        rows.push(object!{path:path.to_string_lossy().as_ref(),kind:if owned {"payload"}else{"unproven"},removable:owned,reason:if owned {""}else{"Files the app can’t confirm it created — kept"},bytes:m.len(),id:id,device:m.dev(),inode:m.ino(),sha256:fp.sha256});
    }
    rows.sort_by_key(|r| r["path"].as_str().unwrap_or("").to_owned());
    Ok(rows)
}
// Presentation is separate from the exact per-file removal review.
fn display_rows(rows: &[JsonValue], doc: &JsonValue) -> Vec<JsonValue> {
    let mut result = Vec::new();
    for row in rows.iter().filter(|r| r["kind"] != "retired") {
        let mut item = row.clone();
        if let Some(id) = row["id"].as_str() {
            item["title"] = payload_title(doc, id).unwrap_or_default().into();
        }
        result.push(item);
    }
    for generation in doc["retiredGenerations"].members() {
        let Some(id) = generation["generation"].as_str() else { continue };
        let matching: Vec<_> = rows.iter().filter(|r| r["kind"] == "retired"
            && Path::new(r["path"].as_str().unwrap_or("")).components().any(|c| c.as_os_str() == id)).collect();
        if matching.is_empty() { continue }
        let bytes: u64 = matching.iter().filter_map(|r| r["bytes"].as_u64()).sum();
        let paths: Vec<_> = matching.iter().map(|r| r["path"].clone()).collect();
        // The captured manifest's timestamp survives retirement; no new date is invented.
        let date = generation["identities"]["manifest.json"]["modifiedSeconds"].clone();
        result.push(object!{kind:"retired",removable:true,bytes:bytes,paths:paths,date:date});
    }
    result
}
fn contains_payload_id(value: &JsonValue, id: &str) -> bool {
    if value.is_object() {
        value["id"].as_str() == Some(id) || value.entries().any(|(_, v)| contains_payload_id(v, id))
    } else { value.members().any(|v| contains_payload_id(v, id)) }
}
fn payload_title(value: &JsonValue, id: &str) -> Option<String> {
    if value.is_object() {
        if let Some(title) = value["title"].as_str().or_else(|| value["name"].as_str()).filter(|t| !t.is_empty()) {
            if contains_payload_id(value, id) { return Some(title.to_owned()) }
        }
        value.entries().find_map(|(_, v)| payload_title(v, id))
    } else { value.members().find_map(|v| payload_title(v, id)) }
}
fn scan(store: &Store) -> Result<JsonValue, NativeError> {
    let lock=RootLock::acquire_existing(store.root()).ok();
    scan_with_lock(store,lock.as_ref())
}
fn scan_with_lock(store: &Store, lock: Option<&RootLock>) -> Result<JsonValue, NativeError> {
    let doc = store.document()?;
    let journals = store.journals()?;
    let pending = journals
        .members()
        .any(|j| j["body"]["kind"] != "maintenance");
    let mut rows = crate::storage::kept(store)?.members().cloned().collect::<Vec<_>>();
    let mut warning = String::new();
    match apply::maintenance_rows(store) {
        Ok(r) => rows.extend(r.members().cloned()),
        Err(e) => warning = e.message,
    }
    for j in journals
        .members()
        .filter(|j| j["body"]["kind"] == "maintenance")
    {
        for r in j["body"]["payloads"].members() {
            if let Some(id) = r["id"].as_str() {
                let path = store.payload_path(id)?;
                if path.try_exists()? {
                    let mut row=r.clone();
                    if !journal_owned(store,r,lock)? {row["kind"]="unproven".into();row["removable"]=false.into();row["reason"]="Files the app can’t confirm it created — kept".into();}
                    rows.push(row);
                }
            }
        }
    }
    let mut library = if journals.is_empty() {
        payloads(store,lock)?
    } else {
        Vec::new()
    };
    rows.append(&mut library);
    if let Some(game) = doc["gamePath"].as_str().filter(|s| !s.is_empty()) {
        let c = Path::new(game).join("game/citadel");
        paths::reject_symlinks(&c)?;
        if c.is_dir() {
            for e in fs::read_dir(&c)? {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("addons")
                    || name == "dmm_localization_default"
                    || e.file_type()?.is_dir()
                        && ![
                            "deadlock_native",
                            "bin",
                            "cfg",
                            "maps",
                            "materials",
                            "models",
                            "resource",
                            "panorama",
                            "scripts",
                            "sound",
                            "sounds",
                            "shaders",
                            "media",
                            "particles",
                            "steam_settings",
                        ]
                        .contains(&name.as_str())
                {
                    rows.push(row(&e.path(), "foreign", false)?);
                }
                if name.starts_with(".native-gameinfo-") {
                    rows.push(row(&e.path(), "unproven", false)?);
                }
            }
        }
        let w = c.join("deadlock_native");
        if w.exists() {
            paths::reject_symlinks(&w)?;
            for e in fs::read_dir(&w)? {
                let e = e?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name == "owner.json" {
                    continue;
                }
                let known = std::iter::once(&doc["applied"])
                    .chain(std::iter::once(&doc["previous"]["applied"]))
                    .chain(doc["retiredGenerations"].members())
                    .any(|a| a["generation"].as_str() == Some(&name));
                let legacy=doc["retiredGenerations"].members().any(|a|a["generation"].as_str()==Some(&name) && (a["manifest"]["version"]!=3 && !apply::retirement_owned(a).unwrap_or(false)));
                if !known || legacy {
                    rows.push(row(&e.path(), "unproven", false)?);
                }
            }
        }
    }
    rows.sort_by_key(|r| r["path"].as_str().unwrap_or("").to_owned());
    let broken = !apply::maintenance_verify(store, &doc["applied"]);
    let bytes: u64 = rows
        .iter()
        .filter(|r| r["removable"] == true)
        .filter_map(|r| r["bytes"].as_u64())
        .sum();
    let count = rows.iter().filter(|r| r["removable"] == true).count();
    let token = paths::digest_bytes(
        store::canonical(
            &object! {revision:doc["revision"].clone(),rows:rows.clone(),journals:journals.clone()},
        )
        .as_bytes(),
    )?;
    Ok(
        object! {unprovenBytes:rows.iter().filter(|r|r["kind"]=="unproven" && r["id"].is_string()).filter_map(|r|r["bytes"].as_u64()).sum::<u64>(),storage:crate::storage::dispatch("native.storage.status", &object!{dataDir:store.root().to_str()})?,items:display_rows(&rows, &doc),rows:rows,bytes:bytes,count:count,broken:broken,warning:warning,pendingRecovery:pending,canClean:!pending&&count>0,token:token,revision:doc["revision"].clone(),resume:journals.members().any(|j|j["body"]["kind"]=="maintenance")},
    )
}
fn journal_owned(store:&Store,r:&JsonValue,lock:Option<&RootLock>)->Result<bool,NativeError>{
    let id=r["id"].as_str().ok_or_else(blocked)?;let path=store.payload_path(id)?;
    let m=match fs::symlink_metadata(path){Ok(m)=>m,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(e)=>return Err(e.into())};
    if !m.is_file(){return Ok(false);}
    store.payload_owned_with_lock(id,r["sha256"].as_str().unwrap_or(""),m.len(),lock)
}
type ParentState=(u64,u64,std::collections::BTreeSet<std::ffi::OsString>);
fn parent_state(path:&Path)->Result<ParentState,NativeError>{
    paths::reject_symlinks(path)?;
    let m=fs::metadata(path)?;
    let children=fs::read_dir(path)?.map(|e|e.map(|e|e.file_name())).collect::<Result<_,_>>()?;
    Ok((m.dev(),m.ino(),children))
}
#[cfg(test)]
thread_local! {static BEFORE_PAYLOAD_UNLINK:std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> = std::cell::RefCell::new(None);}
fn captured_payload_matches(store:&Store,lock:&RootLock,row:&JsonValue,path:&Path,stable:bool)->Result<bool,NativeError> {
    let file=paths::open_regular(path)?;let m=file.metadata()?;
    let device=row["device"].as_u64().ok_or_else(blocked)?;let inode=row["inode"].as_u64().ok_or_else(blocked)?;
    if !stable {return Ok(m.dev()==device && m.ino()==inode);}
    let fsid=row["fsid"].as_str().filter(|v|crate::package::hex(v,16)).ok_or_else(blocked)?;
    paths::same_persisted_object(store.root(),lock.directory(),path,&file,device,inode,Some(fsid))
}
fn finish(
    store: &Store,
    lock: &RootLock,
    id: &str,
    j: &JsonValue,
) -> Result<JsonValue, NativeError> {
    if j["kind"] != "maintenance"
        || j["version"] != 1
        || j.len() != 6
        || !matches!(j["input"]["method"].as_str(),Some("native.maintenance.clean"|"native.deleteMods"))
        || j["input"]["params"]["operationId"] != id
    {
        return Err(blocked());
    }
    let before = &j["beforeDocument"];
    let deletion=if j["input"]["method"]=="native.deleteMods" {
        if j["retire"]!=false || j["input"]["params"]["expectedRevision"]!=before["revision"] {return Err(blocked());}
        Some(crate::mod_removal::plan(before,&j["input"]["params"])? )
    } else {None};
    let referenced=|id:&str|->Result<bool,NativeError>{
        if let Some(removal)=&deletion {crate::mod_removal::referenced(store,&removal.document,id)} else {store.maintenance_referenced(id)}
    };
    if let Some(removal)=&deletion {
        let mut seen=std::collections::BTreeSet::new();
        for row in j["payloads"].members() {
            let id=row["id"].as_str().ok_or_else(blocked)?;
            let file=removal.files.get(id).ok_or_else(blocked)?;
            if !seen.insert(id) || row["sha256"]!=file.sha256 || row["bytes"]!=file.size || referenced(id)? {return Err(blocked());}
        }
    }
    if store.document()? != *before {
        return Err(blocked());
    }
    let game = before["gamePath"].as_str().unwrap_or("");
    // Library deletion changes only app files; game retirement still requires its lock.
    let _guard = if game.is_empty() || deletion.is_some() {
        None
    } else {
        Some(game_access::GameLock::acquire(
            &Path::new(game).join("game/citadel"),
        )?)
    };
    game_access::ensure_game_idle()?;
    // Preflight every library file before retiring any game file.
    for r in j["payloads"].members() {
        let id = r["id"].as_str().ok_or_else(blocked)?;
        let path = store.payload_path(id)?;
        if r["path"].as_str() != path.to_str() {
            return Err(blocked());
        }
        if !path.try_exists()? {
            continue;
        }
        if !journal_owned(store,r,Some(lock))? {continue;}
        if referenced(id)? {return Err(blocked());}
        if !captured_payload_matches(store,lock,r,&path,deletion.is_some())?
            || r["sha256"] != paths::fingerprint(&path)?.sha256
        {
            return Err(blocked());
        }
    }
    let mut parents=std::collections::BTreeMap::new();
    for r in j["payloads"].members() {
        let path=store.payload_path(r["id"].as_str().ok_or_else(blocked)?)?;
        if path.try_exists()? {
            let parent=path.parent().ok_or_else(blocked)?.to_path_buf();
            if !parents.contains_key(&parent) {parents.insert(parent.clone(),parent_state(&parent)?);}
        }
    }
    let after = if let Some(removal)=&deletion {
        removal.document.clone()
    } else if j["retire"] == true {
        apply::maintenance_retire(store, before)?
    } else {
        before.clone()
    };
    for r in j["payloads"].members() {
        game_access::ensure_game_idle()?;
        let path = store.payload_path(r["id"].as_str().ok_or_else(blocked)?)?;
        if path.try_exists()? {
            if !journal_owned(store,r,Some(lock))? {continue;}
            if !captured_payload_matches(store,lock,r,&path,deletion.is_some())? || r["sha256"] != paths::fingerprint(&path)?.sha256 {
                return Err(blocked());
            }
            let parent=path.parent().ok_or_else(blocked)?;
            #[cfg(test)] BEFORE_PAYLOAD_UNLINK.with(|h|{if let Some(h)=h.borrow_mut().take(){h(&path);}});
            let expected=parents.get_mut(parent).ok_or_else(blocked)?;
            if parent_state(parent)?!=*expected {return Err(blocked());}
            if deletion.is_some() && !captured_payload_matches(store,lock,r,&path,true)? {return Err(blocked());}
            fs::remove_file(&path)?;
            expected.2.remove(path.file_name().ok_or_else(blocked)?);
            paths::sync_dir(path.parent().ok_or_else(blocked)?)?;
        }
    }
    let result = store.commit_with_bulk_result(
        lock,
        store::revision(before["revision"].as_str().ok_or_else(blocked)?)?,
        id,
        &j["input"],
        after,
        if deletion.is_some(){j["input"]["params"]["profileId"].as_str()}else{None},
        deletion.as_ref().map(|r|object!{changed:r.count,skipped:json::array![]}),
    )?;
    store.clear_journal(lock, id)?;
    Ok(result)
}
pub(crate) fn delete_mods(params:&JsonValue,lock:&RootLock)->Result<JsonValue,NativeError> {
    let store=Store::open_writer(lock)?;
    let _hash_scope=paths::HashMemo::default().enter();
    let input=crate::native::operation_input("native.deleteMods",params);
    let id=params["operationId"].as_str().filter(|s|paths::safe_id(s)).ok_or_else(NativeError::params)?;
    if let Some(result)=store.replay(id,&input)? {
        if let Some(j)=store.journal(id)? {if j["kind"]=="maintenance" && j["input"]==input {store.clear_journal(&lock,id)?;}}
        return Ok(result);
    }
    let journals=store.journals()?;
    if !journals.is_empty() {
        if journals.len()==1 && journals[0]["id"]==id && journals[0]["body"]["input"]==input {
            return finish(&store,&lock,id,&journals[0]["body"]);
        }
        return Err(NativeError::new("recoveryRequired","Finish the existing recovery before deleting mods."));
    }
    apply::ensure_idle(&store)?;
    let before=store.document()?;
    if params["expectedRevision"]!=before["revision"] {return Err(NativeError::new("staleRevision","The library changed. Refresh and select the mods again."));}
    let removal=crate::mod_removal::plan(&before,params)?;
    let mut rows=Vec::new();
    for file in removal.files.values() {
        if crate::mod_removal::referenced(&store,&removal.document,&file.id)? {continue;}
        let path=store.payload_path(&file.id)?;
        let metadata=match fs::symlink_metadata(&path) {Ok(m) if m.is_file() && !m.file_type().is_symlink()=>m,Ok(_)=>continue,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,Err(e)=>return Err(e.into())};
        if !store.payload_owned_with_lock(&file.id,&file.sha256,file.size,Some(&lock))? {continue;}
        store.seed_payload_hash(&file.id)?;
        if paths::fingerprint(&path)?.sha256!=file.sha256 {continue;}
        rows.push(object!{path:path.to_string_lossy().as_ref(),kind:"payload",removable:true,reason:"",bytes:file.size,id:file.id.clone(),device:metadata.dev(),inode:metadata.ino(),sha256:file.sha256.clone(),fsid:paths::persisted_filesystem_id(&paths::open_regular(&path)?)?});
    }
    let journal=object!{kind:"maintenance",version:1,input:input,beforeDocument:before,payloads:rows,retire:false};
    game_access::ensure_game_idle()?;
    store.write_journal(&lock,id,&journal)?;
    finish(&store,&lock,id,&journal)
}
pub fn dispatch(method: &str, p: &JsonValue) -> Result<JsonValue, NativeError> {
    let root = paths::data_root(p)?;
    if method == "native.maintenance.scan" {
        return Store::open(&root)?.inspect_recovery(scan);
    }
    if method != "native.maintenance.clean" {
        return Err(NativeError::params());
    }
    let lock = RootLock::acquire(&root)?;
    let store = Store::open_writer(&lock)?;
    let existing = store.journals()?;
    if let Some(e) = existing
        .members()
        .find(|e| e["body"]["kind"] == "maintenance")
    {
        if existing.len() != 1 {
            return Err(blocked());
        }
        let id = e["id"].as_str().ok_or_else(blocked)?;
        if let Some(result) = store.replay(id, &e["body"]["input"])? {
            store.clear_journal(&lock, id)?;
            return Ok(result);
        }
        return finish(&store, &lock, id, &e["body"]);
    }
    if !existing.is_empty() {
        return Err(NativeError::new(
            "recoveryRequired",
            "Finish the existing recovery before maintenance.",
        ));
    }
    let input = object! {method:method,params:p.clone()};
    let id = p["operationId"]
        .as_str()
        .filter(|s| paths::safe_id(s))
        .ok_or_else(NativeError::params)?;
    if let Some(result) = store.replay(id, &input)? {
        return Ok(result);
    }
    let report = scan_with_lock(&store,Some(&lock))?;
    if p["token"] != report["token"]
        || p["expectedRevision"] != report["revision"]
        || report["canClean"] != true
    {
        return Err(blocked());
    }
    let j = object! {kind:"maintenance",version:1,input:input,beforeDocument:store.document()?,payloads:report["rows"].members().filter(|r|r["kind"]=="payload").cloned().collect::<Vec<_>>(),retire:report["rows"].members().any(|r|r["kind"]=="retired"&&r["removable"]==true)};
    game_access::ensure_game_idle()?;
    store.write_journal(&lock, id, &j)?;
    finish(&store, &lock, id, &j)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{import, Fixture};
    fn setup() -> Fixture {
        let f = Fixture::new();
        let mut s = crate::native::NativeService::new();
        import(&mut s, &f);
        f
    }
    fn owned_fixture(st:&Store,path:&Path){
        let db=crate::sqlite::Database::open(&st.root().join("library.sqlite3"),true).unwrap();
        let fp=paths::fingerprint(path).unwrap();let id=path.file_stem().unwrap().to_str().unwrap();
        db.execute("CREATE TABLE IF NOT EXISTS payload_inode_receipts (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, identity TEXT NOT NULL)",&[]).unwrap();
        db.execute("INSERT OR REPLACE INTO payload_inode_receipts(id,sha256,identity) VALUES(?,?,?)",&[id,&fp.sha256,&crate::apply::identity(&fs::metadata(path).unwrap()).dump()]).unwrap();
    }
    fn params(f: &Fixture) -> JsonValue {
        object! {dataDir:f.data.to_str().unwrap()}
    }
    fn deletion_request(f:&Fixture, revision:&str)->JsonValue {
        object!{dataDir:f.data.to_str().unwrap(),profileId:"default",modIds:json::array!["local-a"],expectedRevision:revision,operationId:"delete-mod"}
    }
    #[test]
    fn item104_delete_ignores_unrelated_symlinked_content_receipt() {
        let f=setup();
        let lock=RootLock::acquire(&f.data).unwrap();
        let st=Store::open_writer(&lock).unwrap();
        let doc=st.document().unwrap();
        let file=crate::options::retained_packages(&doc["profiles"]["default"]["mods"][0],crate::package::layout_version(&doc).unwrap()).unwrap()[0].directory.clone();
        let deleted=st.payload_path(&file.id).unwrap();
        owned_fixture(&st,&deleted);
        let unrelated=st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();
        fs::write(&unrelated,b"earlier imported payload").unwrap();
        paths::make_readonly(&unrelated).unwrap();
        owned_fixture(&st,&unrelated);
        {
            let _scope=paths::HashMemo::default().enter();
            let fp=paths::fingerprint(&unrelated).unwrap();
            st.remember_apply_hashes(&[(unrelated.clone(),fp)]).unwrap();
        }
        let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();
        assert!(db.query("SELECT stamp FROM apply_verified_files WHERE path=?",&[unrelated.to_str().unwrap()]).unwrap().is_some());
        let target=f.root.join("foreign-file");
        fs::write(&target,b"preserve foreign bytes").unwrap();
        fs::remove_file(&unrelated).unwrap();
        std::os::unix::fs::symlink(&target,&unrelated).unwrap();
        let result=delete_mods(&deletion_request(&f,"1"),&lock).unwrap();
        assert_eq!(result["revision"],"2");
        assert!(!deleted.exists());
        assert!(fs::symlink_metadata(&unrelated).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&unrelated).unwrap(),target);
        assert_eq!(fs::read(&target).unwrap(),b"preserve foreign bytes");
        assert!(st.journals().unwrap().is_empty());
    }
    #[test]
    fn item104_delete_hashes_once_or_trusts_matching_content_receipt() {
        for receipt in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let doc=st.document().unwrap();
            let file=crate::options::retained_packages(&doc["profiles"]["default"]["mods"][0],crate::package::layout_version(&doc).unwrap()).unwrap()[0].directory.clone();
            let path=st.payload_path(&file.id).unwrap();owned_fixture(&st,&path);
            if receipt {let _scope=paths::HashMemo::default().enter();let fp=paths::fingerprint(&path).unwrap();st.remember_apply_hashes(&[(path.clone(),fp)]).unwrap();}
            paths::HASH_READS.with(|r|r.borrow_mut().clear());
            delete_mods(&deletion_request(&f,"1"),&lock).unwrap();
            let reads=paths::HASH_READS.with(|r|r.borrow().get(&path).copied().unwrap_or(0));
            assert_eq!(reads,if receipt {0}else{1},"delete must reuse proof while retaining final identity/ownership checks");assert!(!path.exists());
        }
    }
    #[test]
    fn item100_delete_mods_removes_all_profile_links_and_only_owned_payloads() {
        for owned in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let mut doc=st.document().unwrap();
            let record=doc["profiles"]["default"]["mods"][0].clone();
            let file=crate::options::retained_packages(&record,crate::package::layout_version(&doc).unwrap()).unwrap()[0].directory.clone();
            let path=st.payload_path(&file.id).unwrap();if owned {owned_fixture(&st,&path);} else {let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),true).unwrap();db.execute("DELETE FROM payload_inode_receipts WHERE id=?",&[&file.id]).unwrap();}
            doc["profiles"]["other"]["mods"]=json::array![record];
            st.commit(&lock,1,"seed-delete",&object!{},doc,None).unwrap();drop(lock);drop(st);
            let p=deletion_request(&f,"2");let mut service=crate::native::NativeService::new();
            let result=service.dispatch("native.deleteMods",&p).unwrap();
            assert!(result["mods"].is_empty());assert_eq!(result["revision"],"3");
            let doc=Store::open(&f.data).unwrap().document().unwrap();
            assert!(doc["profiles"]["other"]["mods"].is_empty());
            assert_eq!(path.exists(),!owned,"only proven app payloads may be deleted");
            assert!(f.game.join("game/citadel/addons/pak01_dir.vpk").exists(),"original imports stay");
            assert_eq!(crate::native::NativeService::new().dispatch("native.deleteMods",&p).unwrap(),result,"restart replay must be exact");
        }
    }
    #[test]
    fn item100_delete_mods_preserves_shared_payloads() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let mut doc=st.document().unwrap();
        let mut other=doc["profiles"]["default"]["mods"][0].clone();
        let file=crate::options::retained_packages(&other,crate::package::layout_version(&doc).unwrap()).unwrap()[0].directory.clone();
        let path=st.payload_path(&file.id).unwrap();owned_fixture(&st,&path);
        other["source"]["id"]="keep-mod".into();doc["profiles"]["other"]["mods"]=json::array![other];
        st.commit(&lock,1,"seed-shared",&object!{},doc,None).unwrap();drop(lock);
        crate::native::NativeService::new().dispatch("native.deleteMods",&deletion_request(&f,"2")).unwrap();
        assert!(path.exists());assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["profiles"]["other"]["mods"].len(),1);
    }
    #[test]
    fn item100_delete_mods_rejects_stale_unknown_and_duplicate_inputs() {
        let f=setup();
        for (ids,revision) in [(json::array!["local-a"],"0"),(json::array!["missing"],"1"),(json::array!["local-a","local-a"],"1")] {
            let mut p=deletion_request(&f,revision);p["modIds"]=ids;
            assert!(crate::native::NativeService::new().dispatch("native.deleteMods",&p).is_err());
            assert_eq!(Store::open(&f.data).unwrap().document().unwrap()["revision"],"1");
        }
    }
    #[test]
    fn item100_delete_keeps_a_replacement_at_the_final_unlink() {
        let f=setup();let st=Store::open(&f.data).unwrap();let doc=st.document().unwrap();
        let file=crate::options::retained_packages(&doc["profiles"]["default"]["mods"][0],crate::package::layout_version(&doc).unwrap()).unwrap()[0].directory.clone();
        let path=st.payload_path(&file.id).unwrap();let inode=fs::metadata(&path).unwrap().ino();
        BEFORE_PAYLOAD_UNLINK.with(|hook|*hook.borrow_mut()=Some(Box::new(|path| {
            let replacement=path.with_extension("replacement");fs::write(&replacement,fs::read(path).unwrap()).unwrap();fs::rename(replacement,path).unwrap();
        })));
        let error=crate::native::NativeService::new().dispatch("native.deleteMods",&deletion_request(&f,"1")).unwrap_err();
        assert_eq!(error.kind,"maintenanceChanged");assert!(path.exists());assert_ne!(fs::metadata(&path).unwrap().ino(),inode);
        dispatch("native.maintenance.clean",&params(&f)).unwrap();
        assert!(path.exists(),"recovery must preserve the unowned replacement");
        assert!(Store::open(&f.data).unwrap().document().unwrap()["profiles"]["default"]["mods"].is_empty());
    }
    #[test]
    fn item100_delete_without_game_folder_does_not_strand_the_library() {
        let f=setup();
        fs::rename(&f.game,f.root.join("moved-game")).unwrap();
        let mut service=crate::native::NativeService::new();
        let result=service.dispatch("native.deleteMods",&deletion_request(&f,"1")).unwrap();
        assert_eq!(result["revision"],"2");assert!(result["mods"].is_empty());
        assert!(Store::open(&f.data).unwrap().journals().unwrap().is_empty());
        let edit=object!{dataDir:f.data.to_str().unwrap(),profileId:"other",modId:"local-b",enabled:false,expectedRevision:"2",operationId:"edit-after-delete"};
        assert_eq!(service.dispatch("native.setEnabled",&edit).unwrap()["revision"],"3");
    }
    #[test]
    fn item100_delete_journal_resumes_after_unlink_or_device_renumber() {
        for missing in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let before=st.document().unwrap();
            let file=crate::options::retained_packages(&before["profiles"]["default"]["mods"][0],crate::package::layout_version(&before).unwrap()).unwrap()[0].directory.clone();
            let path=st.payload_path(&file.id).unwrap();owned_fixture(&st,&path);let m=fs::metadata(&path).unwrap();
            let fsid=paths::persisted_filesystem_id(&fs::File::open(&path).unwrap()).unwrap();
            let row=object!{path:path.to_str().unwrap(),kind:"payload",removable:true,reason:"",bytes:file.size,id:file.id.clone(),device:m.dev()+1,inode:m.ino(),sha256:file.sha256,fsid:fsid};
            let input=crate::native::operation_input("native.deleteMods",&deletion_request(&f,"1"));
            let j=object!{kind:"maintenance",version:1,input:input,beforeDocument:before,payloads:json::array![row],retire:false};
            st.write_journal(&lock,"delete-mod",&j).unwrap();
            if missing {fs::remove_file(&path).unwrap();}
            fs::rename(&f.game,f.root.join("moved-game")).unwrap();
            drop(st);drop(lock);
            let result=dispatch("native.maintenance.clean",&params(&f)).unwrap();
            assert!(result["mods"].is_empty());assert!(!path.exists());
            assert!(Store::open(&f.data).unwrap().journals().unwrap().is_empty());
        }
    }
    #[test]
    fn item100_delete_preserves_applied_checkpoint_and_payload() {
        let f=setup();fs::write(f.game.join("game/citadel/gameinfo.gi"), "\"GameInfo\"\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n Game core\n }\n }\n}\n").unwrap();
        let mut service=crate::native::NativeService::new();let mut p=params(&f);
        p["profileId"]="default".into();p["gamePath"]=f.game.to_str().unwrap().into();p["deploymentLayout"]="shards-v1".into();
        let review=service.dispatch("native.applyPreview",&p).unwrap();
        let apply=object!{dataDir:f.data.to_str().unwrap(),expectedRevision:"1",operationId:"apply-before-delete",deploymentLayout:"shards-v1",token:review["token"].clone()};
        service.dispatch("native.apply",&apply).unwrap();
        let before=Store::open(&f.data).unwrap().document().unwrap();
        let payload=crate::options::retained_packages(&before["profiles"]["default"]["mods"][0],crate::package::layout_version(&before).unwrap()).unwrap()[0].directory.clone();
        let path=Store::open(&f.data).unwrap().payload_path(&payload.id).unwrap();
        let gameinfo=fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();
        service.dispatch("native.deleteMods",&deletion_request(&f,"2")).unwrap();
        let after=Store::open(&f.data).unwrap().document().unwrap();
        assert!(after["profiles"]["default"]["mods"].is_empty());assert_eq!(after["applied"],before["applied"]);assert_eq!(after["previous"],before["previous"]);
        assert!(path.exists());assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),gameinfo);
    }
    #[test]
    fn round11_maintenance_parent_final_check() {
        for replace in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
            let id="abcdefabcdefabcdefabcdefabcdefab";let path=st.payload_path(id).unwrap();fs::write(&path,b"owned").unwrap();owned_fixture(&st,&path);
            let fp=paths::fingerprint(&path).unwrap();let m=fs::metadata(&path).unwrap();
            let row=object!{id:id,path:path.to_str(),device:m.dev(),inode:m.ino(),sha256:fp.sha256};
            let j=object!{kind:"maintenance",version:1,input:object!{method:"native.maintenance.clean",params:object!{operationId:"cleanup"}},beforeDocument:st.document().unwrap(),payloads:json::array![row],retire:false};
            st.write_journal(&lock,"cleanup",&j).unwrap();
            BEFORE_PAYLOAD_UNLINK.with(|h|*h.borrow_mut()=Some(Box::new(move|path|{
                let parent=path.parent().unwrap();
                if replace {let held=parent.with_extension("held");fs::rename(parent,&held).unwrap();fs::create_dir(parent).unwrap();for entry in fs::read_dir(held).unwrap(){let old=entry.unwrap().path();fs::hard_link(&old,parent.join(old.file_name().unwrap())).unwrap();}}
                else {fs::write(parent.join("foreign"),b"keep").unwrap();}
            })));
            assert!(finish(&st,&lock,"cleanup",&j).is_err());assert_eq!(fs::read(path).unwrap(),b"owned");
            BEFORE_PAYLOAD_UNLINK.with(|h|assert!(h.borrow().is_none()));
        }
    }
    #[test]
    fn round8_legacy_journal_keeps_unowned_entries_and_clears() {
        for replacement in [false,true] {
            let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let id="abcdefabcdefabcdefabcdefabcdefab";let path=st.payload_path(id).unwrap();fs::write(&path,b"foreign").unwrap();
            let fp=paths::fingerprint(&path).unwrap();let m=fs::metadata(&path).unwrap();
            let row=object!{id:id,path:path.to_str(),kind:"payload",removable:true,device:m.dev(),inode:m.ino(),sha256:fp.sha256};
            if replacement {let target=f.data.join("kept");fs::write(&target,b"target").unwrap();fs::remove_file(&path).unwrap();std::os::unix::fs::symlink(target,&path).unwrap();}
            let input=object!{method:"native.maintenance.clean",params:object!{operationId:"legacy"}};
            let j=object!{kind:"maintenance",version:1,input:input,beforeDocument:st.document().unwrap(),payloads:json::array![row],retire:false};st.write_journal(&lock,"legacy",&j).unwrap();
            let scan=scan(&st).unwrap();assert!(scan["rows"].members().any(|r|r["path"]==path.to_str().unwrap() && r["removable"]==false));
            finish(&st,&lock,"legacy",&j).unwrap();assert!(fs::symlink_metadata(&path).is_ok());assert!(st.journals().unwrap().is_empty());
        }
    }
    #[test]
    fn round3_scan_while_remote_owner_holds_lease() {
        let f=crate::test_support::Fixture::new();crate::test_support::import(&mut crate::native::NativeService::new(),&f);
        let _owner=crate::remote::job_store::RemoteJobStore::open(&f.data).unwrap();
        fs::write(f.data.join("payloads/abcdefabcdefabcdefabcdefabcdefab.vpk"),b"orphan").unwrap();
        let st=Store::open(&f.data).unwrap();payloads(&st,None).expect("scan uses only library evidence during downloads");
    }
    #[test]
    fn round5_unrelated_manifest_tuple_kept() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        let id="abcdefabcdefabcdefabcdefabcdefab";let p=st.payload_path(id).unwrap();fs::write(&p,b"foreign").unwrap();let fp=paths::fingerprint(&p).unwrap();
        let mut doc=st.document().unwrap();doc["importLedger"]["unrelatedManifestProperty"]=object!{id:id,sha256:fp.sha256,size:fp.size};
        st.commit(&lock,1,"foreign-tuple",&object!{},doc,None).unwrap();
        assert!(payloads(&st,None).unwrap().iter().any(|r|r["path"]==p.to_str().unwrap() && r["removable"]==false),"foreign manifest tuple must never authorize deletion");
    }
    #[test]
    fn round5_fresh_journal_has_barrier() {
        let f=Fixture::new();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();
        st.write_journal(&lock,"fresh",&object!{kind:"import"}).unwrap();
        let db=crate::sqlite::Database::open(&f.data.join("library.sqlite3"),false).unwrap();
        let row=db.query("SELECT body FROM library WHERE id=1",&[]).unwrap().expect("first journal needs a library barrier row");
        assert_eq!(json::parse(&row[0]).unwrap()["storeVersion"],5);
        assert_eq!(st.document().unwrap_err().kind,"nativeNotImported");
    }
    #[test]
    fn legacy_ledger_orphan_is_kept() {
        let f=setup();let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let id="abcdefabcdefabcdefabcdefabcdefab";let p=st.payload_path(id).unwrap();fs::write(&p,b"old").unwrap();let fp=paths::fingerprint(&p).unwrap();
        let mut doc=st.document().unwrap();doc["importLedger"]["payloads"].push(object!{id:id,sha256:fp.sha256,size:fp.size}).unwrap();st.commit(&lock,1,"seed",&object!{},doc,None).unwrap();
        assert!(payloads(&st,None).unwrap().iter().any(|r|r["path"]==p.to_str().unwrap() && r["removable"]==false));
    }
    #[test]
    fn receipt_rejects_same_bytes_replacement() {
        let f=setup();let st=Store::open(&f.data).unwrap();let p=st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();
        fs::write(&p,b"owned").unwrap();owned_fixture(&st,&p);
        let other=p.with_extension("replacement");fs::write(&other,b"owned").unwrap();fs::rename(other,&p).unwrap();
        assert!(payloads(&st,None).unwrap().iter().any(|r|r["path"]==p.to_str().unwrap() && r["removable"]==false));
    }
    #[test]
    fn unknown_payload_filename_is_not_ownership() {
        let f=setup();let st=Store::open(&f.data).unwrap();let path=st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();fs::write(&path,b"foreign").unwrap();
        let rows=payloads(&st,None).unwrap();assert!(rows.iter().any(|r|r["path"]==path.to_str().unwrap() && r["removable"]==false));
    }
    #[test]
    fn maintenance_presentation_groups_files_without_changing_removal_evidence() {
        let rows = vec![
            object!{kind:"retired",removable:true,path:"/game/old/shard01/a.vpk",bytes:78},
            object!{kind:"retired",removable:true,path:"/game/old/shard01",bytes:0},
            object!{kind:"retired",removable:true,path:"/game/old/manifest.json",bytes:22},
            object!{kind:"payload",removable:true,path:"/data/file.vpk",id:"gone",bytes:8},
            object!{kind:"foreign",removable:false,path:"/game/addons",bytes:40},
        ];
        let doc = object!{retiredGenerations:json::array![object!{generation:"old",identities:object!{"manifest.json":object!{modifiedSeconds:12345}}}]};
        let items = display_rows(&rows, &doc);
        assert_eq!(items.len(), 3);
        let old = items.iter().find(|r| r["kind"] == "retired").unwrap();
        assert_eq!(old["bytes"], 100);
        assert_eq!(old["date"], 12345);
        assert_eq!(old["paths"].len(), 3);
        assert_eq!(items[0]["title"], "");
        assert_eq!(rows.len(), 5);
        let known = object!{profiles:object!{default:object!{mods:json::array![object!{title:"A mod",payload:object!{id:"gone"}}]}}};
        assert_eq!(payload_title(&known, "gone").as_deref(), Some("A mod"));
        assert_eq!(payload_title(&known, "unknown"), None);
        let name = object!{name:"Imported mod",localImport:object!{payload:object!{id:"gone"}}};
        assert_eq!(payload_title(&name, "gone").as_deref(), Some("Imported mod"));
        assert_eq!(payload_title(&object!{title:"gone",id:"another"}, "gone"), None);
    }
    #[test]
    fn maintenance_retires_captured_generation_and_preserves_foreign() {
        for interrupted in [false, true] {
            let f = setup();
            let c = f.game.join("game/citadel");
            let original="GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n";
            fs::write(c.join("gameinfo.gi"), original).unwrap();
            let lock = RootLock::acquire(&f.data).unwrap();
            let st = Store::open_writer(&lock).unwrap();
            let plan =
                crate::apply::ApplyPlan::review(&st,"default",&f.game).unwrap();
            let input = object! {method:"native.apply",params:object!{dataDir:f.data.to_str().unwrap(),operationId:"apply",expectedRevision:"1",token:"0123456789abcdef0123456789abcdef",deploymentLayout:"shards-v1"}};
            plan.apply(&st, &lock, 1, "apply", &input).unwrap();
            let mut doc = st.document().unwrap();
            let old = doc["applied"].clone();
            let dir = c
                .join("deadlock_native")
                .join(old["generation"].as_str().unwrap());
            doc["retiredGenerations"] = json::array![old];
            doc["applied"] = JsonValue::Null;
            doc["previous"] = JsonValue::Null;
            doc["canRestore"] = false.into();
            fs::write(c.join("gameinfo.gi"), original).unwrap();
            st.commit(
                &lock,
                2,
                "retire-fixture",
                &object! {method:"test"},
                doc,
                None,
            )
            .unwrap();
            drop(st);
            drop(lock);
            let report = dispatch("native.maintenance.scan", &params(&f)).unwrap();
            assert!(report["count"].as_usize().unwrap() > 0, "{}", report.dump());
            let mut p = params(&f);
            p["token"] = report["token"].clone();
            p["expectedRevision"] = report["revision"].clone();
            p["operationId"] = "retire".into();
            if interrupted {
                let lock = RootLock::acquire(&f.data).unwrap();
                let st = Store::open_writer(&lock).unwrap();
                let j = object! {kind:"maintenance",version:1,input:object!{method:"native.maintenance.clean",params:p.clone()},beforeDocument:st.document().unwrap(),payloads:Vec::<JsonValue>::new(),retire:true};
                st.write_journal(&lock, "retire", &j).unwrap();
                let file = report["rows"]
                    .members()
                    .find(|r| r["bytes"].as_u64().unwrap_or(0) > 0 && r["kind"] == "retired")
                    .unwrap();
                fs::remove_file(file["path"].as_str().unwrap()).unwrap();
            }
            dispatch("native.maintenance.clean", &p).unwrap();
            assert!(!dir.exists());
            assert!(c.join("addons/pak01_dir.vpk").exists());
            assert!(
                Store::open(&f.data).unwrap().document().unwrap()["retiredGenerations"].is_empty()
            );
        }
    }
    #[test]
    fn maintenance_scan_and_clean_preserve_foreign_and_referenced_payloads() {
        let f = setup();
        let store = Store::open(&f.data).unwrap();
        let id = "abcdefabcdefabcdefabcdefabcdefab";
        let path = store.payload_path(id).unwrap();
        fs::write(&path, b"unreferenced").unwrap();
        owned_fixture(&store,&path);
        let report = dispatch("native.maintenance.scan", &params(&f)).unwrap();
        assert!(report["rows"].members().any(|r| r["kind"] == "foreign"));
        assert_eq!(report["bytes"], 12);
        assert_eq!(report["count"], 1);
        let mut p = params(&f);
        p["token"] = report["token"].clone();
        p["expectedRevision"] = report["revision"].clone();
        p["operationId"] = "gc".into();
        dispatch("native.maintenance.clean", &p).unwrap();
        assert!(!path.exists());
        assert!(f.game.join("game/citadel/addons/pak01_dir.vpk").exists());
        assert_eq!(
            dispatch("native.maintenance.scan", &params(&f)).unwrap()["count"],
            0
        );
    }
    #[test]
    fn maintenance_rejects_stale_review_and_preserves_symlinks() {
        let f = setup();
        let st = Store::open(&f.data).unwrap();
        let path = st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();
        fs::write(&path, b"one").unwrap();
        owned_fixture(&st,&path);
        let report = dispatch("native.maintenance.scan", &params(&f)).unwrap();
        fs::write(&path, b"changed").unwrap();
        let mut p = params(&f);
        p["token"] = report["token"].clone();
        p["expectedRevision"] = report["revision"].clone();
        p["operationId"] = "stale".into();
        assert!(dispatch("native.maintenance.clean", &p).is_err());
        assert!(path.exists());
        assert!(st.journals().unwrap().is_empty());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(f.game.join("game/citadel/addons/pak01_dir.vpk"), &path)
            .unwrap();
        assert!(dispatch("native.maintenance.scan", &params(&f)).is_err());
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    #[test]
    fn maintenance_interrupted_payload_cleanup_resumes_recorded_identity() {
        let f = setup();
        let lock = RootLock::acquire(&f.data).unwrap();
        let st = Store::open_writer(&lock).unwrap();
        let path = st.payload_path("abcdefabcdefabcdefabcdefabcdefab").unwrap();
        fs::write(&path, b"orphan").unwrap();
        owned_fixture(&st,&path);
        let input = object! {method:"native.maintenance.clean",params:object!{operationId:"resume",dataDir:f.data.to_str().unwrap()}};
        let j = object! {kind:"maintenance",version:1,input:input,beforeDocument:st.document().unwrap(),payloads:payloads(&st,None).unwrap(),retire:false};
        st.write_journal(&lock, "resume", &j).unwrap();
        fs::remove_file(&path).unwrap();
        crate::sqlite::Database::open(&st.root().join("library.sqlite3"),true).unwrap().execute("DROP TABLE payload_inode_receipts",&[]).unwrap();
        finish(&st, &lock, "resume", &j).unwrap();
        assert!(st.journals().unwrap().is_empty());
        assert_eq!(st.document().unwrap()["revision"], "2");
    }
    #[test]
    fn maintenance_reports_unknown_staging_without_adopting_it() {
        let f = setup();
        let dir = f
            .game
            .join("game/citadel/deadlock_native/unfinished/shard01");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("foreign.vpk"), b"keep").unwrap();
        let report = dispatch("native.maintenance.scan", &params(&f)).unwrap();
        assert!(report["rows"]
            .members()
            .any(|r| r["kind"] == "unproven" && r["removable"] == false));
        assert!(dir.join("foreign.vpk").exists());
    }
}


#[cfg(test)]
#[test]
fn item93_maintenance_scan_does_not_create_writer_lock() {
    let fixture=crate::test_support::Fixture::new();
    crate::test_support::import(&mut crate::native::NativeService::new(),&fixture);
    let lock=fixture.data.join(".writer.lock");
    std::fs::remove_file(&lock).unwrap();
    let store=Store::open(&fixture.data).unwrap();
    scan(&store).unwrap();
    assert!(!lock.exists(),"read-only maintenance scans must not create a writer lock");
}
