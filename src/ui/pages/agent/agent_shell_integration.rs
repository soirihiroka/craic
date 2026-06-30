use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;
use vte4::prelude::*;

use super::provider::{AgentProvider, is_default_agent_title, normalize_title_text};
use crate::ui::agent_status::AgentActiveState;

const TITLE_SCAN_ROWS: i64 = 1000;
pub(in crate::ui::pages::agent) const TERMINAL_LOG_PREVIEW_CHARS: usize = 1200;

struct TerminalTextScan {
    text: String,
    cursor_row: i64,
    start_row: i64,
    end_row: i64,
    end_col: i64,
    trimmed_empty: bool,
}

struct ActiveStateTextScan {
    text: String,
    cursor_row: i64,
    visible_rows: i64,
    start_row: i64,
    end_row: i64,
    end_col: i64,
    trimmed_empty: bool,
}

#[derive(Debug)]
pub(in crate::ui::pages::agent) struct AgentNotification {
    pub(in crate::ui::pages::agent) summary: String,
    pub(in crate::ui::pages::agent) body: String,
}

impl AgentNotification {
    pub(in crate::ui::pages::agent) fn new(
        summary: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            body: body.into(),
        }
    }

    pub(in crate::ui::pages::agent) fn ready(agent_name: &str, title: &str) -> Self {
        let title = notification_title(title);
        let body = title
            .map(|title| format!("{title} is ready"))
            .unwrap_or_else(|| format!("{agent_name} is ready"));
        Self::new(format!("{agent_name} is ready"), body)
    }

    pub(in crate::ui::pages::agent) fn waiting_for_user(agent_name: &str, title: &str) -> Self {
        let title = notification_title(title);
        let body = title
            .map(|title| format!("{title} needs your input"))
            .unwrap_or_else(|| format!("{agent_name} needs your input"));
        Self::new(format!("{agent_name} needs input"), body)
    }
}

pub(in crate::ui::pages::agent) trait AgentShellIntegration:
    Sync
{
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

    fn log_spawn_completed_after_close(&self, pid: glib::Pid, command: &str) {
        log::debug!(
            "agent command spawn completed after close pid={} command={}",
            pid.0,
            command
        );
    }

    fn log_spawn_failed(&self, command: &str, err: &glib::Error) {
        log::warn!("agent command spawn failed command={command}: {err}");
    }

    fn log_spawn_failed_after_close(&self, command: &str, err: &glib::Error) {
        log::debug!("agent command spawn failed after close command={command}: {err}");
    }

    fn log_child_exited(&self, status: i32, message: &str) {
        log::info!("agent child exited status={} message={}", status, message);
    }

    fn log_child_exit_ignored_while_closing(&self, status: i32) {
        log::debug!("agent child exit ignored while closing status={status}");
    }
}

fn notification_title(title: &str) -> Option<String> {
    let title = normalize_title_text(title);
    (!title.is_empty() && !is_default_agent_title(&title)).then_some(title)
}

pub(in crate::ui::pages::agent) fn session_title(
    provider: &'static dyn AgentProvider,
    terminal: &vte4::Terminal,
    log_scan: bool,
) -> Option<String> {
    let scan = recent_terminal_text(terminal)?;
    let title = provider.shell_integration().title_from_text(&scan.text);
    if log_scan {
        log::debug!(
            "agent terminal text scan provider={} cursor_row={} start_row={} end_row={} end_col={} bytes={} trimmed_empty={} title={:?} preview={}",
            provider.label(),
            scan.cursor_row,
            scan.start_row,
            scan.end_row,
            scan.end_col,
            scan.text.len(),
            scan.trimmed_empty,
            title,
            log_preview(&scan.text, TERMINAL_LOG_PREVIEW_CHARS)
        );
    }
    (!scan.trimmed_empty).then_some(title).flatten()
}

