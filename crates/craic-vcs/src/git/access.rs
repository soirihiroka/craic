use crate::CommitMessageContext;
use crate::git::{
    self, BranchInfo, ChangedFile, GitSettings, RepositorySnapshot, WorkspaceSnapshot,
};
use crate::github::GitHubAccess;
use crate::gitignore;
use crate::system::capabilities::{
    files::FileAccess,
    shell::{
        ShellAccess, ShellCommandOutput, ShellCommandRunRequest, ShellCommandSpec, ShellRunRequest,
    },
};
use crate::system::path::WorkspaceRef;
use crate::{bitbucket, gitlab};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const GIT_CHANGE_LISTENER_INTERVAL: Duration = Duration::from_secs(2);
const GIT_BACKGROUND_PULL_INTERVAL: Duration = Duration::from_secs(300);
const CHECK_IGNORE_SCRIPT: &str = include_str!("scripts/check_ignore.sh");
const COMMIT_SELECTED_SCRIPT: &str = include_str!("scripts/commit_selected.sh");
const INITIALIZE_REPOSITORY_SCRIPT: &str = include_str!("scripts/initialize_repository.sh");
const PYTHON_DISCARD_PATH_SCRIPT: &str = include_str!("scripts/discard_path.py");
const PYTHON_COMMIT_MESSAGE_DIFF_SCRIPT: &str = include_str!("scripts/commit_message_diff.py");
const PYTHON_DIFF_SCRIPT: &str = include_str!("scripts/diff.py");
const PYTHON_BYTES_SCRIPT: &str = include_str!("scripts/bytes.py");
const PYTHON_HISTORY_PAGE_SCRIPT: &str = include_str!("scripts/history_page.py");
const PYTHON_WATCH_SCRIPT: &str = include_str!("scripts/watch.py");

pub type OperationCallback<T> = Box<dyn FnOnce(Result<T, String>) + Send + 'static>;
pub type WatchCallback<T> = Box<dyn FnMut(Result<T, String>) + Send + 'static>;
pub type ProgressCallback = Box<dyn FnMut(String) + Send + 'static>;
pub type ChangeListener = Arc<dyn Fn() + Send + Sync + 'static>;

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
    let (sender, receiver) = mpsc::channel();
    shell.run_fast_command(
        ShellCommandRunRequest::new("git clone", working_dir, "git").args(args),
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    let output = receiver
        .recv()
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
    _listener: ChangeListenerSubscription,
    _thread: Option<thread::JoinHandle<()>>,
}

impl FileDiffSubscription {
    fn spawn<T, F>(
        label: impl Into<String>,
        git: GitRepoHandle,
        file_path: String,
        listener: ChangeListenerSubscription,
        events: mpsc::Receiver<()>,
        mut callback: WatchCallback<T>,
        mut load: F,
    ) -> Self
    where
        T: Send + 'static,
        F: FnMut(&GitRepoHandle, &str) -> Result<T, String> + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!("git file diff watcher started label={thread_label} path={file_path}");
            callback(load(&git, &file_path));

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
                        callback(load(&git, &file_path));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }

            log::info!("git file diff watcher stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            _listener: listener,
            _thread: Some(thread),
        }
    }
}

impl Drop for FileDiffSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
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
                if stop_receiver.recv_timeout(interval).is_ok() {
                    break;
                }

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
    hooks: Vec<Arc<dyn GitOperationHook>>,
}

struct CommitTargetPlan {
    force_remove_paths: Vec<String>,
    update_paths: Vec<String>,
}

impl GitRepoHandle {
    pub fn new(
        workspace: WorkspaceRef,
        shell: Arc<dyn ShellAccess>,
        files: Arc<dyn FileAccess>,
    ) -> Self {
        Self {
            workspace,
            shell,
            files,
            hooks: Vec::new(),
        }
    }

