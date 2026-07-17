pub use craic_agent::{agent_history, agent_provider, agent_status, agent_usage};
pub use craic_config as config;
pub use craic_system::system;
pub use craic_vcs::git;

pub const AGENT_SESSION_NOTIFICATION_ACTION: &str = "open-agent-session";
pub const AGENT_SESSION_NOTIFICATION_DETAILED_ACTION: &str = "app.open-agent-session";

pub fn agent_session_notification_id(session_id: u64) -> String {
    format!("agent-session-{session_id}")
}

pub mod ui {
    pub use crate::{AGENT_SESSION_NOTIFICATION_DETAILED_ACTION, agent_session_notification_id};
    pub use craic_agent::{agent_history, agent_status, agent_usage};
    pub use craic_ui_core::ui::{canvas_scroll, components};

    pub mod pages {
        pub use craic_ui_core::ui::pages::*;

        mod agent;

        pub use agent::AgentPage;
    }
}

pub use ui::pages::AgentPage;
