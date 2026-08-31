use crate::{
    EditorDocument, EditorMetrics, EditorSearchMatch, EditorSelection, TextDiagnosticKind,
    TextDiagnosticSpan, TextSyntaxSpan,
};
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextStyle,
};
use skia_safe::{Canvas, Color, Color4f, FontMgr, Paint, PathBuilder, Rect};

pub struct EditorPaintRequest<'a> {
    pub document: &'a EditorDocument,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub selection: EditorSelection,
    pub marked_text: Option<&'a str>,
    pub diagnostics: &'a [TextDiagnosticSpan],
    pub search_matches: &'a [EditorSearchMatch],
    pub active_search_match: Option<usize>,
    pub completion_items: &'a [String],
    pub completion_selected: usize,
    pub focused: bool,
    pub metrics: EditorMetrics,
}

pub fn paint_editor(canvas: &Canvas, request: EditorPaintRequest<'_>) {
    canvas.clear(Color::from_rgb(30, 30, 30));
    let metrics = request.metrics;
    let fonts = font_collection();
    let text_x = metrics.gutter_width + metrics.text_inset - request.scroll_x as f32;
    let visible = request.document.visible_line_range(
        request.scroll_y,
        request.viewport_height as f64,
        metrics,
    );
    canvas.save();
    canvas.clip_rect(
        Rect::from_xywh(0.0, 0.0, request.viewport_width, request.viewport_height),
        None,
        false,
    );
    fill_rect(
        canvas,
        0.0,
        0.0,
        metrics.gutter_width,
        request.viewport_height,
        rgba(0.105, 0.105, 0.105, 1.0),
    );
    let (selection_start, selection_end) = request.selection.normalized();
    for visual_line in visible {
        let Some(source_line) = request.document.visual_lines().get(visual_line).copied() else {
            continue;
        };
        let Some(line) = request.document.lines().get(source_line) else {
            continue;
        };
        let y =
            metrics.text_inset + visual_line as f32 * metrics.line_height - request.scroll_y as f32;
        draw_text(
            canvas,
            &fonts,
            &(source_line + 1).to_string(),
            metrics.gutter_width - metrics.text_inset - 8.0,
            y + metrics.font_size * 1.27,
            Color::from_rgb(116, 116, 116),
            metrics.font_size,
            true,
        );
        let first_match = request
            .search_matches
            .partition_point(|search_match| search_match.end <= line.start);
        for (relative_index, search_match) in
            request.search_matches[first_match..].iter().enumerate()
        {
            if search_match.start >= line.end {
                break;
            }
            let start = search_match.start.max(line.start);
            let end = search_match.end.min(line.end);
            if start >= end
                || end > request.document.text().len()
                || !request.document.text().is_char_boundary(start)
                || !request.document.text().is_char_boundary(end)
            {
                continue;
            }
            fill_rect(
                canvas,
                text_x
                    + text_advance(
                        &request.document.text()[line.start..start],
                        metrics.char_width,
                    ),
                y + 2.0,
                text_advance(&request.document.text()[start..end], metrics.char_width).max(2.0),
                metrics.line_height - 4.0,
                if request.active_search_match == Some(first_match + relative_index) {
                    rgba(1.0, 0.75, 0.44, 0.62)
                } else {
                    rgba(0.96, 0.83, 0.18, 0.30)
                },
            );
        }
        let line_selection_start = selection_start.max(line.start).min(line.end);
        let line_selection_end = selection_end.max(line.start).min(line.end);
        if line_selection_start < line_selection_end {
            let before = &request.document.text()[line.start..line_selection_start];
            let selected = &request.document.text()[line_selection_start..line_selection_end];
            fill_rect(
                canvas,
                text_x + text_advance(before, metrics.char_width),
                y + 1.0,
                text_advance(selected, metrics.char_width).max(2.0),
                metrics.line_height - 2.0,
                rgba(0.18, 0.46, 0.80, 0.72),
            );
        }
        draw_syntax_line(
            canvas,
            &fonts,
            request.document,
            line.start,
            line.end,
            text_x,
            y + metrics.font_size * 1.27,
            metrics,
        );
        if let Some(fold) = request.document.fold_starting_at(source_line) {
            draw_fold_control(canvas, y, metrics, fold.expanded);
            if !fold.expanded {
                let line_text = request.document.line_text(source_line);
                draw_text(
                    canvas,
                    &fonts,
                    &format!(
                        "  … {} lines",
                        fold.end_line.saturating_sub(fold.start_line)
                    ),
                    text_x + text_advance(line_text, metrics.char_width),
                    y + metrics.font_size * 1.27,
                    Color::from_rgb(145, 145, 145),
                    metrics.font_size,
                    false,
                );
            }
        }
        draw_diagnostics_line(
            canvas,
            request.document,
            request.diagnostics,
            line.start,
            line.end,
            text_x,
            y + metrics.line_height - 2.0,
            metrics,
        );
    }
    if request.focused && request.selection.is_empty() {
        let (_, column) = request
            .document
            .line_column_for_offset(request.selection.focus);
        let line = request
            .document
            .visual_line_for_offset(request.selection.focus);
        let x = text_x + column as f32 * metrics.char_width;
        let y = metrics.text_inset + line as f32 * metrics.line_height - request.scroll_y as f32;
        fill_rect(
            canvas,
            x,
            y + 2.0,
            1.5,
            metrics.line_height - 4.0,
            rgba(0.90, 0.90, 0.90, 1.0),
        );
        if let Some(marked_text) = request.marked_text.filter(|text| !text.is_empty()) {
            let marked_width = text_advance(marked_text, metrics.char_width).max(2.0);
            fill_rect(
                canvas,
                x,
                y + metrics.line_height - 2.0,
                marked_width,
                1.0,
                rgba(0.90, 0.90, 0.90, 0.9),
            );
            draw_text(
                canvas,
                &fonts,
                marked_text,
                x,
                y + metrics.font_size * 1.27,
                Color::from_rgb(225, 225, 225),
                metrics.font_size,
                false,
            );
        }
    }
    draw_completion_popup(canvas, &fonts, &request);
    canvas.restore();
}

