use super::{
    FileBrowser, MAX_TREE_ROWS, should_skip,
    tree::{BrowserRow, RowIgnoreDisplay},
};
use crate::system::capabilities::files::{DirectoryEntry, DirectoryListing, FileKind};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

impl FileBrowser {
    pub(super) fn visible_rows(self: &Rc<Self>) -> Vec<BrowserRow> {
        let workspace = self.workspace.borrow().clone();
        let file_access = self.file_access.borrow().clone();
        let tree_signature = tree_rows_cache_signature(&workspace, &self.expanded_dirs.borrow());
        let mut rows = if let Some(cache) = self.tree_rows_cache.borrow().as_ref()
            && cache.signature == tree_signature
        {
            cache.rows.clone()
        } else {
            self.load_open_directory_rows(&workspace, &file_access, &self.expanded_dirs.borrow());
            let mut rows = Vec::new();
            self.collect_rows("", &self.expanded_dirs.borrow(), &mut rows);
            self.tree_rows_cache.replace(Some(TreeRowsCache {
                signature: tree_signature,
                rows: rows.clone(),
            }));
            rows
        };
        self.refresh_git_ignore_cache_for_rules(&rows);
        self.apply_git_ignore_cache(&mut rows);
        self.queue_git_ignore_query(&rows);
        rows
    }

    pub(super) fn invalidate_tree_rows_cache(&self) {
        self.tree_rows_cache.borrow_mut().take();
        self.tree_directory_cache.borrow_mut().clear();
        self.rows_signature.borrow_mut().clear();
    }

    pub(super) fn invalidate_tree_directory_cache_for_changed_files(
        &self,
        changed_files: &[crate::git::ChangedFile],
    ) {
        if changed_files.is_empty() {
            return;
        }

        let mut cache = self.tree_directory_cache.borrow_mut();
        for changed in changed_files {
            let parent = super::parent_folder(&changed.path);
            cache.remove(&parent);
        }
        self.tree_rows_cache.borrow_mut().take();
        self.rows_signature.borrow_mut().clear();
    }

    fn load_open_directory_rows(
        &self,
        workspace: &crate::system::WorkspaceRef,
        file_access: &std::sync::Arc<dyn crate::system::capabilities::files::FileAccess>,
        expanded_dirs: &HashSet<String>,
    ) {
        let open_dirs = open_directory_relatives(expanded_dirs);
        let missing_dirs = {
            let cache = self.tree_directory_cache.borrow();
            open_dirs
                .into_iter()
                .filter(|relative| !cache.contains_key(relative))
                .collect::<Vec<_>>()
        };
        if missing_dirs.is_empty() {
            return;
        }

        let paths = missing_dirs
            .iter()
            .map(|relative| workspace.path(relative))
            .collect::<Vec<_>>();
        log::trace!(
            "file browser list directories count={} workspace={}",
            paths.len(),
            workspace.display_name
        );
        let listings = match file_access.list_dirs(&paths) {
            Ok(listings) => listings,
            Err(err) => {
                log::debug!(
                    "file browser list directories failed workspace={} dir_count={} err={err}",
                    workspace.display_name,
                    paths.len()
                );
                Vec::new()
            }
        };
        self.cache_directory_listings(listings);
    }

    fn cache_directory_listings(&self, listings: Vec<DirectoryListing>) {
        let mut cache = self.tree_directory_cache.borrow_mut();
        for listing in listings {
            let relative = listing.path.relative_or_empty().to_string();
            let depth = directory_child_depth(&relative);
            let mut children = listing
                .entries
                .into_iter()
                .filter_map(|entry| browser_entry_from_capability(&relative, depth, entry))
                .collect::<Vec<_>>();
            children.sort_by(|left, right| {
                right
                    .is_dir
                    .cmp(&left.is_dir)
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            });
            cache.insert(relative, children);
        }
    }

