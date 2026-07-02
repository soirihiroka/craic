use crate::{bitbucket, github, gitlab};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub use crate::workspace_config::QuickActionConfig;

const COMMIT_TIMEZONE_KEY: &str = "craic.commitTimezone";
const USE_SYSTEM_TIMEZONE_KEY: &str = "craic.useSystemTimezone";
const SHOW_REMOTE_OWNER_WARNING_KEY: &str = "craic.showRemoteOwnerWarning";
const DEFAULT_COMMIT_TIMEZONE: &str = "+0000";
pub(crate) const MAX_TEXT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

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

#[derive(Clone, Debug)]
pub struct FileComparison {
    pub rows: Vec<FileDiffRow>,
    pub fingerprint: u64,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FilePathPair {
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct GitStatusEntry {
    status_code: String,
    path: String,
    old_path: Option<String>,
    unmerged: bool,
    untracked: bool,
}

#[derive(Clone, Debug)]
pub struct BytesComparison {
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub fingerprint: u64,
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

impl FileComparison {
    fn from_rows(rows: Vec<FileDiffRow>) -> Self {
        let fingerprint = fingerprint_diff_rows(&rows);
        let insertions = rows
            .iter()
            .filter(|row| row.right_kind == DiffKind::Added)
            .count();
        let deletions = rows
            .iter()
            .filter(|row| row.left_kind == DiffKind::Deleted)
            .count();
        Self {
            rows,
            fingerprint,
            insertions,
            deletions,
        }
    }
}

impl BytesComparison {
    pub(crate) fn from_parts(before: Option<Vec<u8>>, after: Option<Vec<u8>>) -> Self {
        let fingerprint = fingerprint_optional_bytes(before.as_deref(), after.as_deref());
        Self {
            before,
            after,
            fingerprint,
        }
    }
}

fn fingerprint_diff_rows(rows: &[FileDiffRow]) -> u64 {
    let mut fingerprint = 0xcbf29ce484222325;
    hash_usize(&mut fingerprint, rows.len());
    for row in rows {
        hash_option_usize(&mut fingerprint, row.left_number);
        hash_option_usize(&mut fingerprint, row.right_number);
        hash_kind(&mut fingerprint, row.left_kind);
        hash_kind(&mut fingerprint, row.right_kind);
        hash_option_text(&mut fingerprint, row.left_text.as_deref());
        hash_option_text(&mut fingerprint, row.right_text.as_deref());
    }
    fingerprint
}

fn fingerprint_optional_bytes(before: Option<&[u8]>, after: Option<&[u8]>) -> u64 {
    let mut fingerprint = 0xcbf29ce484222325;
    hash_optional_bytes(&mut fingerprint, before);
    hash_optional_bytes(&mut fingerprint, after);
    fingerprint
}

fn hash_option_usize(fingerprint: &mut u64, value: Option<usize>) {
    match value {
        Some(value) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, value);
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_kind(fingerprint: &mut u64, kind: DiffKind) {
    hash_u8(
        fingerprint,
        match kind {
            DiffKind::Context => 0,
            DiffKind::Deleted => 1,
            DiffKind::Added => 2,
            DiffKind::Fold => 3,
        },
    );
}

fn hash_option_text(fingerprint: &mut u64, text: Option<&str>) {
    match text {
        Some(text) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, text.len());
            hash_bytes(fingerprint, text.as_bytes());
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_optional_bytes(fingerprint: &mut u64, bytes: Option<&[u8]>) {
    match bytes {
        Some(bytes) => {
            hash_u8(fingerprint, 1);
            hash_usize(fingerprint, bytes.len());
            hash_bytes(fingerprint, bytes);
        }
        None => hash_u8(fingerprint, 0),
    }
}

fn hash_usize(fingerprint: &mut u64, value: usize) {
    hash_bytes(fingerprint, &value.to_le_bytes());
}

fn hash_u8(fingerprint: &mut u64, value: u8) {
    hash_bytes(fingerprint, &[value]);
}

fn hash_bytes(fingerprint: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *fingerprint ^= u64::from(*byte);
        *fingerprint = fingerprint.wrapping_mul(0x100000001b3);
    }
}

pub fn snapshot(path: &Path) -> Result<RepositorySnapshot, String> {
    let mut timing = GitSnapshotTimer::new(path);
    let root = repo_root(path)?;
    timing.mark("repo-root");

    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Repository")
        .to_string();
    let branch = current_branch(&root)?;
    timing.mark("current-branch");

    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    });
    let remote_url = remote_name
        .as_deref()
        .and_then(|remote| remote_url(&root, remote));
    let remote_owner = remote_url.as_deref().and_then(remote_owner_from_remote_url);
    timing.mark("remote-metadata");

    let (ahead, behind, has_upstream) = ahead_behind_count(&root);
    timing.mark("ahead-behind");

    let last_fetch_at = last_fetch_at(&root);
    let user_name = config_string(&root, "user.name");
    let user_email = config_string(&root, "user.email");
    let github_avatar_url = user_email
        .as_deref()
        .and_then(github::login_from_noreply_email)
        .map(|login| github::avatar_url_for_login(&login));
    timing.mark("user-metadata");

    let branches = branches(&root, remote_name.as_deref())?;
    timing.mark("branches");

    let warn_if_remote_owner_mismatch =
        local_config_bool_with_default(path, SHOW_REMOTE_OWNER_WARNING_KEY, true);
    timing.mark("local-settings");

    let changed_files = changed_files(&root)?;
    timing.mark("changed-files");

    let history_head = history_head(&root);
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

    let plan = commit_target_plan(path, files)?;
    if plan.force_remove_paths.is_empty() && plan.update_paths.is_empty() {
        return Err("Select at least one file to commit.".to_string());
    }
    reset_index_for_selected_commit(path)?;
    stage_commit_plan(path, &plan)?;

    run_git_owned_with_commit_timezone_and_stdin(
        path,
        &["commit".to_string(), "-F".to_string(), "-".to_string()],
        &commit_message_stdin(summary, description),
    )
}

struct CommitTargetPlan {
    force_remove_paths: Vec<String>,
    update_paths: Vec<String>,
}

fn commit_target_plan(path: &Path, selected_files: &[String]) -> Result<CommitTargetPlan, String> {
    let entries = status_entries(path)?;
    let mut force_remove_paths = Vec::<String>::new();
    let mut update_paths = Vec::<String>::new();
    let mut seen_force_remove_paths = HashSet::new();
    let mut seen_update_paths = HashSet::new();

    for requested in selected_files {
        let mut resolved = false;

        for entry in &entries {
            if !porcelain_entry_matches_path(entry, requested) {
                continue;
            }

            push_commit_target_paths(
                &mut force_remove_paths,
                &mut seen_force_remove_paths,
                porcelain_entry_force_remove_paths(entry),
            );
            push_commit_target_paths(
                &mut update_paths,
                &mut seen_update_paths,
                porcelain_entry_update_paths(entry),
            );

            resolved = true;
            break;
        }

        if !resolved {
            if seen_update_paths.insert(requested.clone()) {
                update_paths.push(requested.clone());
            }
        }
    }

    log::debug!(
        "resolved git commit targets selected_count={} force_remove_count={} update_count={}",
        selected_files.len(),
        force_remove_paths.len(),
        update_paths.len()
    );

    Ok(CommitTargetPlan {
        force_remove_paths,
        update_paths,
    })
}

fn push_commit_target_paths(
    target: &mut Vec<String>,
    seen: &mut HashSet<String>,
    paths: Vec<String>,
) {
    for path in paths {
        if seen.insert(path.clone()) {
            target.push(path);
        }
    }
}

fn reset_index_for_selected_commit(path: &Path) -> Result<(), String> {
    if run_git(path, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        run_git(path, &["reset", "--", "."]).map(|_| ())
    } else {
        run_git(path, &["rm", "--cached", "-r", "--ignore-unmatch", "."]).map(|_| ())
    }
}

fn stage_commit_plan(path: &Path, plan: &CommitTargetPlan) -> Result<(), String> {
    if !plan.force_remove_paths.is_empty() {
        update_index_paths(path, &["--force-remove"], &plan.force_remove_paths)?;
    }
    if !plan.update_paths.is_empty() {
        update_index_paths(
            path,
            &["--add", "--remove", "--replace"],
            &plan.update_paths,
        )?;
    }
    Ok(())
}

fn update_index_paths(path: &Path, options: &[&str], paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut args = vec!["update-index".to_string()];
    args.extend(options.iter().map(|option| option.to_string()));
    args.push("-z".to_string());
    args.push("--stdin".to_string());

    let mut stdin = Vec::new();
    for path in paths {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }

    run_git_owned_with_stdin(path, &args, &stdin).map(|_| ())
}

fn commit_message_stdin(summary: &str, description: &str) -> Vec<u8> {
    let mut message = summary.trim().to_string();
    let description = description.trim();
    if !description.is_empty() {
        message.push_str("\n\n");
        message.push_str(description);
    }
    message.push('\n');
    message.into_bytes()
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
    let local_git_config = crate::workspace_config::git_config(path);

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
        github_auth_account: local_git_config.github_auth_account,
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

    crate::workspace_config::save_git_config(
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
    crate::workspace_config::quick_action_config(path)
}

pub fn save_quick_action_config(
    path: &Path,
    actions: Vec<QuickActionConfig>,
) -> Result<(), String> {
    crate::workspace_config::save_quick_action_config(path, actions)
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
    let mut files = status_entries(repo_path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.unmerged || entry.status_code.contains('U'))
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
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
    repo_root(path)
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
    let root = repo_root(path).ok()?;
    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    })?;
    remote_url(&root, &remote_name).and_then(|url| parse_repo_slug_from_remote_url(&url))
}

pub fn remote_commit_web_url(remote_url: &str, hash: &str) -> String {
    format!("{}/commit/{hash}", remote_web_url(remote_url))
}

pub fn comparison(path: &Path, file_path: &str) -> Result<FileComparison, String> {
    let start = Instant::now();
    let paths = worktree_file_path_pair(path, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_worktree_text_previewable(path, old_path, new_path)?;

    let diff = worktree_diff(path, &paths, file_path)?;
    let rows = complete_diff_rows(
        parse_unified_diff(&diff),
        &head_file_lines(path, old_path)?,
        &workdir_file_lines(path, new_path)?,
        paths_changed(&paths),
    );
    log::info!(
        "git worktree comparison complete path={} rows={} elapsed_ms={}",
        file_path,
        rows.len(),
        start.elapsed().as_millis()
    );

    Ok(FileComparison::from_rows(rows))
}

pub fn commit_details(path: &Path, hash: &str) -> Result<Commit, String> {
    let output = run_git(
        path,
        &[
            "show",
            "-s",
            "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ct%x1f%B",
            hash,
        ],
    )?;
    let tags = tags_for_commit(path, hash).unwrap_or_default();
    let (insertions, deletions) = commit_line_stats(path, hash).unwrap_or_default();
    parse_commit_details(&output, tags, insertions, deletions)
}

pub fn commit_message(path: &Path, hash: &str) -> Result<CommitMessage, String> {
    let message = run_git(path, &["show", "-s", "--format=%B", hash])?;
    let (summary, description) = commit_message_parts(&message);

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
    let output = run_git(path, &["rev-list", "--parents", "-n", "1", hash])?;
    Ok(output.split_whitespace().nth(1).map(ToString::to_string))
}

pub fn tags_for_commit(path: &Path, hash: &str) -> Result<Vec<String>, String> {
    let mut tags = run_git(path, &["tag", "--points-at", hash])?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    tags.sort();
    Ok(tags)
}

pub fn commit_page(path: &Path, after: Option<&str>, limit: usize) -> Result<CommitPage, String> {
    paged_commits(path, after, limit)
}

pub fn commit_search_page(
    path: &Path,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    if query.trim().is_empty() {
        return paged_commits(path, after, limit);
    }
    paged_commit_search(path, query, after, limit)
}

pub fn commit_changed_files(path: &Path, hash: &str) -> Result<Vec<ChangedFile>, String> {
    let output = run_git_bytes(
        path,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "-z",
            hash,
        ],
    )?;
    let mut files = parse_name_status_files_z(&output);

    sort_changed_files(&mut files);
    Ok(files)
}

pub fn commit_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<FileComparison, String> {
    let start = Instant::now();
    let paths = commit_file_path_pair(path, hash, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    ensure_commit_text_previewable(path, hash, old_path, new_path)?;
    let parent = commit_parent_hash(path, hash)?;
    let diff = commit_diff(path, hash, &paths, file_path)?;
    let rows = complete_diff_rows(
        parse_unified_diff(&diff),
        &tree_file_lines_opt(path, parent.as_deref(), old_path)?,
        &tree_file_lines(path, hash, new_path)?,
        paths_changed(&paths),
    );
    log::info!(
        "git commit comparison complete hash={} path={} rows={} elapsed_ms={}",
        short_hash(hash),
        file_path,
        rows.len(),
        start.elapsed().as_millis()
    );

    Ok(FileComparison::from_rows(rows))
}

fn repo_root(path: &Path) -> Result<PathBuf, String> {
    let root = run_git(path, &["rev-parse", "--show-toplevel"])?;
    if root.is_empty() {
        return Err("Bare repositories are not supported.".to_string());
    }
    Ok(PathBuf::from(root))
}

fn git_dir(path: &Path) -> Option<PathBuf> {
    let output = run_git(path, &["rev-parse", "--absolute-git-dir"]).ok()?;
    (!output.is_empty()).then(|| PathBuf::from(output))
}

fn current_branch(root: &Path) -> Result<String, String> {
    if let Ok(branch) = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        if !branch.is_empty() && branch != "HEAD" {
            return Ok(branch);
        }
    }

    if let Ok(hash) = run_git(root, &["rev-parse", "--short", "HEAD"]) {
        if !hash.is_empty() {
            return Ok(hash);
        }
    }

    Ok(local_config_string(root, "init.defaultBranch").unwrap_or_else(|| "main".to_string()))
}

fn branches(root: &Path, remote_name: Option<&str>) -> Result<Vec<BranchInfo>, String> {
    let current = current_branch(root).unwrap_or_default();
    let mut branches = Vec::new();
    let output = run_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)%00%(refname)%00%(upstream:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;

    for line in output.lines() {
        let mut fields = line.split('\0');
        let Some(name) = fields.next().filter(|name| !name.is_empty()) else {
            continue;
        };
        let refname = fields.next().unwrap_or_default();
        let upstream = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        if name.ends_with("/HEAD") {
            continue;
        }
        let kind = if refname.starts_with("refs/remotes/") {
            BranchKind::Remote
        } else {
            BranchKind::Local
        };

        branches.push(BranchInfo {
            name: name.to_string(),
            is_current: kind == BranchKind::Local && name == current,
            kind,
            upstream,
            is_default: false,
            is_recent: false,
        });
    }

    let default_name = default_branch_name(root, remote_name);
    let remote_ref = remote_name
        .and_then(|remote| remote_head(root, remote).map(|head| format!("{remote}/{head}")));
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

fn default_branch_name(root: &Path, remote_name: Option<&str>) -> String {
    remote_name
        .and_then(|remote| remote_head(root, remote))
        .or_else(|| local_config_string(root, "init.defaultBranch"))
        .unwrap_or_else(|| "main".to_string())
}

fn remote_head(root: &Path, remote_name: &str) -> Option<String> {
    let target = run_git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            &format!("refs/remotes/{remote_name}/HEAD"),
        ],
    )
    .ok()?;
    target
        .strip_prefix(&format!("{remote_name}/"))
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

fn upstream_remote(root: &Path) -> Option<String> {
    run_git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()
    .and_then(|upstream| upstream.split('/').next().map(ToString::to_string))
    .filter(|remote| !remote.is_empty())
}

fn remote_url(root: &Path, remote_name: &str) -> Option<String> {
    run_git(root, &["remote", "get-url", remote_name])
        .ok()
        .filter(|url| !url.is_empty())
}

fn config_string(path: &Path, key: &str) -> Option<String> {
    run_git(path, &["config", "--get", key])
        .ok()
        .filter(|value| !value.is_empty())
}

fn ahead_behind_count(root: &Path) -> (u32, u32, bool) {
    let Ok(out) = run_git(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
            "--",
        ],
    ) else {
        return (0, 0, false);
    };
    let mut parts = out.split_whitespace();
    let ahead = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let behind = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    (ahead, behind, true)
}

