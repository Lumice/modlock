//! Exact, versioned remote wire contracts. Sensitive handles and URLs are never public DTOs.
use crate::{catalog::contract as common, native::NativeError, paths, protocol, store};
use json::{array, object, JsonValue};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, os::fd::OwnedFd, path::PathBuf};

pub const REQUEST_LIMIT: usize = 64 * 1024;
pub const STATUS_LIMIT: usize = 128 * 1024;
pub const PAGE_LIMIT: usize = 64 * 1024;
pub const PRIVATE_LIMIT: usize = 16 * 1024;
pub const BODY_LIMIT: usize = 5 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 64;
pub const MAX_PROVIDER_ROWS: usize = 128;
pub const MAX_CANDIDATES: u64 = 1_024;
pub const MAX_ARCHIVE_ENTRIES: u64 = 10_000;
pub const MAX_MEMBER_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_COHORT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024 * 1024;

pub const METHODS: [(&str, u16); 23] = [
    ("native.remote.open",153),("native.remote.resolve",154),
    ("native.remote.resolveStatus",155),("native.remote.resolveRelease",156),
    ("native.remote.start",157),("native.remote.list",158),("native.remote.status",159),
    ("native.remote.control",160),("native.remote.prepare",161),
    ("native.remote.candidates",162),("native.remote.candidateMembers",163),
    ("native.remote.diagnostics",164),("native.remote.review",165),
    ("native.remote.commit",166),("native.remote.close",167),
    ("native.remote.prepareRelease",168),
    ("native.remote.replaceTarget",170),("native.remote.replaceSkip",171),
    ("native.remote.replaceStart",172),("native.remote.replaceList",173),
    ("native.remote.replaceStatus",174),("native.remote.replaceReview",175),
    ("native.remote.replaceCommit",176),
];
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum RemoteMethod {Open,Resolve,ResolveStatus,ResolveRelease,Start,List,Status,Control,Prepare,Candidates,CandidateMembers,Diagnostics,Review,Commit,Close,PrepareRelease,ReplaceTarget,ReplaceSkip,ReplaceStart,ReplaceList,ReplaceStatus,ReplaceReview,ReplaceCommit}
impl RemoteMethod {
    pub(crate) fn parse(name:&str)->Result<Self,NativeError>{
        Ok(match name{"native.remote.open"=>Self::Open,"native.remote.resolve"=>Self::Resolve,"native.remote.resolveStatus"=>Self::ResolveStatus,"native.remote.resolveRelease"=>Self::ResolveRelease,
            "native.remote.start"=>Self::Start,"native.remote.list"=>Self::List,"native.remote.status"=>Self::Status,"native.remote.control"=>Self::Control,"native.remote.prepare"=>Self::Prepare,
            "native.remote.candidates"=>Self::Candidates,"native.remote.candidateMembers"=>Self::CandidateMembers,"native.remote.diagnostics"=>Self::Diagnostics,"native.remote.review"=>Self::Review,
            "native.remote.commit"=>Self::Commit,"native.remote.close"=>Self::Close,"native.remote.prepareRelease"=>Self::PrepareRelease,
            "native.remote.replaceTarget"=>Self::ReplaceTarget,"native.remote.replaceSkip"=>Self::ReplaceSkip,"native.remote.replaceStart"=>Self::ReplaceStart,
            "native.remote.replaceList"=>Self::ReplaceList,"native.remote.replaceStatus"=>Self::ReplaceStatus,"native.remote.replaceReview"=>Self::ReplaceReview,
            "native.remote.replaceCommit"=>Self::ReplaceCommit,_=>return Err(invalid())})
    }
    pub(crate) fn name(self)->&'static str{METHODS[self as usize].0}
    pub(crate) fn call(self)->u16{METHODS[self as usize].1}
}
/// Validated requests/results cannot be constructed with extra public fields.
pub(crate) struct RemoteRequestV1 {method:RemoteMethod,params:JsonValue,root:PathBuf}
impl RemoteRequestV1 {
    pub(crate) fn parse(method:RemoteMethod,params:JsonValue)->Result<Self,NativeError>{let root=validate_request(method.name(),&params)?;Ok(Self{method,params,root})}
    pub(crate) fn method(&self)->RemoteMethod{self.method}
    pub(crate) fn params(&self)->&JsonValue{&self.params}
    pub(crate) fn root(&self)->&std::path::Path{&self.root}
}
pub(crate) struct RemoteResultV1 {method:RemoteMethod,value:JsonValue}
impl RemoteResultV1 {
    pub(crate) fn parse(method:RemoteMethod,value:JsonValue)->Result<Self,NativeError>{validate_result(method.name(),&value)?;Ok(Self{method,value})}
    pub(crate) fn json(&self)->JsonValue{self.value.clone()}
    pub(crate) fn method(&self)->RemoteMethod{self.method}
}
pub(crate) fn decode_request_envelope(bytes:&[u8])->Result<(JsonValue,RemoteRequestV1),NativeError>{
    if bytes.len()>REQUEST_LIMIT||!bytes.ends_with(b"\n")||!protocol::unique_keys(bytes)||!protocol::integer_numbers(bytes){return Err(invalid());}
    let v=json::parse(std::str::from_utf8(bytes).map_err(|_|invalid())?).map_err(|_|invalid())?;
    common::exact(&v,&["jsonrpc","id","method","params"])?;
    if v["jsonrpc"]!="2.0"||!protocol::signed_integer_id(&v["id"])||v["id"].dump().len()>20{return Err(invalid());}
    let method=RemoteMethod::parse(v["method"].as_str().ok_or_else(invalid)?)?;
    Ok((v["id"].clone(),RemoteRequestV1::parse(method,v["params"].clone())?))
}
pub(crate) fn encode_result_envelope(id:&JsonValue,result:&RemoteResultV1)->Result<Vec<u8>,NativeError>{
    if !protocol::signed_integer_id(id)||id.dump().len()>20{return Err(invalid());}
    let envelope=object!{jsonrpc:"2.0",id:id.clone(),result:result.json()};
    if !result_envelope_ok(result.method.name(),&envelope){return Err(invalid());}
    let mut bytes=envelope.dump().into_bytes();bytes.push(b'\n');Ok(bytes)
}

macro_rules! identity {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub(crate) struct $name(String);
        impl $name {
            pub(crate) fn parse(value:&str)->Result<Self,NativeError>{
                common::token(&JsonValue::from(value))?;Ok(Self(value.to_owned()))
            }
            pub(crate) fn generate()->Result<Self,NativeError>{Self::parse(&common::new_token()?)}
            pub(crate) fn as_str(&self)->&str{&self.0}
        }
    )+};
}
identity!(JobId,RequestKey,PrepareKey,SessionId,SelectionToken,ReviewToken,ClaimId,TransferEpoch);

