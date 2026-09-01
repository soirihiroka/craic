use crate::CommitMessageContext;
use crate::git::{
    self, BranchInfo, ChangedFile, GitSettings, RepositorySnapshot, WorkspaceSnapshot,
};
use crate::github::GitHubAccess;
use crate::gitignore;
use crate::system::capabilities::{
    files::FileAccess,
    shell::{
        ShellAccess, ShellCommandEvent, ShellCommandOutput, ShellCommandRunRequest,
        ShellCommandSpec, ShellRunRequest,
    },
    terminal_link::{TerminalLinkAccess, TerminalLinkTarget},
};
use crate::system::path::FileNodePath;
use crate::system::path::WorkspaceRef;
use crate::{bitbucket, gitlab};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

const GIT_CHANGE_LISTENER_INTERVAL: Duration = Duration::from_secs(2);
const GIT_BACKGROUND_PULL_INTERVAL: Duration = Duration::from_secs(60);
const DEFAULT_COMMIT_TIMEZONE: &str = "+0000";
const RECENT_BRANCHES_LIMIT: usize = 5;
const CHECK_IGNORE_SCRIPT: &str = include_str!("scripts/check_ignore.sh");
const COMMIT_SELECTED_SCRIPT: &str = include_str!("scripts/commit_selected.sh");
const INITIALIZE_REPOSITORY_SCRIPT: &str = include_str!("scripts/initialize_repository.sh");
const PYTHON_DISCARD_PATH_SCRIPT: &str = include_str!("scripts/discard_path.py");
const PYTHON_COMMIT_MESSAGE_DIFF_SCRIPT: &str = include_str!("scripts/commit_message_diff.py");
const PYTHON_DIFF_SCRIPT: &str = include_str!("scripts/diff.py");
const PYTHON_BYTES_SCRIPT: &str = include_str!("scripts/bytes.py");
const PYTHON_HISTORY_PAGE_SCRIPT: &str = include_str!("scripts/history_page.py");
const PYTHON_WATCH_SCRIPT: &str = include_str!("scripts/watch.py");

fn successful_fetch_times() -> &'static Mutex<HashMap<String, SystemTime>> {
    static FETCH_TIMES: OnceLock<Mutex<HashMap<String, SystemTime>>> = OnceLock::new();
    FETCH_TIMES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub type ChangeListener = Arc<dyn Fn() + Send + Sync + 'static>;
pub type GitOperationReceiver<T> = oneshot::Receiver<Result<T, String>>;
pub type FileDiffReceiver<T> = mpsc::Receiver<Result<T, String>>;

#[derive(Debug)]
pub enum GitCommandEvent {
    Progress { message: String },
    Completed { message: Option<String> },
    Failed { message: String },
}

pub type GitCommandGenerator = tokio_mpsc::Receiver<GitCommandEvent>;

const GIT_COMMAND_EVENT_CAPACITY: usize = 256;

pub trait GitOperationHook: Send + Sync {
    fn pre(&self) -> Result<Box<dyn GitOperationPostHook>, String>;
}

pub trait GitOperationPostHook: Send {
    fn post(self: Box<Self>) -> Result<(), String>;
}

pub fn clone_repository_with_shell(
    shell: Arc<dyn ShellAccess>,
    working_dir: crate::system::path::WorkspacePath,
    remote: &str,
    destination_name: &str,
) -> Result<String, String> {
    let args = vec![
        "clone".to_string(),
        remote.trim().to_string(),
        destination_name.to_string(),
    ];
    let output = shell
        .run_fast_command(ShellCommandRunRequest::new("git clone", working_dir, "git").args(args))
        .blocking_recv()
        .map_err(|_| "git clone command did not return a result.".to_string())??;
    if output.status_success(&[0]) {
        Ok("Repository cloned.".to_string())
    } else {
        let message = output.failure_message();
        Err(if message.is_empty() {
            format!("git clone failed with status {:?}", output.status_code)
        } else {
            message
        })
    }
}

