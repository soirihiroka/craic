use super::{
    BrowserTarget, FileBrowser, SearchMatch, TransferRowProgress, show_row_context_menu,
    tree::{BrowserRow, TreeRowRole},
};
use crate::system::FileNodePath;
use crate::system::capabilities::files::FileNodeKind;
use crate::ui::{canvas_scrollbar, components::tree_view, file_status, file_type};
use gtk::gdk;
use gtk::prelude::*;
use std::rc::Rc;

const ROOT_GAP_HEIGHT: f64 = tree_view::ICON_ROW_HEIGHT_F64 * 4.0;
const STATUS_ICON_CLASS: &str = "repo-browser-status-icon";
const STATUS_ICON_END_MARGIN: i32 = -4;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum BrowserListRow {
    Tree(BrowserRow),
    NewEntry(NewEntryRow),
    RenameEntry(RenameEntryRow),
    Loading(LoadingRow),
    Search(SearchMatch),
    Status(String),
    RootGap,
}

impl BrowserListRow {
    pub(super) fn height(&self) -> f64 {
        match self {
            Self::Tree(_)
            | Self::NewEntry(_)
            | Self::RenameEntry(_)
            | Self::Loading(_)
            | Self::Search(_) => tree_view::ICON_ROW_HEIGHT_F64,
            Self::Status(_) => tree_view::ICON_ROW_HEIGHT_F64,
            Self::RootGap => ROOT_GAP_HEIGHT,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct NewEntryRow {
    pub(super) folder: FileNodePath,
    pub(super) default_name: String,
    pub(super) depth: usize,
    pub(super) kind: super::NewEntryKind,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RenameEntryRow {
    pub(super) row: BrowserRow,
    pub(super) original_name: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LoadingRow {
    pub(super) folder: FileNodePath,
    pub(super) depth: usize,
}

impl FileBrowser {
    pub(super) fn connect_browser_rows(self: &Rc<Self>) {
        self.install_row_drop_targets();

        self.tree.connect_pointer_press({
            let browser = self.clone();

            move |gesture, x, y, content_y, hit| {
                let hit = hit
                    .map(|row| row.state.to_row())
                    .filter(|row| !matches!(row, BrowserListRow::RootGap));
                if browser.pending_new_entry.borrow().is_some() {
                    if matches!(
                        hit.as_ref(),
                        Some(BrowserListRow::NewEntry(_)) | Some(BrowserListRow::RenameEntry(_))
                    ) {
                        return;
                    }
                    browser.queue_cancel_pending_new_entry();
                }
                if hit.is_some() {
                    return;
                }

                match gesture.current_button() {
                    1 => {
                        browser.unselect_file_browser();
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    }
                    3 => {
                        let target =
                            browser.target_for_node_path(browser.gap_context_folder(content_y));
                        show_row_context_menu(&browser, &browser.tree.scroller, target, x, y);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    }
                    _ => {}
                }
            }
        });
    }

    fn install_row_drop_targets(self: &Rc<Self>) {
        tree_view::FileDropTarget::builder(super::FILE_TRANSFER_MIME_TYPES)
            .on_file_hover({
                let browser = self.clone();

                move |paths, y, actions, modifiers| {
                    browser.handle_drop_hover(
                        paths.as_ref().is_some_and(|paths| !paths.is_empty()),
                        browser.drop_target_folder_at_y(y),
                        actions,
                        modifiers,
                    )
                }
            })
            .on_file_drop({
                let browser = self.clone();

                move |sources, y, actions, modifiers| {
                    let target_path = browser.drop_target_folder_at_y(y);
                    browser.handle_dropped_paths(
                        !sources.is_empty(),
                        target_path,
                        actions,
                        modifiers,
                    )
                }
            })
            .on_async_hover({
                let browser = self.clone();

                move |y, actions, modifiers| {
                    browser.handle_drop_hover(
                        false,
                        browser.drop_target_folder_at_y(y),
                        actions,
                        modifiers,
                    )
                }
            })
            .on_async_drop({
                let browser = self.clone();

                move |_drop, y, actions, modifiers| {
                    let target_path = browser.drop_target_folder_at_y(y);
                    browser.handle_dropped_paths(false, target_path, actions, modifiers)
                }
            })
            .on_leave({
                let browser = self.clone();

                move || browser.clear_drop_target_folder()
            })
            .build()
            .install(&self.tree.list);
    }

    pub(super) fn set_browser_rows(self: &Rc<Self>, mut rows: Vec<BrowserListRow>) {
        if !matches!(rows.last(), Some(BrowserListRow::RootGap)) {
            rows.push(BrowserListRow::RootGap);
        }

        let restore_focus = self.file_view_has_focus();
        let tree_rows = rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.browser_tree_row(row, index))
            .collect::<Vec<_>>();

        let mount_browser = self.clone();
        let update_browser = self.clone();
        let renderer = tree_view::TreeRenderer::new(
            move |index, _, state| mount_browser.row_widget(index, state),
            move |index, widget, previous, next| {
                update_browser.update_row_widget(index, widget, previous, next);
            },
        );
        let stats = self.tree.set_rows(tree_rows, renderer);
        self.list_rows.replace(rows);

        if stats.changed() {
            self.tree.list.queue_resize();
            self.tree.list.queue_draw();
        }
        if restore_focus && stats.changed() {
            self.focus_selected_row();
        }
    }

    pub(super) fn refresh_browser_row_state(self: &Rc<Self>) {
        let rows = self.list_rows.borrow().clone();
        self.set_browser_rows(rows);
    }

    fn update_row_widget(
        self: &Rc<Self>,
        index: usize,
        widget: &gtk::Widget,
        previous: &tree_view::TreeRenderState<BrowserListRowKey, BrowserListRowRenderState>,
        next: &tree_view::TreeRenderState<BrowserListRowKey, BrowserListRowRenderState>,
    ) {
        if previous.sticky != next.sticky
            || previous.bottom != next.bottom
            || previous.width != next.width
        {
            widget.set_size_request(next.width, next.row.height as i32);
            tree_view::sync_icon_row_bottom_sticky(widget, next.bottom);
        }

        match (&previous.row.state, &next.row.state) {
            (
                BrowserListRowRenderState::Tree {
                    row: previous_row,
                    expanded: previous_expanded,
                    selected: previous_selected,
                    drop_target: previous_drop_target,
                    status: previous_status,
                    transfer_progress: previous_transfer_progress,
                },
                BrowserListRowRenderState::Tree {
                    row,
                    expanded,
                    selected,
                    drop_target,
                    status,
                    transfer_progress,
                },
            ) => {
                self.update_tree_row_widget(
                    widget,
                    previous_row,
                    *previous_expanded,
                    *previous_selected,
                    *previous_drop_target,
                    previous_status.as_deref(),
                    previous_transfer_progress.as_ref(),
                    row,
                    *expanded,
                    *selected,
                    *drop_target,
                    status.as_deref(),
                    transfer_progress.as_ref(),
                );
            }
            (
                BrowserListRowRenderState::Search {
                    search_match: previous,
                    selected: previous_selected,
                },
                BrowserListRowRenderState::Search {
                    search_match: next,
                    selected,
                },
            ) => {
                update_search_row_widget(widget, previous, *previous_selected, next, *selected);
            }
            (
                BrowserListRowRenderState::NewEntry(previous),
                BrowserListRowRenderState::NewEntry(next),
            ) => {
                update_new_entry_row_widget(widget, previous, next);
            }
            (
                BrowserListRowRenderState::RenameEntry {
                    row: previous,
                    expanded: previous_expanded,
                },
                BrowserListRowRenderState::RenameEntry {
                    row: next,
                    expanded,
                },
            ) => {
                update_rename_entry_row_widget(
                    widget,
                    previous,
                    *previous_expanded,
                    next,
                    *expanded,
                );
            }
            (
                BrowserListRowRenderState::Status(previous),
                BrowserListRowRenderState::Status(next),
            ) => {
                update_status_row_widget(widget, previous, next);
            }
            (BrowserListRowRenderState::Loading(_), BrowserListRowRenderState::Loading(_)) => {}
            (BrowserListRowRenderState::RootGap, BrowserListRowRenderState::RootGap) => {}
            _ => replace_row_widget(widget, self.row_widget(index, next)),
        }
    }

    fn update_tree_row_widget(
        self: &Rc<Self>,
        widget: &gtk::Widget,
        previous_row: &BrowserRow,
        previous_expanded: bool,
        previous_selected: bool,
        previous_drop_target: bool,
        previous_status: Option<&str>,
        previous_transfer_progress: Option<&TransferRowProgress>,
        row: &BrowserRow,
        expanded: bool,
        selected: bool,
        drop_target: bool,
        status: Option<&str>,
        transfer_progress: Option<&TransferRowProgress>,
    ) {
        if previous_row.depth != row.depth {
            tree_view::sync_icon_row_depth(widget, row.depth);
        }
        if previous_selected != selected {
            tree_view::sync_icon_row_selected(widget, selected);
        }
        if previous_drop_target != drop_target {
            tree_view::sync_icon_row_drop_target(widget, drop_target);
        }
        if previous_row.ignore != row.ignore {
            if let Some(disclosure) = tree_view::icon_row_disclosure(widget) {
                tree_view::sync_dimmed(&disclosure, row.ignore.is_ignored());
            }
            if let Some(icon) = tree_view::icon_row_icon(widget) {
                tree_view::sync_dimmed(&icon, row.ignore.is_ignored());
            }
            if let Some(label) = tree_view::icon_row_title(widget) {
                tree_view::sync_dimmed(&label, row.ignore.is_ignored());
            }
        }
        if previous_expanded != expanded
            && row.tree_role == TreeRowRole::Branch
            && let Some(handle) = tree_view::icon_row_disclosure(widget)
        {
            let key = tree_row_key(row);
            let should_animate = self.tree.prepare_disclosure(&key, expanded);
            if should_animate {
                self.tree.animate_disclosure(&handle, key);
            } else {
                handle.queue_draw();
            }
        }
        if previous_transfer_progress != transfer_progress {
            sync_transfer_progress(self, widget, &row.node_path, transfer_progress);
        }
        if previous_status != status {
            sync_status_icon(widget, status);
        }
    }

    fn row_widget(
        self: &Rc<Self>,
        index: usize,
        render: &tree_view::TreeRenderState<BrowserListRowKey, BrowserListRowRenderState>,
    ) -> gtk::Widget {
        let widget = match &render.row.state {
            BrowserListRowRenderState::Tree {
                row,
                expanded,
                selected,
                drop_target,
                status,
                transfer_progress,
            } => self.tree_row_widget(
                row,
                *expanded,
                *selected,
                *drop_target,
                status.as_deref(),
                transfer_progress.as_ref(),
                render,
            ),
            BrowserListRowRenderState::NewEntry(row) => self.new_entry_row_widget(row),
            BrowserListRowRenderState::RenameEntry { row, expanded } => {
                self.rename_entry_row_widget(row, *expanded)
            }
            BrowserListRowRenderState::Loading(row) => self.loading_row_widget(row),
            BrowserListRowRenderState::Search {
                search_match,
                selected,
            } => self.search_row_widget(search_match, *selected),
            BrowserListRowRenderState::Status(message) => self.status_row_widget(message, index),
            BrowserListRowRenderState::RootGap => root_gap_row_widget(),
        };
        widget.set_size_request(render.width, render.row.height as i32);
        widget.upcast()
    }

    fn tree_row_widget(
        self: &Rc<Self>,
        row: &BrowserRow,
        expanded: bool,
        selected: bool,
        drop_target: bool,
        status: Option<&str>,
        transfer_progress: Option<&TransferRowProgress>,
        render: &tree_view::TreeRenderState<BrowserListRowKey, BrowserListRowRenderState>,
    ) -> gtk::Box {
        let mut builder = tree_view::IconRow::builder(&row.name);
        if !row.is_dir {
            builder = builder.set_icon(row_file_icon(row));
        }
        builder = builder
            .depth(row.depth)
            .selected(selected)
            .sticky(render.sticky)
            .bottom_sticky(render.bottom)
            .end_padding(row_end_padding())
            .on_primary_click(tree_primary_click_handler(self, row.clone(), render.sticky))
            .on_secondary_click(tree_secondary_click_handler(self, row.clone()))
            .drag_source(file_drag_source(self, row.node_path.clone()));
        if row.tree_role == TreeRowRole::Branch {
            let handle = self.tree.disclosure_widget(tree_row_key(row), expanded);
            tree_view::sync_dimmed(&handle, row.ignore.is_ignored());
            builder = builder.disclosure(handle);
        }
        if let Some(progress) = transfer_progress {
            let browser = self.clone();
            let row_path = row.node_path.clone();
            builder = builder.progress(icon_row_progress(progress), move || {
                if let Some(progress) = browser.transfer_progress_for_path(&row_path) {
                    browser.confirm_cancel_transfers(progress.transfer_ids);
                }
            });
        }
        if !row.capabilities.writable {
            builder = builder.trailing(read_only_icon());
        }

        let icon_row = builder.build();
        tree_view::sync_dimmed(&icon_row.title, row.ignore.is_ignored());
        tree_view::sync_icon_row_drop_target(&icon_row.root, drop_target);
        sync_status_icon(&icon_row.root.clone().upcast(), status);
        if render.sticky && row.is_dir {
            self.install_folder_drop_targets(&icon_row.root, row.node_path.clone());
        }
        icon_row.root
    }

    fn new_entry_row_widget(self: &Rc<Self>, row: &NewEntryRow) -> gtk::Box {
        let mut builder = tree_view::IconRow::builder(&row.default_name);
        if row.kind != super::NewEntryKind::Folder {
            builder = builder.set_icon(file_row_icon(&row.default_name, false));
        }
        let browser = self.clone();
        let activate_row = row.clone();
        let escape_browser = self.clone();
        let focus_browser = self.clone();
        let allowlist = self.spellcheck_allowlist.borrow().clone();
        builder
            .depth(row.depth)
            .end_padding(row_end_padding())
            .editable()
            .on_edit_changed(move |entry, icon| {
                let name = entry.text();
                sync_new_file_icon(icon, name.as_str());
                sync_spellcheck_entry_warning(entry, name.as_str(), &allowlist);
            })
            .on_edit_activate(move |name| {
                browser.finish_pending_new_entry(&activate_row.folder, activate_row.kind, name);
            })
            .on_edit_escape(move || {
                escape_browser.focus_browser_shell();
                escape_browser.cancel_pending_new_entry();
            })
            .on_edit_focus_leave(move || {
                focus_browser.queue_cancel_pending_new_entry();
            })
            .build()
            .root
    }

    fn rename_entry_row_widget(self: &Rc<Self>, row: &RenameEntryRow, expanded: bool) -> gtk::Box {
        let mut builder = tree_view::IconRow::builder(&row.original_name);
        if !row.row.is_dir {
            builder = builder.set_icon(row_file_icon(&row.row));
        }
        builder = builder
            .depth(row.row.depth)
            .dimmed(row.row.ignore.is_ignored())
            .end_padding(row_end_padding())
            .editable();
        if row.row.tree_role == TreeRowRole::Branch {
            let handle = self
                .tree
                .disclosure_widget(rename_row_key(&row.row.node_path), expanded);
            tree_view::sync_dimmed(&handle, row.row.ignore.is_ignored());
            builder = builder.disclosure(handle);
        }
        let target = BrowserTarget::from_row(&row.row);
        let browser = self.clone();
        let escape_browser = self.clone();
        let focus_browser = self.clone();
        let allowlist = self.spellcheck_allowlist.borrow().clone();
        builder
            .on_edit_changed(move |entry, _| {
                sync_spellcheck_entry_warning(entry, entry.text().as_str(), &allowlist);
            })
            .on_edit_activate(move |name| {
                browser.finish_pending_rename(&target, name);
            })
            .on_edit_escape(move || {
                escape_browser.focus_browser_shell();
                escape_browser.cancel_pending_rename();
            })
            .on_edit_focus_leave(move || {
                focus_browser.queue_cancel_pending_rename();
            })
            .build()
            .root
    }

    fn loading_row_widget(self: &Rc<Self>, row: &LoadingRow) -> gtk::Box {
        let icon_row = tree_view::IconRow::builder("Loading...")
            .set_icon(loading_disclosure_spinner())
            .depth(row.depth)
            .end_padding(row_end_padding())
            .build();
        icon_row.root
    }

    fn search_row_widget(self: &Rc<Self>, search_match: &SearchMatch, selected: bool) -> gtk::Box {
        let line = gtk::Label::builder()
            .label(format!("{}:", search_match.line_number))
            .xalign(1.0)
            .width_request(42)
            .build();
        line.add_css_class("dim-label");
        line.add_css_class("caption");

        let row = tree_view::IconRow::builder(search_match.text.trim())
            .set_icon(line)
            .depth(search_match.depth)
            .selected(selected)
            .end_padding(row_end_padding())
            .on_primary_click({
                let browser = self.clone();
                let search_match = search_match.clone();

                move |_, _, _, _| {
                    browser.queue_cancel_pending_new_entry();
                    browser.set_selected_search_match(search_match.clone());
                }
            })
            .on_secondary_click({
                let browser = self.clone();
                let search_match = search_match.clone();

                move |parent, gesture, x, y| {
                    browser.queue_cancel_pending_new_entry();
                    browser.active_folder.replace(
                        search_match
                            .node_path
                            .parent()
                            .unwrap_or_else(|| browser.root_node_path()),
                    );
                    browser.set_selected_node_path(Some(search_match.node_path.clone()));
                    show_row_context_menu(
                        &browser,
                        parent,
                        browser.target_for_node_path(search_match.node_path.clone()),
                        x,
                        y,
                    );
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            })
            .drag_source(file_drag_source(self, search_match.node_path.clone()))
            .build();
        row.root.add_css_class("repo-browser-search-row");
        row.title.add_css_class("dim-label");
        row.title.add_css_class("caption");
        row.root
    }

    fn status_row_widget(self: &Rc<Self>, message: &str, index: usize) -> gtk::Box {
        let row = tree_view::IconRow::builder(message)
            .end_padding(row_end_padding())
            .on_secondary_click({
                let browser = self.clone();

                move |parent, gesture, x, y| {
                    browser.queue_cancel_pending_new_entry();
                    let content_y = browser
                        .list_rows
                        .borrow()
                        .iter()
                        .take(index)
                        .map(BrowserListRow::height)
                        .sum::<f64>()
                        + y;
                    let target =
                        browser.target_for_node_path(browser.gap_context_folder(content_y));
                    show_row_context_menu(&browser, parent, target, x, y);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            })
            .build();
        row.root.add_css_class("repo-browser-status-row");
        row.title.add_css_class("dim-label");
        row.root
    }

    pub(super) fn install_folder_drop_targets<W: IsA<gtk::Widget>>(
        self: &Rc<Self>,
        widget: &W,
        folder: FileNodePath,
    ) {
        tree_view::FileDropTarget::builder(super::FILE_TRANSFER_MIME_TYPES)
            .on_file_hover({
                let browser = self.clone();
                let folder = folder.clone();

                move |paths, _, actions, modifiers| {
                    browser.handle_drop_hover(
                        paths.as_ref().is_some_and(|paths| !paths.is_empty()),
                        folder.clone(),
                        actions,
                        modifiers,
                    )
                }
            })
            .on_file_drop({
                let browser = self.clone();
                let folder = folder.clone();

                move |sources, _, actions, modifiers| {
                    browser.handle_dropped_paths(
                        !sources.is_empty(),
                        folder.clone(),
                        actions,
                        modifiers,
                    )
                }
            })
            .on_async_hover({
                let browser = self.clone();
                let folder = folder.clone();

                move |_, actions, modifiers| {
                    browser.handle_drop_hover(false, folder.clone(), actions, modifiers)
                }
            })
            .on_async_drop({
                let browser = self.clone();

                move |_drop, _, actions, modifiers| {
                    let folder = folder.clone();
                    browser.handle_dropped_paths(false, folder, actions, modifiers)
                }
            })
            .on_leave({
                let browser = self.clone();

                move || browser.clear_drop_target_folder()
            })
            .build()
            .install(widget);
    }

    fn drop_target_folder_at_y(&self, y: f64) -> FileNodePath {
        if let Some(sticky_row) = self
            .tree
            .sticky_row_at_viewport_y(y - self.tree.scroller.vadjustment().value())
            && let BrowserListRowRenderState::Tree { row, .. } = sticky_row.state
        {
            return if row.is_dir {
                row.node_path
            } else {
                row.node_path
                    .parent()
                    .unwrap_or_else(|| self.root_node_path())
            };
        }

        let Some(hit) = self.browser_list_row_at_y(y) else {
            return self.gap_context_folder(y);
        };

        match hit {
            BrowserListRow::Tree(row) if row.is_dir => row.node_path,
            BrowserListRow::Tree(row) => row
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path()),
            BrowserListRow::NewEntry(row) => row.folder,
            BrowserListRow::RenameEntry(row) => row
                .row
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path()),
            BrowserListRow::Loading(row) => row.folder,
            BrowserListRow::Search(search_match) => search_match
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path()),
            BrowserListRow::Status(_) => self.gap_context_folder(y),
            BrowserListRow::RootGap => self.root_node_path(),
        }
    }

    pub(super) fn browser_list_row_at_y(&self, y: f64) -> Option<BrowserListRow> {
        self.tree
            .row_at_content_y(y)
            .map(|row| row.state.to_row())
            .filter(|row| !matches!(row, BrowserListRow::RootGap))
    }

    pub(super) fn folder_for_gap_at_y(&self, y: f64) -> Option<FileNodePath> {
        if matches!(
            self.tree.row_at_content_y(y).map(|row| row.state.to_row()),
            Some(BrowserListRow::RootGap)
        ) {
            return Some(self.root_node_path());
        }

        self.tree
            .last_row_before_content_y_matching(y, |row| {
                matches!(row.state, BrowserListRowRenderState::Tree { .. })
            })
            .and_then(|row| match row.state.to_row() {
                BrowserListRow::Tree(row) => Some(if row.is_dir && self.tree_row_expanded(&row) {
                    row.node_path
                } else {
                    row.node_path
                        .parent()
                        .unwrap_or_else(|| self.root_node_path())
                }),
                _ => None,
            })
    }

    fn browser_tree_row(
        &self,
        row: &BrowserListRow,
        index: usize,
    ) -> tree_view::TreeRow<BrowserListRowKey, BrowserListRowRenderState> {
        let state = self.browser_list_row_render_state(row);
        let (depth, branch, expanded, sticky) = match &state {
            BrowserListRowRenderState::Tree { row, expanded, .. } => (
                row.depth,
                row.tree_role == TreeRowRole::Branch,
                *expanded,
                row.tree_role == TreeRowRole::Branch,
            ),
            BrowserListRowRenderState::NewEntry(row) => (row.depth, false, false, false),
            BrowserListRowRenderState::RenameEntry { row, .. } => {
                (row.row.depth, false, false, false)
            }
            BrowserListRowRenderState::Loading(row) => (row.depth, false, false, false),
            BrowserListRowRenderState::Search { search_match, .. } => {
                (search_match.depth, false, false, false)
            }
            BrowserListRowRenderState::Status(_) | BrowserListRowRenderState::RootGap => {
                (0, false, false, false)
            }
        };

        tree_view::TreeRow {
            key: browser_list_row_key(row, index),
            depth,
            height: row.height(),
            branch,
            expanded,
            sticky,
            state,
        }
    }

    fn browser_list_row_render_state(&self, row: &BrowserListRow) -> BrowserListRowRenderState {
        let selected = self.selected_node_path.borrow().clone();
        let selected_search_match = self.selected_search_match.borrow().clone();
        let drop_target = self.current_drop_target_folder();

        match row {
            BrowserListRow::Tree(row) => BrowserListRowRenderState::Tree {
                row: row.clone(),
                expanded: self.tree_row_expanded(row),
                selected: selected_search_match.is_none()
                    && selected.as_ref() == Some(&row.node_path),
                drop_target: drop_target.as_ref() == Some(&row.node_path),
                status: self.changed_file_statuses.borrow().get(&row.path).cloned(),
                transfer_progress: self.transfer_progress_for_path(&row.node_path),
            },
            BrowserListRow::NewEntry(row) => BrowserListRowRenderState::NewEntry(row.clone()),
            BrowserListRow::RenameEntry(row) => BrowserListRowRenderState::RenameEntry {
                row: row.clone(),
                expanded: self.tree_row_expanded(&row.row),
            },
            BrowserListRow::Loading(row) => BrowserListRowRenderState::Loading(row.clone()),
            BrowserListRow::Search(search_match) => BrowserListRowRenderState::Search {
                search_match: search_match.clone(),
                selected: selected_search_match.as_ref() == Some(&search_match.selection_key()),
            },
            BrowserListRow::Status(message) => BrowserListRowRenderState::Status(message.clone()),
            BrowserListRow::RootGap => BrowserListRowRenderState::RootGap,
        }
    }

    fn tree_row_expanded(&self, row: &BrowserRow) -> bool {
        if row.tree_role != TreeRowRole::Branch {
            return false;
        }

        if !self.search_query.borrow().is_empty() {
            !self.search_collapsed_dirs.borrow().contains(&row.node_path)
        } else {
            self.expanded_dirs.borrow().contains(&row.node_path)
        }
    }
}

fn sync_spellcheck_entry_warning(
    entry: &gtk::Entry,
    name: &str,
    allowlist: &crate::spellcheck::SpellcheckAllowlist,
) {
    let issues = crate::spellcheck::check_filename(name, allowlist);
    if let Some(issue) = issues.first() {
        entry.set_icon_from_icon_name(
            gtk::EntryIconPosition::Secondary,
            Some("dialog-warning-symbolic"),
        );
        entry.set_icon_tooltip_text(
            gtk::EntryIconPosition::Secondary,
            Some(&format!("Possible typo: {}", issue.word)),
        );
    } else {
        entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, None);
        entry.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, None);
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum BrowserListRowKey {
    Tree {
        path: FileNodePath,
        is_dir: bool,
        tree_role: TreeRowRole,
    },
    NewEntry {
        folder: FileNodePath,
        kind: super::NewEntryKind,
    },
    RenameEntry {
        path: FileNodePath,
    },
    Loading {
        folder: FileNodePath,
    },
    Search {
        path: FileNodePath,
        line_number: u64,
        start: usize,
        end: usize,
    },
    Status {
        index: usize,
        message: String,
    },
    RootGap,
}

#[derive(Clone, PartialEq)]
pub(super) enum BrowserListRowRenderState {
    Tree {
        row: BrowserRow,
        expanded: bool,
        selected: bool,
        drop_target: bool,
        status: Option<String>,
        transfer_progress: Option<TransferRowProgress>,
    },
    NewEntry(NewEntryRow),
    RenameEntry {
        row: RenameEntryRow,
        expanded: bool,
    },
    Loading(LoadingRow),
    Search {
        search_match: SearchMatch,
        selected: bool,
    },
    Status(String),
    RootGap,
}

impl BrowserListRowRenderState {
    pub(super) fn to_row(&self) -> BrowserListRow {
        match self {
            Self::Tree { row, .. } => BrowserListRow::Tree(row.clone()),
            Self::NewEntry(row) => BrowserListRow::NewEntry(row.clone()),
            Self::RenameEntry { row, .. } => BrowserListRow::RenameEntry(row.clone()),
            Self::Loading(row) => BrowserListRow::Loading(row.clone()),
            Self::Search { search_match, .. } => BrowserListRow::Search(search_match.clone()),
            Self::Status(message) => BrowserListRow::Status(message.clone()),
            Self::RootGap => BrowserListRow::RootGap,
        }
    }
}

pub(super) fn browser_list_row_key(row: &BrowserListRow, index: usize) -> BrowserListRowKey {
    match row {
        BrowserListRow::Tree(row) => tree_row_key(row),
        BrowserListRow::NewEntry(row) => BrowserListRowKey::NewEntry {
            folder: row.folder.clone(),
            kind: row.kind,
        },
        BrowserListRow::RenameEntry(row) => rename_row_key(&row.row.node_path),
        BrowserListRow::Loading(row) => BrowserListRowKey::Loading {
            folder: row.folder.clone(),
        },
        BrowserListRow::Search(search_match) => BrowserListRowKey::Search {
            path: search_match.node_path.clone(),
            line_number: search_match.line_number,
            start: search_match.start,
            end: search_match.end,
        },
        BrowserListRow::Status(message) => BrowserListRowKey::Status {
            index,
            message: message.clone(),
        },
        BrowserListRow::RootGap => BrowserListRowKey::RootGap,
    }
}

pub(super) fn tree_row_key(row: &BrowserRow) -> BrowserListRowKey {
    BrowserListRowKey::Tree {
        path: row.node_path.clone(),
        is_dir: row.is_dir,
        tree_role: row.tree_role,
    }
}

fn rename_row_key(path: &FileNodePath) -> BrowserListRowKey {
    BrowserListRowKey::RenameEntry { path: path.clone() }
}

fn root_gap_row_widget() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .height_request(ROOT_GAP_HEIGHT as i32)
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .build()
}

fn row_end_padding() -> i32 {
    canvas_scrollbar::WIDTH.ceil() as i32 + 4
}

fn loading_disclosure_spinner() -> adw::Spinner {
    let spinner = adw::Spinner::builder()
        .width_request(tree_view::ICON_SIZE)
        .height_request(tree_view::ICON_SIZE)
        .valign(gtk::Align::Center)
        .build();
    spinner.set_can_target(false);
    spinner.set_sensitive(false);
    spinner.set_tooltip_text(Some("Loading"));
    spinner
}

fn tree_primary_click_handler(
    browser: &Rc<FileBrowser>,
    row: BrowserRow,
    sticky: bool,
) -> impl Fn(&gtk::Button, &gtk::GestureClick, f64, f64) + 'static {
    let browser = browser.clone();

    move |_, _, _, _| {
        browser.queue_cancel_pending_new_entry();
        if !browser.search_query.borrow().is_empty() && row.tree_role == TreeRowRole::Branch {
            browser.set_selected_node_path(Some(row.node_path.clone()));
            browser.active_folder.replace(row.node_path.clone());
            browser.toggle_dir(&row.node_path);
            return;
        }

        browser.set_selected_node_path(Some(row.node_path.clone()));
        if !row.is_dir {
            browser.active_folder.replace(
                row.node_path
                    .parent()
                    .unwrap_or_else(|| browser.root_node_path()),
            );
        } else {
            browser.active_folder.replace(row.node_path.clone());
        }
        if row.tree_role == TreeRowRole::Branch {
            let key = tree_row_key(&row);
            if sticky
                && browser.search_query.borrow().is_empty()
                && !browser.tree.row_is_below_sticky(&key)
            {
                browser.tree.scroll_row_below_sticky(&key);
                return;
            }
            browser.toggle_dir(&row.node_path);
        }
    }
}

fn tree_secondary_click_handler(
    browser: &Rc<FileBrowser>,
    row: BrowserRow,
) -> impl Fn(&gtk::Button, &gtk::GestureClick, f64, f64) + 'static {
    let browser = browser.clone();

