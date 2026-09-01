use adw::prelude::*;
use gtk::glib::prelude::ToVariant;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};

use super::super::PageContext;
use super::agent_shell_integration::{self, AgentNotification, AgentShellIntegration};
use super::app_session::{AppChatSession, AppChatState};
use super::prompts::{PromptBar, PromptSelection};
use super::smart_summary;
use super::{
    AGENT_ICON_PIXEL_SIZE,
    provider::{self, AgentProvider, CommandSpec},
};
use crate::config;
use crate::system::capabilities::shell::ShellAccess;
use crate::system::capabilities::terminal_link::TerminalLinkTarget;
use crate::system::{ProviderKind, WorkspacePath};
use crate::ui::agent_history::{self, AgentSessionRow, RestoreState};
use crate::ui::agent_status::{AgentActiveState, AgentInactiveState, AgentSessionState};
use crate::ui::agent_usage::{AgentResourceUsage, ProcessSnapshot, ProcessUsageTracker};
use crate::ui::components::search::{SearchOption, SearchPanel};
use crate::ui::pages::PageCommand;
use crate::ui::{AGENT_SESSION_NOTIFICATION_DETAILED_ACTION, agent_session_notification_id};
use craic_ui_core::ui::command_mailbox;
use craic_ui_terminal::ui::components::terminal as terminal_component;
use craic_ui_terminal::vte::{SpawnSpec, VteTerminal, terminal_environment};

#[cfg(test)]
use super::provider::agy::terminal_text_active_state as agy_terminal_text_active_state;

const CTRL_BACKSPACE_SEQUENCE: &[u8] = b"\x17";
const NOTIFICATION_APP_NAME: &str = "Craic";
const NOTIFICATION_TIMEOUT_MS: &str = "5000";
const WAITING_AGENT_SESSION_ICON: &str = "hand-touch-symbolic";
const SMART_SUMMARY_TRIGGER_ROWS: i64 = 500;
const CODEX_MAPPING_RETRY_DELAYS_MS: &[u64] = &[1_800, 8_000, 30_000, 90_000];
const TERMINAL_CONFLICTING_ACCELS: &[(&str, &[&str])] = &[
    ("app.pull", &["<Control>p"]),
    ("app.push", &["<Control>u"]),
    ("app.refresh", &["<Control>r"]),
    ("app.refresh_page", &["F5"]),
    ("app.preferences", &["<Control>comma"]),
    ("app.shortcuts", &["<Control>question"]),
    ("app.about", &["F1"]),
];
type FocusHandlers = Rc<RefCell<Vec<Box<dyn Fn(bool)>>>>;

#[derive(Clone)]
struct AgentSession {
    id: u64,
    session_uuid: String,
    provider: &'static dyn AgentProvider,
    root: gtk::Overlay,
    terminal: VteTerminal,
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
    _remote_image_uploads: Option<Rc<RemoteImageUploads>>,
}

struct RemoteImageUploads {
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    images: RefCell<Vec<super::remote_image::RemoteImage>>,
}

