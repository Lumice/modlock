//! Session-only fresh GameBanana authority. Never accepts cached catalog projections.
use super::{contract::{self, ConsentFile, SessionId, SubmissionKeyV1}, metadata::MetadataCompletion};
use crate::{catalog::{provider,transport::MetadataRequest},native::NativeError,paths};
use json::{object,JsonValue};
use std::{collections::BTreeSet,time::{Duration,Instant}};

#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum MetadataRoute { Profile, Files, Servers }
fn route_url(submission:&SubmissionKeyV1,route:MetadataRoute)->String{
    match route{
        MetadataRoute::Profile=>format!("https://gamebanana.com/apiv11/{}/{}/ProfilePage",submission.model(),submission.submission_id),
        MetadataRoute::Files=>format!("https://gamebanana.com/apiv11/{}/{}/DownloadPage",submission.model(),submission.submission_id),
        MetadataRoute::Servers=>"https://gamebanana.com/apiv11/Util/Fileservers?_nPage=1".into(),
    }
}
pub(crate) fn metadata_request(submission:&SubmissionKeyV1,route:MetadataRoute)->Result<MetadataRequest,NativeError>{
    SubmissionKeyV1::parse(&submission.json())?;
    Ok(MetadataRequest{request_id:crate::catalog::contract::new_token()?,url:route_url(submission,route)})
}
/// This stores metadata only; the transport's trusted pre-dispatch time is not
/// reconstructed from a later status poll or from provider-controlled input.
pub(crate) struct RawResponse { route:MetadataRoute,request_url:String,final_url:String,status:u16,body:Vec<u8>,requested_at:Instant,session:SessionId }
impl RawResponse {
    pub(crate) fn from_completion(route:MetadataRoute,completion:MetadataCompletion)->Result<Self,NativeError>{
        if let Some(error)=completion.error.as_ref(){return Err(match error.kind.as_str(){
            "transportTls"=>NativeError::new("transportTls","Fresh metadata TLS validation failed."),
            "transportTimeout"=>NativeError::new("transportTimeout","Fresh metadata request timed out."),
            "transportStorage"=>NativeError::new("transportStorage","Fresh metadata storage is unavailable."),
            "transportProtocol"|"transportBodyLimit"=>NativeError::new("transportFraming","Fresh metadata framing is unavailable."),
            "transportRedirect"=>host_error(),
            _=>NativeError::new("transportNetwork","Fresh metadata network request failed."),
        });}
        let status=completion.http_status.ok_or_else(unavailable)?;
        match status{401|403=>return Err(denied()),404=>return Err(NativeError::new("providerNotFound","Provider metadata was not found.")),200=>{},_=>return Err(unavailable())}
        let body=completion.body.ok_or_else(unavailable)?;if body.len()>contract::BODY_LIMIT{return Err(malformed());}
        Ok(Self{route,request_url:completion.request_url,final_url:completion.final_url.ok_or_else(unavailable)?,status,body,requested_at:completion.requested_at,session:completion.session})
    }
}
#[derive(Clone)]
pub(crate) struct ExpectedSelection {
    pub submission_name:String,pub root:ConsentFile,pub candidate_fingerprint:String,pub accept_archived:bool,
    /// None means the family has not yet been discovered; Some is complete.
    pub cohort:Option<Vec<ConsentFile>>,pub cohort_fingerprint:Option<String>,
}
pub(crate) struct AuthorityRequest {pub session:SessionId,pub submission:SubmissionKeyV1,pub selected:Option<ExpectedSelection>}
struct AuthorizedFile {consent:ConsentFile,url:String,description:String}
/// Deliberately lacks serialization and Debug: URLs and host authority stay local.
pub(crate) struct FreshAuthorization {
    session:SessionId,submission:SubmissionKeyV1,submission_name:String,files:Vec<AuthorizedFile>,allowed_hosts:BTreeSet<String>,denied_hosts:BTreeSet<String>,
    selected_root:Option<String>,cohort_fingerprint:Option<String>,expires_at:Instant,
}
impl FreshAuthorization {
    pub(crate) fn candidates(&self)->Vec<JsonValue>{self.files.iter().map(|f|object!{
        fileKey:object!{submission:self.submission.json(),fileId:f.consent.file_id.clone()},submissionName:self.submission_name.clone(),
        description:f.description.clone(),originalName:f.consent.original_name.clone(),declaredSize:f.consent.json()["declaredSize"].clone(),createdAt:f.consent.json()["createdAt"].clone(),
        md5:f.consent.json()["md5"].clone(),list:f.consent.list.clone(),archiveFlag:f.consent.json()["archiveFlag"].clone(),
        candidateFingerprint:contract::candidate_fingerprint(&self.submission,&self.submission_name,&f.consent)
    }).collect()}
    pub(crate) fn submission(&self)->&SubmissionKeyV1{&self.submission}
    pub(crate) fn submission_name(&self)->&str{&self.submission_name}
    pub(crate) fn selected_root(&self)->Option<&str>{self.selected_root.as_deref()}
    pub(crate) fn cohort_fingerprint(&self)->Option<&str>{self.cohort_fingerprint.as_deref()}
    pub(crate) fn expires_at(&self)->Instant{self.expires_at}
    pub(crate) fn valid_at(&self,now:Instant)->bool{now<self.expires_at}
    pub(crate) fn allowed_hosts(&self,session:&SessionId,now:Instant)->Option<&BTreeSet<String>>{if &self.session==session&&self.valid_at(now){Some(&self.allowed_hosts)}else{None}}
    pub(crate) fn denied_hosts(&self,session:&SessionId,now:Instant)->Option<&BTreeSet<String>>{if &self.session==session&&self.valid_at(now){Some(&self.denied_hosts)}else{None}}
    pub(crate) fn consent(&self,file_id:&str)->Option<&ConsentFile>{self.files.iter().find(|f|f.consent.file_id==file_id).map(|f|&f.consent)}
    pub(crate) fn request_url(&self,session:&SessionId,file_id:&str,now:Instant)->Option<&str>{if &self.session!=session||!self.valid_at(now){return None;}self.files.iter().find(|f|f.consent.file_id==file_id).map(|f|f.url.as_str())}
    pub(crate) fn allows_url(&self,session:&SessionId,url:&str,now:Instant)->bool{&self.session==session&&self.valid_at(now)&&parse_https_url(url).is_ok_and(|host|!self.denied_hosts.contains(&host)&&self.allowed_hosts.contains(&host))}
    pub(crate) fn allows_header_url(&self,session:&SessionId,url:&str,canonical_target:Option<&str>,now:Instant)->bool{
        if &self.session!=session||!self.valid_at(now){return false;}
        parse_https_url(url).is_ok_and(|host|!self.denied_hosts.contains(&host)&&(self.allowed_hosts.contains(&host)||(canonical_target==Some(url)&&is_canonical_cache_host(&host))))
    }
    pub(crate) fn allows_redirect(&self,session:&SessionId,from_url:&str,to_url:&str,now:Instant)->bool{
        if &self.session!=session||!self.valid_at(now){return false;}
        let Ok(from_host)=parse_https_url(from_url) else {return false;};
        let Ok(to_host)=parse_https_url(to_url) else {return false;};
        if self.denied_hosts.contains(&from_host)||self.denied_hosts.contains(&to_host)||!self.allowed_hosts.contains(&from_host){return false;}
        if self.allowed_hosts.contains(&to_host){return true;}
        is_canonical_redirect_origin(&from_host)&&is_canonical_cache_host(&to_host)
    }
}
pub(crate) struct FreshFileAuthorityV1;
fn malformed()->NativeError{NativeError::new("providerMalformed","Fresh provider metadata is malformed or contradicts its request.")}
fn unknown()->NativeError{NativeError::new("eligibilityUnknown","Fresh provider eligibility evidence is incomplete.")}
fn denied()->NativeError{NativeError::new("providerDenied","The provider does not permit this submission.")}
fn unavailable()->NativeError{NativeError::new("providerUnavailable","Fresh provider metadata is unavailable.")}
fn changed()->NativeError{NativeError::new("needsSelection","Fresh provider metadata no longer matches the selected files.")}
fn host_error()->NativeError{NativeError::new("hostNotAllowed","The provider download host is not in the fresh allowed set.")}

