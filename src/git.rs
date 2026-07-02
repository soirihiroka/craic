use crate::{bitbucket, github, gitlab};
use git2::{
    BranchType, Delta, DiffFindOptions, DiffLineType, DiffOptions, ErrorCode, ObjectType, Oid,
    Repository, Status, StatusOptions, Tree,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const COMMIT_TIMEZONE_KEY: &str = "craic.commitTimezone";
const USE_SYSTEM_TIMEZONE_KEY: &str = "craic.useSystemTimezone";
const SHOW_REMOTE_OWNER_WARNING_KEY: &str = "craic.showRemoteOwnerWarning";
const DEFAULT_COMMIT_TIMEZONE: &str = "+0000";
const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const LOCAL_CONFIG_DIR: &str = ".craic/local";
const LOCAL_CONFIG_FILE: &str = "config.toml";
const LOCAL_GITIGNORE_FILE: &str = ".gitignore";
const LOCAL_GITIGNORE_CONTENTS: &str = "*\n";

struct GitSnapshotTimer {
    repo: String,
    start: Instant,
    previous: Instant,
}

impl GitSnapshotTimer {
    fn new(path: &Path) -> Self {
        let now = Instant::now();
        Self {
            repo: path.display().to_string(),
            start: now,
            previous: now,
        }
    }

    fn mark(&mut self, step: &str) {
        let now = Instant::now();
        log::info!(
            "git snapshot step={step} repo={} step_ms={} total_ms={}",
            self.repo,
            now.duration_since(self.previous).as_millis(),
            now.duration_since(self.start).as_millis()
        );
        self.previous = now;
    }

    fn finish(&self, branch: &str, branch_count: usize, changed_file_count: usize) {
        log::info!(
            "git snapshot finished repo={} total_ms={} branch={} branches={} changed_files={}",
            self.repo,
            self.start.elapsed().as_millis(),
            branch,
            branch_count,
            changed_file_count
        );
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepositorySnapshot {
    pub name: String,
    pub branch: String,
    pub branches: Vec<BranchInfo>,
    pub remote_name: Option<String>,
    pub remote_url: Option<String>,
    pub remote_owner: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
    pub last_fetch_at: Option<SystemTime>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub github_avatar_url: Option<String>,
    pub warn_if_remote_owner_mismatch: bool,
    pub changed_files: Vec<ChangedFile>,
    pub history_head: Option<String>,
}

const RECENT_BRANCHES_LIMIT: usize = 5;

#[derive(Clone, Debug, Default)]
pub struct GitSettings {
    pub global_user_name: Option<String>,
    pub global_user_email: Option<String>,
    pub local_user_name: Option<String>,
    pub local_user_email: Option<String>,
    pub use_global_user: bool,
    pub commit_timezone: Option<String>,
    pub warn_if_remote_owner_mismatch: bool,
    pub use_system_timezone: bool,
    pub github_auth_account: Option<github::GitHubAuthAccount>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct QuickActionConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_target_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalWorkspaceConfig {
    #[serde(default)]
    git: LocalGitConfig,
    #[serde(default)]
    github: LocalGitHubConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quick_action: Option<LocalQuickActionConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalGitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    use_system_timezone: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warn_if_remote_owner_mismatch: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalGitHubConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_login: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct LocalQuickActionConfig {
    #[serde(default)]
    actions: Vec<QuickActionConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub git_status_bits: u32,
    pub worktree_signature: Option<ChangedFileSignature>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFileSignature {
    pub is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub comment: String,
    pub author: String,
    pub author_email: Option<String>,
    pub relative_time: String,
    pub insertions: usize,
    pub deletions: usize,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CommitMessage {
    pub summary: String,
    pub description: String,
}

#[derive(Clone, Debug, Default)]
pub struct CommitPage {
    pub commits: Vec<Commit>,
    pub has_more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchInfo {
    pub name: String,
    pub is_current: bool,
    pub kind: BranchKind,
    pub upstream: Option<String>,
    pub is_default: bool,
    pub is_recent: bool,
}

pub struct FileComparison {
    pub rows: Vec<FileDiffRow>,
}

#[derive(Clone, Debug, Default)]
struct FilePathPair {
    old_path: Option<String>,
    new_path: Option<String>,
}

#[derive(Debug)]
struct DiffRowsWithFilePaths {
    rows: Vec<FileDiffRow>,
    paths: FilePathPair,
}

pub struct BytesComparison {
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Deleted,
    Added,
    Fold,
}

#[derive(Clone, Debug)]
pub struct FileDiffRow {
    pub left_number: Option<usize>,
    pub right_number: Option<usize>,
    pub left_text: Option<String>,
    pub right_text: Option<String>,
    pub left_kind: DiffKind,
    pub right_kind: DiffKind,
}

pub fn snapshot(path: &Path) -> Result<RepositorySnapshot, String> {
    let mut timing = GitSnapshotTimer::new(path);
    let repo = open_repo(path)?;
    timing.mark("open-repo");

    let root = repo_root(&repo)?;
    timing.mark("repo-root");

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Repository")
        .to_string();
    let branch = current_branch(&repo)?;
    timing.mark("current-branch");

    let remote_name = upstream_remote(&repo)
        .or_else(|| Some("origin".to_string()).filter(|remote| repo.find_remote(remote).is_ok()));
    let remote_url = remote_name
        .as_deref()
        .and_then(|remote| repo.find_remote(remote).ok())
        .and_then(|remote| remote.url().ok().map(ToString::to_string));
    let remote_owner = remote_url.as_deref().and_then(remote_owner_from_remote_url);
    timing.mark("remote-metadata");

    let (ahead, behind, has_upstream) = ahead_behind_count(&repo);
    timing.mark("ahead-behind");

    let last_fetch_at = last_fetch_at(&repo);
    let user_name = config_string(&repo, "user.name");
    let user_email = config_string(&repo, "user.email");
    let github_avatar_url = user_email
        .as_deref()
        .and_then(github::login_from_noreply_email)
        .map(|login| github::avatar_url_for_login(&login));
    timing.mark("user-metadata");

    let branches = branches(&repo, &root, remote_name.as_deref())?;
    timing.mark("branches");

    let warn_if_remote_owner_mismatch =
        local_config_bool_with_default(path, SHOW_REMOTE_OWNER_WARNING_KEY, true);
    timing.mark("local-settings");

    let changed_files = changed_files(&repo)?;
    timing.mark("changed-files");

    let history_head = history_head(&repo);
    timing.mark("history-head");

    timing.finish(&branch, branches.len(), changed_files.len());

    Ok(RepositorySnapshot {
        name,
        branch,
        branches,
        remote_name,
        remote_url,
        remote_owner,
        ahead,
        behind,
        has_upstream,
        last_fetch_at,
        user_name,
        user_email,
        github_avatar_url,
        warn_if_remote_owner_mismatch,
        changed_files,
        history_head,
    })
}

pub fn initialize_repository(path: &Path) -> Result<String, String> {
    log::info!("git init start path={}", path.display());
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("init")
        .output()
        .map_err(|err| format!("Failed to run git init: {err}"))?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        log::info!("git init complete path={}", path.display());
        if !stdout.is_empty() {
            Ok(stdout)
        } else if !stderr.is_empty() {
            Ok(stderr)
        } else {
            Ok("Initialized Git repository.".to_string())
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() { stderr } else { stdout };
        log::warn!("git init failed path={} error={}", path.display(), message);
        Err(if message.is_empty() {
            "git init failed.".to_string()
        } else {
            message
        })
    }
}

pub fn commit_paths(
    path: &Path,
    summary: &str,
    description: &str,
    files: &[String],
) -> Result<String, String> {
    let summary = summary.trim();

    if summary.is_empty() {
        return Err("Commit summary is required.".to_string());
    }

    if files.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }

    let commit_files = commit_target_paths(path, files)?;
    if commit_files.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }
    stage_commit_paths(path, &commit_files)?;

    let mut args = vec![
        "commit".to_string(),
        "--only".to_string(),
        "-m".to_string(),
        summary.to_string(),
    ];
    let description = description.trim();
    if !description.is_empty() {
        args.push("-m".to_string());
        args.push(description.to_string());
    }
    args.push("--".to_string());
    args.extend(commit_files.iter().cloned());

    run_git_owned_with_commit_timezone(path, &args)
}

fn commit_target_paths(path: &Path, selected_files: &[String]) -> Result<Vec<String>, String> {
    let repo = Repository::open(path).map_err(|err| err.message().to_string())?;
    let mut options = changed_files_status_options(true);
    let statuses = match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses,
        Err(err) => {
            log::warn!(
                "failed to refresh git index while resolving commit targets: {}",
                err.message()
            );
            let mut fallback_options = changed_files_status_options(false);
            repo.statuses(Some(&mut fallback_options))
                .map_err(|err| err.message().to_string())?
        }
    };

    let mut snapshot_entries = Vec::new();
    for entry in statuses
        .iter()
        .filter(|entry| entry.status() != Status::CURRENT)
    {
        if entry.status().contains(Status::IGNORED) {
            continue;
        }

        snapshot_entries.push((
            status_entry_old_path(&entry),
            status_entry_new_path(&entry),
            entry.status(),
        ));
    }

    let mut commit_files = Vec::<String>::new();
    let mut seen = HashSet::new();

    for requested in selected_files {
        let mut resolved = false;

        for (old_path, new_path, status) in &snapshot_entries {
            let matches_requested = old_path.as_deref().is_some_and(|path| path == requested)
                || new_path.as_deref().is_some_and(|path| path == requested);

            if !matches_requested {
                continue;
            }

            let staged: Vec<String> =
                if status.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
                    [old_path.as_deref(), new_path.as_deref()]
                        .into_iter()
                        .flatten()
                        .map(ToString::to_string)
                        .collect()
                } else {
                    new_path
                        .as_ref()
                        .or(old_path.as_ref())
                        .into_iter()
                        .map(ToString::to_string)
                        .collect()
                };

            for staged_path in staged {
                if seen.insert(staged_path.clone()) {
                    commit_files.push(staged_path);
                }
            }

            resolved = true;
            break;
        }

        if !resolved && seen.insert(requested.clone()) {
            commit_files.push(requested.clone());
        }
    }

    Ok(commit_files)
}

fn stage_commit_paths(path: &Path, files: &[String]) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }

    let mut add_args = vec!["add".to_string(), "--all".to_string(), "--".to_string()];
    add_args.extend(files.iter().cloned());
    run_git_owned(path, &add_args).map(|_| ())
}

pub fn discard_path(path: &Path, file_path: &str) -> Result<String, String> {
    let status = run_git(path, &["status", "--porcelain", "--", file_path])?;
    if status.lines().any(|line| line.starts_with("??")) {
        let target = path.join(file_path);
        if target.is_dir() {
            std::fs::remove_dir_all(&target).map_err(|err| err.to_string())?;
        } else if target.exists() {
            std::fs::remove_file(&target).map_err(|err| err.to_string())?;
        }
        return Ok(format!("Discarded {file_path}."));
    }

    run_git(
        path,
        &["restore", "--staged", "--worktree", "--", file_path],
    )?;
    Ok(format!("Discarded {file_path}."))
}

pub fn settings(path: &Path) -> GitSettings {
    let local_user_name = local_config_string(path, "user.name");
    let local_user_email = local_config_string(path, "user.email");
    let local_workspace_config = load_local_workspace_config(path);
    let local_git_config = local_workspace_config.git.clone();

    GitSettings {
        global_user_name: global_config_string("user.name"),
        global_user_email: global_config_string("user.email"),
        use_global_user: local_user_name.is_none() && local_user_email.is_none(),
        local_user_name,
        local_user_email,
        commit_timezone: local_git_config
            .commit_timezone
            .or_else(|| local_config_string(path, COMMIT_TIMEZONE_KEY)),
        warn_if_remote_owner_mismatch: local_git_config
            .warn_if_remote_owner_mismatch
            .unwrap_or_else(|| {
                local_config_bool_with_default(path, SHOW_REMOTE_OWNER_WARNING_KEY, true)
            }),
        use_system_timezone: local_git_config
            .use_system_timezone
            .unwrap_or_else(|| local_config_bool(path, USE_SYSTEM_TIMEZONE_KEY)),
        github_auth_account: local_github_auth_account(&local_workspace_config.github),
    }
}

pub fn save_settings(
    path: &Path,
    use_global_user: bool,
    user_name: &str,
    user_email: &str,
    commit_timezone: &str,
    warn_if_remote_owner_mismatch: bool,
    use_system_timezone: bool,
    github_auth_account: Option<&github::GitHubAuthAccount>,
) -> Result<(), String> {
    if use_global_user {
        unset_local_config(path, "user.name")?;
        unset_local_config(path, "user.email")?;
    } else {
        set_local_config(path, "user.name", user_name.trim())?;
        set_local_config(path, "user.email", user_email.trim())?;
    }

    save_local_git_config(
        path,
        commit_timezone,
        warn_if_remote_owner_mismatch,
        use_system_timezone,
        github_auth_account,
    )?;

    let _ = unset_local_config(path, COMMIT_TIMEZONE_KEY);
    let _ = unset_local_config(path, USE_SYSTEM_TIMEZONE_KEY);
    let _ = unset_local_config(path, SHOW_REMOTE_OWNER_WARNING_KEY);

    Ok(())
}

pub fn quick_action_config(path: &Path) -> Option<Vec<QuickActionConfig>> {
    load_local_workspace_config(path)
        .quick_action
        .map(|config| config.actions)
}

pub fn save_quick_action_config(
    path: &Path,
    actions: Vec<QuickActionConfig>,
) -> Result<(), String> {
    let mut config = load_local_workspace_config(path);
    config.quick_action = Some(LocalQuickActionConfig { actions });
    save_local_workspace_config(path, &config)
}

pub fn save_author_email(path: &Path, email: &str) -> Result<(), String> {
    let email = email.trim();
    if email.is_empty() {
        return Err("Author email is required.".to_string());
    }

    set_local_config(path, "user.email", email)
}

pub fn push(path: &Path) -> Result<String, String> {
    switch_github_auth_for_workspace(path)?;
    run_git(path, &["push"])
}

fn conflicted_files(repo_path: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(repo) = open_repo(repo_path) {
        if let Ok(index) = repo.index() {
            if let Ok(conflicts) = index.conflicts() {
                for conflict_res in conflicts {
                    if let Ok(conflict) = conflict_res {
                        let entry = conflict
                            .our
                            .as_ref()
                            .or(conflict.their.as_ref())
                            .or(conflict.ancestor.as_ref());
                        if let Some(entry) = entry {
                            let path_str = String::from_utf8_lossy(&entry.path).into_owned();
                            if !files.contains(&path_str) {
                                files.push(path_str);
                            }
                        }
                    }
                }
            }
        }
    }
    files
}

pub fn pull(path: &Path) -> Result<String, String> {
    switch_github_auth_for_workspace(path)?;
    match run_git(path, &["pull", "--rebase"]) {
        Ok(output) => Ok(output),
        Err(err) => {
            let conflicts = conflicted_files(path);
            let _ = run_git(path, &["rebase", "--abort"]);
            let _ = run_git(path, &["merge", "--abort"]);
            if !conflicts.is_empty() {
                let file_list = conflicts
                    .iter()
                    .map(|file| format!("  • {file}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(format!(
                    "Pull failed due to merge conflicts in the following files:\n\n{file_list}\n\nNote: The pull/rebase was aborted automatically to keep your repository in a safe state."
                ))
            } else {
                Err(format!(
                    "{err}\n\nNote: The pull/rebase was aborted automatically to keep your repository in a safe state."
                ))
            }
        }
    }
}

pub fn publish(path: &Path, remote: &str, branch: &str) -> Result<String, String> {
    switch_github_auth_for_workspace(path)?;
    run_git(path, &["push", "-u", remote, branch])
}

pub fn root_for_path(path: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(path).ok()?;
    repo_root(&repo)
        .ok()
        .map(|root| root.canonicalize().unwrap_or(root))
}

pub fn fetch_with_progress(
    path: &Path,
    remote: Option<&str>,
    mut progress: impl FnMut(String),
) -> Result<String, String> {
    switch_github_auth_for_workspace(path)?;

    let mut args = vec!["fetch", "--progress"];
    if let Some(remote) = remote {
        args.push(remote);
    }

    let mut last_progress = String::new();
    run_git_streaming_stderr(path, &args, |line| {
        if let Some(label) = git_fetch_progress_label(line)
            && label != last_progress
        {
            last_progress = label.clone();
            progress(label);
        }
    })
}

pub fn checkout_branch(path: &Path, branch: &str) -> Result<String, String> {
    run_git(path, &["checkout", branch])
}

pub fn checkout_remote_branch(
    path: &Path,
    remote_branch: &str,
    local_branch: &str,
) -> Result<String, String> {
    run_git(path, &["checkout", remote_branch, "-b", local_branch, "--"])
}

pub fn checkout_pull_request(path: &Path, number: u32) -> Result<String, String> {
    let output = Command::new("gh")
        .current_dir(path)
        .arg("pr")
        .arg("checkout")
        .arg(number.to_string())
        .output()
        .map_err(|err| format!("Failed to run gh pr checkout {number}: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh pr checkout {number} failed: {stdout}")
        } else {
            format!("gh pr checkout {number} failed: {stderr}")
        })
    }
}

