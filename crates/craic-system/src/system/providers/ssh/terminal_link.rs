use super::{SshCommandRunner, remote_workspace_path, shell_quote};
use crate::system::capabilities::terminal_link::{
    TerminalLinkAccess, TerminalLinkTarget, resolve_terminal_file,
};
use crate::system::path::{WorkspacePath, WorkspaceRef};

#[derive(Clone, Debug)]
pub struct SshTerminalLinkAccess {
    workspace: WorkspaceRef,
    runner: SshCommandRunner,
}

impl SshTerminalLinkAccess {
    pub fn new(workspace: WorkspaceRef, runner: SshCommandRunner) -> Self {
        Self { workspace, runner }
    }

    fn remote_path_exists(&self, path: &WorkspacePath) -> bool {
        let remote_path = remote_workspace_path(&self.workspace, path);
        let script = format!("test -e {}", shell_quote(&remote_path));
        self.runner
            .run_script("terminal link path exists", &script)
            .is_ok()
    }
}

impl TerminalLinkAccess for SshTerminalLinkAccess {
    fn resolve_file(&self, launch_dir: &str, target: &str) -> Result<TerminalLinkTarget, String> {
        let resolved = resolve_terminal_file(&self.workspace.root, launch_dir, target, |path| {
            self.remote_path_exists(path)
        })?;
        log::info!(
            "ssh terminal link resolved workspace={} target={} launch_dir={} resolved={}",
            self.workspace.display_name,
            target,
            launch_dir,
            terminal_link_target_display(&resolved)
        );
        Ok(resolved)
    }
}

fn terminal_link_target_display(target: &TerminalLinkTarget) -> &str {
    match target {
        TerminalLinkTarget::Workspace(path) => path.display(),
        TerminalLinkTarget::External(path) => &path.absolute,
    }
}
