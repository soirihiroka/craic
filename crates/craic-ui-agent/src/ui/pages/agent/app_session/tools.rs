use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use craic_codex_app_server::AppServerError;
use craic_codex_app_server::protocol::{
    AppsInstalledParams, AppsListParams, ExperimentalFeatureEnablementSetParams,
    ExperimentalFeatureListParams, GetAccountParams, ListMcpServerStatusParams,
    McpServerStatusDetail, PluginInstalledParams, PluginListParams, RequestId, SkillsListParams,
    ThreadBackgroundTerminalsCleanParams, ThreadBackgroundTerminalsListParams,
    ThreadBackgroundTerminalsTerminateParams, ThreadGoalClearParams, ThreadGoalGetParams,
    ThreadGoalSetParams, ThreadShellCommandParams,
};
use gtk::gio;
use serde_json::Value;

use super::super::codex_chat::{
    ComposerAttachment, ComposerAttachmentKind, TimelineItem, TimelineItemKind, TimelineItemStatus,
};
use super::AppChatSessionInner;

pub(super) struct ToolRequest {
    title: String,
    kind: ToolRequestKind,
}

enum ToolRequestKind {
    Timeline,
    SkillPicker,
    BackgroundTerminals {
        session: Weak<AppChatSessionInner>,
        thread_id: String,
    },
    ExperimentalFeatures {
        session: Weak<AppChatSessionInner>,
    },
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

