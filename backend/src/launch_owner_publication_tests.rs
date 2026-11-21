//! Publication barriers exercise the real worker. PATH is set only in a child
//! process environment; the test runner's process-wide environment is untouched.
use super::*;
use crate::test_support::{Fixture,import,vpk};
use std::{fs,os::unix::fs::PermissionsExt,process::Command};

#[test]
fn prepared_publication_follows_handoff_commits(){run(false);}
#[test]
fn cancellation_before_publication_still_stops_dispatch(){run(true);}

fn call(owner:&mut LaunchOwner,method:&str,mut p:JsonValue)->JsonValue{
    p["dataDir"]=owner.root.to_str().unwrap().into();owner.dispatch(method,&p).unwrap()
}
fn control(owner:&mut LaunchOwner,method:&str,mut p:JsonValue)->JsonValue{
    let id=paths::random_id().unwrap();p["controlId"]=id.clone().into();call(owner,method,p);
    let deadline=Instant::now()+Duration::from_secs(10);
    loop{let r=call(owner,"native.launch.control",object!{controlId:id.clone()});
        if r["state"]=="succeeded"{return r["value"].clone();}
        assert_eq!(r["state"],"pending","{}",r.dump());assert!(Instant::now()<deadline);thread::sleep(Duration::from_millis(5));}
}
fn status(owner:&mut LaunchOwner,epoch:&JsonValue,attempt:&str)->JsonValue{
    call(owner,"native.launch.status",object!{epoch:epoch.clone(),attemptId:attempt})
}
fn wait_phase(owner:&mut LaunchOwner,epoch:&JsonValue,attempt:&str,phase:&str)->JsonValue{
    let deadline=Instant::now()+Duration::from_secs(10);
    loop{let v=status(owner,epoch,attempt);if v["phase"]==phase{return v;}
        assert!(Instant::now()<deadline,"waiting for {phase}: {}",v.dump());thread::sleep(Duration::from_millis(5));}
}
fn run(cancel:bool){
    let name=if cancel{"cancellation_before_publication_still_stops_dispatch"}else{"prepared_publication_follows_handoff_commits"};
    if std::env::var("ITEM114_PUBLICATION_CHILD").as_deref()!=Ok(name){
        let dir=std::env::temp_dir().join(format!("publication-child-{}",paths::random_id().unwrap()));fs::create_dir(&dir).unwrap();
        let steam=dir.join("steam");let spawned=dir.join("spawned");
        fs::write(&steam,format!("#!/bin/sh\n/usr/bin/touch '{}'\n",spawned.display())).unwrap();
        fs::set_permissions(&steam,fs::Permissions::from_mode(0o700)).unwrap();
        let output=Command::new(std::env::current_exe().unwrap())
            .args(["--exact",&format!("launch_owner::publication_tests::{name}"),"--nocapture","--test-threads=1"])
            .env("ITEM114_PUBLICATION_CHILD",name).env("ITEM114_SPAWN_LOG",&spawned)
            .env("PATH",format!("{}:{}",dir.display(),std::env::var("PATH").unwrap()))
            .output().unwrap();
        fs::remove_dir_all(dir).unwrap();
        assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));return;
    }
    let f=Fixture::new();let spawned=PathBuf::from(std::env::var_os("ITEM114_SPAWN_LOG").unwrap());
    fs::write(f.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel/addons\n Game citadel\n }\n }\n}\n").unwrap();
    let resource=f.game.join("game/core/modlock_assets/pak01_dir.vpk");fs::create_dir_all(resource.parent().unwrap()).unwrap();fs::write(resource,vpk("scripts/core.vdata_c",0)).unwrap();
    let mut service=crate::native::NativeService::default();import(&mut service,&f);
    let preview=service.dispatch("native.applyPreview",&object!{dataDir:f.data.to_str(),profileId:"default",gamePath:f.game.to_str(),deploymentLayout:"shards-v1"}).unwrap();
    service.dispatch("native.apply",&object!{dataDir:f.data.to_str(),operationId:"initial-apply",expectedRevision:"1",token:preview["token"].clone(),deploymentLayout:"shards-v1"}).unwrap();drop(service);
    let pool=ChildPool::new().unwrap();let mut owner=LaunchOwner::new(f.data.clone(),pool.clone(),Arc::new(Mutex::new(launch_process::Monitor::default()))).unwrap();
    let session=control(&mut owner,"native.launch.open",object!{openId:paths::random_id().unwrap()});let epoch=session["epoch"].clone();
    let settings=Store::open_history(&f.data).unwrap().launch_settings().unwrap();let attempt=paths::random_id().unwrap();
    control(&mut owner,"native.launch.begin",object!{epoch:epoch.clone(),attemptId:attempt.clone(),profileId:"default",mode:"modded",expectedRevision:"2",settingsRevision:settings["revision"].clone()});
    let ready=wait_phase(&mut owner,&epoch,&attempt,"ready")["attempt"]["ready"].clone();
    let (point_tx,point_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
    *owner.shared.publication_probe.lock().unwrap()=Some(Box::new(move|point,store|{
        if matches!(point,PublicationPoint::BeforeHandoffWrites){store.launch_database().execute("BEGIN EXCLUSIVE",&[]).unwrap();}
        point_tx.send(point).unwrap();release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        if matches!(point,PublicationPoint::BeforeHandoffWrites){store.launch_database().execute("ROLLBACK",&[]).unwrap();}
    }));
    let p=object!{epoch:epoch.clone(),attemptId:attempt.clone(),controlId:paths::random_id().unwrap(),reviewToken:ready["reviewToken"].clone(),preparationOperationId:ready["preparationOperationId"].clone(),preparationInput:ready["preparationInput"].clone(),settingsRevision:settings["revision"].clone()};
    call(&mut owner,"native.launch.commit",p);
    assert!(matches!(point_rx.recv_timeout(Duration::from_secs(10)).unwrap(),PublicationPoint::BeforeHandoffWrites));
    let while_writing=status(&mut owner,&epoch,&attempt);
    let no_early_outcome=while_writing["attempt"]["preparation"].is_null() && while_writing["preparationState"]!="prepared";
    // Actual contention exists. The GUI must not be told to issue this read yet.
    let busy=Store::open_history(&f.data).unwrap().operation(ready["preparationOperationId"].as_str().unwrap()).unwrap_err();
    assert_eq!(busy.kind,"nativeBusy");
    if cancel{let c=call(&mut owner,"native.launch.cancel",object!{epoch:epoch.clone(),attemptId:attempt.clone()});assert_eq!(c["status"],"requested");assert!(owner.shared.cancelled());}
    release_tx.send(()).unwrap();
    assert!(matches!(point_rx.recv_timeout(Duration::from_secs(10)).unwrap(),PublicationPoint::BeforePublish));
    let before_publish=status(&mut owner,&epoch,&attempt);
    let publication_delayed=before_publish["attempt"]["preparation"].is_null();
    // All handoff writes have returned. The outcome is committed and readable
    // before the cache is allowed to announce it, without sleeps or thresholds.
    let outcome=Store::open_history(&f.data).unwrap().operation(ready["preparationOperationId"].as_str().unwrap()).unwrap();
    release_tx.send(()).unwrap();
    let published=wait_phase(&mut owner,&epoch,&attempt,if cancel{"cancelled"}else{"waitingForGame"});
    let visible_outcome=Store::open_history(&f.data).unwrap().operation(ready["preparationOperationId"].as_str().unwrap()).unwrap();
    if cancel{
        assert!(published["attempt"]["handoff"].is_null());
        assert_eq!(published["attempt"]["outcome"]["reason"],"beforeDispatch");
        assert!(!spawned.exists(),"Cancel before handoff must suppress spawn");
    }
    drop(owner);pool.shutdown();
    assert!(no_early_outcome,"prepared attempt was observable while handoff writes still follow");
    assert!(publication_delayed,"outcome was published before the final handoff commit boundary");
    assert_eq!(outcome["status"],"succeeded");assert_eq!(visible_outcome["status"],"succeeded");assert_eq!(published["preparationState"],"prepared");
}