fn last_fetch_at(root: &Path) -> Option<SystemTime> {
    git_dir(root)
        .and_then(|dir| std::fs::metadata(dir.join("FETCH_HEAD")).ok())
        .and_then(|metadata| metadata.modified().ok())
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

fn switch_github_auth_for_workspace(path: &Path) -> Result<(), String> {
    let Some(account) = crate::workspace_config::github_auth_account(path) else {
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

fn changed_files(root: &Path) -> Result<Vec<ChangedFile>, String> {
    let mut files = status_entries(root)?
        .iter()
        .filter(|entry| status_entry_visible(entry))
        .map(|entry| {
            let mut file = changed_file_from_porcelain_entry(entry);
            if file.status == "M" && deletion_only_change(root, &file.path) {
                file.status = "M-".to_string();
            }
            file.worktree_signature = changed_file_worktree_signature(root, &file.path);
            file
        })
        .collect::<Vec<_>>();

    sort_changed_files(&mut files);
    Ok(files)
}

fn status_entries(path: &Path) -> Result<Vec<GitStatusEntry>, String> {
    let output = run_git_bytes(
        path,
        &[
            "--no-optional-locks",
            "status",
            "--untracked-files=all",
            "--branch",
            "--porcelain=2",
            "-z",
        ],
    )?;
    Ok(parse_porcelain_status_entries(&output))
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

pub(crate) fn parse_porcelain_status_entries(bytes: &[u8]) -> Vec<GitStatusEntry> {
    let tokens = bytes
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).to_string())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let field = &tokens[index];
        index += 1;

        match field.as_bytes().first().copied() {
            Some(b'1') => {
                if let Some(entry) = parse_porcelain_changed_entry(field) {
                    entries.push(entry);
                }
            }
            Some(b'2') => {
                let old_path = tokens.get(index).cloned();
                index += usize::from(old_path.is_some());
                if let Some(entry) = parse_porcelain_renamed_entry(field, old_path) {
                    entries.push(entry);
                }
            }
            Some(b'u') => {
                if let Some(entry) = parse_porcelain_unmerged_entry(field) {
                    entries.push(entry);
                }
            }
            Some(b'?') => {
                if let Some(path) = field.strip_prefix("? ").filter(|path| !path.is_empty()) {
                    entries.push(GitStatusEntry {
                        status_code: "??".to_string(),
                        path: path.to_string(),
                        old_path: None,
                        unmerged: false,
                        untracked: true,
                    });
                }
            }
            _ => {}
        }
    }

    entries
}

fn parse_porcelain_changed_entry(field: &str) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(9, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..6 {
        parts.next()?;
    }
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path: None,
        unmerged: false,
        untracked: false,
    })
}