pub fn create_branch(path: &Path, branch: &str) -> Result<String, String> {
    run_git(path, &["checkout", "-b", branch])
}

pub fn checkout_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["checkout", hash])
}

pub fn create_branch_at_commit(path: &Path, branch: &str, hash: &str) -> Result<String, String> {
    run_git(path, &["checkout", "-b", branch, hash])
}

pub fn create_tag(path: &Path, tag: &str, hash: &str) -> Result<String, String> {
    run_git(path, &["tag", tag, hash])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Mixed,
    Hard,
}

pub fn reset_to_commit(path: &Path, hash: &str, mode: ResetMode) -> Result<String, String> {
    let mode_arg = match mode {
        ResetMode::Mixed => "--mixed",
        ResetMode::Hard => "--hard",
    };
    run_git(path, &["reset", mode_arg, hash])
}

pub fn revert_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["revert", "--no-edit", hash])
}

pub fn cherry_pick_commit(path: &Path, hash: &str) -> Result<String, String> {
    run_git(path, &["cherry-pick", hash])
}

pub fn amend_head(path: &Path, summary: &str, description: &str) -> Result<String, String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("Commit summary is required.".to_string());
    }

    let mut args = vec![
        "commit".to_string(),
        "--amend".to_string(),
        "-m".to_string(),
        summary.to_string(),
    ];
    let description = description.trim();
    if !description.is_empty() {
        args.push("-m".to_string());
        args.push(description.to_string());
    }

    run_git_owned_with_commit_timezone(path, &args)
}

