//! Live preparation authority. No persisted outcome can reconstruct these plans.
//! The caller acquires RootLock then GameLock and retains both through handoff.
use crate::{
    apply::{self, ApplyPlan}, deployment_intent::{self, DeploymentIntent},
    game_access::GameLock, native::NativeError, paths,
    prepare_control::PrepareControl, store::{self, RootLock, Store},
};
use json::{object, JsonValue};
use std::{fs, os::unix::fs::{MetadataExt,OpenOptionsExt}, path::{Path, PathBuf}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationMode { Modded, Vanilla }
impl PreparationMode {
    fn name(self) -> &'static str { match self { Self::Modded => "modded", Self::Vanilla => "vanilla" } }
}

/// Factual phases, with no synthetic percentage or duration estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationProgress {
    Verifying,
    Repairing { checked:u32, total:u32 },
    /// Completed, verified physical payload files and their bytes only;
    /// coalesced aliases, manifests, and directories are excluded.
    Copying { files_done: u64, files_total: u64, bytes_done: u64, bytes_total: u64 },
    Committing,
}

pub enum PreparationPlan {
    Apply(ApplyPreparationPlan),
    Vanilla(VanillaPlan),
    Verify(VerifiedPreparationPlan),
}
pub struct ApplyPreparationPlan { review: Review }
pub struct VanillaPlan { review: Review }
pub struct VerifiedPreparationPlan { review: Review }

struct Review {
    root: PathBuf,
    game: PathBuf,
    profile: String,
    revision: u64,
    token: String,
    mode: PreparationMode,
    observation: Observation,
}
#[derive(Clone)]
struct Observation {
    apply: ApplyPlan,
    document: JsonValue,
    evidence: JsonValue,
    directories: JsonValue,
    files: FileSnapshot,
}

/// Private construction is intentional: the digest is not itself authority.
/// Consumers retain the live lock guards and this object, never deserialize it.
pub struct PreparedProof {
    evidence: JsonValue,
    digest: String,
    files: FileSnapshot,
}
impl PreparedProof {
    pub fn evidence(&self) -> &JsonValue { &self.evidence }
    pub fn sha256(&self) -> &str { &self.digest }
    /// Recheck the live review's file set immediately before dispatch admission.
    pub(crate) fn revalidate(&self, store: &Store, lock: &RootLock, game: &GameLock) -> Result<(),NativeError> {
        store.locked(lock)?;
        let path=Path::new(self.evidence["gamePath"].as_str().ok_or_else(stale)?);
        game.validate(&path.join("game/citadel")).map_err(|_|stale())?;
        if directories(store.root(),path).map_err(|_|stale())? != self.evidence["directories"]
            || paths::digest_bytes(store::canonical(&store.document()?).as_bytes())? != self.evidence["documentSha256"].as_str().ok_or_else(stale)? {
            return Err(stale());
        }
        self.files.recheck(store.root(),path,&self.evidence["selectedSources"])

    }

}

pub struct PreparedSetup {
    pub snapshot: JsonValue,
    pub resulting_revision: u64,
    pub changed: bool,
    pub profile_id: String,
    pub mode: PreparationMode,
    pub proof: PreparedProof,
    pub gameinfo_sha256: String,
}