fn parse_porcelain_renamed_entry(field: &str, old_path: Option<String>) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(10, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..6 {
        parts.next()?;
    }
    parts.next()?;
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path,
        unmerged: false,
        untracked: false,
    })
}

fn parse_porcelain_unmerged_entry(field: &str) -> Option<GitStatusEntry> {
    let mut parts = field.splitn(11, ' ');
    parts.next()?;
    let status_code = parts.next()?.to_string();
    for _ in 0..8 {
        parts.next()?;
    }
    let path = parts.next()?.to_string();
    Some(GitStatusEntry {
        status_code,
        path,
        old_path: None,
        unmerged: true,
        untracked: false,
    })
}

pub(crate) fn status_entry_visible(entry: &GitStatusEntry) -> bool {
    entry.status_code != "AD"
}

pub(crate) fn changed_file_from_porcelain_entry(entry: &GitStatusEntry) -> ChangedFile {
    ChangedFile {
        status: porcelain_entry_status_label(entry).to_string(),
        path: entry.path.clone(),
        git_status_bits: 0,
        worktree_signature: None,
    }
}

fn porcelain_entry_status_label(entry: &GitStatusEntry) -> &'static str {
    if entry.unmerged || entry.status_code.contains('U') {
        "U"
    } else if entry.status_code.contains('R') || entry.status_code.contains('C') {
        "R"
    } else if entry.status_code.contains('D') {
        "D"
    } else if entry.untracked || entry.status_code.contains('A') || entry.status_code.contains('?')
    {
        "A"
    } else {
        "M"
    }
}

