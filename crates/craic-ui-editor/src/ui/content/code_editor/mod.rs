mod diagnostics;
mod search;

use crate::config;
use crate::git::{DiffKind, FileComparison};
use crate::language_support::{language_support, language_support_for_id};
use crate::markdown_lint::MarkdownLintIssue;
use crate::spellcheck::SpellcheckIssue;
use crate::ui::components::search::SearchPanel;
use craic_file_support::LanguageId;
use gtk::{gdk, glib};
use sourceview5::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use craic_render_skia::byte_offset_for_line_column;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentRevision(u64);

#[derive(Clone)]
pub struct CodeEditor {
    pub root: gtk::Box,
    view: sourceview5::View,
    buffer: sourceview5::Buffer,
    scroller: gtk::ScrolledWindow,
    scroll_mark: gtk::TextMark,
    search_panel: SearchPanel,
    // Keep the context alive; search callbacks use weak references to avoid a buffer cycle.
    _search_context: sourceview5::SearchContext,
    font_provider: gtk::CssProvider,
    state: Rc<EditorState>,
}

type EditCallback = Rc<dyn Fn()>;
type MarkdownLintIgnoreCallback = Rc<dyn Fn(String)>;

struct EditorState {
    revision: Cell<u64>,
    loading: Cell<bool>,
    user_action_depth: Cell<u32>,
    pending_edit: Cell<bool>,
    language: Cell<LanguageId>,
    font_size: Cell<f64>,
    edit_callbacks: RefCell<Vec<EditCallback>>,
    markdown_lint_ignore_callbacks: RefCell<Vec<MarkdownLintIgnoreCallback>>,
    markdown_lint_issues: RefCell<Vec<MarkdownLintIssue>>,
    spellcheck_issues: RefCell<Vec<SpellcheckIssue>>,
}

