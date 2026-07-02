use super::code_editor::canvas::{self, TextColor};
use crate::config;
use crate::git::{DiffKind, FileDiffRow};
use crate::ui::canvas_scrollbar;
use adw::prelude::*;
use gtk::cairo;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use unicode_segmentation::UnicodeSegmentation;

const MIN_CONTENT_WIDTH: i32 = 360;
const CELL_PADDING: f64 = 8.0;
const PREFIX_WIDTH: f64 = 18.0;
const DIVIDER_WIDTH: f64 = 1.0;

#[derive(Clone)]
pub(in crate::ui) struct DiffCanvas {
    pub(in crate::ui) root: gtk::DrawingArea,
    state: Rc<DiffCanvasState>,
}

struct DiffCanvasState {
    rows: RefCell<Vec<FileDiffRow>>,
    font_size: Cell<f64>,
    scroll_y: Cell<f64>,
    content_height: Cell<f64>,
    scrollbar_hover: Rc<Cell<bool>>,
    scrollbar_active: Rc<Cell<bool>>,
    scrollbar_hover_progress: Rc<Cell<f64>>,
    scrollbar_animating: Rc<Cell<bool>>,
    scrollbar_drag: Cell<Option<canvas_scrollbar::Drag>>,
    fold_callback: RefCell<Option<Rc<dyn Fn(usize)>>>,
    font_size_adjust_callback: RefCell<Option<Rc<dyn Fn(f64)>>>,
}

#[derive(Clone)]
struct RowLayout {
    y: f64,
    height: f64,
    left_lines: Vec<String>,
    right_lines: Vec<String>,
}

#[derive(Clone, Copy)]
struct DiffCanvasTheme {
    background: Color,
    foreground: Color,
    muted: Color,
    gutter: Color,
    divider: Color,
    added_background: Color,
    added_text: Color,
    deleted_background: Color,
    deleted_text: Color,
    missing_background: Color,
    fold_background: Color,
    fold_text: Color,
}

#[derive(Clone, Copy)]
struct Color {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

impl Color {
    const fn rgba(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    fn text(self) -> TextColor {
        TextColor::rgb(self.red, self.green, self.blue)
    }
}

impl DiffCanvas {
    pub(in crate::ui) fn new() -> Self {
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .focusable(true)
            .build();
        area.set_size_request(MIN_CONTENT_WIDTH, 160);

        let font_size = config::load().font_sizes.diff;
        let state = Rc::new(DiffCanvasState {
            rows: RefCell::new(Vec::new()),
            font_size: Cell::new(font_size),
            scroll_y: Cell::new(0.0),
            content_height: Cell::new(1.0),
            scrollbar_hover: Rc::new(Cell::new(false)),
            scrollbar_active: Rc::new(Cell::new(false)),
            scrollbar_hover_progress: Rc::new(Cell::new(0.0)),
            scrollbar_animating: Rc::new(Cell::new(false)),
            scrollbar_drag: Cell::new(None),
            fold_callback: RefCell::new(None),
            font_size_adjust_callback: RefCell::new(None),
        });

        area.set_draw_func({
            let state = state.clone();
            move |area, context, width, height| draw(area, context, width, height, &state)
        });
        area.connect_resize({
            let state = state.clone();
            move |area, _, _| {
                clamp_scroll(area, &state);
                area.queue_draw();
            }
        });

        install_scroll(&area, &state);
        install_clicks(&area, &state);
        install_motion(&area, &state);
        install_font_shortcuts(&area, &state);

        Self { root: area, state }
    }

    pub(in crate::ui) fn set_rows(&self, rows: Vec<FileDiffRow>) {
        self.state.rows.replace(rows);
        clamp_scroll(&self.root, &self.state);
        self.root.queue_draw();
    }

    pub(in crate::ui) fn clear(&self) {
        self.state.rows.borrow_mut().clear();
        self.state.scroll_y.set(0.0);
        self.root.queue_draw();
    }

    pub(in crate::ui) fn scroll_y(&self) -> f64 {
        self.state.scroll_y.get()
    }

    pub(in crate::ui) fn set_scroll_y(&self, scroll_y: f64) {
        set_scroll_y(&self.root, &self.state, scroll_y);
    }

