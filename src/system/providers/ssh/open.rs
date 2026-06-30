use crate::system::capabilities::open::{OpenAccess, OpenTargetKind};
use crate::system::path::{WorkspacePath, WorkspaceRef};

#[derive(Clone, Debug)]
pub(crate) struct SshOpenAccess {
    workspace: WorkspaceRef,
    label: String,
}

impl SshOpenAccess {
    pub(crate) fn new(workspace: WorkspaceRef, label: String) -> Self {
        Self { workspace, label }
    }
}

impl OpenAccess for SshOpenAccess {
    fn copyable_path(&self, path: &WorkspacePath) -> String {
        format!("{}:{}", self.label, path.absolute)
    }

    fn open_path(&self, _path: &WorkspacePath, _kind: OpenTargetKind) -> Result<String, String> {
        Err(format!(
            "Opening remote desktop paths is unavailable for SSH workspace {}.",
            self.workspace.display_name
        ))
    }

    fn reveal_path(&self, _path: &WorkspacePath) -> Result<String, String> {
        Err(format!(
            "Revealing remote desktop paths is unavailable for SSH workspace {}.",
            self.workspace.display_name
        ))
    }

    fn open_url(&self, _url: &str) -> Result<String, String> {
        Err("Opening URLs from SSH workspaces is not wired yet.".to_string())
    }
}