pub fn stash_changes(path: &Path) -> Result<String, String> {
    run_git(path, &["stash", "-u"])
}

pub fn pop_stash(path: &Path) -> Result<String, String> {
    run_git(path, &["stash", "pop"])
}

pub fn remote_web_url(remote_url: &str) -> String {
    let trimmed = remote_url.trim_end_matches(".git");

    if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        return format!("https://github.com/{path}");
    }

    trimmed.to_string()
}

pub fn github_slug_for_path(path: &Path) -> Option<String> {
    let repo = open_repo(path).ok()?;
    github_slug_for_repo(&repo)
}

pub fn remote_commit_web_url(remote_url: &str, hash: &str) -> String {
    format!("{}/commit/{hash}", remote_web_url(remote_url))
}

pub fn comparison(path: &Path, file_path: &str) -> Result<FileComparison, String> {
    let repo = open_repo(path)?;
    let DiffRowsWithFilePaths { rows, paths } = file_diff_rows_with_paths(&repo, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_worktree_text_previewable(&repo, old_path, new_path)?;

    let rows = complete_diff_rows(
        rows,
        &head_file_lines(&repo, old_path)?,
        &workdir_file_lines(&repo, new_path)?,
    );

    Ok(FileComparison { rows })
}

pub fn commit_details(path: &Path, hash: &str) -> Result<Commit, String> {
    let repo = open_repo(path)?;
    let oid = Oid::from_str(hash).map_err(|err| err.message().to_string())?;
    let commit = repo
        .find_commit(oid)
        .map_err(|err| err.message().to_string())?;
    let tags = tags_for_commit(path, hash).unwrap_or_default();

    Ok(commit_info(&repo, oid, &commit, tags))
}

pub fn commit_message(path: &Path, hash: &str) -> Result<CommitMessage, String> {
    let repo = open_repo(path)?;
    let oid = Oid::from_str(hash).map_err(|err| err.message().to_string())?;
    let commit = repo
        .find_commit(oid)
        .map_err(|err| err.message().to_string())?;
    let (summary, description) = commit_message_parts(commit.message().unwrap_or_default());

    Ok(CommitMessage {
        summary,
        description,
    })
}

fn commit_message_parts(message: &str) -> (String, String) {
    let message = message.trim_end();
    let mut parts = message.splitn(2, '\n');
    let summary = parts.next().unwrap_or_default().trim().to_string();
    let description = parts
        .next()
        .unwrap_or_default()
        .trim_start_matches('\n')
        .trim_end()
        .to_string();

    (summary, description)
}

pub fn commit_parent_hash(path: &Path, hash: &str) -> Result<Option<String>, String> {
    let repo = open_repo(path)?;
    let oid = Oid::from_str(hash).map_err(|err| err.message().to_string())?;
    let commit = repo
        .find_commit(oid)
        .map_err(|err| err.message().to_string())?;

    if commit.parent_count() == 0 {
        return Ok(None);
    }

    commit
        .parent_id(0)
        .map(|oid| Some(oid.to_string()))
        .map_err(|err| err.message().to_string())
}

pub fn tags_for_commit(path: &Path, hash: &str) -> Result<Vec<String>, String> {
    let repo = open_repo(path)?;
    let oid = Oid::from_str(hash).map_err(|err| err.message().to_string())?;
    let tag_names = repo
        .tag_names(None)
        .map_err(|err| err.message().to_string())?;
    let mut tags = Vec::new();

    for name in tag_names.iter().filter_map(|name| name.ok().flatten()) {
        let Ok(object) = repo.revparse_single(&format!("refs/tags/{name}")) else {
            continue;
        };
        let Ok(commit) = object.peel(ObjectType::Commit) else {
            continue;
        };
        if commit.id() == oid {
            tags.push(name.to_string());
        }
    }

    tags.sort();
    Ok(tags)
}

pub fn commit_page(path: &Path, after: Option<&str>, limit: usize) -> Result<CommitPage, String> {
    let repo = open_repo(path)?;
    paged_commits(&repo, after, limit)
}

pub fn commit_search_page(
    path: &Path,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let repo = open_repo(path)?;
    if query.trim().is_empty() {
        return paged_commits(&repo, after, limit);
    }
    paged_commit_search(&repo, query, after, limit)
}

pub fn commit_changed_files(path: &Path, hash: &str) -> Result<Vec<ChangedFile>, String> {
    let repo = open_repo(path)?;
    let (old_tree, new_tree) = commit_trees(&repo, hash)?;
    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
        .map_err(|err| err.message().to_string())?;
    let mut files = Vec::new();

    diff.foreach(
        &mut |delta, _progress| {
            if let Some(path) = path_from_delta(&delta) {
                files.push(ChangedFile {
                    status: delta_status_label(delta.status()).to_string(),
                    path,
                    git_status_bits: 0,
                    worktree_signature: None,
                });
            }
            true
        },
        None,
        None,
        None,
    )
    .map_err(|err| err.message().to_string())?;

    sort_changed_files(&mut files);
    Ok(files)
}

pub fn commit_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<FileComparison, String> {
    let repo = open_repo(path)?;
    let (old_tree, new_tree) = commit_trees(&repo, hash)?;
    let DiffRowsWithFilePaths { rows, paths } =
        commit_file_diff_rows_with_paths(&repo, old_tree.as_ref(), &new_tree, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_commit_text_previewable(&repo, old_tree.as_ref(), &new_tree, old_path, new_path)?;
    let rows = complete_diff_rows(
        rows,
        &tree_file_lines_opt(&repo, old_tree.as_ref(), old_path)?,
        &tree_file_lines(&repo, &new_tree, new_path)?,
    );

    Ok(FileComparison { rows })
}

fn open_repo(path: &Path) -> Result<Repository, String> {
    Repository::discover(path).map_err(|err| err.message().to_string())
}

fn repo_root(repo: &Repository) -> Result<PathBuf, String> {
    repo.workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| "Bare repositories are not supported.".to_string())
}

fn current_branch(repo: &Repository) -> Result<String, String> {
    match repo.head() {
        Ok(head) => {
            if let Ok(name) = head.shorthand() {
                return Ok(name.to_string());
            }
            if let Some(oid) = head.target() {
                return Ok(oid.to_string()[..7].to_string());
            }
            Err("Unable to resolve HEAD.".to_string())
        }
        Err(err) if err.code() == ErrorCode::UnbornBranch => Ok("main".to_string()),
        Err(err) => Err(err.message().to_string()),
    }
}

fn branches(
    repo: &Repository,
    root: &Path,
    remote_name: Option<&str>,
) -> Result<Vec<BranchInfo>, String> {
    let current = current_branch(repo).unwrap_or_default();
    let mut branches = Vec::new();
    let iter = repo
        .branches(None)
        .map_err(|err| err.message().to_string())?;

    for branch in iter {
        let (branch, kind) = branch.map_err(|err| err.message().to_string())?;
        let Some(name) = branch.name().map_err(|err| err.message().to_string())? else {
            continue;
        };
        if name.ends_with("/HEAD") {
            continue;
        }
        let upstream = if kind == BranchType::Local {
            branch
                .upstream()
                .ok()
                .and_then(|upstream| upstream.name().ok().flatten().map(ToString::to_string))
        } else {
            None
        };

        branches.push(BranchInfo {
            name: name.to_string(),
            is_current: name == current,
            kind: if kind == BranchType::Local {
                BranchKind::Local
            } else {
                BranchKind::Remote
            },
            upstream,
            is_default: false,
            is_recent: false,
        });
    }

    let default_name = default_branch_name(repo, remote_name);
    let remote_ref = remote_name
        .and_then(|remote| remote_head(repo, remote).map(|head| format!("{remote}/{head}")));
    if let Some(index) = find_default_branch_index(&branches, &default_name, remote_ref.as_deref())
    {
        branches[index].is_default = true;
    }

    let mut by_name = HashMap::new();
    for (index, branch) in branches.iter().enumerate() {
        if branch.kind == BranchKind::Local {
            by_name.insert(branch.name.clone(), index);
        }
    }
    for name in recent_branch_names(root, RECENT_BRANCHES_LIMIT + 1) {
        if let Some(index) = by_name.get(&name).copied()
            && !branches[index].is_default
        {
            branches[index].is_recent = true;
        }
    }
    log::debug!(
        "branch metadata grouped root={} branches={} default={} recent={}",
        root.display(),
        branches.len(),
        branches.iter().filter(|branch| branch.is_default).count(),
        branches.iter().filter(|branch| branch.is_recent).count()
    );

    branches.sort_by(|left, right| match (left.is_current, right.is_current) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.name.cmp(&right.name),
    });

    Ok(branches)
}

