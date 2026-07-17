use super::{SshCommandRunner, shell_quote};
use crate::system::capabilities::shell::{
    ShellAccess, ShellCommandRunRequest, ShellCommandSpec, ShellRunCallback, ShellRunRequest,
};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct SshShellAccess {
    workspace: WorkspaceRef,
    host: String,
    command_lookup: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl SshShellAccess {
    pub fn new(workspace: WorkspaceRef, host: String) -> Self {
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

    fn fast_command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        let remote = fast_remote_command(working_dir, program, args);
        log::debug!(
            "ssh fast command created workspace={} working_dir={} program={}",
            self.workspace.display_name,
            working_dir.display(),
            program
        );
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg(remote))
    }

    fn run_script(&self, request: ShellRunRequest, callback: ShellRunCallback) {
        let host = self.host.clone();
        thread::spawn(move || {
            let remote = format!(
                "cd {} && {}",
                shell_quote(&request.working_dir.absolute),
                request.script
            );
            callback(SshCommandRunner::new(host).run_script_output_with_stdin(
                &request.operation,
                &remote,
                request.stdin.as_deref(),
            ));
        });
    }

    fn run_fast_script(&self, request: ShellRunRequest, callback: ShellRunCallback) {
        let host = self.host.clone();
        thread::spawn(move || {
            let remote = format!(
                "cd {} && {}",
                shell_quote(&request.working_dir.absolute),
                request.script
            );
            callback(SshCommandRunner::new(host).run_script_output_with_stdin(
                &request.operation,
                &remote,
                request.stdin.as_deref(),
            ));
        });
    }

    fn run_fast_command(&self, request: ShellCommandRunRequest, callback: ShellRunCallback) {
        let host = self.host.clone();
        thread::spawn(move || {
            let remote = fast_remote_command(&request.working_dir, &request.program, &request.args);
            callback(SshCommandRunner::new(host).run_script_output_with_stdin(
                &request.operation,
                &remote,
                request.stdin.as_deref(),
            ));
        });
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
        let script = format!(
            "exec \"${{SHELL:-/bin/sh}}\" -i -c {}",
            shell_quote(&lookup)
        );
        let start = Instant::now();
        log::debug!(
            "ssh shell which start host={} workspace={} program={}",
            self.host,
            self.workspace.display_name,
            program
        );
        let output = SshCommandRunner::new(self.host.clone()).run_script_output_with_stdin(
            "fast ssh command lookup",
            &script,
            None,
        )?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let resolved = if output.status_success(&[0]) {
            let resolved = shell_which_output(&stdout);
            log::debug!(
                "ssh shell which complete host={} workspace={} program={} status={:?} elapsed_ms={} resolved={:?} stderr={}",
                self.host,
                self.workspace.display_name,
                program,
                output.status_code,
                start.elapsed().as_millis(),
                resolved,
                output.stderr_text_trimmed()
            );
            resolved
        } else {
            log::debug!(
                "ssh shell which complete host={} workspace={} program={} status={:?} elapsed_ms={} resolved=None stderr={}",
                self.host,
                self.workspace.display_name,
                program,
                output.status_code,
                start.elapsed().as_millis(),
                output.stderr_text_trimmed()
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

fn fast_remote_command(working_dir: &WorkspacePath, program: &str, args: &[String]) -> String {
    let mut script = format!(
        "cd {} && exec {}",
        shell_quote(&working_dir.absolute),
        shell_quote(program)
    );
    for arg in args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script
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
