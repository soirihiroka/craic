use crate::system::capabilities::open::{OpenAccess, OpenTargetKind};
use crate::system::path::{FileNodePath, WorkspacePath, WorkspaceRef};

#[derive(Clone, Debug)]
pub(crate) struct SshOpenAccess {
    workspace: WorkspaceRef,
    label: String,
}

impl SshOpenAccess {
    pub(crate) fn new(workspace: WorkspaceRef, label: String) -> Self {
        Self { workspace, label }
    }

    fn workspace_path(&self, path: &FileNodePath) -> Result<WorkspacePath, String> {
        path.to_workspace_path(&self.workspace)
            .ok_or_else(|| "Opening virtual or external file nodes is unavailable.".to_string())
    }
}

impl OpenAccess for SshOpenAccess {
    fn copyable_path(&self, path: &FileNodePath) -> String {
        path.to_workspace_path(&self.workspace)
            .map(|path| format!("{}:{}", self.label, path.absolute))
            .unwrap_or_else(|| path.display())
    }

    fn open_path(&self, path: &FileNodePath, _kind: OpenTargetKind) -> Result<String, String> {
        let _ = self.workspace_path(path)?;
        Err(format!(
            "Opening remote desktop paths is unavailable for SSH workspace {}.",
            self.workspace.display_name
        ))
    }

    fn reveal_path(&self, path: &FileNodePath) -> Result<String, String> {
        let _ = self.workspace_path(path)?;
        Err(format!(
            "Revealing remote desktop paths is unavailable for SSH workspace {}.",
            self.workspace.display_name
        ))
    }

    fn open_url(&self, _url: &str) -> Result<String, String> {
        Err("Opening URLs from SSH workspaces is not wired yet.".to_string())
    }
}
