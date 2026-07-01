use adw::prelude::*;
use gtk::glib::prelude::ToVariant;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use vte4::prelude::*;

use super::super::PageContext;
use super::agent_shell_integration::{self, AgentNotification, AgentShellIntegration};
use super::prompts::{PromptBar, PromptSelection};
use super::smart_summary;
use super::{
    AGENT_ICON_PIXEL_SIZE,
    provider::{self, AgentProvider, CommandSpec},
};
use crate::config;
use crate::ui::agent_history::{self, AgentSessionRow, RestoreState};
use crate::ui::agent_status::{AgentActiveState, AgentInactiveState, AgentSessionState};
use crate::ui::agent_usage::{AgentResourceUsage, ProcessSnapshot, ProcessUsageTracker};
use crate::ui::{AGENT_SESSION_NOTIFICATION_DETAILED_ACTION, agent_session_notification_id};
use crate::ui::{
    canvas_scroll,
    components::{context_menu, terminal as terminal_component},
};

#[cfg(test)]
use super::provider::agy::terminal_text_active_state as agy_terminal_text_active_state;

const DEFAULT_COLUMNS: i64 = 100;
const DEFAULT_ROWS: i64 = 34;
const TERM_NAME: &str = "xterm-256color";
const COLORTERM_NAME: &str = "truecolor";
const VTE_VERSION: &str = "8400";
const CTRL_BACKSPACE_SEQUENCE: &[u8] = b"\x17";
const NOTIFICATION_APP_NAME: &str = "Craic";
const NOTIFICATION_TIMEOUT_MS: &str = "5000";
const WAITING_AGENT_SESSION_ICON: &str = "hand-touch-symbolic";
const SMART_SUMMARY_TRIGGER_ROWS: i64 = 500;
const CODEX_MAPPING_RETRY_DELAYS_MS: &[u64] = &[1_800, 8_000, 30_000, 90_000];
#[derive(Clone)]
struct AgentSession {
    id: u64,
    session_uuid: String,
    provider: &'static dyn AgentProvider,
    root: gtk::Overlay,
    terminal: vte4::Terminal,
    child_pid: Rc<Cell<Option<glib::Pid>>>,
    state: Rc<Cell<TerminalSessionState>>,
    active_state: Rc<Cell<AgentActiveState>>,
    icon_stack: gtk::Stack,
    label: gtk::Label,
    title_locked: Rc<Cell<bool>>,
    local_history_id: Rc<Cell<Option<i64>>>,
    loading_poll_count: Rc<Cell<u8>>,
    summary_requested: Rc<Cell<bool>>,
    summary_in_flight: Rc<Cell<bool>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalSessionState {
    Starting,
    Running,
    Exited,
    Closing,
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct LoadedHistorySessionStatus {
    pub(in crate::ui) session_id: u64,
    pub(in crate::ui) terminal_state: &'static str,
    pub(in crate::ui) active_state: Option<AgentActiveState>,
}

#[derive(Clone, Debug)]
pub(in crate::ui) struct ActiveSessionStatus {
    pub(in crate::ui) session_id: u64,
    pub(in crate::ui) session_uuid: String,
    pub(in crate::ui) local_history_id: Option<i64>,
    pub(in crate::ui) provider_id: &'static str,
    pub(in crate::ui) title: String,
    pub(in crate::ui) terminal_state: &'static str,
    pub(in crate::ui) active_state: Option<AgentActiveState>,
}

pub(in crate::ui) struct AgentChat {
    pub(in crate::ui) root: gtk::Box,
    ctx: PageContext,
    prompt_bar: PromptBar,
    notebook: gtk::Notebook,
    sessions: Rc<RefCell<Vec<AgentSession>>>,
    next_session_id: Rc<Cell<u64>>,
    working_directory: Rc<RefCell<PathBuf>>,
    workspace_history: Rc<RefCell<agent_history::WorkspaceKey>>,
    new_session_callback: Rc<
        RefCell<
            Option<
                Rc<dyn Fn(u64, &'static dyn AgentProvider, String, Option<i64>, AgentSessionState)>,
            >,
        >,
    >,
    title_callback: Rc<RefCell<Option<Rc<dyn Fn(u64, String)>>>>,
    state_callback:
        Rc<RefCell<Option<Rc<dyn Fn(u64, &'static dyn AgentProvider, AgentSessionState)>>>>,
    resource_usage_callback: Rc<RefCell<Option<Rc<dyn Fn(u64, Option<AgentResourceUsage>)>>>>,
    close_callback: Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    usage_tracker: Rc<RefCell<ProcessUsageTracker>>,
}

impl AgentChat {
    pub(in crate::ui) fn new(ctx: PageContext) -> Self {
        let prompt_bar = PromptBar::new();
        let local_workspace_path = ctx.local_workspace_path();
        prompt_bar.set_local_repo_path(local_workspace_path.as_deref());
        let workspace = ctx.workspace_ref();
        let initial_workspace_path = PathBuf::from(&workspace.root.absolute);
        let initial_workspace_history =
            agent_history::workspace_for_system_path(ctx.workspace_key(), workspace.root.absolute);

        let notebook = gtk::Notebook::builder()
            .show_tabs(false)
            .show_border(false)
            .hexpand(true)
            .vexpand(true)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&prompt_bar.root);
        root.append(&notebook);

        let chat = Self {
            root,
            ctx,
            prompt_bar,
            notebook,
            sessions: Rc::new(RefCell::new(Vec::new())),
            next_session_id: Rc::new(Cell::new(1)),
            working_directory: Rc::new(RefCell::new(initial_workspace_path)),
            workspace_history: Rc::new(RefCell::new(initial_workspace_history)),
            new_session_callback: Rc::new(RefCell::new(None)),
            title_callback: Rc::new(RefCell::new(None)),
            state_callback: Rc::new(RefCell::new(None)),
            resource_usage_callback: Rc::new(RefCell::new(None)),
            close_callback: Rc::new(RefCell::new(None)),
            history_callback: Rc::new(RefCell::new(None)),
            usage_tracker: Rc::new(RefCell::new(ProcessUsageTracker::new())),
        };

        chat.connect_controls();
        chat.start_status_polling();

        chat
    }

    pub fn set_workspace_from_context(&self) {
        let workspace = self.ctx.workspace_ref();
        self.working_directory
            .replace(PathBuf::from(&workspace.root.absolute));
        self.workspace_history
            .replace(agent_history::workspace_for_system_path(
                self.ctx.workspace_key(),
                workspace.root.absolute,
            ));
        let local_workspace_path = self.ctx.local_workspace_path();
        self.prompt_bar
            .set_local_repo_path(local_workspace_path.as_deref());
    }

    pub fn show(&self) {
        if self.sessions.borrow().is_empty() {
            self.start_chat(provider::default_provider());
        } else {
            self.focus_active_terminal();
        }
    }

    pub fn start_chat(&self, provider: &'static dyn AgentProvider) {
        let session_id = self.reserve_session_id();
        let title = provider.default_title();
        let session_uuid = agent_history::new_session_uuid();
        self.start_chat_with_id(session_id, &session_uuid, provider, &title);
    }

    fn start_chat_with_id(
        &self,
        session_id: u64,
        session_uuid: &str,
        provider: &'static dyn AgentProvider,
        title: &str,
    ) {
        if session_id >= self.next_session_id.get() {
            self.next_session_id.set(session_id + 1);
        }
        let system = self.ctx.system_ref();
        let workspace = self.ctx.workspace_ref();
        let command = provider.command(&system, &workspace);
        provider.shell_integration().log_session_create(
            session_id,
            provider,
            title,
            command.target_working_dir(),
            &command.display(),
        );
        let _ = self.create_session(
            session_id,
            session_uuid,
            provider,
            title,
            &command,
            None,
            AgentActiveState::NewChat,
        );
    }

    pub(in crate::ui) fn restore_session(&self, row: &AgentSessionRow) -> Result<u64, String> {
        if !row.restore_state.is_restorable() {
            log::info!(
                "agent history restore ignored local_id={} provider={} restore_state={}",
                row.id,
                row.provider_id,
                row.restore_state.as_str()
            );
            return Err("Agent session is not restorable.".to_string());
        }
        let cli_session_id = row.cli_session_id.as_deref().ok_or_else(|| {
            format!(
                "Agent session local_id={} is marked restorable without a CLI session id.",
                row.id
            )
        })?;
        let provider = provider::all_providers()
            .iter()
            .copied()
            .find(|provider| provider.provider_id() == row.provider_id)
            .ok_or_else(|| format!("Unknown agent provider {}.", row.provider_id))?;
        let command = provider.restore_command(
            &self.ctx.system_ref(),
            &self.ctx.workspace_ref(),
            cli_session_id,
        )?;
        let session_id = history_session_id(row.id)?;
        if let Some(active_session) = session_by_id(&self.sessions, session_id) {
            if active_session.local_history_id.get() == Some(row.id) {
                self.show_session(session_id);
                log::info!(
                    "agent history restore focused already-active session local_id={} session_id={}",
                    row.id,
                    session_id
                );
                return Ok(session_id);
            }

            log::warn!(
                "agent history restore blocked by active session id collision local_id={} session_id={}",
                row.id,
                session_id
            );
            return Err(format!(
                "Craic session id {session_id} is already active for another agent session."
            ));
        }
        self.reserve_session_id_at_least(session_id);
        provider.shell_integration().log_session_create(
            session_id,
            provider,
            &row.title,
            &row.repo_path.display().to_string(),
            &command.display(),
        );
        let _ = self.create_session(
            session_id,
            &row.session_uuid,
            provider,
            &row.title,
            &command,
            Some(row.id),
            AgentActiveState::Loading,
        )?;
        notify_history_changed(&self.history_callback);
        log::info!(
            "agent history restore started local_id={} session_id={} provider={} cli_session_id={}",
            row.id,
            session_id,
            provider.provider_id(),
            cli_session_id
        );
        Ok(session_id)
    }

    pub fn show_session(&self, session_id: u64) -> bool {
        if let Some(session) = session_by_id(&self.sessions, session_id) {
            if let Some(page_num) = self.notebook.page_num(&session.root) {
                self.notebook.set_current_page(Some(page_num));
            }
            session.terminal.grab_focus();
            true
        } else {
            false
        }
    }

    pub fn add_file_reference(&self, file_path: &str) {
        self.show();
        let Some(session) = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)))
            .and_then(|page| session_by_page(&self.sessions, &page))
        else {
            return;
        };

        session
            .terminal
            .feed_child(format!("@{file_path} ").as_bytes());
        session.terminal.grab_focus();
    }

