use crate::{Editor, EditorFrame, Paint};
use craic_render_skia::{
    EditorDocument, EditorMetrics, TextDiagnosticKind, TextDiagnosticSpan, TextSyntaxSpan,
};
use gpui::Rgba;

pub fn paint_editor(frame: &mut EditorFrame, editor: &Editor, focused: bool, dark: bool) {
    let width = f32::from(frame.size.width);
    let height = f32::from(frame.size.height);
    let scroll_x = editor.viewport.scroll_x();
    let scroll_y = editor.viewport.scroll_y();
    let foreground = if dark {
        rgb(225, 225, 225)
    } else {
        rgb(35, 35, 35)
    };
    fill_rect(
        frame,
        0.0,
        0.0,
        width,
        height,
        if dark {
            rgb(30, 30, 30)
        } else {
            rgb(250, 250, 250)
        },
    );
    let metrics = editor.metrics;
    let text_x = metrics.gutter_width + metrics.text_inset - scroll_x as f32;
    let visible = editor
        .document
        .visible_line_range(scroll_y, height as f64, metrics);
    fill_rect(
        frame,
        0.0,
        0.0,
        metrics.gutter_width,
        height,
        if dark {
            rgb(27, 27, 27)
        } else {
            rgb(240, 240, 240)
        },
    );
    let (selection_start, selection_end) = editor.selection.normalized();
    for visual_line in visible {
        let Some(display_line) = editor.document.visual_lines().get(visual_line) else {
            continue;
        };
        let source_line = display_line.source_line;
        let Some(line) = editor.document.lines().get(source_line) else {
            continue;
        };
        let y = metrics.text_inset + visual_line as f32 * metrics.line_height - scroll_y as f32;
        if display_line.wrap_index == 0 {
            frame.text(
                &(source_line + 1).to_string(),
                metrics.gutter_width - metrics.text_inset - 8.0,
                y + metrics.font_size * 1.27,
                rgb(116, 116, 116),
                metrics.font_size,
                true,
            );
        }
        let first_match = editor
            .search_matches
            .partition_point(|search_match| search_match.end <= display_line.start);
        for (relative_index, search_match) in
            editor.search_matches[first_match..].iter().enumerate()
        {
            if search_match.start >= display_line.end {
                break;
            }
            let start = search_match.start.max(display_line.start);
            let end = search_match.end.min(display_line.end);
            if start >= end
                || end > editor.document.text().len()
                || !editor.document.text().is_char_boundary(start)
                || !editor.document.text().is_char_boundary(end)
            {
                continue;
            }
            fill_rect(
                frame,
                text_x
                    + text_advance(
                        &editor.document.text()[display_line.start..start],
                        metrics.char_width,
                    ),
                y + 2.0,
                text_advance(&editor.document.text()[start..end], metrics.char_width).max(2.0),
                metrics.line_height - 4.0,
                if editor.active_search_match == Some(first_match + relative_index) {
                    rgba(1.0, 0.75, 0.44, 0.62)
                } else {
                    rgba(0.96, 0.83, 0.18, 0.30)
                },
            );
        }
        let line_selection_start = selection_start
            .max(display_line.start)
            .min(display_line.end);
        let line_selection_end = selection_end.max(display_line.start).min(display_line.end);
        let newline_selected = display_line.end == line.end
            && line.end_with_newline > line.end
            && selection_start <= line.end
            && selection_end > line.end;
        if line_selection_start < line_selection_end || newline_selected {
            let before = &editor.document.text()[display_line.start..line_selection_start];
            let selected = &editor.document.text()[line_selection_start..line_selection_end];
            fill_rect(
                frame,
                text_x + text_advance(before, metrics.char_width),
                y + 1.0,
                (text_advance(selected, metrics.char_width)
                    + if newline_selected {
                        metrics.char_width * 0.5
                    } else {
                        0.0
                    })
                .max(metrics.char_width * 0.5),
                metrics.line_height - 2.0,
                rgba(0.18, 0.46, 0.80, 0.72),
            );
        }
        draw_syntax_line(
            frame,
            &editor.document,
            display_line.start,
            display_line.end,
            text_x,
            y + metrics.font_size * 1.27,
            metrics,
            foreground,
            dark,
        );
        if display_line.wrap_index == 0
            && let Some(fold) = editor.document.fold_starting_at(source_line)
        {
            draw_fold_control(frame, y, metrics, fold.expanded);
            if !fold.expanded {
                let line_text = editor.document.line_text(source_line);
                frame.text(
                    &format!(
                        "  … {} lines",
                        fold.end_line.saturating_sub(fold.start_line)
                    ),
                    text_x + text_advance(line_text, metrics.char_width),
                    y + metrics.font_size * 1.27,
                    rgb(145, 145, 145),
                    metrics.font_size,
                    false,
                );
            }
        }
        draw_diagnostics_line(
            frame,
            &editor.document,
            &editor.diagnostics,
            display_line.start,
            display_line.end,
            text_x,
            y + metrics.line_height - 2.0,
            metrics,
        );
    }
    if focused && editor.selection.is_empty() {
        let line = editor
            .document
            .visual_line_for_offset(editor.selection.focus);
        let column = editor
            .document
            .visual_column_for_offset(editor.selection.focus);
        let x = text_x + column as f32 * metrics.char_width;
        let y = metrics.text_inset + line as f32 * metrics.line_height - scroll_y as f32;
        fill_rect(
            frame,
            x,
            y + 2.0,
            1.5,
            metrics.line_height - 4.0,
            foreground,
        );
        let marked_text = &editor.marked_text;
        if !marked_text.is_empty() {
            let marked_width = text_advance(marked_text, metrics.char_width).max(2.0);
            fill_rect(
                frame,
                x,
                y + metrics.line_height - 2.0,
                marked_width,
                1.0,
                foreground,
            );
            frame.text(
                marked_text,
                x,
                y + metrics.font_size * 1.27,
                foreground,
                metrics.font_size,
                false,
            );
        }
    }
    draw_completion_popup(frame, editor, foreground, dark);
}

