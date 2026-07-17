mod agent_shell_integration;
mod left;
mod prompts;
mod provider;
mod right;
mod smart_summary;

use super::{
    Page, PageBadge, PageCommand, PageCommandResult, PageContext, PageInitializeComplete,
    PageRefreshComplete, PageRefreshRequest,
};
use crate::git::WorkspaceSnapshot;
use crate::ui::agent_history;
use adw::prelude::*;
use gtk::gio;
use left::{AgentList, AgentListContextAction, AgentListSelection};
use right::AgentChat;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const AGENT_ICON_PIXEL_SIZE: i32 = 18;

#[derive(Clone)]
pub struct AgentPage {
    ctx: PageContext,
    state: Rc<RefCell<Option<AgentPageState>>>,
    initializing: Rc<Cell<bool>>,
    init_callbacks: Rc<RefCell<Vec<PageInitializeComplete>>>,
    pending_commands: Rc<RefCell<Vec<PageCommand>>>,
}

#[derive(Clone)]
struct AgentPageState {
    list: Rc<AgentList>,
    chat: Rc<AgentChat>,
}

impl AgentPage {
    pub fn new(ctx: PageContext) -> Self {
        Self {
            ctx,
            state: Rc::new(RefCell::new(None)),
            initializing: Rc::new(Cell::new(false)),
            init_callbacks: Rc::new(RefCell::new(Vec::new())),
            pending_commands: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn finish_initialize(&self) {
        if self.state.borrow().is_some() {
            self.initializing.set(false);
            self.complete_init_callbacks();
            return;
        }

        let started = Instant::now();
        log::info!("agent page initialization started");
        let state = AgentPageState {
            list: Rc::new(AgentList::new()),
            chat: Rc::new(AgentChat::new(self.ctx.clone())),
        };
        let workspace = self.ctx.workspace_ref();
        state
            .list
            .set_workspace_key(self.ctx.workspace_key(), workspace.root.absolute);
        self.connect_session_lifecycle(&state);
        state.chat.connect_prompt_bar();
        self.state.replace(Some(state));

        for command in self
            .pending_commands
            .borrow_mut()
            .drain(..)
            .collect::<Vec<_>>()
        {
            self.handle_command(&command);
        }

        self.initializing.set(false);
        self.complete_init_callbacks();
        log::info!(
            "agent page initialization completed elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }

    fn complete_init_callbacks(&self) {
        let Some(state) = self.state.borrow().clone() else {
            return;
        };

        for callback in self.init_callbacks.borrow_mut().drain(..) {
            callback(
                state.list.root.clone().upcast(),
                state.chat.root.clone().upcast(),
            );
        }
    }

    fn connect_session_lifecycle(&self, state: &AgentPageState) {
        let list = state.list.clone();
        let chat = state.chat.clone();

        chat.connect_title_changed({
            let list = list.clone();

            move |session_id, title| {
                list.update_title(session_id, &title);
            }
        });

        chat.connect_state_changed({
            let ctx = self.ctx.clone();
            let list = list.clone();

            move |session_id, provider, state| {
                let state_changed = list.set_session_state(session_id, provider, state);
                if state_changed {
                    ctx.notify_badge_changed();
                }
            }
        });

        chat.connect_resource_usage_changed({
            let list = list.clone();

            move |session_id, usage| {
                list.set_resource_usage(session_id, usage);
            }
        });

        chat.connect_new_session({
            let ctx = self.ctx.clone();
            let list = list.clone();

            move |session_id, provider, title, local_history_id, state| {
                list.add_session_row(session_id, provider, &title, local_history_id, state);
                list.select_session(session_id);
                ctx.notify_badge_changed();
            }
        });

        chat.connect_history_changed({
            let list = list.clone();

            move || {
                list.reload_history();
            }
        });

        chat.connect_close_requested({
            let ctx = self.ctx.clone();
            let list = list.clone();

            move |session_id| {
                if list.remove_session(session_id) {
                    ctx.notify_badge_changed();
                }
            }
        });

        list.connect_new_chat({
            let chat = chat.clone();

            move |provider| {
                chat.start_chat(provider);
            }
        });

        list.connect_selected({
            let ctx = self.ctx.clone();
            let chat = chat.clone();
            let list = list.clone();

            move |selection| match selection {
                AgentListSelection::Active(session_id) => {
                    chat.show_session(session_id);
                }
                AgentListSelection::History(local_id) => {
                    match agent_history::lookup_session(local_id) {
                        Ok(Some(row)) => match chat.restore_session(&row) {
                            Ok(session_id) => {
                                list.select_session(session_id);
                                ctx.notify_badge_changed();
                            }
                            Err(err) => {
                                log::info!(
                                    "agent history restore ignored local_id={} error={}",
                                    local_id,
                                    err
                                );
                            }
                        },
                        Ok(None) => {
                            log::warn!("agent history restore ignored missing local_id={local_id}");
                        }
                        Err(err) => {
                            log::warn!(
                                "agent history restore lookup failed local_id={} error={}",
                                local_id,
                                err
                            );
                        }
                    }
                }
            }
        });

        list.connect_context_action({
            let ctx = self.ctx.clone();
            let list = list.clone();
            let chat = chat.clone();

            move |action| match action {
                AgentListContextAction::ViewStatusHistory(local_id) => {
                    show_history_session_status(&ctx, &chat, local_id);
                }
                AgentListContextAction::ViewStatusActive(session_id) => {
                    show_active_session_status(&ctx, &chat, session_id);
                }
                AgentListContextAction::GenerateSummaryHistory(local_id) => {
                    request_generate_history_summary(&ctx, &chat, local_id);
                }
                AgentListContextAction::GenerateSummaryActive(session_id) => {
                    request_generate_active_summary(&ctx, &chat, session_id);
                }
                AgentListContextAction::SetSessionIdHistory(local_id) => {
                    prompt_set_history_session_id(&ctx, &list, local_id);
                }
                AgentListContextAction::SetSessionIdActive(session_id) => {
                    prompt_set_active_session_id(&ctx, &list, &chat, session_id);
                }
                AgentListContextAction::Unload(local_id) => {
                    chat.request_unload_history_session(local_id);
                }
                AgentListContextAction::Delete(local_id) => {
                    prompt_delete_session(&ctx, &list, &chat, local_id);
                }
            }
        });

        list.connect_close_requested({
            let chat = chat.clone();

            move |session_id| {
                chat.request_close_session(session_id);
            }
        });
    }
}

fn request_generate_history_summary(ctx: &PageContext, chat: &Rc<AgentChat>, local_id: i64) {
    match chat.generate_history_session_summary(local_id) {
        Ok(()) => {
            log::info!("agent smart summary manually requested local_id={local_id}");
            ctx.refresh(Some("Summary generation started.".to_string()));
        }
        Err(err) => ctx.show_error("Generate Summary Failed", &err),
    }
}

fn request_generate_active_summary(ctx: &PageContext, chat: &Rc<AgentChat>, session_id: u64) {
    match chat.generate_active_session_summary(session_id) {
        Ok(()) => {
            log::info!("agent smart summary manually requested session_id={session_id}");
            ctx.refresh(Some("Summary generation started.".to_string()));
        }
        Err(err) => ctx.show_error("Generate Summary Failed", &err),
    }
}

fn prompt_set_history_session_id(ctx: &PageContext, list: &AgentList, local_id: i64) {
    let Some(window) = ctx.window() else {
        return;
    };
    let row = match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            ctx.show_error(
                "Set Session ID Failed",
                "Agent session history was not found.",
            );
            return;
        }
        Err(err) => {
            ctx.show_error("Set Session ID Failed", &err);
            return;
        }
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Set Session ID")
        .body("Enter the CLI session ID to use when restoring this session.")
        .build();
    let default_session_id = agent_history::suggested_cli_session_id(&row).unwrap_or_default();
    let entry = gtk::Entry::builder()
        .placeholder_text("Session ID")
        .text(&default_session_id)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let list = list.clone();
        move |response| {
            if response.as_str() != "save" {
                return;
            }

            let cli_session_id = entry.text().trim().to_string();
            if cli_session_id.is_empty() {
                ctx.show_error("Set Session ID Failed", "Enter a session ID.");
                return;
            }

            match agent_history::set_manual_session_id(local_id, &cli_session_id) {
                Ok(()) => {
                    log::info!("agent session id manually saved local_id={local_id}");
                    list.reload_history();
                    ctx.refresh(Some("Agent session ID saved.".to_string()));
                }
                Err(err) => ctx.show_error("Set Session ID Failed", &err),
            }
        }
    });
}

fn prompt_set_active_session_id(
    ctx: &PageContext,
    list: &AgentList,
    chat: &Rc<AgentChat>,
    session_id: u64,
) {
    let Some(window) = ctx.window() else {
        return;
    };
    let Some(status) = chat.active_session_status(session_id) else {
        ctx.show_error("Set Session ID Failed", "Agent session is not active.");
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Set Session ID")
        .body("Enter the CLI session ID to use when restoring this session.")
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("Session ID")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let list = list.clone();
        let chat = chat.clone();
        move |response| {
            if response.as_str() != "save" {
                return;
            }

            let cli_session_id = entry.text().trim().to_string();
            if cli_session_id.is_empty() {
                ctx.show_error("Set Session ID Failed", "Enter a session ID.");
                return;
            }

            match chat.set_active_session_cli_id(status.session_id, &cli_session_id) {
                Ok(local_id) => {
                    log::info!(
                        "agent active session id manually saved session_id={} local_id={}",
                        status.session_id,
                        local_id
                    );
                    list.reload_history();
                    ctx.refresh(Some("Agent session ID saved.".to_string()));
                }
                Err(err) => ctx.show_error("Set Session ID Failed", &err),
            }
        }
    });
}

