use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use super::suggest;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree,
};

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;
const MAX_QUERY_CACHE_ENTRIES: usize = 16;
const RAINBOW_CSV_STYLES: [Style; 10] = [
    Style {
        foreground: "#e06c75",
    },
    Style {
        foreground: "#d19a66",
    },
    Style {
        foreground: "#e5c07b",
    },
    Style {
        foreground: "#98c379",
    },
    Style {
        foreground: "#56b6c2",
    },
    Style {
        foreground: "#61afef",
    },
    Style {
        foreground: "#c678dd",
    },
    Style {
        foreground: "#d16d9e",
    },
    Style {
        foreground: "#7ec16e",
    },
    Style {
        foreground: "#6f9fd8",
    },
];

#[derive(Clone, Copy)]
pub struct Style {
    foreground: &'static str,
}

#[derive(Clone)]
pub struct HighlightRange {
    pub start: usize,
    pub end: usize,
    pub style: Style,
    priority: u8,
}

#[derive(Clone)]
pub struct SyntaxIssue {
    pub start: usize,
    pub end: usize,
}

thread_local! {
    static QUERY_CACHE: RefCell<HashMap<String, Query>> = RefCell::new(HashMap::new());
}

pub struct SyntaxHighlighter {
    language_name: String,
    language: Option<Language>,
    parser: Parser,
    tree: Option<Tree>,
    source: String,
}

impl SyntaxHighlighter {
    pub fn new(language_name: &str) -> Self {
        let mut highlighter = Self {
            language_name: String::new(),
            language: None,
            parser: Parser::new(),
            tree: None,
            source: String::new(),
        };
        highlighter.set_language(language_name);
        highlighter
    }

    pub fn set_language(&mut self, language_name: &str) {
        let normalized = normalize_language_name(language_name);
        if self.language_name == normalized {
            return;
        }
        self.language_name = normalized.clone();
        self.language = language_for(&normalized);
        self.tree = None;
        if let Some(language) = self.language.as_ref() {
            let _ = self.parser.set_language(language);
        }
    }

    pub fn set_source(&mut self, source: &str) {
        self.apply_edit(0, self.source.len(), source);
    }

    pub fn apply_edit(&mut self, start: usize, old_end: usize, replacement: &str) {
        let start = start.min(self.source.len());
        let old_end = old_end.min(self.source.len()).max(start);
        let Some(language) = self.language.as_ref() else {
            self.source.replace_range(start..old_end, replacement);
            self.tree = None;
            return;
        };
        if self.source.len() > MAX_HIGHLIGHT_BYTES
            || self.source.lines().count() > MAX_HIGHLIGHT_LINES
        {
            self.source.replace_range(start..old_end, replacement);
            self.tree = None;
            return;
        }

        let old_start_position = point_for(&self.source, start);
        let old_end_position = point_for(&self.source, old_end);
        self.source.replace_range(start..old_end, replacement);
        let new_end = start + replacement.len();
        let new_end_position = point_for(&self.source, new_end);

        if let Some(tree) = self.tree.as_mut() {
            tree.edit(&InputEdit {
                start_byte: start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: old_start_position,
                old_end_position,
                new_end_position,
            });
        }
        let _ = self.parser.set_language(language);
        self.tree = self.parser.parse(&self.source, self.tree.as_ref());
    }

    pub fn highlight_current(&self) -> Vec<HighlightRange> {
        if self.language_name == "csv" {
            return rainbow_csv_ranges(&self.source);
        }
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        highlight_ranges_for_tree(
            &self.language_name,
            self.language.as_ref(),
            tree,
            &self.source,
        )
    }

    pub fn fold_ranges_current(&self) -> Vec<(usize, usize)> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };

        let mut ranges = Vec::new();
        collect_fold_ranges(&self.language_name, tree.root_node(), &mut ranges);
        ranges.sort_unstable();
        ranges.dedup();
        ranges
    }

    pub fn has_error_current(&self) -> bool {
        self.tree
            .as_ref()
            .is_some_and(|tree| tree.root_node().has_error())
    }

    pub fn syntax_issues_current(&self) -> Vec<SyntaxIssue> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let root = tree.root_node();
        if !root.has_error() {
            return Vec::new();
        }

        let mut issues = Vec::new();
        collect_syntax_issues(root, &self.source, &mut issues);
        issues.sort_by_key(|issue| (issue.start, issue.end));
        issues.dedup_by(|left, right| left.start == right.start && left.end == right.end);
        issues
    }

    pub fn completions_current(&self, cursor: usize) -> Option<suggest::CompletionSet> {
        let tree = self.tree.as_ref()?;
        suggest::completions(&self.language_name, tree, &self.source, cursor)
    }
}

