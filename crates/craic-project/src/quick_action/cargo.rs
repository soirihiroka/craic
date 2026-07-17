use super::{CARGO_ICON_NAME, RunCommand, RunItem};
use std::path::{Path, PathBuf};

pub(super) fn cargo_manifest_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join("Cargo.toml");
    path.is_file().then_some(path)
}

pub(super) fn discover_cargo_targets(repo_path: &Path) -> Vec<RunItem> {
    if cargo_manifest_path(repo_path).is_none() {
        return Vec::new();
    }

    [
        ("build", "Build (Cargo)", "cargo build"),
        ("run", "Run (Cargo)", "cargo run"),
        ("check", "Check (Cargo)", "cargo check"),
        ("test", "Test (Cargo)", "cargo test"),
    ]
    .into_iter()
    .map(|(id, label, command)| RunItem {
        id: format!("cargo:{id}"),
        label: label.to_string(),
        icon_name: CARGO_ICON_NAME.to_string(),
        command: RunCommand::ShellCommand {
            command: command.to_string(),
        },
    })
    .collect()
}
