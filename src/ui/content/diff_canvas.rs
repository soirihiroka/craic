use super::{
    canvas_overshoot,
    code_editor::{
        canvas::{self, TextColor},
        selection::{AnchoredSelection, clipped_bounds},
    },
};
use crate::config;
use crate::git::{DiffKind, FileDiffRow};
use crate::language_support::{HighlightRange, SyntaxHighlighter, language_hint_from_path};
use crate::ui::canvas_scrollbar;
use adw::prelude::*;
use gtk::cairo;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
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
    overshoot: canvas_overshoot::EdgeGlow,
    scrollbar_hover: Rc<Cell<bool>>,
    scrollbar_active: Rc<Cell<bool>>,
    scrollbar_hover_progress: Rc<Cell<f64>>,
    scrollbar_animating: Rc<Cell<bool>>,
    scrollbar_drag: Cell<Option<canvas_scrollbar::Drag>>,
    fold_callback: RefCell<Option<Rc<dyn Fn(usize)>>>,
    font_size_adjust_callback: RefCell<Option<Rc<dyn Fn(f64)>>>,
    layout_generation: Cell<u64>,
    layout_cache: RefCell<Option<DiffLayoutCache>>,
    max_line_number: Cell<usize>,
    fold_row_count: Cell<usize>,
    syntax: RefCell<Option<DiffSyntaxState>>,
    syntax_signature: RefCell<Option<DiffSyntaxSignature>>,
    selection: RefCell<Option<DiffSelection>>,
    selection_drag: Cell<Option<DiffSelectionPoint>>,
    active_side: Cell<DiffCanvasSide>,
    last_layout_log: RefCell<Option<LayoutLogSnapshot>>,
}

#[derive(Clone)]
struct RowLayout {
    y: f64,
    height: f64,
    left_lines: Vec<WrappedLine>,
    right_lines: Vec<WrappedLine>,
}

#[derive(Clone)]
struct WrappedLine {
    text: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
struct CachedRowLayout {
    y: f64,
    height: f64,
    shared_visual_line_count: usize,
    measured: bool,
}

struct DiffLayoutCache {
    generation: u64,
    content_width: i32,
    gutter_width_bits: u64,
    line_height_bits: u64,
    rows: Vec<CachedRowLayout>,
    markers: Vec<CachedScrollbarMarker>,
    measured_rows: usize,
    max_shared_visual_line_count: usize,
    content_height: f64,
}

#[derive(Clone, Copy)]
struct CachedScrollbarMarker {
    row: usize,
    kind: canvas_scrollbar::MarkerKind,
}

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

#[derive(Clone, Copy, PartialEq)]
struct LayoutLogSnapshot {
    rows: usize,
    folds: usize,
    measured_rows: usize,
    content_width: i32,
    content_height_bits: u64,
    max_shared_visual_line_count: usize,
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
    selection_background: Color,
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
            overshoot: canvas_overshoot::EdgeGlow::new(),
            scrollbar_hover: Rc::new(Cell::new(false)),
            scrollbar_active: Rc::new(Cell::new(false)),
            scrollbar_hover_progress: Rc::new(Cell::new(0.0)),
            scrollbar_animating: Rc::new(Cell::new(false)),
            scrollbar_drag: Cell::new(None),
            fold_callback: RefCell::new(None),
            font_size_adjust_callback: RefCell::new(None),
            layout_generation: Cell::new(1),
            layout_cache: RefCell::new(None),
            max_line_number: Cell::new(1),
            fold_row_count: Cell::new(0),
            syntax: RefCell::new(None),
            syntax_signature: RefCell::new(None),
            selection: RefCell::new(None),
            selection_drag: Cell::new(None),
            active_side: Cell::new(DiffCanvasSide::Right),
            last_layout_log: RefCell::new(None),
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
        install_key_shortcuts(&area, &state);

        Self { root: area, state }
    }

