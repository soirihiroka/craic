use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use craic_agent::agent_history::{self, CodexThreadOverlayUpsert};
use craic_agent::remote_media::{self, RemoteMedia, RemoteMediaKind};
use craic_codex_app_server::protocol::{
    AppsInstalledParams, AppsListParams, ConfigReadParams, ExperimentalFeatureEnablementSetParams,
    ExperimentalFeatureListParams, GetAccountParams, ListMcpServerStatusParams,
    McpServerStatusDetail, ModelListParams, PermissionProfileListParams, PluginInstalledParams,
    PluginListParams, RequestId, ReviewDelivery as CodexReviewDelivery, ReviewStartParams,
    ReviewTarget as CodexReviewTarget, RpcError, SkillsListParams, ThreadArchiveParams,
    ThreadBackgroundTerminalsCleanParams, ThreadBackgroundTerminalsListParams,
    ThreadBackgroundTerminalsTerminateParams, ThreadCompactStartParams, ThreadDeleteParams,
    ThreadForkParams, ThreadGoalClearParams, ThreadGoalGetParams, ThreadGoalSetParams,
    ThreadListCwdFilter, ThreadListParams, ThreadResumeParams, ThreadRollbackParams,
    ThreadSetNameParams, ThreadSettingsUpdateParams, ThreadShellCommandParams, ThreadStartParams,
    ThreadUnarchiveParams, ThreadUnsubscribeParams, TurnInterruptParams, TurnStartParams,
    UserInput,
};
use craic_codex_app_server::{AppServer, AppServerError, AppServerEvent, ConnectionState};
use craic_system::SystemProvider;
use craic_system::system::capabilities::shell::ShellAccess;
use craic_system::system::providers::local::LocalProvider;
use craic_system::system::providers::ssh::{SshProvider, SshProviderConfig};
use craic_system::system::{ProviderKind, WorkspacePath};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const EVENT_DRAIN_LIMIT: usize = 256;
const MAX_RENDERED_JSON_BYTES: usize = 24 * 1024;
pub const DEFAULT_SERVICE_TIER_ID: &str = "__default__";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    pub workspace_id: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Connecting,
    Initializing,
    Ready,
    Running,
    Stopping,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptKind {
    User,
    Assistant,
    Developer,
    Reasoning,
    Plan,
    Command,
    FileChange,
    Tool,
    McpTool,
    Web,
    Image,
    Collaboration,
    Review,
    Compaction,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Model,
    Reasoning,
    Personality,
    ServiceTier,
    Permissions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadOperationKind {
    Rename,
    Archive,
    Unarchive,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveThreadAction {
    Fork,
    Compact,
    Rollback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewTarget {
    UncommittedChanges,
    BaseBranch(String),
    Commit(String),
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolAction {
    ViewThreadGoal,
    SetThreadGoal(String),
    ClearThreadGoal,
    RunShellCommand(String),
    BackgroundTerminals,
    Skills,
    McpServers,
    Apps,
    Plugins,
    ExperimentalFeatures,
    StopBackgroundTerminal(String),
    StopAllBackgroundTerminals,
    SetExperimentalFeatures(BTreeMap<String, bool>),
    AccountUsage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundTerminal {
    pub process_id: String,
    pub command: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillOption {
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentalFeature {
    pub name: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct TranscriptItem {
    pub id: String,
    pub kind: TranscriptKind,
    pub status: TranscriptStatus,
    pub title: Option<String>,
    pub body: String,
    pub detail: Option<String>,
    pub image: Option<TranscriptImageSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptImageSource {
    WorkspacePath(String),
    DataUri(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Audio,
    Mention,
    Skill,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub path: PathBuf,
    pub label: String,
    pub kind: AttachmentKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub last_total_tokens: u64,
    pub context_limit: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub smart_summary: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub updated_at: i64,
    pub pinned: bool,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestOption {
    pub value: String,
    pub label: String,
    pub destructive: bool,
}

#[derive(Clone, Debug)]
pub struct PendingRequest {
    pub key: String,
    pub title: String,
    pub message: String,
    pub options: Vec<RequestOption>,
    pub allows_text: bool,
    pub text_placeholder: Option<String>,
    pub secret: bool,
}

#[derive(Clone, Debug)]
pub enum RequestResponse {
    Choice(String),
    Text(String),
    Cancel,
}

#[derive(Debug)]
pub enum Command {
    Start {
        identity: SessionIdentity,
        workspace: craic_config::ConfiguredWorkspace,
        cancellation: CancellationToken,
        model: Option<String>,
        reasoning: Option<String>,
        personality: Option<String>,
        service_tier: Option<String>,
        permissions: Option<String>,
    },
    Send {
        identity: SessionIdentity,
        text: String,
        attachments: Vec<Attachment>,
    },
    Interrupt {
        identity: SessionIdentity,
    },
    SetModel {
        identity: SessionIdentity,
        model: String,
    },
    SetReasoning {
        identity: SessionIdentity,
        reasoning: String,
    },
    SetPersonality {
        identity: SessionIdentity,
        personality: String,
    },
    SetServiceTier {
        identity: SessionIdentity,
        service_tier: String,
    },
    SetPermissions {
        identity: SessionIdentity,
        permissions: String,
    },
    Resume {
        identity: SessionIdentity,
        thread_id: String,
    },
    FilterThreads {
        identity: SessionIdentity,
        query: String,
        archived: bool,
    },
    RenameThread {
        identity: SessionIdentity,
        thread_id: String,
        name: String,
    },
    ArchiveThread {
        identity: SessionIdentity,
        thread_id: String,
    },
    UnarchiveThread {
        identity: SessionIdentity,
        thread_id: String,
    },
    DeleteThread {
        identity: SessionIdentity,
        thread_id: String,
    },
    RunActiveThreadAction {
        identity: SessionIdentity,
        action: ActiveThreadAction,
    },
    StartReview {
        identity: SessionIdentity,
        target: ReviewTarget,
        detached: bool,
    },
    RunTool {
        identity: SessionIdentity,
        action: ToolAction,
    },
    Respond {
        identity: SessionIdentity,
        request_key: String,
        response: RequestResponse,
    },
    Reset,
    Shutdown {
        completed: Option<std::sync::mpsc::SyncSender<()>>,
    },
}

#[derive(Debug)]
pub enum Event {
    State {
        identity: SessionIdentity,
        state: SessionState,
        detail: Option<String>,
    },
    ThreadReady {
        identity: SessionIdentity,
        thread_id: String,
        title: Option<String>,
    },
    Upsert {
        identity: SessionIdentity,
        item: TranscriptItem,
    },
    Models {
        identity: SessionIdentity,
        options: Vec<SelectorOption>,
        selected: Option<String>,
    },
    ReasoningOptions {
        identity: SessionIdentity,
        options: Vec<SelectorOption>,
        selected: Option<String>,
    },
    PersonalityOptions {
        identity: SessionIdentity,
        options: Vec<SelectorOption>,
        selected: Option<String>,
    },
    ServiceTierOptions {
        identity: SessionIdentity,
        options: Vec<SelectorOption>,
        selected: Option<String>,
    },
    PermissionProfiles {
        identity: SessionIdentity,
        options: Vec<SelectorOption>,
        selected: Option<String>,
    },
    SettingApplied {
        identity: SessionIdentity,
        setting: SettingKind,
    },
    Usage {
        identity: SessionIdentity,
        usage: Option<TokenUsage>,
    },
    Threads {
        identity: SessionIdentity,
        threads: Vec<ThreadSummary>,
    },
    TranscriptCleared {
        identity: SessionIdentity,
    },
    ThreadClosed {
        identity: SessionIdentity,
        message: String,
    },
    ThreadOperationApplied {
        identity: SessionIdentity,
        thread_id: String,
        operation: ThreadOperationKind,
    },
    Request {
        identity: SessionIdentity,
        request: PendingRequest,
    },
    RequestResolved {
        identity: SessionIdentity,
        request_key: String,
    },
    BackgroundTerminals {
        identity: SessionIdentity,
        terminals: Vec<BackgroundTerminal>,
    },
    Skills {
        identity: SessionIdentity,
        skills: Vec<SkillOption>,
    },
    ExperimentalFeatures {
        identity: SessionIdentity,
        features: Vec<ExperimentalFeature>,
    },
    Cleared,
}

struct Session {
    identity: SessionIdentity,
    workspace_key: String,
    workspace_root: String,
    remote_media: Option<RemoteMediaContext>,
    cancellation: CancellationToken,
    server: AppServer,
    thread_id: Option<String>,
    thread_title: Option<String>,
    active_turn_id: Option<String>,
    timeline: HashMap<String, TranscriptItem>,
    next_local_id: u64,
    model_options: Vec<SelectorOption>,
    model_reasoning: HashMap<String, Vec<SelectorOption>>,
    model_service_tiers: HashMap<String, ModelServiceTiers>,
    permission_options: Vec<SelectorOption>,
    selected_model: Option<String>,
    selected_reasoning: Option<String>,
    selected_personality: Option<String>,
    selected_service_tier: Option<String>,
    selected_permissions: Option<String>,
    model_overridden: bool,
    reasoning_overridden: bool,
    personality_overridden: bool,
    service_tier_overridden: bool,
    permissions_overridden: bool,
    context_window_fallback: Option<u64>,
    pending_settings: HashMap<RequestId, PendingSetting>,
    resume_pending: bool,
    resume_previous_thread: Option<String>,
    thread_list_request: Option<RequestId>,
    thread_list_query: String,
    thread_list_archived: bool,
    pending_thread_operations: HashMap<RequestId, PendingThreadOperation>,
    pending_reviews: HashMap<RequestId, bool>,
    pending_tools: HashMap<RequestId, PendingTool>,
    pending_requests: HashMap<String, ServerRequest>,
    pending_turn_media: HashMap<RequestId, Vec<RemoteMedia>>,
}

#[derive(Clone)]
struct RemoteMediaContext {
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
}

enum PendingSetting {
    Model {
        model: Option<String>,
        reasoning: Option<String>,
        service_tier: Option<String>,
    },
    Reasoning(Option<String>),
    Personality(Option<String>),
    ServiceTier(Option<String>),
    Permissions(Option<String>),
}

#[derive(Clone)]
struct ModelServiceTiers {
    options: Vec<SelectorOption>,
    default: Option<String>,
}

enum PendingThreadOperation {
    Rename { thread_id: String, name: String },
    Archive { thread_id: String },
    Unarchive { thread_id: String },
    Delete { thread_id: String },
}

enum PendingTool {
    Timeline(String),
    BackgroundTerminals,
    Skills,
    ExperimentalFeatures,
}

impl PendingTool {
    fn title(&self) -> &str {
        match self {
            Self::Timeline(title) => title,
            Self::BackgroundTerminals => "Background terminals",
            Self::Skills => "Skills",
            Self::ExperimentalFeatures => "Experimental features",
        }
    }
}

struct ServerRequest {
    id: RequestId,
    method: String,
    params: Value,
}

impl Session {
    fn matches(&self, identity: &SessionIdentity) -> bool {
        self.identity == *identity && !self.cancellation.is_cancelled()
    }
}

pub async fn run<F>(mut commands: mpsc::Receiver<Command>, emit: F)
where
    F: Fn(Event) + Send + Sync + 'static,
{
    let mut session = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(16));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    shutdown_session(session.take(), &emit).await;
                    return;
                };
                match command {
                    Command::Start {
                        identity,
                        workspace,
                        cancellation,
                        model,
                        reasoning,
                        personality,
                        service_tier,
                        permissions,
                    } => {
                        shutdown_session(session.take(), &emit).await;
                        emit(Event::Cleared);
                        emit_state(&emit, &identity, SessionState::Connecting, None);
                        let startup = tokio::task::spawn_blocking(move || {
                            let (config, workspace_root, remote_media) = prepare(&workspace)?;
                            AppServer::spawn(config)
                                .map(|server| (server, workspace_root, remote_media))
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| format!("Codex startup task failed: {error}"));
                        match startup.and_then(|result| result) {
                            Ok((server, workspace_root, remote_media)) if !cancellation.is_cancelled() => {
                                log::info!(
                                    "native Codex session process started workspace={} generation={}",
                                    identity.workspace_id,
                                    identity.generation
                                );
                                let model_overridden = model.is_some();
                                let reasoning_overridden = reasoning.is_some();
                                let personality_overridden = personality.is_some();
                                let service_tier_overridden = service_tier.is_some();
                                let permissions_overridden = permissions.is_some();
                                let workspace_key = identity.workspace_id.clone();
                                session = Some(Session {
                                    identity,
                                    workspace_key,
                                    workspace_root,
                                    remote_media,
                                    cancellation,
                                    server,
                                    thread_id: None,
                                    thread_title: None,
                                    active_turn_id: None,
                                    timeline: HashMap::new(),
                                    next_local_id: 0,
                                    model_options: Vec::new(),
                                    model_reasoning: HashMap::new(),
                                    model_service_tiers: HashMap::new(),
                                    permission_options: Vec::new(),
                                    selected_model: model,
                                    selected_reasoning: reasoning,
                                    selected_personality: personality,
                                    selected_service_tier: service_tier,
                                    selected_permissions: permissions,
                                    model_overridden,
                                    reasoning_overridden,
                                    personality_overridden,
                                    service_tier_overridden,
                                    permissions_overridden,
                                    context_window_fallback: None,
                                    pending_settings: HashMap::new(),
                                    resume_pending: false,
                                    resume_previous_thread: None,
                                    thread_list_request: None,
                                    thread_list_query: String::new(),
                                    thread_list_archived: false,
                                    pending_thread_operations: HashMap::new(),
                                    pending_reviews: HashMap::new(),
                                    pending_tools: HashMap::new(),
                                    pending_requests: HashMap::new(),
                                    pending_turn_media: HashMap::new(),
                                });
                            }
                            Ok((mut server, _, _)) => {
                                let _ = tokio::task::spawn_blocking(move || server.shutdown()).await;
                            }
                            Err(error) => {
                                emit_state(&emit, &identity, SessionState::Closed, Some(error));
                            }
                        }
                    }
                    Command::Send { identity, text, attachments } => {
                        let text = text.trim().to_owned();
                        if text.is_empty() && attachments.is_empty() {
                            continue;
                        }
                        let remote_context = {
                            let Some(active) = session.as_ref().filter(|active| active.matches(&identity)) else {
                                continue;
                            };
                            if active.thread_id.is_none() {
                                emit_error(&emit, &identity, "The Codex thread is not ready yet");
                                continue;
                            }
                            if active.active_turn_id.is_some() {
                                emit_error(&emit, &identity, "Wait for the current Codex turn to finish or stop it first");
                                continue;
                            }
                            if active.resume_pending {
                                emit_error(&emit, &identity, "A Codex chat is already opening");
                                continue;
                            }
                            active.remote_media.clone()
                        };
                        let (attachments, uploaded) = if let Some(context) = remote_context {
                            let materialization = tokio::task::spawn_blocking(move || {
                                materialize_attachments(context, attachments)
                            })
                            .await
                            .map_err(|error| format!("Remote attachment task failed: {error}"));
                            match materialization.and_then(|result| result) {
                                Ok(result) => result,
                                Err(error) => {
                                    emit_error(&emit, &identity, &error);
                                    continue;
                                }
                            }
                        } else {
                            (attachments, Vec::new())
                        };
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.cancellation.is_cancelled() {
                            remove_remote_media(active.remote_media.as_ref(), uploaded);
                            continue;
                        }
                        let Some(thread_id) = active.thread_id.clone() else {
                            remove_remote_media(active.remote_media.as_ref(), uploaded);
                            continue;
                        };
                        active.next_local_id = active.next_local_id.wrapping_add(1);
                        let client_id = format!("craic-native-user-{}", active.next_local_id);
                        let mut extra = serde_json::Map::new();
                        if let Some(reasoning) = active.selected_reasoning.clone() {
                            extra.insert("effort".to_owned(), Value::String(reasoning));
                        }
                        let mut input = Vec::with_capacity(attachments.len() + usize::from(!text.is_empty()));
                        if !text.is_empty() {
                            input.push(UserInput::text(&text));
                        }
                        input.extend(attachments.iter().map(|attachment| match attachment.kind {
                            AttachmentKind::Image => UserInput::LocalImage {
                                path: attachment.path.clone(),
                                detail: None,
                            },
                            AttachmentKind::Audio => UserInput::LocalAudio {
                                path: attachment.path.clone(),
                            },
                            AttachmentKind::Mention => UserInput::Mention {
                                name: attachment.label.clone(),
                                path: attachment.path.to_string_lossy().into_owned(),
                            },
                            AttachmentKind::Skill => UserInput::Skill {
                                name: attachment.label.clone(),
                                path: attachment.path.clone(),
                            },
                        }));
                        let mut display = (!text.is_empty()).then(|| text.clone()).into_iter().collect::<Vec<_>>();
                        display.extend(attachments.iter().map(|attachment| format!("[{}]", attachment.label)));
                        match active.server.turn_start(TurnStartParams {
                            thread_id,
                            client_user_message_id: Some(client_id.clone()),
                            input,
                            cwd: None,
                            permissions: active.selected_permissions.clone(),
                            model: active.selected_model.clone(),
                            personality: active.selected_personality.clone(),
                            service_tier: selected_service_tier_wire(active),
                            extra,
                        }) {
                            Ok(request_id) => {
                                if !uploaded.is_empty() {
                                    active.pending_turn_media.insert(request_id, uploaded);
                                }
                                if active.thread_title.is_none()
                                    && let Some(title) = concise_title(&text)
                                {
                                    active.thread_title = Some(title.clone());
                                    persist_thread_overlay(
                                        active.workspace_key.clone(),
                                        active.thread_id.clone(),
                                        Some(title),
                                    )
                                    .await;
                                }
                                let item = TranscriptItem {
                                    id: client_id,
                                    kind: TranscriptKind::User,
                                    status: TranscriptStatus::Completed,
                                    title: Some("You".to_owned()),
                                    body: display.join("\n"),
                                    detail: None,
                                    image: None,
                                };
                                active.timeline.insert(item.id.clone(), item.clone());
                                emit(Event::Upsert { identity: identity.clone(), item });
                                emit_state(&emit, &identity, SessionState::Running, Some("Codex is working…".to_owned()));
                            }
                            Err(error) => {
                                remove_remote_media(active.remote_media.as_ref(), uploaded);
                                emit_error(&emit, &identity, &error.to_string());
                            }
                        }
                    }
                    Command::Interrupt { identity } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let (Some(thread_id), Some(turn_id)) =
                            (active.thread_id.clone(), active.active_turn_id.clone())
                        else {
                            continue;
                        };
                        if let Err(error) = active.server.turn_interrupt(TurnInterruptParams { thread_id, turn_id }) {
                            emit_error(&emit, &identity, &error.to_string());
                        } else {
                            emit_state(&emit, &identity, SessionState::Stopping, Some("Stopping turn…".to_owned()));
                        }
                    }
                    Command::SetModel { identity, model } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !active.model_options.iter().any(|option| option.id == model) {
                            emit_error(&emit, &identity, "The selected Codex model is no longer available");
                            continue;
                        }
                        let previous_model = active.selected_model.clone();
                        let previous_reasoning = active.selected_reasoning.clone();
                        let previous_service_tier = active.selected_service_tier.clone();
                        active.selected_model = Some(model.clone());
                        active.model_overridden = true;
                        update_reasoning_options(active);
                        update_service_tier_options(active);
                        emit_model_options(active, &emit);
                        emit_reasoning_options(active, &emit);
                        emit_service_tier_options(active, &emit);
                        emit(Event::Usage {
                            identity: identity.clone(),
                            usage: None,
                        });
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                model: Some(model),
                                effort: active.selected_reasoning.clone(),
                                service_tier: selected_service_tier_wire(active),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Model {
                                            model: previous_model,
                                            reasoning: previous_reasoning,
                                            service_tier: previous_service_tier,
                                        },
                                    );
                                }
                                Err(error) => {
                                    active.selected_model = previous_model;
                                    active.selected_reasoning = previous_reasoning;
                                    active.selected_service_tier = previous_service_tier;
                                    emit_model_options(active, &emit);
                                    emit_reasoning_options(active, &emit);
                                    emit_service_tier_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetReasoning { identity, reasoning } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !reasoning_options(active).iter().any(|option| option.id == reasoning) {
                            emit_error(&emit, &identity, "The selected reasoning effort is unavailable for this model");
                            continue;
                        }
                        let previous = active.selected_reasoning.clone();
                        active.selected_reasoning = Some(reasoning.clone());
                        active.reasoning_overridden = true;
                        emit_reasoning_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                effort: Some(reasoning),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Reasoning(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_reasoning = previous;
                                    emit_reasoning_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetPersonality { identity, personality } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !personality_options().iter().any(|option| option.id == personality) {
                            emit_error(&emit, &identity, "The selected Codex personality is unavailable");
                            continue;
                        }
                        let previous = active.selected_personality.clone();
                        active.selected_personality = Some(personality.clone());
                        active.personality_overridden = true;
                        emit_personality_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                personality: Some(personality),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Personality(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_personality = previous;
                                    emit_personality_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetServiceTier { identity, service_tier } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !service_tier_options(active).iter().any(|option| option.id == service_tier) {
                            emit_error(&emit, &identity, "The selected response speed is unavailable for this model");
                            continue;
                        }
                        let previous = active.selected_service_tier.clone();
                        active.selected_service_tier = Some(service_tier);
                        active.service_tier_overridden = true;
                        emit_service_tier_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                service_tier: selected_service_tier_wire(active),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::ServiceTier(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_service_tier = previous;
                                    emit_service_tier_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetPermissions { identity, permissions } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !active.permission_options.iter().any(|option| option.id == permissions) {
                            emit_error(&emit, &identity, "The selected Codex permission profile is no longer available");
                            continue;
                        }
                        let previous = active.selected_permissions.clone();
                        active.selected_permissions = Some(permissions.clone());
                        active.permissions_overridden = true;
                        emit_permission_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                permissions: Some(permissions),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Permissions(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_permissions = previous;
                                    emit_permission_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::Resume { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before switching chats");
                            continue;
                        }
                        if active.thread_id.as_deref() == Some(thread_id.as_str()) {
                            continue;
                        }
                        let previous_thread = active.thread_id.clone();
                        match active
                            .server
                            .thread_resume(thread_resume_params(active, thread_id))
                        {
                            Ok(_) => {
                                active.resume_pending = true;
                                active.resume_previous_thread = previous_thread;
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Opening Codex chat…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::FilterThreads {
                        identity,
                        query,
                        archived,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        active.thread_list_query = query.trim().to_owned();
                        active.thread_list_archived = archived;
                        if let Err(error) = request_thread_list(active) {
                            emit_error(&emit, &identity, &error);
                        }
                    }
                    Command::RenameThread {
                        identity,
                        thread_id,
                        name,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let name = name.trim();
                        if name.is_empty() {
                            emit_error(&emit, &identity, "A Codex thread name cannot be empty");
                            continue;
                        }
                        match active.server.thread_set_name(ThreadSetNameParams {
                            thread_id: thread_id.clone(),
                            name: name.to_owned(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Rename {
                                        thread_id,
                                        name: name.to_owned(),
                                    },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::ArchiveThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_archive(ThreadArchiveParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Archive { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::UnarchiveThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_unarchive(ThreadUnarchiveParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Unarchive { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::DeleteThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_delete(ThreadDeleteParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Delete { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::RunActiveThreadAction { identity, action } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(thread_id) = active.thread_id.clone() else {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before changing the thread");
                            continue;
                        }
                        let result = match action {
                            ActiveThreadAction::Fork => active.server.thread_fork(ThreadForkParams {
                                thread_id,
                                model: active.selected_model.clone(),
                                service_tier: selected_service_tier_wire(active),
                                cwd: Some(active.workspace_root.clone()),
                                permissions: active.selected_permissions.clone(),
                                ..Default::default()
                            }),
                            ActiveThreadAction::Compact => active.server.thread_compact_start(
                                ThreadCompactStartParams { thread_id },
                            ),
                            ActiveThreadAction::Rollback => active.server.thread_rollback(
                                ThreadRollbackParams {
                                    thread_id,
                                    num_turns: 1,
                                },
                            ),
                        };
                        match result {
                            Ok(_) if action == ActiveThreadAction::Fork => {
                                active.resume_pending = true;
                                active.resume_previous_thread = active.thread_id.clone();
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Forking Codex chat…".to_owned()),
                                );
                            }
                            Ok(_) => {}
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::StartReview { identity, target, detached } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(thread_id) = active.thread_id.clone() else {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before starting a review");
                            continue;
                        }
                        let target = match target {
                            ReviewTarget::UncommittedChanges => CodexReviewTarget::UncommittedChanges,
                            ReviewTarget::BaseBranch(branch) => CodexReviewTarget::BaseBranch { branch },
                            ReviewTarget::Commit(sha) => CodexReviewTarget::Commit { sha, title: None },
                            ReviewTarget::Custom(instructions) => CodexReviewTarget::Custom { instructions },
                        };
                        match active.server.review_start(ReviewStartParams {
                            thread_id,
                            target,
                            delivery: Some(if detached {
                                CodexReviewDelivery::Detached
                            } else {
                                CodexReviewDelivery::Inline
                            }),
                        }) {
                            Ok(request_id) => {
                                active.pending_reviews.insert(request_id, detached);
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Running,
                                    Some("Codex is reviewing…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::RunTool { identity, action } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.thread_id.is_none() {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        }
                        for (pending, request) in start_tool_requests(active, action) {
                            match request {
                                Ok(request_id) => {
                                    active.pending_tools.insert(request_id, pending);
                                }
                                Err(error) => emit_error(
                                    &emit,
                                    &identity,
                                    &format!("{} failed: {error}", pending.title()),
                                ),
                            }
                        }
                    }
                    Command::Respond {
                        identity,
                        request_key,
                        response,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(pending) = active.pending_requests.remove(&request_key) else {
                            emit_error(&emit, &identity, "That Codex request is no longer pending");
                            continue;
                        };
                        match response_for_server_request(
                            &pending.method,
                            &pending.params,
                            response,
                        ) {
                            Ok(result) => match active.server.respond(pending.id.clone(), result) {
                                Ok(()) => emit(Event::RequestResolved {
                                    identity: identity.clone(),
                                    request_key,
                                }),
                                Err(error) => {
                                    active.pending_requests.insert(request_key.clone(), pending);
                                    emit_error(&emit, &identity, &error.to_string());
                                    if let Some(pending) = active.pending_requests.get(&request_key)
                                        && let Some(request) = pending_request_from_server(
                                            &request_key,
                                            &pending.method,
                                            &pending.params,
                                        )
                                    {
                                        emit(Event::Request {
                                            identity: identity.clone(),
                                            request,
                                        });
                                    }
                                }
                            },
                            Err(error) => {
                                active.pending_requests.insert(request_key.clone(), pending);
                                emit_error(&emit, &identity, &error);
                            }
                        }
                    }
                    Command::Reset => {
                        shutdown_session(session.take(), &emit).await;
                        emit(Event::Cleared);
                    }
                    Command::Shutdown { completed } => {
                        shutdown_session(session.take(), &emit).await;
                        if let Some(completed) = completed {
                            let _ = completed.send(());
                        }
                        return;
                    }
                }
            }
            _ = ticker.tick(), if session.is_some() => {
                let cancelled = session
                    .as_ref()
                    .is_some_and(|active| active.cancellation.is_cancelled());
                if cancelled {
                    shutdown_session(session.take(), &emit).await;
                    emit(Event::Cleared);
                    continue;
                }
                let mut close = false;
                if let Some(active) = session.as_mut() {
                    for _ in 0..EVENT_DRAIN_LIMIT {
                        match active.server.try_recv() {
                            Ok(event) => {
                                if handle_server_event(active, event, &emit).await {
                                    close = true;
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                close = true;
                                break;
                            }
                        }
                    }
                }
                if close {
                    shutdown_session(session.take(), &emit).await;
                }
            }
        }
    }
}

fn prepare(
    workspace: &craic_config::ConfiguredWorkspace,
) -> Result<
    (
        craic_codex_app_server::AppServerConfig,
        String,
        Option<RemoteMediaContext>,
    ),
    String,
> {
    let (workspace_ref, shell, provider_kind) = match &workspace.provider {
        craic_config::WorkspaceProvider::Local => {
            let path = craic_config::expand_config_path_for_ui(&workspace.path)
                .unwrap_or_else(|| std::path::PathBuf::from(&workspace.path));
            let provider = LocalProvider::new();
            let workspace_ref = LocalProvider::workspace_for_path(&path);
            let shell = provider
                .shell(&workspace_ref)
                .ok_or_else(|| "Shell access is unavailable for this workspace".to_owned())?;
            (workspace_ref, shell, ProviderKind::Local)
        }
        craic_config::WorkspaceProvider::Ssh { host } => {
            let provider = SshProvider::new(SshProviderConfig::new(host.clone()));
            let workspace_ref = provider.workspace_for_remote_path(workspace.path.clone());
            let shell = provider
                .shell(&workspace_ref)
                .ok_or_else(|| "Shell access is unavailable for this SSH workspace".to_owned())?;
            (workspace_ref, shell, ProviderKind::Ssh)
        }
    };
    let root = workspace_ref.root.absolute.clone();
    let config = craic_agent::app_server::config(shell.as_ref(), &workspace_ref, provider_kind)?;
    let remote_media = (provider_kind != ProviderKind::Local).then(|| RemoteMediaContext {
        shell,
        working_dir: workspace_ref.root,
    });
    Ok((config, root, remote_media))
}

fn materialize_attachments(
    context: RemoteMediaContext,
    attachments: Vec<Attachment>,
) -> Result<(Vec<Attachment>, Vec<RemoteMedia>), String> {
    let mut resolved = Vec::with_capacity(attachments.len());
    let mut uploaded = Vec::with_capacity(attachments.len());
    for mut attachment in attachments {
        if matches!(
            attachment.kind,
            AttachmentKind::Mention | AttachmentKind::Skill
        ) {
            resolved.push(attachment);
            continue;
        }
        let kind = match attachment.kind {
            AttachmentKind::Image => RemoteMediaKind::Image,
            AttachmentKind::Audio => RemoteMediaKind::Audio,
            AttachmentKind::Mention | AttachmentKind::Skill => {
                unreachable!("references are never uploaded")
            }
        };
        match remote_media::materialize(
            context.shell.clone(),
            context.working_dir.clone(),
            attachment.path.clone(),
            kind,
        ) {
            Ok(remote) => {
                attachment.path = PathBuf::from(&remote.path);
                resolved.push(attachment);
                uploaded.push(remote);
            }
            Err(error) => {
                remote_media::remove(context.shell, context.working_dir, uploaded);
                return Err(error);
            }
        }
    }
    Ok((resolved, uploaded))
}

fn remove_remote_media(context: Option<&RemoteMediaContext>, uploaded: Vec<RemoteMedia>) {
    if let Some(context) = context {
        remote_media::remove(context.shell.clone(), context.working_dir.clone(), uploaded);
    }
}

fn remove_pending_turn_media(session: &mut Session, request_id: &RequestId) {
    if let Some(uploaded) = session.pending_turn_media.remove(request_id) {
        remove_remote_media(session.remote_media.as_ref(), uploaded);
    }
}

async fn handle_server_event<F>(session: &mut Session, event: AppServerEvent, emit: &F) -> bool
where
    F: Fn(Event),
{
    let identity = session.identity.clone();
    match event {
        AppServerEvent::StateChanged(ConnectionState::Starting) => {
            emit_state(emit, &identity, SessionState::Connecting, None)
        }
        AppServerEvent::StateChanged(ConnectionState::Initializing) => emit_state(
            emit,
            &identity,
            SessionState::Initializing,
            Some("Initializing Codex…".to_owned()),
        ),
        AppServerEvent::StateChanged(ConnectionState::Stopping) => {
            emit_state(emit, &identity, SessionState::Stopping, None)
        }
        AppServerEvent::StateChanged(ConnectionState::Stopped) => {
            emit_state(emit, &identity, SessionState::Closed, None);
            return true;
        }
        AppServerEvent::StateChanged(ConnectionState::Crashed) => {
            emit_state(
                emit,
                &identity,
                SessionState::Closed,
                Some("Codex App Server crashed".to_owned()),
            );
            return true;
        }
        AppServerEvent::StateChanged(ConnectionState::Ready) => {}
        AppServerEvent::Ready(_) => {
            emit_state(
                emit,
                &identity,
                SessionState::Initializing,
                Some("Starting a new Codex chat…".to_owned()),
            );
            for result in [
                session.server.model_list(ModelListParams::default()),
                session.server.config_read(ConfigReadParams {
                    include_layers: false,
                    cwd: Some(session.workspace_root.clone()),
                }),
                session
                    .server
                    .permission_profile_list(PermissionProfileListParams {
                        cwd: Some(session.workspace_root.clone()),
                        ..Default::default()
                    }),
            ] {
                if let Err(error) = result {
                    emit_error(emit, &identity, &error.to_string());
                }
            }
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
            let mut extra = serde_json::Map::new();
            if let Some(reasoning) = session.selected_reasoning.clone() {
                extra.insert("effort".to_owned(), Value::String(reasoning));
            }
            if let Err(error) = session.server.thread_start(ThreadStartParams {
                cwd: Some(session.workspace_root.clone()),
                model: session.selected_model.clone(),
                permissions: session.selected_permissions.clone(),
                personality: session.selected_personality.clone(),
                service_tier: selected_service_tier_wire(session),
                extra,
                ..Default::default()
            }) {
                emit_error(emit, &identity, &error.to_string());
                emit_state(
                    emit,
                    &identity,
                    SessionState::Closed,
                    Some("Unable to start a Codex thread".to_owned()),
                );
                return true;
            }
        }
        AppServerEvent::Response { response, method } => {
            if let Some(pending) = session.pending_tools.remove(&response.id) {
                match pending {
                    PendingTool::Timeline(title) => {
                        session.next_local_id = session.next_local_id.wrapping_add(1);
                        let item = TranscriptItem {
                            id: format!("craic-native-tool-result-{}", session.next_local_id),
                            kind: TranscriptKind::Tool,
                            status: TranscriptStatus::Completed,
                            title: Some(title),
                            body: summarize_tool_result(&response.result),
                            detail: Some(compact_json(&response.result)),
                            image: None,
                        };
                        session.timeline.insert(item.id.clone(), item.clone());
                        emit(Event::Upsert {
                            identity: identity.clone(),
                            item,
                        });
                    }
                    PendingTool::BackgroundTerminals => emit(Event::BackgroundTerminals {
                        identity: identity.clone(),
                        terminals: parse_background_terminals(&response.result),
                    }),
                    PendingTool::Skills => emit(Event::Skills {
                        identity: identity.clone(),
                        skills: parse_skills(&response.result),
                    }),
                    PendingTool::ExperimentalFeatures => emit(Event::ExperimentalFeatures {
                        identity: identity.clone(),
                        features: parse_experimental_features(&response.result),
                    }),
                }
                return false;
            }
            match method.as_deref() {
                Some("thread/start")
                | Some("thread/resume")
                | Some("thread/fork")
                | Some("thread/rollback") => {
                    let Some(thread_id) = response
                        .result
                        .pointer("/thread/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    else {
                        emit_error(emit, &identity, "Codex did not return a thread identifier");
                        emit_state(
                            emit,
                            &identity,
                            SessionState::Closed,
                            Some("Codex returned an invalid thread response".to_owned()),
                        );
                        return true;
                    };
                    let resumed = method.as_deref() != Some("thread/start");
                    if resumed {
                        session.resume_pending = false;
                        if let Some(previous) = session.resume_previous_thread.take()
                            && previous != thread_id
                            && let Err(error) =
                                session.server.thread_unsubscribe(ThreadUnsubscribeParams {
                                    thread_id: previous,
                                })
                        {
                            emit_error(emit, &identity, &error.to_string());
                        }
                        session.active_turn_id = None;
                        session.timeline.clear();
                        emit(Event::TranscriptCleared {
                            identity: identity.clone(),
                        });
                    }
                    emit(Event::Usage {
                        identity: identity.clone(),
                        usage: None,
                    });
                    session.thread_id = Some(thread_id.clone());
                    if resumed {
                        apply_resumed_history(session, &response.result, emit);
                    }
                    if let Some(model) = response
                        .result
                        .pointer("/thread/model")
                        .or_else(|| response.result.get("model"))
                        .and_then(Value::as_str)
                    {
                        session.selected_model = Some(model.to_owned());
                        if !session.model_options.is_empty() {
                            emit_model_options(session, emit);
                        }
                    }
                    if let Some(reasoning) = response
                        .result
                        .pointer("/thread/reasoningEffort")
                        .or_else(|| response.result.get("reasoningEffort"))
                        .and_then(Value::as_str)
                    {
                        session.selected_reasoning = Some(reasoning.to_owned());
                    }
                    update_reasoning_options(session);
                    update_service_tier_options(session);
                    update_personality_options(session);
                    if !session.model_options.is_empty() {
                        emit_reasoning_options(session, emit);
                        emit_service_tier_options(session, emit);
                        emit_personality_options(session, emit);
                    }
                    if let Some(personality) = response
                        .result
                        .pointer("/thread/personality")
                        .or_else(|| response.result.get("personality"))
                        .and_then(Value::as_str)
                    {
                        session.selected_personality = Some(personality.to_owned());
                        emit_personality_options(session, emit);
                    }
                    if let Some(service_tier) = response
                        .result
                        .pointer("/thread/serviceTier")
                        .or_else(|| response.result.get("serviceTier"))
                        .and_then(Value::as_str)
                    {
                        session.selected_service_tier = Some(service_tier.to_owned());
                        emit_service_tier_options(session, emit);
                    } else if response
                        .result
                        .pointer("/thread/serviceTier")
                        .or_else(|| response.result.get("serviceTier"))
                        .is_some_and(Value::is_null)
                    {
                        session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
                        emit_service_tier_options(session, emit);
                    }
                    if let Some(permissions) = response
                        .result
                        .pointer("/thread/activePermissionProfile/id")
                        .or_else(|| response.result.pointer("/activePermissionProfile/id"))
                        .and_then(Value::as_str)
                    {
                        session.selected_permissions = Some(permissions.to_owned());
                        if !session.permission_options.is_empty() {
                            emit_permission_options(session, emit);
                        }
                    }
                    let title = response
                        .result
                        .pointer("/thread/name")
                        .or_else(|| response.result.pointer("/thread/preview"))
                        .and_then(Value::as_str)
                        .filter(|title| !title.trim().is_empty())
                        .map(str::to_owned);
                    session.thread_title = title.clone();
                    persist_thread_overlay(
                        session.workspace_key.clone(),
                        session.thread_id.clone(),
                        title.clone(),
                    )
                    .await;
                    emit(Event::ThreadReady {
                        identity: identity.clone(),
                        thread_id,
                        title,
                    });
                    let ready_state = if session.active_turn_id.is_some() {
                        SessionState::Running
                    } else {
                        SessionState::Ready
                    };
                    emit_state(
                        emit,
                        &identity,
                        ready_state,
                        Some(if ready_state == SessionState::Running {
                            "Codex is working…".to_owned()
                        } else {
                            "Ready".to_owned()
                        }),
                    );
                    if let Err(error) = request_thread_list(session) {
                        emit_error(emit, &identity, &error);
                    }
                }
                Some("turn/start") => {
                    remove_pending_turn_media(session, &response.id);
                    if let Some(turn_id) =
                        response.result.pointer("/turn/id").and_then(Value::as_str)
                    {
                        session.active_turn_id = Some(turn_id.to_owned());
                    }
                    emit_state(
                        emit,
                        &identity,
                        SessionState::Running,
                        Some("Codex is working…".to_owned()),
                    );
                }
                Some("review/start") => {
                    let detached = session
                        .pending_reviews
                        .remove(&response.id)
                        .unwrap_or(false);
                    if detached
                        && let Some(review_thread_id) = response
                            .result
                            .get("reviewThreadId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    {
                        let previous_thread = session.thread_id.clone();
                        match session
                            .server
                            .thread_resume(thread_resume_params(session, review_thread_id))
                        {
                            Ok(_) => {
                                session.resume_pending = true;
                                session.resume_previous_thread = previous_thread;
                                emit_state(
                                    emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Opening review thread…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(emit, &identity, &error.to_string()),
                        }
                    } else {
                        if let Some(turn_id) =
                            response.result.pointer("/turn/id").and_then(Value::as_str)
                        {
                            session.active_turn_id = Some(turn_id.to_owned());
                        }
                        emit_state(
                            emit,
                            &identity,
                            SessionState::Running,
                            Some("Codex is reviewing…".to_owned()),
                        );
                    }
                }
                Some("turn/interrupt") => {}
                Some("model/list") => apply_model_catalog(session, &response.result, emit),
                Some("config/read") => apply_config_defaults(session, &response.result, emit),
                Some("permissionProfile/list") => {
                    apply_permission_profiles(session, &response.result, emit)
                }
                Some("thread/list") => {
                    apply_thread_list(session, &response.id, &response.result, emit).await
                }
                Some("thread/name/set")
                | Some("thread/archive")
                | Some("thread/unarchive")
                | Some("thread/delete") => {
                    apply_thread_operation_response(session, &response.id, emit).await;
                }
                Some("thread/settings/update") => {
                    if let Some(pending) = session.pending_settings.remove(&response.id) {
                        emit(Event::SettingApplied {
                            identity: identity.clone(),
                            setting: match pending {
                                PendingSetting::Model { .. } => SettingKind::Model,
                                PendingSetting::Reasoning(_) => SettingKind::Reasoning,
                                PendingSetting::Personality(_) => SettingKind::Personality,
                                PendingSetting::ServiceTier(_) => SettingKind::ServiceTier,
                                PendingSetting::Permissions(_) => SettingKind::Permissions,
                            },
                        });
                    }
                }
                _ => {}
            }
        }
        AppServerEvent::ErrorResponse { response, method } => {
            let tool_title = session.pending_tools.remove(&response.id);
            if method.as_deref() == Some("turn/start") {
                remove_pending_turn_media(session, &response.id);
            }
            if method.as_deref() == Some("review/start") {
                session.pending_reviews.remove(&response.id);
            }
            if method.as_deref() == Some("thread/list")
                && session.thread_list_request.as_ref() == Some(&response.id)
            {
                session.thread_list_request = None;
            }
            if matches!(
                method.as_deref(),
                Some("thread/name/set" | "thread/archive" | "thread/unarchive" | "thread/delete")
            ) {
                session.pending_thread_operations.remove(&response.id);
            }
            if method.as_deref() == Some("thread/settings/update")
                && let Some(pending) = session.pending_settings.remove(&response.id)
            {
                match pending {
                    PendingSetting::Model {
                        model,
                        reasoning,
                        service_tier,
                    } => {
                        session.selected_model = model;
                        session.selected_reasoning = reasoning;
                        session.selected_service_tier = service_tier;
                        emit_model_options(session, emit);
                        emit_reasoning_options(session, emit);
                        emit_service_tier_options(session, emit);
                    }
                    PendingSetting::Reasoning(previous) => {
                        session.selected_reasoning = previous;
                        emit_reasoning_options(session, emit);
                    }
                    PendingSetting::Personality(previous) => {
                        session.selected_personality = previous;
                        emit_personality_options(session, emit);
                    }
                    PendingSetting::ServiceTier(previous) => {
                        session.selected_service_tier = previous;
                        emit_service_tier_options(session, emit);
                    }
                    PendingSetting::Permissions(previous) => {
                        session.selected_permissions = previous;
                        emit_permission_options(session, emit);
                    }
                }
            }
            emit_error(
                emit,
                &identity,
                &format!(
                    "{} failed: {}",
                    tool_title
                        .as_ref()
                        .map(PendingTool::title)
                        .or(method.as_deref())
                        .unwrap_or("Codex request"),
                    response.error.message
                ),
            );
            if method.as_deref() == Some("thread/start") {
                emit_state(
                    emit,
                    &identity,
                    SessionState::Closed,
                    Some(response.error.message),
                );
                return true;
            }
            if matches!(
                method.as_deref(),
                Some("thread/resume" | "thread/fork" | "thread/rollback")
            ) {
                session.resume_pending = false;
                session.resume_previous_thread = None;
                emit_state(
                    emit,
                    &identity,
                    SessionState::Ready,
                    Some("Unable to open Codex chat".to_owned()),
                );
            }
            if matches!(
                method.as_deref(),
                Some("turn/start") | Some("turn/interrupt") | Some("review/start")
            ) {
                session.active_turn_id = None;
                emit_state(
                    emit,
                    &identity,
                    SessionState::Ready,
                    Some("Ready".to_owned()),
                );
            }
        }
        AppServerEvent::ServerRequest(request) => {
            if request.method == "currentTime/read" {
                let current_time_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                let _ = session.server.respond(
                    request.id,
                    serde_json::json!({ "currentTimeAt": current_time_at }),
                );
                return false;
            }
            let params = request.params.unwrap_or(Value::Null);
            if params
                .get("threadId")
                .and_then(Value::as_str)
                .is_some_and(|thread_id| session.thread_id.as_deref() != Some(thread_id))
            {
                let _ = session.server.respond_error(
                    request.id,
                    RpcError {
                        code: -32600,
                        message: "request targets an inactive thread".to_owned(),
                        data: None,
                    },
                );
                return false;
            }
            let request_key = request_id_key(&request.id);
            let Some(presentation) =
                pending_request_from_server(&request_key, &request.method, &params)
            else {
                let method = request.method;
                let _ = session.server.respond_error(
                    request.id,
                    RpcError {
                        code: -32601,
                        message: format!("Craic for macOS does not yet handle {method}"),
                        data: None,
                    },
                );
                emit_error(
                    emit,
                    &identity,
                    &format!("Codex requested unsupported operation {method}"),
                );
                return false;
            };
            session.pending_requests.insert(
                request_key,
                ServerRequest {
                    id: request.id,
                    method: request.method,
                    params,
                },
            );
            emit(Event::Request {
                identity,
                request: presentation,
            });
        }
        AppServerEvent::Notification(notification) => {
            handle_notification(session, &notification.method, notification.params, emit).await
        }
        AppServerEvent::Diagnostic(message) => {
            log::debug!("native Codex diagnostic bytes={}", message.len())
        }
        AppServerEvent::ProtocolError(message) => emit_error(emit, &identity, &message),
        AppServerEvent::ProcessExited(status) => {
            emit_state(
                emit,
                &identity,
                SessionState::Closed,
                Some(format!(
                    "Codex App Server exited with status {:?}",
                    status.code
                )),
            );
            return true;
        }
    }
    false
}

fn apply_model_catalog<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let Some(models) = result.get("data").and_then(Value::as_array) else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned an invalid model catalog",
        );
        return;
    };
    if models.is_empty() {
        log::warn!("native Codex model catalog was empty response={result}");
    }
    let mut default = None;
    let mut options = Vec::new();
    let mut reasoning_by_model = HashMap::new();
    let mut service_tiers_by_model = HashMap::new();
    for model in models {
        let Some(id) = model
            .get("model")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if model.get("isDefault").and_then(Value::as_bool) == Some(true) {
            default = Some(id.to_owned());
        }
        options.push(SelectorOption {
            id: id.to_owned(),
            label: model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned(),
        });
        reasoning_by_model.insert(
            id.to_owned(),
            model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| {
                    let id = effort.get("reasoningEffort")?.as_str()?;
                    Some(SelectorOption {
                        id: id.to_owned(),
                        label: title_case(id),
                    })
                })
                .collect(),
        );
        let mut service_tiers = vec![SelectorOption {
            id: DEFAULT_SERVICE_TIER_ID.to_owned(),
            label: "Standard".to_owned(),
        }];
        service_tiers.extend(
            model
                .get("serviceTiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tier| {
                    let id = tier.get("id").and_then(Value::as_str)?;
                    Some(SelectorOption {
                        id: id.to_owned(),
                        label: tier
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_owned(),
                    })
                }),
        );
        service_tiers_by_model.insert(
            id.to_owned(),
            ModelServiceTiers {
                options: service_tiers,
                default: model
                    .get("defaultServiceTier")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
        );
    }
    session.model_options = options;
    session.model_reasoning = reasoning_by_model;
    session.model_service_tiers = service_tiers_by_model;
    if !session.selected_model.as_ref().is_some_and(|selected| {
        session
            .model_options
            .iter()
            .any(|option| option.id == *selected)
    }) {
        session.selected_model = default.or_else(|| {
            session
                .model_options
                .first()
                .map(|option| option.id.clone())
        });
        session.model_overridden = false;
    }
    update_reasoning_options(session);
    update_service_tier_options(session);
    update_personality_options(session);
    emit_model_options(session, emit);
    emit_reasoning_options(session, emit);
    emit_service_tier_options(session, emit);
    emit_personality_options(session, emit);
}

fn request_thread_list(session: &mut Session) -> Result<(), String> {
    let mut extra = serde_json::Map::new();
    extra.insert("sortKey".to_owned(), Value::String("updated_at".to_owned()));
    extra.insert("sortDirection".to_owned(), Value::String("desc".to_owned()));
    let request_id = session
        .server
        .thread_list(ThreadListParams {
            limit: Some(100),
            archived: Some(session.thread_list_archived),
            cwd: Some(ThreadListCwdFilter::One(session.workspace_root.clone())),
            search_term: (!session.thread_list_query.is_empty())
                .then(|| session.thread_list_query.clone()),
            extra,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
    session.thread_list_request = Some(request_id);
    Ok(())
}

fn start_tool_requests(
    session: &Session,
    action: ToolAction,
) -> Vec<(PendingTool, Result<RequestId, AppServerError>)> {
    let Some(thread_id) = session.thread_id.clone() else {
        return Vec::new();
    };
    match action {
        ToolAction::ViewThreadGoal => vec![(
            PendingTool::Timeline("Thread goal".to_owned()),
            session
                .server
                .thread_goal_get(ThreadGoalGetParams { thread_id }),
        )],
        ToolAction::SetThreadGoal(objective) => vec![(
            PendingTool::Timeline("Set thread goal".to_owned()),
            session.server.thread_goal_set(ThreadGoalSetParams {
                thread_id,
                objective: Some(objective),
                status: None,
                token_budget: None,
            }),
        )],
        ToolAction::ClearThreadGoal => vec![(
            PendingTool::Timeline("Clear thread goal".to_owned()),
            session
                .server
                .thread_goal_clear(ThreadGoalClearParams { thread_id }),
        )],
        ToolAction::RunShellCommand(command) => vec![(
            PendingTool::Timeline("Shell command".to_owned()),
            session
                .server
                .thread_shell_command(ThreadShellCommandParams { thread_id, command }),
        )],
        ToolAction::BackgroundTerminals => vec![(
            PendingTool::BackgroundTerminals,
            session
                .server
                .thread_background_terminals_list(ThreadBackgroundTerminalsListParams {
                    thread_id,
                    cursor: None,
                    limit: Some(100),
                }),
        )],
        ToolAction::Skills => vec![(
            PendingTool::Skills,
            session.server.skills_list(SkillsListParams {
                cwds: vec![PathBuf::from(&session.workspace_root)],
                force_reload: false,
            }),
        )],
        ToolAction::McpServers => vec![(
            PendingTool::Timeline("MCP servers".to_owned()),
            session
                .server
                .mcp_server_status_list(ListMcpServerStatusParams {
                    cursor: None,
                    limit: Some(100),
                    detail: Some(McpServerStatusDetail::Full),
                    thread_id: Some(thread_id),
                }),
        )],
        ToolAction::Apps => vec![
            (
                PendingTool::Timeline("Available apps & connectors".to_owned()),
                session.server.apps_list(AppsListParams {
                    cursor: None,
                    limit: Some(100),
                    thread_id: Some(thread_id.clone()),
                    force_refetch: false,
                }),
            ),
            (
                PendingTool::Timeline("Installed apps & connectors".to_owned()),
                session.server.apps_installed(AppsInstalledParams {
                    thread_id: Some(thread_id),
                    force_refresh: false,
                }),
            ),
        ],
        ToolAction::Plugins => vec![
            (
                PendingTool::Timeline("Available plugins".to_owned()),
                session.server.plugin_list(PluginListParams {
                    cwds: Some(vec![PathBuf::from(&session.workspace_root)]),
                    marketplace_kinds: None,
                    force_refetch: false,
                }),
            ),
            (
                PendingTool::Timeline("Installed plugins".to_owned()),
                session.server.plugin_installed(PluginInstalledParams {
                    cwds: Some(vec![PathBuf::from(&session.workspace_root)]),
                    install_suggestion_plugin_names: None,
                }),
            ),
        ],
        ToolAction::ExperimentalFeatures => vec![(
            PendingTool::ExperimentalFeatures,
            session
                .server
                .experimental_feature_list(ExperimentalFeatureListParams {
                    cursor: None,
                    limit: Some(100),
                    thread_id: Some(thread_id),
                }),
        )],
        ToolAction::StopBackgroundTerminal(process_id) => vec![(
            PendingTool::Timeline("Stop background terminal".to_owned()),
            session.server.thread_background_terminals_terminate(
                ThreadBackgroundTerminalsTerminateParams {
                    thread_id,
                    process_id,
                },
            ),
        )],
        ToolAction::StopAllBackgroundTerminals => vec![(
            PendingTool::Timeline("Stop all background terminals".to_owned()),
            session.server.thread_background_terminals_clean(
                ThreadBackgroundTerminalsCleanParams { thread_id },
            ),
        )],
        ToolAction::SetExperimentalFeatures(enablement) => vec![(
            PendingTool::Timeline("Update experimental features".to_owned()),
            session.server.experimental_feature_enablement_set(
                ExperimentalFeatureEnablementSetParams { enablement },
            ),
        )],
        ToolAction::AccountUsage => vec![
            (
                PendingTool::Timeline("Account".to_owned()),
                session.server.account_read(GetAccountParams {
                    refresh_token: false,
                }),
            ),
            (
                PendingTool::Timeline("Account rate limits".to_owned()),
                session.server.account_rate_limits_read(),
            ),
            (
                PendingTool::Timeline("Account usage".to_owned()),
                session.server.account_usage_read(),
            ),
        ],
    }
}

async fn persist_thread_overlay(
    workspace_key: String,
    thread_id: Option<String>,
    task_description: Option<String>,
) {
    let Some(thread_id) = thread_id else {
        return;
    };
    let operation_workspace = workspace_key.clone();
    let operation_thread = thread_id.clone();
    let operation = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let existing =
            agent_history::lookup_codex_thread_overlay(&operation_workspace, &operation_thread)
                .map_err(|error| error.to_string())?;
        let task_description = task_description
            .filter(|description| !description.trim().is_empty())
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|overlay| overlay.task_description.clone())
            });
        let tags = existing.map(|overlay| overlay.tags).unwrap_or_default();
        agent_history::upsert_codex_thread_overlay(CodexThreadOverlayUpsert {
            thread_id: operation_thread,
            workspace_key: operation_workspace,
            task_description,
            tags,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    })
    .await;
    match operation {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::warn!(
                "failed persisting native Codex thread overlay workspace={workspace_key} thread_id={thread_id}: {error}"
            );
        }
        Err(error) => log::warn!(
            "native Codex thread overlay task failed workspace={workspace_key} thread_id={thread_id}: {error}"
        ),
    }
}

async fn apply_thread_operation_response<F>(session: &mut Session, request_id: &RequestId, emit: &F)
where
    F: Fn(Event),
{
    let Some(operation) = session.pending_thread_operations.remove(request_id) else {
        return;
    };
    match operation {
        PendingThreadOperation::Rename { thread_id, name } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Rename,
            });
            if session.thread_id.as_deref() == Some(thread_id.as_str()) {
                session.thread_title = Some(name.clone());
                persist_thread_overlay(
                    session.workspace_key.clone(),
                    session.thread_id.clone(),
                    Some(name.clone()),
                )
                .await;
                emit(Event::ThreadReady {
                    identity: session.identity.clone(),
                    thread_id,
                    title: Some(name),
                });
            }
        }
        PendingThreadOperation::Archive { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Archive,
            });
            close_active_thread(session, &thread_id, "Codex thread archived", emit)
        }
        PendingThreadOperation::Unarchive { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id,
                operation: ThreadOperationKind::Unarchive,
            });
        }
        PendingThreadOperation::Delete { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Delete,
            });
            let workspace_key = session.workspace_key.clone();
            let operation_workspace = workspace_key.clone();
            let operation_thread = thread_id.clone();
            match tokio::task::spawn_blocking(move || {
                agent_history::delete_codex_thread_overlay(&operation_workspace, &operation_thread)
                    .map_err(|error| error.to_string())
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!(
                    "failed deleting native Codex thread overlay workspace={workspace_key} thread_id={thread_id}: {error}"
                ),
                Err(error) => log::warn!(
                    "native Codex thread overlay deletion task failed workspace={workspace_key} thread_id={thread_id}: {error}"
                ),
            }
            close_active_thread(session, &thread_id, "Codex thread deleted", emit)
        }
    }
    if let Err(error) = request_thread_list(session) {
        emit_error(emit, &session.identity, &error);
    }
}

fn close_active_thread<F>(session: &mut Session, thread_id: &str, message: &str, emit: &F)
where
    F: Fn(Event),
{
    if session.thread_id.as_deref() != Some(thread_id) {
        return;
    }
    session.thread_id = None;
    session.thread_title = None;
    session.active_turn_id = None;
    session.resume_pending = false;
    session.resume_previous_thread = None;
    session.timeline.clear();
    for request_key in session.pending_requests.drain().map(|(key, _)| key) {
        emit(Event::RequestResolved {
            identity: session.identity.clone(),
            request_key,
        });
    }
    emit(Event::TranscriptCleared {
        identity: session.identity.clone(),
    });
    emit(Event::Usage {
        identity: session.identity.clone(),
        usage: None,
    });
    emit(Event::ThreadClosed {
        identity: session.identity.clone(),
        message: message.to_owned(),
    });
    emit_state(
        emit,
        &session.identity,
        SessionState::Closed,
        Some(message.to_owned()),
    );
}

fn thread_resume_params(session: &Session, thread_id: String) -> ThreadResumeParams {
    let mut extra = serde_json::Map::new();
    extra.insert("excludeTurns".to_owned(), Value::Bool(true));
    if let Some(reasoning) = session.selected_reasoning.clone() {
        extra.insert("effort".to_owned(), Value::String(reasoning));
    }
    extra.insert(
        "initialTurnsPage".to_owned(),
        serde_json::json!({
            "limit": 100,
            "sortDirection": "desc",
            "itemsView": "full"
        }),
    );
    ThreadResumeParams {
        thread_id,
        model: session.selected_model.clone(),
        model_provider: None,
        cwd: Some(session.workspace_root.clone()),
        permissions: session.selected_permissions.clone(),
        personality: session.selected_personality.clone(),
        service_tier: selected_service_tier_wire(session),
        extra,
    }
}

async fn apply_thread_list<F>(
    session: &mut Session,
    request_id: &RequestId,
    result: &Value,
    emit: &F,
) where
    F: Fn(Event),
{
    if session.thread_list_request.as_ref() != Some(request_id) {
        return;
    }
    session.thread_list_request = None;
    let Some(data) = result.get("data").and_then(Value::as_array).cloned() else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned an invalid thread history response",
        );
        return;
    };
    let workspace_key = session.workspace_key.clone();
    let thread_list_archived = session.thread_list_archived;
    let identity = session.identity.clone();
    let operation_workspace = workspace_key.clone();
    let overlays = match tokio::task::spawn_blocking(move || {
        agent_history::list_codex_thread_overlays(&operation_workspace, 10_000, 0)
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(overlays)) => overlays
            .into_iter()
            .map(|overlay| (overlay.thread_id.clone(), overlay))
            .collect::<HashMap<_, _>>(),
        Ok(Err(error)) => {
            log::warn!(
                "failed loading native Codex thread overlays workspace={workspace_key}: {error}"
            );
            HashMap::new()
        }
        Err(error) => {
            log::warn!(
                "native Codex thread overlay load task failed workspace={workspace_key}: {error}"
            );
            HashMap::new()
        }
    };
    let mut threads = data
        .iter()
        .filter_map(|thread| {
            let id = thread.get("id").and_then(Value::as_str)?.to_owned();
            let smart_summary = overlays
                .get(&id)
                .and_then(|overlay| overlay.task_description.as_deref())
                .map(str::trim)
                .filter(|summary| !summary.is_empty())
                .map(str::to_owned);
            let preview = thread
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let title = thread
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    if let Some(summary) = smart_summary.as_deref() {
                        summary
                    } else if preview.trim().is_empty() {
                        "Untitled Codex chat"
                    } else {
                        preview.as_str()
                    }
                })
                .to_owned();
            Some(ThreadSummary {
                id,
                title,
                preview,
                smart_summary,
                model: thread
                    .get("model")
                    .or_else(|| thread.get("modelProvider"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                status: thread
                    .pointer("/status/type")
                    .and_then(Value::as_str)
                    .map(title_case),
                updated_at: thread
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
                    .saturating_mul(1_000),
                pinned: thread
                    .get("isPinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                archived: thread
                    .get("archived")
                    .or_else(|| thread.get("isArchived"))
                    .and_then(Value::as_bool)
                    .unwrap_or(thread_list_archived),
            })
        })
        .collect::<Vec<_>>();
    threads.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    emit(Event::Threads { identity, threads });
}

fn apply_resumed_history<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let initial_page = result.get("initialTurnsPage");
    let turns = initial_page
        .and_then(|page| page.get("data"))
        .and_then(Value::as_array)
        .or_else(|| result.pointer("/thread/turns").and_then(Value::as_array));
    let mut turns = turns.cloned().unwrap_or_default();
    if initial_page.is_some() {
        turns.reverse();
    }
    for turn in &turns {
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(item) = transcript_from_history_item(item) {
                session.timeline.insert(item.id.clone(), item.clone());
                emit(Event::Upsert {
                    identity: session.identity.clone(),
                    item,
                });
            }
        }
        if turn.get("status").and_then(Value::as_str) == Some("inProgress") {
            session.active_turn_id = turn.get("id").and_then(Value::as_str).map(str::to_owned);
        }
    }
}

fn transcript_from_history_item(item: &Value) -> Option<TranscriptItem> {
    if item.get("type").and_then(Value::as_str) == Some("userMessage") {
        return Some(TranscriptItem {
            id: item.get("id")?.as_str()?.to_owned(),
            kind: TranscriptKind::User,
            status: TranscriptStatus::Completed,
            title: Some("You".to_owned()),
            body: item
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| flatten_text(item.get("content"))),
            detail: None,
            image: None,
        });
    }
    transcript_from_item(item, true)
}

fn apply_config_defaults<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let config = result.get("config").unwrap_or(result);
    session.context_window_fallback = config
        .get("model_context_window")
        .or_else(|| config.get("modelContextWindow"))
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0);
    if !session.model_overridden
        && let Some(model) = config.get("model").and_then(Value::as_str)
    {
        session.selected_model = Some(model.to_owned());
        if !session.model_options.is_empty() {
            emit_model_options(session, emit);
        }
    }
    if !session.reasoning_overridden
        && let Some(reasoning) = config
            .get("model_reasoning_effort")
            .or_else(|| config.get("modelReasoningEffort"))
            .and_then(Value::as_str)
    {
        session.selected_reasoning = Some(reasoning.to_owned());
        update_reasoning_options(session);
        if !session.model_options.is_empty() {
            emit_reasoning_options(session, emit);
        }
    }
    if !session.personality_overridden
        && let Some(personality) = config.get("personality").and_then(Value::as_str)
    {
        session.selected_personality = Some(personality.to_owned());
        update_personality_options(session);
        emit_personality_options(session, emit);
    }
    if !session.service_tier_overridden {
        if let Some(service_tier) = config
            .get("service_tier")
            .or_else(|| config.get("serviceTier"))
            .and_then(Value::as_str)
        {
            session.selected_service_tier = Some(service_tier.to_owned());
        } else if config
            .get("service_tier")
            .or_else(|| config.get("serviceTier"))
            .is_some_and(Value::is_null)
        {
            session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
        }
        update_service_tier_options(session);
        emit_service_tier_options(session, emit);
    }
    if !session.permissions_overridden
        && let Some(permissions) = ["permissions", "default_permissions", "defaultPermissions"]
            .into_iter()
            .find_map(|key| config.get(key).and_then(Value::as_str))
    {
        session.selected_permissions = Some(permissions.to_owned());
        if !session.permission_options.is_empty() {
            emit_permission_options(session, emit);
        }
    }
}

fn apply_permission_profiles<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let Some(profiles) = result.get("data").and_then(Value::as_array) else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned invalid permission profiles",
        );
        return;
    };
    if profiles.is_empty() {
        log::warn!("native Codex permission profile catalog was empty response={result}");
    }
    session.permission_options = profiles
        .iter()
        .filter(|profile| profile.get("allowed").and_then(Value::as_bool) != Some(false))
        .filter_map(|profile| {
            let id = profile.get("id").and_then(Value::as_str)?;
            Some(SelectorOption {
                id: id.to_owned(),
                label: permission_label(id),
            })
        })
        .collect();
    if !session
        .selected_permissions
        .as_ref()
        .is_some_and(|selected| {
            session
                .permission_options
                .iter()
                .any(|option| option.id == *selected)
        })
    {
        session.selected_permissions = session
            .permission_options
            .first()
            .map(|option| option.id.clone());
        session.permissions_overridden = false;
    }
    emit_permission_options(session, emit);
}

fn emit_model_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::Models {
        identity: session.identity.clone(),
        options: session.model_options.clone(),
        selected: session.selected_model.clone(),
    });
}

fn reasoning_options(session: &Session) -> Vec<SelectorOption> {
    session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_reasoning.get(model))
        .filter(|options| !options.is_empty())
        .cloned()
        .unwrap_or_else(|| {
            ["low", "medium", "high", "xhigh", "max", "ultra"]
                .into_iter()
                .map(|id| SelectorOption {
                    id: id.to_owned(),
                    label: title_case(id),
                })
                .collect()
        })
}

