use gtk::glib;
use craic_ui_terminal::alacritty::AlacrittyTerminal;
use std::cell::RefCell;
use std::process::ExitStatus;
use std::rc::Rc;

use super::provider::{AgentProvider, is_default_agent_title, normalize_title_text};
use crate::ui::agent_status::AgentActiveState;

const TITLE_SCAN_ROWS: i64 = 1000;
pub const TERMINAL_LOG_PREVIEW_CHARS: usize = 1200;

struct TerminalTextScan {
    text: String,
    trimmed_empty: bool,
}

struct ActiveStateTextScan {
    text: String,
    trimmed_empty: bool,
}

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

    fn log_spawned(&self, pid: glib::Pid, command: &str) {
        log::info!("agent command spawned pid={} command={}", pid.0, command);
    }

    fn log_spawn_failed(&self, command: &str, err: &str) {
        log::warn!("agent command spawn failed command={command}: {err}");
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

pub fn session_title(
    provider: &'static dyn AgentProvider,
    terminal: &AlacrittyTerminal,
    _log_scan: bool,
) -> Option<String> {
    let scan = recent_terminal_text(terminal)?;
    let title = provider.shell_integration().title_from_text(&scan.text);
    (!scan.trimmed_empty).then_some(title).flatten()
}

pub fn active_state(
    _session_id: u64,
    provider: &'static dyn AgentProvider,
    terminal: &AlacrittyTerminal,
    _log_scan: bool,
) -> AgentActiveState {
    let window_title = terminal_window_title(terminal);
    let recent_text = Rc::new(RefCell::new(None::<Option<String>>));
    let active_state = provider.shell_integration().active_state(
        window_title.as_ref().map(|title| title.as_str()),
        &|| {
            if let Some(text) = recent_text.borrow().as_ref() {
                return text.clone();
            }

            let text = recent_terminal_active_state_text(terminal).and_then(|scan| {
                let text = (!scan.trimmed_empty).then(|| scan.text.clone());
                text
            });
            recent_text.replace(Some(text.clone()));
            text
        },
    );
    active_state
}

fn terminal_window_title(terminal: &AlacrittyTerminal) -> Option<String> {
    terminal.title()
}

fn recent_terminal_text(terminal: &AlacrittyTerminal) -> Option<TerminalTextScan> {
    let text = terminal
        .text_before_cursor(TITLE_SCAN_ROWS as usize)?
        .trim_start_matches(|ch| matches!(ch, '\n' | '\r'))
        .to_string();
    let trimmed_empty = text.trim().is_empty();
    Some(TerminalTextScan {
        text,
        trimmed_empty,
    })
}

fn recent_terminal_active_state_text(
    terminal: &AlacrittyTerminal,
) -> Option<ActiveStateTextScan> {
    let text = terminal
        .recent_text(TITLE_SCAN_ROWS as usize)?
        .trim_start_matches(|ch| matches!(ch, '\n' | '\r'))
        .to_string();
    let trimmed_empty = text.trim().is_empty();
    Some(ActiveStateTextScan {
        text,
        trimmed_empty,
    })
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
