//! Typed, transient preparation. Only owner-created sealed handles grant access.
use super::{contract::{self, CandidateDigestMember, PrepareKey, SessionId, SealedRemoteSourceV1}, owner, preparation_slot::PreparationSlotLease, quarantine::RemoteQuarantineAuthority, replacement_contract::{self,ReplacementCommitInputV1,ReplacementPurpose}, replacement_target};
use crate::{archive_content::{self, ArchiveBudget, ArchiveInput, ArchiveKind, PreparedArchive}, archive_quarantine::QuarantineLease, local_content, native::NativeError, options::{ExpectedFamily,OptionCatalog,OptionEntry}, package::{self,FileRef,PackageRef,SourcePackage}, paths::{self,Fingerprint}, prepare_control::PrepareControl, store::{self,RootLock,Store}};
use json::{object,JsonValue};
use std::{collections::{BTreeMap,BTreeSet,HashMap}, fs::{self,File},  os::unix::fs::MetadataExt, path::{Path,PathBuf}, sync::Arc, time::Instant};

fn invalid() -> NativeError { NativeError::new("invalidSelection","The selected preparation is unavailable or changed.") }
fn limit() -> NativeError { NativeError::new("archiveLimit","The complete set of alternatives cannot be represented within its storage limits.") }
fn stale() -> NativeError { NativeError::new("staleReview","The preparation changed and must be reviewed again.") }
fn public_error(e:NativeError)->NativeError{
    if contract::ERROR_KINDS.contains(&e.kind.as_str()){return e;}
    let kind=match e.kind.as_str(){
        "archiveLimitExceeded"|"localSourceTooLarge"|"sourceTooLarge"|"nativeResponseLimit"|"invalidOptions"=>"archiveLimit",
        "archiveContractUnsupported"|"archiveDecodeFailed"=>"archiveUnsupported",
        "archiveRejected"=>"archiveRejected",
        "invalidPackage"=>"layoutUnsupported",
        "stalePreview"|"staleLocalSource"=>"staleReview",
        "localPreparationCancelled"|"nativeBusy"=>"remoteBusy",
        _=>"transportStorage",
    };
    NativeError::new(kind,match kind{"archiveLimit"=>"The complete set exceeds a preparation limit.","archiveUnsupported"=>"The archive format is unavailable.","archiveRejected"=>"The archive failed admission.","layoutUnsupported"=>"The VPK layout is unsupported.","staleReview"=>"The preparation changed and must be reviewed again.","remoteBusy"=>"The preparation is retiring.",_=>"Remote preparation could not be completed."})
}

#[derive(Clone)]
pub(crate) struct PrepareContext {
    pub root:PathBuf,pub session:SessionId,pub prepare_key:PrepareKey,pub profile_id:String,
    pub expected_revision:u64,pub expires_at:Instant,pub status_template:JsonValue,pub replacement:Option<ReplacementPin>,
}
#[derive(Clone)]
pub(crate) struct ReplacementPin {pub purpose:ReplacementPurpose,pub mod_id:String,pub target_digest:String,pub initial_revision:String}
#[derive(Clone,PartialEq,Eq,PartialOrd,Ord)]
pub(crate) struct CandidateId(String);
impl CandidateId {
    pub(crate) fn parse(value:&str)->Result<Self,NativeError>{if package::hex(value,64){Ok(Self(value.into()))}else{Err(invalid())}}
    pub(crate) fn as_str(&self)->&str{&self.0}
}
#[derive(Clone)]
struct Candidate {
    id:CandidateId,option_id:Option<String>,option_name:String,source_label:String,
    source:Option<SourcePackage>,package:Option<PackageRef>,members:Vec<JsonValue>,logical:u64,reason:Option<String>,
}
pub(crate) struct CandidatePageV1 {pub page:u32,pub has_more:bool,pub candidates:Vec<JsonValue>}
pub(crate) struct MemberPageV1 {pub page:u32,pub has_more:bool,pub members:Vec<JsonValue>}
pub(crate) struct DiagnosticPageV1 {pub page:u32,pub has_more:bool,pub diagnostics:Vec<JsonValue>}
pub(crate) struct RemoteReviewV1 {pub request_key:String,pub candidate_ids:Vec<String>,pub metadata:JsonValue,pub expires_at:Instant,pub control:PrepareControl}

pub(crate) struct PreparedRemote {
    source:SealedRemoteSourceV1,slot:PreparationSlotLease,authority:RemoteQuarantineAuthority,
    quarantine:Arc<QuarantineLease>,archives:Vec<Arc<PreparedArchive>>,fingerprints:BTreeMap<PathBuf,Fingerprint>,
    context:PrepareContext,candidates:Vec<Candidate>,diagnostics:Vec<JsonValue>,source_bytes:u64,unique_payload_bytes:u64,verification:Vec<FileStamp>,
}
#[derive(Clone,PartialEq,Eq)]
struct FileStamp {fsid:String,device:u64,inode:u64,size:u64,mtime:i64,mtime_ns:i64,ctime:i64,ctime_ns:i64,mode:u32,uid:u32,links:u64}
impl FileStamp {fn read(file:&File)->Result<Self,NativeError>{let m=file.metadata()?;Ok(Self{fsid:paths::persisted_filesystem_id(file)?,device:m.dev(),inode:m.ino(),size:m.len(),mtime:m.mtime(),mtime_ns:m.mtime_nsec(),ctime:m.ctime(),ctime_ns:m.ctime_nsec(),mode:m.mode(),uid:m.uid(),links:m.nlink()})}}
pub(crate) struct RemotePublicationPlan {
    pub prepared:Arc<PreparedRemote>,pub token:String,pub mod_id:String,pub selection_digest:String,
    pub profile_id:String,pub expected_revision:u64,pub record:JsonValue,pub projected_document:JsonValue,
    pub summary:JsonValue,pub copies:BTreeMap<String,(FileRef,PathBuf,Fingerprint)>,pub expires_at:Instant,pub purpose:RemotePlanPurpose,
}
pub(crate) enum RemotePlanPurpose {Add,Replace{pin:ReplacementPin,after_record_digest:String,candidate_ids:Vec<String>,option_ids:Vec<String>}}
pub(crate) struct RemotePreparation;

