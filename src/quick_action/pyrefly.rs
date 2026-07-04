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

    let contents = match std::fs::read_to_string(&pyproject_path) {
        Ok(contents) => contents,
        Err(err) => {
            log::warn!(
                "quick action Pyrefly discovery skipped: failed to read {}: {err}",
                pyproject_path.display()
            );
            return Vec::new();
        }
    };

    let pyproject = match toml::from_str::<toml::Value>(&contents) {
        Ok(pyproject) => pyproject,
        Err(err) => {
            log::warn!(
                "quick action Pyrefly discovery skipped: failed to parse {}: {err}",
                pyproject_path.display()
            );
            return Vec::new();
        }
    };

    if !pyproject
        .get("tool")
        .and_then(|tool| tool.get("pyrefly"))
        .is_some_and(toml::Value::is_table)
    {
        log::debug!(
            "quick action Pyrefly discovery skipped: no [tool.pyrefly] table in {}",
            pyproject_path.display()
        );
        return Vec::new();
    }

    log::debug!(
        "quick action Pyrefly discovery found config in {}",
        pyproject_path.display()
    );
    vec![RunItem {
        id: "pyrefly:check".to_string(),
        label: "Check (Pyrefly)".to_string(),
        icon_name: PYREFLY_ICON_NAME.to_string(),
        command: RunCommand::ShellCommand {
            command: "pyrefly check".to_string(),
        },
    }]
}