fn show_history_session_status(ctx: &PageContext, chat: &Rc<AgentChat>, local_id: i64) {
    let Some(window) = ctx.window() else {
        return;
    };
    let row = match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            ctx.show_error("View Status Failed", "Agent session history was not found.");
            return;
        }
        Err(err) => {
            ctx.show_error("View Status Failed", &err);
            return;
        }
    };
    let loaded_status = chat.loaded_history_session_status(local_id);

    let dialog = adw::AlertDialog::builder()
        .heading("Session Status")
        .body(&row.title)
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.set_extra_child(Some(&session_status_card(&row, loaded_status.as_ref())));
    dialog.present(Some(&window));
}

fn show_active_session_status(ctx: &PageContext, chat: &Rc<AgentChat>, session_id: u64) {
    let Some(window) = ctx.window() else {
        return;
    };
    let Some(status) = chat.active_session_status(session_id) else {
        ctx.show_error("View Status Failed", "Agent session is not active.");
        return;
    };

    let dialog = adw::AlertDialog::builder()
        .heading("Session Status")
        .body(&status.title)
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.set_extra_child(Some(&active_session_status_card(&status)));
    dialog.present(Some(&window));
}

fn session_status_card(
    row: &agent_history::AgentSessionRow,
    loaded_status: Option<&right::LoadedHistorySessionStatus>,
) -> gtk::ScrolledWindow {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    card.add_css_class("card");

    card.append(&status_row("Craic Session ID", &row.session_uuid));
    card.append(&status_row(
        "Loaded",
        if loaded_status.is_some() { "Yes" } else { "No" },
    ));
    if let Some(status) = loaded_status {
        card.append(&status_row("Loaded Tab ID", &status.session_id.to_string()));
        card.append(&status_row("Terminal State", status.terminal_state));
        card.append(&status_row(
            "Active State",
            &status
                .active_state
                .map(|state| format!("{state:?}"))
                .unwrap_or_else(|| "None".to_string()),
        ));
    }
    card.append(&status_row("Provider", &row.provider_id));
    card.append(&status_row("Restore State", row.restore_state.as_str()));
    card.append(&status_row(
        "CLI Session ID",
        row.cli_session_id.as_deref().unwrap_or(""),
    ));
    card.append(&status_row("Title", &row.title));
    card.append(&status_row("Normalized Title", &row.normalized_title));
    card.append(&status_row("Workspace Key", &row.workspace_key));
    card.append(&status_row(
        "Git Remote URL",
        row.git_remote_url.as_deref().unwrap_or(""),
    ));
    card.append(&status_row("Repo Path", &row.repo_path.to_string_lossy()));
    card.append(&status_row(
        "Created",
        &format_history_ms(row.created_at_ms),
    ));
    card.append(&status_row(
        "Updated",
        &format_history_ms(row.updated_at_ms),
    ));
    card.append(&status_row(
        "Last Seen",
        &format_history_ms(row.last_seen_at_ms),
    ));
    card.append(&status_row(
        "Ended",
        &row.ended_at_ms
            .map(format_history_ms)
            .unwrap_or_else(|| "".to_string()),
    ));
    card.append(&status_row(
        "Metadata",
        &pretty_metadata_json(&row.metadata_json),
    ));

    gtk::ScrolledWindow::builder()
        .min_content_width(520)
        .min_content_height(420)
        .max_content_height(520)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&card)
        .build()
}

