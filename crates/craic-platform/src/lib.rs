use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

pub fn terminal_environment() -> HashMap<String, String> {
    let mut environment = std::env::vars().collect::<HashMap<_, _>>();
    environment.remove("NO_COLOR");
    environment.insert("TERM".to_string(), "xterm-256color".to_string());
    environment.insert("COLORTERM".to_string(), "truecolor".to_string());
    environment.insert("TERM_PROGRAM".to_string(), "Craic".to_string());
    environment.insert("CLICOLOR".to_string(), "1".to_string());
    if !["LC_ALL", "LC_CTYPE", "LANG"].into_iter().any(|key| {
        environment.get(key).is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
    }) {
        environment.insert("LC_CTYPE".to_string(), "C.UTF-8".to_string());
    }
    environment
}

pub trait MainThreadDispatcher: Send + Sync {
    /// Enqueues `job` for a later native-main-loop turn. Implementations must not run it inline.
    fn schedule(&self, job: Box<dyn FnOnce() + Send>) -> Result<(), UiDispatchError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiDispatchError {
    message: String,
}

impl UiDispatchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for UiDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UiDispatchError {}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiContextId(uuid::Uuid);

impl UiContextId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for UiContextId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiEffectId(uuid::Uuid);

impl UiEffectId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for UiEffectId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRequest {
    pub heading: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmRequest {
    pub heading: String,
    pub message: String,
    pub confirm_label: String,
    pub cancel_label: String,
    pub destructive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRequest {
    pub heading: String,
    pub message: String,
    pub initial_value: String,
    pub confirm_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathPickerMode {
    OpenFile,
    OpenDirectory,
    SaveFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPickerRequest {
    pub mode: PathPickerMode,
    pub title: String,
    pub initial_path: Option<PathBuf>,
    pub allowed_extensions: Vec<String>,
    pub allow_multiple: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenPathKind {
    File,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenPathRequest {
    pub path: PathBuf,
    pub kind: OpenPathKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiEffect {
    Alert(AlertRequest),
    Confirm(ConfirmRequest),
    Prompt(PromptRequest),
    ChoosePath(PathPickerRequest),
    OpenPath(OpenPathRequest),
    RevealPath(PathBuf),
    OpenUrl(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiEffectRequest {
    pub id: UiEffectId,
    pub context: UiContextId,
    pub effect: UiEffect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiEffectResult {
    Acknowledged,
    Confirmed(bool),
    Prompted(Option<String>),
    PathsChosen(Vec<PathBuf>),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiEffectCompletion {
    pub id: UiEffectId,
    pub result: UiEffectResult,
}
