use crate::git::ChangedFile;
use crate::spellcheck::SpellcheckAllowlist;
use crate::system::capabilities::{
    files::{FileAccess, FileNodeInfo, FileNodeKind, FileWatchSubscription},
    git::GitAccess,
    open::{DesktopOpenAccess, DesktopOpenActivation},
};
use crate::system::{FileNodePath, WorkspaceRef};
use crate::ui::components::context_menu;
use crate::ui::components::search::SearchPanel;
use crate::ui::components::tree_view;
use adw::prelude::*;
use gtk::{gdk, gio};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

mod file_ops;
mod ignore_status;
mod menu;
mod rows;
mod search;
mod transfer;
mod tree;
mod tree_loader;
mod watch;

pub(in crate::ui) use self::file_ops::NewEntryKind;
use self::file_ops::{PendingNewEntry, PendingRenameEntry};
use self::ignore_status::{GitIgnoreCache, GitIgnoreRuleFileSignature};
use self::search::SearchMatch;
use self::search::{SearchOutput, SearchSelectionKey, SearchSignature};
use self::transfer::TransferRowProgress;
use self::transfer::{ActiveTransfer, FileClipboard, TransferOperation};
use self::tree::{BrowserRow, RowCapabilities};
use self::tree_loader::{RowSignature, TreeRowsCache, insert_changed_path_status, rows_signature};
use self::watch::FileBrowserWatchSignature;

const MAX_TREE_ROWS: usize = 4_000;
const MAX_SEARCH_RESULTS: usize = 250;
const MAX_SEARCH_FILE_BYTES: u64 = 1024 * 1024;
const SEARCH_POLL_MS: u64 = 75;
const SEARCH_DEBOUNCE_MS: u64 = 250;
const FILE_TRANSFER_MIME_TYPES: &[&str] = &[
    "x-special/gnome-copied-files",
    "text/uri-list",
    "text/plain;charset=utf-8",
    "text/plain",
];

type SelectionCallback = Rc<dyn Fn(FileNodePath)>;
type SearchMatchCallback = Rc<dyn Fn(FileNodePath, usize, usize)>;
type PathCallback = Rc<dyn Fn(String, bool)>;
type RunCallback = Rc<dyn Fn(String)>;
type ChatCallback = Rc<dyn Fn(String)>;
type IgnoreCallback = Rc<dyn Fn(String)>;
type ContainerFileActionCallback = Rc<dyn Fn(String, ContainerFileAction)>;
type OpenFailedCallback = Rc<dyn Fn(String)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ContainerFileAction {
    BuildImage,
    ComposeUp,
    ComposePull,
    ComposeRestart,
    ComposeDown,
}

pub(in crate::ui) struct FileBrowser {
    pub(in crate::ui) root: gtk::Box,
    search_panel: SearchPanel,
    tree: Rc<tree_view::TreeView<rows::BrowserListRowKey, rows::BrowserListRowRenderState>>,
    workspace: RefCell<WorkspaceRef>,
    file_access: RefCell<Arc<dyn FileAccess>>,
    git_access: RefCell<Option<Arc<dyn GitAccess>>>,
    desktop_opener: RefCell<Option<Arc<dyn DesktopOpenAccess>>>,
    expanded_dirs: RefCell<HashSet<FileNodePath>>,
    active_folder: RefCell<FileNodePath>,
    selected_node_path: Rc<RefCell<Option<FileNodePath>>>,
    selected_search_match: RefCell<Option<SearchSelectionKey>>,
    search_collapsed_dirs: RefCell<HashSet<FileNodePath>>,
    search_query: Rc<RefCell<String>>,
    search_case_sensitive: Cell<bool>,
    search_whole_word: Cell<bool>,
    search_regex: Cell<bool>,
    search_generation: Rc<Cell<u64>>,
    search_source: RefCell<Option<gtk::glib::SourceId>>,
    last_search_signature: RefCell<Option<SearchSignature>>,
    search_output: RefCell<Option<SearchOutput>>,
    rows_signature: RefCell<Vec<RowSignature>>,
    callbacks: RefCell<Vec<SelectionCallback>>,
    search_match_callbacks: RefCell<Vec<SearchMatchCallback>>,
    terminal_callbacks: RefCell<Vec<PathCallback>>,
    run_terminal_callbacks: RefCell<Vec<RunCallback>>,
    chat_callbacks: RefCell<Vec<ChatCallback>>,
    ignore_callbacks: RefCell<Vec<IgnoreCallback>>,
    container_file_action_callbacks: RefCell<Vec<ContainerFileActionCallback>>,
    open_failed_callbacks: RefCell<Vec<OpenFailedCallback>>,
    changed_file_statuses: RefCell<HashMap<String, String>>,
    tree_rows_cache: RefCell<Option<TreeRowsCache>>,
    tree_directory_cache: RefCell<HashMap<FileNodePath, Vec<BrowserRow>>>,
    tree_directory_loading: RefCell<HashSet<FileNodePath>>,
    tree_directory_load_generation: Rc<Cell<u64>>,
    file_watch_generation: Rc<Cell<u64>>,
    file_watch_signature: RefCell<Option<FileBrowserWatchSignature>>,
    file_watch_subscriptions: RefCell<Vec<FileWatchSubscription>>,
    file_watch_event_source: RefCell<Option<gtk::glib::SourceId>>,
    git_ignore_cache: RefCell<GitIgnoreCache>,
    git_ignore_generation: Rc<Cell<u64>>,
    git_ignore_rules_signature: RefCell<Option<Vec<GitIgnoreRuleFileSignature>>>,
    file_clipboard: RefCell<Option<FileClipboard>>,
    next_transfer_id: Cell<u64>,
    active_transfers: RefCell<HashMap<u64, ActiveTransfer>>,
    internal_drag_paths: RefCell<Option<Vec<FileNodePath>>>,
    drop_target_folder: RefCell<Option<FileNodePath>>,
    drop_hover_generation: Rc<Cell<u64>>,
    active_context_menu: RefCell<Option<gtk::Popover>>,
    pending_new_entry: RefCell<Option<PendingNewEntry>>,
    pending_rename_entry: RefCell<Option<PendingRenameEntry>>,
    deleting_paths: RefCell<HashSet<FileNodePath>>,
    delete_watch_suppression_paths: RefCell<HashSet<FileNodePath>>,
    spellcheck_allowlist: RefCell<SpellcheckAllowlist>,
    last_created_file_extension: RefCell<Option<String>>,
    displayed_rows: RefCell<Vec<BrowserRow>>,
    list_rows: RefCell<Vec<rows::BrowserListRow>>,
    rebuilding: Rc<Cell<bool>>,
    terminal_actions_available: Cell<bool>,
    container_actions_available: Cell<bool>,
}

