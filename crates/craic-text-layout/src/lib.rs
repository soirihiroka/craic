use rayon::prelude::*;
use unicode_segmentation::UnicodeSegmentation;

const PARALLEL_WRAP_MIN_LINES: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FoldRange {
    pub start_line: usize,
    pub end_line: usize,
    pub expanded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisualLine {
    pub source_line: usize,
    pub start: usize,
    pub end: usize,
    pub wrap_index: usize,
    pub folded: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnSlice {
    pub start: usize,
    pub end: usize,
    pub start_column: f64,
}

#[derive(Clone, Copy)]
struct LineJob {
    source_line: usize,
    start: usize,
    end: usize,
    folded: Option<usize>,
}

pub fn build_visual_lines(
    text: &str,
    folds: &[FoldRange],
    wrap: bool,
    wrap_width: f64,
    minimum_width: f64,
    mut measure: impl FnMut(&str) -> f64,
) -> Vec<VisualLine> {
    build_visual_lines_with(text, folds, |lines, source_line, start, end| {
        push_wrapped_visual_lines(
            lines,
            source_line,
            start,
            end,
            text,
            wrap,
            wrap_width.max(minimum_width),
            &mut measure,
        );
    })
}

fn build_visual_lines_with(
    text: &str,
    folds: &[FoldRange],
    mut push_line: impl FnMut(&mut Vec<VisualLine>, usize, usize, usize),
) -> Vec<VisualLine> {
    let mut lines = Vec::new();
    let mut source_line = 0;
    let line_ranges = logical_line_ranges(text);

    while source_line < line_ranges.len() {
        let (line_start, line_end) = line_ranges[source_line];
        if let Some(fold_index) = collapsed_fold_starting_at(folds, source_line) {
            lines.push(VisualLine {
                source_line,
                start: line_start,
                end: line_end,
                wrap_index: 0,
                folded: Some(fold_index),
            });
            source_line = folds[fold_index]
                .end_line
                .saturating_add(1)
                .min(line_ranges.len());
            continue;
        }

        push_line(&mut lines, source_line, line_start, line_end);
        source_line += 1;
    }

    if lines.is_empty() {
        lines.push(VisualLine {
            source_line: 0,
            start: 0,
            end: 0,
            wrap_index: 0,
            folded: None,
        });
    }
    lines
}

pub fn build_visual_lines_monospace(
    text: &str,
    folds: &[FoldRange],
    wrap: bool,
    wrap_columns: f64,
) -> Vec<VisualLine> {
    let wrap_columns = wrap_columns.max(1.0);
    let jobs = line_jobs(text, folds);
    if jobs.len() < PARALLEL_WRAP_MIN_LINES {
        let mut lines = Vec::with_capacity(jobs.len());
        for job in jobs {
            push_monospace_job(&mut lines, job, text, wrap, wrap_columns);
        }
        return lines;
    }

    jobs.par_iter()
        .map(|job| {
            let mut lines = Vec::new();
            push_monospace_job(&mut lines, *job, text, wrap, wrap_columns);
            lines
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}

fn line_jobs(text: &str, folds: &[FoldRange]) -> Vec<LineJob> {
    let line_ranges = logical_line_ranges(text);
    let mut jobs = Vec::with_capacity(line_ranges.len());
    let mut source_line = 0;
    while source_line < line_ranges.len() {
        let (start, end) = line_ranges[source_line];
        if let Some(fold_index) = collapsed_fold_starting_at(folds, source_line) {
            jobs.push(LineJob {
                source_line,
                start,
                end,
                folded: Some(fold_index),
            });
            source_line = folds[fold_index]
                .end_line
                .saturating_add(1)
                .min(line_ranges.len());
        } else {
            jobs.push(LineJob {
                source_line,
                start,
                end,
                folded: None,
            });
            source_line += 1;
        }
    }
    jobs
}

fn push_monospace_job(
    lines: &mut Vec<VisualLine>,
    job: LineJob,
    text: &str,
    wrap: bool,
    wrap_columns: f64,
) {
    if job.folded.is_some() {
        lines.push(VisualLine {
            source_line: job.source_line,
            start: job.start,
            end: job.end,
            wrap_index: 0,
            folded: job.folded,
        });
        return;
    }
    push_monospace_visual_lines(
        lines,
        job.source_line,
        job.start,
        job.end,
        text,
        wrap,
        wrap_columns,
    );
}

pub fn visual_spans(lines: &[VisualLine], source_line_count: usize) -> Vec<Option<(usize, usize)>> {
    let mut spans = vec![None; source_line_count];
    for (index, line) in lines.iter().enumerate() {
        let Some(span) = spans.get_mut(line.source_line) else {
            continue;
        };
        match span {
            Some((first, count)) => *count = index.saturating_sub(*first) + 1,
            None => *span = Some((index, 1)),
        }
    }
    spans
}

pub fn visual_line_index_for_offset(lines: &[VisualLine], offset: usize) -> usize {
    if lines.is_empty() {
        return 0;
    }

    let index = lines.partition_point(|line| line.end < offset);
    if let Some(line) = lines.get(index)
        && offset >= line.start
        && offset <= line.end
    {
        return index;
    }
    if index > 0 {
        let previous = &lines[index - 1];
        let next_start = lines
            .get(index)
            .map(|line| line.start)
            .unwrap_or(usize::MAX);
        if previous.folded.is_some() && offset < next_start {
            return index - 1;
        }
    }
    index.min(lines.len() - 1)
}

pub fn max_line_columns(text: &str) -> f64 {
    logical_line_ranges(text)
        .into_iter()
        .map(|(start, end)| {
            if text[start..end].is_ascii() {
                text[start..end]
                    .bytes()
                    .map(|byte| if byte == b'\t' { 4.0 } else { 1.0 })
                    .sum()
            } else {
                text[start..end].graphemes(true).map(grapheme_columns).sum()
            }
        })
        .fold(0.0, f64::max)
}

pub fn column_slice(text: &str, first_column: f64, last_column: f64) -> ColumnSlice {
    let first_column = first_column.max(0.0);
    let last_column = last_column.max(first_column + 1.0);
    let mut start = 0;
    let mut end = text.len();
    let mut start_column = 0.0;
    let mut column = 0.0;
    let mut found_start = false;

    for (offset, grapheme) in text.grapheme_indices(true) {
        let width = grapheme_columns(grapheme);
        let next_column = column + width;
        if !found_start && next_column > first_column {
            start = offset;
            start_column = column;
            found_start = true;
        }
        if found_start && column >= last_column {
            end = offset;
            break;
        }
        column = next_column;
    }

    if !found_start {
        start = text.len();
        end = text.len();
        start_column = column;
    }

    ColumnSlice {
        start,
        end: end.max(start),
        start_column,
    }
}

pub fn logical_line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut line_start = 0;

    for (byte, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push((line_start, byte));
            line_start = byte.saturating_add(ch.len_utf8()).min(text.len());
        }
    }

    ranges.push((line_start, text.len()));
    ranges
}

fn collapsed_fold_starting_at(folds: &[FoldRange], source_line: usize) -> Option<usize> {
    folds
        .iter()
        .enumerate()
        .filter(|(_, fold)| fold.start_line == source_line && !fold.expanded)
        .max_by_key(|(_, fold)| fold.end_line)
        .map(|(index, _)| index)
}

#[allow(clippy::too_many_arguments)]
fn push_wrapped_visual_lines(
    lines: &mut Vec<VisualLine>,
    source_line: usize,
    start: usize,
    end: usize,
    text: &str,
    wrap: bool,
    wrap_width: f64,
    measure: &mut impl FnMut(&str) -> f64,
) {
    if start == end || !wrap {
        lines.push(VisualLine {
            source_line,
            start,
            end,
            wrap_index: 0,
            folded: None,
        });
        return;
    }

    let mut segment_start = start;
    let mut line_width = 0.0;
    let mut wrap_index = 0;
    for (byte, grapheme) in text[start..end].grapheme_indices(true) {
        let grapheme_start = start + byte;
        let grapheme_width = measure(grapheme);
        if segment_start < grapheme_start && line_width + grapheme_width > wrap_width {
            lines.push(VisualLine {
                source_line,
                start: segment_start,
                end: grapheme_start,
                wrap_index,
                folded: None,
            });
            segment_start = grapheme_start;
            line_width = 0.0;
            wrap_index += 1;
        }
        line_width += grapheme_width;
    }
    lines.push(VisualLine {
        source_line,
        start: segment_start,
        end,
        wrap_index,
        folded: None,
    });
}

fn push_monospace_visual_lines(
    lines: &mut Vec<VisualLine>,
    source_line: usize,
    start: usize,
    end: usize,
    text: &str,
    wrap: bool,
    wrap_columns: f64,
) {
    if start == end || !wrap {
        lines.push(VisualLine {
            source_line,
            start,
            end,
            wrap_index: 0,
            folded: None,
        });
        return;
    }

    let mut segment_start = start;
    let mut line_columns = 0.0;
    let mut wrap_index = 0;
    let mut push_grapheme = |offset: usize, columns: f64| {
        let grapheme_start = start + offset;
        if segment_start < grapheme_start && line_columns + columns > wrap_columns {
            lines.push(VisualLine {
                source_line,
                start: segment_start,
                end: grapheme_start,
                wrap_index,
                folded: None,
            });
            segment_start = grapheme_start;
            line_columns = 0.0;
            wrap_index += 1;
        }
        line_columns += columns;
    };

    let line = &text[start..end];
    if line.is_ascii() {
        for (offset, byte) in line.bytes().enumerate() {
            push_grapheme(
                offset,
                match byte {
                    b'\t' => 4.0,
                    byte if byte.is_ascii_control() => 0.0,
                    _ => 1.0,
                },
            );
        }
    } else {
        for (offset, grapheme) in line.grapheme_indices(true) {
            push_grapheme(offset, grapheme_columns(grapheme));
        }
    }
    drop(push_grapheme);

    lines.push(VisualLine {
        source_line,
        start: segment_start,
        end,
        wrap_index,
        folded: None,
    });
}

pub fn grapheme_columns(grapheme: &str) -> f64 {
    if grapheme == "\t" {
        return 4.0;
    }
    if grapheme.chars().all(|ch| ch.is_ascii_control()) {
        return 0.0;
    }
    if grapheme.is_ascii() {
        return grapheme.len().max(1) as f64;
    }

    let mut columns = 0.0f64;
    for ch in grapheme.chars() {
        if is_combining_mark(ch) {
            continue;
        }
        columns += if is_wide_char(ch) { 2.0 } else { 1.0 };
    }
    columns.max(1.0)
}

fn is_combining_mark(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}

fn is_wide_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2329..=0x232A
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
    )
}
