use crate::git::WorkspaceSnapshot;
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::system::capabilities::{
    docker::DockerAccess, files::FileAccess, open::DesktopOpenAccess, shell::ShellAccess,
    terminal_link::TerminalLinkAccess, url::UrlOpenAccess,
};
use crate::system::{
    FileNodePath, SystemPath, SystemProviderRegistry, SystemRef, WorkspacePath, WorkspaceRef,
};
use crate::terminal::CommandSpec;
use crate::ui::dialogs::show_error_dialog;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

mod agent;
mod changes;
mod containers;
mod file;
mod history;
mod preview_reconcile;

pub(crate) trait Page {
    fn label(&self) -> &'static str;
    fn icon_name(&self) -> &'static str;
    fn initialize(&self, completion: PageInitializeComplete);
    fn activate(&self) {}
    fn workspace_changed(&self) {}
    fn refresh(&self, snapshot: &WorkspaceSnapshot, completion: PageRefreshComplete);
    fn refresh_page(&self, _completion: PageRefreshComplete) -> PageRefreshRequest {
        PageRefreshRequest::WorkspaceSnapshot
    }
    fn set_error(&self, message: &str);
    fn badge(&self) -> Option<PageBadge> {
        None
    }
    fn running_agent_session_count(&self) -> usize {
        0
    }
    fn toggle_left_search(&self) -> bool {
        false
    }
    fn toggle_right_search(&self) -> bool {
        false
    }
    fn handle_command(&self, _command: &PageCommand) -> PageCommandResult {
        PageCommandResult::Ignored
    }
}

pub(crate) type PageRef = Rc<dyn Page + 'static>;
pub(crate) type PageInitializeComplete = Box<dyn FnOnce(gtk::Widget, gtk::Widget) + 'static>;
pub(crate) type PageRefreshComplete = Rc<dyn Fn() + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageRefreshRequest {
    WorkspaceSnapshot,
    Custom,
}

#[derive(Clone)]
pub(crate) struct PageBadge {
    text: String,
}