fn draw_completion_popup(frame: &mut EditorFrame, editor: &Editor, foreground: Rgba, dark: bool) {
    let width = f32::from(frame.size.width);
    let height = f32::from(frame.size.height);
    if editor.completion_items.is_empty() {
        return;
    }
    let metrics = editor.metrics;
    let visual_line = editor
        .document
        .visual_line_for_offset(editor.selection.focus);
    let column = editor
        .document
        .visual_column_for_offset(editor.selection.focus);
    let caret_x = metrics.gutter_width + metrics.text_inset + column as f32 * metrics.char_width
        - editor.viewport.scroll_x() as f32;
    let caret_y = metrics.text_inset + visual_line as f32 * metrics.line_height
        - editor.viewport.scroll_y() as f32;
    let visible_count = editor.completion_items.len().min(8);
    let first_visible = editor
        .completion_selected
        .saturating_add(1)
        .saturating_sub(visible_count);
    let row_height = metrics.line_height.max(22.0);
    let popup_height = visible_count as f32 * row_height + 8.0;
    let longest = editor
        .completion_items
        .iter()
        .skip(first_visible)
        .take(visible_count)
        .map(|item| item.chars().count())
        .max()
        .unwrap_or(0);
    let popup_width =
        (longest as f32 * metrics.char_width + 28.0).clamp(180.0, (width - 16.0).max(180.0));
    let x = caret_x.clamp(8.0, (width - popup_width - 8.0).max(8.0));
    let below = caret_y + metrics.line_height + 4.0;
    let y = if below + popup_height <= height - 8.0 {
        below
    } else {
        (caret_y - popup_height - 4.0).max(8.0)
    };
    frame.quad(
        x,
        y,
        popup_width,
        popup_height,
        9.0,
        if dark {
            rgb(30, 30, 33)
        } else {
            rgb(245, 245, 245)
        },
    );
    for (row, item) in editor
        .completion_items
        .iter()
        .skip(first_visible)
        .take(visible_count)
        .enumerate()
    {
        let row_y = y + 4.0 + row as f32 * row_height;
        if first_visible + row == editor.completion_selected {
            fill_rect(
                frame,
                x + 4.0,
                row_y,
                popup_width - 8.0,
                row_height,
                rgba(0.12, 0.46, 0.84, 0.92),
            );
        }
        frame.text(
            item,
            x + 12.0,
            row_y + metrics.font_size * 1.27,
            if row + first_visible == editor.completion_selected {
                rgb(255, 255, 255)
            } else {
                foreground
            },
            metrics.font_size,
            false,
        );
    }
}

