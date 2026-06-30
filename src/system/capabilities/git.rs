use crate::system::capabilities::github::GitHubAccess;
use crate::{git, gitignore};
use std::collections::HashSet;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) type GitWatchCallback = Arc<dyn Fn() + Send + Sync + 'static>;

pub(crate) struct GitWatchSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl GitWatchSubscription {
    pub(crate) fn spawn<F>(
        label: impl Into<String>,
        interval: Duration,
        mut snapshot: F,
        callback: GitWatchCallback,
    ) -> Self
    where
        F: FnMut() -> Result<git::RepositorySnapshot, String> + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!(
                "git watcher started label={} interval_ms={}",
                thread_label,
                interval.as_millis()
            );
            let mut previous_snapshot: Option<git::RepositorySnapshot> = None;
            let mut previous_error: Option<String> = None;

            loop {
                let cycle_start = Instant::now();
                match snapshot() {
                    Ok(next_snapshot) => {
                        if previous_error.take().is_some() {
                            log::info!("git watcher recovered label={thread_label}");
                        }

                        let changed = previous_snapshot
                            .as_ref()
                            .is_some_and(|previous| previous != &next_snapshot);
                        if changed {
                            log::info!(
                                "git watcher change detected label={} branch={} changed_files={}",
                                thread_label,
                                next_snapshot.branch,
                                next_snapshot.changed_files.len()
                            );
                            callback();
                        } else if previous_snapshot.is_none() {
                            log::debug!(
                                "git watcher initial snapshot label={} branch={} changed_files={}",
                                thread_label,
                                next_snapshot.branch,
                                next_snapshot.changed_files.len()
                            );
                        }
                        previous_snapshot = Some(next_snapshot);
                    }
                    Err(err) => {
                        if previous_error.as_deref() == Some(err.as_str()) {
                            log::debug!("git watcher repeated error label={thread_label}: {err}");
                        } else {
                            log::warn!("git watcher error label={thread_label}: {err}");
                            previous_error = Some(err);
                        }
                    }
                }

                let remaining = interval.saturating_sub(cycle_start.elapsed());
                if stop_receiver.recv_timeout(remaining).is_ok() {
                    break;
                }
            }

            log::info!("git watcher stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            _thread: Some(thread),
        }
    }
}

impl Drop for GitWatchSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }
}

pub(crate) trait GitAccess: Send + Sync {
    fn snapshot(&self) -> Result<git::RepositorySnapshot, String>;
    fn watch(&self, callback: GitWatchCallback) -> Result<GitWatchSubscription, String>;
    fn repo_metadata(&self, github: Option<&dyn GitHubAccess>) -> git::RepoMetadata;
    fn commit_paths(
        &self,
        summary: &str,
        description: &str,
        files: &[String],
    ) -> Result<String, String>;
    fn discard_path(&self, file_path: &str) -> Result<String, String>;
    fn check_ignored_paths(
        &self,
        checks: &[gitignore::IgnoreCheck],
    ) -> Result<HashSet<String>, String>;
    fn settings(&self) -> git::GitSettings;
    fn save_settings(&self, settings: &git::GitSettings) -> Result<(), String>;
    fn save_author_email(&self, email: &str) -> Result<(), String>;
    fn push(&self) -> Result<String, String>;
    fn pull(&self) -> Result<String, String>;
    fn publish(&self, remote: &str, branch: &str) -> Result<String, String>;
    fn fetch_with_progress(
        &self,
        remote: Option<&str>,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String>;
    fn checkout_branch(&self, branch: &str) -> Result<String, String>;
    fn checkout_remote_branch(
        &self,
        remote_branch: &str,
        local_branch: &str,
    ) -> Result<String, String>;
    fn checkout_pull_request(&self, number: u32) -> Result<String, String>;
    fn create_branch(&self, branch: &str) -> Result<String, String>;
    fn checkout_commit(&self, hash: &str) -> Result<String, String>;
    fn create_branch_at_commit(&self, branch: &str, hash: &str) -> Result<String, String>;
    fn create_tag(&self, tag: &str, hash: &str) -> Result<String, String>;
    fn reset_to_commit(&self, hash: &str, mode: git::ResetMode) -> Result<String, String>;
    fn revert_commit(&self, hash: &str) -> Result<String, String>;
    fn cherry_pick_commit(&self, hash: &str) -> Result<String, String>;
    fn amend_head(&self, summary: &str, description: &str) -> Result<String, String>;
    fn stash_changes(&self) -> Result<String, String>;
    fn pop_stash(&self) -> Result<String, String>;
    fn commit_page(&self, after: Option<&str>, limit: usize) -> Result<git::CommitPage, String>;
    fn commit_search_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String>;
    fn commit_details(&self, hash: &str) -> Result<git::Commit, String>;
    fn commit_message(&self, hash: &str) -> Result<git::CommitMessage, String>;
    fn commit_parent_hash(&self, hash: &str) -> Result<Option<String>, String>;
    fn commit_changed_files(&self, hash: &str) -> Result<Vec<git::ChangedFile>, String>;
    fn comparison(&self, file_path: &str) -> Result<git::FileComparison, String>;
    fn bytes_comparison(&self, file_path: &str) -> Result<git::BytesComparison, String>;
    fn commit_comparison(&self, hash: &str, file_path: &str)
    -> Result<git::FileComparison, String>;
    fn commit_bytes_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::BytesComparison, String>;
}
