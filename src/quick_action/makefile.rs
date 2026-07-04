use super::{MAKEFILE_ICON_NAME, RunCommand, RunItem};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node, Parser};

pub(super) fn makefile_path(repo_path: &Path) -> Option<PathBuf> {
    ["GNUmakefile", "makefile", "Makefile"]
        .into_iter()
        .map(|name| repo_path.join(name))
        .find(|path| path.is_file())
}

pub(super) fn discover_makefile_targets(repo_path: &Path) -> Vec<RunItem> {
    makefile_path(repo_path)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|contents| parse_makefile_targets(&contents))
        .unwrap_or_default()
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
