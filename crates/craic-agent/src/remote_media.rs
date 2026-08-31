use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use craic_system::system::WorkspacePath;
use craic_system::system::capabilities::shell::{
    ShellAccess, ShellCommandOutput, ShellCommandRunRequest, ShellRunRequest,
};

const CANCELLABLE_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const BOUNDED_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_CANCELLABLE_MEDIA_BYTES: usize = 64 * 1024 * 1024;

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

    let script = upload_script(&extension);
    let output = shell
        .run_fast_script(
            ShellRunRequest::new("upload Codex media", working_dir, script).stdin(bytes),
        )
        .blocking_recv()
        .map_err(|_| "Remote media upload did not return a result".to_owned())??;
    finish_materialize(output, &extension, byte_count, kind)
}

pub fn materialize_cancellable(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    source: PathBuf,
    kind: RemoteMediaKind,
    cancel_requested: impl Fn() -> bool,
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
    let deadline = Instant::now() + CANCELLABLE_UPLOAD_TIMEOUT;
    let bytes = read_source_cancellable(&source, &cancel_requested, deadline)?;
    if bytes.is_empty() {
        return Err(format!("{} is empty", source.display()));
    }
    let byte_count = bytes.len();
    let output = run_bounded_command(
        shell,
        working_dir,
        "sh",
        &["-c".to_owned(), upload_script(&extension)],
        Some(bytes),
        &cancel_requested,
        deadline,
        "Remote media upload",
    )?;
    finish_materialize(output, &extension, byte_count, kind)
}

fn upload_script(extension: &str) -> String {
    format!(
        "umask 077; craic_dir=; cleanup() {{ if [ -n \"$craic_dir\" ]; then rm -rf -- \"$craic_dir\"; fi; }}; trap cleanup EXIT HUP INT TERM; craic_dir=$(mktemp -d \"${{TMPDIR:-/tmp}}/craic-codex-media-XXXXXX\") || exit 1; craic_path=\"$craic_dir/attachment.{extension}\"; cat > \"$craic_path\" || exit 1; printf '%s' \"$craic_path\"; trap - EXIT HUP INT TERM"
    )
}

fn finish_materialize(
    output: ShellCommandOutput,
    extension: &str,
    byte_count: usize,
    kind: RemoteMediaKind,
) -> Result<RemoteMedia, String> {
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

pub fn remove_bounded(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    attachments: Vec<RemoteMedia>,
) {
    if attachments.is_empty() {
        return;
    }
    let args = std::iter::once("-rf".to_owned())
        .chain(std::iter::once("--".to_owned()))
        .chain(
            attachments
                .into_iter()
                .map(|attachment| attachment.cleanup_path),
        )
        .collect::<Vec<_>>();
    match run_bounded_command(
        shell,
        working_dir,
        "rm",
        &args,
        None,
        &|| false,
        Instant::now() + BOUNDED_CLEANUP_TIMEOUT,
        "Remote media cleanup",
    ) {
        Ok(output) if output.status_success(&[0]) => {}
        Ok(output) => log::warn!(
            "failed removing uploaded Codex media: {}",
            output.failure_message()
        ),
        Err(error) => log::warn!("failed removing uploaded Codex media: {error}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_bounded_command(
    shell: Arc<dyn ShellAccess>,
    working_dir: WorkspacePath,
    program: &str,
    args: &[String],
    stdin: Option<Vec<u8>>,
    cancel_requested: &impl Fn() -> bool,
    deadline: Instant,
    operation: &str,
) -> Result<ShellCommandOutput, String> {
    let command_spec = shell.fast_command(&working_dir, program, args)?;
    let mut command = Command::new(&command_spec.program);
    command
        .args(&command_spec.args)
        .current_dir(&command_spec.working_dir.absolute)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }
    #[cfg(unix)]
    command.process_group(0);

    run_bounded_process(
        command,
        stdin,
        cancel_requested,
        deadline,
        operation,
        None,
        true,
    )
}

fn read_source_cancellable(
    source: &Path,
    cancel_requested: &impl Fn() -> bool,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/bin/cat");
    command
        .arg("--")
        .arg(source)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let output = run_bounded_process(
        command,
        None,
        cancel_requested,
        deadline,
        "Remote media source read",
        Some(MAX_CANCELLABLE_MEDIA_BYTES),
        false,
    )?;
    if output.status_success(&[0]) {
        Ok(output.stdout)
    } else {
        let message = output.failure_message();
        Err(if message.is_empty() {
            format!("Failed to read {}", source.display())
        } else {
            format!("Failed to read {}: {message}", source.display())
        })
    }
}

fn run_bounded_process(
    mut command: Command,
    stdin: Option<Vec<u8>>,
    cancel_requested: &impl Fn() -> bool,
    deadline: Instant,
    operation: &str,
    stdout_limit: Option<usize>,
    recover_completed_stdout: bool,
) -> Result<ShellCommandOutput, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("{operation} could not start: {error}"))?;
    let stdin_writer = stdin.and_then(|bytes| {
        child.stdin.take().map(|mut pipe| {
            thread::spawn(move || {
                pipe.write_all(&bytes)
                    .map_err(|error| format!("Failed writing remote media: {error}"))
            })
        })
    });
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| read_output(pipe, "stdout", stdout_limit));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| read_output(pipe, "stderr", None));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = terminate_and_collect(
                    &mut child,
                    stdin_writer,
                    stdout_reader,
                    stderr_reader,
                    operation,
                );
                return Err(format!("{operation} could not be awaited: {error}"));
            }
        }
        if cancel_requested() {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            log::info!("{operation} cancellation requested; terminating process group");
            let output = terminate_and_collect(
                &mut child,
                stdin_writer,
                stdout_reader,
                stderr_reader,
                operation,
            );
            if let Some(mut output) = output
                && (output.status_success(&[0])
                    || (recover_completed_stdout && !output.stdout.is_empty()))
            {
                output.status_code = Some(0);
                return Ok(output);
            }
            return Err(format!("{operation} was cancelled."));
        }
        if Instant::now() >= deadline {
            log::warn!("{operation} exceeded its deadline; terminating process group");
            let output = terminate_and_collect(
                &mut child,
                stdin_writer,
                stdout_reader,
                stderr_reader,
                operation,
            );
            if let Some(mut output) = output
                && (output.status_success(&[0])
                    || (recover_completed_stdout && !output.stdout.is_empty()))
            {
                output.status_code = Some(0);
                return Ok(output);
            }
            return Err(format!("{operation} timed out."));
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    };
    collect_command_output(
        status,
        stdin_writer,
        stdout_reader,
        stderr_reader,
        operation,
    )
}