pub struct ChangeListenerSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    child: Arc<Mutex<Option<Child>>>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl ChangeListenerSubscription {
    fn spawn(
        label: impl Into<String>,
        command: ShellCommandSpec,
        listener: ChangeListener,
    ) -> Self {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let child_slot = Arc::new(Mutex::new(None::<Child>));
        let thread_child_slot = child_slot.clone();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!("git watcher process starting label={thread_label}");
            let mut command_process = Command::new(&command.program);
            command_process
                .args(&command.args)
                .current_dir(&command.working_dir.absolute)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = match command_process.spawn() {
                Ok(child) => child,
                Err(err) => {
                    log::warn!("git watcher failed to start label={thread_label}: {err}");
                    return;
                }
            };

            let Some(stdout) = child.stdout.take() else {
                log::warn!("git watcher missing stdout label={thread_label}");
                let _ = child.kill();
                return;
            };
            let stderr = child.stderr.take();

            if let Some(stderr) = stderr {
                let stderr_label = thread_label.clone();
                thread::spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        let line = line.trim();
                        if !line.is_empty() {
                            log::warn!("git watcher stderr label={stderr_label}: {line}");
                        }
                    }
                });
            }

            if let Ok(mut slot) = thread_child_slot.lock() {
                *slot = Some(child);
            }

            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if stop_receiver.try_recv().is_ok() {
                    break;
                }
                let Ok(line) = line else {
                    break;
                };
                match line.trim() {
                    "ready" => log::debug!("git watcher ready label={thread_label}"),
                    "changed" => {
                        log::info!("git watcher change detected label={thread_label}");
                        listener();
                    }
                    "recovered" => log::info!("git watcher recovered label={thread_label}"),
                    "" => {}
                    message if message.starts_with("error\t") => {
                        log::warn!("git watcher error label={thread_label}: {}", &message[6..]);
                    }
                    message => log::debug!("git watcher event label={thread_label}: {message}"),
                };
            }

            if let Ok(mut slot) = thread_child_slot.lock()
                && let Some(mut child) = slot.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }

            log::info!("git watcher stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            child: child_slot,
            _thread: Some(thread),
        }
    }
}

impl Drop for ChangeListenerSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
    }
}

pub struct FileDiffSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    wake_sender: Option<mpsc::Sender<()>>,
    _listener: ChangeListenerSubscription,
    thread: Option<thread::JoinHandle<()>>,
}

impl FileDiffSubscription {
    fn spawn<T, F>(
        label: impl Into<String>,
        git: GitRepoHandle,
        file_path: String,
        listener: ChangeListenerSubscription,
        events: mpsc::Receiver<()>,
        wake_sender: mpsc::Sender<()>,
        mut load: F,
    ) -> (Self, FileDiffReceiver<T>)
    where
        T: Send + Sync + 'static,
        F: FnMut(&GitRepoHandle, &str) -> Result<T, String> + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let (sender, receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!("git file diff watcher started label={thread_label} path={file_path}");
            if sender.send(load(&git, &file_path)).is_err() {
                log::info!(
                    "git file diff watcher stopped before initial delivery label={thread_label}"
                );
                return;
            }

            loop {
                if stop_receiver.try_recv().is_ok() {
                    break;
                }

                match events.recv_timeout(Duration::from_millis(200)) {
                    Ok(()) => {
                        while events.try_recv().is_ok() {}
                        if stop_receiver.try_recv().is_ok() {
                            break;
                        }
                        if sender.send(load(&git, &file_path)).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }

            log::info!("git file diff watcher stopped label={thread_label}");
        });

        (
            Self {
                stop_sender: Some(stop_sender),
                wake_sender: Some(wake_sender),
                _listener: listener,
                thread: Some(thread),
            },
            receiver,
        )
    }
}

impl Drop for FileDiffSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
        if let Some(wake_sender) = self.wake_sender.take() {
            let _ = wake_sender.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("git file diff watcher panicked during shutdown");
        }
    }
}

pub struct BackgroundPullSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl BackgroundPullSubscription {
    fn spawn<F>(
        label: impl Into<String>,
        interval: Duration,
        mut pull: F,
        listener: Option<ChangeListener>,
    ) -> Self
    where
        F: FnMut() -> Result<String, String> + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!(
                "git background pull loop started label={} interval_ms={}",
                thread_label,
                interval.as_millis()
            );
            let mut previous_error: Option<String> = None;