    move |parent, gesture, x, y| {
        browser.queue_cancel_pending_new_entry();
        browser.set_selected_node_path(Some(row.node_path.clone()));
        browser.active_folder.replace(if row.is_dir {
            row.node_path.clone()
        } else {
            row.node_path
                .parent()
                .unwrap_or_else(|| browser.root_node_path())
        });
        show_row_context_menu(&browser, parent, BrowserTarget::from_row(&row), x, y);
        gesture.set_state(gtk::EventSequenceState::Claimed);
    }
}

fn file_drag_source(browser: &Rc<FileBrowser>, path: FileNodePath) -> tree_view::DragSource {
    tree_view::DragSource::builder()
        .prepare({
            let browser = browser.clone();
            let path = path.clone();

            move || {
                let Ok(info) = browser.file_access.borrow().info(&path) else {
                    return None;
                };
                if !info.capabilities.readable {
                    return None;
                }

                let bytes = gtk::glib::Bytes::from_owned(path.display().into_bytes());
                Some(gdk::ContentProvider::for_bytes("text/plain", &bytes))
            }
        })
        .on_begin({
            let browser = browser.clone();
            let path = path.clone();

            move || browser.set_internal_drag_paths(vec![path.clone()])
        })
        .on_cancel({
            let browser = browser.clone();

            move || {
                browser.clear_internal_drag_paths();
                false
            }
        })
        .on_end({
            let browser = browser.clone();

            move || browser.clear_internal_drag_paths()
        })
        .build()
}

