use super::shell_quote;
use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct SshShellAccess {
    workspace: WorkspaceRef,
    host: String,
    command_lookup: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl SshShellAccess {
    pub(crate) fn new(workspace: WorkspaceRef, host: String) -> Self {
        Self {
            workspace,
            host,
            command_lookup: Arc::new(Mutex::new(HashMap::new())),
        }
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
        if command_name_can_use_path(program) {
            if let Err(err) = self.which(program) {
                log::warn!(
                    "ssh shell command lookup failed host={} workspace={} program={} error={}",
                    self.host,
                    self.workspace.display_name,
                    program,
                    err
                );
            }
            let mut script = format!("exec {}", shell_quote(program));
            for arg in args {
                script.push(' ');
                script.push_str(&shell_quote(arg));
            }
            let remote = format!(
                "cd {} && exec \"${{SHELL:-/bin/sh}}\" -i -c {}",
                shell_quote(&working_dir.absolute),
                shell_quote(&script)
            );
            log::debug!(
                "ssh shell command created workspace={} working_dir={} program={} via_default_shell=true",
                self.workspace.display_name,
                working_dir.display(),
                program
            );
            return Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
                .arg(self.host.clone())
                .arg("-t")
                .arg(remote));
        }

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

    fn which(&self, program: &str) -> Result<Option<String>, String> {
        if !command_name_can_use_path(program) {
            return Ok(None);
        }

        if let Some(cached) = self
            .command_lookup
            .lock()
            .map_err(|_| "Shell command lookup cache is unavailable.".to_string())?
            .get(program)
            .cloned()
        {
            log::debug!(
                "ssh shell which cache hit host={} workspace={} program={} resolved={:?}",
                self.host,
                self.workspace.display_name,
                program,
                cached
            );
            return Ok(cached);
        }

        let lookup = format!("command -v {}", shell_quote(program));
        let remote = format!(
            "exec \"${{SHELL:-/bin/sh}}\" -i -c {}",
            shell_quote(&lookup)
        );
        let output = Command::new("ssh")
            .arg(&self.host)
            .arg(remote)
            .output()
            .map_err(|err| format!("Failed to start ssh command lookup: {err}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let resolved = if output.status.success() {
            let resolved = shell_which_output(&stdout);
            log::debug!(
                "ssh shell which host={} workspace={} program={} resolved={:?}",
                self.host,
                self.workspace.display_name,
                program,
                resolved
            );
            resolved
        } else {
            log::debug!(
                "ssh shell which missing host={} workspace={} program={} status={} stderr={}",
                self.host,
                self.workspace.display_name,
                program,
                output.status,
                stderr.trim()
            );
            None
        };

        self.command_lookup
            .lock()
            .map_err(|_| "Shell command lookup cache is unavailable.".to_string())?
            .insert(program.to_string(), resolved.clone());
        Ok(resolved)
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
