use crate::workspace_config::QuickActionAdditionalConfig;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod bun;
mod cargo;
mod gradle;
mod makefile;
mod pyrefly;

const MAKEFILE_ICON_NAME: &str = "text-makefile-symbolic";
const BUN_ICON_NAME: &str = "devicon-bun-symbolic";
const CARGO_ICON_NAME: &str = "text-rust-symbolic";
const GRADLE_ICON_NAME: &str = "devicon-gradle-symbolic";
const PYREFLY_ICON_NAME: &str = "text-x-python-symbolic";
const CUSTOM_ICON_NAME: &str = "utilities-terminal-symbolic";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunItem {
    pub id: String,
    pub label: String,
    pub icon_name: String,
    pub command: RunCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCommand {
    MakeTarget { target: String },
    BunScript { script: String },
    ShellCommand { command: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTargetsSignature {
    cargo: FileSignature,
    makefile: FileSignature,
    package_json: FileSignature,
    bun_lock: FileSignature,
    gradle: FileSignature,
    android_manifest: FileSignature,
    pyproject: FileSignature,
    local_config: FileSignature,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSignature {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

pub fn targets_signature(repo_path: &Path) -> RunTargetsSignature {
    let gradle = file_signature(gradle::gradle_project_path(repo_path));
    let android_manifest = if gradle.path.is_some() {
        file_signature(gradle::android_manifest_path(repo_path))
    } else {
        file_signature(None)
    };
    RunTargetsSignature {
        cargo: file_signature(cargo::cargo_manifest_path(repo_path)),
        makefile: file_signature(makefile::makefile_path(repo_path)),
        package_json: file_signature(bun::package_json_path(repo_path)),
        bun_lock: file_signature(bun::bun_lock_path(repo_path)),
        gradle,
        android_manifest,
        pyproject: file_signature(pyrefly::pyproject_path(repo_path)),
        local_config: file_signature(local_config_path(repo_path)),
    }
}

fn local_config_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join(".craic").join("local").join("config.toml");
    path.is_file().then_some(path)
}

pub fn discover(repo_path: &Path) -> Vec<RunItem> {
    let mut targets = cargo::discover_cargo_targets(repo_path);
    targets.extend(makefile::discover_makefile_targets(repo_path));
    targets.extend(bun::discover_bun_scripts(repo_path));
    targets.extend(gradle::discover_gradle_targets(repo_path));
    targets.extend(pyrefly::discover_pyrefly_targets(repo_path));
    targets
}

pub fn discover_additional_quick_actions(
    additional: &[QuickActionAdditionalConfig],
) -> Vec<RunItem> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();

    for action in additional {
        let Some(raw_command) = action.command.as_deref() else {
            log::warn!("quick action additional command skipped: missing command");
            continue;
        };

        let command = raw_command.trim();
        if command.is_empty() {
            log::warn!("quick action additional command skipped: command is empty");
            continue;
        }

        let label = action
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(command)
            .to_string();

        let icon_name = action
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|icon| !icon.is_empty())
            .unwrap_or(CUSTOM_ICON_NAME)
            .to_string();

        let mut hasher = DefaultHasher::new();
        command.hash(&mut hasher);
        let id = format!("custom:{label}:{}", hasher.finish());
        if !seen.insert(id.clone()) {
            continue;
        }

        targets.push(RunItem {
            id,
            label,
            icon_name,
            command: RunCommand::ShellCommand {
                command: command.to_string(),
            },
        });
    }

    targets
}

fn file_signature(path: Option<PathBuf>) -> FileSignature {
    let metadata = path.as_ref().and_then(|path| fs::metadata(path).ok());
    FileSignature {
        path,
        modified: metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok()),
        len: metadata.map(|metadata| metadata.len()),
    }
}