fn label(s:&str)->bool{
    !s.is_empty()&&s.len()<=63&&s.is_ascii()&&s.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'-')
        &&s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)&&s.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
}
pub(crate) fn valid_allowed_host(host:&str)->bool{
    if host=="gamebanana.com"||host=="api.gamebanana.com"{return true;}
    host.strip_suffix(".gamebanana.com").is_some_and(label)
}
pub(crate) fn is_canonical_redirect_origin(host:&str)->bool{
    matches!(host,"gamebanana.com"|"files.gamebanana.com")
}
pub(crate) fn is_canonical_cache_host(host:&str)->bool{
    host.strip_suffix(".gamebanana.com")
        .and_then(|label_part|label_part.strip_prefix("filecache"))
        .is_some_and(|digits|!digits.is_empty()&&digits.len()<=6&&digits.bytes().all(|b|b.is_ascii_digit()))
}
/// Strict encoded HTTPS URI parser. It never decodes or rewrites the resource
/// path/query, so the transfer owner can bind resume to the exact encoded URI.
pub(crate) fn parse_https_url(url:&str)->Result<String,NativeError>{
    if url.len()>4096||!url.is_ascii()||url.bytes().any(|b|b<=32||b==127||b==b'\\'||b==b'#'){return Err(host_error());}
    let rest=url.strip_prefix("https://").ok_or_else(host_error)?;
    let split=rest.find(['/', '?']).unwrap_or(rest.len());let authority=&rest[..split];
    if authority.contains('@')||authority.contains('%')||authority.contains('[')||authority.contains(']'){return Err(host_error());}
    let host=if let Some((host,port))=authority.split_once(':'){if port!="443"||host.contains(':'){return Err(host_error());}host}else{authority};
    if !valid_allowed_host(host){return Err(host_error());}
    let tail=&rest.as_bytes()[split..];let mut i=0;
    while i<tail.len(){if tail[i]==b'%' {
        if i+2>=tail.len()||!tail[i+1].is_ascii_hexdigit()||!tail[i+2].is_ascii_hexdigit(){return Err(host_error());}
        if tail[i+1].is_ascii_lowercase()||tail[i+2].is_ascii_lowercase(){return Err(host_error());}
        let h=|c:u8|if c.is_ascii_digit(){c-b'0'}else{c.to_ascii_lowercase()-b'a'+10};let n=h(tail[i+1])*16+h(tail[i+2]);
        if n<=32||n>=127||n.is_ascii_alphanumeric()||b"-._~\\/#?%@".contains(&n){return Err(host_error());}i+=3;
    }else{i+=1;}}
    Ok(host.to_owned())
}
fn object(v:&JsonValue)->Result<(),NativeError>{if v.is_object(){Ok(())}else{Err(malformed())}}
fn required_false(v:&JsonValue,key:&str)->Result<(),NativeError>{
    if !v.has_key(key)||v[key].is_null(){return Err(unknown());}
    match v[key].as_bool(){Some(false)=>Ok(()),Some(true)=>Err(denied()),None=>Err(malformed())}
}
fn checked_id(v:&JsonValue)->Result<String,NativeError>{v.as_u64().filter(|n|*n>0).map(|n|n.to_string()).ok_or_else(malformed)}
fn optional_number(v:&JsonValue,key:&str)->Result<Option<String>,NativeError>{
    if !v.has_key(key)||v[key].is_null(){return Ok(None);}v[key].as_u64().map(|n|Some(n.to_string())).ok_or_else(malformed)
}
fn reject_provider_error(v:&JsonValue)->Result<(),NativeError>{
    if ["error","error_code","_sErrorCode"].iter().any(|k|v.has_key(k)){Err(malformed())}else{Ok(())}
}
fn parse_profile(v:&JsonValue,key:&SubmissionKeyV1)->Result<String,NativeError>{
    object(v)?;reject_provider_error(v)?;
    if checked_id(&v["_idRow"])?!=key.submission_id||v["_sProfileUrl"].as_str()!=Some(key.profile_url().as_str()){return Err(malformed());}
    if v.has_key("_sModelName")&&v["_sModelName"].as_str()!=Some(key.model()){return Err(malformed());}
    for k in ["_bIsPrivate","_bIsTrashed","_bIsWithheld"]{required_false(v,k)?;}
    if !v.has_key("_aGame")||v["_aGame"].is_null(){return Err(unknown());}
    if !v["_aGame"].is_object(){return Err(malformed());}
    if !v["_aGame"].has_key("_idRow")||v["_aGame"]["_idRow"].is_null(){return Err(unknown());}
    if checked_id(&v["_aGame"]["_idRow"])?!="20948"{return Err(denied());}
    v["_sName"].as_str().filter(|s|!s.trim().is_empty()&&s.len()<=256&&!s.chars().any(char::is_control)).map(str::to_owned).ok_or_else(malformed)
}
fn parse_files(v:&JsonValue,key:&SubmissionKeyV1)->Result<Vec<AuthorizedFile>,NativeError>{
    object(v)?;reject_provider_error(v)?;required_false(v,"_bIsTrashed")?;required_false(v,"_bIsWithheld")?;
    if v.has_key("_idRow")&&checked_id(&v["_idRow"])?!=key.submission_id{return Err(malformed());}
    if v.has_key("_sModelName")&&v["_sModelName"].as_str()!=Some(key.model()){return Err(malformed());}
    if !v.has_key("_aFiles")&&!v.has_key("_aArchivedFiles"){return Err(malformed());}
    let mut files=Vec::new();let mut ids=BTreeSet::new();
    for (field,list) in [("_aFiles","current"),("_aArchivedFiles","archived")]{
        if !v.has_key(field){continue;}let rows=&v[field];if !rows.is_array(){return Err(malformed());}
        for row in rows.members(){
            object(row)?;let file_id=checked_id(&row["_idRow"])?;
            let name=row["_sFile"].as_str().filter(|s|paths::safe_name(s)).ok_or_else(malformed)?.to_owned();
            if !ids.insert(file_id.clone()){return Err(malformed());}
            let size=optional_number(row,"_nFilesize")?;let time=optional_number(row,"_tsDateAdded")?;
            let md5=if !row.has_key("_sMd5Checksum")||row["_sMd5Checksum"].is_null(){None}else{
                let value=row["_sMd5Checksum"].as_str().filter(|s|s.len()==32&&s.bytes().all(|b|b.is_ascii_hexdigit())).ok_or_else(malformed)?;Some(value.to_ascii_lowercase())
            };
            let flag=if !row.has_key("_bIsArchived")||row["_bIsArchived"].is_null(){None}else{Some(row["_bIsArchived"].as_bool().ok_or_else(malformed)?)};
            let url=row["_sDownloadUrl"].as_str().ok_or_else(malformed)?.to_owned();parse_https_url(&url)?;
            // Presentation only: keep provider text outside ConsentFile and all fingerprints.
            let raw=row["_sDescription"].as_str().unwrap_or("");
            // 1 KiB keeps eight escaped rows well inside the 64 KiB response limit.
            let mut end=raw.len().min(1024);
            while !raw.is_char_boundary(end){end-=1;}
            let description=provider::plain(&raw[..end]).chars().filter(|c|!c.is_control()).collect::<String>();
            files.push(AuthorizedFile{consent:ConsentFile{file_id,original_name:name,declared_size:size,created_at:time,md5,list:list.into(),archive_flag:flag},url,description});
            if files.len()>contract::MAX_PROVIDER_ROWS{return Err(malformed());}
        }
    }Ok(files)
}
fn parse_servers(v:&JsonValue)->Result<(BTreeSet<String>,BTreeSet<String>),NativeError>{
    object(v)?;reject_provider_error(v)?;let rows=&v["_aRecords"];if !v.has_key("_aRecords")||!rows.is_array()||rows.len()>128{return Err(malformed());}
    // Only up rows join allowed_hosts. Non-up rows are explicitly tracked in
    // denied_hosts; canonical download redirects may reach unlisted filecache<N>
    // nodes only when not denied and originated from gamebanana.com/files.gamebanana.com.
    let mut ids=BTreeSet::new();let mut domains=BTreeSet::new();let mut hosts=BTreeSet::from(["gamebanana.com".to_string(),"api.gamebanana.com".to_string(),"files.gamebanana.com".to_string()]);
    let mut denied=BTreeSet::new();
    for row in rows.members(){object(row)?;let id=checked_id(&row["_idRow"])?;let d=row["_sDomain"].as_str().filter(|s|s.is_ascii()).ok_or_else(malformed)?.to_ascii_lowercase();
        if !label(&d)||!ids.insert(id)||!domains.insert(d.clone()){return Err(malformed());}
        let state=row["_sState"].as_str().filter(|s|s.len()<=128&&!s.chars().any(char::is_control)).ok_or_else(malformed)?;
        let fqdn=format!("{d}.gamebanana.com");
        if state=="up"{hosts.insert(fqdn);if hosts.len()>64{return Err(unavailable());}}
        else {hosts.remove(&fqdn);denied.insert(fqdn);}
    }Ok((hosts,denied))
}