fn update_reasoning_options(session: &mut Session) {
    let options = reasoning_options(session);
    if !session
        .selected_reasoning
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        session.selected_reasoning = options.first().map(|option| option.id.clone());
    }
}

fn emit_reasoning_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::ReasoningOptions {
        identity: session.identity.clone(),
        options: reasoning_options(session),
        selected: session.selected_reasoning.clone(),
    });
}

fn personality_options() -> Vec<SelectorOption> {
    ["friendly", "pragmatic", "none"]
        .into_iter()
        .map(|id| SelectorOption {
            id: id.to_owned(),
            label: title_case(id),
        })
        .collect()
}

fn update_personality_options(session: &mut Session) {
    let options = personality_options();
    if !session
        .selected_personality
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        session.selected_personality = Some("pragmatic".to_owned());
    }
}

fn emit_personality_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::PersonalityOptions {
        identity: session.identity.clone(),
        options: personality_options(),
        selected: session.selected_personality.clone(),
    });
}

fn service_tier_options(session: &Session) -> Vec<SelectorOption> {
    session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_service_tiers.get(model))
        .map(|tiers| tiers.options.clone())
        .unwrap_or_else(|| {
            vec![SelectorOption {
                id: DEFAULT_SERVICE_TIER_ID.to_owned(),
                label: "Standard".to_owned(),
            }]
        })
}

