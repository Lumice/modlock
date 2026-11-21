//! Linux-only, bounded decoder process ownership. No persisted PID is authority.
use crate::{native::NativeError, paths, prepare_control::PrepareControl};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    os::{fd::AsRawFd, raw::{c_int, c_long, c_ulong}, unix::{fs::MetadataExt, process::CommandExt}},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) const BUFFER: usize = 64 * 1024;
const WALL: Duration = Duration::from_secs(600);
const DIAGNOSTICS: u64 = 64 * 1024;

#[cfg(test)]
#[path = "../tests/archive_boundary/process.rs"]
mod tests;

#[repr(C)]
struct RLimit { current: c_ulong, maximum: c_ulong }
unsafe extern "C" {
    fn setpgid(pid: c_int, pgid: c_int) -> c_int;
    fn prctl(option: c_int, ...) -> c_int;
    fn getpid() -> c_int;
    fn getppid() -> c_int;
    fn _exit(status: c_int) -> !;
    fn setrlimit(resource: c_int, limit: *const RLimit) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn fgetxattr(fd: c_int, name: *const u8, value: *mut u8, size: usize) -> isize;
}

pub(crate) fn limit(message: &str) -> NativeError {
    NativeError::new("archiveLimitExceeded", message)
}

#[derive(Default)]
pub(crate) struct ProcessBudget {
    elapsed: Duration,
    diagnostics: u64,
}
impl ProcessBudget {
    fn check(&self, start: Instant, control: &PrepareControl) -> Result<(), NativeError> {
        control.check()?;
        if self.elapsed.saturating_add(start.elapsed()) >= WALL {
            return Err(limit("Archive decoder cumulative wall admission limit exceeded."));
        }
        Ok(())
    }
}

