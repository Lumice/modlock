mod support;
use modlock_helper::{native::NativeService, sqlite::Database, store};
use json::{object, JsonValue};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use support::{import, mutation, Fixture};

const JOB: &str = "11111111111111111111111111111111";
fn owned() -> (Fixture, NativeService, std::path::PathBuf) {
    let f = Fixture::new();
    let mut service = NativeService::new();
    import(&mut service, &f);
    let source = f.root.join("local.vpk");
    fs::write(&source, support::vpk("models/local.vmdl_c", 9)).unwrap();
    (f, service, source)
}
fn prepare(f: &Fixture, source: &Path, id: &str) -> JsonValue {
    object! {dataDir:f.data.to_str().unwrap(),jobId:id,profileId:"default",expectedRevision:"1",roots:json::array![source.to_str().unwrap()]}
}
fn id_params(f: &Fixture, id: &str) -> JsonValue {
    object! {dataDir:f.data.to_str().unwrap(),jobId:id}
}
fn ready(s: &mut NativeService, f: &Fixture, id: &str) -> JsonValue {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = s
            .dispatch("native.local.status", &id_params(f, id))
            .unwrap();
        if status["phase"] == "ready" {
            return status;
        }
        assert!(
            !["failed", "cancelled", "interrupted"].contains(&status["phase"].as_str().unwrap()),
            "{status}"
        );
        assert!(Instant::now() < deadline, "{status}");
        thread::sleep(Duration::from_millis(5));
    }
}
fn review(s: &mut NativeService, f: &Fixture, id: &str) -> JsonValue {
    let mut p = id_params(f, id);
    p["includeCandidates"] = true.into();
    let b = s.dispatch("native.local.status", &p).unwrap();
    let mut ids = b["candidateSummary"]["defaultSelectedCandidateIds"]
        .members()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    p.remove("includeCandidates");
    p["candidateIds"] = JsonValue::from(ids);
    p["metadata"] = b["candidateSummary"]["defaultMetadata"].clone();
    s.dispatch("native.local.review", &p).unwrap()
}
struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<String>,
    next: u64,
}
impl Helper {
    fn new(f: &Fixture, barrier: bool) -> Self {
        Self::with_commit_barrier(f, barrier, false)
    }
    fn new_commit(f: &Fixture) -> Self {
        Self::with_commit_barrier(f, true, true)
    }
    fn with_commit_barrier(f: &Fixture, barrier: bool, commit: bool) -> Self {
        let mut c = Command::new(env!("CARGO_BIN_EXE_modlock-helper"));
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if commit {
            let shim = f.root.join("commit-barrier.so");
            let status = Command::new("cc").args(["-shared", "-fPIC", "-o"]).arg(&shim)
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/commit_barrier.c"))
                .arg("-ldl").status().unwrap();
            assert!(status.success(), "compile the private writer barrier");
            c.env("LD_PRELOAD", shim);
        }
        if barrier {
            let bin = f.root.join("test-bin");
            fs::create_dir(&bin).unwrap();
            let wrapper = bin.join("sha256sum");
            fs::write(
                &wrapper,
                r#"#!/usr/bin/python3
import os, time
root=os.environ['LOCAL_TEST_BARRIER']
with open(root+'/reader-pid', 'w') as f: f.write(str(os.getpid()))
while not os.path.exists(root+'/release'):
    time.sleep(0.01)
os.execv('/usr/bin/sha256sum', ['sha256sum'])
"#,
            )
            .unwrap();
            fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700)).unwrap();
            c.env("LOCAL_TEST_BARRIER", &f.root)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        }
        let mut child = c.spawn().unwrap();
        let input = child.stdin.take();
        let out = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            output,
            next: 0,
        }
    }
    fn send(&mut self, method: &str, params: JsonValue) {
        self.next += 1;
        let p = self.input.as_mut().unwrap();
        writeln!(
            p,
            "{}",
            object! {jsonrpc:"2.0",id:self.next,method:method,params:params}.dump()
        )
        .unwrap();
        p.flush().unwrap();
    }
    fn receive(&self) -> JsonValue {
        let line = self
            .output
            .recv_timeout(Duration::from_secs(10))
            .expect("live helper must answer");
        assert!(line.len() + 1 <= 48 * 1024);
        json::parse(&line).unwrap()
    }
    fn call(&mut self, method: &str, p: JsonValue) -> JsonValue {
        self.send(method, p);
        self.receive()
    }
    fn close(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                assert!(s.success());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "helper did not exit cooperatively"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        self.input.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
fn barrier(f: &Fixture) -> u32 {
    let p = f.root.join("reader-pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(s) = fs::read_to_string(&p) {
            if let Ok(pid) = s.parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "reader did not reach barrier");
        thread::sleep(Duration::from_millis(5));
    }
}
fn gone(pid: u32) {
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "owned reader was not reaped"
    );
}

