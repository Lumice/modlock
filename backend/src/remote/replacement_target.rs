//! Admission of one current, explicitly identified remote replacement target.
//!
//! This is metadata authority only. A receipt and a record digest do not
//! establish the health or currency of any installed bytes.
use super::contract::SubmissionKeyV1;
use crate::{catalog, fonts, local_content::option_sources, local_import, native::NativeError,
    options, package, paths, store};
use json::{object, JsonValue};
use std::{collections::HashSet, path::Path};

fn unsupported(message: &'static str) -> NativeError {
    NativeError::new("eligibilityUnknown", message)
}

/// A complete row stays private to the worker. The public projection does not
/// grant authority to replace a row without rereading this exact record.
pub(crate) struct ReplacementTarget {
    pub record: JsonValue,
    pub layout: u32,
    pub digest: String,
    pub submission: Option<SubmissionKeyV1>,
    pub root_file_id: String,
    pub public: JsonValue,
}

impl ReplacementTarget {
    pub(crate) fn accepts_submission(&self,purpose:super::replacement_contract::ReplacementPurpose,key:&SubmissionKeyV1)->bool {
        if purpose==super::replacement_contract::ReplacementPurpose::Link {self.submission.is_none()}
        else if purpose==super::replacement_contract::ReplacementPurpose::Reinstall
            && self.root_file_id.is_empty() {false}
        else {self.submission.as_ref()==Some(key)}
    }
}

pub(crate) fn read(root: &Path, profile: &str, mod_id: &str, revision: u64)
    -> Result<ReplacementTarget, NativeError>
{
    let (record, layout) = option_sources::pinned_record(root, profile, mod_id, revision)
        .map_err(|e|if e.kind=="unknownMod"{unsupported("The selected mod is no longer available in this profile.")}else{e})?;
    admit(record, layout, profile, mod_id, revision)
}

/// GameBanana page of an association-only link (version 2: chosen by the user
/// when importing a mod found in the game, without a download receipt). It is
/// a submission identity only; it provides no evidence of the installed version.
pub(crate) fn association_submission(source: &JsonValue) -> Option<JsonValue> {
    let overlay = &source["nativeRemoteLink"];
    if crate::catalog::contract::exact(overlay,&["version","association","submission","receipt"]).is_err()
        || overlay["version"] != 2 || overlay["association"] != "userSelected" || !overlay["receipt"].is_null() {
        return None;
    }
    SubmissionKeyV1::parse(&overlay["submission"]).ok().map(|s| s.json())
}

pub(crate) fn valid_library_date(v: &JsonValue) -> bool {
    let Some(s) = v.as_str() else { return false };
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z'
        || b.iter().enumerate().any(|(i,c)| ![4,7,10,13,16,19].contains(&i) && !c.is_ascii_digit()) { return false; }
    let n = |a:usize,z:usize| s[a..z].parse::<u32>().unwrap_or(0);
    let y=n(0,4); let month=n(5,7); let day=n(8,10);
    let max = match month { 2 => if y%4==0 && (y%100!=0 || y%400==0) {29} else {28}, 4|6|9|11 => 30, 1|3|5|7|8|10|12 =>31, _=>0 };
    y>0 && day>0 && day<=max && n(11,13)<24 && n(14,16)<60 && n(17,19)<60
}

