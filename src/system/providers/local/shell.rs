use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::ffi::OsString;
use std::process::Command;

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
        let program = self.resolved_or_original(program);
        log::debug!(
            "local shell command created workspace={} working_dir={} program={}",
            self.workspace.display_name,
            working_dir.display(),
            program
        );
        let mut command = ShellCommandSpec::new(program.as_str(), working_dir.clone());
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

    fn which(&self, program: &str) -> Result<Option<String>, String> {
        if !command_name_can_use_path(program) {
            return Ok(None);
        }

        let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));
        let script = format!("command -v {}", shell_quote(program));
        let output = Command::new(&shell)
            .arg("-i")
            .arg("-c")
            .arg(&script)
            .current_dir(&self.workspace.root.absolute)
            .output()
            .map_err(|err| {
                format!(
                    "Failed to start default shell {} for command lookup: {err}",
                    shell.to_string_lossy()
                )
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            let resolved = shell_which_output(&stdout);
            log::debug!(
                "local shell which workspace={} program={} resolved={:?}",
                self.workspace.display_name,
                program,
                resolved
            );
            Ok(resolved)
        } else {
            log::debug!(
                "local shell which missing workspace={} program={} status={} stderr={}",
                self.workspace.display_name,
                program,
                output.status,
                stderr.trim()
            );
            Ok(None)
        }
    }
}

impl LocalShellAccess {
    fn resolved_or_original(&self, program: &str) -> String {
        match self.which(program) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => program.to_string(),
            Err(err) => {
                log::warn!(
                    "local shell command lookup failed workspace={} program={} error={}",
                    self.workspace.display_name,
                    program,
                    err
                );
                program.to_string()
            }
        }
    }
}

fn command_name_can_use_path(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && program
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn shell_which_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
