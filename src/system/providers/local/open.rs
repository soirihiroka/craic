use crate::system::capabilities::open::{OpenAccess, OpenTargetKind};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Debug)]
pub(crate) struct LocalOpenAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalOpenAccess {
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

impl OpenAccess for LocalOpenAccess {
    fn copyable_path(&self, path: &WorkspacePath) -> String {
        self.local_path(path)
            .unwrap_or_else(|_| PathBuf::from(&path.absolute))
            .display()
            .to_string()
    }

    fn open_path(&self, path: &WorkspacePath, _kind: OpenTargetKind) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        log::info!(
            "local open path start workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        Command::new("xdg-open")
            .arg(&local_path)
            .spawn()
            .map_err(|err| format!("Failed to open {}: {err}", local_path.display()))?;
        Ok("Opened path.".to_string())
    }

    fn reveal_path(&self, path: &WorkspacePath) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        let target = if local_path.is_dir() {
            local_path.clone()
        } else {
            local_path
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| self.root.clone())
        };
        log::info!(
            "local reveal path start workspace={} path={} target={}",
            self.workspace.display_name,
            local_path.display(),
            target.display()
        );
        Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map_err(|err| format!("Failed to reveal {}: {err}", local_path.display()))?;
        Ok("Opened containing folder.".to_string())
    }

    fn open_url(&self, url: &str) -> Result<String, String> {
        log::info!(
            "local open url start workspace={} url_len={}",
            self.workspace.display_name,
            url.len()
        );
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|err| format!("Failed to open URL: {err}"))?;
        Ok("Opened URL.".to_string())
    }
}
