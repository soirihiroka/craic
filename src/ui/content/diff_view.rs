use super::{code_editor, widgets};
use crate::config;
use crate::git::{DiffKind, FileComparison, FileDiffRow};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const FOLD_KEEP_CONTEXT: usize = 3;
const FOLD_MIN_HIDDEN: usize = 8;
#[derive(Clone)]
pub(in crate::ui) struct DiffView {
    pub(in crate::ui) root: gtk::Box,
    title: gtk::Label,
    stats: gtk::Box,
    added: gtk::Label,
    deleted: gtk::Label,
    left_editor: code_editor::CodeEditor,
    right_editor: code_editor::CodeEditor,
    full_rows: Rc<RefCell<Vec<FileDiffRow>>>,
    folds: Rc<RefCell<Vec<DiffFoldRange>>>,
    language: Rc<RefCell<String>>,
    current_signature: Rc<RefCell<Option<String>>>,
}

impl DiffView {
    pub(in crate::ui) fn new(title: &str) -> Self {
        let title = widgets::heading(title);
        title.set_wrap(false);
        title.set_lines(1);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_width_chars(1);
        title.set_hexpand(true);

        let added = stats_label("");
        added.add_css_class("success");
        let deleted = stats_label("");
        deleted.add_css_class("error");
        let stats = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        stats.append(&added);
        stats.append(&deleted);

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        header.append(&title);
        header.append(&stats);

        let font_size = config::load().font_sizes.diff;
        let left_editor = code_editor::CodeEditor::new("", "");
        let right_editor = code_editor::CodeEditor::new("", "");
        left_editor.set_font_size(font_size);
        right_editor.set_font_size(font_size);
        configure_diff_editor(&left_editor, code_editor::GutterSide::Right, false);
        configure_diff_editor(&right_editor, code_editor::GutterSide::Left, true);
        connect_diff_font_size_shortcuts(&left_editor, &right_editor);

        let divider = gtk::Separator::new(gtk::Orientation::Vertical);
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .vexpand(true)
            .build();
        body.append(&left_editor.root);
        body.append(&divider);
        body.append(&right_editor.root);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&body);

        let full_rows = Rc::new(RefCell::new(Vec::new()));
        let folds = Rc::new(RefCell::new(Vec::new()));
        let language = Rc::new(RefCell::new(String::new()));
        let syncing = Rc::new(Cell::new(false));

        connect_shared_scroll(&left_editor, &right_editor, &syncing);
        connect_diff_folds(&left_editor, &right_editor, &full_rows, &folds, &language);

