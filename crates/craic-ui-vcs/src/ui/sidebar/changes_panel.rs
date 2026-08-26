use super::super::{canvas_scroll, widgets};
use super::changes::{
    ChangedFileContextCallback, ChangedFileItem, changed_file_factory, default_commit_summary,
    file_signature, set_realized_file_checks, update_commit_button_sensitivity_for_paths,
};
use super::commit_panel::CommitPanel;
use crate::git::RepositorySnapshot;
use crate::ui::components::search::SearchPanel;
use adw::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Clone)]
pub struct ChangesPanel {
    pub root: gtk::Stack,
    pub selection_header: gtk::Box,
    pub initialize_button: gtk::Button,
    model: gio::ListStore,
    filtered_model: gtk::FilterListModel,
    filter: gtk::CustomFilter,
    selection: gtk::SingleSelection,
    files_list: gtk::ListView,
    summary_entry: gtk::Entry,
    generate_button: gtk::Button,
    commit_button: gtk::Button,
    commit_spinner: adw::Spinner,
    commit_running: Rc<Cell<bool>>,
    files_stack: gtk::Stack,
    search_panel: SearchPanel,
    select_all_check: gtk::CheckButton,
    select_all_label: gtk::Label,
    selection_syncing: Rc<Cell<bool>>,
    file_signature: Rc<RefCell<Vec<(String, String)>>>,
    search_query: Rc<RefCell<String>>,
    checked_paths: Rc<RefCell<HashSet<String>>>,
    context_requested: Rc<RefCell<Option<Rc<ChangedFileContextCallback>>>>,
}

impl ChangesPanel {
    pub fn new(commit_panel: &CommitPanel) -> Self {
        let model = gio::ListStore::new::<ChangedFileItem>();
        let search_query = Rc::new(RefCell::new(String::new()));
        let filter = gtk::CustomFilter::new({
            let search_query = search_query.clone();

            move |object| {
                let Some(file) = object.downcast_ref::<ChangedFileItem>() else {
                    return false;
                };
                file.matches_search(&search_query.borrow())
            }
        });
        let filtered_model = gtk::FilterListModel::new(Some(model.clone()), Some(filter.clone()));
        let selection = gtk::SingleSelection::new(Some(filtered_model.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

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
        let file_signature = Rc::new(RefCell::new(Vec::new()));
        let checked_paths = Rc::new(RefCell::new(HashSet::new()));
        let commit_running = Rc::new(Cell::new(false));
        let context_requested = Rc::new(RefCell::new(None));
        let controls_changed: Rc<dyn Fn()> = Rc::new({
            let filtered_model = filtered_model.clone();
            let checked_paths = checked_paths.clone();
            let select_all_check = select_all_check.clone();
            let select_all_label = select_all_label.clone();
            let selection_syncing = selection_syncing.clone();
            let summary_entry = commit_panel.summary_entry.clone();
            let generate_button = commit_panel.generate_button.clone();
            let commit_button = commit_panel.commit_button.clone();
            let file_signature = file_signature.clone();
            let commit_running = commit_running.clone();

            move || {
                refresh_control_widgets(
                    &filtered_model,
                    &checked_paths.borrow(),
                    &select_all_check,
                    &select_all_label,
                    &selection_syncing,
                    &summary_entry,
                    &generate_button,
                    &commit_button,
                    &file_signature.borrow(),
                    commit_running.get(),
                );
            }
        });
        let factory = changed_file_factory(
            &selection,
            checked_paths.clone(),
            selection_syncing.clone(),
            controls_changed,
            context_requested.clone(),
        );
        let files_list = gtk::ListView::new(Some(selection.clone()), Some(factory));
        files_list.add_css_class("navigation-sidebar");

        let search_panel = SearchPanel::new("Search changed files");
        search_panel.set_options_visible(false);
        search_panel.set_navigation_visible(false);

        let files_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
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
        let clean_status_page = adw::StatusPage::builder()
            .icon_name("builder-check-symbolic")
            .title("No local changes")
            .description("Working tree is clean.")
            .hexpand(true)
            .vexpand(true)
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

        let files_stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        files_stack.add_named(&files_box, Some("files"));
        files_stack.add_named(&clean_status_page, Some("clean"));
        files_stack.set_visible_child_name("files");

        let content = gtk::Paned::new(gtk::Orientation::Vertical);
        content.set_vexpand(true);
        content.set_start_child(Some(&files_stack));
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
            selection_header,
            initialize_button,
            model,
            filtered_model,
            filter,
            selection,
            files_list,
            summary_entry: commit_panel.summary_entry.clone(),
            generate_button: commit_panel.generate_button.clone(),
            commit_button: commit_panel.commit_button.clone(),
            commit_spinner: commit_panel.commit_spinner.clone(),
            commit_running,
            files_stack,
            search_panel,
            select_all_check,
            select_all_label,
            selection_syncing,
            file_signature,
            search_query,
            checked_paths,
            context_requested,
        };
        panel.search_panel.set_key_capture_widget(&panel.root);
        panel.search_panel.install_shortcuts(&panel.root);
        panel.connect_search();
        panel.connect_summary_changed();
        panel.connect_select_all();
        panel
    }

    pub fn update(&self, snapshot: &RepositorySnapshot) {
        if snapshot.changed_files.is_empty() {
            self.show_clean_repository(snapshot);
            return;
        }

        self.root.set_visible_child_name("content");
        self.files_stack.set_visible_child_name("files");
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
            self.update_checked_paths(snapshot, &previous_paths);

            update_changed_file_model(&self.model, snapshot);
            if let Some(path) = selected {
                self.select_file_path(&path);
            }
        }
        self.refresh_controls();
    }

