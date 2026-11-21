mod support;
use modlock_helper::{native::NativeService,performance,store::{RootLock,Store}};
use support::*;
use std::fs;
use json::{object,JsonValue};
#[test]
fn preset_only_apply_clear_none_and_restore_exact_overlay() {
 let f=Fixture::new();let mut service=NativeService::new();
 let stock=include_str!("../data/perf-stock.gi");let upstream=performance::overlay(stock,"dmm-clean").unwrap();
 fs::write(f.game.join("game/citadel/gameinfo.gi"),&upstream).unwrap();import(&mut service,&f);
 {
  let lock=RootLock::acquire(&f.data).unwrap();let st=Store::open_writer(&lock).unwrap();let mut d=st.document().unwrap();
  for m in d["profiles"]["default"]["mods"].members_mut(){m["enabled"]=false.into();}
  st.commit(&lock,1,"disable",&object!{},d,None).unwrap();
 }
 let mut select=mutation(&f,"2","choose");select["presetId"]="optilock-fps".into();service.dispatch("native.performance.select",&select).unwrap();
 let apply=|s:&mut NativeService,revision:&str,id:&str|->JsonValue{
  let mut p=f.params();p["profileId"]="default".into();p["gamePath"]=f.game.to_str().unwrap().into();p["deploymentLayout"]="shards-v1".into();
  let review=s.dispatch("native.applyPreview", &support::current_layout(&p)).unwrap();assert_eq!(review["fileCount"],0);
  let mut p=mutation(&f,revision,id);p["token"]=review["token"].clone();p["deploymentLayout"]="shards-v1".into();s.dispatch("native.apply", &support::current_layout(&p)).unwrap()
 };
 apply(&mut service,"3","perf-only");let first=fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap();assert_ne!(first,upstream.as_bytes());
 {
  use modlock_helper::{game_access::GameLock,launch_preparation::PreparationPlan,prepare_control::PrepareControl};
  let lock=RootLock::acquire(&f.data).unwrap();let store=Store::open_writer(&lock).unwrap();let guard=GameLock::acquire(&f.game.join("game/citadel")).unwrap();
  let vanilla=PreparationPlan::review_vanilla(&store,&lock,&guard,&f.game,&PrepareControl::new(),&mut |_|{}).unwrap();
  assert!(matches!(vanilla,PreparationPlan::Verify(_)),"Vanilla preserves the performance overlay when no preset is requested");
 }
 let mut p=mutation(&f,"4","clear");p["presetId"]="".into();let snap=service.dispatch("native.performance.select",&p).unwrap();assert_eq!(snap["pendingChanges"],true);
 assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),first,"selection must wait for Apply");
 apply(&mut service,"5","clear-apply");let cleared=fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap();assert!(!cleared.contains("dmm-perf"),"explicit None removes the reviewed preset overlay");
 assert_eq!(service.dispatch("native.snapshot",&f.params()).unwrap()["pendingStatus"],"current");
 service.dispatch("native.restore",&mutation(&f,"6","restore-preset")).unwrap();assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),first);
}
#[test]
fn restore_returns_exact_external_dmm_bytes() {
 let f=Fixture::new();let mut s=NativeService::new();let before=performance::overlay(include_str!("../data/perf-stock.gi"),"dmm-clean").unwrap();
 fs::write(f.game.join("game/citadel/gameinfo.gi"),&before).unwrap();import(&mut s,&f);
 let mut p=f.params();p["profileId"]="default".into();p["gamePath"]=f.game.to_str().unwrap().into();let review=s.dispatch("native.applyPreview", &support::current_layout(&p)).unwrap();
 assert_eq!(review["performance"]["detected"]["presetId"],"dmm-clean");
 let mut p=mutation(&f,"1","apply");p["token"]=review["token"].clone();s.dispatch("native.apply", &support::current_layout(&p)).unwrap();s.dispatch("native.restore",&mutation(&f,"2","restore")).unwrap();
 assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),before.as_bytes());
}
#[test]
fn restore_returns_exact_old_revision_optimizationlock_bytes() {
 let f=Fixture::new();let mut s=NativeService::new();
 let before=performance::overlay(include_str!("../data/perf-stock.gi"),"optimizationlock").unwrap()
  .replace("rev=7c7c90cc25","rev=92bebec")
  .replace("\"r_farz\"\t\t\"6800\"","\"r_farz\"\t\t\"7000\"")
  .replace("\"r_mapextents\"\t\t\"6800\"","\"r_mapextents\"\t\t\"7000\"")
  .replace("\"snd_mixahead\"\t\t\"0.01\"","\"snd_mixahead\"\t\t\"0.05\"");
 assert_ne!(before,performance::overlay(include_str!("../data/perf-stock.gi"),"optimizationlock").unwrap());
 fs::write(f.game.join("game/citadel/gameinfo.gi"),&before).unwrap();import(&mut s,&f);
 let mut select=mutation(&f,"1","choose");select["presetId"]="optimizationlock".into();s.dispatch("native.performance.select",&select).unwrap();
 let mut p=f.params();p["profileId"]="default".into();p["gamePath"]=f.game.to_str().unwrap().into();
 let review=s.dispatch("native.applyPreview",&support::current_layout(&p)).unwrap();
 assert_eq!(review["performance"]["overlayAction"],"replace");
 assert_eq!(review["performance"]["detected"]["markerId"],"optimizationlock");
 assert_eq!(review["performance"]["detected"]["revision"],"92bebec");
 let mut p=mutation(&f,"2","apply");p["token"]=review["token"].clone();s.dispatch("native.apply",&support::current_layout(&p)).unwrap();
 let after=fs::read_to_string(f.game.join("game/citadel/gameinfo.gi")).unwrap();
 assert_ne!(after.as_bytes(),before.as_bytes());
 assert!(after.contains("rev=7c7c90cc25"));
 s.dispatch("native.restore",&mutation(&f,"3","restore")).unwrap();
 assert_eq!(fs::read(f.game.join("game/citadel/gameinfo.gi")).unwrap(),before.as_bytes());
}

