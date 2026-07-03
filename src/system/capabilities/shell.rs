use crate::system::path::WorkspacePath;
use std::ffi::{CStr, OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::ptr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellCommandSpec {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) working_dir: WorkspacePath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellCommandOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) status_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellRunRequest {
    pub(crate) operation: String,
    pub(crate) working_dir: WorkspacePath,
    pub(crate) script: String,
    pub(crate) stdin: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellCommandRunRequest {
    pub(crate) operation: String,
    pub(crate) working_dir: WorkspacePath,
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) stdin: Option<Vec<u8>>,
}

impl ShellRunRequest {
    pub(crate) fn new(
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

    pub(crate) fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

impl ShellCommandRunRequest {
    pub(crate) fn new(
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

    pub(crate) fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub(crate) fn stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

pub(crate) type ShellRunCallback =
    Box<dyn FnOnce(Result<ShellCommandOutput, String>) + Send + 'static>;

impl ShellCommandOutput {
    pub(crate) fn status_success(&self, success_codes: &[i32]) -> bool {
        self.status_code
            .is_some_and(|code| success_codes.contains(&code))
    }

    pub(crate) fn stdout_text_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    pub(crate) fn stdout_text_untrimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    pub(crate) fn stderr_text_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }

    pub(crate) fn failure_message(&self) -> String {
        let stderr = self.stderr_text_trimmed();
        if stderr.is_empty() {
            self.stdout_text_trimmed()
        } else {
            stderr
        }
    }
}

impl ShellCommandSpec {
    pub(crate) fn new(program: impl Into<OsString>, working_dir: impl Into<WorkspacePath>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: working_dir.into(),
        }
    }

    pub(crate) fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }
}

pub(crate) trait ShellAccess: Send + Sync {
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
    fn run_script(&self, request: ShellRunRequest, callback: ShellRunCallback);
    fn run_fast_script(&self, request: ShellRunRequest, callback: ShellRunCallback);
    fn run_fast_command(&self, request: ShellCommandRunRequest, callback: ShellRunCallback);
    fn command_display(&self, command: &ShellCommandSpec) -> String;
}

pub(crate) fn default_shell() -> OsString {
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
