use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

mod history;
mod link_activation;
mod notifications;
mod settings;
mod tools;

use adw::prelude::*;
use craic_codex_app_server::protocol::{
    Request, RequestId, RpcError, ThreadStartParams, TurnInterruptParams, TurnStartParams,
    TurnSteerParams, UserInput,
};
use craic_codex_app_server::{
    AppServer, AppServerConfig, AppServerEvent, ConnectionState, ExitStatus,
};
use gtk::{gio, glib};
use serde_json::{Value, json};

use self::history::PickerRequest;
use self::settings::{DEFAULT_SERVICE_TIER_ID, ModelServiceTiers, set_initial_selector_options};

use super::super::{PageCommand, PageContext};
use super::codex_chat::{
    ChatConnectionStatus, ChatSelector, CodexChatAction, CodexChatView, CollaborationParticipant,
    ComposerAttachment, ComposerAttachmentKind, ComposerSubmission, PendingRequestResponse,
    QueueDirection, QueuedSubmission, SelectorOption, TimelineItem, TimelineItemKind,
    TimelineItemStatus,
};
use super::thread_picker::CodexThreadPicker;
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{ProviderKind, WorkspaceRef};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const MAX_EVENTS_PER_POLL: usize = 256;
const MAX_RENDERED_JSON_BYTES: usize = 24 * 1024;

type TitleCallback = Rc<dyn Fn(u64, String)>;
type StateCallback = Rc<dyn Fn(u64, AppChatState)>;
type SessionCallback = Rc<dyn Fn(u64)>;
type ThreadCallback = Rc<dyn Fn(u64, String, String)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppChatState {
    Connecting,
    Initializing,
    StartingThread,
    Ready,
    Running,
    AwaitingInput,
    Failed(String),
    Closing,
    Closed,
}

#[derive(Clone)]
pub(crate) struct AppChatSession {
    inner: Rc<AppChatSessionInner>,
}

struct PendingServerRequest {
    id: RequestId,
    method: String,
    params: Value,
}

struct QueuedTurn {
    id: String,
    submission: ComposerSubmission,
}

#[derive(Clone, Copy)]
enum ComposerContextSelection {
    Media,
    WorkspaceFile,
    WorkspaceFolder,
}

struct AppChatSessionInner {
    id: u64,
    ctx: PageContext,
    root: gtk::Box,
    content: gtk::Stack,
    view: CodexChatView,
    picker: CodexThreadPicker,
    workspace_key: String,
    workspace_root: String,
    startup: RefCell<Option<Receiver<Result<AppServer, String>>>>,
    server: RefCell<Option<AppServer>>,
    poll_source: RefCell<Option<glib::SourceId>>,
    lifecycle: RefCell<AppChatState>,
    thread_id: RefCell<Option<String>>,
    resume_thread_id: RefCell<Option<String>>,
    local_history_id: Cell<Option<i64>>,
    active_turn_id: RefCell<Option<String>>,
    title: RefCell<String>,
    timeline: RefCell<HashMap<String, TimelineItem>>,
    pending_requests: RefCell<HashMap<String, PendingServerRequest>>,
    selected_values: RefCell<HashMap<ChatSelector, String>>,
    dirty_selectors: RefCell<HashSet<ChatSelector>>,
    model_reasoning: RefCell<HashMap<String, Vec<SelectorOption>>>,
    model_service_tiers: RefCell<HashMap<String, ModelServiceTiers>>,
    context_window_fallback: Cell<Option<u64>>,
    collaboration_modes: RefCell<HashMap<String, Value>>,
    collaboration: RefCell<HashMap<String, CollaborationParticipant>>,
    picker_cursor: RefCell<Option<String>>,
    picker_requests: RefCell<HashMap<RequestId, PickerRequest>>,
    picker_generation: Cell<u64>,
    turns_cursor: RefCell<Option<String>>,
    turns_request: RefCell<Option<RequestId>>,
    tool_requests: RefCell<HashMap<RequestId, tools::ToolRequest>>,
    thread_operations: RefCell<HashMap<RequestId, (String, String)>>,
    queued_turns: RefCell<Vec<QueuedTurn>>,
    temporary_attachments: RefCell<HashMap<String, PathBuf>>,
    next_local_id: Cell<u64>,
    closing: Cell<bool>,
    title_callback: RefCell<Option<TitleCallback>>,
    state_callback: RefCell<Option<StateCallback>>,
    close_callback: RefCell<Option<SessionCallback>>,
    history_callback: RefCell<Option<SessionCallback>>,
    thread_callback: RefCell<Option<ThreadCallback>>,
}

impl AppChatSession {
    pub(crate) fn new(id: u64, ctx: PageContext) -> Result<Self, String> {
        Self::new_with_thread(id, ctx, None, None, "New Codex chat")
    }

    pub(crate) fn resume(
        id: u64,
        ctx: PageContext,
        thread_id: String,
        local_history_id: i64,
        title: &str,
    ) -> Result<Self, String> {
        Self::new_with_thread(id, ctx, Some(thread_id), Some(local_history_id), title)
    }

    fn new_with_thread(
        id: u64,
        ctx: PageContext,
        resume_thread_id: Option<String>,
        local_history_id: Option<i64>,
        title: &str,
    ) -> Result<Self, String> {
        let shell = ctx
            .shell()
            .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
        let provider_kind = ctx.system_ref().provider_kind;
        let workspace = ctx.workspace_ref();
        let view = CodexChatView::new();
        let picker = CodexThreadPicker::new();
        view.set_connection_status(ChatConnectionStatus::Connecting);
        view.set_composer_enabled(false);
        set_initial_selector_options(&view);

        let content = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        let loading_spinner = adw::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        content.add_named(&loading_spinner, Some("loading"));
        content.add_named(&view.root, Some("chat"));
        content.add_named(&picker.root, Some("threads"));
        content.set_visible_child_name("loading");
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&content);
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let startup_workspace = workspace.clone();
        std::thread::Builder::new()
            .name(format!("codex-app-session-start-{id}"))
            .spawn(move || {
                log::info!("native Codex startup worker started session_id={id}");
                let result = app_server_config(shell.as_ref(), &startup_workspace, provider_kind)
                    .and_then(|config| AppServer::spawn(config).map_err(|error| error.to_string()));
                log::info!(
                    "native Codex startup worker finished session_id={id} success={}",
                    result.is_ok()
                );
                let _ = startup_tx.send(result);
            })
            .map_err(|error| format!("Failed to start Codex App Server worker: {error}"))?;

