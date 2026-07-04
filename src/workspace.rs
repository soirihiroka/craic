use crate::config::{ConfiguredWorkspace, WorkspaceProvider};
use crate::system::provider::{
    ProviderWorkspaceEntry, ProviderWorkspaceListRequest, ProviderWorkspaceSource, SystemProvider,
};
use crate::system::providers::local::LocalProvider;
use crate::system::providers::ssh::{SshProvider, SshProviderConfig};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceEntry {
    pub(crate) workspace: ConfiguredWorkspace,
    pub(crate) label: String,
}

impl WorkspaceEntry {
    pub(crate) fn selection_id(&self) -> String {
        self.workspace.selection_id()
    }

    pub(crate) fn is_local(&self) -> bool {
        self.workspace.provider.is_local()
    }

    pub(crate) fn local_path(&self) -> Option<PathBuf> {
        if self.is_local() {
            crate::config::expand_config_path_for_ui(&self.workspace.path)
        } else {
            None
        }
    }
}

pub(crate) fn discover_configured_workspaces() -> Vec<WorkspaceEntry> {
    let config = crate::config::load();
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut requests: HashMap<WorkspaceProvider, ProviderWorkspaceListRequest> = HashMap::new();
    let mut configured_workspaces: HashMap<(String, String), ConfiguredWorkspace> = HashMap::new();
    let mut configured_roots: HashMap<(String, String), ConfiguredWorkspace> = HashMap::new();

    for workspace in config.workspaces {
        requests
            .entry(workspace.provider.clone())
            .or_default()
            .workspace_paths
            .push(workspace.path.clone());
        configured_workspaces.insert(
            (workspace.provider_id(), workspace.path.clone()),
            workspace.clone(),
        );
    }

    for root in config.workspace_roots {
        requests
            .entry(root.provider.clone())
            .or_default()
            .root_paths
            .push(root.path.clone());
        configured_roots.insert((root.provider_id(), root.path.clone()), root.clone());
    }

    for (provider, request) in requests {
        let provider_id = provider.id();
        let listed = list_provider_workspaces(&provider, request).unwrap_or_else(|err| {
            log::warn!(
                "workspace bulk metadata failed provider={} err={}",
                provider_id,
                err.trim()
            );
            Vec::new()
        });
        let returned_workspace_sources = listed
            .iter()
            .filter_map(|entry| match &entry.source {
                ProviderWorkspaceSource::Workspace { path } => {
                    Some((provider_id.clone(), path.clone()))
                }
                ProviderWorkspaceSource::Root { .. } => None,
            })
            .collect::<HashSet<_>>();
        push_listed_workspaces(
            &mut entries,
            &mut seen,
            listed,
            provider,
            &configured_workspaces,
            &configured_roots,
        );
        push_missing_configured_workspaces(
            &mut entries,
            &mut seen,
            &provider_id,
            &configured_workspaces,
            &returned_workspace_sources,
        );
    }

    entries.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| {
                left.workspace
                    .provider_id()
                    .cmp(&right.workspace.provider_id())
            })
            .then_with(|| left.workspace.path.cmp(&right.workspace.path))
    });
    entries
}

fn push_listed_workspaces(
    entries: &mut Vec<WorkspaceEntry>,
    seen: &mut HashSet<String>,
    listed: Vec<ProviderWorkspaceEntry>,
    provider: WorkspaceProvider,
    configured_workspaces: &HashMap<(String, String), ConfiguredWorkspace>,
    configured_roots: &HashMap<(String, String), ConfiguredWorkspace>,
) {
    let provider_id = provider.id();
    for workspace in listed {
        let (display_name, color) = match &workspace.source {
            ProviderWorkspaceSource::Workspace { path } => configured_workspaces
                .get(&(provider_id.clone(), path.clone()))
                .map(|configured| {
                    (
                        configured
                            .display_name
                            .clone()
                            .unwrap_or_else(|| workspace.display_name.clone()),
                        configured.color.clone(),
                    )
                })
                .unwrap_or_else(|| (workspace.display_name.clone(), None)),
            ProviderWorkspaceSource::Root { path } => configured_roots
                .get(&(provider_id.clone(), path.clone()))
                .map(|configured| (workspace.display_name.clone(), configured.color.clone()))
                .unwrap_or_else(|| (workspace.display_name.clone(), None)),
        };
        push_workspace(
            entries,
            seen,
            ConfiguredWorkspace {
                path: workspace.path,
                provider: provider.clone(),
                display_name: Some(display_name),
                color,
            },
        );
    }
}

fn push_missing_configured_workspaces(
    entries: &mut Vec<WorkspaceEntry>,
    seen: &mut HashSet<String>,
    provider_id: &str,
    configured_workspaces: &HashMap<(String, String), ConfiguredWorkspace>,
    returned_workspace_sources: &HashSet<(String, String)>,
) {
    for ((configured_provider_id, configured_path), workspace) in configured_workspaces {
        if configured_provider_id != provider_id {
            continue;
        }
        if returned_workspace_sources
            .contains(&(configured_provider_id.clone(), configured_path.clone()))
        {
            continue;
        }
        log::debug!(
            "workspace bulk metadata missing explicit workspace provider={} path={}",
            configured_provider_id,
            configured_path
        );
        push_workspace(entries, seen, normalize_workspace(workspace.clone()));
    }
}

fn list_provider_workspaces(
    provider: &WorkspaceProvider,
    request: ProviderWorkspaceListRequest,
) -> Result<Vec<ProviderWorkspaceEntry>, String> {
    log::debug!(
        "workspace bulk metadata request provider={} workspaces={} roots={}",
        provider.id(),
        request.workspace_paths.len(),
        request.root_paths.len()
    );
    match provider {
        WorkspaceProvider::Local => LocalProvider::new().list_workspaces(request),
        WorkspaceProvider::Ssh { host } => {
            SshProvider::new(SshProviderConfig::new(host.clone())).list_workspaces(request)
        }
    }
}

fn normalize_workspace(mut workspace: ConfiguredWorkspace) -> ConfiguredWorkspace {
    if workspace.provider.is_local()
        && let Some(path) = crate::config::expand_config_path_for_ui(&workspace.path)
    {
        workspace.path = path
            .canonicalize()
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
    }
    workspace
}

pub(crate) fn workspace_from_selection_id(id: &str) -> ConfiguredWorkspace {
    let (provider, path) = id.split_once('|').unwrap_or(("local", id));
    ConfiguredWorkspace {
        path: path.to_string(),
        provider: WorkspaceProvider::parse(Some(provider)),
        display_name: None,
        color: None,
    }
}

fn push_workspace(
    entries: &mut Vec<WorkspaceEntry>,
    seen: &mut HashSet<String>,
    mut workspace: ConfiguredWorkspace,
) {
    let key = workspace.selection_id();
    if !seen.insert(key) {
        return;
    }
    if workspace.color.is_none() {
        workspace.color =
            crate::config::workspace_color_for(&workspace.provider_id(), &workspace.path);
    }
    entries.push(WorkspaceEntry {
        label: workspace.label(),
        workspace,
    });
}
