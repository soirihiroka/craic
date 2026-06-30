use crate::system::path::WorkspacePath;
use std::ffi::OsString;

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
    fn command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String>;
    fn command_display(&self, command: &ShellCommandSpec) -> String;
}