    pub(in crate::ui) fn connect_prompt_bar(self: &Rc<Self>) {
        self.prompt_bar.connect_prompt_selected({
            let chat = self.clone();

            move |selection| {
                chat.send_prompt_selection(selection);
            }
        });
    }

    pub fn connect_title_changed<F>(&self, callback: F)
    where
        F: Fn(u64, String) + 'static,
    {
        self.title_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_state_changed<F>(&self, callback: F)
    where
        F: Fn(u64, &'static dyn AgentProvider, AgentSessionState) + 'static,
    {
        self.state_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_resource_usage_changed<F>(&self, callback: F)
    where
        F: Fn(u64, Option<AgentResourceUsage>) + 'static,
    {
        self.resource_usage_callback
            .replace(Some(Rc::new(callback)));
    }

    pub fn connect_close_requested<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.close_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_history_changed<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.history_callback.replace(Some(Rc::new(callback)));
    }

    pub fn connect_new_session<F>(&self, callback: F)
    where
        F: Fn(u64, &'static dyn AgentProvider, String, Option<i64>, AgentSessionState) + 'static,
    {
        self.new_session_callback.replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn running_session_count(&self) -> usize {
        self.sessions
            .borrow()
            .iter()
            .filter(|session| match session.state.get() {
                TerminalSessionState::Starting => {
                    let state = session.active_state.get();
                    let counts = active_state_counts_as_running(state);
                    if is_selected_session(session, &self.notebook) {
                        log::debug!(
                            "agent running count starting session_id={} provider={} active_state={:?} counts={}",
                            session.id,
                            session.provider.provider_id(),
                            state,
                            counts
                        );
                    }
                    counts
                }
                TerminalSessionState::Running => {
                    let log_scan = is_selected_session(session, &self.notebook);
                    let state = agent_shell_integration::active_state(
                        session.id,
                        session.provider,
                        &session.terminal,
                        log_scan,
                    );
                    session.active_state.set(state);
                    active_state_counts_as_running(state)
                }
                TerminalSessionState::Exited | TerminalSessionState::Closing => false,
            })
            .count()
    }

    pub fn request_close_session(&self, session_id: u64) {
        request_close_session(
            session_id,
            &self.root,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn request_unload_history_session(&self, local_id: i64) {
        request_unload_history_session(
            local_id,
            &self.root,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn close_history_session(&self, local_id: i64) {
        let Some(session) = session_by_local_history_id(&self.sessions, local_id) else {
            return;
        };
        close_session(
            session.id,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );
    }

    pub fn history_session_is_loaded(&self, local_id: i64) -> bool {
        session_by_local_history_id(&self.sessions, local_id).is_some()
    }

    pub fn loaded_history_session_status(
        &self,
        local_id: i64,
    ) -> Option<LoadedHistorySessionStatus> {
        let session = session_by_local_history_id(&self.sessions, local_id)?;
        Some(loaded_history_session_status(&session))
    }

    pub fn active_session_status(&self, session_id: u64) -> Option<ActiveSessionStatus> {
        let session = session_by_id(&self.sessions, session_id)?;
        Some(active_session_status(&session))
    }

    pub fn set_active_session_cli_id(
        &self,
        session_id: u64,
        cli_session_id: &str,
    ) -> Result<i64, String> {
        let session = session_by_id(&self.sessions, session_id)
            .ok_or_else(|| format!("Agent session {session_id} is not active."))?;
        let local_id = ensure_agent_history_session(
            &session,
            &self.workspace_history,
            &self.history_callback,
        )?;
        agent_history::set_manual_session_id(local_id, cli_session_id)?;
        notify_history_changed(&self.history_callback);
        Ok(local_id)
    }

    pub fn generate_active_session_summary(&self, session_id: u64) -> Result<(), String> {
        let session = session_by_id(&self.sessions, session_id)
            .ok_or_else(|| format!("Agent session {session_id} is not active."))?;
        start_smart_summary(
            &session,
            &self.workspace_history,
            &self.history_callback,
            SmartSummaryMode::Manual,
        )
    }

    pub fn generate_history_session_summary(&self, local_id: i64) -> Result<(), String> {
        let session = session_by_local_history_id(&self.sessions, local_id)
            .ok_or_else(|| "Load the session before generating a summary.".to_string())?;
        start_smart_summary(
            &session,
            &self.workspace_history,
            &self.history_callback,
            SmartSummaryMode::Manual,
        )
    }

    fn reserve_session_id(&self) -> u64 {
        self.sync_next_session_id_with_history();
        let mut session_id = self.next_session_id.get().max(1);
        while session_by_id(&self.sessions, session_id).is_some() {
            session_id = session_id.saturating_add(1);
        }
        self.next_session_id.set(session_id.saturating_add(1));
        session_id
    }

    fn reserve_session_id_at_least(&self, session_id: u64) {
        if session_id >= self.next_session_id.get() {
            self.next_session_id.set(session_id.saturating_add(1));
        }
    }

    fn sync_next_session_id_with_history(&self) {
        match agent_history::max_local_session_id() {
            Ok(Some(max_id)) => match history_session_id(max_id) {
                Ok(max_session_id) => self.reserve_session_id_at_least(max_session_id),
                Err(err) => {
                    log::warn!("agent history max session id ignored: {err}");
                }
            },
            Ok(None) => {}
            Err(err) => {
                log::warn!("agent history max session id lookup failed: {err}");
            }
        }
    }

    fn focus_active_terminal(&self) {
        if let Some(session) = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)))
            .and_then(|page| session_by_page(&self.sessions, &page))
        {
            session.terminal.grab_focus();
        }
    }

    fn send_prompt_selection(&self, selection: Result<PromptSelection, String>) {
        match selection {
            Ok(selection) => self.send_prompt_to_active_terminal(&selection.content),
            Err(err) => self.ctx.show_error("Prompt Failed", &err),
        }
    }

    fn send_prompt_to_active_terminal(&self, content: &str) {
        self.show();
        let Some(session) = self
            .notebook
            .current_page()
            .and_then(|page_num| self.notebook.nth_page(Some(page_num)))
            .and_then(|page| session_by_page(&self.sessions, &page))
        else {
            return;
        };

        session.terminal.paste_text(content);
        session.terminal.grab_focus();
    }

    fn connect_controls(&self) {
        self.notebook.connect_switch_page({
            let sessions = self.sessions.clone();

            move |_, page, _| {
                if let Some(session) = session_by_page(&sessions, page) {
                    session.terminal.grab_focus();
                }
            }
        });
    }

    fn create_session(
        &self,
        session_id: u64,
        session_uuid: &str,
        provider: &'static dyn AgentProvider,
        title: &str,
        command: &CommandSpec,
        local_history_id: Option<i64>,
        initial_active_state: AgentActiveState,
    ) -> Result<AgentSession, String> {
        let terminal = configured_terminal(config::load().font_sizes.shell, &self.sessions);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .kinetic_scrolling(false)
            .hexpand(true)
            .vexpand(true)
            .child(&terminal)
            .build();
        let autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&scroller));
        root.add_overlay(&autoscroll_marker);
        let autoscroll = Rc::new(canvas_scroll::MiddleAutoscroll::new());
        canvas_scroll::install_scrolled_window_middle_autoscroll_with_state(
            &scroller,
            &autoscroll_marker,
            &autoscroll,
            canvas_scroll::AutoscrollAxes::Vertical,
            "agent_terminal",
            {
                let scroller = scroller.clone();
                let terminal = terminal.clone();
                move |cursor| {
                    scroller.set_cursor_from_name(cursor);
                    terminal.set_cursor_from_name(cursor);
                }
            },
        );

        let session_name = session_id.to_string();
        root.set_widget_name(&session_name);

        let display_title = if initial_active_state == AgentActiveState::NewChat {
            provider.default_title()
        } else {
            title.to_string()
        };
        let label = gtk::Label::builder()
            .label(&display_title)
            .ellipsize(pango::EllipsizeMode::End)
            .width_chars(12)
            .max_width_chars(18)
            .xalign(0.0)
            .build();

        let close_button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close session")
            .valign(gtk::Align::Center)
            .build();
        close_button.add_css_class("flat");
        close_button.add_css_class("circular");

        let icon = gtk::Image::from_icon_name(provider.session_icon_name());
        let waiting_icon = gtk::Image::from_icon_name(WAITING_AGENT_SESSION_ICON);
        icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        waiting_icon.set_pixel_size(AGENT_ICON_PIXEL_SIZE);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(AGENT_ICON_PIXEL_SIZE, AGENT_ICON_PIXEL_SIZE);
        spinner.set_valign(gtk::Align::Center);

        let icon_stack = gtk::Stack::builder().build();
        icon_stack.add_named(&icon, Some("icon"));
        icon_stack.add_named(&waiting_icon, Some("waiting"));
        icon_stack.add_named(&spinner, Some("spinner"));
        icon_stack.set_visible_child_name("icon");

        let tab_label = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(6)
            .margin_end(6)
            .build();
        tab_label.append(&icon_stack);
        tab_label.append(&label);
        tab_label.append(&close_button);

        let page_num = self.notebook.append_page(&root, Some(&tab_label));
        self.notebook.set_tab_reorderable(&root, true);
        self.notebook.set_current_page(Some(page_num));

        close_button.connect_clicked({
            let sessions = self.sessions.clone();
            let notebook = self.notebook.clone();
            let root = self.root.clone();
            let close_callback = self.close_callback.clone();
            let history_callback = self.history_callback.clone();

            move |_| {
                request_close_session(
                    session_id,
                    &root,
                    &sessions,
                    &notebook,
                    &close_callback,
                    &history_callback,
                );
            }
        });

        let child_pid = Rc::new(Cell::new(None));
        let state = Rc::new(Cell::new(TerminalSessionState::Starting));
        let active_state = Rc::new(Cell::new(initial_active_state));
        let loading_poll_count = Rc::new(Cell::new(0));
        let summary_requested = Rc::new(Cell::new(false));
        let summary_in_flight = Rc::new(Cell::new(false));

        install_exit_key_handler(
            session_id,
            &terminal,
            &state,
            &self.sessions,
            &self.notebook,
            &self.close_callback,
            &self.history_callback,
        );

        connect_child_exit(
            session_id,
            provider,
            &terminal,
            &label,
            &display_title,
            &child_pid,
            &state,
            provider.shell_integration(),
            &self.state_callback,
        );
        let title_locked = Rc::new(Cell::new(!provider::is_default_agent_title(title)));
        let local_history_id = Rc::new(Cell::new(local_history_id));
        connect_title_updates(
            session_id,
            session_uuid,
            provider,
            &terminal,
            &label,
            &state,
            &title_locked,
            &active_state,
            &local_history_id,
            &self.notebook,
            &self.workspace_history,
            &self.state_callback,
            &self.title_callback,
            &self.history_callback,
        );
        spawn_command(
            &terminal,
            command,
            &child_pid,
            &state,
            provider.shell_integration(),
            session_id,
            provider,
            &self.state_callback,
        )?;

        let session = AgentSession {
            id: session_id,
            session_uuid: session_uuid.to_string(),
            provider,
            root,
            terminal,
            child_pid,
            state,
            active_state,
            icon_stack,
            label,
            title_locked,
            local_history_id,
            loading_poll_count,
            summary_requested,
            summary_in_flight,
        };

        self.sessions.borrow_mut().push(session.clone());
        session.terminal.grab_focus();

        if let Some(ref cb) = *self.new_session_callback.borrow() {
            cb(
                session_id,
                provider,
                display_title.clone(),
                session.local_history_id.get(),
                AgentSessionState::Active(initial_active_state),
            );
        }
        notify_session_state_changed(
            &self.state_callback,
            session_id,
            provider,
            AgentSessionState::Active(initial_active_state),
        );

        Ok(session)
    }

    fn start_status_polling(&self) {
        let sessions = self.sessions.clone();
        let state_callback = self.state_callback.clone();
        let resource_usage_callback = self.resource_usage_callback.clone();
        let usage_tracker = self.usage_tracker.clone();
        let title_callback = self.title_callback.clone();
        let history_callback = self.history_callback.clone();
        let workspace_history = self.workspace_history.clone();
        let notebook = self.notebook.clone();
        let ctx = self.ctx.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
            let borrowed_sessions = sessions.borrow();
            let session_ids = borrowed_sessions
                .iter()
                .map(|session| session.id)
                .collect::<Vec<_>>();
            let process_snapshot = borrowed_sessions
                .iter()
                .any(|session| session.state.get() == TerminalSessionState::Running)
                .then(ProcessSnapshot::read)
                .flatten();

            for session in borrowed_sessions.iter() {
                let terminal_state = session.state.get();
                let previous_session_state = match terminal_state {
                    TerminalSessionState::Starting => {
                        AgentSessionState::Active(session.active_state.get())
                    }
                    TerminalSessionState::Running => {
                        AgentSessionState::Active(session.active_state.get())
                    }
                    TerminalSessionState::Exited | TerminalSessionState::Closing => {
                        AgentSessionState::Inactive(AgentInactiveState::Dead)
                    }
                };
                let process_running = terminal_state == TerminalSessionState::Running;
                let session_state = session_state_for_poll(session, terminal_state);
                let active_state = match session_state {
                    AgentSessionState::Active(state) => Some(state),
                    AgentSessionState::Inactive(_) => None,
                };
                if let Some(active_state) = active_state {
                    session.active_state.set(active_state);
                }
                let is_loading = active_state == Some(AgentActiveState::Loading);
                let loading_poll_count = if is_loading {
                    let next = session.loading_poll_count.get().saturating_add(1);
                    session.loading_poll_count.set(next);
                    next
                } else {
                    session.loading_poll_count.replace(0)
                };
                match active_state {
                    Some(AgentActiveState::Loading) => {
                        session.icon_stack.set_visible_child_name("spinner");
                    }
                    Some(AgentActiveState::Asking) => {
                        session.icon_stack.set_visible_child_name("waiting");
                    }
                    Some(AgentActiveState::NewChat | AgentActiveState::Idle) | None => {
                        session.icon_stack.set_visible_child_name("icon");
                    }
                }
                if process_running
                    && loading_poll_count > 1
                    && matches!(
                        active_state,
                        Some(AgentActiveState::Idle | AgentActiveState::Asking)
                    )
                {
                    notify_agent_turn_complete(
                        &ctx,
                        session.id,
                        session.provider,
                        active_state.expect("active state checked above"),
                        session.label.text().as_str(),
                    );
                }
                if session_state != previous_session_state {
                    retry_empty_codex_mapping_on_status_change(
                        session,
                        previous_session_state,
                        session_state,
                        &history_callback,
                    );
                    if let Some(ref cb) = *state_callback.borrow() {
                        cb(session.id, session.provider, session_state);
                    }
                }
                update_resource_usage(
                    session,
                    process_running,
                    process_snapshot.as_ref(),
                    &usage_tracker,
                    &resource_usage_callback,
                );
                maybe_start_smart_summary(session, &workspace_history, &history_callback);

                if session.title_locked.get() {
                    continue;
                }

                let log_scan = is_selected_session(session, &notebook);
                if let Some(title) = agent_shell_integration::session_title(
                    session.provider,
                    &session.terminal,
                    log_scan,
                ) {
                    if log_scan {
                        log::debug!(
                            "agent title parsed session_id={} provider={} title={}",
                            session.id,
                            session.provider.label(),
                            agent_shell_integration::log_preview(
                                &title,
                                agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
                            )
                        );
                    }
                    if session.label.text().as_str() == title.as_str() {
                        session.title_locked.set(true);
                        continue;
                    }

                    session.label.set_label(&title);
                    session.title_locked.set(true);
                    if session.active_state.get() == AgentActiveState::NewChat {
                        let next_active_state = match terminal_state {
                            TerminalSessionState::Running => agent_shell_integration::active_state(
                                session.id,
                                session.provider,
                                &session.terminal,
                                log_scan,
                            ),
                            TerminalSessionState::Starting => AgentActiveState::NewChat,
                            TerminalSessionState::Exited | TerminalSessionState::Closing => {
                                AgentActiveState::Idle
                            }
                        };
                        if log_scan {
                            log::debug!(
                                "agent title update active state session_id={} provider={} terminal_state={:?} next_active_state={:?}",
                                session.id,
                                session.provider.provider_id(),
                                terminal_state,
                                next_active_state
                            );
                        }
                        session.active_state.set(next_active_state);
                        notify_session_state_changed(
                            &state_callback,
                            session.id,
                            session.provider,
                            AgentSessionState::Active(next_active_state),
                        );
                    }
                    persist_agent_session_title(
                        session.id,
                        session.provider,
                        &title,
                        &workspace_history,
                        &session.local_history_id,
                        &session.session_uuid,
                        &history_callback,
                    );
                    if let Some(ref cb) = *title_callback.borrow() {
                        cb(session.id, title);
                    }
                }
            }
            usage_tracker.borrow_mut().retain_sessions(&session_ids);

            glib::ControlFlow::Continue
        });
    }
}

fn update_resource_usage(
    session: &AgentSession,
    process_running: bool,
    process_snapshot: Option<&ProcessSnapshot>,
    usage_tracker: &Rc<RefCell<ProcessUsageTracker>>,
    resource_usage_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64, Option<AgentResourceUsage>)>>>>,
) {
    let usage = if process_running {
        match (session.child_pid.get(), process_snapshot) {
            (Some(pid), Some(snapshot)) => {
                usage_tracker
                    .borrow_mut()
                    .sample(session.id, pid.0 as libc::pid_t, snapshot)
            }
            _ => {
                usage_tracker.borrow_mut().clear(session.id);
                None
            }
        }
    } else {
        usage_tracker.borrow_mut().clear(session.id);
        None
    };

    if let Some(ref cb) = *resource_usage_callback.borrow() {
        cb(session.id, usage);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SmartSummaryMode {
    Automatic,
    Manual,
}

fn maybe_start_smart_summary(
    session: &AgentSession,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    if session.state.get() != TerminalSessionState::Running {
        return;
    }

    let (_cursor_column, cursor_row) = session.terminal.cursor_position();
    if cursor_row < SMART_SUMMARY_TRIGGER_ROWS {
        return;
    }

    if let Err(err) = start_smart_summary(
        session,
        workspace_history,
        history_callback,
        SmartSummaryMode::Automatic,
    ) {
        log::debug!(
            "agent smart summary automatic start skipped session_id={} provider={} error={}",
            session.id,
            session.provider.provider_id(),
            err
        );
    }
}

fn start_smart_summary(
    session: &AgentSession,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
    mode: SmartSummaryMode,
) -> Result<(), String> {
    if session.summary_in_flight.get() {
        return Err("A smart summary is already running for this session.".to_string());
    }
    if mode == SmartSummaryMode::Automatic && session.summary_requested.get() {
        return Err("A smart summary was already requested for this session.".to_string());
    }
    if session.state.get() != TerminalSessionState::Running {
        return Err("The session terminal is not running.".to_string());
    }

    let local_id = ensure_agent_history_session(session, workspace_history, history_callback)
        .map_err(|err| {
            if mode == SmartSummaryMode::Automatic {
                session.summary_requested.set(true);
            }
            err
        })?;

    let existing_tags = match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => {
            if mode == SmartSummaryMode::Automatic && row.task_description.is_some() {
                session.summary_requested.set(true);
                return Err("The session already has a smart summary.".to_string());
            }
            match agent_history::workspace_tags(&row.workspace_key) {
                Ok(tags) => tags,
                Err(err) => {
                    log::warn!(
                        "agent smart summary existing tags load failed local_id={} workspace_key={} error={}",
                        local_id,
                        row.workspace_key,
                        err
                    );
                    Vec::new()
                }
            }
        }
        Ok(None) => Vec::new(),
        Err(err) => {
            if mode == SmartSummaryMode::Automatic {
                return Err(format!("Smart summary history lookup failed: {err}"));
            }
            log::warn!("agent smart summary history lookup failed local_id={local_id}: {err}");
            Vec::new()
        }
    };

    let terminal_text = terminal_full_text(&session.terminal)
        .ok_or_else(|| "The session terminal has no transcript to summarize.".to_string())?;
    let (_cursor_column, cursor_row) = session.terminal.cursor_position();
    let title = session.label.text().to_string();
    let shell_provider_id = session.provider.provider_id().to_string();
    session.summary_in_flight.set(true);
    session.summary_requested.set(true);
    log::info!(
        "agent smart summary queued session_id={} local_id={} provider={} mode={:?} terminal_bytes={} cursor_row={}",
        session.id,
        local_id,
        shell_provider_id,
        mode,
        terminal_text.len(),
        cursor_row
    );

    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result =
            smart_summary::generate(&shell_provider_id, &title, &terminal_text, &existing_tags)
                .and_then(|summary| {
                    agent_history::update_session_summary(local_id, &summary)?;
                    Ok(summary)
                });
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), {
        let summary_in_flight = session.summary_in_flight.clone();
        let history_callback = history_callback.clone();

        move || match receiver.try_recv() {
            Ok(Ok(summary)) => {
                summary_in_flight.set(false);
                log::info!(
                    "agent smart summary complete local_id={} description_bytes={} tags={}",
                    local_id,
                    summary.task_description.len(),
                    summary.tags.len()
                );
                notify_history_changed(&history_callback);
                gtk::glib::ControlFlow::Break
            }
            Ok(Err(err)) => {
                summary_in_flight.set(false);
                log::warn!("agent smart summary failed local_id={local_id}: {err}");
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                summary_in_flight.set(false);
                log::warn!("agent smart summary worker disconnected local_id={local_id}");
                gtk::glib::ControlFlow::Break
            }
        }
    });

    Ok(())
}

