use super::repo_cache::{
    RepoIconKind, cache_repo_icon_kind, cached_repo_icon_kind, kind_from_metadata,
};
use crate::git::RepositorySnapshot;
use crate::system::ProviderKind;
use crate::system::capabilities::git::GitAccess;
use crate::system::capabilities::github::GitHubAccess;
use crate::system::path::{SystemId, SystemRef, WorkspaceId, pathbuf_to_target_absolute};
use crate::system::provider::{ProviderWorkspaceGitStatus, ProviderWorkspaceRemote};
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
                preload_workspace_repo_metadata(result.metadata, repository_picker.clone());

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
    metadata: Vec<WorkspaceRepoMetadataRequest>,
}

#[derive(Clone, Debug)]
struct WorkspaceRepoMetadataRequest {
    item_id: String,
    workspace_key: String,
    workspace_root: String,
    remote: ProviderWorkspaceRemote,
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
    git_access: Arc<dyn GitAccess>,
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
        git_access,
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
    let mut metadata = Vec::new();

    for entry in entries {
        let id = entry.selection_id();
        collect_workspace_repo_metadata_request(&entry, &id, &mut metadata);
        let (label, kind) = workspace_picker_label_kind(&entry);

        log::debug!(
            "workspace picker item resolved id={} label={} icon={}",
            id,
            label,
            kind.icon_name()
        );
        items.push(picker::PickerItem::new(id, label, kind.icon_name()));
    }

    RepositoryPickerItems { items, metadata }
}

fn collect_workspace_repo_metadata_request(
    entry: &crate::workspace::WorkspaceEntry,
    item_id: &str,
    metadata: &mut Vec<WorkspaceRepoMetadataRequest>,
) {
    let Some(workspace_key) = workspace_cache_key(entry) else {
        return;
    };
    if cached_repo_icon_kind(&workspace_key).is_some() {
        return;
    }
    match entry.git.as_ref() {
        Some(ProviderWorkspaceGitStatus::NotRepo) => {
            cache_repo_icon_kind(workspace_key, RepoIconKind::Folder);
        }
        Some(ProviderWorkspaceGitStatus::Repo {
            remote: Some(remote),
        }) if remote.slug.is_some() => {
            metadata.push(WorkspaceRepoMetadataRequest {
                item_id: item_id.to_string(),
                workspace_key,
                workspace_root: entry.workspace.path.clone(),
                remote: remote.clone(),
            });
        }
        _ => {}
    }
}

fn preload_workspace_repo_metadata(
    metadata: Vec<WorkspaceRepoMetadataRequest>,
    repository_picker: picker::Picker,
) {
    if metadata.is_empty() {
        return;
    }
    log::debug!(
        "workspace repo metadata preload queued count={}",
        metadata.len()
    );
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut updates = Vec::new();
        for request in metadata {
            if cached_repo_icon_kind(&request.workspace_key).is_some() {
                continue;
            }
            let kind = fetch_workspace_repo_icon_kind(&request).unwrap_or(RepoIconKind::Git);
            cache_repo_icon_kind(request.workspace_key.clone(), kind);
            updates.push((request.item_id, kind));
        }
        let _ = sender.send(updates);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        match receiver.try_recv() {
            Ok(updates) => {
                for (item_id, kind) in updates {
                    repository_picker.update_item_icon(&item_id, kind.icon_name());
                }
                gtk::glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
        }
    });
}

fn fetch_workspace_repo_icon_kind(request: &WorkspaceRepoMetadataRequest) -> Option<RepoIconKind> {
    let slug = request.remote.slug.as_deref()?;
    let remote_name = request.remote.name.as_deref();
    let remote_url = request.remote.url.as_str();

    if crate::github::parse_github_url(remote_url).is_some() {
        return crate::github::repo_metadata_for_workspace(
            &request.workspace_key,
            &request.workspace_root,
            slug,
            remote_name,
            Some(remote_url),
            || crate::github::fetch_repo_metadata(slug),
        )
        .map(github_icon_kind)
        .map_err(|err| {
            log::warn!(
                "workspace github repo metadata preload failed repo={} err={}",
                slug,
                err
            );
            err
        })
        .ok();
    }

    if crate::gitlab::parse_gitlab_url(remote_url).is_some() {
        return crate::gitlab::repo_metadata_for_workspace(
            &request.workspace_key,
            &request.workspace_root,
            slug,
            remote_name,
            Some(remote_url),
            || crate::gitlab::fetch_repo_metadata(remote_url),
        )
        .map(gitlab_icon_kind)
        .map_err(|err| {
            log::warn!(
                "workspace gitlab repo metadata preload failed repo={} err={}",
                slug,
                err
            );
            err
        })
        .ok();
    }

    if crate::bitbucket::parse_bitbucket_url(remote_url).is_some() {
        return crate::bitbucket::repo_metadata_for_workspace(
            &request.workspace_key,
            &request.workspace_root,
            slug,
            remote_name,
            Some(remote_url),
            || crate::bitbucket::fetch_repo_metadata(remote_url),
        )
        .map(bitbucket_icon_kind)
        .map_err(|err| {
            log::warn!(
                "workspace bitbucket repo metadata preload failed repo={} err={}",
                slug,
                err
            );
            err
        })
        .ok();
    }

    None
}

