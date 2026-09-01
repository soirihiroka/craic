use super::rows;
use super::{BrowserTarget, FileBrowser, should_skip};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{
    FileAccess, FileCopyRequest, FileDeleteRequest, FileDownloadDestination, FileDownloadRequest,
    FileKind, FileMoveRequest, FileOperation, FileOperationEvent, FileOperationProgress,
    FileOperationReceiver, FileRead, FileReadRequest, FileWriteMode, FileWritePayload,
    FileWriteRequest,
};
use crate::system::capabilities::open::DesktopOpenTargetKind;
use adw::prelude::*;
use craic_ui_core::ui::command_mailbox;
use gtk::{gdk, gio};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
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

include!("transfer/actions.rs");

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
    retry: Option<TransferRetry>,
}

#[derive(Clone)]
struct WorkspaceTransferRetry {
    clipboard: FileClipboard,
    target_folder: FileNodePath,
    allow_sudo: bool,
    destination_access: Arc<dyn FileAccess>,
    sudo_destination: bool,
}

#[derive(Clone)]
enum TransferRetry {
    Local {
        sources: Vec<PathBuf>,
        target_folder: FileNodePath,
        allow_sudo: bool,
    },
    Workspace(WorkspaceTransferRetry),
}

impl TransferRetry {
    fn allow_sudo(&self) -> bool {
        match self {
            Self::Local { allow_sudo, .. } => *allow_sudo,
            Self::Workspace(retry) => retry.allow_sudo,
        }
    }
}

impl ActiveTransfer {
    fn new(
        operation: TransferOperation,
        total_files: u64,
        auto_focus: bool,
        cancel_requested: Arc<AtomicBool>,
        retry: Option<TransferRetry>,
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
            retry,
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
    let mut receiver = file_access.write_node_events(request);
    let mut last_completed_bytes = 0u64;
    loop {
        match receiver.blocking_recv() {
            Some(FileOperationEvent::Progress(update)) => {
                let delta = update.completed_bytes.saturating_sub(last_completed_bytes);
                last_completed_bytes = update.completed_bytes;
                local_progress.add_bytes(delta);
                progress(local_progress.to_update(display_path));
            }
            Some(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            None => return Err("write operation did not return a result.".to_string()),
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
    if operation == TransferOperation::Copy
        && source_access.supports_download()
        && let Some(local_folder) = destination_access.local_path(&target_folder)
    {
        let mut destinations = Vec::with_capacity(sources.len());
        for source in &sources {
            check_transfer_canceled(cancel_requested.as_ref())?;
            let name = file_name_for_transfer(source)?;
            let destination = target_folder.join_child(name);
            if destination_access.info(&destination).is_ok() {
                return Err(format!("{} already exists.", destination.display()));
            }
            destinations.push(destination);
        }
        if let Some(destination) = destinations.first() {
            progress(TransferProgressUpdate {
                current_path: Some(destination.clone()),
                copied_bytes: 0,
                total_bytes: 0,
                copied_files: 0,
                total_files: sources.len() as u64,
            });
        }
        source_access.download_to_local(FileDownloadRequest {
            sources,
            destination: FileDownloadDestination::Folder(local_folder),
            cancel_requested: Some(cancel_requested),
        })?;
        if let Some(destination) = destinations.last() {
            progress(TransferProgressUpdate {
                current_path: Some(destination.clone()),
                copied_bytes: 0,
                total_bytes: 0,
                copied_files: destinations.len() as u64,
                total_files: destinations.len() as u64,
            });
        }
        return Ok(destinations);
    }

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
                    destination_access.clone(),
                    source.clone(),
                    destination.clone(),
                    cancel_requested.clone(),
                    completed_before,
                    total_files,
                    progress,
                )?;
                if let Err(err) =
                    delete_file_access_node(source_access, source, Some(cancel_requested))
                {
                    if let Err(cleanup_err) =
                        delete_file_access_node(destination_access, destination.clone(), None)
                    {
                        log::warn!(
                            "cross-provider move rollback failed path={} err={cleanup_err}",
                            destination.display()
                        );
                    }
                    return Err(err);
                }
                Ok(destination)
            }
        };
    }

    let receiver = match operation {
        TransferOperation::Copy => destination_access.copy_node_events(FileCopyRequest {
            source: source.clone(),
            destination: destination.clone(),
            cancel_requested: Some(cancel_requested.clone()),
        }),
        TransferOperation::Move => destination_access.move_node_events(FileMoveRequest {
            source: source.clone(),
            destination_parent: target_folder,
            new_name: name,
            cancel_requested: Some(cancel_requested.clone()),
        }),
    };
    let mut receiver = receiver;

    loop {
        match receiver.blocking_recv() {
            Some(FileOperationEvent::Progress(update)) => {
                progress(transfer_progress_update(
                    update,
                    completed_before,
                    total_files,
                    &destination,
                    0,
                    None,
                ));
            }
            Some(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            None => {
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
            let copy_children = (|| {
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
                Ok(())
            })();
            if let Err(err) = copy_children {
                if let Err(cleanup_err) =
                    delete_file_access_node(destination_access, destination.clone(), None)
                {
                    log::warn!(
                        "cross-provider directory rollback failed path={} err={cleanup_err}",
                        destination.display()
                    );
                }
                return Err(err);
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
    let receiver = file_access.read_with_info_events(request);
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
    let receiver = file_access.write_node_events(request);
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
    let mut receiver = file_access.delete_events(FileDeleteRequest {
        path,
        cancel_requested,
    });
    loop {
        match receiver.blocking_recv() {
            Some(FileOperationEvent::Progress(_)) => {}
            Some(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            None => return Err("Delete operation did not return a result.".to_string()),
        }
    }
}

fn wait_for_file_operation<T>(
    mut receiver: FileOperationReceiver<T>,
    operation: FileOperation,
    completed_before: u64,
    total_files: u64,
    destination: &FileNodePath,
    byte_offset: u64,
    total_bytes: Option<u64>,
    progress: &mut impl FnMut(TransferProgressUpdate),
) -> Result<T, String> {
    loop {
        match receiver.blocking_recv() {
            Some(FileOperationEvent::Progress(update)) => {
                progress(transfer_progress_update(
                    update,
                    completed_before,
                    total_files,
                    destination,
                    byte_offset,
                    total_bytes,
                ));
            }
            Some(FileOperationEvent::Finished(result)) => {
                return result.map_err(|err| err.to_string());
            }
            None => {
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