        Self {
            root,
            title,
            stats,
            added,
            deleted,
            left_editor,
            right_editor,
            full_rows,
            folds,
            language,
            current_signature: Rc::new(RefCell::new(None)),
        }
    }

    pub(in crate::ui) fn set_diff(&self, file_path: &str, comparison: &FileComparison) {
        self.title.set_label(file_path);
        let (insertions, deletions) = diff_line_stats(&comparison.rows);
        self.added.set_label(&format!("+{insertions}"));
        self.deleted.set_label(&format!("-{deletions}"));
        self.stats.set_visible(true);

        let signature = format!("{file_path}\0{:?}", comparison.rows);
        if self.current_signature.borrow().as_ref() == Some(&signature) {
            return;
        }

        let old_scroll_y = self.right_editor.scroll_y();
        self.language
            .replace(code_editor::language_hint_from_path(file_path));
        let previous_folds = self.folds.borrow().clone();
        self.full_rows.replace(comparison.rows.clone());
        self.folds
            .replace(build_initial_folds(&comparison.rows, &previous_folds));
        refresh_editors(
            &self.left_editor,
            &self.right_editor,
            &self.full_rows,
            &self.folds,
            &self.language,
            old_scroll_y,
        );
        self.current_signature.replace(Some(signature));
    }

    pub(in crate::ui) fn clear(&self, title_text: &str) {
        self.title.set_label(title_text);
        self.stats.set_visible(false);
        self.current_signature.borrow_mut().take();
        self.full_rows.borrow_mut().clear();
        self.folds.borrow_mut().clear();
        self.language.borrow_mut().clear();
        refresh_editors(
            &self.left_editor,
            &self.right_editor,
            &self.full_rows,
            &self.folds,
            &self.language,
            0.0,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiffFoldRange {
    start: usize,
    end: usize,
    expanded: bool,
}

fn configure_diff_editor(
    editor: &code_editor::CodeEditor,
    side: code_editor::GutterSide,
    scrollbar_visible: bool,
) {
    editor.set_read_only(true);
    editor.set_auto_folding_enabled(false);
    editor.set_gutter_side(side);
    editor.set_scrollbar_visible(scrollbar_visible);
    editor.root.set_vexpand(true);
}

fn connect_diff_font_size_shortcuts(
    left_editor: &code_editor::CodeEditor,
    right_editor: &code_editor::CodeEditor,
) {
    for editor in [left_editor, right_editor] {
        editor.set_font_size_adjust_callback({
            let left_editor = left_editor.clone();
            let right_editor = right_editor.clone();

            move |delta| {
                let next = config::normalize_font_size(
                    right_editor.font_size() + delta,
                    config::DEFAULT_DIFF_FONT_SIZE,
                );
                left_editor.set_font_size(next);
                right_editor.set_font_size(next);
                config::save_diff_font_size(next);
            }
        });
    }
}

fn connect_shared_scroll(
    left_editor: &code_editor::CodeEditor,
    right_editor: &code_editor::CodeEditor,
    syncing: &Rc<Cell<bool>>,
) {
    left_editor.connect_scroll_changed({
        let right_editor = right_editor.clone();
        let syncing = syncing.clone();
        move |scroll_y| {
            if syncing.replace(true) {
                syncing.set(true);
                return;
            }
            right_editor.set_scroll_y(scroll_y);
            syncing.set(false);
        }
    });
    right_editor.connect_scroll_changed({
        let left_editor = left_editor.clone();
        let syncing = syncing.clone();
        move |scroll_y| {
            if syncing.replace(true) {
                syncing.set(true);
                return;
            }
            left_editor.set_scroll_y(scroll_y);
            syncing.set(false);
        }
    });
}

fn connect_diff_folds(
    left_editor: &code_editor::CodeEditor,
    right_editor: &code_editor::CodeEditor,
    full_rows: &Rc<RefCell<Vec<FileDiffRow>>>,
    folds: &Rc<RefCell<Vec<DiffFoldRange>>>,
    language: &Rc<RefCell<String>>,
) {
    for editor in [left_editor, right_editor] {
        editor.set_diff_fold_callback({
            let left_editor = left_editor.clone();
            let right_editor = right_editor.clone();
            let full_rows = full_rows.clone();
            let folds = folds.clone();
            let language = language.clone();
            move |fold_index| {
                toggle_fold(&folds, fold_index);
                let scroll_y = right_editor.scroll_y();
                let left_editor = left_editor.clone();
                let right_editor = right_editor.clone();
                let full_rows = full_rows.clone();
                let folds = folds.clone();
                let language = language.clone();
                gtk::glib::idle_add_local_once(move || {
                    refresh_editors(
                        &left_editor,
                        &right_editor,
                        &full_rows,
                        &folds,
                        &language,
                        scroll_y,
                    );
                });
            }
        });
    }
}

fn toggle_fold(folds: &Rc<RefCell<Vec<DiffFoldRange>>>, fold_index: usize) {
    let mut folds = folds.borrow_mut();
    let Some(fold) = folds.get_mut(fold_index) else {
        return;
    };
    fold.expanded = !fold.expanded;
}

fn refresh_editors(
    left_editor: &code_editor::CodeEditor,
    right_editor: &code_editor::CodeEditor,
    full_rows: &Rc<RefCell<Vec<FileDiffRow>>>,
    folds: &Rc<RefCell<Vec<DiffFoldRange>>>,
    language: &Rc<RefCell<String>>,
    scroll_y: f64,
) {
    let display_rows = display_rows(&full_rows.borrow(), &folds.borrow());
    let language = language.borrow().clone();
    let (left_document, right_document) = editor_documents(&display_rows, &language);
    let markers = scrollbar_markers(&display_rows);

    left_editor.set_diff_document(left_document);
    right_editor.set_diff_document(right_document);
    left_editor.set_scrollbar_markers(Vec::new());
    right_editor.set_scrollbar_markers(markers);
    left_editor.set_scroll_y(scroll_y);
    right_editor.set_scroll_y(scroll_y);
}

fn editor_documents(
    rows: &[FileDiffRow],
    language: &str,
) -> (
    code_editor::DiffEditorDocument,
    code_editor::DiffEditorDocument,
) {
    let mut left_rows = Vec::with_capacity(rows.len());
    let mut right_rows = Vec::with_capacity(rows.len());

    for row in rows {
        let left_text = row.left_text.clone().unwrap_or_default();
        let right_text = row.right_text.clone().unwrap_or_default();
        left_rows.push(code_editor::DiffEditorRow {
            number: row.left_number.filter(|_| !is_fold_row(row)),
            text: left_text.clone(),
            paired_text: right_text.clone(),
            kind: editor_kind(row.left_number, row.left_text.as_ref(), row.left_kind),
            fold_index: fold_index(row),
            fold_expanded: fold_expanded(row),
            show_fold_control: false,
        });
        right_rows.push(code_editor::DiffEditorRow {
            number: row.right_number.filter(|_| !is_fold_row(row)),
            text: right_text,
            paired_text: left_text,
            kind: editor_kind(row.right_number, row.right_text.as_ref(), row.right_kind),
            fold_index: fold_index(row),
            fold_expanded: fold_expanded(row),
            show_fold_control: is_fold_row(row),
        });
    }

    (
        code_editor::DiffEditorDocument {
            rows: left_rows,
            language: language.to_string(),
        },
        code_editor::DiffEditorDocument {
            rows: right_rows,
            language: language.to_string(),
        },
    )
}

fn editor_kind(
    number: Option<usize>,
    text: Option<&String>,
    kind: DiffKind,
) -> code_editor::EditorDiffKind {
    if kind == DiffKind::Fold {
        return code_editor::EditorDiffKind::Fold;
    }
    if number.is_none() && text.is_none() {
        return code_editor::EditorDiffKind::Missing;
    }
    match kind {
        DiffKind::Context => code_editor::EditorDiffKind::Context,
        DiffKind::Deleted => code_editor::EditorDiffKind::Deleted,
        DiffKind::Added => code_editor::EditorDiffKind::Added,
        DiffKind::Fold => code_editor::EditorDiffKind::Fold,
    }
}

fn build_initial_folds(
    rows: &[FileDiffRow],
    previous_folds: &[DiffFoldRange],
) -> Vec<DiffFoldRange> {
    let mut folds = Vec::new();
    let mut index = 0;

    while index < rows.len() {
        if !is_context_row(&rows[index]) {
            index += 1;
            continue;
        }

        let run_start = index;
        while index < rows.len() && is_context_row(&rows[index]) {
            index += 1;
        }
        let run_end = index;
        let has_before = run_start > 0;
        let has_after = run_end < rows.len();
        let keep_before = if has_before { FOLD_KEEP_CONTEXT } else { 0 };
        let keep_after = if has_after { FOLD_KEEP_CONTEXT } else { 0 };
        let fold_start = (run_start + keep_before).min(run_end);
        let fold_end = run_end.saturating_sub(keep_after).max(fold_start);

        if fold_end.saturating_sub(fold_start) >= FOLD_MIN_HIDDEN {
            let previous = previous_folds
                .iter()
                .find(|fold| fold.start == fold_start && fold.end == fold_end)
                .copied();
            folds.push(previous.unwrap_or(DiffFoldRange {
                start: fold_start,
                end: fold_end,
                expanded: false,
            }));
        }
    }

    folds
}

fn display_rows(full_rows: &[FileDiffRow], folds: &[DiffFoldRange]) -> Vec<FileDiffRow> {
    if folds.is_empty() {
        return full_rows.to_vec();
    }

    let mut rows = Vec::new();
    let mut source_index = 0;

    for (fold_index, fold) in folds.iter().copied().enumerate() {
        while source_index < fold.start {
            if let Some(row) = full_rows.get(source_index) {
                rows.push(row.clone());
            }
            source_index += 1;
        }

        rows.push(fold_row(fold_index, fold));

        if fold.expanded {
            while source_index < fold.end {
                if let Some(row) = full_rows.get(source_index) {
                    rows.push(row.clone());
                }
                source_index += 1;
            }
        } else {
            source_index = fold.end;
        }
    }

    while source_index < full_rows.len() {
        if let Some(row) = full_rows.get(source_index) {
            rows.push(row.clone());
        }
        source_index += 1;
    }

    rows
}

fn fold_row(fold_index: usize, fold: DiffFoldRange) -> FileDiffRow {
    let count = fold.end.saturating_sub(fold.start);
    let label = match (fold.expanded, count) {
        (true, 1) => "- 1 shown line".to_string(),
        (true, count) => format!("- {count} shown lines"),
        (false, 1) => "+ 1 hidden line".to_string(),
        (false, count) => format!("+ {count} hidden lines"),
    };

    FileDiffRow {
        left_number: Some(fold_index),
        right_number: Some(fold_index),
        left_text: Some(label.clone()),
        right_text: Some(label),
        left_kind: DiffKind::Fold,
        right_kind: DiffKind::Fold,
    }
}

fn fold_index(row: &FileDiffRow) -> Option<usize> {
    is_fold_row(row).then_some(row.left_number?)
}

fn fold_expanded(row: &FileDiffRow) -> bool {
    is_fold_row(row)
        && row
            .left_text
            .as_deref()
            .is_some_and(|text| text.starts_with("- "))
}

fn scrollbar_markers(rows: &[FileDiffRow]) -> Vec<code_editor::ScrollbarMarker> {
    rows.iter()
        .enumerate()
        .filter_map(|(row_index, row)| {
            let added = row.left_kind == DiffKind::Added || row.right_kind == DiffKind::Added;
            let deleted = row.left_kind == DiffKind::Deleted || row.right_kind == DiffKind::Deleted;
            let kind = match (added, deleted) {
                (true, true) => code_editor::ScrollbarMarkerKind::Mixed,
                (true, false) => code_editor::ScrollbarMarkerKind::Added,
                (false, true) => code_editor::ScrollbarMarkerKind::Deleted,
                (false, false) => return None,
            };
            Some(code_editor::ScrollbarMarker {
                row: row_index,
                kind,
            })
        })
        .collect()
}

fn is_context_row(row: &FileDiffRow) -> bool {
    row.left_kind == DiffKind::Context && row.right_kind == DiffKind::Context
}

fn is_fold_row(row: &FileDiffRow) -> bool {
    row.left_kind == DiffKind::Fold || row.right_kind == DiffKind::Fold
}

fn diff_line_stats(rows: &[FileDiffRow]) -> (usize, usize) {
    let insertions = rows
        .iter()
        .filter(|row| row.right_kind == DiffKind::Added)
        .count();
    let deletions = rows
        .iter()
        .filter(|row| row.left_kind == DiffKind::Deleted)
        .count();

    (insertions, deletions)
}

fn stats_label(text: &str) -> gtk::Label {
    let label = widgets::muted(text);
    label.add_css_class("numeric");
    label.set_wrap(false);
    label.set_valign(gtk::Align::Center);
    label
}