fn draw_completion_popup(
    canvas: &Canvas,
    fonts: &FontCollection,
    request: &EditorPaintRequest<'_>,
) {
    if request.completion_items.is_empty() {
        return;
    }
    let metrics = request.metrics;
    let (_, column) = request
        .document
        .line_column_for_offset(request.selection.focus);
    let visual_line = request
        .document
        .visual_line_for_offset(request.selection.focus);
    let caret_x = metrics.gutter_width + metrics.text_inset + column as f32 * metrics.char_width
        - request.scroll_x as f32;
    let caret_y =
        metrics.text_inset + visual_line as f32 * metrics.line_height - request.scroll_y as f32;
    let visible_count = request.completion_items.len().min(8);
    let first_visible = request
        .completion_selected
        .saturating_add(1)
        .saturating_sub(visible_count);
    let row_height = metrics.line_height.max(22.0);
    let popup_height = visible_count as f32 * row_height + 8.0;
    let longest = request
        .completion_items
        .iter()
        .skip(first_visible)
        .take(visible_count)
        .map(|item| item.chars().count())
        .max()
        .unwrap_or(0);
    let popup_width = (longest as f32 * metrics.char_width + 28.0)
        .clamp(180.0, (request.viewport_width - 16.0).max(180.0));
    let x = caret_x.clamp(8.0, (request.viewport_width - popup_width - 8.0).max(8.0));
    let below = caret_y + metrics.line_height + 4.0;
    let y = if below + popup_height <= request.viewport_height - 8.0 {
        below
    } else {
        (caret_y - popup_height - 4.0).max(8.0)
    };
    let mut background = Paint::new(rgba(0.12, 0.12, 0.13, 0.98), None);
    background.set_anti_alias(true);
    canvas.draw_round_rect(
        Rect::from_xywh(x, y, popup_width, popup_height),
        9.0,
        9.0,
        &background,
    );
    for (row, item) in request
        .completion_items
        .iter()
        .skip(first_visible)
        .take(visible_count)
        .enumerate()
    {
        let row_y = y + 4.0 + row as f32 * row_height;
        if first_visible + row == request.completion_selected {
            fill_rect(
                canvas,
                x + 4.0,
                row_y,
                popup_width - 8.0,
                row_height,
                rgba(0.12, 0.46, 0.84, 0.92),
            );
        }
        draw_text(
            canvas,
            fonts,
            item,
            x + 12.0,
            row_y + metrics.font_size * 1.27,
            Color::from_rgb(235, 235, 235),
            metrics.font_size,
            false,
        );
    }
}

