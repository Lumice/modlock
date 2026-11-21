use super::*;
use crate::archive_content::test_support::Fixture;

#[test]
fn item93_legacy_quarantine_recovers_changed_device_with_locked_root() {
    let fixture=Fixture::new();
    let store=fixture.store();
    let authority=QuarantineAuthority::for_held_jobs_store(&store);
    let lease=QuarantineLease::create(&authority,"reboot").unwrap();
    if !paths::test_volatile_filesystem(&lease.root_fd) {return;}
    let claim=lease.open_claim(lease.allocate(1,&PrepareControl::new()).unwrap().remove(0)).unwrap();
    drop(claim);
    let scratch=lease.scratch_path().unwrap();let registry=lease.registry_path().unwrap();
    drop(lease);
    let mut value=json::parse(&fs::read_to_string(&registry).unwrap()).unwrap();
    value.remove("rootFsid");
    for key in ["rootIdentity","scratchIdentity"] {let device=value[key]["device"].as_u64().unwrap();value[key]["device"]=(device+1).into();}
    for claim in value["claims"].members_mut() {let id=&mut claim["identity"];if id.is_object(){let device=id["device"].as_u64().unwrap();id["device"]=(device+1).into();}}
    fs::write(&registry,value.dump()).unwrap();
    assert_eq!(recover_quarantines(&authority).unwrap().len(),1);
    assert!(!scratch.exists());
}

#[test]
fn item93_quarantine_rejects_changed_inode_owner_and_filesystem() {
    for case in ["inode","owner","filesystem"] {
        let fixture=Fixture::new();let store=fixture.store();
        let authority=QuarantineAuthority::for_held_jobs_store(&store);
        let lease=QuarantineLease::create(&authority,"reboot-negative").unwrap();
        let scratch=lease.scratch_path().unwrap();let registry=lease.registry_path().unwrap();
        drop(lease);
        let mut value=json::parse(&fs::read_to_string(&registry).unwrap()).unwrap();
        match case {
            "inode"=>{let id=&mut value["claims"][0]["identity"];id["inode"]=(id["inode"].as_u64().unwrap()+1).into();},
            "owner"=>{let id=&mut value["claims"][0]["identity"];id["uid"]=(id["uid"].as_u64().unwrap()+1).into();},
            _=>value["rootFsid"]="0000000000000001".into(),
        }
        fs::write(&registry,value.dump()).unwrap();
        assert!(recover_quarantines(&authority).is_err(),"accepted changed {case}");
        assert!(scratch.join(OWNER).exists(),"deleted evidence for {case}");
    }
}

#[test]
fn item93_remote_quarantine_recovers_changed_device() {
    let fixture=Fixture::new();let _store=fixture.store();let root=fixture.root.join("native");
    let lease=QuarantineLease::create_in(&root,"remote-reboot",paths::random_id().unwrap(),Domain::Remote).unwrap();
    if !paths::test_volatile_filesystem(&lease.root_fd) {return;}
    let scratch=lease.scratch_path().unwrap();let registry=lease.registry_path().unwrap();drop(lease);
    let mut value=json::parse(&fs::read_to_string(&registry).unwrap()).unwrap();
    assert!(value["rootFsid"].is_string());
    for key in ["rootIdentity","scratchIdentity"] {let device=value[key]["device"].as_u64().unwrap();value[key]["device"]=(device+1).into();}
    for claim in value["claims"].members_mut() {let id=&mut claim["identity"];if id.is_object(){let device=id["device"].as_u64().unwrap();id["device"]=(device+1).into();}}
    fs::write(&registry,value.dump()).unwrap();
    let (root_fd,mut states)=preflight_quarantines(&root,Domain::Remote,false).unwrap();
    assert_eq!(states.len(),1);cleanup_registry(&root_fd,&mut states[0]).unwrap();
    assert!(!scratch.exists());
}