    pub(in crate::ui) fn set_fold_callback<F>(&self, callback: F)
    where
        F: Fn(usize) + 'static,
    {
        self.state.fold_callback.replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn font_size(&self) -> f64 {
        self.state.font_size.get()
    }

    pub(in crate::ui) fn set_font_size(&self, font_size: f64) {
        let font_size = config::normalize_font_size(font_size, config::DEFAULT_DIFF_FONT_SIZE);
        if (self.state.font_size.get() - font_size).abs() <= f64::EPSILON {
            return;
        }
        self.state.font_size.set(font_size);
        clamp_scroll(&self.root, &self.state);
        self.root.queue_draw();
    }

    pub(in crate::ui) fn set_font_size_adjust_callback<F>(&self, callback: F)
    where
        F: Fn(f64) + 'static,
    {
        self.state
            .font_size_adjust_callback
            .replace(Some(Rc::new(callback)));
    }
}

fn draw(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<DiffCanvasState>,
) {
    let theme = theme_for(area);
    fill_rect(context, 0.0, 0.0, width as f64, height as f64, theme.background);

    let content_width = canvas_scrollbar::content_width(width).max(MIN_CONTENT_WIDTH.min(width.max(1)));
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let rows = state.rows.borrow();
    let gutter_width = gutter_width(area, state, rows.as_slice());
    let layouts = row_layouts(area, state, rows.as_slice(), content_width, gutter_width, metrics.line_height);
    let content_height = layouts
        .last()
        .map(|layout| layout.y + layout.height)
        .unwrap_or(metrics.line_height)
        .max(metrics.line_height);
    state.content_height.set(content_height);
    clamp_scroll(area, state);

    let scroll_y = state.scroll_y.get();
    let divider_x = (content_width as f64 / 2.0).floor();
    fill_rect(
        context,
        divider_x,
        0.0,
        DIVIDER_WIDTH,
        height as f64,
        theme.divider,
    );

    for (index, row) in rows.iter().enumerate() {
        let Some(layout) = layouts.get(index) else {
            continue;
        };
        let y = layout.y - scroll_y;
        if y > height as f64 || y + layout.height < 0.0 {
            continue;
        }
        draw_row(
            area,
            context,
            state,
            row,
            layout,
            y,
            content_width,
            gutter_width,
            metrics.line_height,
            metrics.baseline_offset,
            theme,
        );
    }

    draw_scrollbar(area, context, width, height, state, &layouts, rows.as_slice());
}

fn row_layouts(
    area: &gtk::DrawingArea,
    state: &Rc<DiffCanvasState>,
    rows: &[FileDiffRow],
    content_width: i32,
    gutter_width: f64,
    line_height: f64,
) -> Vec<RowLayout> {
    let half_width = (content_width as f64 / 2.0).floor();
    let text_width = (half_width - gutter_width - PREFIX_WIDTH - CELL_PADDING * 2.0)
        .max(canvas::text_width_for_size(area, state.font_size.get(), "0"));
    let mut layouts = Vec::with_capacity(rows.len());
    let mut y = 0.0;

    for row in rows {
        let left_lines = wrap_text(area, state, row.left_text.as_deref().unwrap_or_default(), text_width);
        let right_lines = wrap_text(
            area,
            state,
            row.right_text.as_deref().unwrap_or_default(),
            text_width,
        );
        let line_count = left_lines.len().max(right_lines.len()).max(1);
        let height = line_count as f64 * line_height;
        layouts.push(RowLayout {
            y,
            height,
            left_lines,
            right_lines,
        });
        y += height;
    }

    layouts
}

fn draw_row(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<DiffCanvasState>,
    row: &FileDiffRow,
    layout: &RowLayout,
    y: f64,
    content_width: i32,
    gutter_width: f64,
    line_height: f64,
    baseline_offset: f64,
    theme: DiffCanvasTheme,
) {
    let half_width = (content_width as f64 / 2.0).floor();
    if is_fold_row(row) {
        fill_rect(context, 0.0, y, content_width as f64, layout.height, theme.fold_background);
        let label = row.right_text.as_deref().or(row.left_text.as_deref()).unwrap_or("");
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            label,
            CELL_PADDING,
            y + baseline_offset,
            theme.fold_text.text(),
        );
        return;
    }

    draw_side_background(context, 0.0, y, half_width, layout.height, row.left_kind, theme);
    draw_side_background(
        context,
        half_width + DIVIDER_WIDTH,
        y,
        half_width,
        layout.height,
        row.right_kind,
        theme,
    );

