use super::capabilities::github::GitHubAccess;
use super::capabilities::{
    docker::DockerAccess, files::FileAccess, git::GitAccess, open::DesktopOpenAccess,
    shell::ShellAccess, terminal_link::TerminalLinkAccess,
};
use super::path::{ProviderKind, SystemId, WorkspaceRef};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default)]
pub(crate) struct ProviderWorkspaceListRequest {
    pub(crate) workspace_paths: Vec<String>,
    pub(crate) root_paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderWorkspaceEntry {
    pub(crate) path: String,
    pub(crate) display_name: String,
    pub(crate) source: ProviderWorkspaceSource,
    pub(crate) git: ProviderWorkspaceGitStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderWorkspaceSource {
    Workspace { path: String },
    Root { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProviderWorkspaceRemote {
    pub(crate) name: Option<String>,
    pub(crate) url: String,
    pub(crate) host: Option<String>,
    pub(crate) slug: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderWorkspaceGitStatus {
    NotRepo,
    Repo {
        remote: Option<ProviderWorkspaceRemote>,
    },
}

pub(crate) trait SystemProvider: Send + Sync {
    fn id(&self) -> SystemId;
    fn kind(&self) -> ProviderKind;
    fn label(&self) -> String;

    fn list_workspaces(
        &self,
        request: ProviderWorkspaceListRequest,
    ) -> Result<Vec<ProviderWorkspaceEntry>, String>;
    fn files(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn FileAccess>>;
    fn git(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitAccess>>;
    fn github(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitHubAccess>>;
    fn shell(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn ShellAccess>>;
    fn docker(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn DockerAccess>>;
    fn desktop_opener(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn DesktopOpenAccess>>;
    fn terminal_links(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn TerminalLinkAccess>>;
}

#[derive(Clone, Default)]
pub(crate) struct SystemProviderRegistry {
    providers: Arc<RwLock<HashMap<SystemId, Arc<dyn SystemProvider>>>>,
    files: Arc<RwLock<HashMap<String, Arc<dyn FileAccess>>>>,
    git: Arc<RwLock<HashMap<String, Arc<dyn GitAccess>>>>,
    github: Arc<RwLock<HashMap<String, Arc<dyn GitHubAccess>>>>,
    shell: Arc<RwLock<HashMap<String, Arc<dyn ShellAccess>>>>,
    docker: Arc<RwLock<HashMap<String, Arc<dyn DockerAccess>>>>,
    desktop_opener: Arc<RwLock<HashMap<String, Arc<dyn DesktopOpenAccess>>>>,
    terminal_links: Arc<RwLock<HashMap<String, Arc<dyn TerminalLinkAccess>>>>,
}

impl SystemProviderRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register(&self, provider: Arc<dyn SystemProvider>) {
        let id = provider.id();
        log::info!(
            "registering system provider id={} kind={} label={}",
            id,
            provider.kind(),
            provider.label()
        );
        self.clear_cached_capabilities_for_system(&id);
        self.providers
            .write()
            .expect("system provider registry poisoned")
            .insert(id, provider);
    }

    pub(crate) fn provider(&self, id: &SystemId) -> Option<Arc<dyn SystemProvider>> {
        self.providers
            .read()
            .expect("system provider registry poisoned")
            .get(id)
            .cloned()
    }

    pub(crate) fn files(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn FileAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_file_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.files(workspace);
        log_capability_absence(access.is_some(), &provider.label(), workspace, "files");
        if let Some(access) = &access {
            self.files
                .write()
                .expect("system files cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn git(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn GitAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_git_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.git(workspace);
        log_capability_absence(access.is_some(), &provider.label(), workspace, "git");
        if let Some(access) = &access {
            self.git
                .write()
                .expect("system git cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn github(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn GitHubAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_github_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.github(workspace);
        log_capability_absence(access.is_some(), &provider.label(), workspace, "github");
        if let Some(access) = &access {
            self.github
                .write()
                .expect("system github cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn shell(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn ShellAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_shell_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.shell(workspace);
        log_capability_absence(access.is_some(), &provider.label(), workspace, "shell");
        if let Some(access) = &access {
            self.shell
                .write()
                .expect("system shell cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn docker(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn DockerAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_docker_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.docker(workspace);
        log_capability_absence(access.is_some(), &provider.label(), workspace, "docker");
        if let Some(access) = &access {
            self.docker
                .write()
                .expect("system docker cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn desktop_opener(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn DesktopOpenAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_desktop_open_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.desktop_opener(workspace);
        log_capability_absence(
            access.is_some(),
            &provider.label(),
            workspace,
            "desktop-open",
        );
        if let Some(access) = &access {
            self.desktop_opener
                .write()
                .expect("system desktop open cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    pub(crate) fn terminal_links(
        &self,
        system_id: &SystemId,
        workspace: &WorkspaceRef,
    ) -> Option<Arc<dyn TerminalLinkAccess>> {
        let key = capability_key(system_id, workspace);
        if let Some(access) = self.cached_terminal_link_access(&key) {
            return Some(access);
        }
        let provider = self.provider(system_id)?;
        let access = provider.terminal_links(workspace);
        log_capability_absence(
            access.is_some(),
            &provider.label(),
            workspace,
            "terminal-link",
        );
        if let Some(access) = &access {
            self.terminal_links
                .write()
                .expect("system terminal link cache poisoned")
                .insert(key, access.clone());
        }
        access
    }

    fn cached_file_access(&self, key: &str) -> Option<Arc<dyn FileAccess>> {
        self.files
            .read()
            .expect("system files cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_git_access(&self, key: &str) -> Option<Arc<dyn GitAccess>> {
        self.git
            .read()
            .expect("system git cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_github_access(&self, key: &str) -> Option<Arc<dyn GitHubAccess>> {
        self.github
            .read()
            .expect("system github cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_shell_access(&self, key: &str) -> Option<Arc<dyn ShellAccess>> {
        self.shell
            .read()
            .expect("system shell cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_docker_access(&self, key: &str) -> Option<Arc<dyn DockerAccess>> {
        self.docker
            .read()
            .expect("system docker cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_desktop_open_access(&self, key: &str) -> Option<Arc<dyn DesktopOpenAccess>> {
        self.desktop_opener
            .read()
            .expect("system desktop open cache poisoned")
            .get(key)
            .cloned()
    }

    fn cached_terminal_link_access(&self, key: &str) -> Option<Arc<dyn TerminalLinkAccess>> {
        self.terminal_links
            .read()
            .expect("system terminal link cache poisoned")
            .get(key)
            .cloned()
    }

    fn clear_cached_capabilities_for_system(&self, system_id: &SystemId) {
        let prefix = format!("{system_id}|");
        self.files
            .write()
            .expect("system files cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.git
            .write()
            .expect("system git cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.github
            .write()
            .expect("system github cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.shell
            .write()
            .expect("system shell cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.docker
            .write()
            .expect("system docker cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.desktop_opener
            .write()
            .expect("system desktop open cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
        self.terminal_links
            .write()
            .expect("system terminal link cache poisoned")
            .retain(|key, _| !key.starts_with(&prefix));
    }
}

fn capability_key(system_id: &SystemId, workspace: &WorkspaceRef) -> String {
    format!("{}|{}", system_id, workspace.id)
}

fn log_capability_absence(
    present: bool,
    provider_label: &str,
    workspace: &WorkspaceRef,
    capability: &str,
) {
    if !present {
        log::debug!(
            "capability unavailable provider={} workspace={} capability={}",
            provider_label,
            workspace.display_name,
            capability
        );
    }
}
