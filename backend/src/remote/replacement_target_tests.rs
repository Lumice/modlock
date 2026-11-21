use super::*;
use crate::remote::replacement_contract::ReplacementPurpose as Purpose;
fn legacy() -> JsonValue {
    object! {source:object!{id:"old-mod",remoteId:"historical",status:"downloaded",
    name:"Kept",author:"Author",category:"Skin",description:"Notes",installOrder:7},
    enabled:true,available:true,previewHidden:true,unavailableReason:JsonValue::Null,
    packages:json::array![object!{directory:object!{id:"a".repeat(32),sha256:"b".repeat(64),size:12,sourceName:"pak01_dir.vpk"},companions:json::array![]}]}
}
#[test]
fn legacy_link_eligibility_and_closed_provenance() {
    let row = legacy();
    let target = admit(row.clone(), 2, "default", "old-mod", 0).unwrap();
    crate::remote::replacement_contract::validate_result("native.remote.replaceTarget",&object!{version:1,root:"/tmp/link-test",sessionId:"a".repeat(32),requestKey:"b".repeat(32),phase:"ready",target:target.public.clone(),skip:object!{state:"none",entry:JsonValue::Null},error:JsonValue::Null}).unwrap();
    assert!(target.public["receipt"].is_null());
    for field in ["localImport", "nativeRemoteLink", "type", "origin"] {
        let mut bad = row.clone();
        bad["source"][field] = object! {unexpected:true};
        assert!(admit(bad, 2, "default", "old-mod", 0).is_err());
    }
    let mut bad = row.clone();
    bad["options"] = JsonValue::Null;
    assert!(admit(bad, 2, "default", "old-mod", 0).is_err());
    assert!(admit(row.clone(), 2, "default", "../unsafe", 0).is_err());
    let mut bad = row;
    bad["available"] = false.into();
    assert!(admit(bad, 2, "default", "old-mod", 0).is_err());
}
#[test]
fn link_publication_preserves_history_and_recovers_interruptions() {
    link_recovery(false, false, false);
}
#[test]
fn item48_old_remote_receipt_recovers_without_folder_change() {
    link_recovery(true, false, false);
}
#[test]
fn item68_append_preserves_variants_selection_and_recovers() {
    link_recovery(false, true, false);
}
#[test]
fn review_append_preserves_root_receipt() {
    link_recovery_case(false,true,false,false,false,true,false);
}
#[test]
fn review_append_shared_payloads_are_admitted() {
    link_recovery_case(false,true,false,true,false,true,false);
}
#[test]
fn review_page_link_retains_owned_artwork() {
    link_recovery_case(false,false,true,false,true,false,false);
}
#[test]
fn item70_page_link_update_preserves_identity_and_recovers() {
    link_recovery(false, false, true);
}
fn link_recovery(old_receipt:bool, append:bool, page_link:bool) {
    link_recovery_case(old_receipt,append,page_link,false,false,append,false);
}
#[test]
fn item93_legacy_remote_publication_recovery_survives_device_change() {
    link_recovery_case(false,false,true,false,false,false,true);
}
fn link_recovery_case(old_receipt:bool, append:bool, page_link:bool, shared:bool, artwork:bool, preserve_receipt:bool, reboot:bool) {
    use crate::remote::{
        publication, recovery, replacement_contract::ReplacementCommitInputV1, replacement_recovery,
    };
    use crate::store::{RootLock, Store};
    use std::{fs, os::unix::fs::MetadataExt};
    let linking = !append && !page_link;
    for version in if old_receipt || append || page_link {vec![4]} else {vec![2,3,4]} {
        for interruption in [
            None,
            Some("memberCopied"),
            Some("published"),
            Some("database"),
            Some("stageCreatedUnrecorded"),
        ] {
            if old_receipt && interruption!=Some("database") {continue;}
            if reboot && !matches!(interruption,Some("memberCopied"|"published"|"database")) {continue;}
            let fixture=crate::test_support::Fixture::new();
            let root = std::env::temp_dir().join(format!(
                "link-unit-{}-{}",
                std::process::id(),
                paths::random_id().unwrap()
            ));
            fs::create_dir(&root).unwrap();
            let lock = RootLock::acquire(&root).unwrap();
            let store = Store::open_writer(&lock).unwrap();
            let mut old = legacy();
            if page_link {
                old["source"]["kind"]="local".into();
                old["source"]["localImport"]=object!{version:1,profileId:"default"};
                old["source"]["link"]="https://gamebanana.com/mods/123".into();
                old["source"]["nativeRemoteLink"]=object!{version:2,association:"userSelected",submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"123"},receipt:JsonValue::Null};
                admit(old.clone(),2,"default","old-mod",1).unwrap();
            }
            let mut old_bytes = Vec::new();
            for n in [0x55aa1234u32, 1, 1] { old_bytes.extend(n.to_le_bytes()); }
            old_bytes.push(0);
            let companion=b"shared";
            let family=|path|crate::test_support::family_vpk(1,&[crate::test_support::FamilyEntry{path,crc:0,archive:0,offset:0,length:6,preload:&[]}],&[]);
            if shared {
                old_bytes=family("models/first.vmdl_c");
                old["packages"][0]["companions"]=json::array![object!{archiveIndex:0,file:object!{id:"4".repeat(32),sha256:paths::digest_bytes(companion).unwrap(),size:companion.len(),sourceName:"pak01_000.vpk"}}];
                let mut other=old["packages"][0].clone();
                other["directory"]["id"]="6".repeat(32).into();
                other["directory"]["sha256"]=paths::digest_bytes(&family("models/other.vmdl_c")).unwrap().into();
                other["directory"]["size"]=family("models/other.vmdl_c").len().into();
                old["packages"].push(other).unwrap();
            }
            if append {
                old["packages"][0]["directory"]["size"] = old_bytes.len().into();
                old["packages"][0]["directory"]["sha256"] = paths::digest_bytes(&old_bytes).unwrap().into();
                let catalog = option_sources::catalog_for_record(&old, 2).unwrap();
                old = replace_record(&old, catalog.encode(2).unwrap(), object!{version:1,submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"123"},rootFileId:"455",cohortFingerprint:"7".repeat(64)}, Purpose::Link);
            }
            if artwork {
                let bytes=b"retained artwork";
                fs::create_dir(root.join("artwork")).unwrap();
                fs::write(root.join(format!("artwork/{}.image","5".repeat(32))),bytes).unwrap();
                old["source"]["localImport"]["artwork"]=object!{id:"5".repeat(32),sha256:paths::digest_bytes(bytes).unwrap(),size:bytes.len(),sourceName:"preview.png"};
                old["source"]["localImport"]["artworkFormat"]="png".into();
            }
            let mut before = object! {storeVersion:version,revision:"0",sourcePath:"/historical/state.json",gamePath:"",activeProfileId:"default",
            profiles:object!{default:object!{id:"default",name:"Default",protected:true,mods:json::array![old.clone()]},other:object!{id:"other",name:"Other",mods:json::array![]}},
            previewPolicy:object!{},importArchive:object!{keep:true},importLedger:object!{sourcePayloads:json::array![]},applied:JsonValue::Null,previous:JsonValue::Null,canRestore:false,recoveryRequired:false};
            if version >= 3 {
                before["packageVersion"] = 2.into();
                before["fonts"] =
                    object! {version:1,installed:JsonValue::Null,previous:JsonValue::Null};
            }
            if old_receipt {
                before["gamePath"]=fixture.game.to_str().unwrap().into();
                fs::write(fixture.game.join("game/citadel/gameinfo.gi"),"GameInfo\n{\n FileSystem\n {\n SearchPaths\n {\n Game citadel\n }\n }\n}\n").unwrap();

            }
            store
                .commit(&lock, 0, "initialize", &object! {}, before, Some("default"))
                .unwrap();
            let before = store.document().unwrap();
            fs::create_dir(root.join("payloads")).unwrap();
            if shared {
                fs::write(root.join(format!("payloads/{}.vpk","6".repeat(32))),family("models/other.vmdl_c")).unwrap();
                fs::write(root.join(format!("payloads/{}.vpk","4".repeat(32))),companion).unwrap();
                assert_eq!(crate::local_import::record_files(&old,2).unwrap().len(),4);
            }
            fs::write(
                root.join(format!("payloads/{}.vpk", "a".repeat(32))),
                if append {old_bytes.as_slice()} else {b"old"},
            )
            .unwrap();
            // A real, self-contained VPK with an empty directory tree.
            let mut bytes = Vec::new();
            for n in [0x55aa1234u32, 1, 1] {
                bytes.extend(n.to_le_bytes());
            }
            bytes.push(0);
            let source = root.join("fresh.vpk");
            fs::write(&source, bytes).unwrap();
            let fp = paths::fingerprint(&source).unwrap();
            let fresh = package::FileRef {
                id: "c".repeat(32),
                sha256: fp.sha256.clone(),
                size: fp.size,
                source_name: "fresh_dir.vpk".into(),
            };
            let mut fresh_legacy = old.clone();
            fresh_legacy.remove("options");
            fresh_legacy["packages"] = legacy()["packages"].clone();
            fresh_legacy["packages"][0]["directory"] = package::encode_file(&fresh);
            let mut catalog = option_sources::catalog_for_record(&fresh_legacy, 2).unwrap();
            let selected = JsonValue::from(catalog.selected_ids.clone());
            let receipt = object! {version:1,submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"123"},rootFileId:"456",cohortFingerprint:"d".repeat(64)};
            if append {
                let prior = option_sources::catalog_for_record(&old, 2).unwrap();
                let mut entries = prior.entries;
                entries.extend(catalog.entries);
                catalog.entries = entries;
                catalog.selected_ids = prior.selected_ids;
            }
            let record = replace_record(&old, catalog.encode(2).unwrap(), receipt.clone(), if append {Purpose::Append} else if linking {Purpose::Link} else {Purpose::Update});
            let admitted = admit(record.clone(), 2, "default", "old-mod", 1).unwrap();
            assert_eq!(admitted.root_file_id, if preserve_receipt {"455"} else {"456"});
            if append {assert_eq!(super::receipt(&record),super::receipt(&old));}
            assert!(admitted.submission.is_some());
            let mut history = record["source"].clone();
            history.remove("nativeRemoteLink");
            if append || page_link { history["nativeRemoteLink"] = old["source"]["nativeRemoteLink"].clone(); }
            assert_eq!(history, old["source"]);
            for k in ["enabled", "available", "previewHidden"] {
                assert_eq!(record[k], old[k]);
            }
            assert!(!record.has_key("packages"));
            assert!(!record.has_key("payloads"));
            let mut projected = before.clone();
            transition(&mut projected, 2).unwrap();
            projected["profiles"]["default"]["mods"][0] = record.clone();
            projected["revision"] = "2".into();
            let candidates = json::array!["e".repeat(64)];
            let mut input = object! {method:"native.remote.replaceCommit",params:object!{operationId:"link-op",jobId:"f".repeat(32),profileId:"default",modId:"old-mod",purpose:if append {"append"} else if page_link {"update"} else {"link"},initialRevision:"1",expectedRevision:"1",targetDigest:option_sources::record_digest(&old).unwrap(),afterRecordDigest:admitted.digest,consentFingerprint:"e".repeat(64),cohortFingerprint:"d".repeat(64),selectionDigest:"f".repeat(64)}};
            input["params"]["selectionDigest"] =
                crate::remote::replacement_contract::selection_digest(
                    &ReplacementCommitInputV1::parse(input.clone()).unwrap(),
                    &candidates,
                    &selected,
                )
                .unwrap()
                .into();
            let stage_name = format!(".import-remote-replacement-{}", "9".repeat(32));
            let meta = fs::metadata(&root).unwrap();
            let mut journal = object! {kind:if linking {"remotePayloadLink"} else {"remotePayloadReplace"},version:1,action:if linking {"linkRemotePayloadMod"} else {"replaceRemotePayloadMod"},mode:"rawVpk",operationId:"link-op",input:input.clone(),expectedRevision:"1",targetProfileId:"default",beforeDigest:paths::digest_bytes(before.dump().as_bytes()).unwrap(),afterDigest:paths::digest_bytes(projected.dump().as_bytes()).unwrap(),beforeDocument:before.clone(),libraryVersionBefore:version,libraryVersionAfter:4,packageVersion:2,modId:"old-mod",beforeRecord:old.clone(),record:record.clone(),candidateIds:candidates,optionIds:selected,stage:stage_name.clone(),owner:object!{kind:if linking {"remotePayloadLink"} else {"remotePayloadReplace"},version:1,action:if linking {"linkRemotePayloadMod"} else {"replaceRemotePayloadMod"},operationId:"link-op",stage:stage_name.clone(),nonce:"8".repeat(32),rootDevice:meta.dev().to_string(),rootInode:meta.ino().to_string(),authorityDigest:""},stageDevice:JsonValue::Null,stageInode:JsonValue::Null,ownerDevice:JsonValue::Null,ownerInode:JsonValue::Null,files:json::array![object!{id:fresh.id.clone(),sha256:fresh.sha256.clone(),size:fresh.size,sourceName:fresh.source_name.clone(),role:"payload",device:JsonValue::Null,inode:JsonValue::Null,complete:false}],reusedFiles:json::array![],cleanup:object!{mode:"none",cursor:"0",intent:JsonValue::Null}};
            journal["owner"]["authorityDigest"] =
                publication::authority_digest(&journal).unwrap().into();
            let recovered=replacement_recovery::admit(&store, &journal).unwrap();
            assert_eq!(recovered.projected,projected);
            assert_eq!(recovered.files.len(),1);
            assert!(recovered.files.contains_key(&fresh.id));
            if append {
                let mut bad=journal.clone();
                bad["record"]["source"]["nativeRemoteLink"]["receipt"]=receipt.clone();
                assert!(replacement_recovery::admit(&store,&bad).is_err());
            }
            if artwork {
                assert!(!recovered.files.contains_key(&"5".repeat(32)));
                let mut bad=journal.clone();bad["record"]["source"]["localImport"]["artwork"]["id"]="6".repeat(32).into();
                assert!(replacement_recovery::admit(&store,&bad).is_err());
            }
            let mut tampered = journal.clone();
            tampered["record"]["source"]["remoteId"] = "guessed".into();
            assert!(replacement_recovery::admit(&store, &tampered).is_err());
            if append {
                verify_retained(&root,&old,2).unwrap();
                let original=root.join(format!("payloads/{}.vpk","a".repeat(32)));
                fs::write(&original,b"changed").unwrap();
                assert!(verify_retained(&root,&old,2).is_err());
                fs::write(&original,&old_bytes).unwrap();
            }
            store.write_journal(&lock, "link-op", &journal).unwrap();
            let mut copies = crate::local_import::CopySet::new();
            copies.insert(
                fresh.id.clone(),
                (fresh.clone(), source, "payload".into(), fp),
            );
            let packages = crate::options::retained_packages(&record, 2).unwrap().into_iter()
                .filter(|p|!append || p.directory.id==fresh.id).collect();
            let result = crate::local_import::write_immutable_copies(
                &store,
                &lock,
                1,
                "link-op",
                &input,
                projected.clone(),
                "default",
                &root.join(stage_name),
                copies,
                packages,
                journal,
                &mut || Ok(()),
                &mut |at| {
                    if interruption == Some(at) {
                        Err(NativeError::new("interrupted", "Test interruption"))
                    } else {
                        Ok(())
                    }
                },
            );
            if let Some(point) = interruption {
                assert!(result.is_err());
                if reboot {
                    if !paths::test_volatile_filesystem(lock.directory()) {continue;}
                    let mut saved=store.journal("link-op").unwrap().unwrap();
                    let old_dev=fs::metadata(&root).unwrap().dev()+1;
                    saved["owner"]["rootDevice"]=old_dev.to_string().into();
                    for key in ["stageDevice","ownerDevice"] {if !saved[key].is_null(){saved[key]=old_dev.into();}}
                    for row in saved["files"].members_mut() {if !row["device"].is_null(){row["device"]=old_dev.into();}}
                    let owner=root.join(saved["stage"].as_str().unwrap()).join("owner.json");
                    fs::write(owner,saved["owner"].dump()).unwrap();
                    store.write_journal(&lock,"link-op",&saved).unwrap();
                }
                if old_receipt {
                    assert_eq!(store.operation("link-op").unwrap()["status"],"succeeded");
                    let mut old=store.document().unwrap();old.remove("gameFolderChanged");
                    let receipt=crate::store::snapshot_document(&old,Some("default")).unwrap();
                    store.launch_database().execute("UPDATE operations SET result=? WHERE id='link-op'",&[&object!{status:"succeeded",result:receipt}.dump()]).unwrap();
                }
                let recovery_input=crate::remote::contract::RemoteRecoveryInputV1::parse(object!{method:"native.recover",params:object!{operationId:"recover-link",expectedRevision:if point=="database" {"2"} else {"1"}}}).unwrap();
                let recovered = recovery::RemoteRecovery::recover(&lock, &recovery_input);
                if point == "stageCreatedUnrecorded" {
                    assert!(recovered.is_err());
                    assert!(store.journal("link-op").unwrap().is_some());
                    assert!(fs::read_dir(&root).unwrap().any(|e| e
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".import-remote-replacement-")));
                } else {
                    recovered.unwrap();
                    assert!(store.journals().unwrap().is_empty());
                    assert_eq!(Store::open(&root).unwrap().snapshot(None).unwrap()["recoveryRequired"],false);
                    assert_eq!(
                        store.document().unwrap(),
                        if point == "database" {
                            projected.clone()
                        } else {
                            before.clone()
                        }
                    );
                    assert_eq!(
                        root.join(format!("payloads/{}.vpk", fresh.id)).exists(),
                        point == "database"
                    );
                }
            } else {
                result.unwrap();
                recovery::cleanup_committed(
                    &store,
                    &lock,
                    &store.journal("link-op").unwrap().unwrap(),
                )
                .unwrap();
                assert_eq!(store.document().unwrap(), projected);
                if page_link {
                    let saved=store.document().unwrap();let row=&saved["profiles"]["default"]["mods"][0];
                    assert_eq!(row["source"]["id"],"old-mod");
                    assert_eq!(row["enabled"],true);
                    assert_eq!(super::receipt(row)["rootFileId"],"456");
                    assert_eq!(crate::options::for_record(row,2).unwrap().packages(true).unwrap()[0].directory.id,fresh.id);
                    assert_eq!(fs::read(root.join(format!("payloads/{}.vpk",fresh.id))).unwrap(),fs::read(root.join("fresh.vpk")).unwrap());
                }
                if append {
                    let saved = store.document().unwrap();
                    assert_eq!(saved["profiles"]["default"]["mods"].len(), 1);
                    let result = &saved["profiles"]["default"]["mods"][0];
                    assert_eq!(result["options"]["entries"].len(), if shared {3} else {2});
                    assert_eq!(result["options"]["entries"][0], old["options"]["entries"][0]);
                    assert_eq!(result["options"]["selectedIds"], old["options"]["selectedIds"]);
                    let mut changed = crate::options::for_record(result, 2).unwrap();
                    changed.selected_ids = vec![changed.entries.last().unwrap().id.clone()];
                    changed.validate_selection_save().unwrap();
                    assert_eq!(changed.packages(true).unwrap()[0].directory.id, fresh.id);
                }
                assert_eq!(
                    store.document().unwrap()["profiles"]["other"],
                    before["profiles"]["other"]
                );
                let target = read(&root, "default", "old-mod", 2).unwrap();
                assert_eq!(target.public["receipt"], if preserve_receipt {super::receipt(&old).clone()} else {receipt.clone()});
                let key = target.submission.as_ref().unwrap();
                for purpose in [
                    crate::remote::replacement_contract::ReplacementPurpose::Update,
                    crate::remote::replacement_contract::ReplacementPurpose::Reinstall,
                ] {
                    assert!(target.accepts_submission(purpose, key));
                }
                assert!(!target.accepts_submission(
                    crate::remote::replacement_contract::ReplacementPurpose::Link,
                    key
                ));
                let mut fresh_receipt = receipt.clone();
                fresh_receipt["rootFileId"] = "789".into();
                let updated = replace_record(
                    &record,
                    record["options"].clone(),
                    fresh_receipt.clone(),
                    Purpose::Update,
                );
                assert_eq!(
                    admit(updated.clone(), 2, "default", "old-mod", 3)
                        .unwrap()
                        .public["receipt"],
                    fresh_receipt
                );
                assert_eq!(
                    updated["source"]["nativeRemoteLink"]["submission"],
                    record["source"]["nativeRemoteLink"]["submission"]
                );
            }
            assert_eq!(
                fs::read(root.join(format!("payloads/{}.vpk", "a".repeat(32)))).unwrap(),
                if append {old_bytes.as_slice()} else {b"old"}
            );
            if append && interruption.is_none() {
                let saved=store.document().unwrap();
                let entry=saved["profiles"]["default"]["mods"][0]["options"]["entries"].members().last().unwrap();
                let chosen=entry["id"].as_str().unwrap().to_owned();
                let plan=crate::profile_edit::ProfileEditPlan::review(&store,"default",2,
                    crate::profile_edit::ProfileEditChange::Selection{mod_id:"old-mod".into(),option_ids:vec![chosen.clone()]}).unwrap();
                plan.commit(&store,&lock,2,"switch-variant",&object!{selection:chosen.clone()}).unwrap();
                let reopened=Store::open(&root).unwrap().document().unwrap();
                let record=&reopened["profiles"]["default"]["mods"][0];
                assert_eq!(record["options"]["entries"].len(),if shared {3} else {2});
                assert_eq!(record["options"]["selectedIds"],json::array![chosen]);
                assert_eq!(package::selected_packages_exact(record,2).unwrap()[0].directory.id,fresh.id);
            }
            if artwork {
                let saved=store.document().unwrap();
                assert_eq!(saved["profiles"]["default"]["mods"][0]["source"]["localImport"],old["source"]["localImport"]);
                assert_eq!(fs::read(root.join(format!("artwork/{}.image","5".repeat(32)))).unwrap(),b"retained artwork");
            }
            drop(store);
            drop(lock);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