// The generic provider parser preserves unrelated rich fields and fractional
// numbers. Check canonical numeric spelling only at paths used as authority;
// the shared strict scanner already rejects duplicate decoded keys everywhere.
fn numeric_authority_path(route:MetadataRoute,path:&[String])->bool{
    let p=path.iter().map(String::as_str).collect::<Vec<_>>();
    match route{
        MetadataRoute::Profile=>matches!(p.as_slice(),["_idRow"]|["_aGame","_idRow"]),
        MetadataRoute::Files=>matches!(p.as_slice(),["_idRow"]|["_aFiles"|"_aArchivedFiles","[]","_idRow"|"_nFilesize"|"_tsDateAdded"]),
        MetadataRoute::Servers=>matches!(p.as_slice(),["_aRecords","[]","_idRow"]),
    }
}
fn authority_numbers(bytes:&[u8],route:MetadataRoute)->Result<(),NativeError>{
    fn space(b:&[u8],i:&mut usize){while b.get(*i).is_some_and(u8::is_ascii_whitespace){*i+=1;}}
    fn string(b:&[u8],i:&mut usize)->Result<String,NativeError>{
        let start=*i;if b.get(*i)!=Some(&b'"'){return Err(malformed());}*i+=1;
        while let Some(c)=b.get(*i){*i+=1;if *c==b'\\'{*i+=1;}else if *c==b'"'{
            return json::parse(std::str::from_utf8(&b[start..*i]).map_err(|_|malformed())?).ok().and_then(|v|v.as_str().map(str::to_owned)).ok_or_else(malformed);
        }}Err(malformed())
    }
    fn value(b:&[u8],i:&mut usize,path:&mut Vec<String>,route:MetadataRoute)->Result<(),NativeError>{
        space(b,i);match b.get(*i).copied().ok_or_else(malformed)?{
            b'{' =>{*i+=1;space(b,i);if b.get(*i)==Some(&b'}'){*i+=1;return Ok(());}
                loop{space(b,i);let key=string(b,i)?;space(b,i);if b.get(*i)!=Some(&b':'){return Err(malformed());}*i+=1;path.push(key);value(b,i,path,route)?;path.pop();space(b,i);
                    match b.get(*i){Some(b'}')=>{*i+=1;break;},Some(b',')=>*i+=1,_=>return Err(malformed())}}
            },
            b'['=>{*i+=1;space(b,i);if b.get(*i)==Some(&b']'){*i+=1;return Ok(());}path.push("[]".into());loop{value(b,i,path,route)?;space(b,i);
                match b.get(*i){Some(b']')=>{*i+=1;break;},Some(b',')=>*i+=1,_=>return Err(malformed())}}path.pop();},
            b'"'=>{string(b,i)?;},
            _=>{let start=*i;while b.get(*i).is_some_and(|c|!c.is_ascii_whitespace()&&!b",]}".contains(c)){*i+=1;}if *i==start{return Err(malformed());}
                let raw=&b[start..*i];if numeric_authority_path(route,path)&&matches!(raw.first(),Some(b'-'|b'0'..=b'9')){
                    if raw.len()>20||!raw.iter().all(u8::is_ascii_digit)||(raw.len()>1&&raw[0]==b'0'){return Err(malformed());}
                }
            }
        }Ok(())
    }
    let mut i=0;value(bytes,&mut i,&mut Vec::new(),route)?;space(bytes,&mut i);if i!=bytes.len(){return Err(malformed());}Ok(())
}
fn parse_raw(response:&RawResponse)->Result<JsonValue,NativeError>{
    if response.body.len()>contract::BODY_LIMIT{return Err(malformed());}
    let value=provider::parse_bounded(&response.body,contract::BODY_LIMIT).map_err(|_|malformed())?;
    authority_numbers(&response.body,response.route)?;Ok(value)
}
impl FreshFileAuthorityV1 {
    pub(crate) fn evaluate(request:&AuthorityRequest,responses:&[RawResponse;3],now:Instant)->Result<FreshAuthorization,NativeError>{
        SubmissionKeyV1::parse(&request.submission.json()).map_err(|_|malformed())?;
        let routes=[MetadataRoute::Profile,MetadataRoute::Files,MetadataRoute::Servers];
        let mut oldest=now;
        for (response,route) in responses.iter().zip(routes){
            let expected=route_url(&request.submission,route);
            if response.session!=request.session||response.route!=route||response.request_url!=expected||response.final_url!=expected||response.status!=200{return Err(malformed());}
            if response.requested_at>now{return Err(malformed());}oldest=oldest.min(response.requested_at);
        }
        let expires_at=oldest.checked_add(Duration::from_secs(300)).ok_or_else(unavailable)?;if now>=expires_at{return Err(unavailable());}
        let profile=parse_raw(&responses[0])?;let downloads=parse_raw(&responses[1])?;let servers=parse_raw(&responses[2])?;
        let submission_name=parse_profile(&profile,&request.submission)?;let files=parse_files(&downloads,&request.submission)?;let (allowed_hosts,denied_hosts)=parse_servers(&servers)?;
        for file in &files{let host=parse_https_url(&file.url)?;if denied_hosts.contains(&host)||!allowed_hosts.contains(&host){return Err(host_error());}}
        let mut selected_root=None;let mut cohort_fingerprint=None;
        if let Some(selected)=&request.selected{
            ConsentFile::parse(&selected.root.json()).map_err(|_|changed())?;
            let root=files.iter().find(|f|f.consent.file_id==selected.root.file_id).ok_or_else(changed)?;
            if selected.submission_name!=submission_name||root.consent!=selected.root
                ||contract::candidate_fingerprint(&request.submission,&submission_name,&root.consent)!=selected.candidate_fingerprint{return Err(changed());}
            if root.consent.archived()&&!selected.accept_archived{return Err(denied());}
            selected_root=Some(root.consent.file_id.clone());
            match (&selected.cohort,&selected.cohort_fingerprint){
                (None,None)=>{},
                (Some(cohort),Some(expected))=>{
                    let actual=contract::cohort_fingerprint(&request.submission,&root.consent.file_id,cohort).map_err(|_|changed())?;
                    if &actual!=expected{return Err(changed());}
                    for member in cohort{
                        let fresh=files.iter().find(|f|f.consent.file_id==member.file_id).ok_or_else(changed)?;
                        if &fresh.consent!=member||member.list!=root.consent.list{return Err(changed());}
                        if member.archived()&&!selected.accept_archived{return Err(denied());}
                    }
                    cohort_fingerprint=Some(actual);
                },_=>return Err(changed()),
            }
        }
        Ok(FreshAuthorization{session:request.session.clone(),submission:request.submission.clone(),submission_name,files,allowed_hosts,denied_hosts,selected_root,cohort_fingerprint,expires_at})
    }
}

