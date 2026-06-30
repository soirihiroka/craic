use crate::system::path::{SystemPath, WorkspacePath, WorkspaceRef};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileSignature {
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub(crate) struct FileMetadata {
    pub(crate) path: SystemPath,
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) readonly: bool,
    pub(crate) executable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct FileRead {
    pub(crate) metadata: FileMetadata,
    pub(crate) bytes: Option<Vec<u8>>,
}

impl FileRead {
    pub(crate) fn into_bytes(self) -> Result<Vec<u8>, String> {
        match self.bytes {
            Some(bytes) => Ok(bytes),
            None if self.metadata.kind == FileKind::File => Err(format!(
                "File is too large to read ({} bytes).",
                self.metadata.len
            )),
            None => Err("Select a file to read.".to_string()),
        }
    }

    pub(crate) fn into_text(self) -> Result<String, String> {
        let bytes = self.into_bytes()?;
        if bytes.contains(&0) {
            return Err("Binary file preview is unavailable.".to_string());
        }
        String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8 text.".to_string())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DirectoryEntry {
    pub(crate) path: SystemPath,
    pub(crate) name: String,
    pub(crate) kind: FileKind,
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) executable: bool,
    pub(crate) git_ignored: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectoryListing {
    pub(crate) path: WorkspacePath,
    pub(crate) entries: Vec<DirectoryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileWatchRequest {
    pub(crate) paths: Vec<WorkspacePath>,
    pub(crate) recursive: bool,
}

pub(crate) type FileWatchChanges = HashSet<WorkspacePath>;
pub(crate) type FileWatchCallback = Arc<dyn Fn(FileWatchChanges) + Send + Sync + 'static>;

pub(crate) struct FileWatchSubscription {
    stop_sender: Option<mpsc::Sender<()>>,
    _thread: Option<thread::JoinHandle<()>>,
}

impl FileWatchSubscription {
    pub(crate) fn spawn_signature_map_loop<F>(
        label: impl Into<String>,
        interval: Duration,
        mut snapshot: F,
        callback: FileWatchCallback,
    ) -> Self
    where
        F: FnMut() -> Result<HashMap<WorkspacePath, Option<FileSignature>>, String>
            + Send
            + 'static,
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
            let mut previous_snapshot: Option<HashMap<WorkspacePath, Option<FileSignature>>> = None;
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

    pub(crate) fn spawn_thread<F>(label: impl Into<String>, run: F) -> Self
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
pub(crate) struct FileSearchQuery {
    pub(crate) root: WorkspacePath,
    pub(crate) query: String,
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
    pub(crate) max_results: usize,
    pub(crate) max_file_bytes: u64,
    pub(crate) excluded_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileSearchMatch {
    pub(crate) path: WorkspacePath,
    pub(crate) line_number: u64,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) line_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileSearchOutput {
    pub(crate) matches: Vec<FileSearchMatch>,
    pub(crate) limited: bool,
}

pub(crate) trait FileAccess: Send + Sync {
    fn workspace(&self) -> WorkspaceRef;
    fn watch(
        &self,
        request: FileWatchRequest,
        callback: FileWatchCallback,
    ) -> Result<FileWatchSubscription, String>;
    fn metadata(&self, path: &WorkspacePath) -> Result<FileMetadata, String>;
    fn list_dirs(&self, paths: &[WorkspacePath]) -> Result<Vec<DirectoryListing>, String>;
    fn read_with_metadata(
        &self,
        path: &WorkspacePath,
        max_bytes: Option<u64>,
    ) -> Result<FileRead, String>;
    fn read_bytes(&self, path: &WorkspacePath, max_bytes: Option<u64>) -> Result<Vec<u8>, String> {
        self.read_with_metadata(path, max_bytes)?.into_bytes()
    }
    fn read_text(&self, path: &WorkspacePath, max_bytes: Option<u64>) -> Result<String, String> {
        self.read_with_metadata(path, max_bytes)?.into_text()
    }
    fn write_bytes(&self, path: &WorkspacePath, contents: &[u8]) -> Result<(), String>;
    fn write_text(&self, path: &WorkspacePath, contents: &str) -> Result<(), String>;
    fn create_file(&self, path: &WorkspacePath) -> Result<(), String>;
    fn create_dir(&self, path: &WorkspacePath) -> Result<(), String>;
    fn rename(&self, source: &WorkspacePath, destination: &WorkspacePath) -> Result<(), String>;
    fn delete(&self, path: &WorkspacePath) -> Result<(), String>;
    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String>;
}
