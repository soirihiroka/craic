use crate::system::capabilities::git::{GitAccess, GitWatchCallback, GitWatchSubscription};
use crate::system::capabilities::github::GitHubAccess;
use crate::system::path::WorkspaceRef;
use crate::{bitbucket, git, gitignore, gitlab};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

const LOCAL_GIT_WATCH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub(crate) struct LocalGitAccess {
    workspace: WorkspaceRef,
    root: PathBuf,
}

impl LocalGitAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        let root = PathBuf::from(&workspace.root.absolute);
        Self { workspace, root }
    }
}

impl GitAccess for LocalGitAccess {
    fn snapshot(&self) -> Result<git::RepositorySnapshot, String> {
        log::debug!(
            "local git snapshot start workspace={} root={}",
            self.workspace.display_name,
            self.root.display()
        );
        git::snapshot(&self.root)
    }

    fn watch(&self, callback: GitWatchCallback) -> Result<GitWatchSubscription, String> {
        let root = self.root.clone();
        let label = format!("local:{}", self.workspace.display_name);
        log::info!(
            "local git watcher registered workspace={} root={} interval_ms={}",
            self.workspace.display_name,
            root.display(),
            LOCAL_GIT_WATCH_INTERVAL.as_millis()
        );
        Ok(GitWatchSubscription::spawn(
            label,
            LOCAL_GIT_WATCH_INTERVAL,
            move || git::snapshot(&root),
            callback,
        ))
    }

    fn repo_metadata(&self, github: Option<&dyn GitHubAccess>) -> git::RepoMetadata {
        log::debug!(
            "local git repo metadata start workspace={} root={}",
            self.workspace.display_name,
            self.root.display()
        );
        git::get_repo_metadata_with(
            &self.root,
            &|repo_slug, remote_name, remote_url| {
                github.and_then(|access| {
                    access
                        .repo_metadata(repo_slug, remote_name, remote_url)
                        .ok()
                })
            },
            &|repo_slug, remote_name, remote_url| {
                gitlab::repo_metadata_for_workspace(
                    &self.workspace.id.to_string(),
                    &self.workspace.root.absolute,
                    repo_slug,
                    remote_name,
                    remote_url,
                    || {
                        let remote_url = remote_url.ok_or_else(|| {
                            "Cannot fetch GitLab metadata without a remote URL.".to_string()
                        })?;
                        gitlab::fetch_repo_metadata(remote_url)
                    },
                )
                .ok()
            },
            &|repo_slug, remote_name, remote_url| {
                bitbucket::repo_metadata_for_workspace(
                    &self.workspace.id.to_string(),
                    &self.workspace.root.absolute,
                    repo_slug,
                    remote_name,
                    remote_url,
                    || {
                        let remote_url = remote_url.ok_or_else(|| {
                            "Cannot fetch Bitbucket metadata without a remote URL.".to_string()
                        })?;
                        bitbucket::fetch_repo_metadata(remote_url)
                    },
                )
                .ok()
            },
        )
    }

    fn commit_paths(
        &self,
        summary: &str,
        description: &str,
        files: &[String],
    ) -> Result<String, String> {
        log::info!(
            "local git commit start workspace={} file_count={}",
            self.workspace.display_name,
            files.len()
        );
        git::commit_paths(&self.root, summary, description, files)
    }

    fn discard_path(&self, file_path: &str) -> Result<String, String> {
        log::info!(
            "local git discard start workspace={} path={}",
            self.workspace.display_name,
            file_path
        );
        git::discard_path(&self.root, file_path)
    }

    fn check_ignored_paths(
        &self,
        checks: &[gitignore::IgnoreCheck],
    ) -> Result<HashSet<String>, String> {
        log::debug!(
            "local git check-ignore start workspace={} path_count={}",
            self.workspace.display_name,
            checks.len()
        );
        gitignore::check_ignored_paths(&self.root, checks)
    }

    fn settings(&self) -> git::GitSettings {
        git::settings(&self.root)
    }

    fn save_settings(&self, settings: &git::GitSettings) -> Result<(), String> {
        git::save_settings(
            &self.root,
            settings.use_global_user,
            settings.local_user_name.as_deref().unwrap_or_default(),
            settings.local_user_email.as_deref().unwrap_or_default(),
            settings.commit_timezone.as_deref().unwrap_or_default(),
            settings.warn_if_remote_owner_mismatch,
            settings.use_system_timezone,
            settings.github_auth_account.as_ref(),
        )
    }

    fn save_author_email(&self, email: &str) -> Result<(), String> {
        git::save_author_email(&self.root, email)
    }

    fn push(&self) -> Result<String, String> {
        log::info!(
            "local git push start workspace={}",
            self.workspace.display_name
        );
        git::push(&self.root)
    }

    fn pull(&self) -> Result<String, String> {
        log::info!(
            "local git pull start workspace={}",
            self.workspace.display_name
        );
        git::pull(&self.root)
    }

    fn publish(&self, remote: &str, branch: &str) -> Result<String, String> {
        git::publish(&self.root, remote, branch)
    }

    fn fetch_with_progress(
        &self,
        remote: Option<&str>,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        log::info!(
            "local git fetch start workspace={} remote={:?}",
            self.workspace.display_name,
            remote
        );
        git::fetch_with_progress(&self.root, remote, progress)
    }

