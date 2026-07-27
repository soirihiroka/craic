use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::PathBuf;
use std::process::Child;
use std::process::ChildStderr;
use std::process::ChildStdout;
use std::process::Command;
use std::process::ExitStatus as ProcessExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::SyncSender;
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::TrySendError;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;

use crate::protocol::ApprovalResponse;
use crate::protocol::ClientInfo;
use crate::protocol::ErrorResponse;
use crate::protocol::InitializeCapabilities;
use crate::protocol::InitializeParams;
use crate::protocol::InitializeResult;
use crate::protocol::Notification;
use crate::protocol::Request;
use crate::protocol::RequestId;
use crate::protocol::Response;
use crate::protocol::RpcError;
use crate::protocol::ThreadListParams;
use crate::protocol::ThreadReadParams;
use crate::protocol::ThreadResumeParams;
use crate::protocol::ThreadStartParams;
use crate::protocol::TurnInterruptParams;
use crate::protocol::TurnStartParams;
use crate::protocol::TurnSteerParams;
use crate::version::VersionError;
use crate::version::check_codex_version;

const INITIALIZE_REQUEST_ID: i64 = 1;

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

enum WriterCommand {
    Value(Value),
    InitializeAck(InitializeResult),
    Shutdown,
}

enum ProcessCommand {
    GracefulShutdown(Duration),
    Terminate,
}

pub struct AppServer {
    command_tx: Option<SyncSender<WriterCommand>>,
    process_tx: Option<SyncSender<ProcessCommand>>,
    event_tx: mpsc::Sender<AppServerEvent>,
    event_queue_saturated: Arc<AtomicBool>,
    events_rx: Receiver<AppServerEvent>,
    state: Arc<Mutex<ConnectionState>>,
    pending: Arc<Mutex<HashMap<RequestId, String>>>,
    next_request_id: AtomicI64,
    threads: Vec<JoinHandle<()>>,
    shutdown_timeout: Duration,
}

