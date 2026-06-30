use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::ffi::OsString;

#[derive(Clone, Debug)]
pub(crate) struct LocalShellAccess {
    workspace: WorkspaceRef,
}

impl LocalShellAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        Self { workspace }
    }
}

impl ShellAccess for LocalShellAccess {
    fn interactive_shell(
        &self,
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String> {
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
        let working_dir = working_dir
            .cloned()
            .unwrap_or_else(|| self.workspace.root.clone());
        log::debug!(
            "local shell command created workspace={} working_dir={}",
            self.workspace.display_name,
            working_dir.display()
        );
        Ok(ShellCommandSpec::new(shell, working_dir).arg("-i"))
    }

    fn command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        log::debug!(
            "local shell command created workspace={} working_dir={} program={}",
            self.workspace.display_name,
            working_dir.display(),
            program
        );
        let mut command = ShellCommandSpec::new(program, working_dir.clone());
        for arg in args {
            command = command.arg(arg.as_str());
        }
        Ok(command)
    }

    fn command_display(&self, command: &ShellCommandSpec) -> String {
        std::iter::once(command.program.clone())
            .chain(command.args.iter().cloned())
            .map(|part| part.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
