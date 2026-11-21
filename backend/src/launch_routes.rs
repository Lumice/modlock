//! Source-backed host routes, discovered only for a live explicit commit.
//! Query-only Flatpak children are bounded; the selected handler is persistent.
use crate::{launch_record::validate_arguments, native::NativeError};
use json::{object,JsonValue};
use std::{env,fs,io::{Read,Write},os::{fd::{AsRawFd,FromRawFd},unix::{fs::PermissionsExt,net::UnixStream}},path::PathBuf,process::{Child,Command,Stdio},thread,time::{Duration,Instant}};
pub(crate) struct Route {program:PathBuf,args:Vec<String>,kind:&'static str,eligibility:&'static str}
impl Route {
    pub fn value(&self)->JsonValue{object!{kind:self.kind,eligibility:self.eligibility}}
    pub fn spawn(&self,raw:&str)->Result<Child,NativeError>{
        validate_arguments(raw)?;
        Command::new(&self.program).args(&self.args).arg(format!("steam://run/1422450//{raw}"))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
            .map_err(|_|NativeError::new("steamSpawnFailed","The selected Steam handler could not be started."))
    }
}
fn executable(name:&str)->Option<PathBuf>{
    let path=env::var_os("PATH")?;if path.len()>8192{return None;}
    for dir in env::split_paths(&path).take(32) {
        if !dir.is_absolute(){continue;}let p=dir.join(name);
        if fs::metadata(&p).ok().is_some_and(|m|m.is_file() && m.permissions().mode()&0o111!=0){return Some(p);}
    }None
}
struct QueryChild<'a> {child:Option<Child>,retained:&'a mut Option<Child>}
impl Drop for QueryChild<'_> {fn drop(&mut self){
    let Some(mut child)=self.child.take() else{return;};
    if child.try_wait().ok().flatten().is_some(){return;}
    let _=child.kill();
    // Never block cooperative retirement on an uninterruptible query. If the
    // kernel has not reaped it yet, transfer it into the owner's only child slot
    // and refuse all subsequent routes/dispatch for this attempt.
    if child.try_wait().ok().flatten().is_none(){*self.retained=Some(child);}
}}
fn installed_ref(program:&PathBuf,scope:&str,retained:&mut Option<Child>)->Option<Vec<String>> {
    let child=Command::new(program).args(["info",scope,"--show-ref","com.valvesoftware.Steam"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut child=QueryChild{child:Some(child),retained};let mut pipe=child.child.as_mut()?.stdout.take()?;
    unsafe extern "C" {fn fcntl(fd:i32,cmd:i32,...)->i32;}
    if unsafe{fcntl(pipe.as_raw_fd(),4,0x800)}<0{return None;}
    let deadline=Instant::now()+Duration::from_secs(2);let mut bytes=Vec::new();let mut exited=None;let mut eof=false;
    loop {
        let mut buf=[0u8;1025];
        match pipe.read(&mut buf){Ok(0)=>eof=true,Ok(n)=>{if bytes.len()+n>1024{return None;}bytes.extend_from_slice(&buf[..n]);},Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{},Err(_)=>return None}
        if exited.is_none(){exited=child.child.as_mut()?.try_wait().ok()?;}
        if eof && exited.is_some(){break;}
        if Instant::now()>=deadline{return None;}thread::sleep(Duration::from_millis(10));
    }
    if !exited?.success(){return None;}
    let value=std::str::from_utf8(&bytes).ok()?.strip_suffix('\n').unwrap_or(std::str::from_utf8(&bytes).ok()?);
    let pieces:Vec<_>=value.split('/').collect();
    if pieces.len()!=4 || pieces[0]!="app" || pieces[1]!="com.valvesoftware.Steam" || pieces[2..].iter().any(|s|s.is_empty() || s.len()>128 || !s.bytes().all(|b|b.is_ascii_alphanumeric() || b"._-".contains(&b))){return None;}
    Some(vec!["run".into(),scope.into(),format!("--arch={}",pieces[2]),format!("--branch={}",pieces[3]),pieces[1].into()])
}
fn snap_socket()->Option<UnixStream>{
    #[repr(C)] struct Address{family:u16,path:[u8;108]}
    #[repr(C)] struct Poll{fd:i32,events:i16,revents:i16}
    unsafe extern "C"{fn socket(domain:i32,kind:i32,protocol:i32)->i32;fn connect(fd:i32,address:*const Address,len:u32)->i32;fn poll(fds:*mut Poll,count:usize,timeout:i32)->i32;fn getsockopt(fd:i32,level:i32,name:i32,value:*mut i32,len:*mut u32)->i32;}
    let fd=unsafe{socket(1,1|0x800|0x80000,0)};if fd<0{return None;}let stream=unsafe{UnixStream::from_raw_fd(fd)};
    let mut address=Address{family:1,path:[0;108]};let path=b"/run/snapd.socket";address.path[..path.len()].copy_from_slice(path);
    if unsafe{connect(fd,&address,(2+path.len()+1) as u32)}<0 {
        if !matches!(std::io::Error::last_os_error().raw_os_error(),Some(115)|Some(11)){return None;}
        let mut p=Poll{fd,events:4,revents:0};if unsafe{poll(&mut p,1,500)}!=1 || p.revents&4==0{return None;}
        let mut error=0;let mut len=4;if unsafe{getsockopt(fd,1,4,&mut error,&mut len)}<0 || error!=0{return None;}
    }
    stream.set_nonblocking(false).ok()?;Some(stream)
}
fn snap_installed()->bool{snap_info().unwrap_or(false)}
fn snap_info()->Option<bool>{
    let mut socket=snap_socket()?;
    socket.set_read_timeout(Some(Duration::from_millis(500))).ok()?;socket.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    socket.write_all(b"GET /v2/snaps/steam HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
    let start=Instant::now();let mut bytes=Vec::new();let mut buf=[0u8;4096];
    loop {
        if start.elapsed()>Duration::from_secs(2){return None;}
        let n=socket.read(&mut buf).ok()?;if n==0{break;}if bytes.len()+n>32768{return None;}bytes.extend_from_slice(&buf[..n]);
    }
    let split=bytes.windows(4).position(|b|b==b"\r\n\r\n")?;
    if split>8192{return None;}let header=std::str::from_utf8(&bytes[..split]).ok()?;
    let mut lines=header.split("\r\n");if !matches!(lines.next()?,"HTTP/1.1 200 OK"|"HTTP/1.0 200 OK"){return None;}
    let mut length=None;
    for line in lines {let (key,value)=line.split_once(':')?;if key.eq_ignore_ascii_case("transfer-encoding"){return None;}if key.eq_ignore_ascii_case("content-length"){if length.is_some(){return None;}length=Some(value.trim().parse::<usize>().ok()?);}}
    let body=&bytes[split+4..];if length!=Some(body.len()){return None;}
    let v=json::parse(std::str::from_utf8(body).ok()?).ok()?;if !crate::protocol::unique_keys(body){return None;}let r=&v["result"];
    Some(v["type"]=="sync" && v["status-code"]==200 && r["name"]=="steam" && r["status"]=="active" && (!r.has_key("broken") || r["broken"]=="" || r["broken"]==false) && r["apps"].is_array() && r["apps"].members().any(|a|a["name"]=="steam"))
}
pub(crate) fn discover(retained:&mut Option<Child>)->Result<Route,NativeError>{
    if let Some(program)=executable("steam") {
        // /snap/bin/steam is an installed-package wrapper, not a direct route.
        if !program.starts_with("/snap") && !fs::canonicalize(&program).ok().is_some_and(|p|p.file_name().is_some_and(|n|n=="snap")) {
            return Ok(Route{program,args:vec![],kind:"direct",eligibility:"executable"});
        }
    }
    if let Some(program)=executable("flatpak") {for scope in ["--user","--system"] {
        if let Some(args)=installed_ref(&program,scope,retained){return Ok(Route{program,args,kind:if scope=="--user"{"flatpakUser"}else{"flatpakSystem"},eligibility:"installedRef"});}
        if retained.is_some(){return Err(NativeError::new("steamRouteQueryUncertain","The installed-route query has not exited; no handler was dispatched."));}
    }}
    if let Some(program)=executable("snap") {if snap_installed(){return Ok(Route{program,args:vec!["run".into(),"steam".into()],kind:"snap",eligibility:"installedPackage"});}}
    if let Some(program)=executable("xdg-open"){return Ok(Route{program,args:vec![],kind:"generic",eligibility:"handlerRequest"});}
    Err(NativeError::new("steamRouteUnavailable","No eligible host Steam route or generic URI handler is available."))
}