impl CodeEditor {
    pub fn new(language: &str, text: &str) -> Self {
        sourceview5::init();
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_implicit_trailing_newline(false);
        buffer.set_enable_undo(true);
        let scroll_mark = buffer.create_mark(None, &buffer.start_iter(), true);
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_show_line_numbers(true);
        view.set_show_line_marks(true);
        view.set_highlight_current_line(true);
        view.set_auto_indent(true);
        view.set_indent_on_tab(true);
        view.set_indent_width(4);
        view.set_tab_width(4);
        view.set_insert_spaces_instead_of_tabs(true);
        view.set_smart_backspace(true);
        view.set_smart_home_end(sourceview5::SmartHomeEndType::Before);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_left_margin(8);
        view.set_right_margin(8);
        view.set_top_margin(8);
        view.set_bottom_margin(8);
        view.set_hexpand(true);
        view.set_vexpand(true);
        view.set_enable_snippets(true);
        let words = sourceview5::CompletionWords::new(Some("Document words"));
        words.register(&buffer);
        view.completion().add_provider(&words);
        for (category, icon) in [
            ("git-added", "list-add-symbolic"),
            ("git-deleted", "list-remove-symbolic"),
        ] {
            let attributes = sourceview5::MarkAttributes::new();
            attributes.set_icon_name(icon);
            view.set_mark_attributes(category, &attributes, 0);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(false)
            .width_request(240)
            .height_request(100)
            .build();
        let font_provider = gtk::CssProvider::new();
        // TextView has no font-size property. Keep zoom styling local to this view.
        #[allow(deprecated)]
        view.style_context()
            .add_provider(&font_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        let state = Rc::new(EditorState {
            revision: Cell::new(1),
            loading: Cell::new(false),
            user_action_depth: Cell::new(0),
            pending_edit: Cell::new(false),
            language: Cell::new(LanguageId::PlainText),
            font_size: Cell::new(config::DEFAULT_EDITOR_FONT_SIZE),
            edit_callbacks: RefCell::new(Vec::new()),
            markdown_lint_ignore_callbacks: RefCell::new(Vec::new()),
            markdown_lint_issues: RefCell::new(Vec::new()),
            spellcheck_issues: RefCell::new(Vec::new()),
        });
        buffer.connect_begin_user_action({
            let state = Rc::downgrade(&state);
            move |_| {
                if let Some(state) = state.upgrade() {
                    state
                        .user_action_depth
                        .set(state.user_action_depth.get() + 1);
                }
            }
        });
        buffer.connect_changed({
            let state = Rc::downgrade(&state);
            move |buffer| {
                let Some(state) = state.upgrade() else { return };
                state
                    .revision
                    .set(state.revision.get().wrapping_add(1).max(1));
                diagnostics::clear(buffer, &state);
                if !state.loading.get() {
                    if state.user_action_depth.get() > 0 {
                        state.pending_edit.set(true);
                    } else {
                        notify_edit(&state);
                    }
                }
            }
        });
        buffer.connect_end_user_action({
            let state = Rc::downgrade(&state);
            move |_| {
                let Some(state) = state.upgrade() else { return };
                state
                    .user_action_depth
                    .set(state.user_action_depth.get().saturating_sub(1));
                if state.user_action_depth.get() == 0
                    && state.pending_edit.replace(false)
                    && !state.loading.get()
                {
                    notify_edit(&state);
                }
            }
        });
        let (search_panel, search_context) = search::install(&view, &buffer, &root);
        root.append(&search_panel.widget());
        root.append(&scroller);
        let style_manager = adw::StyleManager::for_display(&view.display());
        // GBinding disconnects automatically when the buffer is finalized.
        style_manager
            .bind_property("dark", &buffer, "style-scheme")
            .transform_to(|_, dark: bool| {
                Some(sourceview5::StyleSchemeManager::default().scheme(if dark {
                    "Adwaita-dark"
                } else {
                    "Adwaita"
                }))
            })
            .sync_create()
            .build();
        diagnostics::install(&view, &buffer, &state);
        install_shortcuts(&view, &root, &font_provider, &state);
        let editor = Self {
            root,
            view,
            buffer,
            scroller,
            scroll_mark,
            search_panel,
            _search_context: search_context,
            font_provider,
            state,
        };
        editor.set_font_size(config::DEFAULT_EDITOR_FONT_SIZE);
        editor.set_document(language, text);
        editor
    }

    pub fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub fn set_text(&self, text: &str) {
        if self.document_text() == text {
            return;
        }
        let scroll_offset = self.source_offset_at_scroll_top();
        self.state.loading.set(true);
        // Loading/reloading a document must neither trigger autosave nor enter undo history.
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(text);
        self.buffer.end_irreversible_action();
        self.buffer.set_modified(false);
        self.state.loading.set(false);
        self.clear_file_diff();
        self.set_source_offset_at_scroll_top(scroll_offset);
        log::debug!(
            "source editor document replaced bytes={} revision={}",
            text.len(),
            self.state.revision.get()
        );
    }

    pub fn set_document(&self, language: &str, text: &str) {
        self.set_document_id(language_support(language).id, text);
    }

    pub fn set_document_id(&self, language: LanguageId, text: &str) {
        self.set_language_id(language);
        self.set_text(text);
    }

    pub fn select_range(&self, start: usize, end: usize) {
        let text = self.document_text();
        let end = iter_at_byte(&self.buffer, &text, end.max(start));
        let start = iter_at_byte(&self.buffer, &text, start);
        self.buffer.select_range(&end, &start);
        self.view.grab_focus();
        self.view
            .scroll_to_mark(&self.buffer.selection_bound(), 0.1, false, 0.0, 0.0);
    }

    pub fn select_line_column(&self, line: usize, column: usize) {
        let offset = byte_offset_for_line_column(&self.document_text(), line, column);
        self.select_range(offset, offset);
    }

    pub fn set_file_diff(&self, comparison: Option<&FileComparison>) {
        self.clear_file_diff();
        let Some(comparison) = comparison else { return };
        let mut insertion_at = 0;
        for row in &comparison.rows {
            if let Some(number) = row.right_number {
                if row.right_kind == DiffKind::Added {
                    if let Some(iter) = self.buffer.iter_at_line(number.saturating_sub(1) as i32) {
                        self.buffer.create_source_mark(None, "git-added", &iter);
                    }
                }
                insertion_at = number;
            }
            if row.left_kind == DiffKind::Deleted && row.left_number.is_some() {
                let line = insertion_at.min(self.buffer.line_count().saturating_sub(1) as usize);
                if let Some(iter) = self.buffer.iter_at_line(line as i32) {
                    self.buffer.create_source_mark(None, "git-deleted", &iter);
                }
            }
        }
    }

    pub fn clear_file_diff(&self) {
        let (start, end) = self.buffer.bounds();
        for category in ["git-added", "git-deleted"] {
            self.buffer
                .remove_source_marks(&start, &end, Some(category));
        }
    }

    pub fn set_spellcheck_issues(&self, issues: Vec<SpellcheckIssue>) {
        diagnostics::apply_ranges(
            &self.buffer,
            "spellcheck",
            issues.iter().map(|issue| (issue.start, issue.end)),
        );
        self.state.spellcheck_issues.replace(issues);
        diagnostics::update_menu(&self.view, &self.state, self.buffer.cursor_position());
    }

    pub fn set_markdown_lint_issues(&self, issues: Vec<MarkdownLintIssue>) {
        diagnostics::apply_ranges(
            &self.buffer,
            "markdown-lint",
            issues.iter().map(|issue| (issue.start, issue.end)),
        );
        self.state.markdown_lint_issues.replace(issues);
        diagnostics::update_menu(&self.view, &self.state, self.buffer.cursor_position());
    }

    pub fn set_language(&self, language: &str) {
        self.set_language_id(language_support(language).id);
    }

    pub fn set_language_id(&self, language: LanguageId) {
        self.state.language.set(language);
        let manager = sourceview5::LanguageManager::default();
        let preferred = match language {
            LanguageId::PlainText => "",
            LanguageId::Bash => "sh",
            LanguageId::JavaScript => "js",
            LanguageId::Jsx => "jsx",
            LanguageId::Tsx => "typescript-jsx",
            LanguageId::Python => "python3",
            LanguageId::CSharp => "c-sharp",
            LanguageId::ObjectiveC => "objc",
            LanguageId::VisualBasic => "vbnet",
            LanguageId::Make => "makefile",
            LanguageId::MarkdownInline => "markdown",
            LanguageId::Hlsl | LanguageId::Slang => "cpp",
            _ => "",
        };
        let support = language_support_for_id(language);
        let syntax = (language != LanguageId::PlainText)
            .then(|| {
                std::iter::once(preferred)
                    .chain(support.aliases.iter().copied())
                    .find_map(|id| manager.language(id))
            })
            .flatten();
        self.buffer.set_language(syntax.as_ref());
    }

    pub fn set_font_size(&self, font_size: f64) {
        set_font_size(&self.font_provider, &self.state, font_size);
    }

    pub fn document_text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, true).to_string()
    }