    fn checkout_branch(&self, branch: &str) -> Result<String, String> {
        log::info!(
            "local git checkout branch start workspace={} branch={}",
            self.workspace.display_name,
            branch
        );
        git::checkout_branch(&self.root, branch)
    }

    fn checkout_remote_branch(
        &self,
        remote_branch: &str,
        local_branch: &str,
    ) -> Result<String, String> {
        log::info!(
            "local git checkout remote branch start workspace={} remote_branch={} local_branch={}",
            self.workspace.display_name,
            remote_branch,
            local_branch
        );
        git::checkout_remote_branch(&self.root, remote_branch, local_branch)
    }

    fn checkout_pull_request(&self, number: u32) -> Result<String, String> {
        log::info!(
            "local git checkout pull request start workspace={} number={}",
            self.workspace.display_name,
            number
        );
        git::checkout_pull_request(&self.root, number)
    }

    fn create_branch(&self, branch: &str) -> Result<String, String> {
        log::info!(
            "local git create branch start workspace={} branch={}",
            self.workspace.display_name,
            branch
        );
        git::create_branch(&self.root, branch)
    }

    fn checkout_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "local git checkout commit start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::checkout_commit(&self.root, hash)
    }

    fn create_branch_at_commit(&self, branch: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "local git create branch at commit start workspace={} branch={} hash={}",
            self.workspace.display_name,
            branch,
            short_hash(hash)
        );
        git::create_branch_at_commit(&self.root, branch, hash)
    }

    fn create_tag(&self, tag: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "local git create tag start workspace={} tag={} hash={}",
            self.workspace.display_name,
            tag,
            short_hash(hash)
        );
        git::create_tag(&self.root, tag, hash)
    }

    fn reset_to_commit(&self, hash: &str, mode: git::ResetMode) -> Result<String, String> {
        log::info!(
            "local git reset start workspace={} mode={:?} hash={}",
            self.workspace.display_name,
            mode,
            short_hash(hash)
        );
        git::reset_to_commit(&self.root, hash, mode)
    }

    fn revert_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "local git revert start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::revert_commit(&self.root, hash)
    }

    fn cherry_pick_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "local git cherry-pick start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::cherry_pick_commit(&self.root, hash)
    }

    fn amend_head(&self, summary: &str, description: &str) -> Result<String, String> {
        log::info!(
            "local git amend head start workspace={} summary_len={} description_len={}",
            self.workspace.display_name,
            summary.len(),
            description.len()
        );
        git::amend_head(&self.root, summary, description)
    }

    fn stash_changes(&self) -> Result<String, String> {
        log::info!(
            "local git stash start workspace={}",
            self.workspace.display_name
        );
        git::stash_changes(&self.root)
    }

    fn pop_stash(&self) -> Result<String, String> {
        log::info!(
            "local git stash pop start workspace={}",
            self.workspace.display_name
        );
        git::pop_stash(&self.root)
    }

    fn commit_page(&self, after: Option<&str>, limit: usize) -> Result<git::CommitPage, String> {
        log::debug!(
            "local git commit page start workspace={} after={:?} limit={}",
            self.workspace.display_name,
            after.map(short_hash),
            limit
        );
        git::commit_page(&self.root, after, limit)
    }

    fn commit_search_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        log::info!(
            "local git commit search start workspace={} query_len={} after={:?} limit={}",
            self.workspace.display_name,
            query.len(),
            after.map(short_hash),
            limit
        );
        let result = git::commit_search_page(&self.root, query, after, limit);
        match &result {
            Ok(page) => log::debug!(
                "local git commit search complete workspace={} count={} has_more={}",
                self.workspace.display_name,
                page.commits.len(),
                page.has_more
            ),
            Err(err) => log::warn!(
                "local git commit search failed workspace={} error={}",
                self.workspace.display_name,
                err
            ),
        }
        result
    }

    fn commit_details(&self, hash: &str) -> Result<git::Commit, String> {
        log::debug!(
            "local git commit details start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::commit_details(&self.root, hash)
    }

    fn commit_message(&self, hash: &str) -> Result<git::CommitMessage, String> {
        log::debug!(
            "local git commit message start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::commit_message(&self.root, hash)
    }

    fn commit_parent_hash(&self, hash: &str) -> Result<Option<String>, String> {
        log::debug!(
            "local git commit parent start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::commit_parent_hash(&self.root, hash)
    }

    fn commit_changed_files(&self, hash: &str) -> Result<Vec<git::ChangedFile>, String> {
        log::debug!(
            "local git commit files start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        git::commit_changed_files(&self.root, hash)
    }

    fn comparison(&self, file_path: &str) -> Result<git::FileComparison, String> {
        git::comparison(&self.root, file_path)
    }

    fn bytes_comparison(&self, file_path: &str) -> Result<git::BytesComparison, String> {
        git::bytes_comparison(&self.root, file_path)
    }

    fn commit_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::FileComparison, String> {
        log::debug!(
            "local git commit comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        git::commit_comparison(&self.root, hash, file_path)
    }

    fn commit_bytes_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::BytesComparison, String> {
        log::debug!(
            "local git commit bytes comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        git::commit_bytes_comparison(&self.root, hash, file_path)
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}
