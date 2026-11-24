#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalActivation {
    Url(String),
    File(TerminalFileActivation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFileActivation {
    pub target: String,
    pub launch_dir: String,
}
