impl AppDelegate {
    fn handle_objc_delegate_application_did_finish_launching(&self, notification: &NSNotification) {
        self.did_finish_launching(notification);
    }

    fn handle_objc_delegate_should_terminate_after_last_window_closed(&self, _application: &NSApplication) -> bool {
        true
    }

    fn handle_objc_delegate_application_should_terminate(
        &self,
        _application: &NSApplication,
    ) -> NSApplicationTerminateReply {
        if self.ivars().close_confirmed.get() || !self.has_active_native_session() {
            self.prepare_for_native_shutdown();
            self.schedule_controlled_app_stop();
            return NSApplicationTerminateReply::TerminateCancel;
        }

        self.ivars()
            .quit_requested_during_close_confirmation
            .set(true);
        if !self.present_close_confirmation() {
            self.ivars()
                .quit_requested_during_close_confirmation
                .set(false);
            self.prepare_for_native_shutdown();
            self.schedule_controlled_app_stop();
        }
        // Cancel AppKit's termination transaction immediately. The retained native sheet
        // owns the user decision and re-enters the controlled shutdown path after it has
        // dismissed, avoiding the circular modal wait imposed by `TerminateLater`.
        NSApplicationTerminateReply::TerminateCancel
    }

    fn handle_objc_delegate_application_will_terminate(&self, _notification: &NSNotification) {
        self.prepare_for_native_shutdown();
    }

    fn handle_objc_delegate_window_should_close(&self, _window: &NSWindow) -> Bool {
        if self.ivars().close_confirmed.get() || !self.has_active_native_session() {
            return Bool::YES;
        }

        Bool::new(!self.present_close_confirmation())
    }

    fn handle_objc_delegate_window_did_resize(&self, _notification: &NSNotification) {
        self.layout_sidebar();
        self.layout_content();
        self.layout_native_terminal_tabs();
    }

    fn handle_objc_delegate_window_did_change_occlusion_state(&self, _notification: &NSNotification) {
        self.update_native_renderer_occlusion();
    }

    fn handle_objc_delegate_window_will_close(&self, _notification: &NSNotification) {
        self.prepare_for_native_shutdown();
        self.schedule_controlled_app_stop();
    }

    fn handle_objc_delegate_menu_will_open(&self, menu: &NSMenu) {
        if let Some(history) = self.ivars().history.get()
            && std::ptr::eq(menu, &*history.menu)
        {
            let clicked = history.table.clickedRow();
            if let Ok(clicked) = usize::try_from(clicked)
                && clicked < history.commits.borrow().len()
            {
                history.table.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(clicked),
                    false,
                );
                self.select_history_commit(clicked);
            } else {
                menu.cancelTrackingWithoutAnimation();
                return;
            }
            self.rebuild_history_menu();
            return;
        }
        if let Some(files) = self.ivars().files.get()
            && std::ptr::eq(menu, &*files.menu)
        {
            let clicked = files.table.clickedRow();
            if let Ok(clicked) = usize::try_from(clicked)
                && clicked < self.filtered_file_tree_rows().len()
            {
                files.table.selectRowIndexes_byExtendingSelection(
                    &NSIndexSet::indexSetWithIndex(clicked),
                    false,
                );
            }
            self.rebuild_workspace_file_menu();
        }
    }

    fn handle_objc_delegate_split_view_did_resize_subviews(&self, _notification: &NSNotification) {
        self.layout_native_terminal_panel();
    }

    fn handle_objc_delegate_split_view_constrain_split_position_of_subview_at(
        &self,
        split_view: &NSSplitView,
        proposed_position: f64,
        divider_index: isize,
    ) -> f64 {
        if divider_index != 0 {
            return proposed_position;
        }
        let height = split_view.bounds().size.height;
        if self
            .ivars()
            .changes_split
            .get()
            .is_some_and(|changes_split| std::ptr::eq(split_view, &**changes_split))
        {
            let minimum_browser_height = 160.0;
            let minimum_composer_height = 160.0;
            let maximum_composer_height =
                520.0_f64.min((height - minimum_browser_height).max(0.0));
            let minimum_position =
                (height - maximum_composer_height).max(minimum_browser_height);
            let maximum_position = (height - minimum_composer_height).max(minimum_position);
            return proposed_position.clamp(minimum_position, maximum_position);
        }
        if self
            .ivars()
            .content_split
            .get()
            .is_some_and(|content_split| std::ptr::eq(split_view, &**content_split))
        {
            let minimum_page_height = 180.0;
            let minimum_terminal_height = 140.0;
            let maximum_terminal_height =
                520.0_f64.min((height - minimum_page_height).max(0.0));
            let minimum_position =
                (height - maximum_terminal_height).max(minimum_page_height);
            let maximum_position = (height - minimum_terminal_height).max(minimum_position);
            return proposed_position.clamp(minimum_position, maximum_position);
        }
        proposed_position
    }
}
