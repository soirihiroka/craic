use crate::system::capabilities::shell::{
    ShellAccess, ShellCommandOutput, ShellCommandRunRequest, ShellCommandSpec, ShellRunCallback,
    ShellRunRequest, default_shell,
};
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

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

    fn fast_command(
        &self,
        working_dir: &WorkspacePath,
        program: &str,
        args: &[String],
    ) -> Result<ShellCommandSpec, String> {
        log::debug!(
            "local fast command created workspace={} working_dir={} program={}",
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

    fn run_script(&self, request: ShellRunRequest, callback: ShellRunCallback) {
        let workspace = self.workspace.display_name.clone();
        thread::spawn(move || {
            callback(run_local_script(
                workspace,
                request,
                default_shell(),
                ShellScriptMode::UserInteractive,
            ));
        });
    }

    fn run_fast_script(&self, request: ShellRunRequest, callback: ShellRunCallback) {
        let workspace = self.workspace.display_name.clone();
        thread::spawn(move || {
            callback(run_local_script(
                workspace,
                request,
                OsString::from("/bin/sh"),
                ShellScriptMode::Fast,
            ));
        });
    }

    fn run_fast_command(&self, request: ShellCommandRunRequest, callback: ShellRunCallback) {
        let workspace = self.workspace.display_name.clone();
        thread::spawn(move || {
            callback(run_local_command(workspace, request));
        });
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

        let resolved = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let path = dir.join(program);
                let metadata = std::fs::metadata(&path).ok()?;
                (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                    .then(|| path.to_string_lossy().to_string())
            })
        });
        log::debug!(
            "local fast which workspace={} program={} resolved={:?}",
            self.workspace.display_name,
            program,
            resolved
        );

        self.command_lookup
            .lock()
            .map_err(|_| "Shell command lookup cache is unavailable.".to_string())?
            .insert(program.to_string(), resolved.clone());
        Ok(resolved)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellScriptMode {
    UserInteractive,
    Fast,
}

fn run_local_script(
    workspace_name: String,
    request: ShellRunRequest,
    shell: OsString,
    mode: ShellScriptMode,
) -> Result<ShellCommandOutput, String> {
    log::info!(
        "local shell command start workspace={} operation={} working_dir={} script_bytes={} mode={:?}",
        workspace_name,
        request.operation,
        request.working_dir.display(),
        request.script.len(),
        mode
    );

    let mut command = Command::new(&shell);
    if mode == ShellScriptMode::UserInteractive {
        command.arg("-i");
    }
    command
        .arg("-c")
        .arg(&request.script)
        .current_dir(&request.working_dir.absolute)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let mut child = command.spawn().map_err(|err| {
        format!(
            "Failed to start shell {} for {}: {err}",
            shell.to_string_lossy(),
            request.operation
        )
    })?;
    if let Some(stdin_bytes) = request.stdin.as_deref()
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin.write_all(stdin_bytes).map_err(|err| {
            format!(
                "Failed to write shell stdin for {}: {err}",
                request.operation
            )
        })?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for shell {}: {err}", request.operation))?;
    if output.status.success() {
        log::info!(
            "local shell command complete workspace={} operation={} stdout_bytes={}",
            workspace_name,
            request.operation,
            output.stdout.len()
        );
    } else {
        log::warn!(
            "local shell command failed workspace={} operation={} status={} stderr={}",
            workspace_name,
            request.operation,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(ShellCommandOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        status_code: output.status.code(),
    })
}

fn run_local_command(
    workspace_name: String,
    request: ShellCommandRunRequest,
) -> Result<ShellCommandOutput, String> {
    log::info!(
        "local fast command start workspace={} operation={} working_dir={} program={} args={}",
        workspace_name,
        request.operation,
        request.working_dir.display(),
        request.program,
        request.args.len()
    );
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(&request.working_dir.absolute)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(Stdio::piped());
    }

    let mut child = command.spawn().map_err(|err| {
        format!(
            "Failed to start fast command {} for {}: {err}",
            request.program, request.operation
        )
    })?;
    if let Some(stdin_bytes) = request.stdin.as_deref()
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin.write_all(stdin_bytes).map_err(|err| {
            format!(
                "Failed to write fast command stdin for {}: {err}",
                request.operation
            )
        })?;
    }

    let output = child.wait_with_output().map_err(|err| {
        format!(
            "Failed to wait for fast command {}: {err}",
            request.operation
        )
    })?;
    if output.status.success() {
        log::info!(
            "local fast command complete workspace={} operation={} stdout_bytes={}",
            workspace_name,
            request.operation,
            output.stdout.len()
        );
    } else {
        log::warn!(
            "local fast command failed workspace={} operation={} status={} stderr={}",
            workspace_name,
            request.operation,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(ShellCommandOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        status_code: output.status.code(),
    })
}

fn command_name_can_use_path(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && program
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
