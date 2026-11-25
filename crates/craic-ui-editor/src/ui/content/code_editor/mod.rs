mod diagnostics;
mod input;
mod search;
mod viewport;

use crate::config;
use crate::git::{DiffKind, FileComparison};
use crate::language_support::language_support;
use crate::markdown_lint::MarkdownLintIssue;
use crate::spellcheck::SpellcheckIssue;
use crate::ui::components::search::SearchPanel;
use craic_file_support::LanguageId;
use craic_render_skia::{
    EditorMetrics, EditorSelection, TextDiagnosticKind, TextDiagnosticSpan,
    analyze_text_syntax_for_language,
};
use craic_ui_code::{Editor, SearchOptions};
use gpui::{Rgba, rgb};
use gtk::prelude::*;
use gtk::{Orientation, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;
use viewport::Viewport;

pub use craic_render_skia::byte_offset_for_line_column;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentRevision(u64);

#[derive(Clone)]
pub struct CodeEditor {
    pub root: gtk::Box,
    view: Viewport,
}

type EditCallback = Rc<dyn Fn()>;
type MarkdownLintIgnoreCallback = Rc<dyn Fn(String)>;
type ScrollCallback = Rc<dyn Fn(f64)>;

struct EditorState {
    editor: RefCell<Editor>,
    revision: Cell<u64>,
    adjustment: gtk::Adjustment,
    syncing_scroll: Cell<bool>,
    last_offset: Cell<Option<usize>>,
    pending_source_offset: Cell<Option<usize>>,
    edit_callbacks: RefCell<Vec<EditCallback>>,
    scroll_callbacks: RefCell<Vec<ScrollCallback>>,
    markdown_lint_ignore_callbacks: RefCell<Vec<MarkdownLintIgnoreCallback>>,
    markdown_lint_issues: RefCell<Vec<MarkdownLintIssue>>,
    spellcheck_issues: RefCell<Vec<SpellcheckIssue>>,
    syntax_diagnostics: RefCell<Vec<TextDiagnosticSpan>>,
    analysis_task: RefCell<Option<glib::JoinHandle<()>>>,
    diff_marks: RefCell<HashMap<usize, Rgba>>,
    search_panel: SearchPanel,
    search_open: Cell<bool>,
    search_options: Cell<SearchOptions>,
}

impl CodeEditor {
    pub fn new(language: &str, text: &str) -> Self {
        let adjustment = gtk::Adjustment::new(0.0, 0.0, 0.0, 16.0, 100.0, 0.0);
        let search_panel = SearchPanel::new("Search");
        let state = Rc::new(EditorState {
            editor: RefCell::new(Editor::new(config::DEFAULT_EDITOR_FONT_SIZE as f32)),
            revision: Cell::new(1),
            adjustment: adjustment.clone(),
            syncing_scroll: Cell::new(false),
            last_offset: Cell::new(None),
            pending_source_offset: Cell::new(None),
            edit_callbacks: RefCell::new(Vec::new()),
            scroll_callbacks: RefCell::new(Vec::new()),
            markdown_lint_ignore_callbacks: RefCell::new(Vec::new()),
            markdown_lint_issues: RefCell::new(Vec::new()),
            spellcheck_issues: RefCell::new(Vec::new()),
            syntax_diagnostics: RefCell::new(Vec::new()),
            analysis_task: RefCell::new(None),
            diff_marks: RefCell::new(HashMap::new()),
            search_panel: search_panel.clone(),
            search_open: Cell::new(false),
            search_options: Cell::new(SearchOptions::default()),
        });
        let view = Viewport::new(state);
        let content = gtk::Box::new(Orientation::Horizontal, 0);
        content.set_vexpand(true);
        content.append(&view);
        let scrollbar = gtk::Scrollbar::new(Orientation::Vertical, Some(&adjustment));
        content.append(&scrollbar);
        adjustment.connect_value_changed({
            let weak = view.downgrade();
            move |adjustment| {
                if let Some(view) = weak.upgrade()
                    && !view.state().syncing_scroll.get()
                {
                    let current = view.state().editor.borrow().viewport.scroll_y();
                    view.scroll(0.0, adjustment.value() - current);
                }
            }
        });
        let root = gtk::Box::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_size_request(240, 100);
        search::install(&view, &root);
        root.append(&search_panel.widget());
        root.append(&content);
        input::install(&view);
        let editor = Self { root, view };
        editor.set_document(language, text);
        editor
    }

    pub fn toggle_search(&self) {
        self.view.state().search_panel.toggle();
    }

    pub fn set_text(&self, text: &str) {
        let state = self.view.state();
        {
            let mut editor = state.editor.borrow_mut();
            if editor.document.text() == text {
                return;
            }
            let offset = editor
                .viewport
                .source_offset(&editor.document, editor.metrics);
            editor.reset_document(
                text.to_owned(),
                Vec::new(),
                EditorSelection::default(),
                true,
            );
            state.pending_source_offset.set(Some(offset));
        }
        self.view.clear_frame();
        self.view.document_changed(false);
    }

    pub fn set_document(&self, language: &str, text: &str) {
        self.set_document_id(language_support(language).id, text);
    }

    pub fn set_document_id(&self, language: LanguageId, text: &str) {
        let state = self.view.state();
        if state.editor.borrow().document.text() == text {
            self.set_language_id(language);
        } else {
            state.editor.borrow_mut().language = language;
            self.set_text(text);
        }
    }

    pub fn select_range(&self, start: usize, end: usize) {
        self.view
            .state()
            .editor
            .borrow_mut()
            .select(EditorSelection {
                anchor: start,
                focus: end.max(start),
            });
        self.view.grab_focus();
        self.view.refresh();
    }

    pub fn select_line_column(&self, line: usize, column: usize) {
        let offset = byte_offset_for_line_column(&self.document_text(), line, column);
        self.select_range(offset, offset);
    }

    pub fn set_file_diff(&self, comparison: Option<&FileComparison>) {
        let state = self.view.state();
        let mut marks = state.diff_marks.borrow_mut();
        marks.clear();
        if let Some(comparison) = comparison {
            let last_line = state
                .editor
                .borrow()
                .document
                .lines()
                .len()
                .saturating_sub(1);
            let mut insertion_at = 0;
            for row in &comparison.rows {
                if let Some(number) = row.right_number {
                    if row.right_kind == DiffKind::Added {
                        marks.insert(number.saturating_sub(1), rgb(0x57a64a));
                    }
                    insertion_at = number;
                }
                if row.left_kind == DiffKind::Deleted && row.left_number.is_some() {
                    marks.insert(insertion_at.min(last_line), rgb(0xda5b5d));
                }
            }
        }
        self.view.queue_draw();
    }

    pub fn clear_file_diff(&self) {
        self.set_file_diff(None);
    }

    pub fn set_spellcheck_issues(&self, issues: Vec<SpellcheckIssue>) {
        self.view.state().spellcheck_issues.replace(issues);
        self.view.refresh_diagnostics();
    }

    pub fn set_markdown_lint_issues(&self, issues: Vec<MarkdownLintIssue>) {
        self.view.state().markdown_lint_issues.replace(issues);
        self.view.refresh_diagnostics();
    }

    pub fn set_language(&self, language: &str) {
        self.set_language_id(language_support(language).id);
    }

    pub fn set_language_id(&self, language: LanguageId) {
        let mut editor = self.view.state().editor.borrow_mut();
        if editor.language != language {
            editor.language = language;
            drop(editor);
            self.view.analyze(false);
        }
    }

    pub fn set_font_size(&self, size: f64) {
        self.view.set_font_size(size);
    }

    pub fn document_text(&self) -> String {
        self.view.state().editor.borrow().document.text().to_owned()
    }

    pub fn document_revision(&self) -> DocumentRevision {
        DocumentRevision(self.view.state().revision.get())
    }

    pub fn set_editable(&self, editable: bool) {
        let mut editor = self.view.state().editor.borrow_mut();
        editor.editable = editable;
        if !editable {
            editor.marked_text.clear();
            editor.clear_completion();
        }
        drop(editor);
        self.view
            .update_property(&[gtk::accessible::Property::ReadOnly(!editable)]);
        self.view.queue_draw();
    }

    pub fn set_read_only(&self, read_only: bool) {
        self.set_editable(!read_only);
    }

    pub fn connect_scroll_changed<F: Fn(f64) + 'static>(&self, callback: F) {
        self.view
            .state()
            .scroll_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub fn source_offset_at_scroll_top(&self) -> usize {
        let state = self.view.state();
        let editor = state.editor.borrow();
        state.pending_source_offset.get().unwrap_or_else(|| {
            editor
                .viewport
                .source_offset(&editor.document, editor.metrics)
        })
    }

    pub fn set_source_offset_at_scroll_top(&self, offset: usize) {
        self.view.state().pending_source_offset.set(Some(offset));
        self.view.refresh();
    }

    pub fn connect_edit<F: Fn() + 'static>(&self, callback: F) {
        self.view
            .state()
            .edit_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub fn connect_markdown_lint_ignore<F: Fn(String) + 'static>(&self, callback: F) {
        self.view
            .state()
            .markdown_lint_ignore_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }
}