#[test]
fn local_job_cancel_answers_while_hashing() {
    let (f, s, source) = owned();
    drop(s);
    let before = fs::read(&source).unwrap();
    let mtime = fs::metadata(&source).unwrap().modified().unwrap();
    let mut h = Helper::new(&f, true);
    assert_eq!(
        h.call("native.local.prepare", prepare(&f, &source, JOB))["result"]["jobId"],
        JOB
    );
    let pid = barrier(&f);
    let reply = h.call("native.local.cancel", id_params(&f, JOB));
    assert_eq!(reply["result"]["phase"], "cancelRequested", "{reply}");
    assert!(!f.root.join("release").exists());
    fs::write(f.root.join("release"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < deadline,
            "cancelled worker was not observed"
        );
        let r = h.call("native.local.status", id_params(&f, JOB));
        if r["result"]["phase"] == "cancelled" {
            break;
        }
        assert_eq!(r["result"]["phase"], "cancelRequested");
        thread::sleep(Duration::from_millis(5));
    }
    gone(pid);
    h.close();
    assert!(!fs::read_dir(&f.data).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".local-quarantine-")));
    assert_eq!(fs::read(&source).unwrap(), before);
    assert_eq!(fs::metadata(&source).unwrap().modified().unwrap(), mtime);
}
#[test]
fn local_job_preparation_does_not_block_profile_reads() {
    let (f, s, source) = owned();
    drop(s);
    let mut h = Helper::new(&f, true);
    assert!(h.call("native.local.prepare", prepare(&f, &source, JOB))["result"].is_object());
    let pid = barrier(&f);
    let r = h.call(
        "native.snapshot",
        object! {dataDir:f.data.to_str().unwrap(),profileId:"other"},
    );
    assert_eq!(r["result"]["selectedProfileId"], "other");
    assert!(!f.root.join("release").exists());
    h.close();
    gone(pid);
}
#[test]
fn local_job_input_replay_is_exact() {
    let (f, mut s, source) = owned();
    let p = prepare(&f, &source, JOB);
    s.dispatch("native.local.prepare", &p).unwrap();
    ready(&mut s, &f, JOB);
    let a = s.dispatch("native.local.prepare", &p).unwrap();
    let b = s.dispatch("native.local.prepare", &p).unwrap();
    assert_eq!(a, b);
    for key in ["profileId", "expectedRevision", "roots", "artworkPath"] {
        let mut changed = p.clone();
        changed[key] = match key {
            "profileId" => "other".into(),
            "expectedRevision" => "2".into(),
            "roots" => json::array![source.to_str().unwrap(), source.to_str().unwrap()],
            _ => source.to_str().unwrap().into(),
        };
        assert_eq!(
            s.dispatch("native.local.prepare", &changed)
                .unwrap_err()
                .kind,
            "jobIdConflict"
        );
    }
    for (key, v) in [
        ("unexpected", true.into()),
        ("artworkPath", JsonValue::Null),
        ("expectedRevision", "01".into()),
    ] {
        let mut p = p.clone();
        p[key] = v;
        assert_eq!(
            s.dispatch("native.local.prepare", &p).unwrap_err().kind,
            "invalidParams"
        );
    }
    let mut alias = p.clone();
    alias["dataDir"] = format!("{}/", f.data.display()).into();
    assert_eq!(s.dispatch("native.local.prepare", &alias).unwrap(), b);
}
#[test]
fn local_job_profile_revision_is_pinned() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    s.dispatch(
        "native.snapshot",
        &object! {dataDir:f.data.to_str().unwrap(),profileId:"other"},
    )
    .unwrap();
    let mut change = mutation(&f, "1", "rename-during-preparation");
    change["action"] = "rename".into();
    change["profileId"] = "default".into();
    change["name"] = "Renamed".into();
    s.dispatch("native.profile", &change).unwrap();
    let b = s
        .dispatch("native.local.status", &id_params(&f, JOB))
        .unwrap();
    assert_eq!(b["profileId"], "default");
    assert_eq!(b["profileNameAtStart"], "Default");
    assert_eq!(b["expectedRevision"], "1");
    assert_eq!(b["observedRevision"], "2");
    let mut p = id_params(&f, JOB);
    p["candidateIds"] = json::array![];
    p["metadata"] = object! {name:"n",author:"",category:"",link:"",description:""};
    assert_eq!(
        s.dispatch("native.local.review", &p).unwrap_err().kind,
        "staleRevision"
    );
}
#[test]
fn local_job_stale_response_sequence() {
    let (f, mut s, source) = owned();
    let initial = s
        .dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    let r = ready(&mut s, &f, JOB);
    assert!(
        store::revision(r["sequence"].as_str().unwrap()).unwrap()
            > store::revision(initial["sequence"].as_str().unwrap()).unwrap()
    );
    let a = s
        .dispatch("native.local.cancel", &id_params(&f, JOB))
        .unwrap();
    assert_eq!(a["phase"], "cancelled");
    assert_eq!(
        s.dispatch("native.local.status", &id_params(&f, JOB))
            .unwrap(),
        a
    );
    assert_eq!(
        s.dispatch("native.local.cancel", &id_params(&f, JOB))
            .unwrap(),
        a
    );
    drop(s);
    let mut s = NativeService::new();
    assert_eq!(
        s.dispatch("native.local.status", &id_params(&f, JOB))
            .unwrap(),
        a
    );
}
#[test]
fn local_job_busy_and_eight_record_bounds() {
    let (f, s, source) = owned();
    drop(s);
    let mut h = Helper::new(&f, true);
    h.call("native.local.prepare", prepare(&f, &source, JOB));
    barrier(&f);
    assert_eq!(
        h.call(
            "native.local.prepare",
            prepare(&f, &source, &"2".repeat(32))
        )["error"]["data"]["kind"],
        "nativeBusy"
    );
    h.close();
    let mut s = NativeService::new();
    for n in 2..=8 {
        let id = format!("{n:032x}");
        s.dispatch("native.local.prepare", &prepare(&f, &source, &id))
            .unwrap();
        ready(&mut s, &f, &id);
        s.dispatch("native.local.cancel", &id_params(&f, &id))
            .unwrap();
    }
    let all = s.dispatch("native.local.status", &f.params()).unwrap();
    assert_eq!(all.len(), 8);
    assert!(all.dump().len() < 48 * 1024);
    assert_eq!(
        s.dispatch(
            "native.local.prepare",
            &prepare(&f, &source, &format!("{:032x}", 9))
        )
        .unwrap_err()
        .kind,
        "localJobLimit"
    );
    s.dispatch("native.local.dismiss", &id_params(&f, JOB))
        .unwrap();
    assert_eq!(
        s.dispatch("native.local.status", &f.params())
            .unwrap()
            .len(),
        7
    );
}
#[test]
fn local_job_restart_never_resumes_or_reuses_token() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    let review = review(&mut s, &f, JOB);
    assert!(review["token"].is_string());
    drop(s);
    fs::remove_file(&source).unwrap();
    let mut s = NativeService::new();
    let r = s
        .dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    assert_eq!(r["phase"], "interrupted");
    assert!(!r.has_key("token"));
    assert_eq!(
        s.dispatch(
            "native.local.status",
            &object! {dataDir:f.data.to_str().unwrap(),jobId:JOB,includeCandidates:true}
        )
        .unwrap_err()
        .kind,
        "localJobNotReady"
    );
}
#[test]
fn local_job_unknown_schema_preserves_bytes() {
    let (f, mut s, _) = owned();
    let dbpath = f.data.join("local-jobs.sqlite3");
    let db = Database::open(&dbpath, true).unwrap();
    db.execute("PRAGMA user_version=99", &[]).unwrap();
    drop(db);
    let before = fs::read(&dbpath).unwrap();
    assert_eq!(
        s.dispatch("native.local.status", &f.params())
            .unwrap_err()
            .kind,
        "unsupportedLocalJobVersion"
    );
    assert_eq!(fs::read(&dbpath).unwrap(), before);
    assert_eq!(
        s.dispatch("native.snapshot", &f.params()).unwrap()["revision"],
        "1"
    );
    assert_eq!(
        s.dispatch(
            "native.operation",
            &object! {dataDir:f.data.to_str().unwrap(),operationId:"import-1"}
        )
        .unwrap()["status"],
        "succeeded"
    );
}
#[test]
fn local_job_eof_cancels_reader_but_does_not_kill_writer() {
    let (f, s, source) = owned();
    drop(s);
    let mut h = Helper::new(&f, true);
    h.call("native.local.prepare", prepare(&f, &source, JOB));
    let pid = barrier(&f);
    h.close();
    gone(pid);
    assert!(!fs::read_dir(&f.data).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".local-quarantine-")));
    let mut h = Helper::new(&f, false);
    let mut p = mutation(&f, "1", "eof-writer");
    p["action"] = "rename".into();
    p["profileId"] = "default".into();
    p["name"] = "EOF saved".into();
    h.send("native.profile", p);
    h.close();
    assert_eq!(h.receive()["result"]["revision"], "2");
    let mut s = NativeService::new();
    assert_eq!(
        s.dispatch(
            "native.operation",
            &object! {dataDir:f.data.to_str().unwrap(),operationId:"eof-writer"}
        )
        .unwrap()["status"],
        "succeeded"
    );
}
#[test]
fn local_job_admission_and_read_paths_never_bootstrap() {
    let f = Fixture::new();
    let mut s = NativeService::new();
    assert_eq!(
        s.dispatch("native.local.status", &f.params()).unwrap(),
        json::array![]
    );
    assert!(!f.data.exists());
    let p = prepare(&f, &f.source, JOB);
    assert_eq!(
        s.dispatch("native.local.prepare", &p).unwrap_err().kind,
        "nativeNotImported"
    );
    assert!(!f.data.exists());
    import(&mut s, &f);
    assert!(!f.data.join("local-jobs.sqlite3").exists());
    let mut p = p;
    p["profileId"] = "missing".into();
    assert_eq!(
        s.dispatch("native.local.prepare", &p).unwrap_err().kind,
        "unknownProfile"
    );
    assert!(!f.data.join("local-jobs.sqlite3").exists());
}
#[test]
fn local_job_lease_root_binding_and_small_bulk_status() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    let mut other = NativeService::new();
    assert_eq!(
        other
            .dispatch("native.local.status", &f.params())
            .unwrap_err()
            .kind,
        "nativeBusy"
    );
    assert!(other.dispatch("native.snapshot", &f.params()).is_ok());
    let (g, _, _) = owned();
    assert_eq!(
        s.dispatch("native.local.status", &g.params())
            .unwrap_err()
            .kind,
        "invalidParams"
    );
    let small = s
        .dispatch("native.local.status", &id_params(&f, JOB))
        .unwrap();
    assert!(small.dump().len() <= 4096);
    assert!(!small.has_key("candidateSummary"));
    let mut p = id_params(&f, JOB);
    p["includeCandidates"] = true.into();
    let bulk = s.dispatch("native.local.status", &p).unwrap();
    let c = &bulk["candidateSummary"];
    assert_eq!(c["candidates"][0]["members"][0]["role"], "directory");
    assert_eq!(
        c["candidates"][0]["members"][0]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(c["sources"][0]["kind"], "file");
    assert!(c["exclusions"].is_array());
    assert!(c["defaultMetadata"].is_object());
    let a = review(&mut s, &f, JOB);
    let b = review(&mut s, &f, JOB);
    assert_ne!(a["token"], b["token"]);
    assert_eq!(b["expectedRevision"], "1");
    assert!(b["uniqueCopyBytes"].as_u64().unwrap() > 0);
}

