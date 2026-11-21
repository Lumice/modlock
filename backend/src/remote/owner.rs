//! Bounded service-thread facade and one root-bound remote worker.
use super::{authority::{self, AuthorityRequest, ExpectedSelection, FreshAuthorization, FreshFileAuthorityV1, MetadataRoute, RawResponse}, contract::{self, ConsentFile, JobId, RemoteCommitInput, SessionId, SubmissionKeyV1}, job_store::{self, JobSnapshot, JobTransition, RemoteJobInputV1, ReplacementJobInputV2, RemoteJobStore, RemoteStoreLease}, metadata::RemoteMetadata, preparation::{CandidateId,PrepareContext,PreparedRemote,RemotePreparation,RemotePublicationPlan,RemoteReviewV1,RemotePlanPurpose,ReplacementPin}, preparation_slot::{PreparationSlot,PreparationSlotLease}, publication::RemotePublisher, quarantine::RemoteQuarantineAuthority, replacement_contract::ReplacementPurpose, replacement_skips, replacement_target, transfer::{self,TransferChild,TransferCommandV1,TransferStartV1}};
use crate::{apply, native::NativeError, paths, store::{self,ExactRemoteOutcome,RootLock,Store}};
use json::{object, JsonValue};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap, BTreeSet, VecDeque}, ffi::{CString,c_void}, fs::{File,OpenOptions}, io::{self,Read,Seek,SeekFrom}, os::{fd::{AsRawFd,FromRawFd}, unix::fs::{MetadataExt,OpenOptionsExt}}, path::{Path,PathBuf}, sync::{Arc,Condvar,Mutex,Weak,atomic::{AtomicBool,Ordering}}, thread::{self,JoinHandle}, time::{Duration,Instant}};

const MAX_JOBS:usize=8;
const MAX_RESOLVES:usize=8;
const MAX_QUEUE:usize=32;
const NONCONTROL_QUEUE:usize=MAX_QUEUE-MAX_JOBS;
fn busy()->NativeError{NativeError::new("remoteBusy","Remote work is not yet confirmed; retain and retry the same identity.")}
fn unknown()->NativeError{NativeError::new("outcomeUnknown","The remote outcome is not yet known; retain and reconcile the same identity.")}
fn conflict()->NativeError{NativeError::new("jobIdConflict","The remote identity belongs to different input.")}
fn failure(e:&NativeError)->JsonValue{
    let kind=if contract::ERROR_KINDS.contains(&e.kind.as_str()) && e.kind!="jobAbsent"{e.kind.as_str()}else{"transportStorage"};
    object!{kind:kind,message:if e.message.len()<=512&&!e.message.chars().any(char::is_control)&&!e.message.contains('/')&&!e.message.contains('\\'){e.message.clone()}else{"Remote work could not be completed.".to_owned()},retryAfterSeconds:JsonValue::Null}
}
fn same_start(a:&JsonValue,b:&JsonValue)->bool{["jobId","candidateFingerprint","fileId","profileId","expectedRevision","acceptArchived"].iter().all(|k|a[*k]==b[*k])
    &&a.has_key("purpose")==b.has_key("purpose")&&["purpose","modId","targetDigest"].iter().all(|k|a[*k]==b[*k])}
fn same_stored_start(p:&JsonValue,j:&JobSnapshot)->bool{
    p["jobId"]==j.input["jobId"]&&p["candidateFingerprint"]==j.input["consentFingerprint"]&&p["fileId"]==j.input["rootFileId"]&&p["profileId"]==j.input["profileId"]&&p["expectedRevision"]==j.input["initialRevision"]&&p["acceptArchived"]==j.input["acceptArchived"]
        &&p.has_key("purpose")== (j.input["version"]==2)&&["purpose","modId","targetDigest"].iter().all(|k|p[*k]==j.input[*k])
}
fn pinned_profile(root:&Path,p:&JsonValue)->Result<String,NativeError>{
    let library=Store::open(root)?;apply::ensure_idle(&library)?;let document=library.document()?;
    if document["revision"]!=p["expectedRevision"]{return Err(NativeError::new("staleRevision","The pinned profile revision changed."));}
    let profile=p["profileId"].as_str().ok_or_else(NativeError::params)?;if !document["profiles"][profile].is_object(){return Err(NativeError::new("invalidSelection","The selected profile is unavailable."));}
    document["profiles"][profile]["name"].as_str().filter(|s|!s.is_empty()&&s.len()<=256&&!s.chars().any(char::is_control)).map(str::to_owned).ok_or_else(||NativeError::new("invalidSelection","The selected profile name cannot be represented."))
}
fn expected_for_job(j:&JobSnapshot)->Result<(SubmissionKeyV1,ExpectedSelection),NativeError>{
    let root=ConsentFile::parse(&j.body["selection"]["rootFile"])?;let submission=SubmissionKeyV1::parse(&j.input["submission"])?;
    if j.body["cohort"]["complete"]!=true{return Err(busy());}
    let cohort=j.body["cohort"]["files"].members().map(ConsentFile::parse).collect::<Result<Vec<_>,_>>()?;
    let expected=ExpectedSelection{submission_name:j.body["selection"]["submissionName"].as_str().ok_or_else(unknown)?.into(),root,candidate_fingerprint:j.input["consentFingerprint"].as_str().ok_or_else(unknown)?.into(),accept_archived:j.input["acceptArchived"].as_bool().ok_or_else(unknown)?,cohort:Some(cohort),cohort_fingerprint:j.body["cohort"]["fingerprint"].as_str().map(str::to_owned)};
    Ok((submission,expected))
}
#[derive(Clone)]
struct Resolve {submission:JsonValue,phase:&'static str,candidates:Vec<JsonValue>,token:Option<String>,error:JsonValue}
#[derive(Clone)]
struct Start {params:JsonValue,error:Option<NativeError>,admission:bool}
#[derive(Clone)]
struct Control {seq:String,action:String,result:Option<Result<String,NativeError>>}
#[derive(Clone)]
struct CommitReplay {input:JsonValue,result:Option<Result<JsonValue,NativeError>>,admission:bool}
#[derive(Clone)]
struct FrozenCommit {input:RemoteCommitInput,plan:Arc<RemotePublicationPlan>,done:Option<Arc<AtomicBool>>,worker:Arc<Mutex<Option<JoinHandle<PublicationAttempt>>>>,retired:bool,generation:u64}
enum Work { Resolve{key:String,submission:SubmissionKeyV1}, Start{params:JsonValue}, Resume{job:String,action:String}, Control{job:String,seq:String,action:String}, CommitReplay{id:String,input:JsonValue,token:Option<String>}, Prepare{params:JsonValue}, Review{params:JsonValue,generation:u64}, PrepareRelease{job:String,key:String}, ReplacementFact{key:String} }
struct ReplacementFact {method:&'static str,root_spelling:Arc<str>,params:JsonValue,result:JsonValue,expires:Instant,inflight:bool,expired:bool}
#[derive(Clone)]
struct PreparationView {params:JsonValue,prepared:Option<Arc<PreparedRemote>>,review:JsonValue,review_params:Option<JsonValue>,review_control:Option<crate::prepare_control::PrepareControl>,review_expires:Option<Instant>,generation:u64,release:bool,barrier:bool,released:bool,admission:bool}
struct State {
    phase:&'static str,error:JsonValue,worker_exited:bool,reconciled:bool,claims_admitted:bool,
    jobs:BTreeMap<String,JobSnapshot>,progress:BTreeMap<String,JsonValue>,
    resolves:BTreeMap<String,Resolve>,starts:BTreeMap<String,Start>,controls:BTreeMap<String,Control>,commits:BTreeMap<String,CommitReplay>,frozen:BTreeMap<String,FrozenCommit>,recovery_custody:Option<(Arc<RemoteStoreLease>,PreparationSlotLease)>,preparations:BTreeMap<String,PreparationView>,used_prepare_keys:BTreeSet<String>,replacement_facts:BTreeMap<String,ReplacementFact>,queue:VecDeque<Work>,
}
struct Shared {state:Mutex<State>,wake:Condvar,close:AtomicBool,launch_blocked:AtomicBool,startup_blocked:AtomicBool,startup_pending:AtomicBool,startup_done:AtomicBool,recovery_borrowed:Arc<AtomicBool>}
/// Holds the matching source, lease and slot until the recovery owner has a
/// joined worker and authoritative retirement proof.
pub(crate) struct RecoveryBorrow {
    pub(crate) lease:Arc<RemoteStoreLease>,pub(crate) slot:PreparationSlotLease,
    _prepared:Option<Arc<PreparedRemote>>,held:Arc<AtomicBool>,
}
/// Constructed only by the serialized owner after its job and admission
/// checks. The lease pins the admitted root and job store through settlement.
pub(super) struct NoAdmissionProof {
    input: RemoteCommitInput,
    lease: Arc<RemoteStoreLease>,
}
impl NoAdmissionProof {
    pub(super) fn input(&self) -> &RemoteCommitInput { &self.input }
    pub(super) fn lease(&self) -> &RemoteStoreLease { &self.lease }
}
impl Drop for RecoveryBorrow{fn drop(&mut self){self.held.store(false,Ordering::Release);}}
pub(crate) struct RemoteOwner {root:PathBuf,session:SessionId,shared:Arc<Shared>,worker:Option<JoinHandle<()>>}
impl RemoteOwner {
    pub(crate) fn open(root:PathBuf,session:SessionId)->Result<Self,NativeError>{
        Self::open_gated(root,session,false)
    }
    pub(crate) fn open_gated(root:PathBuf,session:SessionId,blocked:bool)->Result<Self,NativeError>{
        let root=paths::absolute(&root)?;if root.to_str().is_none(){return Err(NativeError::params());}
        let shared=Arc::new(Shared{state:Mutex::new(State{phase:if blocked{"waiting"}else{"opening"},error:JsonValue::Null,worker_exited:false,reconciled:false,claims_admitted:false,jobs:BTreeMap::new(),progress:BTreeMap::new(),resolves:BTreeMap::new(),starts:BTreeMap::new(),controls:BTreeMap::new(),commits:BTreeMap::new(),frozen:BTreeMap::new(),recovery_custody:None,preparations:BTreeMap::new(),used_prepare_keys:BTreeSet::new(),replacement_facts:BTreeMap::new(),queue:VecDeque::new()}),wake:Condvar::new(),close:AtomicBool::new(false),launch_blocked:AtomicBool::new(blocked),startup_blocked:AtomicBool::new(blocked),startup_pending:AtomicBool::new(!blocked),startup_done:AtomicBool::new(false),recovery_borrowed:Arc::new(AtomicBool::new(false))});
        let worker_root=root.clone();let worker_session=session.clone();let s=shared.clone();
        let worker=thread::Builder::new().name("native-remote".into()).spawn(move||run(worker_root,worker_session,s)).map_err(|_|NativeError::new("remoteBusy","The remote worker could not start."))?;
        Ok(Self{root,session,shared,worker:Some(worker)})
    }
    pub(crate) fn root(&self)->&Path{&self.root}
    pub(crate) fn session(&self)->&SessionId{&self.session}
    /// Borrow only the matching root/job authority after the publication
    /// thread has actually joined. No new slot or job lease is acquired.
    pub(crate) fn recovery_custody(&self,target:&RemoteCommitInput)->Option<RecoveryBorrow>{
        let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        let (lease,slot,prepared)=if let Some(f)=s.frozen.values().find(|f|f.input.operation_id()==target.operation_id()){
            if !f.retired||store::canonical(f.input.json())!=store::canonical(target.json()){return None;}
            (f.plan.prepared.authority().store_lease,f.plan.prepared.slot(),Some(f.plan.prepared.clone()))
        }else if s.jobs.values().any(|j|j.body["state"]=="committing"&&store::canonical(&j.body["commit"])==store::canonical(target.json())){
            let (lease,slot)=s.recovery_custody.clone()?;(lease,slot,None)
        }else{return None;};
        let held=self.shared.recovery_borrowed.clone();
        if held.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return None;}
        Some(RecoveryBorrow{lease,slot,_prepared:prepared,held})
    }
    pub(crate) fn has_admission(&self)->bool{self.shared.startup_pending.load(Ordering::Acquire)||{let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.starts.values().any(|x|x.admission)||s.preparations.values().any(|x|x.admission)||s.commits.values().any(|x|x.admission)||s.replacement_facts.values().any(|x|x.method=="native.remote.replaceSkip"&&x.result["phase"]=="pending")||!s.frozen.is_empty()||s.jobs.values().any(|j|j.body["state"]=="committing")}}
    pub(crate) fn set_launch_blocked(&self,blocked:bool){
        let _guard=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        self.shared.launch_blocked.store(blocked,Ordering::Release);
        if !self.shared.startup_done.load(Ordering::Acquire){if !blocked{self.shared.startup_pending.store(true,Ordering::Release);}self.shared.startup_blocked.store(blocked,Ordering::Release);self.shared.wake.notify_one();}
    }
    pub(crate) fn is_closed(&mut self)->bool{
        if self.worker.is_none(){return self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).phase=="closed";}
        if !self.worker.as_ref().is_some_and(|h|h.is_finished()){return false;}
        if let Some(h)=self.worker.take(){if h.join().is_err(){return false;}}
        self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).phase=="closed"
    }
    pub(crate) fn begin_close(&mut self){self.shared.close.store(true,Ordering::Release);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if s.phase!="closed"{s.phase="closing";}self.shared.wake.notify_all();}
    /// Memory-only invalidation for a bound review attempt, including invalid
    /// forms. An exact in-flight retry remains the same attempt.
    pub(crate) fn revoke_review_attempt(&self,job:&JobId,params:&JsonValue){
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if s.frozen.contains_key(job.as_str()){return;}
        if !s.jobs.get(job.as_str()).is_some_and(|j|!matches!(j.body["state"].as_str(),Some("committing"|"installed"))){return;}
        let Some(view)=s.preparations.get_mut(job.as_str()).filter(|v|!v.release&&!v.released) else{return;};
        if view.review["phase"]=="pending"&&view.review_params.as_ref()==Some(params){return;}
        if let Some(control)=view.review_control.take(){control.request_cancel();}
        if let Some(next)=view.generation.checked_add(1){view.generation=next;}else{view.release=true;}
        view.review=JsonValue::Null;view.review_params=None;view.review_expires=None;view.admission=false;
    }
    /// Read-only gate classification for the serial NativeService thread.
    pub(crate) fn is_new_gated(&self,method:&str,p:&JsonValue)->bool{
        let s=self.shared.state.lock().unwrap_or_else(|x|x.into_inner());
        match method {
            "native.remote.resolve"=>!s.resolves.contains_key(p["requestKey"].as_str().unwrap_or("")),
            "native.remote.start"|"native.remote.replaceStart"=>!s.starts.contains_key(p["jobId"].as_str().unwrap_or(""))&&!s.jobs.contains_key(p["jobId"].as_str().unwrap_or("")),
            "native.remote.prepare"=>!s.preparations.get(p["jobId"].as_str().unwrap_or("")).is_some_and(|v|v.params==*p&&!v.released),
            "native.remote.review"|"native.remote.replaceReview"=>!s.preparations.get(p["jobId"].as_str().unwrap_or("")).is_some_and(|v|v.review_params.as_ref()==Some(p)&&!v.release&&!v.released),
            "native.remote.replaceTarget"|"native.remote.replaceSkip"=>!s.replacement_facts.contains_key(p["requestKey"].as_str().unwrap_or("")),
            _=>false,
        }
    }
    pub(crate) fn dispatch(&mut self,method:&str,p:&JsonValue)->Result<JsonValue,NativeError>{
        let root=contract::validate_request(method,p)?;if root!=self.root||p["sessionId"]!=self.session.as_str(){return Err(NativeError::params());}
        let root_text=self.root.to_str().unwrap().to_owned();let session_text=self.session.as_str().to_owned();
        let base=||object!{version:1,root:root_text.clone(),sessionId:session_text.clone()};
        if method=="native.remote.close"{self.begin_close();let joined=self.is_closed();let s=self.shared.state.lock().unwrap_or_else(|x|x.into_inner());let mut out=base();out["phase"]=if joined{"closed"}else if s.worker_exited{"waiting"}else{"closing"}.into();out["error"]=JsonValue::Null;return Ok(out);}
        let mut s=self.shared.state.lock().unwrap_or_else(|x|x.into_inner());
        if method=="native.remote.open"{let mut out=base();out["phase"]=match s.phase{"opening"=>"opening","ready"=>"ready","failed"=>"failed",_=>"waiting"}.into();out["error"]=if s.phase=="failed"{s.error.clone()}else{JsonValue::Null};return Ok(out);}
        if self.shared.close.load(Ordering::Acquire){return Err(busy());}
        match method {
            "native.remote.replaceTarget"|"native.remote.replaceSkip"=>{
                let key=p["requestKey"].as_str().unwrap();let now=Instant::now();let root_spelling=p["dataDir"].as_str().unwrap();let mut fact_input=p.clone();
                for field in ["version","dataDir","sessionId","requestKey"]{fact_input.remove(field);}
                for fact in s.replacement_facts.values_mut(){if !fact.inflight&&now>=fact.expires&&(fact.method=="native.remote.replaceTarget"||fact.result["phase"]!="pending"){fact.expired=true;fact.result=JsonValue::Null;}}
                if let Some(old)=s.replacement_facts.get(key){
                    if old.method!=method||old.root_spelling.as_ref()!=root_spelling||old.params!=fact_input{return Err(conflict());}
                    if old.expired{return Err(NativeError::new("staleObservation","This observation expired; observe again with a new request key."));}
                    let result=old.result.clone();let retry=!old.inflight&&result["phase"]=="pending";
                    if retry {if s.queue.len()>=NONCONTROL_QUEUE{return Err(unknown());}s.replacement_facts.get_mut(key).unwrap().inflight=true;s.queue.push_back(Work::ReplacementFact{key:key.into()});self.shared.wake.notify_one();}
                    return Ok(result);
                }
                let pending=s.replacement_facts.values().filter(|f|f.inflight||f.result["phase"]=="pending").count();
                let targets=s.replacement_facts.values().filter(|f|f.method=="native.remote.replaceTarget"&&!f.expired).count();
                if method=="native.remote.replaceSkip"&&(s.commits.values().any(|c|c.admission)||!s.frozen.is_empty()||s.jobs.values().any(|j|j.body["state"]=="committing")){return Err(busy());}
                if s.phase!="ready"||!s.reconciled||s.replacement_facts.len()>=512||pending>=8||s.queue.len()>=NONCONTROL_QUEUE||method=="native.remote.replaceTarget"&&targets>=8{return Err(busy());}
                let mut result=base();result["requestKey"]=key.into();result["phase"]="pending".into();result["error"]=JsonValue::Null;
                if method=="native.remote.replaceTarget"{result["target"]=JsonValue::Null;result["skip"]=JsonValue::Null;}else{result["entry"]=JsonValue::Null;}
                let interned=s.replacement_facts.values().find(|f|f.root_spelling.as_ref()==root_spelling).map(|f|f.root_spelling.clone()).unwrap_or_else(||Arc::from(root_spelling));
                s.replacement_facts.insert(key.into(),ReplacementFact{method:if method=="native.remote.replaceTarget"{"native.remote.replaceTarget"}else{"native.remote.replaceSkip"},root_spelling:interned,params:fact_input,result:result.clone(),expires:now+Duration::from_secs(300),inflight:true,expired:false});
                s.queue.push_back(Work::ReplacementFact{key:key.into()});self.shared.wake.notify_one();Ok(result)
            },
            "native.remote.resolve"=>{
                let key=p["requestKey"].as_str().unwrap();
                if let Some(r)=s.resolves.get(key){if r.submission!=p["submission"]{return Err(conflict());}}
                else{if s.phase!="ready"||s.resolves.len()>=MAX_RESOLVES||s.queue.len()>=NONCONTROL_QUEUE{return Err(busy());}
                    let submission=SubmissionKeyV1::parse(&p["submission"])?;
                    s.resolves.insert(key.into(),Resolve{submission:p["submission"].clone(),phase:"pending",candidates:Vec::new(),token:None,error:JsonValue::Null});
                    s.queue.push_back(Work::Resolve{key:key.into(),submission});self.shared.wake.notify_one();}
                let mut out=base();out["requestKey"]=key.into();out["state"]="pending".into();Ok(out)
            },
            "native.remote.resolveStatus"=>{
                let key=p["requestKey"].as_str().unwrap();let r=s.resolves.get(key).ok_or_else(busy)?;let page=p["page"].as_u32().unwrap() as usize;
                let start=page.checked_mul(8).ok_or_else(NativeError::params)?;let end=start.saturating_add(8).min(r.candidates.len());let items=if start<r.candidates.len(){r.candidates[start..end].to_vec()}else{Vec::new()};
                let mut out=base();out["requestKey"]=key.into();out["phase"]=r.phase.into();out["page"]=p["page"].clone();out["hasMore"]=(end<r.candidates.len()).into();out["selectionToken"]=r.token.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null);out["candidates"]=JsonValue::from(items);out["error"]=r.error.clone();Ok(out)
            },
            "native.remote.resolveRelease"=>{
                let key=p["requestKey"].as_str().unwrap();if s.resolves.get(key).is_some_and(|r|r.phase=="pending"){return Err(busy());}
                let released=s.resolves.remove(key).is_some();let mut out=base();out["requestKey"]=key.into();out["released"]=released.into();Ok(out)
            },
            "native.remote.start"|"native.remote.replaceStart"=>{
                let id=p["jobId"].as_str().unwrap();
                if let Some(j)=s.jobs.get(id){if !same_stored_start(p,j){return Err(conflict());}}
                else if let Some(old)=s.starts.get(id){if !same_start(p,&old.params){return Err(conflict());}if let Some(e)=&old.error{if e.kind!="outcomeUnknown"{let e=e.clone();s.starts.remove(id);return Err(e);}if s.queue.len()>=NONCONTROL_QUEUE{return Err(unknown());}let params=old.params.clone();s.starts.get_mut(id).unwrap().error=None;s.queue.push_back(Work::Start{params});self.shared.wake.notify_one();return Err(unknown());}}
                else{let skip_pending=s.replacement_facts.values().any(|v|v.method=="native.remote.replaceSkip"&&v.result["phase"]=="pending");
                    if s.phase!="ready"||!s.reconciled||s.preparations.values().any(|v|!v.released)||skip_pending&&!(p.has_key("purpose")&&(p["purpose"]=="force"||p["purpose"]=="reinstall"))||s.jobs.values().filter(|j|!job_store::terminal_clean(&j.body)).count()+s.starts.len()>=MAX_JOBS||s.queue.len()>=NONCONTROL_QUEUE{return Err(busy());}
                    s.starts.insert(id.into(),Start{params:p.clone(),error:None,admission:true});s.queue.push_back(Work::Start{params:p.clone()});self.shared.wake.notify_one();}
                let mut out=base();out["jobId"]=id.into();out["state"]="pending".into();Ok(out)
            },
            "native.remote.list"|"native.remote.replaceList"=>{
                if s.phase=="opening"||!s.reconciled&&s.jobs.values().all(|j|job_store::is_tombstone(&j.body)){return Err(busy());}let replacement=method=="native.remote.replaceList";
                let rows=s.jobs.values().filter(|j|!job_store::terminal_clean(&j.body)&&!job_store::is_tombstone(&j.body)&&(j.input["version"]==2)==replacement).map(|j|{let mut v=object!{jobId:j.input["jobId"].clone(),submission:j.input["submission"].clone(),profileId:j.input["profileId"].clone(),state:j.body["state"].clone(),sequence:j.body["sequence"].clone(),cleanupPending:j.body["cleanupPending"].clone()};if replacement{v["purpose"]=j.input["purpose"].clone();v["modId"]=j.input["modId"].clone();v["targetDigest"]=j.input["targetDigest"].clone();}v}).collect::<Vec<_>>();
                let mut out=base();out["jobs"]=JsonValue::from(rows);Ok(out)
            },
            "native.remote.status"|"native.remote.replaceStatus"=>{
                let id=p["jobId"].as_str().unwrap();if s.preparations.get(id).is_some_and(|v|v.review_expires.is_some_and(|deadline|Instant::now()>=deadline)){
                    if let Some(view)=s.preparations.get_mut(id){view.review=JsonValue::Null;view.review_params=None;view.review_expires=None;view.generation=view.generation.saturating_add(1);}}
                if let Some(j)=s.jobs.get(id){if (j.input["version"]==2)!=(method=="native.remote.replaceStatus"){return Err(conflict());}if j.body["claims"].len()>0&&!s.claims_admitted{return Err(ambiguous());}let j=j.clone();let progress=s.progress.get(id).cloned();let review=s.preparations.get(id).map(|p|p.review.clone()).unwrap_or(JsonValue::Null);drop(s);let mut status=job_store::status(&j,&self.root,&self.session,progress)?;
                    if method=="native.remote.replaceStatus"{return Ok(object!{version:1,root:root_text.clone(),sessionId:session_text.clone(),purpose:j.input["purpose"].clone(),modId:j.input["modId"].clone(),targetDigest:j.input["targetDigest"].clone(),status:status,review:review});}
                    status["review"]=review;return Ok(contract::RemoteStatusV1::parse(status)?.json());}
                if let Some(start)=s.starts.get(id){if start.params.has_key("purpose")!=(method=="native.remote.replaceStatus"){return Err(conflict());}if let Some(e)=&start.error{let e=e.clone();if e.kind!="outcomeUnknown"{s.starts.remove(id);}return Err(e);}return Err(busy());}
                if !s.reconciled||s.phase!="ready"{return Err(busy());}Err(NativeError::new("jobAbsent","No retained remote job has this identity."))
            },
            "native.remote.control"=>{
                let id=p["jobId"].as_str().unwrap();let seq=p["controlSeq"].as_str().unwrap();let action=p["action"].as_str().unwrap();
                if let Some(j)=s.jobs.get(id){let last=&j.body["lastControl"];if !last.is_null(){let n=store::revision(seq)?;let previous=store::revision(last["controlSeq"].as_str().unwrap())?;
                    if n==previous{if last["action"]!=action{return Err(conflict());}let mut out=base();out["jobId"]=id.into();out["controlSeq"]=seq.into();out["state"]=last["result"].clone();return Ok(out);}
                    if n<previous{return Err(NativeError::new("controlReceiptExpired","This control receipt is no longer retained."));}}if job_store::is_tombstone(&j.body){return Err(NativeError::new("controlReceiptExpired","This control receipt is no longer retained."));}}
                else if s.reconciled&&!s.starts.contains_key(id){return Err(NativeError::new("controlReceiptExpired","No control receipt is retained for this identity."));}else{return Err(busy());}
                if matches!(action,"resume"|"retry"|"dismiss")&&!s.reconciled{return Err(busy());}
                if let Some(c)=s.controls.get(id){if c.seq==seq {if c.action!=action{return Err(conflict());}if let Some(result)=&c.result{if result.as_ref().is_err_and(|e|e.kind=="outcomeUnknown"){if s.queue.len()>=MAX_QUEUE{return Err(unknown());}s.controls.get_mut(id).unwrap().result=None;s.queue.push_front(Work::Control{job:id.into(),seq:seq.into(),action:action.into()});self.shared.wake.notify_one();return Err(unknown());}let state=result.clone()?;let mut out=base();out["jobId"]=id.into();out["controlSeq"]=seq.into();out["state"]=state.into();return Ok(out);}return Err(busy());}if c.result.is_none()||c.result.as_ref().is_some_and(|r|r.as_ref().is_err_and(|e|e.kind=="outcomeUnknown")){return Err(busy());}}
                if s.queue.len()>=MAX_QUEUE{return Err(busy());}s.controls.insert(id.into(),Control{seq:seq.into(),action:action.into(),result:None});s.queue.push_front(Work::Control{job:id.into(),seq:seq.into(),action:action.into()});self.shared.wake.notify_one();Err(busy())
            },
            "native.remote.commit"|"native.remote.replaceCommit"=>{
                let id=p["operationId"].as_str().unwrap();let replacement=method=="native.remote.replaceCommit";
                let input=if replacement{super::replacement_contract::ReplacementCommitInputV1::from_params(p)?.json().clone()}else{object!{method:"native.remote.commit",params:object!{operationId:p["operationId"].clone(),jobId:p["jobId"].clone(),profileId:p["profileId"].clone(),expectedRevision:p["expectedRevision"].clone(),consentFingerprint:p["consentFingerprint"].clone(),cohortFingerprint:p["cohortFingerprint"].clone(),selectionDigest:p["selectionDigest"].clone()}}};
                let job=p["jobId"].as_str().unwrap();
                if s.jobs.get(job).is_some_and(|j|(j.input["version"]==2)!=replacement)||s.starts.get(job).is_some_and(|j|j.params.has_key("purpose")!=replacement){return Err(conflict());}
                if let Some(old)=s.commits.get(id){if store::canonical(&old.input)!=store::canonical(&input){return Err(NativeError::new("operationIdConflict","This operation ID belongs to different input."));}if let Some(result)=old.result.clone(){s.commits.remove(id);return result;}return Err(unknown());}
                if s.commits.values().any(|c| c.admission && c.input["params"]["jobId"] == input["params"]["jobId"]) {
                    return Err(busy());
                }
                if s.commits.len() >= MAX_JOBS || s.queue.len() >= NONCONTROL_QUEUE { return Err(busy()); }
                let token = p["reviewToken"].as_str().map(str::to_owned);
                // Even null-token replay reserves admission until it has either
                // replayed or declined settlement. This linearizes a possible
                // history write against launch admission on the service thread.
                s.commits.insert(id.into(), CommitReplay { input: input.clone(), result: None, admission: true });
                s.queue.push_back(Work::CommitReplay { id: id.into(), input, token });
                self.shared.wake.notify_one();
                Err(unknown())
            },
            "native.remote.prepare"=>{
                let id=p["jobId"].as_str().unwrap();let key=p["prepareKey"].as_str().unwrap();
                if s.commits.values().any(|c|c.admission&&c.input["params"]["jobId"]==id){return Err(busy());}
                if let Some(view)=s.preparations.get(id){if !view.released {if view.params!=*p{return Err(conflict());}let mut out=base();out["jobId"]=id.into();out["prepareKey"]=key.into();out["state"]="pending".into();return Ok(out);}}
                if s.phase!="ready"||!s.reconciled||s.queue.len()>=NONCONTROL_QUEUE||s.starts.values().any(|v|v.admission)||s.preparations.values().any(|v|!v.released){return Err(busy());}
                let job=s.jobs.get(id).ok_or_else(busy)?;if job.body["state"]!="downloaded"||job.input["profileId"]!=p["profileId"]{return Err(busy());}
                if s.used_prepare_keys.contains(key){return Err(NativeError::new("staleReview","This preparation key was already retired."));}if s.used_prepare_keys.len()>=1024{return Err(busy());}s.used_prepare_keys.insert(key.into());
                s.preparations.insert(id.into(),PreparationView{params:p.clone(),prepared:None,review:JsonValue::Null,review_params:None,review_control:None,review_expires:None,generation:0,release:false,barrier:false,released:false,admission:true});
                s.queue.push_back(Work::Prepare{params:p.clone()});self.shared.wake.notify_one();let mut out=base();out["jobId"]=id.into();out["prepareKey"]=key.into();out["state"]="pending".into();Ok(out)
            },
            "native.remote.candidates"|"native.remote.candidateMembers"|"native.remote.diagnostics"=>{
                let id=p["jobId"].as_str().unwrap();let key=p["prepareKey"].as_str().unwrap();
                let prepared=s.preparations.get(id).filter(|v|!v.release&&!v.released&&v.params["prepareKey"]==key).and_then(|v|v.prepared.clone()).ok_or_else(busy)?;drop(s);
                let page=p["page"].as_u32().unwrap();let mut out=base();out["jobId"]=id.into();out["prepareKey"]=key.into();out["page"]=page.into();
                match method {
                    "native.remote.candidates"=>{let result=prepared.candidate_page(page)?;out["hasMore"]=result.has_more.into();out["candidates"]=JsonValue::from(result.candidates);},
                    "native.remote.candidateMembers"=>{let candidate=CandidateId::parse(p["candidateId"].as_str().unwrap())?;let result=prepared.member_page(&candidate,page)?;out["candidateId"]=candidate.as_str().into();out["hasMore"]=result.has_more.into();out["members"]=JsonValue::from(result.members);},
                    _=>{let result=prepared.diagnostic_page(page)?;out["hasMore"]=result.has_more.into();out["diagnostics"]=JsonValue::from(result.diagnostics);},
                }Ok(out)
            },
            "native.remote.review"|"native.remote.replaceReview"=>{
                let id=p["jobId"].as_str().unwrap();let key=p["requestKey"].as_str().unwrap();
                if s.jobs.get(id).is_some_and(|j|(j.input["version"]==2)!=(method=="native.remote.replaceReview")){return Err(conflict());}
                if s.jobs.get(id).is_some_and(|j|j.input["purpose"]=="update")&&s.replacement_facts.values().any(|f|f.method=="native.remote.replaceSkip"&&f.result["phase"]=="pending"){return Err(busy());}
                if s.commits.values().any(|c|c.admission&&c.input["params"]["jobId"]==id){return Err(busy());}
                if s.frozen.contains_key(id){return Err(NativeError::new("cancelTooLate","The operation is already committing."));}
                if s.queue.len()>=NONCONTROL_QUEUE{return Err(busy());}
                let view=s.preparations.get_mut(id).filter(|v|!v.release&&!v.released&&v.prepared.is_some()).ok_or_else(busy)?;
                if let Some(old)=&view.review_params{if old["requestKey"]==key{if old!=p{return Err(conflict());}let mut out=base();out["jobId"]=id.into();out["requestKey"]=key.into();out["state"]="pending".into();return Ok(out);}}
                view.review=if method=="native.remote.replaceReview"{object!{requestKey:key,phase:"pending",reviewToken:JsonValue::Null,selectionDigest:JsonValue::Null,afterRecordDigest:JsonValue::Null,summary:JsonValue::Null,error:JsonValue::Null}}else{object!{requestKey:key,phase:"pending",reviewToken:JsonValue::Null,selectionDigest:JsonValue::Null,modId:JsonValue::Null,summary:JsonValue::Null,error:JsonValue::Null}};view.review_params=Some(p.clone());view.admission=true;let generation=view.generation;
                s.queue.push_back(Work::Review{params:p.clone(),generation});self.shared.wake.notify_one();let mut out=base();out["jobId"]=id.into();out["requestKey"]=key.into();out["state"]="pending".into();Ok(out)
            },
            "native.remote.prepareRelease"=>{
                let id=p["jobId"].as_str().unwrap();let key=p["prepareKey"].as_str().unwrap();
                if s.frozen.contains_key(id)||s.jobs.get(id).is_some_and(|j|j.body["state"]=="committing"){return Err(NativeError::new("cancelTooLate","The operation is already committing."));}
                let queue_full=s.queue.len()>=MAX_QUEUE;
                let Some(view)=s.preparations.get_mut(id).filter(|v|v.params["prepareKey"]==key) else {let mut out=base();out["jobId"]=id.into();out["prepareKey"]=key.into();out["state"]="stale".into();return Ok(out);};
                let state=if view.released{"released"}else{"pending"};let queue=!view.release&&!view.released;if queue&&queue_full{return Err(busy());}view.release=true;view.review=JsonValue::Null;view.review_params=None;
                if queue {s.queue.push_front(Work::PrepareRelease{job:id.into(),key:key.into()});self.shared.wake.notify_one();}
                let mut out=base();out["jobId"]=id.into();out["prepareKey"]=key.into();out["state"]=state.into();Ok(out)
            },
            _=>Err(NativeError::new("methodNotFound","Unknown remote method.")),
        }
    }
}
impl Drop for RemoteOwner {fn drop(&mut self){self.begin_close();if let Some(h)=self.worker.take(){let _=h.join();}}}

