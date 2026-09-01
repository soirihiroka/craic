impl AppDelegate {
    fn begin_workspace_transition(&self, workspace_id: &str) {
        log::info!("native workspace transition started workspace={workspace_id}");
        self.ivars().page_service_requests.borrow_mut().clear();
        if !self.ivars().terminal_sessions.borrow().is_empty() {
            log::info!("stopping workspace-scoped native terminal before workspace transition");
            self.shutdown_all_native_terminals();
        }
        self.cancel_commit_message_generation();
        self.ivars()
            .active_workspace_id
            .replace(Some(workspace_id.to_string()));
        self.ivars().workspace_handle.replace(None);
        self.ivars().git_handle.replace(None);
        if let Some(settings) = self.ivars().commit_message_settings.get() {
            settings
                .workspace_request_id
                .set(settings.workspace_request_id.get().wrapping_add(1));
            settings.workspace_settings.borrow_mut().take();
            settings.workspace_loading.set(false);
            unsafe { settings.workspace_spinner.stopAnimation(None) };
            settings
                .workspace_status
                .setStringValue(&NSString::from_str(
                    "Loading the selected workspace’s Git settings…",
                ));
            self.update_workspace_settings_control_state();
        }
        self.clear_native_quick_actions("Loading workspace…");
        self.ivars().files_monitor.borrow_mut().take();
        self.ivars().repository_background_pull.borrow_mut().take();
        self.ivars().repository_monitor.borrow_mut().take();
        self.ivars().repository_snapshot.replace(None);
        self.ivars().repository_loading.set(true);
        self.ivars()
            .repository_initialization_in_progress
            .set(false);
        if let Some(agents) = self.ivars().agents.get() {
            agents
                .generation
                .set(agents.generation.get().wrapping_add(1));
        }
        if let Some(commands) = self.ivars().agent_commands.get()
            && let Err(error) = commands.try_send(NativeAgentCommand::Reset)
        {
            log::warn!("native Codex workspace reset queue failed: {error}");
        }
        self.reset_native_agent_ui();
        self.ivars().changes_filter_query.borrow_mut().clear();
        self.ivars().changes_search_visible.set(false);
        if let Some(search) = self.ivars().changes_search.get() {
            search.setStringValue(&NSString::new());
        }
        if let Some(popover) = self.ivars().branch_popover.get() {
            popover.close();
        }
        if let Some(popover) = self.ivars().author_popover.get() {
            popover.close();
        }
        if let Some(popover) = self.ivars().author_warning_popover.borrow_mut().take() {
            popover.close();
        }
        self.clear_changed_files("Loading workspace…");
        self.reset_history("Loading workspace…");
        self.clear_font_preview();
        self.clear_sqlite_preview();
        if let Some(files) = self.ivars().files.get() {
            files.generation.set(files.generation.get().wrapping_add(1));
            files
                .preview_request_id
                .set(files.preview_request_id.get().wrapping_add(1));
            files.rows.borrow_mut().clear();
            files.dirty.set(false);
            files.expanded.borrow_mut().clear();
            files.selected_path.borrow_mut().take();
            files
                .drop_hover_generation
                .set(files.drop_hover_generation.get().wrapping_add(1));
            files.drop_hover_path.borrow_mut().take();
            files.loaded_text_path.borrow_mut().take();
            files.loaded_text_signature.borrow_mut().take();
            files.text_buffer.borrow_mut().clear();
            files.text_selection.set(NSRange::new(0, 0));
            files.text_editable.set(false);
            files.preview_code.clear_completions();
            files.pending_text_selection.borrow_mut().take();
            files
                .text_edit_generation
                .set(files.text_edit_generation.get().wrapping_add(1));
            files.text_dirty.set(false);
            files.text_save_in_progress.set(false);
            files.preview_text.setEditable(false);
            files.table.reloadData();
            files.status.setHidden(false);
            files.mutation_in_progress.set(false);
            files
                .status
                .setStringValue(&NSString::from_str("Loading workspace…"));
            files
                .title
                .setStringValue(&NSString::from_str("Select a file"));
            files.metadata_base.borrow_mut().clear();
            files.metadata.setStringValue(&NSString::new());
            files.empty.setStringValue(&NSString::from_str(
                "Select a file or folder from the workspace tree.",
            ));
            files.empty.setHidden(false);
            files.preview_scroll.setHidden(true);
            files.preview_image.setHidden(true);
            files.preview_image.clear_image();
            files.preview_pdf.setHidden(true);
            // SAFETY: Workspace transitions run on AppKit's main thread.
            unsafe { files.preview_pdf.setDocument(None) };
            self.clear_csv_table_preview();
            files.preview_web_mode.set(NativeWebPreviewMode::Hidden);
            files.preview_web.setHidden(true);
            files.preview_divider.setHidden(true);
            // SAFETY: Workspace transitions run on AppKit's main thread.
            unsafe { files.preview_web.stopLoading() };
            unsafe { files.preview_spinner.stopAnimation(None) };
            files.preview_spinner.setHidden(true);
        }
        if let Some(containers) = self.ivars().containers.get() {
            containers
                .generation
                .set(containers.generation.get().wrapping_add(1));
            containers
                .detail_request_id
                .set(containers.detail_request_id.get().wrapping_add(1));
            containers
                .action_request_id
                .set(containers.action_request_id.get().wrapping_add(1));
            containers.rows.borrow_mut().clear();
            containers.expanded_groups.borrow_mut().clear();
            containers.selected_id.borrow_mut().take();
            containers.selected_group_key.borrow_mut().take();
            containers.query.borrow_mut().clear();
            containers.search.setStringValue(&NSString::new());
            containers.table.reloadData();
            containers.scroll.setHidden(true);
            containers.dirty.set(true);
            containers.loading.set(false);
            containers.action_in_progress.set(false);
            unsafe { containers.spinner.stopAnimation(None) };
            containers.spinner.setHidden(true);
            containers.status.setHidden(false);
            containers
                .status
                .setStringValue(&NSString::from_str("Loading workspace…"));
            containers
                .title
                .setStringValue(&NSString::from_str("Containers"));
            containers.subtitle.setStringValue(&NSString::new());
            containers.details_scroll.setHidden(true);
            containers.inspect_code.setHidden(true);
            containers.inspect.setHidden(true);
            containers.empty.setStringValue(&NSString::from_str(
                "Select a container or Compose project.",
            ));
            containers.empty.setHidden(false);
            for button in [
                &containers.logs,
                &containers.inspect,
                &containers.shell,
                &containers.start,
                &containers.stop,
                &containers.restart,
                &containers.remove,
            ] {
                button.setEnabled(false);
            }
        }
        self.set_repository_controls_unavailable("Loading workspace…");
        self.update_commit_composer_state();
        self.layout_sidebar();
    }

    fn request_repository_load(&self, workspace: craic_config::ConfiguredWorkspace) {
        let workspace_id = workspace.selection_id();
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        let Some(requests) = self.ivars().repository_requests.get() else {
            let message = "Repository service is unavailable.".to_string();
            log::warn!("repository load ignored because repository service is unavailable");
            self.apply_repository_snapshot(&workspace_id, None, None, Err(message));
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::Load {
            workspace,
            cancellation,
        }) {
            let message = format!("Unable to queue workspace load: {error}");
            log::warn!("repository load queue rejected request error={error}");
            self.apply_repository_snapshot(&workspace_id, None, None, Err(message));
        }
    }

    fn apply_repository_snapshot(
        &self,
        workspace_id: &str,
        handle: Option<Arc<GitRepoHandle>>,
        core_request: Option<RepositoryCoreRefreshRequest>,
        result: Result<WorkspaceSnapshot, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            log::debug!("discarding stale repository snapshot workspace={workspace_id}");
            return;
        }
        if let Some(RepositoryCoreRefreshRequest::Workspace(request)) = core_request.as_ref()
            && request.identity.workspace_generation != self.ivars().workspace_generation.get()
        {
            log::debug!(
                "discarding stale native workspace refresh request={} generation={}",
                request.identity.request_id,
                request.identity.workspace_generation.get()
            );
            self.complete_workspace_refresh_cancelled(request.identity);
            return;
        }
        let completes_workspace_refresh = matches!(
            core_request.as_ref(),
            Some(RepositoryCoreRefreshRequest::Workspace(_))
        );
        let page_service_result = result
            .as_ref()
            .map(|_| serde_json::Value::Null)
            .map_err(Clone::clone);
        let first_workspace_snapshot = self.ivars().workspace_handle.borrow().is_none();
        self.ivars().repository_loading.set(false);
        if result.is_ok() {
            self.ivars().workspace_handle.replace(handle.clone());
            self.request_native_quick_actions();
        }
        match result {
            Ok(WorkspaceSnapshot::Repository(snapshot)) => {
                let history_changed = self
                    .ivars()
                    .repository_snapshot
                    .borrow()
                    .as_ref()
                    .and_then(|previous| previous.history_head.as_ref())
                    != snapshot.history_head.as_ref();
                if history_changed {
                    self.reset_history("Loading commits...");
                }
                self.ivars().file_preview_cache.borrow_mut().clear();
                self.ivars().git_handle.replace(handle.clone());
                if self.ivars().repository_monitor.borrow().is_none()
                    && let (Some(handle), Some(requests)) =
                        (handle, self.ivars().repository_requests.get().cloned())
                {
                    if let Some(cancellation) = self.workspace_cancellation_token() {
                        let monitored_workspace = workspace_id.to_string();
                        let monitored_handle = handle.clone();
                        let listener: ChangeListener = Arc::new(move || {
                            if let Err(error) = requests.try_send(RepositoryRequest::Refresh {
                                workspace_id: monitored_workspace.clone(),
                                handle: monitored_handle.clone(),
                                core_request: None,
                                cancellation: cancellation.clone(),
                            }) {
                                log::debug!(
                                    "native repository monitor coalesced refresh workspace={} error={error}",
                                    monitored_workspace
                                );
                            }
                        });
                        self.ivars()
                            .repository_monitor
                            .replace(Some(handle.add_on_change_listener(listener.clone())));
                        self.ivars()
                            .repository_background_pull
                            .replace(Some(handle.schedule_background_pull_loop(Some(listener))));
                        log::info!(
                            "native repository monitor and background pull subscribed workspace={workspace_id}"
                        );
                    } else {
                        log::debug!(
                            "native repository monitor skipped during workspace cancellation workspace={workspace_id}"
                        );
                    }
                }
                self.update_changed_files_snapshot(&snapshot);
                self.ivars()
                    .repository_snapshot
                    .replace(Some(snapshot.clone()));
                self.restore_changes_page_badge();
                self.configure_repository_controls(
                    &snapshot,
                    self.ivars().workspace_refresh_loading.get(),
                );
                self.refresh_changed_file_results();
                self.update_commit_composer_state();
                if let Some(path) = self.ivars().selected_change_path.borrow().clone() {
                    self.request_file_comparison(path);
                }
                if self.is_active_page("changes")
                    && let Some(content_empty) = self.ivars().content_empty.get()
                {
                    content_empty.setStringValue(&NSString::from_str(
                        if snapshot.changed_files.is_empty() {
                            "No changes"
                        } else {
                            "Select a changed file to review its diff"
                        },
                    ));
                }
                log::info!(
                    "native repository snapshot applied workspace={} branch={} changes={}",
                    workspace_id,
                    snapshot.branch,
                    snapshot.changed_files.len()
                );
                if self.is_active_page("history")
                    && (history_changed
                        || self
                            .ivars()
                            .history
                            .get()
                            .is_some_and(|history| history.commits.borrow().is_empty()))
                {
                    self.request_history_page(true);
                }
                if self.is_active_page("files") {
                    self.request_files_tree();
                }
                if self.is_active_page("containers") {
                    self.request_containers();
                }
            }
            Ok(WorkspaceSnapshot::NonRepository { name }) => {
                self.ivars().repository_background_pull.borrow_mut().take();
                self.ivars().repository_monitor.borrow_mut().take();
                self.ivars().git_handle.replace(None);
                self.ivars().repository_snapshot.replace(None);
                self.set_page_badge("changes", NativePageBadge::None);
                self.set_page_badge("history", NativePageBadge::None);
                self.clear_changed_files("No repository content");
                self.reset_history("No repository content");
                self.set_repository_controls_unavailable("Not a Git repository.");
                self.update_commit_composer_state();
                if self.is_active_page("files") {
                    self.request_files_tree();
                }
                if self.is_active_page("containers") {
                    self.request_containers();
                }
                if self.is_active_page("changes")
                    && let Some(content_empty) = self.ivars().content_empty.get()
                {
                    content_empty.setStringValue(&NSString::from_str("No repository content"));
                    self.update_repository_initialization_home(&name);
                }
                log::info!("native non-repository workspace applied workspace={name}");
            }
            Err(error) => {
                self.ivars().repository_background_pull.borrow_mut().take();
                self.ivars().repository_monitor.borrow_mut().take();
                self.ivars().files_monitor.borrow_mut().take();
                self.ivars().workspace_handle.replace(None);
                self.ivars().git_handle.replace(None);
                self.ivars().repository_snapshot.replace(None);
                self.clear_native_quick_actions("Quick Actions are unavailable.");
                for page_id in ["changes", "history", "files", "containers"] {
                    self.set_page_badge(page_id, NativePageBadge::None);
                }
                self.clear_changed_files("Unable to load workspace");
                self.reset_history("Unable to load workspace");
                self.set_repository_controls_unavailable(&error);
                self.update_commit_composer_state();
                if let Some(files) = self.ivars().files.get() {
                    files.loading.set(false);
                    unsafe { files.spinner.stopAnimation(None) };
                    files.spinner.setHidden(true);
                    files.status.setHidden(false);
                    files
                        .status
                        .setStringValue(&NSString::from_str("Unable to load workspace"));
                }
                if let Some(containers) = self.ivars().containers.get() {
                    containers.loading.set(false);
                    containers.scroll.setHidden(true);
                    unsafe { containers.spinner.stopAnimation(None) };
                    containers.spinner.setHidden(true);
                    containers.status.setHidden(false);
                    containers
                        .status
                        .setStringValue(&NSString::from_str("Unable to load workspace"));
                }
                if self.is_active_page("changes")
                    && let Some(content_empty) = self.ivars().content_empty.get()
                {
                    content_empty.setStringValue(&NSString::from_str("Unable to load workspace"));
                }
                log::warn!("native repository snapshot failed workspace={workspace_id}: {error}");
            }
        }
        if self.ivars().workspace_refresh_loading.get() && !completes_workspace_refresh {
            self.set_repository_action_progress(workspace_id, "Refreshing…");
        }
        if first_workspace_snapshot
            && self
                .ivars()
                .commit_message_settings
                .get()
                .is_some_and(|settings| settings.window.isVisible())
        {
            self.request_workspace_settings_load();
        }
        match core_request {
            Some(RepositoryCoreRefreshRequest::Page(page_request_id)) => self
                .complete_pending_page_service_id("changes", page_request_id, page_service_result),
            Some(RepositoryCoreRefreshRequest::Workspace(request)) => {
                self.complete_workspace_refresh(request.identity, page_service_result.map(|_| ()))
            }
            None => {}
        }
    }

    fn update_commit_composer_state(&self) {
        let Some(composer) = self.ivars().commit_composer.get() else {
            return;
        };
        let snapshot = self.ivars().repository_snapshot.borrow();
        let mut selected_files = self
            .ivars()
            .checked_change_paths
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        selected_files.sort();
        let has_selected_files = !selected_files.is_empty();
        let repository_available = self.ivars().git_handle.borrow().is_some();
        composer.set_branch(snapshot.as_ref().map(|snapshot| snapshot.branch.as_str()));
        let file_signature = snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .changed_files
                    .iter()
                    .map(|file| (file.path.clone(), file.status.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let default_summary = default_commit_summary(&selected_files, &file_signature);
        let avatar_source = snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .github_avatar_url
                .as_ref()
                .map(|url| (format!("url:{url}"), AvatarSource::Url(url.to_string())))
                .or_else(|| {
                    snapshot.user_email.as_ref().and_then(|email| {
                        let email = email.trim();
                        (!email.is_empty()).then(|| {
                            (
                                format!("email:{}", email.to_ascii_lowercase()),
                                AvatarSource::Email(email.to_string()),
                            )
                        })
                    })
                })
        });
        let avatar_key = avatar_source.as_ref().map(|(key, _)| key.clone());
        let avatar_image = avatar_key
            .as_deref()
            .and_then(|key| self.ivars().avatar_images.borrow().get(key).cloned());
        self.ivars()
            .commit_avatar_source
            .replace(avatar_key.clone());
        let author_warning = snapshot
            .as_ref()
            .and_then(RepositorySnapshot::remote_author_warning_text);
        composer.set_author(
            snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.user_name.as_deref()),
            avatar_image.as_deref(),
            author_warning.as_deref(),
        );
        drop(snapshot);
        if avatar_image.is_none()
            && let Some((cache_key, source)) = avatar_source
        {
            self.request_avatar(cache_key, source);
        }
        composer.set_repository_available(repository_available);
        composer.set_has_selected_files(has_selected_files);
        composer.set_default_summary(default_summary);
        composer.set_can_generate(true);
    }

    fn cancel_commit_message_generation(&self) {
        if let Some(cancellation) = self.ivars().commit_message_cancellation.borrow_mut().take() {
            cancellation.cancel();
        }
        self.ivars().commit_message_generation_id.set(
            self.ivars()
                .commit_message_generation_id
                .get()
                .wrapping_add(1),
        );
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_generating(false);
        }
        log::info!("native commit message generation cancel requested");
    }

    fn apply_generated_commit_message(
        &self,
        workspace_id: &str,
        request_id: u64,
        provider_label: &str,
        result: Result<CommitMessageDraft, String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id)
            || self.ivars().commit_message_generation_id.get() != request_id
        {
            log::debug!(
                "discarding stale commit message generation workspace={workspace_id} request_id={request_id}"
            );
            return;
        }
        self.ivars().commit_message_cancellation.borrow_mut().take();
        let Some(composer) = self.ivars().commit_composer.get() else {
            return;
        };
        composer.set_generating(false);
        match result {
            Ok(draft) => {
                composer.set_message(&draft.summary, &draft.description);
                log::info!(
                    "native commit message generated provider={} request_id={request_id}",
                    provider_label
                );
            }
            Err(error) if is_canceled_error(&error) => {
                log::info!("native commit message generation canceled request_id={request_id}");
            }
            Err(error) => {
                self.present_path_action_error("Generate Commit Message Failed", &error);
                log::warn!(
                    "native commit message generation failed request_id={request_id}: {error}"
                );
            }
        }
    }

    fn finish_commit(&self, workspace_id: &str, result: &Result<String, String>) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_committing(false);
            match result {
                Ok(message) => {
                    composer.set_message("", "");
                    composer.show_completion(message);
                }
                Err(_) => self.update_commit_composer_state(),
            }
        }
        if let Err(error) = result {
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(&NSString::from_str("Commit Failed"));
            alert.setInformativeText(&NSString::from_str(error));
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            if let Some(window) = self.ivars().window.get() {
                alert.beginSheetModalForWindow_completionHandler(window, None);
            }
        }
    }

    fn update_changed_files_snapshot(&self, snapshot: &RepositorySnapshot) {
        let previous_paths = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .map(|previous| {
                previous
                    .changed_files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let next_paths = snapshot
            .changed_files
            .iter()
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();
        let mut checked = self.ivars().checked_change_paths.borrow_mut();
        checked.retain(|path| next_paths.contains(path.as_str()));
        for path in &next_paths {
            if !previous_paths.contains(path) {
                checked.insert(path.clone());
            }
        }
        drop(checked);

        let selection_is_valid = self
            .ivars()
            .selected_change_path
            .borrow()
            .as_ref()
            .is_some_and(|path| next_paths.contains(path.as_str()));
        if !selection_is_valid {
            self.ivars().selected_change_path.borrow_mut().take();
            self.ivars().loaded_diff_path.borrow_mut().take();
            self.ivars().loaded_image_path.borrow_mut().take();
            self.ivars().diff_loading_request_id.set(None);
            self.ivars()
                .diff_request_id
                .set(self.ivars().diff_request_id.get().wrapping_add(1));
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.clear();
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setImage(None);
                image.setHidden(true);
            }
            self.clear_changed_binary_preview();
            if let Some(spinner) = self.ivars().diff_spinner.get() {
                // SAFETY: The retained progress indicator is animated on the AppKit main thread.
                unsafe { spinner.stopAnimation(None) };
                spinner.setHidden(true);
            }
            if self.is_active_page("changes")
                && let Some(empty) = self.ivars().content_empty.get()
            {
                empty.setStringValue(&NSString::from_str(if snapshot.changed_files.is_empty() {
                    "No local changes"
                } else {
                    "Select a changed file to review its diff"
                }));
                empty.setHidden(false);
            }
        }
    }

    fn clear_changed_files(&self, message: &str) {
        self.hide_repository_home();
        self.ivars().selected_change_path.borrow_mut().take();
        self.ivars().checked_change_paths.borrow_mut().clear();
        self.ivars().loaded_diff_path.borrow_mut().take();
        self.ivars().loaded_image_path.borrow_mut().take();
        self.ivars().file_preview_cache.borrow_mut().clear();
        self.ivars().diff_loading_request_id.set(None);
        self.ivars()
            .diff_request_id
            .set(self.ivars().diff_request_id.get().wrapping_add(1));
        if let Some(list) = self.ivars().changes_list.get() {
            let subviews = list.subviews();
            for index in 0..subviews.count() {
                subviews.objectAtIndex(index).removeFromSuperview();
            }
        }
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.clear();
            diff_view.setHidden(true);
        }
        if let Some(image) = self.ivars().image_preview.get() {
            image.setImage(None);
            image.setHidden(true);
        }
        self.clear_changed_binary_preview();
        if let Some(spinner) = self.ivars().diff_spinner.get() {
            // SAFETY: The retained progress indicator is animated on the AppKit main thread.
            unsafe { spinner.stopAnimation(None) };
            spinner.setHidden(true);
        }
        if self.is_active_page("changes")
            && let Some(empty) = self.ivars().content_empty.get()
        {
            empty.setStringValue(&NSString::from_str(message));
            empty.setHidden(false);
        }
        self.refresh_selection_header();
    }

    fn configure_repository_controls(&self, snapshot: &RepositorySnapshot, running: bool) {
        if let Some(branch) = self.ivars().branch_button.get() {
            branch.setTitle(&NSString::from_str(if snapshot.branch.is_empty() {
                "Branch"
            } else {
                &snapshot.branch
            }));
            branch.setEnabled(!running && !snapshot.branch.is_empty());
        }
        if self
            .ivars()
            .branch_popover
            .get()
            .is_some_and(|popover| popover.isShown())
        {
            let filter = self
                .ivars()
                .branch_search
                .get()
                .map(|search| search.stringValue().to_string())
                .unwrap_or_default();
            self.update_branch_footer();
            self.refresh_branch_results(&filter);
        }
        let Some(fetch) = self.ivars().fetch_button.get() else {
            return;
        };
        let remote = snapshot.remote_name.as_deref().unwrap_or("remote");
        let (label, symbol) = if !snapshot.has_upstream {
            if snapshot.remote_name.is_some() {
                ("Publish branch".to_string(), "paperplane")
            } else {
                ("Publish repository".to_string(), "paperplane")
            }
        } else if snapshot.behind > 0 && snapshot.ahead > 0 {
            (format!("Pull & Push {remote}"), "arrow.up.arrow.down")
        } else if snapshot.behind > 0 {
            (format!("Pull {remote}"), "arrow.down")
        } else if snapshot.ahead > 0 {
            (format!("Push {remote}"), "arrow.up")
        } else {
            (format!("Fetch {remote}"), "arrow.triangle.2.circlepath")
        };
        fetch.setTitle(&NSString::from_str(&format!("\u{2002}{label}")));
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(&label)),
        ) {
            fetch.setImage(Some(&image));
        }
        self.set_fetch_spinner_running(running);
        fetch.setToolTip(Some(&NSString::from_str(&label)));
        fetch.setEnabled(
            !running && (snapshot.remote_name.is_some() || !snapshot.branch.is_empty()),
        );
        if self.is_active_page("changes") && self.ivars().selected_change_path.borrow().is_none() {
            self.update_repository_home(snapshot, running);
        }
    }

    fn set_repository_action_progress(&self, workspace_id: &str, progress: &str) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        if let Some(fetch) = self.ivars().fetch_button.get() {
            let progress = progress.trim();
            if !progress.is_empty() {
                fetch.setTitle(&NSString::from_str(&format!("\u{2003}\u{2003}{progress}")));
                fetch.setToolTip(Some(&NSString::from_str(progress)));
            }
            self.set_fetch_spinner_running(true);
            fetch.setEnabled(false);
        }
        if let Some(action) = self.ivars().content_home_action.get()
            && !action.isHidden()
        {
            action.setTitle(&NSString::from_str("Working…"));
            action.setEnabled(false);
        }
    }

    fn repository_action_failed(&self, workspace_id: &str, title: &str, message: &str) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        if let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() {
            self.configure_repository_controls(&snapshot, false);
        }
        if let Some(fetch) = self.ivars().fetch_button.get() {
            fetch.setToolTip(Some(&NSString::from_str(message)));
        }
        self.present_path_action_error(title, message);
        log::warn!("native git action failed workspace={workspace_id}: {message}");
    }

    fn show_local_changes_overwritten_dialog(
        &self,
        workspace_id: String,
        handle: Arc<GitRepoHandle>,
        snapshot: RepositorySnapshot,
        action: NativeRemoteAction,
        files: Vec<String>,
    ) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(&workspace_id) {
            return;
        }
        self.configure_repository_controls(&snapshot, false);
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Local Changes Would Be Overwritten"));
        alert.setInformativeText(&NSString::from_str(&local_changes_overwritten_body(
            action, &snapshot, &files,
        )));
        alert.addButtonWithTitle(&NSString::from_str("Stash Changes and Continue"));
        alert.addButtonWithTitle(&NSString::from_str("Close"));
        alert.setAlertStyle(NSAlertStyle::Warning);
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response != NSAlertFirstButtonReturn
                || delegate.ivars().active_workspace_id.borrow().as_deref()
                    != Some(workspace_id.as_str())
            {
                return;
            }
            let Some(requests) = delegate.ivars().repository_requests.get() else {
                delegate.repository_action_failed(
                    &workspace_id,
                    "Stash Failed",
                    "The repository service is unavailable.",
                );
                return;
            };
            let Some(cancellation) = delegate.workspace_cancellation_token() else {
                delegate.repository_action_failed(
                    &workspace_id,
                    "Stash Failed",
                    "Workspace cancellation is unavailable.",
                );
                return;
            };
            delegate.set_repository_action_progress(&workspace_id, "Stashing changes…");
            if let Err(error) = requests.try_send(RepositoryRequest::RunGitAction {
                workspace_id: workspace_id.clone(),
                handle: handle.clone(),
                snapshot: snapshot.clone(),
                action,
                stash_before: true,
                cancellation,
            }) {
                delegate.repository_action_failed(
                    &workspace_id,
                    "Stash Failed",
                    &format!("The stash request could not be queued: {error}"),
                );
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn set_branch_action_progress(&self, workspace_id: &str, label: &str) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        if let Some(branch) = self.ivars().branch_button.get() {
            branch.setTitle(&NSString::from_str(label));
            branch.setEnabled(false);
        }
        if let Some(fetch) = self.ivars().fetch_button.get() {
            fetch.setEnabled(false);
        }
    }

    fn branch_action_failed(&self, workspace_id: &str, message: &str) {
        if self.ivars().active_workspace_id.borrow().as_deref() != Some(workspace_id) {
            return;
        }
        if let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() {
            self.configure_repository_controls(&snapshot, false);
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("Branch Operation Failed"));
        alert.setInformativeText(&NSString::from_str(message));
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
        log::warn!("native branch action failed workspace={workspace_id}: {message}");
    }

    fn set_repository_controls_unavailable(&self, message: &str) {
        if let Some(branch) = self.ivars().branch_button.get() {
            branch.setTitle(&NSString::from_str("Branch"));
            branch.setEnabled(false);
        }
        if let Some(fetch) = self.ivars().fetch_button.get() {
            fetch.setTitle(&NSString::from_str("\u{2002}Fetch remote"));
            self.set_fetch_spinner_running(false);
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str("arrow.triangle.2.circlepath"),
                Some(&NSString::from_str("Fetch remote")),
            ) {
                fetch.setImage(Some(&image));
            }
            fetch.setEnabled(false);
            fetch.setToolTip(Some(&NSString::from_str(message)));
        }
    }

    fn set_fetch_spinner_running(&self, running: bool) {
        let Some(spinner) = self.ivars().fetch_spinner.get() else {
            return;
        };
        if running {
            if let Some(fetch) = self.ivars().fetch_button.get() {
                fetch.setImage(None);
            }
            // SAFETY: The spinner is retained by the delegate and animated on AppKit's main thread.
            unsafe { spinner.startAnimation(None) };
        } else {
            // SAFETY: The spinner is retained by the delegate and animated on AppKit's main thread.
            unsafe { spinner.stopAnimation(None) };
        }
    }

}
