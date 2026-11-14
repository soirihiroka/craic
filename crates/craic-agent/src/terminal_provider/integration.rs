use super::{AgentProvider, is_default_agent_title, normalize_title_text};
use crate::agent_status::AgentActiveState;
use std::process::ExitStatus;

pub const TERMINAL_LOG_PREVIEW_CHARS: usize = 1200;

#[derive(Debug)]
pub struct AgentNotification {
    pub summary: String,
    pub body: String,
}

impl AgentNotification {
    pub fn new(summary: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            body: body.into(),
        }
    }

    pub fn ready(agent_name: &str, title: &str) -> Self {
        let title = notification_title(title);
        let body = title
            .map(|title| format!("{title} is ready"))
            .unwrap_or_else(|| format!("{agent_name} is ready"));
        Self::new(format!("{agent_name} is ready"), body)
    }

    pub fn waiting_for_user(agent_name: &str, title: &str) -> Self {
        let title = notification_title(title);
        let body = title
            .map(|title| format!("{title} needs your input"))
            .unwrap_or_else(|| format!("{agent_name} needs your input"));
        Self::new(format!("{agent_name} needs input"), body)
    }
}

pub trait AgentShellIntegration: Sync {
    fn title_from_text(&self, text: &str) -> Option<String>;

    fn active_state(
        &self,
        window_title: Option<&str>,
        recent_terminal_text: &dyn Fn() -> Option<String>,
    ) -> AgentActiveState;

    fn notification(&self, active_state: AgentActiveState, title: &str) -> AgentNotification {
        match active_state {
            AgentActiveState::Asking => AgentNotification::waiting_for_user("Agent", title),
            AgentActiveState::NewChat | AgentActiveState::Idle | AgentActiveState::Loading => {
                AgentNotification::ready("Agent", title)
            }
        }
    }

    fn log_session_create(
        &self,
        session_id: u64,
        provider: &'static dyn AgentProvider,
        title: &str,
        working_dir: &str,
        command: &str,
    ) {
        log::info!(
            "agent session create session_id={} provider={} title={} working_dir={} command={}",
            session_id,
            provider.label(),
            log_preview(title, TERMINAL_LOG_PREVIEW_CHARS),
            working_dir,
            command
        );
    }

    fn log_spawn_requested(&self, working_dir: &str, command: &str, env_count: usize) {
        log::info!(
            "agent command spawn requested working_dir={} argv={} env_count={}",
            working_dir,
            command,
            env_count
        );
    }

    fn log_spawned(&self, pid: i32, command: &str) {
        log::info!("agent command spawned pid={} command={}", pid, command);
    }

    fn log_spawn_completed_after_close(&self, pid: i32, command: &str) {
        log::debug!(
            "agent command spawn completed after close pid={} command={command}",
            pid
        );
    }

    fn log_spawn_failed(&self, command: &str, err: &str) {
        log::warn!("agent command spawn failed command={command}: {err}");
    }

    fn log_spawn_failed_after_close(&self, command: &str, err: &str) {
        log::debug!("agent command spawn failed after close command={command}: {err}");
    }

    fn log_child_exited(&self, status: ExitStatus, message: &str) {
        log::info!("agent child exited status={status:?} message={message}");
    }

    fn log_child_exit_ignored_while_closing(&self, status: ExitStatus) {
        log::debug!("agent child exit ignored while closing status={status:?}");
    }
}

fn notification_title(title: &str) -> Option<String> {
    let title = normalize_title_text(title);
    (!title.is_empty() && !is_default_agent_title(&title)).then_some(title)
}

pub fn log_preview(text: &str, max_chars: usize) -> String {
    let mut preview = String::with_capacity(text.len().min(max_chars));
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        match ch {
            '\n' => preview.push_str("\\n"),
            '\r' => preview.push_str("\\r"),
            '\t' => preview.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                write!(preview, "\\u{{{:x}}}", ch as u32)
                    .expect("writing to a String should not fail");
            }
            ch => preview.push(ch),
        }
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}