fn default_branch_name(repo: &Repository, remote_name: Option<&str>) -> String {
    remote_name
        .and_then(|remote| remote_head(repo, remote))
        .or_else(|| config_string(repo, "init.defaultBranch"))
        .unwrap_or_else(|| "main".to_string())
}

fn remote_head(repo: &Repository, remote_name: &str) -> Option<String> {
    let reference = repo
        .find_reference(&format!("refs/remotes/{remote_name}/HEAD"))
        .ok()?;
    let target = reference.symbolic_target().ok()??;
    target
        .strip_prefix(&format!("refs/remotes/{remote_name}/"))
        .map(ToString::to_string)
}

fn find_default_branch_index(
    branches: &[BranchInfo],
    default_name: &str,
    remote_ref: Option<&str>,
) -> Option<usize> {
    let mut local_hit = None;
    let mut local_tracking_hit = None;
    let mut remote_hit = None;

    for (index, branch) in branches.iter().enumerate() {
        match branch.kind {
            BranchKind::Local => {
                if branch.name == default_name {
                    local_hit = Some(index);
                }
                if remote_ref
                    .is_some_and(|remote_ref| branch.upstream.as_deref() == Some(remote_ref))
                    && (local_tracking_hit.is_none() || branch.name == default_name)
                {
                    local_tracking_hit = Some(index);
                }
            }
            BranchKind::Remote => {
                if remote_ref.is_some_and(|remote_ref| branch.name == remote_ref) {
                    remote_hit = Some(index);
                }
            }
        }
    }

    local_tracking_hit.or(local_hit).or(remote_hit)
}

fn recent_branch_names(root: &Path, limit: usize) -> Vec<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "log",
            "-g",
            "--no-abbrev-commit",
            "--pretty=oneline",
            "HEAD",
            "-n",
            "2500",
            "--",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    parse_recent_branch_names(&String::from_utf8_lossy(&output.stdout), limit)
}

fn parse_recent_branch_names(output: &str, limit: usize) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    let mut excluded = HashSet::new();

    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        let is_renamed = lower.contains(" renamed ");
        let Some((from, to)) = line
            .split_once(": moving from ")
            .or_else(|| line.split_once(" renamed "))
            .and_then(|(_, rest)| rest.rsplit_once(" to "))
        else {
            continue;
        };
        let from = from.trim().trim_start_matches("refs/heads/");
        let to = to.trim().trim_start_matches("refs/heads/");
        if is_renamed {
            excluded.insert(from.to_string());
        }
        if !to.is_empty() && !excluded.contains(to) && seen.insert(to.to_string()) {
            names.push(to.to_string());
        }
        if names.len() >= limit {
            break;
        }
    }

    names
}

fn upstream_remote(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    let refname = head.name().ok()?;
    repo.branch_upstream_remote(refname)
        .ok()
        .and_then(|name| name.as_str().ok().map(ToString::to_string))
}

fn github_slug_for_repo(repo: &Repository) -> Option<String> {
    let remote_name = upstream_remote(repo)
        .or_else(|| Some("origin".to_string()).filter(|remote| repo.find_remote(remote).is_ok()))?;
    let remote = repo.find_remote(&remote_name).ok()?;
    let url = remote.url().ok()?;
    parse_repo_slug_from_remote_url(url)
}

fn ahead_behind_count(repo: &Repository) -> (u32, u32, bool) {
    let Ok(head) = repo.head() else {
        return (0, 0, false);
    };
    let Some(local_oid) = head.target() else {
        return (0, 0, false);
    };
    let Ok(branch_name) = head.shorthand() else {
        return (0, 0, false);
    };
    let Ok(branch) = repo.find_branch(branch_name, BranchType::Local) else {
        return (0, 0, false);
    };
    let Ok(upstream) = branch.upstream() else {
        return (0, 0, false);
    };
    let Some(upstream_oid) = upstream.get().target() else {
        return (0, 0, true);
    };

    repo.graph_ahead_behind(local_oid, upstream_oid)
        .map(|(ahead, behind)| {
            (
                ahead.min(u32::MAX as usize) as u32,
                behind.min(u32::MAX as usize) as u32,
                true,
            )
        })
        .unwrap_or((0, 0, true))
}

fn last_fetch_at(repo: &Repository) -> Option<SystemTime> {
    std::fs::metadata(repo.path().join("FETCH_HEAD"))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
}

fn config_string(repo: &Repository, key: &str) -> Option<String> {
    repo.config()
        .ok()
        .and_then(|config| config.get_string(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn local_config_string(path: &Path, key: &str) -> Option<String> {
    run_git(path, &["config", "--local", "--get", key])
        .ok()
        .filter(|value| !value.is_empty())
}

fn local_config_bool(path: &Path, key: &str) -> bool {
    run_git(path, &["config", "--local", "--bool", "--get", key])
        .ok()
        .is_some_and(|value| value == "true")
}

fn local_config_bool_with_default(path: &Path, key: &str, default: bool) -> bool {
    run_git(path, &["config", "--local", "--bool", "--get", key])
        .ok()
        .and_then(|value| {
            if value == "true" {
                Some(true)
            } else if value == "false" {
                Some(false)
            } else {
                None
            }
        })
        .unwrap_or(default)
}

fn load_local_workspace_config(path: &Path) -> LocalWorkspaceConfig {
    let config_path = local_workspace_config_path(path);

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return LocalWorkspaceConfig::default();
        }
        Err(err) => {
            log::warn!(
                "failed to read local workspace config path={} err={}",
                config_path.display(),
                err
            );
            return LocalWorkspaceConfig::default();
        }
    };

    match toml::from_str::<LocalWorkspaceConfig>(&contents) {
        Ok(config) => config,
        Err(err) => {
            log::warn!(
                "failed to parse local workspace config path={} err={}",
                config_path.display(),
                err
            );
            LocalWorkspaceConfig::default()
        }
    }
}

fn save_local_git_config(
    path: &Path,
    commit_timezone: &str,
    warn_if_remote_owner_mismatch: bool,
    use_system_timezone: bool,
    github_auth_account: Option<&github::GitHubAuthAccount>,
) -> Result<(), String> {
    let timezone = commit_timezone.trim();
    let commit_timezone = if timezone.is_empty() {
        None
    } else {
        Some(normalize_timezone(timezone)?)
    };

    let mut config = load_local_workspace_config(path);
    config.git.commit_timezone = commit_timezone;
    config.git.use_system_timezone = Some(use_system_timezone);
    config.git.warn_if_remote_owner_mismatch = Some(warn_if_remote_owner_mismatch);
    config.github = local_github_config(github_auth_account);
    save_local_workspace_config(path, &config)
}

fn local_github_auth_account(config: &LocalGitHubConfig) -> Option<github::GitHubAuthAccount> {
    let host = config.auth_host.as_deref()?.trim();
    let login = config.auth_login.as_deref()?.trim();
    if host.is_empty() || login.is_empty() {
        return None;
    }

    Some(github::GitHubAuthAccount {
        host: host.to_string(),
        login: login.to_string(),
    })
}

fn local_github_config(account: Option<&github::GitHubAuthAccount>) -> LocalGitHubConfig {
    let Some(account) = account else {
        return LocalGitHubConfig::default();
    };
    let host = account.host.trim();
    let login = account.login.trim();
    if host.is_empty() || login.is_empty() {
        return LocalGitHubConfig::default();
    }

    LocalGitHubConfig {
        auth_host: Some(host.to_string()),
        auth_login: Some(login.to_string()),
    }
}

fn switch_github_auth_for_workspace(path: &Path) -> Result<(), String> {
    let config = load_local_workspace_config(path);
    let Some(account) = local_github_auth_account(&config.github) else {
        return Ok(());
    };

    log::info!(
        "switching github auth account for workspace host={} account={}",
        account.host,
        account.login
    );
    let output = Command::new("gh")
        .arg("auth")
        .arg("switch")
        .arg("--hostname")
        .arg(&account.host)
        .arg("--user")
        .arg(&account.login)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run gh auth switch: {err}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() {
            format!("gh auth switch failed: {stdout}")
        } else {
            format!("gh auth switch failed: {stderr}")
        })
    }
}