fn draw_fold_control(frame: &mut EditorFrame, y: f32, metrics: EditorMetrics, expanded: bool) {
    let center_y = y + metrics.line_height * 0.5;
    let vertices = if expanded {
        [
            (9.0, center_y - 2.5),
            (17.0, center_y - 2.5),
            (13.0, center_y + 3.5),
        ]
    } else {
        [
            (10.5, center_y - 4.0),
            (16.5, center_y),
            (10.5, center_y + 4.0),
        ]
    };
    frame
        .commands
        .push(Paint::Triangle(vertices, rgb(145, 145, 145)));
}

fn draw_diagnostics_line(
    frame: &mut EditorFrame,
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
            TextDiagnosticKind::Error => rgb(224, 27, 36),
            TextDiagnosticKind::Warning | TextDiagnosticKind::Spelling => rgb(246, 211, 45),
        };
        frame.underline(x, y - 1.0, width, color);
    }
}

fn draw_syntax_line(
    frame: &mut EditorFrame,
    document: &EditorDocument,
    line_start: usize,
    line_end: usize,
    mut x: f32,
    baseline: f32,
    metrics: EditorMetrics,
    foreground: Rgba,
    dark: bool,
) {
    let mut cursor = line_start;
    let first_span = document
        .syntax()
        .partition_point(|span| span.end <= line_start);
    for span in &document.syntax()[first_span..] {
        if span.start >= line_end {
            break;
        }
        let start = span.start.max(line_start).max(cursor);
        let end = span.end.min(line_end);
        if cursor < start {
            let plain = &document.text()[cursor..start];
            frame.text(plain, x, baseline, foreground, metrics.font_size, false);
            x += text_advance(plain, metrics.char_width);
        }
        if start < end {
            let segment = &document.text()[start..end];
            frame.text(
                segment,
                x,
                baseline,
                syntax_color(span, dark),
                metrics.font_size,
                false,
            );
            x += text_advance(segment, metrics.char_width);
            cursor = end;
        }
    }
    if cursor < line_end {
        frame.text(
            &document.text()[cursor..line_end],
            x,
            baseline,
            foreground,
            metrics.font_size,
            false,
        );
    }
}

fn syntax_color(span: &TextSyntaxSpan, dark: bool) -> Rgba {
    let [r, g, b] = span.color;
    let max = r.max(g).max(b);
    let scale = if !dark && max > 0.56 { 0.56 / max } else { 1.0 };
    rgba(r * scale, g * scale, b * scale, 1.0)
}

fn text_advance(text: &str, char_width: f32) -> f32 {
    craic_text_layout::text_columns(text) as f32 * char_width
}

fn fill_rect(frame: &mut EditorFrame, x: f32, y: f32, width: f32, height: f32, color: Rgba) {
    frame.quad(x, y, width, height, 0.0, color);
}

const fn rgb(red: u8, green: u8, blue: u8) -> Rgba {
    rgba(
        red as f32 / 255.0,
        green as f32 / 255.0,
        blue as f32 / 255.0,
        1.0,
    )
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}
