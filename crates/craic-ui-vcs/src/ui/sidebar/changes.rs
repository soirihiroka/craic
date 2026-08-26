use super::super::widgets;
use crate::git::RepositorySnapshot;
use adw::prelude::*;
use craic_ui_core::ui::file_status;
use gtk::glib;
use gtk::glib::subclass::prelude::ObjectSubclassIsExt;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

const CHANGED_FILE_ROW_DATA_KEY: &str = "craic-changed-file-row";

pub type ChangedFileContextCallback = dyn Fn(&gtk::Widget, String, f64, f64, u32) + 'static;

pub fn file_signature(snapshot: &RepositorySnapshot) -> Vec<(String, String)> {
    snapshot
        .changed_files
        .iter()
        .map(|file| (file.path.clone(), file.status.clone()))
        .collect()
}

pub fn changed_file_factory(
    selection: &gtk::SingleSelection,
    checked_paths: Rc<RefCell<HashSet<String>>>,
    checks_syncing: Rc<Cell<bool>>,
    controls_changed: Rc<dyn Fn()>,
    context_requested: Rc<RefCell<Option<Rc<ChangedFileContextCallback>>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup({
        let selection = selection.clone();
        let checked_paths = checked_paths.clone();

        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = changed_file_row();

            row.check.connect_toggled({
                let item = item.clone();
                let row = row.clone();
                let checked_paths = checked_paths.clone();
                let checks_syncing = checks_syncing.clone();
                let controls_changed = controls_changed.clone();

                move |button| {
                    if row.binding.get() || checks_syncing.get() {
                        return;
                    }
                    let Some(file) = item.item().and_downcast::<ChangedFileItem>() else {
                        return;
                    };
                    let path = file.path();
                    if button.is_active() {
                        checked_paths.borrow_mut().insert(path);
                    } else {
                        checked_paths.borrow_mut().remove(&path);
                    }
                    controls_changed();
                }
            });

            let click = gtk::GestureClick::builder().button(3).build();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed({
                let item = item.clone();
                let root = row.root.clone();
                let selection = selection.clone();
                let context_requested = context_requested.clone();

                move |gesture, _, x, y| {
                    let Some(file) = item.item().and_downcast::<ChangedFileItem>() else {
                        return;
                    };
                    if item.position() != gtk::INVALID_LIST_POSITION {
                        selection.set_selected(item.position());
                    }
                    if let Some(callback) = context_requested.borrow().clone() {
                        callback(
                            root.upcast_ref(),
                            file.path(),
                            x,
                            y,
                            gesture.current_event_time(),
                        );
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            });
            row.root.add_controller(click);

            item.set_child(Some(&row.root));
            unsafe {
                item.set_data(CHANGED_FILE_ROW_DATA_KEY, row);
            }
        }
    });
    factory.connect_bind({
        let checked_paths = checked_paths.clone();

        move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(file) = item.item().and_downcast::<ChangedFileItem>() else {
                return;
            };
            let Some(row) = changed_file_row_from_item(item) else {
                return;
            };

            let path = file.path();
            row.binding.set(true);
            row.title.set_label(&path);
            row.check.set_active(checked_paths.borrow().contains(&path));
            while let Some(child) = row.status.first_child() {
                row.status.remove(&child);
            }
            row.status.append(&file_status::icon(&file.status()));
            row.binding.set(false);
        }
    });
    factory
}

#[derive(Clone)]
struct ChangedFileRow {
    root: gtk::Box,
    check: gtk::CheckButton,
    title: gtk::Label,
    status: gtk::Box,
    binding: Rc<Cell<bool>>,
}

fn changed_file_row_from_item(item: &gtk::ListItem) -> Option<ChangedFileRow> {
    let row = unsafe { item.data::<ChangedFileRow>(CHANGED_FILE_ROW_DATA_KEY) }?;
    Some(unsafe { row.as_ref().clone() })
}

