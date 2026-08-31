use crate::system::capabilities::open::{DesktopOpenAccess, DesktopOpenTargetKind};
use crate::system::path::{FileNodePath, WorkspacePath, WorkspaceRef};
use craic_platform::{OpenPathKind, OpenPathRequest, UiEffect};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct LocalDesktopOpenAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalDesktopOpenAccess {
    pub fn new(workspace: WorkspaceRef) -> Self {
        let root = PathBuf::from(&workspace.root.absolute);
        Self { workspace, root }
    }

    fn workspace_path(&self, path: &FileNodePath) -> Result<WorkspacePath, String> {
        path.to_workspace_path(&self.workspace)
            .ok_or_else(|| "Opening virtual or external file nodes is unavailable.".to_string())
    }

    fn local_path(&self, path: &FileNodePath) -> Result<PathBuf, String> {
        let path = self.workspace_path(path)?;
        let local_path = match path.relative.as_deref() {
            Some(relative) if !relative.is_empty() => self.root.join(relative),
            _ => PathBuf::from(path.absolute),
        };
        if local_path.starts_with(&self.root) {
            Ok(local_path)
        } else {
            Err("Path is outside the workspace.".to_string())
        }
    }
}

impl DesktopOpenAccess for LocalDesktopOpenAccess {
    fn resolve_open_path(
        &self,
        path: &FileNodePath,
        kind: DesktopOpenTargetKind,
    ) -> Result<UiEffect, String> {
        let local_path = self.local_path(path)?;
        log::debug!(
            "local open path resolved workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        Ok(UiEffect::OpenPath(OpenPathRequest {
            path: local_path,
            kind: match kind {
                DesktopOpenTargetKind::File => OpenPathKind::File,
                DesktopOpenTargetKind::Folder => OpenPathKind::Folder,
            },
        }))
    }

    fn resolve_reveal_path(&self, path: &FileNodePath) -> Result<UiEffect, String> {
        let local_path = self.local_path(path)?;
        log::debug!(
            "local reveal path resolved workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        Ok(UiEffect::RevealPath(local_path))
    }
}