    pub(super) fn load_background_terminals(self: &Rc<Self>) {
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            self.push_error("The Codex thread is not ready yet".to_owned());
            return;
        };
        let request = self.server.borrow().as_ref().map(|server| {
            server.thread_background_terminals_list(ThreadBackgroundTerminalsListParams {
                thread_id: thread_id.clone(),
                cursor: None,
                limit: Some(100),
            })
        });
        if let Some(request) = request {
            match request {
                Ok(request_id) => {
                    self.tool_requests.borrow_mut().insert(
                        request_id,
                        ToolRequest {
                            title: "Background terminals".to_owned(),
                            kind: ToolRequestKind::BackgroundTerminals {
                                session: Rc::downgrade(self),
                                thread_id,
                            },
                        },
                    );
                }
                Err(error) => self.push_error(format!("Background terminals failed: {error}")),
            }
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
            match request {
                Ok(request_id) => {
                    self.tool_requests.borrow_mut().insert(
                        request_id,
                        ToolRequest {
                            title: "Skills".to_owned(),
                            kind: ToolRequestKind::SkillPicker,
                        },
                    );
                }
                Err(error) => self.push_error(format!("Skills failed: {error}")),
            }
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

    pub(super) fn load_experimental_features(self: &Rc<Self>) {
        let request = self.server.borrow().as_ref().map(|server| {
            server.experimental_feature_list(ExperimentalFeatureListParams {
                cursor: None,
                limit: Some(100),
                thread_id: self.thread_id.borrow().clone(),
            })
        });
        if let Some(request) = request {
            match request {
                Ok(request_id) => {
                    self.tool_requests.borrow_mut().insert(
                        request_id,
                        ToolRequest {
                            title: "Experimental features".to_owned(),
                            kind: ToolRequestKind::ExperimentalFeatures {
                                session: Rc::downgrade(self),
                            },
                        },
                    );
                }
                Err(error) => self.push_error(format!("Experimental features failed: {error}")),
            }
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
                        kind: ToolRequestKind::Timeline,
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
        match request.kind {
            ToolRequestKind::SkillPicker => self.show_skill_picker(result),
            ToolRequestKind::BackgroundTerminals { session, thread_id } => {
                self.show_background_terminals(result, session, thread_id)
            }
            ToolRequestKind::ExperimentalFeatures { session } => {
                self.show_experimental_features(result, session)
            }
            ToolRequestKind::Timeline => self.upsert_timeline(TimelineItem {
                id: self.next_id("tool-result"),
                kind: TimelineItemKind::Tool,
                title: Some(request.title),
                body: tool_result_summary(result),
                detail: Some(super::compact_json(result)),
                status: TimelineItemStatus::Completed,
            }),
        }
        true
    }

    fn show_background_terminals(
        &self,
        result: &Value,
        session: Weak<AppChatSessionInner>,
        thread_id: String,
    ) {
        let terminals = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|terminal| {
                Some((
                    terminal.get("processId")?.as_str()?.to_owned(),
                    terminal
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("Background command")
                        .to_owned(),
                    terminal
                        .get("cwd")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    terminal.get("osPid").and_then(Value::as_u64),
                    terminal.get("cpuPercent").and_then(Value::as_f64),
                    terminal.get("rssKb").and_then(Value::as_u64),
                ))
            })
            .collect::<Vec<_>>();
        if terminals.is_empty() {
            self.push_warning(
                "Background terminals",
                "No background terminals are running for this thread.".to_owned(),
            );
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Background Terminals")
            .body("Inspect or stop commands that are still running for this thread.")
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .show_separators(true)
            .build();
        list.add_css_class("boxed-list");
        for (process_id, command, cwd, os_pid, cpu_percent, rss_kb) in terminals {
            let labels = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .hexpand(true)
                .build();
            labels.append(
                &gtk::Label::builder()
                    .label(&command)
                    .tooltip_text(&command)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build(),
            );
            let mut details = Vec::new();
            if !cwd.is_empty() {
                details.push(cwd);
            }
            if let Some(os_pid) = os_pid {
                details.push(format!("PID {os_pid}"));
            }
            if let Some(cpu_percent) = cpu_percent {
                details.push(format!("CPU {cpu_percent:.1}%"));
            }
            if let Some(rss_kb) = rss_kb {
                details.push(format!("RSS {rss_kb} KiB"));
            }
            if !details.is_empty() {
                labels.append(
                    &gtk::Label::builder()
                        .label(details.join(" · "))
                        .css_classes(["caption", "dim-label"])
                        .xalign(0.0)
                        .ellipsize(gtk::pango::EllipsizeMode::Middle)
                        .build(),
                );
            }
            let stop = gtk::Button::builder()
                .icon_name("media-playback-stop-symbolic")
                .tooltip_text("Stop background terminal")
                .valign(gtk::Align::Center)
                .build();
            stop.add_css_class("flat");
            stop.add_css_class("destructive-action");
            stop.update_property(&[gtk::accessible::Property::Label("Stop background terminal")]);
            stop.connect_clicked({
                let session = session.clone();
                let thread_id = thread_id.clone();
                move |button| {
                    let Some(session) = session.upgrade() else {
                        return;
                    };
                    button.set_sensitive(false);
                    let request = session.server.borrow().as_ref().map(|server| {
                        server.thread_background_terminals_terminate(
                            ThreadBackgroundTerminalsTerminateParams {
                                thread_id: thread_id.clone(),
                                process_id: process_id.clone(),
                            },
                        )
                    });
                    if let Some(request) = request {
                        session.track_tool_request("Stop background terminal", request);
                    }
                }
            });
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .margin_top(5)
                .margin_bottom(5)
                .margin_start(8)
                .margin_end(6)
                .build();
            content.append(&labels);
            content.append(&stop);
            list.append(
                &gtk::ListBoxRow::builder()
                    .activatable(false)
                    .selectable(false)
                    .child(&content)
                    .build(),
            );
        }
        dialog.set_extra_child(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .min_content_height(160)
                .max_content_height(420)
                .propagate_natural_height(true)
                .child(&list)
                .build(),
        ));
        dialog.add_response("close", "Close");
        dialog.add_response("stop-all", "Stop All");
        dialog.set_response_appearance("stop-all", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        let parent = self.root.root().and_downcast::<gtk::Window>();
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "stop-all" {
                    return;
                }
                let Some(session) = session.upgrade() else {
                    return;
                };
                let request = session.server.borrow().as_ref().map(|server| {
                    server.thread_background_terminals_clean(ThreadBackgroundTerminalsCleanParams {
                        thread_id,
                    })
                });
                if let Some(request) = request {
                    session.track_tool_request("Stop all background terminals", request);
                }
            },
        );
    }

