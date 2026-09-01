impl GitRepoHandle {
    pub fn workspace_files(&self) -> Arc<dyn FileAccess> {
        self.files.clone()
    }

    pub fn interactive_shell_command(&self) -> Result<(ShellCommandSpec, String), String> {
        let command = self.shell.interactive_shell(Some(&self.workspace.root))?;
        let title = self.shell.command_display(&command);
        Ok((command, title))
    }

    pub fn interactive_shell_command_at(
        &self,
        directory: &FileNodePath,
    ) -> Result<(ShellCommandSpec, String), String> {
        let working_directory = directory
            .to_workspace_path(&self.workspace)
            .ok_or_else(|| "The terminal directory is outside the active workspace.".to_string())?;
        let command = self.shell.interactive_shell(Some(&working_directory))?;
        let title = self.shell.command_display(&command);
        Ok((command, title))
    }

    pub fn terminal_command(
        &self,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        self.shell.command(&self.workspace.root, program, args)
    }

    pub fn terminal_command_at(
        &self,
        directory: &FileNodePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        let working_directory = directory
            .to_workspace_path(&self.workspace)
            .ok_or_else(|| "The terminal directory is outside the active workspace.".to_string())?;
        self.shell.command(&working_directory, program, args)
    }

    pub fn resolved_terminal_command(
        &self,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        let resolved = self
            .shell
            .which(program)?
            .ok_or_else(|| format!("{program} was not found on the workspace shell path."))?;
        log::debug!("terminal command resolved program={program} path={resolved}");
        self.shell.command(&self.workspace.root, &resolved, args)
    }

    pub fn resolve_workspace_file_link(&self, target: &str) -> Result<TerminalLinkTarget, String> {
        self.terminal_links
            .as_ref()
            .ok_or_else(|| "File-link navigation is unavailable for this workspace.".to_string())?
            .resolve_file(&self.workspace.root.absolute, target)
    }

    pub fn github_commit_email_options(
        &self,
    ) -> Result<Vec<crate::github::CommitEmailOption>, String> {
        let gh = self
            .shell
            .which("gh")?
            .ok_or_else(|| "gh was not found on the user shell path.".to_string())?;
        crate::github::commit_email_options_with_gh(&gh)
    }

    pub fn github_avatar_url_for_email(&self, email: &str) -> Result<String, String> {
        if crate::github::login_from_noreply_email(email).is_some() {
            return crate::github::avatar_url_for_email(email);
        }
        let gh = self
            .shell
            .which("gh")?
            .ok_or_else(|| "gh was not found on the user shell path.".to_string())?;
        crate::github::avatar_url_for_email_with_gh(email, &gh)
    }

    pub fn load_repository_snapshot(&self) -> Result<RepositorySnapshot, String> {
        self.repository_snapshot_blocking()
    }

    pub fn load_workspace_snapshot(&self) -> Result<WorkspaceSnapshot, String> {
        self.workspace_snapshot_blocking()
    }

    pub fn snapshot(&self) -> GitOperationReceiver<RepositorySnapshot> {
        let handle = self.clone();
        run_operation("git snapshot", move || {
            handle.repository_snapshot_blocking()
        })
    }

    pub fn workspace_snapshot(&self) -> GitOperationReceiver<WorkspaceSnapshot> {
        let handle = self.clone();
        run_operation("git workspace snapshot", move || {
            handle.workspace_snapshot_blocking()
        })
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
    ) -> GitOperationReceiver<git::WorkspaceRepositoryMetadata> {
        let handle = self.clone();
        run_operation("git workspace metadata", move || {
            handle.run_with_hooks("git workspace metadata", || {
                Ok(handle.workspace_metadata_blocking(github.as_deref()))
            })
        })
    }

    pub async fn workspace_metadata_async(
        &self,
        github: Option<Arc<dyn GitHubAccess>>,
    ) -> Result<git::WorkspaceRepositoryMetadata, String> {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || {
            handle.run_with_hooks("git workspace metadata", || {
                Ok(handle.workspace_metadata_blocking(github.as_deref()))
            })
        })
        .await
        .map_err(|error| format!("Git workspace metadata task did not complete: {error}"))?
    }

    pub fn initialize_repository(&self) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git initialize repository", move || {
            handle.initialize_repository_blocking()
        })
    }

    pub async fn initialize_repository_async(&self) -> Result<String, String> {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || handle.initialize_repository_blocking())
            .await
            .map_err(|error| format!("Git initialization task did not complete: {error}"))?
    }

    pub fn commit_message_context(
        &self,
        files: &[String],
    ) -> GitOperationReceiver<CommitMessageContext> {
        let handle = self.clone();
        let files = files.to_vec();
        run_operation("git commit message context", move || {
            handle.commit_message_context_blocking(&files)
        })
    }

    pub async fn commit_message_context_async(
        &self,
        files: &[String],
    ) -> Result<CommitMessageContext, String> {
        let handle = self.clone();
        let files = files.to_vec();
        tokio::task::spawn_blocking(move || handle.commit_message_context_blocking(&files))
            .await
            .map_err(|error| format!("Git commit-message context task did not complete: {error}"))?
    }

    pub fn commit_paths(
        &self,
        summary: &str,
        description: &str,
        files: &[String],
    ) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        let files = files.to_vec();
        run_operation("git commit", move || {
            handle.commit_paths_blocking(&summary, &description, &files)
        })
    }

    pub async fn commit_paths_async(
        &self,
        summary: &str,
        description: &str,
        files: &[String],
    ) -> Result<String, String> {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        let files = files.to_vec();
        tokio::task::spawn_blocking(move || {
            handle.commit_paths_blocking(&summary, &description, &files)
        })
        .await
        .map_err(|error| format!("Git commit task did not complete: {error}"))?
    }

    pub fn discard_path(&self, file_path: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let file_path = file_path.to_string();
        run_operation("git discard", move || {
            handle.discard_path_blocking(&file_path)
        })
    }

    pub async fn discard_path_async(&self, file_path: &str) -> Result<String, String> {
        let handle = self.clone();
        let file_path = file_path.to_string();
        tokio::task::spawn_blocking(move || handle.discard_path_blocking(&file_path))
            .await
            .map_err(|error| format!("Git discard task did not complete: {error}"))?
    }

    pub fn check_ignored_paths(
        &self,
        checks: &[gitignore::IgnoreCheck],
    ) -> GitOperationReceiver<HashSet<String>> {
        let handle = self.clone();
        let checks = checks.to_vec();
        run_operation("git check ignored paths", move || {
            handle.check_ignored_paths_blocking(&checks)
        })
    }

    pub fn settings(&self) -> GitOperationReceiver<GitSettings> {
        let handle = self.clone();
        run_operation("git settings", move || Ok(handle.settings_blocking()))
    }

    pub async fn settings_async(&self) -> Result<GitSettings, String> {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || handle.settings_blocking())
            .await
            .map_err(|error| format!("Git settings task did not complete: {error}"))
    }

    pub fn save_settings(&self, settings: &GitSettings) -> GitOperationReceiver<()> {
        let handle = self.clone();
        let settings = settings.clone();
        run_operation("git save settings", move || {
            handle.save_settings_blocking(&settings)
        })
    }

    pub async fn save_settings_async(&self, settings: &GitSettings) -> Result<(), String> {
        let handle = self.clone();
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || handle.save_settings_blocking(&settings))
            .await
            .map_err(|error| format!("Git settings save task did not complete: {error}"))?
    }

    pub fn save_author_identity(&self, name: &str, email: &str) -> GitOperationReceiver<()> {
        let handle = self.clone();
        let name = name.to_string();
        let email = email.to_string();
        run_operation("git save author identity", move || {
            handle.save_author_identity_blocking(&name, &email)
        })
    }

    pub async fn save_author_identity_async(&self, name: &str, email: &str) -> Result<(), String> {
        let handle = self.clone();
        let name = name.to_string();
        let email = email.to_string();
        tokio::task::spawn_blocking(move || handle.save_author_identity_blocking(&name, &email))
            .await
            .map_err(|error| format!("Git author update task did not complete: {error}"))?
    }

    pub fn push(&self) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git push", move || handle.push_blocking())
    }

    pub fn push_with_progress(&self) -> GitCommandGenerator {
        let handle = self.clone();
        git_command_generator("git push", move |progress| {
            handle.push_with_progress_blocking(progress)
        })
    }

    pub fn pull(&self) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git pull", move || handle.pull_blocking())
    }

    pub fn pull_with_progress(&self) -> GitCommandGenerator {
        let handle = self.clone();
        git_command_generator("git pull", move |progress| {
            handle.pull_with_progress_blocking(progress)
        })
    }

    pub fn pull_push_with_progress(&self) -> GitCommandGenerator {
        let handle = self.clone();
        git_command_generator("git pull and push", move |progress| {
            handle.pull_with_progress_blocking(progress)?;
            handle
                .push_with_progress_blocking(progress)
                .map_err(|err| format!("Pull succeeded, but Push failed: {err}"))
        })
    }

    pub fn publish(&self, remote: &str, branch: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let remote = remote.to_string();
        let branch = branch.to_string();
        run_operation("git publish", move || {
            handle.publish_blocking(&remote, &branch)
        })
    }

    pub fn publish_with_progress(&self, remote: &str, branch: &str) -> GitCommandGenerator {
        let handle = self.clone();
        let remote = remote.to_string();
        let branch = branch.to_string();
        git_command_generator("git publish", move |progress| {
            handle.publish_with_progress_blocking(&remote, &branch, progress)
        })
    }

    pub fn fetch_with_progress(&self, remote: Option<&str>) -> GitCommandGenerator {
        let handle = self.clone();
        let remote = remote.map(ToString::to_string);
        git_command_generator("git fetch", move |progress| {
            handle.fetch_with_progress_blocking(remote.as_deref(), progress)
        })
    }

    pub fn checkout_branch(&self, branch: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let branch = branch.to_string();
        run_operation("git checkout branch", move || {
            handle.checkout_branch_blocking(&branch)
        })
    }

    pub async fn checkout_branch_async(&self, branch: &str) -> Result<String, String> {
        let handle = self.clone();
        let branch = branch.to_string();
        tokio::task::spawn_blocking(move || handle.checkout_branch_blocking(&branch))
            .await
            .map_err(|error| format!("Git checkout task did not complete: {error}"))?
    }

    pub fn checkout_remote_branch(
        &self,
        remote_branch: &str,
        local_branch: &str,
    ) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let remote_branch = remote_branch.to_string();
        let local_branch = local_branch.to_string();
        run_operation("git checkout remote branch", move || {
            handle.checkout_remote_branch_blocking(&remote_branch, &local_branch)
        })
    }

    pub fn checkout_pull_request(&self, number: u32) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git checkout pull request", move || {
            handle.checkout_pull_request_blocking(number)
        })
    }

    pub fn create_branch(&self, branch: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let branch = branch.to_string();
        run_operation("git create branch", move || {
            handle.create_branch_blocking(&branch)
        })
    }

    pub async fn create_branch_async(&self, branch: &str) -> Result<String, String> {
        let handle = self.clone();
        let branch = branch.to_string();
        tokio::task::spawn_blocking(move || handle.create_branch_blocking(&branch))
            .await
            .map_err(|error| format!("Git branch-creation task did not complete: {error}"))?
    }

    pub fn merge_branch(&self, branch: &str) -> GitOperationReceiver<git::MergeResult> {
        let handle = self.clone();
        let branch = branch.to_string();
        run_operation("git merge branch", move || {
            handle.merge_branch_blocking(&branch)
        })
    }

    pub async fn merge_branch_async(&self, branch: &str) -> Result<git::MergeResult, String> {
        let handle = self.clone();
        let branch = branch.to_string();
        tokio::task::spawn_blocking(move || handle.merge_branch_blocking(&branch))
            .await
            .map_err(|error| format!("Git merge task did not complete: {error}"))?
    }

    pub fn checkout_commit(&self, hash: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git checkout commit", move || {
            handle.checkout_commit_blocking(&hash)
        })
    }

    pub async fn checkout_commit_async(&self, hash: &str) -> Result<String, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.checkout_commit_blocking(&hash))
            .await
            .map_err(|error| format!("Git commit-checkout task did not complete: {error}"))?
    }

    pub fn create_branch_at_commit(
        &self,
        branch: &str,
        hash: &str,
    ) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let branch = branch.to_string();
        let hash = hash.to_string();
        run_operation("git create branch at commit", move || {
            handle.create_branch_at_commit_blocking(&branch, &hash)
        })
    }

    pub async fn create_branch_at_commit_async(
        &self,
        branch: &str,
        hash: &str,
    ) -> Result<String, String> {
        let handle = self.clone();
        let branch = branch.to_string();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.create_branch_at_commit_blocking(&branch, &hash))
            .await
            .map_err(|error| format!("Git branch-creation task did not complete: {error}"))?
    }

    pub fn create_tag(&self, tag: &str, hash: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let tag = tag.to_string();
        let hash = hash.to_string();
        run_operation("git create tag", move || {
            handle.create_tag_blocking(&tag, &hash)
        })
    }

    pub async fn create_tag_async(&self, tag: &str, hash: &str) -> Result<String, String> {
        let handle = self.clone();
        let tag = tag.to_string();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.create_tag_blocking(&tag, &hash))
            .await
            .map_err(|error| format!("Git tag-creation task did not complete: {error}"))?
    }

    pub fn reset_to_commit(
        &self,
        hash: &str,
        mode: git::ResetMode,
    ) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git reset", move || {
            handle.reset_to_commit_blocking(&hash, mode)
        })
    }

    pub async fn reset_to_commit_async(
        &self,
        hash: &str,
        mode: git::ResetMode,
    ) -> Result<String, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.reset_to_commit_blocking(&hash, mode))
            .await
            .map_err(|error| format!("Git reset task did not complete: {error}"))?
    }

    pub fn revert_commit(&self, hash: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git revert", move || handle.revert_commit_blocking(&hash))
    }

    pub async fn revert_commit_async(&self, hash: &str) -> Result<String, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.revert_commit_blocking(&hash))
            .await
            .map_err(|error| format!("Git revert task did not complete: {error}"))?
    }

    pub fn cherry_pick_commit(&self, hash: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git cherry-pick", move || {
            handle.cherry_pick_commit_blocking(&hash)
        })
    }

    pub async fn cherry_pick_commit_async(&self, hash: &str) -> Result<String, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.cherry_pick_commit_blocking(&hash))
            .await
            .map_err(|error| format!("Git cherry-pick task did not complete: {error}"))?
    }

    pub fn amend_head(&self, summary: &str, description: &str) -> GitOperationReceiver<String> {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        run_operation("git amend head", move || {
            handle.amend_head_blocking(&summary, &description)
        })
    }

    pub async fn amend_head_async(
        &self,
        summary: &str,
        description: &str,
    ) -> Result<String, String> {
        let handle = self.clone();
        let summary = summary.to_string();
        let description = description.to_string();
        tokio::task::spawn_blocking(move || handle.amend_head_blocking(&summary, &description))
            .await
            .map_err(|error| format!("Git amend task did not complete: {error}"))?
    }

    pub fn stash_changes(&self) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git stash", move || handle.stash_changes_blocking())
    }

    pub async fn stash_changes_async(&self) -> Result<String, String> {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || handle.stash_changes_blocking())
            .await
            .map_err(|error| format!("Git stash task did not complete: {error}"))?
    }

    pub fn add_ignore_pattern(&self, pattern: String) -> GitOperationReceiver<String> {
        let receiver = gitignore::add_pattern_to_workspace(self.files.clone(), pattern);
        run_operation("git ignore", move || {
            receiver
                .recv()
                .unwrap_or_else(|_| Err("Ignore operation ended without a result.".to_string()))
        })
    }

    pub async fn add_ignore_pattern_async(&self, pattern: String) -> Result<String, String> {
        let receiver = gitignore::add_pattern_to_workspace(self.files.clone(), pattern);
        tokio::task::spawn_blocking(move || {
            receiver
                .recv()
                .unwrap_or_else(|_| Err("Ignore operation ended without a result.".to_string()))
        })
        .await
        .map_err(|error| format!("Ignore operation task did not complete: {error}"))?
    }

    pub fn pop_stash(&self) -> GitOperationReceiver<String> {
        let handle = self.clone();
        run_operation("git stash pop", move || handle.pop_stash_blocking())
    }

    pub fn commit_page(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> GitOperationReceiver<git::CommitPage> {
        let handle = self.clone();
        let after = after.map(ToString::to_string);
        run_operation("git commit page", move || {
            handle.commit_page_blocking(after.as_deref(), limit)
        })
    }

    pub fn commit_search_page(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> GitOperationReceiver<git::CommitPage> {
        let handle = self.clone();
        let query = query.to_string();
        let after = after.map(ToString::to_string);
        run_operation("git commit search page", move || {
            handle.commit_search_page_blocking(&query, after.as_deref(), limit)
        })
    }

    pub async fn commit_page_async(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        let handle = self.clone();
        let after = after.map(ToString::to_string);
        tokio::task::spawn_blocking(move || handle.commit_page_blocking(after.as_deref(), limit))
            .await
            .map_err(|error| format!("Git history page task did not complete: {error}"))?
    }

    pub async fn commit_search_page_async(
        &self,
        query: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<git::CommitPage, String> {
        let handle = self.clone();
        let query = query.to_string();
        let after = after.map(ToString::to_string);
        tokio::task::spawn_blocking(move || {
            handle.commit_search_page_blocking(&query, after.as_deref(), limit)
        })
        .await
        .map_err(|error| format!("Git history search task did not complete: {error}"))?
    }

    pub fn commit_details(&self, hash: &str) -> GitOperationReceiver<git::Commit> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit details", move || {
            handle.commit_details_blocking(&hash)
        })
    }

    pub async fn commit_details_async(&self, hash: &str) -> Result<git::Commit, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.commit_details_blocking(&hash))
            .await
            .map_err(|error| format!("Git commit-details task did not complete: {error}"))?
    }

    pub fn commit_message(&self, hash: &str) -> GitOperationReceiver<git::CommitMessage> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit message", move || {
            handle.commit_message_blocking(&hash)
        })
    }

    pub fn commit_parent_hash(&self, hash: &str) -> GitOperationReceiver<Option<String>> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit parent hash", move || {
            handle.commit_parent_hash_blocking(&hash)
        })
    }

    pub async fn commit_parent_hash_async(&self, hash: &str) -> Result<Option<String>, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.commit_parent_hash_blocking(&hash))
            .await
            .map_err(|error| format!("Git parent-hash task did not complete: {error}"))?
    }

    pub fn commit_changed_files(&self, hash: &str) -> GitOperationReceiver<Vec<git::ChangedFile>> {
        let handle = self.clone();
        let hash = hash.to_string();
        run_operation("git commit changed files", move || {
            handle.commit_changed_files_blocking(&hash)
        })
    }

    pub async fn commit_changed_files_async(
        &self,
        hash: &str,
    ) -> Result<Vec<git::ChangedFile>, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || handle.commit_changed_files_blocking(&hash))
            .await
            .map_err(|error| format!("Git changed-files task did not complete: {error}"))?
    }

    pub fn comparison(&self, file_path: &str) -> GitOperationReceiver<git::FileComparison> {
        let handle = self.clone();
        let file_path = file_path.to_string();
        run_operation("git comparison", move || {
            handle.comparison_blocking(&file_path)
        })
    }

    pub async fn comparison_async(&self, file_path: &str) -> Result<git::FileComparison, String> {
        let handle = self.clone();
        let file_path = file_path.to_string();
        tokio::task::spawn_blocking(move || handle.comparison_blocking(&file_path))
            .await
            .map_err(|error| format!("Git comparison task did not complete: {error}"))?
    }

    pub async fn bytes_comparison_async(
        &self,
        file_path: &str,
    ) -> Result<git::BytesComparison, String> {
        let handle = self.clone();
        let file_path = file_path.to_string();
        tokio::task::spawn_blocking(move || handle.bytes_comparison_blocking(&file_path))
            .await
            .map_err(|error| format!("Git byte-comparison task did not complete: {error}"))?
    }

    pub fn watch_comparison(
        &self,
        file_path: &str,
    ) -> (FileDiffSubscription, FileDiffReceiver<git::FileComparison>) {
        self.watch_file_diff(file_path, |handle, file_path| {
            handle.comparison_blocking(file_path)
        })
    }

    pub fn watch_bytes_comparison(
        &self,
        file_path: &str,
    ) -> (FileDiffSubscription, FileDiffReceiver<git::BytesComparison>) {
        self.watch_file_diff(file_path, |handle, file_path| {
            handle.bytes_comparison_blocking(file_path)
        })
    }

    pub fn commit_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> GitOperationReceiver<git::FileComparison> {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        run_operation("git commit comparison", move || {
            handle.commit_comparison_blocking(&hash, &file_path)
        })
    }

    pub async fn commit_comparison_async(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::FileComparison, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        tokio::task::spawn_blocking(move || handle.commit_comparison_blocking(&hash, &file_path))
            .await
            .map_err(|error| format!("Git commit-comparison task did not complete: {error}"))?
    }

    pub fn commit_bytes_comparison(
        &self,
        hash: &str,
        file_path: &str,
    ) -> GitOperationReceiver<git::BytesComparison> {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        run_operation("git commit bytes comparison", move || {
            handle.commit_bytes_comparison_blocking(&hash, &file_path)
        })
    }

    pub async fn commit_bytes_comparison_async(
        &self,
        hash: &str,
        file_path: &str,
    ) -> Result<git::BytesComparison, String> {
        let handle = self.clone();
        let hash = hash.to_string();
        let file_path = file_path.to_string();
        tokio::task::spawn_blocking(move || {
            handle.commit_bytes_comparison_blocking(&hash, &file_path)
        })
        .await
        .map_err(|error| format!("Git commit byte-comparison task did not complete: {error}"))?
    }

    fn watch_file_diff<T, F>(
        &self,
        file_path: &str,
        load: F,
    ) -> (FileDiffSubscription, FileDiffReceiver<T>)
    where
        T: Send + Sync + 'static,
        F: FnMut(&GitRepoHandle, &str) -> Result<T, String> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let wake_sender = sender.clone();
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
            wake_sender,
            load,
        )
    }
}