    pub fn document_revision(&self) -> DocumentRevision {
        DocumentRevision(self.state.revision.get())
    }

    pub fn set_editable(&self, editable: bool) {
        self.view.set_editable(editable);
        self.view.set_cursor_visible(editable);
        diagnostics::update_menu(&self.view, &self.state, self.buffer.cursor_position());
    }

    pub fn set_read_only(&self, read_only: bool) {
        self.set_editable(!read_only);
    }

    pub fn connect_scroll_changed<F: Fn(f64) + 'static>(&self, callback: F) {
        self.scroller
            .vadjustment()
            .connect_value_changed(move |adjustment| callback(adjustment.value()));
    }

    pub fn source_offset_at_scroll_top(&self) -> usize {
        let (iter, _) = self.view.line_at_y(self.view.visible_rect().y());
        self.buffer
            .text(&self.buffer.start_iter(), &iter, true)
            .len()
    }

    pub fn set_source_offset_at_scroll_top(&self, offset: usize) {
        let iter = iter_at_byte(&self.buffer, &self.document_text(), offset);
        // scroll_to_mark also works before allocation; GTK scrolls after validating layout.
        self.buffer.move_mark(&self.scroll_mark, &iter);
        self.view
            .scroll_to_mark(&self.scroll_mark, 0.0, true, 0.0, 0.0);
    }

    pub fn connect_edit<F: Fn() + 'static>(&self, callback: F) {
        self.state
            .edit_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub fn connect_markdown_lint_ignore<F: Fn(String) + 'static>(&self, callback: F) {
        self.state
            .markdown_lint_ignore_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }
}