    fn show_experimental_features(&self, result: &Value, session: Weak<AppChatSessionInner>) {
        let mut features = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|feature| {
                let name = feature.get("name")?.as_str()?.to_owned();
                let label = feature
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_owned();
                let description = feature
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| feature.get("announcement").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned();
                let enabled = feature.get("enabled")?.as_bool()?;
                Some((name, label, description, enabled))
            })
            .collect::<Vec<_>>();
        features.sort_by_key(|feature| feature.1.to_lowercase());
        if features.is_empty() {
            self.push_warning(
                "Experimental features",
                "No configurable experimental features are available.".to_owned(),
            );
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Experimental Features")
            .body("Changes apply to the current Codex App Server process.")
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .build();
        list.add_css_class("boxed-list");
        let mut switches = Vec::new();
        for (name, label, description, enabled) in features {
            let toggle = gtk::Switch::builder()
                .active(enabled)
                .valign(gtk::Align::Center)
                .build();
            let row = adw::ActionRow::builder()
                .title(&label)
                .subtitle(&description)
                .activatable_widget(&toggle)
                .build();
            row.add_suffix(&toggle);
            list.append(&row);
            switches.push((name, enabled, toggle));
        }
        dialog.set_extra_child(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .min_content_height(220)
                .max_content_height(460)
                .propagate_natural_height(true)
                .child(&list)
                .build(),
        ));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("apply", "Apply");
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");
        let parent = self.root.root().and_downcast::<gtk::Window>();
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "apply" {
                    return;
                }
                let enablement = switches
                    .iter()
                    .filter(|(_, initial, toggle)| toggle.is_active() != *initial)
                    .map(|(name, _, toggle)| (name.clone(), toggle.is_active()))
                    .collect::<BTreeMap<_, _>>();
                if enablement.is_empty() {
                    return;
                }
                let Some(session) = session.upgrade() else {
                    return;
                };
                let request = session.server.borrow().as_ref().map(|server| {
                    server.experimental_feature_enablement_set(
                        ExperimentalFeatureEnablementSetParams { enablement },
                    )
                });
                if let Some(request) = request {
                    session.track_tool_request("Update experimental features", request);
                }
            },
        );
    }

    fn show_skill_picker(&self, result: &Value) {
        let mut skills = result
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
                Some((
                    skill.get("name")?.as_str()?.to_owned(),
                    skill
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    skill.get("path")?.as_str()?.to_owned(),
                ))
            })
            .collect::<Vec<_>>();
        skills.sort_by_key(|skill| skill.0.to_lowercase());
        skills.dedup_by(|left, right| left.2 == right.2);
        if skills.is_empty() {
            self.push_warning(
                "Skills",
                "No enabled skills are available for this workspace.".to_owned(),
            );
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Add a Skill")
            .body("Select a Codex skill to include with the next message.")
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .build();
        list.add_css_class("boxed-list");
        for (name, description, path) in &skills {
            let labels = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .margin_top(8)
                .margin_bottom(8)
                .margin_start(10)
                .margin_end(10)
                .build();
            labels.append(
                &gtk::Label::builder()
                    .label(name)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .build(),
            );
            if !description.is_empty() {
                labels.append(
                    &gtk::Label::builder()
                        .label(description)
                        .xalign(0.0)
                        .wrap(true)
                        .wrap_mode(gtk::pango::WrapMode::WordChar)
                        .css_classes(["caption", "dim-label"])
                        .build(),
                );
            }
            let row = gtk::ListBoxRow::builder()
                .child(&labels)
                .tooltip_text(path)
                .build();
            list.append(&row);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(220)
            .max_content_height(420)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        dialog.set_extra_child(Some(&scroller));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("add", "Add Skill");
        dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dialog.set_response_enabled("add", false);
        dialog.set_default_response(Some("add"));
        dialog.set_close_response("cancel");
        list.connect_row_selected({
            let dialog = dialog.clone();
            move |_, row| dialog.set_response_enabled("add", row.is_some())
        });

        let parent = self.root.root().and_downcast::<gtk::Window>();
        let view = self.view.clone();
        dialog.choose(
            parent.as_ref(),
            None::<&gio::Cancellable>,
            move |response| {
                if response.as_str() != "add" {
                    return;
                }
                let Some(index) = list
                    .selected_row()
                    .and_then(|row| usize::try_from(row.index()).ok())
                else {
                    return;
                };
                let Some((name, _, path)) = skills.get(index) else {
                    return;
                };
                view.add_attachment(ComposerAttachment {
                    id: format!("skill:{path}"),
                    label: name.clone(),
                    kind: ComposerAttachmentKind::Skill,
                    reference: path.clone(),
                });
            },
        );
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
                Value::String(value) => Some(format!(
                    "{}: {value}",
                    craic_agent::display::title_case(key)
                )),
                Value::Number(value) => Some(format!(
                    "{}: {value}",
                    craic_agent::display::title_case(key)
                )),
                Value::Bool(value) => Some(format!(
                    "{}: {value}",
                    craic_agent::display::title_case(key)
                )),
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
