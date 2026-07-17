use super::{
    BrowserTarget, FileBrowser, SEARCH_POLL_MS, file_name, rows, should_skip, tree::BrowserRow,
    tree_loader::sort_browser_rows,
};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{
    FileDeleteRequest, FileMoveRequest, FileOperationEvent, FileWriteMode, FileWritePayload,
    FileWriteRequest,
};
use adw::prelude::*;
use gtk::gio;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

const DELETE_WATCH_SUPPRESSION_MS: u64 = 1_200;

impl FileBrowser {
    pub fn browser_list_rows_with_pending_new_entry(
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
        self.insert_directory_loading_rows(&mut list_rows);

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

    fn insert_directory_loading_rows(&self, list_rows: &mut Vec<rows::BrowserListRow>) {
        let loading = self.tree_directory_loading.borrow().clone();
        if loading.is_empty() {
            return;
        }

        let root = self.root_node_path();
        if loading.contains(&root) {
            list_rows.insert(
                0,
                rows::BrowserListRow::Loading(rows::LoadingRow {
                    folder: root,
                    depth: 0,
                }),
            );
        }

        let mut index = 0usize;
        while index < list_rows.len() {
            let Some((folder, depth)) = loading_row_after(&list_rows[index], &loading) else {
                index += 1;
                continue;
            };
            list_rows.insert(
                index + 1,
                rows::BrowserListRow::Loading(rows::LoadingRow { folder, depth }),
            );
            index += 2;
        }
    }

    pub fn create_file_in_folder(self: &Rc<Self>, folder: &FileNodePath) {
        self.start_pending_new_entry(folder, NewEntryKind::File);
    }

    pub fn create_folder_in_folder(self: &Rc<Self>, folder: &FileNodePath) {
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

    pub fn finish_pending_new_entry(
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
            Ok(()) => {}
            Err(err) => {
                self.rebuild();
                self.show_error(kind.error_heading(), &err);
            }
        }
    }

    pub fn cancel_pending_new_entry(self: &Rc<Self>) {
        if self.pending_new_entry.borrow_mut().take().is_some() {
            self.rebuild();
        }
    }

    pub fn unselect_file_browser(self: &Rc<Self>) {
        self.set_selected_node_path(None);
        self.focus_browser_shell();
    }

    pub fn queue_cancel_pending_new_entry(self: &Rc<Self>) {
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
        let created = candidate.clone();
        let payload = match kind {
            NewEntryKind::File => FileWritePayload::File(Vec::new()),
            NewEntryKind::Folder => FileWritePayload::Directory,
        };
        let (sender, receiver) = mpsc::channel();
        file_access.write_node(
            FileWriteRequest {
                path: created.clone(),
                mode: FileWriteMode::CreateNew,
                payload,
                cancel_requested: None,
            },
            Box::new(move |event| {
                if let FileOperationEvent::Finished(result) = event {
                    let _ = sender.send(result);
                }
            }),
        );

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();
            let folder = folder.clone();
            let name = name.to_string();

            move || match receiver.try_recv() {
                Ok(Ok(())) => {
                    browser.finish_successful_create(&folder, &name, kind, created.clone());
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    browser.rebuild();
                    browser.show_error(kind.error_heading(), &err.to_string());
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    browser.rebuild();
                    browser.show_error(
                        kind.error_heading(),
                        "Create operation did not return a result.",
                    );
                    gtk::glib::ControlFlow::Break
                }
            }
        });
        self.refresh_browser_row_state();
        Ok(())
    }

    fn finish_successful_create(
        self: &Rc<Self>,
        folder: &FileNodePath,
        name: &str,
        kind: NewEntryKind,
        created: FileNodePath,
    ) {
        if kind == NewEntryKind::File {
            self.remember_created_file_extension(name);
        }
        if !folder.is_root() {
            self.expanded_dirs.borrow_mut().insert(folder.clone());
        }
        self.insert_created_target_into_browser_state(folder, &created);
        self.spellcheck_allowlist
            .replace(crate::spellcheck::SpellcheckAllowlist::default());

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

        self.rebuild_if_changed();
    }

    fn insert_created_target_into_browser_state(
        &self,
        folder: &FileNodePath,
        created: &FileNodePath,
    ) {
        let info = match self.file_access.borrow().info(created) {
            Ok(info) => info,
            Err(err) => {
                log::debug!(
                    "file browser create cache update skipped path={} err={err}",
                    created.display()
                );
                self.tree_directory_cache.borrow_mut().remove(folder);
                self.tree_rows_cache.borrow_mut().take();
                self.rows_signature.borrow_mut().clear();
                return;
            }
        };

        if let Some(rows) = self.tree_directory_cache.borrow_mut().get_mut(folder) {
            let row = BrowserRow::from_info(info, child_depth(folder));
            rows.retain(|existing| existing.node_path != row.node_path);
            rows.push(row);
            sort_browser_rows(rows);
        }
        self.tree_rows_cache.borrow_mut().take();
        self.rows_signature.borrow_mut().clear();
    }

    pub fn rename_target(self: &Rc<Self>, target: &BrowserTarget) {
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

    pub fn finish_pending_rename(self: &Rc<Self>, target: &BrowserTarget, new_name: String) {
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

    pub fn cancel_pending_rename(self: &Rc<Self>) {
        if self.pending_rename_entry.borrow_mut().take().is_some() {
            self.rebuild();
        }
    }

    pub fn queue_cancel_pending_rename(self: &Rc<Self>) {
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
        let file_access = self.file_access.borrow().clone();
        let (sender, receiver) = mpsc::channel();
        file_access.move_node(
            FileMoveRequest {
                source: target.node_path.clone(),
                destination_parent: parent.clone(),
                new_name: new_name.to_string(),
                cancel_requested: None,
            },
            Box::new(move |event| {
                if let FileOperationEvent::Finished(result) = event {
                    let _ = sender.send(result);
                }
            }),
        );

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();
            let target = target.clone();

            move || match receiver.try_recv() {
                Ok(Ok(renamed)) => {
                    browser.finish_successful_rename(&target, parent.clone(), renamed);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    browser.rebuild();
                    browser.show_error("Rename Failed", &format!("Unable to rename: {err}"));
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    browser.rebuild();
                    browser
                        .show_error("Rename Failed", "Rename operation did not return a result.");
                    gtk::glib::ControlFlow::Break
                }
            }
        });
        self.rebuild();
        Ok(())
    }

    fn finish_successful_rename(
        self: &Rc<Self>,
        target: &BrowserTarget,
        parent: FileNodePath,
        renamed: FileNodePath,
    ) {
        self.invalidate_tree_rows_cache();
        self.spellcheck_allowlist
            .replace(crate::spellcheck::SpellcheckAllowlist::default());
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
    }

    pub fn delete_selected_file(self: &Rc<Self>) {
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

    pub fn delete_target(self: &Rc<Self>, target: BrowserTarget) {
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
        if !self.start_pending_delete(&path) {
            return;
        }

        let file_access = self.file_access.borrow().clone();
        let (sender, receiver) = mpsc::channel();
        file_access.delete(
            FileDeleteRequest {
                path: path.clone(),
                cancel_requested: None,
            },
            Box::new(move |result| {
                if let FileOperationEvent::Finished(result) = result {
                    let _ = sender.send(result);
                }
            }),
        );

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();

            move || match receiver.try_recv() {
                Ok(Ok(())) => {
                    browser.finish_successful_delete(&target);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(message)) => {
                    browser.finish_failed_delete(&target.node_path);
                    browser.show_error("Delete Failed", &message.to_string());
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    browser.finish_failed_delete(&target.node_path);
                    browser
                        .show_error("Delete Failed", "Delete operation did not return a result.");
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    }

    fn start_pending_delete(self: &Rc<Self>, path: &FileNodePath) -> bool {
        {
            let mut deleting = self.deleting_paths.borrow_mut();
            if !deleting.insert(path.clone()) {
                log::debug!(
                    "file browser delete ignored duplicate path={}",
                    path.display()
                );
                return false;
            }
        }

        self.pending_new_entry.borrow_mut().take();
        self.pending_rename_entry.borrow_mut().take();
        self.refresh_browser_row_state();
        true
    }

    fn finish_successful_delete(self: &Rc<Self>, target: &BrowserTarget) {
        self.deleting_paths.borrow_mut().remove(&target.node_path);
        self.suppress_delete_watch_events(target.node_path.clone());
        self.remove_deleted_target_from_browser_state(target);

        if self.search_query.borrow().is_empty() {
            self.rebuild_if_changed();
        } else {
            self.remove_deleted_target_from_search_cache(target);
            self.rebuild_search_result_rows_from_cache();
        }
    }

    fn finish_failed_delete(self: &Rc<Self>, path: &FileNodePath) {
        self.deleting_paths.borrow_mut().remove(path);
        self.delete_watch_suppression_paths
            .borrow_mut()
            .remove(path);
        self.rows_signature.borrow_mut().clear();
        if self.search_query.borrow().is_empty() {
            self.rebuild();
        } else {
            self.rebuild_search_result_rows_from_cache();
        }
    }

    fn suppress_delete_watch_events(self: &Rc<Self>, path: FileNodePath) {
        self.delete_watch_suppression_paths
            .borrow_mut()
            .insert(path.clone());
        log::debug!(
            "file browser delete watch suppression start path={}",
            path.display()
        );

        let browser = self.clone();
        gtk::glib::timeout_add_local_once(
            Duration::from_millis(DELETE_WATCH_SUPPRESSION_MS),
            move || {
                let removed = browser
                    .delete_watch_suppression_paths
                    .borrow_mut()
                    .remove(&path);
                if removed {
                    log::debug!(
                        "file browser delete watch suppression end path={}",
                        path.display()
                    );
                }
            },
        );
    }

    fn remove_deleted_target_from_browser_state(&self, target: &BrowserTarget) {
        let parent = target
            .node_path
            .parent()
            .unwrap_or_else(|| self.root_node_path());

        {
            let mut cache = self.tree_directory_cache.borrow_mut();
            if let Some(rows) = cache.get_mut(&parent) {
                rows.retain(|row| !target_affects_path(&row.node_path, target));
            }
            if target.is_dir {
                cache.retain(|path, _| !target_affects_path(path, target));
            }
        }

        if target.is_dir {
            remove_expanded_dir(&mut self.expanded_dirs.borrow_mut(), &target.node_path);
        }
        if active_folder_affected(&self.active_folder.borrow(), target) {
            self.active_folder.replace(parent);
        }
        if target_affects_selection(self.selected_node_path.borrow().as_ref(), target) {
            self.selected_node_path.borrow_mut().take();
            self.selected_search_match.borrow_mut().take();
        }

        self.tree_rows_cache.borrow_mut().take();
        self.rows_signature.borrow_mut().clear();
    }
}

#[derive(Clone)]
pub struct PendingNewEntry {
    folder: FileNodePath,
    default_name: String,
    kind: NewEntryKind,
}

#[derive(Clone)]
pub struct PendingRenameEntry {
    path: FileNodePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NewEntryKind {
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

fn target_affects_path(path: &FileNodePath, target: &BrowserTarget) -> bool {
    path == &target.node_path || (target.is_dir && path.is_child_of(&target.node_path))
}

fn active_folder_affected(active_folder: &FileNodePath, target: &BrowserTarget) -> bool {
    target.is_dir
        && (active_folder == &target.node_path || active_folder.is_child_of(&target.node_path))
}

fn loading_row_after(
    row: &rows::BrowserListRow,
    loading: &HashSet<FileNodePath>,
) -> Option<(FileNodePath, usize)> {
    match row {
        rows::BrowserListRow::Tree(row)
            if row.tree_role == super::tree::TreeRowRole::Branch
                && loading.contains(&row.node_path) =>
        {
            Some((row.node_path.clone(), row.depth + 1))
        }
        _ => None,
    }
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
