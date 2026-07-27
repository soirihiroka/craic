use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

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

use self::settings::{DEFAULT_SERVICE_TIER_ID, ModelServiceTiers, set_initial_selector_options};

use super::super::{PageCommand, PageContext};
use super::codex_chat::{
    ChatConnectionStatus, ChatSelector, CodexChatAction, CodexChatView, CollaborationParticipant,
    CollaborationParticipantStatus, CollaborationProgress, ComposerAttachment,
    ComposerAttachmentKind, ComposerSubmission, PendingRequestResponse, PlanProgress, PlanStep,
    PlanStepStatus, SelectorOption, TimelineItem, TimelineItemKind, TimelineItemStatus, TokenUsage,
};
use super::thread_picker::{
    CodexThreadPicker, ThreadPickerAction, ThreadPickerRow, ThreadPickerSort,
};
use crate::system::ProviderKind;
use crate::ui::agent_history::{self, CodexThreadOverlay, CodexThreadOverlayUpsert};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const MAX_EVENTS_PER_POLL: usize = 256;
const MAX_RENDERED_JSON_BYTES: usize = 24 * 1024;

type TitleCallback = Rc<dyn Fn(u64, String)>;
type StateCallback = Rc<dyn Fn(u64, AppChatState)>;
type SessionCallback = Rc<dyn Fn(u64)>;

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

struct PickerRequest {
    query: String,
    append: bool,
    archived: bool,
    sort: ThreadPickerSort,
    cursor: Option<String>,
    generation: u64,
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
    active_turn_id: RefCell<Option<String>>,
    title: RefCell<String>,
    timeline: RefCell<HashMap<String, TimelineItem>>,
    pending_requests: RefCell<HashMap<String, PendingServerRequest>>,
    selected_values: RefCell<HashMap<ChatSelector, String>>,
    dirty_selectors: RefCell<HashSet<ChatSelector>>,
    model_reasoning: RefCell<HashMap<String, Vec<SelectorOption>>>,
    model_service_tiers: RefCell<HashMap<String, ModelServiceTiers>>,
    model_supports_personality: RefCell<HashMap<String, bool>>,
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
    next_local_id: Cell<u64>,
    closing: Cell<bool>,
    title_callback: RefCell<Option<TitleCallback>>,
    state_callback: RefCell<Option<StateCallback>>,
    close_callback: RefCell<Option<SessionCallback>>,
    history_callback: RefCell<Option<SessionCallback>>,
}

