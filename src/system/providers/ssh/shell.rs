use super::shell_quote;
use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::ffi::OsString;

#[derive(Clone, Debug)]
pub(crate) struct SshShellAccess {
    workspace: WorkspaceRef,
    host: String,
}

impl SshShellAccess {
    pub(crate) fn new(workspace: WorkspaceRef, host: String) -> Self {
        Self { workspace, host }
    }
}

impl ShellAccess for SshShellAccess {
    fn interactive_shell(
        &self,
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String> {
        let working_dir = working_dir.unwrap_or(&self.workspace.root);
        let remote = format!(
            "cd {} && exec \"${{SHELL:-/bin/sh}}\" -i",
            shell_quote(&working_dir.absolute)
        );
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg("-t")
            .arg(remote))
    }

    fn command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        let mut remote = format!(
            "cd {} && exec {}",
            shell_quote(&working_dir.absolute),
            shell_quote(program)
        );
        for arg in args {
            remote.push(' ');
            remote.push_str(&shell_quote(arg.as_str()));
        }
        log::debug!(
            "ssh shell command created workspace={} working_dir={} program={}",
            self.workspace.display_name,
            working_dir.display(),
            program
        );
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg("-t")
            .arg(remote))
    }

    fn command_display(&self, command: &ShellCommandSpec) -> String {
        std::iter::once(command.program.clone())
            .chain(command.args.iter().cloned())
            .map(|part: OsString| part.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }
}
