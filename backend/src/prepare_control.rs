//! Cooperative controls for read-only preparation. No writer is owned here.
use crate::{
    native::NativeError,
    paths::{self, Fingerprint},
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
#[derive(Clone, Debug, Default)]
pub struct PrepareProgress {
    pub phase: String,
    pub files_done: u64,
    pub files_total: Option<u64>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}
#[derive(Clone, Default)]
pub struct PrepareControl {
    inner: Arc<Inner>,
}
#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    #[cfg(test)]
    hashes_started: std::sync::atomic::AtomicUsize,
    progress: Mutex<PrepareProgress>,
    #[cfg(test)]
    hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl PrepareControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn never_cancelled() -> Self {
        Self::new()
    }
    pub fn request_cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }
    pub fn progress(&self) -> PrepareProgress {
        self.inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn check(&self) -> Result<(), NativeError> {
        #[cfg(test)]
        {
            let hook = self.inner.hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        if self.inner.cancelled.load(Ordering::Acquire) {
            Err(NativeError::new(
                "localPreparationCancelled",
                "Local preparation was cancelled.",
            ))
        } else {
            Ok(())
        }
    }
    pub fn report(&self, mut progress: PrepareProgress) {
        progress.phase = progress.phase.chars().take(64).collect();
        *self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = progress;
    }
    #[cfg(test)]
    pub(crate) fn hashes_started(&self) -> usize {
        self.inner.hashes_started.load(Ordering::SeqCst)
    }
    #[cfg(test)]
    pub(crate) fn check_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self.inner.hook.lock().unwrap() = Some(hook);
    }
}
fn identity(m: &fs::Metadata) -> (u64, i64, i64, u64, u64) {
    (m.len(), m.mtime(), m.mtime_nsec(), m.dev(), m.ino())
}
fn hash_error() -> NativeError {
    NativeError::new(
        "nativeIo",
        "Cannot fingerprint payload; invalid or excessive sha256sum output.",
    )
}
struct ReaderChild(Child);
impl Drop for ReaderChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
// Linux fcntl operates only on our SHA child's pipes. It cannot affect a writer.
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}
pub fn fingerprint_controlled(
    path: &Path,
    control: &PrepareControl,
) -> Result<Fingerprint, NativeError> {
    fingerprint_command(
        path,
        control,
        &mut Command::new("sha256sum"),
        None,
        &mut |_| {},
    )
}
/// Bind admission to the exact descriptor handed to the reader, before spawn.
pub(crate) fn fingerprint_admitted(
    path: &Path,
    control: &PrepareControl,
    expected: &fs::Metadata,
    max_bytes: u64,
) -> Result<Fingerprint, NativeError> {
    fingerprint_command(
        path,
        control,
        &mut Command::new("sha256sum"),
        Some((expected, max_bytes)),
        &mut |_| {},
    )
}
fn fingerprint_command(
    path: &Path,
    control: &PrepareControl,
    command: &mut Command,
    admission: Option<(&fs::Metadata, u64)>,
    spawned: &mut impl FnMut(u32),
) -> Result<Fingerprint, NativeError> {
    control.check()?;
    let mut file = paths::open_regular(path)?;
    let before = file.metadata()?;
    if let Some((expected, max_bytes)) = admission {
        if before.len() > max_bytes
            || identity(&before) != identity(expected)
            || (before.ctime(), before.ctime_nsec(), before.mode())
                != (expected.ctime(), expected.ctime_nsec(), expected.mode())
        {
            return Err(NativeError::new(
                "stalePreview",
                "A source descriptor changed or exceeds its admitted size; review again.",
            ));
        }
    }
    file.seek(SeekFrom::Start(0))?;
    let mut child = ReaderChild(
        command
            .stdin(Stdio::from(file.try_clone()?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    #[cfg(test)]
    control.inner.hashes_started.fetch_add(1, Ordering::SeqCst);
    spawned(child.0.id());
    let sha = read_sha(&mut child, control, None)?;
    if identity(&before) != identity(&file.metadata()?)
        || identity(&before) != identity(&paths::open_regular(path)?.metadata()?)
    {
        return Err(NativeError::new(
            "stalePreview",
            "A source path changed while being read; review again.",
        ));
    }
    Ok(Fingerprint {
        sha256: sha,
        size: before.len(),
        modified_seconds: before.mtime(),
        modified_nanos: before.mtime_nsec(),
        device: before.dev(),
        inode: before.ino(),
    })
}
fn nonblocking(pipe: &impl AsRawFd) -> Result<(), NativeError> {
    let flags = unsafe { fcntl(pipe.as_raw_fd(), 3) };
    if flags < 0 || unsafe { fcntl(pipe.as_raw_fd(), 4, flags | 0x800) } < 0 {
        return Err(hash_error());
    }
    Ok(())
}
fn read_sha(
    child: &mut ReaderChild,
    control: &PrepareControl,
    mut input: Option<(ChildStdin, &[u8])>,
) -> Result<String, NativeError> {
    let mut pipe = child.0.stdout.take().ok_or_else(hash_error)?;
    nonblocking(&pipe)?;
    if let Some((stdin, _)) = &input {
        nonblocking(stdin)?;
    }
    let mut output = Vec::with_capacity(129);
    let mut eof = false;
    let mut status = None;
    loop {
        control.check()?;
        if let Some((stdin, pending)) = input.as_mut() {
            match stdin.write(pending) {
                Ok(0) if !pending.is_empty() => return Err(hash_error()),
                Ok(n) => *pending = &pending[n..],
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if input
            .as_ref()
            .is_some_and(|(_, pending)| pending.is_empty())
        {
            input.take();
        }
        loop {
            control.check()?;
            let mut buffer = [0u8; 129];
            match pipe.read(&mut buffer[..129 - output.len()]) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => {
                    output.extend_from_slice(&buffer[..n]);
                    if output.len() > 128 {
                        return Err(hash_error());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        if status.is_none() {
            status = child.0.try_wait()?;
        }
        if eof && status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    control.check()?;
    if !status.unwrap().success() {
        return Err(hash_error());
    }
    let sha = std::str::from_utf8(&output)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(hash_error)?
        .to_ascii_lowercase();
    Ok(sha)
}
/// Candidate identities use the same bounded/cancellable SHA reader as files.
pub(crate) fn digest_controlled(
    bytes: &[u8],
    control: &PrepareControl,
) -> Result<String, NativeError> {
    digest_bounded_controlled(bytes, 64 * 1024, control)
}
/// Hash already-admitted memory through the existing cancellable SHA pipes.
/// The caller chooses a tighter bound; no caller may exceed 16 MiB.
pub(crate) fn digest_bounded_controlled(
    bytes: &[u8],
    max_bytes: usize,
    control: &PrepareControl,
) -> Result<String, NativeError> {
    control.check()?;
    if max_bytes > 16 * 1024 * 1024 || bytes.len() > max_bytes {
        return Err(hash_error());
    }
    let mut child = ReaderChild(
        Command::new("sha256sum")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let input = child.0.stdin.take().ok_or_else(hash_error)?;
    read_sha(&mut child, control, Some((input, bytes)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;
    use std::sync::{atomic::AtomicUsize, Barrier};
    #[test]
    fn local_hash_oversized_output_aborts_owned_child() {
        let f = Fixture::new();
        let p = f.root.join("hash");
        fs::write(&p, b"reader").unwrap();
        let mut pid = 0;
        let start = std::time::Instant::now();
        let e = fingerprint_command(
            &p,
            &PrepareControl::new(),
            &mut Command::new("yes"),
            None,
            &mut |id| pid = id,
        )
        .unwrap_err();
        assert_eq!(e.kind, "nativeIo");
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(!f.data.exists());
    }
    #[test]
    fn local_cancel_hash_reaps_only_reader_child() {
        let f = Fixture::new();
        let p = f.root.join("hash");
        fs::write(&p, b"reader").unwrap();
        let mut unrelated = Command::new("sleep").arg("30").spawn().unwrap();
        let control = PrepareControl::new();
        let barrier = Arc::new(Barrier::new(2));
        let worker_c = control.clone();
        let worker_b = barrier.clone();
        let worker = std::thread::spawn(move || {
            let mut pid = 0;
            let error = fingerprint_command(
                &p,
                &worker_c,
                Command::new("sleep").arg("30"),
                None,
                &mut |id| {
                    pid = id;
                    worker_b.wait();
                    worker_b.wait();
                },
            )
            .unwrap_err();
            (pid, error)
        });
        barrier.wait();
        control.request_cancel();
        barrier.wait();
        let (pid, e) = worker.join().unwrap();
        assert_eq!(e.kind, "localPreparationCancelled");
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert!(!f.data.exists());
    }
    #[test]
    fn local_candidate_digest_uses_bounded_controlled_reader() {
        let control = PrepareControl::new();
        assert_eq!(
            digest_controlled(b"[0,\"name_dir.vpk\"]", &control).unwrap(),
            paths::digest_bytes(b"[0,\"name_dir.vpk\"]").unwrap()
        );
        control.request_cancel();
        assert_eq!(
            digest_controlled(b"candidate", &control).unwrap_err().kind,
            "localPreparationCancelled"
        );
    }
    #[test]
    fn bounded_progress_and_check_hook() {
        let c = PrepareControl::new();
        c.report(PrepareProgress {
            phase: "雪".repeat(200),
            ..Default::default()
        });
        assert_eq!(c.progress().phase.chars().count(), 64);
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        c.check_hook(Arc::new(move || {
            n.fetch_add(1, Ordering::Relaxed);
        }));
        c.check().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn local_admitted_descriptor_reopen_and_size_guard_prevent_sha_start() {
        for change in ["replace", "enlarge", "limit"] {
            let f = Fixture::new();
            let path = f.root.join("member.vpk");
            fs::write(&path, b"reader").unwrap();
            let expected = fs::metadata(&path).unwrap();
            let c = PrepareControl::new();
            let changed = path.clone();
            c.check_hook(Arc::new(move || match change {
                "replace" => {
                    fs::rename(&changed, changed.with_extension("keep")).unwrap();
                    fs::write(&changed, b"reader").unwrap();
                }
                "enlarge" => fs::OpenOptions::new()
                    .write(true)
                    .open(&changed)
                    .unwrap()
                    .set_len(16 * 1024 * 1024 * 1024 + 1)
                    .unwrap(),
                _ => {}
            }));
            let limit = if change == "limit" {
                5
            } else {
                16 * 1024 * 1024 * 1024
            };
            let result = fingerprint_admitted(&path, &c, &expected, limit);
            c.check_hook(Arc::new(|| {}));
            assert_eq!(result.unwrap_err().kind, "stalePreview");
            assert_eq!(
                c.hashes_started(),
                0,
                "{change} must be rejected on the verified descriptor before spawn"
            );
            assert!(!f.data.exists());
        }
    }
}