pub(crate) fn admit(record: JsonValue, layout: u32, profile: &str, mod_id: &str,
    revision: u64) -> Result<ReplacementTarget, NativeError>
{
    if !paths::safe_id(profile) || !paths::safe_id(mod_id) { return Err(unsupported("This mod has an unsafe identity.")); }
    if record.has_key("updatedAt") && !valid_library_date(&record["updatedAt"]) { return Err(unsupported("This mod has an invalid library update date.")); }
    let source=&record["source"];
    let association=association_submission(source);
    let linked=source.has_key("nativeRemoteLink");
    let legacy=linked || !source.has_key("remoteReceipt");
    let receipt;
    let submission;
    if legacy {
        let linked_local=linked && source["kind"]=="local" && source["localImport"].is_object();
        if !source.is_object() || source["id"]!=mod_id || (source.has_key("localImport") && !linked_local)
            || !(source["kind"].is_null() || source["kind"]=="remote" || linked_local) || source.has_key("type") || source.has_key("origin")
            || record["available"]!=true || !record["unavailableReason"].is_null()
            || source.has_key("remoteReceipt") {
            return Err(unsupported("This mod has unavailable, local or ambiguous provenance."));
        }
        // Unknown row envelopes can carry auxiliary ownership: do not discard them.
        if record.entries().any(|(k,_)| !["source","enabled","available","previewHidden","unavailableReason","packages","payloads","options","updatedAt"].contains(&k)) {
            return Err(unsupported("This mod has unsupported retained content."));
        }
        if linked {
            let overlay=&source["nativeRemoteLink"];
            crate::catalog::contract::exact(overlay,&["version","association","submission","receipt"])
                .map_err(|_|unsupported("This mod has an unknown link association."))?;
            if association.is_none() && (overlay["version"]!=1 || overlay["association"]!="userSelected") {
                return Err(unsupported("This mod has an unknown link association."));
            }
            if association.is_none() {
                super::replacement_contract::validate_receipt(&overlay["receipt"])
                    .map_err(|_|unsupported("This mod has an invalid linked receipt."))?;
                if overlay["submission"]!=overlay["receipt"]["submission"] {return Err(unsupported("This mod has conflicting link evidence."));}
            }
            receipt=overlay["receipt"].clone();
            submission=Some(SubmissionKeyV1::parse(&overlay["submission"])?);
        } else { receipt=JsonValue::Null; submission=None; }
    } else {
        if !mod_id.strip_prefix("remote-").is_some_and(|s|package::hex(s,32)){return Err(unsupported("This mod has no supported native remote identity."));}
        let record_keys=["source","available","previewHidden","enabled","options"];
        let source_keys=["id","remoteId","kind","name","author","category","link","description","status","installedVpks","installOrder","createdAt","remoteReceipt"];
        if !record.is_object()||record.len()!=record_keys.len()+usize::from(record.has_key("updatedAt"))||record_keys.iter().any(|k|!record.has_key(k))
            ||!source.is_object()||source.len()!=source_keys.len()||source_keys.iter().any(|k|!source.has_key(k))
            ||source["status"]!="downloaded"||!source["installedVpks"].is_array()||!source["installedVpks"].is_empty()||source["createdAt"].as_str().is_none()
        {return Err(unsupported("This mod has unsupported or ambiguous source provenance."));}
        if source["id"] != mod_id || source["kind"] != "remote" || !source["localImport"].is_null()
            || !source["type"].is_null() || !source["origin"].is_null()
        {
            return Err(unsupported("This mod has ambiguous or local source provenance."));
        }
        let native_receipt = &source["remoteReceipt"];

        if !native_receipt.is_object() || native_receipt.len() != 4
            || ["version", "submission", "rootFileId", "cohortFingerprint"].iter().any(|k| !native_receipt.has_key(k))
            || native_receipt["version"] != 1
            || !native_receipt["rootFileId"].as_str().is_some_and(|s| store::revision(s).is_ok_and(|n| n > 0))
            || !native_receipt["cohortFingerprint"].as_str().is_some_and(|s| package::hex(s, 64))
        {
            return Err(unsupported("This mod has no supported remote native_receipt."));
        }
        let native_submission = SubmissionKeyV1::parse(&native_receipt["submission"])
            .map_err(|_| unsupported("This mod has no supported remote native_receipt."))?;
        let slug = catalog::provider::SubmissionKeyV1::parse(&native_submission.json())
            .map_err(|_| unsupported("This mod has no supported remote identity."))?.slug();
        if source["remoteId"] != slug
            || source.has_key("provider") && source["provider"] != "gamebanana"
            || source.has_key("submissionType") && source["submissionType"] != native_submission.submission_type
            || source.has_key("submissionId") && source["submissionId"] != native_submission.submission_id
            || !(source["link"].is_null() || source["link"] == "" || source["link"] == native_submission.profile_url())
        {
            return Err(unsupported("This mod has conflicting remote identity evidence."));
        }
        receipt=source["remoteReceipt"].clone();
        submission=Some(native_submission);
    }
    let choices = option_sources::catalog_for_record(&record, layout)
        .map_err(|_| unsupported("This mod has unsupported or ambiguous choices."))?;
    (if choices.selected_ids.is_empty() { Ok(()) } else { choices.validate_selection_save() })
        .map_err(|_| unsupported("This mod has unsupported or ambiguous choices."))?;
    if choices.entries.is_empty() || choices.entries.iter().any(|e|
        e.package.is_none() || e.expected_family.is_none() || e.archive_binding.is_some() || e.unavailable_reason.is_some())
        || !choices.retained_archives.is_empty() || !choices.attachments.is_empty()
        || !local_import::stored_archive_refs(&record).map_err(|_| unsupported("This mod has unsupported retained sources."))?.is_empty()
        || !fonts::stored_refs(&record).map_err(|_| unsupported("This mod has unsupported managed fonts."))?.is_empty()
    {
        return Err(unsupported("This mod has retained originals or unsupported managed content."));
    }
    let mut metadata = object! {name:source["name"].clone(),author:source["author"].clone(),
        category:source["category"].clone(),description:source["description"].clone()};
    if legacy {for k in ["author","category","description"] {if source[k].is_null(){metadata[k]="".into();}}}
    super::contract::validate_metadata_fields(&metadata)
        .map_err(|_| unsupported("This mod has unsupported user metadata."))?;
    if ["enabled", "previewHidden", "available"].iter().any(|k| !record[*k].is_boolean())
        || source["installOrder"].as_i64().is_none()
    {
        return Err(unsupported("This mod has unsupported profile fields."));
    }
    let digest = option_sources::record_digest(&record)?;
    let public = object! {profileId:profile,modId:mod_id,revision:revision.to_string(),
        recordDigest:digest.clone(),receipt:receipt.clone(),enabled:record["enabled"].clone(),
        previewHidden:record["previewHidden"].clone(),available:record["available"].clone(),
        installOrder:source["installOrder"].clone(),metadata:metadata,
        optionCount:choices.entries.len().to_string(),selectedOptionIds:JsonValue::from(choices.selected_ids)};
    Ok(ReplacementTarget { root_file_id: receipt["rootFileId"].as_str().unwrap_or("").to_owned(),
        submission, record, layout, digest, public })
}

