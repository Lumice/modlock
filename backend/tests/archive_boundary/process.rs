use super::*;
use crate::archive_content::test_support::Fixture;
use std::sync::{Arc, Mutex};

unsafe extern "C" {
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

fn assert_reaped(pid: u32) {
    assert_eq!(unsafe { waitpid(pid as i32, std::ptr::null_mut(), 1) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(10)); // ECHILD
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn cancelled_or_expired_work_is_not_spawned() {
    let control = PrepareControl::new();
    control.request_cancel();
    let error = run(
        "lsar",
        &[],
        &mut Vec::new(),
        8,
        &mut ProcessBudget::default(),
        &control,
    )
    .unwrap_err();
    assert_eq!(error.kind, "localPreparationCancelled");
    let mut budget = ProcessBudget {
        elapsed: WALL,
        diagnostics: 0,
    };
    assert_eq!(
        run(
            "lsar",
            &[],
            &mut Vec::new(),
            8,
            &mut budget,
            &PrepareControl::new()
        )
        .unwrap_err()
        .kind,
        "archiveLimitExceeded"
    );
}

#[test]
fn executable_guard_refuses_untrusted_ownership() {
    // In a normal host namespace this exercises successful admission; in the
    // sandbox it proves that uid65534 is not silently treated as trusted root.
    let metadata = std::fs::metadata("/usr/bin/sha256sum").unwrap();
    if metadata.uid() != 0 {
        assert_eq!(
            decoder("sha256sum").err().unwrap().kind,
            "archiveContractUnsupported"
        );
    } else {
        decoder("sha256sum").unwrap();
    }
}

#[test]
fn owned_child_drop_kills_and_reaps_its_live_group() {
    // Exercise the actual ownership destructor with a real sleeping process.
    // This tests teardown alone, not decoder admission or the runner loop.
    let mut command = Command::new("/usr/bin/sleep");
    command.arg("60");
    unsafe {
        command.pre_exec(|| {
            if setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = OwnedChild {
        child: command.spawn().unwrap(),
        reaped: false,
    };
    let pid = child.child.id();
    drop(child);
    assert_reaped(pid);
}

#[test]
#[ignore = "host qualification: production executable ownership guard requires host uid mapping"]
fn host_runner_bounds_output_diagnostics_and_input_lengths() {
    let control = PrepareControl::new();
    let mut out = Vec::new();
    assert_eq!(
        run(
            "sha256sum",
            &[],
            &mut out,
            8,
            &mut ProcessBudget::default(),
            &control
        )
        .unwrap_err()
        .kind,
        "archiveLimitExceeded"
    );
    assert!(out.len() <= 8);
    let args: Vec<OsString> = (0..2000)
        .map(|i| format!("/no-such-archive-boundary-file-{i}").into())
        .collect();
    assert_eq!(
        run(
            "sha256sum",
            &args,
            &mut Vec::new(),
            128,
            &mut ProcessBudget::default(),
            &control
        )
        .unwrap_err()
        .kind,
        "archiveLimitExceeded"
    );
    let fixture = Fixture::new();
    let path = fixture.root.join("input");
    std::fs::write(&path, b"12345").unwrap();
    for length in [4, 6] {
        let mut file = File::open(&path).unwrap();
        assert_eq!(
            run_with_input(
                "sha256sum",
                &[],
                &mut Vec::new(),
                128,
                Some((&mut file, length)),
                &mut ProcessBudget::default(),
                &control
            )
            .unwrap_err()
            .kind,
            "stalePreview"
        );
    }
}

#[test]
#[ignore = "host qualification: production executable ownership guard requires host uid mapping"]
fn host_cancellation_returns_only_after_owned_child_reap() {
    let fixture = Fixture::new();
    let path = fixture.root.join("input");
    File::create(&path)
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let mut file = File::open(&path).unwrap();
    let control = PrepareControl::new();
    let cancel = control.clone();
    let observed = Arc::new(Mutex::new(None));
    let record = observed.clone();
    control.check_hook(Arc::new(move || {
        let children = std::fs::read_to_string("/proc/thread-self/children").unwrap();
        if let Some(pid) = children.split_whitespace().next() {
            *record.lock().unwrap() = Some(pid.parse::<u32>().unwrap());
            cancel.request_cancel();
        }
    }));
    let result = run_with_input(
        "sha256sum",
        &[],
        &mut Vec::new(),
        128,
        Some((&mut file, 1024 * 1024 * 1024)),
        &mut ProcessBudget::default(),
        &control,
    );
    control.check_hook(Arc::new(|| {}));
    assert_eq!(result.unwrap_err().kind, "localPreparationCancelled");
    assert_reaped(observed.lock().unwrap().expect("child was never observed"));
}

#[test]
fn missing_decoder_names_install_and_manual_recovery() {
    let temp = std::env::temp_dir().join(format!("dlmm-missing-decoder-{}", std::process::id()));
    assert!(!temp.exists());
    let error = super::open_decoder(&temp).unwrap_err();
    assert_eq!(error.kind, "archiveDecoderMissing");
    assert!(error.message.contains("lsar and unar"));
    assert!(error.message.contains("unar package"));
    assert!(error.message.contains("VPK"));
}

#[test]
#[ignore = "host performance regression: bounded hash pipe must not sleep once per pipe buffer"]
fn item103_hash_pipe_has_no_fixed_backoff_per_buffer() {
    let f=Fixture::new(); let path=f.root.join("hash-input");
    std::fs::write(&path,vec![42u8;64*1024*1024]).unwrap();
    let mut file=File::open(path).unwrap();let start=Instant::now();
    run_with_input("sha256sum",&[],&mut Vec::new(),128,Some((&mut file,64*1024*1024)),&mut ProcessBudget::default(),&PrepareControl::new()).unwrap();
    assert!(start.elapsed()<Duration::from_millis(700),"64 MiB hash took {:?}",start.elapsed());
}
