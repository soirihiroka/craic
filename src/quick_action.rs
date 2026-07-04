use crate::workspace_config::QuickActionAdditionalConfig;
use serde_json::Value;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tree_sitter::{Language, Node, Parser};
use walkdir::WalkDir;

const MAKEFILE_ICON_NAME: &str = "text-makefile-symbolic";
const BUN_ICON_NAME: &str = "devicon-bun-symbolic";
const GRADLE_ICON_NAME: &str = "devicon-groovy-symbolic";
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
    makefile: FileSignature,
    package_json: FileSignature,
    bun_lock: FileSignature,
    gradle: FileSignature,
    android_manifest: FileSignature,
    local_config: FileSignature,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSignature {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

pub fn targets_signature(repo_path: &Path) -> RunTargetsSignature {
    RunTargetsSignature {
        makefile: file_signature(makefile_path(repo_path)),
        package_json: file_signature(package_json_path(repo_path)),
        bun_lock: file_signature(bun_lock_path(repo_path)),
        gradle: file_signature(gradle_project_path(repo_path)),
        android_manifest: file_signature(android_manifest_path(repo_path)),
        local_config: file_signature(local_config_path(repo_path)),
    }
}

fn local_config_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join(".craic").join("local").join("config.toml");
    path.is_file().then_some(path)
}

pub fn discover(repo_path: &Path) -> Vec<RunItem> {
    let mut targets = makefile_path(repo_path)
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|contents| parse_makefile_targets(&contents))
        .unwrap_or_default();

    targets.extend(discover_bun_scripts(repo_path));
    targets.extend(discover_gradle_targets(repo_path));

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

fn makefile_path(repo_path: &Path) -> Option<PathBuf> {
    ["GNUmakefile", "makefile", "Makefile"]
        .into_iter()
        .map(|name| repo_path.join(name))
        .find(|path| path.is_file())
}

fn package_json_path(repo_path: &Path) -> Option<PathBuf> {
    let path = repo_path.join("package.json");
    path.is_file().then_some(path)
}

fn bun_lock_path(repo_path: &Path) -> Option<PathBuf> {
    ["bun.lock", "bun.lockb"]
        .into_iter()
        .map(|name| repo_path.join(name))
        .find(|path| path.is_file())
}

fn gradle_project_path(repo_path: &Path) -> Option<PathBuf> {
    const GRADLE_FILES: [&str; 4] = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ];

    for name in GRADLE_FILES {
        let path = repo_path.join(name);
        if path.is_file() {
            return Some(path);
        }
    }

    if gradlew_path(repo_path).is_some() {
        return Some(repo_path.join("gradlew"));
    }

    if let Ok(entries) = fs::read_dir(repo_path) {
        for entry in entries.filter_map(Result::ok) {
            let module_root = entry.path();
            if !module_root.is_dir() {
                continue;
            }

            for name in GRADLE_FILES {
                let path = module_root.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    None
}

fn gradlew_path(repo_path: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        let path = repo_path.join("gradlew.bat");
        if path.is_file() {
            return Some(path);
        }
    }

    let path = repo_path.join("gradlew");
    if path.is_file() {
        return Some(path);
    }
    None
}

fn gradle_command(repo_path: &Path) -> String {
    if gradlew_path(repo_path).is_some() {
        if cfg!(windows) {
            "gradlew.bat".to_string()
        } else {
            "./gradlew".to_string()
        }
    } else {
        "gradle".to_string()
    }
}

fn android_manifest_path(repo_path: &Path) -> Option<PathBuf> {
    const MANIFEST_PATHS: [&str; 2] = [
        "app/src/main/AndroidManifest.xml",
        "src/main/AndroidManifest.xml",
    ];

    for path in MANIFEST_PATHS {
        let manifest_path = repo_path.join(path);
        if manifest_path.is_file() {
            return Some(manifest_path);
        }
    }

    let mut modules = fs::read_dir(repo_path).ok()?;

    for entry in modules.by_ref().filter_map(Result::ok) {
        let module_path = entry.path();
        if !module_path.is_dir() {
            continue;
        }

        let manifest_path = module_path.join("src/main/AndroidManifest.xml");
        if manifest_path.is_file() {
            return Some(manifest_path);
        }
    }

    for entry in WalkDir::new(repo_path)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "AndroidManifest.xml")
    {
        return Some(entry.path().to_path_buf());
    }
    None
}

