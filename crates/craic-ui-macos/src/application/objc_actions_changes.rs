impl AppDelegate {
    fn handle_objc_add_branch(&self, _sender: &NSButton) {
        if let Some(popover) = self.ivars().branch_popover.get() {
            popover.close();
        }
        self.show_new_branch_sheet();
    }

    fn handle_objc_toggle_merge_branch(&self, _sender: &NSButton) {
        let merge_mode = !self.ivars().branch_merge_mode.get();
        self.ivars().branch_merge_mode.set(merge_mode);
        if let Some(search) = self.ivars().branch_search.get() {
            search.setStringValue(&NSString::new());
            search.setPlaceholderString(Some(&NSString::from_str(if merge_mode {
                "Search branches to merge"
            } else {
                "Search branches"
            })));
        }
        self.update_branch_footer();
        self.refresh_branch_results("");
    }

    fn handle_objc_activate_changed_file(&self, sender: &NSButton) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let Some(path) = snapshot
            .changed_files
            .get(index)
            .map(|file| file.path.clone())
        else {
            log::warn!("changed-file selection index out of range index={index}");
            return;
        };
        self.ivars().selected_change_path.replace(Some(path.clone()));
        if let Some(list) = self.ivars().changes_list.get() {
            let containers = list.subviews();
            for container_index in 0..containers.count() {
                let row_views = containers.objectAtIndex(container_index).subviews();
                if row_views.count() < 4 {
                    continue;
                }
                let Ok(highlight) = row_views.objectAtIndex(0).downcast::<NSBox>() else {
                    continue;
                };
                let Ok(row) = row_views.objectAtIndex(2).downcast::<NSButton>() else {
                    continue;
                };
                let Ok(status) = row_views.objectAtIndex(3).downcast::<NSImageView>() else {
                    continue;
                };
                let is_selected = row.tag() == sender.tag();
                highlight.setHidden(!is_selected);
                row.setState(if is_selected {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                let status_color = if is_selected {
                    NSColor::selectedControlTextColor()
                } else {
                    NSColor::secondaryLabelColor()
                };
                status.setContentTintColor(Some(&status_color));
            }
            log::debug!(
                "updated changed-file selection in place index={index} clip_origin_y={}",
                self.ivars()
                    .changes_scroll
                    .get()
                    .map(|scroll| scroll.contentView().bounds().origin.y)
                    .unwrap_or(0.0)
            );
        }
        self.request_file_comparison(path);
    }

    fn handle_objc_toggle_changed_file(&self, sender: &NSButton) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(path) = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.changed_files.get(index))
            .map(|file| file.path.clone())
        else {
            return;
        };
        if sender.state() == NSControlStateValueOn {
            self.ivars().checked_change_paths.borrow_mut().insert(path);
        } else {
            self.ivars().checked_change_paths.borrow_mut().remove(&path);
        }
        self.update_commit_composer_state();
        self.refresh_selection_header();
    }

    fn handle_objc_toggle_all_changed_files(&self, sender: &NSButton) {
        self.set_all_visible_changed_files_checked(sender.state() == NSControlStateValueOn);
    }

    fn handle_objc_select_all_changed_files_from_menu(&self, _sender: &NSMenuItem) {
        self.set_all_visible_changed_files_checked(true);
    }

    fn handle_objc_deselect_all_changed_files_from_menu(&self, _sender: &NSMenuItem) {
        self.set_all_visible_changed_files_checked(false);
    }

    fn handle_objc_confirm_discard_all_changes(&self, _sender: &NSMenuItem) {
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let paths = snapshot
            .changed_files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return;
        }
        let message = if paths.len() == 1 {
            format!(
                "Are you sure you want to discard all changes to:\n\n{}",
                paths[0]
            )
        } else {
            format!(
                "Are you sure you want to discard all changes to {} files?",
                paths.len()
            )
        };
        self.request_discard_confirmation(
            paths,
            "Confirm Discard Changes".to_string(),
            message,
        );
    }

    fn handle_objc_open_changed_file(&self, sender: &NSMenuItem) {
        let Some(relative_path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        let full_path = match self.local_changed_file_path(&relative_path) {
            Ok(path) => path,
            Err(message) => {
                self.present_path_action_error("Unable to Open File", &message);
                return;
            }
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&full_path.to_string_lossy()));
        if !NSWorkspace::sharedWorkspace().openURL(&url) {
            self.present_path_action_error(
                "Unable to Open File",
                &format!("No application could open {}.", full_path.display()),
            );
        }
    }

    fn handle_objc_open_changed_file_in_code(&self, sender: &NSMenuItem) {
        let Some(relative_path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        let full_path = match self.local_changed_file_path(&relative_path) {
            Ok(path) => path,
            Err(message) => {
                self.present_path_action_error("Unable to Open File", &message);
                return;
            }
        };
        let workspace = NSWorkspace::sharedWorkspace();
        let Some(application_url) = workspace.URLForApplicationWithBundleIdentifier(
            &NSString::from_str("com.microsoft.VSCode"),
        ) else {
            self.present_path_action_error(
                "Unable to Open File",
                "Visual Studio Code is not installed or is not registered with macOS.",
            );
            return;
        };
        let file_url = NSURL::fileURLWithPath(&NSString::from_str(&full_path.to_string_lossy()));
        let configuration = NSWorkspaceOpenConfiguration::new();
        configuration.setActivates(true);
        workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
            &NSArray::from_slice(&[&*file_url]),
            &application_url,
            &configuration,
            None,
        );
    }

    fn handle_objc_reveal_changed_file(&self, sender: &NSMenuItem) {
        let Some(relative_path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        let full_path = match self.local_changed_file_path(&relative_path) {
            Ok(path) => path,
            Err(message) => {
                self.present_path_action_error("Unable to Reveal File", &message);
                return;
            }
        };
        let root = full_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| full_path.clone());
        let selected = NSString::from_str(&full_path.to_string_lossy());
        if !NSWorkspace::sharedWorkspace().selectFile_inFileViewerRootedAtPath(
            Some(&selected),
            &NSString::from_str(&root.to_string_lossy()),
        ) {
            self.present_path_action_error(
                "Unable to Reveal File",
                &format!("Finder could not reveal {}.", full_path.display()),
            );
        }
    }

    fn handle_objc_show_changed_file_in_files(&self, sender: &NSMenuItem) {
        let Some(path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        self.enqueue_workspace_file_location(path, None, None);
    }

    fn handle_objc_add_changed_ignore_pattern(&self, sender: &NSMenuItem) {
        let Some(pattern) = sender
            .representedObject()
            .and_then(|object| object.downcast::<NSString>().ok())
            .map(|pattern| pattern.to_string())
        else {
            log::warn!("ignore-pattern action missing represented pattern");
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
        self.set_changes_operation_progress("Adding ignore pattern…");
        if let Err(error) = requests.try_send(RepositoryRequest::AddIgnorePattern {
            workspace_id,
            handle,
            pattern,
            cancellation,
        }) {
            self.changes_operation_failed(
                "Ignore Failed",
                &format!("Ignore request could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_stash_all_changes(&self, _sender: &NSMenuItem) {
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
        self.set_changes_operation_progress("Stashing changes…");
        if let Err(error) = requests.try_send(RepositoryRequest::Stash {
            workspace_id,
            handle,
            cancellation,
        }) {
            self.changes_operation_failed(
                "Stash Failed",
                &format!("Stash request could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_copy_changed_relative_path(&self, sender: &NSMenuItem) {
        if let Some(relative_path) = self.changed_file_path_for_tag(sender.tag()) {
            self.copy_text_to_pasteboard(&relative_path);
        }
    }

    fn handle_objc_copy_changed_absolute_path(&self, sender: &NSMenuItem) {
        let Some(relative_path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        match self.local_changed_file_path(&relative_path) {
            Ok(path) => self.copy_text_to_pasteboard(&path.to_string_lossy()),
            Err(message) => self.present_path_action_error("Unable to Copy Path", &message),
        }
    }

    fn handle_objc_confirm_discard_changed_file(&self, sender: &NSMenuItem) {
        let Some(path) = self.changed_file_path_for_tag(sender.tag()) else {
            return;
        };
        self.request_discard_confirmation(
            vec![path.clone()],
            "Discard Changes?".to_string(),
            format!("Discard all uncommitted changes to {path}? This cannot be undone."),
        );
    }

    fn handle_objc_select_commit_author(&self, sender: &NSButton) {
        self.show_commit_author_picker(sender);
    }

    fn handle_objc_show_commit_author_warning(&self, sender: &NSButton) {
        let Some(message) = self
            .ivars()
            .commit_composer
            .get()
            .and_then(CommitComposer::author_warning_text)
        else {
            return;
        };
        if let Some(popover) = self.ivars().author_warning_popover.borrow_mut().take() {
            popover.close();
        }
        let size = NSSize::new(340.0, 100.0);
        let content = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            NSRect::new(NSPoint::ZERO, size),
        );
        let heading = NSTextField::labelWithString(
            &NSString::from_str("Git author differs from remote owner"),
            self.mtm(),
        );
        heading.setFrame(NSRect::new(
            NSPoint::new(14.0, 68.0),
            NSSize::new(size.width - 28.0, 18.0),
        ));
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        heading.setTextColor(Some(&NSColor::labelColor()));
        heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        content.addSubview(&heading);
        let detail = NSTextField::wrappingLabelWithString(
            &NSString::from_str(&message),
            self.mtm(),
        );
        detail.setFrame(NSRect::new(
            NSPoint::new(14.0, 12.0),
            NSSize::new(size.width - 28.0, 48.0),
        ));
        detail.setMaximumNumberOfLines(3);
        detail.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        detail.setToolTip(Some(&NSString::from_str(&message)));
        content.addSubview(&detail);
        let controller = NSViewController::new(self.mtm());
        controller.setView(&content);
        controller.setPreferredContentSize(size);
        let popover = NSPopover::new(self.mtm());
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setContentSize(size);
        popover.setContentViewController(Some(&controller));
        popover.showRelativeToRect_ofView_preferredEdge(
            sender.bounds(),
            sender,
            NSRectEdge::MaxX,
        );
        self.ivars().author_warning_popover.replace(Some(popover));
        log::debug!("native remote-owner warning popover opened size=340x100");
    }

    fn handle_objc_select_commit_author_option(&self, sender: &NSButton) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(option) = self.ivars().author_options.borrow().get(index).cloned() else {
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
        if let Err(error) = requests.try_send(RepositoryRequest::SaveCommitAuthor {
            workspace_id,
            handle,
            option,
            cancellation,
        }) {
            self.present_path_action_error(
                "Author Selection Failed",
                &format!("Unable to queue author update: {error}"),
            );
        }
    }

    fn handle_objc_commit_summary_changed(&self, _sender: &NSTextField) {
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.clear_completion();
            composer.refresh_action_state();
        }
    }

    fn handle_objc_generate_commit_message(&self, _sender: &NSButton) {
        let Some(composer) = self.ivars().commit_composer.get() else {
            return;
        };
        if composer.is_generating() {
            self.cancel_commit_message_generation();
            return;
        }
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            self.present_path_action_error(
                "Generate Commit Message Failed",
                "Commit message generation is unavailable for this workspace.",
            );
            return;
        };
        let mut files = self
            .ivars()
            .checked_change_paths
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        files.sort();
        if files.is_empty() {
            self.present_path_action_error(
                "Generate Commit Message Failed",
                "Select at least one file before generating a commit message.",
            );
            return;
        }
        let request_id = self
            .ivars()
            .commit_message_generation_id
            .get()
            .wrapping_add(1);
        self.ivars()
            .commit_message_generation_id
            .set(request_id);
        let cancellation = CancellationToken::new();
        let Some(workspace_cancellation) = self.workspace_cancellation_token() else {
            self.present_path_action_error(
                "Generate Commit Message Failed",
                "The workspace is no longer active.",
            );
            return;
        };
        self.ivars()
            .commit_message_cancellation
            .replace(Some(cancellation.clone()));
        composer.clear_completion();
        composer.set_generating(true);
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.cancel_commit_message_generation();
            self.present_path_action_error(
                "Generate Commit Message Failed",
                "The repository service is unavailable.",
            );
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::GenerateCommitMessage {
            workspace_id,
            handle,
            files,
            request_id,
            cancellation,
            workspace_cancellation,
        }) {
            self.cancel_commit_message_generation();
            self.present_path_action_error(
                "Generate Commit Message Failed",
                &format!("Unable to queue commit-message generation: {error}"),
            );
            return;
        }
        log::info!("native commit message generation requested request_id={request_id}");
    }

    fn handle_objc_mouse_entered(&self, _event: &NSEvent) {
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_generation_hovered(true);
        }
    }

    fn handle_objc_mouse_exited(&self, _event: &NSEvent) {
        if let Some(composer) = self.ivars().commit_composer.get() {
            composer.set_generation_hovered(false);
        }
    }

    fn handle_objc_commit_changes(&self, _sender: &NSButton) {
        let Some(workspace_id) = self.ivars().active_workspace_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().git_handle.borrow().clone() else {
            return;
        };
        let Some(composer) = self.ivars().commit_composer.get() else {
            return;
        };
        let files = self
            .ivars()
            .checked_change_paths
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if files.is_empty() || composer.summary().trim().is_empty() {
            return;
        }
        let Some(requests) = self.ivars().repository_requests.get() else {
            return;
        };
        let Some(cancellation) = self.workspace_cancellation_token() else {
            return;
        };
        composer.set_committing(true);
        let request = RepositoryRequest::Commit {
            workspace_id,
            handle,
            summary: composer.summary(),
            description: composer.description(),
            files,
            cancellation,
        };
        if let Err(error) = requests.try_send(request) {
            composer.set_committing(false);
            self.changes_operation_failed(
                "Commit Failed",
                &format!("Commit request could not be queued: {error}"),
            );
        }
    }

    fn handle_objc_fetch_remote(&self, _sender: &NSButton) {
        self.request_remote_action(NativeRemoteAction::Contextual);
    }

    fn handle_objc_open_repository_suggestion_in_editor(&self, _sender: &NSButton) {
        self.open_active_repository_in_editor();
    }

    fn handle_objc_open_repository_suggestion_in_ghostty(&self, _sender: &NSButton) {
        self.open_active_repository_in_ghostty();
    }

    fn handle_objc_show_repository_suggestion_in_finder(&self, _sender: &NSButton) {
        self.show_active_repository_in_finder();
    }

    fn handle_objc_open_repository_suggestion_remote(&self, _sender: &NSButton) {
        self.open_active_repository_remote();
    }

    fn handle_objc_initialize_repository_suggestion(&self, _sender: &NSButton) {
        self.initialize_active_repository();
    }

    fn handle_objc_toggle_terminal(&self, sender: &NSToolbarItem) {
        let visible = !self.ivars().terminal_visible.get();
        if visible {
            let general_id = self
                .ivars()
                .active_general_terminal_id
                .get()
                .or_else(|| {
                    self.ivars()
                        .terminal_sessions
                        .borrow()
                        .iter()
                        .find(|session| {
                            session.placement == NativeTerminalPlacement::General
                        })
                        .map(|session| session.id)
                });
            if let Some(id) = general_id {
                self.activate_native_terminal_session(id);
            } else if let Err(error) = self.spawn_native_terminal_session() {
                self.present_path_action_error("Unable to Open Terminal", &error);
                return;
            }
        }
        self.set_native_terminal_visible(visible);
        sender.setToolTip(Some(&NSString::from_str(if visible {
            "Hide terminal"
        } else {
            "Show terminal"
        })));
        log::info!("native terminal visibility changed visible={visible}");
    }

    fn handle_objc_new_terminal_session(&self, _sender: &NSButton) {
        if let Err(error) = self.spawn_native_terminal_session() {
            self.present_path_action_error("Unable to Open Terminal", &error);
        }
    }

    fn handle_objc_close_terminal_session(&self, sender: &NSButton) {
        let requested_id = sender.tag();
        let Some(id) = (requested_id > 0)
            .then_some(requested_id)
            .or_else(|| self.ivars().active_terminal_id.get())
        else {
            return;
        };
        self.request_native_terminal_close(id);
    }

    fn handle_objc_select_terminal_session(&self, sender: &NSButton) {
        self.activate_native_terminal_session(sender.tag());
    }
}
