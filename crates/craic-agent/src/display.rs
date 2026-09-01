pub fn title_case(value: &str) -> String {
    value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn relative_time(updated_at_ms: i64) -> String {
    if updated_at_ms <= 0 {
        return "Unknown time".to_string();
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(updated_at_ms);
    let age_seconds = now_ms.saturating_sub(updated_at_ms) / 1_000;
    match age_seconds {
        0..=59 => "Just now".to_string(),
        60..=3_599 => format!("{}m ago", age_seconds / 60),
        3_600..=86_399 => format!("{}h ago", age_seconds / 3_600),
        86_400..=2_591_999 => format!("{}d ago", age_seconds / 86_400),
        _ => format!("{}mo ago", age_seconds / 2_592_000),
    }
}

pub fn permission_profile_label(id: &str) -> String {
    match id {
        ":read-only" => "Read only".to_string(),
        ":workspace" => "Workspace".to_string(),
        ":full-access" | ":danger-full-access" => "Full access".to_string(),
        _ => title_case(id.trim_start_matches(':')),
    }
}

pub fn concise_title(prompt: &str) -> Option<String> {
    let prompt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return None;
    }
    let mut title = prompt.chars().take(72).collect::<String>();
    if title.chars().count() < prompt.chars().count() {
        title.push('…');
    }
    Some(title)
}

pub fn request_id_key(id: &RequestId) -> String {
    match id {
        RequestId::Integer(value) => format!("integer:{value}"),
        RequestId::String(value) => format!("string:{value}"),
    }
}

pub fn compact_json(value: &Value) -> String {
    truncated_json(value, MAX_RENDERED_JSON_BYTES, "output")
}

pub fn compact_request_json(value: &Value) -> String {
    let rendered = compact_json(value);
    truncate_rendered(rendered, MAX_REQUEST_ARGUMENT_BYTES, "arguments")
}

fn truncated_json(value: &Value, limit: usize, label: &str) -> String {
    let rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    truncate_rendered(rendered, limit, label)
}

fn truncate_rendered(mut rendered: String, limit: usize, label: &str) -> String {
    if rendered.len() <= limit {
        return rendered;
    }
    let mut boundary = limit;
    while !rendered.is_char_boundary(boundary) {
        boundary -= 1;
    }
    rendered.truncate(boundary);
    rendered.push_str(&format!("\n… {label} truncated …"));
    rendered
}
use std::time::{SystemTime, UNIX_EPOCH};

use craic_codex_app_server::protocol::RequestId;
use serde_json::Value;

const MAX_RENDERED_JSON_BYTES: usize = 24 * 1024;
const MAX_REQUEST_ARGUMENT_BYTES: usize = 4 * 1024;
