use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use craic_codex_app_server::AppServerError;
use craic_codex_app_server::protocol::{
    AppsInstalledParams, AppsListParams, ExperimentalFeatureListParams, GetAccountParams,
    ListMcpServerStatusParams, McpServerStatusDetail, PluginInstalledParams, PluginListParams,
    RequestId, SkillsListParams, ThreadBackgroundTerminalsListParams, ThreadGoalClearParams,
    ThreadGoalGetParams, ThreadGoalSetParams, ThreadShellCommandParams,
};
use gtk::gio;
use serde_json::Value;

use super::super::codex_chat::{TimelineItem, TimelineItemKind, TimelineItemStatus};
use super::AppChatSessionInner;

pub(super) struct ToolRequest {
    title: String,
}

impl AppChatSessionInner {
    pub(super) fn prompt_thread_goal(self: &Rc<Self>) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading("Thread Goal")
            .body("View, replace, or clear the goal tracked by this Codex thread.")
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text("Goal objective")
            .activates_default(true)
            .hexpand(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("view", "View Current");
        dialog.add_response("clear", "Clear");
        dialog.add_response("save", "Set Goal");
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                let Some(session) = weak.upgrade() else {
                    return;
                };
                let request = {
                    let server = session.server.borrow();
                    let Some(server) = server.as_ref() else {
                        return;
                    };
                    match response.as_str() {
                        "view" => server.thread_goal_get(ThreadGoalGetParams {
                            thread_id: thread_id.clone(),
                        }),
                        "clear" => server.thread_goal_clear(ThreadGoalClearParams {
                            thread_id: thread_id.clone(),
                        }),
                        "save" => {
                            let objective = entry.text().trim().to_owned();
                            if objective.is_empty() {
                                return;
                            }
                            server.thread_goal_set(ThreadGoalSetParams {
                                thread_id: thread_id.clone(),
                                objective: Some(objective),
                                status: None,
                                token_budget: None,
                            })
                        }
                        _ => return,
                    }
                };
                session.track_tool_request("Thread goal", request);
            },
        );
    }

    pub(super) fn prompt_shell_command(self: &Rc<Self>) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading("Run Thread Shell Command")
            .body("This command runs through the thread's shell with full local access, outside the Codex sandbox.")
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text("Shell command")
            .activates_default(true)
            .hexpand(true)
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("run", "Run Command");
        dialog.set_response_appearance("run", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "run" {
                    return;
                }
                let command = entry.text().trim().to_owned();
                if command.is_empty() {
                    return;
                }
                let Some(session) = weak.upgrade() else {
                    return;
                };
                let request = session.server.borrow().as_ref().map(|server| {
                    server.thread_shell_command(ThreadShellCommandParams { thread_id, command })
                });
                if let Some(request) = request {
                    session.track_tool_request("Shell command", request);
                }
            },
        );
    }

    pub(super) fn load_background_terminals(&self) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return;
        };
        let request = self.server.borrow().as_ref().map(|server| {
            server.thread_background_terminals_list(ThreadBackgroundTerminalsListParams {
                thread_id,
                cursor: None,
                limit: Some(100),
            })
        });
        if let Some(request) = request {
            self.track_tool_request("Background terminals", request);
        }
    }

    pub(super) fn load_skills(&self) {
        let request = self.server.borrow().as_ref().map(|server| {
            server.skills_list(SkillsListParams {
                cwds: vec![PathBuf::from(&self.workspace_root)],
                force_reload: false,
            })
        });
        if let Some(request) = request {
            self.track_tool_request("Skills", request);
        }
    }

    pub(super) fn load_mcp_servers(&self) {
        let request = self.server.borrow().as_ref().map(|server| {
            server.mcp_server_status_list(ListMcpServerStatusParams {
                cursor: None,
                limit: Some(100),
                detail: Some(McpServerStatusDetail::Full),
                thread_id: self.thread_id.borrow().clone(),
            })
        });
        if let Some(request) = request {
            self.track_tool_request("MCP servers", request);
        }
    }

    pub(super) fn load_apps(&self) {
        let thread_id = self.thread_id.borrow().clone();
        let requests = {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return;
            };
            (
                server.apps_list(AppsListParams {
                    cursor: None,
                    limit: Some(100),
                    thread_id: thread_id.clone(),
                    force_refetch: false,
                }),
                server.apps_installed(AppsInstalledParams {
                    thread_id,
                    force_refresh: false,
                }),
            )
        };
        self.track_tool_request("Available apps & connectors", requests.0);
        self.track_tool_request("Installed apps & connectors", requests.1);
    }

    pub(super) fn load_plugins(&self) {
        let cwds = Some(vec![PathBuf::from(&self.workspace_root)]);
        let requests = {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return;
            };
            (
                server.plugin_list(PluginListParams {
                    cwds: cwds.clone(),
                    marketplace_kinds: None,
                    force_refetch: false,
                }),
                server.plugin_installed(PluginInstalledParams {
                    cwds,
                    install_suggestion_plugin_names: None,
                }),
            )
        };
        self.track_tool_request("Available plugins", requests.0);
        self.track_tool_request("Installed plugins", requests.1);
    }

    pub(super) fn load_experimental_features(&self) {
        let request = self.server.borrow().as_ref().map(|server| {
            server.experimental_feature_list(ExperimentalFeatureListParams {
                cursor: None,
                limit: Some(100),
                thread_id: self.thread_id.borrow().clone(),
            })
        });
        if let Some(request) = request {
            self.track_tool_request("Experimental features", request);
        }
    }

    pub(super) fn load_account_usage(&self) {
        let requests = {
            let server = self.server.borrow();
            let Some(server) = server.as_ref() else {
                return;
            };
            (
                server.account_read(GetAccountParams {
                    refresh_token: false,
                }),
                server.account_rate_limits_read(),
                server.account_usage_read(),
            )
        };
        self.track_tool_request("Account", requests.0);
        self.track_tool_request("Account rate limits", requests.1);
        self.track_tool_request("Account usage", requests.2);
    }

    fn track_tool_request(&self, title: &str, request: Result<RequestId, AppServerError>) {
        match request {
            Ok(request_id) => {
                self.tool_requests.borrow_mut().insert(
                    request_id,
                    ToolRequest {
                        title: title.to_owned(),
                    },
                );
            }
            Err(error) => self.push_error(format!("{title} failed: {error}")),
        }
    }

    pub(super) fn handle_tool_response(&self, request_id: &RequestId, result: &Value) -> bool {
        let Some(request) = self.tool_requests.borrow_mut().remove(request_id) else {
            return false;
        };
        self.upsert_timeline(TimelineItem {
            id: self.next_id("tool-result"),
            kind: TimelineItemKind::Tool,
            title: Some(request.title),
            body: tool_result_summary(result),
            detail: Some(super::compact_json(result)),
            status: TimelineItemStatus::Completed,
        });
        true
    }

    pub(super) fn handle_tool_error(&self, request_id: &RequestId, message: &str) -> bool {
        let Some(request) = self.tool_requests.borrow_mut().remove(request_id) else {
            return false;
        };
        self.push_error(format!("{} failed: {message}", request.title));
        true
    }
}

