use super::{PYREFLY_ICON_NAME, RunCommand, RunItem};
use std::path::{Path, PathBuf};

pub(super) fn pyproject_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join("pyproject.toml");
    path.is_file().then_some(path)
}

pub(super) fn discover_pyrefly_targets(repo_path: &Path) -> Vec<RunItem> {
    let Some(pyproject_path) = pyproject_path(repo_path) else {
        return Vec::new();
    };

    let Ok(contents) = std::fs::read_to_string(&pyproject_path) else {
        log::warn!(
            "quick action Pyrefly discovery skipped: failed to read {}",
            pyproject_path.display()
        );
        return Vec::new();
    };

    let Ok(pyproject) = contents.parse::<toml::Value>() else {
        log::warn!(
            "quick action Pyrefly discovery skipped: failed to parse {}",
            pyproject_path.display()
        );
        return Vec::new();
    };

    if !pyproject
        .get("tool")
        .and_then(|tool| tool.get("pyrefly"))
        .is_some_and(toml::Value::is_table)
    {
        return Vec::new();
    }

    vec![RunItem {
        id: "pyrefly:check".to_string(),
        label: "Check (Pyrefly)".to_string(),
        icon_name: PYREFLY_ICON_NAME.to_string(),
        command: RunCommand::ShellCommand {
            command: "pyrefly check".to_string(),
        },
    }]
}
