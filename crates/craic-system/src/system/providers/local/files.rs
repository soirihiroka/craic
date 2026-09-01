use crate::system::capabilities::files::{
    DirectoryListing, FILE_OPERATION_EVENT_CAPACITY, FILE_WATCH_EVENT_CAPACITY, FileAccess,
    FileCopyRequest, FileDeleteRequest, FileKind, FileMoveRequest, FileNodeCapabilities,
    FileNodeInfo, FileOperation, FileOperationEmitter, FileOperationError, FileOperationErrorKind,
    FileOperationEvent, FileOperationProgress, FileOperationReceiver, FileRead, FileReadRequest,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileSudoError,
    FileSudoErrorKind, FileSudoPassword, FileWatchChanges, FileWatchReceiver, FileWatchRequest,
    FileWatchSubscription, FileWriteMode, FileWritePayload, FileWriteRequest,
    file_operation_canceled,
};
use crate::system::path::{
    ArchiveFormat, FileNodePath, FileNodeRef, SystemRef, WorkspacePath, WorkspaceRef,
};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::CString;
use std::fs;
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};
use walkdir::WalkDir;
use zeroize::Zeroizing;

const LOCAL_FILE_MONITOR_RATE_LIMIT: Duration = Duration::from_millis(250);
const LOCAL_FILE_MONITOR_SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(25);
const LOCAL_FILE_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(750);
const LOCAL_ARCHIVE_PYTHON_CANDIDATES: &[&str] = &["python3", "python"];
const LOCAL_FILE_OPERATION_CHUNK_BYTES: usize = 256 * 1024;
const FINALIZE_STAGED_PATH_SCRIPT: &str = include_str!("../finalize_staged_path.sh");

#[derive(Clone, Debug)]
pub struct LocalFileAccess {
    system: SystemRef,
    workspace: WorkspaceRef,
    root_path: PathBuf,
    file_watch_service: Arc<LocalFileWatchService>,
    sudo: bool,
}

