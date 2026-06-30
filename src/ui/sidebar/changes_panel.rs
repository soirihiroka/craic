use super::super::{canvas_scroll, widgets};
use super::changes::{
    ChangedFilesReconciler, checked_file_paths, clear_changed_files, default_commit_summary,
    file_signature, install_empty_list_unselect, install_empty_scroller_unselect,
    reconcile_changed_files, set_all_file_checks, update_commit_button_sensitivity_for_paths,
    update_selection_header,
};
use super::commit_panel::CommitPanel;
use crate::git::RepositorySnapshot;
use crate::ui::components::search::SearchPanel;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Clone)]
pub(in crate::ui) struct ChangesPanel {
    pub(in crate::ui) root: gtk::Stack,
    pub(in crate::ui) files_list: gtk::ListBox,
    pub(in crate::ui) selection_header: gtk::Box,
    pub(in crate::ui) initialize_button: gtk::Button,
    summary_entry: gtk::Entry,
    generate_button: gtk::Button,
    commit_button: gtk::Button,
    search_panel: SearchPanel,
    select_all_check: gtk::CheckButton,
    select_all_label: gtk::Label,
    selection_syncing: Rc<Cell<bool>>,
    file_signature: Rc<RefCell<Vec<(String, String)>>>,
    latest_snapshot: Rc<RefCell<Option<RepositorySnapshot>>>,
    search_query: Rc<RefCell<String>>,
    checked_paths: Rc<RefCell<HashSet<String>>>,
    file_reconciler: Rc<RefCell<ChangedFilesReconciler>>,
}

impl ChangesPanel {
    pub(in crate::ui) fn new(commit_panel: &CommitPanel) -> Self {
        let files_list = gtk::ListBox::new();
        files_list.set_selection_mode(gtk::SelectionMode::Single);
        files_list.add_css_class("navigation-sidebar");
        install_empty_list_unselect(&files_list);

        let select_all_check = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .tooltip_text("Select all changed files")
            .build();
        let select_all_label = widgets::muted("0 changed files");
        select_all_label.set_hexpand(true);
        select_all_label.set_xalign(0.0);

        let selection_header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(10)
            .margin_end(10)
            .margin_bottom(6)
            .build();
        selection_header.append(&select_all_check);
        selection_header.append(&select_all_label);

        let selection_syncing = Rc::new(Cell::new(false));
        let search_panel = SearchPanel::new("Search changed files");
        search_panel.set_options_visible(false);
        search_panel.set_navigation_visible(false);

        let files_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .vexpand(true)
            .child(&files_list)
            .build();
        let files_autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let files_scroller_overlay = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        files_scroller_overlay.set_child(Some(&files_scroller));
        files_scroller_overlay.add_overlay(&files_autoscroll_marker);
        canvas_scroll::install_scrolled_window_middle_autoscroll(
            &files_scroller,
            &files_autoscroll_marker,
            canvas_scroll::AutoscrollAxes::Vertical,
            "changes_list",
        );
        install_empty_scroller_unselect(&files_scroller, &files_list);

        let initialize_button = gtk::Button::builder()
            .label("Initialize Git Repository")
            .halign(gtk::Align::Center)
            .build();
        initialize_button.add_css_class("suggested-action");

        let status_page = adw::StatusPage::builder()
            .icon_name("branch-fork-symbolic")
            .title("Repository not initialized")
            .description("Initialize Git to track changes in this workspace.")
            .hexpand(true)
            .vexpand(true)
            .child(&initialize_button)
            .build();

        let files_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        files_content.append(&selection_header);
        files_content.append(&files_scroller_overlay);

        let files_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        files_box.append(&search_panel.widget());
        files_box.append(&files_content);

        let content = gtk::Paned::new(gtk::Orientation::Vertical);
        content.set_vexpand(true);
        content.set_start_child(Some(&files_box));
        content.set_end_child(Some(&commit_panel.root));
        content.set_resize_start_child(true);
        content.set_shrink_start_child(false);
        content.set_resize_end_child(false);
        content.set_shrink_end_child(false);
        content.set_position(9999);

        let root = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        root.add_named(&content, Some("content"));
        root.add_named(&status_page, Some("status"));
        root.set_visible_child_name("content");

        let panel = Self {
            root,
            files_list,
            selection_header,
            initialize_button,
            summary_entry: commit_panel.summary_entry.clone(),
            generate_button: commit_panel.generate_button.clone(),
            commit_button: commit_panel.commit_button.clone(),
            search_panel,
            select_all_check,
            select_all_label,
            selection_syncing,
            file_signature: Rc::new(RefCell::new(Vec::new())),
            latest_snapshot: Rc::new(RefCell::new(None)),
            search_query: Rc::new(RefCell::new(String::new())),
            checked_paths: Rc::new(RefCell::new(HashSet::new())),
            file_reconciler: Rc::new(RefCell::new(ChangedFilesReconciler::new())),
        };
        panel.search_panel.set_key_capture_widget(&panel.root);
        panel.search_panel.install_shortcuts(&panel.root);
        panel.connect_search();
        panel.connect_summary_changed();
        panel.connect_select_all();
        panel
    }