fn terminal_full_text(terminal: &vte4::Terminal) -> Option<String> {
    let (_cursor_column, cursor_row) = terminal.cursor_position();
    let visible_end_row = terminal.row_count().saturating_sub(1);
    let end_row = visible_end_row.max(cursor_row.saturating_sub(1));
    if end_row <= 0 {
        return None;
    }
    let end_col = terminal.column_count().max(1);
    let (text, _) = terminal.text_range_format(vte4::Format::Text, 0, 0, end_row, end_col);
    let text = text?
        .trim_start_matches(|ch| matches!(ch, '\n' | '\r'))
        .to_string();
    (!text.trim().is_empty()).then_some(text)
}

fn request_close_session(
    session_id: u64,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let Some(session) = session_by_id(sessions, session_id) else {
        return;
    };

    let Some(state) = close_confirmation_state(&session) else {
        close_session(
            session_id,
            sessions,
            notebook,
            close_callback,
            history_callback,
        );
        return;
    };

    confirm_close_active_agent_session(
        &session,
        state,
        root,
        sessions,
        notebook,
        close_callback,
        history_callback,
    );
}

fn request_unload_history_session(
    local_id: i64,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let Some(session) = session_by_local_history_id(sessions, local_id) else {
        log::debug!("agent unload ignored missing active local_id={local_id}");
        return;
    };

    let mapping = if session.provider.provider_id() == "codex" {
        agent_history::map_codex_session(local_id)
    } else {
        Ok(agent_history::CodexMappingOutcome::Unsupported)
    };

    match mapping {
        Ok(agent_history::CodexMappingOutcome::Restorable(cli_session_id)) => {
            log::info!(
                "agent unload mapped session local_id={} session_id={} cli_session_id={}",
                local_id,
                session.id,
                cli_session_id
            );
            close_session(
                session.id,
                sessions,
                notebook,
                close_callback,
                history_callback,
            );
        }
        Ok(outcome) => {
            confirm_unload_without_restorable_id(
                &session,
                &format!("Craic could not find a restorable CLI session ID ({outcome:?})."),
                root,
                sessions,
                notebook,
                close_callback,
                history_callback,
            );
        }
        Err(err) => {
            confirm_unload_without_restorable_id(
                &session,
                &format!("Craic could not find a restorable CLI session ID: {err}"),
                root,
                sessions,
                notebook,
                close_callback,
                history_callback,
            );
        }
    }
}

