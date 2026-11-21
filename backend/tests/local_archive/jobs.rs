use std::time::{Instant, Duration};
use super::tests::{finished, ready, select, setup, ID};
use super::*;
use crate::test_support;

#[test]
fn archive_job_new_database_is_version_two() {
    let (f, _jobs, _) = setup();
    let db = crate::sqlite::Database::open(&f.data.join("local-jobs.sqlite3"), false).unwrap();
    assert_eq!(
        db.query("PRAGMA user_version", &[]).unwrap().unwrap(),
        ["2"]
    );
}
#[test]
fn archive_job_phases_survive_restart_validation() {
    let (f, jobs, r) = setup();
    let input = admitted_input(&r).unwrap();
    for p in ["copyingArchives", "listingArchives", "extractingArchives"] {
        let mut body = initial(&r, ID, "Default");
        body["phase"] = p.into();
        assert!(validate_record(&f.data, ID, &input, &body).is_ok(), "{p}");
        assert_eq!(next(body).unwrap()["canCancel"], true);
    }
    drop(jobs);
}
#[test]
fn archive_job_unknown_quarantine_blocks_prepare_but_keeps_history_readable() {
    let (f, jobs, r) = setup();
    drop(jobs);
    let unknown = f.data.join(".local-quarantine-unknown");
    std::fs::create_dir(&unknown).unwrap();
    let mut jobs = LocalJobs::open(&f.data).unwrap().unwrap();
    assert!(jobs.status(None, false).is_ok());
    assert_eq!(jobs.start(r, ID).unwrap_err().kind, "recoveryRequired");
    assert!(unknown.exists());
    assert!(Store::open(&f.data).unwrap().document().is_ok());
}
#[test]
fn archive_job_restarts_clean_owned_quarantine_after_expiring_review() {
    use crate::archive_quarantine::{QuarantineAuthority, QuarantineLease};
    let (f, mut jobs, r) = setup();
    ready(&mut jobs, r);
    let lease =
        QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&jobs.db), ID).unwrap();
    let scratch = lease.scratch_path().unwrap();
    let registry = lease.registry_path().unwrap();
    drop(lease);
    drop(jobs);
    let mut jobs = LocalJobs::open(&f.data).unwrap().unwrap();
    assert_eq!(
        jobs.status(Some(ID), false).unwrap()["phase"],
        "interrupted"
    );
    assert!(!scratch.exists());
    assert!(!registry.exists());
}
#[test]
fn archive_job_raw_worker_has_eager_lease_then_releases_it() {
    let (f, mut jobs, r) = setup();
    jobs.start(r, ID).unwrap();
    let claims = || {
        std::fs::read_dir(&f.data)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".local-quarantine-")
            })
            .count()
    };
    assert_eq!(claims(), 2);
    finished(&jobs);
    assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "ready");
    assert_eq!(claims(), 0);
}

