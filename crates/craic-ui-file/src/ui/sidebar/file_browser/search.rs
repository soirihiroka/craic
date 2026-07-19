use super::{
    BrowserTarget, FileBrowser, MAX_SEARCH_FILE_BYTES, MAX_SEARCH_RESULTS, SEARCH_DEBOUNCE_MS,
    rows, skipped_names,
    tree::{BrowserRow, TreeRowRole},
};
use crate::system::FileNodePath;
use crate::system::capabilities::files::{FileSearchMatch, FileSearchOutput, FileSearchQuery};
use crate::ui::components::search::SearchOption;
use craic_ui_core::ui::command_mailbox;
use gtk::prelude::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::thread;
use std::time::Duration;

impl FileBrowser {
    pub fn connect_search(self: &Rc<Self>) {
        self.search_panel.connect_query_changed({
            let browser = self.clone();

            move |query| {
                browser.update_search_query(query.trim().to_string());
            }
        });
        self.search_panel.connect_closed({
            let browser = self.clone();

            move || {
                browser.update_search_query(String::new());
            }
        });
        self.search_panel
            .connect_option_toggled(SearchOption::CaseSensitive, {
                let browser = self.clone();

                move |active| {
                    browser.update_search_mode(SearchMode::CaseSensitive, active);
                }
            });
        self.search_panel
            .connect_option_toggled(SearchOption::WholeWord, {
                let browser = self.clone();

                move |active| {
                    browser.update_search_mode(SearchMode::WholeWord, active);
                }
            });
        self.search_panel
            .connect_option_toggled(SearchOption::Regex, {
                let browser = self.clone();

                move |active| {
                    browser.update_search_mode(SearchMode::Regex, active);
                }
            });
    }

