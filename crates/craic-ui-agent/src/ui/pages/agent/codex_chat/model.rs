use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const STRUCTURED_RESPONSE_PREFIX: &str = "craic-structured-request:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatConnectionStatus {
    Disconnected,
    Connecting,
    Initializing,
    Ready,
    Reconnecting,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatSelector {
    Model,
    Reasoning,
    Personality,
    Permissions,
    Collaboration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAttachmentKind {
    File,
    Image,
    Audio,
    Mention,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerAttachment {
    pub id: String,
    pub label: String,
    pub kind: ComposerAttachmentKind,
    pub reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerSubmission {
    pub text: String,
    pub attachments: Vec<ComposerAttachment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineItemKind {
    UserMessage,
    AssistantMessage,
    DeveloperMessage,
    Reasoning,
    Plan,
    Command,
    CommandOutput,
    FileChange,
    Tool,
    McpTool,
    Web,
    Image,
    Audio,
    Collaboration,
    Review,
    Compaction,
    Warning,
    Error,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineItemStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineItem {
    pub id: String,
    pub kind: TimelineItemKind,
    pub title: Option<String>,
    pub body: String,
    pub detail: Option<String>,
    pub status: TimelineItemStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatTimelineEntry {
    Item(TimelineItem),
    PendingRequest(PendingRequest),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
    pub context_limit: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub status: PlanStepStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanProgress {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub steps: Vec<PlanStep>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollaborationParticipantStatus {
    Pending,
    Working,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollaborationParticipant {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub status: CollaborationParticipantStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollaborationProgress {
    pub title: Option<String>,
    pub participants: Vec<CollaborationParticipant>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingRequestKind {
    Approval,
    UserInput,
    StructuredUserInput(RequestUserInput),
    McpElicitation,
    McpForm(McpFormRequest),
    McpUrl(McpUrlRequest),
    DynamicTool,
    DynamicToolOutput(DynamicToolRequest),
    TokenRefresh,
    Unknown(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestSelectionMode {
    Single,
    Multiple,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructuredRequestOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<StructuredRequestOption>,
    pub selection_mode: RequestSelectionMode,
    pub allows_other: bool,
    pub is_secret: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestUserInput {
    pub questions: Vec<RequestUserInputQuestion>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpFormFieldKind {
    Text {
        default: Option<String>,
        placeholder: Option<String>,
        format: Option<String>,
        secret: bool,
    },
    Number {
        default: Option<String>,
        minimum: Option<String>,
        maximum: Option<String>,
        integer: bool,
    },
    Boolean {
        default: bool,
    },
    Select {
        options: Vec<StructuredRequestOption>,
        multiple: bool,
        defaults: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpFormField {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: McpFormFieldKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpFormRequest {
    pub fields: Vec<McpFormField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpUrlRequest {
    pub url: String,
    pub elicitation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicToolRequest {
    pub output_placeholder: Option<String>,
    pub allows_failure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestOptionStyle {
    Default,
    Suggested,
    Destructive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestOption {
    pub id: String,
    pub label: String,
    pub style: RequestOptionStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRequest {
    pub request_id: String,
    pub kind: PendingRequestKind,
    pub title: String,
    pub description: String,
    pub options: Vec<RequestOption>,
    pub allows_text: bool,
    pub text_placeholder: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingRequestResponse {
    Option(String),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUserInputAnswer {
    pub answers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpElicitationResponseAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DynamicToolOutputContent {
    InputText { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StructuredRequestResponse {
    UserInput {
        answers: BTreeMap<String, RequestUserInputAnswer>,
    },
    McpElicitation {
        action: McpElicitationResponseAction,
        content: Option<BTreeMap<String, Value>>,
    },
    DynamicTool {
        content_items: Vec<DynamicToolOutputContent>,
        success: bool,
    },
}

impl PendingRequestResponse {
    pub fn structured(response: StructuredRequestResponse) -> Self {
        let encoded = serde_json::to_string(&response)
            .expect("structured Codex request responses contain only JSON-safe values");
        Self::Text(format!("{STRUCTURED_RESPONSE_PREFIX}{encoded}"))
    }

    pub fn structured_payload(
        &self,
    ) -> Result<Option<StructuredRequestResponse>, serde_json::Error> {
        let Self::Text(value) = self else {
            return Ok(None);
        };
        let Some(value) = value.strip_prefix(STRUCTURED_RESPONSE_PREFIX) else {
            return Ok(None);
        };
        serde_json::from_str(value).map(Some)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexChatAction {
    NewThread,
    OpenThread,
    ResumeThread,
    ShowHistory,
    ForkThread,
    ArchiveThread,
    CompactThread,
    StartReview,
    UndoLastTurn,
    OpenChanges,
    Submit(ComposerSubmission),
    Steer(ComposerSubmission),
    Interrupt,
    ChooseAttachment,
    ChooseMention,
    FilesDropped(Vec<String>),
    AttachmentRemoved(String),
    SelectorChanged {
        selector: ChatSelector,
        value: Option<String>,
    },
    ResolveRequest {
        request_id: String,
        response: PendingRequestResponse,
    },
}