impl Viewport {
    fn set_font_size(&self, size: f64) -> f64 {
        let size = config::normalize_font_size(size, config::DEFAULT_EDITOR_FONT_SIZE);
        let mut editor = self.state().editor.borrow_mut();
        editor.metrics = EditorMetrics::for_font_size(size as f32);
        editor.reveal_selection();
        drop(editor);
        self.refresh();
        size
    }

    fn scroll(&self, x: f64, y: f64) {
        let mut state = self.state().editor.borrow_mut();
        let Editor {
            document,
            viewport,
            metrics,
            ..
        } = &mut *state;
        viewport.scroll_by(x, y, document, *metrics);
        drop(state);
        self.refresh();
    }

    fn refresh(&self) {
        let state = self.state();
        let (old_scroll, scroll_y, document_height, page, step, source_offset) = {
            let mut editor = state.editor.borrow_mut();
            let Editor {
                document,
                viewport,
                metrics,
                ..
            } = &mut *editor;
            let page = viewport.height();
            let step = f64::from(metrics.line_height);
            let current = viewport.scroll_y();
            if page > 0.0
                && viewport.width() > 0.0
                && let Some(offset) = state.pending_source_offset.take()
            {
                let row = document.visual_line_for_offset(offset.min(document.text().len()));
                let y = f64::from(metrics.text_inset) + row as f64 * step;
                viewport.scroll_by(0.0, y - current, document, *metrics);
            }
            (
                state.adjustment.value(),
                viewport.scroll_y(),
                f64::from(document.content_size(*metrics).1),
                page,
                step,
                viewport.source_offset(document, *metrics),
            )
        };
        state.syncing_scroll.set(true);
        state
            .adjustment
            .configure(scroll_y, 0.0, document_height.max(page), step, page, page);
        state.syncing_scroll.set(false);
        let previous = state.last_offset.replace(Some(source_offset));
        self.queue_draw();
        if old_scroll != scroll_y || previous.is_some_and(|previous| previous != source_offset) {
            let callbacks = state.scroll_callbacks.borrow().clone();
            for callback in callbacks {
                callback(scroll_y);
            }
        }
    }