enum FetchPurpose {Resolve{key:String,submission:SubmissionKeyV1},Start{params:JsonValue,submission:SubmissionKeyV1,expected:ExpectedSelection},Resume{job:String,action:String,submission:SubmissionKeyV1,expected:ExpectedSelection},Prepare{params:JsonValue,submission:SubmissionKeyV1,expected:ExpectedSelection},Review{params:JsonValue,generation:u64,submission:SubmissionKeyV1,expected:ExpectedSelection}}
struct Fetch {purpose:FetchPurpose,route:usize,active:Option<String>,responses:Vec<RawResponse>}
struct Worker {
    root:PathBuf,root_handle:Option<File>,root_identity:Option<(u64,u64)>,session:SessionId,shared:Arc<Shared>,store:Option<RemoteJobStore>,metadata:Option<RemoteMetadata>,
    metadata_ticket:Option<Arc<AtomicBool>>,transfer_ticket:Option<Arc<AtomicBool>>,authorizations:BTreeMap<String,FreshAuthorization>,job_authorizations:BTreeMap<String,FreshAuthorization>,authorization_prepares:BTreeMap<String,String>,attempt_modes:BTreeMap<String,String>,fetch:Option<Fetch>,active:Option<ActiveTransfer>,hashing:Option<HashWorker>,discovery:Option<DiscoveryWorker>,resume_hash:Option<ResumeWorker>,
    reconcile_slot:Option<PreparationSlotLease>,preparation:Option<PreparationWorker>,preparation_guard:Option<PreparationRetirement>,ready:BTreeMap<String,Arc<PreparedRemote>>,review_worker:Option<ReviewWorker>,review_guard:Option<ReviewRetirement>,retiring_preparation:Option<PreparationRetirement>,plans:BTreeMap<String,(u64,RemotePublicationPlan)>,deferred_prepare:Option<JsonValue>,deferred_review:Option<(JsonValue,u64)>,commit_worker:Option<CommitWorker>,pending_skips:BTreeMap<String,(Arc<RemoteStoreLease>,replacement_skips::PendingWrite)>,
}
struct CommitWorker {input:RemoteCommitInput,plan:Arc<RemotePublicationPlan>,generation:u64,control:crate::prepare_control::PrepareControl,
    validation:Option<JoinHandle<Result<(),NativeError>>>,publication:Option<Arc<Mutex<Option<JoinHandle<PublicationAttempt>>>>>,frozen:bool,reserved:bool,dispatched:bool}
