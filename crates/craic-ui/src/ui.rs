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
use crate::system::providers::ssh::{SshProvider, SshProviderConfig};
use crate::system::{SystemProviderRegistry, SystemRef, WorkspacePath, WorkspaceRef};
use adw::prelude::*;
use app_menu::{app_menu, install_actions};
use branch_actions::connect_branch_actions;
use dialogs::show_error_dialog;
use dialogs::show_startup_crash_dialog;
use git_actions::run_git_action;
use pages::{PageCommand, PageCommandResult, PageRefreshRequest};
use pango::prelude::FontMapExt;
use shell_actions::connect_shell_actions;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const JETBRAINS_MONO_DIR: &str = "JetBrainsMono";
const GIT_SNAPSHOT_POLL_INTERVAL: Duration = Duration::from_millis(75);

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

pub fn build_ui(
    app: &adw::Application,
    launch_start: Instant,
    startup_workspace: Option<crate::config::ConfiguredWorkspace>,
    startup_error: Option<String>,
) {
    let mut startup = StartupTimer::new(launch_start);
    startup.mark("activate");

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    startup.mark("style-manager");

    register_bundled_fonts();
    startup.mark("register-bundled-fonts");

    install_actions(app);
    startup.mark("install-actions");

    let provider = gtk::CssProvider::new();
    let workspace_color_provider = gtk::CssProvider::new();
    provider.load_from_data(&format!(
        "{}{}",
        components::search::SEARCH_PANEL_CSS,
        r#"
        .changes-badge, .agent-badge {
            background-color: @accent_bg_color;
            color: @accent_fg_color;
            border-radius: 9999px;
            font-weight: bold;
            font-size: 0.68em;
            min-width: 14px;
            min-height: 14px;
            padding: 0;
        }
        .git-action-card {
            border: 1px solid rgba(53, 132, 228, 0.4);
            background-color: rgba(53, 132, 228, 0.05);
            border-radius: 12px;
        }
        textview.agent-message-text,
        textview.agent-message-text text,
        textview.agent-transcript-text,
        textview.agent-transcript-text text {
            background-color: transparent;
        }
        .agent-transcript-icon {
            -gtk-icon-size: 16px;
            padding-right: 4px;
        }
        .pdf-preview-scroller {
            background-color: @window_bg_color;
        }
        .pdf-preview-page {
            background-color: @view_bg_color;
            border: 1px solid alpha(@view_fg_color, 0.08);
            border-radius: 2px;
            box-shadow: 0 3px 12px rgba(0, 0, 0, 0.32);
        }
        button.terminal-session-close-button {
            min-width: 0;
            min-height: 0;
            padding: 3px;
        }
        .svg-preview-scroller {
            border: none;
            background-color: transparent;
        }
        .markdown-preview {
            border: none;
            background-color: transparent;
            box-shadow: none;
        }
        textview.agent-composer-input,
        textview.agent-composer-input text {
            border-radius: 8px;
            font-size: 1.1em;
        }
        .code-editor-completion-list {
            padding: 4px;
            background-color: transparent;
        }
        .code-editor-completion-row {
            padding: 5px 10px;
        }
        .code-editor-completion-label {
            font-family: monospace;
        }
    "#
    ));
    startup.mark("load-css");

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &workspace_color_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        let icon_theme = gtk::IconTheme::for_display(&display);
        let mut search_paths: Vec<PathBuf> = asset_search_paths()
            .into_iter()
            .filter(|path| path.is_dir())
            .collect();

        for path in icon_theme.search_path() {
            if !search_paths.contains(&path) {
                search_paths.push(path);
            }
        }
        let search_path_refs: Vec<&std::path::Path> =
            search_paths.iter().map(|path| path.as_path()).collect();
        icon_theme.set_search_path(&search_path_refs);
        log::info!(
            "startup icon search paths configured count={}",
            search_path_refs.len()
        );
    } else {
        log::warn!("startup icon search paths skipped because GTK display is unavailable");
    }
    startup.mark("configure-display-assets");

    let menu = app_menu();
    startup.mark("build-app-menu");

    let provider_registry = SystemProviderRegistry::new();
    startup.mark("create-provider-registry");

    let active_workspace = initial_workspace(&provider_registry, startup_workspace.as_ref());
    startup.mark("resolve-initial-workspace");

    let repo_path = active_workspace.repo_path.clone();
    let initial_workspace_key = active_workspace.workspace_ref.id.to_string();
    let initial_snapshot: Option<git::WorkspaceSnapshot> = None;

    let repo_path_cell = Rc::new(RefCell::new(repo_path.clone()));
    let system_ref_cell = Rc::new(RefCell::new(active_workspace.system_ref));
    let workspace_ref_cell = Rc::new(RefCell::new(active_workspace.workspace_ref));
    let window_cell = Rc::new(RefCell::new(None::<adw::ApplicationWindow>));
    let git_action_running = Rc::new(Cell::new(false));
    let state_slot: Rc<RefCell<Weak<AppState>>> = Rc::new(RefCell::new(Weak::new()));

    let page_context = pages::PageContext::new(
        repo_path_cell.clone(),
        system_ref_cell.clone(),
        workspace_ref_cell.clone(),
        provider_registry.clone(),
        window_cell.clone(),
        git_action_running.clone(),
        Rc::new({
            let state_slot = state_slot.clone();
            move |message, show_toast| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    refresh_with_toast(&state, message, show_toast);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move || {
                if let Some(state) = state_slot.borrow().upgrade() {
                    run_git_action(&state);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |message| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    state.show_toast(message);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |working_dir| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    let system = state.system_ref.borrow().clone();
                    let workspace = state.workspace_ref.borrow().clone();
                    let Some(shell) = state.providers.shell(&system.id, &workspace) else {
                        return Err("Terminal is unavailable for this workspace.".to_string());
                    };
                    let command = shell.interactive_shell(Some(working_dir))?;
                    let title = shell.command_display(&command);
                    state.content.run_shell_command(&command, &title)
                } else {
                    Err("Application is not ready.".to_string())
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |command, title| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    state.content.run_shell_command(command, title)
                } else {
                    Err("Application is not ready.".to_string())
                }
            }
        }),
        Rc::new(|workspace_key, git_handle, on_result| {
            request_provider_git_snapshot(workspace_key, git_handle, on_result);
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move || {
                if let Some(state) = state_slot.borrow().upgrade() {
                    state.sidebar.update_page_badges(&state.pages);
                }
            }
        }),
        Rc::new({
            let state_slot = state_slot.clone();
            move |command| {
                if let Some(state) = state_slot.borrow().upgrade() {
                    dispatch_page_command(&state, command);
                }
            }
        }),
    );
    startup.mark("build-page-context");

    let pages = pages::build_pages(page_context);
    startup.mark("build-pages");

    let sidebar = sidebar::build(
        &menu,
        initial_snapshot
            .as_ref()
            .and_then(git::WorkspaceSnapshot::repository),
        &initial_workspace_key,
        &workspace_ref_cell.borrow().display_name,
        &system_ref_cell.borrow(),
        &pages,
    );
    startup.mark("build-sidebar");

    let content = content::build(
        &menu,
        initial_snapshot
            .as_ref()
            .and_then(git::WorkspaceSnapshot::repository),
    );
    startup.mark("build-content");

    if system_ref_cell.borrow().provider_kind == crate::system::path::ProviderKind::Local {
        let step_start = Instant::now();
        content.refresh_run_targets(&repo_path);
        log::info!(
            "startup run targets refreshed repo={} elapsed_ms={}",
            repo_path.display(),
            step_start.elapsed().as_millis()
        );
    } else {
        log::info!(
            "startup run targets skipped provider_kind={}",
            system_ref_cell.borrow().provider_kind
        );
    }
    startup.mark("refresh-run-targets");

    let page_host = pages::PageHost::new(&sidebar.page_slot(), &content.page_slot());
    let page_refresh_generations = (0..pages.len()).map(|_| Cell::new(0)).collect();
    startup.mark("build-page-host");

    let main_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    main_paned.set_start_child(Some(&sidebar.root));
    main_paned.set_end_child(Some(&content.root));
    main_paned.set_resize_start_child(false);
    main_paned.set_shrink_start_child(false);
    main_paned.set_resize_end_child(true);
    main_paned.set_shrink_end_child(false);
    main_paned.set_position(400);

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&main_paned));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Craic")
        .default_width(1440)
        .default_height(920)
        .content(&toast_overlay)
        .build();
    *window_cell.borrow_mut() = Some(window.clone());
    startup.mark("build-window");

    let state = Rc::new(AppState {
        repo_path: repo_path_cell,
        system_ref: system_ref_cell,
        workspace_ref: workspace_ref_cell,
        providers: provider_registry,
        window: window.clone(),
        toast_overlay,
        sidebar,
        content,
        pages,
        page_host,
        active_page: Cell::new(usize::MAX),
        page_refresh_generations,
        git_action_running,
        last_error: RefCell::new(None),
        last_snapshot: RefCell::new(initial_snapshot.clone()),
        last_snapshot_repo: RefCell::new(initial_snapshot.as_ref().map(|_| repo_path.clone())),
        snapshot_sequence: Cell::new(0),
        snapshot_refresh_running: Cell::new(false),
        queued_workspace_refresh: RefCell::new(None),
        repository_monitor: RepositoryMonitor::default(),
        workspace_color_provider,
    });
    *state_slot.borrow_mut() = Rc::downgrade(&state);
    startup.mark("build-app-state");

    state.sidebar.load_repos_async();
    startup.mark("queue-workspace-list-load");

    apply_workspace_color(&state);
    startup.mark("apply-workspace-color");

    activate_page(&state, 0);
    startup.mark("activate-initial-page");

    refresh_active_repo_metadata(&state, None);
    startup.mark("queue-repo-metadata-refresh");

    connect_git_actions(&state);
    startup.mark("connect-actions");

    start_repository_monitor(&state);
    startup.mark("start-repository-monitor");

    refresh(&state, None);
    startup.mark("queue-initial-refresh");

    connect_window_close_confirmation(&state);
    startup.mark("connect-close-confirmation");

    window.present();
    startup.mark("present-window");

    pages::warm_pages_in_background(&state.pages);
    startup.mark("queue-page-background-initialization");

    if let Some(notice) = crate::crash_log::take_latest_crash_notice() {
        log::warn!(
            "showing previous crash notice path={}",
            notice.path.display()
        );
        show_startup_crash_dialog(&window, &notice);
    }
    if let Some(error) = startup_error.as_deref() {
        show_error_dialog(&window, "Open Workspace Failed", error);
    }
}