fn discover_gradle_targets(repo_path: &Path) -> Vec<RunItem> {
    if gradle_project_path(repo_path).is_none() {
        return Vec::new();
    }

    let gradle_program = gradle_command(repo_path);
    let is_android = android_manifest_path(repo_path).is_some();
    let mut targets = vec![RunItem {
        id: "gradle:build".to_string(),
        label: "Build (Gradle)".to_string(),
        icon_name: GRADLE_ICON_NAME.to_string(),
        command: RunCommand::ShellCommand {
            command: format!("{gradle_program} build"),
        },
    }];

    if is_android {
        targets.push(RunItem {
            id: "gradle:assemble-debug".to_string(),
            label: "Build Debug APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} assembleDebug"),
            },
        });
        targets.push(RunItem {
            id: "gradle:assemble-release".to_string(),
            label: "Build Release APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} assembleRelease"),
            },
        });
        targets.push(RunItem {
            id: "gradle:install-debug".to_string(),
            label: "Install Debug APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} installDebug"),
            },
        });
        targets.push(RunItem {
            id: "gradle:install-release".to_string(),
            label: "Install Release APK (Gradle)".to_string(),
            icon_name: GRADLE_ICON_NAME.to_string(),
            command: RunCommand::ShellCommand {
                command: format!("{gradle_program} installRelease"),
            },
        });
    }

    targets
}

fn parse_makefile_targets(contents: &str) -> Vec<RunItem> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    let mut parser = Parser::new();
    let language: Language = tree_sitter_make::LANGUAGE.into();

    if parser.set_language(&language).is_err() {
        return targets;
    }

    let Some(tree) = parser.parse(contents, None) else {
        return targets;
    };

    collect_make_targets(
        tree.root_node(),
        contents.as_bytes(),
        &mut seen,
        &mut targets,
    );

    targets
}

fn collect_make_targets(
    node: Node<'_>,
    source: &[u8],
    seen: &mut HashSet<String>,
    targets: &mut Vec<RunItem>,
) {
    if node.kind() == "rule" {
        collect_rule_targets(node, source, seen, targets);
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_make_targets(child, source, seen, targets);
    }
}

fn collect_rule_targets(
    rule: Node<'_>,
    source: &[u8],
    seen: &mut HashSet<String>,
    targets: &mut Vec<RunItem>,
) {
    let mut cursor = rule.walk();
    for child in rule.named_children(&mut cursor) {
        if child.kind() == "targets" {
            collect_target_words(child, source, seen, targets);
            return;
        }
    }
}

fn collect_target_words(
    targets_node: Node<'_>,
    source: &[u8],
    seen: &mut HashSet<String>,
    targets: &mut Vec<RunItem>,
) {
    let mut cursor = targets_node.walk();
    for child in targets_node.named_children(&mut cursor) {
        if child.kind() != "word" {
            continue;
        }

        let Ok(target) = child.utf8_text(source) else {
            continue;
        };
        if !is_runnable_make_target(target) || !seen.insert(target.to_string()) {
            continue;
        }

        targets.push(RunItem {
            id: format!("make:{target}"),
            label: target.to_string(),
            icon_name: MAKEFILE_ICON_NAME.to_string(),
            command: RunCommand::MakeTarget {
                target: target.to_string(),
            },
        });
    }
}

fn is_runnable_make_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('.')
        && !target.contains('%')
        && !target.contains('/')
        && !target.contains('\\')
}

fn discover_bun_scripts(repo_path: &Path) -> Vec<RunItem> {
    let (Some(package_json_path), Some(_bun_lock_path)) =
        (package_json_path(repo_path), bun_lock_path(repo_path))
    else {
        return Vec::new();
    };

    let Ok(contents) = fs::read_to_string(&package_json_path) else {
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
