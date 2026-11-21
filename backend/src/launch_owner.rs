//! Main NativeService's single asynchronous launch owner. SQLite never crosses a
//! thread boundary. Controls return cached admission, never filesystem success.
use crate::{game_access::GameLock,launch_preparation::{PreparationPlan,PreparationProgress},launch_process,launch_record as r,launch_routes,launch_store::{RootOwnerLease,LiveReview,DispatchPermit,CommitAdmission,ObservedGameCycle,TerminalBeforeHandoff},native::{NativeError,operation_input},paths,prepare_control::PrepareControl,store::{Store,RootLock}};
use json::{object,JsonValue};
use std::{collections::BTreeMap,path::{Path,PathBuf},process::Child,sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering},mpsc::{self,SyncSender,Receiver,TrySendError}},thread::{self,JoinHandle},time::{Duration,Instant}};

enum ChildPoolSlot { Free, Reserved{id:u64,root:PathBuf}, Running{id:u64,root:PathBuf,child:Child} }
struct ChildPoolInner {slots:Mutex<Vec<ChildPoolSlot>>,next:AtomicU64,stop:AtomicBool}
/// Service-owned actual child handles, including in-flight spawn reservations.
pub(crate) struct ChildPool {inner:Arc<ChildPoolInner>}
impl ChildPool {
    pub(crate) fn new()->Result<Arc<Self>,NativeError>{
        let inner=Arc::new(ChildPoolInner{slots:Mutex::new((0..8).map(|_|ChildPoolSlot::Free).collect()),next:AtomicU64::new(1),stop:AtomicBool::new(false)});
        let sweep=inner.clone();thread::Builder::new().name("native-launch-reaper".into()).spawn(move||{
            while !sweep.stop.load(Ordering::Acquire){
                {let mut slots=sweep.slots.lock().unwrap_or_else(|p|p.into_inner());for slot in slots.iter_mut(){if let ChildPoolSlot::Running{child,..}=slot{if matches!(child.try_wait(),Ok(Some(_))){*slot=ChildPoolSlot::Free;}}}}
                thread::sleep(Duration::from_millis(50));
            }
        }).map_err(|_|NativeError::new("launchBusy","The launcher ownership pool could not start."))?;
        Ok(Arc::new(Self{inner}))
    }
    fn reserve(self:&Arc<Self>,root:&Path)->Result<Reservation,NativeError>{
        let id=self.inner.next.fetch_update(Ordering::AcqRel,Ordering::Acquire,|n|n.checked_add(1)).map_err(|_|busy())?;
        let mut slots=self.inner.slots.lock().unwrap_or_else(|p|p.into_inner());let index=slots.iter().position(|s|matches!(s,ChildPoolSlot::Free)).ok_or_else(busy)?;
        slots[index]=ChildPoolSlot::Reserved{id,root:root.to_owned()};Ok(Reservation{inner:Arc::new(ReservationInner{pool:self.inner.clone(),index,id})})
    }
    pub(crate) fn retained_for(&self,root:&Path)->bool{self.inner.slots.lock().unwrap_or_else(|p|p.into_inner()).iter().any(|s|matches!(s,ChildPoolSlot::Running{root:r,..} if r==root))}
    fn retained_generation(&self,root:&Path,generation:u64)->bool{self.inner.slots.lock().unwrap_or_else(|p|p.into_inner()).iter().any(|s|matches!(s,ChildPoolSlot::Running{id,root:r,..} if *id==generation&&r==root))}
    fn has_capacity(&self)->bool{self.inner.slots.lock().unwrap_or_else(|p|p.into_inner()).iter().any(|s|matches!(s,ChildPoolSlot::Free))}
    pub(crate) fn shutdown(&self){self.inner.stop.store(true,Ordering::Release);}
}
#[derive(Clone)] struct Reservation {inner:Arc<ReservationInner>}
struct ReservationInner {pool:Arc<ChildPoolInner>,index:usize,id:u64}
impl Reservation {
    fn adopt(&self,child:Child){
        let mut child=Some(child);
        loop {
            {let mut slots=self.inner.pool.slots.lock().unwrap_or_else(|p|p.into_inner());let slot=&mut slots[self.inner.index];
                if matches!(slot,ChildPoolSlot::Reserved{id,..} if *id==self.inner.id){
                    if let ChildPoolSlot::Reserved{id,root}=std::mem::replace(slot,ChildPoolSlot::Free){*slot=ChildPoolSlot::Running{id,root,child:child.take().unwrap()};return;}
                }}
            // An unexpected reservation conflict cannot discard the sole handle.
            thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for ReservationInner {fn drop(&mut self){let mut slots=self.pool.slots.lock().unwrap_or_else(|p|p.into_inner());if matches!(&slots[self.index],ChildPoolSlot::Reserved{id,..} if *id==self.id){slots[self.index]=ChildPoolSlot::Free;}}}
struct LaunchChild {child:Option<Child>,reservation:Reservation}
impl LaunchChild {fn empty(reservation:Reservation)->Self{Self{child:None,reservation}}fn new(child:Child,reservation:Reservation)->Self{Self{child:Some(child),reservation}}}
impl Drop for LaunchChild {fn drop(&mut self){if let Some(child)=self.child.take(){self.reservation.adopt(child);}}}
const LIMIT:usize=64*1024-256;
fn busy()->NativeError{NativeError::new("launchBusy","The launch owner is busy; poll its existing control or attempt.")}
fn exact(p:&JsonValue,keys:&[&str])->Result<(),NativeError>{if !p.is_object() || p.len()!=keys.len() || keys.iter().any(|k|!p.has_key(k)){Err(NativeError::params())}else{Ok(())}}
fn s<'a>(p:&'a JsonValue,k:&str)->Result<&'a str,NativeError>{p[k].as_str().ok_or_else(NativeError::params)}
fn u(p:&JsonValue,k:&str)->Result<u64,NativeError>{crate::store::revision(s(p,k)?)}
fn id(p:&JsonValue,k:&str)->Result<(),NativeError>{if paths::safe_id(s(p,k)?){Ok(())}else{Err(NativeError::params())}}
fn error_value(e:&NativeError)->JsonValue{
    let kind=if e.kind.len()<=64 && paths::safe_id(&e.kind){e.kind.as_str()}else{"launchFailed"};
    let mut message:String=e.message.chars().map(|c|if c.is_control(){' '}else{c}).collect();
    if message.len()>256 {
        let start=message.rfind("; refusal check failed: ").into_iter().chain(message.rfind("Cause during ")).max().unwrap_or(0);
        let detail=&message[start..];let mut end=detail.len().min(253);
        while !detail.is_char_boundary(end){end-=1;}
        message=format!("{}…",&detail[..end]);
    }
    object!{kind:kind,message:message}
}
struct Receipt {digest:String,value:JsonValue}
struct Cache {dispatch_started:Option<Instant>,receipts:BTreeMap<String,Receipt>,status:JsonValue,active:bool,working:bool,child:bool,closed:bool,queued_begin:Option<(String,String)>,cancel_attempt:Option<(String,String)>}
impl Default for Cache {fn default()->Self{Self{dispatch_started:None,receipts:BTreeMap::new(),status:JsonValue::Null,active:false,working:false,child:false,closed:false,queued_begin:None,cancel_attempt:None}}}
#[cfg(test)]
#[derive(Clone,Copy,Debug)]
enum PublicationPoint { BeforeHandoffWrites, BeforePublish }
struct Shared {
    #[cfg(test)] publication_probe:Mutex<Option<Box<dyn FnMut(PublicationPoint,&Store)+Send>>>,
    cache:Mutex<Cache>,gate:Mutex<()>,cancel:AtomicBool,close:AtomicBool,game_seen:AtomicBool,control:Mutex<PrepareControl>}
impl Shared {
    #[cfg(test)]
    fn probe(&self,point:PublicationPoint,store:&Store){
        if let Some(probe)=self.publication_probe.lock().unwrap().as_mut(){probe(point,store);}
    }
    fn update(&self,f:impl FnOnce(&mut JsonValue)) {
        let mut cache=self.cache.lock().unwrap_or_else(|p|p.into_inner());
        if cache.status.is_null(){return;}
        let before=cache.status.clone();f(&mut cache.status);
        if before["phase"]!="ready" && cache.status["phase"]=="ready" {cache.working=false;cache.queued_begin=None;}
        if before!=cache.status {let n=u(&before,"sequence").unwrap_or(0);cache.status["sequence"]=n.saturating_add(1).to_string().into();}
    }
    fn receipt(&self,control:&str,result:Result<JsonValue,NativeError>){
        let result=result.and_then(|v|{r::bounded(&v,LIMIT-512)?;Ok(v)});
        let mut cache=self.cache.lock().unwrap_or_else(|p|p.into_inner());
        if let Some(receipt)=cache.receipts.get_mut(control).filter(|r|r.value["state"]=="pending"){
            match result {Ok(value)=>{receipt.value["state"]="succeeded".into();receipt.value["value"]=value;},Err(e)=>{receipt.value["state"]="failed".into();receipt.value["error"]=error_value(&e);}}
        }
    }
    fn cancelled(&self)->bool{self.cancel.load(Ordering::Acquire)||self.close.load(Ordering::Acquire)}
}
struct Command {method:String,params:JsonValue,control:String}
pub(crate) struct LaunchOwner {root:PathBuf,shared:Arc<Shared>,sender:SyncSender<Command>,worker:Option<JoinHandle<()>>,monitor:Arc<Mutex<launch_process::Monitor>>,pool:Arc<ChildPool>,generation:Arc<AtomicU64>}
impl LaunchOwner {
    pub fn new(root:PathBuf,pool:Arc<ChildPool>,monitor:Arc<Mutex<launch_process::Monitor>>)->Result<Self,NativeError>{
        let generation=Arc::new(AtomicU64::new(0));
        let (sender,receiver)=mpsc::sync_channel(1);
        let shared=Arc::new(Shared{#[cfg(test)] publication_probe:Mutex::new(None),cache:Mutex::new(Cache::default()),gate:Mutex::new(()),cancel:AtomicBool::new(false),close:AtomicBool::new(false),game_seen:AtomicBool::new(false),control:Mutex::new(PrepareControl::new())});
        let state=shared.clone();let worker_root=root.clone();let worker_pool=pool.clone();let worker_generation=generation.clone();
        let worker=thread::Builder::new().name("native-launch".into()).spawn(move||Worker::new(worker_root,state,worker_pool,worker_generation).run(receiver)).map_err(|_|NativeError::new("launchWorkerUnavailable","The launch worker could not be created."))?;
        Ok(Self{root,shared,sender,worker:Some(worker),monitor,pool,generation})
    }
    pub fn root(&self)->&Path{&self.root}
    pub fn retiring(&self)->bool{self.shared.close.load(Ordering::Acquire)}
    pub fn busy(&self)->bool{self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).working}
    pub fn blocks_writes(&self)->bool{let c=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());c.active||c.working||c.child||self.shared.close.load(Ordering::Acquire)}
    pub(crate) fn has_control(&self,p:&JsonValue)->bool{p["controlId"].as_str().is_some_and(|id|self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).receipts.contains_key(id))}
    pub(crate) fn joined(&mut self)->bool{
        if let Some(worker)=self.worker.as_ref(){if !worker.is_finished(){return false;}}
        if let Some(worker)=self.worker.take(){if worker.join().is_err(){return false;}self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).closed=true;}
        self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).closed
    }
    pub fn dispatch(&mut self,method:&str,params:&JsonValue)->Result<JsonValue,NativeError>{
        validate(method,params)?;
        if paths::data_root(params)?!=self.root{return Err(NativeError::new("launchRootBusy","Retire the current launch owner before changing libraries."));}
        match method {
            "native.launch.control"=>{
                let c=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
                return Ok(c.receipts.get(s(params,"controlId")?).map(|r|r.value.clone()).unwrap_or_else(||object!{version:1,controlId:params["controlId"].clone(),state:"unknown",inputSha256:JsonValue::Null,value:JsonValue::Null,error:JsonValue::Null}));
            },
            "native.launch.status"=>{
                let c=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
                if c.status["epoch"]!=params["epoch"] || c.status["attemptId"]!=params["attemptId"]{return Ok(object!{version:1,epoch:params["epoch"].clone(),attemptId:params["attemptId"].clone(),phase:"unknown"});}
                return Ok(c.status.clone());
            },
            "native.launch.owner"=>{
                let joined=self.joined();
                let c=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
                return Ok(object!{version:1,state:if joined{"closed"}else if self.shared.close.load(Ordering::Acquire){"closing"}else{"open"},busy:c.working,active:c.active,childRetained:c.child||self.pool.retained_generation(&self.root,self.generation.load(Ordering::Acquire))});
            },
            "native.game.status"=>{
                let scan_started=Instant::now();
                let status=self.monitor.lock().unwrap_or_else(|p|p.into_inner()).status()?;
                let cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
                if status["scanComplete"]==true && status["state"]=="running" && cache.active
                    && cache.dispatch_started.is_some_and(|spawned|scan_started>=spawned) {
                    self.shared.game_seen.store(true,Ordering::Release);
                }
                return Ok(status);
            },
            "native.game.stop"=>return self.monitor.lock().unwrap_or_else(|p|p.into_inner()).stop(s(params,"sessionToken")?),
            "native.launch.cancel"=>{
                // try_lock never makes cancellation wait on SQLite/fsync. The
                // worker owns the serialization point while dispatch is admitted.
                let gate=self.shared.gate.try_lock().ok();
                let target=(s(params,"epoch")?.to_owned(),s(params,"attemptId")?.to_owned());
                let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
                let current=cache.active && cache.status["epoch"]==params["epoch"] && cache.status["attemptId"]==params["attemptId"];
                if !current && cache.queued_begin.as_ref()!=Some(&target){return Err(NativeError::new("unknownLiveLaunch","This helper does not own that live attempt."));}
                cache.cancel_attempt=Some(target);
                // A queued begin records intent only; its own fresh control is
                // cancelled when that identity is published. Never cancel some
                // other attempt's flag/control on behalf of queued history.
                if current {
                    self.shared.cancel.store(true,Ordering::Release);
                    self.shared.control.lock().unwrap_or_else(|p|p.into_inner()).request_cancel();
                }
                if current && cache.status["cancelRequested"]!=true {cache.status["cancelRequested"]=true.into();let n=u(&cache.status,"sequence")?;cache.status["sequence"]=n.saturating_add(1).to_string().into();}
                return Ok(object!{version:1,epoch:params["epoch"].clone(),attemptId:params["attemptId"].clone(),status:if gate.is_some(){"requested"}else{"dispatchInProgress"}});
            },
            "native.launch.close"=>{self.request_close();return Ok(object!{version:1,status:"closing"});},
            _=>{},
        }
        if self.shared.close.load(Ordering::Acquire){return Err(busy());}
        let control=s(params,"controlId")?.to_owned();
        let mut input=operation_input(method,params);input["params"].remove("controlId");
        // Root is part of admission identity; only a hash is cached for settings.
        let digest=r::digest(&object!{rootPath:self.root.to_str(),input:input})?;
        let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
        if let Some(old)=cache.receipts.get(&control){if old.digest!=digest{return Err(NativeError::new("launchControlConflict","This control ID already names a different command."));}return Ok(accepted(&control,&digest));}
        if cache.receipts.len()>=64{return Err(NativeError::new("launchControlLimit","This owner's 64 control receipts are retained; retire it before opening another owner."));}
        if cache.working{return Err(busy());}
        if cache.active && matches!(method,"native.launch.begin"|"native.launch.settings.set"|"native.launch.resolve"|"native.launch.open") {
            // Same begin is allowed through the durable replay-first handler.
            if method!="native.launch.begin" || cache.status["epoch"]!=params["epoch"] || cache.status["attemptId"]!=params["attemptId"] {return Err(busy());}
        }
        let receipt=object!{version:1,controlId:control.clone(),state:"pending",inputSha256:digest.clone(),value:JsonValue::Null,error:JsonValue::Null};
        cache.receipts.insert(control.clone(),Receipt{digest:digest.clone(),value:receipt});cache.working=true;
        if method=="native.launch.begin"{cache.queued_begin=Some((s(params,"epoch")?.into(),s(params,"attemptId")?.into()));}
        let command=Command{method:method.into(),params:params.clone(),control:control.clone()};
        match self.sender.try_send(command){Ok(())=>Ok(accepted(&control,&digest)),Err(TrySendError::Full(_))=>{cache.receipts.remove(&control);cache.working=false;cache.queued_begin=None;Err(busy())},Err(TrySendError::Disconnected(_))=>{cache.receipts.remove(&control);cache.working=false;cache.queued_begin=None;Err(NativeError::new("launchOwnerClosed","The launch worker has retired; reconcile durable identities before opening a new owner."))}}
    }
    fn request_close(&self){let _gate=self.shared.gate.try_lock().ok();self.shared.close.store(true,Ordering::Release);self.shared.cancel.store(true,Ordering::Release);self.shared.control.lock().unwrap_or_else(|p|p.into_inner()).request_cancel();}
}
impl Drop for LaunchOwner {fn drop(&mut self){self.request_close();if let Some(worker)=self.worker.take(){let _=worker.join();}}}
fn accepted(id:&str,digest:&str)->JsonValue{object!{version:1,accepted:true,controlId:id,inputSha256:digest}}

