use crate::system::path::WorkspacePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenTargetKind {
    File,
    Folder,
}

pub(crate) trait OpenAccess: Send + Sync {
    fn copyable_path(&self, path: &WorkspacePath) -> String;
    fn open_path(&self, path: &WorkspacePath, kind: OpenTargetKind) -> Result<String, String>;
    fn reveal_path(&self, path: &WorkspacePath) -> Result<String, String>;
    fn open_url(&self, url: &str) -> Result<String, String>;
}
