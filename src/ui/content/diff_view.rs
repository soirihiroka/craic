use super::{diff_canvas::DiffCanvas, widgets};
use crate::git::{DiffKind, FileComparison, FileDiffRow};
use crate::ui::components::search::SearchPanel;
use adw::prelude::*;
use std::cell::RefCell;
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
    search_panel: SearchPanel,
    canvas: DiffCanvas,
    full_rows: Rc<RefCell<Vec<FileDiffRow>>>,
    folds: Rc<RefCell<Vec<DiffFoldRange>>>,
    current_signature: Rc<RefCell<Option<DiffSignature>>>,
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

        let canvas = DiffCanvas::new();
        let search_panel = SearchPanel::new("Search diff");
        search_panel.set_clear_on_close(false);
        search_panel.set_options_visible(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&search_panel.widget());
        root.append(&canvas.root);
        search_panel.set_key_capture_widget(&root);
        search_panel.install_shortcuts(&root);
        search_panel.install_shortcuts(&canvas.root);
        connect_diff_search(&search_panel, &canvas);

        let full_rows = Rc::new(RefCell::new(Vec::new()));
        let folds = Rc::new(RefCell::new(Vec::new()));
        connect_diff_folds(&canvas, &search_panel, &full_rows, &folds);

        Self {
            root,
            title,
            stats,
            added,
            deleted,
            search_panel,
            canvas,
            full_rows,
            folds,
            current_signature: Rc::new(RefCell::new(None)),
        }
    }

    pub(in crate::ui) fn set_diff(&self, file_path: &str, comparison: &FileComparison) {
        self.title.set_label(file_path);
        self.added.set_label(&format!("+{}", comparison.insertions));
        self.deleted
            .set_label(&format!("-{}", comparison.deletions));
        self.stats.set_visible(true);

        let signature =
            DiffSignature::new(file_path, comparison.rows.len(), comparison.fingerprint);
        let previous_signature = self.current_signature.borrow().clone();
        if previous_signature.as_ref() == Some(&signature) {
            log::debug!(
                "diff_view unchanged path={} rows={} fingerprint={:016x}",
                file_path,
                signature.row_count,
                signature.fingerprint
            );
            return;
        }

        let old_scroll_y = self.canvas.scroll_y();
        let preserve_scroll = previous_signature
            .as_ref()
            .is_some_and(|previous| previous.file_path == file_path);
        let scroll_y = if preserve_scroll { old_scroll_y } else { 0.0 };
        let previous_folds = if preserve_scroll {
            self.folds.borrow().clone()
        } else {
            Vec::new()
        };
        self.full_rows.replace(comparison.rows.clone());
        let next_folds = build_initial_folds(&comparison.rows, &previous_folds);
        log::debug!(
            "diff_view refresh path={} rows={} folds={} previous_folds={} preserve_scroll={} scroll_y={:.1} fingerprint={:016x}",
            file_path,
            signature.row_count,
            next_folds.len(),
            previous_folds.len(),
            preserve_scroll,
            scroll_y,
            signature.fingerprint
        );
        self.folds.replace(next_folds);
        self.canvas
            .set_syntax_for_file(file_path, comparison.fingerprint, &comparison.rows);
        refresh_canvas(&self.canvas, &self.full_rows, &self.folds, scroll_y);
        self.search_panel.set_status(&self.canvas.search_status());
        self.current_signature.replace(Some(signature));
    }

    pub(in crate::ui) fn clear(&self, title_text: &str) {
        self.title.set_label(title_text);
        self.stats.set_visible(false);
        self.current_signature.borrow_mut().take();
        self.full_rows.borrow_mut().clear();
        self.folds.borrow_mut().clear();
        self.canvas.clear();
        self.search_panel.set_status(&self.canvas.search_status());
    }

    pub(in crate::ui) fn toggle_search(&self) {
        self.search_panel.toggle();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffSignature {
    file_path: String,
    row_count: usize,
    fingerprint: u64,
}

impl DiffSignature {
    fn new(file_path: &str, row_count: usize, fingerprint: u64) -> Self {
        Self {
            file_path: file_path.to_string(),
            row_count,
            fingerprint,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiffFoldRange {
    start: usize,
    end: usize,
    expanded: bool,
}

fn connect_diff_folds(
    canvas: &DiffCanvas,
    search_panel: &SearchPanel,
    full_rows: &Rc<RefCell<Vec<FileDiffRow>>>,
    folds: &Rc<RefCell<Vec<DiffFoldRange>>>,
) {
    canvas.set_fold_callback({
        let canvas = canvas.clone();
        let search_panel = search_panel.clone();
        let full_rows = full_rows.clone();
        let folds = folds.clone();
        move |fold_index| {
            if !toggle_fold(&folds, fold_index) {
                return;
            }
            let scroll_y = canvas.scroll_y();
            refresh_canvas(&canvas, &full_rows, &folds, scroll_y);
            search_panel.set_status(&canvas.search_status());
        }
    });
}

fn connect_diff_search(search_panel: &SearchPanel, canvas: &DiffCanvas) {
    search_panel.connect_query_changed({
        let search_panel = search_panel.clone();
        let canvas = canvas.clone();

        move |query| {
            canvas.set_search_query(&query);
            search_panel.set_status(&canvas.search_status());
        }
    });
    search_panel.connect_opened({
        let search_panel = search_panel.clone();
        let canvas = canvas.clone();

        move || {
            canvas.set_search_query(&search_panel.query());
            search_panel.set_status(&canvas.search_status());
        }
    });
    search_panel.connect_closed({
        let canvas = canvas.clone();

        move || canvas.focus()
    });
    search_panel.connect_previous({
        let search_panel = search_panel.clone();
        let canvas = canvas.clone();

        move || {
            canvas.search_previous();
            search_panel.set_status(&canvas.search_status());
        }
    });
    search_panel.connect_next({
        let search_panel = search_panel.clone();
        let canvas = canvas.clone();

        move || {
            canvas.search_next();
            search_panel.set_status(&canvas.search_status());
        }
    });
}

fn toggle_fold(folds: &Rc<RefCell<Vec<DiffFoldRange>>>, fold_index: usize) -> bool {
    let mut folds = folds.borrow_mut();
    let Some(fold) = folds.get_mut(fold_index) else {
        log::debug!("diff_view ignored stale fold toggle index={fold_index}");
        return false;
    };
    fold.expanded = !fold.expanded;
    log::debug!(
        "diff_view toggled fold index={} start={} end={} expanded={}",
        fold_index,
        fold.start,
        fold.end,
        fold.expanded
    );
    true
}

fn normalize_diff_folds(folds: &mut Vec<DiffFoldRange>, row_count: usize) {
    let before = folds.clone();
    let mut normalized: Vec<DiffFoldRange> = Vec::with_capacity(folds.len());

    folds.sort_by_key(|fold| (fold.start, fold.end));
    for mut fold in folds.iter().copied() {
        fold.start = fold.start.min(row_count);
        fold.end = fold.end.min(row_count);
        if fold.start >= fold.end {
            continue;
        }
        if let Some(previous) = normalized.last() {
            if fold.start < previous.end {
                fold.start = previous.end;
            }
            if fold.start >= fold.end {
                continue;
            }
        }
        normalized.push(fold);
    }

    if *folds == normalized {
        return;
    }

    log::debug!(
        "diff_view normalized folds before={} after={} row_count={row_count}",
        before.len(),
        normalized.len()
    );
    *folds = normalized;
}

fn refresh_canvas(
    canvas: &DiffCanvas,
    full_rows: &Rc<RefCell<Vec<FileDiffRow>>>,
    folds: &Rc<RefCell<Vec<DiffFoldRange>>>,
    scroll_y: f64,
) {
    let full_rows = full_rows.borrow();
    let mut folds = folds.borrow_mut();
    normalize_diff_folds(&mut folds, full_rows.len());
    let display_rows = display_rows(&full_rows, &folds);
    log::debug!(
        "diff_view canvas refresh source_rows={} display_rows={} folds={} expanded_folds={} scroll_y={:.1}",
        full_rows.len(),
        display_rows.len(),
        folds.len(),
        folds.iter().filter(|fold| fold.expanded).count(),
        scroll_y
    );
    canvas.set_rows(display_rows);
    canvas.set_scroll_y(scroll_y);
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

fn is_context_row(row: &FileDiffRow) -> bool {
    row.left_kind == DiffKind::Context && row.right_kind == DiffKind::Context
}

fn stats_label(text: &str) -> gtk::Label {
    let label = widgets::muted(text);
    label.add_css_class("numeric");
    label.set_wrap(false);
    label.set_valign(gtk::Align::Center);
    label
}
