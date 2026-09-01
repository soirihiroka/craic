include!("application/prelude.rs");
include!("application/state.rs");
include!("application/messages.rs");

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class has no Drop implementation.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppDelegateIvars]
    pub(crate) struct AppDelegate;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: Method signatures match NSApplicationDelegate.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, notification: &NSNotification){
            self.handle_objc_delegate_application_did_finish_launching(notification)
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _application: &NSApplication) -> bool{
            self.handle_objc_delegate_should_terminate_after_last_window_closed(_application)
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn application_should_terminate(
            &self,
            _application: &NSApplication,
        ) -> NSApplicationTerminateReply{
            self.handle_objc_delegate_application_should_terminate(_application)
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn application_will_terminate(&self, _notification: &NSNotification){
            self.handle_objc_delegate_application_will_terminate(_notification)
        }
    }

    // SAFETY: Method signatures match NSWindowDelegate.
    unsafe impl NSWindowDelegate for AppDelegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _window: &NSWindow) -> Bool{
            self.handle_objc_delegate_window_should_close(_window)
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification){
            self.handle_objc_delegate_window_did_resize(_notification)
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn window_did_change_occlusion_state(&self, _notification: &NSNotification){
            self.handle_objc_delegate_window_did_change_occlusion_state(_notification)
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification){
            self.handle_objc_delegate_window_will_close(_notification)
        }
    }

    // SAFETY: The retained Containers menu invokes this main-thread delegate synchronously.
    unsafe impl NSMenuDelegate for AppDelegate {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu){
            self.handle_objc_delegate_menu_will_open(menu)
        }
    }

    // SAFETY: AppDelegate is main-thread-only and the retained Changes split view invokes its
    // delegate synchronously while dragging its single native divider.
    unsafe impl NSSplitViewDelegate for AppDelegate {
        #[unsafe(method(splitViewDidResizeSubviews:))]
        #[allow(non_snake_case)]
        fn splitViewDidResizeSubviews(&self, _notification: &NSNotification){
            self.handle_objc_delegate_split_view_did_resize_subviews(_notification)
        }

        #[unsafe(method(splitView:constrainSplitPosition:ofSubviewAt:))]
        #[allow(non_snake_case)]
        fn splitView_constrainSplitPosition_ofSubviewAt(
            &self,
            split_view: &NSSplitView,
            proposed_position: f64,
            divider_index: isize,
        ) -> f64{
            self.handle_objc_delegate_split_view_constrain_split_position_of_subview_at(split_view, proposed_position, divider_index)
        }
    }

    // SAFETY: AppDelegate is main-thread-only and satisfies the delegate's NSObject contract.
    unsafe impl NSControlTextEditingDelegate for AppDelegate {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification){
            self.handle_objc_delegate_control_text_did_change(notification)
        }
    }

    // SAFETY: AppDelegate is main-thread-only and implements the text editing delegate contract.
    unsafe impl NSTextFieldDelegate for AppDelegate {}

    // SAFETY: AppDelegate is main-thread-only and receives notifications only from retained text
    // views whose delegate it owns for the lifetime of the window.
    unsafe impl NSTextDelegate for AppDelegate {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, notification: &NSNotification){
            self.handle_objc_delegate_text_did_change(notification)
        }
    }

    // SAFETY: AppDelegate is main-thread-only and only handles links installed into its retained
    // native Codex transcript view.
    unsafe impl NSTextViewDelegate for AppDelegate {
        #[unsafe(method(textView:clickedOnLink:atIndex:))]
        #[allow(non_snake_case)]
        unsafe fn textView_clickedOnLink_atIndex(
            &self,
            text_view: &NSTextView,
            link: &AnyObject,
            _char_index: usize,
        ) -> Bool{
            unsafe { self.handle_objc_delegate_text_view_clicked_on_link_at_index(text_view, link, _char_index) }
        }

    }

    // SAFETY: The retained WKWebView invokes its weak delegate on AppKit's main thread, and the
    // policy handler is called exactly once for every navigation decision.
    unsafe impl WKNavigationDelegate for AppDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        #[allow(non_snake_case)]
        unsafe fn webView_decidePolicyForNavigationAction_decisionHandler(
            &self,
            _web_view: &WKWebView,
            navigation_action: &WKNavigationAction,
            decision_handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ){
            unsafe { self.handle_objc_delegate_web_view_decide_policy_for_navigation_action_decision_handler(_web_view, navigation_action, decision_handler) }
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        #[allow(non_snake_case)]
        unsafe fn webView_didFinishNavigation(
            &self,
            web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
        ){
            unsafe { self.handle_objc_delegate_web_view_did_finish_navigation(web_view, _navigation) }
        }
    }

    // SAFETY: Menu validation runs on AppKit's main thread and only reads frontend-owned state.
    unsafe impl NSMenuItemValidation for AppDelegate {
        #[unsafe(method(validateMenuItem:))]
        #[allow(non_snake_case)]
        fn validateMenuItem(&self, menu_item: &NSMenuItem) -> objc2::runtime::Bool{
            self.handle_objc_delegate_validate_menu_item(menu_item)
        }
    }

    // SAFETY: The table view only calls these methods on AppKit's main thread.
    unsafe impl NSTableViewDataSource for AppDelegate {
        #[unsafe(method(numberOfRowsInTableView:))]
        #[allow(non_snake_case)]
        fn numberOfRowsInTableView(&self, table: &NSTableView) -> isize{
            self.handle_objc_delegate_number_of_rows_in_table_view(table)
        }

        #[unsafe(method(tableView:writeRowsWithIndexes:toPasteboard:))]
        #[allow(non_snake_case)]
        fn tableView_writeRowsWithIndexes_toPasteboard(
            &self,
            table: &NSTableView,
            rows: &NSIndexSet,
            pasteboard: &NSPasteboard,
        ) -> objc2::runtime::Bool{
            self.handle_objc_delegate_table_view_write_rows_with_indexes_to_pasteboard(table, rows, pasteboard)
        }

        #[unsafe(method(tableView:validateDrop:proposedRow:proposedDropOperation:))]
        #[allow(non_snake_case)]
        fn tableView_validateDrop_proposedRow_proposedDropOperation(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            _drop_operation: NSTableViewDropOperation,
        ) -> NSDragOperation{
            self.handle_objc_delegate_table_view_validate_drop_proposed_row_proposed_drop_operation(table, info, row, _drop_operation)
        }

        #[unsafe(method(tableView:acceptDrop:row:dropOperation:))]
        #[allow(non_snake_case)]
        fn tableView_acceptDrop_row_dropOperation(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            _drop_operation: NSTableViewDropOperation,
        ) -> objc2::runtime::Bool{
            self.handle_objc_delegate_table_view_accept_drop_row_drop_operation(table, info, row, _drop_operation)
        }
    }

    // SAFETY: Returned views are retained and all callbacks remain on AppKit's main thread.
    unsafe impl NSTableViewDelegate for AppDelegate {
        #[unsafe(method(tableView:isGroupRow:))]
        #[allow(non_snake_case)]
        fn tableView_isGroupRow(&self, table: &NSTableView, row: isize) -> bool{
            self.handle_objc_delegate_table_view_is_group_row(table, row)
        }

        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        #[allow(non_snake_case)]
        fn tableView_viewForTableColumn_row(
            &self,
            table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>>{
            self.handle_objc_delegate_table_view_view_for_table_column_row(table, column, row)
        }

        #[unsafe(method_id(tableView:rowViewForRow:))]
        #[allow(non_snake_case)]
        fn tableView_rowViewForRow(
            &self,
            table: &NSTableView,
            _row: isize,
        ) -> Option<Retained<NSTableRowView>>{
            self.handle_objc_delegate_table_view_row_view_for_row(table, _row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        #[allow(non_snake_case)]
        fn tableViewSelectionDidChange(&self, notification: &NSNotification){
            self.handle_objc_delegate_table_view_selection_did_change(notification)
        }

        #[unsafe(method(tableView:didClickTableColumn:))]
        #[allow(non_snake_case)]
        fn tableView_didClickTableColumn(
            &self,
            table: &NSTableView,
            table_column: &NSTableColumn,
        ){
            self.handle_objc_delegate_table_view_did_click_table_column(table, table_column)
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        #[allow(non_snake_case)]
        fn tableView_shouldSelectRow(
            &self,
            table: &NSTableView,
            row: isize,
        ) -> objc2::runtime::Bool{
            self.handle_objc_delegate_table_view_should_select_row(table, row)
        }

        #[unsafe(method(tableView:heightOfRow:))]
        #[allow(non_snake_case)]
        fn tableView_heightOfRow(&self, table: &NSTableView, row: isize) -> f64{
            self.handle_objc_delegate_table_view_height_of_row(table, row)
        }
    }

    impl AppDelegate {
        #[unsafe(method(stopApplicationRunLoop:))]
        fn stop_application_run_loop(&self, _sender: Option<&AnyObject>){
            self.handle_objc_stop_application_run_loop(_sender)
        }

        #[unsafe(method(finishConfirmedClose:))]
        fn finish_confirmed_close(&self, _sender: Option<&AnyObject>){
            self.handle_objc_finish_confirmed_close(_sender)
        }

        #[unsafe(method(newAgentChat:))]
        fn new_agent_chat(&self, _sender: &NSButton){
            self.handle_objc_new_agent_chat(_sender)
        }

        #[unsafe(method(newCodexCli:))]
        fn new_codex_cli(&self, _sender: &NSButton){
            self.handle_objc_new_codex_cli(_sender)
        }

        #[unsafe(method(newAgy:))]
        fn new_agy(&self, _sender: &NSButton){
            self.handle_objc_new_agy(_sender)
        }

        #[unsafe(method(sendAgentMessage:))]
        fn send_agent_message(&self, _sender: &NSButton){
            self.handle_objc_send_agent_message(_sender)
        }

        #[unsafe(method(attachAgentFiles:))]
        fn attach_agent_files(&self, _sender: &AnyObject){
            self.handle_objc_attach_agent_files(_sender)
        }

        #[unsafe(method(referenceAgentFile:))]
        fn reference_agent_file(&self, _sender: &NSMenuItem){
            self.handle_objc_reference_agent_file(_sender)
        }

        #[unsafe(method(referenceAgentFolder:))]
        fn reference_agent_folder(&self, _sender: &NSMenuItem){
            self.handle_objc_reference_agent_folder(_sender)
        }

        #[unsafe(method(clearAgentAttachments:))]
        fn clear_agent_attachments(&self, _sender: &NSButton){
            self.handle_objc_clear_agent_attachments(_sender)
        }

        #[unsafe(method(stopAgentTurn:))]
        fn stop_agent_turn(&self, _sender: &NSButton){
            self.handle_objc_stop_agent_turn(_sender)
        }

        #[unsafe(method(selectAgentModel:))]
        fn select_agent_model(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_model(sender)
        }

        #[unsafe(method(selectAgentReasoning:))]
        fn select_agent_reasoning(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_reasoning(sender)
        }

        #[unsafe(method(selectAgentPersonality:))]
        fn select_agent_personality(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_personality(sender)
        }

        #[unsafe(method(selectAgentServiceTier:))]
        fn select_agent_service_tier(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_service_tier(sender)
        }

        #[unsafe(method(selectAgentPermissions:))]
        fn select_agent_permissions(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_permissions(sender)
        }

        #[unsafe(method(resumeAgentThread:))]
        fn resume_agent_thread(&self, sender: &NSButton){
            self.handle_objc_resume_agent_thread(sender)
        }

        #[unsafe(method(filterAgentThreads:))]
        fn filter_agent_threads(&self, sender: &NSSearchField){
            self.handle_objc_filter_agent_threads(sender)
        }

        #[unsafe(method(selectAgentThreadScope:))]
        fn select_agent_thread_scope(&self, sender: &NSPopUpButton){
            self.handle_objc_select_agent_thread_scope(sender)
        }

        #[unsafe(method(renameAgentThread:))]
        fn rename_agent_thread(&self, sender: &NSMenuItem){
            self.handle_objc_rename_agent_thread(sender)
        }

        #[unsafe(method(archiveAgentThread:))]
        fn archive_agent_thread(&self, sender: &NSMenuItem){
            self.handle_objc_archive_agent_thread(sender)
        }

        #[unsafe(method(unarchiveAgentThread:))]
        fn unarchive_agent_thread(&self, sender: &NSMenuItem){
            self.handle_objc_unarchive_agent_thread(sender)
        }

        #[unsafe(method(deleteAgentThread:))]
        fn delete_agent_thread(&self, sender: &NSMenuItem){
            self.handle_objc_delete_agent_thread(sender)
        }

        #[unsafe(method(showAgentThreadHistory:))]
        fn show_agent_thread_history(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_thread_history(_sender)
        }

        #[unsafe(method(showAgentThreadGoal:))]
        fn show_agent_thread_goal(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_thread_goal(_sender)
        }

        #[unsafe(method(runAgentShellCommand:))]
        fn run_agent_shell_command(&self, _sender: &NSMenuItem){
            self.handle_objc_run_agent_shell_command(_sender)
        }

        #[unsafe(method(showAgentBackgroundTerminals:))]
        fn show_agent_background_terminals(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_background_terminals(_sender)
        }

        #[unsafe(method(showAgentSkills:))]
        fn show_agent_skills(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_skills(_sender)
        }

        #[unsafe(method(showAgentMcpServers:))]
        fn show_agent_mcp_servers(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_mcp_servers(_sender)
        }

        #[unsafe(method(showAgentApps:))]
        fn show_agent_apps(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_apps(_sender)
        }

        #[unsafe(method(showAgentPlugins:))]
        fn show_agent_plugins(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_plugins(_sender)
        }

        #[unsafe(method(showAgentExperimentalFeatures:))]
        fn show_agent_experimental_features(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_experimental_features(_sender)
        }

        #[unsafe(method(showAgentAccountUsage:))]
        fn show_agent_account_usage(&self, _sender: &NSMenuItem){
            self.handle_objc_show_agent_account_usage(_sender)
        }

        #[unsafe(method(forkActiveAgentThread:))]
        fn fork_active_agent_thread(&self, _sender: &NSMenuItem){
            self.handle_objc_fork_active_agent_thread(_sender)
        }

        #[unsafe(method(compactActiveAgentThread:))]
        fn compact_active_agent_thread(&self, _sender: &NSMenuItem){
            self.handle_objc_compact_active_agent_thread(_sender)
        }

        #[unsafe(method(startAgentReview:))]
        fn start_agent_review(&self, _sender: &NSMenuItem){
            self.handle_objc_start_agent_review(_sender)
        }

        #[unsafe(method(rollbackActiveAgentThread:))]
        fn rollback_active_agent_thread(&self, _sender: &NSMenuItem){
            self.handle_objc_rollback_active_agent_thread(_sender)
        }

        #[unsafe(method(archiveActiveAgentThread:))]
        fn archive_active_agent_thread(&self, _sender: &NSMenuItem){
            self.handle_objc_archive_active_agent_thread(_sender)
        }

        #[unsafe(method(openAgentChanges:))]
        fn open_agent_changes(&self, _sender: &NSMenuItem){
            self.handle_objc_open_agent_changes(_sender)
        }

        #[unsafe(method(selectPage:))]
        fn select_page(&self, sender: &NSToolbarItemGroup){
            self.handle_objc_select_page(sender)
        }

        #[unsafe(method(filterFiles:))]
        fn filter_files(&self, sender: &NSSearchField){
            self.handle_objc_filter_files(sender)
        }

        #[unsafe(method(filterContainers:))]
        fn filter_containers(&self, sender: &NSSearchField){
            self.handle_objc_filter_containers(sender)
        }

        #[unsafe(method(showContainerLogs:))]
        fn show_container_logs(&self, _sender: &NSButton){
            self.handle_objc_show_container_logs(_sender)
        }

        #[unsafe(method(inspectContainer:))]
        fn inspect_container(&self, _sender: &NSButton){
            self.handle_objc_inspect_container(_sender)
        }

        #[unsafe(method(attachContainerShell:))]
        fn attach_container_shell(&self, _sender: &NSButton){
            self.handle_objc_attach_container_shell(_sender)
        }

        #[unsafe(method(startContainer:))]
        fn start_container(&self, _sender: &NSButton){
            self.handle_objc_start_container(_sender)
        }

        #[unsafe(method(stopContainer:))]
        fn stop_container(&self, _sender: &NSButton){
            self.handle_objc_stop_container(_sender)
        }

        #[unsafe(method(restartContainer:))]
        fn restart_container(&self, _sender: &NSButton){
            self.handle_objc_restart_container(_sender)
        }

        #[unsafe(method(removeContainer:))]
        fn remove_container(&self, _sender: &NSButton){
            self.handle_objc_remove_container(_sender)
        }

        #[unsafe(method(newWorkspaceFile:))]
        fn new_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_new_workspace_file(_sender)
        }

        #[unsafe(method(newWorkspaceFolder:))]
        fn new_workspace_folder(&self, _sender: &NSMenuItem){
            self.handle_objc_new_workspace_folder(_sender)
        }

        #[unsafe(method(uploadWorkspaceFiles:))]
        fn upload_workspace_files(&self, _sender: &NSMenuItem){
            self.handle_objc_upload_workspace_files(_sender)
        }

        #[unsafe(method(renameWorkspaceFile:))]
        fn rename_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_rename_workspace_file(_sender)
        }

        #[unsafe(method(duplicateWorkspaceFile:))]
        fn duplicate_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_duplicate_workspace_file(_sender)
        }

        #[unsafe(method(moveWorkspaceFile:))]
        fn move_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_move_workspace_file(_sender)
        }

        #[unsafe(method(deleteWorkspaceFile:))]
        fn delete_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_delete_workspace_file(_sender)
        }

        #[unsafe(method(selectSqliteTable:))]
        #[allow(non_snake_case)]
        fn select_sqlite_table(&self, sender: &NSPopUpButton){
            self.handle_objc_select_sqlite_table(sender)
        }

        #[unsafe(method(selectSqliteFilterColumn:))]
        #[allow(non_snake_case)]
        fn select_sqlite_filter_column(&self, sender: &NSPopUpButton){
            self.handle_objc_select_sqlite_filter_column(sender)
        }

        #[unsafe(method(filterSqliteRows:))]
        #[allow(non_snake_case)]
        fn filter_sqlite_rows(&self, sender: &NSSearchField){
            self.handle_objc_filter_sqlite_rows(sender)
        }

        #[unsafe(method(previousSqlitePage:))]
        #[allow(non_snake_case)]
        fn previous_sqlite_page(&self, _sender: &NSButton){
            self.handle_objc_previous_sqlite_page(_sender)
        }

        #[unsafe(method(nextSqlitePage:))]
        #[allow(non_snake_case)]
        fn next_sqlite_page(&self, _sender: &NSButton){
            self.handle_objc_next_sqlite_page(_sender)
        }

        #[unsafe(method(reloadSqlitePreview:))]
        #[allow(non_snake_case)]
        fn reload_sqlite_preview(&self, _sender: &NSButton){
            self.handle_objc_reload_sqlite_preview(_sender)
        }

        #[unsafe(method(toggleFileDirectory:))]
        fn toggle_file_directory(&self, sender: &NSButton){
            self.handle_objc_toggle_file_directory(sender)
        }

        #[unsafe(method(activateWorkspaceSelection:))]
        fn activate_workspace_selection(&self, _sender: &AnyObject){
            self.handle_objc_activate_workspace_selection(_sender)
        }

        #[unsafe(method(openWorkspaceFile:))]
        fn open_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_open_workspace_file(_sender)
        }

        #[unsafe(method(revealWorkspaceFile:))]
        fn reveal_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_reveal_workspace_file(_sender)
        }

        #[unsafe(method(openWorkspaceFileInTerminal:))]
        fn open_workspace_file_in_terminal(&self, _sender: &NSMenuItem){
            self.handle_objc_open_workspace_file_in_terminal(_sender)
        }

        #[unsafe(method(runWorkspaceFileInTerminal:))]
        fn run_workspace_file_in_terminal(&self, _sender: &NSMenuItem){
            self.handle_objc_run_workspace_file_in_terminal(_sender)
        }

        #[unsafe(method(addWorkspaceFileToChat:))]
        fn add_workspace_file_to_chat(&self, _sender: &NSMenuItem){
            self.handle_objc_add_workspace_file_to_chat(_sender)
        }

        #[unsafe(method(addWorkspaceIgnorePattern:))]
        fn add_workspace_ignore_pattern(&self, sender: &NSMenuItem){
            self.handle_objc_add_workspace_ignore_pattern(sender)
        }

        #[unsafe(method(runWorkspaceContainerFileAction:))]
        fn run_workspace_container_file_action(&self, sender: &NSMenuItem){
            self.handle_objc_run_workspace_container_file_action(sender)
        }

        #[unsafe(method(copyWorkspaceFileRelativePath:))]
        fn copy_workspace_file_relative_path(&self, _sender: &NSMenuItem){
            self.handle_objc_copy_workspace_file_relative_path(_sender)
        }

        #[unsafe(method(copyWorkspaceFileProviderPath:))]
        fn copy_workspace_file_provider_path(&self, _sender: &NSMenuItem){
            self.handle_objc_copy_workspace_file_provider_path(_sender)
        }

        #[unsafe(method(copyWorkspaceFile:))]
        fn copy_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_copy_workspace_file(_sender)
        }

        #[unsafe(method(cutWorkspaceFile:))]
        fn cut_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_cut_workspace_file(_sender)
        }

        #[unsafe(method(pasteWorkspaceFile:))]
        fn paste_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_paste_workspace_file(_sender)
        }

        #[unsafe(method(downloadWorkspaceFile:))]
        fn download_workspace_file(&self, _sender: &NSMenuItem){
            self.handle_objc_download_workspace_file(_sender)
        }

        #[unsafe(method(filterDiff:))]
        fn filter_diff(&self, sender: &NSSearchField){
            self.handle_objc_filter_diff(sender)
        }

        #[unsafe(method(previousDiffMatch:))]
        fn previous_diff_match(&self, _sender: &NSButton){
            self.handle_objc_previous_diff_match(_sender)
        }

        #[unsafe(method(nextDiffMatch:))]
        fn next_diff_match(&self, _sender: &NSButton){
            self.handle_objc_next_diff_match(_sender)
        }

        #[unsafe(method(closeDiffSearch:))]
        fn close_diff_search(&self, _sender: &NSButton){
            self.handle_objc_close_diff_search(_sender)
        }

        #[unsafe(method(filterEditor:))]
        fn filter_editor(&self, sender: &NSSearchField){
            self.handle_objc_filter_editor(sender)
        }

        #[unsafe(method(previousEditorMatch:))]
        fn previous_editor_match(&self, _sender: &NSButton){
            self.handle_objc_previous_editor_match(_sender)
        }

        #[unsafe(method(nextEditorMatch:))]
        fn next_editor_match(&self, _sender: &NSButton){
            self.handle_objc_next_editor_match(_sender)
        }

        #[unsafe(method(closeEditorSearch:))]
        fn close_editor_search(&self, _sender: &NSButton){
            self.handle_objc_close_editor_search(_sender)
        }

        #[unsafe(method(findContent:))]
        fn find_content(&self, sender: &NSMenuItem){
            self.handle_objc_find_content(sender)
        }

        #[unsafe(method(filterChangedFiles:))]
        fn filter_changed_files(&self, sender: &NSSearchField){
            self.handle_objc_filter_changed_files(sender)
        }

        #[unsafe(method(closeChangedFilesSearch:))]
        fn close_changed_files_search(&self, _sender: &NSButton){
            self.handle_objc_close_changed_files_search(_sender)
        }

        #[unsafe(method(activatePageFromMenu:))]
        fn activate_page_from_menu(&self, sender: &NSMenuItem){
            self.handle_objc_activate_page_from_menu(sender)
        }

        #[unsafe(method(filterHistory:))]
        fn filter_history(&self, sender: &NSSearchField){
            self.handle_objc_filter_history(sender)
        }

        #[unsafe(method(historyClipBoundsChanged:))]
        fn history_clip_bounds_changed(&self, _notification: &NSNotification){
            self.handle_objc_history_clip_bounds_changed(_notification)
        }

        #[unsafe(method(copyHistoryHash:))]
        fn copy_history_hash(&self, _sender: &NSButton){
            self.handle_objc_copy_history_hash(_sender)
        }

        #[unsafe(method(openHistoryRemote:))]
        fn open_history_remote(&self, _sender: &NSButton){
            self.handle_objc_open_history_remote(_sender)
        }

        #[unsafe(method(checkoutHistoryCommit:))]
        fn checkout_history_commit(&self, _sender: &NSMenuItem){
            self.handle_objc_checkout_history_commit(_sender)
        }

        #[unsafe(method(checkoutHistoryParent:))]
        fn checkout_history_parent(&self, _sender: &NSMenuItem){
            self.handle_objc_checkout_history_parent(_sender)
        }

        #[unsafe(method(newHistoryBranch:))]
        fn new_history_branch(&self, _sender: &NSMenuItem){
            self.handle_objc_new_history_branch(_sender)
        }

        #[unsafe(method(createHistoryTag:))]
        fn create_history_tag(&self, _sender: &NSMenuItem){
            self.handle_objc_create_history_tag(_sender)
        }

        #[unsafe(method(cherryPickHistoryCommit:))]
        fn cherry_pick_history_commit(&self, _sender: &NSMenuItem){
            self.handle_objc_cherry_pick_history_commit(_sender)
        }

        #[unsafe(method(revertHistoryCommit:))]
        fn revert_history_commit(&self, _sender: &NSMenuItem){
            self.handle_objc_revert_history_commit(_sender)
        }

        #[unsafe(method(amendHistoryHead:))]
        fn amend_history_head(&self, _sender: &NSMenuItem){
            self.handle_objc_amend_history_head(_sender)
        }

        #[unsafe(method(resetHistoryMixed:))]
        fn reset_history_mixed(&self, _sender: &NSMenuItem){
            self.handle_objc_reset_history_mixed(_sender)
        }

        #[unsafe(method(resetHistoryHard:))]
        fn reset_history_hard(&self, _sender: &NSMenuItem){
            self.handle_objc_reset_history_hard(_sender)
        }

        #[unsafe(method(toggleWorkspacePicker:))]
        fn toggle_workspace_picker(&self, sender: &NSButton){
            self.handle_objc_toggle_workspace_picker(sender)
        }

        #[unsafe(method(filterWorkspaces:))]
        fn filter_workspaces(&self, sender: &NSSearchField){
            self.handle_objc_filter_workspaces(sender)
        }

        #[unsafe(method(activateWorkspaceRow:))]
        fn activate_workspace_row(&self, sender: &NSTableView){
            self.handle_objc_activate_workspace_row(sender)
        }

        #[unsafe(method(activateWorkspaceOption:))]
        fn activate_workspace_option(&self, sender: &NSButton){
            self.handle_objc_activate_workspace_option(sender)
        }

        #[unsafe(method(addWorkspace:))]
        fn add_workspace(&self, _sender: &NSButton){
            self.handle_objc_add_workspace(_sender)
        }

        #[unsafe(method(workspaceCreateNameChanged:))]
        fn workspace_create_name_changed(&self, sender: &NSTextField){
            self.handle_objc_workspace_create_name_changed(sender)
        }

        #[unsafe(method(workspaceCreateRemoteChanged:))]
        fn workspace_create_remote_changed(&self, sender: &NSTextField){
            self.handle_objc_workspace_create_remote_changed(sender)
        }

        #[unsafe(method(submitWorkspaceCreation:))]
        fn submit_workspace_creation_action(&self, _sender: &NSButton){
            self.handle_objc_submit_workspace_creation_action(_sender)
        }

        #[unsafe(method(openWorkspace:))]
        fn open_workspace(&self, _sender: &NSMenuItem){
            self.handle_objc_open_workspace(_sender)
        }

        #[unsafe(method(refreshWorkspace:))]
        fn refresh_workspace(&self, _sender: &NSMenuItem){
            self.handle_objc_refresh_workspace(_sender)
        }

        #[unsafe(method(refreshPage:))]
        fn refresh_page(&self, _sender: &NSMenuItem){
            self.handle_objc_refresh_page(_sender)
        }

        #[unsafe(method(newWindow:))]
        fn new_window(&self, _sender: &NSMenuItem){
            self.handle_objc_new_window(_sender)
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: &NSMenuItem){
            self.handle_objc_show_settings(_sender)
        }

        #[unsafe(method(workspaceUseGlobalChanged:))]
        fn workspace_use_global_changed(&self, _sender: &NSButton){
            self.handle_objc_workspace_use_global_changed(_sender)
        }

        #[unsafe(method(saveWorkspaceSettings:))]
        fn save_workspace_settings(&self, _sender: &NSButton){
            self.handle_objc_save_workspace_settings(_sender)
        }

        #[unsafe(method(saveFontSizes:))]
        fn save_font_sizes(&self, _sender: &NSButton){
            self.handle_objc_save_font_sizes(_sender)
        }

        #[unsafe(method(increaseFontSize:))]
        fn increase_font_size(&self, _sender: &NSMenuItem){
            self.handle_objc_increase_font_size(_sender)
        }

        #[unsafe(method(decreaseFontSize:))]
        fn decrease_font_size(&self, _sender: &NSMenuItem){
            self.handle_objc_decrease_font_size(_sender)
        }

        #[unsafe(method(resetFontSize:))]
        fn reset_font_size(&self, _sender: &NSMenuItem){
            self.handle_objc_reset_font_size(_sender)
        }

        #[unsafe(method(showKeyboardShortcuts:))]
        fn show_keyboard_shortcuts(&self, _sender: &NSMenuItem){
            self.handle_objc_show_keyboard_shortcuts(_sender)
        }

        #[unsafe(method(openCraicWebsite:))]
        fn open_craic_website(&self, _sender: &NSMenuItem){
            self.handle_objc_open_craic_website(_sender)
        }

        #[unsafe(method(reportCraicIssue:))]
        fn report_craic_issue(&self, _sender: &NSMenuItem){
            self.handle_objc_report_craic_issue(_sender)
        }

        #[unsafe(method(pullRemote:))]
        fn pull_remote(&self, _sender: &NSMenuItem){
            self.handle_objc_pull_remote(_sender)
        }

        #[unsafe(method(pushRemote:))]
        fn push_remote(&self, _sender: &NSMenuItem){
            self.handle_objc_push_remote(_sender)
        }

        #[unsafe(method(commitMessageProviderChanged:))]
        fn commit_message_provider_changed(&self, sender: &NSPopUpButton){
            self.handle_objc_commit_message_provider_changed(sender)
        }

        #[unsafe(method(commitMessageModelChanged:))]
        fn commit_message_model_changed(&self, sender: &NSPopUpButton){
            self.handle_objc_commit_message_model_changed(sender)
        }

        #[unsafe(method(toggleBranchPicker:))]
        fn toggle_branch_picker(&self, sender: &NSButton){
            self.handle_objc_toggle_branch_picker(sender)
        }

        #[unsafe(method(filterBranches:))]
        fn filter_branches(&self, sender: &NSSearchField){
            self.handle_objc_filter_branches(sender)
        }

        #[unsafe(method(activateBranchRow:))]
        fn activate_branch_row(&self, sender: &NSButton){
            self.handle_objc_activate_branch_row(sender)
        }

        #[unsafe(method(addBranch:))]
        fn add_branch(&self, _sender: &NSButton){
            self.handle_objc_add_branch(_sender)
        }

        #[unsafe(method(toggleMergeBranch:))]
        fn toggle_merge_branch(&self, _sender: &NSButton){
            self.handle_objc_toggle_merge_branch(_sender)
        }

        #[unsafe(method(activateChangedFile:))]
        fn activate_changed_file(&self, sender: &NSButton){
            self.handle_objc_activate_changed_file(sender)
        }

        #[unsafe(method(toggleChangedFile:))]
        fn toggle_changed_file(&self, sender: &NSButton){
            self.handle_objc_toggle_changed_file(sender)
        }

        #[unsafe(method(toggleAllChangedFiles:))]
        fn toggle_all_changed_files(&self, sender: &NSButton){
            self.handle_objc_toggle_all_changed_files(sender)
        }

        #[unsafe(method(selectAllChangedFilesFromMenu:))]
        fn select_all_changed_files_from_menu(&self, _sender: &NSMenuItem){
            self.handle_objc_select_all_changed_files_from_menu(_sender)
        }

        #[unsafe(method(deselectAllChangedFilesFromMenu:))]
        fn deselect_all_changed_files_from_menu(&self, _sender: &NSMenuItem){
            self.handle_objc_deselect_all_changed_files_from_menu(_sender)
        }

        #[unsafe(method(confirmDiscardAllChanges:))]
        fn confirm_discard_all_changes(&self, _sender: &NSMenuItem){
            self.handle_objc_confirm_discard_all_changes(_sender)
        }

        #[unsafe(method(openChangedFile:))]
        fn open_changed_file(&self, sender: &NSMenuItem){
            self.handle_objc_open_changed_file(sender)
        }

        #[unsafe(method(openChangedFileInCode:))]
        fn open_changed_file_in_code(&self, sender: &NSMenuItem){
            self.handle_objc_open_changed_file_in_code(sender)
        }

        #[unsafe(method(revealChangedFile:))]
        fn reveal_changed_file(&self, sender: &NSMenuItem){
            self.handle_objc_reveal_changed_file(sender)
        }

        #[unsafe(method(showChangedFileInFiles:))]
        fn show_changed_file_in_files(&self, sender: &NSMenuItem){
            self.handle_objc_show_changed_file_in_files(sender)
        }

        #[unsafe(method(addChangedIgnorePattern:))]
        fn add_changed_ignore_pattern(&self, sender: &NSMenuItem){
            self.handle_objc_add_changed_ignore_pattern(sender)
        }

        #[unsafe(method(stashAllChanges:))]
        fn stash_all_changes(&self, _sender: &NSMenuItem){
            self.handle_objc_stash_all_changes(_sender)
        }

        #[unsafe(method(copyChangedRelativePath:))]
        fn copy_changed_relative_path(&self, sender: &NSMenuItem){
            self.handle_objc_copy_changed_relative_path(sender)
        }

        #[unsafe(method(copyChangedAbsolutePath:))]
        fn copy_changed_absolute_path(&self, sender: &NSMenuItem){
            self.handle_objc_copy_changed_absolute_path(sender)
        }

        #[unsafe(method(confirmDiscardChangedFile:))]
        fn confirm_discard_changed_file(&self, sender: &NSMenuItem){
            self.handle_objc_confirm_discard_changed_file(sender)
        }

        #[unsafe(method(selectCommitAuthor:))]
        fn select_commit_author(&self, sender: &NSButton){
            self.handle_objc_select_commit_author(sender)
        }

        #[unsafe(method(showCommitAuthorWarning:))]
        fn show_commit_author_warning(&self, sender: &NSButton){
            self.handle_objc_show_commit_author_warning(sender)
        }

        #[unsafe(method(selectCommitAuthorOption:))]
        fn select_commit_author_option(&self, sender: &NSButton){
            self.handle_objc_select_commit_author_option(sender)
        }

        #[unsafe(method(commitSummaryChanged:))]
        fn commit_summary_changed(&self, _sender: &NSTextField){
            self.handle_objc_commit_summary_changed(_sender)
        }

        #[unsafe(method(generateCommitMessage:))]
        fn generate_commit_message(&self, _sender: &NSButton){
            self.handle_objc_generate_commit_message(_sender)
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent){
            self.handle_objc_mouse_entered(_event)
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent){
            self.handle_objc_mouse_exited(_event)
        }

        #[unsafe(method(commitChanges:))]
        fn commit_changes(&self, _sender: &NSButton){
            self.handle_objc_commit_changes(_sender)
        }

        #[unsafe(method(fetchRemote:))]
        fn fetch_remote(&self, _sender: &NSButton){
            self.handle_objc_fetch_remote(_sender)
        }

        #[unsafe(method(openRepositorySuggestionInEditor:))]
        fn open_repository_suggestion_in_editor(&self, _sender: &NSButton){
            self.handle_objc_open_repository_suggestion_in_editor(_sender)
        }

        #[unsafe(method(openRepositorySuggestionInGhostty:))]
        fn open_repository_suggestion_in_ghostty(&self, _sender: &NSButton){
            self.handle_objc_open_repository_suggestion_in_ghostty(_sender)
        }

        #[unsafe(method(showRepositorySuggestionInFinder:))]
        fn show_repository_suggestion_in_finder(&self, _sender: &NSButton){
            self.handle_objc_show_repository_suggestion_in_finder(_sender)
        }

        #[unsafe(method(openRepositorySuggestionRemote:))]
        fn open_repository_suggestion_remote(&self, _sender: &NSButton){
            self.handle_objc_open_repository_suggestion_remote(_sender)
        }

        #[unsafe(method(initializeRepositorySuggestion:))]
        fn initialize_repository_suggestion(&self, _sender: &NSButton){
            self.handle_objc_initialize_repository_suggestion(_sender)
        }

        #[unsafe(method(toggleTerminal:))]
        fn toggle_terminal(&self, sender: &NSToolbarItem){
            self.handle_objc_toggle_terminal(sender)
        }

        #[unsafe(method(newTerminalSession:))]
        fn new_terminal_session(&self, _sender: &NSButton){
            self.handle_objc_new_terminal_session(_sender)
        }

        #[unsafe(method(closeTerminalSession:))]
        fn close_terminal_session(&self, sender: &NSButton){
            self.handle_objc_close_terminal_session(sender)
        }

        #[unsafe(method(selectTerminalSession:))]
        fn select_terminal_session(&self, sender: &NSButton){
            self.handle_objc_select_terminal_session(sender)
        }

        #[unsafe(method(copyTerminalWorkingDirectory:))]
        fn copy_terminal_working_directory(&self, sender: &NSMenuItem){
            self.handle_objc_copy_terminal_working_directory(sender)
        }

        #[unsafe(method(revealTerminalWorkingDirectory:))]
        fn reveal_terminal_working_directory(&self, sender: &NSMenuItem){
            self.handle_objc_reveal_terminal_working_directory(sender)
        }

        #[unsafe(method(moveTerminalSessionLeft:))]
        fn move_terminal_session_left(&self, sender: &NSMenuItem){
            self.handle_objc_move_terminal_session_left(sender)
        }

        #[unsafe(method(moveTerminalSessionRight:))]
        fn move_terminal_session_right(&self, sender: &NSMenuItem){
            self.handle_objc_move_terminal_session_right(sender)
        }

        #[unsafe(method(closeTerminalSessionFromMenu:))]
        fn close_terminal_session_from_menu(&self, sender: &NSMenuItem){
            self.handle_objc_close_terminal_session_from_menu(sender)
        }

        #[unsafe(method(filterTerminal:))]
        fn filter_terminal(&self, _sender: &NSSearchField){
            self.handle_objc_filter_terminal(_sender)
        }

        #[unsafe(method(previousTerminalMatch:))]
        fn previous_terminal_match(&self, _sender: &NSButton){
            self.handle_objc_previous_terminal_match(_sender)
        }

        #[unsafe(method(nextTerminalMatch:))]
        fn next_terminal_match(&self, _sender: &NSButton){
            self.handle_objc_next_terminal_match(_sender)
        }

        #[unsafe(method(toggleTerminalSearchOption:))]
        fn toggle_terminal_search_option(&self, _sender: &NSButton){
            self.handle_objc_toggle_terminal_search_option(_sender)
        }

        #[unsafe(method(closeTerminalSearch:))]
        fn close_terminal_search(&self, _sender: &NSButton){
            self.handle_objc_close_terminal_search(_sender)
        }

        #[unsafe(method(autoCloseTerminalSession:))]
        fn auto_close_terminal_session(&self, timer: &NSTimer){
            self.handle_objc_auto_close_terminal_session(timer)
        }

        #[unsafe(method(refreshAgentTerminalUsage:))]
        fn refresh_agent_terminal_usage(&self, _timer: &NSTimer){
            self.handle_objc_refresh_agent_terminal_usage(_timer)
        }

        #[unsafe(method(hideNativeToast:))]
        fn hide_native_toast(&self, _timer: &NSTimer){
            self.handle_objc_hide_native_toast(_timer)
        }

        #[unsafe(method(runQuickAction:))]
        fn run_quick_action(&self, sender: &NSToolbarItem){
            self.handle_objc_run_quick_action(sender)
        }

        #[unsafe(method(selectQuickAction:))]
        fn select_quick_action(&self, sender: &NSMenuItem){
            self.handle_objc_select_quick_action(sender)
        }

        #[unsafe(method(addQuickAction:))]
        fn add_quick_action(&self, _sender: &NSToolbarItem){
            self.handle_objc_add_quick_action(_sender)
        }

        #[unsafe(method(removeQuickAction:))]
        fn remove_quick_action(&self, sender: &NSMenuItem){
            self.handle_objc_remove_quick_action(sender)
        }

        #[unsafe(method(refreshQuickActions:))]
        fn refresh_quick_actions(&self, _sender: &NSMenuItem){
            self.handle_objc_refresh_quick_actions(_sender)
        }

        #[unsafe(method(runQuickCommand:))]
        fn run_quick_command(&self, _sender: &NSMenuItem){
            self.handle_objc_run_quick_command(_sender)
        }

    }

    // SAFETY: Method signatures match NSToolbarDelegate and all returned objects are owned.
    unsafe impl NSToolbarDelegate for AppDelegate {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn toolbar_default_item_identifiers(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>>{
            self.handle_objc_delegate_toolbar_default_item_identifiers(_toolbar)
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn toolbar_allowed_item_identifiers(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>>{
            self.handle_objc_delegate_toolbar_allowed_item_identifiers(_toolbar)
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            will_be_inserted: bool,
        ) -> Option<Retained<NSToolbarItem>>{
            self.handle_objc_delegate_toolbar_item(_toolbar, identifier, will_be_inserted)
        }
    }
);

include!("application/core.rs");
include!("application/quick_actions.rs");
include!("application/terminal.rs");
include!("application/agent_session_ui.rs");
include!("application/agent_render.rs");
include!("application/layout.rs");
include!("application/settings.rs");
include!("application/workspace_picker.rs");
include!("application/containers.rs");
include!("application/file_actions.rs");
include!("application/file_preview.rs");
include!("application/file_tables.rs");
include!("application/commit_author.rs");
include!("application/branches.rs");
include!("application/changes.rs");
include!("application/history.rs");
include!("application/repository_state.rs");
include!("application/workspace_create.rs");
include!("application/completions.rs");
include!("application/toolbar.rs");
include!("application/history_builder.rs");
include!("application/files_builder.rs");
include!("application/containers_builder.rs");
include!("application/agents_builder.rs");
include!("application/lifecycle.rs");
include!("application/events.rs");
include!("application/objc_actions_core.rs");
include!("application/objc_actions_agents.rs");
include!("application/objc_actions_files.rs");
include!("application/objc_actions_history.rs");
include!("application/objc_actions_changes.rs");
include!("application/objc_actions_terminal.rs");
include!("application/objc_delegate_window.rs");
include!("application/objc_delegate_text.rs");
include!("application/objc_delegate_tables.rs");
include!("application/objc_delegate_toolbar.rs");
include!("application/repository_service.rs");
include!("application/repository_core.rs");
include!("application/repository_history.rs");
include!("application/repository_files.rs");
include!("application/repository_containers.rs");
include!("application/repository_media.rs");
include!("application/repository_commit_settings.rs");
include!("application/repository_changes.rs");

include!("application/helpers.rs");

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
    let (startup_options, startup_error) = match startup_options() {
        Ok(options) => (options, None),
        Err(error) => {
            log::warn!("native startup workspace argument ignored: {error}");
            (NativeStartupOptions::default(), Some(error))
        }
    };
    let startup_workspace = startup_options.workspace;
    let startup_files_requested = startup_options.open_path.is_some();
    let mtm = MainThreadMarker::new().expect("Craic must start on the macOS main thread");
    let application = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    delegate.ivars().startup_error.replace(startup_error);
    delegate
        .ivars()
        .pending_files_path
        .replace(startup_options.open_path);
    delegate
        .ivars()
        .pending_files_line
        .set(startup_options.line);
    delegate
        .ivars()
        .pending_files_column
        .set(startup_options.column);
    let (runtime, mut channels) = ApplicationRuntime::start(RuntimeConfig::default())
        .expect("failed to start the Craic application runtime");
    let app_handle = channels.handle.clone();
    assert!(
        delegate.ivars().app_handle.set(app_handle.clone()).is_ok(),
        "application handle is initialized once"
    );
    assert!(
        delegate.ivars().ui_context.set(UiContextId::new()).is_ok(),
        "frontend UI context is initialized once"
    );

    let main_delegate = Arc::new(MainThreadBound::new(delegate.clone(), mtm));
    let (frontend_completion_tx, frontend_completion_rx) = std::sync::mpsc::channel();
    assert!(
        delegate
            .ivars()
            .frontend_completions
            .set(frontend_completion_tx.clone())
            .is_ok(),
        "frontend completion sender is initialized once"
    );
    let completion_delegate = main_delegate.clone();
    let frontend_bridge = std::thread::Builder::new()
        .name("craic-appkit-completions".to_string())
        .spawn(move || {
            while let Ok(completion) = frontend_completion_rx.recv() {
                if matches!(completion, FrontendCompletion::Shutdown) {
                    break;
                }
                let delegate = completion_delegate.clone();
                if let Err(error) = crate::AppKitDispatcher.schedule(Box::new(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("frontend completion must execute on the main thread");
                    delegate.get(mtm).apply_frontend_completion(completion);
                })) {
                    log::error!("frontend completion dispatch failed error={error}");
                    break;
                }
            }
            log::info!("native frontend completion bridge stopped");
        })
        .expect("failed to start native frontend completion bridge");
    let (terminal_media_tx, terminal_media_rx) = std::sync::mpsc::channel();
    assert!(
        delegate
            .ivars()
            .terminal_media_commands
            .set(terminal_media_tx)
            .is_ok(),
        "native terminal media command sender is initialized once"
    );
    let terminal_media_completion_tx = frontend_completion_tx.clone();
    let terminal_media_worker = std::thread::Builder::new()
        .name("craic-terminal-media".to_string())
        .spawn(move || {
            let mut sessions = HashMap::<isize, NativeTerminalMediaWorkerSession>::new();
            while let Ok(command) = terminal_media_rx.recv() {
                match command {
                    NativeTerminalMediaCommand::Upload {
                        session_id,
                        context,
                        sources,
                    } => {
                        let workspace_id = context.workspace_id.clone();
                        let result = upload_native_terminal_remote_images(&context, sources).map(
                            |uploaded| {
                                let paths = uploaded
                                    .iter()
                                    .map(|media| media.path.clone())
                                    .collect::<Vec<_>>();
                                sessions
                                    .entry(session_id)
                                    .or_insert_with(|| NativeTerminalMediaWorkerSession {
                                        shell: context.shell.clone(),
                                        working_dir: context.working_dir.clone(),
                                        uploaded: Vec::new(),
                                    })
                                    .uploaded
                                    .extend(uploaded);
                                paths
                            },
                        );
                        if terminal_media_completion_tx
                            .send(FrontendCompletion::TerminalRemoteImages {
                                workspace_id,
                                session_id,
                                result,
                            })
                            .is_err()
                        {
                            log::warn!(
                                "remote terminal image completion dropped during shutdown session={session_id}"
                            );
                        }
                    }
                    NativeTerminalMediaCommand::Close { session_id } => {
                        if let Some(session) = sessions.remove(&session_id) {
                            remote_media::remove_bounded(
                                session.shell,
                                session.working_dir,
                                session.uploaded,
                            );
                        }
                    }
                    NativeTerminalMediaCommand::Shutdown => {
                        for (_, session) in sessions.drain() {
                            remote_media::remove_bounded(
                                session.shell,
                                session.working_dir,
                                session.uploaded,
                            );
                        }
                        break;
                    }
                }
            }
            log::info!("native terminal media worker stopped");
        })
        .expect("failed to start native terminal media worker");
    let (agent_command_tx, agent_command_rx) = tokio::sync::mpsc::channel(64);
    assert!(
        delegate
            .ivars()
            .agent_commands
            .set(agent_command_tx.clone())
            .is_ok(),
        "native agent command sender is initialized once"
    );
    let agent_completion_tx = frontend_completion_tx.clone();
    runtime
        .spawn(crate::agent_session::run(agent_command_rx, move |event| {
            if agent_completion_tx
                .send(FrontendCompletion::Agent(event))
                .is_err()
            {
                log::warn!("native Codex completion dropped during shutdown");
            }
        }))
        .expect("application runtime must accept the native agent service");
    let (frontend_request_tx, mut frontend_request_rx) = tokio::sync::mpsc::channel(8);
    assert!(
        delegate
            .ivars()
            .frontend_requests
            .set(frontend_request_tx)
            .is_ok(),
        "frontend request sender is initialized once"
    );
    let frontend_effect_client = runtime.ui_effect_client();
    let frontend_effect_context = delegate
        .ivars()
        .ui_context
        .get()
        .expect("frontend UI context is initialized")
        .clone();
    let frontend_task_completion_tx = frontend_completion_tx.clone();
    let frontend_task_cancellation = runtime.cancellation_token();
    runtime
        .spawn(async move {
            loop {
                let request = tokio::select! {
                    _ = frontend_task_cancellation.cancelled() => break,
                    request = frontend_request_rx.recv() => request,
                };
                let Some(request) = request else {
                    break;
                };
                match request {
                    FrontendRequest::OpenWorkspace => {
                        let result = tokio::select! {
                            _ = frontend_task_cancellation.cancelled() => break,
                            result = frontend_effect_client.request(
                                frontend_effect_context.clone(),
                                UiEffect::ChoosePath(PathPickerRequest {
                                    mode: PathPickerMode::OpenDirectory,
                                    title: "Open Workspace".to_string(),
                                    initial_path: None,
                                    allowed_extensions: Vec::new(),
                                    allow_multiple: false,
                                }),
                            ) => result,
                        };
                        if frontend_task_completion_tx
                            .send(FrontendCompletion::OpenWorkspace(result))
                            .is_err()
                        {
                            break;
                        }
                    }
                    FrontendRequest::CreateWorkspace {
                        request_id,
                        request,
                    } => {
                        let creation =
                            tokio::task::spawn_blocking(move || create_native_workspace(request));
                        let result = creation.await.unwrap_or_else(|error| {
                            Err(format!("Workspace creation did not complete: {error}"))
                        });
                        if frontend_task_completion_tx
                            .send(FrontendCompletion::WorkspaceCreated { request_id, result })
                            .is_err()
                        {
                            break;
                        }
                    }
                    FrontendRequest::ConfirmDiscard {
                        paths,
                        heading,
                        message,
                    } => {
                        let result = tokio::select! {
                            _ = frontend_task_cancellation.cancelled() => break,
                            result = frontend_effect_client.request(
                                frontend_effect_context.clone(),
                                UiEffect::Confirm(ConfirmRequest {
                                    heading,
                                    message,
                                    confirm_label: "Discard Changes".to_string(),
                                    cancel_label: "Cancel".to_string(),
                                    destructive: true,
                                }),
                            ) => result,
                        };
                        if frontend_task_completion_tx
                            .send(FrontendCompletion::ConfirmDiscard { paths, result })
                            .is_err()
                        {
                            break;
                        }
                    }
                    FrontendRequest::SaveLastWorkspace(workspace) => {
                        if let Err(error) = tokio::task::spawn_blocking(move || {
                            craic_config::save_last_workspace(&workspace)
                        })
                        .await
                        {
                            log::warn!("last-workspace save task did not complete: {error}");
                        }
                    }
                }
            }
            log::info!("native frontend request service stopped");
        })
        .expect("application runtime must accept the native frontend request service");
    let (workspace_metadata_tx, mut workspace_metadata_rx) = tokio::sync::mpsc::channel(4);
    assert!(
        delegate
            .ivars()
            .workspace_metadata_requests
            .set(workspace_metadata_tx)
            .is_ok(),
        "workspace metadata request sender is initialized once"
    );
    let workspace_metadata_completion_tx = frontend_completion_tx.clone();
    let workspace_metadata_cancellation = runtime.cancellation_token();
    runtime
        .spawn(async move {
            loop {
                let request = tokio::select! {
                    _ = workspace_metadata_cancellation.cancelled() => break,
                    request = workspace_metadata_rx.recv() => request,
                };
                let Some(request) = request else {
                    break;
                };
                log::debug!(
                    "native workspace metadata batch started generation={} count={}",
                    request.generation,
                    request.entries.len()
                );
                for entry in request.entries {
                    let workspace_id = entry.selection_id();
                    let result = tokio::select! {
                        _ = workspace_metadata_cancellation.cancelled() => break,
                        result = load_native_workspace_metadata(entry) => result,
                    };
                    if workspace_metadata_completion_tx
                        .send(FrontendCompletion::WorkspaceMetadata {
                            workspace_id,
                            generation: request.generation,
                            result,
                        })
                        .is_err()
                    {
                        log::warn!("workspace metadata completion dropped during shutdown");
                        return;
                    }
                }
            }
            log::info!("native workspace metadata service stopped");
        })
        .expect("application runtime must accept workspace metadata service");

    let (repository_request_tx, repository_request_rx) = tokio::sync::mpsc::channel(16);
    assert!(
        delegate
            .ivars()
            .repository_requests
            .set(repository_request_tx)
            .is_ok(),
        "repository request sender is initialized once"
    );
    let repository_completion_tx = frontend_completion_tx.clone();
    let repository_service_cancellation = runtime.cancellation_token();
    let retired_jobs = runtime.retired_job_sender();
    runtime
        .spawn(run_repository_service(
            repository_request_rx,
            repository_completion_tx,
            repository_service_cancellation,
            retired_jobs,
        ))
        .expect("application runtime must accept repository service");
    let bridge_delegate = main_delegate.clone();
    let bridge = std::thread::Builder::new()
        .name("craic-appkit-events".to_string())
        .spawn(move || {
            let dispatcher = crate::AppKitDispatcher;
            while let Some(event) = channels.events.blocking_recv() {
                let delegate = bridge_delegate.clone();
                if let Err(error) = dispatcher.schedule(Box::new(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("AppKit event dispatch must execute on the main thread");
                    delegate.get(mtm).apply_event(event);
                })) {
                    log::error!("native UI event dispatch failed error={error}");
                    break;
                }
            }
            log::info!("native UI event bridge stopped");
        })
        .expect("failed to start the native UI event bridge");

    let (workspace_discovery_tx, mut workspace_discovery_rx) = tokio::sync::mpsc::channel(1);
    assert!(
        delegate
            .ivars()
            .workspace_discovery_requests
            .set(workspace_discovery_tx)
            .is_ok(),
        "workspace discovery sender is initialized once"
    );
    let discovery_completion_tx = frontend_completion_tx.clone();
    let discovery_cancellation = runtime.cancellation_token();
    runtime
        .spawn(async move {
            loop {
                let request = tokio::select! {
                    _ = discovery_cancellation.cancelled() => break,
                    request = workspace_discovery_rx.recv() => request,
                };
                let Some(request) = request else {
                    break;
                };
                let generation = request.generation;
                let select_workspace = request.select_workspace;
                let discovered = tokio::task::spawn_blocking(move || {
                let mut entries = craic_system::workspace::discover_configured_workspaces();
                let preferred = request.preferred;
                if let Some(workspace) = preferred.as_ref()
                    && !entries
                        .iter()
                        .any(|entry| entry.selection_id() == workspace.selection_id())
                {
                    entries.push(WorkspaceEntry {
                        label: workspace.label(),
                        workspace: workspace.clone(),
                    });
                    entries.sort_by_key(|entry| entry.label.to_lowercase());
                }
                (entries, preferred)
            })
            .await;
                let (entries, preferred) = match discovered {
                    Ok(result) => result,
                    Err(error) => {
                        let message = error.to_string();
                        log::error!(
                            "native workspace discovery task failed generation={generation} error={message}"
                        );
                        if discovery_completion_tx
                            .send(FrontendCompletion::WorkspaceDiscoveryFailed {
                                generation,
                                message: format!(
                                    "Workspace discovery did not complete: {message}"
                                ),
                            })
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                if discovery_completion_tx
                    .send(FrontendCompletion::WorkspaceEntries {
                        generation,
                        entries,
                        preferred,
                        select_workspace,
                    })
                    .is_err()
                {
                    log::warn!("native workspace discovery dropped during shutdown");
                    break;
                }
            }
            log::info!("native workspace discovery service stopped");
        })
        .expect("application runtime must accept workspace discovery service");
    delegate.request_workspace_discovery(
        startup_workspace.or_else(craic_config::last_workspace),
        true,
    );

    application.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let saved_page_index = if startup_files_requested {
        PAGE_DESCRIPTORS
            .iter()
            .position(|descriptor| descriptor.id == "files")
            .unwrap_or(0)
    } else {
        usize::try_from(
            NSUserDefaults::standardUserDefaults()
                .integerForKey(&NSString::from_str(ACTIVE_PAGE_DEFAULT)),
        )
        .ok()
        .filter(|index| *index < PAGE_DESCRIPTORS.len())
        .unwrap_or(0)
    };
    log::debug!(
        "restoring native active page index={} page={}",
        saved_page_index,
        PAGE_DESCRIPTORS[saved_page_index].id
    );
    if let Err(command) = app_handle.try_send(AppCommand::ActivatePage(
        PAGE_DESCRIPTORS[saved_page_index].page_id(),
    )) {
        log::warn!("initial page activation rejected command={command:?}");
    }
    application.run();
    delegate.prepare_for_native_shutdown();
    if let Some(commands) = delegate.ivars().terminal_media_commands.get()
        && let Err(error) = commands.send(NativeTerminalMediaCommand::Shutdown)
    {
        log::debug!("native terminal media worker was already stopped: {error}");
    }
    if terminal_media_worker.join().is_err() {
        log::error!("native terminal media worker panicked during shutdown");
    }

    let (agent_shutdown_tx, agent_shutdown_rx) = std::sync::mpsc::sync_channel(1);
    match agent_command_tx.try_send(NativeAgentCommand::Shutdown {
        completed: Some(agent_shutdown_tx),
    }) {
        Ok(()) => {
            if agent_shutdown_rx
                .recv_timeout(Duration::from_secs(4))
                .is_err()
            {
                log::warn!("native Codex service shutdown handshake timed out");
            }
        }
        Err(error) => log::debug!("native Codex service was already stopped: {error}"),
    }
    runtime.shutdown(Duration::from_secs(5));
    let _ = frontend_completion_tx.send(FrontendCompletion::Shutdown);
    if frontend_bridge.join().is_err() {
        log::error!("native frontend completion bridge panicked during shutdown");
    }
    if bridge.join().is_err() {
        log::error!("native UI event bridge panicked during shutdown");
    }
    drop(main_delegate);
    log::info!("native macOS application stopped");
}