pub(crate) fn porcelain_entry_matches_path(entry: &GitStatusEntry, path: &str) -> bool {
    entry.path == path || entry.old_path.as_deref() == Some(path)
}

pub(crate) fn porcelain_entry_force_remove_paths(entry: &GitStatusEntry) -> Vec<String> {
    if entry.old_path.is_some() || entry.status_code.contains('D') {
        return vec![entry.old_path.as_ref().unwrap_or(&entry.path).clone()];
    }
    Vec::new()
}

pub(crate) fn porcelain_entry_update_paths(entry: &GitStatusEntry) -> Vec<String> {
    if entry.status_code.contains('D') && entry.old_path.is_none() {
        return Vec::new();
    }
    vec![entry.path.clone()]
}

fn history_head(root: &Path) -> Option<String> {
    run_git(root, &["rev-parse", "HEAD"]).ok()
}

fn paged_commits(path: &Path, after: Option<&str>, limit: usize) -> Result<CommitPage, String> {
    let hashes = rev_list_hashes(path, after, limit)?;
    commits_from_hashes(path, hashes, limit, |_| true)
}

fn paged_commit_search(
    path: &Path,
    query: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<CommitPage, String> {
    let needle = query.to_lowercase();
    let hashes = rev_list_hashes(path, after, usize::MAX)?;
    commits_from_hashes(path, hashes, limit, |commit| {
        commit_search_text(commit).to_lowercase().contains(&needle)
    })
}

fn rev_list_hashes(path: &Path, after: Option<&str>, limit: usize) -> Result<Vec<String>, String> {
    let output = run_git(path, &["rev-list", "HEAD"])?;
    let mut hashes = Vec::new();
    let mut collecting = after.is_none();
    let fetch_limit = limit.saturating_add(1);

    for hash in output
        .lines()
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
    {
        if !collecting {
            if Some(hash) == after {
                collecting = true;
            }
            continue;
        }

        hashes.push(hash.to_string());
        if hashes.len() >= fetch_limit {
            break;
        }
    }

    Ok(hashes)
}

fn commits_from_hashes(
    path: &Path,
    hashes: Vec<String>,
    limit: usize,
    mut include: impl FnMut(&Commit) -> bool,
) -> Result<CommitPage, String> {
    let mut commits = Vec::new();
    let mut has_more = false;

    for hash in hashes {
        let commit = commit_details(path, &hash)?;
        if !include(&commit) {
            continue;
        }
        if commits.len() == limit {
            has_more = true;
            break;
        }
        commits.push(commit);
    }

    Ok(CommitPage { commits, has_more })
}

fn commit_search_text(commit: &Commit) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        commit.hash,
        commit.short_hash,
        commit.subject,
        commit.comment,
        commit.author,
        commit.author_email.as_deref().unwrap_or_default()
    )
}

