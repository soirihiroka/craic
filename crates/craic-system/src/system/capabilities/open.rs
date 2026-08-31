use crate::system::path::FileNodePath;
use craic_platform::UiEffect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopOpenTargetKind {
    File,
    Folder,
}

pub trait DesktopOpenAccess: Send + Sync {
    fn resolve_open_path(
        &self,
        path: &FileNodePath,
        kind: DesktopOpenTargetKind,
    ) -> Result<UiEffect, String>;
    fn resolve_reveal_path(&self, path: &FileNodePath) -> Result<UiEffect, String>;
}
