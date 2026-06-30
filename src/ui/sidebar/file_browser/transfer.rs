use super::{BrowserTarget, FileBrowser, join_relative, parent_folder, should_skip};
use crate::system::capabilities::files::{FileAccess, FileKind};
use crate::system::capabilities::open::OpenTargetKind;
use adw::prelude::*;
use gtk::gdk;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, TryRecvError},
};
use std::thread;
use std::time::Duration;

const TRANSFER_CANCELED_MESSAGE: &str = "Transfer canceled.";

impl FileBrowser {
    pub(super) fn set_internal_drag_paths(&self, paths: Vec<String>) {
        self.internal_drag_paths.replace(Some(paths));
    }

    pub(super) fn clear_internal_drag_paths(self: &Rc<Self>) {
        self.internal_drag_paths.borrow_mut().take();
        self.clear_drop_target_folder();
    }

    pub(super) fn handle_drop_hover(
        self: &Rc<Self>,
        _external_sources_available: bool,
        target_relative: String,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> gdk::DragAction {
        let Some(operation) =
            self.drop_operation_for_target(&target_relative, available_actions, modifiers)
        else {
            self.clear_drop_target_folder();
            return gdk::DragAction::empty();
        };

        self.set_drop_target_folder(Some(target_relative));
        operation.drag_action()
    }

    pub(super) fn handle_dropped_paths(
        self: &Rc<Self>,
        external_sources_available: bool,
        target_relative: String,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> bool {
        let Some(operation) =
            self.drop_operation_for_target(&target_relative, available_actions, modifiers)
        else {
            self.clear_drop_target_folder();
            if external_sources_available {
                self.show_error(
                    "Drop Unavailable",
                    "Dropping local files into this workspace is not available.",
                );
            }
            return false;
        };
        self.clear_drop_target_folder();

        let Some(paths) = self.internal_drag_paths.borrow().clone() else {
            self.show_error(
                "Drop Unavailable",
                "Dropping local files into this workspace is not available.",
            );
            return false;
        };
        self.transfer_workspace_paths_to_folder(paths, target_relative, operation, false);
        true
    }

    fn drop_operation_for_target(
        &self,
        target_relative: &str,
        available_actions: gdk::DragAction,
        modifiers: gdk::ModifierType,
    ) -> Option<TransferOperation> {
        if self.internal_drag_paths.borrow().is_none() {
            return None;
        }
        let target = self.workspace_path(target_relative);
        let Ok(metadata) = self.file_access.borrow().metadata(&target) else {
            return None;
        };
        if metadata.kind != FileKind::Directory {
            return None;
        }

        let operation = if copy_drag_modifier(modifiers) {
            TransferOperation::Copy
        } else {
            TransferOperation::Move
        };
        operation
            .action_allowed(available_actions)
            .then_some(operation)
    }

    fn set_drop_target_folder(self: &Rc<Self>, target: Option<String>) {
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

    pub(super) fn clear_drop_target_folder(self: &Rc<Self>) {
        self.set_drop_target_folder(None);
    }

    fn schedule_drop_auto_expand(self: &Rc<Self>, target: String) {
        if target.is_empty()
            || !self.search_query.borrow().is_empty()
            || self.expanded_dirs.borrow().contains(&target)
            || !self.workspace_is_directory(&target)
        {
            return;
        }

        let generation = self.drop_hover_generation.get();
        gtk::glib::timeout_add_local_once(Duration::from_millis(500), {
            let browser = self.clone();

            move || {
                if browser.drop_hover_generation.get() != generation
                    || browser.drop_target_folder.borrow().as_deref() != Some(target.as_str())
                    || !browser.search_query.borrow().is_empty()
                    || !browser.workspace_is_directory(&target)
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

    pub(super) fn current_drop_target_folder(&self) -> Option<String> {
        self.drop_target_folder.borrow().clone()
    }

    fn workspace_is_directory(&self, relative: &str) -> bool {
        self.file_access
            .borrow()
            .metadata(&self.workspace_path(relative))
            .is_ok_and(|metadata| metadata.kind == FileKind::Directory)
    }

    fn transfer_workspace_paths_to_folder(
        self: &Rc<Self>,
        sources: Vec<String>,
        target_relative: String,
        operation: TransferOperation,
        auto_focus: bool,
    ) {
        if sources.is_empty() {
            return;
        }
        if !self.workspace_is_directory(&target_relative) {
            self.show_error(operation.failure_heading(), "Drop target is not a folder.");
            return;
        }

        let workspace = self.workspace.borrow().clone();
        let file_access = self.file_access.borrow().clone();
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
            ),
        );
        self.refresh_transfer_progress_rows();

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            log::info!(
                "file transfer start workspace={} operation={operation:?} count={}",
                workspace.display_name,
                sources.len()
            );
            let progress_sender = sender.clone();
            let result = transfer_workspace_paths(
                file_access,
                &workspace,
                sources,
                &target_relative,
                operation,
                &cancel_requested,
                move |progress| {
                    let _ = progress_sender.send(TransferEvent::Progress(progress));
                },
            );
            let _ = sender.send(TransferEvent::Finished(result));
        });

        gtk::glib::timeout_add_local(Duration::from_millis(super::SEARCH_POLL_MS), {
            let browser = self.clone();

            move || {
                let mut progress_changed = false;
                let mut progress_path_changed = false;

                loop {
                    match receiver.try_recv() {
                        Ok(TransferEvent::Progress(progress)) => {
                            if browser.set_transfer_progress(transfer_id, progress) {
                                progress_path_changed = true;
                            }
                            progress_changed = true;
                        }
                        Ok(TransferEvent::Finished(result)) => {
                            browser.finish_transfer(transfer_id, operation, result);
                            return gtk::glib::ControlFlow::Break;
                        }
                        Err(TryRecvError::Empty) => {
                            if progress_path_changed {
                                browser.invalidate_tree_rows_cache();
                                browser.rebuild_if_changed();
                            } else if progress_changed {
                                browser.refresh_transfer_progress_rows();
                            }
                            return gtk::glib::ControlFlow::Continue;
                        }
                        Err(TryRecvError::Disconnected) => {
                            browser.finish_disconnected_transfer(transfer_id, operation);
                            return gtk::glib::ControlFlow::Break;
                        }
                    }
                }
            }
        });
    }

    fn set_transfer_progress(&self, transfer_id: u64, progress: TransferProgressUpdate) -> bool {
        if let Some(active) = self.active_transfers.borrow_mut().get_mut(&transfer_id) {
            let current_path_changed = active.current_relative != progress.current_relative;
            active.current_relative = progress.current_relative;
            active.copied_bytes = progress.copied_bytes;
            active.total_bytes = progress.total_bytes;
            active.copied_files = progress.copied_files;
            active.total_files = progress.total_files;
            return current_path_changed;
        }
        false
    }

    fn finish_transfer(
        self: &Rc<Self>,
        transfer_id: u64,
        operation: TransferOperation,
        result: Result<Vec<String>, String>,
    ) {
        let auto_focus = self
            .active_transfers
            .borrow_mut()
            .remove(&transfer_id)
            .is_some_and(|active| active.auto_focus);
        self.refresh_transfer_progress_rows();

        match result {
            Ok(destinations) => {
                if operation == TransferOperation::Move {
                    self.file_clipboard.borrow_mut().take();
                }
                self.invalidate_tree_rows_cache();
                self.rebuild_if_changed();
                if auto_focus {
                    self.auto_focus_transferred_items(destinations);
                }
            }
            Err(message) => {
                self.invalidate_tree_rows_cache();
                self.rebuild_if_changed();
                if message == TRANSFER_CANCELED_MESSAGE {
                    log::info!("file transfer canceled id={transfer_id}");
                } else {
                    self.show_error(operation.failure_heading(), &message);
                }
            }
        }
    }

    fn finish_disconnected_transfer(
        self: &Rc<Self>,
        transfer_id: u64,
        operation: TransferOperation,
    ) {
        self.active_transfers.borrow_mut().remove(&transfer_id);
        self.refresh_transfer_progress_rows();
        self.invalidate_tree_rows_cache();
        self.rebuild_if_changed();
        self.show_error(
            operation.failure_heading(),
            "Transfer operation did not return a result.",
        );
    }

    fn refresh_transfer_progress_rows(self: &Rc<Self>) {
        let rows = self.list_rows.borrow().clone();
        self.set_browser_rows(rows);
    }

    fn auto_focus_transferred_items(self: &Rc<Self>, destinations: Vec<String>) {
        let Some(selected) = destinations.into_iter().find(|path| !path.is_empty()) else {
            return;
        };
        self.set_selected_path(selected.clone());
        self.scroll_selected_row_into_view();
        self.focus_selected_row();
        log::info!("file transfer auto-focused item path={selected}");
    }

    pub(super) fn confirm_cancel_transfers(self: &Rc<Self>, transfer_ids: Vec<u64>) {
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

    fn cancel_transfers(&self, transfer_ids: &[u64]) {
        for transfer_id in transfer_ids {
            if let Some(transfer) = self.active_transfers.borrow().get(transfer_id) {
                transfer.cancel_requested.store(true, Ordering::Relaxed);
                log::info!("file transfer cancel requested id={transfer_id}");
            }
        }
    }

    pub(super) fn transfer_progress_for_path(&self, path: &str) -> Option<TransferRowProgress> {
        let transfers = self.active_transfers.borrow();
        let mut count = 0usize;
        let mut copied_bytes = 0u64;
        let mut total_bytes = 0u64;
        let mut copied_files = 0u64;
        let mut total_files = 0u64;
        let mut operation = None;
        let mut transfer_ids = Vec::new();

        for (transfer_id, transfer) in transfers.iter() {
            if transfer.current_relative.as_deref() != Some(path) {
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

    pub(super) fn paste_target_folder(self: &Rc<Self>) -> String {
        let selected = self.selected_path.borrow().clone();
        if selected.is_empty() {
            return self.active_folder.borrow().clone();
        }

        if self.workspace_is_directory(&selected) {
            selected
        } else {
            parent_folder(&selected)
        }
    }

    pub(super) fn target_paste_folder(&self, target: &BrowserTarget) -> String {
        if target.is_dir {
            target.path.clone()
        } else {
            parent_folder(&target.path)
        }
    }

    pub(super) fn paste_clipboard_files(self: &Rc<Self>) {
        self.paste_into_folder(self.paste_target_folder());
    }

    pub(super) fn paste_into_folder(self: &Rc<Self>, target_relative: String) {
        let Some(clipboard) = self.file_clipboard.borrow().clone() else {
            return;
        };
        self.transfer_workspace_paths_to_folder(
            clipboard.paths,
            target_relative,
            clipboard.operation,
            true,
        );
    }

    pub(super) fn open_target(self: &Rc<Self>, target: &BrowserTarget) {
        if target.is_dir {
            if !target.path.is_empty() {
                self.toggle_dir(&target.path);
            } else {
                self.open_external(target);
            }
        } else {
            self.set_selected_path(target.path.clone());
        }
    }

    pub(super) fn copy_target(&self, target: &BrowserTarget, operation: TransferOperation) {
        self.file_clipboard.replace(Some(FileClipboard {
            paths: vec![target.path.clone()],
            operation,
        }));
        set_clipboard_text(&target.path);
    }

    pub(super) fn copy_selected_target(&self, operation: TransferOperation) {
        let path = self.selected_path.borrow().clone();
        if path.is_empty() {
            return;
        }

        let workspace_path = self.workspace_path(&path);
        let is_dir = match self.file_access.borrow().metadata(&workspace_path) {
            Ok(metadata) => metadata.kind == FileKind::Directory,
            Err(err) => {
                let heading = match operation {
                    TransferOperation::Copy => "Copy Failed",
                    TransferOperation::Move => "Cut Failed",
                };
                self.show_error(heading, &format!("Unable to inspect {path}: {err}"));
                return;
            }
        };
        self.copy_target(
            &BrowserTarget {
                path,
                is_dir,
                executable: false,
            },
            operation,
        );
    }

    pub(super) fn copy_absolute_path(&self, target: &BrowserTarget) {
        self.file_clipboard.borrow_mut().take();
        let path = self.workspace_path(&target.path);
        let text = self
            .opener
            .borrow()
            .as_ref()
            .map(|opener| opener.copyable_path(&path))
            .unwrap_or_else(|| path.absolute);
        set_clipboard_text(&text);
    }

    pub(super) fn copy_relative_path(&self, target: &BrowserTarget) {
        self.file_clipboard.borrow_mut().take();
        set_clipboard_text(&target.path);
    }

    pub(super) fn open_external(self: &Rc<Self>, target: &BrowserTarget) {
        let Some(opener) = self.opener.borrow().clone() else {
            self.notify_open_message("Opening files externally is unavailable for this workspace.");
            return;
        };
        let kind = if target.is_dir {
            OpenTargetKind::Folder
        } else {
            OpenTargetKind::File
        };
        match opener.open_path(&self.workspace_path(&target.path), kind) {
            Ok(message) => self.notify_open_message(&message),
            Err(err) => self.notify_open_message(&err),
        }
    }

    pub(super) fn open_containing_folder(self: &Rc<Self>, target: &BrowserTarget) {
        let Some(opener) = self.opener.borrow().clone() else {
            self.show_error(
                "Open Failed",
                "Opening containing folders is unavailable for this workspace.",
            );
            return;
        };
        match opener.reveal_path(&self.workspace_path(&target.path)) {
            Ok(message) => self.notify_open_message(&message),
            Err(err) => self.show_error("Open Failed", &err),
        }
    }

    pub(super) fn open_terminal(&self, target: &BrowserTarget) {
        let callbacks = self.terminal_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone(), target.is_dir);
        }
    }

    pub(super) fn run_in_terminal(&self, target: &BrowserTarget) {
        if target.is_dir || !target.executable {
            return;
        }
        let callbacks = self.run_terminal_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone());
        }
    }

    pub(super) fn add_to_chat(&self, target: &BrowserTarget) {
        if target.is_dir {
            return;
        }
        let callbacks = self.chat_callbacks.borrow().clone();
        for callback in callbacks {
            callback(target.path.clone());
        }
    }

    pub(super) fn add_to_ignore(&self, pattern: &str) {
        let callbacks = self.ignore_callbacks.borrow().clone();
        for callback in callbacks {
            callback(pattern.to_string());
        }
    }

    pub(super) fn run_container_file_action(
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

#[derive(Clone)]
pub(super) struct FileClipboard {
    paths: Vec<String>,
    operation: TransferOperation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransferOperation {
    Copy,
    Move,
}

impl TransferOperation {
    fn drag_action(self) -> gdk::DragAction {
        match self {
            Self::Copy => gdk::DragAction::COPY,
            Self::Move => gdk::DragAction::MOVE,
        }
    }

    fn action_allowed(self, actions: gdk::DragAction) -> bool {
        actions.contains(self.drag_action())
    }

    fn present_participle(self) -> &'static str {
        match self {
            Self::Copy => "Copying",
            Self::Move => "Moving",
        }
    }

    fn failure_heading(self) -> &'static str {
        match self {
            Self::Copy => "Copy Failed",
            Self::Move => "Move Failed",
        }
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct TransferRowProgress {
    pub(super) fraction: f64,
    pub(super) transfer_ids: Vec<u64>,
    pub(super) tooltip: String,
}

pub(super) struct ActiveTransfer {
    operation: TransferOperation,
    auto_focus: bool,
    cancel_requested: Arc<AtomicBool>,
    current_relative: Option<String>,
    copied_bytes: u64,
    total_bytes: u64,
    copied_files: u64,
    total_files: u64,
}

impl ActiveTransfer {
    fn new(
        operation: TransferOperation,
        total_files: u64,
        auto_focus: bool,
        cancel_requested: Arc<AtomicBool>,
    ) -> Self {
        Self {
            operation,
            auto_focus,
            cancel_requested,
            current_relative: None,
            copied_bytes: 0,
            total_bytes: 0,
            copied_files: 0,
            total_files,
        }
    }
}

#[derive(Clone)]
struct TransferProgressUpdate {
    current_relative: Option<String>,
    copied_bytes: u64,
    total_bytes: u64,
    copied_files: u64,
    total_files: u64,
}

enum TransferEvent {
    Progress(TransferProgressUpdate),
    Finished(Result<Vec<String>, String>),
}

fn transfer_workspace_paths(
    file_access: Arc<dyn FileAccess>,
    workspace: &crate::system::WorkspaceRef,
    sources: Vec<String>,
    target_relative: &str,
    operation: TransferOperation,
    cancel_requested: &AtomicBool,
    mut progress: impl FnMut(TransferProgressUpdate),
) -> Result<Vec<String>, String> {
    let mut destinations = Vec::new();
    let total_files = sources.len() as u64;
    let mut copied_files = 0u64;
    for source in sources {
        check_transfer_canceled(cancel_requested)?;
        let name = file_name_for_transfer(&source)?;
        let destination = join_relative(
            (!target_relative.is_empty()).then_some(target_relative),
            &name,
        );
        if source == destination {
            continue;
        }
        if file_access.metadata(&workspace.path(&destination)).is_ok() {
            return Err(format!("{destination} already exists."));
        }
        match operation {
            TransferOperation::Copy => copy_workspace_entry(
                file_access.clone(),
                workspace,
                &source,
                &destination,
                cancel_requested,
                &mut progress,
            )?,
            TransferOperation::Move => {
                file_access.rename(&workspace.path(&source), &workspace.path(&destination))?;
            }
        }
        copied_files = copied_files.saturating_add(1);
        progress(TransferProgressUpdate {
            current_relative: Some(destination.clone()),
            copied_bytes: 0,
            total_bytes: 0,
            copied_files,
            total_files,
        });
        destinations.push(destination);
    }
    Ok(destinations)
}

fn copy_workspace_entry(
    file_access: Arc<dyn FileAccess>,
    workspace: &crate::system::WorkspaceRef,
    source_relative: &str,
    destination_relative: &str,
    cancel_requested: &AtomicBool,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<(), String> {
    check_transfer_canceled(cancel_requested)?;
    let source = workspace.path(source_relative);
    let destination = workspace.path(destination_relative);
    let metadata = file_access.metadata(&source)?;
    match metadata.kind {
        FileKind::Directory => {
            file_access.create_dir(&destination)?;
            progress(TransferProgressUpdate {
                current_relative: Some(destination_relative.to_string()),
                copied_bytes: 0,
                total_bytes: 0,
                copied_files: 0,
                total_files: 0,
            });
            let entries = file_access
                .list_dirs(std::slice::from_ref(&source))?
                .into_iter()
                .next()
                .map(|listing| listing.entries)
                .unwrap_or_default();
            for entry in entries {
                let Some(child_source) = entry.path.path.relative.clone() else {
                    continue;
                };
                if should_skip(&entry.name) {
                    continue;
                }
                let child_destination = join_relative(Some(destination_relative), &entry.name);
                copy_workspace_entry(
                    file_access.clone(),
                    workspace,
                    &child_source,
                    &child_destination,
                    cancel_requested,
                    progress,
                )?;
            }
        }
        FileKind::File | FileKind::Symlink | FileKind::Other => {
            let bytes = file_access.read_bytes(&source, None)?;
            let total_bytes = bytes.len() as u64;
            file_access.write_bytes(&destination, &bytes)?;
            progress(TransferProgressUpdate {
                current_relative: Some(destination_relative.to_string()),
                copied_bytes: total_bytes,
                total_bytes,
                copied_files: 1,
                total_files: 1,
            });
        }
    }
    Ok(())
}

fn file_name_for_transfer(relative: &str) -> Result<String, String> {
    let name = relative
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .ok_or_else(|| "Cannot transfer workspace root.".to_string())?;
    if should_skip(name) {
        return Err("That name is hidden by the file browser.".to_string());
    }
    Ok(name.to_string())
}

fn check_transfer_canceled(cancel_requested: &AtomicBool) -> Result<(), String> {
    if cancel_requested.load(Ordering::Relaxed) {
        Err(TRANSFER_CANCELED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn copy_drag_modifier(modifiers: gdk::ModifierType) -> bool {
    modifiers.contains(gdk::ModifierType::ALT_MASK)
}

#[cfg(not(target_os = "macos"))]
fn copy_drag_modifier(modifiers: gdk::ModifierType) -> bool {
    modifiers.contains(gdk::ModifierType::CONTROL_MASK)
}

fn set_clipboard_text(text: &str) {
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}
