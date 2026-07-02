use crate::system::capabilities::files::{FileAccess, FileKind};
use crate::system::path::WorkspaceRef;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use typos::tokens::{Identifier, Tokenizer, Word};
use typos::{Dictionary, Status};

const MAX_SPELLCHECK_BYTES: usize = 512 * 1024;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpellcheckIssue {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) word: String,
    pub(crate) corrections: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SpellcheckAllowlist {
    words: HashSet<String>,
}

pub(crate) fn check_document(
    language: &str,
    path: Option<&str>,
    text: &str,
    allowlist: &SpellcheckAllowlist,
) -> Vec<SpellcheckIssue> {
    if text.len() > MAX_SPELLCHECK_BYTES {
        log::debug!(
            "spellcheck document skipped reason=too-large path={} bytes={}",
            path.unwrap_or_default(),
            text.len()
        );
        return Vec::new();
    }

    let checker = Spellchecker::new(allowlist);
    let mut issues = Vec::new();
    for (start, end) in spellcheck_spans(language, path, text) {
        checker.check_slice(text, start, end, &mut issues);
    }
    log::debug!(
        "spellcheck document complete path={} language={} issues={}",
        path.unwrap_or_default(),
        language,
        issues.len()
    );
    issues
}

pub(crate) fn check_filename(name: &str, allowlist: &SpellcheckAllowlist) -> Vec<SpellcheckIssue> {
    let checker = Spellchecker::new(allowlist);
    let mut issues = Vec::new();
    checker.check_slice(name, 0, name.len(), &mut issues);
    if !issues.is_empty() {
        log::debug!(
            "spellcheck filename warning name={} issues={}",
            name,
            issues.len()
        );
    }
    issues
}

pub(crate) fn load_manifest_allowlist(
    workspace: &WorkspaceRef,
    file_access: Arc<dyn FileAccess>,
) -> SpellcheckAllowlist {
    let mut allowlist = SpellcheckAllowlist::default();
    let root = file_access.root();
    for manifest in ["Cargo.toml", "pyproject.toml", "package.json"] {
        let path = root.join_child(manifest);
        let Ok(info) = file_access.info(&path) else {
            continue;
        };
        if info.kind != FileKind::File || info.len_or_zero() > MAX_MANIFEST_BYTES {
            continue;
        }
        let Ok(text) = file_access.read_text(&path, Some(MAX_MANIFEST_BYTES)) else {
            continue;
        };
        match manifest {
            "Cargo.toml" | "pyproject.toml" => collect_toml_manifest_words(&text, &mut allowlist),
            "package.json" => collect_package_json_words(&text, &mut allowlist),
            _ => {}
        }
    }
    log::info!(
        "spellcheck manifest allowlist loaded workspace={} words={}",
        workspace.display_name,
        allowlist.words.len()
    );
    allowlist
}

impl SpellcheckAllowlist {
    pub(crate) fn insert_name(&mut self, value: &str) {
        for part in normalized_name_parts(value) {
            self.words.insert(part);
        }
    }

    fn contains(&self, token: &str) -> bool {
        self.words.contains(&normalize_word(token))
    }
}

struct Spellchecker<'a> {
    tokenizer: Tokenizer,
    dictionary: TyposDictionary<'a>,
}

impl<'a> Spellchecker<'a> {
    fn new(allowlist: &'a SpellcheckAllowlist) -> Self {
        Self {
            tokenizer: Tokenizer::new(),
            dictionary: TyposDictionary { allowlist },
        }
    }

    fn check_slice(
        &self,
        source: &str,
        span_start: usize,
        span_end: usize,
        issues: &mut Vec<SpellcheckIssue>,
    ) {
        if span_start >= span_end || span_end > source.len() {
            return;
        }
        for typo in typos::check_str(
            &source[span_start..span_end],
            &self.tokenizer,
            &self.dictionary,
        ) {
            let start = span_start + typo.byte_offset;
            let end = start + typo.typo.len();
            if start < end
                && end <= source.len()
                && source.is_char_boundary(start)
                && source.is_char_boundary(end)
            {
                issues.push(SpellcheckIssue {
                    start,
                    end,
                    word: typo.typo.into_owned(),
                    corrections: corrections_for_status(&typo.corrections),
                });
            }
        }
    }
}

