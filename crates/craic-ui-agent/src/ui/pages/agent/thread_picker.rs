use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ui::canvas_scroll;

type ActionCallback = Rc<dyn Fn(ThreadPickerAction)>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThreadPickerRow {
    pub thread_id: String,
    pub title: String,
    pub preview: String,
    pub model: Option<String>,
    pub updated_at_ms: i64,
    pub status: Option<String>,
    pub tags: Vec<String>,
    pub archived: bool,
    pub pinned: bool,
}

impl ThreadPickerRow {
    fn normalized(mut self) -> Option<Self> {
        self.thread_id = self.thread_id.trim().to_string();
        if self.thread_id.is_empty() {
            return None;
        }
        self.title = normalized_text(&self.title);
        if self.title.is_empty() {
            self.title = "Untitled Codex thread".to_string();
        }
        self.preview = normalized_text(&self.preview);
        self.model = self
            .model
            .as_deref()
            .map(normalized_text)
            .filter(|model| !model.is_empty());
        self.status = self
            .status
            .as_deref()
            .map(normalized_text)
            .filter(|status| !status.is_empty());

        let mut seen = HashSet::new();
        self.tags = self
            .tags
            .into_iter()
            .map(|tag| normalized_text(&tag))
            .filter(|tag| !tag.is_empty())
            .filter(|tag| seen.insert(tag.to_ascii_lowercase()))
            .take(12)
            .collect();
        Some(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ThreadPickerSort {
    #[default]
    Updated,
    Created,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ThreadPickerAction {
    SearchChanged(String),
    ArchivedChanged(bool),
    SortChanged(ThreadPickerSort),
    Resume(String),
    Fork(String),
    Rename {
        thread_id: String,
        current_name: String,
    },
    EditTags {
        thread_id: String,
        tags: Vec<String>,
    },
    Archive(String),
    Unarchive(String),
    Delete(String),
    Pin(String),
    Unpin(String),
    LoadMore,
    Cancel,
}

#[derive(Clone, Debug)]
enum ThreadPickerEntry {
    Thread(ThreadPickerRow),
    Footer {
        loading: bool,
        has_more: bool,
        error: Option<String>,
    },
}

struct PickerState {
    model: gio::ListStore,
    stack: gtk::Stack,
    empty_spinner: adw::Spinner,
    empty_title: gtk::Label,
    empty_description: gtk::Label,
    callbacks: Rc<RefCell<Vec<ActionCallback>>>,
    thread_ids: RefCell<HashSet<String>>,
    loading: Cell<bool>,
    has_more: Cell<bool>,
    load_request_pending: Cell<bool>,
    error: RefCell<Option<String>>,
}

#[derive(Clone)]
pub(crate) struct CodexThreadPicker {
    pub root: gtk::Box,
    search_entry: gtk::SearchEntry,
    archived_toggle: gtk::ToggleButton,
    sort: gtk::DropDown,
    suppress_search: Rc<Cell<bool>>,
    state: Rc<PickerState>,
}

impl CodexThreadPicker {
    pub fn new() -> Self {
        let callbacks = Rc::new(RefCell::new(Vec::<ActionCallback>::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let list = gtk::ListView::new(Some(selection), None::<gtk::ListItemFactory>);
        list.set_hexpand(true);
        list.set_vexpand(true);
        list.set_single_click_activate(true);
        list.add_css_class("navigation-sidebar");

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&list)
            .build();
        let autoscroll_marker = gtk::DrawingArea::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .hexpand(true)
            .vexpand(true)
            .can_target(false)
            .build();
        let scroller_overlay = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        scroller_overlay.set_child(Some(&scroller));
        scroller_overlay.add_overlay(&autoscroll_marker);
        canvas_scroll::install_scrolled_window_middle_autoscroll(
            &scroller,
            &autoscroll_marker,
            canvas_scroll::AutoscrollAxes::Vertical,
            "codex_thread_picker",
        );

        let empty_spinner = adw::Spinner::new();
        empty_spinner.set_size_request(40, 40);
        empty_spinner.set_visible(false);
        let empty_title = gtk::Label::builder()
            .label("No Codex threads")
            .css_classes(["title-2"])
            .justify(gtk::Justification::Center)
            .wrap(true)
            .build();
        let empty_description = gtk::Label::builder()
            .label("Try a different search or start a new conversation.")
            .css_classes(["dim-label"])
            .justify(gtk::Justification::Center)
            .wrap(true)
            .build();
        let empty_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .margin_start(24)
            .margin_end(24)
            .build();
        empty_content.append(&empty_spinner);
        empty_content.append(&empty_title);
        empty_content.append(&empty_description);

        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        stack.add_named(&scroller_overlay, Some("threads"));
        stack.add_named(&empty_content, Some("empty"));
        stack.set_visible_child_name("empty");

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search Codex threads")
            .search_delay(200)
            .hexpand(true)
            .build();
        let cancel_button = gtk::Button::builder().label("Cancel").build();
        cancel_button.add_css_class("flat");
        let archived_toggle = gtk::ToggleButton::builder()
            .label("Archived")
            .tooltip_text("Show archived Codex threads")
            .build();
        let sort = gtk::DropDown::from_strings(&["Recently Updated", "Recently Created"]);
        sort.set_tooltip_text(Some("Choose how Codex threads are ordered"));
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        header.append(&search_entry);
        header.append(&archived_toggle);
        header.append(&cancel_button);

        let filters = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        filters.append(
            &gtk::Label::builder()
                .label("Sort")
                .css_classes(["dim-label", "caption"])
                .build(),
        );
        filters.append(&sort);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&header);
        root.append(&filters);
        root.append(&stack);

        let state = Rc::new(PickerState {
            model,
            stack,
            empty_spinner,
            empty_title,
            empty_description,
            callbacks,
            thread_ids: RefCell::new(HashSet::new()),
            loading: Cell::new(false),
            has_more: Cell::new(false),
            load_request_pending: Cell::new(false),
            error: RefCell::new(None),
        });
        list.set_factory(Some(&thread_factory(Rc::downgrade(&state))));
        let picker = Self {
            root,
            search_entry,
            archived_toggle,
            sort,
            suppress_search: Rc::new(Cell::new(false)),
            state,
        };
        picker.connect_controls(&list, &cancel_button);
        picker.update_view();
        picker
    }

    pub fn connect_action<F>(&self, callback: F)
    where
        F: Fn(ThreadPickerAction) + 'static,
    {
        self.state.callbacks.borrow_mut().push(Rc::new(callback));
    }

    pub fn set_rows(&self, rows: Vec<ThreadPickerRow>, has_more: bool) {
        self.state.model.remove_all();
        self.state.thread_ids.borrow_mut().clear();
        self.append_normalized_rows(rows);
        self.finish_page_update(has_more);
    }

    pub fn append_rows(&self, rows: Vec<ThreadPickerRow>, has_more: bool) {
        self.append_normalized_rows(rows);
        self.finish_page_update(has_more);
    }

    pub fn set_loading(&self, loading: bool) {
        self.state.loading.set(loading);
        if !loading {
            self.state.load_request_pending.set(false);
        }
        self.update_view();
    }

    pub fn set_error(&self, message: Option<&str>) {
        self.state.error.replace(
            message
                .map(normalized_text)
                .filter(|message| !message.is_empty()),
        );
        self.state.loading.set(false);
        self.state.load_request_pending.set(false);
        self.update_view();
    }

    pub fn set_query(&self, query: &str) {
        let query = query.trim();
        if self.search_entry.text().as_str() == query {
            return;
        }
        self.suppress_search.set(true);
        self.search_entry.set_text(query);
        self.suppress_search.set(false);
    }

    pub fn query(&self) -> String {
        self.search_entry.text().trim().to_string()
    }

    pub fn archived_only(&self) -> bool {
        self.archived_toggle.is_active()
    }

    pub fn sort(&self) -> ThreadPickerSort {
        match self.sort.selected() {
            1 => ThreadPickerSort::Created,
            _ => ThreadPickerSort::Updated,
        }
    }

    pub fn focus_search(&self) {
        self.search_entry.grab_focus();
    }

    fn append_normalized_rows(&self, rows: Vec<ThreadPickerRow>) {
        remove_footer(&self.state);
        let mut thread_ids = self.state.thread_ids.borrow_mut();
        let mut invalid_count = 0usize;
        for row in rows {
            let Some(row) = row.normalized() else {
                invalid_count += 1;
                continue;
            };
            if !thread_ids.insert(row.thread_id.clone()) {
                continue;
            }
            self.state
                .model
                .append(&glib::BoxedAnyObject::new(ThreadPickerEntry::Thread(row)));
        }
        if invalid_count > 0 {
            log::warn!("Codex thread picker ignored {invalid_count} rows without thread IDs");
        }
    }

    fn finish_page_update(&self, has_more: bool) {
        self.state.has_more.set(has_more);
        self.state.loading.set(false);
        self.state.load_request_pending.set(false);
        self.state.error.borrow_mut().take();
        self.update_view();
    }

    fn connect_controls(&self, list: &gtk::ListView, cancel_button: &gtk::Button) {
        self.search_entry.connect_search_changed({
            let suppress_search = self.suppress_search.clone();
            let state = Rc::downgrade(&self.state);

            move |entry| {
                if suppress_search.get() {
                    return;
                }
                let Some(state) = state.upgrade() else {
                    return;
                };
                let query = entry.text().trim().to_string();
                reset_for_filter_change(&state);
                update_picker_view(&state, &query);
                emit_to(&state.callbacks, ThreadPickerAction::SearchChanged(query));
            }
        });
        self.archived_toggle.connect_toggled({
            let state = Rc::downgrade(&self.state);
            let search_entry = self.search_entry.downgrade();

            move |toggle| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                reset_for_filter_change(&state);
                let query = search_entry
                    .upgrade()
                    .map(|entry| entry.text().trim().to_string())
                    .unwrap_or_default();
                update_picker_view(&state, &query);
                emit_to(
                    &state.callbacks,
                    ThreadPickerAction::ArchivedChanged(toggle.is_active()),
                );
            }
        });
        self.sort.connect_selected_notify({
            let state = Rc::downgrade(&self.state);
            let search_entry = self.search_entry.downgrade();

            move |sort| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                reset_for_filter_change(&state);
                let query = search_entry
                    .upgrade()
                    .map(|entry| entry.text().trim().to_string())
                    .unwrap_or_default();
                update_picker_view(&state, &query);
                emit_to(
                    &state.callbacks,
                    ThreadPickerAction::SortChanged(match sort.selected() {
                        1 => ThreadPickerSort::Created,
                        _ => ThreadPickerSort::Updated,
                    }),
                );
            }
        });
        cancel_button.connect_clicked({
            let state = Rc::downgrade(&self.state);
            move |_| {
                if let Some(state) = state.upgrade() {
                    emit_to(&state.callbacks, ThreadPickerAction::Cancel);
                }
            }
        });
        list.connect_activate({
            let state = Rc::downgrade(&self.state);

            move |_, position| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let Some(row) = thread_at(&state, position) else {
                    return;
                };
                emit_to(
                    &state.callbacks,
                    if row.archived {
                        ThreadPickerAction::Unarchive(row.thread_id)
                    } else {
                        ThreadPickerAction::Resume(row.thread_id)
                    },
                );
            }
        });
    }

    fn update_view(&self) {
        update_picker_view(&self.state, &self.query());
    }
}

fn thread_at(state: &PickerState, position: u32) -> Option<ThreadPickerRow> {
    let item = state
        .model
        .item(position)?
        .downcast::<glib::BoxedAnyObject>()
        .ok()?;
    match item.borrow::<ThreadPickerEntry>().clone() {
        ThreadPickerEntry::Thread(row) => Some(row),
        ThreadPickerEntry::Footer { .. } => None,
    }
}

fn reset_for_filter_change(state: &PickerState) {
    state.model.remove_all();
    state.thread_ids.borrow_mut().clear();
    state.error.borrow_mut().take();
    state.has_more.set(false);
    state.load_request_pending.set(false);
}

fn update_picker_view(state: &Rc<PickerState>, query: &str) {
    sync_footer(state);
    let count = state.thread_ids.borrow().len();
    let loading = state.loading.get() || state.load_request_pending.get();
    let has_rows = count > 0;
    state
        .stack
        .set_visible_child_name(if has_rows { "threads" } else { "empty" });
    state.empty_spinner.set_visible(!has_rows && loading);

    if let Some(error) = state.error.borrow().as_deref() {
        state.empty_title.set_label("Could not load threads");
        state.empty_description.set_label(error);
        state.empty_description.set_visible(true);
    } else if loading {
        state.empty_title.set_label("Loading threads…");
        state.empty_description.set_visible(false);
    } else if query.is_empty() {
        state.empty_title.set_label("No Codex threads");
        state
            .empty_description
            .set_label("Start a new conversation to create one.");
        state.empty_description.set_visible(true);
    } else {
        state.empty_title.set_label("No matching threads");
        state.empty_description.set_label("Try a different search.");
        state.empty_description.set_visible(true);
    }
}

fn thread_factory(state: std::rc::Weak<PickerState>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(row) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            item.set_child(None::<&gtk::Widget>);
            return;
        };
        let entry = row.borrow::<ThreadPickerEntry>().clone();
        let Some(state) = state.upgrade() else {
            item.set_child(None::<&gtk::Widget>);
            return;
        };
        let child = match entry {
            ThreadPickerEntry::Thread(row) => thread_row(&row, &state.callbacks),
            ThreadPickerEntry::Footer {
                loading,
                has_more,
                error,
            } => thread_footer(&state, loading, has_more, error.as_deref()),
        };
        item.set_child(Some(&child));
    });
    factory.connect_unbind(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            item.set_child(None::<&gtk::Widget>);
        }
    });
    factory
}