fn rainbow_csv_ranges(source: &str) -> Vec<HighlightRange> {
    if source.len() > MAX_HIGHLIGHT_BYTES || source.lines().count() > MAX_HIGHLIGHT_LINES {
        return Vec::new();
    }

    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut field_start = 0;
    let mut column = 0;
    let mut cursor = 0;
    let mut quoted = false;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'"' if quoted && bytes.get(cursor + 1) == Some(&b'"') => {
                cursor += 2;
                continue;
            }
            b'"' if quoted => quoted = false,
            b'"' if cursor == field_start => quoted = true,
            b',' if !quoted => {
                push_csv_field(&mut ranges, field_start, cursor, column);
                field_start = cursor + 1;
                column += 1;
            }
            b'\n' if !quoted => {
                push_csv_field(&mut ranges, field_start, cursor, column);
                field_start = cursor + 1;
                column = 0;
            }
            b'\r' if !quoted => {
                push_csv_field(&mut ranges, field_start, cursor, column);
                cursor += usize::from(bytes.get(cursor + 1) == Some(&b'\n'));
                field_start = cursor + 1;
                column = 0;
            }
            _ => {}
        }
        cursor += 1;
    }

    push_csv_field(&mut ranges, field_start, source.len(), column);
    ranges
}

fn push_csv_field(ranges: &mut Vec<HighlightRange>, start: usize, end: usize, column: usize) {
    if start >= end {
        return;
    }
    ranges.push(HighlightRange {
        start,
        end,
        style: RAINBOW_CSV_STYLES[column % RAINBOW_CSV_STYLES.len()],
        priority: 50,
    });
}

impl Style {
    pub fn color(self) -> (f64, f64, f64) {
        rgb(self.foreground)
    }
}

fn collect_fold_ranges(language_name: &str, node: Node<'_>, ranges: &mut Vec<(usize, usize)>) {
    if node.is_named() && is_foldable_node(language_name, node.kind()) {
        let start_line = node.start_position().row;
        let end_line = node.end_position().row;
        if end_line > start_line {
            ranges.push((start_line, end_line));
        }
    }

    for index in 0..node.child_count() {
        if let Some(child) = node.child(index as u32) {
            collect_fold_ranges(language_name, child, ranges);
        }
    }
}

fn collect_syntax_issues(node: Node<'_>, source: &str, issues: &mut Vec<SyntaxIssue>) {
    if node.is_error() || node.is_missing() {
        if let Some(issue) = syntax_issue_range(node.start_byte(), node.end_byte(), source) {
            issues.push(issue);
        }
        return;
    }

    if !node.has_error() {
        return;
    }

    for index in 0..node.child_count() {
        if let Some(child) = node.child(index as u32) {
            collect_syntax_issues(child, source, issues);
        }
    }
}

fn syntax_issue_range(start: usize, end: usize, source: &str) -> Option<SyntaxIssue> {
    let start = start.min(source.len());
    let end = end.min(source.len());
    if start < end && source.is_char_boundary(start) && source.is_char_boundary(end) {
        return Some(SyntaxIssue { start, end });
    }

    if start < source.len() {
        let end = next_char_boundary(source, start.saturating_add(1));
        return (start < end).then_some(SyntaxIssue { start, end });
    }

    let start = previous_char_boundary(source, start.saturating_sub(1));
    (start < source.len()).then_some(SyntaxIssue {
        start,
        end: source.len(),
    })
}

