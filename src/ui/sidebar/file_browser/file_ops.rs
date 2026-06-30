use super::{
    BrowserTarget, FileBrowser, SEARCH_POLL_MS, file_name, join_relative, parent_folder, rows,
    should_skip, tree::BrowserRow,
};
use crate::system::capabilities::files::FileKind;
use adw::prelude::*;
use gtk::gio;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

impl FileBrowser {
    pub(super) fn browser_list_rows_with_pending_new_entry(
        &self,
        rows: Vec<BrowserRow>,
    ) -> Vec<rows::BrowserListRow> {
        let pending = self.pending_new_entry.borrow().clone();
        let pending_rename = self.pending_rename_entry.borrow().clone();
        let mut list_rows = rows
            .into_iter()
            .map(|row| {
                if pending_rename
                    .as_ref()
                    .is_some_and(|pending| pending.path == row.path)
                {
                    rows::BrowserListRow::RenameEntry(rows::RenameEntryRow {
                        original_name: row.name.clone(),
                        row,
                    })
                } else {
                    rows::BrowserListRow::Tree(row)
                }
            })
            .collect::<Vec<_>>();

        let Some(pending) = pending else {
            return list_rows;
        };

        let new_entry_row = rows::NewEntryRow {
            folder: pending.folder.clone(),
            default_name: pending.default_name.clone(),
            kind: pending.kind,
            depth: child_depth(&pending.folder),
        };
        let insert_at = pending_new_entry_insert_index(&list_rows, &pending);
        list_rows.insert(insert_at, rows::BrowserListRow::NewEntry(new_entry_row));
        list_rows
    }

    pub(super) fn create_file_in_folder(self: &Rc<Self>, folder: &str) {
        self.start_pending_new_entry(folder, NewEntryKind::File);
    }

    pub(super) fn create_folder_in_folder(self: &Rc<Self>, folder: &str) {
        self.start_pending_new_entry(folder, NewEntryKind::Folder);
    }

