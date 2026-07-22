use super::skia_canvas;
use super::{
    canvas_overshoot,
    code_editor::{
        canvas::{self, TextColor},
        selection::{
            AnchoredSelection, DragSelection, SelectionMode, clipped_bounds, drag_for_mode,
            selection_for_drag, selection_for_mode, word_bounds_at as text_word_bounds_at,
        },
    },
    diff_layout, skia_gl_area,
};
use crate::config;
use crate::git::{DiffKind, FileDiffRow};
use crate::language_support::{HighlightRange, SyntaxHighlighter, language_hint_from_path};
use crate::ui::components::context_menu;
use crate::ui::{canvas_scroll, canvas_scrollbar};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

const MIN_CONTENT_WIDTH: i32 = 360;
const CELL_PADDING: f64 = 8.0;
const PREFIX_WIDTH: f64 = 18.0;
const DIVIDER_WIDTH: f64 = 1.0;
const DRAG_AUTOSCROLL_ZONE_LINES: f64 = 2.0;
const DRAG_AUTOSCROLL_MIN_LINES_PER_FRAME: f64 = 0.5;
const DRAG_AUTOSCROLL_BASE_EXTRA_LINES_PER_FRAME: f64 = 1.5;
const DRAG_AUTOSCROLL_AGGRESSIVE_EXTRA_LINES_PER_FRAME: f64 = 2.0;
const DRAG_AUTOSCROLL_OUTSIDE_EXTRA_LINES_PER_FRAME: f64 = 2.0;

#[derive(Clone)]
pub struct DiffCanvas {
    pub root: gtk::Overlay,
    area: gtk::GLArea,
    spinner: adw::Spinner,
    state: Rc<DiffCanvasState>,
}

struct DiffCanvasState {
    rows: RefCell<Vec<FileDiffRow>>,
    font_size: Cell<f64>,
    scroll_y: Cell<f64>,
    content_height: Cell<f64>,
    overshoot: canvas_overshoot::EdgeGlow,
    scrollbar_hover: Rc<Cell<bool>>,
    scrollbar_active: Rc<Cell<bool>>,
    scrollbar_hover_progress: Rc<Cell<f64>>,
    scrollbar_animating: Rc<Cell<bool>>,
    scrollbar_smooth_scroll: canvas_scrollbar::SmoothScroll,
    scrollbar_drag: Cell<Option<canvas_scrollbar::Drag>>,
    middle_autoscroll: Rc<canvas_scroll::MiddleAutoscroll>,
    fold_callback: RefCell<Option<Rc<dyn Fn(usize)>>>,
    layout_generation: Cell<u64>,
    layout_cache: RefCell<Option<DiffLayoutCache>>,
    layout_pending_signature: RefCell<Option<DiffLayoutSignature>>,
    layout_request_id: Cell<u64>,
    text_width_cache: RefCell<canvas::TextWidthCache>,
    max_line_number: Cell<usize>,
    fold_row_count: Cell<usize>,
    syntax: RefCell<Option<DiffSyntaxState>>,
    syntax_signature: RefCell<Option<DiffSyntaxSignature>>,
    selection: RefCell<Option<DiffSelection>>,
    selection_drag: Cell<Option<DragSelection<DiffSelectionPoint>>>,
    active_side: Cell<DiffCanvasSide>,
    search: RefCell<DiffSearchState>,
}

type DiffLayoutCache = diff_layout::Cache;
type DiffLayoutSignature = diff_layout::Signature;
type RowLayout = diff_layout::RowLayout;
type WrappedLine = diff_layout::WrappedLine;

struct DiffSyntaxState {
    left: DiffSyntaxSide,
    right: DiffSyntaxSide,
}