    fn collect_rows(
        &self,
        relative: &str,
        expanded_dirs: &HashSet<String>,
        rows: &mut Vec<BrowserRow>,
    ) {
        if rows.len() >= MAX_TREE_ROWS {
            return;
        }

        let children = self
            .tree_directory_cache
            .borrow()
            .get(relative)
            .cloned()
            .unwrap_or_default();
        for child in children {
            let should_descend = child.is_dir && expanded_dirs.contains(&child.path);
            let child_path = child.path.clone();
            rows.push(child);
            if rows.len() >= MAX_TREE_ROWS {
                return;
            }
            if should_descend {
                self.collect_rows(&child_path, expanded_dirs, rows);
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RowSignature {
    path: String,
    is_dir: bool,
    executable: bool,
    status: Option<String>,
    ignore: RowIgnoreDisplay,
}

pub(super) struct TreeRowsCache {
    signature: TreeRowsCacheSignature,
    rows: Vec<BrowserRow>,
}

#[derive(PartialEq, Eq)]
pub(super) struct TreeRowsCacheSignature {
    workspace_id: String,
    expanded_dirs: Vec<String>,
}

fn tree_rows_cache_signature(
    workspace: &crate::system::WorkspaceRef,
    expanded_dirs: &HashSet<String>,
) -> TreeRowsCacheSignature {
    let mut expanded_dirs = expanded_dirs.iter().cloned().collect::<Vec<_>>();
    expanded_dirs.sort();

    TreeRowsCacheSignature {
        workspace_id: workspace.id.to_string(),
        expanded_dirs,
    }
}

fn open_directory_relatives(expanded_dirs: &HashSet<String>) -> Vec<String> {
    let mut dirs = vec![String::new()];
    let mut expanded_dirs = expanded_dirs.iter().cloned().collect::<Vec<_>>();
    expanded_dirs.sort_by(|left, right| {
        directory_child_depth(left)
            .cmp(&directory_child_depth(right))
            .then_with(|| left.cmp(right))
    });
    for dir in expanded_dirs {
        if !dir.is_empty() && expanded_directory_is_visible(&dir, dirs.as_slice()) {
            dirs.push(dir);
        }
    }
    dirs
}

fn expanded_directory_is_visible(dir: &str, visible_dirs: &[String]) -> bool {
    let parent = parent_folder(dir);
    visible_dirs.iter().any(|visible| visible == &parent)
}

fn directory_child_depth(relative: &str) -> usize {
    if relative.is_empty() {
        0
    } else {
        relative.matches('/').count() + 1
    }
}

fn browser_entry_from_capability(
    parent: &str,
    depth: usize,
    entry: DirectoryEntry,
) -> Option<BrowserRow> {
    let name = entry.name;
    if should_skip(&name) {
        return None;
    }
    let is_dir = entry.kind == FileKind::Directory;
    let path = if parent.is_empty() {
        name.clone()
    } else {
        format!("{parent}/{name}")
    };
    let mut row = if is_dir {
        BrowserRow::folder(path, name, depth)
    } else {
        BrowserRow::file(path, name, depth)
    };
    row.executable = entry.executable;
    row.ignore_known = entry.git_ignored.is_some();
    if entry.git_ignored == Some(true) {
        row.ignore = RowIgnoreDisplay::GitIgnored;
    }
    Some(row)
}

pub(super) fn insert_changed_path_status(
    file_statuses: &mut HashMap<String, String>,
    path: &str,
    status: &str,
) {
    merge_changed_status(file_statuses, path, status);

    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        merge_changed_status(file_statuses, parent, status);
        current = parent;
    }
}

fn merge_changed_status(file_statuses: &mut HashMap<String, String>, path: &str, status: &str) {
    match file_statuses.get(path) {
        Some(existing) if status_rank(existing) >= status_rank(status) => {}
        _ => {
            file_statuses.insert(path.to_string(), status.to_string());
        }
    }
}

fn status_rank(status: &str) -> u8 {
    if status.contains('U') {
        5
    } else if status.contains('D') {
        4
    } else if status.contains('A') || status.contains('?') {
        3
    } else if status.contains('R') {
        2
    } else {
        1
    }
}

pub(super) fn rows_signature(
    rows: &[BrowserRow],
    changed_file_statuses: &HashMap<String, String>,
) -> Vec<RowSignature> {
    rows.iter()
        .take(MAX_TREE_ROWS)
        .map(|row| RowSignature {
            path: row.path.clone(),
            is_dir: row.is_dir,
            executable: row.executable,
            status: changed_file_statuses.get(&row.path).cloned(),
            ignore: row.ignore,
        })
        .collect()
}

fn parent_folder(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}