fn close_confirmation_state(session: &AgentSession) -> Option<AgentActiveState> {
    match session.state.get() {
        TerminalSessionState::Starting => Some(session.active_state.get()),
        TerminalSessionState::Running => {
            let state = if session.active_state.get() == AgentActiveState::NewChat {
                AgentActiveState::NewChat
            } else {
                agent_shell_integration::active_state(
                    session.id,
                    session.provider,
                    &session.terminal,
                    false,
                )
            };
            session.active_state.set(state);
            match state {
                AgentActiveState::Loading | AgentActiveState::Asking => Some(state),
                AgentActiveState::NewChat | AgentActiveState::Idle => None,
            }
        }
        TerminalSessionState::Exited | TerminalSessionState::Closing => None,
    }
}

fn confirm_close_active_agent_session(
    session: &AgentSession,
    state: AgentActiveState,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let body = match state {
        AgentActiveState::Asking => format!(
            "{} is asking a question. Closing this agent tab will terminate it.",
            session.provider.label()
        ),
        AgentActiveState::NewChat => format!(
            "{} has a new chat open. Closing this agent tab will terminate it.",
            session.provider.label()
        ),
        AgentActiveState::Loading => format!(
            "{} is still loading or working. Closing this agent tab will terminate it.",
            session.provider.label()
        ),
        AgentActiveState::Idle => format!(
            "{} is still open. Closing this agent tab will terminate it.",
            session.provider.label()
        ),
    };
    log::info!(
        "agent session close confirmation shown session_id={} provider={} state={:?}",
        session.id,
        session.provider.label(),
        state
    );

    let dialog = adw::AlertDialog::builder()
        .heading("Close Agent Tab?")
        .body(&body)
        .build();
    dialog.add_response("cancel", "Keep Open");
    dialog.add_response("close", "Close Tab");
    dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = root
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, {
        let session_id = session.id;
        let provider_label = session.provider.label();
        let sessions = sessions.clone();
        let notebook = notebook.clone();
        let close_callback = close_callback.clone();
        let history_callback = history_callback.clone();

        move |response| {
            if response.as_str() != "close" {
                log::info!(
                    "agent session close cancelled session_id={} provider={}",
                    session_id,
                    provider_label
                );
                return;
            }

            log::info!(
                "agent session close confirmed session_id={} provider={}",
                session_id,
                provider_label
            );
            close_session(
                session_id,
                &sessions,
                &notebook,
                &close_callback,
                &history_callback,
            );
        }
    });
}