fn stale() -> NativeError {
    NativeError::new("stalePreview", "Preparation evidence changed; review the active setup again.")
}
// Captured only in memory following a full review. Directory membership and
// file identities are compared again at dispatch; a mismatch is never rehashed.
#[derive(Clone, PartialEq, Eq)]
struct FileSnapshot {
    entries: std::collections::BTreeMap<PathBuf, (u8,u64,i64,i64,u64,u64,String,Option<PathBuf>)>,
    gameinfo: Vec<u8>,
}
impl FileSnapshot {
    fn capture(root:&Path, game:&Path, evidence:&JsonValue)->Result<Self,NativeError> {
        let mut entries=std::collections::BTreeMap::new();
        fn visit(path:&Path,citadel:&Path,entries:&mut std::collections::BTreeMap<PathBuf,(u8,u64,i64,i64,u64,u64,String,Option<PathBuf>)>)->Result<(),NativeError> {
            let m=fs::symlink_metadata(path).map_err(|_|stale())?;
            if m.file_type().is_symlink() {
                let link=fs::OpenOptions::new().read(true).custom_flags(0x200000|0x20000).open(path).map_err(|_|stale())?;
                let actual=link.metadata().map_err(|_|stale())?;
                if (m.dev(),m.ino())!=(actual.dev(),actual.ino()){return Err(stale());}
                let fsid=paths::persisted_filesystem_id(&link).map_err(|_|stale())?;
                entries.insert(path.to_owned(),(2,m.len(),m.mtime(),m.mtime_nsec(),m.dev(),m.ino(),fsid,Some(fs::read_link(path).map_err(|_|stale())?)));
                return Ok(());
            }
            if !m.is_dir() && !m.is_file(){return Err(stale());}
            let f=fs::OpenOptions::new().read(true).custom_flags(0x20000 | if m.is_dir(){0x10000}else{0}).open(path).map_err(|_|stale())?;
            let actual=f.metadata().map_err(|_|stale())?;
            if (m.dev(),m.ino())!=(actual.dev(),actual.ino()){return Err(stale());}
            let fsid=paths::persisted_filesystem_id(&f).map_err(|_|stale())?;
            let row=if m.is_dir(){(1,0,0,0,m.dev(),m.ino(),fsid,None)}else{(0,m.len(),m.mtime(),m.mtime_nsec(),m.dev(),m.ino(),fsid,None)};
            entries.insert(path.to_owned(),row);
            if m.is_dir() {
                for child in fs::read_dir(path).map_err(|_|stale())? {
                    let child=child.map_err(|_|stale())?.path();
                    if path==citadel && child.file_name().is_some_and(|n|n==crate::launch_record::MARKER){continue;}
                    visit(&child,citadel,entries)?;
                }
            }
            Ok(())
        }
        let citadel=game.join("game/citadel");
        visit(&citadel,&citadel,&mut entries)?;
        let core=game.join("game/core");
        if core.exists(){visit(&core,&citadel,&mut entries)?;}
        let payloads=root.join("payloads");
        if payloads.exists(){visit(&payloads,&citadel,&mut entries)?;}
        for entry in evidence["sources"].members().chain(evidence["importedManifests"].members()) {
            let path=Path::new(entry["path"].as_str().ok_or_else(stale)?);
            if !entries.contains_key(path){visit(path,&citadel,&mut entries)?;}
        }
        for entry in evidence["sources"].members().chain(evidence["importedManifests"].members()) {
            let path=Path::new(entry["path"].as_str().ok_or_else(stale)?);
            let row=entries.get(path).ok_or_else(stale)?;
            let fp=&entry["identity"];
            if row.0!=0 || Some(row.1)!=fp["size"].as_u64() || Some(row.2)!=fp["modifiedSeconds"].as_i64()
                || Some(row.3)!=fp["modifiedNanos"].as_i64() || Some(row.4)!=fp["device"].as_u64()
                || Some(row.5)!=fp["inode"].as_u64() {return Err(stale());}
        }
        let gameinfo=paths::read_bounded(&citadel.join("gameinfo.gi"),4*1024*1024)?;
        Ok(Self{entries,gameinfo})
    }
    fn recheck(&self,root:&Path,game:&Path,evidence:&JsonValue)->Result<(),NativeError> {
        if *self != Self::capture(root,game,evidence).map_err(|_|stale())? {return Err(stale());} Ok(())
    }
}
fn revision(doc: &JsonValue) -> Result<u64, NativeError> {
    store::revision(doc["revision"].as_str().ok_or_else(NativeError::params)?)
}
fn directory(path: &Path) -> Result<JsonValue, NativeError> {
    paths::reject_symlinks(path)?;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() { return Err(stale()); }
    let held=fs::OpenOptions::new().read(true).custom_flags(0x10000|0x20000).open(path)?;
    let current=held.metadata()?;
    if (meta.dev(),meta.ino())!=(current.dev(),current.ino()) { return Err(stale()); }
    Ok(object!{device:meta.dev(),inode:meta.ino(),fsid:paths::persisted_filesystem_id(&held)?})
}
fn directories(root: &Path, game: &Path) -> Result<JsonValue, NativeError> {
    Ok(object!{root:directory(root)?,game:directory(game)?,citadel:directory(&game.join("game/citadel"))?})
}
fn observe(store: &Store, lock: &RootLock, guard: &GameLock, profile: &str, game: &Path, mode: PreparationMode, progress:&mut dyn FnMut(PreparationProgress)) -> Result<Observation, NativeError> {
    store.locked(lock)?;
    guard.validate(&game.join("game/citadel"))?;
    let before = store.document()?;
    let checkpoint=if before["applied"].is_object() {&before["applied"]} else {&before["restoredSetup"]};
    if before["gameFolderChanged"]==true || checkpoint["gameinfo"].as_str().is_some_and(|expected|
        paths::read_bounded(&game.join("game/citadel/gameinfo.gi"),4*1024*1024).is_ok_and(|live|live!=expected.as_bytes())) {
        return Err(NativeError::new("applyReviewRequired","The game files were changed by another mod manager. Review the changes to continue."));
    }
    let identities = directories(store.root(), game)?;
    let before_files=FileSnapshot::capture(store.root(),game,&JsonValue::Null)?;
    let apply = match mode {
        PreparationMode::Modded => ApplyPlan::review_shards_locked(store, profile, game, guard, lock, progress)?,
        PreparationMode::Vanilla => ApplyPlan::review_vanilla_locked(store, game, guard,lock)?,
    };
    if apply.summary["profileId"] != profile { return Err(stale()); }
    let evidence = apply.preparation_evidence(store, guard,lock)?;
    if store.document()? != before || directories(store.root(), game)? != identities { return Err(stale()); }
    store.locked(lock)?;
    guard.validate(&game.join("game/citadel"))?;
    let files=FileSnapshot::capture(store.root(),game,&evidence)?;
    let citadel=game.join("game/citadel"); let game_core=game.join("game/core"); let payloads=store.root().join("payloads");
    let core=files.entries.iter().filter(|(p,_)|p.starts_with(&citadel)||p.starts_with(&game_core)||p.starts_with(&payloads)).map(|(p,v)|(p.clone(),v.clone())).collect();
    if before_files.entries!=core || before_files.gameinfo!=files.gameinfo {return Err(stale());}
    Ok(Observation { apply, document: before, evidence, directories: identities, files })
}
fn unchanged(observation: &Observation, profile: &str, mode: PreparationMode) -> Result<bool, NativeError> {
    let doc = &observation.document;
    let desired = DeploymentIntent::decode(&observation.evidence["intent"])?;
    let performance_current=if let Some(text)=doc["applied"]["gameinfo"].as_str() {
        crate::performance::overlay(text,if mode==PreparationMode::Vanilla {""}else{doc["performancePreset"].as_str().unwrap_or("")})?==text
    }else{false};
    // Exact intent (including every source binding), not just its semantic hash.
    Ok(doc["applied"]["mountedSubset"].is_null() && (mode==PreparationMode::Vanilla || doc["applied"]["manifest"]["repairs"]==observation.apply.repair_receipt()) && performance_current && (mode != PreparationMode::Vanilla || doc["applied"]["manifest"]["version"] == 3)
        && doc["activeProfileId"] == profile && !doc["applied"].is_null()
        && deployment_intent::applied_intent(&doc["applied"])? == Some(desired))
}
fn proof(root: &Path, game: &Path, profile: &str, observation: &Observation, mode: PreparationMode) -> Result<PreparedProof, NativeError> {
    if !unchanged(observation, profile, mode)? { return Err(stale()); }
    let doc = &observation.document;
    let evidence = object!{version:1,mode:mode.name(),rootPath:root.to_string_lossy().as_ref(),
        gamePath:game.to_string_lossy().as_ref(),profileId:profile,revision:doc["revision"].clone(),
        directories:observation.directories.clone(),
        documentSha256:paths::digest_bytes(store::canonical(doc).as_bytes())?,
        applied:doc["applied"].clone(),selectedSources:observation.evidence.clone(),
        fontState:crate::fonts::FontState::decode(doc)?.encode()?};
    store::ensure_internal(&evidence)?;
    let digest = paths::digest_bytes(store::canonical(&evidence).as_bytes())?;
    Ok(PreparedProof { evidence, digest, files:observation.files.clone() })
}
impl PreparationPlan {
    pub fn review(
        store: &Store, lock: &RootLock, game_lock: &GameLock, profile: &str, game: &Path,
        control: &PrepareControl, progress: &mut dyn FnMut(PreparationProgress),
    ) -> Result<Self, NativeError> {
        Self::review_mode(store, lock, game_lock, profile, game, PreparationMode::Modded, control, progress)
    }
    /// Vanilla always targets the currently active profile, irrespective of view.
    pub fn review_vanilla(
        store: &Store, lock: &RootLock, game_lock: &GameLock, game: &Path,
        control: &PrepareControl, progress: &mut dyn FnMut(PreparationProgress),
    ) -> Result<Self, NativeError> {
        control.check()?;
        store.locked(lock)?;
        game_lock.validate(&paths::absolute(game)?.join("game/citadel"))?;
        let intent = DeploymentIntent::empty_active(&store.document()?)?;
        Self::review_mode(store, lock, game_lock, &intent.profile_id, game, PreparationMode::Vanilla, control, progress)
    }
    fn review_mode(
        store: &Store, lock: &RootLock, game_lock: &GameLock, profile: &str, game: &Path,
        mode: PreparationMode, control: &PrepareControl, progress: &mut dyn FnMut(PreparationProgress),
    ) -> Result<Self, NativeError> {
        control.check()?;
        progress(PreparationProgress::Verifying);
        let game = paths::absolute(game)?;
        let mut observation = observe(store, lock, game_lock, profile, &game, mode, progress)?;
        let verify = unchanged(&observation, profile, mode)?;
        if mode == PreparationMode::Vanilla {
            observation.apply.summary["retainedMounts"]=JsonValue::new_array();
        }
        if mode == PreparationMode::Vanilla && verify {
            // Verify installs/removes nothing; show its actual retained empty mount.
            observation.apply.summary["replacedMounts"]=JsonValue::new_array();
            observation.apply.summary["installedMounts"]=JsonValue::new_array();
            observation.apply.summary["retainedMounts"]=crate::generation::mounts(&observation.document["applied"]["manifest"])?.into();
        }
        if mode == PreparationMode::Vanilla { store::ensure_internal(&observation.apply.summary)?; }
        let review = Review { root: store.root().to_owned(), game, profile: profile.into(),
            revision: revision(&observation.document)?, token: paths::random_id()?, mode, observation };
        control.check()?;
        Ok(if verify { Self::Verify(VerifiedPreparationPlan { review }) }
            else if mode == PreparationMode::Vanilla { Self::Vanilla(VanillaPlan { review }) }
            else { Self::Apply(ApplyPreparationPlan { review }) })
    }
    fn reviewed(&self) -> &Review {
        match self { Self::Apply(p) => &p.review, Self::Vanilla(p) => &p.review, Self::Verify(p) => &p.review }
    }
    pub fn root(&self) -> &Path { &self.reviewed().root }
    pub fn game(&self) -> &Path { &self.reviewed().game }
    pub fn profile_id(&self) -> &str { &self.reviewed().profile }
    pub fn expected_revision(&self) -> u64 { self.reviewed().revision }
    pub fn token(&self) -> &str { &self.reviewed().token }
    pub fn mode(&self) -> PreparationMode { self.reviewed().mode }
    pub fn summary(&self) -> &JsonValue { &self.reviewed().observation.apply.summary }
    /// The ordinary Apply action is preserved, including real layout acknowledgement.
    pub fn operation_input(&self, operation_id: &str) -> Result<JsonValue, NativeError> {
        if !paths::safe_id(operation_id) { return Err(NativeError::params()); }
        let r = self.reviewed();
        let mut params = object!{operationId:operation_id,expectedRevision:r.revision.to_string(),token:r.token.clone()};
        let method = match self {
            Self::Apply(_) => { params["deploymentLayout"] = "shards-v1".into(); "native.apply" }
            Self::Vanilla(_) => { params["deploymentLayout"] = "shards-v1".into(); "native.vanilla" }
            Self::Verify(_) => "native.prepare.verify",
        };
        Ok(object!{method:method,params:params})
    }
    /// Replay is checked before stale state; a returned PreparedSetup still requires
    /// fresh live proof at the recorded result revision. Query history separately
    /// when reconciling an old terminal outcome that no longer describes the game.
    pub fn execute(
        &self, store: &Store, lock: &RootLock, game_lock: &GameLock,
        expected: u64, operation_id: &str, input: &JsonValue,
        control: &PrepareControl, progress: &mut dyn FnMut(PreparationProgress),
    ) -> Result<PreparedSetup, NativeError> {
        let replay = store.replay(operation_id, input)?;
        if expected != self.expected_revision() || *input != self.operation_input(operation_id)? { return Err(stale()); }
        let r = self.reviewed();
        if store.root() != r.root { return Err(stale()); }
        store.locked(lock)?;
        game_lock.validate(&r.game.join("game/citadel"))?;
        let snapshot = if let Some(snapshot) = replay { snapshot } else {
            control.check()?;
            match self {
                Self::Apply(p) => {
                    progress(PreparationProgress::Verifying);
                    p.review.observation.apply.apply_locked(store, lock, game_lock, expected, operation_id, input, control, progress)?
                }
                Self::Vanilla(p) => {
                    progress(PreparationProgress::Verifying);
                    p.review.observation.apply.apply_locked(store, lock, game_lock, expected, operation_id, input, control, progress)?
                }
                Self::Verify(p) => store.record_prepared_verification(lock, game_lock, expected, operation_id, input, p, control, progress)?,
            }
        };
        // Apply itself verifies and writes its result. For an unchanged setup,
        // retain the complete observation made by this live review.
        let resulting_revision = revision(&snapshot)?;
        let changed = matches!(self, Self::Apply(_) | Self::Vanilla(_));
        let required_revision = if changed { expected.checked_add(1).ok_or_else(stale)? } else { expected };
        let document=store.document()?;
        if resulting_revision != required_revision || revision(&document)? != resulting_revision
            || directories(store.root(),&r.game)? != r.observation.directories { return Err(stale()); }
        let mut current=r.observation.clone();
        if changed {
            r.observation.apply.verify_prepared_result(&document)?;
            current.document=document;
            let fp=paths::fingerprint(&r.game.join("game/citadel/gameinfo.gi"))?;
            current.evidence["gameinfo"]=crate::fonts::fingerprint_json(&fp);
            current.files=FileSnapshot::capture(store.root(),&r.game,&current.evidence)?;
        } else if document != r.observation.document {return Err(stale());}
        let proof = proof(&r.root, &r.game, &r.profile, &current, r.mode)?;
        let gameinfo_sha256 = current.evidence["gameinfo"]["sha256"].as_str().ok_or_else(stale)?.to_owned();
        Ok(PreparedSetup { snapshot, resulting_revision, changed, profile_id:r.profile.clone(),
            mode:r.mode, proof, gameinfo_sha256 })
    }
}
impl VerifiedPreparationPlan {
    pub(crate) fn admit_input(&self, expected: u64, id: &str, input: &JsonValue) -> Result<(), NativeError> {
        let canonical = object!{method:"native.prepare.verify",params:object!{operationId:id,
            expectedRevision:expected.to_string(),token:self.review.token.clone()}};
        if !paths::safe_id(id) || !crate::package::hex(&self.review.token,32)
            || expected != self.review.revision || *input != canonical { return Err(NativeError::params()); }
        Ok(())
    }
    pub(crate) fn revalidate(&self, store: &Store, lock: &RootLock, guard: &GameLock) -> Result<(), NativeError> {
        store.locked(lock)?;
        guard.validate(&self.review.game.join("game/citadel"))?;
        if store.root()!=self.review.root || store.document()?!=self.review.observation.document {return Err(stale());}
        if !unchanged(&self.review.observation, &self.review.profile, self.review.mode)? { return Err(stale()); }
        apply::ensure_idle(store)?;
        crate::game_access::ensure_game_idle()
    }
    pub(crate) fn profile_id(&self) -> &str { &self.review.profile }
}

#[cfg(test)]
pub(crate) fn assert_partial_play_requires_apply(apply:ApplyPlan,full:JsonValue,partial:JsonValue) {
    let evidence=object!{intent:full["applied"]["deploymentIntent"].clone()};
    let mut observed=Observation{apply,document:full,evidence,directories:JsonValue::Null,files:FileSnapshot{entries:Default::default(),gameinfo:Vec::new()}};
    assert!(unchanged(&observed,"default",PreparationMode::Modded).unwrap());
    observed.document=partial;
    assert!(!unchanged(&observed,"default",PreparationMode::Modded).unwrap(),"Play must reapply a restored partial mount");
}
