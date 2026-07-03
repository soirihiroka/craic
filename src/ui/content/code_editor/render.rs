use super::super::canvas_overshoot;
mod highlights;
mod line_numbers;
mod theme;

use self::highlights::{
    draw_highlighted_slice, draw_markdown_lint_issues, draw_spellcheck_issues, draw_syntax_issues,
};
use self::line_numbers::{
    LineNumberStyle, draw_deleted_hint, draw_gutter, draw_line_number, gutter_width_for_state,
    gutter_x, text_bounds, viewport_width_for_state,
};
use self::theme::{Color, EditorTheme, editor_theme, lerp_color};
use super::{
    CELL_PADDING, EditorState, FoldControlKey, FoldIconState, FoldRange, LayoutCache,
    MIN_CONTENT_WIDTH, ScrollbarMarkerKind, SearchMatch, VisualLine, canvas,
};
use crate::ui::{canvas_scroll, canvas_scrollbar};
use adw::prelude::*;
use gtk::cairo;
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use std::cell::RefCell;
use std::io::Cursor;
use std::rc::Rc;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

const FOLD_CONTROL_SIZE: f64 = 14.0;
const FOLD_ICON: &[u8] = include_bytes!("../../../assets/pan-down-symbolic.svg");
const FOLD_ICON_COLLAPSED_ANGLE: f64 = -std::f64::consts::FRAC_PI_2;
const FOLD_ICON_EXPANDED_ANGLE: f64 = 0.0;
const FOLD_ICON_STATE_LIMIT: usize = 512;

thread_local! {
    static FOLD_ICON_BASE_PIXBUF: RefCell<Option<Pixbuf>> = RefCell::new(None);
}

pub(super) type FontMetrics = canvas::FontMetrics;

#[derive(Clone, Copy)]
pub(super) enum FoldAction {
    Toggle {
        index: usize,
        start_line: usize,
        end_line: usize,
    },
}

#[derive(Clone, Copy)]
struct FoldControlHit {
    key: FoldControlKey,
    action: FoldAction,
}

pub(super) fn draw_editor(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<EditorState>,
) {
    let _ = context.save();
    refresh_font_metrics(area, state);
    let theme = editor_theme(area);
    let line_height = line_height(state);
    let char_width = char_width(state);
    fill_rect(
        context,
        0.0,
        0.0,
        width as f64,
        height as f64,
        theme.background,
    );

    let text = state.text.borrow();
    let viewport_height = height.max(1) as f64;
    let viewport_width = viewport_width_for_state(state, width);
    ensure_layout(area, state, viewport_width, viewport_height, &text);
    ensure_highlights(area, state, &text);
    let (clip_left, clip_top, clip_right, clip_bottom) =
        context
            .clip_extents()
            .unwrap_or((0.0, 0.0, width as f64, height as f64));
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        return;
    };
    let highlights = state.highlight_cache.borrow();
    let syntax_issues = state.syntax_issues.borrow();
    let markdown_lint_issues = state.markdown_lint_issues.borrow();
    let spellcheck_issues = state.spellcheck_issues.borrow();
    let visual_lines = &layout.visual_lines;
    let gutter = layout.gutter_width;
    let gutter_side = state.gutter_side.get();
    let gutter_x = gutter_x(gutter_side, width, gutter);
    let (text_left, text_right) = text_bounds(gutter_side, width, gutter);
    let text_x = text_left - state.scroll_x.get();
    let added_lines = state.git_added_lines.borrow();
    let deleted_hints = state.git_deleted_hint_counts.borrow();
    let selection = super::selection_bounds(state);
    let scroll_y = state.scroll_y.get();

    draw_gutter(
        context,
        gutter_x,
        gutter,
        height as f64,
        theme.gutter_background,
    );

    let first_line = ((clip_top + scroll_y).max(0.0) / line_height).floor() as usize;
    let last_line = (((clip_bottom + scroll_y).max(0.0) / line_height).ceil() as usize + 1)
        .min(visual_lines.len());

    for index in first_line.min(visual_lines.len())..last_line {
        let line = &visual_lines[index];
        let y = index as f64 * line_height - scroll_y;
        let baseline = y + baseline_offset(state);
        if line.wrap_index == 0 {
            let is_added = added_lines.get(line.source_line).copied().unwrap_or(false);
            let deleted_count = deleted_hints.get(line.source_line).copied().unwrap_or(0);
            let fold_control = line
                .folded
                .or_else(|| fold_index_starting_at(state, line.source_line))
                .and_then(|index| {
                    state
                        .folds
                        .borrow()
                        .get(index)
                        .map(|fold| (FoldControlKey::editor(index), fold.expanded))
                });
            draw_line_number(
                area,
                state,
                context,
                gutter_x,
                gutter,
                line.source_line + 1,
                true,
                y,
                baseline,
                LineNumberStyle {
                    added: is_added,
                    deleted: false,
                    missing: false,
                    fold_control,
                },
                theme,
            );
            if deleted_count > 0 {
                draw_deleted_hint(context, gutter_x, gutter, y, baseline, deleted_count, theme);
            }
        }
        if let Some(fold_index) = line.folded {
            if let Some(fold) = state.folds.borrow().get(fold_index).copied() {
                let label = fold_label(fold);
                draw_plain_text(
                    area,
                    context,
                    state,
                    &label,
                    text_x,
                    baseline,
                    theme.folded_text,
                );
            }
            continue;
        }
        draw_search_matches(
            area,
            context,
            state,
            &text,
            line,
            text_x,
            text_right,
            y,
            line_height,
            theme,
        );
        if let Some((selection_start, selection_end)) = selection {
            let start = selection_start.max(line.start);
            let end = selection_end.min(line.end);
            let newline_selected = line.end < text.len()
                && text.as_bytes().get(line.end) == Some(&b'\n')
                && selection_start <= line.end
                && selection_end > line.end;
            if start < end || newline_selected {
                let selected_x = text_x + text_width(area, state, &text[line.start..start]);
                let selected_width = text_width(area, state, &text[start..end])
                    + if newline_selected {
                        char_width * 0.5
                    } else {
                        0.0
                    };
                fill_rect(
                    context,
                    selected_x,
                    y,
                    selected_width
                        .max(char_width * 0.5)
                        .min((text_right - text_x).max(0.0)),
                    line_height,
                    theme.selection,
                );
            }
        }
        if selection.is_none() {
            draw_empty_selection_marker(context, state, line, text_x, y, theme.selection);
        }
        draw_highlighted_slice(
            area,
            context,
            state,
            &text,
            &highlights,
            line.start,
            line.end,
            text_x,
            baseline,
            text_right.min(clip_right),
            text_left.max(clip_left),
        );
        draw_spellcheck_issues(
            area,
            context,
            state,
            &text,
            &spellcheck_issues,
            line.start,
            line.end,
            text_x,
            baseline,
            theme,
        );
        draw_markdown_lint_issues(
            area,
            context,
            state,
            &text,
            &markdown_lint_issues,
            line.start,
            line.end,
            text_x,
            baseline,
            theme,
        );
        draw_syntax_issues(
            area,
            context,
            state,
            &text,
            &syntax_issues,
            line.start,
            line.end,
            text_x,
            baseline,
            theme,
        );
    }

    draw_preedit(
        area,
        context,
        state,
        &text,
        visual_lines,
        text_x,
        scroll_y,
        theme,
    );

    if area.has_focus()
        && state.editable.get()
        && state.cursor_visible.get()
        && state.preedit.borrow().is_empty()
    {
        draw_cursor(
            area,
            context,
            state,
            &text,
            visual_lines,
            text_x,
            scroll_y,
            theme,
        );
    }

    draw_scrollbar(area, context, width, height, state);
    canvas_overshoot::draw(context, width, height, &state.overshoot);
    draw_middle_autoscroll_marker(context, width, height, state, theme);
    let _ = context.restore();
}