fn parse_commit_details(
    output: &str,
    tags: Vec<String>,
    insertions: usize,
    deletions: usize,
) -> Result<Commit, String> {
    let mut parts = output.splitn(6, '\x1f');
    let hash = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Commit details did not include a hash.".to_string())?
        .trim()
        .to_string();
    let short_hash = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| short_hash(&hash).to_string());
    let author = parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "Unknown author".to_string());
    let author_email = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let timestamp = parts
        .next()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let message = parts.next().unwrap_or_default();
    let (subject, comment) = commit_message_parts(message);

    Ok(Commit {
        hash,
        short_hash,
        subject: if subject.is_empty() {
            "Untitled commit".to_string()
        } else {
            subject
        },
        comment,
        author,
        author_email,
        relative_time: relative_time(timestamp),
        insertions,
        deletions,
        tags,
    })
}

fn commit_line_stats(path: &Path, hash: &str) -> Result<(usize, usize), String> {
    let output = run_git(path, &["show", "--numstat", "--format=", hash])?;
    Ok(numstat_totals(&output))
}

fn numstat_totals(output: &str) -> (usize, usize) {
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    for line in output.lines() {
        let mut fields = line.split('\t');
        let Some(added) = fields.next() else {
            continue;
        };
        let Some(deleted) = fields.next() else {
            continue;
        };
        insertions += added.parse::<usize>().unwrap_or(0);
        deletions += deleted.parse::<usize>().unwrap_or(0);
    }
    (insertions, deletions)
}

fn parse_name_status_files_z(bytes: &[u8]) -> Vec<ChangedFile> {
    parse_name_status_entries_z(bytes)
        .into_iter()
        .map(|entry| ChangedFile {
            status: name_status_label(&entry.status).to_string(),
            path: entry.new_path.or(entry.old_path).unwrap_or_default(),
            git_status_bits: 0,
            worktree_signature: None,
        })
        .filter(|file| !file.path.is_empty())
        .collect()
}

fn parse_name_status_path_pairs_z(bytes: &[u8]) -> Vec<FilePathPair> {
    parse_name_status_entries_z(bytes)
        .into_iter()
        .map(|entry| FilePathPair {
            old_path: entry.old_path,
            new_path: entry.new_path,
        })
        .collect()
}

struct NameStatusEntry {
    status: String,
    old_path: Option<String>,
    new_path: Option<String>,
}

fn parse_name_status_entries_z(bytes: &[u8]) -> Vec<NameStatusEntry> {
    let tokens = bytes
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).to_string())
        .collect::<Vec<_>>();
    let mut entries = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let status = tokens[index].clone();
        index += 1;
        let Some(kind) = status.chars().next() else {
            continue;
        };

        match kind {
            'R' | 'C' => {
                let old_path = tokens.get(index).cloned();
                let new_path = tokens.get(index + 1).cloned();
                index += usize::from(old_path.is_some()) + usize::from(new_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path,
                    new_path,
                });
            }
            'A' => {
                let new_path = tokens.get(index).cloned();
                index += usize::from(new_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path: None,
                    new_path,
                });
            }
            'D' => {
                let old_path = tokens.get(index).cloned();
                index += usize::from(old_path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path,
                    new_path: None,
                });
            }
            _ => {
                let path = tokens.get(index).cloned();
                index += usize::from(path.is_some());
                entries.push(NameStatusEntry {
                    status,
                    old_path: path.clone(),
                    new_path: path,
                });
            }
        }
    }

    entries
}

