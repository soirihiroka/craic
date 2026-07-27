use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::super::codex_chat::{ChatSelector, CodexChatView, SelectorOption};
use super::{AppChatSessionInner, title_case};

pub(super) const DEFAULT_SERVICE_TIER_ID: &str = "__default__";

#[derive(Clone)]
pub(super) struct ModelServiceTiers {
    options: Vec<SelectorOption>,
    default: Option<String>,
}

impl AppChatSessionInner {
    pub(super) fn apply_model_catalog(&self, result: &Value) {
        let Some(models) = result.get("data").and_then(Value::as_array) else {
            return;
        };
        let mut options = Vec::new();
        let mut reasoning_by_model = HashMap::new();
        let mut service_tiers_by_model = HashMap::new();
        let mut personality_by_model = HashMap::new();
        let mut catalog_default = None;
        for model in models {
            let Some(id) = model
                .get("model")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let label = model
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id);
            options.push(SelectorOption {
                id: id.to_owned(),
                label: label.to_owned(),
            });
            let reasoning = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| {
                    let id = effort.get("reasoningEffort")?.as_str()?;
                    Some(SelectorOption {
                        id: id.to_owned(),
                        label: title_case(id),
                    })
                })
                .collect::<Vec<_>>();
            reasoning_by_model.insert(id.to_owned(), reasoning);
            if let Some(supports_personality) =
                model.get("supportsPersonality").and_then(Value::as_bool)
            {
                personality_by_model.insert(id.to_owned(), supports_personality);
            }
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
                        Some(SelectorOption {
                            id: tier.get("id")?.as_str()?.to_owned(),
                            label: tier
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_else(|| tier.get("id").and_then(Value::as_str).unwrap())
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
            if model.get("isDefault").and_then(Value::as_bool) == Some(true) {
                catalog_default = Some(id.to_owned());
            }
        }
        self.model_reasoning.replace(reasoning_by_model);
        self.model_service_tiers.replace(service_tiers_by_model);
        self.model_supports_personality
            .replace(personality_by_model);
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Model)
            .cloned()
            .or(catalog_default);
        if let Some(selected) = selected.as_ref() {
            self.selected_values
                .borrow_mut()
                .insert(ChatSelector::Model, selected.clone());
        }
        self.view
            .set_selector_options(ChatSelector::Model, &options, selected.as_deref());
        self.update_reasoning_options();
        self.update_service_tier_options();
        self.update_personality_options();
    }

    pub(super) fn apply_config_defaults(&self, result: &Value) {
        let config = result.get("config").unwrap_or(result);
        self.context_window_fallback.set(
            config
                .get("model_context_window")
                .or_else(|| config.get("modelContextWindow"))
                .and_then(Value::as_i64)
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value > 0),
        );
        let defaults = [
            (ChatSelector::Model, &["model"][..]),
            (
                ChatSelector::Reasoning,
                &["model_reasoning_effort", "modelReasoningEffort"][..],
            ),
            (
                ChatSelector::ReasoningSummary,
                &["model_reasoning_summary", "modelReasoningSummary"][..],
            ),
            (ChatSelector::Personality, &["personality"][..]),
            (
                ChatSelector::Permissions,
                &["permissions", "default_permissions", "defaultPermissions"][..],
            ),
            (
                ChatSelector::ServiceTier,
                &["service_tier", "serviceTier"][..],
            ),
            (
                ChatSelector::ApprovalReviewer,
                &["approvals_reviewer", "approvalsReviewer"][..],
            ),
        ];
        for (selector, keys) in defaults {
            if let Some(value) = keys
                .iter()
                .find_map(|key| config.get(key).and_then(Value::as_str))
            {
                self.selected_values
                    .borrow_mut()
                    .insert(selector, value.to_owned());
            }
        }
        self.update_reasoning_options();
        self.update_service_tier_options();
        self.update_personality_options();
        for selector in [
            ChatSelector::ReasoningSummary,
            ChatSelector::ApprovalReviewer,
        ] {
            if let Some(selected) = self.selected_values.borrow().get(&selector).cloned() {
                let option_ids: &[&str] = match selector {
                    ChatSelector::ReasoningSummary => &["auto", "concise", "detailed", "none"][..],
                    ChatSelector::ApprovalReviewer => {
                        &["user", "auto_review", "guardian_subagent"][..]
                    }
                    _ => unreachable!(),
                };
                let options = option_ids
                    .iter()
                    .map(|id| SelectorOption {
                        id: (*id).to_owned(),
                        label: title_case(id),
                    })
                    .collect::<Vec<_>>();
                self.view
                    .set_selector_options(selector, &options, Some(&selected));
            }
        }
    }

    pub(super) fn apply_permission_profiles(&self, result: &Value) {
        let options = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|profile| profile.get("allowed").and_then(Value::as_bool) != Some(false))
            .filter_map(|profile| {
                let id = profile.get("id")?.as_str()?;
                Some(SelectorOption {
                    id: id.to_owned(),
                    label: permission_label(id),
                })
            })
            .collect::<Vec<_>>();
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Permissions)
            .cloned()
            .or_else(|| options.first().map(|option| option.id.clone()));
        if let Some(selected) = selected.as_ref() {
            self.selected_values
                .borrow_mut()
                .insert(ChatSelector::Permissions, selected.clone());
        }
        self.view
            .set_selector_options(ChatSelector::Permissions, &options, selected.as_deref());
    }

    pub(super) fn apply_collaboration_modes(&self, result: &Value) {
        let mut modes = HashMap::new();
        let options = result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|mode| {
                let name = mode.get("name")?.as_str()?.to_owned();
                modes.insert(name.clone(), mode.clone());
                Some(SelectorOption {
                    id: name.clone(),
                    label: name,
                })
            })
            .collect::<Vec<_>>();
        self.collaboration_modes.replace(modes);
        let selected = options
            .iter()
            .find(|option| option.id.eq_ignore_ascii_case("default"))
            .or_else(|| options.first())
            .map(|option| option.id.clone());
        if let Some(selected) = selected.as_ref() {
            self.selected_values
                .borrow_mut()
                .entry(ChatSelector::Collaboration)
                .or_insert_with(|| selected.clone());
        }
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Collaboration)
            .cloned();
        self.view
            .set_selector_options(ChatSelector::Collaboration, &options, selected.as_deref());
    }

    pub(super) fn apply_thread_settings(&self, params: &Value) {
        let settings = params.get("threadSettings").unwrap_or(params);
        for (selector, key) in [
            (ChatSelector::Model, "model"),
            (ChatSelector::Reasoning, "effort"),
            (ChatSelector::ReasoningSummary, "summary"),
            (ChatSelector::Personality, "personality"),
            (ChatSelector::ServiceTier, "serviceTier"),
            (ChatSelector::ApprovalReviewer, "approvalsReviewer"),
        ] {
            if let Some(value) = settings.get(key).and_then(Value::as_str) {
                self.selected_values
                    .borrow_mut()
                    .insert(selector, value.to_owned());
            }
        }
        if settings.get("serviceTier").is_some_and(Value::is_null) {
            self.selected_values.borrow_mut().insert(
                ChatSelector::ServiceTier,
                DEFAULT_SERVICE_TIER_ID.to_owned(),
            );
        }
        if let Some(value) = settings
            .pointer("/activePermissionProfile/id")
            .and_then(Value::as_str)
        {
            self.selected_values
                .borrow_mut()
                .insert(ChatSelector::Permissions, value.to_owned());
        }
        self.update_reasoning_options();
        self.update_service_tier_options();
        self.update_personality_options();
        for (selector, options) in [
            (
                ChatSelector::ReasoningSummary,
                &["auto", "concise", "detailed", "none"][..],
            ),
            (
                ChatSelector::ApprovalReviewer,
                &["user", "auto_review", "guardian_subagent"][..],
            ),
        ] {
            let options = options
                .iter()
                .map(|id| SelectorOption {
                    id: (*id).to_owned(),
                    label: title_case(id),
                })
                .collect::<Vec<_>>();
            let selected = self.selected_values.borrow().get(&selector).cloned();
            self.view
                .set_selector_options(selector, &options, selected.as_deref());
        }
    }

    pub(super) fn update_reasoning_options(&self) {
        let selected_model = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Model)
            .cloned();
        let options = selected_model
            .as_ref()
            .and_then(|model| self.model_reasoning.borrow().get(model).cloned())
            .unwrap_or_else(|| {
                ["low", "medium", "high", "xhigh", "max", "ultra"]
                    .into_iter()
                    .map(|id| SelectorOption {
                        id: id.to_owned(),
                        label: title_case(id),
                    })
                    .collect()
            });
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Reasoning)
            .cloned()
            .filter(|selected| options.iter().any(|option| option.id == *selected))
            .or_else(|| options.first().map(|option| option.id.clone()));
        if let Some(selected) = selected.as_ref() {
            self.selected_values
                .borrow_mut()
                .insert(ChatSelector::Reasoning, selected.clone());
        }
        self.view
            .set_selector_options(ChatSelector::Reasoning, &options, selected.as_deref());
    }

    pub(super) fn update_service_tier_options(&self) {
        let selected_model = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Model)
            .cloned();
        let tiers = selected_model
            .as_ref()
            .and_then(|model| self.model_service_tiers.borrow().get(model).cloned());
        let options = tiers
            .as_ref()
            .map(|tiers| tiers.options.clone())
            .unwrap_or_else(|| {
                vec![SelectorOption {
                    id: DEFAULT_SERVICE_TIER_ID.to_owned(),
                    label: "Standard".to_owned(),
                }]
            });
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::ServiceTier)
            .cloned()
            .filter(|selected| options.iter().any(|option| option.id == *selected))
            .or_else(|| tiers.as_ref().and_then(|tiers| tiers.default.clone()))
            .filter(|selected| options.iter().any(|option| option.id == *selected))
            .unwrap_or_else(|| DEFAULT_SERVICE_TIER_ID.to_owned());
        self.selected_values
            .borrow_mut()
            .insert(ChatSelector::ServiceTier, selected.clone());
        self.view.set_selector_options(
            ChatSelector::ServiceTier,
            &options,
            Some(selected.as_str()),
        );
    }

    pub(super) fn update_personality_options(&self) {
        let selected_model = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Model)
            .cloned();
        let supported = selected_model
            .as_ref()
            .and_then(|model| self.model_supports_personality.borrow().get(model).copied())
            .unwrap_or(true);
        let options = supported
            .then(|| {
                ["friendly", "pragmatic", "none"]
                    .into_iter()
                    .map(|id| SelectorOption {
                        id: id.to_owned(),
                        label: title_case(id),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let selected = self
            .selected_values
            .borrow()
            .get(&ChatSelector::Personality)
            .cloned();
        self.view
            .set_selector_options(ChatSelector::Personality, &options, selected.as_deref());
    }

    pub(super) fn update_selector(&self, selector: ChatSelector, value: Option<String>) {
        self.dirty_selectors.borrow_mut().insert(selector);
        let Some(value) = value else {
            self.selected_values.borrow_mut().remove(&selector);
            return;
        };
        self.selected_values
            .borrow_mut()
            .insert(selector, value.clone());
        if selector == ChatSelector::Model {
            self.view.set_usage(None);
            self.context_window_fallback.set(None);
            self.update_reasoning_options();
            self.update_service_tier_options();
            self.update_personality_options();
        }
        let Some(thread_id) = self.thread_id.borrow().clone() else {
            return;
        };
        let field = match selector {
            ChatSelector::Model => json!({ "model": value }),
            ChatSelector::Reasoning => json!({ "effort": value }),
            ChatSelector::ReasoningSummary => json!({ "summary": value }),
            ChatSelector::Personality => json!({ "personality": value }),
            ChatSelector::Permissions => json!({ "permissions": value }),
            ChatSelector::ServiceTier => json!({
                "serviceTier": (value != DEFAULT_SERVICE_TIER_ID).then_some(value)
            }),
            ChatSelector::ApprovalReviewer => json!({ "approvalsReviewer": value }),
            ChatSelector::Collaboration => {
                let Some(mode) = self.collaboration_mode(&value) else {
                    return;
                };
                json!({ "collaborationMode": mode })
            }
        };
        let mut params = field.as_object().cloned().unwrap_or_default();
        params.insert("threadId".to_owned(), Value::String(thread_id));
        if let Some(server) = self.server.borrow().as_ref()
            && let Err(error) =
                server.send_raw_request("thread/settings/update", Some(Value::Object(params)))
        {
            self.push_error(error.to_string());
        }
    }

    pub(super) fn collaboration_mode(&self, name: &str) -> Option<Value> {
        let mask = self.collaboration_modes.borrow().get(name)?.clone();
        let mode = mask.get("mode")?.as_str()?;
        let model = mask
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.selected_values
                    .borrow()
                    .get(&ChatSelector::Model)
                    .cloned()
            })?;
        let effort = match mask.get("reasoning_effort") {
            Some(Value::String(value)) => Some(Value::String(value.clone())),
            Some(Value::Null) => None,
            _ => self
                .selected_values
                .borrow()
                .get(&ChatSelector::Reasoning)
                .cloned()
                .map(Value::String),
        };
        Some(json!({
            "mode": mode,
            "settings": {
                "model": model,
                "reasoning_effort": effort,
                "developer_instructions": null
            }
        }))
    }

    pub(super) fn turn_settings(&self) -> Map<String, Value> {
        let selected = self.selected_values.borrow().clone();
        let dirty = self.dirty_selectors.borrow();
        let mut settings = Map::new();
        for (selector, field) in [
            (ChatSelector::Model, "model"),
            (ChatSelector::Reasoning, "effort"),
            (ChatSelector::ReasoningSummary, "summary"),
            (ChatSelector::Personality, "personality"),
            (ChatSelector::Permissions, "permissions"),
            (ChatSelector::ApprovalReviewer, "approvalsReviewer"),
        ] {
            if dirty.contains(&selector)
                && let Some(value) = selected.get(&selector)
            {
                settings.insert(field.to_owned(), Value::String(value.clone()));
            }
        }
        if dirty.contains(&ChatSelector::ServiceTier)
            && let Some(value) = selected.get(&ChatSelector::ServiceTier)
        {
            settings.insert(
                "serviceTier".to_owned(),
                if value == DEFAULT_SERVICE_TIER_ID {
                    Value::Null
                } else {
                    Value::String(value.clone())
                },
            );
        }
        if dirty.contains(&ChatSelector::Collaboration)
            && let Some(name) = selected.get(&ChatSelector::Collaboration)
            && let Some(mode) = self.collaboration_mode(name)
        {
            settings.insert("collaborationMode".to_owned(), mode);
        }
        settings
    }
}

