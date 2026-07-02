pub(in crate::ui) mod canvas;
mod diff_document;
mod input;
mod render;
pub(in crate::ui) mod selection;
mod text_buffer;

use super::canvas_overshoot;
use crate::config;
use crate::git::{DiffKind, FileComparison};
use crate::language_support::{
    CompletionItem, CompletionSet, HighlightRange, SyntaxHighlighter, SyntaxIssue,
    apply_edit_to_ranges,
};
use crate::spellcheck::SpellcheckIssue;
use crate::ui::canvas_scroll;
use crate::ui::components::search::SearchPanel;
use adw::prelude::*;
use gtk::gdk;
use selection::Selection;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;
use text_buffer::{TextBuffer, TextWidthCache, clamp_to_char_boundary, next_char_boundary};

pub(in crate::ui) use crate::language_support::language_hint_from_path;
pub(in crate::ui) use diff_document::{
    DiffEditorDocument, DiffEditorRow, EditorDiffKind, ScrollbarMarker, ScrollbarMarkerKind,
};
pub(in crate::ui) use text_buffer::byte_offset_for_line_column;

const CELL_PADDING: f64 = 8.0;
const DIFF_PREFIX_WIDTH: f64 = 22.0;
const MIN_CONTENT_WIDTH: i32 = 240;
const MAX_HISTORY_SNAPSHOTS: usize = 200;

#[derive(Clone)]
pub(in crate::ui) struct CodeEditor {
    pub(in crate::ui) root: gtk::Box,
    area: gtk::DrawingArea,
    search_panel: SearchPanel,
    state: Rc<EditorState>,
}

type EditCallback = Rc<dyn Fn()>;
type ScrollCallback = Rc<dyn Fn(f64)>;
type DiffFoldCallback = Rc<dyn Fn(usize)>;
type FontSizeAdjustCallback = Rc<dyn Fn(f64)>;

struct EditorState {
    text: RefCell<TextBuffer>,
    syntax_source: RefCell<Option<String>>,
    language: RefCell<String>,
    font_size: Cell<f64>,
    char_width: Cell<f64>,
    char_spacing: Cell<f64>,
    line_height: Cell<f64>,
    baseline_offset: Cell<f64>,
    editable: Cell<bool>,
    wrap: Cell<bool>,
    scrollbar_visible: Cell<bool>,
    gutter_side: Cell<GutterSide>,
    auto_folding_enabled: Cell<bool>,
    selection: RefCell<Option<Selection>>,
    cursor: Cell<usize>,
    preedit: RefCell<String>,
    folds: RefCell<Vec<FoldRange>>,
    fold_generation: Cell<u64>,
    diff_rows: RefCell<Option<Vec<DiffEditorRow>>>,
    scrollbar_markers: RefCell<Vec<ScrollbarMarker>>,
    scroll_x: Cell<f64>,
    scroll_y: Cell<f64>,
    scrollbar_hover: Rc<Cell<bool>>,
    scrollbar_active: Rc<Cell<bool>>,
    scrollbar_hover_progress: Rc<Cell<f64>>,
    scrollbar_animating: Rc<Cell<bool>>,
    middle_autoscroll: Rc<canvas_scroll::MiddleAutoscroll>,
    fold_hovered: Cell<Option<FoldControlKey>>,
    fold_pressed: Cell<Option<FoldControlKey>>,
    fold_hover_progress: Cell<f64>,
    fold_hover_animating: Cell<bool>,
    fold_icon_states: RefCell<Vec<FoldIconState>>,
    fold_icon_animating: Cell<bool>,
    overshoot: canvas_overshoot::EdgeGlow,
    content_width: Cell<f64>,
    content_height: Cell<f64>,
    layout_cache: RefCell<Option<LayoutCache>>,
    layout_dirty: Cell<bool>,
    text_width_cache: RefCell<TextWidthCache>,
    highlight_cache: RefCell<Vec<HighlightRange>>,
    highlights_dirty: Cell<bool>,
    syntax_generation: Cell<u64>,
    highlight_request_generation: Cell<u64>,
    syntax_sender: Sender<SyntaxWorkerCommand>,
    syntax_error: Cell<bool>,
    syntax_issues: RefCell<Vec<SyntaxIssue>>,
    completion: RefCell<CompletionState>,
    completion_ui: RefCell<Option<CompletionUi>>,
    search: RefCell<SearchState>,
    cursor_visible: Cell<bool>,
    undo_stack: RefCell<Vec<HistorySnapshot>>,
    redo_stack: RefCell<Vec<HistorySnapshot>>,
    edit_callbacks: RefCell<Vec<EditCallback>>,
    scroll_callbacks: RefCell<Vec<ScrollCallback>>,
    diff_fold_callback: RefCell<Option<DiffFoldCallback>>,
    font_size_adjust_callback: RefCell<Option<FontSizeAdjustCallback>>,
    git_added_lines: RefCell<Vec<bool>>,
    git_deleted_hint_counts: RefCell<Vec<usize>>,
    spellcheck_issues: RefCell<Vec<SpellcheckIssue>>,
}

#[derive(Default)]
struct SearchState {
    query: String,
    matches: Vec<SearchMatch>,
    active: Option<usize>,
}

#[derive(Clone, Copy)]
struct SearchMatch {
    start: usize,
    end: usize,
}

enum SyntaxWorkerCommand {
    Reset {
        generation: u64,
        language: String,
        source: String,
        auto_folds: bool,
    },
    Edit {
        generation: u64,
        start: usize,
        old_end: usize,
        replacement: String,
        auto_folds: bool,
    },
    Suggest {
        generation: u64,
        request_id: u64,
        cursor: usize,
    },
}

enum SyntaxWorkerResult {
    Analysis {
        generation: u64,
        highlights: Vec<HighlightRange>,
        folds: Option<Vec<(usize, usize)>>,
        syntax_error: bool,
        syntax_issues: Vec<SyntaxIssue>,
    },
    Suggestions {
        generation: u64,
        request_id: u64,
        completions: Option<CompletionSet>,
    },
}