fn thread_footer(
    state: &Rc<PickerState>,
    loading: bool,
    has_more: bool,
    error: Option<&str>,
) -> gtk::Widget {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .margin_bottom(16)
        .margin_start(12)
        .margin_end(12)
        .build();
    if loading {
        let spinner = adw::Spinner::new();
        spinner.set_size_request(20, 20);
        content.append(&spinner);
    }
    if let Some(error) = error {
        content.append(
            &gtk::Label::builder()
                .label(format!("Could not load more threads: {error}"))
                .css_classes(["dim-label"])
                .wrap(true)
                .justify(gtk::Justification::Center)
                .tooltip_text(error)
                .build(),
        );
    }
    if has_more && !loading {
        let button = gtk::Button::with_label(if error.is_some() {
            "Retry"
        } else {
            "Load More"
        });
        button.connect_clicked({
            let state = Rc::downgrade(state);
            move |_| {
                if let Some(state) = state.upgrade() {
                    request_more(&state);
                }
            }
        });
        content.append(&button);
    }
    content.upcast()
}

fn request_more(state: &Rc<PickerState>) {
    if state.loading.get() || !state.has_more.get() || state.load_request_pending.replace(true) {
        return;
    }
    sync_footer(state);
    emit_to(&state.callbacks, ThreadPickerAction::LoadMore);
}