#[test]
fn archive_job_v1_open_preserves_schema_and_prepare_upgrades_all_rows() {
    let (f, mut jobs, r) = setup();
    ready(&mut jobs, r.clone());
    jobs.cancel(ID).unwrap();
    drop(jobs);
    let db = crate::sqlite::Database::open(&f.data.join("local-jobs.sqlite3"), true).unwrap();
    db.execute("PRAGMA user_version=1", &[]).unwrap();
    let before = db
        .query("SELECT input,body FROM jobs WHERE id=?", &[ID])
        .unwrap()
        .unwrap();
    let mut jobs = LocalJobs::open(&f.data).unwrap().unwrap();
    assert_eq!(
        db.query("PRAGMA user_version", &[]).unwrap().unwrap(),
        ["1"]
    );
    assert_eq!(
        db.query("SELECT input,body FROM jobs WHERE id=?", &[ID])
            .unwrap()
            .unwrap(),
        before
    );
    jobs.start(r, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
    assert_eq!(
        db.query("PRAGMA user_version", &[]).unwrap().unwrap(),
        ["2"]
    );
    assert_eq!(
        db.query("SELECT input,body FROM jobs WHERE id=?", &[ID])
            .unwrap()
            .unwrap(),
        before
    );
}
#[test]
fn archive_job_cleanup_failure_stays_pending_and_blocks_preparation() {
    use crate::archive_quarantine::QuarantineAuthority;
    let (f, mut jobs, r) = setup();
    ready(&mut jobs, r.clone());
    let authority = QuarantineAuthority::for_held_jobs_store(&jobs.db);
    let lease = QuarantineLease::create(&authority, ID).unwrap();
    let scratch = lease.scratch_path().unwrap();
    std::fs::write(scratch.join("unknown"), b"keep").unwrap();
    jobs.leases.insert(ID.into(), lease);
    let body = jobs.cancel(ID).unwrap();
    assert_eq!(body["phase"], "cancelRequested");
    assert_eq!(body["cleanupError"]["kind"], "recoveryRequired");
    assert!(scratch.join("unknown").exists());
    assert_eq!(
        jobs.start(r, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap_err()
            .kind,
        "recoveryRequired"
    );
    assert!(Store::open(&f.data).unwrap().document().is_ok());
}
#[test]
fn archive_job_cancel_retained_clone_cannot_admit_another_prepare() {
    use crate::archive_quarantine::QuarantineAuthority;
    let (_f, mut jobs, r) = setup();
    ready(&mut jobs, r.clone());
    let lease =
        QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&jobs.db), ID).unwrap();
    let scratch = lease.scratch_path().unwrap();
    jobs.leases.insert(ID.into(), lease.clone());
    assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelRequested");
    assert!(scratch.exists());
    assert_eq!(
        jobs.start(r, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .unwrap_err()
            .kind,
        "nativeBusy"
    );
    drop(lease);
    assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "cancelled");
    assert!(!scratch.exists());
}

