//! Synchronous transport for the Codex App Server JSONL protocol.
//!
//! The wire protocol deliberately remains represented as tolerant JSON values. The small typed
//! request helpers in [`protocol`] cover the common chat path without coupling Craic to a
//! particular Codex source checkout.

mod client;
pub mod protocol;
mod version;

pub use client::AppServer;
pub use client::AppServerConfig;
pub use client::AppServerError;
pub use client::AppServerEvent;
pub use client::ConnectionState;
pub use client::ExitStatus;
pub use version::CodexVersion;
pub use version::MINIMUM_CODEX_VERSION;
pub use version::VersionError;
pub use version::check_codex_version;
