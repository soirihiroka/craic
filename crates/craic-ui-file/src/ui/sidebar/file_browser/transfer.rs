use super::rows;
use super::{BrowserTarget, FileBrowser, should_skip};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{
    FileAccess, FileCopyRequest, FileDeleteRequest, FileKind, FileMoveRequest, FileOperation,
    FileOperationEvent, FileOperationProgress, FileRead, FileReadRequest, FileWriteMode,
    FileWritePayload, FileWriteRequest,
};
use crate::system::capabilities::open::{DesktopOpenActivation, DesktopOpenTargetKind};
use adw::prelude::*;
use gtk::gdk;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

const TRANSFER_CANCELED_MESSAGE: &str = "Transfer canceled.";
const LOCAL_FILE_TRANSFER_CHUNK_BYTES: usize = 1024 * 1024;
const TRANSFER_PROGRESS_EMIT_INTERVAL: Duration = Duration::from_secs(1);

static FILE_CLIPBOARD: OnceLock<Mutex<Option<FileClipboard>>> = OnceLock::new();
static DRAG_CLIPBOARD: OnceLock<Mutex<Option<FileClipboard>>> = OnceLock::new();
static NEXT_TRANSFER_UI_HANDLER_ID: AtomicU64 = AtomicU64::new(1);

type TransferEventQueue = Arc<Mutex<VecDeque<TransferEvent>>>;

thread_local! {
    static TRANSFER_UI_HANDLERS: RefCell<HashMap<u64, TransferUiHandler>> =
        RefCell::new(HashMap::new());
}

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
        if self.internal_drag_paths.borrow().is_none() && shared_drag_clipboard().is_none() {
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
        if sources.is_empty() {
            return;
        }
        let operation = TransferOperation::Copy;

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
        if clipboard.paths.is_empty() {
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
                clipboard.paths.len() as u64,
                auto_focus,
                cancel_requested.clone(),
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
                file_access,
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
                    self.show_error(operation.failure_heading(), &message);
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
                self.open_external(
                    target,
                    DesktopOpenActivation::from_parent(parent_window.as_ref()),
                );
            }
        } else {
            self.set_selected_node_path(Some(target.node_path.clone()));
        }
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

    pub fn open_external(
        self: &Rc<Self>,
        target: &BrowserTarget,
        activation: DesktopOpenActivation,
    ) {
        let Some(desktop_opener) = self.desktop_opener.borrow().clone() else {
            self.notify_open_message("Opening files externally is unavailable for this workspace.");
            return;
        };
        let kind = if target.is_dir {
            DesktopOpenTargetKind::Folder
        } else {
            DesktopOpenTargetKind::File
        };
        match desktop_opener.open_path(&target.node_path, kind, activation) {
            Ok(message) => self.notify_open_message(&message),
            Err(err) => self.notify_open_message(&err),
        }
    }

    pub fn open_containing_folder(
        self: &Rc<Self>,
        target: &BrowserTarget,
        activation: DesktopOpenActivation,
    ) {
        let Some(desktop_opener) = self.desktop_opener.borrow().clone() else {
            self.show_error(
                "Open Failed",
                "Opening containing folders is unavailable for this workspace.",
            );
            return;
        };
        match desktop_opener.reveal_path(&target.node_path, activation) {
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

#[derive(Clone)]
pub struct FileClipboard {
    source_access: Arc<dyn FileAccess>,
    paths: Vec<FileNodePath>,
    operation: TransferOperation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferOperation {
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
pub struct TransferRowProgress {
    pub fraction: f64,
    pub transfer_ids: Vec<u64>,
    pub tooltip: String,
}

pub struct ActiveTransfer {
    operation: TransferOperation,
    auto_focus: bool,
    cancel_requested: Arc<AtomicBool>,
    current_path: Option<FileNodePath>,
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
            current_path: None,
            copied_bytes: 0,
            total_bytes: 0,
            copied_files: 0,
            total_files,
        }
    }
}

#[derive(Clone)]
struct TransferProgressUpdate {
    current_path: Option<FileNodePath>,
    copied_bytes: u64,
    total_bytes: u64,
    copied_files: u64,
    total_files: u64,
}

enum TransferEvent {
    Progress(TransferProgressUpdate),
    Finished(Result<Vec<FileNodePath>, String>),
}

struct TransferUiHandler {
    browser: Weak<FileBrowser>,
    transfer_id: u64,
    operation: TransferOperation,
    queue: TransferEventQueue,
    latest_progress: Arc<Mutex<Option<TransferProgressUpdate>>>,
    drain_scheduled: Arc<AtomicBool>,
}

#[derive(Clone)]
struct TransferUiDispatcher {
    handler_id: u64,
    queue: TransferEventQueue,
    latest_progress: Arc<Mutex<Option<TransferProgressUpdate>>>,
    drain_scheduled: Arc<AtomicBool>,
}

impl TransferUiDispatcher {
    fn new(browser: &Rc<FileBrowser>, transfer_id: u64, operation: TransferOperation) -> Self {
        let handler_id = NEXT_TRANSFER_UI_HANDLER_ID.fetch_add(1, Ordering::Relaxed);
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let latest_progress = Arc::new(Mutex::new(None));
        let drain_scheduled = Arc::new(AtomicBool::new(false));
        TRANSFER_UI_HANDLERS.with(|handlers| {
            handlers.borrow_mut().insert(
                handler_id,
                TransferUiHandler {
                    browser: Rc::downgrade(browser),
                    transfer_id,
                    operation,
                    queue: queue.clone(),
                    latest_progress: latest_progress.clone(),
                    drain_scheduled: drain_scheduled.clone(),
                },
            );
        });
        gtk::glib::timeout_add_local(TRANSFER_PROGRESS_EMIT_INTERVAL, move || {
            transfer_ui_tick(handler_id)
        });

        Self {
            handler_id,
            queue,
            latest_progress,
            drain_scheduled,
        }
    }

    fn send(&self, event: TransferEvent) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(event);
        } else {
            return;
        }

        if self
            .drain_scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let handler_id = self.handler_id;
            gtk::glib::idle_add_once(move || drain_transfer_ui_events(handler_id));
        }
    }
}

