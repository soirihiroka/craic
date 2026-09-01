fn remove_pending_turn_media(session: &mut Session, request_id: &RequestId) {
    if let Some(uploaded) = session.pending_turn_media.remove(request_id) {
        remove_remote_media(session.remote_media.as_ref(), uploaded);
    }
}
async fn handle_server_event<F>(session: &mut Session, event: AppServerEvent, emit: &F) -> bool
where
    F: Fn(Event),
{
    let identity = session.identity.clone();
    match event {
        AppServerEvent::StateChanged(ConnectionState::Starting) => {
            emit_state(emit, &identity, SessionState::Connecting, None)
        }
        AppServerEvent::StateChanged(ConnectionState::Initializing) => emit_state(
            emit,
            &identity,
            SessionState::Initializing,
            Some("Initializing Codex…".to_owned()),
        ),
        AppServerEvent::StateChanged(ConnectionState::Stopping) => {
            emit_state(emit, &identity, SessionState::Stopping, None)
        }
        AppServerEvent::StateChanged(ConnectionState::Stopped) => {
            emit_state(emit, &identity, SessionState::Closed, None);
            return true;
        }
        AppServerEvent::StateChanged(ConnectionState::Crashed) => {
            emit_state(
                emit,
                &identity,
                SessionState::Closed,
                Some("Codex App Server crashed".to_owned()),
            );
            return true;
        }
        AppServerEvent::StateChanged(ConnectionState::Ready) => {}
        AppServerEvent::Ready(_) => {
            emit_state(
                emit,
                &identity,
                SessionState::Initializing,
                Some("Starting a new Codex chat…".to_owned()),
            );
            for result in [
                session.server.model_list(ModelListParams::default()),
                session.server.config_read(ConfigReadParams {
                    include_layers: false,
                    cwd: Some(session.workspace_root.clone()),
                }),
                session
                    .server
                    .permission_profile_list(PermissionProfileListParams {
                        cwd: Some(session.workspace_root.clone()),
                        ..Default::default()
                    }),
            ] {
                if let Err(error) = result {
                    emit_error(emit, &identity, &error.to_string());
                }
            }
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
            let mut extra = serde_json::Map::new();
            if let Some(reasoning) = session.selected_reasoning.clone() {
                extra.insert("effort".to_owned(), Value::String(reasoning));
            }
            if let Err(error) = session.server.thread_start(ThreadStartParams {
                cwd: Some(session.workspace_root.clone()),
                model: session.selected_model.clone(),
                permissions: session.selected_permissions.clone(),
                personality: session.selected_personality.clone(),
                service_tier: selected_service_tier_wire(session),
                extra,
                ..Default::default()
            }) {
                emit_error(emit, &identity, &error.to_string());
                emit_state(
                    emit,
                    &identity,
                    SessionState::Closed,
                    Some("Unable to start a Codex thread".to_owned()),
                );
                return true;
            }
        }
        AppServerEvent::Response { response, method } => {
            if let Some(pending) = session.pending_tools.remove(&response.id) {
                match pending {
                    PendingTool::Timeline(title) => {
                        session.next_local_id = session.next_local_id.wrapping_add(1);
                        let item = TranscriptItem {
                            id: format!("craic-native-tool-result-{}", session.next_local_id),
                            kind: TranscriptKind::Tool,
                            status: TranscriptStatus::Completed,
                            title: Some(title),
                            body: summarize_tool_result(&response.result),
                            detail: Some(compact_json(&response.result)),
                            image: None,
                        };
                        session.timeline.insert(item.id.clone(), item.clone());
                        emit(Event::Upsert {
                            identity: identity.clone(),
                            item,
                        });
                    }
                    PendingTool::BackgroundTerminals => emit(Event::BackgroundTerminals {
                        identity: identity.clone(),
                        terminals: parse_background_terminals(&response.result),
                    }),
                    PendingTool::Skills => emit(Event::Skills {
                        identity: identity.clone(),
                        skills: parse_skills(&response.result),
                    }),
                    PendingTool::ExperimentalFeatures => emit(Event::ExperimentalFeatures {
                        identity: identity.clone(),
                        features: parse_experimental_features(&response.result),
                    }),
                }
                return false;
            }
            match method.as_deref() {
                Some("thread/start")
                | Some("thread/resume")
                | Some("thread/fork")
                | Some("thread/rollback") => {
                    let Some(thread_id) = response
                        .result
                        .pointer("/thread/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                    else {
                        emit_error(emit, &identity, "Codex did not return a thread identifier");
                        emit_state(
                            emit,
                            &identity,
                            SessionState::Closed,
                            Some("Codex returned an invalid thread response".to_owned()),
                        );
                        return true;
                    };
                    let resumed = method.as_deref() != Some("thread/start");
                    if resumed {
                        session.resume_pending = false;
                        if let Some(previous) = session.resume_previous_thread.take()
                            && previous != thread_id
                            && let Err(error) =
                                session.server.thread_unsubscribe(ThreadUnsubscribeParams {
                                    thread_id: previous,
                                })
                        {
                            emit_error(emit, &identity, &error.to_string());
                        }
                        session.active_turn_id = None;
                        session.timeline.clear();
                        emit(Event::TranscriptCleared {
                            identity: identity.clone(),
                        });
                    }
                    emit(Event::Usage {
                        identity: identity.clone(),
                        usage: None,
                    });
                    session.thread_id = Some(thread_id.clone());
                    if resumed {
                        apply_resumed_history(session, &response.result, emit);
                    }
                    if let Some(model) = response
                        .result
                        .pointer("/thread/model")
                        .or_else(|| response.result.get("model"))
                        .and_then(Value::as_str)
                    {
                        session.selected_model = Some(model.to_owned());
                        if !session.model_options.is_empty() {
                            emit_model_options(session, emit);
                        }
                    }
                    if let Some(reasoning) = response
                        .result
                        .pointer("/thread/reasoningEffort")
                        .or_else(|| response.result.get("reasoningEffort"))
                        .and_then(Value::as_str)
                    {
                        session.selected_reasoning = Some(reasoning.to_owned());
                    }
                    update_reasoning_options(session);
                    update_service_tier_options(session);
                    update_personality_options(session);
                    if !session.model_options.is_empty() {
                        emit_reasoning_options(session, emit);
                        emit_service_tier_options(session, emit);
                        emit_personality_options(session, emit);
                    }
                    if let Some(personality) = response
                        .result
                        .pointer("/thread/personality")
                        .or_else(|| response.result.get("personality"))
                        .and_then(Value::as_str)
                    {
                        session.selected_personality = Some(personality.to_owned());
                        emit_personality_options(session, emit);
                    }
                    if let Some(service_tier) = response
                        .result
                        .pointer("/thread/serviceTier")
                        .or_else(|| response.result.get("serviceTier"))
                        .and_then(Value::as_str)
                    {
                        session.selected_service_tier = Some(service_tier.to_owned());
                        emit_service_tier_options(session, emit);
                    } else if response
                        .result
                        .pointer("/thread/serviceTier")
                        .or_else(|| response.result.get("serviceTier"))
                        .is_some_and(Value::is_null)
                    {
                        session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
                        emit_service_tier_options(session, emit);
                    }
                    if let Some(permissions) = response
                        .result
                        .pointer("/thread/activePermissionProfile/id")
                        .or_else(|| response.result.pointer("/activePermissionProfile/id"))
                        .and_then(Value::as_str)
                    {
                        session.selected_permissions = Some(permissions.to_owned());
                        if !session.permission_options.is_empty() {
                            emit_permission_options(session, emit);
                        }
                    }
                    let title = response
                        .result
                        .pointer("/thread/name")
                        .or_else(|| response.result.pointer("/thread/preview"))
                        .and_then(Value::as_str)
                        .filter(|title| !title.trim().is_empty())
                        .map(str::to_owned);
                    session.thread_title = title.clone();
                    persist_thread_overlay(
                        session.workspace_key.clone(),
                        session.thread_id.clone(),
                        title.clone(),
                    )
                    .await;
                    emit(Event::ThreadReady {
                        identity: identity.clone(),
                        thread_id,
                        title,
                    });
                    let ready_state = if session.active_turn_id.is_some() {
                        SessionState::Running
                    } else {
                        SessionState::Ready
                    };
                    emit_state(
                        emit,
                        &identity,
                        ready_state,
                        Some(if ready_state == SessionState::Running {
                            "Codex is working…".to_owned()
                        } else {
                            "Ready".to_owned()
                        }),
                    );
                    if let Err(error) = request_thread_list(session) {
                        emit_error(emit, &identity, &error);
                    }
                }
                Some("turn/start") => {
                    remove_pending_turn_media(session, &response.id);
                    if let Some(turn_id) =
                        response.result.pointer("/turn/id").and_then(Value::as_str)
                    {
                        session.active_turn_id = Some(turn_id.to_owned());
                    }
                    emit_state(
                        emit,
                        &identity,
                        SessionState::Running,
                        Some("Codex is working…".to_owned()),
                    );
                }
                Some("review/start") => {
                    let detached = session
                        .pending_reviews
                        .remove(&response.id)
                        .unwrap_or(false);
                    if detached
                        && let Some(review_thread_id) = response
                            .result
                            .get("reviewThreadId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    {
                        let previous_thread = session.thread_id.clone();
                        match session
                            .server
                            .thread_resume(thread_resume_params(session, review_thread_id))
                        {
                            Ok(_) => {
                                session.resume_pending = true;
                                session.resume_previous_thread = previous_thread;
                                emit_state(
                                    emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Opening review thread…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(emit, &identity, &error.to_string()),
                        }
                    } else {
                        if let Some(turn_id) =
                            response.result.pointer("/turn/id").and_then(Value::as_str)
                        {
                            session.active_turn_id = Some(turn_id.to_owned());
                        }
                        emit_state(
                            emit,
                            &identity,
                            SessionState::Running,
                            Some("Codex is reviewing…".to_owned()),
                        );
                    }
                }
                Some("turn/interrupt") => {}
                Some("model/list") => apply_model_catalog(session, &response.result, emit),
                Some("config/read") => apply_config_defaults(session, &response.result, emit),
                Some("permissionProfile/list") => {
                    apply_permission_profiles(session, &response.result, emit)
                }
                Some("thread/list") => {
                    apply_thread_list(session, &response.id, &response.result, emit).await
                }
                Some("thread/name/set")
                | Some("thread/archive")
                | Some("thread/unarchive")
                | Some("thread/delete") => {
                    apply_thread_operation_response(session, &response.id, emit).await;
                }
                Some("thread/settings/update") => {
                    if let Some(pending) = session.pending_settings.remove(&response.id) {
                        emit(Event::SettingApplied {
                            identity: identity.clone(),
                            setting: match pending {
                                PendingSetting::Model { .. } => SettingKind::Model,
                                PendingSetting::Reasoning(_) => SettingKind::Reasoning,
                                PendingSetting::Personality(_) => SettingKind::Personality,
                                PendingSetting::ServiceTier(_) => SettingKind::ServiceTier,
                                PendingSetting::Permissions(_) => SettingKind::Permissions,
                            },
                        });
                    }
                }
                _ => {}
            }
        }
        AppServerEvent::ErrorResponse { response, method } => {
            let tool_title = session.pending_tools.remove(&response.id);
            if method.as_deref() == Some("turn/start") {
                remove_pending_turn_media(session, &response.id);
            }
            if method.as_deref() == Some("review/start") {
                session.pending_reviews.remove(&response.id);
            }
            if method.as_deref() == Some("thread/list")
                && session.thread_list_request.as_ref() == Some(&response.id)
            {
                session.thread_list_request = None;
            }
            if matches!(
                method.as_deref(),
                Some("thread/name/set" | "thread/archive" | "thread/unarchive" | "thread/delete")
            ) {
                session.pending_thread_operations.remove(&response.id);
            }
            if method.as_deref() == Some("thread/settings/update")
                && let Some(pending) = session.pending_settings.remove(&response.id)
            {
                match pending {
                    PendingSetting::Model {
                        model,
                        reasoning,
                        service_tier,
                    } => {
                        session.selected_model = model;
                        session.selected_reasoning = reasoning;
                        session.selected_service_tier = service_tier;
                        emit_model_options(session, emit);
                        emit_reasoning_options(session, emit);
                        emit_service_tier_options(session, emit);
                    }
                    PendingSetting::Reasoning(previous) => {
                        session.selected_reasoning = previous;
                        emit_reasoning_options(session, emit);
                    }
                    PendingSetting::Personality(previous) => {
                        session.selected_personality = previous;
                        emit_personality_options(session, emit);
                    }
                    PendingSetting::ServiceTier(previous) => {
                        session.selected_service_tier = previous;
                        emit_service_tier_options(session, emit);
                    }
                    PendingSetting::Permissions(previous) => {
                        session.selected_permissions = previous;
                        emit_permission_options(session, emit);
                    }
                }
            }
            emit_error(
                emit,
                &identity,
                &format!(
                    "{} failed: {}",
                    tool_title
                        .as_ref()
                        .map(PendingTool::title)
                        .or(method.as_deref())
                        .unwrap_or("Codex request"),
                    response.error.message
                ),
            );
            if method.as_deref() == Some("thread/start") {
                emit_state(
                    emit,
                    &identity,
                    SessionState::Closed,
                    Some(response.error.message),
                );
                return true;
            }
            if matches!(
                method.as_deref(),
                Some("thread/resume" | "thread/fork" | "thread/rollback")
            ) {
                session.resume_pending = false;
                session.resume_previous_thread = None;
                emit_state(
                    emit,
                    &identity,
                    SessionState::Ready,
                    Some("Unable to open Codex chat".to_owned()),
                );
            }
            if matches!(
                method.as_deref(),
                Some("turn/start") | Some("turn/interrupt") | Some("review/start")
            ) {
                session.active_turn_id = None;
                emit_state(
                    emit,
                    &identity,
                    SessionState::Ready,
                    Some("Ready".to_owned()),
                );
            }
        }
        AppServerEvent::ServerRequest(request) => {
            if request.method == "currentTime/read" {
                let current_time_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                let _ = session.server.respond(
                    request.id,
                    serde_json::json!({ "currentTimeAt": current_time_at }),
                );
                return false;
            }
            let params = request.params.unwrap_or(Value::Null);
            if params
                .get("threadId")
                .and_then(Value::as_str)
                .is_some_and(|thread_id| session.thread_id.as_deref() != Some(thread_id))
            {
                let _ = session.server.respond_error(
                    request.id,
                    RpcError {
                        code: -32600,
                        message: "request targets an inactive thread".to_owned(),
                        data: None,
                    },
                );
                return false;
            }
            let request_key = request_id_key(&request.id);
            let Some(presentation) =
                pending_request_from_server(&request_key, &request.method, &params)
            else {
                let method = request.method;
                let _ = session.server.respond_error(
                    request.id,
                    RpcError {
                        code: -32601,
                        message: format!("Craic for macOS does not yet handle {method}"),
                        data: None,
                    },
                );
                emit_error(
                    emit,
                    &identity,
                    &format!("Codex requested unsupported operation {method}"),
                );
                return false;
            };
            session.pending_requests.insert(
                request_key,
                ServerRequest {
                    id: request.id,
                    method: request.method,
                    params,
                },
            );
            emit(Event::Request {
                identity,
                request: presentation,
            });
        }
        AppServerEvent::Notification(notification) => {
            handle_notification(session, &notification.method, notification.params, emit).await
        }
        AppServerEvent::Diagnostic(message) => {
            log::debug!("native Codex diagnostic bytes={}", message.len())
        }
        AppServerEvent::ProtocolError(message) => emit_error(emit, &identity, &message),
        AppServerEvent::ProcessExited(status) => {
            emit_state(
                emit,
                &identity,
                SessionState::Closed,
                Some(format!(
                    "Codex App Server exited with status {:?}",
                    status.code
                )),
            );
            return true;
        }
    }
    false
}

fn apply_model_catalog<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let Some(models) = result.get("data").and_then(Value::as_array) else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned an invalid model catalog",
        );
        return;
    };
    if models.is_empty() {
        log::warn!("native Codex model catalog was empty response={result}");
    }
    let mut default = None;
    let mut options = Vec::new();
    let mut reasoning_by_model = HashMap::new();
    let mut service_tiers_by_model = HashMap::new();
    for model in models {
        let Some(id) = model
            .get("model")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if model.get("isDefault").and_then(Value::as_bool) == Some(true) {
            default = Some(id.to_owned());
        }
        options.push(SelectorOption {
            id: id.to_owned(),
            label: model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned(),
        });
        reasoning_by_model.insert(
            id.to_owned(),
            model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| {
                    let id = effort.get("reasoningEffort")?.as_str()?;
                    Some(SelectorOption {
                        id: id.to_owned(),
                        label: craic_agent::display::title_case(id),
                    })
                })
                .collect(),
        );
        let mut service_tiers = vec![SelectorOption {
            id: DEFAULT_SERVICE_TIER_ID.to_owned(),
            label: "Standard".to_owned(),
        }];
        service_tiers.extend(
            model
                .get("serviceTiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tier| {
                    let id = tier.get("id").and_then(Value::as_str)?;
                    Some(SelectorOption {
                        id: id.to_owned(),
                        label: tier
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_owned(),
                    })
                }),
        );
        service_tiers_by_model.insert(
            id.to_owned(),
            ModelServiceTiers {
                options: service_tiers,
                default: model
                    .get("defaultServiceTier")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
        );
    }
    session.model_options = options;
    session.model_reasoning = reasoning_by_model;
    session.model_service_tiers = service_tiers_by_model;
    if !session.selected_model.as_ref().is_some_and(|selected| {
        session
            .model_options
            .iter()
            .any(|option| option.id == *selected)
    }) {
        session.selected_model = default.or_else(|| {
            session
                .model_options
                .first()
                .map(|option| option.id.clone())
        });
        session.model_overridden = false;
    }
    update_reasoning_options(session);
    update_service_tier_options(session);
    update_personality_options(session);
    emit_model_options(session, emit);
    emit_reasoning_options(session, emit);
    emit_service_tier_options(session, emit);
    emit_personality_options(session, emit);
}

