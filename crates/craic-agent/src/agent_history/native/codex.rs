use super::NativeSession;
use craic_codex_app_server::protocol::{
    RequestId, RpcError, ThreadListCwdFilter, ThreadListParams,
};
use craic_codex_app_server::{AppServer, AppServerConfig, AppServerEvent, ConnectionState};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HISTORY_TIMEOUT: Duration = Duration::from_secs(60);
const CANCELLATION_INTERVAL: Duration = Duration::from_millis(250);

struct HistoryRequest {
    cwd: PathBuf,
    limit: usize,
    deadline: Instant,
    cancellation: crate::agent_provider::CancellationToken,
    reply: mpsc::SyncSender<Result<Vec<NativeSession>, String>>,
}

enum HistoryError {
    Timeout(&'static str),
    Canceled,
    Closed,
    Protocol(String),
    Rpc(String),
    Startup(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(phase) => write!(formatter, "Codex history timed out while {phase}"),
            Self::Canceled => formatter.write_str("Codex history request canceled"),
            Self::Closed => formatter.write_str("Codex history connection closed"),
            Self::Protocol(error) | Self::Rpc(error) | Self::Startup(error) => {
                formatter.write_str(error)
            }
        }
    }
}

/// Owns a background connection to the same installed Codex used by the terminal.
/// Dropping the last sender ends the worker and shuts down its app-server off the UI thread.
#[derive(Clone)]
pub struct CodexHistoryReader {
    requests: mpsc::Sender<HistoryRequest>,
}

impl CodexHistoryReader {
    pub fn new(config: AppServerConfig) -> Result<Self, String> {
        let (requests, receiver) = mpsc::channel::<HistoryRequest>();
        std::thread::Builder::new()
            .name("codex-history".into())
            .spawn(move || {
                let mut server = None;
                while let Ok(request) = receiver.recv() {
                    let result = read_history_request(&mut server, &config, &request)
                        .map_err(|error| error.to_string());
                    let _ = request.reply.send(result);
                }
                if server.is_some() {
                    log::info!("stopping Codex history connection");
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { requests })
    }

    pub(super) fn list(&self, cwd: &Path, limit: usize) -> Result<Vec<NativeSession>, String> {
        let (reply, response) = mpsc::sync_channel(1);
        let cancellation = crate::agent_provider::CancellationToken::new();
        self.requests
            .send(HistoryRequest {
                cwd: cwd.to_path_buf(),
                limit,
                deadline: Instant::now() + HISTORY_TIMEOUT,
                cancellation: cancellation.clone(),
                reply,
            })
            .map_err(|_| "Codex history worker stopped".to_string())?;
        let result = response.recv_timeout(HISTORY_TIMEOUT);
        // A timed-out caller must not leave a queued or paginating RPC working
        // indefinitely and blocking every later refresh behind it.
        cancellation.cancel();
        match result {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "Codex history did not finish within {} seconds",
                HISTORY_TIMEOUT.as_secs()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Codex history worker stopped".to_owned())
            }
        }
    }
}

fn read_history_request(
    server: &mut Option<AppServer>,
    config: &AppServerConfig,
    request: &HistoryRequest,
) -> Result<Vec<NativeSession>, HistoryError> {
    if request.cancellation.is_canceled() {
        return Err(HistoryError::Canceled);
    }
    if Instant::now() >= request.deadline {
        return Err(HistoryError::Timeout("waiting for the history worker"));
    }
    for attempt in 0..2 {
        if request.cancellation.is_canceled() {
            return Err(HistoryError::Canceled);
        }
        if Instant::now() >= request.deadline {
            return Err(HistoryError::Timeout("reconnecting the history connection"));
        }
        // Initialization, state-DB fallback and every page share one attempt
        // deadline. A reconnect gets only the remaining end-to-end budget.
        let deadline = request.deadline.min(Instant::now() + REQUEST_TIMEOUT);
        let result = (|| {
            if server.is_none() {
                log::info!("starting Codex history connection attempt={}", attempt + 1);
                *server = Some(
                    AppServer::spawn(config.clone())
                        .map_err(|error| HistoryError::Startup(error.to_string()))?,
                );
                wait_for_response(
                    server.as_ref().unwrap(),
                    None,
                    deadline,
                    &request.cancellation,
                )?;
                log::info!("Codex history connection ready");
            }
            list_threads(
                server.as_ref().unwrap(),
                &request.cwd,
                request.limit,
                deadline,
                &request.cancellation,
            )
        })();
        match result {
            Ok(rows) => return Ok(rows),
            Err(error) => {
                let retry = attempt == 0
                    && matches!(
                        error,
                        HistoryError::Timeout(_) | HistoryError::Closed | HistoryError::Protocol(_)
                    )
                    && !request.cancellation.is_canceled()
                    && Instant::now() < request.deadline;
                // Never reuse a transport with an outstanding/late response.
                // Shutdown is bounded and runs on this history worker.
                server.take();
                if !retry {
                    return Err(error);
                }
                log::warn!("Codex history connection reset after failure; retrying once: {error}");
            }
        }
    }
    unreachable!("the final attempt returns its result")
}

fn list_threads(
    server: &AppServer,
    cwd: &Path,
    limit: usize,
    deadline: Instant,
    cancellation: &crate::agent_provider::CancellationToken,
) -> Result<Vec<NativeSession>, HistoryError> {
    let mut sessions = Vec::new();
    let mut cursor = None;
    loop {
        if cancellation.is_canceled() {
            return Err(HistoryError::Canceled);
        }
        if Instant::now() >= deadline {
            return Err(HistoryError::Timeout("reading thread/list"));
        }
        // Match the native resume picker's interactive, unarchived listing. Let
        // Codex resolve its own configured database, rollouts, names and paths.
        let mut params = ThreadListParams {
            cwd: Some(ThreadListCwdFilter::One(cwd.to_string_lossy().into_owned())),
            limit: Some(limit.saturating_sub(sessions.len()).clamp(1, 100) as u32),
            archived: Some(false),
            cursor: cursor.clone(),
            extra: json!({
                "sortKey": "updated_at",
                "modelProviders": [],
                "useStateDbOnly": true,
                "includeAttachedThreads": false,
                "currentAccountOnly": true
            })
            .as_object()
            .unwrap()
            .clone(),
            ..Default::default()
        };
        let request_id = server
            .thread_list(params.clone())
            .map_err(|error| HistoryError::Protocol(error.to_string()))?;
        let result = match wait_for_response(server, Some(&request_id), deadline, cancellation) {
            Err(HistoryError::Rpc(error)) if error.contains("state DB unavailable") => {
                params.extra.insert("useStateDbOnly".into(), json!(false));
                let request_id = server
                    .thread_list(params)
                    .map_err(|error| HistoryError::Protocol(error.to_string()))?;
                wait_for_response(server, Some(&request_id), deadline, cancellation)?
            }
            result => result?,
        };
        let threads = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                HistoryError::Protocol("Codex thread/list returned no thread array".to_string())
            })?;
        for thread in threads {
            let id = thread
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    HistoryError::Protocol("Codex returned a thread without an ID".to_string())
                })?;
            let title = ["name", "title", "preview"]
                .iter()
                .find_map(|field| {
                    thread
                        .get(field)
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                })
                .unwrap_or("Untitled Codex thread");
            let created_at_ms = thread
                .get("createdAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .saturating_mul(1000);
            let updated_at_ms = thread
                .get("updatedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .saturating_mul(1000);
            sessions.push(NativeSession {
                provider: "codex",
                id: id.to_string(),
                title: title.to_string(),
                created_at_ms,
                updated_at_ms,
            });
        }
        let next_cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty());
        if sessions.len() >= limit || next_cursor.is_none() {
            break;
        }
        if next_cursor == cursor.as_deref() {
            return Err(HistoryError::Protocol(
                "Codex repeated a history pagination cursor".to_string(),
            ));
        }
        cursor = next_cursor.map(str::to_string);
    }
    sessions.truncate(limit);
    Ok(sessions)
}

