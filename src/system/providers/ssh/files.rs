use super::{SshCommandRunner, remote_workspace_path, shell_quote, workspace_path_for_remote};
use crate::system::capabilities::files::{
    DirectoryListing, FileAccess, FileCopyRequest, FileDeleteRequest, FileKind, FileMoveRequest,
    FileNodeCapabilities, FileNodeInfo, FileOperation, FileOperationCallback, FileOperationError,
    FileOperationErrorKind, FileOperationEvent, FileOperationProgress, FileRead, FileReadRequest,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileWatchCallback,
    FileWatchRequest, FileWatchSubscription, FileWriteMode, FileWritePayload, FileWriteRequest,
    file_operation_canceled,
};
use crate::system::path::{ArchiveFormat, FileNodePath, SystemRef, WorkspacePath, WorkspaceRef};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

const SSH_FILE_WATCH_POLL_INTERVAL: Duration = Duration::from_secs(60);
const SSH_LIST_DIR_CACHE_TTL: Duration = Duration::from_millis(500);
const COPY_PATH_SCRIPT: &str = include_str!("scripts/copy_path.sh");
const FILE_SIGNATURES_SCRIPT: &str = include_str!("scripts/file_signatures.sh");
const LIST_DIRS_SCRIPT: &str = include_str!("scripts/list_dirs.sh");
const METADATA_SCRIPT: &str = include_str!("scripts/metadata.sh");
const MOVE_PATH_SCRIPT: &str = include_str!("scripts/move_path.sh");
const READ_WITH_INFO_SCRIPT: &str = include_str!("scripts/read_with_info.sh");
const WRITE_CREATE_NEW_SCRIPT: &str = include_str!("scripts/write_create_new.sh");
const WRITE_REPLACE_SCRIPT: &str = include_str!("scripts/write_replace.sh");

#[derive(Clone, Debug)]
pub(crate) struct SshFileAccess {
    system: SystemRef,
    workspace: WorkspaceRef,
    runner: SshCommandRunner,
    list_dir_cache: Arc<Mutex<SshListDirCache>>,
}

#[derive(Debug, Default)]
struct SshListDirCache {
    entries: HashMap<String, CachedDirectoryListing>,
}

#[derive(Clone, Debug)]
struct CachedDirectoryListing {
    listing: DirectoryListing,
    cached_at: Instant,
}

#[derive(Clone, Debug)]
struct SshResolvedFileNode {
    path: FileNodePath,
    remote_path: String,
}

#[derive(Deserialize)]
struct RemoteSearchOutput {
    text_matches: Vec<RemoteSearchMatch>,
    file_name_matches: Vec<String>,
    limited: bool,
}

#[derive(Deserialize)]
struct RemoteSearchMatch {
    path: String,
    line_number: u64,
    start: usize,
    end: usize,
    line_text: String,
}

impl SshFileAccess {
    pub(crate) fn new(
        system: SystemRef,
        workspace: WorkspaceRef,
        runner: SshCommandRunner,
    ) -> Self {
        Self {
            system,
            workspace,
            runner,
            list_dir_cache: Arc::new(Mutex::new(SshListDirCache::default())),
        }
    }

    fn resolve_native_node(
        &self,
        path: &FileNodePath,
        operation: &str,
    ) -> Result<SshResolvedFileNode, String> {
        let node_display = file_node_display(path);
        let Some((root_id, system_id)) = path.root_ref() else {
            log::warn!(
                "ssh file node operation denied workspace={} operation={} node={} reason=missing-root",
                self.workspace.display_name,
                operation,
                node_display
            );
            return Err("File node does not belong to this SSH workspace.".to_string());
        };

        if root_id != self.workspace.id.as_str() || system_id != &self.system.id {
            log::warn!(
                "ssh file node operation denied workspace={} operation={} node={} reason=wrong-root",
                self.workspace.display_name,
                operation,
                node_display
            );
            return Err("File node does not belong to this SSH workspace.".to_string());
        }

        if path.contains_archive() {
            return self.reject_virtual_node(
                path,
                operation,
                "SSH archive browsing is not supported for remote workspaces. Extract the archive on the remote system first.",
            );
        }

        let Some(relative) = path.native_relative() else {
            return self.reject_virtual_node(
                path,
                operation,
                "Virtual SSH file nodes are unsupported for this operation.",
            );
        };
        let workspace_path =
            WorkspacePath::from_workspace_relative(&self.workspace.root, &relative);
        let remote_path = remote_workspace_path(&self.workspace, &workspace_path);
        log::info!(
            "ssh file node resolved workspace={} operation={} node={} remote_path={}",
            self.workspace.display_name,
            operation,
            node_display,
            remote_path
        );
        Ok(SshResolvedFileNode {
            path: path.clone(),
            remote_path,
        })
    }

