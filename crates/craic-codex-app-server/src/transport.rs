use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError as StdTrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::mpsc::{Receiver, Sender, error::TrySendError};
use tokio::time::{Instant, sleep_until};
use tokio_util::sync::CancellationToken;

use crate::lifecycle::{AppServerError, AppServerEvent, ConnectionState};
use crate::protocol::{
    ErrorResponse, InitializeResult, Notification, Request, RequestId, Response,
};

pub(crate) const INITIALIZE_REQUEST_ID: i64 = 1;
const MAX_JSONL_FRAME_BYTES: usize = 32 * 1024 * 1024;

enum FrameRead {
    Frame(Vec<u8>),
    Oversized,
    Cancelled,
    Eof,
}

enum ParsedMessage {
    ServerRequest(Request),
    Notification(Notification),
    Response {
        response: Response,
        initialize: Option<Result<InitializeResult, String>>,
    },
    ErrorResponse(ErrorResponse),
    ProtocolError(String),
}

pub(crate) enum WriterCommand {
    Message(OutboundMessage),
    InitializeAck(InitializeResult),
    Shutdown,
}

pub(crate) enum OutboundMessage {
    Request(Request),
    Notification(Notification),
    Response(Response),
    ErrorResponse(ErrorResponse),
    Initialized,
}

pub(crate) enum ProcessCommand {
    GracefulShutdown(Duration),
    Terminate,
}

pub(crate) fn enqueue_command(
    sender: &Sender<WriterCommand>,
    command: WriterCommand,
) -> Result<(), AppServerError> {
    sender.try_send(command).map_err(|error| match error {
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
    cancellation: CancellationToken,
) {
    loop {
        let command = tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            command = commands.recv() => match command {
                Some(command) => command,
                None => break,
            },
        };
        match command {
            WriterCommand::Message(message) => {
                match write_json_line(&mut stdin, message, &cancellation).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        transport_failed(&state, &events, &saturated, &process, error);
                        break;
                    }
                }
            }
            WriterCommand::InitializeAck(initialize) => {
                match write_json_line(&mut stdin, OutboundMessage::Initialized, &cancellation).await
                {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        transport_failed(&state, &events, &saturated, &process, error);
                        break;
                    }
                }
                if cancellation.is_cancelled() {
                    break;
                }
                let mut current = lock(&state);
                if cancellation.is_cancelled() || *current != ConnectionState::Initializing {
                    break;
                }
                log::info!("Codex App Server lifecycle: {:?} -> Ready", *current);
                *current = ConnectionState::Ready;
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::StateChanged(ConnectionState::Ready),
                );
                emit_event(&events, &saturated, AppServerEvent::Ready(initialize));
            }
            WriterCommand::Shutdown => break,
        }
    }
}

async fn write_json_line(
    writer: &mut (impl AsyncWrite + Unpin),
    message: OutboundMessage,
    cancellation: &CancellationToken,
) -> Result<bool, std::io::Error> {
    // Large request parameters can make serialization expensive. Keep that owned work off the I/O
    // workers and await it here so writes remain ordered and shutdown joins the work.
    let serialized = tokio::task::spawn_blocking(move || {
        let mut bytes = match message {
            OutboundMessage::Request(message) => serde_json::to_vec(&message),
            OutboundMessage::Notification(message) => serde_json::to_vec(&message),
            OutboundMessage::Response(message) => serde_json::to_vec(&message),
            OutboundMessage::ErrorResponse(message) => serde_json::to_vec(&message),
            OutboundMessage::Initialized => {
                serde_json::to_vec(&serde_json::json!({ "method": "initialized" }))
            }
        }
        .map_err(std::io::Error::other)?;
        if bytes.len() > MAX_JSONL_FRAME_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "serialized App Server JSONL message exceeds the {} byte limit",
                    MAX_JSONL_FRAME_BYTES
                ),
            ));
        }
        bytes.push(b'\n');
        Ok(bytes)
    })
    .await;
    if cancellation.is_cancelled() {
        return Ok(false);
    }
    let bytes = serialized.map_err(|error| {
        std::io::Error::other(format!("JSONL serializer task failed: {error}"))
    })??;
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Ok(false),
        result = async {
            writer.write_all(&bytes).await?;
            writer.flush().await
        } => result.map(|()| true),
    }
}

async fn read_jsonl_frame(
    reader: &mut (impl AsyncBufRead + Unpin),
    buffer: &mut Vec<u8>,
    cancellation: &CancellationToken,
) -> Result<FrameRead, std::io::Error> {
    buffer.clear();
    let mut oversized = false;
    loop {
        let available = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Ok(FrameRead::Cancelled),
            result = reader.fill_buf() => result?,
        };
        if available.is_empty() {
            return Ok(if oversized {
                FrameRead::Oversized
            } else if buffer.is_empty() {
                FrameRead::Eof
            } else {
                FrameRead::Frame(std::mem::take(buffer))
            });
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let content_len = newline.unwrap_or(available.len());
        if !oversized {
            if buffer.len().saturating_add(content_len) > MAX_JSONL_FRAME_BYTES {
                oversized = true;
                buffer.clear();
            } else {
                buffer.extend_from_slice(&available[..content_len]);
            }
        }
        let consumed = content_len + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(if oversized {
                FrameRead::Oversized
            } else {
                FrameRead::Frame(std::mem::take(buffer))
            });
        }
    }
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
    cancellation: CancellationToken,
) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::with_capacity(8 * 1024);
    loop {
        let mut frame = match read_jsonl_frame(&mut reader, &mut buffer, &cancellation).await {
            Ok(FrameRead::Cancelled | FrameRead::Eof) => break,
            Ok(FrameRead::Frame(frame)) => frame,
            Ok(FrameRead::Oversized) => {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "App Server JSONL message exceeds the {} byte limit",
                        MAX_JSONL_FRAME_BYTES
                    )),
                );
                continue;
            }
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
        };
        while matches!(frame.last(), Some(b'\r')) {
            frame.pop();
        }
        if frame.is_empty() {
            continue;
        }
        if cancellation.is_cancelled() {
            break;
        }
        let parsed = tokio::task::spawn_blocking(move || parse_message(frame)).await;
        // Never abandon an in-flight blocking parser. Once it is joined, cancellation wins before
        // the decoded message can mutate pending-request or connection state.
        if cancellation.is_cancelled() {
            break;
        }
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "App Server JSONL parser task failed: {error}"
                    )),
                );
                let _ = process.try_send(ProcessCommand::Terminate);
                break;
            }
        };
        route_message(
            parsed,
            &commands,
            &process,
            &state,
            &pending,
            &events,
            &saturated,
            &cancellation,
        );
    }
}