    pub(in crate::ui) fn update(&self, snapshot: &RepositorySnapshot) {
        self.root.set_visible_child_name("content");
        let signature = file_signature(snapshot);
        if *self.file_signature.borrow() != signature {
            let selected = self.selected_file_path();
            let previous_paths = self
                .file_signature
                .borrow()
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<HashSet<_>>();
            *self.file_signature.borrow_mut() = signature;
            self.latest_snapshot.replace(Some(snapshot.clone()));
            self.reconcile_checked_paths(snapshot, &previous_paths);

            self.render_snapshot(snapshot, selected.as_deref());
        } else {
            self.latest_snapshot.replace(Some(snapshot.clone()));
        }
        self.refresh_controls();
    }

    pub(in crate::ui) fn selected_file_path(&self) -> Option<String> {
        self.files_list
            .selected_row()
            .map(|row| row.widget_name().to_string())
            .filter(|path| !path.is_empty())
    }

    pub(in crate::ui) fn checked_file_paths(&self) -> Vec<String> {
        self.sync_checked_paths_from_visible_rows();
        let mut paths = self
            .checked_paths
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    pub(in crate::ui) fn has_changed_files(&self) -> bool {
        let mut child = self.files_list.first_child();

        while let Some(widget) = child {
            let next = widget.next_sibling();
            if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                if !row.widget_name().is_empty() {
                    return true;
                }
            }
            child = next;
        }

        false
    }

    pub(in crate::ui) fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub(in crate::ui) fn set_all_checked(&self, active: bool) {
        set_all_file_checks(&self.files_list, active);
        self.sync_checked_paths_from_visible_rows();
        self.refresh_controls();
    }

    pub(in crate::ui) fn commit_summary(&self) -> String {
        let summary = self.summary_entry.text().trim().to_string();
        if !summary.is_empty() {
            return summary;
        }

        default_commit_summary(&self.checked_file_paths(), &self.file_signature.borrow())
            .unwrap_or_default()
    }

    pub(in crate::ui) fn clear(&self) {
        self.root.set_visible_child_name("content");
        let mut reconciler = self.file_reconciler.borrow_mut();
        clear_changed_files(&self.files_list, &mut reconciler);
        self.file_signature.borrow_mut().clear();
        self.latest_snapshot.borrow_mut().take();
        self.search_query.borrow_mut().clear();
        self.checked_paths.borrow_mut().clear();
        self.search_panel.set_query("", false);
        self.refresh_controls();
    }

    pub(in crate::ui) fn show_initialize_repository(&self) {
        let mut reconciler = self.file_reconciler.borrow_mut();
        clear_changed_files(&self.files_list, &mut reconciler);
        self.file_signature.borrow_mut().clear();
        self.latest_snapshot.borrow_mut().take();
        self.search_query.borrow_mut().clear();
        self.checked_paths.borrow_mut().clear();
        self.search_panel.set_query("", false);
        self.root.set_visible_child_name("status");
        self.refresh_controls();
    }

    fn connect_search(&self) {
        self.search_panel.connect_query_changed({
            let panel = self.clone();

            move |query| panel.update_search_query(query.trim().to_string())
        });
        self.search_panel.connect_closed({
            let panel = self.clone();

            move || panel.update_search_query(String::new())
        });
    }

    fn update_search_query(&self, query: String) {
        if *self.search_query.borrow() == query {
            return;
        }
        self.sync_checked_paths_from_visible_rows();
        self.search_query.replace(query.clone());
        log::debug!("changes search updated query_len={}", query.len());
        if let Some(snapshot) = self.latest_snapshot.borrow().clone() {
            self.render_snapshot(&snapshot, self.selected_file_path().as_deref());
        }
        self.refresh_controls();
    }