    pub fn set_selected_search_match(self: &Rc<Self>, search_match: SearchMatch) {
        self.active_folder.replace(
            search_match
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path()),
        );
        self.selected_node_path
            .replace(Some(search_match.node_path.clone()));
        self.selected_search_match
            .replace(Some(search_match.selection_key()));
        self.refresh_browser_row_state();
        let callbacks = self.search_match_callbacks.borrow().clone();
        for callback in callbacks {
            callback(
                search_match.node_path.clone(),
                search_match.start,
                search_match.end,
            );
        }
        self.focus_selected_row();
    }

    pub fn update_search_query(self: &Rc<Self>, query: String) {
        if *self.search_query.borrow() == query {
            return;
        }

        self.search_query.replace(query);
        self.search_collapsed_dirs.borrow_mut().clear();
        self.search_output.borrow_mut().take();
        self.pending_new_entry.borrow_mut().take();
        self.pending_rename_entry.borrow_mut().take();
        self.search_generation
            .set(self.search_generation.get().wrapping_add(1));
        self.set_selected_node_path(None);

        if self.search_query.borrow().is_empty() {
            self.cancel_pending_search();
            self.rebuild();
        } else {
            self.start_search();
        }
    }

    fn update_search_mode(self: &Rc<Self>, mode: SearchMode, active: bool) {
        let changed = match mode {
            SearchMode::CaseSensitive if self.search_case_sensitive.get() != active => {
                self.search_case_sensitive.set(active);
                true
            }
            SearchMode::WholeWord if self.search_whole_word.get() != active => {
                self.search_whole_word.set(active);
                true
            }
            SearchMode::Regex if self.search_regex.get() != active => {
                self.search_regex.set(active);
                true
            }
            _ => false,
        };
        if !changed {
            return;
        }

        self.search_collapsed_dirs.borrow_mut().clear();
        self.search_output.borrow_mut().take();
        self.search_generation
            .set(self.search_generation.get().wrapping_add(1));
        self.set_selected_node_path(None);
        if !self.search_query.borrow().is_empty() {
            self.start_search();
        }
    }

    fn search_options(&self) -> SearchOptions {
        SearchOptions {
            query: self.search_query.borrow().clone(),
            case_sensitive: self.search_case_sensitive.get(),
            whole_word: self.search_whole_word.get(),
            regex: self.search_regex.get(),
        }
    }

    pub fn start_search(self: &Rc<Self>) {
        self.cancel_pending_search();
        let options = self.search_options();
        if options.query.is_empty() {
            return;
        }
        let workspace = self.workspace.borrow().clone();
        let root = self.root_node_path();
        let signature = SearchSignature {
            workspace_id: workspace.id.to_string(),
            root,
            options: options.clone(),
            changed_files: changed_file_search_signature(&self.changed_file_statuses.borrow()),
        };
        if self.last_search_signature.borrow().as_ref() == Some(&signature) {
            return;
        }

        let generation = self.search_generation.get();
        self.replace_status_row("Searching repository text...");

        let browser = self.clone();
        let source_id = gtk::glib::timeout_add_local_once(
            Duration::from_millis(SEARCH_DEBOUNCE_MS),
            move || {
                browser.search_source.borrow_mut().take();
                if browser.search_generation.get() != generation {
                    return;
                }
                browser.launch_search(generation);
            },
        );
        self.search_source.replace(Some(source_id));
    }

    fn launch_search(self: &Rc<Self>, generation: u64) {
        let options = self.search_options();
        if options.query.is_empty() || self.search_generation.get() != generation {
            return;
        }
        self.stop_file_watch_scope();
        let file_access = self.file_access.borrow().clone();
        let workspace = self.workspace.borrow().clone();
        let root = file_access.root();
        let signature = SearchSignature {
            workspace_id: workspace.id.to_string(),
            root: root.clone(),
            options: options.clone(),
            changed_files: changed_file_search_signature(&self.changed_file_statuses.borrow()),
        };
        if self.last_search_signature.borrow().as_ref() == Some(&signature) {
            return;
        }
        self.last_search_signature.replace(Some(signature));

        let result_command = command_mailbox::once({
            let browser = self.clone();

            move |result: SearchResult| {
                if result.generation == browser.search_generation.get()
                    && result.options == browser.search_options()
                {
                    browser.replace_search_result_rows(result.result);
                }
            }
        });

        thread::spawn(move || {
            let result = search_repository_text_with_access(file_access, root, &options);
            result_command.send(SearchResult {
                generation,
                options,
                result,
            });
        });
    }

    fn cancel_pending_search(&self) {
        if let Some(source_id) = self.search_source.borrow_mut().take() {
            source_id.remove();
        }
    }

    fn replace_search_result_rows(self: &Rc<Self>, result: Result<SearchOutput, String>) {
        match result {
            Ok(output) => {
                self.search_output.replace(Some(output));
                self.rebuild_search_result_rows_from_cache();
            }
            Err(message) => {
                self.search_output.borrow_mut().take();
                self.replace_search_rows(vec![rows::BrowserListRow::Status(message)]);
            }
        }
    }

    pub fn rebuild_search_result_rows_from_cache(self: &Rc<Self>) {
        let rows = match self.search_output.borrow().clone() {
            Some(output)
                if output.text_matches.is_empty() && output.file_name_matches.is_empty() =>
            {
                vec![rows::BrowserListRow::Status(
                    "No matches found.".to_string(),
                )]
            }
            Some(output) => {
                let mut rows = search_tree_rows(
                    output.text_matches,
                    output.file_name_matches,
                    &output.metadata_rows,
                    &self.search_collapsed_dirs.borrow(),
                    &self.root_node_path(),
                );
                if output.limited {
                    rows.push(rows::BrowserListRow::Status(format!(
                        "Showing first {MAX_SEARCH_RESULTS} matches."
                    )));
                }
                rows
            }
            None => return,
        };
        self.replace_search_rows(rows);
    }

    pub fn remove_deleted_target_from_search_cache(&self, target: &BrowserTarget) {
        if let Some(output) = self.search_output.borrow_mut().as_mut() {
            output.text_matches.retain(|search_match| {
                search_match.node_path != target.node_path
                    && !(target.is_dir && search_match.node_path.is_child_of(&target.node_path))
            });
            output.file_name_matches.retain(|node_path| {
                node_path != &target.node_path
                    && !(target.is_dir && node_path.is_child_of(&target.node_path))
            });
            output.metadata_rows.retain(|node_path, _| {
                node_path != &target.node_path
                    && !(target.is_dir && node_path.is_child_of(&target.node_path))
            });
        }
    }

    fn replace_search_rows(self: &Rc<Self>, rows: Vec<rows::BrowserListRow>) {
        self.rebuilding.set(true);
        self.displayed_rows.borrow_mut().clear();
        self.set_browser_rows(rows);

        self.rebuilding.set(false);
        let adjustment = self.tree.scroller.vadjustment();
        gtk::glib::idle_add_local_once(move || adjustment.set_value(0.0));
    }

    fn replace_status_row(self: &Rc<Self>, message: &str) {
        self.rebuilding.set(true);
        self.set_browser_rows(vec![rows::BrowserListRow::Status(message.to_string())]);
        self.displayed_rows.borrow_mut().clear();
        self.rebuilding.set(false);
        let adjustment = self.tree.scroller.vadjustment();
        gtk::glib::idle_add_local_once(move || adjustment.set_value(0.0));
    }
}

