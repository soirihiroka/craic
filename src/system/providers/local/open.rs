use crate::system::capabilities::open::{
    DesktopOpenAccess, DesktopOpenActivation, DesktopOpenTargetKind,
};
use crate::system::path::{FileNodePath, WorkspacePath, WorkspaceRef};
use gtk::gio;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) struct LocalDesktopOpenAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalDesktopOpenAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        let root = PathBuf::from(&workspace.root.absolute);
        Self { workspace, root }
    }

    fn workspace_path(&self, path: &FileNodePath) -> Result<WorkspacePath, String> {
        path.to_workspace_path(&self.workspace)
            .ok_or_else(|| "Opening virtual or external file nodes is unavailable.".to_string())
    }

    fn local_path_for_workspace(&self, path: &WorkspacePath) -> Result<PathBuf, String> {
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

    fn local_path(&self, path: &FileNodePath) -> Result<PathBuf, String> {
        let workspace_path = self.workspace_path(path)?;
        self.local_path_for_workspace(&workspace_path)
    }

    fn launch_file(&self, local_path: &PathBuf, activation: DesktopOpenActivation) {
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        let file = gio::File::for_path(local_path);
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.launch(
            activation.parent.as_ref(),
            None::<&gio::Cancellable>,
            move |result| match result {
                Ok(()) => log::info!(
                    "local file launcher complete workspace={} path={}",
                    workspace_name,
                    path_display
                ),
                Err(err) => log::warn!(
                    "local file launcher failed workspace={} path={}: {}",
                    workspace_name,
                    path_display,
                    err
                ),
            },
        );
    }

    fn open_containing_folder(&self, local_path: &PathBuf, activation: DesktopOpenActivation) {
        // Use GtkFileLauncher instead of calling FileManager1 directly. GTK
        // routes through the portal/native implementation and supplies the
        // parent-window activation data Wayland compositors require for focus.
        let path_display = local_path.display().to_string();
        let workspace_name = self.workspace.display_name.clone();
        let file = gio::File::for_path(local_path);
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.open_containing_folder(
            activation.parent.as_ref(),
            None::<&gio::Cancellable>,
            move |result| match result {
                Ok(()) => log::info!(
                    "local file reveal complete workspace={} path={}",
                    workspace_name,
                    path_display
                ),
                Err(err) => log::warn!(
                    "local file reveal failed workspace={} path={}: {}",
                    workspace_name,
                    path_display,
                    err
                ),
            },
        );
    }

    fn open_path_in_file_manager(
        &self,
        local_path: &PathBuf,
        activation: DesktopOpenActivation,
    ) -> Result<(), String> {
        let metadata = local_path
            .metadata()
            .map_err(|err| format!("Unable to inspect {}: {err}", local_path.display()))?;
        if metadata.is_dir() {
            self.launch_file(local_path, activation);
        } else {
            self.open_containing_folder(local_path, activation);
        }
        Ok(())
    }
}

impl DesktopOpenAccess for LocalDesktopOpenAccess {
    fn open_path(
        &self,
        path: &FileNodePath,
        kind: DesktopOpenTargetKind,
        activation: DesktopOpenActivation,
    ) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        log::info!(
            "local open path start workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        if kind == DesktopOpenTargetKind::Folder {
            self.open_path_in_file_manager(&local_path, activation)?;
            return Ok("Opened path.".to_string());
        }

        self.launch_file(&local_path, activation);
        Ok("Opened path.".to_string())
    }

    fn reveal_path(
        &self,
        path: &FileNodePath,
        activation: DesktopOpenActivation,
    ) -> Result<String, String> {
        let local_path = self.local_path(path)?;
        log::info!(
            "local reveal path start workspace={} path={}",
            self.workspace.display_name,
            local_path.display()
        );
        self.open_containing_folder(&local_path, activation);
        Ok("Opened containing folder.".to_string())
    }
}