#[cfg(test)]
pub(crate) mod regression_tests {
    use super::*;
    pub(crate) fn captured_fileservers_json() -> JsonValue {
        let mut records = json::JsonValue::new_array();
        for (id, domain, state) in [
            (1u64, "filecache20", "up"), (6, "filecache31", "up"), (9, "filecache35", "up"),
            (10, "filecache36", "up"), (11, "filecache37", "up"), (12, "filecache38", "up"),
            (13, "filecache39", "up"), (14, "filecache40", "up"), (15, "filecache41", "up"),
            (16, "filecache42", "up"), (17, "filecache43", "up"), (18, "filecache44", "up"),
            (19, "filecache45", "up"), (20, "filecache46", "up"), (21, "filecache47", "up"),
            (24, "filecache52", "up"), (25, "filecache53", "up"), (26, "filecache54", "up"),
            (2, "filecache25", "terminated"), (7, "filecache32", "terminated"), (8, "filecache33", "terminated"),
        ] {
            records.push(object! { _idRow: id, _sDomain: domain, _sState: state }).unwrap();
        }
        object! { _aMetadata: object! { _nRecordCount: 21, _bIsComplete: true }, _aRecords: records }
    }

    #[test]
    fn file_description_is_display_only_bounded_plain_text() {
        let key=SubmissionKeyV1{submission_type:"mod".into(),submission_id:"42".into()};
        let session=SessionId::parse(&"a".repeat(32)).unwrap();
        let now=Instant::now();
        let profile=object!{_idRow:42,_sProfileUrl:key.profile_url(),_sName:"Variants",_bIsPrivate:false,_bIsTrashed:false,_bIsWithheld:false,_aGame:object!{_idRow:20948}};
        let mut downloads=object!{_bIsTrashed:false,_bIsWithheld:false,_aFiles:json::array![object!{_idRow:12,_sFile:"variant.zip",_sDownloadUrl:"https://files.gamebanana.com/variant.zip",_bIsArchived:false}]};
        let servers=object!{_aRecords:json::array![]};
        let responses=|downloads:&JsonValue| {
            [profile.clone(),downloads.clone(),servers.clone()].into_iter().zip([MetadataRoute::Profile,MetadataRoute::Files,MetadataRoute::Servers]).map(|(body,route)| {
                let url=route_url(&key,route);
                RawResponse{route,request_url:url.clone(),final_url:url,status:200,body:body.dump().into_bytes(),requested_at:now,session:session.clone()}
            }).collect::<Vec<_>>().try_into().ok().unwrap()
        };
        let request=AuthorityRequest{session:session.clone(),submission:key.clone(),selected:None};
        let original=FreshFileAuthorityV1::evaluate(&request,&responses(&downloads),now).unwrap();
        let candidate=original.candidates().remove(0);
        assert_eq!(candidate["description"],"");
        let selected=ExpectedSelection{submission_name:"Variants".into(),root:original.consent("12").unwrap().clone(),
            candidate_fingerprint:candidate["candidateFingerprint"].as_str().unwrap().into(),accept_archived:false,cohort:None,cohort_fingerprint:None};
        let request=AuthorityRequest{session:session.clone(),submission:key.clone(),selected:Some(selected)};
        for (input,expected) in [(JsonValue::from("<b>reze_human</b> &amp; bomb\u{0}\u{1b}")," reze_human  & bomb".to_string()),
            (JsonValue::from("é".repeat(3000)),"é".repeat(512)),
            (JsonValue::from(format!("{}é","a".repeat(1023))),"a".repeat(1023)),
            (JsonValue::Null,String::new()),(JsonValue::from(7),String::new())] {
            downloads["_aFiles"][0]["_sDescription"]=input;
            let fresh=FreshFileAuthorityV1::evaluate(&request,&responses(&downloads),now).unwrap();
            let row=fresh.candidates().remove(0);
            assert_eq!(row["description"],expected);
            assert_eq!(row["candidateFingerprint"],candidate["candidateFingerprint"]);
            assert_eq!(fresh.selected_root(),Some("12"));
            contract::validate_result("native.remote.resolveStatus",&object!{version:1,root:"/synthetic",sessionId:request.session.as_str(),requestKey:"b".repeat(32),phase:"ready",page:0,hasMore:false,selectionToken:"c".repeat(32),candidates:json::array![row],error:JsonValue::Null}).unwrap();
        }
    }