fn confirm_unload_without_restorable_id(
    session: &AgentSession,
    reason: &str,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let body = format!(
        "{reason}\n\nUnloading this session will close the agent tab, but it may not be restorable until a session ID is set manually."
    );
    log::warn!(
        "agent unload confirmation shown without restorable id session_id={} local_id={:?} provider={} reason={}",
        session.id,
        session.local_history_id.get(),
        session.provider.provider_id(),
        reason
    );

    let dialog = adw::AlertDialog::builder()
        .heading("Unload Without Session ID?")
        .body(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("unload", "Unload Session");
    dialog.set_response_appearance("unload", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = root
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());
    dialog.choose(parent.as_ref(), None::<&gio::Cancellable>, {
        let session_id = session.id;
        let sessions = sessions.clone();
        let notebook = notebook.clone();
        let close_callback = close_callback.clone();
        let history_callback = history_callback.clone();

        move |response| {
            if response.as_str() != "unload" {
                log::info!("agent unload cancelled session_id={session_id}");
                return;
            }
            log::info!("agent unload confirmed session_id={session_id}");
            close_session(
                session_id,
                &sessions,
                &notebook,
                &close_callback,
                &history_callback,
            );
        }
    });
}

fn close_session(
    session_id: u64,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let session = {
        let mut sessions = sessions.borrow_mut();
        let Some(index) = sessions.iter().position(|session| session.id == session_id) else {
            return;
        };
        sessions.remove(index)
    };

    session.state.set(TerminalSessionState::Closing);
    let child_pid = session.child_pid.get();
    session.child_pid.set(None);
    terminate_child(child_pid);
    if let Some(page_num) = notebook.page_num(&session.root) {
        notebook.remove_page(Some(page_num));
    }

    if let Some(next_session) = notebook
        .current_page()
        .and_then(|page_num| notebook.nth_page(Some(page_num)))
        .and_then(|page| session_by_page(sessions, &page))
        .or_else(|| sessions.borrow().last().cloned())
    {
        if let Some(page_num) = notebook.page_num(&next_session.root) {
            notebook.set_current_page(Some(page_num));
        }
        next_session.terminal.grab_focus();
    }

    if let Some(ref cb) = *close_callback.borrow() {
        cb(session_id);
    }

    mark_agent_history_ended(&session, history_callback);
}

fn session_by_id(
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    session_id: u64,
) -> Option<AgentSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
}

fn session_by_local_history_id(
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    local_id: i64,
) -> Option<AgentSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.local_history_id.get() == Some(local_id))
        .cloned()
}

fn loaded_history_session_status(session: &AgentSession) -> LoadedHistorySessionStatus {
    let terminal_state = session.state.get();
    let active_state = match terminal_state {
        TerminalSessionState::Starting => Some(session.active_state.get()),
        TerminalSessionState::Running => {
            let state = if session.active_state.get() == AgentActiveState::NewChat {
                AgentActiveState::NewChat
            } else {
                agent_shell_integration::active_state(
                    session.id,
                    session.provider,
                    &session.terminal,
                    false,
                )
            };
            session.active_state.set(state);
            Some(state)
        }
        TerminalSessionState::Exited | TerminalSessionState::Closing => None,
    };

    LoadedHistorySessionStatus {
        session_id: session.id,
        terminal_state: terminal_session_state_label(terminal_state),
        active_state,
    }
}

fn active_session_status(session: &AgentSession) -> ActiveSessionStatus {
    let loaded = loaded_history_session_status(session);
    ActiveSessionStatus {
        session_id: session.id,
        session_uuid: session.session_uuid.clone(),
        local_history_id: session.local_history_id.get(),
        provider_id: session.provider.provider_id(),
        title: session.label.text().to_string(),
        terminal_state: loaded.terminal_state,
        active_state: loaded.active_state,
    }
}

fn terminal_session_state_label(state: TerminalSessionState) -> &'static str {
    match state {
        TerminalSessionState::Starting => "Starting",
        TerminalSessionState::Running => "Running",
        TerminalSessionState::Exited => "Exited",
        TerminalSessionState::Closing => "Closing",
    }
}

fn ensure_agent_history_session(
    session: &AgentSession,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) -> Result<i64, String> {
    if let Some(local_id) = session.local_history_id.get() {
        return Ok(local_id);
    }

    let workspace = workspace_history.borrow().clone();
    let initial_restore_state = if session.provider.provider_id() == "codex" {
        RestoreState::Unmapped
    } else {
        RestoreState::Unsupported
    };
    let row = agent_history::upsert_session_for_manual_id(
        agent_history::AgentSessionUpsert {
            provider_id: session.provider.provider_id().to_string(),
            workspace,
            title: session.label.text().to_string(),
            initial_restore_state,
            session_uuid: Some(session.session_uuid.clone()),
        },
        session.id,
    )?;
    session.local_history_id.set(Some(row.id));
    log::info!(
        "agent history persisted for manual session id session_id={} local_id={} provider={} title={}",
        session.id,
        row.id,
        session.provider.provider_id(),
        agent_shell_integration::log_preview(
            &row.title,
            agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
        )
    );
    notify_history_changed(history_callback);
    Ok(row.id)
}

fn history_session_id(local_id: i64) -> Result<u64, String> {
    u64::try_from(local_id)
        .ok()
        .filter(|session_id| *session_id > 0)
        .ok_or_else(|| format!("Invalid Craic session id {local_id}."))
}

fn session_by_page(
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    page: &gtk::Widget,
) -> Option<AgentSession> {
    page.widget_name()
        .parse::<u64>()
        .ok()
        .and_then(|session_id| session_by_id(sessions, session_id))
}

fn is_selected_session(session: &AgentSession, notebook: &gtk::Notebook) -> bool {
    selected_session_id(notebook) == Some(session.id)
}

fn selected_session_id(notebook: &gtk::Notebook) -> Option<u64> {
    notebook
        .current_page()
        .and_then(|page_num| notebook.nth_page(Some(page_num)))
        .and_then(|page| page.widget_name().parse::<u64>().ok())
}

fn active_state_counts_as_running(state: AgentActiveState) -> bool {
    matches!(state, AgentActiveState::Loading | AgentActiveState::Asking)
}