fn request_thread_list(session: &mut Session) -> Result<(), String> {
    let mut extra = serde_json::Map::new();
    extra.insert("sortKey".to_owned(), Value::String("updated_at".to_owned()));
    extra.insert("sortDirection".to_owned(), Value::String("desc".to_owned()));
    let request_id = session
        .server
        .thread_list(ThreadListParams {
            limit: Some(100),
            archived: Some(session.thread_list_archived),
            cwd: Some(ThreadListCwdFilter::One(session.workspace_root.clone())),
            search_term: (!session.thread_list_query.is_empty())
                .then(|| session.thread_list_query.clone()),
            extra,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
    session.thread_list_request = Some(request_id);
    Ok(())
}

fn start_tool_requests(
    session: &Session,
    action: ToolAction,
) -> Vec<(PendingTool, Result<RequestId, AppServerError>)> {
    let Some(thread_id) = session.thread_id.clone() else {
        return Vec::new();
    };
    match action {
        ToolAction::ViewThreadGoal => vec![(
            PendingTool::Timeline("Thread goal".to_owned()),
            session
                .server
                .thread_goal_get(ThreadGoalGetParams { thread_id }),
        )],
        ToolAction::SetThreadGoal(objective) => vec![(
            PendingTool::Timeline("Set thread goal".to_owned()),
            session.server.thread_goal_set(ThreadGoalSetParams {
                thread_id,
                objective: Some(objective),
                status: None,
                token_budget: None,
            }),
        )],
        ToolAction::ClearThreadGoal => vec![(
            PendingTool::Timeline("Clear thread goal".to_owned()),
            session
                .server
                .thread_goal_clear(ThreadGoalClearParams { thread_id }),
        )],
        ToolAction::RunShellCommand(command) => vec![(
            PendingTool::Timeline("Shell command".to_owned()),
            session
                .server
                .thread_shell_command(ThreadShellCommandParams { thread_id, command }),
        )],
        ToolAction::BackgroundTerminals => vec![(
            PendingTool::BackgroundTerminals,
            session
                .server
                .thread_background_terminals_list(ThreadBackgroundTerminalsListParams {
                    thread_id,
                    cursor: None,
                    limit: Some(100),
                }),
        )],
        ToolAction::Skills => vec![(
            PendingTool::Skills,
            session.server.skills_list(SkillsListParams {
                cwds: vec![PathBuf::from(&session.workspace_root)],
                force_reload: false,
            }),
        )],
        ToolAction::McpServers => vec![(
            PendingTool::Timeline("MCP servers".to_owned()),
            session
                .server
                .mcp_server_status_list(ListMcpServerStatusParams {
                    cursor: None,
                    limit: Some(100),
                    detail: Some(McpServerStatusDetail::Full),
                    thread_id: Some(thread_id),
                }),
        )],
        ToolAction::Apps => vec![
            (
                PendingTool::Timeline("Available apps & connectors".to_owned()),
                session.server.apps_list(AppsListParams {
                    cursor: None,
                    limit: Some(100),
                    thread_id: Some(thread_id.clone()),
                    force_refetch: false,
                }),
            ),
            (
                PendingTool::Timeline("Installed apps & connectors".to_owned()),
                session.server.apps_installed(AppsInstalledParams {
                    thread_id: Some(thread_id),
                    force_refresh: false,
                }),
            ),
        ],
        ToolAction::Plugins => vec![
            (
                PendingTool::Timeline("Available plugins".to_owned()),
                session.server.plugin_list(PluginListParams {
                    cwds: Some(vec![PathBuf::from(&session.workspace_root)]),
                    marketplace_kinds: None,
                    force_refetch: false,
                }),
            ),
            (
                PendingTool::Timeline("Installed plugins".to_owned()),
                session.server.plugin_installed(PluginInstalledParams {
                    cwds: Some(vec![PathBuf::from(&session.workspace_root)]),
                    install_suggestion_plugin_names: None,
                }),
            ),
        ],
        ToolAction::ExperimentalFeatures => vec![(
            PendingTool::ExperimentalFeatures,
            session
                .server
                .experimental_feature_list(ExperimentalFeatureListParams {
                    cursor: None,
                    limit: Some(100),
                    thread_id: Some(thread_id),
                }),
        )],
        ToolAction::StopBackgroundTerminal(process_id) => vec![(
            PendingTool::Timeline("Stop background terminal".to_owned()),
            session.server.thread_background_terminals_terminate(
                ThreadBackgroundTerminalsTerminateParams {
                    thread_id,
                    process_id,
                },
            ),
        )],
        ToolAction::StopAllBackgroundTerminals => vec![(
            PendingTool::Timeline("Stop all background terminals".to_owned()),
            session.server.thread_background_terminals_clean(
                ThreadBackgroundTerminalsCleanParams { thread_id },
            ),
        )],
        ToolAction::SetExperimentalFeatures(enablement) => vec![(
            PendingTool::Timeline("Update experimental features".to_owned()),
            session.server.experimental_feature_enablement_set(
                ExperimentalFeatureEnablementSetParams { enablement },
            ),
        )],
        ToolAction::AccountUsage => vec![
            (
                PendingTool::Timeline("Account".to_owned()),
                session.server.account_read(GetAccountParams {
                    refresh_token: false,
                }),
            ),
            (
                PendingTool::Timeline("Account rate limits".to_owned()),
                session.server.account_rate_limits_read(),
            ),
            (
                PendingTool::Timeline("Account usage".to_owned()),
                session.server.account_usage_read(),
            ),
        ],
    }
}

async fn persist_thread_overlay(
    workspace_key: String,
    thread_id: Option<String>,
    task_description: Option<String>,
) {
    let Some(thread_id) = thread_id else {
        return;
    };
    let operation_workspace = workspace_key.clone();
    let operation_thread = thread_id.clone();
    let operation = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let existing =
            agent_history::lookup_codex_thread_overlay(&operation_workspace, &operation_thread)
                .map_err(|error| error.to_string())?;
        let task_description = task_description
            .filter(|description| !description.trim().is_empty())
            .or_else(|| {
                existing
                    .as_ref()
                    .and_then(|overlay| overlay.task_description.clone())
            });
        let tags = existing.map(|overlay| overlay.tags).unwrap_or_default();
        agent_history::upsert_codex_thread_overlay(CodexThreadOverlayUpsert {
            thread_id: operation_thread,
            workspace_key: operation_workspace,
            task_description,
            tags,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    })
    .await;
    match operation {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            log::warn!(
                "failed persisting native Codex thread overlay workspace={workspace_key} thread_id={thread_id}: {error}"
            );
        }
        Err(error) => log::warn!(
            "native Codex thread overlay task failed workspace={workspace_key} thread_id={thread_id}: {error}"
        ),
    }
}

async fn apply_thread_operation_response<F>(session: &mut Session, request_id: &RequestId, emit: &F)
where
    F: Fn(Event),
{
    let Some(operation) = session.pending_thread_operations.remove(request_id) else {
        return;
    };
    match operation {
        PendingThreadOperation::Rename { thread_id, name } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Rename,
            });
            if session.thread_id.as_deref() == Some(thread_id.as_str()) {
                session.thread_title = Some(name.clone());
                persist_thread_overlay(
                    session.workspace_key.clone(),
                    session.thread_id.clone(),
                    Some(name.clone()),
                )
                .await;
                emit(Event::ThreadReady {
                    identity: session.identity.clone(),
                    thread_id,
                    title: Some(name),
                });
            }
        }
        PendingThreadOperation::Archive { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Archive,
            });
            close_active_thread(session, &thread_id, "Codex thread archived", emit)
        }
        PendingThreadOperation::Unarchive { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id,
                operation: ThreadOperationKind::Unarchive,
            });
        }
        PendingThreadOperation::Delete { thread_id } => {
            emit(Event::ThreadOperationApplied {
                identity: session.identity.clone(),
                thread_id: thread_id.clone(),
                operation: ThreadOperationKind::Delete,
            });
            let workspace_key = session.workspace_key.clone();
            let operation_workspace = workspace_key.clone();
            let operation_thread = thread_id.clone();
            match tokio::task::spawn_blocking(move || {
                agent_history::delete_codex_thread_overlay(&operation_workspace, &operation_thread)
                    .map_err(|error| error.to_string())
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::warn!(
                    "failed deleting native Codex thread overlay workspace={workspace_key} thread_id={thread_id}: {error}"
                ),
                Err(error) => log::warn!(
                    "native Codex thread overlay deletion task failed workspace={workspace_key} thread_id={thread_id}: {error}"
                ),
            }
            close_active_thread(session, &thread_id, "Codex thread deleted", emit)
        }
    }
    if let Err(error) = request_thread_list(session) {
        emit_error(emit, &session.identity, &error);
    }
}

