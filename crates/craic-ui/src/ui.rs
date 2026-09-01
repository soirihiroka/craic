mod app_menu;
mod branch_actions;
pub(crate) use craic_ui_core::ui::components;
mod content;
mod dialogs;
pub(crate) use craic_ui_core::ui::file_type;
mod git_actions;
mod pages;
pub(crate) use craic_ui_core::ui::picker;
mod preferences;
mod shell_actions;
mod shortcuts;
mod sidebar;
pub(crate) use craic_ui_core::ui::widgets;

use crate::git;
use crate::system::capabilities::{
    shell::ShellAccess, terminal_link::TerminalLinkAccess, url::UrlOpenAccess,
};
use crate::system::path::SystemId;
use crate::system::provider::SystemProvider;
use crate::system::providers::local::LocalProvider;
use crate::system::{SystemProviderRegistry, SystemRef, WorkspacePath, WorkspaceRef};
use adw::prelude::*;
use app_menu::{app_menu, install_actions, launch_workspace_location_in_new_instance};
use branch_actions::connect_branch_actions;
use craic_app_core::{
    ActionId, AppCommand, AppHandle, ApplicationRuntime, Badge, Generation,
    PageCommand as CorePageCommand, PageId as CorePageId, PageServiceRequest, PageViewState,
    RuntimeConfig, ServiceCompletion, UiEvent, WorkspaceId, WorkspaceRefreshCompletion,
    WorkspaceRefreshIdentity, WorkspaceRefreshOptions, WorkspaceRefreshRequest, WorkspaceSelection,
};
use craic_ui_core::ui::command_mailbox;
use dialogs::show_error_dialog;
use dialogs::show_startup_crash_dialog;
use git_actions::run_git_action;
use pages::{PageCommand, PageCommandResult, PageRefreshRequest};
use pango::prelude::FontMapExt;
use shell_actions::connect_shell_actions;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::Instant;

const JETBRAINS_MONO_DIR: &str = "JetBrainsMono";

#[derive(Clone, Debug)]
pub struct StartupOpenLocation {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

struct StartupTimer {
    start: Instant,
    previous: Instant,
}

impl StartupTimer {
    fn new(start: Instant) -> Self {
        Self {
            start,
            previous: start,
        }
    }

    fn mark(&mut self, step: &str) {
        let now = Instant::now();
        log::info!(
            "startup step={step} step_ms={} total_ms={}",
            now.duration_since(self.previous).as_millis(),
            now.duration_since(self.start).as_millis()
        );
        self.previous = now;
    }
}

include!("ui/build.rs");

enum WorkspaceModelCommand {
    ProviderSnapshot {
        workspace_key: String,
        git_handle: Arc<crate::git::GitRepoHandle>,
        respond_to:
            command_mailbox::UiCommandSender<(String, Result<git::RepositorySnapshot, String>)>,
    },
    ProviderWorkspaceSnapshot {
        workspace_key: String,
        git_handle: Arc<crate::git::GitRepoHandle>,
        respond_to:
            command_mailbox::UiCommandSender<(String, Result<git::WorkspaceSnapshot, String>)>,
    },
}

fn workspace_model_worker() -> &'static mpsc::Sender<WorkspaceModelCommand> {
    static WORKER: OnceLock<mpsc::Sender<WorkspaceModelCommand>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<WorkspaceModelCommand>();
        thread::spawn(move || {
            log::info!("workspace model worker started");

            for command in receiver {
                match command {
                    WorkspaceModelCommand::ProviderSnapshot {
                        workspace_key,
                        git_handle,
                        respond_to,
                    } => {
                        let step_start = Instant::now();
                        let result = git_handle.load_repository_snapshot();
                        log::info!(
                            "git snapshot worker finished workspace={} status={} elapsed_ms={}",
                            workspace_key,
                            if result.is_ok() { "ok" } else { "error" },
                            step_start.elapsed().as_millis()
                        );
                        respond_to.send((workspace_key, result));
                    }
                    WorkspaceModelCommand::ProviderWorkspaceSnapshot {
                        workspace_key,
                        git_handle,
                        respond_to,
                    } => {
                        let step_start = Instant::now();
                        let result = git_handle.load_workspace_snapshot();
                        log::info!(
                            "git workspace snapshot worker finished workspace={} status={} elapsed_ms={}",
                            workspace_key,
                            if result.is_ok() { "ok" } else { "error" },
                            step_start.elapsed().as_millis()
                        );
                        respond_to.send((workspace_key, result));
                    }
                }
            }

            log::warn!("workspace model worker stopped");
        });

        sender
    })
}

pub(crate) fn request_provider_git_snapshot<F>(
    workspace_key: String,
    git_handle: Arc<crate::git::GitRepoHandle>,
    on_result: F,
) where
    F: FnMut(String, Result<git::RepositorySnapshot, String>) + 'static,
{
    let mut on_result = on_result;
    let request_start = Instant::now();
    let response = command_mailbox::once(
        move |(key, result): (String, Result<git::RepositorySnapshot, String>)| {
            match &result {
                Ok(snapshot) => log::info!(
                    "provider git snapshot received workspace={} elapsed_ms={} branch={} changed_files={} branches={}",
                    key,
                    request_start.elapsed().as_millis(),
                    snapshot.branch,
                    snapshot.changed_files.len(),
                    snapshot.branches.len()
                ),
                Err(err) => log::warn!(
                    "provider git snapshot failed workspace={} elapsed_ms={}: {}",
                    key,
                    request_start.elapsed().as_millis(),
                    err
                ),
            }
            on_result(key, result);
        },
    );

    if let Err(err) = workspace_model_worker().send(WorkspaceModelCommand::ProviderSnapshot {
        workspace_key: workspace_key.clone(),
        git_handle,
        respond_to: response,
    }) {
        log::error!("failed to enqueue provider git snapshot request for {workspace_key}: {err}");
        let WorkspaceModelCommand::ProviderSnapshot { respond_to, .. } = err.0 else {
            unreachable!("snapshot send error returned a different command")
        };
        respond_to.send((
            workspace_key,
            Err("Git snapshot worker is unavailable.".to_string()),
        ));
        return;
    }

    log::info!("provider git snapshot queued workspace={workspace_key}");
}

