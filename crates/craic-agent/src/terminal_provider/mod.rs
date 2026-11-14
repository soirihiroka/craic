pub mod agy;
pub mod codex;
pub mod grok;
pub mod opencode;

use std::ffi::OsString;

pub mod integration;
use self::integration::AgentShellIntegration;
use crate::agent_status::AgentActiveState;
use crate::system::capabilities::shell::ShellAccess;
use crate::system::{SystemRef, WorkspaceRef};

pub use craic_system::terminal::TerminalCommand as CommandSpec;

pub trait AgentProvider: Sync {
    fn default_title(&self) -> String {
        format!("New {} Chat", self.label())
    }
    fn provider_id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn session_icon_name(&self) -> &'static str;
    fn command(
        &self,
        shell: Option<&dyn ShellAccess>,
        system: &SystemRef,
        workspace: &WorkspaceRef,
    ) -> Result<CommandSpec, String>;
    fn restore_command(
        &self,
        shell: Option<&dyn ShellAccess>,
        system: &SystemRef,
        workspace: &WorkspaceRef,
        cli_session_id: &str,
    ) -> Result<CommandSpec, String>;
    fn shell_integration(&self) -> &'static dyn AgentShellIntegration;
}

fn target_command(
    shell: Option<&dyn ShellAccess>,
    workspace: &WorkspaceRef,
    program: OsString,
    args: Vec<OsString>,
) -> Result<CommandSpec, String> {
    let shell = shell.ok_or_else(|| "Shell access is unavailable.".to_owned())?;
    let program = program
        .to_str()
        .ok_or_else(|| "Terminal program is not valid UTF-8.".to_owned())?;
    let args = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "Terminal argument is not valid UTF-8.".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut command = shell
        .command(&workspace.root, program, &args)?
        .terminal_command();
    command.target_working_directory = workspace.root.absolute.clone();
    command.utf8_argv()?;
    Ok(command)
}

pub const PROVIDERS: [&'static dyn AgentProvider; 4] = [
    &codex::PROVIDER,
    &grok::PROVIDER,
    &opencode::PROVIDER,
    &agy::PROVIDER,
];

pub fn all_providers() -> &'static [&'static dyn AgentProvider] {
    &PROVIDERS
}

pub fn is_default_agent_title(title: &str) -> bool {
    title == "New Chat"
        || title == grok::PROVIDER.default_title().as_str()
        || title == codex::PROVIDER.default_title().as_str()
        || title == agy::PROVIDER.default_title().as_str()
        || title == opencode::PROVIDER.default_title().as_str()
}

pub fn normalize_title_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn command_binary(name: &str, shell: Option<&dyn ShellAccess>) -> Result<OsString, String> {
    let shell =
        shell.ok_or_else(|| format!("Shell access is unavailable; cannot resolve {name}."))?;
    let Some(path) = shell.which(name)? else {
        return Err(format!("{name} was not found on the fast shell path."));
    };
    let path = path.trim();
    if !path.contains('/') {
        return Err(format!(
            "{name} resolved to {path}, not an executable path."
        ));
    }
    log::debug!("agent command resolved program={} path={}", name, path);
    Ok(OsString::from(path))
}

fn window_title_active_state(title: Option<&str>) -> Option<AgentActiveState> {
    let title = title?;

    if window_title_needs_user_input(title) {
        return Some(AgentActiveState::Asking);
    }

    if window_title_has_active_progress(title) {
        return Some(AgentActiveState::Loading);
    }

    None
}

fn window_title_has_active_progress(title: &str) -> bool {
    let title = normalize_title_text(title);
    if title.is_empty() {
        return false;
    }

    title.chars().any(is_spinner_frame)
        || title
            .split(|ch| matches!(ch, '|' | '·' | '•'))
            .any(|segment| {
                let lower = normalize_title_text(segment).to_ascii_lowercase();
                lower.contains("action required")
                    || lower == "working"
                    || lower == "thinking"
                    || lower == "waiting"
                    || lower == "starting"
                    || lower.starts_with("tasks ")
            })
}

fn window_title_needs_user_input(title: &str) -> bool {
    normalize_title_text(title)
        .to_ascii_lowercase()
        .contains("action required")
}

fn prompt_title_from_text(text: &str) -> Option<String> {
    text.lines().find_map(user_prompt_title)
}

fn user_prompt_title(line: &str) -> Option<String> {
    let line = line.trim();
    let prompt = line
        .strip_prefix('›')
        .or_else(|| line.strip_prefix('>'))?
        .trim();
    title_candidate(prompt)
}

fn title_candidate(text: &str) -> Option<String> {
    let title = normalize_title_text(text);
    if title.is_empty() || ignored_title(&title) {
        return None;
    }
    Some(title)
}

fn ignored_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower == "new chat"
        || lower.starts_with('/')
        || lower.starts_with("tip:")
        || lower.starts_with("model:")
        || lower.starts_with("directory:")
        || lower.starts_with("you have ")
        || lower.starts_with("model changed")
        || looks_like_model_status_title(&lower)
        || lower.contains("openai codex")
        || lower.contains("antigravity cli")
        || lower.contains("using agy cli")
        || lower == "agy"
        || lower.starts_with("find and fix a bug in @filename")
        || lower.contains("what do you want to tackle")
}

fn looks_like_model_status_title(title: &str) -> bool {
    let mut parts = title.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };

    first.starts_with("gpt-")
        && parts.any(|part| {
            matches!(
                part.trim_matches(|ch: char| !ch.is_ascii_alphanumeric()),
                "low" | "medium" | "high"
            )
        })
}

fn is_spinner_frame(ch: char) -> bool {
    matches!(
        ch,
        '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏'
    )
}
