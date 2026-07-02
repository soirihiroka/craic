use crate::gitignore;
use crate::system::capabilities::files::{
    DirectoryListing, FileAccess, FileKind, FileNodeCapabilities, FileNodeInfo, FileRead,
    FileSearchMatch, FileSearchOutput, FileSearchQuery, FileSignature, FileWatchCallback,
    FileWatchChanges, FileWatchRequest, FileWatchSubscription,
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
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, mpsc};
use std::time::{Duration, UNIX_EPOCH};
use walkdir::WalkDir;

const LOCAL_FILE_MONITOR_RATE_LIMIT_MS: i32 = 250;
const LOCAL_FILE_MONITOR_STOP_POLL_MS: u64 = 250;
const LOCAL_FILE_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(750);
const LOCAL_ARCHIVE_PYTHON_CANDIDATES: &[&str] = &["python3", "python"];

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
        let mut infos = paths
            .iter()
            .map(|path| self.info(path))
            .collect::<Result<Vec<_>, _>>()?;
        apply_local_git_ignore_to_infos(&self.root_path, &mut infos);
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

    fn read_with_info(
        &self,
        path: &FileNodePath,
        max_bytes: Option<u64>,
    ) -> Result<FileRead, String> {
        if path.contains_archive() {
            return self.read_archive_member(path, max_bytes);
        }
        let local_path = self.local_path_for_node(path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to read {}: {err}", path.display()))?;
        let info = self.info_for_native_node(path)?;
        let bytes = if matches!(info.kind, FileKind::File | FileKind::Archive { .. })
            && max_bytes.is_none_or(|max_bytes| metadata.len() <= max_bytes)
        {
            Some(
                fs::read(&local_path)
                    .map_err(|err| format!("Unable to read {}: {err}", path.display()))?,
            )
        } else {
            None
        };
        Ok(FileRead { info, bytes })
    }

    fn write_bytes(&self, path: &FileNodePath, contents: &[u8]) -> Result<(), String> {
        if path.contains_archive() {
            return self.deny_virtual_write("write_bytes", path);
        }
        let local_path = self.local_path_for_node(path)?;
        fs::write(&local_path, contents)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))
    }

    fn write_text(&self, path: &FileNodePath, contents: &str) -> Result<(), String> {
        if path.contains_archive() {
            return self.deny_virtual_write("write_text", path);
        }
        let local_path = self.local_path_for_node(path)?;
        let metadata = fs::metadata(&local_path)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))?;
        if !metadata.is_file() {
            return Err("Select a file to edit.".to_string());
        }
        fs::write(&local_path, contents)
            .map_err(|err| format!("Unable to write {}: {err}", path.display()))
    }

    fn create_file(&self, parent: &FileNodePath, name: &str) -> Result<FileNodePath, String> {
        if parent.contains_archive() {
            log::warn!(
                "local archive create file denied parent={}",
                parent.display()
            );
            return Err("Archive contents are read-only.".to_string());
        }
        validate_child_name(name)?;
        let path = parent.join_child(name);
        let local_path = self.local_path_for_node(&path)?;
        if let Some(parent) = local_path.parent()
            && !parent.is_dir()
        {
            return Err("Parent folder does not exist.".to_string());
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&local_path)
            .map(|_| path)
            .map_err(|err| format!("Unable to create file: {err}"))
    }

    fn create_dir(&self, parent: &FileNodePath, name: &str) -> Result<FileNodePath, String> {
        if parent.contains_archive() {
            log::warn!(
                "local archive create folder denied parent={}",
                parent.display()
            );
            return Err("Archive contents are read-only.".to_string());
        }
        validate_child_name(name)?;
        let path = parent.join_child(name);
        let local_path = self.local_path_for_node(&path)?;
        fs::create_dir(&local_path)
            .map(|_| path)
            .map_err(|err| format!("Unable to create folder: {err}"))
    }

    fn rename(
        &self,
        source: &FileNodePath,
        destination_parent: &FileNodePath,
        new_name: &str,
    ) -> Result<FileNodePath, String> {
        if source.contains_archive() || destination_parent.contains_archive() {
            log::warn!(
                "local archive rename denied source={} destination_parent={}",
                source.display(),
                destination_parent.display()
            );
            return Err("Archive contents are read-only.".to_string());
        }
        validate_child_name(new_name)?;
        let destination = destination_parent.join_child(new_name);
        let source_path = self.local_path_for_node(source)?;
        let destination_path = self.local_path_for_node(&destination)?;
        if destination_path.exists() {
            return Err("Destination already exists.".to_string());
        }
        fs::rename(&source_path, &destination_path)
            .map(|_| destination)
            .map_err(|err| format!("Unable to rename: {err}"))
    }

    fn delete(&self, path: &FileNodePath) -> Result<(), String> {
        if path.contains_archive() {
            return self.deny_virtual_write("delete", path);
        }
        let local_path = self.local_path_for_node(path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to inspect path: {err}"))?;
        if metadata.is_dir() {
            fs::remove_dir_all(&local_path).map_err(|err| format!("Unable to delete folder: {err}"))
        } else {
            fs::remove_file(&local_path).map_err(|err| format!("Unable to delete file: {err}"))
        }
    }

    fn search_text(&self, query: FileSearchQuery) -> Result<FileSearchOutput, String> {
        if !query.root.is_native() {
            return Err("Searching archive contents is unsupported.".to_string());
        }
        let root = self.local_path_for_node(&query.root)?;
        let matcher = build_search_regex(&query)?;
        let excluded_names = query.excluded_names.clone();
        let mut matches = Vec::new();
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
            let path = self.node_path_for_local(entry.path());
            collect_file_matches(&path, &text, &matcher, &query, &mut matches, &mut limited);
            if limited {
                break;
            }
        }

        matches.sort_by(|left, right| {
            left.path
                .display()
                .cmp(&right.path.display())
                .then_with(|| left.start.cmp(&right.start))
        });
        log::info!(
            "file search complete provider=local workspace={} matches={} limited={}",
            self.workspace.display_name,
            matches.len(),
            limited
        );
        Ok(FileSearchOutput { matches, limited })
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

fn apply_local_git_ignore_to_infos(root_path: &Path, infos: &mut [FileNodeInfo]) {
    let checks = infos
        .iter()
        .filter(|info| info.capabilities.native)
        .filter_map(|info| {
            let path = info.path.native_relative()?;
            (!path.is_empty()).then(|| gitignore::IgnoreCheck {
                path,
                is_dir: info.kind == FileKind::Directory,
            })
        })
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return;
    }

    let ignored_paths = match gitignore::check_ignored_paths(root_path, &checks) {
        Ok(ignored_paths) => ignored_paths,
        Err(err) => {
            log::debug!(
                "local info git ignore unavailable root={} err={err}",
                root_path.display()
            );
            return;
        }
    };
    for info in infos {
        if let Some(path) = info.path.native_relative()
            && !path.is_empty()
        {
            info.git_ignored = Some(ignored_paths.contains(&path));
        }
    }
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
    matches: &mut Vec<FileSearchMatch>,
    limited: &mut bool,
) {
    for found in matcher.find_iter(text) {
        if found.is_empty() {
            continue;
        }
        matches.push(FileSearchMatch {
            path: path.clone(),
            line_number: line_number_for_offset(text, found.start()),
            start: found.start(),
            end: found.end(),
            line_text: search_match_preview(text, found.start(), found.end()),
        });
        if matches.len() >= query.max_results {
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

const ARCHIVE_LIST_SCRIPT: &str = r#"
import datetime, json, re, sys, tarfile, zipfile

fmt, path = sys.argv[1], sys.argv[2]
drive_prefix = re.compile(r'^[A-Za-z]:')

def normalize_member(raw):
    raw = raw.rstrip('/')
    if not raw:
        return None
    if raw.startswith('/') or raw.startswith('\\') or '\\' in raw or drive_prefix.match(raw):
        return None
    parts = raw.split('/')
    if any(part in ('', '.', '..') or drive_prefix.match(part) for part in parts):
        return None
    return '/'.join(parts)

def zip_modified(info):
    try:
        return datetime.datetime(*info.date_time, tzinfo=datetime.timezone.utc).timestamp()
    except Exception:
        return None

def zip_mode(info):
    mode = (info.external_attr >> 16) & 0o777
    return mode or None

members = []
invalid = 0
if fmt == 'zip':
    with zipfile.ZipFile(path) as archive:
        for info in archive.infolist():
            name = normalize_member(info.filename)
            if name is None:
                invalid += 1
                continue
            kind = 'dir' if info.is_dir() else 'file'
            members.append({
                'name': name,
                'kind': kind,
                'len': None if kind == 'dir' else info.file_size,
                'modified': zip_modified(info),
                'mode': zip_mode(info),
            })
else:
    mode = {'tar': 'r:', 'tar.gz': 'r:gz', 'tar.xz': 'r:xz', 'tar.bz2': 'r:bz2'}[fmt]
    with tarfile.open(path, mode) as archive:
        for member in archive.getmembers():
            name = normalize_member(member.name)
            if name is None:
                invalid += 1
                continue
            if member.isdir():
                kind = 'dir'
            elif member.issym() or member.islnk():
                kind = 'symlink'
            elif member.isfile():
                kind = 'file'
            else:
                kind = 'other'
            members.append({
                'name': name,
                'kind': kind,
                'len': None if kind == 'dir' else member.size,
                'modified': member.mtime,
                'mode': member.mode & 0o777 if member.mode is not None else None,
            })
print(json.dumps({'members': members, 'invalid': invalid}))
"#;

const ARCHIVE_READ_SCRIPT: &str = r#"
import re, sys, tarfile, zipfile

fmt, path, member_name, max_bytes_raw = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
max_bytes = int(max_bytes_raw)
drive_prefix = re.compile(r'^[A-Za-z]:')

def safe_member(raw):
    if not raw or raw.startswith('/') or raw.startswith('\\') or '\\' in raw or drive_prefix.match(raw):
        return False
    return not any(part in ('', '.', '..') or drive_prefix.match(part) for part in raw.split('/'))

if not safe_member(member_name):
    raise SystemExit('unsafe archive member name')

if fmt == 'zip':
    with zipfile.ZipFile(path) as archive:
        info = archive.getinfo(member_name)
        if info.is_dir():
            raise SystemExit('archive member is not a file')
        if max_bytes >= 0 and info.file_size > max_bytes:
            raise SystemExit('archive member exceeds read limit')
        with archive.open(info) as member:
            data = member.read()
else:
    mode = {'tar': 'r:', 'tar.gz': 'r:gz', 'tar.xz': 'r:xz', 'tar.bz2': 'r:bz2'}[fmt]
    with tarfile.open(path, mode) as archive:
        member_info = archive.getmember(member_name)
        if not member_info.isfile():
            raise SystemExit('archive member is not a file')
        if max_bytes >= 0 and member_info.size > max_bytes:
            raise SystemExit('archive member exceeds read limit')
        member = archive.extractfile(member_info)
        if member is None:
            raise SystemExit('archive member is not a file')
        data = member.read()
if max_bytes >= 0 and len(data) > max_bytes:
    raise SystemExit('archive member exceeds read limit')
sys.stdout.buffer.write(data)
"#;