fn invalid()->NativeError{NativeError::params()}
fn text<'a>(v:&'a JsonValue,max:usize)->Result<&'a str,NativeError>{
    v.as_str().filter(|s|s.len()<=max&&!s.chars().any(char::is_control)).ok_or_else(invalid)
}
fn nonempty<'a>(v:&'a JsonValue,max:usize)->Result<&'a str,NativeError>{
    text(v,max).and_then(|s|if s.is_empty(){Err(invalid())}else{Ok(s)})
}
fn decimal(v:&JsonValue)->Result<u64,NativeError>{common::u64_text(v)}
fn positive(v:&JsonValue)->Result<u64,NativeError>{let n=decimal(v)?;if n==0{Err(invalid())}else{Ok(n)}}
fn bounded_decimal(v:&JsonValue,max:u64)->Result<u64,NativeError>{let n=decimal(v)?;if n>max{Err(invalid())}else{Ok(n)}}
fn optional_bounded_decimal(v:&JsonValue,max:u64)->Result<Option<u64>,NativeError>{if v.is_null(){Ok(None)}else{bounded_decimal(v,max).map(Some)}}
fn token(v:&JsonValue)->Result<(),NativeError>{common::token(v).map(|_|())}
fn hash(v:&JsonValue,n:usize)->Result<(),NativeError>{
    if v.as_str().is_some_and(|s|s.len()==n&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))){Ok(())}else{Err(invalid())}
}
fn optional(v:&JsonValue,check:impl FnOnce(&JsonValue)->Result<(),NativeError>)->Result<(),NativeError>{if v.is_null(){Ok(())}else{check(v)}}
fn bool_value(v:&JsonValue)->Result<(),NativeError>{v.as_bool().map(|_|()).ok_or_else(invalid)}
fn page(v:&JsonValue)->Result<(),NativeError>{v.as_u32().map(|_|()).ok_or_else(invalid)}
fn safe_id(v:&JsonValue)->Result<(),NativeError>{if v.as_str().is_some_and(paths::safe_id){Ok(())}else{Err(invalid())}}
fn safe_name(v:&JsonValue)->Result<(),NativeError>{if v.as_str().is_some_and(paths::safe_name){Ok(())}else{Err(invalid())}}
fn public_message(v:&JsonValue,max:usize)->Result<(),NativeError>{let s=text(v,max)?;if s.contains('/')||s.contains('\\'){Err(invalid())}else{Ok(())}}
fn one_of(v:&JsonValue,allowed:&[&str])->Result<(),NativeError>{if v.as_str().is_some_and(|s|allowed.contains(&s)){Ok(())}else{Err(invalid())}}
fn nullable_decimal(v:&JsonValue)->Result<(),NativeError>{optional(v,|x|decimal(x).map(|_|()))}
fn nullable_token(v:&JsonValue)->Result<(),NativeError>{optional(v,token)}
fn nullable_hash(v:&JsonValue)->Result<(),NativeError>{optional(v,|x|hash(x,64))}
fn opt_string(v:&Option<String>)->JsonValue{v.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null)}
fn opt_bool(v:Option<bool>)->JsonValue{v.map(JsonValue::from).unwrap_or(JsonValue::Null)}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SubmissionKeyV1 { pub submission_type:String, pub submission_id:String }
impl SubmissionKeyV1 {
    pub(crate) fn parse(v:&JsonValue)->Result<Self,NativeError>{
        common::exact(v,&["provider","submissionType","submissionId"])?;
        if v["provider"]!="gamebanana"{return Err(invalid());}
        one_of(&v["submissionType"],&["mod","sound","wip"])?;positive(&v["submissionId"])?;
        Ok(Self{submission_type:v["submissionType"].as_str().unwrap().into(),submission_id:v["submissionId"].as_str().unwrap().into()})
    }
    pub(crate) fn json(&self)->JsonValue{object!{provider:"gamebanana",submissionType:self.submission_type.clone(),submissionId:self.submission_id.clone()}}
    pub(crate) fn model(&self)->&str{match self.submission_type.as_str(){"mod"=>"Mod","sound"=>"Sound",_=>"Wip"}}
    pub(crate) fn profile_url(&self)->String{format!("https://gamebanana.com/{}/{}",match self.submission_type.as_str(){"mod"=>"mods","sound"=>"sounds",_=>"wips"},self.submission_id)}
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderFileKeyV1 { pub submission:SubmissionKeyV1, pub file_id:String }
impl ProviderFileKeyV1 {
    pub(crate) fn json(&self)->JsonValue{object!{submission:self.submission.json(),fileId:self.file_id.clone()}}
    pub(crate) fn parse(v:&JsonValue)->Result<Self,NativeError>{common::exact(v,&["submission","fileId"])?;positive(&v["fileId"])?;Ok(Self{submission:SubmissionKeyV1::parse(&v["submission"])?,file_id:v["fileId"].as_str().unwrap().into()})}
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ConsentFile {
    pub file_id:String,pub original_name:String,pub declared_size:Option<String>,pub created_at:Option<String>,
    pub md5:Option<String>,pub list:String,pub archive_flag:Option<bool>,
}
impl ConsentFile {
    pub(crate) fn parse(v:&JsonValue)->Result<Self,NativeError>{
        common::exact(v,&["fileId","originalName","declaredSize","createdAt","md5","list","archiveFlag"])?;
        positive(&v["fileId"])?;safe_name(&v["originalName"])?;nullable_decimal(&v["declaredSize"])?;nullable_decimal(&v["createdAt"])?;
        optional(&v["md5"],|x|hash(x,32))?;one_of(&v["list"],&["current","archived"])?;optional(&v["archiveFlag"],bool_value)?;
        Ok(Self{file_id:v["fileId"].as_str().unwrap().into(),original_name:v["originalName"].as_str().unwrap().into(),declared_size:v["declaredSize"].as_str().map(str::to_owned),created_at:v["createdAt"].as_str().map(str::to_owned),md5:v["md5"].as_str().map(str::to_owned),list:v["list"].as_str().unwrap().into(),archive_flag:v["archiveFlag"].as_bool()})
    }
    pub(crate) fn json(&self)->JsonValue{object!{fileId:self.file_id.clone(),originalName:self.original_name.clone(),declaredSize:opt_string(&self.declared_size),createdAt:opt_string(&self.created_at),md5:opt_string(&self.md5),list:self.list.clone(),archiveFlag:opt_bool(self.archive_flag)}}
    pub(crate) fn archived(&self)->bool{self.list=="archived"||self.archive_flag==Some(true)}
}
#[derive(Clone)]
pub(crate) struct MemberReceiptV1 {pub file:ConsentFile,pub measured_size:String,pub sha256:String,pub provider_checksum:String}
impl MemberReceiptV1 {
    pub(crate) fn parse(v:&JsonValue)->Result<Self,NativeError>{
        common::exact(v,&["fileId","originalName","declaredSize","createdAt","md5","list","archiveFlag","measuredSize","sha256","providerChecksum"])?;
        let file=ConsentFile::parse(&object!{fileId:v["fileId"].clone(),originalName:v["originalName"].clone(),declaredSize:v["declaredSize"].clone(),createdAt:v["createdAt"].clone(),md5:v["md5"].clone(),list:v["list"].clone(),archiveFlag:v["archiveFlag"].clone()})?;
        let size=positive(&v["measuredSize"])?;if size>MAX_MEMBER_BYTES{return Err(invalid());}hash(&v["sha256"],64)?;
        one_of(&v["providerChecksum"],&["matched","absent"])?;
        if (file.md5.is_some())!=(v["providerChecksum"]=="matched"){return Err(invalid());}
        if file.declared_size.as_deref().and_then(|n|n.parse::<u64>().ok()).is_some_and(|n|n>0&&n!=size){return Err(invalid());}
        Ok(Self{file,measured_size:v["measuredSize"].as_str().unwrap().into(),sha256:v["sha256"].as_str().unwrap().into(),provider_checksum:v["providerChecksum"].as_str().unwrap().into()})
    }
    pub(crate) fn json(&self)->JsonValue{let mut v=self.file.json();v["measuredSize"]=self.measured_size.clone().into();v["sha256"]=self.sha256.clone().into();v["providerChecksum"]=self.provider_checksum.clone().into();v}
}

#[derive(Clone)]
pub(crate) struct ClaimIdentity {pub root_device:u64,pub root_inode:u64,pub directory_device:u64,pub directory_inode:u64,pub data_device:u64,pub data_inode:u64,pub lease_device:u64,pub lease_inode:u64,pub uid:u32,pub link_count:u64}
pub(crate) struct ClaimSpawnFds {pub write_fd:OwnedFd,pub read_fd:OwnedFd,pub shared_lease:OwnedFd,pub identity:ClaimIdentity}
pub(crate) struct SealedRemoteMember {pub file_key:ProviderFileKeyV1,pub receipt:MemberReceiptV1,pub read_fd:OwnedFd,pub claim_lease:OwnedFd,pub claim_identity:ClaimIdentity,
    pub claim_id:ClaimId,pub claim_directory:OwnedFd}
pub(crate) struct SealedRemoteSourceV1 {pub job_id:JobId,pub submission:SubmissionKeyV1,pub root_file_id:String,pub consent_fingerprint:String,pub cohort_fingerprint:String,pub members:Vec<SealedRemoteMember>}

/// The durable operation identity excludes all transient session authority.
#[derive(Clone)]
pub(crate) struct RemoteCommitInputV1 { value:JsonValue }
impl RemoteCommitInputV1 {
    pub(crate) fn from_params(p:&JsonValue)->Result<Self,NativeError>{
        Self::parse(object!{method:"native.remote.commit",params:object!{operationId:p["operationId"].clone(),jobId:p["jobId"].clone(),profileId:p["profileId"].clone(),expectedRevision:p["expectedRevision"].clone(),consentFingerprint:p["consentFingerprint"].clone(),cohortFingerprint:p["cohortFingerprint"].clone(),selectionDigest:p["selectionDigest"].clone()}})
    }
    pub(crate) fn parse(value:JsonValue)->Result<Self,NativeError>{
        common::exact(&value,&["method","params"])?;
        if value["method"]!="native.remote.commit"{return Err(invalid());}
        let p=&value["params"];
        common::exact(p,&["operationId","jobId","profileId","expectedRevision","consentFingerprint","cohortFingerprint","selectionDigest"])?;
        safe_id(&p["operationId"])?;token(&p["jobId"])?;safe_id(&p["profileId"])?;decimal(&p["expectedRevision"])?;
        for key in ["consentFingerprint","cohortFingerprint","selectionDigest"]{hash(&p[key],64)?;}
        Ok(Self{value})
    }
    pub(crate) fn json(&self)->&JsonValue{&self.value}
    pub(crate) fn params(&self)->&JsonValue{&self.value["params"]}
    pub(crate) fn operation_id(&self)->&str{self.params()["operationId"].as_str().unwrap()}
}

/// Disjoint durable authorities share lifecycle machinery, never their decoder.
#[derive(Clone)]
pub(crate) enum RemoteCommitInput {
    Add(RemoteCommitInputV1),
    Replace(super::replacement_contract::ReplacementCommitInputV1),
}
impl RemoteCommitInput {
    pub(crate) fn parse(value:JsonValue)->Result<Self,NativeError>{
        match value["method"].as_str(){
            Some("native.remote.commit")=>RemoteCommitInputV1::parse(value).map(Self::Add),
            Some("native.remote.replaceCommit")=>super::replacement_contract::ReplacementCommitInputV1::parse(value).map(Self::Replace),
            _=>Err(invalid()),
        }
    }
    pub(crate) fn json(&self)->&JsonValue{match self{Self::Add(v)=>v.json(),Self::Replace(v)=>v.json()}}
    pub(crate) fn params(&self)->&JsonValue{match self{Self::Add(v)=>v.params(),Self::Replace(v)=>v.params()}}
    pub(crate) fn operation_id(&self)->&str{match self{Self::Add(v)=>v.operation_id(),Self::Replace(v)=>v.operation_id()}}
}

#[derive(Clone)]
pub(crate) struct RemoteRecoveryInputV1 { value:JsonValue }
impl RemoteRecoveryInputV1 {
    pub(crate) fn parse(value:JsonValue)->Result<Self,NativeError>{
        common::exact(&value,&["method","params"])?;
        if value["method"]!="native.recover"{return Err(invalid());}
        let p=&value["params"];common::exact(p,&["operationId","expectedRevision"])?;
        safe_id(&p["operationId"])?;decimal(&p["expectedRevision"])?;
        Ok(Self{value})
    }
    pub(crate) fn json(&self)->&JsonValue{&self.value}
    pub(crate) fn operation_id(&self)->&str{self.value["params"]["operationId"].as_str().unwrap()}
    pub(crate) fn expected_revision(&self)->u64{decimal(&self.value["params"]["expectedRevision"]).unwrap()}
}

pub(crate) fn digest(value:&JsonValue)->String{format!("{:x}",Sha256::digest(store::canonical(value).as_bytes()))}
pub(crate) fn candidate_fingerprint(submission:&SubmissionKeyV1,name:&str,file:&ConsentFile)->String{digest(&array!["remoteConsent/v1",submission.json(),name,file.json()])}
pub(crate) fn cohort_fingerprint(submission:&SubmissionKeyV1,root:&str,files:&[ConsentFile])->Result<String,NativeError>{
    positive(&JsonValue::from(root))?;
    if files.is_empty()||files.len()>MAX_MEMBERS||files[0].file_id!=root{return Err(invalid());}
    let mut previous=0u64;let mut ids=BTreeSet::new();
    for (i,f) in files.iter().enumerate(){ConsentFile::parse(&f.json())?;let n=positive(&JsonValue::from(f.file_id.as_str()))?;if !ids.insert(n)||(i>1&&n<=previous){return Err(invalid());}if i>0{previous=n;}}
    Ok(digest(&array!["remoteCohort/v1",submission.json(),root,JsonValue::from(files.iter().map(ConsentFile::json).collect::<Vec<_>>())]))
}
pub(crate) fn selection_digest(selected_option_ids:&[String],metadata:&JsonValue)->Result<String,NativeError>{
    metadata_fields(metadata)?;if selected_option_ids.is_empty()||selected_option_ids.len()>99{return Err(invalid());}
    let mut previous:Option<&str>=None;for id in selected_option_ids{token(&JsonValue::from(id.as_str()))?;if previous.is_some_and(|p|p>=id.as_str()){return Err(invalid());}previous=Some(id);}
    Ok(digest(&array!["remoteSelection/v1",JsonValue::from(selected_option_ids.to_vec()),metadata.clone()]))
}
pub(crate) struct CandidateDigestMember {pub role:String,pub index:Option<String>,pub normalized_name:String,pub sha256:String,pub size:String}
pub(crate) fn prepared_candidate_id(job:&JobId,prepare:&PrepareKey,namespace:&str,members:&[CandidateDigestMember])->Result<String,NativeError>{
    if namespace.len()>4096||namespace.starts_with('/')||namespace.contains('\\')||namespace.split('/').any(|p|p=="."||p=="..")||members.is_empty()||members.len()>MAX_ARCHIVE_ENTRIES as usize{return Err(invalid());}
    let mut out=Vec::new();let mut previous=None;
    for (i,m) in members.iter().enumerate(){safe_name(&JsonValue::from(m.normalized_name.as_str()))?;hash(&JsonValue::from(m.sha256.as_str()),64)?;if positive(&JsonValue::from(m.size.as_str()))?>MAX_MEMBER_BYTES{return Err(invalid());}
        if i==0{if m.role!="directory"||m.index.is_some(){return Err(invalid());}}else{if m.role!="companion"{return Err(invalid());}let index=decimal(&JsonValue::from(m.index.as_deref().ok_or_else(invalid)?))?;if previous.is_some_and(|p|index<=p){return Err(invalid());}previous=Some(index);}
        out.push(object!{role:m.role.clone(),index:opt_string(&m.index),normalizedName:m.normalized_name.clone(),sha256:m.sha256.clone(),size:m.size.clone()});
    }
    // json::array! inspects its expressions before collecting them; bind the
    // owned vector first so the size calculation cannot move it twice.
    let members=JsonValue::from(out);
    Ok(digest(&array!["remoteCandidate/v1",job.as_str(),prepare.as_str(),namespace,members]))
}

fn metadata_fields(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["name","author","category","description"])?;nonempty(&v["name"],256)?;text(&v["author"],256)?;text(&v["category"],128)?;
    let d=v["description"].as_str().filter(|s|s.len()<=16384&&!s.chars().any(|c|c.is_control()&&c!='\n'&&c!='\t')).ok_or_else(invalid)?;let _=d;Ok(())
}
pub(crate) fn validate_metadata_fields(v:&JsonValue)->Result<(),NativeError>{metadata_fields(v)}
/// Identify the owner independently of review form validity. The caller has
/// already admitted the complete, bounded, unambiguous JSON-RPC envelope;
/// exact Add/replacement form admission remains the request decoder's job.
pub(crate) fn review_revocation_target(p:&JsonValue)->Result<(PathBuf,SessionId,JobId),NativeError>{
    if !p.is_object()||p["version"]!=1{return Err(invalid());}
    Ok((common::root(&p["dataDir"])? ,SessionId::parse(p["sessionId"].as_str().ok_or_else(invalid)?)?,JobId::parse(p["jobId"].as_str().ok_or_else(invalid)?)?))
}
pub(crate) fn validate_request(method:&str,p:&JsonValue)->Result<PathBuf,NativeError>{
    if method.starts_with("native.remote.replace"){return super::replacement_contract::validate_request(method,p);}
    let extra:&[&str]=match method{
        "native.remote.open"|"native.remote.list"|"native.remote.close"=>&[],
        "native.remote.resolve"=>&["requestKey","submission"],
        "native.remote.resolveStatus"=>&["requestKey","page"],"native.remote.resolveRelease"=>&["requestKey"],
        "native.remote.start"=>&["jobId","selectionToken","candidateFingerprint","fileId","profileId","expectedRevision","acceptArchived"],
        "native.remote.status"=>&["jobId"],"native.remote.control"=>&["jobId","controlSeq","action"],
        "native.remote.prepare"=>&["jobId","profileId","expectedRevision","acknowledgeRebase","prepareKey"],
        "native.remote.candidates"|"native.remote.diagnostics"=>&["jobId","prepareKey","page"],
        "native.remote.candidateMembers"=>&["jobId","prepareKey","candidateId","page"],
        "native.remote.review"=>&["jobId","profileId","expectedRevision","requestKey","candidateIds","metadata"],
        "native.remote.commit"=>&["jobId","profileId","expectedRevision","reviewToken","operationId","consentFingerprint","cohortFingerprint","selectionDigest"],
        "native.remote.prepareRelease"=>&["jobId","prepareKey"],_=>return Err(NativeError::new("methodNotFound","Unknown remote method.")),
    };
    let mut keys=vec!["version","dataDir","sessionId"];keys.extend_from_slice(extra);common::exact(p,&keys)?;
    if p["version"]!=1||!common::typed(p){return Err(invalid());}let root=common::root(&p["dataDir"])?;token(&p["sessionId"])?;
    for k in ["jobId","requestKey","selectionToken","prepareKey"]{if p.has_key(k){token(&p[k])?;}}
    if p.has_key("reviewToken"){nullable_token(&p["reviewToken"])?;}
    for k in ["candidateFingerprint","candidateId","consentFingerprint","cohortFingerprint","selectionDigest"]{if p.has_key(k){hash(&p[k],64)?;}}
    for k in ["expectedRevision","controlSeq"]{if p.has_key(k){decimal(&p[k])?;}}
    if p.has_key("fileId"){positive(&p["fileId"])?;}if p.has_key("profileId"){safe_id(&p["profileId"])?;}if p.has_key("operationId"){safe_id(&p["operationId"])?;}
    if p.has_key("page"){page(&p["page"])?;}if p.has_key("submission"){SubmissionKeyV1::parse(&p["submission"])?;}
    for k in ["acceptArchived","acknowledgeRebase"]{if p.has_key(k){bool_value(&p[k])?;}}
    if p.has_key("action"){one_of(&p["action"],&["pause","resume","retry","cancel","abandon","dismiss"])?;}
    if p.has_key("metadata"){metadata_fields(&p["metadata"])?;}
    if p.has_key("candidateIds"){
        let ids=&p["candidateIds"];if !ids.is_array()||ids.len()==0||ids.len()>99{return Err(invalid());}let mut previous=None;
        for id in ids.members(){hash(id,64)?;let s=id.as_str().unwrap();if previous.is_some_and(|p|p>=s){return Err(invalid());}previous=Some(s);}
    }
    Ok(root)
}

pub(crate) const ERROR_KINDS:&[&str]=&["eligibilityUnknown","providerDenied","providerNotFound","providerMalformed","providerUnavailable","needsSelection","hostNotAllowed","transportTls","transportNetwork","transportTimeout","transportFraming","transportChecksum","transportStorage","claimAmbiguous","storageFull","layoutUnsupported","archiveUnsupported","archiveRejected","archiveLimit","invalidSelection","staleRevision","staleReview","duplicateSubmission","launchBusy","recoveryRequired","outcomeUnknown","remoteBusy","controlReceiptExpired","jobIdConflict","cancelTooLate"];
pub(crate) struct ErrorV1 {pub kind:String,pub message:String,pub retry_after_seconds:Option<String>}
impl ErrorV1 {
    pub(crate) fn parse(v:&JsonValue)->Result<Self,NativeError>{error_value(v)?;Ok(Self{kind:v["kind"].as_str().unwrap().into(),message:v["message"].as_str().unwrap().into(),retry_after_seconds:v["retryAfterSeconds"].as_str().map(str::to_owned)})}
    pub(crate) fn json(&self)->Result<JsonValue,NativeError>{let v=object!{kind:self.kind.clone(),message:self.message.clone(),retryAfterSeconds:opt_string(&self.retry_after_seconds)};error_value(&v)?;Ok(v)}
}
fn error_value(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["kind","message","retryAfterSeconds"])?;one_of(&v["kind"],ERROR_KINDS)?;
    let message=text(&v["message"],512)?;if message.contains('/')||message.contains('\\')||message.contains("://"){return Err(invalid());}
    if !v["retryAfterSeconds"].is_null()&&decimal(&v["retryAfterSeconds"] )?>86400{return Err(invalid());}Ok(())
}
pub(crate) fn validate_error_value(v:&JsonValue)->Result<(),NativeError>{error_value(v)}
fn optional_error(v:&JsonValue)->Result<(),NativeError>{optional(v,error_value)}
fn row_candidate(v:&JsonValue)->Result<(),NativeError>{
    let mut identity=v.clone();identity.remove("description");
    common::exact(&identity,&["fileKey","submissionName","originalName","declaredSize","createdAt","md5","list","archiveFlag","candidateFingerprint"])?;
    if v.has_key("description"){text(&v["description"],1024)?;}
    ProviderFileKeyV1::parse(&v["fileKey"])?;nonempty(&v["submissionName"],256)?;safe_name(&v["originalName"])?;
    nullable_decimal(&v["declaredSize"])?;nullable_decimal(&v["createdAt"])?;optional(&v["md5"],|x|hash(x,32))?;
    one_of(&v["list"],&["current","archived"])?;optional(&v["archiveFlag"],bool_value)?;hash(&v["candidateFingerprint"],64)
}
fn summary(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["selectedCandidateIds","selectedOptionIds","readyCount","selectedCount","retainedCount","uniquePayloadBytes","retainedLogicalBytes","sourceBytes","diagnosticCount"])?;
    for (key,len) in [("selectedCandidateIds",64),("selectedOptionIds",32)]{
        let a=&v[key];if !a.is_array()||a.len()==0||a.len()>99{return Err(invalid());}let mut last=None;
        for x in a.members(){hash(x,len)?;let s=x.as_str().unwrap();if last.is_some_and(|p|p>=s){return Err(invalid());}last=Some(s);}
    }
    let ready=bounded_decimal(&v["readyCount"],MAX_CANDIDATES)?;
    let selected=bounded_decimal(&v["selectedCount"],99)?;
    let retained=bounded_decimal(&v["retainedCount"],MAX_CANDIDATES)?;
    bounded_decimal(&v["diagnosticCount"],MAX_ARCHIVE_ENTRIES)?;
    if bounded_decimal(&v["uniquePayloadBytes"],MAX_PAYLOAD_BYTES)?==0{return Err(invalid());}
    let retained_logical=bounded_decimal(&v["retainedLogicalBytes"],MAX_PAYLOAD_BYTES)?;
    if bounded_decimal(&v["sourceBytes"],MAX_COHORT_BYTES)?==0{return Err(invalid());}
    if selected!=v["selectedCandidateIds"].len() as u64||selected!=v["selectedOptionIds"].len() as u64||ready.checked_sub(selected)!=Some(retained)||(retained==0)!=(retained_logical==0){return Err(invalid());}Ok(())
}
pub(crate) fn validate_summary(v:&JsonValue)->Result<(),NativeError>{summary(v)}
fn review_value(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["requestKey","phase","reviewToken","selectionDigest","modId","summary","error"])?;
    token(&v["requestKey"])?;one_of(&v["phase"],&["pending","ready","failed"])?;
    nullable_token(&v["reviewToken"])?;nullable_hash(&v["selectionDigest"])?;optional(&v["modId"],safe_id)?;optional(&v["summary"],summary)?;optional_error(&v["error"])?;
    match v["phase"].as_str().unwrap(){
        "ready"=>if v["reviewToken"].is_null()||v["selectionDigest"].is_null()||v["modId"].is_null()||v["summary"].is_null()||!v["error"].is_null(){return Err(invalid());},
        "failed"=>if v["error"].is_null()||!v["reviewToken"].is_null()||!v["selectionDigest"].is_null()||!v["modId"].is_null()||!v["summary"].is_null(){return Err(invalid());},
        _=>if !v["reviewToken"].is_null()||!v["selectionDigest"].is_null()||!v["modId"].is_null()||!v["summary"].is_null()||!v["error"].is_null(){return Err(invalid());}
    }Ok(())
}
fn status_value(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["version","root","sessionId","jobId","submission","profileId","profileNameAtStart","initialRevision","reviewRevision","sequence","state","progress","cohort","prepareKey","review","operationId","lastControl","error","cleanupPending"])?;
    if v["version"]!=1{return Err(invalid());}common::root(&v["root"])?;token(&v["sessionId"])?;token(&v["jobId"])?;SubmissionKeyV1::parse(&v["submission"])?;safe_id(&v["profileId"])?;
    nonempty(&v["profileNameAtStart"],256)?;decimal(&v["initialRevision"])?;nullable_decimal(&v["reviewRevision"])?;decimal(&v["sequence"])?;
    one_of(&v["state"],&["resolving","transferring","discoveringFamily","sealing","downloaded","preparing","ready","reviewing","committing","installed","paused","failed","needsSelection","cancelRequested","cancelled","abandoned"])?;
    if !v["progress"].is_null(){let p=&v["progress"];common::exact(p,&["epoch","volatileSequence","bytesWritten","durableOffset","total"])?;token(&p["epoch"])?;decimal(&p["volatileSequence"])?;
        let written=bounded_decimal(&p["bytesWritten"],MAX_MEMBER_BYTES)?;let durable=bounded_decimal(&p["durableOffset"],MAX_MEMBER_BYTES)?;
        let total=optional_bounded_decimal(&p["total"],MAX_MEMBER_BYTES)?;if durable>written||total.is_some_and(|n|written>n||durable>n){return Err(invalid());}}
    let cohort=&v["cohort"];common::exact(cohort,&["fingerprint","complete","members"])?;nullable_hash(&cohort["fingerprint"])?;bool_value(&cohort["complete"])?;
    if !cohort["members"].is_array()||cohort["members"].len()>MAX_MEMBERS{return Err(invalid());}
    let mut ids=BTreeSet::new();let mut observed=0u64;for m in cohort["members"].members(){common::exact(m,&["fileId","originalName","state","durableOffset","total","receipt"])?;
        let id=positive(&m["fileId"])?;if !ids.insert(id){return Err(invalid());}safe_name(&m["originalName"])?;
        nonempty(&m["state"],32)?;let durable=bounded_decimal(&m["durableOffset"],MAX_MEMBER_BYTES)?;
        let total=optional_bounded_decimal(&m["total"],MAX_MEMBER_BYTES)?;if total.is_some_and(|n|durable>n){return Err(invalid());}
        let bytes=if m["receipt"].is_null(){durable}else{let receipt=MemberReceiptV1::parse(&m["receipt"])?;let measured=positive(&m["receipt"]["measuredSize"])?;
            if receipt.file.file_id!=m["fileId"].as_str().unwrap()||receipt.file.original_name!=m["originalName"].as_str().unwrap()||durable!=measured||total!=Some(measured){return Err(invalid());}measured};
        observed=observed.checked_add(bytes).ok_or_else(invalid)?;if observed>MAX_COHORT_BYTES{return Err(invalid());}
    }
    if cohort["complete"]==true&&cohort["fingerprint"].is_null(){return Err(invalid());}
    nullable_token(&v["prepareKey"])?;optional(&v["review"],review_value)?;optional(&v["operationId"],safe_id)?;optional_error(&v["error"])?;bool_value(&v["cleanupPending"])?;
    if !v["lastControl"].is_null(){let c=&v["lastControl"];common::exact(c,&["controlSeq","action","result"])?;decimal(&c["controlSeq"])?;one_of(&c["action"],&["pause","resume","retry","cancel","abandon","dismiss"])?;one_of(&c["result"],&["accepted","rejected"])?;}Ok(())
}
pub(crate) fn validate_status(v:&JsonValue)->Result<(),NativeError>{status_value(v)}
/// A validated status cannot contain provider URLs, source paths, or handles.
pub(crate) struct RemoteStatusV1(JsonValue);
impl RemoteStatusV1 {
    pub(crate) fn parse(value:JsonValue)->Result<Self,NativeError>{status_value(&value)?;Ok(Self(value))}
    pub(crate) fn json(&self)->JsonValue{self.0.clone()}
}
fn common_result(v:&JsonValue,extra:&[&str])->Result<(),NativeError>{
    let mut keys=vec!["version","root","sessionId"];keys.extend_from_slice(extra);common::exact(v,&keys)?;
    if v["version"]!=1{return Err(invalid());}common::root(&v["root"])?;token(&v["sessionId"])?;Ok(())
}
fn preparation_candidate(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["candidateId","optionId","optionName","sourceLabel","available","unavailableReason","memberCount","logicalBytes"])?;
    hash(&v["candidateId"],64)?;nullable_token(&v["optionId"])?;text(&v["optionName"],1024)?;safe_name(&v["sourceLabel"])?;bool_value(&v["available"])?;
    optional(&v["unavailableReason"],|x|public_message(x,512))?;let members=bounded_decimal(&v["memberCount"],MAX_ARCHIVE_ENTRIES)?;
    let logical=optional_bounded_decimal(&v["logicalBytes"],MAX_PAYLOAD_BYTES)?;
    if v["available"]==true&&(v["optionId"].is_null()||logical.is_none_or(|n|n==0)||members==0||!v["unavailableReason"].is_null()){return Err(invalid());}
    if v["available"]==false&&(!v["optionId"].is_null()||!v["logicalBytes"].is_null()||v["unavailableReason"].is_null()){return Err(invalid());}Ok(())
}
fn member_row(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["name","role","index","measuredSize"])?;safe_name(&v["name"])?;one_of(&v["role"],&["directory","companion"])?;nullable_decimal(&v["index"])?;if positive(&v["measuredSize"])? > MAX_MEMBER_BYTES{return Err(invalid());}
    if (v["role"]=="directory")!=v["index"].is_null(){return Err(invalid());}Ok(())
}
fn diagnostic(v:&JsonValue)->Result<(),NativeError>{
    common::exact(v,&["kind","candidateId","count","message"])?;one_of(&v["kind"],&["excludedNonVpk","nestedArchive","incompleteFamily","invalidFamily","unsupportedType"])?;nullable_hash(&v["candidateId"])?;bounded_decimal(&v["count"],MAX_ARCHIVE_ENTRIES)?;public_message(&v["message"],512)?;Ok(())
}
pub(crate) fn validate_result(method:&str,v:&JsonValue)->Result<(),NativeError>{
    if method.starts_with("native.remote.replace"){return super::replacement_contract::validate_result(method,v);}
    if method=="native.remote.status"{return status_value(v);}
    if method=="native.remote.commit"&&v.has_key("protocolVersion"){
        // Store::replay returns the existing native snapshot, with its existing
        // snapshot contract and response allowance, before any live token lookup.
        let mut keys=vec!["protocolVersion","sourceKind","sourcePath","storeVersion","revision","activeProfileId","selectedProfileId","profiles","mods","warnings","pendingChanges","pendingStatus","pendingReason","gamePath","appliedProfileId","canRestore","recoveryRequired"];
        let version=v["storeVersion"].as_u32().filter(|n|(1..=4).contains(n)).ok_or_else(invalid)?;
        if version>=3{keys.extend_from_slice(&["packageVersion","fonts"]);}
        // Snapshots taken after "Find a broken mod" landed carry its session view; older replays do not.
        for optional in ["bisect","fixAutomatically","launchFingerprint","storageRoot"]{if v.has_key(optional){keys.push(optional);}}
        common::exact(v,&keys)?;
        if v["protocolVersion"]!=1||v["sourceKind"]!="native"||!v["profiles"].is_array()||!v["mods"].is_array()||!v["warnings"].is_array(){return Err(invalid());}
        decimal(&v["revision"])?;bool_value(&v["pendingChanges"])?;bool_value(&v["canRestore"])?;bool_value(&v["recoveryRequired"])?;return Ok(());
    }
    let extra:&[&str]=match method{
        "native.remote.open"|"native.remote.close"=>&["phase","error"],
        "native.remote.resolve"=>&["requestKey","state"],"native.remote.resolveStatus"=>&["requestKey","phase","page","hasMore","selectionToken","candidates","error"],
        "native.remote.resolveRelease"=>&["requestKey","released"],"native.remote.start"=>&["jobId","state"],"native.remote.list"=>&["jobs"],
        "native.remote.control"=>&["jobId","controlSeq","state"],"native.remote.prepare"=>&["jobId","prepareKey","state"],
        "native.remote.candidates"=>&["jobId","prepareKey","page","hasMore","candidates"],"native.remote.candidateMembers"=>&["jobId","prepareKey","candidateId","page","hasMore","members"],
        "native.remote.diagnostics"=>&["jobId","prepareKey","page","hasMore","diagnostics"],"native.remote.review"=>&["jobId","requestKey","state"],
        "native.remote.commit"=>&["jobId","operationId","state"],"native.remote.prepareRelease"=>&["jobId","prepareKey","state"],_=>return Err(invalid()),
    };common_result(v,extra)?;
    for k in ["jobId","prepareKey","requestKey"]{if v.has_key(k){token(&v[k])?;}}
    if v.has_key("candidateId"){hash(&v["candidateId"],64)?;}if v.has_key("operationId"){safe_id(&v["operationId"])?;}
    if v.has_key("controlSeq"){decimal(&v["controlSeq"])?;}if v.has_key("page"){page(&v["page"])?;}if v.has_key("hasMore"){bool_value(&v["hasMore"])?;}
    if v.has_key("released"){bool_value(&v["released"])?;}if v.has_key("error"){optional_error(&v["error"])?;}
    if method=="native.remote.open"{one_of(&v["phase"],&["opening","ready","waiting","failed"])?;}
    if method=="native.remote.close"{one_of(&v["phase"],&["closing","waiting","closed"])?;}
    if method=="native.remote.resolveStatus"{
        one_of(&v["phase"],&["pending","ready","failed"])?;nullable_token(&v["selectionToken"])?;
        if !v["candidates"].is_array()||v["candidates"].len()>8{return Err(invalid());}for x in v["candidates"].members(){row_candidate(x)?;}
        if v["phase"]=="ready"{if v["selectionToken"].is_null()||!v["error"].is_null(){return Err(invalid());}}
        else if !v["selectionToken"].is_null()||v["candidates"].len()!=0||v["hasMore"]!=false{return Err(invalid());}
        if v["phase"]=="failed"&&v["error"].is_null(){return Err(invalid());}
        if v["phase"]=="pending"&&!v["error"].is_null(){return Err(invalid());}
    }
    if method=="native.remote.list"{let a=&v["jobs"];if !a.is_array()||a.len()>8{return Err(invalid());}for x in a.members(){common::exact(x,&["jobId","submission","profileId","state","sequence","cleanupPending"])?;token(&x["jobId"])?;SubmissionKeyV1::parse(&x["submission"])?;safe_id(&x["profileId"])?;text(&x["state"],32)?;decimal(&x["sequence"])?;bool_value(&x["cleanupPending"])?;}}
    for (key,validator) in [("candidates",preparation_candidate as fn(&JsonValue)->Result<(),NativeError>),("members",member_row),("diagnostics",diagnostic)]{
        if v.has_key(key)&&method!="native.remote.resolveStatus"{let a=&v[key];if !a.is_array()||a.len()>8{return Err(invalid());}for x in a.members(){validator(x)?;}}
    }
    if v.has_key("state"){
        let allowed:&[&str]=match method{"native.remote.control"=>&["accepted","rejected"],"native.remote.prepareRelease"=>&["pending","released","stale"],_=>&["pending"]};one_of(&v["state"],allowed)?;
    }Ok(())
}
/// Bounds are applied to the complete JSON-RPC envelope including its LF.
pub(crate) fn result_budget(method:&str)->usize{if matches!(method,"native.remote.status"|"native.remote.replaceStatus"){STATUS_LIMIT}else{PAGE_LIMIT}}
pub(crate) fn validate_result_binding(method:&str,request:&JsonValue,result:&JsonValue)->Result<(),NativeError>{
    if method.starts_with("native.remote.replace"){return super::replacement_contract::validate_binding(method,request,result);}
    if method=="native.remote.commit"&&result.has_key("protocolVersion"){
        if result["selectedProfileId"]!=request["profileId"]{return Err(invalid());}
        let next=decimal(&request["expectedRevision"])?.checked_add(1).ok_or_else(invalid)?;
        if decimal(&result["revision"])?!=next{return Err(invalid());}return Ok(());
    }
    if common::root(&result["root"])?!=common::root(&request["dataDir"])?||result["sessionId"]!=request["sessionId"]{return Err(invalid());}
    for key in ["jobId","requestKey","prepareKey","candidateId","operationId","page","controlSeq"]{
        if request.has_key(key)&&result.has_key(key)&&request[key]!=result[key]{return Err(invalid());}
    }Ok(())
}
pub(crate) fn result_envelope_ok(method:&str,envelope:&JsonValue)->bool{
    let limit=if matches!(method,"native.remote.commit"|"native.remote.replaceCommit")&&envelope["result"].has_key("protocolVersion"){protocol::MAX_RESPONSE_BYTES}else{result_budget(method)};
    protocol::native_value_bytes(envelope).is_some_and(|n|n.checked_add(1).is_some_and(|n|n<=limit))
}

