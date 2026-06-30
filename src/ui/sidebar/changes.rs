use super::super::file_row;
use crate::git::RepositorySnapshot;
use adw::prelude::*;
use craic_diff_ui::{Element, PartialEqRenderState};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ChangedFileRowRenderState {
    status: String,
}

pub(super) type ChangedFilesReconciler =
    craic_diff_ui::gtk::ListBoxReconciler<String, ChangedFileRowRenderState>;

pub(super) fn file_signature(snapshot: &RepositorySnapshot) -> Vec<(String, String)> {
    snapshot
        .changed_files
        .iter()
        .map(|file| (file.path.clone(), file.status.clone()))
        .collect()
}

pub(super) fn reconcile_changed_files(
    list: &gtk::ListBox,
    reconciler: &mut ChangedFilesReconciler,
    snapshot: &RepositorySnapshot,
    selected: Option<&str>,
    summary_entry: &gtk::Entry,
    generate_button: &gtk::Button,
    commit_button: &gtk::Button,
    select_all_check: &gtk::CheckButton,
    select_all_label: &gtk::Label,
    selection_syncing: &Rc<Cell<bool>>,
    file_signature: Rc<RefCell<Vec<(String, String)>>>,
    checked_paths: Rc<RefCell<HashSet<String>>>,
) {
    let elements = snapshot
        .changed_files
        .iter()
        .map(|file| {
            Element::new(
                file.path.clone(),
                ChangedFileRowRenderState {
                    status: file.status.clone(),
                },
            )
        })
        .collect::<Vec<_>>();

    reconciler.reconcile(
        list,
        elements,
        PartialEqRenderState,
        {
            let list = list.clone();
            let summary_entry = summary_entry.clone();
            let generate_button = generate_button.clone();
            let commit_button = commit_button.clone();
            let select_all_check = select_all_check.clone();
            let select_all_label = select_all_label.clone();
            let selection_syncing = selection_syncing.clone();

            move |_, path, state| {
                let row = file_row::changed_file_row(path, &state.status, true);
                if let Some(check_button) = row_check_button(&row) {
                    check_button.set_active(checked_paths.borrow().contains(path));
                    check_button.connect_toggled({
                        let list = list.clone();
                        let summary_entry = summary_entry.clone();
                        let generate_button = generate_button.clone();
                        let commit_button = commit_button.clone();
                        let select_all_check = select_all_check.clone();
                        let select_all_label = select_all_label.clone();
                        let selection_syncing = selection_syncing.clone();
                        let file_signature = file_signature.clone();
                        let checked_paths = checked_paths.clone();
                        let path = path.clone();

                        move |button| {
                            if button.is_active() {
                                checked_paths.borrow_mut().insert(path.clone());
                            } else {
                                checked_paths.borrow_mut().remove(&path);
                            }
                            update_commit_button_sensitivity_for_paths(
                                &checked_paths.borrow(),
                                &summary_entry,
                                &commit_button,
                                &file_signature.borrow(),
                            );
                            generate_button.set_sensitive(!checked_paths.borrow().is_empty());
                            update_selection_header(
                                &list,
                                &select_all_check,
                                &select_all_label,
                                &selection_syncing,
                            );
                        }
                    });
                }
                row.upcast::<gtk::Widget>()
            }
        },
        move |_, widget, previous, next| {
            if previous.status == next.status {
                return;
            }

            if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
                file_row::update_changed_file_row_status(&row, &next.status);
            }
        },
    );

    if let Some(selected) = selected {
        match row_for_path(list, selected) {
            Some(row) => list.select_row(Some(&row)),
            None => list.unselect_all(),
        }
    }

    update_selection_header(list, select_all_check, select_all_label, selection_syncing);
}

pub(super) fn clear_changed_files(list: &gtk::ListBox, reconciler: &mut ChangedFilesReconciler) {
    reconciler.reconcile(
        list,
        Vec::<Element<String, ChangedFileRowRenderState>>::new(),
        PartialEqRenderState,
        |_, path, state| {
            file_row::changed_file_row(path, &state.status, true).upcast::<gtk::Widget>()
        },
        |_, _, _, _| {},
    );
}