fn name_status_label(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "A",
        Some('D') => "D",
        Some('R') | Some('C') => "R",
        Some('U') => "U",
        _ => "M",
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
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

pub(crate) const MAX_BINARY_PREVIEW_BYTES: usize = 32 * 1024 * 1024;

pub fn bytes_comparison(path: &Path, file_path: &str) -> Result<BytesComparison, String> {
    let paths = worktree_file_path_pair(path, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);

    Ok(BytesComparison::from_parts(
        tree_file_binary_bytes_opt(path, Some("HEAD"), old_path)?,
        workdir_binary_bytes(path, new_path)?,
    ))
}

pub fn commit_bytes_comparison(
    path: &Path,
    hash: &str,
    file_path: &str,
) -> Result<BytesComparison, String> {
    let paths = commit_file_path_pair(path, hash, file_path)?;
    let old_path = paths.old_path.as_deref().unwrap_or(file_path);
    let new_path = paths.new_path.as_deref().unwrap_or(file_path);
    let parent = commit_parent_hash(path, hash)?;

    Ok(BytesComparison::from_parts(
        tree_file_binary_bytes_opt(path, parent.as_deref(), old_path)?,
        tree_file_binary_bytes_opt(path, Some(hash), new_path)?,
    ))
}

fn ensure_worktree_text_previewable(
    repo_path: &Path,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    ensure_tree_text_previewable(repo_path, Some("HEAD"), old_path)?;
    ensure_workdir_text_previewable(repo_path, new_path)
}

fn ensure_commit_text_previewable(
    repo_path: &Path,
    hash: &str,
    old_path: &str,
    new_path: &str,
) -> Result<(), String> {
    let parent = commit_parent_hash(repo_path, hash)?;
    ensure_tree_text_previewable(repo_path, parent.as_deref(), old_path)?;
    ensure_tree_text_previewable(repo_path, Some(hash), new_path)
}

fn complete_diff_rows(
    rows: Vec<FileDiffRow>,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> Vec<FileDiffRow> {
    if rows.is_empty() {
        if complete_empty {
            let mut complete = Vec::new();
            append_context_gap(
                &mut complete,
                left_lines,
                right_lines,
                1,
                left_lines.len().saturating_add(1),
                1,
                right_lines.len().saturating_add(1),
            );
            return complete;
        }
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

pub(crate) fn paths_changed(paths: &FilePathPair) -> bool {
    paths.old_path.is_some() && paths.new_path.is_some() && paths.old_path != paths.new_path
}

fn worktree_file_path_pair(path: &Path, file_path: &str) -> Result<FilePathPair, String> {
    let entries = status_entries(path)?;
    Ok(worktree_file_path_pair_from_entries(&entries, file_path))
}

pub(crate) fn worktree_file_path_pair_from_entries(
    entries: &[GitStatusEntry],
    file_path: &str,
) -> FilePathPair {
    entries
        .iter()
        .find(|entry| porcelain_entry_matches_path(entry, file_path))
        .map(|entry| FilePathPair {
            old_path: if entry.untracked {
                None
            } else {
                entry.old_path.clone().or_else(|| Some(entry.path.clone()))
            },
            new_path: Some(entry.path.clone()),
        })
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        })
}

fn commit_file_path_pair(path: &Path, hash: &str, file_path: &str) -> Result<FilePathPair, String> {
    Ok(commit_name_status_entries(path, hash)?
        .into_iter()
        .find(|paths| is_file_path_match(file_path, paths))
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        }))
}

fn commit_name_status_entries(path: &Path, hash: &str) -> Result<Vec<FilePathPair>, String> {
    let output = run_git_bytes(
        path,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-M",
            "-z",
            hash,
        ],
    )?;
    Ok(parse_name_status_path_pairs_z(&output))
}

pub(crate) fn commit_file_path_pair_from_name_status_bytes(
    bytes: &[u8],
    file_path: &str,
) -> FilePathPair {
    parse_name_status_path_pairs_z(bytes)
        .into_iter()
        .find(|paths| is_file_path_match(file_path, paths))
        .unwrap_or_else(|| FilePathPair {
            old_path: Some(file_path.to_string()),
            new_path: Some(file_path.to_string()),
        })
}

fn worktree_diff(path: &Path, paths: &FilePathPair, fallback_path: &str) -> Result<String, String> {
    if paths.old_path.is_none()
        && let Some(new_path) = paths.new_path.as_deref()
    {
        let root = repo_root(path)?;
        return run_git_owned_with_success_codes(
            path,
            &[
                "diff".to_string(),
                "--no-index".to_string(),
                "--no-ext-diff".to_string(),
                "--no-color".to_string(),
                "--unified=3".to_string(),
                "--".to_string(),
                "/dev/null".to_string(),
                root.join(new_path).display().to_string(),
            ],
            &[0, 1],
        );
    }

    let mut args = vec![
        "diff".to_string(),
        "HEAD".to_string(),
        "--no-ext-diff".to_string(),
        "--find-renames".to_string(),
        "--no-color".to_string(),
        "--unified=3".to_string(),
    ];
    args.extend(diff_args_for_paths(&[
        paths.old_path.as_deref(),
        paths.new_path.as_deref(),
        Some(fallback_path),
    ]));
    run_git_owned(path, &args)
}