            loop {
                let start = Instant::now();
                match pull() {
                    Ok(output) => {
                        if previous_error.take().is_some() {
                            log::info!("git background pull recovered label={thread_label}");
                        }
                        log::info!(
                            "git background pull complete label={} elapsed_ms={} output_len={}",
                            thread_label,
                            start.elapsed().as_millis(),
                            output.len()
                        );
                        if let Some(listener) = listener.as_ref() {
                            listener();
                        }
                    }
                    Err(err) => {
                        if previous_error.as_deref() == Some(err.as_str()) {
                            log::debug!(
                                "git background pull repeated error label={thread_label}: {err}"
                            );
                        } else {
                            log::warn!("git background pull error label={thread_label}: {err}");
                            previous_error = Some(err);
                        }
                    }
                }

                match stop_receiver.recv_timeout(interval) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }

            log::info!("git background pull loop stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            _thread: Some(thread),
        }
    }
}

impl Drop for BackgroundPullSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }
}

#[derive(Deserialize)]
struct RemoteCommitPage {
    commits: Vec<RemoteCommitRow>,
    has_more: bool,
}

#[derive(Deserialize)]
struct RemoteCommitRow {
    hash: String,
    short_hash: String,
    author_b64: String,
    author_email_b64: String,
    subject_b64: String,
    timestamp: i64,
    insertions: usize,
    deletions: usize,
    tags_b64: Vec<String>,
}

#[derive(Deserialize)]
struct PythonDiffResponse {
    diff_b64: String,
    left_b64: Option<String>,
    right_b64: Option<String>,
    paths_changed: bool,
}

#[derive(Deserialize)]
struct PythonBytesResponse {
    before_b64: Option<String>,
    after_b64: Option<String>,
}

#[derive(Deserialize)]
struct PythonCommitMessageDiffResponse {
    diff_b64: String,
}

#[derive(Deserialize)]
struct PythonDiscardPathResponse {
    message: String,
}

#[derive(Clone)]
pub struct GitRepoHandle {
    workspace: WorkspaceRef,
    shell: Arc<dyn ShellAccess>,
    files: Arc<dyn FileAccess>,
    terminal_links: Option<Arc<dyn TerminalLinkAccess>>,
    hooks: Vec<Arc<dyn GitOperationHook>>,
}

struct CommitTargetPlan {
    force_remove_paths: Vec<String>,
    update_paths: Vec<String>,
}

include!("access/construction.rs");

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_script_with_args(script: &str, args: &[String]) -> String {
    let mut command = String::from("set --");
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command.push('\n');
    command.push_str(script);
    command
}

fn run_operation<T, F>(operation: &'static str, run: F) -> GitOperationReceiver<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    thread::spawn(move || {
        let start = Instant::now();
        let result = run();
        log::debug!(
            "git operation complete operation={} status={} elapsed_ms={}",
            operation,
            if result.is_ok() { "ok" } else { "error" },
            start.elapsed().as_millis()
        );
        let _ = sender.send(result);
    });
    receiver
}

