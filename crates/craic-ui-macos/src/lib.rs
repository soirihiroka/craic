#![cfg(target_os = "macos")]
#![deny(unsafe_op_in_unsafe_fn)]
#![recursion_limit = "256"]

mod agent_session;
mod application;
mod code_view;
mod commit_composer;
mod diff_view;
mod dispatcher;
mod image_view;
mod sqlite_preview;
mod terminal_view;

pub use application::run;
pub use dispatcher::AppKitDispatcher;
