//! Dispatch only touches bounded memory. Worker owns every filesystem handle.
use super::{contract as c, transport::MetadataTransport};
use crate::{native::NativeError, paths};
use json::{object, JsonValue};
use std::{collections::{BTreeMap, VecDeque}, fs::{self, File, OpenOptions}, os::{fd::AsRawFd, unix::fs::{MetadataExt, OpenOptionsExt}}, path::{Path, PathBuf}, sync::{Arc, Mutex, Condvar, atomic::{AtomicBool, Ordering}}, thread::{self, JoinHandle}, time::Duration};

#[derive(Clone)]
pub struct Work { pub request_key: String, pub operation: JsonValue, pub cancelled: Arc<AtomicBool> }
/// A step must perform at most one bounded cache transaction/page. Pending work
/// is rotated behind ready reads; it must never synchronously wait for network.
pub enum Step { Pending, Complete(Result<JsonValue, NativeError>) }
pub trait Executor {
    fn step(&mut self, work: &Work, transport: &mut MetadataTransport) -> Step;
    fn idle(&mut self)->Result<(),NativeError>{Ok(())}
    fn retry_after(&self,_key:&str)->Option<u32>{None}
    fn progress(&self,_key:&str)->Option<JsonValue>{None}
    /// Override when one operation owns multiple metadata request IDs. A step
    /// that already committed must return its real Complete result directly.
    fn cancel(&mut self, work:&Work, transport:&mut MetadataTransport) {
        transport.discard(work.request_key.clone());
    }
}
struct Receipt { digest: String, generation: String, operation: JsonValue, phase: &'static str, progress:JsonValue, result: JsonValue, error: JsonValue, cancel: Arc<AtomicBool> }
impl Receipt {
    fn terminal(&self) -> bool { matches!(self.phase, "succeeded" | "failed" | "cancelled" | "released") }
    fn status(&self, root: &str, session: &str, key: &str) -> JsonValue {
        object!{version:1,root:root,catalogSession:session,requestKey:key,viewGeneration:self.generation.clone(),inputDigest:self.digest.clone(),operation:self.operation.clone(),phase:self.phase,progress:self.progress.clone(),result:self.result.clone(),error:self.error.clone()}
    }
}
struct State { root: String, phase: &'static str, worker_exited: bool, error: JsonValue, receipts: BTreeMap<String, Receipt>, queue: VecDeque<Work> }
struct Shared { state: Mutex<State>, wake: Condvar, close: AtomicBool, launch_root: Mutex<Option<PathBuf>> }
pub struct CatalogOwner { lexical: PathBuf, session: String, shared: Arc<Shared>, worker: Option<JoinHandle<()>>, previous_close: Option<(String, String)> }
impl CatalogOwner {
    pub fn new(root: PathBuf) -> Result<Self, NativeError> {
        let session = c::new_token()?;
        let shared = Arc::new(Shared { state: Mutex::new(State { root: root.to_str().ok_or_else(NativeError::params)?.into(), phase:"opening", worker_exited:false, error:JsonValue::Null, receipts:BTreeMap::new(), queue:VecDeque::new() }), wake:Condvar::new(), close:AtomicBool::new(false), launch_root:Mutex::new(None) });
        {
            let state=shared.state.lock().unwrap_or_else(|p|p.into_inner());
            let value=object!{version:1,root:state.root.clone(),catalogSession:session.clone(),phase:"opening",error:JsonValue::Null};
            if !c::bounded(&value,c::CONTROL_LIMIT-c::WRAPPER_RESERVE){return Err(c::error("responseTooLarge"));}
        }
        // Opening is started by open dispatch, after the launch root constraint
        // is supplied. No filesystem work occurs in this constructor.
        Ok(Self { lexical:root, session, shared, worker:None, previous_close:None })
    }
    pub fn constrain_launch_root(&self, root: &Path) -> Result<(), NativeError> {
        let state = self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if state.phase != "closed" && root != self.lexical && root != Path::new(&state.root) { return Err(NativeError::new("launchRootBusy", "Catalog and launch must use the same data root.")); }
        *self.shared.launch_root.lock().unwrap_or_else(|p|p.into_inner()) = Some(root.to_owned());
        Ok(())
    }
    pub fn request_close(&self) {
        self.shared.close.store(true, Ordering::Release);
        let mut s = self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if s.phase != "closed" { s.phase = if s.worker_exited {"closed"}else{"closing"}; s.error = JsonValue::Null; }
        if s.phase == "closed" { s.receipts.clear(); s.queue.clear(); }
        for r in s.receipts.values() { r.cancel.store(true, Ordering::Release); }
        self.shared.wake.notify_all();
    }
    fn session_status(&self, s: &State) -> JsonValue {
        object!{version:1,root:s.root.clone(),catalogSession:self.session.clone(),phase:s.phase,error:if s.phase=="failed"{s.error.clone()}else{JsonValue::Null}}
    }
    pub fn dispatch(&mut self, method: &str, params: &JsonValue) -> Result<JsonValue, NativeError> {
        let root = c::validate(method, params)?;
        if method == "native.catalog.close" {
            if let Some((session, previous_root)) = &self.previous_close {
                if params["catalogSession"] == session.as_str() && root == Path::new(previous_root) { return Ok(object!{version:1,root:previous_root.clone(),catalogSession:session.clone(),phase:"closed"}); }
            }
        }
        let closed = self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).phase == "closed";
        if method == "native.catalog.open" && closed {
            let old_root = self.shared.state.lock().unwrap_or_else(|p|p.into_inner()).root.clone();
            let mut fresh = Self::new(root.clone())?;
            fresh.previous_close = Some((self.session.clone(), old_root));
            *fresh.shared.launch_root.lock().unwrap_or_else(|p|p.into_inner()) = self.shared.launch_root.lock().unwrap_or_else(|p|p.into_inner()).clone();
            // The closed marker is published only after transport/lease teardown.
            // Detach the finished tail: never join on main RPC dispatch.
            self.worker.take();
            std::mem::swap(self, &mut fresh);
        }
        let mut s = self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if root != self.lexical && root != Path::new(&s.root) { return Err(c::error("catalogRootBusy")); }
        if method == "native.catalog.open" {
            let result = self.session_status(&s);
            if self.worker.is_none() && s.phase == "opening" {
                let shared = self.shared.clone(); let root = self.lexical.clone(); let session = self.session.clone();
                match thread::Builder::new().name("native-catalog".into()).spawn(move || run(root, session, shared)) {
                    Ok(worker)=>self.worker=Some(worker),
                    Err(_)=>{s.phase="failed";s.worker_exited=true;s.error=c::failure("catalogUnavailable","Cannot start catalog worker.");return Ok(self.session_status(&s));}
                }
            }
            return Ok(result);
        }
        if params["catalogSession"] != self.session { return Err(c::error("catalogSessionMismatch")); }
        if method == "native.catalog.close" {
            drop(s); self.request_close();
            let s = self.shared.state.lock().unwrap_or_else(|p|p.into_inner());
            return Ok(object!{version:1,root:s.root.clone(),catalogSession:self.session.clone(),phase:s.phase});
        }
        if method == "native.catalog.status" && params["requestKey"].is_null() { return Ok(self.session_status(&s)); }
        let key = c::token(&params["requestKey"])?;
        if method == "native.catalog.begin" {
            if s.phase != "ready" { return Err(c::error(if matches!(s.phase,"closed"|"closing"){"catalogClosed"}else{"catalogUnavailable"})); }
            super::query::validate_operation(&params["operation"])?;
            let digest = c::digest(&object!{root:s.root.clone(),catalogSession:self.session.clone(),requestKey:key,viewGeneration:params["viewGeneration"].clone(),operation:params["operation"].clone()});
            if let Some(r) = s.receipts.get(key) {
                if r.digest != digest || r.phase == "released" { return Err(c::error("catalogKeyConflict")); }
            } else {
                if s.receipts.len() >= c::SESSION_KEYS { return Err(c::error("catalogSessionExhausted")); }
                if s.receipts.values().filter(|r|r.phase != "released").count() >= c::RECEIPTS { return Err(c::error("catalogBusy")); }
                let r = Receipt { digest, generation:params["viewGeneration"].as_str().unwrap().into(), operation:params["operation"].clone(), phase:"queued", progress:JsonValue::Null, result:JsonValue::Null, error:JsonValue::Null, cancel:Arc::new(AtomicBool::new(false)) };
                if !c::bounded(&r.status(&s.root, &self.session, key), c::CONTROL_LIMIT-c::WRAPPER_RESERVE-c::PROGRESS_RESERVE) { return Err(c::error("responseTooLarge")); }
                s.queue.push_back(Work{request_key:key.into(),operation:r.operation.clone(),cancelled:r.cancel.clone()});
                s.receipts.insert(key.into(), r); self.shared.wake.notify_one();
            }
            let r = &s.receipts[key];
            return Ok(object!{version:1,root:s.root.clone(),catalogSession:self.session.clone(),requestKey:key,viewGeneration:r.generation.clone(),inputDigest:r.digest.clone(),phase:r.phase});
        }
        let root = s.root.clone();
        let r = s.receipts.get_mut(key).ok_or_else(|| c::error("catalogUnknownRequest"))?;
        match method {
            "native.catalog.status" => Ok(r.status(&root, &self.session, key)),
            "native.catalog.cancel" => {
                let state = if r.phase=="released" {"released"} else if r.terminal() {"alreadyTerminal"} else {r.cancel.store(true,Ordering::Release);self.shared.wake.notify_one();"requested"};
                Ok(object!{version:1,root:root,catalogSession:self.session.clone(),requestKey:key,state:state})
            },
            "native.catalog.release" => {
                if !r.terminal() { return Err(c::error("catalogRequestPending")); }
                r.phase="released"; r.operation=object!{kind:r.operation["kind"].clone(),args:JsonValue::Null}; r.result=JsonValue::Null; r.error=JsonValue::Null; r.progress=JsonValue::Null;
                Ok(object!{version:1,root:root,catalogSession:self.session.clone(),requestKey:key,state:"released"})
            },
            _ => Err(c::error("catalogUnavailable")),
        }
    }
}
impl Drop for CatalogOwner {
    fn drop(&mut self) { self.request_close(); if let Some(worker)=self.worker.take(){let _=worker.join();} }
}

/// Separate cache authority, never RootLock or the library/launch mutation gate.
/// It does not lock preferences: Task2 owns a short independent preference lock.
struct CacheLease { root:PathBuf, directory:File, lock:File, cache:PathBuf }
impl CacheLease {
    fn acquire(root:&Path) -> Result<Self,NativeError> {
        paths::private_dir(root)?; paths::reject_symlinks(root)?;
        let root=fs::canonicalize(root)?;
        let directory=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&root)?;
        let cache=root.join("cache/catalog"); paths::private_dir(&cache)?;
        let lock=OpenOptions::new().read(true).write(true).create(true).mode(0o600).custom_flags(0x20000|0x800).open(cache.join(".owner.lock"))?;
        if !lock.metadata()?.is_file(){return Err(c::error("catalogStorage"));}
        unsafe extern "C"{fn flock(fd:i32,op:i32)->i32;}
        if unsafe{flock(lock.as_raw_fd(),2|4)}!=0{return Err(c::error("catalogBusy"));}
        let lease=Self{root,directory,lock,cache};lease.validate()?;Ok(lease)
    }
    fn validate(&self)->Result<(),NativeError>{
        paths::reject_symlinks(&self.root)?;
        if fs::canonicalize(&self.root)?!=self.root{return Err(c::error("catalogStorage"));}
        let current=fs::metadata(&self.root)?;let held=self.directory.metadata()?;
        let named=paths::open_regular(&self.cache.join(".owner.lock"))?.metadata()?;let lock=self.lock.metadata()?;
        if (current.dev(),current.ino())!=(held.dev(),held.ino()) || (named.dev(),named.ino())!=(lock.dev(),lock.ino()){return Err(c::error("catalogStorage"));}Ok(())
    }
}
fn run(root:PathBuf, session:String, shared:Arc<Shared>) {
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||worker(root,&session,&shared)));
    let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());
    if shared.close.load(Ordering::Acquire){s.phase="closed";s.error=JsonValue::Null;}
    else {
        s.phase="failed";
        let message=match &result{Ok(Err(e))=>e.message.as_str(),Err(_)=>"Catalog worker stopped unexpectedly.",_=>"Catalog storage or ownership could not be retained."};
        s.error=c::failure("catalogStorage",message);
    }
    for r in s.receipts.values_mut().filter(|r|!r.terminal()) {r.phase=if shared.close.load(Ordering::Acquire){"cancelled"}else{"failed"};r.error=c::failure(if r.phase=="cancelled"{"catalogCancelled"}else{"catalogUnavailable"},"Catalog work stopped before completion.");}
    s.queue.clear();
    s.worker_exited=true;
    if s.phase=="closed"{s.receipts.clear();}
}
fn worker(root:PathBuf, session:&str, shared:&Shared)->Result<(),NativeError>{
    if shared.close.load(Ordering::Acquire){return Ok(());}
    let lease=CacheLease::acquire(&root)?;
    if shared.close.load(Ordering::Acquire){return Ok(());}
    if shared.launch_root.lock().unwrap_or_else(|p|p.into_inner()).as_ref().is_some_and(|p|p!=&lease.root){return Err(c::error("catalogRootBusy"));}
    let mut transport=MetadataTransport::new(lease.cache.join("spools"),session.to_owned())?;
    let mut executor=super::executor::NativeExecutor::open(&lease.root)?;
    lease.validate()?;
    {let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());if !shared.close.load(Ordering::Acquire){s.root=lease.root.to_str().ok_or_else(NativeError::params)?.into();s.phase="ready";}}
    while !shared.close.load(Ordering::Acquire){
        transport.tick();
        let work={let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());s.queue.pop_front()};
        let Some(work)=work else {lease.validate()?;executor.idle()?;let s=shared.state.lock().unwrap_or_else(|p|p.into_inner());let _=shared.wake.wait_timeout(s,Duration::from_millis(20));continue;};
        lease.validate()?;
        {let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());if let Some(r)=s.receipts.get_mut(&work.request_key){r.phase="running";}}
        let mut step=if work.cancelled.load(Ordering::Acquire){executor.cancel(&work,&mut transport);Step::Complete(Err(c::error("catalogCancelled")))}else{executor.step(&work,&mut transport)};
        // Producer access and validation happen before the shared receipt lock.
        let progress=executor.progress(&work.request_key);
        let progress=match progress {Some(value) if c::progress(&value).is_ok()=>Some(value),Some(_)=>{executor.cancel(&work,&mut transport);step=Step::Complete(Err(c::error("catalogUnavailable")));None},None=>None};
        let mut s=shared.state.lock().unwrap_or_else(|p|p.into_inner());
        if let Some(progress)=progress{if let Some(r)=s.receipts.get_mut(&work.request_key){r.progress=progress;}}
        match step{
            Step::Pending=>{s.queue.push_back(work);let _=shared.wake.wait_timeout(s,Duration::from_millis(20));},
            Step::Complete(result)=>{
                let root=s.root.clone();
                let r=s.receipts.get_mut(&work.request_key).unwrap();
                match result{
                    Ok(value)=>{
                        if c::exact(&value,&["identity","catalogRevision","provenance","data"]).is_ok(){r.phase="succeeded";r.result=value;}
                        else{r.phase="failed";r.error=c::failure("catalogUnavailable","Catalog producer returned invalid result evidence.");}
                    },
                    Err(e)=>{r.phase=if e.kind=="catalogCancelled"{"cancelled"}else{"failed"};r.error=c::failure(if c::TERMINAL_ERRORS.contains(&e.kind.as_str()){&e.kind}else if matches!(e.kind.as_str(),"nativeIo"|"nativeDatabase"){"catalogStorage"}else{"catalogTransport"},&e.message);if let Some(delay)=executor.retry_after(&work.request_key){r.error["retryAfterSeconds"]=delay.min(86400).into();}},
                }
                if !c::bounded(&r.status(&root,session,&work.request_key),c::TERMINAL_LIMIT-c::WRAPPER_RESERVE){r.phase="failed";r.result=JsonValue::Null;r.error=c::failure("catalogResponseTooLarge","Catalog result exceeded its complete response budget.");}
                // Eight receipts, each <= 1 MiB (including identity), enforce
                // the 8 MiB terminal storage ceiling without silent eviction.
                debug_assert!(c::RESULT_STORAGE == c::RECEIPTS*c::TERMINAL_LIMIT);
            }
        }
    }
    transport.close();
    Ok(())
}
