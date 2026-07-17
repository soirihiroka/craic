use super::super::{canvas_scroll, widgets};
use crate::git::{self, Commit, GitRepoHandle, RepositorySnapshot};
use crate::ui::components::search::SearchPanel;
use adw::prelude::*;
use gtk::glib;
use gtk::glib::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gio, glib::object::Cast};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

const HISTORY_PAGE_SIZE: usize = 32;
const HISTORY_APPEND_CHUNK_SIZE: usize = 8;
const HISTORY_PREFETCH_DISTANCE: f64 = 360.0;
const COMMIT_ROW_DATA_KEY: &str = "craic-history-commit-row";

type HistoryContextCallback = dyn Fn(&gtk::Box, String, f64, f64, u32) + 'static;

#[derive(Clone)]
pub struct HistoryList {
    pub root: gtk::Box,
    model: gio::ListStore,
    selection: gtk::SingleSelection,
    scroller: gtk::ScrolledWindow,
    history_stack: gtk::Stack,
    empty_spinner: adw::Spinner,
    empty_status_label: gtk::Label,
    loading_spinner: adw::Spinner,
    status_label: gtk::Label,
    search_panel: SearchPanel,
    footer: adw::Clamp,
    state: Rc<HistoryState>,
}

#[derive(Default)]
struct HistoryState {
    workspace_key: RefCell<Option<String>>,
    git_handle: RefCell<Option<Arc<GitRepoHandle>>>,
    history_head: RefCell<Option<String>>,
    cursor: RefCell<Option<String>>,
    search_query: RefCell<String>,
    loading: Cell<bool>,
    has_more: Cell<bool>,
    generation: Cell<u64>,
    context_requested: RefCell<Option<Rc<HistoryContextCallback>>>,
}

