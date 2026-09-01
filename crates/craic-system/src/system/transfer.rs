use crate::system::capabilities::files::{
    FileAccess, FileCopyRequest, FileDeleteRequest, FileDownloadDestination, FileDownloadRequest,
    FileNodeKind, FileOperation, FileReadRequest, FileWriteMode, FileWritePayload,
    FileWriteRequest, wait_file_operation,
};
use crate::system::path::FileNodePath;
use crate::system::provider::SystemProvider;
use crate::system::providers::local::LocalProvider;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

const TRANSFER_CHUNK_BYTES: usize = 256 * 1024;

pub fn transfer_local_paths(
    destination_access: Arc<dyn FileAccess>,
    sources: Vec<PathBuf>,
    destination_parent: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
) -> Result<Vec<FileNodePath>, String> {
    if sources.is_empty() {
        return Err("Choose at least one file or folder to upload.".to_string());
    }

    let mut transferred = Vec::with_capacity(sources.len());
    for source in sources {
        check_canceled(cancel_requested.as_ref())?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("Unable to determine a name for {}.", source.display()))?;
        let parent = source
            .parent()
            .ok_or_else(|| format!("Unable to determine the parent of {}.", source.display()))?;
        let provider = LocalProvider::new();
        let source_workspace = LocalProvider::workspace_for_path(parent);
        let source_access = provider
            .files(&source_workspace)
            .ok_or_else(|| "Local file access is unavailable.".to_string())?;
        let source_node =
            FileNodePath::root(&provider.system_ref(), &source_workspace).join_child(name);
        let destination = destination_parent.join_child(name);
        let result = transfer_file_node(
            source_access,
            destination_access.clone(),
            source_node,
            destination.clone(),
            cancel_requested.clone(),
        );
        match result {
            Ok(path) => transferred.push(path),
            Err(error) => {
                for path in transferred.iter().rev() {
                    if let Err(cleanup_error) = wait_file_operation(
                        destination_access.delete_events(FileDeleteRequest {
                            path: path.clone(),
                            cancel_requested: None,
                        }),
                        FileOperation::Delete,
                    ) {
                        log::warn!(
                            "local transfer cleanup failed path={} error={cleanup_error}",
                            path.display()
                        );
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(transferred)
}

pub fn transfer_file_node(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    source: FileNodePath,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
) -> Result<FileNodePath, String> {
    check_canceled(cancel_requested.as_ref())?;
    if target_path_is_equal_or_descendant(
        source_access.as_ref(),
        &source,
        destination_access.as_ref(),
        &destination,
    ) {
        return Err("Cannot copy or move an item into itself.".to_string());
    }

    if Arc::ptr_eq(&source_access, &destination_access) {
        return wait_file_operation(
            destination_access.copy_node_events(FileCopyRequest {
                source,
                destination,
                cancel_requested: Some(cancel_requested),
            }),
            FileOperation::Copy,
        )
        .map_err(|error| error.to_string());
    }

    let destination_parent = destination
        .parent()
        .ok_or_else(|| "Cannot transfer an item to the workspace root.".to_string())?;
    let staged_destination =
        destination_parent.join_child(format!(".craic-transfer-{}", Uuid::new_v4().simple()));
    if let Err(error) = copy_between_file_accesses(
        source_access,
        destination_access.clone(),
        source,
        staged_destination.clone(),
        cancel_requested.clone(),
    ) {
        cleanup_staged_node(destination_access.as_ref(), &staged_destination);
        return Err(error);
    }
    if let Err(error) = destination_access.finalize_staged_node(
        &staged_destination,
        &destination,
        cancel_requested.as_ref(),
    ) {
        cleanup_staged_node(destination_access.as_ref(), &staged_destination);
        return Err(error);
    }
    Ok(destination)
}

fn cleanup_staged_node(destination_access: &dyn FileAccess, staged_destination: &FileNodePath) {
    if let Err(error) = wait_file_operation(
        destination_access.delete_events(FileDeleteRequest {
            path: staged_destination.clone(),
            cancel_requested: None,
        }),
        FileOperation::Delete,
    ) {
        log::warn!(
            "staged transfer cleanup failed path={} error={error}",
            staged_destination.display()
        );
    }
}

fn copy_between_file_accesses(
    source_access: Arc<dyn FileAccess>,
    destination_access: Arc<dyn FileAccess>,
    source: FileNodePath,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
) -> Result<(), String> {
    check_canceled(cancel_requested.as_ref())?;
    let info = source_access.info(&source)?;
    match info.kind {
        FileNodeKind::Directory => {
            wait_file_operation(
                destination_access.write_node_events(FileWriteRequest {
                    path: destination.clone(),
                    mode: FileWriteMode::CreateNew,
                    payload: FileWritePayload::Directory,
                    cancel_requested: Some(cancel_requested.clone()),
                }),
                FileOperation::Write,
            )
            .map_err(|error| error.to_string())?;
            let listing = source_access
                .list_dirs(std::slice::from_ref(&source))?
                .into_iter()
                .next()
                .ok_or_else(|| format!("Unable to list {}.", source.display()))?;
            for child in listing.entries {
                check_canceled(cancel_requested.as_ref())?;
                let name = child
                    .file_name()
                    .ok_or_else(|| format!("Cannot transfer {}.", child.display()))?
                    .to_string();
                copy_between_file_accesses(
                    source_access.clone(),
                    destination_access.clone(),
                    child,
                    destination.join_child(name),
                    cancel_requested.clone(),
                )?;
            }
            Ok(())
        }
        FileNodeKind::File | FileNodeKind::Archive { .. } => {
            if let Some(local_source) =
                local_transfer_source(source_access.clone(), &source, cancel_requested.clone())?
            {
                return copy_local_file_to_access(
                    local_source.path(),
                    destination_access.clone(),
                    destination.clone(),
                    cancel_requested,
                );
            }
            let bytes = wait_file_operation(
                source_access.read_with_info_events(FileReadRequest {
                    path: source,
                    max_bytes: Some(TRANSFER_CHUNK_BYTES as u64),
                    cancel_requested: Some(cancel_requested.clone()),
                }),
                FileOperation::Read,
            )
            .map_err(|error| error.to_string())?
            .into_bytes()?;
            wait_file_operation(
                destination_access.write_node_events(FileWriteRequest {
                    path: destination.clone(),
                    mode: FileWriteMode::CreateNew,
                    payload: FileWritePayload::File(bytes),
                    cancel_requested: Some(cancel_requested),
                }),
                FileOperation::Write,
            )
            .map_err(|error| error.to_string())
        }
        FileNodeKind::Symlink | FileNodeKind::Other => Err(format!(
            "Copying {} between different providers is unsupported.",
            source.display()
        )),
    }
}

fn target_path_is_equal_or_descendant(
    source_access: &dyn FileAccess,
    source: &FileNodePath,
    destination_access: &dyn FileAccess,
    destination: &FileNodePath,
) -> bool {
    let Some((_, source_system)) = source.root_ref() else {
        return false;
    };
    let Some((_, destination_system)) = destination.root_ref() else {
        return false;
    };
    if source_system != destination_system {
        return false;
    }
    let source_workspace = source_access.workspace();
    let destination_workspace = destination_access.workspace();
    let Some(source_path) = source.to_workspace_path(&source_workspace) else {
        return false;
    };
    let Some(destination_path) = destination.to_workspace_path(&destination_workspace) else {
        return false;
    };
    let source = source_path.absolute.trim_end_matches('/');
    let destination = destination_path.absolute.trim_end_matches('/');
    let source = if source.is_empty() { "/" } else { source };
    let destination = if destination.is_empty() {
        "/"
    } else {
        destination
    };
    destination == source
        || (source == "/" && destination.starts_with('/'))
        || destination
            .strip_prefix(source)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

enum LocalTransferSource {
    Borrowed(PathBuf),
    Temporary { path: PathBuf, directory: PathBuf },
}

impl LocalTransferSource {
    fn path(&self) -> &Path {
        match self {
            Self::Borrowed(path) | Self::Temporary { path, .. } => path,
        }
    }
}

impl Drop for LocalTransferSource {
    fn drop(&mut self) {
        let Self::Temporary { directory, .. } = self else {
            return;
        };
        if let Err(error) = std::fs::remove_dir_all(&*directory) {
            log::warn!(
                "cross-provider transfer temporary directory cleanup failed path={} error={error}",
                directory.display()
            );
        }
    }
}

fn local_transfer_source(
    source_access: Arc<dyn FileAccess>,
    source: &FileNodePath,
    cancel_requested: Arc<AtomicBool>,
) -> Result<Option<LocalTransferSource>, String> {
    if let Some(path) = source_access.local_path(source) {
        return Ok(Some(LocalTransferSource::Borrowed(path)));
    }
    if !source_access.supports_download() {
        return Ok(None);
    }
    check_canceled(cancel_requested.as_ref())?;
    let directory = std::env::temp_dir().join(format!("craic-transfer-{}", Uuid::new_v4()));
    std::fs::create_dir(&directory).map_err(|error| {
        format!(
            "Unable to create transfer staging directory {}: {error}",
            directory.display()
        )
    })?;
    let downloaded = source_access.download_to_local(FileDownloadRequest {
        sources: vec![source.clone()],
        destination: FileDownloadDestination::Folder(directory.clone()),
        cancel_requested: Some(cancel_requested),
    });
    let path = match downloaded {
        Ok(paths) => paths.into_iter().next().ok_or_else(|| {
            "The source provider did not return a staged transfer file.".to_string()
        }),
        Err(error) => Err(error),
    };
    match path {
        Ok(path) if path.starts_with(&directory) => {
            Ok(Some(LocalTransferSource::Temporary { path, directory }))
        }
        Ok(path) => {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&directory) {
                log::warn!(
                    "cross-provider transfer invalid staging cleanup failed path={} error={cleanup_error}",
                    directory.display()
                );
            }
            Err(format!(
                "The source provider returned a transfer path outside its staging directory: {}",
                path.display()
            ))
        }
        Err(error) => {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&directory) {
                log::warn!(
                    "cross-provider transfer failed staging cleanup failed path={} error={cleanup_error}",
                    directory.display()
                );
            }
            Err(error)
        }
    }
}

fn copy_local_file_to_access(
    source: &Path,
    destination_access: Arc<dyn FileAccess>,
    destination: FileNodePath,
    cancel_requested: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut file = File::open(source)
        .map_err(|error| format!("Unable to open {}: {error}", source.display()))?;
    let mut buffer = vec![0; TRANSFER_CHUNK_BYTES];
    let mut mode = FileWriteMode::CreateNew;
    loop {
        check_canceled(cancel_requested.as_ref())?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Unable to read {}: {error}", source.display()))?;
        if read == 0 && mode == FileWriteMode::Append {
            break;
        }
        wait_file_operation(
            destination_access.write_node_events(FileWriteRequest {
                path: destination.clone(),
                mode,
                payload: FileWritePayload::File(buffer[..read].to_vec()),
                cancel_requested: Some(cancel_requested.clone()),
            }),
            FileOperation::Write,
        )
        .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        mode = FileWriteMode::Append;
    }
    Ok(())
}

fn check_canceled(cancel_requested: &AtomicBool) -> Result<(), String> {
    if cancel_requested.load(Ordering::Relaxed) {
        Err("Operation canceled.".to_string())
    } else {
        Ok(())
    }
}