fn active_session_status_card(status: &right::ActiveSessionStatus) -> gtk::ScrolledWindow {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    card.add_css_class("card");

    card.append(&status_row("Craic Session ID", &status.session_uuid));
    card.append(&status_row("Loaded", "Yes"));
    card.append(&status_row("Loaded Tab ID", &status.session_id.to_string()));
    card.append(&status_row(
        "History Row",
        &status
            .local_history_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "Not saved yet".to_string()),
    ));
    card.append(&status_row("Provider", status.provider_id));
    card.append(&status_row("Title", &status.title));
    card.append(&status_row("Terminal State", status.terminal_state));
    card.append(&status_row(
        "Active State",
        &status
            .active_state
            .map(|state| format!("{state:?}"))
            .unwrap_or_else(|| "None".to_string()),
    ));

    gtk::ScrolledWindow::builder()
        .min_content_width(520)
        .min_content_height(280)
        .max_content_height(420)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&card)
        .build()
}

fn status_row(title: &str, value: &str) -> gtk::Box {
    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .width_chars(16)
        .build();
    title_label.add_css_class("dim-label");
    title_label.add_css_class("caption-heading");

    let value_label = gtk::Label::builder()
        .label(value)
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .selectable(true)
        .build();

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(12)
        .margin_end(12)
        .build();
    row.append(&title_label);
    row.append(&value_label);
    row
}