pub(crate) fn request_provider_workspace_snapshot<F>(
    workspace_key: String,
    git_handle: Arc<crate::git::GitRepoHandle>,
    on_result: F,
) where
    F: FnMut(String, Result<git::WorkspaceSnapshot, String>) + 'static,
{
    let mut on_result = on_result;
    let request_start = Instant::now();
    let response = command_mailbox::once(
        move |(key, result): (String, Result<git::WorkspaceSnapshot, String>)| {
            match &result {
                Ok(snapshot) => log::info!(
                    "provider workspace snapshot received workspace={} elapsed_ms={} name={}",
                    key,
                    request_start.elapsed().as_millis(),
                    snapshot.name()
                ),
                Err(err) => log::warn!(
                    "provider workspace snapshot failed workspace={} elapsed_ms={}: {}",
                    key,
                    request_start.elapsed().as_millis(),
                    err
                ),
            }
            on_result(key, result);
        },
    );

    if let Err(err) =
        workspace_model_worker().send(WorkspaceModelCommand::ProviderWorkspaceSnapshot {
            workspace_key: workspace_key.clone(),
            git_handle,
            respond_to: response,
        })
    {
        log::error!(
            "failed to enqueue provider workspace snapshot request for {workspace_key}: {err}"
        );
        let WorkspaceModelCommand::ProviderWorkspaceSnapshot { respond_to, .. } = err.0 else {
            unreachable!("workspace snapshot send error returned a different command")
        };
        respond_to.send((
            workspace_key,
            Err("Git snapshot worker is unavailable.".to_string()),
        ));
        return;
    }

    log::info!("provider workspace snapshot queued workspace={workspace_key}");
}

fn git_handle_for_active_workspace(state: &Rc<AppState>) -> Option<Arc<crate::git::GitRepoHandle>> {
    let system_id = state.system_ref.borrow().id.clone();
    let workspace = state.workspace_ref.borrow().clone();
    git_handle_for_workspace(state, &system_id, &workspace)
}

fn git_handle_for_workspace(
    state: &Rc<AppState>,
    system_id: &SystemId,
    workspace: &crate::system::WorkspaceRef,
) -> Option<Arc<crate::git::GitRepoHandle>> {
    let files = state.providers.files(system_id, workspace)?;
    let shell = state.providers.shell(system_id, workspace)?;
    let mut handle =
        crate::git::GitRepoHandle::new(workspace.clone(), shell.clone(), files.clone());
    let account =
        crate::workspace_config::git_config_from_file_access(files.as_ref()).github_auth_account;
    if let Some(hook) = crate::github::git_auth_hook(shell, workspace.root.clone(), account) {
        handle = handle.with_hook(hook);
    }
    Some(Arc::new(handle))
}

#[derive(Clone)]
struct ActiveWorkspace {
    repo_path: PathBuf,
    system_ref: SystemRef,
    workspace_ref: WorkspaceRef,
}

fn initial_workspace(
    registry: &SystemProviderRegistry,
    startup_workspace: Option<&crate::config::ConfiguredWorkspace>,
) -> ActiveWorkspace {
    let step_start = Instant::now();
    if let Some(workspace) = startup_workspace {
        let active_workspace = active_workspace_from_config(registry, workspace);
        log::info!(
            "startup initial workspace source=cli provider={} path={} elapsed_ms={}",
            workspace.provider_id(),
            workspace.path,
            step_start.elapsed().as_millis()
        );
        return active_workspace;
    }

    if let Some(workspace) = crate::config::last_workspace() {
        let active_workspace = active_workspace_from_config(registry, &workspace);
        log::info!(
            "startup initial workspace source=last provider={} path={} elapsed_ms={}",
            workspace.provider_id(),
            workspace.path,
            step_start.elapsed().as_millis()
        );
        return active_workspace;
    }

    let discovery_start = Instant::now();
    let workspaces = crate::workspace::discover_configured_workspaces();
    log::info!(
        "startup configured workspace discovery count={} elapsed_ms={}",
        workspaces.len(),
        discovery_start.elapsed().as_millis()
    );
    if let Some(entry) = workspaces.into_iter().next() {
        let active_workspace = active_workspace_from_config(registry, &entry.workspace);
        log::info!(
            "startup initial workspace source=configured provider={} path={} elapsed_ms={}",
            entry.workspace.provider_id(),
            entry.workspace.path,
            step_start.elapsed().as_millis()
        );
        return active_workspace;
    }

    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let active_workspace = local_active_workspace(registry, current_dir.clone());
    log::info!(
        "startup initial workspace source=current-dir path={} elapsed_ms={}",
        current_dir.display(),
        step_start.elapsed().as_millis()
    );
    active_workspace
}

fn active_workspace_from_config(
    registry: &SystemProviderRegistry,
    workspace: &crate::config::ConfiguredWorkspace,
) -> ActiveWorkspace {
    let access = craic_system::workspace::configured_workspace_access(workspace);
    let system_ref = access.system;
    registry.register(access.provider);
    ActiveWorkspace {
        repo_path: access.display_path,
        system_ref,
        workspace_ref: access.workspace,
    }
}

fn local_active_workspace(registry: &SystemProviderRegistry, path: PathBuf) -> ActiveWorkspace {
    let provider = Arc::new(LocalProvider::new());
    registry.register(provider.clone() as Arc<dyn SystemProvider>);
    let workspace_ref = LocalProvider::workspace_for_path(&path);
    ActiveWorkspace {
        repo_path: path,
        system_ref: provider.system_ref(),
        workspace_ref,
    }
}

pub(crate) fn asset_search_paths() -> Vec<PathBuf> {
    craic_ui_core::ui::asset_search_paths()
}

fn register_bundled_fonts() {
    let step_start = Instant::now();
    let font_map = pangocairo::FontMap::default();
    let font_files = bundled_font_files();
    let font_count = font_files.len();
    let mut registered_count = 0;

    for path in font_files {
        match font_map.add_font_file(&path) {
            Ok(()) => registered_count += 1,
            Err(error) => log::warn!(
                "Failed to register bundled font {}: {error}",
                path.display()
            ),
        }
    }

    if registered_count > 0 {
        font_map.changed();
    }

    log::info!(
        "startup bundled fonts discovered={} registered={} elapsed_ms={}",
        font_count,
        registered_count,
        step_start.elapsed().as_millis()
    );
}

fn bundled_font_files() -> Vec<PathBuf> {
    let mut files = Vec::new();

    for dir in font_search_paths().into_iter().filter(|path| path.is_dir()) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if is_font_file(&path) && !files.contains(&path) {
                files.push(path);
            }
        }
    }

    files
}

