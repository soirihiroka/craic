fn pending_request_from_server(key: &str, method: &str, params: &Value) -> Option<PendingRequest> {
    let approval = |title: &str, fallback: &str, command: bool| PendingRequest {
        key: key.to_owned(),
        title: title.to_owned(),
        message: approval_description(params, fallback),
        options: approval_options(params, command),
        allows_text: false,
        multiline_text: false,
        text_placeholder: None,
        secret: false,
    };
    match method {
        "item/commandExecution/requestApproval" => Some(approval(
            "Run command?",
            "Codex wants to run a command.",
            true,
        )),
        "item/fileChange/requestApproval" => Some(approval(
            "Apply file changes?",
            "Codex wants to modify files.",
            false,
        )),
        "item/permissions/requestApproval" => Some(PendingRequest {
            key: key.to_owned(),
            title: "Grant additional permissions?".to_owned(),
            message: approval_description(params, "Codex requested additional access."),
            options: map_approval_options(permission_approval_options()),
            allows_text: false,
            multiline_text: false,
            text_placeholder: None,
            secret: false,
        }),
        "item/tool/requestUserInput" => {
            let questions = params
                .get("questions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let first = questions.first().cloned().unwrap_or(Value::Null);
            let multiple = questions.len() > 1;
            let options = if multiple {
                Vec::new()
            } else {
                first
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|option| {
                        let label = option.get("label").and_then(Value::as_str)?;
                        Some(request_option(label, label, false))
                    })
                    .collect()
            };
            let message = questions
                .iter()
                .map(|question| {
                    question
                        .get("question")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex needs input")
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            Some(PendingRequest {
                key: key.to_owned(),
                title: first
                    .get("header")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex needs input")
                    .to_owned(),
                message,
                allows_text: multiple
                    || options.is_empty()
                    || first.get("isOther").and_then(Value::as_bool) == Some(true),
                multiline_text: false,
                options,
                text_placeholder: Some(if multiple {
                    r#"{"question_id":"answer"}"#.to_owned()
                } else {
                    "Enter your response".to_owned()
                }),
                secret: questions.iter().any(|question| {
                    question.get("isSecret").and_then(Value::as_bool) == Some(true)
                }),
            })
        }
        "mcpServer/elicitation/request" => {
            let mode = params.get("mode").and_then(Value::as_str);
            let message = params
                .get("message")
                .or_else(|| params.get("description"))
                .and_then(Value::as_str)
                .unwrap_or("An MCP server needs additional input.")
                .to_owned();
            Some(PendingRequest {
                key: key.to_owned(),
                title: params
                    .get("serverName")
                    .and_then(Value::as_str)
                    .map(|name| format!("{name} needs input"))
                    .unwrap_or_else(|| "MCP server request".to_owned()),
                message,
                options: if mode == Some("url") {
                    vec![
                        request_option("accept", "Acknowledge URL", false),
                        request_option("decline", "Decline", false),
                        request_option("cancel", "Cancel", true),
                    ]
                } else {
                    Vec::new()
                },
                allows_text: mode != Some("url"),
                multiline_text: false,
                text_placeholder: (mode != Some("url")).then(|| "Enter JSON response".to_owned()),
                secret: params
                    .get("requestedSchema")
                    .is_some_and(schema_contains_secret),
            })
        }
        "item/tool/call" => {
            let tool = params
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let qualified_tool = params
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|namespace| !namespace.is_empty())
                .map(|namespace| format!("{namespace}/{tool}"))
                .unwrap_or_else(|| tool.to_owned());
            let mut display_tool = qualified_tool.chars().take(96).collect::<String>();
            if display_tool.chars().count() < qualified_tool.chars().count() {
                display_tool.push('…');
            }
            Some(PendingRequest {
                key: key.to_owned(),
                title: format!("Dynamic tool: {display_tool}"),
                message: params
                    .get("arguments")
                    .map(compact_request_json)
                    .unwrap_or_default(),
                options: vec![request_option("fail", "Report Failure", true)],
                allows_text: true,
                multiline_text: true,
                text_placeholder: Some(format!("Return output for {display_tool}")),
                secret: false,
            })
        }
        _ => None,
    }
}
fn request_option(value: &str, label: &str, destructive: bool) -> RequestOption {
    RequestOption {
        value: value.to_owned(),
        label: label.to_owned(),
        destructive,
    }
}

fn approval_options(params: &Value, command: bool) -> Vec<RequestOption> {
    map_approval_options(shared_approval_options(params, command))
}

fn map_approval_options(options: Vec<ApprovalOption>) -> Vec<RequestOption> {
    options
        .into_iter()
        .map(|option| RequestOption {
            value: option.value,
            label: option.label,
            destructive: option.style == ApprovalOptionStyle::Destructive,
        })
        .collect()
}

fn schema_contains_secret(value: &Value) -> bool {
    match value {
        Value::Object(values) => {
            ["isSecret", "secret", "sensitive", "writeOnly"]
                .into_iter()
                .any(|key| values.get(key).and_then(Value::as_bool) == Some(true))
                || values.values().any(schema_contains_secret)
        }
        Value::Array(values) => values.iter().any(schema_contains_secret),
        _ => false,
    }
}

fn response_for_server_request(
    method: &str,
    params: &Value,
    response: RequestResponse,
) -> Result<Value, String> {
    if method == "item/tool/call" {
        return match response {
            RequestResponse::Text(text) => Ok(serde_json::json!({
                "contentItems": [{ "type": "inputText", "text": text }],
                "success": true
            })),
            RequestResponse::Choice(text) if text != "fail" => Ok(serde_json::json!({
                "contentItems": [{ "type": "inputText", "text": text }],
                "success": true
            })),
            RequestResponse::Choice(_) | RequestResponse::Cancel => Ok(serde_json::json!({
                "contentItems": [],
                "success": false
            })),
        };
    }
    let value = match response {
        RequestResponse::Choice(value) | RequestResponse::Text(value) => value,
        RequestResponse::Cancel => match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                "decline".to_owned()
            }
            "item/permissions/requestApproval" => "decline".to_owned(),
            "mcpServer/elicitation/request" => "cancel".to_owned(),
            "item/tool/requestUserInput" => String::new(),
            _ => return Err(format!("Unsupported Codex server request: {method}")),
        },
    };
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Ok(approval_decision_response(value))
        }
        "item/permissions/requestApproval" => Ok(permission_approval_response(params, &value)),
        "item/tool/requestUserInput" => user_input_response(params, &value),
        "mcpServer/elicitation/request" => {
            if matches!(value.as_str(), "decline" | "cancel") {
                Ok(serde_json::json!({ "action": value, "content": null, "_meta": null }))
            } else if value == "accept" {
                Ok(serde_json::json!({ "action": "accept", "content": null, "_meta": null }))
            } else {
                Ok(serde_json::json!({
                    "action": "accept",
                    "content": serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value)),
                    "_meta": null
                }))
            }
        }
        _ => Err(format!("Unsupported Codex server request: {method}")),
    }
}

