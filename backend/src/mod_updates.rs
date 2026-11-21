//! Derived GameBanana update evidence. Upstream update UX: GPL-3.0.
//! This cache grants no replacement authority; every update still needs review.
use crate::{catalog::{provider::SubmissionKeyV1, store::CatalogStore}, native::NativeError, paths, store::Store};
use json::{object, JsonValue};

fn family(name: &str) -> String {
    // Strip explicit version tokens only. Variant names and arbitrary numbers matter.
    name.rsplit_once('.').map(|(stem,_)|stem).unwrap_or(name).to_lowercase().split(['_', '-', ' ']).filter(|s| {
        !(s.starts_with('v') && s[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
            && s[1..].chars().all(|c| c.is_ascii_digit() || c == '.'))
    }).collect::<Vec<_>>().join("_")
}
pub fn decide(installed: &str, files: &JsonValue, complete: bool) -> JsonValue {
    let unsure = || object!{status:"unsure",reason:"The installed file or its version family could not be confirmed."};
    if installed.is_empty() { return object!{status:"unsure",versionUnknown:true,reason:"Version unknown, update to the latest file"}; }
    if !complete { return unsure(); }
    let Some(old) = files.members().find(|f| f["fileId"] == installed) else { return unsure() };
    let Some(date) = old["createdAt"].as_u64() else { return unsure() };
    let Some(name) = old["name"].as_str().filter(|s| !s.is_empty()) else { return unsure() };
    let active: Vec<_> = files.members().filter(|f| f["isArchived"] != true && f["fileId"].as_str().and_then(|s|s.parse::<u64>().ok()).zip(installed.parse::<u64>().ok()).is_some_and(|(new,old)|new>old) && f["createdAt"].as_u64().is_some_and(|d| d > date)).collect();
    let mut candidates: Vec<_> = active.iter().filter(|f| f["name"].as_str().is_some_and(|n| family(n)==family(name))).collect();
    candidates.sort_by_key(|f| f["createdAt"].as_u64().unwrap_or(0));
    if let Some(new) = candidates.last() {
        if candidates.iter().filter(|f| f["createdAt"] == new["createdAt"]).count() != 1 { return unsure(); }
        return object!{status:"available",fileId:new["fileId"].clone(),createdAt:new["createdAt"].clone()};
    }
    if old["isArchived"] == true && !active.is_empty() { return unsure(); }
    object!{status:"current"}
}
pub fn dispatch(p: &JsonValue) -> Result<JsonValue, NativeError> {
    if p.entries().any(|(k,_)| !["dataDir","profileId","finish"].contains(&k)) || !p["profileId"].is_string() || !p["finish"].is_boolean() { return Err(NativeError::params()); }
    let root = paths::data_root(p)?;
    let doc = Store::open(&root)?.document()?;
    let profile = p["profileId"].as_str().unwrap();
    if !doc["profiles"].has_key(profile) { return Err(NativeError::params()); }
    paths::private_dir(&root.join("cache/catalog"))?;
    let mut cache = CatalogStore::open(&root)?;
    let layout = crate::package::layout_version(&doc)?;
    let mut rows = json::array![];
    for r in doc["profiles"][profile]["mods"].members() {
        let receipt = crate::remote::replacement_target::receipt(r);
        let association = crate::remote::replacement_target::association_submission(&r["source"]);
        let sub = association.clone().unwrap_or_else(|| receipt["submission"].clone());
        let Ok(key) = SubmissionKeyV1::parse(&sub) else { continue };
        let installed = receipt["rootFileId"].as_str().map(str::to_owned).or_else(|| if association.is_some() {None} else {crate::profile_share::cache_identity(r,layout).ok().and_then(|id| cache.state(&id).ok().flatten())}).unwrap_or_default();
        // A private or removed page is one mod's state, not a failed check for the whole library.
        let hidden = cache.unavailable(&key)?.is_some();
        let mut row = if hidden {
            object!{status:"unsure",reason:"This mod is private or removed on GameBanana, so updates can't be checked."}
        } else {
            let files = cache.resource("files", &format!("{}:all",key.slug()))?;
            files.map(|f| decide(&installed,&f.payload,f.complete)).unwrap_or_else(|| decide(&installed,&JsonValue::Null,false))
        };
        row["modId"] = r["source"]["id"].clone(); row["submission"] = sub;
        row["recordDigest"] = crate::local_content::option_sources::record_digest(r)?.into();
        if !hidden { if let Some(log) = cache.resource("changelog", &format!("{}:1",key.slug()))? {
            row["changelog"] = log.payload;
        } }
        rows.push(row).map_err(|_| NativeError::params())?;
    }
    let stamp = format!("mod-updates:{profile}");
    if p["finish"] == true { cache.set_state(&stamp,&crate::catalog::store::now().to_string())?; }
    Ok(object!{profileId:profile,revision:doc["revision"].clone(),lastUpdateCheck:cache.state(&stamp)?.and_then(|s|s.parse::<u64>().ok()),mods:rows})
}
#[cfg(test)] mod tests {
    use super::*;
    fn file(id: &str,name: &str,date:u64) -> JsonValue { object!{fileId:id,name:name,createdAt:date,isArchived:false} }
    #[test] fn mod_updates_ambiguous_and_archived() {
        let mut old=file("1","blue_v1.zip",10);old["isArchived"]=true.into();
        assert_eq!(decide("1",&json::array![old.clone(),file("2","blue_v2.zip",20)],true)["status"],"available");
        assert_eq!(decide("1",&json::array![old.clone(),file("2","red.zip",20)],true)["status"],"unsure");
        assert_eq!(decide("1",&json::array![old.clone(),file("2","blue_v2.zip",20),file("3","blue_v3.zip",20)],true)["status"],"unsure");
        assert_eq!(decide("3",&json::array![file("3","blue.zip",10),file("2","blue.zip",20)],true)["status"],"current");
        assert_eq!(decide("",&JsonValue::Null,true)["status"],"unsure");
    }
    #[test] fn mod_updates_decisions() {
        let files=json::array![file("1","blue_v1.zip",10),file("2","blue_v2.zip",20),file("3","red_v2.zip",30)];
        assert_eq!(decide("1",&files,true)["fileId"],"2");
        assert_eq!(decide("2",&files,true)["status"],"current");
        assert_eq!(decide("9",&files,true)["status"],"unsure");
        assert_eq!(decide("1",&files,false)["status"],"unsure");
        assert_eq!(decide("1",&json::array![file("1","blue.zip",10)],true)["status"],"current");
        assert_eq!(decide("1",&json::array![file("1","blue.zip",10),file("2","blue.zip",10)],true)["status"],"current");
    }
    #[test] fn mod_updates_unknown_version_never_claims_current() {
        for complete in [false,true] {
            let state=decide("",&json::array![file("12","latest.zip",20)],complete);
            assert_eq!(state["status"],"unsure");
            assert_eq!(state["versionUnknown"],true);
            assert_eq!(state["reason"],"Version unknown, update to the latest file");
        }
    }
    #[test] fn mod_updates_page_link_ignores_sharing_cache_identity() {
        let fixture=crate::test_support::Fixture::new();
        let sub=object!{provider:"gamebanana",submissionType:"mod",submissionId:"123"};
        let row=object!{source:object!{id:"local-one",kind:"local",name:"Kept",installOrder:1,
            localImport:object!{version:1},nativeRemoteLink:object!{version:2,association:"userSelected",submission:sub.clone(),receipt:JsonValue::Null}},
            enabled:true,available:true,previewHidden:false,
            packages:json::array![object!{directory:object!{id:"a".repeat(32),sha256:"b".repeat(64),size:12,sourceName:"pak01_dir.vpk"},companions:json::array![]}]};
        let identity=crate::profile_share::cache_identity(&row,2).unwrap();
        let doc=object!{storeVersion:4,packageVersion:2,revision:"0",sourcePath:"",gamePath:"",activeProfileId:"default",
            profiles:object!{default:object!{id:"default",name:"Default",protected:true,mods:json::array![row]}},
            previewPolicy:object!{},importArchive:object!{keep:true},importLedger:object!{sourcePayloads:json::array![]},
            fonts:object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null},applied:JsonValue::Null,previous:JsonValue::Null,canRestore:false,recoveryRequired:false};
        let lock=crate::store::RootLock::acquire(&fixture.data).unwrap();
        Store::open_writer(&lock).unwrap().commit(&lock,0,"initialize",&object!{},doc,Some("default")).unwrap();
        drop(lock);
        paths::private_dir(&fixture.data.join("cache/catalog")).unwrap();
        let mut cache=CatalogStore::open(&fixture.data).unwrap();
        cache.set_state(&identity,"456").unwrap();
        let resource=format!("{}:all",SubmissionKeyV1::parse(&sub).unwrap().slug());
        cache.put_resource("files",&resource,&json::array![file("456","latest.zip",20)],true).unwrap();
        drop(cache);
        let result=dispatch(&object!{dataDir:fixture.data.to_str().unwrap(),profileId:"default",finish:false}).unwrap();
        let saved=Store::open(&fixture.data).unwrap().document().unwrap();
        let target=crate::remote::replacement_target::admit(saved["profiles"]["default"]["mods"][0].clone(),2,"default","local-one",1).unwrap();
        assert_eq!(result["mods"][0]["recordDigest"],target.public["recordDigest"]);
        assert_eq!(result["mods"][0]["status"],"unsure");
        assert_eq!(result["mods"][0]["versionUnknown"],true);
        // GameBanana hiding this page must not fail the check for the whole library.
        let mut cache=CatalogStore::open(&fixture.data).unwrap();
        cache.mark_unavailable(&SubmissionKeyV1::parse(&sub).unwrap(),"catalogPrivate").unwrap();
        drop(cache);
        let hidden=dispatch(&object!{dataDir:fixture.data.to_str().unwrap(),profileId:"default",finish:true}).unwrap();
        assert_eq!(hidden["mods"][0]["status"],"unsure");
        assert_eq!(hidden["mods"][0]["modId"],"local-one");
        assert!(hidden["mods"][0]["reason"].as_str().unwrap().contains("private or removed"));
        assert!(hidden["lastUpdateCheck"].is_number());
    }
}