fn fingerprint(file:&File,sha256:&str)->Result<Fingerprint,NativeError>{
    let m=file.metadata()?;if !m.is_file()||m.len()>contract::MAX_MEMBER_BYTES{return Err(stale());}
    Ok(Fingerprint{sha256:sha256.into(),size:m.len(),modified_seconds:m.mtime(),modified_nanos:m.mtime_nsec(),device:m.dev(),inode:m.ino()})
}
fn source_file(member:&contract::SealedRemoteMember)->Result<File,NativeError>{Ok(File::from(member.read_fd.try_clone()?))}
fn size_add(a:u64,b:u64,maximum:u64)->Result<u64,NativeError>{a.checked_add(b).filter(|n|*n<=maximum).ok_or_else(limit)}
fn short_label(value:&str,id:&str,duplicate:bool)->Result<String,NativeError>{
    if !paths::safe_name(value){return Err(limit());}
    if !duplicate{return Ok(value.into());}
    let suffix=format!(" [{id}]");let mut name=value.to_owned();
    while name.len()+suffix.len()>255{name.pop().ok_or_else(limit)?;}
    name.push_str(&suffix);if !paths::safe_name(&name){return Err(limit());}Ok(name)
}
fn diagnostic(kind:&str,id:Option<&str>,message:&str)->JsonValue{object!{kind:kind,candidateId:id.map(JsonValue::from).unwrap_or(JsonValue::Null),count:"1",message:message}}
fn member_json(name:&str,index:Option<u16>,size:u64)->JsonValue{object!{name:name,role:if index.is_none(){"directory"}else{"companion"},index:index.map(|v|JsonValue::from(v.to_string())).unwrap_or(JsonValue::Null),measuredSize:size.to_string()}}
fn archive_diagnostics(archive:&PreparedArchive)->Vec<JsonValue>{
    archive.members().iter().filter_map(|m|match &m.kind {
        ArchiveKind::Rejected(reason)=>Some(match reason.as_str(){
            "notVpk"=>diagnostic("excludedNonVpk",None,"A non-VPK archive entry was excluded."),
            "nestedArchiveNotExpanded"=>diagnostic("nestedArchive",None,"A nested archive was not expanded."),
            _=>diagnostic("unsupportedType",None,"An unsupported archive entry was excluded."),
        }),_=>None,
    }).collect()
}