enum PublicationAttempt { LockBusy, Finished(Result<JsonValue,NativeError>) }
struct PreparationWorker {job:String,key:String,control:crate::prepare_control::PrepareControl,slot:PreparationSlotLease,authority:RemoteQuarantineAuthority,handle:Option<JoinHandle<()>>,result:Option<Result<PreparedRemote,NativeError>>}
struct ReviewWorker {job:String,key:String,request:String,generation:u64,control:crate::prepare_control::PrepareControl,handle:JoinHandle<()>}
struct PreparationRetirement {job:String,key:String,slot:PreparationSlotLease,authority:RemoteQuarantineAuthority,control:crate::prepare_control::PrepareControl,done:Arc<AtomicBool>,prepared:Option<Weak<PreparedRemote>>,result:Arc<Mutex<Option<Result<PreparedRemote,NativeError>>>>}
struct ReviewRetirement {job:String,key:String,control:crate::prepare_control::PrepareControl,done:Arc<AtomicBool>,result:Arc<Mutex<Option<Result<RemotePublicationPlan,NativeError>>>>}
struct PreparationDone(Arc<AtomicBool>);
impl Drop for PreparationDone {fn drop(&mut self){self.0.store(true,Ordering::Release);}}
impl Worker {
    fn replacement_fact(&mut self,key:&str){
        let (method,params)={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let Some(f)=s.replacement_facts.get(key).filter(|f|f.inflight&&!f.expired) else{return;};(f.method,f.params.clone())};
        let result=(||->Result<Option<JsonValue>,NativeError>{
            self.validate_root()?;
            let lease=if let Some((lease,_))=self.pending_skips.get(key){lease.clone()}
                else if let Some(db)=&self.store{db.lease()}
                else if let Some((lease,_))=self.pending_skips.values().next(){lease.clone()}
                else{Arc::new(RemoteStoreLease::acquire(&self.root)?)};
            if Some(lease.identity())!=self.root_identity{return Err(ambiguous());}
            let lock=RootLock::acquire(&self.root)?;lease.validate()?;lock.validate()?;
            let profile=params["profileId"].as_str().ok_or_else(NativeError::params)?;
            let mod_id=params["modId"].as_str().ok_or_else(NativeError::params)?;
            if method=="native.remote.replaceTarget"{
                let revision=store::revision(params["expectedRevision"].as_str().ok_or_else(NativeError::params)?)?;
                let target=replacement_target::read(&self.root,profile,mod_id,revision)?;
                let uncertain={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());self.pending_skips.keys().any(|key|s.replacement_facts.get(key).is_some_and(|f|f.params["profileId"]==profile&&f.params["modId"]==mod_id))};
                let skip=if uncertain{object!{state:"unavailable",entry:JsonValue::Null}}else{replacement_skips::observe(&lease,&lock,profile,mod_id,&target.digest)};
                return Ok(Some(object!{target:target.public,skip:skip}));
            }
            if let Some((_,pending))=self.pending_skips.get_mut(key){
                let observed=replacement_skips::reconcile(&lease,&lock,pending,profile,mod_id)?;
                if let Some(value)=observed{self.pending_skips.remove(key);return Ok(Some(object!{entry:value}));}
                return Ok(None);
            }
            let fresh=params["selectionToken"].as_str().and_then(|token|self.authorizations.get(token));
            match replacement_skips::compare_set(&lease,&lock,&params,&self.session,fresh)?{
                replacement_skips::WriteResult::Done(value)=>Ok(Some(object!{entry:value})),
                replacement_skips::WriteResult::Uncertain(pending)=>{self.pending_skips.insert(key.into(),(lease.clone(),pending));Ok(None)},
            }
        })();
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let Some(f)=s.replacement_facts.get_mut(key).filter(|f|f.method==method&&f.params==params) else{return;};
        f.inflight=false;
        match result {
            Ok(Some(value))=>{f.result["phase"]="ready".into();for (k,v) in value.entries(){f.result[k]=v.clone();}f.result["error"]=JsonValue::Null;},
            Ok(None)=>{},
            Err(e) if self.pending_skips.contains_key(key)=>{let _=e;},
            Err(e) if e.kind=="nativeBusy"||e.kind=="remoteBusy"=>{},
            Err(e)=>{f.result["phase"]="failed".into();f.result["error"]=failure(&e);},
        }
    }
    fn retry_prepare(&mut self,params:JsonValue){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let id=params["jobId"].as_str().unwrap_or("");if !s.preparations.get(id).is_some_and(|v|!v.release&&!v.released&&v.params==params){return;}if s.queue.len()<NONCONTROL_QUEUE{s.queue.push_back(Work::Prepare{params});}else{self.deferred_prepare=Some(params);}}
    fn retry_review(&mut self,params:JsonValue,generation:u64){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let id=params["jobId"].as_str().unwrap_or("");if !s.preparations.get(id).is_some_and(|v|!v.release&&!v.released&&v.generation==generation&&v.review_params.as_ref()==Some(&params)){return;}if s.queue.len()<NONCONTROL_QUEUE{s.queue.push_back(Work::Review{params,generation});}else{self.deferred_review=Some((params,generation));}}
    /// Task 5 must call this gate under its publication admission before taking
    /// a plan; a stale map entry never restores a revoked or expired grant.
    fn take_live_plan(&mut self,id:&str,token:&str)->Result<RemotePublicationPlan,NativeError>{
        let generation=self.plans.get(id).map(|(generation,_)|*generation).ok_or_else(busy)?;
        let valid={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(id).is_some_and(|view|
            !view.release&&!view.released&&view.generation==generation&&view.review["phase"]=="ready"&&view.review["reviewToken"]==token
                &&view.review_expires.is_some_and(|expiry|Instant::now()<expiry))};
        if !valid{self.plans.remove(id);return Err(NativeError::new("staleReview","The review grant is no longer live."));}
        self.plans.remove(id).map(|(_,plan)|plan).ok_or_else(busy)
    }
    fn initialize(&mut self)->Result<(),NativeError>{
        // A library is read before any private namespace can be created.
        paths::reject_symlinks(&self.root)?;let handle=OpenOptions::new().read(true).custom_flags(0x10000|0x20000|0x800).open(&self.root)?;let meta=handle.metadata()?;
        if !meta.is_dir()||meta.uid()!=unsafe{geteuid()}{return Err(ambiguous());}self.root_identity=Some(identity(&meta));self.root_handle=Some(handle);self.validate_root()?;
        Store::open(&self.root)?;self.validate_root()?;
        self.store=RemoteJobStore::open_existing(&self.root)?;
        if self.store.as_ref().is_some_and(|db|Some(db.lease().identity())!=self.root_identity){return Err(ambiguous());}self.validate_root()?;
        let (waiting,jobs)=if let Some(db)=&mut self.store {let result=db.reconcile()?;(result.waiting,result.jobs)} else {(!job_store::empty_namespace(&self.root)?,Vec::new())};
        let claims_admitted=self.store.as_ref().is_none_or(|db|verify_claim_set(db).is_ok());
        // Observe the exact central operation and journal before treating any
        // interrupted commit as reconcilable. A failed read never invents an outcome.
        for j in &jobs {if j.body["state"]=="committing"{let _=self.observe_committing(j);}}
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        for j in jobs{s.jobs.insert(j.input["jobId"].as_str().ok_or_else(unknown)?.to_owned(),j);}
        s.claims_admitted=claims_admitted;s.reconciled=!waiting&&claims_admitted;s.phase=if waiting||!claims_admitted{"waiting"}else{"ready"};Ok(())
    }
    fn validate_root(&self)->Result<(),NativeError>{
        paths::reject_symlinks(&self.root).map_err(|_|ambiguous())?;let held=self.root_handle.as_ref().ok_or_else(ambiguous)?.metadata().map_err(|_|ambiguous())?;let named=std::fs::symlink_metadata(&self.root).map_err(|_|ambiguous())?;
        if !held.is_dir()||!named.is_dir()||Some(identity(&held))!=self.root_identity||Some(identity(&named))!=self.root_identity{return Err(ambiguous());}Ok(())
    }
    fn read_anchor(&self)->Result<File,NativeError>{
        paths::reject_symlinks(&self.root).map_err(|_|unknown())?;
        let f=if let Some(held)=&self.root_handle{held.try_clone().map_err(|_|unknown())?}else{OpenOptions::new().read(true).custom_flags(0x10000|0x20000|0x800).open(&self.root).map_err(|_|unknown())?};
        self.validate_anchor(&f)?;Ok(f)
    }
    fn validate_anchor(&self,held:&File)->Result<(),NativeError>{
        paths::reject_symlinks(&self.root).map_err(|_|unknown())?;let a=held.metadata().map_err(|_|unknown())?;let b=std::fs::symlink_metadata(&self.root).map_err(|_|unknown())?;
        if !a.is_dir()||!b.is_dir()||a.uid()!=unsafe{geteuid()}||identity(&a)!=identity(&b)||self.root_identity.is_some_and(|id|id!=identity(&a)){return Err(unknown());}Ok(())
    }
    fn observe_committing(&self,j:&JobSnapshot)->Result<(),NativeError>{
        let input=&j.body["commit"];let id=input["params"]["operationId"].as_str().ok_or_else(unknown)?;let history=Store::open_history(&self.root).map_err(|_|unknown())?;
        let replay=history.replay(id,input);let journal=history.journal(id).map_err(|_|unknown())?;
        if let Some(journal)=journal{if journal["operationId"]!=id||!journal.has_key("input")||store::canonical(&journal["input"])!=store::canonical(input){return Err(unknown());}}
        if replay.as_ref().is_err_and(|e|e.kind=="operationIdConflict"){return Err(unknown());}Ok(())
    }
    fn ensure_metadata(&mut self)->Result<(),NativeError>{
        self.validate_root()?;Store::open(&self.root)?;self.validate_root()?;
        if self.metadata.is_none(){let m=RemoteMetadata::new(self.root.clone(),self.session.clone())?;self.metadata_ticket=Some(m.retirement_ticket());self.metadata=Some(m);if let Err(e)=self.validate_root(){if let Some(m)=self.metadata.as_mut(){m.begin_close();}return Err(e);}}Ok(())
    }
    fn begin_fetch(&mut self,purpose:FetchPurpose)->Result<(),NativeError>{self.ensure_metadata()?;self.fetch=Some(Fetch{purpose,route:0,active:None,responses:Vec::new()});Ok(())}
    fn start_work(&mut self,work:Work){match work {
        Work::ReplacementFact{key}=>self.replacement_fact(&key),
        Work::Resolve{key,submission}=>{if self.fetch.is_some(){self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).queue.push_back(Work::Resolve{key,submission});return;}
            if let Err(e)=self.begin_fetch(FetchPurpose::Resolve{key:key.clone(),submission}){self.resolve_failed(&key,&e);}},
        Work::Start{params}=>{if self.fetch.is_some(){self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).queue.push_back(Work::Start{params});return;}
            if let Some(db)=self.store.as_ref(){let id=params["jobId"].as_str().unwrap();match JobId::parse(id).and_then(|id|db.get(&id)){
                Ok(Some(mut j))=>{if !same_stored_start(&params,&j){self.start_failed(id,conflict());return;}
                    if j.body["state"]=="resolving"&&!self.job_authorizations.contains_key(id){j.body["state"]="paused".into();let result=JobId::parse(id).and_then(|job|self.store.as_mut().ok_or_else(unknown)?.transition(&job,JobTransition::Body(j.body)));match result{Ok(next)=>j=next,Err(_)=>{self.start_failed(id,unknown());return;}}}
                    let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.jobs.insert(id.into(),j);s.starts.remove(id);return;},
                Err(_)=>{self.start_failed(id,unknown());return;},Ok(None)=>{},
            }}
            let result=(||{let token=params["selectionToken"].as_str().ok_or_else(NativeError::params)?;let a=self.authorizations.get(token).filter(|a|a.valid_at(Instant::now())).ok_or_else(||NativeError::new("needsSelection","Resolve fresh files before starting."))?;
                let file=a.consent(params["fileId"].as_str().unwrap()).ok_or_else(||NativeError::new("needsSelection","The selected file is no longer available."))?.clone();
                if contract::candidate_fingerprint(a.submission(),a.submission_name(),&file)!=params["candidateFingerprint"].as_str().unwrap(){return Err(conflict());}
                let submission=a.submission().clone();let expected=ExpectedSelection{submission_name:a.submission_name().into(),root:file,candidate_fingerprint:params["candidateFingerprint"].as_str().unwrap().into(),accept_archived:params["acceptArchived"].as_bool().unwrap(),cohort:None,cohort_fingerprint:None};
                self.begin_fetch(FetchPurpose::Start{params:params.clone(),submission,expected})})();
            if let Err(e)=result{self.start_failed(params["jobId"].as_str().unwrap(),e);}},
        Work::Resume{job,action}=>{if self.fetch.is_some(){self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).queue.push_back(Work::Resume{job,action});return;}
            let result=(||{let db=self.store.as_ref().ok_or_else(unknown)?;let j=db.get(&JobId::parse(&job)?)?.ok_or_else(unknown)?;
                let root=ConsentFile::parse(&j.body["selection"]["rootFile"])?;let submission=SubmissionKeyV1::parse(&j.input["submission"])?;
                let complete=j.body["cohort"]["complete"]==true;let cohort=if complete{Some(j.body["cohort"]["files"].members().map(ConsentFile::parse).collect::<Result<Vec<_>,_>>()?)}else{None};
                let expected=ExpectedSelection{submission_name:j.body["selection"]["submissionName"].as_str().ok_or_else(unknown)?.into(),root,candidate_fingerprint:j.input["consentFingerprint"].as_str().ok_or_else(unknown)?.into(),accept_archived:j.input["acceptArchived"].as_bool().ok_or_else(unknown)?,cohort,cohort_fingerprint:if complete{j.body["cohort"]["fingerprint"].as_str().map(str::to_owned)}else{None}};
                self.begin_fetch(FetchPurpose::Resume{job:job.clone(),action,submission,expected})})();if let Err(e)=result{self.mark_failed(&job,e);}},
        Work::Control{job,seq,action}=>{let result=self.control(&job,&seq,&action);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(c)=s.controls.get_mut(&job).filter(|c|c.seq==seq&&c.action==action){c.result=Some(result);}},
        Work::Prepare{params}=>{
            let id=params["jobId"].as_str().unwrap().to_owned();
            if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(&id).is_some_and(|v|v.params==params&&!v.release&&!v.released){return;}
            if self.authorization_prepares.get(&id).is_some_and(|key|params["prepareKey"]==*key)&&self.job_authorizations.get(&id).is_some_and(|a|a.valid_at(Instant::now())){
                match self.begin_preparation(&params){Ok(())=>{},Err(e) if e.kind=="nativeBusy"||e.kind=="remoteBusy"=>self.retry_prepare(params),Err(e)=>self.fail_preparation(&id,params["prepareKey"].as_str().unwrap(),&e)}
            }else{
                if self.fetch.is_some(){self.retry_prepare(params);return;}
                let result=(||{let j=self.store.as_ref().ok_or_else(unknown)?.get(&JobId::parse(&id)?)?.ok_or_else(unknown)?;let (submission,expected)=expected_for_job(&j)?;self.begin_fetch(FetchPurpose::Prepare{params:params.clone(),submission,expected})})();
                if let Err(e)=result{self.fail_preparation(&id,params["prepareKey"].as_str().unwrap(),&e);}
            }
        },
        Work::Review{params,generation}=>{
            let id=params["jobId"].as_str().unwrap().to_owned();
            if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(&id).is_some_and(|v|v.review_params.as_ref()==Some(&params)&&v.generation==generation&&!v.release&&!v.released){return;}
            self.plans.remove(&id);
            let prepare_key={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(&id).and_then(|v|v.params["prepareKey"].as_str().map(str::to_owned))};
            if self.authorization_prepares.get(&id)==prepare_key.as_ref()&&self.job_authorizations.get(&id).is_some_and(|a|a.valid_at(Instant::now())){
                match self.begin_review(&params,generation){Ok(())=>{},Err(e) if e.kind=="nativeBusy"||e.kind=="remoteBusy"=>self.retry_review(params,generation),Err(e)=>self.fail_review(&id,&params,generation,&e)}
            }else{
                if self.fetch.is_some(){self.retry_review(params,generation);return;}
                let result=(||{let j=self.store.as_ref().ok_or_else(unknown)?.get(&JobId::parse(&id)?)?.ok_or_else(unknown)?;let (submission,expected)=expected_for_job(&j)?;self.begin_fetch(FetchPurpose::Review{params:params.clone(),generation,submission,expected})})();
                if let Err(e)=result{self.fail_review(&id,&params,generation,&e);}
            }
        },
        Work::PrepareRelease{job,key}=>self.begin_release(&job,&key),
        Work::CommitReplay{id,input,token}=>self.begin_commit(&id,input,token),
    }}
    fn commit_reply(&self,input:&RemoteCommitInput,result:Result<JsonValue,NativeError>){
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if let Some(r)=s.commits.get_mut(input.operation_id()).filter(|r|store::canonical(&r.input)==store::canonical(input.json())){r.admission=false;r.result=Some(result);}
    }
    fn pending_commit(&self,input:&RemoteCommitInput)->JsonValue{object!{version:1,root:self.root.to_str(),sessionId:self.session.as_str(),jobId:input.params()["jobId"].clone(),operationId:input.operation_id(),state:"pending"}}
    fn authorization_matches(&self,job:&str,row:&JobSnapshot)->Result<(),NativeError>{
        let auth=self.job_authorizations.get(job).filter(|a|a.valid_at(Instant::now())).ok_or_else(||NativeError::new("staleReview","Fresh authorization expired."))?;
        if auth.submission().json()!=row.input["submission"]||auth.submission_name()!=row.body["selection"]["submissionName"].as_str().ok_or_else(unknown)?
            ||auth.selected_root()!=row.input["rootFileId"].as_str()||auth.cohort_fingerprint()!=row.body["cohort"]["fingerprint"].as_str(){return Err(unknown());}
        for item in row.body["cohort"]["files"].members(){let file=ConsentFile::parse(item)?;if auth.consent(&file.file_id)!=Some(&file){return Err(unknown());}}Ok(())
    }
    fn no_admission_proof(&self, input: &RemoteCommitInput) -> Result<NoAdmissionProof, NativeError> {
        let job = input.params()["jobId"].as_str().ok_or_else(unknown)?;
        if self.commit_worker.is_some() { return Err(unknown()); }

        let db = self.store.as_ref().ok_or_else(unknown)?;
        let lease = db.lease();
        lease.validate().map_err(|_| unknown())?;
        let row = db.get(&JobId::parse(job)?).map_err(|_| unknown())?.ok_or_else(unknown)?;
        verify_claim_set(db).map_err(|_| unknown())?;
        match input {
            RemoteCommitInput::Add(_) if row.input["version"]==1=>{},
            RemoteCommitInput::Replace(_) if row.input["version"]==2&&row.input["purpose"]==input.params()["purpose"]
                &&row.input["modId"]==input.params()["modId"]&&row.input["targetDigest"]==input.params()["targetDigest"]
                &&row.input["initialRevision"]==input.params()["initialRevision"]=>{},
            _=>return Err(unknown()),
        }
        if !row.body["commit"].is_null()
            || row.input["jobId"] != input.params()["jobId"]
            || row.input["profileId"] != input.params()["profileId"]
            || row.input["consentFingerprint"] != input.params()["consentFingerprint"]
            || row.body["cohort"]["complete"] != true
            || row.body["cohort"]["fingerprint"] != input.params()["cohortFingerprint"]
        {
            return Err(unknown());
        }

        {
            let mut s = self.shared.state.lock().unwrap_or_else(|p| p.into_inner());
            let matching_fence = s.commits.get(input.operation_id()).is_some_and(|c| {
                c.admission && store::canonical(&c.input) == store::canonical(input.json())
            });
            let competing_commit = s.commits.iter().any(|(id, c)| {
                id != input.operation_id() && c.admission && c.input["params"]["jobId"] == job
            });
            if self.shared.close.load(Ordering::Acquire)
                || self.shared.launch_blocked.load(Ordering::Acquire)
                || s.phase != "ready"
                || !s.reconciled
                || !s.claims_admitted
                || s.frozen.contains_key(job)
                || s.starts.get(job).is_some_and(|x| x.admission)
                || s.preparations.get(job).is_some_and(|v| v.admission)
                || competing_commit
                || !matching_fence
            {
                return Err(unknown());
            }
            // Retire any live grant before releasing the fence. A subsequent
            // publication requires a fresh explicit review.
            if let Some(view) = s.preparations.get_mut(job) {
                if let Some(control) = view.review_control.take() { control.request_cancel(); }
                view.review = JsonValue::Null;
                view.review_params = None;
                view.review_expires = None;
                view.generation = view.generation.checked_add(1).ok_or_else(unknown)?;
            }
        }

        lease.validate().map_err(|_| unknown())?;
        Ok(NoAdmissionProof { input: input.clone(), lease })
    }
    fn begin_commit(&mut self,id:&str,raw:JsonValue,token:Option<String>){
        let input=match RemoteCommitInput::parse(raw){Ok(input)=>input,Err(e)=>{let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(r)=s.commits.get_mut(id){r.admission=false;r.result=Some(Err(e));}return;}};
        // The central operation and matching journal are always examined before
        // a transient job, token or in-memory frozen plan.
        let replay=(||->Result<Option<JsonValue>,NativeError>{let anchor=self.read_anchor()?;let history=Store::open_history(&self.root)?;let result=history.replay(id,input.json());self.validate_anchor(&anchor)?;if let Some(value)=result?{return Ok(Some(value));}
            let journal=history.journal(id)?;self.validate_anchor(&anchor)?;if journal.is_some(){return Err(NativeError::new("recoveryRequired","The remote operation has an unresolved journal."));}Ok(None)})();
        match replay {Ok(Some(value))=>{
            let matching=(||->Result<(),NativeError>{if let Some(db)=&self.store{
                if let Some(row)=db.get(&JobId::parse(input.params()["jobId"].as_str().ok_or_else(unknown)?)?).map_err(|_|unknown())?{
                    if (row.input["version"]==2)!=matches!(&input,RemoteCommitInput::Replace(_)){return Err(conflict());}
                }}Ok(())})();
            self.commit_reply(&input,matching.map(|_|value));return;
        },Err(e)=>{self.commit_reply(&input,Err(e));return;},Ok(None)=>{}}
        if matches!(&input,RemoteCommitInput::Replace(v) if v.params()["purpose"]=="update"){
            let pending=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).replacement_facts.values().any(|f|f.method=="native.remote.replaceSkip"&&f.result["phase"]=="pending");
            if pending{self.commit_reply(&input,Err(busy()));return;}
        }
        let job=input.params()["jobId"].as_str().unwrap();
        if let Some(worker)=&self.commit_worker {if store::canonical(worker.input.json())==store::canonical(input.json())&&worker.reserved{self.commit_reply(&input,Ok(self.pending_commit(&input)));}else{self.commit_reply(&input,Err(unknown()));}return;}
        if self.store.as_ref().and_then(|db|JobId::parse(job).ok().and_then(|id|db.get(&id).ok().flatten())).is_some_and(|row|row.body["state"]=="committing"&&store::canonical(&row.body["commit"])==store::canonical(input.json())){
            self.commit_reply(&input,Ok(self.pending_commit(&input)));return;
        }
        if token.is_none() {
            let result = (|| {
                let proof = self.no_admission_proof(&input)?;
                let lock = RootLock::acquire(&self.root).map_err(|_| unknown())?;
                RemotePublisher::settle_unadmitted(&lock, &proof)
            })();
            self.commit_reply(&input, result);
            return;
        }
        let result=(||->Result<(Arc<RemotePublicationPlan>,u64),NativeError>{
            if self.shared.close.load(Ordering::Acquire){return Err(busy());}
            let db=self.store.as_ref().ok_or_else(unknown)?;let row=db.get(&JobId::parse(job)?)?.ok_or_else(busy)?;
            if row.body["state"]=="committing"&&store::canonical(&row.body["commit"])==store::canonical(input.json()){return Err(unknown());}
            if self.shared.launch_blocked.load(Ordering::Acquire){return Err(NativeError::new("launchBusy","The launch owner is using this library."));}
            if row.body["state"]!="ready"{return Err(busy());}verify_claim_set(db)?;self.authorization_matches(job,&row)?;
            let token=token.as_deref().ok_or_else(||NativeError::new("staleReview","A live review is required."))?;
            let (generation,plan)=self.plans.get(job).ok_or_else(busy)?;let generation=*generation;
            plan.prepared.validate_job(&row)?;plan.validate_commit(&input)?;
            if plan.selection_digest!=input.params()["selectionDigest"]||Instant::now()>=plan.expires_at{return Err(NativeError::new("staleReview","The review is no longer current."));}
            let plan=self.take_live_plan(job,token)?;
            Ok((Arc::new(plan),generation))
        })();
        let (plan,generation)=match result{Ok(v)=>v,Err(e)=>{self.commit_reply(&input,Err(e));return;}};
        let control=crate::prepare_control::PrepareControl::new();
        {let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(job).filter(|v|v.generation==generation&&!v.release&&!v.released){view.review_control=Some(control.clone());}else{drop(s);self.commit_reply(&input,Err(busy()));return;}}
        let source=plan.prepared.clone();let worker_control=control.clone();
        let validation=match thread::Builder::new().name("remote-publication-validation".into()).spawn(move||source.revalidate(&worker_control)){Ok(h)=>h,Err(_)=>{let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(v)=s.preparations.get_mut(job).filter(|v|v.generation==generation){v.review=JsonValue::Null;v.review_params=None;v.review_control=None;v.review_expires=None;v.generation=v.generation.saturating_add(1);}drop(s);self.commit_reply(&input,Err(busy()));return;}};
        self.commit_worker=Some(CommitWorker{input,plan,generation,control,validation:Some(validation),publication:None,frozen:false,reserved:false,dispatched:false});
    }
    fn invalidate_commit_grant(&self,work:&CommitWorker){
        let job=work.input.params()["jobId"].as_str().unwrap();let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        s.frozen.remove(job);if let Some(view)=s.preparations.get_mut(job).filter(|v|v.generation==work.generation){view.review=JsonValue::Null;view.review_params=None;view.review_control=None;view.review_expires=None;view.generation=view.generation.saturating_add(1);view.admission=false;}
    }
    fn tick_commit(&mut self){
        let Some(mut work)=self.commit_worker.take() else{return;};
        let job=work.input.params()["jobId"].as_str().unwrap().to_owned();
        if let Some(h)=work.validation.as_ref(){
            if !h.is_finished(){self.commit_worker=Some(work);return;}
            let result=work.validation.take().unwrap().join().unwrap_or_else(|_|Err(unknown()));
            if let Err(e)=result{
                self.invalidate_commit_grant(&work);
                self.commit_reply(&work.input,Err(e));
                return;
            }
            let checked=(||->Result<JobSnapshot,NativeError>{
                let db=self.store.as_ref().ok_or_else(unknown)?;
                let row=db.get(&JobId::parse(&job)?)?.ok_or_else(unknown)?;
                verify_claim_set(db)?;
                self.authorization_matches(&job,&row)?;
                work.plan.prepared.validate_job(&row)?;
                if row.body["state"]!="ready"||Instant::now()>=work.plan.expires_at{return Err(busy());}
                Ok(row)
            })();
            if let Err(e)=checked{
                self.invalidate_commit_grant(&work);
                self.commit_reply(&work.input,Err(e));
                return;
            }
            // This short lock is the review-vs-commit admission linearization.
            // No database or filesystem work runs while it is held.
            let frozen={
                let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
                let valid=!self.shared.close.load(Ordering::Acquire)&&!self.shared.launch_blocked.load(Ordering::Acquire)&&!s.frozen.contains_key(&job)
                    &&s.preparations.get(&job).is_some_and(|v|!v.release&&!v.released&&v.generation==work.generation&&v.review["phase"]=="ready"&&v.review["reviewToken"]==work.plan.token&&v.review_expires.is_some_and(|t|Instant::now()<t));
                if valid{
                    s.frozen.insert(job.clone(),FrozenCommit{input:work.input.clone(),plan:work.plan.clone(),done:None,worker:Arc::new(Mutex::new(None)),retired:false,generation:work.generation});
                }
                valid
            };
            if !frozen{
                self.invalidate_commit_grant(&work);
                self.commit_reply(&work.input,Err(NativeError::new("staleReview","The review is no longer current.")));
                return;
            }
            work.frozen=true;
        }
        if work.frozen&&!work.reserved {
            let reservation=(||->Result<bool,NativeError>{
                let db=self.store.as_mut().ok_or_else(unknown)?;
                let jid=JobId::parse(&job)?;
                let mut row=db.get(&jid)?.ok_or_else(unknown)?;
                if row.body["state"]=="committing"{
                    if store::canonical(&row.body["commit"])!=store::canonical(work.input.json()){return Err(unknown());}
                    publish(&self.shared,row);
                    return Ok(true);
                }
                if row.body["state"]!="ready"{return Err(unknown());}
                work.plan.prepared.validate_job(&row)?;
                if Instant::now()>=work.plan.expires_at{return Ok(false);}
                row.body["state"]="committing".into();
                row.body["commit"]=work.input.json().clone();
                row.body["cleanupPending"]=true.into();
                let next=db.transition(&jid,JobTransition::Body(row.body))?;
                publish(&self.shared,next);
                Ok(true)
            })();
            match reservation{
                Ok(true)=>{
                    work.reserved=true;
                    self.commit_reply(&work.input,Ok(self.pending_commit(&work.input)));
                },
                Ok(false)=>{
                    self.invalidate_commit_grant(&work);
                    self.commit_reply(&work.input,Err(NativeError::new("staleReview","The review expired before commit reservation.")));
                    return;
                },
                Err(_)=>{self.commit_worker=Some(work);return;}
            }
        }
        if work.reserved&&!work.dispatched {
            let root=self.root.clone();
            let plan=work.plan.clone();
            let input=work.input.clone();
            let done=Arc::new(AtomicBool::new(false));
            let holder={
                let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
                let frozen=s.frozen.get_mut(&job).unwrap();
                frozen.done=Some(done.clone());
                frozen.retired=false;
                frozen.worker.clone()
            };
            match thread::Builder::new().name("remote-publisher".into()).spawn(move||{
                let _done=PreparationDone(done);
                match RootLock::acquire(&root){
                    Ok(lock)=>PublicationAttempt::Finished(match &input{RemoteCommitInput::Add(add)=>RemotePublisher::commit(&lock,&plan,add),RemoteCommitInput::Replace(replace)=>super::replacement_publication::ReplacementPublisher::commit(&lock,&plan,replace)}),
                    Err(e) if e.kind=="nativeBusy"=>PublicationAttempt::LockBusy,
                    Err(e)=>PublicationAttempt::Finished(Err(e))
                }
            }){
                Ok(handle)=>{
                    *holder.lock().unwrap_or_else(|p|p.into_inner())=Some(handle);
                    work.publication=Some(holder);
                    work.dispatched=true;
                },
                Err(_)=>{
                    if let Some(frozen)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.get_mut(&job){frozen.done=None;}
                }
            }
        }
        if let Some(holder)=work.publication.as_ref(){
            let handle={
                let mut slot=holder.lock().unwrap_or_else(|p|p.into_inner());
                if !slot.as_ref().is_some_and(|h|h.is_finished()){drop(slot);self.commit_worker=Some(work);return;}
                slot.take()
            };
            let joined=handle.unwrap().join();
            work.publication=None;
            if let Some(frozen)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.get_mut(&job){frozen.retired=true;}
            match joined{
                Ok(PublicationAttempt::LockBusy)=>{
                    work.dispatched=false;
                    if let Some(frozen)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.get_mut(&job){frozen.done=None;}
                    self.commit_worker=Some(work);
                    return;
                },
                Ok(PublicationAttempt::Finished(result))=>{let _=result;},
                Err(_)=>{}
            }
        }
        let terminal=(||->Result<Option<Option<NativeError>>,NativeError>{
            let anchor=self.read_anchor()?;
            let history=Store::open_history(&self.root)?;
            let outcome=history.exact_any_remote_outcome(&work.input)?;
            let journal=history.journal(work.input.operation_id())?;
            self.validate_anchor(&anchor)?;
            if journal.is_some(){return Ok(None);}
            Ok(match outcome{
                ExactRemoteOutcome::Succeeded(_)=>Some(None),
                ExactRemoteOutcome::Failed(error)=>Some(Some(error)),
                ExactRemoteOutcome::Absent=>None
            })
        })();
        let terminal_error=match terminal{Ok(Some(v))=>v,_=>{self.commit_worker=Some(work);return;}};
        let installed=terminal_error.is_none();
        let changed=(||->Result<(),NativeError>{
            let db=self.store.as_mut().ok_or_else(unknown)?;
            let jid=JobId::parse(&job)?;
            let mut row=db.get(&jid)?.ok_or_else(unknown)?;
            if store::canonical(&row.body["commit"])!=store::canonical(work.input.json()){return Err(unknown());}
            if row.body["state"]==if installed{"installed"}else{"failed"}{return Ok(());}
            if row.body["state"]!="committing"{return Err(unknown());}
            row.body["state"]=if installed{"installed"}else{"failed"}.into();
            row.body["cleanupPending"]=true.into();
            row.body["error"]=terminal_error.as_ref().map(failure).unwrap_or(JsonValue::Null);
            let next=db.transition(&jid,JobTransition::Body(row.body))?;
            publish(&self.shared,next);
            Ok(())
        })();
        if changed.is_err(){self.commit_worker=Some(work);return;}
        {
            let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
            s.frozen.remove(&job);
            if let Some(view)=s.preparations.get_mut(&job){
                view.release=true;
                view.barrier=true;
                view.review=JsonValue::Null;
                view.review_params=None;
                view.review_control=None;
                view.admission=false;
            }
            s.reconciled=false;
            s.phase="waiting";
        }
    }
    fn tick_orphan_frozen(&mut self){
        if self.commit_worker.is_some(){return;}
        let frozen={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.frozen.values().next().cloned()};
        let Some(frozen)=frozen else{return;};
        let dispatched=frozen.done.is_some();
        let publication=if dispatched&&!frozen.retired{if frozen.worker.lock().unwrap_or_else(|p|p.into_inner()).is_none(){return;}Some(frozen.worker.clone())}else{None};
        self.commit_worker=Some(CommitWorker{input:frozen.input,plan:frozen.plan,generation:frozen.generation,control:crate::prepare_control::PrepareControl::new(),validation:None,publication,frozen:true,reserved:dispatched,dispatched});
    }
    fn resolve_failed(&self,key:&str,e:&NativeError){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(r)=s.resolves.get_mut(key){r.phase="failed";r.error=failure(e);r.candidates.clear();r.token=None;}}
    fn start_failed(&self,id:&str,e:NativeError){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(p)=s.starts.get_mut(id){if e.kind!="outcomeUnknown"{p.admission=false;}p.error=Some(e);}}
    fn fail_preparation(&mut self,id:&str,key:&str,e:&NativeError){
        if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(id).is_some_and(|v|!v.release&&!v.released&&v.params["prepareKey"]==key){return;}
        let mut proven=false;
        if let (Some(db),Ok(job))=(self.store.as_mut(),JobId::parse(id)){
            if let Ok(Some(mut row))=db.get(&job){
                if !row.body["prepare"].is_null()&&row.body["prepare"]["prepareKey"]!=key{return;}
                if !matches!(row.body["state"].as_str(),Some("cancelRequested"|"cancelled"|"abandoned"|"committing"|"needsSelection")){
                    row.body["state"]=if e.kind=="needsSelection"{"needsSelection"}else{"downloaded"}.into();row.body["prepare"]=JsonValue::Null;row.body["error"]=failure(e);
                    if let Ok(next)=db.transition(&job,JobTransition::Body(row.body)){publish(&self.shared,next);proven=true;}
                }else{proven=true;}
            }
        }
        if proven{self.authorization_prepares.remove(id);self.job_authorizations.remove(id);}
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if !proven{s.phase="waiting";s.reconciled=false;return;}if let Some(view)=s.preparations.get_mut(id).filter(|v|v.params["prepareKey"]==key){view.admission=false;view.released=true;view.prepared=None;view.review=JsonValue::Null;}
    }
    fn fail_review(&mut self,id:&str,params:&JsonValue,generation:u64,e:&NativeError){
        if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(id).is_some_and(|v|v.generation==generation&&v.review_params.as_ref()==Some(params)&&!v.release){return;}
        let mut invalidated=false;
        if let (Some(db),Ok(job))=(self.store.as_mut(),JobId::parse(id)){
            if let Ok(Some(mut row))=db.get(&job){
                if e.kind=="needsSelection"&&!row.body["prepare"].is_null()&&matches!(row.body["state"].as_str(),Some("ready"|"reviewing")){
                    row.body["state"]="preparing".into();row.body["cleanupPending"]=true.into();row.body["error"]=failure(e);
                    if let Ok(next)=db.transition(&job,JobTransition::Body(row.body)){publish(&self.shared,next);invalidated=true;}
                }else if row.body["state"]=="reviewing"{row.body["state"]="ready".into();if let Ok(next)=db.transition(&job,JobTransition::Body(row.body)){publish(&self.shared,next);}}
            }
        }
        let restore={let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(id){
            if view.generation==generation&&view.review_params.as_ref()==Some(params)&&!view.release{
                view.admission=false;view.review_control=None;view.review=if params.has_key("modId"){object!{requestKey:params["requestKey"].clone(),phase:"failed",reviewToken:JsonValue::Null,selectionDigest:JsonValue::Null,afterRecordDigest:JsonValue::Null,summary:JsonValue::Null,error:failure(e)}}else{object!{requestKey:params["requestKey"].clone(),phase:"failed",reviewToken:JsonValue::Null,selectionDigest:JsonValue::Null,modId:JsonValue::Null,summary:JsonValue::Null,error:failure(e)}};
                if invalidated{view.release=true;view.barrier=true;}false
            }else{invalidated&&!view.release&&!view.released}
        }else{false}};
        if restore {if let (Some(db),Ok(job))=(self.store.as_mut(),JobId::parse(id)){if let Ok(Some(mut row))=db.get(&job){
            if row.body["state"]=="preparing"&&row.body["cleanupPending"]==true&&row.body["error"]["kind"]=="needsSelection"{
                row.body["state"]="ready".into();row.body["cleanupPending"]=false.into();row.body["error"]=JsonValue::Null;
                if let Ok(next)=db.transition(&job,JobTransition::Body(row.body)){publish(&self.shared,next);}
            }
        }}}
    }
    fn begin_preparation(&mut self,params:&JsonValue)->Result<(),NativeError>{
        if self.preparation.is_some()||self.preparation_guard.is_some()||!self.ready.is_empty()||self.review_worker.is_some()||self.review_guard.is_some(){return Err(busy());}
        let id=params["jobId"].as_str().ok_or_else(NativeError::params)?;let job=JobId::parse(id)?;
        let expiry=self.job_authorizations.get(id).filter(|a|a.valid_at(Instant::now())).ok_or_else(busy)?.expires_at();
        let db=self.store.as_mut().ok_or_else(unknown)?;let lease=db.lease();let slot=PreparationSlot::try_acquire(&self.root)?;
        if slot.identity()!=lease.identity(){return Err(ambiguous());}
        let lock=RootLock::acquire(&self.root)?;if Instant::now()>=expiry{return Err(busy());}
        let mut row=db.get(&job)?.ok_or_else(unknown)?;if row.body["state"]!="downloaded"||row.input["profileId"]!=params["profileId"]{return Err(busy());}
        if row.input["initialRevision"]!=params["expectedRevision"]&&params["acknowledgeRebase"]!=true{return Err(NativeError::new("staleRevision","The destination changed and must be acknowledged."));}
        pinned_profile(&self.root,params)?;if Instant::now()>=expiry{return Err(busy());}
        let replacement=if row.input["version"]==2 {
            let mod_id=row.input["modId"].as_str().ok_or_else(unknown)?;let revision=store::revision(params["expectedRevision"].as_str().ok_or_else(unknown)?)?;
            let target=replacement_target::read(&self.root,params["profileId"].as_str().ok_or_else(unknown)?,mod_id,revision)?;
            if row.input["targetDigest"]!=target.digest||(row.input["purpose"]!="link" && target.submission.as_ref().is_none_or(|s|row.input["submission"]!=s.json())){return Err(NativeError::new("staleReview","The original replacement target changed."));}
            Some(ReplacementPin{purpose:ReplacementPurpose::parse(&row.input["purpose"])?,mod_id:mod_id.into(),target_digest:target.digest,initial_revision:row.input["initialRevision"].as_str().ok_or_else(unknown)?.into()})
        }else{None};
        let key=contract::PrepareKey::parse(params["prepareKey"].as_str().ok_or_else(NativeError::params)?)?;
        row.body["prepare"]=object!{prepareKey:key.as_str(),profileId:params["profileId"].clone(),expectedRevision:params["expectedRevision"].clone(),acknowledgeRebase:params["acknowledgeRebase"].clone()};row.body["state"]="preparing".into();row.body["error"]=JsonValue::Null;
        let row=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,row.clone());drop(lock);
        let source=open_sealed_source(db,&job)?;let nonce=contract::ClaimId::parse(key.as_str())?;let authority=RemoteQuarantineAuthority::new(lease,job,nonce)?;
        let context=PrepareContext{root:self.root.clone(),session:self.session.clone(),prepare_key:key,profile_id:params["profileId"].as_str().unwrap().into(),expected_revision:store::revision(params["expectedRevision"].as_str().unwrap())?,expires_at:expiry,status_template:job_store::status(&row,&self.root,&self.session,None)?,replacement};
        let control=crate::prepare_control::PrepareControl::new();let worker_control=control.clone();let worker_slot=slot.clone();let worker_authority=authority.clone();
        let done=Arc::new(AtomicBool::new(false));let worker_done=done.clone();let result=Arc::new(Mutex::new(None));let worker_result=result.clone();
        self.preparation_guard=Some(PreparationRetirement{job:id.into(),key:params["prepareKey"].as_str().unwrap().into(),slot:slot.clone(),authority:authority.clone(),control:control.clone(),done,prepared:None,result});
        let handle=match thread::Builder::new().name("remote-preparation".into()).spawn(move||{let _done=PreparationDone(worker_done);let value=RemotePreparation::start(source,worker_slot,worker_authority,context,worker_control);*worker_result.lock().unwrap_or_else(|p|p.into_inner())=Some(value);}){
            Ok(handle)=>handle,Err(_)=>{self.preparation_guard=None;return Err(err_kind("transportStorage"));}
        };
        self.preparation=Some(PreparationWorker{job:id.into(),key:params["prepareKey"].as_str().unwrap().into(),control,slot,authority,handle:Some(handle),result:None});Ok(())
    }
    fn begin_review(&mut self,params:&JsonValue,expected_generation:u64)->Result<(),NativeError>{
        if self.review_worker.is_some(){return Err(busy());}
        let id=params["jobId"].as_str().ok_or_else(NativeError::params)?;let job=JobId::parse(id)?;
        let expiry=self.job_authorizations.get(id).filter(|a|a.valid_at(Instant::now())).ok_or_else(busy)?.expires_at();
        let prepared=self.ready.get(id).cloned().ok_or_else(busy)?;let db=self.store.as_mut().ok_or_else(unknown)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;
        if row.body["state"]!="ready"||row.input["profileId"]!=params["profileId"]||row.body["prepare"]["expectedRevision"]!=params["expectedRevision"]{return Err(NativeError::new("invalidSelection","The pinned preparation is unavailable."));}
        if (row.input["version"]==2)!=params.has_key("modId")||row.input["version"]==2&&(row.input["modId"]!=params["modId"]||row.input["targetDigest"]!=params["targetDigest"]){return Err(conflict());}
        let key=row.body["prepare"]["prepareKey"].as_str().ok_or_else(unknown)?.to_owned();
        let control=crate::prepare_control::PrepareControl::new();
        let generation={let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let view=s.preparations.get_mut(id).filter(|v|!v.release&&v.generation==expected_generation&&v.review_params.as_ref()==Some(params)).ok_or_else(busy)?;view.review_control=Some(control.clone());view.generation};
        row.body["state"]="reviewing".into();let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);
        if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(id).is_some_and(|v|!v.release&&v.generation==generation&&v.review_params.as_ref()==Some(params)){
            if let Some(mut row)=db.get(&job)?{if row.body["state"]=="reviewing"{row.body["state"]="ready".into();let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);}}return Ok(());
        }
        let request=RemoteReviewV1{request_key:params["requestKey"].as_str().unwrap().into(),candidate_ids:params["candidateIds"].members().map(|x|x.as_str().unwrap().to_owned()).collect(),metadata:params["metadata"].clone(),expires_at:expiry,control:control.clone()};
        let done=Arc::new(AtomicBool::new(false));let worker_done=done.clone();let result=Arc::new(Mutex::new(None));let worker_result=result.clone();
        self.review_guard=Some(ReviewRetirement{job:id.into(),key:key.clone(),control:control.clone(),done,result});
        let handle=match thread::Builder::new().name("remote-review".into()).spawn(move||{let _done=PreparationDone(worker_done);let value=prepared.review(request);*worker_result.lock().unwrap_or_else(|p|p.into_inner())=Some(value);}){Ok(handle)=>handle,Err(_)=>{self.review_guard=None;return Err(err_kind("transportStorage"));}};
        self.review_worker=Some(ReviewWorker{job:id.into(),key,request:params["requestKey"].as_str().unwrap().into(),generation,control,handle});Ok(())
    }
    fn begin_release(&mut self,id:&str,key:&str){
        if self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.contains_key(id){return;}
        let valid={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(id).is_some_and(|v|v.release&&!v.released&&v.params["prepareKey"]==key)};
        if !valid{return;}
        let result=(||->Result<bool,NativeError>{
            let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(id)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;
            if row.body["state"]=="committing"{return Err(NativeError::new("cancelTooLate","The operation is already committing."));}
            if row.body["state"]=="cancelRequested"||row.body["state"]=="cancelled"{return Ok(true);}
            if row.body["prepare"].is_null(){return Ok(false);}
            if row.body["prepare"]["prepareKey"]!=key{return Err(busy());}
            if row.body["state"]=="preparing"&&row.body["cleanupPending"]==true{return Ok(true);}
            row.body["state"]="preparing".into();row.body["cleanupPending"]=true.into();
            let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);Ok(true)
        })();
        match result {
            Ok(true)=>{
                if let Some(view)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get_mut(id).filter(|v|v.params["prepareKey"]==key){view.barrier=true;}
                if let Some(w)=self.preparation.as_ref().filter(|w|w.job==id&&w.key==key){w.control.request_cancel();}
                if let Some(w)=self.review_worker.as_ref().filter(|w|w.job==id&&w.key==key){w.control.request_cancel();}
                if let Some(w)=self.commit_worker.as_ref().filter(|w|w.input.params()["jobId"]==id&&!w.frozen){w.control.request_cancel();}
                if let Some(w)=self.preparation_guard.as_ref().filter(|w|w.job==id&&w.key==key){w.control.request_cancel();}
                if let Some(w)=self.review_guard.as_ref().filter(|w|w.job==id&&w.key==key){w.control.request_cancel();}
                self.plans.remove(id);
            },
            Ok(false)=>{let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(id).filter(|v|v.params["prepareKey"]==key){view.released=true;view.admission=false;}},
            Err(_)=>{}, // The bounded owner loop retries the same keyed barrier.
        }
    }
    fn finish_release(&mut self,id:&str,key:&str)->bool{
        let result=(||->Result<(),NativeError>{let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(id)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;
            if row.body["state"]!="preparing"||row.body["prepare"]["prepareKey"]!=key||row.body["cleanupPending"]!=true||!row.body["commit"].is_null(){return Err(busy());}
            row.body["state"]=if row.body["error"]["kind"]=="needsSelection"{"needsSelection"}else{"downloaded"}.into();row.body["prepare"]=JsonValue::Null;row.body["cleanupPending"]=false.into();
            let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);Ok(())})();
        if result.is_err(){return false;}
        self.authorization_prepares.remove(id);self.job_authorizations.remove(id);
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(id).filter(|v|v.params["prepareKey"]==key){view.released=true;view.prepared=None;view.review=JsonValue::Null;view.admission=false;}true
    }
    fn tick_fetch(&mut self){
        let Some(mut f)=self.fetch.take() else{return;};
        let route=[MetadataRoute::Profile,MetadataRoute::Files,MetadataRoute::Servers][f.route];
        let submission=match &f.purpose{FetchPurpose::Resolve{submission,..}|FetchPurpose::Start{submission,..}|FetchPurpose::Resume{submission,..}|FetchPurpose::Prepare{submission,..}|FetchPurpose::Review{submission,..}=>submission};
        let step=(||->Result<bool,NativeError>{
            let metadata=self.metadata.as_mut().ok_or_else(unknown)?;
            if f.active.is_none(){let request=authority::metadata_request(submission,route)?;let id=request.request_id.clone();metadata.submit(request).map_err(|_|NativeError::new("transportNetwork","Fresh metadata is unavailable."))?;f.active=Some(id);return Ok(false);}
            let id=f.active.as_deref().unwrap();let Some(result)=metadata.poll_for(id) else{return Ok(false);};
            f.responses.push(RawResponse::from_completion(route,result)?);f.active=None;f.route+=1;Ok(f.route==3)
        })();
        match step {
            Ok(false)=>self.fetch=Some(f),
            Err(e)=>match f.purpose{FetchPurpose::Resolve{key,..}=>self.resolve_failed(&key,&e),FetchPurpose::Start{params,..}=>self.start_failed(params["jobId"].as_str().unwrap(),e),FetchPurpose::Resume{job,..}=>self.mark_failed(&job,e),FetchPurpose::Prepare{params,..}=>self.fail_preparation(params["jobId"].as_str().unwrap(),params["prepareKey"].as_str().unwrap(),&e),FetchPurpose::Review{params,generation,..}=>self.fail_review(params["jobId"].as_str().unwrap(),&params,generation,&e)},
            Ok(true)=>{
                let responses:Result<[RawResponse;3],_>=f.responses.try_into();let responses=match responses{Ok(r)=>r,Err(_)=>return};
                let request=match &f.purpose{FetchPurpose::Resolve{submission,..}=>AuthorityRequest{session:self.session.clone(),submission:submission.clone(),selected:None},FetchPurpose::Start{submission,expected,..}|FetchPurpose::Resume{submission,expected,..}|FetchPurpose::Prepare{submission,expected,..}|FetchPurpose::Review{submission,expected,..}=>AuthorityRequest{session:self.session.clone(),submission:submission.clone(),selected:Some(expected.clone())}};
                let result=FreshFileAuthorityV1::evaluate(&request,&responses,Instant::now());
                match (f.purpose,result) {
                    (FetchPurpose::Resolve{key,..},Ok(auth))=>{
                        let token=match contract::SelectionToken::generate(){Ok(t)=>t.as_str().to_owned(),Err(e)=>{self.resolve_failed(&key,&e);return;}};
                        let candidates=auth.candidates();self.authorizations.insert(token.clone(),auth);
                        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(r)=s.resolves.get_mut(&key){r.candidates=candidates;r.token=Some(token);r.phase="ready";r.error=JsonValue::Null;}
                    },
                    (FetchPurpose::Resolve{key,..},Err(e))=>self.resolve_failed(&key,&e),
                    (FetchPurpose::Start{params,submission,expected},Ok(auth))=>{
                        let id=params["jobId"].as_str().unwrap().to_owned();match self.admit_start(&params,submission,expected,&auth){Ok(j)=>{self.job_authorizations.insert(id.clone(),auth);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.jobs.insert(id.clone(),j);s.starts.remove(&id);},Err(e)=>self.start_failed(&id,e)}
                    },
                    (FetchPurpose::Start{params,..},Err(e))=>self.start_failed(params["jobId"].as_str().unwrap(),e),
                    (FetchPurpose::Resume{job,action,..},Ok(auth))=>{self.attempt_modes.insert(job.clone(),action);self.job_authorizations.insert(job,auth);},
                    (FetchPurpose::Resume{job,..},Err(e))=>self.mark_failed(&job,e),
                    (FetchPurpose::Prepare{params,..},Ok(auth))=>{let id=params["jobId"].as_str().unwrap().to_owned();let key=params["prepareKey"].as_str().unwrap().to_owned();let current=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get(&id).is_some_and(|v|!v.release&&!v.released&&v.params==params);if current{self.authorization_prepares.insert(id.clone(),key);self.job_authorizations.insert(id,auth);self.retry_prepare(params);}},
                    (FetchPurpose::Prepare{params,..},Err(e))=>self.fail_preparation(params["jobId"].as_str().unwrap(),params["prepareKey"].as_str().unwrap(),&e),
                    (FetchPurpose::Review{params,generation,..},Ok(auth))=>{let id=params["jobId"].as_str().unwrap().to_owned();let key={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(&id).filter(|v|!v.release&&!v.released&&v.generation==generation&&v.review_params.as_ref()==Some(&params)).and_then(|v|v.params["prepareKey"].as_str().map(str::to_owned))};if let Some(key)=key{self.authorization_prepares.insert(id.clone(),key);self.job_authorizations.insert(id,auth);self.retry_review(params,generation);}},
                    (FetchPurpose::Review{params,generation,..},Err(e))=>self.fail_review(params["jobId"].as_str().unwrap(),&params,generation,&e),
                }
            },
        }
    }
    fn admit_start(&mut self,p:&JsonValue,submission:SubmissionKeyV1,expected:ExpectedSelection,auth:&FreshAuthorization)->Result<JobSnapshot,NativeError>{
        if !auth.valid_at(Instant::now()){return Err(NativeError::new("providerUnavailable","Fresh authorization expired before admission."));}
        self.validate_root()?;
        // A fresh root is first inspected under RootLock without creating the
        // remote namespace. Then acquire the remote lease and recheck the pin
        // under a new RootLock, preserving the lease-before-writer lock order.
        let fresh_lease=if self.store.is_none(){let preliminary=RootLock::acquire(&self.root)?;if Some(identity(&preliminary.directory().metadata()?))!=self.root_identity{return Err(ambiguous());}let _=pinned_profile(&self.root,p)?;if !auth.valid_at(Instant::now()){return Err(NativeError::new("providerUnavailable","Fresh authorization expired before admission."));}drop(preliminary);
            Some(if let Some((lease,_))=self.pending_skips.values().next(){lease.clone()}else{Arc::new(RemoteStoreLease::acquire(&self.root)?)})}else{None};
        let lock=RootLock::acquire(&self.root)?;if Some(identity(&lock.directory().metadata()?))!=self.root_identity{return Err(ambiguous());}self.validate_root()?;if !auth.valid_at(Instant::now()){return Err(NativeError::new("providerUnavailable","Fresh authorization expired before admission."));}
        let name=pinned_profile(&self.root,p)?;let profile=p["profileId"].as_str().unwrap();
        let replacement=if p.has_key("purpose"){
            let purpose=ReplacementPurpose::parse(&p["purpose"])?;let mod_id=p["modId"].as_str().ok_or_else(NativeError::params)?;
            let revision=store::revision(p["expectedRevision"].as_str().ok_or_else(NativeError::params)?)?;
            let target=replacement_target::read(&self.root,profile,mod_id,revision)?;
            if p["targetDigest"]!=target.digest||!target.accepts_submission(purpose,&submission){return Err(NativeError::new("staleReview","The replacement target or submission changed."));}
            if purpose==ReplacementPurpose::Reinstall&&p["fileId"]!=target.root_file_id{return Err(NativeError::new("invalidSelection","Reinstall requires the saved root file."));}
            if purpose==ReplacementPurpose::Update{
                let lease=if let Some(db)=&self.store{db.lease()}else{fresh_lease.clone().ok_or_else(unknown)?};
                replacement_skips::check_update(&lease,&lock,profile,mod_id,&target.digest,p["fileId"].as_str().ok_or_else(unknown)?,p["candidateFingerprint"].as_str().ok_or_else(unknown)?)?;
            }
            Some((purpose,mod_id.to_owned(),target.digest))
        }else{None};
        if !auth.valid_at(Instant::now()){return Err(NativeError::new("providerUnavailable","Fresh authorization expired before admission."));}
        crate::catalog::store::reserve(&self.root).map_err(|_|err_kind("storageFull"))?;
        // Metadata admission may have taken time. Recheck under the root and
        // job leases before a new DB or job can assert fresh history.
        let namespace_safe=if self.store.is_none(){job_store::empty_namespace(&self.root)}else{job_store::quarantine_namespace_present(&self.root).map(|present|!present)};
        if !matches!(namespace_safe,Ok(true)){
            let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.reconciled=false;s.claims_admitted=false;s.phase="waiting";
            return Err(ambiguous());
        }
        if self.store.is_none(){
            let lease=fresh_lease.ok_or_else(unknown)?;if Some(lease.identity())!=self.root_identity{return Err(ambiguous());}
            self.store=Some(RemoteJobStore::open_with_lease(lease,true).map_err(|e|{
                let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.reconciled=false;s.claims_admitted=false;s.phase="waiting";e
            })?);
        }
        if !auth.valid_at(Instant::now()){return Err(NativeError::new("providerUnavailable","Fresh authorization expired before admission."));}
        let input=RemoteJobInputV1{job_id:JobId::parse(p["jobId"].as_str().unwrap())?,submission,root_file:expected.root,profile_id:profile.into(),initial_revision:p["expectedRevision"].as_str().unwrap().into(),accept_archived:p["acceptArchived"].as_bool().unwrap(),consent_fingerprint:expected.candidate_fingerprint,submission_name:expected.submission_name,profile_name_at_start:name};
        let receipt=if let Some((purpose,mod_id,target_digest))=replacement{
            self.store.as_mut().unwrap().reserve_replacement_start(&ReplacementJobInputV2{base:input,purpose,mod_id,target_digest})?
        }else{self.store.as_mut().unwrap().reserve_start(&input)?};drop(lock);Ok(receipt.snapshot)
    }
    fn control(&mut self,id:&str,seq:&str,action:&str)->Result<String,NativeError>{
        let frozen=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.contains_key(id);
        let job=JobId::parse(id)?;let db=self.store.as_mut().ok_or_else(busy)?;let current=db.get(&job).map_err(|_|unknown())?.ok_or_else(||NativeError::new("controlReceiptExpired","No control receipt is retained for this identity."))?;
        if !current.body["lastControl"].is_null(){let last=&current.body["lastControl"];let requested=store::revision(seq)?;let prior=store::revision(last["controlSeq"].as_str().ok_or_else(unknown)?)?;
            if requested==prior {if last["action"]!=action{return Err(conflict());}let result=last["result"].as_str().ok_or_else(unknown)?.to_owned();publish(&self.shared,current);return Ok(result);}if requested<prior{return Err(NativeError::new("controlReceiptExpired","This control receipt is no longer retained."));}}
        let mut b=current.body.clone();let state=b["state"].as_str().unwrap_or("").to_owned();
        let has_slot={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.queue.len()<MAX_QUEUE};
        let preparation_live=self.preparation.as_ref().is_some_and(|w|w.job==id)||self.preparation_guard.as_ref().is_some_and(|w|w.job==id)||self.review_worker.as_ref().is_some_and(|w|w.job==id)||self.review_guard.as_ref().is_some_and(|w|w.job==id)||self.ready.contains_key(id)||self.retiring_preparation.as_ref().is_some_and(|w|w.job==id);
        let no_live=self.active.as_ref().is_none_or(|a|a.job!=id)&&self.hashing.as_ref().is_none_or(|h|h.job!=id)&&self.discovery.as_ref().is_none_or(|h|h.job!=id)&&self.resume_hash.as_ref().is_none_or(|h|h.job!=id)&&!preparation_live;
        let dismissible=if action=="dismiss"&&matches!(state.as_str(),"installed"|"cancelled"|"abandoned"|"failed")&&b["cleanupPending"]==false&&b["claims"].members().all(|c|c["state"]=="cleaned")&&no_live{
            verify_claim_set(db)?;if b["commit"].is_null(){true}else{let history=Store::open_history(&self.root).map_err(|_|unknown())?;let input=RemoteCommitInput::parse(b["commit"].clone()).map_err(|_|unknown())?;let terminal=matches!(history.exact_any_remote_outcome(&input).map_err(|_|unknown())?,ExactRemoteOutcome::Succeeded(_)|ExactRemoteOutcome::Failed(_))&&history.journal(input.operation_id()).map_err(|_|unknown())?.is_none();db.lease().validate()?;terminal}
        }else{false};
        let accepted=match action {
            "pause" if matches!(state.as_str(),"resolving"|"transferring"|"discoveringFamily"|"sealing")=>{b["state"]="paused".into();true},
            "resume"|"retry" if b["commit"].is_null()&&has_slot&&matches!(state.as_str(),"paused"|"failed")=>{b["state"]="resolving".into();b["error"]=JsonValue::Null;true},
            "cancel" if b["commit"].is_null()&&!frozen&&!matches!(state.as_str(),"committing"|"installed"|"cancelled"|"abandoned")=>{b["state"]=if self.active.as_ref().is_some_and(|a|a.job==id)||preparation_live{"cancelRequested"}else{"cancelled"}.into();b["cleanupPending"]=(b["claims"].len()>0||preparation_live).into();true},
            "abandon" if !frozen&&matches!(state.as_str(),"failed"|"paused"|"needsSelection"|"downloaded")=>{b["state"]="abandoned".into();b["cleanupPending"]=(b["claims"].members().any(|c|c["state"]!="cleaned")).into();true},
            "dismiss" if dismissible || (state=="failed" && no_live && !frozen && b["commit"].is_null())=>{if state=="failed"{b["state"]="abandoned".into();b["cleanupPending"]=(b["claims"].members().any(|c|c["state"]!="cleaned")).into();}true},
            _=>false,
        };
        if action=="cancel"&&(frozen||!b["commit"].is_null()||state=="committing"){b["error"]=failure(&NativeError::new("cancelTooLate","The operation is already committing."));}
        let outcome=if accepted{"accepted"}else{"rejected"};
        let next=db.transition(&job,JobTransition::Control{seq:seq.into(),action:action.into(),result:outcome.into(),body:b})?;
        let rows=if accepted&&action=="dismiss"{Some(db.snapshots().map_err(|_|unknown())?)}else{None};
        let mut shared=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());shared.jobs.insert(id.into(),next);if accepted&&matches!(action,"pause"|"cancel"|"abandon"){shared.progress.remove(id);}
        if accepted&&action=="cancel" {
            if let Some(w)=self.preparation.as_ref().filter(|w|w.job==id){w.control.request_cancel();}
            if let Some(w)=self.review_worker.as_ref().filter(|w|w.job==id){w.control.request_cancel();}
            if let Some(w)=self.commit_worker.as_ref().filter(|w|w.input.params()["jobId"]==id&&!w.frozen){w.control.request_cancel();}
            if let Some(view)=shared.preparations.get_mut(id){view.release=true;view.barrier=true;view.review=JsonValue::Null;view.review_params=None;view.admission=false;}
            self.plans.remove(id);
        }
        if accepted&&matches!(action,"cancel"|"abandon"|"dismiss") {shared.reconciled=false;shared.phase="waiting";}
        if let Some(rows)=rows{shared.jobs=rows.into_iter().map(|j|(j.input["jobId"].as_str().unwrap().to_owned(),j)).collect();let ids=shared.jobs.keys().cloned().collect::<BTreeSet<_>>();shared.controls.retain(|k,_|ids.contains(k));}
        if accepted&&matches!(action,"resume"|"retry"){shared.queue.push_back(Work::Resume{job:id.into(),action:action.into()});self.shared.wake.notify_one();}Ok(outcome.into())
    }
}