    #[test]
    fn canonical_alias_is_exact_and_https_only() {
        let (hosts,denied)=parse_servers(&object!{_aRecords:json::array![object!{_idRow:1,_sDomain:"filecache42",_sState:"up"},object!{_idRow:2,_sDomain:"filecache25",_sState:"terminated"}]}).unwrap();
        assert!(hosts.contains("files.gamebanana.com"));
        assert!(hosts.contains("filecache42.gamebanana.com"));
        assert!(!hosts.contains("filecache25.gamebanana.com"));
        assert!(denied.contains("filecache25.gamebanana.com"));
        for url in ["http://files.gamebanana.com/mods/a.zip", "https://evil.example/a.zip", "https://files.gamebanana.com.evil.example/a.zip"] {
            assert!(!parse_https_url(url).is_ok_and(|h|hosts.contains(&h)));
        }
    }

    #[test]
    fn captured_complete_fileservers_omits_active_cache_allows_only_canonical_redirect() {
        let key=SubmissionKeyV1{submission_type:"mod".into(),submission_id:"663317".into()};
        let session=SessionId::parse(&"a".repeat(32)).unwrap();
        let other_session=SessionId::parse(&"b".repeat(32)).unwrap();
        let now=Instant::now();
        let profile=object!{_idRow:663317,_sProfileUrl:key.profile_url(),_sName:"Girlfailure Silver [Reworked]",_bIsPrivate:false,_bIsTrashed:false,_bIsWithheld:false,_aGame:object!{_idRow:20948}};
        let downloads=object!{_idRow:663317,_bIsTrashed:false,_bIsWithheld:false,_aFiles:json::array![object!{
            _idRow:1886611,_sFile:"girlfailure_silver_v2.rar",_nFilesize:57893932,
            _sMd5Checksum:"391274a4784a63c8cf6bb500da8cd2d6",
            _sDownloadUrl:"https://gamebanana.com/dl/1886611",_bIsArchived:false
        }]};
        let servers=captured_fileservers_json();
        let responses=[profile,downloads,servers].into_iter().zip([MetadataRoute::Profile,MetadataRoute::Files,MetadataRoute::Servers]).map(|(body,route)|{
            let url=route_url(&key,route);
            RawResponse{route,request_url:url.clone(),final_url:url,status:200,body:body.dump().into_bytes(),requested_at:now,session:session.clone()}
        }).collect::<Vec<_>>().try_into().ok().unwrap();
        let auth=FreshFileAuthorityV1::evaluate(&AuthorityRequest{session:session.clone(),submission:key,selected:None},&responses,now).unwrap();

        let allowed=auth.allowed_hosts(&session,now).unwrap();
        let denied_set=auth.denied_hosts(&session,now).unwrap();
        assert!(!allowed.contains("filecache28.gamebanana.com"));
        assert!(!allowed.contains("filecache25.gamebanana.com"));
        assert!(denied_set.contains("filecache25.gamebanana.com"));
        assert!(denied_set.contains("filecache32.gamebanana.com"));
        assert!(denied_set.contains("filecache33.gamebanana.com"));

        // Direct unlisted URLs are never in allowed_hosts.
        assert!(auth.allows_url(&session,"https://gamebanana.com/dl/1886611",now));
        assert!(auth.allows_url(&session,"https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",now));
        assert!(auth.allows_url(&session,"https://filecache40.gamebanana.com/mods/girlfailure_silver_v2.rar",now));
        assert!(!auth.allows_url(&session,"https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",now));

        // Canonical redirect chain succeeds while keeping filecache28 unlisted and scoped.
        assert!(auth.allows_redirect(&session,"https://gamebanana.com/dl/1886611","https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",now));
        assert!(auth.allows_redirect(&session,"https://files.gamebanana.com/mods/girlfailure_silver_v2.rar","https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",now));
        assert!(auth.allows_redirect(&session,"https://gamebanana.com/dl/1886611","https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",now));
        assert!(auth.allows_redirect(&session,"https://files.gamebanana.com/mods/girlfailure_silver_v2.rar","https://filecache40.gamebanana.com/mods/girlfailure_silver_v2.rar",now));

        // Explicit non-up (terminated) hosts fail closed even though they match filecache<N>.
        assert!(!auth.allows_redirect(&session,"https://files.gamebanana.com/mods/a.rar","https://filecache25.gamebanana.com/mods/a.rar",now));
        assert!(!auth.allows_redirect(&session,"https://files.gamebanana.com/mods/a.rar","https://filecache32.gamebanana.com/mods/a.rar",now));
        assert!(!auth.allows_redirect(&session,"https://files.gamebanana.com/mods/a.rar","https://filecache33.gamebanana.com/mods/a.rar",now));

        // Non-canonical origins, chained hops out of an unlisted cache, and invalid hosts fail closed.
        assert!(!auth.allows_redirect(&session,"https://api.gamebanana.com/dl/1","https://filecache28.gamebanana.com/mods/a.rar",now));
        assert!(!auth.allows_redirect(&session,"https://filecache31.gamebanana.com/mods/a.rar","https://filecache28.gamebanana.com/mods/a.rar",now));
        assert!(!auth.allows_redirect(&session,"https://filecache28.gamebanana.com/mods/a.rar","https://filecache29.gamebanana.com/mods/a.rar",now));
        assert!(!auth.allows_redirect(&session,"https://filecache28.gamebanana.com/mods/a.rar","https://files.gamebanana.com/mods/a.rar",now));
        for bad in [
            "https://cdn.gamebanana.com/mods/a.rar",
            "https://evil.gamebanana.com/mods/a.rar",
            "https://filecache.gamebanana.com/mods/a.rar",
            "https://filecache28a.gamebanana.com/mods/a.rar",
            "https://filecache-28.gamebanana.com/mods/a.rar",
            "https://sub.filecache28.gamebanana.com/mods/a.rar",
            "https://filecache28.gamebanana.com.evil.example/mods/a.rar",
            "http://filecache28.gamebanana.com/mods/a.rar",
            "https://filecache28.gamebanana.com:8443/mods/a.rar",
            "https://user@filecache28.gamebanana.com/mods/a.rar",
            "https://127.0.0.1/mods/a.rar",
        ] {
            assert!(!auth.allows_redirect(&session,"https://files.gamebanana.com/mods/a.rar",bad,now),"must reject {bad}");
        }

        // Exact-target header admission requires matching canonical_target and valid session/expiry.
        let canonical="https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar";
        assert!(auth.allows_header_url(&session,canonical,Some(canonical),now));
        assert!(!auth.allows_header_url(&session,canonical,None,now));
        assert!(!auth.allows_header_url(&session,"https://filecache28.gamebanana.com/mods/other.rar",Some(canonical),now));
        assert!(!auth.allows_header_url(&session,"https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar?x=1",Some(canonical),now));
        assert!(!auth.allows_redirect(&other_session,"https://files.gamebanana.com/mods/a.rar",canonical,now));
        assert!(!auth.allows_redirect(&session,"https://files.gamebanana.com/mods/a.rar",canonical,now+Duration::from_secs(301)));
    }
}