fn save_local_workspace_config(path: &Path, config: &LocalWorkspaceConfig) -> Result<(), String> {
    let config_path = local_workspace_config_path(path);
    let local_dir = config_path
        .parent()
        .ok_or_else(|| "Failed to resolve local workspace config directory.".to_string())?;

    std::fs::create_dir_all(local_dir).map_err(|err| {
        format!(
            "Failed to create local workspace config directory {}: {err}",
            local_dir.display()
        )
    })?;
    ensure_local_workspace_gitignore(local_dir)?;

    let contents = toml::to_string_pretty(config)
        .map_err(|err| format!("Failed to serialize local workspace config: {err}"))?;
    std::fs::write(&config_path, contents).map_err(|err| {
        format!(
            "Failed to write local workspace config {}: {err}",
            config_path.display()
        )
    })?;
    log::info!(
        "saved local workspace config path={}",
        config_path.display()
    );
    Ok(())
}

fn ensure_local_workspace_gitignore(local_dir: &Path) -> Result<(), String> {
    let gitignore_path = local_dir.join(LOCAL_GITIGNORE_FILE);
    std::fs::write(&gitignore_path, LOCAL_GITIGNORE_CONTENTS).map_err(|err| {
        format!(
            "Failed to write local workspace gitignore {}: {err}",
            gitignore_path.display()
        )
    })?;
    log::debug!(
        "initialized local workspace gitignore path={}",
        gitignore_path.display()
    );
    Ok(())
}

fn local_workspace_config_path(path: &Path) -> PathBuf {
    path.join(LOCAL_CONFIG_DIR).join(LOCAL_CONFIG_FILE)
}

fn global_config_string(key: &str) -> Option<String> {
    Command::new("git")
        .args(["config", "--global", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn set_local_config(path: &Path, key: &str, value: &str) -> Result<(), String> {
    run_git(path, &["config", "--local", key, value]).map(|_| ())
}

fn unset_local_config(path: &Path, key: &str) -> Result<(), String> {
    let output = Command::new("git")
        .args(["config", "--local", "--unset", key])
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() || output.status.code() == Some(5) {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn changed_files(repo: &Repository) -> Result<Vec<ChangedFile>, String> {
    let root = repo_root(repo)?;
    let mut options = changed_files_status_options(true);
    let statuses = match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses,
        Err(err) => {
            log::warn!(
                "failed to refresh git index while reading changed files: {}",
                err.message()
            );
            let mut options = changed_files_status_options(false);
            repo.statuses(Some(&mut options))
                .map_err(|err| err.message().to_string())?
        }
    };
    let mut files = Vec::new();

    for entry in statuses
        .iter()
        .filter(|entry| entry.status() != Status::CURRENT)
    {
        if entry.status().contains(Status::IGNORED) {
            continue;
        }

        let raw_status = entry.status();
        let path = status_path(&entry);
        let mut status = status_label(raw_status).to_string();
        if status == "M" && deletion_only_change(&root, &path) {
            status = "M-".to_string();
        }

        let worktree_signature = changed_file_worktree_signature(&root, &path);
        files.push(ChangedFile {
            status,
            path,
            git_status_bits: raw_status.bits(),
            worktree_signature,
        });
    }

    sort_changed_files(&mut files);
    Ok(files)
}

fn changed_files_status_options(update_index: bool) -> StatusOptions {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true)
        .update_index(update_index);
    options
}

fn changed_file_worktree_signature(root: &Path, file_path: &str) -> Option<ChangedFileSignature> {
    let metadata = std::fs::symlink_metadata(root.join(file_path)).ok()?;
    Some(ChangedFileSignature {
        is_dir: metadata.is_dir(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

fn deletion_only_change(repo_path: &Path, file_path: &str) -> bool {
    let Ok(output) = run_git(repo_path, &["diff", "--numstat", "HEAD", "--", file_path]) else {
        return false;
    };
    let mut insertions = 0;
    let mut deletions = 0;

    for line in output.lines() {
        let mut fields = line.split('\t');
        let (Some(added), Some(deleted)) = (fields.next(), fields.next()) else {
            continue;
        };
        let (Ok(added), Ok(deleted)) = (added.parse::<usize>(), deleted.parse::<usize>()) else {
            return false;
        };
        insertions += added;
        deletions += deleted;
    }

    insertions == 0 && deletions > 0
}

fn sort_changed_files(files: &mut [ChangedFile]) {
    files.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.status.cmp(&right.status))
    });
}

fn status_path(entry: &git2::StatusEntry<'_>) -> String {
    entry
        .index_to_workdir()
        .and_then(|delta| path_from_delta(&delta))
        .or_else(|| {
            entry
                .head_to_index()
                .and_then(|delta| path_from_delta(&delta))
        })
        .unwrap_or_else(|| String::from_utf8_lossy(entry.path_bytes()).to_string())
}

fn path_from_delta(delta: &git2::DiffDelta<'_>) -> Option<String> {
    let path = match delta.status() {
        Delta::Deleted => delta.old_file().path().or_else(|| delta.new_file().path()),
        _ => delta.new_file().path().or_else(|| delta.old_file().path()),
    }?;

    Some(path.to_string_lossy().to_string())
}

fn status_entry_old_path(entry: &git2::StatusEntry<'_>) -> Option<String> {
    entry
        .head_to_index()
        .or_else(|| entry.index_to_workdir())
        .and_then(|delta| delta.old_file().path())
        .map(|path| path.to_string_lossy().to_string())
}

fn status_entry_new_path(entry: &git2::StatusEntry<'_>) -> Option<String> {
    entry
        .head_to_index()
        .or_else(|| entry.index_to_workdir())
        .and_then(|delta| delta.new_file().path())
        .map(|path| path.to_string_lossy().to_string())
}

fn status_label(status: Status) -> &'static str {
    if status.contains(Status::CONFLICTED) {
        "U"
    } else if status.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
        "R"
    } else if status.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
        "D"
    } else if status.intersects(Status::INDEX_NEW | Status::WT_NEW) {
        "A"
    } else {
        "M"
    }
}

fn delta_status_label(delta: Delta) -> &'static str {
    match delta {
        Delta::Added => "A",
        Delta::Deleted => "D",
        Delta::Renamed => "R",
        Delta::Conflicted => "U",
        _ => "M",
    }
}

fn history_head(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|head| head.target())
        .map(|oid| oid.to_string())
}

fn commit_tags_map(repo: &Repository) -> std::collections::HashMap<Oid, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    let Ok(tag_names) = repo.tag_names(None) else {
        return map;
    };
    for name in tag_names.iter().filter_map(|name| name.ok().flatten()) {
        let Ok(object) = repo.revparse_single(&format!("refs/tags/{name}")) else {
            continue;
        };
        let Ok(commit) = object.peel(ObjectType::Commit) else {
            continue;
        };
        map.entry(commit.id())
            .or_insert_with(Vec::new)
            .push(name.to_string());
    }
    for tags in map.values_mut() {
        tags.sort();
    }
    map
}