fn replace_row_widget(widget: &gtk::Widget, next: gtk::Widget) {
    let Some(parent) = widget.parent() else {
        return;
    };
    if let Ok(list) = parent.clone().downcast::<gtk::Box>() {
        list.insert_child_after(&next, Some(widget));
        list.remove(widget);
    } else if let Ok(layer) = parent.downcast::<gtk::Fixed>() {
        layer.put(&next, 0.0, 0.0);
        layer.remove(widget);
    }
}

fn sync_transfer_progress(
    browser: &Rc<FileBrowser>,
    widget: &gtk::Widget,
    row_path: &FileNodePath,
    progress: Option<&TransferRowProgress>,
) {
    let callback = progress.map(|_| {
        let browser = browser.clone();
        let row_path = row_path.clone();
        Rc::new(move || {
            if let Some(progress) = browser.transfer_progress_for_path(&row_path) {
                browser.confirm_cancel_transfers(progress.transfer_ids);
            }
        }) as tree_view::IconRowProgressCallback
    });
    let icon_progress = progress.map(icon_row_progress);
    tree_view::sync_icon_row_progress(widget, icon_progress.as_ref(), callback);
}

fn icon_row_progress(progress: &TransferRowProgress) -> tree_view::IconRowProgress {
    tree_view::IconRowProgress {
        fraction: progress.fraction,
        tooltip: progress.tooltip.clone(),
    }
}

