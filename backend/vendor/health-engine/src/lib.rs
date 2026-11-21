pub mod errors;
pub mod mod_manager {
    pub mod localization_overlay;
    pub mod mod_health;
    pub mod vdata_history;
}
/// Standalone evidence-based check; all reconstruction remains in memory.
pub fn check_with_history(
    citadel: &std::path::Path,
    inputs: Vec<(String, Vec<std::path::PathBuf>)>,
    build: Option<u32>,
    observed: Option<&[u8]>,
    progress: impl Fn(u32, u32),
) -> Result<(serde_json::Value, Vec<u8>), errors::Error> {
    use mod_manager::{
        localization_overlay::{
            self as overlay, GameIndex, LocalizationModInput, LocalizationOverlayPlan,
        },
        mod_health::{CompatibilityChoice, ModFindings},
        vdata_history::VdataHistoryIndex,
    };
    use std::{collections::BTreeSet, sync::Arc};
    let analysis_version = std::fs::read_to_string(citadel.join("steam.inf"))
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("ClientVersion=")
                    .and_then(|v| v.trim().parse::<u32>().ok())
            })
        });
    let game = Arc::new(GameIndex::open(citadel)?);
    let mut history = observed
        .and_then(|b| VdataHistoryIndex::from_bytes(b).ok())
        .unwrap_or_default();
    let mut recorded = BTreeSet::new();
    let total = inputs.len() as u32;
    let mut mods = Vec::new();
    progress(0, total);
    for (id, vpks) in inputs {
        let input = LocalizationModInput {
            mod_id: id.clone(),
            vpks,
        };
        let covered = overlay::has_checkable_resources(&input)?;
        let f = if covered {
            let (plan, _) = LocalizationOverlayPlan::build_standalone(
                &game,
                input,
                &mut history,
                analysis_version,
                &mut recorded,
            )?;
            ModFindings::from_analysis(&plan.analyze(&[])?, &id, true)
        } else {
            ModFindings::uncovered()
        };
        let symptoms: BTreeSet<_> = f
            .fix_symptoms
            .union(&f.remaining_symptoms)
            .copied()
            .collect();
        mods.push(serde_json::json!({"modId":id,"status":f.status(CompatibilityChoice::Unset),"symptoms":symptoms,"fixSymptoms":f.fix_symptoms,"remainingSymptoms":f.remaining_symptoms,"repairs":f.repairs,"warnings":f.warnings,"gameBuild":build,"source":"standalone"}));
        progress(mods.len() as u32, total);
    }
    Ok((
        serde_json::json!({"gameBuild":build,"mods":mods}),
        history.to_bytes().map_err(errors::Error::ModInvalid)?,
    ))
}

pub fn check(
    citadel: &std::path::Path,
    inputs: Vec<(String, Vec<std::path::PathBuf>)>,
    build: Option<u32>,
    progress: impl Fn(u32, u32),
) -> Result<serde_json::Value, errors::Error> {
    check_with_history(citadel, inputs, build, None, progress).map(|(report, _)| report)
}

mod packing;
/// Builds in caller-owned private staging. Game and mod inputs are read only.
pub fn repair(citadel: &std::path::Path, inputs: Vec<(String, Vec<std::path::PathBuf>)>, enabled: std::collections::BTreeSet<String>, output: &std::path::Path, workspace: &std::path::Path) -> Result<serde_json::Value, errors::Error> { repair_with_progress(citadel,inputs,enabled,output,workspace,|_,_|{}) }
pub fn repair_with_progress(citadel: &std::path::Path, inputs: Vec<(String, Vec<std::path::PathBuf>)>, enabled: std::collections::BTreeSet<String>, output: &std::path::Path, workspace: &std::path::Path, progress: impl Fn(u32,u32)) -> Result<serde_json::Value, errors::Error> {
 use mod_manager::{localization_overlay::{LocalizationOverlayPlan,LocalizationModInput},vdata_history::VdataHistoryIndex};
 let mods:Vec<_>=inputs.into_iter().map(|(mod_id,vpks)|LocalizationModInput{mod_id,vpks}).collect();
 let version=std::fs::read_to_string(citadel.join("steam.inf")).ok().and_then(|s|s.lines().find_map(|l|l.trim().strip_prefix("ClientVersion=").and_then(|v|v.trim().parse().ok())));
 let (plan,_,_)=LocalizationOverlayPlan::build_learning_history(citadel,&mods,VdataHistoryIndex::default(),version,&enabled,&|p|progress(p.checked,p.total))?;
 let analysis=plan.analyze(&[])?;
 let result=plan.write(output,&[],workspace)?;
 let findings:Vec<_>=mods.iter().filter(|m|enabled.contains(&m.mod_id)).map(|m|{let f=mod_manager::mod_health::ModFindings::from_analysis(&analysis,&m.mod_id,true);serde_json::json!({"modId":m.mod_id,"repairs":f.repairs,"warnings":f.warnings,"fixSymptoms":f.fix_symptoms,"remainingSymptoms":f.remaining_symptoms})}).collect();
 Ok(serde_json::json!({"analysis":analysis,"result":result,"mods":findings}))
}

#[doc(hidden)]
pub use source2_model::vpk_extract::VpkArchive;