impl FileBrowser {
    pub(in crate::ui) fn new(
        file_access: Arc<dyn FileAccess>,
        git_access: Option<Arc<dyn GitAccess>>,
    ) -> Rc<Self> {
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(2)
            .margin_bottom(6)
            .build();

        let search_panel = SearchPanel::new("Search workspace");
        search_panel.set_navigation_visible(false);
        header.append(&search_panel.widget());

        let tree = tree_view::TreeView::<
            rows::BrowserListRowKey,
            rows::BrowserListRowRenderState,
        >::builder()
            .vscrollbar_policy(gtk::PolicyType::External)
            .autoscroll_context("file_browser")
            .canvas_scrollbar(true)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .focusable(true)
            .vexpand(true)
            .build();
        root.add_css_class("craic-tree-view");
        root.append(&header);
        root.append(&tree.root);

        let workspace = file_access.workspace();
        let root_node_path = file_access.root();
        let browser = Rc::new(Self {
            root,
            search_panel,
            tree,
            workspace: RefCell::new(workspace),
            file_access: RefCell::new(file_access),
            git_access: RefCell::new(git_access),
            desktop_opener: RefCell::new(None),
            expanded_dirs: RefCell::new(HashSet::new()),
            active_folder: RefCell::new(root_node_path),
            selected_node_path: Rc::new(RefCell::new(None)),
            selected_search_match: RefCell::new(None),
            search_collapsed_dirs: RefCell::new(HashSet::new()),
            search_query: Rc::new(RefCell::new(String::new())),
            search_case_sensitive: Cell::new(false),
            search_whole_word: Cell::new(false),
            search_regex: Cell::new(false),
            search_generation: Rc::new(Cell::new(0)),
            search_source: RefCell::new(None),
            last_search_signature: RefCell::new(None),
            search_output: RefCell::new(None),
            rows_signature: RefCell::new(Vec::new()),
            callbacks: RefCell::new(Vec::new()),
            search_match_callbacks: RefCell::new(Vec::new()),
            terminal_callbacks: RefCell::new(Vec::new()),
            run_terminal_callbacks: RefCell::new(Vec::new()),
            chat_callbacks: RefCell::new(Vec::new()),
            ignore_callbacks: RefCell::new(Vec::new()),
            container_file_action_callbacks: RefCell::new(Vec::new()),
            open_failed_callbacks: RefCell::new(Vec::new()),
            changed_file_statuses: RefCell::new(HashMap::new()),
            tree_rows_cache: RefCell::new(None),
            tree_directory_cache: RefCell::new(HashMap::new()),
            tree_directory_loading: RefCell::new(HashSet::new()),
            tree_directory_load_generation: Rc::new(Cell::new(0)),
            file_watch_generation: Rc::new(Cell::new(0)),
            file_watch_signature: RefCell::new(None),
            file_watch_subscriptions: RefCell::new(Vec::new()),
            file_watch_event_source: RefCell::new(None),
            git_ignore_cache: RefCell::new(GitIgnoreCache::default()),
            git_ignore_generation: Rc::new(Cell::new(0)),
            git_ignore_rules_signature: RefCell::new(None),
            file_clipboard: RefCell::new(None),
            next_transfer_id: Cell::new(1),
            active_transfers: RefCell::new(HashMap::new()),
            internal_drag_paths: RefCell::new(None),
            drop_target_folder: RefCell::new(None),
            drop_hover_generation: Rc::new(Cell::new(0)),
            active_context_menu: RefCell::new(None),
            pending_new_entry: RefCell::new(None),
            pending_rename_entry: RefCell::new(None),
            deleting_paths: RefCell::new(HashSet::new()),
            delete_watch_suppression_paths: RefCell::new(HashSet::new()),
            spellcheck_allowlist: RefCell::new(SpellcheckAllowlist::default()),
            last_created_file_extension: RefCell::new(None),
            displayed_rows: RefCell::new(Vec::new()),
            list_rows: RefCell::new(Vec::new()),
            rebuilding: Rc::new(Cell::new(false)),
            terminal_actions_available: Cell::new(false),
            container_actions_available: Cell::new(false),
        });

        browser.connect_search();
        browser.search_panel.set_key_capture_widget(&browser.root);
        browser.search_panel.install_shortcuts(&browser.root);
        browser.connect_paste();
        browser.connect_browser_rows();
        let initial_file_access = { browser.file_access.borrow().clone() };
        let initial_git_access = { browser.git_access.borrow().clone() };
        browser.refresh(None, initial_file_access, initial_git_access);
        browser
    }