pub(super) fn invalidate_layout(state: &Rc<EditorState>) {
    state.layout_cache.borrow_mut().take();
    state.layout_dirty.set(true);
}

pub(super) fn invalidate_highlights(state: &Rc<EditorState>) {
    state.highlights_dirty.set(true);
}

pub(super) fn refresh_font_metrics(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let metrics = measure_font_metrics(area, state.font_size.get());
    if (metrics.char_width - state.char_width.get()).abs() > 0.01
        || (metrics.char_spacing - state.char_spacing.get()).abs() > 0.01
        || (metrics.line_height - state.line_height.get()).abs() > 0.01
        || (metrics.baseline_offset - state.baseline_offset.get()).abs() > 0.01
    {
        state.char_width.set(metrics.char_width);
        state.char_spacing.set(metrics.char_spacing);
        state.line_height.set(metrics.line_height);
        state.baseline_offset.set(metrics.baseline_offset);
        invalidate_layout(state);
    }
}

pub(super) fn measure_font_metrics(area: &gtk::DrawingArea, font_size: f64) -> FontMetrics {
    canvas::measure_font_metrics(area, font_size, super::line_height_for_font_size)
}

fn font_size_key(font_size: f64) -> i32 {
    canvas::font_size_key(font_size)
}

fn ensure_layout(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    viewport_width: i32,
    viewport_height: f64,
    text: &str,
) {
    let wrap = state.wrap.get();
    let font_size = font_size_key(state.font_size.get());
    let fold_generation = state.fold_generation.get();
    let needs_rebuild = state.layout_dirty.get()
        || match state.layout_cache.borrow().as_ref() {
            Some(cache) => {
                cache.viewport_width != viewport_width
                    || cache.font_size != font_size
                    || cache.text_len != text.len()
                    || cache.fold_generation != fold_generation
                    || cache.gutter_width != gutter_width_for_state(area, state, line_count(text))
                    || cache.wrap != wrap
            }
            None => true,
        };

    if needs_rebuild {
        let line_count = line_count(text);
        let gutter_width = gutter_width_for_state(area, state, line_count);
        let visual_lines = build_visual_lines(area, state, viewport_width, text, gutter_width);
        let line_height = line_height(state);
        let content_height = visual_lines.len().max(1) as f64 * line_height;
        let content_width =
            content_width_for(area, state, viewport_width, text, gutter_width) as f64;
        state.layout_cache.replace(Some(LayoutCache {
            viewport_width,
            font_size,
            text_len: text.len(),
            fold_generation,
            wrap,
            gutter_width,
            content_width: content_width.max(viewport_width as f64),
            content_height: content_height.max(line_height),
            visual_lines,
        }));
        state.layout_dirty.set(false);
    }

    let cache = state.layout_cache.borrow();
    let Some(cache) = cache.as_ref() else {
        return;
    };
    state.content_height.set(cache.content_height);
    state.content_width.set(cache.content_width);
    let max_y = max_scroll_y(state, viewport_height);
    state.scroll_y.set(state.scroll_y.get().clamp(0.0, max_y));
    let max_x = (cache.content_width - viewport_width as f64).max(0.0);
    state.scroll_x.set(state.scroll_x.get().clamp(0.0, max_x));
}

