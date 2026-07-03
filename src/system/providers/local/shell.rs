use crate::system::capabilities::shell::{ShellAccess, ShellCommandSpec, default_shell};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub(crate) struct LocalShellAccess {
    workspace: WorkspaceRef,
    command_lookup: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl LocalShellAccess {
    pub(crate) fn new(workspace: WorkspaceRef) -> Self {
        Self {
            workspace,
            command_lookup: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ShellAccess for LocalShellAccess {
    fn interactive_shell(
        &self,
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String> {
        let shell = default_shell();
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
        if command_name_can_use_path(program) {
            if let Err(err) = self.which(program) {
                log::warn!(
                    "local shell command lookup failed workspace={} program={} error={}",
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
            log::debug!(
                "local shell command created workspace={} working_dir={} program={} via_default_shell=true",
                self.workspace.display_name,
                working_dir.display(),
                program
            );
            return Ok(ShellCommandSpec::new(default_shell(), working_dir.clone())
                .arg("-i")
                .arg("-c")
                .arg(script));
        }

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
                "local shell which cache hit workspace={} program={} resolved={:?}",
                self.workspace.display_name,
                program,
                cached
            );
            return Ok(cached);
        }

        let shell = default_shell();
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
        let resolved = if output.status.success() {
            let resolved = shell_which_output(&stdout);
            log::debug!(
                "local shell which workspace={} program={} resolved={:?}",
                self.workspace.display_name,
                program,
                resolved
            );
            resolved
        } else {
            log::debug!(
                "local shell which missing workspace={} program={} status={} stderr={}",
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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
