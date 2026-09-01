impl AppDelegate {
    fn handle_objc_copy_terminal_working_directory(&self, sender: &NSMenuItem) {
        let working_directory = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == sender.tag())
            .map(|session| session.working_directory.clone());
        if let Some(working_directory) = working_directory {
            self.copy_text_to_pasteboard(&working_directory);
        }
    }

    fn handle_objc_reveal_terminal_working_directory(&self, sender: &NSMenuItem) {
        let local_path = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == sender.tag())
            .and_then(|session| session.local_working_directory.clone());
        let Some(local_path) = local_path else {
            return;
        };
        let url = NSURL::fileURLWithPath(&NSString::from_str(&local_path.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_slice(&[&*url]));
    }

    fn handle_objc_move_terminal_session_left(&self, sender: &NSMenuItem) {
        self.reorder_native_terminal_session(sender.tag(), -1);
    }

    fn handle_objc_move_terminal_session_right(&self, sender: &NSMenuItem) {
        self.reorder_native_terminal_session(sender.tag(), 1);
    }

    fn handle_objc_close_terminal_session_from_menu(&self, sender: &NSMenuItem) {
        self.request_native_terminal_close(sender.tag());
    }

    fn handle_objc_filter_terminal(&self, _sender: &NSSearchField) {
        self.apply_native_terminal_search(TerminalSearchDirection::Next);
    }

    fn handle_objc_previous_terminal_match(&self, _sender: &NSButton) {
        self.apply_native_terminal_search(TerminalSearchDirection::Previous);
    }

    fn handle_objc_next_terminal_match(&self, _sender: &NSButton) {
        self.apply_native_terminal_search(TerminalSearchDirection::Next);
    }

    fn handle_objc_toggle_terminal_search_option(&self, _sender: &NSButton) {
        self.apply_native_terminal_search(TerminalSearchDirection::Next);
    }

    fn handle_objc_close_terminal_search(&self, _sender: &NSButton) {
        self.hide_native_terminal_search();
    }

    fn handle_objc_auto_close_terminal_session(&self, timer: &NSTimer) {
        let Some(id) = timer
            .userInfo()
            .and_then(|value| value.downcast::<NSString>().ok())
            .and_then(|value| value.to_string().parse::<isize>().ok())
        else {
            return;
        };
        let should_close = self
            .ivars()
            .terminal_sessions
            .borrow()
            .iter()
            .find(|session| session.id == id)
            .is_some_and(|session| session.view.exited_successfully());
        if should_close {
            log::info!(
                "auto-closing native terminal id={id} after {}s of inactivity",
                TERMINAL_AUTO_CLOSE_IDLE_SECONDS
            );
            self.finish_native_terminal_close(id);
        }
    }

    fn handle_objc_refresh_agent_terminal_usage(&self, _timer: &NSTimer) {
        self.sample_native_agent_terminal_usage();
    }

    fn handle_objc_hide_native_toast(&self, _timer: &NSTimer) {
        self.ivars().toast_timer.borrow_mut().take();
        if let Some(toast) = self.ivars().toast.get() {
            toast.setHidden(true);
        }
    }

    fn handle_objc_run_quick_action(&self, sender: &NSToolbarItem) {
        let Ok(slot) = usize::try_from(sender.tag()) else {
            return;
        };
        let Some(target) = self.native_quick_action_target(slot) else {
            self.present_path_action_error(
                "Choose a Quick Action",
                "Use the arrow beside the toolbar button to choose a discovered project action, or run an explicit command.",
            );
            return;
        };
        self.run_native_quick_action(target);
    }

    fn handle_objc_select_quick_action(&self, sender: &NSMenuItem) {
        if let Some(menu) = unsafe { sender.menu() } {
            menu.cancelTracking();
        }
        let Ok(encoded) = usize::try_from(sender.tag()) else {
            return;
        };
        let target_count = self.ivars().quick_action_targets.borrow().len();
        if target_count == 0 {
            return;
        }
        let slot = encoded / target_count;
        let target_index = encoded % target_count;
        let target = self
            .ivars()
            .quick_action_targets
            .borrow()
            .get(target_index)
            .cloned();
        let Some(target) = target else {
            return;
        };
        self.select_native_quick_action(slot, &target);
    }

    fn handle_objc_add_quick_action(&self, _sender: &NSToolbarItem) {
        self.ivars()
            .quick_action_configs
            .borrow_mut()
            .push(QuickActionConfig::default());
        self.configure_native_quick_action_group();
        self.save_native_quick_action_configuration();
    }

    fn handle_objc_remove_quick_action(&self, sender: &NSMenuItem) {
        if let Some(menu) = unsafe { sender.menu() } {
            menu.cancelTracking();
        }
        let Ok(slot) = usize::try_from(sender.tag()) else {
            return;
        };
        if slot >= self.ivars().quick_action_configs.borrow().len() {
            return;
        }
        self.ivars().quick_action_configs.borrow_mut().remove(slot);
        self.configure_native_quick_action_group();
        self.save_native_quick_action_configuration();
    }

    fn handle_objc_refresh_quick_actions(&self, _sender: &NSMenuItem) {
        self.request_native_quick_actions();
    }

    fn handle_objc_run_quick_command(&self, _sender: &NSMenuItem) {
        self.show_native_quick_command_sheet();
    }
}
