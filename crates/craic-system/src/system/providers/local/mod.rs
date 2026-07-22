mod docker;
mod files;
mod open;
mod shell;
mod terminal_link;

use self::docker::LocalDockerAccess;
use self::files::{LocalFileAccess, LocalFileWatchService};
use self::open::LocalDesktopOpenAccess;
use self::shell::LocalShellAccess;
use self::terminal_link::LocalTerminalLinkAccess;
use super::url::GioUrlOpenAccess;
use crate::system::capabilities::{
    docker::DockerAccess, files::FileAccess, open::DesktopOpenAccess, shell::ShellAccess,
    terminal_link::TerminalLinkAccess, url::UrlOpenAccess,
};
use crate::system::path::{
    ProviderKind, SystemId, SystemRef, WorkspacePath, WorkspaceRef, path_display_name,
    pathbuf_to_target_absolute, workspace_id_for_absolute_path,
};
use crate::system::provider::{
    ProviderWorkspaceEntry, ProviderWorkspaceListRequest, ProviderWorkspaceSource, SystemProvider,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug)]
pub struct LocalProvider {
    system: SystemRef,
    file_watch_service: Arc<OnceLock<Arc<LocalFileWatchService>>>,
}

impl LocalProvider {
    pub fn new() -> Self {
        Self {
            system: SystemRef::new(SystemId::new("local"), ProviderKind::Local, None),
            file_watch_service: Arc::new(OnceLock::new()),
        }
    }

    pub fn system_ref(&self) -> SystemRef {
        self.system.clone()
    }

    pub fn workspace_for_path(path: &Path) -> WorkspaceRef {
        let root = canonical_or_original(path);
        WorkspaceRef::new(
            workspace_id_for_absolute_path(&root),
            WorkspacePath::from_absolute(pathbuf_to_target_absolute(root.clone())),
            path_display_name(&root),
        )
    }
}

impl Default for LocalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemProvider for LocalProvider {
    fn id(&self) -> SystemId {
        self.system.id.clone()
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Local
    }

    fn label(&self) -> String {
        "Local machine".to_string()
    }

    fn list_workspaces(
        &self,
        request: ProviderWorkspaceListRequest,
    ) -> Result<Vec<ProviderWorkspaceEntry>, String> {
        let mut workspaces = Vec::new();
        for configured_path in request.workspace_paths {
            let Some(path) = craic_config::expand_config_path_for_ui(&configured_path) else {
                continue;
            };
            if path.is_dir() {
                workspaces.push(local_workspace_entry(
                    path,
                    ProviderWorkspaceSource::Workspace {
                        path: configured_path,
                    },
                ));
            }
        }
        for root in request.root_paths {
            let Some(root_path) = craic_config::expand_config_path_for_ui(&root) else {
                continue;
            };
            let entries = match std::fs::read_dir(&root_path) {
                Ok(entries) => entries,
                Err(err) => {
                    log::warn!(
                        "local workspace root bulk listing failed root={}: {err}",
                        root_path.display()
                    );
                    continue;
                }
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    workspaces.push(local_workspace_entry(
                        path,
                        ProviderWorkspaceSource::Root { path: root.clone() },
                    ));
                }
            }
        }
        workspaces.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
        });
        log::debug!(
            "local workspace bulk list workspaces={} roots={} count={}",
            workspaces
                .iter()
                .filter(|entry| matches!(entry.source, ProviderWorkspaceSource::Workspace { .. }))
                .count(),
            workspaces
                .iter()
                .filter(|entry| matches!(entry.source, ProviderWorkspaceSource::Root { .. }))
                .count(),
            workspaces.len()
        );
        Ok(workspaces)
    }

    fn files(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn FileAccess>> {
        log::debug!(
            "creating local files capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalFileAccess::new(
            self.system.clone(),
            workspace.clone(),
            self.file_watch_service
                .get_or_init(LocalFileWatchService::new)
                .clone(),
        )))
    }

    fn shell(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn ShellAccess>> {
        log::debug!(
            "creating local shell capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalShellAccess::new(workspace.clone())))
    }

    fn docker(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn DockerAccess>> {
        if !command_exists("docker") {
            log::debug!(
                "local docker capability unavailable workspace={} root={} reason=missing-docker",
                workspace.display_name,
                workspace.root.absolute
            );
            return None;
        }

        log::debug!(
            "creating local docker capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalDockerAccess::new(workspace.clone())))
    }

    fn desktop_opener(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn DesktopOpenAccess>> {
        log::debug!(
            "creating local desktop-open capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalDesktopOpenAccess::new(workspace.clone())))
    }

    fn url_opener(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn UrlOpenAccess>> {
        log::debug!(
            "creating local url-open capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(GioUrlOpenAccess::new(
            self.label(),
            workspace.clone(),
            None,
        )))
    }

    fn terminal_links(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn TerminalLinkAccess>> {
        log::debug!(
            "creating local terminal-link capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalTerminalLinkAccess::new(workspace.clone())))
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn local_workspace_entry(path: PathBuf, source: ProviderWorkspaceSource) -> ProviderWorkspaceEntry {
    let path = path.canonicalize().unwrap_or(path);
    ProviderWorkspaceEntry {
        display_name: path_display_name(&path),
        path: pathbuf_to_target_absolute(path),
        source,
    }
}

fn command_exists(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}