impl HistoryList {
    pub fn new() -> Self {
        let model = gio::ListStore::new::<CommitItem>();
        let selection = gtk::SingleSelection::new(Some(model.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let state = Rc::new(HistoryState::default());
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let model = model.clone();
            let selection = selection.clone();
            let state = state.clone();

            move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let row = commit_row();
                install_commit_context_menu(&row.root, &model, &selection, state.clone());
                item.set_child(Some(&row.root));
                unsafe {
                    item.set_data(COMMIT_ROW_DATA_KEY, row);
                }
            }
        });
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(commit) = item.item().and_downcast::<CommitItem>() else {
                return;
            };
            let Some(row) = commit_row_from_item(item) else {
                return;
            };
            bind_commit_row(&row, &commit);
        });

        let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        view.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();

        let autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let history_overlay = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        history_overlay.set_child(Some(&scroller));
        history_overlay.add_overlay(&autoscroll_marker);
        canvas_scroll::install_scrolled_window_middle_autoscroll(
            &scroller,
            &autoscroll_marker,
            canvas_scroll::AutoscrollAxes::Vertical,
            "git_history",
        );

        let empty_spinner = adw::Spinner::new();
        empty_spinner.set_visible(false);
        empty_spinner.set_size_request(24, 24);

        let empty_status_label = widgets::muted("Loading commits...");
        empty_status_label.set_halign(gtk::Align::Center);
        empty_status_label.set_justify(gtk::Justification::Center);

        let empty_state = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        empty_state.append(&empty_spinner);
        empty_state.append(&empty_status_label);

        let history_stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        history_stack.add_named(&history_overlay, Some("history"));
        history_stack.add_named(&empty_state, Some("empty"));
        history_stack.set_visible_child_name("empty");

        let loading_spinner = adw::Spinner::new();
        loading_spinner.set_visible(false);
        loading_spinner.set_valign(gtk::Align::Center);
        loading_spinner.set_size_request(16, 16);

        let status_label = widgets::muted("");
        status_label.set_visible(false);
        status_label.set_hexpand(false);
        status_label.set_wrap(false);
        status_label.set_lines(1);
        status_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let loading_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(20)
            .margin_end(20)
            .height_request(40)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        status_label.set_valign(gtk::Align::Center);
        status_label.set_hexpand(true);
        status_label.set_halign(gtk::Align::Start);
        loading_bar.append(&loading_spinner);
        loading_bar.append(&status_label);

        let footer = adw::Clamp::builder()
            .maximum_size(640)
            .tightening_threshold(420)
            .halign(gtk::Align::Center)
            .hexpand(true)
            .child(&loading_bar)
            .build();
        footer.set_visible(false);
        let search_panel = SearchPanel::new("Search commits");
        search_panel.set_options_visible(false);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        root.append(&search_panel.widget());
        root.append(&history_stack);
        root.append(&footer);

        let history = Self {
            root,
            model,
            selection,
            scroller,
            history_stack,
            empty_spinner,
            empty_status_label,
            loading_spinner,
            status_label,
            search_panel,
            footer,
            state,
        };
        history.search_panel.set_key_capture_widget(&history.root);
        history.search_panel.install_shortcuts(&history.root);
        history.connect_search();
        history.connect_auto_paging();
        history.update_footer();
        history
    }

    pub fn connect_selected<F>(&self, callback: F)
    where
        F: Fn() + 'static,
    {
        self.selection.connect_selected_notify(move |_| callback());
    }

    pub fn connect_context_requested<F>(&self, callback: F)
    where
        F: Fn(&gtk::Box, String, f64, f64, u32) + 'static,
    {
        *self.state.context_requested.borrow_mut() = Some(Rc::new(callback));
    }

    fn connect_auto_paging(&self) {
        self.scroller.connect_edge_reached({
            let history = self.clone();

            move |_, position| {
                if position == gtk::PositionType::Bottom {
                    history.load_next_page(false);
                }
            }
        });

        self.scroller.vadjustment().connect_value_changed({
            let history = self.clone();

            move |adjustment| {
                if adjustment_is_near_bottom(adjustment) {
                    history.load_next_page(false);
                }
            }
        });

        self.scroller.vadjustment().connect_upper_notify({
            let history = self.clone();

            move |adjustment| history.fill_visible_page(adjustment)
        });

        self.scroller.vadjustment().connect_page_size_notify({
            let history = self.clone();

            move |adjustment| history.fill_visible_page(adjustment)
        });
    }

    fn connect_search(&self) {
        self.search_panel.connect_query_changed({
            let history = self.clone();

            move |query| history.update_search_query(query.trim().to_string())
        });
        self.search_panel.connect_closed({
            let history = self.clone();

            move || history.update_search_query(String::new())
        });
        self.search_panel.connect_previous({
            let history = self.clone();

            move || history.select_relative(-1)
        });
        self.search_panel.connect_next({
            let history = self.clone();

            move || history.select_relative(1)
        });
    }

    fn update_search_query(&self, query: String) {
        if *self.state.search_query.borrow() == query {
            return;
        }
        self.state.search_query.replace(query.clone());
        self.state.cursor.borrow_mut().take();
        self.state.loading.set(false);
        self.state
            .has_more
            .set(self.state.history_head.borrow().is_some());
        self.state
            .generation
            .set(self.state.generation.get().wrapping_add(1));
        self.model.remove_all();
        self.selection.unselect_all();
        log::debug!("history search updated query_len={}", query.len());
        self.update_footer();
        self.load_next_page(false);
    }

    pub fn update(
        &self,
        snapshot: &RepositorySnapshot,
        workspace_key: String,
        git_handle: Option<Arc<GitRepoHandle>>,
    ) {
        let workspace_changed = self.state.workspace_key.borrow().as_ref() != Some(&workspace_key);
        let head_changed = *self.state.history_head.borrow() != snapshot.history_head;
        self.state.git_handle.replace(git_handle);

        if !workspace_changed && !head_changed {
            return;
        }

        *self.state.workspace_key.borrow_mut() = Some(workspace_key);
        *self.state.history_head.borrow_mut() = snapshot.history_head.clone();
        self.state.cursor.borrow_mut().take();
        self.state.loading.set(false);
        self.state.has_more.set(snapshot.history_head.is_some());
        self.state
            .generation
            .set(self.state.generation.get().wrapping_add(1));
        self.model.remove_all();
        self.selection.unselect_all();
        self.update_footer();
    }

    pub fn ensure_loaded(&self) {
        if self.model.n_items() == 0 && self.state.history_head.borrow().is_some() {
            self.load_next_page(false);
        }
    }

    pub fn toggle_search(&self) {
        self.search_panel.toggle();
    }

    pub fn load_next_page(&self, select_first: bool) {
        if self.state.loading.get() || !self.state.has_more.get() {
            return;
        }

        let Some(workspace_key) = self.state.workspace_key.borrow().clone() else {
            return;
        };
        let Some(git_handle) = self.state.git_handle.borrow().clone() else {
            self.finish_page_load(
                Err("Git is unavailable for this workspace.".to_string()),
                select_first,
            );
            return;
        };
        let cursor = self.state.cursor.borrow().clone();
        let search_query = self.state.search_query.borrow().clone();
        let generation = self.state.generation.get();
        let (sender, receiver) = mpsc::channel();

        self.state.loading.set(true);
        self.update_footer();

        log::debug!(
            "history page load start workspace={} cursor={:?} query_len={}",
            workspace_key,
            cursor.as_deref().map(short_hash),
            search_query.len()
        );
        if search_query.is_empty() {
            git_handle.commit_page(
                cursor.as_deref(),
                HISTORY_PAGE_SIZE,
                Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            );
        } else {
            git_handle.commit_search_page(
                &search_query,
                cursor.as_deref(),
                HISTORY_PAGE_SIZE,
                Box::new(move |result| {
                    let _ = sender.send(result);
                }),
            );
        }

        gtk::glib::timeout_add_local(Duration::from_millis(75), {
            let history = self.clone();

            move || match receiver.try_recv() {
                Ok(result) => {
                    if history.state.generation.get() != generation {
                        return gtk::glib::ControlFlow::Break;
                    }
                    history.finish_page_load(result, select_first);
                    gtk::glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    if history.state.generation.get() == generation {
                        history.finish_page_load(
                            Err("History loading did not return a result.".to_string()),
                            select_first,
                        );
                    }
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    }

    pub fn selected_commit_hash(&self) -> Option<String> {
        self.selection
            .selected_item()
            .and_downcast::<CommitItem>()
            .map(|commit| commit.hash())
            .filter(|hash| !hash.is_empty())
    }

    pub fn select_first_commit(&self) {
        if self.selection.selected_item().is_some() || self.model.n_items() == 0 {
            return;
        }

        self.selection.set_selected(0);
    }

    pub fn clear(&self) {
        self.state.workspace_key.borrow_mut().take();
        self.state.git_handle.borrow_mut().take();
        self.state.history_head.borrow_mut().take();
        self.state.cursor.borrow_mut().take();
        self.state.search_query.borrow_mut().clear();
        self.state.loading.set(false);
        self.state.has_more.set(false);
        self.state
            .generation
            .set(self.state.generation.get().wrapping_add(1));
        self.model.remove_all();
        self.selection.unselect_all();
        self.search_panel.set_query("", false);
        self.update_footer();
    }

    fn select_relative(&self, direction: i32) {
        let count = self.model.n_items();
        if count == 0 || direction == 0 {
            return;
        }
        let current = self.selection.selected();
        let next = if current == gtk::INVALID_LIST_POSITION {
            if direction > 0 { 0 } else { count - 1 }
        } else {
            (current as i32 + direction).clamp(0, count.saturating_sub(1) as i32) as u32
        };
        self.selection.set_selected(next);
    }

    fn finish_page_load(&self, result: Result<git::CommitPage, String>, select_first: bool) {
        match result {
            Ok(page) => self.append_page_commits(page, select_first),
            Err(err) => {
                self.state.loading.set(false);
                self.state.has_more.set(false);
                let message = format!("Failed to load history: {err}");
                self.loading_spinner.set_visible(false);
                if self.model.n_items() == 0 {
                    self.history_stack.set_visible_child_name("empty");
                    self.empty_spinner.set_visible(false);
                    self.empty_status_label.set_label(&message);
                    self.empty_status_label.set_visible(true);
                    self.status_label.set_visible(false);
                    self.footer.set_visible(false);
                } else {
                    self.status_label.set_label(&message);
                    self.status_label.set_visible(true);
                    self.footer.set_visible(false);
                }
            }
        }
    }

    fn append_page_commits(&self, page: git::CommitPage, select_first: bool) {
        let generation = self.state.generation.get();
        let last_hash = page.commits.last().map(|commit| commit.hash.clone());
        let has_more = page.has_more;
        let mut commits = VecDeque::from(page.commits);

        if commits.is_empty() {
            self.state.has_more.set(false);
            self.state.loading.set(false);
            self.update_footer();
            return;
        }

        gtk::glib::idle_add_local({
            let history = self.clone();
            let mut selected_first = false;

            move || {
                if history.state.generation.get() != generation {
                    return gtk::glib::ControlFlow::Break;
                }

                for _ in 0..HISTORY_APPEND_CHUNK_SIZE {
                    let Some(commit) = commits.pop_front() else {
                        break;
                    };
                    history.model.append(&CommitItem::new(&commit));
                }

                history.update_footer();
                if select_first && !selected_first {
                    history.select_first_commit();
                    selected_first = true;
                }

                if commits.is_empty() {
                    if let Some(hash) = last_hash.clone() {
                        *history.state.cursor.borrow_mut() = Some(hash);
                    }
                    history.state.has_more.set(has_more);
                    history.state.loading.set(false);
                    history.update_footer();
                    history.schedule_visible_page_fill();
                    gtk::glib::ControlFlow::Break
                } else {
                    gtk::glib::ControlFlow::Continue
                }
            }
        });
    }

    fn schedule_visible_page_fill(&self) {
        let history = self.clone();
        gtk::glib::idle_add_local_once(move || {
            let adjustment = history.scroller.vadjustment();
            history.fill_visible_page(&adjustment);

            gtk::glib::timeout_add_local_once(Duration::from_millis(50), move || {
                let adjustment = history.scroller.vadjustment();
                history.fill_visible_page(&adjustment);
            });
        });
    }

    fn fill_visible_page(&self, adjustment: &gtk::Adjustment) {
        if self.state.loading.get() || !self.state.has_more.get() {
            return;
        }

        if adjustment.upper() <= adjustment.page_size() + 1.0
            || adjustment_is_near_bottom(adjustment)
        {
            self.load_next_page(false);
        }
    }

    fn update_footer(&self) {
        let search_query = self.state.search_query.borrow().clone();
        let search_active = !search_query.is_empty();
        if self.model.n_items() == 0 {
            self.history_stack.set_visible_child_name("empty");
            self.loading_spinner.set_visible(false);
            self.status_label.set_visible(false);
            self.footer.set_visible(false);

            let show_spinner = self.state.history_head.borrow().is_some()
                && (self.state.loading.get() || self.state.has_more.get());
            self.empty_spinner.set_visible(show_spinner);
            self.empty_status_label
                .set_label(if show_spinner && search_active {
                    "Searching commits..."
                } else if show_spinner {
                    "Loading commits..."
                } else if search_active {
                    "No matching commits."
                } else {
                    "History is empty."
                });
            self.empty_status_label.set_visible(true);
            return;
        }

        self.history_stack.set_visible_child_name("history");
        self.empty_spinner.set_visible(false);
        self.empty_status_label.set_visible(false);

        if self.state.loading.get() {
            self.loading_spinner.set_visible(true);
            self.status_label.set_label(if search_active {
                "Searching more commits..."
            } else {
                "Loading more commits..."
            });
            self.status_label.set_visible(true);
            self.footer.set_visible(true);
            return;
        }

        self.loading_spinner.set_visible(false);
        if search_active {
            let status = if self.state.has_more.get() {
                format!("{} loaded, more available", self.model.n_items())
            } else {
                format!("{} matches", self.model.n_items())
            };
            self.status_label.set_label(&status);
            self.status_label.set_visible(true);
            self.footer.set_visible(true);
        } else {
            self.status_label.set_visible(false);
            self.footer.set_visible(false);
        }
    }
}

fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

fn adjustment_is_near_bottom(adjustment: &gtk::Adjustment) -> bool {
    adjustment.value() + adjustment.page_size() + HISTORY_PREFETCH_DISTANCE >= adjustment.upper()
}

#[derive(Clone)]
struct CommitRow {
    root: gtk::Box,
    title: gtk::Label,
    tags_box: gtk::Box,
    avatar: adw::Avatar,
    author: gtk::Label,
    hash: gtk::Label,
    time: gtk::Label,
    added: gtk::Label,
    deleted: gtk::Label,
}

fn commit_row_from_item(item: &gtk::ListItem) -> Option<CommitRow> {
    let row = unsafe { item.data::<CommitRow>(COMMIT_ROW_DATA_KEY) }?;
    Some(unsafe { row.as_ref().clone() })
}

fn commit_row() -> CommitRow {
    let title = widgets::heading("");
    title.set_wrap(false);
    title.set_lines(1);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_width_chars(1);
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Fill);
    title.set_xalign(0.0);

    let avatar = adw::Avatar::builder()
        .size(18)
        .show_initials(true)
        .valign(gtk::Align::Center)
        .build();

    let author = metadata_label("");
    author.set_ellipsize(gtk::pango::EllipsizeMode::End);
    author.set_width_chars(1);
    author.set_max_width_chars(18);

    let hash = metadata_label("");
    hash.add_css_class("numeric");

    let time = metadata_label("");
    time.set_halign(gtk::Align::End);
    time.set_xalign(1.0);

    let spacer = gtk::Box::builder().hexpand(true).build();

    let added = metadata_label("");
    added.add_css_class("numeric");
    added.add_css_class("success");

    let deleted = metadata_label("");
    deleted.add_css_class("numeric");
    deleted.add_css_class("error");

    let stats = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    stats.append(&added);
    stats.append(&deleted);

    let title_spacer = gtk::Box::builder().hexpand(true).build();

    let tags_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(4)
        .valign(gtk::Align::Start)
        .build();

    let title_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    title_row.append(&title);
    title_row.append(&title_spacer);
    title_row.append(&tags_box);
    title_row.append(&stats);

    let metadata = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .hexpand(true)
        .build();
    metadata.append(&avatar);
    metadata.append(&author);
    metadata.append(&separator_label());
    metadata.append(&hash);
    metadata.append(&spacer);
    metadata.append(&time);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(10)
        .margin_end(10)
        .build();
    content.append(&title_row);
    content.append(&metadata);

    CommitRow {
        root: content,
        title,
        tags_box,
        avatar,
        author,
        hash,
        time,
        added,
        deleted,
    }
}

fn install_commit_context_menu(
    row: &gtk::Box,
    model: &gio::ListStore,
    selection: &gtk::SingleSelection,
    state: Rc<HistoryState>,
) {
    let click = gtk::GestureClick::builder().button(3).build();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed({
        let row = row.clone();
        let model = model.clone();
        let selection = selection.clone();

        move |gesture, _, x, y| {
            let hash = row.widget_name().to_string();
            if hash.is_empty() {
                return;
            }

            select_commit_hash(&model, &selection, &hash);
            if let Some(callback) = state.context_requested.borrow().clone() {
                callback(&row, hash, x, y, gesture.current_event_time());
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }
    });
    row.add_controller(click);
}

fn select_commit_hash(model: &gio::ListStore, selection: &gtk::SingleSelection, hash: &str) {
    for index in 0..model.n_items() {
        let Some(commit) = model.item(index).and_downcast::<CommitItem>() else {
            continue;
        };
        if commit.hash() == hash {
            selection.set_selected(index);
            return;
        }
    }
}

fn metadata_label(text: &str) -> gtk::Label {
    let label = widgets::muted(text);
    label.set_wrap(false);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_xalign(0.0);
    label.set_valign(gtk::Align::Center);
    label
}

fn separator_label() -> gtk::Label {
    metadata_label("·")
}

fn line_stats_text(insertions: usize, deletions: usize) -> (String, String) {
    (format!("+{insertions}"), format!("-{deletions}"))
}

fn bind_commit_row(row: &CommitRow, commit: &CommitItem) {
    row.root.set_widget_name(&commit.hash());
    let author = commit.author();
    row.avatar
        .set_custom_image(Option::<&gtk::gdk::Paintable>::None);
    row.avatar.set_widget_name("");
    row.avatar.set_text(Some(&author));
    row.avatar.set_tooltip_text(Some(&author));
    if let Some(email) = commit.author_email() {
        widgets::fetch_avatar(&row.avatar, widgets::AvatarSource::Email(email));
    }
    row.title.set_label(&commit.subject());
    row.author.set_label(&author);
    row.hash.set_label(&commit.short_hash());
    row.time.set_label(&commit.relative_time());
    let (added, deleted) = line_stats_text(commit.insertions(), commit.deletions());
    row.added.set_label(&added);
    row.deleted.set_label(&deleted);

    while let Some(child) = row.tags_box.first_child() {
        row.tags_box.remove(&child);
    }
    for tag in commit.tags() {
        let label = gtk::Label::builder()
            .label(&tag)
            .valign(gtk::Align::Center)
            .build();
        label.add_css_class("pill");
        row.tags_box.append(&label);
    }
}

glib::wrapper! {
    pub struct CommitItem(ObjectSubclass<commit_item::CommitItem>);
}

impl CommitItem {
    fn new(commit: &Commit) -> Self {
        let item: Self = glib::Object::builder().build();
        let imp = item.imp();
        *imp.hash.borrow_mut() = commit.hash.clone();
        *imp.short_hash.borrow_mut() = commit.short_hash.clone();
        *imp.subject.borrow_mut() = commit.subject.clone();
        *imp.author.borrow_mut() = commit.author.clone();
        *imp.author_email.borrow_mut() = commit.author_email.clone();
        *imp.relative_time.borrow_mut() = commit.relative_time.clone();
        imp.insertions.set(commit.insertions);
        imp.deletions.set(commit.deletions);
        *imp.tags.borrow_mut() = commit.tags.clone();
        item
    }

    fn hash(&self) -> String {
        self.imp().hash.borrow().clone()
    }

    fn short_hash(&self) -> String {
        self.imp().short_hash.borrow().clone()
    }

    fn subject(&self) -> String {
        self.imp().subject.borrow().clone()
    }

    fn author(&self) -> String {
        self.imp().author.borrow().clone()
    }

    fn author_email(&self) -> Option<String> {
        self.imp().author_email.borrow().clone()
    }

    fn relative_time(&self) -> String {
        self.imp().relative_time.borrow().clone()
    }

    fn insertions(&self) -> usize {
        self.imp().insertions.get()
    }

    fn deletions(&self) -> usize {
        self.imp().deletions.get()
    }

    fn tags(&self) -> Vec<String> {
        self.imp().tags.borrow().clone()
    }
}

mod commit_item {
    use gtk::glib;
    use gtk::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct CommitItem {
        pub hash: RefCell<String>,
        pub short_hash: RefCell<String>,
        pub subject: RefCell<String>,
        pub author: RefCell<String>,
        pub author_email: RefCell<Option<String>>,
        pub relative_time: RefCell<String>,
        pub insertions: Cell<usize>,
        pub deletions: Cell<usize>,
        pub tags: RefCell<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CommitItem {
        const NAME: &'static str = "CraicHistoryCommitItem";
        type Type = super::CommitItem;
    }

    impl ObjectImpl for CommitItem {}
}