fn close_active_thread<F>(session: &mut Session, thread_id: &str, message: &str, emit: &F)
where
    F: Fn(Event),
{
    if session.thread_id.as_deref() != Some(thread_id) {
        return;
    }
    session.thread_id = None;
    session.thread_title = None;
    session.active_turn_id = None;
    session.resume_pending = false;
    session.resume_previous_thread = None;
    session.timeline.clear();
    for request_key in session.pending_requests.drain().map(|(key, _)| key) {
        emit(Event::RequestResolved {
            identity: session.identity.clone(),
            request_key,
        });
    }
    emit(Event::TranscriptCleared {
        identity: session.identity.clone(),
    });
    emit(Event::Usage {
        identity: session.identity.clone(),
        usage: None,
    });
    emit(Event::ThreadClosed {
        identity: session.identity.clone(),
        message: message.to_owned(),
    });
    emit_state(
        emit,
        &session.identity,
        SessionState::Closed,
        Some(message.to_owned()),
    );
}

fn thread_resume_params(session: &Session, thread_id: String) -> ThreadResumeParams {
    let mut extra = serde_json::Map::new();
    extra.insert("excludeTurns".to_owned(), Value::Bool(true));
    if let Some(reasoning) = session.selected_reasoning.clone() {
        extra.insert("effort".to_owned(), Value::String(reasoning));
    }
    extra.insert(
        "initialTurnsPage".to_owned(),
        serde_json::json!({
            "limit": 100,
            "sortDirection": "desc",
            "itemsView": "full"
        }),
    );
    ThreadResumeParams {
        thread_id,
        model: session.selected_model.clone(),
        model_provider: None,
        cwd: Some(session.workspace_root.clone()),
        permissions: session.selected_permissions.clone(),
        personality: session.selected_personality.clone(),
        service_tier: selected_service_tier_wire(session),
        extra,
    }
}

