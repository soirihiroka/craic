use super::repo_cache::{
    RepoIconKind, RepoProperties, cache_resolved_repo_properties, cached_repo_icon_kind,
    cached_repo_properties, kind_from_metadata,
};
use crate::git::GitRepoHandle;
use crate::git::RepositorySnapshot;
use crate::github::GitHubAccess;
use crate::system::ProviderKind;
use crate::system::path::{
    SystemId, SystemRef, WorkspaceId, WorkspacePath, WorkspaceRef, pathbuf_to_target_absolute,
};
use crate::system::provider::SystemProvider;
use crate::system::providers::local::LocalProvider;
use crate::system::providers::ssh::{SshProvider, SshProviderConfig};
use crate::ui::picker;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::Duration;

pub(super) fn load_repos_async(
    repository_picker: picker::Picker,
    repo_loading: Rc<Cell<bool>>,
    repo_metadata_loading: Rc<Cell<bool>>,
) {
    if repo_loading.get() {
        return;
    }

    repo_loading.set(true);
    repository_picker.set_loading(true);

    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let result = repository_picker_items();
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(result) => {
                repository_picker.set_items(result.items);
                repository_picker.set_loading(false);
                repo_loading.set(false);
                refresh_workspace_metadata_in_background(
                    result.metadata_requests,
                    repository_picker.clone(),
                    repo_metadata_loading.clone(),
                );

                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                repository_picker.set_loading(false);
                repo_loading.set(false);
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

struct RepositoryPickerItems {
    items: Vec<picker::PickerItem>,
    metadata_requests: Vec<WorkspaceMetadataRequest>,
}

#[derive(Clone)]
struct WorkspaceMetadataRequest {
    item_id: String,
    workspace_key: String,
    workspace: crate::config::ConfiguredWorkspace,
    label: String,
    preserve_on_missing_remote_label: bool,
}

struct WorkspaceMetadataResult {
    item_id: String,
    properties: RepoProperties,
}

pub(super) fn current_repo_icon_kind(
    workspace_key: &str,
    _repository_picker: &picker::Picker,
) -> Option<RepoIconKind> {
    cached_repo_icon_kind(workspace_key)
}

pub(super) fn repository_button_label(snapshot: &RepositorySnapshot, system: &SystemRef) -> String {
    snapshot
        .remote_url
        .as_deref()
        .and_then(|url| remote_workspace_label(url, system_hostname(system)))
        .unwrap_or_else(|| snapshot.name.clone())
}

pub(super) fn refresh_repo_icon_kind(
    workspace_key: String,
    item_id: Option<String>,
    workspace_host: Option<String>,
    repository_picker: &picker::Picker,
    repo_icon_loading: Rc<Cell<bool>>,
    git_handle: Arc<GitRepoHandle>,
    github_access: Option<Arc<dyn GitHubAccess>>,
) {
    if repo_icon_loading.get() {
        return;
    }

    repo_icon_loading.set(true);
    repository_picker.set_button_spinner();
    resolve_repo_kind_in_background(
        workspace_key,
        item_id,
        workspace_host,
        git_handle,
        github_access,
        repository_picker.clone(),
        true,
        {
            let repo_icon_loading = repo_icon_loading.clone();
            move || repo_icon_loading.set(false)
        },
    );
}

fn repository_picker_items() -> RepositoryPickerItems {
    let entries = crate::workspace::discover_configured_workspaces();
    log::debug!("workspace picker resolving items count={}", entries.len());
    let mut items = Vec::new();
    let mut metadata_requests = Vec::new();

    for entry in entries {
        let id = entry.selection_id();
        let workspace_key = workspace_cache_key(&entry);
        let fallback_label = entry.label;
        let cached_properties = workspace_key.as_deref().and_then(cached_repo_properties);
        let label = cached_properties
            .as_ref()
            .and_then(|properties| properties.remote_label.clone())
            .unwrap_or_else(|| fallback_label.clone());
        let cached_kind = cached_properties.as_ref().map(|properties| properties.kind);
        let icon_name = cached_kind.map(|kind| kind.icon_name().to_string());
        let needs_remote_label_fill = cached_properties.as_ref().is_some_and(|properties| {
            properties.remote_label.is_none() && properties.kind.is_remote_metadata()
        });

        if let Some(workspace_key) = workspace_key
            && (cached_properties.is_none() || needs_remote_label_fill)
        {
            metadata_requests.push(WorkspaceMetadataRequest {
                item_id: id.clone(),
                workspace_key,
                workspace: entry.workspace.clone(),
                label: fallback_label.clone(),
                preserve_on_missing_remote_label: needs_remote_label_fill,
            });
        }

        log::debug!(
            "workspace picker item resolved id={} label={} icon={}",
            id,
            label,
            icon_name.as_deref().unwrap_or("loading")
        );
        items.push(picker::PickerItem {
            id,
            label,
            fallback_label,
            icon_name,
            color: entry
                .workspace
                .color
                .as_ref()
                .map(|color| color.background.clone()),
        });
    }

    RepositoryPickerItems {
        items,
        metadata_requests,
    }
}

fn format_remote_workspace_label(repo_slug: &str, host: Option<&str>) -> String {
    host.map(display_hostname)
        .filter(|host| !host.is_empty() && !is_local_hostname(host))
        .map(|host| format!("{repo_slug}@{host}"))
        .unwrap_or_else(|| repo_slug.to_string())
}

fn remote_workspace_label(remote_url: &str, host: Option<&str>) -> Option<String> {
    remote_slug(remote_url).map(|slug| format_remote_workspace_label(&slug, host))
}

fn remote_slug(remote_url: &str) -> Option<String> {
    crate::github::parse_github_url(remote_url)
        .or_else(|| crate::gitlab::parse_gitlab_url(remote_url))
        .or_else(|| crate::bitbucket::parse_bitbucket_url(remote_url))
        .or_else(|| generic_remote_slug(remote_url))
}

fn generic_remote_slug(remote_url: &str) -> Option<String> {
    let remote_url = remote_url.trim();
    if remote_url.is_empty() {
        return None;
    }

    let path = if let Some((_, tail)) = remote_url.split_once("://") {
        tail.split_once('/').map(|(_, path)| path)?
    } else if let Some((_, path)) = remote_url.split_once(':') {
        path
    } else {
        remote_url
    };
    let path = path
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path);
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }

    Some(format!(
        "{}/{}",
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    ))
}