fn remove_footer(state: &PickerState) {
    let count = state.model.n_items();
    if count == 0 {
        return;
    }
    let is_footer = state
        .model
        .item(count - 1)
        .and_downcast::<glib::BoxedAnyObject>()
        .is_some_and(|item| {
            matches!(
                &*item.borrow::<ThreadPickerEntry>(),
                ThreadPickerEntry::Footer { .. }
            )
        });
    if is_footer {
        state.model.remove(count - 1);
    }
}

fn sync_footer(state: &Rc<PickerState>) {
    remove_footer(state);
    if state.thread_ids.borrow().is_empty() {
        return;
    }
    let loading = state.loading.get() || state.load_request_pending.get();
    let has_more = state.has_more.get();
    let error = state.error.borrow().clone();
    if loading || has_more || error.is_some() {
        state
            .model
            .append(&glib::BoxedAnyObject::new(ThreadPickerEntry::Footer {
                loading,
                has_more,
                error,
            }));
    }
}

fn thread_row(row: &ThreadPickerRow, callbacks: &Rc<RefCell<Vec<ActionCallback>>>) -> gtk::Widget {
    let title = gtk::Label::builder()
        .label(&row.title)
        .css_classes(["heading"])
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();
    let time = gtk::Label::builder()
        .label(relative_time(row.updated_at_ms))
        .css_classes(["dim-label", "caption"])
        .build();
    let heading = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    if row.pinned {
        let pin = gtk::Image::from_icon_name("view-pin-symbolic");
        pin.set_tooltip_text(Some("Pinned"));
        heading.append(&pin);
    }
    heading.append(&title);
    heading.append(&time);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&heading);
    if !row.preview.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(&row.preview)
                .css_classes(["dim-label"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .lines(2)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .build(),
        );
    }

    let metadata = thread_metadata(row);
    if !metadata.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(metadata.join("  •  "))
                .css_classes(["dim-label", "caption"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
    }
    if !row.tags.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(format!("Tags: {}", row.tags.join(", ")))
                .css_classes(["dim-label", "caption"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
    }

    let resume_button = gtk::Button::builder()
        .label(if row.archived { "Unarchive" } else { "Resume" })
        .valign(gtk::Align::Center)
        .build();
    resume_button.add_css_class("suggested-action");
    resume_button.connect_clicked({
        let callbacks = callbacks.clone();
        let thread_id = row.thread_id.clone();

        let archived = row.archived;

        move |_| {
            emit_to(
                &callbacks,
                if archived {
                    ThreadPickerAction::Unarchive(thread_id.clone())
                } else {
                    ThreadPickerAction::Resume(thread_id.clone())
                },
            )
        }
    });
    let menu_button = thread_menu_button(row, callbacks);
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    actions.append(&resume_button);
    actions.append(&menu_button);

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    row_box.append(&content);
    row_box.append(&actions);
    let clamp = adw::Clamp::builder()
        .maximum_size(900)
        .tightening_threshold(620)
        .margin_top(3)
        .margin_bottom(3)
        .margin_start(8)
        .margin_end(8)
        .child(&row_box)
        .build();
    clamp.upcast()
}

fn thread_menu_button(
    row: &ThreadPickerRow,
    callbacks: &Rc<RefCell<Vec<ActionCallback>>>,
) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();
    let entries = [
        (
            "fork",
            "Fork",
            ThreadPickerAction::Fork(row.thread_id.clone()),
        ),
        (
            "rename",
            "Rename…",
            ThreadPickerAction::Rename {
                thread_id: row.thread_id.clone(),
                current_name: row.title.clone(),
            },
        ),
        (
            "tags",
            "Edit Tags…",
            ThreadPickerAction::EditTags {
                thread_id: row.thread_id.clone(),
                tags: row.tags.clone(),
            },
        ),
        if row.pinned {
            (
                "pin",
                "Unpin",
                ThreadPickerAction::Unpin(row.thread_id.clone()),
            )
        } else {
            ("pin", "Pin", ThreadPickerAction::Pin(row.thread_id.clone()))
        },
        if row.archived {
            (
                "archive",
                "Unarchive",
                ThreadPickerAction::Unarchive(row.thread_id.clone()),
            )
        } else {
            (
                "archive",
                "Archive",
                ThreadPickerAction::Archive(row.thread_id.clone()),
            )
        },
        (
            "delete",
            "Delete",
            ThreadPickerAction::Delete(row.thread_id.clone()),
        ),
    ];
    for (name, label, action) in entries {
        menu.append(Some(label), Some(&format!("thread.{name}")));
        let simple_action = gio::SimpleAction::new(name, None);
        simple_action.connect_activate({
            let callbacks = callbacks.clone();

            move |_, _| emit_to(&callbacks, action.clone())
        });
        actions.add_action(&simple_action);
    }

    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .menu_model(&menu)
        .tooltip_text("Thread actions")
        .valign(gtk::Align::Center)
        .build();
    button.add_css_class("flat");
    button.insert_action_group("thread", Some(&actions));
    button
}

fn thread_metadata(row: &ThreadPickerRow) -> Vec<String> {
    let mut metadata = Vec::new();
    if let Some(model) = row.model.as_ref() {
        metadata.push(format!("Provider: {model}"));
    }
    if let Some(status) = row.status.as_ref() {
        metadata.push(status.clone());
    }
    if row.archived {
        metadata.push("Archived".to_string());
    }
    metadata
}

fn emit_to(callbacks: &Rc<RefCell<Vec<ActionCallback>>>, action: ThreadPickerAction) {
    for callback in callbacks.borrow().iter() {
        callback(action.clone());
    }
}

fn normalized_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn relative_time(updated_at_ms: i64) -> String {
    if updated_at_ms <= 0 {
        return "Unknown time".to_string();
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(updated_at_ms);
    let age_seconds = now_ms.saturating_sub(updated_at_ms) / 1_000;
    match age_seconds {
        0..=59 => "Just now".to_string(),
        60..=3_599 => format!("{}m ago", age_seconds / 60),
        3_600..=86_399 => format!("{}h ago", age_seconds / 3_600),
        86_400..=2_591_999 => format!("{}d ago", age_seconds / 86_400),
        _ => format!("{}mo ago", age_seconds / 2_592_000),
    }
}