fn ensure_highlights(area: &gtk::DrawingArea, state: &Rc<EditorState>, text: &str) {
    if !state.highlights_dirty.get() {
        return;
    }
    super::schedule_highlights(area, state, text);
}

fn draw_cursor(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    text: &str,
    visual_lines: &[VisualLine],
    text_x: f64,
    scroll_y: f64,
    theme: EditorTheme,
) {
    let Some((x, y, width, height)) =
        cursor_visual_rect(area, state, text, visual_lines, text_x, scroll_y)
    else {
        return;
    };
    fill_rect(context, x, y, width, height, theme.cursor);
}

fn draw_preedit(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    text: &str,
    visual_lines: &[VisualLine],
    text_x: f64,
    scroll_y: f64,
    theme: EditorTheme,
) {
    let preedit = state.preedit.borrow();
    if preedit.is_empty() {
        return;
    }
    let Some((x, y, _, _)) = cursor_visual_rect(area, state, text, visual_lines, text_x, scroll_y)
    else {
        return;
    };
    let line_index = visual_line_index_for_offset(visual_lines, state.cursor.get().min(text.len()));
    let baseline = line_index as f64 * line_height(state) - scroll_y + baseline_offset(state);
    let width = text_width(area, state, &preedit);
    draw_plain_text(
        area,
        context,
        state,
        &preedit,
        x,
        baseline,
        theme.foreground,
    );
    fill_rect(
        context,
        x,
        y + line_height(state) - 4.0,
        width.max(char_width(state) * 0.5),
        1.0,
        theme.cursor,
    );
}

fn cursor_visual_rect(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    text: &str,
    visual_lines: &[VisualLine],
    text_x: f64,
    scroll_y: f64,
) -> Option<(f64, f64, f64, f64)> {
    let cursor = state.cursor.get().min(text.len());
    let line_index = visual_line_index_for_offset(visual_lines, cursor);
    let line = visual_lines.get(line_index)?;

    let cursor = cursor.clamp(line.start, line.end);
    let line_height = line_height(state);
    Some((
        text_x + text_width(area, state, &text[line.start..cursor]),
        line_index as f64 * line_height - scroll_y + 3.0,
        1.5,
        line_height - 6.0,
    ))
}

pub(super) fn cursor_rect(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
) -> Option<(f64, f64, f64, f64)> {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let layout = layout.as_ref()?;
    let gutter = layout.gutter_width;
    let (text_left, _) = text_bounds(state.gutter_side.get(), area.allocated_width(), gutter);
    let text_x = text_left - state.scroll_x.get();
    cursor_visual_rect(
        area,
        state,
        &text,
        &layout.visual_lines,
        text_x,
        state.scroll_y.get(),
    )
}

pub(super) fn vertical_cursor_target(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    delta: isize,
) -> Option<usize> {
    if delta == 0 {
        return Some(state.cursor.get().min(state.text.borrow().len()));
    }

    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let layout = layout.as_ref()?;
    if layout.visual_lines.is_empty() {
        return None;
    }

    let cursor = state.cursor.get().min(text.len());
    let current_index = visual_line_index_for_offset(&layout.visual_lines, cursor);
    let current_line = layout.visual_lines.get(current_index)?;
    let cursor = cursor.clamp(current_line.start, current_line.end);
    let line_x = text_width(area, state, &text[current_line.start..cursor]);

    let target_index = if delta < 0 {
        current_index.saturating_sub(delta.unsigned_abs())
    } else {
        current_index
            .saturating_add(delta as usize)
            .min(layout.visual_lines.len().saturating_sub(1))
    };
    if target_index == current_index {
        return None;
    }

    let target_line = layout.visual_lines.get(target_index)?;
    if target_line.folded.is_some() {
        return Some(target_line.start);
    }

    Some(
        target_line.start
            + offset_for_x(
                area,
                state,
                &text[target_line.start..target_line.end],
                line_x,
            ),
    )
}

fn draw_empty_selection_marker(
    context: &cairo::Context,
    state: &Rc<EditorState>,
    line: &VisualLine,
    text_x: f64,
    y: f64,
    color: Color,
) {
    let Some(selection) = *state.selection.borrow() else {
        return;
    };
    if selection.anchor != selection.focus
        || selection.anchor != line.start
        || line.start != line.end
    {
        return;
    }
    fill_rect(
        context,
        text_x,
        y,
        char_width(state) * 0.5,
        line_height(state),
        color,
    );
}