    pub(in crate::ui) fn refresh(
        self: &Rc<Self>,
        changed_files: Option<&[ChangedFile]>,
        file_access: Arc<dyn FileAccess>,
        git_access: Option<Arc<dyn GitAccess>>,
    ) {
        let workspace = file_access.workspace();
        let workspace_changed = *self.workspace.borrow() != workspace;
        let file_access_changed = {
            let current = self.file_access.borrow();
            !Arc::ptr_eq(&current, &file_access)
        };
        if workspace_changed || file_access_changed {
            self.stop_file_watch_scope();
        }
        self.file_access.replace(file_access);
        self.spellcheck_allowlist
            .replace(crate::spellcheck::SpellcheckAllowlist::default());
        let git_availability_changed = self.git_access.borrow().is_some() != git_access.is_some();
        self.git_access.replace(git_access);
        if git_availability_changed {
            self.clear_git_ignore_cache();
            self.git_ignore_rules_signature.borrow_mut().take();
            self.rows_signature.borrow_mut().clear();
        }
        let file_statuses_changed = {
            let mut next_file_statuses = HashMap::new();
            if let Some(files) = changed_files {
                for file in files {
                    insert_changed_path_status(&mut next_file_statuses, &file.path, &file.status);
                }
            }
            let mut file_statuses = self.changed_file_statuses.borrow_mut();
            if *file_statuses == next_file_statuses {
                false
            } else {
                *file_statuses = next_file_statuses;
                true
            }
        };
        if file_statuses_changed {
            if let Some(files) = changed_files {
                self.invalidate_tree_directory_cache_for_changed_files(files);
            }
            self.last_search_signature.borrow_mut().take();
            if !self.search_query.borrow().is_empty() {
                self.search_generation
                    .set(self.search_generation.get().wrapping_add(1));
            }
        }
        if workspace_changed {
            self.workspace.replace(workspace);
            self.clear_git_ignore_cache();
            self.git_ignore_rules_signature.borrow_mut().take();
            self.expanded_dirs.borrow_mut().clear();
            self.active_folder.replace(self.root_node_path());
            self.selected_node_path.borrow_mut().take();
            self.selected_search_match.borrow_mut().take();
            self.pending_new_entry.borrow_mut().take();
            self.pending_rename_entry.borrow_mut().take();
            self.search_collapsed_dirs.borrow_mut().clear();
            self.invalidate_tree_rows_cache();
            self.last_search_signature.borrow_mut().take();
            self.search_output.borrow_mut().take();
            self.search_generation
                .set(self.search_generation.get().wrapping_add(1));
            self.displayed_rows.borrow_mut().clear();
        }
        if self.search_query.borrow().is_empty() {
            self.rebuild_if_changed();
        } else {
            self.start_search();
        }
    }

    pub(super) fn root_node_path(&self) -> FileNodePath {
        self.file_access.borrow().root()
    }

    pub(super) fn node_path(&self, relative: &str) -> FileNodePath {
        node_path_for_relative(&self.root_node_path(), relative)
    }

    pub(in crate::ui) fn set_desktop_opener(
        &self,
        desktop_opener: Option<Arc<dyn DesktopOpenAccess>>,
    ) {
        self.desktop_opener.replace(desktop_opener);
    }

    pub(in crate::ui) fn set_terminal_actions_available(&self, available: bool) {
        self.terminal_actions_available.set(available);
    }

    pub(in crate::ui) fn set_container_actions_available(&self, available: bool) {
        self.container_actions_available.set(available);
    }

    pub(in crate::ui) fn selected_node_path(&self) -> FileNodePath {
        self.selected_node_path
            .borrow()
            .clone()
            .unwrap_or_else(|| self.root_node_path())
    }