enum GitWorkerCommand {
    ProviderSnapshot {
        workspace_key: String,
        git_handle: Arc<crate::git::GitRepoHandle>,
        respond_to: mpsc::Sender<(String, Result<git::RepositorySnapshot, String>)>,
    },
    ProviderWorkspaceSnapshot {
        workspace_key: String,
        git_handle: Arc<crate::git::GitRepoHandle>,
        respond_to: mpsc::Sender<(String, Result<git::WorkspaceSnapshot, String>)>,
    },
}

fn git_snapshot_worker() -> &'static mpsc::Sender<GitWorkerCommand> {
    static WORKER: OnceLock<mpsc::Sender<GitWorkerCommand>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<GitWorkerCommand>();
        thread::spawn(move || {
            log::info!("git snapshot worker started");

            for command in receiver {
                match command {
                    GitWorkerCommand::ProviderSnapshot {
                        workspace_key,
                        git_handle,
                        respond_to,
                    } => {
                        let step_start = Instant::now();
                        git_handle.snapshot(Box::new(move |result| {
                            let status = if result.is_ok() { "ok" } else { "error" };
                            log::info!(
                                "git snapshot worker finished workspace={} status={} elapsed_ms={}",
                                workspace_key,
                                status,
                                step_start.elapsed().as_millis()
                            );
                            if respond_to.send((workspace_key.clone(), result)).is_err() {
                                log::warn!(
                                    "git snapshot worker: provider response receiver disconnected for {}",
                                    workspace_key,
                                );
                            }
                        }));
                    }
                    GitWorkerCommand::ProviderWorkspaceSnapshot {
                        workspace_key,
                        git_handle,
                        respond_to,
                    } => {
                        let step_start = Instant::now();
                        git_handle.workspace_snapshot(Box::new(move |result| {
                            let status = if result.is_ok() { "ok" } else { "error" };
                            log::info!(
                                "git workspace snapshot worker finished workspace={} status={} elapsed_ms={}",
                                workspace_key,
                                status,
                                step_start.elapsed().as_millis()
                            );
                            if respond_to.send((workspace_key.clone(), result)).is_err() {
                                log::warn!(
                                    "git workspace snapshot worker: provider response receiver disconnected for {}",
                                    workspace_key,
                                );
                            }
                        }));
                    }
                }
            }

            log::warn!("git snapshot worker stopped");
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
    let (sender, receiver) = mpsc::channel();
    let request_start = Instant::now();

    if let Err(err) = git_snapshot_worker().send(GitWorkerCommand::ProviderSnapshot {
        workspace_key: workspace_key.clone(),
        git_handle,
        respond_to: sender,
    }) {
        log::error!("failed to enqueue provider git snapshot request for {workspace_key}: {err}");
        on_result(
            workspace_key,
            Err("Git snapshot worker is unavailable.".to_string()),
        );
        return;
    }

    log::info!("provider git snapshot queued workspace={workspace_key}");

    gtk::glib::timeout_add_local(GIT_SNAPSHOT_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok((key, result)) => {
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
                gtk::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("provider git snapshot response channel disconnected before completion");
                gtk::glib::ControlFlow::Break
            }
        }
    });
}