fn git_command_generator<F>(operation: &'static str, run: F) -> GitCommandGenerator
where
    F: FnOnce(&mut dyn FnMut(String)) -> Result<String, String> + Send + 'static,
{
    let (sender, receiver) = tokio_mpsc::channel(GIT_COMMAND_EVENT_CAPACITY);
    thread::spawn(move || {
        let start = Instant::now();
        let progress_sender = sender.clone();
        let result = run(&mut move |message| {
            let _ = progress_sender.blocking_send(GitCommandEvent::Progress { message });
        });
        log::debug!(
            "git command generator complete operation={} status={} elapsed_ms={}",
            operation,
            if result.is_ok() { "ok" } else { "error" },
            start.elapsed().as_millis()
        );
        let event = match result {
            Ok(message) => GitCommandEvent::Completed {
                message: (!message.is_empty()).then_some(message),
            },
            Err(message) => GitCommandEvent::Failed { message },
        };
        let _ = sender.blocking_send(event);
    });
    receiver
}

include!("access/operations.rs");
include!("access/repository.rs");

impl GitRepoHandle {
    fn remote_branches(&self) -> Result<Vec<BranchInfo>, String> {
        let current = self
            .git_ok(&["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()])
            .unwrap_or_default();
        let out = self.git(&[
            "for-each-ref".into(),
            "--format=%(refname:short)".into(),
            "refs/heads".into(),
        ])?;
        let recent = self.recent_branch_names();
        Ok(out
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| BranchInfo {
                name: line.trim().to_string(),
                is_current: line.trim() == current,
                upstream: None,
                is_default: line.trim() == "main",
                recent_order: recent.iter().position(|name| name == line.trim()),
            })
            .collect())
    }

    fn recent_branch_names(&self) -> Vec<String> {
        let Ok(out) = self.git(&[
            "log".into(),
            "-g".into(),
            "--format=%gs".into(),
            "HEAD".into(),
            "-n".into(),
            "2500".into(),
            "--".into(),
        ]) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut recent = Vec::new();
        for line in out.lines() {
            let Some((_, branch)) = line
                .strip_prefix("checkout: moving from ")
                .and_then(|movement| movement.rsplit_once(" to "))
            else {
                continue;
            };
            if seen.insert(branch.to_string()) {
                recent.push(branch.to_string());
            }
            if recent.len() >= RECENT_BRANCHES_LIMIT + 1 {
                break;
            }
        }
        recent
    }

    fn ahead_behind(&self) -> (u32, u32, bool) {
        let Ok(out) = self.git_ok(&[
            "rev-list".into(),
            "--left-right".into(),
            "--count".into(),
            "HEAD...@{upstream}".into(),
        ]) else {
            return (0, 0, false);
        };
        let mut parts = out.split_whitespace();
        let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        (ahead, behind, true)
    }

    fn changed_files(&self) -> Result<Vec<ChangedFile>, String> {
        let output = self.run_command_output(
            "git status",
            "git",
            &[
                "--no-optional-locks".to_string(),
                "status".to_string(),
                "--untracked-files=all".to_string(),
                "--branch".to_string(),
                "--porcelain=2".to_string(),
                "-z".to_string(),
            ],
            None,
            &[0],
        )?;
        let mut files = git::parse_porcelain_status_entries(&output.stdout)
            .into_iter()
            .filter(git::status_entry_visible)
            .map(|entry| git::changed_file_from_porcelain_entry(&entry))
            .collect::<Vec<_>>();
        self.populate_worktree_signatures(&mut files);
        files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.status.cmp(&right.status))
        });
        Ok(files)
    }

    fn populate_worktree_signatures(&self, files: &mut [ChangedFile]) {
        for file in files {
            let node_path = self.files.root().join_child(&file.path);
            file.worktree_signature =
                self.files
                    .info(&node_path)
                    .ok()
                    .map(|info| git::ChangedFileSignature {
                        is_dir: info.kind.is_directory(),
                        len: info.len.unwrap_or(0),
                        modified: info.modified,
                    });
        }
    }

    fn commit_target_plan(&self, selected_files: &[String]) -> Result<CommitTargetPlan, String> {
        let output = self.run_command_output(
            "git commit status",
            "git",
            &[
                "--no-optional-locks".to_string(),
                "status".to_string(),
                "--untracked-files=all".to_string(),
                "--branch".to_string(),
                "--porcelain=2".to_string(),
                "-z".to_string(),
            ],
            None,
            &[0],
        )?;
        let entries = git::parse_porcelain_status_entries(&output.stdout);
        let mut force_remove_paths = Vec::new();
        let mut update_paths = Vec::new();
        let mut seen_force_remove_paths = HashSet::new();
        let mut seen_update_paths = HashSet::new();

        for requested in selected_files {
            let mut resolved = false;

            for entry in &entries {
                if !git::porcelain_entry_matches_path(entry, requested) {
                    continue;
                }

                push_commit_target_paths(
                    &mut force_remove_paths,
                    &mut seen_force_remove_paths,
                    git::porcelain_entry_force_remove_paths(entry),
                );
                push_commit_target_paths(
                    &mut update_paths,
                    &mut seen_update_paths,
                    git::porcelain_entry_update_paths(entry),
                );

                resolved = true;
                break;
            }

            if !resolved && seen_update_paths.insert(requested.clone()) {
                update_paths.push(requested.clone());
            }
        }

        log::debug!(
            "shell git commit targets resolved workspace={} selected_count={} force_remove_count={} update_count={}",
            self.workspace.display_name,
            selected_files.len(),
            force_remove_paths.len(),
            update_paths.len()
        );

        Ok(CommitTargetPlan {
            force_remove_paths,
            update_paths,
        })
    }

    fn commit_tags(&self, hash: &str) -> Result<Vec<String>, String> {
        let output = self.git(&["tag".into(), "--points-at".into(), hash.into()])?;
        let mut tags = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        tags.sort();
        Ok(tags)
    }

    fn commit_stats(&self, hash: &str) -> Result<(usize, usize), String> {
        let output = self.git(&[
            "show".into(),
            "--numstat".into(),
            "--format=".into(),
            hash.into(),
        ])?;
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
        Ok((insertions, deletions))
    }
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