    fn show_clean_repository(&self, snapshot: &RepositorySnapshot) {
        self.model.remove_all();
        self.selection.unselect_all();
        self.file_signature.borrow_mut().clear();
        self.search_query.borrow_mut().clear();
        self.checked_paths.borrow_mut().clear();
        self.search_panel.set_query("", false);
        self.root.set_visible_child_name("content");
        self.files_stack.set_visible_child_name("clean");
        log::debug!(
            "changes panel showing clean status workspace={} branch={}",
            snapshot.name,
            snapshot.branch
        );
        self.refresh_controls();
    }

    pub fn connect_file_selected<F>(&self, callback: F)
    where
        F: Fn(Option<String>) + 'static,
    {
        self.selection.connect_selected_notify(move |selection| {
            let path = selection
                .selected_item()
                .and_downcast::<ChangedFileItem>()
                .map(|file| file.path());
            callback(path);
        });
    }

    pub fn connect_file_context_requested<F>(&self, callback: F)
    where
        F: Fn(&gtk::Widget, String, f64, f64, u32) + 'static,
    {
        *self.context_requested.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn selected_file_path(&self) -> Option<String> {
        self.selection
            .selected_item()
            .and_downcast::<ChangedFileItem>()
            .map(|file| file.path())
    }

    pub fn checked_file_paths(&self) -> Vec<String> {
        let mut paths = self
            .checked_paths
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }

    pub fn has_changed_files(&self) -> bool {
        self.model.n_items() > 0
    }

    pub fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub fn set_all_checked(&self, active: bool) {
        set_filtered_checked_paths(&self.filtered_model, &self.checked_paths, active);
        self.selection_syncing.set(true);
        set_realized_file_checks(&self.files_list, active);
        self.selection_syncing.set(false);
        self.refresh_controls();
    }

    pub fn clear_selection(&self) {
        self.selection.unselect_all();
    }

    pub fn commit_summary(&self) -> String {
        let summary = self.summary_entry.text().trim().to_string();
        if !summary.is_empty() {
            return summary;
        }

        default_commit_summary(&self.checked_file_paths(), &self.file_signature.borrow())
            .unwrap_or_default()
    }

    pub fn begin_commit(&self) -> bool {
        if self.commit_running.replace(true) {
            return false;
        }
        self.commit_spinner.set_visible(true);
        self.refresh_controls();
        true
    }

    pub fn finish_commit(&self) {
        self.commit_running.set(false);
        self.commit_spinner.set_visible(false);
        self.refresh_controls();
    }

    pub fn clear(&self) {
        self.root.set_visible_child_name("content");
        self.files_stack.set_visible_child_name("files");
        self.model.remove_all();
        self.selection.unselect_all();
        self.file_signature.borrow_mut().clear();
        self.search_query.borrow_mut().clear();
        self.checked_paths.borrow_mut().clear();
        self.search_panel.set_query("", false);
        self.refresh_controls();
    }

    pub fn show_initialize_repository(&self) {
        self.model.remove_all();
        self.selection.unselect_all();
        self.file_signature.borrow_mut().clear();
        self.search_query.borrow_mut().clear();
        self.checked_paths.borrow_mut().clear();
        self.search_panel.set_query("", false);
        self.root.set_visible_child_name("status");
        self.refresh_controls();
    }

    fn connect_search(&self) {
        self.search_panel.connect_query_changed({
            let panel = self.clone();

            move |query| panel.update_search_query(query.trim().to_lowercase())
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
        self.search_query.replace(query.clone());
        log::debug!("changes search updated query_len={}", query.len());
        self.filter.changed(gtk::FilterChange::Different);
        self.refresh_controls();
    }

    fn update_checked_paths(
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

    fn connect_summary_changed(&self) {
        self.summary_entry.connect_changed({
            let commit_button = self.commit_button.clone();
            let file_signature = self.file_signature.clone();
            let checked_paths = self.checked_paths.clone();
            let commit_running = self.commit_running.clone();

            move |entry| {
                update_commit_button_sensitivity_for_paths(
                    &checked_paths.borrow(),
                    entry,
                    &commit_button,
                    &file_signature.borrow(),
                    commit_running.get(),
                );
            }
        });
    }

    fn connect_select_all(&self) {
        self.select_all_check.connect_toggled({
            let filtered_model = self.filtered_model.clone();
            let files_list = self.files_list.clone();
            let checked_paths = self.checked_paths.clone();
            let selection_syncing = self.selection_syncing.clone();
            let select_all_check = self.select_all_check.clone();
            let select_all_label = self.select_all_label.clone();
            let summary_entry = self.summary_entry.clone();
            let generate_button = self.generate_button.clone();
            let commit_button = self.commit_button.clone();
            let file_signature = self.file_signature.clone();
            let commit_running = self.commit_running.clone();

            move |button| {
                if selection_syncing.get() {
                    return;
                }

                set_filtered_checked_paths(&filtered_model, &checked_paths, button.is_active());
                selection_syncing.set(true);
                set_realized_file_checks(&files_list, button.is_active());
                selection_syncing.set(false);
                refresh_control_widgets(
                    &filtered_model,
                    &checked_paths.borrow(),
                    &select_all_check,
                    &select_all_label,
                    &selection_syncing,
                    &summary_entry,
                    &generate_button,
                    &commit_button,
                    &file_signature.borrow(),
                    commit_running.get(),
                );
            }
        });
    }

    fn select_file_path(&self, path: &str) {
        for position in 0..self.filtered_model.n_items() {
            let Some(file) = self
                .filtered_model
                .item(position)
                .and_downcast::<ChangedFileItem>()
            else {
                continue;
            };
            if file.path() == path {
                self.selection.set_selected(position);
                return;
            }
        }
        self.selection.unselect_all();
    }

    fn refresh_controls(&self) {
        refresh_control_widgets(
            &self.filtered_model,
            &self.checked_paths.borrow(),
            &self.select_all_check,
            &self.select_all_label,
            &self.selection_syncing,
            &self.summary_entry,
            &self.generate_button,
            &self.commit_button,
            &self.file_signature.borrow(),
            self.commit_running.get(),
        );
    }
}

fn update_changed_file_model(model: &gio::ListStore, snapshot: &RepositorySnapshot) {
    let old_len = model.n_items() as usize;
    let new_len = snapshot.changed_files.len();
    let shared_len = old_len.min(new_len);
    let mut prefix = 0;
    while prefix < shared_len {
        let Some(existing) = model.item(prefix as u32).and_downcast::<ChangedFileItem>() else {
            break;
        };
        let desired = &snapshot.changed_files[prefix];
        if !existing.matches(&desired.path, &desired.status) {
            break;
        }
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix < shared_len - prefix {
        let Some(existing) = model
            .item((old_len - suffix - 1) as u32)
            .and_downcast::<ChangedFileItem>()
        else {
            break;
        };
        let desired = &snapshot.changed_files[new_len - suffix - 1];
        if !existing.matches(&desired.path, &desired.status) {
            break;
        }
        suffix += 1;
    }

    let additions = snapshot.changed_files[prefix..new_len - suffix]
        .iter()
        .map(|file| ChangedFileItem::new(&file.path, &file.status))
        .collect::<Vec<_>>();
    let removed = old_len - prefix - suffix;
    log::debug!(
        "changes virtual model updated files={} changed_start={} removed={} added={}",
        new_len,
        prefix,
        removed,
        additions.len()
    );
    model.splice(prefix as u32, removed as u32, &additions);
}

fn set_filtered_checked_paths(
    model: &gtk::FilterListModel,
    checked_paths: &RefCell<HashSet<String>>,
    active: bool,
) {
    let mut checked = checked_paths.borrow_mut();
    for position in 0..model.n_items() {
        let Some(file) = model.item(position).and_downcast::<ChangedFileItem>() else {
            continue;
        };
        if active {
            checked.insert(file.path());
        } else {
            checked.remove(&file.path());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh_control_widgets(
    model: &gtk::FilterListModel,
    checked_paths: &HashSet<String>,
    select_all_check: &gtk::CheckButton,
    select_all_label: &gtk::Label,
    selection_syncing: &Cell<bool>,
    summary_entry: &gtk::Entry,
    generate_button: &gtk::Button,
    commit_button: &gtk::Button,
    file_signature: &[(String, String)],
    commit_running: bool,
) {
    let total = model.n_items();
    let checked = (0..total)
        .filter_map(|position| model.item(position).and_downcast::<ChangedFileItem>())
        .filter(|file| file.is_checked(checked_paths))
        .count() as u32;

    selection_syncing.set(true);
    select_all_check.set_sensitive(total > 0);
    select_all_check.set_inconsistent(checked > 0 && checked < total);
    select_all_check.set_active(total > 0 && checked == total);
    selection_syncing.set(false);
    select_all_label.set_label(&match total {
        0 => "0 changed files".to_string(),
        1 => "1 changed file".to_string(),
        count => format!("{count} changed files"),
    });

    update_commit_button_sensitivity_for_paths(
        checked_paths,
        summary_entry,
        commit_button,
        file_signature,
        commit_running,
    );
    generate_button.set_sensitive(!checked_paths.is_empty());
}
