impl Session {
    fn matches(&self, identity: &SessionIdentity) -> bool {
        self.identity == *identity && !self.cancellation.is_cancelled()
    }
}
pub async fn run<F>(mut commands: mpsc::Receiver<Command>, emit: F)
where
    F: Fn(Event) + Send + Sync + 'static,
{
    let mut session = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(16));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    shutdown_session(session.take(), &emit).await;
                    return;
                };
                match command {
                    Command::Start {
                        identity,
                        workspace,
                        cancellation,
                        model,
                        reasoning,
                        personality,
                        service_tier,
                        permissions,
                    } => {
                        shutdown_session(session.take(), &emit).await;
                        emit(Event::Cleared);
                        emit_state(&emit, &identity, SessionState::Connecting, None);
                        let startup = tokio::task::spawn_blocking(move || {
                            let (config, workspace_root, remote_media) = prepare(&workspace)?;
                            AppServer::spawn(config)
                                .map(|server| (server, workspace_root, remote_media))
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| format!("Codex startup task failed: {error}"));
                        match startup.and_then(|result| result) {
                            Ok((server, workspace_root, remote_media)) if !cancellation.is_cancelled() => {
                                log::info!(
                                    "native Codex session process started workspace={} generation={}",
                                    identity.workspace_id,
                                    identity.generation
                                );
                                let model_overridden = model.is_some();
                                let reasoning_overridden = reasoning.is_some();
                                let personality_overridden = personality.is_some();
                                let service_tier_overridden = service_tier.is_some();
                                let permissions_overridden = permissions.is_some();
                                let workspace_key = identity.workspace_id.clone();
                                session = Some(Session {
                                    identity,
                                    workspace_key,
                                    workspace_root,
                                    remote_media,
                                    cancellation,
                                    server,
                                    thread_id: None,
                                    thread_title: None,
                                    active_turn_id: None,
                                    timeline: HashMap::new(),
                                    next_local_id: 0,
                                    model_options: Vec::new(),
                                    model_reasoning: HashMap::new(),
                                    model_service_tiers: HashMap::new(),
                                    permission_options: Vec::new(),
                                    selected_model: model,
                                    selected_reasoning: reasoning,
                                    selected_personality: personality,
                                    selected_service_tier: service_tier,
                                    selected_permissions: permissions,
                                    model_overridden,
                                    reasoning_overridden,
                                    personality_overridden,
                                    service_tier_overridden,
                                    permissions_overridden,
                                    context_window_fallback: None,
                                    pending_settings: HashMap::new(),
                                    resume_pending: false,
                                    resume_previous_thread: None,
                                    thread_list_request: None,
                                    thread_list_query: String::new(),
                                    thread_list_archived: false,
                                    pending_thread_operations: HashMap::new(),
                                    pending_reviews: HashMap::new(),
                                    pending_tools: HashMap::new(),
                                    pending_requests: HashMap::new(),
                                    pending_turn_media: HashMap::new(),
                                });
                            }
                            Ok((mut server, _, _)) => {
                                let _ = tokio::task::spawn_blocking(move || server.shutdown()).await;
                            }
                            Err(error) => {
                                emit_state(&emit, &identity, SessionState::Closed, Some(error));
                            }
                        }
                    }
                    Command::Send { identity, text, attachments } => {
                        let text = text.trim().to_owned();
                        if text.is_empty() && attachments.is_empty() {
                            continue;
                        }
                        let remote_context = {
                            let Some(active) = session.as_ref().filter(|active| active.matches(&identity)) else {
                                continue;
                            };
                            if active.thread_id.is_none() {
                                emit_error(&emit, &identity, "The Codex thread is not ready yet");
                                continue;
                            }
                            if active.active_turn_id.is_some() {
                                emit_error(&emit, &identity, "Wait for the current Codex turn to finish or stop it first");
                                continue;
                            }
                            if active.resume_pending {
                                emit_error(&emit, &identity, "A Codex chat is already opening");
                                continue;
                            }
                            active.remote_media.clone()
                        };
                        let (attachments, uploaded) = if let Some(context) = remote_context {
                            let materialization = tokio::task::spawn_blocking(move || {
                                materialize_attachments(context, attachments)
                            })
                            .await
                            .map_err(|error| format!("Remote attachment task failed: {error}"));
                            match materialization.and_then(|result| result) {
                                Ok(result) => result,
                                Err(error) => {
                                    emit_error(&emit, &identity, &error);
                                    continue;
                                }
                            }
                        } else {
                            (attachments, Vec::new())
                        };
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.cancellation.is_cancelled() {
                            remove_remote_media(active.remote_media.as_ref(), uploaded);
                            continue;
                        }
                        let Some(thread_id) = active.thread_id.clone() else {
                            remove_remote_media(active.remote_media.as_ref(), uploaded);
                            continue;
                        };
                        active.next_local_id = active.next_local_id.wrapping_add(1);
                        let client_id = format!("craic-native-user-{}", active.next_local_id);
                        let mut extra = serde_json::Map::new();
                        if let Some(reasoning) = active.selected_reasoning.clone() {
                            extra.insert("effort".to_owned(), Value::String(reasoning));
                        }
                        let mut input = Vec::with_capacity(attachments.len() + usize::from(!text.is_empty()));
                        if !text.is_empty() {
                            input.push(UserInput::text(&text));
                        }
                        input.extend(attachments.iter().map(|attachment| match attachment.kind {
                            AttachmentKind::Image => UserInput::LocalImage {
                                path: attachment.path.clone(),
                                detail: None,
                            },
                            AttachmentKind::Audio => UserInput::LocalAudio {
                                path: attachment.path.clone(),
                            },
                            AttachmentKind::Mention => UserInput::Mention {
                                name: attachment.label.clone(),
                                path: attachment.path.to_string_lossy().into_owned(),
                            },
                            AttachmentKind::Skill => UserInput::Skill {
                                name: attachment.label.clone(),
                                path: attachment.path.clone(),
                            },
                        }));
                        let mut display = (!text.is_empty()).then(|| text.clone()).into_iter().collect::<Vec<_>>();
                        display.extend(attachments.iter().map(|attachment| format!("[{}]", attachment.label)));
                        match active.server.turn_start(TurnStartParams {
                            thread_id,
                            client_user_message_id: Some(client_id.clone()),
                            input,
                            cwd: None,
                            permissions: active.selected_permissions.clone(),
                            model: active.selected_model.clone(),
                            personality: active.selected_personality.clone(),
                            service_tier: selected_service_tier_wire(active),
                            extra,
                        }) {
                            Ok(request_id) => {
                                if !uploaded.is_empty() {
                                    active.pending_turn_media.insert(request_id, uploaded);
                                }
                                if active.thread_title.is_none()
                                    && let Some(title) = concise_title(&text)
                                {
                                    active.thread_title = Some(title.clone());
                                    persist_thread_overlay(
                                        active.workspace_key.clone(),
                                        active.thread_id.clone(),
                                        Some(title),
                                    )
                                    .await;
                                }
                                let item = TranscriptItem {
                                    id: client_id,
                                    kind: TranscriptKind::User,
                                    status: TranscriptStatus::Completed,
                                    title: Some("You".to_owned()),
                                    body: display.join("\n"),
                                    detail: None,
                                    image: None,
                                };
                                active.timeline.insert(item.id.clone(), item.clone());
                                emit(Event::Upsert { identity: identity.clone(), item });
                                emit_state(&emit, &identity, SessionState::Running, Some("Codex is working…".to_owned()));
                            }
                            Err(error) => {
                                remove_remote_media(active.remote_media.as_ref(), uploaded);
                                emit_error(&emit, &identity, &error.to_string());
                            }
                        }
                    }
                    Command::Interrupt { identity } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let (Some(thread_id), Some(turn_id)) =
                            (active.thread_id.clone(), active.active_turn_id.clone())
                        else {
                            continue;
                        };
                        if let Err(error) = active.server.turn_interrupt(TurnInterruptParams { thread_id, turn_id }) {
                            emit_error(&emit, &identity, &error.to_string());
                        } else {
                            emit_state(&emit, &identity, SessionState::Stopping, Some("Stopping turn…".to_owned()));
                        }
                    }
                    Command::SetModel { identity, model } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !active.model_options.iter().any(|option| option.id == model) {
                            emit_error(&emit, &identity, "The selected Codex model is no longer available");
                            continue;
                        }
                        let previous_model = active.selected_model.clone();
                        let previous_reasoning = active.selected_reasoning.clone();
                        let previous_service_tier = active.selected_service_tier.clone();
                        active.selected_model = Some(model.clone());
                        active.model_overridden = true;
                        update_reasoning_options(active);
                        update_service_tier_options(active);
                        emit_model_options(active, &emit);
                        emit_reasoning_options(active, &emit);
                        emit_service_tier_options(active, &emit);
                        emit(Event::Usage {
                            identity: identity.clone(),
                            usage: None,
                        });
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                model: Some(model),
                                effort: active.selected_reasoning.clone(),
                                service_tier: selected_service_tier_wire(active),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Model {
                                            model: previous_model,
                                            reasoning: previous_reasoning,
                                            service_tier: previous_service_tier,
                                        },
                                    );
                                }
                                Err(error) => {
                                    active.selected_model = previous_model;
                                    active.selected_reasoning = previous_reasoning;
                                    active.selected_service_tier = previous_service_tier;
                                    emit_model_options(active, &emit);
                                    emit_reasoning_options(active, &emit);
                                    emit_service_tier_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetReasoning { identity, reasoning } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !reasoning_options(active).iter().any(|option| option.id == reasoning) {
                            emit_error(&emit, &identity, "The selected reasoning effort is unavailable for this model");
                            continue;
                        }
                        let previous = active.selected_reasoning.clone();
                        active.selected_reasoning = Some(reasoning.clone());
                        active.reasoning_overridden = true;
                        emit_reasoning_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                effort: Some(reasoning),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Reasoning(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_reasoning = previous;
                                    emit_reasoning_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetPersonality { identity, personality } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !personality_options().iter().any(|option| option.id == personality) {
                            emit_error(&emit, &identity, "The selected Codex personality is unavailable");
                            continue;
                        }
                        let previous = active.selected_personality.clone();
                        active.selected_personality = Some(personality.clone());
                        active.personality_overridden = true;
                        emit_personality_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                personality: Some(personality),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Personality(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_personality = previous;
                                    emit_personality_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetServiceTier { identity, service_tier } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !service_tier_options(active).iter().any(|option| option.id == service_tier) {
                            emit_error(&emit, &identity, "The selected response speed is unavailable for this model");
                            continue;
                        }
                        let previous = active.selected_service_tier.clone();
                        active.selected_service_tier = Some(service_tier);
                        active.service_tier_overridden = true;
                        emit_service_tier_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                service_tier: selected_service_tier_wire(active),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::ServiceTier(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_service_tier = previous;
                                    emit_service_tier_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::SetPermissions { identity, permissions } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if !active.permission_options.iter().any(|option| option.id == permissions) {
                            emit_error(&emit, &identity, "The selected Codex permission profile is no longer available");
                            continue;
                        }
                        let previous = active.selected_permissions.clone();
                        active.selected_permissions = Some(permissions.clone());
                        active.permissions_overridden = true;
                        emit_permission_options(active, &emit);
                        if let Some(thread_id) = active.thread_id.clone() {
                            match active.server.thread_settings_update(ThreadSettingsUpdateParams {
                                thread_id,
                                permissions: Some(permissions),
                                ..Default::default()
                            }) {
                                Ok(request_id) => {
                                    active.pending_settings.insert(
                                        request_id,
                                        PendingSetting::Permissions(previous),
                                    );
                                }
                                Err(error) => {
                                    active.selected_permissions = previous;
                                    emit_permission_options(active, &emit);
                                    emit_error(&emit, &identity, &error.to_string());
                                }
                            }
                        }
                    }
                    Command::Resume { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before switching chats");
                            continue;
                        }
                        if active.thread_id.as_deref() == Some(thread_id.as_str()) {
                            continue;
                        }
                        let previous_thread = active.thread_id.clone();
                        match active
                            .server
                            .thread_resume(thread_resume_params(active, thread_id))
                        {
                            Ok(_) => {
                                active.resume_pending = true;
                                active.resume_previous_thread = previous_thread;
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Opening Codex chat…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::FilterThreads {
                        identity,
                        query,
                        archived,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        active.thread_list_query = query.trim().to_owned();
                        active.thread_list_archived = archived;
                        if let Err(error) = request_thread_list(active) {
                            emit_error(&emit, &identity, &error);
                        }
                    }
                    Command::RenameThread {
                        identity,
                        thread_id,
                        name,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let name = name.trim();
                        if name.is_empty() {
                            emit_error(&emit, &identity, "A Codex thread name cannot be empty");
                            continue;
                        }
                        match active.server.thread_set_name(ThreadSetNameParams {
                            thread_id: thread_id.clone(),
                            name: name.to_owned(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Rename {
                                        thread_id,
                                        name: name.to_owned(),
                                    },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::ArchiveThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_archive(ThreadArchiveParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Archive { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::UnarchiveThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_unarchive(ThreadUnarchiveParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Unarchive { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::DeleteThread { identity, thread_id } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        match active.server.thread_delete(ThreadDeleteParams {
                            thread_id: thread_id.clone(),
                        }) {
                            Ok(request_id) => {
                                active.pending_thread_operations.insert(
                                    request_id,
                                    PendingThreadOperation::Delete { thread_id },
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::RunActiveThreadAction { identity, action } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(thread_id) = active.thread_id.clone() else {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before changing the thread");
                            continue;
                        }
                        let result = match action {
                            ActiveThreadAction::Fork => active.server.thread_fork(ThreadForkParams {
                                thread_id,
                                model: active.selected_model.clone(),
                                service_tier: selected_service_tier_wire(active),
                                cwd: Some(active.workspace_root.clone()),
                                permissions: active.selected_permissions.clone(),
                                ..Default::default()
                            }),
                            ActiveThreadAction::Compact => active.server.thread_compact_start(
                                ThreadCompactStartParams { thread_id },
                            ),
                            ActiveThreadAction::Rollback => active.server.thread_rollback(
                                ThreadRollbackParams {
                                    thread_id,
                                    num_turns: 1,
                                },
                            ),
                        };
                        match result {
                            Ok(_) if action == ActiveThreadAction::Fork => {
                                active.resume_pending = true;
                                active.resume_previous_thread = active.thread_id.clone();
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Initializing,
                                    Some("Forking Codex chat…".to_owned()),
                                );
                            }
                            Ok(_) => {}
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::StartReview { identity, target, detached } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(thread_id) = active.thread_id.clone() else {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        };
                        if active.active_turn_id.is_some() {
                            emit_error(&emit, &identity, "Stop the current Codex turn before starting a review");
                            continue;
                        }
                        let target = match target {
                            ReviewTarget::UncommittedChanges => CodexReviewTarget::UncommittedChanges,
                            ReviewTarget::BaseBranch(branch) => CodexReviewTarget::BaseBranch { branch },
                            ReviewTarget::Commit(sha) => CodexReviewTarget::Commit { sha, title: None },
                            ReviewTarget::Custom(instructions) => CodexReviewTarget::Custom { instructions },
                        };
                        match active.server.review_start(ReviewStartParams {
                            thread_id,
                            target,
                            delivery: Some(if detached {
                                CodexReviewDelivery::Detached
                            } else {
                                CodexReviewDelivery::Inline
                            }),
                        }) {
                            Ok(request_id) => {
                                active.pending_reviews.insert(request_id, detached);
                                emit_state(
                                    &emit,
                                    &identity,
                                    SessionState::Running,
                                    Some("Codex is reviewing…".to_owned()),
                                );
                            }
                            Err(error) => emit_error(&emit, &identity, &error.to_string()),
                        }
                    }
                    Command::RunTool { identity, action } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        if active.thread_id.is_none() {
                            emit_error(&emit, &identity, "There is no active Codex thread");
                            continue;
                        }
                        for (pending, request) in start_tool_requests(active, action) {
                            match request {
                                Ok(request_id) => {
                                    active.pending_tools.insert(request_id, pending);
                                }
                                Err(error) => emit_error(
                                    &emit,
                                    &identity,
                                    &format!("{} failed: {error}", pending.title()),
                                ),
                            }
                        }
                    }
                    Command::Respond {
                        identity,
                        request_key,
                        response,
                    } => {
                        let Some(active) = session.as_mut().filter(|active| active.matches(&identity)) else {
                            continue;
                        };
                        let Some(pending) = active.pending_requests.remove(&request_key) else {
                            emit_error(&emit, &identity, "That Codex request is no longer pending");
                            continue;
                        };
                        match response_for_server_request(
                            &pending.method,
                            &pending.params,
                            response,
                        ) {
                            Ok(result) => match active.server.respond(pending.id.clone(), result) {
                                Ok(()) => emit(Event::RequestResolved {
                                    identity: identity.clone(),
                                    request_key,
                                }),
                                Err(error) => {
                                    active.pending_requests.insert(request_key.clone(), pending);
                                    emit_error(&emit, &identity, &error.to_string());
                                    if let Some(pending) = active.pending_requests.get(&request_key)
                                        && let Some(request) = pending_request_from_server(
                                            &request_key,
                                            &pending.method,
                                            &pending.params,
                                        )
                                    {
                                        emit(Event::Request {
                                            identity: identity.clone(),
                                            request,
                                        });
                                    }
                                }
                            },
                            Err(error) => {
                                active.pending_requests.insert(request_key.clone(), pending);
                                emit_error(&emit, &identity, &error);
                            }
                        }
                    }
                    Command::Reset => {
                        shutdown_session(session.take(), &emit).await;
                        emit(Event::Cleared);
                    }
                    Command::Shutdown { completed } => {
                        shutdown_session(session.take(), &emit).await;
                        if let Some(completed) = completed {
                            let _ = completed.send(());
                        }
                        return;
                    }
                }
            }
            _ = ticker.tick(), if session.is_some() => {
                let cancelled = session
                    .as_ref()
                    .is_some_and(|active| active.cancellation.is_cancelled());
                if cancelled {
                    shutdown_session(session.take(), &emit).await;
                    emit(Event::Cleared);
                    continue;
                }
                let mut close = false;
                if let Some(active) = session.as_mut() {
                    for _ in 0..EVENT_DRAIN_LIMIT {
                        match active.server.try_recv() {
                            Ok(event) => {
                                if handle_server_event(active, event, &emit).await {
                                    close = true;
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                close = true;
                                break;
                            }
                        }
                    }
                }
                if close {
                    shutdown_session(session.take(), &emit).await;
                }
            }
        }
    }
}
