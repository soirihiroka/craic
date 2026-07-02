use super::{SshCommandRunner, shell_quote};
use crate::git::{
    self, BranchInfo, ChangedFile, ChangedFileSignature, GitSettings, RepositorySnapshot,
};
use crate::gitignore;
use crate::system::capabilities::{
    files::FileAccess,
    git::{GitAccess, GitWatchCallback, GitWatchSubscription},
    github::GitHubAccess,
};
use crate::system::path::WorkspaceRef;
use crate::{bitbucket, gitlab};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SSH_GIT_WATCH_INTERVAL: Duration = Duration::from_secs(30);

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

#[derive(Clone)]
pub(crate) struct SshGitAccess {
    workspace: WorkspaceRef,
    runner: SshCommandRunner,
    files: Arc<dyn FileAccess>,
}

struct SshCommitTargetPlan {
    force_remove_paths: Vec<String>,
    update_paths: Vec<String>,
}

impl SshGitAccess {
    pub(crate) fn new(
        workspace: WorkspaceRef,
        runner: SshCommandRunner,
        files: Arc<dyn FileAccess>,
    ) -> Self {
        Self {
            workspace,
            runner,
            files,
        }
    }

    fn git(&self, args: &[String]) -> Result<String, String> {
        let args = args
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        self.runner.run_text(
            "git",
            &format!(
                "git -C {} {}",
                shell_quote(&self.workspace.root.absolute),
                args
            ),
        )
    }

    fn git_ok(&self, args: &[String]) -> Result<String, String> {
        self.git(args).map(|out| out.trim().to_string())
    }
}