fn iter_at_byte(buffer: &sourceview5::Buffer, text: &str, offset: usize) -> gtk::TextIter {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    buffer.iter_at_offset(text[..offset].chars().count() as i32)
}

fn start_offset(text: &str, char_offset: i32) -> usize {
    text.char_indices()
        .nth(char_offset as usize)
        .map_or(text.len(), |(offset, _)| offset)
}

fn notify_edit(state: &EditorState) {
    let callbacks = state.edit_callbacks.borrow().clone();
    for callback in callbacks {
        callback();
    }
}

fn set_font_size(provider: &gtk::CssProvider, state: &EditorState, size: f64) {
    let size = config::normalize_font_size(size, config::DEFAULT_EDITOR_FONT_SIZE);
    state.font_size.set(size);
    provider.load_from_data(&format!("textview {{ font-size: {size}px; }}"));
}

fn install_shortcuts(
    view: &sourceview5::View,
    root: &gtk::Box,
    provider: &gtk::CssProvider,
    state: &Rc<EditorState>,
) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let view = view.downgrade();
        let provider = provider.clone();
        let state = Rc::downgrade(state);
        move |_, key, _, modifiers| {
            if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                || modifiers.contains(gdk::ModifierType::ALT_MASK)
            {
                return glib::Propagation::Proceed;
            }
            let (Some(view), Some(state)) = (view.upgrade(), state.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            let delta = match key {
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => Some(1.0),
                gdk::Key::minus | gdk::Key::underscore | gdk::Key::KP_Subtract => Some(-1.0),
                _ => None,
            };
            if let Some(delta) = delta {
                set_font_size(&provider, &state, state.font_size.get() + delta);
                config::save_editor_font_size(state.font_size.get());
                return glib::Propagation::Stop;
            }
            if matches!(key, gdk::Key::slash | gdk::Key::question) && view.is_editable() {
                let buffer = view
                    .buffer()
                    .downcast::<sourceview5::Buffer>()
                    .expect("source buffer");
                toggle_line_comment(&buffer, state.language.get());
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    });
    root.add_controller(keys);
}

fn toggle_line_comment(buffer: &sourceview5::Buffer, language: LanguageId) {
    let Some(prefix) = language_support_for_id(language).line_comment else {
        return;
    };
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    let selection = craic_render_skia::EditorSelection {
        anchor: start_offset(
            &text,
            buffer.iter_at_mark(&buffer.selection_bound()).offset(),
        ),
        focus: start_offset(&text, buffer.cursor_position()),
    };
    let Some(edit) = craic_render_skia::toggle_editor_line_comment(&text, selection, prefix) else {
        return;
    };
    let selection = edit.map_selection(selection);
    let mut start = iter_at_byte(buffer, &text, edit.range_start);
    let mut end = iter_at_byte(buffer, &text, edit.range_end);
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut end);
    buffer.insert(&mut start, &edit.replacement);
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    buffer.select_range(
        &iter_at_byte(buffer, &text, selection.focus),
        &iter_at_byte(buffer, &text, selection.anchor),
    );
    buffer.end_user_action();
}