#[test]
fn local_job_startup_validates_every_row_before_reconciliation() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    drop(s);
    let path = f.data.join("local-jobs.sqlite3");
    let db = Database::open(&path, true).unwrap();
    let row = db
        .query("SELECT input,body FROM jobs WHERE id=?", &[JOB])
        .unwrap()
        .unwrap();
    let mut bad = json::parse(&row[1]).unwrap();
    bad["jobId"] = "f".repeat(32).into();
    bad["sequence"] = "01".into();
    db.execute(
        "INSERT INTO jobs(id,input,body) VALUES(?,?,?)",
        &[&"f".repeat(32), &row[0], &bad.dump()],
    )
    .unwrap();
    drop(db);
    let before = fs::read(&path).unwrap();
    let mut s = NativeService::new();
    assert_eq!(
        s.dispatch("native.local.status", &f.params())
            .unwrap_err()
            .kind,
        "localJobDatabase"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(s.dispatch("native.snapshot", &f.params()).is_ok());
}
#[test]
fn local_job_restart_reconciles_committing_without_running_writers() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    drop(s);
    let lock = store::RootLock::acquire(&f.data).unwrap();
    let library = store::Store::open_writer(&lock).unwrap();
    library
        .record_failure(
            &lock,
            "rejected-local",
            &object! {},
            &modlock_helper::native::NativeError::new("invalidLocalSelection", "rejected"),
        )
        .unwrap();
    library
        .write_journal(&lock, "journal-local", &object! {kind:"import",stage:".import-0123456789abcdef0123456789abcdef",input:object!{method:"native.importCommit",params:object!{}},payloads:json::array![]})
        .unwrap();
    drop(library);
    drop(lock);
    let path = f.data.join("local-jobs.sqlite3");
    let db = Database::open(&path, true).unwrap();
    let row = db
        .query("SELECT input,body FROM jobs WHERE id=?", &[JOB])
        .unwrap()
        .unwrap();
    db.execute("DELETE FROM jobs", &[]).unwrap();
    for (n, op) in [
        (1, "import-1"),
        (2, "rejected-local"),
        (3, "journal-local"),
        (4, "missing-local"),
    ] {
        let id = format!("{n:032x}");
        let mut b = json::parse(&row[1]).unwrap();
        b["jobId"] = id.clone().into();
        b["phase"] = "committing".into();
        b["canCancel"] = false.into();
        b["operationId"] = op.into();
        db.execute(
            "INSERT INTO jobs(id,input,body) VALUES(?,?,?)",
            &[&id, &row[0], &b.dump()],
        )
        .unwrap();
    }
    drop(db);
    let library_before = fs::read(f.data.join("library.sqlite3")).unwrap();
    let mut s = NativeService::new();
    let all = s.dispatch("native.local.status", &f.params()).unwrap();
    for (n, p) in ["succeeded", "failed", "recoveryRequired", "interrupted"]
        .iter()
        .enumerate()
    {
        assert_eq!(all[n]["phase"], *p);
        assert!(!all[n].has_key("token"));
    }
    assert_eq!(
        fs::read(f.data.join("library.sqlite3")).unwrap(),
        library_before
    );
    assert_eq!(
        s.dispatch(
            "native.local.dismiss",
            &id_params(&f, &format!("{:032x}", 3))
        )
        .unwrap_err()
        .kind,
        "recoveryRequired"
    );
}