    draw_side(
        area,
        context,
        state,
        row.left_number,
        row.left_kind,
        &layout.left_lines,
        0.0,
        y,
        gutter_width,
        line_height,
        baseline_offset,
        theme,
    );
    draw_side(
        area,
        context,
        state,
        row.right_number,
        row.right_kind,
        &layout.right_lines,
        half_width + DIVIDER_WIDTH,
        y,
        gutter_width,
        line_height,
        baseline_offset,
        theme,
    );
}

fn draw_side(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    state: &Rc<DiffCanvasState>,
    number: Option<usize>,
    kind: DiffKind,
    lines: &[String],
    x: f64,
    y: f64,
    gutter_width: f64,
    line_height: f64,
    baseline_offset: f64,
    theme: DiffCanvasTheme,
) {
    fill_rect(context, x, y, gutter_width, line_height * lines.len().max(1) as f64, theme.gutter);

    let text_color = match kind {
        DiffKind::Added => theme.added_text,
        DiffKind::Deleted => theme.deleted_text,
        DiffKind::Context | DiffKind::Fold => theme.foreground,
    };
    if let Some(number) = number {
        let number = number.to_string();
        let number_width = canvas::text_width_for_size(area, state.font_size.get(), &number);
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            &number,
            x + gutter_width - PREFIX_WIDTH - CELL_PADDING - number_width,
            y + baseline_offset,
            theme.muted.text(),
        );
    }
    if let Some(prefix) = diff_prefix(kind) {
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            prefix,
            x + gutter_width - PREFIX_WIDTH + 5.0,
            y + baseline_offset,
            theme.muted.text(),
        );
    }

    for (index, line) in lines.iter().enumerate() {
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            line,
            x + gutter_width + CELL_PADDING,
            y + baseline_offset + index as f64 * line_height,
            text_color.text(),
        );
    }
}

fn draw_side_background(
    context: &cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    kind: DiffKind,
    theme: DiffCanvasTheme,
) {
    let color = match kind {
        DiffKind::Added => theme.added_background,
        DiffKind::Deleted => theme.deleted_background,
        DiffKind::Context => theme.background,
        DiffKind::Fold => theme.fold_background,
    };
    fill_rect(context, x, y, width, height, color);
}

fn draw_scrollbar(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: &Rc<DiffCanvasState>,
    layouts: &[RowLayout],
    rows: &[FileDiffRow],
) {
    let total_height = state.content_height.get();
    let hover = state.scrollbar_hover_progress.get();
    let theme = canvas_scrollbar::Theme::for_widget(area);
    canvas_scrollbar::draw_track(context, width, height, total_height, hover, theme);
    draw_scrollbar_markers(context, width, height, total_height, hover, layouts, rows);
    canvas_scrollbar::draw_thumb(
        context,
        width,
        height,
        total_height,
        state.scroll_y.get(),
        hover,
        state.scrollbar_active.get(),
        theme,
    );
}

fn draw_scrollbar_markers(
    context: &cairo::Context,
    width: i32,
    height: i32,
    total_height: f64,
    hover: f64,
    layouts: &[RowLayout],
    rows: &[FileDiffRow],
) {
    if rows.is_empty() || total_height <= 0.0 {
        return;
    }
    let _ = context.save();
    canvas_scrollbar::clip_to_track(context, width, height, hover);
    let (track_x, track_y, track_width, track_height) =
        canvas_scrollbar::visual_track_rect(width, height, hover);

    for (index, row) in rows.iter().enumerate() {
        let Some(kind) = marker_kind(row) else {
            continue;
        };
        let Some(layout) = layouts.get(index) else {
            continue;
        };
        let marker_y = track_y + (layout.y / total_height) * track_height;
        let marker_height = (layout.height / total_height * track_height)
            .max(2.0)
            .min(track_y + track_height - marker_y);
        canvas_scrollbar::draw_marker(
            context,
            kind,
            track_x,
            marker_y,
            track_width,
            marker_height,
            hover,
        );
    }
    let _ = context.restore();
}

