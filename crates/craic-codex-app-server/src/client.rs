use std::collections::HashMap;
use std::process::Command as StdCommand;
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
use tokio::sync::mpsc::Sender;

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
use crate::protocol::{
    CancelLoginAccountParams, CollaborationModeListParams, CommandExecParams,
    CommandExecResizeParams, CommandExecTerminateParams, CommandExecWriteParams,
    ConfigBatchWriteParams, ConfigReadParams, ConfigValueWriteParams, EnvironmentAddParams,
    EnvironmentInfoParams, EnvironmentStatusParams, FsCopyParams, FsCreateDirectoryParams,
    FsGetMetadataParams, FsReadDirectoryParams, FsReadFileParams, FsRemoveParams, FsUnwatchParams,
    FsWatchParams, FsWriteFileParams, FuzzyFileSearchParams, FuzzyFileSearchSessionStartParams,
    FuzzyFileSearchSessionStopParams, FuzzyFileSearchSessionUpdateParams, GetAuthStatusParams,
    GetConversationSummaryParams, GitDiffToRemoteParams, LoginAccountParams, ModelListParams,
    ModelProviderCapabilitiesReadParams, PermissionProfileListParams, ProcessKillParams,
    ProcessResizePtyParams, ProcessSpawnParams, ProcessWriteStdinParams,
};
use crate::protocol::{
    ConsumeAccountRateLimitResetCreditParams, ExternalAgentConfigDetectParams,
    ExternalAgentConfigImportParams, FeedbackUploadParams, MockExperimentalMethodParams,
    PluginShareCheckoutParams, PluginShareDeleteParams, PluginShareListParams,
    PluginShareSaveParams, PluginShareUpdateTargetsParams, RemoteControlClientsListParams,
    RemoteControlClientsRevokeParams, RemoteControlDisableParams, RemoteControlEnableParams,
    RemoteControlPairingStartParams, RemoteControlPairingStatusParams,
    SendAddCreditsNudgeEmailParams, ThreadApproveGuardianDeniedActionParams,
    ThreadDecrementElicitationParams, ThreadForkParams, ThreadIncrementElicitationParams,
    ThreadInjectItemsParams, ThreadItemsListParams, ThreadLoadedListParams,
    ThreadMemoryModeSetParams, ThreadRealtimeAppendAudioParams, ThreadRealtimeAppendSpeechParams,
    ThreadRealtimeAppendTextParams, ThreadRealtimeListVoicesParams, ThreadRealtimeStartParams,
    ThreadRealtimeStopParams, ThreadSearchOccurrencesParams, ThreadSearchParams,
    WindowsSandboxSetupStartParams,
};
use crate::transport::{
    INITIALIZE_REQUEST_ID, ProcessCommand, WriterCommand, emit_event, enqueue_value, lock,
    run_process_waiter, run_stderr_reader, run_stdout_reader, run_writer, set_state,
};
use crate::version::check_codex_version;

pub struct AppServer {
    command_tx: Option<Sender<WriterCommand>>,
    process_tx: Option<Sender<ProcessCommand>>,
    event_tx: SyncSender<AppServerEvent>,
    event_queue_saturated: Arc<AtomicBool>,
    events_rx: Receiver<AppServerEvent>,
    state: Arc<Mutex<ConnectionState>>,
    pending: Arc<Mutex<HashMap<RequestId, String>>>,
    next_request_id: AtomicI64,
    supervisor: Option<JoinHandle<()>>,
    shutdown_timeout: Duration,
}