#[test]
fn cleanup_requires_exclusive_ownership_and_removes_only_owned_files() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let authority = QuarantineAuthority::for_held_jobs_store(&store);
    let lease = QuarantineLease::create(&authority, "cleanup-test").unwrap();
    let scratch = lease.scratch_path().unwrap();
    let registry = lease.registry_path().unwrap();
    assert_eq!(fs::metadata(&scratch).unwrap().mode() & 0o777, 0o700);
    let mut output = lease
        .open_claim(lease.allocate(1, &PrepareControl::new()).unwrap().remove(0))
        .unwrap();
    assert_eq!(output.file.metadata().unwrap().mode() & 0o777, 0o600);
    output.file.write_all(b"owned bytes").unwrap();
    output.file.sync_all().unwrap();
    lease.complete_batch(&[output.name]).unwrap();
    drop(output.file);
    let sentinel = fixture.root.join("native/unrelated");
    fs::write(&sentinel, b"keep").unwrap();
    assert_eq!(
        lease.clone().cleanup(&authority).unwrap_err().kind,
        "nativeBusy"
    );
    assert!(scratch.exists());
    assert!(registry.exists());
    lease.cleanup(&authority).unwrap();
    assert!(!scratch.exists());
    assert!(!registry.exists());
    assert_eq!(fs::read(sentinel).unwrap(), b"keep");
}

#[test]
fn restart_preflights_every_lease_before_deleting_any_files() {
    for ambiguity in [
        "unknown",
        "replacement",
        "symlink",
        "hardlink",
        "missing",
        "next",
    ] {
        let fixture = Fixture::new();
        let store = fixture.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&store);
        let good = QuarantineLease::create(&authority, "good").unwrap();
        let good_path = good.scratch_path().unwrap();
        let bad = QuarantineLease::create(&authority, "bad").unwrap();
        let bad_path = bad.scratch_path().unwrap();
        let output = bad
            .open_claim(bad.allocate(1, &PrepareControl::new()).unwrap().remove(0))
            .unwrap();
        let path = output.path.clone();
        drop(output);
        match ambiguity {
            "unknown" => fs::write(bad_path.join("unknown"), b"keep").unwrap(),
            "replacement" => {
                fs::rename(&path, fixture.root.join("original-inode")).unwrap();
                fs::write(&path, b"replacement").unwrap();
            }
            "symlink" => {
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(fixture.root.join("outside"), &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, fixture.root.join("extra-link")).unwrap(),
            "missing" => fs::remove_file(&path).unwrap(),
            "next" => fs::write(
                fixture.root.join("native").join(format!(
                    "{}.next",
                    bad.registry_path()
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                )),
                b"incomplete replacement",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        drop(good);
        drop(bad);
        assert_eq!(
            recover_quarantines(&authority).unwrap_err().kind,
            "recoveryRequired",
            "{ambiguity}"
        );
        assert!(
            good_path.join(OWNER).exists(),
            "deleted good lease for {ambiguity}"
        );
        assert!(
            bad_path.join(OWNER).exists(),
            "deleted bad owner for {ambiguity}"
        );
    }
}

#[test]
fn restart_resumes_durable_cleanup_but_preserves_created_identity_gaps() {
    for case in [
        "ordinary",
        "partial-cleanup",
        "scratch-removed",
        "absent-intent",
        "present-gap",
    ] {
        let fixture = Fixture::new();
        let store = fixture.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&store);
        let lease = QuarantineLease::create(&authority, "restart").unwrap();
        let scratch = lease.scratch_path().unwrap();
        let registry = lease.registry_path().unwrap();
        let output = lease
            .open_claim(lease.allocate(1, &PrepareControl::new()).unwrap().remove(0))
            .unwrap();
        let path = output.path;
        drop(output.file);
        {
            let mut state = lease.state.lock().unwrap();
            match case {
                "partial-cleanup" | "scratch-removed" => {
                    state.cleanup_started = true;
                    persist(&lease.root_fd, &mut state).unwrap();
                    fs::remove_file(&path).unwrap();
                    if case == "scratch-removed" {
                        fs::remove_file(scratch.join(OWNER)).unwrap();
                        fs::remove_dir(&scratch).unwrap();
                    }
                }
                "absent-intent" | "present-gap" => {
                    state.claims.last_mut().unwrap().identity = None;
                    persist(&lease.root_fd, &mut state).unwrap();
                    if case == "absent-intent" {
                        fs::remove_file(&path).unwrap();
                    }
                }
                _ => {}
            }
        }
        drop(lease);
        if case == "present-gap" {
            assert_eq!(
                recover_quarantines(&authority).unwrap_err().kind,
                "recoveryRequired"
            );
            assert!(path.exists());
            assert!(registry.exists());
        } else {
            let recovered = recover_quarantines(&authority).unwrap();
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].job_id, "restart");
            assert!(!scratch.exists());
            assert!(!registry.exists());
        }
    }
}