impl GitAccess for SshGitAccess {
    fn snapshot(&self) -> Result<RepositorySnapshot, String> {
        log::info!(
            "ssh git snapshot start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
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

    fn watch(&self, callback: GitWatchCallback) -> Result<GitWatchSubscription, String> {
        let git = self.clone();
        let label = format!("ssh:{}", self.workspace.display_name);
        log::info!(
            "ssh git watcher registered workspace={} root={} interval_secs={}",
            self.workspace.display_name,
            self.workspace.root.absolute,
            SSH_GIT_WATCH_INTERVAL.as_secs()
        );
        Ok(GitWatchSubscription::spawn(
            label,
            SSH_GIT_WATCH_INTERVAL,
            move || git.snapshot(),
            callback,
        ))
    }

    fn repo_metadata(&self, github: Option<&dyn GitHubAccess>) -> git::RepoMetadata {
        log::debug!(
            "ssh git repo metadata start workspace={} root={}",
            self.workspace.display_name,
            self.workspace.root.absolute
        );
        if self
            .git_ok(&["rev-parse".into(), "--is-inside-work-tree".into()])
            .is_err()
        {
            log::debug!(
                "ssh git repo metadata unavailable workspace={} reason=not-a-repo",
                self.workspace.display_name
            );
            return git::RepoMetadata::Folder;
        }

        if self
            .git_ok(&["remote".into(), "get-url".into(), "upstream".into()])
            .is_ok()
        {
            return git::RepoMetadata::Fork;
        }

        let remote_name = self
            .git_ok(&[
                "rev-parse".into(),
                "--abbrev-ref".into(),
                "--symbolic-full-name".into(),
                "@{upstream}".into(),
            ])
            .ok()
            .and_then(|upstream| upstream.split('/').next().map(ToString::to_string))
            .filter(|remote| !remote.is_empty())
            .or_else(|| {
                self.git_ok(&["remote".into(), "get-url".into(), "origin".into()])
                    .ok()
                    .map(|_| "origin".to_string())
            });

        let Some(remote_name) = remote_name else {
            log::debug!(
                "ssh git repo metadata unavailable workspace={} reason=no-remote",
                self.workspace.display_name
            );
            return git::RepoMetadata::Private;
        };
        let Some(remote_url) = self
            .git_ok(&["remote".into(), "get-url".into(), remote_name.clone()])
            .ok()
        else {
            log::debug!(
                "ssh git repo metadata unavailable workspace={} remote={} reason=no-url",
                self.workspace.display_name,
                remote_name
            );
            return git::RepoMetadata::Private;
        };

        if let Some(repo_slug) = crate::github::parse_github_url(&remote_url) {
            if let Some(github) = github {
                match github.repo_metadata(&repo_slug, Some(&remote_name), Some(&remote_url)) {
                    Ok(crate::github::GitHubRepoMetadata::Fork) => return git::RepoMetadata::Fork,
                    Ok(crate::github::GitHubRepoMetadata::Private) => {
                        return git::RepoMetadata::Private;
                    }
                    Ok(crate::github::GitHubRepoMetadata::Public) => {
                        return git::RepoMetadata::Public;
                    }
                    Err(err) => {
                        log::warn!(
                            "ssh git repo metadata failed workspace={} repo={} err={}",
                            self.workspace.display_name,
                            repo_slug,
                            err
                        );
                        return git::RepoMetadata::Private;
                    }
                }
            }
            log::debug!(
                "ssh git repo metadata unavailable workspace={} repo={} reason=no-github-capability",
                self.workspace.display_name,
                repo_slug
            );
            return git::RepoMetadata::Private;
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
                Ok(crate::gitlab::GitLabRepoMetadata::Fork) => return git::RepoMetadata::Fork,
                Ok(crate::gitlab::GitLabRepoMetadata::Private) => {
                    return git::RepoMetadata::Private;
                }
                Ok(crate::gitlab::GitLabRepoMetadata::Public) => {
                    return git::RepoMetadata::Public;
                }
                Err(err) => {
                    log::warn!(
                        "ssh git repo metadata failed workspace={} repo={} err={}",
                        self.workspace.display_name,
                        repo_slug,
                        err
                    );
                    return git::RepoMetadata::Private;
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
                    return git::RepoMetadata::Fork;
                }
                Ok(crate::bitbucket::BitbucketRepoMetadata::Private) => {
                    return git::RepoMetadata::Private;
                }
                Ok(crate::bitbucket::BitbucketRepoMetadata::Public) => {
                    return git::RepoMetadata::Public;
                }
                Err(err) => {
                    log::warn!(
                        "ssh git repo metadata failed workspace={} repo={} err={}",
                        self.workspace.display_name,
                        repo_slug,
                        err
                    );
                    return git::RepoMetadata::Private;
                }
            }
        }

        log::debug!(
            "ssh git repo metadata unavailable workspace={} remote={} reason=not-github-or-gitlab-or-bitbucket",
            self.workspace.display_name,
            remote_name
        );
        git::RepoMetadata::Private
    }

    fn commit_paths(
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
            "ssh git commit start workspace={} file_count={}",
            self.workspace.display_name,
            files.len()
        );

        let plan = self.commit_target_plan(files)?;
        if plan.force_remove_paths.is_empty() && plan.update_paths.is_empty() {
            return Err("Select at least one file to commit.".to_string());
        }

        let mut script = format!(
            "cd {} || exit 2\n\
             if git rev-parse --verify HEAD >/dev/null 2>&1; then\n\
               git reset -- .\n\
             else\n\
               git rm --cached -r --ignore-unmatch . >/dev/null 2>&1 || true\n\
             fi",
            shell_quote(&self.workspace.root.absolute)
        );
        if !plan.force_remove_paths.is_empty() {
            script.push_str("\ngit update-index --force-remove --");
            for file in &plan.force_remove_paths {
                script.push(' ');
                script.push_str(&shell_quote(file));
            }
        }
        if !plan.update_paths.is_empty() {
            script.push_str("\ngit update-index --add --remove --replace --");
            for file in &plan.update_paths {
                script.push(' ');
                script.push_str(&shell_quote(file));
            }
        }
        script.push_str("\ngit commit -F -");

        let stdin = commit_message_stdin(summary, description);
        let output = self
            .runner
            .run_script_with_stdin("git commit", &script, Some(&stdin))?;
        String::from_utf8(output.stdout)
            .map_err(|_| "ssh git commit returned non-UTF-8".to_string())
    }

    fn discard_path(&self, file_path: &str) -> Result<String, String> {
        let script = format!(
            "cd {} && if git status --porcelain -- {} | grep -q '^??'; then rm -rf -- {}; else git restore --staged --worktree -- {}; fi",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(file_path),
            shell_quote(file_path),
            shell_quote(file_path)
        );
        self.runner.run_text("git discard", &script)
    }