pub(crate) fn receipt(record:&JsonValue)->&JsonValue {
    if record["source"].has_key("nativeRemoteLink") { &record["source"]["nativeRemoteLink"]["receipt"] }
    else { &record["source"]["remoteReceipt"] }
}
/// Retain the complete old catalog and its saved selection. New options have
/// fresh immutable payload identities and are reviewed independently.
pub(crate) fn append_options(old:&JsonValue, fresh:&options::OptionCatalog, layout:u32)->Result<JsonValue,NativeError> {
    let mut catalog=options::for_record(old,layout)?;
    let old_ids=local_import::record_files(old,layout)?.into_iter().map(|(f,_)|f.id).collect::<HashSet<_>>();
    if !fresh.retained_archives.is_empty() || !fresh.attachments.is_empty()
        || fresh.entries.iter().any(|e|catalog.entries.iter().any(|old|old.id==e.id)
            || e.package.as_ref().is_none_or(|p|std::iter::once(&p.directory).chain(p.companions.values()).any(|f|old_ids.contains(&f.id)))) {
        return Err(unsupported("The downloaded files cannot be added to these choices."));
    }
    catalog.entries.extend(fresh.entries.clone());
    if catalog.selected_ids.is_empty() {catalog.selected_ids=fresh.selected_ids.clone();}
    catalog.validate_selection_save()?;
    catalog.encode(layout)
}
pub(crate) fn verify_retained(root:&Path, record:&JsonValue, layout:u32)->Result<(),NativeError> {
    for package in options::retained_packages(record,layout)? {
        crate::generation::verify_package(&package,layout,|file| {
            let mut row=package::encode_file(file);row["role"]="payload".into();
            local_import::destination(root,&row)
        })?;
    }
    Ok(())
}
pub(crate) fn replace_record(old:&JsonValue, catalog:JsonValue, fresh:JsonValue, purpose:super::replacement_contract::ReplacementPurpose)->JsonValue {
    let mut record=old.clone(); record.remove("packages");record.remove("payloads");record["options"]=catalog;
    if purpose==super::replacement_contract::ReplacementPurpose::Append {return record;}
    if purpose==super::replacement_contract::ReplacementPurpose::Link {record["source"]["nativeRemoteLink"]=object!{version:1,association:"userSelected",submission:fresh["submission"].clone(),receipt:fresh};}
    else if record["source"].has_key("nativeRemoteLink") {record["source"]["nativeRemoteLink"]["version"]=1.into();record["source"]["nativeRemoteLink"]["receipt"]=fresh;}
    else {record["source"]["remoteReceipt"]=fresh;}
    record
}
pub(crate) fn transition(document:&mut JsonValue,layout:u32)->Result<(),NativeError>{
    let version=package::document_version(document)?;
    if version<3 && document.has_key("fonts") {return Err(unsupported("This library has unknown historical font evidence."));}
    document["storeVersion"]=4.into();document["packageVersion"]=layout.into();
    if version<3 {document["fonts"]=object!{version:1,installed:JsonValue::Null,previous:JsonValue::Null};}
    Ok(())
}

/// Reject every already-retained physical identity, including checkpoints
/// whose choice catalog is no longer present in the current profile.
pub(crate) fn reject_payload_collisions(root:&Path, document:&JsonValue, layout:u32,
    ids:&HashSet<String>) -> Result<(),NativeError>
{
    let collision=||unsupported("New payload identities conflict with retained library evidence.");
    local_import::reject_existing_claims(document,layout,ids,None).map_err(|_|collision())?;
    crate::deployment_metadata::admit(root,document).map_err(|_|collision())?;
    for checkpoint in std::iter::once(&document["applied"]).chain(std::iter::once(&document["previous"]["applied"]))
        .chain(document["retiredGenerations"].members()).filter(|c|!c.is_null()) {
        let view=crate::generation::decode(&checkpoint["manifest"]).map_err(|_|collision())?;
        if view.files.iter().any(|file|ids.contains(&file.payload.id)){return Err(collision());}
        if let Some(intent)=crate::deployment_intent::applied_intent(checkpoint).map_err(|_|collision())? {
            for owner in intent.mods {for package in owner.packages {
                if ids.contains(&package.directory.id)||package.companions.values().any(|f|ids.contains(&f.id)){return Err(collision());}
            }}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "replacement_target_tests.rs"]
mod link_tests;
