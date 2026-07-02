use super::{
    FileBrowser, MAX_TREE_ROWS, SEARCH_POLL_MS, file_name,
    tree::{BrowserRow, RowIgnoreDisplay},
};
use crate::gitignore::IgnoreCheck;
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileAccess, FileNodeKind};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const GIT_IGNORE_CACHE_TTL: Duration = Duration::from_secs(5);

impl FileBrowser {
    pub(super) fn refresh_git_ignore_cache_for_rules(&self, rows: &[BrowserRow]) {
        if rows
            .iter()
            .all(|row| row.ignore_known || !row.capabilities.native)
        {
            self.git_ignore_rules_signature.borrow_mut().take();
            self.git_ignore_cache.borrow_mut().clear();
            return;
        }

        let file_access = self.file_access.borrow().clone();
        let root = self.root_node_path();
        let signature = git_ignore_rules_signature(file_access.as_ref(), &root, rows);
        let mut current_signature = self.git_ignore_rules_signature.borrow_mut();
        if current_signature.as_ref() == Some(&signature) {
            return;
        }

        let had_signature = current_signature.is_some();
        *current_signature = Some(signature);
        if had_signature {
            self.refresh_git_ignore_cache();
        }
    }

    pub(super) fn apply_git_ignore_cache(&self, rows: &mut [BrowserRow]) {
        let cache = self.git_ignore_cache.borrow();
        let mut ignored_stack = Vec::new();

        for row in rows {
            ignored_stack.truncate(row.depth);
            let parent_ignored = ignored_stack.last().copied().unwrap_or(false);
            let provider_ignored = row.ignore == RowIgnoreDisplay::GitIgnored;
            row.ignore = match cache.entries.get(&row.path).map(|entry| entry.state) {
                Some(GitIgnoreState::Ignored) => RowIgnoreDisplay::GitIgnored,
                Some(GitIgnoreState::NotIgnored) => RowIgnoreDisplay::None,
                None if provider_ignored => RowIgnoreDisplay::GitIgnored,
                None if parent_ignored => RowIgnoreDisplay::Inherited,
                None => RowIgnoreDisplay::None,
            };
            ignored_stack.push(row.ignore.is_ignored());
        }
    }

