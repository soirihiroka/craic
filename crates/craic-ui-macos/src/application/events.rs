impl AppDelegate {
    fn apply_event(&self, event: UiEvent) {
        match event {
            UiEvent::ApplicationState(state) => self.apply_application_state(&state),
            UiEvent::PageState {
                page,
                revision,
                state,
            } => self.apply_page_state(&page, revision, &state),
            UiEvent::PageCommand(command) => self.apply_page_command(command),
            UiEvent::PageServiceRequest(request) => self.apply_page_service_request(request),
            UiEvent::WorkspaceRefreshRequest(request) => self.request_workspace_refresh(request),
            UiEvent::Effect(request) => {
                if self.ivars().ui_context.get() == Some(&request.context) {
                    self.apply_ui_effect(request.id, request.effect);
                } else {
                    submit_ui_effect_completion(
                        self.ivars().app_handle.get(),
                        request.id,
                        UiEffectResult::Failed(
                            "The target native UI context is no longer active".to_string(),
                        ),
                    );
                }
            }
            UiEvent::ShutdownReady => {
                log::info!("native UI received application shutdown readiness");
            }
        }
    }

    fn apply_page_command(&self, command: PageCommand) {
        if command.page.as_ref().map(PageId::as_str) != Some("files")
            || command.action.as_str() != "open-file-location"
        {
            log::warn!(
                "native ignored unsupported app-core page command page={} action={}",
                command.page.as_ref().map(PageId::as_str).unwrap_or("none"),
                command.action.as_str()
            );
            return;
        }
        let Some(payload) = command.payload.as_object() else {
            log::warn!("native ignored malformed open-file-location payload: expected object");
            return;
        };
        let Some(path) = payload.get("path").and_then(serde_json::Value::as_str) else {
            log::warn!("native ignored malformed open-file-location payload: invalid path");
            return;
        };
        let line = match craic_app_core::optional_usize(payload.get("line")) {
            Ok(line) => line,
            Err(()) => {
                log::warn!("native ignored malformed open-file-location payload: invalid line");
                return;
            }
        };
        let column = match craic_app_core::optional_usize(payload.get("column")) {
            Ok(column) => column,
            Err(()) => {
                log::warn!("native ignored malformed open-file-location payload: invalid column");
                return;
            }
        };
        self.apply_workspace_file_location(path.to_string(), line, column);
    }

    fn apply_page_state(&self, page: &PageId, revision: u64, state: &PageViewState) {
        let mut revisions = self.ivars().page_state_revisions.borrow_mut();
        let current = revisions.entry(page.as_str().to_string()).or_default();
        if revision <= *current {
            log::debug!(
                "ignored stale native page state page={} revision={} current_revision={}",
                page.as_str(),
                revision,
                *current
            );
            return;
        }
        *current = revision;
        drop(revisions);

        let badge = if state.refreshing {
            NativePageBadge::Indicator
        } else if let Some(badge) = state.badge.as_ref() {
            badge
                .text
                .parse::<usize>()
                .ok()
                .map(NativePageBadge::Count)
                .unwrap_or(NativePageBadge::Indicator)
        } else {
            NativePageBadge::None
        };
        self.render_page_badge(page.as_str(), badge);
        log::debug!(
            "native page state applied page={} revision={} refreshing={}",
            page.as_str(),
            revision,
            state.refreshing
        );
    }

    fn apply_page_service_request(&self, request: PageServiceRequest) {
        let page = request
            .command
            .page
            .as_ref()
            .map(PageId::as_str)
            .unwrap_or_default()
            .to_string();
        if request.command.action.as_str() != "refresh" {
            self.complete_page_service_request(
                &request,
                Err(format!(
                    "Unsupported native page command: {}",
                    request.command.action.as_str()
                )),
            );
            return;
        }
        let page_request_id = request.request_id.to_string();
        self.ivars()
            .page_service_requests
            .borrow_mut()
            .insert(page.clone(), request);
        match page.as_str() {
            "changes" => self.request_changes_refresh(page_request_id),
            "history" => self.request_history_page(true),
            "files" => self.request_files_tree(),
            "containers" => self.request_containers(),
            "agents" => self.complete_pending_page_service("agents", Ok(serde_json::Value::Null)),
            _ => {
                self.complete_pending_page_service(
                    &page,
                    Err(format!("Unknown native page: {page}")),
                );
            }
        }
    }

    fn complete_pending_page_service(&self, page: &str, result: Result<serde_json::Value, String>) {
        let request = self.ivars().page_service_requests.borrow_mut().remove(page);
        if let Some(request) = request {
            self.complete_page_service_request(&request, result);
        }
    }

    fn complete_pending_page_service_id(
        &self,
        page: &str,
        request_id: String,
        result: Result<serde_json::Value, String>,
    ) {
        let request = self
            .ivars()
            .page_service_requests
            .borrow()
            .get(page)
            .filter(|request| request.request_id.to_string() == request_id.as_str())
            .cloned();
        if let Some(request) = request {
            self.ivars().page_service_requests.borrow_mut().remove(page);
            self.complete_page_service_request(&request, result);
        } else {
            log::debug!(
                "ignored stale native page service completion page={} request={}",
                page,
                request_id
            );
        }
    }

    fn complete_page_service_request(
        &self,
        request: &PageServiceRequest,
        result: Result<serde_json::Value, String>,
    ) {
        let completion = match result {
            Ok(payload) => ServiceCompletion::Succeeded {
                request_id: request.request_id,
                generation: request.page_generation,
                payload,
            },
            Err(message) => ServiceCompletion::Failed {
                request_id: request.request_id,
                generation: request.page_generation,
                message,
            },
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ServiceCompleted(completion)) {
            log::warn!("native page service completion queue rejected command={command:?}");
        }
    }

    fn complete_workspace_refresh(
        &self,
        identity: WorkspaceRefreshIdentity,
        result: Result<(), String>,
    ) {
        let completion = match result {
            Ok(()) => WorkspaceRefreshCompletion::Succeeded(identity),
            Err(message) => WorkspaceRefreshCompletion::Failed { identity, message },
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::WorkspaceRefreshCompleted(completion)) {
            log::warn!("native workspace refresh completion queue rejected command={command:?}");
        }
    }

    fn complete_workspace_refresh_cancelled(&self, identity: WorkspaceRefreshIdentity) {
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::WorkspaceRefreshCompleted(
            WorkspaceRefreshCompletion::Cancelled(identity),
        )) {
            log::warn!("native workspace refresh cancellation queue rejected command={command:?}");
        }
    }

    fn apply_ui_effect(&self, id: UiEffectId, effect: UiEffect) {
        let Some(window) = self.ivars().window.get() else {
            submit_ui_effect_completion(
                self.ivars().app_handle.get(),
                id,
                UiEffectResult::Failed("The application window is unavailable".to_string()),
            );
            return;
        };
        match effect {
            UiEffect::Alert(request) => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(&NSString::from_str(&request.heading));
                alert.setInformativeText(&NSString::from_str(&request.message));
                alert.addButtonWithTitle(&NSString::from_str("OK"));
                let handle = self.ivars().app_handle.get().cloned();
                let completion = RcBlock::new(move |_response| {
                    submit_ui_effect_completion(
                        handle.as_ref(),
                        id.clone(),
                        UiEffectResult::Acknowledged,
                    );
                });
                alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
            }
            UiEffect::Confirm(request) => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(&NSString::from_str(&request.heading));
                alert.setInformativeText(&NSString::from_str(&request.message));
                alert.addButtonWithTitle(&NSString::from_str(&request.confirm_label));
                alert.addButtonWithTitle(&NSString::from_str(&request.cancel_label));
                if request.destructive {
                    alert.setAlertStyle(NSAlertStyle::Warning);
                    if let Some(button) = alert.buttons().firstObject() {
                        button.setHasDestructiveAction(true);
                    }
                }
                let handle = self.ivars().app_handle.get().cloned();
                let completion = RcBlock::new(move |response| {
                    submit_ui_effect_completion(
                        handle.as_ref(),
                        id.clone(),
                        UiEffectResult::Confirmed(response == NSAlertFirstButtonReturn),
                    );
                });
                alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
            }
            UiEffect::Prompt(request) => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(&NSString::from_str(&request.heading));
                alert.setInformativeText(&NSString::from_str(&request.message));
                alert.addButtonWithTitle(&NSString::from_str(&request.confirm_label));
                alert.addButtonWithTitle(&NSString::from_str("Cancel"));
                let input = NSTextField::initWithFrame(
                    NSTextField::alloc(self.mtm()),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(360.0, 24.0)),
                );
                input.setStringValue(&NSString::from_str(&request.initial_value));
                alert.setAccessoryView(Some(&input));
                let handle = self.ivars().app_handle.get().cloned();
                let completion = RcBlock::new(move |response| {
                    let result = if response == NSAlertFirstButtonReturn {
                        UiEffectResult::Prompted(Some(input.stringValue().to_string()))
                    } else {
                        UiEffectResult::Prompted(None)
                    };
                    submit_ui_effect_completion(handle.as_ref(), id.clone(), result);
                });
                alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
            }
            UiEffect::ChoosePath(request) => {
                let initial_url = request.initial_path.as_ref().map(|path| {
                    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
                });
                let allowed = NSArray::from_retained_slice(
                    &request
                        .allowed_extensions
                        .iter()
                        .filter_map(|extension| {
                            UTType::typeWithFilenameExtension(&NSString::from_str(
                                extension.trim_start_matches('.'),
                            ))
                        })
                        .collect::<Vec<_>>(),
                );
                let handle = self.ivars().app_handle.get().cloned();
                match request.mode {
                    PathPickerMode::OpenFile | PathPickerMode::OpenDirectory => {
                        let panel = NSOpenPanel::openPanel(self.mtm());
                        panel.setTitle(Some(&NSString::from_str(&request.title)));
                        panel.setCanChooseFiles(request.mode == PathPickerMode::OpenFile);
                        panel
                            .setCanChooseDirectories(request.mode == PathPickerMode::OpenDirectory);
                        panel.setAllowsMultipleSelection(request.allow_multiple);
                        if !allowed.is_empty() {
                            panel.setAllowedContentTypes(&allowed);
                        }
                        if let Some(url) = initial_url.as_ref() {
                            panel.setDirectoryURL(Some(url));
                        }
                        let retained_panel = panel.clone();
                        let completion = RcBlock::new(move |response| {
                            let result = if response == NSModalResponseOK {
                                UiEffectResult::PathsChosen(
                                    retained_panel
                                        .URLs()
                                        .iter()
                                        .filter_map(|url| url.path())
                                        .map(|path| PathBuf::from(path.to_string()))
                                        .collect(),
                                )
                            } else {
                                UiEffectResult::Cancelled
                            };
                            submit_ui_effect_completion(handle.as_ref(), id.clone(), result);
                        });
                        panel.beginSheetModalForWindow_completionHandler(window, &completion);
                    }
                    PathPickerMode::SaveFile => {
                        let panel = NSSavePanel::savePanel(self.mtm());
                        panel.setTitle(Some(&NSString::from_str(&request.title)));
                        if !allowed.is_empty() {
                            panel.setAllowedContentTypes(&allowed);
                        }
                        if let Some(url) = initial_url.as_ref() {
                            panel.setDirectoryURL(Some(url));
                        }
                        let retained_panel = panel.clone();
                        let completion = RcBlock::new(move |response| {
                            let result = if response == NSModalResponseOK {
                                retained_panel
                                    .URL()
                                    .and_then(|url| url.path())
                                    .map(|path| {
                                        UiEffectResult::PathsChosen(vec![PathBuf::from(
                                            path.to_string(),
                                        )])
                                    })
                                    .unwrap_or_else(|| {
                                        UiEffectResult::Failed(
                                            "The save panel returned no destination".to_string(),
                                        )
                                    })
                            } else {
                                UiEffectResult::Cancelled
                            };
                            submit_ui_effect_completion(handle.as_ref(), id.clone(), result);
                        });
                        panel.beginSheetModalForWindow_completionHandler(window, &completion);
                    }
                }
            }
            UiEffect::OpenPath(request) => {
                let url =
                    NSURL::fileURLWithPath(&NSString::from_str(&request.path.to_string_lossy()));
                let result = if NSWorkspace::sharedWorkspace().openURL(&url) {
                    UiEffectResult::Acknowledged
                } else {
                    UiEffectResult::Failed(format!("Unable to open {}", request.path.display()))
                };
                submit_ui_effect_completion(self.ivars().app_handle.get(), id, result);
            }
            UiEffect::RevealPath(path) => {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                NSWorkspace::sharedWorkspace()
                    .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
                submit_ui_effect_completion(
                    self.ivars().app_handle.get(),
                    id,
                    UiEffectResult::Acknowledged,
                );
            }
            UiEffect::OpenUrl(url) => {
                let result = NSURL::URLWithString(&NSString::from_str(&url))
                    .filter(|url| NSWorkspace::sharedWorkspace().openURL(url))
                    .map_or_else(
                        || UiEffectResult::Failed(format!("Unable to open URL: {url}")),
                        |_| UiEffectResult::Acknowledged,
                    );
                submit_ui_effect_completion(self.ivars().app_handle.get(), id, result);
            }
        }
    }

    fn apply_application_state(&self, state: &ApplicationViewState) {
        self.ivars()
            .workspace_generation
            .set(state.workspace_generation);
        let refresh_loading = state
            .refreshing
            .iter()
            .any(|scope| matches!(scope, RefreshScope::Workspace));
        if self
            .ivars()
            .workspace_refresh_loading
            .replace(refresh_loading)
            != refresh_loading
        {
            if refresh_loading {
                if let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() {
                    self.set_repository_action_progress(&workspace_id, "Refreshing…");
                }
            } else if let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() {
                self.configure_repository_controls(&snapshot, false);
            }
        }
        if let Some(selection) = state.workspace.as_ref()
            && let Some(workspace) = self
                .ivars()
                .workspaces
                .borrow()
                .iter()
                .find(|candidate| candidate.selection_id() == selection.id.as_str())
        {
            self.apply_workspace_button_appearance(workspace);
        }
        let Some(active_page) = state.active_page.as_ref() else {
            return;
        };
        let Some(descriptor) = page_descriptor(active_page) else {
            log::warn!("unknown active page id={}", active_page.as_str());
            return;
        };
        let provider_changed =
            self.ivars().active_page_id.borrow().as_deref() != Some(descriptor.id);
        if provider_changed {
            // A field editor can otherwise remain first responder after its owning page and
            // accessory collapse, which leaks keyboard input into an invisible provider.
            if let Some(window) = self.ivars().window.get() {
                window.makeFirstResponder(None);
            }
            log::debug!(
                "native page provider changed page={} changes_search={} history_search={} files_search={} containers_search={} agents_search={}",
                descriptor.id,
                self.ivars().changes_search_visible.get(),
                self.ivars().history_search_visible.get(),
                self.ivars().files_search_visible.get(),
                self.ivars().containers_search_visible.get(),
                self.ivars().agents_search_visible.get()
            );
        }
        self.ivars()
            .active_page_id
            .replace(Some(descriptor.id.to_string()));
        let detail = match descriptor.id {
            "changes" => "Select a changed file to review its diff.",
            "history" => "Commit history will appear here.",
            "files" => "Select a file from the workspace browser.",
            "containers" => "Running and stopped containers will appear here.",
            "agents" => "Start or resume an agent session.",
            _ => return,
        };
        let showing_changes = descriptor.id == "changes";
        let showing_history = descriptor.id == "history";
        let showing_files = descriptor.id == "files";
        let showing_containers = descriptor.id == "containers";
        let showing_agents = descriptor.id == "agents";
        if provider_changed
            && !showing_agents
            && self.ivars().terminal_search_visible.get()
            && self.ivars().terminal_search_placement.get() == Some(NativeTerminalPlacement::Agent)
        {
            self.hide_native_terminal_search();
        }
        if provider_changed && !showing_changes && self.ivars().changes_search_visible.get() {
            self.hide_changes_search();
        }
        if provider_changed && !showing_history && self.ivars().history_search_visible.get() {
            self.hide_history_search();
        }
        if provider_changed && !showing_files && self.ivars().files_search_visible.get() {
            self.hide_files_search();
        }
        if provider_changed
            && !showing_files
            && self
                .ivars()
                .files
                .get()
                .is_some_and(|files| files.editor_search_visible.get())
        {
            self.hide_editor_search();
        }
        if provider_changed && !showing_containers && self.ivars().containers_search_visible.get() {
            self.hide_containers_search();
        }
        if provider_changed && !showing_agents && self.ivars().agents_search_visible.get() {
            self.hide_agents_search();
        }
        if let Some(changes_split) = self.ivars().changes_split.get() {
            changes_split.setHidden(!showing_changes);
        }
        if let Some(top_cover) = self.ivars().changes_top_cover.get() {
            top_cover.setHidden(!showing_changes);
        }
        if let Some(search_panel) = self.ivars().changes_search_panel.get() {
            search_panel.setHidden(!showing_changes || !self.ivars().changes_search_visible.get());
        }
        if let Some(history) = self.ivars().history.get() {
            history.sidebar_root.setHidden(!showing_history);
            history.content_root.setHidden(!showing_history);
            if !showing_history {
                history.diff.setHidden(true);
                history.binary_preview.setHidden(true);
            }
        }
        if let Some(files) = self.ivars().files.get() {
            files.sidebar_root.setHidden(!showing_files);
            files.content_root.setHidden(!showing_files);
        }
        if let Some(containers) = self.ivars().containers.get() {
            containers.sidebar_root.setHidden(!showing_containers);
            containers.content_root.setHidden(!showing_containers);
        }
        if let Some(agents) = self.ivars().agents.get() {
            agents.sidebar_root.setHidden(!showing_agents);
            agents.content_root.setHidden(!showing_agents);
        }
        if showing_history {
            self.hide_repository_home();
            if let Some(search_panel) = self.ivars().diff_search_panel.get() {
                search_panel.setHidden(true);
            }
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(true);
            }
            if let Some(diff_spinner) = self.ivars().diff_spinner.get() {
                // SAFETY: The retained progress indicator is animated on AppKit's main thread.
                unsafe { diff_spinner.stopAnimation(None) };
                diff_spinner.setHidden(true);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setHidden(true);
            }
            let should_load = self.ivars().git_handle.borrow().is_some()
                && self.ivars().history.get().is_some_and(|history| {
                    history.commits.borrow().is_empty() && !history.loading.get()
                });
            if should_load {
                self.request_history_page(true);
            } else if let Some(history) = self.ivars().history.get()
                && let Some(selected) = history.selected_file.borrow().as_deref()
            {
                let diff_loaded = history.loaded_diff_path.borrow().as_deref() == Some(selected);
                let binary_loaded =
                    history.loaded_binary_path.borrow().as_deref() == Some(selected);
                history.diff.setHidden(!diff_loaded);
                history.binary_preview.setHidden(!binary_loaded);
                history.empty.setHidden(diff_loaded || binary_loaded);
            }
        } else if showing_files {
            self.hide_repository_home();
            if let Some(search_panel) = self.ivars().diff_search_panel.get() {
                search_panel.setHidden(true);
            }
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(true);
            }
            if let Some(diff_spinner) = self.ivars().diff_spinner.get() {
                unsafe { diff_spinner.stopAnimation(None) };
                diff_spinner.setHidden(true);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setHidden(true);
            }
            let should_load = self.ivars().workspace_handle.borrow().is_some()
                && self.ivars().files.get().is_some_and(|files| {
                    (files.rows.borrow().is_empty() || files.dirty.get()) && !files.loading.get()
                });
            if should_load {
                self.request_files_tree();
            }
        } else if showing_containers {
            self.hide_repository_home();
            if let Some(search_panel) = self.ivars().diff_search_panel.get() {
                search_panel.setHidden(true);
            }
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(true);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setHidden(true);
            }
            let should_load = self.ivars().containers.get().is_some_and(|containers| {
                (containers.rows.borrow().is_empty() || containers.dirty.get())
                    && !containers.loading.get()
            });
            if should_load {
                self.request_containers();
            }
        } else if showing_agents {
            self.hide_repository_home();
            if let Some(search_panel) = self.ivars().diff_search_panel.get() {
                search_panel.setHidden(true);
            }
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(true);
            }
            if let Some(diff_spinner) = self.ivars().diff_spinner.get() {
                unsafe { diff_spinner.stopAnimation(None) };
                diff_spinner.setHidden(true);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setHidden(true);
            }
        } else if !showing_changes {
            self.hide_repository_home();
            if let Some(search_panel) = self.ivars().diff_search_panel.get() {
                search_panel.setHidden(true);
            }
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(true);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(true);
            }
            if let Some(diff_spinner) = self.ivars().diff_spinner.get() {
                // SAFETY: The retained progress indicator is animated on the AppKit main thread.
                unsafe { diff_spinner.stopAnimation(None) };
                diff_spinner.setHidden(true);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setStringValue(&NSString::from_str(detail));
                empty.setHidden(false);
            }
        } else if let Some(selected_path) = self.ivars().selected_change_path.borrow().clone() {
            self.hide_repository_home();
            let diff_is_loaded =
                self.ivars().loaded_diff_path.borrow().as_deref() == Some(selected_path.as_str());
            let image_is_loaded =
                self.ivars().loaded_image_path.borrow().as_deref() == Some(selected_path.as_str());
            let diff_is_loading = self.ivars().diff_loading_request_id.get()
                == Some(self.ivars().diff_request_id.get());
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.setHidden(!diff_is_loaded);
            }
            if let Some(image) = self.ivars().image_preview.get() {
                image.setHidden(true);
            }
            if let Some(binary) = self.ivars().binary_preview.get() {
                binary.setHidden(!image_is_loaded);
            }
            if let Some(spinner) = self.ivars().diff_spinner.get() {
                if diff_is_loading {
                    // SAFETY: The retained progress indicator is animated on AppKit's main thread.
                    unsafe { spinner.startAnimation(None) };
                } else {
                    // SAFETY: The retained progress indicator is animated on AppKit's main thread.
                    unsafe { spinner.stopAnimation(None) };
                }
                spinner.setHidden(!diff_is_loading);
            }
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setHidden(diff_is_loaded || image_is_loaded || diff_is_loading);
            }
        } else if self.ivars().repository_loading.get() {
            self.hide_repository_home();
            if let Some(empty) = self.ivars().content_empty.get() {
                empty.setStringValue(&NSString::from_str("Loading workspace…"));
                empty.setHidden(false);
            }
        } else if let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() {
            self.update_repository_home(&snapshot, false);
        } else if self.ivars().workspace_handle.borrow().is_some() {
            self.update_repository_initialization_home("this workspace");
        } else if let Some(empty) = self.ivars().content_empty.get() {
            self.hide_repository_home();
            let message = if self.ivars().repository_loading.get() {
                "Loading workspace…"
            } else {
                "Select a changed file to review its diff"
            };
            empty.setStringValue(&NSString::from_str(message));
            empty.setHidden(false);
        }
        if let Some(page_switcher) = self.ivars().page_switcher.get()
            && let Some(index) = PAGE_DESCRIPTORS
                .iter()
                .position(|candidate| candidate.id == descriptor.id)
        {
            page_switcher.setSelectedIndex(index as isize);
        }
        if let Some(diff) = self.ivars().diff_view.get() {
            diff.refresh_renderer_visibility();
        }
        if let Some(history) = self.ivars().history.get() {
            history.diff.refresh_renderer_visibility();
        }
        for session in self.ivars().terminal_sessions.borrow().iter() {
            session.view.refresh_renderer_visibility();
        }
        self.layout_sidebar();
    }

    fn apply_workspace_entries(
        &self,
        generation: u64,
        entries: Vec<WorkspaceEntry>,
        preferred: Option<craic_config::ConfiguredWorkspace>,
        select_workspace: bool,
    ) {
        if self.ivars().workspace_discovery_generation.get() != generation {
            log::debug!(
                "stale workspace discovery ignored generation={} current_generation={}",
                generation,
                self.ivars().workspace_discovery_generation.get()
            );
            return;
        }
        self.ivars().workspace_discovery_loading.set(false);
        let active_workspace_id = self.ivars().active_workspace_id.borrow().clone();
        let selected = if select_workspace {
            preferred
                .as_ref()
                .and_then(|preferred| {
                    entries
                        .iter()
                        .find(|entry| entry.selection_id() == preferred.selection_id())
                })
                .or_else(|| entries.first())
                .cloned()
        } else {
            active_workspace_id.as_deref().and_then(|workspace_id| {
                entries
                    .iter()
                    .find(|entry| entry.selection_id() == workspace_id)
                    .cloned()
            })
        };

        self.ivars().workspace_metadata.borrow_mut().clear();
        self.ivars().workspace_metadata_pending.borrow_mut().clear();
        self.ivars().workspace_metadata_generation.set(
            self.ivars()
                .workspace_metadata_generation
                .get()
                .wrapping_add(1),
        );
        self.ivars().workspaces.replace(entries.clone());
        self.queue_workspace_metadata(entries);
        let filter = self
            .ivars()
            .workspace_search
            .get()
            .map(|search| search.stringValue().to_string())
            .unwrap_or_default();
        self.refresh_workspace_results(&filter);
        self.refresh_workspace_loading_indicators();
        if let Some(selected) = selected.as_ref() {
            self.apply_workspace_button_appearance(selected);
        } else if let Some(button) = self.ivars().workspace_button.get() {
            button.setTitle(&NSString::from_str("Workspace"));
            button.setContentTintColor(None);
        }

        if select_workspace
            && let Some(selected) = selected
            && let Some(handle) = self.ivars().app_handle.get()
        {
            let workspace_id = selected.selection_id();
            let selection = WorkspaceSelection {
                id: WorkspaceId::new(&workspace_id),
            };
            if let Err(command) = handle.try_send(AppCommand::SelectWorkspace(selection)) {
                log::warn!("initial workspace selection rejected command={command:?}");
                return;
            }
            self.begin_workspace_transition(&workspace_id);
            self.request_repository_load(selected.workspace);
        }
    }
}