fn parse_message(frame: Vec<u8>) -> ParsedMessage {
    let value = match serde_json::from_slice::<Value>(&frame) {
        Ok(value) => value,
        Err(error) => {
            return ParsedMessage::ProtocolError(format!(
                "malformed App Server JSONL message: {error}"
            ));
        }
    };
    let has_method = value.get("method").is_some();
    let has_id = value.get("id").is_some();
    if has_method && has_id {
        match serde_json::from_value::<Request>(value) {
            Ok(request) => ParsedMessage::ServerRequest(request),
            Err(error) => ParsedMessage::ProtocolError(format!("invalid server request: {error}")),
        }
    } else if has_method {
        match serde_json::from_value::<Notification>(value) {
            Ok(notification) => ParsedMessage::Notification(notification),
            Err(error) => ParsedMessage::ProtocolError(format!("invalid notification: {error}")),
        }
    } else if value.get("result").is_some() && has_id {
        match serde_json::from_value::<Response>(value) {
            Ok(response) => {
                let initialize =
                    (response.id == RequestId::Integer(INITIALIZE_REQUEST_ID)).then(|| {
                        serde_json::from_value::<InitializeResult>(response.result.clone())
                            .map_err(|error| format!("invalid initialize response: {error}"))
                    });
                ParsedMessage::Response {
                    response,
                    initialize,
                }
            }
            Err(error) => ParsedMessage::ProtocolError(format!("invalid response: {error}")),
        }
    } else if value.get("error").is_some() && has_id {
        match serde_json::from_value::<ErrorResponse>(value) {
            Ok(response) => ParsedMessage::ErrorResponse(response),
            Err(error) => ParsedMessage::ProtocolError(format!("invalid error response: {error}")),
        }
    } else {
        ParsedMessage::ProtocolError("unrecognized App Server envelope".to_owned())
    }
}

#[allow(clippy::too_many_arguments)]
fn route_message(
    message: ParsedMessage,
    commands: &Sender<WriterCommand>,
    process: &Sender<ProcessCommand>,
    state: &Arc<Mutex<ConnectionState>>,
    pending: &Arc<Mutex<HashMap<RequestId, String>>>,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    cancellation: &CancellationToken,
) {
    let mut current = lock(state);
    if cancellation.is_cancelled()
        || matches!(
            *current,
            ConnectionState::Stopped | ConnectionState::Crashed
        )
    {
        return;
    }
    match message {
        ParsedMessage::ServerRequest(request) => {
            emit_event(events, saturated, AppServerEvent::ServerRequest(request));
        }
        ParsedMessage::Notification(notification) => {
            emit_event(
                events,
                saturated,
                AppServerEvent::Notification(notification),
            );
        }
        ParsedMessage::Response {
            response,
            initialize,
        } => {
            let method = lock(pending).remove(&response.id);
            if let Some(initialize) = initialize {
                match initialize {
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
                            initialization_failed(
                                &mut current,
                                events,
                                saturated,
                                process,
                                message,
                            );
                        }
                    }
                    Err(error) => {
                        initialization_failed(&mut current, events, saturated, process, &error)
                    }
                }
            }
            emit_event(
                events,
                saturated,
                AppServerEvent::Response { response, method },
            );
        }
        ParsedMessage::ErrorResponse(response) => {
            let method = lock(pending).remove(&response.id);
            if response.id == RequestId::Integer(INITIALIZE_REQUEST_ID) {
                initialization_failed(
                    &mut current,
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
        ParsedMessage::ProtocolError(error) => {
            emit_event(events, saturated, AppServerEvent::ProtocolError(error));
        }
    }
}

pub(crate) async fn run_stderr_reader(
    stderr: ChildStderr,
    events: SyncSender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
    cancellation: CancellationToken,
) {
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let read = tokio::select! {
            biased;
            _ = cancellation.cancelled() => break,
            result = reader.read_until(b'\n', &mut buffer) => result,
        };
        if cancellation.is_cancelled() {
            break;
        }
        match read {
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
    state: &mut ConnectionState,
    events: &SyncSender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    process: &Sender<ProcessCommand>,
    message: &str,
) {
    if *state != ConnectionState::Crashed {
        log::info!("Codex App Server lifecycle: {:?} -> Crashed", *state);
        *state = ConnectionState::Crashed;
        emit_event(
            events,
            saturated,
            AppServerEvent::StateChanged(ConnectionState::Crashed),
        );
    }
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