fn sync_status_icon(widget: &gtk::Widget, status: Option<&str>) {
    let Some(content) = tree_view::icon_row_content(widget) else {
        return;
    };
    let Some(label) = tree_view::icon_row_title(widget) else {
        return;
    };
    if let Some(existing) = tree_view::icon_row_child_after(&label, STATUS_ICON_CLASS) {
        content.remove(&existing);
    }

    if let Some(status) = status {
        let icon = file_status::icon(status);
        icon.add_css_class(STATUS_ICON_CLASS);
        icon.set_margin_end(STATUS_ICON_END_MARGIN);
        content.insert_child_after(&icon, Some(&label));
    }
}

fn update_search_row_widget(
    widget: &gtk::Widget,
    previous: &SearchMatch,
    previous_selected: bool,
    next: &SearchMatch,
    selected: bool,
) {
    if previous.depth != next.depth {
        tree_view::sync_icon_row_depth(widget, next.depth);
    }
    if previous_selected != selected {
        tree_view::sync_icon_row_selected(widget, selected);
    }
    if previous.text != next.text
        && let Some(preview) = tree_view::icon_row_title(widget)
    {
        preview.set_label(next.text.trim());
    }
}

fn update_new_entry_row_widget(widget: &gtk::Widget, previous: &NewEntryRow, next: &NewEntryRow) {
    if previous.depth != next.depth {
        tree_view::sync_icon_row_depth(widget, next.depth);
    }

    if previous.default_name != next.default_name || previous.kind != next.kind {
        if let Some(icon) = tree_view::icon_row_icon(widget) {
            sync_new_file_icon(&icon, &next.default_name);
        }
        tree_view::sync_icon_row_text(widget, &next.default_name);
    }
}