fn paged_commits(
    repo: &Repository,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let mut walk = repo.revwalk().map_err(|err| err.message().to_string())?;
    if walk.push_head().is_err() {
        return Ok(CommitPage::default());
    }

    let tags_map = commit_tags_map(repo);

    let after_oid = after
        .map(Oid::from_str)
        .transpose()
        .map_err(|err| err.message().to_string())?;
    let mut collecting = after_oid.is_none();
    let mut commits = Vec::new();
    for oid in walk.flatten() {
        if !collecting {
            if Some(oid) == after_oid {
                collecting = true;
            }
            continue;
        }

        if commits.len() == limit {
            return Ok(CommitPage {
                commits,
                has_more: true,
            });
        }

        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let tags = tags_map.get(&oid).cloned().unwrap_or_default();
        commits.push(commit_info(repo, oid, &commit, tags));
    }

    Ok(CommitPage {
        commits,
        has_more: false,
    })
}

fn paged_commit_search(
    repo: &Repository,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let mut walk = repo.revwalk().map_err(|err| err.message().to_string())?;
    if walk.push_head().is_err() {
        return Ok(CommitPage::default());
    }

    let needle = query.to_lowercase();
    let tags_map = commit_tags_map(repo);
    let after_oid = after
        .map(Oid::from_str)
        .transpose()
        .map_err(|err| err.message().to_string())?;
    let mut collecting = after_oid.is_none();
    let mut commits = Vec::new();

    for oid in walk.flatten() {
        if !collecting {
            if Some(oid) == after_oid {
                collecting = true;
            }
            continue;
        }

        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let tags = tags_map.get(&oid).cloned().unwrap_or_default();
        if !commit_matches_search(oid, &commit, &tags, &needle) {
            continue;
        }

        if commits.len() == limit {
            return Ok(CommitPage {
                commits,
                has_more: true,
            });
        }

        commits.push(commit_info(repo, oid, &commit, tags));
    }

    Ok(CommitPage {
        commits,
        has_more: false,
    })
}

fn commit_matches_search(
    oid: Oid,
    commit: &git2::Commit<'_>,
    tags: &[String],
    needle: &str,
) -> bool {
    let hash = oid.to_string();
    if hash.to_lowercase().contains(needle) || hash[..7].to_lowercase().contains(needle) {
        return true;
    }

    let message = commit.message().unwrap_or_default();
    let author = commit.author();
    let haystack = format!(
        "{}\n{}\n{}\n{}",
        message,
        author.name().unwrap_or_default(),
        author.email().unwrap_or_default(),
        tags.join("\n")
    );
    haystack.to_lowercase().contains(needle)
}

fn commit_info(
    repo: &Repository,
    oid: Oid,
    commit: &git2::Commit<'_>,
    tags: Vec<String>,
) -> Commit {
    let hash = oid.to_string();
    let (subject, comment) = commit_message_parts(commit.message().unwrap_or_default());
    let (insertions, deletions) = commit_line_stats(repo, commit).unwrap_or_default();
    Commit {
        hash: hash.clone(),
        short_hash: hash[..7].to_string(),
        subject: if subject.is_empty() {
            "Untitled commit".to_string()
        } else {
            subject
        },
        comment,
        author: commit
            .author()
            .name()
            .unwrap_or("Unknown author")
            .to_string(),
        author_email: commit.author().email().ok().map(ToString::to_string),
        relative_time: relative_time(commit.time().seconds()),
        insertions,
        deletions,
        tags,
    }
}

fn commit_line_stats(
    repo: &Repository,
    commit: &git2::Commit<'_>,
) -> Result<(usize, usize), String> {
    let new_tree = commit.tree().map_err(|err| err.message().to_string())?;
    let old_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .and_then(|parent| parent.tree())
                .map_err(|err| err.message().to_string())?,
        )
    };
    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
        .map_err(|err| err.message().to_string())?;
    let stats = diff.stats().map_err(|err| err.message().to_string())?;

    Ok((stats.insertions(), stats.deletions()))
}

fn relative_time(seconds: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(seconds);
    let elapsed = now.saturating_sub(seconds);

    match elapsed {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(elapsed / 60, "minute"),
        3_600..=86_399 => plural(elapsed / 3_600, "hour"),
        86_400..=2_592_000 => plural(elapsed / 86_400, "day"),
        _ => plural(elapsed / 2_592_000, "month"),
    }
}

fn plural(value: i64, unit: &str) -> String {
    if value == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{value} {unit}s ago")
    }
}

fn file_diff_rows_with_paths(
    repo: &Repository,
    file_path: &str,
) -> Result<DiffRowsWithFilePaths, String> {
    let mut options = DiffOptions::new();
    configure_diff_options_for_path(file_path, &mut options);

    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    let mut diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut options))
        .map_err(|err| err.message().to_string())?;
    diff.find_similar(Some(&mut DiffFindOptions::new()))
        .map_err(|err| err.message().to_string())?;

    diff_rows_and_paths(&diff, file_path)
}

fn commit_file_diff_rows_with_paths(
    repo: &Repository,
    old_tree: Option<&Tree<'_>>,
    new_tree: &Tree<'_>,
    file_path: &str,
) -> Result<DiffRowsWithFilePaths, String> {
    let mut options = DiffOptions::new();
    configure_diff_options_for_path(file_path, &mut options);
    let mut diff = repo
        .diff_tree_to_tree(old_tree, Some(new_tree), Some(&mut options))
        .map_err(|err| err.message().to_string())?;
    diff.find_similar(Some(&mut DiffFindOptions::new()))
        .map_err(|err| err.message().to_string())?;

    diff_rows_and_paths(&diff, file_path)
}

const MAX_BINARY_PREVIEW_BYTES: usize = 32 * 1024 * 1024;

pub fn bytes_comparison(path: &Path, file_path: &str) -> Result<BytesComparison, String> {
    let repo = open_repo(path)?;
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    let DiffRowsWithFilePaths { paths, .. } = file_diff_rows_with_paths(&repo, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);

    Ok(BytesComparison {
        before: tree_file_binary_bytes_opt(&repo, head_tree.as_ref(), old_path)?,
        after: workdir_binary_bytes(&repo, new_path)?,
    })
}

pub fn commit_bytes_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<BytesComparison, String> {
    let repo = open_repo(path)?;
    let (old_tree, new_tree) = commit_trees(&repo, hash)?;
    let DiffRowsWithFilePaths { paths, .. } =
        commit_file_diff_rows_with_paths(&repo, old_tree.as_ref(), &new_tree, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);

    Ok(BytesComparison {
        before: tree_file_binary_bytes_opt(&repo, old_tree.as_ref(), old_path)?,
        after: tree_file_binary_bytes_opt(&repo, Some(&new_tree), new_path)?,
    })
}

fn ensure_worktree_text_previewable(
    repo: &Repository,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    ensure_tree_text_previewable(repo, head_tree.as_ref(), old_path)?;
    ensure_workdir_text_previewable(repo, new_path)
}

fn ensure_commit_text_previewable(
    repo: &Repository,
    old_tree: Option<&Tree<'_>>,
    new_tree: &Tree<'_>,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    ensure_tree_text_previewable(repo, old_tree, old_path)?;
    ensure_tree_text_previewable(repo, Some(new_tree), new_path)
}

fn complete_diff_rows(
    rows: Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
) -> Vec<FileDiffRow> {
    if rows.is_empty() {
        return rows;
    }

    let mut complete = Vec::new();
    let mut next_left = 1;
    let mut next_right = 1;

    for row in rows {
        if let (Some(left_number), Some(right_number)) = (row.left_number, row.right_number) {
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                next_left,
                left_number,
                next_right,
                right_number,
            );
        }

        if let Some(number) = row.left_number {
            next_left = number.saturating_add(1);
        }
        if let Some(number) = row.right_number {
            next_right = number.saturating_add(1);
        }

        complete.push(row);
    }

    append_context_gap(
        &mut complete,
        left_lines,
        right_lines,
        next_left,
        left_lines.len().saturating_add(1),
        next_right,
        right_lines.len().saturating_add(1),
    );

    complete
}

