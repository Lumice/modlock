//! Private, per-attempt transfer process. URLs and descriptors never cross a public RPC.
use super::{authority, contract::{self, ClaimSpawnFds, TransferEpoch}};
use crate::native::NativeError;
use json::JsonValue;
use sha2::{Digest, Sha256};
use std::{collections::{BTreeSet,VecDeque}, ffi::CString, fs::File, io::{self, Read, Write}, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::{ffi::OsStrExt, fs::MetadataExt}}, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicI8, Ordering}}, time::Duration};

const PIPE_BUDGET: usize = 32 * 1024 * 1024;
const QUEUE_BUDGET: usize = 64 * 1024;
const READ_QUANTUM: usize = 64 * 1024;
fn failure(kind: &str) -> NativeError { NativeError::new(kind, "The remote transfer could not be completed.") }
fn protocol() -> NativeError { failure("transportFraming") }

/// Validated frozen start frame. Values are kept private to the owner/child pipe.
pub(crate) struct TransferStartV1 { value: JsonValue, epoch: TransferEpoch }
impl TransferStartV1 {
    pub(crate) fn parse(value: JsonValue) -> Result<Self, NativeError> {
        let encoded = contract::encode_private_frame(&value)?;
        if value["kind"] != "start" { return Err(protocol()); }
        let epoch = TransferEpoch::parse(value["epoch"].as_str().ok_or_else(protocol)?)?;
        effective_uri(value["requestUrl"].as_str().ok_or_else(protocol)?)?;
        if !value["ifRange"].is_null() && !strong_etag(value["ifRange"].as_str().ok_or_else(protocol)?) { return Err(protocol()); }
        let _ = encoded;
        Ok(Self { value, epoch })
    }
    pub(crate) fn json(&self) -> &JsonValue { &self.value }
}
/// One accepted child event. The associated JSON is private, never a public DTO.
pub(crate) struct TransferEventV1 { value: JsonValue }
impl TransferEventV1 {
    pub(crate) fn json(&self) -> &JsonValue { &self.value }
    pub(crate) fn kind(&self) -> &str { self.value["kind"].as_str().unwrap_or("") }
}
/// A validated owner command, bound to the attempt again at send time.
pub(crate) struct TransferCommandV1 { value: JsonValue }
impl TransferCommandV1 {
    pub(crate) fn parse(value: JsonValue) -> Result<Self, NativeError> {
        contract::encode_private_frame(&value)?;
        if !matches!(value["kind"].as_str(), Some("allow" | "deny" | "checkpointAck" | "abort")) { return Err(protocol()); }
        Ok(Self { value })
    }
    pub(crate) fn json(&self) -> &JsonValue { &self.value }
}
fn strong_etag(s: &str) -> bool {
    let b = s.as_bytes(); b.len() >= 2 && b.len() <= 1024 && b[0] == b'"' && b[b.len()-1] == b'"' && b[1..b.len()-1].iter().all(|x| *x == 0x21 || (0x23..=0x7e).contains(x))
}
fn http_number(s:&str)->Result<u64,NativeError> {
    if s.is_empty()||!s.bytes().all(|b|b.is_ascii_digit()) {return Err(protocol());}
    s.parse().map_err(|_|protocol())
}
fn optional_number(value:&JsonValue)->Result<Option<u64>,NativeError> {
    if value.is_null() {Ok(None)} else {Ok(Some(value.as_str().ok_or_else(protocol)?.parse().map_err(|_|protocol())?))}
}
struct ResponseEvidence { total:Option<u64>, end:Option<u64>, etag:Option<String> }
fn response_evidence(data:&JsonValue,ranged:bool,offset:u64,cap:u64,if_range:Option<&str>)->Result<ResponseEvidence,NativeError> {
    if data["identityEstablished"]!=true {return Err(protocol());}
    let status=http_number(data["status"].as_str().ok_or_else(protocol)?)?;
    let mut seen=BTreeSet::new();let mut length=None;let mut transfer=false;let mut etag=None;let mut range=None;let mut location=false;
    for h in data["headers"].members() {
        let name=h["name"].as_str().ok_or_else(protocol)?.to_ascii_lowercase();let value=h["value"].as_str().ok_or_else(protocol)?.trim_matches([' ','\t']);
        if matches!(name.as_str(),"content-length"|"transfer-encoding"|"content-encoding"|"content-range"|"etag"|"location"|"content-type")&&!seen.insert(name.clone()){return Err(protocol());}
        match name.as_str() {
            "content-length"=>length=Some(http_number(value)?),
            "transfer-encoding"=>{if !value.eq_ignore_ascii_case("chunked"){return Err(protocol());}transfer=true;},
            "content-encoding"=>if !value.eq_ignore_ascii_case("identity"){return Err(protocol());},
            "etag"=>{if strong_etag(value){etag=Some(value.to_owned());}else if !value.strip_prefix("W/").is_some_and(strong_etag){return Err(protocol());}},
            "location"=>{if value.is_empty(){return Err(protocol());}location=true;},
            "content-type"=>if value.to_ascii_lowercase().starts_with("multipart/"){return Err(protocol());},
            "content-range"=>{
                let value=value.strip_prefix("bytes ").ok_or_else(protocol)?;let (bounds,total)=value.split_once('/').ok_or_else(protocol)?;
                let (start,end)=bounds.split_once('-').ok_or_else(protocol)?;let (start,end,total)=(http_number(start)?,http_number(end)?,http_number(total)?);
                if start!=offset||end<start||end>=total||total>cap{return Err(protocol());}range=Some((end,total));
            },_=>{},
        }
    }
    if transfer&&length.is_some(){return Err(protocol());}
    if matches!(status,301|302|303|307|308) {
        if ranged||!location{return Err(protocol());}return Ok(ResponseEvidence{total:None,end:None,etag});
    }
    if status==200 {
        if ranged||offset!=0||range.is_some()||length.is_some_and(|n|n>cap){return Err(protocol());}
        return Ok(ResponseEvidence{total:length,end:None,etag});
    }
    if status!=206||!ranged||etag.as_deref()!=if_range{return Err(protocol());}
    let (end,total)=range.ok_or_else(protocol)?;
    let segment=end.checked_sub(offset).and_then(|n|n.checked_add(1)).ok_or_else(protocol)?;
    if length.is_some_and(|n|n!=segment){return Err(protocol());}
    Ok(ResponseEvidence{total:Some(total),end:Some(end),etag})
}
fn uri_char(c: u8) -> bool { c.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?%".contains(&c) }
/// Exact absolute URI used by the HTTP request, with only an empty path expanded.
pub(crate) fn effective_uri(url: &str) -> Result<String, NativeError> {
    authority::parse_https_url(url)?;
    let rest = url.strip_prefix("https://").ok_or_else(protocol)?;
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let tail = &rest[split..];
    if !tail.bytes().all(uri_char) { return Err(protocol()); }
    let mut result = String::with_capacity(url.len()+1);
    result.push_str("https://"); result.push_str(&rest[..split]);
    if tail.is_empty() || tail.starts_with('?') { result.push('/'); }
    result.push_str(tail);
    if result.len() > 4096 { return Err(protocol()); }
    Ok(result)
}
pub(crate) fn effective_uri_sha256(url: &str) -> Result<String, NativeError> {
    Ok(format!("{:x}", Sha256::digest(effective_uri(url)?.as_bytes())))
}
fn has_dot_segments(path: &str) -> bool { path.split('/').any(|s| s == "." || s == "..") }
fn remove_dot_segments(path: &str) -> String {
    let mut input = path.to_owned(); let mut output = String::new();
    while !input.is_empty() {
        if input.starts_with("../") { input.drain(..3); }
        else if input.starts_with("./") { input.drain(..2); }
        else if input.starts_with("/./") { input.drain(..2); }
        else if input == "/." { input = "/".into(); }
        else if input.starts_with("/../") || input == "/.." {
            if input == "/.." { input = "/".into(); } else { input.drain(..3); }
            if let Some(pos) = output.rfind('/') { output.truncate(pos); } else { output.clear(); }
        } else if input == "." || input == ".." { input.clear(); }
        else {
            let end = if input.starts_with('/') { input[1..].find('/').map(|x|x+1).unwrap_or(input.len()) } else { input.find('/').unwrap_or(input.len()) };
            output.push_str(&input[..end]); input.drain(..end);
        }
    }
    output
}
/// Resolve a single Location without QUrl or percent decoding. Absolute signed
/// URLs with dot segments are rejected rather than silently reconstructed.
pub(crate) fn resolve_redirect(base: &str, location: &str) -> Result<String, NativeError> {
    let base = effective_uri(base)?;
    if location.len()>4096 || !location.is_ascii() || location.bytes().any(|b| b<=32 || b==127 || b==b'\\' || b==b'#') { return Err(protocol()); }
    if location.starts_with("https://") || location.starts_with("//") {
        let absolute = if location.starts_with("//") { format!("https:{location}") } else { location.to_owned() };
        let rest = absolute.strip_prefix("https://").ok_or_else(protocol)?;
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        if has_dot_segments(rest[split..].split('?').next().unwrap_or("")) { return Err(protocol()); }
        return effective_uri(&absolute);
    }
    if location.contains(':') && !location.starts_with('/') && !location.starts_with('?') && location.split('/').next().unwrap_or("").contains(':') { return Err(protocol()); }
    let rest = &base[8..]; let split = rest.find('/').ok_or_else(protocol)?;
    let origin = format!("https://{}", &rest[..split]);
    let current = &rest[split..]; let current_path = current.split('?').next().unwrap_or("/");
    let (path, query) = match location.split_once('?') { Some((p,q)) => (p,Some(q)), None => (location,None) };
    let combined = if path.is_empty() { current_path.to_owned() } else if path.starts_with('/') { remove_dot_segments(path) }
        else { let index = current_path.rfind('/').ok_or_else(protocol)?; remove_dot_segments(&format!("{}{}", &current_path[..=index], path)) };
    let mut target = format!("{origin}{combined}");
    if let Some(q)=query { target.push('?'); target.push_str(q); }
    else if path.is_empty() { if let Some((_,q))=current.split_once('?') { target.push('?'); target.push_str(q); } }
    effective_uri(&target)
}
fn duplicate_at_least_six(fd: i32) -> io::Result<OwnedFd> {
    unsafe extern "C" { fn fcntl(fd:i32, cmd:i32, ...) -> i32; }
    let n=unsafe { fcntl(fd, 1030, 6) }; // F_DUPFD_CLOEXEC
    if n<0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(n) }) }
}
fn pipe() -> io::Result<(File, File)> {
    unsafe extern "C" { fn pipe2(fds:*mut i32, flags:i32) -> i32; }
    let mut fds=[-1;2];
    if unsafe { pipe2(fds.as_mut_ptr(), 0x800 | 0x80000) }<0 { return Err(io::Error::last_os_error()); }
    Ok((unsafe { File::from_raw_fd(fds[0]) }, unsafe { File::from_raw_fd(fds[1]) }))
}
fn metadata(fd: &OwnedFd) -> Result<std::fs::Metadata, NativeError> {
    File::from(fd.try_clone().map_err(|_| failure("transportStorage"))?).metadata().map_err(|_| failure("transportStorage"))
}
fn flags(fd: &OwnedFd) -> Result<i32, NativeError> {
    unsafe extern "C" { fn fcntl(fd:i32, cmd:i32, ...) -> i32; }
    let value=unsafe { fcntl(fd.as_raw_fd(),3) }; if value<0 { Err(failure("transportStorage")) } else { Ok(value) }
}
fn check_descriptors(start: &TransferStartV1, fds: &ClaimSpawnFds) -> Result<(), NativeError> {
    unsafe extern "C" { fn geteuid() -> u32; fn flock(fd:i32,operation:i32)->i32; }
    let id=&fds.identity; let a=metadata(&fds.write_fd)?; let b=metadata(&fds.read_fd)?; let l=metadata(&fds.shared_lease)?;
    let uid=unsafe { geteuid() };
    if id.uid!=uid || id.link_count!=1 || !a.is_file() || !b.is_file() || !l.is_file()
        || (a.dev(),a.ino(),a.uid(),a.nlink())!=(id.data_device,id.data_inode,uid,1)
        || (b.dev(),b.ino(),b.uid(),b.nlink())!=(id.data_device,id.data_inode,uid,1)
        || (l.dev(),l.ino(),l.uid(),l.nlink())!=(id.lease_device,id.lease_inode,uid,1)
        || (a.dev(),a.ino())==(l.dev(),l.ino()) || a.mode()&0o077!=0 || l.mode()&0o077!=0
        || start.value["claimIdentity"]["device"].as_str()!=Some(&id.data_device.to_string())
        || start.value["claimIdentity"]["inode"].as_str()!=Some(&id.data_inode.to_string())
        || start.value["claimIdentity"]["uid"].as_str()!=Some(&id.uid.to_string())
        || a.len()!=start.value["offset"].as_str().and_then(|s|s.parse::<u64>().ok()).ok_or_else(protocol)?
        || flags(&fds.write_fd)? & 3 == 0 || flags(&fds.write_fd)? & 0x400 != 0 || flags(&fds.read_fd)? & 3 != 0
    { return Err(failure("claimAmbiguous")); }
    // The inherited open description keeps a shared lock throughout child life.
    if unsafe { flock(fds.shared_lease.as_raw_fd(), 1|4) } != 0 { return Err(failure("claimAmbiguous")); }
    Ok(())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Headers, Decision, Body, Checkpoint, Terminal }
