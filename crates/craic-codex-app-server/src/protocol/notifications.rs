use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Notification;

macro_rules! server_notification_methods {
    ($($variant:ident => $method:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ServerNotificationMethod {
            $($variant),+
        }

        impl ServerNotificationMethod {
            pub fn from_wire(method: &str) -> Option<Self> {
                match method {
                    $($method => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $method,)+
                }
            }
        }

        impl Serialize for ServerNotificationMethod {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for ServerNotificationMethod {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let method = String::deserialize(deserializer)?;
                Self::from_wire(&method).ok_or_else(|| {
                    serde::de::Error::custom(format!("unknown server notification method {method}"))
                })
            }
        }

        impl std::fmt::Display for ServerNotificationMethod {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

server_notification_methods! {
    Error => "error",
    ThreadStarted => "thread/started",
    ThreadStatusChanged => "thread/status/changed",
    ThreadArchived => "thread/archived",
    ThreadDeleted => "thread/deleted",
    ThreadUnarchived => "thread/unarchived",
    ThreadClosed => "thread/closed",
    SkillsChanged => "skills/changed",
    ThreadNameUpdated => "thread/name/updated",
    ThreadGoalUpdated => "thread/goal/updated",
    ThreadGoalCleared => "thread/goal/cleared",
    EnvironmentConnected => "thread/environment/connected",
    EnvironmentDisconnected => "thread/environment/disconnected",
    ThreadSettingsUpdated => "thread/settings/updated",
    ThreadTokenUsageUpdated => "thread/tokenUsage/updated",
    TurnStarted => "turn/started",
    HookStarted => "hook/started",
    TurnCompleted => "turn/completed",
    HookCompleted => "hook/completed",
    TurnDiffUpdated => "turn/diff/updated",
    TurnPlanUpdated => "turn/plan/updated",
    ItemStarted => "item/started",
    ItemGuardianApprovalReviewStarted => "item/autoApprovalReview/started",
    ItemGuardianApprovalReviewCompleted => "item/autoApprovalReview/completed",
    ItemCompleted => "item/completed",
    RawResponseItemCompleted => "rawResponseItem/completed",
    RawResponseCompleted => "rawResponse/completed",
    AgentMessageDelta => "item/agentMessage/delta",
    PlanDelta => "item/plan/delta",
    CommandExecOutputDelta => "command/exec/outputDelta",
    ProcessOutputDelta => "process/outputDelta",
    ProcessExited => "process/exited",
    CommandExecutionOutputDelta => "item/commandExecution/outputDelta",
    TerminalInteraction => "item/commandExecution/terminalInteraction",
    FileChangeOutputDelta => "item/fileChange/outputDelta",
    FileChangePatchUpdated => "item/fileChange/patchUpdated",
    ServerRequestResolved => "serverRequest/resolved",
    McpToolCallProgress => "item/mcpToolCall/progress",
    McpServerOauthLoginCompleted => "mcpServer/oauthLogin/completed",
    McpServerStatusUpdated => "mcpServer/startupStatus/updated",
    AccountUpdated => "account/updated",
    AccountRateLimitsUpdated => "account/rateLimits/updated",
    AppListUpdated => "app/list/updated",
    RemoteControlStatusChanged => "remoteControl/status/changed",
    ExternalAgentConfigImportProgress => "externalAgentConfig/import/progress",
    ExternalAgentConfigImportCompleted => "externalAgentConfig/import/completed",
    FsChanged => "fs/changed",
    ReasoningSummaryTextDelta => "item/reasoning/summaryTextDelta",
    ReasoningSummaryPartAdded => "item/reasoning/summaryPartAdded",
    ReasoningTextDelta => "item/reasoning/textDelta",
    ContextCompacted => "thread/compacted",
    ModelRerouted => "model/rerouted",
    ModelVerification => "model/verification",
    TurnModerationMetadata => "turn/moderationMetadata",
    ModelSafetyBufferingUpdated => "model/safetyBuffering/updated",
    Warning => "warning",
    GuardianWarning => "guardianWarning",
    DeprecationNotice => "deprecationNotice",
    ConfigWarning => "configWarning",
    FuzzyFileSearchSessionUpdated => "fuzzyFileSearch/sessionUpdated",
    FuzzyFileSearchSessionCompleted => "fuzzyFileSearch/sessionCompleted",
    ThreadRealtimeStarted => "thread/realtime/started",
    ThreadRealtimeItemAdded => "thread/realtime/itemAdded",
    ThreadRealtimeTranscriptDelta => "thread/realtime/transcript/delta",
    ThreadRealtimeTranscriptDone => "thread/realtime/transcript/done",
    ThreadRealtimeOutputAudioDelta => "thread/realtime/outputAudio/delta",
    ThreadRealtimeSdp => "thread/realtime/sdp",
    ThreadRealtimeError => "thread/realtime/error",
    ThreadRealtimeClosed => "thread/realtime/closed",
    WindowsWorldWritableWarning => "windows/worldWritableWarning",
    WindowsSandboxSetupCompleted => "windowsSandbox/setupCompleted",
    AccountLoginCompleted => "account/login/completed",
}

impl Notification {
    pub fn server_method(&self) -> Option<ServerNotificationMethod> {
        ServerNotificationMethod::from_wire(&self.method)
    }

    pub fn deserialize_params<P: DeserializeOwned>(&self) -> Result<P, serde_json::Error> {
        serde_json::from_value(self.params.clone().unwrap_or(serde_json::Value::Null))
    }
}

macro_rules! thread_id_notifications {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct $name {
                pub thread_id: String,
            }
        )+
    };
}

thread_id_notifications! {
    ThreadArchivedNotification,
    ThreadDeletedNotification,
    ThreadUnarchivedNotification,
    ThreadClosedNotification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadNameUpdatedNotification {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiffUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandExecutionOutputDeltaNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInteractionNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub process_id: String,
    pub stdin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRequestResolvedNotification {
    pub thread_id: String,
    pub request_id: super::RequestId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningNotification {
    pub thread_id: Option<String>,
    pub message: String,
}
