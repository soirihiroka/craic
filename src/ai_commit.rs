use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMessageDraft {
    pub summary: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitMessageContext {
    pub(crate) repo_name: String,
    pub(crate) branch: String,
    pub(crate) files: Vec<String>,
    pub(crate) statuses: String,
    pub(crate) diff: String,
    pub(crate) commit_convention: Option<String>,
}

pub fn generate_from_context(
    context: CommitMessageContext,
    provider_id: &str,
    model: Option<&str>,
    cancellation: &crate::agent_provider::CancellationToken,
) -> Result<CommitMessageDraft, String> {
    if context.files.is_empty() {
        return Err("Select at least one file before generating a commit message.".to_string());
    }

    let prompt = commit_prompt(
        &context.repo_name,
        &context.branch,
        &context.files,
        &context.statuses,
        &context.diff,
        context.commit_convention,
    );
    crate::agent_provider::generate_structured::<AgentDraft, CommitMessageDraft, _>(
        provider_id,
        model,
        &prompt,
        "JSON commit message",
        validate_draft,
        cancellation,
    )
}

fn commit_prompt(
    repo_name: &str,
    branch: &str,
    files: &[String],
    statuses: &str,
    diff: &str,
    commit_convention: Option<String>,
) -> String {
    let convention = commit_convention_prompt(&commit_convention);

    format!(
        r#"You are generating a Git commit message for Craic.

Read-only mode:
- Do not edit files.
- Do not run commands that write to disk.
- Use only the repository metadata and diff included in this prompt.

Return only a JSON object with this exact shape:
{{"summary":"imperative commit summary, 72 characters or less","description":"commit comment/body explaining the important changes"}}

Repository: {repo_name}
Branch: {branch}
Selected files:
{files}

File status:
{statuses}

Diff:
```diff
{diff}
```
{convention}
"#,
        repo_name = repo_name,
        branch = branch,
        files = files.join("\n"),
        statuses = statuses,
        diff = diff,
        convention = convention,
    )
}

fn commit_convention_prompt(commit_convention: &Option<String>) -> String {
    let Some(convention) = commit_convention
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return String::new();
    };

    format!(
        r#"

Additional convention guidance:
{convention}
"#
    )
}

#[derive(Deserialize)]
struct AgentDraft {
    summary: Option<String>,
    description: Option<String>,
    comments: Option<String>,
    comment: Option<String>,
    body: Option<String>,
}

fn validate_draft(draft: AgentDraft) -> Result<CommitMessageDraft, String> {
    let summary = draft
        .summary
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err("Agent returned an empty commit summary.".to_string());
    }

    let description = draft
        .description
        .or(draft.comments)
        .or(draft.comment)
        .or(draft.body)
        .unwrap_or_default()
        .trim()
        .to_string();
    if description.is_empty() {
        return Err("Agent returned empty commit comments.".to_string());
    }

    Ok(CommitMessageDraft {
        summary,
        description,
    })
}