impl PageBadge {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone)]
pub(crate) enum PageCommand {
    OpenSearchMatch {
        path: FileNodePath,
        start: usize,
        end: usize,
    },
    OpenFileLocation {
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
    AddFileToAgent(String),
    OpenAgentSession(u64),
    OpenCommit(String),
    ClearSelection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageCommandResult {
    Ignored,
    Handled,
    HandledAndActivate,
}

#[derive(Clone)]
pub(crate) struct PageContext {
    repo_path: Rc<RefCell<PathBuf>>,
    system_ref: Rc<RefCell<SystemRef>>,
    workspace_ref: Rc<RefCell<WorkspaceRef>>,
    providers: SystemProviderRegistry,
    window: Rc<RefCell<Option<adw::ApplicationWindow>>>,
    git_action_running: Rc<Cell<bool>>,
    refresh: Rc<dyn Fn(Option<String>, bool)>,
    show_toast: Rc<dyn Fn(&str)>,
    run_git_action: Rc<dyn Fn()>,
    open_terminal: Rc<dyn Fn(&WorkspacePath) -> Result<(), String>>,
    run_terminal_command: Rc<dyn Fn(&CommandSpec, &str) -> Result<(), String>>,
    run_shell_command: Rc<dyn Fn(&ShellCommandSpec, &str) -> Result<(), String>>,
    notify_badge_changed: Rc<dyn Fn()>,
    dispatch_command: Rc<dyn Fn(PageCommand)>,
}

#[allow(dead_code)]
impl PageContext {
    pub(crate) fn new(
        repo_path: Rc<RefCell<PathBuf>>,
        system_ref: Rc<RefCell<SystemRef>>,
        workspace_ref: Rc<RefCell<WorkspaceRef>>,
        providers: SystemProviderRegistry,
        window: Rc<RefCell<Option<adw::ApplicationWindow>>>,
        git_action_running: Rc<Cell<bool>>,
        refresh: Rc<dyn Fn(Option<String>, bool)>,
        run_git_action: Rc<dyn Fn()>,
        show_toast: Rc<dyn Fn(&str)>,
        open_terminal: Rc<dyn Fn(&WorkspacePath) -> Result<(), String>>,
        run_terminal_command: Rc<dyn Fn(&CommandSpec, &str) -> Result<(), String>>,
        run_shell_command: Rc<dyn Fn(&ShellCommandSpec, &str) -> Result<(), String>>,
        notify_badge_changed: Rc<dyn Fn()>,
        dispatch_command: Rc<dyn Fn(PageCommand)>,
    ) -> Self {
        Self {
            repo_path,
            system_ref,
            workspace_ref,
            providers,
            window,
            git_action_running,
            refresh,
            show_toast,
            run_git_action,
            open_terminal,
            run_terminal_command,
            run_shell_command,
            notify_badge_changed,
            dispatch_command,
        }
    }

    pub(super) fn local_workspace_path(&self) -> Option<PathBuf> {
        (self.system_ref.borrow().provider_kind == crate::system::ProviderKind::Local)
            .then(|| self.repo_path.borrow().clone())
    }

    pub(super) fn system_ref(&self) -> SystemRef {
        self.system_ref.borrow().clone()
    }

    pub(super) fn workspace_ref(&self) -> WorkspaceRef {
        self.workspace_ref.borrow().clone()
    }

    pub(super) fn workspace_key(&self) -> String {
        self.workspace_ref.borrow().id.to_string()
    }

    pub(super) fn workspace_is_current(&self, workspace_key: &str) -> bool {
        self.workspace_ref.borrow().id.to_string() == workspace_key
    }

    pub(super) fn system_path(&self, relative: &str) -> SystemPath {
        let workspace = self.workspace_ref();
        let path = workspace.path(relative);
        SystemPath::new(self.system_ref(), workspace, path)
    }

    pub(super) fn workspace_node_path(&self, relative: &str) -> FileNodePath {
        self.workspace_ref().node_path(&self.system_ref(), relative)
    }

    pub(super) fn workspace_root_node_path(&self) -> FileNodePath {
        self.workspace_ref().root_node_path(&self.system_ref())
    }

    pub(super) fn files(&self) -> Option<Arc<dyn FileAccess>> {
        self.providers
            .files(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn git(&self) -> Option<Arc<crate::git::GitRepoHandle>> {
        let workspace = self.workspace_ref();
        let system_id = self.system_ref.borrow().id.clone();
        let files = self.providers.files(&system_id, &workspace)?;
        let shell = self.providers.shell(&system_id, &workspace)?;
        let mut handle =
            crate::git::GitRepoHandle::new(workspace.clone(), shell.clone(), files.clone());
        let account = crate::workspace_config::git_config_from_file_access(files.as_ref())
            .github_auth_account;
        if let Some(hook) = crate::github::git_auth_hook(shell, workspace.root.clone(), account) {
            handle = handle.with_hook(hook);
        }
        Some(Arc::new(handle))
    }

    pub(super) fn git_unavailable_message(&self) -> String {
        format!(
            "Git is unavailable for workspace {}.",
            self.workspace_ref.borrow().display_name
        )
    }

    pub(super) fn shell(&self) -> Option<Arc<dyn ShellAccess>> {
        self.providers
            .shell(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn docker(&self) -> Option<Arc<dyn DockerAccess>> {
        self.providers
            .docker(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn desktop_opener(&self) -> Option<Arc<dyn DesktopOpenAccess>> {
        self.providers
            .desktop_opener(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn url_opener(&self) -> Option<Arc<dyn UrlOpenAccess>> {
        self.providers
            .url_opener(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn terminal_links(&self) -> Option<Arc<dyn TerminalLinkAccess>> {
        self.providers
            .terminal_links(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn desktop_opener_unavailable_message(&self) -> String {
        format!(
            "Opening paths is unavailable for workspace {}.",
            self.workspace_ref.borrow().display_name
        )
    }

    pub(super) fn url_opener_unavailable_message(&self) -> String {
        format!(
            "Opening links is unavailable for workspace {}.",
            self.workspace_ref.borrow().display_name
        )
    }

    pub(super) fn window(&self) -> Option<adw::ApplicationWindow> {
        self.window.borrow().clone()
    }

    pub(super) fn show_error(&self, heading: &str, message: &str) {
        if let Some(window) = self.window() {
            show_error_dialog(&window, heading, message);
        }
    }

    pub(super) fn refresh(&self, message: Option<String>) {
        (self.refresh)(message, true);
    }

    pub(super) fn refresh_without_toast(&self, message: Option<String>) {
        (self.refresh)(message, false);
    }

    pub(super) fn show_toast(&self, message: &str) {
        (self.show_toast)(message);
    }

    pub(super) fn git_action_running(&self) -> bool {
        self.git_action_running.get()
    }

    pub(super) fn run_git_action(&self) {
        (self.run_git_action)();
    }

    pub(super) fn open_terminal(&self, working_dir: &WorkspacePath) -> Result<(), String> {
        (self.open_terminal)(working_dir)
    }

    pub(super) fn run_terminal_command(
        &self,
        command: &CommandSpec,
        title: &str,
    ) -> Result<(), String> {
        (self.run_terminal_command)(command, title)
    }

    pub(super) fn run_shell_command(
        &self,
        command: &ShellCommandSpec,
        title: &str,
    ) -> Result<(), String> {
        (self.run_shell_command)(command, title)
    }

    pub(super) fn notify_badge_changed(&self) {
        (self.notify_badge_changed)();
    }

    pub(super) fn dispatch_command(&self, command: PageCommand) {
        (self.dispatch_command)(command);
    }
}

pub(crate) fn build_pages(ctx: PageContext) -> Vec<PageRef> {
    vec![
        Rc::new(changes::ChangesPage::new(ctx.clone())),
        Rc::new(history::HistoryPage::new(ctx.clone())),
        Rc::new(file::FilePage::new(ctx.clone())),
        Rc::new(containers::ContainersPage::new(ctx.clone())),
        Rc::new(agent::AgentPage::new(ctx)),
    ]
}

pub(crate) fn warm_pages_in_background(pages: &[PageRef]) {
    for page in pages {
        let label = page.label();
        log::debug!("page background initialization requested label={label}");
        page.initialize(Box::new(move |_, _| {
            log::debug!("page background initialization completed label={label}");
        }));
    }
}

pub(crate) struct PageHost {
    left_slot: gtk::Box,
    right_slot: gtk::Box,
    left_hovered: Rc<Cell<bool>>,
    right_hovered: Rc<Cell<bool>>,
    active_index: Cell<Option<usize>>,
    show_generation: Rc<Cell<u64>>,
}

impl PageHost {
    pub(crate) fn new(left_slot: &gtk::Box, right_slot: &gtk::Box) -> Self {
        let left_hovered = Rc::new(Cell::new(false));
        let left_hover = gtk::EventControllerMotion::new();
        left_hover.connect_enter({
            let left_hovered = left_hovered.clone();

            move |_, _, _| left_hovered.set(true)
        });
        left_hover.connect_leave({
            let left_hovered = left_hovered.clone();

            move |_| left_hovered.set(false)
        });
        left_slot.add_controller(left_hover);

        let right_hovered = Rc::new(Cell::new(false));
        let right_hover = gtk::EventControllerMotion::new();
        right_hover.connect_enter({
            let right_hovered = right_hovered.clone();

            move |_, _, _| right_hovered.set(true)
        });
        right_hover.connect_leave({
            let right_hovered = right_hovered.clone();

            move |_| right_hovered.set(false)
        });
        right_slot.add_controller(right_hover);

        Self {
            left_slot: left_slot.clone(),
            right_slot: right_slot.clone(),
            left_hovered,
            right_hovered,
            active_index: Cell::new(None),
            show_generation: Rc::new(Cell::new(0)),
        }
    }

    pub(crate) fn left_hovered(&self) -> bool {
        self.left_hovered.get()
    }

    pub(crate) fn right_hovered(&self) -> bool {
        self.right_hovered.get()
    }

    pub(crate) fn show(&self, pages: &[PageRef], index: usize) {
        if self.active_index.get() == Some(index) {
            return;
        }

        let Some(page) = pages.get(index).cloned() else {
            return;
        };

        let generation = self.show_generation.get().wrapping_add(1).max(1);
        self.show_generation.set(generation);
        clear_slot(&self.left_slot);
        clear_slot(&self.right_slot);

        self.left_slot.append(&loading_screen(page.label()));
        self.right_slot.append(&loading_screen(page.label()));
        self.active_index.set(Some(index));

        let left_slot = self.left_slot.clone();
        let right_slot = self.right_slot.clone();
        let show_generation = self.show_generation.clone();
        let label = page.label();
        page.initialize(Box::new(move |left, right| {
            if show_generation.get() != generation {
                log::trace!(
                    "ignored stale page panes label={} generation={}",
                    label,
                    generation
                );
                return;
            }

            clear_slot(&left_slot);
            clear_slot(&right_slot);
            left_slot.append(&left);
            right_slot.append(&right);
            log::debug!("page panes displayed label={label}");
        }));
    }
}

fn clear_slot(slot: &gtk::Box) {
    while let Some(child) = slot.first_child() {
        slot.remove(&child);
    }
}

fn loading_screen(label: &str) -> gtk::Widget {
    let spinner = adw::Spinner::new();
    spinner.set_size_request(28, 28);
    let label = gtk::Label::builder()
        .label(format!("Loading {label}..."))
        .halign(gtk::Align::Center)
        .css_classes(["dim-label"])
        .build();
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .build();
    root.append(&spinner);
    root.append(&label);
    root.upcast()
}