fn corrections_for_status(status: &Status<'_>) -> Vec<String> {
    match status {
        Status::Corrections(corrections) => corrections
            .iter()
            .take(6)
            .map(|correction| correction.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

struct TyposDictionary<'a> {
    allowlist: &'a SpellcheckAllowlist,
}

impl Dictionary for TyposDictionary<'_> {
    fn correct_ident<'s>(&'s self, ident: Identifier<'_>) -> Option<Status<'s>> {
        status_for_token(ident.token(), self.allowlist)
    }

    fn correct_word<'s>(&'s self, word: Word<'_>) -> Option<Status<'s>> {
        status_for_token(word.token(), self.allowlist)
    }
}

fn status_for_token<'s>(token: &str, allowlist: &'s SpellcheckAllowlist) -> Option<Status<'s>> {
    if should_ignore_token(token) || allowlist.contains(token) {
        return Some(Status::Valid);
    }
    let token = unicase::UniCase::new(token);
    typos_dict::WORD.find(&token).map(|corrections| {
        if corrections.is_empty() {
            Status::Invalid
        } else {
            Status::Corrections(
                corrections
                    .iter()
                    .map(|correction| Cow::Borrowed(*correction))
                    .collect(),
            )
        }
    })
}

fn spellcheck_spans(language: &str, path: Option<&str>, text: &str) -> Vec<(usize, usize)> {
    let language = language.to_ascii_lowercase();
    if matches!(
        language.as_str(),
        "md" | "markdown" | "txt" | "text" | "rst" | "adoc"
    ) {
        return markdown_spans(text);
    }
    if matches!(language.as_str(), "toml" | "json" | "yaml" | "yml") {
        return quoted_value_spans(text);
    }
    if path.is_some_and(|path| path.ends_with(".md") || path.ends_with(".txt")) {
        return markdown_spans(text);
    }
    log::debug!("spellcheck using full document spans for code language={language}");
    full_document_spans(text)
}

fn full_document_spans(text: &str) -> Vec<(usize, usize)> {
    (!text.is_empty())
        .then_some((0, text.len()))
        .into_iter()
        .collect()
}

fn code_comment_and_string_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            let start = index + 2;
            let end = next_newline(bytes, start);
            spans.push((start, end));
            index = end;
        } else if bytes[index] == b'#' {
            let start = index + 1;
            let end = next_newline(bytes, start);
            spans.push((start, end));
            index = end;
        } else if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let start = index + 2;
            let end = find_bytes(bytes, start, b"*/").unwrap_or(bytes.len());
            spans.push((start, end));
            index = end.saturating_add(2);
        } else if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() {
                if bytes[end] == b'\\' {
                    end = end.saturating_add(2);
                } else if bytes[end] == quote {
                    break;
                } else {
                    end += 1;
                }
            }
            spans.push((start, end));
            index = end.saturating_add(1);
        } else {
            index += 1;
        }
    }
    spans
}

fn markdown_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0usize;
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
        } else if !fenced {
            spans.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    if offset < text.len() && !fenced {
        spans.push((offset, text.len()));
    }
    spans
}

fn quoted_value_spans(text: &str) -> Vec<(usize, usize)> {
    code_comment_and_string_spans(text)
}

