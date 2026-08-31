const FOLD_KEEP_CONTEXT: usize = 3;
const FOLD_MIN_HIDDEN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRowKind {
    Context,
    Deleted,
    Added,
    Fold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffSearchMatch {
    pub side: DiffSide,
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiffTextPoint {
    pub side: DiffSide,
    pub row: usize,
    pub byte: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffTextSelection {
    pub anchor: DiffTextPoint,
    pub focus: DiffTextPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub left_number: Option<usize>,
    pub right_number: Option<usize>,
    pub left_text: Option<String>,
    pub right_text: Option<String>,
    pub left_kind: DiffRowKind,
    pub right_kind: DiffRowKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffDocument {
    pub rows: Vec<DiffRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffFoldRange {
    pub start: usize,
    pub end: usize,
    pub expanded: bool,
}

impl DiffDocument {
    pub fn folded_rows(&self) -> Vec<DiffRow> {
        display_diff_rows(&self.rows, &build_initial_diff_folds(&self.rows, &[]))
    }
}

pub fn build_initial_diff_folds(
    rows: &[DiffRow],
    previous: &[DiffFoldRange],
) -> Vec<DiffFoldRange> {
    let mut folds = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if !is_context_row(&rows[index]) {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < rows.len() && is_context_row(&rows[index]) {
            index += 1;
        }
        let run_end = index;
        let keep_before = if run_start > 0 { FOLD_KEEP_CONTEXT } else { 0 };
        let keep_after = if run_end < rows.len() {
            FOLD_KEEP_CONTEXT
        } else {
            0
        };
        let start = (run_start + keep_before).min(run_end);
        let end = run_end.saturating_sub(keep_after).max(start);
        if end.saturating_sub(start) >= FOLD_MIN_HIDDEN {
            folds.push(
                previous
                    .iter()
                    .find(|fold| fold.start == start && fold.end == end)
                    .copied()
                    .unwrap_or(DiffFoldRange {
                        start,
                        end,
                        expanded: false,
                    }),
            );
        }
    }
    folds
}

pub fn display_diff_rows(rows: &[DiffRow], folds: &[DiffFoldRange]) -> Vec<DiffRow> {
    if folds.is_empty() {
        return rows.to_vec();
    }
    let mut output = Vec::new();
    let mut source_index = 0;
    for (fold_index, fold) in folds.iter().copied().enumerate() {
        while source_index < fold.start {
            if let Some(row) = rows.get(source_index) {
                output.push(row.clone());
            }
            source_index += 1;
        }
        let hidden = fold.end.saturating_sub(fold.start);
        let label = match (fold.expanded, hidden) {
            (true, 1) => "- 1 shown line".to_string(),
            (true, count) => format!("- {count} shown lines"),
            (false, 1) => "+ 1 hidden line".to_string(),
            (false, count) => format!("+ {count} hidden lines"),
        };
        output.push(DiffRow {
            left_number: Some(fold_index),
            right_number: Some(fold_index),
            left_text: Some(label.clone()),
            right_text: Some(label),
            left_kind: DiffRowKind::Fold,
            right_kind: DiffRowKind::Fold,
        });
        if fold.expanded {
            while source_index < fold.end {
                if let Some(row) = rows.get(source_index) {
                    output.push(row.clone());
                }
                source_index += 1;
            }
        } else {
            source_index = fold.end;
        }
    }
    output.extend(rows[source_index.min(rows.len())..].iter().cloned());
    output
}

pub fn normalize_diff_folds(folds: &mut Vec<DiffFoldRange>, row_count: usize) -> bool {
    let mut normalized: Vec<DiffFoldRange> = Vec::with_capacity(folds.len());
    folds.sort_by_key(|fold| (fold.start, fold.end));
    for mut fold in folds.iter().copied() {
        fold.start = fold.start.min(row_count);
        fold.end = fold.end.min(row_count);
        if fold.start >= fold.end {
            continue;
        }
        if let Some(previous) = normalized.last() {
            if fold.start < previous.end {
                fold.start = previous.end;
            }
            if fold.start >= fold.end {
                continue;
            }
        }
        normalized.push(fold);
    }
    if *folds == normalized {
        false
    } else {
        *folds = normalized;
        true
    }
}

pub fn find_diff_search_matches(rows: &[DiffRow], query: &str) -> Vec<DiffSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
            continue;
        }
        for (side, text) in [
            (DiffSide::Left, row.left_text.as_deref()),
            (DiffSide::Right, row.right_text.as_deref()),
        ] {
            let Some(text) = text else { continue };
            push_text_search_matches(&mut matches, side, row_index, text, query);
        }
    }
    matches
}

pub fn diff_text_for_side(row: &DiffRow, side: DiffSide) -> Option<&str> {
    match side {
        DiffSide::Left => row.left_text.as_deref(),
        DiffSide::Right => row.right_text.as_deref(),
    }
}

pub fn selected_diff_text(rows: &[DiffRow], selection: DiffTextSelection) -> Option<String> {
    if selection.anchor.side != selection.focus.side {
        return None;
    }
    let (start, end) = if selection.anchor <= selection.focus {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    };
    let mut output = String::new();
    for row_index in start.row..=end.row {
        let Some(row) = rows.get(row_index) else {
            continue;
        };
        if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
            continue;
        }
        let Some(text) = diff_text_for_side(row, start.side) else {
            continue;
        };
        let text_start = if row_index == start.row {
            start.byte.min(text.len())
        } else {
            0
        };
        let text_end = if row_index == end.row {
            end.byte.min(text.len())
        } else {
            text.len()
        }
        .max(text_start);
        if !text.is_char_boundary(text_start) || !text.is_char_boundary(text_end) {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&text[text_start..text_end]);
    }
    Some(output)
}

pub fn select_all_diff_text(rows: &[DiffRow], side: DiffSide) -> Option<DiffTextSelection> {
    let first = rows.iter().enumerate().find_map(|(row, diff_row)| {
        (diff_row.left_kind != DiffRowKind::Fold
            && diff_row.right_kind != DiffRowKind::Fold
            && diff_text_for_side(diff_row, side).is_some())
        .then_some(row)
    })?;
    let (last, end) = rows.iter().enumerate().rev().find_map(|(row, diff_row)| {
        if diff_row.left_kind == DiffRowKind::Fold || diff_row.right_kind == DiffRowKind::Fold {
            return None;
        }
        diff_text_for_side(diff_row, side).map(|text| (row, text.len()))
    })?;
    Some(DiffTextSelection {
        anchor: DiffTextPoint {
            side,
            row: first,
            byte: 0,
        },
        focus: DiffTextPoint {
            side,
            row: last,
            byte: end,
        },
    })
}

fn push_text_search_matches(
    matches: &mut Vec<DiffSearchMatch>,
    side: DiffSide,
    row: usize,
    text: &str,
    query: &str,
) {
    let (haystack, needle) = if query.is_ascii() {
        (text.to_ascii_lowercase(), query.to_ascii_lowercase())
    } else {
        (text.to_string(), query.to_string())
    };
    let mut cursor = 0;
    while cursor <= haystack.len() {
        let Some(relative) = haystack[cursor..].find(&needle) else {
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            matches.push(DiffSearchMatch {
                side,
                row,
                start,
                end,
            });
            cursor = end.max(start.saturating_add(1));
        } else {
            cursor = start.saturating_add(1).min(text.len());
            while cursor < text.len() && !text.is_char_boundary(cursor) {
                cursor += 1;
            }
        }
    }
}

fn is_context_row(row: &DiffRow) -> bool {
    row.left_kind == DiffRowKind::Context && row.right_kind == DiffRowKind::Context
}
