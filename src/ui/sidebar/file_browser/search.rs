use super::{
    FileBrowser, MAX_SEARCH_FILE_BYTES, MAX_SEARCH_RESULTS, SEARCH_DEBOUNCE_MS, SEARCH_POLL_MS,
    file_name, parent_folder, rows, skipped_names, tree::BrowserRow,
};
use crate::system::capabilities::files::{FileSearchMatch, FileSearchOutput, FileSearchQuery};
use crate::ui::components::search::SearchOption;
use gtk::prelude::*;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;

impl FileBrowser {
    pub(super) fn connect_search(self: &Rc<Self>) {
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

    pub(super) fn set_selected_search_match(self: &Rc<Self>, search_match: SearchMatch) {
        self.active_folder
            .replace(parent_folder(&search_match.path));
        self.selected_path.replace(search_match.path.clone());
        self.selected_search_match
            .replace(Some(search_match.selection_key()));
        self.refresh_browser_row_state();
        let callbacks = self.search_match_callbacks.borrow().clone();
        for callback in callbacks {
            callback(
                search_match.path.clone(),
                search_match.start,
                search_match.end,
            );
        }
        self.focus_selected_row();
    }

    pub(super) fn update_search_query(self: &Rc<Self>, query: String) {
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
        self.set_selected_path(String::new());

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
        self.set_selected_path(String::new());
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

    pub(super) fn start_search(self: &Rc<Self>) {
        self.cancel_pending_search();
        let options = self.search_options();
        if options.query.is_empty() {
            return;
        }
        let workspace = self.workspace.borrow().clone();
        let signature = SearchSignature {
            workspace_id: workspace.id.to_string(),
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
        let signature = SearchSignature {
            workspace_id: workspace.id.to_string(),
            options: options.clone(),
            changed_files: changed_file_search_signature(&self.changed_file_statuses.borrow()),
        };
        if self.last_search_signature.borrow().as_ref() == Some(&signature) {
            return;
        }
        self.last_search_signature.replace(Some(signature));

        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let result =
                search_repository_text_with_access(file_access, workspace.root.clone(), &options);
            let _ = sender.send(SearchResult {
                generation,
                options,
                result,
            });
        });

        gtk::glib::timeout_add_local(Duration::from_millis(SEARCH_POLL_MS), {
            let browser = self.clone();

            move || match receiver.try_recv() {
                Ok(result) => {
                    if result.generation == browser.search_generation.get()
                        && result.options == browser.search_options()
                    {
                        browser.replace_search_result_rows(result.result);
                    }
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    if generation == browser.search_generation.get() {
                        browser.replace_search_result_rows(Err(
                            "File search did not return a result.".to_string(),
                        ));
                    }
                    gtk::glib::ControlFlow::Break
                }
            }
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

    pub(super) fn rebuild_search_result_rows_from_cache(self: &Rc<Self>) {
        let rows = match self.search_output.borrow().clone() {
            Some(output) if output.matches.is_empty() => {
                vec![rows::BrowserListRow::Status(
                    "No matches found.".to_string(),
                )]
            }
            Some(output) => {
                let mut rows =
                    search_tree_rows(output.matches, &self.search_collapsed_dirs.borrow());
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
pub(super) struct SearchSignature {
    workspace_id: String,
    options: SearchOptions,
    changed_files: Vec<(String, String)>,
}

enum SearchMode {
    CaseSensitive,
    WholeWord,
    Regex,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SearchOutput {
    matches: Vec<SearchMatch>,
    limited: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SearchSelectionKey {
    path: String,
    line_number: u64,
    start: usize,
    end: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SearchMatch {
    pub(super) path: String,
    pub(super) line_number: u64,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) depth: usize,
    pub(super) text: String,
}

impl SearchMatch {
    pub(super) fn selection_key(&self) -> SearchSelectionKey {
        SearchSelectionKey {
            path: self.path.clone(),
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
    root: crate::system::WorkspacePath,
    options: &SearchOptions,
) -> Result<SearchOutput, String> {
    let output = file_access.search_text(FileSearchQuery {
        root,
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
    Ok(search_output_from_capability(output))
}

fn search_output_from_capability(output: FileSearchOutput) -> SearchOutput {
    SearchOutput {
        matches: output
            .matches
            .into_iter()
            .filter_map(search_match_from_capability)
            .collect(),
        limited: output.limited,
    }
}

fn search_match_from_capability(found: FileSearchMatch) -> Option<SearchMatch> {
    Some(SearchMatch {
        path: found.path.relative?,
        line_number: found.line_number,
        start: found.start,
        end: found.end,
        depth: 0,
        text: found.line_text,
    })
}

fn search_tree_rows(
    mut matches: Vec<SearchMatch>,
    collapsed_dirs: &HashSet<String>,
) -> Vec<rows::BrowserListRow> {
    let mut rows = Vec::new();
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();

    for mut search_match in matches.drain(..) {
        let parts = search_match
            .path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let mut current = String::new();
        let mut hidden_by_collapsed_dir = false;
        for (depth, name) in parts.iter().take(parts.len().saturating_sub(1)).enumerate() {
            if current.is_empty() {
                current.push_str(name);
            } else {
                current.push('/');
                current.push_str(name);
            }
            if seen_dirs.insert(current.clone()) {
                rows.push(rows::BrowserListRow::Tree(BrowserRow::folder(
                    current.clone(),
                    (*name).to_string(),
                    depth,
                )));
            }
            if collapsed_dirs.contains(&current) {
                hidden_by_collapsed_dir = true;
                break;
            }
        }
        if hidden_by_collapsed_dir {
            continue;
        }

        if seen_files.insert(search_match.path.clone()) {
            rows.push(rows::BrowserListRow::Tree(BrowserRow::search_file_group(
                search_match.path.clone(),
                file_name(&search_match.path).to_string(),
                parts.len().saturating_sub(1),
            )));
        }

        if collapsed_dirs.contains(&search_match.path) {
            continue;
        }

        search_match.depth = parts.len();
        rows.push(rows::BrowserListRow::Search(search_match));
    }

    rows
}