    pub(in crate::ui) fn reveal_workspace_path(self: &Rc<Self>, path: &str) {
        if !self.search_query.borrow().is_empty() {
            self.search_panel.close();
        }

        let node_path = self.node_path(path);
        let parent = node_path.parent().unwrap_or_else(|| self.root_node_path());
        self.active_folder.replace(parent.clone());
        expand_parent_folders(&self.expanded_dirs, &parent);
        self.selected_node_path.replace(Some(node_path));
        self.selected_search_match.borrow_mut().take();
        let rows = self.visible_rows();
        let status_signatures = self.changed_file_statuses.borrow();
        let signature = rows_signature(&rows, &status_signatures);
        self.replace_rows(rows, signature, TreeScrollTarget::RevealSelection);
        log::debug!("file browser revealed workspace path={path}");
    }

    pub(in crate::ui) fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub(in crate::ui) fn connect_selected<F>(&self, callback: F)
    where
        F: Fn(FileNodePath) + 'static,
    {
        self.callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_search_match_selected<F>(&self, callback: F)
    where
        F: Fn(FileNodePath, usize, usize) + 'static,
    {
        self.search_match_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_open_terminal_requested<F>(&self, callback: F)
    where
        F: Fn(String, bool) + 'static,
    {
        self.terminal_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_run_in_terminal_requested<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        self.run_terminal_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_add_to_chat_requested<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        self.chat_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_ignore_requested<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        self.ignore_callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_container_file_action_requested<F>(&self, callback: F)
    where
        F: Fn(String, ContainerFileAction) + 'static,
    {
        self.container_file_action_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    pub(in crate::ui) fn connect_open_failed<F>(&self, callback: F)
    where
        F: Fn(String) + 'static,
    {
        self.open_failed_callbacks
            .borrow_mut()
            .push(Rc::new(callback));
    }

    fn connect_paste(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let browser = self.clone();

            move |_, key, _, modifiers| {
                if (browser.pending_new_entry.borrow().is_some()
                    || browser.pending_rename_entry.borrow().is_some())
                    && !browser.search_panel.has_focus()
                {
                    if key == gdk::Key::Escape {
                        browser.cancel_pending_new_entry();
                        browser.cancel_pending_rename();
                        return gtk::glib::Propagation::Stop;
                    }
                    return gtk::glib::Propagation::Proceed;
                }

                let is_paste = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::v | gdk::Key::V);
                if is_paste && !browser.search_panel.has_focus() {
                    browser.paste_clipboard_files();
                    return gtk::glib::Propagation::Stop;
                }

                let is_copy = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::c | gdk::Key::C);
                if is_copy && !browser.search_panel.has_focus() {
                    browser.copy_selected_target(TransferOperation::Copy);
                    return gtk::glib::Propagation::Stop;
                }

                let is_cut = modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                    && matches!(key, gdk::Key::x | gdk::Key::X);
                if is_cut && !browser.search_panel.has_focus() {
                    browser.copy_selected_target(TransferOperation::Move);
                    return gtk::glib::Propagation::Stop;
                }

                if key == gdk::Key::Delete && !browser.search_panel.has_focus() {
                    browser.delete_selected_file();
                    return gtk::glib::Propagation::Stop;
                }

                let is_navigation = !modifiers.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::SUPER_MASK,
                );
                if is_navigation && !browser.has_text_input_focus() {
                    if matches!(
                        key,
                        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
                    ) && browser.toggle_selected_folder()
                    {
                        return gtk::glib::Propagation::Stop;
                    }

                    let direction = match key {
                        gdk::Key::Down => Some(1),
                        gdk::Key::Up => Some(-1),
                        _ => None,
                    };
                    if let Some(direction) = direction {
                        browser.select_relative_row(direction);
                        return gtk::glib::Propagation::Stop;
                    }
                }

                gtk::glib::Propagation::Proceed
            }
        });
        self.root.add_controller(keys);
    }

    pub(super) fn toggle_dir(self: &Rc<Self>, path: &FileNodePath) {
        if !self.search_query.borrow().is_empty() {
            {
                let mut collapsed = self.search_collapsed_dirs.borrow_mut();
                if collapsed.contains(path) {
                    collapsed.remove(path);
                } else {
                    collapsed.insert(path.clone());
                }
            }
            self.rebuild_search_result_rows_from_cache();
            return;
        }
        self.active_folder.replace(path.clone());
        {
            let mut expanded = self.expanded_dirs.borrow_mut();
            if expanded.contains(path) {
                expanded.remove(path);
            } else if !path.is_root() {
                expanded.insert(path.clone());
            }
        }
        self.rebuild();
    }

    fn select_relative_row(self: &Rc<Self>, direction: i32) {
        let rows = self.list_rows.borrow();
        if rows.is_empty() || direction == 0 {
            return;
        }

        let selected = self.selected_node_path.borrow().clone();
        let selected_search_match = self.selected_search_match.borrow().clone();
        let current_index = rows.iter().position(|row| {
            row_matches_selection(row, selected.as_ref(), selected_search_match.as_ref())
        });

        let mut index = match current_index {
            Some(index) => index as i32 + direction,
            None if direction > 0 => 0,
            None => rows.len() as i32 - 1,
        };

        while index >= 0 && (index as usize) < rows.len() {
            let row = rows[index as usize].clone();
            if row_is_selectable(&row) {
                drop(rows);
                self.select_browser_row(&row);
                self.scroll_row_into_view(&row, index as usize);
                self.focus_selected_row();
                return;
            }
            index += direction;
        }
    }

    fn select_browser_row(self: &Rc<Self>, row: &rows::BrowserListRow) {
        match row {
            rows::BrowserListRow::Tree(row) => {
                self.active_folder
                    .replace(if row.tree_role == tree::TreeRowRole::Branch {
                        row.node_path.clone()
                    } else {
                        row.node_path
                            .parent()
                            .unwrap_or_else(|| self.root_node_path())
                    });
                self.set_selected_node_path(Some(row.node_path.clone()));
            }
            rows::BrowserListRow::NewEntry(_)
            | rows::BrowserListRow::RenameEntry(_)
            | rows::BrowserListRow::Loading(_) => {}
            rows::BrowserListRow::Search(search_match) => {
                self.set_selected_search_match(search_match.clone());
            }
            rows::BrowserListRow::Status(_) | rows::BrowserListRow::RootGap => {}
        }
    }

    fn toggle_selected_folder(self: &Rc<Self>) -> bool {
        let selected = self.selected_node_path.borrow().clone();
        let selected_search_match = self.selected_search_match.borrow().clone();
        let selected_row = self
            .list_rows
            .borrow()
            .iter()
            .find(|row| {
                row_matches_selection(row, selected.as_ref(), selected_search_match.as_ref())
            })
            .cloned();

        let Some(rows::BrowserListRow::Tree(row)) = selected_row else {
            return false;
        };
        if row.tree_role != tree::TreeRowRole::Branch {
            return false;
        }

        self.active_folder.replace(row.node_path.clone());
        self.toggle_dir(&row.node_path);
        true
    }

    pub(super) fn scroll_selected_row_into_view(&self) {
        let Some(key) = self.selected_row_key() else {
            return;
        };
        self.tree.scroll_row_into_view(&key);
    }

    fn scroll_row_into_view(&self, row: &rows::BrowserListRow, row_index: usize) {
        self.tree
            .scroll_row_into_view(&rows::browser_list_row_key(row, row_index));
    }

    fn focus_selected_row(&self) {
        let Some(key) = self.selected_row_key() else {
            return;
        };
        self.tree.focus_row(&key);
    }

    pub(super) fn focus_browser_shell(&self) {
        let adjustment = self.tree.scroller.vadjustment();
        let scroll_y = adjustment.value();
        let _ = self.root.grab_focus();
        set_scroll_value(&adjustment, scroll_y);
    }

    pub(super) fn focus_pending_new_entry(&self) {
        let Some(key) = self.pending_new_entry_row_key() else {
            return;
        };

        self.tree.scroll_row_into_view(&key);
        self.tree
            .focus_edit_row(&key, tree_view::EditFocusPlacement::Start);
    }

    pub(super) fn focus_pending_rename_entry(&self) {
        let Some(key) = self.pending_rename_entry_row_key() else {
            return;
        };

        self.tree.scroll_row_into_view(&key);
        log::debug!("file browser rename focus key={key:?}");
        self.tree
            .focus_edit_row(&key, tree_view::EditFocusPlacement::SelectBeforeFirstDot);
    }

    fn selected_row_key(&self) -> Option<rows::BrowserListRowKey> {
        let selected = self.selected_node_path.borrow().clone();
        let selected_search_match = self.selected_search_match.borrow().clone();
        self.list_rows
            .borrow()
            .iter()
            .enumerate()
            .find(|(_, row)| {
                row_matches_selection(row, selected.as_ref(), selected_search_match.as_ref())
            })
            .map(|(index, row)| rows::browser_list_row_key(row, index))
    }

    fn pending_new_entry_row_key(&self) -> Option<rows::BrowserListRowKey> {
        self.list_rows
            .borrow()
            .iter()
            .enumerate()
            .find(|(_, row)| matches!(row, rows::BrowserListRow::NewEntry(_)))
            .map(|(index, row)| rows::browser_list_row_key(row, index))
    }

    fn pending_rename_entry_row_key(&self) -> Option<rows::BrowserListRowKey> {
        self.list_rows
            .borrow()
            .iter()
            .enumerate()
            .find(|(_, row)| matches!(row, rows::BrowserListRow::RenameEntry(_)))
            .map(|(index, row)| rows::browser_list_row_key(row, index))
    }

    fn file_view_has_focus(&self) -> bool {
        self.root.has_focus() || self.tree.has_row_focus()
    }

    fn has_text_input_focus(&self) -> bool {
        if self.search_panel.has_focus() {
            return true;
        }

        if self.pending_new_entry.borrow().is_none() && self.pending_rename_entry.borrow().is_none()
        {
            return false;
        }

        self.tree.has_edit_focus()
    }

    pub(super) fn set_selected_node_path(self: &Rc<Self>, selected: Option<FileNodePath>) {
        if *self.selected_node_path.borrow() == selected
            && self.selected_search_match.borrow().is_none()
        {
            return;
        }
        let callback_path = selected.clone().unwrap_or_else(|| self.root_node_path());
        self.selected_node_path.replace(selected);
        self.selected_search_match.borrow_mut().take();
        self.refresh_browser_row_state();
        let callbacks = self.callbacks.borrow().clone();
        for callback in callbacks {
            callback(callback_path.clone());
        }
        self.focus_selected_row();
    }

    pub(super) fn rebuild_if_changed(self: &Rc<Self>) {
        let rows = self.visible_rows();
        let status_signatures = self.changed_file_statuses.borrow();
        let signature = rows_signature(&rows, &status_signatures);
        if *self.rows_signature.borrow() == signature {
            return;
        }
        self.replace_rows(
            rows,
            signature,
            TreeScrollTarget::Preserve(self.tree.scroller.vadjustment().value()),
        );
    }

    pub(super) fn rebuild(self: &Rc<Self>) {
        let rows = self.visible_rows();
        let status_signatures = self.changed_file_statuses.borrow();
        let signature = rows_signature(&rows, &status_signatures);
        self.replace_rows(
            rows,
            signature,
            TreeScrollTarget::Preserve(self.tree.scroller.vadjustment().value()),
        );
    }

    fn replace_rows(
        self: &Rc<Self>,
        rows: Vec<BrowserRow>,
        signature: Vec<RowSignature>,
        scroll_target: TreeScrollTarget,
    ) {
        self.rebuilding.set(true);

        let rows = rows.into_iter().take(MAX_TREE_ROWS).collect::<Vec<_>>();
        self.displayed_rows.replace(rows.clone());
        self.update_file_watch_scope(&rows);
        self.set_browser_rows(self.browser_list_rows_with_pending_new_entry(rows));
        self.rows_signature.replace(signature);

        self.rebuilding.set(false);
        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            browser.apply_tree_scroll(scroll_target);
        });
    }

    fn gap_context_folder(&self, list_y: f64) -> FileNodePath {
        if let Some(folder) = self.folder_for_gap_at_y(list_y) {
            return folder;
        }

        self.tree
            .last_sticky_row()
            .and_then(|row| match row.state.to_row() {
                rows::BrowserListRow::Tree(row) => Some(row.node_path),
                _ => None,
            })
            .unwrap_or_else(|| self.active_folder.borrow().clone())
    }

    fn apply_tree_scroll(self: &Rc<Self>, target: TreeScrollTarget) {
        match target {
            TreeScrollTarget::Preserve(scroll_y) => {
                let adjustment = self.tree.scroller.vadjustment();
                set_scroll_value(&adjustment, scroll_y);
                self.restore_scroll_soon(scroll_y);
            }
            TreeScrollTarget::RevealSelection => {
                self.scroll_selected_row_into_view();
                self.focus_selected_row();
                self.tree.update_sticky_rows();
                self.tree.list.queue_draw();
            }
        }
    }

    fn restore_scroll_soon(self: &Rc<Self>, scroll_y: f64) {
        let adjustment = self.tree.scroller.vadjustment();
        let browser = self.clone();
        gtk::glib::idle_add_local_once(move || {
            set_scroll_value(&adjustment, scroll_y);
            browser.tree.update_sticky_rows();
            browser.tree.list.queue_draw();
        });
    }

    pub(super) fn show_error(&self, heading: &str, message: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(message));
        dialog.add_response("ok", "OK");
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        dialog.present(Some(&self.root));
    }

    fn notify_open_message(&self, message: &str) {
        let callbacks = self.open_failed_callbacks.borrow().clone();
        for callback in callbacks {
            callback(message.to_string());
        }
    }

    fn target_for_node_path(&self, node_path: FileNodePath) -> BrowserTarget {
        match self.file_access.borrow().info(&node_path) {
            Ok(info) => BrowserTarget::from_info(info),
            Err(err) => {
                log::debug!(
                    "file browser target info unavailable path={} err={err}",
                    node_path.display()
                );
                BrowserTarget::fallback(node_path)
            }
        }
    }

    fn can_paste_into_target(&self, target: &BrowserTarget) -> bool {
        if self.file_clipboard.borrow().is_none() {
            return false;
        }
        let folder = if target.is_dir {
            target.node_path.clone()
        } else {
            target
                .node_path
                .parent()
                .unwrap_or_else(|| self.root_node_path())
        };
        self.file_access
            .borrow()
            .info(&folder)
            .is_ok_and(|info| info.capabilities.creatable)
    }
}

