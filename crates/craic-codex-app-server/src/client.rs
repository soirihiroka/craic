use std::collections::HashMap;
use std::process::Command;
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
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::lifecycle::{AppServerConfig, AppServerError, AppServerEvent, ConnectionState};
use crate::protocol::ApprovalResponse;
use crate::protocol::AppsInstalledParams;
use crate::protocol::AppsListParams;
use crate::protocol::AppsReadParams;
use crate::protocol::ErrorResponse;
use crate::protocol::ExperimentalFeatureEnablementSetParams;
use crate::protocol::ExperimentalFeatureListParams;
use crate::protocol::GetAccountParams;
use crate::protocol::HooksListParams;
use crate::protocol::InitializeParams;
use crate::protocol::ListMcpServerStatusParams;
use crate::protocol::MarketplaceAddParams;
use crate::protocol::MarketplaceRemoveParams;
use crate::protocol::MarketplaceUpgradeParams;
use crate::protocol::McpResourceReadParams;
use crate::protocol::McpServerOauthLoginParams;
use crate::protocol::McpServerToolCallParams;
use crate::protocol::Notification;
use crate::protocol::PluginInstallParams;
use crate::protocol::PluginInstalledParams;
use crate::protocol::PluginListParams;
use crate::protocol::PluginReadParams;
use crate::protocol::PluginSkillReadParams;
use crate::protocol::PluginUninstallParams;
use crate::protocol::Request;
use crate::protocol::RequestId;
use crate::protocol::Response;
use crate::protocol::ReviewStartParams;
use crate::protocol::RpcError;
use crate::protocol::SkillsConfigWriteParams;
use crate::protocol::SkillsExtraRootsSetParams;
use crate::protocol::SkillsListParams;
use crate::protocol::ThreadArchiveParams;
use crate::protocol::ThreadBackgroundTerminalsCleanParams;
use crate::protocol::ThreadBackgroundTerminalsListParams;
use crate::protocol::ThreadBackgroundTerminalsTerminateParams;
use crate::protocol::ThreadCompactStartParams;
use crate::protocol::ThreadDeleteParams;
use crate::protocol::ThreadGoalClearParams;
use crate::protocol::ThreadGoalGetParams;
use crate::protocol::ThreadGoalSetParams;
use crate::protocol::ThreadListParams;
use crate::protocol::ThreadMetadataUpdateParams;
use crate::protocol::ThreadReadParams;
use crate::protocol::ThreadResumeParams;
use crate::protocol::ThreadRollbackParams;
use crate::protocol::ThreadSetNameParams;
use crate::protocol::ThreadSettingsUpdateParams;
use crate::protocol::ThreadShellCommandParams;
use crate::protocol::ThreadStartParams;
use crate::protocol::ThreadTurnsListParams;
use crate::protocol::ThreadUnarchiveParams;
use crate::protocol::ThreadUnsubscribeParams;
use crate::protocol::TurnInterruptParams;
use crate::protocol::TurnStartParams;
use crate::protocol::TurnSteerParams;
use crate::transport::{
    INITIALIZE_REQUEST_ID, ProcessCommand, WriterCommand, emit_event, enqueue_value, lock,
    run_process_waiter, run_stderr_reader, run_stdout_reader, run_writer, set_state,
};
use crate::version::check_codex_version;

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
            emitted_at_ms: None,
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

    pub fn thread_settings_update(
        &self,
        params: ThreadSettingsUpdateParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/settings/update", params)
    }

    pub fn thread_turns_list(
        &self,
        params: ThreadTurnsListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/turns/list", params)
    }

    pub fn thread_set_name(
        &self,
        params: ThreadSetNameParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/name/set", params)
    }

    pub fn thread_goal_set(
        &self,
        params: ThreadGoalSetParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/goal/set", params)
    }

    pub fn thread_goal_get(
        &self,
        params: ThreadGoalGetParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/goal/get", params)
    }

    pub fn thread_goal_clear(
        &self,
        params: ThreadGoalClearParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/goal/clear", params)
    }

    pub fn thread_archive(&self, params: ThreadArchiveParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/archive", params)
    }

    pub fn thread_unarchive(
        &self,
        params: ThreadUnarchiveParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/unarchive", params)
    }

    pub fn thread_delete(&self, params: ThreadDeleteParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/delete", params)
    }

    pub fn thread_unsubscribe(
        &self,
        params: ThreadUnsubscribeParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/unsubscribe", params)
    }

    pub fn thread_metadata_update(
        &self,
        params: ThreadMetadataUpdateParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/metadata/update", params)
    }

    pub fn thread_compact_start(
        &self,
        params: ThreadCompactStartParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/compact/start", params)
    }

    pub fn thread_shell_command(
        &self,
        params: ThreadShellCommandParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/shellCommand", params)
    }

    pub fn thread_background_terminals_clean(
        &self,
        params: ThreadBackgroundTerminalsCleanParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/backgroundTerminals/clean", params)
    }

    pub fn thread_background_terminals_list(
        &self,
        params: ThreadBackgroundTerminalsListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/backgroundTerminals/list", params)
    }

    pub fn thread_background_terminals_terminate(
        &self,
        params: ThreadBackgroundTerminalsTerminateParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/backgroundTerminals/terminate", params)
    }

    pub fn thread_rollback(
        &self,
        params: ThreadRollbackParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/rollback", params)
    }

    pub fn skills_list(&self, params: SkillsListParams) -> Result<RequestId, AppServerError> {
        self.send_request("skills/list", params)
    }

    pub fn skills_config_write(
        &self,
        params: SkillsConfigWriteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("skills/config/write", params)
    }

    pub fn skills_extra_roots_set(
        &self,
        params: SkillsExtraRootsSetParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("skills/extraRoots/set", params)
    }

    pub fn hooks_list(&self, params: HooksListParams) -> Result<RequestId, AppServerError> {
        self.send_request("hooks/list", params)
    }

    pub fn marketplace_add(
        &self,
        params: MarketplaceAddParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("marketplace/add", params)
    }

    pub fn marketplace_remove(
        &self,
        params: MarketplaceRemoveParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("marketplace/remove", params)
    }

    pub fn marketplace_upgrade(
        &self,
        params: MarketplaceUpgradeParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("marketplace/upgrade", params)
    }

    pub fn mcp_server_oauth_login(
        &self,
        params: McpServerOauthLoginParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("mcpServer/oauth/login", params)
    }

    pub fn mcp_server_status_list(
        &self,
        params: ListMcpServerStatusParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("mcpServerStatus/list", params)
    }

    pub fn mcp_server_reload(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("config/mcpServer/reload")
    }

    pub fn mcp_resource_read(
        &self,
        params: McpResourceReadParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("mcpServer/resource/read", params)
    }

    pub fn mcp_server_tool_call(
        &self,
        params: McpServerToolCallParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("mcpServer/tool/call", params)
    }

    pub fn apps_read(&self, params: AppsReadParams) -> Result<RequestId, AppServerError> {
        self.send_request("app/read", params)
    }

    pub fn apps_list(&self, params: AppsListParams) -> Result<RequestId, AppServerError> {
        self.send_request("app/list", params)
    }

    pub fn apps_installed(&self, params: AppsInstalledParams) -> Result<RequestId, AppServerError> {
        self.send_request("app/installed", params)
    }

    pub fn plugin_list(&self, params: PluginListParams) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/list", params)
    }

    pub fn plugin_installed(
        &self,
        params: PluginInstalledParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/installed", params)
    }

    pub fn plugin_read(&self, params: PluginReadParams) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/read", params)
    }

    pub fn plugin_skill_read(
        &self,
        params: PluginSkillReadParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/skill/read", params)
    }

    pub fn plugin_install(&self, params: PluginInstallParams) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/install", params)
    }

    pub fn plugin_uninstall(
        &self,
        params: PluginUninstallParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/uninstall", params)
    }

    pub fn experimental_feature_list(
        &self,
        params: ExperimentalFeatureListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("experimentalFeature/list", params)
    }

    pub fn experimental_feature_enablement_set(
        &self,
        params: ExperimentalFeatureEnablementSetParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("experimentalFeature/enablement/set", params)
    }

    pub fn account_read(&self, params: GetAccountParams) -> Result<RequestId, AppServerError> {
        self.send_request("account/read", params)
    }

    pub fn account_rate_limits_read(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("account/rateLimits/read")
    }

    pub fn account_usage_read(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("account/usage/read")
    }

    pub fn review_start(&self, params: ReviewStartParams) -> Result<RequestId, AppServerError> {
        self.send_request("review/start", params)
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
