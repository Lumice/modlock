//! Pure generation3 placement. Source verification remains the Apply writer's
//! responsibility, including every selected binding that aliases another owner.
use crate::{deployment_intent::DeploymentIntent, native::NativeError, package::{self, PackageRef}};
use std::{collections::{HashMap, HashSet}, ops::Range};

pub const PER_SHARD_LIMIT: usize = 99;
pub const MAX_SHARDS: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutCounts {
    pub selected_references: u64,
    pub unique_families: u64,
    pub companion_files: u64,
    pub physical_files: u64,
    pub total_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedFamily {
    pub package: PackageRef,
    pub mod_id: String,
    pub mod_ids: Vec<String>,
    pub shard_index: u8,
    pub slot: u8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutShard {
    pub index: u8,
    /// Range in the global, first-encounter family sequence.
    pub families: Range<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModBindings {
    pub mod_id: String,
    /// One global family index per original selected reference, in intent order.
    pub families: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentLayout {
    pub families: Vec<PlacedFamily>,
    pub bindings: Vec<ModBindings>,
    pub shards: Vec<LayoutShard>,
    pub counts: LayoutCounts,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapacityOverflow {
    /// Complete input populations, never just the successfully placed prefix.
    pub counts: LayoutCounts,
    pub partial_shards: Vec<LayoutShard>,
    pub first_failing_mod_id: String,
    pub first_failing_group_size: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRequired {
    pub mod_id: String,
    pub reason: &'static str,
    /// Unknown populations must be represented as null by a display codec.
    pub counts: Option<LayoutCounts>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutOutcome {
    Fits(DeploymentLayout),
    ExceedsCapacity(CapacityOverflow),
    SelectionRequired(SelectionRequired),
}
fn limit() -> NativeError {
    NativeError::new("deploymentLayoutLimit", "Deployment populations or byte totals exceed representable limits.")
}
fn add(total: &mut u64, n: u64) -> Result<(), NativeError> {
    *total = total.checked_add(n).ok_or_else(limit)?;
    Ok(())
}

pub fn plan(intent: &DeploymentIntent) -> Result<LayoutOutcome, NativeError> {
    // Public structs cannot bypass the immutable codec or its JSON bound.
    let intent = DeploymentIntent::decode(&intent.encode())?;
    for row in &intent.mods {
        let mut concrete = HashSet::new();
        let reason = if row.packages.len() > PER_SHARD_LIMIT {
            Some("Select at most 99 packages for this mod.")
        } else if row.packages.iter().any(|p| !concrete.insert((&p.directory.id, &p.directory.source_name))) {
            Some("Repeated concrete package bindings must be resolved before Apply.")
        } else { None };
        if let Some(reason) = reason {
            return Ok(LayoutOutcome::SelectionRequired(SelectionRequired {mod_id:row.mod_id.clone(), reason, counts:None}));
        }
    }
    let mut counts = LayoutCounts {selected_references:0, unique_families:0, companion_files:0, physical_files:0, total_bytes:0};
    let mut keys = HashMap::<String, usize>::new();
    let mut families = Vec::<PlacedFamily>::new();
    let mut bindings = Vec::new();
    let mut groups = Vec::new();
    for row in &intent.mods {
        let start = families.len();
        let mut selected = Vec::new();
        for p in &row.packages {
            add(&mut counts.selected_references, 1)?;
            let key = package::family_key(p)?;
            let index = if let Some(&index) = keys.get(&key) {
                // A mod may select distinct concrete aliases of one family.
                if families[index].mod_ids.last() != Some(&row.mod_id) {
                    families[index].mod_ids.push(row.mod_id.clone());
                }
                index
            } else {
                let index = families.len();
                keys.insert(key, index);
                add(&mut counts.unique_families, 1)?;
                add(&mut counts.companion_files, u64::try_from(p.companions.len()).map_err(|_| limit())?)?;
                for file in std::iter::once(&p.directory).chain(p.companions.values()) {
                    add(&mut counts.physical_files, 1)?;
                    add(&mut counts.total_bytes, file.size)?;
                }
                families.push(PlacedFamily {package:p.clone(), mod_id:row.mod_id.clone(), mod_ids:vec![row.mod_id.clone()], shard_index:0, slot:0});
                index
            };
            selected.push(index);
        }
        if start != families.len() { groups.push((row.mod_id.clone(), start..families.len())); }
        bindings.push(ModBindings {mod_id:row.mod_id.clone(), families:selected});
    }
    let mut shards = vec![LayoutShard {index:1, families:0..0}];
    for (mod_id, group) in groups {
        let count = group.len();
        if shards.last().unwrap().families.len() + count > PER_SHARD_LIMIT {
            if shards.len() == MAX_SHARDS {
                return Ok(LayoutOutcome::ExceedsCapacity(CapacityOverflow {counts, partial_shards:shards,
                    first_failing_mod_id:mod_id, first_failing_group_size:count}));
            }
            shards.push(LayoutShard {index:(shards.len()+1) as u8, families:group.start..group.start});
        }
        let shard = shards.last_mut().unwrap();
        for (offset, family) in families[group.clone()].iter_mut().enumerate() {
            family.shard_index = shard.index;
            family.slot = (shard.families.len()+offset+1) as u8;
        }
        shard.families.end = group.end;
    }
    Ok(LayoutOutcome::Fits(DeploymentLayout {families, bindings, shards, counts}))
}