fn collect_command_output(
    status: std::process::ExitStatus,
    stdin_writer: Option<thread::JoinHandle<Result<(), String>>>,
    stdout_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    stderr_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    operation: &str,
) -> Result<ShellCommandOutput, String> {
    let stdin_result = if let Some(writer) = stdin_writer {
        writer
            .join()
            .map_err(|_| format!("{operation} input writer stopped unexpectedly."))?
    } else {
        Ok(())
    };
    let stdout_result = join_output(stdout_reader, operation, "stdout");
    let stderr_result = join_output(stderr_reader, operation, "stderr");
    stdin_result?;
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    Ok(ShellCommandOutput {
        stdout,
        stderr,
        status_code: status.code(),
    })
}

fn terminate_and_collect(
    child: &mut Child,
    stdin_writer: Option<thread::JoinHandle<Result<(), String>>>,
    stdout_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    stderr_reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    operation: &str,
) -> Option<ShellCommandOutput> {
    let status = terminate_process_group(child)?;
    collect_command_output(
        status,
        stdin_writer,
        stdout_reader,
        stderr_reader,
        operation,
    )
    .ok()
}

fn read_output<R>(
    mut pipe: R,
    stream: &'static str,
    limit: Option<usize>,
) -> thread::JoinHandle<Result<Vec<u8>, String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = pipe
                .read(&mut buffer)
                .map_err(|error| format!("Failed reading remote media {stream}: {error}"))?;
            if count == 0 {
                return Ok(bytes);
            }
            if limit.is_some_and(|limit| bytes.len().saturating_add(count) > limit) {
                return Err(format!(
                    "Remote media exceeds the {} MiB upload limit.",
                    MAX_CANCELLABLE_MEDIA_BYTES / (1024 * 1024)
                ));
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
    })
}

fn join_output(
    reader: Option<thread::JoinHandle<Result<Vec<u8>, String>>>,
    operation: &str,
    stream: &str,
) -> Result<Vec<u8>, String> {
    reader
        .map(|reader| {
            reader
                .join()
                .map_err(|_| format!("{operation} {stream} reader stopped unexpectedly."))?
        })
        .transpose()
        .map(|bytes| bytes.unwrap_or_default())
}

fn terminate_process_group(child: &mut Child) -> Option<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(child.id()) {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    child.wait().ok()
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