fn changed_file_row() -> ChangedFileRow {
    let check = gtk::CheckButton::builder()
        .valign(gtk::Align::Center)
        .build();
    let title = widgets::heading("");
    title.set_wrap(false);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_width_chars(1);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(2)
        .margin_end(6)
        .build();
    root.append(&check);
    root.append(&title);
    root.append(&status);

    ChangedFileRow {
        root,
        check,
        title,
        status,
        binding: Rc::new(Cell::new(false)),
    }
}

glib::wrapper! {
    pub struct ChangedFileItem(ObjectSubclass<changed_file_item::ChangedFileItem>);
}

impl ChangedFileItem {
    pub fn new(path: &str, status: &str) -> Self {
        let item: Self = glib::Object::builder().build();
        *item.imp().path.borrow_mut() = path.to_string();
        *item.imp().status.borrow_mut() = status.to_string();
        item
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }

    pub fn status(&self) -> String {
        self.imp().status.borrow().clone()
    }

    pub fn matches(&self, path: &str, status: &str) -> bool {
        self.imp().path.borrow().as_str() == path && self.imp().status.borrow().as_str() == status
    }

    pub fn matches_search(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let path = self.imp().path.borrow();
        path.to_lowercase().contains(query)
            || file_name(&path).to_lowercase().contains(query)
            || self.imp().status.borrow().to_lowercase().contains(query)
    }

    pub fn is_checked(&self, checked_paths: &HashSet<String>) -> bool {
        checked_paths.contains(self.imp().path.borrow().as_str())
    }
}

mod changed_file_item {
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct ChangedFileItem {
        pub path: RefCell<String>,
        pub status: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChangedFileItem {
        const NAME: &'static str = "CraicChangedFileItem";
        type Type = super::ChangedFileItem;
    }

    impl ObjectImpl for ChangedFileItem {}
}

pub fn set_realized_file_checks(view: &gtk::ListView, active: bool) {
    set_descendant_checks(view.upcast_ref(), active);
}

fn set_descendant_checks(widget: &gtk::Widget, active: bool) {
    if let Some(button) = widget.downcast_ref::<gtk::CheckButton>() {
        button.set_active(active);
        return;
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        set_descendant_checks(&widget, active);
        child = next;
    }
}

pub fn update_commit_button_sensitivity_for_paths(
    files: &HashSet<String>,
    summary_entry: &gtk::Entry,
    commit_button: &gtk::Button,
    file_signature: &[(String, String)],
    commit_running: bool,
) {
    let default_summary = if files.len() <= 2 {
        let mut files = files.iter().cloned().collect::<Vec<_>>();
        files.sort();
        default_commit_summary(&files, file_signature)
    } else {
        None
    };
    let has_summary = !summary_entry.text().trim().is_empty() || default_summary.is_some();
    let has_checked_file = !files.is_empty();
    summary_entry.set_placeholder_text(Some(
        default_summary.as_deref().unwrap_or("Summary (required)"),
    ));
    commit_button.set_sensitive(!commit_running && has_summary && has_checked_file);
}

pub fn default_commit_summary(
    files: &[String],
    file_signature: &[(String, String)],
) -> Option<String> {
    match files {
        [file] => Some(format!(
            "{} {}",
            action_for(status_for(file, file_signature)),
            file_name(file)
        )),
        [first, second] => Some(format!(
            "{} {} and {} {}",
            action_for(status_for(first, file_signature)),
            file_name(first),
            action_for(status_for(second, file_signature)).to_lowercase(),
            file_name(second)
        )),
        _ => None,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

fn status_for<'a>(path: &str, file_signature: &'a [(String, String)]) -> &'a str {
    file_signature
        .iter()
        .find(|(file_path, _)| file_path == path)
        .map(|(_, status)| status.as_str())
        .unwrap_or_default()
}

fn action_for(status: &str) -> &'static str {
    if status.contains('D') {
        "Delete"
    } else if status == "M-" {
        "Clean up"
    } else if status.contains('A') || status.contains('?') {
        "Create"
    } else {
        "Update"
    }
}