fn session_state_for_poll(
    session: &AgentSession,
    terminal_state: TerminalSessionState,
) -> AgentSessionState {
    match terminal_state {
        TerminalSessionState::Starting => AgentSessionState::Active(session.active_state.get()),
        TerminalSessionState::Running => {
            if session.active_state.get() == AgentActiveState::NewChat {
                AgentSessionState::Active(AgentActiveState::NewChat)
            } else {
                AgentSessionState::Active(agent_shell_integration::active_state(
                    session.id,
                    session.provider,
                    &session.terminal,
                    false,
                ))
            }
        }
        TerminalSessionState::Exited | TerminalSessionState::Closing => {
            AgentSessionState::Inactive(AgentInactiveState::Dead)
        }
    }
}

fn configured_terminal(
    font_size: f64,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
) -> vte4::Terminal {
    let terminal =
        terminal_component::configured_terminal(font_size, DEFAULT_COLUMNS, DEFAULT_ROWS);
    install_terminal_shortcuts(&terminal, sessions);
    terminal
}

fn set_terminal_font(terminal: &vte4::Terminal, font_size: f64) {
    terminal_component::set_font(terminal, font_size);
}

fn install_terminal_shortcuts(
    terminal: &vte4::Terminal,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let terminal = terminal.clone();
        let sessions = sessions.clone();

        move |_, key, _, modifiers| {
            if let Some(delta) = font_size_delta_for_key(key, modifiers) {
                let current = config::load().font_sizes.shell;
                let next =
                    config::normalize_font_size(current + delta, config::DEFAULT_SHELL_FONT_SIZE);
                if (next - current).abs() > f64::EPSILON {
                    set_terminal_font_for_sessions(&terminal, &sessions, next);
                    config::save_shell_font_size(next);
                }
                return glib::Propagation::Stop;
            }

            reset_kinetic_scroll_if_needed(&terminal, key);

            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

            if ctrl
                && !shift
                && matches!(key, gdk::Key::c | gdk::Key::C)
                && terminal.has_selection()
            {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::c | gdk::Key::C) {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::v | gdk::Key::V) {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && shift && key == gdk::Key::Insert {
                copy_terminal_selection(&terminal);
                return glib::Propagation::Stop;
            }

            if shift && key == gdk::Key::Insert {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && !shift && key == gdk::Key::BackSpace {
                terminal.feed_child(CTRL_BACKSPACE_SEQUENCE);
                return glib::Propagation::Stop;
            }

            if let Some(sequence) = modified_key_sequence(key, modifiers) {
                terminal.feed_child(sequence.as_bytes());
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }
    });
    terminal.add_controller(keys);
}

fn set_terminal_font_for_sessions(
    terminal: &vte4::Terminal,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    font_size: f64,
) {
    set_terminal_font(terminal, font_size);
    for session in sessions.borrow().iter() {
        set_terminal_font(&session.terminal, font_size);
    }
}

fn font_size_delta_for_key(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<f64> {
    if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        || modifiers.contains(gdk::ModifierType::ALT_MASK)
    {
        return None;
    }

    if key == gdk::Key::plus || key == gdk::Key::equal || key == gdk::Key::KP_Add {
        return Some(1.0);
    }
    if key == gdk::Key::minus || key == gdk::Key::underscore || key == gdk::Key::KP_Subtract {
        return Some(-1.0);
    }

    None
}

fn copy_terminal_selection(terminal: &vte4::Terminal) {
    context_menu::copy_terminal_selection(terminal);
}

fn reset_kinetic_scroll_if_needed(terminal: &vte4::Terminal, key: gdk::Key) {
    if is_modifier_key(key) || !terminal.is_scroll_on_keystroke() {
        return;
    }

    let Some(scroller) = terminal
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())
    else {
        return;
    };
    if !scroller.is_kinetic_scrolling() {
        return;
    }

    let adjustment = scroller.vadjustment();
    if adjustment.upper() - adjustment.page_size() > adjustment.value() {
        scroller.set_kinetic_scrolling(false);
        scroller.set_kinetic_scrolling(true);
    }
}

fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::ISO_Level5_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Num_Lock
    )
}

fn modified_key_sequence(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<String> {
    let modifier_mask = kitty_modifier_mask(modifiers)?;
    let codepoint = modified_key_codepoint(key)?;
    Some(format!("\x1b[{codepoint};{modifier_mask}:1u"))
}

fn kitty_modifier_mask(modifiers: gdk::ModifierType) -> Option<u8> {
    let mut mask = 1;

    if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        mask += 1;
    }
    if modifiers.contains(gdk::ModifierType::ALT_MASK) {
        mask += 2;
    }
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        mask += 4;
    }
    if modifiers.contains(gdk::ModifierType::SUPER_MASK) {
        mask += 8;
    }

    (mask != 1).then_some(mask)
}

fn modified_key_codepoint(key: gdk::Key) -> Option<u32> {
    match key {
        gdk::Key::Return | gdk::Key::KP_Enter => Some(13),
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => Some(9),
        gdk::Key::BackSpace => Some(127),
        gdk::Key::Escape => Some(27),
        gdk::Key::Left => Some(57_417),
        gdk::Key::Right => Some(57_418),
        gdk::Key::Up => Some(57_419),
        gdk::Key::Down => Some(57_420),
        gdk::Key::Page_Up => Some(57_421),
        gdk::Key::Page_Down => Some(57_422),
        gdk::Key::Home => Some(57_423),
        gdk::Key::End => Some(57_424),
        gdk::Key::Insert => Some(57_425),
        gdk::Key::Delete => Some(57_426),
        _ => None,
    }
}

fn install_exit_key_handler(
    session_id: u64,
    terminal: &vte4::Terminal,
    state: &Rc<Cell<TerminalSessionState>>,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let state = state.clone();
        let sessions = sessions.clone();
        let notebook = notebook.clone();
        let close_callback = close_callback.clone();
        let history_callback = history_callback.clone();

        move |_, key, _, _| {
            if state.get() == TerminalSessionState::Exited
                && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
            {
                close_session(
                    session_id,
                    &sessions,
                    &notebook,
                    &close_callback,
                    &history_callback,
                );
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }
    });
    terminal.add_controller(keys);
}