pub(crate) fn request_provider_workspace_snapshot<F>(
    workspace_key: String,
    git_handle: Arc<crate::git::GitRepoHandle>,
    on_result: F,
) where
    F: FnMut(String, Result<git::WorkspaceSnapshot, String>) + 'static,
{
    let mut on_result = on_result;
    let (sender, receiver) = mpsc::channel();
    let request_start = Instant::now();

    if let Err(err) = git_snapshot_worker().send(GitWorkerCommand::ProviderWorkspaceSnapshot {
        workspace_key: workspace_key.clone(),
        git_handle,
        respond_to: sender,
    }) {
        log::error!(
            "failed to enqueue provider workspace snapshot request for {workspace_key}: {err}"
        );
        on_result(
            workspace_key,
            Err("Git snapshot worker is unavailable.".to_string()),
        );
        return;
    }

    log::info!("provider workspace snapshot queued workspace={workspace_key}");

    gtk::glib::timeout_add_local(GIT_SNAPSHOT_POLL_INTERVAL, move || {
        match receiver.try_recv() {
            Ok((key, result)) => {
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
                gtk::glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!(
                    "provider workspace snapshot response channel disconnected before completion"
                );
                gtk::glib::ControlFlow::Break
            }
        }
    });
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
    match &workspace.provider {
        crate::config::WorkspaceProvider::Local => {
            let path = crate::config::expand_config_path_for_ui(&workspace.path)
                .unwrap_or_else(|| PathBuf::from(&workspace.path));
            local_active_workspace(registry, path)
        }
        crate::config::WorkspaceProvider::Ssh { host } => {
            let provider = Arc::new(SshProvider::new(SshProviderConfig::new(host.clone())));
            registry.register(provider.clone() as Arc<dyn SystemProvider>);
            let workspace_ref = provider.workspace_for_remote_path(workspace.path.clone());
            ActiveWorkspace {
                repo_path: PathBuf::from(&workspace.path),
                system_ref: provider.system_ref(),
                workspace_ref,
            }
        }
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
    active_page: Cell<usize>,
    page_refresh_generations: Vec<Cell<u64>>,
    git_action_running: Rc<Cell<bool>>,
    last_error: RefCell<Option<String>>,
    last_snapshot: RefCell<Option<git::WorkspaceSnapshot>>,
    last_snapshot_repo: RefCell<Option<PathBuf>>,
    snapshot_sequence: Cell<u64>,
    snapshot_refresh_running: Cell<bool>,
    queued_workspace_refresh: RefCell<Option<QueuedWorkspaceRefresh>>,
    repository_monitor: RepositoryMonitor,
    workspace_color_provider: gtk::CssProvider,
}

impl AppState {
    fn show_toast(&self, message: &str) {
        self.toast_overlay.add_toast(adw::Toast::new(message));
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

#[derive(Clone, Debug)]
struct QueuedWorkspaceRefresh {
    message: Option<String>,
    show_toast: bool,
    force_update: bool,
}

#[derive(Default)]
struct RepositoryMonitor {
    workspace_key: RefCell<Option<String>>,
    subscription: RefCell<Option<crate::git::ChangeListenerSubscription>>,
    background_pull: RefCell<Option<crate::git::BackgroundPullSubscription>>,
    event_source: RefCell<Option<gtk::glib::SourceId>>,
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

        let (sender, receiver) = mpsc::channel();
        let sender = Arc::new(Mutex::new(sender));
        let listener: crate::git::ChangeListener = Arc::new(move || {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(());
            }
        });
        let subscription = git_handle.add_on_change_listener(listener.clone());
        let background_pull = git_handle.schedule_background_pull_loop(Some(listener));

        let state_weak = Rc::downgrade(state);
        let source_workspace_key = workspace_key.clone();
        let source_id = gtk::glib::timeout_add_local(GIT_SNAPSHOT_POLL_INTERVAL, move || {
            let Some(state) = state_weak.upgrade() else {
                return gtk::glib::ControlFlow::Break;
            };

            if state.workspace_ref.borrow().id.to_string() != source_workspace_key {
                log::debug!(
                    "stopping repository monitor event drain for inactive workspace {}",
                    source_workspace_key
                );
                return gtk::glib::ControlFlow::Break;
            }

            let mut should_refresh = false;
            while receiver.try_recv().is_ok() {
                should_refresh = true;
            }

            if should_refresh {
                log::debug!(
                    "refreshing repository workspace={} from provider git watcher",
                    source_workspace_key
                );
                refresh_from_monitor(&state, true);
            }

            gtk::glib::ControlFlow::Continue
        });

        self.subscription.replace(Some(subscription));
        self.background_pull.replace(Some(background_pull));
        self.event_source.replace(Some(source_id));
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
        if let Some(source_id) = self.event_source.borrow_mut().take() {
            source_id.remove();
        }
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
        dispatch_page_command(self, command)
    }
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
    refresh_workspace(state, message, true, true);
}

fn refresh_without_toast(state: &Rc<AppState>, message: Option<String>) {
    refresh_workspace(state, message, false, true);
}

fn refresh_with_toast(state: &Rc<AppState>, message: Option<String>, show_toast: bool) {
    refresh_workspace(state, message, show_toast, true);
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

fn refresh_from_monitor(state: &Rc<AppState>, force_update: bool) {
    refresh_workspace(state, None, false, force_update);
}

fn refresh_workspace(
    state: &Rc<AppState>,
    message: Option<String>,
    show_toast: bool,
    force_update: bool,
) {
    let request = QueuedWorkspaceRefresh {
        message,
        show_toast,
        force_update,
    };
    if state.snapshot_refresh_running.get() {
        if request.message.is_none() && !request.show_toast && !request.force_update {
            log::trace!("skipped workspace polling refresh because a snapshot is already running");
            return;
        }
        queue_workspace_refresh(state, request);
        return;
    }
    state.snapshot_refresh_running.set(true);

    let repo_path = state.repo_path.borrow().clone();
    state.repository_monitor.ensure_for_workspace(state);
    if state.system_ref.borrow().provider_kind == crate::system::path::ProviderKind::Local {
        state.content.refresh_run_targets(&repo_path);
    } else {
        state.content.clear_run_targets();
    }
    let sequence = state.snapshot_sequence.get().wrapping_add(1);
    state.snapshot_sequence.set(sequence);
    let message = request.message.clone();
    let show_toast = request.show_toast;
    let force_update = request.force_update;

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
            refresh_pages(&state.pages, &snapshot, move || {
                complete_snapshot_refresh(&completion_state);
            });
            return;
        } else if let Some(message) = message.as_deref()
            && show_toast
        {
            state.show_toast(message);
        }

        complete_snapshot_refresh(state);
        return;
    };

    request_provider_workspace_snapshot(workspace_key.clone(), git_handle, {
        let state = state.clone();
        move |response_workspace_key, result| {
            if response_workspace_key != workspace_key {
                log::warn!(
                    "discarding stale snapshot response for {} (current workspace {})",
                    response_workspace_key,
                    workspace_key,
                );
            } else if state.snapshot_sequence.get() != sequence {
                log::trace!(
                    "discarding outdated snapshot result for {} (sequence {})",
                    repo_path.display(),
                    sequence,
                );
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
                            complete_snapshot_refresh(&state);
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
                            refresh_pages(&state.pages, &snapshot, move || {
                                complete_snapshot_refresh(&completion_state);
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
                    }
                }
            }
            complete_snapshot_refresh(&state);
        }
    })
}

