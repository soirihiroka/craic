use std::collections::{HashMap, HashSet};
use tree_sitter::{Node, Tree};

use super::{CompletionItem, CompletionSet};

const BUILDER_FAMILY_KEY: &str = "family:builder";

struct CompletionContext {
    receiver: String,
    prefix: String,
    replacement_start: usize,
    replacement_end: usize,
}

pub(super) fn completions(tree: &Tree, source: &str, cursor: usize) -> Option<CompletionSet> {
    let cursor = previous_char_boundary(source, cursor);
    if syntax_blocks_completion(tree.root_node(), source, cursor) {
        return None;
    }

    let context = field_context_from_tree(tree.root_node(), source, cursor)
        .or_else(|| field_context_from_text(source, cursor))?;
    let scope = local_scope_for_cursor(tree.root_node(), cursor);
    let candidates = local_method_candidates(scope, source, cursor, &context.receiver);
    let items = candidates
        .into_iter()
        .filter(|label| label.starts_with(context.prefix.as_str()))
        .map(|label| CompletionItem {
            insert_text: label.clone(),
            label,
        })
        .collect::<Vec<_>>();

    (!items.is_empty()).then_some(CompletionSet {
        items,
        replacement_start: context.replacement_start,
        replacement_end: context.replacement_end,
    })
}