fn is_foldable_node(language_name: &str, kind: &str) -> bool {
    if matches!(
        kind,
        "block"
            | "statement_block"
            | "compound_statement"
            | "declaration_list"
            | "initializer_list"
            | "argument_list"
            | "parameters"
            | "formal_parameters"
            | "parenthesized_expression"
            | "array"
            | "object"
            | "pair"
            | "element"
            | "stylesheet"
            | "rule_set"
            | "block_mapping"
            | "block_sequence"
            | "table"
            | "array_table"
            | "table_array_element"
            | "inline_table"
    ) {
        return true;
    }

    match normalize_language_name(language_name).as_str() {
        "rust" | "rs" => matches!(
            kind,
            "function_item"
                | "impl_item"
                | "trait_item"
                | "struct_item"
                | "enum_item"
                | "mod_item"
                | "macro_definition"
                | "match_block"
        ),
        "python" | "py" | "pyw" => matches!(
            kind,
            "function_definition"
                | "class_definition"
                | "decorated_definition"
                | "for_statement"
                | "while_statement"
                | "if_statement"
                | "with_statement"
                | "try_statement"
                | "match_statement"
        ),
        "javascript" | "js" | "mjs" | "cjs" | "jsx" | "typescript" | "ts" | "tsx" => matches!(
            kind,
            "function_declaration"
                | "function"
                | "arrow_function"
                | "method_definition"
                | "class_declaration"
                | "class"
                | "interface_declaration"
                | "enum_declaration"
                | "type_alias_declaration"
        ),
        "go" | "golang" => matches!(
            kind,
            "function_declaration"
                | "method_declaration"
                | "type_declaration"
                | "struct_type"
                | "interface_type"
                | "literal_value"
        ),
        "java" => matches!(
            kind,
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "method_declaration"
                | "constructor_declaration"
        ),
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => matches!(
            kind,
            "function_definition"
                | "struct_specifier"
                | "union_specifier"
                | "enum_specifier"
                | "namespace_definition"
                | "class_specifier"
                | "template_declaration"
        ),
        "ruby" | "rb" => matches!(
            kind,
            "method"
                | "singleton_method"
                | "class"
                | "module"
                | "do_block"
                | "begin"
                | "if"
                | "case"
        ),
        "bash" | "sh" | "shell" | "zsh" => matches!(
            kind,
            "function_definition"
                | "if_statement"
                | "for_statement"
                | "while_statement"
                | "case_statement"
                | "subshell"
                | "compound_statement"
        ),
        "html" | "htm" | "xml" | "xhtml" => {
            matches!(kind, "element" | "script_element" | "style_element")
        }
        "css" | "scss" => matches!(kind, "rule_set" | "media_statement" | "supports_statement"),
        "toml" => matches!(
            kind,
            "table" | "array_table" | "table_array_element" | "inline_table" | "array"
        ),
        "yaml" | "yml" => matches!(kind, "block_mapping" | "block_sequence"),
        "json" | "jsonc" | "json5" => matches!(kind, "object" | "array"),
        "markdown" | "md" | "mdown" | "mkd" => matches!(
            kind,
            "section" | "fenced_code_block" | "list" | "block_quote"
        ),
        _ => false,
    }
}

pub fn language_hint_from_path(path: &str) -> String {
    let path = path.to_ascii_lowercase();
    let file_name = path.rsplit('/').next().unwrap_or(path.as_str());
    match file_name {
        name if name == ".env" || name.starts_with(".env.") => {
            return "bash".to_string();
        }
        ".bash_profile" | ".bashrc" | ".profile" | ".zprofile" | ".zshrc" => {
            return "bash".to_string();
        }
        "cargo.lock" | "uv.lock" => return "toml".to_string(),
        "gemfile" | "rakefile" => return "ruby".to_string(),
        "gnumakefile" | "makefile" => return "make".to_string(),
        "caddyfile" => return "caddyfile".to_string(),
        "package.json" | "package-lock.json" | "tsconfig.json" => return "json".to_string(),
        "dockerfile" => return "bash".to_string(),
        _ if file_name.ends_with(".caddyfile") => return "caddyfile".to_string(),
        _ if file_name.ends_with(".desktop.in") => return "ini".to_string(),
        _ if file_name.ends_with("ignore") => return "ignore".to_string(),
        _ => {}
    }
    if file_name.ends_with(".svg") || file_name.ends_with(".svgz") {
        return "xml".to_string();
    }
    path.rsplit('.')
        .next()
        .filter(|extension| *extension != path)
        .unwrap_or_default()
        .to_string()
}