fn install_scroll(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let area = area.clone();
        let state = state.clone();
        move |_, _, dy| {
            let line_height = canvas::measure_font_metrics(&area, state.font_size.get(), |font_size| {
                (font_size + 9.0).ceil()
            })
            .line_height;
            set_scroll_y(&area, &state, state.scroll_y.get() + dy * line_height * 3.0);
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(scroll);
}

fn install_clicks(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.connect_pressed({
        let area = area.clone();
        let state = state.clone();
        move |gesture, _, x, y| {
            area.grab_focus();
            if canvas_scrollbar::point_in_lane(
                area.allocated_width(),
                area.allocated_height(),
                state.content_height.get(),
                x,
            ) {
                if let Some(scroll_y) = canvas_scrollbar::scroll_for_lane_press(
                    area.allocated_width(),
                    area.allocated_height(),
                    state.content_height.get(),
                    state.scroll_y.get(),
                    x,
                    y,
                ) {
                    set_scroll_y(&area, &state, scroll_y);
                    state
                        .scrollbar_drag
                        .set(Some(canvas_scrollbar::Drag::new(state.scroll_y.get())));
                    canvas_scrollbar::set_active(
                        &area,
                        &state.scrollbar_hover,
                        &state.scrollbar_active,
                        &state.scrollbar_hover_progress,
                        &state.scrollbar_animating,
                        true,
                    );
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
            }

            if let Some(fold_index) = fold_index_at(&area, &state, y) {
                if let Some(callback) = state.fold_callback.borrow().as_ref().cloned() {
                    callback(fold_index);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            }
        }
    });
    click.connect_released({
        let area = area.clone();
        let state = state.clone();
        move |_, _, _, _| {
            state.scrollbar_drag.set(None);
            canvas_scrollbar::set_active(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                false,
            );
        }
    });
    area.add_controller(click);

    let drag = gtk::GestureDrag::new();
    drag.connect_drag_update({
        let area = area.clone();
        let state = state.clone();
        move |gesture, _, offset_y| {
            let Some(drag) = state.scrollbar_drag.get() else {
                return;
            };
            let Some((_, start_y)) = gesture.start_point() else {
                return;
            };
            let Some((_, _, _, thumb_height)) = canvas_scrollbar::thumb_rect(
                area.allocated_width(),
                area.allocated_height(),
                state.content_height.get(),
                drag.scroll_for_delta(
                    0.0,
                    area.allocated_height() as f64,
                    1.0,
                    canvas_scrollbar::max_scroll(
                        state.content_height.get(),
                        area.allocated_height() as f64,
                    ),
                ),
            ) else {
                return;
            };
            let max_scroll =
                canvas_scrollbar::max_scroll(state.content_height.get(), area.allocated_height() as f64);
            set_scroll_y(
                &area,
                &state,
                drag.scroll_for_delta(offset_y + start_y - start_y, area.allocated_height() as f64, thumb_height, max_scroll),
            );
        }
    });
    area.add_controller(drag);
}

fn install_motion(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let area = area.clone();
        let state = state.clone();
        move |_, x, _| {
            let hover = canvas_scrollbar::point_in_lane(
                area.allocated_width(),
                area.allocated_height(),
                state.content_height.get(),
                x,
            );
            canvas_scrollbar::set_hover(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                hover,
            );
        }
    });
    motion.connect_leave({
        let area = area.clone();
        let state = state.clone();
        move |_| {
            canvas_scrollbar::set_hover(
                &area,
                &state.scrollbar_hover,
                &state.scrollbar_active,
                &state.scrollbar_hover_progress,
                &state.scrollbar_animating,
                false,
            );
        }
    });
    area.add_controller(motion);
}

fn install_font_shortcuts(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let area = area.clone();
        let state = state.clone();
        move |_, key, _, modifiers| {
            let Some(delta) = font_size_delta_for_key(key, modifiers) else {
                return gtk::glib::Propagation::Proceed;
            };
            if let Some(callback) = state.font_size_adjust_callback.borrow().as_ref().cloned() {
                callback(delta);
            } else {
                let next = config::normalize_font_size(
                    state.font_size.get() + delta,
                    config::DEFAULT_DIFF_FONT_SIZE,
                );
                state.font_size.set(next);
                config::save_diff_font_size(next);
                clamp_scroll(&area, &state);
                area.queue_draw();
            }
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(keys);
}

fn set_scroll_y(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>, scroll_y: f64) {
    let max_scroll =
        canvas_scrollbar::max_scroll(state.content_height.get(), area.allocated_height() as f64);
    state.scroll_y.set(scroll_y.clamp(0.0, max_scroll));
    area.queue_draw();
}

fn clamp_scroll(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let max_scroll =
        canvas_scrollbar::max_scroll(state.content_height.get(), area.allocated_height() as f64);
    state.scroll_y.set(state.scroll_y.get().clamp(0.0, max_scroll));
}

fn fold_index_at(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>, y: f64) -> Option<usize> {
    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let rows = state.rows.borrow();
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let layouts = row_layouts(
        area,
        state,
        rows.as_slice(),
        content_width,
        gutter_width(area, state, rows.as_slice()),
        metrics.line_height,
    );
    let document_y = y + state.scroll_y.get();
    let row_index = layouts
        .iter()
        .position(|layout| document_y >= layout.y && document_y < layout.y + layout.height)?;
    let row = rows.get(row_index)?;
    is_fold_row(row).then(|| row.left_number).flatten()
}

fn wrap_text(
    area: &gtk::DrawingArea,
    state: &Rc<DiffCanvasState>,
    text: &str,
    wrap_width: f64,
) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut segment_start = 0;
    let mut line_width = 0.0;
    for (byte, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = canvas::text_width_for_size(area, state.font_size.get(), grapheme);
        if segment_start < byte && line_width + grapheme_width > wrap_width {
            lines.push(text[segment_start..byte].to_string());
            segment_start = byte;
            line_width = 0.0;
        }
        line_width += grapheme_width;
    }
    lines.push(text[segment_start..].to_string());
    lines
}

fn gutter_width(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>, rows: &[FileDiffRow]) -> f64 {
    let max_number = rows
        .iter()
        .flat_map(|row| [row.left_number, row.right_number])
        .flatten()
        .max()
        .unwrap_or(1)
        .to_string();
    canvas::text_width_for_size(area, state.font_size.get(), &max_number)
        + PREFIX_WIDTH
        + CELL_PADDING * 2.0
}

fn fill_rect(context: &cairo::Context, x: f64, y: f64, width: f64, height: f64, color: Color) {
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
    context.rectangle(x, y, width, height);
    let _ = context.fill();
}

fn diff_prefix(kind: DiffKind) -> Option<&'static str> {
    match kind {
        DiffKind::Added => Some("+"),
        DiffKind::Deleted => Some("-"),
        DiffKind::Context | DiffKind::Fold => None,
    }
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

fn is_fold_row(row: &FileDiffRow) -> bool {
    row.left_kind == DiffKind::Fold || row.right_kind == DiffKind::Fold
}

fn font_size_delta_for_key(key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> Option<f64> {
    if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
        || modifiers.contains(gtk::gdk::ModifierType::ALT_MASK)
    {
        return None;
    }
    if key == gtk::gdk::Key::plus || key == gtk::gdk::Key::equal || key == gtk::gdk::Key::KP_Add {
        return Some(1.0);
    }
    if key == gtk::gdk::Key::minus
        || key == gtk::gdk::Key::underscore
        || key == gtk::gdk::Key::KP_Subtract
    {
        return Some(-1.0);
    }
    None
}

fn theme_for(area: &gtk::DrawingArea) -> DiffCanvasTheme {
    let dark = adw::StyleManager::for_display(&area.display()).is_dark();
    if dark {
        DiffCanvasTheme {
            background: Color::rgba(0.118, 0.118, 0.135, 1.0),
            foreground: Color::rgba(0.86, 0.86, 0.86, 1.0),
            muted: Color::rgba(0.56, 0.56, 0.60, 1.0),
            gutter: Color::rgba(0.105, 0.105, 0.12, 1.0),
            divider: Color::rgba(0.25, 0.25, 0.28, 1.0),
            added_background: Color::rgba(0.10, 0.24, 0.16, 1.0),
            added_text: Color::rgba(0.64, 0.80, 0.55, 1.0),
            deleted_background: Color::rgba(0.30, 0.12, 0.14, 1.0),
            deleted_text: Color::rgba(0.92, 0.42, 0.46, 1.0),
            missing_background: Color::rgba(0.095, 0.095, 0.11, 1.0),
            fold_background: Color::rgba(0.14, 0.14, 0.16, 1.0),
            fold_text: Color::rgba(0.62, 0.66, 0.72, 1.0),
        }
    } else {
        DiffCanvasTheme {
            background: Color::rgba(0.98, 0.98, 0.98, 1.0),
            foreground: Color::rgba(0.16, 0.16, 0.18, 1.0),
            muted: Color::rgba(0.48, 0.48, 0.52, 1.0),
            gutter: Color::rgba(0.94, 0.94, 0.95, 1.0),
            divider: Color::rgba(0.78, 0.78, 0.80, 1.0),
            added_background: Color::rgba(0.86, 0.96, 0.88, 1.0),
            added_text: Color::rgba(0.16, 0.42, 0.20, 1.0),
            deleted_background: Color::rgba(1.0, 0.88, 0.88, 1.0),
            deleted_text: Color::rgba(0.62, 0.16, 0.18, 1.0),
            missing_background: Color::rgba(0.92, 0.92, 0.93, 1.0),
            fold_background: Color::rgba(0.90, 0.92, 0.95, 1.0),
            fold_text: Color::rgba(0.30, 0.34, 0.40, 1.0),
        }
    }
}