fn update_service_tier_options(session: &mut Session) {
    let options = service_tier_options(session);
    if session
        .selected_service_tier
        .as_ref()
        .is_some_and(|selected| options.iter().any(|option| option.id == *selected))
    {
        return;
    }
    session.selected_service_tier = session
        .selected_model
        .as_ref()
        .and_then(|model| session.model_service_tiers.get(model))
        .and_then(|tiers| tiers.default.clone())
        .filter(|selected| options.iter().any(|option| option.id == *selected))
        .or_else(|| Some(DEFAULT_SERVICE_TIER_ID.to_owned()));
}

fn selected_service_tier_wire(session: &Session) -> Option<Option<String>> {
    session
        .selected_service_tier
        .as_ref()
        .map(|service_tier| (service_tier != DEFAULT_SERVICE_TIER_ID).then(|| service_tier.clone()))
}

fn emit_service_tier_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::ServiceTierOptions {
        identity: session.identity.clone(),
        options: service_tier_options(session),
        selected: session.selected_service_tier.clone(),
    });
}

fn emit_permission_options<F>(session: &Session, emit: &F)
where
    F: Fn(Event),
{
    emit(Event::PermissionProfiles {
        identity: session.identity.clone(),
        options: session.permission_options.clone(),
        selected: session.selected_permissions.clone(),
    });
}