fn system_hostname(system: &SystemRef) -> Option<&str> {
    (system.provider_kind == ProviderKind::Ssh)
        .then(|| system.host.as_ref().map(|host| host.label()))
        .flatten()
}

fn display_hostname(host: &str) -> &str {
    let host = host.trim();
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    host.trim_matches('/')
}

fn is_local_hostname(host: &str) -> bool {
    let normalized = host
        .trim()
        .trim_matches('/')
        .trim_matches(|ch| ch == '[' || ch == ']')
        .to_ascii_lowercase();
    if matches!(normalized.as_str(), "localhost" | "127.0.0.1" | "::1") {
        return true;
    }

    let without_port = normalized
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(normalized.as_str());
    matches!(without_port, "localhost" | "127.0.0.1")
}

fn resolve_repo_kind_in_background<F: Fn() + 'static>(
    workspace_key: String,
    item_id: Option<String>,
    workspace_host: Option<String>,
    git_handle: Arc<GitRepoHandle>,
    github_access: Option<Arc<dyn GitHubAccess>>,
    repository_picker: picker::Picker,
    update_button: bool,
    on_done: F,
) {
    let (sender, receiver) = std::sync::mpsc::channel();

    git_handle.workspace_metadata(
        github_access,
        Box::new(move |result| {
            let properties = match result {
                Ok(metadata) => {
                    let properties =
                        repo_properties_from_metadata(metadata, workspace_host.as_deref());
                    cache_resolved_repo_properties(workspace_key.clone(), properties)
                }
                Err(err) => {
                    log::warn!("repo metadata refresh failed workspace={workspace_key}: {err}");
                    cached_repo_properties(&workspace_key).unwrap_or(RepoProperties {
                        kind: RepoIconKind::Unknown,
                        remote_label: None,
                    })
                }
            };
            let _ = sender.send(properties);
        }),
    );

    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(properties) => {
                if let Some(item_id) = item_id.as_deref() {
                    repository_picker.update_item_metadata(
                        item_id,
                        properties.remote_label.as_deref(),
                        properties.kind.icon_name(),
                    );
                }
                if update_button {
                    repository_picker.set_button_icon(properties.kind.icon_name());
                }
                on_done();
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                on_done();
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn refresh_workspace_metadata_in_background(
    requests: Vec<WorkspaceMetadataRequest>,
    repository_picker: picker::Picker,
    repo_metadata_loading: Rc<Cell<bool>>,
) {
    if requests.is_empty() || repo_metadata_loading.get() {
        return;
    }

    let request_count = requests.len();
    repo_metadata_loading.set(true);
    log::debug!("workspace picker metadata refresh queued count={request_count}");

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for request in requests {
            let preserve_on_missing_remote_label = request.preserve_on_missing_remote_label;
            let properties = resolve_workspace_metadata(&request);
            let properties =
                if preserve_on_missing_remote_label && properties.remote_label.is_none() {
                    cached_repo_properties(&request.workspace_key).unwrap_or(properties)
                } else {
                    cache_resolved_repo_properties(request.workspace_key.clone(), properties)
                };
            if sender
                .send(WorkspaceMetadataResult {
                    item_id: request.item_id,
                    properties,
                })
                .is_err()
            {
                break;
            }
        }
    });

    let mut remaining = request_count;
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(result) => {
                repository_picker.update_item_metadata(
                    &result.item_id,
                    result.properties.remote_label.as_deref(),
                    result.properties.kind.icon_name(),
                );
                remaining = remaining.saturating_sub(1);
                if remaining == 0 {
                    repo_metadata_loading.set(false);
                    log::debug!("workspace picker metadata refresh complete");
                    gtk::glib::ControlFlow::Break
                } else {
                    gtk::glib::ControlFlow::Continue
                }
            }
            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                repo_metadata_loading.set(false);
                log::warn!("workspace picker metadata refresh stopped before completion");
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

fn resolve_workspace_metadata(request: &WorkspaceMetadataRequest) -> RepoProperties {
    match &request.workspace.provider {
        crate::config::WorkspaceProvider::Local => {
            let Some(path) = crate::config::expand_config_path_for_ui(&request.workspace.path)
            else {
                log::debug!(
                    "workspace picker metadata skipped label={} reason=invalid-local-path",
                    request.label
                );
                return RepoProperties {
                    kind: RepoIconKind::Folder,
                    remote_label: None,
                };
            };
            let provider = LocalProvider::new();
            let workspace = LocalProvider::workspace_for_path(&path);
            resolve_workspace_metadata_with_provider(&provider, &workspace, None)
        }
        crate::config::WorkspaceProvider::Ssh { host } => {
            if !request.workspace.path.starts_with('/') {
                log::debug!(
                    "workspace picker metadata skipped label={} provider=ssh:{} reason=relative-path",
                    request.label,
                    host
                );
                return RepoProperties {
                    kind: RepoIconKind::Folder,
                    remote_label: None,
                };
            }
            let provider = SshProvider::new(SshProviderConfig::new(host.clone()));
            let system = provider.system_ref();
            let workspace = WorkspaceRef::new(
                WorkspaceId::for_target(&system.id, &request.workspace.path),
                WorkspacePath::from_absolute(request.workspace.path.clone()),
                request.label.clone(),
            );
            resolve_workspace_metadata_with_provider(&provider, &workspace, Some(host))
        }
    }
}

fn resolve_workspace_metadata_with_provider(
    provider: &dyn SystemProvider,
    workspace: &WorkspaceRef,
    workspace_host: Option<&str>,
) -> RepoProperties {
    let Some(files) = provider.files(workspace) else {
        log::debug!(
            "workspace picker metadata skipped workspace={} reason=no-files",
            workspace.display_name
        );
        return RepoProperties {
            kind: RepoIconKind::Unknown,
            remote_label: None,
        };
    };
    let Some(shell) = provider.shell(workspace) else {
        log::debug!(
            "workspace picker metadata skipped workspace={} reason=no-shell",
            workspace.display_name
        );
        return RepoProperties {
            kind: RepoIconKind::Unknown,
            remote_label: None,
        };
    };

    let account =
        crate::workspace_config::git_config_from_file_access(files.as_ref()).github_auth_account;
    let mut git_handle = GitRepoHandle::new(workspace.clone(), shell.clone(), files);
    if let Some(hook) = crate::github::git_auth_hook(shell, workspace.root.clone(), account) {
        git_handle = git_handle.with_hook(hook);
    }

    let (sender, receiver) = mpsc::channel();
    git_handle.workspace_metadata(
        craic_vcs::github_access_for_provider(provider, workspace),
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    match receiver.recv() {
        Ok(Ok(metadata)) => repo_properties_from_metadata(metadata, workspace_host),
        Ok(Err(err)) => {
            log::warn!(
                "workspace picker metadata failed workspace={}: {err}",
                workspace.display_name
            );
            RepoProperties {
                kind: RepoIconKind::Unknown,
                remote_label: None,
            }
        }
        Err(_) => {
            log::warn!(
                "workspace picker metadata result channel closed workspace={}",
                workspace.display_name
            );
            RepoProperties {
                kind: RepoIconKind::Unknown,
                remote_label: None,
            }
        }
    }
}

fn repo_properties_from_metadata(
    metadata: crate::git::WorkspaceRepositoryMetadata,
    workspace_host: Option<&str>,
) -> RepoProperties {
    RepoProperties {
        kind: kind_from_metadata(metadata.kind),
        remote_label: metadata
            .remote_url
            .as_deref()
            .and_then(|remote_url| remote_workspace_label(remote_url, workspace_host)),
    }
}

fn workspace_cache_key(entry: &crate::workspace::WorkspaceEntry) -> Option<String> {
    match &entry.workspace.provider {
        crate::config::WorkspaceProvider::Local => {
            let path = entry.local_path()?;
            Some(
                WorkspaceId::for_target(&SystemId::new("local"), &pathbuf_to_target_absolute(path))
                    .to_string(),
            )
        }
        crate::config::WorkspaceProvider::Ssh { host } => {
            entry.workspace.path.starts_with('/').then(|| {
                WorkspaceId::for_target(
                    &SystemId::new(format!("ssh:{host}")),
                    &entry.workspace.path,
                )
                .to_string()
            })
        }
    }
}
