use crate::system::capabilities::files::{
    DirectoryListing, FileAccess, FileCopyRequest, FileDeleteRequest, FileKind, FileMoveRequest,
    FileNodeCapabilities, FileNodeInfo, FileOperation, FileOperationCallback, FileOperationError,
    FileOperationErrorKind, FileOperationEvent, FileOperationProgress, FileRead, FileReadRequest,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileWatchCallback,
    FileWatchChanges, FileWatchRequest, FileWatchSubscription, FileWriteMode, FileWritePayload,
    FileWriteRequest, file_operation_canceled,
};
use crate::system::path::{
    ArchiveFormat, FileNodePath, FileNodeRef, SystemRef, WorkspacePath, WorkspaceRef,
};
use gtk::prelude::*;
use gtk::{gio, glib};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use walkdir::WalkDir;

const LOCAL_FILE_MONITOR_RATE_LIMIT_MS: i32 = 250;
const LOCAL_FILE_MONITOR_STOP_POLL_MS: u64 = 250;
const LOCAL_FILE_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(750);
const LOCAL_ARCHIVE_PYTHON_CANDIDATES: &[&str] = &["python3", "python"];
const LOCAL_FILE_OPERATION_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct LocalFileAccess {
    system: SystemRef,
    workspace: WorkspaceRef,
    root_path: PathBuf,
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

impl LocalFileAccess {
    pub(crate) fn new(system: SystemRef, workspace: WorkspaceRef) -> Self {
        let root_path = PathBuf::from(&workspace.root.absolute);
        Self {
            system,
            workspace,
            root_path,
        }
    }

    fn root_node(&self) -> FileNodePath {
        self.workspace.root_node_path(&self.system)
    }

    fn native_workspace_path(&self, path: &FileNodePath) -> Result<WorkspacePath, String> {
        let display = file_node_display(path);
        let Some((root_id, system_id)) = path.root_ref() else {
            log::warn!(
                "local file node resolve denied workspace={} node={} reason=missing-root",
                self.workspace.display_name,
                display
            );
            return Err("File node does not belong to this local workspace.".to_string());
        };
        if root_id != self.workspace.id.as_str() || system_id != &self.system.id {
            log::warn!(
                "local file node resolve denied workspace={} node={} reason=wrong-root",
                self.workspace.display_name,
                display
            );
            return Err("File node does not belong to this local workspace.".to_string());
        }

        let mut parts = Vec::new();
        for node in path.nodes.iter().skip(1) {
            match node {
                FileNodeRef::NativeChild { name } => {
                    validate_native_child_name(name)?;
                    parts.push(name.as_str());
                }
                FileNodeRef::ArchiveRoot { .. } | FileNodeRef::ArchiveChild { .. } => {
                    return Err("File node is not a native workspace path.".to_string());
                }
                FileNodeRef::Root { .. } => {
                    return Err("File node has an unexpected root component.".to_string());
                }
            }
        }

        let relative = parts.join("/");
        let workspace_path =
            WorkspacePath::from_workspace_relative(&self.workspace.root, &relative);
        let local_path = if relative.is_empty() {
            self.root_path.clone()
        } else {
            self.root_path.join(&relative)
        };
        log::debug!(
            "local file node resolved workspace={} node={} local={}",
            self.workspace.display_name,
            display,
            local_path.display()
        );
        Ok(workspace_path)
    }

    fn local_path_for_workspace(&self, path: &WorkspacePath) -> Result<PathBuf, String> {
        let local_path = match path.relative.as_deref() {
            Some(relative) if !relative.is_empty() => self.root_path.join(relative),
            _ => PathBuf::from(&path.absolute),
        };

        if local_path.starts_with(&self.root_path) {
            Ok(local_path)
        } else {
            Err("Path is outside the workspace.".to_string())
        }
    }

    fn local_path_for_node(&self, path: &FileNodePath) -> Result<PathBuf, String> {
        self.local_path_for_workspace(&self.native_workspace_path(path)?)
    }

    fn workspace_path_for_local(&self, path: &Path) -> WorkspacePath {
        let relative = path
            .strip_prefix(&self.root_path)
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        WorkspacePath::from_workspace_relative(&self.workspace.root, &relative)
    }

    fn node_path_for_local(&self, path: &Path) -> FileNodePath {
        let workspace_path = self.workspace_path_for_local(path);
        self.workspace
            .node_path(&self.system, workspace_path.relative_or_empty())
    }

    fn info_for_native_node(&self, path: &FileNodePath) -> Result<FileNodeInfo, String> {
        let workspace_path = self.native_workspace_path(path)?;
        let local_path = self.local_path_for_workspace(&workspace_path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to inspect {}: {err}", path.display()))?;
        let mut kind = file_kind(&metadata);
        let mut capabilities = match kind {
            FileKind::Directory => {
                FileNodeCapabilities::native_directory(!metadata.permissions().readonly())
            }
            FileKind::File => FileNodeCapabilities::native_file(!metadata.permissions().readonly()),
            FileKind::Symlink | FileKind::Other => {
                FileNodeCapabilities::native_other(!metadata.permissions().readonly())
            }
            FileKind::Archive { .. } => unreachable!(),
        };
        if kind == FileKind::File
            && let Some(format) = path.file_name().and_then(ArchiveFormat::from_name)
        {
            let supported = archive_format_supported(format);
            kind = FileKind::Archive { format };
            capabilities = FileNodeCapabilities {
                listable: supported,
                ..FileNodeCapabilities::native_file(!metadata.permissions().readonly())
            };
            log::info!(
                "local archive detected workspace={} path={} format={} supported={}",
                self.workspace.display_name,
                path.display(),
                format,
                supported
            );
        }
        Ok(FileNodeInfo {
            path: path.clone(),
            display_name: path
                .file_name()
                .map(ToString::to_string)
                .unwrap_or_else(|| self.workspace.display_name.clone()),
            kind,
            len: Some(metadata.len()),
            modified: metadata.modified().ok(),
            owner: None,
            group: None,
            mode: Some(mode_bits(&metadata)),
            git_ignored: None,
            capabilities,
        })
    }

    fn archive_target(&self, path: &FileNodePath) -> Result<ArchiveTarget, String> {
        let archive_root_index = path
            .nodes
            .iter()
            .position(|node| matches!(node, FileNodeRef::ArchiveRoot { .. }))
            .ok_or_else(|| "File node does not open an archive.".to_string())?;
        if path.nodes[archive_root_index + 1..]
            .iter()
            .any(|node| matches!(node, FileNodeRef::ArchiveRoot { .. }))
        {
            log::warn!("local nested archive unsupported path={}", path.display());
            return Err("Nested archive browsing is unsupported for this provider.".to_string());
        }
        let format = match &path.nodes[archive_root_index] {
            FileNodeRef::ArchiveRoot { format } => *format,
            _ => unreachable!(),
        };
        if !archive_format_supported(format) {
            log::warn!(
                "local archive operation unsupported format={} path={}",
                format,
                path.display()
            );
            return Err(format!(
                "{} archive browsing is unsupported on this system.",
                format
            ));
        }
        let archive_node = FileNodePath {
            nodes: path.nodes[..archive_root_index].to_vec(),
        };
        let archive_path = self.local_path_for_node(&archive_node)?;
        let mut parts = Vec::new();
        for node in &path.nodes[archive_root_index + 1..] {
            match node {
                FileNodeRef::ArchiveChild { name } => {
                    validate_archive_child_name(name)?;
                    parts.push(name.as_str());
                }
                _ => return Err("Invalid archive file node path.".to_string()),
            }
        }
        let member = (!parts.is_empty()).then(|| parts.join("/"));
        log::debug!(
            "local archive target resolved workspace={} archive={} format={} member={}",
            self.workspace.display_name,
            archive_node.display(),
            format,
            member.as_deref().unwrap_or("")
        );
        Ok(ArchiveTarget {
            archive_node,
            archive_path,
            format,
            member,
        })
    }

    fn archive_members(&self, target: &ArchiveTarget) -> Result<Vec<ArchiveMember>, String> {
        log::info!(
            "local archive listing start workspace={} archive={} format={}",
            self.workspace.display_name,
            target.archive_node.display(),
            target.format
        );
        let output = self.run_archive_python("list", target, &[])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            log::warn!(
                "local archive listing failed archive={} format={} status={} stderr={}",
                target.archive_node.display(),
                target.format,
                output.status,
                stderr
            );
            return Err(if stderr.is_empty() {
                "Unable to inspect archive.".to_string()
            } else {
                format!("Unable to inspect archive: {stderr}")
            });
        }
        let mut listing: ArchiveListOutput = serde_json::from_slice(&output.stdout)
            .map_err(|err| format!("Invalid archive listing response: {err}"))?;
        if listing.invalid > 0 {
            log::warn!(
                "local archive unsafe members rejected archive={} format={} count={}",
                target.archive_node.display(),
                target.format,
                listing.invalid
            );
        }
        listing.members.retain(|member| {
            if archive_member_name_is_safe(&member.name) {
                true
            } else {
                log::warn!(
                    "local archive unsafe member skipped archive={} member={}",
                    target.archive_node.display(),
                    member.name
                );
                false
            }
        });
        log::info!(
            "local archive listing complete archive={} members={}",
            target.archive_node.display(),
            listing.members.len()
        );
        Ok(listing.members)
    }

    fn run_archive_python(
        &self,
        operation: &str,
        target: &ArchiveTarget,
        args: &[String],
    ) -> Result<Output, String> {
        let mut missing_python = false;
        for program in LOCAL_ARCHIVE_PYTHON_CANDIDATES {
            let mut command = Command::new(program);
            command
                .arg("-c")
                .arg(match operation {
                    "read" => ARCHIVE_READ_SCRIPT,
                    _ => ARCHIVE_LIST_SCRIPT,
                })
                .arg(archive_format_arg(target.format))
                .arg(&target.archive_path);
            for arg in args {
                command.arg(arg);
            }
            match command.output() {
                Ok(output) => return Ok(output),
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    missing_python = true;
                }
                Err(err) => {
                    return Err(format!(
                        "Unable to start archive {operation} helper for {}: {err}",
                        target.archive_node.display()
                    ));
                }
            }
        }
        if missing_python {
            Err("Python is required to browse local archives.".to_string())
        } else {
            Err("No local archive helper is configured.".to_string())
        }
    }

    fn list_archive_dir(&self, path: &FileNodePath) -> Result<DirectoryListing, String> {
        let target = self.archive_target(path)?;
        let members = self.archive_members(&target)?;
        let tree = ArchiveTree::from_members(&members);
        let prefix = target.member.as_deref().unwrap_or("");
        if !prefix.is_empty() {
            let exact = members
                .iter()
                .find(|member| member.name.trim_end_matches('/') == prefix);
            if !tree.contains_dir(prefix) {
                match exact {
                    Some(member) if member.kind == "dir" => {}
                    Some(_)
                        if path
                            .file_name()
                            .and_then(ArchiveFormat::from_name)
                            .is_some() =>
                    {
                        log::warn!(
                            "local nested archive listing unsupported archive={} member={}",
                            target.archive_node.display(),
                            prefix
                        );
                        return Err(
                            "Nested archive browsing is unsupported for this provider.".to_string()
                        );
                    }
                    Some(_) => return Err("Select a folder or archive to list.".to_string()),
                    None => return Err(format!("Archive member not found: {prefix}")),
                }
            }
        }

        let mut entries = tree
            .child_names(prefix)
            .into_iter()
            .map(|child_name| path.join_child(child_name))
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.display().cmp(&right.display()));
        log::info!(
            "local archive directory listed workspace={} archive={} dir={} entries={}",
            self.workspace.display_name,
            target.archive_node.display(),
            path.display(),
            entries.len()
        );
        Ok(DirectoryListing {
            path: path.clone(),
            entries,
        })
    }

    fn info_for_archive_node(&self, path: &FileNodePath) -> Result<FileNodeInfo, String> {
        let target = self.archive_target(path)?;
        let Some(member_path) = target.member.as_deref() else {
            let metadata = fs::symlink_metadata(&target.archive_path).map_err(|err| {
                format!(
                    "Unable to inspect archive {}: {err}",
                    target.archive_node.display()
                )
            })?;
            if !metadata.is_file() {
                return Err("Select an archive file to browse.".to_string());
            }
            return Ok(FileNodeInfo {
                path: path.clone(),
                display_name: target
                    .archive_node
                    .file_name()
                    .unwrap_or("archive")
                    .to_string(),
                kind: FileKind::Directory,
                len: None,
                modified: metadata.modified().ok(),
                owner: None,
                group: None,
                mode: None,
                git_ignored: None,
                capabilities: FileNodeCapabilities::virtual_directory(),
            });
        };
        let members = self.archive_members(&target)?;
        let exact = members
            .iter()
            .find(|member| member.name.trim_end_matches('/') == member_path);
        let has_children = members.iter().any(|member| {
            member
                .name
                .trim_end_matches('/')
                .strip_prefix(member_path)
                .is_some_and(|suffix| suffix.starts_with('/'))
        });
        let kind = if has_children || exact.is_some_and(|member| member.kind == "dir") {
            FileKind::Directory
        } else if exact.is_some_and(|member| member.kind == "symlink") {
            FileKind::Symlink
        } else if exact.is_some_and(|member| member.kind == "file") {
            if let Some(format) = path.file_name().and_then(ArchiveFormat::from_name) {
                FileKind::Archive { format }
            } else {
                FileKind::File
            }
        } else if exact.is_some() {
            FileKind::Other
        } else {
            return Err(format!("Archive member not found: {member_path}"));
        };
        let capabilities = match kind {
            FileKind::Directory => FileNodeCapabilities::virtual_directory(),
            FileKind::Archive { .. } => FileNodeCapabilities {
                listable: false,
                ..FileNodeCapabilities::virtual_file()
            },
            FileKind::File => FileNodeCapabilities::virtual_file(),
            FileKind::Symlink | FileKind::Other => FileNodeCapabilities::default(),
        };
        Ok(FileNodeInfo {
            path: path.clone(),
            display_name: path.file_name().unwrap_or(member_path).to_string(),
            kind,
            len: exact.and_then(|member| member.len),
            modified: exact.and_then(|member| {
                member
                    .modified
                    .map(|secs| UNIX_EPOCH + Duration::from_secs_f64(secs.max(0.0)))
            }),
            owner: None,
            group: None,
            mode: exact.and_then(|member| member.mode),
            git_ignored: None,
            capabilities,
        })
    }

    fn read_archive_member(
        &self,
        path: &FileNodePath,
        max_bytes: Option<u64>,
    ) -> Result<FileRead, String> {
        let info = self.info_for_archive_node(path)?;
        if !info.kind.is_file() {
            return Ok(FileRead { info, bytes: None });
        }
        if let Some(max_bytes) = max_bytes
            && info.len.is_some_and(|len| len > max_bytes)
        {
            return Ok(FileRead { info, bytes: None });
        }
        let target = self.archive_target(path)?;
        let member = target
            .member
            .as_deref()
            .ok_or_else(|| "Select a file to read.".to_string())?;
        log::info!(
            "local archive read start workspace={} archive={} member={} max_bytes={:?}",
            self.workspace.display_name,
            target.archive_node.display(),
            member,
            max_bytes
        );
        let max_arg = max_bytes
            .map(|max_bytes| max_bytes.to_string())
            .unwrap_or_else(|| "-1".to_string());
        let output = self.run_archive_python("read", &target, &[member.to_string(), max_arg])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            log::warn!(
                "local archive read failed archive={} member={} status={} stderr={}",
                target.archive_node.display(),
                member,
                output.status,
                stderr
            );
            return Err(if stderr.is_empty() {
                "Unable to read archive member.".to_string()
            } else {
                format!("Unable to read archive member: {stderr}")
            });
        }
        if let Some(max_bytes) = max_bytes
            && output.stdout.len() as u64 > max_bytes
        {
            return Ok(FileRead { info, bytes: None });
        }
        log::info!(
            "local archive read complete archive={} member={} bytes={}",
            target.archive_node.display(),
            member,
            output.stdout.len()
        );
        Ok(FileRead {
            info,
            bytes: Some(output.stdout),
        })
    }

    fn deny_virtual_write(&self, operation: &str, path: &FileNodePath) -> Result<(), String> {
        log::warn!(
            "local virtual write denied operation={} workspace={} path={}",
            operation,
            self.workspace.display_name,
            path.display()
        );
        Err("Archive contents are read-only.".to_string())
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

    fn io_error(
        operation: FileOperation,
        source: Option<FileNodePath>,
        destination: Option<FileNodePath>,
        action: &str,
        err: std::io::Error,
    ) -> FileOperationError {
        Self::operation_error(
            operation,
            local_io_error_kind(&err),
            source,
            destination,
            format!("{action}: {err}"),
        )
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
        if request.path.contains_archive() {
            let read = self
                .read_archive_member(&request.path, request.max_bytes)
                .map_err(|err| {
                    Self::operation_error(
                        operation,
                        FileOperationErrorKind::Unsupported,
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
                        total_bytes: bytes.len() as u64,
                        completed_files: 1,
                        total_files: 1,
                        destination: None,
                    },
                );
            }
            return Ok(read);
        }

        let local_path = self.local_path_for_node(&request.path).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::OutsideWorkspace,
                Some(request.path.clone()),
                None,
                err,
            )
        })?;
        let metadata = fs::symlink_metadata(&local_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(request.path.clone()),
                None,
                &format!("Unable to inspect {}", request.path.display()),
                err,
            )
        })?;
        let info = self.info_for_native_node(&request.path).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::Io,
                Some(request.path.clone()),
                None,
                err,
            )
        })?;
        if !matches!(info.kind, FileKind::File | FileKind::Archive { .. }) {
            return Ok(FileRead { info, bytes: None });
        }
        if request
            .max_bytes
            .is_some_and(|max_bytes| metadata.len() > max_bytes)
        {
            return Ok(FileRead { info, bytes: None });
        }

        let total_bytes = metadata.len();
        let mut file = fs::File::open(&local_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(request.path.clone()),
                None,
                &format!("Unable to read {}", request.path.display()),
                err,
            )
        })?;
        let mut bytes = Vec::new();
        let mut completed_bytes = 0u64;
        let mut buffer = vec![0u8; LOCAL_FILE_OPERATION_CHUNK_BYTES];
        loop {
            Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
            let read = file.read(&mut buffer).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(request.path.clone()),
                    None,
                    &format!("Unable to read {}", request.path.display()),
                    err,
                )
            })?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            completed_bytes = completed_bytes.saturating_add(read as u64);
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_bytes,
                    total_bytes,
                    completed_files: (completed_bytes == total_bytes) as u64,
                    total_files: 1,
                    destination: None,
                },
            );
        }

        Ok(FileRead {
            info,
            bytes: Some(bytes),
        })
    }

    fn perform_write_node(
        &self,
        request: &FileWriteRequest,
        callback: &FileOperationCallback<()>,
    ) -> Result<(), FileOperationError> {
        let operation = FileOperation::Write;
        Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
        if request.path.contains_archive() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.path.clone()),
                None,
                "Archive contents are read-only.",
            ));
        }
        let local_path = self.local_path_for_node(&request.path).map_err(|err| {
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
            fs::create_dir(&local_path).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(request.path.clone()),
                    None,
                    &format!("Unable to create {}", request.path.display()),
                    err,
                )
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
        if let Ok(metadata) = fs::metadata(&local_path)
            && !metadata.is_file()
        {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.path.clone()),
                None,
                "Select a file to write.",
            ));
        }

        let mut options = fs::OpenOptions::new();
        options.write(true);
        match request.mode {
            FileWriteMode::CreateNew => {
                options.create_new(true);
            }
            FileWriteMode::Replace => {
                options.create(true).truncate(true);
            }
            FileWriteMode::Append => {
                options.create(false).append(true);
            }
        }
        let mut file = options.open(&local_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(request.path.clone()),
                None,
                &format!("Unable to write {}", request.path.display()),
                err,
            )
        })?;
        let total_bytes = contents.len() as u64;
        let mut completed_bytes = 0u64;
        for chunk in contents.chunks(LOCAL_FILE_OPERATION_CHUNK_BYTES) {
            Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
            file.write_all(chunk).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(request.path.clone()),
                    None,
                    &format!("Unable to write {}", request.path.display()),
                    err,
                )
            })?;
            completed_bytes = completed_bytes.saturating_add(chunk.len() as u64);
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    destination: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_bytes,
                    total_bytes,
                    completed_files: (completed_bytes == total_bytes) as u64,
                    total_files: 1,
                },
            );
        }
        if total_bytes == 0 {
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
        }
        Ok(())
    }

    fn perform_copy_node(
        &self,
        request: &FileCopyRequest,
        operation: FileOperation,
        callback: &FileOperationCallback<FileNodePath>,
    ) -> Result<FileNodePath, FileOperationError> {
        Self::check_canceled(operation, &request.source, &request.cancel_requested)?;
        if request.source.contains_archive() || request.destination.contains_archive() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.source.clone()),
                Some(request.destination.clone()),
                "Archive contents are read-only.",
            ));
        }
        if request.source == request.destination {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::AlreadyExists,
                Some(request.source.clone()),
                Some(request.destination.clone()),
                format!("{} already exists.", request.destination.display()),
            ));
        }
        let source_path = self.local_path_for_node(&request.source).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::OutsideWorkspace,
                Some(request.source.clone()),
                Some(request.destination.clone()),
                err,
            )
        })?;
        let destination_path = self
            .local_path_for_node(&request.destination)
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::OutsideWorkspace,
                    Some(request.source.clone()),
                    Some(request.destination.clone()),
                    err,
                )
            })?;
        if destination_path.exists() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::AlreadyExists,
                Some(request.source.clone()),
                Some(request.destination.clone()),
                format!("{} already exists.", request.destination.display()),
            ));
        }
        let totals = local_copy_totals(
            &source_path,
            &request.source,
            operation,
            &request.destination,
        )?;
        let mut progress = LocalCopyProgress {
            completed_bytes: 0,
            completed_files: 0,
            total_bytes: totals.bytes,
            total_files: totals.files,
        };
        self.copy_entry(
            &source_path,
            &destination_path,
            &request.source,
            &request.destination,
            operation,
            &request.cancel_requested,
            &mut progress,
            callback,
        )?;
        Ok(request.destination.clone())
    }

    fn copy_entry(
        &self,
        source_path: &Path,
        destination_path: &Path,
        source: &FileNodePath,
        destination: &FileNodePath,
        operation: FileOperation,
        cancel_requested: &Option<crate::system::capabilities::files::FileCancellation>,
        progress: &mut LocalCopyProgress,
        callback: &FileOperationCallback<FileNodePath>,
    ) -> Result<(), FileOperationError> {
        Self::check_canceled(operation, source, cancel_requested)?;
        let metadata = fs::symlink_metadata(source_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(source.clone()),
                Some(destination.clone()),
                &format!("Unable to inspect {}", source.display()),
                err,
            )
        })?;
        if metadata.is_dir() {
            fs::create_dir(destination_path).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(source.clone()),
                    Some(destination.clone()),
                    &format!("Unable to create {}", destination.display()),
                    err,
                )
            })?;
            progress.completed_files = progress.completed_files.saturating_add(1);
            Self::emit_progress(
                callback,
                progress.to_event(operation, source, destination, destination),
            );
            for entry in fs::read_dir(source_path).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(source.clone()),
                    Some(destination.clone()),
                    &format!("Unable to list {}", source.display()),
                    err,
                )
            })? {
                let entry = entry.map_err(|err| {
                    Self::io_error(
                        operation,
                        Some(source.clone()),
                        Some(destination.clone()),
                        "Unable to read directory entry",
                        err,
                    )
                })?;
                let name = entry.file_name();
                let name = name.to_string_lossy().to_string();
                let child_source = source.join_child(&name);
                let child_destination = destination.join_child(&name);
                let child_destination_path = destination_path.join(&name);
                if child_destination_path.exists() {
                    return Err(Self::operation_error(
                        operation,
                        FileOperationErrorKind::AlreadyExists,
                        Some(child_source),
                        Some(child_destination.clone()),
                        format!("{} already exists.", child_destination.display()),
                    ));
                }
                self.copy_entry(
                    &entry.path(),
                    &child_destination_path,
                    &child_source,
                    &child_destination,
                    operation,
                    cancel_requested,
                    progress,
                    callback,
                )?;
            }
            return Ok(());
        }

        if metadata.file_type().is_symlink() {
            copy_local_symlink(
                source_path,
                destination_path,
                operation,
                source,
                destination,
            )?;
            progress.completed_files = progress.completed_files.saturating_add(1);
            Self::emit_progress(
                callback,
                progress.to_event(operation, source, destination, destination),
            );
            return Ok(());
        }

        if !metadata.is_file() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(source.clone()),
                Some(destination.clone()),
                "Only files, folders, and symlinks can be copied.",
            ));
        }

        let mut source_file = fs::File::open(source_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(source.clone()),
                Some(destination.clone()),
                &format!("Unable to read {}", source.display()),
                err,
            )
        })?;
        let mut destination_file = fs::File::create(destination_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(source.clone()),
                Some(destination.clone()),
                &format!("Unable to write {}", destination.display()),
                err,
            )
        })?;
        let mut buffer = vec![0u8; LOCAL_FILE_OPERATION_CHUNK_BYTES];
        loop {
            Self::check_canceled(operation, source, cancel_requested)?;
            let read = source_file.read(&mut buffer).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(source.clone()),
                    Some(destination.clone()),
                    &format!("Unable to read {}", source.display()),
                    err,
                )
            })?;
            if read == 0 {
                break;
            }
            destination_file.write_all(&buffer[..read]).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(source.clone()),
                    Some(destination.clone()),
                    &format!("Unable to write {}", destination.display()),
                    err,
                )
            })?;
            progress.completed_bytes = progress.completed_bytes.saturating_add(read as u64);
            Self::emit_progress(
                callback,
                progress.to_event(operation, source, destination, destination),
            );
        }
        progress.completed_files = progress.completed_files.saturating_add(1);
        Self::emit_progress(
            callback,
            progress.to_event(operation, source, destination, destination),
        );
        Ok(())
    }

    fn perform_move_node(
        &self,
        request: &FileMoveRequest,
        callback: &FileOperationCallback<FileNodePath>,
    ) -> Result<FileNodePath, FileOperationError> {
        let operation = FileOperation::Move;
        Self::check_canceled(operation, &request.source, &request.cancel_requested)?;
        if request.source.contains_archive() || request.destination_parent.contains_archive() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.source.clone()),
                Some(request.destination()),
                "Archive contents are read-only.",
            ));
        }
        validate_child_name(&request.new_name).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::InvalidName,
                Some(request.source.clone()),
                Some(request.destination()),
                err,
            )
        })?;
        let destination = request.destination();
        if request.source == destination {
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.source.clone()),
                    destination: Some(destination.clone()),
                    current_path: Some(destination.clone()),
                    completed_files: 1,
                    total_files: 1,
                    ..FileOperationProgress::new(operation)
                },
            );
            return Ok(destination);
        }
        let source_path = self.local_path_for_node(&request.source).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::OutsideWorkspace,
                Some(request.source.clone()),
                Some(destination.clone()),
                err,
            )
        })?;
        let destination_path = self.local_path_for_node(&destination).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::OutsideWorkspace,
                Some(request.source.clone()),
                Some(destination.clone()),
                err,
            )
        })?;
        if destination_path.exists() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::AlreadyExists,
                Some(request.source.clone()),
                Some(destination.clone()),
                format!("{} already exists.", destination.display()),
            ));
        }
        match fs::rename(&source_path, &destination_path) {
            Ok(()) => {
                Self::emit_progress(
                    callback,
                    FileOperationProgress {
                        operation,
                        source: Some(request.source.clone()),
                        destination: Some(destination.clone()),
                        current_path: Some(destination.clone()),
                        completed_files: 1,
                        total_files: 1,
                        ..FileOperationProgress::new(operation)
                    },
                );
                Ok(destination)
            }
            Err(err) if local_io_error_is_cross_device(&err) => {
                let copy_request = FileCopyRequest {
                    source: request.source.clone(),
                    destination: destination.clone(),
                    cancel_requested: request.cancel_requested.clone(),
                };
                self.perform_copy_node(&copy_request, operation, callback)?;
                self.perform_delete(
                    &FileDeleteRequest {
                        path: request.source.clone(),
                        cancel_requested: request.cancel_requested.clone(),
                    },
                    None,
                )?;
                Ok(destination)
            }
            Err(err) => Err(Self::io_error(
                operation,
                Some(request.source.clone()),
                Some(destination.clone()),
                "Unable to move file node",
                err,
            )),
        }
    }

    fn perform_delete(
        &self,
        request: &FileDeleteRequest,
        callback: Option<&FileOperationCallback<()>>,
    ) -> Result<(), FileOperationError> {
        let operation = FileOperation::Delete;
        Self::check_canceled(operation, &request.path, &request.cancel_requested)?;
        if request.path.contains_archive() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.path.clone()),
                None,
                "Archive contents are read-only.",
            ));
        }
        let local_path = self.local_path_for_node(&request.path).map_err(|err| {
            Self::operation_error(
                operation,
                FileOperationErrorKind::OutsideWorkspace,
                Some(request.path.clone()),
                None,
                err,
            )
        })?;
        let metadata = fs::symlink_metadata(&local_path).map_err(|err| {
            Self::io_error(
                operation,
                Some(request.path.clone()),
                None,
                "Unable to inspect path",
                err,
            )
        })?;
        if metadata.is_dir() {
            fs::remove_dir_all(&local_path).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(request.path.clone()),
                    None,
                    "Unable to delete folder",
                    err,
                )
            })?;
        } else {
            fs::remove_file(&local_path).map_err(|err| {
                Self::io_error(
                    operation,
                    Some(request.path.clone()),
                    None,
                    "Unable to delete file",
                    err,
                )
            })?;
        }
        if let Some(callback) = callback {
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
        }
        Ok(())
    }
}