fn permission_label(id: &str) -> String {
    match id {
        ":read-only" => "Read only".to_owned(),
        ":workspace" => "Workspace".to_owned(),
        ":full-access" | ":danger-full-access" => "Full access".to_owned(),
        _ => title_case(id.trim_start_matches(':')),
    }
}

fn title_case(value: &str) -> String {
    value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn concise_title(prompt: &str) -> Option<String> {
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return None;
    }
    let mut title = prompt.chars().take(72).collect::<String>();
    if title.chars().count() < prompt.chars().count() {
        title.push('…');
    }
    Some(title)
}

fn request_id_key(id: &RequestId) -> String {
    match id {
        RequestId::String(value) => format!("string:{value}"),
        RequestId::Integer(value) => format!("integer:{value}"),
    }
}

fn pending_request_from_server(key: &str, method: &str, params: &Value) -> Option<PendingRequest> {
    let approval = |title: &str, fallback: &str, command: bool| PendingRequest {
        key: key.to_owned(),
        title: title.to_owned(),
        message: approval_description(params, fallback),
        options: approval_options(params, command),
        allows_text: false,
        text_placeholder: None,
        secret: false,
    };
    match method {
        "item/commandExecution/requestApproval" => Some(approval(
            "Run command?",
            "Codex wants to run a command.",
            true,
        )),
        "item/fileChange/requestApproval" => Some(approval(
            "Apply file changes?",
            "Codex wants to modify files.",
            false,
        )),
        "item/permissions/requestApproval" => Some(PendingRequest {
            key: key.to_owned(),
            title: "Grant additional permissions?".to_owned(),
            message: approval_description(params, "Codex requested additional access."),
            options: vec![
                request_option("grant", "Grant for this turn", false),
                request_option("grant-session", "Grant for session", false),
                request_option("decline", "Decline", true),
            ],
            allows_text: false,
            text_placeholder: None,
            secret: false,
        }),
        "item/tool/requestUserInput" => {
            let questions = params
                .get("questions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first = questions.first().cloned().unwrap_or(Value::Null);
            let multiple = questions.len() > 1;
            let options = if multiple {
                Vec::new()
            } else {
                first
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        let label = option.get("label").and_then(Value::as_str)?;
                        Some(request_option(label, label, false))
                    })
                    .collect()
            };
            let message = questions
                .iter()
                .map(|question| {
                    question
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex needs input")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            Some(PendingRequest {
                key: key.to_owned(),
                title: first
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex needs input")
                    .to_owned(),
                message,
                allows_text: multiple
                    || options.is_empty()
                    || first.get("isOther").and_then(Value::as_bool) == Some(true),
                options,
                text_placeholder: Some(if multiple {
                    r#"{"question_id":"answer"}"#.to_owned()
                } else {
                    "Enter your response".to_owned()
                }),
                secret: questions.iter().any(|question| {
                    question.get("isSecret").and_then(Value::as_bool) == Some(true)
                }),
            })
        }
        "mcpServer/elicitation/request" => {
            let mode = params.get("mode").and_then(Value::as_str);
            let message = params
                .get("message")
                .or_else(|| params.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("An MCP server needs additional input.")
                .to_owned();
            Some(PendingRequest {
                key: key.to_owned(),
                title: params
                    .get("serverName")
                    .and_then(Value::as_str)
                    .map(|name| format!("{name} needs input"))
                    .unwrap_or_else(|| "MCP server request".to_owned()),
                message,
                options: if mode == Some("url") {
                    vec![
                        request_option("accept", "Acknowledge URL", false),
                        request_option("decline", "Decline", false),
                        request_option("cancel", "Cancel", true),
                    ]
                } else {
                    Vec::new()
                },
                allows_text: mode != Some("url"),
                text_placeholder: (mode != Some("url")).then(|| "Enter JSON response".to_owned()),
                secret: params
                    .get("requestedSchema")
                    .is_some_and(schema_contains_secret),
            })
        }
        _ => None,
    }
}