fn draw_search_matches(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    source: &str,
    line: &VisualLine,
    text_x: f64,
    text_right: f64,
    y: f64,
    line_height: f64,
    theme: EditorTheme,
) {
    let search = state.search.borrow();
    if search.matches.is_empty() {
        return;
    }

    let first_match = search
        .matches
        .partition_point(|search_match| search_match.end <= line.start);
    for (index, search_match) in search.matches[first_match..].iter().enumerate() {
        if search_match.start >= line.end {
            break;
        }
        if !valid_search_match(source, search_match) {
            continue;
        }

        let start = search_match.start.max(line.start);
        let end = search_match.end.min(line.end);
        if start >= end {
            continue;
        }

        let match_x = text_x + text_width(area, state, &source[line.start..start]);
        let match_width = text_width(area, state, &source[start..end]).max(char_width(state) * 0.5);
        let visible_width = match_width.min((text_right - match_x).max(0.0));
        if visible_width <= 0.0 {
            continue;
        }

        let actual_index = first_match + index;
        fill_rect(
            context,
            match_x,
            y + 2.0,
            visible_width,
            line_height - 4.0,
            if search.active == Some(actual_index) {
                theme.search_match_current
            } else {
                theme.search_match
            },
        );
    }
}

fn valid_search_match(source: &str, search_match: &SearchMatch) -> bool {
    search_match.start < search_match.end
        && search_match.end <= source.len()
        && source.is_char_boundary(search_match.start)
        && source.is_char_boundary(search_match.end)
}

