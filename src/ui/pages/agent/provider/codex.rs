use super::super::agent_shell_integration::{
    AgentNotification, AgentShellIntegration, TERMINAL_LOG_PREVIEW_CHARS, log_preview,
};
use crate::ui::agent_status::AgentActiveState;

use super::{
    AgentProvider, CommandSpec, command_binary, normalize_title_text, title_candidate,
    window_title_active_state,
};
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{SystemRef, WorkspaceRef};

pub(in crate::ui::pages::agent) static PROVIDER: Provider = Provider;
static SHELL_INTEGRATION: ShellIntegration = ShellIntegration;

pub(in crate::ui::pages::agent) struct Provider;
struct ShellIntegration;

impl AgentProvider for Provider {
    fn provider_id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn session_icon_name(&self) -> &'static str {
        "craic-codex-symbolic"
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
            command_binary("codex", shell)?,
            vec!["--cd".into(), workspace.root.absolute.clone().into()],
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
            command_binary("codex", shell)?,
            vec![
                "--cd".into(),
                workspace.root.absolute.clone().into(),
                "resume".into(),
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
        codex_title_from_text(text)
    }

    fn active_state(
        &self,
        window_title: Option<&str>,
        _recent_terminal_text: &dyn Fn() -> Option<String>,
    ) -> AgentActiveState {
        window_title_active_state(window_title).unwrap_or(AgentActiveState::Idle)
    }

    fn notification(&self, active_state: AgentActiveState, title: &str) -> AgentNotification {
        match active_state {
            AgentActiveState::Asking => AgentNotification::waiting_for_user("Codex", title),
            AgentActiveState::NewChat | AgentActiveState::Idle | AgentActiveState::Loading => {
                AgentNotification::ready("Codex", title)
            }
        }
    }
}

fn codex_title_from_text(text: &str) -> Option<String> {
    let lines = text.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let Some(title) = codex_prompt_title(line) else {
            continue;
        };

        if codex_prompt_title_is_menu_choice(&lines, index, &title) {
            log::debug!(
                "agent codex title candidate ignored reason=interactive_menu title={}",
                log_preview(&title, TERMINAL_LOG_PREVIEW_CHARS)
            );
            continue;
        }

        return Some(title);
    }

    None
}

fn codex_prompt_title(line: &str) -> Option<String> {
    let line = line.trim();
    let prompt = line
        .strip_prefix('›')
        .or_else(|| line.strip_prefix('>'))?
        .trim();
    title_candidate(prompt)
}

fn codex_prompt_title_is_menu_choice(lines: &[&str], index: usize, title: &str) -> bool {
    // Codex uses the same `›` prefix for picker selections and user prompts.
    // When `/model` opens at startup, selecting model/effort can otherwise lock
    // the session title to rows like "2. Medium (default) ...".
    if strip_numbered_menu_prefix(title).is_none() {
        return false;
    }

    has_recent_codex_picker_header(lines, index) || looks_like_standalone_codex_menu_title(title)
}

fn has_recent_codex_picker_header(lines: &[&str], index: usize) -> bool {
    let start = index.saturating_sub(32);
    lines[start..index]
        .iter()
        .any(|line| line_is_codex_picker_header(line))
}

fn line_is_codex_picker_header(line: &str) -> bool {
    let lower = normalize_title_text(line).to_ascii_lowercase();
    lower == "select model and effort" || lower.starts_with("select reasoning level")
}

fn looks_like_standalone_codex_menu_title(title: &str) -> bool {
    let Some(rest) = strip_numbered_menu_prefix(title) else {
        return false;
    };
    let lower = normalize_title_text(rest).to_ascii_lowercase();

    looks_like_codex_model_menu_option(&lower) || looks_like_codex_reasoning_menu_option(&lower)
}

fn looks_like_codex_model_menu_option(title: &str) -> bool {
    title.starts_with("gpt-")
        && (title.contains("(current)")
            || title.contains("frontier model")
            || title.contains("strong model")
            || title.contains("small, fast")
            || title.contains("cost-efficient"))
}

fn looks_like_codex_reasoning_menu_option(title: &str) -> bool {
    let Some(rest) = strip_effort_label(title) else {
        return false;
    };
    let rest = rest
        .trim_start()
        .strip_prefix("(default)")
        .unwrap_or(rest)
        .trim_start();

    rest.starts_with("fast responses")
        || rest.contains("reasoning depth")
        || rest.contains("everyday tasks")
        || rest.contains("complex problems")
}

fn strip_effort_label(title: &str) -> Option<&str> {
    title
        .strip_prefix("extra high")
        .or_else(|| title.strip_prefix("high"))
        .or_else(|| title.strip_prefix("medium"))
        .or_else(|| title.strip_prefix("low"))
        .or_else(|| title.strip_prefix("minimal"))
        .or_else(|| title.strip_prefix("auto"))
}

fn strip_numbered_menu_prefix(title: &str) -> Option<&str> {
    let title = title.trim_start();
    let (number, rest) = title.split_once('.')?;
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let rest = rest.trim_start();
    (!rest.is_empty()).then_some(rest)
}
