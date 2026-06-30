use super::SshCommandRunner;
use super::shell_quote;
use crate::system::capabilities::docker::{ComposeFileAction, DockerAccess};
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::system::path::{WorkspacePath, WorkspaceRef};
use std::path::Path;

#[derive(Clone, Debug)]
pub(crate) struct SshDockerAccess {
    workspace: WorkspaceRef,
    host: String,
    runner: SshCommandRunner,
}

impl SshDockerAccess {
    pub(crate) fn new(workspace: WorkspaceRef, host: String) -> Self {
        Self {
            workspace,
            runner: SshCommandRunner::new(host.clone()),
            host,
        }
    }
}

impl DockerAccess for SshDockerAccess {
    fn run_docker(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<Vec<u8>, String> {
        let working_dir = working_dir.unwrap_or(&self.workspace.root);
        let mut remote = format!("cd {} && docker", shell_quote(&working_dir.absolute));
        for arg in args {
            remote.push(' ');
            remote.push_str(&shell_quote(arg));
        }
        log::debug!(
            "ssh docker command start host={} workspace={} working_dir={} args={:?}",
            self.host,
            self.workspace.display_name,
            working_dir.display(),
            args
        );
        let output = self.runner.run_text("docker", &remote)?;
        log::debug!(
            "ssh docker command complete host={} stdout_bytes={}",
            self.host,
            output.len()
        );
        Ok(output.into_bytes())
    }

    fn docker_command(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String> {
        let working_dir = working_dir.unwrap_or(&self.workspace.root);
        let mut remote = format!("cd {} && docker", shell_quote(&working_dir.absolute));
        for arg in args {
            remote.push(' ');
            remote.push_str(&shell_quote(arg));
        }
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg("-t")
            .arg(remote))
    }

    fn build_image_command(&self, dockerfile: &WorkspacePath) -> Result<ShellCommandSpec, String> {
        let relative = dockerfile.relative_or_empty();
        let context = dockerfile_context_path(relative);
        let tag = docker_image_tag(&self.workspace.display_name, &context);
        let remote = format!(
            "cd {} && docker build -f {} -t {} {}",
            shell_quote(&self.workspace.root.absolute),
            shell_quote(relative),
            shell_quote(&tag),
            shell_quote(&context)
        );
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg("-t")
            .arg(remote))
    }

    fn compose_file_command(
        &self,
        compose_file: &WorkspacePath,
        action: ComposeFileAction,
    ) -> Result<ShellCommandSpec, String> {
        let compose_file = compose_file.relative_or_empty();
        let remote = match action {
            ComposeFileAction::Up => format!(
                "cd {} && docker compose -f {} up -d --build",
                shell_quote(&self.workspace.root.absolute),
                shell_quote(compose_file)
            ),
            ComposeFileAction::Restart => {
                let restart = shell_quote(compose_file);
                format!(
                    "cd {} && docker compose -f {restart} restart || docker compose -f {restart} up -d --build",
                    shell_quote(&self.workspace.root.absolute)
                )
            }
            ComposeFileAction::Down => format!(
                "cd {} && docker compose -f {} down",
                shell_quote(&self.workspace.root.absolute),
                shell_quote(compose_file)
            ),
            ComposeFileAction::Pull => format!(
                "cd {} && docker compose -f {} pull",
                shell_quote(&self.workspace.root.absolute),
                shell_quote(compose_file)
            ),
        };
        Ok(ShellCommandSpec::new("ssh", self.workspace.root.clone())
            .arg(self.host.clone())
            .arg("-t")
            .arg(remote))
    }
}

fn dockerfile_context_path(dockerfile_path: &str) -> String {
    Path::new(dockerfile_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn docker_image_tag(workspace_name: &str, context_path: &str) -> String {
    let source_name = if context_path == "." {
        workspace_name
    } else {
        Path::new(context_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
    };
    let tag = source_name
        .chars()
        .map(|ch| {
            let ch = ch.to_ascii_lowercase();
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(|ch| matches!(ch, '.' | '_' | '-'))
        .to_string();
    if tag.is_empty() {
        "image".to_string()
    } else {
        tag
    }
}