struct SearchResult {
    generation: u64,
    options: SearchOptions,
    result: Result<SearchOutput, String>,
}

#[derive(Clone, PartialEq, Eq)]
struct SearchOptions {
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SearchSignature {
    workspace_id: String,
    root: FileNodePath,
    options: SearchOptions,
    changed_files: Vec<(String, String)>,
}

enum SearchMode {
    CaseSensitive,
    WholeWord,
    Regex,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SearchOutput {
    text_matches: Vec<SearchMatch>,
    file_name_matches: Vec<FileNodePath>,
    metadata_rows: HashMap<FileNodePath, BrowserRow>,
    limited: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SearchSelectionKey {
    node_path: FileNodePath,
    line_number: u64,
    start: usize,
    end: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub node_path: FileNodePath,
    pub line_number: u64,
    pub start: usize,
    pub end: usize,
    pub depth: usize,
    pub text: String,
}

impl SearchMatch {
    pub fn selection_key(&self) -> SearchSelectionKey {
        SearchSelectionKey {
            node_path: self.node_path.clone(),
            line_number: self.line_number,
            start: self.start,
            end: self.end,
        }
    }
}

fn changed_file_search_signature(statuses: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut signature = statuses
        .iter()
        .map(|(path, status)| (path.clone(), status.clone()))
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

fn search_repository_text_with_access(
    file_access: std::sync::Arc<dyn crate::system::capabilities::files::FileAccess>,
    root: FileNodePath,
    options: &SearchOptions,
) -> Result<SearchOutput, String> {
    if !root.is_native() {
        return Err("Search is unavailable for virtual file nodes.".to_string());
    }
    let output = file_access.search_text(FileSearchQuery {
        root: root.clone(),
        query: options.query.clone(),
        case_sensitive: options.case_sensitive,
        whole_word: options.whole_word,
        regex: options.regex,
        max_results: MAX_SEARCH_RESULTS,
        max_file_bytes: MAX_SEARCH_FILE_BYTES,
        excluded_names: skipped_names()
            .into_iter()
            .map(ToString::to_string)
            .collect(),
    })?;
    search_output_from_capability(output, &root, file_access.as_ref())
}

fn search_output_from_capability(
    output: FileSearchOutput,
    root: &FileNodePath,
    file_access: &dyn crate::system::capabilities::files::FileAccess,
) -> Result<SearchOutput, String> {
    let text_matches = output
        .text_matches
        .into_iter()
        .filter_map(|found| search_match_from_capability(found, root))
        .collect::<Vec<_>>();
    let file_name_matches = normalized_file_name_matches(output.file_name_matches, root);
    let metadata_rows = search_metadata_rows(file_access, root, &text_matches, &file_name_matches)?;
    Ok(SearchOutput {
        text_matches,
        file_name_matches,
        metadata_rows,
        limited: output.limited,
    })
}

fn search_match_from_capability(
    found: FileSearchMatch,
    _root: &FileNodePath,
) -> Option<SearchMatch> {
    found.path.native_relative()?;
    Some(SearchMatch {
        node_path: found.path,
        line_number: found.line_number,
        start: found.start,
        end: found.end,
        depth: 0,
        text: found.line_text,
    })
}

fn normalized_file_name_matches(
    mut file_name_matches: Vec<FileNodePath>,
    root: &FileNodePath,
) -> Vec<FileNodePath> {
    file_name_matches.retain(|path| path != root && path.native_relative().is_some());
    file_name_matches.sort_by_key(FileNodePath::display);
    file_name_matches.dedup();
    file_name_matches
}

fn search_metadata_rows(
    file_access: &dyn crate::system::capabilities::files::FileAccess,
    root: &FileNodePath,
    text_matches: &[SearchMatch],
    file_name_matches: &[FileNodePath],
) -> Result<HashMap<FileNodePath, BrowserRow>, String> {
    let mut paths = Vec::new();
    for search_match in text_matches {
        push_search_metadata_paths(&mut paths, &search_match.node_path, root);
    }
    for path in file_name_matches {
        push_search_metadata_paths(&mut paths, path, root);
    }
    paths.sort_by_key(FileNodePath::display);
    paths.dedup();

    let rows = file_access
        .info_many(&paths)?
        .into_iter()
        .map(|info| {
            let depth = search_node_depth(&info.path);
            let row = BrowserRow::from_info(info, depth);
            (row.node_path.clone(), row)
        })
        .collect();
    Ok(rows)
}

fn push_search_metadata_paths(
    paths: &mut Vec<FileNodePath>,
    path: &FileNodePath,
    root: &FileNodePath,
) {
    let mut current = Some(path.clone());
    while let Some(path) = current {
        if &path == root {
            break;
        }
        paths.push(path.clone());
        current = path.parent();
    }
}

fn search_node_depth(path: &FileNodePath) -> usize {
    path.native_relative()
        .unwrap_or_else(|| path.display())
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(1)
}

fn search_tree_rows(
    mut text_matches: Vec<SearchMatch>,
    file_name_matches: Vec<FileNodePath>,
    metadata_rows: &HashMap<FileNodePath, BrowserRow>,
    collapsed_dirs: &HashSet<FileNodePath>,
    root: &FileNodePath,
) -> Vec<rows::BrowserListRow> {
    let mut rows = Vec::new();
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut text_matches_by_path: HashMap<FileNodePath, Vec<SearchMatch>> = HashMap::new();
    let mut ordered_paths = file_name_matches;

    for search_match in text_matches.drain(..) {
        ordered_paths.push(search_match.node_path.clone());
        text_matches_by_path
            .entry(search_match.node_path.clone())
            .or_default()
            .push(search_match);
    }
    ordered_paths.sort_by_key(FileNodePath::display);
    ordered_paths.dedup();

    for path in ordered_paths {
        let mut matches = text_matches_by_path.remove(&path).unwrap_or_default();
        let has_text_matches = !matches.is_empty();
        let relative = path.native_relative().unwrap_or_else(|| path.display());
        let parts = relative
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let mut current_node = root.clone();
        let mut hidden_by_collapsed_dir = false;
        for (depth, name) in parts.iter().take(parts.len().saturating_sub(1)).enumerate() {
            current_node = current_node.join_child(*name);
            if seen_dirs.insert(current_node.clone()) {
                if let Some(row) = metadata_rows.get(&current_node) {
                    rows.push(rows::BrowserListRow::Tree(row.clone()));
                } else {
                    log::debug!(
                        "file search metadata missing ancestor path={} depth={}",
                        current_node.display(),
                        depth
                    );
                }
            }
            if collapsed_dirs.contains(&current_node) {
                hidden_by_collapsed_dir = true;
                break;
            }
        }
        if hidden_by_collapsed_dir {
            continue;
        }

        let Some(file_row) = metadata_rows.get(&path) else {
            log::debug!("file search metadata missing file path={}", path.display());
            continue;
        };
        if seen_files.insert(path.clone()) {
            let mut row = file_row.clone();
            if has_text_matches {
                row.tree_role = TreeRowRole::Branch;
            }
            rows.push(rows::BrowserListRow::Tree(row));
        }

        if collapsed_dirs.contains(&path) {
            continue;
        }

        for mut search_match in matches.drain(..) {
            search_match.depth = parts.len();
            rows.push(rows::BrowserListRow::Search(search_match));
        }
    }

    rows
}