struct OwnedProcess { pid: i32 }
impl OwnedProcess {
    fn try_wait(&mut self) -> Option<bool> {
        unsafe extern "C" { fn waitpid(pid:i32,status:*mut i32,options:i32)->i32; }
        let mut status=0; if unsafe { waitpid(self.pid,&mut status,1) }==self.pid {Some(status==0)} else {None}
    }
}
pub(crate) struct TransferChild {
    child: Option<OwnedProcess>, input: Option<File>, output: Option<File>, lease: Option<OwnedFd>,
    write_queue: VecDeque<u8>, read_queue: Vec<u8>, read_total: usize, write_total: usize,
    epoch: TransferEpoch, start: JsonValue, current: String, current_is_canonical_cache: bool, ranged: bool, phase: Phase,
    decision: Option<JsonValue>, checkpoint: Option<(String,String)>, failed: bool,
    retired: Arc<AtomicBool>,
    exit_status: Arc<AtomicI8>,
    response:Option<ResponseEvidence>,last_written:u64,last_ack:u64,redirects:u8,
}
impl TransferChild {
    pub(crate) fn accepts_abort(&self) -> bool { !self.failed && self.phase != Phase::Terminal }
    pub(crate) fn start(start: TransferStartV1, fds: ClaimSpawnFds) -> Result<Self, NativeError> {
        check_descriptors(&start,&fds)?;
        let executable=std::env::current_exe().map_err(|_|failure("transportNetwork"))?.with_file_name("modlock-network");
        let (child_stdin, parent_stdin)=pipe().map_err(|_|failure("transportNetwork"))?;
        let (parent_stdout, child_stdout)=pipe().map_err(|_|failure("transportNetwork"))?;
        let w=duplicate_at_least_six(fds.write_fd.as_raw_fd()).map_err(|_|failure("transportStorage"))?;
        let r=duplicate_at_least_six(fds.read_fd.as_raw_fd()).map_err(|_|failure("transportStorage"))?;
        let l=duplicate_at_least_six(fds.shared_lease.as_raw_fd()).map_err(|_|failure("transportStorage"))?;
        let input_source=duplicate_at_least_six(child_stdin.as_raw_fd()).map_err(|_|failure("transportNetwork"))?;
        let output_source=duplicate_at_least_six(child_stdout.as_raw_fd()).map_err(|_|failure("transportNetwork"))?;
        let null=File::options().write(true).open("/dev/null").map_err(|_|failure("transportNetwork"))?;
        let error_source=duplicate_at_least_six(null.as_raw_fd()).map_err(|_|failure("transportNetwork"))?;
        let (wf,rf,lf)=(w.as_raw_fd(),r.as_raw_fd(),l.as_raw_fd());
        let (inf,outf,errf)=(input_source.as_raw_fd(),output_source.as_raw_fd(),error_source.as_raw_fd());
        let initial=contract::encode_private_frame(&start.value)?;
        let current=effective_uri(start.value["requestUrl"].as_str().ok_or_else(protocol)?)?;
        let ranged=!start.value["uriSha256"].is_null() && effective_uri_sha256(&current)?==start.value["uriSha256"].as_str().unwrap_or("");
        let path=CString::new(executable.as_os_str().as_bytes()).map_err(|_|failure("transportNetwork"))?;
        let arg=CString::new("--transfer").map_err(|_|failure("transportNetwork"))?;
        let argv=[path.as_ptr(),arg.as_ptr(),std::ptr::null()];
        let mut queue=VecDeque::with_capacity(initial.len());queue.extend(initial.iter().copied());
        let initial_offset=http_number(start.value["offset"].as_str().ok_or_else(protocol)?)?;
        let retired=Arc::new(AtomicBool::new(false));let exit_status=Arc::new(AtomicI8::new(-1));
        unsafe extern "C" {
            fn fork()->i32; fn dup2(old:i32,new:i32)->i32;
            fn close_range(first:u32,last:u32,flags:i32)->i32;
            fn execv(path:*const std::ffi::c_char,argv:*const *const std::ffi::c_char)->i32;
            fn _exit(code:i32)->!;
        }
        // No std::process internal exec-error pipe can occupy 3/4/5. All
        // mapping sources are distinct held descriptors >=6. After fork the
        // child executes only async-signal-safe descriptor/exec operations.
        let pid=unsafe { fork() };
        if pid<0 { return Err(failure("transportNetwork")); }
        if pid==0 { unsafe {
            if dup2(inf,0)<0 || dup2(outf,1)<0 || dup2(errf,2)<0 || dup2(wf,3)<0 || dup2(rf,4)<0 || dup2(lf,5)<0 { _exit(127); }
            if close_range(6,u32::MAX,0)<0 { _exit(127); }
            execv(path.as_ptr(),argv.as_ptr()); _exit(127);
        } }
        drop(child_stdin); drop(child_stdout);
        let child=OwnedProcess { pid };
        // The PID is owned immediately, including child-side setup/exec failure.
        Ok(Self { child:Some(child), input:Some(parent_stdin), output:Some(parent_stdout), lease:Some(fds.shared_lease), write_queue:queue, read_queue:Vec::new(), read_total:0, write_total:initial.len(), epoch:start.epoch, start:start.value, current, current_is_canonical_cache:false, ranged, phase:Phase::Headers, decision:None, checkpoint:None, failed:false, retired, exit_status, response:None,last_written:initial_offset,last_ack:initial_offset,redirects:0 })
    }
    fn fail(&mut self) -> NativeError { self.failed=true; self.input.take(); self.write_queue.clear(); protocol() }
    fn pump_write(&mut self) -> Result<(), NativeError> {
        while !self.write_queue.is_empty() {
            let (a,b)=self.write_queue.as_slices(); let bytes=if !a.is_empty(){a}else{b};
            let Some(input)=self.input.as_mut() else { return Err(self.fail()); };
            match input.write(bytes) { Ok(0)=>return Err(self.fail()), Ok(n)=>{self.write_queue.drain(..n);}, Err(e) if e.kind()==io::ErrorKind::WouldBlock=>break, Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue, Err(_)=>return Err(self.fail()) }
        }
        Ok(())
    }
    fn accept_event(&mut self, value: JsonValue) -> Result<TransferEventV1, NativeError> {
        if value["epoch"].as_str()!=Some(self.epoch.as_str()) { return Err(self.fail()); }
        let kind=value["kind"].as_str().ok_or_else(protocol)?;
        match kind {
            "headers" if self.phase==Phase::Headers => {
                if value["data"]["url"].as_str()!=Some(self.current.as_str()) { return Err(self.fail()); }
                let offset=http_number(self.start["offset"].as_str().ok_or_else(protocol)?)?;
                let cap=http_number(self.start["byteCap"].as_str().ok_or_else(protocol)?)?;
                let evidence=response_evidence(&value["data"],self.ranged,offset,cap,self.start["ifRange"].as_str()).map_err(|_|self.fail())?;
                self.response=Some(evidence);
                self.decision=Some(value["data"].clone()); self.phase=Phase::Decision;
            },
            "progress" if self.phase==Phase::Body => {
                let data=&value["data"];let n=http_number(data["bytesWritten"].as_str().ok_or_else(protocol)?)?;
                let response=self.response.as_ref().ok_or_else(protocol)?;
                let cap=http_number(self.start["byteCap"].as_str().ok_or_else(protocol)?)?;
                if n<self.last_written||n>cap||response.total.is_some_and(|t|n>t)||optional_number(&data["total"])?!=response.total{return Err(self.fail());}
                self.last_written=n;
            },
            "checkpoint" if self.phase==Phase::Body => {
                let d=&value["data"];let n=http_number(d["offset"].as_str().ok_or_else(protocol)?)?;let response=self.response.as_ref().ok_or_else(protocol)?;
                let cap=http_number(self.start["byteCap"].as_str().ok_or_else(protocol)?)?;
                if n<self.last_written||n<=self.last_ack||n>cap||response.total.is_some_and(|t|n>t)||optional_number(&d["total"])?!=response.total
                    ||d["strongEtag"].as_str()!=response.etag.as_deref()||d["effectiveUriSha256"].as_str()!=Some(effective_uri_sha256(&self.current)?.as_str()) {return Err(self.fail());}
                self.last_written=n;self.checkpoint=Some((d["offset"].as_str().ok_or_else(protocol)?.into(),d["prefixSha256"].as_str().ok_or_else(protocol)?.into()));self.phase=Phase::Checkpoint;
            },
            "finished" if self.phase==Phase::Body => {
                let d=&value["data"];let n=http_number(d["bytesWritten"].as_str().ok_or_else(protocol)?)?;let response=self.response.as_ref().ok_or_else(protocol)?;
                let cap=http_number(self.start["byteCap"].as_str().ok_or_else(protocol)?)?;
                if d["networkError"]!="NoError"||n==0||n<self.last_written||n>cap||response.end.is_some_and(|end|n!=end+1)
                    ||(response.end.is_none()&&response.total.is_some_and(|total|n!=total))
                    ||response.end.zip(response.total).is_some_and(|(end,total)|end+1<total&&self.last_ack!=n) {return Err(self.fail());}
                self.last_written=n;self.phase=Phase::Terminal;
            },
            "error" if self.phase!=Phase::Terminal => { self.phase=Phase::Terminal; },
            _=>return Err(self.fail()),
        }
        Ok(TransferEventV1 { value })
    }
    pub(crate) fn poll(&mut self) -> Result<Vec<TransferEventV1>, NativeError> {
        if !self.failed { self.pump_write()?; }
        let mut budget=READ_QUANTUM; let mut eof=false;
        while budget>0 && self.read_queue.len()<QUEUE_BUDGET {
            let mut buf=[0u8;8192]; let amount=buf.len().min(budget).min(QUEUE_BUDGET-self.read_queue.len());
            let Some(output)=self.output.as_mut() else { break; };
            match output.read(&mut buf[..amount]) {
                Ok(0)=>{eof=true;break}, Ok(n)=>{self.read_total=self.read_total.checked_add(n).ok_or_else(protocol)?;if self.read_total>PIPE_BUDGET{return Err(self.fail());}self.read_queue.extend_from_slice(&buf[..n]);budget-=n;},
                Err(e) if e.kind()==io::ErrorKind::WouldBlock=>break, Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue, Err(_)=>return Err(self.fail()),
            }
        }
        let mut events=Vec::new();
        while events.len()<64 {
            let Some(end)=self.read_queue.iter().position(|b|*b==b'\n') else { if self.read_queue.len()>=contract::PRIVATE_LIMIT {return Err(self.fail());}break; };
            if end+1>contract::PRIVATE_LIMIT { return Err(self.fail()); }
            let frame:Vec<u8>=self.read_queue.drain(..=end).collect();
            let value=contract::decode_private_frame(&frame).map_err(|_|self.fail())?;
            events.push(self.accept_event(value)?);
        }
        if eof { self.output.take(); if !self.read_queue.is_empty() || self.phase!=Phase::Terminal { return Err(self.fail()); } }
        Ok(events)
    }
    pub(crate) fn send(&mut self, command: TransferCommandV1) -> Result<(), NativeError> {
        if self.failed || command.value["epoch"].as_str()!=Some(self.epoch.as_str()) { return Err(self.fail()); }
        let kind=command.value["kind"].as_str().ok_or_else(protocol)?; let data=&command.value["data"];
        let mut next=self.phase; let mut new_url:Option<(String,bool)>=None; let mut new_ranged=self.ranged;
        match kind {
            "abort" if self.phase!=Phase::Terminal => {next=Phase::Terminal;},
            "deny" if self.phase==Phase::Decision => {next=Phase::Terminal;},
            "checkpointAck" if self.phase==Phase::Checkpoint => {
                if self.checkpoint.as_ref().is_none_or(|(offset,hash)|data["offset"].as_str()!=Some(offset)||data["prefixSha256"].as_str()!=Some(hash)) { return Err(self.fail()); }
                next=Phase::Body;
            },
            "allow" if self.phase==Phase::Decision => {
                let decision=self.decision.as_ref().ok_or_else(protocol)?;
                let status=decision["status"].as_str().and_then(|x|x.parse::<u16>().ok()).ok_or_else(protocol)?;
                let url=data["url"].as_str().ok_or_else(protocol)?;
                if data["mode"]=="body" {
                    if url!=self.current || (self.ranged && status!=206) || (!self.ranged && status!=200) || decision["identityEstablished"]!=true { return Err(self.fail()); }
                    let Ok(host)=authority::parse_https_url(url) else { return Err(self.fail()); };
                    if self.start["deniedHosts"].members().any(|v|v.as_str()==Some(host.as_str())) { return Err(self.fail()); }
                    if !self.start["allowedHosts"].members().any(|v|v.as_str()==Some(host.as_str()))
                        && !(self.current_is_canonical_cache && authority::is_canonical_cache_host(&host)) { return Err(self.fail()); }
                    if self.ranged {
                        if data["rangeStart"]!=self.start["offset"] || data["ifRange"]!=self.start["ifRange"] {return Err(self.fail());}
                    } else if !data["rangeStart"].is_null() || !data["ifRange"].is_null() || self.start["offset"]!="0" {return Err(self.fail());}
                    next=Phase::Body;
                } else if data["mode"]=="redirect" {
                    if self.ranged || self.redirects>=3 || self.current_is_canonical_cache || !matches!(status,301|302|303|307|308) { return Err(self.fail()); }
                    let locations:Vec<&str>=decision["headers"].members().filter(|h|h["name"].as_str().is_some_and(|n|n.eq_ignore_ascii_case("location"))).filter_map(|h|h["value"].as_str()).collect();
                    if locations.len()!=1 { return Err(self.fail()); }
                    let resolved=resolve_redirect(&self.current,locations[0].trim_matches([' ','\t'])).map_err(|_|self.fail())?;
                    if url!=resolved || effective_uri(url).map_err(|_|self.fail())?!=resolved { return Err(self.fail()); }
                    let from_host=authority::parse_https_url(&self.current).map_err(|_|self.fail())?;
                    let to_host=authority::parse_https_url(&resolved).map_err(|_|self.fail())?;
                    let is_denied=|h:&str|self.start["deniedHosts"].members().any(|v|v.as_str()==Some(h));
                    let is_allowed=|h:&str|self.start["allowedHosts"].members().any(|v|v.as_str()==Some(h));
                    if is_denied(&from_host) || is_denied(&to_host) || !is_allowed(&from_host) { return Err(self.fail()); }
                    let next_is_canonical_cache = if is_allowed(&to_host) {
                        false
                    } else if authority::is_canonical_redirect_origin(&from_host) && authority::is_canonical_cache_host(&to_host) {
                        true
                    } else {
                        return Err(self.fail());
                    };
                    let range=!self.start["uriSha256"].is_null() && effective_uri_sha256(&resolved).map_err(|_|self.fail())?==self.start["uriSha256"].as_str().unwrap_or("");
                    if range {if data["rangeStart"]!=self.start["offset"]||data["ifRange"]!=self.start["ifRange"]{return Err(self.fail());}}
                    else if !data["rangeStart"].is_null()||!data["ifRange"].is_null(){return Err(self.fail());}
                    new_url=Some((resolved,next_is_canonical_cache)); new_ranged=range; next=Phase::Headers;
                } else {return Err(self.fail());}
            }, _=>return Err(self.fail()),
        }
        let bytes=contract::encode_private_frame(&command.value)?;
        if self.write_queue.len()+bytes.len()>QUEUE_BUDGET || self.write_total.checked_add(bytes.len()).is_none_or(|n|n>PIPE_BUDGET) {return Err(self.fail());}
        self.write_total+=bytes.len(); self.write_queue.extend(bytes); self.phase=next;
        if let Some((url,is_canonical))=new_url {self.current=url;self.current_is_canonical_cache=is_canonical;self.ranged=new_ranged;self.response=None;self.redirects+=1;}
        self.decision=None; if kind=="checkpointAck" {self.last_ack=self.last_written;self.checkpoint=None;}
        self.pump_write()
    }
    pub(crate) fn is_reaped(&mut self) -> bool {
        if let Some(child)=self.child.as_mut() {
            let Some(success)=child.try_wait() else {return false;};
            self.child.take(); self.lease.take(); self.exit_status.store(if success{1}else{0},Ordering::Release); self.retired.store(true,Ordering::Release);
        }
        true
    }
    pub(crate) fn retirement_ticket(&self) -> Arc<AtomicBool> { Arc::clone(&self.retired) }
    pub(crate) fn exit_success(&self) -> Option<bool> {match self.exit_status.load(Ordering::Acquire) {-1=>None,1=>Some(true),_=>Some(false)}}
}
struct Reaper { child:OwnedProcess, output:Option<File>, lease:Option<OwnedFd>, retired:Arc<AtomicBool>, exit_status:Arc<AtomicI8> }
impl Reaper { fn run(mut self) {
    loop {
        if let Some(output)=self.output.as_mut() { let mut b=[0;8192];for _ in 0..8 {match output.read(&mut b) {Ok(0)=>break, Ok(_)=>{}, Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue, Err(_)=>break}} }
        if let Some(success)=self.child.try_wait() {self.exit_status.store(if success{1}else{0},Ordering::Release);break;} std::thread::sleep(Duration::from_millis(50));
    }
    self.output.take(); self.lease.take(); self.retired.store(true,Ordering::Release);
} }
impl Drop for TransferChild { fn drop(&mut self) {
    self.input.take();
    if self.is_reaped() {return;}
    let Some(child)=self.child.take() else {return;};
    let work=Reaper {child,output:self.output.take(),lease:self.lease.take(),retired:Arc::clone(&self.retired),exit_status:Arc::clone(&self.exit_status)};
    let shared=Arc::new(Mutex::new(Some(work))); let thread=Arc::clone(&shared);
    if std::thread::Builder::new().name("remote-transfer-reaper".into()).spawn(move || {let work=thread.lock().unwrap_or_else(|e|e.into_inner()).take();if let Some(work)=work {work.run();}}).is_err() {
        let work=shared.lock().unwrap_or_else(|e|e.into_inner()).take();if let Some(work)=work {work.run();}
    }
} }