fn ambiguous()->NativeError{NativeError::new("claimAmbiguous","Remote claim ownership is incomplete; preserve it for reconciliation.")}
unsafe extern "C" {fn openat(dir:i32,name:*const i8,flags:i32,...)->i32;fn mkdirat(dir:i32,name:*const i8,mode:u32)->i32;fn unlinkat(dir:i32,name:*const i8,flags:i32)->i32;fn flock(fd:i32,op:i32)->i32;fn geteuid()->u32;fn getdents64(fd:i32,buf:*mut c_void,len:usize)->isize;}
fn try_at(dir:&File,name:&str,flags:i32,mode:u32)->io::Result<File>{
    let name=CString::new(name).map_err(|_|io::Error::other("invalid private name"))?;let fd=unsafe{openat(dir.as_raw_fd(),name.as_ptr(),flags|0x800|0x20000|0x80000,mode)};
    if fd<0{return Err(io::Error::last_os_error());}Ok(unsafe{File::from_raw_fd(fd)})
}
fn at(dir:&File,name:&str,flags:i32,mode:u32)->Result<File,NativeError>{try_at(dir,name,flags,mode).map_err(|_|ambiguous())}
fn optional_at(dir:&File,name:&str,flags:i32)->Result<Option<File>,NativeError>{match try_at(dir,name,flags,0){Ok(f)=>Ok(Some(f)),Err(e) if e.kind()==io::ErrorKind::NotFound=>Ok(None),Err(_)=>Err(ambiguous())}}
fn unlink_at(dir:&File,name:&str,directory:bool)->Result<(),NativeError>{let name=CString::new(name).map_err(|_|ambiguous())?;if unsafe{unlinkat(dir.as_raw_fd(),name.as_ptr(),if directory{0x200}else{0})}!=0{return Err(ambiguous());}Ok(())}
fn entries(dir:&File,mut visit:impl FnMut(&[u8])->Result<(),NativeError>)->Result<(),NativeError>{
    let fresh=at(dir,".",0x10000,0)?;let mut buf=[0u8;8192];
    loop {let n=unsafe{getdents64(fresh.as_raw_fd(),buf.as_mut_ptr() as *mut c_void,buf.len())};if n<0{return Err(ambiguous());}if n==0{return Ok(());}
        let mut i=0usize;while i<n as usize{if i+19>n as usize{return Err(ambiguous());}let len=u16::from_ne_bytes([buf[i+16],buf[i+17]]) as usize;if len<20||i+len>n as usize{return Err(ambiguous());}let raw=&buf[i+19..i+len];let end=raw.iter().position(|b|*b==0).ok_or_else(ambiguous)?;let name=&raw[..end];if name!=b"."&&name!=b".."{visit(name)?;}i+=len;}}
}
fn identity(m:&std::fs::Metadata)->(u64,u64){(m.dev(),m.ino())}
fn private_file(m:&std::fs::Metadata)->bool{m.is_file()&&m.uid()==unsafe{geteuid()}&&m.nlink()==1&&m.mode()&0o077==0}
fn private_dir(m:&std::fs::Metadata)->bool{m.is_dir()&&m.uid()==unsafe{geteuid()}&&m.mode()&0o077==0}
fn expected_pair(c:&JsonValue,dev:&str,ino:&str,lease:&RemoteStoreLease,path:&Path,file:&File)->bool{
    let (Some(device),Some(inode))=(c[dev].as_str().and_then(|s|s.parse::<u64>().ok()),c[ino].as_str().and_then(|s|s.parse::<u64>().ok())) else{return false};
    let fsid=c["claimFsid"].as_str();
    paths::same_persisted_object(lease.root(),lease.root_directory(),path,file,device,inode,fsid).unwrap_or(false)
}
fn missing_intent(c:&JsonValue,kind:&str)->bool{c["state"]=="cleaning"&&c["cleanup"]["intent"]["kind"]==kind}
fn verify_claim_set(db:&RemoteJobStore)->Result<u64,NativeError>{
    let lease=db.lease();lease.validate()?;let rows=db.snapshots()?;let mut expected=BTreeMap::new();
    for j in rows {for c in j.body["claims"].members(){if c["state"]=="cleaned"{continue;}let id=c["claimId"].as_str().ok_or_else(ambiguous)?;if expected.insert(format!(".remote-claim-{id}"),c.clone()).is_some(){return Err(ambiguous());}}}
    let mut seen=BTreeSet::new();let mut identities=BTreeSet::new();identities.insert(lease.identity());let mut bytes=0u64;
    entries(lease.directory(),|name|{
        if !name.starts_with(b".remote-claim-"){return Ok(());}let name=std::str::from_utf8(name).map_err(|_|ambiguous())?;let c=expected.get(name).ok_or_else(ambiguous)?;seen.insert(name.to_owned());
        let claim_path=lease.bulk_path().join(name);
        if c["dirDevice"].is_null(){return Err(ambiguous());}let dir=at(lease.directory(),name,0x10000,0)?;let m=dir.metadata().map_err(|_|ambiguous())?;if !private_dir(&m)||!expected_pair(c,"dirDevice","dirInode",&lease,&claim_path,&dir)||!identities.insert(identity(&m)){return Err(ambiguous());}
        let cleaning=c["state"]=="cleaning";let cursor=c["cleanup"]["cursor"].as_str().and_then(|s|s.parse::<u64>().ok()).ok_or_else(ambiguous)?;
        let expect_data=!c["fileDevice"].is_null()&&(!cleaning||cursor==0);let expect_lock=!c["leaseDevice"].is_null()&&(!cleaning||cursor<=1);
        let mut data=false;let mut lock=false;entries(&dir,|child|{match child {b"data"=>{if data||!expect_data{return Err(ambiguous());}let f=at(&dir,"data",0,0)?;let m=f.metadata().map_err(|_|ambiguous())?;let durable=c["durableOffset"].as_str().and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);if !private_file(&m)||!expected_pair(c,"fileDevice","fileInode",&lease,&claim_path.join("data"),&f)||!identities.insert(identity(&m))||c["uid"].as_str().and_then(|s|s.parse::<u32>().ok())!=Some(m.uid())||c["linkCount"]!="1"||m.len()>contract::MAX_MEMBER_BYTES||m.len()<durable{return Err(ambiguous());}bytes=bytes.checked_add(m.len()).filter(|n|*n<=contract::MAX_COHORT_BYTES).ok_or_else(ambiguous)?;data=true;Ok(())},
            b"lease"=>{if lock||!expect_lock{return Err(ambiguous());}let f=at(&dir,"lease",0,0)?;let m=f.metadata().map_err(|_|ambiguous())?;if !private_file(&m)||!expected_pair(c,"leaseDevice","leaseInode",&lease,&claim_path.join("lease"),&f)||!identities.insert(identity(&m)){return Err(ambiguous());}lock=true;Ok(())},_=>Err(ambiguous())}})?;
        if expect_data&&!data&&!missing_intent(c,"data")||expect_lock&&!lock&&!missing_intent(c,"lease"){return Err(ambiguous());}Ok(())
    })?;
    for (name,c) in expected{if !seen.contains(&name)&&!c["dirDevice"].is_null()&&!missing_intent(&c,"directory"){return Err(ambiguous());}}
    lease.validate()?;Ok(bytes)
}
struct Claim {directory:File,reader:File,writer:Option<File>,shared:Option<File>,identity:contract::ClaimIdentity}
fn claim_name(id:&str)->String{format!(".remote-claim-{id}")}
/// Revalidate both held and named claim identities. This function may hash and
/// therefore runs only on the preparation worker, never under RootLock.
pub(crate) fn revalidate_sealed_source(source:&contract::SealedRemoteSourceV1, lease:&RemoteStoreLease, control:&crate::prepare_control::PrepareControl, hash:bool)->Result<(),NativeError>{
    lease.validate()?;
    if source.members.is_empty() || source.members.len()>contract::MAX_MEMBERS || source.members[0].file_key.file_id!=source.root_file_id {return Err(ambiguous());}
    let mut ids=BTreeSet::new();let mut files=Vec::new();
    for member in &source.members {
        control.check()?;
        if member.file_key.submission!=source.submission || member.file_key.file_id!=member.receipt.file.file_id || !ids.insert(member.file_key.file_id.clone()) {return Err(ambiguous());}
        let id=&member.claim_identity;
        if (id.root_device,id.root_inode)!=lease.identity(){return Err(ambiguous());}
        let dir=File::from(member.claim_directory.try_clone().map_err(|_|ambiguous())?);
        let read=File::from(member.read_fd.try_clone().map_err(|_|ambiguous())?);
        let held_lock=File::from(member.claim_lease.try_clone().map_err(|_|ambiguous())?);
        let named_dir=at(lease.directory(),&claim_name(member.claim_id.as_str()),0x10000,0)?;
        let dm=dir.metadata().map_err(|_|ambiguous())?;let nd=named_dir.metadata().map_err(|_|ambiguous())?;
        if !private_dir(&dm)||!private_dir(&nd)||identity(&dm)!=(id.directory_device,id.directory_inode)||identity(&nd)!=identity(&dm){return Err(ambiguous());}
        let named_data=at(&dir,"data",0,0)?;let named_lock=at(&dir,"lease",0,0)?;
        let check=||->Result<(),NativeError>{
            let a=read.metadata().map_err(|_|ambiguous())?;let b=named_data.metadata().map_err(|_|ambiguous())?;
            let l=held_lock.metadata().map_err(|_|ambiguous())?;let n=named_lock.metadata().map_err(|_|ambiguous())?;
            if !private_file(&a)||!private_file(&b)||!private_file(&l)||!private_file(&n)
                ||(a.dev(),a.ino(),a.uid(),a.nlink())!=(id.data_device,id.data_inode,id.uid,id.link_count)
                ||identity(&b)!=identity(&a)||identity(&l)!=(id.lease_device,id.lease_inode)||identity(&n)!=identity(&l)
                ||a.len().to_string()!=member.receipt.measured_size||b.len()!=a.len(){return Err(ambiguous());}Ok(())
        };
        check()?;
        if hash {
            let mut read=read;read.seek(SeekFrom::Start(0)).map_err(|_|ambiguous())?;
            let mut digest=Sha256::new();let mut buf=[0u8;65536];let mut size=0u64;
            loop{control.check()?;let n=read.read(&mut buf).map_err(|_|ambiguous())?;if n==0{break;}size=size.checked_add(n as u64).filter(|n|*n<=contract::MAX_MEMBER_BYTES).ok_or_else(ambiguous)?;digest.update(&buf[..n]);}
            if size.to_string()!=member.receipt.measured_size||format!("{:x}",digest.finalize())!=member.receipt.sha256{return Err(err_kind("transportChecksum"));}
            let after=read.metadata().map_err(|_|ambiguous())?;
            if (after.dev(),after.ino(),after.uid(),after.nlink(),after.len())!=(id.data_device,id.data_inode,id.uid,id.link_count,size){return Err(ambiguous());}
        }
        let final_dir=at(lease.directory(),&claim_name(member.claim_id.as_str()),0x10000,0)?;
        if identity(&final_dir.metadata().map_err(|_|ambiguous())?)!=(id.directory_device,id.directory_inode)
            ||identity(&at(&final_dir,"data",0,0)?.metadata().map_err(|_|ambiguous())?)!=(id.data_device,id.data_inode)
            ||identity(&at(&final_dir,"lease",0,0)?.metadata().map_err(|_|ambiguous())?)!=(id.lease_device,id.lease_inode){return Err(ambiguous());}
        files.push(member.receipt.file.clone());
    }
    if contract::cohort_fingerprint(&source.submission,&source.root_file_id,&files)?!=source.cohort_fingerprint{return Err(ambiguous());}
    lease.validate()?;Ok(())
}

