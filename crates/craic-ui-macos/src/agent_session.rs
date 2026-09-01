use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use craic_agent::agent_history::{self, CodexThreadOverlayUpsert};
use craic_agent::approval::{
    ApprovalOption, ApprovalOptionStyle, approval_decision_response, approval_description,
    approval_options as shared_approval_options, permission_approval_options,
    permission_approval_response,
};
use craic_agent::display::{
    compact_json, compact_request_json, concise_title,
    permission_profile_label as permission_label, request_id_key,
};
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
use craic_system::system::capabilities::shell::ShellAccess;
use craic_system::system::{ProviderKind, WorkspacePath};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const EVENT_DRAIN_LIMIT: usize = 256;
pub const DEFAULT_SERVICE_TIER_ID: &str = "__default__";

include!("agent_session/types.rs");
include!("agent_session/command_loop.rs");
include!("agent_session/preparation.rs");
include!("agent_session/server_events.rs");
include!("agent_session/settings.rs");
include!("agent_session/approvals.rs");
include!("agent_session/transcript.rs");
