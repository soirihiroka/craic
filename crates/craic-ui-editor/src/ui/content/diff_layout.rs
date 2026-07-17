use crate::git::{DiffKind, FileDiffRow};
use crate::ui::canvas_scrollbar;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub generation: u64,
    pub content_width: i32,
    pub gutter_width_bits: u64,
    pub line_height_bits: u64,
    pub text_width_bits: u64,
    pub char_width_bits: u64,
    pub rows: usize,
}

#[derive(Clone)]
pub struct WrappedLine {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct RowLayout {
    pub y: f64,
    pub height: f64,
    pub left_lines: Vec<WrappedLine>,
    pub right_lines: Vec<WrappedLine>,
}

pub struct Cache {
    pub signature: Signature,
    pub rows: Vec<RowLayout>,
    pub markers: Vec<ScrollbarMarker>,
    pub content_height: f64,
}

#[derive(Clone, Copy)]
pub struct ScrollbarMarker {
    pub row: usize,
    pub kind: canvas_scrollbar::MarkerKind,
}

pub struct Request {
    pub signature: Signature,
    pub rows: Vec<FileDiffRow>,
    pub text_width: f64,
    pub line_height: f64,
    pub char_width: f64,
}

pub struct Result {
    pub cache: Cache,
}

impl Signature {
    pub fn new(
        generation: u64,
        content_width: i32,
        gutter_width: f64,
        line_height: f64,
        text_width: f64,
        char_width: f64,
        rows: usize,
    ) -> Self {
        Self {
            generation,
            content_width,
            gutter_width_bits: gutter_width.to_bits(),
            line_height_bits: line_height.to_bits(),
            text_width_bits: text_width.to_bits(),
            char_width_bits: char_width.to_bits(),
            rows,
        }
    }
}

pub fn build(request: Request) -> Result {
    let mut y = 0.0;
    let mut layouts = Vec::with_capacity(request.rows.len());
    let mut markers = Vec::new();

    for (index, row) in request.rows.iter().enumerate() {
        if let Some(kind) = marker_kind(row) {
            markers.push(ScrollbarMarker { row: index, kind });
        }

        let left_lines = wrap_text(
            row.left_text.as_deref().unwrap_or_default(),
            request.text_width,
            request.char_width,
        );
        let right_lines = wrap_text(
            row.right_text.as_deref().unwrap_or_default(),
            request.text_width,
            request.char_width,
        );
        let shared_visual_line_count = left_lines.len().max(right_lines.len()).max(1);
        let height = shared_visual_line_count as f64 * request.line_height;
        layouts.push(RowLayout {
            y,
            height,
            left_lines,
            right_lines,
        });
        y += height;
    }

    let content_height = y.max(request.line_height.max(1.0));
    let cache = Cache {
        signature: request.signature,
        rows: layouts,
        markers,
        content_height,
    };

    Result { cache }
}

pub fn visible_row_range(
    cache: &Cache,
    scroll_y: f64,
    viewport_height: f64,
    overscan: f64,
) -> std::ops::Range<usize> {
    if cache.rows.is_empty() {
        return 0..0;
    }

    let start_y = (scroll_y - overscan).max(0.0);
    let end_y = scroll_y + viewport_height.max(1.0) + overscan;
    let mut start = cache
        .rows
        .partition_point(|layout| layout.y + layout.height < start_y);
    if start > 0 {
        start -= 1;
    }
    let mut end = start;
    while end < cache.rows.len() && cache.rows[end].y <= end_y {
        end += 1;
    }
    if end < cache.rows.len() {
        end += 1;
    }
    start..end
}

pub fn row_index_at_y(cache: &Cache, document_y: f64) -> Option<usize> {
    if cache.rows.is_empty() {
        return None;
    }

    let mut index = cache
        .rows
        .partition_point(|layout| layout.y + layout.height <= document_y);
    if index >= cache.rows.len() {
        index = cache.rows.len().saturating_sub(1);
    }
    let layout = cache.rows.get(index)?;
    (document_y >= layout.y && document_y < layout.y + layout.height).then_some(index)
}

fn wrap_text(text: &str, wrap_width: f64, char_width: f64) -> Vec<WrappedLine> {
    if text.is_empty() {
        return vec![WrappedLine {
            text: String::new(),
            start: 0,
            end: 0,
        }];
    }

    let wrap_columns = (wrap_width / char_width.max(1.0)).max(1.0);
    let mut lines = Vec::new();
    let mut segment_start = 0;
    let mut segment_columns = 0.0;

    for (byte, grapheme) in text.grapheme_indices(true) {
        let width = grapheme_columns(grapheme);
        if segment_start < byte && segment_columns + width > wrap_columns {
            lines.push(WrappedLine {
                text: text[segment_start..byte].to_string(),
                start: segment_start,
                end: byte,
            });
            segment_start = byte;
            segment_columns = 0.0;
        }
        segment_columns += width;
    }

    lines.push(WrappedLine {
        text: text[segment_start..].to_string(),
        start: segment_start,
        end: text.len(),
    });
    lines
}

fn grapheme_columns(grapheme: &str) -> f64 {
    if grapheme == "\t" {
        return 4.0;
    }
    if grapheme.chars().all(|ch| ch.is_ascii_control()) {
        return 0.0;
    }
    if grapheme.is_ascii() {
        return grapheme.chars().count().max(1) as f64;
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

fn marker_kind(row: &FileDiffRow) -> Option<canvas_scrollbar::MarkerKind> {
    let added = row.right_kind == DiffKind::Added;
    let deleted = row.left_kind == DiffKind::Deleted;
    match (added, deleted) {
        (true, true) => Some(canvas_scrollbar::MarkerKind::Mixed),
        (true, false) => Some(canvas_scrollbar::MarkerKind::Added),
        (false, true) => Some(canvas_scrollbar::MarkerKind::Deleted),
        (false, false) => None,
    }
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
