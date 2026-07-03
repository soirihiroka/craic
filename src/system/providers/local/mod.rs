mod docker;
mod files;
mod git;
mod github;
mod open;
mod shell;
mod terminal_link;

use self::docker::LocalDockerAccess;
use self::files::LocalFileAccess;
use self::git::LocalGitAccess;
use self::github::LocalGitHubAccess;
use self::open::LocalDesktopOpenAccess;
use self::shell::LocalShellAccess;
use self::terminal_link::LocalTerminalLinkAccess;
use super::url::GioUrlOpenAccess;
use crate::system::capabilities::github::GitHubAccess;
use crate::system::capabilities::{
    docker::DockerAccess, files::FileAccess, git::GitAccess, open::DesktopOpenAccess,
    shell::ShellAccess, terminal_link::TerminalLinkAccess, url::UrlOpenAccess,
};
use crate::system::path::{
    ProviderKind, SystemId, SystemRef, WorkspacePath, WorkspaceRef, path_display_name,
    pathbuf_to_target_absolute, workspace_id_for_absolute_path,
};
use crate::system::provider::{
    ProviderWorkspaceEntry, ProviderWorkspaceGitStatus, ProviderWorkspaceListRequest,
    ProviderWorkspaceRemote, ProviderWorkspaceSource, SystemProvider,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct LocalProvider {
    system: SystemRef,
}

impl LocalProvider {
    pub(crate) fn new() -> Self {
        Self {
            system: SystemRef::new(SystemId::new("local"), ProviderKind::Local, None),
        }
    }

    pub(crate) fn system_ref(&self) -> SystemRef {
        self.system.clone()
    }

    pub(crate) fn workspace_for_path(path: &Path) -> WorkspaceRef {
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
            let Some(path) = crate::config::expand_config_path_for_ui(&configured_path) else {
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
            let Some(root_path) = crate::config::expand_config_path_for_ui(&root) else {
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
        )))
    }

    fn git(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitAccess>> {
        let root = PathBuf::from(&workspace.root.absolute);
        if crate::git::root_for_path(&root).is_none() {
            log::debug!(
                "local git capability unavailable workspace={} root={}",
                workspace.display_name,
                workspace.root.absolute
            );
            return None;
        }

        log::debug!(
            "creating local git capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalGitAccess::new(workspace.clone())))
    }

    fn github(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitHubAccess>> {
        if !command_exists("gh") {
            log::debug!(
                "local github capability unavailable workspace={} root={} reason=missing-gh",
                workspace.display_name,
                workspace.root.absolute
            );
            return None;
        }

        log::debug!(
            "creating local github capability workspace={} root={}",
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(LocalGitHubAccess::new(workspace.clone())))
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
    let git = local_git_status(&path);
    ProviderWorkspaceEntry {
        display_name: path_display_name(&path),
        path: pathbuf_to_target_absolute(path),
        source,
        git,
    }
}

fn local_git_status(path: &Path) -> ProviderWorkspaceGitStatus {
    if !git_success(path, &["rev-parse", "--is-inside-work-tree"]) {
        return ProviderWorkspaceGitStatus::NotRepo;
    }

    let remote_name = local_remote_name(path);
    let remote = remote_name
        .as_deref()
        .and_then(|remote| git_output(path, &["remote", "get-url", remote]).ok())
        .filter(|url| !url.is_empty())
        .map(|url| ProviderWorkspaceRemote {
            name: remote_name,
            host: remote_host(&url),
            slug: remote_slug(&url),
            url,
        });

    ProviderWorkspaceGitStatus::Repo { remote }
}

fn local_remote_name(path: &Path) -> Option<String> {
    git_output(
        path,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()
    .and_then(|upstream| upstream.split('/').next().map(ToString::to_string))
    .filter(|remote| !remote.is_empty())
    .or_else(|| git_success(path, &["remote", "get-url", "origin"]).then(|| "origin".to_string()))
    .or_else(|| {
        git_output(path, &["remote"])
            .ok()
            .and_then(|remotes| remotes.lines().next().map(ToString::to_string))
            .filter(|remote| !remote.is_empty())
    })
}

fn git_success(path: &Path, args: &[&str]) -> bool {
    crate::git::run_git_success(path, args).unwrap_or(false)
}

fn git_output(path: &Path, args: &[&str]) -> Result<String, String> {
    crate::git::run_git(path, args)
}

fn remote_slug(remote_url: &str) -> Option<String> {
    crate::github::parse_github_url(remote_url)
        .or_else(|| crate::gitlab::parse_gitlab_url(remote_url))
        .or_else(|| crate::bitbucket::parse_bitbucket_url(remote_url))
}

fn remote_host(remote_url: &str) -> Option<String> {
    let remote_url = remote_url.trim();
    if let Some(rest) = remote_url.strip_prefix("git@") {
        return rest.split_once(':').map(|(host, _)| host.to_string());
    }
    if let Some((_, rest)) = remote_url.split_once("://") {
        return rest
            .split('/')
            .next()
            .filter(|host| !host.is_empty())
            .map(|host| host.trim_start_matches("git@").to_string());
    }
    None
}

fn command_exists(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}