impl AppServer {
    pub fn spawn(config: AppServerConfig) -> Result<Self, AppServerError> {
        if let Some((program, args)) = &config.version_command {
            let mut command = Command::new(program);
            command.args(args);
            if let Some(cwd) = &config.cwd {
                command.current_dir(cwd);
            }
            check_codex_version(&mut command).map_err(AppServerError::Version)?;
        }

        let capacity = config.channel_capacity.max(1);
        let state = Arc::new(Mutex::new(ConnectionState::Starting));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (command_tx, command_rx) = mpsc::sync_channel(capacity);
        let (event_tx, events_rx) = mpsc::channel();
        let (process_tx, process_rx) = mpsc::sync_channel(2);
        let event_queue_saturated = Arc::new(AtomicBool::new(false));

        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(AppServerError::Spawn)?;
        let process_group_id = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or(AppServerError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(AppServerError::MissingPipe("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or(AppServerError::MissingPipe("stderr"))?;

        emit_event(
            &event_tx,
            &event_queue_saturated,
            AppServerEvent::StateChanged(ConnectionState::Starting),
        );
        set_state(
            &state,
            &event_tx,
            &event_queue_saturated,
            ConnectionState::Initializing,
        );

        let writer_state = Arc::clone(&state);
        let writer_events = event_tx.clone();
        let writer_saturated = Arc::clone(&event_queue_saturated);
        let writer_process = process_tx.clone();
        let writer = thread::Builder::new()
            .name("codex-app-server-writer".to_owned())
            .spawn(move || {
                run_writer(
                    stdin,
                    command_rx,
                    writer_state,
                    writer_events,
                    writer_saturated,
                    writer_process,
                )
            })
            .map_err(AppServerError::Spawn)?;

        let reader_state = Arc::clone(&state);
        let reader_pending = Arc::clone(&pending);
        let reader_events = event_tx.clone();
        let reader_saturated = Arc::clone(&event_queue_saturated);
        let reader_commands = command_tx.clone();
        let reader_process = process_tx.clone();
        let reader = thread::Builder::new()
            .name("codex-app-server-reader".to_owned())
            .spawn(move || {
                run_stdout_reader(
                    stdout,
                    reader_commands,
                    reader_process,
                    reader_state,
                    reader_pending,
                    reader_events,
                    reader_saturated,
                )
            })
            .map_err(AppServerError::Spawn)?;

        let stderr_events = event_tx.clone();
        let stderr_saturated = Arc::clone(&event_queue_saturated);
        let stderr_reader = thread::Builder::new()
            .name("codex-app-server-stderr".to_owned())
            .spawn(move || run_stderr_reader(stderr, stderr_events, stderr_saturated))
            .map_err(AppServerError::Spawn)?;

        let process_state = Arc::clone(&state);
        let process_events = event_tx.clone();
        let process_saturated = Arc::clone(&event_queue_saturated);
        let waiter = thread::Builder::new()
            .name("codex-app-server-waiter".to_owned())
            .spawn(move || {
                run_process_waiter(
                    child,
                    process_rx,
                    process_state,
                    process_events,
                    process_saturated,
                    process_group_id,
                )
            })
            .map_err(AppServerError::Spawn)?;

        let initialize = Request {
            id: RequestId::Integer(INITIALIZE_REQUEST_ID),
            method: "initialize".to_owned(),
            params: Some(
                serde_json::to_value(InitializeParams {
                    client_info: config.client_info,
                    capabilities: Some(config.capabilities),
                })
                .map_err(AppServerError::Serialize)?,
            ),
            trace: None,
        };
        lock(&pending).insert(initialize.id.clone(), initialize.method.clone());
        enqueue_value(&command_tx, initialize).map_err(|error| {
            let _ = process_tx.try_send(ProcessCommand::Terminate);
            error
        })?;

        Ok(Self {
            command_tx: Some(command_tx),
            process_tx: Some(process_tx),
            event_tx,
            event_queue_saturated,
            events_rx,
            state,
            pending,
            next_request_id: AtomicI64::new(INITIALIZE_REQUEST_ID + 1),
            threads: vec![writer, reader, stderr_reader, waiter],
            shutdown_timeout: config.graceful_shutdown_timeout,
        })
    }

    pub fn state(&self) -> ConnectionState {
        *lock(&self.state)
    }

    pub fn events(&self) -> &Receiver<AppServerEvent> {
        &self.events_rx
    }

    pub fn try_recv(&self) -> Result<AppServerEvent, TryRecvError> {
        self.events_rx.try_recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<AppServerEvent, RecvTimeoutError> {
        self.events_rx.recv_timeout(timeout)
    }

    pub fn send_request<P: Serialize>(
        &self,
        method: impl Into<String>,
        params: P,
    ) -> Result<RequestId, AppServerError> {
        self.send_optional_request(method, Some(params))
    }

    pub fn send_request_without_params(
        &self,
        method: impl Into<String>,
    ) -> Result<RequestId, AppServerError> {
        self.send_optional_request::<Value>(method, None)
    }

    pub fn send_raw_request(
        &self,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<RequestId, AppServerError> {
        self.ensure_ready()?;
        let method = method.into();
        let id = self.next_request_id()?;
        let request = Request {
            id: id.clone(),
            method: method.clone(),
            params,
            trace: None,
        };
        lock(&self.pending).insert(id.clone(), method);
        if let Err(error) = self.enqueue(request) {
            lock(&self.pending).remove(&id);
            return Err(error);
        }
        Ok(id)
    }

    pub fn send_notification<P: Serialize>(
        &self,
        method: impl Into<String>,
        params: P,
    ) -> Result<(), AppServerError> {
        self.ensure_ready()?;
        self.enqueue(Notification {
            method: method.into(),
            params: Some(serde_json::to_value(params).map_err(AppServerError::Serialize)?),
        })
    }

    pub fn respond(&self, id: RequestId, result: Value) -> Result<(), AppServerError> {
        self.ensure_ready()?;
        self.enqueue(Response { id, result })
    }

    pub fn respond_error(&self, id: RequestId, error: RpcError) -> Result<(), AppServerError> {
        self.ensure_ready()?;
        self.enqueue(ErrorResponse { id, error })
    }

    pub fn respond_approval(&self, id: RequestId, decision: Value) -> Result<(), AppServerError> {
        self.respond(
            id,
            serde_json::to_value(ApprovalResponse { decision })
                .map_err(AppServerError::Serialize)?,
        )
    }

    pub fn thread_start(&self, params: ThreadStartParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/start", params)
    }

    pub fn thread_resume(&self, params: ThreadResumeParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/resume", params)
    }

    pub fn thread_list(&self, params: ThreadListParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/list", params)
    }

    pub fn thread_read(&self, params: ThreadReadParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/read", params)
    }

    pub fn turn_start(&self, params: TurnStartParams) -> Result<RequestId, AppServerError> {
        self.send_request("turn/start", params)
    }

    pub fn turn_steer(&self, params: TurnSteerParams) -> Result<RequestId, AppServerError> {
        self.send_request("turn/steer", params)
    }

    pub fn turn_interrupt(&self, params: TurnInterruptParams) -> Result<RequestId, AppServerError> {
        self.send_request("turn/interrupt", params)
    }

    pub fn shutdown(&mut self) {
        if self.threads.is_empty() {
            return;
        }
        let current = self.state();
        if !matches!(current, ConnectionState::Stopped | ConnectionState::Crashed) {
            *lock(&self.state) = ConnectionState::Stopping;
            log::info!("Codex App Server lifecycle: {current:?} -> Stopping");
            emit_event(
                &self.event_tx,
                &self.event_queue_saturated,
                AppServerEvent::StateChanged(ConnectionState::Stopping),
            );
        }

        if let Some(command_tx) = self.command_tx.take() {
            let _ = command_tx.send(WriterCommand::Shutdown);
        }
        if let Some(process_tx) = self.process_tx.take() {
            let _ = process_tx.send(ProcessCommand::GracefulShutdown(self.shutdown_timeout));
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }

    fn send_optional_request<P: Serialize>(
        &self,
        method: impl Into<String>,
        params: Option<P>,
    ) -> Result<RequestId, AppServerError> {
        let params = params
            .map(serde_json::to_value)
            .transpose()
            .map_err(AppServerError::Serialize)?;
        self.send_raw_request(method, params)
    }

    fn ensure_ready(&self) -> Result<(), AppServerError> {
        let state = self.state();
        if state != ConnectionState::Ready {
            return Err(AppServerError::NotReady(state));
        }
        Ok(())
    }

    fn next_request_id(&self) -> Result<RequestId, AppServerError> {
        self.next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(RequestId::Integer)
            .map_err(|_| AppServerError::RequestIdExhausted)
    }

    fn enqueue<T: Serialize>(&self, message: T) -> Result<(), AppServerError> {
        let command_tx = self
            .command_tx
            .as_ref()
            .ok_or(AppServerError::Disconnected)?;
        enqueue_value(command_tx, message)
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn enqueue_value<T: Serialize>(
    sender: &SyncSender<WriterCommand>,
    message: T,
) -> Result<(), AppServerError> {
    let value = serde_json::to_value(message).map_err(AppServerError::Serialize)?;
    sender
        .try_send(WriterCommand::Value(value))
        .map_err(|error| match error {
            TrySendError::Full(_) => AppServerError::CommandQueueFull,
            TrySendError::Disconnected(_) => AppServerError::Disconnected,
        })
}

fn run_writer(
    mut stdin: impl Write,
    commands: Receiver<WriterCommand>,
    state: Arc<Mutex<ConnectionState>>,
    events: mpsc::Sender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
    process: SyncSender<ProcessCommand>,
) {
    while let Ok(command) = commands.recv() {
        match command {
            WriterCommand::Value(value) => {
                if let Err(error) = write_json_line(&mut stdin, &value) {
                    transport_failed(&state, &events, &saturated, &process, error);
                    break;
                }
            }
            WriterCommand::InitializeAck(initialize) => {
                let initialized = serde_json::json!({ "method": "initialized" });
                if let Err(error) = write_json_line(&mut stdin, &initialized) {
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

fn write_json_line(writer: &mut impl Write, value: &Value) -> Result<(), std::io::Error> {
    serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[allow(clippy::too_many_arguments)]
fn run_stdout_reader(
    stdout: ChildStdout,
    commands: SyncSender<WriterCommand>,
    process: SyncSender<ProcessCommand>,
    state: Arc<Mutex<ConnectionState>>,
    pending: Arc<Mutex<HashMap<RequestId, String>>>,
    events: mpsc::Sender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
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
    commands: &SyncSender<WriterCommand>,
    process: &SyncSender<ProcessCommand>,
    state: &Arc<Mutex<ConnectionState>>,
    pending: &Arc<Mutex<HashMap<RequestId, String>>>,
    events: &mpsc::Sender<AppServerEvent>,
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
                            if commands
                                .send(WriterCommand::InitializeAck(initialize))
                                .is_err()
                            {
                                initialization_failed(
                                    state,
                                    events,
                                    saturated,
                                    process,
                                    "writer disconnected before initialized acknowledgement",
                                );
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

fn run_stderr_reader(
    stderr: ChildStderr,
    events: mpsc::Sender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stderr);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
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

fn run_process_waiter(
    mut child: Child,
    commands: Receiver<ProcessCommand>,
    state: Arc<Mutex<ConnectionState>>,
    events: mpsc::Sender<AppServerEvent>,
    saturated: Arc<AtomicBool>,
    process_group_id: u32,
) {
    let mut shutdown_deadline = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
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
            Ok(None) => {}
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
        }

        if shutdown_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            if let Err(error) = terminate_process_group(&mut child, process_group_id) {
                emit_event(
                    &events,
                    &saturated,
                    AppServerEvent::ProtocolError(format!(
                        "failed to terminate App Server: {error}"
                    )),
                );
            }
            let _ = child.wait();
            continue;
        }

        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(ProcessCommand::GracefulShutdown(timeout)) => {
                shutdown_deadline = Some(Instant::now() + timeout);
            }
            Ok(ProcessCommand::Terminate) => {
                let _ = terminate_process_group(&mut child, process_group_id);
                let _ = child.wait();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                shutdown_deadline.get_or_insert(Instant::now() + Duration::from_secs(2));
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
        child.kill()
    }
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child, _process_group_id: u32) -> std::io::Result<()> {
    child.kill()
}

fn transport_failed(
    state: &Arc<Mutex<ConnectionState>>,
    events: &mpsc::Sender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    process: &SyncSender<ProcessCommand>,
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
    events: &mpsc::Sender<AppServerEvent>,
    saturated: &Arc<AtomicBool>,
    process: &SyncSender<ProcessCommand>,
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

fn set_state(
    state: &Arc<Mutex<ConnectionState>>,
    events: &mpsc::Sender<AppServerEvent>,
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

fn emit_event(
    sender: &mpsc::Sender<AppServerEvent>,
    saturated: &AtomicBool,
    event: AppServerEvent,
) {
    let _ = sender.send(event);
    saturated.store(false, Ordering::Relaxed);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