fn open_sealed_source(db:&RemoteJobStore,job:&JobId)->Result<contract::SealedRemoteSourceV1,NativeError>{
    verify_claim_set(db)?;let row=db.get(job)?.ok_or_else(ambiguous)?;
    if row.body["cohort"]["complete"]!=true{return Err(ambiguous());}
    let submission=SubmissionKeyV1::parse(&row.input["submission"])?;let lease=db.lease();let mut members=Vec::new();
    for value in row.body["cohort"]["files"].members(){
        let file=ConsentFile::parse(value)?;let index=claim_index(&row,&file.file_id)?;let c=&row.body["claims"][index];
        if c["state"]!="sealed"{return Err(ambiguous());}
        let receipt=contract::MemberReceiptV1::parse(&c["receipt"])?;if receipt.file!=file{return Err(ambiguous());}
        let claim_id=contract::ClaimId::parse(c["claimId"].as_str().ok_or_else(ambiguous)?)?;
        let claim_path=lease.bulk_path().join(claim_name(claim_id.as_str()));
        let directory=at(lease.directory(),&claim_name(claim_id.as_str()),0x10000,0)?;let dm=directory.metadata().map_err(|_|ambiguous())?;
        let held=at(&directory,"lease",2,0)?;let lm=held.metadata().map_err(|_|ambiguous())?;
        if !private_dir(&dm)||!private_file(&lm)||!expected_pair(c,"dirDevice","dirInode",&lease,&claim_path,&directory)||!expected_pair(c,"leaseDevice","leaseInode",&lease,&claim_path.join("lease"),&held)||unsafe{flock(held.as_raw_fd(),2|4)}!=0{return Err(ambiguous());}
        let reader=at(&directory,"data",0,0)?;let m=reader.metadata().map_err(|_|ambiguous())?;
        if !private_file(&m)||!expected_pair(c,"fileDevice","fileInode",&lease,&claim_path.join("data"),&reader)||m.len().to_string()!=receipt.measured_size{return Err(ambiguous());}
        let (root_device,root_inode)=lease.identity();
        members.push(contract::SealedRemoteMember{file_key:contract::ProviderFileKeyV1{submission:submission.clone(),file_id:file.file_id.clone()},receipt,read_fd:reader.into(),claim_lease:held.into(),claim_directory:directory.into(),claim_id,
            claim_identity:contract::ClaimIdentity{root_device,root_inode,directory_device:dm.dev(),directory_inode:dm.ino(),data_device:m.dev(),data_inode:m.ino(),lease_device:lm.dev(),lease_inode:lm.ino(),uid:m.uid(),link_count:m.nlink()}});
    }
    let source=contract::SealedRemoteSourceV1{job_id:job.clone(),submission,root_file_id:row.input["rootFileId"].as_str().ok_or_else(ambiguous)?.into(),consent_fingerprint:row.input["consentFingerprint"].as_str().ok_or_else(ambiguous)?.into(),cohort_fingerprint:row.body["cohort"]["fingerprint"].as_str().ok_or_else(ambiguous)?.into(),members};
    revalidate_sealed_source(&source,&lease,&crate::prepare_control::PrepareControl::new(),false)?;Ok(source)
}
fn publish(shared:&Shared,j:JobSnapshot){let id=j.input["jobId"].as_str().unwrap().to_owned();shared.state.lock().unwrap_or_else(|p|p.into_inner()).jobs.insert(id,j);}
fn create_claim(db:&mut RemoteJobStore,shared:&Shared,job:&JobId,file:&ConsentFile)->Result<Claim,NativeError>{
    verify_claim_set(db)?;crate::catalog::store::reserve(db.lease().root()).map_err(|_|NativeError::new("storageFull","Remote storage headroom is unavailable."))?;
    let mut current=db.get(job)?.ok_or_else(ambiguous)?;if current.body["claims"].members().any(|c|c["fileId"].as_str()==Some(file.file_id.as_str())){return Err(ambiguous());}
    let id=contract::ClaimId::generate()?;let name=claim_name(id.as_str());
    let c=object!{fileId:file.file_id.clone(),claimId:id.as_str(),state:"intent",claimFsid:JsonValue::Null,dirDevice:JsonValue::Null,dirInode:JsonValue::Null,fileDevice:JsonValue::Null,fileInode:JsonValue::Null,leaseDevice:JsonValue::Null,leaseInode:JsonValue::Null,uid:JsonValue::Null,linkCount:JsonValue::Null,durableOffset:JsonValue::Null,prefixSha256:JsonValue::Null,strongEtag:JsonValue::Null,effectiveUriSha256:JsonValue::Null,total:JsonValue::Null,epoch:JsonValue::Null,receipt:JsonValue::Null,cleanup:object!{cursor:"0",intent:JsonValue::Null}};
    current.body["claims"].push(c).map_err(|_|ambiguous())?;current=db.transition(job,JobTransition::Body(current.body))?;publish(shared,current.clone());
    let lease=db.lease();lease.validate()?;let cname=CString::new(name.clone()).map_err(|_|ambiguous())?;if unsafe{mkdirat(lease.directory().as_raw_fd(),cname.as_ptr(),0o700)}!=0{return Err(ambiguous());}
    let dir=at(lease.directory(),&name,0x10000,0)?;let dm=dir.metadata().map_err(|_|ambiguous())?;if !private_dir(&dm){return Err(ambiguous());}lease.directory().sync_all().map_err(|_|ambiguous())?;
    let index=current.body["claims"].len()-1;current.body["claims"][index]["dirDevice"]=dm.dev().to_string().into();current.body["claims"][index]["dirInode"]=dm.ino().to_string().into();current.body["claims"][index]["claimFsid"]=paths::persisted_filesystem_id(&dir).map_err(|_|ambiguous())?.into();current=db.transition(job,JobTransition::Body(current.body))?;publish(shared,current.clone());
    let writer=at(&dir,"data",2|0x40|0x80,0o600)?;let fm=writer.metadata().map_err(|_|ambiguous())?;if !private_file(&fm){return Err(ambiguous());}writer.sync_all().map_err(|_|ambiguous())?;dir.sync_all().map_err(|_|ambiguous())?;
    current.body["claims"][index]["fileDevice"]=fm.dev().to_string().into();current.body["claims"][index]["fileInode"]=fm.ino().to_string().into();current.body["claims"][index]["uid"]=fm.uid().to_string().into();current.body["claims"][index]["linkCount"]=fm.nlink().to_string().into();current=db.transition(job,JobTransition::Body(current.body))?;publish(shared,current.clone());
    let lock=at(&dir,"lease",2|0x40|0x80,0o600)?;let lm=lock.metadata().map_err(|_|ambiguous())?;if !private_file(&lm)||identity(&lm)==identity(&fm){return Err(ambiguous());}lock.sync_all().map_err(|_|ambiguous())?;dir.sync_all().map_err(|_|ambiguous())?;
    current.body["claims"][index]["leaseDevice"]=lm.dev().to_string().into();current.body["claims"][index]["leaseInode"]=lm.ino().to_string().into();current.body["claims"][index]["durableOffset"]="0".into();current.body["claims"][index]["state"]="owned".into();current=db.transition(job,JobTransition::Body(current.body))?;publish(shared,current);
    if unsafe{flock(lock.as_raw_fd(),1|4)}!=0{return Err(ambiguous());}let reader=at(&dir,"data",0,0)?;let rm=reader.metadata().map_err(|_|ambiguous())?;if identity(&rm)!=identity(&fm){return Err(ambiguous());}
    verify_claim_set(db)?;let (root_device,root_inode)=lease.identity();
    Ok(Claim{directory:dir,reader,writer:Some(writer),shared:Some(lock),identity:contract::ClaimIdentity{root_device,root_inode,directory_device:dm.dev(),directory_inode:dm.ino(),data_device:fm.dev(),data_inode:fm.ino(),lease_device:lm.dev(),lease_inode:lm.ino(),uid:fm.uid(),link_count:fm.nlink()}})
}

/// Reserve a fully populated and maximally escaped claim/status for every
/// known member, not just the abbreviated current progress representation.
fn reserve_cohort(j:&JobSnapshot,root:&Path,session:&SessionId)->Result<(),NativeError>{
    let mut projected=j.clone();let mut claims=Vec::new();let mut projected_bytes=0u64;
    for f in projected.body["cohort"]["files"].members(){
        let c=ConsentFile::parse(f)?;let size=c.declared_size.as_deref().and_then(|s|s.parse::<u64>().ok()).filter(|n|*n>0).unwrap_or(1);
        if size>contract::MAX_MEMBER_BYTES{return Err(NativeError::new("storageFull","A selected member exceeds the transfer limit."));}
        projected_bytes=projected_bytes.checked_add(size).filter(|n|*n<=contract::MAX_COHORT_BYTES).ok_or_else(||NativeError::new("storageFull","The selected cohort exceeds the transfer limit."))?;
        let mut receipt=c.json();receipt["measuredSize"]=size.to_string().into();receipt["sha256"]="f".repeat(64).into();receipt["providerChecksum"]=if c.md5.is_some(){"matched"}else{"absent"}.into();
        claims.push(object!{fileId:c.file_id,claimId:"f".repeat(32),state:"sealed",claimFsid:"f".repeat(16),dirDevice:u64::MAX.to_string(),dirInode:u64::MAX.to_string(),fileDevice:u64::MAX.to_string(),fileInode:u64::MAX.to_string(),leaseDevice:u64::MAX.to_string(),leaseInode:u64::MAX.to_string(),uid:u32::MAX.to_string(),linkCount:"1",durableOffset:size.to_string(),prefixSha256:"f".repeat(64),strongEtag:format!("\"{}\"", "\\".repeat(1022)),effectiveUriSha256:"f".repeat(64),total:size.to_string(),epoch:"f".repeat(32),receipt:receipt,cleanup:object!{cursor:"3",intent:object!{kind:"directory",device:u64::MAX.to_string(),inode:u64::MAX.to_string()}}});
    }
    projected.body["claims"]=JsonValue::from(claims);projected.body["sequence"]=u64::MAX.to_string().into();projected.body["lastControl"]=object!{controlSeq:u64::MAX.to_string(),action:"abandon",result:"rejected"};
    projected.body["error"]=object!{kind:"providerUnavailable",message:"\"".repeat(512),retryAfterSeconds:"86400"};projected.body["cleanupPending"]=true.into();
    let progress=object!{epoch:"f".repeat(32),volatileSequence:u64::MAX.to_string(),bytesWritten:"0",durableOffset:"0",total:JsonValue::Null};
    let mut status=job_store::status(&projected,root,session,Some(progress)).map_err(|_|NativeError::new("storageFull","The complete remote cohort cannot fit in status."))?;
    // Worst-case digit lengths are charged independently of whether all such
    // values could coexist under the aggregate byte cap.
    for c in projected.body["claims"].members_mut(){c["durableOffset"]=contract::MAX_MEMBER_BYTES.to_string().into();c["total"]=contract::MAX_MEMBER_BYTES.to_string().into();c["receipt"]["measuredSize"]=contract::MAX_MEMBER_BYTES.to_string().into();}
    for m in status["cohort"]["members"].members_mut(){m["durableOffset"]=contract::MAX_MEMBER_BYTES.to_string().into();m["total"]=contract::MAX_MEMBER_BYTES.to_string().into();m["receipt"]["measuredSize"]=contract::MAX_MEMBER_BYTES.to_string().into();}
    status["progress"]["bytesWritten"]=contract::MAX_MEMBER_BYTES.to_string().into();status["progress"]["durableOffset"]=contract::MAX_MEMBER_BYTES.to_string().into();status["progress"]["total"]=contract::MAX_MEMBER_BYTES.to_string().into();
    let token="f".repeat(32);let hash="f".repeat(64);let operation="x".repeat(64);
    projected.body["cohort"]["fingerprint"]=hash.clone().into();projected.body["cohort"]["complete"]=true.into();
    projected.body["prepare"]=object!{prepareKey:token.clone(),profileId:projected.input["profileId"].clone(),expectedRevision:u64::MAX.to_string(),acknowledgeRebase:true};
    projected.body["commit"]=if projected.input["version"]==2{object!{method:"native.remote.replaceCommit",params:object!{operationId:operation.clone(),jobId:projected.input["jobId"].clone(),profileId:projected.input["profileId"].clone(),modId:projected.input["modId"].clone(),purpose:"reinstall",initialRevision:u64::MAX.to_string(),expectedRevision:u64::MAX.to_string(),targetDigest:hash.clone(),afterRecordDigest:hash.clone(),consentFingerprint:hash.clone(),cohortFingerprint:hash.clone(),selectionDigest:hash.clone()}}}else{object!{method:"native.remote.commit",params:object!{operationId:operation.clone(),jobId:projected.input["jobId"].clone(),profileId:projected.input["profileId"].clone(),expectedRevision:u64::MAX.to_string(),consentFingerprint:hash.clone(),cohortFingerprint:hash.clone(),selectionDigest:hash.clone()}}};
    projected.body["state"]="discoveringFamily".into();status["cohort"]["fingerprint"]=hash.clone().into();status["cohort"]["complete"]=true.into();status["state"]="discoveringFamily".into();status["reviewRevision"]=u64::MAX.to_string().into();status["prepareKey"]=token.clone().into();status["operationId"]=operation.clone().into();
    let candidates=JsonValue::from((0..99u64).map(|i|format!("{i:064x}")).collect::<Vec<_>>());let options=JsonValue::from((0..99u64).map(|i|format!("{i:032x}")).collect::<Vec<_>>());
    let summary=object!{selectedCandidateIds:candidates,selectedOptionIds:options,readyCount:"1024",selectedCount:"99",retainedCount:"925",uniquePayloadBytes:contract::MAX_PAYLOAD_BYTES.to_string(),retainedLogicalBytes:contract::MAX_PAYLOAD_BYTES.to_string(),sourceBytes:contract::MAX_COHORT_BYTES.to_string(),diagnosticCount:"10000"};
    let (method,result)=if projected.input["version"]==2{
        let review=object!{requestKey:token.clone(),phase:"ready",reviewToken:token,selectionDigest:hash.clone(),afterRecordDigest:hash,summary:summary,error:JsonValue::Null};
        ("native.remote.replaceStatus",object!{version:1,root:root.to_str(),sessionId:session.as_str(),purpose:"reinstall",modId:projected.input["modId"].clone(),targetDigest:projected.input["targetDigest"].clone(),status:status,review:review})
    }else{
        status["review"]=object!{requestKey:token.clone(),phase:"ready",reviewToken:token,selectionDigest:hash,modId:operation,summary:summary,error:JsonValue::Null};("native.remote.status",status)
    };
    if store::canonical(&projected.body).len()>128*1024{return Err(NativeError::new("storageFull","The complete remote cohort cannot fit in durable history."));}
    let envelope=object!{jsonrpc:"2.0",id:i64::MIN,result:result};
    if !contract::result_envelope_ok(method,&envelope){return Err(NativeError::new("storageFull","The complete remote cohort cannot fit in status."));}Ok(())
}
fn raw_companions(auth:&FreshAuthorization,root:&ConsentFile,dir:&crate::vpk::VpkDirectory)->Result<Vec<ConsentFile>,NativeError>{
    let name=&root.original_name;if !name.to_ascii_lowercase().ends_with(".vpk")||name.len()<4{return Err(NativeError::new("layoutUnsupported","The selected raw package name is unavailable."));}
    let base=&name[..name.len()-4];let stem=base.strip_suffix("_dir").unwrap_or(base);let mut members=Vec::new();
    let rows=auth.candidates();for index in crate::vpk::required_archives(dir){
        let expected=format!("{stem}_{index:03}.vpk");let mut matches=Vec::new();
        for row in &rows {let id=row["fileKey"]["fileId"].as_str().ok_or_else(ambiguous)?;let c=auth.consent(id).ok_or_else(ambiguous)?;
            if c.list==root.list&&c.original_name.eq_ignore_ascii_case(&expected){matches.push(c.clone());}}
        if matches.len()!=1||matches[0].original_name!=expected||matches[0].file_id==root.file_id{return Err(NativeError::new("layoutUnsupported","The selected package requires an ambiguous or missing companion."));}
        members.push(matches.remove(0));
    }
    members.sort_by_key(|c|c.file_id.parse::<u64>().unwrap_or(u64::MAX));if members.len()+1>contract::MAX_MEMBERS||members.windows(2).any(|p|p[0].file_id==p[1].file_id){return Err(NativeError::new("layoutUnsupported","The package family exceeds its member limit."));}Ok(members)
}

