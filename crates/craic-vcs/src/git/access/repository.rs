impl GitRepoHandle {
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
        let remote_name = self.primary_remote_name();
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
            last_fetch_at: self.last_fetch_at(),
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

        let config = crate::workspace_config::git_config_from_file_access(self.files.as_ref());
        let timezone = match config.commit_timezone {
            Some(timezone) => Some(crate::workspace_config::normalize_timezone(&timezone)?),
            None if config.use_system_timezone.unwrap_or(false) => None,
            None => Some(DEFAULT_COMMIT_TIMEZONE.to_string()),
        };
        let git_date = match timezone {
            Some(timezone) => {
                let seconds = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|err| err.to_string())?
                    .as_secs();
                log::debug!("using commit timezone {timezone}");
                format!("@{seconds} {timezone}")
            }
            None => {
                log::debug!("using system timezone for commit");
                String::new()
            }
        };

        if self
            .git_ok(&["rev-parse".into(), "--verify".into(), "HEAD".into()])
            .is_ok()
        {
            self.git(&["reset".into(), "--".into(), ".".into()])?;
        } else {
            self.git(&[
                "rm".into(),
                "--cached".into(),
                "-r".into(),
                "--ignore-unmatch".into(),
                ".".into(),
            ])?;
        }

        if !plan.force_remove_paths.is_empty() {
            let mut paths = plan.force_remove_paths.join("\0").into_bytes();
            paths.push(0);
            self.run_command_output(
                "git commit remove paths",
                "git",
                &[
                    "update-index".into(),
                    "--force-remove".into(),
                    "-z".into(),
                    "--stdin".into(),
                ],
                Some(&paths),
                &[0],
            )?;
        }

        if !plan.update_paths.is_empty() {
            let mut paths = plan.update_paths.join("\0").into_bytes();
            paths.push(0);
            self.run_command_output(
                "git commit update paths",
                "git",
                &[
                    "update-index".into(),
                    "--add".into(),
                    "--remove".into(),
                    "--replace".into(),
                    "-z".into(),
                    "--stdin".into(),
                ],
                Some(&paths),
                &[0],
            )?;
        }

        let script = shell_script_with_args(COMMIT_SELECTED_SCRIPT, &[git_date]);

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
        let global_user_name = self
            .git_ok(&[
                "config".into(),
                "--global".into(),
                "--get".into(),
                "user.name".into(),
            ])
            .ok();
        let global_user_email = self
            .git_ok(&[
                "config".into(),
                "--global".into(),
                "--get".into(),
                "user.email".into(),
            ])
            .ok();
        let local_user_name = self
            .git_ok(&[
                "config".into(),
                "--local".into(),
                "--get".into(),
                "user.name".into(),
            ])
            .ok();
        let local_user_email = self
            .git_ok(&[
                "config".into(),
                "--local".into(),
                "--get".into(),
                "user.email".into(),
            ])
            .ok();
        let use_global_user = local_user_name.is_none() && local_user_email.is_none();
        GitSettings {
            global_user_name,
            global_user_email,
            local_user_name,
            local_user_email,
            use_global_user,
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

    fn push_with_progress_blocking(
        &self,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        self.run_with_hooks("git push", || {
            self.git_with_progress(&["push".into()], progress)
        })
    }

    fn pull_blocking(&self) -> Result<String, String> {
        let result = self.run_with_hooks("git pull", || {
            self.git(&[
                "-c".into(),
                "rebase.backend=merge".into(),
                "pull".into(),
                "--ff".into(),
                "--recurse-submodules".into(),
            ])
        });
        if result.is_ok() {
            self.record_fetch();
        }
        result
    }

    fn pull_with_progress_blocking(
        &self,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        let result = self.run_with_hooks("git pull", || {
            self.git_with_progress(
                &[
                    "-c".into(),
                    "rebase.backend=merge".into(),
                    "pull".into(),
                    "--ff".into(),
                    "--recurse-submodules".into(),
                ],
                progress,
            )
        });
        if result.is_ok() {
            self.record_fetch();
        }
        result
    }

    fn publish_blocking(&self, remote: &str, branch: &str) -> Result<String, String> {
        self.run_with_hooks("git publish", || {
            self.git(&["push".into(), "-u".into(), remote.into(), branch.into()])
        })
    }

    fn publish_with_progress_blocking(
        &self,
        remote: &str,
        branch: &str,
        progress: &mut dyn FnMut(String),
    ) -> Result<String, String> {
        self.run_with_hooks("git publish", || {
            self.git_with_progress(
                &["push".into(), "-u".into(), remote.into(), branch.into()],
                progress,
            )
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
        let result = self.run_with_hooks("git fetch", || self.git_with_progress(&args, progress));
        if result.is_ok() {
            self.record_fetch();
        }
        result
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

    fn merge_branch_blocking(&self, branch: &str) -> Result<git::MergeResult, String> {
        log::info!(
            "shell git merge branch start workspace={} branch={}",
            self.workspace.display_name,
            branch
        );
        self.run_with_hooks("git merge branch", || {
            let output = self.run_command_output(
                "git merge branch",
                "git",
                &["merge".into(), branch.into()],
                None,
                &[0, 1],
            )?;
            let stdout = output.stdout_text_trimmed();
            let stderr = output.stderr_text_trimmed();

            if output.status_code == Some(0) {
                return if stdout == "Already up to date." {
                    Ok(git::MergeResult::AlreadyUpToDate)
                } else {
                    Ok(git::MergeResult::Success)
                };
            }

            if self
                .git_ok(&[
                    "rev-parse".into(),
                    "-q".into(),
                    "--verify".into(),
                    "MERGE_HEAD".into(),
                ])
                .is_ok()
            {
                let message = if stderr.is_empty() { stdout } else { stderr };
                return Ok(git::MergeResult::Conflicts(message));
            }

            let message = if stderr.is_empty() { stdout } else { stderr };
            Err(if message.is_empty() {
                "Git merge failed.".to_string()
            } else {
                message
            })
        })
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
