use super::{FileBrowser, tree::BrowserRow};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileWatchCallback, FileWatchRequest};
use craic_ui_core::ui::command_mailbox;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const FILE_BROWSER_WATCH_REFRESH_DEBOUNCE: Duration = Duration::from_millis(350);
const FILE_BROWSER_WATCH_REFRESH_MAX_DELAY: Duration = Duration::from_millis(1200);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBrowserWatchSignature {
    workspace_id: String,
    directories: Vec<FileNodePath>,
}

impl FileBrowser {
    pub fn update_file_watch_scope(self: &Rc<Self>, rows: &[BrowserRow]) {
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
        let generation = self.file_watch_generation.get();
        let file_access = self.file_access.borrow().clone();
        let browser = Rc::downgrade(self);
        let (updates, event_subscription) =
            command_mailbox::latest(move |(event_generation, changed_paths)| {
                let Some(browser) = browser.upgrade() else {
                    return;
                };
                browser.refresh_watched_folder_view(event_generation, changed_paths);
            });
        let (changes_sender, changes_receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let debounce_workspace = signature.workspace_id.clone();
        thread::spawn(move || {
            debounce_file_watch_events(
                debounce_workspace,
                generation,
                changes_receiver,
                stop_receiver,
                updates,
            );
        });
        let mut subscriptions = Vec::new();

        let request = FileWatchRequest {
            paths: signature.directories.clone(),
            recursive: false,
        };
        let callback: FileWatchCallback = Arc::new(move |changes| {
            let _ = changes_sender.send(changes);
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
            let _ = stop_sender.send(());
            return;
        }

        self.file_watch_signature.replace(Some(signature.clone()));
        self.file_watch_subscriptions.replace(subscriptions);
        self.file_watch_event_subscription
            .replace(Some(event_subscription));
        self.file_watch_debounce_stop.replace(Some(stop_sender));
    }

    pub fn stop_file_watch_scope(&self) {
        self.file_watch_generation
            .set(self.file_watch_generation.get().wrapping_add(1).max(1));
        if let Some(stop_sender) = self.file_watch_debounce_stop.borrow_mut().take() {
            let _ = stop_sender.send(());
        }
        self.file_watch_event_subscription.borrow_mut().take();
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

fn debounce_file_watch_events(
    workspace_id: String,
    generation: u64,
    changes: mpsc::Receiver<HashSet<FileNodePath>>,
    stop: mpsc::Receiver<()>,
    updates: command_mailbox::UiCommandSender<(u64, HashSet<FileNodePath>)>,
) {
    log::info!(
        "file browser debounce worker started workspace={} generation={}",
        workspace_id,
        generation
    );
    let mut pending = HashSet::new();
    let mut pending_since = None;
    let mut last_change_at = None;
    loop {
        if stop.try_recv().is_ok() {
            break;
        }
        match changes.recv_timeout(Duration::from_millis(50)) {
            Ok(paths) => {
                let now = Instant::now();
                pending_since.get_or_insert(now);
                last_change_at = Some(now);
                pending.extend(paths);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        let quiet = last_change_at
            .is_some_and(|last| now.duration_since(last) >= FILE_BROWSER_WATCH_REFRESH_DEBOUNCE);
        let max_delay = pending_since
            .is_some_and(|first| now.duration_since(first) >= FILE_BROWSER_WATCH_REFRESH_MAX_DELAY);
        if !pending.is_empty() && (quiet || max_delay) {
            updates.send((generation, std::mem::take(&mut pending)));
            pending_since = None;
            last_change_at = None;
        }
    }
    log::info!(
        "file browser debounce worker stopped workspace={} generation={}",
        workspace_id,
        generation
    );
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