struct ActiveTransfer {
    job:String,file:ConsentFile,claim:Claim,child:TransferChild,epoch:String,
    offset:u64,if_range:Option<String>,uri_digest:Option<String>,total:Option<u64>,http_length:Option<u64>,
    finished:Option<JsonValue>,failed:Option<NativeError>,abort_sent:bool,volatile:u64,canonical_target:Option<String>,
}
struct HashInput {active:ActiveTransfer,exclusive:File,result:JsonValue,written:u64,inspect:bool,directory:Option<crate::vpk::VpkDirectory>,layout_error:Option<NativeError>}
struct HashWorker {job:String,handle:JoinHandle<Result<HashInput,NativeError>>}
struct DiscoveryInput {job:String,root:ConsentFile,reader:File,exclusive:File,device:u64,inode:u64,sha256:String,size:u64,directory:Option<crate::vpk::VpkDirectory>}
struct DiscoveryWorker {job:String,handle:JoinHandle<Result<DiscoveryInput,NativeError>>}
struct ResumeInput {job:String,file:ConsentFile,claim:Claim,offset:u64,etag:Option<String>,uri:Option<String>,prefix:Option<String>,reset:bool}
struct ResumeWorker {job:String,handle:JoinHandle<Result<ResumeInput,NativeError>>}
fn hash_prefix(mut input:ResumeInput)->Result<ResumeInput,NativeError>{
    if input.offset>0{let mut digest=Sha256::new();let mut remaining=input.offset;let mut buf=[0u8;65536];while remaining>0{let amount=remaining.min(buf.len() as u64) as usize;let n=input.claim.reader.read(&mut buf[..amount]).map_err(|_|ambiguous())?;if n==0{return Err(ambiguous());}digest.update(&buf[..n]);remaining-=n as u64;}
        if Some(format!("{:x}",digest.finalize()))!=input.prefix{return Err(ambiguous());}}
    let m=input.claim.reader.metadata().map_err(|_|ambiguous())?;let id=&input.claim.identity;if !private_file(&m)||(m.dev(),m.ino(),m.uid(),m.nlink())!=(id.data_device,id.data_inode,id.uid,id.link_count)||m.len()<input.offset{return Err(ambiguous());}Ok(input)
}
fn hash_discovery(mut input:DiscoveryInput)->Result<DiscoveryInput,NativeError>{
    let mut digest=Sha256::new();let mut buf=[0u8;65536];let mut count=0u64;loop{let n=input.reader.read(&mut buf).map_err(|_|ambiguous())?;if n==0{break;}count=count.checked_add(n as u64).filter(|n|*n<=contract::MAX_MEMBER_BYTES).ok_or_else(ambiguous)?;digest.update(&buf[..n]);}
    let meta=input.reader.metadata().map_err(|_|ambiguous())?;if !private_file(&meta)||(meta.dev(),meta.ino(),meta.len())!=(input.device,input.inode,input.size)||count!=input.size||format!("{:x}",digest.finalize())!=input.sha256{return Err(err_kind("transportChecksum"));}
    if input.root.original_name.to_ascii_lowercase().ends_with(".vpk"){input.directory=Some(crate::vpk::inspect_held_directory_controlled(&mut input.reader,&crate::prepare_control::PrepareControl::never_cancelled()).map_err(|_|err_kind("layoutUnsupported"))?);}Ok(input)
}
fn hash_owned(mut input:HashInput)->Result<HashInput,NativeError>{
    let meta=input.active.claim.reader.metadata().map_err(|_|ambiguous())?;let id=&input.active.claim.identity;
    if !private_file(&meta)||(meta.dev(),meta.ino(),meta.uid(),meta.nlink(),meta.len())!=(id.data_device,id.data_inode,id.uid,id.link_count,input.written){return Err(ambiguous());}
    input.active.claim.reader.seek(SeekFrom::Start(0)).map_err(|_|ambiguous())?;let mut digest=Sha256::new();let mut count=0u64;let mut buf=[0u8;65536];
    loop{let n=input.active.claim.reader.read(&mut buf).map_err(|_|ambiguous())?;if n==0{break;}count=count.checked_add(n as u64).filter(|n|*n<=contract::MAX_MEMBER_BYTES).ok_or_else(ambiguous)?;digest.update(&buf[..n]);}
    let after=input.active.claim.reader.metadata().map_err(|_|ambiguous())?;
    if !private_file(&after)||(after.dev(),after.ino(),after.uid(),after.nlink(),after.len())!=(id.data_device,id.data_inode,id.uid,id.link_count,input.written)||count!=input.written||format!("{:x}",digest.finalize())!=input.result["sha256"].as_str().unwrap_or(""){return Err(err_kind("transportChecksum"));}
    if input.inspect{match crate::vpk::inspect_held_directory_controlled(&mut input.active.claim.reader,&crate::prepare_control::PrepareControl::never_cancelled()){Ok(directory)=>input.directory=Some(directory),Err(_)=>input.layout_error=Some(err_kind("layoutUnsupported"))}}
    Ok(input)
}
pub(crate) fn free_bytes(root:&Path)->Result<u64,NativeError>{
    let bulk=crate::storage::root(root)?;
    Ok(disk_space(&bulk)?.1)
}
/// Filesystem usage at the supplied physical path (used, available bytes).
pub(crate) fn disk_space(root:&Path)->Result<(u64,u64),NativeError>{
    #[repr(C)] struct Stat{bsize:std::ffi::c_ulong,frsize:std::ffi::c_ulong,blocks:u64,bfree:u64,bavail:u64,files:u64,ffree:u64,favail:u64,fsid:std::ffi::c_ulong,flag:std::ffi::c_ulong,namemax:std::ffi::c_ulong,spare:[i32;6]}
    unsafe extern "C"{fn statvfs(path:*const i8,result:*mut Stat)->i32;}
    let path=CString::new(root.as_os_str().as_encoded_bytes()).map_err(|_|ambiguous())?;let mut value=std::mem::MaybeUninit::<Stat>::uninit();
    if unsafe{statvfs(path.as_ptr(),value.as_mut_ptr())}!=0{return Err(ambiguous());}let value=unsafe{value.assume_init()};Ok((value.blocks.saturating_sub(value.bfree).checked_mul(value.frsize as u64).unwrap_or(0),value.bavail.checked_mul(value.frsize as u64).unwrap_or(0)))
}
fn claim_index(j:&JobSnapshot,id:&str)->Result<usize,NativeError>{j.body["claims"].members().position(|c|c["fileId"]==id).ok_or_else(ambiguous)}
fn cmd(epoch:&str,kind:&str,data:JsonValue)->Result<TransferCommandV1,NativeError>{TransferCommandV1::parse(object!{version:1,kind:kind,epoch:epoch,data:data})}
fn err_kind(kind:&str)->NativeError{
    let message=match kind{
        "hostNotAllowed"=>"The provider download redirect host is not authorized.",
        _=>"The remote transfer could not be completed.",
    };
    NativeError::new(kind,message)
}
impl Worker {
    fn mark_failed(&mut self,id:&str,e:NativeError){
        self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.remove(id);
        if self.store.as_ref().is_some_and(|db|verify_claim_set(db).is_err()){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.claims_admitted=false;s.reconciled=false;s.phase="waiting";}
        let Some(db)=self.store.as_mut() else{return;};let Ok(job)=JobId::parse(id) else{return;};let Ok(Some(mut j))=db.get(&job) else{return;};
        if matches!(j.body["state"].as_str(),Some("cancelRequested"|"cancelled"|"abandoned"|"paused")){return;}
        j.body["state"]=if e.kind=="needsSelection"{"needsSelection"}else{"failed"}.into();j.body["error"]=failure(&e);
        j.body["cleanupPending"]=(j.body["claims"].members().any(|c|c["state"]!="cleaned")).into();
        if let Ok(j)=db.transition(&job,JobTransition::Body(j.body)){publish(&self.shared,j);}
    }
    fn start_next_transfer(&mut self){
        if self.active.is_some()||self.hashing.is_some()||self.discovery.is_some()||self.resume_hash.is_some()||self.transfer_ticket.as_ref().is_some_and(|t|!t.load(Ordering::Acquire)){return;}
        if !self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).reconciled{return;}
        let candidate={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());self.job_authorizations.keys().find_map(|id|{
            let j=s.jobs.get(id)?;if !matches!(j.body["state"].as_str(),Some("resolving"|"transferring")){return None;}
            let file=j.body["cohort"]["files"].members().find(|f|!j.body["claims"].members().any(|c|c["fileId"]==f["fileId"]&&!c["receipt"].is_null()))?;
            Some((id.clone(),file.clone()))})};
        let Some((id,file))=candidate else{
            let pending={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());self.job_authorizations.keys().find(|id|s.jobs.get(*id).is_some_and(|j|matches!(j.body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily"))&&j.body["cohort"]["complete"]==false&&j.body["claims"].members().any(|c|c["fileId"]==j.input["rootFileId"]&&!c["receipt"].is_null()))).cloned()};
            if let Some(id)=pending{if let Err(e)=self.discover_sealed(&id){self.mark_failed(&id,e);self.job_authorizations.remove(&id);}}return;
        };let result=self.launch_transfer(&id,&file);if let Err(e)=result{self.mark_failed(&id,e);self.job_authorizations.remove(&id);}
    }
    fn launch_transfer(&mut self,id:&str,value:&JsonValue)->Result<(),NativeError>{
        let file=ConsentFile::parse(value)?;if !self.job_authorizations.get(id).is_some_and(|a|a.valid_at(Instant::now())){return Err(err_kind("providerUnavailable"));}
        let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(id)?;let current=db.get(&job)?.ok_or_else(ambiguous)?;reserve_cohort(&current,&self.root,&self.session)?;
        let prior=current.body["claims"].members().any(|c|c["fileId"].as_str()==Some(file.file_id.as_str()));
        if prior{
            let input=prepare_resume(db,&job,&file,self.attempt_modes.get(id).map(String::as_str).unwrap_or("resume"))?;
            let holder=Arc::new(Mutex::new(Some(input)));let worker_holder=holder.clone();let handle=thread::Builder::new().name("remote-prefix-hash".into()).spawn(move||{let input=worker_holder.lock().unwrap_or_else(|p|p.into_inner()).take().ok_or_else(ambiguous)?;hash_prefix(input)}).map_err(|_|err_kind("transportStorage"))?;
            self.resume_hash=Some(ResumeWorker{job:id.into(),handle});return Ok(());
        }
        let claim=create_claim(db,&self.shared,&job,&file)?;self.spawn_claim(id,file,claim,0,None,None)
    }
    fn spawn_claim(&mut self,id:&str,file:ConsentFile,mut claim:Claim,offset:u64,if_range:Option<String>,uri_digest:Option<String>)->Result<(),NativeError>{
        let auth=self.job_authorizations.get(id).ok_or_else(unknown)?;let now=Instant::now();let url=auth.request_url(&self.session,&file.file_id,now).ok_or_else(||err_kind("providerUnavailable"))?.to_owned();
        let hosts=auth.allowed_hosts(&self.session,now).ok_or_else(||err_kind("providerUnavailable"))?.iter().cloned().collect::<Vec<_>>();
        let denied=auth.denied_hosts(&self.session,now).ok_or_else(||err_kind("providerUnavailable"))?.iter().cloned().collect::<Vec<_>>();
        let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(id)?;
        let used=verify_claim_set(db)?;let free=free_bytes(&self.root)?.saturating_sub(1024*1024*1024);let remaining=contract::MAX_COHORT_BYTES.checked_sub(used).ok_or_else(ambiguous)?;
        let cap=contract::MAX_MEMBER_BYTES.min(offset.saturating_add(remaining)).min(offset.saturating_add(free));if cap<=offset{return Err(err_kind("storageFull"));}
        if file.declared_size.as_deref().and_then(|x|x.parse::<u64>().ok()).is_some_and(|n|n>0&&n>cap){return Err(err_kind("storageFull"));}
        let epoch=contract::TransferEpoch::generate()?;
        let mut j=db.get(&job)?.ok_or_else(ambiguous)?;let index=claim_index(&j,&file.file_id)?;j.body["claims"][index]["epoch"]=epoch.as_str().into();j.body["state"]="transferring".into();j.body["error"]=JsonValue::Null;
        let j=db.transition(&job,JobTransition::Body(j.body))?;publish(&self.shared,j);
        if !self.job_authorizations.get(id).is_some_and(|a|a.valid_at(Instant::now())){return Err(err_kind("providerUnavailable"));}
        let identity=claim.identity.clone();let fds=contract::ClaimSpawnFds{write_fd:claim.writer.take().ok_or_else(ambiguous)?.into(),read_fd:claim.reader.try_clone().map_err(|_|ambiguous())?.into(),shared_lease:claim.shared.as_ref().ok_or_else(ambiguous)?.try_clone().map_err(|_|ambiguous())?.into(),identity};
        let start=TransferStartV1::parse(object!{version:1,kind:"start",epoch:epoch.as_str(),requestUrl:url,allowedHosts:JsonValue::from(hosts),deniedHosts:JsonValue::from(denied),offset:offset.to_string(),ifRange:if_range.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null),uriSha256:uri_digest.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null),byteCap:cap.to_string(),claimIdentity:object!{device:claim.identity.data_device.to_string(),inode:claim.identity.data_inode.to_string(),uid:claim.identity.uid.to_string()}})?;
        let child=TransferChild::start(start,fds)?;
        self.transfer_ticket=Some(child.retirement_ticket());self.active=Some(ActiveTransfer{job:id.into(),file,claim,child,epoch:epoch.as_str().into(),offset,if_range,uri_digest,total:None,http_length:None,finished:None,failed:None,abort_sent:false,volatile:0,canonical_target:None});Ok(())
    }
    fn finish_resume(&mut self,mut input:ResumeInput)->Result<(),NativeError>{
        if !self.job_authorizations.get(&input.job).is_some_and(|a|a.valid_at(Instant::now())){return Err(err_kind("providerUnavailable"));}
        let db=self.store.as_mut().ok_or_else(unknown)?;verify_claim_set(db)?;let id=JobId::parse(&input.job)?;let mut j=db.get(&id)?.ok_or_else(unknown)?;
        if !matches!(j.body["state"].as_str(),Some("resolving"|"transferring")){return Ok(());}let i=claim_index(&j,&input.file.file_id)?;
        if j.body["claims"][i]["durableOffset"].as_str().and_then(|s|s.parse::<u64>().ok()).unwrap_or(0)!=input.offset||j.body["claims"][i]["prefixSha256"].as_str()!=input.prefix.as_deref(){return Err(ambiguous());}
        let meta=input.claim.reader.metadata().map_err(|_|ambiguous())?;let identity=&input.claim.identity;if !private_file(&meta)||(meta.dev(),meta.ino(),meta.uid(),meta.nlink())!=(identity.data_device,identity.data_inode,identity.uid,identity.link_count)||meta.len()<input.offset{return Err(ambiguous());}
        let offset=if input.reset{0}else{input.offset};if meta.len()!=offset{let writer=input.claim.writer.as_ref().ok_or_else(ambiguous)?;writer.set_len(offset).map_err(|_|ambiguous())?;writer.sync_all().map_err(|_|ambiguous())?;}
        if offset==0{j.body["claims"][i]["durableOffset"]="0".into();j.body["claims"][i]["prefixSha256"]=JsonValue::Null;j.body["claims"][i]["strongEtag"]=JsonValue::Null;j.body["claims"][i]["effectiveUriSha256"]=JsonValue::Null;j.body["claims"][i]["total"]=JsonValue::Null;j.body["claims"][i]["state"]="owned".into();let next=db.transition(&id,JobTransition::Body(j.body))?;publish(&self.shared,next);input.etag=None;input.uri=None;}
        verify_claim_set(db)?;let lease=input.claim.shared.as_ref().ok_or_else(ambiguous)?;if unsafe{flock(lease.as_raw_fd(),1|4)}!=0{return Err(ambiguous());}
        let job=input.job.clone();self.spawn_claim(&job,input.file,input.claim,offset,input.etag,input.uri)
    }
    fn tick_transfer(&mut self){
        let Some(mut active)=self.active.take() else{return;};
        let status={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.jobs.get(&active.job).map(|j|j.body["state"].as_str().unwrap_or("").to_owned())};
        let abort=if self.shared.close.load(Ordering::Acquire){Some("close")}else{match status.as_deref(){Some("paused")=>Some("pause"),Some("cancelRequested"|"cancelled"|"abandoned")=>Some("cancel"),_ if active.failed.is_some()=>Some("failure"),_=>None}};
        if let Some(reason)=abort {if !active.abort_sent{if active.child.accepts_abort(){match cmd(&active.epoch,"abort",object!{reason:reason}).and_then(|c|active.child.send(c)){Ok(())=>active.abort_sent=true,Err(e)=>{active.abort_sent=true;if active.failed.is_none(){active.failed=Some(e);}}}}else{active.abort_sent=true;}}}
        match active.child.poll(){Ok(events)=>for event in events {if active.failed.is_none()&&!active.abort_sent{if let Err(e)=self.transfer_event(&mut active,event.json()){active.failed=Some(e);}}},Err(e)=>{if active.failed.is_none(){active.failed=Some(e);}}}
        let exited=active.child.is_reaped();
        if exited&&(active.abort_sent||active.failed.is_some()||active.finished.is_some()){
            let id=active.job.clone();if matches!(abort,Some("pause"|"cancel"|"close")){self.after_abort(&active);}else if let Some(e)=active.failed.take(){self.mark_failed(&id,e);}else if active.child.exit_success()!=Some(true){self.mark_failed(&id,err_kind("transportNetwork"));}
            else if let Err(e)=self.seal_or_pause(active){self.mark_failed(&id,e);}return;
        }
        self.active=Some(active);
    }
    fn transfer_event(&mut self,a:&mut ActiveTransfer,event:&JsonValue)->Result<(),NativeError>{
        if event["epoch"].as_str()!=Some(a.epoch.as_str()){return Err(err_kind("transportFraming"));}let d=&event["data"];
        match event["kind"].as_str().unwrap_or("") {
            "headers"=>{
                self.validate_root()?;
                let status=d["status"].as_str().and_then(|s|s.parse::<u16>().ok()).ok_or_else(||err_kind("transportFraming"))?;
                let url=d["url"].as_str().ok_or_else(||err_kind("transportFraming"))?;
                let auth=self.job_authorizations.get(&a.job).ok_or_else(unknown)?;if !auth.allows_header_url(&self.session,url,a.canonical_target.as_deref(),Instant::now()){return Err(err_kind("hostNotAllowed"));}
                if matches!(status,301|302|303|307|308){
                    if a.canonical_target.is_some(){return Err(err_kind("hostNotAllowed"));}
                    let loc=d["headers"].members().filter(|h|h["name"].as_str().is_some_and(|n|n.eq_ignore_ascii_case("location"))).map(|h|h["value"].as_str().unwrap_or("")).collect::<Vec<_>>();
                    if loc.len()!=1{return Err(err_kind("transportFraming"));}let next=transfer::resolve_redirect(url,loc[0].trim_matches([' ','\t']))?;if !auth.allows_redirect(&self.session,url,&next,Instant::now()){return Err(err_kind("hostNotAllowed"));}
                    let range=a.uri_digest.as_ref().is_some_and(|h|transfer::effective_uri_sha256(&next).ok().as_ref()==Some(h));
                    let data=object!{mode:"redirect",url:next.clone(),rangeStart:if range{JsonValue::from(a.offset.to_string())}else{JsonValue::Null},ifRange:if range{a.if_range.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null)}else{JsonValue::Null}};
                    a.child.send(cmd(&a.epoch,"allow",data)?)?;
                    a.canonical_target=if auth.allows_url(&self.session,&next,Instant::now()){None}else{Some(next)};
                    return Ok(());
                }
                if !matches!(status,200|206)||d["identityEstablished"]!=true{return Err(err_kind("transportFraming"));}
                let mut length=None;let mut total=None;
                for h in d["headers"].members(){let n=h["name"].as_str().unwrap_or("");let value=h["value"].as_str().unwrap_or("").trim_matches([' ','\t']);
                    if n.eq_ignore_ascii_case("content-length"){length=value.parse::<u64>().ok();}
                    if n.eq_ignore_ascii_case("content-range"){total=value.rsplit_once('/').and_then(|(_,v)|v.parse::<u64>().ok());}}
                if status==200{total=length;}
                if a.offset>0{let db=self.store.as_ref().ok_or_else(unknown)?;let j=db.get(&JobId::parse(&a.job)?)?.ok_or_else(unknown)?;let i=claim_index(&j,&a.file.file_id)?;let prior=j.body["claims"][i]["total"].as_str().and_then(|s|s.parse::<u64>().ok());if prior.is_some()&&prior!=total{return Err(err_kind("transportFraming"));}}
                if let Some(provider)=a.file.declared_size.as_deref().and_then(|s|s.parse::<u64>().ok()).filter(|n|*n>0){if total.is_some_and(|n|n!=provider){return Err(err_kind("transportChecksum"));}}
                a.http_length=length;a.total=total;
                let ranged=a.offset>0;let data=object!{mode:"body",url:url,rangeStart:if ranged{JsonValue::from(a.offset.to_string())}else{JsonValue::Null},ifRange:if ranged{a.if_range.as_deref().map(JsonValue::from).unwrap_or(JsonValue::Null)}else{JsonValue::Null}};
                a.child.send(cmd(&a.epoch,"allow",data)?)
            },
            "progress"=>{
                let written=d["bytesWritten"].as_str().and_then(|s|s.parse::<u64>().ok()).ok_or_else(||err_kind("transportFraming"))?;let db=self.store.as_ref().ok_or_else(unknown)?;let j=db.get(&JobId::parse(&a.job)?)?.ok_or_else(unknown)?;let i=claim_index(&j,&a.file.file_id)?;
                let durable=j.body["claims"][i]["durableOffset"].as_str().and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);if durable>written{return Err(err_kind("transportFraming"));}a.volatile=a.volatile.checked_add(1).ok_or_else(unknown)?;
                let progress=object!{epoch:a.epoch.clone(),volatileSequence:a.volatile.to_string(),bytesWritten:written.to_string(),durableOffset:durable.to_string(),total:d["total"].clone()};
                self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.insert(a.job.clone(),progress);Ok(())
            },
            "checkpoint"=>{
                crate::catalog::store::reserve(&self.root).map_err(|_|err_kind("storageFull"))?;let db=self.store.as_mut().ok_or_else(unknown)?;let id=JobId::parse(&a.job)?;let mut j=db.get(&id)?.ok_or_else(unknown)?;let i=claim_index(&j,&a.file.file_id)?;
                verify_claim_set(db)?;let held=a.claim.reader.metadata().map_err(|_|ambiguous())?;let directory=a.claim.directory.metadata().map_err(|_|ambiguous())?;let lease=a.claim.shared.as_ref().ok_or_else(ambiguous)?.metadata().map_err(|_|ambiguous())?;
                if (held.dev(),held.ino(),held.uid(),held.nlink())!=(a.claim.identity.data_device,a.claim.identity.data_inode,a.claim.identity.uid,a.claim.identity.link_count)||(directory.dev(),directory.ino())!=(a.claim.identity.directory_device,a.claim.identity.directory_inode)||(lease.dev(),lease.ino())!=(a.claim.identity.lease_device,a.claim.identity.lease_inode){return Err(ambiguous());}
                if j.body["claims"][i]["epoch"].as_str()!=Some(a.epoch.as_str()){return Err(err_kind("transportFraming"));}
                j.body["claims"][i]["state"]="partial".into();for (target,source) in [("durableOffset","offset"),("prefixSha256","prefixSha256"),("strongEtag","strongEtag"),("effectiveUriSha256","effectiveUriSha256"),("total","total")]{j.body["claims"][i][target]=d[source].clone();}
                let next=db.transition(&id,JobTransition::Body(j.body))?;verify_claim_set(db)?;let offset=d["offset"].clone();let hash=d["prefixSha256"].clone();
                {let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.jobs.insert(a.job.clone(),next);a.volatile=a.volatile.checked_add(1).ok_or_else(unknown)?;s.progress.insert(a.job.clone(),object!{epoch:a.epoch.clone(),volatileSequence:a.volatile.to_string(),bytesWritten:offset.clone(),durableOffset:offset.clone(),total:d["total"].clone()});}
                a.child.send(cmd(&a.epoch,"checkpointAck",object!{offset:offset,prefixSha256:hash})?)
            },
            "finished"=>{a.finished=Some(d.clone());Ok(())},
            "error"=>Err(err_kind(d["kind"].as_str().unwrap_or("transportNetwork"))),
            _=>Err(err_kind("transportFraming")),
        }
    }
    fn after_abort(&mut self,a:&ActiveTransfer){
        self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.remove(&a.job);
        let Some(db)=self.store.as_mut() else{return;};let Ok(id)=JobId::parse(&a.job) else{return;};let Ok(Some(mut j))=db.get(&id) else{return;};
        if j.body["state"]=="cancelRequested"{j.body["state"]="cancelled".into();j.body["cleanupPending"]=(j.body["claims"].len()>0).into();if let Ok(next)=db.transition(&id,JobTransition::Body(j.body)){publish(&self.shared,next);}}
        self.job_authorizations.remove(&a.job);
    }
    fn seal_or_pause(&mut self,mut a:ActiveTransfer)->Result<(),NativeError>{
        let result=a.finished.take().ok_or_else(||err_kind("transportFraming"))?;let written=result["bytesWritten"].as_str().and_then(|s|s.parse::<u64>().ok()).filter(|n|*n>0).ok_or_else(||err_kind("transportFraming"))?;
        if result["networkError"]!="NoError"{return Err(err_kind("transportNetwork"));}
        let id=JobId::parse(&a.job)?;let db=self.store.as_mut().ok_or_else(unknown)?;let mut j=db.get(&id)?.ok_or_else(unknown)?;
        if matches!(j.body["state"].as_str(),Some("cancelRequested"|"cancelled"|"paused"|"abandoned")){return Err(err_kind("remoteBusy"));}
        if a.total.is_some_and(|n|written<n){
            let i=claim_index(&j,&a.file.file_id)?;if j.body["claims"][i]["durableOffset"].as_str().and_then(|s|s.parse::<u64>().ok())!=Some(written){return Err(err_kind("transportFraming"));}
            j.body["state"]="paused".into();j.body["error"]=JsonValue::Null;let next=db.transition(&id,JobTransition::Body(j.body))?;publish(&self.shared,next);self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.remove(&a.job);self.job_authorizations.remove(&a.job);return Ok(());
        }
        if a.total.is_some_and(|n|n!=written){return Err(err_kind("transportFraming"));}
        let declared=a.file.declared_size.as_deref().and_then(|s|s.parse::<u64>().ok()).filter(|n|*n>0);
        if declared.is_some_and(|n|n!=written){return Err(err_kind("transportChecksum"));}
        let md5=result["md5"].as_str().ok_or_else(||err_kind("transportFraming"))?;
        if a.file.md5.as_deref().is_some_and(|expected|expected!=md5){return Err(err_kind("transportChecksum"));}
        if !a.http_length.is_some_and(|n|n>0)&&declared.is_none()&&a.file.md5.is_none(){return Err(err_kind("transportFraming"));}
        j.body["state"]="sealing".into();let next=db.transition(&id,JobTransition::Body(j.body))?;publish(&self.shared,next.clone());
        // Parent and child shared descriptions are retired before an independent exclusive open.
        a.claim.shared.take();let exclusive=at(&a.claim.directory,"lease",2,0)?;let lm=exclusive.metadata().map_err(|_|ambiguous())?;
        if !private_file(&lm)||(lm.dev(),lm.ino())!=(a.claim.identity.lease_device,a.claim.identity.lease_inode)||unsafe{flock(exclusive.as_raw_fd(),2|4)}!=0{return Err(ambiguous());}
        verify_claim_set(db)?;let meta=a.claim.reader.metadata().map_err(|_|ambiguous())?;
        if !private_file(&meta)||(meta.dev(),meta.ino(),meta.uid(),meta.nlink(),meta.len())!=(a.claim.identity.data_device,a.claim.identity.data_inode,a.claim.identity.uid,a.claim.identity.link_count,written){return Err(ambiguous());}
        let inspect=next.body["cohort"]["complete"]==false&&a.file.original_name.to_ascii_lowercase().ends_with(".vpk");
        let job=a.job.clone();let input=HashInput{active:a,exclusive,result,written,inspect,directory:None,layout_error:None};
        // The holder retains the exclusive lease if thread creation fails.
        let holder=Arc::new(Mutex::new(Some(input)));let worker_holder=holder.clone();
        let handle=thread::Builder::new().name("remote-seal-hash".into()).spawn(move||{
            let input=worker_holder.lock().unwrap_or_else(|p|p.into_inner()).take().ok_or_else(ambiguous)?;hash_owned(input)
        }).map_err(|_|err_kind("transportStorage"))?;
        self.hashing=Some(HashWorker{job,handle});Ok(())
    }
    fn finish_hash(&mut self,input:HashInput)->Result<(),NativeError>{
        let HashInput{active:a,exclusive,result,written,directory,layout_error,..}=input;let id=JobId::parse(&a.job)?;let db=self.store.as_mut().ok_or_else(unknown)?;
        verify_claim_set(db)?;let meta=a.claim.reader.metadata().map_err(|_|ambiguous())?;if meta.len()!=written||!private_file(&meta)||(meta.dev(),meta.ino())!=(a.claim.identity.data_device,a.claim.identity.data_inode){return Err(ambiguous());}let mut j=db.get(&id)?.ok_or_else(unknown)?;
        if j.body["state"]!="sealing"{self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.remove(&a.job);return Ok(());}
        let index=claim_index(&j,&a.file.file_id)?;
        let mut receipt=a.file.json();receipt["measuredSize"]=written.to_string().into();receipt["sha256"]=result["sha256"].clone();receipt["providerChecksum"]=if a.file.md5.is_some(){"matched"}else{"absent"}.into();
        contract::MemberReceiptV1::parse(&receipt)?;j.body["claims"][index]["state"]="sealed".into();j.body["claims"][index]["durableOffset"]=written.to_string().into();j.body["claims"][index]["prefixSha256"]=result["sha256"].clone();j.body["claims"][index]["total"]=written.to_string().into();j.body["claims"][index]["receipt"]=receipt;
        j.body["state"]="discoveringFamily".into();let next=db.transition(&id,JobTransition::Body(j.body))?;publish(&self.shared,next.clone());
        // Inspection uses the already held claim descriptor while exclusive ownership remains held.
        if next.body["cohort"]["complete"]==false {
            let auth=self.job_authorizations.get(&a.job).filter(|x|x.valid_at(Instant::now())).ok_or_else(||err_kind("providerUnavailable"))?;
            let root=ConsentFile::parse(&next.body["selection"]["rootFile"])?;
            let mut files=vec![root.clone()];
            if root.original_name.to_ascii_lowercase().ends_with(".vpk"){
                let directory=directory.as_ref().ok_or_else(||layout_error.clone().unwrap_or_else(||err_kind("layoutUnsupported")))?;
                files.extend(raw_companions(auth,&root,directory)?);
            }
            if files.iter().any(ConsentFile::archived)&&next.input["acceptArchived"]!=true{return Err(err_kind("providerDenied"));}
            let submission=SubmissionKeyV1::parse(&next.input["submission"])?;let fingerprint=contract::cohort_fingerprint(&submission,&root.file_id,&files)?;
            let mut complete=next;complete.body["cohort"]=object!{fingerprint:fingerprint,complete:true,files:JsonValue::from(files.iter().map(ConsentFile::json).collect::<Vec<_>>())};
            reserve_cohort(&complete,&self.root,&self.session)?;
            let ready=complete.body["cohort"]["files"].members().all(|f|complete.body["claims"].members().any(|c|c["fileId"]==f["fileId"]&&!c["receipt"].is_null()));
            complete.body["state"]=if ready{"downloaded"}else{"transferring"}.into();let next=db.transition(&id,JobTransition::Body(complete.body))?;publish(&self.shared,next);if ready{self.job_authorizations.remove(&a.job);}
        }else{
            let mut complete=next;let ready=complete.body["cohort"]["files"].members().all(|f|complete.body["claims"].members().any(|c|c["fileId"]==f["fileId"]&&!c["receipt"].is_null()));
            complete.body["state"]=if ready{"downloaded"}else{"transferring"}.into();let next=db.transition(&id,JobTransition::Body(complete.body))?;publish(&self.shared,next);if ready{self.job_authorizations.remove(&a.job);}
        }
        drop(exclusive);self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).progress.remove(&a.job);Ok(())
    }
}

fn prepare_resume(db:&RemoteJobStore,job:&JobId,file:&ConsentFile,mode:&str)->Result<ResumeInput,NativeError>{
    verify_claim_set(db)?;let lease=db.lease();let j=db.get(job)?.ok_or_else(ambiguous)?;let index=claim_index(&j,&file.file_id)?;let c=j.body["claims"][index].clone();
    if !matches!(c["state"].as_str(),Some("owned"|"partial")){return Err(ambiguous());}
    let name=claim_name(c["claimId"].as_str().ok_or_else(ambiguous)?);let dir=at(lease.directory(),&name,0x10000,0)?;let dm=dir.metadata().map_err(|_|ambiguous())?;
    let claim_path=lease.bulk_path().join(&name);
    let lock=at(&dir,"lease",2,0)?;let lm=lock.metadata().map_err(|_|ambiguous())?;
    if !private_dir(&dm)||!private_file(&lm)||!expected_pair(&c,"dirDevice","dirInode",&lease,&claim_path,&dir)||!expected_pair(&c,"leaseDevice","leaseInode",&lease,&claim_path.join("lease"),&lock)||unsafe{flock(lock.as_raw_fd(),2|4)}!=0{return Err(ambiguous());}
    let reader=at(&dir,"data",0,0)?;let writer=at(&dir,"data",2,0)?;let fm=reader.metadata().map_err(|_|ambiguous())?;let wm=writer.metadata().map_err(|_|ambiguous())?;
    if !private_file(&fm)||identity(&fm)!=identity(&wm)||!expected_pair(&c,"fileDevice","fileInode",&lease,&claim_path.join("data"),&reader){return Err(ambiguous());}
    let offset=c["durableOffset"].as_str().and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);if fm.len()<offset{return Err(ambiguous());}
    let etag=c["strongEtag"].as_str().map(str::to_owned);let uri=c["effectiveUriSha256"].as_str().map(str::to_owned);let prefix=c["prefixSha256"].as_str().map(str::to_owned);
    if mode!="retry"&&offset>0&&(etag.is_none()||uri.is_none()){return Err(err_kind("transportFraming"));}let (root_device,root_inode)=lease.identity();
    Ok(ResumeInput{job:job.as_str().into(),file:file.clone(),claim:Claim{directory:dir,reader,writer:Some(writer),shared:Some(lock),identity:contract::ClaimIdentity{root_device,root_inode,directory_device:dm.dev(),directory_inode:dm.ino(),data_device:fm.dev(),data_inode:fm.ino(),lease_device:lm.dev(),lease_inode:lm.ino(),uid:fm.uid(),link_count:fm.nlink()}},offset,etag,uri,prefix,reset:mode=="retry"})
}

