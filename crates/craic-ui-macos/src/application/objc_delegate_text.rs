impl AppDelegate {
    fn handle_objc_delegate_control_text_did_change(&self, notification: &NSNotification) {
        if let Some(object) = notification.object() {
            let name = self.ivars().workspace_create_name.borrow().clone();
            if let Some(name) = name.as_ref()
                && std::ptr::eq(
                    &*object,
                    (&**name as *const NSTextField).cast::<AnyObject>(),
                )
            {
                self.workspace_create_name_did_change(name);
                return;
            }
            let remote = self.ivars().workspace_create_remote.borrow().clone();
            if let Some(remote) = remote.as_ref()
                && std::ptr::eq(
                    &*object,
                    (&**remote as *const NSTextField).cast::<AnyObject>(),
                )
            {
                self.workspace_create_remote_did_change(remote);
                return;
            }
        }
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.clear_completion();
            composer.refresh_action_state();
        }
    }

    fn handle_objc_delegate_text_did_change(&self, notification: &NSNotification) {
        if let Some(object) = notification.object() {
            for (input, submit) in self
                .ivars()
                .agent_request_multiline_inputs
                .borrow()
                .values()
            {
                if std::ptr::eq(
                    &*object,
                    (&**input as *const NSTextView).cast::<AnyObject>(),
                ) {
                    submit.setEnabled(!input.string().to_string().trim().is_empty());
                    return;
                }
            }
        }
        if let (Some(agents), Some(object)) =
            (self.ivars().agents.get(), notification.object())
            && std::ptr::eq(
                &*object,
                (&*agents.composer as *const NSTextView).cast::<AnyObject>(),
            )
        {
            self.refresh_agent_controls();
            return;
        }
        if let (Some(files), Some(object)) =
            (self.ivars().files.get(), notification.object())
            && std::ptr::eq(
                &*object,
                (&*files.preview_text as *const NSTextView).cast::<AnyObject>(),
            )
        {
            if !files.suppress_text_change.get() {
                files.preview_code.clear_completions();
                self.schedule_workspace_file_save();
            }
            return;
        }
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.clear_completion();
            composer.refresh_action_state();
        }
    }

    unsafe fn handle_objc_delegate_text_view_clicked_on_link_at_index(
        &self,
        text_view: &NSTextView,
        link: &AnyObject,
        _char_index: usize,
    ) -> Bool {
        let Some(agents) = self.ivars().agents.get() else {
            return false.into();
        };
        if !text_view.isDescendantOf(&agents.transcript_table) {
            return false.into();
        }
        let Some(value) = link.downcast_ref::<NSString>() else {
            log::warn!("native Codex transcript link had an unsupported value type");
            return false.into();
        };
        self.activate_native_agent_link(destination_target(&value.to_string()));
        true.into()
    }

    unsafe fn handle_objc_delegate_web_view_decide_policy_for_navigation_action_decision_handler(
        &self,
        _web_view: &WKWebView,
        navigation_action: &WKNavigationAction,
        decision_handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
    ) {
        // SAFETY: WebKit supplied a live navigation action for this synchronous callback.
        let navigation_type = unsafe { navigation_action.navigationType() };
        if navigation_type != WKNavigationType::LinkActivated {
            decision_handler.call((WKNavigationActionPolicy::Allow,));
            return;
        }

        // SAFETY: WebKit owns the request for the duration of this callback; URL returns a
        // retained object before the policy handler completes.
        let url = unsafe { navigation_action.request() }.URL();
        decision_handler.call((WKNavigationActionPolicy::Cancel,));
        let Some(url) = url else {
            return;
        };
        let scheme = url
            .scheme()
            .map(|scheme| scheme.to_string().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(scheme.as_str(), "http" | "https" | "mailto" | "file") {
            log::debug!("native web preview ignored link scheme={scheme}");
            return;
        }
        self.confirm_open_preview_url(url);
    }

    unsafe fn handle_objc_delegate_web_view_did_finish_navigation(
        &self,
        web_view: &WKWebView,
        _navigation: Option<&WKNavigation>,
    ) {
        let Some(files) = self.ivars().files.get() else {
            return;
        };
        if !std::ptr::eq(web_view, &*files.preview_web)
            || files.preview_web_mode.get() != NativeWebPreviewMode::BesideEditor
        {
            return;
        }
        if let Some(source_offset) = self.current_files_editor_source_offset() {
            files.markdown_editor_source_offset.set(Some(source_offset));
            self.scroll_files_markdown_preview_to_source_offset(source_offset);
        }
    }

    fn handle_objc_delegate_validate_menu_item(&self, menu_item: &NSMenuItem) -> objc2::runtime::Bool {
        let Some(action) = menu_item.action() else {
            return true.into();
        };
        if action == sel!(newWorkspaceFile:) || action == sel!(newWorkspaceFolder:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self.workspace_file_creation_parent().is_some())
            .into();
        }
        if action == sel!(uploadWorkspaceFiles:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self.workspace_file_creation_parent().is_some())
            .into();
        }
        if action == sel!(copyWorkspaceFile:) || action == sel!(cutWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self
                    .selected_workspace_file_info()
                    .is_some_and(|(_, info)| {
                        !info.path.is_root()
                            && if action == sel!(cutWorkspaceFile:) {
                                info.capabilities.movable
                            } else {
                                info.capabilities.readable
                            }
                    }))
                .into();
        }
        if action == sel!(pasteWorkspaceFile:) {
            if self
                .ivars()
                .files
                .get()
                .is_none_or(|files| files.mutation_in_progress.get())
            {
                return false.into();
            }
            let Some((access, destination_parent)) = self.workspace_file_creation_parent()
            else {
                return false.into();
            };
            return workspace_file_clipboard_from_pasteboard(
                &NSPasteboard::generalPasteboard(),
                access.as_ref(),
            )
            .is_some_and(|(source, _)| {
                source != destination_parent
                    && !destination_parent.is_descendant_of(&source)
            })
            .into();
        }
        if action == sel!(renameWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self
                    .selected_workspace_file_info()
                    .is_some_and(|(_, info)| !info.path.is_root() && info.capabilities.movable))
            .into();
        }
        if action == sel!(duplicateWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self.selected_workspace_file_info().is_some_and(|(_, info)| {
                    !info.path.is_root()
                        && info.capabilities.readable
                        && info.path.parent().is_some_and(|parent| {
                            self.ivars().files.get().is_some_and(|files| {
                                files.rows.borrow().iter().any(|row| {
                                    row.info.path == parent && row.info.capabilities.creatable
                                })
                            })
                        })
                }))
            .into();
        }
        if action == sel!(moveWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self
                    .selected_workspace_file_info()
                    .is_some_and(|(_, info)| !info.path.is_root() && info.capabilities.movable))
            .into();
        }
        if action == sel!(deleteWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self
                    .selected_workspace_file_info()
                    .is_some_and(|(_, info)| !info.path.is_root() && info.capabilities.deletable))
            .into();
        }
        if action == sel!(downloadWorkspaceFile:) {
            return (self
                .ivars()
                .files
                .get()
                .is_some_and(|files| !files.mutation_in_progress.get())
                && self.selected_workspace_file_info().is_some_and(|(access, info)| {
                    access.supports_download()
                        && !info.path.is_root()
                        && info.capabilities.readable
                }))
            .into();
        }
        if action == sel!(copyTerminalWorkingDirectory:)
            || action == sel!(revealTerminalWorkingDirectory:)
        {
            let sessions = self.ivars().terminal_sessions.borrow();
            let Some(session) = sessions
                .iter()
                .find(|session| session.id == menu_item.tag())
            else {
                return false.into();
            };
            return if action == sel!(revealTerminalWorkingDirectory:) {
                session.local_working_directory.is_some()
            } else {
                !session.working_directory.is_empty()
            }
            .into();
        }
        if action == sel!(moveTerminalSessionLeft:)
            || action == sel!(moveTerminalSessionRight:)
        {
            let sessions = self.ivars().terminal_sessions.borrow();
            let Some(index) = sessions
                .iter()
                .position(|session| session.id == menu_item.tag())
            else {
                return false.into();
            };
            let placement = sessions[index].placement;
            return if action == sel!(moveTerminalSessionLeft:) {
                sessions[..index]
                    .iter()
                    .any(|session| session.placement == placement)
            } else {
                sessions[index + 1..]
                    .iter()
                    .any(|session| session.placement == placement)
            }
            .into();
        }
        if action == sel!(showContainerLogs:)
            || action == sel!(inspectContainer:)
            || action == sel!(attachContainerShell:)
            || action == sel!(startContainer:)
            || action == sel!(stopContainer:)
            || action == sel!(restartContainer:)
            || action == sel!(removeContainer:)
        {
            if self.ivars().containers.get().is_none() {
                return false.into();
            }
            let container = self.selected_container();
            let compose = self.selected_compose_project();
            let compose_prefix = if compose.is_some() { "Compose " } else { "" };
            let (title, enabled, _destructive) = if action == sel!(showContainerLogs:) {
                (
                    if compose.is_some() {
                        "Compose Logs".to_string()
                    } else {
                        "View Logs".to_string()
                    },
                    container.is_some() || compose.is_some(),
                    false,
                )
            } else if action == sel!(inspectContainer:) {
                ("Inspect".to_string(), container.is_some(), false)
            } else if action == sel!(attachContainerShell:) {
                (
                    "Attach Shell".to_string(),
                    container
                        .as_ref()
                        .is_some_and(|item| docker::state_is_running(&item.state)),
                    false,
                )
            } else if action == sel!(startContainer:) {
                (
                    format!("{compose_prefix}Start"),
                    compose.is_some()
                        || container.as_ref().is_some_and(ContainerSummary::can_start),
                    false,
                )
            } else if action == sel!(stopContainer:) {
                (
                    format!("{compose_prefix}Stop"),
                    compose.is_some()
                        || container.as_ref().is_some_and(ContainerSummary::can_stop),
                    false,
                )
            } else if action == sel!(restartContainer:) {
                (
                    format!("{compose_prefix}Restart"),
                    compose.is_some()
                        || container
                            .as_ref()
                            .is_some_and(ContainerSummary::can_restart),
                    false,
                )
            } else {
                (
                    if compose.is_some() {
                        "Compose Down".to_string()
                    } else {
                        "Remove".to_string()
                    },
                    compose.is_some()
                        || container.as_ref().is_some_and(ContainerSummary::can_remove),
                    true,
                )
            };
            menu_item.setTitle(&NSString::from_str(&title));
            let lifecycle_action = action == sel!(startContainer:)
                || action == sel!(stopContainer:)
                || action == sel!(restartContainer:)
                || action == sel!(removeContainer:);
            let action_in_progress = self
                .ivars()
                .containers
                .get()
                .is_some_and(|containers| containers.action_in_progress.get());
            return (enabled && (!lifecycle_action || !action_in_progress)).into();
        }
        if action == sel!(pullRemote:) || action == sel!(pushRemote:) {
            return (self.ivars().git_handle.borrow().is_some()
                && self.ivars().repository_snapshot.borrow().is_some()
                && !self.ivars().repository_loading.get())
            .into();
        }
        true.into()
    }

    fn handle_objc_delegate_number_of_rows_in_table_view(&self, table: &NSTableView) -> isize {
        if self
            .ivars()
            .workspace_table
            .get()
            .is_some_and(|workspace_table| std::ptr::eq(table, &**workspace_table))
        {
            let count = self.ivars().workspace_results.borrow().len();
            return count.max(1) as isize;
        }
        if self
            .ivars()
            .author_table
            .get()
            .is_some_and(|author_table| std::ptr::eq(table, &**author_table))
        {
            let options = self.ivars().author_options.borrow();
            let has_status = self.ivars().author_loading.get()
                || self.ivars().author_error.borrow().is_some()
                || options.is_empty();
            return (options.len() + usize::from(has_status)) as isize;
        }
        if self
            .ivars()
            .agents
            .get()
            .is_some_and(|agents| std::ptr::eq(table, &*agents.transcript_table))
        {
            return self
                .ivars()
                .agents
                .get()
                .map_or(0, |agents| agents.transcript_items.borrow().len() as isize);
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &*files.preview_table))
        {
            return self
                .ivars()
                .files
                .get()
                .map_or(0, |files| files.preview_table_rows.borrow().len() as isize);
        }
        if self
            .ivars()
            .files
            .get()
            .is_some_and(|files| std::ptr::eq(table, &**files.table))
        {
            return self.ivars().files.get().map_or(0, |files| {
                files
                    .rows
                    .borrow()
                    .iter()
                    .filter(|row| {
                        let query = files.query.borrow();
                        query.is_empty()
                            || row.info.display_name.to_lowercase().contains(query.as_str())
                            || row.info.path.display().to_lowercase().contains(query.as_str())
                    })
                    .count() as isize
            });
        }
        if self
            .ivars()
            .containers
            .get()
            .is_some_and(|containers| std::ptr::eq(table, &**containers.table))
        {
            return self.filtered_container_rows().len() as isize;
        }
        let Some(history) = self.ivars().history.get() else {
            return 0;
        };
        if std::ptr::eq(table, &*history.table) {
            let commit_count = history.commits.borrow().len();
            (commit_count + usize::from(history.loading.get() && commit_count > 0)) as isize
        } else if std::ptr::eq(table, &*history.files_table) {
            history.files.borrow().len() as isize
        } else {
            0
        }
    }
}
