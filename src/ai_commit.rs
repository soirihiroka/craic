use crate::git;
use serde::Deserialize;
use std::fs;
use std::path::Path;
use toml::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMessageDraft {
    pub summary: String,
    pub description: String,
}

pub fn generate(
    repo_path: &Path,
    provider_id: &str,
    model: Option<&str>,
    files: &[String],
    cancellation: &crate::agent_provider::CancellationToken,
) -> Result<CommitMessageDraft, String> {
    if files.is_empty() {
        return Err("Select at least one file before generating a commit message.".to_string());
    }

    let snapshot = git::snapshot(repo_path)?;
    let prompt = commit_prompt(
        &snapshot.name,
        &snapshot.branch,
        files,
        &selected_statuses(&snapshot.changed_files, files),
        &selected_diff(repo_path, files)?,
        read_commit_convention(repo_path),
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

fn selected_statuses(changed_files: &[git::ChangedFile], selected_files: &[String]) -> String {
    selected_files
        .iter()
        .map(|path| {
            let status = changed_files
                .iter()
                .find(|file| file.path == *path)
                .map(|file| file.status.as_str())
                .unwrap_or("?");
            format!("{status} {path}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn selected_diff(repo_path: &Path, files: &[String]) -> Result<String, String> {
    let mut diff = git_diff(repo_path, files)?;
    let untracked = untracked_file_details(repo_path, files)?;

    if !untracked.is_empty() {
        if !diff.trim().is_empty() {
            diff.push_str("\n\n");
        }
        diff.push_str(&untracked);
    }

    if diff.trim().is_empty() {
        Err("No diff found for the selected files.".to_string())
    } else {
        Ok(diff)
    }
}

fn git_diff(repo_path: &Path, files: &[String]) -> Result<String, String> {
    let mut args = vec![
        "diff".to_string(),
        "--no-color".to_string(),
        "--no-ext-diff".to_string(),
        "--find-renames".to_string(),
        "HEAD".to_string(),
        "--".to_string(),
    ];
    args.extend(files.iter().cloned());

    run_git(repo_path, &args).or_else(|_| {
        let mut fallback = vec![
            "diff".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "--find-renames".to_string(),
            "--".to_string(),
        ];
        fallback.extend(files.iter().cloned());
        run_git(repo_path, &fallback)
    })
}

fn untracked_file_details(repo_path: &Path, files: &[String]) -> Result<String, String> {
    let mut details = Vec::new();

    for file in files {
        if tracked(repo_path, file)? {
            continue;
        }

        let path = repo_path.join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            details.push(format!(
                "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@\n[untracked file could not be read]"
            ));
            continue;
        };

        match String::from_utf8(bytes) {
            Ok(text) => details.push(format!(
                "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@\n{}",
                text.lines()
                    .map(|line| format!("+{line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
            Err(_) => details.push(format!(
                "diff --git a/{file} b/{file}\nnew file mode 100644\n--- /dev/null\n+++ b/{file}\n@@\n[untracked binary file omitted]"
            )),
        }
    }

    Ok(details.join("\n\n"))
}

fn tracked(repo_path: &Path, file: &str) -> Result<bool, String> {
    git::run_git_success(repo_path, &["ls-files", "--error-unmatch", "--", file])
}

fn run_git(repo_path: &Path, args: &[String]) -> Result<String, String> {
    git::run_git_owned_untrimmed(repo_path, args)
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

fn read_commit_convention(repo_path: &Path) -> Option<String> {
    let path = repo_path.join(".craic").join("config.toml");

    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) => {
            log::debug!(
                "No repo commit convention config found at {}: {}",
                path.display(),
                err
            );
            return None;
        }
    };

    let value = match toml::from_str::<Value>(&contents) {
        Ok(value) => value,
        Err(err) => {
            log::warn!(
                "Failed to parse commit convention from {}: {}",
                path.display(),
                err
            );
            return None;
        }
    };

    let Some(raw) = value.get("commit_convention") else {
        log::debug!("No commit_convention key in {}", path.display());
        return None;
    };

    let convention = match raw {
        Value::String(value) => value.trim().to_string(),
        Value::Array(values) => values
            .iter()
            .filter_map(|entry| entry.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        _ => {
            log::warn!(
                "Unsupported commit_convention type in {} (expected string or array)",
                path.display()
            );
            return None;
        }
    };

    let convention = convention.trim().to_string();
    if convention.is_empty() {
        log::warn!("commit_convention in {} is empty", path.display());
        return None;
    }

    log::info!("Using commit_convention from {}", path.display());
    Some(convention)
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