fn queue_workspace_refresh(state: &Rc<AppState>, request: QueuedWorkspaceRefresh) {
    let mut queued = state.queued_workspace_refresh.borrow_mut();
    let already_queued = queued.is_some();
    match queued.as_mut() {
        Some(existing) => {
            if request.message.is_some() {
                existing.message = request.message;
            }
            existing.show_toast |= request.show_toast;
            existing.force_update |= request.force_update;
        }
        None => {
            *queued = Some(request);
        }
    }
    if already_queued {
        log::trace!("coalesced workspace refresh while snapshot is running");
    } else {
        log::debug!("queued workspace refresh while snapshot is running");
    }
}

fn complete_snapshot_refresh(state: &Rc<AppState>) {
    state.snapshot_refresh_running.set(false);
    let queued = state.queued_workspace_refresh.borrow_mut().take();
    if let Some(queued) = queued {
        log::debug!("running queued workspace refresh after snapshot completion");
        refresh_workspace(
            state,
            queued.message,
            queued.show_toast,
            queued.force_update,
        );
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

fn refresh_active_page(state: &Rc<AppState>) {
    let index = state.active_page.get();
    let Some(page) = state.pages.get(index).cloned() else {
        log::warn!("active page refresh ignored invalid index={index}");
        return;
    };
    let Some(generation_cell) = state.page_refresh_generations.get(index) else {
        log::warn!("active page refresh ignored missing generation index={index}");
        return;
    };

    let generation = generation_cell.get().wrapping_add(1).max(1);
    generation_cell.set(generation);
    state.sidebar.set_page_refreshing(index, true);

    let label = page.label();
    log::info!("page refresh requested index={index} label={label}");
    let state_weak = Rc::downgrade(state);
    let refresh_page = page.clone();
    page.initialize(Box::new(move |_, _| {
        let Some(state) = state_weak.upgrade() else {
            return;
        };
        if !page_refresh_generation_is_current(&state, index, generation) {
            log::trace!(
                "ignored page refresh after stale initialization index={} label={} generation={}",
                index,
                label,
                generation
            );
            return;
        }

        let state_weak = Rc::downgrade(&state);
        let completion: pages::PageRefreshComplete = Rc::new(move || {
            if let Some(state) = state_weak.upgrade() {
                complete_page_refresh(&state, index, generation);
            }
        });

        match refresh_page.refresh_page(completion) {
            PageRefreshRequest::WorkspaceSnapshot => {
                refresh_workspace_page(&state, index, generation, refresh_page);
            }
            PageRefreshRequest::Custom => {
                log::debug!("page refresh delegated to page index={index} label={label}");
            }
        }
    }));
}

fn refresh_workspace_page(
    state: &Rc<AppState>,
    index: usize,
    generation: u64,
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
                    complete_page_refresh(&state, index, generation);
                }
            }),
        );
        return;
    };

    request_provider_workspace_snapshot(workspace_key.clone(), git_handle, {
        let state = state.clone();
        move |response_workspace_key, result| {
            if !page_refresh_generation_is_current(&state, index, generation) {
                log::trace!(
                    "discarding stale page refresh result index={} label={} generation={}",
                    index,
                    label,
                    generation
                );
                return;
            }

            if response_workspace_key != workspace_key {
                log::warn!(
                    "discarding page refresh response for {} (requested {})",
                    response_workspace_key,
                    workspace_key,
                );
                complete_page_refresh(&state, index, generation);
                return;
            }

            if state.workspace_ref.borrow().id.to_string() != workspace_key {
                log::debug!(
                    "discarding page refresh for inactive workspace {}",
                    workspace_key
                );
                complete_page_refresh(&state, index, generation);
                return;
            }

            match result {
                Ok(snapshot) => {
                    log::info!("page refresh completed index={index} label={label}");
                    let state_weak = Rc::downgrade(&state);
                    page.refresh(
                        &snapshot,
                        Rc::new(move || {
                            if let Some(state) = state_weak.upgrade() {
                                complete_page_refresh(&state, index, generation);
                            }
                        }),
                    );
                    return;
                }
                Err(err) => {
                    log::warn!("page refresh failed index={index} label={label}: {err}");
                    page.set_error(&err);
                    show_error_dialog(&state.window, "Refresh Failed", &err);
                }
            }

            complete_page_refresh(&state, index, generation);
        }
    });
}

fn complete_page_refresh(state: &Rc<AppState>, index: usize, generation: u64) {
    if !page_refresh_generation_is_current(state, index, generation) {
        log::trace!("ignored stale page refresh completion index={index} generation={generation}");
        return;
    }

    state.sidebar.set_page_refreshing(index, false);
    log::debug!("page refresh indicator cleared index={index} generation={generation}");
}

fn page_refresh_generation_is_current(state: &AppState, index: usize, generation: u64) -> bool {
    state
        .page_refresh_generations
        .get(index)
        .is_some_and(|cell| cell.get() == generation)
}

fn activate_page(state: &Rc<AppState>, index: usize) {
    if index >= state.pages.len() {
        return;
    }

    if state.active_page.get() != index {
        state.active_page.set(index);
        state.page_host.show(&state.pages, index);
        let page = state.pages[index].clone();
        let activate_page = page.clone();
        page.initialize(Box::new(move |_, _| activate_page.activate()));
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
