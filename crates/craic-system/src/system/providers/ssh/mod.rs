mod docker;
mod files;
mod shell;
mod terminal_link;

use self::docker::SshDockerAccess;
use self::files::SshFileAccess;
use self::shell::SshShellAccess;
use self::terminal_link::SshTerminalLinkAccess;
use super::url::GioUrlOpenAccess;
use crate::system::capabilities::{
    docker::DockerAccess,
    files::FileAccess,
    open::DesktopOpenAccess,
    shell::{ShellAccess, ShellCommandOutput},
    terminal_link::TerminalLinkAccess,
    url::UrlOpenAccess,
};
use crate::system::path::{
    HostRef, ProviderKind, SystemId, SystemRef, WorkspaceId, WorkspacePath, WorkspaceRef,
};
use crate::system::provider::{
    ProviderWorkspaceEntry, ProviderWorkspaceListRequest, ProviderWorkspaceSource, SystemProvider,
};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

const SSH_WORKSPACE_LIST_SCRIPT: &str = include_str!("scripts/list_workspaces.sh");
const SSH_RESOLVE_PATH_SCRIPT: &str = include_str!("scripts/resolve_path.sh");
const SSH_STDIN_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct SshProviderConfig {
    pub host: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SshProvider {
    system: SystemRef,
    config: SshProviderConfig,
}

#[derive(Clone, Debug)]
pub struct SshCommandRunner {
    host: String,
    label: String,
}

#[derive(Debug)]
pub struct SshOutput {
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl SshProviderConfig {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            label: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl SshProvider {
    pub fn new(config: SshProviderConfig) -> Self {
        let label = config.label.clone().unwrap_or_else(|| config.host.clone());
        let id = SystemId::new(format!("ssh:{}", config.host));
        Self {
            system: SystemRef::new(id, ProviderKind::Ssh, Some(HostRef::new(label))),
            config,
        }
    }

    pub fn system_ref(&self) -> SystemRef {
        self.system.clone()
    }

    pub fn workspace_for_remote_path(&self, absolute_path: impl Into<String>) -> WorkspaceRef {
        let configured_path = absolute_path.into();
        let absolute_path = self
            .runner()
            .resolve_path(&configured_path)
            .unwrap_or_else(|err| {
                log::warn!(
                    "failed to resolve ssh workspace path provider={} path={} err={}",
                    self.label(),
                    configured_path,
                    err
                );
                configured_path
            });
        let display_name = absolute_path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&absolute_path)
            .to_string();
        WorkspaceRef::new(
            WorkspaceId::for_target(&self.system.id, &absolute_path),
            WorkspacePath::from_absolute(absolute_path),
            display_name,
        )
    }

    fn runner(&self) -> SshCommandRunner {
        SshCommandRunner {
            host: self.config.host.clone(),
            label: self.label(),
        }
    }
}

impl SystemProvider for SshProvider {
    fn id(&self) -> SystemId {
        self.system.id.clone()
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Ssh
    }

    fn label(&self) -> String {
        self.config
            .label
            .clone()
            .unwrap_or_else(|| self.config.host.clone())
    }

    fn list_workspaces(
        &self,
        request: ProviderWorkspaceListRequest,
    ) -> Result<Vec<ProviderWorkspaceEntry>, String> {
        let runner = self.runner();
        let mut input = String::new();
        for path in &request.workspace_paths {
            input.push_str("W\t");
            input.push_str(path);
            input.push('\n');
        }
        for path in &request.root_paths {
            input.push_str("R\t");
            input.push_str(path);
            input.push('\n');
        }

        let output = runner.run_script_with_stdin(
            "bulk list ssh workspaces",
            SSH_WORKSPACE_LIST_SCRIPT,
            Some(input.as_bytes()),
        )?;
        let output = String::from_utf8(output.stdout)
            .map_err(|_| "ssh bulk list ssh workspaces returned non-UTF-8".to_string())?;
        let mut workspaces = output
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let source_kind = fields.next().unwrap_or_default();
                let source_path = fields.next().unwrap_or_default().to_string();
                let path = fields.next().unwrap_or_default().trim();
                let path = path.trim();
                if path.is_empty() {
                    return None;
                }
                let source = match source_kind {
                    "W" => ProviderWorkspaceSource::Workspace { path: source_path },
                    "R" => ProviderWorkspaceSource::Root { path: source_path },
                    _ => return None,
                };
                Some(ProviderWorkspaceEntry {
                    display_name: path
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(path)
                        .to_string(),
                    path: path.to_string(),
                    source,
                })
            })
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
        });
        log::debug!(
            "ssh workspace bulk list provider={} workspaces={} roots={} count={}",
            self.label(),
            request.workspace_paths.len(),
            request.root_paths.len(),
            workspaces.len()
        );
        Ok(workspaces)
    }

    fn files(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn FileAccess>> {
        log::debug!(
            "creating ssh files capability provider={} workspace={} root={}",
            self.label(),
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(SshFileAccess::new(
            self.system.clone(),
            workspace.clone(),
            self.runner(),
        )))
    }

    fn shell(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn ShellAccess>> {
        Some(Arc::new(SshShellAccess::new(
            workspace.clone(),
            self.config.host.clone(),
        )))
    }

    fn docker(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn DockerAccess>> {
        Some(Arc::new(SshDockerAccess::new(
            workspace.clone(),
            self.config.host.clone(),
        )))
    }

    fn desktop_opener(&self, _workspace: &WorkspaceRef) -> Option<Arc<dyn DesktopOpenAccess>> {
        None
    }

    fn url_opener(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn UrlOpenAccess>> {
        log::debug!(
            "creating ssh url-open capability provider={} workspace={} root={}",
            self.label(),
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(GioUrlOpenAccess::new(
            self.label(),
            workspace.clone(),
            Some(self.config.host.clone()),
        )))
    }

    fn terminal_links(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn TerminalLinkAccess>> {
        log::debug!(
            "creating ssh terminal-link capability provider={} workspace={} root={}",
            self.label(),
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(SshTerminalLinkAccess::new(
            workspace.clone(),
            self.runner(),
        )))
    }
}

impl SshCommandRunner {
    pub fn new(host: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            label: host.clone(),
            host,
        }
    }

    pub fn run_script(&self, operation: &str, script: &str) -> Result<SshOutput, String> {
        self.run_script_with_stdin(operation, script, None)
    }

    pub fn run_script_output_with_stdin(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
    ) -> Result<ShellCommandOutput, String> {
        self.run_script_output_with_stdin_progress(operation, script, stdin, |_, _| Ok(()))
    }

    pub fn run_script_output_with_stdin_progress<F>(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
        mut progress: F,
    ) -> Result<ShellCommandOutput, String>
    where
        F: FnMut(u64, u64) -> Result<(), String>,
    {
        let remote_command = format!("sh -lc {}", shell_quote(script));
        log::info!(
            "ssh command start provider={} operation={} script_bytes={}",
            self.label,
            operation,
            script.len()
        );

        let mut command = Command::new("ssh");
        command
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=5m")
            .arg("-o")
            .arg(format!(
                "ControlPath=/tmp/craic-ssh-{}-%r@%h:%p",
                sanitize_id(&self.host)
            ))
            .arg("--")
            .arg(&self.host)
            .arg(remote_command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }

        let mut child = command
            .spawn()
            .map_err(|err| format!("Failed to start ssh for {operation}: {err}"))?;
        if let Some(stdin_bytes) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
        {
            let total_bytes = stdin_bytes.len() as u64;
            if let Err(err) = progress(0, total_bytes) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
            let mut completed_bytes = 0u64;
            for chunk in stdin_bytes.chunks(SSH_STDIN_CHUNK_BYTES) {
                child_stdin
                    .write_all(chunk)
                    .map_err(|err| format!("Failed to write ssh stdin for {operation}: {err}"))?;
                completed_bytes = completed_bytes.saturating_add(chunk.len() as u64);
                if let Err(err) = progress(completed_bytes, total_bytes) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|err| format!("Failed to wait for ssh {operation}: {err}"))?;
        if output.status.success() {
            log::info!(
                "ssh command complete provider={} operation={} stdout_bytes={}",
                self.label,
                operation,
                output.stdout.len()
            );
        } else {
            log::warn!(
                "ssh command failed provider={} operation={} status={} stderr={}",
                self.label,
                operation,
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

    pub fn run_script_with_stdin(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
    ) -> Result<SshOutput, String> {
        let output = self.run_script_output_with_stdin(operation, script, stdin)?;
        ssh_output_or_error(operation, output)
    }

    pub fn run_script_with_stdin_progress<F>(
        &self,
        operation: &str,
        script: &str,
        stdin: &[u8],
        progress: F,
    ) -> Result<SshOutput, String>
    where
        F: FnMut(u64, u64) -> Result<(), String>,
    {
        let output =
            self.run_script_output_with_stdin_progress(operation, script, Some(stdin), progress)?;
        ssh_output_or_error(operation, output)
    }

    pub fn run_script_with_stdout_progress<F>(
        &self,
        operation: &str,
        script: &str,
        mut progress: F,
    ) -> Result<SshOutput, String>
    where
        F: FnMut(&[u8]) -> Result<(), String>,
    {
        let remote_command = format!("sh -lc {}", shell_quote(script));
        log::info!(
            "ssh command start provider={} operation={} script_bytes={}",
            self.label,
            operation,
            script.len()
        );

        let mut child = Command::new("ssh")
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=5m")
            .arg("-o")
            .arg(format!(
                "ControlPath=/tmp/craic-ssh-{}-%r@%h:%p",
                sanitize_id(&self.host)
            ))
            .arg("--")
            .arg(&self.host)
            .arg(remote_command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("Failed to start ssh for {operation}: {err}"))?;

        let mut stdout = Vec::new();
        if let Some(mut child_stdout) = child.stdout.take() {
            let mut buffer = vec![0u8; SSH_STDIN_CHUNK_BYTES];
            loop {
                let read = child_stdout
                    .read(&mut buffer)
                    .map_err(|err| format!("Failed to read ssh stdout for {operation}: {err}"))?;
                if read == 0 {
                    break;
                }
                stdout.extend_from_slice(&buffer[..read]);
                if let Err(err) = progress(&stdout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|err| format!("Failed to wait for ssh {operation}: {err}"))?;
        if output.status.success() {
            log::info!(
                "ssh command complete provider={} operation={} stdout_bytes={}",
                self.label,
                operation,
                stdout.len()
            );
        } else {
            log::warn!(
                "ssh command failed provider={} operation={} status={} stderr={}",
                self.label,
                operation,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        ssh_output_or_error(
            operation,
            ShellCommandOutput {
                stdout,
                stderr: output.stderr,
                status_code: output.status.code(),
            },
        )
    }

    pub fn run_text(&self, operation: &str, script: &str) -> Result<String, String> {
        let output = self.run_script(operation, script)?;
        String::from_utf8(output.stdout).map_err(|_| format!("ssh {operation} returned non-UTF-8"))
    }

    pub fn resolve_path(&self, path: &str) -> Result<String, String> {
        let script = format!("set -- {}\n{SSH_RESOLVE_PATH_SCRIPT}", shell_quote(path));
        let output = self.run_text("resolve remote path", &script)?;
        let resolved = output.lines().next().unwrap_or(path).trim().to_string();
        log::debug!(
            "ssh path resolved provider={} input={} resolved={}",
            self.label,
            path,
            resolved
        );
        Ok(resolved)
    }
}

fn ssh_output_or_error(operation: &str, output: ShellCommandOutput) -> Result<SshOutput, String> {
    let stderr = output.stderr_text_trimmed();

    if output.status_success(&[0]) {
        Ok(SshOutput {
            stdout: output.stdout,
            stderr,
        })
    } else {
        Err(if stderr.is_empty() {
            format!(
                "ssh {operation} failed with status {:?}",
                output.status_code
            )
        } else {
            stderr
        })
    }
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn remote_workspace_path(workspace: &WorkspaceRef, path: &WorkspacePath) -> String {
    match path.relative.as_deref() {
        Some(relative) if !relative.is_empty() => {
            format!(
                "{}/{}",
                workspace.root.absolute.trim_end_matches('/'),
                relative.trim_start_matches('/')
            )
        }
        _ => path.absolute.clone(),
    }
}

pub fn workspace_path_for_remote(workspace: &WorkspaceRef, absolute: &str) -> WorkspacePath {
    let root = workspace.root.absolute.trim_end_matches('/');
    let relative = absolute
        .strip_prefix(root)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .unwrap_or("");
    WorkspacePath::from_workspace_relative(&workspace.root, relative)
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}