pub(in crate::ui::pages::agent) fn active_state(
    session_id: u64,
    provider: &'static dyn AgentProvider,
    terminal: &vte4::Terminal,
    log_scan: bool,
) -> AgentActiveState {
    let window_title = terminal_window_title(terminal);
    let recent_text = Rc::new(RefCell::new(None::<Option<String>>));
    let recent_scan = Rc::new(RefCell::new(None::<ActiveStateTextScan>));
    let active_state = provider.shell_integration().active_state(
        window_title.as_ref().map(|title| title.as_str()),
        &|| {
            if let Some(text) = recent_text.borrow().as_ref() {
                return text.clone();
            }

            let text = recent_terminal_active_state_text(terminal).and_then(|scan| {
                let text = (!scan.trimmed_empty).then(|| scan.text.clone());
                recent_scan.replace(Some(scan));
                text
            });
            recent_text.replace(Some(text.clone()));
            text
        },
    );
    let text_for_log = recent_text.borrow().as_ref().and_then(|text| {
        text.as_ref()
            .map(|text| log_tail_preview(text, TERMINAL_LOG_PREVIEW_CHARS))
    });

    if log_scan {
        if let Some(scan) = recent_scan.borrow().as_ref() {
            log::debug!(
                "agent terminal active_state text scan cursor_row={} visible_rows={} start_row={} end_row={} end_col={} bytes={} trimmed_empty={} tail_preview={}",
                scan.cursor_row,
                scan.visible_rows,
                scan.start_row,
                scan.end_row,
                scan.end_col,
                scan.text.len(),
                scan.trimmed_empty,
                log_tail_preview(&scan.text, TERMINAL_LOG_PREVIEW_CHARS)
            );
        }
        log::debug!(
            "agent terminal active_state session_id={} provider={} active_state={:?} window_title={:?} recent_text_preview={:?}",
            session_id,
            provider.label(),
            active_state,
            window_title,
            text_for_log
        );
    }
    active_state
}

fn terminal_window_title(terminal: &vte4::Terminal) -> Option<glib::GString> {
    terminal.property::<Option<glib::GString>>("window-title")
}

fn recent_terminal_text(terminal: &vte4::Terminal) -> Option<TerminalTextScan> {
    let (_cursor_column, cursor_row) = terminal.cursor_position();
    if cursor_row <= 0 {
        return None;
    }

    let end_row = cursor_row - 1;
    let start_row = (end_row - TITLE_SCAN_ROWS).max(0);
    let end_col = terminal.column_count().max(1);
    let (text, _) = terminal.text_range_format(vte4::Format::Text, start_row, 0, end_row, end_col);
    let text = text?
        .trim_start_matches(|ch| matches!(ch, '\n' | '\r'))
        .to_string();
    let trimmed_empty = text.trim().is_empty();
    Some(TerminalTextScan {
        text,
        cursor_row,
        start_row,
        end_row,
        end_col,
        trimmed_empty,
    })
}

fn recent_terminal_active_state_text(terminal: &vte4::Terminal) -> Option<ActiveStateTextScan> {
    let (_cursor_column, cursor_row) = terminal.cursor_position();
    let visible_end_row = terminal.row_count().saturating_sub(1);
    let cursor_end_row = cursor_row.saturating_sub(1);
    let end_row = visible_end_row.max(cursor_end_row);
    let start_row = (end_row - TITLE_SCAN_ROWS).max(0);
    let end_col = terminal.column_count().max(1);
    let (text, _) = terminal.text_range_format(vte4::Format::Text, start_row, 0, end_row, end_col);
    let text = text?
        .trim_start_matches(|ch| matches!(ch, '\n' | '\r'))
        .to_string();
    let trimmed_empty = text.trim().is_empty();
    Some(ActiveStateTextScan {
        text,
        cursor_row,
        visible_rows: terminal.row_count(),
        start_row,
        end_row,
        end_col,
        trimmed_empty,
    })
}

pub(in crate::ui::pages::agent) fn log_preview(text: &str, max_chars: usize) -> String {
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

pub(in crate::ui::pages::agent) fn log_tail_preview(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(max_chars);
    let mut preview = String::with_capacity(chars.len().min(max_chars));
    if start > 0 {
        preview.push_str("...");
    }
    for ch in chars[start..].iter().copied() {
        match ch {
            '\n' => preview.push_str("\\n"),
            '\r' => preview.push_str("\\r"),
            '\t' => preview.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write as _;
                write!(preview, "\\u{:04x}", ch as u32)
                    .expect("writing to a String should not fail");
            }
            ch => preview.push(ch),
        }
    }
    preview
}