    fn reject_virtual_node<T>(
        &self,
        path: &FileNodePath,
        operation: &str,
        message: &str,
    ) -> Result<T, String> {
        log::warn!(
            "ssh file node operation denied workspace={} operation={} node={} reason=virtual-or-archive",
            self.workspace.display_name,
            operation,
            file_node_display(path)
        );
        Err(message.to_string())
    }

    fn node_info(
        &self,
        path: FileNodePath,
        kind: FileKind,
        len: u64,
        modified: Option<f64>,
        readonly: bool,
        executable: bool,
        git_ignored: Option<bool>,
    ) -> FileNodeInfo {
        let mut node_kind = kind;
        let mut capabilities = match kind {
            FileKind::Directory => FileNodeCapabilities::native_directory(!readonly),
            FileKind::File => FileNodeCapabilities::native_file(!readonly),
            FileKind::Symlink | FileKind::Other => FileNodeCapabilities::native_other(!readonly),
            FileKind::Archive { .. } => unreachable!(),
        };
        if kind == FileKind::File
            && let Some(format) = path.file_name().and_then(ArchiveFormat::from_name)
        {
            node_kind = FileKind::Archive { format };
            capabilities = FileNodeCapabilities {
                listable: false,
                ..FileNodeCapabilities::archive_file()
            };
        }
        FileNodeInfo {
            path: path.clone(),
            display_name: path
                .file_name()
                .map(ToString::to_string)
                .unwrap_or_else(|| self.workspace.display_name.clone()),
            kind: node_kind,
            len: Some(len),
            modified: remote_time(modified),
            owner: None,
            group: None,
            mode: executable.then_some(0o111),
            git_ignored,
            capabilities,
        }
    }

    fn operation_error(
        operation: FileOperation,
        kind: FileOperationErrorKind,
        source: Option<FileNodePath>,
        destination: Option<FileNodePath>,
        message: impl Into<String>,
    ) -> FileOperationError {
        FileOperationError::from_message(operation, kind, source, destination, message)
    }

    fn remote_error(
        operation: FileOperation,
        source: Option<FileNodePath>,
        destination: Option<FileNodePath>,
        message: impl Into<String>,
    ) -> FileOperationError {
        let message = message.into();
        let kind = if message.contains("CRAIC-ERROR\talready-exists") {
            FileOperationErrorKind::AlreadyExists
        } else if message.contains("CRAIC-ERROR\tnot-found") {
            FileOperationErrorKind::NotFound
        } else if message.contains("Permission denied") {
            FileOperationErrorKind::PermissionDenied
        } else {
            FileOperationErrorKind::Remote
        };
        let message = message
            .lines()
            .find(|line| !line.starts_with("CRAIC-ERROR\t"))
            .unwrap_or(&message)
            .to_string();
        Self::operation_error(operation, kind, source, destination, message)
    }

    fn canceled_error(operation: FileOperation, source: &FileNodePath) -> FileOperationError {
        FileOperationError::canceled(operation).with_source(source.clone())
    }

    fn check_canceled(
        operation: FileOperation,
        source: &FileNodePath,
        request: &Option<crate::system::capabilities::files::FileCancellation>,
    ) -> Result<(), FileOperationError> {
        if file_operation_canceled(request) {
            Err(Self::canceled_error(operation, source))
        } else {
            Ok(())
        }
    }

    fn emit_progress<T>(callback: &FileOperationCallback<T>, progress: FileOperationProgress) {
        callback(FileOperationEvent::Progress(progress));
    }