fn trim_query_cache(cache: &mut HashMap<String, Query>) {
    if cache.len() < MAX_QUERY_CACHE_ENTRIES {
        return;
    }
    let Some(first_key) = cache.keys().next().cloned() else {
        return;
    };
    cache.remove(&first_key);
}

fn point_for(source: &str, byte: usize) -> Point {
    let mut row = 0;
    let mut column = 0;
    for ch in source[..byte.min(source.len())].chars() {
        if ch == '\n' {
            row += 1;
            column = 0;
        } else {
            column += ch.len_utf8();
        }
    }
    Point { row, column }
}

fn previous_char_boundary(source: &str, mut offset: usize) -> usize {
    offset = offset.min(source.len());
    while offset > 0 && !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn next_char_boundary(source: &str, mut offset: usize) -> usize {
    offset = offset.min(source.len());
    while offset < source.len() && !source.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn highlight_ranges_for_tree(
    language_name: &str,
    language: Option<&Language>,
    tree: &Tree,
    source: &str,
) -> Vec<HighlightRange> {
    let mut ranges = Vec::new();
    collect_highlight_ranges(
        language_name,
        language,
        tree.root_node(),
        source,
        &mut ranges,
    );
    normalize_ranges(ranges, source.len())
}

fn collect_highlight_ranges(
    language_name: &str,
    language: Option<&Language>,
    root: Node<'_>,
    source: &str,
    ranges: &mut Vec<HighlightRange>,
) {
    if let Some(language) = language {
        if collect_query_ranges(language_name, language, root, source, ranges) {
            return;
        }
    }
    collect_ranges(root, source, ranges);
}

fn collect_query_ranges(
    language_name: &str,
    language: &Language,
    root: Node<'_>,
    source: &str,
    ranges: &mut Vec<HighlightRange>,
) -> bool {
    let Some(query_source) = highlight_query_for(language_name) else {
        return false;
    };

    QUERY_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(language_name) {
            let Ok(query) = Query::new(language, query_source.as_ref()) else {
                return false;
            };
            trim_query_cache(&mut cache);
            cache.insert(language_name.to_string(), query);
        }

        let Some(query) = cache.get(language_name) else {
            return false;
        };
        let capture_names = query.capture_names();
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(2048);
        let mut captures = cursor.captures(query, root, source.as_bytes());
        while let Some((query_match, capture_index)) = captures.next() {
            let Some(capture) = query_match.captures.get(*capture_index) else {
                continue;
            };
            let Some(capture_name) = capture_names.get(capture.index as usize) else {
                continue;
            };
            let Some((style, priority)) = style_for_capture(capture_name) else {
                continue;
            };
            ranges.push(HighlightRange {
                start: capture.node.start_byte(),
                end: capture.node.end_byte(),
                style,
                priority,
            });
        }
        true
    })
}

fn normalize_ranges(mut ranges: Vec<HighlightRange>, source_len: usize) -> Vec<HighlightRange> {
    ranges.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut normalized = Vec::with_capacity(ranges.len());
    let mut cursor = 0;
    for range in ranges {
        if range.start < cursor || range.end <= range.start || range.end > source_len {
            continue;
        }
        cursor = range.end;
        normalized.push(range);
    }
    normalized
}