#[test]
fn cancelled_batch_can_be_cleaned_by_live_owner() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let authority = QuarantineAuthority::for_held_jobs_store(&store);
    let lease = QuarantineLease::create(&authority, "cancelled").unwrap();
    let scratch = lease.scratch_path().unwrap();
    let watch = scratch.clone();
    let control = PrepareControl::new();
    let cancel = control.clone();
    control.check_hook(Arc::new(move || {
        if watch.join("file-00000001").exists() {
            cancel.request_cancel();
        }
    }));
    assert_eq!(
        lease.allocate(3, &control).err().unwrap().kind,
        "localPreparationCancelled"
    );
    control.check_hook(Arc::new(|| {}));
    assert!(scratch.join("file-00000001").exists());
    assert!(!scratch.join("file-00000002").exists());
    lease.cleanup(&authority).unwrap();
    assert!(!scratch.exists());
}

#[test]
fn restart_accepts_absent_scratch_intent_but_refuses_present_unowned_scratch() {
    for present in [false, true] {
        let fixture = Fixture::new();
        let store = fixture.store();
        let authority = QuarantineAuthority::for_held_jobs_store(&store);
        let lease = QuarantineLease::create(&authority, "planned").unwrap();
        let scratch = lease.scratch_path().unwrap();
        fs::remove_file(scratch.join(OWNER)).unwrap();
        if !present {
            fs::remove_dir(&scratch).unwrap();
        }
        {
            let mut state = lease.state.lock().unwrap();
            state.scratch_identity = None;
            state.claims.clear();
            persist(&lease.root_fd, &mut state).unwrap();
        }
        drop(lease);
        if present {
            assert_eq!(
                recover_quarantines(&authority).unwrap_err().kind,
                "recoveryRequired"
            );
            assert!(scratch.exists());
        } else {
            assert_eq!(recover_quarantines(&authority).unwrap().len(), 1);
        }
    }
}

#[test]
fn live_cleanup_preserves_replaced_scratch_and_original_directory() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let authority = QuarantineAuthority::for_held_jobs_store(&store);
    let lease = QuarantineLease::create(&authority, "replacement").unwrap();
    let scratch = lease.scratch_path().unwrap();
    let moved = fixture.root.join("original-scratch");
    fs::rename(&scratch, &moved).unwrap();
    fs::create_dir(&scratch).unwrap();
    fs::write(scratch.join("keep"), b"replacement").unwrap();
    assert_eq!(
        lease.cleanup(&authority).unwrap_err().kind,
        "recoveryRequired"
    );
    assert_eq!(fs::read(scratch.join("keep")).unwrap(), b"replacement");
    assert!(moved.join(OWNER).exists());
}

#[test]
fn inspect_complete_set_is_read_only_and_preserves_live_claims() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let authority = QuarantineAuthority::for_held_jobs_store(&store);
    let lease = QuarantineLease::create(&authority, "inspect-live").unwrap();
    let registry = lease.registry_path().unwrap();
    let scratch = lease.scratch_path().unwrap();
    let before = fs::read(&registry).unwrap();
    let claims = inspect_quarantines(&authority).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].job_id, "inspect-live");
    assert_eq!(claims[0].nonce, lease.nonce().unwrap());
    fs::write(scratch.join("unknown"), b"preserve").unwrap();
    assert!(inspect_quarantines(&authority).is_err());
    assert_eq!(fs::read(&registry).unwrap(), before);
    assert_eq!(fs::read(scratch.join("unknown")).unwrap(), b"preserve");
}
