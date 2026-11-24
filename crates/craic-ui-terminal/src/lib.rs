#![cfg(target_os = "linux")]

pub use craic_config as config;
pub use craic_system::system;

pub mod terminal;
pub mod ui;

pub mod gtk_terminal;
#[cfg(test)]
mod process;

pub use terminal::*;