pub(crate) fn validate(method:&str,p:&JsonValue)->Result<(),NativeError>{
    let fields:&[&str]=match method {
        "native.launch.open"=>&["controlId","openId"],
        "native.launch.session"|"native.launch.settings.get"|"native.launch.barriers"=>&["controlId"],
        "native.launch.settings.set"=>&["controlId","epoch","operationId","expectedSettingsRevision","additionalArguments"],
        "native.launch.settings.operation"=>&["controlId","epoch","operationId"],
        "native.launch.begin"=>&["controlId","epoch","attemptId","profileId","mode","expectedRevision","settingsRevision"],
        "native.launch.commit"=>&["controlId","epoch","attemptId","reviewToken","preparationOperationId","preparationInput","settingsRevision"],
        "native.launch.query"=>&["controlId","epoch","attemptId"],
        "native.launch.status"|"native.launch.cancel"=>&["epoch","attemptId"],
        "native.launch.control"=>&["controlId"],
        "native.launch.resolve"=>&["controlId","epoch","journalId","expectedRecordRevision","pairSha256","steamIntentCancelled"],
        "native.game.stop"=>&["sessionToken"],
        "native.game.status"|"native.launch.close"|"native.launch.owner"=>&[],
        _=>return Err(NativeError::new("methodNotFound","Unknown launch control.")),
    };
    let mut keys=vec!["dataDir"];keys.extend(fields);exact(p,&keys)?;let root=s(p,"dataDir")?;
    if root.len()>4096 || !Path::new(root).is_absolute() || paths::absolute(Path::new(root))?!=Path::new(root){return Err(NativeError::params());}
    for key in ["controlId","openId","attemptId","profileId","operationId","journalId","preparationOperationId"] {if p.has_key(key){id(p,key)?;}}
    for key in ["epoch","expectedRevision","settingsRevision","expectedSettingsRevision","expectedRecordRevision"] {if p.has_key(key){let n=u(p,key)?;if key=="epoch" && n==0{return Err(NativeError::params());}}}
    for (key,len) in [("reviewToken",32),("sessionToken",32),("pairSha256",64)] {if p.has_key(key) && !crate::package::hex(s(p,key)?,len){return Err(NativeError::params());}}
    if method=="native.launch.settings.set"{r::validate_arguments(s(p,"additionalArguments")?)?;}
    if method=="native.launch.resolve"{p["steamIntentCancelled"].as_bool().ok_or_else(NativeError::params)?;}
    if matches!(method,"native.launch.begin"|"native.launch.commit") {let mut input=operation_input(method,p);input["params"].remove("controlId");if method.ends_with("begin"){r::begin(&input)}else{r::commit(&input)}.map_err(|_|NativeError::params())?;}
    r::bounded(p,64*1024)
}
struct Live {epoch:u64,id:String,game:PathBuf,review:Option<LiveReview>,begin:JsonValue}
struct Observation {permit:DispatchPermit,game:PathBuf,started:Instant,next:Instant,seen:Vec<launch_process::Identity>}
struct Worker {root:PathBuf,shared:Arc<Shared>,lease:Option<RootOwnerLease>,live:Option<Live>,observation:Option<Observation>,reobserved:Option<String>,child:Option<LaunchChild>,pool:Arc<ChildPool>,reservation:Option<Reservation>,generation:Arc<AtomicU64>}
impl Worker {
    fn new(root:PathBuf,shared:Arc<Shared>,pool:Arc<ChildPool>,generation:Arc<AtomicU64>)->Self{Self{root,shared,lease:None,live:None,observation:None,reobserved:None,child:None,pool,reservation:None,generation}}
    fn lease(&self)->Result<&RootOwnerLease,NativeError>{self.lease.as_ref().ok_or_else(||NativeError::new("launchOwnerNotOpen","Open an explicit launch owner session first."))}
    fn run(mut self,receiver:Receiver<Command>){
        loop {
            if self.shared.close.load(Ordering::Acquire){self.cancel_live();break;}
            self.reap();self.observe();
            if self.shared.cancel.load(Ordering::Acquire){self.cancel_live();}
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(command)=>{
                    if self.shared.close.load(Ordering::Acquire){self.shared.receipt(&command.control,Err(NativeError::new("launchOwnerClosed","The owner retired before this command ran; reconcile durable identities.")));self.cancel_live();break;}
                    // Request identity alone grants no ownership of attempt
                    // status. Replay and pre-admission rejection leave this false.
                    let mut owns_attempt=false;
                    let result=self.command(&command,&mut owns_attempt);
                    if let Err(error)=&result {
                        if owns_attempt && self.live.is_none() && self.observation.is_none() {
                            let query=Store::open_history(&self.root).and_then(|store|store.launch_query(u(&command.params,"epoch")?,s(&command.params,"attemptId")?));
                            self.shared.update(|v|{if v["epoch"]==command.params["epoch"] && v["attemptId"]==command.params["attemptId"] {
                                if let Ok(query)=query {v["attempt"]=query;}
                                publish_owned_failure(v,error,self.shared.cancelled());
                            }});
                            self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;
                        }
                    }
                    let published_ready=command.method=="native.launch.begin" && owns_attempt && result.is_ok();
                    self.shared.receipt(&command.control,result);
                    // Ready already released admission. A newly queued commit owns working now.
                    if !published_ready {let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());cache.working=false;cache.queued_begin=None;}
                },Err(mpsc::RecvTimeoutError::Timeout)=>{},Err(mpsc::RecvTimeoutError::Disconnected)=>{self.cancel_live();break;},
            }
        }
        // Transfer a still-owned actual child to the service pool before the
        // worker and its root lease retire. Reap only already exited children.
        self.reap();self.child.take();self.lease.take();
        let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());cache.working=false;cache.active=false;cache.child=false;
        for receipt in cache.receipts.values_mut().filter(|r|r.value["state"]=="pending") {receipt.value["state"]="failed".into();receipt.value["error"]=error_value(&NativeError::new("launchOwnerClosed","The owner retired; reconcile durable identities before retrying."));}
    }
    fn command(&mut self,c:&Command,owns_attempt:&mut bool)->Result<JsonValue,NativeError>{
        let p=&c.params;
        match c.method.as_str() {
            "native.launch.open"=>{
                if self.lease.is_none(){self.lease=Some(RootOwnerLease::acquire(&self.root)?);}
                let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;
                store.launch_owner_open(&lock,self.lease.as_mut().unwrap(),s(p,"openId")?)
            },
            "native.launch.begin"=>self.begin(c,owns_attempt),
            "native.launch.commit"=>self.commit(c,owns_attempt),
            "native.launch.resolve"=>self.resolve(p),
            "native.launch.barriers"=>{
                // A restarted owner has no live permit. Reconcile only an aged,
                // intact marker, and leave every other barrier visible.
                if self.lease.is_none() {let _=self.reconcile_stale();}
                Store::open_history(&self.root)?.launch_barriers()
            },
            "native.launch.settings.set"=>{
                if self.live.is_some() || self.observation.is_some(){return Err(busy());}
                let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;
                store.launch_settings_set(&lock,self.lease()?,u(p,"epoch")?,s(p,"operationId")?,u(p,"expectedSettingsRevision")?,s(p,"additionalArguments")?)
            },
            method=>{let store=Store::open_history(&self.root)?;match method{
                "native.launch.session"=>store.launch_session(),"native.launch.settings.get"=>store.launch_settings(),
                "native.launch.settings.operation"=>store.launch_settings_operation(u(p,"epoch")?,s(p,"operationId")?),
                "native.launch.query"=>store.launch_query(u(p,"epoch")?,s(p,"attemptId")?),_=>Err(NativeError::params()),
            }},
        }
    }
    fn reconcile_stale(&mut self)->Result<(),NativeError>{
        let history=Store::open_history(&self.root)?;
        let barriers=history.launch_barriers()?;
        let Some(row)=barriers["barriers"].members().next() else{return Ok(());};
        let journal=s(row,"journalId")?;
        let body=history.journal(journal)?.ok_or_else(r::invalid)?;
        let resuming=body["stage"]=="settling" && matches!(body["settlement"]["reason"].as_str(),Some("startTimedOut"|"gameCycleCompleted"));
        if row["stage"]!="dispatchIntent" && !resuming {return Ok(());}
        let observable=!resuming && history.handoff_can_observe(journal)?;
        let scan=launch_process::scan();
        if !scan.complete {return Ok(());}
        if !scan.games.is_empty() {if observable {self.reobserved=Some(journal.to_owned());}return Ok(());}
        let observed=observable && self.reobserved.as_deref()==Some(journal);
        if !resuming && !observed && !history.handoff_expired(journal)? {return Ok(());}
        let mut lease=RootOwnerLease::acquire(&self.root)?;
        let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;
        store.launch_owner_open(&lock,&mut lease,&paths::random_id()?)?;
        let claim=store.settlement_claim(&lock,&lease,journal,u(row,"recordRevision")?,s(row,"pairSha256")?)?;
        let body=store.journal(journal)?.ok_or_else(r::invalid)?;let game_path=PathBuf::from(s(&body["binding"],"gamePath")?);
        let game=GameLock::acquire_for_settlement(&store,&lock,&game_path.join("game/citadel"),&claim)?;
        let idle=launch_process::idle(&game_path)?;
        if resuming {store.resume_settlement(&lock,&lease,&game,claim,idle)?;} else if observed {store.settle_reobserved(&lock,&lease,&game,claim,idle)?;} else {store.settle_timed_out(&lock,&lease,&game,claim,idle)?;}
        self.reobserved=None;Ok(())
    }
    fn begin(&mut self,c:&Command,owns_attempt:&mut bool)->Result<JsonValue,NativeError>{
        let mut input=operation_input(&c.method,&c.params);input["params"].remove("controlId");let p=&input["params"];let epoch=u(p,"epoch")?;let id=s(p,"attemptId")?.to_owned();
        let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;let lease=self.lease()?;
        let previous=store.launch_query(epoch,&id)?;
        if previous["state"]!="unknown" {return store.launch_begin(&lock,lease,&input);}
        if self.live.is_some() || self.observation.is_some() || self.child.is_some(){return Err(busy());}
        if self.reservation.is_none()&&!self.pool.has_capacity(){return Err(busy());}
        let query=store.launch_begin(&lock,lease,&input)?;
        self.shared.game_seen.store(false,Ordering::Release);
        *owns_attempt=true; // This command admitted new durable work, not history.
        let control=PrepareControl::new();let mut status=initial_status(epoch,&id,query.clone());
        {let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());
            let cancelled=cache.cancel_attempt.as_ref()==Some(&(epoch.to_string(),id.clone())) || self.shared.close.load(Ordering::Acquire);
            self.shared.cancel.store(cancelled,Ordering::Release);if cancelled{control.request_cancel();status["cancelRequested"]=true.into();}
            *self.shared.control.lock().unwrap_or_else(|p|p.into_inner())=control.clone();cache.status=status;cache.active=true;cache.dispatch_started=None;
        }
        // The control receipt confirms durable identity only. Review is still live work.
        self.shared.receipt(&c.control,Ok(query));
        let result=(||{
            control.check()?;let game=PathBuf::from(s(&store.document()?,"gamePath")?);
            let game_lock=GameLock::acquire(&game.join("game/citadel"))?;launch_process::idle(&game)?;
            let plan=if p["mode"]=="vanilla"{PreparationPlan::review_vanilla(&store,&lock,&game_lock,&game,&control,&mut |_|{})?}else{PreparationPlan::review(&store,&lock,&game_lock,s(p,"profileId")?,&game,&control,&mut |_|{})?};
            let review=store.launch_ready(&lock,lease,epoch,&id,0,plan,&paths::random_id()?)?;
            let ready_query=store.launch_query(epoch,&id)?;admit_status(&ready_query)?;
            Ok::<_,NativeError>((review,ready_query,game))
        })();
        match result {
            Ok((review,query,game))=>{self.live=Some(Live{epoch,id,game,review:Some(review),begin:input});self.shared.update(|v|{v["phase"]="ready".into();v["attempt"]=query.clone();v["preparationState"]="reviewed".into();});Ok(query)},
            // A concurrent Cancel does not turn an unrelated failure into success.
            Err(e)=>{let cancelled=self.shared.cancelled() && e.kind=="localPreparationCancelled";let query=finish_before(&store,&lock,lease,epoch,&id,cancelled)?;self.shared.update(|v|{v["attempt"]=query.clone();v["phase"]=if cancelled{"cancelled"}else{"failed"}.into();v["error"]=error_value(&e);});self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;Err(e)},
        }
    }
    fn commit(&mut self,c:&Command,owns_attempt:&mut bool)->Result<JsonValue,NativeError>{
        let mut input=operation_input(&c.method,&c.params);input["params"].remove("controlId");let epoch=u(&input["params"],"epoch")?;let id=s(&input["params"],"attemptId")?.to_owned();
        let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;
        if let Some(query)=store.launch_commit_replay(&lock,self.lease()?,&input)?{return Ok(query);}
        let live=self.live.as_ref().filter(|l|l.epoch==epoch && l.id==id).ok_or_else(||NativeError::new("launchReviewExpired","Only this owner's live review may commit; historical ready data grants no launch permission."))?;
        if live.review.is_none(){return Err(busy());}
        // Validate exact review before consuming it, including the canonical action.
        let ready=live.review.as_ref().unwrap().ready();let p=&input["params"];
        if p["reviewToken"]!=ready["reviewToken"] || p["preparationOperationId"]!=ready["preparationOperationId"] || p["preparationInput"]!=ready["preparationInput"] || p["settingsRevision"]!=live.begin["params"]["settingsRevision"]{return Err(NativeError::new("launchIdentityConflict","Commit does not match the exact live preparation review."));}
        if self.reservation.is_none(){let reservation=self.pool.reserve(&self.root)?;self.generation.store(reservation.inner.id,Ordering::Release);self.reservation=Some(reservation);}
        let reservation=self.reservation.as_ref().unwrap().clone();
        // Only the exact live review may transfer attempt work to this command.
        // A consuming admission failure must retire that review, unlike replay.
        *owns_attempt=true;
        let live=self.live.take().unwrap();let game_path=live.game.clone();let lease=self.lease()?;
        let operation=store.launch_commit(&lock,lease,live.review.unwrap(),&input)?;
        let CommitAdmission::First(operation)=operation else{return store.launch_query(epoch,&id);};
        let query=store.launch_query(epoch,&id)?;self.shared.update(|v|{v["phase"]="preparing".into();v["preparationState"]="executing".into();v["attempt"]=query.clone();});self.shared.receipt(&c.control,Ok(query));
        let shared=self.shared.clone();let control=shared.control.lock().unwrap_or_else(|p|p.into_inner()).clone();
        let mut retained_query=LaunchChild::empty(reservation.clone());
        let result=(||{
            let game=GameLock::acquire(&game_path.join("game/citadel"))?;
            launch_process::idle(&game_path)?;control.check()?;
            let route=launch_routes::discover(&mut retained_query.child)?;control.check()?;
            shared.update(|v|v["route"]=route.value());
            let mut committed=false;
            let prepared=operation.prepare(&store,&lock,lease,&game,&control,&mut |event|{
                // Unit callbacks cannot wait for an observer or panic if it left.
                let Ok(mut cache)=shared.cache.try_lock() else{return;};
                let v=&mut cache.status;if v.is_null(){return;}
                let progress=match event {
                    PreparationProgress::Repairing{checked,total}=>object!{kind:"repairing",checked:checked,total:total},
                    PreparationProgress::Verifying=>{if committed{v["phase"]="verifyingPrepared".into();}object!{kind:"verifying"}},
                    PreparationProgress::Committing=>{committed=true;object!{kind:"committing"}},
                    PreparationProgress::Copying{files_done,files_total,bytes_done,bytes_total}=>object!{kind:"copying",filesDone:files_done.to_string(),filesTotal:files_total.to_string(),bytesDone:bytes_done.to_string(),bytesTotal:bytes_total.to_string()},
                };v["progress"]=progress;let n=u(v,"sequence").unwrap_or(0);v["sequence"]=n.saturating_add(1).to_string().into();
            })?;
            // Do not expose the preparation-bearing attempt yet: the GUI reads
            // its outcome as soon as it is visible, while handoff commits follow.
            #[cfg(test)] shared.probe(PublicationPoint::BeforeHandoffWrites,&store);
            if shared.cancelled(){let q=finish_before(&store,&lock,lease,epoch,&id,true)?;return Ok((q,None,None));}
            let claim=store.reserve_handoff(&lock,lease,&game,prepared,&paths::random_id()?)?;
            shared.update(|v|{v["phase"]="openingSteam".into();v["handoffState"]="armed".into();v["progress"]=JsonValue::Null;});
            // The single gate serializes cancellation with durable admission and
            // the only spawn syscall. No route retry/fallback follows this point.
            let _gate=shared.gate.lock().unwrap_or_else(|p|p.into_inner());
            if shared.cancelled(){
                drop(game);let q=settle_current(&store,&lock,lease,&game_path,false)?;return Ok((q,None,None));
            }
            let fresh=launch_process::idle(&game_path)?;
            let permit=match store.admit_dispatch(&lock,lease,&game,claim,fresh) {
                Ok(permit)=>permit,
                Err(error)=>{
                    drop(game);
                    let barriers=store.launch_barriers()?;
                    if barriers["barriers"].members().any(|b|b["stage"]=="armed") {
                        settle_current(&store,&lock,lease,&game_path,true)?;
                    }
                    return Err(error);
                }
            };
            let query=store.launch_query(epoch,&id)?;
            let raw=store.launch_settings()?;let raw=s(&raw,"additionalArguments")?;
            // If EOF/cancel arrived while admission was syncing, preserve the
            // dispatch-intent barrier but suppress the still-unissued syscall.
            if shared.cancelled(){return Ok((query,None,None));}
            match route.spawn(raw) {
                Ok(child)=>{let spawned=Instant::now();shared.cache.lock().unwrap_or_else(|p|p.into_inner()).dispatch_started=Some(spawned);let child=LaunchChild::new(child,reservation.clone());let _=store.record_dispatch(&lock,lease,&permit);Ok((query,Some(child),Some(Observation{permit,game:game_path.clone(),started:spawned,next:spawned,seen:Vec::new()})))},
                Err(error)=>{
                    shared.update(|v|{v["handoffState"]="spawnFailed".into();v["error"]=error_value(&error);});
                    let idle=launch_process::idle(&game_path)?;
                    let q=permit.settle_spawn_failed(&store,&lock,lease,&game,idle)?;Ok((q,None,None))
                },
            }
        })();
        match result {
            Ok((query,child,observation))=>{
                let dispatched=child.is_some();self.child=child;self.observation=observation;
                #[cfg(test)] self.shared.probe(PublicationPoint::BeforePublish,&store);
                // Preparation, handoff admission and dispatch recording have
                // finished writing before the cache invites an outcome read.
                self.shared.update(|v|{v["preparationState"]="prepared".into();v["progress"]=JsonValue::Null;v["attempt"]=query.clone();v["phase"]=if dispatched{"waitingForGame"}else if query["outcome"]["status"]=="cancelled"{"cancelled"}else if query["outcome"]["reason"]=="spawnFailed"{"failed"}else{"cancelled"}.into();if dispatched{v["handoffState"]="dispatched".into();v["observationState"]="waiting".into();v["launcher"]=object!{state:"running",exitCode:JsonValue::Null};}else if query["state"]!="terminal"{v["handoffState"]="uncertain".into();v["observationState"]="retired".into();}else if query["handoff"]["stage"]=="settled" && query["outcome"]["reason"]!="spawnFailed"{v["handoffState"]="settled".into();}});
                let mut cache=self.shared.cache.lock().unwrap_or_else(|p|p.into_inner());cache.active=dispatched;cache.child=dispatched;Ok(query)
            },
            Err(error)=>{
                if retained_query.child.is_some(){self.child=Some(retained_query);self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).child=true;self.shared.update(|v|v["launcher"]=object!{state:"queryRetained",exitCode:JsonValue::Null});}
                record_preparation_failure(&store,&lock,&input["params"]["preparationInput"],&error)?;
                let query=store.launch_query(epoch,&id)?;
                let query=if query["handoff"].is_null() && query["state"]!="terminal"{finish_before(&store,&lock,self.lease()?,epoch,&id,self.shared.cancelled() && error.kind=="localPreparationCancelled")?}else{query};
                self.shared.update(|v|{v["attempt"]=query.clone();publish_owned_failure(v,&error,self.shared.cancelled());v["preparationState"]=if query["preparation"].is_null(){"notConfirmed"}else{"prepared"}.into();v["progress"]=JsonValue::Null;});
                self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;Err(error)
            },
        }
    }
    fn cancel_live(&mut self){
        if self.observation.take().is_some(){self.shared.update(|v|{v["phase"]="cancelled".into();v["observationState"]="retired".into();v["handoffState"]="uncertain".into();});}
        if let Some(live)=self.live.take(){
            let result=(||{let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;finish_before(&store,&lock,self.lease()?,live.epoch,&live.id,true)})();
            self.shared.update(|v|{v["phase"]="cancelled".into();match result{Ok(q)=>v["attempt"]=q,Err(e)=>v["error"]=error_value(&e)}});
        }
        self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;
    }
    fn reap(&mut self){
        let Some(holder)=self.child.as_mut() else{return;};let Some(child)=holder.child.as_mut() else{return;};
        match child.try_wait(){
            Ok(Some(exit))=>{holder.child.take();self.child=None;self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).child=false;self.shared.update(|v|v["launcher"]=object!{state:"exited",exitCode:exit.code()});},
            Err(_)=>{self.shared.update(|v|v["launcher"]=object!{state:"unavailable",exitCode:JsonValue::Null});},
            Ok(None)=>{},
        }
    }
    fn observe(&mut self){
        let Some(mut observation)=self.observation.take() else{return;};
        if self.shared.cancelled(){self.observation=Some(observation);return;}
        // The deadline bounds startup only. Once a complete scan has seen the
        // game, retain this live permit for its cycle even across unavailable scans.
        if Instant::now()<observation.next{self.observation=Some(observation);return;}
        observation.next=Instant::now()+Duration::from_secs(2);
        let scan=launch_process::scan();
        if !scan.complete{self.shared.update(|v|v["observationState"]="unavailable".into());self.observation=Some(observation);return;}
        if !scan.games.is_empty(){
            self.shared.game_seen.store(true,Ordering::Release);
            observation.seen=scan.games;self.shared.update(|v|{v["phase"]="running".into();v["observationState"]="running".into();});self.observation=Some(observation);return;
        }
        if observation.seen.is_empty() && !self.shared.game_seen.load(Ordering::Acquire){
            if observation.started.elapsed()>=Duration::from_secs(180) {
                let result=(||{
                    let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;let lease=self.lease()?;
                    let barriers=store.launch_barriers()?;let row=&barriers["barriers"][0];
                    let claim=store.settlement_claim(&lock,lease,s(row,"journalId")?,u(row,"recordRevision")?,s(row,"pairSha256")?)?;
                    let game=GameLock::acquire_for_settlement(&store,&lock,&observation.game.join("game/citadel"),&claim)?;
                    let idle=launch_process::idle(&observation.game)?;
                    store.settle_timed_out(&lock,lease,&game,claim,idle)
                })();
                if let Ok(q)=result {self.shared.update(|v|{v["attempt"]=q;v["phase"]="resolved".into();v["observationState"]="retired".into();v["handoffState"]="settled".into();});self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;return;}
            }
            self.shared.update(|v|v["observationState"]="waiting".into());self.observation=Some(observation);return;
        }
        // A complete fresh scan after an observed post-dispatch start is the only
        // live-cycle evidence. Reacquire root BEFORE the typed game guard.
        let result=(||{
            let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;let lease=self.lease()?;
            let game=GameLock::acquire_for_handoff(&store,&lock,&observation.game.join("game/citadel"),observation.permit.claim())?;
            let idle=launch_process::idle(&observation.game)?;
            let cycle=ObservedGameCycle::from_completed_cycle(&observation.permit,observation.game.clone());
            observation.permit.settle_game_cycle(&store,&lock,lease,&game,idle,cycle)
        })();
        self.shared.update(|v|match result{Ok(q)=>{v["attempt"]=q;v["phase"]="completed".into();v["observationState"]="exited".into();v["handoffState"]="settled".into();},Err(e)=>{v["phase"]="failed".into();v["observationState"]="retired".into();v["handoffState"]="uncertain".into();v["error"]=error_value(&e);}});
        self.shared.cache.lock().unwrap_or_else(|p|p.into_inner()).active=false;
    }
    fn resolve(&mut self,p:&JsonValue)->Result<JsonValue,NativeError>{
        if self.live.is_some() || self.observation.is_some(){return Err(busy());}
        let lock=RootLock::acquire(&self.root)?;let store=Store::open_writer(&lock)?;let lease=self.lease()?;
        if lease.epoch()!=Some(u(p,"epoch")?){return Err(NativeError::new("staleLaunchEpoch","Resolution requires the current owner's epoch."));}
        let journal=s(p,"journalId")?;let claim=store.settlement_claim(&lock,lease,journal,u(p,"expectedRecordRevision")?,s(p,"pairSha256")?)?;
        let body=store.journal(journal)?.ok_or_else(r::invalid)?;let game_path=PathBuf::from(s(&body["binding"],"gamePath")?);
        let game=GameLock::acquire_for_settlement(&store,&lock,&game_path.join("game/citadel"),&claim)?;let idle=launch_process::idle(&game_path)?;
        let result=match s(&body,"stage")?{"reserved"|"armed"=>store.settle_before_dispatch(&lock,lease,&game,claim,idle),"settling"=>store.resume_settlement(&lock,lease,&game,claim,idle),_=>store.resolve_launch(&lock,lease,&game,claim,idle,p["steamIntentCancelled"]==true)}?;
        self.shared.update(|v|{if v["epoch"]==result["epoch"] && v["attemptId"]==result["attemptId"]{v["attempt"]=result.clone();v["phase"]="resolved".into();v["handoffState"]="settled".into();}});Ok(result)
    }
}
// Called only after a command acquired real attempt work. Durable terminal
// outcomes outrank a later command error; a non-null settled handoff is not uncertain.
fn publish_owned_failure(status:&mut JsonValue,error:&NativeError,cancelled:bool){
    status["error"]=error_value(error);
    if status["attempt"]["state"]=="terminal" {
        status["phase"]=match status["attempt"]["outcome"]["status"].as_str(){
            Some("observed")=>"completed",Some("resolved")=>"resolved",Some("cancelled")=>"cancelled",_=>"failed",
        }.into();
        if status["attempt"]["outcome"]["status"]=="observed"{status["observationState"]="exited".into();}
    }else{status["phase"]=if cancelled && error.kind=="localPreparationCancelled"{"cancelled"}else{"failed"}.into();}
    if status["attempt"]["handoff"]["stage"]=="settled" {
        status["handoffState"]=if status["attempt"]["outcome"]["reason"]=="spawnFailed"{"spawnFailed"}else{"settled"}.into();
    }else if !status["attempt"]["handoff"].is_null() && status["handoffState"]!="spawnFailed"{status["handoffState"]="uncertain".into();}
}
fn initial_status(epoch:u64,id:&str,attempt:JsonValue)->JsonValue{object!{version:1,epoch:epoch.to_string(),attemptId:id,sequence:"0",phase:"reviewing",attempt:attempt,preparationState:"reviewing",handoffState:"notDispatched",observationState:"notStarted",progress:JsonValue::Null,route:JsonValue::Null,launcher:JsonValue::Null,error:JsonValue::Null,cancelRequested:false}}
fn admit_status(query:&JsonValue)->Result<(),NativeError>{
    let mut widest=initial_status(u(query,"epoch")?,s(query,"attemptId")?,query.clone());widest["sequence"]=u64::MAX.to_string().into();widest["phase"]="verifyingPrepared".into();widest["preparationState"]="notConfirmed".into();widest["handoffState"]="notDispatched".into();widest["observationState"]="unavailable".into();
    widest["route"]=object!{kind:"flatpakSystem",eligibility:"installedPackage"};widest["launcher"]=object!{state:"queryRetained",exitCode:i32::MIN};widest["error"]=object!{kind:"x".repeat(64),message:"\\".repeat(256)};
    widest["progress"]=object!{kind:"copying",filesDone:u64::MAX.to_string(),filesTotal:u64::MAX.to_string(),bytesDone:u64::MAX.to_string(),bytesTotal:u64::MAX.to_string()};
    // Reserve future preparation/handoff/outcome as well as the transport wrapper.
    widest["attempt"]["recordRevision"]=u64::MAX.to_string().into();widest["attempt"]["state"]="preparing".into();
    widest["attempt"]["preparation"]=object!{operationId:query["ready"]["preparationOperationId"].clone(),input:query["ready"]["preparationInput"].clone(),resultingRevision:u64::MAX.to_string(),changed:false,proofSha256:"f".repeat(64),gameinfoSha256:"f".repeat(64)};
    widest["attempt"]["handoff"]=object!{journalId:"x".repeat(64),stage:"dispatchIntent"};widest["attempt"]["outcome"]=object!{status:"cancelled",reason:"steamIntentCancelled"};r::bounded(&widest,LIMIT)
}
fn finish_before(store:&Store,lock:&RootLock,lease:&RootOwnerLease,epoch:u64,id:&str,cancelled:bool)->Result<JsonValue,NativeError>{
    let q=store.launch_query(epoch,id)?;store.launch_finish_before_handoff(lock,lease,epoch,id,u(&q,"recordRevision")?,if cancelled{TerminalBeforeHandoff::Cancelled}else{TerminalBeforeHandoff::PreparationFailed})
}
fn settle_current(store:&Store,lock:&RootLock,lease:&RootOwnerLease,game_path:&Path,failed:bool)->Result<JsonValue,NativeError>{
    let barriers=store.launch_barriers()?;let row=&barriers["barriers"][0];let claim=store.settlement_claim(lock,lease,s(row,"journalId")?,u(row,"recordRevision")?,s(row,"pairSha256")?)?;
    let game=GameLock::acquire_for_settlement(store,lock,&game_path.join("game/citadel"),&claim)?;let idle=launch_process::idle(game_path)?;
    if failed{store.settle_failed_before_dispatch(lock,lease,&game,claim,idle)}else{store.settle_before_dispatch(lock,lease,&game,claim,idle)}
}

