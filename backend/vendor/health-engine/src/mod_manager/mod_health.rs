use crate::errors::Error;
use crate::mod_manager::localization_overlay::LocalizationOverlayAnalysis;
use crate::mod_manager::localization_overlay::asset_compatibility::{
    AssetWarning, RepairKind as AssetRepairKind, WarningKind as AssetWarningKind,
};
use crate::mod_manager::localization_overlay::vdata_compatibility::{
    RepairKind as DataRepairKind, WarningKind as DataWarningKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const CACHE_FILE: &str = "mod-health.json";
// Increment when status or symptom rules change, so cached verdicts are rechecked.
const HEALTH_RULES_REVISION: u32 = 10;

static CACHE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModHealthStatus {
    Works,
    Fixable,
    /// Has repairs, but some findings remain after them.
    PartlyFixable,
    Fixed,
    /// Fixable or partly fixable, but the player chose to keep the mod's
    /// original files.
    OriginalFiles,
    NeedsUpdate,
    NotCovered,
}

/// What a player would notice in game, grouped from the detailed repair and
/// finding kinds of the compatibility analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModHealthSymptom {
    CameraAim,
    Animations,
    Appearance,
    Model,
    GameData,
    CrashRisk,
}

/// `launch` results come from the full analysis of every opted-in mod together
/// and take precedence over `standalone` checks of the same files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModHealthSource {
    Standalone,
    Launch,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModHealth {
    pub mod_id: String,
    pub status: ModHealthStatus,
    /// Union of `fix_symptoms` and `remaining_symptoms`.
    pub symptoms: Vec<ModHealthSymptom>,
    /// What the available repairs address.
    pub fix_symptoms: Vec<ModHealthSymptom>,
    /// What the repairs leave unresolved.
    pub remaining_symptoms: Vec<ModHealthSymptom>,
    pub repairs: u32,
    pub warnings: u32,
    pub game_build: Option<u32>,
    pub checked_at: String,
    pub source: ModHealthSource,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModHealthReport {
    pub game_build: Option<u32>,
    /// The game build of the check before the latest one, so the UI can tell
    /// when Deadlock updated since mods were last checked.
    pub previous_game_build: Option<u32>,
    pub mods: Vec<ModHealth>,
}

/// Sent as `mod-health-progress` while `check_mod_health` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModHealthProgress {
    pub checked: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FixModsStatus {
    Applied,
    ReviewRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixModsResult {
    pub status: FixModsStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompatibilityChoice {
    Unset,
    Enabled,
    OriginalFiles,
}

pub(crate) enum Finding<'a> {
    AssetRepair(&'a AssetRepairKind),
    AssetWarning(&'a AssetWarning),
    DataRepair(&'a DataRepairKind),
    DataWarning(&'a DataWarningKind),
    Localization,
}

/// Structural model faults the engine can reject outright, and missing
/// references it may not have a fallback for, are reported as crash risks.
/// A missing texture only changes how the model looks.
pub(crate) fn symptom(finding: Finding) -> ModHealthSymptom {
    use ModHealthSymptom::*;
    match finding {
        Finding::AssetRepair(kind) => match kind {
            AssetRepairKind::AnimationSkeleton
            | AssetRepairKind::AnimationSkeletonRebased
            | AssetRepairKind::AnimationInterface
            | AssetRepairKind::AnimationClipTracks
            | AssetRepairKind::AnimationRigRebase
            | AssetRepairKind::TaggedSounds => Animations,
            AssetRepairKind::CameraInterface
            | AssetRepairKind::CameraControls
            | AssetRepairKind::CameraAttachmentFrame => CameraAim,
            AssetRepairKind::MaterialBindings => Appearance,
            AssetRepairKind::ModelRelocation => Model,
        },
        Finding::AssetWarning(warning) if is_missing_texture(warning) => Appearance,
        Finding::AssetWarning(warning) => match warning.kind {
            AssetWarningKind::UnverifiedSkeleton
            | AssetWarningKind::MissingAnimationInterface
            | AssetWarningKind::AnimationMapping
            | AssetWarningKind::MissingSoundBindings => Animations,
            AssetWarningKind::OutdatedCameraAttachments
            | AssetWarningKind::MissingCameraAttachments => CameraAim,
            AssetWarningKind::MaterialMapping | AssetWarningKind::MaterialInterface => Appearance,
            AssetWarningKind::ModelMapping | AssetWarningKind::OverlappingModel => Model,
            AssetWarningKind::InvalidSkeleton
            | AssetWarningKind::InvalidBoneRemapping
            | AssetWarningKind::InvalidAttachment
            | AssetWarningKind::InvalidHitbox
            | AssetWarningKind::MissingResource
            | AssetWarningKind::UnreadableResource
            | AssetWarningKind::MissingCameraInterface => CrashRisk,
        },
        Finding::DataRepair(
            DataRepairKind::HeroDevelopmentState
            | DataRepairKind::InheritedDefinition
            | DataRepairKind::RetainedDependency
            | DataRepairKind::CurrentReference
            | DataRepairKind::EnumMapEntry
            | DataRepairKind::RestoredGameData
            | DataRepairKind::RestoredGameText,
        ) => GameData,
        Finding::DataWarning(
            DataWarningKind::MissingDefinition
            | DataWarningKind::UnverifiedDefinition
            | DataWarningKind::UnverifiedInheritance
            | DataWarningKind::MissingDevelopmentState
            | DataWarningKind::OmittedHeroDefinition
            | DataWarningKind::OmittedEnumMapEntry
            | DataWarningKind::UnreadableData,
        ) => GameData,
        Finding::Localization => GameData,
    }
}

fn is_missing_texture(warning: &AssetWarning) -> bool {
    let target = warning.detail.to_ascii_lowercase();
    warning.kind == AssetWarningKind::MissingResource
        && (target.ends_with(".vtex") || target.ends_with(".vtex_c"))
}

/// Warnings that stay listed in review but don't mark a mod as needing an
/// update. Missing textures and shader differences only change
/// looks. Retired `.vanmgrph` graphs are what animation includes still name
/// after an update; a primary model that really lost its animations reports
/// `MissingAnimationInterface` as well. Portrait attachments only frame the HUD
/// portrait. A skin updated for Paint the Town that plays correctly in game
/// still reports every one of these.
fn is_informational(warning: &AssetWarning) -> bool {
    match warning.kind {
        AssetWarningKind::MaterialInterface => true,
        AssetWarningKind::MissingResource => {
            is_missing_texture(warning)
                || warning.detail.to_ascii_lowercase().ends_with(".vanmgrph")
        }
        AssetWarningKind::InvalidAttachment | AssetWarningKind::MissingCameraAttachments => {
            warning.detail.starts_with("portrait_")
        }
        _ => false,
    }
}

/// A prop model, such as a particle's model, never acts as a hero, so hero
/// interface warnings do not apply.
fn affects_mod(analysis: &LocalizationOverlayAnalysis, warning: &AssetWarning) -> bool {
    !is_informational(warning)
        && !(analysis.prop_models.contains(&warning.file_path)
            && matches!(
                warning.kind,
                AssetWarningKind::MissingCameraInterface
                    | AssetWarningKind::MissingCameraAttachments
                    | AssetWarningKind::MissingAnimationInterface
                    | AssetWarningKind::OutdatedCameraAttachments
                    | AssetWarningKind::ModelMapping
            ))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModFindings {
    pub repairs: u32,
    pub warnings: u32,
    needs_update: bool,
    pub fix_symptoms: BTreeSet<ModHealthSymptom>,
    pub remaining_symptoms: BTreeSet<ModHealthSymptom>,
    /// Whether the mod has any resource the analysis can check.
    covered: bool,
}

impl ModFindings {
    pub(crate) fn uncovered() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn with_repairs(repairs: u32) -> Self {
        Self {
            repairs,
            covered: true,
            ..Self::default()
        }
    }

    /// Uncertain baselines and stale localization snapshots still pause launch
    /// for review, but they describe how a mod's edits are merged rather than a
    /// defect, so they do not mark the mod as needing an update. Warnings on
    /// models the game never loads are left out, and informational warnings
    /// count as warnings without requiring an update.
    pub(crate) fn from_analysis(
        analysis: &LocalizationOverlayAnalysis,
        mod_id: &str,
        covered: bool,
    ) -> Self {
        let repairs: Vec<_> = analysis
            .asset_repairs
            .iter()
            .filter(|repair| repair.mod_id == mod_id)
            .map(|repair| Finding::AssetRepair(&repair.kind))
            .chain(
                analysis
                    .data_repairs
                    .iter()
                    .filter(|repair| repair.mod_id == mod_id)
                    .map(|repair| Finding::DataRepair(&repair.kind)),
            )
            .collect();
        let warnings: Vec<_> = analysis
            .asset_warnings
            .iter()
            .filter(|warning| {
                warning.mod_id == mod_id && !analysis.unloaded_models.contains(&warning.file_path)
            })
            .map(Finding::AssetWarning)
            .chain(
                analysis
                    .data_warnings
                    .iter()
                    .filter(|warning| warning.mod_id == mod_id)
                    .map(|warning| Finding::DataWarning(&warning.kind)),
            )
            .chain(
                analysis
                    .parse_warnings
                    .iter()
                    .filter(|warning| warning.mod_id == mod_id)
                    .map(|_| Finding::Localization),
            )
            .collect();
        let warning_count = warnings.len() as u32;
        let remaining_symptoms: BTreeSet<_> = warnings
      .into_iter()
      .filter(
        |finding| !matches!(finding, Finding::AssetWarning(warning) if !affects_mod(analysis, warning)),
      )
      .map(symptom)
      .collect();
        Self {
            repairs: repairs.len() as u32,
            warnings: warning_count,
            needs_update: !remaining_symptoms.is_empty(),
            fix_symptoms: repairs.into_iter().map(symptom).collect(),
            remaining_symptoms,
            covered,
        }
    }

    pub(crate) fn status(&self, choice: CompatibilityChoice) -> ModHealthStatus {
        if self.repairs > 0 {
            match choice {
                CompatibilityChoice::Enabled => ModHealthStatus::Fixed,
                CompatibilityChoice::OriginalFiles => ModHealthStatus::OriginalFiles,
                CompatibilityChoice::Unset if self.needs_update => ModHealthStatus::PartlyFixable,
                CompatibilityChoice::Unset => ModHealthStatus::Fixable,
            }
        } else if self.needs_update {
            ModHealthStatus::NeedsUpdate
        } else if !self.covered {
            ModHealthStatus::NotCovered
        } else {
            ModHealthStatus::Works
        }
    }
}
