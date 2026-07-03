use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tree_sitter::{Language, Node, Parser};

const MAKEFILE_ICON_NAME: &str = "text-makefile-symbolic";
const BUN_ICON_NAME: &str = "devicon-bun-symbolic";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunItem {
    pub id: String,
    pub label: String,
    pub icon_name: &'static str,
    pub command: RunCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCommand {
    MakeTarget { target: String },
    BunScript { script: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTargetsSignature {
    makefile: FileSignature,
    package_json: FileSignature,
    bun_lock: FileSignature,
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
    }
}

pub fn discover(repo_path: &Path) -> Vec<RunItem> {
    let mut targets = makefile_path(repo_path)
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|contents| parse_makefile_targets(&contents))
        .unwrap_or_default();

    targets.extend(discover_bun_scripts(repo_path));
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
            icon_name: MAKEFILE_ICON_NAME,
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
                icon_name: BUN_ICON_NAME,
                command: RunCommand::BunScript {
                    script: script.to_string(),
                },
            })
        })
        .collect()
}