#[cfg(test)]
impl TransferChild {
    pub(crate) fn test_pipes_with_pid(start: TransferStartV1, pid: i32, input: File, output: File) -> Result<Self, NativeError> {
        let current = effective_uri(start.value["requestUrl"].as_str().ok_or_else(protocol)?)?;
        let initial_offset = http_number(start.value["offset"].as_str().ok_or_else(protocol)?)?;
        Ok(Self {
            child: Some(OwnedProcess { pid }),
            input: Some(input),
            output: Some(output),
            lease: None,
            write_queue: VecDeque::new(),
            read_queue: Vec::new(),
            read_total: 0,
            write_total: 0,
            epoch: start.epoch,
            start: start.value,
            current,
            current_is_canonical_cache: false,
            ranged: false,
            phase: Phase::Headers,
            decision: None,
            checkpoint: None,
            failed: false,
            retired: Arc::new(AtomicBool::new(false)),
            exit_status: Arc::new(AtomicI8::new(-1)),
            response: None,
            last_written: initial_offset,
            last_ack: initial_offset,
            redirects: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use json::object;

    fn nonblocking(file: &File) {
        unsafe extern "C" { fn fcntl(fd: i32, cmd: i32, ...) -> i32; }
        let flags = unsafe { fcntl(file.as_raw_fd(), 3) };
        assert!(flags >= 0);
        assert_eq!(unsafe { fcntl(file.as_raw_fd(), 4, flags | 0x800) }, 0);
    }

    fn exited_child(input_w: File, output_r: File, request_url: &str, allowed: Vec<&str>, denied: Vec<&str>) -> TransferChild {
        unsafe extern "C" { fn fork() -> i32; fn _exit(code: i32) -> !; }
        let pid = unsafe { fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe { _exit(0); }
        }
        nonblocking(&input_w);
        nonblocking(&output_r);
        let epoch = "a".repeat(32);
        let start = TransferStartV1::parse(object! {
            version: 1,
            kind: "start",
            epoch: epoch,
            requestUrl: request_url,
            allowedHosts: JsonValue::from(allowed.into_iter().map(str::to_owned).collect::<Vec<_>>()),
            deniedHosts: JsonValue::from(denied.into_iter().map(str::to_owned).collect::<Vec<_>>()),
            offset: "0",
            ifRange: JsonValue::Null,
            uriSha256: JsonValue::Null,
            byteCap: "1000000",
            claimIdentity: object! { device: "1", inode: "2", uid: "1000" },
        }).unwrap();
        TransferChild::test_pipes_with_pid(start, pid, input_w, output_r).unwrap()
    }

    fn write_frame(w: &mut File, value: &JsonValue) {
        let mut bytes = value.dump().into_bytes();
        bytes.push(b'\n');
        w.write_all(&bytes).unwrap();
    }

    #[test]
    fn canonical_cache_redirect_allows_exact_body_and_blocks_chain_or_denied() {
        let epoch = "a".repeat(32);
        let (stdin_r, stdin_w) = pipe().unwrap();
        let (stdout_r, mut stdout_w) = pipe().unwrap();
        let mut child = exited_child(
            stdin_w,
            stdout_r,
            "https://gamebanana.com/dl/1886611",
            vec!["gamebanana.com", "files.gamebanana.com"],
            vec!["filecache25.gamebanana.com"],
        );
        let _keep_r = stdin_r;

        // Hop 1: gamebanana.com -> files.gamebanana.com
        write_frame(&mut stdout_w, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://gamebanana.com/dl/1886611",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar" }],
            }
        });
        assert_eq!(child.poll().unwrap().len(), 1);
        assert!(child.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "redirect", url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).is_ok());

        // Hop 2: files.gamebanana.com -> unlisted filecache28.gamebanana.com
        write_frame(&mut stdout_w, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar" }],
            }
        });
        assert_eq!(child.poll().unwrap().len(), 1);
        assert!(child.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "redirect", url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).is_ok());
        assert!(child.current_is_canonical_cache);

        // Adversarial: chained redirect out of unlisted filecache28 (even relative) is rejected.
        write_frame(&mut stdout_w, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "/mods/other.rar" }],
            }
        });
        assert_eq!(child.poll().unwrap().len(), 1);
        assert!(child.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "redirect", url: "https://filecache28.gamebanana.com/mods/other.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).is_err());

        // Fresh child reaching filecache28 approves exact body transfer.
        let (stdin_r2, stdin_w2) = pipe().unwrap();
        let (stdout_r2, mut stdout_w2) = pipe().unwrap();
        let mut child2 = exited_child(
            stdin_w2,
            stdout_r2,
            "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
            vec!["gamebanana.com", "files.gamebanana.com"],
            vec!["filecache25.gamebanana.com"],
        );
        let _keep_r2 = stdin_r2;
        write_frame(&mut stdout_w2, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar" }],
            }
        });
        assert_eq!(child2.poll().unwrap().len(), 1);
        child2.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "redirect", url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).unwrap();
        write_frame(&mut stdout_w2, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "200",
                identityEstablished: true,
                headers: json::array![object! { name: "Content-Length", value: "100" }],
            }
        });
        assert_eq!(child2.poll().unwrap().len(), 1);
        // Mismatched path on body allow is rejected.
        assert!(child2.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "body", url: "https://filecache28.gamebanana.com/mods/other.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).is_err());

        // Valid exact body allow succeeds on fresh child.
        let (stdin_r3, stdin_w3) = pipe().unwrap();
        let (stdout_r3, mut stdout_w3) = pipe().unwrap();
        let mut child3 = exited_child(
            stdin_w3,
            stdout_r3,
            "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
            vec!["gamebanana.com", "files.gamebanana.com"],
            vec!["filecache25.gamebanana.com"],
        );
        let _keep_r3 = stdin_r3;
        write_frame(&mut stdout_w3, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar" }],
            }
        });
        child3.poll().unwrap();
        child3.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "redirect", url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).unwrap();
        write_frame(&mut stdout_w3, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "200",
                identityEstablished: true,
                headers: json::array![object! { name: "Content-Length", value: "100" }],
            }
        });
        child3.poll().unwrap();
        child3.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch.clone(),
            data: object! { mode: "body", url: "https://filecache28.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).unwrap();

        // Denied host (filecache25) is rejected even from files.gamebanana.com.
        let (stdin_r4, stdin_w4) = pipe().unwrap();
        let (stdout_r4, mut stdout_w4) = pipe().unwrap();
        let mut child4 = exited_child(
            stdin_w4,
            stdout_r4,
            "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
            vec!["gamebanana.com", "files.gamebanana.com"],
            vec!["filecache25.gamebanana.com"],
        );
        let _keep_r4 = stdin_r4;
        write_frame(&mut stdout_w4, &object! {
            version: 1, kind: "headers", epoch: epoch.clone(),
            data: object! {
                url: "https://files.gamebanana.com/mods/girlfailure_silver_v2.rar",
                status: "302",
                identityEstablished: true,
                headers: json::array![object! { name: "Location", value: "https://filecache25.gamebanana.com/mods/girlfailure_silver_v2.rar" }],
            }
        });
        child4.poll().unwrap();
        assert!(child4.send(TransferCommandV1::parse(object! {
            version: 1, kind: "allow", epoch: epoch,
            data: object! { mode: "redirect", url: "https://filecache25.gamebanana.com/mods/girlfailure_silver_v2.rar", rangeStart: JsonValue::Null, ifRange: JsonValue::Null }
        }).unwrap()).is_err());

        for mut c in [child, child2, child3, child4] {
            for _ in 0..50 {
                if c.is_reaped() { break; }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}