    fn perform_read_with_info(
        &self,
        request: &FileReadRequest,
        callback: &FileOperationCallback<FileRead>,
    ) -> Result<FileRead, FileOperationError> {
        let operation = FileOperation::Read;
        Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
        let resolved = self
            .resolve_native_node(&request.path, "read_with_info")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.path.clone()),
                    None,
                    err,
                )
            })?;
        let script = shell_read_with_info_script(&resolved.remote_path, request.max_bytes);
        let output = self
            .runner
            .run_script("read file node with info", &script)
            .map_err(|err| Self::remote_error(operation, Some(request.path.clone()), None, err))?;
        let read = parse_read_output(self, resolved.path, output.stdout).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::Protocol,
                Some(request.path.clone()),
                None,
                err,
            )
        })?;
        if let Some(bytes) = read.bytes.as_ref() {
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_bytes: bytes.len() as u64,
                    total_bytes: read.info.len_or_zero(),
                    completed_files: 1,
                    total_files: 1,
                    destination: None,
                },
            );
        }
        Ok(read)
    }

    fn perform_write_node(
        &self,
        request: &FileWriteRequest,
        callback: &FileOperationCallback<()>,
    ) -> Result<(), FileOperationError> {
        let operation = FileOperation::Write;
        Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
        let resolved = self
            .resolve_native_node(&request.path, "write_node")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.path.clone()),
                    None,
                    err,
                )
            })?;
        if matches!(request.payload, FileWritePayload::Directory) {
            if request.mode != FileWriteMode::CreateNew {
                return Err(Self::operation_error(
                    operation,
                    FileOperationErrorKind::Unsupported,
                    Some(request.path.clone()),
                    None,
                    "Directories can only be created with create-new mode.",
                ));
            }
            let script = format!("mkdir -- {}", shell_quote(&resolved.remote_path));
            self.runner
                .run_script("create directory", &script)
                .map_err(|err| {
                    Self::remote_error(operation, Some(request.path.clone()), None, err)
                })?;
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    destination: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_files: 1,
                    total_files: 1,
                    ..FileOperationProgress::new(operation)
                },
            );
            return Ok(());
        }

        let FileWritePayload::File(contents) = &request.payload else {
            unreachable!();
        };
        let script = match request.mode {
            FileWriteMode::CreateNew => shell_script_with_args(
                WRITE_CREATE_NEW_SCRIPT,
                std::slice::from_ref(&resolved.remote_path),
            ),
            FileWriteMode::Replace => shell_script_with_args(
                WRITE_REPLACE_SCRIPT,
                std::slice::from_ref(&resolved.remote_path),
            ),
        };
        self.runner
            .run_script_with_stdin("write file", &script, Some(contents))
            .map_err(|err| Self::remote_error(operation, Some(request.path.clone()), None, err))?;
        Self::emit_progress(
            callback,
            FileOperationProgress {
                operation,
                source: Some(request.path.clone()),
                destination: Some(request.path.clone()),
                current_path: Some(request.path.clone()),
                completed_bytes: contents.len() as u64,
                total_bytes: contents.len() as u64,
                completed_files: 1,
                total_files: 1,
            },
        );
        Ok(())
    }

    fn perform_copy_node(
        &self,
        request: &FileCopyRequest,
        callback: &FileOperationCallback<FileNodePath>,
    ) -> Result<FileNodePath, FileOperationError> {
        let operation = FileOperation::Copy;
        Self::check_canceled(operation, &request.source, &request.cancel_requested)?;
        let source = self
            .resolve_native_node(&request.source, "copy_source")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.source.clone()),
                    Some(request.destination.clone()),
                    err,
                )
            })?;
        let destination = self
            .resolve_native_node(&request.destination, "copy_destination")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.source.clone()),
                    Some(request.destination.clone()),
                    err,
                )
            })?;
        if request.source == request.destination {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::AlreadyExists,
                Some(request.source.clone()),
                Some(request.destination.clone()),
                format!("{} already exists.", request.destination.display()),
            ));
        }
        let script = shell_script_with_args(
            COPY_PATH_SCRIPT,
            &[source.remote_path.clone(), destination.remote_path.clone()],
        );
        self.runner
            .run_script("copy path", &script)
            .map_err(|err| {
                Self::remote_error(
                    operation,
                    Some(request.source.clone()),
                    Some(request.destination.clone()),
                    err,
                )
            })?;
        Self::emit_progress(
            callback,
            FileOperationProgress {
                operation,
                source: Some(request.source.clone()),
                destination: Some(request.destination.clone()),
                current_path: Some(request.destination.clone()),
                completed_files: 1,
                total_files: 1,
                ..FileOperationProgress::new(operation)
            },
        );
        Ok(request.destination.clone())
    }

    fn perform_move_node(
        &self,
        request: &FileMoveRequest,
        callback: &FileOperationCallback<FileNodePath>,
    ) -> Result<FileNodePath, FileOperationError> {
        let operation = FileOperation::Move;
        Self::check_canceled(operation, &request.source, &request.cancel_requested)?;
        validate_child_name(&request.new_name).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::InvalidName,
                Some(request.source.clone()),
                Some(request.destination()),
                err,
            )
        })?;
        let source = self
            .resolve_native_node(&request.source, "move_source")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.source.clone()),
                    Some(request.destination()),
                    err,
                )
            })?;
        let destination_path = request.destination();
        if request.source == destination_path {
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.source.clone()),
                    destination: Some(destination_path.clone()),
                    current_path: Some(destination_path.clone()),
                    completed_files: 1,
                    total_files: 1,
                    ..FileOperationProgress::new(operation)
                },
            );
            return Ok(destination_path);
        }
        let destination = self
            .resolve_native_node(&destination_path, "move_destination")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.source.clone()),
                    Some(destination_path.clone()),
                    err,
                )
            })?;
        let script = shell_script_with_args(
            MOVE_PATH_SCRIPT,
            &[source.remote_path.clone(), destination.remote_path.clone()],
        );
        self.runner
            .run_script("move path", &script)
            .map_err(|err| {
                Self::remote_error(
                    operation,
                    Some(request.source.clone()),
                    Some(destination_path.clone()),
                    err,
                )
            })?;
        Self::emit_progress(
            callback,
            FileOperationProgress {
                operation,
                source: Some(request.source.clone()),
                destination: Some(destination_path.clone()),
                current_path: Some(destination_path.clone()),
                completed_files: 1,
                total_files: 1,
                ..FileOperationProgress::new(operation)
            },
        );
        Ok(destination_path)
    }

    fn perform_delete(
        &self,
        request: &FileDeleteRequest,
        callback: &FileOperationCallback<()>,
    ) -> Result<(), FileOperationError> {
        let operation = FileOperation::Delete;
        Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
        let resolved = self
            .resolve_native_node(&request.path, "delete")
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.path.clone()),
                    None,
                    err,
                )
            })?;
        let script = format!("rm -rf -- {}", shell_quote(&resolved.remote_path));
        self.runner
            .run_script("delete path", &script)
            .map_err(|err| Self::remote_error(operation, Some(request.path.clone()), None, err))?;
        Self::emit_progress(
            callback,
            FileOperationProgress {
                operation,
                source: Some(request.path.clone()),
                current_path: Some(request.path.clone()),
                completed_files: 1,
                total_files: 1,
                ..FileOperationProgress::new(operation)
            },
        );
        Ok(())
    }
}

