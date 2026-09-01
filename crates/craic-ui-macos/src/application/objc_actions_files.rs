impl AppDelegate {
    fn handle_objc_filter_files(&self, sender: &NSSearchField) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        files
            .query
            .replace(sender.stringValue().to_string().trim().to_lowercase());
        files.table.reloadData();
        files.status.setHidden(!self.filtered_file_tree_rows().is_empty());
        if self.filtered_file_tree_rows().is_empty() {
            files
                .status
                .setStringValue(&NSString::from_str("No matching workspace files."));
        }
    }

    fn handle_objc_filter_containers(&self, sender: &NSSearchField) {
        let Some(containers) = self.ivars().containers.get() else {
            return;
        };
        containers
            .query
            .replace(sender.stringValue().to_string().trim().to_lowercase());
        containers.table.reloadData();
        let has_rows = !containers.rows.borrow().is_empty();
        let no_matches = self.filtered_container_rows().is_empty()
            && has_rows;
        containers.scroll.setHidden(!has_rows || no_matches);
        containers.status.setHidden(has_rows && !no_matches);
        if no_matches {
            containers
                .status
                .setStringValue(&NSString::from_str("No matching containers."));
        }
    }

    fn handle_objc_show_container_logs(&self, _sender: &NSButton) {
        let access = match self.active_docker_access() {
            Ok(access) => access,
            Err(error) => {
                self.present_path_action_error("Docker Terminal Failed", &error);
                return;
            }
        };
        let (command, title, message) = if let Some(container) = self.selected_container() {
            let args = ["logs", "--tail", "1000", "-f", container.id.as_str()]
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            (
                access.docker_command(&args, None),
                format!("Logs {}", container.display_name()),
                "Started Docker logs in terminal.",
            )
        } else if let Some(compose) = self.selected_compose_project() {
            let working_directory = compose
                .working_dir
                .as_ref()
                .map(|path| WorkspacePath::from_absolute(path.clone()));
            (
                access.docker_command(
                    &docker::compose_args(&compose, &["logs", "--tail", "1000", "-f"]),
                    working_directory.as_ref(),
                ),
                format!("Compose Logs {}", compose.project),
                "Started Compose logs in terminal.",
            )
        } else {
            return;
        };
        let command = match command {
            Ok(command) => command.activity(ShellCommandActivity::LogStream),
            Err(error) => {
                self.present_path_action_error("Docker Terminal Failed", &error);
                return;
            }
        };
        log::info!("native Docker log terminal start title={title}");
        match self.spawn_native_terminal_command(command, title) {
            Ok(()) => self.show_native_toast(message),
            Err(error) => self.present_path_action_error("Docker Terminal Failed", &error),
        }
    }

    fn handle_objc_inspect_container(&self, _sender: &NSButton) {
        self.request_container_detail(ContainerDetailKind::Inspect);
    }

    fn handle_objc_attach_container_shell(&self, _sender: &NSButton) {
        let Some(container) = self.selected_container() else {
            return;
        };
        if !docker::state_is_running(&container.state) {
            return;
        }
        let access = match self.active_docker_access() {
            Ok(access) => access,
            Err(error) => {
                self.present_path_action_error("Docker Terminal Failed", &error);
                return;
            }
        };
        let command = match access.docker_command(
            &[
                "exec".to_string(),
                "-it".to_string(),
                container.id.clone(),
                "sh".to_string(),
            ],
            None,
        ) {
            Ok(command) => command,
            Err(error) => {
                self.present_path_action_error("Docker Terminal Failed", &error);
                return;
            }
        };
        if let Err(error) =
            self.spawn_native_terminal_command(command, container.display_name().to_string())
        {
            self.present_path_action_error("Docker Terminal Failed", &error);
        }
    }

    fn handle_objc_start_container(&self, _sender: &NSButton) {
        self.request_container_action(docker::ContainerAction::Start);
    }

    fn handle_objc_stop_container(&self, _sender: &NSButton) {
        self.request_container_action(docker::ContainerAction::Stop);
    }

    fn handle_objc_restart_container(&self, _sender: &NSButton) {
        self.request_container_action(docker::ContainerAction::Restart);
    }

    fn handle_objc_remove_container(&self, _sender: &NSButton) {
        let container = self.selected_container();
        let compose = self.selected_compose_project();
        if container.is_none() && compose.is_none() {
            return;
        }
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        let alert = NSAlert::new(self.mtm());
        if let Some(compose) = compose {
            alert.setMessageText(&NSString::from_str(&format!(
                "Take down {}?",
                compose.project
            )));
            alert.setInformativeText(&NSString::from_str(
                "Docker Compose will stop and remove this project's containers and network.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Down"));
        } else if let Some(container) = container {
            alert.setMessageText(&NSString::from_str(&format!(
                "Remove {}?",
                container.display_name()
            )));
            alert.setInformativeText(&NSString::from_str(
                "The stopped container will be permanently removed.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Remove"));
        }
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.setAlertStyle(NSAlertStyle::Warning);
        if let Some(button) = alert.buttons().firstObject() {
            button.setHasDestructiveAction(true);
        }
        let delegate = self.retain();
        let completion = RcBlock::new(move |response| {
            if response == NSAlertFirstButtonReturn {
                delegate.request_container_action(docker::ContainerAction::Remove);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&completion));
    }

    fn handle_objc_new_workspace_file(&self, _sender: &NSMenuItem) {
        self.prompt_new_workspace_entry(false);
    }

    fn handle_objc_new_workspace_folder(&self, _sender: &NSMenuItem) {
        self.prompt_new_workspace_entry(true);
    }

    fn handle_objc_upload_workspace_files(&self, _sender: &NSMenuItem) {
        self.choose_workspace_files_to_upload();
    }

    fn handle_objc_rename_workspace_file(&self, _sender: &NSMenuItem) {
        self.prompt_rename_workspace_file();
    }

    fn handle_objc_duplicate_workspace_file(&self, _sender: &NSMenuItem) {
        self.prompt_duplicate_workspace_file();
    }

    fn handle_objc_move_workspace_file(&self, _sender: &NSMenuItem) {
        self.prompt_move_workspace_file();
    }

    fn handle_objc_delete_workspace_file(&self, _sender: &NSMenuItem) {
        self.confirm_delete_workspace_file();
    }

    fn handle_objc_select_sqlite_table(&self, sender: &NSPopUpButton) {
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let changed = {
            let mut state = files.sqlite_state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            if index >= state.tables.len() || state.selected_table == index {
                false
            } else {
                state.selected_table = index;
                state.columns.clear();
                state.page = 0;
                state.total_rows = 0;
                state.filter.clear();
                state.filter_column = None;
                state.sort = None;
                true
            }
        };
        if changed {
            files.sqlite_filter.setStringValue(&NSString::new());
            files.sqlite_column_selector.removeAllItems();
            files
                .sqlite_column_selector
                .addItemWithTitle(&NSString::from_str("All columns"));
            self.request_workspace_sqlite_page();
        }
    }

    fn handle_objc_select_sqlite_filter_column(&self, sender: &NSPopUpButton) {
        let index = sender.indexOfSelectedItem();
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if let Some(state) = files.sqlite_state.borrow_mut().as_mut() {
            state.filter_column = usize::try_from(index).ok().and_then(|index| {
                index
                    .checked_sub(1)
                    .filter(|column| *column < state.columns.len())
            });
            state.page = 0;
        }
        self.request_workspace_sqlite_page();
    }

    fn handle_objc_filter_sqlite_rows(&self, sender: &NSSearchField) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let query = sender.stringValue().to_string();
        if let Some(state) = files.sqlite_state.borrow_mut().as_mut() {
            state.filter = query;
            state.page = 0;
        }
        let generation = files.sqlite_generation.get().wrapping_add(1);
        files.sqlite_generation.set(generation);
        let delegate = MainThreadBound::new(self.retain(), self.mtm());
        let when = DispatchTime::try_from(Duration::from_millis(180))
            .expect("180 milliseconds fits dispatch time");
        let _ = DispatchQueue::main().after(when, move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let delegate = delegate.get(mtm);
            if delegate
                .ivars()
                .files
                .get()
                .is_some_and(|files| files.sqlite_generation.get() == generation)
            {
                delegate.request_workspace_sqlite_page();
            }
        });
    }

    fn handle_objc_previous_sqlite_page(&self, _sender: &NSButton) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let changed = {
            let mut state = files.sqlite_state.borrow_mut();
            if let Some(state) = state.as_mut()
                && state.page > 0
            {
                state.page -= 1;
                true
            } else {
                false
            }
        };
        if changed {
            self.request_workspace_sqlite_page();
        }
    }

    fn handle_objc_next_sqlite_page(&self, _sender: &NSButton) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let changed = {
            let mut state = files.sqlite_state.borrow_mut();
            if let Some(state) = state.as_mut()
                && (state.page + 1) * sqlite_preview::PAGE_SIZE < state.total_rows
            {
                state.page += 1;
                true
            } else {
                false
            }
        };
        if changed {
            self.request_workspace_sqlite_page();
        }
    }

    fn handle_objc_reload_sqlite_preview(&self, _sender: &NSButton) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        let Some(path) = files.selected_path.borrow().clone() else {
            return;
        };
        let info = files
            .rows
            .borrow()
            .iter()
            .find(|row| row.info.path == path)
            .map(|row| row.info.clone());
        if let Some(info) = info {
            self.request_workspace_sqlite(path, info, None);
        }
    }

    fn handle_objc_toggle_file_directory(&self, sender: &NSButton) {
        if let Ok(row) = usize::try_from(sender.tag()) {
            self.toggle_file_tree_row(row);
        }
    }

    fn handle_objc_activate_workspace_selection(&self, _sender: &AnyObject) {
        self.open_selected_workspace_entry();
    }

    fn handle_objc_open_workspace_file(&self, _sender: &NSMenuItem) {
        self.open_selected_workspace_file_external();
    }

    fn handle_objc_reveal_workspace_file(&self, _sender: &NSMenuItem) {
        let Some((access, path, _kind)) = self.selected_workspace_file() else {
            return;
        };
        let Some(local_path) = access.local_path(&path) else {
            self.present_path_action_error(
                "Unable to Reveal Item",
                "Revealing this provider item in Finder is unavailable.",
            );
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&local_path.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
    }

    fn handle_objc_open_workspace_file_in_terminal(&self, _sender: &NSMenuItem) {
        let Some((access, path, kind)) = self.selected_workspace_file() else {
            return;
        };
        let directory = if kind.is_directory() {
            path
        } else {
            path.parent().unwrap_or_else(|| access.root())
        };
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.present_path_action_error(
                "Unable to Open Terminal",
                "The active workspace is not ready.",
            );
            return;
        };
        let working_directory = access.copy_path(&directory);
        let local_working_directory = access.local_path(&directory);
        let result = handle
            .interactive_shell_command_at(&directory)
            .and_then(|(command, title)| {
                self.spawn_native_terminal_command_with_directory(
                    command,
                    title,
                    working_directory,
                    local_working_directory,
                )
            });
        if let Err(error) = result {
            self.present_path_action_error("Unable to Open Terminal", &error);
        }
    }

    fn handle_objc_run_workspace_file_in_terminal(&self, _sender: &NSMenuItem) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.kind.is_directory() || !info.executable() {
            return;
        }
        let Some(handle) = self.ivars().workspace_handle.borrow().clone() else {
            self.present_path_action_error(
                "Unable to Run File",
                "The active workspace is not ready.",
            );
            return;
        };
        let directory = info.path.parent().unwrap_or_else(|| access.root());
        let program = access.copy_path(&info.path);
        let result = handle
            .terminal_command_at(&directory, &program, &[])
            .and_then(|command| {
                self.spawn_native_terminal_command(
                    command,
                    format!("Run {}", info.display_name),
                )
            });
        match result {
            Ok(()) => self.set_native_terminal_visible(true),
            Err(error) => self.present_path_action_error("Unable to Run File", &error),
        }
    }

    fn handle_objc_add_workspace_file_to_chat(&self, _sender: &NSMenuItem) {
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.kind.is_directory() || !info.capabilities.readable {
            return;
        }
        self.append_native_agent_reference(PathBuf::from(access.copy_path(&info.path)));
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ActivatePage(PageId::new("agents"))) {
            log::warn!("Files add-to-chat page activation rejected command={command:?}");
        } else if let Some(index) = PAGE_DESCRIPTORS
            .iter()
            .position(|descriptor| descriptor.id == "agents")
        {
            NSUserDefaults::standardUserDefaults()
                .setInteger_forKey(index as isize, &NSString::from_str(ACTIVE_PAGE_DEFAULT));
        }
    }

    fn handle_objc_add_workspace_ignore_pattern(&self, sender: &NSMenuItem) {
        let Some(pattern) = sender
            .representedObject()
            .and_then(|object| object.downcast::<NSString>().ok())
            .map(|pattern| pattern.to_string())
        else {
            log::warn!("Files ignore-pattern action missing represented pattern");
            return;
        };
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
        self.show_native_toast("Adding ignore pattern…");
        if let Err(error) = requests.try_send(RepositoryRequest::AddIgnorePattern {
            workspace_id,
            handle,
            pattern,
            cancellation,
        }) {
            self.present_path_action_error(
                "Ignore Failed",
                &format!("Ignore request could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_run_workspace_container_file_action(&self, sender: &NSMenuItem) {
        let Some(action) = sender
            .representedObject()
            .and_then(|object| object.downcast::<NSString>().ok())
            .map(|action| action.to_string())
        else {
            log::warn!("Files container action missing represented action");
            return;
        };
        let Some((access, info)) = self.selected_workspace_file_info() else {
            return;
        };
        if info.kind.is_directory() {
            return;
        }
        let Some(workspace_path) = info.path.to_workspace_path(&access.workspace()) else {
            self.present_path_action_error(
                "Container Action Failed",
                "The selected file is outside the active workspace.",
            );
            return;
        };
        let docker = match self.active_docker_access() {
            Ok(docker) => docker,
            Err(error) => {
                self.present_path_action_error("Container Action Failed", &error);
                return;
            }
        };
        let result = match action.as_str() {
            "build-image" => docker
                .build_image_command(&workspace_path)
                .map(|command| (command, "Docker Build")),
            "compose-logs" => docker
                .compose_file_command(&workspace_path, ComposeFileAction::Logs)
                .map(|command| (command, "Compose Logs")),
            "compose-up" => docker
                .compose_file_command(&workspace_path, ComposeFileAction::Up)
                .map(|command| (command, "Compose Up")),
            "compose-pull" => docker
                .compose_file_command(&workspace_path, ComposeFileAction::Pull)
                .map(|command| (command, "Compose Pull")),
            "compose-restart" => docker
                .compose_file_command(&workspace_path, ComposeFileAction::Restart)
                .map(|command| (command, "Compose Restart")),
            "compose-down" => docker
                .compose_file_command(&workspace_path, ComposeFileAction::Down)
                .map(|command| (command, "Compose Down")),
            _ => {
                log::warn!("Files container action ignored unknown action={action}");
                return;
            }
        };
        let result = result.and_then(|(command, title)| {
            self.spawn_native_terminal_command(command, title.to_string())
        });
        match result {
            Ok(()) => {
                self.set_native_terminal_visible(true);
                log::info!(
                    "native Files container action started action={} path={}",
                    action,
                    info.path.display()
                );
            }
            Err(error) => self.present_path_action_error("Container Action Failed", &error),
        }
    }

    fn handle_objc_copy_workspace_file_relative_path(&self, _sender: &NSMenuItem) {
        let Some((_access, path, _kind)) = self.selected_workspace_file() else {
            return;
        };
        self.copy_text_to_pasteboard(&path.display());
    }

    fn handle_objc_copy_workspace_file_provider_path(&self, _sender: &NSMenuItem) {
        let Some((access, path, _kind)) = self.selected_workspace_file() else {
            return;
        };
        self.copy_text_to_pasteboard(&access.copy_path(&path));
    }

    fn handle_objc_copy_workspace_file(&self, _sender: &NSMenuItem) {
        self.store_workspace_file_clipboard(false);
    }

    fn handle_objc_cut_workspace_file(&self, _sender: &NSMenuItem) {
        self.store_workspace_file_clipboard(true);
    }

    fn handle_objc_paste_workspace_file(&self, _sender: &NSMenuItem) {
        self.paste_workspace_file_from_clipboard();
    }

    fn handle_objc_download_workspace_file(&self, _sender: &NSMenuItem) {
        self.choose_workspace_file_download_destination();
    }

    fn handle_objc_filter_diff(&self, sender: &NSSearchField) {
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.set_search_query(&sender.stringValue().to_string());
            self.update_diff_search_status();
        }
    }

    fn handle_objc_previous_diff_match(&self, _sender: &NSButton) {
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.search_previous();
            self.update_diff_search_status();
        }
    }

    fn handle_objc_next_diff_match(&self, _sender: &NSButton) {
        if let Some(diff_view) = self.ivars().diff_view.get() {
            diff_view.search_next();
            self.update_diff_search_status();
        }
    }

    fn handle_objc_close_diff_search(&self, _sender: &NSButton) {
        if let Some(panel) = self.ivars().diff_search_panel.get() {
            panel.setHidden(true);
        }
        if let (Some(window), Some(diff_view)) =
            (self.ivars().window.get(), self.ivars().diff_view.get())
        {
            window.makeFirstResponder(Some(diff_view));
        }
    }

    fn handle_objc_filter_editor(&self, sender: &NSSearchField) {
        if let Some(files) = self.ivars().files.get() {
            files
                .preview_code
                .set_search_query(&sender.stringValue().to_string());
            self.update_editor_search_status();
        }
    }

    fn handle_objc_previous_editor_match(&self, _sender: &NSButton) {
        if let Some(files) = self.ivars().files.get() {
            files.preview_code.search_previous();
            self.update_editor_search_status();
        }
    }

    fn handle_objc_next_editor_match(&self, _sender: &NSButton) {
        if let Some(files) = self.ivars().files.get() {
            files.preview_code.search_next();
            self.update_editor_search_status();
        }
    }

    fn handle_objc_close_editor_search(&self, _sender: &NSButton) {
        self.hide_editor_search();
    }

    fn handle_objc_find_content(&self, _sender: &NSMenuItem) {
        let visible_terminal_search = self.ivars().terminal_search_visible.get()
            && self
                .ivars()
                .terminal_search_placement
                .get()
                .is_some_and(|placement| self.native_terminal_placement_is_visible(placement));
        if visible_terminal_search
            || self
                .active_native_terminal_view()
                .is_some_and(|terminal| terminal.is_focused())
        {
            self.show_native_terminal_search();
            return;
        }
        if self.is_active_page("files") {
            if let Some(files) = self.ivars().files.get() {
                if files.editor_search_visible.get() || files.preview_code.is_focused() {
                    self.show_editor_search();
                    return;
                }
                if self.ivars().files_search_visible.get() {
                    self.hide_files_search();
                } else {
                    self.show_files_search();
                }
            }
            return;
        }
        if self.is_active_page("containers") {
            if self.ivars().containers_search_visible.get() {
                self.hide_containers_search();
            } else {
                self.show_containers_search();
            }
            return;
        }
        if self.is_active_page("agents") {
            if self.ivars().agents_search_visible.get() {
                self.hide_agents_search();
            } else {
                self.show_agents_search();
            }
            return;
        }
        if self.is_active_page("history") {
            if self.ivars().history_search_visible.get() {
                self.hide_history_search();
            } else {
                self.show_history_search();
            }
            return;
        }
        if !self.is_active_page("changes") {
            return;
        }
        let focus_is_in_content = self
            .ivars()
            .window
            .get()
            .and_then(|window| window.firstResponder())
            .and_then(|responder| responder.downcast::<NSView>().ok())
            .zip(self.ivars().content.get())
            .is_some_and(|(view, content)| view.isDescendantOf(content));
        if focus_is_in_content {
            if let Some(diff_view) = self.ivars().diff_view.get() {
                diff_view.focus_search();
            }
        } else if self.ivars().changes_search_visible.get() {
            self.hide_changes_search();
        } else {
            self.show_changes_search();
        }
    }

    fn handle_objc_filter_changed_files(&self, sender: &NSSearchField) {
        let query = sender.stringValue().to_string().trim().to_lowercase();
        if *self.ivars().changes_filter_query.borrow() == query {
            return;
        }
        self.ivars().changes_filter_query.replace(query.clone());
        if let Some(selected) = self.ivars().selected_change_path.borrow().as_deref()
            && let Some(snapshot) = self.ivars().repository_snapshot.borrow().as_ref()
            && let Some(file) = snapshot
                .changed_files
                .iter()
                .find(|file| file.path == selected)
            && !changed_file_matches_query(&file.path, &file.status, &query)
        {
            self.clear_changed_file_preview("Select a changed file to review its diff");
        }
        self.refresh_changed_file_results();
        log::debug!("native changes search updated query_len={}", query.len());
    }

    fn handle_objc_close_changed_files_search(&self, _sender: &NSButton) {
        self.hide_changes_search();
    }

    fn handle_objc_activate_page_from_menu(&self, sender: &NSMenuItem) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(descriptor) = PAGE_DESCRIPTORS.get(index) else {
            return;
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::ActivatePage(descriptor.page_id())) {
            log::warn!("page menu activation queue rejected command={command:?}");
        } else {
            NSUserDefaults::standardUserDefaults().setInteger_forKey(
                index as isize,
                &NSString::from_str(ACTIVE_PAGE_DEFAULT),
            );
        }
    }

    fn handle_objc_filter_history(&self, sender: &NSSearchField) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        history
            .query
            .replace(sender.stringValue().to_string().trim().to_string());
        if history.loading.get() {
            history.pending_search.set(true);
            return;
        }
        self.request_history_page(true);
    }

    fn handle_objc_history_clip_bounds_changed(&self, _notification: &NSNotification) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if !self.is_active_page("history") || history.loading.get() || !history.has_more.get() {
            return;
        }
        let visible = history.scroll.contentView().bounds();
        let remaining =
            history.table.bounds().size.height - visible.origin.y - visible.size.height;
        if remaining <= 360.0 {
            self.request_history_page(false);
        }
    }
}
