use std::collections::HashMap;

use craic_language::{CompletionSet, SyntaxHighlighter, language_id_from_path};

use crate::{DiffRow, DiffRowKind, DiffSide};

#[derive(Clone, Copy, Debug)]
pub struct DiffSyntaxSpan {
    pub side: DiffSide,
    pub line_number: usize,
    pub start: usize,
    pub end: usize,
    pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct TextSyntaxSpan {
    pub start: usize,
    pub end: usize,
    pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextDiagnosticKind {
    Error,
    Warning,
    Spelling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextDiagnosticSpan {
    pub start: usize,
    pub end: usize,
    pub kind: TextDiagnosticKind,
}

pub struct TextSyntaxAnalysis {
    pub syntax: Vec<TextSyntaxSpan>,
    pub diagnostics: Vec<TextDiagnosticSpan>,
    pub fold_ranges: Vec<(usize, usize)>,
    pub completions: Option<CompletionSet>,
}

pub fn build_text_syntax(path: &str, source: &str) -> Vec<TextSyntaxSpan> {
    analyze_text_syntax(path, source, None).syntax
}

pub fn analyze_text_syntax(
    path: &str,
    source: &str,
    completion_cursor: Option<usize>,
) -> TextSyntaxAnalysis {
    let mut highlighter = SyntaxHighlighter::new_id(language_id_from_path(path));
    highlighter.set_source(source);
    let mut highlights = highlighter.highlight_current();
    highlights.sort_by_key(|range| (range.start, range.end));
    let syntax = highlights
        .into_iter()
        .filter_map(|range| {
            (range.start < range.end && range.end <= source.len()).then(|| {
                let color = range.style.color();
                TextSyntaxSpan {
                    start: range.start,
                    end: range.end,
                    color: [color.0 as f32, color.1 as f32, color.2 as f32],
                }
            })
        })
        .collect();
    let fold_ranges = highlighter.fold_ranges_current();
    let diagnostics = highlighter
        .syntax_issues_current()
        .into_iter()
        .filter_map(|issue| {
            (issue.start < issue.end && issue.end <= source.len()).then_some(TextDiagnosticSpan {
                start: issue.start,
                end: issue.end,
                kind: TextDiagnosticKind::Error,
            })
        })
        .collect();
    let completions = completion_cursor.and_then(|cursor| highlighter.completions_current(cursor));
    TextSyntaxAnalysis {
        syntax,
        diagnostics,
        fold_ranges,
        completions,
    }
}

pub fn build_diff_syntax(path: &str, rows: &[DiffRow]) -> Vec<DiffSyntaxSpan> {
    let language = language_id_from_path(path);
    let mut spans = Vec::new();
    for side in [DiffSide::Left, DiffSide::Right] {
        let mut source = String::new();
        let mut row_sources = HashMap::new();
        for row in rows {
            if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
                continue;
            }
            let (number, text) = match side {
                DiffSide::Left => (row.left_number, row.left_text.as_deref()),
                DiffSide::Right => (row.right_number, row.right_text.as_deref()),
            };
            let (Some(number), Some(text)) = (number, text) else {
                continue;
            };
            let start = source.len();
            source.push_str(text);
            let end = source.len();
            row_sources.insert(number, (start, end));
            source.push('\n');
        }

        let mut highlighter = SyntaxHighlighter::new_id(language);
        highlighter.set_source(&source);
        let mut highlights = highlighter.highlight_current();
        highlights.sort_by_key(|range| (range.start, range.end));
        for (line_number, (source_start, source_end)) in row_sources {
            let first = highlights.partition_point(|range| range.end <= source_start);
            for range in &highlights[first..] {
                if range.start >= source_end {
                    break;
                }
                let start = range.start.max(source_start);
                let end = range.end.min(source_end);
                if start >= end {
                    continue;
                }
                let color = range.style.color();
                spans.push(DiffSyntaxSpan {
                    side,
                    line_number,
                    start: start - source_start,
                    end: end - source_start,
                    color: [color.0 as f32, color.1 as f32, color.2 as f32],
                });
            }
        }
    }
    spans.sort_by_key(|span| (span.line_number, span.side, span.start, span.end));
    spans
}
