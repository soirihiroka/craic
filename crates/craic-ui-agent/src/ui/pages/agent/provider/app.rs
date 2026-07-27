use super::super::agent_shell_integration::AgentShellIntegration;
use super::{AgentProvider, CommandSpec};
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{SystemRef, WorkspaceRef};

pub static PROVIDER: Provider = Provider;

pub struct Provider;

impl AgentProvider for Provider {
    fn provider_id(&self) -> &'static str {
        "codex-app"
    }

    fn label(&self) -> &'static str {
        "App"
    }

    fn session_icon_name(&self) -> &'static str {
        "craic-codex-symbolic"
    }

    fn command(
        &self,
        _shell: Option<&dyn ShellAccess>,
        _system: &SystemRef,
        _workspace: &WorkspaceRef,
    ) -> Result<CommandSpec, String> {
        Err("App sessions are started through the Codex App Server.".to_string())
    }

    fn restore_command(
        &self,
        _shell: Option<&dyn ShellAccess>,
        _system: &SystemRef,
        _workspace: &WorkspaceRef,
        _cli_session_id: &str,
    ) -> Result<CommandSpec, String> {
        Err("App sessions are restored through the Codex App Server.".to_string())
    }

    fn shell_integration(&self) -> &'static dyn AgentShellIntegration {
        super::codex::PROVIDER.shell_integration()
    }
}