impl AppChatSession {
    pub(crate) fn new(id: u64, ctx: PageContext) -> Result<Self, String> {
        let config = app_server_config(&ctx)?;
        let view = CodexChatView::new();
        let picker = CodexThreadPicker::new();
        view.set_connection_status(ChatConnectionStatus::Connecting);
        view.set_composer_enabled(false);
        set_initial_selector_options(&view);

        let workspace = ctx.workspace_ref();
        let content = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        content.add_named(&view.root, Some("chat"));
        content.add_named(&picker.root, Some("threads"));
        content.set_visible_child_name("chat");
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&content);
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("codex-app-session-start-{id}"))
            .spawn(move || {
                let result = AppServer::spawn(config).map_err(|error| error.to_string());
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
            active_turn_id: RefCell::new(None),
            title: RefCell::new("New Codex chat".to_owned()),
            timeline: RefCell::new(HashMap::new()),
            pending_requests: RefCell::new(HashMap::new()),
            selected_values: RefCell::new(HashMap::new()),
            dirty_selectors: RefCell::new(HashSet::new()),
            model_reasoning: RefCell::new(HashMap::new()),
            model_service_tiers: RefCell::new(HashMap::new()),
            model_supports_personality: RefCell::new(HashMap::new()),
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
            next_local_id: Cell::new(1),
            closing: Cell::new(false),
            title_callback: RefCell::new(None),
            state_callback: RefCell::new(None),
            close_callback: RefCell::new(None),
            history_callback: RefCell::new(None),
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
        if self.inner.content.visible_child_name().as_deref() == Some("threads") {
            self.inner.picker.focus_search();
        } else {
            self.inner.view.focus_composer();
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

    fn connect_picker_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.picker.connect_action(move |action| {
            if let Some(session) = weak.upgrade() {
                session.handle_picker_action(action);
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
                self.thread_operations.borrow_mut().remove(&response.id);
                if self.handle_tool_error(&response.id, &response.error.message) {
                    return;
                }
                let operation = method.as_deref().unwrap_or("request");
                self.push_error(format!("{operation} failed: {}", response.error.message));
                if method.as_deref() == Some("thread/turns/list")
                    && self.turns_request.borrow().as_ref() == Some(&response.id)
                {
                    self.turns_request.borrow_mut().take();
                    self.view.set_older_turns_loading(false);
                }
                if method.as_deref() == Some("thread/list") {
                    let request = self.picker_requests.borrow_mut().remove(&response.id);
                    if request.is_some_and(|request| {
                        request.generation == self.picker_generation.get()
                            && request.query == self.picker.query()
                            && request.archived == self.picker.archived_only()
                            && request.sort == self.picker.sort()
                            && request.cursor == *self.picker_cursor.borrow()
                    }) {
                        self.picker.set_error(Some(&response.error.message));
                    }
                }
                match method.as_deref() {
                    Some("thread/start") => self.fail(response.error.message),
                    Some("thread/resume") | Some("thread/fork") => {
                        self.content.set_visible_child_name("threads");
                        self.picker.set_loading(false);
                        self.picker.set_error(Some(&response.error.message));
                    }
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
                self.hide_thread_picker();
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
                self.set_state(AppChatState::Running);
                self.view.set_turn_active(true);
            }
            Some("thread/archive")
            | Some("thread/unarchive")
            | Some("thread/delete")
            | Some("thread/name/set")
            | Some("thread/metadata/update") => {
                if let Some((operation, thread_id)) =
                    self.thread_operations.borrow_mut().remove(request_id)
                    && operation == "thread/delete"
                    && let Err(error) =
                        agent_history::delete_codex_thread_overlay(&self.workspace_key, &thread_id)
                {
                    log::warn!(
                        "failed deleting Codex thread overlay session_id={} thread_id={}: {error}",
                        self.id,
                        thread_id
                    );
                }
                self.load_thread_page(false);
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
            CodexChatAction::Submit(submission) => self.submit(submission, false),
            CodexChatAction::Steer(submission) => self.submit(submission, true),
            CodexChatAction::Interrupt => self.interrupt(),
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
            CodexChatAction::AttachmentRemoved(_) => {}
        }
    }

    fn handle_picker_action(self: &Rc<Self>, action: ThreadPickerAction) {
        match action {
            ThreadPickerAction::SearchChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::ArchivedChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::SortChanged(_) => self.load_thread_page(false),
            ThreadPickerAction::LoadMore => self.load_thread_page(true),
            ThreadPickerAction::Resume(thread_id) => {
                self.prepare_thread_switch();
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
            }
            ThreadPickerAction::Fork(thread_id) => {
                self.prepare_thread_switch();
                self.send_thread_operation("thread/fork", &thread_id, json!({}));
            }
            ThreadPickerAction::Rename {
                thread_id,
                current_name,
            } => self.prompt_thread_rename(thread_id, current_name),
            ThreadPickerAction::EditTags { thread_id, tags } => {
                self.prompt_thread_tags(thread_id, tags)
            }
            ThreadPickerAction::Archive(thread_id) => {
                self.send_thread_operation("thread/archive", &thread_id, json!({}));
            }
            ThreadPickerAction::Unarchive(thread_id) => {
                self.send_thread_operation("thread/unarchive", &thread_id, json!({}));
            }
            ThreadPickerAction::Delete(thread_id) => {
                self.confirm_thread_delete(thread_id);
            }
            ThreadPickerAction::Pin(thread_id) => self.send_thread_operation(
                "thread/metadata/update",
                &thread_id,
                json!({ "isPinned": true }),
            ),
            ThreadPickerAction::Unpin(thread_id) => self.send_thread_operation(
                "thread/metadata/update",
                &thread_id,
                json!({ "isPinned": false }),
            ),
            ThreadPickerAction::Cancel => {
                if self.thread_id.borrow().is_some() {
                    self.hide_thread_picker();
                }
            }
        }
    }

    fn prompt_thread_rename(self: &Rc<Self>, thread_id: String, current_name: String) {
        let dialog = adw::AlertDialog::builder()
            .heading("Rename Codex Thread")
            .body("Choose the name shown in Codex thread history.")
            .build();
        let entry = gtk::Entry::builder()
            .text(&current_name)
            .placeholder_text("Thread name")
            .activates_default(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "rename" {
                    return;
                }
                let name = entry.text().trim().to_owned();
                if name.is_empty() {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.send_thread_operation(
                        "thread/name/set",
                        &thread_id,
                        json!({ "name": name }),
                    );
                }
            },
        );
    }

    fn prompt_thread_tags(self: &Rc<Self>, thread_id: String, tags: Vec<String>) {
        let dialog = adw::AlertDialog::builder()
            .heading("Edit Thread Tags")
            .body("Separate tags with commas. Tags are stored locally for this workspace.")
            .build();
        let entry = gtk::Entry::builder()
            .text(tags.join(", "))
            .placeholder_text("bug, frontend, follow-up")
            .activates_default(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "save" {
                    return;
                }
                let tags = entry
                    .text()
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let Some(session) = weak.upgrade() else {
                    return;
                };
                match agent_history::update_codex_thread_overlay_tags(
                    &session.workspace_key,
                    &thread_id,
                    &tags,
                ) {
                    Ok(_) => {
                        session.load_thread_page(false);
                        if let Some(callback) = session.history_callback.borrow().clone() {
                            callback(session.id);
                        }
                    }
                    Err(error) => session.picker.set_error(Some(&error)),
                }
            },
        );
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

    fn confirm_thread_delete(self: &Rc<Self>, thread_id: String) {
        let dialog = adw::AlertDialog::builder()
            .heading("Delete Codex Thread?")
            .body("This permanently deletes the thread and its local history metadata.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete Thread");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "delete" {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.send_thread_operation("thread/delete", &thread_id, json!({}));
                }
            },
        );
    }

    fn submit(&self, submission: ComposerSubmission, steer: bool) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return;
        };
        let input = submission_inputs(&submission);
        if input.is_empty() {
            return;
        }
        let client_id = self.next_id("user");
        let mut extra = self.turn_settings();
        let result = {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return;
            };
            if steer {
                let Some(expected_turn_id) = self.active_turn_id.borrow().clone() else {
                    self.push_error("There is no active turn to steer".to_owned());
                    return;
                };
                server.turn_steer(TurnSteerParams {
                    thread_id,
                    client_user_message_id: Some(client_id.clone()),
                    input,
                    expected_turn_id,
                    extra,
                })
            } else {
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
            return;
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
        if !steer && self.title.borrow().as_str() == "New Codex chat" {
            if let Some(title) = concise_title(&submission.text) {
                self.set_title(&title);
                self.persist_overlay(Some(title));
            }
        }
        self.set_state(AppChatState::Running);
        self.view.set_turn_active(true);
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
        self.set_state(AppChatState::Failed(message));
        self.clear_pending_requests();
    }
}