#[test]
fn local_job_unknown_wal_schema_preserves_existing_sidecars() {
    let (f, mut s, _) = owned();
    let path = f.data.join("local-jobs.sqlite3");
    let db = Database::open(&path, true).unwrap();
    db.query("PRAGMA journal_mode=WAL", &[]).unwrap();
    db.execute("PRAGMA user_version=99", &[]).unwrap();
    let artifacts = [
        "local-jobs.sqlite3",
        "local-jobs.sqlite3-wal",
        "local-jobs.sqlite3-shm",
    ]
    .map(|name| {
        let p = f.data.join(name);
        let bytes = fs::read(&p).unwrap();
        (p, bytes)
    });
    assert_eq!(
        s.dispatch("native.local.status", &f.params())
            .unwrap_err()
            .kind,
        "unsupportedLocalJobVersion"
    );
    for (path, bytes) in artifacts {
        assert!(
            fs::read(&path).unwrap() == bytes,
            "changed {}",
            path.display()
        );
    }
    drop(db);
}
#[test]
fn local_job_historical_operation_replay_does_not_regress_observed_revision() {
    let (f, mut s, source) = owned();
    s.dispatch("native.local.prepare", &prepare(&f, &source, JOB))
        .unwrap();
    ready(&mut s, &f, JOB);
    let mut p = mutation(&f, "1", "first-rename");
    p["action"] = "rename".into();
    p["profileId"] = "default".into();
    p["name"] = "First".into();
    s.dispatch("native.profile", &p).unwrap();
    let mut q = mutation(&f, "2", "second-rename");
    q["action"] = "rename".into();
    q["profileId"] = "default".into();
    q["name"] = "Second".into();
    s.dispatch("native.profile", &q).unwrap();
    assert_eq!(
        s.dispatch("native.local.status", &id_params(&f, JOB))
            .unwrap()["observedRevision"],
        "3"
    );
    s.dispatch("native.profile", &p).unwrap();
    assert_eq!(
        s.dispatch("native.local.status", &id_params(&f, JOB))
            .unwrap()["observedRevision"],
        "3"
    );
}

