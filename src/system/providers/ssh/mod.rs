mod docker;
mod files;
mod git;
mod github;
mod open;
mod shell;
mod terminal_link;

use self::docker::SshDockerAccess;
use self::files::SshFileAccess;
use self::git::SshGitAccess;
use self::github::SshGitHubAccess;
use self::open::SshOpenAccess;
use self::shell::SshShellAccess;
use self::terminal_link::SshTerminalLinkAccess;
use crate::system::capabilities::github::GitHubAccess;
use crate::system::capabilities::{
    docker::DockerAccess, files::FileAccess, git::GitAccess, open::OpenAccess, shell::ShellAccess,
    terminal_link::TerminalLinkAccess,
};
use crate::system::path::{
    HostRef, ProviderKind, SystemId, SystemRef, WorkspaceId, WorkspacePath, WorkspaceRef,
};
use crate::system::provider::{
    ProviderWorkspaceEntry, ProviderWorkspaceGitStatus, ProviderWorkspaceListRequest,
    ProviderWorkspaceRemote, ProviderWorkspaceSource, SystemProvider,
};
use crate::{bitbucket, github as github_api, gitlab};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct SshProviderConfig {
    pub(crate) host: String,
    pub(crate) label: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SshProvider {
    system: SystemRef,
    config: SshProviderConfig,
}

#[derive(Clone, Debug)]
pub(crate) struct SshCommandRunner {
    host: String,
    label: String,
}

#[derive(Debug)]
pub(crate) struct SshOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl SshProviderConfig {
    pub(crate) fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            label: None,
        }
    }

    pub(crate) fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl SshProvider {
    pub(crate) fn new(config: SshProviderConfig) -> Self {
        let label = config.label.clone().unwrap_or_else(|| config.host.clone());
        let id = SystemId::new(format!("ssh:{}", config.host));
        Self {
            system: SystemRef::new(id, ProviderKind::Ssh, Some(HostRef::new(label))),
            config,
        }
    }

    pub(crate) fn system_ref(&self) -> SystemRef {
        self.system.clone()
    }

    pub(crate) fn workspace_for_remote_path(
        &self,
        absolute_path: impl Into<String>,
    ) -> WorkspaceRef {
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

        let script = "emit_workspace() { \
             kind=$1; source=$2; d=$3; remote=''; url=''; \
             if git -C \"$d\" rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
               remote=$(git -C \"$d\" rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null | sed 's#/.*##' || true); \
               if [ -z \"$remote\" ] && git -C \"$d\" remote get-url origin >/dev/null 2>&1; then remote=origin; fi; \
               if [ -z \"$remote\" ]; then remote=$(git -C \"$d\" remote 2>/dev/null | head -n 1); fi; \
               if [ -n \"$remote\" ]; then url=$(git -C \"$d\" remote get-url \"$remote\" 2>/dev/null || true); fi; \
               if [ -z \"$url\" ]; then url='-'; fi; \
             fi; \
             printf '%s\\t%s\\t%s\\t%s\\t%s\\n' \"$kind\" \"$source\" \"$d\" \"$remote\" \"$url\"; \
           }; \
           resolve_path() { \
             p=$1; \
             if [ \"$p\" = '~' ]; then printf '%s\\n' \"$HOME\"; \
             else case \"$p\" in \"~/\"*) printf '%s/%s\\n' \"$HOME\" \"${p#\\~/}\" ;; *) printf '%s\\n' \"$p\" ;; esac; fi; \
           }; \
           while IFS='	' read -r kind raw_path; do \
             [ -n \"$kind\" ] || continue; \
             path=$(resolve_path \"$raw_path\"); \
             case \"$kind\" in \
               W) [ -d \"$path\" ] && emit_workspace W \"$raw_path\" \"$path\" ;; \
               R) [ -d \"$path\" ] && find \"$path\" -mindepth 1 -maxdepth 1 -type d -print | while IFS= read -r d; do emit_workspace R \"$raw_path\" \"$d\"; done ;; \
             esac; \
           done";
        let output = runner.run_script_with_stdin(
            "bulk list ssh workspaces",
            script,
            Some(input.as_bytes()),
        )?;
        let output = String::from_utf8(output.stdout)
            .map_err(|_| "ssh bulk list ssh workspaces returned non-UTF-8".to_string())?;
        let mut workspaces = output
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(5, '\t');
                let source_kind = fields.next().unwrap_or_default();
                let source_path = fields.next().unwrap_or_default().to_string();
                let path = fields.next().unwrap_or_default().trim();
                let remote_name = fields.next().unwrap_or_default().trim();
                let remote_url = fields.next().unwrap_or_default().trim();
                let path = path.trim();
                if path.is_empty() {
                    return None;
                }
                let source = match source_kind {
                    "W" => ProviderWorkspaceSource::Workspace { path: source_path },
                    "R" => ProviderWorkspaceSource::Root { path: source_path },
                    _ => return None,
                };
                let git = if remote_url.is_empty() {
                    ProviderWorkspaceGitStatus::NotRepo
                } else {
                    let remote = (remote_url != "-").then(|| ProviderWorkspaceRemote {
                        name: (!remote_name.is_empty()).then(|| remote_name.to_string()),
                        url: remote_url.to_string(),
                        host: remote_host(remote_url),
                        slug: remote_slug(remote_url),
                    });
                    ProviderWorkspaceGitStatus::Repo { remote }
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
                    git,
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

    fn git(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitAccess>> {
        log::debug!(
            "creating ssh git capability provider={} workspace={} root={}",
            self.label(),
            workspace.display_name,
            workspace.root.absolute
        );
        let files: Arc<dyn FileAccess> = Arc::new(SshFileAccess::new(
            self.system.clone(),
            workspace.clone(),
            self.runner(),
        ));
        Some(Arc::new(SshGitAccess::new(
            workspace.clone(),
            self.runner(),
            files,
        )))
    }

    fn github(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn GitHubAccess>> {
        log::debug!(
            "creating ssh github capability provider={} workspace={} root={}",
            self.label(),
            workspace.display_name,
            workspace.root.absolute
        );
        Some(Arc::new(SshGitHubAccess::new(workspace.clone())))
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

    fn opener(&self, workspace: &WorkspaceRef) -> Option<Arc<dyn OpenAccess>> {
        Some(Arc::new(SshOpenAccess::new(
            workspace.clone(),
            self.label(),
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
    pub(crate) fn new(host: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            label: host.clone(),
            host,
        }
    }

    pub(crate) fn run_script(&self, operation: &str, script: &str) -> Result<SshOutput, String> {
        self.run_script_with_stdin(operation, script, None)
    }

    pub(crate) fn run_script_with_stdin(
        &self,
        operation: &str,
        script: &str,
        stdin: Option<&[u8]>,
    ) -> Result<SshOutput, String> {
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
            child_stdin
                .write_all(stdin_bytes)
                .map_err(|err| format!("Failed to write ssh stdin for {operation}: {err}"))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|err| format!("Failed to wait for ssh {operation}: {err}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        if output.status.success() {
            log::info!(
                "ssh command complete provider={} operation={} stdout_bytes={}",
                self.label,
                operation,
                output.stdout.len()
            );
            Ok(SshOutput {
                stdout: output.stdout,
                stderr,
            })
        } else {
            log::warn!(
                "ssh command failed provider={} operation={} status={} stderr={}",
                self.label,
                operation,
                output.status,
                stderr
            );
            Err(if stderr.is_empty() {
                format!("ssh {operation} failed with status {}", output.status)
            } else {
                stderr
            })
        }
    }

    pub(crate) fn run_text(&self, operation: &str, script: &str) -> Result<String, String> {
        let output = self.run_script(operation, script)?;
        String::from_utf8(output.stdout).map_err(|_| format!("ssh {operation} returned non-UTF-8"))
    }

    pub(crate) fn resolve_path(&self, path: &str) -> Result<String, String> {
        let script = format!(
            "p={}; if [ \"$p\" = '~' ]; then printf '%s\\n' \"$HOME\"; else case \"$p\" in \"~/\"*) printf '%s/%s\\n' \"$HOME\" \"${{p#\\~/}}\" ;; *) printf '%s\\n' \"$p\" ;; esac; fi",
            shell_quote(path)
        );
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

pub(crate) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(crate) fn remote_workspace_path(workspace: &WorkspaceRef, path: &WorkspacePath) -> String {
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

pub(crate) fn workspace_path_for_remote(workspace: &WorkspaceRef, absolute: &str) -> WorkspacePath {
    let root = workspace.root.absolute.trim_end_matches('/');
    let relative = absolute
        .strip_prefix(root)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .unwrap_or("");
    WorkspacePath::from_workspace_relative(&workspace.root, relative)
}

fn remote_slug(remote_url: &str) -> Option<String> {
    github_api::parse_github_url(remote_url)
        .or_else(|| gitlab::parse_gitlab_url(remote_url))
        .or_else(|| bitbucket::parse_bitbucket_url(remote_url))
}

fn remote_host(remote_url: &str) -> Option<String> {
    let remote_url = remote_url.trim();
    if let Some(rest) = remote_url.strip_prefix("git@") {
        return rest.split_once(':').map(|(host, _)| host.to_string());
    }
    if let Some((_, rest)) = remote_url.split_once("://") {
        return rest
            .split('/')
            .next()
            .filter(|host| !host.is_empty())
            .map(|host| host.trim_start_matches("git@").to_string());
    }
    None
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}