fn user_input_response(params: &Value, value: &str) -> Result<Value, String> {
    let questions = params
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "Codex user-input request did not contain questions".to_owned())?;
    if questions.len() == 1 {
        let id = questions[0]
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "Codex user-input question did not contain an id".to_owned())?;
        let mut answers = serde_json::Map::new();
        answers.insert(id.to_owned(), serde_json::json!({ "answers": [value] }));
        return Ok(serde_json::json!({ "answers": answers }));
    }
    let values = serde_json::from_str::<serde_json::Map<String, Value>>(value)
        .map_err(|error| format!("Enter a JSON object containing each answer: {error}"))?;
    let answers = questions
        .iter()
        .filter_map(|question| {
            let id = question.get("id")?.as_str()?;
            let value = values
                .get(id)
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let values = match value {
                Value::Array(values) => values,
                value => vec![value],
            };
            Some((id.to_owned(), serde_json::json!({ "answers": values })))
        })
        .collect::<serde_json::Map<_, _>>();
    Ok(serde_json::json!({ "answers": answers }))
}

async fn handle_notification<F>(
    session: &mut Session,
    method: &str,
    params: Option<Value>,
    emit: &F,
) where
    F: Fn(Event),
{
    let params = params.unwrap_or(Value::Null);
    let identity = session.identity.clone();
    match method {
        "serverRequest/resolved" => {
            if let Some(request_id) = params.get("requestId").cloned()
                && let Ok(request_id) = serde_json::from_value::<RequestId>(request_id)
            {
                let request_key = request_id_key(&request_id);
                if session.pending_requests.remove(&request_key).is_some() {
                    emit(Event::RequestResolved {
                        identity,
                        request_key,
                    });
                }
            }
        }
        "thread/started" => {
            if session.thread_id.is_none()
                && let Some(thread_id) = params.pointer("/thread/id").and_then(Value::as_str)
            {
                session.thread_id = Some(thread_id.to_owned());
            }
        }
        "thread/settings/updated" => {
            let settings = params.get("threadSettings").unwrap_or(&params);
            if let Some(model) = settings.get("model").and_then(Value::as_str) {
                session.selected_model = Some(model.to_owned());
                emit_model_options(session, emit);
            }
            if let Some(reasoning) = settings
                .get("effort")
                .or_else(|| settings.get("reasoningEffort"))
                .and_then(Value::as_str)
            {
                session.selected_reasoning = Some(reasoning.to_owned());
            }
            update_reasoning_options(session);
            update_service_tier_options(session);
            update_personality_options(session);
            emit_reasoning_options(session, emit);
            if let Some(personality) = settings.get("personality").and_then(Value::as_str) {
                session.selected_personality = Some(personality.to_owned());
            }
            emit_personality_options(session, emit);
            if let Some(service_tier) = settings.get("serviceTier").and_then(Value::as_str) {
                session.selected_service_tier = Some(service_tier.to_owned());
            } else if settings.get("serviceTier").is_some_and(Value::is_null) {
                session.selected_service_tier = Some(DEFAULT_SERVICE_TIER_ID.to_owned());
            }
            emit_service_tier_options(session, emit);
            if let Some(permissions) = settings
                .pointer("/activePermissionProfile/id")
                .or_else(|| settings.get("permissions"))
                .and_then(Value::as_str)
            {
                session.selected_permissions = Some(permissions.to_owned());
                emit_permission_options(session, emit);
            }
        }
        "thread/name/updated" => {
            if let (Some(thread_id), Some(name)) = (
                params.get("threadId").and_then(Value::as_str),
                params
                    .get("name")
                    .or_else(|| params.pointer("/thread/name"))
                    .and_then(Value::as_str),
            ) && session.thread_id.as_deref() == Some(thread_id)
            {
                session.thread_title = Some(name.to_owned());
                persist_thread_overlay(
                    session.workspace_key.clone(),
                    session.thread_id.clone(),
                    Some(name.to_owned()),
                )
                .await;
                emit(Event::ThreadReady {
                    identity: identity.clone(),
                    thread_id: thread_id.to_owned(),
                    title: Some(name.to_owned()),
                });
            }
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/compacted" => {
            session.next_local_id = session.next_local_id.wrapping_add(1);
            let item = TranscriptItem {
                id: format!("craic-native-compaction-{}", session.next_local_id),
                kind: TranscriptKind::Compaction,
                status: TranscriptStatus::Completed,
                title: Some("Context compacted".to_owned()),
                body: "Codex compacted this conversation's context.".to_owned(),
                detail: None,
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "thread/archived" | "thread/deleted" | "thread/closed" => {
            if let Some(thread_id) = params
                .get("threadId")
                .or_else(|| params.pointer("/thread/id"))
                .and_then(Value::as_str)
            {
                close_active_thread(
                    session,
                    thread_id,
                    if method == "thread/archived" {
                        "Codex thread archived"
                    } else if method == "thread/deleted" {
                        "Codex thread deleted"
                    } else {
                        "Codex thread closed"
                    },
                    emit,
                );
            }
            if method != "thread/closed"
                && let Err(error) = request_thread_list(session)
            {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/unarchived" => {
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "thread/tokenUsage/updated" => apply_token_usage(session, &params, emit),
        "hook/started" | "hook/completed" => {
            let run = params.get("run").unwrap_or(&params);
            let id = run.get("id").and_then(Value::as_str).unwrap_or("current");
            let event = run
                .get("eventName")
                .and_then(Value::as_str)
                .unwrap_or("hook");
            let completed = method == "hook/completed";
            let status = transcript_status(run.get("status").and_then(Value::as_str), completed);
            let item = TranscriptItem {
                id: format!("hook:{id}"),
                kind: TranscriptKind::Tool,
                status,
                title: Some(format!("Hook · {}", craic_agent::display::title_case(event))),
                body: run
                    .get("statusMessage")
                    .and_then(Value::as_str)
                    .unwrap_or(if completed {
                        "Hook completed."
                    } else {
                        "Hook running."
                    })
                    .to_owned(),
                detail: run
                    .get("entries")
                    .filter(|entries| {
                        entries
                            .as_array()
                            .is_some_and(|entries| !entries.is_empty())
                    })
                    .map(compact_json),
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "turn/started" => {
            session.active_turn_id = params
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            emit_state(
                emit,
                &identity,
                SessionState::Running,
                Some("Codex is working…".to_owned()),
            );
        }
        "turn/completed" => {
            session.active_turn_id = None;
            let failed = params.pointer("/turn/status").and_then(Value::as_str) == Some("failed");
            if failed
                && let Some(message) = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
            {
                emit_error(emit, &identity, message);
            }
            emit_state(
                emit,
                &identity,
                SessionState::Ready,
                Some("Ready".to_owned()),
            );
            if let Err(error) = request_thread_list(session) {
                emit_error(emit, &identity, &error);
            }
        }
        "item/started" | "item/completed" => {
            if let Some(item) = params.get("item")
                && let Some(item) = transcript_from_item(item, method == "item/completed")
            {
                session.timeline.insert(item.id.clone(), item.clone());
                emit(Event::Upsert { identity, item });
            }
        }
        "item/agentMessage/delta"
        | "item/plan/delta"
        | "item/reasoning/summaryTextDelta"
        | "item/reasoning/textDelta"
        | "item/commandExecution/outputDelta"
        | "item/fileChange/outputDelta" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(Value::as_str) else {
                return;
            };
            if delta.is_empty() {
                return;
            }
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: transcript_delta_kind(method),
                    status: TranscriptStatus::Running,
                    title: activity_title(method),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            if matches!(
                method,
                "item/commandExecution/outputDelta"
                    | "item/fileChange/outputDelta"
                    | "item/reasoning/textDelta"
            ) {
                item.detail.get_or_insert_with(String::new).push_str(delta);
            } else {
                item.body.push_str(delta);
            }
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/reasoning/summaryPartAdded" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let summary_index = params
                .get("summaryIndex")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::Reasoning,
                    status: TranscriptStatus::Running,
                    title: Some("Reasoning".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(format!(
                "Reasoning summary part {} started.",
                summary_index + 1
            ));
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/commandExecution/terminalInteraction" => {
            let id = params
                .get("itemId")
                .and_then(Value::as_str)
                .or_else(|| params.get("processId").and_then(Value::as_str))
                .unwrap_or("current");
            let process_id = params
                .get("processId")
                .and_then(Value::as_str)
                .unwrap_or("process");
            let stdin = params
                .get("stdin")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let item = session
                .timeline
                .entry(id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: id.to_owned(),
                    kind: TranscriptKind::Command,
                    status: TranscriptStatus::Running,
                    title: Some("Terminal interaction".to_owned()),
                    body: format!("Sent input to {process_id}."),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            let detail = item.detail.get_or_insert_with(String::new);
            if !detail.is_empty() && !detail.ends_with('\n') {
                detail.push('\n');
            }
            detail.push_str("Input: ");
            detail.push_str(if stdin.is_empty() { "<empty>" } else { stdin });
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/fileChange/patchUpdated" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let detail = params
                .get("patch")
                .or_else(|| params.get("changes"))
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| compact_json(value))
                })
                .unwrap_or_default();
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::FileChange,
                    status: TranscriptStatus::Running,
                    title: Some("File changes".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(detail);
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "item/mcpToolCall/progress" => {
            let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
                return;
            };
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("MCP tool is working.");
            let item = session
                .timeline
                .entry(item_id.to_owned())
                .or_insert_with(|| TranscriptItem {
                    id: item_id.to_owned(),
                    kind: TranscriptKind::McpTool,
                    status: TranscriptStatus::Running,
                    title: Some("MCP tool".to_owned()),
                    body: String::new(),
                    detail: None,
                    image: None,
                });
            item.status = TranscriptStatus::Running;
            item.detail = Some(message.to_owned());
            emit(Event::Upsert {
                identity,
                item: item.clone(),
            });
        }
        "turn/diff/updated" => {
            let turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("current");
            let item = TranscriptItem {
                id: format!("turn-diff:{turn_id}"),
                kind: TranscriptKind::FileChange,
                status: TranscriptStatus::Running,
                title: Some("Turn changes".to_owned()),
                body: params
                    .get("diff")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                detail: None,
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "turn/plan/updated" => {
            let turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("current");
            let body = params
                .get("plan")
                .and_then(Value::as_array)
                .map(|steps| {
                    steps
                        .iter()
                        .map(|step| {
                            let marker = match step.get("status").and_then(Value::as_str) {
                                Some("completed") => "✓",
                                Some("inProgress") => "→",
                                Some("failed") => "×",
                                _ => "·",
                            };
                            format!(
                                "{marker} {}",
                                step.get("step")
                                    .and_then(Value::as_str)
                                    .unwrap_or("Unnamed step")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let item = TranscriptItem {
                id: format!("turn-plan:{turn_id}"),
                kind: TranscriptKind::Plan,
                status: TranscriptStatus::Running,
                title: Some("Plan".to_owned()),
                body,
                detail: params
                    .get("explanation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                image: None,
            };
            session.timeline.insert(item.id.clone(), item.clone());
            emit(Event::Upsert { identity, item });
        }
        "error" => {
            let message = params
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Codex reported an error");
            if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                emit_warning(emit, &identity, "Codex is retrying", message);
            } else {
                emit_error(emit, &identity, message);
            }
        }
        "warning" | "guardianWarning" | "configWarning" => {
            let message = params
                .get("message")
                .or_else(|| params.get("details"))
                .and_then(Value::as_str)
                .unwrap_or("Codex reported a warning");
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Codex warning");
            emit_warning(emit, &identity, title, message);
        }
        _ => {}
    }
}
