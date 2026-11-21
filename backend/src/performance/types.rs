#[derive(Debug, Clone)]
pub struct ResolvedEntry {
    pub path: Vec<String>,
    pub value: Option<String>,
}
#[derive(Debug, Clone)]
pub struct AppliedOverlay {
    pub config_id: String,
    pub rev: String,
    pub line_count: u32,
    pub hand_edited: Vec<Vec<String>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignTool {
    Grimoire,
    Deadtune,
    SqookyUpdater,
    GameinfoEditor,
}
#[derive(Debug, Clone)]
pub struct ForeignOverlay {
    pub tool: ForeignTool,
    pub label: Option<String>,
    pub line_count: u32,
}
