use super::repo_cache::{
    RepoIconKind, cache_background_repo_icon_kind, cache_repo_icon_kind, cached_repo_icon_kind,
    kind_from_metadata,
};
use crate::git::GitRepoHandle;
use crate::git::RepositorySnapshot;
use crate::system::ProviderKind;
use crate::system::capabilities::github::GitHubAccess;
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
}

struct WorkspaceMetadataResult {
    item_id: String,
    kind: RepoIconKind,
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
        .and_then(crate::github::parse_github_url)
        .map(|slug| format_github_workspace_label(&slug, system_hostname(system)))
        .or_else(|| {
            snapshot
                .remote_url
                .as_deref()
                .and_then(crate::gitlab::parse_gitlab_url)
                .map(|slug| format_gitlab_workspace_label(&slug, system_hostname(system)))
        })
        .or_else(|| {
            snapshot
                .remote_url
                .as_deref()
                .and_then(crate::bitbucket::parse_bitbucket_url)
                .map(|slug| format_bitbucket_workspace_label(&slug, system_hostname(system)))
        })
        .unwrap_or_else(|| snapshot.name.clone())
}

pub(super) fn refresh_repo_icon_kind(
    workspace_key: String,
    item_id: Option<String>,
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
        let label = entry.label;
        let cached_kind = workspace_key
            .as_deref()
            .and_then(cached_repo_icon_kind)
            .filter(|kind| *kind != RepoIconKind::Unknown);
        let icon_name = cached_kind.map(|kind| kind.icon_name().to_string());

        if let Some(workspace_key) = workspace_key {
            metadata_requests.push(WorkspaceMetadataRequest {
                item_id: id.clone(),
                workspace_key,
                workspace: entry.workspace.clone(),
                label: label.clone(),
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
            icon_name,
        });
    }

    RepositoryPickerItems {
        items,
        metadata_requests,
    }
}

fn format_remote_workspace_label(repo_slug: &str, host: Option<&str>) -> String {
    host.map(display_hostname)
        .filter(|host| !host.is_empty())
        .map(|host| format!("{repo_slug}@{host}"))
        .unwrap_or_else(|| repo_slug.to_string())
}

fn format_github_workspace_label(repo_slug: &str, host: Option<&str>) -> String {
    format_remote_workspace_label(repo_slug, host)
}

fn format_gitlab_workspace_label(repo_slug: &str, host: Option<&str>) -> String {
    format_remote_workspace_label(repo_slug, host)
}

fn format_bitbucket_workspace_label(repo_slug: &str, host: Option<&str>) -> String {
    format_remote_workspace_label(repo_slug, host)
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

fn resolve_repo_kind_in_background<F: Fn() + 'static>(
    workspace_key: String,
    item_id: Option<String>,
    git_handle: Arc<GitRepoHandle>,
    github_access: Option<Arc<dyn GitHubAccess>>,
    repository_picker: picker::Picker,
    update_button: bool,
    on_done: F,
) {
    let (sender, receiver) = std::sync::mpsc::channel();

    git_handle.repo_metadata(
        github_access,
        Box::new(move |result| {
            let kind = match result {
                Ok(metadata) => kind_from_metadata(metadata),
                Err(err) => {
                    log::warn!("repo metadata refresh failed workspace={workspace_key}: {err}");
                    RepoIconKind::Unknown
                }
            };
            cache_repo_icon_kind(workspace_key, kind);
            let _ = sender.send(kind);
        }),
    );

    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(kind) => {
                if let Some(item_id) = item_id.as_deref() {
                    repository_picker.update_item_icon(item_id, kind.icon_name());
                }
                if update_button {
                    repository_picker.set_button_icon(kind.icon_name());
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
            let kind = resolve_workspace_metadata(&request);
            let kind = cache_background_repo_icon_kind(request.workspace_key.clone(), kind);
            if sender
                .send(WorkspaceMetadataResult {
                    item_id: request.item_id,
                    kind,
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
                repository_picker.update_item_icon(&result.item_id, result.kind.icon_name());
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

fn resolve_workspace_metadata(request: &WorkspaceMetadataRequest) -> RepoIconKind {
    match &request.workspace.provider {
        crate::config::WorkspaceProvider::Local => {
            let Some(path) = crate::config::expand_config_path_for_ui(&request.workspace.path)
            else {
                log::debug!(
                    "workspace picker metadata skipped label={} reason=invalid-local-path",
                    request.label
                );
                return RepoIconKind::Folder;
            };
            let provider = LocalProvider::new();
            let workspace = LocalProvider::workspace_for_path(&path);
            resolve_workspace_metadata_with_provider(&provider, &workspace)
        }
        crate::config::WorkspaceProvider::Ssh { host } => {
            if !request.workspace.path.starts_with('/') {
                log::debug!(
                    "workspace picker metadata skipped label={} provider=ssh:{} reason=relative-path",
                    request.label,
                    host
                );
                return RepoIconKind::Folder;
            }
            let provider = SshProvider::new(SshProviderConfig::new(host.clone()));
            let system = provider.system_ref();
            let workspace = WorkspaceRef::new(
                WorkspaceId::for_target(&system.id, &request.workspace.path),
                WorkspacePath::from_absolute(request.workspace.path.clone()),
                request.label.clone(),
            );
            resolve_workspace_metadata_with_provider(&provider, &workspace)
        }
    }
}

fn resolve_workspace_metadata_with_provider(
    provider: &dyn SystemProvider,
    workspace: &WorkspaceRef,
) -> RepoIconKind {
    let Some(files) = provider.files(workspace) else {
        log::debug!(
            "workspace picker metadata skipped workspace={} reason=no-files",
            workspace.display_name
        );
        return RepoIconKind::Unknown;
    };
    let Some(shell) = provider.shell(workspace) else {
        log::debug!(
            "workspace picker metadata skipped workspace={} reason=no-shell",
            workspace.display_name
        );
        return RepoIconKind::Unknown;
    };

    let account =
        crate::workspace_config::git_config_from_file_access(files.as_ref()).github_auth_account;
    let mut git_handle = GitRepoHandle::new(workspace.clone(), shell.clone(), files);
    if let Some(hook) = crate::github::git_auth_hook(shell, workspace.root.clone(), account) {
        git_handle = git_handle.with_hook(hook);
    }

    let (sender, receiver) = mpsc::channel();
    git_handle.repo_metadata(
        provider.github(workspace),
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    match receiver.recv() {
        Ok(Ok(metadata)) => kind_from_metadata(metadata),
        Ok(Err(err)) => {
            log::warn!(
                "workspace picker metadata failed workspace={}: {err}",
                workspace.display_name
            );
            RepoIconKind::Unknown
        }
        Err(_) => {
            log::warn!(
                "workspace picker metadata result channel closed workspace={}",
                workspace.display_name
            );
            RepoIconKind::Unknown
        }
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