async fn apply_thread_list<F>(
    session: &mut Session,
    request_id: &RequestId,
    result: &Value,
    emit: &F,
) where
    F: Fn(Event),
{
    if session.thread_list_request.as_ref() != Some(request_id) {
        return;
    }
    session.thread_list_request = None;
    let Some(data) = result.get("data").and_then(Value::as_array).cloned() else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned an invalid thread history response",
        );
        return;
    };
    let workspace_key = session.workspace_key.clone();
    let thread_list_archived = session.thread_list_archived;
    let identity = session.identity.clone();
    let operation_workspace = workspace_key.clone();
    let overlays = match tokio::task::spawn_blocking(move || {
        agent_history::list_codex_thread_overlays(&operation_workspace, 10_000, 0)
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(overlays)) => overlays
            .into_iter()
            .map(|overlay| (overlay.thread_id.clone(), overlay))
            .collect::<HashMap<_, _>>(),
        Ok(Err(error)) => {
            log::warn!(
                "failed loading native Codex thread overlays workspace={workspace_key}: {error}"
            );
            HashMap::new()
        }
        Err(error) => {
            log::warn!(
                "native Codex thread overlay load task failed workspace={workspace_key}: {error}"
            );
            HashMap::new()
        }
    };
    let mut threads = data
        .iter()
        .filter_map(|thread| {
            let id = thread.get("id").and_then(Value::as_str)?.to_owned();
            let smart_summary = overlays
                .get(&id)
                .and_then(|overlay| overlay.task_description.as_deref())
                .map(str::trim)
                .filter(|summary| !summary.is_empty())
                .map(str::to_owned);
            let preview = thread
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let title = thread
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    if let Some(summary) = smart_summary.as_deref() {
                        summary
                    } else if preview.trim().is_empty() {
                        "Untitled Codex chat"
                    } else {
                        preview.as_str()
                    }
                })
                .to_owned();
            Some(ThreadSummary {
                id,
                title,
                preview,
                smart_summary,
                model: thread
                    .get("model")
                    .or_else(|| thread.get("modelProvider"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                status: thread
                    .pointer("/status/type")
                    .and_then(Value::as_str)
                    .map(craic_agent::display::title_case),
                updated_at: thread
                    .get("updatedAt")
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
                    .saturating_mul(1_000),
                pinned: thread
                    .get("isPinned")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                archived: thread
                    .get("archived")
                    .or_else(|| thread.get("isArchived"))
                    .and_then(Value::as_bool)
                    .unwrap_or(thread_list_archived),
            })
        })
        .collect::<Vec<_>>();
    threads.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    emit(Event::Threads { identity, threads });
}

