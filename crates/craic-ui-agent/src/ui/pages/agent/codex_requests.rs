use std::collections::HashSet;

use serde_json::{Map, Value, json};

use super::codex_chat::{
    DynamicToolRequest, McpFormField, McpFormFieldKind, McpFormRequest, McpUrlRequest,
    PendingRequest, PendingRequestKind, PendingRequestResponse, RequestOption, RequestOptionStyle,
    RequestSelectionMode, RequestUserInput, RequestUserInputQuestion, StructuredRequestOption,
    StructuredRequestResponse,
};

const MAX_RENDERED_JSON_BYTES: usize = 24 * 1024;

pub(super) fn pending_request_from_server(
    request_id: &str,
    method: &str,
    params: &Value,
) -> PendingRequest {
    match method {
        "item/commandExecution/requestApproval" => PendingRequest {
            request_id: request_id.to_owned(),
            kind: PendingRequestKind::Approval,
            title: "Run command?".to_owned(),
            description: approval_description(params, "Codex wants to run a command."),
            options: approval_options(params, true),
            allows_text: false,
            text_placeholder: None,
        },
        "item/fileChange/requestApproval" => PendingRequest {
            request_id: request_id.to_owned(),
            kind: PendingRequestKind::Approval,
            title: "Apply file changes?".to_owned(),
            description: approval_description(params, "Codex wants to modify files."),
            options: approval_options(params, false),
            allows_text: false,
            text_placeholder: None,
        },
        "item/permissions/requestApproval" => PendingRequest {
            request_id: request_id.to_owned(),
            kind: PendingRequestKind::Approval,
            title: "Grant additional permissions?".to_owned(),
            description: approval_description(params, "Codex requested additional access."),
            options: vec![
                request_option(
                    "grant",
                    "Grant for this turn",
                    RequestOptionStyle::Suggested,
                ),
                request_option(
                    "grant-session",
                    "Grant for session",
                    RequestOptionStyle::Default,
                ),
                request_option("decline", "Decline", RequestOptionStyle::Destructive),
            ],
            allows_text: false,
            text_placeholder: None,
        },
        "item/tool/requestUserInput" => structured_user_input(params)
            .map(|input| PendingRequest {
                request_id: request_id.to_owned(),
                title: input
                    .questions
                    .first()
                    .map(|question| question.header.clone())
                    .unwrap_or_else(|| "Codex needs input".to_owned()),
                description: auto_resolution_description(params),
                kind: PendingRequestKind::StructuredUserInput(input),
                options: Vec::new(),
                allows_text: false,
                text_placeholder: None,
            })
            .unwrap_or_else(|| legacy_user_input_request(request_id, params)),
        "mcpServer/elicitation/request" => match params.get("mode").and_then(Value::as_str) {
            Some("form" | "openai/form") => mcp_form_request(params)
                .map(|form| structured_mcp_form_request(request_id, params, form))
                .unwrap_or_else(|| legacy_mcp_request(request_id, params)),
            Some("url") => mcp_url_request(params)
                .map(|url| structured_mcp_url_request(request_id, params, url))
                .unwrap_or_else(|| legacy_mcp_request(request_id, params)),
            _ => legacy_mcp_request(request_id, params),
        },
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
            PendingRequest {
                request_id: request_id.to_owned(),
                kind: PendingRequestKind::DynamicToolOutput(DynamicToolRequest {
                    output_placeholder: Some(format!("Return output for {qualified_tool}")),
                    allows_failure: true,
                }),
                title: format!("Dynamic tool: {qualified_tool}"),
                description: params
                    .get("arguments")
                    .map(compact_json)
                    .unwrap_or_default(),
                options: Vec::new(),
                allows_text: false,
                text_placeholder: None,
            }
        }
        "account/chatgptAuthTokens/refresh" => PendingRequest {
            request_id: request_id.to_owned(),
            kind: PendingRequestKind::TokenRefresh,
            title: "Authentication refresh requested".to_owned(),
            description: "The configured App Server requested new authentication tokens."
                .to_owned(),
            options: vec![request_option(
                "unavailable",
                "Cannot refresh",
                RequestOptionStyle::Destructive,
            )],
            allows_text: false,
            text_placeholder: None,
        },
        _ => PendingRequest {
            request_id: request_id.to_owned(),
            kind: PendingRequestKind::Unknown(method.to_owned()),
            title: "Codex request".to_owned(),
            description: format!("{method}\n{}", compact_json(params)),
            options: vec![request_option(
                "unsupported",
                "Report unsupported",
                RequestOptionStyle::Destructive,
            )],
            allows_text: false,
            text_placeholder: None,
        },
    }
}