/// The worker owns this child until wait/reap. A reaped PID is never signalled.
struct OwnedChild { child: Child, reaped: bool }
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.reaped {
            // pre_exec established exactly this child's process group.
            unsafe { kill(-(self.child.id() as i32), 9); }
            // A failed wait is not proof of exit. Keep the owning worker and all
            // source/quarantine/slot guards until the actual child is reaped.
            while !self.reaped {
                match self.child.wait() {
                    Ok(_) => self.reaped = true,
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
    }
}

fn nonblocking(fd: c_int) -> Result<(), NativeError> {
    let flags = unsafe { fcntl(fd, 3) };
    if flags < 0 || unsafe { fcntl(fd, 4, flags | 0x800) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

// Preserve the strict system-decoder contract while making missing tools actionable.
fn open_decoder(path: &Path) -> Result<File, NativeError> {
    if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Err(NativeError::new("archiveDecoderMissing",
            "Archive imports need lsar and unar. Install your distribution's unar package, then try again. You can also extract the archive yourself and add its VPK files or folder."));
    }
    paths::open_regular(path)
}

fn decoder(name: &str) -> Result<Command, NativeError> {
    let path = match name {
        "lsar" => Path::new("/usr/bin/lsar"),
        "unar" => Path::new("/usr/bin/unar"),
        "sha256sum" => Path::new("/usr/bin/sha256sum"),
        _ => return Err(NativeError::params()),
    };
    let executable = open_decoder(path)?;
    for ancestor in path.ancestors().skip(1) {
        let metadata = std::fs::metadata(ancestor)?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(NativeError::new("archiveContractUnsupported", "Decoder executable ancestors must be root-owned and not writable by other users."));
        }
    }
    let meta = executable.metadata()?;
    // Privileged or caller-writable decoders would invalidate PDEATHSIG policy.
    let capabilities = unsafe {
        fgetxattr(executable.as_raw_fd(), b"security.capability\0".as_ptr(), std::ptr::null_mut(), 0)
    };
    if meta.uid() != 0 || meta.mode() & 0o6022 != 0 || meta.mode() & 0o111 == 0
        || capabilities >= 0
        || !matches!(std::io::Error::last_os_error().raw_os_error(), Some(61) | Some(95))
    {
        return Err(NativeError::new("archiveContractUnsupported", "Decoder must be an ordinary root-owned, unprivileged executable."));
    }
    let parent = unsafe { getpid() };
    let mut command = Command::new(path);
    command.env_clear().env("LC_ALL", "C.UTF-8").env("PATH", "/usr/bin:/bin");
    // Only async-signal-safe libc operations; no allocation, locks or Rust I/O.
    // PDEATHSIG concerns the spawning thread and direct decoder, not descendants.
    unsafe {
        command.pre_exec(move || {
            if setpgid(0, 0) != 0 || prctl(1, 9 as c_ulong, 0 as c_long, 0 as c_long, 0 as c_long) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if getppid() != parent { _exit(127); }
            for (resource, bound) in [(0, 60), (9, 1024 * 1024 * 1024), (4, 0)] {
                let r = RLimit { current: bound, maximum: bound };
                if setrlimit(resource, &r) != 0 { return Err(std::io::Error::last_os_error()); }
            }
            Ok(())
        });
    }
    Ok(command)
}

/// Pipes are drained together by one nonblocking loop, so no reader thread can
/// outlive the child. Filesystem writes/wait may block: WALL is admission policy,
/// not a promise of hard cancellation latency. Errors reap before returning.
pub(crate) fn run(
    name: &str, arguments: &[OsString], output: &mut impl Write, max_output: u64,
    budget: &mut ProcessBudget, control: &PrepareControl,
) -> Result<u64, NativeError> {
    run_with_input(name, arguments, output, max_output, None, budget, control)
}

/// Feed only admitted bytes, rather than handing a growing source descriptor to
/// a hashing child. A single extra byte checks EOF without entering the pipe.
pub(crate) fn run_with_input(
    name: &str, arguments: &[OsString], output: &mut impl Write, max_output: u64,
    mut input: Option<(&mut File, u64)>, budget: &mut ProcessBudget, control: &PrepareControl,
) -> Result<u64, NativeError> {
    let start = Instant::now();
    #[cfg(test)]
    let input_bytes=input.as_ref().map(|(_,n)|*n).unwrap_or(0);
    budget.check(start, control)?;
    #[cfg(test)]
    let mut idle_wait=Duration::ZERO;
    let result = (|| {
        let mut command = decoder(name)?;
        let mut child = OwnedChild {
            child: command.args(arguments).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?,
            reaped: false,
        };
        let mut stdout = child.child.stdout.take().ok_or_else(NativeError::params)?;
        let mut stderr = child.child.stderr.take().ok_or_else(NativeError::params)?;
        let mut stdin = child.child.stdin.take();
        if let Some(pipe) = &stdin { nonblocking(pipe.as_raw_fd())?; }
        nonblocking(stdout.as_raw_fd())?;
        nonblocking(stderr.as_raw_fd())?;
        let mut bytes = 0u64;
        let mut out_eof = false;
        let mut err_eof = false;
        let mut status = None;
        let mut buffer = [0u8; BUFFER];
        let mut pending = [0u8; BUFFER];
        let mut pending_start = 0usize;
        let mut pending_end = 0usize;
        loop {
            budget.check(start, control)?;
            let mut made_progress = false;
            if let (Some((file, remaining)), Some(pipe)) = (input.as_mut(), stdin.as_mut()) {
                if pending_start == pending_end {
                    pending_start = 0;
                    pending_end = 0;
                    if *remaining != 0 {
                        let n = file.read(&mut pending[..(*remaining).min(BUFFER as u64) as usize])?;
                        if n == 0 { return Err(NativeError::new("stalePreview", "Admitted archive input became shorter while hashing.")); }
                        *remaining -= n as u64;
                        pending_end = n;
                    }
                }
                if pending_end != pending_start {
                    match pipe.write(&pending[pending_start..pending_end]) {
                        Ok(0) => return Err(NativeError::new("archiveDecodeFailed", "Hash input pipe closed early.")),
                        Ok(n) => { pending_start += n; made_progress = true; },
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {},
                        Err(e) => return Err(e.into()),
                    }
                }
                if *remaining == 0 && pending_start == pending_end {
                    if file.read(&mut pending[..1])? != 0 { return Err(NativeError::new("stalePreview", "Admitted archive input grew while hashing.")); }
                    stdin.take();
                    input.take();
                }
            }
            // At most one buffer from each stream per cycle: neither flood can
            // starve the other stream, cancellation, wall checks, or child reap.
            if !err_eof {
                match stderr.read(&mut buffer) {
                    Ok(0) => err_eof = true,
                    Ok(n) => {
                        made_progress = true;
                        budget.diagnostics = budget.diagnostics.checked_add(n as u64).ok_or_else(|| limit("Diagnostic counter overflow."))?;
                        if budget.diagnostics > DIAGNOSTICS { return Err(limit("Archive diagnostics exceeded 64 KiB for this job.")); }
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if !out_eof {
                match stdout.read(&mut buffer) {
                    Ok(0) => out_eof = true,
                    Ok(n) => {
                        made_progress = true;
                        bytes = bytes.checked_add(n as u64).ok_or_else(|| limit("Decoder output counter overflow."))?;
                        if bytes > max_output { return Err(limit("Archive decoder exceeded its admitted output size.")); }
                        output.write_all(&buffer[..n])?;
                    }
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if status.is_none() {
                status = child.child.try_wait()?;
                if status.is_some() { child.reaped = true; }
            }
            if out_eof && err_eof && status.is_some() {
                if input.is_some() { return Err(NativeError::new("archiveDecodeFailed", "Hash child exited before consuming the admitted input.")); }
                break;
            }
            if !made_progress {
                #[cfg(test)] let wait=Instant::now();
                // Wake on pipe readiness instead of charging every pipe buffer
                // a fixed delay. The timeout bounds cancellation/reap latency.
                let mut fds = Vec::new();
                if !out_eof { fds.push(libc::pollfd{fd:stdout.as_raw_fd(),events:libc::POLLIN,revents:0}); }
                if !err_eof { fds.push(libc::pollfd{fd:stderr.as_raw_fd(),events:libc::POLLIN,revents:0}); }
                if let Some(pipe)=&stdin { fds.push(libc::pollfd{fd:pipe.as_raw_fd(),events:libc::POLLOUT,revents:0}); }
                let result=unsafe{libc::poll(fds.as_mut_ptr(),fds.len() as libc::nfds_t,20)};
                #[cfg(test)] {idle_wait+=wait.elapsed();}
                if result<0 && std::io::Error::last_os_error().kind()!=std::io::ErrorKind::Interrupted {return Err(std::io::Error::last_os_error().into());}
            }
        }
        budget.check(start, control)?;
        if !status.unwrap().success() {
            return Err(NativeError::new("archiveDecodeFailed", "Archive decoder did not exit successfully."));
        }
        Ok(bytes)
    })();
    #[cfg(test)]
    if std::env::var_os("ITEM103_ARCHIVE").is_some(){eprintln!("item103 decoder={name} ms={} input_bytes={input_bytes} output_bytes={:?} idle_wait_ms={}",start.elapsed().as_millis(),result.as_ref().ok(),idle_wait.as_millis());}
    budget.elapsed = budget.elapsed.saturating_add(start.elapsed());
    result
}