    fn check_ignored_paths(
        &self,
        checks: &[gitignore::IgnoreCheck],
    ) -> Result<HashSet<String>, String> {
        if checks.is_empty() {
            return Ok(HashSet::new());
        }

        log::debug!(
            "ssh git check-ignore start workspace={} path_count={}",
            self.workspace.display_name,
            checks.len()
        );
        let script = format!(
            "cd {} || exit 2\n\
             git check-ignore --stdin -z\n\
             status=$?\n\
             [ \"$status\" -eq 0 ] || [ \"$status\" -eq 1 ] || exit \"$status\"\n\
             exit 0",
            shell_quote(&self.workspace.root.absolute)
        );
        let stdin = gitignore::check_ignore_stdin(checks);
        let output =
            self.runner
                .run_script_with_stdin("git check-ignore", &script, Some(&stdin))?;
        Ok(gitignore::parse_check_ignore_output(checks, &output.stdout))
    }

    fn settings(&self) -> GitSettings {
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

    fn save_settings(&self, settings: &GitSettings) -> Result<(), String> {
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

    fn save_author_email(&self, email: &str) -> Result<(), String> {
        self.git(&[
            "config".into(),
            "--local".into(),
            "user.email".into(),
            email.trim().to_string(),
        ])
        .map(|_| ())
    }

    fn push(&self) -> Result<String, String> {
        self.git(&["push".into()])
    }

    fn pull(&self) -> Result<String, String> {
        self.git(&["pull".into(), "--rebase".into()])
    }

    fn publish(&self, remote: &str, branch: &str) -> Result<String, String> {
        self.git(&["push".into(), "-u".into(), remote.into(), branch.into()])
    }

    fn fetch_with_progress(
        &self,
        remote: Option<&str>,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        progress("Fetching remote...".to_string());
        let mut args = vec!["fetch".to_string(), "--progress".to_string()];
        if let Some(remote) = remote {
            args.push(remote.to_string());
        }
        self.git(&args)
    }

    fn checkout_branch(&self, branch: &str) -> Result<String, String> {
        self.git(&["checkout".into(), branch.into()])
    }

    fn checkout_remote_branch(
        &self,
        remote_branch: &str,
        local_branch: &str,
    ) -> Result<String, String> {
        log::info!(
            "ssh git checkout remote branch start workspace={} remote_branch={} local_branch={}",
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

    fn checkout_pull_request(&self, number: u32) -> Result<String, String> {
        log::info!(
            "ssh git checkout pull request start workspace={} number={}",
            self.workspace.display_name,
            number
        );
        let script = format!(
            "cd {} && gh_cmd=$(command -v gh 2>/dev/null || {{ if [ -x /home/linuxbrew/.linuxbrew/bin/gh ]; then printf '%s\\n' /home/linuxbrew/.linuxbrew/bin/gh; elif [ -x \"$HOME/.local/bin/gh\" ]; then printf '%s\\n' \"$HOME/.local/bin/gh\"; else printf '%s\\n' gh; fi; }}) && \"$gh_cmd\" pr checkout {}",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(&number.to_string())
        );
        self.runner.run_text("gh pr checkout", &script)
    }

    fn create_branch(&self, branch: &str) -> Result<String, String> {
        log::info!(
            "ssh git create branch start workspace={} branch={}",
            self.workspace.display_name,
            branch
        );
        self.git(&["checkout".into(), "-b".into(), branch.into()])
    }

    fn checkout_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "ssh git checkout commit start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["checkout".into(), hash.into()])
    }

    fn create_branch_at_commit(&self, branch: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "ssh git create branch at commit start workspace={} branch={} hash={}",
            self.workspace.display_name,
            branch,
            short_hash(hash)
        );
        self.git(&["checkout".into(), "-b".into(), branch.into(), hash.into()])
    }

    fn create_tag(&self, tag: &str, hash: &str) -> Result<String, String> {
        log::info!(
            "ssh git create tag start workspace={} tag={} hash={}",
            self.workspace.display_name,
            tag,
            short_hash(hash)
        );
        self.git(&["tag".into(), tag.into(), hash.into()])
    }

    fn reset_to_commit(&self, hash: &str, mode: git::ResetMode) -> Result<String, String> {
        let mode_arg = match mode {
            git::ResetMode::Mixed => "--mixed",
            git::ResetMode::Hard => "--hard",
        };
        log::info!(
            "ssh git reset start workspace={} mode={:?} hash={}",
            self.workspace.display_name,
            mode,
            short_hash(hash)
        );
        self.git(&["reset".into(), mode_arg.into(), hash.into()])
    }

    fn revert_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "ssh git revert start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["revert".into(), "--no-edit".into(), hash.into()])
    }

    fn cherry_pick_commit(&self, hash: &str) -> Result<String, String> {
        log::info!(
            "ssh git cherry-pick start workspace={} hash={}",
            self.workspace.display_name,
            short_hash(hash)
        );
        self.git(&["cherry-pick".into(), hash.into()])
    }

    fn amend_head(&self, summary: &str, description: &str) -> Result<String, String> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("Commit summary is required.".to_string());
        }