fn collect_toml_manifest_words(text: &str, allowlist: &mut SpellcheckAllowlist) {
    let Ok(value) = text.parse::<toml::Value>() else {
        return;
    };
    collect_toml_key(&value, &["package", "name"], allowlist);
    collect_toml_key(&value, &["project", "name"], allowlist);
    collect_toml_table_keys(&value, &["dependencies"], allowlist);
    collect_toml_table_keys(&value, &["dev-dependencies"], allowlist);
    collect_toml_table_keys(&value, &["build-dependencies"], allowlist);
    collect_toml_table_keys(&value, &["workspace", "dependencies"], allowlist);
    collect_toml_table_keys(&value, &["tool", "poetry", "dependencies"], allowlist);
    collect_toml_table_keys(&value, &["tool", "poetry", "dev-dependencies"], allowlist);
    collect_toml_array_strings(&value, &["project", "dependencies"], allowlist);
    collect_toml_optional_dependencies(&value, allowlist);
}

fn collect_package_json_words(text: &str, allowlist: &mut SpellcheckAllowlist) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if let Some(name) = value.get("name").and_then(|value| value.as_str()) {
        allowlist.insert_name(name);
    }
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
        "bundledDependencies",
    ] {
        collect_json_dependency_names(value.get(key), allowlist);
    }
}

fn collect_toml_key(value: &toml::Value, path: &[&str], allowlist: &mut SpellcheckAllowlist) {
    if let Some(name) = toml_path(value, path).and_then(|value| value.as_str()) {
        allowlist.insert_name(name);
    }
}

fn collect_toml_table_keys(
    value: &toml::Value,
    path: &[&str],
    allowlist: &mut SpellcheckAllowlist,
) {
    let Some(table) = toml_path(value, path).and_then(|value| value.as_table()) else {
        return;
    };
    for (key, value) in table {
        allowlist.insert_name(key);
        if let Some(package) = value
            .as_table()
            .and_then(|table| table.get("package"))
            .and_then(|value| value.as_str())
        {
            allowlist.insert_name(package);
        }
    }
}

fn collect_toml_array_strings(
    value: &toml::Value,
    path: &[&str],
    allowlist: &mut SpellcheckAllowlist,
) {
    let Some(items) = toml_path(value, path).and_then(|value| value.as_array()) else {
        return;
    };
    for item in items {
        if let Some(name) = item.as_str().and_then(dependency_name_from_specifier) {
            allowlist.insert_name(name);
        }
    }
}

fn collect_toml_optional_dependencies(value: &toml::Value, allowlist: &mut SpellcheckAllowlist) {
    let Some(table) =
        toml_path(value, &["project", "optional-dependencies"]).and_then(|value| value.as_table())
    else {
        return;
    };
    for (group, items) in table {
        allowlist.insert_name(group);
        if let Some(items) = items.as_array() {
            for item in items {
                if let Some(name) = item.as_str().and_then(dependency_name_from_specifier) {
                    allowlist.insert_name(name);
                }
            }
        }
    }
}

fn collect_json_dependency_names(
    value: Option<&serde_json::Value>,
    allowlist: &mut SpellcheckAllowlist,
) {
    match value {
        Some(serde_json::Value::Object(map)) => {
            for key in map.keys() {
                allowlist.insert_name(key);
            }
        }
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(name) = item.as_str() {
                    allowlist.insert_name(name);
                }
            }
        }
        _ => {}
    }
}

fn toml_path<'a>(value: &'a toml::Value, path: &[&str]) -> Option<&'a toml::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn dependency_name_from_specifier(specifier: &str) -> Option<&str> {
    specifier
        .split(|ch: char| ch.is_ascii_whitespace() || matches!(ch, '<' | '>' | '=' | '[' | ';'))
        .find(|part| !part.is_empty())
}

fn normalized_name_parts(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let normalized = normalize_word(value);
    if !normalized.is_empty() {
        parts.push(normalized);
    }
    for part in value.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let normalized = normalize_word(part);
        if !normalized.is_empty() {
            parts.push(normalized);
        }
    }
    parts
}

fn normalize_word(value: &str) -> String {
    value
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
        .to_ascii_lowercase()
}

fn should_ignore_token(token: &str) -> bool {
    token.len() < 3
        || token
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || !ch.is_ascii_alphabetic())
        || token.chars().any(|ch| ch.is_ascii_digit())
}

fn next_newline(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|offset| start + offset)
        .unwrap_or(bytes.len())
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}