    pub fn with_hook(mut self, hook: Arc<dyn GitOperationHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    fn git(&self, args: &[String]) -> Result<String, String> {
        self.run_command_text("git", "git", args, None, &[0])
    }

    fn git_ok(&self, args: &[String]) -> Result<String, String> {
        self.git(args).map(|out| out.trim().to_string())
    }

    fn run_script_output(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<ShellCommandOutput, String> {
        let (sender, receiver) = mpsc::channel();
        let mut request = ShellRunRequest::new(operation, self.workspace.root.clone(), script);
        if let Some(stdin) = stdin {
            request = request.stdin(stdin.to_vec());
        }
        self.shell.run_fast_script(
            request,
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        let output = receiver
            .recv()
            .map_err(|_| format!("{operation} shell command did not return a result."))??;
        if output.status_success(success_codes) {
            Ok(output)
        } else {
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
        }
    }

    fn run_script_text(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<String, String> {
        Ok(self
            .run_script_output(operation, script, stdin, success_codes)?
            .stdout_text_trimmed())
    }

    fn run_command_output(
        &self,
        operation: &str,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<ShellCommandOutput, String> {
        let (sender, receiver) = mpsc::channel();
        let mut request =
            ShellCommandRunRequest::new(operation, self.workspace.root.clone(), program)
                .args(args.iter().cloned());
        if let Some(stdin) = stdin {
            request = request.stdin(stdin.to_vec());
        }
        self.shell.run_fast_command(
            request,
            Box::new(move |result| {
                let _ = sender.send(result);
            }),
        );
        let output = receiver
            .recv()
            .map_err(|_| format!("{operation} command did not return a result."))??;
        if output.status_success(success_codes) {
            Ok(output)
        } else {
            let message = output.failure_message();
            Err(if message.is_empty() {
                format!("{operation} failed with status {:?}", output.status_code)
            } else {
                message
            })
        }
    }

    fn run_command_text(
        &self,
        operation: &str,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        success_codes: &[i32],
    ) -> Result<String, String> {
        Ok(self
            .run_command_output(operation, program, args, stdin, success_codes)?
            .stdout_text_trimmed())
    }

    fn run_python_json<T: DeserializeOwned>(
        &self,
        operation: &str,
        script: &str,
        input: serde_json::Value,
    ) -> Result<T, String> {
        let stdin = serde_json::to_vec(&input)
            .map_err(|err| format!("{operation} request serialization failed: {err}"))?;
        let output = self.run_command_output(
            operation,
            "python3",
            &["-c".to_string(), script.to_string()],
            Some(&stdin),
            &[0],
        )?;
        serde_json::from_slice(&output.stdout)
            .map_err(|err| format!("{operation} returned invalid JSON: {err}"))
    }

    fn run_with_hooks<T, F>(&self, operation: &str, run: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        let mut post_hooks = Vec::new();
        for hook in &self.hooks {
            match hook.pre() {
                Ok(post_hook) => post_hooks.push(post_hook),
                Err(err) => {
                    for post_hook in post_hooks.into_iter().rev() {
                        if let Err(post_err) = post_hook.post() {
                            log::warn!(
                                "git operation hook cleanup failed operation={} error={}",
                                operation,
                                post_err
                            );
                        }
                    }
                    return Err(err);
                }
            }
        }

        let result = run();
        let mut post_error = None;
        for post_hook in post_hooks.into_iter().rev() {
            if let Err(err) = post_hook.post() {
                log::warn!(
                    "git operation post hook failed operation={} error={}",
                    operation,
                    err
                );
                if post_error.is_none() {
                    post_error = Some(err);
                }
            }
        }

        match (result, post_error) {
            (Ok(value), None) => Ok(value),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), _) => Err(err),
        }
    }
}

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

fn run_operation<T, F>(operation: &'static str, callback: OperationCallback<T>, run: F)
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    thread::spawn(move || {
        let start = Instant::now();
        let result = run();
        log::debug!(
            "git operation complete operation={} status={} elapsed_ms={}",
            operation,
            if result.is_ok() { "ok" } else { "error" },
            start.elapsed().as_millis()
        );
        callback(result);
    });
}

impl GitRepoHandle {
    pub fn snapshot(&self, callback: OperationCallback<RepositorySnapshot>) {
        let handle = self.clone();
        run_operation("git snapshot", callback, move || {
            handle.repository_snapshot_blocking()
        });
    }

    pub fn workspace_snapshot(&self, callback: OperationCallback<WorkspaceSnapshot>) {
        let handle = self.clone();
        run_operation("git workspace snapshot", callback, move || {
            handle.workspace_snapshot_blocking()
        });
    }

    pub fn add_on_change_listener(&self, listener: ChangeListener) -> ChangeListenerSubscription {
        self.add_on_change_listener_blocking(listener)
    }

    pub fn schedule_background_pull_loop(
        &self,
        listener: Option<ChangeListener>,
    ) -> BackgroundPullSubscription {
        let git = self.clone();
        let label = format!("shell:{}", self.workspace.display_name);
        log::info!(
            "shell git background pull scheduled workspace={} root={} interval_secs={}",
            self.workspace.display_name,
            self.workspace.root.absolute,
            GIT_BACKGROUND_PULL_INTERVAL.as_secs()
        );
        BackgroundPullSubscription::spawn(
            label,
            GIT_BACKGROUND_PULL_INTERVAL,
            move || git.pull_blocking(),
            listener,
        )
    }

    pub fn workspace_metadata(
        &self,
        github: Option<Arc<dyn GitHubAccess>>,
        callback: OperationCallback<git::WorkspaceRepositoryMetadata>,
    ) {
        let handle = self.clone();
        run_operation("git workspace metadata", callback, move || {
            handle.run_with_hooks("git workspace metadata", || {
                Ok(handle.workspace_metadata_blocking(github.as_deref()))
            })
        });
    }

    pub fn initialize_repository(&self, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git initialize repository", callback, move || {
            handle.initialize_repository_blocking()
        });
    }

    pub fn commit_message_context(
        &self,
        files: &[String],
        callback: OperationCallback<CommitMessageContext>,
    ) {
        let handle = self.clone();
        let files = files.to_vec();
        run_operation("git commit message context", callback, move || {
            handle.commit_message_context_blocking(&files)
        });
    }

    pub fn commit_paths(
        &self,
        summary: &str,
        description: &str,
        files: &[String],
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        let files = files.to_vec();
        run_operation("git commit", callback, move || {
            handle.commit_paths_blocking(&summary, &description, &files)
        });
    }

    pub fn discard_path(&self, file_path: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let file_path = file_path.to_string();
        run_operation("git discard", callback, move || {
            handle.discard_path_blocking(&file_path)
        });
    }

    pub fn check_ignored_paths(
        &self,
        checks: &[gitignore::IgnoreCheck],
        callback: OperationCallback<HashSet<String>>,
    ) {
        let handle = self.clone();
        let checks = checks.to_vec();
        run_operation("git check ignored paths", callback, move || {
            handle.check_ignored_paths_blocking(&checks)
        });
    }

    pub fn settings(&self, callback: OperationCallback<GitSettings>) {
        let handle = self.clone();
        run_operation("git settings", callback, move || {
            Ok(handle.settings_blocking())
        });
    }

    pub fn save_settings(&self, settings: &GitSettings, callback: OperationCallback<()>) {
        let handle = self.clone();
        let settings = settings.clone();
        run_operation("git save settings", callback, move || {
            handle.save_settings_blocking(&settings)
        });
    }

    pub fn save_author_identity(&self, name: &str, email: &str, callback: OperationCallback<()>) {
        let handle = self.clone();
        let name = name.to_string();
        let email = email.to_string();
        run_operation("git save author identity", callback, move || {
            handle.save_author_identity_blocking(&name, &email)
        });
    }

    pub fn push(&self, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git push", callback, move || handle.push_blocking());
    }

    pub fn pull(&self, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git pull", callback, move || handle.pull_blocking());
    }

    pub fn publish(&self, remote: &str, branch: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let remote = remote.to_string();
        let branch = branch.to_string();
        run_operation("git publish", callback, move || {
            handle.publish_blocking(&remote, &branch)
        });
    }

    pub fn fetch_with_progress(
        &self,
        remote: Option<&str>,
        mut progress: ProgressCallback,
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let remote = remote.map(ToString::to_string);
        run_operation("git fetch", callback, move || {
            handle.fetch_with_progress_blocking(remote.as_deref(), &mut progress)
        });
    }

    pub fn checkout_branch(&self, branch: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let branch = branch.to_string();
        run_operation("git checkout branch", callback, move || {
            handle.checkout_branch_blocking(&branch)
        });
    }

    pub fn checkout_remote_branch(
        &self,
        remote_branch: &str,
        local_branch: &str,
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let remote_branch = remote_branch.to_string();
        let local_branch = local_branch.to_string();
        run_operation("git checkout remote branch", callback, move || {
            handle.checkout_remote_branch_blocking(&remote_branch, &local_branch)
        });
    }

    pub fn checkout_pull_request(&self, number: u32, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git checkout pull request", callback, move || {
            handle.checkout_pull_request_blocking(number)
        });
    }

    pub fn create_branch(&self, branch: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let branch = branch.to_string();
        run_operation("git create branch", callback, move || {
            handle.create_branch_blocking(&branch)
        });
    }

    pub fn checkout_commit(&self, hash: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git checkout commit", callback, move || {
            handle.checkout_commit_blocking(&hash)
        });
    }

    pub fn create_branch_at_commit(
        &self,
        branch: &str,
        hash: &str,
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let branch = branch.to_string();
        let hash = hash.to_string();
        run_operation("git create branch at commit", callback, move || {
            handle.create_branch_at_commit_blocking(&branch, &hash)
        });
    }

    pub fn create_tag(&self, tag: &str, hash: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let tag = tag.to_string();
        let hash = hash.to_string();
        run_operation("git create tag", callback, move || {
            handle.create_tag_blocking(&tag, &hash)
        });
    }

    pub fn reset_to_commit(
        &self,
        hash: &str,
        mode: git::ResetMode,
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git reset", callback, move || {
            handle.reset_to_commit_blocking(&hash, mode)
        });
    }

    pub fn revert_commit(&self, hash: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git revert", callback, move || {
            handle.revert_commit_blocking(&hash)
        });
    }

    pub fn cherry_pick_commit(&self, hash: &str, callback: OperationCallback<String>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git cherry-pick", callback, move || {
            handle.cherry_pick_commit_blocking(&hash)
        });
    }

    pub fn amend_head(
        &self,
        summary: &str,
        description: &str,
        callback: OperationCallback<String>,
    ) {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        run_operation("git amend head", callback, move || {
            handle.amend_head_blocking(&summary, &description)
        });
    }

    pub fn stash_changes(&self, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git stash", callback, move || {
            handle.stash_changes_blocking()
        });
    }

    pub fn pop_stash(&self, callback: OperationCallback<String>) {
        let handle = self.clone();
        run_operation("git stash pop", callback, move || {
            handle.pop_stash_blocking()
        });
    }

    pub fn commit_page(
        &self,
        after: Option<&str>,
        limit: usize,
        callback: OperationCallback<git::CommitPage>,
    ) {
        let handle = self.clone();
        let after = after.map(ToString::to_string);
        run_operation("git commit page", callback, move || {
            handle.commit_page_blocking(after.as_deref(), limit)
        });
    }

    pub fn commit_search_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
        callback: OperationCallback<git::CommitPage>,
    ) {
        let handle = self.clone();
        let query = query.to_string();
        let after = after.map(ToString::to_string);
        run_operation("git commit search page", callback, move || {
            handle.commit_search_page_blocking(&query, after.as_deref(), limit)
        });
    }

    pub fn commit_details(&self, hash: &str, callback: OperationCallback<git::Commit>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit details", callback, move || {
            handle.commit_details_blocking(&hash)
        });
    }

    pub fn commit_message(&self, hash: &str, callback: OperationCallback<git::CommitMessage>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit message", callback, move || {
            handle.commit_message_blocking(&hash)
        });
    }

    pub fn commit_parent_hash(&self, hash: &str, callback: OperationCallback<Option<String>>) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit parent hash", callback, move || {
            handle.commit_parent_hash_blocking(&hash)
        });
    }

    pub fn commit_changed_files(
        &self,
        hash: &str,
        callback: OperationCallback<Vec<git::ChangedFile>>,
    ) {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit changed files", callback, move || {
            handle.commit_changed_files_blocking(&hash)
        });
    }

    pub fn comparison(&self, file_path: &str, callback: OperationCallback<git::FileComparison>) {
        let handle = self.clone();
        let file_path = file_path.to_string();
        run_operation("git comparison", callback, move || {
            handle.comparison_blocking(&file_path)
        });
    }

    pub fn watch_comparison(
        &self,
        file_path: &str,
        callback: WatchCallback<git::FileComparison>,
    ) -> FileDiffSubscription {
        self.watch_file_diff(file_path, callback, |handle, file_path| {
            handle.comparison_blocking(file_path)
        })
    }

    pub fn watch_bytes_comparison(
        &self,
        file_path: &str,
        callback: WatchCallback<git::BytesComparison>,
    ) -> FileDiffSubscription {
        self.watch_file_diff(file_path, callback, |handle, file_path| {
            handle.bytes_comparison_blocking(file_path)
        })
    }

    pub fn commit_comparison(
        &self,
        hash: &str,
        file_path: &str,
        callback: OperationCallback<git::FileComparison>,
    ) {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        run_operation("git commit comparison", callback, move || {
            handle.commit_comparison_blocking(&hash, &file_path)
        });
    }

    pub fn commit_bytes_comparison(
        &self,
        hash: &str,
        file_path: &str,
        callback: OperationCallback<git::BytesComparison>,
    ) {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        run_operation("git commit bytes comparison", callback, move || {
            handle.commit_bytes_comparison_blocking(&hash, &file_path)
        });
    }

    fn watch_file_diff<T, F>(
        &self,
        file_path: &str,
        callback: WatchCallback<T>,
        load: F,
    ) -> FileDiffSubscription
    where
        T: Send + 'static,
        F: FnMut(&GitRepoHandle, &str) -> Result<T, String> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let sender = Arc::new(Mutex::new(sender));
        let listener: ChangeListener = Arc::new(move || {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(());
            }
        });
        let subscription = self.add_on_change_listener(listener);
        FileDiffSubscription::spawn(
            format!("shell:{}:{file_path}", self.workspace.display_name),
            self.clone(),
            file_path.to_string(),
            subscription,
            receiver,
            callback,
            load,
        )
    }

    fn workspace_snapshot_blocking(&self) -> Result<WorkspaceSnapshot, String> {
        log::info!(
            "shell git snapshot start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        if self
            .git_ok(&["rev-parse".into(), "--is-inside-work-tree".into()])
            .is_err()
        {
            log::debug!(
                "shell git snapshot unavailable workspace={} reason=not-a-repo",
                self.workspace.display_name
            );
            return Ok(WorkspaceSnapshot::NonRepository {
                name: self.workspace.display_name.clone(),
            });
        }

        self.repository_snapshot_blocking()
            .map(WorkspaceSnapshot::Repository)
    }

    fn repository_snapshot_blocking(&self) -> Result<RepositorySnapshot, String> {
        if self
            .git_ok(&["rev-parse".into(), "--is-inside-work-tree".into()])
            .is_err()
        {
            log::debug!(
                "shell git snapshot unavailable workspace={} reason=not-a-repo",
                self.workspace.display_name
            );
            return Err("Not a git repository.".to_string());
        }

        let name = self.workspace.display_name.clone();
        let branch = self
            .git_ok(&["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()])
            .unwrap_or_else(|_| "HEAD".to_string());
        let branches = self.remote_branches().unwrap_or_default();
        let remote_name = self
            .git_ok(&["remote".into()])
            .ok()
            .and_then(|out| out.lines().next().map(ToString::to_string))
            .filter(|name| !name.is_empty());
        let remote_url = remote_name.as_ref().and_then(|remote| {
            self.git_ok(&["remote".into(), "get-url".into(), remote.clone()])
                .ok()
        });
        let remote_owner = remote_url
            .as_deref()
            .and_then(|url| {
                crate::github::parse_github_url(url)
                    .or_else(|| crate::gitlab::parse_gitlab_url(url))
                    .or_else(|| crate::bitbucket::parse_bitbucket_url(url))
            })
            .and_then(|slug| slug.split('/').next().map(str::to_string));
        let (ahead, behind, has_upstream) = self.ahead_behind();
        let changed_files = self.changed_files()?;
        let user_email = self
            .git_ok(&["config".into(), "--get".into(), "user.email".into()])
            .ok();
        let github_avatar_url = user_email
            .as_deref()
            .and_then(crate::github::login_from_noreply_email)
            .map(|login| crate::github::avatar_url_for_login(&login));
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
            last_fetch_at: None,
            user_name: self
                .git_ok(&["config".into(), "--get".into(), "user.name".into()])
                .ok(),
            user_email,
            github_avatar_url,
            warn_if_remote_owner_mismatch: true,
            changed_files,
            history_head: self.git_ok(&["rev-parse".into(), "HEAD".into()]).ok(),
        })
    }

    fn add_on_change_listener_blocking(
        &self,
        listener: ChangeListener,
    ) -> ChangeListenerSubscription {
        let label = format!("shell:{}", self.workspace.display_name);
        log::info!(
            "shell git change listener registered workspace={} root={} interval_secs={}",
            self.workspace.display_name,
            self.workspace.root.absolute,
            GIT_CHANGE_LISTENER_INTERVAL.as_secs()
        );
        let request = json!({
            "repo": self.workspace.root.absolute,
            "interval_seconds": GIT_CHANGE_LISTENER_INTERVAL.as_secs_f64(),
        })
        .to_string();
        let command = self
            .shell
            .fast_command(
                &self.workspace.root,
                "python3",
                &[
                    "-u".to_string(),
                    "-c".to_string(),
                    PYTHON_WATCH_SCRIPT.to_string(),
                    request,
                ],
            )
            .unwrap_or_else(|err| {
                log::warn!(
                    "git watcher python command creation failed workspace={} error={}",
                    self.workspace.display_name,
                    err
                );
                ShellCommandSpec::new("false", self.workspace.root.clone())
            });
        ChangeListenerSubscription::spawn(label, command, listener)
    }

    fn workspace_metadata_blocking(
        &self,
        github: Option<&dyn GitHubAccess>,
    ) -> git::WorkspaceRepositoryMetadata {
        log::debug!(
            "shell git repo metadata start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        if self
            .git_ok(&["rev-parse".into(), "--is-inside-work-tree".into()])
            .is_err()
        {
            log::debug!(
                "shell git repo metadata unavailable workspace={} reason=not-a-repo",
                self.workspace.display_name
            );
            return git::WorkspaceRepositoryMetadata {
                kind: git::RepoMetadata::Folder,
                remote_url: None,
            };
        }

        let has_upstream_remote = self
            .git_ok(&["remote".into(), "get-url".into(), "upstream".into()])
            .is_ok();
        let remote_name = self.primary_remote_name();

        let Some(remote_name) = remote_name else {
            log::debug!(
                "shell git repo metadata unavailable workspace={} reason=no-remote",
                self.workspace.display_name
            );
            return git::WorkspaceRepositoryMetadata {
                kind: git::RepoMetadata::Local,
                remote_url: None,
            };
        };
        let Some(remote_url) = self.remote_url(&remote_name) else {
            log::debug!(
                "shell git repo metadata unavailable workspace={} remote={} reason=no-url",
                self.workspace.display_name,
                remote_name
            );
            return git::WorkspaceRepositoryMetadata {
                kind: git::RepoMetadata::Unknown,
                remote_url: None,
            };
        };

        if has_upstream_remote {
            return git::WorkspaceRepositoryMetadata {
                kind: git::RepoMetadata::Fork,
                remote_url: Some(remote_url),
            };
        }

        if let Some(repo_slug) = crate::github::parse_github_url(&remote_url) {
            if let Some(github) = github {
                match github.repo_metadata(&repo_slug, Some(&remote_name), Some(&remote_url)) {
                    Ok(crate::github::GitHubRepoMetadata::Fork) => {
                        return git::WorkspaceRepositoryMetadata {
                            kind: git::RepoMetadata::Fork,
                            remote_url: Some(remote_url),
                        };
                    }
                    Ok(crate::github::GitHubRepoMetadata::Private) => {
                        return git::WorkspaceRepositoryMetadata {
                            kind: git::RepoMetadata::Private,
                            remote_url: Some(remote_url),
                        };
                    }
                    Ok(crate::github::GitHubRepoMetadata::Public) => {
                        return git::WorkspaceRepositoryMetadata {
                            kind: git::RepoMetadata::Public,
                            remote_url: Some(remote_url),
                        };
                    }
                    Err(err) => {
                        log::warn!(
                            "shell git repo metadata failed workspace={} repo={} err={}",
                            self.workspace.display_name,
                            repo_slug,
                            err
                        );
                        return git::WorkspaceRepositoryMetadata {
                            kind: git::RepoMetadata::Unknown,
                            remote_url: Some(remote_url),
                        };
                    }
                }
            }
            log::debug!(
                "shell git repo metadata unavailable workspace={} repo={} reason=no-github-capability",
                self.workspace.display_name,
                repo_slug
            );
            return git::WorkspaceRepositoryMetadata {
                kind: git::RepoMetadata::Unknown,
                remote_url: Some(remote_url),
            };
        }

        if let Some(repo_slug) = crate::gitlab::parse_gitlab_url(&remote_url) {
            match gitlab::repo_metadata_for_workspace(
                &self.workspace.id.to_string(),
                &self.workspace.root.absolute,
                &repo_slug,
                Some(&remote_name),
                Some(&remote_url),
                || gitlab::fetch_repo_metadata(&remote_url),
            ) {
                Ok(crate::gitlab::GitLabRepoMetadata::Fork) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Fork,
                        remote_url: Some(remote_url),
                    };
                }
                Ok(crate::gitlab::GitLabRepoMetadata::Private) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Private,
                        remote_url: Some(remote_url),
                    };
                }
                Ok(crate::gitlab::GitLabRepoMetadata::Public) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Public,
                        remote_url: Some(remote_url),
                    };
                }
                Err(err) => {
                    log::warn!(
                        "shell git repo metadata failed workspace={} repo={} err={}",
                        self.workspace.display_name,
                        repo_slug,
                        err
                    );
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Unknown,
                        remote_url: Some(remote_url),
                    };
                }
            }
        }

        if let Some(repo_slug) = crate::bitbucket::parse_bitbucket_url(&remote_url) {
            match bitbucket::repo_metadata_for_workspace(
                &self.workspace.id.to_string(),
                &self.workspace.root.absolute,
                &repo_slug,
                Some(&remote_name),
                Some(&remote_url),
                || bitbucket::fetch_repo_metadata(&remote_url),
            ) {
                Ok(crate::bitbucket::BitbucketRepoMetadata::Fork) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Fork,
                        remote_url: Some(remote_url),
                    };
                }
                Ok(crate::bitbucket::BitbucketRepoMetadata::Private) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Private,
                        remote_url: Some(remote_url),
                    };
                }
                Ok(crate::bitbucket::BitbucketRepoMetadata::Public) => {
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Public,
                        remote_url: Some(remote_url),
                    };
                }
                Err(err) => {
                    log::warn!(
                        "shell git repo metadata failed workspace={} repo={} err={}",
                        self.workspace.display_name,
                        repo_slug,
                        err
                    );
                    return git::WorkspaceRepositoryMetadata {
                        kind: git::RepoMetadata::Unknown,
                        remote_url: Some(remote_url),
                    };
                }
            }
        }

        log::debug!(
            "shell git repo metadata unavailable workspace={} remote={} reason=not-github-or-gitlab-or-bitbucket",
            self.workspace.display_name,
            remote_name
        );
        git::WorkspaceRepositoryMetadata {
            kind: git::RepoMetadata::Unknown,
            remote_url: Some(remote_url),
        }
    }

    fn primary_remote_name(&self) -> Option<String> {
        self.git_ok(&[
            "rev-parse".into(),
            "--abbrev-ref".into(),
            "--symbolic-full-name".into(),
            "@{upstream}".into(),
        ])
        .ok()
        .and_then(|upstream| upstream.split('/').next().map(ToString::to_string))
        .filter(|remote| !remote.is_empty())
        .or_else(|| self.remote_url("origin").map(|_| "origin".to_string()))
        .or_else(|| {
            self.git_ok(&["remote".into()])
                .ok()
                .and_then(|out| out.lines().next().map(ToString::to_string))
                .filter(|remote| !remote.is_empty())
        })
    }

    fn remote_url(&self, remote_name: &str) -> Option<String> {
        self.git_ok(&["remote".into(), "get-url".into(), remote_name.to_string()])
            .ok()
            .filter(|url| !url.is_empty())
    }

    fn commit_paths_blocking(
        &self,
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

        log::info!(
            "shell git commit start workspace={} file_count={}",
            self.workspace.display_name,
            files.len()
        );

        let plan = self.commit_target_plan(files)?;
        if plan.force_remove_paths.is_empty() && plan.update_paths.is_empty() {
            return Err("Select at least one file to commit.".to_string());
        }

        let mut args = vec![
            self.workspace.root.absolute.clone(),
            plan.force_remove_paths.len().to_string(),
            plan.update_paths.len().to_string(),
        ];
        args.extend(plan.force_remove_paths);
        args.extend(plan.update_paths);
        let script = shell_script_with_args(COMMIT_SELECTED_SCRIPT, &args);

        let stdin = commit_message_stdin(summary, description);
        let output = self.run_script_output("git commit", &script, Some(&stdin), &[0])?;
        String::from_utf8(output.stdout)
            .map_err(|_| "shell git commit returned non-UTF-8".to_string())
    }

    fn discard_path_blocking(&self, file_path: &str) -> Result<String, String> {
        let response: PythonDiscardPathResponse = self.run_python_json(
            "git discard",
            PYTHON_DISCARD_PATH_SCRIPT,
            json!({ "path": file_path }),
        )?;
        Ok(response.message)
    }

    fn check_ignored_paths_blocking(
        &self,
        checks: &[gitignore::IgnoreCheck],
    ) -> Result<HashSet<String>, String> {
        if checks.is_empty() {
            return Ok(HashSet::new());
        }

        log::debug!(
            "shell git check-ignore start workspace={} path_count={}",
            self.workspace.display_name,
            checks.len()
        );
        let script = shell_script_with_args(
            CHECK_IGNORE_SCRIPT,
            std::slice::from_ref(&self.workspace.root.absolute),
        );
        let stdin = gitignore::check_ignore_stdin(checks);
        let output = self.run_script_output("git check-ignore", &script, Some(&stdin), &[0])?;
        Ok(gitignore::parse_check_ignore_output(checks, &output.stdout))
    }

    fn settings_blocking(&self) -> GitSettings {
        let (
            commit_timezone,
            warn_if_remote_owner_mismatch,
            use_system_timezone,
            github_auth_account,
        ) = {
            let config = crate::workspace_config::git_config_from_file_access(self.files.as_ref());
            (
                config.commit_timezone,
                config.warn_if_remote_owner_mismatch.unwrap_or(true),
                config.use_system_timezone.unwrap_or(false),
                config.github_auth_account,
            )
        };
        GitSettings {
            global_user_name: self
                .git_ok(&[
                    "config".into(),
                    "--global".into(),
                    "--get".into(),
                    "user.name".into(),
                ])
                .ok(),
            global_user_email: self
                .git_ok(&[
                    "config".into(),
                    "--global".into(),
                    "--get".into(),
                    "user.email".into(),
                ])
                .ok(),
            local_user_name: self
                .git_ok(&[
                    "config".into(),
                    "--local".into(),
                    "--get".into(),
                    "user.name".into(),
                ])
                .ok(),
            local_user_email: self
                .git_ok(&[
                    "config".into(),
                    "--local".into(),
                    "--get".into(),
                    "user.email".into(),
                ])
                .ok(),
            use_global_user: false,
            commit_timezone,
            warn_if_remote_owner_mismatch,
            use_system_timezone,
            github_auth_account,
        }
    }

    fn save_settings_blocking(&self, settings: &GitSettings) -> Result<(), String> {
        if settings.use_global_user {
            let _ = self.git(&[
                "config".into(),
                "--local".into(),
                "--unset".into(),
                "user.name".into(),
            ]);
            let _ = self.git(&[
                "config".into(),
                "--local".into(),
                "--unset".into(),
                "user.email".into(),
            ]);
        } else {
            self.git(&[
                "config".into(),
                "--local".into(),
                "user.name".into(),
                settings.local_user_name.clone().unwrap_or_default(),
            ])?;
            self.git(&[
                "config".into(),
                "--local".into(),
                "user.email".into(),
                settings.local_user_email.clone().unwrap_or_default(),
            ])?;
        }

        crate::workspace_config::save_git_config_with_file_access(
            self.files.as_ref(),
            settings.commit_timezone.as_deref().unwrap_or_default(),
            settings.warn_if_remote_owner_mismatch,
            settings.use_system_timezone,
            settings.github_auth_account.as_ref(),
        )
    }

    fn save_author_identity_blocking(&self, name: &str, email: &str) -> Result<(), String> {
        let mut name = name.trim().to_string();
        if name.is_empty() {
            name = crate::github::login_from_noreply_email(email).unwrap_or_default();
        }
        if !name.is_empty() {
            self.git(&["config".into(), "--local".into(), "user.name".into(), name])?;
        }

        self.git(&[
            "config".into(),
            "--local".into(),
            "user.email".into(),
            email.trim().to_string(),
        ])
        .map(|_| ())
    }

    fn push_blocking(&self) -> Result<String, String> {
        self.run_with_hooks("git push", || self.git(&["push".into()]))
    }

    fn pull_blocking(&self) -> Result<String, String> {
        self.run_with_hooks("git pull", || {
            self.git(&[
                "-c".into(),
                "rebase.backend=merge".into(),
                "pull".into(),
                "--ff".into(),
                "--recurse-submodules".into(),
            ])
        })
    }

    fn publish_blocking(&self, remote: &str, branch: &str) -> Result<String, String> {
        self.run_with_hooks("git publish", || {
            self.git(&["push".into(), "-u".into(), remote.into(), branch.into()])
        })
    }

    fn fetch_with_progress_blocking(
        &self,
        remote: Option<&str>,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        progress("Fetching remote...".to_string());
        let mut args = vec!["fetch".to_string(), "--progress".to_string()];
        if let Some(remote) = remote {
            args.push(remote.to_string());
        }
        self.run_with_hooks("git fetch", || self.git(&args))
    }

    fn checkout_branch_blocking(&self, branch: &str) -> Result<String, String> {
        self.git(&["checkout".into(), branch.into()])
    }

    fn checkout_remote_branch_blocking(
        &self,
        remote_branch: &str,
        local_branch: &str,
    ) -> Result<String, String> {
        log::info!(
            "shell git checkout remote branch start workspace={} remote_branch={} local_branch={}",
            self.workspace.display_name,
            remote_branch,
            local_branch
        );
        self.git(&[
            "checkout".into(),
            remote_branch.into(),
            "-b".into(),
            local_branch.into(),
            "--".into(),
        ])
    }

    fn checkout_pull_request_blocking(&self, number: u32) -> Result<String, String> {
        log::info!(
            "shell git checkout pull request start workspace={} number={}",
            self.workspace.display_name,
            number
        );
        self.run_with_hooks("gh pr checkout", || {
            let gh = self
                .shell
                .which("gh")?
                .ok_or_else(|| "gh was not found on the user shell path.".to_string())?;
            self.run_command_text(
                "gh pr checkout",
                &gh,
                &["pr".to_string(), "checkout".to_string(), number.to_string()],
                None,
                &[0],
            )
        })
    }

    fn create_branch_blocking(&self, branch: &str) -> Result<String, String> {
        log::info!(
            "shell git create branch start workspace={} branch={}",
            self.workspace.display_name,
            branch
        );
        self.git(&["checkout".into(), "-b".into(), branch.into()])
    }

    fn checkout_commit_blocking(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "shell git checkout commit start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["checkout".into(), hash.into()])
    }

    fn create_branch_at_commit_blocking(&self, branch: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "shell git create branch at commit start workspace={} branch={} hash={}",
            self.workspace.display_name,
            branch,
            short_hash(hash)
        );
        self.git(&["checkout".into(), "-b".into(), branch.into(), hash.into()])
    }

    fn create_tag_blocking(&self, tag: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "shell git create tag start workspace={} tag={} hash={}",
            self.workspace.display_name,
            tag,
            short_hash(hash)
        );
        self.git(&["tag".into(), tag.into(), hash.into()])
    }

    fn reset_to_commit_blocking(&self, hash: &str, mode: git::ResetMode) -> Result<String, String> {
        let mode_arg = match mode {
            git::ResetMode::Mixed => "--mixed",
            git::ResetMode::Hard => "--hard",
        };
        log::info!(
            "shell git reset start workspace={} mode={:?} hash={}",
            self.workspace.display_name,
            mode,
            short_hash(hash)
        );
        self.git(&["reset".into(), mode_arg.into(), hash.into()])
    }

    fn revert_commit_blocking(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "shell git revert start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["revert".into(), "--no-edit".into(), hash.into()])
    }

    fn cherry_pick_commit_blocking(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "shell git cherry-pick start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["cherry-pick".into(), hash.into()])
    }

    fn amend_head_blocking(&self, summary: &str, description: &str) -> Result<String, String> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("Commit summary is required.".to_string());
        }

        log::info!(
            "shell git amend head start workspace={} summary_len={} description_len={}",
            self.workspace.display_name,
            summary.len(),
            description.len()
        );
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
        self.git(&args)
    }

    fn stash_changes_blocking(&self) -> Result<String, String> {
        log::info!(
            "shell git stash start workspace={}",
            self.workspace.display_name
        );
        self.git(&["stash".into(), "-u".into()])
    }

    fn pop_stash_blocking(&self) -> Result<String, String> {
        log::info!(
            "shell git stash pop start workspace={}",
            self.workspace.display_name
        );
        self.git(&["stash".into(), "pop".into()])
    }

    fn initialize_repository_blocking(&self) -> Result<String, String> {
        log::info!(
            "shell git init start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        let script = shell_script_with_args(
            INITIALIZE_REPOSITORY_SCRIPT,
            std::slice::from_ref(&self.workspace.root.absolute),
        );
        let output = self.run_script_text("git init", &script, None, &[0])?;
        Ok(if output.trim().is_empty() {
            "Initialized empty Git repository.".to_string()
        } else {
            output
        })
    }

    fn commit_message_context_blocking(
        &self,
        files: &[String],
    ) -> Result<CommitMessageContext, String> {
        if files.is_empty() {
            return Err("Select at least one file before generating a commit message.".to_string());
        }
        log::info!(
            "shell git commit message context start workspace={} file_count={}",
            self.workspace.display_name,
            files.len()
        );
        let snapshot = self.repository_snapshot_blocking()?;
        let response: PythonCommitMessageDiffResponse = self.run_python_json(
            "git commit message diff",
            PYTHON_COMMIT_MESSAGE_DIFF_SCRIPT,
            json!({ "files": files }),
        )?;
        let diff = decode_b64_string(response.diff_b64, "git commit message diff")?;
        if diff.trim().is_empty() {
            return Err("No diff found for the selected files.".to_string());
        }
        Ok(CommitMessageContext {
            repo_name: snapshot.name,
            branch: snapshot.branch,
            files: files.to_vec(),
            statuses: selected_statuses(&snapshot.changed_files, files),
            diff,
            commit_convention: crate::workspace_config::commit_convention_from_file_access(
                self.files.as_ref(),
            ),
        })
    }

    fn commit_page_blocking(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        log::debug!(
            "shell git commit page start workspace={} after={:?} limit={}",
            self.workspace.display_name,
            after.map(short_hash),
            limit
        );
        let page: RemoteCommitPage = self.run_python_json(
            "git history page",
            PYTHON_HISTORY_PAGE_SCRIPT,
            json!({
                "after": after.unwrap_or_default(),
                "limit": limit,
            }),
        )?;
        Ok(remote_commit_page(page))
    }

    fn commit_search_page_blocking(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        log::info!(
            "shell git commit search start workspace={} query_len={} after={:?} limit={}",
            self.workspace.display_name,
            query.len(),
            after.map(short_hash),
            limit
        );
        if query.trim().is_empty() {
            return self.commit_page_blocking(after, limit);
        }
        let result = self
            .run_python_json::<RemoteCommitPage>(
                "git history search page",
                PYTHON_HISTORY_PAGE_SCRIPT,
                json!({
                    "after": after.unwrap_or_default(),
                    "limit": limit,
                    "query": query,
                }),
            )
            .map(remote_commit_page);
        match &result {
            Ok(page) => log::debug!(
                "shell git commit search complete workspace={} count={} has_more={}",
                self.workspace.display_name,
                page.commits.len(),
                page.has_more
            ),
            Err(err) => log::warn!(
                "shell git commit search failed workspace={} error={}",
                self.workspace.display_name,
                err
            ),
        }
        result
    }

    fn commit_details_blocking(&self, hash: &str) -> Result<git::Commit, String> {
        log::debug!(
            "shell git commit details start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        let output = self.git(&[
            "show".into(),
            "-s".into(),
            "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ct%x1f%B".into(),
            hash.into(),
        ])?;
        let tags = self.commit_tags(hash).unwrap_or_default();
        let (insertions, deletions) = self.commit_stats(hash).unwrap_or_default();
        parse_commit_details(&output, tags, insertions, deletions)
    }

    fn commit_message_blocking(&self, hash: &str) -> Result<git::CommitMessage, String> {
        log::debug!(
            "shell git commit message start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        let message = self.git(&[
            "show".into(),
            "-s".into(),
            "--format=%B".into(),
            hash.into(),
        ])?;
        let (summary, description) = commit_message_parts(&message);
        Ok(git::CommitMessage {
            summary,
            description,
        })
    }

    fn commit_parent_hash_blocking(&self, hash: &str) -> Result<Option<String>, String> {
        log::debug!(
            "shell git commit parent start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        let output = self.git_ok(&[
            "rev-list".into(),
            "--parents".into(),
            "-n".into(),
            "1".into(),
            hash.into(),
        ])?;
        Ok(output.split_whitespace().nth(1).map(ToString::to_string))
    }

    fn commit_changed_files_blocking(&self, hash: &str) -> Result<Vec<git::ChangedFile>, String> {
        log::debug!(
            "shell git commit files start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        let output = self.git(&[
            "diff-tree".into(),
            "--root".into(),
            "--no-commit-id".into(),
            "--name-status".into(),
            "-r".into(),
            "-M".into(),
            hash.into(),
        ])?;
        let mut files = parse_name_status_files(&output);
        files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.status.cmp(&right.status))
        });
        Ok(files)
    }

    fn comparison_blocking(&self, file_path: &str) -> Result<git::FileComparison, String> {
        let start = Instant::now();
        let response: PythonDiffResponse = self.run_python_json(
            "git worktree comparison",
            PYTHON_DIFF_SCRIPT,
            json!({
                "repo": self.workspace.root.absolute,
                "mode": "worktree",
                "path": file_path,
                "max_text_bytes": git::MAX_TEXT_PREVIEW_BYTES,
            }),
        )?;
        let left_lines = git::text_preview_lines(
            decode_optional_b64(response.left_b64, "git worktree comparison")?.as_deref(),
        )?;
        let right_lines = git::text_preview_lines(
            decode_optional_b64(response.right_b64, "git worktree comparison")?.as_deref(),
        )?;
        let diff = decode_b64_string(response.diff_b64, "git worktree comparison")?;
        let comparison = git::comparison_from_unified_diff(
            &diff,
            &left_lines,
            &right_lines,
            response.paths_changed,
        );
        log::info!(
            "shell git worktree comparison complete workspace={} path={} rows={} elapsed_ms={}",
            self.workspace.display_name,
            file_path,
            comparison.rows.len(),
            start.elapsed().as_millis()
        );
        Ok(comparison)
    }

    fn bytes_comparison_blocking(&self, file_path: &str) -> Result<git::BytesComparison, String> {
        let response: PythonBytesResponse = self.run_python_json(
            "git worktree bytes comparison",
            PYTHON_BYTES_SCRIPT,
            json!({
                "repo": self.workspace.root.absolute,
                "mode": "worktree",
                "path": file_path,
                "max_binary_bytes": git::MAX_BINARY_PREVIEW_BYTES,
            }),
        )?;
        Ok(git::BytesComparison::from_parts(
            decode_optional_b64(response.before_b64, "git worktree bytes comparison")?,
            decode_optional_b64(response.after_b64, "git worktree bytes comparison")?,
        ))
    }

    fn commit_comparison_blocking(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::FileComparison, String> {
        log::debug!(
            "shell git commit comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        let start = Instant::now();
        let response: PythonDiffResponse = self.run_python_json(
            "git commit comparison",
            PYTHON_DIFF_SCRIPT,
            json!({
                "repo": self.workspace.root.absolute,
                "mode": "commit",
                "hash": hash,
                "path": file_path,
                "max_text_bytes": git::MAX_TEXT_PREVIEW_BYTES,
            }),
        )?;
        let left_lines = git::text_preview_lines(
            decode_optional_b64(response.left_b64, "git commit comparison")?.as_deref(),
        )?;
        let right_lines = git::text_preview_lines(
            decode_optional_b64(response.right_b64, "git commit comparison")?.as_deref(),
        )?;
        let diff = decode_b64_string(response.diff_b64, "git commit comparison")?;
        let comparison = git::comparison_from_unified_diff(
            &diff,
            &left_lines,
            &right_lines,
            response.paths_changed,
        );
        log::info!(
            "shell git commit comparison complete workspace={} hash={} path={} rows={} elapsed_ms={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path,
            comparison.rows.len(),
            start.elapsed().as_millis()
        );
        Ok(comparison)
    }

    fn commit_bytes_comparison_blocking(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::BytesComparison, String> {
        log::debug!(
            "shell git commit bytes comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        let response: PythonBytesResponse = self.run_python_json(
            "git commit bytes comparison",
            PYTHON_BYTES_SCRIPT,
            json!({
                "repo": self.workspace.root.absolute,
                "mode": "commit",
                "hash": hash,
                "path": file_path,
                "max_binary_bytes": git::MAX_BINARY_PREVIEW_BYTES,
            }),
        )?;
        Ok(git::BytesComparison::from_parts(
            decode_optional_b64(response.before_b64, "git commit bytes comparison")?,
            decode_optional_b64(response.after_b64, "git commit bytes comparison")?,
        ))
    }
}

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
        Ok(out
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| BranchInfo {
                name: line.trim().to_string(),
                is_current: line.trim() == current,
                upstream: None,
                is_default: line.trim() == "main",
                is_recent: line.trim() == current,
            })
            .collect())
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
