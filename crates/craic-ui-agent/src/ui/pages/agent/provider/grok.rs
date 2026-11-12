use super::super::agent_shell_integration::{AgentNotification, AgentShellIntegration};
use super::{
    AgentProvider, CommandSpec, command_binary, prompt_title_from_text, window_title_active_state,
};
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{SystemRef, WorkspaceRef};
use crate::ui::agent_status::AgentActiveState;

pub static PROVIDER: Provider = Provider;
static SHELL_INTEGRATION: ShellIntegration = ShellIntegration;
pub struct Provider;
struct ShellIntegration;

impl AgentProvider for Provider {
    fn provider_id(&self) -> &'static str {
        "grok"
    }
    fn label(&self) -> &'static str {
        "Grok CLI"
    }
    fn session_icon_name(&self) -> &'static str {
        "craic-grok-symbolic"
    }
    fn command(
        &self,
        shell: Option<&dyn ShellAccess>,
        system: &SystemRef,
        workspace: &WorkspaceRef,
    ) -> Result<CommandSpec, String> {
        Ok(CommandSpec::target(
            system,
            workspace,
            command_binary("grok", shell)?,
            vec!["--no-alt-screen".into()],
        ))
    }
    fn restore_command(
        &self,
        shell: Option<&dyn ShellAccess>,
        system: &SystemRef,
        workspace: &WorkspaceRef,
        cli_session_id: &str,
    ) -> Result<CommandSpec, String> {
        Ok(CommandSpec::target(
            system,
            workspace,
            command_binary("grok", shell)?,
            vec![
                "--no-alt-screen".into(),
                "--resume".into(),
                cli_session_id.into(),
            ],
        ))
    }
    fn shell_integration(&self) -> &'static dyn AgentShellIntegration {
        &SHELL_INTEGRATION
    }
}

impl AgentShellIntegration for ShellIntegration {
    fn title_from_text(&self, text: &str) -> Option<String> {
        prompt_title_from_text(text)
    }
    fn active_state(
        &self,
        window_title: Option<&str>,
        recent_terminal_text: &dyn Fn() -> Option<String>,
    ) -> AgentActiveState {
        window_title_active_state(window_title).unwrap_or_else(|| {
            recent_terminal_text()
                .as_deref()
                .map(super::opencode::terminal_text_active_state)
                .unwrap_or(AgentActiveState::Idle)
        })
    }
    fn notification(&self, state: AgentActiveState, title: &str) -> AgentNotification {
        match state {
            AgentActiveState::Asking => AgentNotification::waiting_for_user("Grok", title),
            _ => AgentNotification::ready("Grok", title),
        }
    }
}