pub fn apply_edit_to_ranges(
    ranges: &mut Vec<HighlightRange>,
    start: usize,
    old_end: usize,
    replacement_len: usize,
    source_len: usize,
) {
    if ranges.is_empty() {
        return;
    }

    let old_end = old_end.max(start);
    let new_end = start.saturating_add(replacement_len);
    let replacement_style =
        (replacement_len > 0).then(|| style_for_replacement(ranges, start, old_end));
    let replacement_style = replacement_style.flatten();
    let mut adjusted = Vec::with_capacity(ranges.len() + usize::from(replacement_style.is_some()));

    for range in ranges.iter().cloned() {
        if range.end <= start {
            push_valid_range(&mut adjusted, range);
            continue;
        }

        if range.start >= old_end {
            push_valid_range(
                &mut adjusted,
                HighlightRange {
                    start: shift_after_edit(range.start, start, old_end, replacement_len),
                    end: shift_after_edit(range.end, start, old_end, replacement_len),
                    ..range
                },
            );
            continue;
        }

        if range.start < start {
            push_valid_range(
                &mut adjusted,
                HighlightRange {
                    end: start,
                    ..range.clone()
                },
            );
        }

        if range.end > old_end {
            push_valid_range(
                &mut adjusted,
                HighlightRange {
                    start: new_end,
                    end: shift_after_edit(range.end, start, old_end, replacement_len),
                    ..range
                },
            );
        }
    }

    if let Some((style, priority)) = replacement_style {
        push_valid_range(
            &mut adjusted,
            HighlightRange {
                start,
                end: new_end,
                style,
                priority,
            },
        );
    }

    adjusted.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| left.end.cmp(&right.end))
    });
    *ranges = normalize_projected_ranges(adjusted, source_len);
}

fn normalize_projected_ranges(
    ranges: Vec<HighlightRange>,
    source_len: usize,
) -> Vec<HighlightRange> {
    let mut normalized = Vec::with_capacity(ranges.len());
    let mut cursor = 0usize;

    for mut range in ranges {
        range.start = range.start.min(source_len);
        range.end = range.end.min(source_len);
        if range.end <= range.start {
            continue;
        }
        if range.start < cursor {
            range.start = cursor;
        }
        if range.end <= range.start {
            continue;
        }
        cursor = range.end;
        normalized.push(range);
    }

    normalized
}

fn style_for_replacement(
    ranges: &[HighlightRange],
    start: usize,
    old_end: usize,
) -> Option<(Style, u8)> {
    let range = if start == old_end {
        ranges
            .iter()
            .find(|range| range.start < start && start < range.end)
    } else {
        ranges
            .iter()
            .find(|range| range.start <= start && start < range.end)
            .or_else(|| {
                ranges
                    .iter()
                    .find(|range| range.start < old_end && range.end > start)
            })
    }?;

    Some((range.style, range.priority))
}

fn shift_after_edit(offset: usize, start: usize, old_end: usize, replacement_len: usize) -> usize {
    let removed_len = old_end.saturating_sub(start);
    if replacement_len >= removed_len {
        offset.saturating_add(replacement_len - removed_len)
    } else {
        offset.saturating_sub(removed_len - replacement_len)
    }
}

fn push_valid_range(ranges: &mut Vec<HighlightRange>, range: HighlightRange) {
    if range.start < range.end {
        ranges.push(range);
    }
}

fn collect_ranges(node: Node<'_>, source: &str, ranges: &mut Vec<HighlightRange>) {
    if let Some(style) = style_for_node(node, source) {
        ranges.push(HighlightRange {
            start: node.start_byte(),
            end: node.end_byte(),
            style,
            priority: 10,
        });

        if is_whole_node_style(node.kind()) {
            return;
        }
    }

    for index in 0..node.child_count() {
        if let Some(child) = node.child(index as u32) {
            collect_ranges(child, source, ranges);
        }
    }
}

