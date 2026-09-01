impl FileBrowser {
    pub fn set_internal_drag_paths(&self, paths: Vec<FileNodePath>) {
        self.internal_drag_paths.replace(Some(paths.clone()));
        set_shared_drag_clipboard(Some(FileClipboard {
            source_access: self.file_access.borrow().clone(),
            paths,
            operation: TransferOperation::Copy,
        }));
    }

    pub fn clear_internal_drag_paths(self: &Rc<Self>) {
        self.internal_drag_paths.borrow_mut().take();
        set_shared_drag_clipboard(None);
        self.clear_drop_target_folder();
    }

    pub fn handle_external_drop_hover(
        self: &Rc<Self>,
        target: FileNodePath,
        available_actions: gdk::DragAction,
    ) -> gdk::DragAction {
        let Some(operation) = self.drop_operation_for_external_target(&target, available_actions)
        else {
            self.clear_drop_target_folder();
            return gdk::DragAction::empty();
        };

        self.set_drop_target_folder(Some(target));
        operation.drag_action()
    }

    pub fn handle_internal_drop_hover(
        self: &Rc<Self>,
        target: FileNodePath,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> gdk::DragAction {
        let Some(operation) =
            self.drop_operation_for_internal_target(&target, available_actions, modifiers)
        else {
            self.clear_drop_target_folder();
            return gdk::DragAction::empty();
        };

        self.set_drop_target_folder(Some(target));
        operation.drag_action()
    }

    pub fn handle_external_dropped_paths(
        self: &Rc<Self>,
        external_sources: Vec<PathBuf>,
        target: FileNodePath,
        available_actions: gdk::DragAction,
    ) -> bool {
        if self
            .drop_operation_for_external_target(&target, available_actions)
            .is_none()
        {
            self.clear_drop_target_folder();
            if !external_sources.is_empty() {
                self.show_error(
                    "Drop Unavailable",
                    "Dropping local files into this workspace is not available.",
                );
            }
            return false;
        }
        self.clear_drop_target_folder();
        self.transfer_local_paths_to_folder(external_sources, target, false);
        true
    }

    pub fn handle_internal_dropped_paths(
        self: &Rc<Self>,
        target: FileNodePath,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> bool {
        let Some(operation) =
            self.drop_operation_for_internal_target(&target, available_actions, modifiers)
        else {
            self.clear_drop_target_folder();
            return false;
        };
        self.clear_drop_target_folder();

        let internal_paths = self.internal_drag_paths.borrow().clone();
        let Some(mut clipboard) = internal_paths
            .map(|paths| FileClipboard {
                source_access: self.file_access.borrow().clone(),
                paths,
                operation,
            })
            .or_else(shared_drag_clipboard)
        else {
            self.show_error("Drop Unavailable", "No file transfer source was available.");
            return false;
        };
        clipboard.operation = operation;
        self.transfer_workspace_paths_to_folder(clipboard, target, operation, false);
        true
    }

    fn drop_operation_for_external_target(
        &self,
        _target: &FileNodePath,
        available_actions: gdk::DragAction,
    ) -> Option<TransferOperation> {
        TransferOperation::Copy
            .action_allowed(available_actions)
            .then_some(TransferOperation::Copy)
    }

    fn drop_operation_for_internal_target(
        &self,
        _target: &FileNodePath,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> Option<TransferOperation> {
        let source_access = if self.internal_drag_paths.borrow().is_some() {
            self.file_access.borrow().clone()
        } else {
            shared_drag_clipboard()?.source_access
        };
        let operation = if !Arc::ptr_eq(&source_access, &self.file_access.borrow())
            || copy_drag_modifier(modifiers)
        {
            TransferOperation::Copy
        } else {
            TransferOperation::Move
        };
        operation
            .action_allowed(available_actions)
            .then_some(operation)
    }

    fn set_drop_target_folder(self: &Rc<Self>, target: Option<FileNodePath>) {
        if *self.drop_target_folder.borrow() == target {
            return;
        }

        self.drop_target_folder.replace(target.clone());
        self.drop_hover_generation
            .set(self.drop_hover_generation.get().wrapping_add(1));
        self.refresh_browser_row_state();

        if let Some(target) = target {
            self.schedule_drop_auto_expand(target);
        }
    }

    pub fn clear_drop_target_folder(self: &Rc<Self>) {
        self.set_drop_target_folder(None);
    }

    fn schedule_drop_auto_expand(self: &Rc<Self>, target: FileNodePath) {
        if target.is_root()
            || !self.search_query.borrow().is_empty()
            || self.expanded_dirs.borrow().contains(&target)
        {
            return;
        }

        let generation = self.drop_hover_generation.get();
        gtk::glib::timeout_add_local_once(Duration::from_millis(500), {
            let browser = self.clone();

            move || {
                if browser.drop_hover_generation.get() != generation
                    || browser.drop_target_folder.borrow().as_ref() != Some(&target)
                    || !browser.search_query.borrow().is_empty()
                    || browser.expanded_dirs.borrow().contains(&target)
                {
                    return;
                }

                browser.expanded_dirs.borrow_mut().insert(target.clone());
                browser.active_folder.replace(target);
                browser.invalidate_tree_rows_cache();
                browser.rebuild();
            }
        });
    }

    fn transfer_local_paths_to_folder(
        self: &Rc<Self>,
        sources: Vec<PathBuf>,
        target_folder: FileNodePath,
        auto_focus: bool,
    ) {
        let file_access = self.file_access.borrow().clone();
        self.transfer_local_paths_with_access(
            sources,
            target_folder,
            auto_focus,
            file_access,
            true,
        );
    }

    fn transfer_local_paths_with_access(
        self: &Rc<Self>,
        sources: Vec<PathBuf>,
        target_folder: FileNodePath,
        auto_focus: bool,
        file_access: Arc<dyn FileAccess>,
        allow_sudo: bool,
    ) {
        if sources.is_empty() {
            return;
        }
        let operation = TransferOperation::Copy;

        let workspace = self.workspace.borrow().clone();
        let transfer_id = self.next_transfer_id.get();
        self.next_transfer_id
            .set(transfer_id.wrapping_add(1).max(1));
        let cancel_requested = Arc::new(AtomicBool::new(false));
        self.active_transfers.borrow_mut().insert(
            transfer_id,
            ActiveTransfer::new(
                operation,
                sources.len() as u64,
                auto_focus,
                cancel_requested.clone(),
                Some(TransferRetry::Local {
                    sources: sources.clone(),
                    target_folder: target_folder.clone(),
                    allow_sudo,
                }),
            ),
        );
        self.refresh_transfer_progress_rows();

        let dispatcher = TransferUiDispatcher::new(self, transfer_id, operation);
        thread::spawn(move || {
            log::info!(
                "local file drop transfer start destination_workspace={} operation={operation:?} count={}",
                workspace.display_name,
                sources.len()
            );
            let mut progress_sender = TransferProgressSender::new(dispatcher.clone());
            let result = transfer_local_paths(
                file_access,
                sources,
                target_folder,
                cancel_requested,
                move |progress| {
                    progress_sender.send(progress);
                },
            );
            dispatcher.send(TransferEvent::Finished(result));
        });
    }

    pub fn active_transfer_rows(&self) -> Vec<rows::TransferRow> {
        self.active_transfers
            .borrow()
            .values()
            .filter_map(|transfer| {
                let path = transfer.current_path.clone()?;
                let file_name = path.file_name().unwrap_or("item");
                Some(rows::TransferRow {
                    name: format!("{} {file_name}", transfer.operation.present_participle()),
                    depth: file_row_depth(&path),
                    path,
                })
            })
            .collect()
    }

    pub fn current_drop_target_folder(&self) -> Option<FileNodePath> {
        self.drop_target_folder.borrow().clone()
    }

    fn workspace_is_directory(&self, path: &FileNodePath) -> bool {
        self.file_access
            .borrow()
            .info(path)
            .is_ok_and(|info| info.kind == FileKind::Directory)
    }

    fn transfer_workspace_paths_to_folder(
        self: &Rc<Self>,
        clipboard: FileClipboard,
        target_folder: FileNodePath,
        operation: TransferOperation,
        auto_focus: bool,
    ) {
        let file_access = self.file_access.borrow().clone();
        self.transfer_workspace_paths_with_access(
            clipboard,
            target_folder,
            operation,
            auto_focus,
            file_access,
            true,
            false,
        );
    }

    fn transfer_workspace_paths_with_access(
        self: &Rc<Self>,
        clipboard: FileClipboard,
        target_folder: FileNodePath,
        operation: TransferOperation,
        auto_focus: bool,
        destination_access: Arc<dyn FileAccess>,
        allow_sudo: bool,
        sudo_destination: bool,
    ) {
        if clipboard.paths.is_empty() {
            return;
        }

        let workspace = self.workspace.borrow().clone();
        let transfer_id = self.next_transfer_id.get();
        self.next_transfer_id
            .set(transfer_id.wrapping_add(1).max(1));
        let cancel_requested = Arc::new(AtomicBool::new(false));
        self.active_transfers.borrow_mut().insert(
            transfer_id,
            ActiveTransfer::new(
                operation,
                clipboard.paths.len() as u64,
                auto_focus,
                cancel_requested.clone(),
                Some(TransferRetry::Workspace(WorkspaceTransferRetry {
                    clipboard: clipboard.clone(),
                    target_folder: target_folder.clone(),
                    allow_sudo,
                    destination_access: destination_access.clone(),
                    sudo_destination,
                })),
            ),
        );
        self.refresh_transfer_progress_rows();

        let dispatcher = TransferUiDispatcher::new(self, transfer_id, operation);
        thread::spawn(move || {
            log::info!(
                "file transfer start destination_workspace={} operation={operation:?} count={}",
                workspace.display_name,
                clipboard.paths.len()
            );
            let mut progress_sender = TransferProgressSender::new(dispatcher.clone());
            let result = transfer_workspace_paths(
                clipboard.source_access,
                destination_access,
                clipboard.paths,
                target_folder,
                operation,
                cancel_requested,
                move |progress| {
                    progress_sender.send(progress);
                },
            );
            dispatcher.send(TransferEvent::Finished(result));
        });
    }

    fn set_transfer_progress(&self, transfer_id: u64, progress: TransferProgressUpdate) -> bool {
        if let Some(active) = self.active_transfers.borrow_mut().get_mut(&transfer_id) {
            let current_path_changed = active.current_path != progress.current_path;
            active.current_path = progress.current_path;
            active.copied_bytes = if current_path_changed {
                progress.copied_bytes
            } else {
                active.copied_bytes.max(progress.copied_bytes)
            };
            active.total_bytes = progress.total_bytes;
            active.copied_files = if current_path_changed {
                progress.copied_files
            } else {
                active.copied_files.max(progress.copied_files)
            };
            active.total_files = progress.total_files;
            return current_path_changed;
        }
        false
    }

    fn finish_transfer(
        self: &Rc<Self>,
        transfer_id: u64,
        operation: TransferOperation,
        result: Result<Vec<FileNodePath>, String>,
    ) {
        let selected_path = self.selected_node_path.borrow().clone();
        let active = self.active_transfers.borrow_mut().remove(&transfer_id);
        let auto_focus = active.as_ref().is_some_and(|active| active.auto_focus);
        let retry = active.as_ref().and_then(|active| active.retry.clone());
        let selected_active_path = selected_path.clone().filter(|path| {
            active
                .as_ref()
                .and_then(|active| active.current_path.as_ref())
                == Some(path)
        });
        self.refresh_transfer_progress_rows();

        match result {
            Ok(destinations) => {
                let selected_destination = selected_path
                    .clone()
                    .filter(|path| destinations.iter().any(|destination| destination == path));
                if operation == TransferOperation::Move {
                    self.file_clipboard.borrow_mut().take();
                }
                self.invalidate_tree_rows_cache();
                self.rebuild_if_changed();
                if auto_focus {
                    self.auto_focus_transferred_items(destinations);
                } else if let Some(selected) = selected_destination.or(selected_active_path) {
                    self.emit_selected_node_path(selected);
                }
            }
            Err(message) => {
                self.invalidate_tree_rows_cache();
                self.rebuild_if_changed();
                if message == TRANSFER_CANCELED_MESSAGE {
                    if selected_active_path.is_some() {
                        self.set_selected_node_path(None);
                    }
                    log::info!("file transfer canceled id={transfer_id}");
                } else {
                    if let Some(selected) = selected_active_path {
                        self.emit_selected_node_path(selected);
                    }
                    if craic_system::system::is_permission_denied_message(&message)
                        && retry.as_ref().is_some_and(TransferRetry::allow_sudo)
                    {
                        let retry = retry.expect("checked above");
                        if let TransferRetry::Workspace(source_retry) = &retry
                            && source_retry.sudo_destination
                        {
                            let source_retry = source_retry.clone();
                            let retry_browser = self.clone();
                            let error_browser = self.clone();
                            crate::ui::sudo::offer_retry(
                                self.root.clone().upcast(),
                                source_retry.clipboard.source_access.clone(),
                                operation.failure_heading(),
                                &message,
                                Rc::new(move |sudo_source| {
                                    let mut clipboard = source_retry.clipboard.clone();
                                    clipboard.source_access = sudo_source;
                                    retry_browser.transfer_workspace_paths_with_access(
                                        clipboard,
                                        source_retry.target_folder.clone(),
                                        operation,
                                        auto_focus,
                                        source_retry.destination_access.clone(),
                                        false,
                                        true,
                                    );
                                }),
                                Rc::new(move |message| {
                                    error_browser.show_error(operation.failure_heading(), &message)
                                }),
                            );
                            return;
                        }
                        let retry_browser = self.clone();
                        let prompt_browser = self.clone();
                        let base_access = self.file_access.borrow().clone();
                        super::file_ops::offer_sudo_retry(
                            prompt_browser,
                            operation.failure_heading(),
                            &message,
                            Rc::new(move |sudo_access| match &retry {
                                TransferRetry::Local {
                                    sources,
                                    target_folder,
                                    ..
                                } => retry_browser.transfer_local_paths_with_access(
                                    sources.clone(),
                                    target_folder.clone(),
                                    auto_focus,
                                    sudo_access,
                                    false,
                                ),
                                TransferRetry::Workspace(retry) => {
                                    let mut clipboard = retry.clipboard.clone();
                                    if Arc::ptr_eq(&clipboard.source_access, &base_access) {
                                        clipboard.source_access = sudo_access.clone();
                                    }
                                    retry_browser.transfer_workspace_paths_with_access(
                                        clipboard,
                                        retry.target_folder.clone(),
                                        operation,
                                        auto_focus,
                                        sudo_access,
                                        !Arc::ptr_eq(&retry.clipboard.source_access, &base_access),
                                        true,
                                    );
                                }
                            }),
                        );
                    } else {
                        self.show_error(operation.failure_heading(), &message);
                    }
                }
            }
        }
    }

    fn refresh_transfer_progress_rows(self: &Rc<Self>) {
        let rows = self.list_rows.borrow().clone();
        self.set_browser_rows(rows);
    }

    fn auto_focus_transferred_items(self: &Rc<Self>, destinations: Vec<FileNodePath>) {
        let Some(selected) = destinations.into_iter().find(|path| !path.is_root()) else {
            return;
        };
        self.set_selected_node_path(Some(selected.clone()));
        self.scroll_selected_row_into_view();
        self.focus_selected_row();
        log::info!(
            "file transfer auto-focused item path={}",
            selected.display()
        );
    }

    pub fn confirm_cancel_transfers(self: &Rc<Self>, transfer_ids: Vec<u64>) {
        let transfer_ids = transfer_ids
            .into_iter()
            .filter(|id| self.active_transfers.borrow().contains_key(id))
            .collect::<Vec<_>>();
        if transfer_ids.is_empty() {
            return;
        }

        let dialog = adw::AlertDialog::builder()
            .heading("Cancel Transfer?")
            .body("Stop copying the current item?")
            .build();
        dialog.add_response("keep", "Keep Copying");
        dialog.add_response("cancel", "Cancel Transfer");
        dialog.set_default_response(Some("keep"));
        dialog.set_close_response("keep");
        dialog.set_response_appearance("cancel", adw::ResponseAppearance::Destructive);
        dialog.choose(Some(&self.root), None::<&gtk::gio::Cancellable>, {
            let browser = self.clone();

            move |response| {
                if response.as_str() == "cancel" {
                    browser.cancel_transfers(&transfer_ids);
                }
            }
        });
    }

    fn cancel_transfers(self: &Rc<Self>, transfer_ids: &[u64]) {
        let mut selected_was_canceled = false;
        let selected_path = self.selected_node_path.borrow().clone();
        for transfer_id in transfer_ids {
            if let Some(transfer) = self.active_transfers.borrow_mut().remove(transfer_id) {
                transfer.cancel_requested.store(true, Ordering::Relaxed);
                selected_was_canceled |= selected_path
                    .as_ref()
                    .is_some_and(|path| transfer.current_path.as_ref() == Some(path));
                log::info!("file transfer cancel requested id={transfer_id}");
            }
        }
        if selected_was_canceled {
            self.set_selected_node_path(None);
        } else {
            self.refresh_transfer_progress_rows();
        }
    }

    pub fn cancel_transfers_for_workspace_change(self: &Rc<Self>) {
        let transfer_ids = self
            .active_transfers
            .borrow()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        if transfer_ids.is_empty() {
            return;
        }
        self.cancel_transfers(&transfer_ids);
        log::info!(
            "file transfers canceled for workspace change count={}",
            transfer_ids.len()
        );
    }

    pub fn transfer_progress_for_path(&self, path: &FileNodePath) -> Option<TransferRowProgress> {
        let transfers = self.active_transfers.borrow();
        let mut count = 0usize;
        let mut copied_bytes = 0u64;
        let mut total_bytes = 0u64;
        let mut copied_files = 0u64;
        let mut total_files = 0u64;
        let mut operation = None;
        let mut transfer_ids = Vec::new();

        for (transfer_id, transfer) in transfers.iter() {
            if transfer.current_path.as_ref() != Some(path) {
                continue;
            }

            count += 1;
            transfer_ids.push(*transfer_id);
            copied_bytes = copied_bytes.saturating_add(transfer.copied_bytes);
            total_bytes = total_bytes.saturating_add(transfer.total_bytes);
            copied_files = copied_files.saturating_add(transfer.copied_files);
            total_files = total_files.saturating_add(transfer.total_files);
            operation.get_or_insert(transfer.operation);
        }

        if count == 0 {
            return None;
        }

        let fraction = if total_bytes > 0 {
            copied_bytes as f64 / total_bytes as f64
        } else if total_files > 0 {
            copied_files as f64 / total_files as f64
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let label = format!("{:.0}%", fraction * 100.0);
        let action = if count == 1 {
            operation
                .map(TransferOperation::present_participle)
                .unwrap_or("Transferring")
                .to_string()
        } else {
            format!("Transferring {count} batches")
        };

        Some(TransferRowProgress {
            fraction,
            transfer_ids,
            tooltip: format!("{action}: {label}"),
        })
    }

    pub fn path_has_active_transfer(&self, path: &FileNodePath) -> bool {
        self.active_transfers
            .borrow()
            .values()
            .any(|transfer| transfer.current_path.as_ref() == Some(path))
    }

    pub fn paste_target_folder(self: &Rc<Self>) -> FileNodePath {
        let Some(selected) = self.selected_node_path.borrow().clone() else {
            return self.active_folder.borrow().clone();
        };

        if self.workspace_is_directory(&selected) {
            selected
        } else {
            selected.parent().unwrap_or_else(|| self.root_node_path())
        }
    }

    pub fn target_paste_folder(&self, target: &BrowserTarget) -> FileNodePath {
        if target.is_dir {
            target.node_path.clone()
        } else {
            target
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path())
        }
    }

    pub fn paste_clipboard_files(self: &Rc<Self>) {
        self.paste_into_folder(self.paste_target_folder());
    }

    pub fn paste_into_folder(self: &Rc<Self>, target_folder: FileNodePath) {
        let Some(clipboard) = self.file_clipboard.borrow().clone() else {
            let Some(clipboard) = shared_file_clipboard() else {
                return;
            };
            self.transfer_workspace_paths_to_folder(
                clipboard.clone(),
                target_folder,
                clipboard.operation,
                true,
            );
            return;
        };
        self.transfer_workspace_paths_to_folder(
            clipboard.clone(),
            target_folder,
            clipboard.operation,
            true,
        );
    }

    pub fn open_target(self: &Rc<Self>, target: &BrowserTarget) {
        if target.is_dir || target.capabilities.listable {
            if !target.node_path.is_root() {
                self.toggle_dir(&target.node_path);
            } else {
                let parent_window = self.root.root().and_downcast::<gtk::Window>();
                self.open_external(target, parent_window);
            }
        } else {
            self.set_selected_node_path(Some(target.node_path.clone()));
        }
    }

    pub fn download_targets(self: &Rc<Self>, target: &BrowserTarget, parent: Option<&gtk::Window>) {
        let sources = self.selected_paths_for_target(target);
        if sources.is_empty() || !self.file_access.borrow().supports_download() {
            return;
        }

        if sources.len() == 1 && !target.is_dir {
            let Some(name) = sources[0].file_name() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title("Download File")
                .accept_label("Download")
                .initial_name(name)
                .modal(true)
                .build();
            dialog.save(parent, None::<&gio::Cancellable>, {
                let browser = self.clone();

                move |result| {
                    let Ok(file) = result else {
                        return;
                    };
                    let Some(destination) = file.path() else {
                        browser.show_error(
                            "Download Failed",
                            "Choose a destination on the local filesystem.",
                        );
                        return;
                    };
                    browser.start_download(sources, FileDownloadDestination::File(destination));
                }
            });
            return;
        }

        let dialog = gtk::FileDialog::builder()
            .title("Choose Download Folder")
            .accept_label("Download")
            .modal(true)
            .build();
        dialog.select_folder(parent, None::<&gio::Cancellable>, {
            let browser = self.clone();

            move |result| {
                let Ok(folder) = result else {
                    return;
                };
                let Some(destination) = folder.path() else {
                    browser.show_error(
                        "Download Failed",
                        "Choose a destination on the local filesystem.",
                    );
                    return;
                };
                browser.start_download(sources, FileDownloadDestination::Folder(destination));
            }
        });
    }

    fn start_download(
        self: &Rc<Self>,
        sources: Vec<FileNodePath>,
        destination: FileDownloadDestination,
    ) {
        let file_access = self.file_access.borrow().clone();
        self.start_download_with_access(file_access, sources, destination, true);
    }

    fn start_download_with_access(
        self: &Rc<Self>,
        file_access: Arc<dyn FileAccess>,
        sources: Vec<FileNodePath>,
        destination: FileDownloadDestination,
        allow_sudo: bool,
    ) {
        let count = sources.len();
        let result_command = command_mailbox::once({
            let browser = self.clone();
            let retry_sources = sources.clone();
            let retry_destination = destination.clone();

            move |result: Result<Vec<PathBuf>, String>| match result {
                Ok(paths) => {
                    let message = if paths.len() == 1 {
                        format!("Downloaded {}.", paths[0].display())
                    } else {
                        format!("Downloaded {} items.", paths.len())
                    };
                    browser.notify_open_message(&message);
                }
                Err(err) if err == TRANSFER_CANCELED_MESSAGE => {
                    log::info!("remote download canceled count={count}");
                }
                Err(err) if allow_sudo && craic_system::system::is_permission_denied_message(&err) => {
                    let retry_browser = browser.clone();
                    let prompt_browser = browser.clone();
                    let retry_sources = retry_sources.clone();
                    let retry_destination = retry_destination.clone();
                    super::file_ops::offer_sudo_retry(
                        prompt_browser,
                        "Download Failed",
                        &err,
                        Rc::new(move |sudo_access| {
                            retry_browser.start_download_with_access(
                                sudo_access,
                                retry_sources.clone(),
                                retry_destination.clone(),
                                false,
                            );
                        }),
                    );
                }
                Err(err) => browser.show_error("Download Failed", &err),
            }
        });
        thread::spawn(move || {
            log::info!("remote download worker start count={count}");
            let result = file_access.download_to_local(FileDownloadRequest {
                sources,
                destination,
                cancel_requested: None,
            });
            result_command.send(result);
        });
    }

    pub fn copy_target(&self, target: &BrowserTarget, operation: TransferOperation) {
        let clipboard = FileClipboard {
            source_access: self.file_access.borrow().clone(),
            paths: vec![target.node_path.clone()],
            operation,
        };
        self.file_clipboard.replace(Some(clipboard.clone()));
        set_shared_file_clipboard(Some(clipboard));
        set_clipboard_text(&target.path);
    }

    pub fn copy_selected_target(&self, operation: TransferOperation) {
        let Some(path) = self.selected_node_path.borrow().clone() else {
            return;
        };
        let target = self.target_for_node_path(path);
        self.copy_target(&target, operation);
    }

    pub fn copy_absolute_path(&self, target: &BrowserTarget) {
        self.file_clipboard.borrow_mut().take();
        set_shared_file_clipboard(None);
        let text = self.file_access.borrow().copy_path(&target.node_path);
        set_clipboard_text(&text);
    }

    pub fn copy_relative_path(&self, target: &BrowserTarget) {
        self.file_clipboard.borrow_mut().take();
        set_shared_file_clipboard(None);
        set_clipboard_text(&target.path);
    }

    pub fn open_external(self: &Rc<Self>, target: &BrowserTarget, parent: Option<gtk::Window>) {
        let Some(desktop_opener) = self.desktop_opener.borrow().clone() else {
            self.notify_open_message("Opening files externally is unavailable for this workspace.");
            return;
        };
        let kind = if target.is_dir {
            DesktopOpenTargetKind::Folder
        } else {
            DesktopOpenTargetKind::File
        };
        match desktop_opener
            .resolve_open_path(&target.node_path, kind)
            .and_then(|effect| craic_ui_core::ui::platform::execute_effect(effect, parent.as_ref()))
        {
            Ok(message) => self.notify_open_message(&message),
            Err(err) => self.notify_open_message(&err),
        }
    }

    pub fn open_containing_folder(
        self: &Rc<Self>,
        target: &BrowserTarget,
        parent: Option<gtk::Window>,
    ) {
        let Some(desktop_opener) = self.desktop_opener.borrow().clone() else {
            self.show_error(
                "Open Failed",
                "Opening containing folders is unavailable for this workspace.",
            );
            return;
        };
        match desktop_opener
            .resolve_reveal_path(&target.node_path)
            .and_then(|effect| craic_ui_core::ui::platform::execute_effect(effect, parent.as_ref()))
        {
            Ok(message) => self.notify_open_message(&message),
            Err(err) => self.show_error("Open Failed", &err),
        }
    }

    pub fn open_terminal(&self, target: &BrowserTarget) {
        let callbacks = self.terminal_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone(), target.is_dir);
        }
    }

    pub fn run_in_terminal(&self, target: &BrowserTarget) {
        if target.is_dir || !target.executable {
            return;
        }
        let callbacks = self.run_terminal_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone());
        }
    }

    pub fn add_to_chat(&self, target: &BrowserTarget) {
        if target.is_dir {
            return;
        }
        let callbacks = self.chat_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone());
        }
    }

    pub fn add_to_ignore(&self, pattern: &str) {
        let callbacks = self.ignore_callbacks.borrow().clone();
        for callback in callbacks {
            callback(pattern.to_string());
        }
    }

    pub fn run_container_file_action(
        &self,
        target: &BrowserTarget,
        action: super::ContainerFileAction,
    ) {
        if target.is_dir {
            return;
        }
        let callbacks = self.container_file_action_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone(), action);
        }
    }
}