fn wait_for_response(
    server: &AppServer,
    request_id: Option<&RequestId>,
    deadline: Instant,
    cancellation: &crate::agent_provider::CancellationToken,
) -> Result<Value, HistoryError> {
    loop {
        if cancellation.is_canceled() {
            return Err(HistoryError::Canceled);
        }
        let phase = if request_id.is_none() {
            "initializing the connection"
        } else {
            "reading thread/list"
        };
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(HistoryError::Timeout(phase))?;
        let event = match server.recv_timeout(remaining.min(CANCELLATION_INTERVAL)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(HistoryError::Closed),
        };
        match event {
            AppServerEvent::Ready(_) if request_id.is_none() => return Ok(Value::Null),
            AppServerEvent::Response { response, .. } if request_id == Some(&response.id) => {
                return Ok(response.result);
            }
            AppServerEvent::ErrorResponse { response, .. }
                if request_id == Some(&response.id) || request_id.is_none() =>
            {
                return Err(HistoryError::Rpc(response.error.message));
            }
            AppServerEvent::ServerRequest(request) => {
                // This connection only browses history. Silently ignoring a
                // server request would leave both peers waiting forever.
                server
                    .respond_error(
                        request.id,
                        RpcError {
                            code: -32601,
                            message: "Client method unavailable in history browser".to_owned(),
                            data: None,
                        },
                    )
                    .map_err(|error| HistoryError::Protocol(error.to_string()))?;
            }
            AppServerEvent::ProtocolError(error) => return Err(HistoryError::Protocol(error)),
            AppServerEvent::ProcessExited(_)
            | AppServerEvent::StateChanged(ConnectionState::Crashed | ConnectionState::Stopped) => {
                return Err(HistoryError::Closed);
            }
            _ => {}
        }
    }
}