impl RemotePreparation {
    pub(crate) fn start(source:SealedRemoteSourceV1,slot:PreparationSlotLease,authority:RemoteQuarantineAuthority,context:PrepareContext,control:PrepareControl)->Result<PreparedRemote,NativeError>{
        Self::start_inner(source,slot,authority,context,control).map_err(public_error)
    }
    fn start_inner(source:SealedRemoteSourceV1,slot:PreparationSlotLease,authority:RemoteQuarantineAuthority,context:PrepareContext,control:PrepareControl)->Result<PreparedRemote,NativeError>{
        authority.validate(&slot)?;
        if source.job_id!=authority.job || context.root!=authority.root() || context.prepare_key.as_str()!=authority.nonce.as_str() {return Err(stale());}
        if Instant::now()>=context.expires_at{return Err(stale());}
        owner::revalidate_sealed_source(&source,&authority.store_lease,&control,false)?;
        if !authority.inspect(&slot)?.is_empty(){return Err(NativeError::new("recoveryRequired","A previous preparation requires reconciliation."));}
        let source_stamps=source.members.iter().map(|m|FileStamp::read(&source_file(m)?)).collect::<Result<Vec<_>,NativeError>>()?;
        let mut source_bytes=0u64;
        for m in &source.members {source_bytes=size_add(source_bytes,m.receipt.measured_size.parse().map_err(|_|stale())?,contract::MAX_COHORT_BYTES)?;}
        let quarantine=authority.create(&slot)?;
        let mut budget=ArchiveBudget::new();let root=&source.members[0];let raw=root.receipt.file.original_name.to_ascii_lowercase().ends_with(".vpk");
        budget.admit_roots(if raw{source.members.len()}else{1})?;
        let mut fingerprints=BTreeMap::new();let mut found=Vec::new();let mut archives=Vec::new();let mut diagnostics=Vec::new();
        if raw {
            for member in &source.members {
                control.check()?;let mut file=source_file(member)?;let fp=fingerprint(&file,&member.receipt.sha256)?;
                let (path,copy)=archive_content::snapshot_held_member(&mut file,&member.receipt.file.original_name,&fp,&quarantine,&mut budget,&control)?;
                fingerprints.insert(path.clone(),copy);found.push((member.receipt.file.original_name.clone(),path));
            }
        } else {
            if source.members.len()!=1{return Err(invalid());}
            let mut file=source_file(root)?;let fp=fingerprint(&file,&root.receipt.sha256)?;
            let input=ArchiveInput{root_ordinal:0,relative_path:root.receipt.file.original_name.clone(),original_path:PathBuf::new(),fingerprint:fp};
            let archive=Arc::new(archive_content::prepare_archive_held(input,&mut file,quarantine.clone(),&mut budget,&control)?);
            diagnostics.extend(archive_diagnostics(&archive));
            fingerprints.insert(archive.snapshot_path().to_owned(),archive.snapshot_fingerprint().clone());
            for member in archive.extracted_members(){fingerprints.insert(member.path().to_owned(),member.fingerprint().clone());found.push((member.member().normalized_path.clone(),member.path().to_owned()));}
            archives.push(archive);
        }
        if diagnostics.len()>contract::MAX_ARCHIVE_ENTRIES as usize{return Err(limit());}
        let resolved=local_content::resolve_remote_families(found.clone(),&control,&mut budget)?;
        if raw && (resolved.len()!=1 || resolved[0].relative_path!=root.receipt.file.original_name || resolved[0].package.is_none()) {return Err(invalid());}
        let path_by_name=found.into_iter().collect::<BTreeMap<_,_>>();let mut candidates=Vec::new();let mut payloads=HashMap::<(String,u64,String),FileRef>::new();
        let mut copies=BTreeMap::<String,PathBuf>::new();
        for candidate in resolved {
            control.check()?;
            let dir_path=path_by_name.get(&candidate.relative_path).ok_or_else(stale)?;
            let dir_fp=fingerprints.get(dir_path).ok_or_else(stale)?;
            if dir_fp.size==0 {diagnostics.push(diagnostic("invalidFamily",None,"An empty VPK candidate is invalid."));continue;}
            let dir_name=Path::new(&candidate.relative_path).file_name().and_then(|n|n.to_str()).filter(|n|paths::safe_name(n)).ok_or_else(limit)?;
            let namespace=Path::new(&candidate.relative_path).parent().and_then(Path::to_str).unwrap_or("");
            let mut digest_members=vec![CandidateDigestMember{role:"directory".into(),index:None,normalized_name:dir_name.into(),sha256:dir_fp.sha256.clone(),size:dir_fp.size.to_string()}];
            let mut members=vec![member_json(dir_name,None,dir_fp.size)];let mut logical=dir_fp.size;
            let observed=candidate.package.as_ref().map(|p|&p.companions).unwrap_or(&candidate.observed_companions);
            for (index,association) in observed {
                let fp=fingerprints.get(&association.source_path).ok_or_else(stale)?;
                if fp.size==0 {continue;}
                logical=size_add(logical,fp.size,contract::MAX_PAYLOAD_BYTES)?;
                digest_members.push(CandidateDigestMember{role:"companion".into(),index:Some(index.to_string()),normalized_name:association.original_name.clone(),sha256:fp.sha256.clone(),size:fp.size.to_string()});
                members.push(member_json(&association.original_name,Some(*index),fp.size));
            }
            let id=CandidateId::parse(&contract::prepared_candidate_id(&source.job_id,&context.prepare_key,namespace,&digest_members)?)?;
            let mut package_ref=None;
            if let Some(package)=&candidate.package {
                let mut allocate=|association:&package::SourceAssociation|->Result<FileRef,NativeError>{
                    let fp=fingerprints.get(&association.source_path).ok_or_else(stale)?;let key=(fp.sha256.clone(),fp.size,association.original_name.clone());
                    if let Some(existing)=payloads.get(&key){return Ok(existing.clone());}
                    let file=FileRef{id:paths::random_id()?,sha256:fp.sha256.clone(),size:fp.size,source_name:association.original_name.clone()};
                    if copies.insert(file.id.clone(),association.source_path.clone()).is_some(){return Err(limit());}payloads.insert(key,file.clone());Ok(file)
                };
                let directory=allocate(&package.directory)?;let companions=package.companions.iter().map(|(index,a)|Ok((*index,allocate(a)?))).collect::<Result<_,NativeError>>()?;
                package_ref=Some(PackageRef{directory,companions});
            }
            let incomplete=candidate.unavailable_reason.as_deref()==Some("incompleteOrAmbiguousFamily");
            let reason=if package_ref.is_some(){None}else if incomplete{Some("Required VPK companions are missing or ambiguous.".to_owned())}else{Some("This VPK family is invalid.".to_owned())};
            if reason.is_some(){diagnostics.push(if incomplete{diagnostic("incompleteFamily",Some(id.as_str()),"Required VPK companions are missing or ambiguous.")}else{diagnostic("invalidFamily",Some(id.as_str()),"A VPK family is invalid.")});}
            candidates.push(Candidate{id,option_id:if package_ref.is_some(){Some(paths::random_id()?)}else{None},option_name:candidate.option_name,source_label:dir_name.into(),source:candidate.package,package:package_ref,members,logical,reason});
        }
        if candidates.len()>contract::MAX_CANDIDATES as usize||diagnostics.len()>contract::MAX_ARCHIVE_ENTRIES as usize{return Err(limit());}
        let mut labels=HashMap::new();for c in &candidates{*labels.entry(c.source_label.to_lowercase()).or_insert(0usize)+=1;}
        for c in &mut candidates{c.source_label=short_label(&c.source_label,c.id.as_str(),labels[&c.source_label.to_lowercase()]>1)?;}
        let mut unique_payload_bytes=0;for file in payloads.values(){unique_payload_bytes=size_add(unique_payload_bytes,file.size,contract::MAX_PAYLOAD_BYTES)?;}
        if unique_payload_bytes>137589948416{return Err(limit());}
        owner::revalidate_sealed_source(&source,&authority.store_lease,&control,false)?;quarantine.validate_sealed()?;authority.validate(&slot)?;
        let mut prepared=PreparedRemote{source,slot,authority,quarantine,archives,fingerprints,context,candidates,diagnostics,source_bytes,unique_payload_bytes,verification:Vec::new()};
        let stamps=prepared.stamps()?;
        if stamps[..source_stamps.len()]!=source_stamps {return Err(stale());}
        prepared.verification=stamps;
        // Ensure no individual page can be silently omitted at the boundary.
        for p in 0..prepared.candidates.len().div_ceil(8){prepared.candidate_page(p as u32)?;}
        for c in &prepared.candidates{for p in 0..c.members.len().div_ceil(8){prepared.member_page(&c.id,p as u32)?;}}
        for p in 0..prepared.diagnostics.len().div_ceil(8){prepared.diagnostic_page(p as u32)?;}
        Ok(prepared)
    }
}

