use crate::agent_provider::{self, CancellationToken};
use crate::config;
use crate::ui::agent_history::AgentSessionSummary;
use serde::Deserialize;

const COMPACTED_TERMINAL_TEXT_CHARS: usize = 100_000;

#[derive(Debug, Deserialize)]
struct AgentSummaryDraft {
    task_description: String,
    tags: Vec<String>,
}

pub(super) fn generate(
    shell_provider_id: &str,
    title: &str,
    terminal_text: &str,
    existing_tags: &[String],
) -> Result<AgentSessionSummary, String> {
    let smart_config = config::smart_feature_config(shell_provider_id);
    let prompt = summary_prompt(title, terminal_text, existing_tags);
    log::info!(
        "agent smart summary request shell_provider={} smart_provider={} model_configured={} prompt_bytes={} existing_tags={}",
        shell_provider_id,
        smart_config.provider,
        smart_config.model.is_some(),
        prompt.len(),
        existing_tags.len()
    );
    agent_provider::generate_structured::<AgentSummaryDraft, AgentSessionSummary, _>(
        &smart_config.provider,
        smart_config.model.as_deref(),
        &prompt,
        "JSON agent session summary",
        validate_summary,
        &CancellationToken::new(),
    )
}

fn validate_summary(draft: AgentSummaryDraft) -> Result<AgentSessionSummary, String> {
    let task_description = draft
        .task_description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if task_description.len() < 80 {
        return Err("Task description is too short.".to_string());
    }

    let mut tags = Vec::new();
    for tag in draft.tags {
        let tag = tag.split_whitespace().collect::<Vec<_>>().join(" ");
        if tag.is_empty()
            || tag.chars().count() > 40
            || tags
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&tag))
        {
            continue;
        }
        tags.push(tag);
        if tags.len() >= 12 {
            break;
        }
    }
    if tags.is_empty() {
        return Err("At least one tag is required.".to_string());
    }

    Ok(AgentSessionSummary {
        task_description,
        tags,
    })
}

fn summary_prompt(title: &str, terminal_text: &str, existing_tags: &[String]) -> String {
    let compacted = compact_terminal_text(terminal_text);
    let existing_tags = if existing_tags.is_empty() {
        "None yet.".to_string()
    } else {
        existing_tags.join(", ")
    };
    format!(
        r#"Summarize this interactive agent session for later search and filtering.

Return only JSON with this exact shape:
{{
  "task_description": "A detailed description of the user's task and what the session appears to be doing.",
  "tags": ["short category", "area or component", "task type"]
}}

Requirements:
- The task_description should be relatively long, specific, and useful for someone finding this session later.
- Tags should be concise searchable labels.
- Include task types, affected areas, or concepts that are evident from the transcript.
- Prefer existing workspace tags when they accurately apply, so related sessions group together.
- Add new tags when the existing tags do not cover important aspects of this session.
- Generate a diverse set of tags: include a mix of task type, subject area, component or feature area, technology or file type when evident, and workflow stage when useful.
- Avoid near-duplicate tags, overly broad tags, and tags that all describe the same dimension.
- Do not invent technologies, components, files, or outcomes that are not supported by the transcript.
- Avoid generic tags that do not help filtering.

Existing workspace tags:
{existing_tags}

Session title:
{title}

Compacted terminal transcript:
{compacted}
"#
    )
}

fn compact_terminal_text(text: &str) -> String {
    let compacted = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = compacted.chars().collect::<Vec<_>>();
    if chars.len() <= COMPACTED_TERMINAL_TEXT_CHARS {
        return compacted;
    }
    let start = chars.len().saturating_sub(COMPACTED_TERMINAL_TEXT_CHARS);
    chars[start..].iter().collect()
}
