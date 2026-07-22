use super::super::super::skia_canvas;
use super::super::{CELL_PADDING, EditorState, FoldControlKey, GutterSide, MIN_CONTENT_WIDTH};
use super::theme::{Color, EditorTheme};
use super::{
    char_width, draw_fold_toggle_icon, draw_plain_text, fill_rect, fold_toggle_rect, line_height,
    text_width, viewport_width,
};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub struct LineNumberStyle {
    pub added: bool,
    pub deleted: bool,
    pub missing: bool,
    pub fold_control: Option<(FoldControlKey, bool)>,
}

pub fn draw_line_number(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    context: &skia_canvas::Context,
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
    let x = gutter_x + gutter - CELL_PADDING - text_width(area, state, &number_text);
    draw_plain_text(area, context, state, &number_text, x, baseline, color);
}

pub fn draw_gutter(
    context: &skia_canvas::Context,
    x: f64,
    width: f64,
    height: f64,
    color: impl Into<Color>,
) {
    fill_rect(context, x, 0.0, width, height, color);
}

pub fn draw_deleted_hint(
    context: &skia_canvas::Context,
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

pub fn gutter_x(side: GutterSide, _width: i32, _gutter: f64) -> f64 {
    match side {
        GutterSide::Left => 0.0,
    }
}

pub fn text_bounds(side: GutterSide, width: i32, gutter: f64) -> (f64, f64) {
    match side {
        GutterSide::Left => (gutter + CELL_PADDING, width as f64 - CELL_PADDING),
    }
}

pub fn gutter_width_for_state(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    line_count: usize,
) -> f64 {
    gutter_width_for_line_count(area, state, line_count)
}

fn gutter_width_for_line_count(
    area: &gtk::GLArea,
    state: &Rc<EditorState>,
    line_count: usize,
) -> f64 {
    text_width(area, state, &line_count.max(1).to_string()) + 4.0 * char_width(state)
}

pub fn viewport_width_for_state(state: &Rc<EditorState>, width: i32) -> i32 {
    if state.scrollbar_visible.get() {
        viewport_width(width)
    } else {
        width.max(MIN_CONTENT_WIDTH.min(width.max(1)))
    }
}
