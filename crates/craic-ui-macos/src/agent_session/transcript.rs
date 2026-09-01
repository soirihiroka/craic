fn apply_token_usage<F>(session: &Session, params: &Value, emit: &F)
where
    F: Fn(Event),
{
    if params
        .get("threadId")
        .and_then(Value::as_str)
        .is_some_and(|thread_id| session.thread_id.as_deref() != Some(thread_id))
    {
        return;
    }
    let Some(total) = params.pointer("/tokenUsage/total") else {
        return;
    };
    let last = params.pointer("/tokenUsage/last").unwrap_or(total);
    emit(Event::Usage {
        identity: session.identity.clone(),
        usage: Some(TokenUsage {
            input_tokens: nonnegative_u64(total.get("inputTokens")),
            cache_write_input_tokens: nonnegative_u64(total.get("cacheWriteInputTokens")),
            cached_input_tokens: nonnegative_u64(total.get("cachedInputTokens")),
            output_tokens: nonnegative_u64(total.get("outputTokens")),
            reasoning_output_tokens: nonnegative_u64(total.get("reasoningOutputTokens")),
            total_tokens: nonnegative_u64(total.get("totalTokens")),
            last_total_tokens: nonnegative_u64(last.get("totalTokens")),
            context_limit: params
                .pointer("/tokenUsage/modelContextWindow")
                .and_then(Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value > 0)
                .or(session.context_window_fallback),
        }),
    });
}

fn nonnegative_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default()
}

