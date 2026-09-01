impl AppDelegate {
    fn handle_objc_copy_history_hash(&self, _sender: &NSButton) {
        let Some(hash) = self
            .ivars()
            .history
            .get()
            .and_then(|history| history.selected_hash.borrow().clone())
        else {
            return;
        };
        self.copy_text_to_pasteboard(&hash);
    }

    fn handle_objc_open_history_remote(&self, _sender: &NSButton) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        let Some(hash) = history.selected_hash.borrow().clone() else {
            return;
        };
        let Some(remote_url) = self
            .ivars()
            .repository_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.remote_url.clone())
        else {
            return;
        };
        let web_url = craic_vcs::git::remote_commit_web_url(&remote_url, &hash);
        let Some(url) = NSURL::URLWithString(&NSString::from_str(&web_url)) else {
            return;
        };
        NSWorkspace::sharedWorkspace().openURL(&url);
    }

    fn handle_objc_checkout_history_commit(&self, _sender: &NSMenuItem) {
        if let Some(hash) = self.selected_history_hash() {
            self.run_history_action(HistoryAction::Checkout {
                hash,
                parent: false,
            });
        }
    }

    fn handle_objc_checkout_history_parent(&self, _sender: &NSMenuItem) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if let Some(hash) = history.selected_parent_hash.borrow().clone() {
            self.run_history_action(HistoryAction::Checkout { hash, parent: true });
        } else if !history.parent_loaded.get() {
            history.pending_checkout_parent.set(true);
            if !history.detail_loading.get() {
                self.retry_selected_history_commit_detail();
                history.pending_checkout_parent.set(true);
            }
            history
                .status
                .setStringValue(&NSString::from_str("Loading parent commit…"));
            history.status.setHidden(false);
        }
    }

    fn handle_objc_new_history_branch(&self, _sender: &NSMenuItem) {
        self.show_history_name_sheet(true);
    }

    fn handle_objc_create_history_tag(&self, _sender: &NSMenuItem) {
        self.show_history_name_sheet(false);
    }

    fn handle_objc_cherry_pick_history_commit(&self, _sender: &NSMenuItem) {
        if let Some(hash) = self.selected_history_hash() {
            self.run_history_action(HistoryAction::CherryPick(hash));
        }
    }

    fn handle_objc_revert_history_commit(&self, _sender: &NSMenuItem) {
        self.confirm_history_action(ResetMode::Mixed, true);
    }

    fn handle_objc_amend_history_head(&self, _sender: &NSMenuItem) {
        let Some(history) = self.ivars().history.get() else {
            return;
        };
        if history.selected_commit.borrow().is_some() {
            self.show_history_amend_sheet();
        } else {
            history.pending_amend.set(true);
            if !history.detail_loading.get() {
                self.retry_selected_history_commit_detail();
                history.pending_amend.set(true);
            }
            history
                .status
                .setStringValue(&NSString::from_str("Loading commit message…"));
            history.status.setHidden(false);
        }
    }

    fn handle_objc_reset_history_mixed(&self, _sender: &NSMenuItem) {
        self.confirm_history_action(ResetMode::Mixed, false);
    }

    fn handle_objc_reset_history_hard(&self, _sender: &NSMenuItem) {
        self.confirm_history_action(ResetMode::Hard, false);
    }

    fn handle_objc_toggle_workspace_picker(&self, sender: &NSButton) {
        self.show_workspace_picker(sender);
    }

    fn handle_objc_filter_workspaces(&self, sender: &NSSearchField) {
        self.refresh_workspace_results(&sender.stringValue().to_string());
    }

    fn handle_objc_activate_workspace_row(&self, sender: &NSTableView) {
        let Ok(visible_index) = usize::try_from(sender.selectedRow()) else {
            return;
        };
        let Some(index) = self
            .ivars()
            .workspace_results
            .borrow()
            .get(visible_index)
            .copied()
        else {
            return;
        };
        self.activate_workspace_at(index);
    }

    fn handle_objc_activate_workspace_option(&self, sender: &NSButton) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        self.activate_workspace_at(index);
    }

    fn handle_objc_add_workspace(&self, _sender: &NSButton) {
        if let Some(popover) = self.ivars().workspace_popover.get() {
            popover.close();
        }
        self.show_create_workspace_dialog();
    }

    fn handle_objc_workspace_create_name_changed(&self, sender: &NSTextField) {
        self.workspace_create_name_did_change(sender);
    }

    fn handle_objc_workspace_create_remote_changed(&self, sender: &NSTextField) {
        self.workspace_create_remote_did_change(sender);
    }

    fn handle_objc_submit_workspace_creation_action(&self, _sender: &NSButton) {
        self.submit_workspace_creation();
    }

    fn handle_objc_open_workspace(&self, _sender: &NSMenuItem) {
        self.show_open_workspace_panel();
    }

    fn handle_objc_refresh_workspace(&self, _sender: &NSMenuItem) {
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) = handle.try_send(AppCommand::Refresh(RefreshScope::Workspace)) {
            log::warn!("native workspace refresh queue rejected command={command:?}");
        }
    }

    fn handle_objc_refresh_page(&self, _sender: &NSMenuItem) {
        let Some(page) = self.ivars().active_page_id.borrow().clone() else {
            return;
        };
        let Some(handle) = self.ivars().app_handle.get() else {
            return;
        };
        if let Err(command) =
            handle.try_send(AppCommand::Refresh(RefreshScope::Page(PageId::new(page))))
        {
            log::warn!("native page refresh queue rejected command={command:?}");
        }
    }

    fn handle_objc_new_window(&self, _sender: &NSMenuItem) {
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setCreatesNewApplicationInstance(true);
        let delegate = Arc::new(MainThreadBound::new(self.retain(), self.mtm()));
        let completion = RcBlock::new(
            move |_application: *mut NSRunningApplication, error: *mut NSError| {
                let Some(error) = (unsafe { error.as_ref() }) else {
                    log::info!("new native window opened");
                    return;
                };
                let message = error.localizedDescription().to_string();
                log::warn!("new native window launch failed error={message}");
                let delegate = delegate.clone();
                DispatchQueue::main().exec_async(move || {
                    let Some(mtm) = MainThreadMarker::new() else {
                        return;
                    };
                    delegate
                        .get(mtm)
                        .present_path_action_error("Failed to Open New Window", &message);
                });
            },
        );
        NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
            &NSBundle::mainBundle().bundleURL(),
            &configuration,
            Some(&completion),
        );
    }

    fn handle_objc_show_settings(&self, _sender: &NSMenuItem) {
        self.show_commit_message_settings();
    }

    fn handle_objc_workspace_use_global_changed(&self, _sender: &NSButton) {
        self.update_workspace_settings_control_state();
    }

    fn handle_objc_save_workspace_settings(&self, _sender: &NSButton) {
        self.request_workspace_settings_save();
    }

    fn handle_objc_save_font_sizes(&self, _sender: &NSButton) {
        self.apply_native_font_size_settings();
    }

    fn handle_objc_increase_font_size(&self, _sender: &NSMenuItem) {
        self.adjust_native_active_font_size(1.0);
    }

    fn handle_objc_decrease_font_size(&self, _sender: &NSMenuItem) {
        self.adjust_native_active_font_size(-1.0);
    }

    fn handle_objc_reset_font_size(&self, _sender: &NSMenuItem) {
        self.reset_native_active_font_size();
    }

    fn handle_objc_show_keyboard_shortcuts(&self, _sender: &NSMenuItem) {
        self.show_native_shortcuts_window();
    }

    fn handle_objc_open_craic_website(&self, _sender: &NSMenuItem) {
        self.open_native_help_url("https://soirihiroka.github.io/craic/");
    }

    fn handle_objc_report_craic_issue(&self, _sender: &NSMenuItem) {
        self.open_native_help_url("https://github.com/soirihiroka/craic/issues");
    }

    fn handle_objc_pull_remote(&self, _sender: &NSMenuItem) {
        self.request_remote_action(NativeRemoteAction::Pull);
    }

    fn handle_objc_push_remote(&self, _sender: &NSMenuItem) {
        self.request_remote_action(NativeRemoteAction::Push);
    }

    fn handle_objc_commit_message_provider_changed(&self, sender: &NSPopUpButton) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(provider_id) = settings.provider_ids.get(index).cloned() else {
            return;
        };
        settings.current_provider.replace(provider_id.clone());
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.set_commit_message_settings_error("The settings service is unavailable.");
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::SaveCommitMessageProvider {
            provider_id: provider_id.clone(),
        }) {
            self.set_commit_message_settings_error(&format!(
                "Unable to save the provider selection: {error}"
            ));
            return;
        }
        self.request_commit_message_models(provider_id, None);
    }

    fn handle_objc_commit_message_model_changed(&self, sender: &NSPopUpButton) {
        let Some(settings) = self.ivars().commit_message_settings.get() else {
            return;
        };
        let Ok(index) = usize::try_from(sender.indexOfSelectedItem()) else {
            return;
        };
        let Some(model) = settings.model_ids.borrow().get(index).cloned() else {
            return;
        };
        let provider_id = settings.current_provider.borrow().clone();
        let Some(requests) = self.ivars().repository_requests.get() else {
            self.set_commit_message_settings_error("The settings service is unavailable.");
            return;
        };
        if let Err(error) = requests.try_send(RepositoryRequest::SaveCommitMessageModel {
            provider_id,
            model,
        }) {
            self.set_commit_message_settings_error(&format!(
                "Unable to save the model selection: {error}"
            ));
        } else {
            settings
                .status
                .setStringValue(&NSString::from_str("Commit-message settings saved."));
        }
    }

    fn handle_objc_toggle_branch_picker(&self, sender: &NSButton) {
        self.show_branch_picker(sender);
    }

    fn handle_objc_filter_branches(&self, sender: &NSSearchField) {
        self.refresh_branch_results(&sender.stringValue().to_string());
    }

    fn handle_objc_activate_branch_row(&self, sender: &NSButton) {
        let Ok(index) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(snapshot) = self.ivars().repository_snapshot.borrow().clone() else {
            return;
        };
        let Some(branch) = snapshot.branches.get(index).map(|branch| branch.name.clone()) else {
            log::warn!("branch selection index out of range index={index}");
            return;
        };
        if !self.ivars().branch_merge_mode.get() && branch == snapshot.branch {
            if let Some(popover) = self.ivars().branch_popover.get() {
                popover.close();
            }
            return;
        }
        if self.ivars().branch_merge_mode.get() && branch == snapshot.branch {
            return;
        }
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
        let action = if self.ivars().branch_merge_mode.get() {
            BranchAction::Merge(branch)
        } else {
            BranchAction::Checkout(branch)
        };
        let request = RepositoryRequest::RunBranchAction {
            workspace_id,
            handle,
            action,
            cancellation,
        };
        if let Err(error) = requests.try_send(request) {
            log::warn!("branch action queue rejected request error={error}");
            return;
        }
        if let Some(popover) = self.ivars().branch_popover.get() {
            popover.close();
        }
    }
}
