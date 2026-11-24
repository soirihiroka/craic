#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalActivation {
    Url(String),
    File(TerminalFileActivation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFileActivation {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub launch_dir: String,
}