fn apply_resumed_history<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let initial_page = result.get("initialTurnsPage");
    let turns = initial_page
        .and_then(|page| page.get("data"))
        .and_then(Value::as_array)
        .or_else(|| result.pointer("/thread/turns").and_then(Value::as_array));
    let mut turns = turns.cloned().unwrap_or_default();
    if initial_page.is_some() {
        turns.reverse();
    }
    for turn in &turns {
        for item in turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(item) = transcript_from_history_item(item) {
                session.timeline.insert(item.id.clone(), item.clone());
                emit(Event::Upsert {
                    identity: session.identity.clone(),
                    item,
                });
            }
        }
        if turn.get("status").and_then(Value::as_str) == Some("inProgress") {
            session.active_turn_id = turn.get("id").and_then(Value::as_str).map(str::to_owned);
        }
    }
}

fn transcript_from_history_item(item: &Value) -> Option<TranscriptItem> {
    if item.get("type").and_then(Value::as_str) == Some("userMessage") {
        return Some(TranscriptItem {
            id: item.get("id")?.as_str()?.to_owned(),
            kind: TranscriptKind::User,
            status: TranscriptStatus::Completed,
            title: Some("You".to_owned()),
            body: item
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| flatten_text(item.get("content"))),
            detail: None,
            image: None,
        });
    }
    transcript_from_item(item, true)
}

