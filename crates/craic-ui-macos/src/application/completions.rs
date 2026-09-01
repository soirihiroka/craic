impl AppDelegate {
    fn show_open_workspace_panel(&self) {
        let Some(requests) = self.ivars().frontend_requests.get() else {
            log::warn!("workspace picker ignored because the UI effect bridge is unavailable");
            return;
        };
        if let Err(error) = requests.try_send(FrontendRequest::OpenWorkspace) {
            log::warn!("workspace picker request queue rejected request error={error}");
        }
    }

    fn request_discard_confirmation(&self, paths: Vec<String>, heading: String, message: String) {
        let Some(requests) = self.ivars().frontend_requests.get() else {
            self.changes_operation_failed(
                "Discard Failed",
                "The native confirmation service is unavailable",
            );
            return;
        };
        if let Err(error) = requests.try_send(FrontendRequest::ConfirmDiscard {
            paths,
            heading,
            message,
        }) {
            self.changes_operation_failed(
                "Discard Failed",
                &format!("The confirmation request could not be queued: {error}"),
            );
        }
    }

    fn apply_repository_completion(&self, completion: RepositoryCompletion) {
        match completion {
            RepositoryCompletion::Snapshot {
                workspace_id,
                cancellation,
                handle,
                core_request,
                result,
            } => {
                if cancellation.is_cancelled() {
                    log::debug!("discarding stale repository snapshot workspace={workspace_id}");
                    return;
                }
                self.apply_repository_snapshot(&workspace_id, handle, core_request, result);
            }
            RepositoryCompletion::ActionProgress {
                workspace_id,
                cancellation,
                message,
            } => {
                if !cancellation.is_cancelled() {
                    self.set_repository_action_progress(&workspace_id, &message);
                }
            }
            RepositoryCompletion::ActionFailed {
                workspace_id,
                cancellation,
                title,
                message,
            } => {
                if !cancellation.is_cancelled() {
                    self.repository_action_failed(&workspace_id, title, &message);
                }
            }
            RepositoryCompletion::ActionNeedsStash {
                workspace_id,
                cancellation,
                handle,
                snapshot,
                action,
                files,
            } => {
                if !cancellation.is_cancelled() {
                    self.show_local_changes_overwritten_dialog(
                        workspace_id,
                        handle,
                        snapshot,
                        action,
                        files,
                    );
                }
            }
            RepositoryCompletion::ActionFinished {
                workspace_id,
                cancellation,
                handle,
                result,
                message,
            } => {
                if cancellation.is_cancelled() {
                    log::debug!("discarding stale git completion workspace={workspace_id}");
                    return;
                }
                let current_workspace = self.ivars().active_workspace_id.borrow().as_deref()
                    == Some(workspace_id.as_str());
                let succeeded = result.is_ok();
                self.apply_repository_snapshot(&workspace_id, Some(handle), None, result);
                if current_workspace && succeeded {
                    self.show_native_toast(
                        message
                            .as_deref()
                            .map(str::trim)
                            .filter(|message| !message.is_empty())
                            .unwrap_or("Git operation completed."),
                    );
                }
            }
            RepositoryCompletion::QuickActions {
                workspace_id,
                generation,
                result,
            } => self.apply_native_quick_actions(&workspace_id, generation, result),
            RepositoryCompletion::QuickActionConfigurationSaved {
                workspace_id,
                result,
            } => {
                if self.ivars().active_workspace_id.borrow().as_deref() == Some(&workspace_id)
                    && let Err(error) = result
                {
                    self.present_path_action_error(
                        "Unable to Save Quick Actions",
                        &format!("The Quick Actions strip will work for this session, but could not be saved: {error}"),
                    );
                }
            }
            RepositoryCompletion::BranchProgress {
                workspace_id,
                cancellation,
                message,
            } => {
                if !cancellation.is_cancelled() {
                    self.set_branch_action_progress(&workspace_id, &message);
                }
            }
            RepositoryCompletion::BranchFailed {
                workspace_id,
                cancellation,
                message,
            } => {
                if !cancellation.is_cancelled() {
                    self.branch_action_failed(&workspace_id, &message);
                }
            }
            RepositoryCompletion::BranchFinished {
                workspace_id,
                cancellation,
                handle,
                result,
                message,
            } => {
                if cancellation.is_cancelled() {
                    log::debug!("discarding stale branch completion workspace={workspace_id}");
                    return;
                }
                let current_workspace = self.ivars().active_workspace_id.borrow().as_deref()
                    == Some(workspace_id.as_str());
                let succeeded = result.is_ok();
                self.apply_repository_snapshot(&workspace_id, Some(handle), None, result);
                if current_workspace && succeeded {
                    self.show_native_toast(message.trim());
                }
            }
            RepositoryCompletion::FileComparison {
                workspace_id,
                path,
                request_id,
                result,
            } => self.apply_file_comparison(&workspace_id, &path, request_id, result),
            RepositoryCompletion::FileBytesComparison {
                workspace_id,
                path,
                request_id,
                result,
            } => self.apply_file_bytes_comparison(&workspace_id, &path, request_id, result),
            RepositoryCompletion::HistoryPage {
                workspace_id,
                generation,
                result,
            } => self.apply_history_page(&workspace_id, generation, result),
            RepositoryCompletion::HistoryCommit {
                workspace_id,
                hash,
                request_id,
                result,
            } => self.apply_history_commit(&workspace_id, &hash, request_id, result),
            RepositoryCompletion::HistoryActionProgress {
                workspace_id,
                cancellation,
                message,
            } => {
                if !cancellation.is_cancelled()
                    && self.ivars().active_workspace_id.borrow().as_deref()
                        == Some(workspace_id.as_str())
                    && let Some(history) = self.ivars().history.get()
                {
                    history.status.setStringValue(&NSString::from_str(&message));
                    history.status.setHidden(false);
                }
            }
            RepositoryCompletion::HistoryActionFailed {
                workspace_id,
                cancellation,
                title,
                message,
            } => {
                if !cancellation.is_cancelled()
                    && self.ivars().active_workspace_id.borrow().as_deref()
                        == Some(workspace_id.as_str())
                {
                    if let Some(history) = self.ivars().history.get() {
                        history.action_in_progress.set(false);
                        history.status.setHidden(true);
                    }
                    self.present_path_action_error(title, &message);
                }
            }
            RepositoryCompletion::HistoryActionFinished {
                workspace_id,
                cancellation,
                handle,
                result,
                message,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                let current = self.ivars().active_workspace_id.borrow().as_deref()
                    == Some(workspace_id.as_str());
                let succeeded = result.is_ok();
                if current && let Some(history) = self.ivars().history.get() {
                    history.action_in_progress.set(false);
                    history.status.setHidden(true);
                }
                self.apply_repository_snapshot(&workspace_id, Some(handle), None, result);
                if current && succeeded {
                    self.show_native_toast(message.trim());
                    self.request_history_page(true);
                }
            }
            RepositoryCompletion::RepositoryInitializationFailed {
                workspace_id,
                cancellation,
                message,
            } => {
                if !cancellation.is_cancelled()
                    && self.ivars().active_workspace_id.borrow().as_deref()
                        == Some(workspace_id.as_str())
                {
                    self.ivars()
                        .repository_initialization_in_progress
                        .set(false);
                    if let Some(button) = self.ivars().content_home_initialize.get() {
                        button.setTitle(&NSString::from_str("Initialize"));
                        button.setEnabled(true);
                    }
                    self.present_path_action_error("Initialize Repository Failed", &message);
                }
            }
            RepositoryCompletion::RepositoryInitializationFinished {
                workspace_id,
                cancellation,
                handle,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                let current = self.ivars().active_workspace_id.borrow().as_deref()
                    == Some(workspace_id.as_str());
                if current {
                    self.ivars()
                        .repository_initialization_in_progress
                        .set(false);
                }
                let succeeded = result.is_ok();
                self.apply_repository_snapshot(&workspace_id, Some(handle), None, result);
                if current && succeeded {
                    self.show_native_toast("Initialized Git repository.");
                }
            }
            RepositoryCompletion::HistoryComparison {
                workspace_id,
                hash,
                path,
                request_id,
                result,
            } => self.apply_history_comparison(&workspace_id, &hash, &path, request_id, result),
            RepositoryCompletion::HistoryBytesComparison {
                workspace_id,
                hash,
                path,
                request_id,
                result,
            } => {
                self.apply_history_bytes_comparison(&workspace_id, &hash, &path, request_id, result)
            }
            RepositoryCompletion::FilesTree {
                workspace_id,
                generation,
                result,
            } => self.apply_files_tree(&workspace_id, generation, result),
            RepositoryCompletion::WorkspaceFile {
                workspace_id,
                path,
                request_id,
                result,
            } => self.apply_workspace_file(&workspace_id, &path, request_id, result),
            RepositoryCompletion::WorkspaceFolder {
                workspace_id,
                path,
                request_id,
                result,
            } => self.apply_workspace_folder(&workspace_id, &path, request_id, result),
            RepositoryCompletion::WorkspaceSqliteSchema {
                workspace_id,
                path,
                request_id,
                result,
            } => self.apply_workspace_sqlite_schema(&workspace_id, &path, request_id, result),
            RepositoryCompletion::WorkspaceSqlitePage {
                workspace_id,
                path,
                generation,
                result,
            } => self.apply_workspace_sqlite_page(&workspace_id, &path, generation, result),
            RepositoryCompletion::WorkspaceFilesChanged { workspace_id } => {
                self.workspace_files_changed(&workspace_id)
            }
            RepositoryCompletion::FileMutationFinished {
                workspace_id,
                access,
                mutation,
                allow_sudo_retry,
                result,
            } => {
                self.finish_file_mutation(&workspace_id, access, mutation, allow_sudo_retry, result)
            }
            RepositoryCompletion::FileDownloadFinished {
                workspace_id,
                access,
                source,
                destination,
                allow_sudo_retry,
                result,
            } => self.finish_workspace_file_download(
                &workspace_id,
                access,
                source,
                destination,
                allow_sudo_retry,
                result,
            ),
            RepositoryCompletion::FileSudoAuthorized {
                workspace_id,
                access,
                retry,
                result,
            } => self.apply_file_sudo_authorization(&workspace_id, access, retry, result),
            RepositoryCompletion::WorkspaceFileSaved {
                workspace_id,
                access,
                path,
                text,
                expected_signature,
                edit_generation,
                allow_sudo_retry,
                result,
            } => self.finish_workspace_file_save(
                &workspace_id,
                access,
                path,
                text,
                expected_signature,
                edit_generation,
                allow_sudo_retry,
                result,
            ),
            RepositoryCompletion::WorkspaceTextHighlighted {
                workspace_id,
                path,
                edit_generation,
                result,
            } => {
                self.apply_workspace_text_highlights(&workspace_id, &path, edit_generation, result)
            }
            RepositoryCompletion::Containers {
                workspace_id,
                generation,
                result,
            } => self.apply_containers(&workspace_id, generation, result),
            RepositoryCompletion::ContainerDetail {
                workspace_id,
                container_id,
                request_id,
                kind,
                result,
            } => {
                self.apply_container_detail(&workspace_id, &container_id, request_id, kind, result)
            }
            RepositoryCompletion::ContainerActionFinished {
                workspace_id,
                workspace_generation,
                request_id,
                result,
            } => self.finish_container_action(
                &workspace_id,
                workspace_generation,
                request_id,
                result,
            ),
            RepositoryCompletion::Avatar { cache_key, result } => {
                self.apply_avatar(&cache_key, result)
            }
            RepositoryCompletion::CommitAuthors {
                workspace_id,
                result,
            } => self.apply_commit_author_options(&workspace_id, result),
            RepositoryCompletion::AgentFileLinkResolved {
                workspace_id,
                line,
                column,
                result,
            } => self.apply_agent_file_link(&workspace_id, line, column, result),
            RepositoryCompletion::AgentImage {
                workspace_id,
                generation,
                item_id,
                source,
                result,
            } => self.apply_native_agent_transcript_image(
                &workspace_id,
                generation,
                &item_id,
                source,
                result,
            ),
            RepositoryCompletion::CommitAuthorFinished {
                workspace_id,
                cancellation,
                handle,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                self.finish_commit_author(&workspace_id, handle, result);
            }
            RepositoryCompletion::CommitFinished {
                workspace_id,
                cancellation,
                handle,
                snapshot,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                if let Some(snapshot) = snapshot {
                    self.apply_repository_snapshot(&workspace_id, handle, None, snapshot);
                }
                self.finish_commit(&workspace_id, &result);
            }
            RepositoryCompletion::CommitMessageGenerated {
                workspace_id,
                cancellation,
                request_id,
                provider_label,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                self.apply_generated_commit_message(
                    &workspace_id,
                    request_id,
                    &provider_label,
                    result,
                );
            }
            RepositoryCompletion::CommitMessageSettingsLoaded {
                request_id,
                provider_id,
                model,
            } => self.apply_commit_message_settings(request_id, provider_id, model),
            RepositoryCompletion::CommitMessageModelsLoaded {
                request_id,
                provider_id,
                selected_model,
                result,
            } => self.apply_commit_message_models(request_id, provider_id, selected_model, result),
            RepositoryCompletion::CommitMessageSettingsFailed { message } => {
                self.set_commit_message_settings_error(&message)
            }
            RepositoryCompletion::WorkspaceSettingsLoaded {
                workspace_id,
                cancellation,
                request_id,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                self.apply_workspace_settings(&workspace_id, request_id, result);
            }
            RepositoryCompletion::WorkspaceSettingsSaved {
                workspace_id,
                cancellation,
                request_id,
                handle,
                result,
            } => {
                if cancellation.is_cancelled() {
                    return;
                }
                self.finish_workspace_settings_save(&workspace_id, request_id, handle, result);
            }
            RepositoryCompletion::ChangesFailed {
                cancellation,
                title,
                message,
            } => {
                if !cancellation.is_cancelled() {
                    self.changes_operation_failed(title, &message);
                }
            }
        }
    }

    fn apply_frontend_completion(&self, completion: FrontendCompletion) {
        match completion {
            FrontendCompletion::Repository(completion) => {
                self.apply_repository_completion(completion)
            }
            FrontendCompletion::Agent(event) => self.apply_native_agent_event(event),
            FrontendCompletion::WorkspaceEntries {
                generation,
                entries,
                preferred,
                select_workspace,
            } => self.apply_workspace_entries(generation, entries, preferred, select_workspace),
            FrontendCompletion::WorkspaceDiscoveryFailed {
                generation,
                message,
            } => {
                if self.ivars().workspace_discovery_generation.get() != generation {
                    log::debug!(
                        "stale workspace discovery failure ignored generation={} current_generation={}",
                        generation,
                        self.ivars().workspace_discovery_generation.get()
                    );
                    return;
                }
                self.ivars().workspace_discovery_loading.set(false);
                let filter = self
                    .ivars()
                    .workspace_search
                    .get()
                    .map(|search| search.stringValue().to_string())
                    .unwrap_or_default();
                self.refresh_workspace_results(&filter);
                self.refresh_workspace_loading_indicators();
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(&NSString::from_str("Unable to Load Workspaces"));
                alert.setInformativeText(&NSString::from_str(&message));
                alert.addButtonWithTitle(&NSString::from_str("OK"));
                if let Some(window) = self.ivars().window.get() {
                    alert.beginSheetModalForWindow_completionHandler(window, None);
                }
            }
            FrontendCompletion::WorkspaceCreated { request_id, result } => {
                self.finish_workspace_creation(request_id, result)
            }
            FrontendCompletion::WorkspaceMetadata {
                workspace_id,
                generation,
                result,
            } => self.apply_workspace_metadata(&workspace_id, generation, result),
            FrontendCompletion::OpenWorkspace(UiEffectResult::PathsChosen(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    self.activate_local_workspace(path.to_string_lossy().into_owned());
                }
            }
            FrontendCompletion::OpenWorkspace(UiEffectResult::Cancelled) => {}
            FrontendCompletion::OpenWorkspace(UiEffectResult::Failed(message)) => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(&NSString::from_str("Unable to Open Workspace"));
                alert.setInformativeText(&NSString::from_str(&message));
                alert.addButtonWithTitle(&NSString::from_str("OK"));
                if let Some(window) = self.ivars().window.get() {
                    alert.beginSheetModalForWindow_completionHandler(window, None);
                }
            }
            FrontendCompletion::OpenWorkspace(result) => {
                log::warn!("workspace picker returned unexpected result={result:?}");
            }
            FrontendCompletion::ConfirmDiscard {
                paths,
                result: UiEffectResult::Confirmed(true),
            } => {
                let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
                    return;
                };
                let Some(handle) = self.ivars().git_handle.borrow().clone() else {
                    return;
                };
                let Some(requests) = self.ivars().repository_requests.get() else {
                    return;
                };
                let Some(cancellation) = self.workspace_cancellation_token() else {
                    return;
                };
                self.set_changes_operation_progress("Discarding changes…");
                if let Err(error) = requests.try_send(RepositoryRequest::Discard {
                    workspace_id,
                    handle,
                    paths,
                    cancellation,
                }) {
                    self.changes_operation_failed(
                        "Discard Failed",
                        &format!("Discard request could not be queued: {error}"),
                    );
                }
            }
            FrontendCompletion::ConfirmDiscard {
                result: UiEffectResult::Confirmed(false) | UiEffectResult::Cancelled,
                ..
            } => {}
            FrontendCompletion::ConfirmDiscard { result, .. } => {
                log::warn!("discard confirmation returned unexpected result={result:?}");
            }
            FrontendCompletion::TerminalRemoteImages {
                workspace_id,
                session_id,
                result,
            } => self.apply_terminal_remote_images(&workspace_id, session_id, result),
            FrontendCompletion::Shutdown => {}
        }
    }

    fn activate_local_workspace(&self, path: String) {
        self.ivars().workspace_discovery_generation.set(
            self.ivars()
                .workspace_discovery_generation
                .get()
                .wrapping_add(1),
        );
        self.ivars().workspace_discovery_loading.set(false);
        let workspace = craic_config::ConfiguredWorkspace::local(path);
        let entry = WorkspaceEntry {
            label: workspace.label(),
            workspace,
        };
        let selection_id = entry.selection_id();
        let mut workspaces = self.ivars().workspaces.borrow_mut();
        let inserted = if !workspaces
            .iter()
            .any(|candidate| candidate.selection_id() == selection_id)
        {
            workspaces.push(entry.clone());
            workspaces.sort_by_key(|candidate| candidate.label.to_lowercase());
            true
        } else {
            false
        };
        drop(workspaces);
        if inserted {
            self.queue_workspace_metadata(vec![entry.clone()]);
        }
        self.apply_workspace_button_appearance(&entry);
        self.refresh_workspace_results("");

        let Some(handle) = self.ivars().app_handle.get() else {
            log::warn!("workspace activation ignored because application actor is unavailable");
            return;
        };
        let selection = WorkspaceSelection {
            id: WorkspaceId::new(selection_id),
        };
        if let Err(command) = handle.try_send(AppCommand::SelectWorkspace(selection)) {
            log::warn!("workspace activation queue rejected command={command:?}");
            return;
        }
        self.begin_workspace_transition(&entry.selection_id());
        self.request_repository_load(entry.workspace.clone());
        self.queue_save_last_workspace(entry.workspace);
    }

    fn set_workspace_button_loading(&self, loading: bool) {
        let Some(spinner) = self.ivars().workspace_button_spinner.get() else {
            return;
        };
        if loading {
            if let Some(button) = self.ivars().workspace_button.get() {
                button.setImage(None);
                button.setToolTip(Some(&NSString::from_str("Loading workspace metadata…")));
            }
            unsafe { spinner.startAnimation(None) };
        } else {
            unsafe { spinner.stopAnimation(None) };
        }
    }

    fn refresh_workspace_loading_indicators(&self) {
        let active = self
            .ivars()
            .active_workspace_id
            .borrow()
            .as_deref()
            .and_then(|workspace_id| {
                self.ivars()
                    .workspaces
                    .borrow()
                    .iter()
                    .find(|workspace| workspace.selection_id() == workspace_id)
                    .cloned()
            });
        if let Some(workspace) = active.as_ref() {
            self.apply_workspace_button_appearance(workspace);
        } else {
            let loading = self.ivars().workspace_discovery_loading.get();
            if let Some(button) = self.ivars().workspace_button.get() {
                button.setTitle(&NSString::from_str("Workspace"));
                button.setContentTintColor(None);
                button.setToolTip(Some(&NSString::from_str("Choose workspace")));
                if !loading
                    && let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                        &NSString::from_str("folder"),
                        Some(&NSString::from_str("Workspace")),
                    )
                {
                    button.setImage(Some(&image));
                }
            }
            self.set_workspace_button_loading(loading);
        }
        if let Some(table) = self.ivars().workspace_table.get() {
            table.reloadData();
        }
    }

    fn apply_workspace_button_appearance(&self, workspace: &WorkspaceEntry) {
        let Some(button) = self.ivars().workspace_button.get() else {
            return;
        };
        let metadata = self
            .ivars()
            .workspace_metadata
            .borrow()
            .get(&workspace.selection_id())
            .cloned();
        let title = metadata
            .as_ref()
            .and_then(|metadata| metadata.remote_label.as_deref())
            .unwrap_or(&workspace.label);
        button.setTitle(&NSString::from_str(title));
        let symbol = metadata
            .as_ref()
            .map(|metadata| native_workspace_metadata_symbol(metadata.kind))
            .unwrap_or("folder");
        let description = metadata
            .as_ref()
            .map(|metadata| native_workspace_metadata_description(metadata.kind))
            .unwrap_or("Workspace");
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&NSString::from_str(description)),
        ) {
            button.setImage(Some(&image));
        }
        button.setContentTintColor(
            workspace
                .workspace
                .color
                .as_ref()
                .and_then(|color| ns_color_from_hex(&color.background))
                .as_deref(),
        );
        button.setToolTip(Some(&NSString::from_str(&format!(
            "Choose workspace — {description}: {}",
            workspace.workspace.path
        ))));
        let loading = self.ivars().workspace_discovery_loading.get()
            || self
                .ivars()
                .workspace_metadata_pending
                .borrow()
                .contains(&workspace.selection_id());
        self.set_workspace_button_loading(loading);
    }

    fn set_page_badge(&self, page_id: &str, badge: NativePageBadge) {
        let core_badge = match &badge {
            NativePageBadge::None => None,
            NativePageBadge::Count(count) => Some(Badge {
                text: count.to_string(),
                attention: false,
            }),
            NativePageBadge::Indicator => Some(Badge {
                text: String::new(),
                attention: true,
            }),
        };
        if let Some(handle) = self.ivars().app_handle.get()
            && let Err(command) = handle.try_send(AppCommand::SetPageBadge {
                page: PageId::new(page_id),
                badge: core_badge,
            })
        {
            log::warn!("native page badge queue rejected command={command:?}");
        }
    }

    fn render_page_badge(&self, page_id: &str, badge: NativePageBadge) {
        let Some(group) = self.ivars().page_switcher.get() else {
            return;
        };
        let Some(index) = PAGE_DESCRIPTORS
            .iter()
            .position(|descriptor| descriptor.id == page_id)
        else {
            return;
        };
        let item = group.subitems().objectAtIndex(index);
        if AnyClass::get(c"NSItemBadge").is_none() || !item.respondsToSelector(sel!(setBadge:)) {
            return;
        }
        let badge = match badge {
            NativePageBadge::None | NativePageBadge::Count(0) => None,
            NativePageBadge::Count(count) => Some(NSItemBadge::badgeWithCount(
                count.min(isize::MAX as usize) as isize,
            )),
            NativePageBadge::Indicator => Some(NSItemBadge::indicatorBadge()),
        };
        item.setBadge(badge.as_deref());
    }

    fn restore_changes_page_badge(&self) {
        let count = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.changed_files.len())
            .unwrap_or(0);
        self.set_page_badge("changes", NativePageBadge::Count(count));
    }

}
