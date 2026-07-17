use super::{BUN_ICON_NAME, RunCommand, RunItem};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn package_json_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join("package.json");
    path.is_file().then_some(path)
}

pub(super) fn bun_lock_path(repo_path: &Path) -> Option<PathBuf> {
    ["bun.lock", "bun.lockb"]
        .into_iter()
        .map(|name| repo_path.join(name))
        .find(|path| path.is_file())
}

pub(super) fn discover_bun_scripts(repo_path: &Path) -> Vec<RunItem> {
    let (Some(package_json_path), Some(_bun_lock_path)) =
        (package_json_path(repo_path), bun_lock_path(repo_path))
    else {
        return Vec::new();
    };

    let Ok(contents) = std::fs::read_to_string(&package_json_path) else {
        log::warn!(
            "quick action Bun script discovery skipped: failed to read {}",
            package_json_path.display()
        );
        return Vec::new();
    };

    let Ok(package_json) = serde_json::from_str::<Value>(&contents) else {
        log::warn!(
            "quick action Bun script discovery skipped: failed to parse {}",
            package_json_path.display()
        );
        return Vec::new();
    };

    let Some(scripts) = package_json.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };

    scripts
        .iter()
        .filter_map(|(script, command)| {
            if script.is_empty() || !command.is_string() {
                return None;
            }

            Some(RunItem {
                id: format!("bun:{script}"),
                label: script.to_string(),
                icon_name: BUN_ICON_NAME.to_string(),
                command: RunCommand::BunScript {
                    script: script.to_string(),
                },
            })
        })
        .collect()
}