fn transcript_from_item(item: &Value, completed: bool) -> Option<TranscriptItem> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    if item_type == "userMessage" {
        return None;
    }
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(item_type)
        .to_owned();
    let (kind, title, body, detail) = match item_type {
        "agentMessage" => (
            TranscriptKind::Assistant,
            Some("Codex".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "plan" => (
            TranscriptKind::Plan,
            Some("Plan".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "reasoning" => (
            TranscriptKind::Reasoning,
            Some("Reasoning".to_owned()),
            flatten_text(item.get("summary")),
            nonempty(flatten_text(item.get("content"))),
        ),
        "hookPrompt" => (
            TranscriptKind::Developer,
            Some("Hook prompt".to_owned()),
            item.get("fragments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|fragment| fragment.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
            None,
        ),
        "commandExecution" => (
            TranscriptKind::Command,
            Some("Command".to_owned()),
            item.get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("aggregatedOutput")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "fileChange" => (
            TranscriptKind::FileChange,
            Some("File changes".to_owned()),
            "Codex updated workspace files.".to_owned(),
            item.get("changes").map(compact_json),
        ),
        "mcpToolCall" => (
            TranscriptKind::McpTool,
            Some(format!(
                "{} / {}",
                item.get("server").and_then(Value::as_str).unwrap_or("MCP"),
                item.get("tool").and_then(Value::as_str).unwrap_or("tool")
            )),
            item.get("arguments").map(compact_json).unwrap_or_default(),
            item.get("result")
                .or_else(|| item.get("error"))
                .map(compact_json),
        ),
        "dynamicToolCall" => (
            TranscriptKind::Tool,
            Some(
                item.get("namespace")
                    .and_then(Value::as_str)
                    .filter(|namespace| !namespace.is_empty())
                    .map(|namespace| {
                        format!(
                            "{namespace} / {}",
                            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
                        )
                    })
                    .unwrap_or_else(|| {
                        item.get("tool")
                            .and_then(Value::as_str)
                            .unwrap_or("Dynamic tool")
                            .to_owned()
                    }),
            ),
            item.get("arguments").map(compact_json).unwrap_or_default(),
            item.get("contentItems").map(compact_json),
        ),
        "webSearch" => (
            TranscriptKind::Web,
            Some("Web search".to_owned()),
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("results").map(compact_json),
        ),
        "imageView" => (
            TranscriptKind::Image,
            Some("Image".to_owned()),
            item.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "imageGeneration" => (
            TranscriptKind::Image,
            Some("Generated image".to_owned()),
            item.get("savedPath")
                .and_then(Value::as_str)
                .or_else(|| {
                    item.get("result")
                        .and_then(Value::as_str)
                        .filter(|result| !result.starts_with("data:"))
                })
                .unwrap_or("Image generated")
                .to_owned(),
            item.get("revisedPrompt")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "collabAgentToolCall" => (
            TranscriptKind::Collaboration,
            Some("Collaboration".to_owned()),
            item.get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    item.get("tool")
                        .and_then(Value::as_str)
                        .map(craic_agent::display::title_case)
                        .unwrap_or_else(|| "Agent activity".to_owned())
                }),
            Some(compact_json(item)),
        ),
        "subAgentActivity" => (
            TranscriptKind::Collaboration,
            Some(format!(
                "Subagent {}",
                item.get("kind")
                    .and_then(Value::as_str)
                    .map(craic_agent::display::title_case)
                    .unwrap_or_else(|| "activity".to_owned())
            )),
            item.get("agentPath")
                .and_then(Value::as_str)
                .unwrap_or("Subagent")
                .to_owned(),
            item.get("agentThreadId")
                .and_then(Value::as_str)
                .map(|thread_id| format!("Thread {thread_id}")),
        ),
        "enteredReviewMode" | "exitedReviewMode" => (
            TranscriptKind::Review,
            Some("Review".to_owned()),
            item.get("review")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "contextCompaction" => (
            TranscriptKind::Compaction,
            Some("Context compacted".to_owned()),
            "Codex compacted this conversation's context.".to_owned(),
            None,
        ),
        "sleep" => (
            TranscriptKind::Tool,
            Some("Waiting".to_owned()),
            item.get("durationMs")
                .and_then(Value::as_u64)
                .map(|duration| format!("Waiting for {duration} ms"))
                .unwrap_or_default(),
            None,
        ),
        other => (
            TranscriptKind::Tool,
            Some(craic_agent::display::title_case(other)),
            compact_json(item),
            None,
        ),
    };
    Some(TranscriptItem {
        id,
        kind,
        status: transcript_status(item.get("status").and_then(Value::as_str), completed),
        title,
        body,
        detail,
        image: transcript_image_source(item_type, item),
    })
}

fn transcript_image_source(item_type: &str, item: &Value) -> Option<TranscriptImageSource> {
    match item_type {
        "imageView" => item
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| TranscriptImageSource::WorkspacePath(path.to_owned())),
        "imageGeneration" => item
            .get("savedPath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(|path| TranscriptImageSource::WorkspacePath(path.to_owned()))
            .or_else(|| {
                item.get("result")
                    .and_then(Value::as_str)
                    .filter(|result| result.starts_with("data:image/"))
                    .map(|result| TranscriptImageSource::DataUri(result.to_owned()))
            }),
        _ => None,
    }
}

fn transcript_status(status: Option<&str>, completed: bool) -> TranscriptStatus {
    match status {
        Some("failed" | "declined" | "denied") => TranscriptStatus::Failed,
        Some("interrupted" | "cancelled" | "canceled" | "aborted" | "timedOut") => {
            TranscriptStatus::Interrupted
        }
        Some("completed" | "ready" | "approved" | "success") => TranscriptStatus::Completed,
        Some("inProgress" | "running") => TranscriptStatus::Running,
        _ if completed => TranscriptStatus::Completed,
        _ => TranscriptStatus::Running,
    }
}

fn activity_title(method: &str) -> Option<String> {
    match method {
        "item/plan/delta" => Some("Plan".to_owned()),
        method if method.starts_with("item/reasoning/") => Some("Reasoning".to_owned()),
        "item/commandExecution/outputDelta" => Some("Command".to_owned()),
        "item/fileChange/outputDelta" => Some("File changes".to_owned()),
        _ => Some("Codex".to_owned()),
    }
}

fn transcript_delta_kind(method: &str) -> TranscriptKind {
    match method {
        "item/agentMessage/delta" => TranscriptKind::Assistant,
        "item/plan/delta" => TranscriptKind::Plan,
        method if method.starts_with("item/reasoning/") => TranscriptKind::Reasoning,
        "item/commandExecution/outputDelta" => TranscriptKind::Command,
        "item/fileChange/outputDelta" => TranscriptKind::FileChange,
        _ => TranscriptKind::Tool,
    }
}

fn flatten_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| flatten_text(Some(value)))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(value)) => value
            .get("text")
            .or_else(|| value.get("content"))
            .map(|value| flatten_text(Some(value)))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn parse_background_terminals(value: &Value) -> Vec<BackgroundTerminal> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|terminal| {
            let process_id = terminal.get("processId")?.as_str()?.to_owned();
            let command = terminal
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("Background command")
                .to_owned();
            let mut details = Vec::new();
            if let Some(cwd) = terminal.get("cwd").and_then(Value::as_str)
                && !cwd.is_empty()
            {
                details.push(cwd.to_owned());
            }
            if let Some(pid) = terminal.get("osPid").and_then(Value::as_u64) {
                details.push(format!("PID {pid}"));
            }
            if let Some(cpu) = terminal.get("cpuPercent").and_then(Value::as_f64) {
                details.push(format!("CPU {cpu:.1}%"));
            }
            if let Some(rss) = terminal.get("rssKb").and_then(Value::as_u64) {
                details.push(format!("RSS {rss} KiB"));
            }
            Some(BackgroundTerminal {
                process_id,
                command,
                detail: details.join(" · "),
            })
        })
        .collect()
}

fn parse_skills(value: &Value) -> Vec<SkillOption> {
    let mut skills = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            entry
                .get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) != Some(false))
        .filter_map(|skill| {
            Some(SkillOption {
                name: skill.get("name")?.as_str()?.to_owned(),
                description: skill
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                path: skill.get("path")?.as_str()?.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    skills.sort_by_key(|skill| skill.name.to_lowercase());
    skills.dedup_by(|left, right| left.path == right.path);
    skills
}

fn parse_experimental_features(value: &Value) -> Vec<ExperimentalFeature> {
    let mut features = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|feature| {
            let name = feature.get("name")?.as_str()?.to_owned();
            Some(ExperimentalFeature {
                label: feature
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_owned(),
                description: feature
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| feature.get("announcement").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned(),
                enabled: feature.get("enabled")?.as_bool()?,
                name,
            })
        })
        .collect::<Vec<_>>();
    features.sort_by_key(|feature| feature.label.to_lowercase());
    features
}

fn summarize_tool_result(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if let Some(objective) = value
        .get("objective")
        .or_else(|| value.pointer("/goal/objective"))
        .and_then(Value::as_str)
    {
        return objective.to_owned();
    }
    if let Some(items) = value.get("data").and_then(Value::as_array) {
        if items.is_empty() {
            return "No items returned.".to_owned();
        }
        let labels = items
            .iter()
            .take(20)
            .filter_map(|item| {
                item.get("displayName")
                    .or_else(|| item.get("name"))
                    .or_else(|| item.get("title"))
                    .or_else(|| item.get("command"))
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
            })
            .collect::<Vec<_>>();
        if !labels.is_empty() {
            let mut summary = labels.join("\n");
            if items.len() > labels.len() {
                summary.push_str(&format!("\n… and {} more", items.len() - labels.len()));
            }
            return summary;
        }
        return format!("{} items returned.", items.len());
    }
    compact_json(value)
}

fn emit_state<F>(emit: &F, identity: &SessionIdentity, state: SessionState, detail: Option<String>)
where
    F: Fn(Event),
{
    emit(Event::State {
        identity: identity.clone(),
        state,
        detail,
    });
}

fn emit_error<F>(emit: &F, identity: &SessionIdentity, message: &str)
where
    F: Fn(Event),
{
    emit(Event::Upsert {
        identity: identity.clone(),
        item: TranscriptItem {
            id: format!("error:{:016x}", stable_hash(message)),
            kind: TranscriptKind::Error,
            status: TranscriptStatus::Failed,
            title: Some("Error".to_owned()),
            body: message.to_owned(),
            detail: None,
            image: None,
        },
    });
}

fn emit_warning<F>(emit: &F, identity: &SessionIdentity, title: &str, message: &str)
where
    F: Fn(Event),
{
    emit(Event::Upsert {
        identity: identity.clone(),
        item: TranscriptItem {
            id: format!("warning:{:016x}", stable_hash(message)),
            kind: TranscriptKind::Warning,
            status: TranscriptStatus::Completed,
            title: Some(title.to_owned()),
            body: message.to_owned(),
            detail: None,
            image: None,
        },
    });
}

async fn shutdown_session<F>(session: Option<Session>, emit: &F)
where
    F: Fn(Event),
{
    let Some(mut session) = session else {
        return;
    };
    let identity = session.identity.clone();
    emit_state(
        emit,
        &identity,
        SessionState::Stopping,
        Some("Closing Codex…".to_owned()),
    );
    let shutdown = tokio::task::spawn_blocking(move || {
        let uploaded = session
            .pending_turn_media
            .drain()
            .flat_map(|(_, media)| media)
            .collect();
        remove_remote_media(session.remote_media.as_ref(), uploaded);
        session.server.shutdown()
    })
    .await;
    if let Err(error) = shutdown {
        log::warn!("native Codex shutdown task failed: {error}");
    }
    emit_state(emit, &identity, SessionState::Closed, None);
    log::info!(
        "native Codex session stopped workspace={} generation={}",
        identity.workspace_id,
        identity.generation
    );
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
