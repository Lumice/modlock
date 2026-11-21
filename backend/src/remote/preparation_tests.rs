use super::*;
use std::os::unix::fs::PermissionsExt;

// Parent-process logical reads (rchar), including pipe reads. Decoder input
// and emitted bytes are reported separately by archive_process instrumentation.
fn read_chars()->u64 { fs::read_to_string("/proc/self/io").unwrap().lines().find_map(|l|l.strip_prefix("rchar: ").and_then(|v|v.trim().parse().ok())).unwrap() }
fn report(phase:&str,at:Instant,reads:u64){eprintln!("item103 phase={phase} ms={} parent_read_bytes={}",at.elapsed().as_millis(),read_chars()-reads);}
fn fixture() -> (crate::test_support::Fixture, PreparedRemote) {
    let f=crate::test_support::Fixture::new();
    let mut service=crate::native::NativeService::new();crate::test_support::import(&mut service,&f);
    let lease=Arc::new(super::super::job_store::RemoteStoreLease::acquire(&f.data).unwrap());
    let slot=super::super::preparation_slot::PreparationSlot::try_acquire(&f.data).unwrap();
    let job=contract::JobId::parse(&"a".repeat(32)).unwrap();let nonce="b".repeat(32);
    let authority=RemoteQuarantineAuthority::new(lease.clone(),job.clone(),contract::ClaimId::parse(&nonce).unwrap()).unwrap();
    let dir=lease.bulk_path().join(format!(".remote-claim-{}","c".repeat(32)));
    fs::create_dir(&dir).unwrap();fs::set_permissions(&dir,fs::Permissions::from_mode(0o700)).unwrap();
    let input=std::env::var_os("ITEM103_REMOTE_ARCHIVE");
    if let Some(path)=&input {fs::copy(path,dir.join("data")).unwrap();}
    else {fs::write(dir.join("data"),crate::test_support::vpk("models/item103.vmdl_c",12)).unwrap();}
    fs::write(dir.join("lease"),b"").unwrap();
    for name in ["data","lease"]{fs::set_permissions(dir.join(name),fs::Permissions::from_mode(0o600)).unwrap();}
    let reads=read_chars();let at=Instant::now();let fp=paths::fingerprint(&dir.join("data")).unwrap();
    report("claim_seal_hash",at,reads);eprintln!("item103 claim_bytes={}",fp.size);
    let file=contract::ConsentFile{file_id:"1".into(),original_name:if input.is_some(){"sakuya.zip"}else{"item103.vpk"}.into(),declared_size:Some(fp.size.to_string()),created_at:None,md5:None,list:"current".into(),archive_flag:Some(false)};
    let submission=contract::SubmissionKeyV1::parse(&object!{provider:"gamebanana",submissionType:"mod",submissionId:"42"}).unwrap();
    let dm=fs::metadata(&dir).unwrap();let lm=fs::metadata(dir.join("lease")).unwrap();let m=fs::metadata(dir.join("data")).unwrap();
    let member=contract::SealedRemoteMember{file_key:contract::ProviderFileKeyV1{submission:submission.clone(),file_id:"1".into()},receipt:contract::MemberReceiptV1{file:file.clone(),measured_size:fp.size.to_string(),sha256:fp.sha256,provider_checksum:"absent".into()},read_fd:File::open(dir.join("data")).unwrap().into(),claim_lease:File::open(dir.join("lease")).unwrap().into(),claim_directory:File::open(&dir).unwrap().into(),claim_id:contract::ClaimId::parse(&"c".repeat(32)).unwrap(),claim_identity:contract::ClaimIdentity{root_device:lease.identity().0,root_inode:lease.identity().1,directory_device:dm.dev(),directory_inode:dm.ino(),data_device:m.dev(),data_inode:m.ino(),lease_device:lm.dev(),lease_inode:lm.ino(),uid:m.uid(),link_count:m.nlink()}};
    let source=SealedRemoteSourceV1{job_id:job,submission:submission.clone(),root_file_id:"1".into(),consent_fingerprint:"d".repeat(64),cohort_fingerprint:contract::cohort_fingerprint(&submission,"1",&[file]).unwrap(),members:vec![member]};
    let context=PrepareContext{root:f.data.clone(),session:SessionId::parse(&"e".repeat(32)).unwrap(),prepare_key:PrepareKey::parse(&nonce).unwrap(),profile_id:"default".into(),expected_revision:1,expires_at:Instant::now()+std::time::Duration::from_secs(600),status_template:object!{version:1,root:f.data.to_str(),sessionId:"e".repeat(32),jobId:source.job_id.as_str(),submission:source.submission.json(),profileId:"default",profileNameAtStart:"Default",initialRevision:"1",reviewRevision:"1",sequence:"1",state:"ready",progress:JsonValue::Null,cohort:object!{fingerprint:source.cohort_fingerprint.clone(),complete:true,members:json::array![object!{fileId:"1",originalName:source.members[0].receipt.file.original_name.clone(),state:"sealed",durableOffset:source.members[0].receipt.measured_size.clone(),total:source.members[0].receipt.measured_size.clone(),receipt:source.members[0].receipt.json()}]},prepareKey:nonce.clone(),review:JsonValue::Null,operationId:JsonValue::Null,lastControl:JsonValue::Null,error:JsonValue::Null,cleanupPending:true},replacement:None};
    let reads=read_chars();let at=Instant::now();let prepared=RemotePreparation::start(source,slot,authority,context,PrepareControl::new()).unwrap();
    report("preparation",at,reads);
    (f,prepared)
}