fn pretty_metadata_json(metadata_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(metadata_json)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|_| metadata_json.to_string())
}

fn format_history_ms(ms: i64) -> String {
    gtk::glib::DateTime::from_unix_local(ms / 1000)
        .and_then(|time| time.format("%Y-%m-%d %H:%M:%S"))
        .map(|label| label.to_string())
        .unwrap_or_else(|_| ms.to_string())
}

fn prompt_delete_session(ctx: &PageContext, list: &AgentList, chat: &Rc<AgentChat>, local_id: i64) {
    let Some(window) = ctx.window() else {
        return;
    };
    let row = match agent_history::lookup_session(local_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            ctx.show_error(
                "Delete Session Failed",
                "Agent session history was not found.",
            );
            return;
        }
        Err(err) => {
            ctx.show_error("Delete Session Failed", &err);
            return;
        }
    };

    let loaded = chat.history_session_is_loaded(local_id);
    let body = if loaded {
        format!(
            "Delete \"{}\" from agent history and close its loaded tab?",
            row.title
        )
    } else {
        format!("Delete \"{}\" from agent history?", row.title)
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Delete Agent Session?")
        .body(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete Session");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.choose(Some(&window), None::<&gio::Cancellable>, {
        let ctx = ctx.clone();
        let list = list.clone();
        let chat = chat.clone();

        move |response| {
            if response.as_str() != "delete" {
                log::info!("agent history delete cancelled local_id={local_id}");
                return;
            }

            chat.close_history_session(local_id);
            match agent_history::delete_session(local_id) {
                Ok(()) => {
                    list.reload_history();
                    ctx.refresh(Some("Agent session deleted.".to_string()));
                }
                Err(err) => ctx.show_error("Delete Session Failed", &err),
            }
        }
    });
}