impl SshListDirCache {
    fn fresh_listing(&self, path: &FileNodePath, now: Instant) -> Option<DirectoryListing> {
        self.entries
            .get(&ssh_list_dir_cache_key(path))
            .filter(|entry| now.duration_since(entry.cached_at) <= SSH_LIST_DIR_CACHE_TTL)
            .map(|entry| entry.listing.clone())
    }

    fn listing(&self, path: &FileNodePath) -> Option<DirectoryListing> {
        self.entries
            .get(&ssh_list_dir_cache_key(path))
            .map(|entry| entry.listing.clone())
    }

    fn insert_listings(&mut self, listings: Vec<DirectoryListing>) {
        let cached_at = Instant::now();
        for listing in listings {
            self.entries.insert(
                ssh_list_dir_cache_key(&listing.path),
                CachedDirectoryListing { listing, cached_at },
            );
        }
    }
}

fn ssh_list_dir_cache_key(path: &FileNodePath) -> String {
    path.display()
}

fn file_node_display(path: &FileNodePath) -> String {
    let display = path.display();
    if display.is_empty() {
        ".".to_string()
    } else {
        display
    }
}

impl FileAccess for SshFileAccess {
    fn workspace(&self) -> WorkspaceRef {
        self.workspace.clone()
    }

    fn root(&self) -> FileNodePath {
        self.workspace.root_node_path(&self.system)
    }