fn slice_page(rows:&[JsonValue],page:u32)->Result<(bool,Vec<JsonValue>),NativeError>{
    let start=(page as usize).checked_mul(8).ok_or_else(invalid)?;let end=start.saturating_add(8).min(rows.len());
    Ok((end<rows.len(),if start<rows.len(){rows[start..end].to_vec()}else{Vec::new()}))
}
fn check_page(method:contract::RemoteMethod,result:JsonValue)->Result<(),NativeError>{
    let checked=contract::RemoteResultV1::parse(method,result)?;
    contract::encode_result_envelope(&JsonValue::from(i64::MIN),&checked).map(|_|()).map_err(|_|limit())
}

fn created_at()->Result<String,NativeError>{
    let seconds=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|invalid())?.as_secs();
    let z=(seconds/86400) as i64+719468;let era=z/146097;let doe=z-era*146097;let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;
    let y=yoe+era*400;let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;let day=doy-(153*mp+2)/5+1;
    let month=mp+if mp<10{3}else{-9};let year=y+if month<=2{1}else{0};
    Ok(format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",seconds/3600%24,seconds/60%60,seconds%60))
}
pub(crate) fn reject_duplicate(doc:&JsonValue,profile:&str,submission:&contract::SubmissionKeyV1)->Result<(),NativeError>{
    let target=crate::catalog::provider::SubmissionKeyV1::parse(&submission.json())?;
    for row in doc["profiles"][profile]["mods"].members(){
        let source=&row["source"];let Some(id)=source["id"].as_str().filter(|id|paths::safe_id(id)) else {continue;};
        if source.has_key("nativeRemoteLink") {
            let linked=replacement_target::admit(row.clone(),package::layout_version(doc)?,profile,id,store::revision(doc["revision"].as_str().ok_or_else(invalid)?)?);
            if linked.is_ok_and(|t|t.submission.as_ref()==Some(submission)){return Err(NativeError::new("duplicateSubmission",format!("This submission already exists as {id}.")));}
            continue;
        }
        if !source["localImport"].is_null() || source["kind"]=="local" || id.strip_prefix("local-").is_some_and(|suffix|package::hex(suffix,32)) {continue;}
        let Some(slug)=source["remoteId"].as_str() else {continue;};
        let Ok(parsed)=crate::catalog::provider::SubmissionKeyV1::from_slug(slug) else {continue;};
        if parsed.slug()!=slug||parsed!=target{continue;}
        // Unestablished historical discriminators and contradictory explicit
        // identity evidence cannot authorize a duplicate decision.
        if !source["type"].is_null()||!source["origin"].is_null()
            ||source.has_key("provider")&&source["provider"]!="gamebanana"
            ||source.has_key("submissionType")&&source["submissionType"]!=submission.submission_type
            ||source.has_key("submissionId")&&source["submissionId"]!=submission.submission_id
            ||!(source["link"].is_null()||source["link"]==""||source["link"]==submission.profile_url()){continue;}
        let receipt=&source["remoteReceipt"];
        let recognized=if !receipt.is_null(){
            receipt.is_object()&&receipt.len()==4&&receipt["version"]==1
                &&contract::SubmissionKeyV1::parse(&receipt["submission"]).is_ok_and(|key|key==*submission)
                &&receipt["rootFileId"].as_str().is_some_and(|id|store::revision(id).is_ok_and(|n|n>0))
                &&receipt["cohortFingerprint"].as_str().is_some_and(|hash|package::hex(hash,64))
                &&source["kind"]=="remote"
        }else{
            source["kind"].is_null()
        };
        if recognized{return Err(NativeError::new("duplicateSubmission",format!("This submission already exists as {id}.")));}
    }
    Ok(())
}
impl PreparedRemote {
    pub(crate) fn root(&self)->&Path{&self.context.root}
    pub(crate) fn submission(&self)->&contract::SubmissionKeyV1{&self.source.submission}
    pub(crate) fn mode(&self)->&'static str{if self.archives.is_empty(){"rawVpk"}else{"archiveVpk"}}
    pub(crate) fn validate_commit_input(&self,input:&contract::RemoteCommitInputV1)->Result<(),NativeError>{
        if self.context.replacement.is_some(){return Err(stale());}self.validate_commit_params(input.params())
    }
    fn validate_commit_params(&self,p:&JsonValue)->Result<(),NativeError>{
        if p["jobId"]!=self.source.job_id.as_str() || p["profileId"]!=self.context.profile_id
            || p["expectedRevision"]!=self.context.expected_revision.to_string()
            || p["consentFingerprint"]!=self.source.consent_fingerprint || p["cohortFingerprint"]!=self.source.cohort_fingerprint {
            return Err(stale());
        }
        Ok(())
    }
    /// Bind the retained handles to the current complete durable job cohort.
    pub(crate) fn validate_job(&self,row:&super::job_store::JobSnapshot)->Result<(),NativeError>{
        if row.input["jobId"]!=self.source.job_id.as_str() || row.input["submission"]!=self.source.submission.json()
            || row.input["rootFileId"]!=self.source.root_file_id || row.input["profileId"]!=self.context.profile_id
            || row.input["consentFingerprint"]!=self.source.consent_fingerprint || row.body["cohort"]["complete"]!=true
            || row.body["cohort"]["fingerprint"]!=self.source.cohort_fingerprint
            || row.body["prepare"]["prepareKey"]!=self.context.prepare_key.as_str()
            || row.body["prepare"]["expectedRevision"]!=self.context.expected_revision.to_string()
            || row.body["claims"].len()!=self.source.members.len() || row.body["cohort"]["files"].len()!=self.source.members.len(){return Err(stale());}
        match &self.context.replacement {
            None if row.input["version"]==1=>{},
            Some(pin) if row.input["version"]==2&&row.input["purpose"]==pin.purpose.as_str()&&row.input["modId"]==pin.mod_id
                &&row.input["targetDigest"]==pin.target_digest&&row.input["initialRevision"]==pin.initial_revision=>{},
            _=>return Err(stale()),
        }
        for (member,file) in self.source.members.iter().zip(row.body["cohort"]["files"].members()){
            let claim=row.body["claims"].members().find(|c|c["claimId"]==member.claim_id.as_str()).ok_or_else(stale)?;
            let id=&member.claim_identity;
            if *file!=member.receipt.file.json() || claim["fileId"]!=member.file_key.file_id || claim["state"]!="sealed"
                || claim["receipt"]!=member.receipt.json() || claim["dirDevice"]!=id.directory_device.to_string()
                || claim["dirInode"]!=id.directory_inode.to_string() || claim["fileDevice"]!=id.data_device.to_string()
                || claim["fileInode"]!=id.data_inode.to_string() || claim["leaseDevice"]!=id.lease_device.to_string()
                || claim["leaseInode"]!=id.lease_inode.to_string() || claim["uid"]!=id.uid.to_string()
                || claim["linkCount"]!=id.link_count.to_string(){return Err(stale());}
        }
        Ok(())
    }
    fn stamps(&self)->Result<Vec<FileStamp>,NativeError>{
        let mut values=Vec::new();for member in &self.source.members{values.push(FileStamp::read(&File::from(member.read_fd.try_clone()?))?);}
        for path in self.fingerprints.keys(){values.push(FileStamp::read(&paths::open_regular(path)?)?);}Ok(values)
    }
    fn base(&self)->JsonValue{object!{version:1,root:self.context.root.to_str(),sessionId:self.context.session.as_str(),jobId:self.source.job_id.as_str(),prepareKey:self.context.prepare_key.as_str()}}
    pub(crate) fn candidate_page(&self,page:u32)->Result<CandidatePageV1,NativeError>{
        let rows=self.candidates.iter().map(|c|object!{candidateId:c.id.as_str(),optionId:c.option_id.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null),optionName:c.option_name.clone(),sourceLabel:c.source_label.clone(),available:c.package.is_some(),unavailableReason:c.reason.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null),memberCount:c.members.len().to_string(),logicalBytes:if c.package.is_some(){JsonValue::from(c.logical.to_string())}else{JsonValue::Null}}).collect::<Vec<_>>();
        let (has_more,candidates)=slice_page(&rows,page)?;let mut result=self.base();result["page"]=page.into();result["hasMore"]=has_more.into();result["candidates"]=JsonValue::from(candidates.clone());
        check_page(contract::RemoteMethod::Candidates,result)?;Ok(CandidatePageV1{page,has_more,candidates})
    }
    pub(crate) fn member_page(&self,id:&CandidateId,page:u32)->Result<MemberPageV1,NativeError>{
        let candidate=self.candidates.iter().find(|c|&c.id==id).ok_or_else(invalid)?;let (has_more,members)=slice_page(&candidate.members,page)?;
        let mut result=self.base();result["candidateId"]=id.as_str().into();result["page"]=page.into();result["hasMore"]=has_more.into();result["members"]=JsonValue::from(members.clone());
        check_page(contract::RemoteMethod::CandidateMembers,result)?;Ok(MemberPageV1{page,has_more,members})
    }
    pub(crate) fn diagnostic_page(&self,page:u32)->Result<DiagnosticPageV1,NativeError>{
        let (has_more,diagnostics)=slice_page(&self.diagnostics,page)?;let mut result=self.base();result["page"]=page.into();result["hasMore"]=has_more.into();result["diagnostics"]=JsonValue::from(diagnostics.clone());
        check_page(contract::RemoteMethod::Diagnostics,result)?;Ok(DiagnosticPageV1{page,has_more,diagnostics})
    }
    pub(crate) fn revalidate(&self,control:&PrepareControl)->Result<(),NativeError>{
        self.authority.validate(&self.slot)?;self.quarantine.validate_sealed()?;
        owner::revalidate_sealed_source(&self.source,&self.authority.store_lease,control,false)?;
        let before=self.stamps()?;
        if self.verification==before{
            self.quarantine.validate_sealed()?;self.authority.validate(&self.slot)?;return Ok(());
        }
        // A changed identity invalidates this preparation; never rehash it into
        // fresh authority. Every private file was verified by its producer.
        Err(stale())
    }
    pub(crate) fn validate_live(&self,control:&PrepareControl)->Result<(),NativeError>{self.revalidate(control)}

    pub(crate) fn slot(&self)->PreparationSlotLease{self.slot.clone()}
    pub(crate) fn authority(&self)->RemoteQuarantineAuthority{self.authority.clone()}
    pub(crate) fn review(self:&Arc<Self>,request:RemoteReviewV1)->Result<RemotePublicationPlan,NativeError>{
        let control=request.control.clone();self.review_inner(request,&control).map_err(public_error)
    }
    fn review_inner(self:&Arc<Self>,request:RemoteReviewV1,control:&PrepareControl)->Result<RemotePublicationPlan,NativeError>{
        if request.candidate_ids.is_empty()||request.candidate_ids.len()>99||request.candidate_ids.windows(2).any(|w|w[0]>=w[1]){return Err(invalid());}
        let selected=request.candidate_ids.iter().collect::<BTreeSet<_>>();let mut selected_ids=Vec::new();let mut entries=Vec::new();let mut copies=BTreeMap::new();let mut retained_logical=0u64;
        for candidate in &self.candidates {
            let Some(package)=&candidate.package else {if selected.contains(&candidate.id.0){return Err(invalid());}continue;};
            let option_id=candidate.option_id.clone().ok_or_else(stale)?;
            if selected.contains(&candidate.id.0){selected_ids.push(option_id.clone());}else{retained_logical=size_add(retained_logical,candidate.logical,contract::MAX_PAYLOAD_BYTES)?;}
            entries.push(OptionEntry{id:option_id,option_name:candidate.option_name.clone(),package:Some(package.clone()),expected_family:Some(ExpectedFamily::from_package(package)),archive_binding:None,source_label:candidate.source_label.clone(),unavailable_reason:None});
            let source=candidate.source.as_ref().ok_or_else(stale)?;
            for (file,association) in std::iter::once((&package.directory,&source.directory)).chain(package.companions.iter().map(|(i,f)|(f,&source.companions[i]))) {
                let fp=self.fingerprints.get(&association.source_path).ok_or_else(stale)?;
                if let Some((old,_,_))=copies.get(&file.id){if old!=file{return Err(stale());}}
                else{copies.insert(file.id.clone(),(file.clone(),association.source_path.clone(),fp.clone()));}
            }
        }
        if selected_ids.len()!=selected.len(){return Err(invalid());}selected_ids.sort();
        let catalog=OptionCatalog{entries,selected_ids:selected_ids.clone(),retained_archives:Vec::new(),attachments:Vec::new()};catalog.validate_selection_save()?;
        let ready=catalog.entries.len();let retained=ready.checked_sub(selected_ids.len()).ok_or_else(invalid)?;
        let summary=object!{selectedCandidateIds:JsonValue::from(request.candidate_ids.clone()),selectedOptionIds:JsonValue::from(selected_ids.clone()),readyCount:ready.to_string(),selectedCount:request.candidate_ids.len().to_string(),retainedCount:retained.to_string(),uniquePayloadBytes:self.unique_payload_bytes.to_string(),retainedLogicalBytes:retained_logical.to_string(),sourceBytes:self.source_bytes.to_string(),diagnosticCount:self.diagnostics.len().to_string()};
        self.revalidate(control)?;if Instant::now()>=request.expires_at{return Err(stale());}
        let lock=RootLock::acquire(&self.context.root)?;control.check()?;if Instant::now()>=request.expires_at{return Err(stale());}
        self.authority.validate(&self.slot)?;let library=Store::open(&self.context.root)?;crate::apply::ensure_idle(&library)?;
        let before=library.document()?;if before["revision"]!=self.context.expected_revision.to_string(){return Err(NativeError::new("staleRevision","The pinned profile revision changed."));}
        if !before["profiles"][&self.context.profile_id].is_object(){return Err(invalid());}
        let library_before=package::document_version(&before)?;let layout=package::layout_version(&before)?;
        for candidate in self.candidates.iter().filter(|c|c.package.is_some()) {
            control.check()?;
            if layout==1 {let source=candidate.source.as_ref().ok_or_else(stale)?;if !source.companions.is_empty()||crate::vpk::inspect(&source.directory.source_path).is_err(){return Err(NativeError::new("layoutUnsupported","The pinned library accepts only compatible standalone VPKs."));}}
        }
        let ids=copies.keys().cloned().collect::<BTreeSet<_>>();
        for (_,profile) in before["profiles"].entries(){for old in profile["mods"].members(){if crate::local_import::record_files(old,layout)?.iter().any(|(f,_)|ids.contains(&f.id)){return Err(limit());}}}
        if crate::font_transaction::backup_refs(&before)?.iter().any(|f|ids.contains(&f.id)){return Err(limit());}
        let token=contract::ReviewToken::generate()?.as_str().to_owned();let operation="x".repeat(64);
        let mut document=before.clone();
        let (mod_id,record,selection_digest,purpose,input,before_record)=if let Some(pin)=&self.context.replacement {
            if library_before!=4 && pin.purpose!=ReplacementPurpose::Link{return Err(invalid());}
            let old=replacement_target::read(&self.context.root,&self.context.profile_id,&pin.mod_id,self.context.expected_revision)?;
            if old.layout!=layout||old.digest!=pin.target_digest||!old.accepts_submission(pin.purpose,&self.source.submission){return Err(stale());}
            if pin.purpose==ReplacementPurpose::Update{super::replacement_skips::check_update(&self.authority.store_lease,&lock,&self.context.profile_id,&pin.mod_id,&pin.target_digest,&self.source.root_file_id,&self.source.consent_fingerprint)?;}
            let claimed=copies.keys().cloned().collect::<std::collections::HashSet<_>>();
            replacement_target::reject_payload_collisions(&self.context.root,&before,layout,&claimed)?;
            let saved_catalog=if pin.purpose==ReplacementPurpose::Append {
                replacement_target::append_options(&old.record,&catalog,layout)?
            } else { catalog.encode(layout)? };
            let mut record=replacement_target::replace_record(&old.record,saved_catalog,object!{version:1,submission:self.source.submission.json(),rootFileId:self.source.root_file_id.clone(),cohortFingerprint:self.source.cohort_fingerprint.clone()},pin.purpose);
            if pin.purpose != ReplacementPurpose::Link { record["updatedAt"] = created_at()?.into(); }
            replacement_target::transition(&mut document,layout)?;
            crate::options::for_record(&record,layout)?.validate_selection_save()?;
            let after=crate::local_content::option_sources::record_digest(&record)?;
            let mut input=object!{method:"native.remote.replaceCommit",params:object!{operationId:operation.clone(),jobId:self.source.job_id.as_str(),profileId:self.context.profile_id.clone(),modId:pin.mod_id.clone(),purpose:pin.purpose.as_str(),initialRevision:pin.initial_revision.clone(),expectedRevision:self.context.expected_revision.to_string(),targetDigest:pin.target_digest.clone(),afterRecordDigest:after.clone(),consentFingerprint:self.source.consent_fingerprint.clone(),cohortFingerprint:self.source.cohort_fingerprint.clone(),selectionDigest:"f".repeat(64)}};
            let digest=replacement_contract::selection_digest(&ReplacementCommitInputV1::parse(input.clone())?,&JsonValue::from(request.candidate_ids.clone()),&JsonValue::from(selected_ids.clone()))?;
            input["params"]["selectionDigest"]=digest.clone().into();
            let index=document["profiles"][&self.context.profile_id]["mods"].members().position(|row|row["source"]["id"]==pin.mod_id).ok_or_else(stale)?;
            document["profiles"][&self.context.profile_id]["mods"][index]=record.clone();
            (pin.mod_id.clone(),record,digest,RemotePlanPurpose::Replace{pin:pin.clone(),after_record_digest:after,candidate_ids:request.candidate_ids.clone(),option_ids:selected_ids.clone()},input,old.record)
        }else{
            reject_duplicate(&before,&self.context.profile_id,&self.source.submission)?;
            let digest=contract::selection_digest(&selected_ids,&request.metadata)?;
            let mut max_order=None::<i64>;for (i,m) in before["profiles"][&self.context.profile_id]["mods"].members().enumerate(){let order=m["source"]["installOrder"].as_i64().unwrap_or(i64::try_from(i).map_err(|_|limit())?);max_order=Some(max_order.map_or(order,|n|n.max(order)));}
            let order=max_order.unwrap_or(-1).checked_add(1).ok_or_else(limit)?;let mod_id=format!("remote-{}",paths::random_id()?);
            if before["profiles"].entries().any(|(_,profile)|profile["mods"].members().any(|row|row["source"]["id"]==mod_id)){return Err(limit());}
            let slug=crate::catalog::provider::SubmissionKeyV1::parse(&self.source.submission.json())?.slug();
            let record=object!{source:object!{id:mod_id.clone(),remoteId:slug,kind:"remote",name:request.metadata["name"].clone(),author:request.metadata["author"].clone(),category:request.metadata["category"].clone(),link:"",description:request.metadata["description"].clone(),status:"downloaded",installedVpks:json::array![],installOrder:order,createdAt:created_at()?,remoteReceipt:object!{version:1,submission:self.source.submission.json(),rootFileId:self.source.root_file_id.clone(),cohortFingerprint:self.source.cohort_fingerprint.clone()}},available:true,previewHidden:false,enabled:false,options:catalog.encode(layout)?};
            crate::options::for_record(&record,layout)?.validate_selection_save()?;
            document["storeVersion"]=4.into();document["packageVersion"]=layout.into();if library_before<3{document["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
            document["profiles"][&self.context.profile_id]["mods"].push(record.clone()).map_err(|_|limit())?;
            let input=object!{method:"native.remote.commit",params:object!{operationId:operation.clone(),jobId:self.source.job_id.as_str(),profileId:self.context.profile_id.clone(),expectedRevision:self.context.expected_revision.to_string(),consentFingerprint:self.source.consent_fingerprint.clone(),cohortFingerprint:self.source.cohort_fingerprint.clone(),selectionDigest:digest.clone()}};
            (mod_id,record,digest,RemotePlanPurpose::Add,input,JsonValue::Null)
        };
        document["revision"]=self.context.expected_revision.checked_add(1).ok_or_else(limit)?.to_string().into();store::ensure_readable(&document)?;
        let snapshot=store::snapshot_document(&document,Some(&self.context.profile_id))?;
        store::ensure_internal(&object!{operationId:operation.clone(),input:input.clone(),outcome:object!{status:"succeeded",result:snapshot}})?;
        let rows=JsonValue::from(copies.values().map(|(f,_,_)|object!{id:f.id.clone(),sha256:f.sha256.clone(),size:f.size,sourceName:f.source_name.clone(),role:"payload",device:u64::MAX,inode:u64::MAX,complete:true}).collect::<Vec<_>>());
        let mut journal=object!{kind:"remotePayloadImport",version:1,action:"addRemotePayloadMod",mode:if self.archives.is_empty(){"rawVpk"}else{"archiveVpk"},operationId:operation.clone(),input:input.clone(),expectedRevision:self.context.expected_revision.to_string(),targetProfileId:self.context.profile_id.clone(),beforeDigest:"f".repeat(64),afterDigest:"f".repeat(64),beforeDocument:before,libraryVersionBefore:library_before,libraryVersionAfter:4,packageVersion:layout,modId:mod_id.clone(),record:record.clone(),stage:format!(".import-remote-payload-{}","f".repeat(32)),owner:object!{kind:"remotePayloadImport",version:1,action:"addRemotePayloadMod",operationId:operation.clone(),stage:format!(".import-remote-payload-{}","f".repeat(32)),nonce:"f".repeat(32),rootDevice:u64::MAX.to_string(),rootInode:u64::MAX.to_string(),authorityDigest:"f".repeat(64)},stageDevice:u64::MAX,stageInode:u64::MAX,ownerDevice:u64::MAX,ownerInode:u64::MAX,files:rows,reusedFiles:json::array![],cleanup:object!{mode:"rollback",cursor:u64::MAX.to_string(),intent:object!{kind:"destination",fileId:"f".repeat(32),device:u64::MAX,inode:u64::MAX}}};
        if let RemotePlanPurpose::Replace{candidate_ids,option_ids,..}=&purpose{
            journal["kind"]="remotePayloadReplace".into();journal["action"]="replaceRemotePayloadMod".into();journal["stage"]=format!(".import-remote-replacement-{}","f".repeat(32)).into();
            journal["owner"]["kind"]="remotePayloadReplace".into();journal["owner"]["action"]="replaceRemotePayloadMod".into();journal["owner"]["stage"]=journal["stage"].clone();
            journal["beforeRecord"]=before_record;journal["candidateIds"]=JsonValue::from(candidate_ids.clone());journal["optionIds"]=JsonValue::from(option_ids.clone());
        }store::ensure_internal(&journal)?;
        let recovery_kind=if matches!(&purpose,RemotePlanPurpose::Replace{..}){"remotePayloadReplaceRecovery"}else{"remotePayloadRecovery"};
        store::ensure_internal(&object!{kind:recovery_kind,version:1,operationId:"x".repeat(64),input:object!{method:"native.recover",params:object!{operationId:"x".repeat(64),expectedRevision:u64::MAX.to_string()}},expectedRevision:u64::MAX.to_string(),targetOperationId:"x".repeat(64),targetInputSha256:"f".repeat(64)})?;
        let mut status=self.context.status_template.clone();status["state"]="ready".into();status["prepareKey"]=self.context.prepare_key.as_str().into();status["reviewRevision"]=self.context.expected_revision.to_string().into();
        let (method,status)=match &purpose{
            RemotePlanPurpose::Add=>{status["review"]=object!{requestKey:request.request_key,phase:"ready",reviewToken:token.clone(),selectionDigest:selection_digest.clone(),modId:mod_id.clone(),summary:summary.clone(),error:JsonValue::Null};(contract::RemoteMethod::Status,status)},
            RemotePlanPurpose::Replace{pin,after_record_digest,..}=>{let review=object!{requestKey:request.request_key,phase:"ready",reviewToken:token.clone(),selectionDigest:selection_digest.clone(),afterRecordDigest:after_record_digest.clone(),summary:summary.clone(),error:JsonValue::Null};
                (contract::RemoteMethod::ReplaceStatus,object!{version:1,root:self.context.root.to_str(),sessionId:self.context.session.as_str(),purpose:pin.purpose.as_str(),modId:pin.mod_id.clone(),targetDigest:pin.target_digest.clone(),status:status,review:review})},
        };
        let checked=contract::RemoteResultV1::parse(method,status)?;contract::encode_result_envelope(&JsonValue::from(i64::MIN),&checked).map_err(|_|limit())?;
        control.check()?;if Instant::now()>=request.expires_at{return Err(stale());}drop(lock);
        Ok(RemotePublicationPlan{prepared:self.clone(),token,mod_id,selection_digest,profile_id:self.context.profile_id.clone(),expected_revision:self.context.expected_revision,record,projected_document:document,summary,copies,expires_at:request.expires_at,purpose})
    }
}
impl RemotePublicationPlan {
    pub(crate) fn validate_commit(&self,input:&contract::RemoteCommitInput)->Result<(),NativeError>{
        self.prepared.validate_commit_params(input.params())?;let p=input.params();
        if p["selectionDigest"]!=self.selection_digest||p["profileId"]!=self.profile_id||p["expectedRevision"]!=self.expected_revision.to_string(){return Err(stale());}
        match (&self.purpose,input){
            (RemotePlanPurpose::Add,contract::RemoteCommitInput::Add(_))=>Ok(()),
            (RemotePlanPurpose::Replace{pin,after_record_digest,..},contract::RemoteCommitInput::Replace(_))
                if p["modId"]==pin.mod_id&&p["purpose"]==pin.purpose.as_str()&&p["initialRevision"]==pin.initial_revision
                    &&p["targetDigest"]==pin.target_digest&&p["afterRecordDigest"]==*after_record_digest=>Ok(()),
            _=>Err(stale()),
        }
    }
}

#[cfg(test)]
#[path = "preparation_tests.rs"]
mod tests;