    fn render_snapshot(&self, snapshot: &RepositorySnapshot, selected: Option<&str>) {
        let filtered = filtered_snapshot(snapshot, &self.search_query.borrow());
        let mut reconciler = self.file_reconciler.borrow_mut();
        reconcile_changed_files(
            &self.files_list,
            &mut reconciler,
            &filtered,
            selected,
            &self.summary_entry,
            &self.generate_button,
            &self.commit_button,
            &self.select_all_check,
            &self.select_all_label,
            &self.selection_syncing,
            self.file_signature.clone(),
            self.checked_paths.clone(),
        );
    }

    fn reconcile_checked_paths(
        &self,
        snapshot: &RepositorySnapshot,
        previous_paths: &HashSet<String>,
    ) {
        let paths = snapshot
            .changed_files
            .iter()
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();
        let mut checked = self.checked_paths.borrow_mut();
        checked.retain(|path| paths.contains(path));
        for path in paths {
            if !previous_paths.contains(&path) {
                checked.insert(path);
            }
        }
    }

    fn sync_checked_paths_from_visible_rows(&self) {
        let visible_paths = visible_file_paths(&self.files_list);
        let checked_visible = checked_file_paths(&self.files_list)
            .into_iter()
            .collect::<HashSet<_>>();
        let mut checked = self.checked_paths.borrow_mut();
        for path in visible_paths {
            if checked_visible.contains(&path) {
                checked.insert(path);
            } else {
                checked.remove(&path);
            }
        }
    }

    fn connect_summary_changed(&self) {
        self.summary_entry.connect_changed({
            let commit_button = self.commit_button.clone();
            let file_signature = self.file_signature.clone();
            let checked_paths = self.checked_paths.clone();

            move |entry| {
                update_commit_button_sensitivity_for_paths(
                    &checked_paths.borrow(),
                    entry,
                    &commit_button,
                    &file_signature.borrow(),
                );
            }
        });
    }

    fn connect_select_all(&self) {
        self.select_all_check.connect_toggled({
            let files_list = self.files_list.clone();
            let summary_entry = self.summary_entry.clone();
            let generate_button = self.generate_button.clone();
            let commit_button = self.commit_button.clone();
            let select_all_check = self.select_all_check.clone();
            let select_all_label = self.select_all_label.clone();
            let selection_syncing = self.selection_syncing.clone();
            let file_signature = self.file_signature.clone();
            let checked_paths = self.checked_paths.clone();

            move |button| {
                if selection_syncing.get() {
                    return;
                }

                set_all_file_checks(&files_list, button.is_active());
                let visible_paths = visible_file_paths(&files_list);
                {
                    let mut checked = checked_paths.borrow_mut();
                    for path in visible_paths {
                        if button.is_active() {
                            checked.insert(path);
                        } else {
                            checked.remove(&path);
                        }
                    }
                }
                update_commit_button_sensitivity_for_paths(
                    &checked_paths.borrow(),
                    &summary_entry,
                    &commit_button,
                    &file_signature.borrow(),
                );
                generate_button.set_sensitive(!checked_paths.borrow().is_empty());
                update_selection_header(
                    &files_list,
                    &select_all_check,
                    &select_all_label,
                    &selection_syncing,
                );
            }
        });
    }

    fn refresh_controls(&self) {
        self.sync_checked_paths_from_visible_rows();
        update_selection_header(
            &self.files_list,
            &self.select_all_check,
            &self.select_all_label,
            &self.selection_syncing,
        );
        update_commit_button_sensitivity_for_paths(
            &self.checked_paths.borrow(),
            &self.summary_entry,
            &self.commit_button,
            &self.file_signature.borrow(),
        );
        self.generate_button
            .set_sensitive(!self.checked_paths.borrow().is_empty());
    }
}

fn filtered_snapshot(snapshot: &RepositorySnapshot, query: &str) -> RepositorySnapshot {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return snapshot.clone();
    }

    let mut filtered = snapshot.clone();
    filtered.changed_files = snapshot
        .changed_files
        .iter()
        .filter(|file| changed_file_matches(file, &query))
        .cloned()
        .collect();
    filtered
}

fn changed_file_matches(file: &crate::git::ChangedFile, query: &str) -> bool {
    file.path.to_lowercase().contains(query)
        || file_name(&file.path).to_lowercase().contains(query)
        || file.status.to_lowercase().contains(query)
}

fn visible_file_paths(list: &gtk::ListBox) -> Vec<String> {
    let mut paths = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            let path = row.widget_name();
            if !path.is_empty() {
                paths.push(path.to_string());
            }
        }
        child = next;
    }
    paths
}

fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}
