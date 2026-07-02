use super::{FileBrowser, tree::BrowserRow};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileWatchCallback, FileWatchRequest};
use gtk::glib;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const FILE_BROWSER_WATCH_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileBrowserWatchSignature {
    workspace_id: String,
    directories: Vec<FileNodePath>,
}

impl FileBrowser {
    pub(super) fn update_file_watch_scope(self: &Rc<Self>, rows: &[BrowserRow]) {
        if !self.search_query.borrow().is_empty() {
            self.stop_file_watch_scope();
            return;
        }

        let workspace = self.workspace.borrow().clone();
        let root = self.root_node_path();
        let directories = open_folder_watch_directories(rows, &self.expanded_dirs.borrow(), &root);
        let signature = FileBrowserWatchSignature {
            workspace_id: workspace.id.to_string(),
            directories,
        };
        if self.file_watch_signature.borrow().as_ref() == Some(&signature) {
            return;
        }

        self.stop_file_watch_scope();
        self.file_watch_signature.replace(Some(signature.clone()));
        let generation = self.file_watch_generation.get();
        let file_access = self.file_access.borrow().clone();
        let (sender, receiver) = mpsc::channel();
        let sender = Arc::new(Mutex::new(sender));
        let mut subscriptions = Vec::new();

        let request = FileWatchRequest {
            paths: signature.directories.clone(),
            recursive: false,
        };
        let callback: FileWatchCallback = Arc::new(move |changes| {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(changes);
            }
        });
        match file_access.watch(request, callback) {
            Ok(subscription) => subscriptions.push(subscription),
            Err(err) => {
                log::warn!(
                    "file browser watch registration failed workspace={} watched_dirs={} err={err}",
                    workspace.display_name,
                    signature.directories.len()
                );
            }
        }

        log::info!(
            "file browser watch scope updated workspace={} watched_dirs={} subscriptions={}",
            workspace.display_name,
            signature.directories.len(),
            subscriptions.len()
        );

        if subscriptions.is_empty() {
            return;
        }

        let browser = Rc::clone(self);
        let source_id =
            glib::timeout_add_local(FILE_BROWSER_WATCH_EVENT_POLL_INTERVAL, move || {
                if browser.file_watch_generation.get() != generation {
                    return glib::ControlFlow::Break;
                }

                let mut changed_paths = HashSet::new();
                while let Ok(changes) = receiver.try_recv() {
                    changed_paths.extend(changes);
                }

                if !changed_paths.is_empty() {
                    browser.refresh_watched_folder_view(generation, changed_paths);
                }

                glib::ControlFlow::Continue
            });
        self.file_watch_subscriptions.replace(subscriptions);
        self.file_watch_event_source.replace(Some(source_id));
    }

    pub(super) fn stop_file_watch_scope(&self) {
        self.file_watch_generation
            .set(self.file_watch_generation.get().wrapping_add(1).max(1));
        if let Some(source_id) = self.file_watch_event_source.borrow_mut().take() {
            source_id.remove();
        }
        self.file_watch_subscriptions.borrow_mut().clear();
        self.file_watch_signature.borrow_mut().take();
    }

    fn refresh_watched_folder_view(
        self: &Rc<Self>,
        generation: u64,
        changed_paths: HashSet<FileNodePath>,
    ) {
        if self.file_watch_generation.get() != generation {
            return;
        }

        let Some(signature) = self.file_watch_signature.borrow().clone() else {
            return;
        };

        let changed_paths = self.delete_watch_filtered_paths(changed_paths);
        if changed_paths.is_empty() {
            log::trace!(
                "file browser watch ignored delete-local changes workspace={} watched_dirs={}",
                signature.workspace_id,
                signature.directories.len()
            );
            return;
        }

        let invalidated_dirs =
            watched_directories_for_changes(&signature.directories, &changed_paths);
        if invalidated_dirs.is_empty() {
            log::trace!(
                "file browser watch ignored changes workspace={} changed_paths={} watched_dirs={}",
                signature.workspace_id,
                changed_paths.len(),
                signature.directories.len()
            );
            return;
        }

        log::debug!(
            "file browser watch refresh workspace={} changed_paths={} invalidated_dirs={}",
            signature.workspace_id,
            changed_paths.len(),
            invalidated_dirs.len()
        );
        {
            let mut cache = self.tree_directory_cache.borrow_mut();
            for directory in &invalidated_dirs {
                cache.remove(directory);
            }
        }
        self.tree_rows_cache.borrow_mut().take();
        self.rows_signature.borrow_mut().clear();

        if self.search_query.borrow().is_empty() {
            self.rebuild_if_changed();
        }
    }

    fn delete_watch_filtered_paths(
        &self,
        changed_paths: HashSet<FileNodePath>,
    ) -> HashSet<FileNodePath> {
        let suppressed = self.delete_watch_suppression_paths.borrow();
        if suppressed.is_empty() {
            return changed_paths;
        }

        changed_paths
            .into_iter()
            .filter(|changed| {
                !suppressed.iter().any(|deleted| {
                    changed == deleted
                        || changed.is_child_of(deleted)
                        || deleted.is_child_of(changed)
                })
            })
            .collect()
    }
}

fn open_folder_watch_directories(
    rows: &[BrowserRow],
    expanded_dirs: &HashSet<FileNodePath>,
    root: &FileNodePath,
) -> Vec<FileNodePath> {
    let mut directories = HashSet::new();
    directories.insert(root.clone());
    for row in rows {
        if row.is_dir && row.capabilities.watchable && expanded_dirs.contains(&row.node_path) {
            directories.insert(row.node_path.clone());
        }
    }

    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(FileNodePath::display);
    directories
}

fn watched_directories_for_changes(
    watched_directories: &[FileNodePath],
    changed_paths: &HashSet<FileNodePath>,
) -> HashSet<FileNodePath> {
    let watched = watched_directories.iter().collect::<HashSet<_>>();
    let mut invalidated = HashSet::new();

    for changed_path in changed_paths {
        if watched.contains(changed_path) {
            invalidated.insert(changed_path.clone());
        }
        if let Some(parent) = nearest_watched_parent(&watched, changed_path) {
            invalidated.insert(parent);
        }
    }

    invalidated
}

fn nearest_watched_parent(
    watched: &HashSet<&FileNodePath>,
    path: &FileNodePath,
) -> Option<FileNodePath> {
    let mut current = path.parent();
    while let Some(path) = current {
        if watched.contains(&path) {
            return Some(path);
        }
        current = path.parent();
    }
    None
}