struct TransferProgressSender {
    dispatcher: TransferUiDispatcher,
    last_progress_at: Option<Instant>,
    last_path: Option<FileNodePath>,
}

impl TransferProgressSender {
    fn new(dispatcher: TransferUiDispatcher) -> Self {
        Self {
            dispatcher,
            last_progress_at: None,
            last_path: None,
        }
    }

    fn send(&mut self, progress: TransferProgressUpdate) {
        let now = Instant::now();
        let path_changed = self.last_path.as_ref() != progress.current_path.as_ref();
        let elapsed = self
            .last_progress_at
            .is_none_or(|last| now.duration_since(last) >= TRANSFER_PROGRESS_EMIT_INTERVAL);
        if let Ok(mut latest_progress) = self.dispatcher.latest_progress.lock() {
            *latest_progress = Some(progress.clone());
        }
        if !path_changed && !elapsed {
            return;
        }

        self.last_progress_at = Some(now);
        self.last_path.clone_from(&progress.current_path);
        self.dispatcher.send(TransferEvent::Progress(progress));
    }
}

fn transfer_ui_tick(handler_id: u64) -> gtk::glib::ControlFlow {
    let Some((browser, transfer_id, latest_progress)) = TRANSFER_UI_HANDLERS.with(|handlers| {
        let handlers = handlers.borrow();
        let handler = handlers.get(&handler_id)?;
        Some((
            handler.browser.upgrade(),
            handler.transfer_id,
            handler.latest_progress.clone(),
        ))
    }) else {
        return gtk::glib::ControlFlow::Break;
    };
    let Some(browser) = browser else {
        TRANSFER_UI_HANDLERS.with(|handlers| {
            handlers.borrow_mut().remove(&handler_id);
        });
        return gtk::glib::ControlFlow::Break;
    };
    if !browser.active_transfers.borrow().contains_key(&transfer_id) {
        return gtk::glib::ControlFlow::Break;
    }

    let progress = latest_progress
        .lock()
        .ok()
        .and_then(|mut progress| progress.take());
    if let Some(progress) = progress {
        if browser.set_transfer_progress(transfer_id, progress) {
            browser.invalidate_tree_rows_cache();
            browser.rebuild_if_changed();
        } else {
            browser.refresh_transfer_progress_rows();
        }
    } else {
        browser.refresh_transfer_progress_rows();
    }
    gtk::glib::ControlFlow::Continue
}

