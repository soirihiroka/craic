use std::path::{Path, PathBuf};
use std::sync::Arc;

use craic_system::system::WorkspacePath;
use craic_system::system::capabilities::shell::{
    ShellAccess, ShellCommandRunRequest, ShellRunRequest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMediaKind {
    Image,
    Audio,
}

#[derive(Clone, Debug)]
pub struct RemoteMedia {
    pub path: String,
    cleanup_path: String,
}

pub fn supported_path(path: &Path, kind: RemoteMediaKind) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            match kind {
                RemoteMediaKind::Image => {
                    matches!(
                        extension.as_str(),
                        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
                    )
                }
                RemoteMediaKind::Audio => matches!(
                    extension.as_str(),
                    "mp3"
                        | "wav"
                        | "m4a"
                        | "aac"
                        | "flac"
                        | "ogg"
                        | "opus"
                        | "aiff"
                        | "aif"
                        | "caf"
                ),
            }
        })
}

pub fn materialize(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    source: PathBuf,
    kind: RemoteMediaKind,
) -> Result<RemoteMedia, String> {
    if !supported_path(&source, kind) {
        return Err(format!(
            "{} is not a supported {} file",
            source.display(),
            match kind {
                RemoteMediaKind::Image => "image",
                RemoteMediaKind::Audio => "audio",
            }
        ));
    }
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .expect("supported media paths have an extension")
        .to_ascii_lowercase();
    let bytes = std::fs::read(&source)
        .map_err(|error| format!("Failed to read {}: {error}", source.display()))?;
    if bytes.is_empty() {
        return Err(format!("{} is empty", source.display()));
    }
    let byte_count = bytes.len();

    let script = format!(
        "umask 077; craic_dir=$(mktemp -d \"${{TMPDIR:-/tmp}}/craic-codex-media-XXXXXX\") || exit 1; craic_path=\"$craic_dir/attachment.{extension}\"; if ! cat > \"$craic_path\"; then rm -rf -- \"$craic_dir\"; exit 1; fi; printf '%s' \"$craic_path\""
    );
    let output = shell
        .run_fast_script(
            ShellRunRequest::new("upload Codex media", working_dir, script).stdin(bytes),
        )
        .blocking_recv()
        .map_err(|_| "Remote media upload did not return a result".to_owned())??;
    if !output.status_success(&[0]) {
        let message = output.failure_message();
        return Err(if message.is_empty() {
            "Remote media upload failed".to_owned()
        } else {
            format!("Remote media upload failed: {message}")
        });
    }

    let path = output.stdout_text_trimmed();
    let (cleanup_path, file_name) = path
        .rsplit_once('/')
        .ok_or_else(|| "Remote media upload returned an invalid path".to_owned())?;
    let cleanup_name = cleanup_path.rsplit('/').next().unwrap_or_default();
    let cleanup_suffix = cleanup_name
        .strip_prefix("craic-codex-media-")
        .unwrap_or_default();
    if file_name != format!("attachment.{extension}")
        || !cleanup_path.starts_with('/')
        || cleanup_suffix.len() != 6
        || !cleanup_suffix
            .bytes()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err("Remote media upload returned an invalid path".to_owned());
    }
    let cleanup_path = cleanup_path.to_owned();
    log::info!(
        "Codex remote media materialized kind={kind:?} bytes={}",
        byte_count
    );
    Ok(RemoteMedia { path, cleanup_path })
}

pub fn remove(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    attachments: Vec<RemoteMedia>,
) {
    if attachments.is_empty() {
        return;
    }
    let cleanup = shell.run_fast_command(
        ShellCommandRunRequest::new("remove uploaded Codex media", working_dir, "rm").args(
            std::iter::once("-rf".to_owned())
                .chain(std::iter::once("--".to_owned()))
                .chain(
                    attachments
                        .into_iter()
                        .map(|attachment| attachment.cleanup_path),
                ),
        ),
    );
    // Cleanup may be requested from the App Server actor itself. Wait on a joined OS thread so
    // Tokio's blocking-receive guard cannot panic inside that async context, while still leaving
    // no detached cleanup worker behind during session shutdown.
    let result = match std::thread::spawn(move || cleanup.blocking_recv()).join() {
        Ok(result) => result,
        Err(_) => {
            log::warn!("remote media cleanup waiter stopped unexpectedly");
            return;
        }
    };
    match result {
        Ok(Ok(output)) if output.status_success(&[0]) => {}
        Ok(Ok(output)) => log::warn!(
            "failed removing uploaded Codex media: {}",
            output.failure_message()
        ),
        Ok(Err(error)) => log::warn!("failed removing uploaded Codex media: {error}"),
        Err(_) => log::warn!("remote media cleanup did not return a result"),
    }
}
