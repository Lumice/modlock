//! Consumer-only fixtures: production assembler with a real sealed typed archive.
use super::*;
pub(crate) fn assembled(archive: Arc<PreparedArchive>, root: &Path) -> LocalPrepared {
    let request = LocalPrepareRequest {
        root: root.into(),
        profile_id: "default".into(),
        expected_revision: 1,
        roots: vec![archive.input().original_path.clone()],
        artwork: None,
    };
    let control = PrepareControl::new();
    let mut budget = ArchiveBudget::new();
    budget.admit_roots(1).unwrap();
    let mut scan = Scan {
        control: &control,
        inventory: Vec::new(),
        provenance: Vec::new(),
        sources: BTreeMap::new(),
        bytes: 0,
        unique: HashSet::new(),
        archives: Vec::new(),
        budget: Some(budget),
        fonts: Vec::new(),
        font_budget: Some(crate::font_content::FontBudget::new()),
    };
    scan.entry(
        0,
        archive.input().relative_path.clone(),
        archive.input().original_path.clone(),
        0,
        true,
    )
    .unwrap();
    let lease=archive.lease().clone();
    assemble_prepared(request, &control, scan, vec![archive], Some(lease)).unwrap()
}