    fn start_pending_new_entry(self: &Rc<Self>, folder: &str, kind: NewEntryKind) {
        if !self.search_query.borrow().is_empty() {
            self.search_panel.set_query("", false);
            self.update_search_query(String::new());
        }

        let default_name = match kind {
            NewEntryKind::File => self
                .last_created_file_extension
                .borrow()
                .clone()
                .unwrap_or_default(),
            NewEntryKind::Folder => String::new(),
        };
        self.pending_new_entry.replace(Some(PendingNewEntry {
            folder: folder.to_string(),
            default_name,
            kind,
        }));
        self.pending_rename_entry.borrow_mut().take();
        self.selected_path.replace(String::new());
        self.selected_search_match.borrow_mut().take();
        self.active_folder.replace(folder.to_string());
        if !folder.is_empty() {
            self.expanded_dirs.borrow_mut().insert(folder.to_string());
        }

        self.rebuild();
        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.focus_pending_new_entry();
        });
    }

    pub(super) fn finish_pending_new_entry(
        self: &Rc<Self>,
        folder: &str,
        kind: NewEntryKind,
        name: String,
    ) {
        let Some(pending) = self.pending_new_entry.borrow().clone() else {
            return;
        };
        if pending.folder != folder || pending.kind != kind {
            return;
        }

        self.pending_new_entry.borrow_mut().take();
        match self.create_child(folder, &name, kind) {
            Ok(()) => {
                if kind == NewEntryKind::File {
                    self.remember_created_file_extension(&name);
                }
            }
            Err(err) => {
                self.rebuild();
                self.show_error(kind.error_heading(), &err);
            }
        }
    }

    pub(super) fn cancel_pending_new_entry(self: &Rc<Self>) {
        if self.pending_new_entry.borrow_mut().take().is_some() {
            self.rebuild();
        }
    }

    pub(super) fn unselect_file_browser(self: &Rc<Self>) {
        self.set_selected_path(String::new());
        self.focus_browser_shell();
    }

    pub(super) fn queue_cancel_pending_new_entry(self: &Rc<Self>) {
        if self.pending_new_entry.borrow().is_none() {
            return;
        }

        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.cancel_pending_new_entry();
        });
    }

    fn remember_created_file_extension(&self, name: &str) {
        let Some((_, extension)) = name.rsplit_once('.') else {
            return;
        };
        if extension.is_empty() {
            return;
        }

        self.last_created_file_extension
            .replace(Some(format!(".{extension}")));
    }

    fn create_child(
        self: &Rc<Self>,
        folder: &str,
        name: &str,
        kind: NewEntryKind,
    ) -> Result<(), String> {
        if name.is_empty() {
            return Err("Enter a name.".to_string());
        }
        if name.contains('/') || name.contains('\\') {
            return Err("Names cannot contain path separators.".to_string());
        }
        if should_skip(name) {
            return Err("That name is hidden by the file browser.".to_string());
        }

        let relative = join_relative((!folder.is_empty()).then_some(folder), name);
        let path = self.workspace_path(&relative);
        if self.file_access.borrow().metadata(&path).is_ok() {
            return Err(format!("{name} already exists."));
        }

        log::info!(
            "file browser create start workspace={} path={} kind={kind:?}",
            self.workspace.borrow().display_name,
            path.display()
        );
        let file_access = self.file_access.borrow().clone();
        match kind {
            NewEntryKind::File => file_access.create_file(&path)?,
            NewEntryKind::Folder => file_access.create_dir(&path)?,
        }

        if !folder.is_empty() {
            self.expanded_dirs.borrow_mut().insert(folder.to_string());
        }
        self.invalidate_tree_rows_cache();
        self.spellcheck_allowlist
            .replace(crate::spellcheck::load_manifest_allowlist(
                &self.workspace.borrow(),
                self.file_access.borrow().clone(),
            ));

        match kind {
            NewEntryKind::File => {
                self.active_folder.replace(folder.to_string());
                self.set_selected_path(relative.clone());
            }
            NewEntryKind::Folder => {
                self.expanded_dirs.borrow_mut().insert(relative.clone());
                self.active_folder.replace(relative);
                self.set_selected_path(String::new());
            }
        }

        self.rebuild();
        Ok(())
    }

    pub(super) fn rename_target(self: &Rc<Self>, target: &BrowserTarget) {
        if !self.search_query.borrow().is_empty() {
            self.search_panel.set_query("", false);
            self.update_search_query(String::new());
        }

        self.pending_new_entry.borrow_mut().take();
        self.pending_rename_entry.replace(Some(PendingRenameEntry {
            path: target.path.clone(),
        }));
        self.selected_path.replace(target.path.clone());
        self.selected_search_match.borrow_mut().take();
        let folder = parent_folder(&target.path);
        self.active_folder.replace(folder.clone());
        if !folder.is_empty() {
            self.expanded_dirs.borrow_mut().insert(folder);
        }

        self.rebuild();
        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.focus_pending_rename_entry();
        });
    }

    pub(super) fn finish_pending_rename(self: &Rc<Self>, target: &BrowserTarget, new_name: String) {
        let Some(pending) = self.pending_rename_entry.borrow().clone() else {
            return;
        };
        if pending.path != target.path {
            return;
        }

        self.pending_rename_entry.borrow_mut().take();
        if new_name.is_empty() || new_name == file_name(&target.path) {
            self.rebuild();
            return;
        }
        if let Err(err) = self.rename_entry(target, &new_name) {
            self.rebuild();
            self.show_error("Rename Failed", &err);
        }
    }

    pub(super) fn cancel_pending_rename(self: &Rc<Self>) {
        if self.pending_rename_entry.borrow_mut().take().is_some() {
            self.rebuild();
        }
    }

    pub(super) fn queue_cancel_pending_rename(self: &Rc<Self>) {
        if self.pending_rename_entry.borrow().is_none() {
            return;
        }

        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.cancel_pending_rename();
        });
    }

    fn rename_entry(self: &Rc<Self>, target: &BrowserTarget, new_name: &str) -> Result<(), String> {
        if new_name.contains('/') || new_name.contains('\\') {
            return Err("Names cannot contain path separators.".to_string());
        }

        let parent = target.path.rsplit_once('/').map(|(parent, _)| parent);
        let new_relative = join_relative(parent, new_name);
        let source = self.workspace_path(&target.path);
        let destination = self.workspace_path(&new_relative);
        if self.file_access.borrow().metadata(&destination).is_ok() {
            return Err(format!("{new_name} already exists."));
        }
        log::info!(
            "file browser rename start workspace={} source={} destination={}",
            self.workspace.borrow().display_name,
            source.display(),
            destination.display()
        );
        self.file_access
            .borrow()
            .rename(&source, &destination)
            .map_err(|err| format!("Unable to rename: {err}"))?;

        self.invalidate_tree_rows_cache();
        self.spellcheck_allowlist
            .replace(crate::spellcheck::load_manifest_allowlist(
                &self.workspace.borrow(),
                self.file_access.borrow().clone(),
            ));
        if target.is_dir {
            rename_expanded_dirs(
                &mut self.expanded_dirs.borrow_mut(),
                &target.path,
                &new_relative,
            );
            self.active_folder.replace(new_relative);
            self.set_selected_path(String::new());
        } else {
            self.active_folder.replace(parent_folder(&new_relative));
            self.set_selected_path(new_relative);
        }
        self.rebuild();
        Ok(())
    }

    pub(super) fn delete_selected_file(self: &Rc<Self>) {
        let path = self.selected_path.borrow().clone();
        if path.is_empty() {
            return;
        }
        let workspace_path = self.workspace_path(&path);
        let is_dir = match self.file_access.borrow().metadata(&workspace_path) {
            Ok(metadata) => metadata.kind == FileKind::Directory,
            Err(err) => {
                self.show_error("Delete Failed", &format!("Unable to inspect {path}: {err}"));
                return;
            }
        };
        self.delete_target(BrowserTarget {
            path,
            is_dir,
            executable: false,
        });
    }

    pub(super) fn delete_target(self: &Rc<Self>, target: BrowserTarget) {
        let body = if target.is_dir {
            format!(
                "Delete the folder \"{}\" and everything inside it?",
                target.path
            )
        } else {
            format!("Delete the file \"{}\"?", target.path)
        };
        let dialog = adw::AlertDialog::builder()
            .heading("Confirm Delete")
            .body(&body)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        dialog.choose(Some(&self.root), None::<&gio::Cancellable>, {
            let browser = self.clone();

            move |response| {
                if response.as_str() == "delete" {
                    browser.delete_confirmed(target);
                }
            }
        });
    }

    fn delete_confirmed(self: &Rc<Self>, target: BrowserTarget) {
        let path = self.workspace_path(&target.path);
        let file_access = self.file_access.borrow().clone();
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            log::info!("file browser delete start path={}", path.display());
            let result = file_access.delete(&path);
            let _ = sender.send(result);
        });

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();

            move || match receiver.try_recv() {
                Ok(Ok(())) => {
                    if target_affects_selection(browser.selected_path.borrow().as_str(), &target) {
                        browser.set_selected_path(String::new());
                    }
                    browser.active_folder.replace(parent_folder(&target.path));
                    remove_expanded_dir(&mut browser.expanded_dirs.borrow_mut(), &target.path);
                    browser.invalidate_tree_rows_cache();
                    browser.rebuild();
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(message)) => {
                    browser.invalidate_tree_rows_cache();
                    browser.rebuild_if_changed();
                    browser.show_error("Delete Failed", &message);
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    browser.invalidate_tree_rows_cache();
                    browser.rebuild_if_changed();
                    browser
                        .show_error("Delete Failed", "Delete operation did not return a result.");
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    }
}