fn apply_config_defaults<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let config = result.get("config").unwrap_or(result);
    session.context_window_fallback = config
        .get("model_context_window")
        .or_else(|| config.get("modelContextWindow"))
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0);
    if !session.model_overridden
        && let Some(model) = config.get("model").and_then(Value::as_str)
    {
        session.selected_model = Some(model.to_owned());
        if !session.model_options.is_empty() {
            emit_model_options(session, emit);
        }
    }
    if !session.reasoning_overridden
        && let Some(reasoning) = config
            .get("model_reasoning_effort")
            .or_else(|| config.get("modelReasoningEffort"))
            .and_then(Value::as_str)
    {
        session.selected_reasoning = Some(reasoning.to_owned());
        update_reasoning_options(session);
        if !session.model_options.is_empty() {
            emit_reasoning_options(session, emit);
        }
    }
    if !session.personality_overridden
        && let Some(personality) = config.get("personality").and_then(Value::as_str)
    {
        session.selected_personality = Some(personality.to_owned());
        update_personality_options(session);
        emit_personality_options(session, emit);
    }
    if !session.service_tier_overridden {
        if let Some(service_tier) = config
            .get("service_tier")
            .or_else(|| config.get("serviceTier"))
            .and_then(Value::as_str)
        {
            session.selected_service_tier = Some(service_tier.to_owned());
        } else if config
            .get("service_tier")
            .or_else(|| config.get("serviceTier"))
            .is_some_and(Value::is_null)
        {
            session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
        }
        update_service_tier_options(session);
        emit_service_tier_options(session, emit);
    }
    if !session.permissions_overridden
        && let Some(permissions) = ["permissions", "default_permissions", "defaultPermissions"]
            .into_iter()
            .find_map(|key| config.get(key).and_then(Value::as_str))
    {
        session.selected_permissions = Some(permissions.to_owned());
        if !session.permission_options.is_empty() {
            emit_permission_options(session, emit);
        }
    }
}

fn apply_permission_profiles<F>(session: &mut Session, result: &Value, emit: &F)
where
    F: Fn(Event),
{
    let Some(profiles) = result.get("data").and_then(Value::as_array) else {
        emit_error(
            emit,
            &session.identity,
            "Codex returned invalid permission profiles",
        );
        return;
    };
    if profiles.is_empty() {
        log::warn!("native Codex permission profile catalog was empty response={result}");
    }
    session.permission_options = profiles
        .iter()
        .filter(|profile| profile.get("allowed").and_then(Value::as_bool) != Some(false))
        .filter_map(|profile| {
            let id = profile.get("id").and_then(Value::as_str)?;
            Some(SelectorOption {
                id: id.to_owned(),
                label: permission_label(id),
            })
        })
        .collect();
    if !session
        .selected_permissions
        .as_ref()
        .is_some_and(|selected| {
            session
                .permission_options
                .iter()
                .any(|option| option.id == *selected)
        })
    {
        session.selected_permissions = session
            .permission_options
            .first()
            .map(|option| option.id.clone());
        session.permissions_overridden = false;
    }
    emit_permission_options(session, emit);
}
