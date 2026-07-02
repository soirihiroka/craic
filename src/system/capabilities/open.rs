use crate::system::path::FileNodePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenTargetKind {
    File,
    Folder,
}

pub(crate) trait OpenAccess: Send + Sync {
    fn copyable_path(&self, path: &FileNodePath) -> String;
    fn open_path(&self, path: &FileNodePath, kind: OpenTargetKind) -> Result<String, String>;
    fn reveal_path(&self, path: &FileNodePath) -> Result<String, String>;
    fn open_url(&self, url: &str) -> Result<String, String>;
}