pub(super) fn response_for_server_request(
    method: &str,
    params: &Value,
    response: PendingRequestResponse,
) -> Result<Value, String> {
    match response.structured_payload() {
        Ok(Some(response)) => return structured_response(method, response),
        Ok(None) => {}
        Err(error) => return Err(format!("Invalid structured Codex response: {error}")),
    }

    let value = match response {
        PendingRequestResponse::Option(value) | PendingRequestResponse::Text(value) => value,
    };
    let result = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            let decision =
                serde_json::from_str::<Value>(&value).unwrap_or_else(|_| Value::String(value));
            json!({ "decision": decision })
        }
        "item/permissions/requestApproval" => match value.as_str() {
            "grant" => {
                json!({ "permissions": params.get("permissions").cloned().unwrap_or_else(|| json!({})), "scope": "turn" })
            }
            "grant-session" => {
                json!({ "permissions": params.get("permissions").cloned().unwrap_or_else(|| json!({})), "scope": "session" })
            }
            _ => json!({ "permissions": {}, "scope": "turn" }),
        },
        "item/tool/requestUserInput" => legacy_user_input_response(params, value)?,
        "mcpServer/elicitation/request" => match value.as_str() {
            "decline" | "cancel" => json!({ "action": value, "content": null, "_meta": null }),
            "accept" => json!({ "action": "accept", "content": null, "_meta": null }),
            _ => json!({
                "action": "accept",
                "content": serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value)),
                "_meta": null
            }),
        },
        "item/tool/call" => {
            let success = value != "fail";
            json!({
                "contentItems": if success { vec![json!({ "type": "inputText", "text": value })] } else { Vec::<Value>::new() },
                "success": success
            })
        }
        _ => return Err(format!("Unsupported Codex server request: {method}")),
    };
    Ok(result)
}

fn structured_response(method: &str, response: StructuredRequestResponse) -> Result<Value, String> {
    match (method, response) {
        ("item/tool/requestUserInput", StructuredRequestResponse::UserInput { answers }) => {
            Ok(json!({ "answers": answers }))
        }
        (
            "mcpServer/elicitation/request",
            StructuredRequestResponse::McpElicitation { action, content },
        ) => Ok(json!({ "action": action, "content": content, "_meta": null })),
        (
            "item/tool/call",
            StructuredRequestResponse::DynamicTool {
                content_items,
                success,
            },
        ) => Ok(json!({ "contentItems": content_items, "success": success })),
        (method, _) => Err(format!(
            "Structured response does not match Codex server request {method}"
        )),
    }
}

