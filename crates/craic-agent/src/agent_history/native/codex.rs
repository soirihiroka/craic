use super::NativeSession;
use craic_codex_app_server::protocol::{RequestId, ThreadListCwdFilter, ThreadListParams};
use craic_codex_app_server::{AppServer, AppServerConfig, AppServerEvent, ConnectionState};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct HistoryRequest {
    cwd: PathBuf,
    limit: usize,
    reply: mpsc::SyncSender<Result<Vec<NativeSession>, String>>,
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
                    let result = (|| {
                        if server.is_none() {
                            log::info!("starting Codex history connection");
                            let connection = AppServer::spawn(config.clone())
                                .map_err(|error| error.to_string())?;
                            wait_for_response(&connection, None)?;
                            server = Some(connection);
                            log::info!("Codex history connection ready");
                        }
                        list_threads(server.as_ref().unwrap(), &request.cwd, request.limit)
                    })();
                    if result.is_err() {
                        // A failed request must not leave a stale response or dead transport
                        // attached to the next refresh.
                        server.take();
                    }
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
        self.requests
            .send(HistoryRequest {
                cwd: cwd.to_path_buf(),
                limit,
                reply,
            })
            .map_err(|_| "Codex history worker stopped".to_string())?;
        response
            .recv_timeout(Duration::from_secs(60))
            .map_err(|error| format!("Codex history request did not finish: {error}"))?
    }
}

fn list_threads(
    server: &AppServer,
    cwd: &Path,
    limit: usize,
) -> Result<Vec<NativeSession>, String> {
    let mut sessions = Vec::new();
    let mut cursor = None;
    loop {
        // Match the native resume picker's interactive, unarchived listing. Let
        // Codex resolve its own configured database, rollouts, names and paths.
        let request_id = server
            .thread_list(ThreadListParams {
                cwd: Some(ThreadListCwdFilter::One(cwd.to_string_lossy().into_owned())),
                limit: Some(100),
                archived: Some(false),
                cursor: cursor.clone(),
                extra: json!({
                    "sortKey": "updated_at",
                    "modelProviders": [],
                    "useStateDbOnly": false,
                    "includeAttachedThreads": false,
                    "currentAccountOnly": true
                })
                .as_object()
                .unwrap()
                .clone(),
                ..Default::default()
            })
            .map_err(|error| error.to_string())?;
        let result = wait_for_response(server, Some(&request_id))?;
        let threads = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| "Codex thread/list returned no thread array".to_string())?;
        for thread in threads {
            let id = thread
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "Codex returned a thread without an ID".to_string())?;
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
            return Err("Codex repeated a history pagination cursor".to_string());
        }
        cursor = next_cursor.map(str::to_string);
    }
    sessions.truncate(limit);
    Ok(sessions)
}

fn wait_for_response(server: &AppServer, request_id: Option<&RequestId>) -> Result<Value, String> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| "Timed out reading Codex history".to_string())?;
        match server
            .recv_timeout(remaining)
            .map_err(|error| error.to_string())?
        {
            AppServerEvent::Ready(_) if request_id.is_none() => return Ok(Value::Null),
            AppServerEvent::Response { response, .. } if request_id == Some(&response.id) => {
                return Ok(response.result);
            }
            AppServerEvent::ErrorResponse { response, .. }
                if request_id == Some(&response.id) || request_id.is_none() =>
            {
                return Err(response.error.message);
            }
            AppServerEvent::ProtocolError(error) => return Err(error),
            AppServerEvent::ProcessExited(_)
            | AppServerEvent::StateChanged(ConnectionState::Crashed | ConnectionState::Stopped) => {
                return Err("Codex history connection closed".to_string());
            }
            _ => {}
        }
    }
}