fn drain_transfer_ui_events(handler_id: u64) {
    let Some((browser, transfer_id, operation, queue, drain_scheduled)) = TRANSFER_UI_HANDLERS
        .with(|handlers| {
            let handlers = handlers.borrow();
            let handler = handlers.get(&handler_id)?;
            Some((
                handler.browser.upgrade(),
                handler.transfer_id,
                handler.operation,
                handler.queue.clone(),
                handler.drain_scheduled.clone(),
            ))
        })
    else {
        return;
    };
    let Some(browser) = browser else {
        TRANSFER_UI_HANDLERS.with(|handlers| {
            handlers.borrow_mut().remove(&handler_id);
        });
        return;
    };

    let mut progress_changed = false;
    let mut progress_path_changed = false;
    let mut finished = false;

    loop {
        let event = queue.lock().ok().and_then(|mut queue| queue.pop_front());
        match event {
            Some(TransferEvent::Progress(progress)) => {
                if browser.set_transfer_progress(transfer_id, progress) {
                    progress_path_changed = true;
                }
                progress_changed = true;
            }
            Some(TransferEvent::Finished(result)) => {
                browser.finish_transfer(transfer_id, operation, result);
                finished = true;
                break;
            }
            None => break,
        }
    }

    if finished {
        TRANSFER_UI_HANDLERS.with(|handlers| {
            handlers.borrow_mut().remove(&handler_id);
        });
        return;
    }

    if progress_path_changed {
        browser.invalidate_tree_rows_cache();
        browser.rebuild_if_changed();
    } else if progress_changed {
        browser.refresh_transfer_progress_rows();
    }

    drain_scheduled.store(false, Ordering::Release);
    let has_more = queue.lock().is_ok_and(|queue| !queue.is_empty());
    if has_more
        && drain_scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        gtk::glib::idle_add_once(move || drain_transfer_ui_events(handler_id));
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LocalTransferTotals {
    bytes: u64,
    files: u64,
}

#[derive(Clone, Copy, Debug)]
struct LocalTransferProgress {
    completed_bytes: u64,
    total_bytes: u64,
    completed_files: u64,
    total_files: u64,
}

impl LocalTransferProgress {
    fn from_totals(totals: LocalTransferTotals) -> Self {
        Self {
            completed_bytes: 0,
            total_bytes: totals.bytes,
            completed_files: 0,
            total_files: totals.files,
        }
    }

    fn add_bytes(&mut self, bytes: u64) {
        self.completed_bytes = self.completed_bytes.saturating_add(bytes);
    }

    fn complete_file(&mut self) {
        self.completed_files = self.completed_files.saturating_add(1);
    }

    fn to_update(self, current_path: &FileNodePath) -> TransferProgressUpdate {
        TransferProgressUpdate {
            current_path: Some(current_path.clone()),
            copied_bytes: self.completed_bytes,
            total_bytes: self.total_bytes,
            copied_files: self.completed_files,
            total_files: self.total_files,
        }
    }
}

fn transfer_local_paths(
    destination_access: Arc<dyn FileAccess>,
    sources: Vec<PathBuf>,
    target_folder: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    mut progress: impl FnMut(TransferProgressUpdate),
) -> Result<Vec<FileNodePath>, String> {
    let mut roots = Vec::new();
    for source in &sources {
        check_transfer_canceled(cancel_requested.as_ref())?;
        let name = file_name_for_local_transfer(&source)?;
        let destination = target_folder.join_child(&name);
        if destination_access.info(&destination).is_ok() {
            return Err(format!("{} already exists.", destination.display()));
        }
        roots.push((source.clone(), destination));
    }
    if let Some((_, destination)) = roots.first() {
        progress(TransferProgressUpdate {
            current_path: Some(destination.clone()),
            copied_bytes: 0,
            total_bytes: 0,
            copied_files: 0,
            total_files: sources.len() as u64,
        });
    }

    let mut totals = LocalTransferTotals::default();
    for (source, _) in &roots {
        check_transfer_canceled(cancel_requested.as_ref())?;
        let source_totals = local_transfer_totals(source, cancel_requested.as_ref())?;
        totals.bytes = totals.bytes.saturating_add(source_totals.bytes);
        totals.files = totals.files.saturating_add(source_totals.files);
    }

    let mut destinations = Vec::new();
    let mut local_progress = LocalTransferProgress::from_totals(totals);
    for (source, destination) in roots {
        check_transfer_canceled(cancel_requested.as_ref())?;
        copy_local_path_to_file_access(
            destination_access.clone(),
            &source,
            destination.clone(),
            destination.clone(),
            cancel_requested.clone(),
            &mut local_progress,
            &mut progress,
        )?;
        progress(local_progress.to_update(&destination));
        destinations.push(destination);
    }
    Ok(destinations)
}

fn local_transfer_totals(
    source: &Path,
    cancel_requested: &AtomicBool,
) -> Result<LocalTransferTotals, String> {
    check_transfer_canceled(cancel_requested)?;
    let metadata = fs::symlink_metadata(source)
        .map_err(|err| format!("Unable to inspect {}: {err}", source.display()))?;
    if metadata.is_dir() {
        let mut totals = LocalTransferTotals { bytes: 0, files: 1 };
        for entry in fs::read_dir(source)
            .map_err(|err| format!("Unable to list {}: {err}", source.display()))?
        {
            let entry = entry.map_err(|err| format!("Unable to read directory entry: {err}"))?;
            let child_totals = local_transfer_totals(&entry.path(), cancel_requested)?;
            totals.bytes = totals.bytes.saturating_add(child_totals.bytes);
            totals.files = totals.files.saturating_add(child_totals.files);
        }
        return Ok(totals);
    }
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Copying symlinks from local file drops is unsupported: {}",
            source.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Only files and folders can be dropped into the file browser: {}",
            source.display()
        ));
    }
    Ok(LocalTransferTotals {
        bytes: metadata.len(),
        files: 1,
    })
}

