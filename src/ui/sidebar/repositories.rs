use super::repo_cache::{
    RepoIconKind, cache_repo_icon_kind, cached_repo_icon_kind, kind_from_metadata,
};
use crate::git::GitRepoHandle;
use crate::git::RepositorySnapshot;
use crate::system::ProviderKind;
use crate::system::capabilities::github::GitHubAccess;
use crate::system::path::{SystemId, SystemRef, WorkspaceId, pathbuf_to_target_absolute};
use crate::ui::picker;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub(super) fn load_repos_async(
    repository_picker: picker::Picker,
    repo_loading: Rc<Cell<bool>>,
    _repo_icon_loading: Rc<Cell<bool>>,
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

    for entry in entries {
        let id = entry.selection_id();
        let (label, kind) = workspace_picker_label_kind(&entry);

        log::debug!(
            "workspace picker item resolved id={} label={} icon={}",
            id,
            label,
            kind.icon_name()
        );
        items.push(picker::PickerItem::new(id, label, kind.icon_name()));
    }

    RepositoryPickerItems { items }
}

fn workspace_picker_label_kind(entry: &crate::workspace::WorkspaceEntry) -> (String, RepoIconKind) {
    let kind = workspace_cache_key(entry)
        .and_then(|workspace_key| cached_repo_icon_kind(&workspace_key))
        .unwrap_or(RepoIconKind::Folder);
    (entry.label.clone(), kind)
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
            let kind = kind_from_metadata(result.unwrap_or(crate::git::RepoMetadata::Private));
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