fn is_function_identifier(node: Node<'_>) -> bool {
    let kind = node.kind();
    if kind != "identifier" && kind != "field_identifier" && kind != "property_identifier" {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    let p_kind = parent.kind();
    if p_kind == "function_declarator"
        || p_kind == "function_item"
        || p_kind == "function_definition"
        || p_kind == "function_declaration"
        || p_kind == "method_declaration"
        || p_kind == "method_definition"
        || p_kind == "call_expression"
    {
        return true;
    }
    if p_kind == "member_expression" || p_kind == "field_expression" {
        if let Some(grandparent) = parent.parent() {
            let gp_kind = grandparent.kind();
            if gp_kind == "call_expression" {
                return true;
            }
        }
    }
    false
}

fn is_type_declaration(node: Node<'_>) -> bool {
    let kind = node.kind();
    if kind != "identifier" && kind != "type_identifier" {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    let p_kind = parent.kind();
    p_kind == "class_declaration"
        || p_kind == "class_definition"
        || p_kind == "struct_item"
        || p_kind == "enum_item"
        || p_kind == "trait_item"
        || p_kind == "type_alias_declaration"
}

fn is_camel_case(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    s.chars().any(|c| c.is_ascii_lowercase())
}

fn is_all_caps(s: &str) -> bool {
    s.len() >= 2
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

fn style_for_node(node: Node<'_>, source: &str) -> Option<Style> {
    let kind = node.kind();

    // Comments
    if kind.contains("comment")
        || kind == "comment_element"
        || kind == "line_comment"
        || kind == "block_comment"
    {
        return Some(Style {
            foreground: "#5c6370",
        });
    }

    // Strings & Characters
    if kind.contains("string")
        || kind.contains("character")
        || kind == "string_literal"
        || kind == "raw_string_literal"
        || kind == "attribute_value"
    {
        return Some(Style {
            foreground: "#98c379",
        });
    }

    // Numbers
    if kind.contains("number")
        || kind.contains("integer")
        || kind.contains("float")
        || kind == "number_literal"
    {
        return Some(Style {
            foreground: "#d19a66",
        });
    }

    // Node text
    let text = if node.end_byte() <= source.len() && node.start_byte() <= node.end_byte() {
        &source[node.start_byte()..node.end_byte()]
    } else {
        ""
    };

    // Types, Classes, Structs
    if kind.contains("type")
        || kind.contains("class")
        || kind.contains("struct")
        || kind == "primitive_type"
        || kind == "type_identifier"
        || kind == "class_name"
        || kind == "class_identifier"
        || kind == "struct_identifier"
        || is_type_declaration(node)
    {
        return Some(Style {
            foreground: "#e5c07b",
        });
    }

    // Functions
    if kind == "function"
        || kind == "function_identifier"
        || kind == "method_identifier"
        || kind == "macro_definition"
    {
        return Some(Style {
            foreground: "#61afef",
        });
    }
    if is_function_identifier(node) {
        return Some(Style {
            foreground: "#61afef",
        });
    }

    // Properties, Fields, and Keys (JSON/TOML/YAML/HTML)
    if kind == "property_identifier"
        || kind == "field_identifier"
        || kind == "attribute_name"
        || kind == "property_name"
        || kind == "key"
    {
        return Some(Style {
            foreground: "#e06c75",
        });
    }

    // JSON/YAML keys (first child of key-value pair)
    if let Some(parent) = node.parent() {
        let p_kind = parent.kind();
        if p_kind == "pair" || p_kind == "block_mapping_pair" || p_kind == "flow_pair" {
            if parent.child(0) == Some(node) {
                return Some(Style {
                    foreground: "#e06c75",
                });
            }
        }
    }

    // CamelCase class names and ALL_CAPS constants
    if kind == "identifier" || kind == "field_identifier" {
        if is_all_caps(text) {
            return Some(Style {
                foreground: "#d19a66",
            });
        }
        if is_camel_case(text) {
            return Some(Style {
                foreground: "#e5c07b",
            });
        }
    }

    // Keywords and Tags
    if kind.contains("keyword")
        || kind == "true"
        || kind == "false"
        || kind == "null"
        || kind == "nil"
        || kind == "undefined"
        || kind == "tag_name"
        || kind == "doctype"
    {
        return Some(Style {
            foreground: "#c678dd",
        });
    }

    // Macros & Preprocessor
    if kind.contains("macro")
        || kind.contains("preproc")
        || kind == "macro_invocation"
        || kind == "preproc_directive"
    {
        return Some(Style {
            foreground: "#56b6c2",
        });
    }

    // Anonymous alphabetic nodes (fallback for keywords like "fn", "let")
    if !node.is_named() {
        if kind.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
            return Some(Style {
                foreground: "#c678dd",
            });
        }
    }

    None
}

fn is_whole_node_style(kind: &str) -> bool {
    kind.contains("comment")
        || kind.contains("string")
        || kind.contains("character")
        || kind.contains("number")
        || kind.contains("integer")
        || kind.contains("float")
}

fn rgb(hex: &str) -> (f64, f64, f64) {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return (0.86, 0.86, 0.86);
    }
    let Ok(value) = u32::from_str_radix(hex, 16) else {
        return (0.86, 0.86, 0.86);
    };
    (
        ((value >> 16) & 0xff) as f64 / 255.0,
        ((value >> 8) & 0xff) as f64 / 255.0,
        (value & 0xff) as f64 / 255.0,
    )
}

fn highlight_query_for(name: &str) -> Option<Cow<'static, str>> {
    match normalize_language_name(name).as_str() {
        "bash" | "sh" | "shell" | "zsh" | "ignore" => {
            Some(Cow::Borrowed(tree_sitter_bash::HIGHLIGHT_QUERY))
        }
        "c" | "h" => Some(Cow::Borrowed(tree_sitter_c::HIGHLIGHT_QUERY)),
        "cpp" | "c++" | "cc" | "cxx" | "hh" | "hpp" | "hxx" => Some(Cow::Owned(
            [
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY,
            ]
            .join("\n"),
        )),
        "caddy" | "caddyfile" => None,
        "css" | "scss" => Some(Cow::Borrowed(tree_sitter_css::HIGHLIGHTS_QUERY)),
        "go" | "golang" => Some(Cow::Borrowed(tree_sitter_go::HIGHLIGHTS_QUERY)),
        "html" | "htm" => Some(Cow::Borrowed(tree_sitter_html::HIGHLIGHTS_QUERY)),
        "xml" | "xhtml" => Some(Cow::Borrowed(tree_sitter_xml::XML_HIGHLIGHT_QUERY)),
        "java" => Some(Cow::Borrowed(tree_sitter_java::HIGHLIGHTS_QUERY)),
        "javascript" | "js" | "mjs" | "cjs" => Some(Cow::Owned(
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ]
            .join("\n"),
        )),
        "jsx" => Some(Cow::Owned(
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ]
            .join("\n"),
        )),
        "json" | "jsonc" | "json5" => Some(Cow::Borrowed(tree_sitter_json::HIGHLIGHTS_QUERY)),
        "hlsl" => None,
        "slang" => None,
        "cuda" | "cu" | "cuh" => Some(Cow::Borrowed(tree_sitter_cuda::HIGHLIGHTS_QUERY)),
        "kotlin" | "kt" | "kts" | "ktm" => None,
        "ini" => Some(Cow::Borrowed(tree_sitter_ini::HIGHLIGHTS_QUERY)),
        "powershell" | "ps1" | "psm1" | "psd1" => None,
        "make" | "mk" | "makefile" => Some(Cow::Borrowed(tree_sitter_make::HIGHLIGHTS_QUERY)),
        "markdown" | "md" | "mdown" | "mkd" => {
            Some(Cow::Borrowed(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK))
        }
        "python" | "py" | "pyw" => Some(Cow::Borrowed(tree_sitter_python::HIGHLIGHTS_QUERY)),
        "ruby" | "rb" => Some(Cow::Borrowed(tree_sitter_ruby::HIGHLIGHTS_QUERY)),
        "rust" | "rs" => Some(Cow::Borrowed(tree_sitter_rust::HIGHLIGHTS_QUERY)),
        "toml" => Some(Cow::Borrowed(tree_sitter_toml_ng::HIGHLIGHTS_QUERY)),
        "typescript" | "ts" => Some(Cow::Owned(
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ]
            .join("\n"),
        )),
        "tsx" => Some(Cow::Owned(
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ]
            .join("\n"),
        )),
        "yaml" | "yml" => Some(Cow::Borrowed(tree_sitter_yaml::HIGHLIGHTS_QUERY)),
        _ => None,
    }
}

