use super::super::{
    CELL_PADDING, DIFF_PREFIX_WIDTH, EditorDiffKind, EditorState, FoldControlKey, GutterSide,
    MIN_CONTENT_WIDTH,
};
use super::theme::{Color, EditorTheme};
use super::{
    char_width, draw_fold_toggle_icon, draw_plain_text, fill_rect, fold_toggle_rect, line_height,
    text_width, viewport_width,
};
use gtk::cairo;
use std::rc::Rc;

#[derive(Clone, Copy)]
pub(super) struct LineNumberStyle {
    pub(super) added: bool,
    pub(super) deleted: bool,
    pub(super) missing: bool,
    pub(super) prefix: Option<&'static str>,
    pub(super) fold_control: Option<(FoldControlKey, bool)>,
    pub(super) show_diff_fold_control: bool,
}

pub(super) fn draw_line_number(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    context: &cairo::Context,
    gutter_x: f64,
    gutter: f64,
    number: usize,
    show_number: bool,
    y: f64,
    baseline: f64,
    style: LineNumberStyle,
    theme: EditorTheme,
) {
    let background = if style.added {
        theme.added_gutter_background
    } else if style.deleted {
        theme.deleted_gutter_background
    } else if style.missing {
        theme.background
    } else {
        theme.gutter_background
    };
    let line_height = line_height(state);
    fill_rect(context, gutter_x, y, gutter, line_height, background);

    if style.show_diff_fold_control {
        if let Some((key, expanded)) = style.fold_control {
            draw_fold_toggle_icon(
                area,
                state,
                context,
                fold_toggle_rect(gutter_x, gutter, y, line_height),
                key,
                expanded,
                theme,
            );
        }
        return;
    }

    if let Some((key, expanded)) = style.fold_control {
        draw_fold_toggle_icon(
            area,
            state,
            context,
            fold_toggle_rect(gutter_x, gutter, y, line_height),
            key,
            expanded,
            theme,
        );
    }

    if !show_number {
        return;
    }

    let number_text = number.to_string();
    let color = if style.added || style.deleted {
        theme.line_number_emphasis
    } else {
        theme.line_number
    };
    let number_area_width = if style.prefix.is_some() {
        gutter - DIFF_PREFIX_WIDTH
    } else {
        gutter
    };
    let x = gutter_x + number_area_width - CELL_PADDING - text_width(area, state, &number_text);
    draw_plain_text(area, context, state, &number_text, x, baseline, color);

    if let Some(prefix) = style.prefix {
        draw_plain_text(
            area,
            context,
            state,
            prefix,
            gutter_x + gutter - DIFF_PREFIX_WIDTH + 7.0,
            baseline,
            theme.line_number_emphasis,
        );
    }
}

pub(super) fn draw_gutter(
    context: &cairo::Context,
    x: f64,
    width: f64,
    height: f64,
    color: impl Into<Color>,
) {
    fill_rect(context, x, 0.0, width, height, color);
}

pub(super) fn draw_deleted_hint(
    context: &cairo::Context,
    gutter_x: f64,
    gutter: f64,
    line_y: f64,
    _baseline: f64,
    _count: usize,
    theme: EditorTheme,
) {
    let hint_width = (gutter - CELL_PADDING * 2.0).max(20.0);
    fill_rect(
        context,
        gutter_x + CELL_PADDING,
        line_y + 2.0,
        hint_width,
        3.0,
        theme.deleted_hint,
    );
}

pub(super) fn draw_line_background(
    context: &cairo::Context,
    side: GutterSide,
    width: i32,
    gutter: f64,
    y: f64,
    line_height: f64,
    kind: EditorDiffKind,
    theme: EditorTheme,
) {
    let color = match kind {
        EditorDiffKind::Added => Some(theme.added_background),
        EditorDiffKind::Deleted => Some(theme.deleted_background),
        EditorDiffKind::Missing | EditorDiffKind::Fold => Some(theme.background),
        EditorDiffKind::Context => None,
    };
    let Some(color) = color else {
        return;
    };
    let (x, row_width) = match side {
        GutterSide::Left => (0.0, width as f64),
        GutterSide::Right => (0.0, width as f64),
    };
    fill_rect(context, x, y, row_width.max(gutter), line_height, color);
}

pub(super) fn gutter_x(side: GutterSide, width: i32, gutter: f64) -> f64 {
    match side {
        GutterSide::Left => 0.0,
        GutterSide::Right => (width as f64 - gutter).max(0.0),
    }
}

pub(super) fn text_bounds(side: GutterSide, width: i32, gutter: f64) -> (f64, f64) {
    match side {
        GutterSide::Left => (gutter + CELL_PADDING, width as f64 - CELL_PADDING),
        GutterSide::Right => (CELL_PADDING, width as f64 - gutter - CELL_PADDING),
    }
}

pub(super) fn gutter_width_for_state(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    line_count: usize,
) -> f64 {
    let mut width = gutter_width_for_line_count(area, state, line_count);
    if state.diff_rows.borrow().is_some() {
        width += DIFF_PREFIX_WIDTH;
    }
    width
}

fn gutter_width_for_line_count(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    line_count: usize,
) -> f64 {
    text_width(area, state, &line_count.max(1).to_string()) + 4.0 * char_width(state)
}

pub(super) fn viewport_width_for_state(state: &Rc<EditorState>, width: i32) -> i32 {
    if state.scrollbar_visible.get() || state.diff_rows.borrow().is_some() {
        viewport_width(width)
    } else {
        width.max(MIN_CONTENT_WIDTH.min(width.max(1)))
    }
}

pub(super) fn diff_prefix(kind: EditorDiffKind) -> Option<&'static str> {
    match kind {
        EditorDiffKind::Added => Some("+"),
        EditorDiffKind::Deleted => Some("-"),
        EditorDiffKind::Context | EditorDiffKind::Missing | EditorDiffKind::Fold => None,
    }
}
