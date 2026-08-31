use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::system::WorkspacePath;
use crate::system::capabilities::shell::ShellAccess;
use craic_agent::remote_media::{RemoteMediaKind, materialize, remove, supported_path};

pub(super) use craic_agent::remote_media::RemoteMedia as RemoteImage;

pub(super) fn supported_image_path(path: &Path) -> bool {
    supported_path(path, RemoteMediaKind::Image)
}

pub(super) fn upload_images(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    sources: Vec<PathBuf>,
    callback: impl FnOnce(Result<Vec<RemoteImage>, String>) + Send + 'static,
) {
    upload_media(
        shell,
        working_dir,
        sources,
        RemoteMediaKind::Image,
        callback,
    );
}

pub(super) fn upload_media(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    sources: Vec<PathBuf>,
    kind: RemoteMediaKind,
    callback: impl FnOnce(Result<Vec<RemoteImage>, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        log::info!(
            "Codex remote media upload started kind={kind:?} count={}",
            sources.len()
        );
        let mut uploaded = Vec::with_capacity(sources.len());
        for source in sources {
            match materialize(shell.clone(), working_dir.clone(), source, kind) {
                Ok(media) => uploaded.push(media),
                Err(error) => {
                    remove(shell, working_dir, uploaded);
                    log::warn!("Codex remote media upload failed kind={kind:?}: {error}");
                    callback(Err(error));
                    return;
                }
            }
        }
        log::info!(
            "Codex remote media upload finished kind={kind:?} count={}",
            uploaded.len()
        );
        callback(Ok(uploaded));
    });
}

pub(super) fn remove_images(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    images: Vec<RemoteImage>,
) {
    remove(shell, working_dir, images);
}