fn style_for_capture(name: &str) -> Option<(Style, u8)> {
    if name.starts_with('_') || name.starts_with("injection.") || name.starts_with("local.") {
        return None;
    }

    let style = if name == "none" || name == "ignore" {
        return None;
    } else if name.starts_with("comment") || name == "doc" {
        (
            Style {
                foreground: "#5c6370",
            },
            70,
        )
    } else if name == "escape" || name == "string.escape" {
        (
            Style {
                foreground: "#56b6c2",
            },
            90,
        )
    } else if name.starts_with("string")
        || name == "embedded"
        || name == "text.literal"
        || name == "string.special.regex"
    {
        (
            Style {
                foreground: "#98c379",
            },
            60,
        )
    } else if name == "number"
        || name == "boolean"
        || name.starts_with("constant")
        || name == "constructor"
    {
        (
            Style {
                foreground: "#d19a66",
            },
            60,
        )
    } else if name.starts_with("keyword")
        || matches!(
            name,
            "conditional" | "exception" | "include" | "repeat" | "clean"
        )
    {
        (
            Style {
                foreground: "#c678dd",
            },
            70,
        )
    } else if name.starts_with("function")
        || name == "reference.call"
        || name == "definition.function"
        || name == "definition.method"
    {
        (
            Style {
                foreground: "#61afef",
            },
            80,
        )
    } else if name.starts_with("type")
        || name.starts_with("definition.")
        || name == "reference.type"
        || name == "reference.class"
        || name == "reference.implementation"
        || name == "text.title"
    {
        (
            Style {
                foreground: "#e5c07b",
            },
            75,
        )
    } else if name == "property"
        || name == "attribute"
        || name == "label"
        || name.starts_with("tag")
        || name == "text.reference"
        || name == "string.special.key"
    {
        (
            Style {
                foreground: "#e06c75",
            },
            65,
        )
    } else if name == "text.uri" {
        (
            Style {
                foreground: "#56b6c2",
            },
            65,
        )
    } else if name.starts_with("operator") || name.starts_with("punctuation") || name == "delimiter"
    {
        (
            Style {
                foreground: "#abb2bf",
            },
            20,
        )
    } else if name == "text.strong" {
        (
            Style {
                foreground: "#e5c07b",
            },
            65,
        )
    } else if name == "text.emphasis" {
        (
            Style {
                foreground: "#c678dd",
            },
            65,
        )
    } else if name.starts_with("text.") {
        (
            Style {
                foreground: "#abb2bf",
            },
            40,
        )
    } else {
        return None;
    };

    Some(style)
}