#[derive(Clone)]
struct BrowserTarget {
    node_path: FileNodePath,
    path: String,
    is_dir: bool,
    executable: bool,
    capabilities: RowCapabilities,
}

impl BrowserTarget {
    fn from_row(row: &BrowserRow) -> Self {
        Self {
            node_path: row.node_path.clone(),
            path: row.path.clone(),
            is_dir: row.is_dir,
            executable: row.executable,
            capabilities: row.capabilities,
        }
    }

    fn from_info(info: FileNodeInfo) -> Self {
        let mut kind = info.kind;
        if matches!(kind, FileNodeKind::File)
            && let Some(format) = crate::system::ArchiveFormat::from_name(&info.display_name)
        {
            kind = FileNodeKind::Archive { format };
        }
        Self {
            path: info.path.display(),
            is_dir: kind == FileNodeKind::Directory,
            executable: info.mode.is_some_and(|mode| mode & 0o111 != 0),
            capabilities: RowCapabilities::from(&info.capabilities),
            node_path: info.path,
        }
    }

    fn fallback(node_path: FileNodePath) -> Self {
        Self {
            path: node_path.display(),
            is_dir: node_path.is_root(),
            executable: false,
            capabilities: RowCapabilities::default(),
            node_path,
        }
    }

    fn is_root(&self) -> bool {
        self.node_path.is_root()
    }