fn isolated_archive_job(
    jobs: &mut LocalJobs,
    r: &LocalPrepareRequest,
    id: &str,
) -> Arc<QuarantineLease> {
    use crate::{
        archive_content::test_support::sealed_archive,
        local_content::archive_test_support::assembled,
    };
    let lease =
        QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&jobs.db), id).unwrap();
    let original = r.roots[0].with_extension(format!("{id}.zip"));
    std::fs::write(&original, b"isolated transport").unwrap();
    let bytes = test_support::vpk("archive.bin", 1);
    let archive = sealed_archive(
        lease.clone(),
        &original,
        0,
        original.file_name().unwrap().to_str().unwrap(),
        &[("folder/a.vpk", &bytes)],
    );
    let archive = crate::archive_content::test_support::with_exclusion(archive, "readme.txt");
    let prepared = assembled(archive, &jobs.root);
    let mut body = initial(&prepared.request, id, "Default");
    body["phase"] = "ready".into();
    let input = admitted_input(&prepared.request).unwrap();
    jobs.db
        .save(&[(id.into(), input.clone(), body.clone())])
        .unwrap();
    jobs.jobs.insert(
        id.into(),
        Job {
            input,
            body,
            prepared: Some(prepared),
            plan: None,
        },
    );
    jobs.leases.insert(id.into(), lease.clone());
    lease
}
#[test]
fn archive_job_cancel_releases_real_prepared_and_review_clones_before_cleanup() {
    let (_f, mut jobs, r) = setup();
    let lease = isolated_archive_job(&mut jobs, &r, ID);
    let scratch = lease.scratch_path().unwrap();
    drop(lease);
    let review = jobs.review(ID, select(&jobs)).unwrap();
    assert_eq!(
        review["candidates"][0]["members"][0]["archive"]["decoderPath"],
        "folder/a.vpk"
    );
    assert_eq!(review["exclusions"][0]["relativePath"], "readme.txt");
    assert_eq!(review["exclusions"][0]["archive"]["index"], 1000);
    let retained = jobs.jobs[ID].plan.clone().unwrap();
    assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelRequested");
    assert!(scratch.exists());
    drop(retained);
    assert_eq!(jobs.status(Some(ID), false).unwrap()["phase"], "cancelled");
    assert!(!scratch.exists());
}
#[test]
fn archive_job_failed_cleanup_never_recovers_another_live_ready_quarantine() {
    let (_f, mut jobs, r) = setup();
    let a = isolated_archive_job(&mut jobs, &r, ID);
    let scratch_a = a.scratch_path().unwrap();
    drop(a);
    let other = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let b = isolated_archive_job(&mut jobs, &r, other);
    let scratch_b = b.scratch_path().unwrap();
    drop(b);
    std::fs::write(scratch_a.join("unknown"), b"keep").unwrap();
    assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelRequested");
    assert!(scratch_b.exists());
    assert_eq!(jobs.status(Some(other), true).unwrap()["phase"], "ready");
    assert!(scratch_a.join("unknown").exists());
    assert!(scratch_b.exists());
}
#[test]
fn archive_job_success_outcome_survives_private_cleanup_refusal() {
    let (f, mut jobs, r) = setup();
    ready(&mut jobs, r);
    // Transport ownership is independent from package publication. Use the
    // accepted raw writer here; archive-original publication belongs to Task 3.
    let lease =
        QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&jobs.db), ID).unwrap();
    let scratch = lease.scratch_path().unwrap();
    std::fs::write(scratch.join("unknown"), b"keep").unwrap();
    jobs.leases.insert(ID.into(), lease);
    let dto = jobs.review(ID, select(&jobs)).unwrap();
    let lock = RootLock::acquire(&f.data).unwrap();
    let plan = jobs
        .reserve_commit(
            &lock,
            ID,
            dto["token"].as_str().unwrap(),
            "default",
            1,
            "private-cleanup",
        )
        .unwrap();
    let outcome = crate::local_import::commit_local(
        &plan,
        &lock,
        1,
        "private-cleanup",
        &object! {},
        &mut |_| Ok(()),
    );
    assert!(outcome.is_ok());
    drop(plan);
    jobs.finish_commit(ID, &outcome).unwrap();
    drop(lock);
    jobs.after_writer_cleanup().unwrap();
    let status = jobs.status(Some(ID), false).unwrap();
    assert_eq!(status["phase"], "succeeded");
    assert_eq!(status["cleanupError"]["kind"], "recoveryRequired");
    assert_eq!(
        Store::open_history(&f.data)
            .unwrap()
            .operation("private-cleanup")
            .unwrap()["status"],
        "succeeded"
    );
    assert!(scratch.join("unknown").exists());
}
#[test]
#[ignore = "host qualification: guarded archive preparation requires the normal host UID namespace and root-owned executable ancestors"]
fn host_archive_job_prepares_reviews_revalidates_and_discards_zip() {
    let (f, mut jobs, mut r) = setup();
    let archive = f.root.join("source.zip");
    let output=std::process::Command::new("python3").args(["-c","import sys,zipfile; z=zipfile.ZipFile(sys.argv[1],'w'); z.write(sys.argv[2],'folder/a.vpk'); z.writestr('readme.txt','excluded'); z.close()"])
            .arg(&archive).arg(&r.roots[0]).output().unwrap();
    assert!(output.status.success());
    let before = std::fs::read(&archive).unwrap();
    r.roots = vec![archive.clone()];
    ready(&mut jobs, r);
    let dto = jobs.review(ID, select(&jobs)).unwrap();
    assert_eq!(dto["candidates"].len(), 1);
    assert_eq!(
        dto["candidates"][0]["members"][0]["sourcePath"],
        archive.to_str().unwrap()
    );
    assert_eq!(dto["exclusions"][0]["relativePath"], "readme.txt");
    let prepared = jobs.jobs[ID].prepared.as_ref().unwrap();
    local_content::revalidate_local(prepared, &PrepareControl::new()).unwrap();
    let scratch = prepared.archive_sources()[0]
        .lease()
        .scratch_path()
        .unwrap();
    assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelled");
    assert!(!scratch.exists());
    assert_eq!(std::fs::read(&archive).unwrap(), before);
}