fn commit_diff(
    path: &Path,
    hash: &str,
    paths: &FilePathPair,
    fallback_path: &str,
) -> Result<String, String> {
    let mut args = vec![
        "show".to_string(),
        "--format=".to_string(),
        "--find-renames".to_string(),
        "--no-ext-diff".to_string(),
        "--no-color".to_string(),
        "--unified=3".to_string(),
        hash.to_string(),
    ];
    args.extend(diff_args_for_paths(&[
        paths.old_path.as_deref(),
        paths.new_path.as_deref(),
        Some(fallback_path),
    ]));
    run_git_owned(path, &args)
}

pub(crate) fn diff_args_for_paths(paths: &[Option<&str>]) -> Vec<String> {
    let mut args = vec!["--".to_string()];
    let mut seen = HashSet::new();
    for path in paths.iter().flatten().filter(|path| !path.is_empty()) {
        if seen.insert((*path).to_string()) {
            args.push((*path).to_string());
        }
    }
    args
}

fn head_file_lines(repo_path: &Path, file_path: &str) -> Result<Vec<String>, String> {
    tree_file_lines_opt(repo_path, Some("HEAD"), file_path)
}

fn workdir_file_lines(repo_path: &Path, file_path: &str) -> Result<Vec<String>, String> {
    let bytes = match workdir_text_bytes(repo_path, file_path)? {
        Some(bytes) => bytes,
        None => return Ok(Vec::new()),
    };
    Ok(lines_from_bytes(&bytes))
}