fn draw_fold_control(canvas: &Canvas, y: f32, metrics: EditorMetrics, expanded: bool) {
    let center_x = 13.0;
    let center_y = y + metrics.line_height * 0.5;
    let mut path = PathBuilder::new();
    if expanded {
        path.move_to((center_x - 4.0, center_y - 2.5));
        path.line_to((center_x + 4.0, center_y - 2.5));
        path.line_to((center_x, center_y + 3.5));
    } else {
        path.move_to((center_x - 2.5, center_y - 4.0));
        path.line_to((center_x + 3.5, center_y));
        path.line_to((center_x - 2.5, center_y + 4.0));
    }
    path.close();
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgb(145, 145, 145));
    paint.set_anti_alias(true);
    canvas.draw_path(&path.detach(), &paint);
}

fn draw_diagnostics_line(
    canvas: &Canvas,
    document: &EditorDocument,
    diagnostics: &[TextDiagnosticSpan],
    line_start: usize,
    line_end: usize,
    text_x: f32,
    y: f32,
    metrics: EditorMetrics,
) {
    let first = diagnostics.partition_point(|diagnostic| diagnostic.end <= line_start);
    for diagnostic in &diagnostics[first..] {
        if diagnostic.start >= line_end {
            break;
        }
        if diagnostic.start >= diagnostic.end
            || diagnostic.end > document.text().len()
            || !document.text().is_char_boundary(diagnostic.start)
            || !document.text().is_char_boundary(diagnostic.end)
        {
            continue;
        }
        let start = diagnostic.start.max(line_start);
        let end = diagnostic.end.min(line_end);
        if start >= end {
            continue;
        }
        let x = text_x + text_advance(&document.text()[line_start..start], metrics.char_width);
        let width = text_advance(&document.text()[start..end], metrics.char_width).max(2.0);
        let color = match diagnostic.kind {
            TextDiagnosticKind::Error => Color::from_rgb(224, 27, 36),
            TextDiagnosticKind::Warning | TextDiagnosticKind::Spelling => {
                Color::from_rgb(246, 211, 45)
            }
        };
        draw_wavy_underline(canvas, x, y, width, color);
    }
}

fn draw_wavy_underline(canvas: &Canvas, x: f32, y: f32, width: f32, color: Color) {
    if width <= 1.0 {
        return;
    }
    let mut path = PathBuilder::new();
    path.move_to((x, y));
    let mut current = 0.0;
    let mut up = true;
    while current < width {
        current = (current + 3.0).min(width);
        path.line_to((x + current, y + if up { -1.4 } else { 1.4 }));
        up = !up;
    }
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.set_anti_alias(true);
    paint.set_stroke(true);
    paint.set_stroke_width(1.2);
    canvas.draw_path(&path.detach(), &paint);
}

fn draw_syntax_line(
    canvas: &Canvas,
    fonts: &FontCollection,
    document: &EditorDocument,
    line_start: usize,
    line_end: usize,
    mut x: f32,
    baseline: f32,
    metrics: EditorMetrics,
) {
    let mut cursor = line_start;
    for span in document.syntax() {
        if span.end <= line_start {
            continue;
        }
        if span.start >= line_end {
            break;
        }
        let start = span.start.max(line_start).max(cursor);
        let end = span.end.min(line_end);
        if cursor < start {
            let plain = &document.text()[cursor..start];
            draw_text(
                canvas,
                fonts,
                plain,
                x,
                baseline,
                Color::from_rgb(225, 225, 225),
                metrics.font_size,
                false,
            );
            x += text_advance(plain, metrics.char_width);
        }
        if start < end {
            let segment = &document.text()[start..end];
            draw_text(
                canvas,
                fonts,
                segment,
                x,
                baseline,
                syntax_color(span),
                metrics.font_size,
                false,
            );
            x += text_advance(segment, metrics.char_width);
            cursor = end;
        }
    }
    if cursor < line_end {
        draw_text(
            canvas,
            fonts,
            &document.text()[cursor..line_end],
            x,
            baseline,
            Color::from_rgb(225, 225, 225),
            metrics.font_size,
            false,
        );
    }
}

fn syntax_color(span: &TextSyntaxSpan) -> Color {
    Color::from_rgb(
        (span.color[0] * 255.0).round() as u8,
        (span.color[1] * 255.0).round() as u8,
        (span.color[2] * 255.0).round() as u8,
    )
}

fn draw_text(
    canvas: &Canvas,
    fonts: &FontCollection,
    text: &str,
    x: f32,
    baseline: f32,
    color: Color,
    font_size: f32,
    align_right: bool,
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
    let x = if align_right {
        x - paragraph.max_intrinsic_width()
    } else {
        x
    };
    paragraph.paint(canvas, (x, baseline - paragraph.alphabetic_baseline()));
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
