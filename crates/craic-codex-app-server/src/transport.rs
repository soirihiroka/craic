use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError as StdTrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::mpsc::{Receiver, Sender, error::TrySendError};
use tokio::time::{Instant, sleep_until};

use crate::lifecycle::{AppServerError, AppServerEvent, ConnectionState};
use crate::protocol::{
    ErrorResponse, InitializeResult, Notification, Request, RequestId, Response,
};

pub(crate) const INITIALIZE_REQUEST_ID: i64 = 1;

pub(crate) enum WriterCommand {
    Value(Value),
    InitializeAck(InitializeResult),
    Shutdown,
}

pub(crate) enum ProcessCommand {
    GracefulShutdown(Duration),
    Terminate,
}

pub(crate) fn enqueue_value<T: Serialize>(
    sender: &Sender<WriterCommand>,
    message: T,
) -> Result<(), AppServerError> {
    let value = serde_json::to_value(message).map_err(AppServerError::Serialize)?;
    sender
        .try_send(WriterCommand::Value(value))
        .map_err(|error| match error {
            TrySendError::Full(_) => AppServerError::CommandQueueFull,
            TrySendError::Closed(_) => AppServerError::Disconnected,
        })
}

pub(crate) async fn run_writer(
    mut stdin: impl AsyncWrite + Unpin,
    mut commands: Receiver<WriterCommand>,
    state: Arc<Mutex<ConnectionState>>,
    events: SyncSender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
    process: Sender<ProcessCommand>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            WriterCommand::Value(value) => {
                if let Err(error) = write_json_line(&mut stdin, &value).await {
                    transport_failed(&state, &events, &saturated, &process, error);
                    break;
                }
            }
            WriterCommand::InitializeAck(initialize) => {
                let initialized = serde_json::json!({ "method": "initialized" });
                if let Err(error) = write_json_line(&mut stdin, &initialized).await {
                    transport_failed(&state, &events, &saturated, &process, error);
                    break;
                }
                set_state(&state, &events, &saturated, ConnectionState::Ready);
                emit_event(&events, &saturated, AppServerEvent::Ready(initialize));
            }
            WriterCommand::Shutdown => break,
        }
    }
}

async fn write_json_line(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), std::io::Error> {
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_stdout_reader(
    stdout: ChildStdout,
    commands: Sender<WriterCommand>,
    process: Sender<ProcessCommand>,
    state: Arc<Mutex<ConnectionState>>,
    pending: Arc<Mutex<HashMap<RequestId, String>>>,
    events: SyncSender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "failed reading App Server stdout: {error}"
                    )),
                );
                break;
            }
        }
        while matches!(buffer.last(), Some(b'\n' | b'\r')) {
            buffer.pop();
        }
        if buffer.is_empty() {
            continue;
        }
        let value = match serde_json::from_slice::<Value>(&buffer) {
            Ok(value) => value,
            Err(error) => {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "malformed App Server JSONL message: {error}"
                    )),
                );
                continue;
            }
        };
        route_message(
            value, &commands, &process, &state, &pending, &events, &saturated,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn route_message(
    value: Value,
    commands: &Sender<WriterCommand>,
    process: &Sender<ProcessCommand>,
    state: &Arc<Mutex<ConnectionState>>,
    pending: &Arc<Mutex<HashMap<RequestId, String>>>,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
) {
    let has_method = value.get("method").is_some();
    let has_id = value.get("id").is_some();
    if has_method && has_id {
        match serde_json::from_value::<Request>(value) {
            Ok(request) => emit_event(events, saturated, AppServerEvent::ServerRequest(request)),
            Err(error) => emit_event(
                events,
                saturated,
                AppServerEvent::ProtocolError(format!("invalid server request: {error}")),
            ),
        }
    } else if has_method {
        match serde_json::from_value::<Notification>(value) {
            Ok(notification) => emit_event(
                events,
                saturated,
                AppServerEvent::Notification(notification),
            ),
            Err(error) => emit_event(
                events,
                saturated,
                AppServerEvent::ProtocolError(format!("invalid notification: {error}")),
            ),
        }
    } else if value.get("result").is_some() && has_id {
        match serde_json::from_value::<Response>(value) {
            Ok(response) => {
                let method = lock(pending).remove(&response.id);
                if response.id == RequestId::Integer(INITIALIZE_REQUEST_ID) {
                    match serde_json::from_value::<InitializeResult>(response.result.clone()) {
                        Ok(initialize) => {
                            if let Err(error) =
                                commands.try_send(WriterCommand::InitializeAck(initialize))
                            {
                                let message = match error {
                                    TrySendError::Full(_) => {
                                        "writer queue full before initialized acknowledgement"
                                    }
                                    TrySendError::Closed(_) => {
                                        "writer disconnected before initialized acknowledgement"
                                    }
                                };
                                initialization_failed(state, events, saturated, process, message);
                            }
                        }
                        Err(error) => initialization_failed(
                            state,
                            events,
                            saturated,
                            process,
                            &format!("invalid initialize response: {error}"),
                        ),
                    }
                }
                emit_event(
                    events,
                    saturated,
                    AppServerEvent::Response { response, method },
                );
            }
            Err(error) => emit_event(
                events,
                saturated,
                AppServerEvent::ProtocolError(format!("invalid response: {error}")),
            ),
        }
    } else if value.get("error").is_some() && has_id {
        match serde_json::from_value::<ErrorResponse>(value) {
            Ok(response) => {
                let method = lock(pending).remove(&response.id);
                if response.id == RequestId::Integer(INITIALIZE_REQUEST_ID) {
                    initialization_failed(
                        state,
                        events,
                        saturated,
                        process,
                        &format!("initialize failed: {}", response.error.message),
                    );
                }
                emit_event(
                    events,
                    saturated,
                    AppServerEvent::ErrorResponse { response, method },
                );
            }
            Err(error) => emit_event(
                events,
                saturated,
                AppServerEvent::ProtocolError(format!("invalid error response: {error}")),
            ),
        }
    } else {
        emit_event(
            events,
            saturated,
            AppServerEvent::ProtocolError("unrecognized App Server envelope".to_owned()),
        );
    }
}

pub(crate) async fn run_stderr_reader(
    stderr: ChildStderr,
    events: SyncSender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => break,
            Ok(_) => {
                let diagnostic = String::from_utf8_lossy(&buffer)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned();
                if !diagnostic.is_empty() {
                    emit_event(&events, &saturated, AppServerEvent::Diagnostic(diagnostic));
                }
            }
            Err(error) => {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "failed reading App Server stderr: {error}"
                    )),
                );
                break;
            }
        }
    }
}

