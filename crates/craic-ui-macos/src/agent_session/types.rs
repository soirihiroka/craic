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
    pub multiline_text: bool,
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
