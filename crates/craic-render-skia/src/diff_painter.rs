use crate::{
    DiffLayoutCache, DiffRow, DiffRowKind, DiffSearchMatch, DiffSide, DiffSyntaxSpan,
    DiffTextSelection, DiffWrappedLine, visible_diff_row_range,
};
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle,
};
use skia_safe::{Canvas, Color, Color4f, FontMgr, Paint, Rect};

const CELL_PADDING: f32 = 10.0;
const GUTTER_WIDTH: f32 = 58.0;
const DIVIDER_WIDTH: f32 = 1.0;
const OVERSCAN: f64 = 88.0;

pub struct DiffPaintRequest<'a> {
    pub rows: &'a [DiffRow],
    pub layout: &'a DiffLayoutCache,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub scroll_y: f64,
    pub selection: Option<DiffTextSelection>,
    pub search_matches: &'a [DiffSearchMatch],
    pub active_search_match: Option<usize>,
    pub syntax: &'a [DiffSyntaxSpan],
    pub char_width: f32,
    pub font_size: f32,
    pub baseline_offset: f32,
}

pub fn paint_diff(canvas: &Canvas, request: DiffPaintRequest<'_>) {
    canvas.clear(Color::from_rgb(30, 30, 30));
    let width = request.viewport_width.max(1.0);
    let height = request.viewport_height.max(1.0);
    let divider_x = width / 2.0;
    let fonts = font_collection();
    let range = visible_diff_row_range(
        request.layout,
        request.scroll_y,
        request.viewport_height as f64,
        OVERSCAN,
    );

    canvas.save();
    canvas.clip_rect(Rect::from_xywh(0.0, 0.0, width, height), None, false);
    for row_index in range {
        let Some(row) = request.rows.get(row_index) else {
            continue;
        };
        let Some(layout) = request.layout.rows.get(row_index) else {
            continue;
        };
        let y = (layout.y - request.scroll_y) as f32;
        let row_height = layout.height as f32;
        if row.left_kind == DiffRowKind::Fold || row.right_kind == DiffRowKind::Fold {
            fill_rect(
                canvas,
                0.0,
                y,
                width,
                row_height,
                rgba(0.17, 0.22, 0.28, 1.0),
            );
            draw_text(
                canvas,
                &fonts,
                row.right_text
                    .as_deref()
                    .or(row.left_text.as_deref())
                    .unwrap_or_default(),
                CELL_PADDING,
                y + request.baseline_offset,
                Color::from_rgb(147, 190, 238),
                request.font_size,
            );
            continue;
        }

        draw_side(
            canvas,
            &fonts,
            request.rows,
            row_index,
            row.left_number,
            row.left_kind,
            DiffSide::Left,
            &layout.left_lines,
            0.0,
            divider_x,
            y,
            row_height,
            request.layout.signature.line_height_bits,
            request.selection,
            request.search_matches,
            request.active_search_match,
            request.syntax,
            request.char_width,
            request.font_size,
            request.baseline_offset,
        );
        draw_side(
            canvas,
            &fonts,
            request.rows,
            row_index,
            row.right_number,
            row.right_kind,
            DiffSide::Right,
            &layout.right_lines,
            divider_x + DIVIDER_WIDTH,
            width - divider_x - DIVIDER_WIDTH,
            y,
            row_height,
            request.layout.signature.line_height_bits,
            request.selection,
            request.search_matches,
            request.active_search_match,
            request.syntax,
            request.char_width,
            request.font_size,
            request.baseline_offset,
        );
    }
    fill_rect(
        canvas,
        divider_x,
        0.0,
        DIVIDER_WIDTH,
        height,
        rgba(0.32, 0.32, 0.32, 1.0),
    );
    canvas.restore();
}

