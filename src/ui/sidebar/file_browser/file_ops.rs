use super::{
    BrowserTarget, FileBrowser, SEARCH_POLL_MS, file_name, rows, should_skip, tree::BrowserRow,
};
use crate::system::FileNodePath;
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
                    .is_some_and(|pending| pending.path == row.node_path)
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
        self.insert_root_loading_row(&mut list_rows);

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

    fn insert_root_loading_row(&self, list_rows: &mut Vec<rows::BrowserListRow>) {
        let root = self.root_node_path();
        if self.tree_directory_loading.borrow().contains(&root) {
            list_rows.insert(
                0,
                rows::BrowserListRow::Loading(rows::LoadingRow {
                    folder: root,
                    depth: 0,
                }),
            );
        }
    }

    pub(super) fn create_file_in_folder(self: &Rc<Self>, folder: &FileNodePath) {
        self.start_pending_new_entry(folder, NewEntryKind::File);
    }

    pub(super) fn create_folder_in_folder(self: &Rc<Self>, folder: &FileNodePath) {
        self.start_pending_new_entry(folder, NewEntryKind::Folder);
    }

    fn start_pending_new_entry(self: &Rc<Self>, folder: &FileNodePath, kind: NewEntryKind) {
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
            folder: folder.clone(),
            default_name,
            kind,
        }));
        self.pending_rename_entry.borrow_mut().take();
        self.selected_node_path.replace(None);
        self.selected_search_match.borrow_mut().take();
        self.active_folder.replace(folder.clone());
        if !folder.is_root() {
            self.expanded_dirs.borrow_mut().insert(folder.clone());
        }

        self.rebuild();
        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.focus_pending_new_entry();
        });
    }

    pub(super) fn finish_pending_new_entry(
        self: &Rc<Self>,
        folder: &FileNodePath,
        kind: NewEntryKind,
        name: String,
    ) {
        let Some(pending) = self.pending_new_entry.borrow().clone() else {
            return;
        };
        if pending.folder != *folder || pending.kind != kind {
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
        self.set_selected_node_path(None);
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
        folder: &FileNodePath,
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

        let candidate = folder.join_child(name);
        if self.file_access.borrow().info(&candidate).is_ok() {
            return Err(format!("{name} already exists."));
        }

        log::info!(
            "file browser create start workspace={} parent={} name={} kind={kind:?}",
            self.workspace.borrow().display_name,
            folder.display(),
            name
        );
        let file_access = self.file_access.borrow().clone();
        let created = match kind {
            NewEntryKind::File => file_access.create_file(folder, name)?,
            NewEntryKind::Folder => file_access.create_dir(folder, name)?,
        };

        if !folder.is_root() {
            self.expanded_dirs.borrow_mut().insert(folder.clone());
        }
        self.invalidate_tree_rows_cache();
        self.spellcheck_allowlist
            .replace(crate::spellcheck::load_manifest_allowlist(
                &self.workspace.borrow(),
                self.file_access.borrow().clone(),
            ));

        match kind {
            NewEntryKind::File => {
                self.active_folder.replace(folder.clone());
                self.set_selected_node_path(Some(created));
            }
            NewEntryKind::Folder => {
                self.expanded_dirs.borrow_mut().insert(created.clone());
                self.active_folder.replace(created.clone());
                self.set_selected_node_path(Some(created));
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
            path: target.node_path.clone(),
        }));
        self.selected_node_path
            .replace(Some(target.node_path.clone()));
        self.selected_search_match.borrow_mut().take();
        let folder = target
            .node_path
            .parent()
            .unwrap_or_else(|| self.root_node_path());
        self.active_folder.replace(folder.clone());
        if !folder.is_root() {
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
        if pending.path != target.node_path {
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

        let parent = target
            .node_path
            .parent()
            .unwrap_or_else(|| self.root_node_path());
        let destination = parent.join_child(new_name);
        if self.file_access.borrow().info(&destination).is_ok() {
            return Err(format!("{new_name} already exists."));
        }
        log::info!(
            "file browser rename start workspace={} source={} destination_parent={} new_name={}",
            self.workspace.borrow().display_name,
            target.node_path.display(),
            parent.display(),
            new_name
        );
        let renamed = self
            .file_access
            .borrow()
            .rename(&target.node_path, &parent, new_name)
            .map_err(|err| format!("Unable to rename: {err}"))?;

        self.invalidate_tree_rows_cache();
        self.spellcheck_allowlist
            .replace(crate::spellcheck::load_manifest_allowlist(
                &self.workspace.borrow(),
                self.file_access.borrow().clone(),
            ));
        if target.is_dir {
            {
                let mut expanded = self.expanded_dirs.borrow_mut();
                rename_expanded_dirs(&mut expanded, &target.node_path, &renamed);
            }
            self.active_folder.replace(renamed.clone());
            self.set_selected_node_path(Some(renamed));
        } else {
            self.active_folder.replace(parent);
            self.set_selected_node_path(Some(renamed));
        }
        self.rebuild();
        Ok(())
    }

    pub(super) fn delete_selected_file(self: &Rc<Self>) {
        let Some(path) = self.selected_node_path.borrow().clone() else {
            return;
        };
        let target = match self.file_access.borrow().info(&path) {
            Ok(info) => BrowserTarget::from_info(info),
            Err(err) => {
                self.show_error(
                    "Delete Failed",
                    &format!("Unable to inspect {}: {err}", path.display()),
                );
                return;
            }
        };
        self.delete_target(target);
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
        let path = target.node_path.clone();
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
                    let selection_affected = {
                        let selected = browser.selected_node_path.borrow();
                        target_affects_selection(selected.as_ref(), &target)
                    };
                    if selection_affected {
                        browser.set_selected_node_path(None);
                    }
                    let parent = target
                        .node_path
                        .parent()
                        .unwrap_or_else(|| browser.root_node_path());
                    browser.active_folder.replace(parent);
                    remove_expanded_dir(&mut browser.expanded_dirs.borrow_mut(), &target.node_path);
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
    folder: FileNodePath,
    default_name: String,
    kind: NewEntryKind,
}

#[derive(Clone)]
pub(super) struct PendingRenameEntry {
    path: FileNodePath,
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

fn target_affects_selection(selected: Option<&FileNodePath>, target: &BrowserTarget) -> bool {
    if selected == Some(&target.node_path) {
        return true;
    }
    target
        .is_dir
        .then_some(())
        .is_some_and(|_| selected.is_some_and(|path| path.is_child_of(&target.node_path)))
}

fn rename_expanded_dirs(
    expanded_dirs: &mut HashSet<FileNodePath>,
    old_path: &FileNodePath,
    new_path: &FileNodePath,
) {
    let renamed = expanded_dirs
        .iter()
        .filter_map(|path| {
            if path == old_path {
                Some(new_path.clone())
            } else if path.is_child_of(old_path) {
                let mut renamed = new_path.clone();
                renamed
                    .nodes
                    .extend(path.nodes[old_path.nodes.len()..].iter().cloned());
                Some(renamed)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    remove_expanded_dir(expanded_dirs, old_path);
    expanded_dirs.extend(renamed);
}

fn remove_expanded_dir(expanded_dirs: &mut HashSet<FileNodePath>, path: &FileNodePath) {
    expanded_dirs.retain(|expanded| expanded != path && !expanded.is_child_of(path));
}

fn child_depth(folder: &FileNodePath) -> usize {
    let display = folder.display();
    if display.is_empty() {
        0
    } else {
        display
            .split('/')
            .filter(|segment| !segment.is_empty())
            .count()
    }
}

fn pending_new_entry_insert_index(
    rows: &[rows::BrowserListRow],
    pending: &PendingNewEntry,
) -> usize {
    let child_depth = child_depth(&pending.folder);
    let (mut index, boundary_depth) = if pending.folder.is_root() {
        (0, None)
    } else {
        let Some(parent_index) = rows.iter().position(|row| {
            matches!(
                row,
                rows::BrowserListRow::Tree(row) if row.is_dir && row.node_path == pending.folder
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
