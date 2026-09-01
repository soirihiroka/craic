use crate::{DiffRow, DiffRowKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMarkerKind {
    Added,
    Deleted,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLayoutSignature {
    pub generation: u64,
    pub content_width: i32,
    pub gutter_width_bits: u64,
    pub line_height_bits: u64,
    pub text_width_bits: u64,
    pub char_width_bits: u64,
    pub rows: usize,
}

#[derive(Clone)]
pub struct DiffWrappedLine {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone)]
pub struct DiffRowLayout {
    pub y: f64,
    pub height: f64,
    pub left_lines: Vec<DiffWrappedLine>,
    pub right_lines: Vec<DiffWrappedLine>,
}

pub struct DiffLayoutCache {
    pub signature: DiffLayoutSignature,
    pub rows: Vec<DiffRowLayout>,
    pub markers: Vec<DiffScrollbarMarker>,
    pub content_height: f64,
}

#[derive(Clone, Copy)]
pub struct DiffScrollbarMarker {
    pub row: usize,
    pub kind: DiffMarkerKind,
}

pub struct DiffLayoutRequest {
    pub signature: DiffLayoutSignature,
    pub rows: Vec<DiffRow>,
    pub text_width: f64,
    pub line_height: f64,
    pub char_width: f64,
}

impl DiffLayoutSignature {
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

pub fn build_diff_layout(request: DiffLayoutRequest) -> DiffLayoutCache {
    let mut y = 0.0;
    let mut layouts = Vec::with_capacity(request.rows.len());
    let mut markers = Vec::new();

    for (index, row) in request.rows.iter().enumerate() {
        if let Some(kind) = marker_kind(row) {
            markers.push(DiffScrollbarMarker { row: index, kind });
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
        layouts.push(DiffRowLayout {
            y,
            height,
            left_lines,
            right_lines,
        });
        y += height;
    }

    DiffLayoutCache {
        signature: request.signature,
        rows: layouts,
        markers,
        content_height: y.max(request.line_height.max(1.0)),
    }
}

pub fn visible_diff_row_range(
    cache: &DiffLayoutCache,
    scroll_y: f64,
    viewport_height: f64,
    overscan: f64,
) -> std::ops::Range<usize> {
    if cache.rows.is_empty() {
        return 0..0;
    }

    let start_y = (scroll_y - overscan).max(0.0);
    let end_y = scroll_y + viewport_height.max(1.0) + overscan;
    let start = cache
        .rows
        .partition_point(|layout| layout.y + layout.height <= start_y);
    let end = cache.rows.partition_point(|layout| layout.y < end_y);
    start..end
}

pub fn diff_row_index_at_y(cache: &DiffLayoutCache, document_y: f64) -> Option<usize> {
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

fn wrap_text(text: &str, wrap_width: f64, char_width: f64) -> Vec<DiffWrappedLine> {
    craic_text_layout::build_visual_lines_monospace(
        text,
        &[],
        true,
        wrap_width / char_width.max(1.0),
    )
    .into_iter()
    .map(|line| DiffWrappedLine {
        text: text
            .get(line.start..line.end)
            .unwrap_or_default()
            .to_string(),
        start: line.start,
        end: line.end,
    })
    .collect()
}

fn marker_kind(row: &DiffRow) -> Option<DiffMarkerKind> {
    let added = row.right_kind == DiffRowKind::Added;
    let deleted = row.left_kind == DiffRowKind::Deleted;
    match (added, deleted) {
        (true, true) => Some(DiffMarkerKind::Mixed),
        (true, false) => Some(DiffMarkerKind::Added),
        (false, true) => Some(DiffMarkerKind::Deleted),
        (false, false) => None,
    }
}