fn request_option(value: &str, label: &str, destructive: bool) -> RequestOption {
    RequestOption {
        value: value.to_owned(),
        label: label.to_owned(),
        destructive,
    }
}

fn approval_description(params: &Value, fallback: &str) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = params.get("reason").and_then(Value::as_str) {
        parts.push(reason.to_owned());
    }
    if let Some(command) = params.get("command").and_then(Value::as_str) {
        parts.push(command.to_owned());
    }
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
        parts.push(format!("Working directory: {cwd}"));
    }
    if parts.is_empty() {
        fallback.to_owned()
    } else {
        parts.join("\n\n")
    }
}

fn approval_options(params: &Value, command: bool) -> Vec<RequestOption> {
    let options = params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|decisions| {
            decisions
                .iter()
                .filter_map(|decision| {
                    if let Some(value) = decision.as_str() {
                        let label = match value {
                            "accept" => {
                                if command {
                                    "Allow once"
                                } else {
                                    "Apply once"
                                }
                            }
                            "acceptForSession" => {
                                if command {
                                    "Allow for session"
                                } else {
                                    "Apply for session"
                                }
                            }
                            "decline" => "Decline",
                            "cancel" => "Cancel",
                            _ => value,
                        };
                        return Some(request_option(
                            value,
                            label,
                            matches!(value, "decline" | "cancel"),
                        ));
                    }
                    let value = serde_json::to_string(decision).ok()?;
                    Some(request_option(&value, "Apply proposed policy", false))
                })
                .collect::<Vec<_>>()
        })
        .filter(|options| !options.is_empty());
    options.unwrap_or_else(|| {
        vec![
            request_option(
                "accept",
                if command { "Allow once" } else { "Apply once" },
                false,
            ),
            request_option(
                "acceptForSession",
                if command {
                    "Allow for session"
                } else {
                    "Apply for session"
                },
                false,
            ),
            request_option("decline", "Decline", true),
        ]
    })
}

