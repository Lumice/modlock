//! Version-one launch evidence codecs. Decoding never creates live authority.
use crate::{native::NativeError, paths, store::{self, Store}};
use json::{object, JsonValue};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

pub const MARKER: &str = ".deadlock-native-launch.json";
pub const CONTROL_LIMIT: usize = 64 * 1024;
pub const MARKER_LIMIT: usize = 16 * 1024;
pub(crate) fn invalid() -> NativeError {
    NativeError::new("launchReconciliationRequired", "Launch evidence is incomplete or changed; preserve it for explicit reconciliation.")
}
pub(crate) fn barrier() -> NativeError {
    NativeError::new("launchResolutionRequired", "A launch reservation requires launch-specific settlement before changing game files.")
}
pub(crate) fn exact(v: &JsonValue, keys: &[&str]) -> Result<(), NativeError> {
    if !v.is_object() || v.len() != keys.len() || keys.iter().any(|k| !v.has_key(k)) { return Err(invalid()); }
    Ok(())
}
pub(crate) fn s<'a>(v: &'a JsonValue, k: &str) -> Result<&'a str, NativeError> { v[k].as_str().ok_or_else(invalid) }
pub(crate) fn u(v: &JsonValue, k: &str) -> Result<u64, NativeError> { store::revision(s(v,k)?).map_err(|_|invalid()) }
pub(crate) fn digest(v: &JsonValue) -> Result<String, NativeError> { paths::digest_bytes(store::canonical(v).as_bytes()) }
pub(crate) fn hex(v: &JsonValue, k: &str, n: usize) -> Result<(), NativeError> {
    if !crate::package::hex(s(v,k)?,n) { return Err(invalid()); } Ok(())
}
pub(crate) fn id(v: &JsonValue, k: &str) -> Result<(), NativeError> {
    if !paths::safe_id(s(v,k)?) { return Err(invalid()); } Ok(())
}
pub(crate) fn bounded(v: &JsonValue, max: usize) -> Result<(), NativeError> {
    if crate::protocol::native_value_bytes(v).is_none_or(|n| n > max) {
        return Err(NativeError::new("launchRecordLimit", "Launch evidence exceeds its readable size limit; no new launch was admitted."));
    } Ok(())
}
pub(crate) fn path(v: &JsonValue, k: &str) -> Result<(), NativeError> {
    let p=s(v,k)?;
    if p.is_empty() || p.len()>4096 || !Path::new(p).is_absolute() || paths::absolute(Path::new(p))? != Path::new(p) { return Err(invalid()); } Ok(())
}
pub fn validate_arguments(raw: &str) -> Result<(), NativeError> {
    if raw.len()>16384 || raw.chars().any(|c| c.is_control() && c!='\t') { return Err(NativeError::new("invalidLaunchArguments", "Additional arguments exceed 16 KiB or contain unsupported control characters.")); } Ok(())
}
pub(crate) fn settings(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["version","revision","additionalArguments"])?;
    if v["version"]!=1 {return Err(invalid());} u(v,"revision")?; validate_arguments(s(v,"additionalArguments")?)
}
pub(crate) fn meta(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["version","epoch","ownerToken","openId","admitted","settingsWrites"])?;
    if v["version"]!=1 || u(v,"epoch")?==0 || u(v,"admitted")?>64 || u(v,"settingsWrites")?>64 {return Err(invalid());}
    hex(v,"ownerToken",32)?; id(v,"openId")
}
pub(crate) fn begin(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["method","params"])?;
    let p=&v["params"]; exact(p,&["epoch","attemptId","profileId","mode","expectedRevision","settingsRevision"])?;
    if v["method"]!="native.launch.begin" || !matches!(s(p,"mode")?,"modded"|"vanilla") || u(p,"epoch")?==0 {return Err(invalid());}
    id(p,"attemptId")?; id(p,"profileId")?; u(p,"expectedRevision")?; u(p,"settingsRevision")?; Ok(())
}
pub(crate) fn preparation_input(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["method","params"])?;
    let p=&v["params"];
    match s(v,"method")? {
        "native.prepare.verify" => exact(p,&["operationId","expectedRevision","token"])?,
        "native.apply"|"native.vanilla" => {
            exact(p,&["operationId","expectedRevision","token","deploymentLayout"])?;
            if p["deploymentLayout"]!="shards-v1" {return Err(invalid());}
        }, _ => return Err(invalid()),
    }
    id(p,"operationId")?; u(p,"expectedRevision")?; hex(p,"token",32)
}
pub(crate) fn commit(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["method","params"])?;
    let p=&v["params"]; exact(p,&["epoch","attemptId","reviewToken","preparationOperationId","preparationInput","settingsRevision"])?;
    if v["method"]!="native.launch.commit" || u(p,"epoch")?==0 {return Err(invalid());}
    id(p,"attemptId")?; id(p,"preparationOperationId")?; hex(p,"reviewToken",32)?; u(p,"settingsRevision")?;
    preparation_input(&p["preparationInput"])?;
    if p["preparationInput"]["params"]["operationId"]!=p["preparationOperationId"] || p["preparationInput"]["params"]["token"]!=p["reviewToken"] {return Err(invalid());} Ok(())
}
fn summary(v:&JsonValue, changed:bool, mode:&str)->Result<(),NativeError>{
    let mut keys=if changed {vec!["packageCount","directoryCount","physicalFileCount","companionFileCount","profileId","fileCount","totalBytes","gamePath","replacedMounts","warnings","summaryVersion","deploymentLayout","shardCount","shards"]}
        else {vec!["profileId","gamePath","mode","physicalFileCount","totalBytes","replacedMounts","installedMounts","retainedMounts","warnings","fontsPreserved"]};
    if changed && mode=="vanilla" {keys.extend(["mode","profileName","installedMounts","fontsPreserved","retainedMounts"]);}
    if v.has_key("performance") {
        let p=&v["performance"];
        let mut fields=vec!["presetId","presetName","entryCount","omittedCount","detected","overlayAction","foreignPolicy","restorePolicy"];
        if p.has_key("changes") {
            if !p["changes"].is_array() || p["changes"].len()>4096 {return Err(invalid());}
            for c in p["changes"].members() {
                exact(c,&["name","from","to"])?;
                if !c["name"].is_string() || !(c["from"].is_null()||c["from"].is_string()) || !(c["to"].is_null()||c["to"].is_string()) {return Err(invalid());}
            }
            fields.push("changes");
        }
        exact(p,&fields)?;
        if !changed || !p["presetId"].is_string() || !(p["presetName"].is_null()||p["presetName"].is_string()) || p["entryCount"].as_u64().is_none() || p["omittedCount"].as_u64().is_none() || !p["detected"].is_object()
            || !matches!(p["overlayAction"].as_str(),Some("replace"|"remove"|"preserve")) || p["foreignPolicy"]!="preserve-and-refuse" || p["restorePolicy"]!="exact-prior-bytes" {return Err(invalid());}
        keys.push("performance");
    }
    if v.has_key("repairs") {
        if !changed || !(v["repairs"].is_null()||v["repairs"].is_object()) || v["repairModCount"].as_u64().is_none() || !v["repairSymptoms"].is_array() || !matches!(v["upstreamRepairMigration"].as_str(),None|Some("replaceMountPreserveFolder")){return Err(invalid());}
        keys.extend(["repairs","repairModCount","repairSymptoms","upstreamRepairMigration"]);
    }
    exact(v,&keys)?;id(v,"profileId")?;path(v,"gamePath")?;
    for k in ["physicalFileCount","totalBytes"] {if v[k].as_u64().is_none(){return Err(invalid());}}
    for k in ["replacedMounts","installedMounts","retainedMounts"] {
        if !v.has_key(k){continue;}if !v[k].is_array() || v[k].members().any(|m|m.as_str().is_none()){return Err(invalid());}
    }
    if !v["warnings"].is_array(){return Err(invalid());}
    for warning in v["warnings"].members(){exact(warning,&["code","message"])?;s(warning,"code")?;s(warning,"message")?;}
    if changed {
        for k in ["packageCount","directoryCount","companionFileCount","fileCount","shardCount"]{if v[k].as_u64().is_none(){return Err(invalid());}}
        if v["summaryVersion"]!=2 || v["deploymentLayout"]!="shards-v1" || !v["shards"].is_array() || v["shardCount"].as_usize()!=Some(v["shards"].len()){return Err(invalid());}
        for (i,shard) in v["shards"].members().enumerate(){exact(shard,&["index","directoryCount"])?;if shard["index"].as_usize()!=Some(i+1) || shard["directoryCount"].as_u64().is_none(){return Err(invalid());}}
        if mode=="vanilla" && (v["mode"]!=mode || v["fontsPreserved"]!=true || v["profileName"].as_str().is_none()){return Err(invalid());}
    }else if v["mode"]!=mode || v["fontsPreserved"]!=true || v["physicalFileCount"]!=0 || v["totalBytes"]!=0 || !v["replacedMounts"].is_empty() || !v["installedMounts"].is_empty(){return Err(invalid());}
    Ok(())
}
pub(crate) fn attempt(v: &JsonValue) -> Result<(), NativeError> {
    exact(v,&["version","epoch","attemptId","beginInput","commitInput","recordRevision","state","settingsSha256","ready","preparation","handoff","outcome"])?;
    if v["version"]!=1 || u(v,"epoch")?==0 {return Err(invalid());} id(v,"attemptId")?; u(v,"recordRevision")?; hex(v,"settingsSha256",64)?; begin(&v["beginInput"])?;
    let b=&v["beginInput"]["params"];
    if b["epoch"]!=v["epoch"] || b["attemptId"]!=v["attemptId"] {return Err(invalid());}
    if !v["ready"].is_null() {
        let r=&v["ready"]; exact(r,&["reviewToken","preparationOperationId","preparationInput","changed","summary"])?;
        hex(r,"reviewToken",32)?; id(r,"preparationOperationId")?; preparation_input(&r["preparationInput"])?;
        if r["preparationInput"]["params"]["token"]!=r["reviewToken"] || r["preparationInput"]["params"]["operationId"]!=r["preparationOperationId"]
            || r["preparationInput"]["params"]["expectedRevision"]!=b["expectedRevision"] || r["changed"].as_bool()!=Some(r["preparationInput"]["method"]!="native.prepare.verify")
            || !r["summary"].is_object() {return Err(invalid());}
        summary(&r["summary"],r["changed"]==true,s(b,"mode")?)?;
        if r["summary"]["profileId"]!=b["profileId"] {return Err(invalid());}
        if r["preparationInput"]["method"]=="native.apply" && b["mode"]!="modded" || r["preparationInput"]["method"]=="native.vanilla" && b["mode"]!="vanilla" {return Err(invalid());}
    }
    if !v["commitInput"].is_null() {
        commit(&v["commitInput"])?; let c=&v["commitInput"]["params"]; let r=&v["ready"];
        if r.is_null() || c["epoch"]!=v["epoch"] || c["attemptId"]!=v["attemptId"] || c["settingsRevision"]!=b["settingsRevision"]
            || c["reviewToken"]!=r["reviewToken"] || c["preparationOperationId"]!=r["preparationOperationId"] || c["preparationInput"]!=r["preparationInput"] {return Err(invalid());}
    }
    if !v["preparation"].is_null() {
        let p=&v["preparation"]; exact(p,&["operationId","input","resultingRevision","changed","proofSha256","gameinfoSha256"])?;
        if v["commitInput"].is_null() || p["operationId"]!=v["ready"]["preparationOperationId"] || p["input"]!=v["ready"]["preparationInput"] || p["changed"]!=v["ready"]["changed"] {return Err(invalid());}
        let before=u(b,"expectedRevision")?;
        let after=if p["changed"]==true {before.checked_add(1).ok_or_else(invalid)?}else{before};
        if u(p,"resultingRevision")?!=after {return Err(invalid());} hex(p,"proofSha256",64)?; hex(p,"gameinfoSha256",64)?;
    }
    if !v["handoff"].is_null() {
        exact(&v["handoff"],&["journalId","stage"])?; id(&v["handoff"],"journalId")?;
        if !matches!(s(&v["handoff"],"stage")?,"reserved"|"armed"|"dispatchIntent"|"settling"|"settled") || v["preparation"].is_null() {return Err(invalid());}
    }
    if !v["outcome"].is_null() {
        exact(&v["outcome"],&["status","reason"])?;
        if !matches!((s(&v["outcome"],"status")?,s(&v["outcome"],"reason")?),
            ("cancelled","beforeDispatch")|("failed","preparationFailed"|"preDispatchFailed"|"spawnFailed")|("observed","gameCycleCompleted")|("resolved","steamIntentCancelled"|"startTimedOut")) {return Err(invalid());}
    }
    let valid=match s(v,"state")? {
        "admitted"=>v["ready"].is_null() && v["commitInput"].is_null() && v["preparation"].is_null() && v["handoff"].is_null() && v["outcome"].is_null(),
        "ready"=>!v["ready"].is_null() && v["commitInput"].is_null() && v["preparation"].is_null() && v["handoff"].is_null() && v["outcome"].is_null(),
        "preparing"=>!v["commitInput"].is_null() && v["preparation"].is_null() && v["handoff"].is_null() && v["outcome"].is_null(),
        "prepared"=>!v["preparation"].is_null() && v["handoff"].is_null() && v["outcome"].is_null(),
        "handoff"=>!v["handoff"].is_null() && v["handoff"]["stage"]!="settled" && v["outcome"].is_null(),
        "terminal"=>!v["outcome"].is_null() && (v["handoff"].is_null() || v["handoff"]["stage"]=="settled"),
        _=>false,
    };
    if !valid {return Err(invalid());}
    if v["state"]=="terminal" {
        let reason=s(&v["outcome"],"reason")?;
        if v["handoff"].is_null() && !matches!(reason,"beforeDispatch"|"preparationFailed")
            || !v["handoff"].is_null() && reason=="preparationFailed" {return Err(invalid());}
    }
    bounded(v, CONTROL_LIMIT-1024)
}
pub(crate) fn query(v: &JsonValue) -> Result<JsonValue, NativeError> {
    attempt(v)?;
    let q=object!{version:1,epoch:v["epoch"].clone(),attemptId:v["attemptId"].clone(),recordRevision:v["recordRevision"].clone(),state:v["state"].clone(),
        ready:v["ready"].clone(),preparation:v["preparation"].clone(),handoff:v["handoff"].clone(),outcome:v["outcome"].clone()};
    bounded(&q,CONTROL_LIMIT-256)?; Ok(q)
}
fn directory(v: &JsonValue) -> Result<(),NativeError> {
    if v.has_key("fsid") { exact(v,&["device","inode","fsid"])?;hex(v,"fsid",16)?; }
    else { exact(v,&["device","inode"])?; }
    if v["device"].as_u64().is_none() || v["inode"].as_u64().is_none_or(|n|n==0) {return Err(invalid());} Ok(())
}
fn fingerprint(v: &JsonValue) -> Result<(),NativeError> { identity(v,false) }
/// Repair evidence names game resource files by metadata only (repairs.rs
/// resource_fingerprint: empty sha256); every other identity carries a hash.
fn identity(v: &JsonValue, metadata_only: bool) -> Result<(),NativeError> {
    exact(v,&["sha256","size","device","inode","modifiedSeconds","modifiedNanos"])?;
    if !(metadata_only && v["sha256"]=="") {hex(v,"sha256",64)?;}
    if v["size"].as_u64().is_none() || v["device"].as_u64().is_none() || v["inode"].as_u64().is_none_or(|n|n==0)
        || v["modifiedSeconds"].as_i64().is_none() || v["modifiedNanos"].as_u32().is_none_or(|n| n>=1_000_000_000) {return Err(invalid());} Ok(())
}
pub(crate) fn proof(v: &JsonValue) -> Result<(),NativeError> {
    exact(v,&["version","mode","rootPath","gamePath","profileId","revision","directories","documentSha256","applied","selectedSources","fontState"])?;
    if v["version"]!=1 || !matches!(s(v,"mode")?,"modded"|"vanilla") {return Err(invalid());}
    path(v,"rootPath")?; path(v,"gamePath")?; id(v,"profileId")?; u(v,"revision")?; hex(v,"documentSha256",64)?;
    exact(&v["directories"],&["root","game","citadel"])?;
    for k in ["root","game","citadel"] {directory(&v["directories"][k])?;}
    let e=&v["selectedSources"]; exact(e,&["intent","sources","importedManifests","gameinfo"])?;
    let intent=crate::deployment_intent::DeploymentIntent::decode(&e["intent"])?;
    if intent.profile_id!=s(v,"profileId")? || crate::deployment_intent::applied_intent(&v["applied"])?!=Some(intent.clone()) {return Err(invalid());}
    if v["mode"]=="vanilla" && (e["intent"]["mods"].len()!=0 || v["applied"]["manifest"]["version"]!=3) {return Err(invalid());}
    let checkpoint_doc=object!{storeVersion:if v["applied"]["manifest"]["version"]==3 {4}else{intent.package_version},packageVersion:intent.package_version,applied:v["applied"].clone()};
    crate::deployment_metadata::admit(Path::new(s(v,"rootPath")?),&checkpoint_doc)?;
    let font_doc=object!{storeVersion:4,fonts:v["fontState"].clone()};
    if crate::fonts::FontState::decode(&font_doc)?.encode()?!=v["fontState"] {return Err(invalid());}
    fingerprint(&e["gameinfo"])?;
    if v["applied"]["gameinfoSha256"]!=e["gameinfo"]["sha256"] {return Err(invalid());}
    let game=Path::new(s(v,"gamePath")?);
    for k in ["sources","importedManifests"] {
        if !e[k].is_array() {return Err(invalid());}
        for row in e[k].members() {
            exact(row,&["path","identity"])?; path(row,"path")?;
            let resource=k=="sources" && crate::repairs::resource_path(game,Path::new(s(row,"path")?));
            identity(&row["identity"],resource)?;
        }
    }
    store::ensure_internal(v)
}
pub(crate) fn file_identity(m: &fs::Metadata) -> JsonValue {object!{device:m.dev().to_string(),inode:m.ino().to_string(),uid:m.uid().to_string()}}
pub(crate) fn marker(j: &JsonValue) -> JsonValue {
    object!{kind:"launchHandoff",version:1,journalId:j["id"].clone(),epoch:j["epoch"].clone(),attemptId:j["attemptId"].clone(),
        binding:j["binding"].clone(),proofSha256:j["proofSha256"].clone()}
}
pub(crate) fn journal(j: &JsonValue) -> Result<(),NativeError> {
    let fields=["kind","version","id","epoch","attemptId","recordRevision","stage","binding","preparedProof","proofSha256","reservation","settlement"];
    if j.has_key("dispatchEvidence") {
        let mut with_evidence=fields.to_vec();with_evidence.push("dispatchEvidence");exact(j,&with_evidence)?;
        let e=&j["dispatchEvidence"];exact(e,&["bootId","wallSeconds","bootMillis"])?;
        if !e["bootId"].is_null() {
            let boot=s(e,"bootId")?;
            if boot.len()!=36 || !boot.bytes().enumerate().all(|(i,c)|if [8,13,18,23].contains(&i){c==b'-'}else{c.is_ascii_hexdigit()}) {return Err(invalid());}
        }
        if e["wallSeconds"].is_null() && e["bootMillis"].is_null() {} else {
            if e["bootId"].is_null() {return Err(invalid());}
            u(e,"wallSeconds")?;u(e,"bootMillis")?;
        }
    } else {exact(j,&fields)?;}
    if j["kind"]!="launchHandoff" || j["version"]!=1 || u(j,"epoch")?==0 {return Err(invalid());}
    id(j,"id")?; id(j,"attemptId")?; u(j,"recordRevision")?; hex(j,"proofSha256",64)?; proof(&j["preparedProof"])?;
    if digest(&j["preparedProof"])?!=s(j,"proofSha256")? {return Err(invalid());}
    let b=&j["binding"]; let p=&j["preparedProof"];
    exact(b,&["rootPath","gamePath","rootIdentity","gameIdentity","citadelIdentity","profileId","mode","resultingRevision","settingsRevision","settingsSha256","preparationOperationId","preparationInputSha256","gameinfoSha256"])?;
    path(b,"rootPath")?; path(b,"gamePath")?; id(b,"profileId")?; id(b,"preparationOperationId")?; u(b,"resultingRevision")?; u(b,"settingsRevision")?;
    for k in ["settingsSha256","preparationInputSha256","gameinfoSha256"] {hex(b,k,64)?;}
    for k in ["rootPath","gamePath","profileId","mode"] {if b[k]!=p[k] {return Err(invalid());}}
    for (k,d) in [("rootIdentity","root"),("gameIdentity","game"),("citadelIdentity","citadel")] {directory(&b[k])?; if b[k]!=p["directories"][d] {return Err(invalid());}}
    if b["resultingRevision"]!=p["revision"] || b["gameinfoSha256"]!=p["selectedSources"]["gameinfo"]["sha256"] || j["id"]==b["preparationOperationId"] {return Err(invalid());}
    let r=&j["reservation"]; exact(r,&["tempName","finalName","identity","bytesSha256"])?;
    let temp=s(r,"tempName")?;
    if temp.len()!=60 || !temp.starts_with(".deadlock-native-launch-") || !temp.ends_with(".tmp") || !temp.get( 24..56).is_some_and(|part|crate::package::hex(part,32)) || r["finalName"]!=MARKER {return Err(invalid());}
    if r["identity"].has_key("fsid") {exact(&r["identity"],&["device","inode","uid","fsid"])?;hex(&r["identity"],"fsid",16)?;}
    else {exact(&r["identity"],&["device","inode","uid"])?;}
    if u(&r["identity"],"inode")?==0 || u(&r["identity"],"uid")?>u32::MAX as u64 {return Err(invalid());} u(&r["identity"],"device")?; hex(r,"bytesSha256",64)?;
    let m=marker(j); bounded(&m,MARKER_LIMIT)?;
    if digest(&m)?!=s(r,"bytesSha256")? {return Err(invalid());}
    match s(j,"stage")? {
        "reserved"|"armed"|"dispatchIntent" if j["settlement"].is_null()=>{},
        "settling"=>{
            let x=&j["settlement"]; exact(x,&["reason","expectedRecordRevision","pairSha256","steamIntentCancelled","tempImage","finalPresent"])?;
            u(x,"expectedRecordRevision")?; hex(x,"pairSha256",64)?;
            if x["pairSha256"]!=digest(&object!{reservation:j["reservation"].clone(),binding:j["binding"].clone(),proofSha256:j["proofSha256"].clone()})?
                || x["finalPresent"].as_bool().is_none()
                || !matches!(s(x,"reason")?,"beforeDispatch"|"preDispatchFailed"|"spawnFailed"|"gameCycleCompleted"|"steamIntentCancelled"|"startTimedOut")
                || x["steamIntentCancelled"].as_bool()!=Some(x["reason"]=="steamIntentCancelled") {return Err(invalid());}
            if !x["tempImage"].is_null() {
                exact(&x["tempImage"],&["size","sha256"])?; hex(&x["tempImage"],"sha256",64)?;
                let size=u(&x["tempImage"],"size")?;
                let bytes=store::canonical(&m);
                if size>bytes.len() as u64 || x["tempImage"]["sha256"]!=paths::digest_bytes(&bytes.as_bytes()[..size as usize])? {return Err(invalid());}
            }
            if x["tempImage"].is_null() && x["finalPresent"]==false {return Err(invalid());}
            if x["finalPresent"]==true && !x["tempImage"].is_null() && x["tempImage"]["sha256"]!=r["bytesSha256"] {return Err(invalid());}
            if next_revision(u(x,"expectedRecordRevision")?)? != u(j,"recordRevision")? {return Err(invalid());}
        }, _=>return Err(invalid()),
    }
    store::ensure_internal(&object!{id:j["id"].clone(),body:j.clone()})
}
fn next_revision(n:u64)->Result<u64,NativeError>{n.checked_add(1).ok_or_else(invalid)}
pub(crate) fn record_binding(j: &JsonValue, a: &JsonValue) -> Result<(),NativeError> {
    journal(j)?; attempt(a)?; let b=&j["binding"];
    if a["epoch"]!=j["epoch"] || a["attemptId"]!=j["attemptId"] || a["recordRevision"]!=j["recordRevision"]
        || a["state"]!="handoff" || a["handoff"]["journalId"]!=j["id"] || a["handoff"]["stage"]!=j["stage"]
        || a["preparation"]["proofSha256"]!=j["proofSha256"] || a["preparation"]["gameinfoSha256"]!=b["gameinfoSha256"]
        || a["preparation"]["resultingRevision"]!=b["resultingRevision"] || a["preparation"]["operationId"]!=b["preparationOperationId"]
        || digest(&a["preparation"]["input"])?!=s(b,"preparationInputSha256")? || a["settingsSha256"]!=b["settingsSha256"]
        || a["beginInput"]["params"]["settingsRevision"]!=b["settingsRevision"] || a["beginInput"]["params"]["mode"]!=b["mode"]
        || a["beginInput"]["params"]["profileId"]!=b["profileId"] {return Err(invalid());} Ok(())
}
/// Original document, not snapshot recovery projection, is the evidence anchor.
/// Cleanup uses record_binding only: it never claims prepared game content is live.
pub(crate) fn current_binding(store: &Store, j: &JsonValue, a: &JsonValue, doc: &JsonValue) -> Result<(),NativeError> {
    record_binding(j,a)?; let b=&j["binding"]; let p=&j["preparedProof"];
    if doc["recoveryRequired"]!=false || s(b,"rootPath")?!=store.root().to_str().ok_or_else(invalid)? || doc["gamePath"]!=b["gamePath"] || doc["revision"]!=b["resultingRevision"]
        || digest(doc)?!=s(p,"documentSha256")? || doc["applied"]!=p["applied"] || doc["activeProfileId"]!=p["profileId"]
        || crate::fonts::FontState::decode(doc)?.encode()?!=p["fontState"] {return Err(invalid());}
    let result=store.replay(s(b,"preparationOperationId")?,&a["preparation"]["input"] )?.ok_or_else(invalid)?;
    if result!=store::snapshot_document(doc,None)? {return Err(invalid());} Ok(())
}
pub(crate) fn projection(j: &JsonValue) -> JsonValue {
    object!{version:1,rootPath:j["binding"]["rootPath"].clone(),journalId:j["id"].clone(),epoch:j["epoch"].clone(),attemptId:j["attemptId"].clone(),
        recordRevision:j["recordRevision"].clone(),stage:j["stage"].clone(),preparedOperationId:j["binding"]["preparationOperationId"].clone(),
        preparedRevision:j["binding"]["resultingRevision"].clone(),proofSha256:j["proofSha256"].clone(),fileRecoveryRequired:false}
}