fn local_scope_for_cursor(root: Node<'_>, cursor: usize) -> Node<'_> {
    let probe = cursor.saturating_sub(1).min(root.end_byte());
    let Some(mut node) = root.descendant_for_byte_range(probe, probe) else {
        return root;
    };

    loop {
        if is_local_scope(node.kind()) {
            return node;
        }
        let Some(parent) = node.parent() else {
            return root;
        };
        node = parent;
    }
}

fn is_local_scope(kind: &str) -> bool {
    matches!(kind, "function_item" | "closure_expression")
}

fn syntax_blocks_completion(root: Node<'_>, source: &str, cursor: usize) -> bool {
    let probe = cursor.saturating_sub(1).min(source.len());
    let Some(mut node) = root.descendant_for_byte_range(probe, probe) else {
        return false;
    };

    loop {
        if matches!(
            node.kind(),
            "line_comment"
                | "block_comment"
                | "string_literal"
                | "raw_string_literal"
                | "char_literal"
        ) {
            return true;
        }
        let Some(parent) = node.parent() else {
            break;
        };
        node = parent;
    }
    false
}

fn field_context_from_tree(
    root: Node<'_>,
    source: &str,
    cursor: usize,
) -> Option<CompletionContext> {
    let probe = cursor.saturating_sub(1).min(source.len());
    let mut node = root.descendant_for_byte_range(probe, probe)?;

    loop {
        if node.kind() == "field_expression" {
            let value = node.child_by_field_name("value")?;
            let field = node.child_by_field_name("field")?;
            if cursor < field.start_byte() || cursor > field.end_byte() {
                return None;
            }
            let prefix = source.get(field.start_byte()..cursor)?.to_string();
            return Some(CompletionContext {
                receiver: value.utf8_text(source.as_bytes()).ok()?.to_string(),
                prefix,
                replacement_start: field.start_byte(),
                replacement_end: cursor,
            });
        }
        node = node.parent()?;
    }
}

fn field_context_from_text(source: &str, cursor: usize) -> Option<CompletionContext> {
    let cursor = previous_char_boundary(source, cursor);
    let prefix_start = identifier_start_before(source, cursor);
    let dot = previous_non_whitespace(source, prefix_start)?;
    if source.get(dot..)?.chars().next()? != '.' {
        return None;
    }
    let receiver_end = previous_non_whitespace(source, dot)?;
    let receiver_end = receiver_end + source.get(receiver_end..)?.chars().next()?.len_utf8();
    let receiver_start = expression_start_before(source, receiver_end)?;
    let receiver = trim_assignment_prefix(source.get(receiver_start..receiver_end)?)
        .trim()
        .to_string();

    (!receiver.is_empty()).then_some(CompletionContext {
        receiver,
        prefix: source.get(prefix_start..cursor)?.to_string(),
        replacement_start: prefix_start,
        replacement_end: cursor,
    })
}

fn local_method_candidates(
    scope: Node<'_>,
    source: &str,
    cursor: usize,
    context_receiver: &str,
) -> Vec<String> {
    let aliases = collect_receiver_aliases(scope, source, cursor);
    let context_keys = receiver_keys_from_text(context_receiver, &aliases)
        .into_iter()
        .collect::<HashSet<_>>();
    if context_keys.is_empty() {
        return Vec::new();
    }

    let mut observed = Vec::new();
    collect_observed_methods(
        scope,
        scope.start_byte(),
        source,
        cursor,
        &aliases,
        &mut observed,
    );
    observed.sort_by_key(|method| method.name_start);

    let mut seen = HashSet::new();
    let mut labels = Vec::new();
    for method in observed {
        if !method
            .receiver_keys
            .iter()
            .any(|key| context_keys.contains(key))
        {
            continue;
        }

        if seen.insert(method.name.clone()) {
            labels.push(method.name);
        }
    }

    labels
}

fn collect_receiver_aliases(
    scope: Node<'_>,
    source: &str,
    cursor: usize,
) -> HashMap<String, Vec<String>> {
    let mut aliases = HashMap::new();
    collect_receiver_aliases_in_scope(scope, scope.start_byte(), source, cursor, &mut aliases);
    aliases
}

fn collect_receiver_aliases_in_scope(
    node: Node<'_>,
    scope_start: usize,
    source: &str,
    cursor: usize,
    aliases: &mut HashMap<String, Vec<String>>,
) {
    if node.start_byte() >= cursor {
        return;
    }

    if node.start_byte() != scope_start && is_local_scope(node.kind()) {
        return;
    }

    if node.kind() == "let_declaration" && node.end_byte() <= cursor {
        if let (Some(pattern), Some(value)) = (
            node.child_by_field_name("pattern"),
            node.child_by_field_name("value"),
        ) {
            if let (Some(name), Ok(value_text)) = (
                simple_pattern_name(pattern, source),
                value.utf8_text(source.as_bytes()),
            ) {
                let keys = receiver_alias_keys(value_text, aliases);
                if !keys.is_empty() {
                    aliases.insert(name, keys);
                }
            }
        }
    }

    let mut cursor_walk = node.walk();
    for child in node.named_children(&mut cursor_walk) {
        collect_receiver_aliases_in_scope(child, scope_start, source, cursor, aliases);
    }
}

fn simple_pattern_name(pattern: Node<'_>, source: &str) -> Option<String> {
    let text = pattern.utf8_text(source.as_bytes()).ok()?.trim();
    let name = text.strip_prefix("mut ").unwrap_or(text).trim();
    is_identifier(name).then(|| name.to_string())
}

fn receiver_alias_keys(receiver: &str, aliases: &HashMap<String, Vec<String>>) -> Vec<String> {
    let compact = compact_expression(receiver);
    if compact.is_empty() {
        return Vec::new();
    }

    if let Some(alias_keys) = aliases.get(compact.as_str()) {
        return alias_keys.clone();
    }

    if chain_root(&compact) == compact && called_function_name(&compact) == Some("builder") {
        return vec![chain_receiver_key(&compact), BUILDER_FAMILY_KEY.to_string()];
    }

    Vec::new()
}

struct ObservedMethod {
    name: String,
    name_start: usize,
    receiver_keys: Vec<String>,
}

fn collect_observed_methods(
    node: Node<'_>,
    scope_start: usize,
    source: &str,
    cursor: usize,
    aliases: &HashMap<String, Vec<String>>,
    observed: &mut Vec<ObservedMethod>,
) {
    if node.start_byte() >= cursor {
        return;
    }

    if node.start_byte() != scope_start && is_local_scope(node.kind()) {
        return;
    }

    if node.kind() == "call_expression" && node.end_byte() <= cursor {
        if let Some((receiver, name)) = method_call_parts(node) {
            if let Ok(name_text) = name.utf8_text(source.as_bytes()) {
                let receiver_text = receiver.utf8_text(source.as_bytes()).unwrap_or_default();
                let receiver_keys = receiver_keys_from_text(receiver_text, aliases);
                if !receiver_keys.is_empty() && is_identifier(name_text) {
                    observed.push(ObservedMethod {
                        name: name_text.to_string(),
                        name_start: name.start_byte(),
                        receiver_keys,
                    });
                }
            }
        }
    }

    let mut cursor_walk = node.walk();
    for child in node.named_children(&mut cursor_walk) {
        collect_observed_methods(child, scope_start, source, cursor, aliases, observed);
    }
}

fn method_call_parts(node: Node<'_>) -> Option<(Node<'_>, Node<'_>)> {
    let function = node.child_by_field_name("function")?;
    field_expression_parts(function).or_else(|| {
        (function.kind() == "generic_function")
            .then(|| function.child_by_field_name("function"))
            .flatten()
            .and_then(field_expression_parts)
    })
}

fn field_expression_parts(node: Node<'_>) -> Option<(Node<'_>, Node<'_>)> {
    (node.kind() == "field_expression").then(|| {
        let receiver = node.child_by_field_name("value")?;
        let name = node.child_by_field_name("field")?;
        Some((receiver, name))
    })?
}

fn receiver_keys_from_text(receiver: &str, aliases: &HashMap<String, Vec<String>>) -> Vec<String> {
    let compact = compact_expression(receiver);
    if compact.is_empty() {
        return Vec::new();
    }

    let root = chain_root(&compact);
    let mut keys = Vec::new();
    push_unique(&mut keys, exact_receiver_key(&compact));

    if root != compact {
        push_unique(&mut keys, exact_receiver_key(root));
    }

    if let Some(alias_keys) = aliases.get(compact.as_str()) {
        for key in alias_keys {
            push_unique(&mut keys, key.clone());
        }
    }

    if let Some(alias_keys) = aliases.get(root) {
        for key in alias_keys {
            push_unique(&mut keys, key.clone());
        }
    }

    if is_builder_like_root(root, aliases) {
        push_unique(&mut keys, chain_receiver_key(root));
        push_unique(&mut keys, BUILDER_FAMILY_KEY.to_string());
    }

    keys
}

fn exact_receiver_key(receiver: &str) -> String {
    format!("exact:{receiver}")
}

fn chain_receiver_key(receiver: &str) -> String {
    format!("chain:{receiver}")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn is_builder_like_root(root: &str, aliases: &HashMap<String, Vec<String>>) -> bool {
    called_function_name(root) == Some("builder")
        || aliases
            .get(root)
            .is_some_and(|keys| keys.iter().any(|key| key == BUILDER_FAMILY_KEY))
}

fn compact_expression(expr: &str) -> String {
    let trimmed = expr.trim().trim_end_matches(';').trim();
    trimmed.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn chain_root(compact: &str) -> &str {
    let mut depth = 0usize;
    for (offset, ch) in compact.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => return &compact[..offset],
            _ => {}
        }
    }

    compact
}

fn called_function_name(compact: &str) -> Option<&str> {
    if !compact.ends_with(')') {
        return None;
    }

    let mut depth = 0usize;
    let mut open = None;
    for (offset, ch) in compact.char_indices().rev() {
        if ch == ')' {
            depth += 1;
        } else if ch == '(' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                open = Some(offset);
                break;
            }
        }
    }

    let open = open?;
    let before_args = compact.get(..open)?;
    let name_start = identifier_start_before(before_args, before_args.len());
    let name = before_args.get(name_start..)?;
    is_identifier(name).then_some(name)
}

