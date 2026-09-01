use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalOptionStyle {
    Suggested,
    Default,
    Destructive,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalOption {
    pub value: String,
    pub label: String,
    pub style: ApprovalOptionStyle,
}

pub fn approval_description(params: &Value, fallback: &str) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = params.get("reason").and_then(Value::as_str) {
        parts.push(reason.to_owned());
    }
    if let Some(command) = params.get("command").and_then(Value::as_str) {
        parts.push(command.to_owned());
    }
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
        parts.push(format!("Working directory: {cwd}"));
    }
    if parts.is_empty() {
        fallback.to_owned()
    } else {
        parts.join("\n\n")
    }
}

pub fn approval_options(params: &Value, command: bool) -> Vec<ApprovalOption> {
    params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|decisions| {
            decisions
                .iter()
                .filter_map(|decision| option_from_decision(decision, command))
                .collect::<Vec<_>>()
        })
        .filter(|options| !options.is_empty())
        .unwrap_or_else(|| fallback_options(command))
}

pub fn permission_approval_options() -> Vec<ApprovalOption> {
    [
        (
            "grant",
            "Grant for this turn",
            ApprovalOptionStyle::Suggested,
        ),
        (
            "grant-session",
            "Grant for session",
            ApprovalOptionStyle::Default,
        ),
        ("decline", "Decline", ApprovalOptionStyle::Destructive),
    ]
    .into_iter()
    .map(|(value, label, style)| ApprovalOption {
        value: value.to_owned(),
        label: label.to_owned(),
        style,
    })
    .collect()
}

pub fn approval_decision_response(value: String) -> Value {
    let decision = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
    serde_json::json!({ "decision": decision })
}

pub fn permission_approval_response(params: &Value, value: &str) -> Value {
    match value {
        "grant" | "grant-session" => serde_json::json!({
            "permissions": params.get("permissions").cloned().unwrap_or_else(|| serde_json::json!({})),
            "scope": if value == "grant" { "turn" } else { "session" }
        }),
        _ => serde_json::json!({ "permissions": {}, "scope": "turn" }),
    }
}

fn option_from_decision(decision: &Value, command: bool) -> Option<ApprovalOption> {
    if let Some(value) = decision.as_str() {
        let label = match value {
            "accept" if command => "Allow once",
            "accept" => "Apply once",
            "acceptForSession" if command => "Allow for session",
            "acceptForSession" => "Apply for session",
            "decline" => "Decline",
            "cancel" => "Cancel turn",
            _ => value,
        };
        let style = match value {
            "accept" => ApprovalOptionStyle::Suggested,
            "decline" | "cancel" => ApprovalOptionStyle::Destructive,
            _ => ApprovalOptionStyle::Default,
        };
        return Some(ApprovalOption {
            value: value.to_owned(),
            label: label.to_owned(),
            style,
        });
    }

    let value = serde_json::to_string(decision).ok()?;
    let label = if decision.get("acceptWithExecpolicyAmendment").is_some() {
        "Allow and remember command"
    } else if decision.get("applyNetworkPolicyAmendment").is_some() {
        "Apply network policy"
    } else {
        "Apply proposed policy"
    };
    Some(ApprovalOption {
        value,
        label: label.to_owned(),
        style: ApprovalOptionStyle::Suggested,
    })
}

fn fallback_options(command: bool) -> Vec<ApprovalOption> {
    [
        (
            "accept",
            if command { "Allow once" } else { "Apply once" },
            ApprovalOptionStyle::Suggested,
        ),
        (
            "acceptForSession",
            if command {
                "Allow for session"
            } else {
                "Apply for session"
            },
            ApprovalOptionStyle::Default,
        ),
        ("decline", "Decline", ApprovalOptionStyle::Destructive),
        ("cancel", "Cancel turn", ApprovalOptionStyle::Destructive),
    ]
    .into_iter()
    .map(|(value, label, style)| ApprovalOption {
        value: value.to_owned(),
        label: label.to_owned(),
        style,
    })
    .collect()
}