impl Worker {
    fn discover_sealed(&mut self,id:&str)->Result<(),NativeError>{
        self.job_authorizations.get(id).filter(|a|a.valid_at(Instant::now())).ok_or_else(||err_kind("providerUnavailable"))?;
        let db=self.store.as_mut().ok_or_else(unknown)?;verify_claim_set(db)?;let job=JobId::parse(id)?;let j=db.get(&job)?.ok_or_else(unknown)?;
        if !matches!(j.body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily")){return Err(unknown());}
        let root=ConsentFile::parse(&j.body["selection"]["rootFile"])?;let i=claim_index(&j,&root.file_id)?;let claim=j.body["claims"][i].clone();let receipt=contract::MemberReceiptV1::parse(&claim["receipt"])?;
        let lease=db.lease();let dir=at(lease.directory(),&claim_name(claim["claimId"].as_str().ok_or_else(ambiguous)?),0x10000,0)?;
        let claim_path=lease.bulk_path().join(claim_name(claim["claimId"].as_str().ok_or_else(ambiguous)?));
        let lock=at(&dir,"lease",2,0)?;let lm=lock.metadata().map_err(|_|ambiguous())?;if !private_file(&lm)||!expected_pair(&claim,"leaseDevice","leaseInode",&lease,&claim_path.join("lease"),&lock)||unsafe{flock(lock.as_raw_fd(),2|4)}!=0{return Err(ambiguous());}
        let reader=at(&dir,"data",0,0)?;let rm=reader.metadata().map_err(|_|ambiguous())?;if !private_file(&rm)||!expected_pair(&claim,"fileDevice","fileInode",&lease,&claim_path.join("data"),&reader)||rm.len().to_string()!=receipt.measured_size{return Err(ambiguous());}
        let input=DiscoveryInput{job:id.into(),root,reader,exclusive:lock,device:rm.dev(),inode:rm.ino(),sha256:receipt.sha256,size:rm.len(),directory:None};
        let holder=Arc::new(Mutex::new(Some(input)));let worker_holder=holder.clone();let handle=thread::Builder::new().name("remote-discovery-hash".into()).spawn(move||{
            let input=worker_holder.lock().unwrap_or_else(|p|p.into_inner()).take().ok_or_else(ambiguous)?;hash_discovery(input)
        }).map_err(|_|err_kind("transportStorage"))?;
        self.discovery=Some(DiscoveryWorker{job:id.into(),handle});Ok(())
    }
    fn finish_discovery(&mut self,input:DiscoveryInput)->Result<(),NativeError>{
        let id=input.job.as_str();let auth=self.job_authorizations.get(id).filter(|a|a.valid_at(Instant::now())).ok_or_else(||err_kind("providerUnavailable"))?;
        let db=self.store.as_mut().ok_or_else(unknown)?;verify_claim_set(db)?;let meta=input.reader.metadata().map_err(|_|ambiguous())?;if !private_file(&meta)||(meta.dev(),meta.ino(),meta.len())!=(input.device,input.inode,input.size){return Err(ambiguous());}let job=JobId::parse(id)?;let mut j=db.get(&job)?.ok_or_else(unknown)?;
        if !matches!(j.body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily")){return Ok(());}
        let root=ConsentFile::parse(&j.body["selection"]["rootFile"])?;if root!=input.root{return Err(ambiguous());}let i=claim_index(&j,&root.file_id)?;let receipt=contract::MemberReceiptV1::parse(&j.body["claims"][i]["receipt"])?;
        if receipt.sha256!=input.sha256||receipt.measured_size.parse::<u64>().ok()!=Some(input.size){return Err(ambiguous());}
        let mut files=vec![root.clone()];if root.original_name.to_ascii_lowercase().ends_with(".vpk"){
            files.extend(raw_companions(auth,&root,input.directory.as_ref().ok_or_else(||err_kind("layoutUnsupported"))?)?);}
        if files.iter().any(ConsentFile::archived)&&j.input["acceptArchived"]!=true{return Err(err_kind("providerDenied"));}
        let submission=SubmissionKeyV1::parse(&j.input["submission"])?;let fingerprint=contract::cohort_fingerprint(&submission,&root.file_id,&files)?;
        j.body["cohort"]=object!{fingerprint:fingerprint,complete:true,files:JsonValue::from(files.iter().map(ConsentFile::json).collect::<Vec<_>>())};reserve_cohort(&j,&self.root,&self.session)?;
        let ready=j.body["cohort"]["files"].members().all(|f|j.body["claims"].members().any(|c|c["fileId"]==f["fileId"]&&!c["receipt"].is_null()));j.body["state"]=if ready{"downloaded"}else{"transferring"}.into();
        let j=db.transition(&job,JobTransition::Body(j.body))?;publish(&self.shared,j);drop(input.exclusive);if ready{self.job_authorizations.remove(id);}Ok(())
    }
}

impl Worker {
    fn tick_preparation(&mut self){
        if self.preparation.as_ref().is_none_or(|w|w.result.is_none()&&!w.handle.as_ref().is_some_and(|h|h.is_finished())){return;}
        let mut worker=self.preparation.take().unwrap();
        if worker.result.is_none(){let joined=worker.handle.take().unwrap().join();worker.result=Some(if joined.is_err(){Err(ambiguous())}else{self.preparation_guard.as_ref().and_then(|g|g.result.lock().unwrap_or_else(|p|p.into_inner()).take()).unwrap_or_else(||Err(ambiguous()))});}
        let active={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(&worker.job).is_some_and(|v|!v.release&&!v.released&&v.params["prepareKey"]==worker.key)};
        let release_unconfirmed={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(&worker.job).is_some_and(|v|v.release&&!v.barrier&&v.params["prepareKey"]==worker.key)};
        if release_unconfirmed{self.preparation=Some(worker);return;}
        let eligible=(||->Result<bool,NativeError>{
            let db=self.store.as_ref().ok_or_else(unknown)?;verify_claim_set(db)?;let row=db.get(&JobId::parse(&worker.job)?)?.ok_or_else(unknown)?;
            Ok(active&&row.body["state"]=="preparing"&&row.body["prepare"]["prepareKey"]==worker.key)
        })();
        if eligible.is_err(){self.preparation=Some(worker);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.reconciled=false;return;}
        if matches!(eligible,Ok(true))&&worker.result.as_ref().is_some_and(Result::is_ok){
            let prepared=worker.result.as_ref().unwrap().as_ref().unwrap();
            let verified=prepared.validate_live(&crate::prepare_control::PrepareControl::new()).and_then(|_|{
                let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(&worker.job)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;
                if row.body["state"]!="preparing"||row.body["prepare"]["prepareKey"]!=worker.key{return Err(busy());}
                row.body["state"]="ready".into();let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);Ok(())
            });
            if verified.is_ok(){
                let prepared=Arc::new(worker.result.take().unwrap().unwrap());if let Some(guard)=self.preparation_guard.as_mut(){guard.prepared=Some(Arc::downgrade(&prepared));}self.ready.insert(worker.job.clone(),prepared.clone());
                let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(&worker.job).filter(|v|!v.release&&v.params["prepareKey"]==worker.key){view.prepared=Some(prepared);view.admission=false;}
                return;
            }
            self.preparation=Some(worker);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.reconciled=false;return;
        }
        let failure=match worker.result.take(){Some(Err(e))=>e,Some(Ok(prepared))=>{drop(prepared);NativeError::new("staleReview","The preparation was interrupted.")},None=>ambiguous()};
        let admitted=vec![(worker.job.clone(),worker.key.clone())];
        if worker.authority.recover(&worker.slot,&admitted).is_err(){worker.result=Some(Err(failure));self.preparation=Some(worker);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.reconciled=false;return;}
        let cancelled=self.store.as_ref().and_then(|db|JobId::parse(&worker.job).ok().and_then(|id|db.get(&id).ok().flatten())).is_some_and(|j|matches!(j.body["state"].as_str(),Some("cancelRequested"|"cancelled")));
        if cancelled {self.reconcile_slot=Some(worker.slot.clone());self.preparation_guard=None;let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.reconciled=false;s.phase="waiting";}
        else if active {self.fail_preparation(&worker.job,&worker.key,&failure);self.preparation_guard=None;}
        else if !self.finish_release(&worker.job,&worker.key){worker.result=Some(Err(failure));self.preparation=Some(worker);}else{self.preparation_guard=None;}
    }
    fn tick_review(&mut self){
        if !self.review_worker.as_ref().is_some_and(|w|w.handle.is_finished()){return;}
        let worker=self.review_worker.take().unwrap();let joined=worker.handle.join();
        let result=if joined.is_err(){Err(ambiguous())}else{self.review_guard.as_ref().and_then(|g|g.result.lock().unwrap_or_else(|p|p.into_inner()).take()).unwrap_or_else(||Err(ambiguous()))};
        self.review_guard=None;
        let params={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.get(&worker.job).filter(|v|!v.release&&!v.released&&v.generation==worker.generation&&v.params["prepareKey"]==worker.key&&v.review_params.as_ref().is_some_and(|p|p["requestKey"]==worker.request)).and_then(|v|v.review_params.clone())};
        let Some(params)=params else{
            if let (Some(db),Ok(job))=(self.store.as_mut(),JobId::parse(&worker.job)){if let Ok(Some(mut row))=db.get(&job){if row.body["state"]=="reviewing"&&row.body["prepare"]["prepareKey"]==worker.key{row.body["state"]="ready".into();if let Ok(next)=db.transition(&job,JobTransition::Body(row.body)){publish(&self.shared,next);}}}}
            return;
        };
        let result=result.and_then(|plan|{
            if !self.job_authorizations.get(&worker.job).is_some_and(|a|a.valid_at(Instant::now())){return Err(NativeError::new("staleReview","Fresh authorization expired before review completed."));}
            plan.prepared.validate_live(&crate::prepare_control::PrepareControl::new())?;let db=self.store.as_mut().ok_or_else(unknown)?;verify_claim_set(db)?;
            let job=JobId::parse(&worker.job)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;if row.body["state"]!="reviewing"||row.body["prepare"]["prepareKey"]!=worker.key{return Err(busy());}
            row.body["state"]="ready".into();let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);
            if !self.job_authorizations.get(&worker.job).is_some_and(|a|a.valid_at(Instant::now())){return Err(NativeError::new("staleReview","Fresh authorization expired before review completed."));}Ok(plan)
        });
        match result {
            Ok(plan)=>{let review=match &plan.purpose{RemotePlanPurpose::Add=>object!{requestKey:worker.request,phase:"ready",reviewToken:plan.token.clone(),selectionDigest:plan.selection_digest.clone(),modId:plan.mod_id.clone(),summary:plan.summary.clone(),error:JsonValue::Null},RemotePlanPurpose::Replace{after_record_digest,..}=>object!{requestKey:worker.request,phase:"ready",reviewToken:plan.token.clone(),selectionDigest:plan.selection_digest.clone(),afterRecordDigest:after_record_digest.clone(),summary:plan.summary.clone(),error:JsonValue::Null}};
                let expires=self.job_authorizations.get(&worker.job).map(FreshAuthorization::expires_at);
                let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(&worker.job).filter(|v|!v.release&&v.generation==worker.generation&&v.review_params.as_ref()==Some(&params)){self.plans.insert(worker.job.clone(),(worker.generation,plan));view.review=review;view.review_control=None;view.review_expires=expires;view.admission=false;}}
            Err(e) if e.kind=="staleReview"&&!self.job_authorizations.get(&worker.job).is_some_and(|a|a.valid_at(Instant::now()))=>{
                let restored=(||->Result<(),NativeError>{let db=self.store.as_mut().ok_or_else(unknown)?;let job=JobId::parse(&worker.job)?;let mut row=db.get(&job)?.ok_or_else(unknown)?;
                    if row.body["prepare"]["prepareKey"]!=worker.key{return Err(busy());}if row.body["state"]=="reviewing"{row.body["state"]="ready".into();let next=db.transition(&job,JobTransition::Body(row.body))?;publish(&self.shared,next);}else if row.body["state"]!="ready"{return Err(busy());}Ok(())})();
                if restored.is_ok(){if let Some(view)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get_mut(&worker.job){view.review_control=None;}self.retry_review(params,worker.generation);}else{self.fail_review(&worker.job,&params,worker.generation,&e);}
            },
            Err(e)=>self.fail_review(&worker.job,&params,worker.generation,&e),
        }
    }
    fn tick_preparation_retirement(&mut self){
        if self.shared.recovery_borrowed.load(Ordering::Acquire){return;}
        if self.preparation.is_some()||self.review_worker.is_some(){return;}
        if self.review_guard.as_ref().is_some_and(|g|!g.done.load(Ordering::Acquire)){return;}
        if let Some(guard)=self.review_guard.take(){guard.result.lock().unwrap_or_else(|p|p.into_inner()).take();}
        if self.retiring_preparation.is_none(){
            let id={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.iter().find(|(id,v)|v.release&&v.barrier&&!v.released&&(self.ready.contains_key(*id)||self.preparation_guard.as_ref().is_some_and(|g|&g.job==*id))).map(|(id,_)|id.clone())};
            if let Some(id)=id {
                if self.preparation_guard.as_ref().is_some_and(|g|g.job==id&&!g.done.load(Ordering::Acquire)){return;}
                if let Some(guard)=self.preparation_guard.as_ref(){guard.result.lock().unwrap_or_else(|p|p.into_inner()).take();}
                self.plans.remove(&id);let prepared=self.ready.remove(&id);
                if let Some(view)=self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).preparations.get_mut(&id){view.prepared=None;}
                drop(prepared);if self.preparation_guard.as_ref().and_then(|g|g.prepared.as_ref()).is_some_and(|p|p.strong_count()!=0){return;}
                self.retiring_preparation=self.preparation_guard.take();
            }
        }
        let Some(retiring)=self.retiring_preparation.take() else{return;};
        let safe=(||->Result<(),NativeError>{let db=self.store.as_ref().ok_or_else(unknown)?;let row=db.get(&JobId::parse(&retiring.job)?)?.ok_or_else(unknown)?;
            if !row.body["commit"].is_null(){verify_claim_set(db)?;let input=RemoteCommitInput::parse(row.body["commit"].clone()).map_err(|_|unknown())?;let history=Store::open_history(&self.root).map_err(|_|unknown())?;
                let agrees=match history.exact_any_remote_outcome(&input).map_err(|_|unknown())?{ExactRemoteOutcome::Succeeded(_)=>row.body["state"]=="installed",ExactRemoteOutcome::Failed(_)=>matches!(row.body["state"].as_str(),Some("failed"|"abandoned")),ExactRemoteOutcome::Absent=>false};
                if !agrees||history.journal(input.operation_id()).map_err(|_|unknown())?.is_some(){return Err(unknown());}db.lease().validate()?;}
            Ok(())})();
        if safe.is_err(){self.retiring_preparation=Some(retiring);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.reconciled=false;return;}
        if retiring.authority.recover(&retiring.slot,&[(retiring.job.clone(),retiring.key.clone())]).is_err(){self.retiring_preparation=Some(retiring);let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.reconciled=false;return;}
        let terminal=self.store.as_ref().and_then(|db|JobId::parse(&retiring.job).ok().and_then(|id|db.get(&id).ok().flatten())).is_some_and(|j|matches!(j.body["state"].as_str(),Some("cancelRequested"|"cancelled"))||matches!(j.body["state"].as_str(),Some("installed"|"failed"|"abandoned"))&&!j.body["commit"].is_null());
        if terminal {self.reconcile_slot=Some(retiring.slot.clone());let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(view)=s.preparations.get_mut(&retiring.job){view.released=true;view.prepared=None;view.admission=false;}s.reconciled=false;s.phase="waiting";}
        else if !self.finish_release(&retiring.job,&retiring.key){self.retiring_preparation=Some(retiring);}
    }
    fn tick_reconcile(&mut self){
        if self.shared.recovery_borrowed.load(Ordering::Acquire){return;}
        if !self.shared.startup_done.load(Ordering::Acquire)||self.shared.startup_blocked.load(Ordering::Acquire){return;}
        {let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());if s.reconciled||s.phase=="failed"{return;}}
        if self.commit_worker.is_some()||self.preparation.is_some()||self.preparation_guard.is_some()||!self.ready.is_empty()||self.review_worker.is_some()||self.review_guard.is_some()||self.retiring_preparation.is_some()
            ||self.active.is_some()||self.hashing.is_some()||self.discovery.is_some()||self.resume_hash.is_some()
            ||self.transfer_ticket.as_ref().is_some_and(|t|!t.load(Ordering::Acquire)){return;}
        if self.store.is_none(){
            if self.validate_root().is_ok()&&job_store::empty_namespace(&self.root).is_ok_and(|empty|empty){let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.reconciled=true;s.claims_admitted=true;s.phase="ready";}
            return;
        }
        if self.reconcile_slot.is_none(){match PreparationSlot::try_acquire(&self.root){Ok(slot)=>self.reconcile_slot=Some(slot),Err(_)=>return,}}
        let slot=self.reconcile_slot.as_ref().unwrap().clone();
        let result=(||->Result<bool,NativeError>{
            let db=self.store.as_mut().ok_or_else(unknown)?;verify_claim_set(db)?;let lease=db.lease();slot.validate()?;
            self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).recovery_custody=Some((lease.clone(),slot.clone()));
            let zero="00000000000000000000000000000000";let authority=RemoteQuarantineAuthority::new(lease.clone(),JobId::parse(zero)?,contract::ClaimId::parse(zero)?)?;
            let mut rows=db.snapshots()?;
            // Never release job-owned scratch until every committing row has
            // an exact durable outcome and no remaining central journal.
            for row in &mut rows {if !row.body["commit"].is_null(){
                let input=RemoteCommitInput::parse(row.body["commit"].clone()).map_err(|_|unknown())?;let history=Store::open_history(&self.root).map_err(|_|unknown())?;
                let outcome=history.exact_any_remote_outcome(&input).map_err(|_|unknown())?;if history.journal(input.operation_id()).map_err(|_|unknown())?.is_some(){return Ok(false);}lease.validate()?;
                let terminal_error=match outcome{ExactRemoteOutcome::Succeeded(_)=>None,ExactRemoteOutcome::Failed(error)=>Some(error),ExactRemoteOutcome::Absent=>return Ok(false)};let installed=terminal_error.is_none();
                if row.body["state"]=="committing"{
                    row.body["state"]=if installed{"installed"}else{"failed"}.into();row.body["cleanupPending"]=true.into();row.body["error"]=terminal_error.as_ref().map(failure).unwrap_or(JsonValue::Null);
                    *row=db.transition(&JobId::parse(row.input["jobId"].as_str().ok_or_else(unknown)?)?,JobTransition::Body(row.body.clone()))?;
                }else if installed&&row.body["state"]!="installed"||!installed&&!matches!(row.body["state"].as_str(),Some("failed"|"abandoned")){return Err(unknown());}
            }}
            let scratch=authority.inspect(&slot)?;let mut admitted=Vec::new();
            for item in &scratch {
                let job=rows.iter().find(|j|j.input["jobId"]==item.job_id).ok_or_else(ambiguous)?;
                if job.body["prepare"]["prepareKey"]!=item.nonce{return Err(ambiguous());}
                admitted.push((item.job_id.clone(),item.nonce.clone()));
            }
            if !scratch.is_empty(){authority.recover(&slot,&admitted)?;}
            let mut rows=db.snapshots()?;
            for row in &mut rows {
                if matches!(row.body["state"].as_str(),Some("preparing"|"ready"|"reviewing")){
                    row.body["state"]=if row.body["error"]["kind"]=="needsSelection"{"needsSelection"}else{"downloaded"}.into();row.body["prepare"]=JsonValue::Null;row.body["cleanupPending"]=false.into();
                    let next=db.transition(&JobId::parse(row.input["jobId"].as_str().ok_or_else(unknown)?)?,JobTransition::Body(row.body.clone()))?;*row=next;
                }else if matches!(row.body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily"|"sealing")) && !self.job_authorizations.contains_key(row.input["jobId"].as_str().unwrap_or("")){
                    row.body["state"]="paused".into();let next=db.transition(&JobId::parse(row.input["jobId"].as_str().ok_or_else(unknown)?)?,JobTransition::Body(row.body.clone()))?;*row=next;
                }
            }
            for row in &rows{publish(&self.shared,row.clone());}
            if rows.iter().any(|j|j.body["state"]=="committing"){return Ok(false);}
            if let Some(row)=rows.iter().find(|j|j.body["state"]=="cancelRequested"||matches!(j.body["state"].as_str(),Some("cancelled"|"abandoned"|"installed"))&&j.body["cleanupPending"]==true||j.body["state"]=="failed"&&!j.body["commit"].is_null()&&j.body["cleanupPending"]==true){
                let permit=RemoteCleanupPermit{slot:&slot,lease:&lease};cleanup_step(db,&self.shared,&JobId::parse(row.input["jobId"].as_str().ok_or_else(unknown)?)?,&permit)?;return Ok(false);
            }
            if rows.iter().any(|j|j.body["state"]=="cancelRequested"){return Ok(false);}
            db.purge_terminal(None,20)?;
            let retained=db.snapshots()?;
            self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).jobs=retained.into_iter().map(|j|(j.input["jobId"].as_str().unwrap().to_owned(),j)).collect();
            Ok(true)
        })();
        match result {
            Ok(true)=>{self.reconcile_slot=None;let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.recovery_custody=None;let terminal=s.jobs.iter().filter(|(_,j)|matches!(j.body["state"].as_str(),Some("cancelled"|"abandoned"|"installed"))).map(|(id,_)|id.clone()).collect::<Vec<_>>();for id in terminal{if let Some(view)=s.preparations.get_mut(&id){view.released=true;view.prepared=None;view.admission=false;}}s.claims_admitted=true;s.reconciled=true;s.phase="ready";},
            Ok(false)=>{},
            Err(_)=>{let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.claims_admitted=false;s.reconciled=false;s.phase="waiting";}
        }
    }
    fn tick_hash(&mut self){
        if !self.hashing.as_ref().is_some_and(|h|h.handle.is_finished()){return;}
        let worker=self.hashing.take().unwrap();let id=worker.job;
        match worker.handle.join(){Ok(Ok(input))=>{if let Err(e)=self.finish_hash(input){self.mark_failed(&id,e);}},Ok(Err(e))=>self.mark_failed(&id,e),Err(_)=>self.mark_failed(&id,ambiguous())}
    }
    fn tick_discovery(&mut self){
        if !self.discovery.as_ref().is_some_and(|h|h.handle.is_finished()){return;}
        let worker=self.discovery.take().unwrap();let id=worker.job;
        match worker.handle.join(){Ok(Ok(input))=>{if let Err(e)=self.finish_discovery(input){self.mark_failed(&id,e);}},Ok(Err(e))=>self.mark_failed(&id,e),Err(_)=>self.mark_failed(&id,ambiguous())}
    }
    fn tick_resume_hash(&mut self){
        if !self.resume_hash.as_ref().is_some_and(|h|h.handle.is_finished()){return;}
        let worker=self.resume_hash.take().unwrap();let id=worker.job;
        match worker.handle.join(){Ok(Ok(input))=>{if let Err(e)=self.finish_resume(input){self.mark_failed(&id,e);}},Ok(Err(e))=>self.mark_failed(&id,e),Err(_)=>self.mark_failed(&id,ambiguous())}
    }
    fn step(&mut self){
        self.tick_reconcile();
        let work={self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).queue.pop_front()};if let Some(work)=work{self.start_work(work);}
        if let Some(params)=self.deferred_prepare.take(){self.retry_prepare(params);}if let Some((params,generation))=self.deferred_review.take(){self.retry_review(params,generation);}
        let releases={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.preparations.iter().filter(|(_,v)|v.release&&!v.barrier&&!v.released).map(|(id,v)|(id.clone(),v.params["prepareKey"].as_str().unwrap_or("").to_owned())).collect::<Vec<_>>()};
        for (id,key) in releases{self.begin_release(&id,&key);}
        self.tick_fetch();self.tick_transfer();self.tick_hash();self.tick_discovery();self.tick_resume_hash();self.tick_preparation();self.tick_review();self.tick_orphan_frozen();self.tick_commit();self.tick_preparation_retirement();self.start_next_transfer();
        let (live,jobs)={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());(s.resolves.values().filter_map(|r|r.token.clone()).chain(s.starts.values().filter_map(|x|x.params["selectionToken"].as_str().map(str::to_owned))).collect::<BTreeSet<_>>(),s.jobs.iter().filter(|(_,j)|!job_store::is_tombstone(&j.body)).map(|(id,_)|id.clone()).collect::<BTreeSet<_>>())};
        self.authorizations.retain(|k,_|live.contains(k));self.job_authorizations.retain(|k,_|jobs.contains(k));self.authorization_prepares.retain(|k,_|jobs.contains(k));self.attempt_modes.retain(|k,_|jobs.contains(k));
    }
    fn retire(&mut self){
        if let Some(m)=self.metadata.as_mut(){m.begin_close();}
        loop {
            let preparations={let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());let frozen=s.frozen.keys().cloned().collect::<BTreeSet<_>>();s.preparations.iter_mut().filter(|(id,v)|!v.released&&!frozen.contains(*id)).map(|(id,v)|{v.release=true;v.review=JsonValue::Null;v.review_params=None;(id.clone(),v.params["prepareKey"].as_str().unwrap_or("").to_owned())}).collect::<Vec<_>>()};
            for (id,key) in preparations {self.begin_release(&id,&key);}
            let pending={let s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.starts.iter().filter(|(_,v)|v.admission).map(|(id,v)|(id.clone(),v.params.clone())).collect::<Vec<_>>()};
            let mut owned=self.job_authorizations.keys().cloned().chain(pending.iter().map(|(id,_)|id.clone())).collect::<BTreeSet<_>>();if let Some(a)=&self.active{owned.insert(a.job.clone());}if let Some(h)=&self.hashing{owned.insert(h.job.clone());}if let Some(h)=&self.discovery{owned.insert(h.job.clone());}if let Some(h)=&self.resume_hash{owned.insert(h.job.clone());}
            let barriers=if let Some(db)=self.store.as_mut(){match db.snapshots(){Ok(rows)=>{let mut all=true;for mut j in rows{
                if owned.contains(j.input["jobId"].as_str().unwrap_or(""))&&matches!(j.body["state"].as_str(),Some("resolving"|"transferring"|"discoveringFamily"|"sealing")){
                    let id=match JobId::parse(j.input["jobId"].as_str().unwrap_or("")){Ok(id)=>id,Err(_)=>{all=false;continue;}};j.body["state"]="paused".into();
                    match db.transition(&id,JobTransition::Body(j.body)){Ok(next)=>publish(&self.shared,next),Err(_)=>all=false}
                }}all},Err(_)=>false}}else{true};
            let mut admissions_done=true;
            if barriers {for (id,params) in pending{
                let observed=if let Some(db)=self.store.as_ref(){JobId::parse(&id).and_then(|job|db.get(&job))}else{self.validate_root().and_then(|_|if job_store::empty_namespace(&self.root)?{Ok(None)}else{Err(unknown())})};
                match observed{Ok(Some(j)) if same_stored_start(&params,&j)=>{let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.jobs.insert(id.clone(),j);s.starts.remove(&id);},Ok(None)=>{self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).starts.remove(&id);},_=>admissions_done=false}
            }}else{admissions_done=false;}
            if !barriers{self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).phase="waiting";}
            self.tick_transfer();self.tick_hash();self.tick_discovery();self.tick_resume_hash();self.tick_preparation();self.tick_review();self.tick_orphan_frozen();self.tick_commit();self.tick_preparation_retirement();self.tick_reconcile();let child_done=self.transfer_ticket.as_ref().is_none_or(|t|t.load(Ordering::Acquire));
            let metadata_done=if let Some(m)=self.metadata.as_mut(){m.is_quiescent()}else{self.metadata_ticket.as_ref().is_none_or(|t|t.load(Ordering::Acquire))};
            if barriers&&admissions_done&&self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).frozen.is_empty()&&self.commit_worker.is_none()&&self.active.is_none()&&self.hashing.is_none()&&self.discovery.is_none()&&self.resume_hash.is_none()&&self.preparation.is_none()&&self.preparation_guard.is_none()&&self.ready.is_empty()&&self.review_worker.is_none()&&self.review_guard.is_none()&&self.retiring_preparation.is_none()&&self.reconcile_slot.is_none()&&child_done&&metadata_done{break;}
            thread::sleep(Duration::from_millis(20));
        }
        self.metadata.take();self.store.take();self.authorizations.clear();self.job_authorizations.clear();self.authorization_prepares.clear();
        let mut s=self.shared.state.lock().unwrap_or_else(|p|p.into_inner());s.queue.clear();s.worker_exited=true;s.phase="closed";s.error=JsonValue::Null;
    }
}
fn run(root:PathBuf,session:SessionId,shared:Arc<Shared>){
    let mut worker=Worker{root,root_handle:None,root_identity:None,session,shared:shared.clone(),store:None,metadata:None,metadata_ticket:None,transfer_ticket:None,authorizations:BTreeMap::new(),job_authorizations:BTreeMap::new(),authorization_prepares:BTreeMap::new(),attempt_modes:BTreeMap::new(),fetch:None,active:None,hashing:None,discovery:None,resume_hash:None,reconcile_slot:None,preparation:None,preparation_guard:None,ready:BTreeMap::new(),review_worker:None,review_guard:None,retiring_preparation:None,plans:BTreeMap::new(),deferred_prepare:None,deferred_review:None,commit_worker:None,pending_skips:BTreeMap::new()};
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
        while !shared.close.load(Ordering::Acquire){
            let initialize={let _s=shared.state.lock().unwrap_or_else(|p|p.into_inner());!shared.startup_done.load(Ordering::Acquire)&&!shared.startup_blocked.load(Ordering::Acquire)};
            if initialize {if let Err(e)=worker.initialize(){let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="failed";s.error=failure(&e);}let _s=shared.state.lock().unwrap_or_else(|p|p.into_inner());shared.startup_done.store(true,Ordering::Release);shared.startup_pending.store(false,Ordering::Release);}
            worker.step();let s=shared.state.lock().unwrap_or_else(|p|p.into_inner());let _=shared.wake.wait_timeout(s,Duration::from_millis(20));
        }
    }));
    if result.is_err(){let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());s.phase="waiting";s.error=JsonValue::Null;}
    shared.close.store(true,Ordering::Release);worker.retire();
}

