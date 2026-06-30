use crate::git::RepositorySnapshot;
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::system::capabilities::{
    docker::DockerAccess, files::FileAccess, git::GitAccess, open::OpenAccess, shell::ShellAccess,
};
use crate::system::{SystemPath, SystemProviderRegistry, SystemRef, WorkspacePath, WorkspaceRef};
use crate::terminal::CommandSpec;
use crate::ui::dialogs::show_error_dialog;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

mod agent;
mod changes;
mod code;
mod containers;
mod history;

pub(crate) trait Page {
    fn label(&self) -> &'static str;
    fn icon_name(&self) -> &'static str;
    fn left(&self) -> gtk::Widget;
    fn right(&self) -> gtk::Widget;

    fn activate(&self) {}
    fn refresh(&self, snapshot: &RepositorySnapshot);
    fn refresh_page(&self, _completion: PageRefreshComplete) -> PageRefreshRequest {
        PageRefreshRequest::RepositorySnapshot
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
pub(crate) type PageRefreshComplete = Rc<dyn Fn() + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageRefreshRequest {
    RepositorySnapshot,
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
        path: String,
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

    pub(super) fn files(&self) -> Option<Arc<dyn FileAccess>> {
        self.providers
            .files(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn git(&self) -> Option<Arc<dyn GitAccess>> {
        self.providers
            .git(&self.system_ref.borrow().id, &self.workspace_ref())
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

    pub(super) fn opener(&self) -> Option<Arc<dyn OpenAccess>> {
        self.providers
            .opener(&self.system_ref.borrow().id, &self.workspace_ref())
    }

    pub(super) fn opener_unavailable_message(&self) -> String {
        format!(
            "Opening paths is unavailable for workspace {}.",
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
        Rc::new(code::CodePage::new(ctx.clone())),
        Rc::new(containers::ContainersPage::new(ctx.clone())),
        Rc::new(agent::AgentPage::new(ctx)),
    ]
}

pub(crate) struct PageHost {
    left_slot: gtk::Box,
    right_slot: gtk::Box,
    left_hovered: Rc<Cell<bool>>,
    right_hovered: Rc<Cell<bool>>,
    active_index: Cell<Option<usize>>,
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

        let Some(page) = pages.get(index) else {
            return;
        };

        clear_slot(&self.left_slot);
        clear_slot(&self.right_slot);

        self.left_slot.append(&page.left());
        self.right_slot.append(&page.right());
        self.active_index.set(Some(index));
    }
}

fn clear_slot(slot: &gtk::Box) {
    while let Some(child) = slot.first_child() {
        slot.remove(&child);
    }
}