// Match ordinary mutation outcome semantics: only a definite rejected operation
// with no outstanding writer journal may get a failed canonical outcome.
fn record_preparation_failure(store:&Store,lock:&RootLock,input:&JsonValue,error:&NativeError)->Result<(),NativeError>{
    if matches!(error.kind.as_str(),"invalidParams"|"operationIdConflict"|"nativeBusy"|"nativeDatabase"|"nativeIo"|"recoveryRequired"|"localJobOutcomeUnknown"|"unsupportedVersion") {return Ok(());}
    if !store.journals()?.is_empty(){return Ok(());}
    let id=s(&input["params"],"operationId")?;
    if store.operation(id)?["status"]=="unknown" {
        let safe=error_value(error);store.record_failure(lock,id,input,&NativeError::new(s(&safe,"kind")?,s(&safe,"message")?))?;
    }
    Ok(())
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn item115_diagnostics_truncation_keeps_latest_cause(){
        let e=NativeError::new("recoveryRequired",format!("{} Cause during Restore: gameRunning: close game; refusal check failed: nativeIo: storageFull: {}","backup ".repeat(40),"é猫".repeat(100)));
        let v=error_value(&e);assert_eq!(v["kind"],"recoveryRequired");let m=v["message"].as_str().unwrap();assert!(m.len()<=256);assert!(m.ends_with('…'));assert!(m.contains("nativeIo"));assert!(m.contains("storageFull"));
    }
    #[test]
    fn game_seen_before_dispatch_does_not_complete_the_handoff() {
        let dir = std::env::temp_dir().join(format!("pre-dispatch-{}", crate::paths::random_id().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = format!("handoff-{}.exe", crate::paths::random_id().unwrap());
        launch_process::TEST_GAME_NAME.with(|v| *v.borrow_mut() = Some(name.clone()));
        let executable = dir.join(&name);
        std::fs::copy("/bin/sleep", &executable).unwrap();
        assert!(launch_process::scan().games.is_empty(), "unique test process must start absent");
        let mut game = std::process::Command::new(&executable).arg("30").spawn().unwrap();
        let pool = ChildPool::new().unwrap();
        let mut owner = LaunchOwner::new(dir.clone(), pool.clone(), Arc::new(Mutex::new(launch_process::Monitor::default()))).unwrap();
        {
            let mut cache = owner.shared.cache.lock().unwrap();
            cache.active = true;
            cache.status = object! {phase:"openingSteam", sequence:"0"};
        }
        let status = owner.dispatch("native.game.status", &object! {dataDir:dir.to_str().unwrap()}).unwrap();
        assert_eq!(status["state"], "running");
        let seen = owner.shared.game_seen.load(Ordering::Acquire);
        owner.shared.cache.lock().unwrap().dispatch_started = Some(Instant::now());
        owner.dispatch("native.game.status", &object! {dataDir:dir.to_str().unwrap()}).unwrap();
        let seen_after_spawn = owner.shared.game_seen.load(Ordering::Acquire);
        game.kill().unwrap(); game.wait().unwrap();
        drop(owner); pool.shutdown(); std::fs::remove_dir_all(dir).unwrap();
        launch_process::TEST_GAME_NAME.with(|v| *v.borrow_mut() = None);
        assert!(!seen, "a game seen before successful spawn cannot prove a game cycle");
        assert!(seen_after_spawn, "a complete post-spawn scan can prove a game cycle");
    }

    #[test]
    fn ready_publication_releases_admission() {
        let shared = Shared {
            publication_probe:Mutex::new(None),
            cache: Mutex::new(Cache { status: object! {sequence:"0", phase:"reviewing"}, working: true, ..Cache::default() }),
            gate: Mutex::new(()), cancel: AtomicBool::new(false), close: AtomicBool::new(false), game_seen: AtomicBool::new(false),
            control: Mutex::new(PrepareControl::new()),
        };
        shared.update(|status| status["phase"] = "ready".into());
        let cache = shared.cache.lock().unwrap();
        assert_eq!(cache.status["phase"], "ready");
        assert!(!cache.working, "ready must be published with admission available");
    }
}

#[cfg(test)]
#[path="launch_owner_publication_tests.rs"]
mod publication_tests;