fn copy_local_path_to_file_access(
    destination_access: Arc<dyn FileAccess>,
    source: &Path,
    destination: FileNodePath,
    display_path: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    local_progress: &mut LocalTransferProgress,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<(), String> {
    check_transfer_canceled(cancel_requested.as_ref())?;
    let metadata = fs::symlink_metadata(source)
        .map_err(|err| format!("Unable to inspect {}: {err}", source.display()))?;
    if destination_access.info(&destination).is_ok() {
        return Err(format!("{} already exists.", destination.display()));
    }
    let result = if metadata.is_dir() {
        write_with_progress(
            destination_access.clone(),
            FileWriteRequest {
                path: destination.clone(),
                mode: FileWriteMode::CreateNew,
                payload: FileWritePayload::Directory,
                cancel_requested: Some(cancel_requested.clone()),
            },
            &display_path,
            local_progress,
            progress,
        )
        .and_then(|_| {
            local_progress.complete_file();
            progress(local_progress.to_update(&display_path));
            for entry in fs::read_dir(source)
                .map_err(|err| format!("Unable to list {}: {err}", source.display()))?
            {
                let entry =
                    entry.map_err(|err| format!("Unable to read directory entry: {err}"))?;
                let child_source = entry.path();
                let name = file_name_for_local_transfer(&child_source)?;
                copy_local_path_to_file_access(
                    destination_access.clone(),
                    &child_source,
                    destination.join_child(name),
                    display_path.clone(),
                    cancel_requested.clone(),
                    local_progress,
                    progress,
                )?;
            }
            Ok(())
        })
    } else if metadata.file_type().is_symlink() {
        Err(format!(
            "Copying symlinks from local file drops is unsupported: {}",
            source.display()
        ))
    } else if !metadata.is_file() {
        Err(format!(
            "Only files and folders can be dropped into the file browser: {}",
            source.display()
        ))
    } else {
        write_local_file_chunks(
            destination_access.clone(),
            source,
            metadata.len(),
            destination.clone(),
            display_path,
            cancel_requested,
            local_progress,
            progress,
        )
    };
    if matches!(&result, Err(message) if message == TRANSFER_CANCELED_MESSAGE) {
        if let Err(err) = delete_file_access_node(destination_access, destination.clone(), None) {
            log::warn!(
                "file transfer canceled cleanup failed path={} err={err}",
                destination.display()
            );
        }
    }
    result
}

fn write_local_file_chunks(
    destination_access: Arc<dyn FileAccess>,
    source: &Path,
    total_bytes: u64,
    destination: FileNodePath,
    display_path: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    local_progress: &mut LocalTransferProgress,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<(), String> {
    let mut file = fs::File::open(source)
        .map_err(|err| format!("Unable to read {}: {err}", source.display()))?;
    let mut buffer = vec![0u8; LOCAL_FILE_TRANSFER_CHUNK_BYTES];
    let mut mode = FileWriteMode::CreateNew;
    let mut wrote_anything = false;
    loop {
        check_transfer_canceled(cancel_requested.as_ref())?;
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("Unable to read {}: {err}", source.display()))?;
        if read == 0 {
            break;
        }
        write_with_progress(
            destination_access.clone(),
            FileWriteRequest {
                path: destination.clone(),
                mode,
                payload: FileWritePayload::File(buffer[..read].to_vec()),
                cancel_requested: Some(cancel_requested.clone()),
            },
            &display_path,
            local_progress,
            progress,
        )?;
        mode = FileWriteMode::Append;
        wrote_anything = true;
    }
    if !wrote_anything {
        debug_assert_eq!(total_bytes, 0);
        write_with_progress(
            destination_access,
            FileWriteRequest {
                path: destination.clone(),
                mode: FileWriteMode::CreateNew,
                payload: FileWritePayload::File(Vec::new()),
                cancel_requested: Some(cancel_requested),
            },
            &display_path,
            local_progress,
            progress,
        )?;
    }
    local_progress.complete_file();
    progress(local_progress.to_update(&display_path));
    Ok(())
}

fn write_with_progress(
    file_access: Arc<dyn FileAccess>,
    request: FileWriteRequest,
    display_path: &FileNodePath,
    local_progress: &mut LocalTransferProgress,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    file_access.write_node(
        request,
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    );
    let mut last_completed_bytes = 0u64;
    loop {
        match receiver.recv() {
            Ok(FileOperationEvent::Progress(update)) => {
                let delta = update.completed_bytes.saturating_sub(last_completed_bytes);
                last_completed_bytes = update.completed_bytes;
                local_progress.add_bytes(delta);
                progress(local_progress.to_update(display_path));
            }
            Ok(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            Err(_) => return Err("write operation did not return a result.".to_string()),
        }
    }
}

fn transfer_workspace_paths(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    sources: Vec<FileNodePath>,
    target_folder: FileNodePath,
    operation: TransferOperation,
    cancel_requested: Arc<AtomicBool>,
    mut progress: impl FnMut(TransferProgressUpdate),
) -> Result<Vec<FileNodePath>, String> {
    let mut destinations = Vec::new();
    let total_files = sources.len() as u64;
    let mut copied_files = 0u64;
    for source in sources {
        check_transfer_canceled(cancel_requested.as_ref())?;
        let name = file_name_for_transfer(&source)?;
        let destination = target_folder.join_child(&name);
        if source == destination {
            continue;
        }
        if destination_access.info(&destination).is_ok() {
            return Err(format!("{} already exists.", destination.display()));
        }
        run_transfer_file_operation(
            source_access.clone(),
            destination_access.clone(),
            operation,
            source.clone(),
            target_folder.clone(),
            name,
            destination.clone(),
            cancel_requested.clone(),
            copied_files,
            total_files,
            &mut progress,
        )?;
        copied_files = copied_files.saturating_add(1);
        progress(TransferProgressUpdate {
            current_path: Some(destination.clone()),
            copied_bytes: 0,
            total_bytes: 0,
            copied_files,
            total_files,
        });
        destinations.push(destination);
    }
    Ok(destinations)
}

fn run_transfer_file_operation(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    operation: TransferOperation,
    source: FileNodePath,
    target_folder: FileNodePath,
    name: String,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    completed_before: u64,
    total_files: u64,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<FileNodePath, String> {
    check_transfer_canceled(cancel_requested.as_ref())?;
    if !Arc::ptr_eq(&source_access, &destination_access) {
        return match operation {
            TransferOperation::Copy => copy_between_file_accesses(
                source_access,
                destination_access,
                source,
                destination,
                cancel_requested,
                completed_before,
                total_files,
                progress,
            ),
            TransferOperation::Move => {
                copy_between_file_accesses(
                    source_access.clone(),
                    destination_access,
                    source.clone(),
                    destination.clone(),
                    cancel_requested.clone(),
                    completed_before,
                    total_files,
                    progress,
                )?;
                delete_file_access_node(source_access, source, Some(cancel_requested))?;
                Ok(destination)
            }
        };
    }

    let (sender, receiver) = mpsc::channel();
    match operation {
        TransferOperation::Copy => destination_access.copy_node(
            FileCopyRequest {
                source: source.clone(),
                destination: destination.clone(),
                cancel_requested: Some(cancel_requested.clone()),
            },
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
        ),
        TransferOperation::Move => destination_access.move_node(
            FileMoveRequest {
                source: source.clone(),
                destination_parent: target_folder,
                new_name: name,
                cancel_requested: Some(cancel_requested.clone()),
            },
            Box::new(move |event| {
                let _ = sender.send(event);
            }),
        ),
    }

    loop {
        match receiver.recv() {
            Ok(FileOperationEvent::Progress(update)) => {
                progress(transfer_progress_update(
                    update,
                    completed_before,
                    total_files,
                    &destination,
                    0,
                    None,
                ));
            }
            Ok(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            Err(_) => {
                return Err(format!(
                    "{} operation did not return a result.",
                    operation.failure_heading()
                ));
            }
        }
    }
}

fn copy_between_file_accesses(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    source: FileNodePath,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    completed_before: u64,
    total_files: u64,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<FileNodePath, String> {
    let result = copy_between_file_accesses_inner(
        source_access,
        destination_access.clone(),
        source,
        destination.clone(),
        cancel_requested,
        completed_before,
        total_files,
        progress,
    );
    if matches!(&result, Err(message) if message == TRANSFER_CANCELED_MESSAGE) {
        if let Err(err) = delete_file_access_node(destination_access, destination.clone(), None) {
            log::warn!(
                "file transfer canceled cleanup failed path={} err={err}",
                destination.display()
            );
        }
    }
    result
}

fn copy_between_file_accesses_inner(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    source: FileNodePath,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
    completed_before: u64,
    total_files: u64,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<FileNodePath, String> {
    check_transfer_canceled(cancel_requested.as_ref())?;
    let info = source_access.info(&source)?;
    match info.kind {
        FileKind::Directory => {
            write_file_access_node(
                destination_access.clone(),
                FileWriteRequest {
                    path: destination.clone(),
                    mode: FileWriteMode::CreateNew,
                    payload: FileWritePayload::Directory,
                    cancel_requested: Some(cancel_requested.clone()),
                },
                completed_before,
                total_files,
                &destination,
                0,
                None,
                progress,
            )?;
            let listings = source_access.list_dirs(std::slice::from_ref(&source))?;
            let Some(listing) = listings.into_iter().next() else {
                return Err(format!("Unable to list {}.", source.display()));
            };
            for child in listing.entries {
                check_transfer_canceled(cancel_requested.as_ref())?;
                let name = file_name_for_transfer(&child)?;
                copy_between_file_accesses(
                    source_access.clone(),
                    destination_access.clone(),
                    child,
                    destination.join_child(name),
                    cancel_requested.clone(),
                    completed_before,
                    total_files,
                    progress,
                )?;
            }
            Ok(destination)
        }
        FileKind::File | FileKind::Archive { .. } => {
            let source_bytes = info.len_or_zero();
            let total_transfer_bytes = source_bytes.saturating_mul(2);
            let read = read_file_access_node(
                source_access,
                FileReadRequest {
                    path: source.clone(),
                    max_bytes: None,
                    cancel_requested: Some(cancel_requested.clone()),
                },
                completed_before,
                total_files,
                &destination,
                0,
                Some(total_transfer_bytes),
                progress,
            )?;
            let bytes = read.into_bytes()?;
            write_file_access_node(
                destination_access,
                FileWriteRequest {
                    path: destination.clone(),
                    mode: FileWriteMode::CreateNew,
                    payload: FileWritePayload::File(bytes),
                    cancel_requested: Some(cancel_requested),
                },
                completed_before,
                total_files,
                &destination,
                source_bytes,
                Some(total_transfer_bytes),
                progress,
            )?;
            Ok(destination)
        }
        FileKind::Symlink | FileKind::Other => Err(format!(
            "Copying {} between different providers is unsupported.",
            source.display()
        )),
    }
}

fn read_file_access_node(
    file_access: Arc<dyn FileAccess>,
    request: FileReadRequest,
    completed_before: u64,
    total_files: u64,
    destination: &FileNodePath,
    byte_offset: u64,
    total_bytes: Option<u64>,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<FileRead, String> {
    let (sender, receiver) = mpsc::channel();
    file_access.read_with_info(
        request,
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    );
    wait_for_file_operation(
        receiver,
        FileOperation::Read,
        completed_before,
        total_files,
        destination,
        byte_offset,
        total_bytes,
        progress,
    )
}

fn write_file_access_node(
    file_access: Arc<dyn FileAccess>,
    request: FileWriteRequest,
    completed_before: u64,
    total_files: u64,
    destination: &FileNodePath,
    byte_offset: u64,
    total_bytes: Option<u64>,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    file_access.write_node(
        request,
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    );
    wait_for_file_operation(
        receiver,
        FileOperation::Write,
        completed_before,
        total_files,
        destination,
        byte_offset,
        total_bytes,
        progress,
    )
}

fn delete_file_access_node(
    file_access: Arc<dyn FileAccess>,
    path: FileNodePath,
    cancel_requested: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    file_access.delete(
        FileDeleteRequest {
            path,
            cancel_requested,
        },
        Box::new(move |event| {
            let _ = sender.send(event);
        }),
    );
    loop {
        match receiver.recv() {
            Ok(FileOperationEvent::Progress(_)) => {}
            Ok(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            Err(_) => return Err("Delete operation did not return a result.".to_string()),
        }
    }
}

fn wait_for_file_operation<T>(
    receiver: mpsc::Receiver<FileOperationEvent<T>>,
    operation: FileOperation,
    completed_before: u64,
    total_files: u64,
    destination: &FileNodePath,
    byte_offset: u64,
    total_bytes: Option<u64>,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<T, String> {
    loop {
        match receiver.recv() {
            Ok(FileOperationEvent::Progress(update)) => {
                progress(transfer_progress_update(
                    update,
                    completed_before,
                    total_files,
                    destination,
                    byte_offset,
                    total_bytes,
                ));
            }
            Ok(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            Err(_) => {
                return Err(format!(
                    "{} operation did not return a result.",
                    operation.label()
                ));
            }
        }
    }
}

fn transfer_progress_update(
    update: FileOperationProgress,
    completed_before: u64,
    total_files: u64,
    destination: &FileNodePath,
    byte_offset: u64,
    total_bytes: Option<u64>,
) -> TransferProgressUpdate {
    TransferProgressUpdate {
        current_path: update.current_path.or_else(|| Some(destination.clone())),
        copied_bytes: byte_offset.saturating_add(update.completed_bytes),
        total_bytes: total_bytes.unwrap_or(update.total_bytes),
        copied_files: completed_before.saturating_add(update.completed_files),
        total_files,
    }
}

fn file_name_for_transfer(path: &FileNodePath) -> Result<String, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "Cannot transfer workspace root.".to_string())?;
    if should_skip(name) {
        return Err("That name is hidden by the file browser.".to_string());
    }
    Ok(name.to_string())
}

fn file_name_for_local_transfer(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Cannot transfer {}.", path.display()))?;
    if should_skip(name) {
        return Err("That name is hidden by the file browser.".to_string());
    }
    Ok(name.to_string())
}

fn file_row_depth(path: &FileNodePath) -> usize {
    let parent = path.parent().unwrap_or_else(|| path.clone());
    let display = parent.display();
    if display.is_empty() {
        0
    } else {
        display
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "!")
            .count()
    }
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

fn shared_file_clipboard() -> Option<FileClipboard> {
    FILE_CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|clipboard| clipboard.clone())
}

fn set_shared_file_clipboard(clipboard: Option<FileClipboard>) {
    if let Ok(mut shared) = FILE_CLIPBOARD.get_or_init(|| Mutex::new(None)).lock() {
        *shared = clipboard;
    }
}

fn shared_drag_clipboard() -> Option<FileClipboard> {
    DRAG_CLIPBOARD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|clipboard| clipboard.clone())
}

fn set_shared_drag_clipboard(clipboard: Option<FileClipboard>) {
    if let Ok(mut shared) = DRAG_CLIPBOARD.get_or_init(|| Mutex::new(None)).lock() {
        *shared = clipboard;
    }
}