    fn container_actions(&self) -> Vec<ContainerFileAction> {
        if self.is_dir || !self.capabilities.native {
            return Vec::new();
        }

        if is_dockerfile_path(&self.path) {
            return vec![ContainerFileAction::BuildImage];
        }

        if is_compose_file_path(&self.path) {
            return vec![
                ContainerFileAction::ComposeUp,
                ContainerFileAction::ComposePull,
                ContainerFileAction::ComposeRestart,
                ContainerFileAction::ComposeDown,
            ];
        }

        Vec::new()
    }
}

fn is_dockerfile_path(path: &str) -> bool {
    let name = file_name(path);
    name == "Dockerfile" || name.starts_with("Dockerfile.") || name == "Containerfile"
}

fn is_compose_file_path(path: &str) -> bool {
    let name = file_name(path).to_ascii_lowercase();
    is_compose_file_name(&name)
}

fn is_compose_file_name(name: &str) -> bool {
    matches!(name, "compose.yml" | "compose.yaml")
        || (name.contains("docker-compose") && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

enum TreeScrollTarget {
    Preserve(f64),
    RevealSelection,
}

pub(super) fn should_skip(name: &str) -> bool {
    skipped_names().contains(&name)
}

pub(super) fn skipped_names() -> [&'static str; 6] {
    [".git", "target", "node_modules", ".next", "dist", "build"]
}

fn show_row_context_menu<W: IsA<gtk::Widget>>(
    browser: &Rc<FileBrowser>,
    parent: &W,
    target: BrowserTarget,
    x: f64,
    y: f64,
) {
    let actions = gio::SimpleActionGroup::new();
    let parent_window = parent.root().and_downcast::<gtk::Window>();
    add_menu_action(&actions, "open", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.open_target(&target)
    });
    let new_file = add_menu_action(&actions, "new-file", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.create_file_in_folder(&target.node_path)
    });
    new_file.set_enabled(target.is_dir && target.capabilities.creatable);
    let new_folder = add_menu_action(&actions, "new-folder", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.create_folder_in_folder(&target.node_path)
    });
    new_folder.set_enabled(target.is_dir && target.capabilities.creatable);
    let open_external = add_menu_action(&actions, "open-external", {
        let browser = browser.clone();
        let target = target.clone();
        let parent_window = parent_window.clone();
        move || {
            browser.open_external(
                &target,
                DesktopOpenActivation::from_parent(parent_window.as_ref()),
            )
        }
    });
    let desktop_open_available =
        browser.desktop_opener.borrow().is_some() && target.capabilities.native;
    open_external.set_enabled(desktop_open_available && target.capabilities.open_external);
    let open_containing_folder = add_menu_action(&actions, "open-containing-folder", {
        let browser = browser.clone();
        let target = target.clone();
        let parent_window = parent_window.clone();
        move || {
            browser.open_containing_folder(
                &target,
                DesktopOpenActivation::from_parent(parent_window.as_ref()),
            )
        }
    });
    open_containing_folder.set_enabled(desktop_open_available && target.capabilities.reveal);
    let open_terminal = add_menu_action(&actions, "open-terminal", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.open_terminal(&target)
    });
    open_terminal.set_enabled(target.capabilities.native);
    let run_terminal = add_menu_action(&actions, "run-terminal", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_in_terminal(&target)
    });
    run_terminal.set_enabled(!target.is_dir && target.executable && target.capabilities.native);
    let add_to_chat = add_menu_action(&actions, "add-to-chat", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.add_to_chat(&target)
    });
    add_to_chat.set_enabled(!target.is_dir && target.capabilities.readable);
    let build_image = add_menu_action(&actions, "build-image", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_container_file_action(&target, ContainerFileAction::BuildImage)
    });
    build_image.set_enabled(
        target
            .container_actions()
            .contains(&ContainerFileAction::BuildImage),
    );
    let compose_up = add_menu_action(&actions, "compose-up", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_container_file_action(&target, ContainerFileAction::ComposeUp)
    });
    compose_up.set_enabled(
        target
            .container_actions()
            .contains(&ContainerFileAction::ComposeUp),
    );
    let compose_pull = add_menu_action(&actions, "compose-pull", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_container_file_action(&target, ContainerFileAction::ComposePull)
    });
    compose_pull.set_enabled(
        target
            .container_actions()
            .contains(&ContainerFileAction::ComposePull),
    );
    let compose_down = add_menu_action(&actions, "compose-down", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_container_file_action(&target, ContainerFileAction::ComposeDown)
    });
    let compose_restart = add_menu_action(&actions, "compose-restart", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.run_container_file_action(&target, ContainerFileAction::ComposeRestart)
    });
    compose_restart.set_enabled(
        target
            .container_actions()
            .contains(&ContainerFileAction::ComposeRestart),
    );
    compose_down.set_enabled(
        target
            .container_actions()
            .contains(&ContainerFileAction::ComposeDown),
    );
    let cut = add_menu_action(&actions, "cut", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.copy_target(&target, TransferOperation::Move)
    });
    cut.set_enabled(!target.is_root() && target.capabilities.movable && target.capabilities.native);
    let copy = add_menu_action(&actions, "copy", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.copy_target(&target, TransferOperation::Copy)
    });
    copy.set_enabled(!target.is_root() && target.capabilities.readable);
    let paste = add_menu_action(&actions, "paste", {
        let browser = browser.clone();
        let target = target.clone();
        move || {
            let folder = browser.target_paste_folder(&target);
            browser.paste_into_folder(folder);
        }
    });
    paste.set_enabled(browser.can_paste_into_target(&target));
    let copy_path = add_menu_action(&actions, "copy-path", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.copy_absolute_path(&target)
    });
    copy_path.set_enabled(target.capabilities.native);
    add_menu_action(&actions, "copy-relative-path", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.copy_relative_path(&target)
    });
    let ignore_pattern = context_menu::add_string_menu_action(&actions, "ignore-pattern", {
        let browser = browser.clone();
        move |pattern| browser.add_to_ignore(pattern)
    });
    ignore_pattern
        .set_enabled(target.capabilities.native && !browser.ignore_callbacks.borrow().is_empty());
    let rename = add_menu_action(&actions, "rename", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.rename_target(&target)
    });
    rename.set_enabled(!target.is_root() && target.capabilities.movable);
    let delete = add_menu_action(&actions, "delete", {
        let browser = browser.clone();
        let target = target.clone();
        move || browser.delete_target(target.clone())
    });
    delete.set_enabled(!target.is_root() && target.capabilities.deletable);

    let terminal_available = browser.terminal_actions_available.get() && target.capabilities.native;
    let container_actions_available =
        browser.container_actions_available.get() && target.capabilities.native;
    menu::repository_row_menu(&target, terminal_available, container_actions_available).popup(
        parent,
        x,
        y,
        &actions,
        &browser.active_context_menu,
    );
}