impl AppServer {
    pub fn spawn(config: AppServerConfig) -> Result<Self, AppServerError> {
        if let Some((program, args)) = &config.version_command {
            let mut command = StdCommand::new(program);
            command.args(args);
            if let Some(cwd) = &config.cwd {
                command.current_dir(cwd);
            }
            check_codex_version(&mut command).map_err(AppServerError::Version)?;
        }

        let command_capacity = config.channel_capacity.max(1);
        // Startup publishes Starting and Initializing before the receiver can be returned to the
        // caller, so the lossless event queue must hold at least those two lifecycle events.
        let event_capacity = config.channel_capacity.max(2);
        let state = Arc::new(Mutex::new(ConnectionState::Starting));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(command_capacity);
        let (event_tx, events_rx) = mpsc::sync_channel(event_capacity);
        let (process_tx, process_rx) = tokio::sync::mpsc::channel(2);
        let event_queue_saturated = Arc::new(AtomicBool::new(false));

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
        let program = config.program;
        let args = config.args;
        let cwd = config.cwd;
        let shutdown_timeout = config.graceful_shutdown_timeout;
        let runtime_state = Arc::clone(&state);
        let runtime_pending = Arc::clone(&pending);
        let runtime_events = event_tx.clone();
        let runtime_saturated = Arc::clone(&event_queue_saturated);
        let runtime_commands = command_tx.clone();
        let runtime_process = process_tx.clone();
        let (startup_tx, startup_rx) = mpsc::sync_channel(1);
        let supervisor = thread::Builder::new()
            .name("codex-app-server-runtime".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("codex-app-server-io")
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = startup_tx.send(Err(AppServerError::Spawn(error)));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let mut command = tokio::process::Command::new(program);
                    command
                        .args(args)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true);
                    #[cfg(unix)]
                    command.process_group(0);
                    if let Some(cwd) = cwd {
                        command.current_dir(cwd);
                    }
                    let mut child = match command.spawn() {
                        Ok(child) => child,
                        Err(error) => {
                            let _ = startup_tx.send(Err(AppServerError::Spawn(error)));
                            return;
                        }
                    };
                    let Some(process_group_id) = child.id() else {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let _ = startup_tx.send(Err(AppServerError::Spawn(std::io::Error::other(
                            "Codex App Server did not report a process ID",
                        ))));
                        return;
                    };
                    let Some(stdin) = child.stdin.take() else {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let _ = startup_tx.send(Err(AppServerError::MissingPipe("stdin")));
                        return;
                    };
                    let Some(stdout) = child.stdout.take() else {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let _ = startup_tx.send(Err(AppServerError::MissingPipe("stdout")));
                        return;
                    };
                    let Some(stderr) = child.stderr.take() else {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        let _ = startup_tx.send(Err(AppServerError::MissingPipe("stderr")));
                        return;
                    };

                    emit_event(
                        &runtime_events,
                        &runtime_saturated,
                        AppServerEvent::StateChanged(ConnectionState::Starting),
                    );
                    set_state(
                        &runtime_state,
                        &runtime_events,
                        &runtime_saturated,
                        ConnectionState::Initializing,
                    );

                    let writer = tokio::spawn(run_writer(
                        stdin,
                        command_rx,
                        Arc::clone(&runtime_state),
                        runtime_events.clone(),
                        Arc::clone(&runtime_saturated),
                        runtime_process.clone(),
                    ));
                    let reader = tokio::spawn(run_stdout_reader(
                        stdout,
                        runtime_commands,
                        runtime_process,
                        Arc::clone(&runtime_state),
                        runtime_pending,
                        runtime_events.clone(),
                        Arc::clone(&runtime_saturated),
                    ));
                    let stderr_reader = tokio::spawn(run_stderr_reader(
                        stderr,
                        runtime_events.clone(),
                        Arc::clone(&runtime_saturated),
                    ));
                    if startup_tx.send(Ok(())).is_err() {
                        writer.abort();
                        reader.abort();
                        stderr_reader.abort();
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        return;
                    }
                    run_process_waiter(
                        child,
                        process_rx,
                        runtime_state,
                        runtime_events,
                        runtime_saturated,
                        process_group_id,
                    )
                    .await;
                    writer.abort();
                    reader.abort();
                    stderr_reader.abort();
                    let _ = writer.await;
                    let _ = reader.await;
                    let _ = stderr_reader.await;
                });
            })
            .map_err(AppServerError::Spawn)?;
        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = supervisor.join();
                return Err(error);
            }
            Err(error) => {
                let _ = supervisor.join();
                return Err(AppServerError::Spawn(std::io::Error::other(format!(
                    "Codex App Server runtime stopped during startup: {error}"
                ))));
            }
        }
        lock(&pending).insert(initialize.id.clone(), initialize.method.clone());
        if let Err(error) = enqueue_value(&command_tx, initialize) {
            let _ = process_tx.try_send(ProcessCommand::Terminate);
            drop(events_rx);
            let _ = supervisor.join();
            return Err(error);
        }

        Ok(Self {
            command_tx: Some(command_tx),
            process_tx: Some(process_tx),
            event_tx,
            event_queue_saturated,
            events_rx,
            state,
            pending,
            next_request_id: AtomicI64::new(INITIALIZE_REQUEST_ID + 1),
            supervisor: Some(supervisor),
            shutdown_timeout,
        })
    }

    pub fn state(&self) -> ConnectionState {
        *lock(&self.state)
    }

    pub fn events(&self) -> &Receiver<AppServerEvent> {
        &self.events_rx
    }

    pub fn try_recv(&self) -> Result<AppServerEvent, TryRecvError> {
        match self.events_rx.try_recv() {
            Err(TryRecvError::Empty) => {
                self.event_queue_saturated.store(false, Ordering::Relaxed);
                Err(TryRecvError::Empty)
            }
            result => result,
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<AppServerEvent, RecvTimeoutError> {
        match self.events_rx.recv_timeout(timeout) {
            Err(RecvTimeoutError::Timeout) => {
                self.event_queue_saturated.store(false, Ordering::Relaxed);
                Err(RecvTimeoutError::Timeout)
            }
            result => result,
        }
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

    pub fn model_list(&self, params: ModelListParams) -> Result<RequestId, AppServerError> {
        self.send_request("model/list", params)
    }

    pub fn model_provider_capabilities_read(
        &self,
        params: ModelProviderCapabilitiesReadParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("modelProvider/capabilities/read", params)
    }

    pub fn collaboration_mode_list(
        &self,
        params: CollaborationModeListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("collaborationMode/list", params)
    }

    pub fn permission_profile_list(
        &self,
        params: PermissionProfileListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("permissionProfile/list", params)
    }

    pub fn config_read(&self, params: ConfigReadParams) -> Result<RequestId, AppServerError> {
        self.send_request("config/read", params)
    }

    pub fn config_value_write(
        &self,
        params: ConfigValueWriteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("config/value/write", params)
    }

    pub fn config_batch_write(
        &self,
        params: ConfigBatchWriteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("config/batchWrite", params)
    }

    pub fn config_requirements_read(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("configRequirements/read")
    }

    pub fn get_auth_status(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("getAuthStatus", params)
    }

    pub fn account_login_start(
        &self,
        params: LoginAccountParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("account/login/start", params)
    }

    pub fn account_login_cancel(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("account/login/cancel", params)
    }

    pub fn account_logout(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("account/logout")
    }

    pub fn account_workspace_messages_read(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("account/workspaceMessages/read")
    }

    pub fn fuzzy_file_search(
        &self,
        params: FuzzyFileSearchParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fuzzyFileSearch", params)
    }

    pub fn fuzzy_file_search_session_start(
        &self,
        params: FuzzyFileSearchSessionStartParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fuzzyFileSearch/sessionStart", params)
    }

    pub fn fuzzy_file_search_session_update(
        &self,
        params: FuzzyFileSearchSessionUpdateParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fuzzyFileSearch/sessionUpdate", params)
    }

    pub fn fuzzy_file_search_session_stop(
        &self,
        params: FuzzyFileSearchSessionStopParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fuzzyFileSearch/sessionStop", params)
    }

    pub fn git_diff_to_remote(
        &self,
        params: GitDiffToRemoteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("gitDiffToRemote", params)
    }

    pub fn get_conversation_summary(
        &self,
        params: GetConversationSummaryParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("getConversationSummary", params)
    }

    pub fn command_exec(&self, params: CommandExecParams) -> Result<RequestId, AppServerError> {
        self.send_request("command/exec", params)
    }

    pub fn command_exec_write(
        &self,
        params: CommandExecWriteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("command/exec/write", params)
    }

    pub fn command_exec_terminate(
        &self,
        params: CommandExecTerminateParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("command/exec/terminate", params)
    }

    pub fn command_exec_resize(
        &self,
        params: CommandExecResizeParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("command/exec/resize", params)
    }

    pub fn process_spawn(&self, params: ProcessSpawnParams) -> Result<RequestId, AppServerError> {
        self.send_request("process/spawn", params)
    }

    pub fn process_write_stdin(
        &self,
        params: ProcessWriteStdinParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("process/writeStdin", params)
    }

    pub fn process_kill(&self, params: ProcessKillParams) -> Result<RequestId, AppServerError> {
        self.send_request("process/kill", params)
    }

    pub fn process_resize_pty(
        &self,
        params: ProcessResizePtyParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("process/resizePty", params)
    }

    pub fn fs_read_file(&self, params: FsReadFileParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/readFile", params)
    }

    pub fn fs_write_file(&self, params: FsWriteFileParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/writeFile", params)
    }

    pub fn fs_create_directory(
        &self,
        params: FsCreateDirectoryParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fs/createDirectory", params)
    }

    pub fn fs_get_metadata(
        &self,
        params: FsGetMetadataParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fs/getMetadata", params)
    }

    pub fn fs_read_directory(
        &self,
        params: FsReadDirectoryParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("fs/readDirectory", params)
    }

    pub fn fs_remove(&self, params: FsRemoveParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/remove", params)
    }

    pub fn fs_copy(&self, params: FsCopyParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/copy", params)
    }

    pub fn fs_watch(&self, params: FsWatchParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/watch", params)
    }

    pub fn fs_unwatch(&self, params: FsUnwatchParams) -> Result<RequestId, AppServerError> {
        self.send_request("fs/unwatch", params)
    }

    pub fn environment_add(
        &self,
        params: EnvironmentAddParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("environment/add", params)
    }

    pub fn environment_info(
        &self,
        params: EnvironmentInfoParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("environment/info", params)
    }

    pub fn environment_status(
        &self,
        params: EnvironmentStatusParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("environment/status", params)
    }

    pub fn account_rate_limit_reset_credit_consume(
        &self,
        params: ConsumeAccountRateLimitResetCreditParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("account/rateLimitResetCredit/consume", params)
    }

    pub fn account_send_add_credits_nudge_email(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("account/sendAddCreditsNudgeEmail", params)
    }

    pub fn external_agent_config_detect(
        &self,
        params: ExternalAgentConfigDetectParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("externalAgentConfig/detect", params)
    }

    pub fn external_agent_config_import(
        &self,
        params: ExternalAgentConfigImportParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("externalAgentConfig/import", params)
    }

    pub fn external_agent_config_import_read_histories(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("externalAgentConfig/import/readHistories")
    }

    pub fn feedback_upload(
        &self,
        params: FeedbackUploadParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("feedback/upload", params)
    }

    pub fn memory_reset(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("memory/reset")
    }

    pub fn mock_experimental_method(
        &self,
        params: MockExperimentalMethodParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("mock/experimentalMethod", params)
    }

    pub fn plugin_share_save(
        &self,
        params: PluginShareSaveParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/share/save", params)
    }

    pub fn plugin_share_update_targets(
        &self,
        params: PluginShareUpdateTargetsParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/share/updateTargets", params)
    }

    pub fn plugin_share_list(
        &self,
        params: PluginShareListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/share/list", params)
    }

    pub fn plugin_share_checkout(
        &self,
        params: PluginShareCheckoutParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/share/checkout", params)
    }

    pub fn plugin_share_delete(
        &self,
        params: PluginShareDeleteParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("plugin/share/delete", params)
    }

    pub fn remote_control_enable(
        &self,
        params: Option<RemoteControlEnableParams>,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/enable", params)
    }

    pub fn remote_control_disable(
        &self,
        params: Option<RemoteControlDisableParams>,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/disable", params)
    }

    pub fn remote_control_status_read(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("remoteControl/status/read")
    }

    pub fn remote_control_pairing_start(
        &self,
        params: RemoteControlPairingStartParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/pairing/start", params)
    }

    pub fn remote_control_pairing_status(
        &self,
        params: RemoteControlPairingStatusParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/pairing/status", params)
    }

    pub fn remote_control_clients_list(
        &self,
        params: RemoteControlClientsListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/client/list", params)
    }

    pub fn remote_control_clients_revoke(
        &self,
        params: RemoteControlClientsRevokeParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("remoteControl/client/revoke", params)
    }

    pub fn thread_fork(&self, params: ThreadForkParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/fork", params)
    }

    pub fn thread_increment_elicitation(
        &self,
        params: ThreadIncrementElicitationParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/increment_elicitation", params)
    }

    pub fn thread_decrement_elicitation(
        &self,
        params: ThreadDecrementElicitationParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/decrement_elicitation", params)
    }

    pub fn thread_approve_guardian_denied_action(
        &self,
        params: ThreadApproveGuardianDeniedActionParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/approveGuardianDeniedAction", params)
    }

    pub fn thread_search(&self, params: ThreadSearchParams) -> Result<RequestId, AppServerError> {
        self.send_request("thread/search", params)
    }

    pub fn thread_search_occurrences(
        &self,
        params: ThreadSearchOccurrencesParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/searchOccurrences", params)
    }

    pub fn thread_loaded_list(
        &self,
        params: ThreadLoadedListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/loaded/list", params)
    }

    pub fn thread_items_list(
        &self,
        params: ThreadItemsListParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/items/list", params)
    }

    pub fn thread_inject_items(
        &self,
        params: ThreadInjectItemsParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/inject_items", params)
    }

    pub fn thread_memory_mode_set(
        &self,
        params: ThreadMemoryModeSetParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/memoryMode/set", params)
    }

    pub fn thread_realtime_start(
        &self,
        params: ThreadRealtimeStartParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/start", params)
    }

    pub fn thread_realtime_append_audio(
        &self,
        params: ThreadRealtimeAppendAudioParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/appendAudio", params)
    }

    pub fn thread_realtime_append_text(
        &self,
        params: ThreadRealtimeAppendTextParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/appendText", params)
    }

    pub fn thread_realtime_append_speech(
        &self,
        params: ThreadRealtimeAppendSpeechParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/appendSpeech", params)
    }

    pub fn thread_realtime_stop(
        &self,
        params: ThreadRealtimeStopParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/stop", params)
    }

    pub fn thread_realtime_list_voices(
        &self,
        params: ThreadRealtimeListVoicesParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("thread/realtime/listVoices", params)
    }

    pub fn windows_sandbox_setup_start(
        &self,
        params: WindowsSandboxSetupStartParams,
    ) -> Result<RequestId, AppServerError> {
        self.send_request("windowsSandbox/setupStart", params)
    }

    pub fn windows_sandbox_readiness(&self) -> Result<RequestId, AppServerError> {
        self.send_request_without_params("windowsSandbox/readiness")
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
        if self.supervisor.is_none() {
            return;
        }
        self.initiate_shutdown();
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }

    fn initiate_shutdown(&mut self) {
        // Event producers may be applying bounded backpressure. Disconnect their receiver before
        // joining them so every blocked send wakes up instead of deadlocking shutdown.
        let (closed_event_tx, closed_events_rx) = mpsc::sync_channel(1);
        drop(closed_event_tx);
        drop(std::mem::replace(&mut self.events_rx, closed_events_rx));

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
            let _ = command_tx.try_send(WriterCommand::Shutdown);
        }
        if let Some(process_tx) = self.process_tx.take() {
            let _ = process_tx.try_send(ProcessCommand::GracefulShutdown(self.shutdown_timeout));
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
        if self.supervisor.is_some() {
            log::info!("Codex App Server dropped; completing bounded shutdown");
            self.shutdown();
        }
    }
}