fn font_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![craic_ui_core::ui::bundled_font_dir()];

    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        paths.push(
            PathBuf::from(data_home)
                .join("fonts")
                .join("craic")
                .join(JETBRAINS_MONO_DIR),
        );
    }

    if let Ok(home) = std::env::var("HOME") {
        paths.push(home_font_path(&home));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for data_dir in data_dirs.split(':').filter(|path| !path.is_empty()) {
        paths.push(
            PathBuf::from(data_dir)
                .join("fonts")
                .join("craic")
                .join(JETBRAINS_MONO_DIR),
        );
    }

    paths
}

fn home_font_path(home: &str) -> PathBuf {
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("fonts")
        .join("craic")
        .join(JETBRAINS_MONO_DIR)
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("ttf") || extension.eq_ignore_ascii_case("otf")
        })
}

struct AppState {
    repo_path: Rc<RefCell<PathBuf>>,
    system_ref: Rc<RefCell<crate::system::SystemRef>>,
    workspace_ref: Rc<RefCell<crate::system::WorkspaceRef>>,
    providers: crate::system::SystemProviderRegistry,
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    sidebar: sidebar::SidebarPane,
    content: content::ContentPane,
    pages: Vec<pages::PageRef>,
    page_host: pages::PageHost,
    active_page: RefCell<Option<pages::PageId>>,
    app_runtime: RefCell<Option<ApplicationRuntime>>,
    app_handle: AppHandle,
    page_state_revisions: RefCell<HashMap<CorePageId, u64>>,
    page_service_requests: RefCell<HashMap<CorePageId, PageServiceRequest>>,
    git_action_running: Rc<Cell<bool>>,
    last_error: RefCell<Option<String>>,
    last_snapshot: RefCell<Option<git::WorkspaceSnapshot>>,
    last_snapshot_repo: RefCell<Option<PathBuf>>,
    workspace_generation: Cell<Generation>,
    workspace_refresh_request: RefCell<Option<WorkspaceRefreshRequest>>,
    repository_monitor: RepositoryMonitor,
    workspace_color_provider: gtk::CssProvider,
}

impl AppState {
    fn show_toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(runtime) = self.app_runtime.get_mut().take() {
            runtime.shutdown(std::time::Duration::from_secs(2));
        }
    }
}

fn apply_workspace_color(state: &Rc<AppState>) {
    let (provider_id, workspace_root, workspace_name) = {
        let system = state.system_ref.borrow();
        let workspace = state.workspace_ref.borrow();
        (
            system.id.as_str().to_string(),
            workspace.root.absolute.clone(),
            workspace.display_name.clone(),
        )
    };

    match crate::config::workspace_color_for(&provider_id, &workspace_root) {
        Some(color) => {
            state
                .workspace_color_provider
                .load_from_data(&workspace_titlebar_css(&color));
            state.sidebar.set_workspace_color_active(true);
            state.content.set_workspace_color_active(true);
            log::info!(
                "workspace titlebar color applied workspace={} provider={} root={} color={}",
                workspace_name,
                provider_id,
                workspace_root,
                color.background
            );
        }
        None => {
            state.workspace_color_provider.load_from_data("");
            state.sidebar.set_workspace_color_active(false);
            state.content.set_workspace_color_active(false);
            log::debug!(
                "workspace titlebar color cleared workspace={} provider={} root={}",
                workspace_name,
                provider_id,
                workspace_root
            );
        }
    }
}

fn workspace_titlebar_css(color: &crate::config::WorkspaceColor) -> String {
    let foreground = workspace_titlebar_foreground(&color.background);
    format!(
        r#"
        @define-color craic_workspace_titlebar_bg {};
        @define-color craic_workspace_titlebar_fg {};

        .workspace-titlebar-color {{
            background-color: @craic_workspace_titlebar_bg;
            color: @craic_workspace_titlebar_fg;
        }}

        .workspace-titlebar-color:backdrop {{
            background-color: alpha(@craic_workspace_titlebar_bg, 0.86);
            color: alpha(@craic_workspace_titlebar_fg, 0.86);
        }}

        .workspace-titlebar-color button,
        .workspace-titlebar-color label,
        .workspace-titlebar-color image {{
            color: @craic_workspace_titlebar_fg;
        }}
        "#,
        color.background, foreground
    )
}

fn workspace_titlebar_foreground(background: &str) -> &'static str {
    let Some((red, green, blue)) = parse_hex_rgb(background) else {
        return "#ffffff";
    };
    let luminance = relative_luminance(red) * 0.2126
        + relative_luminance(green) * 0.7152
        + relative_luminance(blue) * 0.0722;
    if luminance > 0.42 {
        "#111111"
    } else {
        "#ffffff"
    }
}

fn parse_hex_rgb(color: &str) -> Option<(u8, u8, u8)> {
    let hex = color.strip_prefix('#')?;
    match hex.len() {
        3 | 4 => {
            let mut chars = hex.chars();
            let red = doubled_hex(chars.next()?)?;
            let green = doubled_hex(chars.next()?)?;
            let blue = doubled_hex(chars.next()?)?;
            Some((red, green, blue))
        }
        6 | 8 => Some((
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        )),
        _ => None,
    }
}

fn doubled_hex(value: char) -> Option<u8> {
    let mut hex = String::new();
    hex.push(value);
    hex.push(value);
    u8::from_str_radix(&hex, 16).ok()
}