fn selected_statuses(changed_files: &[ChangedFile], selected_files: &[String]) -> String {
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

fn parse_commit_details(
    output: &str,
    tags: Vec<String>,
    insertions: usize,
    deletions: usize,
) -> Result<git::Commit, String> {
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

    Ok(git::Commit {
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

fn remote_commit_page(page: RemoteCommitPage) -> git::CommitPage {
    let commits = page
        .commits
        .into_iter()
        .map(remote_commit_row)
        .collect::<Vec<_>>();
    git::CommitPage {
        commits,
        has_more: page.has_more,
    }
}

fn remote_commit_row(row: RemoteCommitRow) -> git::Commit {
    let subject = decode_remote_string(&row.subject_b64);
    let author = decode_remote_string(&row.author_b64);
    let author_email = decode_remote_string(&row.author_email_b64);
    git::Commit {
        hash: row.hash.clone(),
        short_hash: if row.short_hash.is_empty() {
            short_hash(&row.hash).to_string()
        } else {
            row.short_hash
        },
        subject: if subject.is_empty() {
            "Untitled commit".to_string()
        } else {
            subject
        },
        comment: String::new(),
        author: if author.is_empty() {
            "Unknown author".to_string()
        } else {
            author
        },
        author_email: (!author_email.is_empty()).then_some(author_email),
        relative_time: relative_time(row.timestamp),
        insertions: row.insertions,
        deletions: row.deletions,
        tags: row
            .tags_b64
            .iter()
            .map(|tag| decode_remote_string(tag))
            .filter(|tag| !tag.is_empty())
            .collect(),
    }
}

fn decode_remote_string(value: &str) -> String {
    BASE64
        .decode(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

fn decode_b64_bytes(value: &str, operation: &str) -> Result<Vec<u8>, String> {
    BASE64
        .decode(value)
        .map_err(|err| format!("{operation} returned invalid base64: {err}"))
}

fn decode_optional_b64(value: Option<String>, operation: &str) -> Result<Option<Vec<u8>>, String> {
    value
        .as_deref()
        .map(|value| decode_b64_bytes(value, operation))
        .transpose()
}

fn decode_b64_string(value: String, operation: &str) -> Result<String, String> {
    String::from_utf8(decode_b64_bytes(&value, operation)?)
        .map_err(|_| format!("{operation} returned non-UTF-8 diff output"))
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

fn parse_name_status_files(output: &str) -> Vec<ChangedFile> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let status = parts.next()?.trim();
            if status.is_empty() {
                return None;
            }
            let path = if status.starts_with('R') || status.starts_with('C') {
                let _old_path = parts.next()?;
                parts.next()?
            } else {
                parts.next()?
            };
            Some(ChangedFile {
                status: name_status_label(status).to_string(),
                path: path.to_string(),
                git_status_bits: 0,
                worktree_signature: None,
            })
        })
        .collect()
}

fn name_status_label(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "A",
        Some('D') => "D",
        Some('R') => "R",
        Some('U') => "U",
        _ => "M",
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}
