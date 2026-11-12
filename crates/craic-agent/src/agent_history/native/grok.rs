use super::NativeSession;
use crate::system::capabilities::shell::ShellCommandSpec;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct HistoryRequest {
    cwd: PathBuf,
    limit: usize,
    reply: mpsc::SyncSender<Result<Vec<NativeSession>, String>>,
}

#[derive(Clone)]
pub struct GrokHistoryReader {
    requests: tokio::sync::mpsc::UnboundedSender<HistoryRequest>,
}

impl GrokHistoryReader {
    pub fn new(command: ShellCommandSpec) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let (requests, mut receiver) = tokio::sync::mpsc::unbounded_channel::<HistoryRequest>();
        std::thread::Builder::new()
            .name("grok-history".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let mut connection: Option<GrokConnection> = None;
                    while let Some(request) = receiver.recv().await {
                        let result = tokio::time::timeout(Duration::from_secs(45), async {
                            if connection.is_none() {
                                log::info!("starting Grok history connection");
                                connection = Some(GrokConnection::start(&command).await?);
                                log::info!("Grok history connection ready");
                            }
                            connection
                                .as_mut()
                                .unwrap()
                                .list(&request.cwd, request.limit)
                                .await
                        })
                        .await
                        .unwrap_or_else(|_| Err("Timed out reading Grok history".to_string()));
                        if result.is_err()
                            && let Some(mut process) = connection.take()
                        {
                            let _ = process.child.kill().await;
                        }
                        let _ = request.reply.send(result);
                    }
                    if let Some(mut process) = connection {
                        log::info!("stopping Grok history connection");
                        let _ = process.child.kill().await;
                    }
                });
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
            .map_err(|_| "Grok history worker stopped".to_string())?;
        response
            .recv_timeout(Duration::from_secs(50))
            .map_err(|error| format!("Grok history request did not finish: {error}"))?
    }
}

struct GrokConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_request_id: u64,
}

impl GrokConnection {
    async fn start(command: &ShellCommandSpec) -> Result<Self, String> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .current_dir(&command.working_dir.absolute)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Grok history has no input pipe".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Grok history has no output pipe".to_string())?;
        let mut connection = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_request_id: 1,
        };
        let initialized = connection
            .call(
                "initialize",
                json!({
                    "protocolVersion": 2,
                    "clientCapabilities": {},
                    "clientInfo": { "name": "craic-history", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        if initialized
            .pointer("/agentCapabilities/sessionCapabilities/list")
            .is_none()
        {
            return Err("This Grok CLI does not advertise session/list support".to_string());
        }
        Ok(connection)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut message = serde_json::to_vec(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .map_err(|error| error.to_string())?;
        message.push(b'\n');
        self.stdin
            .write_all(&message)
            .await
            .map_err(|error| error.to_string())?;
        self.stdin
            .flush()
            .await
            .map_err(|error| error.to_string())?;
        while let Some(line) = self
            .stdout
            .next_line()
            .await
            .map_err(|error| error.to_string())?
        {
            let message: Value = serde_json::from_str(&line)
                .map_err(|error| format!("Invalid Grok history response: {error}"))?;
            if message.get("method").is_some() {
                if let Some(request_id) = message.get("id") {
                    // Browsing history never offers filesystem tools or permissions.
                    let response = json!({ "jsonrpc": "2.0", "id": request_id,
                        "error": { "code": -32601, "message": "Client method unavailable in history browser" } });
                    self.stdin
                        .write_all(format!("{response}\n").as_bytes())
                        .await
                        .map_err(|error| error.to_string())?;
                    self.stdin
                        .flush()
                        .await
                        .map_err(|error| error.to_string())?;
                }
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Grok history request failed")
                    .to_string());
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| "Grok history response has no result".to_string());
        }
        Err("Grok history connection closed".to_string())
    }

    async fn list(&mut self, cwd: &Path, limit: usize) -> Result<Vec<NativeSession>, String> {
        let clock = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let mut sessions = Vec::new();
        let mut cursor = None::<String>;
        loop {
            // This is Grok's native ACP listing, also used by T3 Code. It resolves
            // titles, last activity and session visibility in the provider.
            let mut params = json!({ "cwd": cwd.to_string_lossy() });
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let result = self.call("session/list", params).await?;
            let rows = result
                .get("sessions")
                .and_then(Value::as_array)
                .ok_or_else(|| "Grok session/list returned no session array".to_string())?;
            for row in rows {
                let id = row
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| "Grok returned a session without an ID".to_string())?;
                let title = row
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("Untitled Grok session");
                let updated = row.get("updatedAt").and_then(Value::as_str);
                let updated_at_ms = clock
                    .query_row(
                        "SELECT COALESCE(CAST(unixepoch(?1, 'subsec') * 1000 AS INTEGER), 0)",
                        params![updated],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(|error| error.to_string())?;
                sessions.push(NativeSession {
                    provider: "grok",
                    id: id.to_string(),
                    title: title.to_string(),
                    created_at_ms: updated_at_ms,
                    updated_at_ms,
                });
            }
            let next = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty());
            if sessions.len() >= limit || next.is_none() {
                break;
            }
            if next == cursor.as_deref() {
                return Err("Grok repeated a history pagination cursor".to_string());
            }
            cursor = next.map(str::to_string);
        }
        sessions.truncate(limit);
        Ok(sessions)
    }
}