fn build_visual_lines(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    width: i32,
    text: &str,
    gutter_width: f64,
) -> Vec<VisualLine> {
    let wrap_width = wrap_width(state, width, gutter_width);
    let mut lines = Vec::new();
    let mut source_line = 0;
    let folds = state.folds.borrow();
    let line_ranges = logical_line_ranges(text);

    while source_line < line_ranges.len() {
        let (line_start, line_end) = line_ranges[source_line];
        if let Some(fold_index) = collapsed_fold_starting_at(&folds, source_line) {
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
        push_wrapped_visual_lines(
            area,
            state,
            &mut lines,
            source_line,
            line_start,
            line_end,
            text,
            wrap_width,
        );
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

fn logical_line_ranges(text: &str) -> Vec<(usize, usize)> {
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

fn push_wrapped_visual_lines(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    lines: &mut Vec<VisualLine>,
    source_line: usize,
    start: usize,
    end: usize,
    text: &str,
    wrap_width: f64,
) -> usize {
    if start == end {
        lines.push(VisualLine {
            source_line,
            start,
            end,
            wrap_index: 0,
            folded: None,
        });
        return 1;
    }
    if !state.wrap.get() {
        lines.push(VisualLine {
            source_line,
            start,
            end,
            wrap_index: 0,
            folded: None,
        });
        return 1;
    }

    let wrap_width = wrap_width.max(char_width(state));
    let mut segment_start = start;
    let mut line_width = 0.0;
    let mut wrap_index = 0;
    for (byte, grapheme) in text[start..end].grapheme_indices(true) {
        let grapheme_start = start + byte;
        let grapheme_width = text_width(area, state, grapheme);
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
    wrap_index + 1
}

pub(super) fn refresh_size(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    width: i32,
    height: i32,
) {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, width);
    let viewport_height = height.max(1) as f64;
    ensure_layout(area, state, viewport_width, viewport_height, &text);
}

pub(super) fn hit_test(area: &gtk::DrawingArea, state: &Rc<EditorState>, x: f64, y: f64) -> usize {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        return text.len();
    };
    let document_y = y + state.scroll_y.get();
    let line_height = line_height(state);
    let line_index = ((document_y.max(0.0) / line_height).floor() as usize)
        .min(layout.visual_lines.len().saturating_sub(1));
    let Some(line) = layout.visual_lines.get(line_index) else {
        return text.len();
    };
    if line.folded.is_some() {
        return line.start;
    }
    let gutter = layout.gutter_width;
    let (text_left, _) = text_bounds(state.gutter_side.get(), area.allocated_width(), gutter);
    let text_x = text_left - state.scroll_x.get();
    line.start + offset_for_x(area, state, &text[line.start..line.end], x - text_x)
}

pub(super) fn text_range_at_point(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    x: f64,
    y: f64,
) -> Option<(usize, usize)> {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let layout = layout.as_ref()?;
    let document_y = y + state.scroll_y.get();
    if document_y < 0.0 {
        return None;
    }
    let line_height = line_height(state);
    let line_index = (document_y / line_height).floor() as usize;
    let line = layout.visual_lines.get(line_index)?;
    if line.folded.is_some() || line.start >= line.end {
        return None;
    }
    let gutter = layout.gutter_width;
    let (text_left, _) = text_bounds(state.gutter_side.get(), area.allocated_width(), gutter);
    let mut text_x = text_left - state.scroll_x.get();
    for (relative_start, grapheme) in text[line.start..line.end].grapheme_indices(true) {
        let grapheme_start = line.start + relative_start;
        let grapheme_end = grapheme_start + grapheme.len();
        let next_x = text_x + text_width(area, state, grapheme);
        if x >= text_x && x < next_x {
            return Some((grapheme_start, grapheme_end));
        }
        text_x = next_x;
    }
    None
}

pub(super) fn set_scroll_y(area: &gtk::DrawingArea, state: &Rc<EditorState>, value: f64) {
    let viewport_height = area.allocated_height().max(1) as f64;
    let max_scroll = max_scroll_y(state, viewport_height);
    let next = value.clamp(0.0, max_scroll);
    if (next - state.scroll_y.get()).abs() > f64::EPSILON {
        state.scroll_y.set(next);
        super::notify_scroll(state, next);
        area.queue_draw();
    }
}

pub(super) fn set_scroll_x(area: &gtk::DrawingArea, state: &Rc<EditorState>, value: f64) {
    let viewport_width = viewport_width_for_state(state, area.allocated_width()) as f64;
    let max_scroll = (state.content_width.get() - viewport_width).max(0.0);
    let next = value.clamp(0.0, max_scroll);
    if (next - state.scroll_x.get()).abs() > f64::EPSILON {
        state.scroll_x.set(next);
        area.queue_draw();
    }
}

pub(super) fn ensure_offset_visible(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    offset: usize,
) {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        return;
    };
    if layout.visual_lines.is_empty() {
        return;
    }
    let line_index = visual_line_index_for_offset(&layout.visual_lines, offset);

    let viewport_height = area.allocated_height().max(1) as f64;
    let line_height = line_height(state);
    let y = line_index as f64 * line_height;
    if y < state.scroll_y.get() {
        set_scroll_y(area, state, y);
    } else if y + line_height > state.scroll_y.get() + viewport_height {
        set_scroll_y(area, state, y + line_height - viewport_height);
    }

    if !state.wrap.get() {
        let Some(line) = layout.visual_lines.get(line_index) else {
            return;
        };
        let gutter = layout.gutter_width;
        let (text_left, _) = text_bounds(state.gutter_side.get(), area.allocated_width(), gutter);
        let offset = offset.clamp(line.start, line.end);
        let x = text_width(area, state, &text[line.start..offset]) + text_left;
        let viewport_width = viewport_width as f64;
        if x < state.scroll_x.get() + gutter {
            set_scroll_x(area, state, x - gutter);
        } else if x + char_width(state) > state.scroll_x.get() + viewport_width {
            set_scroll_x(area, state, x + char_width(state) - viewport_width);
        }
    }
}

fn visual_line_index_for_offset(visual_lines: &[VisualLine], offset: usize) -> usize {
    for (index, line) in visual_lines.iter().enumerate() {
        if offset >= line.start && offset <= line.end {
            return index;
        }

        if line.folded.is_some() && offset > line.end {
            let next_start = visual_lines
                .get(index + 1)
                .map(|next| next.start)
                .unwrap_or(usize::MAX);
            if offset < next_start {
                return index;
            }
        }
    }

    visual_lines
        .partition_point(|line| line.end < offset)
        .min(visual_lines.len().saturating_sub(1))
}

pub(super) fn scrollbar_thumb(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
) -> Option<(f64, f64, f64, f64)> {
    let width = area.allocated_width();
    let height = area.allocated_height();
    if !state.scrollbar_visible.get() {
        return None;
    }
    canvas_scrollbar::thumb_rect(
        width,
        height,
        state.content_height.get(),
        state.scroll_y.get(),
    )
}

pub(super) fn max_scroll_y(state: &Rc<EditorState>, viewport_height: f64) -> f64 {
    (state.content_height.get() - viewport_height).max(0.0)
}

pub(super) fn source_offset_at_scroll_top(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
) -> usize {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        return 0;
    };
    if layout.visual_lines.is_empty() {
        return 0;
    }

    let line_height = line_height(state);
    let visual_position = state.scroll_y.get().max(0.0) / line_height;
    let line_index =
        (visual_position.floor() as usize).min(layout.visual_lines.len().saturating_sub(1));
    let Some(line) = layout.visual_lines.get(line_index) else {
        return 0;
    };

    let next_start = layout
        .visual_lines
        .get(line_index + 1)
        .map(|next| next.start)
        .unwrap_or(text.len());
    let source_span = next_start.saturating_sub(line.start);
    if source_span == 0 {
        return line.start;
    }

    let progress = (visual_position - line_index as f64).clamp(0.0, 1.0);
    line.start + (source_span as f64 * progress).round() as usize
}

pub(super) fn set_source_offset_at_scroll_top(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    offset: usize,
) {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        return;
    };
    if layout.visual_lines.is_empty() {
        return;
    }

    let offset = super::clamp_to_char_boundary(&text, offset);
    let line_index = visual_line_index_for_offset(&layout.visual_lines, offset);
    let Some(line) = layout.visual_lines.get(line_index) else {
        return;
    };
    let next_start = layout
        .visual_lines
        .get(line_index + 1)
        .map(|next| next.start)
        .unwrap_or(text.len());
    let source_span = next_start.saturating_sub(line.start);
    let progress = if source_span == 0 {
        0.0
    } else {
        (offset.saturating_sub(line.start) as f64 / source_span as f64).clamp(0.0, 1.0)
    };
    let scroll_y = (line_index as f64 + progress) * line_height(state);
    let _ = layout;
    drop(text);
    set_scroll_y(area, state, scroll_y);
}

