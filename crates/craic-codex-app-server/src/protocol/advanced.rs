use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AddCreditsNudgeCreditType {
    #[serde(rename = "credits")]
    Credits,
    #[serde(rename = "usage_limit")]
    UsageLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeAccountRateLimitResetCreditParams {
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAddCreditsNudgeEmailParams {
    pub credit_type: AddCreditsNudgeCreditType,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentConfigDetectParams {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_home: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwds: Option<Vec<PathBuf>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_age_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentConfigImportParams {
    pub migration_items: Vec<ExternalAgentConfigMigrationItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalAgentConfigMigrationItemType {
    #[serde(rename = "AGENTS_MD")]
    AgentsMd,
    #[serde(rename = "CONFIG")]
    Config,
    #[serde(rename = "SKILLS")]
    Skills,
    #[serde(rename = "PLUGINS")]
    Plugins,
    #[serde(rename = "MCP_SERVER_CONFIG")]
    McpServerConfig,
    #[serde(rename = "SUBAGENTS")]
    Subagents,
    #[serde(rename = "HOOKS")]
    Hooks,
    #[serde(rename = "COMMANDS")]
    Commands,
    #[serde(rename = "MEMORY")]
    Memory,
    #[serde(rename = "SESSIONS")]
    Sessions,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationDetails {
    #[serde(default)]
    pub plugins: Vec<Value>,
    #[serde(default)]
    pub skills: Vec<Value>,
    #[serde(default)]
    pub sessions: Vec<Value>,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
    #[serde(default)]
    pub hooks: Vec<Value>,
    #[serde(default)]
    pub subagents: Vec<Value>,
    #[serde(default)]
    pub commands: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAgentConfigMigrationItem {
    pub item_type: ExternalAgentConfigMigrationItemType,
    pub description: String,
    pub cwd: Option<PathBuf>,
    pub details: Option<MigrationDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackUploadParams {
    pub classification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_logs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_log_files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockExperimentalMethodParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowsSandboxSetupMode {
    Elevated,
    Unelevated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSandboxSetupStartParams {
    pub mode: WindowsSandboxSetupMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}