#[derive(Default)]
struct CompletionState {
    request_id: u64,
    items: Vec<CompletionItem>,
    selected: usize,
    replacement_range: Option<(usize, usize)>,
}

#[derive(Clone)]
struct CompletionUi {
    popover: gtk::Popover,
    list: gtk::ListBox,
}

struct HistorySnapshot {
    start: usize,
    removed: String,
    inserted: String,
    before_cursor: usize,
    before_selection: Option<Selection>,
    after_cursor: usize,
    after_selection: Option<Selection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionMode {
    Character,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FoldRange {
    start_line: usize,
    end_line: usize,
    expanded: bool,
    automatic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FoldControlKey {
    kind: FoldControlKind,
    index: usize,
}

impl FoldControlKey {
    fn editor(index: usize) -> Self {
        Self {
            kind: FoldControlKind::Editor,
            index,
        }
    }

    fn diff(index: usize) -> Self {
        Self {
            kind: FoldControlKind::Diff,
            index,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FoldControlKind {
    Editor,
    Diff,
}

#[derive(Clone, Copy)]
struct FoldIconState {
    key: FoldControlKey,
    angle: f64,
    target_angle: f64,
}

#[derive(Clone)]
struct VisualLine {
    source_line: usize,
    start: usize,
    end: usize,
    wrap_index: usize,
    folded: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum GutterSide {
    Left,
    Right,
}

struct LayoutCache {
    viewport_width: i32,
    font_size: i32,
    text_len: usize,
    fold_generation: u64,
    wrap: bool,
    gutter_width: f64,
    content_width: f64,
    content_height: f64,
    visual_lines: Vec<VisualLine>,
}

impl CodeEditor {
    pub(in crate::ui) fn new(language: &str, text: &str) -> Self {
        let line_count = text.lines().count().max(1);
        let font_size = config::DEFAULT_EDITOR_FONT_SIZE;
        let line_height = line_height_for_font_size(font_size);
        let area = gtk::DrawingArea::builder()
            .content_width(MIN_CONTENT_WIDTH)
            .content_height(line_height as i32)
            .focusable(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        let font_metrics = render::measure_font_metrics(&area, font_size);
        let line_height = font_metrics.line_height;
        area.set_content_height(line_height as i32);
        let (syntax_sender, syntax_receiver) = mpsc::channel();
        let (syntax_result_sender, syntax_result_receiver) = mpsc::channel();
        thread::spawn(move || run_syntax_worker(syntax_receiver, syntax_result_sender));

        let state = Rc::new(EditorState {
            text: RefCell::new(TextBuffer::new(text)),
            syntax_source: RefCell::new(None),
            language: RefCell::new(language.to_string()),
            font_size: Cell::new(font_size),
            char_width: Cell::new(font_metrics.char_width),
            char_spacing: Cell::new(font_metrics.char_spacing),
            line_height: Cell::new(font_metrics.line_height),
            baseline_offset: Cell::new(font_metrics.baseline_offset),
            editable: Cell::new(false),
            wrap: Cell::new(true),
            scrollbar_visible: Cell::new(true),
            gutter_side: Cell::new(GutterSide::Left),
            auto_folding_enabled: Cell::new(true),
            selection: RefCell::new(None),
            cursor: Cell::new(0),
            preedit: RefCell::new(String::new()),
            folds: RefCell::new(Vec::new()),
            fold_generation: Cell::new(1),
            diff_rows: RefCell::new(None),
            scrollbar_markers: RefCell::new(Vec::new()),
            scroll_x: Cell::new(0.0),
            scroll_y: Cell::new(0.0),
            scrollbar_hover: Rc::new(Cell::new(false)),
            scrollbar_active: Rc::new(Cell::new(false)),
            scrollbar_hover_progress: Rc::new(Cell::new(0.0)),
            scrollbar_animating: Rc::new(Cell::new(false)),
            middle_autoscroll: Rc::new(canvas_scroll::MiddleAutoscroll::new()),
            fold_hovered: Cell::new(None),
            fold_pressed: Cell::new(None),
            fold_hover_progress: Cell::new(0.0),
            fold_hover_animating: Cell::new(false),
            fold_icon_states: RefCell::new(Vec::new()),
            fold_icon_animating: Cell::new(false),
            overshoot: canvas_overshoot::EdgeGlow::new(),
            content_width: Cell::new(MIN_CONTENT_WIDTH as f64),
            content_height: Cell::new(line_height),
            layout_cache: RefCell::new(None),
            layout_dirty: Cell::new(true),
            text_width_cache: RefCell::new(TextWidthCache::new(font_size)),
            highlight_cache: RefCell::new(Vec::new()),
            highlights_dirty: Cell::new(true),
            syntax_generation: Cell::new(1),
            highlight_request_generation: Cell::new(0),
            syntax_sender,
            syntax_error: Cell::new(false),
            syntax_issues: RefCell::new(Vec::new()),
            completion: RefCell::new(CompletionState::default()),
            completion_ui: RefCell::new(None),
            search: RefCell::new(SearchState::default()),
            cursor_visible: Cell::new(true),
            undo_stack: RefCell::new(Vec::new()),
            redo_stack: RefCell::new(Vec::new()),
            edit_callbacks: RefCell::new(Vec::new()),
            scroll_callbacks: RefCell::new(Vec::new()),
            diff_fold_callback: RefCell::new(None),
            font_size_adjust_callback: RefCell::new(None),
            git_added_lines: RefCell::new(vec![false; line_count]),
            git_deleted_hint_counts: RefCell::new(vec![0; line_count]),
            spellcheck_issues: RefCell::new(Vec::new()),
        });
        install_syntax_result_receiver(&area, &state, syntax_result_receiver);
        reset_syntax_worker(&state);

        area.set_draw_func({
            let state = state.clone();
            move |area, context, width, height| {
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
                    render::draw_editor(area, context, width, height, &state)
                })) {
                    log_draw_panic(&state, width, height, payload.as_ref());
                }
            }
        });
        area.connect_resize({
            let state = state.clone();
            move |area, width, height| render::refresh_size(area, &state, width, height)
        });

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(false)
            .build();
        root.set_size_request(MIN_CONTENT_WIDTH, (line_height * 4.0) as i32);
        let search_panel = build_search_panel(&area, &state);
        root.append(&search_panel.widget());
        root.append(&area);
        input::install_interactions(&area, &root, &state);
        install_font_size_shortcuts(&area, &root, &state);
        search_panel.install_shortcuts(&root);

        let editor = Self {
            root,
            area,
            search_panel,
            state,
        };
        editor.refresh();
        editor
    }

    pub(in crate::ui) fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub(in crate::ui) fn set_text(&self, text: &str) {
        self.set_document_with_language(None, text, false);
    }

    pub(in crate::ui) fn set_document(&self, language: &str, text: &str) {
        self.set_document_with_language(Some(language), text, false);
    }

    pub(in crate::ui) fn select_range(&self, start: usize, end: usize) {
        let text = self.state.text.borrow();
        let start = clamp_to_char_boundary(&text, start);
        let end = clamp_to_char_boundary(&text, end).max(start);
        let line = render::line_for_offset(&text, start);
        drop(text);

        let expanded_fold = {
            let mut folds = self.state.folds.borrow_mut();
            let mut changed = false;
            for fold in folds.iter_mut() {
                if !fold.expanded && fold.start_line <= line && line <= fold.end_line {
                    fold.expanded = true;
                    changed = true;
                }
            }
            changed
        };
        if expanded_fold {
            mark_fold_state_changed(&self.state);
        }

        self.state.selection.replace(Some(Selection {
            anchor: start,
            focus: end,
            visual_anchor: start,
            visual_focus: end,
        }));
        self.state.cursor.set(end);
        self.state.cursor_visible.set(true);
        self.area.grab_focus();
        render::ensure_offset_visible(&self.area, &self.state, start);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn select_line_column(&self, line: usize, column: usize) {
        let text = self.state.text.borrow();
        let offset = byte_offset_for_line_column(&text, line, column);
        drop(text);
        self.select_range(offset, offset);
    }

    pub(in crate::ui) fn set_diff_document(&self, document: DiffEditorDocument) {
        let unchanged = {
            let rows_match = self
                .state
                .diff_rows
                .borrow()
                .as_ref()
                .is_some_and(|rows| rows == &document.rows);
            let source_match = self
                .state
                .syntax_source
                .borrow()
                .as_ref()
                .is_some_and(|source| source == &document.source);
            let language_match =
                self.state.language.borrow().as_str() == document.language.as_str();
            rows_match && source_match && language_match
        };
        if unchanged {
            log::debug!(
                "code_editor diff document unchanged rows={} source_bytes={}",
                document.rows.len(),
                document.source.len()
            );
            return;
        }
        input::dismiss_completion(&self.state);
        let text = document
            .rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.state.text.borrow_mut().set_text(&text);
        self.state.syntax_source.replace(Some(document.source));
        self.state.cursor.set(0);
        self.state.selection.borrow_mut().take();
        self.state.undo_stack.borrow_mut().clear();
        self.state.redo_stack.borrow_mut().clear();
        let had_folds = !self.state.folds.borrow().is_empty();
        self.state.folds.borrow_mut().clear();
        if had_folds {
            log::debug!("code_editor folds cleared reason=diff_document");
            mark_fold_state_changed(&self.state);
        } else {
            clear_fold_interaction_state(&self.state);
        }
        self.state.diff_rows.replace(Some(document.rows));
        self.state.language.replace(document.language.clone());
        self.state.editable.set(false);
        self.state.auto_folding_enabled.set(false);
        self.area.set_focusable(true);
        render::invalidate_layout(&self.state);
        render::invalidate_highlights(&self.state);
        reset_syntax_worker(&self.state);
        reset_git_state(&self.state);
        rebuild_search_matches(&self.area, &self.state);
        self.refresh();
    }

    pub(in crate::ui) fn set_file_diff(&self, comparison: Option<&FileComparison>) {
        apply_file_diff_marks(&self.state, comparison);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn clear_file_diff(&self) {
        reset_git_state(&self.state);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_spellcheck_issues(&self, issues: Vec<SpellcheckIssue>) {
        self.state.spellcheck_issues.replace(issues);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_language(&self, language: &str) {
        if self.state.language.borrow().as_str() == language {
            return;
        }

        input::dismiss_completion(&self.state);
        self.state.language.replace(language.to_string());
        clear_automatic_folds(&self.state, "language changed");
        render::invalidate_layout(&self.state);
        render::invalidate_highlights(&self.state);
        rebuild_auto_folds(&self.area, &self.state);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_font_size(&self, font_size: f64) {
        set_font_size_for_state(&self.area, &self.root, &self.state, font_size);
    }

    pub(in crate::ui) fn document_text(&self) -> String {
        self.state.text.borrow().as_str().to_string()
    }

    pub(in crate::ui) fn font_size(&self) -> f64 {
        self.state.font_size.get()
    }

    pub(in crate::ui) fn set_font_size_adjust_callback<F>(&self, callback: F)
    where
        F: Fn(f64) + 'static,
    {
        self.state
            .font_size_adjust_callback
            .replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn set_editable(&self, editable: bool) {
        self.state.editable.set(editable);
        self.area.set_focusable(true);
        self.area.set_cursor_from_name(None);
        self.state.cursor_visible.set(true);
        log::debug!(
            "code_editor editable changed editable={editable} syntax_error={} syntax_error_visible={}",
            self.state.syntax_error.get(),
            editable && self.state.syntax_error.get()
        );
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_read_only(&self, read_only: bool) {
        self.set_editable(!read_only);
    }

    pub(in crate::ui) fn set_scrollbar_visible(&self, visible: bool) {
        self.state.scrollbar_visible.set(visible);
        render::invalidate_layout(&self.state);
        self.refresh();
    }

    pub(in crate::ui) fn set_gutter_side(&self, side: GutterSide) {
        self.state.gutter_side.set(side);
        render::invalidate_layout(&self.state);
        self.refresh();
    }

    pub(in crate::ui) fn set_scrollbar_markers(&self, markers: Vec<ScrollbarMarker>) {
        self.state.scrollbar_markers.replace(markers);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn connect_scroll_changed<F>(&self, callback: F)
    where
        F: Fn(f64) + 'static,
    {
        self.state
            .scroll_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub(in crate::ui) fn set_scroll_y(&self, value: f64) {
        render::set_scroll_y(&self.area, &self.state, value);
    }

    pub(in crate::ui) fn scroll_y(&self) -> f64 {
        self.state.scroll_y.get()
    }

    pub(in crate::ui) fn source_offset_at_scroll_top(&self) -> usize {
        render::source_offset_at_scroll_top(&self.area, &self.state)
    }

    pub(in crate::ui) fn set_source_offset_at_scroll_top(&self, offset: usize) {
        render::set_source_offset_at_scroll_top(&self.area, &self.state, offset);
    }

    pub(in crate::ui) fn set_auto_folding_enabled(&self, enabled: bool) {
        if self.state.auto_folding_enabled.get() == enabled {
            return;
        }
        self.state.auto_folding_enabled.set(enabled);
        rebuild_auto_folds(&self.area, &self.state);
        render::invalidate_layout(&self.state);
        self.area.queue_draw();
    }

    pub(in crate::ui) fn set_diff_fold_callback<F>(&self, callback: F)
    where
        F: Fn(usize) + 'static,
    {
        self.state
            .diff_fold_callback
            .replace(Some(Rc::new(callback)));
    }

    pub(in crate::ui) fn connect_edit<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.state
            .edit_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    fn refresh(&self) {
        render::refresh_size(
            &self.area,
            &self.state,
            self.area.allocated_width(),
            self.area.allocated_height(),
        );
        self.area.queue_draw();
    }

    fn set_document_with_language(&self, language: Option<&str>, text: &str, force_reset: bool) {
        let language_changed =
            language.is_some_and(|language| self.state.language.borrow().as_str() != language);
        let text_changed = force_reset || self.state.text.borrow().as_str() != text;
        if !language_changed && !text_changed {
            return;
        }

        if let Some(language) = language.filter(|_| language_changed) {
            input::dismiss_completion(&self.state);
            self.state.language.replace(language.to_string());
        }
        if text_changed {
            input::dismiss_completion(&self.state);
            self.state.text.borrow_mut().set_text(text);
            self.state.syntax_source.borrow_mut().take();
            self.state.cursor.set(text.len());
            self.state.selection.borrow_mut().take();
            self.state.undo_stack.borrow_mut().clear();
            self.state.redo_stack.borrow_mut().clear();
            self.state.diff_rows.borrow_mut().take();
            self.state.scrollbar_markers.borrow_mut().clear();
            self.state.spellcheck_issues.borrow_mut().clear();
            reset_git_state(&self.state);
            clear_automatic_folds(&self.state, "document changed");
            normalize_folds_for_current_text(&self.state, "document changed");
            rebuild_search_matches(&self.area, &self.state);
        } else if language_changed {
            clear_automatic_folds(&self.state, "language changed");
        }

        render::invalidate_layout(&self.state);
        render::invalidate_highlights(&self.state);
        rebuild_auto_folds(&self.area, &self.state);
        self.refresh();
    }
}

fn build_search_panel(area: &gtk::DrawingArea, state: &Rc<EditorState>) -> SearchPanel {
    let search_panel = SearchPanel::new("Search");
    search_panel.set_key_capture_widget(area);
    search_panel.set_clear_on_close(false);
    search_panel.set_options_visible(false);

    search_panel.connect_query_changed({
        let area = area.clone();
        let state = state.clone();
        let search_panel = search_panel.clone();

        move |query| {
            update_search_query(&area, &state, &query);
            update_search_status(&search_panel, &state);
        }
    });
    search_panel.connect_opened({
        let area = area.clone();
        let state = state.clone();
        let search_panel = search_panel.clone();

        move || {
            handle_search_opened(&area, &search_panel, &state);
        }
    });
    search_panel.connect_closed({
        let area = area.clone();

        move || {
            area.grab_focus();
        }
    });
    search_panel.connect_previous({
        let area = area.clone();
        let state = state.clone();
        let search_panel = search_panel.clone();

        move || {
            search_previous(&area, &state);
            update_search_status(&search_panel, &state);
        }
    });
    search_panel.connect_next({
        let area = area.clone();
        let state = state.clone();
        let search_panel = search_panel.clone();

        move || {
            search_next(&area, &state);
            update_search_status(&search_panel, &state);
        }
    });

    search_panel
}

fn handle_search_opened(
    area: &gtk::DrawingArea,
    search_panel: &SearchPanel,
    state: &Rc<EditorState>,
) {
    if let Some((start, end)) = selection_bounds(state).filter(|(start, end)| start < end) {
        let selected = state.text.borrow()[start..end].to_string();
        search_panel.set_query(&selected, true);
    } else if search_panel.query().is_empty() {
        rebuild_search_matches(area, state);
    }
    update_search_status(search_panel, state);
    log::debug!(
        "code_editor search opened query_len={}",
        search_panel.query().len()
    );
}

fn update_search_query(area: &gtk::DrawingArea, state: &Rc<EditorState>, query: &str) {
    {
        let mut search = state.search.borrow_mut();
        if search.query == query {
            return;
        }
        search.query = query.to_string();
    }
    rebuild_search_matches(area, state);
    log::debug!("code_editor search query updated len={}", query.len());
}

fn rebuild_search_matches(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let text = state.text.borrow();
    let query = state.search.borrow().query.clone();
    let matches = search_matches(&text, &query);
    let active = if matches.is_empty() {
        None
    } else {
        let cursor = state.cursor.get().min(text.len());
        Some(
            matches
                .iter()
                .position(|search_match| search_match.end > cursor)
                .unwrap_or(0),
        )
    };
    drop(text);

    {
        let mut search = state.search.borrow_mut();
        search.matches = matches;
        search.active = active;
    }
    if active.is_some() {
        select_search_match(area, state);
    } else {
        area.queue_draw();
    }
}

fn search_matches(text: &str, query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let (haystack, needle) = if query.is_ascii() {
        (text.to_ascii_lowercase(), query.to_ascii_lowercase())
    } else {
        (text.to_string(), query.to_string())
    };
    let mut matches = Vec::new();
    let mut cursor = 0usize;
    while cursor <= haystack.len() {
        let Some(relative) = haystack[cursor..].find(&needle) else {
            break;
        };
        let start = cursor + relative;
        let end = start + needle.len();
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            matches.push(SearchMatch { start, end });
            cursor = next_search_cursor(text, start, end);
        } else {
            cursor = next_char_boundary(text, start.saturating_add(1));
        }
    }
    matches
}

fn next_search_cursor(text: &str, start: usize, end: usize) -> usize {
    if start == end {
        next_char_boundary(text, end.saturating_add(1))
    } else {
        end
    }
}

fn search_next(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let len = state.search.borrow().matches.len();
    if len == 0 {
        return;
    }
    {
        let mut search = state.search.borrow_mut();
        search.active = Some(search.active.map(|active| (active + 1) % len).unwrap_or(0));
    }
    select_search_match(area, state);
    log::debug!(
        "code_editor search next active={:?} total={len}",
        state.search.borrow().active
    );
}

fn search_previous(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let len = state.search.borrow().matches.len();
    if len == 0 {
        return;
    }
    {
        let mut search = state.search.borrow_mut();
        search.active = Some(
            search
                .active
                .map(|active| active.checked_sub(1).unwrap_or(len - 1))
                .unwrap_or(len - 1),
        );
    }
    select_search_match(area, state);
    log::debug!(
        "code_editor search previous active={:?} total={len}",
        state.search.borrow().active
    );
}

fn select_search_match(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let search = state.search.borrow();
    let Some(search_match) = search
        .active
        .and_then(|active| search.matches.get(active).copied())
    else {
        drop(search);
        area.queue_draw();
        return;
    };
    drop(search);

    state.selection.replace(Some(Selection {
        anchor: search_match.start,
        focus: search_match.end,
        visual_anchor: search_match.start,
        visual_focus: search_match.end,
    }));
    state.cursor.set(search_match.end);
    state.cursor_visible.set(true);
    render::ensure_offset_visible(area, state, search_match.start);
    area.queue_draw();
}

fn update_search_status(search_panel: &SearchPanel, state: &Rc<EditorState>) {
    let search = state.search.borrow();
    if search.query.is_empty() {
        search_panel.set_status("");
        return;
    }
    let Some(active) = search.active else {
        search_panel.set_status("No Results");
        return;
    };
    search_panel.set_status(&format!("{} of {}", active + 1, search.matches.len()));
}

fn line_height_for_font_size(font_size: f64) -> f64 {
    (font_size + 9.0).ceil()
}

fn install_font_size_shortcuts(area: &gtk::DrawingArea, root: &gtk::Box, state: &Rc<EditorState>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let area = area.clone();
        let root = root.clone();
        let state = state.clone();

        move |_, key, _, modifiers| {
            let Some(delta) = font_size_delta_for_key(key, modifiers) else {
                return gtk::glib::Propagation::Proceed;
            };

            let callback = state.font_size_adjust_callback.borrow().clone();
            if let Some(callback) = callback {
                callback(delta);
            } else {
                let next =
                    set_font_size_for_state(&area, &root, &state, state.font_size.get() + delta);
                config::save_editor_font_size(next);
            }

            gtk::glib::Propagation::Stop
        }
    });
    root.add_controller(keys);
}

fn set_font_size_for_state(
    area: &gtk::DrawingArea,
    root: &gtk::Box,
    state: &Rc<EditorState>,
    font_size: f64,
) -> f64 {
    let font_size = config::normalize_font_size(font_size, config::DEFAULT_EDITOR_FONT_SIZE);
    if (state.font_size.get() - font_size).abs() <= f64::EPSILON {
        return font_size;
    }

    state.font_size.set(font_size);
    state
        .text_width_cache
        .borrow_mut()
        .clear_for_font_size(font_size.round() as i32);
    render::refresh_font_metrics(area, state);
    let line_height = state.line_height.get();
    area.set_content_height(line_height as i32);
    root.set_size_request(MIN_CONTENT_WIDTH, (line_height * 4.0) as i32);
    render::invalidate_layout(state);
    area.queue_draw();
    font_size
}

fn font_size_delta_for_key(key: gdk::Key, modifiers: gdk::ModifierType) -> Option<f64> {
    if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
        || modifiers.contains(gdk::ModifierType::ALT_MASK)
    {
        return None;
    }

    if key == gdk::Key::plus || key == gdk::Key::equal || key == gdk::Key::KP_Add {
        return Some(1.0);
    }
    if key == gdk::Key::minus || key == gdk::Key::underscore || key == gdk::Key::KP_Subtract {
        return Some(-1.0);
    }

    None
}

fn reset_git_state(state: &Rc<EditorState>) {
    let line_count = render::line_count(&state.text.borrow());
    state.git_added_lines.replace(vec![false; line_count]);
    state.git_deleted_hint_counts.replace(vec![0; line_count]);
    state.scrollbar_markers.replace(Vec::new());
}

fn clear_fold_interaction_state(state: &Rc<EditorState>) {
    state.fold_hovered.set(None);
    state.fold_pressed.set(None);
    state.fold_hover_progress.set(0.0);
    state.fold_icon_states.borrow_mut().clear();
}

fn mark_fold_state_changed(state: &Rc<EditorState>) {
    let generation = state.fold_generation.get().wrapping_add(1).max(1);
    state.fold_generation.set(generation);
    clear_fold_interaction_state(state);
    render::invalidate_layout(state);
}

fn clear_automatic_folds(state: &Rc<EditorState>, reason: &str) -> bool {
    let (before, after) = {
        let mut folds = state.folds.borrow_mut();
        let before = folds.len();
        folds.retain(|fold| !fold.automatic);
        (before, folds.len())
    };
    if before == after {
        return false;
    }

    log::debug!(
        "code_editor folds cleared automatic reason={reason} before={before} after={after}"
    );
    mark_fold_state_changed(state);
    true
}

fn normalize_folds_for_current_text(state: &Rc<EditorState>, reason: &str) -> bool {
    let line_count = render::line_count(&state.text.borrow());
    let before = state.folds.borrow().clone();
    let before_len = before.len();
    let mut normalized = before
        .iter()
        .copied()
        .filter(|fold| {
            fold.start_line < line_count
                && fold.end_line < line_count
                && fold.start_line < fold.end_line
        })
        .collect::<Vec<_>>();
    normalized.sort_by_key(|fold| (fold.start_line, fold.end_line, fold.automatic));

    let mut deduped = Vec::<FoldRange>::with_capacity(normalized.len());
    for fold in normalized {
        if let Some(previous) = deduped.last_mut().filter(|previous| {
            previous.start_line == fold.start_line && previous.end_line == fold.end_line
        }) {
            previous.expanded &= fold.expanded;
            previous.automatic &= fold.automatic;
        } else {
            deduped.push(fold);
        }
    }

    if deduped == before {
        return false;
    }

    let after_len = deduped.len();
    state.folds.replace(deduped);
    log::debug!(
        "code_editor folds normalized reason={reason} before={before_len} after={after_len} line_count={line_count}"
    );
    mark_fold_state_changed(state);
    true
}

fn notify_scroll(state: &Rc<EditorState>, scroll_y: f64) {
    for callback in state.scroll_callbacks.borrow().iter() {
        callback(scroll_y);
    }
}

fn notify_diff_fold(state: &Rc<EditorState>, fold_index: usize) {
    let callback = state.diff_fold_callback.borrow().as_ref().cloned();
    if let Some(callback) = callback {
        eprintln!(
            "[code-editor] diff fold action fold_index={fold_index} scroll_y={}",
            state.scroll_y.get()
        );
        callback(fold_index);
    }
}

fn log_draw_panic(state: &Rc<EditorState>, width: i32, height: i32, payload: &(dyn Any + Send)) {
    eprintln!(
        "[code-editor] draw panic: {}",
        panic_payload_message(payload)
    );
    eprintln!(
        "[code-editor] draw area width={width} height={height} editable={} wrap={} scrollbar_visible={} gutter_side={:?} scroll=({}, {}) content=({}, {}) cursor={}",
        state.editable.get(),
        state.wrap.get(),
        state.scrollbar_visible.get(),
        state.gutter_side.get(),
        state.scroll_x.get(),
        state.scroll_y.get(),
        state.content_width.get(),
        state.content_height.get(),
        state.cursor.get(),
    );

    match state.text.try_borrow() {
        Ok(text) => {
            eprintln!(
                "[code-editor] text len={} lines={}",
                text.len(),
                render::line_count(&text)
            );
        }
        Err(_) => eprintln!("[code-editor] text borrow failed"),
    }

    match state.selection.try_borrow() {
        Ok(selection) => eprintln!("[code-editor] selection={selection:?}"),
        Err(_) => eprintln!("[code-editor] selection borrow failed"),
    }

    match state.folds.try_borrow() {
        Ok(folds) => {
            let sample = folds
                .iter()
                .take(12)
                .map(|fold| {
                    format!(
                        "{}..{} expanded={} automatic={}",
                        fold.start_line, fold.end_line, fold.expanded, fold.automatic
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "[code-editor] folds count={} sample=[{sample}]",
                folds.len()
            );
        }
        Err(_) => eprintln!("[code-editor] folds borrow failed"),
    }

    match state.diff_rows.try_borrow() {
        Ok(rows) => {
            if let Some(rows) = rows.as_ref() {
                let sample = rows
                    .iter()
                    .enumerate()
                    .take(8)
                    .map(|(index, row)| {
                        format!(
                            "#{index}: number={:?} kind={:?} fold_index={:?} fold_expanded={} show_fold_control={} source={:?}..{:?} text_len={} paired_len={}",
                            row.number,
                            row.kind,
                            row.fold_index,
                            row.fold_expanded,
                            row.show_fold_control,
                            row.source_start,
                            row.source_end,
                            row.text.len(),
                            row.paired_text.len()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                eprintln!(
                    "[code-editor] diff rows count={} sample=[{sample}]",
                    rows.len()
                );
            } else {
                eprintln!("[code-editor] diff rows none");
            }
        }
        Err(_) => eprintln!("[code-editor] diff rows borrow failed"),
    }

    match state.layout_cache.try_borrow() {
        Ok(cache) => {
            if let Some(cache) = cache.as_ref() {
                let first = cache.visual_lines.first().map(visual_line_debug);
                let last = cache.visual_lines.last().map(visual_line_debug);
                eprintln!(
                    "[code-editor] layout viewport_width={} font_size={} text_len={} wrap={} gutter={} content=({}, {}) visual_lines={} first={:?} last={:?}",
                    cache.viewport_width,
                    cache.font_size,
                    cache.text_len,
                    cache.wrap,
                    cache.gutter_width,
                    cache.content_width,
                    cache.content_height,
                    cache.visual_lines.len(),
                    first,
                    last
                );
                eprintln!(
                    "[code-editor] cursor visual window offset={} [{}]",
                    state.cursor.get(),
                    visual_line_window_debug(&cache.visual_lines, state.cursor.get())
                );
            } else {
                eprintln!("[code-editor] layout none");
            }
        }
        Err(_) => eprintln!("[code-editor] layout borrow failed"),
    }
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "<non-string panic payload>".to_string()
}

fn visual_line_debug(line: &VisualLine) -> String {
    format!(
        "source={} start={} end={} wrap={} folded={:?}",
        line.source_line, line.start, line.end, line.wrap_index, line.folded
    )
}

fn visual_line_window_debug(visual_lines: &[VisualLine], offset: usize) -> String {
    if visual_lines.is_empty() {
        return String::new();
    }

    let center = visual_line_index_for_debug(visual_lines, offset);
    let start = center.saturating_sub(2);
    let end = (center + 3).min(visual_lines.len());
    (start..end)
        .map(|index| format!("#{index}: {}", visual_line_debug(&visual_lines[index])))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn visual_line_index_for_debug(visual_lines: &[VisualLine], offset: usize) -> usize {
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

fn apply_file_diff_marks(state: &Rc<EditorState>, comparison: Option<&FileComparison>) {
    let line_count = render::line_count(&state.text.borrow());
    let mut added = vec![false; line_count];
    let mut deleted_hints = vec![0usize; line_count];

    if let Some(comparison) = comparison {
        let mut insertion_at = 0usize;

        for row in &comparison.rows {
            if row.right_kind == DiffKind::Added {
                if let Some(right_number) = row.right_number {
                    let index = right_number.saturating_sub(1);
                    if index < line_count {
                        added[index] = true;
                    }
                }
            }

            if let Some(right_number) = row.right_number {
                if right_number > 0 {
                    insertion_at = right_number.min(line_count.saturating_add(1));
                }
            }

            if row.left_kind == DiffKind::Deleted && row.left_number.is_some() {
                let line = if line_count == 0 {
                    0
                } else {
                    insertion_at.min(line_count - 1)
                };
                deleted_hints[line] = deleted_hints[line].saturating_add(1);
            }
        }
    }

    state.git_added_lines.replace(added);
    state.git_deleted_hint_counts.replace(deleted_hints);
    state
        .scrollbar_markers
        .replace(file_editor_scrollbar_markers(
            &state.git_added_lines.borrow(),
            &state.git_deleted_hint_counts.borrow(),
        ));
}

fn file_editor_scrollbar_markers(
    added: &[bool],
    deleted_hint_counts: &[usize],
) -> Vec<ScrollbarMarker> {
    let max_len = added.len().min(deleted_hint_counts.len());
    (0..max_len)
        .filter_map(|index| {
            let is_added = added[index];
            let is_deleted = deleted_hint_counts[index] > 0;
            let kind = match (is_added, is_deleted) {
                (true, true) => ScrollbarMarkerKind::Mixed,
                (true, false) => ScrollbarMarkerKind::Added,
                (false, true) => ScrollbarMarkerKind::Deleted,
                (false, false) => return None,
            };
            Some(ScrollbarMarker { row: index, kind })
        })
        .collect()
}

fn clear_git_state_for_state(state: &Rc<EditorState>) {
    reset_git_state(state);
}

fn run_syntax_worker(receiver: Receiver<SyntaxWorkerCommand>, sender: Sender<SyntaxWorkerResult>) {
    let mut highlighter = SyntaxHighlighter::new("");
    let mut latest_generation = 0;
    let mut result_auto_folds = true;

    while let Ok(command) = receiver.recv() {
        let mut analysis_needed = false;
        let mut suggestion_request = None;
        apply_syntax_command(
            &mut highlighter,
            command,
            &mut latest_generation,
            &mut result_auto_folds,
            &mut analysis_needed,
            &mut suggestion_request,
        );
        while let Ok(command) = receiver.try_recv() {
            apply_syntax_command(
                &mut highlighter,
                command,
                &mut latest_generation,
                &mut result_auto_folds,
                &mut analysis_needed,
                &mut suggestion_request,
            );
        }

        if analysis_needed {
            let highlights = highlighter.highlight_current();
            let folds = result_auto_folds.then(|| highlighter.fold_ranges_current());
            let syntax_error = highlighter.has_error_current();
            let syntax_issues = highlighter.syntax_issues_current();
            let _ = sender.send(SyntaxWorkerResult::Analysis {
                generation: latest_generation,
                highlights,
                folds,
                syntax_error,
                syntax_issues,
            });
            result_auto_folds = false;
        }

        if let Some((generation, request_id, cursor)) = suggestion_request {
            let completions = (generation == latest_generation)
                .then(|| highlighter.completions_current(cursor))
                .flatten();
            let _ = sender.send(SyntaxWorkerResult::Suggestions {
                generation,
                request_id,
                completions,
            });
        }
    }
}

fn apply_syntax_command(
    highlighter: &mut SyntaxHighlighter,
    command: SyntaxWorkerCommand,
    latest_generation: &mut u64,
    latest_auto_folds: &mut bool,
    analysis_needed: &mut bool,
    suggestion_request: &mut Option<(u64, u64, usize)>,
) {
    match command {
        SyntaxWorkerCommand::Reset {
            generation,
            language,
            source,
            auto_folds,
        } => {
            highlighter.set_language(&language);
            highlighter.set_source(&source);
            *latest_generation = generation;
            *latest_auto_folds = auto_folds;
            *analysis_needed = true;
        }
        SyntaxWorkerCommand::Edit {
            generation,
            start,
            old_end,
            replacement,
            auto_folds,
        } => {
            highlighter.apply_edit(start, old_end, &replacement);
            *latest_generation = generation;
            *latest_auto_folds |= auto_folds;
            *analysis_needed = true;
        }
        SyntaxWorkerCommand::Suggest {
            generation,
            request_id,
            cursor,
        } => {
            *suggestion_request = Some((generation, request_id, cursor));
        }
    }
}

fn install_syntax_result_receiver(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    receiver: Receiver<SyntaxWorkerResult>,
) {
    let area = area.clone();
    let state = state.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let mut latest_analysis = None;
        let mut latest_suggestions = None;
        loop {
            match receiver.try_recv() {
                Ok(result @ SyntaxWorkerResult::Analysis { .. }) => latest_analysis = Some(result),
                Ok(result @ SyntaxWorkerResult::Suggestions { .. }) => {
                    latest_suggestions = Some(result)
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return gtk::glib::ControlFlow::Break,
            }
        }

        if let Some(result) = latest_analysis {
            apply_syntax_result(&area, &state, result);
        }
        if let Some(result) = latest_suggestions {
            apply_syntax_result(&area, &state, result);
        }

        gtk::glib::ControlFlow::Continue
    });
}

fn apply_syntax_result(
    area: &gtk::DrawingArea,
    state: &Rc<EditorState>,
    result: SyntaxWorkerResult,
) {
    let (generation, highlights, folds, syntax_error, syntax_issues) = match result {
        SyntaxWorkerResult::Analysis {
            generation,
            highlights,
            folds,
            syntax_error,
            syntax_issues,
        } => (generation, highlights, folds, syntax_error, syntax_issues),
        SyntaxWorkerResult::Suggestions {
            generation,
            request_id,
            completions,
        } => {
            if state.syntax_generation.get() == generation
                && state.completion.borrow().request_id == request_id
            {
                input::apply_completion_result(area, state, completions);
            }
            return;
        }
    };

    if state.syntax_generation.get() != generation {
        return;
    }

    if state.syntax_error.replace(syntax_error) != syntax_error {
        log::debug!(
            "code_editor syntax error changed syntax_error={syntax_error} issues={} editable={} generation={generation}",
            syntax_issues.len(),
            state.editable.get()
        );
    }
    state.syntax_issues.replace(syntax_issues);
    state.highlight_cache.replace(highlights);
    state.highlights_dirty.set(false);
    state.highlight_request_generation.set(generation);

    if let Some(ranges) = folds {
        let existing_folds = state.folds.borrow();
        let before_folds = existing_folds.clone();
        let previous_auto_folds = existing_folds
            .iter()
            .copied()
            .filter(|fold| fold.automatic)
            .collect::<Vec<_>>();
        drop(existing_folds);
        let mut folds = state.folds.borrow_mut();
        folds.retain(|fold| !fold.automatic);
        append_auto_folds_preserving_state(&mut folds, &ranges, &previous_auto_folds);
        let folds_changed = *folds != before_folds;
        drop(folds);
        let folds_normalized = normalize_folds_for_current_text(state, "syntax result");
        if folds_changed && !folds_normalized {
            log::debug!(
                "code_editor folds updated from syntax ranges={} generation={generation}",
                ranges.len()
            );
            mark_fold_state_changed(state);
        }
        if folds_changed || folds_normalized {
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
        }
    }

    area.queue_draw();
}

fn send_syntax_command(state: &Rc<EditorState>, command: SyntaxWorkerCommand) {
    if state.syntax_sender.send(command).is_err() {
        log::warn!("syntax highlighter worker is unavailable");
    }
}

fn reset_syntax_worker(state: &Rc<EditorState>) {
    let generation = next_syntax_generation(state);
    state.highlight_request_generation.set(generation);
    let source = state
        .syntax_source
        .borrow()
        .as_ref()
        .cloned()
        .unwrap_or_else(|| state.text.borrow().as_str().to_string());
    let diff_document = state.diff_rows.borrow().is_some();
    let auto_folds = state.auto_folding_enabled.get() && !diff_document;
    log::debug!(
        "code_editor syntax reset generation={} source_bytes={} diff_document={} auto_folds={}",
        generation,
        source.len(),
        diff_document,
        auto_folds
    );
    send_syntax_command(
        state,
        SyntaxWorkerCommand::Reset {
            generation,
            language: state.language.borrow().clone(),
            source,
            auto_folds,
        },
    );
}

fn send_syntax_edit(
    state: &Rc<EditorState>,
    start: usize,
    old_end: usize,
    replacement: &str,
    auto_folds: bool,
) {
    let source_len = state.text.borrow().len();
    apply_edit_to_ranges(
        &mut state.highlight_cache.borrow_mut(),
        start,
        old_end,
        replacement.len(),
        source_len,
    );
    state.highlights_dirty.set(true);
    state.syntax_issues.borrow_mut().clear();
    let generation = next_syntax_generation(state);
    state.highlight_request_generation.set(generation);
    send_syntax_command(
        state,
        SyntaxWorkerCommand::Edit {
            generation,
            start,
            old_end,
            replacement: replacement.to_string(),
            auto_folds,
        },
    );
}

fn request_suggestions(state: &Rc<EditorState>, request_id: u64, cursor: usize) {
    send_syntax_command(
        state,
        SyntaxWorkerCommand::Suggest {
            generation: state.syntax_generation.get(),
            request_id,
            cursor,
        },
    );
}

fn next_syntax_generation(state: &Rc<EditorState>) -> u64 {
    let generation = state.syntax_generation.get().wrapping_add(1).max(1);
    state.syntax_generation.set(generation);
    generation
}

fn schedule_highlights(_area: &gtk::DrawingArea, state: &Rc<EditorState>, _text: &str) {
    let generation = state.syntax_generation.get();
    if state.highlight_request_generation.get() == generation {
        return;
    }
    state.highlight_request_generation.set(generation);
}

fn rebuild_auto_folds(area: &gtk::DrawingArea, state: &Rc<EditorState>) {
    let removed_auto_folds =
        if !state.auto_folding_enabled.get() || state.diff_rows.borrow().is_some() {
            clear_automatic_folds(state, "auto folding unavailable")
        } else {
            false
        };
    if !state.auto_folding_enabled.get() || state.diff_rows.borrow().is_some() {
        if removed_auto_folds {
            render::refresh_size(area, state, area.allocated_width(), area.allocated_height());
            area.queue_draw();
        }
        reset_syntax_worker(state);
        return;
    }

    reset_syntax_worker(state);
}

fn append_auto_folds_preserving_state(
    folds: &mut Vec<FoldRange>,
    ranges: &[(usize, usize)],
    previous_auto_folds: &[FoldRange],
) {
    for &(start_line, end_line) in ranges {
        if end_line <= start_line
            || folds
                .iter()
                .any(|fold| fold.start_line == start_line && fold.end_line == end_line)
        {
            continue;
        }
        let expanded = previous_auto_folds
            .iter()
            .find(|fold| fold.start_line == start_line && fold.end_line == end_line)
            .map(|fold| fold.expanded)
            .unwrap_or(true);
        folds.push(FoldRange {
            start_line,
            end_line,
            expanded,
            automatic: true,
        });
    }
    folds.sort_by_key(|fold| (fold.start_line, fold.end_line));
}

fn selection_bounds(state: &Rc<EditorState>) -> Option<(usize, usize)> {
    let selection = *state.selection.borrow();
    selection.and_then(Selection::visual_bounds)
}

fn notify_edit(state: &Rc<EditorState>) {
    for callback in state.edit_callbacks.borrow().iter() {
        callback();
    }
}
