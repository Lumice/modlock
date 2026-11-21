use crate::{local_job_store::LocalJobStore, paths};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// All filesystem mutation, including deliberately damaged recovery evidence,
/// is confined to this uniquely created disposable directory.
pub struct Fixture {
    pub root: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("archive-boundary-{}", paths::random_id().unwrap()));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
    pub fn store(&self) -> LocalJobStore {
        let root = self.root.join("native");
        fs::create_dir_all(&root).unwrap();
        LocalJobStore::open(&root, true).unwrap().unwrap()
    }
    pub fn archives(&self) {
        command(
            Command::new("python3")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/archive_boundary/fixtures.py"),
                )
                .arg(&self.root),
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
pub fn command(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Consumer isolation fixture: real sealed files and registry claims, without
/// executing or substituting the production decoder. Not worker qualification.
pub(crate) fn sealed_archive(
    lease: std::sync::Arc<crate::archive_quarantine::QuarantineLease>,
    original: &Path,
    root_ordinal: usize,
    relative_path: &str,
    members: &[(&str, &[u8])],
) -> std::sync::Arc<super::PreparedArchive> {
    use super::{ArchiveContentRole, ArchiveInput, ArchiveKind, ArchiveMember, ExtractedMember, PreparedArchive};
    use std::io::Write;
    let control = crate::prepare_control::PrepareControl::new();
    let mut claims = lease
        .allocate(members.len() + 1, &control)
        .unwrap()
        .into_iter();
    let mut snapshot = lease.open_claim(claims.next().unwrap()).unwrap();
    snapshot
        .file
        .write_all(&fs::read(original).unwrap())
        .unwrap();
    snapshot.file.sync_all().unwrap();
    let mut names = vec![snapshot.name];
    let mut extracted = Vec::new();
    let mut manifest = Vec::new();
    for (index, ((name, bytes), claim)) in members.iter().zip(claims).enumerate() {
        let mut output = lease.open_claim(claim).unwrap();
        output.file.write_all(bytes).unwrap();
        output.file.sync_all().unwrap();
        names.push(output.name);
        let member = ArchiveMember {
            index: index as u32,
            decoder_path: (*name).into(),
            normalized_path: (*name).into(),
            size: bytes.len() as u64,
            kind: ArchiveKind::Regular,
        };
        manifest.push(member.clone());
        extracted.push(ExtractedMember {
            role: ArchiveContentRole::Vpk,
            member,
            fingerprint: paths::fingerprint(&output.path).unwrap(),
            path: output.path,
        });
    }
    lease.complete_batch(&names).unwrap();
    std::sync::Arc::new(PreparedArchive {
        input: ArchiveInput {
            root_ordinal,
            relative_path: relative_path.into(),
            original_path: original.into(),
            fingerprint: paths::fingerprint(original).unwrap(),
        },
        snapshot_fingerprint: paths::fingerprint(&snapshot.path).unwrap(),
        snapshot_path: snapshot.path,
        format: "Zip".into(),
        encoding: "UTF-8".into(),
        manifest,
        extracted,
        listing: b"consumer-isolation".to_vec(),
        lease,
    })
}

pub(crate) fn with_exclusion(
    mut archive: std::sync::Arc<super::PreparedArchive>,
    name: &str,
) -> std::sync::Arc<super::PreparedArchive> {
    std::sync::Arc::get_mut(&mut archive)
        .unwrap()
        .manifest
        .push(super::ArchiveMember {
            index: 1000,
            decoder_path: name.into(),
            normalized_path: name.into(),
            size: 8,
            kind: super::ArchiveKind::Rejected("notVpk".into()),
        });
    archive
}
