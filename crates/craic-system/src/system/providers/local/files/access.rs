impl LocalFileAccess {
    pub fn new(
        system: SystemRef,
        workspace: WorkspaceRef,
        file_watch_service: Arc<LocalFileWatchService>,
    ) -> Self {
        let root_path = PathBuf::from(&workspace.root.absolute);
        Self {
            system,
            workspace,
            root_path,
            file_watch_service,
            sudo: false,
        }
    }

    fn authenticate_sudo(password: Option<&FileSudoPassword>) -> Result<(), FileSudoError> {
        let mut command = Command::new("sudo");
        command.env("LC_ALL", "C");
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        if password.is_some() {
            command.args(["-S", "-p", "", "-v"]).stdin(Stdio::piped());
        } else {
            command.args(["-n", "-v"]);
        }
        let mut child = command.spawn().map_err(|err| {
            FileSudoError::new(
                FileSudoErrorKind::Unavailable,
                format!("Unable to start sudo: {err}"),
            )
        })?;
        if let Some(password) = password
            && let Some(mut stdin) = child.stdin.take()
        {
            let mut bytes = Zeroizing::new(password.bytes().to_vec());
            bytes.push(b'\n');
            let result = stdin.write_all(&bytes);
            result.map_err(|err| {
                FileSudoError::new(
                    FileSudoErrorKind::AuthenticationFailed,
                    format!("Unable to send the sudo password: {err}"),
                )
            })?;
        }
        let output = child.wait_with_output().map_err(|err| {
            FileSudoError::new(
                FileSudoErrorKind::Unavailable,
                format!("Unable to wait for sudo authentication: {err}"),
            )
        })?;
        if output.status.success() {
            return Ok(());
        }
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(FileSudoError::new(
            if password.is_some() {
                FileSudoErrorKind::AuthenticationFailed
            } else {
                FileSudoErrorKind::PasswordRequired
            },
            if message.is_empty() {
                "Sudo authentication failed.".to_string()
            } else {
                message
            },
        ))
    }

    fn run_sudo_command(
        &self,
        operation: &str,
        program: &str,
        args: &[&std::ffi::OsStr],
        stdin: Option<&[u8]>,
    ) -> Result<Output, String> {
        let mut command = Command::new("sudo");
        command
            .args(["-n", "--", "env", "LC_ALL=C"])
            .arg(program)
            .args(args);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        log::info!("local sudo file operation start operation={operation}");
        let mut child = command
            .spawn()
            .map_err(|err| format!("Unable to start sudo for {operation}: {err}"))?;
        if let Some(bytes) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
        {
            child_stdin
                .write_all(bytes)
                .map_err(|err| format!("Unable to write sudo stdin for {operation}: {err}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| format!("Unable to wait for sudo {operation}: {err}"))?;
        if output.status.success() {
            log::info!("local sudo file operation complete operation={operation}");
            Ok(output)
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            log::warn!(
                "local sudo file operation failed operation={operation} status={} stderr={message}",
                output.status
            );
            Err(if message.is_empty() {
                format!("Sudo {operation} failed with status {}.", output.status)
            } else {
                message
            })
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

        if !local_path.starts_with(&self.root_path) {
            return Err("Path is outside the workspace.".to_string());
        }
        if self.sudo {
            self.run_sudo_command(
                "validate workspace path",
                "sh",
                &[
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new(
                        "root=$(realpath -e -- \"$1\") || exit 1; candidate=$2; while [ ! -e \"$candidate\" ] && [ ! -L \"$candidate\" ]; do parent=$(dirname -- \"$candidate\") || exit 1; [ \"$parent\" != \"$candidate\" ] || exit 1; candidate=$parent; done; resolved=$(realpath -e -- \"$candidate\") || exit 1; if [ \"$root\" != / ]; then case \"$resolved\" in \"$root\"|\"$root\"/*) ;; *) printf 'CRAIC-ERROR\\toutside-workspace\\n' >&2; exit 18 ;; esac; fi",
                    ),
                    std::ffi::OsStr::new("sh"),
                    self.root_path.as_os_str(),
                    local_path.as_os_str(),
                ],
                None,
            )
            .map_err(|err| {
                if err.contains("CRAIC-ERROR\toutside-workspace") {
                    "Elevated file operation would leave the workspace through a symlink."
                        .to_string()
                } else {
                    format!("Unable to validate elevated workspace path: {err}")
                }
            })?;
        }
        Ok(local_path)
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
        if self.sudo {
            return self.info_for_sudo_native_node(path);
        }
        let workspace_path = self.native_workspace_path(path)?;
        let local_path = self.local_path_for_workspace(&workspace_path)?;
        let metadata = fs::symlink_metadata(&local_path)
            .map_err(|err| format!("Unable to inspect {}: {err}", path.display()))?;
        let writable = local_path_writable(&local_path);
        let mut kind = file_kind(&metadata);
        let mut capabilities = match kind {
            FileKind::Directory => FileNodeCapabilities::native_directory(writable),
            FileKind::File => FileNodeCapabilities::native_file(writable),
            FileKind::Symlink | FileKind::Other => FileNodeCapabilities::native_other(writable),
            FileKind::Archive { .. } => unreachable!(),
        };
        if kind == FileKind::File
            && let Some(format) = path.file_name().and_then(ArchiveFormat::from_name)
        {
            let supported = archive_format_supported(format);
            kind = FileKind::Archive { format };
            capabilities = FileNodeCapabilities {
                listable: supported,
                ..FileNodeCapabilities::native_file(writable)
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

    fn info_for_sudo_native_node(&self, path: &FileNodePath) -> Result<FileNodeInfo, String> {
        let local_path = self.local_path_for_node(path)?;
        let output = self.run_sudo_command(
            "inspect file",
            "stat",
            &[
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new("%F\x1f%s\x1f%Y\x1f%a"),
                std::ffi::OsStr::new("--"),
                local_path.as_os_str(),
            ],
            None,
        )?;
        let raw = String::from_utf8(output.stdout)
            .map_err(|_| "Sudo file metadata was not valid UTF-8.".to_string())?;
        let mut fields = raw.trim_end().split('\x1f');
        let raw_kind = fields.next().unwrap_or_default();
        let len = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| "Sudo file metadata did not include a valid size.".to_string())?;
        let modified = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
        let mode = fields
            .next()
            .and_then(|value| u32::from_str_radix(value, 8).ok());
        let mut kind = match raw_kind {
            "directory" => FileKind::Directory,
            "regular file" | "regular empty file" => FileKind::File,
            "symbolic link" => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let mut capabilities = match kind {
            FileKind::Directory => FileNodeCapabilities::native_directory(true),
            FileKind::File => FileNodeCapabilities::native_file(true),
            FileKind::Symlink | FileKind::Other => FileNodeCapabilities::native_other(true),
            FileKind::Archive { .. } => unreachable!(),
        };
        if kind == FileKind::File
            && let Some(format) = path.file_name().and_then(ArchiveFormat::from_name)
        {
            kind = FileKind::Archive { format };
            capabilities = FileNodeCapabilities {
                listable: archive_format_supported(format),
                ..FileNodeCapabilities::native_file(true)
            };
        }
        Ok(FileNodeInfo {
            path: path.clone(),
            display_name: path
                .file_name()
                .map(ToString::to_string)
                .unwrap_or_else(|| self.workspace.display_name.clone()),
            kind,
            len: Some(len),
            modified,
            owner: None,
            group: None,
            mode,
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

    fn emit_progress<T>(callback: &FileOperationEmitter<T>, progress: FileOperationProgress) {
        callback(FileOperationEvent::Progress(progress));
    }

    fn perform_read_with_info(
        &self,
        request: &FileReadRequest,
        callback: &FileOperationEmitter<FileRead>,
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
        if self.sudo {
            let info = self
                .info_for_sudo_native_node(&request.path)
                .map_err(|err| {
                    Self::operation_error(
                        operation,
                        FileOperationErrorKind::Io,
                        Some(request.path.clone()),
                        None,
                        err,
                    )
                })?;
            if !matches!(info.kind, FileKind::File | FileKind::Archive { .. })
                || request
                    .max_bytes
                    .is_some_and(|max_bytes| info.len_or_zero() > max_bytes)
            {
                return Ok(FileRead { info, bytes: None });
            }
            let output = self
                .run_sudo_command(
                    "read file",
                    "cat",
                    &[std::ffi::OsStr::new("--"), local_path.as_os_str()],
                    None,
                )
                .map_err(|err| {
                    Self::operation_error(
                        operation,
                        FileOperationErrorKind::Io,
                        Some(request.path.clone()),
                        None,
                        err,
                    )
                })?;
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_bytes: output.stdout.len() as u64,
                    total_bytes: info.len_or_zero(),
                    completed_files: 1,
                    total_files: 1,
                    destination: None,
                },
            );
            return Ok(FileRead {
                info,
                bytes: Some(output.stdout),
            });
        }
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
        callback: &FileOperationEmitter<()>,
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
        if self.sudo {
            let result = match &request.payload {
                FileWritePayload::Directory => {
                    if request.mode != FileWriteMode::CreateNew {
                        return Err(Self::operation_error(
                            operation,
                            FileOperationErrorKind::Unsupported,
                            Some(request.path.clone()),
                            None,
                            "Directories can only be created with create-new mode.",
                        ));
                    }
                    self.run_sudo_command(
                        "create directory",
                        "mkdir",
                        &[std::ffi::OsStr::new("--"), local_path.as_os_str()],
                        None,
                    )
                }
                FileWritePayload::File(contents) => {
                    let script = match request.mode {
                        FileWriteMode::CreateNew => "set -C; cat > \"$1\"",
                        FileWriteMode::Replace => "cat > \"$1\"",
                        FileWriteMode::Append => "cat >> \"$1\"",
                    };
                    self.run_sudo_command(
                        "write file",
                        "sh",
                        &[
                            std::ffi::OsStr::new("-c"),
                            std::ffi::OsStr::new(script),
                            std::ffi::OsStr::new("craic-sudo-write"),
                            local_path.as_os_str(),
                        ],
                        Some(contents),
                    )
                }
            };
            result.map_err(|err| {
                Self::operation_error(
                    operation,
                    if err.contains("File exists") {
                        FileOperationErrorKind::AlreadyExists
                    } else {
                        FileOperationErrorKind::Io
                    },
                    Some(request.path.clone()),
                    None,
                    err,
                )
            })?;
            let completed_bytes = match &request.payload {
                FileWritePayload::File(contents) => contents.len() as u64,
                FileWritePayload::Directory => 0,
            };
            Self::emit_progress(
                callback,
                FileOperationProgress {
                    operation,
                    source: Some(request.path.clone()),
                    destination: Some(request.path.clone()),
                    current_path: Some(request.path.clone()),
                    completed_bytes,
                    total_bytes: completed_bytes,
                    completed_files: 1,
                    total_files: 1,
                },
            );
            return Ok(());
        }
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
        callback: &FileOperationEmitter<FileNodePath>,
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
        if self.sudo {
            let result = self.run_sudo_command(
                "copy path",
                "sh",
                &[
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new(
                        "if [ -e \"$2\" ] || [ -L \"$2\" ]; then printf 'CRAIC-ERROR\\talready-exists\\n' >&2; exit 17; fi; exec cp -a -- \"$1\" \"$2\"",
                    ),
                    std::ffi::OsStr::new("sh"),
                    source_path.as_os_str(),
                    destination_path.as_os_str(),
                ],
                None,
            );
            result.map_err(|err| {
                Self::operation_error(
                    operation,
                    if err.contains("CRAIC-ERROR\talready-exists") {
                        FileOperationErrorKind::AlreadyExists
                    } else {
                        FileOperationErrorKind::Io
                    },
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
            return Ok(request.destination.clone());
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
        callback: &FileOperationEmitter<FileNodePath>,
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
        callback: &FileOperationEmitter<FileNodePath>,
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
        if self.sudo {
            let result = self.run_sudo_command(
                "move path",
                "sh",
                &[
                    std::ffi::OsStr::new("-c"),
                    std::ffi::OsStr::new(
                        "if [ -e \"$2\" ] || [ -L \"$2\" ]; then printf 'CRAIC-ERROR\\talready-exists\\n' >&2; exit 17; fi; exec mv -- \"$1\" \"$2\"",
                    ),
                    std::ffi::OsStr::new("sh"),
                    source_path.as_os_str(),
                    destination_path.as_os_str(),
                ],
                None,
            );
            result.map_err(|err| {
                Self::operation_error(
                    operation,
                    if err.contains("CRAIC-ERROR\talready-exists") {
                        FileOperationErrorKind::AlreadyExists
                    } else {
                        FileOperationErrorKind::Io
                    },
                    Some(request.source.clone()),
                    Some(destination.clone()),
                    err,
                )
            })?;
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
        callback: Option<&FileOperationEmitter<()>>,
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
        if request.path.is_root() {
            return Err(Self::operation_error(
                operation,
                FileOperationErrorKind::Unsupported,
                Some(request.path.clone()),
                None,
                "The workspace root cannot be deleted.",
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
        if self.sudo {
            self.run_sudo_command(
                "delete path",
                "rm",
                &[
                    std::ffi::OsStr::new("-rf"),
                    std::ffi::OsStr::new("--"),
                    local_path.as_os_str(),
                ],
                None,
            )
            .map_err(|err| {
                Self::operation_error(
                    operation,
                    FileOperationErrorKind::Io,
                    Some(request.path.clone()),
                    None,
                    err,
                )
            })?;
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
            return Ok(());
        }
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
