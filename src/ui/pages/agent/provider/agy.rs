use super::super::agent_shell_integration::{AgentNotification, AgentShellIntegration};
use crate::ui::agent_status::AgentActiveState;

use super::{
    AgentProvider, CommandSpec, command_binary, is_spinner_frame, normalize_title_text,
    title_candidate, window_title_active_state,
};
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{SystemRef, WorkspaceRef};

pub(in crate::ui::pages::agent) static PROVIDER: Provider = Provider;
static SHELL_INTEGRATION: ShellIntegration = ShellIntegration;

pub(in crate::ui::pages::agent) struct Provider;
struct ShellIntegration;

impl AgentProvider for Provider {
    fn provider_id(&self) -> &'static str {
        "agy"
    }

    fn label(&self) -> &'static str {
        "AGY"
    }

    fn session_icon_name(&self) -> &'static str {
        "craic-antigravity-symbolic"
    }

    fn command(
        &self,
        shell: Option<&dyn ShellAccess>,
        system: &SystemRef,
        workspace: &WorkspaceRef,
    ) -> CommandSpec {
        CommandSpec::target(system, workspace, command_binary("agy", shell), Vec::new())
    }

    fn restore_command(
        &self,
        _shell: Option<&dyn ShellAccess>,
        _system: &SystemRef,
        _workspace: &WorkspaceRef,
        _cli_session_id: &str,
    ) -> Result<CommandSpec, String> {
        Err("AGY sessions cannot be restored from CLI history yet".to_string())
    }

    fn shell_integration(&self) -> &'static dyn AgentShellIntegration {
        &SHELL_INTEGRATION
    }
}

impl AgentShellIntegration for ShellIntegration {
    fn title_from_text(&self, text: &str) -> Option<String> {
        if text_has_interactive_ui(text) {
            return None;
        }

        agy_prompt_title_from_text(text)
    }

    fn active_state(
        &self,
        window_title: Option<&str>,
        recent_terminal_text: &dyn Fn() -> Option<String>,
    ) -> AgentActiveState {
        if let Some(text) = recent_terminal_text() {
            let active_state = terminal_text_active_state(&text);
            if active_state != AgentActiveState::Idle {
                return active_state;
            }
        }

        window_title_active_state(window_title).unwrap_or(AgentActiveState::Idle)
    }

    fn notification(&self, active_state: AgentActiveState, title: &str) -> AgentNotification {
        match active_state {
            AgentActiveState::Asking => AgentNotification::waiting_for_user("Antigravity", title),
            AgentActiveState::NewChat | AgentActiveState::Idle | AgentActiveState::Loading => {
                AgentNotification::ready("Antigravity", title)
            }
        }
    }
}

pub(in crate::ui::pages::agent) fn terminal_text_active_state(text: &str) -> AgentActiveState {
    if text_has_interactive_ui(text) {
        return AgentActiveState::Asking;
    }

    for (line_offset, line) in text.lines().rev().take(24).enumerate() {
        if line_needs_user_input(line) {
            return AgentActiveState::Asking;
        }
        if line_offset < 12 && line_is_active(line) {
            return AgentActiveState::Loading;
        }
    }

    AgentActiveState::Idle
}

fn agy_prompt_title_from_text(text: &str) -> Option<String> {
    text.lines().rev().find_map(agy_user_prompt_title)
}

fn agy_user_prompt_title(line: &str) -> Option<String> {
    let prompt = line.trim().strip_prefix('>')?.trim();
    if agy_prompt_is_ui_control(prompt) {
        return None;
    }

    title_candidate(prompt)
}

fn agy_prompt_is_ui_control(prompt: &str) -> bool {
    let lower = normalize_title_text(prompt).to_ascii_lowercase();
    lower.is_empty()
        || lower.starts_with('/')
        || lower.contains("(current)")
        || lower.starts_with("project")
        || lower.starts_with("shared with antigravity")
        || lower.starts_with("global")
        || lower.starts_with("command(")
        || lower.starts_with("gemini ")
        || lower.starts_with("claude ")
        || lower.starts_with("gpt-")
        || lower.starts_with("gpt ")
}

fn text_has_interactive_ui(text: &str) -> bool {
    text.lines().rev().take(32).any(line_is_interactive_ui)
}

fn line_is_interactive_ui(line: &str) -> bool {
    let line = normalize_title_text(line).to_ascii_lowercase();
    line.contains("keyboard:")
        || line == "switch model"
        || line == "permission config editor"
        || line.starts_with("permissions —")
        || line.contains("select a config scope to edit")
        || line.contains("allowlist") && line.contains("denylist") && line.contains("asklist")
}

fn line_is_active(line: &str) -> bool {
    if line_has_loading_spinner(line) {
        return true;
    }

    let line = normalize_title_text(line).to_ascii_lowercase();
    if line.starts_with('>') {
        return false;
    }

    line == "working"
        || line.starts_with("working...")
        || line.starts_with("loading")
        || line.starts_with("generating")
        || line.contains("esc to cancel")
}

fn line_needs_user_input(line: &str) -> bool {
    let line = normalize_title_text(line).to_ascii_lowercase();
    line.contains("requesting permission for:")
        || line.contains("do you want to proceed?")
        || line.contains("keyboard:")
        || line.contains("not signed in")
        || line == "switch model"
}

fn line_has_loading_spinner(line: &str) -> bool {
    let line = line.trim();
    let Some(spinner) = line.chars().next() else {
        return false;
    };
    if !is_agy_spinner_frame(spinner) {
        return false;
    }

    let text = normalize_title_text(&line[spinner.len_utf8()..]);
    !text.is_empty() && text.ends_with("...")
}

fn is_agy_spinner_frame(ch: char) -> bool {
    is_spinner_frame(ch)
        || matches!(
            ch,
            '⣾' | '⣽'
                | '⣻'
                | '⢿'
                | '⡿'
                | '⣟'
                | '⣯'
                | '⣷'
                | '⠁'
                | '⠂'
                | '⠄'
                | '⡀'
                | '⢀'
                | '⠠'
                | '⠐'
                | '⠈'
        )
}
