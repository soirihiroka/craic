use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeOutputModality {
    Text,
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeVoice {
    Alloy,
    Arbor,
    Ash,
    Ballad,
    Breeze,
    Cedar,
    Coral,
    Cove,
    Echo,
    Ember,
    Juniper,
    Maple,
    Marin,
    Sage,
    Shimmer,
    Sol,
    Spruce,
    Vale,
    Verse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationTextRole {
    User,
    Developer,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeConversationVersion {
    V1,
    V2,
    V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexResponseHandoffMode {
    Thinking,
    Commentary,
    BemTags,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadRealtimeStartTransport {
    Websocket,
    Webrtc { sdp: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeInitialItem {
    pub role: ConversationTextRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeStartParams {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_managed_handoffs: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flush_transcript_tail_on_session_end: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_responses_as_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_response_item_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_response_handoff_mode: Option<CodexResponseHandoffMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_response_handoff_channel_prefixes: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub output_modality: RealtimeOutputModality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_startup_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_items: Option<Vec<ThreadRealtimeInitialItem>>,
    #[serde(
        default,
        deserialize_with = "super::serde_helpers::deserialize_double_option",
        serialize_with = "super::serde_helpers::serialize_double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<ThreadRealtimeStartTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<RealtimeConversationVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<RealtimeVoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeAudioChunk {
    pub data: String,
    pub sample_rate: u32,
    pub num_channels: u16,
    pub samples_per_channel: Option<u32>,
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeAppendAudioParams {
    pub thread_id: String,
    pub audio: ThreadRealtimeAudioChunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeAppendTextParams {
    pub thread_id: String,
    pub text: String,
    pub role: ConversationTextRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeAppendSpeechParams {
    pub thread_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRealtimeStopParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRealtimeListVoicesParams {}
