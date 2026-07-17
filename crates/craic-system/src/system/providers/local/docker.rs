use crate::system::capabilities::docker::{ComposeFileAction, DockerAccess};
use crate::system::capabilities::shell::ShellCommandSpec;
use crate::system::path::{WorkspacePath, WorkspaceRef};
use crate::system::providers::ssh::shell_quote;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct LocalDockerAccess {
    workspace: WorkspaceRef,
}

impl LocalDockerAccess {
    pub fn new(workspace: WorkspaceRef) -> Self {
        Self { workspace }
    }
}

impl DockerAccess for LocalDockerAccess {
    fn run_docker(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<Vec<u8>, String> {
        let working_dir = working_dir.unwrap_or(&self.workspace.root);
        log::debug!(
            "local docker command start workspace={} working_dir={} args={:?}",
            self.workspace.display_name,
            working_dir.display(),
            args
        );
        let output = Command::new("docker")
            .args(args)
            .current_dir(&working_dir.absolute)
            .output()
            .map_err(|err| format!("Failed to run docker: {err}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            log::warn!(
                "local docker command failed status={} working_dir={} stderr={}",
                output.status,
                working_dir.display(),
                stderr.trim()
            );
            return Err(format!("docker failed: {detail}"));
        }
        log::debug!(
            "local docker command complete workspace={} stdout_bytes={}",
            self.workspace.display_name,
            output.stdout.len()
        );
        Ok(output.stdout)
    }

    fn docker_command(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String> {
        let mut command = ShellCommandSpec::new(
            "docker",
            working_dir
                .cloned()
                .unwrap_or_else(|| self.workspace.root.clone()),
        );
        for arg in args {
            command = command.arg(arg.clone());
        }
        Ok(command)
    }

    fn build_image_command(&self, dockerfile: &WorkspacePath) -> Result<ShellCommandSpec, String> {
        let dockerfile_path = dockerfile.relative_or_empty();
        let context_path = dockerfile_context_path(dockerfile_path);
        let tag = format!(
            "{}:latest",
            docker_image_tag(&self.workspace, &context_path)
        );
        log::debug!(
            "local docker build command workspace={} dockerfile={}",
            self.workspace.display_name,
            dockerfile.display()
        );
        Ok(ShellCommandSpec::new("docker", self.workspace.root.clone())
            .arg("build")
            .arg("-f")
            .arg(dockerfile_path.to_string())
            .arg("-t")
            .arg(tag)
            .arg(context_path))
    }

    fn compose_file_command(
        &self,
        compose_file: &WorkspacePath,
        action: ComposeFileAction,
    ) -> Result<ShellCommandSpec, String> {
        let compose_file = compose_file.relative_or_empty().to_string();
        let quoted_compose_file = shell_quote(&compose_file);
        let command = match action {
            ComposeFileAction::Up => ShellCommandSpec::new("docker", self.workspace.root.clone())
                .arg("compose")
                .arg("-f")
                .arg(compose_file.as_str())
                .arg("up")
                .arg("-d")
                .arg("--build"),
            ComposeFileAction::Restart => {
                let restart = format!(
                    "docker compose -f {quoted_compose_file} restart || docker compose -f {quoted_compose_file} up -d --build"
                );
                ShellCommandSpec::new("sh", self.workspace.root.clone())
                    .arg("-lc")
                    .arg(restart)
            }
            ComposeFileAction::Down => ShellCommandSpec::new("docker", self.workspace.root.clone())
                .arg("compose")
                .arg("-f")
                .arg(compose_file.as_str())
                .arg("down"),
            ComposeFileAction::Pull => ShellCommandSpec::new("docker", self.workspace.root.clone())
                .arg("compose")
                .arg("-f")
                .arg(compose_file.as_str())
                .arg("pull"),
        };
        log::debug!(
            "local docker compose command workspace={} file={} action={:?}",
            self.workspace.display_name,
            compose_file,
            action
        );
        Ok(command)
    }
}

fn dockerfile_context_path(dockerfile_path: &str) -> String {
    Path::new(dockerfile_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn docker_image_tag(workspace: &WorkspaceRef, context_path: &str) -> String {
    let source_name = if context_path == "." {
        Some(workspace.display_name.as_str())
    } else {
        Path::new(context_path)
            .file_name()
            .and_then(|name| name.to_str())
    }
    .unwrap_or("image");

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
