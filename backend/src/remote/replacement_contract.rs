//! Replacement-only wire and durable identities. Add's decoder remains exact.
use super::contract::{self, SubmissionKeyV1};
use crate::{catalog::contract as common, native::NativeError, package, paths};
use json::{array, object, JsonValue};
use std::path::PathBuf;

fn invalid() -> NativeError { NativeError::params() }
fn hash(v: &JsonValue, n: usize) -> Result<(), NativeError> {
    if v.as_str().is_some_and(|s| package::hex(s, n)) { Ok(()) } else { Err(invalid()) }
}
fn token(v: &JsonValue) -> Result<(), NativeError> { common::token(v).map(|_| ()) }
fn id(v: &JsonValue) -> Result<(), NativeError> {
    if v.as_str().is_some_and(paths::safe_id) { Ok(()) } else { Err(invalid()) }
}
fn decimal(v: &JsonValue) -> Result<u64, NativeError> { common::u64_text(v) }
fn positive(v: &JsonValue) -> Result<(), NativeError> {
    if decimal(v)? == 0 { Err(invalid()) } else { Ok(()) }
}
fn boolean(v: &JsonValue) -> Result<(), NativeError> {
    v.as_bool().map(|_| ()).ok_or_else(invalid)
}
fn optional(v: &JsonValue, f: impl FnOnce(&JsonValue) -> Result<(), NativeError>) -> Result<(), NativeError> {
    if v.is_null() { Ok(()) } else { f(v) }
}
fn ids(v: &JsonValue, length: usize) -> Result<(), NativeError> {
    if !v.is_array() || v.is_empty() || v.len() > 99 { return Err(invalid()); }
    let mut previous = None;
    for item in v.members() {
        hash(item, length)?;
        let s = item.as_str().unwrap();
        if previous.is_some_and(|p| p >= s) { return Err(invalid()); }
        previous = Some(s);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplacementPurpose { Update, Force, Reinstall, Link, Append }
impl ReplacementPurpose {
    pub(crate) fn parse(value: &JsonValue) -> Result<Self, NativeError> {
        match value.as_str() {
            Some("link") => Ok(Self::Link), Some("update") => Ok(Self::Update), Some("force") => Ok(Self::Force),
            Some("reinstall") => Ok(Self::Reinstall), Some("append") => Ok(Self::Append), _ => Err(invalid()),
        }
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self { Self::Link => "link", Self::Update => "update", Self::Force => "force", Self::Reinstall => "reinstall", Self::Append => "append" }
    }
}

pub(crate) fn skip_entry(value: &JsonValue) -> Result<(), NativeError> {
    common::exact(value, &["targetDigest", "fileId", "candidateFingerprint"])?;
    hash(&value["targetDigest"], 64)?; positive(&value["fileId"])?;
    hash(&value["candidateFingerprint"], 64)
}
fn desired_skip(value: &JsonValue) -> Result<(), NativeError> {
    common::exact(value, &["fileId", "candidateFingerprint"])?;
    positive(&value["fileId"])?; hash(&value["candidateFingerprint"], 64)
}

/// The durable operation identity intentionally excludes session and token.
#[derive(Clone)]
pub(crate) struct ReplacementCommitInputV1 { value: JsonValue }
impl ReplacementCommitInputV1 {
    pub(crate) fn from_params(p: &JsonValue) -> Result<Self, NativeError> {
        Self::parse(object!{method:"native.remote.replaceCommit",params:object!{
            operationId:p["operationId"].clone(),jobId:p["jobId"].clone(),profileId:p["profileId"].clone(),
            modId:p["modId"].clone(),purpose:p["purpose"].clone(),initialRevision:p["initialRevision"].clone(),
            expectedRevision:p["expectedRevision"].clone(),targetDigest:p["targetDigest"].clone(),
            afterRecordDigest:p["afterRecordDigest"].clone(),consentFingerprint:p["consentFingerprint"].clone(),
            cohortFingerprint:p["cohortFingerprint"].clone(),selectionDigest:p["selectionDigest"].clone()}})
    }
    pub(crate) fn parse(value: JsonValue) -> Result<Self, NativeError> {
        common::exact(&value, &["method", "params"])?;
        if value["method"] != "native.remote.replaceCommit" { return Err(invalid()); }
        let p = &value["params"];
        common::exact(p, &["operationId", "jobId", "profileId", "modId", "purpose", "initialRevision",
            "expectedRevision", "targetDigest", "afterRecordDigest", "consentFingerprint", "cohortFingerprint", "selectionDigest"])?;
        id(&p["operationId"])?; token(&p["jobId"])?; id(&p["profileId"])?; id(&p["modId"])?;
        ReplacementPurpose::parse(&p["purpose"])?;
        if decimal(&p["initialRevision"])? > decimal(&p["expectedRevision"])? { return Err(invalid()); }
        for k in ["targetDigest", "afterRecordDigest", "consentFingerprint", "cohortFingerprint", "selectionDigest"] { hash(&p[k], 64)?; }
        Ok(Self { value })
    }
    pub(crate) fn json(&self) -> &JsonValue { &self.value }
    pub(crate) fn params(&self) -> &JsonValue { &self.value["params"] }
    pub(crate) fn operation_id(&self) -> &str { self.params()["operationId"].as_str().unwrap() }
}

pub(crate) fn selection_digest(input: &ReplacementCommitInputV1, candidates: &JsonValue,
    option_ids: &JsonValue) -> Result<String, NativeError>
{
    ids(candidates, 64)?; ids(option_ids, 32)?;
    if candidates.len() != option_ids.len() { return Err(invalid()); }
    let p = input.params();
    Ok(contract::digest(&array!["remoteReplaceSelection/v1",p["purpose"].clone(),p["profileId"].clone(),
        p["modId"].clone(),p["targetDigest"].clone(),p["initialRevision"].clone(),p["expectedRevision"].clone(),
        p["consentFingerprint"].clone(),p["cohortFingerprint"].clone(),candidates.clone(),option_ids.clone(),p["afterRecordDigest"].clone()]))
}

pub(crate) fn validate_request(method: &str, p: &JsonValue) -> Result<PathBuf, NativeError> {
    let extra: &[&str] = match method {
        "native.remote.replaceTarget" => &["requestKey", "profileId", "modId", "expectedRevision"],
        "native.remote.replaceSkip" => &["requestKey", "profileId", "modId", "expectedRevision", "targetDigest", "expectedSkip", "desiredSkip", "selectionToken"],
        "native.remote.replaceStart" => &["jobId", "selectionToken", "candidateFingerprint", "fileId", "profileId", "modId", "expectedRevision", "targetDigest", "purpose", "acceptArchived"],
        "native.remote.replaceList" => &[],
        "native.remote.replaceStatus" => &["jobId"],
        "native.remote.replaceReview" => &["jobId", "profileId", "modId", "expectedRevision", "targetDigest", "requestKey", "candidateIds"],
        "native.remote.replaceCommit" => &["jobId", "profileId", "modId", "purpose", "initialRevision", "expectedRevision", "targetDigest", "afterRecordDigest", "reviewToken", "operationId", "consentFingerprint", "cohortFingerprint", "selectionDigest"],
        _ => return Err(invalid()),
    };
    let mut keys = vec!["version", "dataDir", "sessionId"]; keys.extend_from_slice(extra);
    common::exact(p, &keys)?;
    if p["version"] != 1 || !common::typed(p) { return Err(invalid()); }
    let root = common::root(&p["dataDir"])?; token(&p["sessionId"])?;
    for k in ["jobId", "requestKey"] { if p.has_key(k) { token(&p[k])?; } }
    for k in ["profileId", "modId", "operationId"] { if p.has_key(k) { id(&p[k])?; } }
    for k in ["expectedRevision", "initialRevision"] { if p.has_key(k) { decimal(&p[k])?; } }
    for k in ["targetDigest", "afterRecordDigest", "candidateFingerprint", "consentFingerprint", "cohortFingerprint", "selectionDigest"] {
        if p.has_key(k) { hash(&p[k], 64)?; }
    }
    if p.has_key("purpose") { ReplacementPurpose::parse(&p["purpose"])?; }
    if p.has_key("fileId") { positive(&p["fileId"])?; }
    if p.has_key("acceptArchived") { boolean(&p["acceptArchived"])?; }
    if p.has_key("candidateIds") { ids(&p["candidateIds"], 64)?; }
    if p.has_key("reviewToken") { optional(&p["reviewToken"], token)?; }
    if method == "native.remote.replaceStart" { token(&p["selectionToken"])?; }
    if method == "native.remote.replaceSkip" {
        optional(&p["expectedSkip"], skip_entry)?; optional(&p["desiredSkip"], desired_skip)?;
        if p["desiredSkip"].is_null() { if !p["selectionToken"].is_null() { return Err(invalid()); } }
        else { token(&p["selectionToken"])?; }
    }
    if method == "native.remote.replaceCommit" { ReplacementCommitInputV1::from_params(p)?; }
    Ok(root)
}

pub(crate) fn validate_receipt(v: &JsonValue) -> Result<(), NativeError> {
    common::exact(v, &["version", "submission", "rootFileId", "cohortFingerprint"])?;
    if v["version"] != 1 { return Err(invalid()); }
    SubmissionKeyV1::parse(&v["submission"])?; positive(&v["rootFileId"])?; hash(&v["cohortFingerprint"], 64)
}

fn common_result(v: &JsonValue, extra: &[&str]) -> Result<(), NativeError> {
    let mut keys=vec!["version","root","sessionId"];keys.extend_from_slice(extra);common::exact(v,&keys)?;
    if v["version"]!=1{return Err(invalid());}common::root(&v["root"])?;token(&v["sessionId"])
}
fn optional_error(v:&JsonValue)->Result<(),NativeError>{optional(v,contract::validate_error_value)}
fn target(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["profileId","modId","revision","recordDigest","receipt","enabled","previewHidden","available","installOrder","metadata","optionCount","selectedOptionIds"])?;
    id(&v["profileId"])?;id(&v["modId"])?;decimal(&v["revision"])?;hash(&v["recordDigest"],64)?;optional(&v["receipt"],validate_receipt)?;
    for k in ["enabled","previewHidden","available"]{boolean(&v[k])?;}
    if v["installOrder"].as_i64().is_none(){return Err(invalid());}contract::validate_metadata_fields(&v["metadata"])?;
    let count=decimal(&v["optionCount"])?;if count==0||count>crate::options::MAX_OPTIONS as u64{return Err(invalid());}
    if !v["selectedOptionIds"].is_array() || !v["selectedOptionIds"].is_empty() {ids(&v["selectedOptionIds"],32)?;}if v["selectedOptionIds"].len() as u64>count{return Err(invalid());}Ok(())
}
fn review(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["requestKey","phase","reviewToken","selectionDigest","afterRecordDigest","summary","error"])?;
    token(&v["requestKey"])?;optional(&v["reviewToken"],token)?;optional(&v["selectionDigest"],|x|hash(x,64))?;
    optional(&v["afterRecordDigest"],|x|hash(x,64))?;optional(&v["summary"],contract::validate_summary)?;optional_error(&v["error"])?;
    let ready=!v["reviewToken"].is_null()&&!v["selectionDigest"].is_null()&&!v["afterRecordDigest"].is_null()&&!v["summary"].is_null();
    let empty=v["reviewToken"].is_null()&&v["selectionDigest"].is_null()&&v["afterRecordDigest"].is_null()&&v["summary"].is_null();
    match v["phase"].as_str(){Some("ready") if ready&&v["error"].is_null()=>Ok(()),Some("pending") if empty&&v["error"].is_null()=>Ok(()),Some("failed") if empty&&!v["error"].is_null()=>Ok(()),_=>Err(invalid())}
}
pub(crate) fn validate_result(method:&str,v:&JsonValue)->Result<(),NativeError>{
    if method=="native.remote.replaceCommit"&&v.has_key("protocolVersion"){
        return contract::validate_result("native.remote.commit",v);
    }
    match method {
        "native.remote.replaceTarget"=>{
            common_result(v,&["requestKey","phase","target","skip","error"])?;token(&v["requestKey"])?;optional_error(&v["error"])?;
            match v["phase"].as_str(){
                Some("ready") if v["error"].is_null()=>{
                    target(&v["target"])?;let skip=&v["skip"];common::exact(skip,&["state","entry"])?;
                    match skip["state"].as_str(){
                        Some("none"|"unavailable") if skip["entry"].is_null()=>{},
                        Some("present")=>{skip_entry(&skip["entry"])?;if skip["entry"]["targetDigest"]!=v["target"]["recordDigest"]{return Err(invalid());}},
                        Some("stale")=>{skip_entry(&skip["entry"])?;if skip["entry"]["targetDigest"]==v["target"]["recordDigest"]{return Err(invalid());}},
                        _=>return Err(invalid()),
                    }Ok(())
                },
                Some("pending") if v["target"].is_null()&&v["skip"].is_null()&&v["error"].is_null()=>Ok(()),
                Some("failed") if v["target"].is_null()&&v["skip"].is_null()&&!v["error"].is_null()=>Ok(()),_=>Err(invalid()),
            }
        },
        "native.remote.replaceSkip"=>{
            common_result(v,&["requestKey","phase","entry","error"])?;token(&v["requestKey"])?;
            optional(&v["entry"],skip_entry)?;optional_error(&v["error"])?;
            match v["phase"].as_str(){Some("ready") if v["error"].is_null()=>Ok(()),
                Some("pending") if v["entry"].is_null()&&v["error"].is_null()=>Ok(()),
                Some("failed") if v["entry"].is_null()&&!v["error"].is_null()=>Ok(()),_=>Err(invalid())}
        },
        "native.remote.replaceList"=>{
            common_result(v,&["jobs"])?;let jobs=&v["jobs"];if !jobs.is_array()||jobs.len()>8{return Err(invalid());}
            for row in jobs.members(){common::exact(row,&["jobId","submission","profileId","state","sequence","cleanupPending","purpose","modId","targetDigest"])?;
                token(&row["jobId"])?;SubmissionKeyV1::parse(&row["submission"])?;id(&row["profileId"])?;id(&row["modId"])?;
                ReplacementPurpose::parse(&row["purpose"])?;hash(&row["targetDigest"],64)?;decimal(&row["sequence"])?;boolean(&row["cleanupPending"])?;
                if !row["state"].as_str().is_some_and(|s|!s.is_empty()&&s.len()<=32&&!s.chars().any(char::is_control)){return Err(invalid());}}
            Ok(())
        },
        "native.remote.replaceStatus"=>{
            common_result(v,&["purpose","modId","targetDigest","status","review"])?;ReplacementPurpose::parse(&v["purpose"])?;
            id(&v["modId"])?;hash(&v["targetDigest"],64)?;contract::validate_status(&v["status"])?;
            if !v["status"]["review"].is_null()||v["status"]["root"]!=v["root"]||v["status"]["sessionId"]!=v["sessionId"]{return Err(invalid());}
            optional(&v["review"],review)
        },
        "native.remote.replaceStart"|"native.remote.replaceReview"|"native.remote.replaceCommit"=>{
            let extra:&[&str]=match method{"native.remote.replaceStart"=>&["jobId","state"],"native.remote.replaceReview"=>&["jobId","requestKey","state"],_=>&["jobId","operationId","state"]};
            common_result(v,extra)?;token(&v["jobId"])?;if v.has_key("requestKey"){token(&v["requestKey"])?;}if v.has_key("operationId"){id(&v["operationId"])?;}
            if v["state"]!="pending"{return Err(invalid());}Ok(())
        },_=>Err(invalid()),
    }
}
pub(crate) fn validate_binding(method:&str,request:&JsonValue,result:&JsonValue)->Result<(),NativeError>{
    if method=="native.remote.replaceCommit"&&result.has_key("protocolVersion"){
        return contract::validate_result_binding("native.remote.commit",request,result);
    }
    if common::root(&result["root"])?!=common::root(&request["dataDir"])?||result["sessionId"]!=request["sessionId"]{return Err(invalid());}
    for k in ["jobId","requestKey","operationId"]{if request.has_key(k)&&result.has_key(k)&&request[k]!=result[k]{return Err(invalid());}}
    if method=="native.remote.replaceTarget"&&result["phase"]=="ready" {
        for (r,t) in [("profileId","profileId"),("modId","modId"),("expectedRevision","revision")]{if request[r]!=result["target"][t]{return Err(invalid());}}
    }
    if method=="native.remote.replaceStatus"&&result["status"]["jobId"]!=request["jobId"]{return Err(invalid());}
    Ok(())
}
