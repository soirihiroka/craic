use std::sync::{Arc, Mutex};

use craic_agent::agent_history;
use craic_system::system::ProviderKind;
use craic_system::system::capabilities::shell::ShellCommandSpec;
use craic_system::terminal::TerminalCommand;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

include!("agent_session/types.rs");
include!("agent_session/history.rs");
include!("agent_session/terminal_launch.rs");
include!("agent_session/command_loop.rs");