fn update_rename_entry_row_widget(
    widget: &gtk::Widget,
    previous: &RenameEntryRow,
    previous_expanded: bool,
    next: &RenameEntryRow,
    expanded: bool,
) {
    if previous.row.depth != next.row.depth {
        tree_view::sync_icon_row_depth(widget, next.row.depth);
    }
    if previous.original_name != next.original_name {
        tree_view::sync_icon_row_text(widget, &next.original_name);
    }
    if previous.row.ignore != next.row.ignore {
        if let Some(disclosure) = tree_view::icon_row_disclosure(widget) {
            tree_view::sync_dimmed(&disclosure, next.row.ignore.is_ignored());
        }
        if let Some(icon) = tree_view::icon_row_icon(widget) {
            tree_view::sync_dimmed(&icon, next.row.ignore.is_ignored());
        }
    }
    if previous_expanded != expanded
        && next.row.tree_role == TreeRowRole::Branch
        && let Some(handle) = tree_view::icon_row_disclosure(widget)
    {
        handle.queue_draw();
    }
}

fn update_status_row_widget(widget: &gtk::Widget, previous: &str, next: &str) {
    if previous == next {
        return;
    }
    if let Some(label) = tree_view::icon_row_title(widget) {
        label.set_label(next);
    }
}

fn sync_new_file_icon(icon: &gtk::Widget, name: &str) {
    let Ok(icon) = icon.clone().downcast::<gtk::Image>() else {
        return;
    };
    let detected_type = file_type::detect(name, false);
    let next_icon = file_type::icon(&detected_type);
    if let Some(paintable) = next_icon.paintable() {
        icon.set_paintable(Some(&paintable));
    } else {
        icon.set_icon_name(next_icon.icon_name().as_deref());
    }
    icon.set_pixel_size(tree_view::ICON_SIZE);
}

fn row_file_icon(row: &BrowserRow) -> gtk::Image {
    let icon = if matches!(row.kind, FileNodeKind::Archive { .. }) {
        file_type::icon_for_name("extract-symbolic")
    } else {
        let detected_type = file_type::detect(&row.name, false);
        file_type::icon(&detected_type)
    };
    icon.set_pixel_size(tree_view::ICON_SIZE);
    icon.set_valign(gtk::Align::Center);
    if row.ignore.is_ignored() {
        icon.add_css_class("repo-browser-ignored-content");
    }
    icon
}

fn file_row_icon(name: &str, ignored: bool) -> gtk::Image {
    let detected_type = file_type::detect(name, false);
    let icon = file_type::icon(&detected_type);
    icon.set_pixel_size(tree_view::ICON_SIZE);
    icon.set_valign(gtk::Align::Center);
    if ignored {
        icon.add_css_class("repo-browser-ignored-content");
    }
    icon
}

fn read_only_icon() -> gtk::Image {
    let icon = file_type::icon_for_name("changes-prevent-symbolic");
    icon.set_pixel_size(tree_view::ICON_SIZE);
    icon.set_valign(gtk::Align::Center);
    icon.set_tooltip_text(Some("Read-only"));
    icon
}