fn schema_contains_secret(value: &Value) -> bool {
    match value {
        Value::Object(values) => {
            ["isSecret", "secret", "sensitive", "writeOnly"]
                .into_iter()
                .any(|key| values.get(key).and_then(Value::as_bool) == Some(true))
                || values.values().any(schema_contains_secret)
        }
        Value::Array(values) => values.iter().any(schema_contains_secret),
        _ => false,
    }
}

fn response_for_server_request(
    method: &str,
    params: &Value,
    response: RequestResponse,
) -> Result<Value, String> {
    let value = match response {
        RequestResponse::Choice(value) | RequestResponse::Text(value) => value,
        RequestResponse::Cancel => match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                "decline".to_owned()
            }
            "item/permissions/requestApproval" => "decline".to_owned(),
            "mcpServer/elicitation/request" => "cancel".to_owned(),
            "item/tool/requestUserInput" => String::new(),
            _ => return Err(format!("Unsupported Codex server request: {method}")),
        },
    };
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let decision =
                serde_json::from_str::<Value>(&value).unwrap_or_else(|_| Value::String(value));
            Ok(serde_json::json!({ "decision": decision }))
        }
        "item/permissions/requestApproval" => match value.as_str() {
            "grant" => Ok(serde_json::json!({
                "permissions": params.get("permissions").cloned().unwrap_or_else(|| serde_json::json!({})),
                "scope": "turn"
            })),
            "grant-session" => Ok(serde_json::json!({
                "permissions": params.get("permissions").cloned().unwrap_or_else(|| serde_json::json!({})),
                "scope": "session"
            })),
            _ => Ok(serde_json::json!({ "permissions": {}, "scope": "turn" })),
        },
        "item/tool/requestUserInput" => user_input_response(params, &value),
        "mcpServer/elicitation/request" => {
            if matches!(value.as_str(), "decline" | "cancel") {
                Ok(serde_json::json!({ "action": value, "content": null, "_meta": null }))
            } else if value == "accept" {
                Ok(serde_json::json!({ "action": "accept", "content": null, "_meta": null }))
            } else {
                Ok(serde_json::json!({
                    "action": "accept",
                    "content": serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value)),
                    "_meta": null
                }))
            }
        }
        _ => Err(format!("Unsupported Codex server request: {method}")),
    }
}