/// Cleanup requires the shared preparation slot and the matching store lease.
pub(crate) struct RemoteCleanupPermit<'a> { slot:&'a PreparationSlotLease, lease:&'a RemoteStoreLease }
impl RemoteCleanupPermit<'_> {
    fn validate(&self)->Result<(),NativeError>{self.lease.validate()?;self.slot.validate()?;if self.slot.root()!=self.lease.root()||self.slot.identity()!=self.lease.identity(){return Err(ambiguous());}Ok(())}
}
fn cleanup_intent(c:&JsonValue,kind:&str,device:&str,inode:&str)->JsonValue{object!{kind:kind,device:c[device].clone(),inode:c[inode].clone()}}
fn cleanup_write(db:&mut RemoteJobStore,shared:&Shared,id:&JobId,j:JobSnapshot)->Result<(),NativeError>{let next=db.transition(id,JobTransition::Body(j.body))?;publish(shared,next);Ok(())}
/// One bounded cursor step. All namespace evidence is admitted before each
/// intent and unlink; missing objects advance only behind their exact intent.
fn cleanup_step(db:&mut RemoteJobStore,shared:&Shared,id:&JobId,permit:&RemoteCleanupPermit<'_>)->Result<bool,NativeError>{
    permit.validate()?;
    verify_claim_set(db)?;let lease=db.lease();let mut j=db.get(id)?.ok_or_else(ambiguous)?;
    if !matches!(j.body["state"].as_str(),Some("cancelRequested"|"cancelled"|"abandoned"|"installed"|"failed")){return Err(ambiguous());}
    if !j.body["commit"].is_null(){let input=RemoteCommitInput::parse(j.body["commit"].clone()).map_err(|_|unknown())?;let history=Store::open_history(lease.root()).map_err(|_|unknown())?;
        let outcome=history.exact_any_remote_outcome(&input).map_err(|_|unknown())?;
        let agrees=match outcome{ExactRemoteOutcome::Succeeded(_)=>j.body["state"]=="installed",ExactRemoteOutcome::Failed(_)=>matches!(j.body["state"].as_str(),Some("failed"|"abandoned")),ExactRemoteOutcome::Absent=>false};
        if !agrees||history.journal(input.operation_id()).map_err(|_|unknown())?.is_some(){return Err(unknown());}
        lease.validate()?;
    }else if j.body["state"]=="failed"{return Err(ambiguous());}
    let Some(i)=j.body["claims"].members().position(|c|c["state"]!="cleaned") else{
        if j.body["cleanupPending"]==true||j.body["state"]=="cancelRequested"{j.body["cleanupPending"]=false.into();if j.body["state"]=="cancelRequested"{j.body["state"]="cancelled".into();j.body["prepare"]=JsonValue::Null;}cleanup_write(db,shared,id,j)?;}return Ok(true);
    };
    if j.body["claims"][i]["state"]!="cleaning"{j.body["claims"][i]["state"]="cleaning".into();j.body["cleanupPending"]=true.into();cleanup_write(db,shared,id,j)?;return Ok(false);}
    let c=j.body["claims"][i].clone();let name=claim_name(c["claimId"].as_str().ok_or_else(ambiguous)?);let cursor=c["cleanup"]["cursor"].as_str().and_then(|s|s.parse::<u64>().ok()).ok_or_else(ambiguous)?;
    let claim_path=lease.bulk_path().join(&name);
    let dir=optional_at(lease.directory(),&name,0x10000)?;
    if dir.is_none(){
        if !c["dirDevice"].is_null()&&!missing_intent(&c,"directory"){return Err(ambiguous());}
        lease.directory().sync_all().map_err(|_|ambiguous())?;j.body["claims"][i]["state"]="cleaned".into();j.body["claims"][i]["cleanup"]=object!{cursor:"3",intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;return Ok(false);
    }
    let dir=dir.unwrap();let dm=dir.metadata().map_err(|_|ambiguous())?;if !private_dir(&dm)||!expected_pair(&c,"dirDevice","dirInode",&lease,&claim_path,&dir){return Err(ambiguous());}
    if cursor<=1&&c["leaseDevice"].is_null(){
        if !c["fileDevice"].is_null(){return Err(ambiguous());}
        j.body["claims"][i]["cleanup"]=object!{cursor:"2",intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;return Ok(false);
    }
    if cursor<=1 {
        let exclusive=optional_at(&dir,"lease",2)?;
        if exclusive.is_none(){
            if cursor!=1||!missing_intent(&c,"lease"){return Err(ambiguous());}
            dir.sync_all().map_err(|_|ambiguous())?;j.body["claims"][i]["cleanup"]=object!{cursor:"2",intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;return Ok(false);
        }
        let exclusive=exclusive.unwrap();let lm=exclusive.metadata().map_err(|_|ambiguous())?;
        if !private_file(&lm)||!expected_pair(&c,"leaseDevice","leaseInode",&lease,&claim_path.join("lease"),&exclusive)||unsafe{flock(exclusive.as_raw_fd(),2|4)}!=0{return Err(ambiguous());}
        let (kind,target,device,inode,next)=if cursor==0{("data","data","fileDevice","fileInode","1")}else{("lease","lease","leaseDevice","leaseInode","2")};
        if c[device].is_null(){j.body["claims"][i]["cleanup"]=object!{cursor:next,intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;return Ok(false);}
        if c["cleanup"]["intent"].is_null(){j.body["claims"][i]["cleanup"]["intent"]=cleanup_intent(&c,kind,device,inode);cleanup_write(db,shared,id,j)?;return Ok(false);}
        if !missing_intent(&c,kind){return Err(ambiguous());}verify_claim_set(db)?;
        if let Some(target_file)=optional_at(&dir,target,0)?{let m=target_file.metadata().map_err(|_|ambiguous())?;if !private_file(&m)||!expected_pair(&c,device,inode,&lease,&claim_path.join(target),&target_file){return Err(ambiguous());}
            if kind=="lease"&&identity(&m)!=identity(&lm){return Err(ambiguous());}unlink_at(&dir,target,false)?;}
        dir.sync_all().map_err(|_|ambiguous())?;j.body["claims"][i]["cleanup"]=object!{cursor:next,intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;drop(exclusive);return Ok(false);
    }
    if cursor!=2{return Err(ambiguous());}
    if c["cleanup"]["intent"].is_null(){j.body["claims"][i]["cleanup"]["intent"]=cleanup_intent(&c,"directory","dirDevice","dirInode");cleanup_write(db,shared,id,j)?;return Ok(false);}
    if !missing_intent(&c,"directory"){return Err(ambiguous());}verify_claim_set(db)?;let mut empty=true;entries(&dir,|_|{empty=false;Ok(())})?;if !empty{return Err(ambiguous());}
    lease.validate()?;unlink_at(lease.directory(),&name,true)?;lease.directory().sync_all().map_err(|_|ambiguous())?;j.body["claims"][i]["state"]="cleaned".into();j.body["claims"][i]["cleanup"]=object!{cursor:"3",intent:JsonValue::Null};cleanup_write(db,shared,id,j)?;Ok(false)
}

#[cfg(test)]
#[test]
fn item93_legacy_remote_claims_survive_device_change() {
    use std::os::unix::fs::PermissionsExt;
    let fixture=crate::test_support::Fixture::new();
    std::fs::create_dir(&fixture.data).unwrap();
    if !paths::test_volatile_filesystem(&File::open(&fixture.data).unwrap()) { return; }
    let submission=SubmissionKeyV1{submission_type:"mod".into(),submission_id:"42".into()};
    let file=ConsentFile{file_id:"1".into(),original_name:"sample.vpk".into(),declared_size:Some("52".into()),created_at:None,md5:None,list:"current".into(),archive_flag:None};
    let input=RemoteJobInputV1{job_id:JobId::parse("11111111111111111111111111111111").unwrap(),consent_fingerprint:contract::candidate_fingerprint(&submission,"Sample",&file),submission,root_file:file,profile_id:"default".into(),initial_revision:"1".into(),accept_archived:false,submission_name:"Sample".into(),profile_name_at_start:"Default".into()};
    let mut db=RemoteJobStore::open(&fixture.data).unwrap();
    let mut snapshot=db.reserve_start(&input).unwrap().snapshot;
    let name=claim_name("22222222222222222222222222222222");let dir=fixture.data.join(&name);
    std::fs::create_dir(&dir).unwrap();std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700)).unwrap();
    for n in ["data","lease"] {let path=dir.join(n);std::fs::write(&path,b"").unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o600)).unwrap();}
    let dm=std::fs::metadata(&dir).unwrap();let fm=std::fs::metadata(dir.join("data")).unwrap();let lm=std::fs::metadata(dir.join("lease")).unwrap();
    snapshot.body["claims"].push(object!{fileId:"1",claimId:"22222222222222222222222222222222",state:"owned",dirDevice:(dm.dev()+1).to_string(),dirInode:dm.ino().to_string(),fileDevice:(fm.dev()+1).to_string(),fileInode:fm.ino().to_string(),leaseDevice:(lm.dev()+1).to_string(),leaseInode:lm.ino().to_string(),uid:fm.uid().to_string(),linkCount:"1",durableOffset:"0",prefixSha256:JsonValue::Null,strongEtag:JsonValue::Null,effectiveUriSha256:JsonValue::Null,total:JsonValue::Null,epoch:JsonValue::Null,receipt:JsonValue::Null,cleanup:object!{cursor:"0",intent:JsonValue::Null}}).unwrap();
    db.transition(&input.job_id,JobTransition::Body(snapshot.body)).unwrap();
    assert!(verify_claim_set(&db).is_ok(),"intact owned claims in the locked btrfs root survive a changed device");
    let lease=db.lease();let mut claim=db.get(&input.job_id).unwrap().unwrap().body["claims"][0].clone();
    let file=File::open(dir.join("data")).unwrap();
    claim["fileInode"]=(fm.ino()+1).to_string().into();
    assert!(!expected_pair(&claim,"fileDevice","fileInode",&lease,&dir.join("data"),&file));
    claim["fileInode"]=fm.ino().to_string().into();claim["claimFsid"]="0000000000000001".into();
    assert!(!expected_pair(&claim,"fileDevice","fileInode",&lease,&dir.join("data"),&file));
}

#[cfg(test)]
mod transfer_regression_tests {
    use super::*;
    use json::object;
    use std::io::Write;

    unsafe extern "C" {
        fn pipe2(fds: *mut i32, flags: i32) -> i32;
        fn fork() -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
        fn close_range(first: u32, last: u32, flags: i32) -> i32;
        fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
        fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
        fn close(fd: i32) -> i32;
        fn _exit(code: i32) -> !;
    }
    fn nb_pipe() -> (File, File) {
        let mut fds = [-1i32; 2];
        assert_eq!(unsafe { pipe2(fds.as_mut_ptr(), 0x800 | 0x80000) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }
    fn sync_pipe() -> (File, File) {
        let mut fds = [-1i32; 2];
        assert_eq!(unsafe { pipe2(fds.as_mut_ptr(), 0x80000) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    struct OwnerHarness {
        _fixture: crate::test_support::Fixture,
        worker: Worker,
        job_id: JobId,
        epoch: contract::TransferEpoch,
        stdin_r: Option<File>,
        barrier_w: File,
        stdout_w: Option<File>,
    }

    fn setup_owner_with_child(initial_frames: Option<&[u8]>, close_stdin_early: bool, close_stdout_early: bool) -> OwnerHarness {
        let fixture = crate::test_support::Fixture::new();
        std::fs::create_dir(&fixture.data).unwrap();
        let session = SessionId::parse(&"a".repeat(32)).unwrap();
        let job_id = JobId::parse(&"1".repeat(32)).unwrap();
        let epoch = contract::TransferEpoch::parse(&"2".repeat(32)).unwrap();
        let submission = SubmissionKeyV1 { submission_type: "mod".into(), submission_id: "663317".into() };
        let file = ConsentFile {
            file_id: "1886611".into(),
            original_name: "girlfailure_silver_v2.rar".into(),
            declared_size: Some("57893932".into()),
            created_at: None,
            md5: Some("391274a4784a63c8cf6bb500da8cd2d6".into()),
            list: "current".into(),
            archive_flag: Some(false),
        };
        let input = RemoteJobInputV1 {
            job_id: job_id.clone(),
            consent_fingerprint: contract::candidate_fingerprint(&submission, "Girlfailure", &file),
            submission,
            root_file: file.clone(),
            profile_id: "default".into(),
            initial_revision: "1".into(),
            accept_archived: false,
            submission_name: "Girlfailure".into(),
            profile_name_at_start: "Default".into(),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                phase: "ready",
                error: JsonValue::Null,
                worker_exited: false,
                reconciled: true,
                claims_admitted: true,
                jobs: BTreeMap::new(),
                progress: BTreeMap::new(),
                resolves: BTreeMap::new(),
                starts: BTreeMap::new(),
                controls: BTreeMap::new(),
                commits: BTreeMap::new(),
                frozen: BTreeMap::new(),
                recovery_custody: None,
                preparations: BTreeMap::new(),
                used_prepare_keys: BTreeSet::new(),
                replacement_facts: BTreeMap::new(),
                queue: VecDeque::new(),
            }),
            wake: Condvar::new(),
            close: AtomicBool::new(false),
            launch_blocked: AtomicBool::new(false),
            startup_blocked: AtomicBool::new(false),
            startup_pending: AtomicBool::new(false),
            startup_done: AtomicBool::new(true),
            recovery_borrowed: Arc::new(AtomicBool::new(false)),
        });
        let mut db = RemoteJobStore::open(&fixture.data).unwrap();
        let snapshot = db.reserve_start(&input).unwrap().snapshot;
        publish(&shared, snapshot);
        let claim = create_claim(&mut db, &shared, &job_id, &file).unwrap();
        let start_frame = TransferStartV1::parse(object! {
            version: 1,
            kind: "start",
            epoch: epoch.as_str(),
            requestUrl: "https://gamebanana.com/dl/1886611",
            allowedHosts: json::array!["gamebanana.com", "files.gamebanana.com"],
            deniedHosts: json::array!["filecache25.gamebanana.com"],
            offset: "0",
            ifRange: JsonValue::Null,
            uriSha256: JsonValue::Null,
            byteCap: "1073741824",
            claimIdentity: object! {
                device: claim.identity.data_device.to_string(),
                inode: claim.identity.data_inode.to_string(),
                uid: claim.identity.uid.to_string(),
            }
        }).unwrap();

        let (stdin_r, stdin_w) = nb_pipe();
        let (stdout_r, stdout_w) = nb_pipe();
        let (barrier_r, barrier_w) = sync_pipe();
        let bytes_owned = initial_frames.map(|b| b.to_vec());

        let pid = unsafe { fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                let sr = fcntl(stdin_r.as_raw_fd(), 1030, 200);
                let sw = fcntl(stdout_w.as_raw_fd(), 1030, 200);
                let br = fcntl(barrier_r.as_raw_fd(), 1030, 200);
                if sr < 0 || sw < 0 || br < 0 { _exit(127); }
                if dup2(sr, 3) < 0 || dup2(sw, 4) < 0 || dup2(br, 5) < 0 { _exit(127); }
                let _ = close_range(6, u32::MAX, 0);
                if close_stdin_early {
                    close(3);
                }
                if let Some(ref bytes) = bytes_owned {
                    let _ = write(4, bytes.as_ptr().cast(), bytes.len());
                }
                if close_stdout_early {
                    close(4);
                }
                let mut b = [0u8; 1];
                let _ = read(5, b.as_mut_ptr().cast(), 1);
                if !close_stdin_early {
                    close(3);
                }
                if !close_stdout_early {
                    close(4);
                }
                close(5);
                _exit(1);
            }
        }
        let parent_stdout_w = if initial_frames.is_some() {
            drop(stdout_w);
            None
        } else {
            Some(stdout_w)
        };
        drop(barrier_r);

        if initial_frames.is_some() {
            let mut pfd = [libc::pollfd { fd: stdout_r.as_raw_fd(), events: libc::POLLIN | libc::POLLHUP, revents: 0 }];
            let _ = unsafe { libc::poll(pfd.as_mut_ptr(), 1, 2000) };
        }

        let child = TransferChild::test_pipes_with_pid(start_frame, pid, stdin_w, stdout_r).unwrap();
        let root_handle = File::open(&fixture.data).unwrap();
        let root_meta = root_handle.metadata().unwrap();
        let worker = Worker {
            root: fixture.data.clone(),
            root_handle: Some(root_handle),
            root_identity: Some(identity(&root_meta)),
            session,
            shared,
            store: Some(db),
            metadata: None,
            metadata_ticket: None,
            transfer_ticket: None,
            authorizations: BTreeMap::new(),
            job_authorizations: BTreeMap::new(),
            authorization_prepares: BTreeMap::new(),
            attempt_modes: BTreeMap::new(),
            fetch: None,
            active: Some(ActiveTransfer {
                job: job_id.as_str().into(),
                file,
                claim,
                child,
                epoch: epoch.as_str().into(),
                offset: 0,
                if_range: None,
                uri_digest: None,
                total: None,
                http_length: None,
                finished: None,
                failed: None,
                abort_sent: false,
                volatile: 0,
                canonical_target: None,
            }),
            hashing: None,
            discovery: None,
            resume_hash: None,
            reconcile_slot: None,
            preparation: None,
            preparation_guard: None,
            ready: BTreeMap::new(),
            review_worker: None,
            review_guard: None,
            retiring_preparation: None,
            plans: BTreeMap::new(),
            deferred_prepare: None,
            deferred_review: None,
            commit_worker: None,
            pending_skips: BTreeMap::new(),
        };
        OwnerHarness { _fixture: fixture, worker, job_id, epoch, stdin_r: Some(stdin_r), barrier_w, stdout_w: parent_stdout_w }
    }

    fn reap_and_read_stdin(mut h: OwnerHarness) -> (JobSnapshot, Vec<u8>) {
        drop(h.stdout_w.take());
        drop(h.barrier_w);
        for _ in 0..400 {
            if h.worker.active.is_none() { break; }
            h.worker.tick_transfer();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(h.worker.active.is_none(), "child should be reaped");
        let mut stdin_bytes = Vec::new();
        if let Some(mut stdin_r) = h.stdin_r.take() {
            let mut buf = [0u8; 4096];
            loop {
                match stdin_r.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => stdin_bytes.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        let stored = h.worker.store.as_ref().unwrap().get(&h.job_id).unwrap().unwrap();
        (stored, stdin_bytes)
    }

    #[test]
    fn terminal_transfer_error_survives_delayed_child_exit_without_invalid_abort() {
        let error_frame = object! {
            version: 1,
            kind: "error",
            epoch: "2".repeat(32),
            data: object! { kind: "hostNotAllowed" }
        }.dump() + "\n";
        let mut h = setup_owner_with_child(Some(error_frame.as_bytes()), false, true);

        // Multiple ticks while the child process has not yet exited.
        h.worker.tick_transfer();
        assert!(h.worker.active.is_some(), "child has not exited yet");
        h.worker.tick_transfer();
        assert!(h.worker.active.is_some(), "child still blocked on barrier");
        h.worker.tick_transfer();

        let (stored, stdin_bytes) = reap_and_read_stdin(h);
        assert!(stdin_bytes.is_empty(), "must not send invalid abort frame after terminal error: {:?}", String::from_utf8_lossy(&stdin_bytes));
        assert_eq!(stored.body["state"], "failed");
        assert_eq!(stored.body["error"]["kind"], "hostNotAllowed");
    }

    #[test]
    fn genuine_framing_error_with_delayed_child_exit_reports_transport_framing() {
        let mut h = setup_owner_with_child(Some(b"{not-valid-json\n"), false, true);

        h.worker.tick_transfer();
        assert!(h.worker.active.is_some(), "child still blocked on barrier");
        h.worker.tick_transfer();

        let (stored, stdin_bytes) = reap_and_read_stdin(h);
        assert!(stdin_bytes.is_empty(), "failed pipe child must not receive abort");
        assert_eq!(stored.body["state"], "failed");
        assert_eq!(stored.body["error"]["kind"], "transportFraming");
    }

    #[test]
    fn decision_phase_error_sends_abort_when_open_and_preserves_first_error_on_broken_pipe() {
        let bad_size_headers = object! {
            version: 1,
            kind: "headers",
            epoch: "2".repeat(32),
            data: object! {
                url: "https://gamebanana.com/dl/1886611",
                status: "200",
                identityEstablished: true,
                headers: json::array![object! { name: "Content-Length", value: "999" }],
            }
        }.dump() + "\n";

        let eval_auth = |session: &SessionId, servers: JsonValue| {
            let key = SubmissionKeyV1 { submission_type: "mod".into(), submission_id: "663317".into() };
            let now = Instant::now();
            let profile = object! { _idRow: 663317, _sProfileUrl: key.profile_url(), _sName: "Girlfailure", _bIsPrivate: false, _bIsTrashed: false, _bIsWithheld: false, _aGame: object! { _idRow: 20948 } };
            let downloads = object! { _idRow: 663317, _bIsTrashed: false, _bIsWithheld: false, _aFiles: json::array![object! { _idRow: 1886611, _sFile: "girlfailure_silver_v2.rar", _nFilesize: 57893932, _sMd5Checksum: "391274a4784a63c8cf6bb500da8cd2d6", _sDownloadUrl: "https://gamebanana.com/dl/1886611", _bIsArchived: false }] };
            let responses = [profile, downloads, servers].into_iter().zip([MetadataRoute::Profile, MetadataRoute::Files, MetadataRoute::Servers]).map(|(body, route)| {
                let url = match route {
                    MetadataRoute::Profile => format!("https://gamebanana.com/apiv11/{}/{}/ProfilePage", key.model(), key.submission_id),
                    MetadataRoute::Files => format!("https://gamebanana.com/apiv11/{}/{}/DownloadPage", key.model(), key.submission_id),
                    MetadataRoute::Servers => "https://gamebanana.com/apiv11/Util/Fileservers?_nPage=1".into(),
                };
                RawResponse::from_completion(route, super::super::metadata::MetadataCompletion {
                    request_id: "req".into(),
                    request_url: url.clone(),
                    http_status: Some(200),
                    final_url: Some(url),
                    retry_after_seconds: None,
                    body: Some(body.dump().into_bytes()),
                    error: None,
                    requested_at: now,
                    session: session.clone(),
                }).unwrap()
            }).collect::<Vec<_>>().try_into().ok().unwrap();
            FreshFileAuthorityV1::evaluate(&AuthorityRequest { session: session.clone(), submission: key, selected: None }, &responses, now).unwrap()
        };

        // Case A: pipe open -> child receives abort { reason: "failure" } and first error stays transportChecksum.
        let mut h_open = setup_owner_with_child(Some(bad_size_headers.as_bytes()), false, false);
        let auth = eval_auth(&h_open.worker.session, object! { _aRecords: json::array![] });
        h_open.worker.job_authorizations.insert(h_open.job_id.as_str().into(), auth);
        h_open.worker.tick_transfer(); // poll -> Decision error transportChecksum
        h_open.worker.tick_transfer(); // sends abort { reason: "failure" }
        let (stored_open, stdin_open) = reap_and_read_stdin(h_open);
        assert!(String::from_utf8_lossy(&stdin_open).contains("\"kind\":\"abort\""), "Decision-phase failure should abort open child");
        assert_eq!(stored_open.body["error"]["kind"], "transportChecksum");

        // Case B: pipe broken before tick 2 -> send(abort) returns EPIPE (transportFraming), first error MUST stay transportChecksum.
        let mut h_epipe = setup_owner_with_child(Some(bad_size_headers.as_bytes()), true, false);
        drop(h_epipe.stdin_r.take());
        let auth2 = eval_auth(&h_epipe.worker.session, object! { _aRecords: json::array![] });
        h_epipe.worker.job_authorizations.insert(h_epipe.job_id.as_str().into(), auth2);
        h_epipe.worker.tick_transfer(); // poll -> Decision error transportChecksum
        h_epipe.worker.tick_transfer(); // send(abort) fails EPIPE -> must NOT overwrite transportChecksum
        let (stored_epipe, _) = reap_and_read_stdin(h_epipe);
        assert_eq!(stored_epipe.body["error"]["kind"], "transportChecksum");
    }

    #[test]
    fn owner_captured_fileservers_canonical_redirect_and_adversarial_rejection() {
        let mut h = setup_owner_with_child(None, false, false);
        let key = SubmissionKeyV1 { submission_type: "mod".into(), submission_id: "663317".into() };
        let now = Instant::now();
        let profile = object! { _idRow: 663317, _sProfileUrl: key.profile_url(), _sName: "Girlfailure", _bIsPrivate: false, _bIsTrashed: false, _bIsWithheld: false, _aGame: object! { _idRow: 20948 } };
        let downloads = object! { _idRow: 663317, _bIsTrashed: false, _bIsWithheld: false, _aFiles: json::array![object! { _idRow: 1886611, _sFile: "girlfailure_silver_v2.rar", _nFilesize: 57893932, _sMd5Checksum: "391274a4784a63c8cf6bb500da8cd2d6", _sDownloadUrl: "https://gamebanana.com/dl/1886611", _bIsArchived: false }] };
        let servers = super::super::authority::regression_tests::captured_fileservers_json();
        let responses = [profile, downloads, servers].into_iter().zip([MetadataRoute::Profile, MetadataRoute::Files, MetadataRoute::Servers]).map(|(body, route)| {
            let url = match route {
                MetadataRoute::Profile => format!("https://gamebanana.com/apiv11/{}/{}/ProfilePage", key.model(), key.submission_id),
                MetadataRoute::Files => format!("https://gamebanana.com/apiv11/{}/{}/DownloadPage", key.model(), key.submission_id),
                MetadataRoute::Servers => "https://gamebanana.com/apiv11/Util/Fileservers?_nPage=1".into(),
            };
            RawResponse::from_completion(route, super::super::metadata::MetadataCompletion {
                request_id: "req".into(),
                request_url: url.clone(),
                http_status: Some(200),
                final_url: Some(url),
                retry_after_seconds: None,
                body: Some(body.dump().into_bytes()),
                error: None,
                requested_at: now,
                session: h.worker.session.clone(),
            }).unwrap()
        }).collect::<Vec<_>>().try_into().ok().unwrap();
        let auth = FreshFileAuthorityV1::evaluate(&AuthorityRequest { session: h.worker.session.clone(), submission: key, selected: None }, &responses, now).unwrap();
        h.worker.job_authorizations.insert(h.job_id.as_str().into(), auth);

        let mut active = h.worker.active.take().unwrap();
        // Hop 1: gamebanana.com -> files.gamebanana.com
        {
            let w = h.stdout_w.as_mut().unwrap();
            let frame1 = object! {
                version: 1, kind: "headers", epoch: h.epoch.as_str(),
                data: object! { url: "https://gamebanana.com/dl/1886611", status: "302", identityEstablished: true, headers: json::array![object! { name: "Location", value: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar" }] }
            }.dump() + "\n";
            w.write_all(frame1.as_bytes()).unwrap();
        }
        let events1 = active.child.poll().unwrap();
        assert_eq!(events1.len(), 1);
        h.worker.transfer_event(&mut active, events1[0].json()).unwrap();
        assert!(active.canonical_target.is_none());

        // Hop 2: files.gamebanana.com -> unlisted filecache28.gamebanana.com
        {
            let w = h.stdout_w.as_mut().unwrap();
            let frame2 = object! {
                version: 1, kind: "headers", epoch: h.epoch.as_str(),
                data: object! { url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar", status: "302", identityEstablished: true, headers: json::array![object! { name: "Location", value: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar" }] }
            }.dump() + "\n";
            w.write_all(frame2.as_bytes()).unwrap();
        }
        let events2 = active.child.poll().unwrap();
        assert_eq!(events2.len(), 1);
        h.worker.transfer_event(&mut active, events2[0].json()).unwrap();
        assert_eq!(active.canonical_target.as_deref(), Some("https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar"));

        // Hop 3: 200 OK on exact canonical target -> allow(body) succeeds.
        {
            let w = h.stdout_w.as_mut().unwrap();
            let frame3 = object! {
                version: 1, kind: "headers", epoch: h.epoch.as_str(),
                data: object! { url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", status: "200", identityEstablished: true, headers: json::array![object! { name: "Content-Length", value: "57893932" }] }
            }.dump() + "\n";
            w.write_all(frame3.as_bytes()).unwrap();
        }
        let events3 = active.child.poll().unwrap();
        assert_eq!(events3.len(), 1);
        h.worker.transfer_event(&mut active, events3[0].json()).unwrap();
        h.worker.active = Some(active);

        // Adversarial check via transfer_event directly: chained 302 from filecache28 is rejected with hostNotAllowed.
        let mut active_chain = h.worker.active.take().unwrap();
        let bad_chain = object! {
            version: 1, kind: "headers", epoch: h.epoch.as_str(),
            data: object! { url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", status: "302", identityEstablished: true, headers: json::array![object! { name: "Location", value: "https://filecache29.gamebanana.com/mods/girlfailure_silver_v2.rar" }] }
        };
        let err = h.worker.transfer_event(&mut active_chain, &bad_chain).unwrap_err();
        assert_eq!(err.kind, "hostNotAllowed");
        assert!(err.message.contains("redirect host is not authorized"));

        // Adversarial check: path substitution on filecache28 fails closed with hostNotAllowed.
        let bad_path = object! {
            version: 1, kind: "headers", epoch: h.epoch.as_str(),
            data: object! { url: "https://filecache28.gamebanana.com/mods/other.rar", status: "200", identityEstablished: true, headers: json::array![] }
        };
        assert_eq!(h.worker.transfer_event(&mut active_chain, &bad_path).unwrap_err().kind, "hostNotAllowed");

        active_chain.failed = Some(err);
        h.worker.active = Some(active_chain);
        let (stored, stdin_bytes) = reap_and_read_stdin(h);
        let stdin_text = String::from_utf8_lossy(&stdin_bytes);
        assert!(stdin_text.contains("\"mode\":\"redirect\"") && stdin_text.contains("filecache28.gamebanana.com") && stdin_text.contains("\"mode\":\"body\""));
        assert_eq!(stored.body["error"]["kind"], "hostNotAllowed");
    }
}