fn tool_result_summary(result: &Value) -> String {
    if let Some(items) = result.get("data").and_then(Value::as_array) {
        if items.is_empty() {
            return "No entries found.".to_owned();
        }
        return items
            .iter()
            .map(|item| {
                let label = ["displayName", "name", "title", "id", "server", "processId"]
                    .into_iter()
                    .find_map(|key| item.get(key).and_then(Value::as_str))
                    .unwrap_or("Entry");
                let state = ["status", "enabled", "description"]
                    .into_iter()
                    .find_map(|key| item.get(key))
                    .map(|value| match value {
                        Value::String(value) => value.clone(),
                        Value::Bool(value) => {
                            if *value { "enabled" } else { "disabled" }.to_owned()
                        }
                        value => value.to_string(),
                    });
                state.map_or_else(|| label.to_owned(), |state| format!("{label} — {state}"))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(object) = result.as_object() {
        let lines = object
            .iter()
            .filter_map(|(key, value)| match value {
                Value::String(value) => Some(format!("{}: {value}", super::title_case(key))),
                Value::Number(value) => Some(format!("{}: {value}", super::title_case(key))),
                Value::Bool(value) => Some(format!("{}: {value}", super::title_case(key))),
                Value::Null => None,
                _ => None,
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    "Result received. Expand details to inspect it.".to_owned()
}
