use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use super::super::codex_chat::{
    CollaborationParticipant, CollaborationParticipantStatus, CollaborationProgress, PlanProgress,
    PlanStep, PlanStepStatus, TimelineItem, TimelineItemKind, TimelineItemStatus, TokenUsage,
};
use super::{AppChatSessionInner, AppChatState, compact_json, title_case};

impl AppChatSessionInner {
    pub(super) fn handle_notification(&self, method: &str, params: Option<Value>) {
        let params = params.unwrap_or(Value::Null);
        self.refresh_picker_for_notification(method);
        if self.targets_other_thread(&params) {
            return;
        }
        match method {
            "thread/started" => self.apply_thread_started(&params),
            "thread/name/updated" => {
                if let Some(name) = params.get("threadName").and_then(Value::as_str) {
                    self.set_title(name);
                    self.persist_overlay(Some(name.to_owned()));
                }
            }
            "thread/goal/updated" => self.apply_thread_goal(&params, false),
            "thread/goal/cleared" => self.apply_thread_goal(&params, true),
            "thread/environment/connected" | "thread/environment/disconnected" => {
                self.apply_environment_status(method, &params)
            }
            "thread/settings/updated" => self.apply_thread_settings(&params),
            "thread/status/changed" => match params.pointer("/status/type").and_then(Value::as_str)
            {
                Some("idle") => {
                    self.active_turn_id.borrow_mut().take();
                    self.clear_pending_requests();
                    self.view.set_turn_active(false);
                    self.set_state(AppChatState::Ready);
                }
                Some("active") => {
                    let waiting = params
                        .pointer("/status/activeFlags")
                        .and_then(Value::as_array)
                        .is_some_and(|flags| {
                            flags.iter().any(|flag| {
                                matches!(
                                    flag.as_str(),
                                    Some("waitingOnApproval" | "waitingOnUserInput")
                                )
                            })
                        });
                    self.view.set_turn_active(true);
                    self.set_state(if waiting {
                        AppChatState::AwaitingInput
                    } else {
                        AppChatState::Running
                    });
                }
                Some("systemError") => {
                    self.fail("Codex thread entered a system-error state".to_owned());
                }
                _ => {}
            },
            "turn/started" => {
                if let Some(turn_id) = params.pointer("/turn/id").and_then(Value::as_str) {
                    self.active_turn_id.replace(Some(turn_id.to_owned()));
                }
                self.set_state(AppChatState::Running);
                self.view.set_turn_active(true);
            }
            "turn/completed" => {
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                if status == "failed"
                    && let Some(message) = params
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                {
                    self.push_error(message.to_owned());
                }
                self.active_turn_id.borrow_mut().take();
                self.clear_pending_requests();
                self.view.set_turn_active(false);
                self.view.set_plan_progress(None);
                self.collaboration.borrow_mut().clear();
                self.view.set_collaboration_progress(None);
                self.set_state(AppChatState::Ready);
                self.submit_next_queued();
            }
            "hook/started" | "hook/completed" => self.apply_hook(method, &params),
            "item/started" => {
                if let Some(item) = params.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("contextCompaction") {
                        self.view.set_usage(None);
                    }
                    self.upsert_timeline(timeline_from_item(item, false));
                    self.update_collaboration_progress(item, false);
                }
            }
            "item/completed" => {
                if let Some(item) = params.get("item") {
                    self.upsert_timeline(timeline_from_item(item, true));
                    self.update_collaboration_progress(item, true);
                }
            }
            "item/autoApprovalReview/started" | "item/autoApprovalReview/completed" => {
                self.apply_auto_approval_review(method, &params)
            }
            "item/agentMessage/delta"
            | "item/plan/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta" => self.append_delta(method, &params),
            "item/reasoning/summaryPartAdded" => self.apply_reasoning_summary_part(&params),
            "item/commandExecution/terminalInteraction" => self.apply_terminal_interaction(&params),
            "item/fileChange/patchUpdated" => self.apply_patch_snapshot(&params),
            "item/mcpToolCall/progress" => self.apply_mcp_tool_progress(&params),
            "turn/diff/updated" => self.upsert_timeline(TimelineItem {
                id: format!(
                    "turn-diff:{}",
                    params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or("current")
                ),
                kind: TimelineItemKind::FileChange,
                title: Some("Turn changes".to_owned()),
                body: params
                    .get("diff")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                detail: None,
                status: TimelineItemStatus::Running,
            }),
            "turn/plan/updated" => self.apply_plan(&params),
            "thread/tokenUsage/updated" => self.apply_token_usage(&params),
            "turn/moderationMetadata" => self.apply_moderation_metadata(&params),
            "serverRequest/resolved" => {
                if let Some(request_id) = params.get("requestId").and_then(request_id_key_value) {
                    self.pending_requests.borrow_mut().remove(&request_id);
                    self.view.resolve_pending_request(&request_id);
                    self.restore_state_after_pending_requests();
                }
            }
            "mcpServer/oauthLogin/completed" => self.apply_mcp_oauth(&params),
            "mcpServer/startupStatus/updated" => self.apply_mcp_startup_status(&params),
            "account/updated" => self.apply_account_update(&params),
            "account/login/completed" => self.apply_account_login(&params),
            "account/rateLimits/updated" => self.apply_rate_limits(&params),
            "app/list/updated" => self.apply_catalog_change("apps", &params),
            "skills/changed" => self.apply_catalog_change("skills", &params),
            "remoteControl/status/changed" => self.apply_remote_control_status(&params),
            "externalAgentConfig/import/progress" | "externalAgentConfig/import/completed" => {
                self.apply_external_import(method, &params)
            }
            "fs/changed" => self.apply_fs_changed(&params),
            "model/rerouted" | "model/verification" | "model/safetyBuffering/updated" => {
                self.apply_model_notification(method, &params)
            }
            "command/exec/outputDelta" | "process/outputDelta" => {
                self.append_process_output(method, &params)
            }
            "process/exited" => self.apply_process_exit(&params),
            "fuzzyFileSearch/sessionUpdated" => self.apply_fuzzy_search_updated(&params),
            "fuzzyFileSearch/sessionCompleted" => self.apply_fuzzy_search_completed(&params),
            "thread/realtime/started"
            | "thread/realtime/itemAdded"
            | "thread/realtime/transcript/delta"
            | "thread/realtime/transcript/done"
            | "thread/realtime/outputAudio/delta"
            | "thread/realtime/sdp"
            | "thread/realtime/error"
            | "thread/realtime/closed" => self.apply_realtime_notification(method, &params),
            "windows/worldWritableWarning" => self.apply_world_writable_warning(&params),
            "windowsSandbox/setupCompleted" => self.apply_windows_sandbox_setup(&params),
            "error" => {
                let message = params
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported an error");
                if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                    self.push_warning("Codex is retrying", message.to_owned());
                } else {
                    self.push_error(message.to_owned());
                }
            }
            "warning" | "guardianWarning" => self.push_warning(
                if method == "guardianWarning" {
                    "Guardian warning"
                } else {
                    "Codex warning"
                },
                params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex reported a warning")
                    .to_owned(),
            ),
            "deprecationNotice" | "configWarning" => {
                let title = params.get("summary").and_then(Value::as_str).unwrap_or(
                    if method == "configWarning" {
                        "Configuration warning"
                    } else {
                        "Deprecated Codex feature"
                    },
                );
                let details = params
                    .get("details")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.push_warning(title, details.to_owned());
            }
            "thread/compacted" => self.upsert_timeline(TimelineItem {
                id: self.next_id("compaction"),
                kind: TimelineItemKind::Compaction,
                title: Some("Context compacted".to_owned()),
                body: "Codex compacted this conversation's context.".to_owned(),
                detail: None,
                status: TimelineItemStatus::Completed,
            }),
            "thread/closed" | "thread/deleted" | "thread/archived" => {
                if self.notification_is_current_thread(&params) {
                    self.request_session_close();
                }
            }
            "thread/unarchived" => self.upsert_timeline(TimelineItem {
                id: format!(
                    "thread-unarchived:{}",
                    params
                        .get("threadId")
                        .and_then(Value::as_str)
                        .unwrap_or("current")
                ),
                kind: TimelineItemKind::Tool,
                title: Some("Thread restored".to_owned()),
                body: "This Codex thread was restored from the archive.".to_owned(),
                detail: None,
                status: TimelineItemStatus::Completed,
            }),
            "rawResponseItem/completed" | "rawResponse/completed" => {
                log::trace!(
                    "ignored internal Codex notification session_id={} method={} turn_id={}",
                    self.id,
                    method,
                    params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                );
            }
            _ => log::warn!(
                "unknown Codex notification session_id={} method={} payload_bytes={}",
                self.id,
                method,
                params.to_string().len()
            ),
        }
    }

    fn apply_thread_started(&self, params: &Value) {
        let Some(thread) = params.get("thread") else {
            return;
        };
        let Some(thread_id) = thread.get("id").and_then(Value::as_str) else {
            return;
        };
        let current_thread_id = self.thread_id.borrow().clone();
        match current_thread_id.as_deref() {
            Some(current) if current != thread_id => return,
            None if *self.lifecycle.borrow() != AppChatState::StartingThread => return,
            None => {
                self.thread_id.replace(Some(thread_id.to_owned()));
            }
            Some(_) => {}
        }
        if let Some(name) = thread
            .get("name")
            .and_then(Value::as_str)
            .or_else(|| thread.get("preview").and_then(Value::as_str))
            .filter(|name| !name.trim().is_empty())
        {
            self.set_title(name);
            self.persist_overlay(Some(name.to_owned()));
        }
    }

    fn apply_thread_goal(&self, params: &Value, cleared: bool) {
        let thread_id = params
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        if cleared {
            self.upsert_timeline(TimelineItem {
                id: format!("thread-goal:{thread_id}"),
                kind: TimelineItemKind::Plan,
                title: Some("Thread goal".to_owned()),
                body: "Thread goal cleared.".to_owned(),
                detail: None,
                status: TimelineItemStatus::Completed,
            });
            return;
        }
        let goal = params.get("goal").unwrap_or(params);
        let status = goal
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("active");
        let mut detail = vec![format!("Status: {}", title_case(status))];
        if let Some(tokens_used) = goal.get("tokensUsed").and_then(Value::as_i64) {
            let token_budget = goal.get("tokenBudget").and_then(Value::as_i64);
            detail.push(token_budget.map_or_else(
                || format!("Tokens used: {tokens_used}"),
                |budget| format!("Tokens: {tokens_used} / {budget}"),
            ));
        }
        if let Some(seconds) = goal.get("timeUsedSeconds").and_then(Value::as_i64) {
            detail.push(format!("Elapsed: {seconds}s"));
        }
        self.upsert_timeline(TimelineItem {
            id: format!("thread-goal:{thread_id}"),
            kind: TimelineItemKind::Plan,
            title: Some("Thread goal".to_owned()),
            body: goal
                .get("objective")
                .and_then(Value::as_str)
                .unwrap_or("Thread goal updated.")
                .to_owned(),
            detail: Some(detail.join("\n")),
            status: match status {
                "complete" => TimelineItemStatus::Completed,
                "blocked" | "usageLimited" | "budgetLimited" => TimelineItemStatus::Interrupted,
                _ => TimelineItemStatus::Running,
            },
        });
    }

    fn apply_environment_status(&self, method: &str, params: &Value) {
        let environment = params
            .get("environmentId")
            .and_then(Value::as_str)
            .unwrap_or("environment");
        let connected = method.ends_with("/connected");
        self.upsert_timeline(TimelineItem {
            id: format!("environment:{environment}"),
            kind: TimelineItemKind::Tool,
            title: Some("Execution environment".to_owned()),
            body: format!(
                "{environment} {}.",
                if connected {
                    "connected"
                } else {
                    "disconnected"
                }
            ),
            detail: None,
            status: if connected {
                TimelineItemStatus::Completed
            } else {
                TimelineItemStatus::Interrupted
            },
        });
    }

    fn apply_hook(&self, method: &str, params: &Value) {
        let run = params.get("run").unwrap_or(params);
        let id = run.get("id").and_then(Value::as_str).unwrap_or("current");
        let event = run
            .get("eventName")
            .and_then(Value::as_str)
            .unwrap_or("hook");
        let completed = method == "hook/completed";
        let status = run
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(if completed { "completed" } else { "running" });
        self.upsert_timeline(TimelineItem {
            id: format!("hook:{id}"),
            kind: TimelineItemKind::Tool,
            title: Some(format!("Hook · {}", title_case(event))),
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
            status: event_status(status, completed),
        });
    }

    fn apply_auto_approval_review(&self, method: &str, params: &Value) {
        let id = params
            .get("reviewId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let review = params.get("review").unwrap_or(params);
        let status = review.get("status").and_then(Value::as_str).unwrap_or(
            if method.ends_with("/completed") {
                "completed"
            } else {
                "inProgress"
            },
        );
        let mut body = review
            .get("rationale")
            .and_then(Value::as_str)
            .unwrap_or("Codex is reviewing an approval request.")
            .to_owned();
        if let Some(risk) = review.get("riskLevel").and_then(Value::as_str) {
            body.push_str(&format!("\nRisk: {}", title_case(risk)));
        }
        self.upsert_timeline(TimelineItem {
            id: format!("approval-review:{id}"),
            kind: TimelineItemKind::Review,
            title: Some("Approval auto-review".to_owned()),
            body,
            detail: params.get("action").map(compact_json),
            status: match status {
                "approved" => TimelineItemStatus::Completed,
                "denied" => TimelineItemStatus::Failed,
                "timedOut" | "aborted" => TimelineItemStatus::Interrupted,
                _ => TimelineItemStatus::Running,
            },
        });
    }

    fn apply_terminal_interaction(&self, params: &Value) {
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
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: id.to_owned(),
                kind: TimelineItemKind::Command,
                title: Some("Terminal interaction".to_owned()),
                body: format!("Sent input to {process_id}."),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        let detail = item.detail.get_or_insert_with(String::new);
        if !detail.is_empty() && !detail.ends_with('\n') {
            detail.push('\n');
        }
        detail.push_str("Input: ");
        detail.push_str(if stdin.is_empty() { "<empty>" } else { stdin });
        let item = item.clone();
        drop(timeline);
        self.view.queue_timeline_item(item);
    }

    fn apply_mcp_tool_progress(&self, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP tool is working.");
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: TimelineItemKind::McpTool,
                title: Some("MCP tool".to_owned()),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        item.detail = Some(message.to_owned());
        item.status = TimelineItemStatus::Running;
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_mcp_oauth(&self, params: &Value) {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("MCP server");
        let success = params.get("success").and_then(Value::as_bool) == Some(true);
        self.upsert_timeline(TimelineItem {
            id: format!("mcp-oauth:{name}"),
            kind: TimelineItemKind::McpTool,
            title: Some("MCP authentication".to_owned()),
            body: if success {
                format!("Authenticated {name}.")
            } else {
                format!("Could not authenticate {name}.")
            },
            detail: params
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: if success {
                TimelineItemStatus::Completed
            } else {
                TimelineItemStatus::Failed
            },
        });
    }

    fn apply_mcp_startup_status(&self, params: &Value) {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("MCP server");
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("starting");
        self.upsert_timeline(TimelineItem {
            id: format!("mcp-startup:{name}"),
            kind: TimelineItemKind::McpTool,
            title: Some(format!("MCP · {name}")),
            body: format!("Startup status: {}.", title_case(status)),
            detail: params
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| params.get("failureReason").map(compact_json)),
            status: event_status(status, status != "starting"),
        });
    }

    fn apply_account_update(&self, params: &Value) {
        let mut details = Vec::new();
        if let Some(mode) = params.get("authMode").and_then(Value::as_str) {
            details.push(format!("Authentication: {}", title_case(mode)));
        }
        if let Some(plan) = params.get("planType").and_then(Value::as_str) {
            details.push(format!("Plan: {}", title_case(plan)));
        }
        self.upsert_timeline(TimelineItem {
            id: "account-status".to_owned(),
            kind: TimelineItemKind::Tool,
            title: Some("Codex account".to_owned()),
            body: if details.is_empty() {
                "Account state changed.".to_owned()
            } else {
                details.join("\n")
            },
            detail: None,
            status: TimelineItemStatus::Completed,
        });
    }

    fn apply_account_login(&self, params: &Value) {
        let success = params.get("success").and_then(Value::as_bool) == Some(true);
        self.upsert_timeline(TimelineItem {
            id: "account-login".to_owned(),
            kind: TimelineItemKind::Tool,
            title: Some("Codex sign in".to_owned()),
            body: if success {
                "Sign in completed.".to_owned()
            } else {
                "Sign in failed.".to_owned()
            },
            detail: params
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: if success {
                TimelineItemStatus::Completed
            } else {
                TimelineItemStatus::Failed
            },
        });
    }

    fn apply_rate_limits(&self, params: &Value) {
        let limits = params.get("rateLimits").unwrap_or(params);
        let reached = limits
            .get("spendControlReached")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || limits
                .get("rateLimitReachedType")
                .is_some_and(|value| !value.is_null());
        let name = limits
            .get("limitName")
            .and_then(Value::as_str)
            .unwrap_or("Codex usage");
        let mut body = if reached {
            format!("{name} limit reached.")
        } else {
            format!("{name} limits updated.")
        };
        if let Some(remaining) = limits
            .pointer("/primary/remainingPercent")
            .and_then(Value::as_f64)
        {
            body.push_str(&format!("\n{remaining:.0}% remaining."));
        }
        self.upsert_timeline(TimelineItem {
            id: "account-rate-limits".to_owned(),
            kind: if reached {
                TimelineItemKind::Warning
            } else {
                TimelineItemKind::Tool
            },
            title: Some("Account limits".to_owned()),
            body,
            detail: None,
            status: if reached {
                TimelineItemStatus::Interrupted
            } else {
                TimelineItemStatus::Completed
            },
        });
    }

    fn apply_catalog_change(&self, catalog: &str, params: &Value) {
        let count = params.get("data").and_then(Value::as_array).map(Vec::len);
        self.upsert_timeline(TimelineItem {
            id: format!("catalog-changed:{catalog}"),
            kind: TimelineItemKind::Tool,
            title: Some(format!("{} changed", title_case(catalog))),
            body: count.map_or_else(
                || format!("The available {catalog} changed."),
                |count| format!("{count} {catalog} available."),
            ),
            detail: None,
            status: TimelineItemStatus::Completed,
        });
    }

    fn apply_reasoning_summary_part(&self, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let summary_index = params
            .get("summaryIndex")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: TimelineItemKind::Reasoning,
                title: Some("Reasoning".to_owned()),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        item.detail = Some(format!(
            "Reasoning summary part {} started.",
            summary_index + 1
        ));
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_remote_control_status(&self, params: &Value) {
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("disabled");
        let server_name = params
            .get("serverName")
            .and_then(Value::as_str)
            .unwrap_or("Codex remote control");
        let mut details = Vec::new();
        if let Some(environment) = params.get("environmentId").and_then(Value::as_str) {
            details.push(format!("Environment: {environment}"));
        }
        if let Some(installation) = params.get("installationId").and_then(Value::as_str) {
            details.push(format!("Installation: {installation}"));
        }
        self.upsert_timeline(TimelineItem {
            id: "remote-control-status".to_owned(),
            kind: if status == "errored" {
                TimelineItemKind::Warning
            } else {
                TimelineItemKind::Tool
            },
            title: Some("Remote control".to_owned()),
            body: format!("{server_name}: {}.", title_case(status)),
            detail: (!details.is_empty()).then(|| details.join("\n")),
            status: event_status(status, status != "connecting"),
        });
    }

    fn apply_external_import(&self, method: &str, params: &Value) {
        let import_id = params
            .get("importId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let completed = method.ends_with("/completed");
        let results = params
            .get("itemTypeResults")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let successes = results
            .iter()
            .filter_map(|result| result.get("successes").and_then(Value::as_array))
            .map(Vec::len)
            .sum::<usize>();
        let failures = results
            .iter()
            .filter_map(|result| result.get("failures").and_then(Value::as_array))
            .flatten()
            .collect::<Vec<_>>();
        let detail = failures
            .iter()
            .filter_map(|failure| failure.get("message").and_then(Value::as_str))
            .take(12)
            .collect::<Vec<_>>()
            .join("\n");
        self.upsert_timeline(TimelineItem {
            id: format!("external-import:{import_id}"),
            kind: if failures.is_empty() {
                TimelineItemKind::Tool
            } else {
                TimelineItemKind::Warning
            },
            title: Some("Agent configuration import".to_owned()),
            body: if completed {
                format!(
                    "Import completed: {successes} succeeded, {} failed.",
                    failures.len()
                )
            } else {
                format!("Importing agent configuration: {successes} succeeded so far.")
            },
            detail: (!detail.is_empty()).then_some(detail),
            status: if completed && !failures.is_empty() {
                TimelineItemStatus::Failed
            } else if completed {
                TimelineItemStatus::Completed
            } else {
                TimelineItemStatus::Running
            },
        });
    }

    fn apply_fs_changed(&self, params: &Value) {
        let watch_id = params
            .get("watchId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let paths = params
            .get("changedPaths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        self.upsert_timeline(TimelineItem {
            id: format!("fs-watch:{watch_id}"),
            kind: TimelineItemKind::FileChange,
            title: Some("Workspace files changed".to_owned()),
            body: match paths.len() {
                0 => "A watched workspace path changed.".to_owned(),
                1 => format!("{} changed.", paths[0]),
                count => format!("{count} watched paths changed."),
            },
            detail: (paths.len() > 1).then(|| paths.join("\n")),
            status: TimelineItemStatus::Completed,
        });
    }

    fn apply_fuzzy_search_updated(&self, params: &Value) {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let files = params
            .get("files")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut paths = files
            .iter()
            .filter_map(|file| file.get("path").and_then(Value::as_str))
            .take(20)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if files.len() > paths.len() {
            paths.push(format!("… and {} more", files.len() - paths.len()));
        }
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.upsert_timeline(TimelineItem {
            id: format!("fuzzy-search:{session}"),
            kind: TimelineItemKind::Tool,
            title: Some("File search".to_owned()),
            body: if query.is_empty() {
                format!("Found {} workspace files.", files.len())
            } else {
                format!("Found {} files matching “{query}”.", files.len())
            },
            detail: (!paths.is_empty()).then(|| paths.join("\n")),
            status: TimelineItemStatus::Running,
        });
    }

    fn apply_realtime_notification(&self, method: &str, params: &Value) {
        match method {
            "thread/realtime/started" => self.upsert_timeline(TimelineItem {
                id: "realtime-session".to_owned(),
                kind: TimelineItemKind::Tool,
                title: Some("Realtime session".to_owned()),
                body: format!(
                    "Realtime conversation {} started.",
                    params
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_uppercase)
                        .unwrap_or_else(|| "session".to_owned())
                ),
                detail: params
                    .get("realtimeSessionId")
                    .and_then(Value::as_str)
                    .map(|id| format!("Session: {id}")),
                status: TimelineItemStatus::Running,
            }),
            "thread/realtime/itemAdded" => {
                let item = params.get("item").unwrap_or(params);
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.next_id("realtime-item"));
                self.upsert_timeline(TimelineItem {
                    id,
                    kind: TimelineItemKind::Tool,
                    title: Some("Realtime event".to_owned()),
                    body: item
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("Realtime conversation item received.")
                        .to_owned(),
                    detail: Some(compact_json(item)),
                    status: TimelineItemStatus::Completed,
                });
            }
            "thread/realtime/transcript/delta" | "thread/realtime/transcript/done" => {
                self.apply_realtime_transcript(method, params)
            }
            "thread/realtime/outputAudio/delta" => {
                let bytes = params
                    .pointer("/audio/data")
                    .and_then(Value::as_str)
                    .and_then(|encoded| BASE64.decode(encoded).ok())
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                self.upsert_timeline(TimelineItem {
                    id: "realtime-audio".to_owned(),
                    kind: TimelineItemKind::Tool,
                    title: Some("Realtime audio".to_owned()),
                    body: format!("Received {bytes} bytes of realtime audio."),
                    detail: None,
                    status: TimelineItemStatus::Running,
                });
            }
            "thread/realtime/sdp" => self.upsert_timeline(TimelineItem {
                id: "realtime-transport".to_owned(),
                kind: TimelineItemKind::Tool,
                title: Some("Realtime transport".to_owned()),
                body: "WebRTC negotiation completed.".to_owned(),
                detail: None,
                status: TimelineItemStatus::Completed,
            }),
            "thread/realtime/error" => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("The realtime session failed.")
                    .to_owned();
                self.push_error(message.clone());
                self.upsert_timeline(TimelineItem {
                    id: "realtime-session".to_owned(),
                    kind: TimelineItemKind::Warning,
                    title: Some("Realtime session".to_owned()),
                    body: message,
                    detail: None,
                    status: TimelineItemStatus::Failed,
                });
            }
            "thread/realtime/closed" => self.upsert_timeline(TimelineItem {
                id: "realtime-session".to_owned(),
                kind: TimelineItemKind::Tool,
                title: Some("Realtime session".to_owned()),
                body: "Realtime conversation closed.".to_owned(),
                detail: params
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                status: TimelineItemStatus::Completed,
            }),
            _ => log::warn!(
                "unexpected realtime notification session_id={} method={method}",
                self.id
            ),
        }
    }

    fn apply_realtime_transcript(&self, method: &str, params: &Value) {
        let role = params
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("speaker");
        let completed = method.ends_with("/done");
        let text = params
            .get(if completed { "text" } else { "delta" })
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = format!("realtime-transcript:{role}");
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline.entry(id.clone()).or_insert_with(|| TimelineItem {
            id,
            kind: if role == "assistant" {
                TimelineItemKind::AssistantMessage
            } else {
                TimelineItemKind::UserMessage
            },
            title: Some(format!("Realtime {}", title_case(role))),
            body: String::new(),
            detail: None,
            status: TimelineItemStatus::Running,
        });
        if completed {
            item.body = text.to_owned();
            item.status = TimelineItemStatus::Completed;
        } else {
            item.body.push_str(text);
        }
        let item = item.clone();
        drop(timeline);
        if completed {
            self.view.upsert_timeline_item(item);
        } else {
            self.view.queue_timeline_item(item);
        }
    }

    fn apply_world_writable_warning(&self, params: &Value) {
        let paths = params
            .get("samplePaths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let extra = params
            .get("extraCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut message = if paths.is_empty() {
            "Windows sandbox protection found world-writable directories.".to_owned()
        } else {
            paths.join("\n")
        };
        if extra > 0 {
            message.push_str(&format!("\n… and {extra} more"));
        }
        if params.get("failedScan").and_then(Value::as_bool) == Some(true) {
            message.push_str("\nThe directory scan did not complete.");
        }
        self.push_warning("Windows sandbox warning", message);
    }

    fn apply_windows_sandbox_setup(&self, params: &Value) {
        let success = params.get("success").and_then(Value::as_bool) == Some(true);
        let mode = params
            .get("mode")
            .and_then(Value::as_str)
            .map(title_case)
            .unwrap_or_else(|| "Windows".to_owned());
        self.upsert_timeline(TimelineItem {
            id: "windows-sandbox-setup".to_owned(),
            kind: if success {
                TimelineItemKind::Tool
            } else {
                TimelineItemKind::Warning
            },
            title: Some("Windows sandbox setup".to_owned()),
            body: format!(
                "{mode} sandbox setup {}.",
                if success { "completed" } else { "failed" }
            ),
            detail: params
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: if success {
                TimelineItemStatus::Completed
            } else {
                TimelineItemStatus::Failed
            },
        });
    }

    fn apply_model_notification(&self, method: &str, params: &Value) {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let item = match method {
            "model/rerouted" => TimelineItem {
                id: format!("model-rerouted:{turn_id}"),
                kind: TimelineItemKind::Warning,
                title: Some("Model rerouted".to_owned()),
                body: format!(
                    "{} → {}",
                    params
                        .get("fromModel")
                        .and_then(Value::as_str)
                        .unwrap_or("Previous model"),
                    params
                        .get("toModel")
                        .and_then(Value::as_str)
                        .unwrap_or("Fallback model")
                ),
                detail: params.get("reason").and_then(Value::as_str).map(title_case),
                status: TimelineItemStatus::Completed,
            },
            "model/verification" => {
                let verifications = params
                    .get("verifications")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(title_case)
                    .collect::<Vec<_>>();
                TimelineItem {
                    id: format!("model-verification:{turn_id}"),
                    kind: TimelineItemKind::Tool,
                    title: Some("Model verification".to_owned()),
                    body: if verifications.is_empty() {
                        "Model verification updated.".to_owned()
                    } else {
                        verifications.join("\n")
                    },
                    detail: None,
                    status: TimelineItemStatus::Completed,
                }
            }
            _ => {
                let model = params
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("model");
                let buffering = params
                    .get("showBufferingUi")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut detail = params
                    .get("reasons")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if let Some(faster) = params.get("fasterModel").and_then(Value::as_str) {
                    detail.push(format!("Faster model: {faster}"));
                }
                TimelineItem {
                    id: format!("model-safety-buffering:{turn_id}"),
                    kind: if buffering {
                        TimelineItemKind::Warning
                    } else {
                        TimelineItemKind::Tool
                    },
                    title: Some("Model safety buffering".to_owned()),
                    body: format!(
                        "Safety buffering {} for {model}.",
                        if buffering { "enabled" } else { "disabled" }
                    ),
                    detail: (!detail.is_empty()).then(|| detail.join("\n")),
                    status: if buffering {
                        TimelineItemStatus::Running
                    } else {
                        TimelineItemStatus::Completed
                    },
                }
            }
        };
        self.upsert_timeline(item);
    }

    fn apply_moderation_metadata(&self, params: &Value) {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let metadata = params.get("metadata").unwrap_or(params);
        let body = metadata
            .get("message")
            .or_else(|| metadata.get("summary"))
            .or_else(|| metadata.get("reason"))
            .and_then(Value::as_str)
            .unwrap_or("Moderation metadata updated.")
            .to_owned();
        self.upsert_timeline(TimelineItem {
            id: format!("moderation:{turn_id}"),
            kind: TimelineItemKind::Warning,
            title: Some("Moderation".to_owned()),
            body,
            detail: Some(compact_json(metadata)),
            status: TimelineItemStatus::Completed,
        });
    }

    fn append_process_output(&self, method: &str, params: &Value) {
        let handle = if method == "command/exec/outputDelta" {
            params.get("processId")
        } else {
            params.get("processHandle")
        }
        .and_then(Value::as_str)
        .unwrap_or("process");
        let stream = params
            .get("stream")
            .and_then(Value::as_str)
            .unwrap_or("stdout");
        let output = params
            .get("deltaBase64")
            .and_then(Value::as_str)
            .and_then(|encoded| BASE64.decode(encoded).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let mut timeline = self.timeline.borrow_mut();
        let id = format!("process:{handle}");
        let item = timeline.entry(id.clone()).or_insert_with(|| TimelineItem {
            id,
            kind: TimelineItemKind::Command,
            title: Some("Background process".to_owned()),
            body: format!("Process {handle}"),
            detail: None,
            status: TimelineItemStatus::Running,
        });
        let detail = item.detail.get_or_insert_with(String::new);
        if stream == "stderr" && !output.is_empty() {
            if !detail.is_empty() && !detail.ends_with('\n') {
                detail.push('\n');
            }
            detail.push_str("[stderr] ");
        }
        detail.push_str(&output);
        if params.get("capReached").and_then(Value::as_bool) == Some(true) {
            if !detail.ends_with('\n') {
                detail.push('\n');
            }
            detail.push_str("… output limit reached …");
        }
        let item = item.clone();
        drop(timeline);
        self.view.queue_timeline_item(item);
    }

    fn apply_process_exit(&self, params: &Value) {
        let handle = params
            .get("processHandle")
            .and_then(Value::as_str)
            .unwrap_or("process");
        let exit_code = params.get("exitCode").and_then(Value::as_i64).unwrap_or(-1);
        let mut timeline = self.timeline.borrow_mut();
        let id = format!("process:{handle}");
        let item = timeline.entry(id.clone()).or_insert_with(|| TimelineItem {
            id,
            kind: TimelineItemKind::Command,
            title: Some("Background process".to_owned()),
            body: String::new(),
            detail: None,
            status: TimelineItemStatus::Running,
        });
        item.body = format!("Process {handle} exited with status {exit_code}.");
        let detail = item.detail.get_or_insert_with(String::new);
        for (label, value) in [("stdout", "stdout"), ("stderr", "stderr")] {
            let Some(output) = params.get(value).and_then(Value::as_str) else {
                continue;
            };
            if output.is_empty() {
                continue;
            }
            if !detail.is_empty() && !detail.ends_with('\n') {
                detail.push('\n');
            }
            if label == "stderr" {
                detail.push_str("[stderr] ");
            }
            detail.push_str(output);
        }
        item.status = if exit_code == 0 {
            TimelineItemStatus::Completed
        } else {
            TimelineItemStatus::Failed
        };
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_fuzzy_search_completed(&self, params: &Value) {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        self.upsert_timeline(TimelineItem {
            id: format!("fuzzy-search:{session}"),
            kind: TimelineItemKind::Tool,
            title: Some("File search".to_owned()),
            body: "Workspace file search completed.".to_owned(),
            detail: None,
            status: TimelineItemStatus::Completed,
        });
    }

    fn append_delta(&self, method: &str, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let delta = params
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if delta.is_empty() {
            return;
        }
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: delta_kind(method),
                title: delta_title(method),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
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
        let item = item.clone();
        drop(timeline);
        self.view.queue_timeline_item(item);
    }

    fn apply_patch_snapshot(&self, params: &Value) {
        let Some(item_id) = params.get("itemId").and_then(Value::as_str) else {
            return;
        };
        let detail = params
            .get("patch")
            .or_else(|| params.get("changes"))
            .map(compact_json)
            .unwrap_or_default();
        let mut timeline = self.timeline.borrow_mut();
        let item = timeline
            .entry(item_id.to_owned())
            .or_insert_with(|| TimelineItem {
                id: item_id.to_owned(),
                kind: TimelineItemKind::FileChange,
                title: Some("File changes".to_owned()),
                body: String::new(),
                detail: None,
                status: TimelineItemStatus::Running,
            });
        item.detail = Some(detail);
        let item = item.clone();
        drop(timeline);
        self.view.upsert_timeline_item(item);
    }

    fn apply_plan(&self, params: &Value) {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or("current");
        let steps = params
            .get("plan")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|step| {
                        let marker = match step.get("status").and_then(Value::as_str) {
                            Some("completed") => "✓",
                            Some("inProgress") => "→",
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
        self.upsert_timeline(TimelineItem {
            id: format!("turn-plan:{turn_id}"),
            kind: TimelineItemKind::Plan,
            title: Some("Plan".to_owned()),
            body: steps,
            detail: params
                .get("explanation")
                .and_then(Value::as_str)
                .map(str::to_owned),
            status: TimelineItemStatus::Running,
        });
        let progress = params
            .get("plan")
            .and_then(Value::as_array)
            .map(|steps| PlanProgress {
                title: Some("Plan".to_owned()),
                summary: params
                    .get("explanation")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                steps: steps
                    .iter()
                    .enumerate()
                    .map(|(index, step)| PlanStep {
                        id: format!("{turn_id}:{index}"),
                        label: step
                            .get("step")
                            .and_then(Value::as_str)
                            .unwrap_or("Unnamed step")
                            .to_owned(),
                        detail: None,
                        status: match step.get("status").and_then(Value::as_str) {
                            Some("inProgress") => PlanStepStatus::InProgress,
                            Some("completed") => PlanStepStatus::Completed,
                            Some("failed") => PlanStepStatus::Failed,
                            _ => PlanStepStatus::Pending,
                        },
                    })
                    .collect(),
            });
        self.view.set_plan_progress(progress);
    }

    fn apply_token_usage(&self, params: &Value) {
        let Some(total) = params.pointer("/tokenUsage/total") else {
            return;
        };
        let last = params.pointer("/tokenUsage/last").unwrap_or(total);
        self.view.set_usage(Some(TokenUsage {
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
                .or(self.context_window_fallback.get()),
        }));
    }

    fn update_collaboration_progress(&self, item: &Value, completed: bool) {
        if item.get("type").and_then(Value::as_str) != Some("collabAgentToolCall") {
            return;
        }
        let Some(call_id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let receivers = item
            .get("receiverThreadIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let agents_states = item.get("agentsStates").and_then(Value::as_object);
        for receiver in receivers.iter().copied().chain(
            agents_states
                .into_iter()
                .flat_map(|states| states.keys().map(String::as_str))
                .filter(|receiver| !receivers.contains(receiver)),
        ) {
            let state = agents_states.and_then(|states| states.get(receiver));
            let status_name = state
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str)
                .or_else(|| item.get("status").and_then(Value::as_str));
            let status = match status_name {
                Some("errored" | "failed" | "notFound") => CollaborationParticipantStatus::Failed,
                Some("completed" | "shutdown") if completed => {
                    CollaborationParticipantStatus::Completed
                }
                Some("running" | "inProgress") => CollaborationParticipantStatus::Working,
                _ if completed => CollaborationParticipantStatus::Completed,
                _ => CollaborationParticipantStatus::Pending,
            };
            let id = format!("{call_id}:{receiver}");
            let detail = state
                .and_then(|state| state.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    item.get("prompt")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| Some(format!("Thread {receiver}")));
            self.collaboration.borrow_mut().insert(
                id.clone(),
                CollaborationParticipant {
                    id,
                    label: item
                        .get("tool")
                        .and_then(Value::as_str)
                        .map(title_case)
                        .unwrap_or_else(|| "Subagent".to_owned()),
                    detail,
                    status,
                },
            );
        }
        let mut participants = self
            .collaboration
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        participants.sort_by(|left, right| left.id.cmp(&right.id));
        self.view
            .set_collaboration_progress(Some(CollaborationProgress {
                title: Some("Collaboration".to_owned()),
                participants,
            }));
    }

    fn notification_is_current_thread(&self, params: &Value) -> bool {
        let Some(notified_thread_id) = params.get("threadId").and_then(Value::as_str) else {
            return false;
        };
        self.thread_id.borrow().as_deref() == Some(notified_thread_id)
    }

    pub(super) fn targets_other_thread(&self, params: &Value) -> bool {
        let Some(notified_thread_id) = params.get("threadId").and_then(Value::as_str) else {
            return false;
        };
        match self.thread_id.borrow().as_deref() {
            Some(thread_id) => thread_id != notified_thread_id,
            None => *self.lifecycle.borrow() == AppChatState::StartingThread,
        }
    }
}

fn event_status(status: &str, completed: bool) -> TimelineItemStatus {
    match status {
        "failed" | "denied" => TimelineItemStatus::Failed,
        "cancelled" | "canceled" | "aborted" | "timedOut" => TimelineItemStatus::Interrupted,
        "completed" | "ready" | "approved" | "success" => TimelineItemStatus::Completed,
        _ if completed => TimelineItemStatus::Completed,
        _ => TimelineItemStatus::Running,
    }
}

pub(super) fn timeline_from_item(item: &Value, completed: bool) -> TimelineItem {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let id = (item_type == "userMessage")
        .then(|| item.get("clientId").and_then(Value::as_str))
        .flatten()
        .or_else(|| item.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("unknown:{:016x}", stable_hash(&item.to_string())));
    let (kind, title, body, detail) = match item_type {
        "userMessage" => (
            TimelineItemKind::UserMessage,
            None,
            user_message_text(item.get("content")),
            None,
        ),
        "agentMessage" => (
            TimelineItemKind::AssistantMessage,
            None,
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "plan" => (
            TimelineItemKind::Plan,
            Some("Plan".to_owned()),
            item.get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "reasoning" => (
            TimelineItemKind::Reasoning,
            Some("Reasoning".to_owned()),
            flattened_text(item.get("summary")),
            nonempty(flattened_text(item.get("content"))),
        ),
        "hookPrompt" => (
            TimelineItemKind::DeveloperMessage,
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
            TimelineItemKind::Command,
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
            TimelineItemKind::FileChange,
            Some("File changes".to_owned()),
            file_change_summary(item),
            item.get("changes").map(compact_json),
        ),
        "mcpToolCall" => (
            TimelineItemKind::McpTool,
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
            TimelineItemKind::Tool,
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
        "collabAgentToolCall" => (
            TimelineItemKind::Collaboration,
            Some("Collaboration".to_owned()),
            item.get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    item.get("tool")
                        .and_then(Value::as_str)
                        .map(title_case)
                        .unwrap_or_else(|| "Agent activity".to_owned())
                }),
            Some(compact_json(item)),
        ),
        "subAgentActivity" => (
            TimelineItemKind::Collaboration,
            Some(format!(
                "Subagent {}",
                item.get("kind")
                    .and_then(Value::as_str)
                    .map(title_case)
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
        "webSearch" => (
            TimelineItemKind::Web,
            Some("Web search".to_owned()),
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            item.get("results").map(compact_json),
        ),
        "imageView" => (
            TimelineItemKind::Image,
            Some("Image".to_owned()),
            item.get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "imageGeneration" => (
            TimelineItemKind::Image,
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
        "enteredReviewMode" | "exitedReviewMode" => (
            TimelineItemKind::Review,
            Some("Review".to_owned()),
            item.get("review")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            None,
        ),
        "contextCompaction" => (
            TimelineItemKind::Compaction,
            Some("Context compaction".to_owned()),
            "Codex compacted the conversation context.".to_owned(),
            None,
        ),
        "sleep" => (
            TimelineItemKind::Tool,
            Some("Waiting".to_owned()),
            item.get("durationMs")
                .and_then(Value::as_u64)
                .map(|duration| format!("Waiting for {duration} ms"))
                .unwrap_or_default(),
            None,
        ),
        other => (
            TimelineItemKind::Unknown(other.to_owned()),
            Some(title_case(other)),
            compact_json(item),
            None,
        ),
    };
    TimelineItem {
        id,
        kind,
        title,
        body,
        detail,
        status: timeline_status(item.get("status").and_then(Value::as_str), completed),
    }
}

fn timeline_status(status: Option<&str>, completed: bool) -> TimelineItemStatus {
    match status {
        Some("failed" | "declined") => TimelineItemStatus::Failed,
        Some("interrupted" | "cancelled") => TimelineItemStatus::Interrupted,
        Some("completed") => TimelineItemStatus::Completed,
        Some("inProgress" | "running") => TimelineItemStatus::Running,
        _ if completed => TimelineItemStatus::Completed,
        _ => TimelineItemStatus::Running,
    }
}

fn delta_kind(method: &str) -> TimelineItemKind {
    match method {
        "item/agentMessage/delta" => TimelineItemKind::AssistantMessage,
        "item/plan/delta" => TimelineItemKind::Plan,
        method if method.starts_with("item/reasoning/") => TimelineItemKind::Reasoning,
        "item/commandExecution/outputDelta" => TimelineItemKind::Command,
        "item/fileChange/outputDelta" => TimelineItemKind::FileChange,
        _ => TimelineItemKind::Unknown(method.to_owned()),
    }
}

fn delta_title(method: &str) -> Option<String> {
    match method {
        "item/plan/delta" => Some("Plan".to_owned()),
        method if method.starts_with("item/reasoning/") => Some("Reasoning".to_owned()),
        "item/commandExecution/outputDelta" => Some("Command".to_owned()),
        "item/fileChange/outputDelta" => Some("File changes".to_owned()),
        _ => None,
    }
}

fn request_id_key_value(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => number.as_i64().map(|id| format!("integer:{id}")),
        Value::String(id) => Some(format!("string:{id}")),
        _ => None,
    }
}

fn user_message_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|input| match input.get("type").and_then(Value::as_str) {
            Some("text") => input.get("text").and_then(Value::as_str).map(str::to_owned),
            Some("image") => Some("[Image]".to_owned()),
            Some("localImage") => Some(format!(
                "[Image: {}]",
                input.get("path").and_then(Value::as_str).unwrap_or("image")
            )),
            Some("audio") | Some("localAudio") => Some("[Audio]".to_owned()),
            Some("skill") | Some("mention") => Some(format!(
                "[{}]",
                input
                    .get("name")
                    .or_else(|| input.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or("Attachment")
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn flattened_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|value| match value {
                Value::String(value) => Some(value.clone()),
                Value::Object(object) => object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn file_change_summary(item: &Value) -> String {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            let path = change.get("path")?.as_str()?;
            let kind = change
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("update");
            Some(format!("{kind}: {path}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn nonempty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn stable_hash(value: &str) -> u64 {
    value
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

fn nonnegative_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or_default()
}