fn tree_file_lines_opt(
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<Vec<String>, String> {
    let Some(rev) = rev else {
        return Ok(Vec::new());
    };
    tree_file_lines(repo_path, rev, file_path)
}

fn tree_file_lines(repo_path: &Path, rev: &str, file_path: &str) -> Result<Vec<String>, String> {
    let Some(bytes) = tree_file_bytes(repo_path, rev, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(&bytes)?;
    Ok(lines_from_bytes(&bytes))
}

pub(crate) fn comparison_from_unified_diff(
    diff: &str,
    left_lines: &[String],
    right_lines: &[String],
    complete_empty: bool,
) -> FileComparison {
    FileComparison::from_rows(complete_diff_rows(
        parse_unified_diff(diff),
        left_lines,
        right_lines,
        complete_empty,
    ))
}

pub(crate) fn text_preview_lines(bytes: Option<&[u8]>) -> Result<Vec<String>, String> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    ensure_blob_text_previewable(bytes)?;
    Ok(lines_from_bytes(bytes))
}

fn lines_from_bytes(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(ToString::to_string)
        .collect()
}

fn ensure_workdir_text_previewable(repo_path: &Path, file_path: &str) -> Result<(), String> {
    let _ = workdir_text_bytes(repo_path, file_path)?;
    Ok(())
}

fn ensure_tree_text_previewable(
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<(), String> {
    let Some(rev) = rev else {
        return Ok(());
    };
    let Some(bytes) = tree_file_bytes(repo_path, rev, file_path, MAX_TEXT_PREVIEW_BYTES)? else {
        return Ok(());
    };
    ensure_blob_text_previewable(&bytes)
}

fn workdir_text_bytes(repo_path: &Path, file_path: &str) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo_path)?.join(file_path);
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

fn workdir_binary_bytes(repo_path: &Path, file_path: &str) -> Result<Option<Vec<u8>>, String> {
    let path = repo_root(repo_path)?.join(file_path);
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
    repo_path: &Path,
    rev: Option<&str>,
    file_path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(rev) = rev else {
        return Ok(None);
    };
    tree_file_bytes(repo_path, rev, file_path, MAX_BINARY_PREVIEW_BYTES)
}

fn tree_file_bytes(
    repo_path: &Path,
    rev: &str,
    file_path: &str,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    let spec = format!("{rev}:{file_path}");
    if run_git(repo_path, &["cat-file", "-e", &spec]).is_err() {
        return Ok(None);
    }
    let size = run_git(repo_path, &["cat-file", "-s", &spec])?
        .trim()
        .parse::<usize>()
        .map_err(|err| format!("Failed to parse git object size: {err}"))?;
    if size > max_bytes {
        return Err(format!("{} is too large to preview.", file_name(file_path)));
    }

    run_git_bytes(repo_path, &["show", &spec]).map(Some)
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

#[derive(Default)]
struct DiffRowsBuilder {
    rows: Vec<FileDiffRow>,
    deleted: Vec<PendingDiffLine>,
    added: Vec<PendingDiffLine>,
}

impl DiffRowsBuilder {
    fn push_context(
        &mut self,
        left_number: Option<usize>,
        right_number: Option<usize>,
        text: String,
    ) {
        self.flush();
        self.rows.push(FileDiffRow {
            left_number,
            right_number,
            left_text: Some(text.clone()),
            right_text: Some(text),
            left_kind: DiffKind::Context,
            right_kind: DiffKind::Context,
        });
    }

    fn push_deleted(&mut self, number: Option<usize>, text: String) {
        self.deleted.push(PendingDiffLine { number, text });
    }

    fn push_added(&mut self, number: Option<usize>, text: String) {
        self.added.push(PendingDiffLine { number, text });
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

fn parse_unified_diff(diff: &str) -> Vec<FileDiffRow> {
    let mut builder = DiffRowsBuilder::default();
    let mut next_left = None::<usize>;
    let mut next_right = None::<usize>;
    let mut in_hunk = false;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            builder.flush();
            next_left = None;
            next_right = None;
            in_hunk = false;
            continue;
        }
        if !in_hunk && is_unified_metadata_line(line) {
            builder.flush();
            continue;
        }
        if line.starts_with("@@") {
            builder.flush();
            if let Some((left, right)) = parse_hunk_line_numbers(line) {
                next_left = Some(left);
                next_right = Some(right);
            }
            in_hunk = true;
            continue;
        }
        if line.starts_with("\\ ") {
            continue;
        }
        if !in_hunk {
            continue;
        }

        if let Some(text) = line.strip_prefix('-') {
            let left_number = next_left;
            next_left = next_left.map(|number| number.saturating_add(1));
            builder.push_deleted(left_number, text.to_string());
        } else if let Some(text) = line.strip_prefix('+') {
            let right_number = next_right;
            next_right = next_right.map(|number| number.saturating_add(1));
            builder.push_added(right_number, text.to_string());
        } else {
            let text = line.strip_prefix(' ').unwrap_or(line).to_string();
            let left_number = next_left;
            let right_number = next_right;
            next_left = next_left.map(|number| number.saturating_add(1));
            next_right = next_right.map(|number| number.saturating_add(1));
            builder.push_context(left_number, right_number, text);
        }
    }

    builder.flush();
    builder.rows
}

fn is_unified_metadata_line(line: &str) -> bool {
    line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("new file mode ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("similarity index ")
        || line.starts_with("dissimilarity index ")
        || line.starts_with("Binary files ")
        || line.starts_with("GIT binary patch")
        || line.starts_with("literal ")
        || line.starts_with("delta ")
}

fn parse_hunk_line_numbers(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    parts.next()?;
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    Some((parse_hunk_start(old)?, parse_hunk_start(new)?))
}

fn parse_hunk_start(value: &str) -> Option<usize> {
    value
        .split_once(',')
        .map(|(start, _)| start)
        .unwrap_or(value)
        .parse::<usize>()
        .ok()
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

fn run_git_bytes(path: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output.status.success() {
        Ok(output.stdout)
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
    run_git_owned_with_success_codes(path, args, &[0])
}

fn run_git_owned_with_success_codes(
    path: &Path,
    args: &[String],
    success_codes: &[i32],
) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .map_err(|err| format!("Failed to run git: {err}"))?;

    if output
        .status
        .code()
        .is_some_and(|code| success_codes.contains(&code))
    {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn run_git_owned_with_stdin(path: &Path, args: &[String], stdin: &[u8]) -> Result<String, String> {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .map_err(|err| format!("Failed to write git stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
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
    configure_commit_timezone_env(path, &mut command)?;

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

fn run_git_owned_with_commit_timezone_and_stdin(
    path: &Path,
    args: &[String],
    stdin: &[u8],
) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_commit_timezone_env(path, &mut command)?;

    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to run git: {err}"))?;
    if let Some(mut child_stdin) = child.stdin.take() {
        child_stdin
            .write_all(stdin)
            .map_err(|err| format!("Failed to write git stdin: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

fn configure_commit_timezone_env(path: &Path, command: &mut Command) -> Result<(), String> {
    let local_git_config = crate::workspace_config::git_config(path);
    let commit_timezone = local_git_config
        .commit_timezone
        .or_else(|| local_config_string(path, COMMIT_TIMEZONE_KEY));
    let use_system_timezone = local_git_config
        .use_system_timezone
        .unwrap_or_else(|| local_config_bool(path, USE_SYSTEM_TIMEZONE_KEY));
    let timezone = match commit_timezone {
        Some(timezone) => Some(crate::workspace_config::normalize_timezone(&timezone)?),
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

    Ok(())
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
    let Ok(root) = repo_root(path) else {
        return RepoMetadata::Folder;
    };

    if remote_url(&root, "upstream").is_some() {
        return RepoMetadata::Fork;
    }

    let remote_name = upstream_remote(&root).or_else(|| {
        Some("origin".to_string()).filter(|remote| remote_url(&root, remote).is_some())
    });

    if let Some(name) = remote_name {
        if let Some(url) = remote_url(&root, &name) {
            if let Some(slug) = crate::github::parse_github_url(&url)
                && let Some(metadata) = github_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::github::GitHubRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::github::GitHubRepoMetadata::Private => RepoMetadata::Private,
                    crate::github::GitHubRepoMetadata::Public => RepoMetadata::Public,
                };
            }
            if let Some(slug) = crate::gitlab::parse_gitlab_url(&url)
                && let Some(metadata) = gitlab_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::gitlab::GitLabRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::gitlab::GitLabRepoMetadata::Private => RepoMetadata::Private,
                    crate::gitlab::GitLabRepoMetadata::Public => RepoMetadata::Public,
                };
            }
            if let Some(slug) = crate::bitbucket::parse_bitbucket_url(&url)
                && let Some(metadata) = bitbucket_metadata(&slug, Some(&name), Some(&url))
            {
                return match metadata {
                    crate::bitbucket::BitbucketRepoMetadata::Fork => RepoMetadata::Fork,
                    crate::bitbucket::BitbucketRepoMetadata::Private => RepoMetadata::Private,
                    crate::bitbucket::BitbucketRepoMetadata::Public => RepoMetadata::Public,
                };
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