#[test]
fn local_job_checkpointed_wal_and_short_headers_are_rejected_without_side_effects() {
    for short in [false, true] {
        let (f, mut s, _) = owned();
        let path = f.data.join("local-jobs.sqlite3");
        if short {
            fs::write(&path, b"SQLite format 3\0").unwrap();
        } else {
            let db = Database::open(&path, true).unwrap();
            db.query("PRAGMA journal_mode=WAL", &[]).unwrap();
            db.execute("PRAGMA user_version=99", &[]).unwrap();
            drop(db);
            assert_eq!(fs::read(&path).unwrap()[18], 2);
        }
        assert!(!f.data.join("local-jobs.sqlite3-wal").exists());
        assert!(!f.data.join("local-jobs.sqlite3-shm").exists());
        let before = fs::read(&path).unwrap();
        assert_eq!(
            s.dispatch("native.local.status", &f.params())
                .unwrap_err()
                .kind,
            "unsupportedLocalJobVersion"
        );
        assert!(fs::read(&path).unwrap() == before);
        assert!(!f.data.join("local-jobs.sqlite3-wal").exists());
        assert!(!f.data.join("local-jobs.sqlite3-shm").exists());
    }
}

#[test]
fn local_job_hot_rollback_journal_is_preserved_and_native_reads_remain_usable() {
    let (f, mut service, _) = owned();
    let path = f.data.join("local-jobs.sqlite3");
    let db = Database::open(&path, true).unwrap();
    db.execute("PRAGMA user_version=1", &[]).unwrap();
    db.execute(
        "CREATE TABLE jobs(id TEXT PRIMARY KEY,input TEXT NOT NULL,body TEXT NOT NULL)",
        &[],
    )
    .unwrap();
    drop(db);
    // Crash only this disposable SQLite fixture process after cache spill,
    // leaving genuine uncommitted pages and a hot rollback journal.
    let script = r#"import sqlite3,sys,os
c=sqlite3.connect(sys.argv[1])
c.execute('PRAGMA cache_size=1')
c.execute('BEGIN IMMEDIATE')
for n in range(16):
    c.execute('INSERT INTO jobs VALUES(?,?,?)',(str(n),'{}','x'*8192))
os._exit(0)
"#;
    assert!(Command::new("/usr/bin/python3")
        .args(["-c", script])
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let journal = f.data.join("local-jobs.sqlite3-journal");
    let before = fs::read(&path).unwrap();
    let journal_before = fs::read(&journal).unwrap();
    assert!(!journal_before.is_empty());
    assert_eq!(
        service
            .dispatch("native.local.status", &f.params())
            .unwrap_err()
            .kind,
        "nativeDatabase"
    );
    assert!(fs::read(&path).unwrap() == before, "main database changed");
    assert!(
        fs::read(&journal).unwrap() == journal_before,
        "rollback journal changed"
    );
    assert!(!f.data.join("local-jobs.sqlite3-wal").exists());
    assert!(!f.data.join("local-jobs.sqlite3-shm").exists());
    assert_eq!(
        service.dispatch("native.snapshot", &f.params()).unwrap()["revision"],
        "1"
    );
    assert_eq!(
        service
            .dispatch(
                "native.operation",
                &object! {dataDir:f.data.to_str().unwrap(),operationId:"import-1"}
            )
            .unwrap()["status"],
        "succeeded"
    );
}

#[test]
fn local_commit_job_write_failure_keeps_operation_success_and_returns_unknown() {
    let (f, s, source) = owned();
    drop(s);
    fs::write(f.root.join("release"), b"go").unwrap();
    let mut h = Helper::new_commit(&f);
    h.call("native.local.prepare", prepare(&f, &source, JOB));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = h.call("native.local.status", id_params(&f, JOB));
        if status["result"]["phase"] == "ready" {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let mut params = id_params(&f, JOB);
    params["includeCandidates"] = true.into();
    let candidates = h.call("native.local.status", params)["result"]["candidateSummary"].clone();
    let review=h.call("native.local.review",object!{dataDir:f.data.to_str().unwrap(),jobId:JOB,candidateIds:candidates["defaultSelectedCandidateIds"].clone(),metadata:candidates["defaultMetadata"].clone()});
    assert!(review["result"]["token"].is_string(), "{review}");
    fs::remove_file(f.root.join("release")).unwrap();
    let _ = fs::remove_file(f.root.join("reader-pid"));
    let mut params = mutation(&f, "1", "local-postcommit");
    params["jobId"] = JOB.into();
    params["profileId"] = "default".into();
    params["token"] = review["result"]["token"].clone();
    h.send("native.local.commit", params.clone());
    barrier(&f);
    let db = Database::open(&f.data.join("local-jobs.sqlite3"), true).unwrap();
    let committing = db
        .query("SELECT body FROM jobs WHERE id=?", &[JOB])
        .unwrap()
        .unwrap();
    assert_eq!(json::parse(&committing[0]).unwrap()["phase"], "committing");
    let transaction = db.transaction().unwrap();
    fs::write(f.root.join("release"), b"go").unwrap();
    let response = h.receive();
    assert_eq!(response["error"]["code"], -32603, "{response}");
    assert_eq!(response["error"]["data"]["kind"], "localJobOutcomeUnknown");
    let op = h.call(
        "native.operation",
        object! {dataDir:f.data.to_str().unwrap(),operationId:"local-postcommit"},
    );
    assert_eq!(op["result"]["status"], "succeeded");
    assert_eq!(op["result"]["result"]["revision"], "2");
    assert_eq!(
        h.call("native.snapshot", f.params())["result"]["revision"],
        "2"
    );
    assert_eq!(
        h.call("native.local.commit", params)["result"]["revision"],
        "2"
    );
    assert_eq!(
        db.query("SELECT body FROM jobs WHERE id=?", &[JOB])
            .unwrap()
            .unwrap(),
        committing
    );
    drop(transaction);
    drop(db);
    h.close();
    let status = NativeService::new()
        .dispatch("native.local.status", &id_params(&f, JOB))
        .unwrap();
    assert_eq!(status["phase"], "succeeded");
    assert_eq!(
        store::Store::open(&f.data).unwrap().document().unwrap()["profiles"]["default"]["mods"]
            .len(),
        2
    );
}

#[test]
fn local_cancel_queued_behind_reserved_writer_reports_actual_success() {
    let (f, s, source) = owned();
    drop(s);
    fs::write(f.root.join("release"), b"go").unwrap();
    let mut h = Helper::new_commit(&f);
    h.call("native.local.prepare", prepare(&f, &source, JOB));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = h.call("native.local.status", id_params(&f, JOB));
        if status["result"]["phase"] == "ready" {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let mut bulk = id_params(&f, JOB);
    bulk["includeCandidates"] = true.into();
    let c = h.call("native.local.status", bulk)["result"]["candidateSummary"].clone();
    let r=h.call("native.local.review",object!{dataDir:f.data.to_str().unwrap(),jobId:JOB,candidateIds:c["defaultSelectedCandidateIds"].clone(),metadata:c["defaultMetadata"].clone()});
    fs::remove_file(f.root.join("release")).unwrap();
    let _ = fs::remove_file(f.root.join("reader-pid"));
    let mut params = mutation(&f, "1", "queued-local");
    params["jobId"] = JOB.into();
    params["profileId"] = "default".into();
    params["token"] = r["result"]["token"].clone();
    h.send("native.local.commit", params);
    barrier(&f);
    h.send("native.local.cancel", id_params(&f, JOB));
    assert!(h.output.try_recv().is_err());
    fs::write(f.root.join("release"), b"go").unwrap();
    assert_eq!(h.receive()["result"]["revision"], "2");
    let cancel = h.receive();
    assert_eq!(cancel["result"]["phase"], "succeeded");
    assert_eq!(cancel["result"]["canCancel"], false);
    h.close();
}