        log::info!(
            "ssh git amend head start workspace={} summary_len={} description_len={}",
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

    fn stash_changes(&self) -> Result<String, String> {
        log::info!(
            "ssh git stash start workspace={}",
            self.workspace.display_name
        );
        self.git(&["stash".into(), "-u".into()])
    }

    fn pop_stash(&self) -> Result<String, String> {
        log::info!(
            "ssh git stash pop start workspace={}",
            self.workspace.display_name
        );
        self.git(&["stash".into(), "pop".into()])
    }

    fn commit_page(&self, after: Option<&str>, limit: usize) -> Result<git::CommitPage, String> {
        log::debug!(
            "ssh git commit page start workspace={} after={:?} limit={}",
            self.workspace.display_name,
            after.map(short_hash),
            limit
        );
        let script = format!(
            r#"cd {} || exit
after={}
limit={}
b64() {{ printf '%s' "$1" | base64 | tr -d '\n'; }}
fetch_limit=$((limit + 1))
hashes=$(git rev-list HEAD | awk -v after="$after" -v limit="$fetch_limit" '
BEGIN {{ seen = (after == ""); count = 0 }}
{{
    if (!seen) {{
        if ($0 == after) seen = 1
        next
    }}
    print
    count += 1
    if (count >= limit) exit
}}')
count=$(printf '%s\n' "$hashes" | sed '/^$/d' | wc -l)
if [ "$count" -gt "$limit" ]; then has_more=true; else has_more=false; fi
printf '{{"commits":['
first=1
printf '%s\n' "$hashes" | sed '/^$/d' | head -n "$limit" | while IFS= read -r hash; do
    [ -n "$hash" ] || continue
    if [ "$first" = 1 ]; then first=0; else printf ','; fi
    short_hash=$(git show -s --format=%h "$hash")
    author=$(git show -s --format=%an "$hash")
    author_email=$(git show -s --format=%ae "$hash")
    timestamp=$(git show -s --format=%ct "$hash")
    subject=$(git show -s --format=%s "$hash")
    set -- $(git show --numstat --format= "$hash" | awk '
        $1 ~ /^[0-9]+$/ {{ added += $1 }}
        $2 ~ /^[0-9]+$/ {{ deleted += $2 }}
        END {{ printf "%d %d", added, deleted }}
    ')
    insertions=${{1:-0}}
    deletions=${{2:-0}}
    printf '{{"hash":"%s","short_hash":"%s","author_b64":"%s","author_email_b64":"%s","subject_b64":"%s","timestamp":%s,"insertions":%s,"deletions":%s,"tags_b64":[' \
        "$hash" "$short_hash" "$(b64 "$author")" "$(b64 "$author_email")" "$(b64 "$subject")" "$timestamp" "$insertions" "$deletions"
    tag_first=1
    git tag --points-at "$hash" | while IFS= read -r tag; do
        [ -n "$tag" ] || continue
        if [ "$tag_first" = 1 ]; then tag_first=0; else printf ','; fi
        printf '"%s"' "$(b64 "$tag")"
    done
    printf ']}}'
done
printf '],"has_more":%s}}\n' "$has_more""#,
            shell_quote(&self.workspace.root.absolute),
            shell_quote(after.unwrap_or_default()),
            limit
        );
        parse_remote_commit_page(&self.runner.run_text("git history page", &script)?)
    }

    fn commit_search_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        log::info!(
            "ssh git commit search start workspace={} query_len={} after={:?} limit={}",
            self.workspace.display_name,
            query.len(),
            after.map(short_hash),
            limit
        );
        if query.trim().is_empty() {
            return self.commit_page(after, limit);
        }
        let query_b64 = BASE64.encode(query);
        let script = format!(
            r#"cd {} || exit
after={}
limit={}
query_b64={}
query=$(printf '%s' "$query_b64" | base64 -d 2>/dev/null || true)
query_lc=$(printf '%s' "$query" | tr '[:upper:]' '[:lower:]')
b64() {{ printf '%s' "$1" | base64 | tr -d '\n'; }}
fetch_limit=$((limit + 1))
hashes=$(git rev-list HEAD | while IFS= read -r hash; do
    [ -n "$hash" ] || continue
    if [ -n "$after" ]; then
        if [ "$hash" = "$after" ]; then after=""; fi
        continue
    fi
    short_hash=$(git show -s --format=%h "$hash")
    author=$(git show -s --format=%an "$hash")
    author_email=$(git show -s --format=%ae "$hash")
    message=$(git show -s --format=%B "$hash")
    tags=$(git tag --points-at "$hash")
    search_text=$(printf '%s\n%s\n%s\n%s\n%s\n%s\n' "$hash" "$short_hash" "$message" "$author" "$author_email" "$tags" | tr '[:upper:]' '[:lower:]')
    if printf '%s\n' "$search_text" | awk -v q="$query_lc" 'index($0, q) {{ found = 1 }} END {{ exit found ? 0 : 1 }}'; then
        printf '%s\n' "$hash"
        count=$((count + 1))
        if [ "$count" -ge "$fetch_limit" ]; then break; fi
    fi
done)
count=$(printf '%s\n' "$hashes" | sed '/^$/d' | wc -l)
if [ "$count" -gt "$limit" ]; then has_more=true; else has_more=false; fi
printf '{{"commits":['
first=1
printf '%s\n' "$hashes" | sed '/^$/d' | head -n "$limit" | while IFS= read -r hash; do
    [ -n "$hash" ] || continue
    if [ "$first" = 1 ]; then first=0; else printf ','; fi
    short_hash=$(git show -s --format=%h "$hash")
    author=$(git show -s --format=%an "$hash")
    author_email=$(git show -s --format=%ae "$hash")
    timestamp=$(git show -s --format=%ct "$hash")
    subject=$(git show -s --format=%s "$hash")
    set -- $(git show --numstat --format= "$hash" | awk '
        $1 ~ /^[0-9]+$/ {{ added += $1 }}
        $2 ~ /^[0-9]+$/ {{ deleted += $2 }}
        END {{ printf "%d %d", added, deleted }}
    ')
    insertions=${{1:-0}}
    deletions=${{2:-0}}
    printf '{{"hash":"%s","short_hash":"%s","author_b64":"%s","author_email_b64":"%s","subject_b64":"%s","timestamp":%s,"insertions":%s,"deletions":%s,"tags_b64":[' \
        "$hash" "$short_hash" "$(b64 "$author")" "$(b64 "$author_email")" "$(b64 "$subject")" "$timestamp" "$insertions" "$deletions"
    tag_first=1
    git tag --points-at "$hash" | while IFS= read -r tag; do
        [ -n "$tag" ] || continue
        if [ "$tag_first" = 1 ]; then tag_first=0; else printf ','; fi
        printf '"%s"' "$(b64 "$tag")"
    done
    printf ']}}'
done
printf '],"has_more":%s}}\n' "$has_more""#,
            shell_quote(&self.workspace.root.absolute),
            shell_quote(after.unwrap_or_default()),
            limit,
            shell_quote(&query_b64)
        );
        let result =
            parse_remote_commit_page(&self.runner.run_text("git history search page", &script)?);
        match &result {
            Ok(page) => log::debug!(
                "ssh git commit search complete workspace={} count={} has_more={}",
                self.workspace.display_name,
                page.commits.len(),
                page.has_more
            ),
            Err(err) => log::warn!(
                "ssh git commit search failed workspace={} error={}",
                self.workspace.display_name,
                err
            ),
        }
        result
    }

    fn commit_details(&self, hash: &str) -> Result<git::Commit, String> {
        log::debug!(
            "ssh git commit details start workspace={} hash={}",
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

    fn commit_message(&self, hash: &str) -> Result<git::CommitMessage, String> {
        log::debug!(
            "ssh git commit message start workspace={} hash={}",
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

    fn commit_parent_hash(&self, hash: &str) -> Result<Option<String>, String> {
        log::debug!(
            "ssh git commit parent start workspace={} hash={}",
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

    fn commit_changed_files(&self, hash: &str) -> Result<Vec<git::ChangedFile>, String> {
        log::debug!(
            "ssh git commit files start workspace={} hash={}",
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

    fn comparison(&self, file_path: &str) -> Result<git::FileComparison, String> {
        let start = Instant::now();
        let paths = self.worktree_file_path_pair(file_path)?;
        let old_path = paths.old_path.as_deref().unwrap_or(file_path);
        let new_path = paths.new_path.as_deref().unwrap_or(file_path);
        let left_lines = self.tree_file_text_lines_opt(Some("HEAD"), old_path)?;
        let right_lines = self.workdir_text_lines(new_path)?;
        let diff = self.worktree_diff(&paths, file_path)?;
        let comparison = git::comparison_from_unified_diff(
            &diff,
            &left_lines,
            &right_lines,
            git::paths_changed(&paths),
        );
        log::info!(
            "ssh git worktree comparison complete workspace={} path={} rows={} elapsed_ms={}",
            self.workspace.display_name,
            file_path,
            comparison.rows.len(),
            start.elapsed().as_millis()
        );
        Ok(comparison)
    }

    fn bytes_comparison(&self, file_path: &str) -> Result<git::BytesComparison, String> {
        let paths = self.worktree_file_path_pair(file_path)?;
        let old_path = paths.old_path.as_deref().unwrap_or(file_path);
        let new_path = paths.new_path.as_deref().unwrap_or(file_path);
        Ok(git::BytesComparison::from_parts(
            self.tree_file_binary_bytes_opt(Some("HEAD"), old_path)?,
            self.workdir_binary_bytes(new_path)?,
        ))
    }

    fn commit_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::FileComparison, String> {
        log::debug!(
            "ssh git commit comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        let start = Instant::now();
        let paths = self.commit_file_path_pair(hash, file_path)?;
        let old_path = paths.old_path.as_deref().unwrap_or(file_path);
        let new_path = paths.new_path.as_deref().unwrap_or(file_path);
        let parent = self.commit_parent_hash(hash)?;
        let left_lines = self.tree_file_text_lines_opt(parent.as_deref(), old_path)?;
        let right_lines = self.tree_file_text_lines_opt(Some(hash), new_path)?;
        let diff = self.commit_diff(hash, &paths, file_path)?;
        let comparison = git::comparison_from_unified_diff(
            &diff,
            &left_lines,
            &right_lines,
            git::paths_changed(&paths),
        );
        log::info!(
            "ssh git commit comparison complete workspace={} hash={} path={} rows={} elapsed_ms={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path,
            comparison.rows.len(),
            start.elapsed().as_millis()
        );
        Ok(comparison)
    }

    fn commit_bytes_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::BytesComparison, String> {
        log::debug!(
            "ssh git commit bytes comparison start workspace={} hash={} path={}",
            self.workspace.display_name,
            short_hash(hash),
            file_path
        );
        let paths = self.commit_file_path_pair(hash, file_path)?;
        let old_path = paths.old_path.as_deref().unwrap_or(file_path);
        let new_path = paths.new_path.as_deref().unwrap_or(file_path);
        let parent = self.commit_parent_hash(hash)?;
        let before = self.tree_file_binary_bytes_opt(parent.as_deref(), old_path)?;
        let after = self.tree_file_binary_bytes_opt(Some(hash), new_path)?;
        Ok(git::BytesComparison::from_parts(before, after))
    }
}

impl SshGitAccess {
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
                kind: git::BranchKind::Local,
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
        let script = format!(
            "cd {} && git --no-optional-locks status --untracked-files=all --branch --porcelain=2 -z",
            shell_quote(&self.workspace.root.absolute)
        );
        let output = self.runner.run_script("git status", &script)?;
        let mut files = git::parse_porcelain_status_entries(&output.stdout)
            .into_iter()
            .filter(git::status_entry_visible)
            .map(|entry| git::changed_file_from_porcelain_entry(&entry))
            .collect::<Vec<_>>();
        files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.status.cmp(&right.status))
        });
        Ok(files)
    }

    fn status_entries(&self) -> Result<Vec<git::GitStatusEntry>, String> {
        let script = format!(
            "cd {} && git --no-optional-locks status --untracked-files=all --branch --porcelain=2 -z",
            shell_quote(&self.workspace.root.absolute)
        );
        let output = self.runner.run_script("git status entries", &script)?;
        Ok(git::parse_porcelain_status_entries(&output.stdout))
    }

    fn worktree_file_path_pair(&self, file_path: &str) -> Result<git::FilePathPair, String> {
        Ok(git::worktree_file_path_pair_from_entries(
            &self.status_entries()?,
            file_path,
        ))
    }

    fn commit_file_path_pair(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::FilePathPair, String> {
        let script = format!(
            "cd {} && git diff-tree --root --no-commit-id --name-status -r -M -z {}",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(hash)
        );
        let output = self.runner.run_script("git commit path pairs", &script)?;
        Ok(git::commit_file_path_pair_from_name_status_bytes(
            &output.stdout,
            file_path,
        ))
    }

    fn worktree_diff(
        &self,
        paths: &git::FilePathPair,
        fallback_path: &str,
    ) -> Result<String, String> {
        if paths.old_path.is_none()
            && let Some(new_path) = paths.new_path.as_deref()
        {
            let script = format!(
                "cd {} || exit 2\n\
                 git diff --no-index --no-ext-diff --no-color --unified=3 -- /dev/null {}\n\
                 status=$?\n\
                 if [ \"$status\" -eq 0 ] || [ \"$status\" -eq 1 ]; then exit 0; fi\n\
                 exit \"$status\"",
                shell_quote(&self.workspace.root.absolute),
                shell_quote(new_path)
            );
            return self.runner.run_text("git worktree no-index diff", &script);
        }

        let mut args = vec![
            "diff".to_string(),
            "HEAD".to_string(),
            "--no-ext-diff".to_string(),
            "--find-renames".to_string(),
            "--no-color".to_string(),
            "--unified=3".to_string(),
        ];
        args.extend(git::diff_args_for_paths(&[
            paths.old_path.as_deref(),
            paths.new_path.as_deref(),
            Some(fallback_path),
        ]));
        self.git(&args)
    }

    fn commit_diff(
        &self,
        hash: &str,
        paths: &git::FilePathPair,
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
        args.extend(git::diff_args_for_paths(&[
            paths.old_path.as_deref(),
            paths.new_path.as_deref(),
            Some(fallback_path),
        ]));
        self.git(&args)
    }

    fn tree_file_text_lines_opt(
        &self,
        rev: Option<&str>,
        file_path: &str,
    ) -> Result<Vec<String>, String> {
        let bytes = self.tree_file_bytes_opt(rev, file_path, git::MAX_TEXT_PREVIEW_BYTES, true)?;
        git::text_preview_lines(bytes.as_deref())
    }

    fn workdir_text_lines(&self, file_path: &str) -> Result<Vec<String>, String> {
        let bytes = self.workdir_file_bytes(file_path, git::MAX_TEXT_PREVIEW_BYTES, true)?;
        git::text_preview_lines(bytes.as_deref())
    }

    fn tree_file_binary_bytes_opt(
        &self,
        rev: Option<&str>,
        file_path: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        self.tree_file_bytes_opt(rev, file_path, git::MAX_BINARY_PREVIEW_BYTES, false)
    }

    fn workdir_binary_bytes(&self, file_path: &str) -> Result<Option<Vec<u8>>, String> {
        self.workdir_file_bytes(file_path, git::MAX_BINARY_PREVIEW_BYTES, false)
    }

    fn tree_file_bytes_opt(
        &self,
        rev: Option<&str>,
        file_path: &str,
        max_bytes: usize,
        text_preview: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(rev) = rev else {
            return Ok(None);
        };
        let spec = format!("{rev}:{file_path}");
        let script = format!(
            "cd {} || exit 2\n\
             spec={}\n\
             if ! git cat-file -e \"$spec\" 2>/dev/null; then printf 'CRAIC_MISSING\\n'; exit 0; fi\n\
             size=$(git cat-file -s \"$spec\") || exit $?\n\
             case \"$size\" in ''|*[!0-9]*) printf 'Invalid git object size: %s\\n' \"$size\" >&2; exit 2 ;; esac\n\
             if [ \"$size\" -gt {} ]; then printf 'CRAIC_TOO_LARGE\\n'; exit 0; fi\n\
             printf 'CRAIC_PRESENT\\n'\n\
             git show \"$spec\"",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(&spec),
            max_bytes
        );
        let output = self.runner.run_script("git tree bytes", &script)?;
        parse_optional_preview_bytes(&output.stdout, file_path, text_preview)
    }

    fn workdir_file_bytes(
        &self,
        file_path: &str,
        max_bytes: usize,
        text_preview: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        let script = format!(
            "cd {} || exit 2\n\
             path={}\n\
             if [ ! -e \"$path\" ] || [ -d \"$path\" ]; then printf 'CRAIC_MISSING\\n'; exit 0; fi\n\
             size=$(wc -c < \"$path\" | tr -d '[:space:]') || exit $?\n\
             case \"$size\" in ''|*[!0-9]*) printf 'Invalid file size: %s\\n' \"$size\" >&2; exit 2 ;; esac\n\
             if [ \"$size\" -gt {} ]; then printf 'CRAIC_TOO_LARGE\\n'; exit 0; fi\n\
             printf 'CRAIC_PRESENT\\n'\n\
             cat -- \"$path\"",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(file_path),
            max_bytes
        );
        let output = self.runner.run_script("git workdir bytes", &script)?;
        parse_optional_preview_bytes(&output.stdout, file_path, text_preview)
    }

    fn commit_target_plan(&self, selected_files: &[String]) -> Result<SshCommitTargetPlan, String> {
        let script = format!(
            "cd {} && git --no-optional-locks status --untracked-files=all --branch --porcelain=2 -z",
            shell_quote(&self.workspace.root.absolute)
        );
        let output = self.runner.run_script("git commit status", &script)?;
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
            "ssh git commit targets resolved workspace={} selected_count={} force_remove_count={} update_count={}",
            self.workspace.display_name,
            selected_files.len(),
            force_remove_paths.len(),
            update_paths.len()
        );

        Ok(SshCommitTargetPlan {
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

fn parse_optional_preview_bytes(
    stdout: &[u8],
    file_path: &str,
    text_preview: bool,
) -> Result<Option<Vec<u8>>, String> {
    let Some(header_end) = stdout.iter().position(|byte| *byte == b'\n') else {
        return Err("Remote preview response did not include a header.".to_string());
    };
    let header = &stdout[..header_end];
    let bytes = &stdout[header_end.saturating_add(1)..];

    match header {
        b"CRAIC_MISSING" => Ok(None),
        b"CRAIC_PRESENT" => Ok(Some(bytes.to_vec())),
        b"CRAIC_TOO_LARGE" => {
            let suffix = if text_preview { " as text" } else { "" };
            Err(format!(
                "{} is too large to preview{}.",
                git::file_name(file_path),
                suffix
            ))
        }
        _ => Err("Remote preview response included an invalid header.".to_string()),
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

fn parse_remote_commit_page(output: &str) -> Result<git::CommitPage, String> {
    let page: RemoteCommitPage = serde_json::from_str(output)
        .map_err(|err| format!("Invalid remote history response: {err}"))?;
    let commits = page
        .commits
        .into_iter()
        .map(remote_commit_row)
        .collect::<Vec<_>>();
    Ok(git::CommitPage {
        commits,
        has_more: page.has_more,
    })
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

fn parse_porcelain_status(bytes: &[u8]) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if entry.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&entry[0..2]).to_string();
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        let label = if status.contains('U') {
            "U"
        } else if status.contains('R') {
            "R"
        } else if status.contains('D') {
            "D"
        } else if status.contains('A') || status.contains('?') {
            "A"
        } else {
            "M"
        };
        files.push(ChangedFile {
            status: label.to_string(),
            path,
            git_status_bits: 0,
            worktree_signature: Some(ChangedFileSignature {
                is_dir: false,
                len: 0,
                modified: Some(UNIX_EPOCH + Duration::from_secs(0)),
            }),
        });
    }
    files
}