impl Drop for RemoteImageUploads {
    fn drop(&mut self) {
        super::remote_image::remove_images(
            self.shell.clone(),
            self.working_dir.clone(),
            self.images.get_mut().drain(..).collect(),
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalSessionState {
    Starting,
    Running,
    Exited,
    Closing,
}

#[derive(Clone)]
struct TerminalSearchOptions {
    case_sensitive: Rc<Cell<bool>>,
    whole_word: Rc<Cell<bool>>,
    regex: Rc<Cell<bool>>,
}

#[derive(Clone, Copy, Debug)]
enum TerminalSearchMove {
    Keep,
    Previous,
    Next,
}

impl TerminalSearchOptions {
    fn new() -> Self {
        Self {
            case_sensitive: Rc::new(Cell::new(false)),
            whole_word: Rc::new(Cell::new(false)),
            regex: Rc::new(Cell::new(false)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedHistorySessionStatus {
    pub session_id: u64,
    pub terminal_state: &'static str,
    pub active_state: Option<AgentActiveState>,
}

#[derive(Clone, Debug)]
pub struct ActiveSessionStatus {
    pub session_id: u64,
    pub session_uuid: String,
    pub local_history_id: Option<i64>,
    pub provider_id: &'static str,
    pub title: String,
    pub terminal_state: &'static str,
    pub active_state: Option<AgentActiveState>,
}

pub struct AgentChat {
    pub root: gtk::Box,
    ctx: PageContext,
    prompt_bar: PromptBar,
    search_panel: SearchPanel,
    search_options: TerminalSearchOptions,
    notebook: gtk::Notebook,
    sessions: Rc<RefCell<Vec<AgentSession>>>,
    app_sessions: Rc<RefCell<Vec<AppChatSession>>>,
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
    focus_handlers: FocusHandlers,
    usage_tracker: Rc<RefCell<ProcessUsageTracker>>,
}

include!("right/chat.rs");
include!("right/summary.rs");

fn terminal_full_text(terminal: &VteTerminal) -> Option<String> {
    let text = terminal
        .all_text()?
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
    session.child_pid.set(None);
    session.terminal.terminate();
    if let Some(page_num) = notebook.page_num(&session.root) {
        notebook.remove_page(Some(page_num));
    }

    if let Some(next_session) = notebook
        .current_page()
        .and_then(|page_num| notebook.nth_page(Some(page_num)))
        .and_then(|page| session_by_page(sessions, &page))
    {
        next_session.terminal.grab_focus();
    }

    if let Some(ref cb) = *close_callback.borrow() {
        cb(session_id);
    }

    mark_agent_history_ended(&session, history_callback);
}

fn close_app_session(
    session_id: u64,
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let session = {
        let mut sessions = sessions.borrow_mut();
        let Some(index) = sessions
            .iter()
            .position(|session| session.id() == session_id)
        else {
            return;
        };
        sessions.remove(index)
    };
    let root = session.root();
    if let Some(page_num) = notebook.page_num(&root) {
        notebook.remove_page(Some(page_num));
    }
    session.shutdown();
    if let Some(local_id) = session.local_history_id() {
        if let Err(error) = agent_history::mark_ended(local_id) {
            log::warn!(
                "failed marking Codex App session ended session_id={session_id} local_id={local_id}: {error}"
            );
        } else {
            notify_history_changed(history_callback);
        }
    }
    if let Some(callback) = close_callback.borrow().clone() {
        callback(session_id);
    }
}

fn request_close_app_session(
    session_id: u64,
    root: &gtk::Box,
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    notebook: &gtk::Notebook,
    close_callback: &Rc<RefCell<Option<Rc<dyn Fn(u64)>>>>,
    history_callback: &Rc<RefCell<Option<Rc<dyn Fn()>>>>,
) {
    let Some(session) = app_session_by_id(sessions, session_id) else {
        return;
    };
    if !matches!(
        session.state(),
        AppChatState::Connecting
            | AppChatState::Initializing
            | AppChatState::StartingThread
            | AppChatState::Running
            | AppChatState::AwaitingInput
    ) {
        close_app_session(
            session_id,
            sessions,
            notebook,
            close_callback,
            history_callback,
        );
        return;
    }

    log::info!(
        "native Codex close confirmation shown session_id={} state={:?}",
        session_id,
        session.state()
    );
    let dialog = adw::AlertDialog::builder()
        .heading("Close Codex App Tab?")
        .body("Codex is still connecting, working, or waiting for input. Closing this tab will stop its App Server process.")
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
        let sessions = sessions.clone();
        let notebook = notebook.clone();
        let close_callback = close_callback.clone();
        let history_callback = history_callback.clone();
        move |response| {
            if response.as_str() == "close" {
                close_app_session(
                    session_id,
                    &sessions,
                    &notebook,
                    &close_callback,
                    &history_callback,
                );
            }
        }
    });
}

fn app_session_by_id(
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    session_id: u64,
) -> Option<AppChatSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.id() == session_id)
        .cloned()
}

fn app_session_by_local_history_id(
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    local_id: i64,
) -> Option<AppChatSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.local_history_id() == Some(local_id))
        .cloned()
}

fn app_session_by_thread_id(
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    thread_id: &str,
) -> Option<AppChatSession> {
    sessions
        .borrow()
        .iter()
        .find(|session| session.thread_id().as_deref() == Some(thread_id))
        .cloned()
}

fn app_session_by_page(
    sessions: &Rc<RefCell<Vec<AppChatSession>>>,
    page: &gtk::Widget,
) -> Option<AppChatSession> {
    page.widget_name()
        .parse::<u64>()
        .ok()
        .and_then(|session_id| app_session_by_id(sessions, session_id))
}

fn app_agent_session_state(state: &AppChatState) -> AgentSessionState {
    match state {
        AppChatState::Ready => AgentSessionState::Active(AgentActiveState::Idle),
        AppChatState::AwaitingInput => AgentSessionState::Active(AgentActiveState::Asking),
        AppChatState::Connecting
        | AppChatState::Initializing
        | AppChatState::StartingThread
        | AppChatState::Running => AgentSessionState::Active(AgentActiveState::Loading),
        AppChatState::Failed(_) | AppChatState::Closing | AppChatState::Closed => {
            AgentSessionState::Inactive(AgentInactiveState::Dead)
        }
    }
}

fn app_chat_state_label(state: &AppChatState) -> &'static str {
    match state {
        AppChatState::Connecting => "Connecting",
        AppChatState::Initializing => "Initializing",
        AppChatState::StartingThread => "Starting thread",
        AppChatState::Ready => "Ready",
        AppChatState::Running => "Running",
        AppChatState::AwaitingInput => "Needs input",
        AppChatState::Failed(_) => "Failed",
        AppChatState::Closing => "Closing",
        AppChatState::Closed => "Closed",
    }
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

fn connect_terminal_search_option(
    search_panel: &SearchPanel,
    option: SearchOption,
    option_value: Rc<Cell<bool>>,
    sessions: Rc<RefCell<Vec<AgentSession>>>,
    notebook: gtk::Notebook,
    search_options: TerminalSearchOptions,
) {
    search_panel.connect_option_toggled(option, {
        let search_panel = search_panel.clone();

        move |active| {
            option_value.set(active);
            if let Some(terminal) = active_terminal(&sessions, &notebook) {
                apply_terminal_search(
                    &terminal,
                    &search_panel,
                    &search_options,
                    TerminalSearchMove::Next,
                );
            }
        }
    });
}

fn active_terminal(
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    notebook: &gtk::Notebook,
) -> Option<VteTerminal> {
    notebook
        .current_page()
        .and_then(|page_num| notebook.nth_page(Some(page_num)))
        .and_then(|page| session_by_page(sessions, &page))
        .map(|session| session.terminal)
}

fn apply_terminal_search(
    terminal: &VteTerminal,
    search_panel: &SearchPanel,
    options: &TerminalSearchOptions,
    search_move: TerminalSearchMove,
) {
    let query = search_panel.query();
    if query.is_empty() {
        let _ = terminal.search(None, false);
        search_panel.set_status("");
        return;
    }

    let pattern = terminal_search_pattern(&query, options);
    let backwards = matches!(search_move, TerminalSearchMove::Previous);
    let found = match terminal.search(Some(&pattern), backwards) {
        Ok(found) => found,
        Err(err) => {
            let _ = terminal.search(None, false);
            search_panel.set_status("Invalid");
            log::warn!(
                "agent terminal search regex invalid query_len={} regex_mode={}: {err}",
                query.len(),
                options.regex.get()
            );
            return;
        }
    };

    search_panel.set_status(if found { "Found" } else { "No Results" });
    log::debug!(
        "agent terminal search applied query_len={} move={search_move:?} found={found}",
        query.len()
    );
}

fn terminal_search_pattern(query: &str, options: &TerminalSearchOptions) -> String {
    let mut pattern = if options.regex.get() {
        query.to_string()
    } else {
        regex::escape(query)
    };

    if options.whole_word.get() {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    if !options.case_sensitive.get() {
        pattern = format!("(?i:{pattern})");
    }

    pattern
}

fn handle_agent_terminal_activation(
    ctx: &PageContext,
    activation: terminal_component::TerminalActivation,
) {
    match activation {
        terminal_component::TerminalActivation::Url(url) => {
            confirm_open_agent_terminal_url(ctx.clone(), url)
        }
        terminal_component::TerminalActivation::File(file) => {
            open_agent_terminal_file_location(ctx, &file)
        }
    }
}

fn confirm_open_agent_terminal_url(ctx: PageContext, url: String) {
    let Some(url_opener) = ctx.url_opener() else {
        ctx.show_error(
            "Open Link Failed",
            "Opening links is unavailable for this workspace.",
        );
        log::warn!("agent terminal url activation failed reason=no-url-opener url={url}");
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Open Link?")
        .body(&url)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("open", "Open");
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let parent = ctx.window();
    dialog.choose(
        parent.as_ref(),
        None::<&gio::Cancellable>,
        move |response| {
            if response.as_str() != "open" {
                log::debug!("agent terminal url activation cancelled url={url}");
                return;
            }

            match url_opener
                .resolve_url(&url)
                .and_then(|effect| ctx.execute_effect(effect))
            {
                Ok(message) => {
                    log::info!("agent terminal url opened url={url} message={message}");
                    ctx.show_toast(&message);
                }
                Err(err) => {
                    log::warn!("agent terminal url activation failed url={url}: {err}");
                    ctx.show_error("Open Link Failed", &err);
                }
            }
        },
    );
}

#[derive(Clone, Debug)]
struct TerminalFileLocation {
    path: String,
    line: Option<usize>,
    column: Option<usize>,
}

fn open_agent_terminal_file_location(
    ctx: &PageContext,
    file: &terminal_component::TerminalFileActivation,
) {
    let location = parse_terminal_file_location(&file.target);
    let Some(terminal_links) = ctx.terminal_links() else {
        let message = "Terminal link navigation is unavailable for this workspace.".to_string();
        log::warn!(
            "agent terminal file activation failed target={} launch_dir={} reason=no-terminal-link-capability",
            file.target,
            file.launch_dir
        );
        ctx.show_toast(&message);
        return;
    };
    let target = match terminal_links.resolve_file(&file.launch_dir, &location.path) {
        Ok(path) => path,
        Err(err) => {
            log::warn!(
                "agent terminal file activation failed target={} launch_dir={}: {}",
                file.target,
                file.launch_dir,
                err
            );
            ctx.show_toast(&err);
            return;
        }
    };

    let path = match target {
        TerminalLinkTarget::Workspace(path) => path,
        TerminalLinkTarget::External(path) => {
            log::info!(
                "agent terminal external file activation requesting new window target={} path={} line={:?} column={:?}",
                file.target,
                path.absolute,
                location.line,
                location.column
            );
            ctx.open_external_terminal_path(&path, location.line, location.column);
            return;
        }
    };

    log::info!(
        "agent terminal file activation dispatched target={} resolved_path={} line={:?} column={:?}",
        file.target,
        path.display(),
        location.line,
        location.column
    );
    ctx.dispatch_command(PageCommand::OpenFileLocation {
        path: path.relative_or_empty().to_string(),
        line: location.line,
        column: location.column,
    });
}

fn parse_terminal_file_location(target: &str) -> TerminalFileLocation {
    let target = target
        .strip_prefix("file://")
        .unwrap_or(target)
        .trim()
        .to_string();
    let mut path = target.as_str();
    let mut line = None;
    let mut column = None;

    if let Some((before, last)) = path.rsplit_once(':')
        && let Ok(value) = last.parse::<usize>()
        && value > 0
    {
        if let Some((before_line, maybe_line)) = before.rsplit_once(':')
            && let Ok(line_value) = maybe_line.parse::<usize>()
            && line_value > 0
        {
            path = before_line;
            line = Some(line_value);
            column = Some(value);
        } else {
            path = before;
            line = Some(value);
        }
    }

    TerminalFileLocation {
        path: path.to_string(),
        line,
        column,
    }
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
    search_panel: &SearchPanel,
) -> VteTerminal {
    let terminal = VteTerminal::new(font_size);
    install_terminal_shortcuts(&terminal, sessions, search_panel);
    terminal
}

fn set_terminal_font(terminal: &VteTerminal, font_size: f64) {
    terminal.set_font_size(font_size);
}

fn install_terminal_shortcuts(
    terminal: &VteTerminal,
    sessions: &Rc<RefCell<Vec<AgentSession>>>,
    search_panel: &SearchPanel,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let terminal = terminal.clone();
        let sessions = sessions.clone();
        let search_panel = search_panel.clone();

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

            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

            if ctrl
                && !shift
                && !modifiers.contains(gdk::ModifierType::ALT_MASK)
                && matches!(key, gdk::Key::f | gdk::Key::F)
            {
                search_panel.open();
                return glib::Propagation::Stop;
            }

            if ctrl
                && !shift
                && matches!(key, gdk::Key::c | gdk::Key::C)
                && terminal.has_selection()
            {
                terminal.copy_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::c | gdk::Key::C) {
                terminal.copy_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && shift && matches!(key, gdk::Key::v | gdk::Key::V) {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && !shift && matches!(key, gdk::Key::Insert | gdk::Key::KP_Insert) {
                terminal.copy_clipboard();
                return glib::Propagation::Stop;
            }

            if shift && matches!(key, gdk::Key::Insert | gdk::Key::KP_Insert) {
                terminal.paste_clipboard();
                return glib::Propagation::Stop;
            }

            if ctrl && !shift && key == gdk::Key::BackSpace {
                terminal.feed_child(CTRL_BACKSPACE_SEQUENCE);
                return glib::Propagation::Stop;
            }

            if let Some(sequence) = terminal_component::modified_enter_sequence(key, modifiers) {
                terminal.feed_child(sequence.as_bytes());
                return glib::Propagation::Stop;
            }

            glib::Propagation::Proceed
        }
    });
    terminal.terminal().add_controller(keys);
}

fn set_terminal_font_for_sessions(
    terminal: &VteTerminal,
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

fn install_focus_tracking(terminal: &VteTerminal, focus_handlers: &FocusHandlers) {
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter({
        let focus_handlers = focus_handlers.clone();

        move |_| notify_focus_handlers(&focus_handlers, true)
    });
    focus.connect_leave({
        let focus_handlers = focus_handlers.clone();

        move |_| notify_focus_handlers(&focus_handlers, false)
    });
    terminal.terminal().add_controller(focus);
}

fn notify_focus_handlers(focus_handlers: &FocusHandlers, focused: bool) {
    for handler in focus_handlers.borrow().iter() {
        handler(focused);
    }
}

fn set_terminal_conflicting_accels_enabled(app: &gtk::Application, enabled: bool) {
    for (action, accels) in TERMINAL_CONFLICTING_ACCELS {
        if enabled {
            app.set_accels_for_action(action, accels);
        } else {
            app.set_accels_for_action(action, &[]);
        }
    }
}

fn install_exit_key_handler(
    session_id: u64,
    terminal: &VteTerminal,
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
    terminal.terminal().add_controller(keys);
}

fn connect_child_exit(
    session_id: u64,
    provider: &'static dyn AgentProvider,
    terminal: &VteTerminal,
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
        let terminal = terminal.clone();
        let label = label.clone();
        let fallback_title = fallback_title.to_string();
        let child_pid = child_pid.clone();
        let state = state.clone();
        let state_callback = state_callback.clone();

        move |status| {
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
            let summary = child_exit_summary(status.clone());
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
    terminal: &VteTerminal,
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
    terminal.connect_title_changed({
        let terminal = terminal.clone();
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

        move |_| {
            if state.get() == TerminalSessionState::Closing {
                return;
            }
            if title_locked.get() {
                return;
            }

            let log_scan = selected_session_id(&notebook) == Some(session_id);
            let Some(title) =
                agent_shell_integration::session_title(provider, &terminal, log_scan)
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
                            session_id, provider, &terminal, log_scan,
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

fn child_exit_summary(status: ExitStatus) -> ChildExitSummary {
    if let Some(code) = status.code() {
        return ChildExitSummary {
            message: format!("exited with code {code}"),
            label: format!("exited {code}"),
        };
    }

    if let Some(signal) = status.signal() {
        return ChildExitSummary {
            message: format!("terminated by signal {signal}"),
            label: format!("signal {signal}"),
        };
    }

    ChildExitSummary {
        message: format!("exited with status {status:?}"),
        label: "exited".to_string(),
    }
}

fn spawn_command(
    terminal: &VteTerminal,
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
    let env = terminal_environment();
    let working_dir = command.working_dir();

    shell_integration.log_spawn_requested(working_dir, &command.display(), env.len());
    let mut argv = argv.into_iter();
    let program = argv
        .next()
        .ok_or_else(|| "Cannot start an empty agent command.".to_string())?;
    let display = command.display();
    terminal.spawn(
        SpawnSpec {
            program,
            args: argv.collect(),
            working_directory: PathBuf::from(working_dir),
            env,
        },
        command.target_working_dir().to_string(),
        {
            let terminal = terminal.clone();
            let child_pid = child_pid.clone();
            let state = state.clone();
            let state_callback = state_callback.clone();

            move |result| match result {
                Ok(pid) => {
                    let pid = glib::Pid(pid);
                    if state.get() == TerminalSessionState::Closing {
                        shell_integration.log_spawn_completed_after_close(pid, &display);
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
                    terminal.feed(
                        format!(
                            "Failed to start {display}: {err}\r\n\r\nPress Enter to close the terminal.\r\n"
                        )
                        .as_bytes(),
                    );
                }
            }
        },
    )?;
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