    fn document_changed(&self, edited: bool) {
        let state = self.state();
        state.revision.set(state.revision.get().wrapping_add(1));
        state.spellcheck_issues.borrow_mut().clear();
        state.markdown_lint_issues.borrow_mut().clear();
        state.diff_marks.borrow_mut().clear();
        state.syntax_diagnostics.borrow_mut().clear();
        self.update_property(&[gtk::accessible::Property::ValueText(
            state.editor.borrow().document.text(),
        )]);
        self.analyze(edited);
        if state.search_open.get() {
            search::update(self, !edited);
        }
        self.refresh();
        if edited {
            let callbacks = state.edit_callbacks.borrow().clone();
            for callback in callbacks {
                callback();
            }
        }
    }

    fn analyze(&self, complete: bool) {
        static ANALYSIS_RUNTIME: OnceLock<Result<tokio::runtime::Runtime, std::io::Error>> =
            OnceLock::new();
        let state = self.state();
        if let Some(task) = state.analysis_task.borrow_mut().take() {
            task.abort();
        }
        let Ok(runtime) = ANALYSIS_RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .max_blocking_threads(2)
                .build()
        }) else {
            return;
        };
        let view = self.downgrade();
        let task = glib::spawn_future_local(async move {
            // Coalesce typing before copying and parsing the document.
            if complete {
                glib::timeout_future(Duration::from_millis(50)).await;
            }
            let Some(current) = view.upgrade() else {
                return;
            };
            let (text, language, completion) = {
                let editor = current.state().editor.borrow();
                (
                    editor.document.text().to_owned(),
                    editor.language,
                    (complete && editor.editable && editor.selection.is_empty())
                        .then_some(editor.selection.focus),
                )
            };
            drop(current);
            let analysis = runtime
                .spawn_blocking(move || {
                    analyze_text_syntax_for_language(language, &text, completion)
                })
                .await;
            let Some(view) = view.upgrade() else { return };
            let state = view.state();
            state.analysis_task.borrow_mut().take();
            let Ok(analysis) = analysis else { return };
            let mut editor = state.editor.borrow_mut();
            editor.set_syntax_and_folds(analysis.syntax, analysis.fold_ranges);
            if editor.selection.is_empty()
                && editor.editable
                && completion == Some(editor.selection.focus)
                && let Some(items) = analysis.completions
            {
                editor.completion_range = Some((items.replacement_start, items.replacement_end));
                editor.completion_items = items
                    .items
                    .into_iter()
                    .map(|item| item.insert_text)
                    .collect();
            }
            drop(editor);
            state.syntax_diagnostics.replace(analysis.diagnostics);
            view.refresh_diagnostics();
            view.refresh();
        });
        state.analysis_task.replace(Some(task));
    }

    fn refresh_diagnostics(&self) {
        let state = self.state();
        let mut spans = state.syntax_diagnostics.borrow().clone();
        spans.extend(
            state
                .spellcheck_issues
                .borrow()
                .iter()
                .map(|issue| TextDiagnosticSpan {
                    start: issue.start,
                    end: issue.end,
                    kind: TextDiagnosticKind::Spelling,
                }),
        );
        spans.extend(
            state
                .markdown_lint_issues
                .borrow()
                .iter()
                .map(|issue| TextDiagnosticSpan {
                    start: issue.start,
                    end: issue.end,
                    kind: TextDiagnosticKind::Warning,
                }),
        );
        state.editor.borrow_mut().set_diagnostics(spans);
        self.queue_draw();
    }
}