fn connect_child_exit(
    session_id: u64,
    provider: &'static dyn AgentProvider,
    terminal: &vte4::Terminal,
    label: &gtk::Label,
    fallback_title: &str,
    child_pid: &Rc<Cell<Option<glib::Pid>>>,
    state: &Rc<Cell<TerminalSessionState>>,
    shell_integration: &'static dyn AgentShellIntegration,
    state_callback: &Rc<
        RefCell<Option<Rc<dyn Fn(u64, &'static dyn AgentProvider, AgentSessionState)>>>,
    >,
) {
    terminal.connect_child_exited({
        let label = label.clone();
        let fallback_title = fallback_title.to_string();
        let child_pid = child_pid.clone();
        let state = state.clone();
        let state_callback = state_callback.clone();

        move |terminal, status| {
            child_pid.set(None);
            if state.get() == TerminalSessionState::Closing {
                shell_integration.log_child_exit_ignored_while_closing(status);
                return;
            }

            state.set(TerminalSessionState::Exited);
            notify_session_state_changed(
                &state_callback,
                session_id,
                provider,
                AgentSessionState::Inactive(AgentInactiveState::Dead),
            );
            let summary = child_exit_summary(status);
            shell_integration.log_child_exited(status, &summary.message);
            terminal.feed(
                format!(
                    "\r\n\r\nProgram {}. Press Enter to close the terminal.\r\n",
                    summary.message
                )
                .as_bytes(),
            );
            label.set_label(&format!("{fallback_title} ({})", summary.label));
        }
    });
}

fn connect_title_updates(
    session_id: u64,
    session_uuid: &str,
    provider: &'static dyn AgentProvider,
    terminal: &vte4::Terminal,
    label: &gtk::Label,
    state: &Rc<Cell<TerminalSessionState>>,
    title_locked: &Rc<Cell<bool>>,
    active_state: &Rc<Cell<AgentActiveState>>,
    local_history_id: &Rc<Cell<Option<i64>>>,
    notebook: &gtk::Notebook,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    state_callback: &Rc<
        RefCell<Option<Rc<dyn Fn(u64, &'static dyn AgentProvider, AgentSessionState)>>>,
    >,
    title_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64, String)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    terminal.connect_notify_local(Some("window-title"), {
        let label = label.clone();
        let state = state.clone();
        let title_locked = title_locked.clone();
        let active_state = active_state.clone();
        let local_history_id = local_history_id.clone();
        let notebook = notebook.clone();
        let workspace_history = workspace_history.clone();
        let state_callback = state_callback.clone();
        let title_callback = title_callback.clone();
        let history_callback = history_callback.clone();
        let session_uuid = session_uuid.to_string();

        move |terminal, _| {
            if state.get() == TerminalSessionState::Closing {
                return;
            }
            if title_locked.get() {
                return;
            }

            let log_scan = selected_session_id(&notebook) == Some(session_id);
            let Some(title) = agent_shell_integration::session_title(provider, terminal, log_scan)
            else {
                return;
            };

            if label.text().as_str() == title.as_str() {
                title_locked.set(true);
                return;
            }

            label.set_label(&title);
            title_locked.set(true);
            if active_state.get() == AgentActiveState::NewChat {
                let next_active_state = match state.get() {
                    TerminalSessionState::Running => {
                        agent_shell_integration::active_state(
                            session_id, provider, terminal, log_scan,
                        )
                    }
                    TerminalSessionState::Starting => AgentActiveState::NewChat,
                    TerminalSessionState::Exited | TerminalSessionState::Closing => {
                        AgentActiveState::Idle
                    }
                };
                if log_scan {
                    log::debug!(
                        "agent notify title active state session_id={} provider={} terminal_state={:?} next_active_state={:?}",
                        session_id,
                        provider.provider_id(),
                        state.get(),
                        next_active_state
                    );
                }
                active_state.set(next_active_state);
                notify_session_state_changed(
                    &state_callback,
                    session_id,
                    provider,
                    AgentSessionState::Active(next_active_state),
                );
            }
            persist_agent_session_title(
                session_id,
                provider,
                &title,
                &workspace_history,
                &local_history_id,
                &session_uuid,
                &history_callback,
            );
            if let Some(ref cb) = *title_callback.borrow() {
                cb(session_id, title);
            }
        }
    });
}

fn persist_agent_session_title(
    session_id: u64,
    provider: &'static dyn AgentProvider,
    title: &str,
    workspace_history: &Rc<RefCell<agent_history::WorkspaceKey>>,
    local_history_id: &Rc<Cell<Option<i64>>>,
    session_uuid: &str,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    if !agent_history::default_title_should_persist(title) {
        return;
    }
    if let Some(local_id) = local_history_id.get() {
        match agent_history::update_session_title(local_id, title) {
            Ok(row) => {
                log::info!(
                    "agent history title updated session_id={} local_id={} provider={} title={}",
                    session_id,
                    row.id,
                    provider.provider_id(),
                    agent_shell_integration::log_preview(
                        &row.title,
                        agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
                    )
                );
                if provider.provider_id() == "codex" {
                    let outcome = map_codex_session_now(row.id, "title-update");
                    schedule_codex_mapping_retries(row.id, outcome, history_callback);
                }
                notify_history_changed(history_callback);
            }
            Err(err) => {
                log::warn!(
                    "agent history title update failed session_id={} local_id={} provider={} error={}",
                    session_id,
                    local_id,
                    provider.provider_id(),
                    err
                );
            }
        }
        return;
    }

    let workspace = workspace_history.borrow().clone();
    let initial_restore_state = if provider.provider_id() == "codex" {
        RestoreState::Unmapped
    } else {
        RestoreState::Unsupported
    };
    let upsert = agent_history::AgentSessionUpsert {
        provider_id: provider.provider_id().to_string(),
        workspace,
        title: title.to_string(),
        initial_restore_state,
        session_uuid: Some(session_uuid.to_string()),
    };

    match agent_history::upsert_session(upsert) {
        Ok(row) => {
            local_history_id.set(Some(row.id));
            log::info!(
                "agent history persisted session_id={} local_id={} provider={} title={}",
                session_id,
                row.id,
                provider.provider_id(),
                agent_shell_integration::log_preview(
                    &row.title,
                    agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
                )
            );
            if provider.provider_id() == "codex" {
                let outcome = map_codex_session_now(row.id, "title");
                schedule_codex_mapping_retries(row.id, outcome, history_callback);
            }
            notify_history_changed(history_callback);
        }
        Err(err) => {
            log::warn!(
                "agent history persist failed session_id={} provider={} error={}",
                session_id,
                provider.provider_id(),
                err
            );
        }
    }
}

fn schedule_codex_mapping_retries(
    local_id: i64,
    outcome: Option<agent_history::CodexMappingOutcome>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    if !codex_mapping_should_retry(outcome.as_ref()) {
        return;
    }
    schedule_codex_mapping_retry(local_id, 0, history_callback.clone());
}

fn schedule_codex_mapping_retry(
    local_id: i64,
    attempt: usize,
    history_callback: Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let Some(delay_ms) = CODEX_MAPPING_RETRY_DELAYS_MS.get(attempt).copied() else {
        return;
    };
    glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
        let reason = format!("retry-{}", attempt + 1);
        let outcome = map_codex_session_now(local_id, &reason);
        notify_history_changed(&history_callback);
        if codex_mapping_should_retry(outcome.as_ref()) {
            schedule_codex_mapping_retry(local_id, attempt + 1, history_callback);
        }
    });
}

fn codex_mapping_should_retry(outcome: Option<&agent_history::CodexMappingOutcome>) -> bool {
    matches!(
        outcome,
        None | Some(agent_history::CodexMappingOutcome::Missing)
    )
}

fn retry_empty_codex_mapping_on_status_change(
    session: &AgentSession,
    previous_state: AgentSessionState,
    next_state: AgentSessionState,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    if session.provider.provider_id() != "codex" {
        return;
    }
    if !status_change_should_retry_codex_mapping(previous_state, next_state) {
        return;
    }
    if matches!(
        (previous_state, next_state),
        (AgentSessionState::Active(AgentActiveState::NewChat), _)
            | (_, AgentSessionState::Active(AgentActiveState::NewChat))
    ) {
        log::debug!(
            "agent history codex mapping status-change retry skipped new chat session_id={} previous_state={:?} next_state={:?}",
            session.id,
            previous_state,
            next_state
        );
        return;
    }

    let Some(local_id) = session.local_history_id.get() else {
        return;
    };
    match agent_history::cli_session_id_is_empty(local_id) {
        Ok(true) => {
            log::info!(
                "agent history codex mapping retry on status change session_id={} local_id={} previous_state={:?} next_state={:?}",
                session.id,
                local_id,
                previous_state,
                next_state
            );
            let outcome = map_codex_session_now(local_id, "status-change");
            schedule_codex_mapping_retries(local_id, outcome, history_callback);
            notify_history_changed(history_callback);
        }
        Ok(false) => {}
        Err(err) => {
            log::warn!(
                "agent history codex mapping status-change check failed session_id={} local_id={} error={}",
                session.id,
                local_id,
                err
            );
        }
    }
}

fn status_change_should_retry_codex_mapping(
    previous_state: AgentSessionState,
    next_state: AgentSessionState,
) -> bool {
    matches!(
        (previous_state, next_state),
        (AgentSessionState::Active(_), AgentSessionState::Active(_))
    )
}

fn map_codex_session_now(
    local_id: i64,
    reason: &str,
) -> Option<agent_history::CodexMappingOutcome> {
    match agent_history::map_codex_session(local_id) {
        Ok(outcome) => {
            log::debug!(
                "agent history codex mapping result local_id={} reason={} outcome={:?}",
                local_id,
                reason,
                outcome
            );
            Some(outcome)
        }
        Err(err) => {
            log::warn!(
                "agent history codex mapping failed local_id={} reason={} error={}",
                local_id,
                reason,
                err
            );
            None
        }
    }
}

fn mark_agent_history_ended(
    session: &AgentSession,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let Some(local_id) = session.local_history_id.get() else {
        return;
    };
    if session.provider.provider_id() == "codex" {
        map_codex_session_now(local_id, "close");
    }
    match agent_history::mark_ended(local_id) {
        Ok(()) => {
            log::info!(
                "agent history marked ended session_id={} local_id={} provider={}",
                session.id,
                local_id,
                session.provider.provider_id()
            );
            notify_history_changed(history_callback);
        }
        Err(err) => {
            log::warn!(
                "agent history mark ended failed session_id={} local_id={} error={}",
                session.id,
                local_id,
                err
            );
        }
    }
}

fn notify_history_changed(history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>) {
    if let Some(ref cb) = *history_callback.borrow() {
        cb();
    }
}