fn user_input_response(params: &Value, value: &str) -> Result<Value, String> {
    let questions = params
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex user-input request did not contain questions".to_owned())?;
    if questions.len() == 1 {
        let id = questions[0]
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Codex user-input question did not contain an id".to_owned())?;
        let mut answers = serde_json::Map::new();
        answers.insert(id.to_owned(), serde_json::json!({ "answers": [value] }));
        return Ok(serde_json::json!({ "answers": answers }));
    }
    let values = serde_json::from_str::<serde_json::Map<String, Value>>(value)
        .map_err(|error| format!("Enter a JSON object containing each answer: {error}"))?;
    let answers = questions
        .iter()
        .filter_map(|question| {
            let id = question.get("id")?.as_str()?;
            let value = values
                .get(id)
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let values = match value {
                Value::Array(values) => values,
                value => vec![value],
            };
            Some((id.to_owned(), serde_json::json!({ "answers": values })))
        })
        .collect::<serde_json::Map<_, _>>();
    Ok(serde_json::json!({ "answers": answers }))
}

async fn handle_notification<F>(
    session: &mut Session,
    method: &str,
    params: Option<Value>,
    emit: &F,
) where
    F: Fn(Event),
{
    let params = params.unwrap_or(Value::Null);
    let identity = session.identity.clone();
    match method {
        "serverRequest/resolved" => {
            if let Some(request_id) = params.get("requestId").cloned()
                && let Ok(request_id) = serde_json::from_value::<RequestId>(request_id)
            {
                let request_key = request_id_key(&request_id);
                if session.pending_requests.remove(&request_key).is_some() {
                    emit(Event::RequestResolved {
                        identity,
                        request_key,
                    });
                }
            }
        }
        "thread/started" => {
            if session.thread_id.is_none()
                && let Some(thread_id) = params.pointer("/thread/id").and_then(Value::as_str)
            {
                session.thread_id = Some(thread_id.to_owned());
            }
        }
        "thread/settings/updated" => {
            let settings = params.get("threadSettings").unwrap_or(&params);
            if let Some(model) = settings.get("model").and_then(Value::as_str) {
                session.selected_model = Some(model.to_owned());
                emit_model_options(session, emit);
            }
            if let Some(reasoning) = settings
                .get("effort")
                .or_else(|| settings.get("reasoningEffort"))
                .and_then(Value::as_str)
            {
                session.selected_reasoning = Some(reasoning.to_owned());
            }
            update_reasoning_options(session);
            update_service_tier_options(session);
            update_personality_options(session);
            emit_reasoning_options(session, emit);
            if let Some(personality) = settings.get("personality").and_then(Value::as_str) {
                session.selected_personality = Some(personality.to_owned());
            }
            emit_personality_options(session, emit);
            if let Some(service_tier) = settings.get("serviceTier").and_then(Value::as_str) {
                session.selected_service_tier = Some(service_tier.to_owned());
            } else if settings.get("serviceTier").is_some_and(Value::is_null) {
                session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
            }
            emit_service_tier_options(session, emit);
            if let Some(permissions) = settings
                .pointer("/activePermissionProfile/id")
                .or_else(|| settings.get("permissions"))
                .and_then(Value::as_str)
            {
                session.selected_permissions = Some(permissions.to_owned());
                emit_permission_options(session, emit);
            }
        }
        "thread/name/updated" => {
            if let (Some(thread_id), Some(name)) = (
                params.get("threadId").and_then(Value::as_str),
                params
                    .get("name")
                    .or_else(|| params.pointer("/thread/name"))
                    .and_then(Value::as_str),
            ) && session.thread_id.as_deref() == Some(thread_id)
            {
                session.thread_title = Some(name.to_owned());
                persist_thread_overlay(
                    session.workspace_key.clone(),
                    session.thread_id.clone(),
                    Some(name.to_owned()),
                )
                .await;
                emit(Event::ThreadReady {
                    identity: identity.clone(),
                    thread_id: thread_id.to_owned(),
                    title: Some(name.to_owned()),
                });
            }
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/compacted" => {
            session.next_local_id = session.next_local_id.wrapping_add(1);
            let item = TranscriptItem {
                id: format!("craic-native-compaction-{}", session.next_local_id),
                kind: TranscriptKind::Compaction,
                status: TranscriptStatus::Completed,
                title: Some("Context compacted".to_owned()),
                body: "Codex compacted this conversation's context.".to_owned(),
                detail: None,
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "thread/archived" | "thread/deleted" | "thread/closed" => {
            if let Some(thread_id) = params
                .get("threadId")
                .or_else(|| params.pointer("/thread/id"))
                .and_then(Value::as_str)
            {
                close_active_thread(
                    session,
                    thread_id,
                    if method == "thread/archived" {
                        "Codex thread archived"
                    } else if method == "thread/deleted" {
                        "Codex thread deleted"
                    } else {
                        "Codex thread closed"
                    },
                    emit,
                );
            }
            if method != "thread/closed"
                && let Err(error) = request_thread_list(session)
            {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/unarchived" => {
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/tokenUsage/updated" => apply_token_usage(session, &params, emit),
        "hook/started" | "hook/completed" => {
            let run = params.get("run").unwrap_or(&params);
            let id = run.get("id").and_then(Value::as_str).unwrap_or("current");
            let event = run
                .get("eventName")
                .and_then(Value::as_str)
                .unwrap_or("hook");
            let completed = method == "hook/completed";
            let status = transcript_status(run.get("status").and_then(Value::as_str), completed);
            let item = TranscriptItem {
                id: format!("hook:{id}"),
                kind: TranscriptKind::Tool,
                status,
                title: Some(format!("Hook · {}", title_case(event))),
                body: run
                    .get("statusMessage")
                    .and_then(Value::as_str)
                    .unwrap_or(if completed {
                        "Hook completed."
                    } else {
                        "Hook running."
                    })
                    .to_owned(),
                detail: run
                    .get("entries")
                    .filter(|entries| {
                        entries
                            .as_array()
                            .is_some_and(|entries| !entries.is_empty())
                    })
                    .map(compact_json),
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "turn/started" => {
            session.active_turn_id = params
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            emit_state(
                emit,
                &identity,
                SessionState::Running,
                Some("Codex is working…".to_owned()),
            );
        }
        "turn/completed" => {
            session.active_turn_id = None;
            let failed = params.pointer("/turn/status").and_then(Value::as_str) == Some("failed");
            if failed
                && let Some(message) = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
            {
                emit_error(emit, &identity, message);
            }
            emit_state(
                emit,
                &identity,
                SessionState::Ready,
                Some("Ready".to_owned()),
            );
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "item/started" | "item/completed" => {
            if let Some(item) = params.get("item")
                && let Some(item) = transcript_from_item(item, method == "item/completed")
            {
                session.timeline.insert(item.id.clone(), item.clone());
                emit(Event::Upsert { identity, item });
            }
        }
        "item/agentMessage/delta"
        | "item/plan/delta"
        | "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta"
        | "item/commandExecution/outputDelta"
        | "item/fileChange/outputDelta" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(Value::as_str) else {
                return;
            };
            if delta.is_empty() {
                return;
            }
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: transcript_delta_kind(method),
                    status: TranscriptStatus::Running,
                    title: activity_title(method),
                    body: String::new(),
                    detail: None,
                    image: None,
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
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/reasoning/summaryPartAdded" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let summary_index = params
                .get("summaryIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::Reasoning,
                    status: TranscriptStatus::Running,
                    title: Some("Reasoning".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(format!(
                "Reasoning summary part {} started.",
                summary_index + 1
            ));
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/commandExecution/terminalInteraction" => {
            let id = params
                .get("itemId")
                .and_then(Value::as_str)
                .or_else(|| params.get("processId").and_then(Value::as_str))
                .unwrap_or("current");
            let process_id = params
                .get("processId")
                .and_then(Value::as_str)
                .unwrap_or("process");
            let stdin = params
                .get("stdin")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let item = session
                .timeline
                .entry(id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: id.to_owned(),
                    kind: TranscriptKind::Command,
                    status: TranscriptStatus::Running,
                    title: Some("Terminal interaction".to_owned()),
                    body: format!("Sent input to {process_id}."),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            let detail = item.detail.get_or_insert_with(String::new);
            if !detail.is_empty() && !detail.ends_with('\n') {
                detail.push('\n');
            }
            detail.push_str("Input: ");
            detail.push_str(if stdin.is_empty() { "<empty>" } else { stdin });
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/fileChange/patchUpdated" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let detail = params
                .get("patch")
                .or_else(|| params.get("changes"))
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| compact_json(value))
                })
                .unwrap_or_default();
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::FileChange,
                    status: TranscriptStatus::Running,
                    title: Some("File changes".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(detail);
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/mcpToolCall/progress" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MCP tool is working.");
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::McpTool,
                    status: TranscriptStatus::Running,
                    title: Some("MCP tool".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(message.to_owned());
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "turn/diff/updated" => {
            let turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("current");
            let item = TranscriptItem {
                id: format!("turn-diff:{turn_id}"),
                kind: TranscriptKind::FileChange,
                status: TranscriptStatus::Running,
                title: Some("Turn changes".to_owned()),
                body: params
                    .get("diff")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                detail: None,
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "turn/plan/updated" => {
            let turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("current");
            let body = params
                .get("plan")
                .and_then(Value::as_array)
                .map(|steps| {
                    steps
                        .iter()
                        .map(|step| {
                            let marker = match step.get("status").and_then(Value::as_str) {
                                Some("completed") => "✓",
                                Some("inProgress") => "→",
                                Some("failed") => "×",
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
            let item = TranscriptItem {
                id: format!("turn-plan:{turn_id}"),
                kind: TranscriptKind::Plan,
                status: TranscriptStatus::Running,
                title: Some("Plan".to_owned()),
                body,
                detail: params
                    .get("explanation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "error" => {
            let message = params
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Codex reported an error");
            if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                emit_warning(emit, &identity, "Codex is retrying", message);
            } else {
                emit_error(emit, &identity, message);
            }
        }
        "warning" | "guardianWarning" | "configWarning" => {
            let message = params
                .get("message")
                .or_else(|| params.get("details"))
                .and_then(Value::as_str)
                .unwrap_or("Codex reported a warning");
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Codex warning");
            emit_warning(emit, &identity, title, message);
        }
        _ => {}
    }
}

fn apply_token_usage<F>(session: &Session, params: &Value, emit: &F)
where
    F: Fn(Event),
{
    if params
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(|thread_id| session.thread_id.as_deref() != Some(thread_id))
    {
        return;
    }
    let Some(total) = params.pointer("/tokenUsage/total") else {
        return;
    };
    let last = params.pointer("/tokenUsage/last").unwrap_or(total);
    emit(Event::Usage {
        identity: session.identity.clone(),
        usage: Some(TokenUsage {
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
                .filter(|value| *value > 0)
                .or(session.context_window_fallback),
        }),
    });
}

fn nonnegative_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default()
}

fn transcript_from_item(item: &Value, completed: bool) -> Option<TranscriptItem> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    if item_type == "userMessage" {
        return None;
    }
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(item_type)
        .to_owned();
    let (kind, title, body, detail) = match item_type {
        "agentMessage" => (
            TranscriptKind::Assistant,
            Some("Codex".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "plan" => (
            TranscriptKind::Plan,
            Some("Plan".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "reasoning" => (
            TranscriptKind::Reasoning,
            Some("Reasoning".to_owned()),
            flatten_text(item.get("summary")),
            nonempty(flatten_text(item.get("content"))),
        ),
        "hookPrompt" => (
            TranscriptKind::Developer,
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
            TranscriptKind::Command,
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
            TranscriptKind::FileChange,
            Some("File changes".to_owned()),
            "Codex updated workspace files.".to_owned(),
            item.get("changes").map(compact_json),
        ),
        "mcpToolCall" => (
            TranscriptKind::McpTool,
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
            TranscriptKind::Tool,
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
        "webSearch" => (
            TranscriptKind::Web,
            Some("Web search".to_owned()),
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("results").map(compact_json),
        ),
        "imageView" => (
            TranscriptKind::Image,
            Some("Image".to_owned()),
            item.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "imageGeneration" => (
            TranscriptKind::Image,
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
        "collabAgentToolCall" => (
            TranscriptKind::Collaboration,
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
            TranscriptKind::Collaboration,
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
        "enteredReviewMode" | "exitedReviewMode" => (
            TranscriptKind::Review,
            Some("Review".to_owned()),
            item.get("review")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "contextCompaction" => (
            TranscriptKind::Compaction,
            Some("Context compacted".to_owned()),
            "Codex compacted this conversation's context.".to_owned(),
            None,
        ),
        "sleep" => (
            TranscriptKind::Tool,
            Some("Waiting".to_owned()),
            item.get("durationMs")
                .and_then(Value::as_u64)
                .map(|duration| format!("Waiting for {duration} ms"))
                .unwrap_or_default(),
            None,
        ),
        other => (
            TranscriptKind::Tool,
            Some(title_case(other)),
            compact_json(item),
            None,
        ),
    };
    Some(TranscriptItem {
        id,
        kind,
        status: transcript_status(item.get("status").and_then(Value::as_str), completed),
        title,
        body,
        detail,
        image: transcript_image_source(item_type, item),
    })
}

fn transcript_image_source(item_type: &str, item: &Value) -> Option<TranscriptImageSource> {
    match item_type {
        "imageView" => item
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| TranscriptImageSource::WorkspacePath(path.to_owned())),
        "imageGeneration" => item
            .get("savedPath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| TranscriptImageSource::WorkspacePath(path.to_owned()))
            .or_else(|| {
                item.get("result")
                    .and_then(Value::as_str)
                    .filter(|result| result.starts_with("data:image/"))
                    .map(|result| TranscriptImageSource::DataUri(result.to_owned()))
            }),
        _ => None,
    }
}

fn transcript_status(status: Option<&str>, completed: bool) -> TranscriptStatus {
    match status {
        Some("failed" | "declined" | "denied") => TranscriptStatus::Failed,
        Some("interrupted" | "cancelled" | "canceled" | "aborted" | "timedOut") => {
            TranscriptStatus::Interrupted
        }
        Some("completed" | "ready" | "approved" | "success") => TranscriptStatus::Completed,
        Some("inProgress" | "running") => TranscriptStatus::Running,
        _ if completed => TranscriptStatus::Completed,
        _ => TranscriptStatus::Running,
    }
}

fn activity_title(method: &str) -> Option<String> {
    match method {
        "item/plan/delta" => Some("Plan".to_owned()),
        method if method.starts_with("item/reasoning/") => Some("Reasoning".to_owned()),
        "item/commandExecution/outputDelta" => Some("Command".to_owned()),
        "item/fileChange/outputDelta" => Some("File changes".to_owned()),
        _ => Some("Codex".to_owned()),
    }
}

fn transcript_delta_kind(method: &str) -> TranscriptKind {
    match method {
        "item/agentMessage/delta" => TranscriptKind::Assistant,
        "item/plan/delta" => TranscriptKind::Plan,
        method if method.starts_with("item/reasoning/") => TranscriptKind::Reasoning,
        "item/commandExecution/outputDelta" => TranscriptKind::Command,
        "item/fileChange/outputDelta" => TranscriptKind::FileChange,
        _ => TranscriptKind::Tool,
    }
}

fn flatten_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| flatten_text(Some(value)))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(value)) => value
            .get("text")
            .or_else(|| value.get("content"))
            .map(|value| flatten_text(Some(value)))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn parse_background_terminals(value: &Value) -> Vec<BackgroundTerminal> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|terminal| {
            let process_id = terminal.get("processId")?.as_str()?.to_owned();
            let command = terminal
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("Background command")
                .to_owned();
            let mut details = Vec::new();
            if let Some(cwd) = terminal.get("cwd").and_then(Value::as_str)
                && !cwd.is_empty()
            {
                details.push(cwd.to_owned());
            }
            if let Some(pid) = terminal.get("osPid").and_then(Value::as_u64) {
                details.push(format!("PID {pid}"));
            }
            if let Some(cpu) = terminal.get("cpuPercent").and_then(Value::as_f64) {
                details.push(format!("CPU {cpu:.1}%"));
            }
            if let Some(rss) = terminal.get("rssKb").and_then(Value::as_u64) {
                details.push(format!("RSS {rss} KiB"));
            }
            Some(BackgroundTerminal {
                process_id,
                command,
                detail: details.join(" · "),
            })
        })
        .collect()
}

fn parse_skills(value: &Value) -> Vec<SkillOption> {
    let mut skills = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            entry
                .get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) != Some(false))
        .filter_map(|skill| {
            Some(SkillOption {
                name: skill.get("name")?.as_str()?.to_owned(),
                description: skill
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                path: skill.get("path")?.as_str()?.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    skills.sort_by_key(|skill| skill.name.to_lowercase());
    skills.dedup_by(|left, right| left.path == right.path);
    skills
}

fn parse_experimental_features(value: &Value) -> Vec<ExperimentalFeature> {
    let mut features = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|feature| {
            let name = feature.get("name")?.as_str()?.to_owned();
            Some(ExperimentalFeature {
                label: feature
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_owned(),
                description: feature
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| feature.get("announcement").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned(),
                enabled: feature.get("enabled")?.as_bool()?,
                name,
            })
        })
        .collect::<Vec<_>>();
    features.sort_by_key(|feature| feature.label.to_lowercase());
    features
}

fn summarize_tool_result(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if let Some(objective) = value
        .get("objective")
        .or_else(|| value.pointer("/goal/objective"))
        .and_then(Value::as_str)
    {
        return objective.to_owned();
    }
    if let Some(items) = value.get("data").and_then(Value::as_array) {
        if items.is_empty() {
            return "No items returned.".to_owned();
        }
        let labels = items
            .iter()
            .take(20)
            .filter_map(|item| {
                item.get("displayName")
                    .or_else(|| item.get("name"))
                    .or_else(|| item.get("title"))
                    .or_else(|| item.get("command"))
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>();
        if !labels.is_empty() {
            let mut summary = labels.join("\n");
            if items.len() > labels.len() {
                summary.push_str(&format!("\n… and {} more", items.len() - labels.len()));
            }
            return summary;
        }
        return format!("{} items returned.", items.len());
    }
    compact_json(value)
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

fn emit_state<F>(emit: &F, identity: &SessionIdentity, state: SessionState, detail: Option<String>)
where
    F: Fn(Event),
{
    emit(Event::State {
        identity: identity.clone(),
        state,
        detail,
    });
}

fn emit_error<F>(emit: &F, identity: &SessionIdentity, message: &str)
where
    F: Fn(Event),
{
    emit(Event::Upsert {
        identity: identity.clone(),
        item: TranscriptItem {
            id: format!("error:{:016x}", stable_hash(message)),
            kind: TranscriptKind::Error,
            status: TranscriptStatus::Failed,
            title: Some("Error".to_owned()),
            body: message.to_owned(),
            detail: None,
            image: None,
        },
    });
}

fn emit_warning<F>(emit: &F, identity: &SessionIdentity, title: &str, message: &str)
where
    F: Fn(Event),
{
    emit(Event::Upsert {
        identity: identity.clone(),
        item: TranscriptItem {
            id: format!("warning:{:016x}", stable_hash(message)),
            kind: TranscriptKind::Warning,
            status: TranscriptStatus::Completed,
            title: Some(title.to_owned()),
            body: message.to_owned(),
            detail: None,
            image: None,
        },
    });
}

async fn shutdown_session<F>(session: Option<Session>, emit: &F)
where
    F: Fn(Event),
{
    let Some(mut session) = session else {
        return;
    };
    let identity = session.identity.clone();
    emit_state(
        emit,
        &identity,
        SessionState::Stopping,
        Some("Closing Codex…".to_owned()),
    );
    let shutdown = tokio::task::spawn_blocking(move || {
        let uploaded = session
            .pending_turn_media
            .drain()
            .flat_map(|(_, media)| media)
            .collect();
        remove_remote_media(session.remote_media.as_ref(), uploaded);
        session.server.shutdown()
    })
    .await;
    if let Err(error) = shutdown {
        log::warn!("native Codex shutdown task failed: {error}");
    }
    emit_state(emit, &identity, SessionState::Closed, None);
    log::info!(
        "native Codex session stopped workspace={} generation={}",
        identity.workspace_id,
        identity.generation
    );
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
