//! Bounded connector and Tokio transport for the Codex App Server JSONL protocol.
//!
//! The wire protocol deliberately remains represented as tolerant JSON values. The small typed
//! request helpers in [`protocol`] cover the common chat path without coupling Craic to a
//! particular Codex source checkout.

mod client;
mod lifecycle;
pub mod protocol;
mod transport;
mod version;

pub use client::AppServer;
pub use lifecycle::AppServerConfig;
pub use lifecycle::AppServerError;
pub use lifecycle::AppServerEvent;
pub use lifecycle::ConnectionState;
pub use lifecycle::ExitStatus;
pub use version::CodexVersion;
pub use version::MINIMUM_CODEX_VERSION;
pub use version::VersionError;
pub use version::check_codex_version;
