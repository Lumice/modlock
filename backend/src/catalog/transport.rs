//! Worker-owned nonblocking child driver. No provider normalization occurs here.
use super::contract as c;
use crate::{native::NativeError, paths};
use json::{object,JsonValue};
use sha2::{Digest,Sha256};
use std::{collections::{BTreeMap,VecDeque},fs::{self,File,OpenOptions},io::{Read,Write},os::{fd::{AsRawFd,FromRawFd},unix::{fs::{MetadataExt,OpenOptionsExt,DirBuilderExt},process::CommandExt}},path::{Path,PathBuf},process::{Child,ChildStdin,ChildStdout,ChildStderr,Command,Stdio},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
pub type RequestId=String;
#[derive(Clone)]
pub struct MetadataRequest{pub request_id:RequestId,pub url:String}
#[derive(Clone,Debug)]
pub struct TransportError{pub kind:String,pub message:String}
impl TransportError{fn new(kind:&str)->Self{Self{kind:kind.into(),message:"Metadata transport could not complete the request.".into()}}}
pub struct MetadataCompletion{pub request_id:RequestId,pub http_status:Option<u16>,pub final_url:Option<String>,pub retry_after_seconds:Option<u32>,pub body:Option<MetadataBody>,pub error:Option<TransportError>}
/// A raw body lease prevents retaining a second body through poll().
pub struct MetadataBody { bytes:Vec<u8>, live:Arc<AtomicBool> }
impl std::ops::Deref for MetadataBody { type Target=[u8]; fn deref(&self)->&[u8]{&self.bytes} }
impl Drop for MetadataBody { fn drop(&mut self){self.live.store(false,Ordering::Release);} }
struct Entry{request:MetadataRequest,admitted:bool,cancel:bool,releasing:bool,discard:bool,terminal:Option<JsonValue>}
struct Rpc{id:i64,method:&'static str,key:Option<String>,started:Instant}
pub struct MetadataTransport{
    directory:PathBuf,directory_handle:File,namespace_lease:Option<File>,slot_limit:usize,child:Option<Child>,stdin:Option<ChildStdin>,stdout:Option<ChildStdout>,stderr:Option<ChildStderr>,
    input:Vec<u8>,output:VecDeque<u8>,stderr_tail:Vec<u8>,pending:Option<Rpc>,session:Option<String>,next_id:i64,
    entries:BTreeMap<String,Entry>,released:BTreeMap<String,String>,last_poll:Instant,poll_cursor:Option<String>,failure:Option<TransportError>,closing:bool,rotating:bool,restart_after:Option<Instant>,admitted:usize,body_live:Arc<AtomicBool>,
    // Present only for remote metadata. The child inherits this same open file
    // description, so parent death cannot release its accounting exclusion.
    remote_owner:Option<File>,remote_retired:Option<Arc<AtomicBool>>,
}
fn nonblocking(fd:i32)->std::io::Result<()>{unsafe extern "C"{fn fcntl(fd:i32,cmd:i32,...)->i32;}let flags=unsafe{fcntl(fd,3)};if flags<0 || unsafe{fcntl(fd,4,flags|0x800)}<0{return Err(std::io::Error::last_os_error());}Ok(())}
const SPOOL_LIMIT:u64=20*1024*1024;
const INVENTORY_LIMIT:u64=8192;
const INVENTORY_SLOT:usize=4096;
const INVENTORY_HEADER:usize=52;
const INVENTORY_MAGIC:&[u8;8]=b"DLCINV02";
// Two fixed records. Digest covers magic/sequence/length and the entire padded
// payload. A torn inactive write leaves the previous record independently valid.
fn inventory_slot(payload:&[u8],sequence:u64)->Option<[u8;INVENTORY_SLOT]>{
    if sequence==0||payload.is_empty()||payload.len()>INVENTORY_SLOT-INVENTORY_HEADER{return None;}
    let mut slot=[0u8;INVENTORY_SLOT];slot[..8].copy_from_slice(INVENTORY_MAGIC);
    slot[8..16].copy_from_slice(&sequence.to_be_bytes());slot[16..20].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    slot[INVENTORY_HEADER..INVENTORY_HEADER+payload.len()].copy_from_slice(payload);
    let mut digest=Sha256::new();digest.update(&slot[..20]);digest.update(&slot[INVENTORY_HEADER..]);
    slot[20..INVENTORY_HEADER].copy_from_slice(&digest.finalize());Some(slot)
}
fn inventory_record(bytes:&[u8])->Option<JsonValue>{
    if bytes.len()!=INVENTORY_LIMIT as usize{return None;}
    let mut latest:Option<(u64,JsonValue)>=None;
    for (index,slot) in bytes.chunks_exact(INVENTORY_SLOT).enumerate(){
        if &slot[..8]!=INVENTORY_MAGIC{continue;}
        let sequence=u64::from_be_bytes(slot[8..16].try_into().ok()?);
        let length=u32::from_be_bytes(slot[16..20].try_into().ok()?) as usize;
        if sequence==0||(sequence-1)%2!=index as u64||length==0||length>INVENTORY_SLOT-INVENTORY_HEADER{continue;}
        let mut digest=Sha256::new();digest.update(&slot[..20]);digest.update(&slot[INVENTORY_HEADER..]);
        if &digest.finalize()[..]!=&slot[20..INVENTORY_HEADER]||slot[INVENTORY_HEADER+length..].iter().any(|b|*b!=0){continue;}
        let payload=&slot[INVENTORY_HEADER..INVENTORY_HEADER+length];
        if !crate::protocol::unique_keys(payload)||!crate::protocol::integer_numbers(payload){continue;}
        let Some(value)=std::str::from_utf8(payload).ok().and_then(|s|json::parse(s).ok())else{continue;};
        if latest.as_ref().is_none_or(|(old,_)|sequence>*old){latest=Some((sequence,value));}
    }
    latest.map(|(_,value)|value)
}
const MAX_NAMESPACES:usize=64;
const MAX_SCAN_ENTRIES:usize=4096;
fn storage(reason:&str,path:&Path)->NativeError{
    let name=path.file_name().unwrap_or_default().to_string_lossy().chars().flat_map(char::escape_default).take(72).collect::<String>();
    NativeError::new("catalogStorage",format!("Catalog spool entry {name}: {reason}. Preserved for ownership reconciliation."))
}
fn lock(file:&File,operation:i32)->bool{unsafe extern "C"{fn flock(fd:i32,op:i32)->i32;}unsafe{flock(file.as_raw_fd(),operation)==0}}
fn owned_regular(m:&fs::Metadata,uid:u32)->bool{m.is_file()&&m.uid()==uid&&m.nlink()==1&&(m.mode()&0o077)==0}
fn open_at(directory:&File,name:&str,directory_only:bool)->std::io::Result<File>{
    unsafe extern "C"{fn openat(fd:i32,name:*const std::ffi::c_char,flags:i32,...)->i32;}
    let name=std::ffi::CString::new(name).map_err(|_|std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let fd=unsafe{openat(directory.as_raw_fd(),name.as_ptr(),0x20000|0x80000|0x800|if directory_only{0x10000}else{0})};
    if fd<0{Err(std::io::Error::last_os_error())}else{Ok(unsafe{File::from_raw_fd(fd)})}
}
fn remove_at(directory:&File,name:&str,expected:&fs::Metadata)->std::io::Result<bool>{
    let current=match open_at(directory,name,false){Ok(f)=>f.metadata()?,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(true),Err(e)=>return Err(e)};
    if (current.dev(),current.ino())!=(expected.dev(),expected.ino()){return Ok(false);}
    unsafe extern "C"{fn unlinkat(fd:i32,name:*const std::ffi::c_char,flags:i32)->i32;}
    let name=std::ffi::CString::new(name).unwrap();Ok(unsafe{unlinkat(directory.as_raw_fd(),name.as_ptr(),0)}==0)
}
/// Bounded worker-only sweep. A lease is shared by Rust and its child. Exclusive
/// nonblocking acquisition proves that the session lifetime has expired: neither
/// process can still write, regardless of PID reuse or wall-clock changes.
fn account_namespaces(parent:&Path)->Result<u64,NativeError>{
    let parent_handle=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(parent)?;
    let uid=parent_handle.metadata()?.uid();let mut total=0u64;let mut entries=0usize;let mut retained_namespaces=0usize;
    for (index,entry) in fs::read_dir(parent)?.enumerate(){
        if index>=MAX_NAMESPACES{return Err(storage("namespace scan exceeds 64 entries",parent));}
        let entry=entry?;let name=entry.file_name();let name=name.to_str().ok_or_else(||storage("non-UTF8 namespace cannot be classified",parent))?;
        let path=parent.join(name);let named=fs::symlink_metadata(&path)?;
        if named.is_file()&&named.uid()==uid&&named.nlink()==1{
            let current=open_at(&parent_handle,name,false)?.metadata()?;
            if (current.dev(),current.ino())!=(named.dev(),named.ino()){return Err(storage("unknown file identity changed during scan",&path));}
            // A regular unknown root entry is preserved and charged, not
            // mistaken for a broken namespace or assigned a fresh allowance.
            total=total.checked_add(current.len()).ok_or_else(||storage("byte accounting overflow",&path))?;
            retained_namespaces+=1;
            if total>SPOOL_LIMIT{return Err(storage("preserved files exceed the shared 20 MiB body allowance",&path));}
            continue;
        }
        if !named.is_dir()||named.uid()!=uid||(named.mode()&0o077)!=0{return Err(storage("unsafe namespace type, owner or permissions",&path));}
        let directory=open_at(&parent_handle,name,true)?;let held=directory.metadata()?;
        if (named.dev(),named.ino())!=(held.dev(),held.ino()){return Err(storage("namespace identity changed during scan",&path));}
        let lease=open_at(&directory,".lease",false).ok();
        let active=if let Some(file)=&lease{
            if !owned_regular(&file.metadata()?,uid){return Err(storage("namespace lease ownership is unsafe",&path));}
            !lock(file,2|4)
        }else{false};
        let manifest_file=open_at(&directory,".inventory",false).ok();
        let manifest=manifest_file.as_ref().and_then(|f|{
            let m=f.metadata().ok()?;if !owned_regular(&m,uid)||m.len()>INVENTORY_LIMIT{return None;}
            let mut bytes=Vec::new();f.take(INVENTORY_LIMIT+1).read_to_end(&mut bytes).ok()?;
            inventory_record(&bytes)
        });
        let evidence=manifest.as_ref().filter(|v|{
            let Some(l)=lease.as_ref() else{return false;};
            let fsid=if v.has_key("fsid") {
                if c::exact(v,&["version","session","device","inode","leaseDevice","leaseInode","slots","bodies","fsid"]).is_err(){return false;}
                let Some(id)=v["fsid"].as_str().filter(|s|crate::package::hex(s,16))else{return false;};Some(id)
            }else{
                if c::exact(v,&["version","session","device","inode","leaseDevice","leaseInode","slots","bodies"]).is_err(){return false;}None
            };
            let same=|file:&File,p:&Path,device:&JsonValue,inode:&JsonValue| {
                let (Ok(device),Ok(inode))=(c::u64_text(device),c::u64_text(inode))else{return false;};
                if active && fsid.is_none() && file.metadata().is_ok_and(|m|m.dev()!=device){return false;}
                paths::same_persisted_object(&path,&directory,p,file,device,inode,fsid).unwrap_or(false)
            };
            v["version"]==1&&v["session"]==name&&c::token(&v["session"]).is_ok()
                &&same(&directory,&path,&v["device"],&v["inode"])
                &&same(l,&path.join(".lease"),&v["leaseDevice"],&v["leaseInode"])
                &&v["slots"].as_u32().is_some_and(|n|(1..=4).contains(&n))&&v["bodies"].is_array()&&v["bodies"].len()<=4
        });
        if manifest_file.is_some()&&evidence.is_none(){return Err(storage("inventory or lease identity is invalid/replaced",&path));}
        if active&&evidence.is_none(){return Err(storage("active namespace has no trustworthy slot allowance",&path));}
        let reserved=if active{evidence.unwrap()["slots"].as_u64().unwrap()*c::BODY_LIMIT as u64}else{0};
        let mut bodies=BTreeMap::new();
        if let Some(v)=evidence{
            for body in v["bodies"].members(){
                if c::exact(body,&["token","device","inode"]).is_err(){return Err(storage("inventory body evidence is invalid",&path));}
                let token=c::token(&body["token"]).map_err(|_|storage("inventory token is invalid",&path))?;
                let dev=c::u64_text(&body["device"]).map_err(|_|storage("inventory device is invalid",&path))?;
                let ino=c::u64_text(&body["inode"]).map_err(|_|storage("inventory inode is invalid",&path))?;
                if bodies.insert(token.to_owned(),(dev,ino)).is_some(){return Err(storage("duplicate inventory body token",&path));}
            }
        }
        let mut remaining=0usize;let mut bytes=0u64;
        for entry in fs::read_dir(&path)?{
            entries+=1;if entries>MAX_SCAN_ENTRIES{return Err(storage("file scan exceeds 4096 entries",&path));}
            let entry=entry?;let filename=entry.file_name();let filename=filename.to_str().ok_or_else(||storage("non-UTF8 file cannot be classified",&path))?;
            let file=open_at(&directory,filename,false).map_err(|_|storage("unknown non-regular entry is preserved",&path))?;
            let meta=file.metadata()?;
            if !meta.is_file()||meta.uid()!=uid||meta.nlink()!=1{return Err(storage("unknown file ownership prevents byte accounting",&path));}
            let mapped=bodies.get(filename).is_some_and(|(device,inode)|{
                let fsid=evidence.and_then(|v|v["fsid"].as_str());
                if active&&fsid.is_none()&&*device!=meta.dev(){return false;}
                paths::same_persisted_object(&path,&directory,&entry.path(),&file,*device,*inode,fsid).unwrap_or(false)
            });
            if !active&&evidence.is_some()&&mapped&&owned_regular(&meta,uid)&&meta.len()<=c::BODY_LIMIT as u64&&remove_at(&directory,filename,&meta)?{continue;}
            // Known bounded bookkeeping is separate from the body allowance.
            // Unknown/replaced/incomplete manifests and temporary files count.
            let bookkeeping=match filename{
                ".lease"=>lease.as_ref().and_then(|f|f.metadata().ok()),
                ".inventory"=>manifest_file.as_ref().and_then(|f|f.metadata().ok()),
                _=>None,
            };
            if evidence.is_some()&&owned_regular(&meta,uid)&&meta.len()<=INVENTORY_LIMIT
                &&bookkeeping.is_some_and(|m|(m.dev(),m.ino())==(meta.dev(),meta.ino())){continue;}
            if active&&mapped&&owned_regular(&meta,uid)&&meta.len()<=c::BODY_LIMIT as u64{continue;}
            bytes=bytes.checked_add(meta.len()).ok_or_else(||storage("byte accounting overflow",&path))?;remaining+=1;
        }
        if !active&&evidence.is_some()&&remaining==0{
            let manifest_meta=manifest_file.as_ref().unwrap().metadata()?;let lease_meta=lease.as_ref().unwrap().metadata()?;
            if remove_at(&directory,".inventory",&manifest_meta)?&&remove_at(&directory,".lease",&lease_meta)?{
                directory.sync_all()?;
                // Remove only the empty namespace still naming this directory.
                let now=fs::symlink_metadata(&path)?;
                if (now.dev(),now.ino())==(held.dev(),held.ino()){let _=fs::remove_dir(&path);}
            }
        }
        match fs::symlink_metadata(&path){
            Ok(_)=>retained_namespaces+=1,
            Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(e.into()),
        }
        total=total.checked_add(bytes.checked_add(reserved).ok_or_else(||storage("byte accounting overflow",&path))?).ok_or_else(||storage("byte accounting overflow",&path))?;
        if total>SPOOL_LIMIT{return Err(storage("preserved files exceed the shared 20 MiB body allowance",&path));}
    }
    if retained_namespaces>=MAX_NAMESPACES{return Err(storage("64 preserved namespaces leave no new namespace capacity",parent));}
    parent_handle.sync_all()?;Ok(total)
}
impl MetadataTransport{
    pub fn new(parent:PathBuf,session:String)->Result<Self,NativeError>{
        paths::private_dir(&parent)?;
        let existing=account_namespaces(&parent)?;
        let slots=(SPOOL_LIMIT.saturating_sub(existing)/c::BODY_LIMIT as u64).min(4);
        if slots==0{return Err(storage(&format!("{existing} retained/reserved bytes leave no 5 MiB slot in the 20 MiB budget"), &parent));}
        let directory=parent.join(&session);
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let directory_handle=OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(&directory)?;
        let lease=OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(directory.join(".lease"))?;
        if !lock(&lease,1|4){return Err(storage("cannot hold the namespace lease",&directory));}
        let dir_meta=directory_handle.metadata()?;let lease_meta=lease.metadata()?;
        let inventory=object!{version:1,session:session,device:dir_meta.dev().to_string(),inode:dir_meta.ino().to_string(),leaseDevice:lease_meta.dev().to_string(),leaseInode:lease_meta.ino().to_string(),fsid:paths::persisted_filesystem_id(&directory_handle)?,slots:slots as u32,bodies:json::array![]};
        let first=inventory_slot(inventory.dump().as_bytes(),1).ok_or_else(||storage("initial inventory record exceeds its fixed slot",&directory))?;
        let mut bytes=[0u8;INVENTORY_LIMIT as usize];bytes[..INVENTORY_SLOT].copy_from_slice(&first);
        // Exclusive creation and retained-handle writes never replace a name.
        let mut manifest=OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(directory.join(".inventory"))?;
        manifest.write_all(&bytes)?;manifest.sync_all()?;lease.sync_all()?;directory_handle.sync_all()?;paths::sync_dir(&parent)?;
        Ok(Self{directory,directory_handle,namespace_lease:Some(lease),slot_limit:slots as usize,child:None,stdin:None,stdout:None,stderr:None,input:Vec::new(),output:VecDeque::new(),stderr_tail:Vec::new(),pending:None,session:None,next_id:0,entries:BTreeMap::new(),released:BTreeMap::new(),last_poll:Instant::now()-Duration::from_secs(1),poll_cursor:None,failure:None,closing:false,rotating:false,restart_after:None,admitted:0,body_live:Arc::new(AtomicBool::new(false)),remote_owner:None,remote_retired:None})
    }
    pub(crate) fn new_remote(parent:PathBuf,session:String,owner:File)->Result<Self,NativeError>{
        let mut transport=Self::new(parent,session)?;
        transport.remote_owner=Some(owner);
        transport.remote_retired=Some(Arc::new(AtomicBool::new(false)));
        Ok(transport)
    }
    pub(crate) fn remote_retirement_ticket(&self)->Option<Arc<AtomicBool>>{self.remote_retired.clone()}
    fn start(&mut self)->Result<(),TransportError>{
        if self.remote_owner.is_some(){return self.start_remote();}
        let executable=std::env::current_exe().map_err(|_|TransportError::new("transportProtocol"))?.with_file_name("modlock-network");
        let mut child=Command::new(executable).arg("--spool-dir").arg(&self.directory).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_|TransportError::new("transportNetwork"))?;
        let input=child.stdin.take().unwrap();let output=child.stdout.take().unwrap();let stderr=child.stderr.take().unwrap();
        if [input.as_raw_fd(),output.as_raw_fd(),stderr.as_raw_fd()].into_iter().any(|fd|nonblocking(fd).is_err()){let _=child.kill();let _=child.wait();return Err(TransportError::new("transportProtocol"));}
        self.child=Some(child);self.stdin=Some(input);self.stdout=Some(output);self.stderr=Some(stderr);
        self.send("hello",None,object!{version:1})?;Ok(())
    }
    fn start_remote(&mut self)->Result<(),TransportError>{
        let executable=std::env::current_exe().map_err(|_|TransportError::new("transportProtocol"))?.with_file_name("modlock-network");
        let owner_fd=self.remote_owner.as_ref().ok_or_else(||TransportError::new("transportProtocol"))?.as_raw_fd();
        let mut command=Command::new(executable);
        command.arg("--spool-dir").arg(&self.directory).arg("--metadata-owner-fd").arg("3").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Only async-signal-safe descriptor operations run between fork and exec.
        unsafe { command.pre_exec(move || {
            unsafe extern "C" { fn dup2(old:i32,new:i32)->i32; fn fcntl(fd:i32,cmd:i32,...)->i32; }
            if dup2(owner_fd,3)<0 {return Err(std::io::Error::last_os_error());}
            let flags=fcntl(3,1);
            if flags<0||fcntl(3,2,flags & !1)<0 {return Err(std::io::Error::last_os_error());}
            Ok(())
        }); }
        let mut child=command.spawn().map_err(|_|TransportError::new("transportNetwork"))?;
        self.stdin=child.stdin.take();self.stdout=child.stdout.take();self.stderr=child.stderr.take();
        // Track the child before any subsequent setup can fail.
        self.child=Some(child);
        let input_ok=self.stdin.as_ref().is_some_and(|p|nonblocking(p.as_raw_fd()).is_ok());
        let output_ok=self.stdout.as_ref().is_some_and(|p|nonblocking(p.as_raw_fd()).is_ok());
        let stderr_ok=self.stderr.as_ref().is_some_and(|p|nonblocking(p.as_raw_fd()).is_ok());
        // Only configured nonblocking readers can safely join retirement. Try
        // each independently so a failure in one never discards another.
        if !output_ok{self.stdout.take();}if !stderr_ok{self.stderr.take();}
        if !input_ok||!output_ok||!stderr_ok{self.stdin.take();return Err(TransportError::new("transportProtocol"));}
        self.send("hello",None,object!{version:1})
    }
    pub fn submit(&mut self,request:MetadataRequest)->Result<(),TransportError>{
        if self.closing{return Err(TransportError::new("catalogCancelled"));}
        c::token(&JsonValue::from(request.request_id.as_str())).map_err(|_|TransportError::new("transportProtocol"))?;
        if !metadata_url(&request.url){return Err(TransportError::new("transportProtocol"));}
        if let Some(e)=self.entries.get(&request.request_id){return if e.request.url==request.url{Ok(())}else{Err(TransportError::new("transportProtocol"))};}
        if self.released.contains_key(&request.request_id){return Err(TransportError::new("catalogKeyConflict"));}
        if let Some(e)=&self.failure{return Err(e.clone());}
        if self.entries.len()>=self.slot_limit+8 || self.admitted+self.entries.values().filter(|e|!e.admitted).count()>=4096 || self.rotating{return Err(TransportError::new("catalogBusy"));}
        self.entries.insert(request.request_id.clone(),Entry{request,admitted:false,cancel:false,releasing:false,discard:false,terminal:None});Ok(())
    }
    pub fn cancel(&mut self,id:RequestId){if let Some(e)=self.entries.get_mut(&id){e.cancel=true;}}
    /// Cancel a domain-owned read and release its eventual body without loading
    /// raw bytes. Used when a catalog operation itself has been cancelled.
    pub fn discard(&mut self,id:RequestId){if let Some(e)=self.entries.get_mut(&id){e.cancel=true;e.discard=true;}}
    fn send(&mut self,method:&'static str,key:Option<String>,params:JsonValue)->Result<(),TransportError>{
        if self.pending.is_some(){return Err(TransportError::new("transportProtocol"));}
        self.next_id=self.next_id.checked_add(1).ok_or_else(||TransportError::new("transportProtocol"))?;
        let mut frame=object!{jsonrpc:"2.0",id:self.next_id,method:method,params:params}.dump().into_bytes();frame.push(b'\n');
        if frame.len()>c::FRAME_LIMIT || self.output.len()+frame.len()>c::PIPE_LIMIT{return Err(TransportError::new("transportProtocol"));}
        self.output.extend(frame);self.pending=Some(Rpc{id:self.next_id,method,key,started:Instant::now()});Ok(())
    }
    fn fail(&mut self,error:TransportError){
        if self.failure.is_none(){self.failure=Some(error);}
        self.pending=None;self.output.clear();self.input.clear();self.stdin.take();
        if self.remote_owner.is_some(){self.reap_remote();return;}
        // Only the owned metadata child can be terminated, never main helper.
        if let Some(mut child)=self.child.take(){let _=child.kill();let _=child.wait();}
        self.stdout.take();self.stderr.take();
    }
    pub fn tick(&mut self){if let Err(e)=self.tick_inner(){self.fail(e);}}
    fn tick_inner(&mut self)->Result<(),TransportError>{
        if self.remote_owner.is_some()&&(self.failure.is_some()||self.closing){self.reap_remote();return Ok(());}
        if self.failure.is_some(){return Ok(());}
        if self.child.is_none(){if self.entries.is_empty()||self.closing||self.restart_after.is_some_and(|t|Instant::now()<t){return Ok(());}self.restart_after=None;self.start()?;}
        if let Some(stderr)=self.stderr.as_mut(){let mut b=[0;4096];for _ in 0..4{match stderr.read(&mut b){Ok(0)=>break,Ok(n)=>{self.stderr_tail.extend_from_slice(&b[..n]);if self.stderr_tail.len()>c::STDERR_LIMIT{self.stderr_tail.drain(..self.stderr_tail.len()-c::STDERR_LIMIT);}},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>break,Err(_)=>break}}}
        if let Some(input)=self.stdin.as_mut(){for _ in 0..4{let (bytes,_)=self.output.as_slices();if bytes.is_empty(){break;}match input.write(bytes){Ok(0)=>return Err(TransportError::new("transportNetwork")),Ok(n)=>{self.output.drain(..n);},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>break,Err(_)=>return Err(TransportError::new("transportNetwork"))}}}
        if let Some(output)=self.stdout.as_mut(){let mut b=[0;8192];for _ in 0..8{match output.read(&mut b){Ok(0)=>{if !self.closing&&!self.rotating{return Err(TransportError::new("transportNetwork"));}break;},Ok(n)=>{if self.input.len()+n>c::PIPE_LIMIT{return Err(TransportError::new("transportProtocol"));}self.input.extend_from_slice(&b[..n]);},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>break,Err(_)=>return Err(TransportError::new("transportNetwork"))}}}
        if let Some(end)=self.input.iter().position(|b|*b==b'\n'){
            if end+1>c::CONTROL_LIMIT{return Err(TransportError::new("transportProtocol"));}
            let frame=self.input.drain(..=end).collect::<Vec<_>>();
            if !crate::protocol::unique_keys(&frame)||!crate::protocol::integer_numbers(&frame){return Err(TransportError::new("transportProtocol"));}
            let value=json::parse(std::str::from_utf8(&frame).map_err(|_|TransportError::new("transportProtocol"))?).map_err(|_|TransportError::new("transportProtocol"))?;
            let rpc=self.pending.take().ok_or_else(||TransportError::new("transportProtocol"))?;
            c::exact(&value,if value.has_key("result"){&["jsonrpc","id","result"]}else{&["jsonrpc","id","error"]}).map_err(|_|TransportError::new("transportProtocol"))?;
            if value["jsonrpc"]!="2.0"||value["id"]!=rpc.id{return Err(TransportError::new("transportProtocol"));}
            if value.has_key("error"){return Err(TransportError::new("transportProtocol"));}
            self.response(rpc,&value["result"])?;
        }else if self.input.len()>=c::CONTROL_LIMIT{return Err(TransportError::new("transportProtocol"));}
        if self.pending.as_ref().is_some_and(|p|p.started.elapsed()>Duration::from_secs(10)){return Err(TransportError::new("transportTimeout"));}
        if self.child.as_mut().is_some_and(|ch|ch.try_wait().ok().flatten().is_some()) {
            if self.rotating {
                self.child.take();self.stdin.take();self.stdout.take();self.stderr.take();self.session=None;
                self.pending=None;self.input.clear();self.output.clear();self.admitted=0;self.released.clear();self.rotating=false;self.restart_after=Some(Instant::now()+Duration::from_secs(1));
                return Ok(());
            }
            if !self.closing{return Err(TransportError::new("transportNetwork"));}
        }
        if self.pending.is_none()&&!self.closing&&!self.rotating{
            if self.admitted>=4096&&self.entries.is_empty(){
                if let Some(session)=self.session.clone(){self.rotating=true;self.send("close",None,object!{version:1,transportSession:session})?;}
            }else{self.schedule()?;}
        }
        Ok(())
    }
    fn response(&mut self,rpc:Rpc,v:&JsonValue)->Result<(),TransportError>{
        let invalid=||TransportError::new("transportProtocol");
        if v["version"]!=1{return Err(invalid());}
        if rpc.method=="hello"{
            c::exact(v,&["version","transportSession","capabilities","phase"]).map_err(|_|invalid())?;
            if v["phase"]!="ready"||v["capabilities"]!=json::array!["metadataGetV1"]{return Err(invalid());}
            self.session=Some(c::token(&v["transportSession"]).map_err(|_|invalid())?.into());return Ok(());
        }
        if v["transportSession"].as_str()!=self.session.as_deref(){return Err(invalid());}
        if rpc.method=="close"{c::exact(v,&["version","transportSession","phase"]).map_err(|_|invalid())?;if v["phase"]!="closing"{return Err(invalid());}return Ok(());}
        let key=rpc.key.ok_or_else(invalid)?;
        if v["requestKey"]!=key{return Err(invalid());}
        match rpc.method{
            "get"=>{c::exact(v,&["version","transportSession","requestKey","phase"]).map_err(|_|invalid())?;if !["queued","active","succeeded","failed","cancelled"].contains(&v["phase"].as_str().unwrap_or("")){return Err(invalid());}self.entries.get_mut(&key).ok_or_else(invalid)?.admitted=true;self.admitted+=1;},
            "cancel"=>{c::exact(v,&["version","transportSession","requestKey","state"]).map_err(|_|invalid())?;if !["requested","alreadyTerminal","released"].contains(&v["state"].as_str().unwrap_or("")){return Err(invalid());}self.entries.get_mut(&key).ok_or_else(invalid)?.cancel=false;},
            "release"=>{c::exact(v,&["version","transportSession","requestKey","state"]).map_err(|_|invalid())?;if v["state"]!="released"{return Err(invalid());}if let Some(e)=self.entries.remove(&key){self.released.insert(key.clone(),c::digest(&JsonValue::from(e.request.url)));}},
            "status"=>{validate_status(v)?;if !matches!(v["phase"].as_str(),Some("queued"|"active")){self.entries.get_mut(&key).ok_or_else(invalid)?.terminal=Some(v.clone());}},
            _=>return Err(invalid()),
        }Ok(())
    }
    fn schedule(&mut self)->Result<(),TransportError>{
        let Some(session)=self.session.clone()else{return Ok(());};
        self.entries.retain(|_,e|!(e.discard&&e.terminal.as_ref().is_some_and(|v|!v.has_key("version"))));
        for e in self.entries.values_mut(){if e.discard&&e.terminal.is_some(){e.releasing=true;}}
        let candidate=self.entries.iter().find(|(_,e)|e.releasing).map(|(k,_)|(k.clone(),"release"))
            .or_else(||self.entries.iter().find(|(_,e)|e.cancel&&e.admitted&&e.terminal.is_none()).map(|(k,_)|(k.clone(),"cancel")))
            .or_else(||self.entries.iter().find(|(_,e)|!e.admitted&&e.terminal.is_none()).map(|(k,_)|(k.clone(),"get")));
        if let Some((key,method))=candidate{
            let e=&self.entries[&key];
            if method=="get"&&e.cancel{self.entries.get_mut(&key).unwrap().terminal=Some(object!{phase:"cancelled",error:c::failure("catalogCancelled","Metadata request cancelled.")});self.entries.get_mut(&key).unwrap().admitted=true;return Ok(());}
            let mut params=object!{version:1,transportSession:session,requestKey:key.clone()};
            if method=="get"{params["url"]=e.request.url.clone().into();params["purpose"]="metadata".into();}
            return self.send(method,Some(key),params);
        }
        if self.last_poll.elapsed()>=Duration::from_millis(100){
            let mut keys=self.entries.iter().filter(|(_,e)|e.terminal.is_none()&&!e.releasing).map(|(k,_)|k.clone()).collect::<Vec<_>>();
            if let Some(cursor)=&self.poll_cursor{let split=keys.partition_point(|k|k<=cursor);keys.rotate_left(split);}
            if let Some(key)=keys.first(){self.last_poll=Instant::now();self.poll_cursor=Some(key.clone());return self.send("status",Some(key.clone()),object!{version:1,transportSession:session,requestKey:key.clone()});}
        }
        Ok(())
    }
    /// Consume at most one raw body; caller drops it after normalization before
    /// polling again. Child release is scheduled immediately after consumption.
    /// Domain jobs consume only their owned completion. Other ready bodies stay
    /// in the same bounded spool until their worker step rotates into service.
    pub fn poll_for(&mut self,request_id:&str)->Option<MetadataCompletion>{
        self.tick();
        if self.body_live.load(Ordering::Acquire){return None;}
        let entry=self.entries.get(request_id)?;
        if entry.releasing||entry.discard||entry.terminal.is_none()&&self.failure.is_none(){return None;}
        let key=request_id.to_owned();
        let e=self.entries.get_mut(&key).unwrap();
        let v=e.terminal.take().unwrap_or(JsonValue::Null);e.releasing=true;
        let mut completion=MetadataCompletion{request_id:key.clone(),http_status:v["httpStatus"].as_u16(),final_url:v["finalUrl"].as_str().map(str::to_owned),retry_after_seconds:v["retryAfterSeconds"].as_u32(),body:None,error:self.failure.clone()};
        if completion.error.is_none(){
            if v["phase"]=="succeeded"{match self.read_body(&v){Ok(b)=>{self.body_live.store(true,Ordering::Release);completion.body=Some(MetadataBody{bytes:b,live:self.body_live.clone()});},Err(e)=>completion.error=Some(e)}}
            else{completion.error=Some(TransportError{kind:v["error"]["kind"].as_str().unwrap_or("transportProtocol").into(),message:"Metadata request did not produce a body.".into()});}
        }
        if self.failure.is_some()||!v.has_key("version"){self.entries.remove(&key);}
        Some(completion)
    }
    fn read_body(&self,v:&JsonValue)->Result<Vec<u8>,TransportError>{
        let failure=||TransportError::new("transportStorage");
        let token=c::token(&v["bodyToken"]).map_err(|_|failure())?;let size=c::u64_text(&v["bodyBytes"]).map_err(|_|failure())?;
        if size>c::BODY_LIMIT as u64{return Err(failure());}
        let current=fs::symlink_metadata(&self.directory).map_err(|_|failure())?;let held=self.directory_handle.metadata().map_err(|_|failure())?;
        if !current.is_dir()||(current.dev(),current.ino())!=(held.dev(),held.ino()){return Err(failure());}
        let path=self.directory.join(token);let mut f=paths::open_regular(&path).map_err(|_|failure())?;
        let before=f.metadata().map_err(|_|failure())?;let named=fs::symlink_metadata(&path).map_err(|_|failure())?;
        if before.len()!=size||before.nlink()!=1||(before.dev(),before.ino())!=(named.dev(),named.ino())||before.uid()!=held.uid(){return Err(failure());}
        let mut body=Vec::with_capacity(size as usize);(&mut f).take(size+1).read_to_end(&mut body).map_err(|_|failure())?;
        let after=f.metadata().map_err(|_|failure())?;
        if body.len()!=size as usize||after.len()!=size||(before.mtime(),before.mtime_nsec())!=(after.mtime(),after.mtime_nsec()){return Err(failure());}Ok(body)
    }
    pub fn close(&mut self){
        if self.remote_owner.is_some(){self.begin_remote_close();return;}
        if self.closing{return;}self.closing=true;
        // Outstanding RPC is drained first. EOF is an equivalent cleanup request.
        let deadline=Instant::now()+Duration::from_secs(2);
        while self.child.is_some()&&Instant::now()<deadline{
            self.tick();
            if self.pending.is_none()&&self.stdin.is_some()&&!self.rotating{
                if let Some(session)=self.session.clone(){let _=self.send("close",None,object!{version:1,transportSession:session});}
                else{self.stdin.take();}
            }
            if self.pending.as_ref().is_some_and(|p|p.method=="close")&&self.output.is_empty(){self.stdin.take();}
            if self.child.as_mut().is_some_and(|ch|ch.try_wait().ok().flatten().is_some()){break;}
            std::thread::sleep(Duration::from_millis(10));
        }
        self.failure=Some(TransportError::new("catalogCancelled"));
        self.fail(TransportError::new("catalogCancelled"));
        // Both parent and child leases must be gone before inventory cleanup.
        self.namespace_lease.take();
        if let Some(parent)=self.directory.parent(){let _=account_namespaces(parent);}
    }
    pub(crate) fn begin_remote_close(&mut self){
        if self.closing{return;}
        self.closing=true;
        if self.failure.is_none(){self.failure=Some(TransportError::new("catalogCancelled"));}
        self.pending=None;self.output.clear();self.input.clear();
        // Qt treats EOF as a cooperative abort and closes active replies.
        self.stdin.take();
        self.reap_remote();
    }
    fn reap_remote(&mut self){
        for pipe in [&mut self.stdout as &mut dyn DrainPipe,&mut self.stderr as &mut dyn DrainPipe]{pipe.drain_available();}
        if self.child.as_mut().is_some_and(|c|matches!(c.try_wait(),Ok(Some(_)))){
            self.child.take();self.stdout.take();self.stderr.take();
        }
    }
    pub(crate) fn remote_is_quiescent(&mut self)->bool{
        if !self.closing{return false;}
        self.reap_remote();
        if self.child.is_some(){return false;}
        self.namespace_lease.take();
        if let Some(parent)=self.directory.parent(){let _=account_namespaces(parent);}
        self.remote_owner.take();
        if let Some(retired)=self.remote_retired.as_ref(){retired.store(true,Ordering::Release);}
        true
    }
}
trait DrainPipe{fn drain_available(&mut self);}
impl<T:Read> DrainPipe for Option<T>{fn drain_available(&mut self){if let Some(pipe)=self.as_mut(){let mut bytes=[0;8192];for _ in 0..8{match pipe.read(&mut bytes){Ok(0)=>break,Ok(_)=>{},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>break,Err(_)=>break}}}}}
struct RemoteReaper{child:Child,stdout:Option<ChildStdout>,stderr:Option<ChildStderr>,namespace:Option<File>,owner:Option<File>,parent:Option<PathBuf>,retired:Option<Arc<AtomicBool>>}
impl RemoteReaper{fn run(mut self){
    // Bound each drain so neither pipe starves the other or the exit check.
    // A failed wait is not proof of exit. Keep both locks until a real reap.
    loop{
        for pipe in [&mut self.stdout as &mut dyn DrainPipe,&mut self.stderr as &mut dyn DrainPipe]{pipe.drain_available();}
        match self.child.try_wait(){Ok(Some(_))=>break,_=>std::thread::sleep(Duration::from_millis(50))}
    }
    self.namespace.take();
    if let Some(parent)=self.parent.as_ref(){let _=account_namespaces(parent);}
    self.owner.take();
    if let Some(retired)=self.retired.as_ref(){retired.store(true,Ordering::Release);}
}}
impl Drop for MetadataTransport{fn drop(&mut self){
    if self.remote_owner.is_none(){self.close();return;}
    self.begin_remote_close();
    if self.remote_is_quiescent(){return;}
    self.stdin.take();
    let Some(child)=self.child.take()else{return;};
    let job=RemoteReaper{child,stdout:self.stdout.take(),stderr:self.stderr.take(),namespace:self.namespace_lease.take(),owner:self.remote_owner.take(),parent:self.directory.parent().map(Path::to_path_buf),retired:self.remote_retired.clone()};
    // Keep an independent owner for the fallback if thread creation fails.
    let shared=Arc::new(Mutex::new(Some(job)));let worker=Arc::clone(&shared);
    if std::thread::Builder::new().name("remote-metadata-reaper".into()).spawn(move||{
        let job=worker.lock().unwrap_or_else(|e|e.into_inner()).take();if let Some(job)=job{job.run();}
    }).is_err(){let job=shared.lock().unwrap_or_else(|e|e.into_inner()).take();if let Some(job)=job{job.run();}}
}}
pub fn metadata_url(url:&str)->bool{
    if url.len()>7000||url.bytes().any(|b|b<=32||b==127)||url.contains(['\\','#']){return false;}
    ["https://gamebanana.com/","https://api.gamebanana.com/"].iter().any(|prefix|url.starts_with(prefix))
}
fn validate_status(v:&JsonValue)->Result<(),TransportError>{
    let invalid=||TransportError::new("transportProtocol");
    c::exact(v,&["version","transportSession","requestKey","phase","httpStatus","finalUrl","retryAfterSeconds","bodyToken","bodyBytes","error"]).map_err(|_|invalid())?;
    let phase=v["phase"].as_str().ok_or_else(invalid)?;
    if matches!(phase,"queued"|"active"|"released"){
        if ["httpStatus","finalUrl","retryAfterSeconds","bodyToken","bodyBytes","error"].iter().any(|k|!v[*k].is_null()){return Err(invalid());}return Ok(());
    }
    if !matches!(phase,"succeeded"|"failed"|"cancelled"){return Err(invalid());}
    if !v["httpStatus"].is_null()&&!v["httpStatus"].as_u16().is_some_and(|n|(100..=599).contains(&n)){return Err(invalid());}
    if !v["finalUrl"].is_null()&&!v["finalUrl"].as_str().is_some_and(metadata_url){return Err(invalid());}
    if !v["retryAfterSeconds"].is_null()&&!v["retryAfterSeconds"].as_u32().is_some_and(|n|n<=86400){return Err(invalid());}
    if phase=="succeeded"{
        c::token(&v["bodyToken"]).map_err(|_|invalid())?;
        if c::u64_text(&v["bodyBytes"]).map_err(|_|invalid())?>c::BODY_LIMIT as u64||v["httpStatus"].is_null()||v["finalUrl"].is_null()||!v["error"].is_null(){return Err(invalid());}
    }else{
        if !v["bodyToken"].is_null()||!v["bodyBytes"].is_null(){return Err(invalid());}
        c::exact(&v["error"],&["kind","message","retryAfterSeconds"]).map_err(|_|invalid())?;
        if !v["error"]["kind"].as_str().is_some_and(|k|c::TRANSPORT_ERRORS.contains(&k))||!v["error"]["message"].as_str().is_some_and(|m|m.len()<=256)||(!v["error"]["retryAfterSeconds"].is_null()&&!v["error"]["retryAfterSeconds"].as_u32().is_some_and(|n|n<=86400)){return Err(invalid());}
        if phase=="cancelled"&&v["error"]["kind"]!="catalogCancelled"{return Err(invalid());}
    }Ok(())
}

#[cfg(test)]
#[test]
fn item93_catalog_spool_recovery_survives_device_change() {
    use std::os::unix::fs::PermissionsExt;
    let f=crate::test_support::Fixture::new();let parent=f.root.join("spools");fs::create_dir(&parent).unwrap();
    if !paths::test_volatile_filesystem(&File::open(&parent).unwrap()){return;}
    let token="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";let name="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";let dir=parent.join(name);
    fs::create_dir(&dir).unwrap();fs::set_permissions(&dir,fs::Permissions::from_mode(0o700)).unwrap();
    for (file,body) in [(".lease",b"".as_slice()),(token,b"body".as_slice())] {let p=dir.join(file);fs::write(&p,body).unwrap();fs::set_permissions(p,fs::Permissions::from_mode(0o600)).unwrap();}
    let dm=fs::metadata(&dir).unwrap();let lm=fs::metadata(dir.join(".lease")).unwrap();let bm=fs::metadata(dir.join(token)).unwrap();
    let mut inventory=object!{version:1,session:name,device:(dm.dev()+1).to_string(),inode:dm.ino().to_string(),leaseDevice:(lm.dev()+1).to_string(),leaseInode:lm.ino().to_string(),slots:1,bodies:json::array![object!{token:token,device:(bm.dev()+1).to_string(),inode:bm.ino().to_string()}]};
    let path=dir.join(".inventory");
    let write_inventory=|value:&JsonValue|{let slot=inventory_slot(value.dump().as_bytes(),1).unwrap();let mut bytes=[0u8;INVENTORY_LIMIT as usize];bytes[..INVENTORY_SLOT].copy_from_slice(&slot);fs::write(&path,bytes).unwrap();fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();};
    write_inventory(&inventory);
    let active=File::open(dir.join(".lease")).unwrap();assert!(lock(&active,1|4));
    assert!(account_namespaces(&parent).is_err(),"an active legacy spool cannot use the reboot fallback");
    assert!(dir.exists());drop(active);
    inventory["fsid"]="0000000000000001".into();write_inventory(&inventory);
    assert!(account_namespaces(&parent).is_err(),"a different filesystem ID is never adopted");
    assert!(dir.exists());inventory.remove("fsid");
    inventory["inode"]=(dm.ino()+1).to_string().into();write_inventory(&inventory);
    assert!(account_namespaces(&parent).is_err(),"a different directory inode is never adopted");
    assert!(dir.exists());inventory["inode"]=dm.ino().to_string().into();write_inventory(&inventory);
    assert!(account_namespaces(&parent).is_ok(),"an inactive owned spool should survive a btrfs device change");
    assert!(!dir.exists(),"the exact owned spool should be retired");
}