#[test]
fn item68_append_selects_new_only_for_empty_selection_and_rejects_old_payload_aliases() {
    let row=legacy();
    let catalog=option_sources::catalog_for_record(&row,2).unwrap();
    let mut old=replace_record(&row,catalog.encode(2).unwrap(),JsonValue::Null,Purpose::Update);
    let mut fresh=catalog.clone();
    fresh.entries[0].id="c".repeat(32);fresh.selected_ids=vec!["c".repeat(32)];
    // Merely giving an old payload a fresh option ID cannot prove a download.
    assert!(append_options(&old,&fresh,2).is_err());
    fresh.entries[0].package.as_mut().unwrap().directory.id="d".repeat(32);
    let appended=append_options(&old,&fresh,2).unwrap();
    assert_eq!(appended["selectedIds"],old["options"]["selectedIds"]);
    assert_eq!(appended["entries"][0],old["options"]["entries"][0]);
    old["options"]["selectedIds"]=json::array![];
    assert_eq!(append_options(&old,&fresh,2).unwrap()["selectedIds"],json::array!["c".repeat(32)]);
}
#[test]
fn browse_append_to_page_link_preserves_two_choices_and_selection() {
    let mut row=legacy();
    row["source"]["nativeRemoteLink"]=object!{version:2,association:"userSelected",submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"711907"},receipt:JsonValue::Null};
    let mut old=option_sources::catalog_for_record(&row,2).unwrap();
    let mut second=old.entries[0].clone();
    second.id="b".repeat(32);
    second.package.as_mut().unwrap().directory.id="c".repeat(32);
    old.entries.push(second);
    let selected=old.selected_ids.clone();
    row.remove("packages");row["options"]=old.encode(2).unwrap();
    let target=admit(row.clone(),2,"default","old-mod",0).unwrap();
    let key=SubmissionKeyV1::parse(&row["source"]["nativeRemoteLink"]["submission"]).unwrap();
    assert!(target.accepts_submission(Purpose::Append,&key));
    let mut fresh=old.clone();fresh.entries.truncate(1);
    fresh.entries[0].id="d".repeat(32);
    fresh.entries[0].package.as_mut().unwrap().directory.id="e".repeat(32);
    fresh.selected_ids=vec![fresh.entries[0].id.clone()];
    let appended=append_options(&row,&fresh,2).unwrap();
    assert_eq!(appended["entries"].len(),3);
    assert_eq!(appended["entries"][0],row["options"]["entries"][0]);
    assert_eq!(appended["entries"][1],row["options"]["entries"][1]);
    assert_eq!(appended["selectedIds"],JsonValue::from(selected));
    let saved=replace_record(&row,appended,JsonValue::Null,Purpose::Append);
    assert!(receipt(&saved).is_null(),"adding a variant does not prove the installed version");
}
#[test]
fn association_only_link_allows_update_without_inventing_a_receipt() {
    let mut row = legacy();
    row["source"]["kind"] = "local".into();
    row["source"]["localImport"] = object! {profileId:"default"};
    row["source"]["nativeRemoteLink"] = object! {version:2,association:"userSelected",
        submission:object!{provider:"gamebanana",submissionType:"mod",submissionId:"711907"},receipt:JsonValue::Null};
    let page = association_submission(&row["source"]).unwrap();
    assert_eq!(page["submissionId"], "711907");
    let target = admit(row.clone(), 2, "default", "old-mod", 0).unwrap();
    let key=SubmissionKeyV1::parse(&page).unwrap();
    use crate::remote::replacement_contract::ReplacementPurpose as Purpose;
    assert!(target.public["receipt"].is_null());
    assert!(target.root_file_id.is_empty());
    assert!(target.accepts_submission(Purpose::Update,&key));
    assert!(!target.accepts_submission(Purpose::Link,&key));
    assert!(!target.accepts_submission(Purpose::Reinstall,&key));
    assert!(target.accepts_submission(Purpose::Append,&key));
    let other=SubmissionKeyV1::parse(&object!{provider:"gamebanana",submissionType:"mod",submissionId:"999"}).unwrap();
    assert!(!target.accepts_submission(Purpose::Update,&other));
    assert!(!target.accepts_submission(Purpose::Append,&other));
    let mut unlinked=row.clone();unlinked["source"].remove("nativeRemoteLink");
    assert!(admit(unlinked,2,"default","old-mod",0).is_err());
    let mut unavailable=row.clone();unavailable["available"]=false.into();
    assert!(admit(unavailable,2,"default","old-mod",0).is_err());
    let fresh=object!{version:1,submission:page.clone(),rootFileId:"456",cohortFingerprint:"d".repeat(64)};
    let catalog=option_sources::catalog_for_record(&row,2).unwrap().encode(2).unwrap();
    let next=replace_record(&row,catalog,fresh.clone(),Purpose::Update);
    assert_eq!(next["source"]["link"],row["source"]["link"]);
    assert_eq!(receipt(&next),&fresh);
    let next=admit(next,2,"default","old-mod",1).unwrap();
    assert!(next.accepts_submission(Purpose::Reinstall,&key));
    // Anything other than the exact association-only shape is not a page link.
    for (key, value) in [("version", JsonValue::from(1)), ("association", "other".into()), ("receipt", object! {})] {
        let mut bad = row["source"].clone();
        bad["nativeRemoteLink"][key] = value;
        assert!(association_submission(&bad).is_none());
        let mut invalid=row.clone();invalid["source"]=bad;
        assert!(admit(invalid,2,"default","old-mod",0).is_err());
    }
    let mut extra = row["source"].clone();
    extra["nativeRemoteLink"]["extra"] = true.into();
    assert!(association_submission(&extra).is_none());
}