struct DiffSyntaxSide {
    source: String,
    ranges_by_line: HashMap<usize, (usize, usize)>,
    highlights: Vec<HighlightRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffSyntaxSignature {
    file_path: String,
    row_count: usize,
    fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DiffCanvasSide {
    Left,
    Right,
}

type DiffSelection = AnchoredSelection<DiffSelectionPoint>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DiffSelectionPoint {
    side: DiffCanvasSide,
    row: usize,
    byte: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiffSearchMatch {
    side: DiffCanvasSide,
    row: usize,
    start: usize,
    end: usize,
}

#[derive(Default)]
struct DiffSearchState {
    query: String,
    matches: Vec<DiffSearchMatch>,
    active: Option<usize>,
}

#[derive(Clone, Copy)]
enum DiffContextAction {
    Copy,
    SelectAll,
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
    fold_background: Color,
    fold_text: Color,
    selection_background: Color,
    search_match_background: Color,
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
    pub fn new() -> Self {
        let area = skia_gl_area::new_area();
        area.set_size_request(MIN_CONTENT_WIDTH, 160);
        let spinner = adw::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        spinner.set_size_request(32, 32);
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&area));
        root.add_overlay(&spinner);

        let font_size = config::load().font_sizes.diff;
        let state = Rc::new(DiffCanvasState {
            rows: RefCell::new(Vec::new()),
            font_size: Cell::new(font_size),
            scroll_y: Cell::new(0.0),
            content_height: Cell::new(1.0),
            overshoot: canvas_overshoot::EdgeGlow::new(),
            scrollbar_hover: Rc::new(Cell::new(false)),
            scrollbar_active: Rc::new(Cell::new(false)),
            scrollbar_hover_progress: Rc::new(Cell::new(0.0)),
            scrollbar_animating: Rc::new(Cell::new(false)),
            scrollbar_smooth_scroll: canvas_scrollbar::SmoothScroll::new(),
            scrollbar_drag: Cell::new(None),
            middle_autoscroll: Rc::new(canvas_scroll::MiddleAutoscroll::new()),
            fold_callback: RefCell::new(None),
            layout_generation: Cell::new(1),
            layout_cache: RefCell::new(None),
            layout_pending_signature: RefCell::new(None),
            layout_request_id: Cell::new(0),
            text_width_cache: RefCell::new(canvas::TextWidthCache::new(font_size)),
            max_line_number: Cell::new(1),
            fold_row_count: Cell::new(0),
            syntax: RefCell::new(None),
            syntax_signature: RefCell::new(None),
            selection: RefCell::new(None),
            selection_drag: Cell::new(None),
            active_side: Cell::new(DiffCanvasSide::Right),
            search: RefCell::new(DiffSearchState::default()),
        });

        skia_gl_area::install(&area, {
            let state = state.clone();
            move |area, context, width, height| draw(area, context, width, height, &state)
        });
        area.connect_resize({
            let state = state.clone();
            let spinner = spinner.clone();
            move |area, _, _| {
                request_layout(area, &state, &spinner);
                clamp_scroll(area, &state);
            }
        });

        install_scroll(&area, &state);
        install_diff_middle_autoscroll(&area, &state);
        install_clicks(&area, &state, &spinner);
        install_motion(&area, &state);
        install_key_shortcuts(&area, &state, &spinner);

        Self {
            root,
            area,
            spinner,
            state,
        }
    }

    pub fn set_rows(&self, rows: Vec<FileDiffRow>) {
        let fold_rows = rows.iter().filter(|row| is_fold_row(row)).count();
        let max_line_number = rows
            .iter()
            .flat_map(|row| [row.left_number, row.right_number])
            .flatten()
            .max()
            .unwrap_or(1);
        if diff_rows_equal(&self.state.rows.borrow(), &rows) {
            self.state.fold_row_count.set(fold_rows);
            self.state.max_line_number.set(max_line_number);
            return;
        }

        stop_diff_middle_autoscroll(&self.area, &self.state);
        self.state.fold_row_count.set(fold_rows);
        self.state.max_line_number.set(max_line_number);
        self.state.rows.replace(rows);
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.layout_pending_signature.borrow_mut().take();
        rebuild_search_matches(&self.area, &self.state);
        request_layout(&self.area, &self.state, &self.spinner);
        clamp_scroll(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn set_syntax_for_file(
        &self,
        file_path: &str,
        fingerprint: u64,
        full_rows: &[FileDiffRow],
    ) {
        update_syntax_state(&self.state, file_path, fingerprint, full_rows);
    }

    pub fn clear(&self) {
        stop_diff_middle_autoscroll(&self.area, &self.state);
        self.state.rows.borrow_mut().clear();
        self.state.scroll_y.set(0.0);
        self.state.max_line_number.set(1);
        self.state.fold_row_count.set(0);
        self.state.syntax.borrow_mut().take();
        self.state.syntax_signature.borrow_mut().take();
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state.search.borrow_mut().matches.clear();
        self.state.search.borrow_mut().active = None;
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.layout_pending_signature.borrow_mut().take();
        self.state.content_height.set(1.0);
        self.spinner.set_visible(false);
        self.area.queue_render();
    }

    pub fn scroll_y(&self) -> f64 {
        self.state.scroll_y.get()
    }

    pub fn set_scroll_y(&self, scroll_y: f64) {
        if self.state.layout_cache.borrow().is_some() {
            set_scroll_y(&self.area, &self.state, scroll_y);
        } else {
            // A replacement diff is laid out off-thread. Keep the requested
            // position until that layout arrives, then clamp it to its bounds.
            self.state.scroll_y.set(scroll_y.max(0.0));
            self.area.queue_render();
        }
    }

    pub fn set_fold_callback<F>(&self, callback: F)
    where
        F: Fn(usize) + 'static,
    {
        self.state.fold_callback.replace(Some(Rc::new(callback)));
    }

    pub fn focus(&self) {
        self.area.grab_focus();
    }

    pub fn set_search_query(&self, query: &str) {
        let changed = {
            let mut search = self.state.search.borrow_mut();
            if search.query == query {
                false
            } else {
                search.query = query.to_string();
                true
            }
        };
        if changed {
            rebuild_search_matches(&self.area, &self.state);
        } else {
            select_active_search_match(&self.area, &self.state);
        }
        self.area.queue_render();
    }

    pub fn search_next(&self) {
        let len = self.state.search.borrow().matches.len();
        if len == 0 {
            return;
        }
        {
            let mut search = self.state.search.borrow_mut();
            search.active = Some(search.active.map(|active| (active + 1) % len).unwrap_or(0));
        }
        select_active_search_match(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn search_previous(&self) {
        let len = self.state.search.borrow().matches.len();
        if len == 0 {
            return;
        }
        {
            let mut search = self.state.search.borrow_mut();
            search.active = Some(
                search
                    .active
                    .map(|active| active.checked_sub(1).unwrap_or(len - 1))
                    .unwrap_or(len - 1),
            );
        }
        select_active_search_match(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn search_status(&self) -> String {
        let search = self.state.search.borrow();
        if search.query.is_empty() {
            return String::new();
        }
        let Some(active) = search.active else {
            return "No Results".to_string();
        };
        format!("{} of {}", active + 1, search.matches.len())
    }
}

fn update_syntax_state(
    state: &Rc<DiffCanvasState>,
    file_path: &str,
    fingerprint: u64,
    rows: &[FileDiffRow],
) {
    let signature = DiffSyntaxSignature::new(file_path, rows.len(), fingerprint);
    if state.syntax_signature.borrow().as_ref() == Some(&signature) {
        return;
    }

    let language = language_hint_from_path(file_path);
    let left = build_syntax_side(&language, rows, DiffCanvasSide::Left);
    let right = build_syntax_side(&language, rows, DiffCanvasSide::Right);
    state.syntax.replace(Some(DiffSyntaxState { left, right }));
    state.syntax_signature.replace(Some(signature));
}

fn build_syntax_side(language: &str, rows: &[FileDiffRow], side: DiffCanvasSide) -> DiffSyntaxSide {
    let mut source = String::new();
    let mut ranges_by_line = HashMap::new();
    for row in rows {
        if is_fold_row(row) {
            continue;
        }
        let (number, text) = match side {
            DiffCanvasSide::Left => (row.left_number, row.left_text.as_deref()),
            DiffCanvasSide::Right => (row.right_number, row.right_text.as_deref()),
        };
        let (Some(number), Some(text)) = (number, text) else {
            continue;
        };
        let start = source.len();
        source.push_str(text);
        let end = source.len();
        ranges_by_line.insert(number, (start, end));
        source.push('\n');
    }

    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.set_source(&source);
    let mut highlights = highlighter.highlight_current();
    highlights.sort_by_key(|range| (range.start, range.end));

    DiffSyntaxSide {
        source,
        ranges_by_line,
        highlights,
    }
}

impl DiffSyntaxSignature {
    fn new(file_path: &str, row_count: usize, fingerprint: u64) -> Self {
        Self {
            file_path: file_path.to_string(),
            row_count,
            fingerprint,
        }
    }
}

fn draw(
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    width: i32,
    height: i32,
    state: &Rc<DiffCanvasState>,
) {
    let theme = theme_for(area);
    fill_rect(
        context,
        0.0,
        0.0,
        width as f64,
        height as f64,
        theme.background,
    );

    let content_width =
        canvas_scrollbar::content_width(width).max(MIN_CONTENT_WIDTH.min(width.max(1)));
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let rows = state.rows.borrow();
    let gutter_width = gutter_width(area, state);
    let cache = state.layout_cache.borrow();
    let Some(cache) = cache.as_ref() else {
        canvas_overshoot::draw(context, width, height, &state.overshoot);
        return;
    };
    let content_height = cache.content_height.max(metrics.line_height);
    state.content_height.set(content_height);
    clamp_scroll(area, state);

    let divider_x = (content_width as f64 / 2.0).floor();
    fill_rect(
        context,
        divider_x,
        0.0,
        DIVIDER_WIDTH,
        height as f64,
        theme.divider,
    );

    let scroll_y = state.scroll_y.get();
    let visible_range =
        diff_layout::visible_row_range(cache, scroll_y, height as f64, metrics.line_height * 8.0);
    for index in visible_range {
        let Some(row) = rows.get(index) else {
            continue;
        };
        let Some(layout) = cache.rows.get(index) else {
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
            index,
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

    draw_scrollbar(area, context, width, height, state);
    canvas_overshoot::draw(context, width, height, &state.overshoot);
    draw_middle_autoscroll_marker(context, width, height, state, theme);
}

fn request_layout(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, spinner: &adw::Spinner) -> bool {
    let Some(request) = layout_request_for_area(area, state) else {
        state.layout_cache.borrow_mut().take();
        state.layout_pending_signature.borrow_mut().take();
        state.content_height.set(1.0);
        spinner.set_visible(false);
        return false;
    };

    if state
        .layout_cache
        .borrow()
        .as_ref()
        .is_some_and(|cache| cache.signature == request.signature)
    {
        state.layout_pending_signature.borrow_mut().take();
        spinner.set_visible(false);
        return true;
    }

    if state.layout_pending_signature.borrow().as_ref() == Some(&request.signature) {
        spinner.set_visible(true);
        return false;
    }

    let request_id = state.layout_request_id.get().wrapping_add(1).max(1);
    state.layout_request_id.set(request_id);
    state
        .layout_pending_signature
        .replace(Some(request.signature.clone()));
    spinner.set_visible(true);

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = diff_layout::build(request);
        let _ = sender.send(result);
    });

    gtk::glib::timeout_add_local(Duration::from_millis(16), {
        let area = area.clone();
        let state = state.clone();
        let spinner = spinner.clone();
        move || match receiver.try_recv() {
            Ok(result) => {
                if state.layout_request_id.get() != request_id {
                    return gtk::glib::ControlFlow::Break;
                }
                if state.layout_pending_signature.borrow().as_ref() != Some(&result.cache.signature)
                {
                    return gtk::glib::ControlFlow::Break;
                }

                let content_height = result.cache.content_height;
                state.content_height.set(content_height);
                state.layout_cache.replace(Some(result.cache));
                state.layout_pending_signature.borrow_mut().take();
                spinner.set_visible(false);
                clamp_scroll(&area, &state);
                if state.search.borrow().active.is_some() {
                    select_active_search_match(&area, &state);
                }
                area.queue_render();
                gtk::glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if state.layout_request_id.get() == request_id {
                    state.layout_pending_signature.borrow_mut().take();
                    spinner.set_visible(false);
                    log::warn!("diff_canvas layout worker disconnected request={request_id}");
                }
                gtk::glib::ControlFlow::Break
            }
        }
    });

    false
}

fn layout_request_for_area(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
) -> Option<diff_layout::Request> {
    let rows = state.rows.borrow();
    if rows.is_empty() {
        return None;
    }

    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let gutter_width = gutter_width(area, state);
    let half_width = (content_width as f64 / 2.0).floor();
    let text_width =
        (half_width - gutter_width - PREFIX_WIDTH - CELL_PADDING * 2.0).max(metrics.char_width);
    let signature = diff_layout::Signature::new(
        state.layout_generation.get(),
        content_width,
        gutter_width,
        metrics.line_height,
        text_width,
        metrics.char_width,
        rows.len(),
    );

    Some(diff_layout::Request {
        signature,
        rows: rows.clone(),
        text_width,
        line_height: metrics.line_height,
        char_width: metrics.char_width,
    })
}

fn draw_row(
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    state: &Rc<DiffCanvasState>,
    row_index: usize,
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
        fill_rect(
            context,
            0.0,
            y,
            content_width as f64,
            layout.height,
            theme.fold_background,
        );
        let label = row
            .right_text
            .as_deref()
            .or(row.left_text.as_deref())
            .unwrap_or("");
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

    draw_side_background(
        context,
        0.0,
        y,
        half_width,
        layout.height,
        row.left_kind,
        theme,
    );
    draw_side_background(
        context,
        half_width + DIVIDER_WIDTH,
        y,
        half_width,
        layout.height,
        row.right_kind,
        theme,
    );

    let syntax = state.syntax.borrow();
    draw_side(
        area,
        context,
        state,
        row_index,
        row.left_number,
        row.left_kind,
        &layout.left_lines,
        syntax.as_ref().map(|syntax| &syntax.left),
        DiffCanvasSide::Left,
        0.0,
        y,
        half_width,
        gutter_width,
        layout.height,
        line_height,
        baseline_offset,
        theme,
    );
    draw_side(
        area,
        context,
        state,
        row_index,
        row.right_number,
        row.right_kind,
        &layout.right_lines,
        syntax.as_ref().map(|syntax| &syntax.right),
        DiffCanvasSide::Right,
        half_width + DIVIDER_WIDTH,
        y,
        half_width,
        gutter_width,
        layout.height,
        line_height,
        baseline_offset,
        theme,
    );
}

fn draw_side(
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    state: &Rc<DiffCanvasState>,
    row_index: usize,
    number: Option<usize>,
    kind: DiffKind,
    lines: &[WrappedLine],
    syntax: Option<&DiffSyntaxSide>,
    side: DiffCanvasSide,
    x: f64,
    y: f64,
    side_width: f64,
    gutter_width: f64,
    row_height: f64,
    line_height: f64,
    baseline_offset: f64,
    theme: DiffCanvasTheme,
) {
    let gutter_x = match side {
        DiffCanvasSide::Left => x + side_width - gutter_width,
        DiffCanvasSide::Right => x,
    };
    let text_x = match side {
        DiffCanvasSide::Left => x + CELL_PADDING,
        DiffCanvasSide::Right => x + gutter_width + CELL_PADDING,
    };
    fill_rect(context, gutter_x, y, gutter_width, row_height, theme.gutter);

    let text_color = match kind {
        DiffKind::Added => theme.added_text,
        DiffKind::Deleted => theme.deleted_text,
        DiffKind::Context | DiffKind::Fold => theme.foreground,
    };
    if let Some(number) = number {
        let number = number.to_string();
        let number_width = cached_text_width_for_state(area, state, &number);
        canvas::draw_plain_text(
            area,
            context,
            state.font_size.get(),
            &number,
            gutter_x + gutter_width - PREFIX_WIDTH - CELL_PADDING - number_width,
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
            gutter_x + gutter_width - PREFIX_WIDTH + 5.0,
            y + baseline_offset,
            theme.muted.text(),
        );
    }

    let source_range = number.and_then(|number| {
        syntax
            .and_then(|syntax| syntax.ranges_by_line.get(&number))
            .copied()
    });
    let selection = *state.selection.borrow();
    for (index, line) in lines.iter().enumerate() {
        let baseline = y + baseline_offset + index as f64 * line_height;
        draw_search_matches(
            area,
            context,
            state,
            side,
            row_index,
            line,
            text_x,
            baseline - baseline_offset,
            line_height,
            theme,
        );
        if let Some((selection_start, selection_end)) =
            selection_range_for_wrapped_line(selection, side, row_index, line)
        {
            let prefix = line.text.get(..selection_start).unwrap_or_default();
            let selected = line
                .text
                .get(selection_start..selection_end)
                .unwrap_or_default();
            let selection_x = text_x + cached_text_width_for_state(area, state, prefix);
            let selection_width = cached_text_width_for_state(area, state, selected).max(2.0);
            fill_rect(
                context,
                selection_x,
                baseline - baseline_offset,
                selection_width,
                line_height,
                theme.selection_background,
            );
        }
        if let (Some(syntax), Some((source_start, source_end))) = (syntax, source_range) {
            draw_syntax_line(
                area,
                context,
                state,
                syntax,
                line,
                source_start,
                source_end,
                text_x,
                baseline,
                text_color.text(),
            );
        } else {
            canvas::draw_plain_text(
                area,
                context,
                state.font_size.get(),
                &line.text,
                text_x,
                baseline,
                text_color.text(),
            );
        }
    }
}

fn draw_syntax_line(
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    state: &Rc<DiffCanvasState>,
    syntax: &DiffSyntaxSide,
    line: &WrappedLine,
    source_start: usize,
    source_end: usize,
    mut x: f64,
    baseline: f64,
    fallback: TextColor,
) {
    let font_size = state.font_size.get();
    let absolute_start = source_start.saturating_add(line.start);
    let absolute_end = source_start.saturating_add(line.end);
    if absolute_start >= absolute_end
        || absolute_end > source_end
        || source_end > syntax.source.len()
        || !syntax.source.is_char_boundary(absolute_start)
        || !syntax.source.is_char_boundary(absolute_end)
    {
        canvas::draw_plain_text(area, context, font_size, &line.text, x, baseline, fallback);
        return;
    }

    let mut cursor = absolute_start;
    let first_range = syntax
        .highlights
        .partition_point(|range| range.end <= absolute_start);
    for range in &syntax.highlights[first_range..] {
        if range.start >= absolute_end {
            break;
        }
        if range.start >= range.end
            || range.end > syntax.source.len()
            || !syntax.source.is_char_boundary(range.start)
            || !syntax.source.is_char_boundary(range.end)
        {
            continue;
        }
        let range_start = range.start.max(absolute_start).max(cursor);
        let range_end = range.end.min(absolute_end);
        if range_start >= range_end {
            continue;
        }
        if cursor < range_start {
            let plain = &syntax.source[cursor..range_start];
            canvas::draw_plain_text(area, context, font_size, plain, x, baseline, fallback);
            x += cached_text_width_for_state(area, state, plain);
        }
        let segment = &syntax.source[range_start..range_end];
        let color = range.style.color();
        canvas::draw_plain_text(
            area,
            context,
            font_size,
            segment,
            x,
            baseline,
            TextColor::rgb(color.0, color.1, color.2),
        );
        x += cached_text_width_for_state(area, state, segment);
        cursor = range_end;
    }
    if cursor < absolute_end {
        canvas::draw_plain_text(
            area,
            context,
            font_size,
            &syntax.source[cursor..absolute_end],
            x,
            baseline,
            fallback,
        );
    }
}

fn selection_range_for_wrapped_line(
    selection: Option<DiffSelection>,
    side: DiffCanvasSide,
    row: usize,
    line: &WrappedLine,
) -> Option<(usize, usize)> {
    let selection = selection?;
    if selection.anchor.side != side || selection.focus.side != side {
        return None;
    }
    let (start, end) = selection.ordered()?;
    if row < start.row || row > end.row {
        return None;
    }

    let row_start = if row == start.row { start.byte } else { 0 };
    let row_end = if row == end.row { end.byte } else { usize::MAX };
    clipped_bounds(row_start, row_end, line.start, line.end)
        .map(|(start, end)| (start - line.start, end - line.start))
}

fn draw_search_matches(
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    state: &Rc<DiffCanvasState>,
    side: DiffCanvasSide,
    row_index: usize,
    line: &WrappedLine,
    text_x: f64,
    y: f64,
    line_height: f64,
    theme: DiffCanvasTheme,
) {
    let search = state.search.borrow();
    if search.query.is_empty() || search.matches.is_empty() {
        return;
    }

    let mut index = search
        .matches
        .partition_point(|search_match| search_match.row < row_index);
    while let Some(search_match) = search.matches.get(index) {
        if search_match.row != row_index {
            break;
        }
        index += 1;
        if search_match.side != side {
            continue;
        }
        let Some((start, end)) =
            clipped_bounds(search_match.start, search_match.end, line.start, line.end)
        else {
            continue;
        };
        let relative_start = start.saturating_sub(line.start);
        let relative_end = end.saturating_sub(line.start);
        let prefix = line.text.get(..relative_start).unwrap_or_default();
        let matched = line.text.get(relative_start..relative_end);
        let Some(matched) = matched.filter(|matched| !matched.is_empty()) else {
            continue;
        };
        fill_rect(
            context,
            text_x + cached_text_width_for_state(area, state, prefix),
            y,
            cached_text_width_for_state(area, state, matched).max(2.0),
            line_height,
            theme.search_match_background,
        );
    }
}

fn draw_side_background(
    context: &skia_canvas::Context,
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
    area: &gtk::GLArea,
    context: &skia_canvas::Context,
    width: i32,
    height: i32,
    state: &Rc<DiffCanvasState>,
) {
    let total_height = state.content_height.get();
    let hover = state.scrollbar_hover_progress.get();
    let theme = canvas_scrollbar::Theme::for_widget(area);
    canvas_scrollbar::draw_track(context, width, height, total_height, hover, theme);
    canvas_scrollbar::draw_thumb_fill(
        context,
        width,
        height,
        total_height,
        state.scroll_y.get(),
        hover,
        state.scrollbar_active.get(),
        theme,
    );
    if let Some(cache) = state.layout_cache.borrow().as_ref() {
        draw_scrollbar_markers(context, width, height, total_height, hover, cache);
    }
    canvas_scrollbar::draw_thumb_outline(
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
    context: &skia_canvas::Context,
    width: i32,
    height: i32,
    total_height: f64,
    hover: f64,
    cache: &DiffLayoutCache,
) {
    if cache.markers.is_empty() || total_height <= 0.0 {
        return;
    }
    let _ = context.save();
    canvas_scrollbar::clip_to_track(context, width, height, hover);
    let (track_x, track_y, track_width, track_height) =
        canvas_scrollbar::visual_track_rect(width, height, hover);

    for marker in &cache.markers {
        let Some(layout) = cache.rows.get(marker.row) else {
            continue;
        };
        let marker_y = track_y + (layout.y / total_height) * track_height;
        let marker_height = (layout.height / total_height * track_height)
            .max(2.0)
            .min(track_y + track_height - marker_y);
        canvas_scrollbar::draw_marker(
            context,
            marker.kind,
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
    context: &skia_canvas::Context,
    width: i32,
    height: i32,
    state: &Rc<DiffCanvasState>,
    theme: DiffCanvasTheme,
) {
    canvas_scroll::draw_middle_autoscroll_marker(
        context,
        width,
        height,
        state.middle_autoscroll.state(),
        canvas_scroll::AutoscrollAxes::Vertical,
        canvas_scroll::MarkerStyle {
            foreground: canvas_scroll::MarkerColor::rgba(
                theme.foreground.red,
                theme.foreground.green,
                theme.foreground.blue,
                theme.foreground.alpha * 0.82,
            ),
            background: canvas_scroll::MarkerColor::rgba(
                theme.background.red,
                theme.background.green,
                theme.background.blue,
                theme.background.alpha * 0.92,
            ),
            shadow: canvas_scroll::MarkerColor::rgba(0.0, 0.0, 0.0, 0.34),
        },
    );
}

fn install_scroll(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let area = area.clone();
        let state = state.clone();
        move |controller, _, dy| {
            let line_height =
                canvas::measure_font_metrics(&area, state.font_size.get(), |font_size| {
                    (font_size + 9.0).ceil()
                })
                .line_height;
            let viewport_height = area.allocated_height().max(1) as f64;
            let max_scroll =
                canvas_scrollbar::max_scroll(state.content_height.get(), viewport_height);
            if state.scrollbar_hover.get() && canvas_scrollbar::is_mouse_scroll(controller) {
                let delta = canvas_scrollbar::mouse_wheel_delta(viewport_height, dy);
                canvas_overshoot::pull_for_delta(
                    &area,
                    &state.overshoot,
                    state.scroll_y.get(),
                    max_scroll,
                    delta,
                    canvas_overshoot::Edge::Top,
                    canvas_overshoot::Edge::Bottom,
                );
                let area_for_scroll = area.clone();
                let state_for_scroll = state.clone();
                state.scrollbar_smooth_scroll.scroll_relative(
                    &area,
                    state.scroll_y.get(),
                    delta,
                    0.0,
                    max_scroll,
                    move |value| set_scroll_y(&area_for_scroll, &state_for_scroll, value),
                );
                return gtk::glib::Propagation::Stop;
            }

            state.scrollbar_smooth_scroll.pause();
            let delta = dy * line_height * 3.0;
            canvas_overshoot::pull_for_delta(
                &area,
                &state.overshoot,
                state.scroll_y.get(),
                max_scroll,
                delta,
                canvas_overshoot::Edge::Top,
                canvas_overshoot::Edge::Bottom,
            );
            set_scroll_y(&area, &state, state.scroll_y.get() + delta);
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(scroll);
}

fn install_diff_middle_autoscroll(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    canvas_scroll::install_middle_autoscroll(
        area,
        &state.middle_autoscroll,
        canvas_scroll::AutoscrollAxes::Vertical,
        "diff_canvas",
        {
            let area = area.clone();
            let state = state.clone();
            move || {
                let viewport_height = area.allocated_height().max(1) as f64;
                state.layout_cache.borrow().as_ref().is_some_and(|cache| {
                    canvas_scrollbar::max_scroll(cache.content_height, viewport_height)
                        > f64::EPSILON
                })
            }
        },
        {
            let area = area.clone();
            let state = state.clone();
            move |autoscroll_state| {
                let viewport_height = area.allocated_height().max(1) as f64;
                let max_scroll = state.layout_cache.borrow().as_ref().map_or(0.0, |cache| {
                    canvas_scrollbar::max_scroll(cache.content_height, viewport_height)
                });
                if max_scroll <= f64::EPSILON {
                    return;
                }

                let delta = canvas_scroll::middle_autoscroll_delta(
                    autoscroll_state.pointer.y - autoscroll_state.origin.y,
                );
                if delta.abs() <= f64::EPSILON {
                    return;
                }

                canvas_overshoot::pull_for_delta(
                    &area,
                    &state.overshoot,
                    state.scroll_y.get(),
                    max_scroll,
                    delta,
                    canvas_overshoot::Edge::Top,
                    canvas_overshoot::Edge::Bottom,
                );
                set_scroll_y(&area, &state, state.scroll_y.get() + delta);
            }
        },
        {
            let area = area.clone();
            let state = state.clone();
            move || clear_diff_autoscroll_hover(&area, &state)
        },
        {
            let area = area.clone();
            let state = state.clone();
            move || clear_diff_autoscroll_hover(&area, &state)
        },
        {
            let area = area.clone();
            move |cursor| area.set_cursor_from_name(cursor)
        },
        {
            let area = area.clone();
            move || area.queue_render()
        },
    );
}

fn clear_diff_autoscroll_hover(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    state.scrollbar_drag.set(None);
    state.selection_drag.set(None);
    canvas_scrollbar::set_hover(
        area,
        &state.scrollbar_hover,
        &state.scrollbar_active,
        &state.scrollbar_hover_progress,
        &state.scrollbar_animating,
        false,
    );
    canvas_scrollbar::set_active(
        area,
        &state.scrollbar_hover,
        &state.scrollbar_active,
        &state.scrollbar_hover_progress,
        &state.scrollbar_animating,
        false,
    );
}

fn stop_diff_middle_autoscroll(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    if !state.middle_autoscroll.stop() {
        return;
    }
    area.set_cursor_from_name(None);
    clear_diff_autoscroll_hover(area, state);
    area.queue_render();
}

fn install_clicks(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, spinner: &adw::Spinner) {
    let click_selection_mode = Rc::new(Cell::new(SelectionMode::Character));
    let drag_autoscroll_id = Rc::new(Cell::new(0_u64));
    let drag_autoscroll_pointer = Rc::new(Cell::new(None::<(f64, f64)>));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_pressed({
        let area = area.clone();
        let state = state.clone();
        let click_selection_mode = click_selection_mode.clone();
        move |gesture, n_press, x, y| {
            area.grab_focus();
            if state.middle_autoscroll.is_active() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
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
                    state.scrollbar_smooth_scroll.pause();
                    set_scroll_y(&area, &state, scroll_y);
                    canvas_scrollbar::set_active(
                        &area,
                        &state.scrollbar_hover,
                        &state.scrollbar_active,
                        &state.scrollbar_hover_progress,
                        &state.scrollbar_animating,
                        true,
                    );
                    return;
                }
            }

            if let Some(fold_index) = fold_index_at(&area, &state, y) {
                if let Some(callback) = state.fold_callback.borrow().as_ref().cloned() {
                    callback(fold_index);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
            }

            let mode = SelectionMode::for_press_count(n_press);
            click_selection_mode.set(mode);
            if let Some(point) = selection_point_at(&area, &state, x, y) {
                apply_click_selection(&area, &state, point, mode);
            } else {
                state.selection.borrow_mut().take();
                state.selection_drag.set(None);
                area.queue_render();
            }
        }
    });
    click.connect_released({
        let area = area.clone();
        let state = state.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, _, _, _| {
            state.scrollbar_drag.set(None);
            state.selection_drag.set(None);
            stop_diff_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
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
    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.connect_drag_begin({
        let area = area.clone();
        let state = state.clone();
        let spinner = spinner.clone();
        let click_selection_mode = click_selection_mode.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, x, y| {
            if state.middle_autoscroll.is_active() {
                return;
            }
            area.grab_focus();
            request_layout(&area, &state, &spinner);
            let width = area.allocated_width();
            let height = area.allocated_height();
            let total_height = state.content_height.get();
            if let Some(scroll_y) = canvas_scrollbar::scroll_for_lane_press(
                width,
                height,
                total_height,
                state.scroll_y.get(),
                x,
                y,
            ) {
                state.scrollbar_smooth_scroll.pause();
                set_scroll_y(&area, &state, scroll_y);
                state
                    .scrollbar_drag
                    .set(Some(canvas_scrollbar::Drag::new(state.scroll_y.get())));
                state.selection_drag.set(None);
                stop_diff_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
                canvas_scrollbar::set_active(
                    &area,
                    &state.scrollbar_hover,
                    &state.scrollbar_active,
                    &state.scrollbar_hover_progress,
                    &state.scrollbar_animating,
                    true,
                );
                return;
            }

            state.scrollbar_drag.set(None);
            let Some(point) = selection_point_at(&area, &state, x, y) else {
                state.selection_drag.set(None);
                stop_diff_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
                return;
            };
            let mode = click_selection_mode.get();
            state.active_side.set(point.side);
            state
                .selection_drag
                .set(Some(selection_drag_for_point(&area, &state, point, mode)));
        }
    });
    drag.connect_drag_update({
        let area = area.clone();
        let state = state.clone();
        let click_selection_mode = click_selection_mode.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |gesture, offset_x, offset_y| {
            if state.middle_autoscroll.is_active() {
                return;
            }
            if let Some(drag) = state.scrollbar_drag.get() {
                let Some((_, _, _, thumb_height)) = canvas_scrollbar::thumb_rect(
                    area.allocated_width(),
                    area.allocated_height(),
                    state.content_height.get(),
                    state.scroll_y.get(),
                ) else {
                    return;
                };
                let viewport_height = area.allocated_height().max(1) as f64;
                let max_scroll =
                    canvas_scrollbar::max_scroll(state.content_height.get(), viewport_height);
                state.scrollbar_smooth_scroll.pause();
                set_scroll_y(
                    &area,
                    &state,
                    drag.scroll_for_delta(offset_y, viewport_height, thumb_height, max_scroll),
                );
                return;
            }

            let Some((start_x, start_y)) = gesture.start_point() else {
                return;
            };
            let Some(drag) = state.selection_drag.get().or_else(|| {
                let point = selection_point_at(&area, &state, start_x, start_y)?;
                let mode = click_selection_mode.get();
                state.active_side.set(point.side);
                let drag = selection_drag_for_point(&area, &state, point, mode);
                state.selection_drag.set(Some(drag));
                Some(drag)
            }) else {
                return;
            };

            let pointer_x = start_x + offset_x;
            let pointer_y = start_y + offset_y;
            if let Some(focus) =
                selection_point_at_side(&area, &state, pointer_x, pointer_y, drag.anchor().side)
            {
                apply_drag_selection(&area, &state, drag, focus);
            }
            schedule_diff_drag_autoscroll(
                &area,
                &state,
                &drag_autoscroll_id,
                &drag_autoscroll_pointer,
                pointer_x,
                pointer_y,
                scroll_for_diff_drag_selection(&area, &state, pointer_y),
            );
        }
    });
    drag.connect_drag_end({
        let area = area.clone();
        let state = state.clone();
        let drag_autoscroll_id = drag_autoscroll_id.clone();
        let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
        move |_, _, _| {
            state.scrollbar_drag.set(None);
            state.selection_drag.set(None);
            stop_diff_drag_autoscroll(&drag_autoscroll_id, &drag_autoscroll_pointer);
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
    click.group_with(&drag);
    area.add_controller(click);
    area.add_controller(drag);

    let secondary_click = gtk::GestureClick::new();
    secondary_click.set_button(3);
    secondary_click.connect_pressed({
        let area = area.clone();
        let state = state.clone();
        move |gesture, _, x, y| {
            if state.middle_autoscroll.is_active() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                return;
            }
            show_context_menu(&area, &state, x, y);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    area.add_controller(secondary_click);
}

fn install_motion(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let area = area.clone();
        let state = state.clone();
        move |_, x, _| {
            if state.middle_autoscroll.is_active() {
                clear_diff_autoscroll_hover(&area, &state);
                return;
            }
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

fn install_key_shortcuts(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, spinner: &adw::Spinner) {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let area = area.clone();
        let state = state.clone();
        let spinner = spinner.clone();
        move |_, key, _, modifiers| {
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                && !modifiers.contains(gtk::gdk::ModifierType::ALT_MASK)
                && key == gtk::gdk::Key::c
            {
                copy_selection(&area, &state);
                return gtk::glib::Propagation::Stop;
            }
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                && !modifiers.contains(gtk::gdk::ModifierType::ALT_MASK)
                && key == gtk::gdk::Key::a
            {
                select_all_side(&area, &state, state.active_side.get());
                return gtk::glib::Propagation::Stop;
            }

            let Some(delta) = font_size_delta_for_key(key, modifiers) else {
                return gtk::glib::Propagation::Proceed;
            };
            let next = config::normalize_font_size(
                state.font_size.get() + delta,
                config::DEFAULT_DIFF_FONT_SIZE,
            );
            stop_diff_middle_autoscroll(&area, &state);
            state.font_size.set(next);
            state
                .text_width_cache
                .borrow_mut()
                .clear_for_font_size(canvas::font_size_key(next));
            config::save_diff_font_size(next);
            state
                .layout_generation
                .set(state.layout_generation.get().wrapping_add(1).max(1));
            state.layout_cache.borrow_mut().take();
            state.layout_pending_signature.borrow_mut().take();
            request_layout(&area, &state, &spinner);
            clamp_scroll(&area, &state);
            area.queue_render();
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(keys);
}

fn set_scroll_y(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, scroll_y: f64) {
    let max_scroll = state
        .layout_cache
        .borrow()
        .as_ref()
        .map_or(f64::INFINITY, |cache| {
            canvas_scrollbar::max_scroll(cache.content_height, area.allocated_height() as f64)
        });
    let next_scroll_y = scroll_y.max(0.0).min(max_scroll);
    if (state.scroll_y.get() - next_scroll_y).abs() <= f64::EPSILON {
        return;
    }
    state.scroll_y.set(next_scroll_y);
    area.queue_render();
}

fn apply_click_selection(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    point: DiffSelectionPoint,
    mode: SelectionMode,
) {
    state.active_side.set(point.side);
    state.selection_drag.set(None);
    let selection = selection_for_mode(
        point,
        mode,
        |point| word_bounds_at_point(state, point),
        |point| line_bounds_at_point(state, point),
    );
    state.selection.replace(Some(selection));
    area.queue_render();
}

fn selection_drag_for_point(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    point: DiffSelectionPoint,
    mode: SelectionMode,
) -> DragSelection<DiffSelectionPoint> {
    let (drag, selection) = drag_for_mode(
        point,
        mode,
        |point| word_bounds_at_point(state, point),
        |point| line_bounds_at_point(state, point),
    );
    state.selection.replace(Some(selection));
    area.queue_render();
    drag
}

fn apply_drag_selection(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    drag: DragSelection<DiffSelectionPoint>,
    focus: DiffSelectionPoint,
) {
    let selection = selection_for_drag(
        drag,
        focus,
        |focus| word_bounds_at_point(state, focus),
        |focus| line_bounds_at_point(state, focus),
    );
    state.selection.replace(Some(selection));
    area.queue_render();
}

fn scroll_for_diff_drag_selection(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    pointer_y: f64,
) -> bool {
    let viewport_height = area.allocated_height().max(1) as f64;
    let line_height = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    })
    .line_height;
    let zone = line_height * DRAG_AUTOSCROLL_ZONE_LINES;
    if zone <= f64::EPSILON || viewport_height <= f64::EPSILON {
        return false;
    }
    let before = state.scroll_y.get();

    if pointer_y < 0.0 {
        let overflow = -pointer_y;
        let delta = -(line_height * drag_autoscroll_lines_per_frame(overflow / zone));
        set_scroll_y(area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    if pointer_y > viewport_height {
        let overflow = pointer_y - viewport_height;
        let delta = line_height * drag_autoscroll_lines_per_frame(overflow / zone);
        set_scroll_y(area, state, state.scroll_y.get() + delta);
        return (state.scroll_y.get() - before).abs() > f64::EPSILON;
    }
    false
}

fn drag_autoscroll_lines_per_frame(ratio: f64) -> f64 {
    let ramp_ratio = ratio.clamp(0.0, 1.0);
    let outside_ratio = (ratio - 1.0).max(0.0);
    DRAG_AUTOSCROLL_MIN_LINES_PER_FRAME
        + ramp_ratio * DRAG_AUTOSCROLL_BASE_EXTRA_LINES_PER_FRAME
        + ramp_ratio.powi(3) * DRAG_AUTOSCROLL_AGGRESSIVE_EXTRA_LINES_PER_FRAME
        + outside_ratio * DRAG_AUTOSCROLL_OUTSIDE_EXTRA_LINES_PER_FRAME
}

fn schedule_diff_drag_autoscroll(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    drag_autoscroll_id: &Rc<Cell<u64>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
    pointer_x: f64,
    pointer_y: f64,
    should_scroll: bool,
) {
    if !should_scroll {
        stop_diff_drag_autoscroll(drag_autoscroll_id, drag_autoscroll_pointer);
        return;
    }

    drag_autoscroll_pointer.set(Some((pointer_x, pointer_y)));
    if drag_autoscroll_id.get() != 0 {
        return;
    }

    let next_id = drag_autoscroll_id.get().wrapping_add(1).max(1);
    drag_autoscroll_id.set(next_id);

    let area = area.clone();
    let state = state.clone();
    let drag_autoscroll_id = drag_autoscroll_id.clone();
    let drag_autoscroll_pointer = drag_autoscroll_pointer.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        if drag_autoscroll_id.get() != next_id {
            return gtk::glib::ControlFlow::Break;
        }

        let Some((x, y)) = drag_autoscroll_pointer.get() else {
            drag_autoscroll_id.set(0);
            return gtk::glib::ControlFlow::Break;
        };

        if !scroll_for_diff_drag_selection(&area, &state, y) {
            drag_autoscroll_id.set(0);
            return gtk::glib::ControlFlow::Break;
        }

        if let Some(drag) = state.selection_drag.get() {
            if let Some(focus) = selection_point_at_side(&area, &state, x, y, drag.anchor().side) {
                apply_drag_selection(&area, &state, drag, focus);
                return gtk::glib::ControlFlow::Continue;
            }
        }

        drag_autoscroll_id.set(0);
        gtk::glib::ControlFlow::Break
    });
}

fn stop_diff_drag_autoscroll(
    drag_autoscroll_id: &Rc<Cell<u64>>,
    drag_autoscroll_pointer: &Rc<Cell<Option<(f64, f64)>>>,
) {
    drag_autoscroll_id.set(0);
    drag_autoscroll_pointer.set(None);
}

fn show_context_menu(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, x: f64, y: f64) {
    let side = if x
        < (canvas_scrollbar::content_width(area.allocated_width())
            .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1))) as f64
            / 2.0)
            .floor()
    {
        DiffCanvasSide::Left
    } else {
        DiffCanvasSide::Right
    };

    let copy_enabled = selected_text(state).is_some_and(|text| !text.is_empty());
    let select_all_enabled = state
        .rows
        .borrow()
        .iter()
        .any(|row| !is_fold_row(row) && text_for_side(row, side).is_some());
    context_menu::popup_action_menu(
        area,
        x,
        y,
        vec![context_menu::ActionMenuSection::new(vec![
            context_menu::ActionMenuItem::new("Copy", DiffContextAction::Copy, copy_enabled),
            context_menu::ActionMenuItem::new(
                "Select All",
                DiffContextAction::SelectAll,
                select_all_enabled,
            ),
        ])],
        {
            let area = area.clone();
            let state = state.clone();

            move |action| match action {
                DiffContextAction::Copy => copy_selection(&area, &state),
                DiffContextAction::SelectAll => select_all_side(&area, &state, side),
            }
        },
    );
}

fn copy_selection(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let Some(text) = selected_text(state) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    area.display().clipboard().set_text(&text);
}

fn selected_text(state: &Rc<DiffCanvasState>) -> Option<String> {
    let selection = *state.selection.borrow();
    let selection = selection?;
    if selection.anchor.side != selection.focus.side {
        return None;
    }
    let (start, end) = selection.ordered()?;
    let rows = state.rows.borrow();
    let mut output = String::new();

    for row_index in start.row..=end.row {
        let Some(row) = rows.get(row_index) else {
            continue;
        };
        if is_fold_row(row) {
            continue;
        }
        let Some(text) = text_for_side(row, start.side) else {
            continue;
        };
        let text_start = if row_index == start.row {
            start.byte
        } else {
            0
        };
        let text_end = if row_index == end.row {
            end.byte
        } else {
            text.len()
        };
        let text_start = text_start.min(text.len());
        let text_end = text_end.min(text.len()).max(text_start);
        if !output.is_empty() {
            output.push('\n');
        }
        if text.is_char_boundary(text_start) && text.is_char_boundary(text_end) {
            output.push_str(&text[text_start..text_end]);
        }
    }

    Some(output)
}

fn select_all_side(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, side: DiffCanvasSide) {
    state.active_side.set(side);
    let rows = state.rows.borrow();
    let start = rows
        .iter()
        .enumerate()
        .find(|(_, row)| !is_fold_row(row) && text_for_side(row, side).is_some())
        .map(|(row, _)| DiffSelectionPoint { side, row, byte: 0 });
    let end = rows.iter().enumerate().rev().find_map(|(row, diff_row)| {
        if is_fold_row(diff_row) {
            return None;
        }
        let text = text_for_side(diff_row, side)?;
        Some(DiffSelectionPoint {
            side,
            row,
            byte: text.len(),
        })
    });
    drop(rows);

    if let (Some(anchor), Some(focus)) = (start, end) {
        state
            .selection
            .replace(Some(DiffSelection { anchor, focus }));
        area.queue_render();
    }
}

fn text_for_side(row: &FileDiffRow, side: DiffCanvasSide) -> Option<&str> {
    match side {
        DiffCanvasSide::Left => row.left_text.as_deref(),
        DiffCanvasSide::Right => row.right_text.as_deref(),
    }
}

fn rebuild_search_matches(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let query = state.search.borrow().query.clone();
    let matches = diff_search_matches(&state.rows.borrow(), &query);
    {
        let mut search = state.search.borrow_mut();
        search.matches = matches;
        search.active = (!search.matches.is_empty()).then_some(0);
    }
    select_active_search_match(area, state);
}

fn diff_search_matches(rows: &[FileDiffRow], query: &str) -> Vec<DiffSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        if is_fold_row(row) {
            continue;
        }
        if let Some(text) = row.left_text.as_deref() {
            push_text_search_matches(&mut matches, DiffCanvasSide::Left, row_index, text, query);
        }
        if let Some(text) = row.right_text.as_deref() {
            push_text_search_matches(&mut matches, DiffCanvasSide::Right, row_index, text, query);
        }
    }
    matches
}

fn push_text_search_matches(
    matches: &mut Vec<DiffSearchMatch>,
    side: DiffCanvasSide,
    row: usize,
    text: &str,
    query: &str,
) {
    let (haystack, needle) = if query.is_ascii() {
        (text.to_ascii_lowercase(), query.to_ascii_lowercase())
    } else {
        (text.to_string(), query.to_string())
    };
    let mut cursor = 0usize;
    while cursor <= haystack.len() {
        let Some(relative) = haystack[cursor..].find(&needle) else {
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            matches.push(DiffSearchMatch {
                side,
                row,
                start,
                end,
            });
            cursor = end.max(start.saturating_add(1));
        } else {
            cursor = start.saturating_add(1).min(text.len());
            while cursor < text.len() && !text.is_char_boundary(cursor) {
                cursor += 1;
            }
        }
    }
}

fn select_active_search_match(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let search_match = {
        let search = state.search.borrow();
        search
            .active
            .and_then(|active| search.matches.get(active).copied())
    };
    let Some(search_match) = search_match else {
        area.queue_render();
        return;
    };

    state.active_side.set(search_match.side);
    state.selection.replace(Some(DiffSelection {
        anchor: DiffSelectionPoint {
            side: search_match.side,
            row: search_match.row,
            byte: search_match.start,
        },
        focus: DiffSelectionPoint {
            side: search_match.side,
            row: search_match.row,
            byte: search_match.end,
        },
    }));
    ensure_search_match_visible(area, state, search_match);
}

fn ensure_search_match_visible(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    search_match: DiffSearchMatch,
) {
    let viewport_height = area.allocated_height().max(1) as f64;
    let line_height = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    })
    .line_height;
    let Some((target_y, target_height)) = state
        .layout_cache
        .borrow()
        .as_ref()
        .and_then(|cache| search_match_visual_bounds(cache, search_match, line_height))
    else {
        return;
    };

    let scroll_y = state.scroll_y.get();
    if target_y < scroll_y {
        set_scroll_y(area, state, target_y);
    } else if target_y + target_height > scroll_y + viewport_height {
        set_scroll_y(area, state, target_y + target_height - viewport_height);
    }
}

fn search_match_visual_bounds(
    cache: &DiffLayoutCache,
    search_match: DiffSearchMatch,
    line_height: f64,
) -> Option<(f64, f64)> {
    let layout = cache.rows.get(search_match.row)?;
    let lines = match search_match.side {
        DiffCanvasSide::Left => &layout.left_lines,
        DiffCanvasSide::Right => &layout.right_lines,
    };
    let visual_line = lines
        .iter()
        .position(|line| search_match.start < line.end && search_match.end > line.start)
        .unwrap_or(0);
    Some((layout.y + visual_line as f64 * line_height, line_height))
}

fn word_bounds_at_point(
    state: &Rc<DiffCanvasState>,
    point: DiffSelectionPoint,
) -> Option<(DiffSelectionPoint, DiffSelectionPoint)> {
    let rows = state.rows.borrow();
    let text = text_for_side(rows.get(point.row)?, point.side)?;
    let (start, end) = text_word_bounds_at(text, point.byte)?;
    Some((
        DiffSelectionPoint {
            byte: start,
            ..point
        },
        DiffSelectionPoint { byte: end, ..point },
    ))
}

fn line_bounds_at_point(
    state: &Rc<DiffCanvasState>,
    point: DiffSelectionPoint,
) -> Option<(DiffSelectionPoint, DiffSelectionPoint)> {
    let rows = state.rows.borrow();
    let text = text_for_side(rows.get(point.row)?, point.side)?;
    Some((
        DiffSelectionPoint { byte: 0, ..point },
        DiffSelectionPoint {
            byte: text.len(),
            ..point
        },
    ))
}

fn clamp_scroll(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) {
    let Some(content_height) = state
        .layout_cache
        .borrow()
        .as_ref()
        .map(|cache| cache.content_height)
    else {
        return;
    };
    let max_scroll = canvas_scrollbar::max_scroll(content_height, area.allocated_height() as f64);
    state
        .scroll_y
        .set(state.scroll_y.get().clamp(0.0, max_scroll));
}

fn fold_index_at(_area: &gtk::GLArea, state: &Rc<DiffCanvasState>, y: f64) -> Option<usize> {
    let document_y = y + state.scroll_y.get();
    let cache = state.layout_cache.borrow();
    let row_index = cache
        .as_ref()
        .and_then(|cache| diff_layout::row_index_at_y(cache, document_y))?;
    let rows = state.rows.borrow();
    let row = rows.get(row_index)?;
    is_fold_row(row).then(|| row.left_number).flatten()
}

fn selection_point_at(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    x: f64,
    y: f64,
) -> Option<DiffSelectionPoint> {
    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let half_width = (content_width as f64 / 2.0).floor();
    let side = if x < half_width {
        DiffCanvasSide::Left
    } else {
        DiffCanvasSide::Right
    };
    selection_point_at_side(area, state, x, y, side)
}

fn selection_point_at_side(
    area: &gtk::GLArea,
    state: &Rc<DiffCanvasState>,
    x: f64,
    y: f64,
    side: DiffCanvasSide,
) -> Option<DiffSelectionPoint> {
    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let half_width = (content_width as f64 / 2.0).floor();
    let rows = state.rows.borrow();
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let gutter_width = gutter_width(area, state);
    let document_y = y + state.scroll_y.get();
    let cache = state.layout_cache.borrow();
    let cache = cache.as_ref()?;
    let row_index = diff_layout::row_index_at_y(cache, document_y)?;
    let row = rows.get(row_index)?;
    if is_fold_row(row) {
        return None;
    }
    let text = text_for_side(row, side)?;
    let layout = cache.rows.get(row_index)?;
    let side_x = match side {
        DiffCanvasSide::Left => 0.0,
        DiffCanvasSide::Right => half_width + DIVIDER_WIDTH,
    };
    let text_x = match side {
        DiffCanvasSide::Left => side_x + CELL_PADDING,
        DiffCanvasSide::Right => side_x + gutter_width + CELL_PADDING,
    };
    let lines = match side {
        DiffCanvasSide::Left => &layout.left_lines,
        DiffCanvasSide::Right => &layout.right_lines,
    };
    if lines.is_empty() {
        return None;
    }
    let visual_line = ((document_y - layout.y) / metrics.line_height)
        .floor()
        .max(0.0) as usize;
    let line = lines.get(visual_line.min(lines.len().saturating_sub(1)))?;
    let byte = line.start + byte_for_x(area, state, &line.text, x - text_x);
    Some(DiffSelectionPoint {
        side,
        row: row_index,
        byte: byte.min(line.end).min(text.len()),
    })
}

fn cached_text_width_for_state(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, text: &str) -> f64 {
    let mut cache = state.text_width_cache.borrow_mut();
    canvas::cached_text_width(area, state.font_size.get(), &mut cache, text)
}

fn byte_for_x(area: &gtk::GLArea, state: &Rc<DiffCanvasState>, text: &str, x: f64) -> usize {
    if x <= 0.0 || text.is_empty() {
        return 0;
    }

    let mut width = 0.0;
    for (byte, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = cached_text_width_for_state(area, state, grapheme);
        if x < width + grapheme_width / 2.0 {
            return byte;
        }
        width += grapheme_width;
    }
    text.len()
}

fn gutter_width(area: &gtk::GLArea, state: &Rc<DiffCanvasState>) -> f64 {
    let max_number = state.max_line_number.get().max(1).to_string();
    cached_text_width_for_state(area, state, &max_number) + PREFIX_WIDTH + CELL_PADDING * 2.0
}

fn fill_rect(
    context: &skia_canvas::Context,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color: Color,
) {
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

fn is_fold_row(row: &FileDiffRow) -> bool {
    row.left_kind == DiffKind::Fold || row.right_kind == DiffKind::Fold
}

fn diff_rows_equal(left: &[FileDiffRow], right: &[FileDiffRow]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right.iter()).all(|(left, right)| {
            left.left_number == right.left_number
                && left.right_number == right.right_number
                && left.left_text == right.left_text
                && left.right_text == right.right_text
                && left.left_kind == right.left_kind
                && left.right_kind == right.right_kind
        })
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

fn theme_for(area: &gtk::GLArea) -> DiffCanvasTheme {
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
            fold_background: Color::rgba(0.14, 0.14, 0.16, 1.0),
            fold_text: Color::rgba(0.62, 0.66, 0.72, 1.0),
            search_match_background: Color::rgba(0.75, 0.56, 0.12, 0.36),
            selection_background: Color::rgba(0.26, 0.43, 0.68, 0.42),
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
            fold_background: Color::rgba(0.90, 0.92, 0.95, 1.0),
            fold_text: Color::rgba(0.30, 0.34, 0.40, 1.0),
            search_match_background: Color::rgba(1.0, 0.78, 0.22, 0.42),
            selection_background: Color::rgba(0.48, 0.66, 0.92, 0.42),
        }
    }
}
