use super::shell::ShellCommandSpec;
use crate::system::path::WorkspacePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComposeFileAction {
    Up,
    Restart,
    Down,
    Pull,
}

pub(crate) trait DockerAccess: Send + Sync {
    fn run_docker(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<Vec<u8>, String>;
    fn docker_command(
        &self,
        args: &[String],
        working_dir: Option<&WorkspacePath>,
    ) -> Result<ShellCommandSpec, String>;
    fn build_image_command(&self, dockerfile: &WorkspacePath) -> Result<ShellCommandSpec, String>;
    fn compose_file_command(
        &self,
        compose_file: &WorkspacePath,
        action: ComposeFileAction,
    ) -> Result<ShellCommandSpec, String>;
}
