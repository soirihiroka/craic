use super::{
    FileBrowser, MAX_TREE_ROWS, parent_folder, should_skip,
    tree::{BrowserRow, RowCapabilities, RowIgnoreDisplay},
};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{DirectoryListing, FileAccess, FileNodeKind};
use crate::system::path::FileNodeRef;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

impl FileBrowser {
    pub(super) fn visible_rows(self: &Rc<Self>) -> Vec<BrowserRow> {
        let workspace = self.workspace.borrow().clone();
        let file_access = self.file_access.borrow().clone();
        let tree_signature = tree_rows_cache_signature(
            &workspace,
            &self.root_node_path(),
            &self.expanded_dirs.borrow(),
        );
        let mut rows = if let Some(cache) = self.tree_rows_cache.borrow().as_ref()
            && cache.signature == tree_signature
        {
            cache.rows.clone()
        } else {
            self.load_open_directory_rows(&workspace, &file_access, &self.expanded_dirs.borrow());
            let mut rows = Vec::new();
            self.collect_rows(
                &self.root_node_path(),
                &self.expanded_dirs.borrow(),
                &mut rows,
            );
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
            let parent = parent_folder(&changed.path);
            cache.remove(&self.node_path(&parent));
        }
        self.tree_rows_cache.borrow_mut().take();
        self.rows_signature.borrow_mut().clear();
    }

    fn load_open_directory_rows(
        &self,
        workspace: &crate::system::WorkspaceRef,
        file_access: &Arc<dyn FileAccess>,
        expanded_dirs: &HashSet<FileNodePath>,
    ) {
        let open_dirs = open_directory_paths(expanded_dirs, &self.root_node_path());
        let missing_dirs = {
            let cache = self.tree_directory_cache.borrow();
            open_dirs
                .into_iter()
                .filter(|path| !cache.contains_key(path))
                .collect::<Vec<_>>()
        };
        if missing_dirs.is_empty() {
            return;
        }

        log::trace!(
            "file browser list node directories count={} workspace={}",
            missing_dirs.len(),
            workspace.display_name
        );
        let listings = match file_access.list_dirs(&missing_dirs) {
            Ok(listings) => listings,
            Err(err) => {
                log::debug!(
                    "file browser list node directories failed workspace={} dir_count={} err={err}",
                    workspace.display_name,
                    missing_dirs.len()
                );
                Vec::new()
            }
        };
        self.cache_directory_listings(file_access, listings);
    }

    fn cache_directory_listings(
        &self,
        file_access: &Arc<dyn FileAccess>,
        listings: Vec<DirectoryListing>,
    ) {
        let mut cache = self.tree_directory_cache.borrow_mut();
        for listing in listings {
            let depth = directory_child_depth(&listing.path);
            let entry_paths = listing
                .entries
                .into_iter()
                .filter(|path| path.file_name().is_some_and(|name| !should_skip(name)))
                .collect::<Vec<_>>();
            let infos = match file_access.info_many(&entry_paths) {
                Ok(infos) => infos,
                Err(err) => {
                    log::debug!(
                        "file browser node info failed path={} entries={} err={err}",
                        listing.path.display(),
                        entry_paths.len()
                    );
                    Vec::new()
                }
            };
            let mut children = infos
                .into_iter()
                .filter(|info| !should_skip(&info.display_name))
                .map(|info| BrowserRow::from_info(info, depth))
                .collect::<Vec<_>>();
            children.sort_by(|left, right| {
                right
                    .is_dir
                    .cmp(&left.is_dir)
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            });
            cache.insert(listing.path, children);
        }
    }

    fn collect_rows(
        &self,
        node_path: &FileNodePath,
        expanded_dirs: &HashSet<FileNodePath>,
        rows: &mut Vec<BrowserRow>,
    ) {
        if rows.len() >= MAX_TREE_ROWS {
            return;
        }

        let children = self
            .tree_directory_cache
            .borrow()
            .get(node_path)
            .cloned()
            .unwrap_or_default();
        for child in children {
            let should_descend = child.tree_role == super::tree::TreeRowRole::Branch
                && expanded_dirs.contains(&child.node_path);
            let child_path = child.node_path.clone();
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
    path: FileNodePath,
    kind: FileNodeKind,
    executable: bool,
    capabilities: RowCapabilities,
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
    root: FileNodePath,
    expanded_dirs: Vec<FileNodePath>,
}

fn tree_rows_cache_signature(
    workspace: &crate::system::WorkspaceRef,
    root: &FileNodePath,
    expanded_dirs: &HashSet<FileNodePath>,
) -> TreeRowsCacheSignature {
    let mut expanded_dirs = expanded_dirs.iter().cloned().collect::<Vec<_>>();
    expanded_dirs.sort_by_key(FileNodePath::display);

    TreeRowsCacheSignature {
        workspace_id: workspace.id.to_string(),
        root: root.clone(),
        expanded_dirs,
    }
}

fn open_directory_paths(
    expanded_dirs: &HashSet<FileNodePath>,
    root: &FileNodePath,
) -> Vec<FileNodePath> {
    let mut dirs = vec![root.clone()];
    let mut expanded_dirs = expanded_dirs.iter().cloned().collect::<Vec<_>>();
    expanded_dirs.sort_by(|left, right| {
        directory_child_depth(left)
            .cmp(&directory_child_depth(right))
            .then_with(|| left.display().cmp(&right.display()))
    });
    for dir in expanded_dirs {
        if !dir.is_root() && expanded_directory_is_visible(&dir, dirs.as_slice()) {
            dirs.push(dir);
        }
    }
    dirs
}

fn expanded_directory_is_visible(dir: &FileNodePath, visible_dirs: &[FileNodePath]) -> bool {
    let Some(parent) = visible_parent(dir) else {
        return false;
    };
    visible_dirs.iter().any(|visible| visible == &parent)
}

fn visible_parent(path: &FileNodePath) -> Option<FileNodePath> {
    let parent = path.parent()?;
    if matches!(parent.nodes.last(), Some(FileNodeRef::ArchiveRoot { .. })) {
        parent.parent()
    } else {
        Some(parent)
    }
}

fn directory_child_depth(path: &FileNodePath) -> usize {
    let display = path.display();
    if display.is_empty() {
        0
    } else {
        display
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "!")
            .count()
    }
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
            path: row.node_path.clone(),
            kind: row.kind,
            executable: row.executable,
            capabilities: row.capabilities,
            status: changed_file_statuses.get(&row.path).cloned(),
            ignore: row.ignore,
        })
        .collect()
}
