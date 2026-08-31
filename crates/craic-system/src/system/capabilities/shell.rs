use crate::system::path::WorkspacePath;
use std::ffi::{CStr, OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;
use tokio::sync::{mpsc, oneshot};

pub const SHELL_EVENT_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub working_dir: WorkspacePath,
    pub activity: ShellCommandActivity,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShellCommandActivity {
    #[default]
    Command,
    LogStream,
    LocalInteractiveShell,
    ReportedInteractiveShell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommandTextOutput {
    pub stdout: String,
    pub stderr: String,
    pub status_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellCommandStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub enum ShellCommandEvent {
    Record {
        stream: ShellCommandStream,
        text: String,
    },
    Finished(Result<ShellCommandTextOutput, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRunRequest {
    pub operation: String,
    pub working_dir: WorkspacePath,
    pub script: String,
    pub stdin: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommandRunRequest {
    pub operation: String,
    pub working_dir: WorkspacePath,
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

impl ShellRunRequest {
    pub fn new(
        operation: impl Into<String>,
        working_dir: impl Into<WorkspacePath>,
        script: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            working_dir: working_dir.into(),
            script: script.into(),
            stdin: None,
        }
    }

    pub fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

impl ShellCommandRunRequest {
    pub fn new(
        operation: impl Into<String>,
        working_dir: impl Into<WorkspacePath>,
        program: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            working_dir: working_dir.into(),
            program: program.into(),
            args: Vec::new(),
            stdin: None,
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

pub type ShellCommandResult = oneshot::Receiver<Result<ShellCommandOutput, String>>;
pub type ShellCommandGenerator = mpsc::Receiver<ShellCommandEvent>;

impl ShellCommandOutput {
    pub fn status_success(&self, success_codes: &[i32]) -> bool {
        self.status_code
            .is_some_and(|code| success_codes.contains(&code))
    }

    pub fn stdout_text_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    pub fn stdout_text_untrimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    pub fn stderr_text_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }

    pub fn failure_message(&self) -> String {
        let stderr = self.stderr_text_trimmed();
        if stderr.is_empty() {
            self.stdout_text_trimmed()
        } else {
            stderr
        }
    }
}

impl From<ShellCommandOutput> for ShellCommandTextOutput {
    fn from(output: ShellCommandOutput) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            status_code: output.status_code,
        }
    }
}

impl ShellCommandTextOutput {
    pub fn status_success(&self, success_codes: &[i32]) -> bool {
        self.status_code
            .is_some_and(|code| success_codes.contains(&code))
    }

    pub fn failure_message(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else {
            self.stderr.clone()
        }
    }
}

impl ShellCommandSpec {
    pub fn new(program: impl Into<OsString>, working_dir: impl Into<WorkspacePath>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: working_dir.into(),
            activity: ShellCommandActivity::Command,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn activity(mut self, activity: ShellCommandActivity) -> Self {
        self.activity = activity;
        self
    }
}

pub trait ShellAccess: Send + Sync {
    fn interactive_shell(
        &self,
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String>;
    fn which(&self, program: &str) -> Result<Option<String>, String>;
    fn command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String>;
    fn fast_command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String>;
    fn run_script(&self, request: ShellRunRequest) -> ShellCommandResult;
    fn run_fast_script(&self, request: ShellRunRequest) -> ShellCommandResult;
    fn run_fast_command(&self, request: ShellCommandRunRequest) -> ShellCommandResult;
    fn stream_fast_command(&self, request: ShellCommandRunRequest) -> ShellCommandGenerator;
    fn command_display(&self, command: &ShellCommandSpec) -> String;
}

pub(crate) fn run_streaming_command(
    command: &mut Command,
    stdin: Option<&[u8]>,
    operation: &str,
    event_sender: &mpsc::Sender<ShellCommandEvent>,
) -> Result<ShellCommandOutput, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to start command for {operation}: {err}"))?;
    if let Some(stdin_bytes) = stdin
        && let Some(mut child_stdin) = child.stdin.take()
        && let Err(err) = child_stdin.write_all(stdin_bytes)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "Failed to write command stdin for {operation}: {err}"
        ));
    }

    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Command for {operation} did not provide stdout."));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Command for {operation} did not provide stderr."));
    };

    let stdout_thread = read_command_stream(
        stdout,
        ShellCommandStream::Stdout,
        operation.to_string(),
        event_sender.clone(),
    );
    let stderr_thread = read_command_stream(
        stderr,
        ShellCommandStream::Stderr,
        operation.to_string(),
        event_sender.clone(),
    );
    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for command {operation}: {err}"))?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| format!("Stdout reader stopped unexpectedly for {operation}."))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| format!("Stderr reader stopped unexpectedly for {operation}."))??;

    Ok(ShellCommandOutput {
        stdout,
        stderr,
        status_code: status.code(),
    })
}

fn read_command_stream<R>(
    mut reader: R,
    stream: ShellCommandStream,
    operation: String,
    event_sender: mpsc::Sender<ShellCommandEvent>,
) -> thread::JoinHandle<Result<Vec<u8>, String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut record = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|err| format!("Failed to read {stream:?} for {operation}: {err}"))?;
            if count == 0 {
                send_command_record(&event_sender, stream, &record);
                return Ok(output);
            }

            output.extend_from_slice(&buffer[..count]);
            for byte in &buffer[..count] {
                if matches!(byte, b'\r' | b'\n') {
                    send_command_record(&event_sender, stream, &record);
                    record.clear();
                } else {
                    record.push(*byte);
                }
            }
        }
    })
}

fn send_command_record(
    event_sender: &mpsc::Sender<ShellCommandEvent>,
    stream: ShellCommandStream,
    bytes: &[u8],
) {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if !text.is_empty() {
        let _ = event_sender.blocking_send(ShellCommandEvent::Record { stream, text });
    }
}

pub fn default_shell() -> OsString {
    unsafe {
        let mut passwd: libc::passwd = std::mem::zeroed();
        let mut result = ptr::null_mut();
        let buffer_len = match libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) {
            len if len > 0 => len as usize,
            _ => 16_384,
        };
        let mut buffer = vec![0; buffer_len];
        if libc::getpwuid_r(
            libc::getuid(),
            &mut passwd,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) == 0
            && !result.is_null()
            && !passwd.pw_shell.is_null()
        {
            let shell = CStr::from_ptr(passwd.pw_shell).to_bytes();
            if !shell.is_empty() {
                return OsStr::from_bytes(shell).to_os_string();
            }
        }
    }

    std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"))
}