#[allow(clippy::too_many_arguments)]
fn draw_side(
    canvas: &Canvas,
    fonts: &FontCollection,
    _rows: &[DiffRow],
    row_index: usize,
    number: Option<usize>,
    kind: DiffRowKind,
    side: DiffSide,
    lines: &[DiffWrappedLine],
    x: f32,
    width: f32,
    y: f32,
    height: f32,
    line_height_bits: u64,
    selection: Option<DiffTextSelection>,
    search_matches: &[DiffSearchMatch],
    active_search_match: Option<usize>,
    syntax: &[DiffSyntaxSpan],
    char_width: f32,
    font_size: f32,
    baseline_offset: f32,
) {
    let (background, text_color) = match kind {
        DiffRowKind::Added => (rgba(0.10, 0.28, 0.18, 0.90), Color::from_rgb(211, 245, 220)),
        DiffRowKind::Deleted => (rgba(0.35, 0.12, 0.14, 0.90), Color::from_rgb(255, 218, 220)),
        DiffRowKind::Context | DiffRowKind::Fold => {
            (rgba(0.12, 0.12, 0.12, 1.0), Color::from_rgb(224, 224, 224))
        }
    };
    fill_rect(canvas, x, y, width, height, background);
    let gutter_x = if side == DiffSide::Left {
        x + width - GUTTER_WIDTH
    } else {
        x
    };
    fill_rect(
        canvas,
        gutter_x,
        y,
        GUTTER_WIDTH,
        height,
        rgba(0.15, 0.15, 0.15, 1.0),
    );
    let line_height = f64::from_bits(line_height_bits) as f32;

    if let Some(number) = number {
        draw_text(
            canvas,
            fonts,
            &number.to_string(),
            if side == DiffSide::Left {
                gutter_x + 8.0
            } else {
                gutter_x + 20.0
            },
            y + baseline_offset,
            Color::from_rgb(135, 135, 135),
            font_size,
        );
    }
    let marker = match kind {
        DiffRowKind::Added => "+",
        DiffRowKind::Deleted => "−",
        DiffRowKind::Context | DiffRowKind::Fold => "",
    };
    if !marker.is_empty() {
        draw_text(
            canvas,
            fonts,
            marker,
            if side == DiffSide::Left {
                gutter_x + GUTTER_WIDTH - 15.0
            } else {
                gutter_x + 6.0
            },
            y + baseline_offset,
            Color::from_rgb(155, 155, 155),
            font_size,
        );
    }

    let text_x = if side == DiffSide::Left {
        x + CELL_PADDING
    } else {
        x + GUTTER_WIDTH + CELL_PADDING
    };
    for (line_index, line) in lines.iter().enumerate() {
        let line_y = y + line_index as f32 * line_height;
        draw_matches(
            canvas,
            line,
            side,
            row_index,
            text_x,
            line_y,
            line_height,
            search_matches,
            active_search_match,
            char_width,
        );
        draw_selection(
            canvas,
            line,
            side,
            row_index,
            text_x,
            line_y,
            line_height,
            selection,
            char_width,
        );
        draw_syntax_text(
            canvas,
            fonts,
            &line.text,
            text_x,
            line_y + baseline_offset,
            text_color,
            char_width,
            syntax,
            side,
            number,
            line.start,
            line.end,
            font_size,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_matches(
    canvas: &Canvas,
    line: &DiffWrappedLine,
    side: DiffSide,
    row: usize,
    x: f32,
    y: f32,
    line_height: f32,
    matches: &[DiffSearchMatch],
    active: Option<usize>,
    char_width: f32,
) {
    for (index, search_match) in matches.iter().enumerate() {
        if search_match.side != side || search_match.row != row {
            continue;
        }
        let start = search_match.start.max(line.start);
        let end = search_match.end.min(line.end);
        if start >= end {
            continue;
        }
        let offset = line
            .text
            .get(..start.saturating_sub(line.start))
            .unwrap_or_default();
        let matched = line
            .text
            .get(start.saturating_sub(line.start)..end.saturating_sub(line.start))
            .unwrap_or_default();
        fill_rect(
            canvas,
            x + text_advance(offset, char_width),
            y + 2.0,
            text_advance(matched, char_width).max(2.0),
            line_height - 4.0,
            if Some(index) == active {
                rgba(0.92, 0.62, 0.16, 0.72)
            } else {
                rgba(0.74, 0.53, 0.17, 0.40)
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_selection(
    canvas: &Canvas,
    line: &DiffWrappedLine,
    side: DiffSide,
    row: usize,
    x: f32,
    y: f32,
    line_height: f32,
    selection: Option<DiffTextSelection>,
    char_width: f32,
) {
    let Some(selection) = selection else { return };
    if selection.anchor.side != side || selection.focus.side != side {
        return;
    }
    let (start_point, end_point) = if selection.anchor <= selection.focus {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    };
    if row < start_point.row || row > end_point.row {
        return;
    }
    let row_start = if row == start_point.row {
        start_point.byte
    } else {
        0
    };
    let row_end = if row == end_point.row {
        end_point.byte
    } else {
        usize::MAX
    };
    let start = row_start.max(line.start);
    let end = row_end.min(line.end);
    if start >= end {
        return;
    }
    let offset = line
        .text
        .get(..start.saturating_sub(line.start))
        .unwrap_or_default();
    let selected = line
        .text
        .get(start.saturating_sub(line.start)..end.saturating_sub(line.start))
        .unwrap_or_default();
    fill_rect(
        canvas,
        x + text_advance(offset, char_width),
        y + 1.0,
        text_advance(selected, char_width).max(2.0),
        line_height - 2.0,
        rgba(0.20, 0.48, 0.82, 0.65),
    );
}

fn draw_text(
    canvas: &Canvas,
    fonts: &FontCollection,
    text: &str,
    x: f32,
    baseline: f32,
    color: Color,
    font_size: f32,
) {
    if text.is_empty() {
        return;
    }
    let mut style = TextStyle::new();
    style
        .set_color(color)
        .set_font_size(font_size)
        .set_font_families(&["SF Mono", "Menlo", "monospace"]);
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_style(&style);
    let mut builder = ParagraphBuilder::new(&paragraph_style, fonts.clone());
    builder.push_style(&style);
    builder.add_text(&text.replace('\t', "    "));
    builder.pop();
    let mut paragraph: Paragraph = builder.build();
    paragraph.layout(1_000_000.0);
    paragraph.paint(canvas, (x, baseline - paragraph.alphabetic_baseline()));
}

#[allow(clippy::too_many_arguments)]
fn draw_syntax_text(
    canvas: &Canvas,
    fonts: &FontCollection,
    text: &str,
    mut x: f32,
    baseline: f32,
    fallback: Color,
    char_width: f32,
    syntax: &[DiffSyntaxSpan],
    side: DiffSide,
    line_number: Option<usize>,
    line_start: usize,
    line_end: usize,
    font_size: f32,
) {
    let mut cursor = line_start;
    let Some(line_number) = line_number else {
        draw_text(canvas, fonts, text, x, baseline, fallback, font_size);
        return;
    };
    for span in syntax
        .iter()
        .filter(|span| span.side == side && span.line_number == line_number)
    {
        if span.end <= line_start {
            continue;
        }
        if span.start >= line_end {
            break;
        }
        let start = span.start.max(line_start).max(cursor);
        let end = span.end.min(line_end);
        if start >= end {
            continue;
        }
        let plain_start = cursor.saturating_sub(line_start);
        let plain_end = start.saturating_sub(line_start);
        if let Some(plain) = text
            .get(plain_start..plain_end)
            .filter(|plain| !plain.is_empty())
        {
            draw_text(canvas, fonts, plain, x, baseline, fallback, font_size);
            x += text_advance(plain, char_width);
        }
        let segment_start = start.saturating_sub(line_start);
        let segment_end = end.saturating_sub(line_start);
        let Some(segment) = text
            .get(segment_start..segment_end)
            .filter(|segment| !segment.is_empty())
        else {
            continue;
        };
        draw_text(
            canvas,
            fonts,
            segment,
            x,
            baseline,
            Color::from_rgb(
                (span.color[0] * 255.0).round() as u8,
                (span.color[1] * 255.0).round() as u8,
                (span.color[2] * 255.0).round() as u8,
            ),
            font_size,
        );
        x += text_advance(segment, char_width);
        cursor = end;
    }
    let remaining_start = cursor.saturating_sub(line_start);
    if let Some(remaining) = text.get(remaining_start..).filter(|text| !text.is_empty()) {
        draw_text(canvas, fonts, remaining, x, baseline, fallback, font_size);
    }
}

fn font_collection() -> FontCollection {
    let mut fonts = FontCollection::new();
    fonts.set_default_font_manager(FontMgr::new(), Some("SF Mono"));
    fonts.enable_font_fallback();
    fonts
}

fn text_advance(text: &str, char_width: f32) -> f32 {
    text.chars()
        .map(|character| if character == '\t' { 4.0 } else { 1.0 })
        .sum::<f32>()
        * char_width
}

fn fill_rect(canvas: &Canvas, x: f32, y: f32, width: f32, height: f32, color: Color4f) {
    let mut paint = Paint::new(color, None);
    paint.set_anti_alias(false);
    canvas.draw_rect(Rect::from_xywh(x, y, width, height), &paint);
}

const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Color4f {
    Color4f::new(red, green, blue, alpha)
}
