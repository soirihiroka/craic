use craic_system::system::capabilities::shell::default_shell;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PATH_OUTPUT_PREFIX: &[u8] = b"\x1ecraic-path\x1f";
const PATH_OUTPUT_SUFFIX: u8 = b'\x1e';
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct LoginShellPath {
    pub(crate) shell: OsString,
    pub(crate) entry_count: usize,
    pub(crate) changed: bool,
}

pub(crate) fn import_login_shell_path() -> Result<LoginShellPath, String> {
    let shell = default_shell();
    let mut child = Command::new(&shell)
        .arg("-l")
        .arg("-i")
        .arg("-c")
        .arg("printf '\\036craic-path\\037%s\\036' \"$PATH\"")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            format!(
                "failed to start login shell {}: {error}",
                shell.to_string_lossy()
            )
        })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture login shell output".to_string())?;
    let (output_sender, output_receiver) = std::sync::mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout.read_to_end(&mut output).map(|_| output);
        let _ = output_sender.send(result);
    });

    let deadline = Instant::now() + LOGIN_SHELL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "login shell {} did not return PATH within {} seconds",
                    shell.to_string_lossy(),
                    LOGIN_SHELL_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("failed to wait for login shell: {error}")),
        }
    };
    if !status.success() {
        return Err(format!("login shell exited with status {status}"));
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    let output = output_receiver
        .recv_timeout(remaining)
        .map_err(|_| "timed out while reading login shell PATH".to_string())?
        .map_err(|error| format!("failed to read login shell PATH: {error}"))?;
    let prefix = output
        .windows(PATH_OUTPUT_PREFIX.len())
        .rposition(|candidate| candidate == PATH_OUTPUT_PREFIX)
        .map(|index| index + PATH_OUTPUT_PREFIX.len())
        .ok_or_else(|| "login shell did not emit PATH".to_string())?;
    let suffix = output[prefix..]
        .iter()
        .position(|byte| *byte == PATH_OUTPUT_SUFFIX)
        .map(|index| prefix + index)
        .ok_or_else(|| "login shell emitted an incomplete PATH".to_string())?;
    if suffix == prefix {
        return Err("login shell emitted an empty PATH".to_string());
    }

    let path = OsString::from_vec(output[prefix..suffix].to_vec());
    let entry_count = std::env::split_paths(&path).count();
    let changed = std::env::var_os("PATH").as_ref() != Some(&path);
    // SAFETY: macOS calls this at the start of `run`, before AppKit or the application runtime
    // creates threads. The temporary output reader never accesses the process environment and has
    // completed before this mutation.
    unsafe { std::env::set_var("PATH", &path) };

    Ok(LoginShellPath {
        shell,
        entry_count,
        changed,
    })
}
