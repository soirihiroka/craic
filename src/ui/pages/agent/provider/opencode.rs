use super::super::agent_shell_integration::{AgentNotification, AgentShellIntegration};
use crate::ui::agent_status::AgentActiveState;

use super::{
    AgentProvider, CommandSpec, command_binary, is_spinner_frame, normalize_title_text,
    prompt_title_from_text, title_candidate, window_title_active_state,
};
use crate::system::{SystemRef, WorkspaceRef};

pub(in crate::ui::pages::agent) static PROVIDER: Provider = Provider;
static SHELL_INTEGRATION: ShellIntegration = ShellIntegration;

pub(in crate::ui::pages::agent) struct Provider;
struct ShellIntegration;

impl AgentProvider for Provider {
    fn provider_id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn session_icon_name(&self) -> &'static str {
        "opencode-logo-dark-square"
    }

    fn command(&self, system: &SystemRef, workspace: &WorkspaceRef) -> CommandSpec {
        CommandSpec::target(
            system,
            workspace,
            command_binary("opencode", system),
            Vec::new(),
        )
    }

    fn restore_command(
        &self,
        _system: &SystemRef,
        _workspace: &WorkspaceRef,
        _cli_session_id: &str,
    ) -> Result<CommandSpec, String> {
        Err("OpenCode sessions cannot be restored from CLI history yet".to_string())
    }

    fn shell_integration(&self) -> &'static dyn AgentShellIntegration {
        &SHELL_INTEGRATION
    }
}

impl AgentShellIntegration for ShellIntegration {
    fn title_from_text(&self, text: &str) -> Option<String> {
        opencode_title_from_text(text).or_else(|| prompt_title_from_text(text))
    }

    fn active_state(
        &self,
        window_title: Option<&str>,
        recent_terminal_text: &dyn Fn() -> Option<String>,
    ) -> AgentActiveState {
        if window_title_is_question_prompt(window_title) {
            return AgentActiveState::Asking;
        }

        if let Some(active_state) = window_title_active_state(window_title) {
            return active_state;
        }

        recent_terminal_text()
            .as_deref()
            .map(terminal_text_active_state)
            .unwrap_or(AgentActiveState::Idle)
    }

    fn notification(&self, active_state: AgentActiveState, title: &str) -> AgentNotification {
        match active_state {
            AgentActiveState::Asking => AgentNotification::waiting_for_user("OpenCode", title),
            AgentActiveState::NewChat | AgentActiveState::Idle | AgentActiveState::Loading => {
                AgentNotification::ready("OpenCode", title)
            }
        }
    }
}

pub(in crate::ui::pages::agent) fn terminal_text_active_state(text: &str) -> AgentActiveState {
    if text_needs_user_input(text) {
        AgentActiveState::Asking
    } else if text.lines().rev().take(32).any(line_has_interrupt_hint)
        || text.lines().rev().take(12).any(line_is_active)
    {
        AgentActiveState::Loading
    } else {
        AgentActiveState::Idle
    }
}

fn window_title_is_question_prompt(title: Option<&str>) -> bool {
    title
        .map(normalize_title_text)
        .map(|title| title.to_ascii_lowercase())
        .is_some_and(|title| title.contains("requesting") || title.contains("asking"))
}

fn opencode_title_from_text(text: &str) -> Option<String> {
    text.lines().find_map(user_message_title)
}

fn user_message_title(line: &str) -> Option<String> {
    let line = line.trim_start();
    if !line.starts_with('┃') {
        return None;
    }

    title_candidate(strip_line_prefix(line))
}

fn line_is_active(line: &str) -> bool {
    let line = strip_line_prefix(line);
    let normalized = normalize_title_text(line);
    let lower = normalized.to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    line_starts_with_spinner_activity(&normalized)
        || lower.starts_with("⋯ ")
        || lower == "thinking"
        || lower.starts_with("thinking:")
        || (lower.starts_with("~ ") && lower.ends_with("..."))
        || [
            "writing command...",
            "preparing write...",
            "finding files...",
            "reading file...",
            "searching content...",
            "fetching from the web...",
            "searching web...",
            "delegating...",
            "preparing edit...",
            "preparing patch...",
            "updating todos...",
            "asking questions...",
            "loading skill...",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn line_starts_with_spinner_activity(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    if !is_spinner_frame(first) {
        return false;
    }

    let lower = normalize_title_text(&line[first.len_utf8()..]).to_ascii_lowercase();
    lower.starts_with("thinking")
        || lower.starts_with("reading")
        || lower.starts_with("writing")
        || lower.starts_with("searching")
        || lower.starts_with("running")
}

fn line_has_interrupt_hint(line: &str) -> bool {
    let line = strip_line_prefix(line);
    let lower = normalize_title_text(line).to_ascii_lowercase();
    lower.contains("esc to interrupt")
        || lower.contains("ctrl+c to interrupt")
        || lower.contains("ctrl-c to interrupt")
        || (lower.contains("interrupt") && lower.contains("esc"))
}

fn line_needs_user_input(line: &str) -> bool {
    let line = strip_line_prefix(line);
    let lower = normalize_title_text(line).to_ascii_lowercase();
    lower.contains("permission required")
        || lower.contains("allow once")
        || lower.contains("allow always")
        || lower.contains("reject permission")
}

fn text_needs_user_input(text: &str) -> bool {
    if text.lines().rev().take(64).any(line_needs_user_input) {
        return true;
    }

    let recent_text = text.lines().rev().take(64).collect::<Vec<_>>().join(" ");
    let lower = normalize_title_text(&recent_text).to_ascii_lowercase();
    lower.contains("permission required")
        || lower.contains("allow once")
        || lower.contains("allow always")
        || lower.contains("reject permission")
        || lower.contains("enter submit")
        || lower.contains("esc dismiss")
        || lower.contains("escape dismiss")
        || lower.contains("↑↓ select")
        || lower.contains("select enter")
        || lower.contains("type your own answer")
}

fn strip_line_prefix(line: &str) -> &str {
    let line = line.trim_start();
    line.strip_prefix('┃')
        .or_else(|| line.strip_prefix('│'))
        .unwrap_or(line)
        .trim_start()
}