pub(super) fn checked_file_paths(list: &gtk::ListBox) -> Vec<String> {
    let mut paths = Vec::new();
    let mut child = list.first_child();

    while let Some(widget) = child {
        let next = widget.next_sibling();

        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if row_check_button(&row).is_some_and(|button| button.is_active()) {
                let path = row.widget_name();
                if !path.is_empty() {
                    paths.push(path.to_string());
                }
            }
        }
        child = next;
    }
    paths
}

pub(super) fn update_commit_button_sensitivity_for_paths(
    files: &HashSet<String>,
    summary_entry: &gtk::Entry,
    commit_button: &gtk::Button,
    file_signature: &[(String, String)],
) {
    let mut files = files.iter().cloned().collect::<Vec<_>>();
    files.sort();
    let default_summary = default_commit_summary(&files, file_signature);
    let has_summary = !summary_entry.text().trim().is_empty() || default_summary.is_some();
    let has_checked_file = !files.is_empty();
    summary_entry.set_placeholder_text(Some(
        default_summary.as_deref().unwrap_or("Summary (required)"),
    ));
    commit_button.set_sensitive(has_summary && has_checked_file);
}

pub(super) fn default_commit_summary(
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

pub(super) fn update_selection_header(
    list: &gtk::ListBox,
    select_all_check: &gtk::CheckButton,
    select_all_label: &gtk::Label,
    selection_syncing: &Rc<Cell<bool>>,
) {
    let mut total = 0;
    let mut checked = 0;
    let mut child = list.first_child();

    while let Some(widget) = child {
        let next = widget.next_sibling();

        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if !row.widget_name().is_empty() {
                total += 1;
                if row_check_button(&row).is_some_and(|button| button.is_active()) {
                    checked += 1;
                }
            }
        }

        child = next;
    }

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
}

pub(super) fn set_all_file_checks(list: &gtk::ListBox, active: bool) {
    let mut child = list.first_child();

    while let Some(widget) = child {
        let next = widget.next_sibling();

        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if let Some(button) = row_check_button(&row) {
                button.set_active(active);
            }
        }

        child = next;
    }
}

pub(super) fn install_empty_list_unselect(list: &gtk::ListBox) {
    let click = gtk::GestureClick::new();
    click.connect_pressed({
        let list = list.clone();

        move |_, _, _, y| {
            if list.row_at_y(y as i32).is_none() {
                list.unselect_all();
            }
        }
    });
    list.add_controller(click);
}

pub(super) fn install_empty_scroller_unselect(scroller: &gtk::ScrolledWindow, list: &gtk::ListBox) {
    let click = gtk::GestureClick::new();
    click.connect_pressed({
        let scroller = scroller.clone();
        let list = list.clone();

        move |_, _, _, y| {
            let list_y = y + scroller.vadjustment().value();
            if list.row_at_y(list_y as i32).is_none() {
                list.unselect_all();
            }
        }
    });
    scroller.add_controller(click);
}

fn row_for_path(list: &gtk::ListBox, path: &str) -> Option<gtk::ListBoxRow> {
    let mut child = list.first_child();

    while let Some(widget) = child {
        let next = widget.next_sibling();

        if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
            if row.widget_name() == path {
                return Some(row);
            }
        }

        child = next;
    }

    None
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

fn row_check_button(row: &gtk::ListBoxRow) -> Option<gtk::CheckButton> {
    find_check_button(&row.child()?)
}

fn find_check_button(widget: &gtk::Widget) -> Option<gtk::CheckButton> {
    if let Ok(button) = widget.clone().downcast::<gtk::CheckButton>() {
        return Some(button);
    }

    let mut child = widget.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Some(button) = find_check_button(&widget) {
            return Some(button);
        }
        child = next;
    }

    None
}