impl AppChatSessionInner {
    fn show_thread_picker(&self) {
        self.content.set_visible_child_name("threads");
        self.picker.set_query("");
        self.picker.focus_search();
        self.load_thread_page(false);
    }

    fn hide_thread_picker(&self) {
        self.content.set_visible_child_name("chat");
        self.view.focus_composer();
    }

    fn load_thread_page(&self, append: bool) {
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            self.picker
                .set_error(Some("Codex App Server is not connected."));
            return;
        };
        if !append {
            self.picker_cursor.borrow_mut().take();
        }
        let query = self.picker.query();
        let cursor = self.picker_cursor.borrow().clone();
        let archived = self.picker.archived_only();
        let sort = self.picker.sort();
        let sort_key = match sort {
            ThreadPickerSort::Updated => "updated_at",
            ThreadPickerSort::Created => "created_at",
        };
        let generation = self.picker_generation.get().wrapping_add(1);
        self.picker_generation.set(generation);
        self.picker.set_loading(true);
        let params = json!({
            "cursor": cursor,
            "limit": 50,
            "sortKey": sort_key,
            "sortDirection": "desc",
            "cwd": self.workspace_root,
            "searchTerm": (!query.is_empty()).then_some(query.clone()),
            "archived": archived
        });
        match server.send_raw_request("thread/list", Some(params)) {
            Ok(request_id) => {
                self.picker_requests.borrow_mut().insert(
                    request_id,
                    PickerRequest {
                        query,
                        append,
                        archived,
                        sort,
                        cursor,
                        generation,
                    },
                );
            }
            Err(error) => self.picker.set_error(Some(&error.to_string())),
        }
    }

    fn apply_thread_list(&self, request_id: &RequestId, result: &Value) {
        let Some(request) = self.picker_requests.borrow_mut().remove(request_id) else {
            return;
        };
        if request.generation != self.picker_generation.get()
            || self.picker.query() != request.query
            || self.picker.archived_only() != request.archived
            || self.picker.sort() != request.sort
            || *self.picker_cursor.borrow() != request.cursor
        {
            return;
        }
        let overlays =
            match agent_history::list_codex_thread_overlays(&self.workspace_key, 10_000, 0) {
                Ok(overlays) => overlays
                    .into_iter()
                    .map(|overlay| (overlay.thread_id.clone(), overlay))
                    .collect::<HashMap<_, _>>(),
                Err(error) => {
                    log::warn!(
                        "failed loading Codex thread overlays session_id={}: {error}",
                        self.id
                    );
                    HashMap::new()
                }
            };
        let rows = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|thread| self.thread_picker_row(thread, &overlays, request.archived))
            .collect::<Vec<_>>();
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let has_more = next_cursor.is_some();
        self.picker_cursor.replace(next_cursor);
        if request.append {
            self.picker.append_rows(rows, has_more);
        } else {
            self.picker.set_rows(rows, has_more);
        }
    }

    fn thread_picker_row(
        &self,
        thread: &Value,
        overlays: &HashMap<String, CodexThreadOverlay>,
        archived: bool,
    ) -> Option<ThreadPickerRow> {
        let thread_id = thread.get("id")?.as_str()?.to_owned();
        let overlay = overlays.get(&thread_id);
        let preview = thread
            .get("preview")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let title = thread
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| overlay.and_then(|overlay| overlay.task_description.clone()))
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| preview.clone());
        Some(ThreadPickerRow {
            thread_id,
            title,
            preview,
            model: thread
                .get("model")
                .or_else(|| thread.get("modelProvider"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            updated_at_ms: thread
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .saturating_mul(1_000),
            status: thread
                .pointer("/status/type")
                .and_then(Value::as_str)
                .map(title_case),
            tags: overlay
                .map(|overlay| overlay.tags.clone())
                .unwrap_or_default(),
            archived,
            pinned: thread
                .get("isPinned")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn send_thread_operation(&self, method: &str, thread_id: &str, extra: Value) {
        let mut params = extra.as_object().cloned().unwrap_or_default();
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        match server.send_raw_request(method, Some(Value::Object(params))) {
            Ok(request_id) => {
                self.thread_operations
                    .borrow_mut()
                    .insert(request_id, (method.to_owned(), thread_id.to_owned()));
            }
            Err(error) => {
                self.push_error(error.to_string());
                self.picker.set_error(Some(&error.to_string()));
            }
        }
    }

    fn prepare_thread_switch(&self) {
        if self.active_turn_id.borrow().is_some() {
            self.interrupt();
        }
        if let Some(thread_id) = self.thread_id.borrow_mut().take()
            && let Some(server) = self.server.borrow().as_ref()
        {
            let _ = server
                .send_raw_request("thread/unsubscribe", Some(json!({ "threadId": thread_id })));
        }
        self.active_turn_id.borrow_mut().take();
        self.set_state(AppChatState::StartingThread);
        self.timeline.borrow_mut().clear();
        self.clear_pending_requests();
        self.tool_requests.borrow_mut().clear();
        self.view.clear_timeline();
        self.view.set_usage(None);
        self.turns_cursor.borrow_mut().take();
        self.turns_request.borrow_mut().take();
        self.view.set_older_turns_available(false);
        self.view.set_older_turns_loading(false);
        self.view.set_turn_active(false);
        self.view.set_composer_enabled(false);
        self.set_title("New Codex chat");
        self.picker.set_loading(true);
    }

    fn load_thread_history(&self, result: &Value) {
        self.timeline.borrow_mut().clear();
        self.view.clear_timeline();
        let initial_page = result.get("initialTurnsPage");
        let turns = initial_page
            .and_then(|page| page.get("data"))
            .and_then(Value::as_array)
            .or_else(|| result.pointer("/thread/turns").and_then(Value::as_array));
        let mut turns = turns.cloned().unwrap_or_default();
        if initial_page.is_some() {
            turns.reverse();
        }
        let next_cursor = initial_page
            .and_then(|page| page.get("nextCursor"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.turns_cursor.replace(next_cursor.clone());
        self.view.set_older_turns_available(next_cursor.is_some());
        self.view.set_older_turns_loading(false);
        for turn in &turns {
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.upsert_timeline(timeline_from_item(item, true));
            }
            if turn.get("status").and_then(Value::as_str) == Some("inProgress") {
                if let Some(turn_id) = turn.get("id").and_then(Value::as_str) {
                    self.active_turn_id.replace(Some(turn_id.to_owned()));
                    self.view.set_turn_active(true);
                }
            }
        }
    }

    fn load_older_turns(&self) {
        if self.turns_request.borrow().is_some() {
            return;
        }
        let (Some(thread_id), Some(cursor)) = (
            self.thread_id.borrow().clone(),
            self.turns_cursor.borrow().clone(),
        ) else {
            self.view.set_older_turns_available(false);
            return;
        };
        let server = self.server.borrow();
        let Some(server) = server.as_ref() else {
            return;
        };
        self.view.set_older_turns_loading(true);
        match server.send_raw_request(
            "thread/turns/list",
            Some(json!({
                "threadId": thread_id,
                "cursor": cursor,
                "limit": 100,
                "sortDirection": "desc",
                "itemsView": "full"
            })),
        ) {
            Ok(request_id) => {
                self.turns_request.replace(Some(request_id));
            }
            Err(error) => {
                self.view.set_older_turns_loading(false);
                self.push_error(error.to_string());
            }
        }
    }

    fn apply_older_turns(&self, request_id: &RequestId, result: &Value) {
        if self.turns_request.borrow().as_ref() != Some(request_id) {
            return;
        }
        self.turns_request.borrow_mut().take();
        let mut turns = result
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        turns.reverse();
        let mut items = Vec::new();
        for turn in &turns {
            for item in turn
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let item = timeline_from_item(item, true);
                if !self.timeline.borrow().contains_key(&item.id) {
                    self.timeline
                        .borrow_mut()
                        .insert(item.id.clone(), item.clone());
                    items.push(item);
                }
            }
        }
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.turns_cursor.replace(next_cursor.clone());
        self.view.prepend_timeline_items(&items);
        self.view.set_older_turns_loading(false);
        self.view.set_older_turns_available(next_cursor.is_some());
    }

    fn handle_notification(&self, method: &str, params: Option<Value>) {
        let params = params.unwrap_or(Value::Null);
        if matches!(
            method,
            "thread/name/updated"
                | "thread/archived"
                | "thread/unarchived"
                | "thread/deleted"
                | "thread/started"
        ) && self.content.visible_child_name().as_deref() == Some("threads")
        {
            self.load_thread_page(false);
        }
        if self.targets_other_thread(&params) {
            return;
        }
        match method {
            "thread/name/updated" => {
                if let Some(name) = params.get("threadName").and_then(Value::as_str) {
                    self.set_title(name);
                    self.persist_overlay(Some(name.to_owned()));
                }
            }
            "thread/settings/updated" => self.apply_thread_settings(&params),
            "thread/status/changed" => match params.pointer("/status/type").and_then(Value::as_str)
            {
                Some("idle") => {
                    self.active_turn_id.borrow_mut().take();
                    self.clear_pending_requests();
                    self.view.set_turn_active(false);
                    self.set_state(AppChatState::Ready);
                }
                Some("active") => {
                    let waiting = params
                        .pointer("/status/activeFlags")
                        .and_then(Value::as_array)
                        .is_some_and(|flags| {
                            flags.iter().any(|flag| {
                                matches!(
                                    flag.as_str(),
                                    Some("waitingOnApproval" | "waitingOnUserInput")
                                )
                            })
                        });
                    self.view.set_turn_active(true);
                    self.set_state(if waiting {
                        AppChatState::AwaitingInput
                    } else {
                        AppChatState::Running
                    });
                }
                Some("systemError") => {
                    self.fail("Codex thread entered a system-error state".to_owned());
                }
                _ => {}
            },
            "turn/started" => {
                if let Some(turn_id) = params.pointer("/turn/id").and_then(Value::as_str) {
                    self.active_turn_id.replace(Some(turn_id.to_owned()));
                }
                self.set_state(AppChatState::Running);
                self.view.set_turn_active(true);
            }
            "turn/completed" => {
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                if status == "failed"
                    && let Some(message) = params
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                {
                    self.push_error(message.to_owned());
                }
                self.active_turn_id.borrow_mut().take();
                self.clear_pending_requests();
                self.view.set_turn_active(false);
                self.view.set_plan_progress(None);
                self.collaboration.borrow_mut().clear();
                self.view.set_collaboration_progress(None);
                self.set_state(AppChatState::Ready);
            }
            "item/started" => {
                if let Some(item) = params.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("contextCompaction") {
                        self.view.set_usage(None);
                    }
                    self.upsert_timeline(timeline_from_item(item, false));
                    self.update_collaboration_progress(item, false);
                }
            }
            "item/completed" => {
                if let Some(item) = params.get("item") {
                    self.upsert_timeline(timeline_from_item(item, true));
                    self.update_collaboration_progress(item, true);
                }
            }
            "item/agentMessage/delta"
            | "item/plan/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta" => self.append_delta(method, &params),
            "item/fileChange/patchUpdated" => self.apply_patch_snapshot(&params),
            "turn/diff/updated" => self.upsert_timeline(TimelineItem {
                id: format!(
                    "turn-diff:{}",
                    params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or("current")
                ),
                kind: TimelineItemKind::FileChange,
                title: Some("Turn changes".to_owned()),
                body: params
                    .get("diff")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                detail: None,
                status: TimelineItemStatus::Running,
            }),
            "turn/plan/updated" => self.apply_plan(&params),
            "thread/tokenUsage/updated" => self.apply_token_usage(&params),
            "serverRequest/resolved" => {
                if let Some(request_id) = params.get("requestId").and_then(request_id_key_value) {
                    self.pending_requests.borrow_mut().remove(&request_id);
                    self.view.resolve_pending_request(&request_id);
                    self.restore_state_after_pending_requests();
                }
            }
            "error" => {
                let message = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported an error");
                if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                    self.push_warning("Codex is retrying", message.to_owned());
                } else {
                    self.push_error(message.to_owned());
                }
            }
            "warning" | "guardianWarning" => self.push_warning(
                if method == "guardianWarning" {
                    "Guardian warning"
                } else {
                    "Codex warning"
                },
                params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported a warning")
                    .to_owned(),
            ),
            "deprecationNotice" | "configWarning" => {
                let title = params.get("summary").and_then(Value::as_str).unwrap_or(
                    if method == "configWarning" {
                        "Configuration warning"
                    } else {
                        "Deprecated Codex feature"
                    },
                );
                let details = params
                    .get("details")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.push_warning(title, details.to_owned());
            }
            "thread/compacted" => self.upsert_timeline(TimelineItem {
                id: self.next_id("compaction"),
                kind: TimelineItemKind::Compaction,
                title: Some("Context compacted".to_owned()),
                body: "Codex compacted this conversation's context.".to_owned(),
                detail: None,
                status: TimelineItemStatus::Completed,
            }),
            "thread/closed" | "thread/deleted" | "thread/archived" => {
                if self.notification_is_current_thread(&params) {
                    self.request_session_close();
                }
            }
            _ => {}
        }
    }

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

    fn append_delta(&self, method: &str, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return;
        }
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: delta_kind(method),
                title: delta_title(method),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        if matches!(
            method,
            "item/commandExecution/outputDelta"
                | "item/fileChange/outputDelta"
                | "item/reasoning/textDelta"
        ) {
            item.detail.get_or_insert_with(String::new).push_str(delta);
        } else {
            item.body.push_str(delta);
        }
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_patch_snapshot(&self, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let detail = params
            .get("patch")
            .or_else(|| params.get("changes"))
            .map(compact_json)
            .unwrap_or_default();
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: TimelineItemKind::FileChange,
                title: Some("File changes".to_owned()),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        item.detail = Some(detail);
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_plan(&self, params: &Value) {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let steps = params
            .get("plan")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| {
                        let marker = match step.get("status").and_then(Value::as_str) {
                            Some("completed") => "✓",
                            Some("inProgress") => "→",
                            _ => "·",
                        };
                        format!(
                            "{marker} {}",
                            step.get("step")
                                .and_then(Value::as_str)
                                .unwrap_or("Unnamed step")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        self.upsert_timeline(TimelineItem {
            id: format!("turn-plan:{turn_id}"),
            kind: TimelineItemKind::Plan,
            title: Some("Plan".to_owned()),
            body: steps,
            detail: params
                .get("explanation")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: TimelineItemStatus::Running,
        });
        let progress = params
            .get("plan")
            .and_then(Value::as_array)
            .map(|steps| PlanProgress {
                title: Some("Plan".to_owned()),
                summary: params
                    .get("explanation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                steps: steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| PlanStep {
                        id: format!("{turn_id}:{index}"),
                        label: step
                            .get("step")
                            .and_then(Value::as_str)
                            .unwrap_or("Unnamed step")
                            .to_owned(),
                        detail: None,
                        status: match step.get("status").and_then(Value::as_str) {
                            Some("inProgress") => PlanStepStatus::InProgress,
                            Some("completed") => PlanStepStatus::Completed,
                            Some("failed") => PlanStepStatus::Failed,
                            _ => PlanStepStatus::Pending,
                        },
                    })
                    .collect(),
            });
        self.view.set_plan_progress(progress);
    }

    fn apply_token_usage(&self, params: &Value) {
        let Some(total) = params.pointer("/tokenUsage/total") else {
            return;
        };
        let last = params.pointer("/tokenUsage/last").unwrap_or(total);
        self.view.set_usage(Some(TokenUsage {
            input_tokens: nonnegative_u64(total.get("inputTokens")),
            cache_write_input_tokens: nonnegative_u64(total.get("cacheWriteInputTokens")),
            cached_input_tokens: nonnegative_u64(total.get("cachedInputTokens")),
            output_tokens: nonnegative_u64(total.get("outputTokens")),
            reasoning_output_tokens: nonnegative_u64(total.get("reasoningOutputTokens")),
            total_tokens: nonnegative_u64(total.get("totalTokens")),
            last_total_tokens: nonnegative_u64(last.get("totalTokens")),
            context_limit: params
                .pointer("/tokenUsage/modelContextWindow")
                .and_then(Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
                .or(self.context_window_fallback.get()),
        }));
    }

    fn update_collaboration_progress(&self, item: &Value, completed: bool) {
        if item.get("type").and_then(Value::as_str) != Some("collabAgentToolCall") {
            return;
        }
        let Some(call_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let receivers = item
            .get("receiverThreadIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let agents_states = item.get("agentsStates").and_then(Value::as_object);
        for receiver in receivers.iter().copied().chain(
            agents_states
                .into_iter()
                .flat_map(|states| states.keys().map(String::as_str))
                .filter(|receiver| !receivers.contains(receiver)),
        ) {
            let state = agents_states.and_then(|states| states.get(receiver));
            let status_name = state
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str)
                .or_else(|| item.get("status").and_then(Value::as_str));
            let status = match status_name {
                Some("errored" | "failed" | "notFound") => CollaborationParticipantStatus::Failed,
                Some("completed" | "shutdown") if completed => {
                    CollaborationParticipantStatus::Completed
                }
                Some("running" | "inProgress") => CollaborationParticipantStatus::Working,
                _ if completed => CollaborationParticipantStatus::Completed,
                _ => CollaborationParticipantStatus::Pending,
            };
            let id = format!("{call_id}:{receiver}");
            let detail = state
                .and_then(|state| state.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    item.get("prompt")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| Some(format!("Thread {receiver}")));
            self.collaboration.borrow_mut().insert(
                id.clone(),
                CollaborationParticipant {
                    id,
                    label: item
                        .get("tool")
                        .and_then(Value::as_str)
                        .map(title_case)
                        .unwrap_or_else(|| "Subagent".to_owned()),
                    detail,
                    status,
                },
            );
        }
        let mut participants = self
            .collaboration
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        participants.sort_by(|left, right| left.id.cmp(&right.id));
        self.view
            .set_collaboration_progress(Some(CollaborationProgress {
                title: Some("Collaboration".to_owned()),
                participants,
            }));
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

    fn thread_command(&self, method: &str, extra: Value) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            return;
        };
        let mut params = extra.as_object().cloned().unwrap_or_default();
        params.insert("threadId".to_owned(), Value::String(thread_id));
        if let Some(server) = self.server.borrow().as_ref()
            && let Err(error) = server.send_raw_request(method, Some(Value::Object(params)))
        {
            self.push_error(error.to_string());
        }
    }

    fn request_session_close(&self) {
        if let Some(callback) = self.close_callback.borrow().clone() {
            callback(self.id);
        }
    }

    fn notification_is_current_thread(&self, params: &Value) -> bool {
        let Some(notified_thread_id) = params.get("threadId").and_then(Value::as_str) else {
            return false;
        };
        self.thread_id.borrow().as_deref() == Some(notified_thread_id)
    }

    fn targets_other_thread(&self, params: &Value) -> bool {
        let Some(notified_thread_id) = params.get("threadId").and_then(Value::as_str) else {
            return false;
        };
        match self.thread_id.borrow().as_deref() {
            Some(thread_id) => thread_id != notified_thread_id,
            None => *self.lifecycle.borrow() == AppChatState::StartingThread,
        }
    }

    fn persist_overlay(&self, task_description: Option<String>) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            return;
        };
        let existing = match agent_history::lookup_codex_thread_overlay(
            &self.workspace_key,
            &thread_id,
        ) {
            Ok(existing) => existing,
            Err(error) => {
                log::warn!(
                    "failed reading Codex thread overlay before update session_id={} thread_id={}: {error}",
                    self.id,
                    thread_id
                );
                return;
            }
        };
        let task_description = task_description
            .or_else(|| {
                (self.title.borrow().as_str() != "New Codex chat")
                    .then(|| self.title.borrow().clone())
            })
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|overlay| overlay.task_description.clone())
            });
        let tags = existing.map(|overlay| overlay.tags).unwrap_or_default();
        match agent_history::upsert_codex_thread_overlay(CodexThreadOverlayUpsert {
            thread_id,
            workspace_key: self.workspace_key.clone(),
            task_description,
            tags,
        }) {
            Ok(_) => {
                if let Some(callback) = self.history_callback.borrow().clone() {
                    callback(self.id);
                }
            }
            Err(error) => log::warn!(
                "failed to persist Codex thread overlay session_id={}: {error}",
                self.id
            ),
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
        if let Some(mut server) = self.server.get_mut().take() {
            server.shutdown();
        }
    }
}

