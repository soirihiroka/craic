use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::process::ExitStatus as ProcessExitStatus;
use std::time::Duration;

use crate::protocol::{
    ClientInfo, ErrorResponse, InitializeCapabilities, InitializeResult, Notification, Request,
    Response,
};
use crate::version::VersionError;

#[derive(Debug, Clone)]
pub struct AppServerConfig {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub client_info: ClientInfo,
    pub capabilities: InitializeCapabilities,
    pub channel_capacity: usize,
    pub graceful_shutdown_timeout: Duration,
    /// Command used to enforce the minimum supported runtime. Set to `None` for transports such as
    /// SSH whose launcher performs its own version negotiation.
    pub version_command: Option<(OsString, Vec<OsString>)>,
}

impl Default for AppServerConfig {
    fn default() -> Self {
        Self {
            program: "codex".into(),
            args: vec!["app-server".into(), "--listen".into(), "stdio://".into()],
            cwd: None,
            client_info: ClientInfo::craic(env!("CARGO_PKG_VERSION")),
            capabilities: InitializeCapabilities::default(),
            channel_capacity: 4096,
            graceful_shutdown_timeout: Duration::from_secs(2),
            version_command: Some(("codex".into(), vec!["--version".into()])),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Stopped,
    Starting,
    Initializing,
    Ready,
    Stopping,
    Crashed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub success: bool,
}

impl From<ProcessExitStatus> for ExitStatus {
    fn from(status: ProcessExitStatus) -> Self {
        Self {
            code: status.code(),
            success: status.success(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppServerEvent {
    StateChanged(ConnectionState),
    Ready(InitializeResult),
    Response {
        response: Response,
        method: Option<String>,
    },
    ErrorResponse {
        response: ErrorResponse,
        method: Option<String>,
    },
    ServerRequest(Request),
    Notification(Notification),
    Diagnostic(String),
    ProtocolError(String),
    ProcessExited(ExitStatus),
}

#[derive(Debug)]
pub enum AppServerError {
    Version(VersionError),
    Spawn(std::io::Error),
    MissingPipe(&'static str),
    NotReady(ConnectionState),
    RequestIdExhausted,
    Serialize(serde_json::Error),
    CommandQueueFull,
    Disconnected,
}

impl fmt::Display for AppServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(error) => error.fmt(formatter),
            Self::Spawn(error) => write!(formatter, "failed to start Codex App Server: {error}"),
            Self::MissingPipe(name) => write!(formatter, "Codex App Server has no {name} pipe"),
            Self::NotReady(state) => write!(formatter, "Codex App Server is not ready ({state:?})"),
            Self::RequestIdExhausted => formatter.write_str("Codex request IDs are exhausted"),
            Self::Serialize(error) => {
                write!(formatter, "failed to serialize App Server message: {error}")
            }
            Self::CommandQueueFull => formatter.write_str("Codex command queue is full"),
            Self::Disconnected => formatter.write_str("Codex App Server is disconnected"),
        }
    }
}

impl std::error::Error for AppServerError {}
