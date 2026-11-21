//! Bounded host observations. Only a fresh leader identity can authorize a pidfd signal.
use crate::{native::NativeError, paths};
use json::{object, JsonValue};
use std::{fs::{self, File}, io::{self, Read}, os::{fd::{AsRawFd, FromRawFd}, unix::ffi::OsStrExt}, path::Path, time::{Duration, Instant}};
// Per-attempt budget. A loaded desktop (shader compiles, downloads) can need
// well over 250 ms to read ~500 processes, which used to block Apply.
const SCAN_TIME: Duration = Duration::from_millis(2000);
const SCAN_ATTEMPTS: usize = 3;
const FRESH: Duration = Duration::from_secs(2);
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity { pid: u32, start: u64, uid: [u32;4] }
pub(crate) struct Scan { pub games: Vec<Identity>, pub complete: bool }
fn gone(e: &io::Error) -> bool { e.kind()==io::ErrorKind::NotFound || e.raw_os_error()==Some(3) }
fn bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes=Vec::new(); File::open(path)?.take((limit+1) as u64).read_to_end(&mut bytes)?;
    if bytes.len()>limit {return Err(io::Error::other("process bound"));} Ok(bytes)
}
pub(crate) fn game_name(bytes: &[u8]) -> bool {
    bytes.split(|b|*b==0).next().unwrap_or_default().rsplit(|b|*b==b'/' || *b==b'\\').next().is_some_and(|b|b.eq_ignore_ascii_case(b"deadlock.exe"))
}
fn identity_at(proc_root: &Path, pid: u32) -> io::Result<Option<Identity>> {
    let p=proc_root.join(pid.to_string());
    let stat=bounded(&p.join("stat"),4096)?;
    let stat=std::str::from_utf8(&stat).map_err(|_|io::Error::other("stat"))?;
    let end=stat.rfind(')').ok_or_else(||io::Error::other("stat"))?;
    if stat.split_whitespace().next()!=Some(&pid.to_string()) {return Err(io::Error::other("pid"));}
    let fields:Vec<_>=stat[end+1..].split_whitespace().collect();
    if fields.len()<20 {return Err(io::Error::other("stat"));}
    if matches!(fields[0],"Z"|"X"|"x") {return Ok(None);}
    let start=fields[19].parse().map_err(|_|io::Error::other("start"))?;
    let status=bounded(&p.join("status"),16384)?;
    let status=std::str::from_utf8(&status).map_err(|_|io::Error::other("status"))?;
    let tgid=status.lines().find_map(|l|l.strip_prefix("Tgid:")).and_then(|s|s.trim().parse::<u32>().ok()).ok_or_else(||io::Error::other("tgid"))?;
    if tgid!=pid {return Ok(None);}
    let ids:Vec<u32>=status.lines().find_map(|l|l.strip_prefix("Uid:")).ok_or_else(||io::Error::other("uid"))?.split_whitespace().map(str::parse).collect::<Result<_,_>>().map_err(|_|io::Error::other("uid"))?;
    let uid:[u32;4]=ids.try_into().map_err(|_|io::Error::other("uid"))?;
    Ok(Some(Identity{pid,start,uid}))
}
#[cfg(test)]
thread_local! { pub(crate) static TEST_GAME_NAME: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
fn observed_game_name(bytes: &[u8]) -> bool {
    #[cfg(test)]
    if let Some(name) = TEST_GAME_NAME.with(|v| v.borrow().clone()) {
        return bytes.split(|b| *b == 0).next().unwrap_or_default().rsplit(|b| *b == b'/' || *b == b'\\').next() == Some(name.as_bytes());
    }
    game_name(bytes)
}
fn is_game_at(proc_root: &Path, pid: u32) -> io::Result<bool> {
    let p=proc_root.join(pid.to_string());
    // Only argv[0] decides; long command lines (compilers, linkers) are truncated, not errors.
    let mut args=Vec::new(); File::open(p.join("cmdline"))?.take(16384).read_to_end(&mut args)?;
    let exe=fs::read_link(p.join("exe"));
    if observed_game_name(&args) {return Ok(true);}
    match exe {
        Ok(exe) if exe.as_os_str().as_bytes().len()<=4096=>{
            let bytes=exe.as_os_str().as_bytes();
            Ok(observed_game_name(bytes.strip_suffix(b" (deleted)").unwrap_or(bytes)))
        },
        // Kernel threads have neither an executable nor argv. A disappearing
        // process is handled by the caller's second identity read.
        Err(e) if gone(&e) && args.is_empty()=>Ok(false),
        // Other users' processes (systemd, root daemons) hide their executable
        // link; the game runs as this user, so its argv-0 check above suffices.
        Err(e) if e.kind()==io::ErrorKind::PermissionDenied=>Ok(false),
        // The process exited between reading argv and its exe link; the caller
        // skips vanished processes.
        Err(e) if gone(&e)=>Err(e),
        _=>Err(io::Error::other("executable unavailable")),
    }
}
pub(crate) fn scan() -> Scan {
    scan_at(Path::new("/proc"))
}
/// Retry an incomplete read-only scan before reporting the host unobservable.
pub(crate) fn scan_at(proc_root: &Path) -> Scan {
    let mut last = scan_once(proc_root);
    for _ in 1..SCAN_ATTEMPTS {
        if last.complete { break; }
        std::thread::sleep(Duration::from_millis(50));
        last = scan_once(proc_root);
    }
    last
}
fn scan_once(proc_root: &Path) -> Scan {
    let start=Instant::now(); let mut games=Vec::new(); let mut complete=true;
    let Ok(entries)=fs::read_dir(proc_root) else{return Scan{games,complete:false};};
    for (count,entry) in entries.enumerate() {
        if count>=65536 || start.elapsed()>SCAN_TIME {complete=false;break;}
        let Ok(entry)=entry else{complete=false;continue;};
        let name=entry.file_name();let Some(pid)=name.to_str().filter(|n|!n.is_empty() && n.bytes().all(|b|b.is_ascii_digit())).and_then(|n|n.parse::<u32>().ok()) else{continue;};
        let first=match identity_at(proc_root,pid) {Ok(Some(i))=>i,Ok(None)=>continue,Err(e) if gone(&e)=>continue,Err(_)=>{complete=false;continue;}};
        let found=match is_game_at(proc_root,pid) {Ok(v)=>v,Err(e) if gone(&e)=>continue,Err(_)=>{complete=false;continue;}};
        match identity_at(proc_root,pid) {
            Ok(Some(last)) if last==first=>{if found {if games.len()==16 {complete=false;break;}games.push(first);}},
            Ok(None)=>{},Err(e) if gone(&e)=>{},_=>complete=false,
        }
    }
    if start.elapsed()>SCAN_TIME {complete=false;}
    Scan{games,complete}
}
pub(crate) fn idle(game: &Path) -> Result<crate::launch_store::FreshIdle,NativeError> {
    let result=scan(); if !result.complete {return Err(unavailable());}
    if !result.games.is_empty(){return Err(NativeError::new("gameRunning","Close Deadlock before continuing."));}
    Ok(crate::launch_store::FreshIdle::from_complete_scan(game.to_owned()))
}
fn unavailable()->NativeError{NativeError::new("gameObservationUnavailable","A complete host process observation is unavailable.")}
unsafe extern "C" {fn geteuid()->u32;fn syscall(number:std::os::raw::c_long,...)->std::os::raw::c_long;fn poll(fds:*mut PollFd,nfds:usize,timeout:i32)->i32;}
#[repr(C)] struct PollFd {fd:i32,events:i16,revents:i16}
fn exited(fd:&File)->io::Result<bool>{let mut p=PollFd{fd:fd.as_raw_fd(),events:1,revents:0};let n=unsafe{poll(&mut p,1,0)};if n<0 || p.revents&0x28!=0 {Err(io::Error::last_os_error())}else{Ok(n>0 && p.revents&0x11!=0)}}
#[cfg(any(target_arch="x86_64",target_arch="aarch64"))]
fn pidfd(pid:u32)->io::Result<File>{let fd=unsafe{syscall(434,pid,0u32)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{File::from_raw_fd(fd as i32)})}}
#[cfg(not(any(target_arch="x86_64",target_arch="aarch64")))]
fn pidfd(_:u32)->io::Result<File>{Err(io::Error::other("pidfd unsupported"))}
fn signal(fd:&File)->io::Result<()>{if unsafe{syscall(424,fd.as_raw_fd(),15i32,std::ptr::null::<u8>(),0u32)}<0 {Err(io::Error::last_os_error())}else{Ok(())}}
struct Session {identity:Identity,token:String,seen:Instant}
struct Stop {token:String,fd:File,started:Instant}
pub(crate) struct Monitor {sessions:Vec<Session>,stop:Option<Stop>,last_stop:JsonValue}
impl Default for Monitor {fn default()->Self{Self{sessions:Vec::new(),stop:None,last_stop:JsonValue::Null}}}
impl Monitor {
    fn poll_stop(&mut self) {
        if let Some(stop)=&self.stop {
            let state=match exited(&stop.fd){Ok(true)=>Some("exited"),Err(_)=>Some("unavailable"),Ok(false) if stop.started.elapsed()>=Duration::from_secs(10)=>Some("stillRunning"),_=>None};
            if let Some(state)=state {self.last_stop=stop_value(&stop.token,state);self.stop=None;}
        }
    }
    pub fn status(&mut self)->Result<JsonValue,NativeError>{
        self.poll_stop();let scan=scan();let now=Instant::now();let mut next=Vec::new();let mut rows=JsonValue::new_array();
        for identity in scan.games {
            let token=if let Some(old)=self.sessions.iter().find(|s|s.identity==identity){old.token.clone()}else{paths::random_id()?};
            let same_user=identity.uid.iter().all(|u|*u==unsafe{geteuid()});
            let availability=if !scan.complete{"observationUnavailable"}else if !same_user{"differentUser"}else{match pidfd(identity.pid){Ok(_)=>"available",Err(e) if e.raw_os_error()==Some(38)=>"pidfdUnsupported",Err(_)=>"pidfdUnavailable"}};
            rows.push(object!{sessionToken:token.clone(),canStop:availability=="available",stopAvailability:availability}).map_err(|_|unavailable())?;
            next.push(Session{identity,token,seen:now});
        }
        self.sessions=next;
        let state=if !scan.complete {"unavailable"}else if rows.is_empty(){"idle"}else{"running"};
        Ok(object!{version:1,state:state,scanComplete:scan.complete,processes:rows,stop:self.last_stop.clone()})
    }
    pub fn stop(&mut self,token:&str)->Result<JsonValue,NativeError>{
        let result=self.stop_inner(token);if let Ok(value)=&result{self.last_stop=value.clone();}result
    }
    fn stop_inner(&mut self,token:&str)->Result<JsonValue,NativeError>{
        self.poll_stop();
        if let Some(stop)=&self.stop {return if stop.token==token{Ok(stop_value(token,"stopping"))}else{Err(NativeError::new("gameStopBusy","One game stop is already awaiting confirmation."))};}
        let session=self.sessions.iter().find(|s|s.token==token && s.seen.elapsed()<=FRESH).ok_or_else(||NativeError::new("staleGameSession","Refresh game status before stopping this process."))?;
        if !session.identity.uid.iter().all(|u|*u==unsafe{geteuid()}) {return Ok(stop_value(token,"unavailable"));}
        let fd=match pidfd(session.identity.pid){Ok(fd)=>fd,Err(e) if gone(&e)=>return Ok(stop_value(token,"alreadyExited")),Err(_)=>return Ok(stop_value(token,"unavailable"))};
        match identity_at(Path::new("/proc"),session.identity.pid) {
            Ok(Some(current)) if current==session.identity && is_game_at(Path::new("/proc"),current.pid).unwrap_or(false)=>{},
            Ok(None)=>return Ok(stop_value(token,"alreadyExited")),Err(e) if gone(&e)=>return Ok(stop_value(token,"alreadyExited")),_=>return Ok(stop_value(token,"unavailable")),
        }
        // The descriptor pins the process across the final fresh identity check.
        match exited(&fd){Ok(true)=>return Ok(stop_value(token,"alreadyExited")),Err(_)=>return Ok(stop_value(token,"unavailable")),Ok(false)=>{}}
        if signal(&fd).is_err(){return Ok(stop_value(token,if exited(&fd).unwrap_or(false){"alreadyExited"}else{"unavailable"}));}
        self.stop=Some(Stop{token:token.into(),fd,started:Instant::now()});self.last_stop=stop_value(token,"stopping");Ok(self.last_stop.clone())
    }
}
fn stop_value(token:&str,state:&str)->JsonValue{object!{version:1,sessionToken:token,status:state}}

#[cfg(test)] mod deleted_executable_tests {
    #[test] fn deleted_launcher_is_not_a_game_but_deleted_game_is() {
        let dir=std::env::temp_dir().join(format!("dmm-exe-{}",crate::paths::random_id().unwrap()));
        std::fs::create_dir_all(dir.join("123")).unwrap();std::fs::write(dir.join("123/cmdline"),b"launcher\0").unwrap();
        for (name,expected) in [("/app/T3.AppImage (deleted)",false),("/game/deadlock.exe (deleted)",true)] {
            let path=dir.join("123/exe");let _=std::fs::remove_file(&path);std::os::unix::fs::symlink(name,&path).unwrap();
            assert_eq!(super::is_game_at(&dir,123).unwrap(),expected);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
