use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use crate::system::WorkspacePath;
use crate::system::capabilities::shell::{ShellAccess, ShellCommandRunRequest, ShellRunRequest};

#[derive(Clone, Debug)]
pub(super) struct RemoteImage {
    pub path: String,
    cleanup_path: String,
}

pub(super) fn supported_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
}

pub(super) fn upload_images(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    sources: Vec<PathBuf>,
    callback: impl FnOnce(Result<Vec<RemoteImage>, String>) + Send + 'static,
) {
    std::thread::spawn(move || {
        log::info!("Codex remote image upload started count={}", sources.len());
        let mut uploaded = Vec::with_capacity(sources.len());
        let result = (|| {
            for source in sources {
                let extension = source
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
                    .filter(|extension| {
                        matches!(
                            extension.as_str(),
                            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
                        )
                    })
                    .ok_or_else(|| format!("{} is not a supported image file", source.display()))?;
                let bytes = std::fs::read(&source)
                    .map_err(|error| format!("Failed to read {}: {error}", source.display()))?;
                if bytes.is_empty() {
                    return Err(format!("{} is empty", source.display()));
                }

                let script = format!(
                    "umask 077; craic_dir=$(mktemp -d \"${{TMPDIR:-/tmp}}/craic-codex-image-XXXXXX\") || exit 1; craic_path=\"$craic_dir/image.{extension}\"; if ! cat > \"$craic_path\"; then rm -rf -- \"$craic_dir\"; exit 1; fi; printf '%s' \"$craic_path\""
                );
                let (sender, receiver) = mpsc::sync_channel(1);
                shell.run_fast_script(
                    ShellRunRequest::new("upload Codex image", working_dir.clone(), script)
                        .stdin(bytes),
                    Box::new(move |result| {
                        let _ = sender.send(result);
                    }),
                );
                let output = receiver
                    .recv()
                    .map_err(|_| "Remote image upload did not return a result".to_owned())??;
                if !output.status_success(&[0]) {
                    let message = output.failure_message();
                    return Err(if message.is_empty() {
                        "Remote image upload failed".to_owned()
                    } else {
                        format!("Remote image upload failed: {message}")
                    });
                }
                let path = output.stdout_text_trimmed();
                let (cleanup_path, file_name) = path
                    .rsplit_once('/')
                    .ok_or_else(|| "Remote image upload returned an invalid path".to_owned())?;
                let cleanup_name = cleanup_path.rsplit('/').next().unwrap_or_default();
                let cleanup_suffix = cleanup_name
                    .strip_prefix("craic-codex-image-")
                    .unwrap_or_default();
                if file_name != format!("image.{extension}")
                    || !cleanup_path.starts_with('/')
                    || cleanup_suffix.len() != 6
                    || !cleanup_suffix
                        .bytes()
                        .all(|character| character.is_ascii_alphanumeric())
                {
                    return Err("Remote image upload returned an invalid path".to_owned());
                }
                let cleanup_path = cleanup_path.to_owned();
                uploaded.push(RemoteImage { path, cleanup_path });
            }
            Ok(())
        })();

        if let Err(error) = result {
            remove_images(shell, working_dir, uploaded);
            log::warn!("Codex remote image upload failed: {error}");
            callback(Err(error));
            return;
        }
        log::info!(
            "Codex remote image upload finished count={}",
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
    if images.is_empty() {
        return;
    }
    shell.run_fast_command(
        ShellCommandRunRequest::new("remove uploaded Codex images", working_dir, "rm").args(
            std::iter::once("-rf".to_owned())
                .chain(std::iter::once("--".to_owned()))
                .chain(images.into_iter().map(|image| image.cleanup_path)),
        ),
        Box::new(|result| match result {
            Ok(output) if output.status_success(&[0]) => {}
            Ok(output) => log::warn!(
                "failed removing uploaded Codex images: {}",
                output.failure_message()
            ),
            Err(error) => log::warn!("failed removing uploaded Codex images: {error}"),
        }),
    );
}