    pub(super) fn queue_git_ignore_query(self: &Rc<Self>, rows: &[BrowserRow]) {
        if rows.is_empty() {
            return;
        }
        let Some(git_access) = self.git_access.borrow().clone() else {
            return;
        };

        let generation = self.git_ignore_generation.get();
        let now = Instant::now();
        let queries = {
            let mut cache = self.git_ignore_cache.borrow_mut();
            rows.iter()
                .take(MAX_TREE_ROWS)
                .filter_map(|row| {
                    if row.ignore_known || !row.capabilities.native {
                        return None;
                    }
                    let cache_is_fresh = cache.entries.get(&row.path).is_some_and(|entry| {
                        entry.generation == generation
                            && now.duration_since(entry.checked_at) < GIT_IGNORE_CACHE_TTL
                    });
                    if cache_is_fresh || cache.pending.contains(&row.path) {
                        return None;
                    }
                    cache.pending.insert(row.path.clone());
                    Some(IgnoreCheck {
                        path: row.path.clone(),
                        is_dir: row.is_dir,
                    })
                })
                .collect::<Vec<_>>()
        };
        if queries.is_empty() {
            return;
        }

        let pending_paths = queries
            .iter()
            .map(|query| query.path.clone())
            .collect::<Vec<_>>();
        let disconnected_pending_paths = pending_paths.clone();
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let ignored_paths = git_access.check_ignored_paths(&queries);
            let _ = sender.send(GitIgnoreQueryResult {
                generation,
                paths: pending_paths,
                ignored_paths,
            });
        });

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();

            move || match receiver.try_recv() {
                Ok(result) => {
                    if result.generation == browser.git_ignore_generation.get()
                        && browser.apply_git_ignore_result(result.paths, result.ignored_paths)
                        && browser.search_query.borrow().is_empty()
                    {
                        browser.rebuild_if_changed();
                    }
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    if generation == browser.git_ignore_generation.get() {
                        browser.clear_git_ignore_pending(&disconnected_pending_paths);
                    }
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    }

    fn apply_git_ignore_result(
        &self,
        paths: Vec<String>,
        ignored_paths: Result<HashSet<String>, String>,
    ) -> bool {
        let checked_at = Instant::now();
        let mut cache = self.git_ignore_cache.borrow_mut();
        for path in &paths {
            cache.pending.remove(path);
        }

        let ignored_paths = match ignored_paths {
            Ok(ignored_paths) => ignored_paths,
            Err(err) => {
                log::debug!(
                    "git ignore query failed workspace={} err={err}",
                    self.workspace.borrow().display_name
                );
                return false;
            }
        };

        let mut changed = false;
        let generation = self.git_ignore_generation.get();
        for path in paths {
            let state = if ignored_paths.contains(&path) {
                GitIgnoreState::Ignored
            } else {
                GitIgnoreState::NotIgnored
            };
            if cache
                .entries
                .get(&path)
                .is_none_or(|entry| entry.state != state)
            {
                changed = true;
            }
            cache.entries.insert(
                path,
                GitIgnoreCacheEntry {
                    state,
                    checked_at,
                    generation,
                },
            );
        }
        changed
    }

    fn clear_git_ignore_pending(&self, paths: &[String]) {
        let mut cache = self.git_ignore_cache.borrow_mut();
        for path in paths {
            cache.pending.remove(path);
        }
    }

    fn refresh_git_ignore_cache(&self) {
        self.git_ignore_cache.borrow_mut().pending.clear();
        self.git_ignore_generation
            .set(self.git_ignore_generation.get().wrapping_add(1));
    }

    pub(super) fn clear_git_ignore_cache(&self) {
        self.git_ignore_cache.borrow_mut().clear();
        self.git_ignore_generation
            .set(self.git_ignore_generation.get().wrapping_add(1));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GitIgnoreState {
    Ignored,
    NotIgnored,
}

pub(super) struct GitIgnoreCacheEntry {
    state: GitIgnoreState,
    checked_at: Instant,
    generation: u64,
}

#[derive(Default)]
pub(super) struct GitIgnoreCache {
    entries: HashMap<String, GitIgnoreCacheEntry>,
    pending: HashSet<String>,
}

impl GitIgnoreCache {
    fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct GitIgnoreRuleFileSignature {
    path: FileNodePath,
    state: GitIgnoreRuleFileState,
}

#[derive(PartialEq, Eq)]
enum GitIgnoreRuleFileState {
    Missing,
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
}

struct GitIgnoreQueryResult {
    generation: u64,
    paths: Vec<String>,
    ignored_paths: Result<HashSet<String>, String>,
}

fn git_ignore_rules_signature(
    file_access: &dyn FileAccess,
    root: &FileNodePath,
    rows: &[BrowserRow],
) -> Vec<GitIgnoreRuleFileSignature> {
    let mut paths = rows
        .iter()
        .filter(|row| {
            row.capabilities.native && !row.is_dir && file_name(&row.path) == ".gitignore"
        })
        .map(|row| row.node_path.clone())
        .collect::<Vec<_>>();
    paths.push(
        root.join_child(".git")
            .join_child("info")
            .join_child("exclude"),
    );
    paths.sort_by_key(FileNodePath::display);
    paths.dedup();

    paths
        .into_iter()
        .map(|path| {
            let state = git_ignore_rule_file_state(file_access, &path);
            GitIgnoreRuleFileSignature { path, state }
        })
        .collect()
}

fn git_ignore_rule_file_state(
    file_access: &dyn FileAccess,
    path: &FileNodePath,
) -> GitIgnoreRuleFileState {
    match file_access.info(path) {
        Ok(info) if info.kind == FileNodeKind::File => GitIgnoreRuleFileState::Present {
            len: info.len.unwrap_or_default(),
            modified: info.modified,
        },
        _ => GitIgnoreRuleFileState::Missing,
    }
}
