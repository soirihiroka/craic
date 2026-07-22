pub use craic_agent::agent_provider;
pub use craic_config as config;
pub use craic_project::{quick_action, workspace_config};
pub use craic_system::{system, workspace};
pub use craic_ui_terminal as terminal;
pub use craic_vcs::{bitbucket, git, github, gitlab};

mod crash_log;
mod ui;

pub use crash_log::install as install_crash_log;
pub use ui::{StartupOpenLocation, build_ui};