pub(crate) async fn run_process_waiter(
    mut child: Child,
    mut commands: Receiver<ProcessCommand>,
    state: Arc<Mutex<ConnectionState>>,
    events: SyncSender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
    process_group_id: u32,
) {
    let mut shutdown_deadline: Option<Instant> = None;
    let mut commands_open = true;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = match status {
                    Ok(status) => status,
                    Err(error) => {
                        set_state(&state, &events, &saturated, ConnectionState::Crashed);
                        emit_event(
                            &events,
                            &saturated,
                            AppServerEvent::ProtocolError(format!(
                                "failed waiting for App Server: {error}"
                            )),
                        );
                        break;
                    }
                };
                let target = if *lock(&state) == ConnectionState::Stopping {
                    ConnectionState::Stopped
                } else {
                    ConnectionState::Crashed
                };
                set_state(&state, &events, &saturated, target);
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProcessExited(status.into()),
                );
                break;
            }
            command = commands.recv(), if commands_open => {
                match command {
                    Some(ProcessCommand::GracefulShutdown(timeout)) => {
                        shutdown_deadline = Some(Instant::now() + timeout);
                    }
                    Some(ProcessCommand::Terminate) => {
                        if let Err(error) = terminate_process_group(&mut child, process_group_id) {
                            emit_event(
                                &events,
                                &saturated,
                                AppServerEvent::ProtocolError(format!(
                                    "failed to terminate App Server: {error}"
                                )),
                            );
                        }
                    }
                    None => {
                        commands_open = false;
                        shutdown_deadline
                            .get_or_insert(Instant::now() + Duration::from_secs(2));
                    }
                }
            }
            _ = async {
                if let Some(deadline) = shutdown_deadline {
                    sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                shutdown_deadline = None;
                if let Err(error) = terminate_process_group(&mut child, process_group_id) {
                    emit_event(
                        &events,
                        &saturated,
                        AppServerEvent::ProtocolError(format!(
                            "failed to terminate App Server after shutdown deadline: {error}"
                        )),
                    );
                }
            }
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child, process_group_id: u32) -> std::io::Result<()> {
    let process_group_id = i32::try_from(process_group_id).map_err(std::io::Error::other)?;
    // SAFETY: the child was placed in a new process group whose id is its pid immediately before
    // spawn. A negative pid targets that group and cannot target Craic's process group.
    if unsafe { libc::kill(-process_group_id, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        child.start_kill()
    }
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child, _process_group_id: u32) -> std::io::Result<()> {
    child.start_kill()
}

fn transport_failed(
    state: &Arc<Mutex<ConnectionState>>,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    process: &Sender<ProcessCommand>,
    error: std::io::Error,
) {
    set_state(state, events, saturated, ConnectionState::Crashed);
    emit_event(
        events,
        saturated,
        AppServerEvent::ProtocolError(format!("App Server stdin failed: {error}")),
    );
    let _ = process.try_send(ProcessCommand::Terminate);
}

fn initialization_failed(
    state: &Arc<Mutex<ConnectionState>>,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    process: &Sender<ProcessCommand>,
    message: &str,
) {
    set_state(state, events, saturated, ConnectionState::Crashed);
    emit_event(
        events,
        saturated,
        AppServerEvent::ProtocolError(message.to_owned()),
    );
    let _ = process.try_send(ProcessCommand::Terminate);
}

pub(crate) fn set_state(
    state: &Arc<Mutex<ConnectionState>>,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    next: ConnectionState,
) {
    let mut current = lock(state);
    if *current == next {
        return;
    }
    log::info!("Codex App Server lifecycle: {:?} -> {next:?}", *current);
    *current = next;
    drop(current);
    emit_event(events, saturated, AppServerEvent::StateChanged(next));
}

pub(crate) fn emit_event(
    sender: &SyncSender<AppServerEvent>,
    saturated: &AtomicBool,
    event: AppServerEvent,
) {
    match sender.try_send(event) {
        Ok(()) | Err(StdTrySendError::Disconnected(_)) => {}
        Err(StdTrySendError::Full(event)) => {
            if !saturated.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "Codex App Server event queue is saturated; applying transport backpressure"
                );
            }
            // Shutdown drops the event receiver before joining the runtime, which wakes this send.
            // On the transport's multithreaded Tokio runtime, block_in_place keeps other async I/O
            // tasks schedulable while the bounded compatibility receiver applies backpressure.
            if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
                handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
            }) {
                let _ = tokio::task::block_in_place(|| sender.send(event));
            } else {
                let _ = sender.send(event);
            }
        }
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