// Opt-in qualification against an exported COPY; never opens the live library.
#[test]
#[ignore = "set DLMM_PAGE_LINK_RECORDS to copied profileId/record JSON rows"]
fn copied_page_link_records_are_update_eligible() {
    let path=std::env::var("DLMM_PAGE_LINK_RECORDS").unwrap();
    let rows=json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(rows.is_array() && !rows.is_empty());
    use crate::remote::replacement_contract::ReplacementPurpose as Purpose;
    for row in rows.members() {
        let record=&row["record"];let profile=row["profileId"].as_str().unwrap();
        let id=record["source"]["id"].as_str().unwrap();
        let target=admit(record.clone(),2,profile,id,0).unwrap();
        let key=SubmissionKeyV1::parse(&association_submission(&record["source"]).unwrap()).unwrap();
        assert!(target.public["receipt"].is_null());
        assert!(target.accepts_submission(Purpose::Update,&key));
        assert!(!target.accepts_submission(Purpose::Reinstall,&key));
        let fresh=object!{version:1,submission:key.json(),rootFileId:"456",cohortFingerprint:"d".repeat(64)};
        let next=replace_record(record,record["options"].clone(),fresh,Purpose::Update);
        assert_eq!(next["source"]["link"],record["source"]["link"]);
        assert_eq!(next["enabled"],record["enabled"]);
        assert!(admit(next,2,profile,id,1).unwrap().accepts_submission(Purpose::Reinstall,&key));
        println!("ACCEPT {id}: Update before receipt, Reinstall after receipt");
    }
    println!("PASS {} complete copied library records",rows.len());
}