fn append_context_gap(
    rows: &mut Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) {
    let count = left_end
        .saturating_sub(left_start)
        .min(right_end.saturating_sub(right_start));

    for offset in 0..count {
        let left_number = left_start + offset;
        let right_number = right_start + offset;
        let text = left_lines
            .get(left_number.saturating_sub(1))
            .or_else(|| right_lines.get(right_number.saturating_sub(1)))
            .cloned()
            .unwrap_or_default();

        rows.push(FileDiffRow {
            left_number: Some(left_number),
            right_number: Some(right_number),
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }
}

fn diff_rows_and_paths(
    diff: &git2::Diff<'_>,
    file_path: &str,
) -> Result<DiffRowsWithFilePaths, String> {
    let builder = RefCell::new(DiffRowsBuilder::default());
    let first_paths = RefCell::new(None::<FilePathPair>);
    let matched_paths = RefCell::new(None::<FilePathPair>);

    diff.foreach(
        &mut |delta, _progress| {
            let paths = file_path_pair_from_delta(&delta);

            if first_paths.borrow().is_none() {
                first_paths.replace(Some(paths.clone()));
            }
            if is_file_path_match(file_path, &paths) {
                matched_paths.replace(Some(paths));
            }

            true
        },
        None,
        Some(&mut |_delta, _hunk| {
            builder.borrow_mut().flush();
            true
        }),
        Some(&mut |_delta, _hunk, line| {
            builder.borrow_mut().push(
                line.origin_value(),
                line.old_lineno(),
                line.new_lineno(),
                line.content(),
            );
            true
        }),
    )
    .map_err(|err| err.message().to_string())?;

    let mut builder = builder.into_inner();
    builder.flush();

    let paths = matched_paths
        .into_inner()
        .or(first_paths.into_inner())
        .unwrap_or_default();

    if paths.old_path.is_some() && paths.new_path.is_some() && paths.old_path != paths.new_path {
        log::debug!(
            "resolved renamed diff path for {}: {} -> {}",
            file_path,
            paths.old_path.as_deref().unwrap_or("<missing>"),
            paths.new_path.as_deref().unwrap_or("<missing>")
        );
    }

    Ok(DiffRowsWithFilePaths {
        rows: builder.rows,
        paths,
    })
}

fn configure_diff_options_for_path(file_path: &str, options: &mut DiffOptions) {
    options
        .pathspec(file_path)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .context_lines(3)
        .interhunk_lines(0);
}

fn file_path_pair_from_delta(delta: &git2::DiffDelta<'_>) -> FilePathPair {
    FilePathPair {
        old_path: delta
            .old_file()
            .path()
            .map(|path| path.to_string_lossy().to_string()),
        new_path: delta
            .new_file()
            .path()
            .map(|path| path.to_string_lossy().to_string()),
    }
}

fn is_file_path_match(file_path: &str, paths: &FilePathPair) -> bool {
    paths
        .old_path
        .as_deref()
        .is_some_and(|old_path| old_path == file_path)
        || paths
            .new_path
            .as_deref()
            .is_some_and(|new_path| new_path == file_path)
}

fn head_file_lines(repo: &Repository, file_path: &str) -> Result<Vec<String>, String> {
    let Some(tree) = repo.head().ok().and_then(|head| head.peel_to_tree().ok()) else {
        return Ok(Vec::new());
    };

    tree_file_lines(repo, &tree, file_path)
}

fn workdir_file_lines(repo: &Repository, file_path: &str) -> Result<Vec<String>, String> {
    let bytes = match workdir_text_bytes(repo, file_path)? {
        Some(bytes) => bytes,
        None => return Ok(Vec::new()),
    };

    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(ToString::to_string)
        .collect())
}

fn tree_file_lines_opt(
    repo: &Repository,
    tree: Option<&Tree<'_>>,
    file_path: &str,
) -> Result<Vec<String>, String> {
    match tree {
        Some(tree) => tree_file_lines(repo, tree, file_path),
        None => Ok(Vec::new()),
    }
}

fn tree_file_lines(
    repo: &Repository,
    tree: &Tree<'_>,
    file_path: &str,
) -> Result<Vec<String>, String> {
    let Some(bytes) = tree_file_bytes(repo, tree, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(&bytes)?;

    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(ToString::to_string)
        .collect())
}

fn ensure_workdir_text_previewable(repo: &Repository, file_path: &str) -> Result<(), String> {
    let _ = workdir_text_bytes(repo, file_path)?;
    Ok(())
}

fn ensure_tree_text_previewable(
    repo: &Repository,
    tree: Option<&Tree<'_>>,
    file_path: &str,
) -> Result<(), String> {
    let Some(tree) = tree else {
        return Ok(());
    };
    let Some(bytes) = tree_file_bytes(repo, tree, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(());
    };
    ensure_blob_text_previewable(&bytes)
}

fn workdir_text_bytes(repo: &Repository, file_path: &str) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo)?.join(file_path);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };

    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.len() as usize > MAX_TEXT_PREVIEW_BYTES {
        return Err(format!(
            "{} is too large to preview as text.",
            file_name(file_path)
        ));
    }

    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    ensure_blob_text_previewable(&bytes)?;
    Ok(Some(bytes))
}

fn ensure_blob_text_previewable(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_TEXT_PREVIEW_BYTES {
        return Err("File is too large to preview as text.".to_string());
    }
    if is_binary_bytes(bytes) {
        return Err("Binary files cannot be previewed as text.".to_string());
    }
    Ok(())
}

fn workdir_binary_bytes(repo: &Repository, file_path: &str) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo)?.join(file_path);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };

    if metadata.is_dir() {
        return Ok(None);
    }
    if metadata.len() as usize > MAX_BINARY_PREVIEW_BYTES {
        return Err(format!("{} is too large to preview.", file_name(file_path)));
    }

    std::fs::read(path).map(Some).map_err(|err| err.to_string())
}

fn tree_file_binary_bytes_opt(
    repo: &Repository,
    tree: Option<&Tree<'_>>,
    file_path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(tree) = tree else {
        return Ok(None);
    };
    tree_file_bytes(repo, tree, file_path, MAX_BINARY_PREVIEW_BYTES)
}

fn tree_file_bytes(
    repo: &Repository,
    tree: &Tree<'_>,
    file_path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let entry = match tree.get_path(Path::new(file_path)) {
        Ok(entry) => entry,
        Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
        Err(err) => return Err(err.message().to_string()),
    };
    let object = entry
        .to_object(repo)
        .map_err(|err| err.message().to_string())?;
    let Some(blob) = object.as_blob() else {
        return Ok(None);
    };
    if blob.size() > max_bytes {
        return Err(format!("{} is too large to preview.", file_name(file_path)));
    }

    Ok(Some(blob.content().to_vec()))
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

fn commit_trees<'repo>(
    repo: &'repo Repository,
    hash: &str,
) -> Result<(Option<Tree<'repo>>, Tree<'repo>), String> {
    let oid = Oid::from_str(hash).map_err(|err| err.message().to_string())?;
    let commit = repo
        .find_commit(oid)
        .map_err(|err| err.message().to_string())?;
    let new_tree = commit.tree().map_err(|err| err.message().to_string())?;
    let old_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .and_then(|parent| parent.tree())
                .map_err(|err| err.message().to_string())?,
        )
    };

    Ok((old_tree, new_tree))
}

#[derive(Default)]
struct DiffRowsBuilder {
    rows: Vec<FileDiffRow>,
    deleted: Vec<PendingDiffLine>,
    added: Vec<PendingDiffLine>,
}

impl DiffRowsBuilder {
    fn push(
        &mut self,
        line_type: DiffLineType,
        old_number: Option<u32>,
        new_number: Option<u32>,
        content: &[u8],
    ) {
        match line_type {
            DiffLineType::Context | DiffLineType::ContextEOFNL => {
                self.flush();
                self.rows.push(FileDiffRow {
                    left_number: old_number.map(|number| number as usize),
                    right_number: new_number.map(|number| number as usize),
                    left_text: Some(diff_line_text(content)),
                    right_text: Some(diff_line_text(content)),
                    left_kind: DiffKind::Context,
                    right_kind: DiffKind::Context,
                });
            }
            DiffLineType::Deletion | DiffLineType::DeleteEOFNL => {
                self.deleted.push(PendingDiffLine {
                    number: old_number.map(|number| number as usize),
                    text: diff_line_text(content),
                });
            }
            DiffLineType::Addition | DiffLineType::AddEOFNL => {
                self.added.push(PendingDiffLine {
                    number: new_number.map(|number| number as usize),
                    text: diff_line_text(content),
                });
            }
            DiffLineType::FileHeader | DiffLineType::HunkHeader | DiffLineType::Binary => {
                self.flush();
            }
        }
    }

