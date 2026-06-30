pub(in crate::ui::pages::agent) mod agy;
pub(in crate::ui::pages::agent) mod codex;
pub(in crate::ui::pages::agent) mod opencode;

use std::ffi::OsString;
use std::path::PathBuf;

use super::agent_shell_integration::AgentShellIntegration;
use crate::system::{ProviderKind, SystemRef, WorkspacePath, WorkspaceRef};
use crate::ui::agent_status::AgentActiveState;

pub(in crate::ui::pages::agent) trait AgentProvider: Sync {
    fn default_title(&self) -> String {
        format!("New {} Chat", self.label())
    }
    fn provider_id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn session_icon_name(&self) -> &'static str;
    fn command(&self, system: &SystemRef, workspace: &WorkspaceRef) -> CommandSpec;
    fn restore_command(
        &self,
        system: &SystemRef,
        workspace: &WorkspaceRef,
        cli_session_id: &str,
    ) -> Result<CommandSpec, String>;
    fn shell_integration(&self) -> &'static dyn AgentShellIntegration;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::ui::pages::agent) struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    target_working_dir: WorkspacePath,
    spawn_working_dir: String,
}

impl CommandSpec {
    pub(in crate::ui::pages::agent) fn target(
        system: &SystemRef,
        workspace: &WorkspaceRef,
        program: impl Into<OsString>,
        args: Vec<OsString>,
    ) -> Self {
        let program = program.into();
        if system.provider_kind == ProviderKind::Ssh
            && let Some(host) = system.host.as_ref().map(|host| host.label().to_string())
        {
            let mut remote = format!(
                "cd {} && {}",
                shell_quote(&workspace.root.absolute),
                remote_command_start(&program)
            );
            for arg in &args {
                remote.push(' ');
                remote.push_str(&shell_quote(&arg.to_string_lossy()));
            }
            log::debug!(
                "agent command adapted for ssh host={} workspace={}",
                host,
                workspace.display_name
            );
            return Self {
                program: OsString::from("ssh"),
                args: vec![
                    OsString::from(host),
                    OsString::from("-t"),
                    OsString::from(remote),
                ],
                target_working_dir: workspace.root.clone(),
                spawn_working_dir: "/".to_string(),
            };
        }

        Self {
            program,
            args,
            target_working_dir: workspace.root.clone(),
            spawn_working_dir: workspace.root.absolute.clone(),
        }
    }

    pub(in crate::ui::pages::agent) fn display(&self) -> String {
        self.argv()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(in crate::ui::pages::agent) fn argv(&self) -> Vec<OsString> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }

    pub(in crate::ui::pages::agent) fn working_dir(&self) -> &str {
        &self.spawn_working_dir
    }

    pub(in crate::ui::pages::agent) fn target_working_dir(&self) -> &str {
        self.target_working_dir.display()
    }
}

pub(in crate::ui::pages::agent) fn all_providers() -> &'static [&'static dyn AgentProvider] {
    static PROVIDERS: [&'static dyn AgentProvider; 3] =
        [&codex::PROVIDER, &agy::PROVIDER, &opencode::PROVIDER];
    &PROVIDERS
}

pub(in crate::ui::pages::agent) fn default_provider() -> &'static dyn AgentProvider {
    &codex::PROVIDER
}

pub(in crate::ui::pages::agent) fn is_default_agent_title(title: &str) -> bool {
    title == "New Chat"
        || title == codex::PROVIDER.default_title().as_str()
        || title == agy::PROVIDER.default_title().as_str()
        || title == opencode::PROVIDER.default_title().as_str()
}

pub(in crate::ui::pages::agent) fn normalize_title_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn command_path(name: &str) -> PathBuf {
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let p = path.join(name);
            if p.is_file() {
                return p;
            }
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(name);
        if p.is_file() {
            return p;
        }
    }

    for dir in &[
        "/home/linuxbrew/.linuxbrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ] {
        let p = PathBuf::from(dir).join(name);
        if p.is_file() {
            return p;
        }
    }

    PathBuf::from(name)
}

fn command_binary(name: &str, system: &SystemRef) -> OsString {
    if system.provider_kind == ProviderKind::Local {
        command_path(name).into_os_string()
    } else {
        OsString::from(name)
    }
}

fn remote_command_start(program: &OsString) -> String {
    let program = program.to_string_lossy();
    if remote_command_name_can_use_path(&program) {
        let linuxbrew_path = format!("/home/linuxbrew/.linuxbrew/bin/{program}");
        let local_bin_path = format!("\"$HOME/.local/bin/{program}\"");
        return format!(
            "cmd=$(command -v {program} 2>/dev/null || {{ if [ -x {linuxbrew} ]; then printf '%s\\n' {linuxbrew}; elif [ -x {local_bin} ]; then printf '%s\\n' {local_bin}; else printf '%s\\n' {program}; fi; }}) && exec \"$cmd\"",
            program = shell_quote(&program),
            linuxbrew = shell_quote(&linuxbrew_path),
            local_bin = local_bin_path,
        );
    }

    format!("exec {}", shell_quote(&program))
}

fn remote_command_name_can_use_path(program: &str) -> bool {
    !program.is_empty()
        && !program.contains('/')
        && program
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
