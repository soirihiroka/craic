use crate::system::path::{ArchiveFormat, FileNodePath, WorkspaceRef};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileNodeKind {
    File,
    Directory,
    Symlink,
    Archive { format: ArchiveFormat },
    Other,
}

pub use FileNodeKind as FileKind;

impl FileNodeKind {
    pub fn is_file(self) -> bool {
        matches!(self, Self::File | Self::Archive { .. })
    }

    pub fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }

    pub fn is_archive(self) -> bool {
        matches!(self, Self::Archive { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSignature {
    pub kind: FileNodeKind,
    pub len: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileNodeCapabilities {
    pub readable: bool,
    pub listable: bool,
    pub writable: bool,
    pub creatable: bool,
    pub movable: bool,
    pub deletable: bool,
    pub watchable: bool,
    pub searchable: bool,
    pub open_external: bool,
    pub reveal: bool,
    pub native: bool,
}

impl Default for FileNodeCapabilities {
    fn default() -> Self {
        Self {
            readable: false,
            listable: false,
            writable: false,
            creatable: false,
            movable: false,
            deletable: false,
            watchable: false,
            searchable: false,
            open_external: false,
            reveal: false,
            native: false,
        }
    }
}

impl FileNodeCapabilities {
    pub fn native_file(writable: bool) -> Self {
        Self {
            readable: true,
            writable,
            movable: writable,
            deletable: writable,
            watchable: true,
            searchable: true,
            open_external: true,
            reveal: true,
            native: true,
            ..Self::default()
        }
    }

    pub fn native_directory(writable: bool) -> Self {
        Self {
            readable: true,
            listable: true,
            writable,
            creatable: writable,
            movable: writable,
            deletable: writable,
            watchable: true,
            searchable: true,
            open_external: true,
            reveal: true,
            native: true,
            ..Self::default()
        }
    }

    pub fn native_other(writable: bool) -> Self {
        Self {
            readable: true,
            writable,
            movable: writable,
            deletable: writable,
            watchable: true,
            open_external: true,
            reveal: true,
            native: true,
            ..Self::default()
        }
    }

    pub fn archive_file() -> Self {
        Self {
            readable: true,
            listable: true,
            open_external: true,
            reveal: true,
            native: true,
            ..Self::default()
        }
    }

    pub fn virtual_file() -> Self {
        Self {
            readable: true,
            ..Self::default()
        }
    }

    pub fn virtual_directory() -> Self {
        Self {
            readable: true,
            listable: true,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileNodeInfo {
    pub path: FileNodePath,
    pub display_name: String,
    pub kind: FileNodeKind,
    pub len: Option<u64>,
    pub modified: Option<SystemTime>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub mode: Option<u32>,
    pub git_ignored: Option<bool>,
    pub capabilities: FileNodeCapabilities,
}

pub type FileMetadata = FileNodeInfo;

impl FileNodeInfo {
    pub fn readonly(&self) -> bool {
        !self.capabilities.writable
    }

    pub fn executable(&self) -> bool {
        self.mode.is_some_and(|mode| mode & 0o111 != 0)
    }

    pub fn len_or_zero(&self) -> u64 {
        self.len.unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct FileRead {
    pub info: FileNodeInfo,
    pub bytes: Option<Vec<u8>>,
}

impl FileRead {
    pub fn into_bytes(self) -> Result<Vec<u8>, String> {
        match self.bytes {
            Some(bytes) => Ok(bytes),
            None if self.info.kind.is_file() => Err(format!(
                "File is too large to read ({} bytes).",
                self.info.len_or_zero()
            )),
            None => Err("Select a file to read.".to_string()),
        }
    }

    pub fn into_text(self) -> Result<String, String> {
        let bytes = self.into_bytes()?;
        if bytes.contains(&0) {
            return Err("Binary file preview is unavailable.".to_string());
        }
        String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8 text.".to_string())
    }
}

#[derive(Clone, Debug)]
pub struct DirectoryListing {
    pub path: FileNodePath,
    pub entries: Vec<FileNodePath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileWatchRequest {
    pub paths: Vec<FileNodePath>,
    pub recursive: bool,
}

pub type FileWatchChanges = HashSet<FileNodePath>;
pub type FileWatchCallback = Arc<dyn Fn(FileWatchChanges) + Send + Sync + 'static>;
pub type FileCancellation = Arc<AtomicBool>;
pub type FileOperationCallback<T> = Box<dyn Fn(FileOperationEvent<T>) + Send + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOperation {
    Read,
    Write,
    Copy,
    Move,
    Delete,
}

impl FileOperation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOperationErrorKind {
    NotFound,
    AlreadyExists,
    InvalidName,
    PermissionDenied,
    OutsideWorkspace,
    Unsupported,
    TooLarge,
    Canceled,
    Io,
    Remote,
    Protocol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOperationError {
    pub operation: FileOperation,
    pub kind: FileOperationErrorKind,
    pub source: Option<FileNodePath>,
    pub destination: Option<FileNodePath>,
    pub message: String,
}

impl FileOperationError {
    pub fn new(
        operation: FileOperation,
        kind: FileOperationErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            kind,
            source: None,
            destination: None,
            message: message.into(),
        }
    }

    pub fn with_source(mut self, source: impl Into<FileNodePath>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_destination(mut self, destination: impl Into<FileNodePath>) -> Self {
        self.destination = Some(destination.into());
        self
    }

    pub fn from_message(
        operation: FileOperation,
        kind: FileOperationErrorKind,
        source: Option<FileNodePath>,
        destination: Option<FileNodePath>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            kind,
            source,
            destination,
            message: message.into(),
        }
    }

    pub fn canceled(operation: FileOperation) -> Self {
        Self::new(
            operation,
            FileOperationErrorKind::Canceled,
            "Operation canceled.",
        )
    }
}

impl fmt::Display for FileOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOperationProgress {
    pub operation: FileOperation,
    pub source: Option<FileNodePath>,
    pub destination: Option<FileNodePath>,
    pub current_path: Option<FileNodePath>,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub completed_files: u64,
    pub total_files: u64,
}

impl FileOperationProgress {
    pub fn new(operation: FileOperation) -> Self {
        Self {
            operation,
            source: None,
            destination: None,
            current_path: None,
            completed_bytes: 0,
            total_bytes: 0,
            completed_files: 0,
            total_files: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOperationEvent<T> {
    Progress(FileOperationProgress),
    Finished(Result<T, FileOperationError>),
}

#[derive(Clone, Debug)]
pub struct FileReadRequest {
    pub path: FileNodePath,
    pub max_bytes: Option<u64>,
    pub cancel_requested: Option<FileCancellation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileWriteMode {
    CreateNew,
    Replace,
    Append,
}

#[derive(Clone, Debug)]
pub enum FileWritePayload {
    File(Vec<u8>),
    Directory,
}

#[derive(Clone, Debug)]
pub struct FileWriteRequest {
    pub path: FileNodePath,
    pub mode: FileWriteMode,
    pub payload: FileWritePayload,
    pub cancel_requested: Option<FileCancellation>,
}

#[derive(Clone, Debug)]
pub struct FileCopyRequest {
    pub source: FileNodePath,
    pub destination: FileNodePath,
    pub cancel_requested: Option<FileCancellation>,
}

#[derive(Clone, Debug)]
pub struct FileMoveRequest {
    pub source: FileNodePath,
    pub destination_parent: FileNodePath,
    pub new_name: String,
    pub cancel_requested: Option<FileCancellation>,
}

impl FileMoveRequest {
    pub fn destination(&self) -> FileNodePath {
        self.destination_parent.join_child(&self.new_name)
    }
}

#[derive(Clone, Debug)]
pub struct FileDeleteRequest {
    pub path: FileNodePath,
    pub cancel_requested: Option<FileCancellation>,
}

pub fn file_operation_canceled(cancel_requested: &Option<FileCancellation>) -> bool {
    cancel_requested
        .as_ref()
        .is_some_and(|cancel_requested| cancel_requested.load(Ordering::Relaxed))
}

pub struct FileWatchSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl FileWatchSubscription {
    pub fn spawn_signature_map_loop<F>(
        label: impl Into<String>,
        interval: Duration,
        mut snapshot: F,
        callback: FileWatchCallback,
    ) -> Self
    where
        F: FnMut() -> Result<HashMap<FileNodePath, Option<FileSignature>>, String> + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!(
                "file watcher started label={} interval_ms={}",
                thread_label,
                interval.as_millis()
            );
            let mut previous_snapshot: Option<HashMap<FileNodePath, Option<FileSignature>>> = None;
            let mut previous_error: Option<String> = None;

            loop {
                let cycle_start = Instant::now();
                match snapshot() {
                    Ok(next_snapshot) => {
                        if previous_error.take().is_some() {
                            log::info!("file watcher recovered label={thread_label}");
                        }

                        if let Some(previous) = &previous_snapshot {
                            let mut changes = FileWatchChanges::new();
                            for (path, next_signature) in &next_snapshot {
                                if previous.get(path) != Some(next_signature) {
                                    changes.insert(path.clone());
                                }
                            }
                            for path in previous.keys() {
                                if !next_snapshot.contains_key(path) {
                                    changes.insert(path.clone());
                                }
                            }
                            if !changes.is_empty() {
                                log::info!(
                                    "file watcher change detected label={} changed_paths={}",
                                    thread_label,
                                    changes.len()
                                );
                                callback(changes);
                            }
                        } else {
                            log::debug!(
                                "file watcher initial snapshot label={} watched_paths={}",
                                thread_label,
                                next_snapshot.len()
                            );
                        }
                        previous_snapshot = Some(next_snapshot);
                    }
                    Err(err) => {
                        if previous_error.as_deref() == Some(err.as_str()) {
                            log::debug!("file watcher repeated error label={thread_label}: {err}");
                        } else {
                            log::warn!("file watcher error label={thread_label}: {err}");
                            previous_error = Some(err);
                        }
                    }
                }

                let remaining = interval.saturating_sub(cycle_start.elapsed());
                if stop_receiver.recv_timeout(remaining).is_ok() {
                    break;
                }
            }

            log::info!("file watcher stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            _thread: Some(thread),
        }
    }

    pub fn spawn_thread<F>(label: impl Into<String>, run: F) -> Self
    where
        F: FnOnce(mpsc::Receiver<()>) + Send + 'static,
    {
        let label = label.into();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let thread_label = label.clone();
        let thread = thread::spawn(move || {
            log::info!("file watcher thread started label={thread_label}");
            run(stop_receiver);
            log::info!("file watcher thread stopped label={thread_label}");
        });

        Self {
            stop_sender: Some(stop_sender),
            _thread: Some(thread),
        }
    }
}

impl Drop for FileWatchSubscription {
    fn drop(&mut self) {
        if let Some(stop_sender) = self.stop_sender.take() {
            let _ = stop_sender.send(());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchQuery {
    pub root: FileNodePath,
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub max_results: usize,
    pub max_file_bytes: u64,
    pub excluded_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchMatch {
    pub path: FileNodePath,
    pub line_number: u64,
    pub start: usize,
    pub end: usize,
    pub line_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSearchOutput {
    pub text_matches: Vec<FileSearchMatch>,
    pub file_name_matches: Vec<FileNodePath>,
    pub limited: bool,
}

pub trait FileAccess: Send + Sync {
    fn workspace(&self) -> WorkspaceRef;
    fn root(&self) -> FileNodePath;
    fn copy_path(&self, path: &FileNodePath) -> String {
        path.to_workspace_path(&self.workspace())
            .map(|path| path.absolute)
            .unwrap_or_else(|| path.display())
    }

    fn list_dirs(&self, paths: &[FileNodePath]) -> Result<Vec<DirectoryListing>, String>;
    fn info(&self, path: &FileNodePath) -> Result<FileNodeInfo, String>;
    fn info_many(&self, paths: &[FileNodePath]) -> Result<Vec<FileNodeInfo>, String> {
        paths.iter().map(|path| self.info(path)).collect()
    }

    fn read_with_info(&self, request: FileReadRequest, callback: FileOperationCallback<FileRead>);

    fn write_node(&self, request: FileWriteRequest, callback: FileOperationCallback<()>);
    fn copy_node(&self, request: FileCopyRequest, callback: FileOperationCallback<FileNodePath>);
    fn move_node(&self, request: FileMoveRequest, callback: FileOperationCallback<FileNodePath>);
    fn delete(&self, request: FileDeleteRequest, callback: FileOperationCallback<()>);

    fn watch(
        &self,
        request: FileWatchRequest,
        callback: FileWatchCallback,
    ) -> Result<FileWatchSubscription, String>;
    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String>;
}