fn language_for(name: &str) -> Option<Language> {
    match normalize_language_name(name).as_str() {
        "bash" | "sh" | "shell" | "zsh" | "ignore" => Some(tree_sitter_bash::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "caddy" | "caddyfile" => Some(tree_sitter_caddy::LANGUAGE.into()),
        "cpp" | "c++" | "cc" | "cxx" | "hh" | "hpp" | "hxx" => {
            Some(tree_sitter_cpp::LANGUAGE.into())
        }
        "css" | "scss" => Some(tree_sitter_css::LANGUAGE.into()),
        "go" | "golang" => Some(tree_sitter_go::LANGUAGE.into()),
        "html" | "htm" => Some(tree_sitter_html::LANGUAGE.into()),
        "xml" | "xhtml" => Some(tree_sitter_xml::LANGUAGE_XML.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "javascript" | "js" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "json" | "jsonc" | "json5" => Some(tree_sitter_json::LANGUAGE.into()),
        "make" | "mk" | "makefile" => Some(tree_sitter_make::LANGUAGE.into()),
        "powershell" | "ps1" | "psm1" | "psd1" => Some(tree_sitter_powershell::LANGUAGE.into()),
        "hlsl" => Some(tree_sitter_hlsl::LANGUAGE_HLSL.into()),
        "slang" => Some(tree_sitter_slang::LANGUAGE_SLANG.into()),
        "cuda" | "cu" | "cuh" => Some(tree_sitter_cuda::LANGUAGE.into()),
        "kotlin" | "kt" | "kts" | "ktm" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        "markdown" | "md" | "mdown" | "mkd" => Some(tree_sitter_md::LANGUAGE.into()),
        "python" | "py" | "pyw" => Some(tree_sitter_python::LANGUAGE.into()),
        "ruby" | "rb" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "rust" | "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ini" => Some(tree_sitter_ini::LANGUAGE.into()),
        "toml" => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        "typescript" | "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "yaml" | "yml" => Some(tree_sitter_yaml::LANGUAGE.into()),
        _ => None,
    }
}

fn normalize_language_name(name: &str) -> String {
    name.trim()
        .trim_start_matches('.')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}