pub(super) fn fold_action_at_point(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    x: f64,
    y: f64,
) -> Option<FoldAction> {
    fold_control_hit_at_point(area, state, x, y).map(|hit| hit.action)
}

pub(super) fn fold_control_at_point(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    x: f64,
    y: f64,
) -> Option<FoldControlKey> {
    fold_control_hit_at_point(area, state, x, y).map(|hit| hit.key)
}

fn fold_control_hit_at_point(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    x: f64,
    y: f64,
) -> Option<FoldControlHit> {
    let text = state.text.borrow();
    let viewport_width = viewport_width_for_state(state, area.allocated_width());
    ensure_layout(
        area,
        state,
        viewport_width,
        area.allocated_height().max(1) as f64,
        &text,
    );
    let layout = state.layout_cache.borrow();
    let layout = layout.as_ref()?;
    let document_y = y + state.scroll_y.get();
    let line_height = line_height(state);
    let line_index = ((document_y.max(0.0) / line_height).floor() as usize)
        .min(layout.visual_lines.len().saturating_sub(1));
    let line = layout.visual_lines.get(line_index)?;
    if line.wrap_index != 0 {
        return None;
    }

    let gutter = layout.gutter_width;
    let gutter_x = gutter_x(state.gutter_side.get(), area.allocated_width(), gutter);
    let row_y = line_index as f64 * line_height - state.scroll_y.get();
    let fold_index = line
        .folded
        .or_else(|| fold_index_starting_at(state, line.source_line))?;
    let fold = state.folds.borrow().get(fold_index).copied()?;
    if fold.start_line != line.source_line || fold.end_line <= fold.start_line {
        return None;
    }
    point_in_rect(x, y, fold_toggle_rect(gutter_x, gutter, row_y, line_height)).then_some(
        FoldControlHit {
            key: FoldControlKey::editor(fold_index),
            action: FoldAction::Toggle {
                index: fold_index,
                start_line: fold.start_line,
                end_line: fold.end_line,
            },
        },
    )
}

fn draw_scrollbar(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<EditorState>,
) {
    if !state.scrollbar_visible.get() {
        return;
    }
    let hover = state.scrollbar_hover_progress.get().clamp(0.0, 1.0);
    let active = state.scrollbar_active.get();
    let theme = canvas_scrollbar::Theme::for_widget(area);
    canvas_scrollbar::draw_track(
        context,
        width,
        height,
        state.content_height.get(),
        hover,
        theme,
    );
    canvas_scrollbar::draw_thumb_fill(
        context,
        width,
        height,
        state.content_height.get(),
        state.scroll_y.get(),
        hover,
        active,
        theme,
    );
    draw_scrollbar_markers(context, width, height, state, hover);
    canvas_scrollbar::draw_thumb_outline(
        context,
        width,
        height,
        state.content_height.get(),
        state.scroll_y.get(),
        hover,
        active,
        theme,
    );
}

fn draw_scrollbar_markers(
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<EditorState>,
    hover: f64,
) {
    let markers = state.scrollbar_markers.borrow();
    if markers.is_empty() || state.content_height.get() <= 0.0 {
        return;
    }

    let _ = context.save();
    canvas_scrollbar::clip_to_track(context, width, height, hover);
    let (track_x, track_y, track_width, track_height) =
        canvas_scrollbar::visual_track_rect(width, height, hover);
    let layout = state.layout_cache.borrow();
    let Some(layout) = layout.as_ref() else {
        let _ = context.restore();
        return;
    };
    let total_height = state.content_height.get();

    for marker in markers.iter() {
        let line_height = line_height(state);
        let Some((first_visual_line, visual_line_count)) =
            scrollbar_marker_visual_span(&layout.visual_lines, marker.row)
        else {
            continue;
        };
        let marker_y =
            track_y + ((first_visual_line as f64 * line_height) / total_height) * track_height;
        let marker_height = ((visual_line_count as f64 * line_height) / total_height
            * track_height)
            .max(2.0)
            .min(track_y + track_height - marker_y);
        canvas_scrollbar::draw_marker(
            context,
            match marker.kind {
                ScrollbarMarkerKind::Added => canvas_scrollbar::MarkerKind::Added,
                ScrollbarMarkerKind::Deleted => canvas_scrollbar::MarkerKind::Deleted,
                ScrollbarMarkerKind::Mixed => canvas_scrollbar::MarkerKind::Mixed,
            },
            track_x,
            marker_y,
            track_width,
            marker_height,
            hover,
        );
    }

    let _ = context.restore();
}

fn draw_middle_autoscroll_marker(
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<EditorState>,
    theme: EditorTheme,
) {
    let foreground = theme.foreground.with_alpha(0.82);
    let background = theme.background.with_alpha(0.92);
    let shadow = Color::rgb(0.0, 0.0, 0.0).with_alpha(0.34);
    canvas_scroll::draw_middle_autoscroll_marker(
        context,
        width,
        height,
        state.middle_autoscroll.state(),
        canvas_scroll::AutoscrollAxes::Vertical,
        canvas_scroll::MarkerStyle {
            foreground: canvas_scroll::MarkerColor::rgba(
                foreground.red,
                foreground.green,
                foreground.blue,
                foreground.alpha,
            ),
            background: canvas_scroll::MarkerColor::rgba(
                background.red,
                background.green,
                background.blue,
                background.alpha,
            ),
            shadow: canvas_scroll::MarkerColor::rgba(
                shadow.red,
                shadow.green,
                shadow.blue,
                shadow.alpha,
            ),
        },
    );
}