pub(super) fn set_initial_selector_options(view: &CodexChatView) {
    view.set_selector_options(ChatSelector::Model, &[], None);
    view.set_selector_options(
        ChatSelector::Reasoning,
        &["low", "medium", "high", "xhigh", "max", "ultra"]
            .into_iter()
            .map(|id| SelectorOption {
                id: id.to_owned(),
                label: title_case(id),
            })
            .collect::<Vec<_>>(),
        None,
    );
    view.set_selector_options(
        ChatSelector::ReasoningSummary,
        &["auto", "concise", "detailed", "none"]
            .into_iter()
            .map(|id| SelectorOption {
                id: id.to_owned(),
                label: title_case(id),
            })
            .collect::<Vec<_>>(),
        None,
    );
    view.set_selector_options(
        ChatSelector::Personality,
        &["friendly", "pragmatic", "none"]
            .into_iter()
            .map(|id| SelectorOption {
                id: id.to_owned(),
                label: title_case(id),
            })
            .collect::<Vec<_>>(),
        None,
    );
    view.set_selector_options(ChatSelector::Permissions, &[], None);
    view.set_selector_options(ChatSelector::Collaboration, &[], None);
    view.set_selector_options(
        ChatSelector::ServiceTier,
        &[SelectorOption {
            id: DEFAULT_SERVICE_TIER_ID.to_owned(),
            label: "Standard".to_owned(),
        }],
        Some(DEFAULT_SERVICE_TIER_ID),
    );
    view.set_selector_options(
        ChatSelector::ApprovalReviewer,
        &["user", "auto_review", "guardian_subagent"]
            .into_iter()
            .map(|id| SelectorOption {
                id: id.to_owned(),
                label: title_case(id),
            })
            .collect::<Vec<_>>(),
        Some("user"),
    );
}

fn permission_label(id: &str) -> String {
    match id {
        ":read-only" => "Read only".to_owned(),
        ":workspace" => "Workspace".to_owned(),
        ":full-access" => "Full access".to_owned(),
        _ => title_case(id.trim_start_matches(':')),
    }
}
