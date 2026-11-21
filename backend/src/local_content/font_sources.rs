//! Only the local-content owner admits filesystem authority. Consumers borrow
//! these sources; display provenance cannot be decoded into a source handle.
use crate::{
    archive_content::PreparedArchive,
    font_content::{FontContainer, FontSourceKey},
    native::NativeError,
    paths::Fingerprint,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug)]
pub(crate) struct FontSourceFile {
    path: PathBuf,
    fingerprint: Fingerprint,
    content_stamp: Option<String>,
    origin: FontContainer,
    archive: Option<Arc<PreparedArchive>>,
}
impl FontSourceFile {
    pub(super) fn raw(
        path: PathBuf,
        fingerprint: Fingerprint,
        root_ordinal: usize,
        relative_path: String,
    ) -> Self {
        let content_stamp=crate::paths::open_regular(&path).and_then(|f|crate::paths::content_stamp(&f)).ok();
        Self {
            content_stamp,
            path,
            fingerprint,
            origin: FontContainer::Raw {
                root_ordinal,
                relative_path,
            },
            archive: None,
        }
    }
    /// Reuses the producer-owned sealed member, never a path from evidence.
    pub(super) fn archived(archive: Arc<PreparedArchive>, index: u32) -> Result<Self, NativeError> {
        let member = archive
            .extracted_members()
            .iter()
            .find(|m| m.member().index == index)
            .ok_or_else(NativeError::params)?;
        let origin = FontContainer::Archive {
            root_ordinal: archive.input().root_ordinal,
            archive_relative_path: archive.input().relative_path.clone(),
            archive_sha256: archive.snapshot_fingerprint().sha256.clone(),
            member_index: index,
            decoder_path: member.member().decoder_path.clone(),
            normalized_path: member.member().normalized_path.clone(),
        };
        Ok(Self {
            content_stamp: Some(crate::paths::content_stamp(&crate::paths::open_regular(member.path())?)?),
            path: member.path().to_owned(),
            fingerprint: member.fingerprint().clone(),
            origin,
            archive: Some(archive),
        })
    }
    pub(crate) fn matches_content_stamp(&self,file:&std::fs::File)->Result<bool,NativeError>{
        Ok(self.content_stamp.as_ref()==Some(&crate::paths::content_stamp(file)?))
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }
    pub(crate) fn origin(&self) -> &FontContainer {
        &self.origin
    }
    pub(crate) fn archive(&self) -> Option<&Arc<PreparedArchive>> {
        self.archive.as_ref()
    }
    pub(crate) fn key(&self) -> FontSourceKey {
        FontSourceKey::container(&self.origin)
    }
}

// A wrapper keeps variant construction inside this logical owner as well.
pub(crate) struct FontInput(FontInputKind);
pub(crate) enum FontInputKind {
    Loose(FontSourceFile),
    Vpk {
        candidate_id: String,
        directory: FontSourceFile,
        companions: BTreeMap<u16, FontSourceFile>,
    },
    Unavailable {
        origin: FontContainer,
        filename: String,
        reason: String,
    },
}
impl FontInput {
    pub(super) fn loose(file: FontSourceFile) -> Self {
        Self(FontInputKind::Loose(file))
    }
    pub(super) fn vpk(
        candidate_id: String,
        directory: FontSourceFile,
        companions: BTreeMap<u16, FontSourceFile>,
    ) -> Self {
        Self(FontInputKind::Vpk {
            candidate_id,
            directory,
            companions,
        })
    }
    pub(super) fn unavailable(
        origin: FontContainer,
        filename: String,
        reason: String,
    ) -> Self {
        Self(FontInputKind::Unavailable {
            origin,
            filename,
            reason,
        })
    }
    pub(crate) fn kind(&self) -> &FontInputKind {
        &self.0
    }
}

use super::{fp_json, stale, Found, LocalCandidate, LocalPrepared, Scan};
use crate::{
    archive_content::{
        ArchiveContentRole, ArchiveInput, ArchiveMemberSelector, ArchiveRegularMember,
        ArchiveSelection,
    },
    font_content::{self, FontAdmission, FontBudget, FontCandidate, FontOrigin},
    fonts,
    prepare_control::PrepareControl,
};
use json::{object, JsonValue};
use std::collections::BTreeSet;

