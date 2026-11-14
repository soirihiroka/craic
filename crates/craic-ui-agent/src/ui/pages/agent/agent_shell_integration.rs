use craic_ui_terminal::vte::VteTerminal;
use std::cell::OnceCell;

use super::provider::AgentProvider;
use crate::ui::agent_status::AgentActiveState;

pub use craic_agent::terminal_provider::integration::{
    AgentNotification, AgentShellIntegration, TERMINAL_LOG_PREVIEW_CHARS, log_preview,
};

const TITLE_SCAN_ROWS: usize = 1000;

pub fn session_title(
    provider: &'static dyn AgentProvider,
    terminal: &VteTerminal,
    _log_scan: bool,
) -> Option<String> {
    let text = terminal.text_before_cursor(TITLE_SCAN_ROWS)?;
    let text = text.trim_start_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return None;
    }
    provider.shell_integration().title_from_text(text)
}

pub fn active_state(
    _session_id: u64,
    provider: &'static dyn AgentProvider,
    terminal: &VteTerminal,
    _log_scan: bool,
) -> AgentActiveState {
    let window_title = terminal.title();
    let recent_text = OnceCell::new();
    provider
        .shell_integration()
        .active_state(window_title.as_deref(), &|| {
            recent_text
                .get_or_init(|| {
                    terminal.recent_text(TITLE_SCAN_ROWS).and_then(|text| {
                        let text = text.trim_start_matches(['\n', '\r']);
                        (!text.trim().is_empty()).then(|| text.to_owned())
                    })
                })
                .clone()
        })
}