fn notify_session_state_changed(
    state_callback: &Rc<
        RefCell<Option<Rc<dyn Fn(u64, &'static dyn AgentProvider, AgentSessionState)>>>,
    >,
    session_id: u64,
    provider: &'static dyn AgentProvider,
    state: AgentSessionState,
) {
    if let Some(ref cb) = *state_callback.borrow() {
        cb(session_id, provider, state);
    }
}

fn notify_agent_turn_complete(
    ctx: &PageContext,
    session_id: u64,
    provider: &'static dyn AgentProvider,
    state: AgentActiveState,
    title: &str,
) {
    let notification_content = provider.shell_integration().notification(state, title);
    log::info!(
        "agent notification session_id={} provider={} state={:?} summary={} body={}",
        session_id,
        provider.label(),
        state,
        agent_shell_integration::log_preview(
            &notification_content.summary,
            agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
        ),
        agent_shell_integration::log_preview(
            &notification_content.body,
            agent_shell_integration::TERMINAL_LOG_PREVIEW_CHARS
        )
    );
    if let Some(app) = ctx.window().and_then(|window| window.application()) {
        let notification = gio::Notification::new(&notification_content.summary);
        notification.set_body(Some(&notification_content.body));

        let target = session_id.to_variant();
        notification.set_default_action_and_target_value(
            AGENT_SESSION_NOTIFICATION_DETAILED_ACTION,
            Some(&target),
        );
        notification.add_button_with_target_value(
            "Open",
            AGENT_SESSION_NOTIFICATION_DETAILED_ACTION,
            Some(&target),
        );

        app.send_notification(
            Some(&agent_session_notification_id(session_id)),
            &notification,
        );
        return;
    }

    std::thread::spawn(move || {
        if run_gdbus_notification(&notification_content).is_err() {
            let _ = run_dbus_send_notification(&notification_content);
        }
    });
}

fn run_gdbus_notification(notification: &AgentNotification) -> std::io::Result<()> {
    run_notification_command("gdbus", &gdbus_notification_args(notification))
}

fn run_dbus_send_notification(notification: &AgentNotification) -> std::io::Result<()> {
    run_notification_command("dbus-send", &dbus_send_notification_args(notification))
}

fn run_notification_command(program: &str, args: &[String]) -> std::io::Result<()> {
    let status = Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{program} exited with status {status}"
        )))
    }
}

fn gdbus_notification_args(notification: &AgentNotification) -> Vec<String> {
    vec![
        "call".to_string(),
        "--session".to_string(),
        "--dest".to_string(),
        "org.freedesktop.Notifications".to_string(),
        "--object-path".to_string(),
        "/org/freedesktop/Notifications".to_string(),
        "--method".to_string(),
        "org.freedesktop.Notifications.Notify".to_string(),
        gvariant_string_arg(NOTIFICATION_APP_NAME),
        "uint32 0".to_string(),
        gvariant_string_arg(""),
        gvariant_string_arg(&notification.summary),
        gvariant_string_arg(&notification.body),
        "@as []".to_string(),
        "@a{sv} {}".to_string(),
        format!("int32 {NOTIFICATION_TIMEOUT_MS}"),
    ]
}

fn dbus_send_notification_args(notification: &AgentNotification) -> Vec<String> {
    vec![
        "--session".to_string(),
        "--dest=org.freedesktop.Notifications".to_string(),
        "--type=method_call".to_string(),
        "/org/freedesktop/Notifications".to_string(),
        "org.freedesktop.Notifications.Notify".to_string(),
        format!("string:{NOTIFICATION_APP_NAME}"),
        "uint32:0".to_string(),
        "string:".to_string(),
        format!("string:{}", notification.summary),
        format!("string:{}", notification.body),
        "array:string:".to_string(),
        "dict:string:variant:".to_string(),
        format!("int32:{NOTIFICATION_TIMEOUT_MS}"),
    ]
}

fn gvariant_string_arg(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        match ch {
            '\'' => escaped.push_str("\\'"),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                write!(escaped, "\\u{:04x}", ch as u32)
                    .expect("writing to a String should not fail");
            }
            ch => escaped.push(ch),
        }
    }
    escaped.push('\'');
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_expandable_tool_output_counts_as_loading() {
        let text = "Read(/tmp/example.rs) (ctrl+o to expand)";

        assert_eq!(
            agy_terminal_text_active_state(text),
            AgentActiveState::Loading
        );
    }

    #[test]
    fn agy_newer_tool_output_overrides_stale_permission_prompt() {
        let text = "\
Requesting permission for:
Bash(cargo check)
Do you want to proceed?
Read(/tmp/example.rs) (ctrl+o to expand)";

        assert_eq!(
            agy_terminal_text_active_state(text),
            AgentActiveState::Loading
        );
    }

    #[test]
    fn agy_current_permission_prompt_waits_for_user() {
        let text = "\
Read(/tmp/example.rs) (ctrl+o to expand)
Requesting permission for:
Do you want to proceed?";

        assert_eq!(
            agy_terminal_text_active_state(text),
            AgentActiveState::Asking
        );
    }
}

struct ChildExitSummary {
    message: String,
    label: String,
}

fn child_exit_summary(status: i32) -> ChildExitSummary {
    if libc::WIFEXITED(status) {
        let code = libc::WEXITSTATUS(status);
        return ChildExitSummary {
            message: format!("exited with code {code}"),
            label: format!("exited {code}"),
        };
    }

    if libc::WIFSIGNALED(status) {
        let signal = libc::WTERMSIG(status);
        return ChildExitSummary {
            message: format!("terminated by signal {signal}"),
            label: format!("signal {signal}"),
        };
    }

    ChildExitSummary {
        message: format!("exited with status {status}"),
        label: "exited".to_string(),
    }
}

fn spawn_command(
    terminal: &vte4::Terminal,
    command: &CommandSpec,
    child_pid: &Rc<Cell<Option<glib::Pid>>>,
    state: &Rc<Cell<TerminalSessionState>>,
    shell_integration: &'static dyn AgentShellIntegration,
    session_id: u64,
    provider: &'static dyn AgentProvider,
    state_callback: &Rc<
        RefCell<Option<Rc<dyn Fn(u64, &'static dyn AgentProvider, AgentSessionState)>>>,
    >,
) -> Result<(), String> {
    let argv = command_argv(command)?;
    let argv_refs = argv.iter().map(String::as_str).collect::<Vec<_>>();
    let env = terminal_environment();
    let env_refs = env.iter().map(String::as_str).collect::<Vec<_>>();
    let working_dir = command.working_dir();

    shell_integration.log_spawn_requested(working_dir, &command.display(), env_refs.len());
    terminal.spawn_async(
        vte4::PtyFlags::DEFAULT,
        Some(working_dir),
        &argv_refs,
        &env_refs,
        glib::SpawnFlags::SEARCH_PATH,
        || {},
        -1,
        None::<&gio::Cancellable>,
        {
            let terminal = terminal.clone();
            let child_pid = child_pid.clone();
            let state = state.clone();
            let display = command.display();
            let state_callback = state_callback.clone();

            move |result| match result {
                Ok(pid) => {
                    if state.get() == TerminalSessionState::Closing {
                        shell_integration.log_spawn_completed_after_close(pid, &display);
                        terminate_child(Some(pid));
                        return;
                    }

                    child_pid.set(Some(pid));
                    state.set(TerminalSessionState::Running);
                    shell_integration.log_spawned(pid, &display);
                }
                Err(err) => {
                    if state.get() == TerminalSessionState::Closing {
                        shell_integration.log_spawn_failed_after_close(&display, &err);
                        return;
                    }

                    child_pid.set(None);
                    state.set(TerminalSessionState::Exited);
                    notify_session_state_changed(
                        &state_callback,
                        session_id,
                        provider,
                        AgentSessionState::Inactive(AgentInactiveState::Dead),
                    );
                    shell_integration.log_spawn_failed(&display, &err);
                    let message = format!(
                        "Failed to start {display}: {err}\r\n\r\nPress Enter to close the terminal.\r\n"
                    );
                    terminal.feed(message.as_bytes());
                }
            }
        },
    );
    Ok(())
}

fn command_argv(command: &CommandSpec) -> Result<Vec<String>, String> {
    command
        .argv()
        .into_iter()
        .map(|part| {
            part.into_string().map_err(|part| {
                format!(
                    "Cannot start {}: argument is not valid UTF-8: {}",
                    command.display(),
                    part.to_string_lossy()
                )
            })
        })
        .collect()
}

fn terminal_environment() -> Vec<String> {
    let mut env = std::env::vars().collect::<Vec<_>>();
    set_env(&mut env, "TERM", TERM_NAME);
    set_env(&mut env, "COLORTERM", COLORTERM_NAME);
    set_env(&mut env, "TERM_PROGRAM", "VTE");
    set_env(&mut env, "TERM_PROGRAM_VERSION", VTE_VERSION);
    set_env(&mut env, "VTE_VERSION", VTE_VERSION);

    if !has_utf8_locale(&env) {
        set_env(&mut env, "LC_CTYPE", "C.UTF-8");
    }

    env.into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    env.retain(|(existing, _)| existing != key);
    env.push((key.to_string(), value.to_string()));
}

fn has_utf8_locale(env: &[(String, String)]) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|key| {
        env.iter().any(|(existing_key, value)| {
            existing_key == key && {
                let value = value.to_ascii_lowercase();
                value.contains("utf-8") || value.contains("utf8")
            }
        })
    })
}

fn terminate_child(pid: Option<glib::Pid>) {
    let Some(pid) = pid else {
        return;
    };
    unsafe {
        libc::kill(pid.0 as libc::pid_t, libc::SIGHUP);
    }
}