fn app_server_config(ctx: &PageContext) -> Result<AppServerConfig, String> {
    let shell = ctx
        .shell()
        .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
    let workspace = ctx.workspace_ref();
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
    let mut config = AppServerConfig::default();
    config.program = app.program;
    config.args = app.args;
    config.cwd = (ctx.system_ref().provider_kind == ProviderKind::Local)
        .then(|| PathBuf::from(app.working_dir.absolute));
    config.version_command = Some((version.program, version.args));
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
            ComposerAttachmentKind::File
            | ComposerAttachmentKind::Mention => input.push(UserInput::Mention {
                name: attachment.label.clone(),
                path: attachment.reference.clone(),
            }),
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

fn timeline_from_item(item: &Value, completed: bool) -> TimelineItem {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let id = (item_type == "userMessage")
        .then(|| item.get("clientId").and_then(Value::as_str))
        .flatten()
        .or_else(|| item.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("unknown:{:016x}", stable_hash(&item.to_string())));
    let (kind, title, body, detail) = match item_type {
        "userMessage" => (
            TimelineItemKind::UserMessage,
            None,
            user_message_text(item.get("content")),
            None,
        ),
        "agentMessage" => (
            TimelineItemKind::AssistantMessage,
            None,
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "plan" => (
            TimelineItemKind::Plan,
            Some("Plan".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "reasoning" => (
            TimelineItemKind::Reasoning,
            Some("Reasoning".to_owned()),
            flattened_text(item.get("summary")),
            nonempty(flattened_text(item.get("content"))),
        ),
        "hookPrompt" => (
            TimelineItemKind::DeveloperMessage,
            Some("Hook prompt".to_owned()),
            item.get("fragments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|fragment| fragment.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            None,
        ),
        "commandExecution" => (
            TimelineItemKind::Command,
            Some("Command".to_owned()),
            item.get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("aggregatedOutput")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "fileChange" => (
            TimelineItemKind::FileChange,
            Some("File changes".to_owned()),
            file_change_summary(item),
            item.get("changes").map(compact_json),
        ),
        "mcpToolCall" => (
            TimelineItemKind::McpTool,
            Some(format!(
                "{} / {}",
                item.get("server").and_then(Value::as_str).unwrap_or("MCP"),
                item.get("tool").and_then(Value::as_str).unwrap_or("tool")
            )),
            item.get("arguments").map(compact_json).unwrap_or_default(),
            item.get("result")
                .or_else(|| item.get("error"))
                .map(compact_json),
        ),
        "dynamicToolCall" => (
            TimelineItemKind::Tool,
            Some(
                item.get("namespace")
                    .and_then(Value::as_str)
                    .filter(|namespace| !namespace.is_empty())
                    .map(|namespace| {
                        format!(
                            "{namespace} / {}",
                            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
                        )
                    })
                    .unwrap_or_else(|| {
                        item.get("tool")
                            .and_then(Value::as_str)
                            .unwrap_or("Dynamic tool")
                            .to_owned()
                    }),
            ),
            item.get("arguments").map(compact_json).unwrap_or_default(),
            item.get("contentItems").map(compact_json),
        ),
        "collabAgentToolCall" => (
            TimelineItemKind::Collaboration,
            Some("Collaboration".to_owned()),
            item.get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    item.get("tool")
                        .and_then(Value::as_str)
                        .map(title_case)
                        .unwrap_or_else(|| "Agent activity".to_owned())
                }),
            Some(compact_json(item)),
        ),
        "subAgentActivity" => (
            TimelineItemKind::Collaboration,
            Some(format!(
                "Subagent {}",
                item.get("kind")
                    .and_then(Value::as_str)
                    .map(title_case)
                    .unwrap_or_else(|| "activity".to_owned())
            )),
            item.get("agentPath")
                .and_then(Value::as_str)
                .unwrap_or("Subagent")
                .to_owned(),
            item.get("agentThreadId")
                .and_then(Value::as_str)
                .map(|thread_id| format!("Thread {thread_id}")),
        ),
        "webSearch" => (
            TimelineItemKind::Web,
            Some("Web search".to_owned()),
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("results").map(compact_json),
        ),
        "imageView" => (
            TimelineItemKind::Image,
            Some("Image".to_owned()),
            item.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "imageGeneration" => (
            TimelineItemKind::Image,
            Some("Generated image".to_owned()),
            item.get("savedPath")
                .and_then(Value::as_str)
                .or_else(|| {
                    item.get("result")
                        .and_then(Value::as_str)
                        .filter(|result| !result.starts_with("data:"))
                })
                .unwrap_or("Image generated")
                .to_owned(),
            item.get("revisedPrompt")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "enteredReviewMode" | "exitedReviewMode" => (
            TimelineItemKind::Review,
            Some("Review".to_owned()),
            item.get("review")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "contextCompaction" => (
            TimelineItemKind::Compaction,
            Some("Context compaction".to_owned()),
            "Codex compacted the conversation context.".to_owned(),
            None,
        ),
        "sleep" => (
            TimelineItemKind::Tool,
            Some("Waiting".to_owned()),
            item.get("durationMs")
                .and_then(Value::as_u64)
                .map(|duration| format!("Waiting for {duration} ms"))
                .unwrap_or_default(),
            None,
        ),
        other => (
            TimelineItemKind::Unknown(other.to_owned()),
            Some(title_case(other)),
            compact_json(item),
            None,
        ),
    };
    TimelineItem {
        id,
        kind,
        title,
        body,
        detail,
        status: timeline_status(item.get("status").and_then(Value::as_str), completed),
    }
}

fn timeline_status(status: Option<&str>, completed: bool) -> TimelineItemStatus {
    match status {
        Some("failed" | "declined") => TimelineItemStatus::Failed,
        Some("interrupted" | "cancelled") => TimelineItemStatus::Interrupted,
        Some("completed") => TimelineItemStatus::Completed,
        Some("inProgress" | "running") => TimelineItemStatus::Running,
        _ if completed => TimelineItemStatus::Completed,
        _ => TimelineItemStatus::Running,
    }
}

fn delta_kind(method: &str) -> TimelineItemKind {
    match method {
        "item/agentMessage/delta" => TimelineItemKind::AssistantMessage,
        "item/plan/delta" => TimelineItemKind::Plan,
        method if method.starts_with("item/reasoning/") => TimelineItemKind::Reasoning,
        "item/commandExecution/outputDelta" => TimelineItemKind::Command,
        "item/fileChange/outputDelta" => TimelineItemKind::FileChange,
        _ => TimelineItemKind::Unknown(method.to_owned()),
    }
}

fn delta_title(method: &str) -> Option<String> {
    match method {
        "item/plan/delta" => Some("Plan".to_owned()),
        method if method.starts_with("item/reasoning/") => Some("Reasoning".to_owned()),
        "item/commandExecution/outputDelta" => Some("Command".to_owned()),
        "item/fileChange/outputDelta" => Some("File changes".to_owned()),
        _ => None,
    }
}

fn request_id_key(id: &RequestId) -> String {
    match id {
        RequestId::Integer(id) => format!("integer:{id}"),
        RequestId::String(id) => format!("string:{id}"),
    }
}

fn request_id_key_value(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => number.as_i64().map(|id| format!("integer:{id}")),
        Value::String(id) => Some(format!("string:{id}")),
        _ => None,
    }
}

fn user_message_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| match input.get("type").and_then(Value::as_str) {
            Some("text") => input.get("text").and_then(Value::as_str).map(str::to_owned),
            Some("image") => Some("[Image]".to_owned()),
            Some("localImage") => Some(format!(
                "[Image: {}]",
                input.get("path").and_then(Value::as_str).unwrap_or("image")
            )),
            Some("audio") | Some("localAudio") => Some("[Audio]".to_owned()),
            Some("skill") | Some("mention") => Some(format!(
                "[{}]",
                input
                    .get("name")
                    .or_else(|| input.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or("Attachment")
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn flattened_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Value::String(value) => Some(value.clone()),
                Value::Object(object) => object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn file_change_summary(item: &Value) -> String {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let path = change.get("path")?.as_str()?;
            let kind = change
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("update");
            Some(format!("{kind}: {path}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
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

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
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

fn stable_hash(value: &str) -> u64 {
    value
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn nonnegative_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default()
}