    fn flush(&mut self) {
        for index in 0..self.deleted.len().max(self.added.len()) {
            let deleted = self.deleted.get(index);
            let added = self.added.get(index);

            self.rows.push(FileDiffRow {
                left_number: deleted.and_then(|line| line.number),
                right_number: added.and_then(|line| line.number),
                left_text: deleted.map(|line| line.text.clone()),
                right_text: added.map(|line| line.text.clone()),
                left_kind: if deleted.is_some() {
                    DiffKind::Deleted
                } else {
                    DiffKind::Context
                },
                right_kind: if added.is_some() {
                    DiffKind::Added
                } else {
                    DiffKind::Context
                },
            });
        }

        self.deleted.clear();
        self.added.clear();
    }
}

struct PendingDiffLine {
    number: Option<usize>,
    text: String,
}

fn diff_line_text(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn run_git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn run_git_streaming_stderr(
    path: &Path,
    args: &[&str],
    mut progress: impl FnMut(&str),
) -> Result<String, String> {
    log::debug!(
        "running git command with streamed progress in {}: git {}",
        path.display(),
        args.join(" ")
    );

    let mut child = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture git output.".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture git progress.".to_string())?;

    let stdout_handle = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });

    let mut stderr_output = Vec::new();
    let mut pending_line = Vec::new();
    let mut buffer = [0u8; 1024];

    loop {
        let read = stderr
            .read(&mut buffer)
            .map_err(|err| format!("Failed to read git progress: {err}"))?;
        if read == 0 {
            break;
        }

        stderr_output.extend_from_slice(&buffer[..read]);
        for byte in &buffer[..read] {
            match *byte {
                b'\r' | b'\n' => {
                    emit_git_progress_line(&pending_line, &mut progress);
                    pending_line.clear();
                }
                byte => pending_line.push(byte),
            }
        }
    }
    emit_git_progress_line(&pending_line, &mut progress);

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    let stdout_output = stdout_handle
        .join()
        .map_err(|_| "Git output reader stopped unexpectedly.".to_string())?
        .map_err(|err| format!("Failed to read git output: {err}"))?;

    if status.success() {
        Ok(String::from_utf8_lossy(&stdout_output).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&stderr_output).trim().to_string();
        let stdout = String::from_utf8_lossy(&stdout_output).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn emit_git_progress_line(line: &[u8], progress: &mut impl FnMut(&str)) {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if !line.is_empty() {
        progress(line);
    }
}

fn git_fetch_progress_label(line: &str) -> Option<String> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").unwrap_or(line).trim();
    let stages = [
        ("Enumerating objects", "Enumerating objects"),
        ("Counting objects", "Counting objects"),
        ("Compressing objects", "Compressing objects"),
        ("Receiving objects", "Receiving objects"),
        ("Resolving deltas", "Resolving deltas"),
    ];

    for (prefix, label) in stages {
        if !line.starts_with(prefix) {
            continue;
        }

        if let Some(percent) = progress_percent(line) {
            return Some(format!("{label} {percent}%"));
        }
        if line.contains("done") {
            return Some(format!("{label} done"));
        }
        return Some(label.to_string());
    }

    None
}

fn progress_percent(line: &str) -> Option<String> {
    let percent_index = line.find('%')?;
    let digits = line[..percent_index]
        .chars()
        .rev()
        .skip_while(|ch| ch.is_ascii_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }

    Some(digits.chars().rev().collect())
}

fn run_git_owned(path: &Path, args: &[String]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn run_git_owned_with_commit_timezone(path: &Path, args: &[String]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(path);

    let local_git_config = load_local_workspace_config(path).git;
    let commit_timezone = local_git_config
        .commit_timezone
        .or_else(|| local_config_string(path, COMMIT_TIMEZONE_KEY));
    let use_system_timezone = local_git_config
        .use_system_timezone
        .unwrap_or_else(|| local_config_bool(path, USE_SYSTEM_TIMEZONE_KEY));
    let timezone = match commit_timezone {
        Some(timezone) => Some(normalize_timezone(&timezone)?),
        None if use_system_timezone => None,
        None => Some(DEFAULT_COMMIT_TIMEZONE.to_string()),
    };

    if let Some(timezone) = timezone {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| err.to_string())?
            .as_secs();
        let git_date = format!("@{seconds} {timezone}");
        command.env("GIT_AUTHOR_DATE", &git_date);
        command.env("GIT_COMMITTER_DATE", git_date);
        log::debug!("using commit timezone {timezone}");
    } else {
        log::debug!("using system timezone for commit");
    }

    let output = command
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn normalize_timezone(value: &str) -> Result<String, String> {
    let value = value.trim();
    let compact = if value.len() == 6 && value.as_bytes()[3] == b':' {
        format!("{}{}", &value[..3], &value[4..])
    } else {
        value.to_string()
    };

    let bytes = compact.as_bytes();
    if compact.len() != 5 || !matches!(bytes[0], b'+' | b'-') {
        return Err("Commit timezone must look like +0000, -0500, or +09:30.".to_string());
    }
    if !bytes[1..].iter().all(u8::is_ascii_digit) {
        return Err("Commit timezone must look like +0000, -0500, or +09:30.".to_string());
    }

    let hours: u8 = compact[1..3]
        .parse()
        .map_err(|_| "Commit timezone hours must be between 00 and 23.".to_string())?;
    let minutes: u8 = compact[3..5]
        .parse()
        .map_err(|_| "Commit timezone minutes must be between 00 and 59.".to_string())?;

    if hours > 23 || minutes > 59 {
        return Err("Commit timezone must use hours 00-23 and minutes 00-59.".to_string());
    }

    Ok(compact)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoMetadata {
    Fork,
    Private,
    Public,
    Folder,
}

pub fn get_repo_metadata_with(
    path: &Path,
    github_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::github::GitHubRepoMetadata>,
    gitlab_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::gitlab::GitLabRepoMetadata>,
    bitbucket_metadata: &dyn Fn(
        &str,
        Option<&str>,
        Option<&str>,
    ) -> Option<crate::bitbucket::BitbucketRepoMetadata>,
) -> RepoMetadata {
    let Ok(repo) = open_repo(path) else {
        return RepoMetadata::Folder;
    };

    if repo.find_remote("upstream").is_ok() {
        return RepoMetadata::Fork;
    }

    let remote_name = upstream_remote(&repo)
        .or_else(|| Some("origin".to_string()).filter(|remote| repo.find_remote(remote).is_ok()));

    if let Some(name) = remote_name {
        if let Ok(remote) = repo.find_remote(&name) {
            if let Ok(url) = remote.url() {
                if let Some(slug) = crate::github::parse_github_url(url)
                    && let Some(metadata) = github_metadata(&slug, Some(&name), Some(url))
                {
                    return match metadata {
                        crate::github::GitHubRepoMetadata::Fork => RepoMetadata::Fork,
                        crate::github::GitHubRepoMetadata::Private => RepoMetadata::Private,
                        crate::github::GitHubRepoMetadata::Public => RepoMetadata::Public,
                    };
                }
                if let Some(slug) = crate::gitlab::parse_gitlab_url(url)
                    && let Some(metadata) = gitlab_metadata(&slug, Some(&name), Some(url))
                {
                    return match metadata {
                        crate::gitlab::GitLabRepoMetadata::Fork => RepoMetadata::Fork,
                        crate::gitlab::GitLabRepoMetadata::Private => RepoMetadata::Private,
                        crate::gitlab::GitLabRepoMetadata::Public => RepoMetadata::Public,
                    };
                }
                if let Some(slug) = crate::bitbucket::parse_bitbucket_url(url)
                    && let Some(metadata) = bitbucket_metadata(&slug, Some(&name), Some(url))
                {
                    return match metadata {
                        crate::bitbucket::BitbucketRepoMetadata::Fork => RepoMetadata::Fork,
                        crate::bitbucket::BitbucketRepoMetadata::Private => RepoMetadata::Private,
                        crate::bitbucket::BitbucketRepoMetadata::Public => RepoMetadata::Public,
                    };
                }
            }
        }
    }

    RepoMetadata::Private
}

fn remote_owner_from_remote_url(remote_url: &str) -> Option<String> {
    parse_repo_slug_from_remote_url(remote_url)
        .and_then(|slug| slug.split('/').next().map(str::to_string))
}

fn parse_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    github::parse_github_url(remote_url)
        .or_else(|| gitlab::parse_gitlab_url(remote_url))
        .or_else(|| bitbucket::parse_bitbucket_url(remote_url))
}