    pub(in crate::ui) fn set_rows(&self, rows: Vec<FileDiffRow>) {
        let fold_rows = rows.iter().filter(|row| is_fold_row(row)).count();
        let max_line_number = rows
            .iter()
            .flat_map(|row| [row.left_number, row.right_number])
            .flatten()
            .max()
            .unwrap_or(1);
        log::debug!(
            "diff_canvas set_rows rows={} fold_rows={}",
            rows.len(),
            fold_rows
        );
        self.state.fold_row_count.set(fold_rows);
        self.state.max_line_number.set(max_line_number);
        self.state.rows.replace(rows);
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.last_layout_log.borrow_mut().take();
        refresh_viewport_layout_cache(&self.root, &self.state);
        clamp_scroll(&self.root, &self.state);
        self.root.queue_draw();
    }

    pub(in crate::ui) fn set_syntax_for_file(
        &self,
        file_path: &str,
        fingerprint: u64,
        full_rows: &[FileDiffRow],
    ) {
        update_syntax_state(&self.state, file_path, fingerprint, full_rows);
    }

    pub(in crate::ui) fn clear(&self) {
        self.state.rows.borrow_mut().clear();
        self.state.scroll_y.set(0.0);
        self.state.max_line_number.set(1);
        self.state.fold_row_count.set(0);
        self.state.syntax.borrow_mut().take();
        self.state.syntax_signature.borrow_mut().take();
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.last_layout_log.borrow_mut().take();
        refresh_viewport_layout_cache(&self.root, &self.state);
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
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.last_layout_log.borrow_mut().take();
        refresh_viewport_layout_cache(&self.root, &self.state);
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
    let start = Instant::now();
    let left = build_syntax_side(&language, rows, DiffCanvasSide::Left);
    let right = build_syntax_side(&language, rows, DiffCanvasSide::Right);
    log::debug!(
        "diff_canvas syntax refresh path={} rows={} language={} left_bytes={} left_highlights={} right_bytes={} right_highlights={} duration_ms={}",
        file_path,
        rows.len(),
        language,
        left.source.len(),
        left.highlights.len(),
        right.source.len(),
        right.highlights.len(),
        start.elapsed().as_millis()
    );
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
    area: &gtk::DrawingArea,
    context: &cairo::Context,
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
    let visible_layouts = visible_row_layouts(
        area,
        state,
        rows.as_slice(),
        content_width,
        gutter_width,
        metrics.line_height,
        height,
    );
    let content_height = state
        .layout_cache
        .borrow()
        .as_ref()
        .map(|cache| cache.content_height)
        .unwrap_or(metrics.line_height)
        .max(metrics.line_height);
    state.content_height.set(content_height);
    log_layout_change(state, rows.as_slice(), content_width, content_height);
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

    for (index, layout) in visible_layouts {
        let Some(row) = rows.get(index) else {
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
            &layout,
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
}

fn visible_row_layouts(
    area: &gtk::DrawingArea,
    state: &Rc<DiffCanvasState>,
    rows: &[FileDiffRow],
    content_width: i32,
    gutter_width: f64,
    line_height: f64,
    viewport_height: i32,
) -> Vec<(usize, RowLayout)> {
    let generation = state.layout_generation.get();
    let gutter_width_bits = gutter_width.to_bits();
    let line_height_bits = line_height.to_bits();
    let needs_cache = state.layout_cache.borrow().as_ref().is_none_or(|cache| {
        cache.generation != generation
            || cache.content_width != content_width
            || cache.gutter_width_bits != gutter_width_bits
            || cache.line_height_bits != line_height_bits
            || cache.rows.len() != rows.len()
    });
    if needs_cache {
        let estimated_rows = rows
            .iter()
            .map(|row| estimated_visual_line_count(row).max(1))
            .collect::<Vec<_>>();
        let markers = rows
            .iter()
            .enumerate()
            .filter_map(|(row, diff_row)| {
                marker_kind(diff_row).map(|kind| CachedScrollbarMarker { row, kind })
            })
            .collect::<Vec<_>>();
        let mut cached_rows = Vec::with_capacity(rows.len());
        let mut y = 0.0;
        for shared_visual_line_count in estimated_rows {
            let height = shared_visual_line_count as f64 * line_height;
            cached_rows.push(CachedRowLayout {
                y,
                height,
                shared_visual_line_count,
                measured: false,
            });
            y += height;
        }
        log::debug!(
            "diff_canvas layout_cache rebuild rows={} content_width={} line_height={:.1}",
            rows.len(),
            content_width,
            line_height
        );
        state.layout_cache.replace(Some(DiffLayoutCache {
            generation,
            content_width,
            gutter_width_bits,
            line_height_bits,
            rows: cached_rows,
            markers,
            measured_rows: 0,
            max_shared_visual_line_count: 1,
            content_height: y.max(line_height),
        }));
    }

    let scroll_y = state.scroll_y.get();
    let visible_range = {
        let cache = state.layout_cache.borrow();
        let Some(cache) = cache.as_ref() else {
            return Vec::new();
        };
        visible_row_range(cache, scroll_y, viewport_height as f64, line_height * 8.0)
    };
    let half_width = (content_width as f64 / 2.0).floor();
    let text_width = (half_width - gutter_width - PREFIX_WIDTH - CELL_PADDING * 2.0).max(
        canvas::text_width_for_size(area, state.font_size.get(), "0"),
    );
    let mut layouts = Vec::with_capacity(visible_range.len());
    let mut changed = false;
    let mut cache = state.layout_cache.borrow_mut();
    let Some(cache) = cache.as_mut() else {
        return layouts;
    };

    for index in visible_range {
        let Some(row) = rows.get(index) else {
            continue;
        };
        let left_lines = wrap_text(
            area,
            state,
            row.left_text.as_deref().unwrap_or_default(),
            text_width,
        );
        let right_lines = wrap_text(
            area,
            state,
            row.right_text.as_deref().unwrap_or_default(),
            text_width,
        );
        let shared_visual_line_count = left_lines.len().max(right_lines.len()).max(1);
        let height = shared_visual_line_count as f64 * line_height;
        let was_measured = cache.rows.get(index).is_some_and(|cached| cached.measured);
        if let Some(cached) = cache.rows.get(index) {
            changed |= !was_measured
                || cached.shared_visual_line_count != shared_visual_line_count
                || (cached.height - height).abs() > f64::EPSILON;
        }
        update_cached_row_layout(cache, index, height, shared_visual_line_count);
        if !was_measured {
            cache.measured_rows += 1;
        }
        cache.max_shared_visual_line_count = cache
            .max_shared_visual_line_count
            .max(shared_visual_line_count);
        if let Some(cached) = cache.rows.get(index) {
            layouts.push((
                index,
                RowLayout {
                    y: cached.y,
                    height: cached.height,
                    left_lines,
                    right_lines,
                },
            ));
        }
    }

    if changed {
        log::debug!(
            "diff_canvas layout_cache measured visible_rows={} content_height={:.1}",
            layouts.len(),
            cache.content_height
        );
        area.queue_draw();
    }
    layouts
}

fn estimated_visual_line_count(row: &FileDiffRow) -> usize {
    let left = row
        .left_text
        .as_deref()
        .map(|text| text.len() / 120 + 1)
        .unwrap_or(1);
    let right = row
        .right_text
        .as_deref()
        .map(|text| text.len() / 120 + 1)
        .unwrap_or(1);
    left.max(right)
}

fn visible_row_range(
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

fn update_cached_row_layout(
    cache: &mut DiffLayoutCache,
    index: usize,
    height: f64,
    shared_visual_line_count: usize,
) {
    let Some(cached) = cache.rows.get_mut(index) else {
        return;
    };
    let delta = height - cached.height;
    cached.height = height;
    cached.shared_visual_line_count = shared_visual_line_count;
    cached.measured = true;
    if delta.abs() <= f64::EPSILON {
        return;
    }

    for layout in cache.rows.iter_mut().skip(index + 1) {
        layout.y += delta;
    }
    cache.content_height = (cache.content_height + delta).max(height);
}

fn draw_row(
    area: &gtk::DrawingArea,
    context: &cairo::Context,
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
    area: &gtk::DrawingArea,
    context: &cairo::Context,
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
        let number_width = canvas::text_width_for_size(area, state.font_size.get(), &number);
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
        if let Some((selection_start, selection_end)) =
            selection_range_for_wrapped_line(selection, side, row_index, line)
        {
            let prefix = line.text.get(..selection_start).unwrap_or_default();
            let selected = line
                .text
                .get(selection_start..selection_end)
                .unwrap_or_default();
            let selection_x =
                text_x + canvas::text_width_for_size(area, state.font_size.get(), prefix);
            let selection_width =
                canvas::text_width_for_size(area, state.font_size.get(), selected).max(2.0);
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
                state.font_size.get(),
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
    area: &gtk::DrawingArea,
    context: &cairo::Context,
    font_size: f64,
    syntax: &DiffSyntaxSide,
    line: &WrappedLine,
    source_start: usize,
    source_end: usize,
    mut x: f64,
    baseline: f64,
    fallback: TextColor,
) {
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
            x += canvas::text_width_for_size(area, font_size, plain);
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
        x += canvas::text_width_for_size(area, font_size, segment);
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
) {
    let total_height = state.content_height.get();
    let hover = state.scrollbar_hover_progress.get();
    let theme = canvas_scrollbar::Theme::for_widget(area);
    canvas_scrollbar::draw_track(context, width, height, total_height, hover, theme);
    if let Some(cache) = state.layout_cache.borrow().as_ref() {
        draw_scrollbar_markers(context, width, height, total_height, hover, cache);
    }
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

fn install_scroll(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let area = area.clone();
        let state = state.clone();
        move |_, _, dy| {
            let line_height =
                canvas::measure_font_metrics(&area, state.font_size.get(), |font_size| {
                    (font_size + 9.0).ceil()
                })
                .line_height;
            let delta = dy * line_height * 3.0;
            let viewport_height = area.allocated_height().max(1) as f64;
            canvas_overshoot::pull_for_delta(
                &area,
                &state.overshoot,
                state.scroll_y.get(),
                canvas_scrollbar::max_scroll(state.content_height.get(), viewport_height),
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

            if let Some(point) = selection_point_at(&area, &state, x, y) {
                state.active_side.set(point.side);
                state.selection_drag.set(Some(point));
                state.selection.replace(Some(DiffSelection {
                    anchor: point,
                    focus: point,
                }));
                area.queue_draw();
                gesture.set_state(gtk::EventSequenceState::Claimed);
            } else {
                state.selection.borrow_mut().take();
                state.selection_drag.set(None);
                area.queue_draw();
            }
        }
    });
    click.connect_released({
        let area = area.clone();
        let state = state.clone();
        move |_, _, _, _| {
            state.scrollbar_drag.set(None);
            state.selection_drag.set(None);
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
    drag.set_button(1);
    drag.connect_drag_begin({
        let area = area.clone();
        let state = state.clone();
        move |_, x, y| {
            area.grab_focus();
            refresh_viewport_layout_cache(&area, &state);
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
                log::debug!(
                    "diff_canvas drag_begin scrollbar x={x:.1} y={y:.1} scroll_y={scroll_y:.1}"
                );
                set_scroll_y(&area, &state, scroll_y);
                state
                    .scrollbar_drag
                    .set(Some(canvas_scrollbar::Drag::new(state.scroll_y.get())));
                state.selection_drag.set(None);
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
        }
    });
    drag.connect_drag_update({
        let area = area.clone();
        let state = state.clone();
        move |gesture, offset_x, offset_y| {
            if let Some(drag) = state.scrollbar_drag.get() {
                refresh_viewport_layout_cache(&area, &state);
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
                set_scroll_y(
                    &area,
                    &state,
                    drag.scroll_for_delta(offset_y, viewport_height, thumb_height, max_scroll),
                );
                return;
            }

            let Some((_, start_y)) = gesture.start_point() else {
                return;
            };
            if let Some(anchor) = state.selection_drag.get() {
                if let Some((start_x, _)) = gesture.start_point() {
                    if let Some(mut focus) =
                        selection_point_at(&area, &state, start_x + offset_x, start_y + offset_y)
                    {
                        focus.side = anchor.side;
                        state
                            .selection
                            .replace(Some(DiffSelection { anchor, focus }));
                        area.queue_draw();
                    }
                }
                return;
            }
        }
    });
    drag.connect_drag_end({
        let area = area.clone();
        let state = state.clone();
        move |_, _, _| {
            state.scrollbar_drag.set(None);
            state.selection_drag.set(None);
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

fn install_key_shortcuts(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let area = area.clone();
        let state = state.clone();
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
                select_all_active_side(&area, &state);
                return gtk::glib::Propagation::Stop;
            }

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
                state
                    .layout_generation
                    .set(state.layout_generation.get().wrapping_add(1).max(1));
                state.layout_cache.borrow_mut().take();
                state.last_layout_log.borrow_mut().take();
                clamp_scroll(&area, &state);
                area.queue_draw();
            }
            gtk::glib::Propagation::Stop
        }
    });
    area.add_controller(keys);
}

fn set_scroll_y(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>, scroll_y: f64) {
    refresh_viewport_layout_cache(area, state);
    let max_scroll =
        canvas_scrollbar::max_scroll(state.content_height.get(), area.allocated_height() as f64);
    state.scroll_y.set(scroll_y.clamp(0.0, max_scroll));
    area.queue_draw();
}

fn refresh_viewport_layout_cache(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let rows = state.rows.borrow();
    if rows.is_empty() {
        state.content_height.set(1.0);
        return;
    }
    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let gutter_width = gutter_width(area, state);
    let _ = visible_row_layouts(
        area,
        state,
        rows.as_slice(),
        content_width,
        gutter_width,
        metrics.line_height,
        area.allocated_height(),
    );
    let content_height = state
        .layout_cache
        .borrow()
        .as_ref()
        .map(|cache| cache.content_height)
        .unwrap_or(metrics.line_height)
        .max(metrics.line_height);
    state.content_height.set(content_height);
}

fn copy_selection(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let Some(text) = selected_text(state) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    area.display().clipboard().set_text(&text);
    log::debug!("diff_canvas copied selection bytes={}", text.len());
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

fn select_all_active_side(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let side = state.active_side.get();
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
        area.queue_draw();
    }
}

fn text_for_side(row: &FileDiffRow, side: DiffCanvasSide) -> Option<&str> {
    match side {
        DiffCanvasSide::Left => row.left_text.as_deref(),
        DiffCanvasSide::Right => row.right_text.as_deref(),
    }
}

fn clamp_scroll(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) {
    let max_scroll =
        canvas_scrollbar::max_scroll(state.content_height.get(), area.allocated_height() as f64);
    state
        .scroll_y
        .set(state.scroll_y.get().clamp(0.0, max_scroll));
}

fn fold_index_at(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>, y: f64) -> Option<usize> {
    let content_width = canvas_scrollbar::content_width(area.allocated_width())
        .max(MIN_CONTENT_WIDTH.min(area.allocated_width().max(1)));
    let rows = state.rows.borrow();
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let _ = visible_row_layouts(
        area,
        state,
        rows.as_slice(),
        content_width,
        gutter_width(area, state),
        metrics.line_height,
        area.allocated_height(),
    );
    let document_y = y + state.scroll_y.get();
    let row_index = state
        .layout_cache
        .borrow()
        .as_ref()
        .and_then(|cache| row_index_at_y(cache, document_y))?;
    let row = rows.get(row_index)?;
    let fold_index = is_fold_row(row).then(|| row.left_number).flatten();
    if let Some(fold_index) = fold_index {
        log::debug!("diff_canvas fold_hit row_index={row_index} fold_index={fold_index}");
    }
    fold_index
}

fn row_index_at_y(cache: &DiffLayoutCache, document_y: f64) -> Option<usize> {
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

fn selection_point_at(
    area: &gtk::DrawingArea,
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
    let rows = state.rows.borrow();
    let metrics = canvas::measure_font_metrics(area, state.font_size.get(), |font_size| {
        (font_size + 9.0).ceil()
    });
    let gutter_width = gutter_width(area, state);
    let layouts = visible_row_layouts(
        area,
        state,
        rows.as_slice(),
        content_width,
        gutter_width,
        metrics.line_height,
        area.allocated_height(),
    );
    let document_y = y + state.scroll_y.get();
    let row_index = state
        .layout_cache
        .borrow()
        .as_ref()
        .and_then(|cache| row_index_at_y(cache, document_y))?;
    let row = rows.get(row_index)?;
    if is_fold_row(row) {
        return None;
    }
    let layout = layouts
        .iter()
        .find(|(index, _)| *index == row_index)
        .map(|(_, layout)| layout)?;
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
    let byte = line.start + byte_for_x(area, state.font_size.get(), &line.text, x - text_x);
    Some(DiffSelectionPoint {
        side,
        row: row_index,
        byte: byte.min(line.end),
    })
}

fn byte_for_x(area: &gtk::DrawingArea, font_size: f64, text: &str, x: f64) -> usize {
    if x <= 0.0 || text.is_empty() {
        return 0;
    }

    let mut width = 0.0;
    for (byte, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = canvas::text_width_for_size(area, font_size, grapheme);
        if x < width + grapheme_width / 2.0 {
            return byte;
        }
        width += grapheme_width;
    }
    text.len()
}

fn wrap_text(
    area: &gtk::DrawingArea,
    state: &Rc<DiffCanvasState>,
    text: &str,
    wrap_width: f64,
) -> Vec<WrappedLine> {
    if text.is_empty() {
        return vec![WrappedLine {
            text: String::new(),
            start: 0,
            end: 0,
        }];
    }
    let mut lines = Vec::new();
    let mut segment_start = 0;
    let mut line_width = 0.0;
    for (byte, grapheme) in text.grapheme_indices(true) {
        let grapheme_width = canvas::text_width_for_size(area, state.font_size.get(), grapheme);
        if segment_start < byte && line_width + grapheme_width > wrap_width {
            lines.push(WrappedLine {
                text: text[segment_start..byte].to_string(),
                start: segment_start,
                end: byte,
            });
            segment_start = byte;
            line_width = 0.0;
        }
        line_width += grapheme_width;
    }
    lines.push(WrappedLine {
        text: text[segment_start..].to_string(),
        start: segment_start,
        end: text.len(),
    });
    lines
}

fn gutter_width(area: &gtk::DrawingArea, state: &Rc<DiffCanvasState>) -> f64 {
    let max_number = state.max_line_number.get().max(1).to_string();
    canvas::text_width_for_size(area, state.font_size.get(), &max_number)
        + PREFIX_WIDTH
        + CELL_PADDING * 2.0
}

fn log_layout_change(
    state: &Rc<DiffCanvasState>,
    rows: &[FileDiffRow],
    content_width: i32,
    content_height: f64,
) {
    let (measured_rows, max_shared_visual_line_count) = state
        .layout_cache
        .borrow()
        .as_ref()
        .map(|cache| (cache.measured_rows, cache.max_shared_visual_line_count))
        .unwrap_or((0, 0));
    let snapshot = LayoutLogSnapshot {
        rows: rows.len(),
        folds: state.fold_row_count.get(),
        measured_rows,
        content_width,
        content_height_bits: content_height.to_bits(),
        max_shared_visual_line_count,
    };
    let mut last = state.last_layout_log.borrow_mut();
    if last.as_ref() == Some(&snapshot) {
        return;
    }
    log::debug!(
        "diff_canvas row_layout rows={} fold_rows={} measured_rows={} content_width={} content_height={:.1} max_shared_visual_lines={}",
        snapshot.rows,
        snapshot.folds,
        snapshot.measured_rows,
        snapshot.content_width,
        content_height,
        snapshot.max_shared_visual_line_count
    );
    *last = Some(snapshot);
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
            missing_background: Color::rgba(0.92, 0.92, 0.93, 1.0),
            fold_background: Color::rgba(0.90, 0.92, 0.95, 1.0),
            fold_text: Color::rgba(0.30, 0.34, 0.40, 1.0),
            selection_background: Color::rgba(0.48, 0.66, 0.92, 0.42),
        }
    }
}