fn structured_user_input(params: &Value) -> Option<RequestUserInput> {
    let questions = params.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }
    let questions = questions
        .iter()
        .map(|question| {
            let id = nonempty_string(question.get("id")?)?;
            let header = nonempty_string(question.get("header")?)?;
            let prompt = nonempty_string(question.get("question")?)?;
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|option| {
                    let label = nonempty_string(option.get("label")?)?;
                    Some(StructuredRequestOption {
                        value: label.clone(),
                        label,
                        description: option
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let selection_mode = if question.get("multiple").and_then(Value::as_bool) == Some(true)
                || question.get("selectionMode").and_then(Value::as_str) == Some("multiple")
            {
                RequestSelectionMode::Multiple
            } else {
                RequestSelectionMode::Single
            };
            Some(RequestUserInputQuestion {
                id,
                header,
                question: prompt,
                options,
                selection_mode,
                allows_other: question
                    .get("isOther")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                is_secret: question
                    .get("isSecret")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(RequestUserInput { questions })
}

fn auto_resolution_description(params: &Value) -> String {
    params
        .get("autoResolutionMs")
        .and_then(Value::as_u64)
        .map(|milliseconds| {
            let seconds = milliseconds.div_ceil(1_000);
            format!("Codex may continue automatically if unanswered after {seconds} seconds.")
        })
        .unwrap_or_default()
}

fn legacy_user_input_request(request_id: &str, params: &Value) -> PendingRequest {
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
            .filter_map(|option| option.get("label").and_then(Value::as_str))
            .map(|label| request_option(label, label, RequestOptionStyle::Default))
            .collect::<Vec<_>>()
    };
    let mut description = questions
        .iter()
        .map(|question| {
            let id = question
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("answer");
            let prompt = question
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("Codex needs input");
            format!("{id}: {prompt}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if multiple {
        description.push_str(
            "\n\nEnter a JSON object mapping each question id to a string or string array.",
        );
    }
    if questions
        .iter()
        .any(|question| question.get("isSecret").and_then(Value::as_bool) == Some(true))
    {
        description.push_str("\n\nThis legacy input control cannot mask secret responses.");
    }
    PendingRequest {
        request_id: request_id.to_owned(),
        kind: PendingRequestKind::UserInput,
        title: first
            .get("header")
            .and_then(Value::as_str)
            .unwrap_or("Codex needs input")
            .to_owned(),
        description,
        allows_text: multiple
            || options.is_empty()
            || first.get("isOther").and_then(Value::as_bool) == Some(true),
        options,
        text_placeholder: Some(if multiple {
            r#"{"question_id":"answer"}"#.to_owned()
        } else {
            "Enter your response".to_owned()
        }),
    }
}

fn mcp_form_request(params: &Value) -> Option<McpFormRequest> {
    let schema = params.get("requestedSchema")?.as_object()?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<HashSet<_>>();
    let properties = schema.get("properties")?.as_object()?;
    let fields = properties
        .iter()
        .map(|(id, schema)| mcp_form_field(id, schema, required.contains(id.as_str())))
        .collect::<Option<Vec<_>>>()?;
    Some(McpFormRequest { fields })
}

fn mcp_form_field(id: &str, schema: &Value, required: bool) -> Option<McpFormField> {
    let schema = schema.as_object()?;
    let label = schema
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(id)
        .to_owned();
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let type_name = schema.get("type").and_then(Value::as_str)?;
    let kind = match type_name {
        "string" if schema.contains_key("enum") || schema.contains_key("oneOf") => {
            let (options, defaults) = single_select_schema(schema)?;
            McpFormFieldKind::Select {
                options,
                multiple: false,
                defaults,
                minimum_items: None,
                maximum_items: None,
            }
        }
        "string" => McpFormFieldKind::Text {
            default: schema
                .get("default")
                .and_then(Value::as_str)
                .map(str::to_owned),
            placeholder: schema
                .get("placeholder")
                .and_then(Value::as_str)
                .map(str::to_owned),
            format: schema
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_owned),
            minimum_length: bounded_u32(schema.get("minLength")),
            maximum_length: bounded_u32(schema.get("maxLength")),
            secret: secret_schema(schema),
        },
        "number" | "integer" => McpFormFieldKind::Number {
            default: number_string(schema.get("default")),
            minimum: number_string(schema.get("minimum")),
            maximum: number_string(schema.get("maximum")),
            integer: type_name == "integer",
        },
        "boolean" => McpFormFieldKind::Boolean {
            default: schema.get("default").and_then(Value::as_bool),
        },
        "array" => {
            let (options, defaults) = multi_select_schema(schema)?;
            McpFormFieldKind::Select {
                options,
                multiple: true,
                defaults,
                minimum_items: schema.get("minItems").and_then(Value::as_u64),
                maximum_items: schema.get("maxItems").and_then(Value::as_u64),
            }
        }
        _ => return None,
    };
    Some(McpFormField {
        id: id.to_owned(),
        label,
        description,
        required,
        kind,
    })
}

fn single_select_schema(
    schema: &Map<String, Value>,
) -> Option<(Vec<StructuredRequestOption>, Vec<String>)> {
    let options = if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let labels = schema
            .get("enumNames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let value = value.as_str()?.to_owned();
                let label = labels
                    .get(index)
                    .and_then(Value::as_str)
                    .unwrap_or(&value)
                    .to_owned();
                Some(StructuredRequestOption {
                    value,
                    label,
                    description: None,
                })
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        const_options(schema.get("oneOf")?.as_array()?)?
    };
    let defaults = schema
        .get("default")
        .and_then(Value::as_str)
        .map(|value| vec![value.to_owned()])
        .unwrap_or_default();
    Some((options, defaults))
}

fn multi_select_schema(
    schema: &Map<String, Value>,
) -> Option<(Vec<StructuredRequestOption>, Vec<String>)> {
    let items = schema.get("items")?.as_object()?;
    let options = if let Some(values) = items.get("enum").and_then(Value::as_array) {
        values
            .iter()
            .map(|value| {
                let value = value.as_str()?.to_owned();
                Some(StructuredRequestOption {
                    label: value.clone(),
                    value,
                    description: None,
                })
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        const_options(
            items
                .get("anyOf")
                .or_else(|| items.get("oneOf"))?
                .as_array()?,
        )?
    };
    let defaults = schema
        .get("default")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some((options, defaults))
}

fn const_options(values: &[Value]) -> Option<Vec<StructuredRequestOption>> {
    values
        .iter()
        .map(|option| {
            let value = option.get("const")?.as_str()?.to_owned();
            let label = option
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(&value)
                .to_owned();
            Some(StructuredRequestOption {
                value,
                label,
                description: option
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn secret_schema(schema: &Map<String, Value>) -> bool {
    ["isSecret", "secret", "sensitive", "writeOnly"]
        .into_iter()
        .any(|key| schema.get(key).and_then(Value::as_bool) == Some(true))
        || matches!(
            schema.get("format").and_then(Value::as_str),
            Some("password" | "secret")
        )
        || schema
            .get("_meta")
            .and_then(Value::as_object)
            .is_some_and(|meta| {
                ["isSecret", "secret", "sensitive"]
                    .into_iter()
                    .any(|key| meta.get(key).and_then(Value::as_bool) == Some(true))
            })
}

fn mcp_url_request(params: &Value) -> Option<McpUrlRequest> {
    Some(McpUrlRequest {
        url: nonempty_string(params.get("url")?)?,
        elicitation_id: params
            .get("elicitationId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn structured_mcp_form_request(
    request_id: &str,
    params: &Value,
    form: McpFormRequest,
) -> PendingRequest {
    PendingRequest {
        request_id: request_id.to_owned(),
        kind: PendingRequestKind::McpForm(form),
        title: mcp_request_title(params),
        description: mcp_request_message(params),
        options: Vec::new(),
        allows_text: false,
        text_placeholder: None,
    }
}

fn structured_mcp_url_request(
    request_id: &str,
    params: &Value,
    url: McpUrlRequest,
) -> PendingRequest {
    PendingRequest {
        request_id: request_id.to_owned(),
        kind: PendingRequestKind::McpUrl(url),
        title: mcp_request_title(params),
        description: mcp_request_message(params),
        options: Vec::new(),
        allows_text: false,
        text_placeholder: None,
    }
}

fn legacy_mcp_request(request_id: &str, params: &Value) -> PendingRequest {
    PendingRequest {
        request_id: request_id.to_owned(),
        kind: PendingRequestKind::McpElicitation,
        title: mcp_request_title(params),
        description: {
            let message = mcp_request_message(params);
            params
                .get("url")
                .and_then(Value::as_str)
                .map(|url| format!("{message}\n\nURL: {url}"))
                .unwrap_or(message)
        },
        options: if params.get("mode").and_then(Value::as_str) == Some("url") {
            vec![
                request_option("accept", "Acknowledge URL", RequestOptionStyle::Suggested),
                request_option("decline", "Decline", RequestOptionStyle::Default),
                request_option("cancel", "Cancel", RequestOptionStyle::Destructive),
            ]
        } else {
            vec![
                request_option("decline", "Decline", RequestOptionStyle::Default),
                request_option("cancel", "Cancel", RequestOptionStyle::Destructive),
            ]
        },
        allows_text: params.get("mode").and_then(Value::as_str) != Some("url"),
        text_placeholder: Some("Enter a value or JSON object".to_owned()),
    }
}

fn mcp_request_title(params: &Value) -> String {
    format!(
        "{} needs input",
        params
            .get("serverName")
            .and_then(Value::as_str)
            .unwrap_or("An MCP server")
    )
}

fn mcp_request_message(params: &Value) -> String {
    params
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("The MCP server requested structured input.")
        .to_owned()
}

fn legacy_user_input_response(params: &Value, value: String) -> Result<Value, String> {
    let questions = params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| question.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let answers = if questions.len() > 1 {
        let parsed = serde_json::from_str::<Value>(&value).map_err(|error| {
            format!("Enter answers as a JSON object keyed by question id: {error}")
        })?;
        let object = parsed
            .as_object()
            .ok_or_else(|| "Enter answers as a JSON object keyed by question id".to_owned())?;
        let mut answers = Map::new();
        for question_id in questions {
            let answer = object
                .get(question_id)
                .ok_or_else(|| format!("The JSON response is missing question id {question_id}"))?;
            let values = match answer {
                Value::String(answer) => vec![answer.clone()],
                Value::Array(answers) if answers.iter().all(|answer| answer.is_string()) => answers
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                _ => {
                    return Err(format!(
                        "Answer {question_id} must be a string or string array"
                    ));
                }
            };
            answers.insert(question_id.to_owned(), json!({ "answers": values }));
        }
        answers
    } else {
        questions
            .into_iter()
            .map(|question_id| {
                (
                    question_id.to_owned(),
                    json!({ "answers": [value.clone()] }),
                )
            })
            .collect::<Map<String, Value>>()
    };
    Ok(json!({ "answers": answers }))
}

fn approval_description(params: &Value, fallback: &str) -> String {
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

fn approval_options(params: &Value, command: bool) -> Vec<RequestOption> {
    let decisions = params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|decision| {
                    if let Some(decision) = decision.as_str() {
                        return Some(request_option(
                            decision,
                            decision_label(decision),
                            decision_style(decision),
                        ));
                    }
                    let id = serde_json::to_string(decision).ok()?;
                    let label = if decision.get("acceptWithExecpolicyAmendment").is_some() {
                        "Allow and remember command"
                    } else if decision.get("applyNetworkPolicyAmendment").is_some() {
                        "Apply network policy"
                    } else {
                        "Apply proposed policy"
                    };
                    Some(request_option(&id, label, RequestOptionStyle::Suggested))
                })
                .collect::<Vec<_>>()
        })
        .filter(|decisions| !decisions.is_empty());
    decisions.unwrap_or_else(|| {
        let mut options = vec![
            request_option("accept", "Allow once", RequestOptionStyle::Suggested),
            request_option(
                "acceptForSession",
                "Allow for session",
                RequestOptionStyle::Default,
            ),
        ];
        if !command {
            options[0].label = "Apply once".to_owned();
            options[1].label = "Apply for session".to_owned();
        }
        options.push(request_option(
            "decline",
            "Decline",
            RequestOptionStyle::Destructive,
        ));
        options.push(request_option(
            "cancel",
            "Cancel turn",
            RequestOptionStyle::Destructive,
        ));
        options
    })
}

fn request_option(id: &str, label: &str, style: RequestOptionStyle) -> RequestOption {
    RequestOption {
        id: id.to_owned(),
        label: label.to_owned(),
        style,
    }
}

fn decision_label(decision: &str) -> &str {
    match decision {
        "accept" => "Allow once",
        "acceptForSession" => "Allow for session",
        "decline" => "Decline",
        "cancel" => "Cancel turn",
        _ => decision,
    }
}

fn decision_style(decision: &str) -> RequestOptionStyle {
    match decision {
        "accept" => RequestOptionStyle::Suggested,
        "decline" | "cancel" => RequestOptionStyle::Destructive,
        _ => RequestOptionStyle::Default,
    }
}

fn nonempty_string(value: &Value) -> Option<String> {
    let value = value.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn bounded_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn number_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(value) => Some(value.to_string()),
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn compact_json(value: &Value) -> String {
    let mut rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if rendered.len() > MAX_RENDERED_JSON_BYTES {
        let mut boundary = MAX_RENDERED_JSON_BYTES;
        while !rendered.is_char_boundary(boundary) {
            boundary -= 1;
        }
        rendered.truncate(boundary);
        rendered.push_str("\n… output truncated …");
    }
    rendered
}