fn small_ascii(v:&JsonValue,max:usize)->Result<&str,NativeError>{v.as_str().filter(|s|s.len()<=max&&s.bytes().all(|b|b==b'\t'||(32..=126).contains(&b))).ok_or_else(invalid)}
fn strong_etag(v:&JsonValue)->Result<(),NativeError>{let s=small_ascii(v,1024)?;if s.len()>=2&&s.starts_with('"')&&s.ends_with('"')&&!s[1..s.len()-1].contains('"'){Ok(())}else{Err(invalid())}}
fn private_url(v:&JsonValue)->Result<(),NativeError>{let s=nonempty(v,4096)?;super::authority::parse_https_url(s).map(|_|()).map_err(|_|invalid())}
fn private_data(kind:&str,v:&JsonValue)->Result<(),NativeError>{
    match kind{
        "headers"=>{common::exact(v,&["url","status","headers","identityEstablished"])?;private_url(&v["url"])?;let s=decimal(&v["status"])?;if !(100..=599).contains(&s){return Err(invalid());}bool_value(&v["identityEstablished"])?;
            let h=&v["headers"];if !h.is_array()||h.len()>64{return Err(invalid());}let mut bytes=0usize;for pair in h.members(){common::exact(pair,&["name","value"])?;let n=small_ascii(&pair["name"],8192)?;let value=small_ascii(&pair["value"],8192)?;if n.is_empty()||!n.bytes().all(|c|c.is_ascii_alphanumeric()||b"!#$%&'*+-.^_`|~".contains(&c)){return Err(invalid());}bytes=bytes.checked_add(n.len()+value.len()).ok_or_else(invalid)?;}if bytes>8192{return Err(invalid());}}
        "checkpoint"=>{common::exact(v,&["offset","prefixSha256","strongEtag","effectiveUriSha256","total"])?;decimal(&v["offset"])?;hash(&v["prefixSha256"],64)?;optional(&v["strongEtag"],strong_etag)?;nullable_hash(&v["effectiveUriSha256"])?;nullable_decimal(&v["total"])?;}
        "progress"=>{common::exact(v,&["bytesWritten","total"])?;decimal(&v["bytesWritten"])?;nullable_decimal(&v["total"])?;}
        "finished"=>{common::exact(v,&["bytesWritten","md5","sha256","networkError"])?;decimal(&v["bytesWritten"])?;hash(&v["md5"],32)?;hash(&v["sha256"],64)?;nonempty(&v["networkError"],128)?;}
        "error"=>{common::exact(v,&["kind"])?;one_of(&v["kind"],ERROR_KINDS)?;}
        "allow"=>{common::exact(v,&["mode","url","rangeStart","ifRange"])?;one_of(&v["mode"],&["redirect","body"])?;private_url(&v["url"])?;nullable_decimal(&v["rangeStart"])?;optional(&v["ifRange"],strong_etag)?;if v["rangeStart"].is_null()!=v["ifRange"].is_null(){return Err(invalid());}}
        "deny"=>{common::exact(v,&["reason"])?;text(&v["reason"],512)?;}
        "checkpointAck"=>{common::exact(v,&["offset","prefixSha256"])?;decimal(&v["offset"])?;hash(&v["prefixSha256"],64)?;}
        "abort"=>{common::exact(v,&["reason"])?;one_of(&v["reason"],&["pause","cancel","close","failure"])?;}
        _=>return Err(invalid()),
    }Ok(())
}
fn private_value(v:&JsonValue)->Result<(),NativeError>{
    if v["version"]!=1{return Err(invalid());}token(&v["epoch"])?;
    if v["kind"]=="start"{
        common::exact(v,&["version","kind","epoch","requestUrl","allowedHosts","deniedHosts","offset","ifRange","uriSha256","byteCap","claimIdentity"])?;
        private_url(&v["requestUrl"])?;let hosts=&v["allowedHosts"];if !hosts.is_array()||hosts.len()>64||hosts.len()==0{return Err(invalid());}let mut seen=BTreeSet::new();
        for h in hosts.members(){let s=nonempty(h,253)?;if !super::authority::valid_allowed_host(s)||!seen.insert(s){return Err(invalid());}}
        let denied=&v["deniedHosts"];if !denied.is_array()||denied.len()>128{return Err(invalid());}let mut denied_seen=BTreeSet::new();
        for h in denied.members(){let s=nonempty(h,253)?;if !super::authority::valid_allowed_host(s)||seen.contains(s)||!denied_seen.insert(s){return Err(invalid());}}
        let host=super::authority::parse_https_url(v["requestUrl"].as_str().unwrap()).map_err(|_|invalid())?;if denied_seen.contains(host.as_str())||!seen.contains(host.as_str()){return Err(invalid());}
        let offset=decimal(&v["offset"])?;optional(&v["ifRange"],strong_etag)?;nullable_hash(&v["uriSha256"])?;
        if (offset==0)!=v["ifRange"].is_null()||(offset==0)!=v["uriSha256"].is_null(){return Err(invalid());}
        let cap=positive(&v["byteCap"])?;if cap>MAX_MEMBER_BYTES||offset>cap{return Err(invalid());}
        let id=&v["claimIdentity"];common::exact(id,&["device","inode","uid"])?;for k in ["device","inode","uid"]{decimal(&id[k])?;}
    }else{common::exact(v,&["version","kind","epoch","data"])?;private_data(v["kind"].as_str().ok_or_else(invalid)?,&v["data"])?;}Ok(())
}
pub(crate) fn decode_private_frame(frame:&[u8])->Result<JsonValue,NativeError>{
    if frame.len()>PRIVATE_LIMIT||!frame.ends_with(b"\n")||!protocol::unique_keys(frame)||!protocol::integer_numbers(frame){return Err(invalid());}
    let v=json::parse(std::str::from_utf8(frame).map_err(|_|invalid())?).map_err(|_|invalid())?;private_value(&v)?;Ok(v)
}
pub(crate) fn encode_private_frame(v:&JsonValue)->Result<Vec<u8>,NativeError>{
    private_value(v)?;let mut bytes=v.dump().into_bytes();bytes.push(b'\n');if bytes.len()>PRIVATE_LIMIT{return Err(invalid());}Ok(bytes)
}