fn add_menu_action<F>(group: &gio::SimpleActionGroup, name: &str, activate: F) -> gio::SimpleAction
where
    F: Fn() + 'static,
{
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| activate());
    group.add_action(&action);
    action
}

pub(super) fn parent_folder(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn expand_parent_folders(expanded_dirs: &RefCell<HashSet<FileNodePath>>, folder: &FileNodePath) {
    let mut parents = Vec::new();
    let mut current = Some(folder.clone());
    while let Some(path) = current {
        if !path.is_root() {
            parents.push(path.clone());
        }
        current = path.parent();
    }

    let mut expanded_dirs = expanded_dirs.borrow_mut();
    for parent in parents {
        expanded_dirs.insert(parent);
    }
}

fn row_matches_selection(
    row: &rows::BrowserListRow,
    selected: Option<&FileNodePath>,
    selected_search_match: Option<&SearchSelectionKey>,
) -> bool {
    match row {
        rows::BrowserListRow::Tree(row) => {
            selected_search_match.is_none() && selected == Some(&row.node_path)
        }
        rows::BrowserListRow::NewEntry(_)
        | rows::BrowserListRow::RenameEntry(_)
        | rows::BrowserListRow::Loading(_) => false,
        rows::BrowserListRow::Search(search_match) => {
            selected_search_match == Some(&search_match.selection_key())
        }
        rows::BrowserListRow::Status(_) | rows::BrowserListRow::RootGap => false,
    }
}

fn row_is_selectable(row: &rows::BrowserListRow) -> bool {
    !matches!(
        row,
        rows::BrowserListRow::NewEntry(_)
            | rows::BrowserListRow::RenameEntry(_)
            | rows::BrowserListRow::Loading(_)
            | rows::BrowserListRow::Status(_)
            | rows::BrowserListRow::RootGap
    )
}

pub(super) fn set_scroll_value(adjustment: &gtk::Adjustment, value: f64) {
    adjustment.set_value(
        value
            .min(adjustment.upper() - adjustment.page_size())
            .max(adjustment.lower()),
    );
}

pub(super) fn node_path_for_relative(root: &FileNodePath, relative: &str) -> FileNodePath {
    let mut path = root.clone();
    for segment in relative.split('/').filter(|segment| !segment.is_empty()) {
        path = path.join_child(segment);
    }
    path
}

pub(in crate::ui) fn file_name(path: &str) -> &str {
    path.rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}