impl FileAccess for LocalFileAccess {
    fn workspace(&self) -> WorkspaceRef {
        self.workspace.clone()
    }

    fn root(&self) -> FileNodePath {
        self.root_node()
    }

    fn watch(
        &self,
        request: FileWatchRequest,
        callback: FileWatchCallback,
    ) -> Result<FileWatchSubscription, String> {
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
        let recursive = request.recursive;
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
            "local file watch registered workspace={} paths={} recursive={} mode=gio",
            self.workspace.display_name,
            local_paths.len(),
            request.recursive
        );
        let root_path = self.root_path.clone();
        let system = self.system.clone();
        let workspace = self.workspace.clone();
        Ok(FileWatchSubscription::spawn_thread(
            label,
            move |stop_receiver| {
                run_local_gio_file_monitor(
                    local_paths,
                    root_path,
                    system,
                    workspace,
                    recursive,
                    stop_receiver,
                    callback,
                );
            },
        ))
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

    fn read_with_info(&self, request: FileReadRequest, callback: FileOperationCallback<FileRead>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "local file read worker start path={} max_bytes={:?}",
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
                "local file write worker start path={} payload={}",
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
                "local file copy worker start source={} destination={}",
                request.source.display(),
                request.destination.display()
            );
            let result = access.perform_copy_node(&request, FileOperation::Copy, &callback);
            callback(FileOperationEvent::Finished(result));
        });
    }

    fn move_node(&self, request: FileMoveRequest, callback: FileOperationCallback<FileNodePath>) {
        let access = self.clone();
        thread::spawn(move || {
            log::info!(
                "local file move worker start source={} destination_parent={} new_name={}",
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
                "local file delete worker start path={}",
                request.path.display()
            );
            let result = access.perform_delete(&request, Some(&callback));
            callback(FileOperationEvent::Finished(result));
        });
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

fn run_local_gio_file_monitor(
    local_paths: Vec<(FileNodePath, PathBuf)>,
    root_path: PathBuf,
    system: SystemRef,
    workspace: WorkspaceRef,
    recursive: bool,
    stop_receiver: mpsc::Receiver<()>,
    callback: FileWatchCallback,
) {
    let display_paths = local_paths
        .iter()
        .map(|(_, path)| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let context = glib::MainContext::new();
    let context_result = context.with_thread_default(|| {
        let main_loop = glib::MainLoop::new(Some(&context), false);
        let flags = gio::FileMonitorFlags::WATCH_MOVES | gio::FileMonitorFlags::SEND_MOVED;
        let mut monitors = Vec::new();

        for (_, local_path) in &local_paths {
            let file = gio::File::for_path(local_path);
            let monitor_result = if recursive || local_path.is_dir() {
                file.monitor_directory(flags, None::<&gio::Cancellable>)
            } else {
                file.monitor_file(flags, None::<&gio::Cancellable>)
            };
            let monitor = match monitor_result {
                Ok(monitor) => monitor,
                Err(err) => {
                    log::warn!(
                        "local gio file watch unavailable path={} watched_paths={} err={err}; falling back to polling",
                        local_path.display(),
                        local_paths.len()
                    );
                    run_local_signature_file_monitor(local_paths.clone(), stop_receiver, callback);
                    return;
                }
            };

            monitor.set_rate_limit(LOCAL_FILE_MONITOR_RATE_LIMIT_MS);
            monitor.connect_changed({
                let root_path = root_path.clone();
                let system = system.clone();
                let workspace = workspace.clone();
                let callback = Arc::clone(&callback);

                move |_, file, other_file, event_type| {
                    if !local_file_monitor_event_should_notify(event_type) {
                        return;
                    }

                    let changes = local_file_monitor_changes(&root_path, &system, &workspace, file, other_file);
                    if changes.is_empty() {
                        return;
                    }

                    log::debug!(
                        "local gio file watch event event_type={event_type:?} changed_paths={}",
                        changes.len()
                    );
                    callback(changes);
                }
            });
            monitors.push(monitor);
        }

        if monitors.is_empty() {
            log::warn!("local gio file watch has no paths; falling back to polling");
            run_local_signature_file_monitor(local_paths, stop_receiver, callback);
            return;
        }

        log::info!("local gio file watch started watched_paths={}", monitors.len());

        let stop_loop = main_loop.clone();
        let stop_source = glib::timeout_source_new(
            Duration::from_millis(LOCAL_FILE_MONITOR_STOP_POLL_MS),
            Some("craic-local-file-monitor-stop"),
            glib::Priority::DEFAULT,
            move || {
                if stop_receiver.try_recv().is_ok() {
                    stop_loop.quit();
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            },
        );
        stop_source.attach(Some(&context));

        main_loop.run();
        for monitor in monitors {
            monitor.cancel();
        }
    });

    if let Err(err) = context_result {
        log::warn!(
            "local gio file watch failed to attach thread context paths={} err={err}",
            display_paths
        );
    }
}

fn run_local_signature_file_monitor(
    local_paths: Vec<(FileNodePath, PathBuf)>,
    stop_receiver: mpsc::Receiver<()>,
    callback: FileWatchCallback,
) {
    let mut previous_signatures: Option<HashMap<FileNodePath, Option<FileSignature>>> = None;

    loop {
        let mut next_signatures = HashMap::new();
        for (node_path, local_path) in &local_paths {
            match local_file_signature(local_path) {
                Ok(signature) => {
                    next_signatures.insert(node_path.clone(), signature);
                }
                Err(err) => {
                    log::warn!(
                        "local file watch fallback metadata failed path={} err={err}",
                        local_path.display()
                    );
                    next_signatures.insert(node_path.clone(), None);
                }
            }
        }

        if let Some(previous) = &previous_signatures {
            let changes = changed_signature_paths(previous, &next_signatures);
            if !changes.is_empty() {
                log::debug!(
                    "local file watch fallback change detected watched_paths={} changed_paths={}",
                    next_signatures.len(),
                    changes.len()
                );
                callback(changes);
            }
        } else {
            log::debug!(
                "local file watch fallback initial snapshot watched_paths={}",
                next_signatures.len()
            );
        }
        previous_signatures = Some(next_signatures);

        if stop_receiver
            .recv_timeout(LOCAL_FILE_FALLBACK_POLL_INTERVAL)
            .is_ok()
        {
            break;
        }
    }
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

fn local_file_monitor_event_should_notify(event_type: gio::FileMonitorEvent) -> bool {
    !matches!(
        event_type,
        gio::FileMonitorEvent::AttributeChanged
            | gio::FileMonitorEvent::PreUnmount
            | gio::FileMonitorEvent::Unmounted
    )
}

fn local_file_monitor_changes(
    root_path: &Path,
    system: &SystemRef,
    workspace: &WorkspaceRef,
    file: &gio::File,
    other_file: Option<&gio::File>,
) -> FileWatchChanges {
    let mut changes = FileWatchChanges::new();
    collect_local_file_monitor_path(&mut changes, root_path, system, workspace, file);
    if let Some(other_file) = other_file {
        collect_local_file_monitor_path(&mut changes, root_path, system, workspace, other_file);
    }
    changes
}

fn collect_local_file_monitor_path(
    changes: &mut FileWatchChanges,
    root_path: &Path,
    system: &SystemRef,
    workspace: &WorkspaceRef,
    file: &gio::File,
) {
    let Some(path) = file.path() else {
        return;
    };
    if let Some(node_path) = file_node_path_for_local_root(root_path, system, workspace, &path) {
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
