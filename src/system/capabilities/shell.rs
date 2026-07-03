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