#[test]
fn archive_job_new_preparation_refuses_unknown_claim_created_during_live_session() {
    let (f, mut jobs, r) = setup();
    let lease = isolated_archive_job(&mut jobs, &r, ID);
    let scratch = lease.scratch_path().unwrap();
    drop(lease);
    let unknown = f.data.join(".local-quarantine-unknown-live");
    std::fs::create_dir(&unknown).unwrap();
    let result = jobs.start(r, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert!(
        result.is_err(),
        "new preparation must preflight claims introduced since session startup"
    );
    assert_eq!(result.unwrap_err().kind, "recoveryRequired");
    assert!(scratch.exists());
    assert!(unknown.exists());
}

#[test]
fn archive_job_pending_registry_survives_failed_intent_write() {
    let (_f, mut jobs, r) = setup();
    ready(&mut jobs, r);
    let lease =
        QuarantineLease::create(&QuarantineAuthority::for_held_jobs_store(&jobs.db), ID).unwrap();
    let scratch = lease.scratch_path().unwrap();
    drop(lease); // durable factory ownership, with no surviving worker lease
    jobs.cleanup_targets.insert(
        ID.into(),
        fail_body(
            jobs.jobs[ID].body.clone(),
            "failed",
            Some(&error("nativeIo", "factory stopped")),
        ),
    );
    jobs.pending_registry = true;
    jobs.db.fail_writes = true;
    assert!(jobs.after_writer_cleanup().is_err());
    assert!(
        scratch.exists(),
        "a failed intent write must not consume pending registry ownership"
    );
    jobs.db.fail_writes = false;
    jobs.after_writer_cleanup().unwrap();
    assert!(!scratch.exists());
    assert_eq!(jobs.jobs[ID].body["phase"], "failed");
}

#[test]
fn archive_job_finishing_pending_cancel_preserves_newer_observed_revision() {
    let (_f, mut jobs, r) = setup();
    let lease = isolated_archive_job(&mut jobs, &r, ID);
    assert_eq!(jobs.cancel(ID).unwrap()["phase"], "cancelRequested");
    jobs.note_revision(2);
    assert_eq!(
        jobs.status(Some(ID), false).unwrap()["observedRevision"],
        "2"
    );
    drop(lease);
    let status = jobs.status(Some(ID), false).unwrap();
    assert_eq!(status["phase"], "cancelled");
    assert_eq!(status["observedRevision"], "2");
}

#[test]
#[ignore = "item103 disposable host performance qualification; set ITEM103_ARCHIVE"]
fn item103_stored_archive_timing() {
    let (_f, mut jobs, mut request) = setup();
    request.roots = vec![std::env::var_os("ITEM103_ARCHIVE").expect("ITEM103_ARCHIVE").into()];
    let start = Instant::now();
    jobs.start(request, ID).unwrap();
    let mut previous = String::new();
    loop {
        let status = jobs.status(Some(ID), false).unwrap();
        let phase = status["phase"].as_str().unwrap().to_owned();
        if phase != previous { eprintln!("item103 phase={phase} at_ms={} bytes={}/{}",start.elapsed().as_millis(),status["bytesDone"],status["bytesTotal"]); previous=phase.clone(); }
        if phase=="ready" {break;}
        assert!(!["failed","interrupted"].contains(&phase.as_str()),"{status}");
        assert!(start.elapsed().as_secs()<600);
        thread::sleep(Duration::from_millis(2));
    }
    let at=Instant::now(); let selection=select(&jobs); jobs.review(ID,selection).unwrap();
    eprintln!("item103 review_ms={}",at.elapsed().as_millis());
    let at=Instant::now(); jobs.cancel(ID).unwrap();
    eprintln!("item103 cleanup_ms={} total_ms={}",at.elapsed().as_millis(),start.elapsed().as_millis());
}