fn github_icon_kind(metadata: crate::github::GitHubRepoMetadata) -> RepoIconKind {
    match metadata {
        crate::github::GitHubRepoMetadata::Fork => RepoIconKind::Fork,
        crate::github::GitHubRepoMetadata::Private => RepoIconKind::Private,
        crate::github::GitHubRepoMetadata::Public => RepoIconKind::Public,
    }
}

fn gitlab_icon_kind(metadata: crate::gitlab::GitLabRepoMetadata) -> RepoIconKind {
    match metadata {
        crate::gitlab::GitLabRepoMetadata::Fork => RepoIconKind::Fork,
        crate::gitlab::GitLabRepoMetadata::Private => RepoIconKind::Private,
        crate::gitlab::GitLabRepoMetadata::Public => RepoIconKind::Public,
    }
}

fn bitbucket_icon_kind(metadata: crate::bitbucket::BitbucketRepoMetadata) -> RepoIconKind {
    match metadata {
        crate::bitbucket::BitbucketRepoMetadata::Fork => RepoIconKind::Fork,
        crate::bitbucket::BitbucketRepoMetadata::Private => RepoIconKind::Private,
        crate::bitbucket::BitbucketRepoMetadata::Public => RepoIconKind::Public,
    }
}

fn workspace_picker_label_kind(entry: &crate::workspace::WorkspaceEntry) -> (String, RepoIconKind) {
    if let Some(git) = entry.git.as_ref() {
        return bulk_workspace_picker_label_kind(entry, git);
    }

    let Some(path) = entry.local_path() else {
        return (entry.label.clone(), RepoIconKind::Folder);
    };

    if let Some(slug) = crate::git::github_slug_for_path(&path) {
        let label = format_remote_workspace_label(&slug, None);
        let kind = workspace_cache_key(entry)
            .and_then(|workspace_key| cached_repo_icon_kind(&workspace_key))
            .filter(|kind| *kind != RepoIconKind::Folder)
            .unwrap_or(RepoIconKind::Git);
        return (label, kind);
    }

    if crate::git::root_for_path(&path).is_some() {
        (entry.label.clone(), RepoIconKind::Git)
    } else {
        (entry.label.clone(), RepoIconKind::Folder)
    }
}

fn bulk_workspace_picker_label_kind(
    entry: &crate::workspace::WorkspaceEntry,
    git: &ProviderWorkspaceGitStatus,
) -> (String, RepoIconKind) {
    match git {
        ProviderWorkspaceGitStatus::NotRepo => (entry.label.clone(), RepoIconKind::Folder),
        ProviderWorkspaceGitStatus::Repo { remote } => {
            let label = remote
                .as_ref()
                .and_then(|remote| remote.slug.as_deref())
                .map(|slug| format_remote_workspace_label(slug, entry_hostname(entry)))
                .unwrap_or_else(|| entry.label.clone());
            let kind = workspace_cache_key(entry)
                .and_then(|workspace_key| cached_repo_icon_kind(&workspace_key))
                .filter(|kind| *kind != RepoIconKind::Folder)
                .unwrap_or(RepoIconKind::Git);
            (label, kind)
        }
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

fn entry_hostname(entry: &crate::workspace::WorkspaceEntry) -> Option<&str> {
    match &entry.workspace.provider {
        crate::config::WorkspaceProvider::Local => None,
        crate::config::WorkspaceProvider::Ssh { host } => Some(host),
    }
}

fn display_hostname(host: &str) -> &str {
    let host = host.trim();
    let host = host.rsplit_once('@').map(|(_, host)| host).unwrap_or(host);
    host.trim_matches('/')
}

fn resolve_repo_kind_in_background<F: Fn() + 'static>(
    workspace_key: String,
    item_id: Option<String>,
    git_access: Arc<dyn GitAccess>,
    github_access: Option<Arc<dyn GitHubAccess>>,
    repository_picker: picker::Picker,
    update_button: bool,
    on_done: F,
) {
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let kind = kind_from_metadata(git_access.repo_metadata(github_access.as_deref()));
        cache_repo_icon_kind(workspace_key, kind);
        let _ = sender.send(kind);
    });

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
