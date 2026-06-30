use crate::system::capabilities::terminal_link::{
    TerminalLinkAccess, TerminalLinkTarget, resolve_terminal_file,
};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct LocalTerminalLinkAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalTerminalLinkAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        let root = PathBuf::from(&workspace.root.absolute);
        Self { workspace, root }
    }

    fn local_path(&self, path: &WorkspacePath) -> Result<PathBuf, String> {
        if let Some(relative) = path
            .relative
            .as_deref()
            .filter(|relative| !relative.is_empty())
        {
            let local_path = self.root.join(relative);
            return if local_path.starts_with(&self.root) {
                Ok(local_path)
            } else {
                Err("Path is outside the workspace.".to_string())
            };
        }

        Ok(PathBuf::from(&path.absolute))
    }
}

impl TerminalLinkAccess for LocalTerminalLinkAccess {
    fn resolve_file(&self, launch_dir: &str, target: &str) -> Result<TerminalLinkTarget, String> {
        let resolved = resolve_terminal_file(&self.workspace.root, launch_dir, target, |path| {
            self.local_path(path)
                .map(|path| path.exists())
                .unwrap_or(false)
        })?;
        log::info!(
            "local terminal link resolved target={} launch_dir={} resolved={}",
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