pub(super) struct PendingFont {
    pub found: Found,
    pub reason: Option<String>,
}
pub(super) struct Selector<'a> {
    pub budget: &'a mut FontBudget,
}
impl ArchiveMemberSelector for Selector<'_> {
    fn select(
        &mut self,
        input: &ArchiveInput,
        m: &ArchiveRegularMember,
    ) -> Result<ArchiveSelection, NativeError> {
        let lower = m.normalized_path.to_ascii_lowercase();
        if lower.ends_with(".vpk") {
            return Ok(ArchiveSelection::Extract(ArchiveContentRole::Vpk));
        }
        if font_content::bundled_ttf(&m.normalized_path) {
            let origin = FontContainer::Archive {
                root_ordinal: input.root_ordinal,
                archive_relative_path: input.relative_path.clone(),
                archive_sha256: input.fingerprint.sha256.clone(),
                member_index: m.index,
                decoder_path: m.decoder_path.clone(),
                normalized_path: m.normalized_path.clone(),
            };
            return Ok(
                match self
                    .budget
                    .admit(&FontSourceKey::container(&origin), m.size)?
                {
                    FontAdmission::Available => {
                        ArchiveSelection::Extract(ArchiveContentRole::BundledTtf)
                    }
                    FontAdmission::Unavailable(_) => {
                        ArchiveSelection::Exclude(if m.size > font_content::MAX_FONT_BYTES {
                            "fontTooLarge"
                        } else {
                            "fontBudgetExceeded"
                        })
                    }
                },
            );
        }
        Ok(ArchiveSelection::Exclude(
            if [".zip", ".rar", ".7z"].iter().any(|e| lower.ends_with(e)) {
                "nestedArchiveNotExpanded"
            } else {
                "notVpk"
            },
        ))
    }
}
pub(super) fn discover(
    scan: &mut Scan<'_>,
    archives: &[Arc<PreparedArchive>],
    candidates: &[LocalCandidate],
    fingerprints: &BTreeMap<PathBuf, Option<Fingerprint>>,
    lease: Arc<crate::archive_quarantine::QuarantineLease>,
) -> Result<(Vec<FontCandidate>, Vec<FontSourceFile>), NativeError> {
    let mut inputs = Vec::new();
    let mut sources = Vec::new();
    for pending in &scan.fonts {
        let f = &pending.found;
        let origin = FontContainer::Raw {
            root_ordinal: f.ordinal,
            relative_path: f.relative.clone(),
        };
        if let Some(reason) = &pending.reason {
            inputs.push(FontInput::unavailable(
                origin,
                f.name().into(),
                reason.clone(),
            ));
        } else {
            let source = FontSourceFile::raw(
                f.path.clone(),
                fingerprints
                    .get(&f.path)
                    .and_then(Option::as_ref)
                    .ok_or_else(stale)?
                    .clone(),
                f.ordinal,
                f.relative.clone(),
            );
            sources.push(source.clone());
            inputs.push(FontInput::loose(source));
        }
    }
    for archive in archives {
        for member in archive.members() {
            if !font_content::bundled_ttf(&member.normalized_path) {
                continue;
            }
            if let Some(extracted) = archive.extracted_members().iter().find(|m| {
                m.member().index == member.index && m.role() == ArchiveContentRole::BundledTtf
            }) {
                let source = FontSourceFile::archived(archive.clone(), extracted.member().index)?;
                sources.push(source.clone());
                inputs.push(FontInput::loose(source));
            } else if let crate::archive_content::ArchiveKind::Rejected(reason) = &member.kind {
                let origin = FontContainer::Archive {
                    root_ordinal: archive.input().root_ordinal,
                    archive_relative_path: archive.input().relative_path.clone(),
                    archive_sha256: archive.snapshot_fingerprint().sha256.clone(),
                    member_index: member.index,
                    decoder_path: member.decoder_path.clone(),
                    normalized_path: member.normalized_path.clone(),
                };
                inputs.push(FontInput::unavailable(
                    origin,
                    member.normalized_path.rsplit('/').next().unwrap().into(),
                    reason.clone(),
                ));
            }
        }
    }
    for candidate in candidates {
        let Some(package) = &candidate.package else {
            continue;
        };
        let file = |path: &Path| -> Result<FontSourceFile, NativeError> {
            if let Some((archive, member)) = archives.iter().find_map(|a| {
                a.extracted_members()
                    .iter()
                    .find(|m| m.path() == path)
                    .map(|m| (a, m))
            }) {
                FontSourceFile::archived(archive.clone(), member.member().index)
            } else {
                let found = scan.sources.get(path).ok_or_else(stale)?;
                Ok(FontSourceFile::raw(
                    path.into(),
                    fingerprints
                        .get(path)
                        .and_then(Option::as_ref)
                        .ok_or_else(stale)?
                        .clone(),
                    found.ordinal,
                    found.relative.clone(),
                ))
            }
        };
        let directory = file(&package.directory.source_path)?;
        let companions = package
            .companions
            .iter()
            .map(|(i, a)| Ok((*i, file(&a.source_path)?)))
            .collect::<Result<BTreeMap<_, _>, NativeError>>()?;
        sources.push(directory.clone());
        sources.extend(companions.values().cloned());
        inputs.push(FontInput::vpk(candidate.id.clone(), directory, companions));
    }
    let candidates = font_content::discover_fonts(
        &inputs,
        lease,
        scan.font_budget.as_mut().ok_or_else(stale)?,
        scan.control,
    )?;
    Ok((candidates, sources))
}
pub(super) fn evidence(candidates: &[FontCandidate], sources: &[FontSourceFile]) -> JsonValue {
    object! {candidates:JsonValue::from(candidates.iter().map(fonts::candidate_json).collect::<Vec<_>>()),sources:JsonValue::from(sources.iter().map(|s|object!{origin:fonts::container_json(s.origin()),fingerprint:fp_json(s.path(),s.fingerprint())}).collect::<Vec<_>>()),staged:JsonValue::from(candidates.iter().filter_map(|c|c.prepared.as_ref()).map(|f|fp_json(f.path(),f.fingerprint())).collect::<Vec<_>>())}
}
pub(super) fn revalidate(
    prepared: &LocalPrepared,
    control: &PrepareControl,
) -> Result<(), NativeError> {
    if prepared.evidence["fonts"] != evidence(&prepared.fonts, &prepared.font_sources) {
        return Err(stale());
    }
    let mut seen = BTreeSet::new();
    for source in prepared.font_sources.iter() {
        // The typed preparation map was already checked by archive_sources.
        if prepared.fingerprints.get(source.path()) != Some(source.fingerprint()) {
            return Err(stale());
        }
        seen.insert(source.path());
    }
    for font in prepared.fonts.iter().filter_map(|c| c.prepared.as_ref()) {
        font.lease().validate_sealed()?;
        if seen.insert(font.path()) {
            super::archive_sources::verify_fingerprint(font.path(), font.fingerprint(), control)?;
        }
    }
    Ok(())
}
/// Real contributor coordinates are compared as typed values, never reopened.
pub(super) fn contributing_archives(
    rows: &[FontCandidate],
) -> Result<Vec<Arc<PreparedArchive>>, NativeError> {
    let mut archives = Vec::new();
    for row in rows.iter().filter(|r| r.prepared.is_some()) {
        let mut containers = Vec::new();
        for origin in &row.origins {
            match origin {
                FontOrigin::Loose { container } => containers.push(container.clone()),
                FontOrigin::Vpk { members, .. } => {
                    containers.extend(members.iter().map(|m| m.container.clone()))
                }
                FontOrigin::UnavailableVpk { .. } => {}
            }
        }
        for container in containers {
            let source = row
                .prepared
                .as_ref()
                .unwrap()
                .source_files()
                .iter()
                .find(|s| *s.origin() == container)
                .ok_or_else(stale)?;
            if let Some(archive) = source.archive() {
                if !archives.iter().any(|a| Arc::ptr_eq(a, archive)) {
                    archives.push(archive.clone());
                }
            }
        }
    }
    Ok(archives)
}
pub(super) fn bind_refs(
    rows: &mut JsonValue,
    archives: &[Arc<PreparedArchive>],
    refs: &[crate::package::FileRef],
) -> Result<(), NativeError> {
    fn bind(
        c: &mut JsonValue,
        archives: &[Arc<PreparedArchive>],
        refs: &[crate::package::FileRef],
    ) -> Result<(), NativeError> {
        if c["kind"] != "archive" {
            return Ok(());
        }
        let (_, file) = archives
            .iter()
            .zip(refs)
            .find(|(a, _)| {
                c["rootOrdinal"].as_usize() == Some(a.input().root_ordinal)
                    && c["archiveRelativePath"] == a.input().relative_path
                    && c["archiveSha256"] == a.snapshot_fingerprint().sha256
            })
            .ok_or_else(stale)?;
        c["archiveId"] = file.id.clone().into();
        Ok(())
    }
    for row in rows.members_mut() {
        for origin in row["origins"].members_mut() {
            bind(&mut origin["container"], archives, refs)?;
            for member in origin["members"].members_mut() {
                bind(&mut member["container"], archives, refs)?;
            }
        }
    }
    Ok(())
}
