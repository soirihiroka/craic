use super::{FileBrowser, tree::BrowserRow};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileWatchCallback, FileWatchRequest};
use gtk::glib;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

const FILE_BROWSER_WATCH_REFRESH_DEBOUNCE: Duration = Duration::from_millis(350);
const FILE_BROWSER_WATCH_REFRESH_MAX_DELAY: Duration = Duration::from_millis(1200);
static NEXT_FILE_WATCH_UI_HANDLER_ID: AtomicU64 = AtomicU64::new(1);

type FileWatchPendingSet = Arc<Mutex<HashSet<FileNodePath>>>;

thread_local! {
    static FILE_WATCH_UI_HANDLERS: RefCell<HashMap<u64, FileWatchUiHandler>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBrowserWatchSignature {
    workspace_id: String,
    directories: Vec<FileNodePath>,
}

struct FileWatchUiHandler {
    browser: Weak<FileBrowser>,
    generation: u64,
    pending_from_threads: FileWatchPendingSet,
    event_scheduled: Arc<AtomicBool>,
    pending_changed_paths: HashSet<FileNodePath>,
    pending_since: Option<Instant>,
    last_change_at: Option<Instant>,
    quiet_source: Option<glib::SourceId>,
    max_source: Option<glib::SourceId>,
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
        let handler_id = NEXT_FILE_WATCH_UI_HANDLER_ID.fetch_add(1, Ordering::Relaxed);
        let pending_from_threads = Arc::new(Mutex::new(HashSet::new()));
        let event_scheduled = Arc::new(AtomicBool::new(false));
        let mut subscriptions = Vec::new();
        FILE_WATCH_UI_HANDLERS.with(|handlers| {
            handlers.borrow_mut().insert(
                handler_id,
                FileWatchUiHandler {
                    browser: Rc::downgrade(self),
                    generation,
                    pending_from_threads: pending_from_threads.clone(),
                    event_scheduled: event_scheduled.clone(),
                    pending_changed_paths: HashSet::new(),
                    pending_since: None,
                    last_change_at: None,
                    quiet_source: None,
                    max_source: None,
                },
            );
        });

        let request = FileWatchRequest {
            paths: signature.directories.clone(),
            recursive: false,
        };
        let callback_pending = pending_from_threads.clone();
        let callback_scheduled = event_scheduled.clone();
        let callback: FileWatchCallback = Arc::new(move |changes| {
            if let Ok(mut pending) = callback_pending.lock() {
                pending.extend(changes);
            } else {
                return;
            }

            if callback_scheduled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                gtk::glib::idle_add_once(move || drain_file_watch_ui_events(handler_id));
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
            remove_file_watch_ui_handler(handler_id);
            return;
        }

        self.file_watch_signature.replace(Some(signature.clone()));
        self.file_watch_subscriptions.replace(subscriptions);
        self.file_watch_handler_id.set(Some(handler_id));
    }

    pub fn stop_file_watch_scope(&self) {
        self.file_watch_generation
            .set(self.file_watch_generation.get().wrapping_add(1).max(1));
        if let Some(handler_id) = self.file_watch_handler_id.take() {
            remove_file_watch_ui_handler(handler_id);
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

fn drain_file_watch_ui_events(handler_id: u64) {
    let mut should_schedule_quiet = false;
    let mut should_schedule_max = false;
    FILE_WATCH_UI_HANDLERS.with(|handlers| {
        let mut handlers = handlers.borrow_mut();
        let Some(handler) = handlers.get_mut(&handler_id) else {
            return;
        };
        handler.event_scheduled.store(false, Ordering::Release);
        let Some(browser) = handler.browser.upgrade() else {
            return;
        };
        if browser.file_watch_generation.get() != handler.generation {
            return;
        }

        let changes = handler
            .pending_from_threads
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        if changes.is_empty() {
            return;
        }

        let now = Instant::now();
        if handler.pending_since.is_none() {
            handler.pending_since = Some(now);
            should_schedule_max = handler.max_source.is_none();
        }
        handler.last_change_at = Some(now);
        handler.pending_changed_paths.extend(changes);

        if let Some(source_id) = handler.quiet_source.take() {
            source_id.remove();
        }
        should_schedule_quiet = true;
    });

    if should_schedule_quiet {
        FILE_WATCH_UI_HANDLERS.with(|handlers| {
            if let Some(handler) = handlers.borrow_mut().get_mut(&handler_id) {
                handler.quiet_source = Some(glib::timeout_add_local_once(
                    FILE_BROWSER_WATCH_REFRESH_DEBOUNCE,
                    move || flush_file_watch_ui_events(handler_id, false),
                ));
            }
        });
    }

    if should_schedule_max {
        FILE_WATCH_UI_HANDLERS.with(|handlers| {
            if let Some(handler) = handlers.borrow_mut().get_mut(&handler_id)
                && handler.max_source.is_none()
            {
                handler.max_source = Some(glib::timeout_add_local_once(
                    FILE_BROWSER_WATCH_REFRESH_MAX_DELAY,
                    move || flush_file_watch_ui_events(handler_id, true),
                ));
            }
        });
    }
}

fn flush_file_watch_ui_events(handler_id: u64, force: bool) {
    let mut refresh = None;
    FILE_WATCH_UI_HANDLERS.with(|handlers| {
        let mut handlers = handlers.borrow_mut();
        let Some(handler) = handlers.get_mut(&handler_id) else {
            return;
        };
        let Some(browser) = handler.browser.upgrade() else {
            handlers.remove(&handler_id);
            return;
        };
        if browser.file_watch_generation.get() != handler.generation {
            handlers.remove(&handler_id);
            return;
        }

        if force {
            handler.max_source.take();
        } else {
            handler.quiet_source.take();
            let quiet = handler.last_change_at.is_some_and(|last| {
                Instant::now().duration_since(last) >= FILE_BROWSER_WATCH_REFRESH_DEBOUNCE
            });
            if !quiet {
                handler.quiet_source = Some(glib::timeout_add_local_once(
                    FILE_BROWSER_WATCH_REFRESH_DEBOUNCE,
                    move || flush_file_watch_ui_events(handler_id, false),
                ));
                return;
            }
        }

        if handler.pending_changed_paths.is_empty() {
            handler.pending_since = None;
            handler.last_change_at = None;
            if let Some(source_id) = handler.quiet_source.take() {
                source_id.remove();
            }
            if let Some(source_id) = handler.max_source.take() {
                source_id.remove();
            }
            return;
        }

        let changed_paths = std::mem::take(&mut handler.pending_changed_paths);
        handler.pending_since = None;
        handler.last_change_at = None;
        if let Some(source_id) = handler.quiet_source.take() {
            source_id.remove();
        }
        if let Some(source_id) = handler.max_source.take() {
            source_id.remove();
        }
        refresh = Some((browser, handler.generation, changed_paths));
    });

    if let Some((browser, generation, changed_paths)) = refresh {
        browser.refresh_watched_folder_view(generation, changed_paths);
    }
}

fn remove_file_watch_ui_handler(handler_id: u64) {
    FILE_WATCH_UI_HANDLERS.with(|handlers| {
        if let Some(mut handler) = handlers.borrow_mut().remove(&handler_id) {
            if let Some(source_id) = handler.quiet_source.take() {
                source_id.remove();
            }
            if let Some(source_id) = handler.max_source.take() {
                source_id.remove();
            }
        }
    });
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