    fn info(&self, path: &FileNodePath) -> Result<FileNodeInfo, String> {
        let resolved = self.resolve_native_node(path, "info")?;
        let script = shell_metadata_script(&resolved.remote_path);
        let raw = self.runner.run_text("file node info", &script)?;
        parse_metadata_line(self, resolved.path, raw.trim_end(), None)
    }

    fn info_many(&self, paths: &[FileNodePath]) -> Result<Vec<FileNodeInfo>, String> {
        let infos = paths
            .iter()
            .map(|path| self.info(path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(infos)
    }

    fn list_dirs(&self, paths: &[FileNodePath]) -> Result<Vec<DirectoryListing>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let resolved_paths = paths
            .iter()
            .map(|path| self.resolve_native_node(path, "list_dirs"))
            .collect::<Result<Vec<_>, _>>()?;

        let mut cache = self
            .list_dir_cache
            .lock()
            .map_err(|_| "SSH directory list cache is unavailable.".to_string())?;
        let missing_paths = resolved_paths
            .iter()
            .filter(|resolved| {
                cache
                    .fresh_listing(&resolved.path, Instant::now())
                    .is_none()
            })
            .cloned()
            .collect::<Vec<_>>();

        if !missing_paths.is_empty() {
            log::debug!(
                "ssh list directories cache miss workspace={} requested={} missing={}",
                self.workspace.display_name,
                paths.len(),
                missing_paths.len()
            );
            let remote_paths = missing_paths
                .iter()
                .map(|resolved| resolved.remote_path.clone())
                .collect::<Vec<_>>();
            let script = shell_list_dirs_script(&remote_paths);
            let raw = self.runner.run_text("list directories", &script)?;
            let mut listings = parse_directory_output(&missing_paths, &raw)?;
            for listing in &mut listings {
                listing
                    .entries
                    .sort_by(|left, right| left.display().cmp(&right.display()));
            }
            cache.insert_listings(listings);
        } else {
            log::debug!(
                "ssh list directories cache hit workspace={} requested={}",
                self.workspace.display_name,
                paths.len()
            );
        }

        let listings = resolved_paths
            .iter()
            .filter_map(|resolved| cache.listing(&resolved.path))
            .collect::<Vec<_>>();
        if listings.len() != paths.len() {
            return Err("SSH directory listing response was incomplete.".to_string());
        }
        Ok(listings)
    }

    fn read_with_info(&self, request: FileReadRequest, callback: FileOperationCallback<FileRead>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "ssh file read worker start path={} max_bytes={:?}",
                request.path.display(),
                request.max_bytes
            );
            let result = access.perform_read_with_info(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn write_node(&self, request: FileWriteRequest, callback: FileOperationCallback<()>) {
        let access = self.clone();
        thread::spawn(move || {
            let payload_label = match &request.payload {
                FileWritePayload::File(contents) => format!("file bytes={}", contents.len()),
                FileWritePayload::Directory => "directory".to_string(),
            };
            log::info!(
                "ssh file write worker start path={} payload={}",
                request.path.display(),
                payload_label
            );
            let result = access.perform_write_node(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn copy_node(&self, request: FileCopyRequest, callback: FileOperationCallback<FileNodePath>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "ssh file copy worker start source={} destination={}",
                request.source.display(),
                request.destination.display()
            );
            let result = access.perform_copy_node(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn move_node(&self, request: FileMoveRequest, callback: FileOperationCallback<FileNodePath>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "ssh file move worker start source={} destination_parent={} new_name={}",
                request.source.display(),
                request.destination_parent.display(),
                request.new_name
            );
            let result = access.perform_move_node(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn delete(&self, request: FileDeleteRequest, callback: FileOperationCallback<()>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "ssh file delete worker start path={}",
                request.path.display()
            );
            let result = access.perform_delete(&request, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn watch(
        &self,
        request: FileWatchRequest,
        callback: FileWatchCallback,
    ) -> Result<FileWatchSubscription, String> {
        let requested_paths = if request.paths.is_empty() {
            vec![self.root()]
        } else {
            request.paths.clone()
        };
        let requested = requested_paths
            .iter()
            .map(|path| {
                self.resolve_native_node(path, "watch")
                    .map(|resolved| (resolved.path, resolved.remote_path))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let runner = self.runner.clone();
        let label = if requested_paths.len() == 1 {
            format!(
                "ssh-file:{}:{}",
                self.workspace.display_name,
                file_node_display(&requested_paths[0])
            )
        } else {
            format!(
                "ssh-file:{}:{}paths",
                self.workspace.display_name,
                requested_paths.len()
            )
        };
        log::info!(
            "ssh file watch registered workspace={} paths={} recursive={} interval_secs={}",
            self.workspace.display_name,
            requested.len(),
            request.recursive,
            SSH_FILE_WATCH_POLL_INTERVAL.as_secs()
        );
        Ok(FileWatchSubscription::spawn_signature_map_loop(
            label,
            SSH_FILE_WATCH_POLL_INTERVAL,
            move || remote_file_signatures(&runner, &requested),
            callback,
        ))
    }

    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String> {
        let resolved = self.resolve_native_node(&query.root, "search_text")?;
        log::info!(
            "file search start provider=ssh workspace={} query_len={} root={}",
            self.workspace.display_name,
            query.query.len(),
            query.root.display()
        );
        let excluded_names =
            serde_json::to_string(&query.excluded_names).unwrap_or_else(|_| json!([]).to_string());
        let script = format!(
            "python3 -c {} {} {} {} {} {} {} {} {}",
            shell_quote(SEARCH_SCRIPT),
            shell_quote(&resolved.remote_path),
            shell_quote(&query.query),
            shell_quote(&excluded_names),
            if query.case_sensitive { "1" } else { "0" },
            if query.whole_word { "1" } else { "0" },
            if query.regex { "1" } else { "0" },
            query.max_results,
            query.max_file_bytes,
        );
        let raw = self.runner.run_text("search files", &script)?;
        let output: RemoteSearchOutput = serde_json::from_str(&raw)
            .map_err(|err| format!("Invalid remote search response: {err}"))?;
        let limited = output.limited;
        let text_matches = output
            .text_matches
            .into_iter()
            .map(|found| {
                let workspace_path = workspace_path_for_remote(&self.workspace, &found.path);
                FileSearchMatch {
                    path: self
                        .workspace
                        .node_path(&self.system, workspace_path.relative_or_empty()),
                    line_number: found.line_number,
                    start: found.start,
                    end: found.end,
                    line_text: found.line_text,
                }
            })
            .collect::<Vec<_>>();
        let mut file_name_matches = output
            .file_name_matches
            .into_iter()
            .map(|path| {
                let workspace_path = workspace_path_for_remote(&self.workspace, &path);
                self.workspace
                    .node_path(&self.system, workspace_path.relative_or_empty())
            })
            .collect::<Vec<_>>();
        file_name_matches.sort_by_key(FileNodePath::display);
        log::info!(
            "file search complete provider=ssh workspace={} text_matches={} file_name_matches={} limited={}",
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

fn parse_kind(kind: &str) -> FileKind {
    match kind {
        "f" | "regular file" | "file" => FileKind::File,
        "d" | "directory" | "dir" => FileKind::Directory,
        "l" | "symbolic link" | "symlink" => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

fn shell_script_with_args(script: &str, args: &[String]) -> String {
    let mut command = String::from("set --");
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command.push('\n');
    command.push_str(script);
    command
}

fn remote_time(value: Option<f64>) -> Option<std::time::SystemTime> {
    value.map(|secs| UNIX_EPOCH + Duration::from_secs_f64(secs.max(0.0)))
}

fn shell_metadata_script(remote_path: &str) -> String {
    shell_script_with_args(METADATA_SCRIPT, &[remote_path.to_string()])
}

fn remote_file_signatures(
    runner: &SshCommandRunner,
    requested: &[(FileNodePath, String)],
) -> Result<HashMap<FileNodePath, Option<FileSignature>>, String> {
    let remote_paths = requested
        .iter()
        .map(|(_, remote_path)| remote_path.clone())
        .collect::<Vec<_>>();
    let script = shell_file_signatures_script(&remote_paths);
    let raw = runner.run_text("file watch metadata", &script)?;
    parse_file_signature_lines(requested, &raw)
}

fn shell_file_signatures_script(remote_paths: &[String]) -> String {
    shell_script_with_args(FILE_SIGNATURES_SCRIPT, remote_paths)
}

fn parse_file_signature_lines(
    requested: &[(FileNodePath, String)],
    raw: &str,
) -> Result<HashMap<FileNodePath, Option<FileSignature>>, String> {
    let mut signatures = requested
        .iter()
        .map(|(path, _)| (path.clone(), None))
        .collect::<HashMap<_, _>>();

    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\x1f');
        if fields.next() != Some("CRAIC-WATCH") {
            continue;
        }
        let remote_path = fields.next().unwrap_or_default();
        let state = fields.next().unwrap_or_default();
        let Some((node_path, _)) = requested
            .iter()
            .find(|(_, requested_remote_path)| requested_remote_path == remote_path)
        else {
            continue;
        };

        match state {
            "missing" => {
                signatures.insert(node_path.clone(), None);
            }
            "present" => {
                let kind = parse_kind(fields.next().unwrap_or_default());
                let len = fields
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or_else(|| "Invalid remote file watch metadata length.".to_string())?;
                let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
                signatures.insert(
                    node_path.clone(),
                    Some(FileSignature {
                        kind,
                        len,
                        modified: remote_time(modified),
                    }),
                );
            }
            _ => return Err("Invalid remote file watch metadata state.".to_string()),
        }
    }

    Ok(signatures)
}

fn shell_list_dirs_script(remote_paths: &[String]) -> String {
    shell_script_with_args(LIST_DIRS_SCRIPT, remote_paths)
}

fn shell_read_with_info_script(remote_path: &str, max_bytes: Option<u64>) -> String {
    let max_bytes = max_bytes.map(|value| value.to_string()).unwrap_or_default();
    shell_script_with_args(READ_WITH_INFO_SCRIPT, &[remote_path.to_string(), max_bytes])
}

fn parse_metadata_line(
    access: &SshFileAccess,
    path: FileNodePath,
    line: &str,
    git_ignored: Option<bool>,
) -> Result<FileNodeInfo, String> {
    let mut fields = line.split('\t');
    let kind = parse_kind(fields.next().unwrap_or_default());
    let len = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "Invalid remote metadata length.".to_string())?;
    let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
    let readonly = fields.next().unwrap_or_default() == "1";
    let executable = fields.next().unwrap_or_default() == "1";
    Ok(access.node_info(path, kind, len, modified, readonly, executable, git_ignored))
}

fn parse_directory_output(
    requested: &[SshResolvedFileNode],
    raw: &str,
) -> Result<Vec<DirectoryListing>, String> {
    let mut listings = requested
        .iter()
        .map(|resolved| DirectoryListing {
            path: resolved.path.clone(),
            entries: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut current_index = None;

    for line in raw.lines() {
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\x1f');
        let marker = fields.next().unwrap_or_default();
        if marker == "CRAIC-DIR" {
            let remote_path = fields.next().unwrap_or_default();
            current_index = requested
                .iter()
                .position(|resolved| resolved.remote_path == remote_path);
            continue;
        }
        if marker != "CRAIC-ENTRY" {
            continue;
        }
        let Some(index) = current_index else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        let child = listings[index].path.join_child(name);
        listings[index].entries.push(child);
    }

    Ok(listings)
}

fn parse_read_output(
    access: &SshFileAccess,
    path: FileNodePath,
    stdout: Vec<u8>,
) -> Result<FileRead, String> {
    let Some(header_end) = stdout.iter().position(|byte| *byte == b'\n') else {
        return Err("Invalid remote file read response.".to_string());
    };
    let header = std::str::from_utf8(&stdout[..header_end])
        .map_err(|_| "Invalid remote file read header.".to_string())?;
    let mut fields = header.split('\t');
    if fields.next() != Some("CRAIC-FILE-READ") {
        return Err("Invalid remote file read marker.".to_string());
    }
    let kind = parse_kind(fields.next().unwrap_or_default());
    let len = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "Invalid remote file read length.".to_string())?;
    let modified = fields.next().and_then(|value| value.parse::<f64>().ok());
    let readonly = fields.next().unwrap_or_default() == "1";
    let executable = fields.next().unwrap_or_default() == "1";
    let readable = fields.next().unwrap_or_default() == "1";
    let info = access.node_info(path, kind, len, modified, readonly, executable, None);
    let bytes = readable.then(|| stdout[header_end + 1..].to_vec());
    Ok(FileRead { info, bytes })
}

fn validate_child_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Enter a name.".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Names cannot contain path separators.".to_string());
    }
    Ok(())
}

const SEARCH_SCRIPT: &str = include_str!("scripts/search.py");