fn identifier_start_before(source: &str, cursor: usize) -> usize {
    let mut start = cursor.min(source.len());
    while let Some((previous, ch)) = previous_char(source, start) {
        if !is_identifier_char(ch) {
            break;
        }
        start = previous;
    }
    start
}

fn expression_start_before(source: &str, receiver_end: usize) -> Option<usize> {
    let mut start = 0usize;
    for (offset, ch) in source.char_indices() {
        if offset > receiver_end {
            break;
        }
        if matches!(ch, ';' | '{' | '}') {
            start = offset + ch.len_utf8();
        }
    }
    Some(start)
}

fn trim_assignment_prefix(candidate: &str) -> &str {
    let mut depth = 0usize;
    let mut assignment = None;
    let bytes = candidate.as_bytes();

    for (offset, ch) in candidate.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '=' if depth == 0 => {
                let previous = offset
                    .checked_sub(1)
                    .and_then(|index| bytes.get(index).copied());
                let next = bytes.get(offset + 1).copied();
                if !matches!(previous, Some(b'=' | b'!' | b'<' | b'>'))
                    && !matches!(next, Some(b'=' | b'>'))
                {
                    assignment = Some(offset + ch.len_utf8());
                }
            }
            _ => {}
        }
    }

    assignment
        .and_then(|offset| candidate.get(offset..))
        .unwrap_or(candidate)
}

fn previous_non_whitespace(source: &str, cursor: usize) -> Option<usize> {
    let mut offset = cursor.min(source.len());
    while let Some((previous, ch)) = previous_char(source, offset) {
        if !ch.is_whitespace() {
            return Some(previous);
        }
        offset = previous;
    }
    None
}

fn previous_char(source: &str, cursor: usize) -> Option<(usize, char)> {
    source[..cursor.min(source.len())].char_indices().last()
}

fn previous_char_boundary(source: &str, mut offset: usize) -> usize {
    offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(is_identifier_char)
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    fn completions_at(marked_source: &str) -> Option<CompletionSet> {
        let cursor = marked_source.find("/*cursor*/").unwrap();
        let source = marked_source.replace("/*cursor*/", "");
        let tree = parse(&source);
        completions(&tree, &source, cursor)
    }

    fn labels_at(marked_source: &str) -> Vec<String> {
        completions_at(marked_source)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn offers_methods_from_previous_builder_chain() {
        let labels = labels_at(
            r#"
fn build_box() {
    let _row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    let _other = gtk::Label::builder()./*cursor*/;
}
"#,
        );

        assert_eq!(labels, ["orientation", "spacing", "build"]);
    }

    #[test]
    fn filters_builder_methods_by_prefix() {
        let labels = labels_at(
            r#"
fn build_box() {
    let _row = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

    let _other = gtk::Box::builder().sp/*cursor*/;
}
"#,
        );

        assert_eq!(labels, ["spacing"]);
    }

    #[test]
    fn offers_methods_observed_on_same_local_receiver() {
        let labels = labels_at(
            r#"
fn style_list(list: gtk::ListBox) {
    list.add_css_class("boxed-list");
    list./*cursor*/;
}
"#,
        );

        assert_eq!(labels, ["add_css_class"]);
    }

    #[test]
    fn does_not_offer_methods_from_another_function() {
        let completions = completions_at(
            r#"
fn style_list(list: gtk::ListBox) {
    list.add_css_class("boxed-list");
}

fn clear_list(list: gtk::ListBox) {
    list./*cursor*/;
}
"#,
        );

        assert!(completions.is_none());
    }
}
