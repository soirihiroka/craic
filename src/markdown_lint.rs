use rumdl_lib::config::{Config, MarkdownFlavor};
use rumdl_lib::rules::{all_rules, filter_rules};
use std::path::PathBuf;

const MAX_MARKDOWN_LINT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkdownLintIssue {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn check_document(path: Option<&str>, text: &str) -> Vec<MarkdownLintIssue> {
    if text.len() > MAX_MARKDOWN_LINT_BYTES {
        log::debug!(
            "markdown lint skipped reason=too-large path={} bytes={}",
            path.unwrap_or_default(),
            text.len()
        );
        return Vec::new();
    }

    let config = Config::default();
    let all = all_rules(&config);
    let rules = filter_rules(&all, &config.global);
    let source_file = path.map(PathBuf::from);
    let flavor = source_file
        .as_deref()
        .map(|path| config.get_flavor_for_file(path))
        .unwrap_or(MarkdownFlavor::Standard);

    let warnings = match rumdl_lib::lint(text, &rules, false, flavor, source_file, Some(&config)) {
        Ok(warnings) => warnings,
        Err(err) => {
            log::warn!(
                "markdown lint failed path={} error={err}",
                path.unwrap_or_default()
            );
            return Vec::new();
        }
    };

    let mut issues = warnings
        .into_iter()
        .filter_map(|warning| {
            warning_range(
                text,
                warning.line,
                warning.column,
                warning.end_line,
                warning.end_column,
            )
        })
        .collect::<Vec<_>>();
    issues.sort_by_key(|issue| (issue.start, issue.end));
    issues.dedup();
    log::debug!(
        "markdown lint complete path={} issues={}",
        path.unwrap_or_default(),
        issues.len()
    );
    issues
}

fn warning_range(
    text: &str,
    line: usize,
    column: usize,
    end_line: usize,
    end_column: usize,
) -> Option<MarkdownLintIssue> {
    let start = byte_offset_for_line_column(text, line, column)?;
    let mut end = byte_offset_for_line_column(text, end_line, end_column).unwrap_or(start);
    if end <= start {
        end = line_end_offset(text, start);
    }
    if end <= start {
        end = next_char_boundary(text, start);
    }
    (start < end && end <= text.len()).then_some(MarkdownLintIssue { start, end })
}

fn byte_offset_for_line_column(text: &str, line: usize, column: usize) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }

    let mut current_line = 1usize;
    let mut line_start = 0usize;
    for (offset, ch) in text.char_indices() {
        if current_line == line {
            return byte_offset_for_column(&text[line_start..], line_start, column);
        }
        if ch == '\n' {
            current_line += 1;
            line_start = offset + ch.len_utf8();
        }
    }

    (current_line == line)
        .then(|| byte_offset_for_column(&text[line_start..], line_start, column))?
}

fn byte_offset_for_column(line_text: &str, line_start: usize, column: usize) -> Option<usize> {
    let target = column.saturating_sub(1);
    if target == 0 {
        return Some(line_start);
    }
    line_text
        .char_indices()
        .nth(target)
        .map(|(offset, _)| line_start + offset)
        .or_else(|| Some(line_start + line_text.trim_end_matches(['\r', '\n']).len()))
}

fn line_end_offset(text: &str, start: usize) -> usize {
    text[start..]
        .find('\n')
        .map(|offset| start + offset)
        .unwrap_or(text.len())
}

fn next_char_boundary(text: &str, start: usize) -> usize {
    text[start..]
        .chars()
        .next()
        .map(|ch| start + ch.len_utf8())
        .unwrap_or(start)
}