fn scrollbar_marker_visual_span(
    visual_lines: &[VisualLine],
    source_line: usize,
) -> Option<(usize, usize)> {
    let first = visual_lines
        .iter()
        .position(|line| line.source_line == source_line)?;
    let count = visual_lines[first..]
        .iter()
        .take_while(|line| line.source_line == source_line)
        .count()
        .max(1);
    Some((first, count))
}

fn fold_index_starting_at(state: &Rc<EditorState>, source_line: usize) -> Option<usize> {
    state
        .folds
        .borrow()
        .iter()
        .enumerate()
        .filter(|(_, fold)| fold.start_line == source_line)
        .max_by_key(|(_, fold)| fold.end_line)
        .map(|(index, _)| index)
}

fn fold_toggle_rect(gutter_x: f64, _gutter: f64, y: f64, line_height: f64) -> (f64, f64, f64, f64) {
    (
        gutter_x + 4.0,
        y + (line_height - FOLD_CONTROL_SIZE) / 2.0,
        FOLD_CONTROL_SIZE,
        FOLD_CONTROL_SIZE,
    )
}

fn draw_fold_toggle_icon(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    context: &cairo::Context,
    rect: (f64, f64, f64, f64),
    key: FoldControlKey,
    expanded: bool,
    theme: EditorTheme,
) {
    let angle = fold_icon_angle(area, state, key, expanded);
    let (color, active_amount, pressed) = fold_icon_color(state, key, theme);
    draw_fold_control_background(context, rect, active_amount, pressed, theme);

    let Some(pixbuf) = fold_icon_pixbuf(color) else {
        return;
    };

    let _ = context.save();
    context.translate(rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
    context.rotate(angle);
    context.set_source_pixbuf(&pixbuf, -rect.2 / 2.0, -rect.3 / 2.0);
    let _ = context.paint();
    let _ = context.restore();
}

fn fold_icon_angle(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    key: FoldControlKey,
    expanded: bool,
) -> f64 {
    let target_angle = if expanded {
        FOLD_ICON_EXPANDED_ANGLE
    } else {
        FOLD_ICON_COLLAPSED_ANGLE
    };

    let mut states = state.fold_icon_states.borrow_mut();
    if let Some(icon) = states.iter_mut().find(|icon| icon.key == key) {
        if (icon.target_angle - target_angle).abs() > f64::EPSILON {
            icon.target_angle = target_angle;
            start_fold_icon_animation(area, state);
        }
        return icon.angle;
    }

    if states.len() >= FOLD_ICON_STATE_LIMIT {
        states.remove(0);
    }
    states.push(FoldIconState {
        key,
        angle: target_angle,
        target_angle,
    });
    target_angle
}

fn start_fold_icon_animation(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    if state.fold_icon_animating.get() {
        return;
    }
    state.fold_icon_animating.set(true);

    let area = area.clone();
    let state = state.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut animating = false;
        {
            let mut states = state.fold_icon_states.borrow_mut();
            for icon in states.iter_mut() {
                let delta = icon.target_angle - icon.angle;
                if delta.abs() < 0.01 {
                    icon.angle = icon.target_angle;
                } else {
                    icon.angle += delta * 0.34;
                    animating = true;
                }
            }
        }

        area.queue_draw();
        if animating {
            gtk::glib::ControlFlow::Continue
        } else {
            state.fold_icon_animating.set(false);
            gtk::glib::ControlFlow::Break
        }
    });
}