        let inner = Rc::new(AppChatSessionInner {
            id,
            ctx,
            root,
            content,
            view,
            picker,
            workspace_key: workspace.id.to_string(),
            workspace_root: workspace.root.absolute,
            startup: RefCell::new(Some(startup_rx)),
            server: RefCell::new(None),
            poll_source: RefCell::new(None),
            lifecycle: RefCell::new(AppChatState::Connecting),
            thread_id: RefCell::new(None),
            resume_thread_id: RefCell::new(resume_thread_id),
            local_history_id: Cell::new(local_history_id),
            active_turn_id: RefCell::new(None),
            title: RefCell::new(title.to_owned()),
            timeline: RefCell::new(HashMap::new()),
            pending_requests: RefCell::new(HashMap::new()),
            selected_values: RefCell::new(HashMap::new()),
            dirty_selectors: RefCell::new(HashSet::new()),
            model_reasoning: RefCell::new(HashMap::new()),
            model_service_tiers: RefCell::new(HashMap::new()),
            context_window_fallback: Cell::new(None),
            collaboration_modes: RefCell::new(HashMap::new()),
            collaboration: RefCell::new(HashMap::new()),
            picker_cursor: RefCell::new(None),
            picker_requests: RefCell::new(HashMap::new()),
            picker_generation: Cell::new(0),
            turns_cursor: RefCell::new(None),
            turns_request: RefCell::new(None),
            tool_requests: RefCell::new(HashMap::new()),
            thread_operations: RefCell::new(HashMap::new()),
            queued_turns: RefCell::new(Vec::new()),
            temporary_attachments: RefCell::new(HashMap::new()),
            next_local_id: Cell::new(1),
            closing: Cell::new(false),
            title_callback: RefCell::new(None),
            state_callback: RefCell::new(None),
            close_callback: RefCell::new(None),
            history_callback: RefCell::new(None),
            thread_callback: RefCell::new(None),
        });
        inner.connect_view_actions();
        inner.connect_picker_actions();
        inner.start_polling();
        Ok(Self { inner })
    }

    pub(crate) fn id(&self) -> u64 {
        self.inner.id
    }

    pub(crate) fn root(&self) -> gtk::Box {
        self.inner.root.clone()
    }

    pub(crate) fn show(&self) {
        self.inner.root.set_visible(true);
        self.focus();
    }

    pub(crate) fn focus(&self) {
        match self.inner.content.visible_child_name().as_deref() {
            Some("threads") => self.inner.picker.focus_search(),
            Some("chat") => self.inner.view.focus_composer(),
            _ => {}
        }
    }

    pub(crate) fn add_mention(&self, path: impl Into<String>) {
        self.inner.add_attachment(path.into(), true);
    }

    pub(crate) fn add_prompt(&self, prompt: &str) {
        self.inner.view.set_composer_text(prompt);
        self.inner.view.focus_composer();
    }

    pub(crate) fn running(&self) -> bool {
        matches!(
            &*self.inner.lifecycle.borrow(),
            AppChatState::Connecting
                | AppChatState::Initializing
                | AppChatState::StartingThread
                | AppChatState::Running
                | AppChatState::AwaitingInput
        )
    }

    pub(crate) fn state(&self) -> AppChatState {
        self.inner.lifecycle.borrow().clone()
    }

    pub(crate) fn title(&self) -> String {
        self.inner.title.borrow().clone()
    }

    pub(crate) fn thread_id(&self) -> Option<String> {
        self.inner.thread_id.borrow().clone()
    }

    pub(crate) fn local_history_id(&self) -> Option<i64> {
        self.inner.local_history_id.get()
    }

    pub(crate) fn set_local_history_id(&self, local_history_id: i64) {
        self.inner.local_history_id.set(Some(local_history_id));
    }

    pub(crate) fn connect_title_changed<F>(&self, callback: F)
    where
        F: Fn(u64, String) + 'static,
    {
        self.inner.title_callback.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn connect_state_changed<F>(&self, callback: F)
    where
        F: Fn(u64, AppChatState) + 'static,
    {
        self.inner.state_callback.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn connect_close_requested<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.inner.close_callback.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn connect_history_changed<F>(&self, callback: F)
    where
        F: Fn(u64) + 'static,
    {
        self.inner.history_callback.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn connect_thread_changed<F>(&self, callback: F)
    where
        F: Fn(u64, String, String) + 'static,
    {
        self.inner.thread_callback.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl AppChatSessionInner {
    fn connect_view_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.view.connect_action(move |action| {
            if let Some(session) = weak.upgrade() {
                session.handle_action(action);
            }
        });
    }

    fn start_polling(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(EVENT_POLL_INTERVAL, move || {
            let Some(session) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            session.poll();
            if session.closing.get() {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.poll_source.replace(Some(source));
    }

    fn poll(&self) {
        let startup_result = self.startup.borrow().as_ref().map(Receiver::try_recv);
        match startup_result {
            Some(Ok(Ok(server))) => {
                self.startup.borrow_mut().take();
                self.server.replace(Some(server));
                self.set_state(AppChatState::Initializing);
                self.view
                    .set_connection_status(ChatConnectionStatus::Initializing);
            }
            Some(Ok(Err(error))) => {
                self.startup.borrow_mut().take();
                self.fail(error);
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.startup.borrow_mut().take();
                self.fail("Codex App Server startup worker disconnected".to_owned());
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }

        for _ in 0..MAX_EVENTS_PER_POLL {
            let event = {
                let server = self.server.borrow();
                server.as_ref().map(AppServer::try_recv)
            };
            match event {
                Some(Ok(event)) => self.handle_event(event),
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    if !self.closing.get() {
                        self.fail("Codex App Server event stream disconnected".to_owned());
                    }
                    break;
                }
            }
        }
    }

    fn handle_event(&self, event: AppServerEvent) {
        match event {
            AppServerEvent::StateChanged(ConnectionState::Starting) => {
                self.set_state(AppChatState::Connecting)
            }
            AppServerEvent::StateChanged(ConnectionState::Initializing) => {
                self.set_state(AppChatState::Initializing)
            }
            AppServerEvent::StateChanged(ConnectionState::Stopping) => {
                self.set_state(AppChatState::Closing)
            }
            AppServerEvent::StateChanged(ConnectionState::Stopped) => {
                self.set_state(AppChatState::Closed)
            }
            AppServerEvent::StateChanged(ConnectionState::Crashed) => {
                if !self.closing.get() {
                    self.fail("Codex App Server crashed".to_owned());
                }
            }
            AppServerEvent::StateChanged(ConnectionState::Ready) => {}
            AppServerEvent::Ready(_) => self.app_server_ready(),
            AppServerEvent::Response { response, method } => {
                self.handle_response(method.as_deref(), &response.id, response.result)
            }
            AppServerEvent::ErrorResponse { response, method } => {
                self.handle_history_error(&response.id, method.as_deref(), &response.error.message);
                if self.handle_tool_error(&response.id, &response.error.message) {
                    return;
                }
                let operation = method.as_deref().unwrap_or("request");
                self.push_error(format!("{operation} failed: {}", response.error.message));
                match method.as_deref() {
                    Some("thread/start") => self.fail(response.error.message),
                    Some("thread/resume") | Some("thread/fork") => {}
                    Some("turn/start") | Some("review/start") => {
                        self.active_turn_id.borrow_mut().take();
                        self.clear_pending_requests();
                        self.view.set_turn_active(false);
                        self.view.set_connection_status(ChatConnectionStatus::Ready);
                        self.view.set_composer_enabled(true);
                        self.set_state(AppChatState::Ready);
                    }
                    _ => {}
                }
            }
            AppServerEvent::ServerRequest(request) => self.handle_server_request(request),
            AppServerEvent::Notification(notification) => {
                self.handle_notification(&notification.method, notification.params)
            }
            AppServerEvent::Diagnostic(message) => {
                log::debug!(
                    "Codex App Server diagnostic session_id={} bytes={}",
                    self.id,
                    message.len()
                );
            }
            AppServerEvent::ProtocolError(message) => {
                log::warn!(
                    "Codex App Server protocol error session_id={}: {message}",
                    self.id
                );
                self.push_error(message);
            }
            AppServerEvent::ProcessExited(status) => self.process_exited(status),
        }
    }

    fn app_server_ready(&self) {
        log::info!(
            "Codex App Server ready; creating thread session_id={}",
            self.id
        );
        self.set_state(AppChatState::StartingThread);
        self.view
            .set_connection_status(ChatConnectionStatus::Initializing);
        {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return;
            };
            let _ = server.send_raw_request("model/list", Some(json!({})));
            let _ = server.send_raw_request(
                "config/read",
                Some(json!({ "includeLayers": false, "cwd": self.workspace_root })),
            );
            let _ = server.send_raw_request(
                "permissionProfile/list",
                Some(json!({ "cwd": self.workspace_root })),
            );
            let _ = server.send_raw_request("collaborationMode/list", Some(json!({})));
        }
        let picker_visible = self.content.visible_child_name().as_deref() == Some("threads");
        if picker_visible {
            self.load_thread_page(false);
        }
        if let Some(thread_id) = self.resume_thread_id.borrow_mut().take() {
            self.send_thread_operation(
                "thread/resume",
                &thread_id,
                json!({
                    "excludeTurns": true,
                    "initialTurnsPage": {
                        "limit": 100,
                        "sortDirection": "desc",
                        "itemsView": "full"
                    }
                }),
            );
            return;
        }
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        if let Err(error) = server.thread_start(ThreadStartParams {
            cwd: Some(self.workspace_root.clone()),
            ..Default::default()
        }) {
            self.fail(error.to_string());
        }
    }

    fn handle_response(&self, method: Option<&str>, request_id: &RequestId, result: Value) {
        if self.handle_tool_response(request_id, &result) {
            return;
        }
        match method {
            Some("thread/start") | Some("thread/resume") | Some("thread/fork") => {
                self.thread_operations.borrow_mut().remove(request_id);
                if method != Some("thread/start") {
                    self.load_thread_history(&result);
                }
                self.thread_became_ready(&result);
                if method != Some("thread/start")
                    || self.content.visible_child_name().as_deref() != Some("threads")
                {
                    self.hide_thread_picker();
                }
            }
            Some("thread/read") | Some("thread/rollback") => {
                self.active_turn_id.borrow_mut().take();
                self.load_thread_history(&result);
                self.thread_became_ready(&result);
                self.hide_thread_picker();
            }
            Some("thread/list") => self.apply_thread_list(request_id, &result),
            Some("thread/turns/list") => self.apply_older_turns(request_id, &result),
            Some("model/list") => self.apply_model_catalog(&result),
            Some("config/read") => self.apply_config_defaults(&result),
            Some("permissionProfile/list") => self.apply_permission_profiles(&result),
            Some("collaborationMode/list") => self.apply_collaboration_modes(&result),
            Some("turn/start") | Some("review/start") => {
                if method == Some("review/start")
                    && let Some(review_thread_id) =
                        result.get("reviewThreadId").and_then(Value::as_str)
                {
                    self.prepare_thread_switch();
                    self.send_thread_operation(
                        "thread/resume",
                        review_thread_id,
                        json!({
                            "excludeTurns": true,
                            "initialTurnsPage": {
                                "limit": 100,
                                "sortDirection": "desc",
                                "itemsView": "full"
                            }
                        }),
                    );
                    return;
                }
                if let Some(turn_id) = result.pointer("/turn/id").and_then(Value::as_str) {
                    self.active_turn_id.replace(Some(turn_id.to_owned()));
                }
                self.view.set_turn_steerable(method == Some("turn/start"));
                self.set_state(AppChatState::Running);
                self.view.set_turn_active(true);
            }
            Some("thread/compact/start") => self.view.set_turn_steerable(false),
            Some("thread/archive")
            | Some("thread/unarchive")
            | Some("thread/delete")
            | Some("thread/name/set")
            | Some("thread/metadata/update") => {
                self.apply_thread_operation_response(request_id);
            }
            _ => {}
        }
    }

    fn thread_became_ready(&self, result: &Value) {
        let Some(thread_id) = result.pointer("/thread/id").and_then(Value::as_str) else {
            self.fail("Codex thread response did not contain a thread id".to_owned());
            return;
        };
        self.thread_id.replace(Some(thread_id.to_owned()));
        if self.active_turn_id.borrow().is_some() {
            self.view.set_turn_steerable(
                result
                    .pointer("/thread/canAcceptDirectInput")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            );
            self.set_state(AppChatState::Running);
            self.view.set_turn_active(true);
        } else {
            self.set_state(AppChatState::Ready);
            self.view.set_turn_active(false);
        }
        self.view.set_connection_status(ChatConnectionStatus::Ready);
        self.view.set_composer_enabled(true);
        if let Some(name) = result
            .pointer("/thread/name")
            .and_then(Value::as_str)
            .or_else(|| result.pointer("/thread/preview").and_then(Value::as_str))
            .filter(|name| !name.trim().is_empty())
        {
            self.set_title(name);
        }
        for (selector, pointer) in [
            (ChatSelector::Model, "/model"),
            (ChatSelector::Reasoning, "/reasoningEffort"),
            (ChatSelector::ServiceTier, "/serviceTier"),
            (ChatSelector::ApprovalReviewer, "/approvalsReviewer"),
            (ChatSelector::Permissions, "/activePermissionProfile/id"),
        ] {
            if let Some(value) = result.pointer(pointer).and_then(Value::as_str) {
                self.selected_values
                    .borrow_mut()
                    .insert(selector, value.to_owned());
            }
        }
        if result.get("serviceTier").is_some_and(Value::is_null) {
            self.selected_values.borrow_mut().insert(
                ChatSelector::ServiceTier,
                DEFAULT_SERVICE_TIER_ID.to_owned(),
            );
        }
        self.update_reasoning_options();
        self.update_service_tier_options();
        self.update_personality_options();
        self.persist_overlay(None);
    }

    fn process_exited(&self, status: ExitStatus) {
        if self.closing.get() {
            self.set_state(AppChatState::Closed);
            return;
        }
        self.fail(format!(
            "Codex App Server exited{}",
            status
                .code
                .map(|code| format!(" with status {code}"))
                .unwrap_or_default()
        ));
    }

    fn handle_action(self: &Rc<Self>, action: CodexChatAction) {
        match action {
            CodexChatAction::Submit(submission) => {
                self.submit(submission, false);
            }
            CodexChatAction::Steer(submission) => {
                self.submit(submission, true);
            }
            CodexChatAction::Queue(submission) => self.queue_submission(submission),
            CodexChatAction::EditQueued(id) => self.edit_queued_submission(&id),
            CodexChatAction::RemoveQueued(id) => self.remove_queued_submission(&id),
            CodexChatAction::MoveQueued { id, direction } => {
                self.move_queued_submission(&id, direction)
            }
            CodexChatAction::Interrupt => self.interrupt(),
            CodexChatAction::PastedClipboardImage { png_bytes } => {
                self.add_pasted_clipboard_image(&png_bytes)
            }
            CodexChatAction::FilesDropped(paths) => {
                for path in paths {
                    self.add_attachment(path, false);
                }
            }
            CodexChatAction::ChooseAttachment => {
                self.choose_context(ComposerContextSelection::Media)
            }
            CodexChatAction::ChooseMention => {
                self.choose_context(ComposerContextSelection::WorkspaceFile)
            }
            CodexChatAction::ChooseMentionFolder => {
                self.choose_context(ComposerContextSelection::WorkspaceFolder)
            }
            CodexChatAction::SelectorChanged { selector, value } => {
                self.update_selector(selector, value)
            }
            CodexChatAction::LoadOlderTurns => self.load_older_turns(),
            CodexChatAction::ShowThreadGoal => self.prompt_thread_goal(),
            CodexChatAction::RunShellCommand => self.prompt_shell_command(),
            CodexChatAction::ShowBackgroundTerminals => self.load_background_terminals(),
            CodexChatAction::ShowSkills => self.load_skills(),
            CodexChatAction::ShowMcpServers => self.load_mcp_servers(),
            CodexChatAction::ShowApps => self.load_apps(),
            CodexChatAction::ShowPlugins => self.load_plugins(),
            CodexChatAction::ShowExperimentalFeatures => self.load_experimental_features(),
            CodexChatAction::ShowAccountUsage => self.load_account_usage(),
            CodexChatAction::ResolveRequest {
                request_id,
                response,
            } => self.resolve_request(&request_id, response),
            CodexChatAction::ArchiveThread => self.thread_command("thread/archive", json!({})),
            CodexChatAction::CompactThread => {
                self.thread_command("thread/compact/start", json!({}))
            }
            CodexChatAction::StartReview => self.prompt_review(),
            CodexChatAction::UndoLastTurn => {
                self.thread_command("thread/rollback", json!({ "numTurns": 1 }))
            }
            CodexChatAction::ShowHistory => self.show_thread_picker(),
            CodexChatAction::ForkThread => {
                if let Some(thread_id) = self.thread_id.borrow().clone() {
                    self.prepare_thread_switch();
                    self.send_thread_operation("thread/fork", &thread_id, json!({}));
                }
            }
            CodexChatAction::OpenChanges => {
                self.ctx.dispatch_command(PageCommand::ShowChanges);
            }
            CodexChatAction::OpenLink(target) => {
                link_activation::activate(&self.ctx, &self.workspace_root, target)
            }
            CodexChatAction::AttachmentRemoved(attachment_id) => {
                self.remove_temporary_attachment(&attachment_id)
            }
        }
    }

    fn prompt_review(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::builder()
            .heading("Start Code Review")
            .body("Choose what Codex should review and where the review should run.")
            .build();
        let target_labels = gtk::StringList::new(&[
            "Uncommitted changes",
            "Against base branch",
            "Commit",
            "Custom instructions",
        ]);
        let target = gtk::DropDown::builder().model(&target_labels).build();
        let value = gtk::Entry::builder()
            .placeholder_text("No additional value required")
            .sensitive(false)
            .hexpand(true)
            .build();
        target.connect_selected_notify({
            let value = value.clone();
            move |target| match target.selected() {
                0 => {
                    value.set_sensitive(false);
                    value.set_placeholder_text(Some("No additional value required"));
                }
                1 => {
                    value.set_sensitive(true);
                    value.set_placeholder_text(Some("Base branch, for example main"));
                }
                2 => {
                    value.set_sensitive(true);
                    value.set_placeholder_text(Some("Commit SHA"));
                }
                _ => {
                    value.set_sensitive(true);
                    value.set_placeholder_text(Some("What should Codex review?"));
                }
            }
        });
        let delivery_labels = gtk::StringList::new(&["Inline", "Detached thread"]);
        let delivery = gtk::DropDown::builder()
            .model(&delivery_labels)
            .selected(0)
            .build();
        let fields = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        fields.append(
            &gtk::Label::builder()
                .label("Review target")
                .xalign(0.0)
                .build(),
        );
        fields.append(&target);
        fields.append(&value);
        fields.append(&gtk::Label::builder().label("Delivery").xalign(0.0).build());
        fields.append(&delivery);
        dialog.set_extra_child(Some(&fields));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("start", "Start Review");
        dialog.set_default_response(Some("start"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "start" {
                    return;
                }
                let input = value.text().trim().to_owned();
                let review_target = match target.selected() {
                    0 => json!({ "type": "uncommittedChanges" }),
                    1 if !input.is_empty() => json!({ "type": "baseBranch", "branch": input }),
                    2 if !input.is_empty() => {
                        json!({ "type": "commit", "sha": input, "title": null })
                    }
                    3 if !input.is_empty() => {
                        json!({ "type": "custom", "instructions": input })
                    }
                    _ => return,
                };
                if let Some(session) = weak.upgrade() {
                    session.thread_command(
                        "review/start",
                        json!({
                            "target": review_target,
                            "delivery": if delivery.selected() == 1 { "detached" } else { "inline" }
                        }),
                    );
                }
            },
        );
    }

    fn queue_submission(&self, submission: ComposerSubmission) {
        if submission_inputs(&submission).is_empty() {
            return;
        }
        self.queued_turns.borrow_mut().push(QueuedTurn {
            id: self.next_id("queued-turn"),
            submission,
        });
        self.sync_queued_submissions();
    }

    fn edit_queued_submission(&self, id: &str) {
        let Some(index) = self
            .queued_turns
            .borrow()
            .iter()
            .position(|queued| queued.id == id)
        else {
            return;
        };
        let queued = self.queued_turns.borrow_mut().remove(index);
        self.sync_queued_submissions();
        self.view.restore_submission_for_editing(queued.submission);
    }

    fn remove_queued_submission(&self, id: &str) {
        let Some(index) = self
            .queued_turns
            .borrow()
            .iter()
            .position(|queued| queued.id == id)
        else {
            return;
        };
        let queued = self.queued_turns.borrow_mut().remove(index);
        for attachment in queued.submission.attachments {
            self.remove_temporary_attachment(&attachment.id);
        }
        self.sync_queued_submissions();
    }

    fn move_queued_submission(&self, id: &str, direction: QueueDirection) {
        let Some(index) = self
            .queued_turns
            .borrow()
            .iter()
            .position(|queued| queued.id == id)
        else {
            return;
        };
        let destination = match direction {
            QueueDirection::Up => index.checked_sub(1),
            QueueDirection::Down => {
                (index + 1 < self.queued_turns.borrow().len()).then_some(index + 1)
            }
        };
        let Some(destination) = destination else {
            return;
        };
        self.queued_turns.borrow_mut().swap(index, destination);
        self.sync_queued_submissions();
    }

    fn sync_queued_submissions(&self) {
        let queued = self
            .queued_turns
            .borrow()
            .iter()
            .map(|queued| {
                let full_preview = submission_display_text(&queued.submission)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut preview = full_preview.chars().take(120).collect::<String>();
                if preview.chars().count() < full_preview.chars().count() {
                    preview.push('…');
                }
                QueuedSubmission {
                    id: queued.id.clone(),
                    preview,
                }
            })
            .collect::<Vec<_>>();
        self.view.set_queued_submissions(&queued);
    }

    pub(super) fn submit_next_queued(&self) {
        if self.active_turn_id.borrow().is_some() || !self.pending_requests.borrow().is_empty() {
            return;
        }
        let queued = {
            let mut turns = self.queued_turns.borrow_mut();
            if turns.is_empty() {
                return;
            }
            turns.remove(0)
        };
        self.sync_queued_submissions();
        if !self.submit(queued.submission.clone(), false) {
            self.queued_turns.borrow_mut().insert(0, queued);
            self.sync_queued_submissions();
        }
    }

    fn submit(&self, submission: ComposerSubmission, steer: bool) -> bool {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return false;
        };
        let input = submission_inputs(&submission);
        if input.is_empty() {
            return false;
        }
        let client_id = self.next_id("user");
        let result = {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return false;
            };
            if steer {
                let Some(expected_turn_id) = self.active_turn_id.borrow().clone() else {
                    self.push_error("There is no active turn to steer".to_owned());
                    return false;
                };
                server.turn_steer(TurnSteerParams {
                    thread_id,
                    client_user_message_id: Some(client_id.clone()),
                    input,
                    expected_turn_id,
                    // `turn/steer` accepts input for the in-flight turn, not
                    // the model/personality/permission overrides used by
                    // `turn/start`.
                    extra: Default::default(),
                })
            } else {
                let mut extra = self.turn_settings();
                server.turn_start(TurnStartParams {
                    thread_id,
                    client_user_message_id: Some(client_id.clone()),
                    input,
                    cwd: None,
                    permissions: extra
                        .remove("permissions")
                        .and_then(|value| value.as_str().map(str::to_owned)),
                    model: extra
                        .remove("model")
                        .and_then(|value| value.as_str().map(str::to_owned)),
                    extra,
                })
            }
        };
        if let Err(error) = result {
            self.push_error(error.to_string());
            return false;
        }

        let body = submission_display_text(&submission);
        self.upsert_timeline(TimelineItem {
            id: client_id,
            kind: TimelineItemKind::UserMessage,
            title: None,
            body,
            detail: None,
            status: TimelineItemStatus::Completed,
        });
        self.view.scroll_transcript_to_end();
        if !steer
            && self.title.borrow().as_str() == "New Codex chat"
            && let Some(title) = concise_title(&submission.text)
        {
            self.set_title(&title);
            self.persist_overlay(Some(title));
        }
        self.set_state(AppChatState::Running);
        if !steer {
            self.view.set_turn_steerable(true);
        }
        self.view.set_turn_active(true);
        true
    }

    fn interrupt(&self) {
        let (Some(thread_id), Some(turn_id)) = (
            self.thread_id.borrow().clone(),
            self.active_turn_id.borrow().clone(),
        ) else {
            return;
        };
        if let Some(server) = self.server.borrow().as_ref()
            && let Err(error) = server.turn_interrupt(TurnInterruptParams { thread_id, turn_id })
        {
            self.push_error(error.to_string());
        }
    }

    fn shutdown(&self) {
        if self.closing.replace(true) {
            return;
        }
        log::info!(
            "shutting down native Codex session session_id={} thread_id={:?}",
            self.id,
            self.thread_id.borrow().as_deref()
        );
        self.set_state(AppChatState::Closing);
        self.view.set_composer_enabled(false);
        self.view
            .set_connection_status(ChatConnectionStatus::Disconnected);
        if let Some(source) = self.poll_source.borrow_mut().take() {
            source.remove();
        }
        self.clear_pending_requests();
        if let Some(mut server) = self.server.borrow_mut().take() {
            let session_id = self.id;
            if let Err(error) = std::thread::Builder::new()
                .name(format!("codex-app-session-stop-{session_id}"))
                .spawn(move || server.shutdown())
            {
                log::warn!(
                    "failed to start Codex shutdown worker session_id={session_id}: {error}"
                );
            }
        }
        self.startup.borrow_mut().take();
        self.set_state(AppChatState::Closed);
    }

    fn set_state(&self, state: AppChatState) {
        if *self.lifecycle.borrow() == state {
            return;
        }
        log::info!(
            "native Codex session lifecycle session_id={} {:?} -> {:?}",
            self.id,
            *self.lifecycle.borrow(),
            state
        );
        self.lifecycle.replace(state.clone());
        if let Some(callback) = self.state_callback.borrow().clone() {
            callback(self.id, state);
        }
    }

    fn fail(&self, message: String) {
        self.active_turn_id.borrow_mut().take();
        self.view.set_turn_active(false);
        self.view
            .set_connection_status(ChatConnectionStatus::Failed(message.clone()));
        self.view.set_composer_enabled(false);
        if self.content.visible_child_name().as_deref() == Some("threads") {
            self.picker.set_error(Some(&message));
        } else {
            self.content.set_visible_child_name("chat");
        }
        self.set_state(AppChatState::Failed(message));
        self.clear_pending_requests();
    }
}

impl AppChatSessionInner {
    fn handle_server_request(&self, request: Request) {
        if request
            .params
            .as_ref()
            .is_some_and(|params| self.targets_other_thread(params))
        {
            if let Some(server) = self.server.borrow().as_ref() {
                let _ = server.respond_error(
                    request.id,
                    RpcError {
                        code: -32600,
                        message: "request targets an inactive thread".to_owned(),
                        data: None,
                    },
                );
            }
            return;
        }
        if request.method == "currentTime/read" {
            let current_time_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            if let Some(server) = self.server.borrow().as_ref() {
                let _ = server.respond(request.id, json!({ "currentTimeAt": current_time_at }));
            }
            return;
        }
        let request_key = request_id_key(&request.id);
        let params = request.params.unwrap_or(Value::Null);
        let pending = pending_request_from_server(&request_key, &request.method, &params);
        self.pending_requests.borrow_mut().insert(
            request_key.clone(),
            PendingServerRequest {
                id: request.id,
                method: request.method,
                params,
            },
        );
        self.view.upsert_pending_request(pending);
        self.set_state(AppChatState::AwaitingInput);
    }

    fn resolve_request(&self, request_key: &str, response: PendingRequestResponse) {
        let Some(pending) = self.pending_requests.borrow_mut().remove(request_key) else {
            self.push_error("That Codex request is no longer pending".to_owned());
            self.view.resolve_pending_request(request_key);
            return;
        };
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        let response_result = match pending.method.as_str() {
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
            | "item/tool/call" => {
                let result =
                    match response_for_server_request(&pending.method, &pending.params, response) {
                        Ok(result) => result,
                        Err(message) => {
                            self.push_error(message);
                            let replacement = pending_request_from_server(
                                request_key,
                                &pending.method,
                                &pending.params,
                            );
                            self.pending_requests
                                .borrow_mut()
                                .insert(request_key.to_owned(), pending);
                            self.view.upsert_pending_request(replacement);
                            return;
                        }
                    };
                server.respond(pending.id.clone(), result)
            }
            _ => server.respond_error(
                pending.id.clone(),
                RpcError {
                    code: -32601,
                    message: format!("{} is not provided by this client", pending.method),
                    data: None,
                },
            ),
        };
        match response_result {
            Ok(_) => self.restore_state_after_pending_requests(),
            Err(error) => {
                self.push_error(error.to_string());
                let replacement =
                    pending_request_from_server(request_key, &pending.method, &pending.params);
                self.pending_requests
                    .borrow_mut()
                    .insert(request_key.to_owned(), pending);
                self.view.upsert_pending_request(replacement);
            }
        }
    }

    fn choose_context(self: &Rc<Self>, selection: ComposerContextSelection) {
        let dialog = gtk::FileDialog::builder()
            .title(match selection {
                ComposerContextSelection::Media => "Attach Image or Audio",
                ComposerContextSelection::WorkspaceFile => "Reference Workspace File",
                ComposerContextSelection::WorkspaceFolder => "Reference Workspace Folder",
            })
            .accept_label(match selection {
                ComposerContextSelection::Media => "Attach",
                ComposerContextSelection::WorkspaceFile
                | ComposerContextSelection::WorkspaceFolder => "Reference",
            })
            .modal(true)
            .build();
        if matches!(selection, ComposerContextSelection::Media) {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images and audio"));
            filter.add_mime_type("image/*");
            filter.add_mime_type("audio/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&filter));
        }
        let parent = self.view.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        let selected = move |result: Result<gio::File, glib::Error>| {
            let (Some(session), Ok(file)) = (weak.upgrade(), result) else {
                return;
            };
            let reference = file
                .path()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.uri().to_string());
            session.add_attachment(
                reference,
                !matches!(selection, ComposerContextSelection::Media),
            );
        };
        if matches!(selection, ComposerContextSelection::WorkspaceFolder) {
            dialog.select_folder(parent.as_ref(), None::<&gio::Cancellable>, selected);
        } else {
            dialog.open(parent.as_ref(), None::<&gio::Cancellable>, selected);
        }
    }

    fn add_attachment(&self, reference: String, mention: bool) {
        let label = Path::new(&reference)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or(&reference)
            .to_owned();
        let kind = if mention {
            ComposerAttachmentKind::Mention
        } else {
            attachment_kind(&reference)
        };
        self.view.add_attachment(ComposerAttachment {
            id: format!("{}:{reference}", if mention { "mention" } else { "file" }),
            label,
            kind,
            reference,
        });
    }

    fn add_pasted_clipboard_image(&self, png_bytes: &[u8]) {
        if png_bytes.is_empty() {
            self.push_error("The clipboard image was empty".to_owned());
            return;
        }
        let attachment_id = self.next_id("clipboard-image");
        let filename = format!(
            "craic-codex-clipboard-{}-{}-{}.png",
            std::process::id(),
            self.id,
            self.next_local_id.get()
        );
        let path = std::env::temp_dir().join(filename);
        if let Err(error) = std::fs::write(&path, png_bytes) {
            self.push_error(format!("Failed to save the pasted image: {error}"));
            return;
        }
        self.temporary_attachments
            .borrow_mut()
            .insert(attachment_id.clone(), path.clone());
        self.view.add_attachment(ComposerAttachment {
            id: attachment_id,
            label: "Pasted image".to_owned(),
            kind: ComposerAttachmentKind::Image,
            reference: path.to_string_lossy().into_owned(),
        });
    }

    fn remove_temporary_attachment(&self, attachment_id: &str) {
        let Some(path) = self
            .temporary_attachments
            .borrow_mut()
            .remove(attachment_id)
        else {
            return;
        };
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "failed removing temporary Codex attachment session_id={} path={}: {error}",
                self.id,
                path.display()
            );
        }
    }

    fn request_session_close(&self) {
        if let Some(callback) = self.close_callback.borrow().clone() {
            callback(self.id);
        }
    }

    fn set_title(&self, title: &str) {
        let title = title.trim();
        if title.is_empty() || self.title.borrow().as_str() == title {
            return;
        }
        self.title.replace(title.to_owned());
        if let Some(callback) = self.title_callback.borrow().clone() {
            callback(self.id, title.to_owned());
        }
    }

    fn upsert_timeline(&self, item: TimelineItem) {
        self.timeline
            .borrow_mut()
            .insert(item.id.clone(), item.clone());
        self.view.upsert_timeline_item(item);
    }

    fn push_error(&self, message: String) {
        self.upsert_timeline(TimelineItem {
            id: self.next_id("error"),
            kind: TimelineItemKind::Error,
            title: Some("Codex error".to_owned()),
            body: message,
            detail: None,
            status: TimelineItemStatus::Failed,
        });
    }

    fn push_warning(&self, title: &str, message: String) {
        self.upsert_timeline(TimelineItem {
            id: self.next_id("warning"),
            kind: TimelineItemKind::Warning,
            title: Some(title.to_owned()),
            body: message,
            detail: None,
            status: TimelineItemStatus::Completed,
        });
    }

    fn clear_pending_requests(&self) {
        for request_id in self
            .pending_requests
            .borrow_mut()
            .drain()
            .map(|(request_id, _)| request_id)
            .collect::<Vec<_>>()
        {
            self.view.resolve_pending_request(&request_id);
        }
        self.restore_state_after_pending_requests();
    }

    fn restore_state_after_pending_requests(&self) {
        if !self.pending_requests.borrow().is_empty()
            || *self.lifecycle.borrow() != AppChatState::AwaitingInput
        {
            return;
        }
        if self.active_turn_id.borrow().is_some() {
            self.set_state(AppChatState::Running);
        } else {
            self.set_state(AppChatState::Ready);
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        let id = self.next_local_id.get();
        self.next_local_id.set(id.saturating_add(1));
        format!("{prefix}:{}:{id}", self.id)
    }
}

impl Drop for AppChatSessionInner {
    fn drop(&mut self) {
        if let Some(source) = self.poll_source.get_mut().take() {
            source.remove();
        }
        self.startup.get_mut().take();
        if self.server.get_mut().is_some() {
            log::warn!(
                "native Codex session dropped before shutdown completed session_id={}",
                self.id
            );
        }
        // `AppServer::drop` initiates shutdown and detaches its worker handles. Holding the
        // value until this scope ends keeps this fallback non-blocking for GTK's main thread.
        let _server = self.server.get_mut().take();
        for path in self
            .temporary_attachments
            .get_mut()
            .drain()
            .map(|(_, path)| path)
        {
            if let Err(error) = std::fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!(
                    "failed removing temporary Codex attachment session_id={} path={}: {error}",
                    self.id,
                    path.display()
                );
            }
        }
    }
}

fn app_server_config(
    shell: &dyn ShellAccess,
    workspace: &WorkspaceRef,
    provider_kind: ProviderKind,
) -> Result<AppServerConfig, String> {
    let codex = shell
        .which("codex")?
        .ok_or_else(|| "Codex is not installed on this workspace target".to_owned())?;
    let app_args = vec![
        "app-server".to_owned(),
        "--listen".to_owned(),
        "stdio://".to_owned(),
    ];
    let app = shell.fast_command(&workspace.root, &codex, &app_args)?;
    let version = shell.fast_command(&workspace.root, &codex, &["--version".to_owned()])?;
    let mut config = AppServerConfig {
        program: app.program,
        args: app.args,
        cwd: (provider_kind == ProviderKind::Local)
            .then(|| PathBuf::from(app.working_dir.absolute)),
        version_command: Some((version.program, version.args)),
        ..AppServerConfig::default()
    };
    config.capabilities.experimental_api = true;
    config.capabilities.mcp_server_openai_form_elicitation = true;
    Ok(config)
}

fn submission_inputs(submission: &ComposerSubmission) -> Vec<UserInput> {
    let mut input = Vec::new();
    if !submission.text.trim().is_empty() {
        input.push(UserInput::text(submission.text.trim()));
    }
    for attachment in &submission.attachments {
        match attachment.kind {
            ComposerAttachmentKind::Image => input.push(UserInput::LocalImage {
                path: PathBuf::from(&attachment.reference),
                detail: None,
            }),
            ComposerAttachmentKind::Audio => input.push(UserInput::LocalAudio {
                path: PathBuf::from(&attachment.reference),
            }),
            ComposerAttachmentKind::Skill => input.push(UserInput::Skill {
                name: attachment.label.clone(),
                path: PathBuf::from(&attachment.reference),
            }),
            ComposerAttachmentKind::File | ComposerAttachmentKind::Mention => {
                input.push(UserInput::Mention {
                    name: attachment.label.clone(),
                    path: attachment.reference.clone(),
                })
            }
        }
    }
    input
}

fn submission_display_text(submission: &ComposerSubmission) -> String {
    let mut lines = Vec::new();
    if !submission.text.trim().is_empty() {
        lines.push(submission.text.trim().to_owned());
    }
    lines.extend(
        submission
            .attachments
            .iter()
            .map(|attachment| format!("[{}]", attachment.label)),
    );
    lines.join("\n")
}

use super::codex_requests::{pending_request_from_server, response_for_server_request};

fn request_id_key(id: &RequestId) -> String {
    match id {
        RequestId::Integer(id) => format!("integer:{id}"),
        RequestId::String(id) => format!("string:{id}"),
    }
}

fn compact_json(value: &Value) -> String {
    let mut rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if rendered.len() > MAX_RENDERED_JSON_BYTES {
        let mut boundary = MAX_RENDERED_JSON_BYTES;
        while !rendered.is_char_boundary(boundary) {
            boundary -= 1;
        }
        rendered.truncate(boundary);
        rendered.push_str("\n… output truncated …");
    }
    rendered
}

fn concise_title(prompt: &str) -> Option<String> {
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return None;
    }
    let mut title = prompt.chars().take(72).collect::<String>();
    if title.len() < prompt.len() {
        title.push('…');
    }
    Some(title)
}

fn attachment_kind(reference: &str) -> ComposerAttachmentKind {
    let extension = Path::new(reference)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    ) {
        ComposerAttachmentKind::Image
    } else if matches!(extension.as_str(), "wav" | "mp3" | "m4a" | "ogg" | "flac") {
        ComposerAttachmentKind::Audio
    } else {
        ComposerAttachmentKind::File
    }
}

fn title_case(value: &str) -> String {
    let words = value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>();
    words.join(" ")
}