fn relative_luminance(value: u8) -> f64 {
    let value = f64::from(value) / 255.0;
    if value <= 0.03928 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[derive(Default)]
struct RepositoryMonitor {
    workspace_key: RefCell<Option<String>>,
    subscription: RefCell<Option<crate::git::ChangeListenerSubscription>>,
    background_pull: RefCell<Option<crate::git::BackgroundPullSubscription>>,
    event_subscription: RefCell<Option<command_mailbox::UiCommandSubscription>>,
}

impl RepositoryMonitor {
    fn restart(&self, state: &Rc<AppState>) {
        self.stop();

        let workspace = state.workspace_ref.borrow().clone();
        let workspace_key = workspace.id.to_string();
        self.workspace_key.replace(Some(workspace_key.clone()));

        let Some(git_handle) = git_handle_for_active_workspace(state) else {
            log::debug!(
                "repository monitor unavailable workspace={} reason=no-git-handle",
                workspace.display_name
            );
            return;
        };

        let state_weak = Rc::downgrade(state);
        let source_workspace_key = workspace_key.clone();
        let (sender, event_subscription) = command_mailbox::latest(move |()| {
            let Some(state) = state_weak.upgrade() else {
                return;
            };
            if state.workspace_ref.borrow().id.to_string() != source_workspace_key {
                log::debug!(
                    "ignored repository event for inactive workspace {}",
                    source_workspace_key
                );
                return;
            }
            log::debug!(
                "refreshing repository workspace={} from provider git watcher",
                source_workspace_key
            );
            refresh_from_monitor(&state);
        });
        let listener: crate::git::ChangeListener = Arc::new(move || {
            sender.send(());
        });
        let subscription = git_handle.add_on_change_listener(listener.clone());
        let background_pull = git_handle.schedule_background_pull_loop(Some(listener));

        self.subscription.replace(Some(subscription));
        self.background_pull.replace(Some(background_pull));
        self.event_subscription.replace(Some(event_subscription));
        log::info!(
            "repository monitor subscribed workspace={} key={}",
            workspace.display_name,
            workspace_key
        );
    }

    fn ensure_for_workspace(&self, state: &Rc<AppState>) {
        let workspace_key = state.workspace_ref.borrow().id.to_string();
        if self.workspace_key.borrow().as_deref() == Some(workspace_key.as_str()) {
            return;
        }

        self.restart(state);
    }

    fn stop(&self) {
        self.event_subscription.borrow_mut().take();
        self.subscription.borrow_mut().take();
        self.background_pull.borrow_mut().take();
        self.workspace_key.take();
    }
}

impl content::RepositoryActionContext for Rc<AppState> {
    fn local_workspace_path(&self) -> Option<PathBuf> {
        (self.system_ref.borrow().provider_kind == crate::system::ProviderKind::Local)
            .then(|| self.repo_path.borrow().clone())
    }

    fn workspace_root(&self) -> WorkspacePath {
        self.workspace_ref.borrow().root.clone()
    }

    fn url_opener(&self) -> Option<Arc<dyn UrlOpenAccess>> {
        let system = self.system_ref.borrow().clone();
        let workspace = self.workspace_ref.borrow().clone();
        self.providers.url_opener(&system.id, &workspace)
    }

    fn terminal_links(&self) -> Option<Arc<dyn TerminalLinkAccess>> {
        let system = self.system_ref.borrow().clone();
        let workspace = self.workspace_ref.borrow().clone();
        self.providers.terminal_links(&system.id, &workspace)
    }

    fn open_external_terminal_path(
        &self,
        path: &WorkspacePath,
        line: Option<usize>,
        column: Option<usize>,
    ) {
        prompt_open_external_terminal_path(self, path, line, column);
    }

    fn shell(&self) -> Option<Arc<dyn ShellAccess>> {
        let system = self.system_ref.borrow().clone();
        let workspace = self.workspace_ref.borrow().clone();
        self.providers.shell(&system.id, &workspace)
    }

    fn window(&self) -> adw::ApplicationWindow {
        self.window.clone()
    }

    fn refresh(&self, message: Option<String>) {
        refresh(self, message);
    }

    fn show_toast(&self, message: &str) {
        self.as_ref().show_toast(message);
    }

    fn run_git_action(&self) {
        run_git_action(self);
    }

    fn dispatch_command(&self, command: PageCommand) -> PageCommandResult {
        match command {
            PageCommand::OpenFileLocation { path, line, column } => {
                route_open_file_location(self, path, line, column)
            }
            command => dispatch_page_command(self, command),
        }
    }
}

fn prompt_open_external_terminal_path(
    state: &Rc<AppState>,
    path: &WorkspacePath,
    line: Option<usize>,
    column: Option<usize>,
) {
    let system = state.system_ref.borrow().clone();
    let (workspace_path, selected_path) =
        craic_system::workspace::external_workspace_location(&path.absolute);
    let workspace = match system.provider_kind {
        crate::system::ProviderKind::Local => {
            crate::config::ConfiguredWorkspace::local(workspace_path)
        }
        crate::system::ProviderKind::Ssh => {
            let Some(host) = system.host.as_ref().map(|host| host.label().to_string()) else {
                let message = "The SSH host for this terminal link is unavailable.";
                log::warn!(
                    "external terminal path launch failed path={} reason=missing-ssh-host",
                    path.absolute
                );
                state.show_toast(message);
                return;
            };
            crate::config::ConfiguredWorkspace {
                path: workspace_path,
                provider: crate::config::WorkspaceProvider::Ssh { host },
                display_name: None,
                color: None,
            }
        }
        crate::system::ProviderKind::Container => {
            let message = "Opening external terminal paths is unavailable for this provider.";
            log::warn!(
                "external terminal path launch failed path={} provider={} reason=unsupported-provider",
                path.absolute,
                system.provider_kind
            );
            state.show_toast(message);
            return;
        }
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Open in New Craic Window?")
        .body(format!(
            "The terminal path is outside the current workspace:\n\n{}",
            path.absolute
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Open");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);

    let window = state.window.clone();
    let callback_window = window.clone();
    let target = path.absolute.clone();
    dialog.choose(
        Some(&window),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response.as_str() != "open" {
                log::debug!("external terminal path launch cancelled path={target}");
                return;
            }

            match launch_workspace_location_in_new_instance(
                &workspace,
                &selected_path,
                line,
                column,
            ) {
                Ok(()) => log::info!(
                    "external terminal path launch accepted path={target} workspace={} selected_path={selected_path}",
                    workspace.path
                ),
                Err(err) => {
                    log::warn!("external terminal path launch failed path={target}: {err}");
                    show_error_dialog(&callback_window, "Open Terminal Path Failed", &err);
                }
            }
        },
    );
}

fn connect_git_actions(state: &Rc<AppState>) {
    connect_shell_actions(state);
    state.content.connect_repository_actions(state.clone());
    connect_branch_actions(state);
}

fn connect_window_close_confirmation(state: &Rc<AppState>) {
    let confirmed_close = Rc::new(Cell::new(false));

    state.window.connect_close_request({
        let state = state.clone();
        let confirmed_close = confirmed_close.clone();

        move |window| {
            if confirmed_close.get() {
                return gtk::glib::Propagation::Proceed;
            }

            let running_agent_sessions = running_agent_session_count(&state);
            let active_terminal_tasks = state.content.active_terminal_task_count();
            if running_agent_sessions == 0 && active_terminal_tasks == 0 {
                return gtk::glib::Propagation::Proceed;
            }

            log::info!(
                "window close confirmation requested running_agent_sessions={} active_terminal_tasks={}",
                running_agent_sessions,
                active_terminal_tasks
            );
            confirm_close_with_running_tasks(
                window,
                running_agent_sessions,
                active_terminal_tasks,
                &confirmed_close,
            );
            gtk::glib::Propagation::Stop
        }
    });
}

fn running_agent_session_count(state: &AppState) -> usize {
    state
        .pages
        .iter()
        .map(|page| page.running_agent_session_count())
        .sum()
}

fn confirm_close_with_running_tasks(
    window: &adw::ApplicationWindow,
    active_agent_sessions: usize,
    active_terminal_tasks: usize,
    confirmed_close: &Rc<Cell<bool>>,
) {
    let body = close_confirmation_body(active_agent_sessions, active_terminal_tasks);
    let dialog = adw::AlertDialog::builder()
        .heading(close_confirmation_heading(
            active_agent_sessions,
            active_terminal_tasks,
        ))
        .body(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("close", "Close Window");
    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(window), None::<&gtk::gio::Cancellable>, {
        let window = window.clone();
        let confirmed_close = confirmed_close.clone();

        move |response| {
            if response.as_str() != "close" {
                return;
            }

            confirmed_close.set(true);
            window.close();
        }
    });
}

fn close_confirmation_heading(
    active_agent_sessions: usize,
    active_terminal_tasks: usize,
) -> &'static str {
    match (active_agent_sessions > 0, active_terminal_tasks > 0) {
        (true, true) => "Close Window with Running Tasks?",
        (true, false) => "Close Window with Running Agent?",
        (false, true) => "Close Window with Running Terminal?",
        (false, false) => "Close Window?",
    }
}

fn close_confirmation_body(active_agent_sessions: usize, active_terminal_tasks: usize) -> String {
    match (active_agent_sessions, active_terminal_tasks) {
        (1, 0) => {
            "An agent session is still running. Closing this window will terminate it.".to_string()
        }
        (agents, 0) => {
            format!(
                "{agents} agent sessions are still running. Closing this window will terminate them."
            )
        }
        (0, 1) => {
            "A terminal program is still running. Closing this window will terminate it.".to_string()
        }
        (0, tasks) => {
            format!(
                "{tasks} terminal programs are still running. Closing this window will terminate them."
            )
        }
        (1, 1) => {
            "An agent session and a terminal program are still running. Closing this window will terminate them.".to_string()
        }
        (1, tasks) => {
            format!(
                "An agent session and {tasks} terminal programs are still running. Closing this window will terminate them."
            )
        }
        (agents, 1) => {
            format!(
                "{agents} agent sessions and a terminal program are still running. Closing this window will terminate them."
            )
        }
        (agents, tasks) => {
            format!(
                "{agents} agent sessions and {tasks} terminal programs are still running. Closing this window will terminate them."
            )
        }
    }
}

fn start_repository_monitor(state: &Rc<AppState>) {
    state.repository_monitor.restart(state);
}

fn refresh(state: &Rc<AppState>, message: Option<String>) {
    request_workspace_refresh(state, message, true, true);
}

fn refresh_with_toast(state: &Rc<AppState>, message: Option<String>, show_toast: bool) {
    request_workspace_refresh(state, message, show_toast, true);
}

fn refresh_active_repo_metadata(state: &Rc<AppState>, item_id: Option<String>) {
    let system = state.system_ref.borrow().clone();
    let workspace = state.workspace_ref.borrow().clone();
    let workspace_host = system.host.as_ref().map(|host| host.label().to_string());
    let Some(git_handle) = git_handle_for_workspace(state, &system.id, &workspace) else {
        log::debug!(
            "skipping repo metadata refresh workspace={} reason=no-git-handle",
            workspace.display_name
        );
        state
            .sidebar
            .repository_picker
            .set_button_icon("folder-symbolic");
        return;
    };
    let github_access = craic_vcs::github_access(&state.providers, &system, &workspace);
    log::debug!(
        "repo metadata refresh queued workspace={} key={}",
        workspace.display_name,
        workspace.id
    );
    state.sidebar.refresh_workspace_repo_metadata(
        workspace.id.to_string(),
        item_id,
        workspace_host,
        git_handle,
        github_access,
    );
}

fn refresh_from_monitor(state: &Rc<AppState>) {
    request_workspace_refresh(state, None, false, false);
}

fn request_workspace_refresh(
    state: &Rc<AppState>,
    message: Option<String>,
    show_toast: bool,
    force_update: bool,
) {
    let command = AppCommand::RefreshWorkspace(WorkspaceRefreshOptions {
        message,
        show_toast,
        force_update,
    });
    if let Err(command) = state.app_handle.try_send(command) {
        log::warn!("GTK workspace refresh queue rejected command={command:?}");
    }
}

fn execute_workspace_refresh(state: &Rc<AppState>, request: WorkspaceRefreshRequest) {
    if request.identity.workspace_generation != state.workspace_generation.get() {
        log::debug!(
            "ignored stale GTK workspace refresh request={} generation={}",
            request.identity.request_id,
            request.identity.workspace_generation.get()
        );
        complete_workspace_refresh(state, request.identity, None, true);
        return;
    }
    state
        .workspace_refresh_request
        .replace(Some(request.clone()));

    let repo_path = state.repo_path.borrow().clone();
    state.repository_monitor.ensure_for_workspace(state);
    if state.system_ref.borrow().provider_kind == crate::system::path::ProviderKind::Local {
        state.content.refresh_run_targets(&repo_path);
    } else {
        state.content.clear_run_targets();
    }
    let message = request.options.message.clone();
    let show_toast = request.options.show_toast;
    let force_update = request.options.force_update;

    let workspace_key = state.workspace_ref.borrow().id.to_string();
    let system_id = state.system_ref.borrow().id.clone();
    let workspace_ref = state.workspace_ref.borrow().clone();
    let workspace_name = workspace_ref.display_name.clone();
    let Some(git_handle) = git_handle_for_workspace(state, &system_id, &workspace_ref) else {
        log::debug!("refresh without git metadata for workspace={workspace_name}");
        let snapshot = git::WorkspaceSnapshot::NonRepository {
            name: workspace_name,
        };

        let should_update_snapshot = state.last_snapshot.borrow().as_ref() != Some(&snapshot);
        if should_update_snapshot || force_update || message.is_some() {
            let workspace_key = state.workspace_ref.borrow().id.to_string();
            let system = state.system_ref.borrow().clone();
            state
                .sidebar
                .update_workspace(&snapshot, &workspace_key, &system);
            state
                .content
                .update_workspace(&snapshot, state.git_action_running.get());
            if let Some(message) = message.as_deref()
                && show_toast
            {
                state.show_toast(message);
            }
            state.last_snapshot.replace(Some(snapshot.clone()));
            state
                .last_snapshot_repo
                .replace(Some(state.repo_path.borrow().clone()));
            state.last_error.borrow_mut().take();
            let completion_state = state.clone();
            let identity = request.identity;
            refresh_pages(&state.pages, &snapshot, move || {
                complete_workspace_refresh(&completion_state, identity, None, false);
            });
            return;
        } else if let Some(message) = message.as_deref()
            && show_toast
        {
            state.show_toast(message);
        }

        complete_workspace_refresh(state, request.identity, None, false);
        return;
    };

    request_provider_workspace_snapshot(workspace_key.clone(), git_handle, {
        let state = state.clone();
        move |response_workspace_key, result| {
            if !workspace_refresh_request_is_current(&state, request.identity, &workspace_key) {
                log::debug!(
                    "discarding stale GTK workspace refresh request={} workspace={}",
                    request.identity.request_id,
                    workspace_key
                );
                complete_workspace_refresh(&state, request.identity, None, true);
                return;
            } else if response_workspace_key != workspace_key {
                let message = format!(
                    "discarding stale snapshot response for {} (current workspace {})",
                    response_workspace_key, workspace_key,
                );
                log::warn!("{message}");
                complete_workspace_refresh(&state, request.identity, Some(message), false);
                return;
            } else {
                match result {
                    Ok(snapshot) => {
                        let repo_changed = state.last_snapshot_repo.borrow().as_deref()
                            != Some(repo_path.as_path());
                        let snapshot_changed =
                            state.last_snapshot.borrow().as_ref() != Some(&snapshot);
                        let recovering_from_error = state.last_error.borrow().is_some();
                        let should_update_snapshot =
                            snapshot_changed || repo_changed || recovering_from_error;

                        if !should_update_snapshot && !force_update && message.is_none() {
                            log::debug!(
                                "skipping workspace refresh for {} due no changes",
                                repo_path.display(),
                            );
                            complete_workspace_refresh(&state, request.identity, None, false);
                            return;
                        }

                        state.last_error.borrow_mut().take();
                        state.last_snapshot.replace(Some(snapshot.clone()));
                        state.last_snapshot_repo.replace(Some(repo_path.clone()));

                        if should_update_snapshot || force_update {
                            let workspace_key = state.workspace_ref.borrow().id.to_string();
                            let system = state.system_ref.borrow().clone();
                            state
                                .sidebar
                                .update_workspace(&snapshot, &workspace_key, &system);
                            state
                                .content
                                .update_workspace(&snapshot, state.git_action_running.get());
                            if let Some(message) = message.as_deref()
                                && show_toast
                            {
                                state.show_toast(message);
                            }
                            let completion_state = state.clone();
                            let identity = request.identity;
                            refresh_pages(&state.pages, &snapshot, move || {
                                complete_workspace_refresh(
                                    &completion_state,
                                    identity,
                                    None,
                                    false,
                                );
                            });
                            return;
                        } else if let Some(message) = message.as_deref()
                            && show_toast
                        {
                            state.show_toast(message);
                        }
                    }
                    Err(err) => {
                        state.last_snapshot.borrow_mut().take();
                        state.last_snapshot_repo.borrow_mut().take();
                        let workspace_name = state.workspace_ref.borrow().display_name.clone();
                        state.sidebar.set_error(&err, &workspace_name);
                        state.content.set_error(&err);
                        for page in &state.pages {
                            page.set_error(&err);
                        }
                        if state.last_error.borrow().as_deref() != Some(err.as_str()) {
                            *state.last_error.borrow_mut() = Some(err.clone());
                            show_error_dialog(&state.window, "Repository Error", &err);
                        }
                        complete_workspace_refresh(&state, request.identity, Some(err), false);
                        return;
                    }
                }
            }
            complete_workspace_refresh(&state, request.identity, None, false);
        }
    })
}

fn workspace_refresh_request_is_current(
    state: &AppState,
    identity: WorkspaceRefreshIdentity,
    workspace_key: &str,
) -> bool {
    state.workspace_generation.get() == identity.workspace_generation
        && state.workspace_ref.borrow().id.to_string() == workspace_key
        && state
            .workspace_refresh_request
            .borrow()
            .as_ref()
            .is_some_and(|request| request.identity == identity)
}

fn complete_workspace_refresh(
    state: &AppState,
    identity: WorkspaceRefreshIdentity,
    error: Option<String>,
    cancelled: bool,
) {
    let is_current = state
        .workspace_refresh_request
        .borrow()
        .as_ref()
        .is_some_and(|request| request.identity == identity);
    if is_current {
        state.workspace_refresh_request.borrow_mut().take();
    }
    let completion = if cancelled {
        WorkspaceRefreshCompletion::Cancelled(identity)
    } else if let Some(message) = error {
        WorkspaceRefreshCompletion::Failed { identity, message }
    } else {
        WorkspaceRefreshCompletion::Succeeded(identity)
    };
    if let Err(command) = state
        .app_handle
        .try_send(AppCommand::WorkspaceRefreshCompleted(completion))
    {
        log::warn!("GTK workspace refresh completion queue rejected command={command:?}");
    }
}

fn refresh_pages<F>(pages: &[pages::PageRef], snapshot: &git::WorkspaceSnapshot, completion: F)
where
    F: Fn() + 'static,
{
    if pages.is_empty() {
        completion();
        return;
    }

    let remaining = Rc::new(Cell::new(pages.len()));
    let completion = Rc::new(completion);
    for page in pages {
        let remaining = remaining.clone();
        let completion = completion.clone();
        page.refresh(
            snapshot,
            Rc::new(move || {
                let previous = remaining.get();
                if previous == 0 {
                    return;
                }

                remaining.set(previous - 1);
                if previous == 1 {
                    completion();
                }
            }),
        );
    }
}

fn apply_app_core_event(state: &Rc<AppState>, event: UiEvent) {
    match event {
        UiEvent::ApplicationState(view_state) => {
            if state.workspace_generation.get() != view_state.workspace_generation {
                state
                    .workspace_generation
                    .set(view_state.workspace_generation);
                state.workspace_refresh_request.borrow_mut().take();
            }
            if let Some(page) = view_state.active_page.as_ref()
                && let Some(index) = state
                    .pages
                    .iter()
                    .position(|candidate| candidate.id() == *page)
            {
                activate_page(state, index);
            }
        }
        UiEvent::PageState {
            page,
            revision,
            state: page_state,
        } => apply_app_core_page_state(state, &page, revision, &page_state),
        UiEvent::PageCommand(command) => apply_app_core_page_command(state, command),
        UiEvent::PageServiceRequest(request) => route_app_core_page_service(state, request),
        UiEvent::WorkspaceRefreshRequest(request) => execute_workspace_refresh(state, request),
        UiEvent::Effect(request) => {
            log::warn!(
                "GTK received an unsupported app-core UI effect id={:?}",
                request.id
            );
        }
        UiEvent::ShutdownReady => log::debug!("GTK app-core shutdown ready"),
    }
}

fn apply_app_core_page_command(state: &Rc<AppState>, command: CorePageCommand) {
    if command.page.as_ref().map(CorePageId::as_str) != Some("files")
        || command.action.as_str() != "open-file-location"
    {
        log::warn!(
            "GTK ignored unsupported app-core page command page={} action={}",
            command
                .page
                .as_ref()
                .map(CorePageId::as_str)
                .unwrap_or("none"),
            command.action.as_str()
        );
        return;
    }
    let Some(payload) = command.payload.as_object() else {
        log::warn!("GTK ignored malformed open-file-location payload: expected object");
        return;
    };
    let Some(path) = payload.get("path").and_then(serde_json::Value::as_str) else {
        log::warn!("GTK ignored malformed open-file-location payload: invalid path");
        return;
    };
    let line = match craic_app_core::optional_usize(payload.get("line")) {
        Ok(line) => line,
        Err(()) => {
            log::warn!("GTK ignored malformed open-file-location payload: invalid line");
            return;
        }
    };
    let column = match craic_app_core::optional_usize(payload.get("column")) {
        Ok(column) => column,
        Err(()) => {
            log::warn!("GTK ignored malformed open-file-location payload: invalid column");
            return;
        }
    };
    if dispatch_page_command(
        state,
        PageCommand::OpenFileLocation {
            path: path.to_string(),
            line,
            column,
        },
    ) == PageCommandResult::Ignored
    {
        log::warn!("GTK Files page ignored app-core open-file-location command");
    }
}

fn apply_app_core_page_state(
    state: &AppState,
    page: &CorePageId,
    revision: u64,
    page_state: &PageViewState,
) {
    let mut revisions = state.page_state_revisions.borrow_mut();
    let current = revisions.entry(page.clone()).or_default();
    if revision <= *current {
        return;
    }
    *current = revision;
    drop(revisions);
    let Some(index) = state
        .pages
        .iter()
        .position(|candidate| candidate.id() == *page)
    else {
        log::warn!(
            "GTK app-core page state targets unknown page={}",
            page.as_str()
        );
        return;
    };
    state
        .sidebar
        .set_page_refreshing(index, page_state.refreshing);
    state.sidebar.set_page_badge(
        index,
        page_state
            .badge
            .as_ref()
            .filter(|badge| !badge.text.is_empty())
            .map(|badge| pages::PageBadge::new(badge.text.clone())),
    );
}

fn route_app_core_page_service(state: &Rc<AppState>, request: PageServiceRequest) {
    let Some(page) = request.command.page.clone() else {
        complete_app_core_page_service(
            state,
            &request,
            Some("The routed page command has no target".to_string()),
        );
        return;
    };
    if request.command.action.as_str() != "refresh" {
        complete_app_core_page_service(
            state,
            &request,
            Some(format!(
                "Unsupported GTK page command: {}",
                request.command.action.as_str()
            )),
        );
        return;
    }
    let Some(index) = state
        .pages
        .iter()
        .position(|candidate| candidate.id() == page)
    else {
        complete_app_core_page_service(
            state,
            &request,
            Some(format!("Unknown GTK page: {}", page.as_str())),
        );
        return;
    };
    state
        .page_service_requests
        .borrow_mut()
        .insert(page.clone(), request.clone());
    execute_page_refresh(state, page, index, request.page_generation);
}

fn complete_app_core_page_service(
    state: &AppState,
    request: &PageServiceRequest,
    error: Option<String>,
) {
    let completion = match error {
        None => ServiceCompletion::Succeeded {
            request_id: request.request_id,
            generation: request.page_generation,
            payload: request.command.payload.clone(),
        },
        Some(message) => ServiceCompletion::Failed {
            request_id: request.request_id,
            generation: request.page_generation,
            message,
        },
    };
    if let Err(command) = state
        .app_handle
        .try_send(AppCommand::ServiceCompleted(completion))
    {
        log::warn!("GTK page service completion queue rejected command={command:?}");
    }
}

fn refresh_active_page(state: &Rc<AppState>) {
    let Some(page_id) = state.active_page.borrow().clone() else {
        log::warn!("active page refresh ignored because no page is active");
        return;
    };
    if let Err(command) = state
        .app_handle
        .try_send(AppCommand::RoutePageCommand(CorePageCommand {
            page: Some(page_id),
            action: ActionId::new("refresh"),
            payload: Default::default(),
        }))
    {
        log::warn!("GTK active page refresh queue rejected command={command:?}");
    }
}

fn route_open_file_location(
    state: &AppState,
    path: String,
    line: Option<usize>,
    column: Option<usize>,
) -> PageCommandResult {
    let command = AppCommand::RoutePageCommand(CorePageCommand {
        page: Some(CorePageId::new("files")),
        action: ActionId::new("open-file-location"),
        payload: serde_json::json!({
            "path": path,
            "line": line,
            "column": column,
        }),
    });
    if let Err(command) = state.app_handle.try_send(command) {
        log::warn!("GTK open-file-location queue rejected command={command:?}");
        PageCommandResult::Ignored
    } else {
        PageCommandResult::HandledAndActivate
    }
}

fn execute_page_refresh(
    state: &Rc<AppState>,
    page_id: pages::PageId,
    index: usize,
    generation: Generation,
) {
    let page = state.pages[index].clone();

    let label = page.label();
    log::info!(
        "page refresh requested page={} label={label}",
        page_id.as_str()
    );
    let state_weak = Rc::downgrade(state);
    let refresh_page = page.clone();
    let initialize_page_id = page_id.clone();
    page.initialize(Box::new(move |_, _| {
        let Some(state) = state_weak.upgrade() else {
            return;
        };
        if !page_refresh_generation_is_current(&state, &initialize_page_id, generation) {
            log::trace!(
                "ignored page refresh after stale initialization page={} label={} generation={}",
                initialize_page_id.as_str(),
                label,
                generation.get()
            );
            return;
        }

        let state_weak = Rc::downgrade(&state);
        let completion_page_id = initialize_page_id.clone();
        let completion: pages::PageRefreshComplete = Rc::new(move || {
            if let Some(state) = state_weak.upgrade() {
                complete_page_refresh(&state, &completion_page_id, index, generation);
            }
        });

        match refresh_page.refresh_page(completion) {
            PageRefreshRequest::WorkspaceSnapshot => {
                refresh_workspace_page(
                    &state,
                    initialize_page_id.clone(),
                    index,
                    generation,
                    refresh_page,
                );
            }
            PageRefreshRequest::Custom => {
                log::debug!("page refresh delegated to page index={index} label={label}");
            }
        }
    }));
}

fn refresh_workspace_page(
    state: &Rc<AppState>,
    page_id: pages::PageId,
    index: usize,
    generation: Generation,
    page: pages::PageRef,
) {
    let repo_path = state.repo_path.borrow().clone();
    let workspace_key = state.workspace_ref.borrow().id.to_string();
    let system_id = state.system_ref.borrow().id.clone();
    let workspace_ref = state.workspace_ref.borrow().clone();
    let label = page.label();
    log::debug!(
        "page workspace snapshot refresh queued index={} label={} repo={}",
        index,
        label,
        repo_path.display()
    );

    let Some(git_handle) = git_handle_for_workspace(state, &system_id, &workspace_ref) else {
        let snapshot = git::WorkspaceSnapshot::NonRepository {
            name: workspace_ref.display_name,
        };
        let state_weak = Rc::downgrade(state);
        page.refresh(
            &snapshot,
            Rc::new(move || {
                if let Some(state) = state_weak.upgrade() {
                    complete_page_refresh(&state, &page_id, index, generation);
                }
            }),
        );
        return;
    };

    request_provider_workspace_snapshot(workspace_key.clone(), git_handle, {
        let state = state.clone();
        move |response_workspace_key, result| {
            if !page_refresh_generation_is_current(&state, &page_id, generation) {
                log::trace!(
                    "discarding stale page refresh result index={} label={} generation={}",
                    index,
                    label,
                    generation.get()
                );
                return;
            }

            if response_workspace_key != workspace_key {
                log::warn!(
                    "discarding page refresh response for {} (requested {})",
                    response_workspace_key,
                    workspace_key,
                );
                complete_page_refresh_result(
                    &state,
                    &page_id,
                    index,
                    generation,
                    Some("The workspace refresh response did not match its request".to_string()),
                );
                return;
            }

            if state.workspace_ref.borrow().id.to_string() != workspace_key {
                log::debug!(
                    "discarding page refresh for inactive workspace {}",
                    workspace_key
                );
                complete_page_refresh_result(
                    &state,
                    &page_id,
                    index,
                    generation,
                    Some("The page refresh completed for an inactive workspace".to_string()),
                );
                return;
            }

            match result {
                Ok(snapshot) => {
                    log::info!("page refresh completed index={index} label={label}");
                    let state_weak = Rc::downgrade(&state);
                    let completion_page_id = page_id.clone();
                    page.refresh(
                        &snapshot,
                        Rc::new(move || {
                            if let Some(state) = state_weak.upgrade() {
                                complete_page_refresh(
                                    &state,
                                    &completion_page_id,
                                    index,
                                    generation,
                                );
                            }
                        }),
                    );
                    return;
                }
                Err(err) => {
                    log::warn!("page refresh failed index={index} label={label}: {err}");
                    page.set_error(&err);
                    show_error_dialog(&state.window, "Refresh Failed", &err);
                    complete_page_refresh_result(&state, &page_id, index, generation, Some(err));
                }
            }
        }
    });
}

fn complete_page_refresh(
    state: &Rc<AppState>,
    page_id: &pages::PageId,
    index: usize,
    generation: Generation,
) {
    complete_page_refresh_result(state, page_id, index, generation, None);
}

fn complete_page_refresh_result(
    state: &Rc<AppState>,
    page_id: &pages::PageId,
    index: usize,
    generation: Generation,
    error: Option<String>,
) {
    let request = state
        .page_service_requests
        .borrow()
        .get(page_id)
        .filter(|request| request.page_generation == generation)
        .cloned();
    let Some(request) = request else {
        log::trace!(
            "ignored stale page refresh completion page={} generation={}",
            page_id.as_str(),
            generation.get()
        );
        return;
    };

    state.page_service_requests.borrow_mut().remove(page_id);
    complete_app_core_page_service(state, &request, error);
    log::debug!(
        "page refresh completion submitted page={} index={} generation={}",
        page_id.as_str(),
        index,
        generation.get()
    );
}

fn page_refresh_generation_is_current(
    state: &AppState,
    page_id: &pages::PageId,
    generation: Generation,
) -> bool {
    state
        .page_service_requests
        .borrow()
        .get(page_id)
        .is_some_and(|request| request.page_generation == generation)
}

fn activate_page(state: &Rc<AppState>, index: usize) {
    if index >= state.pages.len() {
        return;
    }

    let page_id = state.pages[index].id();
    if state.active_page.borrow().as_ref() != Some(&page_id) {
        state.active_page.replace(Some(page_id.clone()));
        state.page_host.show(&state.pages, index);
        let page = state.pages[index].clone();
        let activate_page = page.clone();
        page.initialize(Box::new(move |_, _| activate_page.activate()));
        if let Err(command) = state.app_handle.try_send(AppCommand::ActivatePage(page_id)) {
            log::warn!("GTK page activation queue rejected command={command:?}");
        }
    }

    if let Some(button) = state.sidebar.mode_switcher.buttons.get(index) {
        if !button.is_active() {
            button.set_active(true);
        }
    }
}

fn dispatch_page_command(state: &Rc<AppState>, command: PageCommand) -> PageCommandResult {
    let mut handled = false;

    for (index, page) in state.pages.iter().enumerate() {
        match page.handle_command(&command) {
            PageCommandResult::Ignored => {}
            PageCommandResult::Handled => handled = true,
            PageCommandResult::HandledAndActivate => {
                activate_page(state, index);
                return PageCommandResult::HandledAndActivate;
            }
        }
    }

    if handled {
        PageCommandResult::Handled
    } else {
        PageCommandResult::Ignored
    }
}

fn broadcast_page_command(state: &Rc<AppState>, command: PageCommand) {
    for page in &state.pages {
        page.handle_command(&command);
    }
}