#[derive(Clone, Debug)]
struct ArchiveTarget {
    archive_node: FileNodePath,
    archive_path: PathBuf,
    format: ArchiveFormat,
    member: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ArchiveListOutput {
    members: Vec<ArchiveMember>,
    invalid: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct ArchiveMember {
    name: String,
    kind: String,
    len: Option<u64>,
    modified: Option<f64>,
    mode: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct ArchiveTree {
    directories: HashMap<String, ArchiveTreeDirectory>,
}

#[derive(Clone, Debug, Default)]
struct ArchiveTreeDirectory {
    children: HashSet<String>,
}

impl ArchiveTree {
    fn from_members(members: &[ArchiveMember]) -> Self {
        let mut tree = Self::default();
        tree.directories.entry(String::new()).or_default();

        for member in members {
            let name = member.name.trim_end_matches('/');
            if name.is_empty() {
                continue;
            }
            let parts = name.split('/').collect::<Vec<_>>();
            for index in 0..parts.len() {
                let parent = parts[..index].join("/");
                tree.directories
                    .entry(parent)
                    .or_default()
                    .children
                    .insert(parts[index].to_string());

                if index + 1 < parts.len() || member.kind == "dir" {
                    let directory = parts[..=index].join("/");
                    tree.directories.entry(directory).or_default();
                }
            }
        }

        tree
    }

    fn contains_dir(&self, path: &str) -> bool {
        self.directories.contains_key(path)
    }

    fn child_names(&self, path: &str) -> Vec<String> {
        let mut names = self
            .directories
            .get(path)
            .map(|dir| dir.children.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        names.sort();
        names
    }
}

include!("files/access.rs");

fn finalize_local_staged_path(
    source: &Path,
    destination: &Path,
    cancel_requested: &AtomicBool,
) -> Result<(), String> {
    if cancel_requested.load(Ordering::Relaxed) {
        return Err("Operation canceled.".to_string());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "The staged path contains a null byte.".to_string())?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "The destination path contains a null byte.".to_string())?;

    #[cfg(target_os = "macos")]
    let result = unsafe {
        unsafe extern "C" {
            fn renameatx_np(
                fromfd: libc::c_int,
                from: *const libc::c_char,
                tofd: libc::c_int,
                to: *const libc::c_char,
                flags: libc::c_uint,
            ) -> libc::c_int;
        }
        renameatx_np(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            0x0000_0004,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    return Err("Atomic no-replace file finalization is unavailable on this platform.".to_string());
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "Unable to finalize without replacing an existing item: {}",
            std::io::Error::last_os_error()
        ))
    }
}

impl FileAccess for LocalFileAccess {
    fn workspace(&self) -> WorkspaceRef {
        self.workspace.clone()
    }

    fn root(&self) -> FileNodePath {
        self.root_node()
    }

    fn sudo_access(
        &self,
        password: Option<FileSudoPassword>,
    ) -> Result<Arc<dyn FileAccess>, FileSudoError> {
        log::info!(
            "local sudo file authorization start workspace={} password_supplied={}",
            self.workspace.display_name,
            password.is_some()
        );
        Self::authenticate_sudo(password.as_ref())?;
        let mut access = self.clone();
        access.sudo = true;
        log::info!(
            "local sudo file authorization complete workspace={}",
            self.workspace.display_name
        );
        Ok(Arc::new(access))
    }

    fn local_path(&self, path: &FileNodePath) -> Option<PathBuf> {
        (!self.sudo)
            .then(|| self.local_path_for_node(path).ok())
            .flatten()
    }

    fn finalize_staged_node(
        &self,
        source: &FileNodePath,
        destination: &FileNodePath,
        cancel_requested: &AtomicBool,
    ) -> Result<FileNodePath, String> {
        if cancel_requested.load(Ordering::Relaxed) {
            return Err("Operation canceled.".to_string());
        }
        let source_path = self.local_path_for_node(source)?;
        let destination_path = self.local_path_for_node(destination)?;
        if self.sudo {
            self.run_sudo_command(
                "finalize staged path",
                "sh",
                &[
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new(FINALIZE_STAGED_PATH_SCRIPT),
                    std::ffi::OsStr::new("sh"),
                    source_path.as_os_str(),
                    destination_path.as_os_str(),
                ],
                None,
            )?;
        } else {
            finalize_local_staged_path(&source_path, &destination_path, cancel_requested)?;
        }
        Ok(destination.clone())
    }

    fn watch(
        &self,
        request: FileWatchRequest,
    ) -> Result<(FileWatchSubscription, FileWatchReceiver), String> {
        let requested_paths = if request.paths.is_empty() {
            vec![self.root_node()]
        } else {
            request.paths.clone()
        };
        if requested_paths.iter().any(|path| !path.is_native()) {
            return Err("Watching archive contents is unsupported.".to_string());
        }
        let local_paths = requested_paths
            .iter()
            .map(|path| {
                self.local_path_for_node(path)
                    .map(|local_path| (path.clone(), local_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let label = if requested_paths.len() == 1 {
            format!(
                "local-file:{}:{}",
                self.workspace.display_name,
                requested_paths[0].display()
            )
        } else {
            format!(
                "local-file:{}:{}paths",
                self.workspace.display_name,
                requested_paths.len()
            )
        };
        log::info!(
            "local file watch requested workspace={} paths={} mode=shared-notify",
            self.workspace.display_name,
            local_paths.len()
        );
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_WATCH_EVENT_CAPACITY);
        let subscription = self.file_watch_service.register(
            label,
            local_paths,
            self.root_path.clone(),
            self.system.clone(),
            self.workspace.clone(),
            sender,
        )?;
        Ok((subscription, receiver))
    }

    fn info(&self, path: &FileNodePath) -> Result<FileNodeInfo, String> {
        log::trace!(
            "local file node info workspace={} path={}",
            self.workspace.display_name,
            path.display()
        );
        if path.contains_archive() {
            self.info_for_archive_node(path)
        } else {
            self.info_for_native_node(path)
        }
    }

    fn info_many(&self, paths: &[FileNodePath]) -> Result<Vec<FileNodeInfo>, String> {
        let infos = paths
            .iter()
            .map(|path| self.info(path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(infos)
    }

    fn list_dirs(&self, paths: &[FileNodePath]) -> Result<Vec<DirectoryListing>, String> {
        let mut listings = Vec::new();
        for path in paths {
            if path.contains_archive() {
                listings.push(self.list_archive_dir(path)?);
                continue;
            }
            let info = self.info_for_native_node(path)?;
            if let FileKind::Archive { format } = info.kind {
                if !archive_format_supported(format) {
                    return Err(format!(
                        "{} archive browsing is unsupported on this system.",
                        format
                    ));
                }
                let archive_root = path.open_archive(format);
                log::info!(
                    "local native archive listing contents workspace={} archive={} root={}",
                    self.workspace.display_name,
                    path.display(),
                    archive_root.display()
                );
                let mut listing = self.list_archive_dir(&archive_root)?;
                listing.path = path.clone();
                listings.push(listing);
                continue;
            }
            let local_path = self.local_path_for_node(path)?;
            let mut entries = Vec::new();
            if self.sudo {
                let output = self.run_sudo_command(
                    "list directory",
                    "find",
                    &[
                        local_path.as_os_str(),
                        std::ffi::OsStr::new("-mindepth"),
                        std::ffi::OsStr::new("1"),
                        std::ffi::OsStr::new("-maxdepth"),
                        std::ffi::OsStr::new("1"),
                        std::ffi::OsStr::new("-print0"),
                    ],
                    None,
                )?;
                for raw_path in output.stdout.split(|byte| *byte == 0) {
                    if !raw_path.is_empty() {
                        entries.push(self.node_path_for_local(&PathBuf::from(
                            std::ffi::OsString::from_vec(raw_path.to_vec()),
                        )));
                    }
                }
                listings.push(DirectoryListing {
                    path: path.clone(),
                    entries,
                });
                continue;
            }
            for entry in fs::read_dir(&local_path)
                .map_err(|err| format!("Unable to list {}: {err}", path.display()))?
            {
                let entry =
                    entry.map_err(|err| format!("Unable to read directory entry: {err}"))?;
                entries.push(self.node_path_for_local(&entry.path()));
            }
            listings.push(DirectoryListing {
                path: path.clone(),
                entries,
            });
        }
        Ok(listings)
    }

    fn read_with_info_events(&self, request: FileReadRequest) -> FileOperationReceiver<FileRead> {
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
        let access = self.clone();
        thread::spawn(move || {
            let callback = move |event| {
                let _ = sender.blocking_send(event);
            };
            log::info!(
                "local file read worker start path={} max_bytes={:?}",
                request.path.display(),
                request.max_bytes
            );
            let result = access.perform_read_with_info(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
        receiver
    }

    fn write_node_events(&self, request: FileWriteRequest) -> FileOperationReceiver<()> {
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
        let access = self.clone();
        thread::spawn(move || {
            let callback = move |event| {
                let _ = sender.blocking_send(event);
            };
            let payload_label = match &request.payload {
                FileWritePayload::File(contents) => format!("file bytes={}", contents.len()),
                FileWritePayload::Directory => "directory".to_string(),
            };
            log::info!(
                "local file write worker start path={} payload={}",
                request.path.display(),
                payload_label
            );
            let result = access.perform_write_node(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
        receiver
    }

    fn copy_node_events(&self, request: FileCopyRequest) -> FileOperationReceiver<FileNodePath> {
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
        let access = self.clone();
        thread::spawn(move || {
            let callback = move |event| {
                let _ = sender.blocking_send(event);
            };
            log::info!(
                "local file copy worker start source={} destination={}",
                request.source.display(),
                request.destination.display()
            );
            let result = access.perform_copy_node(&request, FileOperation::Copy, &callback);
            callback(FileOperationEvent::Finished(result));
        });
        receiver
    }

    fn move_node_events(&self, request: FileMoveRequest) -> FileOperationReceiver<FileNodePath> {
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
        let access = self.clone();
        thread::spawn(move || {
            let callback = move |event| {
                let _ = sender.blocking_send(event);
            };
            log::info!(
                "local file move worker start source={} destination_parent={} new_name={}",
                request.source.display(),
                request.destination_parent.display(),
                request.new_name
            );
            let result = access.perform_move_node(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
        receiver
    }

    fn delete_events(&self, request: FileDeleteRequest) -> FileOperationReceiver<()> {
        let (sender, receiver) = tokio::sync::mpsc::channel(FILE_OPERATION_EVENT_CAPACITY);
        let access = self.clone();
        thread::spawn(move || {
            let callback = move |event| {
                let _ = sender.blocking_send(event);
            };
            log::info!(
                "local file delete worker start path={}",
                request.path.display()
            );
            let result = access.perform_delete(&request, Some(&callback));
            callback(FileOperationEvent::Finished(result));
        });
        receiver
    }

    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String> {
        if !query.root.is_native() {
            return Err("Searching archive contents is unsupported.".to_string());
        }
        let root = self.local_path_for_node(&query.root)?;
        let matcher = build_search_regex(&query)?;
        let excluded_names = query.excluded_names.clone();
        let mut text_matches = Vec::new();
        let mut file_name_matches = Vec::new();
        let mut limited = false;

        log::info!(
            "file search start provider=local workspace={} query_len={} root={}",
            self.workspace.display_name,
            query.query.len(),
            query.root.display()
        );

        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || entry
                        .file_name()
                        .to_str()
                        .is_none_or(|name| !excluded_names.iter().any(|excluded| excluded == name))
            })
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = self.node_path_for_local(entry.path());
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| matcher.find_iter(name).any(|found| !found.is_empty()))
            {
                if text_matches.len() + file_name_matches.len() >= query.max_results {
                    limited = true;
                    break;
                }
                file_name_matches.push(path.clone());
                if text_matches.len() + file_name_matches.len() >= query.max_results {
                    limited = true;
                    break;
                }
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() > query.max_file_bytes {
                continue;
            }
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            collect_file_matches(
                &path,
                &text,
                &matcher,
                &query,
                file_name_matches.len(),
                &mut text_matches,
                &mut limited,
            );
            if limited {
                break;
            }
        }

        text_matches.sort_by(|left, right| {
            left.path
                .display()
                .cmp(&right.path.display())
                .then_with(|| left.start.cmp(&right.start))
        });
        file_name_matches.sort_by_key(FileNodePath::display);
        log::info!(
            "file search complete provider=local workspace={} text_matches={} file_name_matches={} limited={}",
            self.workspace.display_name,
            text_matches.len(),
            file_name_matches.len(),
            limited
        );
        Ok(FileSearchOutput {
            text_matches,
            file_name_matches,
            limited,
        })
    }
}

fn local_file_signature(path: &Path) -> Result<Option<FileSignature>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(FileSignature {
            kind: file_kind(&metadata),
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "Unable to inspect watched path {}: {err}",
            path.display()
        )),
    }
}

#[derive(Debug)]
pub(super) struct LocalFileWatchService {
    command_sender: mpsc::Sender<LocalFileWatchCommand>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

static NEXT_LOCAL_FILE_WATCH_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);

enum LocalFileWatchCommand {
    Register {
        id: u64,
        label: String,
        local_paths: Vec<(FileNodePath, PathBuf)>,
        root_path: PathBuf,
        system: SystemRef,
        workspace: WorkspaceRef,
        sender: tokio::sync::mpsc::Sender<FileWatchChanges>,
        response: mpsc::Sender<Result<(), String>>,
    },
    Unregister {
        id: u64,
    },
    Shutdown,
}

struct LocalFileWatchRegistration {
    label: String,
    root_path: PathBuf,
    system: SystemRef,
    workspace: WorkspaceRef,
    sender: tokio::sync::mpsc::Sender<FileWatchChanges>,
    monitored_paths: Vec<PathBuf>,
    fallback_paths: Vec<(FileNodePath, PathBuf)>,
    fallback_signatures: HashMap<FileNodePath, Option<FileSignature>>,
    next_fallback_poll: Instant,
    pending_changes: FileWatchChanges,
    next_delivery: Option<Instant>,
    started_at: Instant,
    raw_events: u64,
    delivered_batches: u64,
    changed_paths: u64,
}

struct SharedLocalMonitor {
    _watcher: RecommendedWatcher,
    directory: bool,
    registrations: HashSet<u64>,
}

struct LocalFileMonitorEvent {
    watched_path: PathBuf,
    result: notify::Result<Event>,
}

impl LocalFileWatchService {
    pub(super) fn new() -> Arc<Self> {
        let (command_sender, command_receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("craic-local-file-watch".to_string())
            .spawn(move || run_local_file_watch_service(command_receiver))
            .expect("unable to start local file watch service");
        log::info!("local file watch service started");
        Arc::new(Self {
            command_sender,
            thread: Mutex::new(Some(thread)),
        })
    }

    fn register(
        &self,
        label: String,
        local_paths: Vec<(FileNodePath, PathBuf)>,
        root_path: PathBuf,
        system: SystemRef,
        workspace: WorkspaceRef,
        sender: tokio::sync::mpsc::Sender<FileWatchChanges>,
    ) -> Result<FileWatchSubscription, String> {
        let id = NEXT_LOCAL_FILE_WATCH_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed);
        let (response, result) = mpsc::channel();
        self.command_sender
            .send(LocalFileWatchCommand::Register {
                id,
                label: label.clone(),
                local_paths,
                root_path,
                system,
                workspace,
                sender,
                response,
            })
            .map_err(|_| "Local file watch service is unavailable.".to_string())?;
        match result.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = self
                    .command_sender
                    .send(LocalFileWatchCommand::Unregister { id });
                return Err(err);
            }
            Err(_) => {
                let _ = self
                    .command_sender
                    .send(LocalFileWatchCommand::Unregister { id });
                return Err("Local file watch registration timed out.".to_string());
            }
        }

        let command_sender = self.command_sender.clone();
        Ok(FileWatchSubscription::new(label, move || {
            let _ = command_sender.send(LocalFileWatchCommand::Unregister { id });
        }))
    }
}

impl Drop for LocalFileWatchService {
    fn drop(&mut self) {
        let _ = self.command_sender.send(LocalFileWatchCommand::Shutdown);
        if let Ok(mut thread) = self.thread.lock()
            && let Some(thread) = thread.take()
            && thread.join().is_err()
        {
            log::warn!("local file watch service join failed");
        }
        log::info!("local file watch service stopped");
    }
}

fn run_local_file_watch_service(command_receiver: mpsc::Receiver<LocalFileWatchCommand>) {
    let (event_sender, event_receiver) = mpsc::channel();
    let mut registrations = HashMap::new();
    let mut monitors = HashMap::new();
    let mut running = true;

    while running {
        while let Ok(command) = command_receiver.try_recv() {
            running = handle_local_file_watch_command(
                command,
                &event_sender,
                &mut registrations,
                &mut monitors,
            );
            if !running {
                break;
            }
        }
        if !running {
            break;
        }

        while let Ok(event) = event_receiver.try_recv() {
            dispatch_local_file_monitor_event(event, &monitors, &mut registrations);
        }
        flush_local_file_watch_changes(&mut registrations);
        poll_local_file_watch_fallbacks(&mut registrations);

        match command_receiver.recv_timeout(LOCAL_FILE_MONITOR_SERVICE_POLL_INTERVAL) {
            Ok(command) => {
                running = handle_local_file_watch_command(
                    command,
                    &event_sender,
                    &mut registrations,
                    &mut monitors,
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let ids = registrations.keys().copied().collect::<Vec<_>>();
    for id in ids {
        unregister_local_file_watch(id, &mut registrations, &mut monitors);
    }
}

fn handle_local_file_watch_command(
    command: LocalFileWatchCommand,
    event_sender: &mpsc::Sender<LocalFileMonitorEvent>,
    registrations: &mut HashMap<u64, LocalFileWatchRegistration>,
    monitors: &mut HashMap<PathBuf, SharedLocalMonitor>,
) -> bool {
    match command {
        LocalFileWatchCommand::Register {
            id,
            label,
            local_paths,
            root_path,
            system,
            workspace,
            sender,
            response,
        } => {
            let result = register_local_file_watch(
                id,
                label,
                local_paths,
                root_path,
                system,
                workspace,
                sender,
                event_sender,
                registrations,
                monitors,
            );
            let _ = response.send(result);
            true
        }
        LocalFileWatchCommand::Unregister { id } => {
            unregister_local_file_watch(id, registrations, monitors);
            true
        }
        LocalFileWatchCommand::Shutdown => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn register_local_file_watch(
    id: u64,
    label: String,
    local_paths: Vec<(FileNodePath, PathBuf)>,
    root_path: PathBuf,
    system: SystemRef,
    workspace: WorkspaceRef,
    sender: tokio::sync::mpsc::Sender<FileWatchChanges>,
    event_sender: &mpsc::Sender<LocalFileMonitorEvent>,
    registrations: &mut HashMap<u64, LocalFileWatchRegistration>,
    monitors: &mut HashMap<PathBuf, SharedLocalMonitor>,
) -> Result<(), String> {
    let mut monitored_paths = Vec::new();
    let mut fallback_paths = Vec::new();

    for (node_path, local_path) in local_paths {
        if let Some(shared) = monitors.get_mut(&local_path) {
            shared.registrations.insert(id);
            monitored_paths.push(local_path);
            continue;
        }

        let directory = local_path.is_dir();
        let watch_root = if directory {
            local_path.as_path()
        } else {
            local_path.parent().unwrap_or(local_path.as_path())
        };
        let watched_path = local_path.clone();
        let sender = event_sender.clone();
        let monitor_result = notify::recommended_watcher(move |result| {
            let _ = sender.send(LocalFileMonitorEvent {
                watched_path: watched_path.clone(),
                result,
            });
        })
        .and_then(|mut watcher| {
            watcher.watch(watch_root, RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });
        match monitor_result {
            Ok(watcher) => {
                monitors.insert(
                    local_path.clone(),
                    SharedLocalMonitor {
                        _watcher: watcher,
                        directory,
                        registrations: HashSet::from([id]),
                    },
                );
                monitored_paths.push(local_path);
            }
            Err(err) => {
                log::warn!(
                    "local notify file watch unavailable path={} watch_root={} err={err}; using shared polling fallback",
                    local_path.display(),
                    watch_root.display(),
                );
                fallback_paths.push((node_path, local_path));
            }
        }
    }

    let fallback_signatures = local_file_watch_signatures(&fallback_paths);
    log::info!(
        "local file watch registered id={} label={} notify_paths={} fallback_paths={} shared_monitors={}",
        id,
        label,
        monitored_paths.len(),
        fallback_paths.len(),
        monitors.len()
    );
    registrations.insert(
        id,
        LocalFileWatchRegistration {
            label,
            root_path,
            system,
            workspace,
            sender,
            monitored_paths,
            fallback_paths,
            fallback_signatures,
            next_fallback_poll: Instant::now() + LOCAL_FILE_FALLBACK_POLL_INTERVAL,
            pending_changes: FileWatchChanges::new(),
            next_delivery: None,
            started_at: Instant::now(),
            raw_events: 0,
            delivered_batches: 0,
            changed_paths: 0,
        },
    );
    Ok(())
}

fn unregister_local_file_watch(
    id: u64,
    registrations: &mut HashMap<u64, LocalFileWatchRegistration>,
    monitors: &mut HashMap<PathBuf, SharedLocalMonitor>,
) {
    let Some(registration) = registrations.remove(&id) else {
        return;
    };
    for path in &registration.monitored_paths {
        let remove_monitor = monitors.get_mut(path).is_some_and(|monitor| {
            monitor.registrations.remove(&id);
            monitor.registrations.is_empty()
        });
        if remove_monitor {
            monitors.remove(path);
        }
    }
    log::info!(
        "local file watch unregistered id={} label={} lifetime_ms={} raw_events={} delivered_batches={} changed_paths={} shared_monitors={}",
        id,
        registration.label,
        registration.started_at.elapsed().as_millis(),
        registration.raw_events,
        registration.delivered_batches,
        registration.changed_paths,
        monitors.len()
    );
}

fn dispatch_local_file_monitor_event(
    event: LocalFileMonitorEvent,
    monitors: &HashMap<PathBuf, SharedLocalMonitor>,
    registrations: &mut HashMap<u64, LocalFileWatchRegistration>,
) {
    let Some(monitor) = monitors.get(&event.watched_path) else {
        return;
    };
    let native_event = match event.result {
        Ok(event) if local_file_monitor_event_should_notify(event.kind) => event,
        Ok(_) => return,
        Err(err) => {
            log::warn!(
                "local notify file watch event failed path={} err={err}",
                event.watched_path.display()
            );
            return;
        }
    };
    let registration_ids = monitor.registrations.iter().copied().collect::<Vec<_>>();
    for id in registration_ids {
        let Some(registration) = registrations.get_mut(&id) else {
            continue;
        };
        registration.raw_events += 1;
        let changes = local_file_monitor_path_changes(
            &registration.root_path,
            &registration.system,
            &registration.workspace,
            &event.watched_path,
            monitor.directory,
            &native_event.paths,
        );
        if changes.is_empty() {
            continue;
        }
        registration.pending_changes.extend(changes);
        registration
            .next_delivery
            .get_or_insert_with(|| Instant::now() + LOCAL_FILE_MONITOR_RATE_LIMIT);
    }
}

fn flush_local_file_watch_changes(registrations: &mut HashMap<u64, LocalFileWatchRegistration>) {
    let now = Instant::now();
    for registration in registrations.values_mut() {
        if registration
            .next_delivery
            .is_none_or(|deadline| now < deadline)
        {
            continue;
        }
        registration.next_delivery = None;
        let changes = std::mem::take(&mut registration.pending_changes);
        if changes.is_empty() {
            continue;
        }
        registration.delivered_batches += 1;
        registration.changed_paths += changes.len() as u64;
        let _ = registration.sender.blocking_send(changes);
    }
}

fn poll_local_file_watch_fallbacks(registrations: &mut HashMap<u64, LocalFileWatchRegistration>) {
    let now = Instant::now();
    for registration in registrations.values_mut() {
        if registration.fallback_paths.is_empty() || now < registration.next_fallback_poll {
            continue;
        }
        registration.next_fallback_poll = now + LOCAL_FILE_FALLBACK_POLL_INTERVAL;
        let next_signatures = local_file_watch_signatures(&registration.fallback_paths);
        let changes = changed_signature_paths(&registration.fallback_signatures, &next_signatures);
        registration.fallback_signatures = next_signatures;
        if changes.is_empty() {
            continue;
        }
        registration.delivered_batches += 1;
        registration.changed_paths += changes.len() as u64;
        let _ = registration.sender.blocking_send(changes);
    }
}

fn local_file_watch_signatures(
    local_paths: &[(FileNodePath, PathBuf)],
) -> HashMap<FileNodePath, Option<FileSignature>> {
    local_paths
        .iter()
        .map(|(node_path, local_path)| {
            let signature = match local_file_signature(local_path) {
                Ok(signature) => signature,
                Err(err) => {
                    log::warn!(
                        "local file watch fallback metadata failed path={} err={err}",
                        local_path.display()
                    );
                    None
                }
            };
            (node_path.clone(), signature)
        })
        .collect()
}

fn changed_signature_paths(
    previous: &HashMap<FileNodePath, Option<FileSignature>>,
    next: &HashMap<FileNodePath, Option<FileSignature>>,
) -> FileWatchChanges {
    let mut changes = FileWatchChanges::new();
    for (path, next_signature) in next {
        if previous.get(path) != Some(next_signature) {
            changes.insert(path.clone());
        }
    }
    for path in previous.keys() {
        if !next.contains_key(path) {
            changes.insert(path.clone());
        }
    }
    changes
}

fn local_file_monitor_event_should_notify(event_type: EventKind) -> bool {
    !matches!(
        event_type,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
    )
}

fn local_file_monitor_path_changes(
    root_path: &Path,
    system: &SystemRef,
    workspace: &WorkspaceRef,
    watched_path: &Path,
    directory: bool,
    event_paths: &[PathBuf],
) -> FileWatchChanges {
    let mut changes = FileWatchChanges::new();
    for path in event_paths {
        let matches_watch = if directory {
            path.parent() == Some(watched_path)
        } else {
            path == watched_path
        };
        if matches_watch {
            collect_local_file_monitor_path(&mut changes, root_path, system, workspace, path);
        }
    }
    changes
}

fn collect_local_file_monitor_path(
    changes: &mut FileWatchChanges,
    root_path: &Path,
    system: &SystemRef,
    workspace: &WorkspaceRef,
    path: &Path,
) {
    if let Some(node_path) = file_node_path_for_local_root(root_path, system, workspace, path) {
        changes.insert(node_path);
    }
}

fn file_node_path_for_local_root(
    root_path: &Path,
    system: &SystemRef,
    workspace: &WorkspaceRef,
    path: &Path,
) -> Option<FileNodePath> {
    let relative = path
        .strip_prefix(root_path)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some(workspace.node_path(system, &relative))
}

#[derive(Clone, Copy, Debug, Default)]
struct LocalCopyTotals {
    bytes: u64,
    files: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct LocalCopyProgress {
    completed_bytes: u64,
    completed_files: u64,
    total_bytes: u64,
    total_files: u64,
}

impl LocalCopyProgress {
    fn to_event(
        self,
        operation: FileOperation,
        source: &FileNodePath,
        destination: &FileNodePath,
        current_path: &FileNodePath,
    ) -> FileOperationProgress {
        FileOperationProgress {
            operation,
            source: Some(source.clone()),
            destination: Some(destination.clone()),
            current_path: Some(current_path.clone()),
            completed_bytes: self.completed_bytes,
            total_bytes: self.total_bytes,
            completed_files: self.completed_files,
            total_files: self.total_files,
        }
    }
}

fn local_copy_totals(
    source_path: &Path,
    source: &FileNodePath,
    operation: FileOperation,
    destination: &FileNodePath,
) -> Result<LocalCopyTotals, FileOperationError> {
    let metadata = fs::symlink_metadata(source_path).map_err(|err| {
        LocalFileAccess::io_error(
            operation,
            Some(source.clone()),
            Some(destination.clone()),
            &format!("Unable to inspect {}", source.display()),
            err,
        )
    })?;
    if metadata.is_file() {
        return Ok(LocalCopyTotals {
            bytes: metadata.len(),
            files: 1,
        });
    }
    if metadata.file_type().is_symlink() {
        return Ok(LocalCopyTotals { bytes: 0, files: 1 });
    }
    if !metadata.is_dir() {
        return Err(LocalFileAccess::operation_error(
            operation,
            FileOperationErrorKind::Unsupported,
            Some(source.clone()),
            Some(destination.clone()),
            "Only files, folders, and symlinks can be copied.",
        ));
    }

    let mut totals = LocalCopyTotals { bytes: 0, files: 1 };
    for entry in fs::read_dir(source_path).map_err(|err| {
        LocalFileAccess::io_error(
            operation,
            Some(source.clone()),
            Some(destination.clone()),
            &format!("Unable to list {}", source.display()),
            err,
        )
    })? {
        let entry = entry.map_err(|err| {
            LocalFileAccess::io_error(
                operation,
                Some(source.clone()),
                Some(destination.clone()),
                "Unable to read directory entry",
                err,
            )
        })?;
        let child_name = entry.file_name().to_string_lossy().to_string();
        let child_source = source.join_child(&child_name);
        let child_destination = destination.join_child(&child_name);
        let child_totals =
            local_copy_totals(&entry.path(), &child_source, operation, &child_destination)?;
        totals.bytes = totals.bytes.saturating_add(child_totals.bytes);
        totals.files = totals.files.saturating_add(child_totals.files);
    }
    Ok(totals)
}

#[cfg(unix)]
fn copy_local_symlink(
    source_path: &Path,
    destination_path: &Path,
    operation: FileOperation,
    source: &FileNodePath,
    destination: &FileNodePath,
) -> Result<(), FileOperationError> {
    let target = fs::read_link(source_path).map_err(|err| {
        LocalFileAccess::io_error(
            operation,
            Some(source.clone()),
            Some(destination.clone()),
            &format!("Unable to read symlink {}", source.display()),
            err,
        )
    })?;
    std::os::unix::fs::symlink(&target, destination_path).map_err(|err| {
        LocalFileAccess::io_error(
            operation,
            Some(source.clone()),
            Some(destination.clone()),
            &format!("Unable to copy symlink {}", source.display()),
            err,
        )
    })
}

#[cfg(not(unix))]
fn copy_local_symlink(
    _source_path: &Path,
    _destination_path: &Path,
    operation: FileOperation,
    source: &FileNodePath,
    destination: &FileNodePath,
) -> Result<(), FileOperationError> {
    Err(LocalFileAccess::operation_error(
        operation,
        FileOperationErrorKind::Unsupported,
        Some(source.clone()),
        Some(destination.clone()),
        "Copying symlinks is unsupported on this platform.",
    ))
}

fn local_io_error_kind(err: &std::io::Error) -> FileOperationErrorKind {
    match err.kind() {
        ErrorKind::NotFound => FileOperationErrorKind::NotFound,
        ErrorKind::AlreadyExists => FileOperationErrorKind::AlreadyExists,
        ErrorKind::PermissionDenied => FileOperationErrorKind::PermissionDenied,
        ErrorKind::InvalidInput => FileOperationErrorKind::InvalidName,
        _ => FileOperationErrorKind::Io,
    }
}

fn local_io_error_is_cross_device(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(18)
}

#[cfg(unix)]
fn local_path_writable(path: &Path) -> bool {
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated byte string for the duration of the call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn local_path_writable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| !metadata.permissions().readonly())
}

fn file_kind(metadata: &fs::Metadata) -> FileKind {
    if metadata.is_file() {
        FileKind::File
    } else if metadata.is_dir() {
        FileKind::Directory
    } else if metadata.file_type().is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    }
}

#[cfg(unix)]
fn mode_bits(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn mode_bits(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0
    } else {
        0o200
    }
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn build_search_regex(query: &FileSearchQuery) -> Result<Regex, String> {
    let mut pattern = if query.regex {
        query.query.clone()
    } else {
        regex::escape(&query.query)
    };
    if query.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .dot_matches_new_line(true)
        .build()
        .map_err(|err| format!("Invalid search pattern: {err}"))
}

fn collect_file_matches(
    path: &FileNodePath,
    text: &str,
    matcher: &Regex,
    query: &FileSearchQuery,
    file_name_match_count: usize,
    matches: &mut Vec<FileSearchMatch>,
    limited: &mut bool,
) {
    for found in matcher.find_iter(text) {
        if found.is_empty() {
            continue;
        }
        if matches.len() + file_name_match_count >= query.max_results {
            *limited = true;
            return;
        }
        matches.push(FileSearchMatch {
            path: path.clone(),
            line_number: line_number_for_offset(text, found.start()),
            start: found.start(),
            end: found.end(),
            line_text: search_match_preview(text, found.start(), found.end()),
        });
        if matches.len() + file_name_match_count >= query.max_results {
            *limited = true;
            return;
        }
    }
}

fn line_number_for_offset(text: &str, offset: usize) -> u64 {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u64
        + 1
}

fn search_match_preview(text: &str, start: usize, end: usize) -> String {
    let line_start = text[..start.min(text.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = text[end.min(text.len())..]
        .find('\n')
        .map(|index| end.min(text.len()) + index)
        .unwrap_or(text.len());
    let preview = text[line_start..line_end].trim().replace(['\r', '\n'], " ");
    truncate_search_text(&preview)
}

fn truncate_search_text(text: &str) -> String {
    const MAX_CHARS: usize = 180;

    let mut output = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == MAX_CHARS {
            output.push_str("...");
            return output;
        }
        output.push(ch);
    }
    output
}

fn archive_format_arg(format: ArchiveFormat) -> &'static str {
    match format {
        ArchiveFormat::Zip => "zip",
        ArchiveFormat::Tar => "tar",
        ArchiveFormat::TarGz => "tar.gz",
        ArchiveFormat::TarXz => "tar.xz",
        ArchiveFormat::TarBz2 => "tar.bz2",
        ArchiveFormat::Iso => "iso",
        ArchiveFormat::Img => "img",
    }
}

fn archive_format_supported(format: ArchiveFormat) -> bool {
    matches!(
        format,
        ArchiveFormat::Zip
            | ArchiveFormat::Tar
            | ArchiveFormat::TarGz
            | ArchiveFormat::TarXz
            | ArchiveFormat::TarBz2
    )
}

fn archive_member_name_is_safe(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || has_windows_drive_prefix(name)
    {
        return false;
    }
    !name
        .split('/')
        .any(|part| matches!(part, "" | "." | "..") || has_windows_drive_prefix(part))
}

fn validate_archive_child_name(name: &str) -> Result<(), String> {
    if name.contains('/') || name.contains('\\') || !archive_member_name_is_safe(name) {
        return Err("Unsafe archive member name.".to_string());
    }
    Ok(())
}

fn validate_native_child_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || has_windows_drive_prefix(name)
    {
        return Err("Unsafe native file node name.".to_string());
    }
    Ok(())
}

fn validate_child_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Enter a name.".to_string());
    }
    validate_native_child_name(name).map_err(|_| {
        "Names cannot be absolute, parent-relative, or contain path separators.".to_string()
    })
}

fn has_windows_drive_prefix(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn file_node_display(path: &FileNodePath) -> String {
    let display = path.display();
    if display.is_empty() {
        ".".to_string()
    } else {
        display
    }
}

const ARCHIVE_LIST_SCRIPT: &str = include_str!("scripts/archive_list.py");
const ARCHIVE_READ_SCRIPT: &str = include_str!("scripts/archive_read.py");