#[cfg(test)] mod perf_review_tests {
 use super::*;
 #[test] fn proof_sources_admit_metadata_only_repair_resources() {
  let id=|sha:&str|object!{sha256:sha,size:52,device:50,inode:7,modifiedSeconds:1,modifiedNanos:2};
  identity(&id(""),true).unwrap();
  identity(&id(&"a".repeat(64)),true).unwrap();
  assert!(identity(&id(""),false).is_err(), "gameinfo and manifests keep their hash");
  assert!(identity(&id("abc"),true).is_err(), "a present hash must still be 64 hex");
 }
 #[test] fn launch_retains_performance_review() {
  let mut v=object!{packageCount:0,directoryCount:0,physicalFileCount:0,companionFileCount:0,profileId:"default",fileCount:0,totalBytes:0,gamePath:"/synthetic/game",replacedMounts:json::array![],warnings:json::array![],summaryVersion:2,deploymentLayout:"shards-v1",shardCount:1,shards:json::array![object!{index:1,directoryCount:0}]};
  summary(&v,true,"modded").unwrap();
  v["repairs"]=JsonValue::Null;v["repairModCount"]=0.into();v["repairSymptoms"]=json::array![];v["upstreamRepairMigration"]=JsonValue::Null;
  summary(&v,true,"modded").unwrap();
  v["performance"]=crate::performance::review(include_str!("../data/perf-stock.gi"),"dmm-clean").unwrap();
  summary(&v,true,"modded").unwrap();
 }
}