#[derive(Clone)]
pub(super) struct PendingNewEntry {
    folder: String,
    default_name: String,
    kind: NewEntryKind,
}

#[derive(Clone)]
pub(super) struct PendingRenameEntry {
    path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::ui) enum NewEntryKind {
    File,
    Folder,
}

impl NewEntryKind {
    fn error_heading(self) -> &'static str {
        match self {
            Self::File => "Create File Failed",
            Self::Folder => "Create Folder Failed",
        }
    }
}

fn target_affects_selection(selected: &str, target: &BrowserTarget) -> bool {
    if selected == target.path {
        return true;
    }
    target.is_dir
        && selected
            .strip_prefix(&target.path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn rename_expanded_dirs(expanded_dirs: &mut HashSet<String>, old_path: &str, new_path: &str) {
    let renamed = expanded_dirs
        .iter()
        .filter_map(|path| {
            if path == old_path {
                Some(new_path.to_string())
            } else {
                path.strip_prefix(old_path)
                    .filter(|suffix| suffix.starts_with('/'))
                    .map(|suffix| format!("{new_path}{suffix}"))
            }
        })
        .collect::<Vec<_>>();
    remove_expanded_dir(expanded_dirs, old_path);
    expanded_dirs.extend(renamed);
}

fn remove_expanded_dir(expanded_dirs: &mut HashSet<String>, path: &str) {
    expanded_dirs.retain(|expanded| {
        expanded != path
            && !expanded
                .strip_prefix(path)
                .is_some_and(|suffix| suffix.starts_with('/'))
    });
}

fn child_depth(folder: &str) -> usize {
    folder
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn pending_new_entry_insert_index(
    rows: &[rows::BrowserListRow],
    pending: &PendingNewEntry,
) -> usize {
    let child_depth = child_depth(&pending.folder);
    let (mut index, boundary_depth) = if pending.folder.is_empty() {
        (0, None)
    } else {
        let Some(parent_index) = rows.iter().position(|row| {
            matches!(
                row,
                rows::BrowserListRow::Tree(row) if row.is_dir && row.path == pending.folder
            )
        }) else {
            return rows.len();
        };
        (parent_index + 1, child_depth.checked_sub(1))
    };

    while index < rows.len() {
        let rows::BrowserListRow::Tree(row) = &rows[index] else {
            break;
        };
        if boundary_depth.is_some_and(|depth| row.depth <= depth) {
            break;
        }
        if row.depth < child_depth {
            break;
        }
        if row.depth > child_depth {
            index += 1;
            continue;
        }
        if !row.is_dir {
            break;
        }
        index += 1;
        while index < rows.len() {
            let rows::BrowserListRow::Tree(descendant) = &rows[index] else {
                break;
            };
            if descendant.depth <= row.depth {
                break;
            }
            index += 1;
        }
    }

    index
}