impl Page for AgentPage {
    fn label(&self) -> &'static str {
        "Agents"
    }

    fn icon_name(&self) -> &'static str {
        "brain-augemnted-symbolic"
    }

    fn initialize(&self, completion: PageInitializeComplete) {
        if let Some(state) = self.state.borrow().clone() {
            completion(
                state.list.root.clone().upcast(),
                state.chat.root.clone().upcast(),
            );
            return;
        }

        self.init_callbacks.borrow_mut().push(completion);
        if self.initializing.replace(true) {
            return;
        }

        log::info!("agent page initialization queued");
        let (sender, receiver) = mpsc::channel();
        let shell = self.ctx.shell();
        thread::spawn(move || {
            let started = Instant::now();
            if let Some(shell) = shell {
                for program in ["codex", "agy", "opencode"] {
                    match shell.which(program) {
                        Ok(Some(path)) => {
                            log::debug!("agent page warmed command program={program} path={path}");
                        }
                        Ok(None) => {
                            log::debug!("agent page command unavailable program={program}");
                        }
                        Err(err) => {
                            log::warn!("agent page command warm failed program={program}: {err}");
                        }
                    }
                }
            }
            log::info!(
                "agent page background initialization completed elapsed_ms={}",
                started.elapsed().as_millis()
            );
            let _ = sender.send(());
        });

        let page = self.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(30), move || {
            match receiver.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                    page.finish_initialize();
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            }
        });
    }

    fn activate(&self) {
        if let Some(state) = self.state.borrow().as_ref() {
            state.chat.show();
            return;
        }

        let page = self.clone();
        self.initialize(Box::new(move |_, _| page.activate()));
    }

    fn refresh(&self, _snapshot: &WorkspaceSnapshot, completion: PageRefreshComplete) {
        completion();
    }

    fn workspace_changed(&self) {
        if let Some(state) = self.state.borrow().clone() {
            refresh_agent_workspace(&self.ctx, &state);
        }
    }

    fn refresh_page(&self, completion: PageRefreshComplete) -> PageRefreshRequest {
        log::info!("agent page refresh requested");
        if let Some(state) = self.state.borrow().as_ref() {
            state.list.reload_workspace_history();
            completion();
        } else {
            let page = self.clone();
            self.initialize(Box::new(move |_, _| {
                if let Some(state) = page.state.borrow().as_ref() {
                    state.list.reload_workspace_history();
                }
                completion();
            }));
        }
        PageRefreshRequest::Custom
    }

    fn set_error(&self, _message: &str) {}

    fn badge(&self) -> Option<PageBadge> {
        let running = self
            .state
            .borrow()
            .as_ref()
            .map(|state| state.chat.running_session_count())
            .unwrap_or(0);
        (running > 0).then(|| PageBadge::new(running.to_string()))
    }

    fn running_agent_session_count(&self) -> usize {
        self.state
            .borrow()
            .as_ref()
            .map(|state| state.chat.running_session_count())
            .unwrap_or(0)
    }

    fn toggle_left_search(&self) -> bool {
        let Some(state) = self.state.borrow().clone() else {
            return false;
        };
        state.list.toggle_search();
        true
    }

    fn handle_command(&self, command: &PageCommand) -> PageCommandResult {
        let Some(state) = self.state.borrow().clone() else {
            return match command {
                PageCommand::AddFileToAgent(_) | PageCommand::OpenAgentSession(_) => {
                    self.pending_commands.borrow_mut().push(command.clone());
                    self.initialize(Box::new(|_, _| {}));
                    PageCommandResult::HandledAndActivate
                }
                _ => PageCommandResult::Ignored,
            };
        };

        match command {
            PageCommand::AddFileToAgent(file_path) => {
                state.chat.add_file_reference(file_path);
                PageCommandResult::HandledAndActivate
            }
            PageCommand::OpenAgentSession(session_id) => {
                if state.chat.show_session(*session_id) {
                    state.list.select_session(*session_id);
                    log::debug!("agent session opened from command session_id={session_id}");
                    PageCommandResult::HandledAndActivate
                } else {
                    log::warn!(
                        "agent session open command ignored missing session_id={session_id}"
                    );
                    PageCommandResult::Ignored
                }
            }
            _ => PageCommandResult::Ignored,
        }
    }
}

fn refresh_agent_workspace(ctx: &PageContext, state: &AgentPageState) {
    let workspace_key = ctx.workspace_key();
    let closed_sessions = state.chat.set_workspace_from_context();
    let workspace = ctx.workspace_ref();
    log::info!(
        "agent page workspace refreshed workspace={} root={} closed_sessions={}",
        workspace_key,
        workspace.root.absolute,
        closed_sessions
    );
    state
        .list
        .set_workspace_key(workspace_key, workspace.root.absolute);
}