#[test]
fn item103_changed_verified_identity_never_rehashes_into_authority() {
    let (_f,prepared)=fixture();let control=PrepareControl::new();
    prepared.revalidate(&control).unwrap();
    let path=prepared.fingerprints.keys().next().unwrap();
    // Even an identical rewrite changes ctime/mtime. A review must expire,
    // rather than silently accepting new identity after a fresh content hash.
    let bytes=fs::read(path).unwrap();fs::write(path,bytes).unwrap();
    assert!(prepared.validate_live(&control).is_err());
}

#[test]
#[ignore = "disposable remote host qualification: set ITEM103_REMOTE_ARCHIVE"]
fn item103_remote_archive_timing() {
    let (f,prepared)=fixture();let prepared=Arc::new(prepared);
    let reads=read_chars();let at=Instant::now();prepared.revalidate(&PrepareControl::new()).unwrap();
    report("review_verification",at,reads);
    let reads=read_chars();let at=Instant::now();for _ in 0..5{prepared.validate_live(&PrepareControl::new()).unwrap();}
    report("identity_5x",at,reads);
    let reads=read_chars();let at=Instant::now();
    let plan=prepared.review(RemoteReviewV1{request_key:"f".repeat(32),candidate_ids:vec![prepared.candidates[0].id.0.clone()],metadata:object!{name:"Item 103 disposable",author:"",category:"",description:""},expires_at:Instant::now()+std::time::Duration::from_secs(600),control:PrepareControl::new()}).unwrap();
    report("review",at,reads);
    let input=contract::RemoteCommitInputV1::from_params(&object!{operationId:"item103-perf",jobId:prepared.source.job_id.as_str(),profileId:"default",expectedRevision:"1",consentFingerprint:prepared.source.consent_fingerprint.clone(),cohortFingerprint:prepared.source.cohort_fingerprint.clone(),selectionDigest:plan.selection_digest.clone()}).unwrap();
    paths::HASH_READS.with(|r|r.borrow_mut().clear());
    let reads=read_chars();let at=Instant::now();let lock=RootLock::acquire(&f.data).unwrap();
    super::super::publication::RemotePublisher::commit(&lock,&plan,&input).unwrap();drop(lock);
    report("publication",at,reads);
    assert!(Store::open(&f.data).unwrap().journal("item103-perf").unwrap().is_none());
    paths::HASH_READS.with(|r|assert_eq!(r.borrow().values().sum::<usize>(),plan.copies.len(),"each published file must be content-verified once, including cleanup: {:?}",r.borrow()));
    let authority=prepared.authority();let slot=prepared.slot();let quarantine=prepared.quarantine.clone();
    drop(plan);drop(prepared);let reads=read_chars();let at=Instant::now();
    crate::archive_quarantine::cleanup_remote(quarantine,&authority,&slot).unwrap();
    report("quarantine_cleanup",at,reads);
}

#[test]
fn item103_publication_verifies_each_file_once() { item103_remote_archive_timing(); }

#[test]
fn item115_small_prejournal_eio_records_and_replays_native_io(){
    let(f,prepared)=fixture();let prepared=Arc::new(prepared);
    let plan=prepared.review(RemoteReviewV1{request_key:"f".repeat(32),candidate_ids:vec![prepared.candidates[0].id.0.clone()],metadata:object!{name:"Diagnostic test",author:"",category:"",description:""},expires_at:Instant::now()+std::time::Duration::from_secs(600),control:PrepareControl::new()}).unwrap();
    let input=contract::RemoteCommitInputV1::from_params(&object!{operationId:"item115-eio",jobId:prepared.source.job_id.as_str(),profileId:"default",expectedRevision:"1",consentFingerprint:prepared.source.consent_fingerprint.clone(),cohortFingerprint:prepared.source.cohort_fingerprint.clone(),selectionDigest:plan.selection_digest.clone()}).unwrap();
    let lock=RootLock::acquire(&f.data).unwrap();
    super::super::publication::TEST_PRE_JOURNAL_EIO.with(|v|v.set(true));
    let e=super::super::publication::RemotePublisher::commit(&lock,&plan,&input).unwrap_err();assert_eq!(e.kind,"nativeIo");
    assert_eq!(e.message,"Cannot access native or source files; check paths, available storage and permissions.");
    let st=Store::open_writer(&lock).unwrap();assert!(st.journal("item115-eio").unwrap().is_none());assert_eq!(st.operation("item115-eio").unwrap()["error"]["kind"],"nativeIo");
    assert_eq!(super::super::publication::RemotePublisher::replay(&lock,&input).unwrap_err().kind,"nativeIo");
}