fn fold_icon_color(
    state: &Rc<EditorState>,
    key: FoldControlKey,
    theme: EditorTheme,
) -> (Color, f64, bool) {
    let hovered = state.fold_hovered.get() == Some(key);
    let pressed = state.fold_pressed.get() == Some(key);
    let active_amount = if hovered || pressed {
        state.fold_hover_progress.get().clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hover_color = lerp_color(theme.line_number, theme.line_number_emphasis, active_amount);
    let color = if pressed {
        lerp_color(hover_color, theme.foreground, 0.70)
    } else {
        hover_color
    };
    (color, active_amount, pressed)
}

fn draw_fold_control_background(
    context: &cairo::Context,
    rect: (f64, f64, f64, f64),
    active_amount: f64,
    pressed: bool,
    theme: EditorTheme,
) {
    if active_amount <= 0.01 && !pressed {
        return;
    }
    let alpha = if pressed { 0.24 } else { 0.14 } * active_amount.max(0.40);
    fill_rounded_rect_rgba(
        context,
        rect.0 - 1.0,
        rect.1 - 1.0,
        rect.2 + 2.0,
        rect.3 + 2.0,
        4.0,
        theme.fold_control_background.with_alpha(alpha),
    );
}

fn fold_icon_pixbuf(color: Color) -> Option<Pixbuf> {
    let base = FOLD_ICON_BASE_PIXBUF.with(|slot| {
        let mut cached = slot.borrow_mut();
        if cached.is_none() {
            *cached = Pixbuf::from_read(Cursor::new(FOLD_ICON))
                .ok()
                .and_then(|pixbuf| {
                    pixbuf.scale_simple(
                        FOLD_CONTROL_SIZE as i32,
                        FOLD_CONTROL_SIZE as i32,
                        gtk::gdk_pixbuf::InterpType::Bilinear,
                    )
                });
        }
        cached.clone()
    });
    base.and_then(|pixbuf| recolor_symbolic_pixbuf(pixbuf, color))
}

fn recolor_symbolic_pixbuf(pixbuf: Pixbuf, color: Color) -> Option<Pixbuf> {
    let pixbuf = if pixbuf.has_alpha() {
        pixbuf.copy()?
    } else {
        pixbuf.add_alpha(false, 0, 0, 0).ok()?
    };
    let width = pixbuf.width().max(0) as usize;
    let height = pixbuf.height().max(0) as usize;
    let rowstride = pixbuf.rowstride().max(0) as usize;
    let channels = pixbuf.n_channels().max(0) as usize;
    if width == 0 || height == 0 || channels < 4 {
        return None;
    }

    let red = (color.red.clamp(0.0, 1.0) * 255.0).round() as u8;
    let green = (color.green.clamp(0.0, 1.0) * 255.0).round() as u8;
    let blue = (color.blue.clamp(0.0, 1.0) * 255.0).round() as u8;
    let pixels = unsafe { pixbuf.pixels() };
    for y in 0..height {
        for x in 0..width {
            let offset = y
                .saturating_mul(rowstride)
                .saturating_add(x.saturating_mul(channels));
            if offset + 3 >= pixels.len() {
                continue;
            }
            pixels[offset] = red;
            pixels[offset + 1] = green;
            pixels[offset + 2] = blue;
        }
    }

    Some(pixbuf)
}

fn point_in_rect(x: f64, y: f64, rect: (f64, f64, f64, f64)) -> bool {
    let (rect_x, rect_y, rect_width, rect_height) = rect;
    x >= rect_x && x <= rect_x + rect_width && y >= rect_y && y <= rect_y + rect_height
}

fn wrap_width(state: &Rc<EditorState>, width: i32, gutter_width: f64) -> f64 {
    if !state.wrap.get() {
        return f64::MAX / 2.0;
    }
    (width as f64 - gutter_width - (CELL_PADDING * 2.0)).max(char_width(state))
}

fn content_width_for(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    width: i32,
    text: &str,
    gutter_width: f64,
) -> i32 {
    if state.wrap.get() {
        return width.max(MIN_CONTENT_WIDTH);
    }
    let longest = text
        .lines()
        .map(|line| text_width(area, state, line))
        .fold(0.0, f64::max);
    (longest + gutter_width + CELL_PADDING * 2.0)
        .ceil()
        .max(MIN_CONTENT_WIDTH as f64) as i32
}

pub(super) fn viewport_width(width: i32) -> i32 {
    canvas_scrollbar::content_width(width).max(MIN_CONTENT_WIDTH.min(width.max(1)))
}

pub(super) fn line_count(text: &str) -> usize {
    text.lines().count().max(1)
}

pub(super) fn line_for_offset(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

fn fold_label(fold: FoldRange) -> String {
    let hidden = fold.end_line.saturating_sub(fold.start_line);
    format!("+ {hidden} hidden lines")
}

pub(super) fn line_height(state: &Rc<EditorState>) -> f64 {
    state.line_height.get()
}

fn baseline_offset(state: &Rc<EditorState>) -> f64 {
    state.baseline_offset.get()
}

fn char_width(state: &Rc<EditorState>) -> f64 {
    state.char_width.get()
}

pub(super) fn text_width(area: &gtk::DrawingArea, state: &Rc<EditorState>, text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let mut cache = state.text_width_cache.borrow_mut();
    canvas::cached_text_width(area, state.font_size.get(), &mut cache, text)
}

pub(super) fn offset_for_x(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    text: &str,
    x: f64,
) -> usize {
    let x = x.max(0.0);
    if text.is_empty() || x <= 0.0 {
        return 0;
    }

    let mut width = 0.0;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = text_width(area, state, grapheme);
        if x <= width + grapheme_width / 2.0 {
            return offset;
        }
        width += grapheme_width;
    }
    text.len()
}

fn draw_plain_text(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<EditorState>,
    text: &str,
    x: f64,
    baseline: f64,
    color: impl Into<Color>,
) {
    if text.is_empty() {
        return;
    }
    let color = color.into();
    if color.alpha < 1.0 {
        context.push_group();
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            text,
            x,
            baseline,
            canvas::TextColor::rgb(color.red, color.green, color.blue),
        );
        let _ = context.pop_group_to_source();
        let _ = context.paint_with_alpha(color.alpha);
    } else {
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            text,
            x,
            baseline,
            canvas::TextColor::rgb(color.red, color.green, color.blue),
        );
    }
}

fn fill_rect(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color: impl Into<Color>,
) {
    let color = color.into();
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
    context.rectangle(x, y, width, height);
    let _ = context.fill();
}

fn fill_rounded_rect_rgba(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
    color: impl Into<Color>,
) {
    rounded_rect(context, x, y, width, height, radius);
    let color = color.into();
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
    let _ = context.fill();
}

fn rounded_rect(context: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    context.close_path();
}
